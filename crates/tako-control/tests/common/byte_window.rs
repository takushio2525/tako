//! 見つけた位置から固定のバイト幅で文字列の窓を切る（番犬テストの共有部品。#2043）
//!
//! 番犬は「目印の直後 400 バイトに `Request::X` がある」「呼び出しの手前 1200 バイトに
//! ゲートがある」のように、**目印の近傍を固定の幅**で見ることが多い。これを
//! `&src[at..at + 400]` と添字で書くと、幅の端が**多バイト文字の途中**に来た瞬間に
//! `byte index N is not a char boundary` で panic する。番犬が見る製品コードには日本語の
//! コメントと文言が多いので、**検査と無関係な PR が日本語を 1 行足しただけ**で落ちる
//! （#2020 の `agent_resume_watchdog` が `if !has_children {` から 1200 バイトで踏んだ）。
//!
//! ここは幅の端を**外側へ**文字境界まで広げる（前方の窓は終端を後ろへ・後方の窓は始端を
//! 前へ。広がるのは最大 3 バイト）。内側へ丸めると、端にあった目印の文字列が窓から外れて
//! **番犬が見るべきものを黙って見なくなる**向きに倒れるので採らない。
//! 起点（`at`）は `find` が返した位置を渡す前提で、丸めない（文字境界でなければ落とす）。
//!
//! 固定幅の添字を直書きしないことは `issue2043_byte_window_watchdog` が縛る。
//! 他の crate の `tests/` から使うときも `#[path]` でこのファイルを取り込む（写さない）。
//! 製品ファイルの `#[cfg(test)]` からは取り込めないので、同じ向き（外側）へ std の
//! `ceil_char_boundary` / `floor_char_boundary` で丸める（#1790 の番犬が見る）。
//! 規約は `.agent/conventions.md`「文字列をバイト位置で切らない」

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

fn assert_anchor(text: &str, at: usize) {
    assert!(
        at <= text.len() && text.is_char_boundary(at),
        "窓の起点 {at} が文字境界ではない（長さ {}）。起点は `find` などが返した位置を渡す（#2043）",
        text.len()
    );
}

/// `at` から後ろへ `width` バイトの窓（終端は文字境界まで後ろへ広げる・文字列の末尾で止まる）
pub fn after(text: &str, at: usize, width: usize) -> &str {
    assert_anchor(text, at);
    let end = text.ceil_char_boundary(at.saturating_add(width).min(text.len()));
    &text[at..end]
}

/// `at` の手前 `width` バイトの窓（始端は文字境界まで前へ広げる・文字列の先頭で止まる）
pub fn before(text: &str, at: usize, width: usize) -> &str {
    assert_anchor(text, at);
    let start = text.floor_char_boundary(at.saturating_sub(width));
    &text[start..at]
}

/// 先頭から `width` バイト（終端は文字境界まで後ろへ広げる）。失敗の見出しに原文の頭を出す用
pub fn head(text: &str, width: usize) -> &str {
    after(text, 0, width)
}
