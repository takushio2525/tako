//! 構文の塗りの正規表現へ「当たりようのない行では VM を起こさない」前置きを足す（#1917 / #1935）
//!
//! **ビルド時に走る**: `build.rs` が `#[path]` でこのファイルを読み、対象の構文
//! （[`crate::syntax_mirror::TARGETS`]）の文脈を書き換えて `OUT_DIR` へ書く。
//! 起動時は書き換えずに差し込むだけ（[`crate::syntax_prefilter`]）。アプリ本体では単体テストだけが
//! このモジュールを使う（等価性の検査と、生成物が今のコードで作り直したものと一致することの確認）。
//! このファイルは `crate::syntax_mirror` 以外のモジュールを参照しない。
//!
//! ## 真因（release 実測・1 MB の TS）
//!
//! TS の構文定義（two-face = bat 由来、元は VS Code の TextMate 文法）は先読み・後読みだらけで、
//! fancy-regex はそういう正規表現（hard）を「先頭に `.*?` の輪を付けた VM」へコンパイルする。
//! 当たらない行では**行の全バイト位置で VM が走る**。syntect は 1 行ごとに文脈の正規表現を
//! それぞれ 1 回ずつ探すので、1 MB（27,747 行）で約 310 万回の探索 = 正規表現だけで 5.9 秒、
//! 365 本にほぼ均等に散っていた（1 位でも 8.6%）。どれも「1 行に 1 回呼ばれ、ほぼ当たらない」形。
//! ほかの言語も同じ形で、1 MB の JavaScript は 3.8 秒・C++ は 3.0 秒かかっていた（#1935）。
//!
//! ## 書き換え
//!
//! 正規表現 `R` を次の等価形へ置き換える（`G` は「`R` が位置 p で一致するなら、
//! 必ず p で一致する」先読み・後読みを含まない正規表現 = 必要条件）:
//!
//! ```text
//! \G(?=(?s:.)*?(?:G))(?s:.)*?\K(?:R)
//! ```
//!
//! - `\G` は探索の開始位置でだけ成り立つ（fancy-regex の VM は開始位置より先では即失敗）。
//! - `(?=(?s:.)*?(?:G))` は易しい正規表現なので regex-automata（DFA）が 1 回の走査で調べる。
//!   行のどこにも `G` が無ければここで終わる = **当たらない行では VM を回さない**。
//! - 候補があれば `(?s:.)*?` が開始位置から 1 文字ずつ進めて `R` を試す。これは fancy-regex が
//!   元から付けている `.*?` の輪と同じ順序なので、**最左の一致・各グループの位置が元と同じ**。
//!   `\K` が一致の始点（group 0）を `R` の始点へ置き直す。
//! - 前置きは捕獲グループを持たないので、`R` のグループ番号はずれない（検証で確かめる）。
//!
//! `G` は `R` から「先読み・後読み・単語境界を外したもの」「肯定先読みの中身（途中の先読みなら
//! そこまでの前半 + 中身）」「選択の枝ごとの最良を束ねたもの」を候補にし、最短一致長が最も長い
//! ものを選ぶ。文字クラスは「ASCII の部分 + 非 ASCII の全体」へ広げる（オートマトンを小さくする）。
//! どれも `R` の上位集合なので、`G` が見つからない行で `R` が一致することは無い。後方参照・`\G`・
//! `\K`・条件分岐・サブルーチンを含む正規表現と、fancy-regex が元から regex-automata へ任せる
//! 易しい正規表現は書き換えない。

use std::collections::HashMap;

use fancy_regex::{Assertion, Expr, LookAround};
use syntect::parsing::syntax_definition::Pattern;
use syntect::parsing::{Regex, SyntaxSet};

use crate::syntax_mirror::{Fingerprint, LazyMirror, PrefilteredSyntax, SetMirror};

/// fancy-regex のパースフラグ。syntect は `RegexBuilder::oniguruma_mode(true)` で組み、
/// パーサは常に Unicode を足すので、oniguruma モード（`\<` `\>` を文字として読む）だけ立てる。
/// 0.16 の `FLAG_ONIGURUMA_MODE` は公開されていないので値で持つ（単体テストが意味を確かめる）
pub(crate) const PARSE_FLAGS: u32 = 1 << 6;

