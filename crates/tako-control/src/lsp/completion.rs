//! 補完（予測変換。S5 / #1682）の問い合わせの型と応答の組み立て
//!
//! 問い合わせそのもの（文書を開く・サーバの起動を待つ・要求を投げる・古い要求を
//! `$/cancelRequest` で捨てる）は状態を持つ [`super::manager`] の `Shared::completion` が行い、
//! ここは**その入出力の形**だけを持つ: 要求（[`CompletionRequest`]）・答え（[`CompletionAnswer`]）・
//! 失敗（[`CompletionError`]）と、CLI / MCP へ返す JSON。
//!
//! 応答の読み取り・絞り込み・キーの振り分け表・版の照合は純粋関数として
//! `tako_core::lsp::completion` にある。確定（本文へ入れる）は `crate::dispatch` の
//! `lsp_completion_apply`（GUI の Enter と CLI の `--choice` が同じ 1 本を通る）。
//! **言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::completion::{kind_slug, At, CompletionItem, Trigger};

use super::goto::GotoError;
use super::text;

/// 問い合わせの上限の既定。起動と握手を含む（定義ジャンプと同じ。初回はサーバが
/// プロジェクトを読む時間が要る）。`TAKO_LSP_COMPLETION_TIMEOUT_SECS` で変えられる
/// （0 / 不正 / 空は既定へ落とす = #1503）
pub const DEFAULT_COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

/// `completionItem/resolve` の上限（説明を補うだけ。遅ければ説明なしで一覧を出し続ける）
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// 打鍵から要求を出すまでの待ち（デバウンス）の既定。打ち続けている間は要求を出さず、
/// 指が止まった 1 回だけ問い合わせる（打鍵のたびに投げると古い答えが遅れて届く）。
/// `TAKO_LSP_COMPLETION_DEBOUNCE_MS` で変えられる（不正 / 空は既定へ落とす。0 は「待たない」）
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(80);

/// 一覧の 1 候補の説明を JSON へ載せる上限の文字数（MCP の応答を膨らませない）
pub const JSON_DOC_CHARS: usize = 400;

