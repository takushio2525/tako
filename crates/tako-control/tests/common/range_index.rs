//! 範囲添字（`[a..]` / `[..b]` / `[a..b]` / `[..=b]`）を見つけて、形と受け手で分類する
//! （番犬テストの共有部品。#1757 / #2043 / #1790）
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
//! `.get(a..b)` の範囲も [`get_ranges`] で数える。
//!
//! 後半の [`sites`] は、範囲添字を**形**（固定幅の窓・数値の境界・`.min(` の上限・`len()` から
//! 引く）と**受け手**（宣言の型が `Vec` / スライスなら文字列ではない）で判定する。
//! 番犬・テスト側の固定幅の窓（#2043 の `issue2043_byte_window_watchdog`）の判定をここへ移し、
//! 製品ファイル全体の番犬（#1790 の `issue1790_byte_index_repo_watchdog`）と同じ 1 実装で使う
//! （どちらの形を数えるかは [`Rule`] で選ぶ）。
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
    let mut lines = LineCursor::default();
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
            line: lines.line_of(code, open),
            inner: inner.to_string(),
        });
    }
    hits
}

/// `.get(a..b)` / `.get_mut(a..b)` の範囲の引数を出現順に返す（#2043）。
///
/// 返す [`RangeIndex::open`] は**開き括弧 `(` の位置**で、`inner` は括弧の中身。
/// 添字（[`range_indexes`]）と違って文字の途中でも panic せず `None` を返すが、
/// 番犬の窓を `.get(…).unwrap_or("")` で取ると、端が文字の途中に来た日に**窓が空になって
/// 番犬が黙って何も見なくなる**ので、固定幅の窓を見張る番犬は同じ規則で数える。
/// 中身の最上位に `..` があり `,` / `;` が無いものだけ（`..` 単独は数えない）
pub fn get_ranges(code: &str) -> Vec<RangeIndex> {
    let bytes = code.as_bytes();
    let mut hits = Vec::new();
    for head in [".get(", ".get_mut("] {
        let mut lines = LineCursor::default();
        for (at, _) in code.match_indices(head) {
            let open = at + head.len() - 1;
            let mut depth = 0i32;
            let mut close = None;
            for (i, &c) in bytes.iter().enumerate().skip(open) {
                match c {
                    b'[' | b'(' | b'{' => depth += 1,
                    b']' | b')' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(i);
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
            if !top.contains("..") || top.contains(',') || top.contains(';') || inner.trim() == ".."
            {
                continue;
            }
            hits.push(RangeIndex {
                open,
                line: lines.line_of(code, open),
                inner: inner.to_string(),
            });
        }
    }
    hits.sort_by_key(|r| r.open);
    hits
}

/// 前から順に増える位置の行番号を、前回の位置からの差分だけ数えて返す
/// （`main.rs` は 5 MB あるので、範囲添字ごとに先頭から数え直すと debug の番犬が遅くなる）
#[derive(Default)]
struct LineCursor {
    at: usize,
    line: usize,
}

impl LineCursor {
    fn line_of(&mut self, text: &str, at: usize) -> usize {
        if at < self.at {
            *self = Self::default();
        }
        self.line += text.as_bytes()[self.at..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        self.at = at;
        self.line + 1
    }
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

// =============================================================== 形と受け手の分類（#2043 → #1790）
//
// ここから下は #2043 の番犬（`issue2043_byte_window_watchdog`）が持っていた判定を、製品ファイルの
// 番犬（`issue1790_byte_index_repo_watchdog`）と共有するために移したもの（#1790）。
// 字面から型は読めないので、**危ない形**を境界の字句で選び、受け手の宣言の型・使い方から
// 文字列ではない（`Vec` / スライス）と読めるものを [`Verdict::RowWindow`] として通す。

/// どの形を危ないものとして数えるか（番犬ごとの軸）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// #2043: 番犬・テストの**固定幅の窓**（前方 / 後方 / 先頭）。`.get(a..b)` も数える
    /// （panic はしないが、端が文字の途中に来た日に窓が空になり、番犬が黙って見なくなる）
    FixedWindow,
    /// #1790: 製品コードの**危ない型**。固定幅の窓に加えて、境界が数値リテラルそのもの
    /// （`[1..]` / `[..5]`）・`.min(` を含む・`len()` から引く。`.get` は panic しないので数えない。
    /// 行の窓は起点の添字（`enumerate` / `position`）からは読まない（`s.bytes().enumerate()` の
    /// 位置で文字列を切る形まで通してしまうため。受け手の宣言の型と使い方だけで読む）
    Risky,
}

// --------------------------------------------------------------- 字句

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Num(u64),
    Punct(char),
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 式を識別子・整数・記号へ分ける（眺めはコメントと文字列が潰れているので、それだけで足りる）
pub fn tokens(expr: &str) -> Vec<Tok> {
    let chars: Vec<char> = expr.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let mut word = String::new();
            while i < chars.len() && is_ident_char(chars[i]) {
                word.push(chars[i]);
                i += 1;
            }
            // `1_200` / `200usize` は数字の部分だけを値にする（`0x1F` は 0 になるが幅には使わない）
            let digits: String = word
                .chars()
                .filter(|c| *c != '_')
                .take_while(|c| c.is_ascii_digit())
                .collect();
            out.push(Tok::Num(digits.parse().unwrap_or(0)));
        } else if is_ident_char(c) {
            let mut name = String::new();
            while i < chars.len() && is_ident_char(chars[i]) {
                name.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Ident(name));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

fn ident(name: &str) -> Tok {
    Tok::Ident(name.to_string())
}

/// 小文字か `_` で始まる名前（変数・フィールド。定数と型は起点にならない）
fn is_var(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
}

/// 定数の名前（`LOOKBACK` / `MAX_LEN`）
fn is_const(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// 固定値（2 以上の整数リテラルか定数）。`HEAD.len()` のような目印の長さ・関数呼び出しは除く
/// （目印の長さを足すのは、見つけた ASCII の目印を飛ばすだけで幅ではない）
fn is_fixed(toks: &[Tok], k: usize) -> bool {
    match toks.get(k) {
        Some(Tok::Num(n)) => *n >= 2,
        Some(Tok::Ident(name)) => {
            is_const(name) && !matches!(toks.get(k + 1), Some(Tok::Punct('.' | '(' | ':')))
        }
        _ => false,
    }
}

/// `k` から始まる「変数 . フィールド …」（メソッド呼び出しの手前まで）と、その次の位置。
/// 長いパスの途中（直前が `.`）からは始めない
fn path_at(toks: &[Tok], k: usize) -> Option<(String, usize)> {
    let Some(Tok::Ident(first)) = toks.get(k) else {
        return None;
    };
    if !is_var(first) || (k > 0 && toks[k - 1] == Tok::Punct('.')) {
        return None;
    }
    let mut path = first.clone();
    let mut i = k + 1;
    while let (Some(Tok::Punct('.')), Some(Tok::Ident(seg))) = (toks.get(i), toks.get(i + 1)) {
        if toks.get(i + 2) == Some(&Tok::Punct('(')) {
            break;
        }
        path.push('.');
        path.push_str(seg);
        i += 2;
    }
    Some((path, i))
}

/// 最上位の `..` / `..=` で始端と終端に分ける
pub fn split_bounds(inner: &str) -> (&str, &str) {
    let bytes = inner.as_bytes();
    let mut nest = 0i32;
    let mut i = 0;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => nest += 1,
            b')' | b']' | b'}' => nest -= 1,
            b'.' if nest == 0 && bytes[i + 1] == b'.' => {
                let hi = inner[i + 2..].strip_prefix('=').unwrap_or(&inner[i + 2..]);
                return (inner[..i].trim(), hi.trim());
            }
            _ => {}
        }
        i += 1;
    }
    (inner.trim(), "")
}

// --------------------------------------------------------------- 形

/// 前方の固定幅: 終端に「起点 + 固定値」（起点は始端の先頭のパス）
fn is_forward(lo: &[Tok], hi: &[Tok]) -> bool {
    let Some((anchor, _)) = path_at(lo, 0) else {
        return false;
    };
    (0..hi.len()).any(|k| {
        path_at(hi, k).is_some_and(|(p, next)| {
            p == anchor && hi.get(next) == Some(&Tok::Punct('+')) && is_fixed(hi, next + 1)
        })
    })
}

/// 後方の固定幅: 終端がパスそのもので、始端に「終端.saturating_sub(固定値)」か「終端 - 固定値」
fn is_backward(lo: &[Tok], hi: &[Tok]) -> bool {
    let Some((end, next)) = path_at(hi, 0) else {
        return false;
    };
    if next != hi.len() {
        return false;
    }
    (0..lo.len()).any(|k| {
        path_at(lo, k).is_some_and(|(p, n)| {
            p == end
                && ((lo.get(n) == Some(&Tok::Punct('.'))
                    && lo.get(n + 1) == Some(&ident("saturating_sub"))
                    && lo.get(n + 2) == Some(&Tok::Punct('('))
                    && is_fixed(lo, n + 3))
                    || (lo.get(n) == Some(&Tok::Punct('-')) && is_fixed(lo, n + 1)))
        })
    })
}

/// 先頭（か固定の位置）からの固定幅: 終端が固定値そのものか `len().min(固定値)` / `固定値.min(…)`
fn is_prefix(lo: &[Tok], hi: &[Tok]) -> bool {
    if !(lo.is_empty() || matches!(lo, [Tok::Num(_)])) {
        return false;
    }
    if hi.len() == 1 && is_fixed(hi, 0) {
        return true;
    }
    let lit_or_const = |k: usize| match hi.get(k) {
        Some(Tok::Num(n)) => *n >= 2,
        Some(Tok::Ident(name)) => is_const(name),
        _ => false,
    };
    (0..hi.len()).any(|k| {
        (hi.get(k) == Some(&ident("min"))
            && hi.get(k + 1) == Some(&Tok::Punct('('))
            && lit_or_const(k + 2)
            && hi.get(k + 3) == Some(&Tok::Punct(')')))
            || (lit_or_const(k)
                && hi.get(k + 1) == Some(&Tok::Punct('.'))
                && hi.get(k + 2) == Some(&ident("min")))
    })
}

/// #1790: 境界が数値リテラルそのもの（`[1..]` / `[..5]` / `[124..136]`）。
/// 0 は常に文字境界なので数えない（`[0..n]` は `[..n]` と同じ）
fn is_literal_bound(lo: &[Tok], hi: &[Tok]) -> bool {
    let lit = |b: &[Tok]| matches!(b, [Tok::Num(n)] if *n >= 1);
    lit(lo) || lit(hi)
}

/// #1790: 境界に `.min(` がある（上限で切り詰める形。`[..s.len().min(8)]` / `[..at.min(len)]`）
fn has_min(b: &[Tok]) -> bool {
    b.windows(3)
        .any(|w| w[0] == Tok::Punct('.') && w[1] == ident("min") && w[2] == Tok::Punct('('))
}

/// #1790: 境界が `len()` から引く（末尾から数える形。`[..s.len() - 1]` /
/// `[s.len().saturating_sub(20)..]`）
fn subtracts_from_len(b: &[Tok]) -> bool {
    b.windows(4).enumerate().any(|(k, w)| {
        w[0] == ident("len")
            && w[1] == Tok::Punct('(')
            && w[2] == Tok::Punct(')')
            && (w[3] == Tok::Punct('-')
                || (w[3] == Tok::Punct('.') && b.get(k + 4) == Some(&ident("saturating_sub"))))
    })
}

// --------------------------------------------------------------- 宣言を遡る

/// 行頭が関数の宣言か（`let` を遡るのは同じ関数の中だけ）
fn is_fn_head(line: &str) -> bool {
    let t = line.trim_start();
    [
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "async fn ",
    ]
    .iter()
    .any(|h| t.starts_with(h))
}

/// `site`（バイト位置）より手前で最後の `let [mut] 名前 [: 型] = 右辺;` の右辺。
/// 関数の宣言をまたいだ `let` は別の関数のものなので使わない
fn let_rhs(code: &str, site: usize, name: &str) -> Option<String> {
    let before = &code[..site];
    // 遡るのは手前で最後の関数の宣言の行まで（その行の `let` は数える）
    let mut fn_start = 0usize;
    let mut line_end = before.len();
    while let Some(nl) = before[..line_end].rfind('\n') {
        if is_fn_head(&before[nl + 1..line_end]) {
            fn_start = nl + 1;
            break;
        }
        line_end = nl;
    }
    for (rel, _) in before[fn_start..].rmatch_indices("let ") {
        let at = fn_start + rel;
        if before[..at].chars().next_back().is_some_and(is_ident_char) {
            continue;
        }
        let mut rest = before[at + "let ".len()..].trim_start();
        if let Some(r) = rest.strip_prefix("mut ") {
            rest = r.trim_start();
        }
        let Some(after_name) = rest.strip_prefix(name) else {
            continue;
        };
        if after_name.chars().next().is_some_and(is_ident_char) {
            continue;
        }
        // 型注釈を飛ばして `=`（`==` / `=>` ではない）まで。先に `;` なら代入の無い let
        let tail = &code[site - after_name.len()..];
        let bytes = tail.as_bytes();
        let mut depth = 0i32;
        let mut eq = None;
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth -= 1,
                b';' if depth <= 0 => return None,
                b'=' if depth <= 0
                    && bytes.get(i + 1) != Some(&b'=')
                    && bytes.get(i + 1) != Some(&b'>') =>
                {
                    eq = Some(i + 1);
                    break;
                }
                _ => {}
            }
        }
        let rhs = &tail[eq?..];
        let rbytes = rhs.as_bytes();
        let mut depth = 0i32;
        let end = rbytes
            .iter()
            .enumerate()
            .find_map(|(i, &b)| {
                match b {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => depth -= 1,
                    b';' if depth <= 0 => return Some(i),
                    _ => {}
                }
                None
            })
            .unwrap_or(rhs.len());
        return Some(rhs[..end].to_string());
    }
    None
}

/// 境界が変数 1 つなら、手前の `let` の右辺へ置き換える（1 段だけ）
fn resolve(code: &str, site: usize, bound: Vec<Tok>) -> Vec<Tok> {
    match bound.as_slice() {
        [Tok::Ident(name)] if is_var(name) => let_rhs(code, site, name)
            .map(|rhs| tokens(&rhs))
            .unwrap_or(bound),
        _ => bound,
    }
}

/// 宣言の手がかり（`ty` = 型として読める字面・`whole` = 宣言の行の全体）
struct Decl {
    ty: Vec<Tok>,
    whole: Vec<Tok>,
}

/// `toks` の `from` から、深さ 0 の `stops` のどれかの手前まで
fn until(toks: &[Tok], from: usize, stops: &[char]) -> Vec<Tok> {
    let mut depth = 0i32;
    let mut out = Vec::new();
    for t in toks.iter().skip(from) {
        if let Tok::Punct(c) = t {
            if depth == 0 && stops.contains(c) {
                break;
            }
            match c {
                '(' | '[' | '{' | '<' => depth += 1,
                ')' | ']' | '}' | '>' => depth -= 1,
                _ => {}
            }
        }
        out.push(t.clone());
    }
    out
}

/// 1 ファイルぶんの下ごしらえ（行と、ファイルのどこにあってもよい宣言の行）。
/// `main.rs` は 10 万行あるので、ファイル全体を字句へ分けない（行の字句は使う行だけ遅延で作る）
struct FileIndex<'a> {
    lines: Vec<&'a str>,
    toks: Vec<std::cell::OnceCell<Vec<Tok>>>,
    /// 名前 → 最初の `const` / `static` の宣言行（0 始まり）
    items: std::collections::HashMap<String, usize>,
    /// 名前 → 最初の構造体のフィールドらしい行（行頭か `pub` の直後の `名前:`）
    fields: std::collections::HashMap<String, usize>,
}