/// `targets` の構文の文脈を書き換えて、`Vec<PrefilteredSyntax>` の bincode にする（`build.rs` が呼ぶ）。
/// 並びは `targets` の順・中身は入力だけで決まる（同じ two-face と同じこのファイルなら同じバイト列）
pub(crate) fn generate(base: &SyntaxSet, targets: &[&str]) -> Result<Vec<u8>, String> {
    let original =
        bincode::serialize(base).map_err(|e| format!("構文セットを直列化できない: {e}"))?;
    let set: SetMirror =
        bincode::deserialize(&original).map_err(|e| format!("構文セットの形が想定と違う: {e}"))?;
    // 鏡の並びが syntect と食い違っていれば、往復で元のバイト列に戻らない
    if bincode::serialize(&set).ok().as_deref() != Some(&original[..]) {
        return Err("構文セットの直列化が往復で元に戻らない（syntect の形が変わった）".into());
    }
    // TSX の正規表現はほぼ TS と同じ文字列なので、書き換えの結果を使い回す
    let mut memo: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::with_capacity(targets.len());
    for name in targets {
        let syntax = set
            .syntaxes
            .iter()
            .find(|syntax| syntax.name == *name)
            .ok_or_else(|| format!("構文セットに {name} が無い"))?;
        let one = rewrite_syntax(name, &syntax.serialized_lazy_contexts, &mut memo)?;
        out.push(PrefilteredSyntax {
            name: name.to_string(),
            source: Fingerprint::of(&syntax.serialized_lazy_contexts),
            contexts: one.contexts,
            patterns: one.patterns as u32,
            rewritten: one.rewritten as u32,
        });
    }
    bincode::serialize(&out).map_err(|e| format!("書き換えた文脈を直列化できない: {e}"))
}

/// 1 構文の文脈（`serialized_lazy_contexts`）を書き換えた結果
pub(crate) struct RewrittenSyntax {
    pub(crate) contexts: Vec<u8>,
    pub(crate) patterns: usize,
    pub(crate) rewritten: usize,
}

/// 1 構文の文脈を解き、正規表現を書き換えて詰め直す
pub(crate) fn rewrite_syntax(
    name: &str,
    serialized: &[u8],
    memo: &mut HashMap<String, Option<String>>,
) -> Result<RewrittenSyntax, String> {
    let mut lazy: LazyMirror = syntect::dumps::from_reader(serialized)
        .map_err(|e| format!("{name} の文脈を読めない: {e}"))?;
    let (mut patterns, mut rewritten) = (0, 0);
    for context in &mut lazy.contexts {
        for pattern in &mut context.patterns {
            let Pattern::Match(matcher) = pattern else {
                continue;
            };
            patterns += 1;
            let original = matcher.regex.regex_str();
            let guarded = memo
                .entry(original.to_string())
                .or_insert_with(|| rewrite(original))
                .clone();
            if let Some(guarded) = guarded {
                matcher.regex = Regex::new(guarded);
                rewritten += 1;
            }
        }
    }
    Ok(RewrittenSyntax {
        contexts: compress(&lazy)?,
        patterns,
        rewritten,
    })
}

/// 文脈を syntect と同じ形（zlib で圧縮した bincode）へ詰め直す
/// （読む側の `from_reader` は zlib なら圧縮の強さを問わない）
fn compress(lazy: &LazyMirror) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    bincode::serialize_into(&mut encoder, lazy)
        .map_err(|e| format!("文脈を直列化できない: {e}"))?;
    encoder
        .finish()
        .map_err(|e| format!("文脈を圧縮できない: {e}"))
}

