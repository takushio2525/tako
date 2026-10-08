//! ホバー（型・doc のカード。S4 / #1681）の問い合わせの型と応答の組み立て
//!
//! 問い合わせそのもの（文書を開く / 持ち手として加わる・サーバの起動を待つ・要求を投げる・
//! 古い要求を `$/cancelRequest` で捨てる）は状態を持つ [`super::manager`] の `Shared::hover` が
//! 行い、ここは**その入出力の形**だけを持つ: 要求（[`HoverRequest`]）・答え（[`HoverAnswer`]）・
//! 失敗（[`HoverError`]）と、CLI / MCP へ返す JSON。
//!
//! 応答の読み取り（3 形）・本文の上限・範囲の写しは純粋関数として `tako_core::lsp::hover` にある。
//! カード（Markdown は `md_view::render_block` を通す）は tako-app の `lsp_hover_ui.rs`。
//! **言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::completion::At;
use tako_core::lsp::hover::Hover;

use super::completion::at_json;
use super::goto::GotoError;
use super::text;

/// 問い合わせの上限の既定。起動と握手を含む（定義ジャンプ・補完と同じ。初回はサーバが
/// プロジェクトを読む時間が要る）。`TAKO_LSP_HOVER_TIMEOUT_SECS` で変えられる
/// （0 / 不正 / 空は既定へ落とす = #1503）
pub const DEFAULT_HOVER_TIMEOUT: Duration = Duration::from_secs(30);

/// マウスが識別子に乗ってから問い合わせるまでの待ち（デバウンス）の既定。なぞっていくあいだは
/// 問い合わせず、止まった 1 か所だけ（VS Code / Zed の既定と同じ 300ms）。
/// `TAKO_LSP_HOVER_DELAY_MS` で変えられる（不正 / 空は既定へ落とす。0 は「待たない」）
pub const DEFAULT_DELAY: Duration = Duration::from_millis(300);

