//! visual-test `click-forward` / `click-forward-claude`（FR-2.5.15 / Issue #1961）: マウス報告を
//! 要求する TUI へ、実 OS マウスと同じ `PlatformInput` 経路（GPUI のヒットテストとリスナー配線まで
//! 通る = #738）で押した左クリック・ドラッグが届くか。修飾・設定・フォーカス移動・マウスを
//! 要求しないアプリでは今までどおり tako の選択になるか。
//!
//! `click-forward`（合成のアプリ。`TAKO_PERSIST=1` なら器のペイン = 同じ節を両方で回す）:
//! ① 1002 + 1006 を要求する可視化アプリ（受けたバイトの ESC を行頭の `^[` にして見せる）を
//!    クリック → 押す・離すが届く
//! ② ドラッグしてペインの外（左隣のペイン）で離す → 移動が届き、外は最寄りのセル（0 桁）に丸まる
//! ③ ダブルクリック → 押す・離すが 2 組
//! ④ Option + ドラッグ / Shift + ドラッグ → 届かず tako の選択（選択の文字列が空でない）
//! ⑤ cmd（Windows は Ctrl）+ クリック → 届かない（リンクは tako が取る）
//! ⑥ 設定 off → 届かず選択・on へ戻すとすぐ届く
//! ⑦ フォーカスを移すクリック → 届かない・次のクリックは届く
//! ⑧ マウスを要求しないアプリ → 届かず tako の選択・画面に制御列が出ない
//! ⑨ 1000 だけ（X10 の素の形式）のアプリ → 素の形式（`^[[M`）で届く
//!
//! `click-forward-claude`（実物の Claude Code。`scripts/test-click-forward-1961.sh` が偽の
//! Messages API と一時の設定 dir で起こし、会話を画面より長くして帯に split right を足しておく）:
//! ① ホイールで遡ると出る「Jump to bottom」を押す → 最下部へ戻って消える
//! ② mod の帯の `compact` を押す → 会話が圧縮され、報告の last_press が slash / 成功
//! ③ mod の帯の `split right` を押す → ペインが増え、報告の last_press が tako / 成功
//! ④ AI が出したコマンドカード（#1963。会話の行に描かれ、押す口は帯の tako の行）の `r: run` を押す →
//!    新しいペインで走り、報告の last_press が card_run / 成功。続けて `x: close` を押す → card_dismiss / 成功
//!    （Copy は本物のクリップボードへ書くので押さない）
//!
//! 押す順は ① → ③ → ④ → ②。compact は最後に押す（圧縮の後は要約が最後の発話に混ざり、偽 API が
//! 以後の依頼を要約の依頼と読むので、④ のカードが出なくなる）
//!
//! **tako の選択を作る場面は離すを「窓の外で離した」形（ボタンなしの移動）で終える**:
//! 素の離すは copy-on-select で本物のクリップボードへ書くため（隔離 GUI でも同じ）。
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ落とす（`TAKO_1961_LEGACY=1` は ① で FAILED に
//! なるので、旧挙動の 1 枚が残る = 同一バイナリでの A/B）

use super::*;

const SECTION: &str = "click-forward";
const SECTION_CLAUDE: &str = "click-forward-claude";

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn screen_of(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> String {
    window
        .update(cx, |app, _, _| {
            app.terminals
                .get(&pane)
                .map(|s| s.visible_lines().join("\n"))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

async fn wait_screen(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    budget_ms: u64,
    pred: impl Fn(&str) -> bool,
) -> (bool, String) {
    let started = std::time::Instant::now();
    loop {
        let screen = screen_of(window, cx, pane);
        if pred(&screen) {
            return (true, screen);
        }
        if started.elapsed() > Duration::from_millis(budget_ms) {
            return (false, screen);
        }
        sleep(cx, 50).await;
    }
}

/// セル（col は小数 = 文字列の途中を押すため）の中心の窓座標
fn cell_center(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    col: f32,
    row: usize,
) -> Point<Pixels> {
    window
        .update(cx, |app, _, _| {
            let area = app
                .pane_text_areas
                .iter()
                .find(|(id, _)| *id == pane)
                .map(|(_, b)| *b)?;
            let cell = app.cell_size_for_pane(pane)?;
            Some(point(
                area.origin.x + px(f32::from(cell.width) * (col + 0.5)),
                area.origin.y + px(f32::from(cell.height) * (row as f32 + 0.5)),
            ))
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {SECTION}: ペイン {pane} の矩形が無い"
            ))
        })
}

fn text_area_left(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> Pixels {
    window
        .update(cx, |app, _, _| {
            app.pane_text_areas
                .iter()
                .find(|(id, _)| *id == pane)
                .map(|(_, b)| b.origin.x)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {SECTION}: ペイン {pane} の矩形が無い"
            ))
        })
}

fn send(any: AnyWindowHandle, cx: &mut AsyncApp, input: gpui::PlatformInput) {
    let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
}

/// ポインタを動かして 1 フレーム描く（hitbox の hover はフレーム構築時の判定 = `click_at` と同じ）
fn hover_at(any: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }),
    );
    let _ = capture_frame(any, cx);
}

