//! visual-test `xref-1685`（FR-3.42 / Issue #1685。偽サーバ）: 参照検索の結果を右パネルの
//! references ビューへ出し、一覧の行を**実マウスの ⌘クリック**（Windows は Ctrl）で押すと
//! 新しいペインが 1 枚増えて該当行が見えることを確かめる。
//!
//! ① 識別子の右クリックに「参照を検索」が移動の後に出て、押すと右パネルに 2 行（同じファイル /
//!    別のファイル）が出る ② 別のファイルの行を ⌘クリックすると新しいペインが 1 枚増え、そのペインは
//!    別のファイルの 90 行目（ファイルの下の方 = 開いただけでは見えない）が見えている
//!    ③ 問い合わせたペインを閉じると、保持件数の口（`tako lsp results` = dispatch）が 0 を返す
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1685_LEGACY=1` は ①（項目が無い）で名指しの FAILED に
//! なる = 同一バイナリでの A/B。画像は `TAKO_VISUAL_DUMP_DIR` の下へ節の名前つきで落とす

use super::*;

const LABEL: &str = "xref-1685";
/// 別のファイルの参照の行（0 起点。ファイルの下の方 = 開いた直後の先頭からは見えない）
const TARGET_LINE: usize = 89;

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）。落とした絶対パスを出す
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

fn location(path: &std::path::Path, line: u64, character: u64) -> serde_json::Value {
    serde_json::json!({
        "uri": tako_core::file_uri::from_path(path),
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 6 },
        },
    })
}

fn pane_count(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> usize {
    window
        .update(cx, |app, _, _| {
            app.workspace.active_tab().tree().panes().len()
        })
        .unwrap_or(0)
}

fn retained(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> serde_json::Value {
    window
        .update(cx, |app, _, _| {
            tako_control::dispatch(
                app,
                tako_control::protocol::Request::LspResults {
                    action: "results".into(),
                    pane: None,
                    item: None,
                    open: None,
                    focus: None,
                    limit: None,
                },
                PaneOrigin::Cli,
            )
            .map(|out| out["retained"].clone())
            .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// visual-test `xref-1685`（偽サーバ）。相は冒頭の説明 ①〜③
pub(super) async fn xref_1685_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_core::lsp::menu::MenuItem;
    use tako_core::lsp::xref::XrefKind;
    inject_section_failure(LABEL);
    // 偽サーバの参照の規則（場面の置き場は `completion_scene` が作り直すので、ファイルは場面の後に書く）
    let dir = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    remove_on_fail(&dir);
    let main = dir.join("src").join("main.rs");
    let other = dir.join("src").join("other.rs");
    let rules_file = std::env::temp_dir().join(format!(
        "tako-visual-{LABEL}-xref-{}.json",
        std::process::id()
    ));
    remove_on_fail(&rules_file);
    let rules = serde_json::json!([{ "method": "textDocument/references", "result": [
        location(&main, 1, 4),
        location(&other, TARGET_LINE as u64, 7),
    ] }]);
    std::fs::write(&rules_file, rules.to_string()).expect("xref.json");
    std::env::set_var("TAKO_LSP_FAKE_XREF", &rules_file);
    let source = "fn main() {\n    helper();\n}\n";
    let (pane, _dir, _override_env) = completion_scene(
        any,
        window,
        cx,
        LABEL,
        Some(serde_json::json!({ "items": [] })),
        source,
        (1, 0),
    )
    .await;
    let other_body: String = (0..120)
        .map(|i| {
            if i == TARGET_LINE {
                "pub fn helper() {}\n".to_string()
            } else {
                format!("// other {}\n", i + 1)
            }
        })
        .collect();
    std::fs::write(&other, other_body).expect("other.rs");
    check(
        vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
            window
                .update(cx, |app, _, _| app.lsp_hover_enabled_for(pane))
                .unwrap_or(false)
        })
        .await,
        &format!("visual-test {LABEL}: 編集モードで文書がサーバへつながる（素材の前提）"),
    );
    let word = vt1684_point(window, cx, pane, 1, 4..10)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 2 行目の helper の位置")));

    // ① 右クリック → 「参照を検索」（移動の後）→ 押すと右パネルに 2 行
    let refs_id = MenuItem::Xref(XrefKind::References).id();
    vt1860_click(any, window, cx, word, MouseButton::Right);
    check(
        vt1684_settled(any, window, cx, Duration::from_secs(20)).await,
        &format!("visual-test {LABEL} ①: メニューの言語サーバの節が決まる"),
    );
    let (_, drawn) = vt1684_menu(any, window, cx).unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ① menu={drawn:?}");
    dump(any, cx, "xref-1685-menu.png");
    let at = drawn.iter().position(|id| *id == refs_id);
    check(
        at.is_some_and(|at| at > 0 && drawn[at - 1] == "lsp-implementation"),
        &format!(
            "visual-test {LABEL} ①: 識別子の右クリックに「参照を検索」が移動の後に出る ({drawn:?})"
        ),
    );
    let item = vt1684_item_point(window, cx, refs_id)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: 項目の矩形が無い")));
    vt1860_click(any, window, cx, item, MouseButton::Left);
    let shown = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.panel_visible
                    && app.panel_view == PanelView::References
                    && app.lsp_xref.active().is_some_and(|r| r.body.len() == 2)
                    && app
                        .panel_click_probe_bounds
                        .borrow()
                        .contains_key("xref-row-2")
            })
            .unwrap_or(false)
    })
    .await;
    dump(any, cx, "xref-1685-panel.png");
    check(
        shown,
        &format!("visual-test {LABEL} ①: 押すと右パネルの references ビューに 2 行が出る"),
    );

    // ② 別のファイルの行を ⌘クリック → 新しいペインが 1 枚増え、その行が見えている
    let before = pane_count(window, cx);
    let row = window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds
                .borrow()
                .get("xref-row-2")
                .map(|b| b.center())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ②: 行の矩形が無い")));
    link_click(any, window, cx, row);
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
                    .and_then(|lines| lines.get(TARGET_LINE))
                    .and_then(|l| l.clone());
                let height = win.viewport_size().height;
                panes.len() == before + 1
                    && first <= TARGET_LINE
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
        "TAKO_VISUAL_PIXEL: {LABEL} ② panes={before}->{after} first_visible={first:?} target={}",
        TARGET_LINE + 1
    );
    dump(any, cx, "xref-1685-landed.png");
    check(
        landed,
        &format!(
            "visual-test {LABEL} ②: 行の ⌘クリックで新しいペインが 1 枚増え、{} 行目が見える（{before} → {after} 枚・先頭 {first:?}）",
            TARGET_LINE + 1
        ),
    );

    // ③ 問い合わせたペインを閉じる → 保持件数の口が 0
    check(
        retained(window, cx) == serde_json::json!(1),
        &format!("visual-test {LABEL} ③: 閉じる前は結果を 1 つ持つ"),
    );
    let _ = window.update(cx, |app, _, _| {
        tako_control::dispatch(
            app,
            tako_control::protocol::Request::Close {
                pane: Some(pane.as_u64()),
                force: true,
                caller_role: None,
            },
            PaneOrigin::Cli,
        )
    });
    notify_and_draw(any, window, cx);
    let kept = window
        .update(cx, |app, _, _| (app.lsp_xref.len(), app.lsp_xref.rows()))
        .unwrap_or((usize::MAX, usize::MAX));
    let answered = retained(window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ③ store={kept:?} retained={answered}");
    check(
        kept == (0, 0) && answered == serde_json::json!(0),
        &format!(
            "visual-test {LABEL} ③: ペインを閉じたら保持件数 0（置き場 {kept:?} / 口 {answered}）"
        ),
    );
    std::env::remove_var("TAKO_LSP_FAKE_XREF");
    let _ = std::fs::remove_file(&rules_file);
}
