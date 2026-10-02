//! 整形の問い合わせ（S6 / #1683）の型と応答の組み立て
//!
//! 問い合わせそのもの（文書を揃える・サーバの起動を待つ・要求を投げる）は状態を持つ
//! [`super::manager`] の `Shared::format` が行い、ここは**その入出力の形**だけを持つ:
//! 要求（[`FormatRequest`]）・答え（[`FormatAnswer`]）・失敗（[`FormatError`]）と、応答の JSON。
//!
//! 答えの当て方（並べ方・重なり・undo 1 回）は `tako_core::text_edit::TextBuffer::apply_changes`、
//! 当てる場所（どのペインのバッファか・版が変わっていないか）は `crate::dispatch` が決める。
//! **言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::format::FormatOptions;
use tako_core::lsp::goto::GotoKind;
use tako_core::text_edit::TextChange;

use super::goto::GotoError;
use super::text;

/// 明示的な整形（`tako lsp format` / メニュー / ⌘⇧I）の上限の既定。起動と握手を含む
/// （定義ジャンプと同じ。初回はサーバがプロジェクトを読む時間が要る）。
/// `TAKO_LSP_FORMAT_TIMEOUT_SECS` で変えられる（0 / 不正 / 空は既定へ落とす = #1503）
pub const DEFAULT_FORMAT_TIMEOUT: Duration = Duration::from_secs(30);

/// 保存時整形の上限の既定。**保存を長く待たせない**（超えたら整形せずに保存する）。
/// `TAKO_LSP_FORMAT_ON_SAVE_TIMEOUT_SECS` で変えられる
pub const DEFAULT_ON_SAVE_TIMEOUT: Duration = Duration::from_secs(5);

fn env_timeout(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map_or(default, Duration::from_secs)
}

/// 明示的な整形の上限
pub fn format_timeout() -> Duration {
    env_timeout("TAKO_LSP_FORMAT_TIMEOUT_SECS", DEFAULT_FORMAT_TIMEOUT)
}

/// 保存時整形の上限
pub fn on_save_timeout() -> Duration {
    env_timeout(
        "TAKO_LSP_FORMAT_ON_SAVE_TIMEOUT_SECS",
        DEFAULT_ON_SAVE_TIMEOUT,
    )
}

/// 問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで [`super::LspManager::format`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatRequest {
    pub path: PathBuf,
    /// 整形する本文（頼んだときの編集バッファの全文）。サーバの写しをこれに揃えてから頼み、
    /// 答えの座標もこれで直す
    pub text: String,
    /// 範囲の整形なら、`text` のバイト位置の範囲。`None` は文書全体
    pub range: Option<Range<usize>>,
    pub options: FormatOptions,
    pub timeout: Duration,
}

/// 問い合わせの答え（0 件 = 変える所が無い）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatAnswer {
    /// 答えたサーバの ID（検出表の `id`）
    pub server: &'static str,
    /// 並べて最小にした書き換え（`text_edit::order_changes` を通した後）。範囲は
    /// [`FormatRequest::text`] のバイト位置
    pub changes: Vec<TextChange>,
    /// 範囲の外の空白だけの書き換えを捨てた数（範囲の整形だけ）
    pub dropped: usize,
}