/// `re` を「当たりようのない行では VM を起こさない」等価形へ書き換える。
/// 書き換えない（できない・しても速くならない）ときは `None`
pub(crate) fn rewrite(re: &str) -> Option<String> {
    let tree = Expr::parse_tree_with_flags(re, PARSE_FLAGS).ok()?;
    if !tree.backrefs.is_empty() || has_unsupported(&tree.expr) || !runs_on_vm(&tree.expr) {
        return None;
    }
    let guard = best_guard(&tree.expr)?;
    // `(?x)` の正規表現が `#` のコメントで終わっていると、閉じ括弧がコメントに飲まれる。
    // そのときだけ改行を挟む（x モードの改行は読み飛ばされる）。どちらも構造の検証を通す
    ["", "\n"].into_iter().find_map(|tail| {
        let candidate = format!("\\G(?=(?s:.)*?(?:{guard}))(?s:.)*?\\K(?:{re}{tail})");
        verified(&candidate, &tree.expr).then_some(candidate)
    })
}

/// 書き換えた正規表現が「前置き + 元の `R` そのもの」として読めるか。
/// 前置きに捕獲グループが無い（= `R` のグループ番号がずれない）ことも確かめる
fn verified(candidate: &str, original: &Expr) -> bool {
    let Ok(tree) = Expr::parse_tree_with_flags(candidate, PARSE_FLAGS) else {
        return false;
    };
    let Expr::Concat(items) = &tree.expr else {
        return false;
    };
    let [start, Expr::LookAround(guard, LookAround::LookAhead), skip, keep, body] = &items[..]
    else {
        return false;
    };
    matches!(start, Expr::ContinueFromPreviousMatchEnd)
        && !has_group(guard)
        && matches!(
            skip,
            Expr::Repeat { child, lo: 0, hi: usize::MAX, greedy: false }
                if matches!(**child, Expr::Any { newline: true })
        )
        && matches!(keep, Expr::KeepOut)
        && body == original
}

/// 書き換えの前提が崩れる要素（後方参照・`\G`・`\K`・条件分岐・サブルーチン）を含むか
fn has_unsupported(expr: &Expr) -> bool {
    match expr {
        Expr::Empty
        | Expr::Any { .. }
        | Expr::Assertion(_)
        | Expr::Literal { .. }
        | Expr::Delegate { .. } => false,
        Expr::Concat(items) | Expr::Alt(items) => items.iter().any(has_unsupported),
        Expr::Group(child) | Expr::LookAround(child, _) | Expr::AtomicGroup(child) => {
            has_unsupported(child)
        }
        Expr::Repeat { child, .. } => has_unsupported(child),
        _ => true,
    }
}

/// 捕獲グループを含むか
fn has_group(expr: &Expr) -> bool {
    match expr {
        Expr::Group(_) => true,
        Expr::Concat(items) | Expr::Alt(items) => items.iter().any(has_group),
        Expr::LookAround(child, _) | Expr::AtomicGroup(child) => has_group(child),
        Expr::Repeat { child, .. } => has_group(child),
        _ => false,
    }
}

/// fancy-regex が VM で走らせる（= regex-automata へ丸ごと任せない）正規表現か。
/// fancy-regex 0.16 の `optimize` は「易しい本体 + 末尾の肯定先読み」と「先読みだけ」を
/// 易しい形へ直して任せるので、同じ変形をしてから判定する（任せられるものを書き換えると、
/// かえって VM へ落として遅くする）
pub(crate) fn runs_on_vm(expr: &Expr) -> bool {
    match expr {
        Expr::LookAround(child, LookAround::LookAhead) => is_hard(child),
        Expr::Concat(items) => match items.split_last() {
            Some((Expr::LookAround(child, LookAround::LookAhead), rest)) => {
                rest.iter().any(is_hard) || is_hard(child)
            }
            _ => is_hard(expr),
        },
        _ => is_hard(expr),
    }
}

/// fancy-regex 0.16 の `analyze` が hard とみなすもの（単語境界も VM 行き）
fn is_hard(expr: &Expr) -> bool {
    match expr {
        Expr::Empty | Expr::Any { .. } | Expr::Literal { .. } | Expr::Delegate { .. } => false,
        Expr::Assertion(assertion) => !is_line_or_text_anchor(assertion),
        Expr::Concat(items) | Expr::Alt(items) => items.iter().any(is_hard),
        Expr::Group(child) => is_hard(child),
        Expr::Repeat { child, .. } => is_hard(child),
        _ => true,
    }
}

fn is_line_or_text_anchor(assertion: &Assertion) -> bool {
    matches!(
        assertion,
        Assertion::StartText
            | Assertion::EndText
            | Assertion::StartLine { .. }
            | Assertion::EndLine { .. }
    )
}