fn down(
    any: AnyWindowHandle,
    cx: &mut AsyncApp,
    position: Point<Pixels>,
    modifiers: Modifiers,
    click_count: usize,
) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count,
            first_mouse: false,
        }),
    );
}

fn drag(any: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>, modifiers: Modifiers) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers,
        }),
    );
}

fn up(
    any: AnyWindowHandle,
    cx: &mut AsyncApp,
    position: Point<Pixels>,
    modifiers: Modifiers,
    click_count: usize,
) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count,
        }),
    );
}

/// 窓の外で離した形（`MouseUp` が届かずボタンなしの移動が来る）。tako の選択は
/// copy-on-select せずに畳まれる（本物のクリップボードを汚さない）
fn up_lost(any: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }),
    );
}

async fn click(
    any: AnyWindowHandle,
    cx: &mut AsyncApp,
    position: Point<Pixels>,
    modifiers: Modifiers,
) {
    hover_at(any, cx, position);
    down(any, cx, position, modifiers, 1);
    sleep(cx, 30).await;
    up(any, cx, position, modifiers, 1);
}

/// 最後に押したクリックの行き先（`渡したか:理由`）
fn last_route(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> String {
    window
        .update(cx, |app, _, _| {
            app.click_stats
                .last
                .map(|(pane, forwarded, reason)| format!("{pane}:{forwarded}:{reason}"))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn press_count(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> u64 {
    window
        .update(cx, |app, _, _| app.click_stats.press)
        .unwrap_or(0)
}

/// tako の選択の文字列（無ければ空）と、選択中のペイン
fn selection(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> (String, bool) {
    window
        .update(cx, |app, _, _| {
            (
                app.terminals
                    .get(&pane)
                    .and_then(|s| s.selection_text())
                    .unwrap_or_default(),
                app.selecting == Some(pane),
            )
        })
        .unwrap_or_default()
}

/// 器のペインの問い合わせ（background）が終わるまで待つ
async fn wait_resolved(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    for _ in 0..100 {
        let resolving = window
            .update(cx, |app, _, _| {
                matches!(
                    app.click_forward,
                    Some(crate::click_forward::ClickForward::Resolving { .. })
                )
            })
            .unwrap_or(false);
        if !resolving {
            return;
        }
        sleep(cx, 30).await;
    }
}

fn dispatch_req(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    request: tako_control::protocol::Request,
) -> serde_json::Value {
    window
        .update(cx, |app, _, cx| {
            let mut result = tako_control::dispatch(app, request, PaneOrigin::Cli);
            let _ = app.after_dispatch(&mut result, false, cx);
            cx.notify();
            result
        })
        .ok()
        .and_then(Result::ok)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: dispatch が通らない")))
}

fn split_with(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    from: PaneId,
    direction: tako_control::protocol::Direction,
    command: Vec<String>,
) -> PaneId {
    let reply = dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Split {
            pane: Some(from.as_u64()),
            tab: None,
            direction: Some(direction),
            ratio: None,
            command: Some(command),
            cwd: None,
            focus: Some(true),
        },
    );
    reply["pane"]
        .as_u64()
        .map(PaneId::from_raw)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 分割の応答に pane が無い")))
}

fn focus(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) {
    dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Focus {
            pane: Some(pane.as_u64()),
            direction: None,
        },
    );
}

fn set_forward(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, enabled: bool) {
    dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Scrollback {
            lines: None,
            unfocused_fps: None,
            mouse_forward: Some(enabled),
        },
    );
}

/// 受けたバイトを 1 イベント 1 行で見せる可視化アプリ（ESC → 改行 + `^[`）。
/// `request` はマウスの要求（printf の書式）。要求を出してから `VIS-READY` を出すので、
/// それが見えた時点で要求は端末（器）に届いている
fn visualizer(request: &str) -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        format!(
            r#"stty raw -echo; printf '{request}VIS-READY\r\n'; exec perl -e '$|=1; while (sysread(STDIN,$b,4096)) {{ $b =~ s/\x1b/\r\n^[/g; syswrite(STDOUT,$b) }}'"#
        ),
    ]
}

fn dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    if let Some((frame, _)) = capture_frame(any, cx) {
        let _ = frame.save(std::path::Path::new(&dir).join(name));
    }
}

