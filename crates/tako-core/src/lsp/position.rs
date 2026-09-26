//! UTF-8 バイト位置 ⇄ LSP の位置（0 起点の行・UTF-16 コードユニットの桁）の変換（#1678）
//!
//! `TextBuffer` は全てを UTF-8 のバイト位置で持つ（`text_edit` の不変条件）。一方 LSP 3.17 の
//! 位置の既定は **UTF-16 のコードユニット**で、tako は `positionEncodings` に UTF-16 だけを
//! 申告する（Zed と同じ。`.agent/plans/2026-09-lsp-s1.md` §16-3）。
//!
//! **入口はこの 2 本だけ**にする。サーバが仮に `utf-8` を選んできても別の経路を作らない
//! （経路が 2 本あるとサーバごとに不具合が割れる）。
//!
//! ## 境界の扱い（丸めの方針）
//!
//! - UTF-8 の文字の途中を指すバイト位置は**手前の境界へ丸める**（`text_edit` の
//!   `snap_boundary` と同じ方針）
//! - サロゲートペアの片割れを指す UTF-16 桁も**手前の境界へ丸める**
//!   （絵文字 U+1F600 は UTF-16 で 2 コードユニット・UTF-8 で 4 バイト）
//! - 行の区切りは `\n` だけ。CRLF の `\r` は**桁に数えない**（#1650 で CRLF を保持するように
//!   なったので、`\r` を数えるとサーバの桁と 1 つずれる）。`\r` と `\n` のあいだを指す
//!   バイト位置は `\r` の手前と同じ位置になる
//! - 行末を超える桁はその行の末尾（改行の手前）へ、最終行を超える行は本文の末尾へ丸める
//!
//! 外（CLI / MCP）から来る行・桁は `text_edit::RangeEditError` で**丸めずに拒否**するが、
//! こちらは LSP サーバとのあいだの変換で、サーバの版と数文字ずれた位置が来るのは
//! 正常系（非同期にすれ違う）なので丸める。

