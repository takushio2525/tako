//! 整形（S6 / #1683）の純粋部分: 要求の組み立て・能力の判定・答えの読み取り・範囲の規則
//!
//! 問い合わせそのもの（文書の同期・サーバの起動待ち・要求）は `tako_control::lsp` の manager、
//! 答えを編集バッファへ当てるのは `text_edit::TextBuffer::apply_changes` の 1 実装。
//! **並べ方・重なり・最小化は `text_edit::order_changes` だけが持つ**（ここでは書かない）。
//!
//! 位置の変換（UTF-16 ⇄ UTF-8 バイト）は [`super::position`] の入口だけを通す。答えの座標は
//! **要求したときにサーバが見ていた本文**（送った写し）で直す。

use std::ops::Range;

use serde_json::{json, Value};

use super::position;
use crate::text_edit::{IndentUnit, TextChange};

/// 文書全体の整形の method
pub const FORMATTING_METHOD: &str = "textDocument/formatting";
/// 範囲の整形の method
pub const RANGE_FORMATTING_METHOD: &str = "textDocument/rangeFormatting";

/// タブで字下げするファイルで、タブ 1 つを何桁とみなすか（`FormattingOptions.tabSize`）。
/// tako の表示のタブ幅（`text_edit` のタブストップ）と同じ 4
pub const TAB_SIZE_FOR_TABS: usize = 4;

/// `FormattingOptions` の必須 2 項目。**バッファのインデントの単位から**決める
/// （ファイルの流儀に合わせる。プロジェクトの設定ファイル（rustfmt.toml 等）があれば
/// サーバはそちらを優先する）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub tab_size: usize,
    pub insert_spaces: bool,
}

impl FormatOptions {
    pub fn from_indent(unit: IndentUnit) -> Self {
        match unit {
            IndentUnit::Tab => Self {
                tab_size: TAB_SIZE_FOR_TABS,
                insert_spaces: false,
            },
            IndentUnit::Spaces(width) => Self {
                tab_size: width,
                insert_spaces: true,
            },
        }
    }

    pub fn to_json(self) -> Value {
        json!({ "tabSize": self.tab_size, "insertSpaces": self.insert_spaces })
    }
}

/// 範囲の有無で method を選ぶ
pub fn method(ranged: bool) -> &'static str {
    if ranged {
        RANGE_FORMATTING_METHOD
    } else {
        FORMATTING_METHOD
    }
}

/// 能力のキー（`documentFormattingProvider` / `documentRangeFormattingProvider`）
pub fn provider_key(ranged: bool) -> &'static str {
    if ranged {
        "documentRangeFormattingProvider"
    } else {
        "documentFormattingProvider"
    }
}

/// サーバがその整形に対応しているか。LSP 3.17 の型は `boolean | Options` なので、
/// `true` かオブジェクトなら対応、`false` / 無しなら非対応
pub fn server_supports(capabilities: &Value, ranged: bool) -> bool {
    match capabilities.get(provider_key(ranged)) {
        Some(Value::Bool(supported)) => *supported,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

/// 要求の params。`range` は**サーバが見ている本文** `text` のバイト位置（UTF-16 へはここで直す）
pub fn params(uri: &str, text: &str, options: FormatOptions, range: Option<Range<usize>>) -> Value {
    let mut out = json!({
        "textDocument": { "uri": uri },
        "options": options.to_json(),
    });
    if let Some(range) = range {
        let (sl, sc) = position::lsp_position_of(text, range.start);
        let (el, ec) = position::lsp_position_of(text, range.end);
        out["range"] = json!({
            "start": { "line": sl, "character": sc },
            "end": { "line": el, "character": ec },
        });
    }
    out
}

/// 答え（`TextEdit[] | null`）を読めなかった理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 配列でも null でもない
    NotArray,
    /// `index` 番目（1 始まり）に `range` / `newText` が無い・形が違う
    Malformed { index: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotArray => write!(f, "TextEdit の配列ではない"),
            Self::Malformed { index } => write!(f, "{index} 番目の TextEdit の形が違う"),
        }
    }
}

