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

use tako_core::platform::support::Platform;

/// バイナリ埋め込みの setup changelog
pub const CHANGES_YAML: &str = include_str!("../../../resources/setup/changes.yaml");

/// setup アシスタントの system prompt 正本（バイナリ同梱）。
/// **プラットフォーム別に複製しない**。差分は `platform::facts::render_current` で注入する（#516）
pub const SYSTEM_PROMPT: &str = include_str!("../../../resources/setup/system-prompt.md");

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
    /// AI 系設定の git 共有（Issue #513）。**オプション**なので省略時は何もしない
    /// （標準 setup の質問ゼロ原則 #262 を守る）。明示指定か `--review` でだけ配線する
    pub config_share: Option<SetupConfigShareAnswers>,
}

/// 設定共有の回答（Issue #513）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupConfigShareAnswers {
    /// true で配線する。false / 省略なら何もしない
    pub enable: Option<bool>,
    /// 既存の共有リポジトリ（ローカルパスまたは git URL）。省略時は新規作成
    pub repo: Option<String>,
    /// リポジトリの配置先（省略時は `~/tako-config-sync`）
    pub path: Option<String>,
    /// 新規作成時に origin として登録するリモート URL
    pub remote: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// master が引き継ぎを始める ctx% 閾値（#193 / #749。既定 60、値域 50〜60）。
    /// `None` = 未設定（既定値を使う）。プロファイル側の `ctx_threshold` が優先される。
    /// 解決は `orchestrator::resolve_ctx_threshold` が唯一の正
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx_threshold: Option<u32>,
}

/// #566: `#[derive(Default)]` だと `confirm_close` が `bool::default()` = false、
/// `ctx_threshold` が 0 になり、**config.yaml が無い環境（新規ユーザー・隔離起動）で
/// serde の既定値と食い違う**。`load_config` は不在時に `Ok(default())` を返すので、
/// 「既定 true」と書かれた close 確認が実際には無効になっていた。手書きで揃える
impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            orchestrator: OrchestratorConfig::default(),
            setup: SetupState::default(),
            agents_sync: crate::agents_sync::AgentsSyncConfig::default(),
            spawn_layout: SpawnLayoutSection::default(),
            confirm_close: default_true(),
            ctx_threshold: None,
        }
    }
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
    /// worker ペイン 1 枚に保証する最小の桁数（#1132。0 = 保証しない）。
    /// **旧ファイルにこのキーは無い**ので serde default（= None → 既定値へ解決）で読める。
    /// 移行の手順は不要（#916 の指紋テストが型の変更を検知する）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_worker_cols: Option<u16>,
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
            min_worker_cols: self
                .min_worker_cols
                .map(tako_core::spawn_layout::clamp_min_worker_cols)
                .unwrap_or(defaults.min_worker_cols),
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

// --- setup ディレクトリの置き場（Issue #1019） -------------------------------

/// 旧実装（tako-cli）が直書きしていた相対パス。**macOS の形**なので、
/// Windows / Linux ではその OS に存在しない形のディレクトリを作っていた
const LEGACY_SETUP_REL: &str = "Library/Application Support/tako/setup";

/// `tako setup` が生成物（`setup-instructions.md` / `CLAUDE.md` / `AGENTS.md` /
/// `GEMINI.md` / `changes.yaml` / `setup-context.yaml` / `pending-changes.md` /
/// `templates/`）を置くディレクトリ = `<data_dir>/setup`。
///
/// **data dir の境界（[`tako_core::paths::data_dir`]）を必ず通す**（#1019）。
/// 以前は tako-cli 側で `~/Library/Application Support/tako/setup` を直書きしていたので、
///
/// 1. Windows で `%USERPROFILE%\Library\Application Support\tako\setup` という
///    **その OS に存在しない形**の場所へ書いていた（書く場所と読む場所は同じなので
///    機能はするが、data dir の外なので `tako recover` / 設定共有（#513）/
///    バックアップの対象から外れ、アンインストールでも残る）
/// 2. `TAKO_DATA_DIR` / `TAKO_ISOLATED=1` で隔離したはずの `tako setup` が
///    **本番の setup ディレクトリを書き換えて**いた（#1002 の検証で実際に踏んだ）
pub fn setup_dir() -> Result<PathBuf, String> {
    tako_core::paths::data_dir()
        .map(|d| d.join("setup"))
        .ok_or_else(|| "ホームディレクトリが取得できない（$HOME 未設定）".into())
}

