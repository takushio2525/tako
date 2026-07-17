//! setup — `tako setup` の状態管理とアップデート追従（Issue #94）
//!
//! - config.yaml（`~/Library/Application Support/tako/orchestrator/config.yaml`）の
//!   setup セクションのスキーマと読み書き（CLI の自動適用フローは tako-cli 側）
//! - バイナリ埋め込みの setup changelog（`resources/setup/changes.yaml`）のパースと、
//!   適用済みリビジョンとの突き合わせによる未適用変更の検出
//!
//! 照会結果（[`changes_status`]）は CLI `tako setup --changes` と
//! MCP `tako_setup_changes` の両方から使われる（二重実装を作らない。#83 の教訓）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// バイナリ埋め込みの setup changelog
pub const CHANGES_YAML: &str = include_str!("../../../resources/setup/changes.yaml");

/// setup が採用した値の出所。CLI 表示のラベルを共通化する（Issue #262）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupValueSource {
    Detected,
    Previous,
    Default,
    Input,
}

impl SetupValueSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::Previous => "previous",
            Self::Default => "default",
            Self::Input => "input",
        }
    }
}

/// 最終サマリに表示する setup の 1 変更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPlanChange {
    pub key: String,
    pub before: Option<String>,
    pub after: String,
    pub source: SetupValueSource,
}

/// setup の値解決と書き込みを分離する変更計画（Issue #262 方針 C/D）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupPlan {
    changes: Vec<SetupPlanChange>,
}

impl SetupPlan {
    pub fn push_if_changed(
        &mut self,
        key: impl Into<String>,
        before: Option<&str>,
        after: impl Into<String>,
        source: SetupValueSource,
    ) {
        let after = after.into();
        if before == Some(after.as_str()) {
            return;
        }
        self.changes.push(SetupPlanChange {
            key: key.into(),
            before: before.map(str::to_string),
            after,
            source,
        });
    }

