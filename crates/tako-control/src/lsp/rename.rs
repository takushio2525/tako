//! リネーム（S9 / #1686）の問い合わせの型と応答の組み立て
//!
//! 問い合わせそのもの（文書を揃える・サーバの起動を待つ・`prepareRename` → `rename`）は状態を持つ
//! [`super::manager`] の `Shared::rename` が行い、ここは**その入出力の形**だけを持つ:
//! 要求（[`RenameRequest`]）・答え（[`RenameAnswer`]）・失敗（[`RenameError`]）と、応答の JSON。
//!
//! 答えの読み取りと段階 1 の当てる範囲は `tako_core::lsp::rename`、当て方（並べ方・重なり・undo 1 回）は
//! `tako_core::text_edit::TextBuffer::apply_changes`、当てるか（dry-run・版・段階 1）は `crate::dispatch`。
//! **言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::rename::{Blocker, Spot};
use tako_core::text_edit::TextChange;

use super::goto::GotoError;
use super::text;

/// 名前の変更の上限の既定（起動と握手を含む。rust-analyzer はプロジェクト全体を読んでから答える）。
/// `TAKO_LSP_RENAME_TIMEOUT_SECS` で変えられる（0 / 不正 / 空は既定へ落とす = #1503）
pub const DEFAULT_RENAME_TIMEOUT: Duration = Duration::from_secs(60);

/// 応答に並べる書き換えの場所の上限（全ファイルの合計。箇所の数 `edits` は切る前）
pub const MAX_SPOTS: usize = 200;

/// 名前の変更の上限
pub fn rename_timeout() -> Duration {
    std::env::var("TAKO_LSP_RENAME_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map_or(DEFAULT_RENAME_TIMEOUT, Duration::from_secs)
}

/// `TAKO_1686_LEGACY=1` で **#1686 前の挙動**へ戻す（同一バイナリで A/B を取る入口）:
/// 右クリックメニューに「名前の変更」を出さない。CLI / MCP の `tako lsp rename` は残る
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1686_LEGACY").is_some()
}

/// 問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで [`super::LspManager::rename`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRequest {
    pub path: PathBuf,
    /// tako の座標（0 起点の行・行内の UTF-8 バイト桁）。LSP の座標へは manager がサーバの見ている本文で直す
    pub line: usize,
    pub column: usize,
    pub new_name: String,
    /// 編集バッファの全文（あれば）。サーバの写しをこれに揃えてから頼み、答えの座標もこれで直す。
    /// `None`（編集モードでない dry-run）はディスクの中身
    pub document: Option<String>,
    pub timeout: Duration,
}

/// 書き換えるファイル 1 つ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameFile {
    pub uri: String,
    /// ローカルのパス（`file:` でない URI は `None`）
    pub path: Option<PathBuf>,
    /// 頼んだペインのファイルか
    pub source: bool,
    /// 箇所の数
    pub edits: usize,
    /// 箇所の場所（全ファイルで [`MAX_SPOTS`] まで）
    pub spots: Vec<Spot>,
}

/// 問い合わせの答え
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameAnswer {
    /// 答えたサーバの ID（検出表の `id`）
    pub server: &'static str,
    /// `prepareRename` が示した今の名前（示さないサーバ・送らないサーバは `None`）
    pub placeholder: Option<String>,
    pub files: Vec<RenameFile>,
    /// ファイルの操作（`create` / `rename` / `delete`）の種類
    pub resource_ops: Vec<String>,
    /// 頼んだファイルへの書き換え（`text_edit::order_changes` を通した後。範囲は
    /// [`RenameRequest::document`]（無ければディスクの中身）のバイト位置）
    pub source_changes: Vec<TextChange>,
    /// 段階 1 で当てられない理由（当てられるなら `None`）
    pub blocker: Option<Blocker>,
}

impl RenameAnswer {
    /// 箇所の総数
    pub fn edit_count(&self) -> usize {
        self.files.iter().map(|f| f.edits).sum()
    }

    /// 箇所のあるファイルの数
    pub fn file_count(&self) -> usize {
        self.files.iter().filter(|f| f.edits > 0).count()
    }

    /// 段階 1 の制限の理由（日英は `text` の 1 か所）
    pub fn blocker_reason(&self) -> Option<String> {
        Some(match self.blocker.as_ref()? {
            Blocker::OtherFiles { files, edits } => text::fill(
                text::RENAME_OTHER_FILES_REASON,
                &[("files", &files.to_string()), ("edits", &edits.to_string())],
            ),
            Blocker::ResourceOps { count } => text::fill(
                text::RENAME_RESOURCE_OPS_REASON,
                &[("count", &count.to_string())],
            ),
        })
    }

