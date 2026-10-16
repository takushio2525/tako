//! visual-test `enter-1759`（FR-3.5 / Issue #1759）: Enter の続き 2 つと括弧の自動閉じを
//! **実キーで** 1 回ずつ打ち、打つ前と打った後の画面を撮る。
//!
//! 打鍵は `window.dispatch_keystroke` へ流すので、キーバインドの判定 → 編集の入口 → 打鍵表
//! （Enter）/ 文字入力の口（`replace_text_in_range` → `EditorCommand::Type`）→ `TextBuffer` の
//! 全段を通る。見るのは状態（本文の行とカーソル）で、画像は人が見て確かめる証拠として
//! `TAKO_VISUAL_DUMP_DIR` へ `enter-1759-<相>-before.png` / `-after.png` で落とす。
//!
//! - ① Python: `        return 1` の行末で Enter → 次の行は 1 段浅い（4 桁）。続けて `y = 2` を打つ
//! - ② Markdown: `9. nine` の行末で Enter → `10. ` が入る（番号の繰り上がり）。`ten` を打って
//!   もう 1 回 Enter → `11. `、さらに Enter → 印だけの項目なので印が消えて抜ける
//! - ③ Rust: `let v = ;` の `;` の手前で `f(x` を打つ → `)` が自動で入って `f(x|);`。
//!   `)` を打つと二重にならずに飛び越して `f(x)|;`
//!
//! 最後に 3 つを保存し、保存後の中身を `TAKO_VISUAL_DUMP_DIR` へ `gui-<名前>` で写す
//! （`scripts/test-editor-enter-1759.sh` が CLI `tako edit` の同じ手順の結果とバイト比較する）。
//! `TAKO_1759_LEGACY=1` は 3 つとも #1654 の挙動へ戻るので ① が FAILED = 同一バイナリでの A/B
//! （画像は判定の前に落とすので、旧挙動の前後も残る）。単独実行は `TAKO_VISUAL_ONLY=enter-1759`

use super::*;

const SECTION: &str = "enter-1759";

/// ① の素材（3 行目 = 0 始まりの 2 行目が抜ける文）
pub(super) const PY: &str = "def f(x):\n    if x:\n        return 1\n";
/// ② の素材（2 行目が番号付きの項目）
pub(super) const MD: &str = "- [x] done\n9. nine\n";
/// ③ の素材（2 行目の `;` の手前 = 12 桁で打つ）
pub(super) const RS: &str = "fn main() {\n    let v = ;\n}\n";