/// 答えを tako の座標（書き換える前の本文 `text` のバイト位置）へ直す。`null` は 0 件。
///
/// 位置は LSP の仕様どおり丸める（行末を超える桁は行末へ・最終行を超える行は本文の末尾へ。
/// [`position::LineIndex`] の 1 実装）。並べ方・重なりの検査はここではしない
/// （`text_edit::order_changes`）
pub fn parse_text_edits(result: &Value, text: &str) -> Result<Vec<TextChange>, ParseError> {
    let edits = match result {
        Value::Null => return Ok(Vec::new()),
        Value::Array(edits) => edits,
        _ => return Err(ParseError::NotArray),
    };
    let index = position::LineIndex::new(text);
    let at = |p: &Value| -> Option<usize> {
        let line = usize::try_from(p.get("line")?.as_u64()?).ok()?;
        let character = usize::try_from(p.get("character")?.as_u64()?).ok()?;
        Some(index.byte_offset(line, character))
    };
    edits
        .iter()
        .enumerate()
        .map(|(i, edit)| {
            let malformed = ParseError::Malformed { index: i + 1 };
            let range = edit.get("range").ok_or(malformed.clone())?;
            let start = range.get("start").and_then(at).ok_or(malformed.clone())?;
            let end = range.get("end").and_then(at).ok_or(malformed.clone())?;
            let new_text = edit
                .get("newText")
                .and_then(Value::as_str)
                .ok_or(malformed)?;
            Ok(TextChange {
                range: start..end,
                text: new_text.to_string(),
            })
        })
        .collect()
}

/// 範囲の整形で、範囲の外へ空白以外の書き換えが来た（当てると範囲の外が変わる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideRange {
    /// 外にかかった書き換えの数
    pub count: usize,
}