impl<'a> FileIndex<'a> {
    fn new(code: &'a str) -> Self {
        let lines: Vec<&str> = code.lines().collect();
        let mut index = FileIndex {
            toks: (0..lines.len())
                .map(|_| std::cell::OnceCell::new())
                .collect(),
            lines,
            items: std::collections::HashMap::new(),
            fields: std::collections::HashMap::new(),
        };
        for k in 0..index.lines.len() {
            let l = index.lines[k];
            // 宣言の目印は名前の直後の `:`
            if !l.contains(':') {
                continue;
            }
            if !(l.contains("pub") || l.contains("const") || l.contains("static")) {
                // 行頭の名前だけを見ればよい行は字句へ分けない（`名前 :` の形か）
                let t = l.trim_start();
                if !t.starts_with(|c: char| is_ident_char(c) && !c.is_ascii_digit()) {
                    continue;
                }
                let name: String = t.chars().take_while(|c| is_ident_char(*c)).collect();
                if t[name.len()..].trim_start().starts_with(':') {
                    index.fields.entry(name).or_insert(k);
                }
                continue;
            }
            // 行の中で名前ごとに最初の出現だけを見る
            let line = index.tok(k);
            let mut seen = std::collections::HashSet::new();
            let mut items = Vec::new();
            let mut fields = Vec::new();
            for (i, t) in line.iter().enumerate() {
                let Tok::Ident(name) = t else { continue };
                if !seen.insert(name.as_str()) || line.get(i + 1) != Some(&Tok::Punct(':')) {
                    continue;
                }
                let prev = i.checked_sub(1).and_then(|p| line.get(p));
                if matches!(prev, Some(Tok::Ident(kw)) if kw == "const" || kw == "static") {
                    items.push(name.clone());
                }
                if i == 0 || prev == Some(&ident("pub")) {
                    fields.push(name.clone());
                }
            }
            for name in items {
                index.items.entry(name).or_insert(k);
            }
            for name in fields {
                index.fields.entry(name).or_insert(k);
            }
        }
        index
    }

