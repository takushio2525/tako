//! claude（Claude Code）のグローバル設定ファイル `.claude.json` の置き場の規則（**正本 1 箇所**。#2906）
//!
//! 実測（2026-11-08・claude 2.1.301）: `CLAUDE_CONFIG_DIR=X` なら `X/.claude.json`、未設定なら
//! `<ホーム>/.claude.json`（既定の設定 dir `~/.claude` の**外**）。既定を明示した
//! `CLAUDE_CONFIG_DIR=~/.claude` は `~/.claude/.claude.json` で、未設定とは**別のファイル**になる。
//! 空の値は未設定と同じに扱う（tako の他の `CLAUDE_CONFIG_DIR` の読み方と揃える）。
//!
//! この規則を引く口（どれも自分で `.claude.json` を join しない）:
//!
//! - MCP の user スコープの登録の書き先・登録済みの判定・健全性の判定・直書き・表示
//!   （tako-control の `agent_mcp::claude_user_config_json` → `dispatch::mcp_target_path`）
//! - アンインストールの前の片付け（tako-control の `setup_undo::claude_json_paths`）
//! - 使用量の資格情報の置き場（[`crate::usage::credentials_file`]）
//!
//! #2906 の前は MCP の登録側だけがこの規則を通らず `~/.claude.json` 決め打ちで、
//! `CLAUDE_CONFIG_DIR` を向けた環境では claude CLI が `$CLAUDE_CONFIG_DIR/.claude.json` へ書いた
//! 登録を tako が読めなかった（毎回「追加しました」・健全性の楽観判定・直書きが claude から見えない）。
//! 番犬は `crates/tako-control/tests/issue2906_claude_config_dir_mcp_watchdog.rs`。

use std::path::{Path, PathBuf};

/// claude の設定 dir を指す環境変数
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// claude のグローバル設定のファイル名（MCP の user スコープの登録・信頼済みフォルダ・資格情報）
pub const GLOBAL_JSON: &str = ".claude.json";

/// `CLAUDE_CONFIG_DIR` の値から、明示された設定 dir を引く。空は未設定と同じ（`None`）
pub fn explicit_config_dir(raw: Option<&str>) -> Option<PathBuf> {
    raw.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// claude がグローバル設定（`.claude.json`）を読み書きする場所。
///
/// `config_dir` は明示された設定 dir（[`explicit_config_dir`] の答え・アカウントの設定 dir）。
/// 無ければホーム直下。ホームも引けなければ `None`
pub fn global_json(config_dir: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match config_dir {
        Some(dir) => Some(dir.join(GLOBAL_JSON)),
        None => home.map(|h| h.join(GLOBAL_JSON)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 設定dirが明示されていればその中の_claude_json() {
        let home = Path::new("/home/testuser");
        let dir = explicit_config_dir(Some("/home/testuser/.claude-alt"));
        assert_eq!(
            global_json(dir.as_deref(), Some(home)),
            Some(PathBuf::from("/home/testuser/.claude-alt/.claude.json"))
        );
    }

    #[test]
    fn 未設定と空はホーム直下() {
        let home = Path::new("/home/testuser");
        for raw in [None, Some("")] {
            let dir = explicit_config_dir(raw);
            assert_eq!(dir, None, "{raw:?} は未設定と同じ");
            assert_eq!(
                global_json(dir.as_deref(), Some(home)),
                Some(home.join(".claude.json")),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn 既定を明示するとホーム直下とは別のファイル() {
        let home = Path::new("/home/testuser");
        let dir = explicit_config_dir(Some("/home/testuser/.claude"));
        assert_eq!(
            global_json(dir.as_deref(), Some(home)),
            Some(PathBuf::from("/home/testuser/.claude/.claude.json"))
        );
    }

    #[test]
    fn ホームが引けず設定dirも無ければ決めない() {
        assert_eq!(global_json(None, None), None);
        assert_eq!(
            global_json(Some(Path::new("/x/.claude-b")), None),
            Some(PathBuf::from("/x/.claude-b/.claude.json"))
        );
    }
}
