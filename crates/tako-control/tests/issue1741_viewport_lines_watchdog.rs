//! 可視行数の測り方が 1 実装から外れないことを見張る番犬（#1741）
//!
//! #1649 の追従スクロールは可視行数を「器の高さ − 上下の余白」÷ `theme.line_height`
//! （ターミナルのセル高 = 17px）で、#1652 のページ移動は「器の高さ − 上余白」÷ 描いた行の
//! 高さ（21px）で、**別々に**数えていた。661px の器で実矩形 30 行に対し追従は 37 行と数え、
//! ↓ で進むとカーソルが画面の外へ出た。Page Down の着地は追従の余白に届かず最下段へ
//! 貼り付いた。
//!
//! 直し方は測り方を 1 実装（`preview_row_geometry` → `editor_scroll::visible_rows`）へ
//! 寄せる形。ソース走査で見張るのは次の 4 つ:
//!   1. 追従（`preview_cursor_viewport`）・ページ移動（`run_editor_command_local`）・
//!      IME の見積もり（`preview_pending_cursor_origin`）が可視行数を `preview_row_geometry` から採る
//!   2. それらの本文に `theme.line_height` が無い（セル高で数え直す写しを作らない）
//!   3. `preview_row_geometry` は描いた行の実寸（`preview_text_layouts`）と上余白を
//!      `editor_scroll::visible_rows` へ渡して数える（ここにも `theme.line_height` を入れない）
//!   4. 測り方の実体は 1 つ: `visible_rows(` を製品コードで呼ぶのは `preview_row_geometry` だけで、
//!      旧 API（`LineViewport::from_pixels` / `preview_viewport_lines`）が戻っていない
//!
//! 実際に実矩形と合うか（状態値の関係）は `editor_scroll` の単体テストと
//! visual-test 節 `viewport-lines`（`scripts/test-viewport-lines-1741.sh`）が見る。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

#[path = "common/code_view.rs"]
mod code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";
const CORE: &str = "crates/tako-core/src/editor_scroll.rs";

/// 肯定の存在確認も「無いこと」の確認も、doc コメントの綴りに引きずられないよう
/// コメントを潰した眺めで見る（説明文に `theme.line_height` と書けるようにするため。#1609）
fn read_code(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    code_view::without_comments_checked(&src, rel)
}

/// 製品コードだけの眺め（`#[cfg(test)] mod tests` より前）。単体テストは測り方の
/// 純関数を直接呼んでよいので、規則 4 の数え上げから外す
fn product_part(code: &str) -> &str {
    code.find("#[cfg(test)]\nmod tests")
        .map_or(code, |at| &code[..at])
}

/// `fn <name>(` の本文と、その先頭のバイト位置（行番号を出すのに使う）
fn fn_span<'a>(src: &'a str, name: &str) -> (usize, &'a str) {
    let heads = [
        format!("\n    fn {name}("),
        format!("\n    pub(crate) fn {name}("),
        format!("\n    pub fn {name}("),
        format!("\nfn {name}("),
        format!("\npub(crate) fn {name}("),
        format!("\npub fn {name}("),
    ];
    let at = heads
        .iter()
        .filter_map(|head| src.find(head.as_str()).map(|at| at + 1))
        .min()
        .unwrap_or_else(|| panic!("`fn {name}` が見つからない（#1741）"));
    let rest = &src[at..];
    let body_from = rest.find('\n').map_or(rest.len(), |n| n + 1);
    let end = [
        "\n    fn ",
        "\n    pub fn ",
        "\n    pub(crate) fn ",
        "\nfn ",
        "\npub fn ",
        "\npub(crate) fn ",
    ]
    .iter()
    .filter_map(|next| rest[body_from..].find(next).map(|n| body_from + n))
    .min()
    .unwrap_or(rest.len());
    (at, &rest[..end])
}

fn line_at(src: &str, offset: usize) -> usize {
    src[..offset].matches('\n').count() + 1
}

#[test]
fn 追従とページ移動は同じ測り方から可視行数を採る() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let main = read_code(&root, MAIN);

    for (rel, src, name, 役目) in [
        (
            RENDER,
            &render,
            "preview_cursor_viewport",
            "追従スクロール（#1649）の可視範囲",
        ),
        (
            MAIN,
            &main,
            "run_editor_command_local",
            "ページ移動（#1652）の歩幅",
        ),
        (
            MAIN,
            &main,
            "preview_pending_cursor_origin",
            "IME の 1 フレーム目のアンカーの見積もり",
        ),
    ] {
        let (at, body) = fn_span(src, name);
        assert!(
            body.contains("preview_row_geometry("),
            "{rel}:{} の `{name}`（{役目}）が可視行数を `preview_row_geometry` から採っていない\
             （#1741。追従とページ移動が別々に数えると、追従が「見えている」と判断した行が\
             画面の外にあったり、Page Down の着地が最下段へ貼り付いたりする）",
            line_at(src, at)
        );
        if let Some(bad) = body.find("theme.line_height") {
            panic!(
                "{rel}:{} の `{name}`（{役目}）が `theme.line_height` で数えている（#1741。\
                 あれはターミナルのセル高 13pt × 1.3 = 17px で、コード行は実測 21px + 上余白 14px。\
                 661px の器で実矩形 30 行を 37 行と数え、↓ で進むとカーソルが画面の外へ出た。\
                 可視行数は `preview_row_geometry` の 1 実装から採る）",
                line_at(src, at + bad)
            );
        }
    }
}

