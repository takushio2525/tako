//! エージェント共通ルール同期（Issue #136）
//!
//! 正本ファイル（ユーザー指定）の内容を、各エージェント（claude / codex / agy）の
//! グローバル指示ファイルにマーカーブロックで埋め込む。
//! ブロック外の内容には一切触れない。書き換え前にバックアップ(.bak)を生成する。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const BEGIN_MARKER: &str = "<!-- BEGIN SYNCED COMMON RULES -->";
pub const END_MARKER: &str = "<!-- END SYNCED COMMON RULES -->";

/// 対象エージェントの識別子
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Agy,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    pub fn target_path(self) -> Option<PathBuf> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)?;
        Some(match self {
            Self::Claude => home.join(".claude/CLAUDE.md"),
            Self::Codex => home.join(".codex/AGENTS.md"),
            Self::Agy => home.join(".gemini/GEMINI.md"),
        })
    }

    pub fn all() -> &'static [AgentKind] {
        &[Self::Claude, Self::Codex, Self::Agy]
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" => Some(Self::Agy),
            _ => None,
        }
    }
}

/// config.yaml に保存する同期設定
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsSyncConfig {
    /// 共通ルール正本ファイルの絶対パス
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// 同期対象のエージェント一覧（空 = 全対象）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
}

impl AgentsSyncConfig {
    pub fn is_default(&self) -> bool {
        self.source_path.is_none() && self.targets.is_empty()
    }

    pub fn resolved_targets(&self) -> Vec<AgentKind> {
        if self.targets.is_empty() {
            AgentKind::all().to_vec()
        } else {
            self.targets
                .iter()
                .filter_map(|s| AgentKind::parse(s))
                .collect()
        }
    }
}

/// 同期 1 件の結果
#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub agent: String,
    pub path: String,
    pub action: String, // "created" / "updated" / "unchanged" / "skipped" / "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// マーカーブロックを含む新しいブロック文字列を生成
fn emit_synced_block(content: &str) -> String {
    let mut block = String::new();
    block.push_str(BEGIN_MARKER);
    block.push('\n');
    block.push_str(content);
    if !content.ends_with('\n') {
        block.push('\n');
    }
    block.push_str(END_MARKER);
    block
}

/// ファイル内のマーカーブロックを置換、またはブロックがなければ先頭に挿入。
/// 戻り値: (新しいファイル内容, 変更があったか)
fn replace_synced_block(existing: &str, new_content: &str) -> (String, bool) {
    let new_block = emit_synced_block(new_content);

    if existing.contains(BEGIN_MARKER) {
        let mut result = String::new();
        let mut in_block = false;
        let mut block_emitted = false;
        for line in existing.lines() {
            if line.trim() == BEGIN_MARKER.trim() {
                if !block_emitted {
                    result.push_str(&new_block);
                    result.push('\n');
                    block_emitted = true;
                }
                in_block = true;
                continue;
            }
            if line.trim() == END_MARKER.trim() {
                in_block = false;
                continue;
            }
            if !in_block {
                result.push_str(line);
                result.push('\n');
            }
        }
        // 末尾の余分な改行を整理（元が改行で終わっていなければ除去）
        if !existing.ends_with('\n') && result.ends_with('\n') {
            result.pop();
        }
        let changed = result != existing;
        (result, changed)
    } else {
        // マーカーがない → 先頭に挿入
        let result = format!("{}\n\n{}", new_block, existing);
        (result, true)
    }
}

/// バックアップファイルのパスを決定（.bak、既存なら .bak.1, .bak.2 ...）
fn backup_path(file: &Path) -> PathBuf {
    let name = file
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = file.parent().unwrap_or(file);
    let base = parent.join(format!("{name}.bak"));
    if !base.exists() {
        return base;
    }
    for i in 1..100 {
        let p = parent.join(format!("{name}.bak.{i}"));
        if !p.exists() {
            return p;
        }
    }
    parent.join(format!("{name}.bak.new"))
}

