//! visual-test `drag-autoscroll`（Issue #2025 / #2153 / #310）: ドラッグ選択のオートスクロールが、
//! 送れる履歴が無いときに選択の端を (0, 0) へ寄せないか・最下部で行末へ寄せないか。実 OS マウスと
//! 同じ `PlatformInput` 経路（GPUI のヒットテストとリスナー配線まで通る = #738）で押してドラッグし、
//! ペイン上下端の余白（`DRAG_SCROLL_MARGIN` = 40px）の中でマウスを止めてティックを回す。
//!
//! 場面ごとに新しいペインを立てて閉じる（行を刷る小さなアプリ。マウスは要求しない = tako の選択）:
//! ① 履歴が無い → 選択はドラッグした範囲のまま（修正前は (0, 0) へ縮む）
//! ② 履歴が 1 画面未満 → a. 最上端まで送って選択が先頭行まで伸びる（#310）
//!    b. 最上端に着いた後にマウスを動かす → その位置のまま（修正前は (0, 0) へ戻される）
//! ③ 履歴が 1 画面以上 → 送りながら選択が履歴へ伸びる（#310 のまま）
//! ④ alt screen（全画面 TUI）→ オートスクロールは立たず、選択はドラッグした範囲のまま
//! ⑤ 最下部（履歴あり）で最下行の 5 桁目に止める → 選択はその位置のまま（#2153。修正前は行末へ寄る）
//! ⑥ 履歴の途中（3 行上）→ a. 最下部まで送って選択が最下行の行末まで伸びる（#310）
//!    b. 最下部に着いた後にマウスを動かす → その位置のまま（#2153。修正前は行末へ戻される）
//!
//! 器のペイン（`TAKO_PERSIST=1`）では ①④⑤ だけを判定し、②③ は観察を出し、⑥ は飛ばす（器の外側の
//! 端末は履歴を持たないので #310 の送りがそもそも働かない = この Issue の前からの形）。
//! 判定は最後にまとめて落とす（同一バイナリの A/B: `TAKO_2025_LEGACY=1` は ①・②b・⑤・⑥b、
//! `TAKO_2153_LEGACY=1` は ⑤・⑥b だけで落ちる）。
//! **離すは「窓の外で離した」形で終える**（素の離すは copy-on-select で本物のクリップボードへ書く）

use super::*;

const SECTION: &str = "drag-autoscroll";
/// 各行の目印。8 桁目（0 始まり）が `e` = 端を 8 桁目に置くと選択は `efgh…` から始まる
const TAIL: &str = "abcdefghijklmnop";

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
    needle: &str,
) -> String {
    let started = std::time::Instant::now();
    loop {
        let screen = screen_of(window, cx, pane);
        if screen.contains(needle) {
            return screen;
        }
        if started.elapsed() > Duration::from_millis(budget_ms) {
            fail(&format!(
                "visual-test {SECTION}: 前提 = 画面に {needle:?} が出る。画面: {screen:?}"
            ));
        }
        sleep(cx, 50).await;
    }
}

/// 窓座標。横は `col` セル幅ぶんの位置そのまま（8.2 = 8 桁目の左半分 = 端は 8 桁目を含む）、
/// 縦は `row` 行目の中心
fn cell_at(
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
                area.origin.x + px(f32::from(cell.width) * col),
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

fn send(any: AnyWindowHandle, cx: &mut AsyncApp, input: gpui::PlatformInput) {
    let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
}

fn mouse_move(any: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>, pressed: bool) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: pressed.then_some(MouseButton::Left),
            modifiers: Modifiers::default(),
        }),
    );
}

fn down(any: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>) {
    send(
        any,
        cx,
        gpui::PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
    );
}

/// ペインの状態（選択の文字列・表示位置・履歴・alt screen・オートスクロールの速度係数）
#[derive(Debug, Default)]
struct Probe {
    text: String,
    offset: usize,
    history: usize,
    alt: bool,
    /// `drag_scroll` の速度係数（None = オートスクロールが立っていない）
    speed: Option<f32>,
}

