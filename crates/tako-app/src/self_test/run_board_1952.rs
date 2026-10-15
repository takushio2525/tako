//! visual-test `run-board-1952`（FR-3.18.8 / Issue #1952。エピック #1943 の S5）: 実行ボタン横のドロップダウンの
//! 「基板」「ポート」の欄を**実マウス・実キーで**操作し、`arduino-cli board attach` が `sketch.yaml` を書き、
//! 言語サーバが起き直すことを確かめる。arduino-cli は**本物**（Arduino IDE の同梱）を使う。
//!
//! 素材は `scripts/test-run-board-1952.sh` が用意する（一時 HOME に複製した arduino:avr のコア・一時のスケッチ）:
//! - `TAKO_1952_SKETCH` = 基板を選ぶスケッチのフォルダ（`<名前>/<名前>.ino`・sketch.yaml なし）
//! - `TAKO_1952_REF` = 同じ arduino-cli で `board attach -b arduino:avr:uno -p <PORT>` を直に打ったスケッチ
//!   （**バイト比較の正解**。tako は書式を真似しないので、同じバイトになるはず）
//!
//! - ① `.ino` を編集モードで開く → 一覧（background の `RunResolve`）に `board` 欄が届く・基板は未選択・
//!   言語サーバは unconfigured（A/B: `TAKO_1952_LEGACY=1` は欄が無いのでここで名指しの FAILED）
//! - ② シェブロンを実マウスで押す → 基板とポートの節が描かれる（画像 `open`。① の判定の前に撮る = 旧の腕にも残る）
//! - ③ 「基板を探す」を押して実キーで `uno` → 入っているコアの基板が絞られる（画像 `search`）→ `arduino:avr:uno` の
//!   行を押す → sketch.yaml に `default_fqbn`
//! - ④ 「ポートを指定」を押して実キーでポートを打って Enter → sketch.yaml に `default_port`（画像 `port`）
//! - ⑤ sketch.yaml が `TAKO_1952_REF`（arduino-cli が直に書いたもの）とバイト一致
//! - ⑥ 言語サーバが unconfigured から起きる（starting / running）
//! - ⑦ 境界: コアの入っていない基板（`arduino:renesas_uno:unor4wifi`）を打って Enter → 導入のコマンドが欄の下に出て、
//!   sketch.yaml は 1 バイトも変わらない（画像 `core-missing`）
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ `run-board-1952-<節>.png`。単独実行は `TAKO_VISUAL_ONLY=run-board-1952`

use super::*;

const LABEL: &str = "run-board-1952";
/// ④ で打つポート（繋がっていない基板でも選べる = 実在は問わない）
const PORT: &str = "/dev/cu.usbmodem-tako";
const MISSING_CORE_BOARD: &str = "arduino:renesas_uno:unor4wifi";

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
    let path = dir.join(format!("{LABEL}-{name}.png"));
    if frame.save(&path).is_ok() {
        println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
    }
}

fn probe_bounds(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
) -> Option<gpui::Bounds<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds.borrow().get(key).copied()
        })
        .ok()
        .flatten()
}

/// メニューの中の行を、メニュー本体を実ホイールで送って見える位置まで出す
fn reveal(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp, key: &str) {
    if !key.starts_with("run-menu-") {
        return;
    }
    for _ in 0..40 {
        notify_and_draw(any, window, cx);
        let (Some(menu), Some(b)) = (
            probe_bounds(window, cx, "run-menu"),
            probe_bounds(window, cx, key),
        ) else {
            return;
        };
        let top = menu.top() + px(6.0);
        let bottom = menu.bottom() - px(6.0);
        if b.top() >= top && b.bottom() <= bottom {
            return;
        }
        let dy = if b.bottom() > bottom {
            -(b.bottom() - bottom + px(12.0))
        } else {
            top - b.top() + px(12.0)
        };
        let at = menu.center();
        let _ = any.update(cx, |_, win, cx| {
            win.dispatch_event(
                gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: at,
                    delta: ScrollDelta::Pixels(point(px(0.0), dy)),
                    ..ScrollWheelEvent::default()
                }),
                cx,
            );
        });
    }
}

/// 実マウスで押す（矩形が無ければ名指しで落とす）
fn click_probe(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
    step: &str,
) {
    notify_and_draw(any, window, cx);
    reveal(any, window, cx, key);
    let at = probe_bounds(window, cx, key)
        .map(|b| b.center())
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} {step}: `{key}` の矩形が無い")));
    click_at(any, cx, at);
    notify_and_draw(any, window, cx);
}

fn view_of(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<crate::run_menu_ui::RunView> {
    window
        .update(cx, |app, _, _| app.run_menu.view(pane).cloned())
        .ok()
        .flatten()
}

/// 書き込み（background の `RunnerDefaults { board / port }`）と、その後の取り直しが済むまで待つ
async fn settle_attach(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) {
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(30), &|cx| {
        window
            .update(cx, |app, _, _| app.run_menu.attaching.is_none())
            .unwrap_or(false)
    })
    .await;
    let _ = wait_run_view(any, window, cx, pane, Duration::from_secs(30)).await;
}

