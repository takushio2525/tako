//! 範囲添字（`[a..]` / `[..b]` / `[a..b]` / `[..=b]`）を見つける（番犬テストの共有部品。#1757）
//!
//! 文字列の添字は**バイト位置**なので、多バイト文字の途中に当たると panic する。
//! 同じ型が #1728（プレビューの描画中）/ #1746（tako-cli の gate）/ #1756（テーマ色の長さを
//! バイト長で検査）と 3 回続いたので、番犬の検出器をこの 1 実装にまとめた
//! （`issue1728_byte_truncate_watchdog` と `issue1746_cli_byte_slice_watchdog` が共有する）。
//!
//! 受け取るのは `production_range::code_view_of` で**コメントと文字列を潰した眺め**
//! （バイト長と行番号が保たれるので `file:line` がそのまま使える）。字面から型は読めないので
//! `Vec` / `&[u8]` のスライスも同じ形で拾う。番犬ごとの絞り方は [`RangeIndex`] の分類
//! （[`RangeIndex::is_prefix`] = #1728 / [`RangeIndex::is_rounded`] と [`has_reason`] = #1746）で選ぶ。
//! 規約は `.agent/conventions.md`「文字列をバイト位置で切らない」

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

/// 理由コメントの目印（同じ行か 1 行上）
pub const REASON_MARK: &str = "切り出し安全:";

/// 見つけた範囲添字 1 つ
#[derive(Debug, Clone)]
pub struct RangeIndex {
    /// 開き括弧 `[` のバイト位置
    pub open: usize,
    /// 1 始まりの行番号
    pub line: usize,
    /// 括弧の中身（`..120` / `a..b` など）
    pub inner: String,
}

impl RangeIndex {
    /// 先頭からの範囲（`[..b]` / `[..=b]`）
    pub fn is_prefix(&self) -> bool {
        self.inner.trim_start().starts_with("..")
    }

    /// 添字を文字境界へ丸めてある（`floor_char_boundary(` / `ceil_char_boundary(`）
    pub fn is_rounded(&self) -> bool {
        self.inner.contains("floor_char_boundary(") || self.inner.contains("ceil_char_boundary(")
    }
}

/// 範囲添字を出現順に返す。
///
/// 添字として数えるのは「直前が識別子・`)`・`]`・`?`」の `[` だけ（配列リテラル・型・
/// 属性 `#[`・マクロ `vec![` は直前がそれ以外）。中身の最上位に `..` があり、`,` / `;` が
/// 無いもの（`[a, .., b]` のスライスパターンや `[0; n]` を外す）で、`[..]`（全体）は数えない
pub fn range_indexes(code: &str) -> Vec<RangeIndex> {
    let bytes = code.as_bytes();
    let mut hits = Vec::new();
    for (open, &b) in bytes.iter().enumerate() {
        if b != b'[' || open == 0 {
            continue;
        }
        let prev = bytes[open - 1];
        if !(prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b')' | b']' | b'?')) {
            continue;
        }
        let mut depth = 0i32;
        let mut close = None;
        for (at, &c) in bytes.iter().enumerate().skip(open) {
            match c {
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(at);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let inner = &code[open + 1..close];
        let mut nest = 0i32;
        let mut top = String::new();
        for c in inner.chars() {
            match c {
                '[' | '(' | '{' => nest += 1,
                ']' | ')' | '}' => nest -= 1,
                _ if nest == 0 => top.push(c),
                _ => {}
            }
        }
        if !top.contains("..") || top.contains(',') || top.contains(';') || inner.trim() == ".." {
            continue;
        }
        hits.push(RangeIndex {
            open,
            line: line_at(code, open),
            inner: inner.to_string(),
        });
    }
    hits
}

/// バイト位置の行番号（1 始まり）
pub fn line_at(text: &str, at: usize) -> usize {
    text.as_bytes()[..at]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// 原文の同じ行か 1 行上（注釈行）に [`REASON_MARK`] の理由コメントがある。
/// 2 行以上離れた目印は数えない（理由は切り出しのすぐそばに置く）
pub fn has_reason(raw_lines: &[&str], line: usize) -> bool {
    let here = raw_lines.get(line - 1).copied().unwrap_or("");
    let above = line
        .checked_sub(2)
        .and_then(|i| raw_lines.get(i))
        .copied()
        .unwrap_or("");
    here.contains(REASON_MARK)
        || (above.trim_start().starts_with("//") && above.contains(REASON_MARK))
}