fn probe(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> Probe {
    window
        .update(cx, |app, _, _| {
            let Some(s) = app.terminals.get(&pane) else {
                return Probe::default();
            };
            Probe {
                text: s.selection_text().unwrap_or_default(),
                offset: s.display_offset(),
                history: s.history_size(),
                alt: s.is_alt_screen(),
                speed: app
                    .drag_scroll
                    .as_ref()
                    .filter(|d| d.pane == pane && !d.preview)
                    .map(|d| d.speed_factor),
            }
        })
        .unwrap_or_default()
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

/// 行を刷る小さなアプリ。起動で `TOP-…` を 1 行、以後は 1 行の命令ごとに刷る:
/// `lines N` = `L001-…`〜`LN` と `DONE-N` / `bot N` = N 行の後に改行なしの `BOT-…`（最下行を埋める）/
/// `alt N` = alt screen へ入って `ALT-…` と N 行。マウスは要求しない（= tako の選択）
fn printer() -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        format!(
            r#"stty -echo; printf 'TOP-{TAIL}\n'; while read c n; do case "$c" in alt) printf '\033[?1049h\033[HALT-{TAIL}\n';; esac; i=1; while [ "$i" -le "$n" ]; do printf 'L%03d-{TAIL}\n' "$i"; i=$((i+1)); done; case "$c" in bot) printf 'BOT-{TAIL}';; *) printf 'DONE-%s\n' "$n";; esac; done"#
        ),
    ]
}

/// 新しいペインを立てて `TOP-…` が出るまで待ち、(ペイン, 行数) を返す
async fn open_printer(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    root: PaneId,
) -> (PaneId, usize) {
    let reply = dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Split {
            pane: Some(root.as_u64()),
            tab: None,
            direction: Some(tako_control::protocol::Direction::Right),
            ratio: None,
            command: Some(printer()),
            cwd: None,
            focus: Some(true),
        },
    );
    let pane = reply["pane"]
        .as_u64()
        .map(PaneId::from_raw)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 分割の応答に pane が無い")));
    wait_screen(window, cx, pane, 20_000, &format!("TOP-{TAIL}")).await;
    // 寸法が落ち着くまで描く（行数は PTY の寸法 = 命令の行数の計算に使う）
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        sleep(cx, 50).await;
    }
    let rows = window
        .update(cx, |app, _, _| app.terminals.get(&pane).map(|s| s.size().1))
        .ok()
        .flatten()
        .unwrap_or(0);
    check(
        rows >= 12,
        &format!("visual-test {SECTION}: 前提 = ペインの行数が 12 以上（{rows}）"),
    );
    (pane, rows)
}

/// 命令を 1 行打つ。打鍵と同じ `TerminalSession::write`（器のペインは器の client へ打つ =
/// 器の中のアプリへ届く）。`Request::Send` では器のペインのこのアプリへ届かなかった
/// （実測。この節の主題ではないので原因は追っていない）
async fn command(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    line: &str,
    until: &str,
) {
    let typed = window
        .update(cx, |app, _, _| {
            app.terminals
                .get(&pane)
                .map(|s| s.write(format!("{line}\r").into_bytes()))
                .is_some()
        })
        .unwrap_or(false);
    check(
        typed,
        &format!("visual-test {SECTION}: ペイン {pane} へ打てない"),
    );
    wait_screen(window, cx, pane, 10_000, until).await;
    for _ in 0..3 {
        notify_and_draw(any, window, cx);
        sleep(cx, 50).await;
    }
}

fn close(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) {
    dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Close {
            pane: Some(pane.as_u64()),
            force: true,
            caller_role: None,
        },
    );
}

fn dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    if let Some((frame, _)) = capture_frame(any, cx) {
        let _ = frame.save(std::path::Path::new(&dir).join(name));
    }
}

/// `from` で押し、`to` まで動かして `hold_ms` だけマウスを止める（ティックを回す）
async fn drag_and_hold(
    any: AnyWindowHandle,
    cx: &mut AsyncApp,
    from: Point<Pixels>,
    to: Point<Pixels>,
    hold_ms: u64,
) {
    mouse_move(any, cx, from, false);
    let _ = capture_frame(any, cx);
    down(any, cx, from);
    sleep(cx, 50).await;
    mouse_move(any, cx, to, true);
    sleep(cx, hold_ms).await;
}

/// 窓の外で離した形（`MouseUp` が届かずボタンなしの移動）。copy-on-select せずに畳まれる
fn release_lost(any: AnyWindowHandle, cx: &mut AsyncApp, at: Point<Pixels>) {
    mouse_move(any, cx, at, false);
}

