//! 定義ジャンプの問い合わせ（S3 / #1680）の型と応答の組み立て
//!
//! 問い合わせそのもの（文書を開く・サーバの起動を待つ・要求を投げる）は状態を持つ
//! [`super::manager`] の `Shared::goto` が行い、ここは**その入出力の形**だけを持つ:
//! 要求（[`GotoRequest`]）・答え（[`GotoAnswer`]）・失敗（[`GotoError`]）と、応答の JSON。
//!
//! 着地（どのペインで開くか）は `crate::dispatch` が `tako_core::lsp::goto::plan_landing`
//! で決める。**言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::state::ServerState;
use tako_core::platform::support::Note;

use super::text;

/// 問い合わせの上限の既定。起動と握手を含む（初回はサーバがプロジェクトを読む時間が要る）。
/// 無限に待たない（`.agent/conventions.md`「外部コマンドを待つときは上限を持つ（#1503）」）。
/// `TAKO_LSP_GOTO_TIMEOUT_SECS` で変えられる（0 / 不正 / 空は既定へ落とす）
pub const DEFAULT_GOTO_TIMEOUT: Duration = Duration::from_secs(30);

/// 起動を待つあいだに状態を読み直す間隔（待つのは問い合わせ中だけ。アイドル時は何もしない）
pub const READY_POLL: Duration = Duration::from_millis(20);

/// サーバが「読み込みが済んだか」を知らせる通知（rust-analyzer 等の拡張。#1680）。
/// 読み込みの前の問い合わせに**空で答える**サーバがあり、それを「見つからない」と
/// 読み違えないために受ける（LSP 標準の `$/progress` は検査（`cargo check` 等）の進捗まで
/// 含むので、それを待つと答えが遅れすぎる）
pub const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// 状態を 1 度も知らせないサーバを「送らないサーバ」とみなすまでの猶予（握手の直後から数える）
pub const STATUS_GRACE: Duration = Duration::from_secs(2);

/// クライアントの `experimental` 能力（[`SERVER_STATUS_METHOD`] を送ってもらう）
pub fn experimental_capabilities() -> Value {
    json!({ "serverStatusNotification": true })
}

/// `experimental/serverStatus` の `quiescent`（無ければ `None` = 分からない）
pub fn quiescent_of(params: &Value) -> Option<bool> {
    params.get("quiescent").and_then(Value::as_bool)
}

/// 空の答えのあとに待った結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loading {
    /// 読み込みが済んだ（待った）ので問い直す
    Retry,
    /// 済んでいる / 状態を送らないサーバ = 本当に見つからない
    Settled,
    /// 上限までに済まなかった
    TimedOut,
}

/// 候補の一覧に載せる行の抜粋の上限（文字数）
pub const EXCERPT_CHARS: usize = 160;

/// 本番の上限（env で値だけを変えられる）
pub fn goto_timeout() -> Duration {
    std::env::var("TAKO_LSP_GOTO_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map_or(DEFAULT_GOTO_TIMEOUT, Duration::from_secs)
}

/// 問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで [`super::LspManager::goto`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoRequest {
    pub kind: GotoKind,
    /// 問い合わせる文書
    pub path: PathBuf,
    /// tako の座標（0 起点の行・行内の UTF-8 バイト桁）。LSP の座標（UTF-16）へは
    /// manager が**サーバが見ている本文**で直す（#1769: 単独の `\r` の後ろは LSP では次の行）
    pub line: usize,
    pub column: usize,
    pub timeout: Duration,
    /// 文書を開いていないときにサーバへ渡す本文（編集セッションの全文）。`None` ならディスクから読む
    pub document: Option<String>,
}

/// 飛び先 1 か所（tako の座標へ直したもの）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoTarget {
    pub uri: String,
    pub path: PathBuf,
    /// 0 起点の行
    pub line: usize,
    /// 行内の UTF-8 バイト（`tako edit replace-range` と同じ桁）
    pub column: usize,
    /// 行内の文字数での桁（0 起点。`OpenFile` の `column` は 1 始まりの文字なので +1 して渡す）
    pub char_column: usize,
    /// 候補の一覧に出す、その行の抜粋（前後の空白を落として [`EXCERPT_CHARS`] 字まで）
    pub excerpt: String,
}