/// 旧実装が書いていた場所（`<home>/Library/Application Support/tako/setup`）。
///
/// macOS の既定ではここが `<data_dir>/setup` と**同じ場所**になるので、移設の要否は
/// OS で分岐せず「新旧が違うか」で決める（[`relocation_pair`]）
pub fn legacy_setup_dir() -> Option<PathBuf> {
    tako_core::paths::home_dir().map(|h| h.join(LEGACY_SETUP_REL))
}

/// 旧い場所からの移設（#1019）で何が起きるか。`Check` と `Apply` で同じ形を返す
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRelocation {
    /// 旧い場所
    pub from: PathBuf,
    /// 新しい場所（`<data_dir>/setup`）
    pub to: PathBuf,
    /// 旧ディレクトリの退避先。移設後は**旧内容がまるごとここに残る**
    pub backup: PathBuf,
    /// 新しい場所へ写した（写す）相対パス（`/` 区切りに正規化）
    pub copied: Vec<String>,
    /// 既に新しい場所にあるので触らなかった相対パス
    pub kept: Vec<String>,
}

/// 移設の要否を決める純粋関数（env も fs も読まない = Windows の解決をテストで再現できる）。
///
/// `Some((旧, 新))` を返すのは**すべて**満たすときだけ:
///
/// 1. `TAKO_DATA_DIR` が立っていない — 隔離した検証が**本番の setup を吸い上げない**ため。
///    「隔離したつもりが本番を触る」がこの Issue そのものなので、移設側で同じ穴を開けない。
///    隔離中は旧い場所を見つけても触らず、本番の実行で改めて移す
/// 2. 新旧の場所が違う — macOS の既定では同じ場所なので**既存ユーザーは無移行**
pub(crate) fn relocation_pair(
    legacy: Option<PathBuf>,
    data_dir_overridden: bool,
    data_dir: Option<PathBuf>,
) -> Option<(PathBuf, PathBuf)> {
    if data_dir_overridden {
        return None;
    }
    let legacy = legacy?;
    let to = data_dir?.join("setup");
    if legacy == to {
        return None;
    }
    Some((legacy, to))
}

/// 実環境での新旧の場所（[`relocation_pair`] へ env を渡すだけの薄い口）
fn live_relocation_pair() -> Option<(PathBuf, PathBuf)> {
    let overridden = std::env::var_os("TAKO_DATA_DIR").is_some_and(|v| !v.is_empty());
    relocation_pair(legacy_setup_dir(), overridden, tako_core::paths::data_dir())
}

/// 移設すべき旧い場所と移設先（`(旧, 新)`）。移設が要らなければ None。
///
/// **冪等性の門番はここ**: 移設が済むと旧ディレクトリは退避先へ rename されて
/// 無くなるので、2 回目は `is_dir` が偽 = None になる
/// （「移行済み」を別ファイルへ記録しない。#513 の設定共有で必ず壊れるため）
pub fn setup_relocation_target() -> Option<(PathBuf, PathBuf)> {
    let (from, to) = live_relocation_pair()?;
    from.is_dir().then_some((from, to))
}

