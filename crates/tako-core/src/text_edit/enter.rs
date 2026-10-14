//! Enter の続き（Issue #1759 / エピック #1647 の B レーン）。純粋関数だけで I/O をしない
//!
//! #1654 の Enter（インデントの継承と開き括弧の直後の 1 段）に足す 2 つの規則を持つ。
//! どちらも**言語名を持たない**: 使うかどうかと語は言語の同定の正本 `file_type` の行の列
//! （`dedent_after` / `list_continuation`）が決め、ここは列の値を受けて判定するだけ。
//!
//! - **字下げを戻すキーワード**（[`dedents_after`]）: ブロックを抜ける文（Python の `return` /
//!   `pass` / `break` / `continue` / `raise`）の行で Enter を押すと次の行を 1 段浅くする。
//!   文が続いている行（`\` で終わる・閉じていない括弧がある）では戻さない
//! - **リストの印の引き継ぎ**（[`list_item`]）: `- ` / `* ` / `+ ` / `1. ` / `1) ` /
//!   `- [ ] ` の行で Enter を押すと次の行へ印を引き継ぐ（番号は 1 つ進め、チェックボックスは
//!   空にする）。中身が空の項目で押したら印を消して抜ける。コードブロック（```` ``` ```` /
//!   `~~~`）の中では引き継がない（[`in_fenced_code`]）
//!
//! 本体（どの範囲を何に置き換えるか）は [`super::TextBuffer::newline_and_indent`] の 1 実装

use std::sync::OnceLock;

use super::SCAN_WINDOW;

/// `TAKO_1759_LEGACY=1` で **#1759 前の挙動**へ戻す（同一バイナリで A/B を取る入口）:
/// 字下げを戻さない・リストの印を引き継がない・括弧を自動で閉じない（#1654 の Enter と 1 文字の挿入）
pub fn legacy_1759() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1759_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 識別子の一部になる文字（キーワードの照合を語の単位にする。`returned` / `pass_count` は語が違う）
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 行頭からカーソルまで（`line`）がブロックを抜ける文で始まっているか（#1759）。
///
/// `keywords` は形式の行の `dedent_after` 列。照合は字下げを除いた先頭の語で、語の後ろが
/// 識別子の文字なら別の語。`\` で終わる行（文が次の行へ続く）は戻さない。閉じていない
/// 括弧の判定は文字列・コメントを見分ける走査が要るので呼び手が持つ
pub(super) fn dedents_after(line: &str, keywords: &[&str]) -> bool {
    let statement = line.trim_start_matches([' ', '\t']);
    let starts = keywords.iter().any(|word| {
        statement
            .strip_prefix(word)
            .is_some_and(|rest| !rest.chars().next().is_some_and(is_ident))
    });
    starts && !statement.trim_end_matches([' ', '\t']).ends_with('\\')
}

/// リストの項目の行（#1759）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListItem<'a> {
    /// 印の手前の字下げ
    pub(super) indent: &'a str,
    /// 次の行に置く印（字下げを除く。番号は 1 つ進め、チェックボックスは空にし、
    /// 印の後ろの空白は元の行のまま）
    pub(super) next_marker: String,
    /// 中身の始まり（行頭からのバイト。印・空白・チェックボックスの後ろ）
    pub(super) content_start: usize,
}