    pub fn changes(&self) -> &[SetupPlanChange] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn render_diff(&self) -> String {
        self.changes
            .iter()
            .map(|change| {
                format!(
                    "  - {}: {} -> {} [{}]",
                    change.key,
                    change.before.as_deref().unwrap_or("(未設定)"),
                    change.after,
                    change.source.label()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 検出値、前回値、既定値の優先順で setup 値を解決する。
/// 検出値と前回値が違う場合は previous を残し、呼び出し側が差異を通知できるようにする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSetupValue {
    pub value: String,
    pub source: SetupValueSource,
    pub previous: Option<String>,
}

pub fn resolve_setup_value(
    detected: Option<&str>,
    previous: Option<&str>,
    default: Option<&str>,
) -> Option<ResolvedSetupValue> {
    if let Some(value) = detected {
        return Some(ResolvedSetupValue {
            value: value.to_string(),
            source: SetupValueSource::Detected,
            previous: previous
                .filter(|previous| *previous != value)
                .map(str::to_string),
        });
    }
    if let Some(value) = previous {
        return Some(ResolvedSetupValue {
            value: value.to_string(),
            source: SetupValueSource::Previous,
            previous: None,
        });
    }
    default.map(|value| ResolvedSetupValue {
        value: value.to_string(),
        source: SetupValueSource::Default,
        previous: None,
    })
}

/// CLI / dispatch / MCP から非対話 setup へ渡す全回答（Issue #262 要件 E）。
/// 省略項目は detected → previous → default の順で解決する。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupAnswers {
    pub selected_agent: Option<String>,
    pub provider_plans: BTreeMap<String, String>,
    /// 選択 agent のグローバル指示ファイルへ書く完全な Markdown。
    /// 省略時は既存を維持し、未作成なら同梱既定値を使う。
    pub instruction_content: Option<String>,
    /// profiles/default.yaml の完全な内容。省略時は既存を維持し、未作成なら推奨生成する。
    pub profile: Option<crate::orchestrator::Profile>,
    /// projects.yaml の全プロジェクト。明示時だけ既存一覧を置き換える。
    pub projects: Option<BTreeMap<String, crate::orchestrator::ProjectEntry>>,
    pub orchestrator: Option<SetupOrchestratorAnswers>,
    pub sleep_guard: Option<SetupSleepGuardAnswers>,
    /// setup 完了後に起動するエージェント CLI（Issue #295）。
    /// "claude" / "codex" / "agy" = その場で対話起動、"none" = 起動しない。
    /// 省略時は TTY があれば対話で選択、なければ "none"
    pub launch_agent: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupOrchestratorAnswers {
    pub auto_close: Option<bool>,
    pub auto_push: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupSleepGuardAnswers {
    /// off / on / while-agents-running
    pub mode: Option<String>,
    /// ac-only / always
    pub power: Option<String>,
}

impl SetupAnswers {
    pub fn from_json(input: &str) -> Result<Self, String> {
        let answers: Self =
            serde_json::from_str(input).map_err(|e| format!("setup answers JSON が不正: {e}"))?;
        answers.validate()?;
        Ok(answers)
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(agent) = self.selected_agent.as_deref() {
            if !matches!(agent, "claude" | "codex" | "agy") {
                return Err(format!(
                    "selected_agent は claude / codex / agy のいずれかです: {agent}"
                ));
            }
        }
        for provider in self.provider_plans.keys() {
            if !matches!(provider.as_str(), "claude" | "gpt" | "google") {
                return Err(format!(
                    "provider_plans のキーは claude / gpt / google のいずれかです: {provider}"
                ));
            }
        }
        if self
            .instruction_content
            .as_deref()
            .is_some_and(|content| content.trim().is_empty())
        {
            return Err("instruction_content は空にできません".to_string());
        }
        if let Some(profile) = &self.profile {
            profile.resolve_master_agent()?;
            profile.resolve_worker_agent(None)?;
        }
        if let Some(projects) = &self.projects {
            for (key, project) in projects {
                if key.trim().is_empty() {
                    return Err("projects のキーは空にできません".to_string());
                }
                if project.cwd.trim().is_empty() {
                    return Err(format!("projects.{key}.cwd は空にできません"));
                }
            }
        }
        if let Some(agent) = self.launch_agent.as_deref() {
            if !matches!(agent, "claude" | "codex" | "agy" | "none") {
                return Err(format!(
                    "launch_agent は claude / codex / agy / none のいずれかです: {agent}"
                ));
            }
        }
        if let Some(sleep) = &self.sleep_guard {
            if let Some(mode) = sleep.mode.as_deref() {
                if !matches!(mode, "off" | "on" | "while-agents-running") {
                    return Err(format!(
                        "sleep_guard.mode は off / on / while-agents-running のいずれかです: {mode}"
                    ));
                }
            }
            if let Some(power) = sleep.power.as_deref() {
                if !matches!(power, "ac-only" | "always") {
                    return Err(format!(
                        "sleep_guard.power は ac-only / always のいずれかです: {power}"
                    ));
                }
            }
        }
        Ok(())
    }
}

// --- config.yaml のスキーマ ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupConfig {
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub setup: SetupState,
    /// エージェント共通ルール同期の設定（Issue #136）
    #[serde(
        default,
        skip_serializing_if = "crate::agents_sync::AgentsSyncConfig::is_default"
    )]
    pub agents_sync: crate::agents_sync::AgentsSyncConfig,
    /// worker spawn のレイアウト設定（Issue #165）
    #[serde(default, skip_serializing_if = "SpawnLayoutSection::is_default")]
    pub spawn_layout: SpawnLayoutSection,
    /// タブ/ペインの × ボタン close 時の確認ダイアログ（Issue #172。既定 true）
    #[serde(default = "default_true")]
    pub confirm_close: bool,
    /// master の ctx% 閾値（#193。この値を超えると MASTER_CTX_HIGH 通知。既定 60）
    #[serde(default = "default_ctx_threshold")]
    pub ctx_threshold: u32,
}

/// config.yaml の spawn_layout セクション（Issue #165）。
/// 未設定キーは既定値（master-reserved / 0.5 / grid）に解決される。
/// 不正値は spawn を止めないよう警告なしで既定へフォールバックする
/// （検証つきの変更経路は CLI `tako orchestrator layout` / MCP `tako_orchestrator_layout`）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpawnLayoutSection {
    /// 配置ポリシー（"master-reserved" / "legacy"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    /// master-reserved 時に master 側へ残す取り分（0.1〜0.9）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_ratio: Option<f32>,
    /// worker 領域内の配置アルゴリズム（"grid" / "spiral"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
}

impl SpawnLayoutSection {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// tako-core のレイアウト設定へ解決する。不正値・未設定は既定値へ
    pub fn resolve(&self) -> tako_core::SpawnLayoutConfig {
        let defaults = tako_core::SpawnLayoutConfig::default();
        tako_core::SpawnLayoutConfig {
            policy: self
                .policy
                .as_deref()
                .and_then(|s| tako_core::SpawnLayoutPolicy::parse(s).ok())
                .unwrap_or(defaults.policy),
            master_ratio: self
                .master_ratio
                .map(tako_core::spawn_layout::clamp_master_ratio)
                .unwrap_or(defaults.master_ratio),
            algorithm: self
                .algorithm
                .as_deref()
                .and_then(|s| tako_core::WorkerLayoutAlgorithm::parse(s).ok())
                .unwrap_or(defaults.algorithm),
        }
    }
}

/// × ボタン close の確認ダイアログ有効状態を config.yaml から取得する（Issue #172）
pub fn confirm_close_enabled() -> bool {
    load_config().map(|c| c.confirm_close).unwrap_or(true)
}

/// spawn レイアウト設定を config.yaml から解決する（Issue #165）。
/// 読み取り失敗（$HOME 無し・パース不能）は既定値へフォールバックし、spawn を止めない
pub fn spawn_layout_config() -> tako_core::SpawnLayoutConfig {
    load_config()
        .map(|c| c.spawn_layout.resolve())
        .unwrap_or_default()
}

/// config.yaml の orchestrator セクション。
/// モデル・effort は master が一切参照しないため、ここには置かない（Issue #27 で廃止。
/// 起動設定の正は profiles/*.yaml。旧ファイルに残る master_model 等のキーは無視される）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    #[serde(default = "default_true")]
    pub auto_close: bool,
    #[serde(default = "default_true")]
    pub auto_push: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupState {
    #[serde(default)]
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// 最後に適用した setup リビジョン（Issue #94）。
    /// 0 = 追従機構の導入前に setup した / 未実施（全変更が未適用扱いになる）
    #[serde(default)]
    pub applied_revision: u32,
    /// 最後に setup を完了したときの tako バージョン（診断表示用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_version: Option<String>,
    /// 最後の setup で選択したエージェント CLI（Issue #226）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_agent: Option<String>,
    /// setup が自動検出、前回値、既定値、または answers で解決したプロバイダ別プラン。
    /// キーは claude / gpt / google。token やアカウント識別子は保存しない。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_plans: BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

fn default_ctx_threshold() -> u32 {
    60
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            auto_close: true,
            auto_push: true,
        }
    }
}