    /// 応答の共通部分（`server` / `placeholder` / `files` / `edits` / `resource_ops`）。
    /// `open_in` はそのパスを開いているペイン（頼んだペイン以外。段階 2 の判断材料）
    pub fn to_json(&self, open_in: impl Fn(&std::path::Path) -> Vec<u64>) -> Value {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|file| {
                let spots: Vec<Value> = file
                    .spots
                    .iter()
                    .map(|s| {
                        json!({
                            "line": s.line + 1,
                            "column": s.column,
                            "old": s.old,
                            "new": s.new,
                        })
                    })
                    .collect();
                let mut out = json!({
                    "uri": file.uri,
                    "path": file.path.as_ref().map(|p| p.display().to_string()),
                    "source": file.source,
                    "edits": file.edits,
                    "places": spots,
                });
                if let Some(path) = &file.path {
                    let panes = open_in(path);
                    if !panes.is_empty() {
                        out["open_in"] = json!(panes);
                    }
                }
                out
            })
            .collect();
        let listed: usize = self.files.iter().map(|f| f.spots.len()).sum();
        let mut out = json!({
            "server": self.server,
            "files": files,
            "file_count": self.file_count(),
            "edits": self.edit_count(),
        });
        if listed < self.edit_count() {
            out["places_truncated"] = json!(self.edit_count() - listed);
        }
        if let Some(placeholder) = &self.placeholder {
            out["placeholder"] = json!(placeholder);
        }
        if !self.resource_ops.is_empty() {
            out["resource_ops"] = json!(self.resource_ops);
        }
        out
    }
}

/// 問い合わせの失敗。**「書き換える所が無い」は失敗ではない**（[`RenameAnswer`] の 0 件）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// 言語サーバの状態として定義ジャンプと共通の失敗（無効・受け持つサーバが無い・未導入・
    /// 未応答・止めた / 諦めた・落ちた・閉じられた・読めない）
    Lsp(GotoError),
    /// サーバが名前の変更に対応していない（`renameProvider` が無い）
    Unsupported { server: &'static str },
    /// `prepareRename` が断った（`null` かエラー。`detail` はサーバの文）
    Rejected {
        server: &'static str,
        detail: Option<String>,
    },
    /// `rename` がエラーで断った（不正な名前など）
    Refused {
        server: &'static str,
        detail: String,
    },
    /// 答えを待つあいだに本文が変わった
    Stale,
    /// 答えを読めない・当てられない（形が違う・重なり）
    InvalidEdits {
        server: &'static str,
        detail: String,
    },
}

