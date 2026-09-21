//! Rust ソースの行から「関数定義の頭」を読む（番犬テストの共有部品・Issue #1496）
//!
//! 走査型の番犬は、違反行を**囲んでいる関数名**を名指しして報告する。その追跡を
//! `line.trim_start().strip_prefix("fn ")` で書くと **`pub(crate) fn` / `async fn` が
//! 関数の頭に見えない**ので、中の違反が**手前の関数の名前**で報告される。
//!
//! #1496 の実例は、`pub(crate) fn kill_shelved_tab_clicked` の中の違反が、手前にある
//! `fn shelved_tab_groups` の中として報告されたもの。検出そのものは効く（落ちる）が
//! **名指しが嘘になる**ので、読み手が違う関数を探して時間を失う。
//! `main.rs` は実測で `pub(crate) fn` 51 本 / `async fn` 50 本 / `pub fn` 2 本 /
//! `const fn` 1 本を持つので、取りこぼす面は狭くない。
//!
//! # なぜ tako-core に置くか
//!
//! 同じ追跡をする番犬が **tako-app（`src/main.rs` の `#[cfg(test)] mod`）と
//! tako-control（`tests/*.rs`）の両方**にある。tako-control の `tests/common/` へ置くと
//! tako-app から使えず、クレートを跨いだ `#[path = "../../tako-control/tests/…"]` は
//! tako-app のビルドを別クレートのテスト配置に縛る（この形の先例はリポに無い）。
//! **両方が依存している tako-core** に置くのが、実装を 1 つにしたまま両側から引ける
//! 唯一の置き場。重複実装を 2 つ残して「同じ挙動」をテストで縛る形は、
//! 片方だけ直す事故がそのまま残るので採らない。

/// 行が関数定義の頭なら、その関数名を返す。
///
/// 拾う形（いずれも任意・組み合わせ可）:
///
/// - インデント（`    fn foo()`）
/// - 可視性: `pub` / `pub(crate)` / `pub(super)` / `pub(in crate::a::b)`
/// - 前置キーワード: `default` / `const` / `async` / `unsafe` / `extern "ABI"`
///
/// 関数名は `fn` の直後の識別子（`(` / `<` / 空白 / `;` などで終端）。日本語の
/// テスト関数名（`タブcloseの発生源はgui経路だけが名乗る`）も識別子として読む。
///
/// 頭ではない行（コメント・`let f: fn() = …`・`where F: Fn(…)`・`extern crate`）は
/// [`None`]。**複数行を渡されても 1 行目だけを見る**（行頭からソース末尾までの
/// スライスを渡す呼び出し側があるため）。
pub fn fn_head_name(line: &str) -> Option<&str> {
    let line = line.split('\n').next().unwrap_or(line);
    let mut rest = strip_visibility(line.trim_start());
    // 前置キーワードは順不同・複数（`pub const unsafe extern "C" fn`）
    while let Some(next) = strip_modifier(rest) {
        rest = next;
    }
    let after = strip_keyword(rest, "fn")?;
    let end = after
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    (end > 0).then(|| &after[..end])
}

/// 桁 0（インデント無し）の関数定義の頭か。**トップレベル関数だけ**を追う番犬用
/// （入れ子の `fn` を頭と見なすと、中の違反が内側の名前で報告されて面が変わる）。
pub fn is_top_level_fn_head(line: &str) -> bool {
    let first = line.split('\n').next().unwrap_or(line);
    !first.starts_with(|c: char| c.is_whitespace()) && fn_head_name(first).is_some()
}

/// 可視性（`pub` / `pub(crate)` / `pub(super)` / `pub(in crate::a::b)`）を剥がす
fn strip_visibility(s: &str) -> &str {
    let Some(after) = s.strip_prefix("pub") else {
        return s;
    };
    match after.chars().next() {
        Some('(') => match after.find(')') {
            Some(i) => after[i + 1..].trim_start(),
            None => s,
        },
        Some(c) if c.is_whitespace() => after.trim_start(),
        // `public_api` のような識別子の頭を剥がさない
        _ => s,
    }
}