/// config.yaml のパス（orchestrator 設定ディレクトリ配下）
pub fn config_yaml_path() -> Result<PathBuf, String> {
    crate::orchestrator::config_dir()
        .map(|d| d.join("config.yaml"))
        .ok_or_else(|| "ホームディレクトリが取得できない（$HOME 未設定）".into())
}

pub fn load_config() -> Result<SetupConfig, String> {
    let path = config_yaml_path()?;
    load_config_from(&path)
}

/// パス指定版 load（テスト用に公開）。
/// 不在は default、パース失敗は Err（default に丸めて後続 save で消さない。#169）
pub fn load_config_from(path: &Path) -> Result<SetupConfig, String> {
    if !path.is_file() {
        return Ok(SetupConfig::default());
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("config.yaml の読み取りに失敗: {e}"))?;
    serde_yaml::from_str(&content).map_err(|e| format!("config.yaml のパースに失敗: {e}"))
}

/// 保存（アトミック書き込み + 世代バックアップ。#169）。
/// 注意: `load_config()` → 変更 → `save_config()` の素朴な組み合わせは並行更新を
/// 巻き戻す。更新は [`mutate_config`] を使うこと
pub fn save_config(config: &SetupConfig) -> Result<(), String> {
    let path = config_yaml_path()?;
    let content =
        serde_yaml::to_string(config).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
    crate::config_io::atomic_write_with_backup(&path, &content)
}

/// ロック付き read-modify-write（#169）。
/// パースに失敗した既存 config.yaml は上書きせず Err で中断する
pub fn mutate_config<R>(f: impl FnOnce(&mut SetupConfig) -> R) -> Result<R, String> {
    let path = config_yaml_path()?;
    mutate_config_at(&path, f)
}

/// パス指定版 mutate（テスト用に公開）
pub fn mutate_config_at<R>(
    path: &Path,
    f: impl FnOnce(&mut SetupConfig) -> R,
) -> Result<R, String> {
    let _lock = crate::config_io::lock_exclusive(path)?;
    let mut config = load_config_from(path)?;
    let result = f(&mut config);
    let content =
        serde_yaml::to_string(&config).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
    crate::config_io::atomic_write_with_backup(path, &content)?;
    Ok(result)
}

// --- グローバル指示ファイルと同梱推奨ルールの項目レベル比較（Issue #322） ---

/// 同梱推奨ルールのセクション（バイナリ埋め込み）。
/// (setup ディレクトリへの展開相対パス, 内容)。tako-cli のテンプレート展開と
/// 項目レベル比較の両方がこれを正として使う（二重定義を作らない）
pub const RECOMMENDED_SECTIONS: &[(&str, &str)] = &[
    (
        "templates/sections/00-language.md",
        include_str!("../../../resources/setup/templates/sections/00-language.md"),
    ),
    (
        "templates/sections/01-interaction-style.md",
        include_str!("../../../resources/setup/templates/sections/01-interaction-style.md"),
    ),
    (
        "templates/sections/02-git-workflow.md",
        include_str!("../../../resources/setup/templates/sections/02-git-workflow.md"),
    ),
    (
        "templates/sections/03-code-quality.md",
        include_str!("../../../resources/setup/templates/sections/03-code-quality.md"),
    ),
    (
        "templates/sections/04-safety-rules.md",
        include_str!("../../../resources/setup/templates/sections/04-safety-rules.md"),
    ),
    (
        "templates/sections/05-proposal-quality.md",
        include_str!("../../../resources/setup/templates/sections/05-proposal-quality.md"),
    ),
    (
        "templates/sections/06-completion-verification.md",
        include_str!("../../../resources/setup/templates/sections/06-completion-verification.md"),
    ),
];

