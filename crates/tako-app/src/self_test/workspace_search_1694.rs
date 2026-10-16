//! visual-test `workspace-search-1694`（FR-3.43 / Issue #1694）: ⇧⌘F（Windows は Ctrl+Shift+G）で
//! 右パネルの search ビューを開き、検索欄に打って Enter で探し、結果の行を**実マウスの ⌘クリック**
//! （Windows は Ctrl）で押すと新しいペインが 1 枚増えて該当行が見えることを確かめる。
//!
//! 場面: 一時 dir（git のリポジトリ。`target/` を .gitignore）を cwd にした新しいタブ
//! （= 探す範囲はこの dir だけ。ホームを範囲にしない）。
//! ① ⇧⌘F をキーの配送（キーバインド判定 → `FindInFiles`）へ流すと、右パネルが search ビューで開き
//!    検索欄が打鍵を受ける ② 1 文字ずつ打って Enter → background で探して 2 行（main.rs / other.rs。
//!    .gitignore の `target/` とバイナリは出ない）③ other.rs の行（90 行目 = 開いただけでは見えない）を
//!    ⌘クリック → 新しいペインが 1 枚増え、その行が見えている ④ 見出しの × → 保持 0（`tako search
//!    --results` と同じ口が 0 を返す）。
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ節の名前つきで落とす。単独実行は
//! `TAKO_VISUAL_ONLY=workspace-search-1694`

use super::*;

const LABEL: &str = "workspace-search-1694";
/// other.rs の該当行（1 始まり。ファイルの下の方 = 開いた直後の先頭からは見えない）
const TARGET_LINE: usize = 90;
const NEEDLE: &str = "needle_1694";

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

/// リンクの修飾（macOS は ⌘・それ以外は Ctrl）つきの左クリックを**実 OS マウスと同じ
/// `PlatformInput` 経路**で流す
fn link_click(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    at: Point<Pixels>,
) {
    let modifiers = if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            control: true,
            ..Default::default()
        }
    };
    let send = |cx: &mut AsyncApp, input: gpui::PlatformInput| {
        let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
    };
    send(
        cx,
        gpui::PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers,
        }),
    );
    send(
        cx,
        gpui::PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
    );
    send(
        cx,
        gpui::PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count: 1,
        }),
    );
    notify_and_draw(any, window, cx);
}

fn pane_count(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> usize {
    window
        .update(cx, |app, _, _| {
            app.workspace.active_tab().tree().panes().len()
        })
        .unwrap_or(0)
}

/// `tako search --results` と同じ口の `retained`
fn retained(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> serde_json::Value {
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
                    limit: None,
                    item: None,
                    focus: None,
                },
                PaneOrigin::Cli,
            )
            .map(|out| out["retained"].clone())
            .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// 探す範囲の置き場を作る（git のリポジトリ。`target/` は .gitignore・bin.dat はバイナリ）
fn fixture() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("src");
    std::fs::create_dir_all(dir.join(".git")).expect(".git");
    std::fs::create_dir_all(dir.join("target")).expect("target");
    std::fs::write(dir.join(".gitignore"), "target/\n").expect(".gitignore");
    std::fs::write(
        dir.join("src").join("main.rs"),
        format!("fn main() {{\n    {NEEDLE}();\n}}\n"),
    )
    .expect("main.rs");
    let other: String = (1..=120)
        .map(|i| {
            if i == TARGET_LINE {
                format!("pub fn {NEEDLE}() {{}}\n")
            } else {
                format!("// other {i}\n")
            }
        })
        .collect();
    std::fs::write(dir.join("src").join("other.rs"), other).expect("other.rs");
    std::fs::write(dir.join("target").join("gen.rs"), format!("{NEEDLE}\n")).expect("gen.rs");
    std::fs::write(dir.join("bin.dat"), format!("{NEEDLE}\0\u{1}")).expect("bin.dat");
    tako_core::platform::path::canonicalize(&dir).unwrap_or(dir)
}

