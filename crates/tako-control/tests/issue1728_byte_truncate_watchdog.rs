//! **#1728 の番犬**: `preview_render.rs` の本番コードに、文字列をバイト位置で切る
//! `&<式>[..N]` が無い。
//!
//! ## なぜ止めるのか
//!
//! `&s[..N]` の `N` は**バイト位置**なので、多バイト文字の途中に当たると panic する
//! （`end byte index 40 is not a char boundary; it is inside '析'`）。`preview_render.rs` は
//! 描画のたびに走るので、panic は GPUI の描画中 = **アプリごと落ちる**。
//!
//! 実物（#1728）: Code Runner のツールチップが `&cmd[..60]`、実行メニューが
//! `&plan.command[..40]` を取っていて、`python3 'report_最終版_データ解析結果まとめ_v2.py'`
//! のような日本語のファイル名で落ちる形だった（修正前の単体テストで panic を実測）。
//! 同じファイルの検索欄も `&text[..cursor]` で分けていて、`tako preview-search` が
//! クエリだけ差し替えるとカーソルが文字の途中を指したまま残る。
//!
//! ## 何を固定するか
//!
//! 1. 本番コードに**先頭からの範囲添字**（`[..<式>]` / `[..=<式>]`）が無い。
//!    `N` がリテラルでも変数でも落とす（`&cmd[..max]` へ書き換えても危険は同じ）。
//!    表示の切り詰めは `crate::truncate`（文字数）、分割は `str::floor_char_boundary` で
//!    丸めてから `split_at` する。`Vec` のスライスも同じ字面なので巻き込むが、
//!    このファイルでは 0 件なので、要るときは `.get(..n)` / `iter().take(n)` で書く
//! 2. 検索欄の描画（`render_field_with_cursor`）はカーソルを `floor_char_boundary` で
//!    丸めてから分ける（丸めを外すと `split_at` が同じ panic を起こす）
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で本番コードの眺めに対象の関数が残っていることを、
//! [`修正前の形を実際に落とす`] で**修正前ソースの断片が `file:line` で名指しされる**
//! ことを確かめる（注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中の `#[cfg(test)]`（`preview_render.rs` にも 1 つある）で走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const PREVIEW_RENDER: &str = "crates/tako-app/src/preview_render.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの「コードの眺め」（テスト領域・コメント・文字列を空白へ潰す）。
///
/// どちらもバイト長と行番号が保たれるので `file:line` がそのまま使える。文字列を潰すのは、
/// 製品側の注釈や文言に書いた `[..N]` を拾わないため
fn code_of(src: &str, rel: &str) -> String {
    production_range::code_view_of(&production_range::production(src, rel))
}

/// 先頭からの範囲添字（`[..<式>]` / `[..=<式>]`）が居る行（1 始まり）。
///
/// `[..]`（全体）と `[.., last]`（スライスパターン）は切り詰めではないので数えない
fn byte_prefix_slices(code: &str) -> Vec<usize> {
    const HEAD: &str = "[..";
    let mut hits = Vec::new();
    let mut from = 0usize;
    while let Some(at) = code[from..].find(HEAD) {
        let at = from + at;
        from = at + HEAD.len();
        let next = code[from..].trim_start().chars().next();
        if matches!(next, Some(']') | Some(',')) {
            continue;
        }
        hits.push(code[..at].matches('\n').count() + 1);
    }
    hits
}

/// 関数の窓（宣言行の 1 始まりの行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(code: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = code.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.trim_end() == close)
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。
/// 違反は `file:line — 理由` の形で返す
fn offenders(raw: &str, code: &str, rel: &str) -> Vec<String> {
    let raw_lines: Vec<&str> = raw.lines().collect();
    let mut out: Vec<String> = byte_prefix_slices(code)
        .into_iter()
        .map(|line| {
            let text = raw_lines.get(line - 1).map(|l| l.trim()).unwrap_or("");
            format!(
                "{rel}:{line} — 文字列をバイト位置で切っている（`{text}`）。多バイト文字の途中に\
                 当たると描画中に panic してアプリごと落ちる（#1728）。表示の切り詰めは \
                 `crate::truncate`（文字数）、分割は `floor_char_boundary` で丸めてから `split_at`"
            )
        })
        .collect();

    match fn_window(code, "fn render_field_with_cursor(") {
        None => out.push(format!(
            "{rel}:0 — `render_field_with_cursor` が見つからない（走査が空振り）"
        )),
        Some((at, window)) if !window.contains("floor_char_boundary(") => out.push(format!(
            "{rel}:{at} — 検索欄のカーソルを文字境界へ丸めずに分けている。\
             `tako preview-search` はクエリだけ差し替えるので、カーソルが文字の途中を\
             指したまま残る（#1728）"
        )),
        Some(_) => {}
    }
    out
}

