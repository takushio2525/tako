//! プレビュー本文がペインの文字サイズを読むことを見張る番犬（#1772）
//!
//! ⌘+ / ⌘- / ⌘0（`zoom_focused_pane`）はコード / md のプレビューでも**ペイン単位の文字サイズ**
//! （`pane_font_sizes`）を 13 → 16 のように動かすが、本文はそれを読まずルートの
//! `theme.font_size` を継承していたので、字も行の高さ（21px）も動かなかった。gpui の
//! `StyledText` は字の大きさと行高を**祖先の text style** から採り、`with_default_highlights`
//! へ渡す `TextStyle` のそれは使わない（#947 と同じ機序）ので、効かせる場所は「本文の器」しか無い。
//!
//! 直し方は、本文の文字サイズを 1 実装（`preview_body_font_size` → `pane_font_size`）へ寄せ、
//! 器・md の基準・行の高さの見積もり・仮想リストの測り直しがすべてそこから採る形。
//! ソース走査で見張るのは次の 6 つ:
//!   1. 本文の器（`render_preview_pane` の `preview-scroll`）が `preview_body_font_size` の値を
//!      `.text_size(` で、行の高さを `PREVIEW_BODY_LINE_HEIGHT` で**継承側に**指定する
//!   2. `preview_body_font_size` はペインの文字サイズ（`pane_font_size`）を返す
//!      （テーマ既定は A/B の逃げ道 `preview_font_legacy` の腕だけ）
//!   3. md の基準サイズ（見出し・余白・行の高さ）は本文の文字サイズから: `preview_md_block_sel` と
//!      コピーボタンが `preview_body_font_size` を渡し、`md_view::render_block` / `render_table` は
//!      `theme.font_size` を読まない
//!   4. 行の高さの見積もり（`preview_code_line_height_estimate`）も同じ文字サイズと行の高さから
//!   5. 文字サイズが変わったら仮想リストの全 item を測り直す（`remeasure`）
//!   6. preview_render.rs の製品コードで `theme.font_size` を読むのは規則 2 の逃げ道だけ
//!      （本文の要素を足すときにテーマ既定を直読みする写しを作らない）
//!
//! 実際に字と行が伸びるか（実ピクセル・描いた行の実寸・可視行数・クリック位置・10 万行）は
//! visual-test 節 `editor-font`（`scripts/test-editor-font-1772.sh`。`TAKO_1772_LEGACY=1` の
//! 旧挙動の腕が名指しで落ちる）が見る。

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

const RENDER: &str = "crates/tako-app/src/preview_render.rs";
const MD_VIEW: &str = "crates/tako-app/src/md_view.rs";

/// コメントを潰した眺め（説明文に `theme.font_size` と書けるようにするため。#1609）。
/// 文字列は囲みごと残す（本文の器を id `"preview-scroll"` で探すため）
fn read_code(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    code_view::without_comments_checked(&src, rel)
}

/// 製品コードだけの眺め（`#[cfg(test)] mod tests` より前）
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
        .unwrap_or_else(|| panic!("`fn {name}` が見つからない（#1772）"));
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

/// 空白を落とした眺め（rustfmt の改行位置に判定を左右させない）
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn 本文の器が文字サイズと行の高さを継承側で指定する() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let (at, body) = fn_span(&render, "render_preview_pane");
    let flat = squash(body);

    assert!(
        flat.contains("letbody_font_size=self.preview_body_font_size(pane_id);"),
        "{RENDER}:{} の `render_preview_pane` が本文の文字サイズを `preview_body_font_size` から\
         採っていない（#1772。ペインの文字サイズ = ⌘+ / ⌘- / ⌘0 が動かす値を読む口はそこだけ）",
        line_at(&render, at)
    );
    // 器（`preview-scroll`）の組み立ての中で指定していること。器の外（ヘッダ等）へ付けると
    // クロームまで拡大し、器に付け忘れると本文はルートの `theme.font_size` のまま
    let scroll_at = body.find("\"preview-scroll\"").unwrap_or_else(|| {
        panic!(
            "{RENDER}:{} の `render_preview_pane` に本文の器 `preview-scroll` が見つからない",
            line_at(&render, at)
        )
    });
    // 器の id の直後から、本文を差し込む `.children(body)` までが器の組み立て
    let tail = &body[scroll_at..];
    let until_children = tail.find(".children(body)").unwrap_or(tail.len());
    let container = squash(&tail[..until_children]);
    for (needle, 理由) in [
        (
            ".text_size(px(body_font_size))",
            "字の大きさは祖先の text style から採られる（`TextStyle` に積んでも効かない）",
        ),
        (
            ".line_height(PREVIEW_BODY_LINE_HEIGHT)",
            "行の高さも継承側で比として指定する（祖先の行高が変わっても本文が動かない = #611）",
        ),
    ] {
        assert!(
            container.contains(needle),
            "{RENDER}:{} の本文の器 `preview-scroll` に `{needle}` が無い（#1772。{理由}）",
            line_at(&render, at + scroll_at)
        );
    }
}

