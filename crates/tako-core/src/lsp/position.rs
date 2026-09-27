//! UTF-8 バイト位置 ⇄ LSP の位置（0 起点の行・UTF-16 コードユニットの桁）の変換（#1678 / #1769）
//!
//! `TextBuffer` は全てを UTF-8 のバイト位置で持つ（`text_edit` の不変条件）。一方 LSP 3.17 の
//! 位置の既定は **UTF-16 のコードユニット**で、tako は `positionEncodings` に UTF-16 だけを
//! 申告する（Zed と同じ。`.agent/plans/2026-09-lsp-s1.md` §16-3）。
//!
//! **変換はこのモジュールだけが持つ**。サーバが仮に `utf-8` を選んできても別の経路を作らない
//! （経路が 2 本あるとサーバごとに不具合が割れる）。
//!
//! ## 行の区切り（#1769）
//!
//! 「行」が 2 種類あり、どちらもここで決める:
//!
//! - **LSP の行** = `\n` / `\r\n` / **単独の `\r`** で区切る（LSP 3.17 の仕様がこの 3 つを定める）
//! - **tako の行** = `\n` で区切る（`\r\n` の `\r` は行末。単独の `\r` は行内の文字 =
//!   `text_edit::LineEnding` の #1650 の決め）。診断・定義ジャンプの答えを tako の座標へ
//!   写すとき（[`LineIndex::line_byte_col`]）と、tako の座標から問い合わせるとき
//!   （[`lsp_position_of_line_col`]）はこちら
//!
//! ただし**実サーバは仕様どおりに数えない**（2026-09-27 の実測。単独 CR を含む本文を didOpen し、
//! 定義ジャンプを 2 通りの座標で問う / 未定義の名前の診断の行を見る）:
//!
//! | サーバ | 問い合わせの位置 | 診断の位置 |
//! |---|---|---|
//! | rust-analyzer 1.95.0 | `\n` だけで数える（仕様の座標には `null`） | — |
//! | clangd（Apple 17.0.0） | `\n` だけで数える（仕様の座標には `[]`） | **仕様どおり** |
//! | pyright | — | 仕様どおり |
//! | typescript-language-server（TypeScript 5） | — | 仕様どおり |
//!
//! clangd は 1 つのサーバの中で数え方が食い違う。つまり「サーバごとの数え方」を表に持っても
//! 揃わない。そこで**サーバへ送る本文は単独の `\r` を `\n` に置き換える**（[`wire_text`] /
//! [`wire_range`]）。バイト数も UTF-16 の幅も変わらないので位置は 1 対 1 のまま、
//! サーバの側には単独の `\r` が 1 つも無くなり、どの数え方のサーバでも同じ行になる
//! （上の 2 つは置き換えた本文で仕様の座標に答えることも実測した）。tako の本文
//! （`TextBuffer` とサーバへ送った写し）は 1 バイトも書き換えない = 保存は変更前とバイト一致。
//!
//! 残る限界: **開いていないファイル**（サーバがディスクから自分で読んだもの）は置き換えられない。
//! 定義ジャンプの飛び先がそういうファイルで、しかも単独の `\r` を含むときは、`\n` だけで数える
//! rust-analyzer に限って行がずれうる（clangd の飛び先・pyright・TypeScript は仕様どおり）。
//!
//! ## 境界の扱い（丸めの方針）
//!
//! - UTF-8 の文字の途中を指すバイト位置は**手前の境界へ丸める**（`text_edit` の
//!   `snap_boundary` と同じ方針）
//! - サロゲートペアの片割れを指す UTF-16 桁も**手前の境界へ丸める**
//!   （絵文字 U+1F600 は UTF-16 で 2 コードユニット・UTF-8 で 4 バイト）
//! - 改行（`\n` / `\r\n` / 単独の `\r`）は**桁に数えない**。`\r\n` の `\r` と `\n` のあいだを
//!   指すバイト位置は `\r` の手前と同じ位置になる
//! - 行末を超える桁はその行の末尾（改行の手前）へ、最終行を超える行は本文の末尾へ丸める
//!
//! 外（CLI / MCP）から来る行・桁は `text_edit::RangeEditError` で**丸めずに拒否**するが、
//! こちらは LSP サーバとのあいだの変換で、サーバの版と数文字ずれた位置が来るのは
//! 正常系（非同期にすれ違う）なので丸める。

use std::borrow::Cow;
use std::ops::Range;

/// `i` の `\r` が単独（直後が `\n` でない）か
fn is_lone_cr(bytes: &[u8], i: usize) -> bool {
    bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n')
}

