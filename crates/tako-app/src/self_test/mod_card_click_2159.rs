//! visual-test `mod-card-click`（FR-2.42.53 / Issue #2159）: 実物の Claude Code（`scripts/test-mod-card-click-2159.sh`
//! が偽の Messages API と一時の設定 dir で起こし、AI に `tako_show_command` でカードを 1 枚出させておく）の
//! 会話の行のカードを、実 OS と同じ `PlatformInput` のクリック **1 回**で実行する。
//!
//! ① 会話の行のカードにコマンドの `[ run ]`（ja は `[ 実行 ]`）が出る（帯の `r: run` とは別 = クリックで押す口）
//!    → `mod-card-click-before.png`
//! ② `[ run ]` を 1 回クリック → 新しいペインが 1 枚だけ増える（手元のペインと同じタブ・タイトル `(>) …`・
//!    フォーカスは手元のまま）・カードの実行記録 runs[0] が count 1（GPUI のカードの「実行」と同じ dispatch の run）。
//!    クリックは 150 ms 後にもう 1 回（ダブルクリックの 2 打目）届けても増えない → `mod-card-click-after.png`
//! ③ 実行中（`sleep` で十数秒走る。押す直前に GPUI と同じ判定でまだ実行中かを確かめる）にもう 1 回押す →
//!    tako の dispatch が断り（実行中の再実行）、ペインは増えず count 1 のまま・画面に断った理由のトースト
//!    → `mod-card-click-refused.png`
//! ④ 実行が終わると、会話の行の実行記録が `exited 0`（次の報告 = 最大 15 秒で届く）
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_2159_LEGACY=1` は会話の行にボタンを置かないので ① が FAILED =
//! 同一バイナリでの A/B。単独実行は `TAKO_VISUAL_ONLY=mod-card-click`

use super::*;

const SECTION: &str = "mod-card-click";

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

fn dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Some((frame, _)) = capture_frame(any, cx) {
        let path = std::path::Path::new(&dir).join(name);
        let _ = frame.save(&path);
        println!("TAKO_VISUAL_2159: {SECTION} dump={}", path.display());
    }
}

/// 画面の `label` の中ほどのセル（列, 行）。上から最初に当たった行
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

/// 実 OS と同じ順（移動 → 押す → 離す）の左クリック。`click_count` = 2 はダブルクリックの 2 打目
async fn click(
    any: AnyWindowHandle,
    cx: &mut AsyncApp,
    position: Point<Pixels>,
    click_count: usize,
) {
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
            click_count,
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
            click_count,
        }),
    );
}

/// 1 件目のコマンドの実行記録（実行ペイン・状態・回数）
type RunOf = (PaneId, tako_core::CardRunState, u32);

/// このペインのいちばん新しいカード（id・1 件目の実行記録）
fn card_of(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<(u64, Option<RunOf>)> {
    window
        .update(cx, |app, _, _| {
            let card = app.command_cards.list(Some(pane)).into_iter().last()?;
            let run = card
                .runs()
                .first()
                .copied()
                .flatten()
                .map(|r| (r.pane, r.state, r.count));
            Some((card.id().as_u64(), run))
        })
        .ok()
        .flatten()
}

/// タブのペイン（id の昇順）・フォーカス中のペイン
fn panes_of(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> (Vec<PaneId>, Option<PaneId>) {
    window
        .update(cx, |app, _, _| {
            let tab = app
                .workspace
                .find_tab_of_pane(pane)
                .and_then(|t| app.workspace.get_tab(t));
            let mut ids: Vec<PaneId> = tab
                .map(|t| t.tree().panes().iter().map(|p| p.id()).collect())
                .unwrap_or_default();
            ids.sort();
            (ids, tab.map(|t| t.tree().focused()))
        })
        .unwrap_or_default()
}

fn title_of(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> String {
    window
        .update(cx, |app, _, _| {
            app.workspace
                .find_tab_of_pane(pane)
                .and_then(|t| app.workspace.get_tab(t))
                .and_then(|t| t.tree().get(pane))
                .and_then(|p| p.title())
                .map(str::to_string)
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn owner_of(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, card: u64) -> String {
    window
        .update(cx, |app, _, _| {
            app.command_cards
                .get(tako_core::CommandCardId::from_raw(card))
                .map(|c| {
                    let owner =
                        tako_control::claude_mod::card_owner(app, c, std::time::Instant::now());
                    format!("{}/{}", owner.as_str(), owner.reason())
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

async fn redraw(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        sleep(cx, 100).await;
    }
}

pub(super) async fn mod_card_click_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    // 前提（新旧どちらの腕でも満たす = #1963 の形）: AI が出したカードを mod が会話の行に描き、帯で拾った
    let started = std::time::Instant::now();
    let (pane, card) = loop {
        let found = window
            .update(cx, |app, _, _| {
                app.terminals.iter().find_map(|(id, s)| {
                    let card = app.command_cards.list(Some(*id)).into_iter().last()?;
                    let text = s.visible_lines().join("\n");
                    let row = format!("card {}", card.id().as_u64());
                    let row_ja = format!("カード {}", card.id().as_u64());
                    ((text.contains(&row) || text.contains(&row_ja)) && text.contains("click 2159"))
                        .then_some((*id, card.id().as_u64()))
                })
            })
            .ok()
            .flatten();
        if let Some((pane, card)) = found {
            if owner_of(window, cx, card) == "mod/drawn" {
                break (pane, card);
            }
        }
        if started.elapsed() > Duration::from_secs(300) {
            fail(&format!(
                "visual-test {SECTION}: 前提 = AI が出したカード（click 2159）を mod が会話の行と帯に描かない"
            ));
        }
        sleep(cx, 500).await;
    };
    redraw(any, window, cx).await;
    let (before_panes, before_focus) = panes_of(window, cx, pane);
    println!(
        "TAKO_VISUAL_2159: {SECTION} pane={pane} card={card} panes={} legacy={} click={}",
        before_panes.len(),
        tako_core::claude_mod::card_click_legacy(),
        window
            .update(cx, |app, _, _| tako_control::claude_mod::card_click(app))
            .unwrap_or(false)
    );

    // ① 会話の行のカードにクリックで押す `[ run ]`（帯の `r: run` は角括弧なし）
    let label = ["[ run ]", "[ 実行 ]"];
    let found = label.iter().find_map(|l| find_label(window, cx, pane, l));
    dump(any, cx, "mod-card-click-before.png");
    let Some((col, row)) = found else {
        fail(&format!(
            "visual-test {SECTION} ①: 会話の行のカードに [ run ] が無い（クリックで押す口が無い）。画面: {:?}",
            screen_of(window, cx, pane)
        ));
    };
    println!("TAKO_VISUAL_2159: {SECTION} ① button=({col},{row})");

    // ② クリック 1 回（Claude Code は描かれて 300 ms 以内のクリックを捨てるので 1 秒置く）+ 150 ms 後の 2 打目
    sleep(cx, 1_000).await;
    let at = cell_center(window, cx, pane, col, row);
    let first_click = std::time::Instant::now();
    click(any, cx, at, 1).await;
    sleep(cx, 150).await;
    click(any, cx, at, 2).await;
    let clicked = std::time::Instant::now();
    let new_pane = loop {
        let (now, _) = panes_of(window, cx, pane);
        if let Some(added) = now.iter().find(|p| !before_panes.contains(p)) {
            break *added;
        }
        if clicked.elapsed() > Duration::from_secs(15) {
            fail(&format!(
                "visual-test {SECTION} ②: [ run ] をクリックしても新しいペインが立たない（セル {col},{row}）。画面: {:?}",
                screen_of(window, cx, pane)
            ));
        }
        sleep(cx, 100).await;
    };
    let took = clicked.elapsed().as_millis();
    // 2 打目が遅れて届いても拾えるだけ待ってから数える
    sleep(cx, 2_000).await;
    let (after_panes, after_focus) = panes_of(window, cx, pane);
    let run = card_of(window, cx, pane).and_then(|(_, run)| run);
    let title = title_of(window, cx, new_pane);
    redraw(any, window, cx).await;
    dump(any, cx, "mod-card-click-after.png");
    println!(
        "TAKO_VISUAL_2159: {SECTION} ② new_pane={new_pane} took_ms={took} panes={}→{} title={title:?} focus={:?}→{:?} run={:?}",
        before_panes.len(),
        after_panes.len(),
        before_focus,
        after_focus,
        run
    );
    if after_panes.len() != before_panes.len() + 1 {
        fail(&format!(
            "visual-test {SECTION} ②: ペインが {} 枚増えた（1 枚のはず = ダブルクリックで 2 本走った）",
            after_panes.len() - before_panes.len()
        ));
    }
    match run {
        Some((p, _, 1)) if p == new_pane => {}
        other => fail(&format!(
            "visual-test {SECTION} ②: 実行記録が新しいペインで 1 回になっていない（{other:?}）"
        )),
    }
    if !title.starts_with("(>) ") {
        fail(&format!(
            "visual-test {SECTION} ②: 実行ペインのタイトルが GPUI のカードの実行と違う（{title:?}）"
        ));
    }
    if after_focus != before_focus {
        fail(&format!(
            "visual-test {SECTION} ②: 手元のペインからフォーカスが移った（{before_focus:?} → {after_focus:?}）"
        ));
    }

    // ③ 実行中にもう 1 回押す → tako の dispatch が断る（ペインは増えない・回数も 1 のまま）。押す直前に、GPUI の
    // 2 秒 tick と同じ判定（記録を実行ペインの実態へ寄せる）でまだ実行中かを見る = 1 本目が終わった後の押下
    // （GPUI のカードと同じく再実行になる = 正しい挙動）と取り違えない
    let Some((col, row)) = label.iter().find_map(|l| find_label(window, cx, pane, l)) else {
        fail(&format!("visual-test {SECTION} ③: 画面に [ run ] が無い"));
    };
    let before3 = window
        .update(cx, |app, _, _| {
            tako_control::dispatch::refresh_command_card_runs(app);
        })
        .ok()
        .and_then(|_| card_of(window, cx, pane))
        .and_then(|(_, run)| run);
    let since = first_click.elapsed().as_millis();
    if !matches!(before3, Some((_, tako_core::CardRunState::Running, 1))) {
        fail(&format!(
            "visual-test {SECTION} ③: 前提 = 押す直前に 1 本目がまだ実行中（{before3:?}・1 回目のクリックから {since} ms）"
        ));
    }
    let at = cell_center(window, cx, pane, col, row);
    click(any, cx, at, 1).await;
    let pressed = std::time::Instant::now();
    let toast = loop {
        let screen = screen_of(window, cx, pane);
        if screen.contains("could not run") || screen.contains("実行できませんでした") {
            break true;
        }
        if pressed.elapsed() > Duration::from_secs(4) {
            break false;
        }
        sleep(cx, 100).await;
    };
    redraw(any, window, cx).await;
    dump(any, cx, "mod-card-click-refused.png");
    let (again, _) = panes_of(window, cx, pane);
    let run = card_of(window, cx, pane).and_then(|(_, run)| run);
    println!(
        "TAKO_VISUAL_2159: {SECTION} ③ before={before3:?} since_first_click_ms={since} panes={} run={run:?} toast={toast}",
        again.len()
    );
    if again.len() != after_panes.len() || !matches!(run, Some((_, _, 1))) {
        fail(&format!(
            "visual-test {SECTION} ③: 実行中に押し直したら 2 本目が走った（ペイン {} 枚・{run:?}）",
            again.len()
        ));
    }
    if !toast {
        fail(&format!(
            "visual-test {SECTION} ③: 断った理由のトーストが出ない（画面: {:?}）",
            screen_of(window, cx, pane)
        ));
    }

    // ④ 終わると会話の行の実行記録が exited 0（mod の次の報告 = 最大 15 秒で届く）
    let waited = std::time::Instant::now();
    loop {
        let screen = screen_of(window, cx, pane);
        let exited = format!("pane {} exited 0", new_pane.as_u64());
        let exited_ja = format!("ペイン {} 終了 0", new_pane.as_u64());
        if screen.contains(&exited) || screen.contains(&exited_ja) {
            break;
        }
        if waited.elapsed() > Duration::from_secs(40) {
            fail(&format!(
                "visual-test {SECTION} ④: 会話の行に実行記録（exited 0）が出ない。記録: {:?}・画面: {screen:?}",
                card_of(window, cx, pane)
            ));
        }
        sleep(cx, 500).await;
    }
    redraw(any, window, cx).await;
    dump(any, cx, "mod-card-click-done.png");
    println!(
        "TAKO_VISUAL_2159: {SECTION} ④ run={:?}",
        card_of(window, cx, pane)
    );
    println!("TAKO_VISUAL_2159: {SECTION} ok");
}