pub(super) async fn enter_1759_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::Request as Req;
    inject_section_failure(SECTION);
    let base = ensure_fresh_scene(window, cx, SECTION).await;
    let legacy = tako_core::text_edit::legacy_1759();
    let dir = std::env::temp_dir().join(format!("tako-visual-1759-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("visual-test enter-1759 一時ディレクトリ");
    let dump_dir = std::env::var("TAKO_VISUAL_DUMP_DIR").ok();

    // 開いて編集モードにし、そのペインを掴む（CLI `tako open` + `tako edit start` と同じ dispatch）
    let open = |cx: &mut AsyncApp, name: &str, body: &str| -> (PaneId, std::path::PathBuf) {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("visual-test enter-1759 の素材");
        let pane = window
            .update(cx, |app, _, cx| {
                app.drawer_visible = false;
                app.panel_visible = false;
                let opened = tako_control::dispatch(
                    app,
                    Req::OpenFile {
                        pane: Some(base.as_u64()),
                        path: path.display().to_string(),
                        mode: Some(tako_control::protocol::PreviewModeWire::Code),
                        direction: None,
                        focus: Some(true),
                        new_tab: true,
                        line: None,
                        column: None,
                    },
                    PaneOrigin::Cli,
                )
                .unwrap_or_else(|e| {
                    fail(&format!("visual-test {SECTION}: {name} を開けない: {e:?}"))
                });
                cx.notify();
                opened["pane"].as_u64().map(PaneId::from_raw)
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test {SECTION}: OpenFile 応答に pane が無い"
                ))
            });
        (pane, path)
    };
    let start_editing = |cx: &mut AsyncApp, pane: PaneId| -> bool {
        window
            .update(cx, |app, _, cx| {
                let _ = app.workspace.active_tab_mut().tree_mut().focus(pane);
                let r = tako_control::dispatch(
                    app,
                    Req::PreviewEdit {
                        pane: Some(pane.as_u64()),
                        enabled: Some(true),
                    },
                    PaneOrigin::Cli,
                );
                cx.notify();
                r.ok().and_then(|v| v["editing"].as_bool()).unwrap_or(false)
            })
            .unwrap_or(false)
    };
    // カーソルを置く（行は 1 始まり。CLI `tako edit cursor L:C` と同じ dispatch）
    let place = |cx: &mut AsyncApp, pane: PaneId, line: usize, col: usize| -> bool {
        let ok = window
            .update(cx, |app, _, cx| {
                let r = tako_control::dispatch(
                    app,
                    Req::PreviewCursor {
                        pane: Some(pane.as_u64()),
                        line,
                        col,
                        select_to_line: None,
                        select_to_col: None,
                        expected_version: None,
                    },
                    PaneOrigin::Cli,
                );
                cx.notify();
                r.is_ok()
            })
            .unwrap_or(false);
        notify_and_draw(any, window, cx);
        ok
    };
    // 本文の 1 行（0 始まり）とカーソル（行 0 始まり, バイト桁）
    let observe = |cx: &mut AsyncApp, pane: PaneId, line: usize| -> (String, (usize, usize)) {
        notify_and_draw(any, window, cx);
        window
            .update(cx, |app, _, _| {
                let Some(edit) = app.preview_edits.get(&pane) else {
                    return (String::new(), (0, 0));
                };
                let b = &edit.buffer;
                (
                    b.text()
                        .split('\n')
                        .nth(line)
                        .unwrap_or_default()
                        .to_string(),
                    b.line_byte_col(b.cursor()),
                )
            })
            .unwrap_or_default()
    };
    let shot = |cx: &mut AsyncApp, name: &str| {
        notify_and_draw(any, window, cx);
        let Some((frame, _)) = capture_frame(any, cx) else {
            println!("TAKO_VISUAL_PIXEL: {SECTION} {name} フレームを撮れない");
            return;
        };
        if let Some(dir) = dump_dir.as_ref() {
            let dir = std::path::Path::new(dir);
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(format!("{SECTION}-{name}.png"));
            if frame.save(&path).is_ok() {
                println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
            }
        }
    };
    let report = |label: &str, seen: &(String, (usize, usize))| {
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} {label} line={:?} cursor={:?} legacy={legacy}",
            seen.0, seen.1
        );
    };
    let wait = |cx: &mut AsyncApp| cx.background_executor().timer(Duration::from_millis(60));

    // ── ① Python: 抜ける文の後ろの Enter は 1 段浅い ──────────────────
    let (py, py_path) = open(cx, "enter.py", PY);
    check(
        wait_for_preview_maps(any, window, cx, py, false).await,
        &format!("visual-test {SECTION}: 前提 = enter.py の座標キャッシュが揃う"),
    );
    check(
        start_editing(cx, py),
        &format!("visual-test {SECTION}: 前提 = enter.py の編集を始められる"),
    );
    check(
        place(cx, py, 3, "        return 1".len()),
        &format!("visual-test {SECTION}: 前提 = return の行末へ置ける"),
    );
    shot(cx, "1-python-before");
    press(any, cx, "enter");
    wait(cx).await;
    type_text(any, cx, "y = 2", false);
    wait(cx).await;
    let seen = observe(cx, py, 3);
    report("python-enter", &seen);
    shot(cx, "1-python-after");
    check(
        seen == ("    y = 2".to_string(), (3, 9)),
        &format!(
            "visual-test {SECTION}: ① return の後ろの Enter で 1 段浅くなる (#1759。{seen:?})"
        ),
    );

    // ── ② Markdown: 番号を進めて印を引き継ぎ、空の項目では抜ける ──────────
    let (md, md_path) = open(cx, "list.md", MD);
    check(
        wait_for_preview_maps(any, window, cx, md, false).await,
        &format!("visual-test {SECTION}: 前提 = list.md の座標キャッシュが揃う"),
    );
    check(
        start_editing(cx, md),
        &format!("visual-test {SECTION}: 前提 = list.md の編集を始められる"),
    );
    check(
        place(cx, md, 2, "9. nine".len()),
        &format!("visual-test {SECTION}: 前提 = 9. の行末へ置ける"),
    );
    shot(cx, "2-markdown-before");
    press(any, cx, "enter");
    wait(cx).await;
    let carried = observe(cx, md, 2);
    report("markdown-enter", &carried);
    type_text(any, cx, "ten", false);
    press(any, cx, "enter");
    wait(cx).await;
    let next = observe(cx, md, 3);
    report("markdown-next", &next);
    press(any, cx, "enter");
    wait(cx).await;
    let exited = observe(cx, md, 3);
    report("markdown-exit", &exited);
    shot(cx, "2-markdown-after");
    check(
        carried == ("10. ".to_string(), (2, 4)),
        &format!(
            "visual-test {SECTION}: ② 9. の後ろの Enter で 10. を引き継ぐ (#1759。{carried:?})"
        ),
    );
    check(
        next == ("11. ".to_string(), (3, 4)),
        &format!("visual-test {SECTION}: ② 続けて 11. を引き継ぐ (#1759。{next:?})"),
    );
    check(
        exited == (String::new(), (3, 0)),
        &format!(
            "visual-test {SECTION}: ② 印だけの項目で Enter を押すと印が消える (#1759。{exited:?})"
        ),
    );

    // ── ③ Rust: 開き括弧で閉じ括弧も入り、入れた閉じ括弧は飛び越す ───────
    let (rs, rs_path) = open(cx, "pair.rs", RS);
    check(
        wait_for_preview_maps(any, window, cx, rs, false).await,
        &format!("visual-test {SECTION}: 前提 = pair.rs の座標キャッシュが揃う"),
    );
    check(
        start_editing(cx, rs),
        &format!("visual-test {SECTION}: 前提 = pair.rs の編集を始められる"),
    );
    check(
        place(cx, rs, 2, "    let v = ".len()),
        &format!("visual-test {SECTION}: 前提 = ; の手前へ置ける"),
    );
    shot(cx, "3-rust-before");
    type_text(any, cx, "f(x", false);
    wait(cx).await;
    let closed = observe(cx, rs, 1);
    report("rust-open", &closed);
    shot(cx, "3-rust-after");
    type_text(any, cx, ")", false);
    wait(cx).await;
    let over = observe(cx, rs, 1);
    report("rust-type-over", &over);
    check(
        closed == ("    let v = f(x);".to_string(), (1, 15)),
        &format!("visual-test {SECTION}: ③ ( で ) も入りカーソルは間 (#1759。{closed:?})"),
    );
    check(
        over == ("    let v = f(x);".to_string(), (1, 16)),
        &format!("visual-test {SECTION}: ③ 入れた ) の手前で ) を打つと飛び越す (#1759。{over:?})"),
    );

    // 保存して中身を写す（CLI の同じ手順とバイト比較するため）
    for (pane, path, name) in [
        (py, &py_path, "enter.py"),
        (md, &md_path, "list.md"),
        (rs, &rs_path, "pair.rs"),
    ] {
        let saved = window
            .update(cx, |app, _, cx| {
                let r = tako_control::dispatch(
                    app,
                    Req::PreviewSave {
                        pane: Some(pane.as_u64()),
                        force: false,
                    },
                    PaneOrigin::Cli,
                );
                cx.notify();
                r.is_ok()
            })
            .unwrap_or(false);
        check(
            saved,
            &format!("visual-test {SECTION}: {name} を保存できる"),
        );
        if let Some(dir) = dump_dir.as_ref() {
            let to = std::path::Path::new(dir).join(format!("gui-{name}"));
            let _ = std::fs::copy(path, &to);
            println!("TAKO_VISUAL_DUMP_FILE: {}", to.display());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