/// 本番の問い合わせの上限
pub fn completion_timeout() -> Duration {
    std::env::var("TAKO_LSP_COMPLETION_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map_or(DEFAULT_COMPLETION_TIMEOUT, Duration::from_secs)
}

/// 本番のデバウンス
pub fn debounce() -> Duration {
    std::env::var("TAKO_LSP_COMPLETION_DEBOUNCE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(DEFAULT_DEBOUNCE, Duration::from_millis)
}

/// `TAKO_1682_LEGACY=1` で **#1682 前の挙動**へ戻す（同一バイナリで A/B を取る入口）:
/// 打鍵しても補完を問い合わせない・一覧を出さない（5 キーは従来どおり本文 / 検索欄へ）。
/// CLI / MCP の `tako lsp completion` は残る（戻すのは既存経路への影響だけ）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_1682_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで [`super::LspManager::completion`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    /// 問い合わせる文書
    pub path: PathBuf,
    /// tako の座標（0 起点の行・行内の UTF-8 バイト桁）。LSP の座標（UTF-16）へは manager が
    /// **サーバが見ている本文**で直す（#1769: 単独の `\r` の後ろは LSP では次の行）
    pub line: usize,
    pub column: usize,
    pub timeout: Duration,
    /// 文書を開いていないときにサーバへ渡す本文（編集セッションの全文）。`None` ならディスクから読む
    pub document: Option<String>,
    pub trigger: Trigger,
    /// 打鍵の要求（GUI）か。`true` の要求は**次の `true` の要求が来たら取り消される**
    /// （`$/cancelRequest` を送り、待っている側は [`CompletionError::Superseded`] で返る）。
    /// CLI / MCP の要求は `false`（1 回ずつ答えを待つ。互いに取り消し合わない）
    pub superseding: bool,
    /// 絞り込み後の上位この件数の説明を `completionItem/resolve` で補ってから返す
    /// （CLI / MCP の `resolve`。0 = 補わない。GUI は選んだ 1 件だけを後から補う）
    pub resolve_top: usize,
}

/// 問い合わせの答え（0 件もここ = 失敗ではない）
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionAnswer {
    /// 答えたサーバの ID（検出表の `id`）
    pub server: &'static str,
    /// 候補（tako の座標へ写したもの。並びはサーバの順のまま = 絞り込みと並べ替えは
    /// `tako_core::lsp::completion::rank`）
    pub items: Vec<CompletionItem>,
    /// `CompletionList.isIncomplete`
    pub is_incomplete: bool,
    /// 上限（`tako_core::lsp::completion::MAX_ITEMS`）を超えて捨てた数
    pub dropped: usize,
    /// 問い合わせた位置と、その行の本文（サーバが見ていた本文。絞り込みの問い合わせを切る）
    pub cursor: At,
    pub line_text: String,
    /// サーバが `completionItem/resolve` を持つか（説明を後から補える）
    pub resolvable: bool,
}

/// 問い合わせの失敗
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionError {
    /// 言語サーバの状態（未導入・未応答・落ちた…）。定義ジャンプと同じ型
    Query(GotoError),
    /// 次の打鍵の要求に置き換わった（GUI の内側でだけ起きる）
    Superseded,
    /// 問い合わせのあいだに本文が変わった（候補の位置が今の本文と合わない）
    Edited,
}

impl From<GotoError> for CompletionError {
    fn from(error: GotoError) -> Self {
        Self::Query(error)
    }
}

impl CompletionError {
    /// 応答の `status`（機械可読）
    pub fn status(&self) -> &'static str {
        match self {
            Self::Query(error) => error.status(),
            Self::Superseded => "superseded",
            Self::Edited => "edited",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::Query(error) => error.reason_in(
                text::COMPLETION_UNSUPPORTED_REASON,
                text::COMPLETION_LABEL.text(),
            ),
            Self::Superseded => text::COMPLETION_SUPERSEDED_REASON.text().to_string(),
            Self::Edited => text::COMPLETION_EDITED_REASON.text().to_string(),
        }
    }

    pub fn next_step(&self) -> String {
        match self {
            Self::Query(error) => error.next_step(),
            Self::Superseded | Self::Edited => text::COMPLETION_EDITED_NEXT_STEP.text().to_string(),
        }
    }

    /// 応答の JSON（`status` / `reason` / `next_step`。未導入なら `install_command` も）
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "status": self.status(),
            "action": "completion",
            "reason": self.reason(),
            "next_step": self.next_step(),
        });
        if let Self::Query(error) = self {
            error.add_server(&mut out);
        }
        out
    }
}

/// 候補が 0 件の応答（サーバは答えたが、絞り込み後に 1 件も残らない）
pub fn none_json(server: &str) -> Value {
    json!({
        "status": "none",
        "action": "completion",
        "server": server,
        "reason": text::COMPLETION_NONE_REASON.text(),
        "next_step": text::COMPLETION_NONE_NEXT_STEP.text(),
        "total": 0,
        "items": [],
    })
}

/// tako の座標の wire 形（`tako edit replace-range` と同じ = 行 1 始まり・桁 0 始まりの UTF-8 バイト）
pub fn at_json(at: At) -> Value {
    json!({ "line": at.line + 1, "column": at.col })
}

