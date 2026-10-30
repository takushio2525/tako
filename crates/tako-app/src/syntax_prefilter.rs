//! 構文の塗りの正規表現の前置き（#1917 / #1935）を塗りの構文セットへ差し込み、
//! 入力の大きさで「前置き付き」と「元の正規表現」の構文セットを使い分ける
//!
//! ## 前置き（書き換えそのものと等価性の根拠は [`crate::syntax_rewrite`]）
//!
//! 先読み・後読みの多い構文定義では、fancy-regex が正規表現を「行の全バイト位置で VM を回す」
//! 形で走らせるので、当たらない行でも VM が回る。正規表現 `R` を
//! `\G(?=(?s:.)*?(?:G))(?s:.)*?\K(?:R)`（`G` は易しい必要条件）へ書き換えると、
//! 行に `G` が無ければ DFA の 1 回の走査で落とせる。release 実測で 1 MB の塗りが
//! TS 5.9 → 1.8 秒（#1917）・JavaScript 3.8 → 1.9 秒・C# 3.2 → 1.0 秒・Rust 1.3 → 0.7 秒（#1935）。
//!
//! **書き換えはビルド時**に `build.rs` が行い、対象の構文（[`crate::syntax_mirror::TARGETS`]）の
//! 書き換え済みの文脈を `OUT_DIR/prefiltered-syntaxes.bin` へ置く。起動時は土台の構文セット
//! （two-face + 足した構文。[`crate::syntax_extra::base_set`]）の同じ構文へ差し込むだけで、
//! 正規表現は 1 本も読まない（#1917 の実行時の書き換えは TS / TSX の 2 構文で 34〜36 ms・
//! 全 214 構文なら約 340 ms かかる）。差し込む前に土台の文脈が生成したときと同じか（指紋）を
//! 確かめ、違えばその構文は元のまま使う。
//!
//! ## 小さい入力は元の正規表現で塗る（#1935）
//!
//! 前置き 1 本ごとに下請けのエンジン（regex-automata）のコンパイルの固定費（約 0.07 ms +
//! 前置きの長さに比例する分）が冷えた 1 回目に乗り、小さい入力では探索で取り返せない
//! （冷えた 40 行で Rust +3.5 ms・C# +18 ms・TS +16 ms）。チャットのコードブロック・LSP の
//! ホバー・エディタの全文の塗り直し（5,000 行以下）は UI スレッドで塗るので、
//! **構文ごとの行数（[`plain_max_lines`]）以下の入力は元の正規表現の構文セット（[`plain_set`]）で
//! 塗り**、それより大きい入力だけ前置き付き（[`syntax_set`]）で塗る。行数は言語ごとの損益分岐
//! （冷えた構文セットで先頭 n 行を塗る所要が逆転する行数。TS 約 50 行〜JS / CSS を埋め込む HTML
//! 約 10,000 行）の実測から選んだ（`syntax_mirror::Target::plain_max_lines`）。TS を埋め込む
//! Svelte / Vue の大きい入力は TS / TSX だけ前置き付きの構文セット（[`ts_only_set`] = #1917 と同じ）で
//! 塗る（[`Variant`]）。どの構文セットも正規表現の文字列だけが違い、構文・文脈の並びと番号は同じ
//! （塗りの全記録が一致することを単体テストが確かめる）。
//!
//! 表は `.agent/architecture.md`「構文の塗りの正規表現の前置き（#1917 / #1935）」。
//!
//! A/B: `TAKO_1917_LEGACY=1` = 書き換え前（土台の構文セットそのもの）/
//! `TAKO_1935_LEGACY=1` = #1935 より前（TS / TSX だけ前置き付きの構文セットを、入力の大きさに
//! かかわらず使う）。

use std::sync::OnceLock;

use syntect::parsing::SyntaxSet;

use crate::syntax_mirror::{
    Fingerprint, PrefilteredSyntax, SetMirror, LEGACY_1917_SYNTAXES, TARGETS,
};

/// `build.rs` が作った、対象の構文の書き換え済みの文脈（`Vec<PrefilteredSyntax>` の bincode）
static PREFILTERED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prefiltered-syntaxes.bin"));

/// 対象外の構文の [`plain_max_lines`]。対象外の構文自身の正規表現は 2 つの構文セットで同じなので、
/// 前置き付きで塗って効くのは埋め込みの対象（HTML 系・PHP・テンプレートの中の JS / CSS / HTML 等）だけ。
/// 埋め込み先の前置きのコンパイル費で損益分岐が後ろへずれる（JS / CSS を埋め込む HTML の実測で
/// 8,000〜12,000 行）ので、HTML と同じ 10,000 行
const OTHER_PLAIN_MAX_LINES: usize = 10_000;

/// 対象外のうち TS / TSX を埋め込む構文（`<script lang="ts">`）。#1917 から中の TS を全入力で
/// 前置き付きで塗っていたので、TS と同じ行数を超えたら前置き付きで塗る（単体テストが two-face の
/// 構文から洗い出した表と突き合わせる。Typst は文書の中のコード片なので対象外の既定のまま）
const TS_EMBEDDERS: [&str; 2] = ["Svelte", "Vue Component"];

/// `syntax` の入力をこの行数以下なら元の正規表現の構文セットで塗る（モジュールの doc「小さい入力は…」）
pub(crate) fn plain_max_lines(syntax: &str) -> usize {
    let by_name = |name: &str| {
        TARGETS
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.plain_max_lines)
    };
    by_name(syntax).unwrap_or_else(|| {
        if TS_EMBEDDERS.contains(&syntax) {
            by_name("TypeScript").unwrap_or(OTHER_PLAIN_MAX_LINES)
        } else {
            OTHER_PLAIN_MAX_LINES
        }
    })
}

/// 書き換え前の挙動へ戻す（`TAKO_1917_LEGACY=1`。同一バイナリで A/B を取る入口）
pub(crate) fn legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1917_LEGACY").is_some())
}

