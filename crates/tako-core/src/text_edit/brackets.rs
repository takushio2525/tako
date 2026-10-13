//! 対応する括弧の探索（Issue #1691 / エピック #1647 の SE-3）。純粋関数だけで I/O をしない
//!
//! カーソルの隣の括弧と対になる括弧を探す。エディタはこの 2 文字を強調し、CLI / MCP は
//! 編集系の応答の `document.bracket_match` で同じ位置を読む（どちらも
//! [`TextBuffer::bracket_match`] の 1 実装を引く）。LSP・構文木とは独立に動く。
//!
//! ## カーソルの採り方（VS Code の `matchBracket` と Zed の `refresh_matching_bracket_highlights`）
//!
//! - **選択があるときは採らない**（両者とも「選択が畳まれているときだけ」）
//! - カーソルの**直後**の括弧を先に見て、対が無いときだけ**直前**の括弧を見る。
//!   `)|(` では右の `(` が勝つ（VS Code はカーソルの後ろのトークンを先に見て、Zed は
//!   同じ長さの組なら後に見つけた右の組を残す）
//! - **括弧に接していなければ採らない**（VS Code と同じ。Zed はカーソルを囲む最内の組を
//!   出すが、#1691 は「括弧に置いたら光る」で、打っているあいだ離れた括弧が光り続けると
//!   視線を取られる）
//! - 文字列・コメントの中の括弧は数えない。カーソルの隣がそれなら採らない
//!   （VS Code の `ignoreBracketsInToken`）
//! - **種類ごとに数える**（`(` は `(` と `)` だけを数え、`[` `{` は見ない = VS Code と同じ）。
//!   打っている途中の崩れた入れ子（`( [ )`）でも、同じ種類どうしは対になる
//!
//! ## 文字列とコメントの見分け方
//!
//! 言語ごとの記号は言語の同定の正本 `file_type` の行から引く（行コメント = `comment` 列 #1692、
//! 文字列とブロックコメント = `lexical` 列 #1691。[`BracketSyntax::for_row`]）。このファイルは
//! 記号の表を持たない。構文を解かない近似なので、次は見分けない: JS の正規表現リテラル・
//! テンプレートの `${…}` の中（文字列ごと飛ばす）・ヒアドキュメント・入れ子のブロックコメント・
//! C++ の生文字列 `R"(…)"`
//!
//! ## 所要（NFR-8）
//!
//! 文字列・コメントの中かどうかは先頭から読まないと決まらないので、1 回の探索は
//! 走査の起点から対の括弧まで 1 度だけ走る（O(n)）。GUI はこれを打鍵のたびに UI スレッドで
//! 呼ぶので、(版, カーソル) が同じあいだは [`TextBuffer`] が答えを使い回す。
//! 10 MB まで編集できる（#1660）ので、カーソルから [`SCAN_WINDOW`] より離れた所は読まない
//! （その外の括弧は対が無いのと同じ扱い。VS Code も探索を 20ms で打ち切る）

use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::file_type::{self, FileType};

use super::{CommentSyntax, TextBuffer};

/// カーソルの前後それぞれ、この幅（バイト）より離れた所は読まない。
///
/// 起点がこれより前に来るときは窓の中の最初の行頭から読み始める（その行頭は文字列・
/// コメントの外だと見なす = 1 MB を超える文書でだけ効く近似）。release 実測で 1 MB の走査は
/// 1 打鍵の予算（NFR-8。5,000 行で 1ms 未満）の内に収まる幅
pub const SCAN_WINDOW: usize = 1 << 20;

/// `'` の扱い（#1691）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleQuote {
    /// 区切りとして扱わない（文書。`don't` の `'`）
    None,
    /// 1 文字のリテラル（`'('` / `'\''`）だけを飛ばす。ほかの `'` は Rust のライフタイム・
    /// C++ の桁区切りなので何もしない
    Char,
    /// 文字列（Python / JS / シェル）。1 文字のリテラルの形も飛ばし、ほかは**同じ行で閉じる
    /// ときだけ**文字列にする（閉じなければアポストロフィ）。`'''…'''` は行を跨ぐ
    String,
}

/// 文字列とブロックコメントの書き方（#1691）。言語ごとの値は言語の同定の正本
/// （[`crate::file_type::TABLE`]）の `lexical` 列が持つ。行コメントは `comment` 列（#1692）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lexical {
    /// 行コメントとは別に持つブロックコメントの始まりと終わり（C の系譜の囲み）。入れ子は数えない
    pub block_comment: Option<(&'static str, &'static str)>,
    /// `"…"` を文字列として飛ばす（`"""…"""` と Rust の生文字列 `r#"…"#` も）。行を跨ぐ
    pub double_quote: bool,
    /// `'` の扱い
    pub single_quote: SingleQuote,
    /// `` `…` `` を文字列として飛ばす（JS のテンプレート・Go の生文字列・シェル）。行を跨ぐ
    pub backtick: bool,
}

impl Lexical {
    /// 何も飛ばさない（文書）
    pub const PROSE: Self = Self {
        block_comment: None,
        double_quote: false,
        single_quote: SingleQuote::None,
        backtick: false,
    };
}