/// 本番の問い合わせの上限
pub fn hover_timeout() -> Duration {
    std::env::var("TAKO_LSP_HOVER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map_or(DEFAULT_HOVER_TIMEOUT, Duration::from_secs)
}

/// 本番のデバウンス
pub fn delay() -> Duration {
    std::env::var("TAKO_LSP_HOVER_DELAY_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(DEFAULT_DELAY, Duration::from_millis)
}

/// `TAKO_1681_LEGACY=1` で **#1681 前の挙動**へ戻す（同一バイナリで A/B を取る入口）:
/// マウスを乗せても問い合わせない・カードを出さない（編集メニュー / パレットの「ホバー情報を
/// 表示」も出さない）。CLI / MCP の `tako lsp hover` は残る（戻すのは既存経路への影響だけ）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_1681_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// `TAKO_1893_LEGACY=1` で **#1893 前の挙動**へ戻す（同一バイナリで A/B を取る入口）:
/// 右クリックメニューに「ホバー情報を表示」を出さない・ホバーのキー（⇧⌘H / Ctrl+Shift+H）を
/// 張らない・マウスの要求はサーバの読み込みを待たずに空で終わる（「読み込み中」も出さない）。
/// CLI / MCP の `limit`（全文の口）は残る（戻すのは既存経路への影響だけ）
pub fn legacy_1893() -> bool {
    matches!(
        std::env::var("TAKO_1893_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで [`super::LspManager::hover`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverRequest {
    /// 問い合わせる文書
    pub path: PathBuf,
    /// tako の座標（0 起点の行・行内の UTF-8 バイト桁）。LSP の座標（UTF-16）へは manager が
    /// **サーバが見ている本文**で直す（#1769: 単独の `\r` の後ろは LSP では次の行）
    pub line: usize,
    pub column: usize,
    pub timeout: Duration,
    /// 文書を開いていないときにサーバへ渡す本文（編集セッションの全文）。`None` ならディスクから読む
    pub document: Option<String>,
    /// マウスの要求（GUI）か。`true` の要求は**次の `true` の要求が来たら取り消される**
    /// （`$/cancelRequest` を送り、待っている側は [`HoverError::Superseded`] で返る）。
    /// CLI / MCP・メニューの要求は `false`（1 回ずつ答えを待つ。互いに取り消し合わない）
    pub superseding: bool,
    /// 文書が開いていなければ問い合わせのあいだだけ開くか。**マウスの要求は `false`**
    /// （乗せただけでサーバを起こさない = 設計書 §16-2。開いていなければ [`HoverError::NotOpen`]）
    pub open: bool,
    /// `superseding` の要求の取り消しの番号を、UI スレッドで先に取ったもの
    /// （[`super::LspManager::reserve_hover`]。#1893）。`None` なら manager が背景で取る。
    /// 背景で取ると、その前に UI が語から外れて取り消した（`cancel_hover`）のを**追い越して**
    /// 自分が最新になり、読み込みが済むまで待ち続ける（#1893 の visual-test で実測した競合）
    pub ticket: Option<u64>,
}

/// 問い合わせの答え（`content` が `None` = サーバは答えたが表示するものが無い。失敗ではない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverAnswer {
    /// 答えたサーバの ID（検出表の `id`）
    pub server: &'static str,
    /// 本文（**全文**。切るのは出口 = カードは `MAX_CHARS`・CLI / MCP は `limit`。#1893）
    pub content: Option<Hover>,
    /// `Hover.range` を tako の座標へ写したもの（始点・終点。サーバが付けたときだけ）
    pub range: Option<(At, At)>,
    /// サーバの読み込みを待ってから答えたなら、問い合わせ全体にかかった時間（#1893。補完の
    /// `waited_for_loading` と同じ = CLI / MCP の `waited_for_loading_ms`）
    pub waited_for_loading: Option<Duration>,
}

/// 問い合わせの失敗
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverError {
    /// 言語サーバの状態（未導入・未応答・落ちた…）。定義ジャンプと同じ型
    Query(GotoError),
    /// 次のマウスの要求に置き換わった（GUI の内側でだけ起きる）
    Superseded,
    /// 文書が言語サーバにつながっていない（`open: false` の要求 = マウスのホバー）
    NotOpen,
    /// サーバが上限（`secs` 秒）までプロジェクトを読み込み中だった（#1893。補完の
    /// `CompletionError::Loading` と同じ = 空で答え続けた / 答えずに待たせた）。失敗ではなく「まだ」
    Loading { server: &'static str, secs: u64 },
}

impl From<GotoError> for HoverError {
    fn from(error: GotoError) -> Self {
        Self::Query(error)
    }
}

impl HoverError {
    /// 応答の `status`（機械可読）
    pub fn status(&self) -> &'static str {
        match self {
            Self::Query(error) => error.status(),
            Self::Superseded => "superseded",
            Self::NotOpen => "not-open",
            Self::Loading { .. } => "loading",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::Query(error) => {
                error.reason_in(text::HOVER_UNSUPPORTED_REASON, text::HOVER_LABEL.text())
            }
            Self::Superseded => text::HOVER_SUPERSEDED_REASON.text().to_string(),
            Self::NotOpen => text::HOVER_NOT_OPEN_REASON.text().to_string(),
            // 読み込み中の文は補完と同じ（機能の名前を含まない = 同じ状態に別の言い回しを作らない）
            Self::Loading { server, secs } => text::fill(
                text::COMPLETION_LOADING_REASON,
                &[("server", server), ("secs", &secs.to_string())],
            ),
        }
    }

    pub fn next_step(&self) -> String {
        match self {
            Self::Query(error) => error.next_step(),
            Self::Superseded => text::GOTO_RETRY_NEXT_STEP.text().to_string(),
            Self::NotOpen => text::HOVER_NOT_OPEN_NEXT_STEP.text().to_string(),
            Self::Loading { .. } => text::COMPLETION_LOADING_NEXT_STEP.text().to_string(),
        }
    }

    /// 応答の JSON（`status` / `reason` / `next_step`。未導入なら `install_command` も）
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "status": self.status(),
            "action": "hover",
            "reason": self.reason(),
            "next_step": self.next_step(),
        });
        match self {
            Self::Query(error) => error.add_server(&mut out),
            Self::Loading { server, .. } => out["server"] = json!(server),
            Self::Superseded | Self::NotOpen => {}
        }
        out
    }
}