/// 必要条件の候補から、最短一致長が最も長いもの（1 文字以上）を正規表現の文字列で返す
fn best_guard(expr: &Expr) -> Option<String> {
    let mut candidates = Vec::new();
    necessary(expr, &mut candidates, 0);
    let best = candidates
        .into_iter()
        .map(|candidate| (min_len(&candidate), candidate))
        .filter(|(len, _)| *len > 0)
        // 同じ長さなら先に集めた（= 単純な）候補を選ぶ
        .reduce(|best, next| if next.0 > best.0 { next } else { best })?;
    let mut guard = String::new();
    best.1.to_str(&mut guard, 0);
    Some(guard)
}

/// 「`expr` が位置 p で一致するなら、必ず p で一致する」易しい式を集める
fn necessary(expr: &Expr, out: &mut Vec<Expr>, depth: usize) {
    if let Some(relaxed) = relax(expr) {
        out.push(relaxed);
    }
    // 入れ子の深い文法でも候補が爆発しないように止める
    if depth > 4 {
        return;
    }
    match expr {
        Expr::LookAround(child, LookAround::LookAhead) | Expr::Group(child) => {
            necessary(child, out, depth + 1)
        }
        // 選択: 枝ごとに一番よい条件を選んで束ねる（`}|(?=\*/)` → `}|\*/`）
        Expr::Alt(items) => {
            let branches: Option<Vec<Expr>> = items
                .iter()
                .map(|item| {
                    let mut inner = Vec::new();
                    necessary(item, &mut inner, depth + 1);
                    inner.into_iter().reduce(|best, next| {
                        if min_len(&next) > min_len(&best) {
                            next
                        } else {
                            best
                        }
                    })
                })
                .collect();
            if let Some(branches) = branches {
                out.push(Expr::Alt(branches));
            }
        }
        Expr::Concat(_) => {
            // 途中の肯定先読み: そこまでの前半を緩めた列 + 先読みの中身
            let mut flat = Vec::new();
            flatten(expr, &mut flat);
            let mut prefix: Vec<Expr> = Vec::new();
            for item in flat {
                if let Expr::LookAround(child, LookAround::LookAhead) = item {
                    let mut inner = Vec::new();
                    necessary(child, &mut inner, depth + 1);
                    for candidate in inner {
                        let mut seq = prefix.clone();
                        seq.push(candidate);
                        out.push(Expr::Concat(seq));
                    }
                }
                match relax(item) {
                    Some(relaxed) => prefix.push(relaxed),
                    None => break,
                }
            }
        }
        _ => {}
    }
}

/// 連接とグループを平らに並べる（必要条件を作るときだけ使う。捕獲の有無は問わない）
fn flatten<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Concat(items) => items.iter().for_each(|item| flatten(item, out)),
        Expr::Group(child) => flatten(child, out),
        other => out.push(other),
    }
}

/// 先読み・後読み・単語境界を外し、グループを捕獲しない形にした上位集合。
/// 上位集合を作れない要素（後方参照など）を含むなら `None`
fn relax(expr: &Expr) -> Option<Expr> {
    Some(match expr {
        Expr::Empty | Expr::LookAround(..) | Expr::KeepOut => Expr::Empty,
        Expr::Assertion(assertion) if is_line_or_text_anchor(assertion) => {
            Expr::Assertion(*assertion)
        }
        // 単語境界は外す（位置を狭めるだけの条件なので、外しても上位集合）
        Expr::Assertion(_) => Expr::Empty,
        Expr::Any { .. } | Expr::Literal { .. } => expr.clone(),
        Expr::Delegate { inner, size, casei } => match widened_class(inner, *casei) {
            Some(widened) => Expr::Delegate {
                inner: widened,
                size: *size,
                casei: false,
            },
            None => expr.clone(),
        },
        Expr::Concat(items) => Expr::Concat(items.iter().map(relax).collect::<Option<_>>()?),
        Expr::Alt(items) => Expr::Alt(items.iter().map(relax).collect::<Option<_>>()?),
        Expr::Group(child) | Expr::AtomicGroup(child) => relax(child)?,
        Expr::Repeat {
            child,
            lo,
            hi,
            greedy,
        } => match relax(child)? {
            // 空の繰り返しは `*` だけが残る不正な正規表現になるので空へ畳む
            Expr::Empty => Expr::Empty,
            relaxed => Expr::Repeat {
                // 複数文字のリテラルは `to_str` が括弧を付けない（`ab*` になる）ので包む
                child: Box::new(match relaxed {
                    Expr::Literal { ref val, .. } if val.chars().count() > 1 => {
                        Expr::Concat(vec![relaxed])
                    }
                    other => other,
                }),
                lo: *lo,
                hi: *hi,
                greedy: *greedy,
            },
        },
        _ => return None,
    })
}