/// 対応括弧の探索が中を飛ばす書き方（#1691）。`file_type` の行から [`Self::for_row`] で組む
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BracketSyntax {
    /// 行コメントの記号
    pub line_comment: Option<&'static str>,
    /// ブロックコメントの始まりと終わり（`comment` 列の囲みと `lexical` 列の囲み）
    pub block_comments: [Option<(&'static str, &'static str)>; 2],
    /// 文字列の書き方（`lexical` 列のうちブロックコメント以外）
    pub lexical: Lexical,
}

/// カーソルが採った括弧と、それと対になる括弧（#1691）。位置はどちらもバイト位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BracketMatch {
    /// カーソルの直後か直前の括弧
    pub bracket: usize,
    /// それと対になる括弧
    pub partner: usize,
}

impl BracketMatch {
    /// 開き括弧・閉じ括弧の順に並べた 2 つの位置（描画が両方を同じ色で塗る）
    pub fn ordered(self) -> [usize; 2] {
        if self.bracket <= self.partner {
            [self.bracket, self.partner]
        } else {
            [self.partner, self.bracket]
        }
    }
}

/// `TAKO_1691_LEGACY=1` で **#1691 前の挙動**へ戻す（同一バイナリで A/B を取る入口）:
/// 対応括弧を探さない = エディタは強調せず、応答の `bracket_match` は常に null
pub fn bracket_match_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1691_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 括弧の種類（0 = 丸・1 = 角・2 = 波）と、開きか
fn bracket_kind(b: u8) -> Option<(usize, bool)> {
    match b {
        b'(' => Some((0, true)),
        b')' => Some((0, false)),
        b'[' => Some((1, true)),
        b']' => Some((1, false)),
        b'{' => Some((2, true)),
        b'}' => Some((2, false)),
        _ => None,
    }
}

fn is_bracket_at(text: &str, at: usize) -> bool {
    text.as_bytes()
        .get(at)
        .is_some_and(|b| bracket_kind(*b).is_some())
}

/// 識別子の一部になるバイト（UTF-8 の多バイト文字の断片も含める = 日本語の直後の `'` は
/// アポストロフィ）
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// UTF-8 の先頭バイトから 1 文字のバイト数
fn utf8_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// `from` 以降で `needle` が始まる位置
fn find_seq(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut at = from;
    while at < hay.len() {
        let hit = at + hay[at..].iter().position(|b| *b == first)?;
        if hay[hit..].starts_with(needle) {
            return Some(hit);
        }
        at = hit + 1;
    }
    None
}

/// `from` から閉じの `quote` まで読み、その直後を返す（`\` は次の 1 バイトを飛ばす）。
/// `multiline` でなければ改行で打ち切り `None`。閉じないまま終われば `multiline` なら末尾
fn skip_quoted(bytes: &[u8], from: usize, quote: u8, multiline: bool) -> Option<usize> {
    let mut j = from;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'\n' if !multiline => return None,
            b if b == quote => return Some(j + 1),
            _ => j += 1,
        }
    }
    multiline.then_some(bytes.len())
}

/// `from` から 3 つ並びの閉じ（`"""` / `'''`）まで読み、その直後を返す（閉じなければ末尾）
fn skip_triple(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut j = from;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b if b == quote && bytes[j..].starts_with(&[quote; 3]) => return j + 3,
            _ => j += 1,
        }
    }
    bytes.len()
}

/// `at` の `'` が 1 文字のリテラル（`'x'` / `'あ'` / `'\''` / `'\u{1F600}'`）なら、その直後
fn char_literal(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = bytes.get(at + 1..)?;
    match *rest.first()? {
        // エスケープは閉じの `'` まで 2〜11 バイト（`\u{10FFFF}` が最長）
        b'\\' => (2..=11)
            .map_while(|k| rest.get(k).copied().filter(|b| *b != b'\n').map(|b| (k, b)))
            .find(|(_, b)| *b == b'\'')
            .map(|(k, _)| at + 1 + k + 1),
        b'\'' | b'\n' | b'\r' => None,
        lead => {
            let n = utf8_len(lead);
            (rest.get(n) == Some(&b'\'')).then_some(at + 1 + n + 1)
        }
    }
}

/// `at` の `'` の直前が語か（文字列の接頭辞 `r` / `b` / `f` / `u` とその 2 文字の組は除く）
fn after_word(bytes: &[u8], at: usize) -> bool {
    // 接頭辞は 2 文字までなので、3 文字さかのぼれば足りる（長い語の手前で O(語長) にしない）
    let mut start = at;
    while start > 0 && at - start < 3 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    if start == at {
        return false;
    }
    let run = &bytes[start..at];
    let whole = start == 0 || !is_word(bytes[start - 1]);
    !(whole && run.len() <= 2 && run.iter().all(|b| b"rRbBfFuU".contains(b)))
}

impl BracketSyntax {
    /// 何も飛ばさない書き方（[`Lexical::PROSE`] でコメントも無い）
    pub const PLAIN: Self = Self {
        line_comment: None,
        block_comments: [None, None],
        lexical: Lexical::PROSE,
    };

    /// 形式の行から組む（行コメント = `comment` 列・文字列とブロックコメント = `lexical` 列）。
    /// `comment` 列が囲み（HTML の `<!-- -->`・CSS の `/* */`）なら、それもブロックコメントとして飛ばす
    pub fn for_row(row: &FileType) -> Self {
        let (line_comment, comment_block) = match row.comment {
            Some(CommentSyntax::Line(token)) => (Some(token), None),
            Some(CommentSyntax::Block { open, close }) => (None, Some((open, close))),
            None => (None, None),
        };
        Self {
            line_comment,
            block_comments: [comment_block, row.lexical.block_comment],
            lexical: row.lexical,
        }
    }