/// 問い合わせの答え（0 件 = 見つからない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoAnswer {
    /// 答えたサーバの ID（検出表の `id`）
    pub server: &'static str,
    pub targets: Vec<GotoTarget>,
}

/// 問い合わせの失敗。**「見つからない」は失敗ではない**（[`GotoAnswer`] の 0 件）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GotoError {
    /// `TAKO_1007_LEGACY=1` で LSP を止めている
    Disabled,
    /// この種類のファイルを受け持つサーバが検出表に無い
    NoServer,
    /// サーバが未導入（理由と導入コマンドは manager が組む = #983 の作法）
    NotInstalled {
        server: &'static str,
        reason: String,
        next_step: String,
        install_command: &'static str,
    },
    /// サーバがその種類に対応していない（能力に無い）
    Unsupported { server: &'static str },
    /// 上限までに答えが来なかった。`starting` = 起動と握手が終わらなかった
    Timeout {
        server: &'static str,
        secs: u64,
        starting: bool,
    },
    /// 止めた / 諦めた（restart まで起きない）
    Unavailable {
        server: &'static str,
        state: ServerState,
        crashes: u32,
    },
    /// 問い合わせの途中で落ちた
    Crashed { server: &'static str },
    /// 問い合わせの途中で文書が閉じられた
    Closed,
    /// サーバがエラーで答えた
    ServerError {
        server: &'static str,
        code: i64,
        detail: String,
    },
    /// 文書を読めない（開いていない文書をサーバへ渡すため）
    Unreadable { error: String },
}

impl GotoError {
    /// 応答の `status`（機械可読）
    pub fn status(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoServer => "no-server",
            Self::NotInstalled { .. } => "not-installed",
            Self::Unsupported { .. } => "unsupported",
            Self::Timeout { .. } => "timeout",
            Self::Unavailable { .. } => "unavailable",
            Self::Crashed { .. } => "crashed",
            Self::Closed => "closed",
            Self::ServerError { .. } => "server-error",
            Self::Unreadable { .. } => "unreadable",
        }
    }

    /// 理由（日英は `text` の 1 か所）
    pub fn reason(&self, kind: GotoKind) -> String {
        self.reason_in(text::GOTO_UNSUPPORTED_REASON, kind_label(kind))
    }

    /// 理由。「能力に無い」の文だけは機能ごとに違うので呼び手が渡す（`{server}` と `{kind}` を
    /// 差し込む。#1682 の補完も同じ失敗の型を使う）。それ以外は言語サーバの状態として同じ文
    pub fn reason_in(&self, unsupported: Note, label: &str) -> String {
        match self {
            Self::Disabled => text::DISABLED_REASON.text().to_string(),
            Self::NoServer => text::GOTO_NO_SERVER_REASON.text().to_string(),
            Self::NotInstalled { reason, .. } => reason.clone(),
            Self::Unsupported { server } => {
                text::fill(unsupported, &[("server", server), ("kind", label)])
            }
            Self::Timeout {
                server,
                secs,
                starting,
            } => text::fill(
                if *starting {
                    text::GOTO_START_TIMEOUT_REASON
                } else {
                    text::GOTO_TIMEOUT_REASON
                },
                &[("server", server), ("secs", &secs.to_string())],
            ),
            Self::Unavailable { state, crashes, .. } => match state {
                ServerState::GaveUp => {
                    text::fill(text::GAVE_UP_REASON, &[("count", &crashes.to_string())])
                }
                _ => text::STOPPED_REASON.text().to_string(),
            },
            Self::Crashed { server } => {
                text::fill(text::GOTO_CRASHED_REASON, &[("server", server)])
            }
            Self::Closed => text::GOTO_CLOSED_REASON.text().to_string(),
            Self::ServerError {
                server,
                code,
                detail,
            } => text::fill(
                text::GOTO_SERVER_ERROR_REASON,
                &[
                    ("server", server),
                    ("code", &code.to_string()),
                    ("detail", detail),
                ],
            ),
            Self::Unreadable { error } => {
                text::fill(text::GOTO_UNREADABLE_REASON, &[("error", error)])
            }
        }
    }

    /// 次の一手
    pub fn next_step(&self) -> String {
        match self {
            Self::Disabled | Self::Closed | Self::Unreadable { .. } => {
                text::GOTO_RETRY_NEXT_STEP.text().to_string()
            }
            Self::NoServer => text::GOTO_NO_SERVER_NEXT_STEP.text().to_string(),
            Self::NotInstalled { next_step, .. } => next_step.clone(),
            Self::Unsupported { .. } => text::GOTO_UNSUPPORTED_NEXT_STEP.text().to_string(),
            Self::Timeout { .. } => text::GOTO_TIMEOUT_NEXT_STEP.text().to_string(),
            Self::Unavailable { state, .. } => match state {
                ServerState::GaveUp => text::GAVE_UP_NEXT_STEP.text().to_string(),
                _ => text::STOPPED_NEXT_STEP.text().to_string(),
            },
            Self::Crashed { .. } | Self::ServerError { .. } => {
                text::GOTO_CRASHED_NEXT_STEP.text().to_string()
            }
        }
    }

    /// 応答の JSON（`status` / `reason` / `next_step`。未導入なら `install_command` も）
    pub fn to_json(&self, kind: GotoKind) -> Value {
        let mut out = json!({
            "status": self.status(),
            "kind": kind.slug(),
            "reason": self.reason(kind),
            "next_step": self.next_step(),
        });
        self.add_server(&mut out);
        out
    }

    /// 応答へ答えたサーバ（と未導入なら導入コマンド）を足す（#1682 の補完と共有）
    pub fn add_server(&self, out: &mut Value) {
        match self {
            Self::NotInstalled {
                server,
                install_command,
                ..
            } => {
                out["server"] = json!(server);
                out["install_command"] = json!(install_command);
            }
            Self::Unsupported { server }
            | Self::Timeout { server, .. }
            | Self::Unavailable { server, .. }
            | Self::Crashed { server }
            | Self::ServerError { server, .. } => out["server"] = json!(server),
            _ => {}
        }
    }
}