/// UTF-8 バイト位置 → LSP の `(0 起点の行, UTF-16 桁)`
pub fn lsp_position_of(text: &str, byte_offset: usize) -> (usize, usize) {
    let mut offset = byte_offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.bytes().filter(|&b| b == b'\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let mut segment = &text[line_start..offset];
    // `\r\n` の `\r` は桁に数えない（`\r` と `\n` のあいだを指されても `\r` の手前と同じ）
    if segment.ends_with('\r') && text[offset..].starts_with('\n') {
        segment = &segment[..segment.len() - 1];
    }
    let column = segment.chars().map(char::len_utf16).sum();
    (line, column)
}

/// LSP の `(0 起点の行, UTF-16 桁)` → UTF-8 バイト位置
pub fn byte_offset_of_lsp_position(text: &str, line: usize, utf16_col: usize) -> usize {
    let Some(line_start) = line_start_of(text, line) else {
        return text.len();
    };
    byte_in_line(text, line_start, utf16_col)
}

/// 行頭 `line_start` からの UTF-16 桁 → UTF-8 バイト位置（丸めの正本。[`LineIndex`] も通る）
fn byte_in_line(text: &str, line_start: usize, utf16_col: usize) -> usize {
    let rest = &text[line_start..];
    let mut line_end = rest.find('\n').map_or(text.len(), |i| line_start + i);
    if line_end > line_start && text.as_bytes()[line_end - 1] == b'\r' && line_end < text.len() {
        line_end -= 1;
    }
    let mut units = 0;
    for (index, ch) in text[line_start..line_end].char_indices() {
        let width = ch.len_utf16();
        // 文字の途中（サロゲートペアの片割れ）を指す桁は手前の境界へ丸める
        if units + width > utf16_col {
            return line_start + index;
        }
        units += width;
    }
    line_end
}

/// 0 起点の `line` 行目の先頭バイト位置。行が無ければ `None`
fn line_start_of(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    text.match_indices('\n').nth(line - 1).map(|(i, _)| i + 1)
}

/// 同じ本文へ位置を何度も当てるときの行頭の索引（#1679 の診断の取り込み）。
///
/// [`byte_offset_of_lsp_position`] は呼ぶたびに行頭を数え直す（1 回 O(本文)）ので、
/// 診断を数百件まとめて写すと本文 × 件数になる。これは行頭を 1 回だけ数え、
/// **行の中の丸めは同じ関数（`byte_in_line`）を通す**（入口が増えても丸めは 1 実装のまま。
/// 一致はテストが全位置で固定する）
pub struct LineIndex<'a> {
    text: &'a str,
    /// 各行の先頭バイト位置（`starts[0] == 0`。本文の `\n` の数 + 1 個）
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    /// [`byte_offset_of_lsp_position`] と同じ答え
    pub fn byte_offset(&self, line: usize, utf16_col: usize) -> usize {
        match self.starts.get(line) {
            Some(&start) => byte_in_line(self.text, start, utf16_col),
            None => self.text.len(),
        }
    }

    /// LSP の位置 → `(0 起点の行, 行内の UTF-8 バイト桁)`。
    /// 最終行を超える行は本文の末尾の位置になる（丸めは [`Self::byte_offset`] と同じ）
    pub fn line_byte_col(&self, line: usize, utf16_col: usize) -> (usize, usize) {
        let offset = self.byte_offset(line, utf16_col);
        let line = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line, offset - self.starts[line])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 両方向の固定値（受け入れ条件: 日本語 / 絵文字 / タブ / CRLF / 行末 / 空行 / 空ファイル）
    #[test]
    fn 空ファイルは原点しか持たない() {
        assert_eq!(lsp_position_of("", 0), (0, 0));
        assert_eq!(lsp_position_of("", 9), (0, 0));
        assert_eq!(byte_offset_of_lsp_position("", 0, 0), 0);
        assert_eq!(byte_offset_of_lsp_position("", 3, 5), 0);
    }

    #[test]
    fn ascii_は_バイトと桁が一致する() {
        let text = "abc\ndef";
        assert_eq!(lsp_position_of(text, 5), (1, 1));
        assert_eq!(byte_offset_of_lsp_position(text, 1, 1), 5);
    }

    #[test]
    fn 日本語は1文字3バイトで1桁() {
        let text = "あいう";
        assert_eq!(lsp_position_of(text, 3), (0, 1));
        assert_eq!(lsp_position_of(text, 6), (0, 2));
        assert_eq!(byte_offset_of_lsp_position(text, 0, 2), 6);
        assert_eq!(byte_offset_of_lsp_position(text, 0, 3), 9);
    }

    #[test]
    fn 文字の途中のバイト位置は手前の境界へ丸める() {
        let text = "あいう";
        assert_eq!(lsp_position_of(text, 4), (0, 1));
        assert_eq!(lsp_position_of(text, 5), (0, 1));
    }

    #[test]
    fn 絵文字はサロゲートペアで2桁() {
        let text = "a😀b";
        assert_eq!(lsp_position_of(text, 1), (0, 1));
        assert_eq!(lsp_position_of(text, 5), (0, 3));
        assert_eq!(byte_offset_of_lsp_position(text, 0, 3), 5);
        assert_eq!(byte_offset_of_lsp_position(text, 0, 4), 6);
    }

    #[test]
    fn サロゲートペアの片割れを指す桁は手前の境界へ丸める() {
        let text = "a😀b";
        assert_eq!(byte_offset_of_lsp_position(text, 0, 2), 1);
        // 絵文字の途中のバイト位置も絵文字の手前へ
        assert_eq!(lsp_position_of(text, 3), (0, 1));
    }

    #[test]
    fn タブは1桁() {
        let text = "\tx\n\t\ty";
        assert_eq!(lsp_position_of(text, 1), (0, 1));
        assert_eq!(lsp_position_of(text, 5), (1, 2));
        assert_eq!(byte_offset_of_lsp_position(text, 1, 2), 5);
    }

    #[test]
    fn crlf_の_cr_は桁に数えない() {
        let text = "ab\r\ncd";
        assert_eq!(lsp_position_of(text, 2), (0, 2));
        // `\r` と `\n` のあいだは `\r` の手前と同じ
        assert_eq!(lsp_position_of(text, 3), (0, 2));
        assert_eq!(lsp_position_of(text, 4), (1, 0));
        assert_eq!(byte_offset_of_lsp_position(text, 1, 1), 5);
    }

    #[test]
    fn crlf_の行末を超える桁は_cr_の手前へ丸める() {
        let text = "ab\r\ncd";
        assert_eq!(byte_offset_of_lsp_position(text, 0, 2), 2);
        assert_eq!(byte_offset_of_lsp_position(text, 0, 9), 2);
    }

    #[test]
    fn 行末と最終改行の後ろ() {
        let text = "abc\n";
        assert_eq!(lsp_position_of(text, 3), (0, 3));
        assert_eq!(lsp_position_of(text, 4), (1, 0));
        assert_eq!(byte_offset_of_lsp_position(text, 0, 99), 3);
        assert_eq!(byte_offset_of_lsp_position(text, 1, 0), 4);
    }

    #[test]
    fn 空行が続いても行は改行の数で決まる() {
        let text = "\n\n\nx";
        assert_eq!(lsp_position_of(text, 2), (2, 0));
        assert_eq!(lsp_position_of(text, 4), (3, 1));
        assert_eq!(byte_offset_of_lsp_position(text, 2, 5), 2);
        assert_eq!(byte_offset_of_lsp_position(text, 3, 1), 4);
    }

    #[test]
    fn 最終行を超える行は本文の末尾へ丸める() {
        assert_eq!(byte_offset_of_lsp_position("ab", 10, 0), 2);
        assert_eq!(byte_offset_of_lsp_position("ab\ncd", 2, 0), 5);
    }

    #[test]
    fn 範囲外のバイト位置は末尾へ丸める() {
        assert_eq!(lsp_position_of("ab\nc", 99), (1, 1));
    }

    #[test]
    fn 日本語と絵文字と_crlf_が混ざった行() {
        let text = "日本😀語\r\n次の行";
        // 「語」の手前 = 日(3) 本(3) 😀(4) = 10 バイト / 1 + 1 + 2 = 4 桁
        assert_eq!(lsp_position_of(text, 10), (0, 4));
        assert_eq!(byte_offset_of_lsp_position(text, 0, 4), 10);
        // 「語」の後ろ（`\r` の手前）
        assert_eq!(lsp_position_of(text, 13), (0, 5));
        // 2 行目の「の」
        assert_eq!(lsp_position_of(text, 18), (1, 1));
        assert_eq!(byte_offset_of_lsp_position(text, 1, 1), 18);
    }

    /// 行頭の索引（#1679）は、どの行・どの桁でも 1 回ずつ数え直す版と同じ答えを返す
    /// （丸め = 文字の途中・サロゲートの片割れ・CRLF・行末超え・最終行超えまで含めて）
    #[test]
    fn 行頭の索引は数え直す版と全位置で一致する() {
        for text in [
            "",
            "ab",
            "abc\n",
            "\n\n\nx",
            "ab\r\ncd",
            "日本😀語\r\n次の行",
            "fn 主() {\r\n\t😀 = \"é\";\n\n}\n",
        ] {
            let index = LineIndex::new(text);
            let lines = text.matches('\n').count() + 1;
            for line in 0..lines + 2 {
                for col in 0..14 {
                    let expected = byte_offset_of_lsp_position(text, line, col);
                    assert_eq!(
                        index.byte_offset(line, col),
                        expected,
                        "{text:?} ({line}, {col})"
                    );
                    let (l, c) = index.line_byte_col(line, col);
                    let start = text[..expected].rfind('\n').map_or(0, |i| i + 1);
                    let want_line = text[..expected].matches('\n').count();
                    assert_eq!(
                        (l, c),
                        (want_line, expected - start),
                        "{text:?} ({line}, {col})"
                    );
                }
            }
        }
    }

    /// すべての文字境界で「行き → 帰り」が元へ戻る（`\r` と `\n` のあいだは `\r` の手前へ）
    #[test]
    fn 往復はすべての文字境界で元へ戻る() {
        let text = "fn 主() {\r\n\t😀 = \"é\";\n\n}\n";
        for offset in 0..=text.len() {
            if !text.is_char_boundary(offset) {
                continue;
            }
            let (line, col) = lsp_position_of(text, offset);
            let back = byte_offset_of_lsp_position(text, line, col);
            let between_crlf = offset > 0
                && text.as_bytes()[offset - 1] == b'\r'
                && text[offset..].starts_with('\n');
            let expected = if between_crlf { offset - 1 } else { offset };
            assert_eq!(back, expected, "offset {offset} → ({line}, {col}) → {back}");
        }
    }
}