fn link_mods() -> Modifiers {
    Modifiers {
        platform: cfg!(target_os = "macos"),
        control: !cfg!(target_os = "macos"),
        ..Modifiers::default()
    }
}

pub(super) async fn click_forward_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::Direction;
    let none = Modifiers::default();
    let root = ensure_fresh_scene(window, cx, SECTION).await;
    let vis = split_with(
        window,
        cx,
        root,
        Direction::Right,
        visualizer(r"\033[?1002h\033[?1006h"),
    );
    let (ready, screen) = wait_screen(window, cx, vis, 20_000, |s| s.contains("VIS-READY")).await;
    check(
        ready,
        &format!("visual-test {SECTION}: 前提 = 可視化アプリが起きる。画面: {screen:?}"),
    );
    let tmux = window
        .update(cx, |app, _, _| app.mirror_scroll_pane(vis))
        .unwrap_or(false);
    let mode = if tmux { "tmux" } else { "direct" };
    println!(
        "TAKO_VISUAL_1961: mode={mode} legacy={}",
        tako_core::mouse_forward::legacy()
    );
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        sleep(cx, 50).await;
    }

    // ① クリック（押す・離す）
    let p = cell_center(window, cx, vis, 5.0, 3);
    click(any, cx, p, none).await;
    let (ok, screen) = wait_screen(window, cx, vis, 5_000, |s| {
        s.contains("^[[<0;6;4M") && s.contains("^[[<0;6;4m")
    })
    .await;
    let name = if tako_core::mouse_forward::legacy() {
        format!("click-forward-{mode}-legacy.png")
    } else {
        format!("click-forward-{mode}-click.png")
    };
    dump(any, cx, &name);
    check(
        ok,
        &format!(
            "visual-test {SECTION} ①: 全画面 TUI へのクリックが届かない（{mode}・最後の行き先 {}）。画面: {screen:?}",
            last_route(window, cx)
        ),
    );
    check(
        last_route(window, cx).ends_with(":true:forwarded"),
        &format!("visual-test {SECTION} ①: 行き先 {}", last_route(window, cx)),
    );

    // ② ドラッグしてペインの外（左隣のペイン）で離す。人の手の速さで動かす
    let a = cell_center(window, cx, vis, 7.0, 5);
    hover_at(any, cx, a);
    down(any, cx, a, none, 1);
    for col in [10.0, 14.0] {
        sleep(cx, 120).await;
        let to = cell_center(window, cx, vis, col, 5);
        drag(any, cx, to, none);
    }
    sleep(cx, 120).await;
    let outside = point(text_area_left(window, cx, vis) - px(60.0), a.y);
    drag(any, cx, outside, none);
    sleep(cx, 120).await;
    up(any, cx, outside, none, 1);
    let want = ["^[[<32;11;6M", "^[[<32;15;6M", "^[[<32;1;6M", "^[[<0;1;6m"];
    let (ok, screen) = wait_screen(window, cx, vis, 5_000, |s| {
        want.iter().all(|w| s.contains(w))
    })
    .await;
    dump(any, cx, &format!("click-forward-{mode}-drag.png"));
    check(
        ok,
        &format!(
            "visual-test {SECTION} ②: ドラッグ（外は 0 桁に丸める）が届かない。期待 {want:?} 画面: {screen:?}"
        ),
    );

    // ③ ダブルクリック
    let d = cell_center(window, cx, vis, 9.0, 7);
    hover_at(any, cx, d);
    down(any, cx, d, none, 1);
    up(any, cx, d, none, 1);
    down(any, cx, d, none, 2);
    up(any, cx, d, none, 2);
    let (ok, screen) = wait_screen(window, cx, vis, 5_000, |s| {
        s.matches("^[[<0;10;8M").count() == 2 && s.matches("^[[<0;10;8m").count() == 2
    })
    .await;
    check(
        ok,
        &format!("visual-test {SECTION} ③: ダブルクリックの 2 組が届かない。画面: {screen:?}"),
    );

    // ④ Option + ドラッグ / Shift + ドラッグ = tako の選択（届かない）
    for (label, mods, row) in [
        (
            "option",
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
            9,
        ),
        (
            "shift",
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            11,
        ),
    ] {
        let before = press_count(window, cx);
        let from = cell_center(window, cx, vis, 0.0, row);
        hover_at(any, cx, from);
        down(any, cx, from, mods, 1);
        sleep(cx, 50).await;
        let to = cell_center(window, cx, vis, 12.0, row + 1);
        drag(any, cx, to, mods);
        sleep(cx, 300).await;
        let (text, selecting) = selection(window, cx, vis);
        let route = last_route(window, cx);
        if label == "option" {
            dump(any, cx, &format!("click-forward-{mode}-option-select.png"));
        }
        up_lost(any, cx, from);
        let screen = screen_of(window, cx, vis);
        check(
            route.ends_with(":false:bypass_modifier") && selecting && !text.trim().is_empty(),
            &format!(
                "visual-test {SECTION} ④ {label}: tako の選択にならない（行き先 {route}・選択中 {selecting}・選択 {text:?}）"
            ),
        );
        check(
            press_count(window, cx) == before
                && !screen.contains(&format!("^[[<0;1;{}M", row + 1)),
            &format!("visual-test {SECTION} ④ {label}: 修飾つきのドラッグが TUI へ届いた。画面: {screen:?}"),
        );
    }

    // ⑤ cmd（Windows は Ctrl）+ クリック = tako（リンクの判定）
    let before = press_count(window, cx);
    let c = cell_center(window, cx, vis, 3.0, 13);
    hover_at(any, cx, c);
    down(any, cx, c, link_mods(), 1);
    up_lost(any, cx, c);
    sleep(cx, 300).await;
    let screen = screen_of(window, cx, vis);
    check(
        last_route(window, cx).ends_with(":false:link_modifier")
            && press_count(window, cx) == before
            && !screen.contains("^[[<0;4;14M")
            && !screen.contains("^[[<16;4;14M"),
        &format!(
            "visual-test {SECTION} ⑤: cmd + クリックが TUI へ届いた（行き先 {}）",
            last_route(window, cx)
        ),
    );

    // ⑥ 設定 off → 届かない / on → すぐ届く
    set_forward(window, cx, false);
    let o = cell_center(window, cx, vis, 5.0, 15);
    hover_at(any, cx, o);
    down(any, cx, o, none, 1);
    up_lost(any, cx, o);
    sleep(cx, 300).await;
    let off_route = last_route(window, cx);
    check(
        off_route.ends_with(":false:disabled")
            && !screen_of(window, cx, vis).contains("^[[<0;6;16M"),
        &format!("visual-test {SECTION} ⑥: 設定 off でも届いた（行き先 {off_route}）"),
    );
    set_forward(window, cx, true);
    let on = cell_center(window, cx, vis, 6.0, 16);
    click(any, cx, on, none).await;
    let (ok, screen) = wait_screen(window, cx, vis, 5_000, |s| s.contains("^[[<0;7;17M")).await;
    check(
        ok,
        &format!("visual-test {SECTION} ⑥: on へ戻した直後のクリックが届かない。画面: {screen:?}"),
    );

    // ⑦ フォーカスを移すクリックは届かない・次のクリックは届く
    focus(window, cx, root);
    sleep(cx, 100).await;
    let f = cell_center(window, cx, vis, 4.0, 18);
    hover_at(any, cx, f);
    down(any, cx, f, none, 1);
    up_lost(any, cx, f);
    sleep(cx, 300).await;
    let focus_route = last_route(window, cx);
    check(
        focus_route.ends_with(":false:focus_change")
            && !screen_of(window, cx, vis).contains("^[[<0;5;19M"),
        &format!(
            "visual-test {SECTION} ⑦: フォーカスを移すクリックが届いた（行き先 {focus_route}）"
        ),
    );
    click(any, cx, f, none).await;
    let (ok, screen) = wait_screen(window, cx, vis, 5_000, |s| s.contains("^[[<0;5;19M")).await;
    check(
        ok,
        &format!("visual-test {SECTION} ⑦: フォーカス後のクリックが届かない。画面: {screen:?}"),
    );

    // ⑧ マウスを要求しないアプリ = tako の選択（制御列を打たない）
    let plain = split_with(window, cx, vis, Direction::Down, visualizer(""));
    let (ready, screen) = wait_screen(window, cx, plain, 20_000, |s| s.contains("VIS-READY")).await;
    check(
        ready,
        &format!("visual-test {SECTION} ⑧: 前提 = 可視化アプリが起きる。画面: {screen:?}"),
    );
    for _ in 0..4 {
        notify_and_draw(any, window, cx);
        sleep(cx, 50).await;
    }
    let before = press_count(window, cx);
    let s0 = cell_center(window, cx, plain, 0.0, 0);
    hover_at(any, cx, s0);
    down(any, cx, s0, none, 1);
    sleep(cx, 50).await;
    // 終点はペイン上端の余白（DRAG_SCROLL_MARGIN = 40px）の外へ置く。余白の中だと既存の
    // オートスクロール（#310）が選択を (0, 0) へ縮める（#1961 の主題ではない）
    let to = cell_center(window, cx, plain, 6.0, 4);
    drag(any, cx, to, none);
    wait_resolved(window, cx).await;
    sleep(cx, 200).await;
    let (text, selecting) = selection(window, cx, plain);
    let route = last_route(window, cx);
    up_lost(any, cx, s0);
    sleep(cx, 300).await;
    let screen = screen_of(window, cx, plain);
    check(
        route.ends_with(":false:not_requested")
            && selecting
            && !text.trim().is_empty()
            && press_count(window, cx) == before
            && !screen.contains("^[["),
        &format!(
            "visual-test {SECTION} ⑧: マウスを要求しないアプリの挙動が変わった（行き先 {route}・選択中 {selecting}・選択 {text:?}）。画面: {screen:?}"
        ),
    );

    // ⑨ 1000 だけ（X10 の素の形式）
    let x10 = split_with(
        window,
        cx,
        plain,
        Direction::Right,
        visualizer(r"\033[?1000h"),
    );
    let (ready, screen) = wait_screen(window, cx, x10, 20_000, |s| s.contains("VIS-READY")).await;
    check(
        ready,
        &format!("visual-test {SECTION} ⑨: 前提 = 可視化アプリが起きる。画面: {screen:?}"),
    );
    for _ in 0..4 {
        notify_and_draw(any, window, cx);
        sleep(cx, 50).await;
    }
    let x = cell_center(window, cx, x10, 2.0, 2);
    click(any, cx, x, none).await;
    // 押す = `ESC [ M` + (32+0) (32+3) (32+3) = " ##"、離す = ボタン 3 = "###"
    let (ok, screen) = wait_screen(window, cx, x10, 5_000, |s| {
        s.contains("^[[M ##") && s.contains("^[[M###")
    })
    .await;
    dump(any, cx, &format!("click-forward-{mode}-final.png"));
    check(
        ok,
        &format!("visual-test {SECTION} ⑨: X10 の素の形式で届かない。画面: {screen:?}"),
    );

    let stats = window
        .update(cx, |app, _, _| app.click_stats.clone())
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_1961: {SECTION} mode={mode} ok press={} drag={} release={}",
        stats.press, stats.drag, stats.release
    );
}