    /// `k` 行目の字句（初めて使うときに分ける）
    fn tok(&self, k: usize) -> &Vec<Tok> {
        self.toks[k].get_or_init(|| tokens(self.lines[k]))
    }

    /// `k` 行目から文の終わり（`;` か行末の `{`）まで最大 8 行つないだ字句
    fn whole_from(&self, k: usize) -> Vec<Tok> {
        let mut text = String::new();
        for l in self.lines.iter().skip(k).take(self.stmt_end(k) + 1 - k) {
            text.push_str(l);
            text.push('\n');
        }
        tokens(&text)
    }

    /// `k` 行目から始まる文の最後の行（`;` か行末の `{` で終わる行。最大 8 行）
    fn stmt_end(&self, k: usize) -> usize {
        let last = (k + 7).min(self.lines.len().saturating_sub(1));
        (k..=last)
            .find(|&j| {
                let t = self.lines[j].trim_end();
                t.ends_with(';') || t.ends_with('{')
            })
            .unwrap_or(last)
    }

    /// `name` の宣言を `line`（0 始まり）から上へ探す（同じ関数の中。定数とフィールドはファイル全体）
    fn decl_of(&self, line: usize, name: &str, field: bool) -> Option<Decl> {
        let target = ident(name);
        // 定数・static・構造体のフィールドはファイルのどこにあってもよい
        let item = self.items.get(name).copied();
        let item = if field {
            match (item, self.fields.get(name).copied()) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        } else {
            item
        };
        if let Some(k) = item {
            let whole = self.whole_from(k);
            let at = whole.iter().position(|t| *t == target).unwrap_or(0);
            return Some(Decl {
                ty: until(&whole, at + 2, &['=', ';', ',']),
                whole,
            });
        }
        if field || self.lines.is_empty() {
            return None;
        }
        for k in (0..=line.min(self.lines.len() - 1)).rev() {
            let toks = self.tok(k);
            for (i, _) in toks.iter().enumerate().filter(|(_, t)| **t == target) {
                let prev = i.checked_sub(1).and_then(|p| toks.get(p));
                let prev2 = i.checked_sub(2).and_then(|p| toks.get(p));
                let is_let = prev == Some(&ident("let"))
                    || (prev == Some(&ident("mut")) && prev2 == Some(&ident("let")));
                // `let anchor = &anchor[..];` の右辺の `anchor` は手前の束縛を指す（#1790）。
                // 切り出しの行がこの `let` の文の中なら、この宣言は飛ばして上を探す
                if is_let && line <= self.stmt_end(k) {
                    continue;
                }
                if is_let {
                    let whole = self.whole_from(k);
                    let at = whole.iter().position(|t| *t == target).unwrap_or(0);
                    let ty = if whole.get(at + 1) == Some(&Tok::Punct(':')) {
                        until(&whole, at + 2, &['=', ';'])
                    } else {
                        Vec::new()
                    };
                    return Some(Decl { ty, whole });
                }
                // 関数・クロージャの引数（`name: 型`。rustfmt が折った複数行の引数は行頭に来る）。
                // 型は深さ 0 の `,` / `)` / `|` まで
                let is_param = toks.get(i + 1) == Some(&Tok::Punct(':'))
                    && toks.get(i + 2) != Some(&Tok::Punct(':'))
                    && (prev.is_none()
                        || matches!(prev, Some(Tok::Punct('(' | ',' | '|')))
                        || prev == Some(&ident("mut")));
                if is_param {
                    return Some(Decl {
                        ty: until(toks, i + 2, &[',', ')', '|']),
                        whole: toks.clone(),
                    });
                }
                // 束縛の位置（`for <ここ> in` / `let <ここ> =` / `|<ここ>|`）に居る行
                if binds_at(toks, i) {
                    return Some(Decl {
                        ty: Vec::new(),
                        whole: toks.clone(),
                    });
                }
            }
            if is_fn_head(self.lines[k]) {
                return None;
            }
        }
        None
    }

