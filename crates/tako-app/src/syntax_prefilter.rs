//! TypeScript の塗りの正規表現へ「当たりようのない行では VM を起こさない」前置きを足す（#1917）
//!
//! ## 真因（release 実測・1 MB の TS）
//!
//! TS の構文定義（two-face = bat 由来、元は VS Code の TextMate 文法）は先読み・後読みだらけで、
//! fancy-regex はそういう正規表現（hard）を「先頭に `.*?` の輪を付けた VM」へコンパイルする。
//! 当たらない行では**行の全バイト位置で VM が走る**。syntect は 1 行ごとに文脈の正規表現を
//! それぞれ 1 回ずつ探すので、1 MB（27,747 行）で約 310 万回の探索 = 正規表現だけで 5.9 秒、
//! 365 本にほぼ均等に散っていた（1 位でも 8.6%）。どれも「1 行に 1 回呼ばれ、ほぼ当たらない」形。
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
//!
//! ## 対象と費用
//!
//! 書き換えるのは TypeScript / TypeScriptReact の 2 構文だけ（ほかの言語の構文は 1 バイトも
//! 変えない）。構文セットの直列化（syntect 5 の bincode）をこの 2 構文の文脈だけ解いて書き換え、
//! 詰め直したバイト列を**プロセスで 1 回だけ**作って使い回す。以後の構文セットの取得
//! （#815 の 30 秒解放の後を含む）は元と同じ `from_uncompressed_data` 1 回で済む。
//! 直列化の形が syntect の想定と食い違ったら（往復で元のバイト列に戻らない）書き換えずに
//! 元の構文セットを使う。
//!
//! release 実測: 1 MB の TS 5.9 → 1.8 秒（2 回目 5.6 → 1.5 秒）・TS / Rust の比 4.6 → 1.4 倍。
//! 引き換えに、書き換えた正規表現 1 本ごとに前置きの下請けエンジンが 1 つ増え、そのコンパイルの
//! 固定費（約 0.15 ms / 本）が**冷えた 1 回目**に乗る = 40 行の TS の初回だけ 81〜84 → 100〜101 ms。
//! 300 行で 1 回目から速くなる（343〜356 → 249〜252 ms）。表は `.agent/architecture.md`
//! 「TS の塗りの正規表現の前置き（#1917）」
//!
//! `TAKO_1917_LEGACY=1` で書き換え前（two-face の構文セットそのまま）へ戻して同一バイナリで
//! A/B を取れる。

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use fancy_regex::{Assertion, Expr, LookAround};
use serde::{Deserialize, Serialize};
use syntect::parsing::syntax_definition::{Context, ContextId, Pattern};
use syntect::parsing::{Regex, Scope, SyntaxSet};

/// 書き換える構文（two-face の構文名）
pub(crate) const TARGET_SYNTAXES: [&str; 2] = ["TypeScript", "TypeScriptReact"];

/// fancy-regex のパースフラグ。syntect は `RegexBuilder::oniguruma_mode(true)` で組み、
/// パーサは常に Unicode を足すので、oniguruma モード（`\<` `\>` を文字として読む）だけ立てる。
/// 0.16 の `FLAG_ONIGURUMA_MODE` は公開されていないので値で持つ（単体テストが意味を確かめる）
const PARSE_FLAGS: u32 = 1 << 6;

/// 書き換え前の挙動へ戻す（`TAKO_1917_LEGACY=1`。同一バイナリで A/B を取る入口）
pub(crate) fn legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1917_LEGACY").is_some())
}

/// 塗りに使う構文セット（[`crate::preview`] の `SyntectHighlighter::new` が呼ぶ）
pub(crate) fn syntax_set() -> SyntaxSet {
    if legacy() {
        return two_face::syntax::extra_newlines();
    }
    match built() {
        Ok(built) => syntect::dumps::from_uncompressed_data(&built.bytes)
            .unwrap_or_else(|_| two_face::syntax::extra_newlines()),
        Err(_) => two_face::syntax::extra_newlines(),
    }
}

/// 書き換えた構文セットの直列化と、書き換えた本数
pub(crate) struct Built {
    bytes: Vec<u8>,
    /// 対象構文の正規表現（`Pattern::Match`）の総数（単体テストが書き換えの効き目を数える）
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) patterns: usize,
    /// そのうち前置きを足した数
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) rewritten: usize,
}