/// 選択の頭が端を置いた位置（`TOP-` / `ALT-` 行の 8 桁目 = `e`）か
fn starts_at_col8(text: &str, head: &str) -> bool {
    text.starts_with(&TAIL[4..]) && !text.contains(&format!("{head}-abcd"))
}

/// 選択の尻が `BOT-` 行の途中（マウスを置いた 5 桁目の手前）で止まっているか = 行末へ寄っていない
fn ends_short_on_bot(text: &str) -> bool {
    let last = text.lines().last().unwrap_or("");
    last.starts_with("BOT-") && last.len() < format!("BOT-{TAIL}").len()
}

fn short(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.first().copied().unwrap_or("");
    let last = lines.last().copied().unwrap_or("");
    format!("{} 行・頭 {first:?}・尻 {last:?}", lines.len())
}

pub(super) async fn drag_autoscroll_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let root = ensure_fresh_scene(window, cx, SECTION).await;
    let legacy = legacy_2025();
    let legacy_bottom = legacy_2153();
    let mut failed: Vec<String> = Vec::new();
    let mut judge = |step: &str, ok: bool, detail: String| {
        println!("TAKO_VISUAL_2025: step={step} ok={ok} {detail}");
        if !ok {
            failed.push(format!("{step}（{detail}）"));
        }
    };

    // ① 履歴が無い
    let (pane, rows) = open_printer(any, window, cx, root).await;
    let tmux = window
        .update(cx, |app, _, _| app.mirror_scroll_pane(pane))
        .unwrap_or(false);
    let mode = if tmux { "tmux" } else { "direct" };
    println!(
        "TAKO_VISUAL_2025: mode={mode} legacy={legacy} legacy_2153={legacy_bottom} rows={rows}"
    );
    command(any, window, cx, pane, "lines 4", "DONE-4").await;
    let before = probe(window, cx, pane);
    check(
        before.history == 0 && before.offset == 0,
        &format!("visual-test {SECTION} ①: 前提 = 履歴が無い（{before:?}）"),
    );
    let from = cell_at(window, cx, pane, 2.2, 4);
    let to = cell_at(window, cx, pane, 8.2, 0);
    drag_and_hold(any, cx, from, to, 400).await;
    let p = probe(window, cx, pane);
    dump(any, cx, &format!("{mode}-1-no-history.png"));
    release_lost(any, cx, to);
    // 直接ペインはオートスクロールが立つ（立たないと何も確かめていない）。器のペインは外側の
    // 端末が alt screen（履歴 0）で立たない（実測）= 立ったかどうかは観察として出す
    judge(
        "①",
        starts_at_col8(&p.text, "TOP") && p.offset == 0 && (tmux || p.speed.is_some()),
        format!(
            "履歴なし: 速度係数 {:?}・表示位置 {}・外側 alt {}・選択 {}",
            p.speed,
            p.offset,
            p.alt,
            short(&p.text)
        ),
    );
    close(window, cx, pane);

    // ② 履歴が 1 画面未満（TOP + N 行 + DONE + カーソル行 = N + 3 行 → 履歴 5 行）
    let (pane, rows) = open_printer(any, window, cx, root).await;
    let n = rows + 2;
    command(
        any,
        window,
        cx,
        pane,
        &format!("lines {n}"),
        &format!("DONE-{n}"),
    )
    .await;
    let before = probe(window, cx, pane);
    let from = cell_at(window, cx, pane, 2.2, 4);
    let to = cell_at(window, cx, pane, 8.2, 0);
    if tmux {
        drag_and_hold(any, cx, from, to, 600).await;
        let p = probe(window, cx, pane);
        println!(
            "TAKO_VISUAL_2025: step=②（器・観察） 外側の履歴 {}・表示位置 {}・外側 alt {}・速度係数 {:?}・選択 {}",
            p.history,
            p.offset,
            p.alt,
            p.speed,
            short(&p.text)
        );
        release_lost(any, cx, to);
    } else {
        check(
            (1..rows).contains(&before.history) && before.offset == 0,
            &format!(
                "visual-test {SECTION} ②: 前提 = 履歴が 1 画面未満（{before:?}・行数 {rows}）"
            ),
        );
        drag_and_hold(any, cx, from, to, 50).await;
        // 最上端まで送り切るのを待つ（1 ティック 1 行以上 = 履歴 5 行なら数ティック）
        let started = std::time::Instant::now();
        let mut p = probe(window, cx, pane);
        while p.offset < p.history && started.elapsed() < Duration::from_secs(3) {
            sleep(cx, 30).await;
            p = probe(window, cx, pane);
        }
        sleep(cx, 200).await;
        let p = probe(window, cx, pane);
        dump(any, cx, &format!("{mode}-2a-small-history-scrolled.png"));
        judge(
            "②a",
            p.offset == p.history && p.text.starts_with(&format!("TOP-{TAIL}")),
            format!(
                "1 画面未満の履歴 {}: 表示位置 {}・選択 {}",
                p.history,
                p.offset,
                short(&p.text)
            ),
        );
        // b. 最上端に着いた後にマウスを動かす → その位置で伸ばした端のまま
        let to2 = point(to.x, to.y + px(1.0));
        mouse_move(any, cx, to2, true);
        sleep(cx, 400).await;
        let p = probe(window, cx, pane);
        dump(any, cx, &format!("{mode}-2b-at-top.png"));
        release_lost(any, cx, to2);
        judge(
            "②b",
            starts_at_col8(&p.text, "TOP") && p.offset == p.history && p.speed.is_some(),
            format!(
                "最上端: 表示位置 {}/{}・速度係数 {:?}・選択 {}",
                p.offset,
                p.history,
                p.speed,
                short(&p.text)
            ),
        );
    }
    close(window, cx, pane);

    // ③ 履歴が 1 画面以上
    let (pane, rows) = open_printer(any, window, cx, root).await;
    let n = rows * 3;
    command(
        any,
        window,
        cx,
        pane,
        &format!("lines {n}"),
        &format!("DONE-{n}"),
    )
    .await;
    let before = probe(window, cx, pane);
    let from = cell_at(window, cx, pane, 2.2, 4);
    let to = cell_at(window, cx, pane, 8.2, 0);
    drag_and_hold(any, cx, from, to, 400).await;
    let p = probe(window, cx, pane);
    dump(any, cx, &format!("{mode}-3-large-history.png"));
    release_lost(any, cx, to);
    if tmux {
        println!(
            "TAKO_VISUAL_2025: step=③（器・観察） 外側の履歴 {}・表示位置 {}・外側 alt {}・速度係数 {:?}・選択 {}",
            p.history,
            p.offset,
            p.alt,
            p.speed,
            short(&p.text)
        );
    } else {
        check(
            before.history >= rows,
            &format!(
                "visual-test {SECTION} ③: 前提 = 履歴が 1 画面以上（{before:?}・行数 {rows}）"
            ),
        );
        // 送った行のぶん選択が履歴へ伸びる（元の 0〜4 行目 = 5 行より長い）
        judge(
            "③",
            p.offset > 0 && p.offset < p.history && p.text.lines().count() > 5,
            format!(
                "1 画面以上の履歴 {}: 表示位置 {}・選択 {}",
                p.history,
                p.offset,
                short(&p.text)
            ),
        );
    }
    close(window, cx, pane);

    // ④ alt screen（全画面 TUI）
    let (pane, _) = open_printer(any, window, cx, root).await;
    command(any, window, cx, pane, "alt 4", "DONE-4").await;
    let before = probe(window, cx, pane);
    let from = cell_at(window, cx, pane, 2.2, 4);
    let to = cell_at(window, cx, pane, 8.2, 0);
    drag_and_hold(any, cx, from, to, 400).await;
    let p = probe(window, cx, pane);
    dump(any, cx, &format!("{mode}-4-alt-screen.png"));
    release_lost(any, cx, to);
    // 直接ペインは外側の端末そのものが alt screen = 開始の判定（`update_drag_scroll`）で立たない
    judge(
        "④",
        starts_at_col8(&p.text, "ALT") && (tmux || (before.alt && p.speed.is_none())),
        format!(
            "alt screen {}（外側の履歴 {}）: 速度係数 {:?}・選択 {}",
            before.alt,
            before.history,
            p.speed,
            short(&p.text)
        ),
    );
    close(window, cx, pane);

    // ⑤ 最下部（#2153）: 履歴を作って最下行を埋め（TOP + N 行 + BOT = 行数 + 5 行 → 履歴 5 行）、
    // 最下行の 5 桁目で止める
    let (pane, rows) = open_printer(any, window, cx, root).await;
    command(
        any,
        window,
        cx,
        pane,
        &format!("bot {}", rows + 3),
        &format!("BOT-{TAIL}"),
    )
    .await;
    let before = probe(window, cx, pane);
    if !tmux {
        check(
            before.history > 0 && before.offset == 0,
            &format!("visual-test {SECTION} ⑤: 前提 = 履歴があって最下部にいる（{before:?}）"),
        );
    }
    let from = cell_at(window, cx, pane, 2.2, 1);
    let to = cell_at(window, cx, pane, 5.2, rows - 1);
    drag_and_hold(any, cx, from, to, 400).await;
    let p = probe(window, cx, pane);
    dump(any, cx, &format!("{mode}-5-bottom.png"));
    release_lost(any, cx, to);
    // 直接ペインはオートスクロールが立つ（立たないと何も確かめていない）。器のペインは ① と同じく
    // 外側の端末が alt screen（履歴 0）で立たない = 立ったかどうかは観察として出す
    judge(
        "⑤",
        ends_short_on_bot(&p.text) && p.offset == 0 && (tmux || p.speed.is_some()),
        format!(
            "最下部（履歴 {}）: 速度係数 {:?}・表示位置 {}・選択 {}",
            before.history,
            p.speed,
            p.offset,
            short(&p.text)
        ),
    );
    close(window, cx, pane);

    // ⑥ 履歴の途中（#310 の下へ送る側 + #2153）: 3 行上から下端で止める
    if tmux {
        println!(
            "TAKO_VISUAL_2025: step=⑥（器・飛ばす） 外側の端末に履歴が無く、履歴の途中から送れない"
        );
    } else {
        let (pane, rows) = open_printer(any, window, cx, root).await;
        let n = rows * 3;
        command(
            any,
            window,
            cx,
            pane,
            &format!("bot {n}"),
            &format!("BOT-{TAIL}"),
        )
        .await;
        let back = 3usize;
        let _ = window.update(cx, |app, _, cx| {
            if let Some(s) = app.terminals.get(&pane) {
                s.scroll_display(back as i32);
            }
            cx.notify();
        });
        notify_and_draw(any, window, cx);
        let before = probe(window, cx, pane);
        check(
            before.offset == back && before.history >= rows,
            &format!(
                "visual-test {SECTION} ⑥: 前提 = 履歴の途中（{back} 行上）にいる（{before:?}）"
            ),
        );
        let from = cell_at(window, cx, pane, 2.2, 1);
        let to = cell_at(window, cx, pane, 5.2, rows - 1);
        drag_and_hold(any, cx, from, to, 50).await;
        // 最下部まで送り切るのを待つ（1 ティック 1 行以上 = 3 行なら数ティック）
        let started = std::time::Instant::now();
        let mut p = probe(window, cx, pane);
        while p.offset > 0 && started.elapsed() < Duration::from_secs(3) {
            sleep(cx, 30).await;
            p = probe(window, cx, pane);
        }
        sleep(cx, 200).await;
        let p = probe(window, cx, pane);
        dump(any, cx, &format!("{mode}-6a-mid-history-scrolled.png"));
        // 送った行のぶん選択が伸び（押した 1 行目〜最下行 = 行数 - 1 行 + 送った 3 行）、
        // 尻は最下行の行末（#310 のまま）
        judge(
            "⑥a",
            p.offset == 0
                && p.text.ends_with(&format!("BOT-{TAIL}"))
                && p.text.lines().count() >= rows - 1 + back,
            format!(
                "履歴の途中 {back} 行上: 表示位置 {}・選択 {}",
                p.offset,
                short(&p.text)
            ),
        );
        // b. 最下部に着いた後にマウスを動かす → その位置で伸ばした端のまま
        let to2 = point(to.x, to.y - px(1.0));
        mouse_move(any, cx, to2, true);
        sleep(cx, 400).await;
        let p = probe(window, cx, pane);
        dump(any, cx, &format!("{mode}-6b-at-bottom.png"));
        release_lost(any, cx, to2);
        judge(
            "⑥b",
            ends_short_on_bot(&p.text) && p.offset == 0 && p.speed.is_some(),
            format!(
                "最下部: 表示位置 {}・速度係数 {:?}・選択 {}",
                p.offset,
                p.speed,
                short(&p.text)
            ),
        );
        close(window, cx, pane);
    }

    check(
        failed.is_empty(),
        &format!(
            "visual-test {SECTION}（{mode}）: 落ちた段 = {}",
            failed.join(" / ")
        ),
    );
}