/// 範囲の整形の答えを範囲の中へ収める。**範囲の外は 1 バイトも変えない**。
///
/// `ordered` は `text_edit::order_changes` を通した後（並べて最小にした組）。範囲は書き換える
/// 前の本文のバイト位置で、範囲の端ちょうどへの挿入は中とみなす（外の本文は動かない）。
///
/// 決め: 外にかかる書き換えが**空白だけ**（置き換える前も後も空白か空）なら捨てて残りを当てる
/// （捨てた数を返す。字下げ・行末の空白はサーバが範囲の端を行単位へ広げて返しがち）。
/// **空白以外を含むなら全体を当てない**（行の入れ替えのような書き換えは、片方だけ当てると
/// 行が消える / 増える = 意味が変わる）
pub fn restrict_to_range(
    text: &str,
    ordered: Vec<TextChange>,
    range: &Range<usize>,
) -> Result<(Vec<TextChange>, usize), OutsideRange> {
    let blank = |s: &str| s.chars().all(char::is_whitespace);
    let (inside, outside): (Vec<TextChange>, Vec<TextChange>) = ordered
        .into_iter()
        .partition(|change| change.range.start >= range.start && change.range.end <= range.end);
    if outside
        .iter()
        .any(|change| !blank(&text[change.range.clone()]) || !blank(&change.text))
    {
        return Err(OutsideRange {
            count: outside.len(),
        });
    }
    Ok((inside, outside.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_edit::{order_changes, TextBuffer};
    use std::path::PathBuf;

    fn edit(sl: u64, sc: u64, el: u64, ec: u64, new_text: &str) -> Value {
        json!({
            "range": {
                "start": { "line": sl, "character": sc },
                "end": { "line": el, "character": ec },
            },
            "newText": new_text,
        })
    }

    #[test]
    fn 字下げの単位から整形の設定を決める() {
        assert_eq!(
            FormatOptions::from_indent(IndentUnit::Spaces(2)).to_json(),
            json!({ "tabSize": 2, "insertSpaces": true })
        );
        assert_eq!(
            FormatOptions::from_indent(IndentUnit::Tab).to_json(),
            json!({ "tabSize": 4, "insertSpaces": false })
        );
    }

    #[test]
    fn 能力は真偽でもオブジェクトでも読む() {
        let caps = json!({
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": { "rangesSupport": false },
        });
        assert!(server_supports(&caps, false));
        assert!(server_supports(&caps, true));
        let caps = json!({ "documentFormattingProvider": false });
        assert!(!server_supports(&caps, false));
        assert!(!server_supports(&caps, true));
        assert!(!server_supports(&json!({}), false));
        assert_eq!(method(false), FORMATTING_METHOD);
        assert_eq!(method(true), RANGE_FORMATTING_METHOD);
    }

    /// 範囲は UTF-16 の桁で送る（日本語は 1 文字 = 1・絵文字は 2）
    #[test]
    fn 範囲はサーバの座標で送る() {
        let text = "let 名前 = 1;\n😀x\n";
        // 2 行目の `x`（バイト 20 = 1 行目 16 バイト + 改行 + 絵文字 4 バイト）まで
        let start = text.find('名').unwrap();
        let end = text.find('x').unwrap();
        let options = FormatOptions::from_indent(IndentUnit::Spaces(4));
        let value = params("file:///a.rs", text, options, Some(start..end));
        assert_eq!(
            value["range"]["start"],
            json!({ "line": 0, "character": 4 })
        );
        assert_eq!(value["range"]["end"], json!({ "line": 1, "character": 2 }));
        assert_eq!(value["options"]["tabSize"], json!(4));
        assert!(params("file:///a.rs", text, options, None)
            .get("range")
            .is_none());
    }

    /// 答えの位置は UTF-16 からバイトへ。null は 0 件・行末を超える桁は行末へ丸める
    #[test]
    fn 答えをバイトの範囲へ直す() {
        let text = "fn 名(){\r\n😀\n}";
        let answer = json!([
            edit(0, 4, 0, 4, " "),
            edit(1, 2, 1, 99, ";"),
            edit(9, 0, 9, 0, "\n"),
        ]);
        let changes = parse_text_edits(&answer, text).unwrap();
        let name_end = text.find('(').unwrap();
        let emoji_end = text.find('😀').unwrap() + '😀'.len_utf8();
        assert_eq!(
            changes,
            vec![
                TextChange {
                    range: name_end..name_end,
                    text: " ".into()
                },
                TextChange {
                    range: emoji_end..emoji_end,
                    text: ";".into()
                },
                TextChange {
                    range: text.len()..text.len(),
                    text: "\n".into()
                },
            ]
        );
        assert_eq!(parse_text_edits(&Value::Null, text), Ok(Vec::new()));
        assert_eq!(
            parse_text_edits(&json!({}), text),
            Err(ParseError::NotArray)
        );
        assert_eq!(
            parse_text_edits(&json!([edit(0, 0, 0, 0, ""), { "newText": "x" }]), text),
            Err(ParseError::Malformed { index: 2 })
        );
    }

    /// Issue の受け入れ条件: 範囲の整形は範囲の外を 1 バイトも変えない
    #[test]
    fn 範囲の整形は範囲の外を1バイトも変えない() {
        let text = "fn a(){\nlet x=1;\nlet y=2;\nlet z=3;\n}\n";
        // 2〜3 行目だけを整形する（範囲 = 2 行目の頭から 3 行目の末尾まで）
        let start = text.find("let x").unwrap();
        let end = text.find("let z").unwrap() - 1;
        // サーバは範囲の外（1 行目の `{` の前・4 行目の行頭）も返してきた
        let answer = json!([
            edit(0, 6, 0, 6, " "),
            edit(1, 0, 1, 0, "    "),
            edit(1, 5, 1, 6, " = "),
            edit(2, 0, 2, 0, "    "),
            edit(2, 5, 2, 6, " = "),
            edit(3, 0, 3, 0, "    "),
        ]);
        let changes = parse_text_edits(&answer, text).unwrap();
        let ordered = order_changes(text, changes).unwrap();
        let (kept, dropped) = restrict_to_range(text, ordered, &(start..end)).unwrap();
        assert_eq!(dropped, 2, "外の 2 つ（空白だけ）は捨てる");
        let mut buffer = TextBuffer::from_text(PathBuf::from("a.rs"), text.into());
        buffer.apply_changes(kept, None).unwrap();
        let after = buffer.text();
        assert_eq!(&after.as_bytes()[..start], &text.as_bytes()[..start]);
        let tail = &text.as_bytes()[end..];
        assert_eq!(&after.as_bytes()[after.len() - tail.len()..], tail);
        assert_eq!(
            after,
            "fn a(){\n    let x = 1;\n    let y = 2;\nlet z=3;\n}\n"
        );
    }

    /// 外へ空白以外の書き換えが来たら全体を当てない（行の入れ替えを片方だけ当てると行が消える）
    #[test]
    fn 範囲の外へ空白以外が来たら全体を当てない() {
        let text = "use b;\nuse a;\nfn f() {}\n";
        // 範囲は 1 行目だけ。サーバは `use` を並べ替えて 2 行目も書き換えてきた
        let answer = json!([edit(0, 4, 0, 5, "a"), edit(1, 4, 1, 5, "b")]);
        let ordered = order_changes(text, parse_text_edits(&answer, text).unwrap()).unwrap();
        assert_eq!(
            restrict_to_range(text, ordered, &(0..6)),
            Err(OutsideRange { count: 1 })
        );
    }

    /// 範囲の端ちょうどへの挿入は中（外の本文は動かない）
    #[test]
    fn 範囲の端への挿入は中とみなす() {
        let text = "ab\ncd\n";
        let ordered = order_changes(
            text,
            vec![
                TextChange {
                    range: 3..3,
                    text: "  ".into(),
                },
                TextChange {
                    range: 5..5,
                    text: ";".into(),
                },
            ],
        )
        .unwrap();
        let (kept, dropped) = restrict_to_range(text, ordered, &(3..5)).unwrap();
        assert_eq!((kept.len(), dropped), (2, 0));
    }
}