/// 同梱の既定グローバル指示ファイル（未作成時に setup が書く内容）
pub const INSTRUCTIONS_DEFAULT: &str =
    include_str!("../../../resources/setup/templates/instructions-default.md");

/// 推奨ルール 1 項目内の必須概念。キーワード（小文字化済み）のいずれかが
/// 指示ファイル本文に含まれれば、その概念は記述済みとみなす
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageConcept {
    pub name: String,
    pub keywords: Vec<String>,
}

/// 推奨ルールの 1 項目（sections/*.md の 1 ファイル）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageSection {
    pub title: String,
    pub concepts: Vec<CoverageConcept>,
}

/// セクション md の coverage メタ行（`<!-- coverage: 概念名 = kw | kw -->`）をパースする。
/// メタ行はセクション md 自身が持つ（比較の正をコードに複製しない）
fn parse_coverage_section(md: &str) -> Option<CoverageSection> {
    let mut title: Option<String> = None;
    let mut concepts = Vec::new();
    for line in md.lines() {
        let line = line.trim();
        if title.is_none() {
            if let Some(t) = line.strip_prefix("# ") {
                title = Some(t.trim().to_string());
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("<!-- coverage:") else {
            continue;
        };
        let Some(body) = rest.strip_suffix("-->") else {
            continue;
        };
        let Some((name, keywords)) = body.split_once('=') else {
            continue;
        };
        let keywords: Vec<String> = keywords
            .split('|')
            .map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty())
            .collect();
        let name = name.trim();
        if keywords.is_empty() || name.is_empty() {
            continue;
        }
        concepts.push(CoverageConcept {
            name: name.to_string(),
            keywords,
        });
    }
    let title = title?;
    if concepts.is_empty() {
        return None;
    }
    Some(CoverageSection { title, concepts })
}

/// 同梱推奨ルール全項目の比較定義
pub fn recommended_coverage_sections() -> Vec<CoverageSection> {
    RECOMMENDED_SECTIONS
        .iter()
        .filter_map(|(_, md)| parse_coverage_section(md))
        .collect()
}

/// 項目レベル比較の結果。CLI 表示と setup-context.yaml（setup エージェントの裏取り用）で共有する
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionCoverage {
    /// (項目タイトル, 記述が見当たらない概念名。空 = 全概念カバー)
    pub sections: Vec<(String, Vec<String>)>,
}

impl InstructionCoverage {
    /// 全項目・全概念がカバーされている（= 同梱推奨ルールとの差分なし）
    pub fn is_full(&self) -> bool {
        self.sections.iter().all(|(_, missing)| missing.is_empty())
    }

    /// 「項目タイトル: 概念1・概念2」形式の不足一覧（setup-context.yaml 用）
    pub fn missing_summaries(&self) -> Vec<String> {
        self.sections
            .iter()
            .filter(|(_, missing)| !missing.is_empty())
            .map(|(title, missing)| format!("{title}: {}", missing.join("・")))
            .collect()
    }

    /// CLI / MCP 出力共通の表示行。
    /// 全項目カバー時は「差分なし」を明示する（Issue #322 受け入れ条件 1）
    pub fn render_lines(&self) -> Vec<String> {
        if self.is_full() {
            return vec![format!(
                "同梱推奨ルールとの比較: 全 {} 項目をカバーしています（差分なし）",
                self.sections.len()
            )];
        }
        let covered: Vec<&str> = self
            .sections
            .iter()
            .filter(|(_, missing)| missing.is_empty())
            .map(|(title, _)| title.as_str())
            .collect();
        let mut lines = vec![format!(
            "同梱推奨ルールとの比較（全 {} 項目）:",
            self.sections.len()
        )];
        if !covered.is_empty() {
            lines.push(format!("  [OK] {}", covered.join(" / ")));
        }
        for (title, missing) in &self.sections {
            if missing.is_empty() {
                continue;
            }
            lines.push(format!("  [不足の可能性] {title}: {}", missing.join("・")));
        }
        lines
    }
}

/// 既存のグローバル指示ファイル本文を同梱推奨ルールと項目レベルで比較する。
/// 判定はキーワードの部分一致（大文字小文字無視）で、確定ではなく「不足の可能性」を示す
pub fn compare_instruction_coverage(existing: &str) -> InstructionCoverage {
    let haystack = existing.to_lowercase();
    let sections = recommended_coverage_sections()
        .into_iter()
        .map(|section| {
            let missing = section
                .concepts
                .iter()
                .filter(|c| !c.keywords.iter().any(|k| haystack.contains(k.as_str())))
                .map(|c| c.name.clone())
                .collect();
            (section.title, missing)
        })
        .collect();
    InstructionCoverage { sections }
}

// --- setup changelog（アップデート追従） ---

/// 変更の適用方法の区分
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// tako setup の再実行だけで追従が完了する変更（通知のみ）
    Auto,
    /// ユーザー所有ファイルに関わり、setup エージェントが対話で確認してから適用する変更
    Guided,
}

