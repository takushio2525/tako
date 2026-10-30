//! syntect 5 の構文セットの直列化の鏡と、ビルド時に書き換えた文脈の受け渡しの形（#1917 / #1935）
//!
//! `build.rs` も `#[path]` でこのファイルを読む（ビルド時に書き換えた文脈を作る側と、
//! 起動時に差し込む側 = [`crate::syntax_prefilter`] が同じ形を引く）。
//! このファイルは `crate::` のほかのモジュールを参照しない。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use syntect::parsing::syntax_definition::{Context, ContextId};
use syntect::parsing::Scope;

/// 塗りの正規表現へ前置きを足す構文
pub(crate) struct Target {
    /// two-face の構文名
    pub(crate) name: &'static str,
    /// この行数以下の入力は元の正規表現で塗る（前置きのコンパイルの固定費を探索で取り返せない
    /// 行数 = 冷えた構文セットで先頭 n 行を塗る所要の損益分岐。#1935）
    pub(crate) plain_max_lines: usize,
}

/// 塗りの正規表現へ前置きを足す構文。1 MB の入力の塗りの所要を言語ごとに測り、15% 以上かつ
/// 50 ms 以上短くなるものを選んだ（JSON / TOML は VM で走る正規表現がほぼ無く変わらないので外した）。
/// `plain_max_lines` は、TS / TSX（#1917 から全入力を前置き付きで塗っていた）は損益分岐ちょうど、
/// ほかは損益分岐の計測で前置き付きがはっきり速くなった最初の行数（それ以下は #1935 より前と同じ
/// 元の正規表現で塗る = どの大きさでも遅くならない）。表は `.agent/architecture.md`
/// 「構文の塗りの正規表現の前置き（#1917 / #1935）」。
/// 足すときは `testdata/syntax-samples/` に見本を置く（塗りの全記録の一致を単体テストが見る）
pub(crate) const TARGETS: &[Target] = &[
    // #1917
    target("TypeScript", 50),
    target("TypeScriptReact", 50),
    // #1935
    target("JavaScript", 400),
    target("C#", 400),
    target("C++", 800),
    target("Ruby", 400),
    target("Perl", 1_600),
    target("Python", 800),
    target("Go", 400),
    target("C", 800),
    target("Swift", 400),
    target("Rust", 400),
    target("Java", 800),
    target("Lua", 800),
    target("SQL", 800),
    target("CSS", 1_600),
    target("Bourne Again Shell (bash)", 800),
    target("YAML", 200),
    target("Markdown", 800),
    target("HTML", 1_000),
    target("Kotlin", 800),
];

const fn target(name: &'static str, plain_max_lines: usize) -> Target {
    Target {
        name,
        plain_max_lines,
    }
}

/// #1935 より前の対象（`TAKO_1935_LEGACY=1` の A/B で残す）
pub(crate) const LEGACY_1917_SYNTAXES: [&str; 2] = ["TypeScript", "TypeScriptReact"];

/// `SyntaxSet`（`first_line_cache` / `metadata` は直列化されない）。
/// two-face に無い構文を末尾へ足すとき（`syntax_extra`。#1954）もこの鏡を使う
#[derive(Serialize, Deserialize)]
pub(crate) struct SetMirror {
    pub(crate) syntaxes: Vec<ReferenceMirror>,
    pub(crate) path_syntaxes: Vec<(String, usize)>,
}

/// `SyntaxReference`（`lazy_contexts` は直列化されない。`variables` は鍵の順に並ぶ）
#[derive(Serialize, Deserialize)]
pub(crate) struct ReferenceMirror {
    pub(crate) name: String,
    pub(crate) file_extensions: Vec<String>,
    pub(crate) scope: Scope,
    pub(crate) first_line_match: Option<String>,
    pub(crate) hidden: bool,
    pub(crate) variables: BTreeMap<String, String>,
    pub(crate) serialized_lazy_contexts: Vec<u8>,
}

/// `LazyContexts`（zlib で圧縮した bincode として `serialized_lazy_contexts` に入っている）
#[derive(Serialize, Deserialize, PartialEq)]
pub(crate) struct LazyMirror {
    pub(crate) context_ids: BTreeMap<String, ContextId>,
    pub(crate) contexts: Vec<Context>,
}

/// ビルド時に書き換えた 1 構文の文脈（`build.rs` が `Vec<PrefilteredSyntax>` を bincode で書く）
#[derive(Serialize, Deserialize)]
pub(crate) struct PrefilteredSyntax {
    pub(crate) name: String,
    /// 書き換えの元にした文脈（`serialized_lazy_contexts`）の指紋。起動時の土台の文脈と
    /// 一致したときだけ差し込む（版のずれた two-face に別の文脈を差し込まない）
    pub(crate) source: Fingerprint,
    /// 書き換えた文脈（`serialized_lazy_contexts` と同じ形 = zlib で圧縮した bincode）
    pub(crate) contexts: Vec<u8>,
    /// この構文の正規表現（`Pattern::Match`）の数
    pub(crate) patterns: u32,
    /// そのうち前置きを足した数
    pub(crate) rewritten: u32,
}

/// バイト列の指紋（長さ + FNV-1a 64 ビット。改ざんではなく取り違えを見分ける用途）
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) struct Fingerprint {
    pub(crate) len: u64,
    pub(crate) fnv: u64,
}

impl Fingerprint {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        let fnv = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        Self {
            len: bytes.len() as u64,
            fnv,
        }
    }
}