/// 種類の呼び名（`text::GOTO_KIND_LABELS` を `GotoKind::ALL` の順で引く）
pub fn kind_label(kind: GotoKind) -> &'static str {
    let index = GotoKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_default();
    text::GOTO_KIND_LABELS[index].text()
}

/// 見つからないの応答（サーバは答えたが 0 件）
pub fn not_found_json(kind: GotoKind, server: &str) -> Value {
    let label = kind_label(kind);
    json!({
        "status": "not-found",
        "kind": kind.slug(),
        "server": server,
        "reason": text::fill(text::GOTO_NOT_FOUND_REASON, &[("kind", label)]),
        "next_step": text::GOTO_NOT_FOUND_NEXT_STEP.text(),
        "locations": [],
    })
}

/// 飛び先 1 か所の wire 形。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの UTF-8 バイト）なので、そのまま範囲編集や次の問い合わせへ渡せる
pub fn target_json(target: &GotoTarget) -> Value {
    json!({
        "path": target.path.display().to_string(),
        "line": target.line + 1,
        "column": target.column,
        "uri": target.uri,
        "text": target.excerpt,
    })
}

/// 飛び先の座標を tako の形へ直す（LSP の UTF-16 桁 → 行内の UTF-8 バイト・文字数）。
///
/// `text` は**サーバが見ている本文**（開いている文書なら送った写し、そうでなければ
/// ディスクの中身）。読めなかった（`None`）ときは桁を ASCII とみなして返す
/// （開く側が行・桁を丸める。候補の一覧の抜粋は空になる）
pub fn locate(
    uri: String,
    path: PathBuf,
    line: usize,
    character: usize,
    text: Option<&str>,
) -> GotoTarget {
    let Some(text) = text else {
        return GotoTarget {
            uri,
            path,
            line,
            column: character,
            char_column: character,
            excerpt: String::new(),
        };
    };
    let byte = tako_core::lsp::position::byte_offset_of_lsp_position(text, line, character);
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[byte..].find('\n').map_or(text.len(), |i| byte + i);
    let row = text[..line_start].bytes().filter(|&b| b == b'\n').count();
    let excerpt: String = text[line_start..line_end]
        .trim_end_matches('\r')
        .trim()
        .chars()
        .take(EXCERPT_CHARS)
        .collect();
    GotoTarget {
        uri,
        path,
        line: row,
        column: byte - line_start,
        char_column: text[line_start..byte].chars().count(),
        excerpt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::i18n::Lang;

    fn all_errors() -> Vec<GotoError> {
        vec![
            GotoError::Disabled,
            GotoError::NoServer,
            GotoError::NotInstalled {
                server: "s",
                reason: "r".into(),
                next_step: "n".into(),
                install_command: "c",
            },
            GotoError::Unsupported { server: "s" },
            GotoError::Timeout {
                server: "s",
                secs: 3,
                starting: false,
            },
            GotoError::Timeout {
                server: "s",
                secs: 3,
                starting: true,
            },
            GotoError::Unavailable {
                server: "s",
                state: ServerState::GaveUp,
                crashes: 3,
            },
            GotoError::Unavailable {
                server: "s",
                state: ServerState::Stopped,
                crashes: 0,
            },
            GotoError::Crashed { server: "s" },
            GotoError::Closed,
            GotoError::ServerError {
                server: "s",
                code: -32603,
                detail: "d".into(),
            },
            GotoError::Unreadable { error: "e".into() },
        ]
    }

    #[test]
    fn 失敗はどれも理由と次の一手を持つ() {
        for error in all_errors() {
            let value = error.to_json(GotoKind::Definition);
            assert_eq!(value["status"], json!(error.status()));
            for key in ["reason", "next_step"] {
                let s = value[key].as_str().unwrap_or("");
                assert!(!s.is_empty(), "{error:?} の {key} が空");
                assert!(!s.contains('{'), "{error:?} の {key} に差し込み漏れ: {s}");
            }
        }
    }

    #[test]
    fn 種類の呼び名は日英とも_4_つ別() {
        for lang in [Lang::Ja, Lang::En] {
            let mut labels: Vec<&str> = text::GOTO_KIND_LABELS
                .iter()
                .map(|n| n.text_in(lang))
                .collect();
            labels.sort_unstable();
            labels.dedup();
            assert_eq!(labels.len(), GotoKind::ALL.len());
        }
    }

    #[test]
    fn 座標は_utf16_からバイトと文字数へ直す() {
        // 1 行目: 絵文字（UTF-16 で 2・UTF-8 で 4）の後ろの識別子
        let text = "fn a() {}\n  😀 名前\r\nlast";
        let t = locate("u".into(), PathBuf::from("/x"), 1, 5, Some(text));
        // "  😀 " = UTF-16 で 2 + 2 + 1 = 5 → バイトは 2 + 4 + 1 = 7、文字は 4
        assert_eq!((t.line, t.column, t.char_column), (1, 7, 4));
        assert_eq!(t.excerpt, "😀 名前");
        // 行末を超える桁は行末へ・最終行を超える行は本文の末尾へ丸める
        let t = locate("u".into(), PathBuf::from("/x"), 0, 99, Some(text));
        assert_eq!((t.line, t.column), (0, 9));
        let t = locate("u".into(), PathBuf::from("/x"), 9, 0, Some(text));
        assert_eq!((t.line, t.column), (2, 4));
        // 読めなければ桁をそのまま使う
        let t = locate("u".into(), PathBuf::from("/x"), 3, 2, None);
        assert_eq!((t.line, t.column, t.char_column), (3, 2, 2));
        assert!(t.excerpt.is_empty());
    }

    #[test]
    fn 抜粋は上限の文字数で切る() {
        let long = "あ".repeat(EXCERPT_CHARS + 20);
        let t = locate("u".into(), PathBuf::from("/x"), 0, 0, Some(&long));
        assert_eq!(t.excerpt.chars().count(), EXCERPT_CHARS);
    }

    #[test]
    fn 上限は_env_で変えられ不正は既定へ落ちる() {
        // env を触るテストはこの 1 本だけ（並行する他のテストが同じ名前を読まない）
        std::env::set_var("TAKO_LSP_GOTO_TIMEOUT_SECS", "0");
        assert_eq!(goto_timeout(), DEFAULT_GOTO_TIMEOUT);
        std::env::set_var("TAKO_LSP_GOTO_TIMEOUT_SECS", "7");
        assert_eq!(goto_timeout(), Duration::from_secs(7));
        std::env::remove_var("TAKO_LSP_GOTO_TIMEOUT_SECS");
        assert_eq!(goto_timeout(), DEFAULT_GOTO_TIMEOUT);
    }
}