/// 1 エージェントに対する同期を実行
fn sync_one(agent: AgentKind, source_content: &str) -> SyncResult {
    let path = match agent.target_path() {
        Some(p) => p,
        None => {
            return SyncResult {
                agent: agent.label().into(),
                path: String::new(),
                action: "error".into(),
                backup: None,
                error: Some("ホームディレクトリが取得できない".into()),
            };
        }
    };

    let path_str = path.display().to_string();

    if !path.exists() {
        // 親ディレクトリが存在しない場合はスキップ（エージェント未インストール）
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                return SyncResult {
                    agent: agent.label().into(),
                    path: path_str,
                    action: "skipped".into(),
                    backup: None,
                    error: Some(format!(
                        "ディレクトリ {} が存在しない（未インストール）",
                        parent.display()
                    )),
                };
            }
        }
        // 親ディレクトリはあるがファイルがない → 新規作成
        let content = emit_synced_block(source_content);
        if let Err(e) = std::fs::write(&path, &content) {
            return SyncResult {
                agent: agent.label().into(),
                path: path_str,
                action: "error".into(),
                backup: None,
                error: Some(format!("ファイル作成に失敗: {e}")),
            };
        }
        return SyncResult {
            agent: agent.label().into(),
            path: path_str,
            action: "created".into(),
            backup: None,
            error: None,
        };
    }

    // 既存ファイルを読み込む
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return SyncResult {
                agent: agent.label().into(),
                path: path_str,
                action: "error".into(),
                backup: None,
                error: Some(format!("ファイル読み取りに失敗: {e}")),
            };
        }
    };

    let (new_content, changed) = replace_synced_block(&existing, source_content);

    if !changed {
        return SyncResult {
            agent: agent.label().into(),
            path: path_str,
            action: "unchanged".into(),
            backup: None,
            error: None,
        };
    }

    // バックアップ
    let bak = backup_path(&path);
    if let Err(e) = std::fs::copy(&path, &bak) {
        return SyncResult {
            agent: agent.label().into(),
            path: path_str,
            action: "error".into(),
            backup: None,
            error: Some(format!("バックアップに失敗: {e}")),
        };
    }

    // 書き込み
    if let Err(e) = std::fs::write(&path, &new_content) {
        return SyncResult {
            agent: agent.label().into(),
            path: path_str,
            action: "error".into(),
            backup: Some(bak.display().to_string()),
            error: Some(format!("書き込みに失敗: {e}")),
        };
    }

    SyncResult {
        agent: agent.label().into(),
        path: path_str,
        action: "updated".into(),
        backup: Some(bak.display().to_string()),
        error: None,
    }
}

/// 全対象エージェントに対して同期を実行
pub fn sync_rules(source_path: &Path, targets: &[AgentKind]) -> Result<Vec<SyncResult>, String> {
    if !source_path.is_file() {
        return Err(format!(
            "正本ファイルが見つかりません: {}\n\
             ファイルを作成するか、tako setup で正本のパスを設定してください",
            source_path.display()
        ));
    }

    let source_content = std::fs::read_to_string(source_path)
        .map_err(|e| format!("正本ファイルの読み取りに失敗: {e}"))?;

    if source_content.trim().is_empty() {
        return Err("正本ファイルが空です".into());
    }

    let results: Vec<SyncResult> = targets
        .iter()
        .map(|&a| sync_one(a, &source_content))
        .collect();
    Ok(results)
}

/// 同期状態のチェック（--check 用）。各エージェントのブロック内容が正本と一致しているか
pub fn check_sync_status(source_path: &Path, targets: &[AgentKind]) -> Result<Value, String> {
    if !source_path.is_file() {
        return Ok(json!({
            "configured": true,
            "source_path": source_path.display().to_string(),
            "source_exists": false,
            "status": "source_missing",
            "agents": [],
        }));
    }

    let source_content = std::fs::read_to_string(source_path)
        .map_err(|e| format!("正本ファイルの読み取りに失敗: {e}"))?;

    let expected_block = emit_synced_block(&source_content);

    let mut agents = Vec::new();
    for &agent in targets {
        let path = match agent.target_path() {
            Some(p) => p,
            None => continue,
        };
        let status = if !path.exists() {
            if path.parent().is_none_or(|p| !p.exists()) {
                "not_installed"
            } else {
                "not_synced"
            }
        } else {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if content.contains(&expected_block) {
                        "up_to_date"
                    } else if content.contains(BEGIN_MARKER) {
                        "outdated"
                    } else {
                        "not_synced"
                    }
                }
                Err(_) => "error",
            }
        };
        agents.push(json!({
            "agent": agent.label(),
            "path": path.display().to_string(),
            "status": status,
        }));
    }

    let all_synced = agents
        .iter()
        .all(|a| a["status"] == "up_to_date" || a["status"] == "not_installed");

    Ok(json!({
        "configured": true,
        "source_path": source_path.display().to_string(),
        "source_exists": true,
        "status": if all_synced { "up_to_date" } else { "outdated" },
        "agents": agents,
    }))
}