/// setup changelog の 1 エントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupChange {
    /// 単調増加のリビジョン番号
    pub revision: u32,
    /// この変更が最初に入る tako リリースのバージョン
    pub version: String,
    pub date: String,
    pub kind: ChangeKind,
    pub title: String,
    /// setup エージェント向けの詳細（何が変わったか・確認/適用手順）
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct ChangesFile {
    changes: Vec<SetupChange>,
}

fn parse_changes(yaml: &str) -> Result<Vec<SetupChange>, String> {
    let file: ChangesFile =
        serde_yaml::from_str(yaml).map_err(|e| format!("changes.yaml のパースに失敗: {e}"))?;
    Ok(file.changes)
}

/// 埋め込み changelog の全エントリ（revision 昇順はファイル記載順に依存。テストで検証）
pub fn all_changes() -> Result<Vec<SetupChange>, String> {
    parse_changes(CHANGES_YAML)
}

/// 現在の setup リビジョン（changelog の最大 revision）
pub fn current_revision() -> Result<u32, String> {
    Ok(all_changes()?.iter().map(|c| c.revision).max().unwrap_or(0))
}

/// 適用済みリビジョンより新しい未適用エントリを返す
pub fn pending_changes(applied_revision: u32) -> Result<Vec<SetupChange>, String> {
    Ok(all_changes()?
        .into_iter()
        .filter(|c| c.revision > applied_revision)
        .collect())
}

/// アップデート追従の照会結果（CLI `--json` / MCP `tako_setup_changes` 共通のペイロード）
pub fn changes_status() -> Result<Value, String> {
    let config = load_config()?;
    let current = current_revision()?;
    let applied = config.setup.applied_revision;
    let pending = pending_changes(applied)?;
    Ok(json!({
        "current_revision": current,
        "applied_revision": applied,
        "applied_version": config.setup.applied_version,
        "setup_completed": config.setup.completed,
        "selected_agent": config.setup.selected_agent,
        "provider_plans": config.setup.provider_plans,
        "up_to_date": pending.is_empty(),
        "pending": pending,
    }))
}