fn lsp_state(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<tako_core::lsp::state::ServerState> {
    window
        .update(cx, |app, _, _| {
            app.lsp_document_server(pane).map(|s| s.state)
        })
        .ok()
        .flatten()
}

fn yaml(dir: &std::path::Path) -> Option<Vec<u8>> {
    std::fs::read(dir.join("sketch.yaml")).ok()
}

fn edit_notice(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> Option<String> {
    window
        .update(cx, |app, _, _| {
            app.run_menu
                .edit
                .as_ref()
                .and_then(|e| e.notice.clone())
                .or_else(|| app.run_menu.notice.as_ref().map(|(_, n)| n.clone()))
        })
        .ok()
        .flatten()
}

pub(super) async fn run_board_1952_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_core::lsp::state::ServerState;
    let env_dir = |key: &str| {
        std::env::var(key)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                fail(&format!(
                    "visual-test {LABEL}: {key} が無い（素材はスクリプトが用意する）"
                ))
            })
    };
    let sketch = env_dir("TAKO_1952_SKETCH");
    let reference = env_dir("TAKO_1952_REF");
    let name = sketch
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ino = sketch.join(format!("{name}.ino"));
    let legacy = tako_core::sketch_board::legacy();
    println!(
        "TAKO_VISUAL_1952: legacy={legacy} yaml_before={:?}",
        yaml(&sketch).map(|b| b.len())
    );

    // ① 編集モードで開く → board 欄が届く・未選択・言語サーバは unconfigured
    let pv = window
        .update(cx, |app, _, cx| {
            app.drawer_visible = false;
            app.panel_visible = false;
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Welcome {
                    action: Some("dismiss".into()),
                },
                PaneOrigin::Cli,
            );
            let root = app.workspace.active_tab().tree().focused();
            let opened = tako_control::dispatch(
                app,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(root.as_u64()),
                    path: ino.display().to_string(),
                    mode: Some(tako_control::protocol::PreviewModeWire::Code),
                    direction: Some(tako_control::protocol::Direction::Right),
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            );
            let pane = opened
                .ok()
                .and_then(|v| v["pane"].as_u64())
                .map(PaneId::from_raw)?;
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::PreviewEdit {
                    pane: Some(pane.as_u64()),
                    enabled: Some(true),
                },
                PaneOrigin::Cli,
            );
            app.drain_pending_highlights(cx);
            cx.notify();
            Some(pane)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: スケッチを開けない")));
    check(
        wait_run_view(any, window, cx, pv, Duration::from_secs(40)).await,
        &format!("visual-test {LABEL} ①: 一覧（RunResolve）が届く"),
    );
    let view = view_of(window, cx, pv).unwrap_or_default();
    let board = view.board.clone();
    // 言語サーバの器が状態を持つまで少し待つ（開いた直後は器がまだ無い）
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(10), &|cx| {
        lsp_state(window, cx, pv).is_some()
    })
    .await;
    let state_before = lsp_state(window, cx, pv);
    println!(
        "TAKO_VISUAL_1952: ① board={:?} installed={} ports={:?} lsp={state_before:?}",
        board.as_ref().and_then(|b| b.fqbn.clone()),
        board.as_ref().map_or(0, |b| b.installed.len()),
        board.as_ref().and_then(|b| b.ports.as_ref().map(Vec::len)),
    );
    // ② シェブロンを実マウスで押す → 基板とポートの節（判定の前に開いて撮る = A/B の旧の腕にも
    // 欄の無いメニューの画像が 1 枚残る）
    click_probe(
        any,
        window,
        cx,
        &format!("preview-run-dd-{}", pv.as_u64()),
        "②",
    );
    let opened = vt1860_wait(any, window, cx, Duration::from_secs(5), &|cx| {
        window
            .update(cx, |app, _, _| {
                let probes = app.panel_click_probe_bounds.borrow();
                app.run_menu.is_open(pv)
                    && probes.contains_key("run-menu-board-current")
                    && probes.contains_key("run-menu-port-pick")
            })
            .unwrap_or(false)
    })
    .await;
    reveal(any, window, cx, "run-menu-board-search");
    dump(any, window, cx, "open");
    check(
        board.is_some(),
        &format!(
            "visual-test {LABEL} ①: `.ino` の一覧に基板とポートの欄（board）がある（legacy={legacy}）"
        ),
    );
    let board = board.unwrap_or_default();
    check(
        board.fqbn.is_none() && board.installed.iter().any(|b| b.fqbn == "arduino:avr:uno"),
        &format!(
            "visual-test {LABEL} ①: 未選択・入っているコアの基板に arduino:avr:uno（{} 件）",
            board.installed.len()
        ),
    );
    check(
        state_before == Some(ServerState::Unconfigured),
        &format!(
            "visual-test {LABEL} ①: 基板が無いので言語サーバは unconfigured（{state_before:?}）"
        ),
    );
    check(
        opened,
        &format!("visual-test {LABEL} ②: シェブロンで基板とポートの節が描かれる"),
    );

    // ③ 基板を探す → 実キーで uno → 行を押す
    click_probe(any, window, cx, "run-menu-board-search", "③");
    type_text(any, cx, "uno", false);
    notify_and_draw(any, window, cx);
    let rows = crate::run_menu_ui::filter_boards(&board.installed, "uno", 8);
    let index = rows
        .iter()
        .position(|b| b.fqbn == "arduino:avr:uno")
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {LABEL} ③: 絞り込みに arduino:avr:uno が無い"
            ))
        });
    let key = format!("run-menu-board-match-{index}");
    reveal(any, window, cx, &key);
    dump(any, window, cx, "search");
    click_probe(any, window, cx, &key, "③");
    settle_attach(any, window, cx, pv).await;
    let after_board = yaml(&sketch);
    let view = view_of(window, cx, pv).unwrap_or_default();
    println!(
        "TAKO_VISUAL_1952: ③ yaml={:?} view.fqbn={:?}",
        after_board.as_deref().map(String::from_utf8_lossy),
        view.board.as_ref().and_then(|b| b.fqbn.clone())
    );
    check(
        after_board
            .as_deref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains("default_fqbn: arduino:avr:uno")),
        &format!("visual-test {LABEL} ③: 行を押すと sketch.yaml に default_fqbn が書かれる"),
    );
    check(
        view.board
            .as_ref()
            .is_some_and(|b| b.fqbn.as_deref() == Some("arduino:avr:uno")),
        &format!("visual-test {LABEL} ③: 取り直した一覧の今の基板が arduino:avr:uno"),
    );

    // ④ ポートを指定 → 実キー → Enter
    if !window
        .update(cx, |app, _, _| app.run_menu.is_open(pv))
        .unwrap_or(false)
    {
        click_probe(
            any,
            window,
            cx,
            &format!("preview-run-dd-{}", pv.as_u64()),
            "④",
        );
    }
    click_probe(any, window, cx, "run-menu-port-pick", "④");
    type_text(any, cx, PORT, true);
    settle_attach(any, window, cx, pv).await;
    reveal(any, window, cx, "run-menu-port-0");
    dump(any, window, cx, "port");
    let after_port = yaml(&sketch);
    check(
        after_port
            .as_deref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains(&format!("default_port: {PORT}"))),
        &format!(
            "visual-test {LABEL} ④: Enter で sketch.yaml に default_port（{:?}）",
            after_port.as_deref().map(String::from_utf8_lossy)
        ),
    );

    // ⑤ arduino-cli が直に書いたものとバイト一致
    let want = yaml(&reference);
    println!(
        "TAKO_VISUAL_1952: ⑤ ours={:?} ref={:?}",
        after_port.as_ref().map(Vec::len),
        want.as_ref().map(Vec::len)
    );
    check(
        want.is_some() && after_port == want,
        &format!("visual-test {LABEL} ⑤: ドロップダウンで書いた sketch.yaml が arduino-cli の直書きとバイト一致"),
    );

    // ⑥ 言語サーバが起きる
    let woke = vt1860_wait(any, window, cx, Duration::from_secs(30), &|cx| {
        matches!(
            lsp_state(window, cx, pv),
            Some(ServerState::Starting | ServerState::Running)
        )
    })
    .await;
    let state_after = lsp_state(window, cx, pv);
    println!("TAKO_VISUAL_1952: ⑥ lsp={state_after:?}");
    check(
        woke,
        &format!("visual-test {LABEL} ⑥: 基板を選ぶと言語サーバが起きる（{state_before:?} → {state_after:?}）"),
    );

    // ⑦ 境界: コアの入っていない基板 = 導入のコマンドを示すだけ・sketch.yaml は変わらない
    if !window
        .update(cx, |app, _, _| app.run_menu.is_open(pv))
        .unwrap_or(false)
    {
        click_probe(
            any,
            window,
            cx,
            &format!("preview-run-dd-{}", pv.as_u64()),
            "⑦",
        );
    }
    click_probe(any, window, cx, "run-menu-board-search", "⑦");
    type_text(any, cx, MISSING_CORE_BOARD, true);
    settle_attach(any, window, cx, pv).await;
    let notice = edit_notice(window, cx);
    reveal(any, window, cx, "run-menu-input-notice");
    dump(any, window, cx, "core-missing");
    println!("TAKO_VISUAL_1952: ⑦ notice={notice:?}");
    check(
        notice
            .as_deref()
            .is_some_and(|n| n.contains("core install arduino:renesas_uno")),
        &format!("visual-test {LABEL} ⑦: コアが無い基板は導入のコマンドを示す（{notice:?}）"),
    );
    check(
        yaml(&sketch) == after_port,
        &format!(
            "visual-test {LABEL} ⑦: コアが無い基板を選んでも sketch.yaml は 1 バイトも変わらない"
        ),
    );
    press(any, cx, "escape");
    let _ = window.update(cx, |app, _, cx| {
        app.close_run_menu();
        cx.notify();
    });
}
