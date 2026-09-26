//! 編集カーソルの追従スクロールが編集経路から外れないことを見張る番犬（#1649）
//!
//! 旧実装には `ListState` に対する `scroll_to(cursor)` の呼び出しが **1 件も無く**、
//! 打鍵・矢印の連打・⌘F のヒットへ飛ぶ・undo / redo・貼り付けのどれでもカーソルが
//! 画面外へ出たまま戻らなかった。連鎖して IME の未確定下線も消えていた
//! （カーソル行の `TextLayout` が無い → アンカーが採れない → ターミナル枝へ落ちるが
//! プレビューペインに端末は無いので None）。
//!
//! 追従は「呼ぶ場所を増やす」のではなく**経路の要 1 か所へ載せる**形で直してある。
//! ソース走査で見張るのは次の 6 つ:
//!   1. 編集で状態が動く要（`refresh_preview_from_editor`）が追従を呼ぶ
//!   2. カーソルだけが動く経路（`preview_search_local` / 変換開始）も呼ぶ
//!   3. 判断の算術は `tako_core::editor_scroll` の純関数を通る（UI 層に写しを作らない）
//!   4. 器は 2 種類（仮想リスト / div スクロール）とも動かす
//!   5. 余白は 1 行以上（0 だとカーソルが器の端に貼り付く）
//!   6. プレビュー編集の IME アンカーは画面外でも本文の中で答える
//!
//! 実際に可視範囲へ入るか（状態値の関係）は `tako_core::editor_scroll` の単体テストと
//! visual-test 節 `cursor-follow` が見る。

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

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// **肯定の存在確認**が見る眺め（doc コメントに書いた同じ綴りで緑にならないように潰す。#1609）
fn read_code(root: &Path, rel: &str) -> String {
    code_view::without_comments_checked(&read(root, rel), rel)
}