/// `i` のバイトで LSP の行が終わるか（`\n` / 単独の `\r`。`\r\n` は `\n` の側で数える）
fn ends_lsp_line(bytes: &[u8], i: usize) -> bool {
    bytes[i] == b'\n' || is_lone_cr(bytes, i)
}

/// 文字の途中を指すバイト位置を手前の境界へ
fn snap(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// UTF-8 バイト位置 → LSP の `(0 起点の行, UTF-16 桁)`
pub fn lsp_position_of(text: &str, byte_offset: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    let mut offset = snap(text, byte_offset);
    // `\r\n` のあいだは `\r` の手前と同じ（改行は桁に数えない）
    if offset > 0 && bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
        offset -= 1;
    }
    let mut line = 0;
    let mut line_start = 0;
    for i in 0..offset {
        if ends_lsp_line(bytes, i) {
            line += 1;
            line_start = i + 1;
        }
    }
    let column = text[line_start..offset].chars().map(char::len_utf16).sum();
    (line, column)
}

/// LSP の `(0 起点の行, UTF-16 桁)` → UTF-8 バイト位置
pub fn byte_offset_of_lsp_position(text: &str, line: usize, utf16_col: usize) -> usize {
    let Some(line_start) = lsp_line_start(text, line) else {
        return text.len();
    };
    byte_in_line(text, line_start, utf16_col)
}

/// tako の `(0 起点の行, 行内の UTF-8 バイト桁)` → LSP の `(0 起点の行, UTF-16 桁)`（#1769）。
///
/// tako の行は `\n` で区切る（単独の `\r` は行内の文字）ので、単独の `\r` を含む行の後ろ半分は
/// LSP では次の行になる。行を超える桁はその行の末尾（改行の手前）へ、無い行は本文の末尾へ丸める
pub fn lsp_position_of_line_col(text: &str, line: usize, byte_col: usize) -> (usize, usize) {
    let start = if line == 0 {
        Some(0)
    } else {
        text.match_indices('\n').nth(line - 1).map(|(i, _)| i + 1)
    };
    let Some(start) = start else {
        return lsp_position_of(text, text.len());
    };
    let end = match text[start..].find('\n') {
        Some(i) if i > 0 && text.as_bytes()[start + i - 1] == b'\r' => start + i - 1,
        Some(i) => start + i,
        None => text.len(),
    };
    lsp_position_of(text, start + byte_col.min(end - start))
}

/// 行頭 `line_start` からの UTF-16 桁 → UTF-8 バイト位置（丸めの正本。[`LineIndex`] も通る）
fn byte_in_line(text: &str, line_start: usize, utf16_col: usize) -> usize {
    // 行末 = 最初の `\r` か `\n`（`\r\n` も単独の `\r` も `\r` の手前で行が終わる）
    let line_end = text[line_start..]
        .find(['\r', '\n'])
        .map_or(text.len(), |i| line_start + i);
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

/// 0 起点の LSP の `line` 行目の先頭バイト位置。行が無ければ `None`
fn lsp_line_start(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    let bytes = text.as_bytes();
    (0..bytes.len())
        .filter(|&i| ends_lsp_line(bytes, i))
        .nth(line - 1)
        .map(|i| i + 1)
}

/// サーバへ送る本文（#1769）: 単独の `\r` を `\n` に置き換える。
///
/// バイト数も UTF-16 の幅も変わらない（どちらも 1）ので、[`lsp_position_of`] の位置は
/// そのまま置き換えた本文の位置でもある。単独の `\r` が無ければ借りたまま返す
pub fn wire_text(text: &str) -> Cow<'_, str> {
    wire_range(text, 0..text.len())
}

/// 本文の `range` をサーバへ送る形にする（`didChange` の差し替え部分。#1769）。
///
/// `\r` が単独かどうかは**本文全体の次の 1 バイト**で決める（範囲の最後の `\r` の後ろに
/// 範囲外の `\n` が続くなら `\r\n` の片割れなので置き換えない）
pub fn wire_range(text: &str, range: Range<usize>) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let slice = &text[range.clone()];
    if !range.clone().any(|i| is_lone_cr(bytes, i)) {
        return Cow::Borrowed(slice);
    }
    let out: Vec<u8> = range
        .map(|i| {
            if is_lone_cr(bytes, i) {
                b'\n'
            } else {
                bytes[i]
            }
        })
        .collect();
    // ASCII の 1 バイトを ASCII の 1 バイトへ替えただけなので UTF-8 のまま
    Cow::Owned(
        String::from_utf8(out)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    )
}