/// 書き換えた構文セット（プロセスで 1 回だけ作る。失敗の理由は文字列で残す）
pub(crate) fn built() -> &'static Result<Built, String> {
    static BUILT: OnceLock<Result<Built, String>> = OnceLock::new();
    BUILT.get_or_init(|| build(&two_face::syntax::extra_newlines()))
}

/// 起動時に専用スレッドで [`built`] を済ませておく（release 実測 34〜36 ms。最初の塗りが
/// UI スレッドでも構築を払わない。構築中に塗りが来たら `OnceLock` が出来上がりを待つ）
pub(crate) fn warm_up() {
    if legacy() {
        return;
    }
    // スレッドを立てられない環境でも、最初の塗りで作るだけなので失敗は無視してよい
    let _ = std::thread::Builder::new()
        .name("tako-syntax-prefilter".into())
        .spawn(|| {
            let _ = built();
        });
}

// ---- syntect 5 の直列化の鏡（並び・型を syntect の `#[derive(Serialize)]` と揃える） ----

/// `SyntaxSet`（`first_line_cache` / `metadata` は直列化されない）
#[derive(Serialize, Deserialize)]
struct SetMirror {
    syntaxes: Vec<ReferenceMirror>,
    path_syntaxes: Vec<(String, usize)>,
}

/// `SyntaxReference`（`lazy_contexts` は直列化されない。`variables` は鍵の順に並ぶ）
#[derive(Serialize, Deserialize)]
struct ReferenceMirror {
    name: String,
    file_extensions: Vec<String>,
    scope: Scope,
    first_line_match: Option<String>,
    hidden: bool,
    variables: BTreeMap<String, String>,
    serialized_lazy_contexts: Vec<u8>,
}

/// `LazyContexts`（zlib で圧縮した bincode として `serialized_lazy_contexts` に入っている）
#[derive(Serialize, Deserialize, PartialEq)]
struct LazyMirror {
    context_ids: BTreeMap<String, ContextId>,
    contexts: Vec<Context>,
}

fn build(base: &SyntaxSet) -> Result<Built, String> {
    let original =
        bincode::serialize(base).map_err(|e| format!("構文セットを直列化できない: {e}"))?;
    let mut set: SetMirror =
        bincode::deserialize(&original).map_err(|e| format!("構文セットの形が想定と違う: {e}"))?;
    // 鏡の並びが syntect と食い違っていれば、往復で元のバイト列に戻らない
    if bincode::serialize(&set).ok().as_deref() != Some(&original[..]) {
        return Err("構文セットの直列化が往復で元に戻らない（syntect の形が変わった）".into());
    }
    let mut patterns = 0;
    let mut rewritten = 0;
    // TSX の正規表現はほぼ TS と同じ文字列なので、書き換えの結果を使い回す（構築 25 → 13 ms）
    let mut memo: HashMap<String, Option<String>> = HashMap::new();
    for syntax in set
        .syntaxes
        .iter_mut()
        .filter(|syntax| TARGET_SYNTAXES.contains(&syntax.name.as_str()))
    {
        let mut lazy: LazyMirror =
            syntect::dumps::from_reader(&syntax.serialized_lazy_contexts[..])
                .map_err(|e| format!("{} の文脈を読めない: {e}", syntax.name))?;
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
        syntax.serialized_lazy_contexts = compress(&lazy)?;
    }
    let bytes = bincode::serialize(&set)
        .map_err(|e| format!("書き換えた構文セットを直列化できない: {e}"))?;
    Ok(Built {
        bytes,
        patterns,
        rewritten,
    })
}

/// 文脈を syntect と同じ形（zlib で圧縮した bincode）へ詰め直す。
/// syntect の `dump_binary` は最高圧縮で 2 構文に 17 ms かかるので、速い圧縮にする
/// （読む側の `from_reader` は zlib なら圧縮の強さを問わない）
fn compress(lazy: &LazyMirror) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    bincode::serialize_into(&mut encoder, lazy)
        .map_err(|e| format!("文脈を直列化できない: {e}"))?;
    encoder
        .finish()
        .map_err(|e| format!("文脈を圧縮できない: {e}"))
}

