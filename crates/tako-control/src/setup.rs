//! setup — `tako setup` の状態管理とアップデート追従（Issue #94）
//!
//! - config.yaml（`~/Library/Application Support/tako/orchestrator/config.yaml`）の
//!   setup セクションのスキーマと読み書き（CLI の対話フローは tako-cli 側）
//! - バイナリ埋め込みの setup changelog（`resources/setup/changes.yaml`）のパースと、
//!   適用済みリビジョンとの突き合わせによる未適用変更の検出
//!
//! 照会結果（[`changes_status`]）は CLI `tako setup --changes` と
//! MCP `tako_setup_changes` の両方から使われる（二重実装を作らない。#83 の教訓）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// バイナリ埋め込みの setup changelog
pub const CHANGES_YAML: &str = include_str!("../../../resources/setup/changes.yaml");

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

pub fn load_config() -> Result<SetupConfig, String> {
    let path = config_yaml_path()?;
    if !path.is_file() {
        return Ok(SetupConfig::default());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("config.yaml の読み取りに失敗: {e}"))?;
    serde_yaml::from_str(&content).map_err(|e| format!("config.yaml のパースに失敗: {e}"))
}

pub fn save_config(config: &SetupConfig) -> Result<(), String> {
    let path = config_yaml_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    }
    let content =
        serde_yaml::to_string(config).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("config.yaml の書き込みに失敗: {e}"))
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

    #[test]
    fn config_applied_revision_roundtrip() {
        let mut config = SetupConfig::default();
        config.setup.completed = true;
        config.setup.applied_revision = 4;
        config.setup.applied_version = Some("0.2.9".into());
        let yaml = serde_yaml::to_string(&config).unwrap();
        let back: SetupConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.setup.applied_revision, 4);
        assert_eq!(back.setup.applied_version.as_deref(), Some("0.2.9"));
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
}