/// 画面から文字列を探し、その中ほどのセル（col, row）を返す（全角の幅も `cell_cols` で引く）。
/// `row_marker` を渡すと、その文字列を含む行（帯の行）の中だけで探す
fn find_label(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
    row_marker: Option<&str>,
) -> Option<(f32, usize)> {
    window
        .update(cx, |app, _, _| {
            let session = app.terminals.get(&pane)?;
            let screen = session.screen(&app.theme);
            screen.lines.iter().enumerate().find_map(|(row, line)| {
                if row_marker.is_some_and(|m| !line.text.contains(m)) {
                    return None;
                }
                let byte = line.text.find(label)?;
                let start = line.text[..byte].chars().count();
                let mid = start + label.chars().count() / 2;
                let col = *line.cell_cols.get(mid)?;
                Some((col as f32, row))
            })
        })
        .ok()
        .flatten()
}

fn last_press(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> String {
    window
        .update(cx, |app, _, _| {
            // 一次ソースの引き当ては `claude_mod::lookup` の 1 本（#1880 の番犬）
            tako_core::claude_mod::lookup(
                Some(&app.claude_mod),
                pane.as_u64(),
                std::time::Instant::now(),
                false,
            )
            .ok()
            .and_then(|stored| stored.report.last_press.as_ref())
            .map(|p| format!("{}/{}@{}", p.kind, p.ok, p.at))
            .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn pane_count(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> usize {
    window
        .update(cx, |app, _, _| {
            app.workspace
                .tabs()
                .iter()
                .map(|t| t.tree().panes().len())
                .sum()
        })
        .unwrap_or(0)
}

fn pane_ids(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> Vec<PaneId> {
    window
        .update(cx, |app, _, _| {
            app.workspace
                .tabs()
                .iter()
                .flat_map(|t| t.tree().panes())
                .map(|p| p.id())
                .collect()
        })
        .unwrap_or_default()
}

/// 実物の Claude Code（`click-forward-claude`）。スクリプトが用意し終えるのを画面で待つ
pub(super) async fn click_forward_claude_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let none = Modifiers::default();
    let ready_text = std::env::var("TAKO_1961_READY_TEXT").unwrap_or_else(|_| "LINE-80".into());
    // claude のペイン = 帯に split right があり、長い答えの最後の行が見えているペイン
    let started = std::time::Instant::now();
    let pane = loop {
        let found = window
            .update(cx, |app, _, _| {
                app.terminals.iter().find_map(|(id, s)| {
                    let text = s.visible_lines().join("\n");
                    (text.contains("split right") && text.contains(&ready_text)).then_some(*id)
                })
            })
            .ok()
            .flatten();
        if let Some(pane) = found {
            break pane;
        }
        if started.elapsed() > Duration::from_secs(300) {
            fail(&format!(
                "visual-test {SECTION_CLAUDE}: 前提 = claude の帯に split right と {ready_text} が出ない"
            ));
        }
        sleep(cx, 500).await;
    };
    focus(window, cx, pane);
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        sleep(cx, 100).await;
    }
    println!(
        "TAKO_VISUAL_1961: {SECTION_CLAUDE} pane={pane} tmux={} legacy={}",
        window
            .update(cx, |app, _, _| app.mirror_scroll_pane(pane))
            .unwrap_or(false),
        tako_core::mouse_forward::legacy()
    );

    // ① ホイールで遡る（ホイールは #1961 前から TUI へ渡る）→「Jump to bottom」を押す
    let center = cell_center(window, cx, pane, 20.0, 5);
    hover_at(any, cx, center);
    let mut jump = None;
    for _ in 0..20 {
        send(
            any,
            cx,
            gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: center,
                delta: ScrollDelta::Lines(point(0.0, 3.0)),
                modifiers: Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            }),
        );
        sleep(cx, 150).await;
        jump = find_label(window, cx, pane, "Jump to bottom", None);
        if jump.is_some() {
            break;
        }
    }
    let Some((col, row)) = jump else {
        fail(&format!(
            "visual-test {SECTION_CLAUDE} ①: 遡っても Jump to bottom が出ない。画面: {:?}",
            screen_of(window, cx, pane)
        ));
    };
    sleep(cx, 300).await;
    dump(any, cx, "click-forward-claude-jump-before.png");
    let target = cell_center(window, cx, pane, col, row);
    click(any, cx, target, none).await;
    let (ok, screen) = wait_screen(window, cx, pane, 10_000, |s| {
        !s.contains("Jump to bottom") && s.contains(&ready_text)
    })
    .await;
    let name = if tako_core::mouse_forward::legacy() {
        "click-forward-claude-legacy.png"
    } else {
        "click-forward-claude-jump-after.png"
    };
    dump(any, cx, name);
    check(
        ok,
        &format!(
            "visual-test {SECTION_CLAUDE} ①: Jump to bottom を押しても最下部へ戻らない（行き先 {}）。画面: {screen:?}",
            last_route(window, cx)
        ),
    );

    // ③ 帯の split right
    let mut target = None;
    for _ in 0..40 {
        target = find_label(window, cx, pane, "split right", None);
        if target.is_some() {
            break;
        }
        sleep(cx, 250).await;
    }
    let Some((col, row)) = target else {
        fail(&format!(
            "visual-test {SECTION_CLAUDE} ③: 帯に split right が無い。画面: {:?}",
            screen_of(window, cx, pane)
        ));
    };
    let before_panes = pane_count(window, cx);
    let panes_before_split = pane_ids(window, cx);
    // 描かれて 300 ms 以内のクリックは Claude Code が捨てる（④ の説明）ので、1 秒置いてから押す
    sleep(cx, 1_000).await;
    let (col, row) = find_label(window, cx, pane, "split right", None).unwrap_or((col, row));
    let at = cell_center(window, cx, pane, col, row);
    click(any, cx, at, none).await;
    let mut after_panes = before_panes;
    for _ in 0..80 {
        after_panes = pane_count(window, cx);
        if after_panes > before_panes {
            break;
        }
        sleep(cx, 250).await;
    }
    for _ in 0..4 {
        notify_and_draw(any, window, cx);
        sleep(cx, 100).await;
    }
    dump(any, cx, "click-forward-claude-split.png");
    check(
        after_panes > before_panes,
        &format!(
            "visual-test {SECTION_CLAUDE} ③: split right のボタンを押してもペインが増えない（{before_panes} のまま・行き先 {}）",
            last_route(window, cx)
        ),
    );
    let mut press_split = String::new();
    for _ in 0..60 {
        press_split = last_press(window, cx, pane);
        if press_split.starts_with("tako/true") {
            break;
        }
        sleep(cx, 500).await;
    }
    check(
        press_split.starts_with("tako/true"),
        &format!(
            "visual-test {SECTION_CLAUDE} ③: 報告の last_press が tako / 成功でない: {press_split:?}"
        ),
    );

    // ④ AI が出したコマンドカード（#1963）の Run を帯から押す。カードの押す口は帯の tako の行
    // （会話の行はカードの表示と実行記録）。Copy は本物のクリップボードへ書くので押さない
    // split で増えたペインを閉じて幅を戻し、claude へフォーカスを戻す（フォーカス移動のクリックは渡らない）
    let split = pane_ids(window, cx)
        .into_iter()
        .find(|id| !panes_before_split.contains(id));
    if let Some(split) = split {
        dispatch_req(
            window,
            cx,
            tako_control::protocol::Request::Close {
                pane: Some(split.as_u64()),
                force: true,
                caller_role: None,
                caller_pane: None,
            },
        );
    }
    focus(window, cx, pane);
    sleep(cx, 500).await;
    let card = r#"Show this card: {"commands":["echo CLICK-RUN-1961"],"label":"click test"}"#;
    let _ = window.update(cx, |app, _, _| {
        if let Some(s) = app.terminals.get(&pane) {
            s.write(card.as_bytes().to_vec());
        }
    });
    sleep(cx, 1_000).await;
    let _ = window.update(cx, |app, _, _| {
        if let Some(s) = app.terminals.get(&pane) {
            s.write(b"\r".to_vec());
        }
    });
    let (ok, screen) = wait_screen(window, cx, pane, 60_000, |s| s.contains("CARD-DONE")).await;
    dump(any, cx, "click-forward-claude-card-wait.png");
    check(
        ok,
        &format!("visual-test {SECTION_CLAUDE} ④: 前提 = AI がカードを出す（CARD-DONE）。画面: {screen:?}"),
    );
    let mut run = None;
    for _ in 0..80 {
        for _ in 0..2 {
            notify_and_draw(any, window, cx);
        }
        run = find_label(window, cx, pane, "r: run", Some("card"));
        if run.is_some() {
            break;
        }
        sleep(cx, 250).await;
    }
    let Some((col, row)) = run else {
        fail(&format!(
            "visual-test {SECTION_CLAUDE} ④: 帯にカードの r: run が無い。画面: {:?}",
            screen_of(window, cx, pane)
        ));
    };
    dump(any, cx, "click-forward-claude-card-before.png");
    let before_run = pane_count(window, cx);
    let panes_before_run = pane_ids(window, cx);
    // Claude Code の押せる部品は**描かれて（mount されて）から 300 ms 以内のクリックを迷いクリックとして捨てる**
    // （2.1.294 の `mount_settle`）。帯は報告のたびに描き直されうるので、見えてから 1 秒置いて押し、
    // 効かなければ位置を取り直して押し直す（人の手のクリックも出た瞬間には押さない）
    let mut after_run = before_run;
    let mut run_tries = 0;
    let mut target = Some((col, row));
    while let Some((col, row)) = target {
        run_tries += 1;
        sleep(cx, 1_000).await;
        let (col, row) = find_label(window, cx, pane, "r: run", Some("card")).unwrap_or((col, row));
        let at = cell_center(window, cx, pane, col, row);
        click(any, cx, at, none).await;
        for _ in 0..24 {
            after_run = pane_count(window, cx);
            if after_run > before_run {
                break;
            }
            sleep(cx, 250).await;
        }
        target = (after_run == before_run && run_tries < 3).then_some((col, row));
    }
    let mut card_press = String::new();
    for _ in 0..60 {
        card_press = last_press(window, cx, pane);
        if card_press.starts_with("card_run/true") {
            break;
        }
        sleep(cx, 500).await;
    }
    for _ in 0..4 {
        notify_and_draw(any, window, cx);
        sleep(cx, 100).await;
    }
    dump(any, cx, "click-forward-claude-card-run.png");
    check(
        after_run > before_run && card_press.starts_with("card_run/true"),
        &format!(
            "visual-test {SECTION_CLAUDE} ④: カードの Run を押しても走らない（ペイン {before_run} → {after_run}・last_press {card_press:?}・行き先 {}）",
            last_route(window, cx)
        ),
    );

    // Run で増えたペインを閉じて幅を戻し、帯の「閉じる」（x: close）もクリックしてカードを畳む
    // （帯に compact / split right が収まるように戻す。カードを閉じるボタンの検証も兼ねる）
    if let Some(run_pane) = pane_ids(window, cx)
        .into_iter()
        .find(|id| !panes_before_run.contains(id))
    {
        dispatch_req(
            window,
            cx,
            tako_control::protocol::Request::Close {
                pane: Some(run_pane.as_u64()),
                force: true,
                caller_role: None,
                caller_pane: None,
            },
        );
    }
    focus(window, cx, pane);
    sleep(cx, 500).await;
    let mut close = None;
    for _ in 0..80 {
        for _ in 0..2 {
            notify_and_draw(any, window, cx);
        }
        close = find_label(window, cx, pane, "x: close", Some("card"));
        if close.is_some() {
            break;
        }
        sleep(cx, 250).await;
    }
    let Some((col, row)) = close else {
        fail(&format!(
            "visual-test {SECTION_CLAUDE} ④: 帯にカードの x: close が無い。画面: {:?}",
            screen_of(window, cx, pane)
        ));
    };
    // 出てから 1 秒置いて押し、効かなければ押し直す（上の Run と同じ理由 = mount 直後の 300 ms は捨てられる）
    let mut dismiss_press = String::new();
    let mut close_tries = 0;
    let mut target = Some((col, row));
    while let Some((col, row)) = target {
        close_tries += 1;
        sleep(cx, 1_000).await;
        let (col, row) =
            find_label(window, cx, pane, "x: close", Some("card")).unwrap_or((col, row));
        let at = cell_center(window, cx, pane, col, row);
        click(any, cx, at, none).await;
        for _ in 0..20 {
            dismiss_press = last_press(window, cx, pane);
            if dismiss_press.starts_with("card_dismiss/true") {
                break;
            }
            sleep(cx, 500).await;
        }
        target = (!dismiss_press.starts_with("card_dismiss/true") && close_tries < 3)
            .then_some((col, row));
    }
    check(
        dismiss_press.starts_with("card_dismiss/true"),
        &format!(
            "visual-test {SECTION_CLAUDE} ④: カードの「閉じる」を押しても畳まれない（last_press {dismiss_press:?}・行き先 {}）",
            last_route(window, cx)
        ),
    );
    // 帯が描き直されて compact が戻るのを待つ（Esc は送らない = Claude Code では 2 回で rewind）
    for _ in 0..40 {
        for _ in 0..2 {
            notify_and_draw(any, window, cx);
        }
        if find_label(window, cx, pane, "compact", Some("split right")).is_some() {
            break;
        }
        sleep(cx, 250).await;
    }

    // ② 帯の compact（最後に押す: 圧縮の後は要約が最後の発話に混ざり、偽 API が以後の依頼を
    // 要約の依頼と読む = ④ のカードが出なくなる）
    // 帯の行（split right を含む行）の中の compact（ヒント等の「/compact」を押さない）
    let Some((col, row)) = find_label(window, cx, pane, "compact", Some("split right")) else {
        fail(&format!(
            "visual-test {SECTION_CLAUDE} ②: 帯に compact が無い。画面: {:?}",
            screen_of(window, cx, pane)
        ));
    };
    let before = last_press(window, cx, pane);
    // 描かれて 300 ms 以内のクリックは Claude Code が捨てる（④ の説明）ので、1 秒置いてから押す
    sleep(cx, 1_000).await;
    let (col, row) =
        find_label(window, cx, pane, "compact", Some("split right")).unwrap_or((col, row));
    let at = cell_center(window, cx, pane, col, row);
    click(any, cx, at, none).await;
    let (ok, screen) = wait_screen(window, cx, pane, 120_000, |s| s.contains("Compacted")).await;
    dump(any, cx, "click-forward-claude-compact.png");
    check(
        ok,
        &format!(
            "visual-test {SECTION_CLAUDE} ②: compact のボタンを押しても圧縮されない（行き先 {}）。画面: {screen:?}",
            last_route(window, cx)
        ),
    );
    let mut press_compact = String::new();
    for _ in 0..60 {
        press_compact = last_press(window, cx, pane);
        if press_compact != before && press_compact.starts_with("slash/true") {
            break;
        }
        sleep(cx, 500).await;
    }
    check(
        press_compact.starts_with("slash/true"),
        &format!(
            "visual-test {SECTION_CLAUDE} ②: 報告の last_press が slash / 成功でない: {press_compact:?}"
        ),
    );

    println!(
        "TAKO_VISUAL_1961: {SECTION_CLAUDE} ok panes={before_panes}->{after_panes} split={press_split} card_run_panes={before_run}->{after_run} card={card_press} run_tries={run_tries} dismiss={dismiss_press} close_tries={close_tries} compact={press_compact}"
    );
}
