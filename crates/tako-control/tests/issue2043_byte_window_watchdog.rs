//! **#2043 の番犬**: 番犬・テスト（`crates/*/tests/`）が、文字列を**固定のバイト幅**で
//! 切る添字を直書きしない。
//!
//! ## なぜ止めるのか
//!
//! 番犬は「目印の直後 400 バイト」「呼び出しの手前 1200 バイト」のように、目印の近傍を
//! 固定の幅で見る。これを `&src[at..at + 400]` と添字で書くと、幅の端が多バイト文字の途中に
//! 来た瞬間に `byte index N is not a char boundary` で panic する。番犬が読む製品コードは
//! 日本語のコメントと文言が多いので、**検査と無関係な PR が日本語を 1 行足しただけ**で
//! その PR の CI が赤くなる。実物（#2020 / PR #2037）: `agent_resume_watchdog` が
//! `if !has_children {` から 1200 バイトで切っていて、足したコメントで終端が 0x89 の途中に来た。
//! #2037 は同じ形の 2 本だけ直したので、#2043 で `crates/*/tests/` を洗い出し、12 か所を
//! `common/byte_window.rs` へ寄せた。
//!
//! ## 何を固定するか
//!
//! `crates/*/tests/` の `.rs`（`common/` を含む）の範囲添字 `[a..b]` と `.get(a..b)` のうち、
//! **幅が固定**の形を落とす。固定値 = 2 以上の整数リテラルか定数（`LOOKBACK`）。
//! 境界が識別子 1 つなら、同じ関数の手前の `let` を 1 段だけ遡って右辺で読む:
//!
//! - 前方: 終端が「起点 + 固定値」（`at..at + 400` / `at..(at + 400).min(len)` / `let end = …`）
//! - 後方: 始端が「終点 − 固定値」（`at.saturating_sub(1200)..at` / `at - 200..at`）
//! - 先頭: 先頭（か固定の位置）から固定値まで（`..80` / `..s.len().min(80)` / `17..23`）
//!
//! 通すもの:
//!
//! - `common/byte_window.rs` の `after` / `before` / `head`（端を外側へ文字境界まで広げる）
//! - 添字を `floor_char_boundary(` / `ceil_char_boundary(` で丸めてある
//! - **行の窓**: 受け手の宣言の型が `Vec` / スライス（`Vec<` / `&[` 等）・切り出しの直後が
//!   文字列には無いメソッド（`.iter()` / `.join(` 等）・起点が `enumerate()` / `position(` の
//!   添字のどれか（文字列ではないので文字境界が無い）
//! - 同じ行か 1 行上に `// 切り出し安全: <理由>`（#1746 と同じ目印。既知の固定リテラルなど）
//!
//! 手で丸める `while !s.is_char_boundary(end) { end -= 1 }` も落とす（`let mut end = idx + 1200`
//! を遡って前方の形と読む）。panic はしないが、#2037 の形は窓を**内側へ**縮める = 端にあった
//! 目印を黙って見なくなる向きなので、外側へ広げる 1 実装へ寄せる。
//!
//! 幅が変数（`fn window(s, at, width)` の `width`）の窓は固定幅ではないので見ない（既知の穴）。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙と形の判定が実ファイルで生きていること（行の窓を形として
//! 拾ったうえで除外している件数・補助の利用件数）を、[`修正前の形を実際に落とす`] で
//! #2043 で直した 12 か所の修正前の形が `file:line` で名指しされることを、
//! [`補助は端の多バイト文字で落ちず窓を外側へ広げる`] で修正前の式が panic し補助が通ることを
//! 確かめる（注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

// コメントと文字列を潰した眺め（バイト長と行番号が保たれる。説明文や注入の文字列を拾わない）
#[path = "common/code_view.rs"]
mod code_view;

// 範囲添字の検出は #1728 / #1746 の番犬と共有する 1 実装（#1757）。`.get(a..b)` も同じ部品で数える
#[path = "common/range_index.rs"]
mod range_index;

// 直す先の補助（ここでは補助自体の振る舞いを確かめる）
#[path = "common/byte_window.rs"]
mod byte_window;

