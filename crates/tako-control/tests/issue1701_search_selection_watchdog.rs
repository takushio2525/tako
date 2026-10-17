//! 検索でヒットへ飛んだあとの選択の写しを見張る番犬（#1701）
//!
//! 旧実装は、ヒットへ飛ぶ処理（`preview_search_local`）がカーソルだけを動かし、選択の写し
//! （`preview_selections` = 選択のハイライトと次の打鍵の起点）を揃えるのは本文の組み直し
//! （検索欄を開いているときだけ通る `refresh_preview_from_editor`）の中だけだった。
//! そのため検索欄を開かない CLI / MCP の `PreviewSearch` では、キャレットはヒットへ動くのに
//! 前の選択のハイライトが残った（GUI の ⌘F は直後に組み直すので露見しない）。
//!
//! 直し方は「飛ぶのと写しを揃えるのを 1 実装へ寄せる」形なので、1 行で戻せてしまう形を
//! ソース走査で止める:
//!   1. `preview_search_local`（GUI の検索欄の Enter と dispatch の両方が通る）は
//!      `preview::search_jump` を通り、自分ではカーソルを動かさない
//!   2. `search_jump` は飛んだあとに `sync_selection_copy` で写しを揃える
//!   3. 写しの組み方は `sync_selection_copy` の 1 か所（`sync_preview_selection_from_editor` も通る）
//!   4. GUI の検索欄の Enter / ⇧Enter は `preview_search_local` を通る
//!
//! 写しが実際にヒットへ揃うか（次へ / 前へ・0 件・閲覧中・描画中の Markdown）は
//! `preview.rs` の単体テスト `issue1701_*` が見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。コメントを落とす眺めも同じ部品の中にある（#1609）
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const PREVIEW: &str = "crates/tako-app/src/preview.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 本番コードだけを、コメントを落とした眺めで読む（行番号はそのまま）
fn read_code(rel: &str) -> String {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    let production = production_range::production(&src, rel);
    code_view::without_comments_checked(&production, rel)
}

/// `fn <name>` の頭の行番号と本文（次の `fn` の頭の手前まで）。
/// 見つからなければ名指しで落とす（関数の改名で番犬が空振りしない）
fn fn_body<'a>(src: &'a str, name: &str, rel: &str) -> (usize, &'a str) {
    let mut offset = 0usize;
    let mut head = None;
    for (index, line) in src.lines().enumerate() {
        if tako_core::source_scan::fn_head_name(line) == Some(name) {
            head = Some((index + 1, offset));
            break;
        }
        offset += line.len() + 1;
    }
    let (line, at) = head.unwrap_or_else(|| panic!("{rel} に `fn {name}` が見つからない（#1701）"));
    let rest = &src[at..];
    let after_head = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let mut end = rest.len();
    let mut offset = after_head;
    for body_line in rest[after_head..].lines() {
        if tako_core::source_scan::fn_head_name(body_line).is_some() {
            end = offset;
            break;
        }
        offset += body_line.len() + 1;
    }
    (line, &rest[..end])
}

#[test]
fn 検索のヒットへ飛ぶのは1実装で選択の写しも揃える() {
    let main = read_code(MAIN);
    let preview = read_code(PREVIEW);

    let (line, body) = fn_body(&main, "preview_search_local", MAIN);
    assert!(
        body.contains("preview::search_jump("),
        "{MAIN}:{line} の `preview_search_local` が `preview::search_jump` を通っていない（#1701。\
         飛ぶのと選択の写しを揃えるのを分けると、検索欄を開かない CLI / MCP の検索で\
         前の選択のハイライトが残る）"
    );
    assert!(
        !body.contains("set_cursor("),
        "{MAIN}:{line} の `preview_search_local` が自分でカーソルを動かしている（#1701。\
         次へ / 前へで飛ぶのは `EditState::jump_to_search_hit` の 1 か所 = `search_jump` が写しまで揃える）"
    );

    let (line, body) = fn_body(&preview, "search_jump", PREVIEW);
    assert!(
        body.contains("jump_to_search_hit(") && body.contains("sync_selection_copy("),
        "{PREVIEW}:{line} の `search_jump` がヒットへ飛んだあとに `sync_selection_copy` で\
         選択の写しを揃えていない（#1701）"
    );

    let (line, body) = fn_body(&main, "sync_preview_selection_from_editor", MAIN);
    assert!(
        body.contains("preview::sync_selection_copy(") && !body.contains("PreviewSelection {"),
        "{MAIN}:{line} の `sync_preview_selection_from_editor` が写しを自分で組んでいる（#1701。\
         組み方は `preview::sync_selection_copy` の 1 か所 = 本文を組み直す経路と\
         カーソルだけを動かす経路で同じ写しになる）"
    );

    let (line, body) = fn_body(&main, "handle_search_bar_key", MAIN);
    assert!(
        body.matches("preview_search_local(").count() >= 2,
        "{MAIN}:{line} の検索欄の Enter / ⇧Enter が `preview_search_local` を通っていない（#1701。\
         GUI の ⌘F と CLI / MCP の `PreviewSearch` は同じ関数で飛ぶ）"
    );
}