#[test]
fn 本文の文字サイズはペインの文字サイズ() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let (at, body) = fn_span(&render, "preview_body_font_size");
    let flat = squash(body);
    assert!(
        flat.contains("self.pane_font_size(pane_id)"),
        "{RENDER}:{} の `preview_body_font_size` がペインの文字サイズ（`pane_font_size`）を\
         返していない（#1772。⌘+ / ⌘- / ⌘0 とメニュー = CLI `tako menu invoke` / MCP `tako_menu` が\
         動かすのは `pane_font_sizes`）",
        line_at(&render, at)
    );
    // テーマ既定を返してよいのは A/B の逃げ道の腕だけ
    if let Some(theme_at) = flat.find("self.theme.font_size") {
        let guard = flat.find("ifpreview_font_legacy(){");
        assert!(
            guard.is_some_and(|g| g < theme_at),
            "{RENDER}:{} の `preview_body_font_size` が `preview_font_legacy()` の腕の外で\
             `theme.font_size` を返している（#1772。それが修正前の症状そのもの）",
            line_at(&render, at)
        );
    }
}

#[test]
fn md_の基準サイズは本文の文字サイズから() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let md = read_code(&root, MD_VIEW);

    let (at, body) = fn_span(&render, "preview_md_block_sel");
    let flat = squash(body);
    assert!(
        flat.contains("letbase=self.preview_body_font_size(pane_id);")
            && flat.contains("render_block(&theme,base,"),
        "{RENDER}:{} の `preview_md_block_sel` が md の基準サイズに本文の文字サイズを渡していない\
         （#1772。段落の字だけ大きく、見出し・余白・行の高さは既定のまま食い違う）",
        line_at(&render, at)
    );
    let (at, body) = fn_span(&render, "code_overlay");
    assert!(
        squash(body).contains("preview_body_font_size(pane_id)"),
        "{RENDER}:{} の `code_overlay`（md のコードブロックのコピーボタン）が本文の文字サイズから\
         寸法を採っていない（#1772。拡大した本文に既定サイズのボタンが小さく浮く）",
        line_at(&render, at)
    );

    let product = product_part(&md);
    for name in ["render_block", "render_table"] {
        let (at, body) = fn_span(product, name);
        let head = body.split(") -> AnyElement").next().unwrap_or(body);
        assert!(
            squash(head).contains("base:f32,"),
            "{MD_VIEW}:{} の `{name}` が基準サイズ `base: f32` を引数で受けていない\
             （#1772。プレビューはペインの文字サイズを、アップデート詳細・チャットは\
             `theme.font_size` を渡す）",
            line_at(product, at)
        );
        if let Some(bad) = body.find("theme.font_size") {
            panic!(
                "{MD_VIEW}:{} の `{name}` が `theme.font_size` を読んでいる（#1772。\
                 基準サイズは引数の `base` だけ。テーマ既定を読むと拡大しても md が動かない）",
                line_at(product, at + bad)
            );
        }
    }
}

#[test]
fn 行の高さの見積もりも同じ文字サイズから() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let (at, body) = fn_span(&render, "preview_code_line_height_estimate");
    let flat = squash(body);
    for (needle, 理由) in [
        (
            "self.preview_body_font_size(pane_id)",
            "1 行も描いていないフレームの見積もりが既定サイズの 21px のままになる",
        ),
        (
            "line_height:PREVIEW_BODY_LINE_HEIGHT",
            "本文の器と同じ行の高さの比で見積もる",
        ),
    ] {
        assert!(
            flat.contains(needle),
            "{RENDER}:{} の `preview_code_line_height_estimate` に `{needle}` が無い（#1772。{理由}）",
            line_at(&render, at)
        );
    }
}

#[test]
fn 文字サイズが変わったら仮想リストを測り直す() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let (at, body) = fn_span(&render, "preview_body_list_state_inner");
    let flat = squash(body);
    for (needle, 理由) in [
        (
            "font_size:self.preview_body_font_size(pane_id)",
            "組んだときの文字サイズを控える（変わったかを知る材料）",
        ),
        (
            ".remeasure();",
            "gpui の `list` は見えている item しか測り直さないので、画面外の行が古い高さのまま\
             総高さとスクロール量の見積もりに残る",
        ),
    ] {
        assert!(
            flat.contains(needle),
            "{RENDER}:{} の `preview_body_list_state_inner` に `{needle}` が無い（#1772。{理由}）",
            line_at(&render, at)
        );
    }
}

#[test]
fn テーマ既定の文字サイズを直読みするのは逃げ道だけ() {
    let root = workspace_root();
    let render = read_code(&root, RENDER);
    let product = product_part(&render);
    let (geo_at, geo_body) = fn_span(product, "preview_body_font_size");
    let allowed = geo_at..geo_at + geo_body.len();
    for (offset, _) in product.match_indices("theme.font_size") {
        assert!(
            allowed.contains(&offset),
            "{RENDER}:{} が `theme.font_size` を直に読んでいる（#1772。プレビュー本文の文字サイズは\
             `preview_body_font_size` の 1 実装から採る。テーマ既定を読む写しがあると、⌘+ / ⌘- で\
             そこだけ動かない）",
            line_at(product, offset)
        );
    }
}