    /// 行の窓（文字列ではない）か: 受け手の宣言が `Vec` / スライス、か起点が添字
    /// （`enumerate` / `position`。`anchor` を渡したときだけ）
    fn is_row_window(&self, line: usize, receiver: &str, anchor: Option<&str>) -> bool {
        let mut segs = receiver.split('.');
        let (base, field) = match (segs.next(), segs.next()) {
            (Some("self"), Some(f)) => (f, true),
            (Some(b), _) => (b, false),
            _ => return false,
        };
        if !base.is_empty() {
            if let Some(d) = self.decl_of(line, base, field) {
                if is_vec_type(&d.ty) || (d.ty.is_empty() && builds_vec(&d.whole)) {
                    return true;
                }
            }
        }
        let Some(anchor) = anchor else {
            return false;
        };
        let anchor_base = anchor.split('.').next().unwrap_or(anchor);
        self.decl_of(line, anchor_base, false).is_some_and(|d| {
            d.whole.iter().any(|t| {
                *t == ident("enumerate") || *t == ident("position") || *t == ident("rposition")
            })
        })
    }
}

/// `toks[i]` が束縛のパターンの中か（`for (i, l) in` / `if let Some(x) =` / `|(_, l)|`）
fn binds_at(toks: &[Tok], i: usize) -> bool {
    let before = &toks[..i];
    let after = &toks[i + 1..];
    let last = |kw: &str| before.iter().rposition(|t| *t == ident(kw));
    if let Some(f) = last("for") {
        if !before[f..].contains(&ident("in")) && after.contains(&ident("in")) {
            return true;
        }
    }
    if let Some(l) = last("let") {
        if !before[l..].contains(&Tok::Punct('=')) && after.contains(&Tok::Punct('=')) {
            return true;
        }
    }
    let bars_before = before.iter().filter(|t| **t == Tok::Punct('|')).count();
    bars_before % 2 == 1 && after.contains(&Tok::Punct('|'))
}