    /// パスの書き方（同定は [`file_type::identify_or_default`] の 1 実装。表に無い形式は既定の行）
    pub fn for_path(path: &Path) -> Self {
        Self::for_row(file_type::identify_or_default(path))
    }

    /// `at` から行コメントが始まるか
    fn line_comment_at(&self, bytes: &[u8], at: usize) -> bool {
        let Some(token) = self.line_comment.map(str::as_bytes) else {
            return false;
        };
        let Some(head) = bytes.get(at..at + token.len()) else {
            return false;
        };
        let word_like = token.first().is_some_and(u8::is_ascii_alphanumeric);
        if !(head == token || word_like && head.eq_ignore_ascii_case(token)) {
            return false;
        }
        let prev = at.checked_sub(1).map(|p| bytes[p]);
        // `\%`（LaTeX）・`\#`（シェル）は記号そのもの
        if prev == Some(b'\\') {
            return false;
        }
        let blank = |b: Option<u8>| b.is_none_or(|b| b.is_ascii_whitespace());
        if word_like {
            // `REM` は行頭（インデントの後）にあって、後ろが空白か行末のときだけ
            let line_start = bytes[..at]
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |p| p + 1);
            bytes[line_start..at]
                .iter()
                .all(|b| matches!(b, b' ' | b'\t'))
                && blank(bytes.get(at + token.len()).copied())
        } else if token.first() == Some(&b'#') {
            // `#` は行頭か空白の直後だけ（`${#arr[@]}` / `$#` / YAML の `a#b` はコメントではない）
            blank(prev)
        } else {
            true
        }
    }

    /// `at` の `"` が始める文字列の直後
    fn skip_double(&self, bytes: &[u8], at: usize) -> usize {
        if bytes[at..].starts_with(b"\"\"\"") {
            return skip_triple(bytes, at + 3, b'"');
        }
        // Rust の生文字列 `r#"…"#` / `br##"…"##`: 直前の `#` の数で閉じが決まり、`\` は効かない。
        // 接頭辞が語の頭にあるときだけ（`#` は Rust の上限 255 個まで数える）
        let hashes = bytes[..at]
            .iter()
            .rev()
            .take(256)
            .take_while(|b| **b == b'#')
            .count();
        if let Some(r) = (at - hashes)
            .checked_sub(1)
            .filter(|r| hashes > 0 && bytes[*r] == b'r')
        {
            let start = if r > 0 && bytes[r - 1] == b'b' {
                r - 1
            } else {
                r
            };
            if start == 0 || !is_word(bytes[start - 1]) {
                let mut close = vec![b'"'];
                close.resize(hashes + 1, b'#');
                return find_seq(bytes, at + 1, &close).map_or(bytes.len(), |p| p + close.len());
            }
        }
        skip_quoted(bytes, at + 1, b'"', true).unwrap_or(bytes.len())
    }

    /// `at` の `'` が始める文字列・リテラルの直後（何も始めなければ `at + 1`）
    fn skip_single(&self, bytes: &[u8], at: usize) -> usize {
        if let Some(after) = char_literal(bytes, at) {
            return after;
        }
        if self.lexical.single_quote == SingleQuote::String && !after_word(bytes, at) {
            if bytes[at..].starts_with(b"'''") {
                return skip_triple(bytes, at + 3, b'\'');
            }
            if let Some(after) = skip_quoted(bytes, at + 1, b'\'', false) {
                return after;
            }
        }
        at + 1
    }

    /// `at` のバイトが始める文字列・コメントの直後（何も始めなければ `at + 1`）
    fn skip_at(&self, bytes: &[u8], at: usize) -> usize {
        for (open, close) in self.block_comments.iter().flatten() {
            if bytes[at..].starts_with(open.as_bytes()) {
                return find_seq(bytes, at + open.len(), close.as_bytes())
                    .map_or(bytes.len(), |p| p + close.len());
            }
        }
        if self.line_comment_at(bytes, at) {
            // 改行の手前まで（CRLF の CR もコメントの内）
            return bytes[at..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |p| at + p);
        }
        match bytes[at] {
            b'"' if self.lexical.double_quote => self.skip_double(bytes, at),
            b'\'' if self.lexical.single_quote != SingleQuote::None => self.skip_single(bytes, at),
            b'`' if self.lexical.backtick => {
                skip_quoted(bytes, at + 1, b'`', true).unwrap_or(bytes.len())
            }
            _ => at + 1,
        }
    }

    /// 走査で立ち止まるバイトの表（括弧と、文字列・コメントを始めうるバイト）
    fn stops(&self) -> [bool; 256] {
        let mut stops = [false; 256];
        for b in b"()[]{}" {
            stops[*b as usize] = true;
        }
        let mut mark = |b: u8| {
            stops[b as usize] = true;
            stops[b.to_ascii_lowercase() as usize] = true;
            stops[b.to_ascii_uppercase() as usize] = true;
        };
        if self.lexical.double_quote {
            mark(b'"');
        }
        if self.lexical.single_quote != SingleQuote::None {
            mark(b'\'');
        }
        if self.lexical.backtick {
            mark(b'`');
        }
        if let Some(first) = self.line_comment.and_then(|t| t.as_bytes().first()) {
            mark(*first);
        }
        for (open, _) in self.block_comments.iter().flatten() {
            if let Some(first) = open.as_bytes().first() {
                mark(*first);
            }
        }
        stops
    }
}

