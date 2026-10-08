//! ホバー（型・doc のカード。S4 / #1681）の純粋部分
//!
//! プロセスも I/O も持たない部分だけをここに置く:
//!
//! - 応答の読み取り（`Hover.contents` の 3 形 = `MarkupContent` / `MarkedString` /
//!   `MarkedString[]` と、`null`・空の本文）→ [`parse_response`]（**全文のまま**返す）
//! - 本文の上限（巨大な doc で UI と CLI / MCP の応答を膨らませない = [`MAX_CHARS`]。切ったら印を残す）。
//!   切るのは出口ごと（カード = [`MAX_CHARS`]・CLI / MCP = `limit` の指定 = [`char_limit`]）で、
//!   どちらも [`Hover::limited`] の 1 本を通る（#1893）
//! - `Hover.range` を tako の座標へ（[`locate_range`]）
//! - 能力（`hoverProvider`）の読み取り（[`server_supports`]）
//!
//! 問い合わせ（デバウンス明けの要求・`$/cancelRequest`）は `tako_control::lsp::manager`、
//! カードの描画は tako-app（`lsp_hover_ui.rs`。Markdown は `md_view::render_block` を通す）。
//! **言語名はここに書かない**（検出表だけが持つ = #1678 の番犬）。

use serde_json::Value;

use super::completion::{At, LspRange};
use super::position::LineIndex;

/// 本文の上限の既定（文字数）。超えたら切って [`Hover::truncated`] を立てる。
/// rust-analyzer は `Vec` の上で std の doc を丸ごと（数十 KB）返すので、カード（スクロールする）は
/// いつもこの上限で、CLI / MCP の応答は `limit` を省いたときこの上限で載せる（[`char_limit`]）
pub const MAX_CHARS: usize = 16_000;

/// CLI / MCP の `limit`（#1893）→ 本文の上限。省略 = [`MAX_CHARS`]、`0` = 全文（上限なし）、
/// それ以外 = その字数
pub fn char_limit(limit: Option<usize>) -> Option<usize> {
    match limit {
        None => Some(MAX_CHARS),
        Some(0) => None,
        Some(n) => Some(n),
    }
}

/// 本文の種類（LSP の `MarkupKind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Markup {
    /// Markdown（`render_block` を通して描く）
    Markdown,
    /// 平文（解釈もエスケープもせず、そのまま出す）
    PlainText,
}

impl Markup {
    /// 応答の `kind`（LSP の綴り）
    pub fn slug(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::PlainText => "plaintext",
        }
    }
}

/// 答えを読んだもの（LSP の座標のまま）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub markup: Markup,
    /// 本文（[`parse_response`] の答えは全文。[`Hover::limited`] の後は上限まで）
    pub value: String,
    /// 上限で切ったか
    pub truncated: bool,
    /// 切る前の文字数
    pub total_chars: usize,
    /// `Hover.range`（ホバーの対象の範囲。サーバが付けたときだけ）
    pub range: Option<LspRange>,
}

impl Hover {
    /// 本文を `limit` 文字まで（`None` = 全文）にしたもの（#1893）。切ったら [`Hover::truncated`] を
    /// 立て、[`Hover::total_chars`] は切る前の数のまま。切り方は [`truncate`]（行の切れ目まで戻す）
    pub fn limited(&self, limit: Option<usize>) -> Hover {
        let Some(limit) = limit else {
            return self.clone();
        };
        let (value, truncated, _) = truncate(&self.value, limit);
        Hover {
            markup: self.markup,
            value,
            truncated: self.truncated || truncated,
            total_chars: self.total_chars,
            range: self.range,
        }
    }
}

/// `textDocument/hover` の答えを読む。**表示するものが無ければ `None`**
/// （`null`・`contents` が無い・本文が空白だけ。サーバが「この位置には何も無い」と答えた形）。
///
/// `contents` の 3 形:
/// - `MarkupContent`（`{ kind, value }`）: `kind` が `plaintext` なら平文、それ以外は Markdown
///   （仕様の値は 2 つだけ。知らない値を Markdown として解釈しないよう平文へ倒すのは `plaintext`
///   と綴りが違うときだけにする = rust-analyzer / clangd / pyright は `markdown`）
/// - `MarkedString`（文字列 = Markdown / `{ language, value }` = その言語のコードブロック）
/// - `MarkedString[]`（古い形）: 1 つずつ Markdown にして空行でつなぐ
pub fn parse_response(value: &Value) -> Option<Hover> {
    let contents = value.get("contents")?;
    let (markup, text) = match contents {
        Value::Array(parts) => {
            let joined = parts
                .iter()
                .filter_map(marked_string)
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (Markup::Markdown, joined)
        }
        Value::Object(object) if object.contains_key("kind") => {
            let markup = match object.get("kind").and_then(Value::as_str) {
                Some("plaintext") => Markup::PlainText,
                _ => Markup::Markdown,
            };
            let text = object
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (markup, text)
        }
        other => (Markup::Markdown, marked_string(other)?),
    };
    if text.trim().is_empty() {
        return None;
    }
    let range = value.get("range").and_then(read_range);
    let total_chars = text.chars().count();
    Some(Hover {
        markup,
        value: text,
        truncated: false,
        total_chars,
        range,
    })
}