// ---- 正規表現 1 本の書き換え ----

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
fn runs_on_vm(expr: &Expr) -> bool {
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
    use syntect::easy::HighlightLines;
    use syntect::highlighting::ThemeSet;
    use syntect::parsing::{ParseState, SyntaxDefinition};
    use syntect::util::LinesWithEndings;

    /// TS の見本（宣言・式・型・正規表現リテラル・テンプレート・JSDoc・デコレータ・壊れた行）
    const TS_SAMPLE: &str = r#"/**
 * JSDoc の見本 {@link Foo} @param {string} name 名前
 * @returns {Promise<number>} 数
 */
import { readFile } from "node:fs/promises";
import type { Config as Cfg } from './config';
export * as utils from "./utils";

declare module "x" { export const y: number; }
namespace Outer.Inner { export enum Kind { A = 1, B = A << 2, C = "c" } }
const enum Flags { None = 0, Read = 1 << 0, Write = 1 << 1 }

@sealed
export abstract class Shape<T extends object = {}> implements Drawable {
  #secret = 42;
  private static readonly cache = new Map<string, Shape<any>>();
  protected abstract area(): number;
  constructor(public readonly name: string, private opts?: Partial<T>) { super(); }
  get label(): string { return `${this.name}:${this.#secret}`; }
  set label(v) { this.name = v as unknown as string; }
  async *walk(this: Shape<T>, depth = 0): AsyncGenerator<T, void, unknown> {
    for await (const item of this.items ?? []) yield item!;
  }
} // Shape の終わり（行頭の `}` だけの行は #1399 の番犬がテストの終わりと読むので避ける）

type Mapped<T> = { readonly [K in keyof T as `get${Capitalize<string & K>}`]-?: () => T[K] };
type Cond<T> = T extends (infer U)[] ? U : T extends Promise<infer V> ? V : never;
type Tuple = [first: string, second?: number, ...rest: boolean[]];
interface Point { x: number; y: number; readonly z?: number; [key: string]: unknown }
function isPoint(v: unknown): v is Point { return typeof v === "object" && v !== null && "x" in v; }
function assert(cond: unknown, msg?: string): asserts cond { if (!cond) throw new Error(msg); }

export default async function main<T,>(argv: readonly string[]): Promise<number> {
  const re = /^(?:[a-z]+|\d{2,})\/[^/\n]*$/giu, half = total / 2 / count;
  const tpl = tag`line ${argv.length} and ${await readFile("a", "utf8")}`;
  let x = obj?.a?.[0]?.(1) ?? defaultValue!;
  x ||= 1; x &&= 2; x ??= 3; x **= 2; x >>>= 1;
  const fn = async <U,>(a: U, { b, c = 2 }: { b: string; c?: number }, ...rest: U[]) => a;
  const arrow = (a: number): a is 1 => a === 1;
  const obj2 = { method() { return 1; }, get g() { return 2; }, [computed]: 3, 'quoted': 4, async *gen() {} };
  if (x < 10 && y > 2) { console.log(x as number, <string>y, satisfiesCheck satisfies object); }
  switch (kind) { case Kind.A: break; default: return -1; }
  label: for (let i = 0; i < 10; i++) { if (i % 2) continue label; }
  try { JSON.parse("{}"); } catch { /* 何もしない */ } finally { void 0; }
  return 0x1F + 0b1010 + 0o17 + 1_000_000n + 1.5e-3;
} // main の終わり

// Unicode の識別子と文字列
const 名前 = 値?.長さ ?? "既定";
class Ünïcödé { ñ = 1; }

// 壊れた行（閉じない文字列・コメント・括弧）
const broken = "unterminated
let a = (1 + [2, { b: `x ${ y
/* 閉じないコメント
const after = 1;
"#;

    /// TSX の見本
    const TSX_SAMPLE: &str = r#"import React, { useState } from "react";
type Props = { title: string; items?: readonly string[] };
export const List: React.FC<Props> = ({ title, items = [] }) => {
  const [open, setOpen] = useState<boolean>(false);
  const generic = <T,>(v: T) => v;
  return (
    <section className="list" data-open={open} onClick={() => setOpen(o => !o)}>
      <h2>{title} &amp; more</h2>
      {items.length > 0 ? (
        <ul>{items.map((item, i) => <li key={i}>{item}</li>)}</ul>
      ) : <Empty<string> fallback={<span>none</span>} />}
      {/* JSX のコメント */}
    </section>
  );
};
"#;

    fn base() -> SyntaxSet {
        two_face::syntax::extra_newlines()
    }

    /// 1 行ずつの (スコープ操作, 色・太字・斜体・文字) を並べた塗りの全記録
    fn painted(set: &SyntaxSet, syntax_name: &str, text: &str) -> Vec<String> {
        let syntax = set.find_syntax_by_name(syntax_name).expect("構文がある");
        let theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-eighties.dark")
            .expect("tako と同じテーマ");
        let mut parse = ParseState::new(syntax);
        let mut lines = HighlightLines::new(syntax, &theme);
        LinesWithEndings::from(text)
            .map(|line| {
                let ops = parse.parse_line(line, set).expect("パースできる");
                let regions = lines.highlight_line(line, set).expect("塗れる");
                format!("{ops:?} {regions:?}")
            })
            .collect()
    }

    /// 構文名 → (文脈名, 何番目の正規表現) → 正規表現の文字列
    fn regexes(set: &SyntaxSet, syntax_name: &str) -> BTreeMap<(String, usize), String> {
        let definition: SyntaxDefinition = set
            .clone()
            .into_builder()
            .syntaxes()
            .iter()
            .find(|definition| definition.name == syntax_name)
            .expect("構文がある")
            .clone();
        let mut out = BTreeMap::new();
        for (name, context) in &definition.contexts {
            for (index, pattern) in context
                .patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    Pattern::Match(matcher) => Some(matcher.regex.regex_str().to_string()),
                    Pattern::Include(_) => None,
                })
                .enumerate()
            {
                out.insert((name.clone(), index), pattern);
            }
        }
        out
    }

    /// 探索 1 回のバックトラック回数（fancy-regex の上限を二分探索して数える。決定的）
    fn backtracks(re: &str, text: &str) -> usize {
        let fits = |limit: usize| {
            fancy_regex::RegexBuilder::new(re)
                .oniguruma_mode(true)
                .backtrack_limit(limit)
                .build()
                .expect("コンパイルできる")
                .captures_from_pos(text, 0)
                .is_ok()
        };
        let (mut lo, mut hi) = (0usize, 1_000_000usize);
        assert!(fits(hi), "上限内で終わる: {re}");
        while lo < hi {
            let mid = (lo + hi) / 2;
            if fits(mid) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }

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

    #[test]
    fn 書き換えた正規表現は元と同じ位置と同じグループで一致する() {
        let corpus: Vec<&str> = LinesWithEndings::from(TS_SAMPLE)
            .chain(LinesWithEndings::from(TSX_SAMPLE))
            .collect();
        let mut compared = 0;
        for syntax in TARGET_SYNTAXES {
            for ((context, index), original) in regexes(&base(), syntax) {
                let Some(guarded) = rewrite(&original) else {
                    continue;
                };
                let compile = |re: &str| {
                    fancy_regex::RegexBuilder::new(re)
                        .oniguruma_mode(true)
                        .build()
                        .expect("コンパイルできる")
                };
                let (before, after) = (compile(&original), compile(&guarded));
                for line in &corpus {
                    // syntect は行の途中からも探す（直前のトークンの後ろ）
                    let starts = [
                        0,
                        line.len() / 3,
                        line.len() / 2,
                        line.len().saturating_sub(2),
                    ];
                    for start in starts.into_iter().filter(|s| line.is_char_boundary(*s)) {
                        let groups = |re: &fancy_regex::Regex| {
                            re.captures_from_pos(line, start)
                                .ok()
                                .flatten()
                                .map(|caps| {
                                    (0..caps.len())
                                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                                        .collect::<Vec<_>>()
                                })
                        };
                        assert_eq!(
                            groups(&before),
                            groups(&after),
                            "{syntax}::{context}[{index}] の一致が変わった（{start} から {line:?}）\n元: {original}\n後: {guarded}"
                        );
                        compared += 1;
                    }
                }
            }
        }
        assert!(compared > 50_000, "比べた探索が少なすぎる: {compared}");
    }

    #[test]
    fn tsとtsxの塗りは書き換えの前後で一致する() {
        let (before, after) = (base(), syntax_set());
        for (syntax, sample) in [
            ("TypeScript", TS_SAMPLE),
            ("TypeScriptReact", TSX_SAMPLE),
            ("TypeScriptReact", TS_SAMPLE),
        ] {
            let (old, new) = (
                painted(&before, syntax, sample),
                painted(&after, syntax, sample),
            );
            assert_eq!(old.len(), new.len());
            for (line, (old, new)) in old.iter().zip(&new).enumerate() {
                assert_eq!(old, new, "{syntax} の {} 行目の塗りが変わった", line + 1);
            }
        }
    }

    #[test]
    fn ts以外の構文は1バイトも変えない() {
        let Ok(built) = built() else {
            panic!(
                "書き換えた構文セットを作れない: {:?}",
                built().as_ref().err()
            );
        };
        let original: SetMirror =
            bincode::deserialize(&bincode::serialize(&base()).unwrap()).unwrap();
        let rewritten: SetMirror = bincode::deserialize(&built.bytes).unwrap();
        assert_eq!(original.syntaxes.len(), rewritten.syntaxes.len());
        assert_eq!(original.path_syntaxes, rewritten.path_syntaxes);
        let mut touched = Vec::new();
        for (old, new) in original.syntaxes.iter().zip(&rewritten.syntaxes) {
            assert_eq!(old.name, new.name);
            assert_eq!(old.file_extensions, new.file_extensions);
            assert_eq!(old.variables, new.variables);
            if old.serialized_lazy_contexts != new.serialized_lazy_contexts {
                touched.push(old.name.clone());
            }
        }
        assert_eq!(
            touched, TARGET_SYNTAXES,
            "書き換えたのは TS / TSX の文脈だけ"
        );
    }

    /// 重い正規表現の上位（release 実測で 1 MB の TS の正規表現の時間の 40% 超）が書き換わっていて、
    /// 当たらない行では**元の本体を一度も試さない**（バックトラックが外側の輪の分だけ）こと。
    /// `TAKO_1917_LEGACY=1` や書き換えの退行で、名指しで落ちる
    #[test]
    fn 重い正規表現は当たらない行で本体を試さない() {
        let (before, after) = (
            regexes(&base(), "TypeScript"),
            regexes(&syntax_set(), "TypeScript"),
        );
        // (文脈, 元の正規表現の書き出し)
        let heavy = [
            (
                "template-call",
                r"(?=(([_$\p{L}][_$\p{L}\p{N}]*\s*\??\.\s*)*|(\??\.\s*)?)([_$\p{L}]",
            ),
            (
                "function-call",
                r"(?=(((([_$\p{L}][_$\p{L}\p{N}]*)(\s*\??\.\s*(\#?[_$\p{L}]",
            ),
            (
                "after-operator-block-as-object-literal",
                r"(?<!\+\+|--)(?<=[:=(,\[?+!>]",
            ),
            (
                "support-objects",
                r"(?x) (?<![_$\p{L}\p{N}])(?:(?<=\.\.\.)|(?<!\.)) \b (?:",
            ),
            ("cast", r"(?:(?<!\+\+|--)(?<=^return|"),
            (
                "arrow-function",
                r"(?:(?<![_$\p{L}\p{N}])(?:(?<=\.\.\.)|(?<!\.))(\basync)\s+)?",
            ),
            (
                "object-identifiers",
                r"([_$\p{L}][_$\p{L}\p{N}]*)(?=\s*\??\.\s*prototype\b(?!\$))",
            ),
            ("regex", r"(?<!\+\+|--|})(?<=[=(:,\[?+!]|^return|"),
        ];
        // どれも当たらない行（コメントでも文字列でもない、ありふれたコード）
        let line = "    total += values[index] * weight - offset;\n";
        for (context, head) in heavy {
            let key = before
                .iter()
                .find(|((name, _), re)| name == context && re.starts_with(head))
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| panic!("TypeScript::{context} に `{head}` で始まる正規表現が無い（構文定義が変わった）"));
            let (original, guarded) = (&before[&key], &after[&key]);
            assert!(
                original != guarded,
                "TypeScript::{context}[{}] が書き換わっていない（TAKO_1917_LEGACY が立っているか、書き換えが退行した）",
                key.1
            );
            let (old, new) = (backtracks(original, line), backtracks(guarded, line));
            // 前置きが行を弾くと、残るのは外側の輪が各位置で `\G` に 1 回ずつ失敗する分だけ
            assert!(
                new <= line.len() + 2,
                "TypeScript::{context}[{}] は当たらない行で本体を試している（{new} 回 > 行の長さ {}）",
                key.1,
                line.len()
            );
            assert!(
                old > new,
                "TypeScript::{context}[{}]: 元 {old} 回 / 後 {new} 回",
                key.1
            );
        }
    }

    /// VM で走る正規表現（= 遅い側）のうち、前置きを足せたものの割合。
    /// 書き換えの対象を狭める退行（候補の作り方・hard の判定）を数で捕まえる
    #[test]
    fn vmで走る正規表現の大半を書き換える() {
        let Ok(built) = built() else {
            panic!(
                "書き換えた構文セットを作れない: {:?}",
                built().as_ref().err()
            );
        };
        let (mut on_vm, mut guarded) = (0, 0);
        for syntax in TARGET_SYNTAXES {
            for re in regexes(&base(), syntax).values() {
                let Ok(tree) = Expr::parse_tree_with_flags(re, PARSE_FLAGS) else {
                    continue;
                };
                if runs_on_vm(&tree.expr) {
                    on_vm += 1;
                    guarded += usize::from(rewrite(re).is_some());
                }
            }
        }
        eprintln!(
            "[1917] TS/TSX の正規表現 {} 本 / VM 行き {on_vm} 本 / 書き換え {guarded} 本（構文セット上 {} 本）",
            built.patterns, built.rewritten
        );
        assert_eq!(
            guarded, built.rewritten,
            "構文セットの書き換えと 1 本ずつの書き換えが食い違う"
        );
        // 書けないのは行末・行頭の錨や直前の文字だけを見る幅 0 の正規表現（文脈を抜ける側）で、
        // 必要条件を作っても行のどこでも当たる。2026-10-09 時点で 425 / 573
        assert!(
            guarded * 10 >= on_vm * 7,
            "VM で走る正規表現のうち書き換えたのが 7 割未満: {guarded} / {on_vm}"
        );
    }

    /// 塗りの所要の計測（release で手動実行する。#1917）。
    /// `TAKO_1917_PERF_FILES=a.ts:b.rs` の各ファイルを、アプリと同じ `preview::highlight_text` で
    /// 2 回ずつ塗る（区切りは OS の PATH と同じ = Windows は `;`。1 回目は文脈の展開と
    /// 正規表現のコンパイルを含む）。
    /// `TAKO_1917_LEGACY=1` を付けると書き換え前の構文セットで同じことをする。
    /// `TAKO_1917_PERF_COMPARE=1` を付けると、`.ts` / `.tsx` の塗りの全記録（スコープ操作と色）を
    /// 書き換え前の構文セットと 1 行ずつ比べる
    #[test]
    #[ignore = "release で手動実行する計測（#1917）"]
    fn perf_1917_塗りの所要() {
        use std::time::Instant;
        let files = std::env::var("TAKO_1917_PERF_FILES").expect("TAKO_1917_PERF_FILES が要る");
        let t = Instant::now();
        let size = built().as_ref().map(|built| built.bytes.len());
        eprintln!(
            "[perf1917] legacy={} 書き換えの構築 {:?}（使い回すバイト列 {size:?}）",
            legacy(),
            t.elapsed()
        );
        let t = Instant::now();
        drop(syntax_set());
        eprintln!("[perf1917] 構文セットの取得 {:?}", t.elapsed());
        for path in std::env::split_paths(&files) {
            let path = path.as_path();
            let file = path.display();
            let text = std::fs::read_to_string(path).expect("読める");
            for round in 1..=2 {
                let t = Instant::now();
                let lines = crate::preview::highlight_text(path, &text);
                eprintln!(
                    "[perf1917] {} {} バイト {} 行 {round} 回目 {:.3} 秒",
                    path.file_name().unwrap().to_string_lossy(),
                    text.len(),
                    lines.len(),
                    t.elapsed().as_secs_f64()
                );
            }
            let syntax = match path.extension().and_then(|ext| ext.to_str()) {
                Some("ts") => "TypeScript",
                Some("tsx") => "TypeScriptReact",
                _ => continue,
            };
            if std::env::var_os("TAKO_1917_PERF_COMPARE").is_some() {
                let (before, after) = (
                    painted(&base(), syntax, &text),
                    painted(&syntax_set(), syntax, &text),
                );
                let differ = before.iter().zip(&after).position(|(old, new)| old != new);
                assert_eq!(differ, None, "{file} の塗りが変わった（行 {differ:?}）");
                assert_eq!(before.len(), after.len());
                eprintln!(
                    "[perf1917] {file}: 塗りの全記録 {} 行が前後で一致",
                    before.len()
                );
            }
        }
    }
}