/// 文字列・コメントの外の括弧を前から順に返す走査
struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
    syntax: &'a BracketSyntax,
    stops: [bool; 256],
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str, window: Range<usize>, syntax: &'a BracketSyntax) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: window.start,
            end: window.end.min(text.len()),
            syntax,
            stops: syntax.stops(),
        }
    }

    /// 次の括弧（位置, バイト）
    fn next_bracket(&mut self) -> Option<(usize, u8)> {
        while self.pos < self.end {
            let at = self.pos;
            let b = self.bytes[at];
            if !self.stops[b as usize] {
                self.pos += 1;
                continue;
            }
            if bracket_kind(b).is_some() {
                self.pos += 1;
                return Some((at, b));
            }
            self.pos = self.syntax.skip_at(self.bytes, at).max(at + 1);
        }
        None
    }
}

/// 探索で読む範囲（カーソルの前後 [`SCAN_WINDOW`]）
fn scan_window(text: &str, at: usize) -> Range<usize> {
    let start = if at > SCAN_WINDOW {
        let mut from = at - SCAN_WINDOW;
        while !text.is_char_boundary(from) {
            from += 1;
        }
        text.as_bytes()[from..at]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(from, |p| from + p + 1)
    } else {
        0
    };
    start..at.saturating_add(SCAN_WINDOW).min(text.len())
}

/// 括弧の位置 `targets`（どちらも省略可。並びは前・後）の対を 1 度の走査で探す。
///
/// 後ろの目標（カーソルの採り方で優先する側）の対が見つかった時点で打ち切る。
/// 目標がすべて過ぎて待っている開き括弧が無くなったときも打ち切る
fn resolve(
    text: &str,
    targets: [Option<usize>; 2],
    window: Range<usize>,
    syntax: &BracketSyntax,
) -> [Option<usize>; 2] {
    let mut found = [None, None];
    let Some(last) = targets.iter().rposition(Option::is_some) else {
        return found;
    };
    let last_target = targets.iter().flatten().max().copied().unwrap_or(0);
    let index_of = |pos: usize| targets.iter().position(|t| *t == Some(pos));
    let mut stacks: [Vec<usize>; 3] = Default::default();
    // 目標の開き括弧のうち、まだ閉じを待っている数
    let mut pending = 0usize;
    let mut scanner = Scanner::new(text, window, syntax);
    while let Some((pos, b)) = scanner.next_bracket() {
        let Some((kind, open)) = bracket_kind(b) else {
            continue;
        };
        if open {
            stacks[kind].push(pos);
            if index_of(pos).is_some() {
                pending += 1;
            }
        } else if let Some(opener) = stacks[kind].pop() {
            if let Some(ix) = index_of(opener) {
                found[ix] = Some(pos);
                pending -= 1;
            }
            if let Some(ix) = index_of(pos) {
                found[ix] = Some(opener);
            }
            if found[last].is_some() {
                break;
            }
        }
        if pos >= last_target && pending == 0 {
            break;
        }
    }
    found
}

/// `at` の括弧と対になる括弧の位置（#1691）。
///
/// `at` が括弧でない・文字列やコメントの中・対が無いときは `None`。数えるのは同じ種類の
/// 括弧だけで、文字列・コメントの中は飛ばす（書き方は `syntax`）
pub fn find_partner(text: &str, at: usize, syntax: &BracketSyntax) -> Option<usize> {
    if !is_bracket_at(text, at) {
        return None;
    }
    resolve(text, [Some(at), None], scan_window(text, at), syntax)[0]
}

/// カーソル `cursor`（バイト位置）が採る括弧と、その対（#1691）。
///
/// 直後の括弧に対があればそれ、無ければ直前の括弧の対。どちらも無ければ `None`
/// （採り方はモジュールの説明）。選択の有無は呼び手（[`TextBuffer::bracket_match`]）が見る
pub fn match_at_cursor(text: &str, cursor: usize, syntax: &BracketSyntax) -> Option<BracketMatch> {
    let before = cursor.checked_sub(1).filter(|at| is_bracket_at(text, *at));
    let after = Some(cursor).filter(|at| is_bracket_at(text, *at));
    if before.is_none() && after.is_none() {
        return None;
    }
    let [before_partner, after_partner] =
        resolve(text, [before, after], scan_window(text, cursor), syntax);
    after
        .zip(after_partner)
        .or(before.zip(before_partner))
        .map(|(bracket, partner)| BracketMatch { bracket, partner })
}

/// 控えた答え: (版, カーソル) と、そのときの答え
type CachedMatch = ((u64, usize), Option<BracketMatch>);

/// 答えの置き場（#1691）。(版, カーソル) が同じあいだ使い回す
#[derive(Default)]
pub(super) struct BracketCache {
    last: Mutex<Option<CachedMatch>>,
    /// 探した回数。「描画のたびに探し直さない」を時間ではなく回数で固定する
    searches: AtomicUsize,
}