/// 型の字面が `Vec` / スライス / 配列か
fn is_vec_type(ty: &[Tok]) -> bool {
    ty.iter()
        .any(|t| *t == ident("Vec") || *t == ident("VecDeque"))
        || ty
            .windows(2)
            .any(|w| w == [Tok::Punct('&'), Tok::Punct('[')])
        || ty
            .windows(3)
            .any(|w| w[0] == Tok::Punct('&') && w[1] == ident("mut") && w[2] == Tok::Punct('['))
        || ty
            .windows(4)
            .any(|w| w[0] == Tok::Punct('&') && w[1] == Tok::Punct('\'') && w[3] == Tok::Punct('['))
        || ty.first() == Some(&Tok::Punct('['))
}

/// 注釈の無い `let` の右辺が文字列ではないものを作っているか: `Vec`（`vec![…]` / `Vec::…` /
/// `collect::<Vec<…>>()` / `….to_vec()`）・バイト列（`….as_bytes()` / `….into_bytes()`）・
/// 配列（`[0u8; 512]`）
fn builds_vec(whole: &[Tok]) -> bool {
    let mut rhs: Vec<Tok> = match whole.iter().position(|t| *t == Tok::Punct('=')) {
        Some(eq) => whole[eq + 1..].to_vec(),
        None => return false,
    };
    // 末尾で読む形（`….as_bytes()` / `[…]`）は、文が `;` で閉じているときだけ
    // （`if let Some(x) = ["a", "b"]` の次の行に `.iter().find_map(…)` が続く形を配列と読まない）
    let closed = rhs.last() == Some(&Tok::Punct(';'));
    if closed {
        rhs.pop();
    }
    let ends_with_call = |name: &str| {
        rhs.ends_with(&[
            Tok::Punct('.'),
            ident(name),
            Tok::Punct('('),
            Tok::Punct(')'),
        ])
    };
    (closed
        && (["to_vec", "as_bytes", "into_bytes", "as_bytes_mut"]
            .iter()
            .any(|m| ends_with_call(m))
            || (rhs.first() == Some(&Tok::Punct('[')) && rhs.last() == Some(&Tok::Punct(']')))))
        || matches!(rhs.as_slice(), [Tok::Ident(v), Tok::Punct('!'), ..] if v == "vec")
        || matches!(rhs.as_slice(), [Tok::Ident(v), Tok::Punct(':'), ..] if v == "Vec")
        || rhs.windows(5).any(|w| {
            w[0] == ident("collect")
                && w[1] == Tok::Punct(':')
                && w[2] == Tok::Punct(':')
                && w[3] == Tok::Punct('<')
                && w[4] == ident("Vec")
        })
}