/// MCP / CLI 共通のエントリポイント: 設定を読んで同期を実行し JSON を返す
pub fn run_sync(
    source_override: Option<&str>,
    targets_override: Option<&[String]>,
) -> Result<Value, String> {
    let config = crate::setup::load_config()?;
    let sync_config = &config.agents_sync;

    let source_path = source_override
        .map(PathBuf::from)
        .or_else(|| sync_config.source_path.as_ref().map(PathBuf::from))
        .ok_or(
            "共通ルールの正本パスが設定されていません。\n\
             tako setup で設定するか、--source オプションで指定してください"
                .to_string(),
        )?;

    let targets = match targets_override {
        Some(list) => list.iter().filter_map(|s| AgentKind::parse(s)).collect(),
        None => sync_config.resolved_targets(),
    };

    let results = sync_rules(&source_path, &targets)?;
    Ok(json!({ "results": results }))
}

/// 同期状態の照会（MCP / CLI 共通）
pub fn status() -> Result<Value, String> {
    let config = crate::setup::load_config()?;
    let sync_config = &config.agents_sync;

    match &sync_config.source_path {
        None => Ok(json!({
            "configured": false,
            "status": "not_configured",
            "message": "共通ルール同期は未設定です。tako setup で設定できます",
        })),
        Some(path) => {
            let targets = sync_config.resolved_targets();
            check_sync_status(Path::new(path), &targets)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_block_wraps_content() {
        let block = emit_synced_block("hello\nworld");
        assert!(block.starts_with(BEGIN_MARKER));
        assert!(block.ends_with(END_MARKER));
        assert!(block.contains("hello\nworld\n"));
    }

    #[test]
    fn replace_inserts_at_top_when_no_marker() {
        let existing = "# My Rules\n\nSome content\n";
        let (result, changed) = replace_synced_block(existing, "common rules");
        assert!(changed);
        assert!(result.starts_with(BEGIN_MARKER));
        assert!(result.contains("# My Rules"));
        assert!(result.contains("common rules"));
    }

    #[test]
    fn replace_updates_existing_block() {
        let existing = format!(
            "# Header\n\n{}\nold content\n{}\n\n# Footer\n",
            BEGIN_MARKER, END_MARKER
        );
        let (result, changed) = replace_synced_block(&existing, "new content");
        assert!(changed);
        assert!(result.contains("new content"));
        assert!(!result.contains("old content"));
        assert!(result.contains("# Header"));
        assert!(result.contains("# Footer"));
    }

    #[test]
    fn replace_unchanged_when_same_content() {
        let content = "same content";
        let existing = format!(
            "# Header\n\n{}\n{}\n{}\n\n# Footer\n",
            BEGIN_MARKER, content, END_MARKER
        );
        let (result, changed) = replace_synced_block(&existing, content);
        assert!(!changed);
        assert_eq!(result, existing);
    }

    #[test]
    fn replace_preserves_outside_content() {
        let existing = format!(
            "Line1\nLine2\n{}\nold stuff\n{}\nLine3\nLine4\n",
            BEGIN_MARKER, END_MARKER
        );
        let (result, _) = replace_synced_block(&existing, "new content");
        assert!(result.contains("Line1\nLine2\n"));
        assert!(result.contains("Line3\nLine4\n"));
        assert!(!result.contains("old stuff"));
        assert!(result.contains("new content"));
    }
}