/// visual-test `workspace-search-1694`。相は冒頭の説明 ①〜④
pub(super) async fn workspace_search_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    inject_section_failure(LABEL);
    let dir = fixture();
    // cwd = dir の新しいタブだけにする（範囲 = このタブのファイルツリーのルート = dir）
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

    // ① ⇧⌘F（キーの配送 = キーバインド判定 → FindInFiles）
    let spec = if cfg!(target_os = "macos") {
        "cmd-shift-f"
    } else {
        "ctrl-shift-g"
    };
    vt1860_press(any, window, cx, spec);
    let opened = window
        .update(cx, |app, _, _| {
            app.panel_visible && app.panel_view == PanelView::Search && app.search_input_active()
        })
        .unwrap_or(false);
    dump(any, cx, "workspace-search-1694-open.png");
    check(
        opened,
        &format!("visual-test {LABEL} ①: {spec} で search ビューが開き検索欄が打鍵を受ける"),
    );

    // ② 1 文字ずつ打って Enter（検索欄の打鍵経路 = handle_key → handle_search_key）
    let text = NEEDLE.to_string();
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
        win.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
    });
    let typed = window
        .update(cx, |app, _, _| app.search_ui.input.text().to_string())
        .unwrap_or_default();
    check(
        typed == NEEDLE,
        &format!("visual-test {LABEL} ②: 打鍵が検索欄に入る（{typed:?}）"),
    );
    let found = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.workspace_search.running().is_none()
                    && app
                        .workspace_search
                        .shown()
                        .is_some_and(|s| s.outcome.hits.len() == 2)
                    && app
                        .panel_click_probe_bounds
                        .borrow()
                        .contains_key("search-row-2")
            })
            .unwrap_or(false)
    })
    .await;
    let (rows, roots, skipped) = window
        .update(cx, |app, _, _| {
            app.workspace_search
                .shown()
                .map(|s| {
                    (
                        s.outcome
                            .hits
                            .iter()
                            .map(|h| format!("{}:{}", h.rel, h.line))
                            .collect::<Vec<_>>(),
                        s.roots.clone(),
                        s.outcome.skipped,
                    )
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② rows={rows:?} roots={roots:?} skipped={skipped:?}");
    dump(any, cx, "workspace-search-1694-results.png");
    check(
        found,
        &format!("visual-test {LABEL} ②: Enter で 2 行が出る（{rows:?}）"),
    );
    check(
        roots == vec![dir.clone()],
        &format!("visual-test {LABEL} ②: 範囲はタブのツリーのルート = 置き場だけ（{roots:?}）"),
    );
    check(
        skipped.binary == 1 && !rows.iter().any(|r| r.contains("target")),
        &format!("visual-test {LABEL} ②: .gitignore の target/ とバイナリは出ない（{rows:?} / {skipped:?}）"),
    );

    // ③ other.rs の行を ⌘クリック → 新しいペインが 1 枚増え、その行が見えている
    let before = pane_count(window, cx);
    let row = window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds
                .borrow()
                .get("search-row-2")
                .map(|b| b.center())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ③: 行の矩形が無い")));
    link_click(any, window, cx, row);
    let target = TARGET_LINE - 1;
    let landed = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, win, _| {
                let panes = app.workspace.active_tab().tree().panes();
                let Some(new) = panes.iter().map(|p| p.id()).find(|id| {
                    app.previews
                        .get(id)
                        .is_some_and(|p| p.path.ends_with("other.rs"))
                }) else {
                    return false;
                };
                let first = app.preview_first_visible_line(new).unwrap_or(usize::MAX);
                let layout = app
                    .preview_text_layouts
                    .get(&new)
                    .and_then(|lines| lines.get(target))
                    .and_then(|l| l.clone());
                let height = win.viewport_size().height;
                panes.len() == before + 1
                    && first <= target
                    && layout.is_some_and(|l| {
                        l.position_for_index(0)
                            .is_some_and(|p| p.y >= px(0.0) && p.y < height)
                    })
            })
            .unwrap_or(false)
    })
    .await;
    let (after, first) = window
        .update(cx, |app, _, _| {
            let panes = app.workspace.active_tab().tree().panes();
            let first = panes
                .iter()
                .map(|p| p.id())
                .find(|id| {
                    app.previews
                        .get(id)
                        .is_some_and(|p| p.path.ends_with("other.rs"))
                })
                .and_then(|id| app.preview_first_visible_line(id));
            (panes.len(), first)
        })
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ③ panes={before}->{after} first_visible={first:?} target={TARGET_LINE}"
    );
    dump(any, cx, "workspace-search-1694-landed.png");
    check(
        landed,
        &format!(
            "visual-test {LABEL} ③: 行の ⌘クリックで新しいペインが 1 枚増え、{TARGET_LINE} 行目が見える（{before} → {after} 枚・先頭 {first:?}）"
        ),
    );

    // ④ 見出しの × → 保持 0（--results と同じ口）
    check(
        retained(window, cx) == serde_json::json!(2),
        &format!("visual-test {LABEL} ④: 閉じる前は 2 行を持つ"),
    );
    let _ = window.update(cx, |app, _, cx| {
        app.panel_view = PanelView::Search;
        cx.notify();
    });
    notify_and_draw(any, window, cx);
    let close = window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds
                .borrow()
                .get("panel-search-close")
                .map(|b| b.center())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ④: × の矩形が無い")));
    vt1860_click(any, window, cx, close, MouseButton::Left);
    let answered = retained(window, cx);
    let empty = window
        .update(cx, |app, _, _| app.workspace_search.is_empty())
        .unwrap_or(false);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ retained={answered} empty={empty}");
    dump(any, cx, "workspace-search-1694-cleared.png");
    check(
        empty && answered == serde_json::json!(0),
        &format!("visual-test {LABEL} ④: × で保持 0（置き場が空 {empty} / 口 {answered}）"),
    );
    let _ = std::fs::remove_dir_all(&dir);
}