fn assert_clean(raw: &str, code: &str, rel: &str) {
    let found = offenders(raw, code, rel);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

// --------------------------------------------------------------- テスト

#[test]
fn preview_renderにバイト位置の切り詰めが無い() {
    let raw = read(PREVIEW_RENDER);
    let code = code_of(&raw, PREVIEW_RENDER);
    assert_clean(&raw, &code, PREVIEW_RENDER);
}

/// 走査が空振りしていないこと（#1728 が実在した 3 か所が視界に入っているか）。
///
/// ファイル移動・改名や、テスト領域の潰し方の破損で黙って 0 件になるのを防ぐ。
/// **潰したあとの眺め**で確かめる
#[test]
fn 走査が空振りしていない() {
    let raw = read(PREVIEW_RENDER);
    let code = code_of(&raw, PREVIEW_RENDER);
    for (needle, inside) in [
        // ツールチップ（ヘッダの再生ボタン）
        ("let _tooltip: String", "p.command"),
        // 実行メニュー
        ("fn render_run_menu_overlay(", "plan.command"),
        // 検索 / 置換欄
        ("fn render_field_with_cursor(", "ime_text"),
    ] {
        let at = code
            .find(needle)
            .unwrap_or_else(|| panic!("{PREVIEW_RENDER} の本番コードに `{needle}` が無い"));
        assert!(
            code[at..].contains(inside),
            "{PREVIEW_RENDER}: `{needle}` の後ろに `{inside}` が無い（走査先が違う）"
        );
    }
    // 単体テストの中の `text[*start..]` は本番コードではないので潰れている
    let tests = production_range::tests_only(&raw);
    assert!(
        tests.contains("[*start..]") && !code.contains("[*start..]"),
        "テスト領域が潰れていない / テストの眺めが空（#1420 の範囲取りが壊れている）"
    );
}

/// 番犬に検出力があること: **#1728 以前の形を実際に落とす**（`file:line` の名指しまで）
#[test]
fn 修正前の形を実際に落とす() {
    let check = |snippet: &str| {
        let code = production_range::code_view_of(snippet);
        offenders(snippet, &code, "注入.rs")
    };
    // 検索欄は修正後の形にしておき、1 行ずつの注入が名指しされるかだけを見る
    let field_ok = "
fn render_field_with_cursor(text: &str, cursor: usize) -> String {
    let (before, after) = text.split_at(text.floor_char_boundary(cursor));
    format!(\"{before}|{after}\")
}
";
    for (label, injected, line) in [
        (
            "① ツールチップ（#1728 の修正前そのまま）",
            "fn a(cmd: &str) -> String {\n    format!(\"{}…\", &cmd[..60])\n}\n",
            2,
        ),
        (
            "② 実行メニュー（#1728 の修正前そのまま）",
            "fn b() {\n    let x = 1;\n    let s = format!(\"{}…\", &plan.command[..40]);\n}\n",
            3,
        ),
        (
            "③ 変数の上限へ書き換えた形",
            "fn c(cmd: &str, max: usize) -> &str {\n    &cmd[..max]\n}\n",
            2,
        ),
        (
            "④ 定数・閉区間",
            "fn d(cmd: &str) -> &str {\n    &cmd[..=MAX_LEN]\n}\n",
            2,
        ),
    ] {
        let src = format!("{injected}{field_ok}");
        let found = check(&src);
        let want = format!("注入.rs:{line} —");
        assert!(
            found.iter().any(|f| f.starts_with(&want)),
            "{label} を名指しできなかった（`{want}` が無い）: {found:?}"
        );
        assert_eq!(found.len(), 1, "{label} 以外まで拾った: {found:?}");
    }

    // ⑤ 検索欄の丸めを外した形（#1728 の修正前の `cursor.min(text.len())`）
    let field_legacy = "
fn render_field_with_cursor(text: &str, cursor: usize) -> String {
    let (before, after) = text.split_at(cursor.min(text.len()));
    format!(\"{before}|{after}\")
}
";
    let found = check(field_legacy);
    assert!(
        found
            .iter()
            .any(|f| f.starts_with("注入.rs:2 —") && f.contains("丸めずに")),
        "⑤ 検索欄の丸め外しを名指しできなかった: {found:?}"
    );

    // ⑥ 安全な形は通す（常に落ちる無意味な番犬になっていないこと）
    let ok = format!(
        "
fn ok(cmd: &str, xs: &[u8], s: &str) {{
    let a = crate::truncate(cmd, 40);
    let b = &xs[..];
    if let [.., last] = xs {{}}
    let c = s.get(..40);
    let d = \"&cmd[..60] は文字列の中なので数えない\";
    // &cmd[..60] は注釈の中なので数えない
}}
{field_ok}"
    );
    let found = check(&ok);
    assert!(found.is_empty(), "⑥ 安全な形まで落とした: {found:?}");
}
