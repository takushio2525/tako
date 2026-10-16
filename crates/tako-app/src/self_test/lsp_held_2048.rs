//! visual-test `lsp-held-2048`（Issue #2048）: 隔離 GUI（`TAKO_ISOLATED`）で .rs を編集モードにしても
//! 言語サーバを起こさず、タイトルに控えめな 1 行（「<サーバ> 自動起動なし（隔離）」）だけを出す。
//! 帯（本文の高さが変わる）は出さない。
//!
//! 素材は `scripts/test-lsp-isolated-2048.sh` が用意する（偽サーバを rust-analyzer へ差し替え = 旧の腕でも
//! 実の `cargo check` を走らせない）。判定は新しい挙動を無条件に主張する。`TAKO_2048_LEGACY=1` は
//! 「隔離なのに言語サーバを起こした」の名指しの FAILED。画像は `TAKO_VISUAL_DUMP_DIR` へ
//! （新旧で書き出し先を分けるのはスクリプト）

use super::*;

pub(super) async fn lsp_held_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::Request as Req;
    use tako_core::lsp::state::ServerState;
    const LABEL: &str = "lsp-held-2048";
    inject_section_failure(LABEL);
    let anchor = ensure_fresh_scene(window, cx, LABEL).await;
    let dir = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    // fail() の経路でも消える（#2051）
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("visual-test lsp-held-2048 の一時ディレクトリ");
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"v\"\n").expect("Cargo.toml");
    let path = dir.join("src").join("main.rs");
    std::fs::write(&path, "fn main() {\n    println!(\"held\");\n}\n").expect("main.rs");
    let pane = window
        .update(cx, |app, _, cx| {
            let opened = tako_control::dispatch(
                app,
                Req::OpenFile {
                    pane: Some(anchor.as_u64()),
                    path: path.display().to_string(),
                    mode: Some(tako_control::protocol::PreviewModeWire::Code),
                    direction: Some(tako_control::protocol::Direction::Right),
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            )
            .expect("visual-test lsp-held-2048 を dispatch で開ける");
            let pane = PaneId::from_raw(opened["pane"].as_u64().expect("OpenFile 応答の pane"));
            let _ = app.workspace.active_tab_mut().tree_mut().focus(pane);
            tako_control::dispatch(
                app,
                Req::PreviewEdit {
                    pane: Some(pane.as_u64()),
                    enabled: Some(true),
                },
                PaneOrigin::Cli,
            )
            .expect("visual-test lsp-held-2048: 編集モードへ入れる");
            cx.notify();
            pane
        })
        .unwrap_or_else(|_| fail("visual-test lsp-held-2048 dispatch"));
    check(
        wait_for_preview_maps(any, window, cx, pane, false).await,
        &format!("visual-test {LABEL}: 行が描かれる"),
    );

    // 状態が決まるまで（起こす版なら起動中 → 動作中、起こさない版なら最初から止めた）待つ。
    // 起こす版の偽サーバは 1 秒以内に握手する（scripts の ③ の実測）ので、その数倍
    let started = std::time::Instant::now();
    let mut server = None;
    while started.elapsed() < Duration::from_secs(3) {
        notify_and_draw(any, window, cx);
        server = window
            .update(cx, |app, _, _| app.lsp_document_server(pane))
            .ok()
            .flatten();
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    notify_and_draw(any, window, cx);
    let (badge, band, status) = window
        .update(cx, |app, _, _| {
            (
                app.lsp_status_badge(pane),
                app.panel_click_probe_bounds
                    .borrow()
                    .contains_key(&format!("preview-lsp-action-{}", pane.as_u64())),
                app.lsp.status(None),
            )
        })
        .unwrap_or_else(|_| fail("visual-test lsp-held-2048: 状態を読めない"));
    let spawns = status["servers"][0]["spawn_count"].as_u64().unwrap_or(0);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} state={:?} held={:?} badge={badge:?} band={band} spawns={spawns} autostart={}",
        server.as_ref().map(|s| s.state),
        server.as_ref().map(|s| s.held),
        status["autostart"]
    );
    lsp_held_dump(any, cx, "lsp-held-2048.png");
    check(
        spawns == 0,
        &format!(
            "visual-test {LABEL}: 隔離なのに言語サーバを起こした（spawn {spawns} 回・state={:?}）",
            server.as_ref().map(|s| s.state)
        ),
    );
    let server =
        server.unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 受け持つサーバが無い")));
    check(
        server.state == ServerState::Stopped && server.held,
        &format!(
            "visual-test {LABEL}: 器が「隔離で止めた」でない（{:?} held={}）",
            server.state, server.held
        ),
    );
    let expected = crate::ui_text::preview::lsp_held(server.id);
    check(
        badge.as_ref().map(|(text, _)| text.as_str()) == Some(expected.as_str()),
        &format!("visual-test {LABEL}: タイトルの 1 行が「{expected}」でない（{badge:?}）"),
    );
    check(
        !band,
        &format!("visual-test {LABEL}: 帯（ボタン）が出た = 本文の高さが変わる"),
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）。落とした絶対パスを出す
fn lsp_held_dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
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
