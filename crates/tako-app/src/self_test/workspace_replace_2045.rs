//! visual-test `workspace-replace-2045`（FR-3.43 の置換 / Issue #2045）: 右パネルの search ビューで
//! 置換欄を開き、プレビュー → 1 行の置換 → すべて置換 → 元に戻すを**実マウスのクリックと実打鍵**で
//! 通し、ディスクの中身と置き場の状態（`tako search --results` と同じ口）を各段で確かめる。
//!
//! 場面: 一時 dir（git のリポジトリ）を cwd にした新しいタブ（範囲 = この dir だけ）。
//! ① ⇧⌘F → 検索語を打って Enter → 4 行（a.rs 2 行・b.rs 2 行）
//! ② 検索欄の左の > を押して置換欄を開き、置換後を 1 文字ずつ打つ → 行に差分のプレビューが並ぶ
//!    （ディスクは 1 バイトも変わらない）
//! ③ 1 行目の「置換」を押す → a.rs の 1 行目だけが変わる・新しいペインは開かない（行のクリックへ漏れない）
//! ④ 「すべて置換」を押す → 残りの 3 行が変わる
//! ⑤ 「元に戻す」を押す → ④ の分だけ戻る、もう一度押す → 元の中身へ戻る
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ節の名前つきで落とす。単独実行は
//! `TAKO_VISUAL_ONLY=workspace-replace-2045`

use super::*;

const LABEL: &str = "workspace-replace-2045";
const NEEDLE: &str = "old_name_2045";
const REPLACEMENT: &str = "new_name";
const A_RS: &str = "fn old_name_2045() {}\nfn main() {\n    old_name_2045();\n}\n";
const B_RS: &str = "use crate::old_name_2045;\r\npub fn call() { old_name_2045() }\r\n";

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）
fn dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
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

/// `tako search --results` と同じ口の応答
fn results(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> serde_json::Value {
    window
        .update(cx, |app, _, _| {
            tako_control::dispatch(
                app,
                tako_control::protocol::Request::WorkspaceSearch {
                    action: "results".into(),
                    query: None,
                    path: None,
                    pane: None,
                    regex: None,
                    case_sensitive: None,
                    show: None,
                    limit: Some(0),
                    item: None,
                    focus: None,
                    replacement: None,
                    file: None,
                    confirm: None,
                },
                PaneOrigin::Cli,
            )
            .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// 状態の 1 行（行ごとの置換の状態・取り消しの履歴の数）
fn state_line(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> String {
    let out = results(window, cx);
    let rows: Vec<String> = out["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            format!(
                "{}:{}",
                i["id"],
                i["replace_status"].as_str().unwrap_or("-")
            )
        })
        .collect();
    format!(
        "rows={rows:?} replacement={} history={}",
        out["replace"]["replacement"],
        out["replace"]["history"].as_array().map_or(0, Vec::len)
    )
}

fn probe_center(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
) -> Option<Point<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds
                .borrow()
                .get(key)
                .map(|b| b.center())
        })
        .ok()
        .flatten()
}

fn pane_count(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> usize {
    window
        .update(cx, |app, _, _| {
            app.workspace.active_tab().tree().panes().len()
        })
        .unwrap_or(0)
}

fn fixture() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("src");
    std::fs::create_dir_all(dir.join(".git")).expect(".git");
    std::fs::write(dir.join("src").join("a.rs"), A_RS).expect("a.rs");
    std::fs::write(dir.join("src").join("b.rs"), B_RS).expect("b.rs");
    tako_core::platform::path::canonicalize(&dir).unwrap_or(dir)
}

fn read(dir: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(dir.join("src").join(name)).unwrap_or_default()
}