/// `fn <name>(` の本文を切り出す（トップレベル / impl 内のどちらでも拾える）
fn fn_body(src: &str, name: &str) -> String {
    let heads = [
        format!("\nfn {name}("),
        format!("\npub fn {name}("),
        format!("\npub(crate) fn {name}("),
        format!("\n    fn {name}("),
        format!("\n    pub fn {name}("),
        format!("\n    pub(crate) fn {name}("),
    ];
    let at = heads
        .iter()
        .filter_map(|head| src.find(head.as_str()).map(|at| at + head.len()))
        .min()
        .unwrap_or_else(|| panic!("`fn {name}` が見つからない（#1649）"));
    let rest = &src[at..];
    let end = [
        rest.find("\nfn "),
        rest.find("\npub fn "),
        rest.find("\npub(crate) fn "),
        rest.find("\n    fn "),
        rest.find("\n    pub fn "),
        rest.find("\n    pub(crate) fn "),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// 見つけた綴りを `file:line` で名指しするための行番号
fn line_of(src: &str, needle: &str) -> usize {
    src.find(needle)
        .map(|at| src[..at].matches('\n').count() + 1)
        .unwrap_or(0)
}

#[test]
fn 編集で状態が動く経路は追従スクロールを呼ぶ() {
    let root = workspace_root();
    let code = read_code(&root, MAIN);

    for (name, 理由) in [
        (
            "refresh_preview_from_editor",
            "打鍵・矢印・改行・BS / Del・貼り付け・undo / redo・IME 確定・\
             CLI / MCP の PreviewApply はすべてここを通る。外すと全経路が同時に追従を失う",
        ),
        (
            "preview_search_local",
            "⌘F のヒットへ飛ぶのは本文を組み直さないので、要を通らない。\
             外すと「検索したのにヒットが見えない」へ戻る",
        ),
        (
            "replace_and_mark_text_in_range",
            "IME の変換開始はバッファを触らないので要を通らない。外すと\
             ホイールで離れた位置から変換を始めたときに未確定下線のアンカーが採れない",
        ),
    ] {
        let body = fn_body(&code, name);
        assert!(
            body.contains("follow_preview_cursor("),
            "{MAIN}:{} の `{name}` が `follow_preview_cursor` を呼んでいない（#1649。{理由}）",
            line_of(&code, &format!("fn {name}("))
        );
    }
}

#[test]
fn 編集開始のキャレットは見ている行へ置く() {
    let root = workspace_root();
    let code = read_code(&root, MAIN);
    let body = fn_body(&code, "set_preview_editing_local");

    assert!(
        body.contains("preview_first_visible_line(")
            && body.contains("offset_for_line_byte_col(")
            && body.contains("set_cursor("),
        "{MAIN}:{} の `set_preview_editing_local` が編集開始のキャレットを\
         「見ている行」へ置いていない（#1649。`EditState::open` はオフセット 0 から\
         始めるので、3,000 行目を見ている状態で編集を始めると追従が器を先頭へ\
         引き戻す = 打つ場所と見ている場所が食い違う）",
        line_of(&code, "fn set_preview_editing_local(")
    );
}

#[test]
fn 追従の判断はcoreの純関数を通る() {
    let root = workspace_root();
    let code = read_code(&root, RENDER);
    let body = fn_body(&code, "follow_preview_cursor");

    assert!(
        body.contains("editor_scroll::follow_cursor("),
        "{RENDER}:{} の `follow_preview_cursor` が `editor_scroll::follow_cursor` を\
         通っていない（#1649。先頭可視行の算術を UI 層へ写すと、器が 2 種類あるので\
         片方だけ直した食い違いが生まれ、実ピクセルなしでは固定できなくなる）",
        line_of(&code, "fn follow_preview_cursor(")
    );
    assert!(
        body.contains("editor_scroll::FOLLOW_MARGIN"),
        "{RENDER}:{} の `follow_preview_cursor` が余白の定数を直書きしている（#1649。\
         余白の正本は `tako_core::editor_scroll::FOLLOW_MARGIN` の 1 つだけ）",
        line_of(&code, "fn follow_preview_cursor(")
    );
    // 器は 2 種類（#821 の仮想リストと `TAKO_821_NO_VIRTUAL_LIST=1` の div スクロール）
    for needle in ["list.scroll_to(", "handle.scroll_to_top_of_item("] {
        assert!(
            body.contains(needle),
            "{RENDER}:{} の `follow_preview_cursor` に `{needle}` が無い（#1649。\
             器は仮想リストと div スクロールの 2 種類あり、片方だけ動かすと\
             A/B を取った瞬間に追従が消える）",
            line_of(&code, "fn follow_preview_cursor(")
        );
    }

    let viewport = fn_body(&code, "preview_cursor_viewport");
    assert!(
        viewport.contains("preview_row_geometry("),
        "{RENDER}:{} の `preview_cursor_viewport` が可視行数を `preview_row_geometry` から\
         採っていない（#1649 / #1741。器の高さを 1 行の高さで割る算術を直書きすると、\
         高さが 0 のフレームで可視行数が飽和して「どこへ飛んでも見えている」へ倒れ、\
         ページ移動の歩幅とも食い違う。測り方の番犬は `issue1741_viewport_lines_watchdog`）",
        line_of(&code, "fn preview_cursor_viewport(")
    );
}

#[test]
fn 追従の余白は一行以上を確保する() {
    let root = workspace_root();
    let code = read_code(&root, CORE);

    let at = code
        .find("pub const FOLLOW_MARGIN: usize = ")
        .unwrap_or_else(|| panic!("{CORE} に `FOLLOW_MARGIN` が無い（#1649）"));
    let value: usize = code[at..]
        .split('=')
        .nth(1)
        .and_then(|rest| rest.trim().trim_end_matches(';').split(';').next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("{CORE} の `FOLLOW_MARGIN` が数値リテラルではない（#1649）"));
    assert!(
        value >= 1,
        "{CORE}:{} の `FOLLOW_MARGIN` が {value}（#1649。0 にするとカーソルが器の端に\
         貼り付き、次に打つ 1 文字の行き先が見えない。改行の直後がいちばん分かりやすい）",
        line_of(&code, "pub const FOLLOW_MARGIN")
    );
}

#[test]
fn プレビュー編集のimeアンカーは画面外でも本文の中で答える() {
    let root = workspace_root();
    let code = read_code(&root, MAIN);

    let origin = fn_body(&code, "pane_cursor_origin");
    assert!(
        origin.contains("preview_cursor_origin("),
        "{MAIN}:{} の `pane_cursor_origin` がプレビュー編集を\
         `preview_cursor_origin` へ回していない（#1649。ここで `?` で抜けると\
         ターミナル枝へ落ちるが、プレビューペインに端末は無いので\
         `pane_cursor_origin_for_ime` も None = 未確定下線と候補ウィンドウの\
         除外領域がそのフレームだけ消える）",
        line_of(&code, "fn pane_cursor_origin(")
    );

    let preview_origin = fn_body(&code, "preview_cursor_origin");
    assert!(
        preview_origin.contains("preview_pending_cursor_origin("),
        "{MAIN}:{} の `preview_cursor_origin` が画面外の見積もりへ倒していない（#1649。\
         行の `TextLayout` は paint でしか控えられないので、追従を要求した直後の\
         1 フレームは必ず None になる）",
        line_of(&code, "fn preview_cursor_origin(")
    );

    let pending = fn_body(&code, "preview_pending_cursor_origin");
    assert!(
        pending.contains("editor_scroll::cursor_row_in_viewport("),
        "{MAIN}:{} の `preview_pending_cursor_origin` が\
         `editor_scroll::cursor_row_in_viewport` を通っていない（#1649。\
         「可視化後にカーソル行が来る位置」の算術を UI 層へ写すと、\
         追従の判断とアンカーの見積もりが食い違って下線が本文の外へ出る）",
        line_of(&code, "fn preview_pending_cursor_origin(")
    );
}

#[test]
fn 追従の挙動はabで倒せる入口を持つ() {
    let root = workspace_root();
    let code = read_code(&root, RENDER);

    assert!(
        code.contains("TAKO_1649_LEGACY"),
        "{RENDER} に `TAKO_1649_LEGACY` の A/B 入口が無い（#1649。\
         同一バイナリで「追わない」へ戻せないと、visual-test 節の検出力を実証できない）"
    );
    let legacy = fn_body(&code, "cursor_follow_legacy");
    assert!(
        legacy.contains("env::var_os("),
        "{RENDER}:{} の `cursor_follow_legacy` が env を読んでいない（#1649。\
         アームの目印コメントだけでは A/B は倒せない = #1609 が見つけた偽の緑の型）",
        line_of(&code, "fn cursor_follow_legacy(")
    );
    let follow = fn_body(&code, "follow_preview_cursor");
    assert!(
        follow.contains("cursor_follow_legacy()"),
        "{RENDER}:{} の `follow_preview_cursor` が A/B 入口を見ていない（#1649）",
        line_of(&code, "fn follow_preview_cursor(")
    );
}