/// 切り出しの直後に、文字列には無いスライスのメソッド（`.iter()` / `.join(` 等）が続くか。
/// `match … { Some((at, window)) => window[i..(i + 4).min(window.len())].iter() … }` のように
/// 宣言から型が読めない行の窓も、使い方で `Vec` / スライスと分かる
fn used_as_slice(code: &str, close: usize) -> bool {
    const SLICE_ONLY: [&str; 10] = [
        "iter",
        "iter_mut",
        "into_iter",
        "join",
        "concat",
        "to_vec",
        "windows",
        "chunks",
        "first",
        "last",
    ];
    let rest = code.get(close + 1..).unwrap_or("").trim_start();
    let Some(rest) = rest.strip_prefix('.') else {
        return false;
    };
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|c| is_ident_char(*c))
        .collect();
    SLICE_ONLY.contains(&name.as_str())
}

/// 受け手（`[` / `.get(` の直前の `a.b.c`）。呼び出しの結果（`f()[..]`）は空
fn receiver_before(code: &str, end: usize) -> String {
    let head = &code[..end];
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident_char(*c) || *c == '.')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(end);
    head[start..].trim_start_matches('.').to_string()
}

// --------------------------------------------------------------- 判定

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 理由も丸めも無い危ない形（番犬が落とす）
    Offender,
    /// 丸めてある（`floor_char_boundary(` / `ceil_char_boundary(`）
    Rounded,
    /// 理由コメントがある
    Reason,
    /// 行の窓（`Vec` / スライス・起点が添字）か、`as_bytes()` のバイト列
    RowWindow,
}