/// 旧い場所を新しい場所へ移す。`apply = false` なら**1 バイトも書かず**同じ内訳を返す
/// （`status` の予告と `run` の報告が食い違わないよう、走査も失敗の出方も 1 実装にする）。
///
/// 旧ディレクトリは**消さずに** `setup.pre-v1.bak` へ退避する
pub fn relocate_setup_dir(from: &Path, to: &Path, apply: bool) -> Result<SetupRelocation, String> {
    let backup = free_backup_path(from);
    let mut copied = Vec::new();
    let mut kept = Vec::new();
    walk(from, from, to, apply, 0, &mut copied, &mut kept)?;
    if apply {
        // **写してから退避する**。退避（rename）が済んだ時点で旧い場所は消えるので、
        // 次回は `setup_relocation_target` が None を返す = 冪等
        std::fs::rename(from, &backup).map_err(|e| {
            format!(
                "旧 setup ディレクトリの退避に失敗（{} -> {}）: {e}",
                from.display(),
                backup.display()
            )
        })?;
    }
    copied.sort();
    kept.sort();
    Ok(SetupRelocation {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        backup,
        copied,
        kept,
    })
}

/// 旧い場所を 1 段ずつ写す。**移設先に同名がある場合は触らない**
/// （新しい場所の内容の方が新しいので、旧い写しで上書きしない）
#[allow(clippy::too_many_arguments)]
fn walk(
    root: &Path,
    dir: &Path,
    to_root: &Path,
    apply: bool,
    depth: usize,
    copied: &mut Vec<String>,
    kept: &mut Vec<String>,
) -> Result<(), String> {
    // setup ディレクトリは 2 段（`templates/`）しか無い。循環したリンクを
    // 掴んでも無限に潜らないための保険
    if depth > 8 {
        return Ok(());
    }
    let reader =
        std::fs::read_dir(dir).map_err(|e| format!("{} を読めない: {e}", dir.display()))?;
    for entry in reader {
        let entry = entry.map_err(|e| format!("{} の走査に失敗: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("{} の種別を読めない: {e}", path.display()))?;
        if file_type.is_dir() {
            walk(root, &path, to_root, apply, depth + 1, copied, kept)?;
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let dest = to_root.join(rel);
        let label = rel_label(rel);
        if dest.exists() {
            kept.push(label);
            continue;
        }
        if apply {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{} を作れない: {e}", parent.display()))?;
            }
            std::fs::copy(&path, &dest)
                .map_err(|e| format!("{} を {} へ写せない: {e}", path.display(), dest.display()))?;
        }
        copied.push(label);
    }
    Ok(())
}

/// 相対パスの表示（OS によらず `/` 区切り。報告とテストの期待値を揃える）
fn rel_label(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// 旧ディレクトリの退避先。既に埋まっていたら番号を足す
/// （古い tako をもう一度動かして旧い場所が復活した場合でも前の退避を潰さない）
fn free_backup_path(dir: &Path) -> PathBuf {
    let base = tako_core::migration::backup_path(dir, 1);
    if !base.exists() {
        return base;
    }
    for n in 2..100u32 {
        let mut name = base.as_os_str().to_os_string();
        name.push(format!(".{n}"));
        let candidate = PathBuf::from(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    base
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
    (
        "templates/sections/07-context-budget.md",
        include_str!("../../../resources/setup/templates/sections/07-context-budget.md"),
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
    /// この変更が対象とするプラットフォーム（省略時は全プラットフォーム）。
    /// **正本を OS ごとに複製しないための仕組み**（設計 §4）。
    /// 値は "macos" / "windows"。未知の値はパースで弾く
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
}

impl SetupChange {
    /// 指定プラットフォームが対象か（`platforms` 省略 = 全プラットフォーム対象）
    pub fn applies_to(&self, platform: Platform) -> bool {
        match &self.platforms {
            None => true,
            Some(list) => list.iter().any(|p| Platform::parse(p) == Some(platform)),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChangesFile {
    changes: Vec<SetupChange>,
}

fn parse_changes(yaml: &str) -> Result<Vec<SetupChange>, String> {
    let file: ChangesFile =
        serde_yaml::from_str(yaml).map_err(|e| format!("changes.yaml のパースに失敗: {e}"))?;
    // 未知のプラットフォーム名は黙って無視せず弾く（書き間違いが配信漏れになるため）
    for c in &file.changes {
        if let Some(list) = &c.platforms {
            for p in list {
                if Platform::parse(p).is_none() {
                    return Err(format!(
                        "changes.yaml revision {} の platforms に未知の値: {p}（macos / windows）",
                        c.revision
                    ));
                }
            }
        }
    }
    Ok(file.changes)
}

/// 埋め込み changelog の全エントリ（revision 昇順はファイル記載順に依存。テストで検証）
pub fn all_changes() -> Result<Vec<SetupChange>, String> {
    changes_for(Platform::current())
}

/// 指定プラットフォーム向けの changelog（`platforms:` で絞り込む）。
/// **macOS 上から Windows 向けの配信内容を検証できる**ようにするため引数で受ける
pub fn changes_for(platform: Platform) -> Result<Vec<SetupChange>, String> {
    Ok(parse_changes(CHANGES_YAML)?
        .into_iter()
        .filter(|c| c.applies_to(platform))
        .collect())
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

    // --- setup ディレクトリの置き場（Issue #1019） ---------------------------

    /// `TAKO_DATA_DIR` はプロセス全体のグローバルなので、触るテストは直列化する
    /// （`platform::path` の `LEGACY_ENV` と同じ形）
    static DATA_DIR_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 一時ディレクトリ（テスト間で衝突しない名前）
    fn tmp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-1019-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        dir
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("親を作れる");
        }
        std::fs::write(path, body).expect("書ける");
    }

    /// Windows の環境をエミュレートする（`%USERPROFILE%` と `%APPDATA%` は別の場所）。
    /// 実機がオフラインでも移設の判定と挙動をここで固定できる
    #[test]
    fn windowsの旧パスから_appdata_配下へ移す判定になる() {
        let home = PathBuf::from(r"C:\Users\winuser");
        let appdata = PathBuf::from(r"C:\Users\winuser\AppData\Roaming\tako");
        let pair = relocation_pair(
            Some(home.join(LEGACY_SETUP_REL)),
            false,
            Some(appdata.clone()),
        );
        assert_eq!(
            pair,
            Some((home.join(LEGACY_SETUP_REL), appdata.join("setup"))),
            "%USERPROFILE%\\Library\\... から %APPDATA%\\tako\\setup へ移す"
        );
    }

    /// **macOS の既定は無移行**（新旧が同じ場所）。既存ユーザーの回帰ゼロがこの形で保たれる
    #[test]
    fn macosの既定では新旧が同じ場所なので移設しない() {
        let home = PathBuf::from("/Users/testuser");
        let data = home.join("Library/Application Support/tako");
        assert_eq!(
            relocation_pair(Some(home.join(LEGACY_SETUP_REL)), false, Some(data)),
            None,
            "macOS の既定では data_dir()/setup が旧パスと同一"
        );
    }

    /// **隔離中は本番を吸い上げない**（この Issue の症状を移設側で再現しないための鍵）。
    /// `TAKO_DATA_DIR` が立っているあいだは旧い場所を見つけても触らず、
    /// 本番の実行で改めて移す
    #[test]
    fn 隔離中は旧い場所を見つけても移設しない() {
        let home = PathBuf::from("/Users/testuser");
        assert_eq!(
            relocation_pair(
                Some(home.join(LEGACY_SETUP_REL)),
                true,
                Some(PathBuf::from("/tmp/tako-iso")),
            ),
            None,
            "TAKO_DATA_DIR が立っているあいだは移設しない"
        );
        // 上書きが無ければ同じ入力でも移設する（隔離だけが理由であることの対照）
        assert!(relocation_pair(
            Some(home.join(LEGACY_SETUP_REL)),
            false,
            Some(PathBuf::from("/tmp/tako-iso")),
        )
        .is_some());
    }

    /// 旧い場所の中身が新しい場所へ写り、**旧側は消えずに退避**される。
    /// 2 回目は旧い場所が無いので何も起きない（冪等）
    #[test]
    fn 移設は写して退避し二回目は何もしない() {
        let root = tmp("relocate");
        let from = root.join("legacy/setup");
        let to = root.join("data/setup");
        write(&from.join("setup-context.yaml"), "agent: claude\n");
        write(&from.join("templates/config-default.yaml"), "a: 1\n");

        let done = relocate_setup_dir(&from, &to, true).expect("移設できる");
        assert_eq!(
            done.copied,
            vec![
                "setup-context.yaml".to_string(),
                "templates/config-default.yaml".to_string()
            ]
        );
        assert!(done.kept.is_empty());
        assert_eq!(
            std::fs::read_to_string(to.join("setup-context.yaml")).expect("読める"),
            "agent: claude\n",
            "選んだ agent の状態が新しい場所へ移る"
        );
        assert!(to.join("templates/config-default.yaml").is_file());
        assert!(
            !from.exists(),
            "旧い場所は空く（次回は検出されない = 冪等）"
        );
        assert_eq!(done.backup, from.with_file_name("setup.pre-v1.bak"));
        assert_eq!(
            std::fs::read_to_string(done.backup.join("setup-context.yaml")).expect("読める"),
            "agent: claude\n",
            "旧内容は消さずに退避されている"
        );

        // 2 回目: 公開の入口（setup_relocation_target）が `is_dir` で門番しているので
        // ここへ来ない = 冪等
        assert!(!from.is_dir(), "2 回目は移設の対象にならない");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **移設先に同名があれば触らない**（新しい場所の内容の方が新しい）。
    /// 旧い写しで上書きすると、移設が「巻き戻し」になってしまう
    #[test]
    fn 移設先に既にある内容は上書きしない() {
        let root = tmp("keep");
        let from = root.join("legacy/setup");
        let to = root.join("data/setup");
        write(&from.join("setup-context.yaml"), "agent: codex\n");
        write(&from.join("changes.yaml"), "rev: 1\n");
        write(&to.join("setup-context.yaml"), "agent: claude\n");

        let done = relocate_setup_dir(&from, &to, true).expect("移設できる");
        assert_eq!(done.copied, vec!["changes.yaml".to_string()]);
        assert_eq!(done.kept, vec!["setup-context.yaml".to_string()]);
        assert_eq!(
            std::fs::read_to_string(to.join("setup-context.yaml")).expect("読める"),
            "agent: claude\n",
            "移設先の内容は残る"
        );
        assert_eq!(
            std::fs::read_to_string(done.backup.join("setup-context.yaml")).expect("読める"),
            "agent: codex\n",
            "写さなかった旧内容も退避側には残る"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 見るだけ（`apply = false`）は **1 バイトも書かない**。
    /// `status` の予告と `run` の報告が同じ走査から出ることも一緒に固定する
    #[test]
    fn 見るだけの走査は書き込まない() {
        let root = tmp("dry");
        let from = root.join("legacy/setup");
        let to = root.join("data/setup");
        write(&from.join("setup-context.yaml"), "agent: claude\n");

        let dry = relocate_setup_dir(&from, &to, false).expect("走査できる");
        assert_eq!(dry.copied, vec!["setup-context.yaml".to_string()]);
        assert!(!to.exists(), "移設先を作らない");
        assert!(from.is_dir(), "旧い場所も触らない");

        let done = relocate_setup_dir(&from, &to, true).expect("移設できる");
        assert_eq!(dry.copied, done.copied, "予告と結果が一致する");
        assert_eq!(dry.backup, done.backup);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 退避先が既に埋まっていても前の退避を潰さない
    /// （古い tako をもう一度動かして旧い場所が復活した場合）
    #[test]
    fn 退避先が埋まっていたら番号を足す() {
        let root = tmp("backup2");
        let from = root.join("legacy/setup");
        write(&from.join("changes.yaml"), "rev: 2\n");
        write(
            &from.with_file_name("setup.pre-v1.bak").join("changes.yaml"),
            "rev: 1\n",
        );
        let done = relocate_setup_dir(&from, &root.join("data/setup"), true).expect("移設できる");
        assert_eq!(done.backup, from.with_file_name("setup.pre-v1.bak.2"));
        assert_eq!(
            std::fs::read_to_string(from.with_file_name("setup.pre-v1.bak").join("changes.yaml"))
                .expect("読める"),
            "rev: 1\n",
            "前の退避は無傷"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 旧い場所が無い環境（新規ユーザー・macOS の既存ユーザー）は no-op
    #[test]
    fn 旧い場所が無ければ何もしない() {
        // `setup_relocation_target` は env を読むので、TAKO_DATA_DIR を触るテストと直列化する
        let _guard = DATA_DIR_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = tmp("absent");
        assert!(
            setup_relocation_target().is_none() || std::env::var_os("TAKO_DATA_DIR").is_some(),
            "実環境でも旧い場所が無ければ None"
        );
        // 走査そのものは「読めない」でエラーになる（呼び出し側が is_dir で門番している）
        assert!(relocate_setup_dir(&root.join("nope"), &root.join("to"), false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **macOS のパスが変わっていない**こと（受け入れ条件 3 = 既存ユーザーの回帰ゼロ）。
    /// `TAKO_DATA_DIR` 未指定のとき、正本経由の setup dir が旧実装の直書きと一致する
    #[test]
    #[cfg(target_os = "macos")]
    fn macosのsetupディレクトリは旧実装と同じ場所を指す() {
        let _guard = DATA_DIR_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("TAKO_DATA_DIR");
        std::env::remove_var("TAKO_DATA_DIR");
        let resolved = setup_dir();
        let legacy = legacy_setup_dir();
        if let Some(saved) = saved {
            std::env::set_var("TAKO_DATA_DIR", saved);
        }
        assert_eq!(
            resolved.ok(),
            legacy,
            "macOS の既定は ~/Library/Application Support/tako/setup のまま"
        );
    }

    /// `TAKO_DATA_DIR` を渡したら**その中**を指す（隔離が効く = 受け入れ条件 1 の土台）
    #[test]
    fn 隔離時のsetupディレクトリはtako_data_dirの中を指す() {
        let _guard = DATA_DIR_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("TAKO_DATA_DIR");
        let iso = tmp("isolated");
        std::env::set_var("TAKO_DATA_DIR", &iso);
        let resolved = setup_dir();
        let plan = setup_relocation_target();
        match saved {
            Some(saved) => std::env::set_var("TAKO_DATA_DIR", saved),
            None => std::env::remove_var("TAKO_DATA_DIR"),
        }
        assert_eq!(resolved.ok(), Some(iso.join("setup")));
        assert!(
            plan.is_none(),
            "隔離中は本番の setup ディレクトリへ手を出さない"
        );
        let _ = std::fs::remove_dir_all(&iso);
    }

    /// #566: config.yaml が無い環境（新規ユーザー・隔離起動）でも
    /// serde の既定値（confirm_close=true / ctx_threshold=未設定）と一致すること。
    /// derive(Default) のままだと close 確認が黙って無効になっていた
    #[test]
    fn config不在時の既定はserdeの既定と一致する() {
        let dir = std::env::temp_dir().join(format!("tako-setup-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let missing = dir.join("config.yaml");
        let loaded = load_config_from(&missing).expect("不在は default で成功する");
        assert!(loaded.confirm_close, "close 確認は既定 ON");
        // #749: 未設定は None（「明示 60」と区別できる形）。実効値は resolve が決める
        assert_eq!(loaded.ctx_threshold, None);

        // 空 YAML（キー未設定）を読んだときの serde 既定とも一致する
        let from_serde: SetupConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(from_serde.confirm_close, loaded.confirm_close);
        assert_eq!(from_serde.ctx_threshold, loaded.ctx_threshold);
        assert!(SetupConfig::default().confirm_close);
    }

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

    /// 記入ルール（1 始まり・欠番なし）は**正本ファイルそのもの**に対して検証する。
    ///
    /// `all_changes()` は現在のプラットフォーム向けに絞り込むので、
    /// `platforms:` 付きのエントリ（#525 の rev 15 は Windows 限定）は
    /// **他方のプラットフォームでは欠番になるのが正しい**。絞り込み後の並びに
    /// 連番を求めると、platforms 付きエントリの後ろに 1 件足しただけで落ちる
    #[test]
    fn embedded_changes_parse_and_monotonic() {
        let all = parse_changes(CHANGES_YAML).expect("埋め込み changes.yaml はパースできること");
        assert!(!all.is_empty());
        for (i, change) in all.iter().enumerate() {
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
        // 絞り込み後は欠番が出てよいが、順序（単調増加）は保たれること。
        // pending の並びと「どこまで適用したか」の比較がこれに依存している
        for platform in [Platform::MacOs, Platform::Windows] {
            let list = changes_for(platform).expect("パースできること");
            assert!(!list.is_empty(), "{platform:?} 向けが空");
            assert!(
                list.windows(2).all(|w| w[0].revision < w[1].revision),
                "{platform:?} 向けの revision が単調増加でない"
            );
        }
    }

    #[test]
    fn pending_changes_filters_by_revision() {
        let current = current_revision().unwrap();
        assert!(current >= 4, "初期エントリ 4 件が存在する");
        // 件数は「このプラットフォーム向けの件数」で数える。revision の最大値とは
        // 一致しない（`platforms:` 付きエントリのぶん欠番が出るため）
        let applicable = all_changes().unwrap();
        // 全適用済み → 空
        assert!(pending_changes(current).unwrap().is_empty());
        // 追従機構導入前（0）→ 全件
        assert_eq!(pending_changes(0).unwrap().len(), applicable.len());
        // 途中まで適用 → それ以降のみ
        let pending = pending_changes(2).unwrap();
        assert!(pending.iter().all(|c| c.revision > 2));
        assert_eq!(
            pending.len(),
            applicable.iter().filter(|c| c.revision > 2).count()
        );
    }

    /// `platforms:`（slice 1 で入れた機構）の**最初の実使用**が効いていること（#525）。
    ///
    /// これが壊れると Windows 限定の案内が macOS ユーザーへ流れる（逆も同じ）。
    /// 実物の changes.yaml に対して見るので、リビジョンを足すときに気付ける
    #[test]
    fn windows限定のリビジョンはmacosへ配信されない() {
        let mac = changes_for(Platform::MacOs).expect("changes.yaml をパースできる");
        let win = changes_for(Platform::Windows).expect("changes.yaml をパースできる");

        // #525 のシェル統合案内は Windows だけ
        let rev = 15;
        assert!(
            win.iter().any(|c| c.revision == rev),
            "revision {rev} が Windows 向けに出ない"
        );
        assert!(
            !mac.iter().any(|c| c.revision == rev),
            "revision {rev} が macOS へも配信されている（platforms の絞り込みが効いていない）"
        );

        // platforms 省略のエントリは両方に出る（絞り込みが全体に波及していないこと）
        let shared: Vec<u32> = mac
            .iter()
            .filter(|c| c.platforms.is_none())
            .map(|c| c.revision)
            .collect();
        assert!(!shared.is_empty(), "共通エントリが 1 件も無い");
        for r in shared {
            assert!(
                win.iter().any(|c| c.revision == r),
                "platforms 省略の revision {r} が Windows 側で消えている"
            );
        }
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

    /// 同梱 sections は全 8 項目が coverage メタ行を持ち、パースできること
    #[test]
    fn issue_322_recommended_sections_parse() {
        assert_eq!(RECOMMENDED_SECTIONS.len(), 8, "同梱推奨ルールは 8 項目");
        let sections = recommended_coverage_sections();
        assert_eq!(
            sections.len(),
            8,
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
        assert!(lines[0].contains("全 8 項目"));
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
        assert_eq!(coverage.missing_summaries().len(), 8, "全 8 項目が不足");
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