fn type_text(any: AnyWindowHandle, cx: &mut AsyncApp, text: &str, enter: bool) {
    let text = text.to_string();
    let _ = any.update(cx, |_, win, cx| {
        for ch in text.chars() {
            win.dispatch_keystroke(
                Keystroke {
                    modifiers: Modifiers::default(),
                    key: ch.to_string(),
                    key_char: Some(ch.to_string()),
                },
                cx,
            );
        }
        if enter {
            win.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
        }
    });
}

/// 置き場の取り消しの履歴の数
fn history(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> usize {
    window
        .update(cx, |app, _, _| {
            app.workspace_search.replace().history().len()
        })
        .unwrap_or(usize::MAX)
}

/// ボタンを押して、`done` が真になるまで待つ（押す前に矩形があることも見る）
async fn press(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
    step: &str,
    done: &dyn Fn(&mut AsyncApp) -> bool,
) -> bool {
    notify_and_draw(any, window, cx);
    let at = probe_center(window, cx, key)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} {step}: {key} の矩形が無い")));
    vt1860_click(any, window, cx, at, MouseButton::Left);
    vt1860_wait(any, window, cx, Duration::from_secs(20), done).await
}

/// visual-test `workspace-replace-2045`。相は冒頭の説明 ①〜⑤
pub(super) async fn workspace_replace_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    inject_section_failure(LABEL);
    let dir = fixture();
    let pane = window
        .update(cx, |app, _, cx| {
            let created = tako_control::dispatch(
                app,
                tako_control::protocol::Request::TabNew {
                    title: Some(LABEL.into()),
                    focus: Some(true),
                    cwd: Some(dir.display().to_string()),
                },
                PaneOrigin::User,
            );
            let _ = app.attach_pending_sessions(cx);
            let keep = app.workspace.active_tab().id();
            let others: Vec<TabId> = app
                .workspace
                .tabs()
                .iter()
                .map(|t| t.id())
                .filter(|id| *id != keep)
                .collect();
            for tab in others {
                app.remove_tab(tab, cx);
            }
            app.sync_filetree_roots();
            cx.notify();
            created
                .ok()
                .and_then(|v| v["pane"].as_u64())
                .map(PaneId::from_raw)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 新しいタブを作れない")));
    check(
        wait_for_pane_ready(window, cx, pane, Duration::from_secs(15)).await,
        &format!("visual-test {LABEL}: 前提 = 新しいタブのプロンプトが出る"),
    );

    // ① ⇧⌘F → 検索語 → Enter
    let spec = if cfg!(target_os = "macos") {
        "cmd-shift-f"
    } else {
        "ctrl-shift-g"
    };
    vt1860_press(any, window, cx, spec);
    type_text(any, cx, NEEDLE, true);
    let found = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.workspace_search.running().is_none()
                    && app
                        .workspace_search
                        .shown()
                        .is_some_and(|s| s.outcome.hits.len() == 4)
            })
            .unwrap_or(false)
    })
    .await;
    check(found, &format!("visual-test {LABEL} ①: 検索で 4 行が出る"));

    // ② > で置換欄を開いて打つ → 差分のプレビューが行に並ぶ（ディスクは変わらない）
    notify_and_draw(any, window, cx);
    let toggle = probe_center(window, cx, "search-replace-toggle")
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ②: > の矩形が無い")));
    vt1860_click(any, window, cx, toggle, MouseButton::Left);
    let focused = window
        .update(cx, |app, _, _| {
            app.search_ui.replace_open && app.search_ui.replace_focused && app.search_input_active()
        })
        .unwrap_or(false);
    check(
        focused,
        &format!("visual-test {LABEL} ②: > で置換欄が開き打鍵を受ける"),
    );
    type_text(any, cx, REPLACEMENT, false);
    let previewed = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.workspace_search.replace().preview().is_some_and(|p| {
                    p.replacement == REPLACEMENT
                        && p.rows.len() == 4
                        && p.rows.values().all(|r| {
                            matches!(r, tako_core::workspace_replace::RowPreview::Ready(_))
                        })
                }) && app
                    .panel_click_probe_bounds
                    .borrow()
                    .contains_key("search-replace-row-1")
            })
            .unwrap_or(false)
    })
    .await;
    notify_and_draw(any, window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② {}", state_line(window, cx));
    dump(any, cx, "workspace-replace-2045-preview.png");
    check(
        previewed && read(&dir, "a.rs") == A_RS && read(&dir, "b.rs") == B_RS,
        &format!(
            "visual-test {LABEL} ②: 打った置換後で 4 行のプレビューが並び、ディスクは変わらない"
        ),
    );

    // ③ 1 行目の「置換」→ a.rs の 1 行目だけ・新しいペインは開かない
    let before = pane_count(window, cx);
    let one = press(any, window, cx, "search-replace-row-1", "③", &|cx| {
        history(window, cx) == 1
    })
    .await;
    let a_after_one = read(&dir, "a.rs");
    notify_and_draw(any, window, cx);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ③ {} panes={before}->{}",
        state_line(window, cx),
        pane_count(window, cx)
    );
    dump(any, cx, "workspace-replace-2045-one.png");
    check(
        one && a_after_one == A_RS.replacen(NEEDLE, REPLACEMENT, 1)
            && read(&dir, "b.rs") == B_RS
            && pane_count(window, cx) == before,
        &format!("visual-test {LABEL} ③: 行の「置換」で a.rs の 1 行目だけが変わり、行は開かない（{a_after_one:?}）"),
    );

    // ④ 「すべて置換」→ 残りの 3 行（CRLF は残る）。当てた後のプレビューが置き直されるのを待つ
    // （1 行目は当て済み・残り 3 行が当てられる）
    let refreshed = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.workspace_search.replace().preview().is_some_and(|p| {
                    p.rows
                        .values()
                        .filter(|r| matches!(r, tako_core::workspace_replace::RowPreview::Ready(_)))
                        .count()
                        == 3
                })
            })
            .unwrap_or(false)
    })
    .await;
    check(
        refreshed,
        &format!("visual-test {LABEL} ④: 当てた後のプレビューが残り 3 行で置き直される"),
    );
    let all = press(any, window, cx, "search-replace-all", "④", &|cx| {
        history(window, cx) == 2
    })
    .await;
    notify_and_draw(any, window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ {}", state_line(window, cx));
    dump(any, cx, "workspace-replace-2045-all.png");
    check(
        all && read(&dir, "a.rs") == A_RS.replace(NEEDLE, REPLACEMENT)
            && read(&dir, "b.rs") == B_RS.replace(NEEDLE, REPLACEMENT),
        &format!("visual-test {LABEL} ④: 「すべて置換」で残りの行が変わる（CRLF のまま）"),
    );

    // ⑤ 「元に戻す」→ ④ の分だけ戻る → もう一度 → 元の中身
    let undo_all = press(any, window, cx, "search-replace-undo", "⑤", &|cx| {
        history(window, cx) == 1
    })
    .await;
    notify_and_draw(any, window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑤-1 {}", state_line(window, cx));
    dump(any, cx, "workspace-replace-2045-undo.png");
    check(
        undo_all && read(&dir, "a.rs") == a_after_one && read(&dir, "b.rs") == B_RS,
        &format!("visual-test {LABEL} ⑤: 「元に戻す」で「すべて置換」の分だけ戻る"),
    );
    let undo_one = press(any, window, cx, "search-replace-undo", "⑤", &|cx| {
        history(window, cx) == 0
    })
    .await;
    notify_and_draw(any, window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑤-2 {}", state_line(window, cx));
    dump(any, cx, "workspace-replace-2045-undo-all.png");
    check(
        undo_one && read(&dir, "a.rs") == A_RS && read(&dir, "b.rs") == B_RS,
        &format!("visual-test {LABEL} ⑤: もう一度で元の中身へ戻る"),
    );
    let _ = std::fs::remove_dir_all(&dir);
}
