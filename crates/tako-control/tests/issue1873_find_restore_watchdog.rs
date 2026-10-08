//! 閲覧中の ⌘F 検索を閉じたら描画へ戻す経路を見張る番犬（#1873）
//!
//! 旧実装は、閲覧中の Markdown で ⌘F → 打鍵すると表示がエディタの行（`code`）へ落ち
//! （ヒットはエディタの行の上に描くので、ここまでは意図どおり = #1661 の
//! `shows_editor_lines`）、検索欄を閉じる口（Escape・⌘F のトグル）が `search_visible = false`
//! にするだけで表示を組み直さなかった。閉じても code のまま・目次は空のまま・編集セッションが
//! 残るので目アイコン（コードとして表示 ⇔ md）も出ないままだった。
//!
//! 直し方は「閉じる口を 1 実装へ寄せる」形なので、1 行で戻せてしまう形をソース走査で止める:
//!   1. 検索欄を閉じる（`search_visible = false`）のは `close_preview_search_bar` と、
//!      編集を抜ける口（`set_preview_editing_local` = #1661 で描き直し済み）だけ
//!   2. Escape と ⌘F のトグルが閉じる口を通る（⌘F は開く口も通る）
//!   3. 閉じる口は、閲覧中の描画由来のセッションなら描画へ戻し、未保存でなければセッションを畳む。
//!      戻すのは表示が実際にエディタの行へ落ちていたときだけ（探さずに開いて閉じただけで
//!      描き直すと全文を解き直し、大きい文書は読み込み中の表示を挟む）
//!   4. 開いた検索欄の上で CLI / MCP が探すと、打鍵と同じく表示を組む（`refresh_preview_from_editor`）
//!   5. CLI / MCP の `visible` が GUI と同じ開閉の口を通る（dispatch → ControlHost → 同じ 2 関数）
//!   6. A/B の口（`TAKO_1873_LEGACY`）が判定の 1 か所から効く
//!
//! 実際に戻るか（表示モード・目次・ヒットの節・セッション）は visual-test 節 `md-find-restore` と
//! `scripts/test-md-find-restore-1873.sh`、`preview.rs` の単体テストが見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。コメントを落とす眺めも同じ部品の中にある（#1609）
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const PREVIEW: &str = "crates/tako-app/src/preview.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

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

/// `fn <name>(` の本文を切り出す（トップレベル / impl 内のどちらでも拾える）。
/// 見つからなければ名指しで落とす（関数の改名で番犬が空振りしない）
fn fn_body<'a>(src: &'a str, name: &str, rel: &str) -> &'a str {
    let at = src
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| tako_core::source_scan::fn_head_name(line) == Some(name))
        .map(|(start, _)| start)
        .unwrap_or_else(|| panic!("{rel} に `fn {name}` が見つからない（#1873）"));
    let rest = &src[at..];
    let after_head = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let end = rest[after_head..]
        .lines()
        .scan(after_head, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| tako_core::source_scan::fn_head_name(line).is_some())
        .map_or(rest.len(), |(start, _)| start);
    &rest[..end]
}

/// `from` から `to` の手前までを切り出す（見つからなければ名指しで落とす）
fn region<'a>(src: &'a str, from: &str, to: &str, rel: &str) -> &'a str {
    let start = src
        .find(from)
        .unwrap_or_else(|| panic!("{rel} に `{from}` が見つからない（#1873）"));
    let rest = &src[start..];
    let end = rest[from.len()..]
        .find(to)
        .map_or(rest.len(), |i| i + from.len());
    &rest[..end]
}

/// 見つけた綴りを `file:line` で名指しするための行番号
fn line_of(src: &str, needle: &str) -> usize {
    src.find(needle)
        .map(|at| src[..at].matches('\n').count() + 1)
        .unwrap_or(0)
}

#[test]
fn 検索欄を閉じるのは閉じる口と編集を抜ける口だけ() {
    let code = read_code(MAIN);
    let mut current_fn: Option<&str> = None;
    let mut closes = 0;
    for (index, line) in code.lines().enumerate() {
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            current_fn = Some(name);
        }
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("search_visible=false") || compact.contains("search_visible=!") {
            closes += 1;
            assert!(
                matches!(
                    current_fn,
                    Some("close_preview_search_bar" | "set_preview_editing_local")
                ),
                "{MAIN}:{} の `{}` が検索欄を直に閉じている（#1873。閉じるのは \
                 `close_preview_search_bar` の 1 実装。直に閉じると閲覧中の Markdown が \
                 コード表示のまま・目次は空のまま残る）",
                index + 1,
                current_fn.unwrap_or("?")
            );
        }
    }
    assert!(
        closes >= 2,
        "{MAIN} で検索欄を閉じる代入が {closes} か所しか無い（走査が空振りしている）"
    );
}