impl RenameError {
    /// 応答の `status`（機械可読）
    pub fn status(&self) -> &'static str {
        match self {
            Self::Lsp(error) => error.status(),
            Self::Unsupported { .. } => "unsupported",
            Self::Rejected { .. } => "rejected",
            Self::Refused { .. } => "refused",
            Self::Stale => "stale",
            Self::InvalidEdits { .. } => "invalid-edits",
        }
    }

    /// 理由（日英は `text` の 1 か所）
    pub fn reason(&self) -> String {
        match self {
            // 共通の失敗の文は種類に依らない（種類を使うのは `Unsupported` だけで、それは入らない）
            Self::Lsp(error) => error.reason(GotoKind::Definition),
            Self::Unsupported { server } => {
                text::fill(text::RENAME_UNSUPPORTED_REASON, &[("server", server)])
            }
            Self::Rejected {
                server,
                detail: None,
            } => text::fill(text::RENAME_REJECTED_REASON, &[("server", server)]),
            Self::Rejected {
                server,
                detail: Some(detail),
            } => text::fill(
                text::RENAME_REJECTED_DETAIL_REASON,
                &[("server", server), ("detail", detail)],
            ),
            Self::Refused { server, detail } => text::fill(
                text::RENAME_REFUSED_REASON,
                &[("server", server), ("detail", detail)],
            ),
            Self::Stale => text::RENAME_STALE_REASON.text().to_string(),
            Self::InvalidEdits { server, detail } => text::fill(
                text::RENAME_INVALID_REASON,
                &[("server", server), ("detail", detail)],
            ),
        }
    }

    /// 次の一手
    pub fn next_step(&self) -> String {
        match self {
            Self::Lsp(error) => error.next_step(),
            Self::Unsupported { .. } => text::GOTO_UNSUPPORTED_NEXT_STEP.text().to_string(),
            Self::Rejected { .. } => text::RENAME_REJECTED_NEXT_STEP.text().to_string(),
            Self::Refused { .. } => text::RENAME_REFUSED_NEXT_STEP.text().to_string(),
            Self::Stale => text::GOTO_RETRY_NEXT_STEP.text().to_string(),
            Self::InvalidEdits { .. } => text::FORMAT_INVALID_NEXT_STEP.text().to_string(),
        }
    }

    /// 応答の JSON（`status` / `reason` / `next_step`。未導入なら `install_command` も）
    pub fn to_json(&self) -> Value {
        let mut out = match self {
            // 未導入の導入コマンド・サーバ名の載せ方は定義ジャンプと同じ 1 実装
            Self::Lsp(error) => error.to_json(GotoKind::Definition),
            _ => json!({}),
        };
        if let Some(map) = out.as_object_mut() {
            map.remove("kind");
        }
        out["status"] = json!(self.status());
        out["reason"] = json!(self.reason());
        out["next_step"] = json!(self.next_step());
        match self {
            Self::Unsupported { server }
            | Self::Rejected { server, .. }
            | Self::Refused { server, .. }
            | Self::InvalidEdits { server, .. } => out["server"] = json!(server),
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::lsp::state::ServerState;

    fn all_errors() -> Vec<RenameError> {
        vec![
            RenameError::Lsp(GotoError::Disabled),
            RenameError::Lsp(GotoError::NoServer),
            RenameError::Lsp(GotoError::NotInstalled {
                server: "s",
                reason: "r".into(),
                next_step: "n".into(),
                install_command: "c",
            }),
            RenameError::Lsp(GotoError::Timeout {
                server: "s",
                secs: 3,
                starting: true,
            }),
            RenameError::Lsp(GotoError::Unavailable {
                server: "s",
                state: ServerState::GaveUp,
                crashes: 3,
                held: false,
            }),
            RenameError::Lsp(GotoError::Crashed { server: "s" }),
            RenameError::Unsupported { server: "s" },
            RenameError::Rejected {
                server: "s",
                detail: None,
            },
            RenameError::Rejected {
                server: "s",
                detail: Some("No references found at position".into()),
            },
            RenameError::Refused {
                server: "s",
                detail: "Invalid name".into(),
            },
            RenameError::Stale,
            RenameError::InvalidEdits {
                server: "s",
                detail: "d".into(),
            },
        ]
    }

    #[test]
    fn 失敗はどれも理由と次の一手を持ち定義ジャンプの種類を載せない() {
        for error in all_errors() {
            let value = error.to_json();
            assert_eq!(value["status"], json!(error.status()), "{error:?}");
            assert!(value.get("kind").is_none(), "{error:?}: {value}");
            for key in ["reason", "next_step"] {
                let s = value[key].as_str().unwrap_or("");
                assert!(!s.is_empty(), "{error:?} の {key} が空");
                assert!(!s.contains('{'), "{error:?} の {key} に差し込み漏れ: {s}");
            }
        }
        // サーバの文はそのまま載る（何を断られたかが分かる）
        let refused = all_errors()[9].to_json();
        assert!(refused["reason"].as_str().unwrap().contains("Invalid name"));
        assert_eq!(refused["server"], json!("s"));
    }

    #[test]
    fn 段階1の理由は件数を差し込んだ日英の文() {
        let answer = |blocker| RenameAnswer {
            server: "s",
            placeholder: None,
            files: Vec::new(),
            resource_ops: Vec::new(),
            source_changes: Vec::new(),
            blocker,
        };
        assert_eq!(answer(None).blocker_reason(), None);
        let other = answer(Some(Blocker::OtherFiles { files: 2, edits: 5 }))
            .blocker_reason()
            .unwrap();
        assert_eq!(
            other,
            text::fill(
                text::RENAME_OTHER_FILES_REASON,
                &[("files", "2"), ("edits", "5")]
            )
        );
        assert!(!other.contains('{'));
        let ops = answer(Some(Blocker::ResourceOps { count: 1 }))
            .blocker_reason()
            .unwrap();
        assert_eq!(
            ops,
            text::fill(text::RENAME_RESOURCE_OPS_REASON, &[("count", "1")])
        );
    }

    #[test]
    fn 上限は_env_で変えられ不正は既定へ落ちる() {
        // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
        if tako_core::run_alone!(上限は_env_で変えられ不正は既定へ落ちる) {
            return;
        }
        std::env::set_var("TAKO_LSP_RENAME_TIMEOUT_SECS", "0");
        assert_eq!(rename_timeout(), DEFAULT_RENAME_TIMEOUT);
        std::env::set_var("TAKO_LSP_RENAME_TIMEOUT_SECS", "9");
        assert_eq!(rename_timeout(), Duration::from_secs(9));
        std::env::remove_var("TAKO_LSP_RENAME_TIMEOUT_SECS");
        assert_eq!(rename_timeout(), DEFAULT_RENAME_TIMEOUT);
    }
}