use range_index::{get_ranges, has_reason, range_indexes, RangeIndex, REASON_MARK};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/tests/` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn test_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} が読める: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("ディレクトリ項目").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ が読める");
    for entry in crates {
        let tests = entry.expect("crate の項目").path().join("tests");
        if tests.is_dir() {
            walk(&tests, &mut files);
        }
    }
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();
    rels
}

// --------------------------------------------------------------- 字句

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Num(u64),
    Punct(char),
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 式を識別子・整数・記号へ分ける（眺めはコメントと文字列が潰れているので、それだけで足りる）
fn tokens(expr: &str) -> Vec<Tok> {
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
fn split_bounds(inner: &str) -> (&str, &str) {
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
    for (at, _) in before.rmatch_indices("let ") {
        if before[..at].chars().next_back().is_some_and(is_ident_char) {
            continue;
        }
        if before[at..].lines().skip(1).any(is_fn_head) {
            return None;
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

/// `name` の宣言を `line`（0 始まり）から上へ探す（同じ関数の中。定数とフィールドはファイル全体）
fn decl_of(lines: &[&str], line: usize, name: &str, field: bool) -> Option<Decl> {
    let target = ident(name);
    let whole_from = |k: usize| -> Vec<Tok> {
        // 文の終わり（`;` か行末の `{`）まで最大 8 行つなぐ
        let mut text = String::new();
        for l in lines.iter().skip(k).take(8) {
            text.push_str(l);
            text.push('\n');
            let t = l.trim_end();
            if t.ends_with(';') || t.ends_with('{') {
                break;
            }
        }
        tokens(&text)
    };
    // 定数・static・構造体のフィールドはファイルのどこにあってもよい
    for (k, l) in lines.iter().enumerate() {
        let toks = tokens(l);
        let item = toks.iter().position(|t| *t == target).is_some_and(|i| {
            toks.get(i + 1) == Some(&Tok::Punct(':'))
                && (matches!(toks.get(i.wrapping_sub(1)), Some(Tok::Ident(kw)) if kw == "const" || kw == "static")
                    || (field && (i == 0 || toks.get(i - 1) == Some(&ident("pub")))))
        });
        if item {
            let whole = whole_from(k);
            let at = whole.iter().position(|t| *t == target).unwrap_or(0);
            return Some(Decl {
                ty: until(&whole, at + 2, &['=', ';', ',']),
                whole,
            });
        }
    }
    if field {
        return None;
    }
    for (k, l) in lines.iter().enumerate().take(line + 1).rev() {
        let toks = tokens(l);
        for (i, _) in toks.iter().enumerate().filter(|(_, t)| **t == target) {
            let prev = i.checked_sub(1).and_then(|p| toks.get(p));
            let prev2 = i.checked_sub(2).and_then(|p| toks.get(p));
            let is_let = prev == Some(&ident("let"))
                || (prev == Some(&ident("mut")) && prev2 == Some(&ident("let")));
            if is_let {
                let whole = whole_from(k);
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
                    ty: until(&toks, i + 2, &[',', ')', '|']),
                    whole: toks,
                });
            }
            // 束縛の位置（`for <ここ> in` / `let <ここ> =` / `|<ここ>|`）に居る行
            if binds_at(&toks, i) {
                return Some(Decl {
                    ty: Vec::new(),
                    whole: toks,
                });
            }
        }
        if is_fn_head(l) {
            return None;
        }
    }
    None
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

/// 注釈の無い `let` の右辺が `Vec` を作っているか（`vec![…]` / `Vec::…` / `collect::<Vec<…>>()`）
fn builds_vec(whole: &[Tok]) -> bool {
    let rhs: Vec<Tok> = match whole.iter().position(|t| *t == Tok::Punct('=')) {
        Some(eq) => whole[eq + 1..].to_vec(),
        None => return false,
    };
    matches!(rhs.as_slice(), [Tok::Ident(v), Tok::Punct('!'), ..] if v == "vec")
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

/// 行の窓（文字列ではない）か: 受け手の宣言が `Vec` / スライス、か起点が添字（`enumerate` / `position`）
fn is_row_window(lines: &[&str], line: usize, receiver: &str, anchor: Option<&str>) -> bool {
    let mut segs = receiver.split('.');
    let (base, field) = match (segs.next(), segs.next()) {
        (Some("self"), Some(f)) => (f, true),
        (Some(b), _) => (b, false),
        _ => return false,
    };
    if !base.is_empty() {
        if let Some(d) = decl_of(lines, line, base, field) {
            if is_vec_type(&d.ty) || (d.ty.is_empty() && builds_vec(&d.whole)) {
                return true;
            }
        }
    }
    let Some(anchor) = anchor else {
        return false;
    };
    let anchor_base = anchor.split('.').next().unwrap_or(anchor);
    decl_of(lines, line, anchor_base, false).is_some_and(|d| {
        d.whole.iter().any(|t| {
            *t == ident("enumerate") || *t == ident("position") || *t == ident("rposition")
        })
    })
}

// --------------------------------------------------------------- 判定

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// 直書きの固定幅の窓（落とす）
    Offender,
    /// 丸めてある（`floor_char_boundary(` / `ceil_char_boundary(`）
    Rounded,
    /// 理由コメントがある
    Reason,
    /// 行の窓（`Vec` / スライス・起点が添字）
    RowWindow,
}

/// 固定幅の窓 1 つ
#[derive(Debug, Clone)]
struct Site {
    line: usize,
    shape: &'static str,
    text: String,
    verdict: Verdict,
}

/// 眺め `code` の固定幅の窓をすべて判定する（原文 `raw` は理由コメントと名指しの行に使う）
fn sites(raw: &str, code: &str) -> Vec<Site> {
    let raw_lines: Vec<&str> = raw.lines().collect();
    let code_lines: Vec<&str> = code.lines().collect();
    // (範囲, 受け手の終わり, 添字か)
    let mut found: Vec<(RangeIndex, usize, bool)> = Vec::new();
    for r in range_indexes(code) {
        let end = r.open;
        found.push((r, end, true));
    }
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
            ("前方", path_at(&tokens(lo_s), 0).map(|(p, _)| p))
        } else if is_backward(&lo, &tokens(hi_s)) {
            ("後方", path_at(&tokens(hi_s), 0).map(|(p, _)| p))
        } else if is_prefix(&tokens(lo_s), &hi) {
            ("先頭", None)
        } else {
            continue;
        };
        let rounded = r.is_rounded()
            || [&lo, &hi].iter().any(|b| {
                b.iter()
                    .any(|t| matches!(t, Tok::Ident(n) if n.ends_with("_char_boundary")))
            });
        let receiver = receiver_before(code, recv_end);
        let verdict = if rounded {
            Verdict::Rounded
        } else if has_reason(&raw_lines, r.line) {
            Verdict::Reason
        } else if (indexed && used_as_slice(code, r.open + 1 + r.inner.len()))
            || is_row_window(&code_lines, r.line - 1, &receiver, anchor.as_deref())
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

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。違反は `file:line — 理由`
fn offenders(raw: &str, rel: &str) -> Vec<String> {
    let code = code_view::code_view(raw);
    sites(raw, &code)
        .into_iter()
        .filter(|s| s.verdict == Verdict::Offender)
        .map(|s| {
            format!(
                "{rel}:{} — 文字列を固定のバイト幅で切っている（{}の窓: `{}`）。幅の端が多バイト文字の\
                 途中に来ると panic し、日本語を 1 行足しただけの無関係な PR の CI が赤くなる\
                 （#2020 / #2043）。`tests/common/byte_window.rs` の `after` / `before` / `head` を\
                 `#[path]` で取り込んで通す（端を外側へ文字境界まで広げる）。行の Vec なら宣言に型\
                 （`Vec<…>` / `&[…]`）を書く。既知の固定リテラル等で安全なら同じ行か 1 行上に\
                 `// {REASON_MARK} <理由>` を書く",
                s.line, s.shape, s.text
            )
        })
        .collect()
}