#[test]
fn escapeと検索のトグルが閉じる口を通る() {
    let code = read_code(MAIN);
    let keys = fn_body(&code, "handle_search_bar_key", MAIN);
    assert!(
        keys.contains("close_preview_search_bar("),
        "{MAIN}:{} の `handle_search_bar_key` の Escape が `close_preview_search_bar` を\
         通っていない（#1873。閉じても描画へ戻らない）",
        line_of(&code, "fn handle_search_bar_key(")
    );
    let toggle = region(&code, "_: &FindPreview", ".on_action(", MAIN);
    let at = line_of(&code, "_: &FindPreview");
    assert!(
        toggle.contains("close_preview_search_bar(") && toggle.contains("open_preview_search_bar("),
        "{MAIN}:{at} の ⌘F（`FindPreview`）が開く口 / 閉じる口を通っていない（#1873。\
         トグルで閉じても描画へ戻らない）"
    );
}

#[test]
fn 閉じる口は描画へ戻しセッションを畳む() {
    let code = read_code(MAIN);
    let body = fn_body(&code, "close_preview_search_bar", MAIN);
    let head = line_of(&code, "fn close_preview_search_bar(");
    let judge = body.find("search_close_resumes_rendered()");
    let restore = body.find("restore_rendered_preview(");
    let fold = body.find("preview_edits.remove(");
    assert!(
        judge.is_some() && restore.is_some(),
        "{MAIN}:{head} の `close_preview_search_bar` が `search_close_resumes_rendered` を見て\
         `restore_rendered_preview` を呼んでいない（#1873。Issue の本体: 閉じても code のまま）"
    );
    assert!(
        fold.is_some() && body.contains("dirty()"),
        "{MAIN}:{head} の `close_preview_search_bar` が未保存でないセッションを畳んでいない\
         （#1873。セッションが残ると目アイコンが出ない）"
    );
    assert!(
        restore < fold,
        "{MAIN}:{head} の `close_preview_search_bar` が描き直しより先にセッションを畳んでいる\
         （#1873。描き直しはセッションの本文から組む）"
    );
    assert!(
        body.contains("PreviewMode::Markdown"),
        "{MAIN}:{head} の `close_preview_search_bar` が「表示がエディタの行へ落ちていたか」を\
         見ずに描き直している（#1873。探さずに開いて閉じただけで全文を解き直し、\
         大きい文書は読み込み中の表示を挟む）"
    );
}

#[test]
fn 開いた検索欄の上で探すと打鍵と同じく表示を組む() {
    let code = read_code(MAIN);
    let body = fn_body(&code, "preview_search_local", MAIN);
    assert!(
        body.contains("search_visible") && body.contains("refresh_preview_from_editor("),
        "{MAIN}:{} の `preview_search_local` が、検索欄を開いているときに \
         `refresh_preview_from_editor` で表示を組んでいない（#1873。CLI / MCP で開いて探しても \
         ヒットが描かれず、閉じる口の描き直しも試せない）",
        line_of(&code, "fn preview_search_local(")
    );
}

#[test]
fn cli_mcpの開閉はguiと同じ口を通る() {
    let main = read_code(MAIN);
    let host = fn_body(&main, "set_preview_search_visible", MAIN);
    assert!(
        host.contains("open_preview_search_bar(") && host.contains("close_preview_search_bar("),
        "{MAIN}:{} の `set_preview_search_visible`（ControlHost）が GUI と同じ開く口 / 閉じる口を\
         通っていない（#1873。CLI / MCP で閉じても描画へ戻らない）",
        line_of(&main, "fn set_preview_search_visible(")
    );
    let dispatch = read_code(DISPATCH);
    let arm = region(
        &dispatch,
        "Request::PreviewSearch {",
        "Request::PreviewReplace {",
        DISPATCH,
    );
    let at = line_of(&dispatch, "Request::PreviewSearch {");
    let open = arm.find("set_preview_search_visible(target, true)");
    let search = arm.find(".preview_search(");
    let close = arm.find("set_preview_search_visible(target, false)");
    assert!(
        open.is_some() && close.is_some(),
        "{DISPATCH}:{at} の `PreviewSearch` が `visible` を ControlHost の開く口 / 閉じる口へ\
         渡していない（#1873。CLI / MCP で閉じても描画へ戻らない）"
    );
    assert!(
        open < search && search < close,
        "{DISPATCH}:{at} の `PreviewSearch` の順が「開く → 探す → 閉じる」になっていない\
         （#1873。開いた欄の上で探す = GUI の ⌘F → 打鍵と同じ順）"
    );
    assert!(
        arm.contains("preview_search_visible(target)"),
        "{DISPATCH}:{at} の `PreviewSearch` が応答へ開閉の状態（`search.visible`）を載せていない（#1873）"
    );
}

#[test]
fn ab_の口は判定の1か所から効く() {
    let code = read_code(PREVIEW);
    let legacy = fn_body(&code, "find_restore_legacy", PREVIEW);
    assert!(
        legacy.contains("\"TAKO_1873_LEGACY\""),
        "{PREVIEW}:{} の `find_restore_legacy` が `TAKO_1873_LEGACY` を読んでいない",
        line_of(&code, "fn find_restore_legacy(")
    );
    let judge = fn_body(&code, "search_close_resumes_rendered", PREVIEW);
    assert!(
        judge.contains("find_restore_legacy()") && judge.contains("resumes_rendered()"),
        "{PREVIEW}:{} の `search_close_resumes_rendered` が A/B の口か #1661 の判定\
         （`resumes_rendered`）を見ていない（#1873）",
        line_of(&code, "fn search_close_resumes_rendered(")
    );
}