/// 文字クラスを「ASCII の部分 + 非 ASCII の全体」へ広げる（上位集合）。
///
/// 条件の正規表現は上位集合でありさえすればよい。`\p{L}` のような大きな Unicode クラスを
/// そのまま使うと UTF-8 のオートマトンが大きくなり、DFA が冷えている最初の数行で状態作りに
/// 時間を取られる（40 行の TS の 1 回目が +38 ms）。コードの行はほぼ ASCII なので、
/// 非 ASCII を丸ごと通しても前置きの効き目は落ちない。文字クラス 1 つでないもの・非 ASCII を
/// 含まないものは `None`（そのまま使う）
fn widened_class(inner: &str, casei: bool) -> Option<String> {
    // 候補を作るたびに同じクラスを読み直さない（Unicode の表を引くので 1 回 数十 µs）
    thread_local! {
        static MEMO: std::cell::RefCell<HashMap<(String, bool), Option<String>>> =
            std::cell::RefCell::new(HashMap::new());
    }
    let key = (inner.to_string(), casei);
    if let Some(hit) = MEMO.with(|memo| memo.borrow().get(&key).cloned()) {
        return hit;
    }
    let widened = widen(inner, casei);
    MEMO.with(|memo| memo.borrow_mut().insert(key, widened.clone()));
    widened
}

fn widen(inner: &str, casei: bool) -> Option<String> {
    use regex_syntax::hir::{Class, HirKind};
    let pattern = if casei {
        format!("(?i:{inner})")
    } else {
        inner.to_string()
    };
    let hir = regex_syntax::ParserBuilder::new()
        .build()
        .parse(&pattern)
        .ok()?;
    let HirKind::Class(Class::Unicode(class)) = hir.kind() else {
        return None;
    };
    if class.ranges().iter().all(|range| range.end().is_ascii()) {
        return None;
    }
    let mut out = String::from("[");
    for range in class
        .ranges()
        .iter()
        .filter(|range| range.start().is_ascii())
    {
        let end = range.end().min('\x7f');
        out.push_str(&format!(
            "\\x{{{:x}}}-\\x{{{:x}}}",
            range.start() as u32,
            end as u32
        ));
    }
    out.push_str("\\x{80}-\\x{10FFFF}]");
    Some(out)
}

