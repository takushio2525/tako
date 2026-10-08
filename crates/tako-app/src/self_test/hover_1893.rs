//! visual-test `hover-1893` / `hover-loading` / `hover-loading-real`（FR-3.37 / Issue #1893）:
//! ホバーの続き（右クリックの項目・キー・読み込み中の待ち）を**実マウス・実キーで**確かめる。
//!
//! - `hover-1893`（偽サーバ）: ①識別子の右クリックに「ホバー情報を表示」が移動の後・整形の前に出て、
//!   押すとカードが出る（マウスはメニューの項目の位置のまま = 明示のカードはマウスで閉じない）
//!   ②カードが出ている間の右クリックでカードが閉じてメニューが開く ③識別子でない所の右クリックには
//!   出ない ④キー（⇧⌘H / Ctrl+Shift+H）で編集カーソルの位置にカードが出て、本文は 1 字も変わらない・
//!   巨大な doc はカードでは 16,000 字で切る ⑤補完の一覧が出ている間のキーは一覧を閉じてカードを出す
//! - `hover-loading`（偽サーバの `loading` = 読み込みの遅延の注入）: ①読み込み中に乗せると語の真下に
//!   「読み込み中」の 1 行（基準画像との差分は矩形の外 0 px）②マウスを動かさずに待つと読み込みの後に
//!   カードへ差し替わる ③待つあいだに語から外れると 1 行は消え、待ちも残らない ④読み込みが終わらない
//!   ときは上限で 1 行が消えて `loading`（カードは出ない）
//! - `hover-loading-real`（実の rust-analyzer。無ければ SKIPPED）: 暖機なしで読み込みの最中に乗せると、
//!   済んだらカードが出る。続けてキーでもカードが出る
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1893_LEGACY=1` は `hover-1893` の ①（項目が無い）・
//! `hover-loading` の ①（1 行が出ない）・`hover-loading-real`（カードが出ない）で名指しの FAILED に
//! なる = 同一バイナリでの A/B。画像は `TAKO_VISUAL_DUMP_DIR` の下へ節の名前つきで落とす

use super::*;

/// 編集カーソルの位置のホバーのキー（**綴りはここに直書き**する = 表から引くと、表が壊れても
/// 同じ壊れ方で緑になる。jump-keys と同じ方針）
fn hover_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-shift-h"
    } else {
        "ctrl-shift-h"
    }
}

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

/// カードの今の状態（出ていれば（行, 明示か, 本文の先頭, 切る前の字数））
type CardState = Option<(usize, bool, String, Option<usize>)>;

fn card_state(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> CardState {
    window
        .update(cx, |app, _, _| {
            app.lsp_hover.card.as_ref().map(|c| {
                let head = c
                    .blocks
                    .first()
                    .map(|b| match &b.kind {
                        preview::MdBlockKind::Paragraph { spans }
                        | preview::MdBlockKind::Heading { spans, .. } => {
                            spans.iter().map(|s| s.text.as_str()).collect::<String>()
                        }
                        preview::MdBlockKind::CodeBlock { lines, .. } => lines
                            .first()
                            .map(|line| line.iter().map(|s| s.text.as_str()).collect())
                            .unwrap_or_default(),
                        _ => String::new(),
                    })
                    .unwrap_or_default()
                    .chars()
                    .take(40)
                    .collect::<String>();
                (
                    c.line,
                    c.origin == crate::lsp_hover_ui::HoverOrigin::Explicit,
                    head,
                    c.truncated,
                )
            })
        })
        .ok()
        .flatten()
}

/// 条件が立つまで描き直しながら待つ（上限つき）
async fn until(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    limit: Duration,
    done: &dyn Fn(&mut AsyncApp) -> bool,
) -> bool {
    vt1860_wait(any, window, cx, limit, done).await
}

/// 編集カーソルを置く（行は 1 始まり・桁は行内 UTF-8 バイト = `tako edit cursor`）
fn put_cursor(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    line: usize,
    col: usize,
) {
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::PreviewCursor {
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
    });
}

fn buffer_version(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> Option<u64> {
    window
        .update(cx, |app, _, _| {
            app.preview_edits.get(&pane).map(|e| e.buffer.version())
        })
        .ok()
        .flatten()
}

/// 語でもカードでもない所（1 行目の行末より右 = #1681 の節と同じ。行頭は `fn` という語の上）
fn away_point(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<Point<Pixels>> {
    window
        .update(cx, |app, _, _| {
            let layout = app.preview_text_layouts.get(&pane)?.first()?.clone()?;
            let b = layout.bounds();
            Some(point(
                b.right() - px(4.0),
                b.top() + layout.line_height() / 2.0,
            ))
        })
        .ok()
        .flatten()
}

/// 文書が言語サーバにつながる（マウスのホバーの入口が開く）まで待つ
async fn wait_linked(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
) {
    check(
        until(any, window, cx, Duration::from_secs(20), &|cx| {
            window
                .update(cx, |app, _, _| app.lsp_hover_enabled_for(pane))
                .unwrap_or(false)
        })
        .await,
        &format!("visual-test {label}: 編集モードで文書がサーバへつながる（素材の前提）"),
    );
}

/// visual-test `hover-1893`（偽サーバ）。相は冒頭の説明 ①〜⑤
pub(super) async fn hover_1893_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_core::lsp::menu::MenuItem;
    const LABEL: &str = "hover-1893";
    inject_section_failure(LABEL);
    // 3 行目（0 起点の 2）は 40,000 字を超える doc（カードは 16,000 字で切る）。他の語は語をそのまま返す
    let big: String = (0..2500)
        .map(|i| format!("line {i} of a very long doc\n"))
        .collect();
    let total_chars = big.chars().count();
    let source = "fn main() {\n    let total = 1;\n    let v = total;\n}\n";
    let rules = serde_json::json!([
        { "line": 2, "result": { "contents": { "kind": "markdown", "value": big } } },
        { "echo": true },
    ]);
    let (pane, dir, override_env, rules_file) =
        hover_scene(any, window, cx, LABEL, rules, source).await;
    wait_linked(any, window, cx, pane, LABEL).await;
    let word = vt1684_point(window, cx, pane, 1, 8..13)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 2 行目の total の位置")));

    // ① 識別子の右クリック → 「ホバー情報を表示」（移動の後・整形の前）→ 押すとカード。
    //    押した後にカードが無く、窓のマウス位置が押した位置からずれていた（= 実機のマウスの移動が
    //    割り込んだ。蓋閉じの機では tako-vd が主画面）ときだけやり直す（#1681 の節と同じ。回数を
    //    `interfered=` に出す）。押した位置のままで出なければ実装の不具合として落とす
    let hover_id = MenuItem::Hover.id();
    let mut interfered = 0;
    let (shown, card, item) = loop {
        vt1860_click(any, window, cx, word, MouseButton::Right);
        check(
            vt1684_settled(any, window, cx, Duration::from_secs(20)).await,
            &format!("visual-test {LABEL} ①: メニューの言語サーバの節が決まる"),
        );
        let (_, drawn) = vt1684_menu(any, window, cx).unwrap_or_default();
        println!("TAKO_VISUAL_PIXEL: {LABEL} ① menu={drawn:?}");
        dump(any, cx, "hover-1893-menu.png");
        let at = drawn.iter().position(|id| *id == hover_id);
        check(
            at.is_some(),
            &format!("visual-test {LABEL} ①: 識別子の右クリックに「ホバー情報を表示」が出る (#1893) ({drawn:?})"),
        );
        let at = at.unwrap_or_default();
        check(
            at > 0
                && drawn[at - 1] == "lsp-implementation"
                && drawn.get(at + 1) == Some(&"lsp-format"),
            &format!("visual-test {LABEL} ①: 並びは移動 → ホバー → 整形 ({drawn:?})"),
        );
        let item = vt1684_item_point(window, cx, hover_id)
            .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: 項目の矩形が無い")));
        vt1860_click(any, window, cx, item, MouseButton::Left);
        let shown = until(any, window, cx, Duration::from_secs(20), &|cx| {
            card_state(window, cx).is_some()
        })
        .await;
        // 明示のカードはマウスの位置（メニューの項目の上 = 語の外）では閉じない
        for _ in 0..10 {
            notify_and_draw(any, window, cx);
        }
        let card = card_state(window, cx);
        let (state, moved) = window
            .update(cx, |app, win, _| {
                (
                    format!(
                        "{} status={:?} mouse={:?}",
                        app.lsp_hover.debug_state(),
                        app.lsp_goto_header_status(pane),
                        win.mouse_position()
                    ),
                    win.mouse_position() != item,
                )
            })
            .unwrap_or_default();
        println!(
            "TAKO_VISUAL_PIXEL: {LABEL} ① shown={shown} card={card:?} item={item:?} interfered={interfered} {state}"
        );
        let ok = card.as_ref().is_some_and(|(line, explicit, head, _)| {
            *line == 1 && *explicit && head.contains("total")
        });
        if ok || !moved || interfered >= 3 {
            break (shown, card, item);
        }
        interfered += 1;
        let _ = window.update(cx, |app, _, cx| {
            app.close_lsp_hover();
            cx.notify();
        });
    };
    let _ = item;
    dump(any, cx, "hover-1893-menu-card.png");
    check(
        shown
            && card.as_ref().is_some_and(|(line, explicit, head, _)| {
                *line == 1 && *explicit && head.contains("total")
            }),
        &format!(
            "visual-test {LABEL} ①: 押すとその語のカードが出て、マウスが語の外でも残る ({card:?})"
        ),
    );

    // ② カードが出ている間の右クリック: カードは閉じてメニューが開く
    vt1860_click(any, window, cx, word, MouseButton::Right);
    let menu = vt1684_menu(any, window, cx);
    let card = card_state(window, cx);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ② menu_open={} card={card:?}",
        menu.is_some()
    );
    check(
        menu.is_some() && card.is_none(),
        &format!("visual-test {LABEL} ②: カードが出ている間に右クリックするとカードが閉じてメニューが開く ({card:?})"),
    );
    vt1684_close(any, window, cx);

    // ③ 識別子でない所（字下げ・`=`）: 「ホバー情報を表示」は出ない
    for (what, range) in [("字下げ", 1..2usize), ("記号", 14..15)] {
        let p = vt1684_point(window, cx, pane, 1, range)
            .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ③: {what}の位置")));
        vt1860_click(any, window, cx, p, MouseButton::Right);
        let (section, drawn) = vt1684_menu(any, window, cx).unwrap_or_default();
        println!("TAKO_VISUAL_PIXEL: {LABEL} ③ {what} section={section:?} drawn={drawn:?}");
        check(
            section.is_none() && !drawn.contains(&hover_id),
            &format!("visual-test {LABEL} ③: {what}の右クリックには出ない ({drawn:?})"),
        );
        vt1684_close(any, window, cx);
    }

    // ④ キー: 編集カーソル（3 行目の total の中）の位置でカード。本文は変わらない。巨大な doc は切る
    put_cursor(window, cx, pane, 3, 14);
    notify_and_draw(any, window, cx);
    let before = buffer_version(window, cx, pane);
    vt1860_press(any, window, cx, hover_key());
    let shown = until(any, window, cx, Duration::from_secs(20), &|cx| {
        card_state(window, cx).is_some_and(|(line, ..)| line == 2)
    })
    .await;
    let card = card_state(window, cx);
    let after = buffer_version(window, cx, pane);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ④ key={} card={card:?} version={before:?}->{after:?} total_chars={total_chars}",
        hover_key()
    );
    dump(any, cx, "hover-1893-key-card.png");
    check(
        shown && card.as_ref().is_some_and(|(_, explicit, ..)| *explicit),
        &format!(
            "visual-test {LABEL} ④: キー（{}）で編集カーソルの位置にカードが出る ({card:?})",
            hover_key()
        ),
    );
    check(
        before.is_some() && before == after,
        &format!("visual-test {LABEL} ④: キーは本文を 1 字も変えない ({before:?} → {after:?})"),
    );
    check(
        card.as_ref()
            .is_some_and(|(.., cut)| *cut == Some(total_chars)),
        &format!("visual-test {LABEL} ④: 巨大な doc はカードでは既定の上限で切る ({card:?})"),
    );
    vt1860_press(any, window, cx, "escape");
    check(
        card_state(window, cx).is_none(),
        &format!("visual-test {LABEL} ④: Esc でカードが閉じる"),
    );

    // ⑤ 補完の一覧が出ている間のキー: 一覧を閉じてカードを出す（同じ語の真下に重ねない）
    put_cursor(window, cx, pane, 2, 18);
    completion_type(any, cx, 't');
    let popup = completion_wait_popup(any, window, cx, 300).await;
    vt1860_press(any, window, cx, hover_key());
    let shown = until(any, window, cx, Duration::from_secs(20), &|cx| {
        card_state(window, cx).is_some()
    })
    .await;
    let still_popup = window
        .update(cx, |app, _, _| app.lsp_completion.popup.is_some())
        .unwrap_or(true);
    let card = card_state(window, cx);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ⑤ popup={popup:?} still_popup={still_popup} card={card:?}"
    );
    dump(any, cx, "hover-1893-key-over-completion.png");
    check(
        popup.is_some(),
        &format!("visual-test {LABEL} ⑤: 打つと補完の一覧が出る（素材の前提）"),
    );
    check(
        shown && !still_popup,
        &format!(
            "visual-test {LABEL} ⑤: 一覧が出ている間のキーは一覧を閉じてカードを出す ({card:?})"
        ),
    );
    vt1860_press(any, window, cx, "escape");

    if let Some(name) = override_env {
        std::env::remove_var(name);
    }
    std::env::remove_var("TAKO_LSP_FAKE_COMPLETION");
    std::env::remove_var("TAKO_LSP_FAKE_HOVER");
    let _ = std::fs::remove_file(rules_file);
    let _ = std::fs::remove_dir_all(&dir);
}

/// visual-test `hover-loading`（偽サーバの `loading`）。相は冒頭の説明 ①〜④
pub(super) async fn hover_loading_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "hover-loading";
    inject_section_failure(LABEL);
    // 読み込みの長さは偽サーバが起動のたびに env から読む（scene が編集モードへ入った時点で起きる）
    std::env::set_var("TAKO_LSP_FAKE_SCENARIO", "loading");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_MS", "6000");
    std::env::set_var("TAKO_LSP_HOVER_DELAY_MS", "0");
    let source = "fn main() {\n    let total = 1;\n    let v = total;\n}\n";
    let rules = serde_json::json!([
        { "line": 1, "result": { "contents": { "kind": "markdown", "value": "# total\n\nThe running total." } } },
        { "echo": true },
    ]);
    let (pane, dir, override_env, rules_file) =
        hover_scene(any, window, cx, LABEL, rules, source).await;
    let path = window
        .update(cx, |app, _, _| {
            app.preview_edits
                .get(&pane)
                .map(|e| e.buffer.path().to_path_buf())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 編集バッファが無い")));
    let loading = move |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.lsp.server_loading(&path))
            .unwrap_or(false)
    };
    let note = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.lsp_hover.loading_bounds)
            .ok()
            .flatten()
    };
    wait_linked(any, window, cx, pane, LABEL).await;
    check(
        until(any, window, cx, Duration::from_secs(10), &|cx| loading(cx)).await,
        &format!("visual-test {LABEL}: 偽サーバが読み込み中と知らせる（素材の前提）"),
    );
    let on_word = hover_point(window, cx, pane, 1, 9)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 2 行目の total の位置")));
    let away = away_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 1 行目の行末より右の位置")));

    // ① 読み込み中に乗せる → 「読み込み中」の 1 行
    let started = std::time::Instant::now();
    hover_move(any, cx, on_word);
    let shown = until(any, window, cx, Duration::from_secs(3), &|cx| {
        note(cx).is_some()
    })
    .await;
    let still_loading = loading(cx);
    let state = window
        .update(cx, |app, _, _| app.lsp_hover.debug_state())
        .unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ① note={shown} still_loading={still_loading} {state}");
    check(
        still_loading,
        &format!(
            "visual-test {LABEL} ①: 乗せた時点でまだ読み込み中（素材の前提。読み込みを延ばす）"
        ),
    );
    check(
        shown,
        &format!("visual-test {LABEL} ①: 読み込み中に乗せると「読み込み中」の 1 行が出る (#1893)"),
    );
    // 実ピクセル: 基準画像（同じ場面から 1 行だけを外した 1 枚）との差分が矩形の外に無い
    let Some((with_note, scale)) = capture_frame(any, cx) else {
        fail(&format!("visual-test {LABEL}: フレーム採取"))
    };
    let bounds =
        note(cx).unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 1 行の矩形が無い")));
    let saved = window
        .update(cx, |app, _, cx| {
            let saved = app.lsp_hover.loading.take();
            cx.notify();
            saved
        })
        .ok()
        .flatten();
    let Some((reference, _)) = capture_frame(any, cx) else {
        fail(&format!("visual-test {LABEL}: 基準フレーム採取"))
    };
    let _ = window.update(cx, |app, _, cx| {
        app.lsp_hover.loading = saved;
        cx.notify();
    });
    notify_and_draw(any, window, cx);
    let viewport = window
        .update(cx, |app, _, _| app.preview_viewport_bounds(pane))
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: ビューポート矩形")));
    let (width, height) = with_note.dimensions();
    // 影（`shadow_lg`）が縁の外へ落ちるぶんを含める（補完の「読み込み中」と同じ）
    let margin = 10.0 + 2.0 * 15.0;
    let within = |b: &Bounds<Pixels>, m: f32, lx: f32, ly: f32| {
        lx >= f32::from(b.left()) - m
            && lx <= f32::from(b.right()) + m
            && ly >= f32::from(b.top()) - m
            && ly <= f32::from(b.bottom()) + m
    };
    let count = |flip: bool| {
        let (mut inner, mut outer) = (0usize, 0usize);
        for y in 0..height {
            for x in 0..width {
                if with_note.get_pixel(x, y) == reference.get_pixel(x, y) {
                    continue;
                }
                let ly = if flip { height - 1 - y } else { y } as f32 / scale;
                let lx = x as f32 / scale;
                if within(&bounds, margin, lx, ly) {
                    inner += 1;
                } else if within(&viewport, 0.0, lx, ly) {
                    outer += 1;
                }
            }
        }
        (inner, outer)
    };
    let (a, b) = (count(false), count(true));
    let (inner, outer) = if a.0 >= b.0 { a } else { b };
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ① note bounds={:.1}x{:.1}@{:.1},{:.1} inner={inner} outer={outer}",
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        f32::from(bounds.left()),
        f32::from(bounds.top()),
    );
    if let Ok(dump_dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dump_dir = std::path::Path::new(&dump_dir);
        let _ = std::fs::create_dir_all(dump_dir);
        for (frame, name) in [
            (&with_note, "hover-loading-note.png"),
            (&reference, "hover-loading-note-reference.png"),
        ] {
            let path = dump_dir.join(name);
            if frame.save(&path).is_ok() {
                println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
            }
        }
    }
    check(
        inner > 0,
        &format!("visual-test {LABEL} ①: 「読み込み中」が描かれている（差分 {inner} px）"),
    );
    check(
        outer == 0,
        &format!("visual-test {LABEL} ①: 基準画像との差分が 1 行の矩形の外にある（{outer} px）"),
    );

    // ② マウスを動かさずに待つ → 読み込みの後にカードへ差し替わる。窓のマウス位置がずれていた
    //    （実機のマウスが割り込んで語から外れた）ときだけ置き直す（置いた位置のままで出なければ不具合）
    let mut interfered = 0;
    let mut swapped = false;
    for _ in 0..3 {
        swapped = until(any, window, cx, Duration::from_secs(20), &|cx| {
            card_state(window, cx).is_some()
        })
        .await;
        let moved = window
            .update(cx, |_, win, _| win.mouse_position() != on_word)
            .unwrap_or(false);
        if swapped || !moved {
            break;
        }
        interfered += 1;
        hover_move(any, cx, on_word);
    }
    let card = card_state(window, cx);
    let (after_note, after_loading) = (note(cx), loading(cx));
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ② swapped={swapped} in {:.2}s card={card:?} note={after_note:?} loading={after_loading} interfered={interfered}",
        started.elapsed().as_secs_f32()
    );
    dump(any, cx, "hover-loading-card.png");
    check(
        swapped && card.as_ref().is_some_and(|(line, explicit, head, _)| {
            *line == 1 && !explicit && head.contains("total")
        }),
        &format!("visual-test {LABEL} ②: 読み込みが済むとマウスを動かさずにカードが出る (#1893) ({card:?})"),
    );
    check(
        after_note.is_none() && !after_loading,
        &format!("visual-test {LABEL} ②: カードへ差し替わったら「読み込み中」は消える"),
    );
    hover_move(any, cx, away);
    notify_and_draw(any, window, cx);

    // ③ もう一度読み込ませる（再起動）→ 乗せて 1 行 → 語から外れると消え、待ちも残らない
    let _ = window.update(cx, |app, _, _| app.lsp.restart(None));
    check(
        until(any, window, cx, Duration::from_secs(10), &|cx| loading(cx)).await,
        &format!("visual-test {LABEL} ③: 再起動で読み込み中に戻る（素材の前提）"),
    );
    hover_move(any, cx, on_word);
    let shown = until(any, window, cx, Duration::from_secs(3), &|cx| {
        note(cx).is_some()
    })
    .await;
    hover_move(any, cx, away);
    notify_and_draw(any, window, cx);
    let gone = note(cx).is_none();
    let drained = until(any, window, cx, Duration::from_secs(3), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.lsp_hover.inflight == 0
                    && app.lsp.status(None)["servers"][0]["pending_requests"]
                        == serde_json::json!(0)
            })
            .unwrap_or(false)
    })
    .await;
    let still_loading = loading(cx);
    let detail = window
        .update(cx, |app, _, _| {
            format!(
                "inflight={} servers={}",
                app.lsp_hover.inflight,
                app.lsp.status(None)["servers"]
            )
        })
        .unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ③ note={shown} gone={gone} drained={drained} still_loading={still_loading} {detail}");
    check(
        shown && gone,
        &format!("visual-test {LABEL} ③: 待つあいだに語から外れると「読み込み中」が消える"),
    );
    check(
        drained && still_loading,
        &format!("visual-test {LABEL} ③: 読み込みが済む前に待ちを抜ける（待ち 0・取り消しの答えまで済む）"),
    );

    // ④ 読み込みが終わらない: 上限（3 秒）で 1 行が消え、`loading`（カードは出ない）
    std::env::set_var("TAKO_LSP_FAKE_LOADING_MS", "600000");
    std::env::set_var("TAKO_LSP_HOVER_TIMEOUT_SECS", "3");
    let _ = window.update(cx, |app, _, _| app.lsp.restart(None));
    check(
        until(any, window, cx, Duration::from_secs(10), &|cx| loading(cx)).await,
        &format!("visual-test {LABEL} ④: 再起動で読み込み中に戻る（素材の前提）"),
    );
    hover_move(any, cx, on_word);
    let shown = until(any, window, cx, Duration::from_secs(3), &|cx| {
        note(cx).is_some()
    })
    .await;
    let ended = until(any, window, cx, Duration::from_secs(15), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.lsp_hover.last_failure == Some("loading")
            })
            .unwrap_or(false)
    })
    .await;
    notify_and_draw(any, window, cx);
    let (gone, card) = (note(cx).is_none(), card_state(window, cx));
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ note={shown} ended={ended} gone={gone} card={card:?}");
    check(
        shown && ended && gone && card.is_none(),
        &format!("visual-test {LABEL} ④: 読み込みが終わらなければ上限で「読み込み中」が消える（loading・カードなし）"),
    );
    hover_move(any, cx, away);

    for var in [
        "TAKO_LSP_FAKE_SCENARIO",
        "TAKO_LSP_FAKE_LOADING_MS",
        "TAKO_LSP_HOVER_DELAY_MS",
        "TAKO_LSP_HOVER_TIMEOUT_SECS",
        "TAKO_LSP_FAKE_COMPLETION",
        "TAKO_LSP_FAKE_HOVER",
    ] {
        std::env::remove_var(var);
    }
    if let Some(name) = override_env {
        std::env::remove_var(name);
    }
    let _ = std::fs::remove_file(rules_file);
    let _ = std::fs::remove_dir_all(&dir);
}

/// visual-test `hover-loading-real`（実の rust-analyzer。暖機しない）
pub(super) async fn hover_loading_real_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "hover-loading-real";
    let spec = tako_core::lsp::servers::resolve_in(
        tako_core::lsp::servers::SERVERS,
        std::path::Path::new("main.rs"),
    )
    .expect("rs を受け持つサーバが表に在る")
    .spec;
    let overridden = std::env::var(tako_core::lsp::servers::override_env_name(spec.id))
        .ok()
        .is_some_and(|p| std::path::Path::new(&p).is_file());
    if !overridden && tako_core::platform::exe::find(spec.program).is_none() {
        println!("TAKO_VISUAL_1893_REAL: SKIPPED（{} が無い）", spec.program);
        return;
    }
    inject_section_failure(LABEL);
    std::env::set_var("TAKO_LSP_HOVER_DELAY_MS", "0");
    let source = "fn main() {\n    let s = String::new();\n    println!(\"{}\", s.len());\n}\n";
    let (pane, dir, _) = completion_scene(any, window, cx, LABEL, None, source, (1, 0)).await;
    wait_linked(any, window, cx, pane, LABEL).await;
    // manager が知っているのは編集バッファのパス（一時ディレクトリは正規化で綴りが変わる = /private/var）
    let path = window
        .update(cx, |app, _, _| {
            app.preview_edits
                .get(&pane)
                .map(|e| e.buffer.path().to_path_buf())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 編集バッファが無い")));
    let at = hover_point(window, cx, pane, 1, 13)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: String の位置")));
    // 暖機しない: 起動 / 読み込みの最中に乗せる。前提（読み込み中だった）は新旧どちらの腕でも同じ形で
    // 測る = 乗せてから 1 秒のあいだに 1 度でもサーバの起動中 / 読み込み中を観測したか
    let started = std::time::Instant::now();
    hover_move(any, cx, at);
    let loading_seen = std::cell::Cell::new(false);
    let note_seen: std::cell::Cell<Option<Duration>> = std::cell::Cell::new(None);
    let _ = until(any, window, cx, Duration::from_secs(5), &|cx| {
        let (loading, note, card) = window
            .update(cx, |app, _, _| {
                (
                    app.lsp.server_loading(&path),
                    app.lsp_hover.loading_bounds.is_some(),
                    app.lsp_hover.card.is_some(),
                )
            })
            .unwrap_or((false, false, false));
        loading_seen.set(loading_seen.get() || loading);
        if note && note_seen.get().is_none() {
            note_seen.set(Some(started.elapsed()));
        }
        // 1 秒は前提を測り続ける。1 行が出た / カードが出たら抜ける
        started.elapsed() >= Duration::from_secs(1) && (note_seen.get().is_some() || card)
    })
    .await;
    let loading_at_move = loading_seen.get();
    let note = note_seen.get().is_some();
    let note_after = note_seen.get().unwrap_or_default();
    if note {
        dump(any, cx, "hover-loading-real-note.png");
    }
    let mut interfered = 0;
    let mut shown = false;
    for _ in 0..2 {
        shown = until(any, window, cx, Duration::from_secs(90), &|cx| {
            card_state(window, cx).is_some()
        })
        .await;
        let moved = window
            .update(cx, |_, win, _| win.mouse_position() != at)
            .unwrap_or(false);
        if shown || !moved {
            break;
        }
        interfered += 1;
        hover_move(any, cx, at);
    }
    let card = card_state(window, cx);
    let state = window
        .update(cx, |app, _, _| app.lsp_hover.debug_state())
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} loading_at_move={loading_at_move} note={note} (+{:.2}s) card_after={:.1}s card={card:?} interfered={interfered} {state}",
        note_after.as_secs_f32(),
        started.elapsed().as_secs_f32()
    );
    dump(any, cx, "hover-loading-real.png");
    check(
        loading_at_move,
        &format!("visual-test {LABEL}: 乗せた時点でサーバが起動中 / 読み込み中（素材の前提）"),
    );
    check(
        shown && card.as_ref().is_some_and(|(_, explicit, ..)| !explicit),
        &format!("visual-test {LABEL}: 読み込みの最中に乗せたマウスに、読み込みが済んだらカードが出る (#1893) ({card:?})"),
    );
    // 続けてキー: `s.len()` の len（3 行目）に編集カーソルを置いて押す
    let away = away_point(window, cx, pane).unwrap_or(at);
    hover_move(any, cx, away);
    notify_and_draw(any, window, cx);
    let col = "    println!(\"{}\", s.l".len();
    put_cursor(window, cx, pane, 3, col);
    vt1860_press(any, window, cx, hover_key());
    let keyed = until(any, window, cx, Duration::from_secs(60), &|cx| {
        card_state(window, cx).is_some_and(|(line, explicit, ..)| line == 2 && explicit)
    })
    .await;
    let card = card_state(window, cx);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} key={} card={card:?}",
        hover_key()
    );
    dump(any, cx, "hover-key-real.png");
    check(
        keyed,
        &format!(
            "visual-test {LABEL}: キー（{}）で len のカードが出る ({card:?})",
            hover_key()
        ),
    );
    std::env::remove_var("TAKO_LSP_HOVER_DELAY_MS");
    let _ = std::fs::remove_dir_all(&dir);
}