// --------------------------------------------------------------- テスト

#[test]
fn 番犬とテストに固定バイト幅の文字列の切り出しが無い() {
    let mut found = Vec::new();
    for rel in test_sources() {
        found.extend(offenders(&read(&rel), &rel));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていないこと（列挙が壊れて 0 件・形の判定が死んで 0 件で緑になるのを防ぐ）
#[test]
fn 走査が空振りしていない() {
    let files = test_sources();
    for must in [
        "crates/tako-control/tests/common/byte_window.rs",
        "crates/tako-control/tests/agent_resume_watchdog.rs",
        "crates/tako-core/tests/pty_exit_drain.rs",
        "crates/tako-cli/tests/issue1758_broken_pipe.rs",
    ] {
        assert!(
            files.iter().any(|f| f == must),
            "{must} が列挙に無い（crates/*/tests/ の列挙が壊れている）"
        );
    }
    assert!(files.len() >= 300, "列挙が少なすぎる: {} 件", files.len());

    let mut row = 0usize;
    let mut reason = 0usize;
    let mut helper_users = 0usize;
    for rel in &files {
        let raw = read(rel);
        let code = code_view::code_view(&raw);
        if code.contains("byte_window::") {
            helper_users += 1;
        }
        for s in sites(&raw, &code) {
            match s.verdict {
                Verdict::RowWindow => row += 1,
                Verdict::Reason => reason += 1,
                _ => {}
            }
        }
    }
    // 行の窓（`lines[i..(i + 30).min(lines.len())]` 等）は 2026-10-16 の実測で 33 件。
    // 形の判定が死ぬとここが 0 になる
    assert!(
        row >= 20,
        "行の窓を形として拾えていない（{row} 件）。形の判定が壊れている"
    );
    // 理由コメントで通した固定リテラル（issue1679 の 2 件・platform_parity の 1 件・この番犬の注入 4 件）
    assert!(reason >= 7, "理由コメントの窓が {reason} 件しか見えない");
    // #2043 で寄せた 10 ファイル（＋この番犬）
    assert!(
        helper_users >= 11,
        "byte_window の利用が {helper_users} ファイルしか見えない（寄せた先が戻された?）"
    );
}

#[test]
fn 修正前の形を実際に落とす() {
    // #2043 で直した 12 か所の修正前の形（行はほぼ原文のまま）。各 1 件を名指しすること
    let cases: &[(&str, &str, usize)] = &[
        (
            "① agent_resume（#2037 の手の丸め）",
            "fn a() {\n    let source = read(MAIN);\n    let idx = source.find(\"x\").unwrap();\n    let mut end = (idx + 1200).min(source.len());\n    while !source.is_char_boundary(end) {\n        end -= 1;\n    }\n    let block = &source[idx..end];\n}\n",
            8,
        ),
        (
            "② agy の後方 1200",
            "fn b() {\n    let src = production(\"src/dispatch.rs\");\n    let call = src.find(\"x\").unwrap();\n    let before = &src[call.saturating_sub(1200)..call];\n}\n",
            4,
        ),
        (
            "③ issue1857 の前方 400（フィールド）",
            "fn c() {\n    let mcp = Views::of(MCP_REQUEST, &read(MCP_REQUEST));\n    let persist = mcp.text.find(\"x\").unwrap();\n    let window = &mcp.text[persist..(persist + 400).min(mcp.text.len())];\n}\n",
            4,
        ),
        (
            "④ issue1554 の先頭 200",
            "fn d(main: &str) {\n    let call = main.split(\"x\").nth(1).unwrap_or_default();\n    assert!(call[..call.len().min(200)].contains(\"persist_diag(\"));\n}\n",
            3,
        ),
        (
            "⑤ issue763 の定数で遡る",
            "fn e(src: &str) {\n    const LOOKBACK: usize = 200;\n    let at = src.find(\"x\").unwrap();\n    let back = &src[at.saturating_sub(LOOKBACK)..at];\n}\n",
            4,
        ),
        (
            "⑥ remote_link の let を遡る前方 240",
            "fn f(production: String) {\n    let at = production.find(\"x\").unwrap();\n    let window_end = (at + 240).min(production.len());\n    let window = &production[at..window_end];\n}\n",
            4,
        ),
        (
            "⑦ remote_guide の見出し 80",
            "fn g(body: &str) {\n    for line in body.lines() {\n        println!(\"{}\", &line[..line.len().min(80)]);\n    }\n}\n",
            3,
        ),
        (
            "⑧ .get で取ると端が文字の途中の日に窓が空になる",
            "fn h(src: &str) {\n    let at = src.find(\"x\").unwrap();\n    let window = src.get(at..at + 400).unwrap_or(\"\");\n}\n",
            3,
        ),
        (
            "⑨ 後方の引き算",
            "fn i(src: &str) {\n    let at = src.find(\"x\").unwrap();\n    let back = &src[at - 200..at];\n}\n",
            3,
        ),
        (
            "⑩ 固定の位置（#1756 の形）",
            "fn j(hex: &str) {\n    let r = &hex[0..2];\n}\n",
            2,
        ),
    ];
    for (label, src, line) in cases {
        let found = offenders(src, "注入.rs");
        let want = format!("注入.rs:{line} —");
        assert!(
            found.iter().any(|f| f.starts_with(&want)),
            "{label} を名指しできなかった（`{want}` が無い）: {found:?}"
        );
        assert_eq!(found.len(), 1, "{label} 以外まで拾った: {found:?}");
    }

    // 安全な形・行の窓は通す（常に落ちる無意味な番犬になっていないこと）
    let ok = "
fn ok(src: &str, lines: &[&str], at: usize) {
    let idx = src.find(\"x\").unwrap();
    let a = byte_window::after(src, idx, 1200);
    let b = byte_window::before(src, idx, 700);
    let c = byte_window::head(src, 80);
    let d = &src[idx..src.ceil_char_boundary(idx + 400)];
    let rows: Vec<&str> = src.lines().collect();
    let e = rows[idx..(idx + 30).min(rows.len())].join(\"\\n\");
    let f = &lines[at.saturating_sub(5)..=at];
    for (i, l) in rows.iter().enumerate() {
        let g = &rows[i..(i + 3).min(rows.len())];
    }
    match fn_window(src) {
        Some((at, window)) => {
            let near = window[at..(at + 4).min(window.len())].iter().any(|l| l.is_empty());
        }
        None => {}
    }
    let rest = &src[idx + 3..];
    let mut j = idx + 2;
    let name = &src[idx + 2..j];
    let close = src.find(\"}\").unwrap();
    let h = &src[idx..close + 1];
    // 切り出し安全: 固定のリテラル行（位置そのものの検証）
    let k = &src[17..23];
    let s = \"&src[at..at + 400] は文字列の中なので数えない\";
    // &src[at..at + 400] は注釈の中なので数えない
}
";
    let found = offenders(ok, "注入.rs");
    assert!(found.is_empty(), "安全な形まで落とした: {found:?}");

    // 理由コメントは 2 行以上離すと効かない（そばに置く規約）
    let far = "fn far(src: &str) {\n    // 切り出し安全: 離れた理由\n    let x = 1;\n    let k = &src[17..23];\n}\n";
    let found = offenders(far, "注入.rs");
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:4 —")),
        "離れた理由コメントで通してしまった: {found:?}"
    );
}

/// 修正前の式は端の多バイト文字で panic し、補助は通って窓を**外側へ**広げる
#[test]
fn 補助は端の多バイト文字で落ちず窓を外側へ広げる() {
    // 前方: 「目」が 399..402 に跨がる（400 は文字の途中）
    let text = format!("{}目印", "a".repeat(399));
    let idx = 0usize;
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[idx..(idx + 400).min(text.len())].len());
    assert!(
        old.is_err(),
        "修正前の前方の式が panic しない（注入が効いていない）"
    );
    // #2037 の手の丸め（内側へ縮める）は「目」を窓から落とす
    let mut end = (idx + 400).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // 切り出し安全: #2037 の手の丸めをそのまま再現して、縮む向きを確かめる
    let shrunk = &text[idx..end];
    assert!(
        !shrunk.contains('目'),
        "内側へ丸めると端の文字が落ちる前提が崩れた"
    );
    let window = byte_window::after(&text, 0, 400);
    assert_eq!(window.len(), 402, "終端は文字境界まで後ろへ広がる");
    assert!(
        window.ends_with('目'),
        "端に跨がる文字を窓に含める: {window:?}"
    );

    // 後方: 「目」が 0..3 に跨がり、起点の 1200 バイト手前（2）は文字の途中
    let text = format!("目{}X", "a".repeat(1199));
    let at = text.find('X').unwrap();
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[at.saturating_sub(1200)..at].len());
    assert!(
        old.is_err(),
        "修正前の後方の式が panic しない（注入が効いていない）"
    );
    let window = byte_window::before(&text, at, 1200);
    assert!(window.starts_with('目'), "始端は文字境界まで前へ広がる");
    assert_eq!(window.len(), at);

    // 先頭: 4 バイト目は「い」の途中
    let text = "あいう";
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[..text.len().min(4)].len());
    assert!(
        old.is_err(),
        "修正前の先頭の式が panic しない（注入が効いていない）"
    );
    assert_eq!(byte_window::head(text, 4), "あい");

    // 境界: 幅が末尾 / 先頭を越えたら文字列の端で止まる・幅 0 は空
    assert_eq!(byte_window::after("abc", 1, 10_000), "bc");
    assert_eq!(byte_window::before("abc", 2, 10_000), "ab");
    assert_eq!(byte_window::after("abc", 3, 5), "");
    assert_eq!(byte_window::head("目", 0), "");
    assert_eq!(byte_window::after("a目b", 1, 1), "目");

    // エラー経路: 起点が文字の途中なら補助の名指しで落ちる（黙って丸めない）
    let err = std::panic::catch_unwind(|| byte_window::after("目", 1, 1).len())
        .expect_err("起点が文字の途中なら落ちる");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("#2043"), "補助の名指しで落ちていない: {msg}");
}