impl Clone for BracketCache {
    /// 答えは本文から作り直せる控えなので複製しない
    fn clone(&self) -> Self {
        Self {
            last: Mutex::new(None),
            searches: AtomicUsize::new(self.searches.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for BracketCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BracketCache")
            .field("searches", &self.searches.load(Ordering::Relaxed))
            .finish()
    }
}

impl BracketCache {
    /// 控えを捨てる（パスが変わって書き方が変わりうるとき）
    pub(super) fn clear(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = None;
        }
    }
}

impl TextBuffer {
    /// カーソルの隣の括弧と、それと対になる括弧（#1691）。
    ///
    /// 選択中・括弧に接していない・対が無いときは `None`。書き方はこのバッファのパスで
    /// 言語の同定の正本を引いて決める（[`BracketSyntax::for_path`]）。
    /// (版, カーソル) が同じあいだは探し直さない（GUI は表示中の行ごとに呼ぶ）
    pub fn bracket_match(&self) -> Option<BracketMatch> {
        if bracket_match_legacy() || self.selection().is_some() {
            return None;
        }
        let key = (self.version, self.cursor);
        let mut last = self.brackets.last.lock().ok()?;
        if let Some((cached, answer)) = *last {
            if cached == key {
                return answer;
            }
        }
        self.brackets.searches.fetch_add(1, Ordering::Relaxed);
        let answer = match_at_cursor(
            &self.text,
            self.cursor,
            &BracketSyntax::for_path(&self.path),
        );
        *last = Some((key, answer));
        answer
    }