/// 候補 1 つの wire 形。`number` は絞り込み後の全体の並びでの番号（1 始まり。`--choice` と同じ）、
/// `cursor` は問い合わせた位置。範囲は確定したときに置き換わる所（始点 〜 カーソル + 後ろの分）
pub fn item_json(item: &CompletionItem, number: usize, cursor: At) -> Value {
    let end = At::new(cursor.line, cursor.col + item.tail);
    let mut out = json!({
        "number": number,
        "label": item.label,
        "kind": item.kind.map(kind_slug),
        "insert_text": item.insert_text,
        "range": { "start": at_json(item.start), "end": at_json(end) },
    });
    if let Some(detail) = &item.detail {
        out["detail"] = json!(detail);
    }
    if let Some(doc) = &item.documentation {
        out["documentation"] = json!(doc.chars().take(JSON_DOC_CHARS).collect::<String>());
    }
    if !item.additional.is_empty() {
        out["additional_edits"] = Value::Array(
            item.additional
                .iter()
                .map(|edit| {
                    json!({
                        "range": { "start": at_json(edit.start), "end": at_json(edit.end) },
                        "text": edit.text,
                    })
                })
                .collect(),
        );
    }
    if item.deprecated {
        out["deprecated"] = json!(true);
    }
    if item.preselect {
        out["preselect"] = json!(true);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tako_core::lsp::completion::Edit;
    use tako_core::lsp::state::ServerState;

    fn all_errors() -> Vec<CompletionError> {
        vec![
            CompletionError::Superseded,
            CompletionError::Edited,
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
            assert_eq!(value["action"], json!("completion"));
            for key in ["reason", "next_step"] {
                let s = value[key].as_str().unwrap_or("");
                assert!(!s.is_empty(), "{error:?} の {key} が空");
                assert!(!s.contains('{'), "{error:?} の {key} に差し込み漏れ: {s}");
            }
        }
        // 能力に無いときは補完の文（定義ジャンプの「ジャンプ」の文を使わない）
        let unsupported = CompletionError::from(GotoError::Unsupported { server: "srv" });
        assert_eq!(
            unsupported.reason(),
            text::fill(text::COMPLETION_UNSUPPORTED_REASON, &[("server", "srv")])
        );
    }

    #[test]
    fn 候補の_wire_形は範囲を確定の位置で返す() {
        let item = CompletionItem {
            label: "name".into(),
            kind: Some(6),
            detail: Some("usize".into()),
            documentation: Some("あ".repeat(JSON_DOC_CHARS + 9)),
            filter_text: None,
            sort_text: None,
            preselect: false,
            deprecated: true,
            insert_text: "name".into(),
            start: At::new(4, 8),
            tail: 2,
            additional: vec![Edit {
                start: At::new(0, 0),
                end: At::new(0, 0),
                text: "use x;\n".into(),
            }],
            raw: Value::Null,
        };
        let value = item_json(&item, 3, At::new(4, 10));
        assert_eq!(value["number"], json!(3));
        assert_eq!(value["kind"], json!("variable"));
        // 行は 1 始まり・桁は 0 始まりの UTF-8 バイト（`tako edit replace-range` と同じ）
        assert_eq!(value["range"]["start"], json!({"line": 5, "column": 8}));
        assert_eq!(value["range"]["end"], json!({"line": 5, "column": 12}));
        assert_eq!(value["additional_edits"][0]["text"], json!("use x;\n"));
        assert_eq!(
            value["documentation"].as_str().unwrap().chars().count(),
            JSON_DOC_CHARS
        );
        assert_eq!(value["deprecated"], json!(true));
        assert!(value.get("preselect").is_none(), "偽の印は載せない");
    }

    #[test]
    fn 上限とデバウンスは_env_で変えられ不正は既定へ落ちる() {
        // env を触るテストはこの 1 本だけ（並行する他のテストが同じ名前を読まない）
        std::env::set_var("TAKO_LSP_COMPLETION_TIMEOUT_SECS", "0");
        assert_eq!(completion_timeout(), DEFAULT_COMPLETION_TIMEOUT);
        std::env::set_var("TAKO_LSP_COMPLETION_TIMEOUT_SECS", "4");
        assert_eq!(completion_timeout(), Duration::from_secs(4));
        std::env::remove_var("TAKO_LSP_COMPLETION_TIMEOUT_SECS");
        std::env::set_var("TAKO_LSP_COMPLETION_DEBOUNCE_MS", "x");
        assert_eq!(debounce(), DEFAULT_DEBOUNCE);
        std::env::set_var("TAKO_LSP_COMPLETION_DEBOUNCE_MS", "0");
        assert_eq!(debounce(), Duration::ZERO);
        std::env::remove_var("TAKO_LSP_COMPLETION_DEBOUNCE_MS");
        assert_eq!(debounce(), DEFAULT_DEBOUNCE);
    }
}