/// 問い合わせの失敗。**「変える所が無い」は失敗ではない**（[`FormatAnswer`] の 0 件）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// 言語サーバの状態として定義ジャンプと共通の失敗（無効・受け持つサーバが無い・未導入・
    /// 未応答・止めた / 諦めた・落ちた・閉じられた・エラーで答えた・読めない）。
    /// `GotoError::Unsupported` はここへは入れない（能力は [`Self::Unsupported`] が持つ）
    Lsp(GotoError),
    /// サーバが（範囲の）整形に対応していない
    Unsupported { server: &'static str, ranged: bool },
    /// 答えを待つあいだに本文が変わった（古い本文への答えは位置がずれる）
    Stale,
    /// 答えを当てられない（重なり・範囲外・形が違う）
    InvalidEdits {
        server: &'static str,
        detail: String,
    },
    /// 範囲の整形で、範囲の外へ空白以外の書き換えが来た
    OutsideRange { server: &'static str, count: usize },
}

impl FormatError {
    /// 応答の `status`（機械可読）
    pub fn status(&self) -> &'static str {
        match self {
            Self::Lsp(error) => error.status(),
            Self::Unsupported { .. } => "unsupported",
            Self::Stale => "stale",
            Self::InvalidEdits { .. } => "invalid-edits",
            Self::OutsideRange { .. } => "outside-range",
        }
    }

    /// 理由（日英は `text` の 1 か所）
    pub fn reason(&self) -> String {
        match self {
            // 共通の失敗の文は種類に依らない（種類を使うのは `Unsupported` だけで、それは入らない）
            Self::Lsp(error) => error.reason(GotoKind::Definition),
            Self::Unsupported { server, ranged } => text::fill(
                if *ranged {
                    text::FORMAT_RANGE_UNSUPPORTED_REASON
                } else {
                    text::FORMAT_UNSUPPORTED_REASON
                },
                &[("server", server)],
            ),
            Self::Stale => text::FORMAT_STALE_REASON.text().to_string(),
            Self::InvalidEdits { server, detail } => text::fill(
                text::FORMAT_INVALID_REASON,
                &[("server", server), ("detail", detail)],
            ),
            Self::OutsideRange { server, count } => text::fill(
                text::FORMAT_OUTSIDE_RANGE_REASON,
                &[("server", server), ("count", &count.to_string())],
            ),
        }
    }

    /// 次の一手
    pub fn next_step(&self) -> String {
        match self {
            Self::Lsp(error) => error.next_step(),
            Self::Unsupported { ranged: true, .. } => {
                text::FORMAT_RANGE_UNSUPPORTED_NEXT_STEP.text().to_string()
            }
            Self::Unsupported { ranged: false, .. } => {
                text::GOTO_UNSUPPORTED_NEXT_STEP.text().to_string()
            }
            Self::Stale => text::GOTO_RETRY_NEXT_STEP.text().to_string(),
            Self::InvalidEdits { .. } => text::FORMAT_INVALID_NEXT_STEP.text().to_string(),
            Self::OutsideRange { .. } => text::FORMAT_OUTSIDE_RANGE_NEXT_STEP.text().to_string(),
        }
    }

    /// 応答の JSON（`status` / `reason` / `next_step`。未導入なら `install_command` も）
    pub fn to_json(&self) -> Value {
        let mut out = match self {
            // 未導入の導入コマンド・サーバ名の載せ方は定義ジャンプと同じ 1 実装
            Self::Lsp(error) => error.to_json(GotoKind::Definition),
            _ => json!({ "status": self.status() }),
        };
        if let Some(map) = out.as_object_mut() {
            map.remove("kind");
        }
        out["status"] = json!(self.status());
        out["reason"] = json!(self.reason());
        out["next_step"] = json!(self.next_step());
        match self {
            Self::Unsupported { server, .. }
            | Self::InvalidEdits { server, .. }
            | Self::OutsideRange { server, .. } => out["server"] = json!(server),
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::lsp::state::ServerState;

    fn all_errors() -> Vec<FormatError> {
        vec![
            FormatError::Lsp(GotoError::Disabled),
            FormatError::Lsp(GotoError::NoServer),
            FormatError::Lsp(GotoError::NotInstalled {
                server: "s",
                reason: "r".into(),
                next_step: "n".into(),
                install_command: "c",
            }),
            FormatError::Lsp(GotoError::Timeout {
                server: "s",
                secs: 3,
                starting: true,
            }),
            FormatError::Lsp(GotoError::Unavailable {
                server: "s",
                state: ServerState::GaveUp,
                crashes: 3,
            }),
            FormatError::Lsp(GotoError::Crashed { server: "s" }),
            FormatError::Lsp(GotoError::ServerError {
                server: "s",
                code: -32603,
                detail: "d".into(),
            }),
            FormatError::Unsupported {
                server: "s",
                ranged: false,
            },
            FormatError::Unsupported {
                server: "s",
                ranged: true,
            },
            FormatError::Stale,
            FormatError::InvalidEdits {
                server: "s",
                detail: "d".into(),
            },
            FormatError::OutsideRange {
                server: "s",
                count: 2,
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
        // 未導入は導入コマンドを返す（定義ジャンプと同じ形）
        let value = all_errors()[2].to_json();
        assert_eq!(value["install_command"], json!("c"));
        assert_eq!(value["server"], json!("s"));
    }

    #[test]
    fn 範囲の整形に対応していないときは全体の整形へ案内する() {
        let error = FormatError::Unsupported {
            server: "s",
            ranged: true,
        };
        assert_eq!(
            error.next_step(),
            text::FORMAT_RANGE_UNSUPPORTED_NEXT_STEP.text()
        );
    }

    #[test]
    fn 上限は_env_で変えられ不正は既定へ落ちる() {
        // env を触るテストはこの 1 本だけ（並行する他のテストが同じ名前を読まない）
        std::env::set_var("TAKO_LSP_FORMAT_TIMEOUT_SECS", "0");
        assert_eq!(format_timeout(), DEFAULT_FORMAT_TIMEOUT);
        std::env::set_var("TAKO_LSP_FORMAT_TIMEOUT_SECS", "7");
        assert_eq!(format_timeout(), Duration::from_secs(7));
        std::env::remove_var("TAKO_LSP_FORMAT_TIMEOUT_SECS");
        assert_eq!(format_timeout(), DEFAULT_FORMAT_TIMEOUT);
        std::env::set_var("TAKO_LSP_FORMAT_ON_SAVE_TIMEOUT_SECS", "x");
        assert_eq!(on_save_timeout(), DEFAULT_ON_SAVE_TIMEOUT);
        std::env::remove_var("TAKO_LSP_FORMAT_ON_SAVE_TIMEOUT_SECS");
    }
}