/// #1935 より前の挙動へ戻す（`TAKO_1935_LEGACY=1`。TS / TSX だけ・入力の大きさで分けない）
pub(crate) fn legacy_1935() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1935_LEGACY").is_some())
}

/// 入力の大きさで構文セットを使い分けるか（A/B の env が立っていれば分けない）
pub(crate) fn split_by_size() -> bool {
    !legacy() && !legacy_1935()
}

/// 塗りに使う構文セットの種類（#1935）
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Variant {
    /// 元の正規表現（小さい入力・構文の解決）
    #[default]
    Plain,
    /// TS / TSX だけ前置き付き（= #1917 の構文セット。TS を埋め込む Svelte / Vue の大きい入力）
    TsOnly,
    /// 対象の全構文が前置き付き（大きい入力）
    All,
}

/// `syntax` の `lines` 行の入力を塗る構文セットの種類
pub(crate) fn variant_for(syntax: &str, lines: usize) -> Variant {
    if !split_by_size() || lines <= plain_max_lines(syntax) {
        Variant::Plain
    } else if TS_EMBEDDERS.contains(&syntax) {
        // 中の CSS / JS まで前置き付きにすると、そのコンパイル費で冷えた 1 回目が #1917 より
        // 10〜20 ms 遅くなる（合成の SFC の実測）。#1917 と同じ「TS だけ」で塗る
        Variant::TsOnly
    } else {
        Variant::All
    }
}

/// 大きい入力（[`plain_max_lines`] 行超）の塗りに使う構文セット = 対象の全構文が前置き付き
pub(crate) fn syntax_set() -> SyntaxSet {
    if legacy() {
        return crate::syntax_extra::base_set();
    }
    if legacy_1935() {
        return ts_only_set();
    }
    match built() {
        Ok(built) => syntect::dumps::from_uncompressed_data(&built.bytes)
            .unwrap_or_else(|_| crate::syntax_extra::base_set()),
        Err(_) => crate::syntax_extra::base_set(),
    }
}

/// TS / TSX だけ前置き付きの構文セット（= #1917 の構文セット。`TAKO_1935_LEGACY=1` でも使う）
pub(crate) fn ts_only_set() -> SyntaxSet {
    if legacy() {
        return crate::syntax_extra::base_set();
    }
    match ts_only_bytes() {
        Ok(bytes) => syntect::dumps::from_uncompressed_data(bytes)
            .unwrap_or_else(|_| crate::syntax_extra::base_set()),
        Err(_) => crate::syntax_extra::base_set(),
    }
}

/// 小さい入力の塗りと構文の解決に使う構文セット = 元の正規表現。
/// `TAKO_1935_LEGACY=1` では入力の大きさで分けないので、TS / TSX だけ前置き付きのもの
pub(crate) fn plain_set() -> SyntaxSet {
    if legacy() {
        return crate::syntax_extra::base_set();
    }
    if legacy_1935() {
        return ts_only_set();
    }
    match built() {
        Ok(built) => syntect::dumps::from_uncompressed_data(&built.plain)
            .unwrap_or_else(|_| crate::syntax_extra::base_set()),
        Err(_) => crate::syntax_extra::base_set(),
    }
}

/// 起動時に作る 2 つの構文セットの直列化（どちらも `from_uncompressed_data` で読める形）
pub(crate) struct Built {
    /// 対象の全構文が前置き付き
    bytes: Vec<u8>,
    /// 元の正規表現（土台そのもの）
    plain: Vec<u8>,
    /// 差し込んだ構文の正規表現（`Pattern::Match`）の総数（単体テストが効き目を数える）
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) patterns: usize,
    /// そのうち前置きを足した数
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) rewritten: usize,
    /// 差し込まなかった構文（土台に無い・土台の文脈が生成したときと違う）
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) skipped: Vec<String>,
}

/// 2 つの構文セットの直列化（プロセスで 1 回だけ作る。失敗の理由は文字列で残す）
pub(crate) fn built() -> &'static Result<Built, String> {
    static BUILT: OnceLock<Result<Built, String>> = OnceLock::new();
    BUILT.get_or_init(|| build(&crate::syntax_extra::base_set(), PREFILTERED))
}

/// TS / TSX だけ前置き付きの構文セットの直列化。Svelte / Vue の大きい入力を初めて塗るとき
/// （と `TAKO_1935_LEGACY=1`）に、元の構文セットのバイト列へ差し込んで作る（release 1.6 ms・
/// 塗りの初回は background で起きる。作ったら常駐する）
fn ts_only_bytes() -> &'static Result<Vec<u8>, String> {
    static TS_ONLY: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    TS_ONLY.get_or_init(|| {
        let built = built().as_ref().map_err(String::clone)?;
        splice(&built.plain, PREFILTERED, |name| {
            LEGACY_1917_SYNTAXES.contains(&name)
        })
        .map(|spliced| spliced.bytes)
    })
}

/// 起動時に専用スレッドで [`built`] を済ませておく（最初の塗りが UI スレッドでも構築を払わない。
/// 構築中に塗りが来たら `OnceLock` が出来上がりを待つ）
pub(crate) fn warm_up() {
    if legacy() {
        return;
    }
    // スレッドを立てられない環境でも、最初の塗りで作るだけなので失敗は無視してよい
    let _ = std::thread::Builder::new()
        .name("tako-syntax-prefilter".into())
        .spawn(|| {
            let _ = built();
            if legacy_1935() {
                let _ = ts_only_bytes();
            }
        });
}