    /// 対応括弧を探した回数（#1691。テストが「同じ版・カーソルなら探し直さない」を数で見る）
    pub fn bracket_searches(&self) -> usize {
        self.brackets.searches.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// C の系譜（行コメント `//`・ブロック `/* */`・`"…"`・1 文字のリテラル）
    const C_LIKE: BracketSyntax = BracketSyntax {
        line_comment: Some("//"),
        block_comments: [None, Some(("/*", "*/"))],
        lexical: Lexical {
            block_comment: Some(("/*", "*/")),
            double_quote: true,
            single_quote: SingleQuote::Char,
            backtick: false,
        },
    };

    /// スクリプトの系譜（行コメント `#`・`"…"` と `'…'` の文字列・`` `…` ``）
    const SCRIPT: BracketSyntax = BracketSyntax {
        line_comment: Some("#"),
        block_comments: [None, None],
        lexical: Lexical {
            block_comment: None,
            double_quote: true,
            single_quote: SingleQuote::String,
            backtick: true,
        },
    };

    /// `text` の中で `nth` 番目（0 始まり）に現れる `needle` のバイト位置
    fn nth(text: &str, needle: &str, n: usize) -> usize {
        text.match_indices(needle)
            .nth(n)
            .unwrap_or_else(|| panic!("{needle:?} の {n} 番目が {text:?} に無い"))
            .0
    }

    // ─── 受け入れ条件 1: 5 パターン（対が無いときは None） ───────────────────

    #[test]
    fn ネストした括弧は同じ深さの対を返す() {
        let text = "f(a, (b), [c{d}])";
        assert_eq!(find_partner(text, 1, &C_LIKE), Some(16));
        assert_eq!(find_partner(text, 16, &C_LIKE), Some(1));
        assert_eq!(find_partner(text, 5, &C_LIKE), Some(7));
        assert_eq!(find_partner(text, 10, &C_LIKE), Some(15));
        assert_eq!(find_partner(text, 12, &C_LIKE), Some(14));
    }

    #[test]
    fn 対の無い括弧は_none() {
        let text = "f(a, (b)";
        assert_eq!(find_partner(text, 1, &C_LIKE), None);
        assert_eq!(find_partner(text, 5, &C_LIKE), Some(7));
        // 閉じ括弧だけ
        assert_eq!(find_partner(")", 0, &C_LIKE), None);
        assert_eq!(find_partner("a])", 1, &C_LIKE), None);
        // 括弧でない位置・範囲外
        assert_eq!(find_partner(text, 0, &C_LIKE), None);
        assert_eq!(find_partner(text, 99, &C_LIKE), None);
    }

    #[test]
    fn 文字列の中の括弧は数えない() {
        let text = r#"f("(", x, ')', "\")")"#;
        let close = text.len() - 1;
        assert_eq!(find_partner(text, 1, &C_LIKE), Some(close));
        assert_eq!(find_partner(text, close, &C_LIKE), Some(1));
        // 文字列の中の括弧そのものは採らない
        assert_eq!(find_partner(text, nth(text, "(", 1), &C_LIKE), None);
        assert_eq!(find_partner(text, nth(text, ")", 0), &C_LIKE), None);
    }

    #[test]
    fn コメントの中の括弧は数えない() {
        let text = "f(x // )\n  /* ( */ )";
        let close = text.len() - 1;
        assert_eq!(find_partner(text, 1, &C_LIKE), Some(close));
        assert_eq!(find_partner(text, nth(text, ")", 0), &C_LIKE), None);
        assert_eq!(find_partner(text, nth(text, "(", 1), &C_LIKE), None);
        // `#` のコメント（スクリプト）
        let text = "f(x # )\n)";
        assert_eq!(find_partner(text, 1, &SCRIPT), Some(text.len() - 1));
    }

    #[test]
    fn 日本語混じりでもバイト位置で対を返す() {
        // 全角の括弧（`（` `）`）は括弧として数えない
        let text = "表示（\"あ(い\"）(日本語[配列])";
        let open = nth(text, "(", 1);
        let close = nth(text, ")", 0);
        assert_eq!(find_partner(text, open, &C_LIKE), Some(close));
        assert_eq!(find_partner(text, close, &C_LIKE), Some(open));
        assert_eq!(
            find_partner(text, nth(text, "[", 0), &C_LIKE),
            Some(nth(text, "]", 0))
        );
        // 文字列の中（`"あ(い"`）の `(` は数えない
        assert_eq!(find_partner(text, nth(text, "(", 0), &C_LIKE), None);
        // 1 文字のリテラルが全角でも飛ばす
        let text = "f('あ', ')')";
        assert_eq!(find_partner(text, 1, &C_LIKE), Some(text.len() - 1));
    }

    // ─── 受け入れ条件 2: カーソルの採り方（3 ケース） ──────────────────────

    #[test]
    fn カーソルが括弧の直前なら直後の括弧を採る() {
        let text = "a (b) c";
        assert_eq!(
            match_at_cursor(text, 2, &C_LIKE),
            Some(BracketMatch {
                bracket: 2,
                partner: 4
            })
        );
    }

    #[test]
    fn カーソルが括弧の直後なら直前の括弧を採る() {
        let text = "a (b) c";
        assert_eq!(
            match_at_cursor(text, 5, &C_LIKE),
            Some(BracketMatch {
                bracket: 4,
                partner: 2
            })
        );
    }

    #[test]
    fn カーソルが括弧に接していなければ採らない() {
        let text = "a (b) c";
        for cursor in [0, 1, 6, 7] {
            assert_eq!(match_at_cursor(text, cursor, &C_LIKE), None, "{cursor}");
        }
        // 括弧の内側でも接していなければ採らない（Zed と違い、囲む組は出さない）
        assert_eq!(match_at_cursor("(abc)", 2, &C_LIKE), None);
    }

    #[test]
    fn 両側が括弧なら直後を優先し対が無ければ直前へ戻る() {
        // `)|(`: 右の `(` が勝つ
        let text = "(a)(b)";
        assert_eq!(
            match_at_cursor(text, 3, &C_LIKE),
            Some(BracketMatch {
                bracket: 3,
                partner: 5
            })
        );
        // 右の `(` に対が無ければ左の `)` を採る
        let text = "(a)(b";
        assert_eq!(
            match_at_cursor(text, 3, &C_LIKE),
            Some(BracketMatch {
                bracket: 2,
                partner: 0
            })
        );
        // `(|)`: どちらを採っても同じ組
        let found = match_at_cursor("()", 1, &C_LIKE).expect("対がある");
        assert_eq!(found.ordered(), [0, 1]);
        // `((|(`: 右の `(` が閉じなければ左の `(` の対
        let text = "((()";
        assert_eq!(
            match_at_cursor(text, 2, &C_LIKE),
            Some(BracketMatch {
                bracket: 2,
                partner: 3
            })
        );
    }

    #[test]
    fn カーソルの隣が文字列やコメントの中の括弧なら採らない() {
        let text = "f(\"(\") // (x)";
        assert_eq!(match_at_cursor(text, 3, &C_LIKE), None);
        assert_eq!(match_at_cursor(text, nth(text, "(x", 0), &C_LIKE), None);
    }

    // ─── 種類ごとに数える・言語の書き方 ────────────────────────────────

    #[test]
    fn 種類ごとに数えるので崩れた入れ子でも同じ種類が対になる() {
        // VS Code と同じ: `(` は `(` と `)` だけを数える
        let text = "( [ ) ]";
        assert_eq!(find_partner(text, 0, &C_LIKE), Some(4));
        assert_eq!(find_partner(text, 2, &C_LIKE), Some(6));
    }

    #[test]
    fn rust_のライフタイムと1文字のリテラルと生文字列() {
        let text =
            "fn f<'a>(x: &'a str, c: char) -> &'a str { if c == '(' { x } else { r#\"(\")\"# } }";
        assert_eq!(
            find_partner(text, nth(text, "(", 0), &C_LIKE),
            Some(nth(text, ")", 0))
        );
        assert_eq!(
            find_partner(text, nth(text, "{", 0), &C_LIKE),
            Some(text.len() - 1)
        );
        // エスケープした 1 文字のリテラル
        let text = r"m('\'', ')', '\u{28}')";
        assert_eq!(find_partner(text, 1, &C_LIKE), Some(text.len() - 1));
    }

    #[test]
    fn スクリプトの文字列とアポストロフィ() {
        // `'…'` の文字列・三重引用符・`don't` のアポストロフィ・f 文字列
        let text = "f('(', \"\"\"\n(\n\"\"\", don't, f'{x)}')";
        assert_eq!(find_partner(text, 1, &SCRIPT), Some(text.len() - 1));
        // 同じ行で閉じない `'` はアポストロフィ（中の括弧を数える）
        let text = "it's (fine)";
        assert_eq!(find_partner(text, 5, &SCRIPT), Some(10));
        // シェルの `${#arr[@]}` の `#` はコメントではない
        let text = "echo ${#arr[@]} # (x";
        assert_eq!(find_partner(text, 6, &SCRIPT), Some(14));
        assert_eq!(find_partner(text, 11, &SCRIPT), Some(13));
        // バッククォートの中
        let text = "f(`)`)";
        assert_eq!(find_partner(text, 1, &SCRIPT), Some(5));
    }

    #[test]
    fn エスケープした行コメントの記号はコメントを始めない() {
        let latex = BracketSyntax {
            line_comment: Some("%"),
            ..BracketSyntax::PLAIN
        };
        let text = "\\frac{50\\%}{x} % }";
        assert_eq!(find_partner(text, 5, &latex), Some(10));
        assert_eq!(find_partner(text, nth(text, "}", 2), &latex), None);
        // 英字の記号（`REM`）は行頭だけ
        let bat = BracketSyntax {
            line_comment: Some("REM"),
            ..BracketSyntax::PLAIN
        };
        let text = "echo (rem)\nrem )\n)";
        assert_eq!(find_partner(text, 5, &bat), Some(9));
        assert_eq!(find_partner(text, nth(text, ")", 1), &bat), None);
    }

    #[test]
    fn 文書は引用符を文字列として扱わない() {
        let text = "\"quote (a\" and b)";
        assert_eq!(
            find_partner(text, 7, &BracketSyntax::PLAIN),
            Some(text.len() - 1)
        );
        // コードなら引用符の中なので数えない
        assert_eq!(find_partner(text, 7, &C_LIKE), None);
    }

    #[test]
    fn 先頭と末尾の括弧_crlf_タブ混じり() {
        let text = "{\r\n\tf(a,\t'x')\r\n}";
        assert_eq!(
            match_at_cursor(text, 0, &C_LIKE).map(|m| m.partner),
            Some(text.len() - 1)
        );
        assert_eq!(
            match_at_cursor(text, text.len(), &C_LIKE).map(|m| m.partner),
            Some(0)
        );
        let open = nth(text, "(", 0);
        assert_eq!(find_partner(text, open, &C_LIKE), Some(nth(text, ")", 0)));
        // CRLF の行コメントも改行で終わる
        let text = "(// )\r\n)";
        assert_eq!(find_partner(text, 0, &C_LIKE), Some(text.len() - 1));
        // 空の本文
        assert_eq!(match_at_cursor("", 0, &C_LIKE), None);
    }

    #[test]
    fn 窓より離れた対は探さない() {
        // 巨大な 1 行: 対が窓の外なら None、窓の中なら見つかる
        let far = format!("({})", "x".repeat(SCAN_WINDOW + 10));
        assert_eq!(find_partner(&far, 0, &C_LIKE), None);
        let near = format!("({})", "x".repeat(SCAN_WINDOW - 10));
        assert_eq!(find_partner(&near, 0, &C_LIKE), Some(near.len() - 1));
        // 窓の起点が行の途中に来ても、手前の行頭から読む
        let text = format!("{}\n(a)", "y".repeat(SCAN_WINDOW + 5));
        let open = text.len() - 3;
        assert_eq!(find_partner(&text, open, &C_LIKE), Some(text.len() - 1));
    }

    // ─── 実ファイルの書き方（言語の同定の正本の `comment` 列 + `lexical` 列） ─────

    /// パスの書き方で `open`（`text` の中の位置）の対を探す
    fn partner_in(path: &str, text: &str, open: usize) -> Option<usize> {
        find_partner(text, open, &BracketSyntax::for_path(Path::new(path)))
    }

    #[test]
    fn 実ファイルの書き方は言語の同定の正本から引く() {
        // Rust: `//` と `/* */` と `"…"` を飛ばし、ライフタイムは文字列にしない
        let rs = "fn f<'a>(x: &'a str) { // )\n  /* } */ g(\")\"); }";
        assert_eq!(partner_in("a.rs", rs, nth(rs, "{", 0)), Some(rs.len() - 1));
        assert_eq!(
            partner_in("a.rs", rs, nth(rs, "(", 0)),
            Some(nth(rs, ")", 0))
        );
        // Python: `#` と `'…'`
        let py = "f('(', x # )\n)";
        assert_eq!(partner_in("a.py", py, 1), Some(py.len() - 1));
        // シェル: `${#a[@]}` の `#` はコメントではなく、バッククォートの中は飛ばす
        let sh = "echo ${#a[@]} `(` # (\n";
        assert_eq!(
            partner_in("a.sh", sh, nth(sh, "{", 0)),
            Some(nth(sh, "}", 0))
        );
        assert_eq!(partner_in("a.sh", sh, nth(sh, "(", 1)), None);
        // JS: テンプレート（バッククォート）と `'…'`
        let js = "f(`)`, ')', /* ) */ x)";
        assert_eq!(partner_in("a.ts", js, 1), Some(js.len() - 1));
        // Markdown: 引用符は文字列ではない・`<!-- -->` は飛ばす
        let md = "\"see (a\" <!-- ) --> b)";
        assert_eq!(partner_in("a.md", md, nth(md, "(", 0)), Some(md.len() - 1));
        // LaTeX: `%` は行コメント・`\%` は記号・`"` は文字列ではない
        let tex = "\\frac{50\\%}{\"x} % }";
        assert_eq!(partner_in("a.tex", tex, 5), Some(nth(tex, "}", 0)));
        assert_eq!(
            partner_in("a.tex", tex, nth(tex, "{", 1)),
            Some(nth(tex, "}", 1))
        );
        // 表に無い形式（`.json`）は既定 = `"…"` だけを飛ばす
        let json = r#"{"a": "}", "b": [1]}"#;
        assert_eq!(partner_in("a.json", json, 0), Some(json.len() - 1));
        // コメントの記号を持たない文書（`.txt`）は何も飛ばさない
        let txt = "it's \"(\" b)";
        assert_eq!(
            partner_in("a.txt", txt, nth(txt, "(", 0)),
            Some(txt.len() - 1)
        );
    }

    // ─── TextBuffer の口 ─────────────────────────────────────────────

    #[test]
    fn バッファは選択中は採らず同じ版とカーソルなら探し直さない() {
        let mut buffer = TextBuffer::from_text(PathBuf::from("a.rs"), "f(a)\n".into());
        buffer.set_cursor(1, false);
        let found = buffer.bracket_match().expect("`(` の直前");
        assert_eq!((found.bracket, found.partner), (1, 3));
        let searches = buffer.bracket_searches();
        for _ in 0..40 {
            assert_eq!(buffer.bracket_match(), Some(found));
        }
        assert_eq!(
            buffer.bracket_searches(),
            searches,
            "描画のたびに探し直している"
        );
        // カーソルが動けば探し直す（行末 = 括弧に接していない）
        buffer.set_cursor(5, false);
        assert_eq!(buffer.bracket_match(), None);
        assert_eq!(buffer.bracket_searches(), searches + 1);
        // 選択中は採らない
        buffer.set_selection(1, 4);
        assert_eq!(buffer.bracket_match(), None);
        // 編集で版が進めば探し直す（打った `)` の直後）
        buffer.set_cursor(4, false);
        buffer.insert(")");
        assert_eq!(buffer.bracket_match(), None, "対の無い `)`");
        buffer.set_cursor(0, false);
        buffer.insert("(");
        let found = buffer.bracket_match().expect("打った `(` の直後");
        assert_eq!((found.bracket, found.partner), (0, 5));
    }

    #[test]
    fn 応答の_bracket_match_は行と桁で無ければ_null() {
        let mut buffer =
            TextBuffer::from_text(PathBuf::from("a.rs"), "fn main() {\n    x();\n}\n".into());
        buffer.set_cursor(10, false);
        let state = buffer.document_state();
        assert_eq!(
            state["bracket_match"],
            serde_json::json!({
                "bracket": { "line": 1, "column": 10 },
                "match": { "line": 3, "column": 0 },
            })
        );
        buffer.set_cursor(3, false);
        assert!(buffer.document_state()["bracket_match"].is_null());
    }

    /// 1 打鍵あたりの探索の所要（#1691 の受け入れ条件 4。判定はせず出すだけ）。
    /// `TAKO_1691_PERF_FILE` があればそのファイル、無ければ tako-core の `text_edit.rs` を
    /// 5,000 行まで巡回して使う。
    /// `cargo test -p tako-core --release -- --ignored --nocapture perf_1691`
    #[test]
    #[ignore = "性能計測（--release で --ignored 指定のとき手動実行）"]
    fn perf_1691_対応括弧の探索() {
        use std::time::Instant;
        let source = match std::env::var("TAKO_1691_PERF_FILE") {
            Ok(path) => std::fs::read_to_string(path).expect("計測するファイル"),
            Err(_) => include_str!("../text_edit.rs").to_string(),
        };
        let mut lines: Vec<&str> = source.lines().collect();
        while lines.len() < 5_000 {
            let more: Vec<&str> = source.lines().collect();
            lines.extend(more);
        }
        lines.truncate(5_000);
        let text = lines.join("\n");
        let syntax = BracketSyntax::for_path(Path::new("bench.rs"));
        // 打鍵の位置: 先頭・中央・末尾の括弧の隣（末尾は先頭から全文を読む最悪）
        let brackets: Vec<usize> = text
            .bytes()
            .enumerate()
            .filter(|(_, b)| bracket_kind(*b).is_some())
            .map(|(i, _)| i)
            .collect();
        for (label, at) in [
            ("先頭", brackets[0]),
            ("中央", brackets[brackets.len() / 2]),
            ("末尾", *brackets.last().expect("括弧がある")),
        ] {
            let mut samples = Vec::new();
            let mut found = None;
            for _ in 0..200 {
                let t0 = Instant::now();
                found = match_at_cursor(&text, at, &syntax);
                samples.push(t0.elapsed());
            }
            samples.sort();
            eprintln!(
                "[perf] 1691 lines=5000 bytes={} at={label} median={:?} max={:?} found={found:?}",
                text.len(),
                samples[samples.len() / 2],
                samples[samples.len() - 1],
            );
        }
        // 実際の打鍵（バッファへ 1 文字入れて探す = GUI が 1 打鍵で払うぶん）
        let mut buffer = TextBuffer::from_text(PathBuf::from("bench.rs"), text.clone());
        let end = text.len();
        let mut samples = Vec::new();
        for i in 0..200 {
            buffer.set_cursor(end, false);
            let t0 = Instant::now();
            buffer.insert(if i % 2 == 0 { "(" } else { ")" });
            let _ = buffer.bracket_match();
            samples.push(t0.elapsed());
        }
        samples.sort();
        eprintln!(
            "[perf] 1691 keystroke(insert+match) at=末尾 median={:?} max={:?}",
            samples[samples.len() / 2],
            samples[samples.len() - 1]
        );
        // 10 MB の 1 行（窓の打ち切りが効く最悪）
        let huge = format!("({})", "x".repeat(10_000_000));
        let t0 = Instant::now();
        let found = match_at_cursor(&huge, 0, &syntax);
        eprintln!(
            "[perf] 1691 one-line bytes={} at=先頭 elapsed={:?} found={found:?}",
            huge.len(),
            t0.elapsed()
        );
    }
}