/// 危ない形の範囲 1 つ
#[derive(Debug, Clone)]
pub struct Site {
    /// 1 始まりの行番号
    pub line: usize,
    /// 形の名前（失敗の見出しに出す）
    pub shape: &'static str,
    /// 原文の行（前後の空白を落とす）
    pub text: String,
    pub verdict: Verdict,
}

/// 眺め `code` の危ない形の範囲をすべて判定する（原文 `raw` は理由コメントと名指しの行に使う）。
/// 数える形は `rule` で選ぶ（[`Rule`]）。並びは行の順
pub fn sites(raw: &str, code: &str, rule: Rule) -> Vec<Site> {
    let raw_lines: Vec<&str> = raw.lines().collect();
    let file = FileIndex::new(code);
    // (範囲, 受け手の終わり, 添字か)
    let mut found: Vec<(RangeIndex, usize, bool)> = Vec::new();
    for r in range_indexes(code) {
        let end = r.open;
        found.push((r, end, true));
    }
    if rule == Rule::FixedWindow {
        for r in get_ranges(code) {
            // `(` の手前の `.get` / `.get_mut` を外した位置が受け手の終わり
            let head = &code[..r.open];
            let end = head
                .strip_suffix(".get_mut")
                .or_else(|| head.strip_suffix(".get"))
                .map(str::len)
                .unwrap_or(r.open);
            found.push((r, end, false));
        }
    }
    let mut out = Vec::new();
    for (r, recv_end, indexed) in found {
        let (lo_s, hi_s) = split_bounds(&r.inner);
        let lo = resolve(code, r.open, tokens(lo_s));
        let hi = resolve(code, r.open, tokens(hi_s));
        // 起点から始まる走査カーソル（`let mut j = i + 2; … [i + 2..j]`）は幅が固定ではない
        if !lo.is_empty() && lo == hi {
            continue;
        }
        let (shape, anchor) = if is_forward(&tokens(lo_s), &hi) {
            ("前方の固定幅", path_at(&tokens(lo_s), 0).map(|(p, _)| p))
        } else if is_backward(&lo, &tokens(hi_s)) {
            ("後方の固定幅", path_at(&tokens(hi_s), 0).map(|(p, _)| p))
        } else if is_prefix(&tokens(lo_s), &hi) {
            ("先頭からの固定幅", None)
        } else if rule == Rule::Risky && is_literal_bound(&lo, &hi) {
            ("数値の境界", None)
        } else if rule == Rule::Risky && (has_min(&lo) || has_min(&hi)) {
            ("`.min(` の上限", None)
        } else if rule == Rule::Risky && (subtracts_from_len(&lo) || subtracts_from_len(&hi)) {
            ("`len()` から引く", None)
        } else {
            continue;
        };
        let rounded = r.is_rounded()
            || [&lo, &hi].iter().any(|b| {
                b.iter()
                    .any(|t| matches!(t, Tok::Ident(n) if n.ends_with("_char_boundary")))
            });
        let receiver = receiver_before(code, recv_end);
        // 起点の添字で行の窓と読むのはテストの窓だけ（[`Rule::Risky`] の説明）
        let anchor = if rule == Rule::FixedWindow {
            anchor
        } else {
            None
        };
        let bytes = {
            let head = code[..recv_end].trim_end();
            head.ends_with("as_bytes()") || head.ends_with("as_bytes_mut()")
        };
        let verdict = if rounded {
            Verdict::Rounded
        } else if has_reason(&raw_lines, r.line) {
            Verdict::Reason
        } else if bytes
            || (indexed && used_as_slice(code, r.open + 1 + r.inner.len()))
            || file.is_row_window(r.line - 1, &receiver, anchor.as_deref())
        {
            Verdict::RowWindow
        } else {
            Verdict::Offender
        };
        out.push(Site {
            line: r.line,
            shape,
            text: raw_lines
                .get(r.line - 1)
                .map(|l| l.trim().to_string())
                .unwrap_or_default(),
            verdict,
        });
    }
    out.sort_by_key(|s| s.line);
    out
}