/// 表示するものが無い応答（サーバは答えたが空）
pub fn none_json(server: &str) -> Value {
    json!({
        "status": "none",
        "action": "hover",
        "server": server,
        "reason": text::HOVER_NONE_REASON.text(),
        "next_step": text::HOVER_NONE_NEXT_STEP.text(),
    })
}

/// 答えの wire 形。`contents` は本文（Markdown / 平文のまま）、`kind` はその種類（LSP の綴り）。
/// 範囲は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）。
/// 本文は `limit`（CLI / MCP の `limit`。省略 = `MAX_CHARS`・0 = 全文 = `tako_core::lsp::hover::char_limit`）
/// までにし、切ったら `truncated` と `total_chars` と注記（全文の取り方つき）を載せる（#1893）
pub fn found_json(
    server: &str,
    content: &Hover,
    range: Option<(At, At)>,
    limit: Option<usize>,
) -> Value {
    let chars = tako_core::lsp::hover::char_limit(limit);
    let content = content.limited(chars);
    let mut out = json!({
        "status": "found",
        "action": "hover",
        "server": server,
        "kind": content.markup.slug(),
        "contents": content.value,
    });
    if let Some((start, end)) = range {
        out["range"] = json!({ "start": at_json(start), "end": at_json(end) });
    }
    if let (true, Some(chars)) = (content.truncated, chars) {
        out["truncated"] = json!(true);
        out["total_chars"] = json!(content.total_chars);
        out["note"] = json!(text::fill(
            text::HOVER_TRUNCATED_NOTE,
            &[
                ("total", &content.total_chars.to_string()),
                ("limit", &chars.to_string()),
            ],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::lsp::hover::Markup;
    use tako_core::lsp::state::ServerState;

    fn all_errors() -> Vec<HoverError> {
        vec![
            HoverError::Superseded,
            HoverError::NotOpen,
            HoverError::Loading {
                server: "s",
                secs: 3,
            },
            GotoError::Disabled.into(),
            GotoError::NoServer.into(),
            GotoError::NotInstalled {
                server: "s",
                reason: "r".into(),
                next_step: "n".into(),
                install_command: "c",
            }
            .into(),
            GotoError::Unsupported { server: "s" }.into(),
            GotoError::Timeout {
                server: "s",
                secs: 3,
                starting: true,
            }
            .into(),
            GotoError::Unavailable {
                server: "s",
                state: ServerState::GaveUp,
                crashes: 3,
            }
            .into(),
            GotoError::Crashed { server: "s" }.into(),
            GotoError::Closed.into(),
            GotoError::ServerError {
                server: "s",
                code: -32603,
                detail: "d".into(),
            }
            .into(),
            GotoError::Unreadable { error: "e".into() }.into(),
        ]
    }

    #[test]
    fn 失敗はどれも理由と次の一手を持つ() {
        for error in all_errors() {
            let value = error.to_json();
            assert_eq!(value["status"], json!(error.status()));
            assert_eq!(value["action"], json!("hover"));
            for key in ["reason", "next_step"] {
                let s = value[key].as_str().unwrap_or("");
                assert!(!s.is_empty(), "{error:?} の {key} が空");
                assert!(!s.contains('{'), "{error:?} の {key} に差し込み漏れ: {s}");
            }
        }
        // 能力に無いときはホバーの文（定義ジャンプの「ジャンプ」の文を使わない）
        let unsupported = HoverError::from(GotoError::Unsupported { server: "srv" });
        assert_eq!(
            unsupported.reason(),
            text::fill(text::HOVER_UNSUPPORTED_REASON, &[("server", "srv")])
        );
        // 未導入は導入コマンドまで載る（#983）
        let value = HoverError::from(GotoError::NotInstalled {
            server: "s",
            reason: "r".into(),
            next_step: "n".into(),
            install_command: "brew install x",
        })
        .to_json();
        assert_eq!(value["install_command"], json!("brew install x"));
    }

    #[test]
    fn 答えの_wire_形は本文と種類と範囲を返す() {
        let content = Hover {
            markup: Markup::PlainText,
            value: "a < b".into(),
            truncated: false,
            total_chars: 5,
            range: None,
        };
        let value = found_json("srv", &content, Some((At::new(2, 4), At::new(2, 9))), None);
        assert_eq!(value["status"], json!("found"));
        assert_eq!(value["kind"], json!("plaintext"));
        assert_eq!(value["contents"], json!("a < b"));
        // 行は 1 始まり・桁は 0 始まりの UTF-8 バイト（`tako edit replace-range` と同じ）
        assert_eq!(value["range"]["start"], json!({"line": 3, "column": 4}));
        assert_eq!(value["range"]["end"], json!({"line": 3, "column": 9}));
        assert!(value.get("truncated").is_none(), "切っていなければ載せない");
        assert_eq!(none_json("srv")["status"], json!("none"));
    }

    /// #1893: `limit` で本文の上限を変えられる（省略 = 16,000 字・0 = 全文・N = N 字）。
    /// 切ったときの注記は実際に使った上限と、全文の取り方を言う
    #[test]
    fn limit_で全文も上限の指定も返せる() {
        let big: String = (0..2000)
            .map(|i| format!("line {i} of the doc\n"))
            .collect();
        let total = big.chars().count();
        assert!(total > tako_core::lsp::hover::MAX_CHARS);
        let content = Hover {
            markup: Markup::Markdown,
            value: big.clone(),
            truncated: false,
            total_chars: total,
            range: None,
        };
        // 省略: 既定の上限で切って印と注記
        let value = found_json("srv", &content, None, None);
        assert_eq!(value["truncated"], json!(true));
        assert_eq!(value["total_chars"], json!(total));
        let shown = value["contents"].as_str().unwrap();
        assert!(shown.chars().count() <= tako_core::lsp::hover::MAX_CHARS);
        assert!(big.starts_with(shown));
        let note = value["note"].as_str().unwrap();
        assert!(note.contains(&total.to_string()), "{note}");
        assert!(note.contains("16000"), "{note}");
        assert!(note.contains("--full"), "全文の取り方を言う: {note}");
        // 0 = 全文（印も注記も載せない）
        let value = found_json("srv", &content, None, Some(0));
        assert_eq!(value["contents"], json!(big));
        for key in ["truncated", "total_chars", "note"] {
            assert!(value.get(key).is_none(), "{key}");
        }
        // N = N 字まで（注記の上限もその数）
        let value = found_json("srv", &content, None, Some(100));
        assert!(value["contents"].as_str().unwrap().chars().count() <= 100);
        assert!(value["note"].as_str().unwrap().contains("100"));
        assert!(value.get("range").is_none());
    }

    #[test]
    fn 上限とデバウンスは_env_で変えられ不正は既定へ落ちる() {
        // env を触るテストはこの 1 本だけ（並行する他のテストが同じ名前を読まない）
        std::env::set_var("TAKO_LSP_HOVER_TIMEOUT_SECS", "0");
        assert_eq!(hover_timeout(), DEFAULT_HOVER_TIMEOUT);
        std::env::set_var("TAKO_LSP_HOVER_TIMEOUT_SECS", "4");
        assert_eq!(hover_timeout(), Duration::from_secs(4));
        std::env::remove_var("TAKO_LSP_HOVER_TIMEOUT_SECS");
        std::env::set_var("TAKO_LSP_HOVER_DELAY_MS", "x");
        assert_eq!(delay(), DEFAULT_DELAY);
        std::env::set_var("TAKO_LSP_HOVER_DELAY_MS", "0");
        assert_eq!(delay(), Duration::ZERO);
        std::env::remove_var("TAKO_LSP_HOVER_DELAY_MS");
        assert_eq!(delay(), DEFAULT_DELAY);
    }
}