/// 未適用エントリから pending-changes.md（setup エージェントが Read する追従指示書）を
/// 生成する。auto は「概要を伝えるだけでよい」、guided は「対話で確認・適用する」を明示する
pub fn render_pending_markdown(pending: &[SetupChange], applied_revision: u32) -> String {
    let mut md = String::new();
    md.push_str("# 前回セットアップ以降のアップデート変更（未適用）\n\n");
    md.push_str(&format!(
        "前回適用リビジョン: {applied_revision} → 現在: {}。\n\
         以下の変更が tako のアップデートで setup に入っています。\n\
         **guided** の項目は対話で確認・適用し、**auto** の項目は概要を伝えるだけでよい\n\
         （setup の再実行自体が適用を兼ねる）。\n\n",
        pending
            .iter()
            .map(|c| c.revision)
            .max()
            .unwrap_or(applied_revision),
    ));
    for change in pending {
        let kind = match change.kind {
            ChangeKind::Auto => "auto（自動適用済み・通知のみ）",
            ChangeKind::Guided => "guided（対話で確認・適用が必要）",
        };
        md.push_str(&format!(
            "## rev {} — {}\n\n- 導入バージョン: tako v{}（{}）\n- 区分: {}\n\n{}\n",
            change.revision, change.title, change.version, change.date, kind, change.description,
        ));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup値はdetected_previous_defaultの順で解決する() {
        let detected = resolve_setup_value(Some("pro"), Some("free"), Some("unknown")).unwrap();
        assert_eq!(detected.value, "pro");
        assert_eq!(detected.source, SetupValueSource::Detected);
        assert_eq!(detected.previous.as_deref(), Some("free"));
        assert_eq!(detected.source.label(), "detected");

        let previous = resolve_setup_value(None, Some("max-20x"), Some("unknown")).unwrap();
        assert_eq!(previous.source, SetupValueSource::Previous);
        assert_eq!(previous.source.label(), "previous");

        let default = resolve_setup_value(None, None, Some("unknown")).unwrap();
        assert_eq!(default.source, SetupValueSource::Default);
        assert_eq!(default.source.label(), "default");
        assert!(resolve_setup_value(None, None, None).is_none());
    }

    #[test]
    fn setup_planは実差分だけをsource付きで描画する() {
        let mut plan = SetupPlan::default();
        plan.push_if_changed(
            "setup.selected_agent",
            Some("claude"),
            "claude",
            SetupValueSource::Previous,
        );
        assert!(plan.is_empty());

        plan.push_if_changed(
            "setup.provider_plans.claude",
            Some("free"),
            "pro",
            SetupValueSource::Detected,
        );
        plan.push_if_changed(
            "profiles/default.yaml",
            None,
            "推奨 profile を作成",
            SetupValueSource::Default,
        );
        assert_eq!(plan.changes().len(), 2);
        let diff = plan.render_diff();
        assert!(diff.contains("free -> pro [detected]"));
        assert!(diff.contains("(未設定) -> 推奨 profile を作成 [default]"));
    }

    #[test]
    fn setup_answersは全項目をparseして不正値を拒否する() {
        let answers = SetupAnswers::from_json(
            r##"{
                "selected_agent":"codex",
                "provider_plans":{"gpt":"plus"},
                "instruction_content":"# Rules",
                "profile":{"master_agent":"codex","effort":"high","worker_model_policy":"inherit"},
                "projects":{"app":{"cwd":"~/src/app","description":"main app"}},
                "orchestrator":{"auto_close":false,"auto_push":true},
                "sleep_guard":{"mode":"while-agents-running","power":"ac-only"}
            }"##,
        )
        .unwrap();
        assert_eq!(answers.selected_agent.as_deref(), Some("codex"));
        assert_eq!(answers.provider_plans["gpt"], "plus");
        assert_eq!(answers.projects.as_ref().unwrap()["app"].cwd, "~/src/app");
        assert_eq!(
            answers.orchestrator.as_ref().unwrap().auto_close,
            Some(false)
        );
        assert!(SetupAnswers::from_json(r#"{"selected_agent":"unknown"}"#).is_err());
        assert!(SetupAnswers::from_json(r#"{"extra":true}"#).is_err());
        assert!(SetupAnswers::from_json(r#"{"instruction_content":"  "}"#).is_err());
        assert!(SetupAnswers::from_json(r#"{"projects":{"":{"cwd":"x"}}}"#).is_err());
        // launch_agent（Issue #295）
        assert!(SetupAnswers::from_json(r#"{"launch_agent":"claude"}"#).is_ok());
        assert!(SetupAnswers::from_json(r#"{"launch_agent":"none"}"#).is_ok());
        assert!(SetupAnswers::from_json(r#"{"launch_agent":"unknown"}"#).is_err());
    }

    #[test]
    fn config_roundtrip() {
        let config = SetupConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let back: SetupConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(back.orchestrator.auto_close);
        assert!(back.orchestrator.auto_push);
        assert!(!back.setup.completed);
        assert_eq!(back.setup.applied_revision, 0);
        // モデル・effort は profiles/*.yaml が正。config.yaml には書かない（Issue #27）
        assert!(!yaml.contains("model"));
        assert!(!yaml.contains("[1m]"));
    }

    #[test]
    fn config_ignores_legacy_model_keys() {
        // 旧バージョンの config.yaml（master_model 等入り）も読める後方互換
        let legacy = "orchestrator:\n  master_model: claude-opus-4-6[1m]\n  worker_model: claude-opus-4-6[1m]\n  effort: max\n  auto_close: false\nsetup:\n  completed: true\n";
        let config: SetupConfig = serde_yaml::from_str(legacy).unwrap();
        assert!(!config.orchestrator.auto_close);
        assert!(config.setup.completed);
        // applied_revision 無し = 0（全変更が未適用扱い。Issue #94）
        assert_eq!(config.setup.applied_revision, 0);
        assert!(config.setup.applied_version.is_none());
    }

    /// #169 横展開: 破損 config.yaml への mutate は Err で中断しファイル不変
    #[test]
    fn issue_169_mutate_config_rejects_corrupted_yaml() {
        let dir =
            std::env::temp_dir().join(format!("tako-issue169-setup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let corrupted = "setup:\n  completed: [broken";
        std::fs::write(&path, corrupted).unwrap();

        let result = mutate_config_at(&path, |c| c.setup.completed = true);
        assert!(result.is_err(), "破損 config.yaml は default に丸めず Err");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupted,
            "破損ファイルは書き換えられない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #169 横展開: mutate_config_at は既存フィールドを保持したまま部分更新する
    #[test]
    fn issue_169_mutate_config_preserves_other_fields() {
        let dir = std::env::temp_dir().join(format!(
            "tako-issue169-setup-preserve-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "orchestrator:\n  auto_close: false\n").unwrap();

        mutate_config_at(&path, |c| c.setup.completed = true).unwrap();
        let after = load_config_from(&path).unwrap();
        assert!(after.setup.completed, "変更したフィールドが反映される");
        assert!(
            !after.orchestrator.auto_close,
            "無関係のフィールドは保持される"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_layoutセクションのroundtripと解決() {
        // 既定（未設定）はシリアライズされず、既定値へ解決される（Issue #165）
        let config = SetupConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(!yaml.contains("spawn_layout"));
        let resolved = config.spawn_layout.resolve();
        assert_eq!(resolved, tako_core::SpawnLayoutConfig::default());
        assert_eq!(
            resolved.policy,
            tako_core::SpawnLayoutPolicy::MasterReserved
        );
        assert_eq!(resolved.master_ratio, 0.5);
        assert_eq!(resolved.algorithm, tako_core::WorkerLayoutAlgorithm::Grid);

        // 設定値の round-trip
        let mut config = SetupConfig::default();
        config.spawn_layout.policy = Some("legacy".into());
        config.spawn_layout.master_ratio = Some(0.6);
        config.spawn_layout.algorithm = Some("spiral".into());
        let yaml = serde_yaml::to_string(&config).unwrap();
        let back: SetupConfig = serde_yaml::from_str(&yaml).unwrap();
        let resolved = back.spawn_layout.resolve();
        assert_eq!(resolved.policy, tako_core::SpawnLayoutPolicy::Legacy);
        assert_eq!(resolved.master_ratio, 0.6);
        assert_eq!(resolved.algorithm, tako_core::WorkerLayoutAlgorithm::Spiral);
    }

    #[test]
    fn spawn_layoutの不正値は既定へフォールバックする() {
        // 手編集の不正値で spawn を止めない（Issue #165）
        let yaml = "spawn_layout:\n  policy: golden\n  master_ratio: 7.5\n  algorithm: mosaic\n";
        let config: SetupConfig = serde_yaml::from_str(yaml).unwrap();
        let resolved = config.spawn_layout.resolve();
        assert_eq!(
            resolved.policy,
            tako_core::SpawnLayoutPolicy::MasterReserved
        );
        // 範囲外の比率はクランプ
        assert_eq!(resolved.master_ratio, 0.9);
        assert_eq!(resolved.algorithm, tako_core::WorkerLayoutAlgorithm::Grid);
    }

    #[test]
    fn config_applied_revision_roundtrip() {
        let mut config = SetupConfig::default();
        config.setup.completed = true;
        config.setup.applied_revision = 4;
        config.setup.applied_version = Some("0.2.9".into());
        config.setup.selected_agent = Some("codex".into());
        config
            .setup
            .provider_plans
            .insert("gpt".into(), "plus".into());
        let yaml = serde_yaml::to_string(&config).unwrap();
        let back: SetupConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.setup.applied_revision, 4);
        assert_eq!(back.setup.applied_version.as_deref(), Some("0.2.9"));
        assert_eq!(back.setup.selected_agent.as_deref(), Some("codex"));
        assert_eq!(back.setup.provider_plans["gpt"], "plus");
    }

    #[test]
    fn embedded_changes_parse_and_monotonic() {
        let changes = all_changes().expect("埋め込み changes.yaml はパースできること");
        assert!(!changes.is_empty());
        // revision は 1 始まり・単調増加・欠番なし（記入ルールの機械検証）
        for (i, change) in changes.iter().enumerate() {
            assert_eq!(
                change.revision,
                (i + 1) as u32,
                "revision は 1 始まりの連番: {} 番目が rev {}",
                i + 1,
                change.revision
            );
            assert!(!change.title.is_empty());
            assert!(!change.description.is_empty());
            assert!(!change.version.is_empty());
            assert!(!change.date.is_empty());
        }
    }

    #[test]
    fn pending_changes_filters_by_revision() {
        let current = current_revision().unwrap();
        assert!(current >= 4, "初期エントリ 4 件が存在する");
        // 全適用済み → 空
        assert!(pending_changes(current).unwrap().is_empty());
        // 追従機構導入前（0）→ 全件
        assert_eq!(pending_changes(0).unwrap().len(), current as usize);
        // 途中まで適用 → それ以降のみ
        let pending = pending_changes(2).unwrap();
        assert!(pending.iter().all(|c| c.revision > 2));
        assert_eq!(pending.len(), (current - 2) as usize);
    }

    #[test]
    fn change_kind_deserializes_lowercase() {
        let yaml = "revision: 1\nversion: \"0.2.4\"\ndate: \"2026-07-02\"\nkind: guided\ntitle: t\ndescription: d\n";
        let c: SetupChange = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.kind, ChangeKind::Guided);
    }

    #[test]
    fn pending_markdown_lists_all_entries() {
        let pending = pending_changes(0).unwrap();
        let md = render_pending_markdown(&pending, 0);
        for change in &pending {
            assert!(
                md.contains(&change.title),
                "rev {} のタイトルを含む",
                change.revision
            );
            assert!(md.contains(&format!("rev {}", change.revision)));
        }
        assert!(md.contains("guided"));
        assert!(md.contains("auto"));
    }

    // --- 項目レベル比較（Issue #322） ---

    /// 同梱 sections は全 7 項目が coverage メタ行を持ち、パースできること
    #[test]
    fn issue_322_recommended_sections_parse() {
        assert_eq!(RECOMMENDED_SECTIONS.len(), 7, "同梱推奨ルールは 7 項目");
        let sections = recommended_coverage_sections();
        assert_eq!(
            sections.len(),
            7,
            "全項目が coverage メタ行を持つ（不足はメタ行の記入漏れ）"
        );
        for section in &sections {
            assert!(!section.title.is_empty());
            assert!(
                !section.concepts.is_empty(),
                "{}: 概念が 1 つ以上ある",
                section.title
            );
            for concept in &section.concepts {
                assert!(!concept.keywords.is_empty());
                assert!(
                    concept.keywords.iter().all(|k| *k == k.to_lowercase()),
                    "キーワードは小文字化済み"
                );
            }
        }
    }

    /// 同梱の既定指示ファイルは自分自身の推奨ルールを全項目カバーする（整合性の機械検証）
    #[test]
    fn issue_322_default_instructions_cover_all_sections() {
        let coverage = compare_instruction_coverage(INSTRUCTIONS_DEFAULT);
        assert!(
            coverage.is_full(),
            "既定テンプレートに不足がある: {:?}",
            coverage.missing_summaries()
        );
        let lines = coverage.render_lines();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("差分なし"),
            "差分ゼロは明示する: {lines:?}"
        );
    }

    /// 部分的な指示ファイルは不足項目が具体的に提示される
    #[test]
    fn issue_322_partial_instructions_report_missing() {
        let partial = "# My Rules\n\n## 言語\n\n- 回答は日本語で\n\n## Git\n\n- コミットは機能単位、push は PR ブランチ経由\n";
        let coverage = compare_instruction_coverage(partial);
        assert!(!coverage.is_full());

        let missing = coverage.missing_summaries();
        let joined = missing.join("\n");
        assert!(
            joined.contains("安全ルール"),
            "安全ルール不足を検出: {joined}"
        );
        assert!(joined.contains("完了検証"), "完了検証不足を検出: {joined}");
        // カバー済み項目は不足に出ない
        assert!(!joined.contains("言語設定"), "言語はカバー済み: {joined}");

        let lines = coverage.render_lines();
        assert!(lines[0].contains("全 7 項目"));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[OK]") && l.contains("言語設定")),
            "カバー済み項目の [OK] 行がある: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[不足の可能性]") && l.contains("安全ルール")),
            "不足項目は [不足の可能性] で提示: {lines:?}"
        );
    }

    /// 空の指示ファイルは全項目が不足になる
    #[test]
    fn issue_322_empty_instructions_report_all_missing() {
        let coverage = compare_instruction_coverage("");
        assert!(!coverage.is_full());
        assert_eq!(coverage.missing_summaries().len(), 7, "全 7 項目が不足");
        assert!(!coverage.render_lines().iter().any(|l| l.contains("[OK]")));
    }

    /// coverage メタ行パーサの仕様（不正行の無視・小文字化・空キーワード除外）
    #[test]
    fn issue_322_parse_coverage_section_spec() {
        let md = "# テスト項目\n\n<!-- coverage: 概念A = Foo | バー | -->\n<!-- coverage: 等号を持たない壊れた行 -->\n<!-- coverage: = キーワードのみ -->\n本文\n";
        let section = parse_coverage_section(md).unwrap();
        assert_eq!(section.title, "テスト項目");
        assert_eq!(section.concepts.len(), 1, "不正なメタ行は無視");
        assert_eq!(section.concepts[0].name, "概念A");
        assert_eq!(section.concepts[0].keywords, vec!["foo", "バー"]);

        // タイトルなし・メタ行なしは None
        assert!(parse_coverage_section("本文だけ").is_none());
        assert!(parse_coverage_section("# タイトルのみ\n本文").is_none());
    }
}