/// 最短一致長（文字数。候補選びの目安にだけ使う）
fn min_len(expr: &Expr) -> usize {
    match expr {
        Expr::Any { .. } => 1,
        Expr::Literal { val, .. } => val.chars().count(),
        Expr::Delegate { size, .. } => *size,
        Expr::Concat(items) => items.iter().map(min_len).sum(),
        Expr::Alt(items) => items.iter().map(min_len).min().unwrap_or(0),
        Expr::Repeat { child, lo, .. } => min_len(child).saturating_mul(*lo),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn パースのフラグはsyntectと同じoniguruma_mode() {
        // oniguruma モードでは `\<` は単語境界ではなく文字の `<`
        let tree = Expr::parse_tree_with_flags(r"\<", PARSE_FLAGS).unwrap();
        assert_eq!(
            tree.expr,
            Expr::Literal {
                val: "<".into(),
                casei: false
            },
            "PARSE_FLAGS が oniguruma モードを指していない（fancy-regex のフラグの値が変わった）"
        );
        let plain = Expr::parse_tree_with_flags(r"\<", 0).unwrap();
        assert_ne!(plain.expr, tree.expr);
    }

    #[test]
    fn 書き換えは前置きと元の正規表現そのものになる() {
        let guarded = rewrite(r"\bfoo\b").expect("単語境界は VM 行きなので書き換える");
        assert_eq!(guarded, r"\G(?=(?s:.)*?(?:foo))(?s:.)*?\K(?:\bfoo\b)");
        // 先読みだけの正規表現は中身を条件に使う
        let lookahead = rewrite(r"(?=(?<!\.)\bif\s*\()").expect("先読みの中身を条件に使う");
        // `\s` は「ASCII の部分 + 非 ASCII の全体」へ広げてある
        assert!(
            lookahead
                .starts_with(r"\G(?=(?s:.)*?(?:if[\x{9}-\x{d}\x{20}-\x{20}\x{80}-\x{10FFFF}]*\())"),
            "{lookahead}"
        );
        // 途中の肯定先読み: そこまでの前半 + 中身
        let tail = rewrite(r"(?<!\.)([a-z]+)(?=\s*\.\s*prototype\b)").expect("書き換える");
        assert!(tail.contains("prototype"), "{tail}");
        // x モードのコメントで終わる正規表現は閉じ括弧の前に改行を挟む
        let commented = rewrite("(?x) (?<!\\.) \\b if \\b # if 文").expect("書き換える");
        assert!(commented.ends_with("# if 文\n)"), "{commented:?}");
    }

    #[test]
    fn 条件の文字クラスは元の上位集合へ広げる() {
        let widened = widened_class(r"[_$\p{L}]", false).expect("非 ASCII を含むので広げる");
        assert_eq!(
            widened,
            r"[\x{24}-\x{24}\x{41}-\x{5a}\x{5f}-\x{5f}\x{61}-\x{7a}\x{80}-\x{10FFFF}]"
        );
        // ASCII だけのクラスはそのまま（広げても小さくならない）
        assert_eq!(widened_class("[a-z_]", false), None);
        // 元のクラスに入る文字は必ず広げた方にも入る（大文字小文字を畳むクラスも）
        let probe: Vec<char> = (0u32..0x80)
            .filter_map(char::from_u32)
            .chain("éßΩЖあ漢字ｱ\u{a0}\u{2028}😀".chars())
            .collect();
        for (class, casei) in [
            (r"[_$\p{L}]", false),
            (r"[_$\p{L}\p{N}]", false),
            (r"\s", false),
            (r"\p{Lu}", false),
            (r"[a-zß]", true),
        ] {
            let widened = widened_class(class, casei).expect("広げる");
            let compile = |re: &str| fancy_regex::Regex::new(&format!("^{re}$")).unwrap();
            let original = if casei {
                compile(&format!("(?i:{class})"))
            } else {
                compile(class)
            };
            let wide = compile(&widened);
            for c in &probe {
                let text = c.to_string();
                if original.is_match(&text).unwrap() {
                    assert!(
                        wide.is_match(&text).unwrap(),
                        "{class} → {widened} が {c:?} を落とした"
                    );
                }
            }
        }
    }

    #[test]
    fn 書き換えない正規表現() {
        // 易しい正規表現（fancy-regex が元から regex-automata へ任せる）
        assert_eq!(rewrite(r"[a-z]+\s*="), None);
        // 易しい本体 + 末尾の肯定先読み（fancy-regex の optimize が任せる形）
        assert_eq!(rewrite(r"([a-z]+)(?=\s*=>)"), None);
        assert_eq!(rewrite(r"(?=[a-z]+\s*\()"), None);
        // 後方参照・\G・\K・条件分岐
        assert_eq!(rewrite(r"(?<=x)(['\x22])\w*\1"), None);
        assert_eq!(rewrite(r"\G(?<!\.)\bfoo"), None);
        assert_eq!(rewrite(r"(?<!\.)foo\Kbar"), None);
        // 必要条件が空文字列にしかならない（行のどこでも当たる = 前置きが効かない）
        assert_eq!(rewrite(r"(?<!\.)(?!\s)"), None);
        // 読めない正規表現
        assert_eq!(rewrite(r"(?<!\.)(unclosed"), None);
    }
}