/// `Hover.range`（壊れていれば `None` = 範囲が無いだけで本文は出す）
fn read_range(v: &Value) -> Option<LspRange> {
    let point = |p: &Value| -> Option<(usize, usize)> {
        Some((
            p.get("line")?.as_u64()? as usize,
            p.get("character")?.as_u64()? as usize,
        ))
    };
    Some((point(v.get("start")?)?, point(v.get("end")?)?))
}

/// `MarkedString` 1 つを Markdown へ（文字列はそのまま、`{ language, value }` はコードブロック）。
/// 空のコードは囲まない（空のフェンスだけが残ると「何も無い答え」を本文ありと読み違える）
fn marked_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => {
            let code = object.get("value")?.as_str()?;
            if code.trim().is_empty() {
                return None;
            }
            let language = object
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(fenced(language, code))
        }
        _ => None,
    }
}

/// コードを囲むフェンス。本文の中の最も長い ``` の並びより 1 つ長くする
/// （本文に ``` を含むコードで囲みが途中で閉じない）
fn fenced(language: &str, code: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in code.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!(
        "{fence}{language}\n{}\n{fence}",
        code.trim_end_matches('\n')
    )
}

/// 本文を `limit` 文字までにする（切ったら行の切れ目まで戻す。戻りすぎるなら字で切る）。
/// 返り値は（本文, 切ったか, 切る前の文字数）
pub fn truncate(text: &str, limit: usize) -> (String, bool, usize) {
    let total = text.chars().count();
    if total <= limit {
        return (text.to_string(), false, total);
    }
    let cut = text
        .char_indices()
        .nth(limit)
        .map_or(text.len(), |(i, _)| i);
    let head = &text[..cut];
    // 行の途中で切らない（後半に改行があればそこまで戻す）
    let head = match head.rfind('\n') {
        Some(i) if i >= cut / 2 => &head[..i],
        _ => head,
    };
    (head.to_string(), true, total)
}

/// `Hover.range` を tako の座標（0 起点の行・行内の UTF-8 バイト）へ。`text` はサーバが見ている本文
pub fn locate_range(range: LspRange, text: &str) -> (At, At) {
    let index = LineIndex::new(text);
    let to_at = |(line, character): (usize, usize)| {
        let (line, col) = index.line_byte_col(line, character);
        At::new(line, col)
    };
    (to_at(range.0), to_at(range.1))
}

/// ホバーの能力の `ServerCapabilities` のキー（右クリックメニューの出し分け = #1893 も引く）
pub const PROVIDER_KEY: &str = "hoverProvider";

