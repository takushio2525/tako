//! visual-test `rename-1686` / `rename-1686-real`（FR-3.45 / Issue #1686）: 識別子の右クリックの
//! 「名前の変更…」で欄を開き、**実キーで**新しい名前を打って Enter で書き換え、undo（⌘Z。Windows は
//! Ctrl+Z）1 回で元の全文へ戻ることを確かめる。
//!
//! - `rename-1686`（偽サーバ）:
//!   ① 識別子の右クリックに「名前の変更…」がホバーの後に出て、押すとタイトルの下に今の名前（`value`）の
//!      欄が開き、打鍵と変換の宛先が欄になる
//!   ② 名前を消して `item` を打ち Enter → 語としての `value` だけが `item` になる（`Value::new()` と
//!      `values` は変わらない）・欄が閉じる
//!   ③ undo 1 回で元の全文とバイト一致
//!   ④ 別のファイルに及ぶ名前（偽サーバの規則で `lib.rs` も書き換える答え）は Enter でも当てず、ヘッダに
//!      段階 1 の理由を出す（本文は変わらない）。⑤ Esc で欄を閉じると本文は変わらない
//! - `rename-1686-real`（実の rust-analyzer。無ければ SKIPPED）: 読み込みが済んでから ①〜③ を同じ手順で通す
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1686_LEGACY=1` は ①（項目が無い）で名指しの FAILED に
//! なる = 同一バイナリでの A/B。画像は `TAKO_VISUAL_DUMP_DIR` の下へ節の名前つきで落とす

use super::*;

/// #1653 の事故の形（小文字の `value` だけを変え、`Value::new()` と `values` は変えない）と、
/// ④ で別のファイルにも及ぶ名前（`shared`）
const MAIN: &str = "struct Value;\nimpl Value {\n    fn new() -> Self {\n        Value\n    }\n}\n\nfn main() {\n    let value = Value::new();\n    let values = 1;\n    let shared = values;\n    drop((value, shared));\n}\n";
const RENAMED: &str = "struct Value;\nimpl Value {\n    fn new() -> Self {\n        Value\n    }\n}\n\nfn main() {\n    let item = Value::new();\n    let values = 1;\n    let shared = values;\n    drop((item, shared));\n}\n";
/// `value` の行（0 起点）と桁
const VALUE_AT: (usize, usize) = (8, 8);
/// `shared` の行（0 起点）と桁
const SHARED_AT: (usize, usize) = (10, 8);

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）
fn dump(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp, name: &str) {
    notify_and_draw(any, window, cx);
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    let Some((frame, _)) = capture_frame(any, cx) else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(name);
    if frame.save(&path).is_ok() {
        println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
    }
}

fn buffer_text(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> String {
    window
        .update(cx, |app, _, _| {
            app.preview_edits
                .get(&pane)
                .map(|e| e.buffer.text().to_string())
        })
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn field_text(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> Option<String> {
    window
        .update(cx, |app, _, _| {
            app.lsp_rename
                .input
                .as_ref()
                .map(|i| i.field.text().to_string())
        })
        .ok()
        .flatten()
}

/// 文書が言語サーバにつながり、読み込みが済むまで待つ
async fn wait_ready(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
    limit: Duration,
) {
    let path = window
        .update(cx, |app, _, _| {
            app.preview_edits
                .get(&pane)
                .map(|e| e.buffer.path().to_path_buf())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {label}: 編集バッファが無い")));
    check(
        vt1860_wait(any, window, cx, limit, &|cx| {
            window
                .update(cx, |app, _, _| {
                    app.lsp_hover_enabled_for(pane) && !app.lsp.server_loading(&path)
                })
                .unwrap_or(false)
        })
        .await,
        &format!("visual-test {label}: 文書がサーバへつながり読み込みが済む（素材の前提）"),
    );
}

/// ① 右クリック → 「名前の変更…」→ 欄が今の名前で開く
#[allow(clippy::too_many_arguments)]
async fn open_rename(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    (line, column): (usize, usize),
    word: &str,
    label: &str,
    shot: &str,
) {
    use tako_core::lsp::menu::MenuItem;
    let at = vt1684_point(window, cx, pane, line, column..column + word.len())
        .unwrap_or_else(|| fail(&format!("visual-test {label} ①: {word} の位置")));
    vt1860_click(any, window, cx, at, MouseButton::Right);
    check(
        vt1684_settled(any, window, cx, Duration::from_secs(90)).await,
        &format!("visual-test {label} ①: メニューの言語サーバの節が決まる"),
    );
    let (_, drawn) = vt1684_menu(any, window, cx).unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {label} ① menu={drawn:?}");
    dump(any, window, cx, &format!("{shot}-menu.png"));
    let rename_id = MenuItem::Rename.id();
    check(
        drawn.contains(&rename_id),
        &format!("visual-test {label} ①: 識別子の右クリックに「名前の変更…」が出る ({drawn:?})"),
    );
    let item = vt1684_item_point(window, cx, rename_id)
        .unwrap_or_else(|| fail(&format!("visual-test {label} ①: 項目の矩形が無い")));
    vt1860_click(any, window, cx, item, MouseButton::Left);
    notify_and_draw(any, window, cx);
    let (opened, target) = window
        .update(cx, |app, _, _| {
            (
                app.lsp_rename_input_active(),
                app.app_text_input() == Some(AppTextInput::LspRename),
            )
        })
        .unwrap_or((false, false));
    let typed = field_text(window, cx);
    println!("TAKO_VISUAL_PIXEL: {label} ① opened={opened} ime_target={target} field={typed:?}");
    dump(any, window, cx, &format!("{shot}-input.png"));
    check(
        opened && target && typed.as_deref() == Some(word),
        &format!(
            "visual-test {label} ①: 押すと今の名前（{word}）の欄が開き、打鍵と変換の宛先が欄になる（{typed:?}）"
        ),
    );
}

/// 欄の名前を消して `name` を実キーで打つ
fn retype(any: AnyWindowHandle, cx: &mut AsyncApp, old: &str, name: &str) {
    for _ in old.chars() {
        press(any, cx, "backspace");
    }
    type_text(any, cx, name, false);
}

fn undo_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    }
}

/// ②〜③: Enter で書き換え、undo 1 回で戻る
async fn rename_and_undo(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
    shot: &str,
    limit: Duration,
) {
    retype(any, cx, "value", "item");
    check(
        field_text(window, cx).as_deref() == Some("item"),
        &format!("visual-test {label} ②: 欄に打った名前が入る"),
    );
    let started = std::time::Instant::now();
    press(any, cx, "enter");
    let renamed = vt1860_wait(any, window, cx, limit, &|cx| {
        buffer_text(window, cx, pane) == RENAMED
    })
    .await;
    let closed = field_text(window, cx).is_none();
    let status = window
        .update(cx, |app, _, _| {
            app.lsp_goto
                .status
                .as_ref()
                .map(|(_, m, e, _)| (m.clone(), *e))
        })
        .ok()
        .flatten();
    println!(
        "TAKO_VISUAL_PIXEL: {label} ② renamed={renamed} in {:.1}s closed={closed} status={status:?}",
        started.elapsed().as_secs_f32()
    );
    dump(any, window, cx, &format!("{shot}-renamed.png"));
    check(
        renamed && closed,
        &format!(
            "visual-test {label} ②: Enter で語としての value だけが item になり欄が閉じる（{:?}）",
            buffer_text(window, cx, pane)
        ),
    );
    // ③ undo 1 回（キーは編集の undo と同じ）
    let _ = window.update(cx, |app, _, _| {
        let _ = app.workspace.active_tab_mut().tree_mut().focus(pane);
    });
    press(any, cx, undo_key());
    let undone = vt1860_wait(any, window, cx, Duration::from_secs(10), &|cx| {
        buffer_text(window, cx, pane) == MAIN
    })
    .await;
    println!("TAKO_VISUAL_PIXEL: {label} ③ undone={undone}");
    dump(any, window, cx, &format!("{shot}-undone.png"));
    check(
        undone,
        &format!("visual-test {label} ③: undo 1 回で元の全文とバイト一致へ戻る"),
    );
}

/// visual-test `rename-1686`（偽サーバ）。相は冒頭の説明 ①〜⑤
pub(super) async fn rename_1686_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "rename-1686";
    inject_section_failure(LABEL);
    // 置き場は `completion_scene` が作り直す（同じ名前）。偽サーバは規則を起動のときに読むので、
    // ④ の答えに載せる URI は場面の前に組む（tako は開いたファイルを実パスへ直す = /private/var）
    let tmp = tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir());
    let dir = tmp.join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    remove_on_fail(&dir);
    let main_uri = tako_core::file_uri::from_path(&dir.join("src").join("main.rs"));
    let lib_uri = tako_core::file_uri::from_path(&dir.join("src").join("lib.rs"));
    let edit = |line: u64, character: u64, len: u64| {
        serde_json::json!({
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character + len },
            },
            "newText": "common",
        })
    };
    let rules = serde_json::json!([{
        "method": "textDocument/rename",
        "line": SHARED_AT.0,
        "result": { "changes": {
            main_uri: [edit(10, 8, 6), edit(11, 17, 6)],
            lib_uri: [edit(0, 9, 6), edit(0, 29, 6)],
        }},
    }]);
    let rules_file = tmp.join(format!(
        "tako-visual-{LABEL}-rename-{}.json",
        std::process::id()
    ));
    remove_on_fail(&rules_file);
    std::fs::write(&rules_file, rules.to_string()).expect("rename.json");
    std::env::set_var("TAKO_LSP_FAKE_RENAME", &rules_file);
    let (pane, _dir, _override_env) = completion_scene(
        any,
        window,
        cx,
        LABEL,
        Some(serde_json::json!({ "items": [] })),
        MAIN,
        (8, 0),
    )
    .await;
    wait_ready(any, window, cx, pane, LABEL, Duration::from_secs(20)).await;

    open_rename(any, window, cx, pane, VALUE_AT, "value", LABEL, LABEL).await;
    rename_and_undo(any, window, cx, pane, LABEL, LABEL, Duration::from_secs(20)).await;

    // ④ 別のファイルに及ぶ名前は Enter でも当てない（ヘッダに理由）
    open_rename(
        any,
        window,
        cx,
        pane,
        SHARED_AT,
        "shared",
        LABEL,
        "rename-1686-others",
    )
    .await;
    retype(any, cx, "shared", "common");
    press(any, cx, "enter");
    let shown = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.lsp_rename.input.is_none()
                    && app
                        .lsp_goto
                        .status
                        .as_ref()
                        .is_some_and(|(p, _, error, _)| *p == pane && *error)
            })
            .unwrap_or(false)
    })
    .await;
    let status = window
        .update(cx, |app, _, _| {
            app.lsp_goto.status.as_ref().map(|(_, m, _, _)| m.clone())
        })
        .ok()
        .flatten()
        .unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ shown={shown} status={status:?}");
    dump(any, window, cx, "rename-1686-others-refused.png");
    let reason = tako_control::lsp::text::fill(
        tako_control::lsp::text::RENAME_OTHER_FILES_REASON,
        &[("files", "1"), ("edits", "2")],
    );
    check(
        shown && status.contains(&reason) && buffer_text(window, cx, pane) == MAIN,
        &format!(
            "visual-test {LABEL} ④: 別のファイルに及ぶ名前は当てずにヘッダへ段階 1 の理由（{status:?}）"
        ),
    );

    // ⑤ Esc で欄を閉じる（本文は変わらない）
    open_rename(
        any,
        window,
        cx,
        pane,
        VALUE_AT,
        "value",
        LABEL,
        "rename-1686-esc",
    )
    .await;
    retype(any, cx, "value", "zzz");
    press(any, cx, "escape");
    notify_and_draw(any, window, cx);
    let closed = field_text(window, cx).is_none();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑤ closed={closed}");
    check(
        closed && buffer_text(window, cx, pane) == MAIN,
        &format!("visual-test {LABEL} ⑤: Esc で欄を閉じ、本文は変わらない"),
    );
    std::env::remove_var("TAKO_LSP_FAKE_RENAME");
    let _ = std::fs::remove_file(&rules_file);
}

/// visual-test `rename-1686-real`（実の rust-analyzer。読み込みが済んでから ①〜③）
pub(super) async fn rename_1686_real_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "rename-1686-real";
    let spec = tako_core::lsp::servers::resolve_in(
        tako_core::lsp::servers::SERVERS,
        std::path::Path::new("main.rs"),
    )
    .expect("rs を受け持つサーバが表に在る")
    .spec;
    let overridden = std::env::var(tako_core::lsp::servers::override_env_name(spec.id))
        .ok()
        .is_some_and(|p| std::path::Path::new(&p).is_file());
    if !overridden && tako_core::platform::exe::find(spec.program).is_none() {
        println!("TAKO_VISUAL_1686_REAL: SKIPPED（{} が無い）", spec.program);
        return;
    }
    inject_section_failure(LABEL);
    let (pane, _dir, _) = completion_scene(any, window, cx, LABEL, None, MAIN, (8, 0)).await;
    wait_ready(any, window, cx, pane, LABEL, Duration::from_secs(180)).await;
    open_rename(any, window, cx, pane, VALUE_AT, "value", LABEL, LABEL).await;
    rename_and_undo(any, window, cx, pane, LABEL, LABEL, Duration::from_secs(90)).await;
}