/// 前置キーワードを 1 つ剥がす（剥がせなければ [`None`]）
fn strip_modifier(s: &str) -> Option<&str> {
    for kw in ["default", "const", "async", "unsafe"] {
        if let Some(after) = strip_keyword(s, kw) {
            return Some(after);
        }
    }
    // `extern "C" fn` / `extern fn`（ABI は省略できる）。`extern crate` は
    // このあと `fn` が来ないので結局 `fn_head_name` が None を返す
    strip_keyword(s, "extern").map(strip_abi)
}

/// ABI 文字列（`"C"` / `"system"`）を剥がす
fn strip_abi(s: &str) -> &str {
    let Some(rest) = s.strip_prefix('"') else {
        return s;
    };
    match rest.find('"') {
        Some(i) => rest[i + 1..].trim_start(),
        None => s,
    }
}

/// `kw` + 空白 で始まるなら、その後ろ（先頭の空白を落としたもの）。
/// 空白を要求するので `constant` / `asyncify` のような識別子の頭は剥がさない
fn strip_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let after = s.strip_prefix(kw)?;
    after
        .starts_with(|c: char| c.is_whitespace())
        .then(|| after.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 可視性修飾子つきの関数頭から名前を読む() {
        // #1496 の実例（`pub(crate) fn` が頭に見えず手前の名前で報告された）
        assert_eq!(
            fn_head_name("    pub(crate) fn kill_shelved_tab_clicked(&mut self) {"),
            Some("kill_shelved_tab_clicked")
        );
        for (line, want) in [
            ("fn plain() {", "plain"),
            ("    fn indented(&self) -> bool {", "indented"),
            ("pub fn exported() {", "exported"),
            ("pub(crate) fn crate_local() {", "crate_local"),
            ("    pub(super) fn parent_local() {", "parent_local"),
            ("pub(in crate::a::b) fn scoped() {", "scoped"),
            ("async fn later() {", "later"),
            ("pub async fn exported_later() {", "exported_later"),
            ("const fn compile_time() -> usize {", "compile_time"),
            ("pub(crate) const fn both() -> usize {", "both"),
            ("unsafe fn raw() {", "raw"),
            (
                "    extern \"C\" fn signal_handler(_: i32) {",
                "signal_handler",
            ),
            ("pub unsafe extern \"C\" fn everything() {", "everything"),
            ("default fn specialized() {", "specialized"),
            ("fn generic<T: Copy>(v: T) -> T {", "generic"),
            ("fn declared_only(&self);", "declared_only"),
            ("fn  extra_space() {", "extra_space"),
            // 日本語のテスト関数名（このリポの番犬はこの形で名乗る）
            (
                "    fn タブcloseの発生源はgui経路だけが名乗る() {",
                "タブcloseの発生源はgui経路だけが名乗る",
            ),
        ] {
            assert_eq!(fn_head_name(line), Some(want), "行: {line}");
        }
    }

    #[test]
    fn 関数頭でない行は名前を返さない() {
        for line in [
            "// fn commented_out() {",
            "/// `fn doc_mention()` の説明",
            "    let f: fn() = plain;",
            "where F: Fn(usize) -> bool,",
            "extern crate libc;",
            "pub use crate::x::fn_like;",
            "const LIMIT: usize = 3;",
            "unsafe impl Send for X {}",
            "pub struct Config {",
            "    .map(|s| s.to_string())",
            "",
            "fn ",
        ] {
            assert_eq!(fn_head_name(line), None, "行: {line}");
        }
    }

    #[test]
    fn 複数行を渡されても1行目だけを見る() {
        // 行頭からソース末尾までのスライスを渡す呼び出し側（`enclosing_fn`）がある
        let slice = "pub(crate) fn head() {\n    fn inner() {}\n}\n";
        assert_eq!(fn_head_name(slice), Some("head"));
        assert_eq!(fn_head_name("let x = 1;\npub fn head() {"), None);
    }

    #[test]
    fn トップレベル判定はインデントで分かれる() {
        assert!(is_top_level_fn_head("pub(crate) fn top() {"));
        assert!(is_top_level_fn_head("extern \"C\" fn top_abi() {"));
        assert!(!is_top_level_fn_head("    pub(crate) fn nested() {"));
        assert!(!is_top_level_fn_head("\tfn tabbed() {"));
        assert!(!is_top_level_fn_head("pub struct Config {"));
    }
}