/// サーバがホバーに対応しているか（`hoverProvider` が `true` か options のオブジェクト）
pub fn server_supports(capabilities: &Value) -> bool {
    match capabilities.get(PROVIDER_KEY) {
        Some(Value::Bool(supported)) => *supported,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markup_content_は種類と本文をそのまま読む() {
        let hover = parse_response(&json!({
            "contents": { "kind": "markdown", "value": "# 見出し\n\n本文" },
        }))
        .unwrap();
        assert_eq!(hover.markup, Markup::Markdown);
        assert_eq!(hover.value, "# 見出し\n\n本文");
        assert!(!hover.truncated);
        assert_eq!(hover.range, None);
        let plain = parse_response(&json!({
            "contents": { "kind": "plaintext", "value": "a < b && *c* `d`" },
        }))
        .unwrap();
        assert_eq!(plain.markup, Markup::PlainText);
        // 平文はエスケープもしない（`<` も `*` もそのまま）
        assert_eq!(plain.value, "a < b && *c* `d`");
    }

    #[test]
    fn marked_string_は文字列もコードも配列も_markdown_へ() {
        let text = parse_response(&json!({ "contents": "**太字**" })).unwrap();
        assert_eq!(
            (text.markup, text.value.as_str()),
            (Markup::Markdown, "**太字**")
        );
        let code =
            parse_response(&json!({ "contents": { "language": "rust", "value": "fn f()\n" } }))
                .unwrap();
        assert_eq!(code.value, "```rust\nfn f()\n```");
        let array = parse_response(&json!({
            "contents": [{ "language": "c", "value": "int x" }, "", "doc"],
        }))
        .unwrap();
        assert_eq!(array.value, "```c\nint x\n```\n\ndoc");
        // 本文に ``` を含むコードは囲みを長くする（途中で閉じない）
        let nested =
            parse_response(&json!({ "contents": { "language": "md", "value": "```x```" } }))
                .unwrap();
        assert_eq!(nested.value, "````md\n```x```\n````");
    }

    #[test]
    fn 何も無い答えは_none() {
        for value in [
            json!(null),
            json!({}),
            json!({ "contents": "" }),
            json!({ "contents": "  \n " }),
            json!({ "contents": [] }),
            json!({ "contents": ["", { "language": "rust", "value": "" }] }),
            json!({ "contents": { "kind": "markdown", "value": "" } }),
            json!({ "contents": 3 }),
        ] {
            assert_eq!(parse_response(&value), None, "{value}");
        }
    }

    #[test]
    fn 範囲は_lsp_の座標で読み_tako_の座標へ写す() {
        let hover = parse_response(&json!({
            "contents": "x",
            "range": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 7 } },
        }))
        .unwrap();
        assert_eq!(hover.range, Some(((1, 3), (1, 7))));
        // 絵文字（UTF-16 で 2・UTF-8 で 4）の後ろ: UTF-16 の 3 = バイトの 5
        let text = "fn a() {}\n😀 名前x\n";
        let (start, end) = locate_range(((1, 3), (1, 5)), text);
        assert_eq!(start, At::new(1, 5));
        assert_eq!(end, At::new(1, 11));
        // 壊れた範囲は読まない（範囲が無いだけで本文は出す）
        let broken =
            parse_response(&json!({ "contents": "x", "range": { "start": { "line": 1 } } }))
                .unwrap();
        assert_eq!(broken.range, None);
    }

    #[test]
    fn 巨大な本文は上限で行の切れ目まで切る() {
        let line = "あ".repeat(99);
        let big: String = (0..400).map(|i| format!("{line}{i}\n")).collect();
        let full =
            parse_response(&json!({ "contents": { "kind": "markdown", "value": big } })).unwrap();
        // 読み取りは全文のまま（切るのは出口 = #1893）
        assert!(!full.truncated);
        assert_eq!(full.value, big);
        let hover = full.limited(char_limit(None));
        assert!(hover.truncated);
        assert_eq!(hover.total_chars, big.chars().count());
        assert!(hover.value.chars().count() <= MAX_CHARS);
        assert!(!hover.value.ends_with('\n'));
        assert!(big.starts_with(&hover.value), "先頭から切る");
        // 全文（limit 0）は切らない・指定の上限はその字数まで
        assert_eq!(full.limited(char_limit(Some(0))), full);
        let small = full.limited(char_limit(Some(250)));
        assert!(small.truncated && small.value.chars().count() <= 250);
        assert_eq!(small.total_chars, big.chars().count());
        // 上限より短い本文は何もしない（印も立てない）
        assert_eq!(full.limited(Some(big.chars().count())), full);
        // 改行の無い長い 1 行は字で切る
        let (one, cut, total) = truncate(&"x".repeat(50), 10);
        assert_eq!((one.as_str(), cut, total), ("xxxxxxxxxx", true, 50));
        assert_eq!(truncate("短い", 10), ("短い".to_string(), false, 2));
    }

    /// #1893: `limit` の読み替え（省略 = 既定の上限・0 = 全文・それ以外 = その字数）
    #[test]
    fn limit_の読み替え() {
        assert_eq!(char_limit(None), Some(MAX_CHARS));
        assert_eq!(char_limit(Some(0)), None);
        assert_eq!(char_limit(Some(1)), Some(1));
        assert_eq!(char_limit(Some(40_000)), Some(40_000));
    }

    #[test]
    fn 能力の読み取り() {
        assert!(server_supports(&json!({ "hoverProvider": true })));
        assert!(server_supports(
            &json!({ "hoverProvider": { "workDoneProgress": false } })
        ));
        assert!(!server_supports(&json!({ "hoverProvider": false })));
        assert!(!server_supports(&json!({})));
    }

    #[test]
    fn 種類の綴りは_lsp_と同じ() {
        assert_eq!(Markup::Markdown.slug(), "markdown");
        assert_eq!(Markup::PlainText.slug(), "plaintext");
    }
}