/// 行頭からカーソルまで（`line`）がリストの印で始まっていれば、その項目（#1759）。
///
/// 印は CommonMark のリストの印: `-` / `*` / `+`、または 1〜9 桁の数字 + `.` / `)`。
/// 印の後ろに空白（スペースかタブ）が 1 つ以上要る（`-foo` はリストではない）。
/// その後ろの `[ ]` / `[x]` / `[X]` + 空白（行末でもよい）はチェックボックス。
/// 印の途中にカーソルがある（`-|`）・印より前にある行は項目として扱わない（素の Enter）
pub(super) fn list_item(line: &str) -> Option<ListItem<'_>> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let marker_len = match rest.as_bytes().first()? {
        b'-' | b'*' | b'+' => 1,
        b'0'..=b'9' => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 9 || !matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    let after_marker = &rest[marker_len..];
    let spacing = after_marker.len() - after_marker.trim_start_matches([' ', '\t']).len();
    if spacing == 0 {
        return None;
    }
    let marker = &rest[..marker_len];
    let next = match marker.as_bytes()[0] {
        b'0'..=b'9' => {
            let digits = &marker[..marker_len - 1];
            // 9 桁までなので u64 で溢れない（999999999 + 1 = 10 桁になってもよい）
            let n: u64 = digits.parse().ok()?;
            format!("{}{}", n + 1, &marker[marker_len - 1..])
        }
        _ => marker.to_string(),
    };
    let space = &after_marker[..spacing];
    let mut content_start = indent_len + marker_len + spacing;
    let mut next_marker = format!("{next}{space}");
    // チェックボックス（`[ ]` / `[x]`）は空の箱を引き継ぐ
    let body = &after_marker[spacing..];
    if let Some(after_box) = ["[ ]", "[x]", "[X]"]
        .iter()
        .find_map(|b| body.strip_prefix(b))
    {
        let box_spacing = after_box.len() - after_box.trim_start_matches([' ', '\t']).len();
        if box_spacing > 0 || after_box.is_empty() {
            let gap = if box_spacing > 0 {
                &after_box[..box_spacing]
            } else {
                " "
            };
            next_marker.push_str("[ ]");
            next_marker.push_str(gap);
            content_start += 3 + box_spacing;
        }
    }
    Some(ListItem {
        indent,
        next_marker,
        content_start,
    })
}

/// 区切り線（`---` / `* * *` / `___`）か。リストの印と同じ記号で始まるので先に見分ける（CommonMark）
pub(super) fn is_thematic_break(line: &str) -> bool {
    let line = line.trim_matches([' ', '\t', '\r']);
    let Some(first) = line.chars().next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_')
        && line.chars().filter(|c| *c == first).count() >= 3
        && line.chars().all(|c| c == first || c == ' ' || c == '\t')
}

/// コードブロックの囲みを探し始める位置（#1759）。`line_start` から [`SCAN_WINDOW`] より前は読まない。
///
/// 対応括弧の探索（#1691）と同じ近似で、10 MB の文書でも 1 回の Enter が読むのは 1 MB まで。
/// 窓の頭は文字の途中を避け、窓の中の最初の行頭へ寄せる
pub(super) fn fence_scan_start(text: &str, line_start: usize) -> usize {
    if line_start <= SCAN_WINDOW {
        return 0;
    }
    let mut from = line_start - SCAN_WINDOW;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    text[from..line_start]
        .find('\n')
        .map_or(line_start, |p| from + p + 1)
}