#[test]
fn 可視行数は描いた行の実寸から数える() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let (at, body) = fn_span(&render, "preview_row_geometry");

    for (needle, 理由) in [
        (
            "editor_scroll::visible_rows(",
            "数える算術の正本は tako-core の純関数（実ピクセルなしで固定できる）",
        ),
        (
            "preview_text_layouts",
            "1 行の高さは実際に描いた行の実寸（折り返した行はその高さ）",
        ),
        (
            "PREVIEW_BODY_PADDING",
            "先頭行は器の上端から上余白ぶん下に描かれる（数える起点）",
        ),
        (
            "preview_first_visible_line(",
            "数え始めるのは器の先頭可視行（2 種類の器のどちらでも同じ口）",
        ),
    ] {
        assert!(
            body.contains(needle),
            "{RENDER}:{} の `preview_row_geometry` に `{needle}` が無い（#1741。{理由}）",
            line_at(&render, at)
        );
    }
    if let Some(bad) = body.find("theme.line_height") {
        panic!(
            "{RENDER}:{} の `preview_row_geometry` が `theme.line_height` を使っている（#1741。\
             ターミナルのセル高はコード行の高さではない。未描画のときの見積もりは\
             `preview_code_line_height_estimate`（gpui の既定の行高 = φ 倍）を使う）",
            line_at(&render, at + bad)
        );
    }
    let (est_at, estimate) = fn_span(&render, "preview_code_line_height_estimate");
    if let Some(bad) = estimate.find("theme.line_height") {
        panic!(
            "{RENDER}:{} の `preview_code_line_height_estimate` が `theme.line_height` を\
             使っている（#1741。コード行は祖先の文字サイズ × gpui の既定の行高で組まれる）",
            line_at(&render, est_at + bad)
        );
    }
}

#[test]
fn 測り方の実体は一つだけ() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let main = read_code(&root, MAIN);
    let core = read_code(&root, CORE);

    // `visible_rows(` を製品コードで呼ぶのは `preview_row_geometry` だけ
    let (geo_at, geo_body) = fn_span(&render, "preview_row_geometry");
    let geo_range = geo_at..geo_at + geo_body.len();
    for (rel, src) in [(RENDER, &render), (MAIN, &main)] {
        let product = product_part(src);
        for (offset, _) in product.match_indices("visible_rows(") {
            let inside = rel == RENDER && geo_range.contains(&offset);
            assert!(
                inside,
                "{rel}:{} が `visible_rows` を `preview_row_geometry` の外で呼んでいる（#1741。\
                 可視行数の測り方を 2 つ持つと、追従とページ移動が別の値で動く）",
                line_at(src, offset)
            );
        }
    }

    // 旧 API が戻っていない（器の高さを固定の行高で割る写し）
    for (rel, src, needle, 旧) in [
        (
            CORE,
            &core,
            "fn from_pixels(",
            "`LineViewport::from_pixels`（器の高さ ÷ 1 行の高さ）",
        ),
        (
            MAIN,
            &main,
            "fn preview_viewport_lines(",
            "`preview_viewport_lines`（ページ移動だけが持っていた別の測り方）",
        ),
        (
            RENDER,
            &render,
            "fn preview_viewport_lines(",
            "`preview_viewport_lines`（ページ移動だけが持っていた別の測り方）",
        ),
    ] {
        if let Some(at) = src.find(needle) {
            panic!(
                "{rel}:{} に旧 API {旧} が戻っている（#1741。可視行数は\
                 `preview_row_geometry` → `editor_scroll::visible_rows` の 1 実装だけ）",
                line_at(src, at)
            );
        }
    }
}

#[test]
fn 応答の可視範囲は同じ値を載せる() {
    let root = workspace_root();
    let main = read_code(&root, MAIN);
    let (at, body) = fn_span(&main, "preview_viewport_json");

    assert!(
        body.contains("preview_cursor_viewport(") && body.contains("\"visible_lines\""),
        "{MAIN}:{} の `preview_viewport_json` が追従と同じ可視範囲から `visible_lines` を\
         載せていない（#1741。CLI / MCP の応答から、追従とページ移動が使っている可視行数を\
         GUI の外で読めるようにする口）",
        line_at(&main, at)
    );
}