/// 同じ本文へ位置を何度も当てるときの行頭の索引（#1679 の診断の取り込み）。
///
/// [`byte_offset_of_lsp_position`] は呼ぶたびに行頭を数え直す（1 回 O(本文)）ので、
/// 診断を数百件まとめて写すと本文 × 件数になる。これは行頭を 1 回だけ数え、
/// **行の中の丸めは同じ関数（`byte_in_line`）を通す**（入口が増えても丸めは 1 実装のまま。
/// 一致はテストが全位置で固定する）
pub struct LineIndex<'a> {
    text: &'a str,
    /// LSP の各行の先頭バイト位置（`lsp_starts[0] == 0`）
    lsp_starts: Vec<usize>,
    /// tako の各行（`\n` 区切り）の先頭バイト位置。単独の `\r` が無ければ LSP と同じなので持たない
    tako_starts: Option<Vec<usize>>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let mut lsp_starts = vec![0];
        let mut lone_cr = false;
        for i in 0..bytes.len() {
            if ends_lsp_line(bytes, i) {
                lone_cr |= bytes[i] == b'\r';
                lsp_starts.push(i + 1);
            }
        }
        let tako_starts = lone_cr.then(|| {
            let mut starts = vec![0];
            starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
            starts
        });
        Self {
            text,
            lsp_starts,
            tako_starts,
        }
    }

    /// [`byte_offset_of_lsp_position`] と同じ答え
    pub fn byte_offset(&self, line: usize, utf16_col: usize) -> usize {
        match self.lsp_starts.get(line) {
            Some(&start) => byte_in_line(self.text, start, utf16_col),
            None => self.text.len(),
        }
    }

    /// LSP の位置 → **tako の** `(0 起点の行, 行内の UTF-8 バイト桁)`（行は `\n` 区切り）。
    /// 最終行を超える行は本文の末尾の位置になる（丸めは [`Self::byte_offset`] と同じ）
    pub fn line_byte_col(&self, line: usize, utf16_col: usize) -> (usize, usize) {
        let offset = self.byte_offset(line, utf16_col);
        let starts = self.tako_starts.as_deref().unwrap_or(&self.lsp_starts);
        let line = match starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line, offset - starts[line])
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

    // --- 単独の `\r`（#1769）------------------------------------------------

    /// LSP の仕様では単独の `\r` も行の区切り（tako の行では行内の文字）
    #[test]
    fn 単独の_cr_は_lsp_の行を区切る() {
        let text = "ab\rcd\nef";
        assert_eq!(
            lsp_position_of(text, 2),
            (0, 2),
            "`\\r` の手前は 0 行目の末尾"
        );
        assert_eq!(lsp_position_of(text, 3), (1, 0), "`\\r` の後ろは次の行の頭");
        assert_eq!(lsp_position_of(text, 4), (1, 1));
        assert_eq!(lsp_position_of(text, 7), (2, 1), "`\\n` の後ろは 2 行目");
        assert_eq!(byte_offset_of_lsp_position(text, 1, 1), 4);
        assert_eq!(byte_offset_of_lsp_position(text, 2, 0), 6);
    }

    #[test]
    fn 単独の_cr_の行末を超える桁は_cr_の手前へ丸める() {
        let text = "ab\rcd";
        assert_eq!(byte_offset_of_lsp_position(text, 0, 9), 2);
        assert_eq!(byte_offset_of_lsp_position(text, 1, 9), 5);
    }

    #[test]
    fn 末尾の単独の_cr_の後ろは空の行() {
        let text = "ab\r";
        assert_eq!(lsp_position_of(text, 3), (1, 0));
        assert_eq!(byte_offset_of_lsp_position(text, 1, 0), 3);
        assert_eq!(byte_offset_of_lsp_position(text, 0, 5), 2);
    }

    /// `\r\r\n` = 単独の `\r` + `\r\n`（2 つの区切り）。`\n\r` = `\n` + 単独の `\r`
    #[test]
    fn cr_が続く並びも区切りを1つずつ数える() {
        let text = "a\r\r\nb\n\rc";
        assert_eq!(lsp_position_of(text, 2), (1, 0), "単独の `\\r` の後ろ");
        assert_eq!(
            lsp_position_of(text, 3),
            (1, 0),
            "`\\r\\n` のあいだは `\\r` の手前"
        );
        assert_eq!(lsp_position_of(text, 4), (2, 0));
        assert_eq!(lsp_position_of(text, 7), (4, 0), "`\\n\\r` は 2 つ");
        assert_eq!(byte_offset_of_lsp_position(text, 4, 0), 7);
        assert_eq!(byte_offset_of_lsp_position(text, 3, 0), 6);
    }

    /// tako の行（`\n` 区切り）の桁から問い合わせると、単独の `\r` の後ろは LSP の次の行になる
    #[test]
    fn tako_の行と桁から_lsp_の位置へ直す() {
        let text = "x\r\nab\r😀c\r\ny";
        // tako の 1 行目 = "ab\r😀c"。`c` はバイト桁 7（a b \r 😀 = 1 + 1 + 1 + 4）
        assert_eq!(lsp_position_of_line_col(text, 1, 7), (2, 2), "😀 = 2 桁");
        assert_eq!(lsp_position_of_line_col(text, 1, 2), (1, 2), "`\\r` の手前");
        assert_eq!(lsp_position_of_line_col(text, 1, 3), (2, 0), "`\\r` の後ろ");
        // 行を超える桁は行末（`\r\n` の手前）へ
        assert_eq!(lsp_position_of_line_col(text, 1, 99), (2, 3));
        assert_eq!(lsp_position_of_line_col(text, 2, 1), (3, 1));
        // 無い行は本文の末尾
        assert_eq!(lsp_position_of_line_col(text, 9, 0), (3, 1));
    }

    #[test]
    fn 送る本文は単独の_cr_だけを_lf_に替える() {
        assert!(matches!(wire_text("a\r\nb\nc"), Cow::Borrowed(_)));
        assert_eq!(wire_text("a\rb\r\nc\r"), "a\nb\r\nc\n");
        assert_eq!(wire_text("\r\r\n\r"), "\n\r\n\n");
        let text = "日\r本😀\r";
        assert_eq!(wire_text(text).len(), text.len(), "バイト数は変わらない");
    }

    /// 範囲の最後の `\r` が単独かは範囲の外の次の 1 バイトで決まる
    #[test]
    fn 範囲の送る形は範囲の外まで見て決める() {
        let text = "ab\r\ncd\rX";
        assert_eq!(
            wire_range(text, 1..3),
            "b\r",
            "後ろに `\\n` が続く `\\r` は残す"
        );
        assert_eq!(wire_range(text, 4..7), "cd\n", "後ろが `X` の `\\r` は単独");
        assert_eq!(wire_range(text, 3..3), "");
    }

    /// 送った本文（単独の `\r` を替えたもの）の上で `\n` だけで数えても、元の本文を仕様どおりに
    /// 数えても同じ位置になる = どちらの数え方のサーバとも食い違わない
    #[test]
    fn 送る本文の上ではどちらの数え方でも同じ位置() {
        let text = "fn a() {}\r// x\r\n\rlet 😀 = 1;\rb\r";
        let wire = wire_text(text);
        for offset in 0..=text.len() {
            if !text.is_char_boundary(offset) {
                continue;
            }
            let spec = lsp_position_of(text, offset);
            // `\n` だけで数える（rust-analyzer / clangd の問い合わせの数え方）
            let mut at = offset;
            if wire[..at].ends_with('\r') && wire[at..].starts_with('\n') {
                at -= 1;
            }
            let before = &wire[..at];
            let line = before.matches('\n').count();
            let start = before.rfind('\n').map_or(0, |i| i + 1);
            let column: usize = wire[start..at].chars().map(char::len_utf16).sum();
            assert_eq!(spec, (line, column), "offset {offset}");
        }
    }

    /// 行頭の索引（#1679）は、どの行・どの桁でも 1 回ずつ数え直す版と同じ答えを返す
    /// （丸め = 文字の途中・サロゲートの片割れ・CRLF・単独 CR・行末超え・最終行超えまで含めて）
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
            "a\rb\r\nc\r\r\nd\n\re\r",
        ] {
            let index = LineIndex::new(text);
            let lines = (0..text.len())
                .filter(|&i| ends_lsp_line(text.as_bytes(), i))
                .count()
                + 1;
            for line in 0..lines + 2 {
                for col in 0..14 {
                    let expected = byte_offset_of_lsp_position(text, line, col);
                    assert_eq!(
                        index.byte_offset(line, col),
                        expected,
                        "{text:?} ({line}, {col})"
                    );
                    // tako の座標は `\n` 区切り
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

    /// すべての文字境界で「行き → 帰り」が元へ戻る（`\r` と `\n` のあいだは `\r` の手前へ）。
    /// tako の座標からの行き（[`lsp_position_of_line_col`]）も同じ位置へ着く
    #[test]
    fn 往復はすべての文字境界で元へ戻る() {
        for text in ["fn 主() {\r\n\t😀 = \"é\";\n\n}\n", "a\rb\r\n😀\r\rc\n\r"] {
            let index = LineIndex::new(text);
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
                assert_eq!(back, expected, "{text:?} offset {offset} → ({line}, {col})");
                let (tako_line, tako_col) = index.line_byte_col(line, col);
                assert_eq!(
                    lsp_position_of_line_col(text, tako_line, tako_col),
                    (line, col),
                    "{text:?} offset {offset}"
                );
            }
        }
    }
}