/// `line_start`（行頭）の行がコードブロック（```` ``` ```` / `~~~` の囲み）の中か（#1759）。
///
/// `from`（行頭）から `line_start` の手前までの行を読み、開いたまま閉じていない囲みがあれば中。
/// 囲みは CommonMark と同じく字下げ 3 桁まで・同じ記号 3 つ以上で開き、同じ記号を開きと同じ数
/// 以上並べた行（後ろは空白だけ）で閉じる。```` ``` ```` の囲みの情報文字列に `` ` `` は来ない
pub(super) fn in_fenced_code(text: &str, from: usize, line_start: usize) -> bool {
    let mut open: Option<(u8, usize)> = None;
    for line in text[from..line_start].split('\n') {
        let line = line.trim_end_matches('\r');
        let lead = line.len() - line.trim_start_matches(' ').len();
        if lead > 3 {
            continue;
        }
        let rest = &line[lead..];
        let Some(&mark) = rest.as_bytes().first().filter(|b| matches!(b, b'`' | b'~')) else {
            continue;
        };
        let run = rest.bytes().take_while(|b| *b == mark).count();
        if run < 3 {
            continue;
        }
        let tail = &rest[run..];
        match open {
            None if mark == b'`' && tail.contains('`') => {}
            None => open = Some((mark, run)),
            Some((m, n)) if m == mark && run >= n && tail.trim().is_empty() => open = None,
            Some(_) => {}
        }
    }
    open.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXITS: &[&str] = &["return", "pass", "break", "continue", "raise"];

    #[test]
    fn 抜ける文は語の単位で照合する() {
        for line in [
            "    return",
            "    return x",
            "        return(x)",
            "\tpass",
            "    break",
            "    continue  # 次へ",
            "    raise ValueError(\"x\")",
            "return",
        ] {
            assert!(dedents_after(line, EXITS), "{line:?}");
        }
        for line in [
            "    returned = 1",
            "    pass_count += 1",
            "    x = return_value",
            "    # return",
            "    if x: return",
            "    return a + \\",
            "",
            "    ",
        ] {
            assert!(!dedents_after(line, EXITS), "{line:?}");
        }
        // 語の無い行（列が空 = Python 以外）は戻さない
        assert!(!dedents_after("    return", &[]));
    }

    fn item(line: &str) -> Option<(String, String, usize)> {
        list_item(line).map(|i| (i.indent.to_string(), i.next_marker, i.content_start))
    }

    #[test]
    fn リストの印を読む() {
        assert_eq!(item("- a"), Some(("".into(), "- ".into(), 2)));
        assert_eq!(item("* a"), Some(("".into(), "* ".into(), 2)));
        assert_eq!(item("+ a"), Some(("".into(), "+ ".into(), 2)));
        assert_eq!(item("  - b"), Some(("  ".into(), "- ".into(), 4)));
        // 番号は 1 つ進める。繰り上がりで桁が増える
        assert_eq!(item("1. a"), Some(("".into(), "2. ".into(), 3)));
        assert_eq!(item("9. a"), Some(("".into(), "10. ".into(), 3)));
        assert_eq!(item("99) a"), Some(("".into(), "100) ".into(), 4)));
        assert_eq!(
            item("999999999. a"),
            Some(("".into(), "1000000000. ".into(), 11))
        );
        // 印の後ろの空白は元のまま
        assert_eq!(item("-   a"), Some(("".into(), "-   ".into(), 4)));
        // チェックボックスは空の箱を引き継ぐ
        assert_eq!(item("- [ ] a"), Some(("".into(), "- [ ] ".into(), 6)));
        assert_eq!(item("- [x] done"), Some(("".into(), "- [ ] ".into(), 6)));
        assert_eq!(item("1. [X] a"), Some(("".into(), "2. [ ] ".into(), 7)));
        assert_eq!(item("- [ ]"), Some(("".into(), "- [ ] ".into(), 5)));
        // 空の項目
        assert_eq!(item("- "), Some(("".into(), "- ".into(), 2)));
        // 項目ではない
        for line in [
            "-",
            "-a",
            "1.a",
            "1",
            "10 a",
            "a. b",
            "1234567890. a",
            "",
            "  ",
            "# h",
        ] {
            assert_eq!(item(line), None, "{line:?}");
        }
        // チェックボックスの形をしていない `[` は中身
        assert_eq!(item("- [link](x)"), Some(("".into(), "- ".into(), 2)));
    }

    #[test]
    fn 区切り線を見分ける() {
        for line in ["---", "- - -", "* * *", "___", "  ***  ", "-----\r"] {
            assert!(is_thematic_break(line), "{line:?}");
        }
        for line in ["- a", "--", "- -", "* a *", "", "-*-"] {
            assert!(!is_thematic_break(line), "{line:?}");
        }
    }

    #[test]
    fn コードブロックの中を見分ける() {
        let text = "intro\n```yaml\n- a\n```\n- b\n~~~~\n- c\n~~~\n- d\n";
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(!in_fenced_code(text, 0, at("intro")));
        assert!(in_fenced_code(text, 0, at("- a")));
        assert!(!in_fenced_code(text, 0, at("- b")));
        assert!(in_fenced_code(text, 0, at("- c")));
        // 開きより短い囲み（~~~ < ~~~~）では閉じない
        assert!(in_fenced_code(text, 0, at("- d")));
        // 字下げ 4 桁は囲みではない・情報文字列に ` があれば囲みではない
        let text = "    ```\n- a\n``` a`b\n- b\n";
        assert!(!in_fenced_code(text, 0, text.find("- a").unwrap()));
        assert!(!in_fenced_code(text, 0, text.find("- b").unwrap()));
        // CRLF でも同じ
        let text = "```\r\n- a\r\n```\r\n- b\r\n";
        assert!(in_fenced_code(text, 0, text.find("- a").unwrap()));
        assert!(!in_fenced_code(text, 0, text.find("- b").unwrap()));
    }
}
