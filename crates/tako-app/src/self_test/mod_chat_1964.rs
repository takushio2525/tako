//! visual-test `mod-chat`（FR-2.42.48〜 / Issue #1964）: 実物の Claude Code（`scripts/test-mod-chat-1964.sh` が偽の
//! Messages API と一時の設定 dir で起こし、GUI モードにして 2 ターン = Bash の呼び出しとコードブロックの返答を
//! 流しておく）の同じ会話を、GPUI のチャットビューと mod の描画で並べて撮る（判断点 = 受け入れ 4 の材料）。
//!
//! ① `chat.place = auto`（既定）: GPUI のチャットビューのまま（持ち主 tako / place_auto）→ `mod-chat-gpui.png`
//! ② `tako mod ui set chat.place mod` と同じ dispatch → 次の報告で mod が `gui` の組を受け取り、tako はペインを
//!    ターミナル表示（理由 mod_chat）にして、画面に吹き出し（枠つきの発話）・コードの Copy・ツールの要約が出る
//!    → `mod-chat-mod.png`
//! ③ 会話の行の `コピー 1` を実 OS と同じ `PlatformInput` のクリックで押す → 報告の last_press が code_copy
//!    （スクリプトは ui.copy を拒む mod を同居させる = 本物のクリップボードへ書かない。拒否を尊重するので ok=false）
//! ④ 帯（ctrl+x → Tab → `o`）の「最新のコードをコピー」をキーで押す → last_press が新しい code_copy
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1964_LEGACY=1` はチャットを mod に任せないので ② が FAILED =
//! 同一バイナリでの A/B（旧挙動の ① の 1 枚が残る）。単独実行は `TAKO_VISUAL_ONLY=mod-chat`

use super::*;

const SECTION: &str = "mod-chat";

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

fn dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Some((frame, _)) = capture_frame(any, cx) {
        let path = std::path::Path::new(&dir).join(name);
        let _ = frame.save(&path);
        println!("TAKO_VISUAL_1964: {SECTION} dump={}", path.display());
    }
}

/// ペインの表示（`tako ui-mode` と同じ判定表）と持ち主（`tako mod` の panes[].chat と同じ判断）
fn display_of(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> (String, String) {
    window
        .update(cx, |app, _, _| {
            let status =
                tako_core::ui_mode::PaneDisplayStatus::from_input(app.pane_display_input(pane));
            let owner = tako_control::claude_mod::chat_owner(app, pane, std::time::Instant::now());
            (
                format!(
                    "{}/{}",
                    status.display.as_str(),
                    status.reason.map(|r| r.as_str()).unwrap_or("-")
                ),
                format!("{}/{}", owner.as_str(), owner.reason()),
            )
        })
        .unwrap_or_default()
}

fn last_press(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> String {
    window
        .update(cx, |app, _, _| {
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

/// 画面の `label` の中ほどのセル（列, 行）
fn find_label(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
) -> Option<(f32, usize)> {
    window
        .update(cx, |app, _, _| {
            let session = app.terminals.get(&pane)?;
            let screen = session.screen(&app.theme);
            screen.lines.iter().enumerate().find_map(|(row, line)| {
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

fn send(any: AnyWindowHandle, cx: &mut AsyncApp, input: gpui::PlatformInput) {
    let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
}

async fn click(any: AnyWindowHandle, cx: &mut AsyncApp, position: Point<Pixels>) {
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
    sleep(cx, 30).await;
    send(
        any,
        cx,
        gpui::PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    );
}

/// ペインへキーのバイト列を送る（`tako send --no-newline` と同じ dispatch）
async fn keys(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId, text: &str) {
    dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Send {
            pane: Some(pane.as_u64()),
            text: text.into(),
            newline: false,
            tmux_session: None,
            await_prompt: false,
            paste_guard: None,
        },
    );
    sleep(cx, 600).await;
}

/// 画面に吹き出し（`╭` の枠の直下の行に発話）が出ているか
fn has_bubble(screen: &str, said: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    lines
        .windows(2)
        .any(|w| w[0].trim_end().ends_with('╮') && w[1].contains(said) && w[1].contains('│'))
}

pub(super) async fn mod_chat_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    // スクリプトが 2 ターン流し終え、GPUI のチャットが同じ会話を読み終えるのを待つ
    let started = std::time::Instant::now();
    let pane = loop {
        let found = window
            .update(cx, |app, _, _| {
                app.terminals.iter().find_map(|(id, s)| {
                    let text = s.visible_lines().join("\n");
                    // 2 ターン目（コードブロックの返答）まで GPUI のチャットが読み終えた
                    let chat_done = app.chat_panes.get(id).is_some_and(|c| {
                        c.messages.iter().any(|m| m.text.contains("RUN-DONE"))
                            && c.messages
                                .iter()
                                .any(|m| m.text.contains("hello from tako"))
                    });
                    (text.contains("hello from tako") && chat_done).then_some(*id)
                })
            })
            .ok()
            .flatten();
        if let Some(pane) = found {
            break pane;
        }
        if started.elapsed() > Duration::from_secs(300) {
            fail(&format!(
                "visual-test {SECTION}: 前提 = claude の 2 ターン（RUN-DONE と コードブロックの返答）が画面と GPUI のチャットに出ない"
            ));
        }
        sleep(cx, 500).await;
    };
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        sleep(cx, 100).await;
    }
    println!(
        "TAKO_VISUAL_1964: {SECTION} pane={pane} legacy={}",
        tako_core::claude_mod::chat_legacy()
    );

    // ① 既定（auto）は GPUI のチャットのまま
    let (display, owner) = display_of(window, cx, pane);
    println!("TAKO_VISUAL_1964: {SECTION} ① display={display} owner={owner}");
    if !display.starts_with("chat/") {
        fail(&format!(
            "visual-test {SECTION} ①: 既定（chat.place auto）で GPUI のチャットにならない（display={display} owner={owner}）"
        ));
    }
    dump(any, cx, "mod-chat-gpui.png");

    // ② chat.place mod（`tako mod ui set chat.place mod` と同じ dispatch）→ 次の報告で mod が gui の組を受け取る
    dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Mod {
            action: Some("ui".into()),
            report: None,
            pane: None,
            ui: Some(tako_core::claude_mod_ui::UiRequest {
                op: Some("set".into()),
                key: Some("chat.place".into()),
                value: Some(serde_json::json!("mod")),
                ..Default::default()
            }),
            dry_run: false,
        },
    );
    let switched = std::time::Instant::now();
    let seen = loop {
        let (display, owner) = display_of(window, cx, pane);
        let screen = screen_of(window, cx, pane);
        let bubble = has_bubble(&screen, "Reply with code blocks");
        let drawn = screen.contains("コピー 1") || screen.contains("Copy 1");
        if display == "terminal/mod_chat" && bubble && drawn {
            break (display, owner, bubble, drawn);
        }
        if switched.elapsed() > Duration::from_secs(45) {
            fail(&format!(
                "visual-test {SECTION} ②: chat.place mod でターミナル表示（mod の描画）にならない（display={display} owner={owner} 吹き出し={bubble} Copy={drawn}）。画面: {screen:?}"
            ));
        }
        sleep(cx, 500).await;
    };
    let took = switched.elapsed().as_millis();
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        sleep(cx, 100).await;
    }
    println!(
        "TAKO_VISUAL_1964: {SECTION} ② display={} owner={} bubble={} copy={} took_ms={took}",
        seen.0, seen.1, seen.2, seen.3
    );
    dump(any, cx, "mod-chat-mod.png");

    // ③ 会話の行の Copy をクリックで押す（Claude Code は描かれて 300 ms 以内のクリックを捨てるので 1 秒置く）
    sleep(cx, 1_000).await;
    let before = last_press(window, cx, pane);
    let Some((col, row)) =
        find_label(window, cx, pane, "コピー 1").or_else(|| find_label(window, cx, pane, "Copy 1"))
    else {
        fail(&format!("visual-test {SECTION} ③: 画面に Copy 1 が無い"));
    };
    let at = cell_center(window, cx, pane, col, row);
    click(any, cx, at).await;
    let clicked = std::time::Instant::now();
    let press = loop {
        let now = last_press(window, cx, pane);
        if now != before && now.starts_with("code_copy/") {
            break now;
        }
        if clicked.elapsed() > Duration::from_secs(10) {
            fail(&format!(
                "visual-test {SECTION} ③: Copy 1 をクリックしても mod が押されたと報告しない（last_press {now:?}・セル {col},{row}）"
            ));
        }
        sleep(cx, 200).await;
    };
    println!("TAKO_VISUAL_1964: {SECTION} ③ click=({col},{row}) last_press={press}");

    // ④ 帯の「最新のコードをコピー」をキーで押す（ctrl+x → Tab → ホットキー o）
    sleep(cx, 1_000).await;
    keys(window, cx, pane, "\x1b").await;
    keys(window, cx, pane, "\x18").await;
    keys(window, cx, pane, "\t").await;
    keys(window, cx, pane, "o").await;
    let pressed = std::time::Instant::now();
    let key_press = loop {
        let now = last_press(window, cx, pane);
        if now != press && now.starts_with("code_copy/") {
            break now;
        }
        if pressed.elapsed() > Duration::from_secs(10) {
            fail(&format!(
                "visual-test {SECTION} ④: 帯のキー（ctrl+x → Tab → o）で押しても mod が押されたと報告しない（last_press {now:?}）。画面: {:?}",
                screen_of(window, cx, pane)
            ));
        }
        sleep(cx, 200).await;
    };
    keys(window, cx, pane, "\x1b").await;
    println!("TAKO_VISUAL_1964: {SECTION} ④ key last_press={key_press}");
    println!("TAKO_VISUAL_1964: {SECTION} ok");
}