/// 土台の構文セットへ、ビルド時に書き換えた文脈（`prefiltered`）を全部差し込む
fn build(base: &SyntaxSet, prefiltered: &[u8]) -> Result<Built, String> {
    let original =
        bincode::serialize(base).map_err(|e| format!("構文セットを直列化できない: {e}"))?;
    let set: SetMirror =
        bincode::deserialize(&original).map_err(|e| format!("構文セットの形が想定と違う: {e}"))?;
    // 鏡の並びが syntect と食い違っていれば、往復で元のバイト列に戻らない
    if bincode::serialize(&set).ok().as_deref() != Some(&original[..]) {
        return Err("構文セットの直列化が往復で元に戻らない（syntect の形が変わった）".into());
    }
    let all = splice(&original, prefiltered, |_| true)?;
    Ok(Built {
        bytes: all.bytes,
        plain: original,
        patterns: all.patterns,
        rewritten: all.rewritten,
        skipped: all.skipped,
    })
}

/// [`splice`] の結果
struct Spliced {
    bytes: Vec<u8>,
    patterns: usize,
    rewritten: usize,
    skipped: Vec<String>,
}

/// 構文セットの直列化 `plain` の、`keep` が真の構文へ、ビルド時に書き換えた文脈を差し込む
fn splice(plain: &[u8], prefiltered: &[u8], keep: fn(&str) -> bool) -> Result<Spliced, String> {
    let mut set: SetMirror =
        bincode::deserialize(plain).map_err(|e| format!("構文セットの形が想定と違う: {e}"))?;
    let generated: Vec<PrefilteredSyntax> = bincode::deserialize(prefiltered)
        .map_err(|e| format!("ビルド時に書き換えた文脈を読めない: {e}"))?;
    let (mut patterns, mut rewritten, mut skipped) = (0, 0, Vec::new());
    for one in generated.into_iter().filter(|one| keep(&one.name)) {
        let Some(syntax) = set
            .syntaxes
            .iter_mut()
            .find(|syntax| syntax.name == one.name)
        else {
            skipped.push(one.name);
            continue;
        };
        // 生成したときの土台と違う文脈（two-face の版のずれ）には差し込まない = 元のまま塗る
        if Fingerprint::of(&syntax.serialized_lazy_contexts) != one.source {
            skipped.push(one.name);
            continue;
        }
        syntax.serialized_lazy_contexts = one.contexts;
        patterns += one.patterns as usize;
        rewritten += one.rewritten as usize;
    }
    let bytes = bincode::serialize(&set)
        .map_err(|e| format!("前置き付きの構文セットを直列化できない: {e}"))?;
    Ok(Spliced {
        bytes,
        patterns,
        rewritten,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use fancy_regex::Expr;
    use serde::Deserialize;
    use syntect::easy::HighlightLines;
    use syntect::highlighting::ThemeSet;
    use syntect::parsing::syntax_definition::{Context, Pattern};
    use syntect::parsing::ParseState;
    use syntect::util::LinesWithEndings;

    use crate::syntax_mirror::LazyMirror;
    use crate::syntax_rewrite::{rewrite, runs_on_vm, PARSE_FLAGS};

    /// 対象の構文ごとの見本（宣言・式・文字列・コメント・埋め込み・壊れた行を含む手書きの短いもの）
    const SAMPLES: &[(&str, &str)] = &[
        (
            "TypeScript",
            include_str!("../testdata/syntax-samples/typescript.txt"),
        ),
        (
            "TypeScriptReact",
            include_str!("../testdata/syntax-samples/tsx.txt"),
        ),
        ("Rust", include_str!("../testdata/syntax-samples/rust.txt")),
        (
            "Python",
            include_str!("../testdata/syntax-samples/python.txt"),
        ),
        ("C", include_str!("../testdata/syntax-samples/c.txt")),
        ("C++", include_str!("../testdata/syntax-samples/cpp.txt")),
        ("Go", include_str!("../testdata/syntax-samples/go.txt")),
        ("Java", include_str!("../testdata/syntax-samples/java.txt")),
        (
            "JavaScript",
            include_str!("../testdata/syntax-samples/javascript.txt"),
        ),
        (
            "Bourne Again Shell (bash)",
            include_str!("../testdata/syntax-samples/bash.txt"),
        ),
        (
            "Markdown",
            include_str!("../testdata/syntax-samples/markdown.txt"),
        ),
        ("YAML", include_str!("../testdata/syntax-samples/yaml.txt")),
        ("HTML", include_str!("../testdata/syntax-samples/html.txt")),
        ("CSS", include_str!("../testdata/syntax-samples/css.txt")),
        ("Ruby", include_str!("../testdata/syntax-samples/ruby.txt")),
        ("Perl", include_str!("../testdata/syntax-samples/perl.txt")),
        (
            "Swift",
            include_str!("../testdata/syntax-samples/swift.txt"),
        ),
        ("C#", include_str!("../testdata/syntax-samples/csharp.txt")),
        (
            "Kotlin",
            include_str!("../testdata/syntax-samples/kotlin.txt"),
        ),
        ("Lua", include_str!("../testdata/syntax-samples/lua.txt")),
        ("SQL", include_str!("../testdata/syntax-samples/sql.txt")),
    ];

    /// 塗りの土台（two-face に無い構文を足したもの。#1954）
    fn base() -> SyntaxSet {
        crate::syntax_extra::base_set()
    }

    /// 前置きを足す構文の名前（表の並び）
    fn target_syntaxes() -> Vec<&'static str> {
        TARGETS.iter().map(|target| target.name).collect()
    }

    /// 書き換えた構文セットの組み立て（失敗なら名指しで落とす）
    fn built_ok() -> &'static Built {
        built()
            .as_ref()
            .unwrap_or_else(|e| panic!("前置き付きの構文セットを作れない: {e}"))
    }

    fn prefiltered() -> SyntaxSet {
        syntect::dumps::from_uncompressed_data(&built_ok().bytes).expect("読める")
    }

    fn sample(syntax: &str) -> &'static str {
        SAMPLES
            .iter()
            .find(|(name, _)| *name == syntax)
            .map(|(_, text)| *text)
            .unwrap_or_else(|| panic!("{syntax} の見本が無い（testdata/syntax-samples へ足す）"))
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

    /// `ContextId` の添字は非公開なので、文脈名 → 添字を直列化の鏡で読む
    #[derive(Deserialize)]
    struct IdMirror {
        _syntax_index: usize,
        context_index: usize,
    }

    #[derive(Deserialize)]
    struct NamedContexts {
        context_ids: BTreeMap<String, IdMirror>,
        contexts: Vec<Context>,
    }

    /// 構文名 → (文脈名, 何番目の正規表現) → 正規表現の文字列
    /// （`into_builder` は全構文を組み直すので使わず、その構文の文脈だけ解く）
    fn regexes(set: &SyntaxSet, syntax_name: &str) -> BTreeMap<(String, usize), String> {
        let mirror: SetMirror = bincode::deserialize(&bincode::serialize(set).unwrap()).unwrap();
        let syntax = mirror
            .syntaxes
            .iter()
            .find(|syntax| syntax.name == syntax_name)
            .expect("構文がある");
        let named: NamedContexts =
            syntect::dumps::from_reader(&syntax.serialized_lazy_contexts[..]).unwrap();
        let mut out = BTreeMap::new();
        for (name, id) in &named.context_ids {
            let matches = named.contexts[id.context_index]
                .patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    Pattern::Match(matcher) => Some(matcher.regex.regex_str().to_string()),
                    Pattern::Include(_) => None,
                });
            for (index, pattern) in matches.enumerate() {
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
    fn 対象の構文はすべて見本を持ち土台にある() {
        let base = base();
        for name in target_syntaxes() {
            sample(name);
            assert!(
                base.find_syntax_by_name(name).is_some(),
                "{name} が土台に無い"
            );
        }
        assert!(
            target_syntaxes()
                .iter()
                .all(|name| !["JSON", "TOML"].contains(name)),
            "JSON / TOML は VM で走る正規表現がほぼ無く、1 MB でも速くならない（#1935 の表）"
        );
    }

    /// ビルド時の生成物（`OUT_DIR`）が、今のコードと今の two-face で作り直したものと
    /// 1 バイトも違わない = 生成は入力だけで決まり（再現性）、古い生成物を使っていない
    #[test]
    fn ビルド時の生成物は今のコードで作り直したものと1バイトも違わない() {
        let again = crate::syntax_rewrite::generate(
            &two_face::syntax::extra_newlines(),
            &target_syntaxes(),
        )
        .expect("作り直せる");
        assert!(
            again == PREFILTERED,
            "build.rs の生成物（{} B）と作り直したもの（{} B）が違う",
            PREFILTERED.len(),
            again.len()
        );
        let generated: Vec<PrefilteredSyntax> = bincode::deserialize(PREFILTERED).unwrap();
        let names: Vec<&str> = generated.iter().map(|one| one.name.as_str()).collect();
        assert_eq!(
            names,
            target_syntaxes(),
            "生成物の構文の並びが対象の表と違う"
        );
    }

    #[test]
    fn 前置き付きの塗りは対象の全言語で元と一致する() {
        let (before, after) = (base(), prefiltered());
        let mut cases: Vec<(&str, &str)> = SAMPLES.to_vec();
        // TSX は TS の見本も塗る（#1917 からの組）
        cases.push(("TypeScriptReact", sample("TypeScript")));
        for (syntax, text) in cases {
            let (old, new) = (
                painted(&before, syntax, text),
                painted(&after, syntax, text),
            );
            assert_eq!(old.len(), new.len());
            for (line, (old, new)) in old.iter().zip(&new).enumerate() {
                assert_eq!(old, new, "{syntax} の {} 行目の塗りが変わった", line + 1);
            }
        }
    }

    /// 書き換えた正規表現がすべてコンパイルでき（syntect はコンパイルの失敗で panic する）、
    /// 見本の全行 × 4 つの開始位置で、元と同じ位置・同じグループで一致する
    #[test]
    fn 書き換えた正規表現は元と同じ位置と同じグループで一致する() {
        let (before_set, after_set) = (base(), prefiltered());
        let compile = |re: &str| {
            fancy_regex::RegexBuilder::new(re)
                .oniguruma_mode(true)
                .build()
                .unwrap_or_else(|e| panic!("コンパイルできない: {e}\n{re}"))
        };
        let mut compared = 0;
        for syntax in target_syntaxes() {
            let (before, after) = (regexes(&before_set, syntax), regexes(&after_set, syntax));
            assert_eq!(
                before.keys().collect::<Vec<_>>(),
                after.keys().collect::<Vec<_>>(),
                "{syntax} の文脈の並びが変わった"
            );
            let mut corpus: Vec<&str> = LinesWithEndings::from(sample(syntax)).collect();
            if syntax == "TypeScriptReact" {
                corpus.extend(LinesWithEndings::from(sample("TypeScript")));
            }
            let mut rewritten = 0;
            for (key, original) in &before {
                let guarded = &after[key];
                if guarded == original {
                    continue;
                }
                rewritten += 1;
                let (old, new) = (compile(original), compile(guarded));
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
                            groups(&old),
                            groups(&new),
                            "{syntax}::{}[{}] の一致が変わった（{start} から {line:?}）\n元: {original}\n後: {guarded}",
                            key.0,
                            key.1
                        );
                        compared += 1;
                    }
                }
            }
            assert!(
                rewritten > 0,
                "{syntax} の正規表現が 1 本も書き換わっていない"
            );
        }
        assert!(compared > 200_000, "比べた探索が少なすぎる: {compared}");
    }

    #[test]
    fn 対象外の構文は1バイトも変えない() {
        let built = built_ok();
        assert!(
            built.skipped.is_empty(),
            "差し込めなかった構文: {:?}",
            built.skipped
        );
        let original: SetMirror = bincode::deserialize(&built.plain).unwrap();
        let rewritten: SetMirror = bincode::deserialize(&built.bytes).unwrap();
        let from_base: SetMirror =
            bincode::deserialize(&bincode::serialize(&base()).unwrap()).unwrap();
        assert_eq!(original.syntaxes.len(), rewritten.syntaxes.len());
        assert_eq!(original.syntaxes.len(), from_base.syntaxes.len());
        assert_eq!(original.path_syntaxes, rewritten.path_syntaxes);
        let mut touched = Vec::new();
        for ((old, new), plain) in original
            .syntaxes
            .iter()
            .zip(&rewritten.syntaxes)
            .zip(&from_base.syntaxes)
        {
            assert_eq!(old.name, new.name);
            assert_eq!(old.file_extensions, new.file_extensions);
            assert_eq!(old.variables, new.variables);
            // 小さい入力用の構文セットは土台そのもの
            assert!(
                old.serialized_lazy_contexts == plain.serialized_lazy_contexts,
                "{}",
                old.name
            );
            if old.serialized_lazy_contexts != new.serialized_lazy_contexts {
                touched.push(old.name.as_str());
            }
        }
        let mut targets = target_syntaxes();
        targets.sort_unstable();
        touched.sort_unstable();
        assert_eq!(touched, targets, "前置きを差し込んだのは対象の構文だけ");
    }

    /// 生成したときの土台と違う文脈（two-face の版のずれ）には差し込まず、元の正規表現のまま塗る
    #[test]
    fn 土台の文脈が違う構文は差し込まず元のまま塗る() {
        let mut generated: Vec<PrefilteredSyntax> = bincode::deserialize(PREFILTERED).unwrap();
        let rust = generated.iter_mut().find(|one| one.name == "Rust").unwrap();
        rust.source.fnv ^= 1;
        let tampered = bincode::serialize(&generated).unwrap();
        let base = base();
        let built = build(&base, &tampered).expect("ほかの構文は差し込める");
        assert_eq!(built.skipped, ["Rust"]);
        let set: SyntaxSet = syntect::dumps::from_uncompressed_data(&built.bytes).unwrap();
        assert_eq!(
            regexes(&set, "Rust"),
            regexes(&base, "Rust"),
            "Rust は元の正規表現のまま"
        );
        assert_ne!(
            regexes(&set, "Python"),
            regexes(&base, "Python"),
            "ほかの構文は差し込む"
        );
        assert_eq!(
            painted(&set, "Rust", sample("Rust")),
            painted(&base, "Rust", sample("Rust"))
        );
        // 生成物そのものが読めなければ全体を元へ戻す（呼び手の `syntax_set` が土台を使う）
        assert!(build(&base, b"broken").is_err());
    }

    /// 後方参照などで書き換えられない正規表現は、前置き付きの構文セットでも元の文字列のまま残り、
    /// 同じ構文のほかの正規表現（書き換えたもの）と混ざっても元と同じに塗る（見本の塗りの一致）
    #[test]
    fn 書き換えられない正規表現は元の文字列のまま混ざる() {
        let (before_set, after_set) = (base(), prefiltered());
        let mut kept = Vec::new();
        for syntax in target_syntaxes() {
            let after = regexes(&after_set, syntax);
            for (key, original) in regexes(&before_set, syntax) {
                let Ok(tree) = Expr::parse_tree_with_flags(&original, PARSE_FLAGS) else {
                    continue;
                };
                if runs_on_vm(&tree.expr) && rewrite(&original).is_none() {
                    assert_eq!(after[&key], original, "{syntax}::{}[{}]", key.0, key.1);
                    if !tree.backrefs.is_empty() {
                        kept.push(format!("{syntax}::{}", key.0));
                    }
                }
            }
        }
        // bash / Perl / Ruby のヒアドキュメントの終わり（`\1`）など
        assert!(
            kept.iter()
                .any(|at| at.starts_with("Bourne Again Shell (bash)::")),
            "後方参照を持つ正規表現が見つからない: {kept:?}"
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
        let built = built_ok();
        let (mut on_vm, mut guarded) = (0, 0);
        for syntax in target_syntaxes() {
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
            "[1935] 対象 {} 構文の正規表現 {} 本 / VM 行き {on_vm} 本 / 書き換え {guarded} 本（構文セット上 {} 本）",
            TARGETS.len(),
            built.patterns,
            built.rewritten
        );
        assert_eq!(
            guarded, built.rewritten,
            "構文セットの書き換えと 1 本ずつの書き換えが食い違う"
        );
        // 書けないのは行末・行頭の錨や直前の文字だけを見る幅 0 の正規表現（文脈を抜ける側）と
        // 後方参照を持つもの。2026-10-31 時点で 2,214 / 2,473
        assert!(
            guarded * 10 >= on_vm * 7,
            "VM で走る正規表現のうち書き換えたのが 7 割未満: {guarded} / {on_vm}"
        );
    }

    /// 同じ構文セットを 2 つのスレッドから同時に初めて取っても、差し込みは 1 回で、
    /// どちらも同じ構文セットで同じ塗りになる
    #[test]
    fn 同じ構文セットを2つのスレッドから同時に初めて取っても組み立ては1回() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Barrier;
        let cell: OnceLock<Result<Built, String>> = OnceLock::new();
        let builds = AtomicUsize::new(0);
        let gate = Barrier::new(2);
        let base = base();
        let results: Vec<Vec<String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        gate.wait();
                        let built = cell.get_or_init(|| {
                            builds.fetch_add(1, Ordering::SeqCst);
                            build(&base, PREFILTERED)
                        });
                        let set: SyntaxSet = syntect::dumps::from_uncompressed_data(
                            &built.as_ref().expect("組み立てられる").bytes,
                        )
                        .unwrap();
                        painted(&set, "TypeScript", sample("TypeScript"))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(builds.load(Ordering::SeqCst), 1, "差し込みが二重に走った");
        assert_eq!(results[0], results[1]);
        assert_eq!(
            results[0],
            painted(&base, "TypeScript", sample("TypeScript"))
        );
    }

    /// 小さい入力用の構文セットは元の正規表現・大きい入力用は前置き付き
    #[test]
    fn 小さい入力は元の正規表現で大きい入力は前置き付き() {
        assert!(
            split_by_size(),
            "TAKO_1917_LEGACY / TAKO_1935_LEGACY が立っている"
        );
        let (plain, guarded) = (plain_set(), syntax_set());
        for syntax in ["TypeScript", "Rust"] {
            assert_eq!(
                regexes(&plain, syntax),
                regexes(&base(), syntax),
                "{syntax}"
            );
            assert_ne!(
                regexes(&guarded, syntax),
                regexes(&base(), syntax),
                "{syntax}"
            );
        }
        // TS だけ前置き付き（Svelte / Vue の大きい入力）= TS / TSX は全部と同じ・ほかは元のまま
        let ts_only = ts_only_set();
        for syntax in ["TypeScript", "TypeScriptReact"] {
            assert_eq!(
                regexes(&ts_only, syntax),
                regexes(&guarded, syntax),
                "{syntax}"
            );
        }
        for syntax in ["JavaScript", "CSS", "Rust"] {
            assert_eq!(
                regexes(&ts_only, syntax),
                regexes(&base(), syntax),
                "{syntax}"
            );
        }
        assert_eq!(variant_for("Svelte", 51), Variant::TsOnly);
        assert_eq!(variant_for("Svelte", 50), Variant::Plain);
        assert_eq!(variant_for("HTML", 10_001), Variant::All);
        assert_eq!(variant_for("HTML", 10_000), Variant::Plain);
    }

    /// 元の正規表現で塗る行数は構文ごと（損益分岐の実測。TS は #1917 から全入力を前置き付きで
    /// 塗っていたので損益分岐ちょうどの 50 行・JS / CSS を埋め込む HTML と対象外は 10,000 行・
    /// TS を埋め込む Svelte / Vue は TS と同じ）
    #[test]
    fn 元の正規表現で塗る行数は構文ごとの損益分岐() {
        assert_eq!(plain_max_lines("TypeScript"), 50);
        assert_eq!(plain_max_lines("TypeScriptReact"), 50);
        assert_eq!(plain_max_lines("Rust"), 400);
        assert_eq!(plain_max_lines("CSS"), 1_600);
        assert_eq!(plain_max_lines("HTML"), 10_000);
        assert_eq!(plain_max_lines("Vue Component"), 50);
        assert_eq!(plain_max_lines("Svelte"), 50);
        assert_eq!(plain_max_lines("JSON"), OTHER_PLAIN_MAX_LINES);
        assert_eq!(plain_max_lines("PHP Source"), OTHER_PLAIN_MAX_LINES);
        let mut names = target_syntaxes();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TARGETS.len(), "対象の表に同じ構文が 2 行ある");
        for target in TARGETS {
            assert!(
                (50..=OTHER_PLAIN_MAX_LINES).contains(&target.plain_max_lines),
                "{} の行数 {} が計測の範囲（50〜10,000 行）の外",
                target.name,
                target.plain_max_lines
            );
        }
    }

    /// 対象外で TS / TSX を埋め込む構文は [`TS_EMBEDDERS`]（と文書の中のコード片として持つ Typst）だけ。
    /// two-face が TS を埋め込む構文を足したら、#1917 の挙動（中の TS を前置き付きで塗る）を
    /// 保つかを決めて表へ足すこと
    #[test]
    fn tsを埋め込む対象外の構文は表にある() {
        let set: SetMirror = bincode::deserialize(&bincode::serialize(&base()).unwrap()).unwrap();
        let ts: Vec<usize> = ["TypeScript", "TypeScriptReact"]
            .iter()
            .map(|name| set.syntaxes.iter().position(|s| s.name == *name).unwrap())
            .collect();
        let mut embedders = Vec::new();
        for (at, syntax) in set.syntaxes.iter().enumerate() {
            if ts.contains(&at) || target_syntaxes().contains(&syntax.name.as_str()) {
                continue;
            }
            let lazy: LazyMirror =
                syntect::dumps::from_reader(&syntax.serialized_lazy_contexts[..]).unwrap();
            let text = format!("{:?}", lazy.contexts);
            if ts
                .iter()
                .any(|index| text.contains(&format!("syntax_index: {index},")))
            {
                embedders.push(syntax.name.as_str());
            }
        }
        embedders.sort_unstable();
        let mut expected: Vec<&str> = TS_EMBEDDERS.to_vec();
        expected.push("Typst");
        expected.sort_unstable();
        assert_eq!(embedders, expected);
    }

    /// 塗りの所要の計測（release で手動実行する。#1917 / #1935）。
    /// `TAKO_1917_PERF_FILES=a.ts:b.rs` の各ファイルを、アプリと同じ `preview::highlight_text` で
    /// 2 回ずつ塗る（区切りは OS の PATH と同じ = Windows は `;`。1 回目は文脈の展開と
    /// 正規表現のコンパイルを含む）。
    /// `TAKO_1917_LEGACY=1` / `TAKO_1935_LEGACY=1` を付けると書き換え前 / #1935 より前の構文セットで
    /// 同じことをする。`TAKO_1917_PERF_COMPARE=1` を付けると、塗った構文の全記録（スコープ操作と色）を
    /// 書き換え前の構文セットと 1 行ずつ比べる
    #[test]
    #[ignore = "release で手動実行する計測（#1917 / #1935）"]
    fn perf_1917_塗りの所要() {
        use std::time::Instant;
        let files = std::env::var("TAKO_1917_PERF_FILES").expect("TAKO_1917_PERF_FILES が要る");
        let t = Instant::now();
        let size = built()
            .as_ref()
            .map(|built| (built.bytes.len(), built.plain.len()));
        eprintln!(
            "[perf1917] legacy={} legacy_1935={} 差し込みの構築 {:?}（使い回すバイト列 前置き付き / 元 {size:?}・生成物 {} B）",
            legacy(),
            legacy_1935(),
            t.elapsed(),
            PREFILTERED.len()
        );
        // TS だけ前置き付きのバイト列も先に作る（アプリでは Svelte / Vue の大きい入力の初回に
        // 1 回だけ作る。前後で同じ条件にして、作る費用は別に出す）
        let t = Instant::now();
        let ts_only = ts_only_bytes().as_ref().map(Vec::len);
        eprintln!(
            "[perf1917] TS だけ前置き付きの差し込み {:?}（{ts_only:?}）",
            t.elapsed()
        );
        let t = Instant::now();
        drop(syntax_set());
        eprintln!("[perf1917] 構文セットの取得 {:?}", t.elapsed());
        for path in std::env::split_paths(&files) {
            let path = path.as_path();
            let file = path.display();
            let text = std::fs::read_to_string(path).expect("読める");
            let mut syntax = String::new();
            for round in 1..=2 {
                let t = Instant::now();
                let (lines, name) = crate::preview::highlight_text_with_syntax(path, &text);
                eprintln!(
                    "[perf1917] {} {} バイト {} 行 {round} 回目 {:.3} 秒（{name}）",
                    path.file_name().unwrap().to_string_lossy(),
                    text.len(),
                    lines.len(),
                    t.elapsed().as_secs_f64()
                );
                syntax = name;
            }
            if std::env::var_os("TAKO_1917_PERF_COMPARE").is_some() {
                let set = match variant_for(&syntax, text.lines().count()) {
                    Variant::Plain => plain_set(),
                    Variant::TsOnly => ts_only_set(),
                    Variant::All => syntax_set(),
                };
                let (before, after) = (
                    painted(&base(), &syntax, &text),
                    painted(&set, &syntax, &text),
                );
                let differ = before.iter().zip(&after).position(|(old, new)| old != new);
                assert_eq!(differ, None, "{file} の塗りが変わった（行 {differ:?}）");
                assert_eq!(before.len(), after.len());
                eprintln!(
                    "[perf1917] {file}: {syntax} の塗りの全記録 {} 行が前後で一致",
                    before.len()
                );
            }
        }
    }

    /// 言語ごとの効果の下調べ（#1935 段 1。release で手動実行する）。
    ///
    /// - `TAKO_1935_SURVEY_ALL=1`: 全構文の正規表現の数・VM 行き・書き換えた数・書き換えの費用
    /// - `TAKO_1935_SURVEY_FILES=<表>`: 表の各行 `構文名<TAB>入力のパス` を、書き換え前の構文セットと
    ///   その構文だけ書き換えた構文セットで交互に塗る（`TAKO_1935_ROUNDS` 回。既定 3）。
    ///   冷えた 1 回目（毎回作り直した構文セット）・2 回目・冷えた 40 行の所要を並べる
    #[test]
    #[ignore = "release で手動実行する計測（#1935）"]
    fn perf_1935_言語ごとの効果() {
        use std::time::{Duration, Instant};
        let base = base();
        let base_bytes = bincode::serialize(&base).unwrap();
        if std::env::var_os("TAKO_1935_SURVEY_ALL").is_some() {
            let set: SetMirror = bincode::deserialize(&base_bytes).unwrap();
            for syntax in &set.syntaxes {
                let lazy: LazyMirror =
                    syntect::dumps::from_reader(&syntax.serialized_lazy_contexts[..]).unwrap();
                let on_vm = lazy
                    .contexts
                    .iter()
                    .flat_map(|context| &context.patterns)
                    .filter_map(|pattern| match pattern {
                        Pattern::Match(matcher) => Some(matcher.regex.regex_str()),
                        Pattern::Include(_) => None,
                    })
                    .filter(|re| {
                        Expr::parse_tree_with_flags(re, PARSE_FLAGS)
                            .is_ok_and(|tree| runs_on_vm(&tree.expr))
                    })
                    .count();
                // 書き換えの費用: 1 回目（構文ごとの memo は空）と 3 回の中央値
                let mut costs = Vec::new();
                let mut one = None;
                for _ in 0..3 {
                    let t = Instant::now();
                    let mut memo = std::collections::HashMap::new();
                    one = Some(
                        crate::syntax_rewrite::rewrite_syntax(
                            &syntax.name,
                            &syntax.serialized_lazy_contexts,
                            &mut memo,
                        )
                        .unwrap(),
                    );
                    costs.push(t.elapsed());
                }
                let one = one.unwrap();
                let first = costs[0];
                costs.sort();
                eprintln!(
                    "[survey1935]\t{}\t{}\t{on_vm}\t{}\t{:.2}\t{:.2}\t{}",
                    syntax.name,
                    one.patterns,
                    one.rewritten,
                    first.as_secs_f64() * 1e3,
                    costs[1].as_secs_f64() * 1e3,
                    one.contexts.len() as i64 - syntax.serialized_lazy_contexts.len() as i64
                );
            }
        }
        // 正規表現のコンパイルの費用（冷えた 1 回目の増分の内訳）: 元 / 書き換え後 / 前置きだけ
        if let Ok(names) = std::env::var("TAKO_1935_COMPILE") {
            let compile = |re: &str| {
                let t = Instant::now();
                std::hint::black_box(
                    fancy_regex::RegexBuilder::new(re)
                        .oniguruma_mode(true)
                        .build()
                        .expect("コンパイルできる"),
                );
                t.elapsed()
            };
            let best = |re: &str| (0..5).map(|_| compile(re)).min().unwrap();
            for name in names.split(',') {
                let (mut original, mut guarded, mut guard_only, mut count) =
                    (Duration::ZERO, Duration::ZERO, Duration::ZERO, 0);
                let mut guard_len = 0;
                for re in regexes(&base, name).values() {
                    let Some(rewritten) = rewrite(re) else {
                        continue;
                    };
                    let guard = &rewritten
                        ["\\G(?=(?s:.)*?(?:".len()..rewritten.find("))(?s:.)*?\\K").unwrap()];
                    original += best(re);
                    guarded += best(&rewritten);
                    guard_only += best(&format!("(?=(?s:.)*?(?:{guard}))"));
                    guard_len += guard.len();
                    count += 1;
                }
                eprintln!(
                    "[compile1935]\t{name}\t{count} 本\t元 {:.2} ms\t後 {:.2} ms\t前置きだけ {:.2} ms\t前置きの平均 {} 文字",
                    original.as_secs_f64() * 1e3,
                    guarded.as_secs_f64() * 1e3,
                    guard_only.as_secs_f64() * 1e3,
                    guard_len / count.max(1)
                );
            }
        }
        // 対象の構文を埋め込む構文（埋め込み先の前置きのコンパイル費が損益分岐を後ろへずらす）
        if std::env::var_os("TAKO_1935_EMBEDS").is_some() {
            let set: SetMirror = bincode::deserialize(&base_bytes).unwrap();
            let targets: Vec<(usize, &str)> = TARGETS
                .iter()
                .filter_map(|target| {
                    let at = set.syntaxes.iter().position(|s| s.name == target.name)?;
                    Some((at, target.name))
                })
                .collect();
            for (at, syntax) in set.syntaxes.iter().enumerate() {
                let lazy: LazyMirror =
                    syntect::dumps::from_reader(&syntax.serialized_lazy_contexts[..]).unwrap();
                let text = format!("{:?}", lazy.contexts);
                let embeds: Vec<&str> = targets
                    .iter()
                    .filter(|(index, _)| {
                        *index != at && text.contains(&format!("syntax_index: {index},"))
                    })
                    .map(|(_, name)| *name)
                    .collect();
                if !embeds.is_empty() {
                    eprintln!("[embeds1935]\t{}\t{}", syntax.name, embeds.join(","));
                }
            }
        }
        let Some(table) = std::env::var_os("TAKO_1935_SURVEY_FILES") else {
            return;
        };
        let rounds: usize = std::env::var("TAKO_1935_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        let theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-eighties.dark")
            .unwrap();
        let paint = |set: &SyntaxSet, name: &str, text: &str| -> Duration {
            let syntax = set.find_syntax_by_name(name).expect("構文がある");
            let t = Instant::now();
            let mut lines = HighlightLines::new(syntax, &theme);
            for line in LinesWithEndings::from(text) {
                std::hint::black_box(lines.highlight_line(line, set).expect("塗れる"));
            }
            t.elapsed()
        };
        let median = |mut v: Vec<Duration>| {
            v.sort();
            v[v.len() / 2].as_secs_f64()
        };
        for row in std::fs::read_to_string(table).unwrap().lines() {
            let Some((name, path)) = row.split_once('\t') else {
                continue;
            };
            let text = std::fs::read_to_string(path).unwrap();
            let small: String = text.lines().take(40).map(|l| format!("{l}\n")).collect();
            let generated = crate::syntax_rewrite::generate(&base, &[name]).unwrap();
            let built = build(&base, &generated).unwrap();
            // 損益分岐: 先頭 n 行を、毎回作り直した（冷えた）構文セットで塗る
            if let Ok(sizes) = std::env::var("TAKO_1935_CROSS") {
                let mut cells = Vec::new();
                for n in sizes.split(',').filter_map(|n| n.parse::<usize>().ok()) {
                    let head: String = text.lines().take(n).map(|l| format!("{l}\n")).collect();
                    let mut samples = [Vec::new(), Vec::new()];
                    for round in 0..rounds {
                        let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
                        for variant in order {
                            let bytes = if variant == 0 {
                                &base_bytes
                            } else {
                                &built.bytes
                            };
                            let set: SyntaxSet =
                                syntect::dumps::from_uncompressed_data(bytes).unwrap();
                            samples[variant].push(paint(&set, name, &head));
                        }
                    }
                    let [before, after] = samples.map(median);
                    cells.push(format!("{n} 行 {:.1}→{:.1}", before * 1e3, after * 1e3));
                }
                eprintln!("[cross1935]\t{name}\t{}", cells.join("\t"));
                continue;
            }
            // [書き換え前, 書き換え後] × [冷えた 1 回目, 2 回目, 冷えた 40 行]
            let mut samples = vec![vec![Vec::new(); 3]; 2];
            for round in 0..rounds {
                let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
                for variant in order {
                    let bytes = if variant == 0 {
                        &base_bytes
                    } else {
                        &built.bytes
                    };
                    let set: SyntaxSet = syntect::dumps::from_uncompressed_data(bytes).unwrap();
                    samples[variant][0].push(paint(&set, name, &text));
                    samples[variant][1].push(paint(&set, name, &text));
                    let set: SyntaxSet = syntect::dumps::from_uncompressed_data(bytes).unwrap();
                    samples[variant][2].push(paint(&set, name, &small));
                }
            }
            let [before, after] = [&samples[0], &samples[1]]
                .map(|s| s.iter().map(|v| median(v.clone())).collect::<Vec<f64>>());
            eprintln!(
                "[files1935]\t{name}\t{} B\t{} 行\t書き換え {}/{}\t1 回目 {:.3} → {:.3} 秒\t2 回目 {:.3} → {:.3} 秒\t40 行 {:.1} → {:.1} ms",
                text.len(),
                text.lines().count(),
                built.rewritten,
                built.patterns,
                before[0],
                after[0],
                before[1],
                after[1],
                before[2] * 1e3,
                after[2] * 1e3,
            );
            if std::env::var_os("TAKO_1935_COMPARE").is_some() {
                let after_set: SyntaxSet =
                    syntect::dumps::from_uncompressed_data(&built.bytes).unwrap();
                let (old, new) = (
                    painted(&base, name, &text),
                    painted(&after_set, name, &text),
                );
                let differ = old.iter().zip(&new).position(|(o, n)| o != n);
                assert_eq!(differ, None, "{name} の塗りが変わった（行 {differ:?}）");
                assert_eq!(old.len(), new.len());
                eprintln!(
                    "[files1935]\t{name}\t塗りの全記録 {} 行が前後で一致",
                    old.len()
                );
            }
        }
    }
}
