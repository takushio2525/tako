//! visual-test `completion-loading-limit` / `hover-key-loading` / `hover-footer-1899`（Issue #1899）:
//! LSP の続き 3 の GUI の 3 点を**実 GUI の打鍵・キーの経路**で確かめる（偽サーバ）。
//!
//! - `completion-loading-limit`（偽サーバの `loading`。読み込みは終わらない = 合図を作らない。上限は
//!   `TAKO_LSP_COMPLETION_TIMEOUT_SECS=2` へ縮める = 負荷は使わない）: ①読み込み中に打つと「読み込み中」の
//!   1 行 ②上限に当たると**黙って消えずに**同じ置き場が理由の 1 行へ差し替わる（基準画像との差分は矩形の
//!   外 0 px）③語の外へ出る（空白）と消える ④打ち直すと「読み込み中」へ戻り、また上限で理由の 1 行
//! - `hover-key-loading`（偽サーバの `loading`。読み込みの終わりは合図のファイルでこの節が決める = #1930）:
//!   ①読み込み中にキー（⇧⌘H / Ctrl+Shift+H）で頼むと、マウスと同じく**語の真下**に「読み込み中」の
//!   1 行（ヘッダは問い合わせ中の印だけ）②合図で読み込みが済むとカードへ差し替わり 1 行は消える
//!   ③再起動で読み込み中へ戻し、右クリックの「ホバー情報を表示」で頼んでも同じく語の真下に 1 行
//!   （メニューへの割り込みは開き直して `interfered=` に数える = `vt1899_menu`）→ 合図でカード
//! - `hover-footer-1899`（偽サーバ。40,000 字を超える doc）: キーで出したカードは 16,000 字で切り、
//!   脚注に人向けの全文の取り方（`tako lsp hover --pane N --line L --column C --full`）が出る
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1899_LEGACY=1`（#1899 前）は `completion-loading-limit` の
//! ②（黙って消える）・`hover-key-loading` の ①（語の真下に出ない）・`hover-footer-1899`（脚注に取り方が
//! 無い）で名指しの FAILED になる = 同一バイナリでの A/B。画像は `TAKO_VISUAL_DUMP_DIR` の下へ
//! 節の名前とテーマつきで落とす（両テーマ）

use super::*;

/// 編集カーソルの位置のホバーのキー（**綴りはここに直書き**する = 表から引くと、表が壊れても
/// 同じ壊れ方で緑になる。hover-1893 と同じ方針）
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

fn set_theme(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, mode: &str) {
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Theme {
                action: Some("set".into()),
                mode: Some(mode.into()),
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
}

/// 両テーマの 1 枚ずつを落とす（暗 → 明 → 暗へ戻す）。画面の状態は変えない（テーマだけ）
fn dump_both_themes(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    stem: &str,
) {
    if std::env::var_os("TAKO_VISUAL_DUMP_DIR").is_none() {
        return;
    }
    for mode in ["dark", "light"] {
        set_theme(window, cx, mode);
        notify_and_draw(any, window, cx);
        notify_and_draw(any, window, cx);
        dump(any, cx, &format!("{stem}-{mode}.png"));
    }
    set_theme(window, cx, "dark");
    notify_and_draw(any, window, cx);
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

/// 編集バッファのパス（manager が知っている綴り。一時ディレクトリは正規化で綴りが変わりうる）
fn buffer_path(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
) -> std::path::PathBuf {
    window
        .update(cx, |app, _, _| {
            app.preview_edits
                .get(&pane)
                .map(|e| e.buffer.path().to_path_buf())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {label}: 編集バッファが無い")))
}

/// 起動が済んで（握手を終えて）から読み込み中になったのを待つ（`server_loading` は起動中も真 =
/// それだけで進むと起動中に頼む。#1930。`completion-loading` / `hover-loading` と同じ前提）
async fn wait_running_loading(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    path: &std::path::Path,
    label: &str,
) {
    check(
        vt1860_wait(any, window, cx, Duration::from_secs(30), &|cx| {
            window
                .update(cx, |app, _, _| {
                    let server = &app.lsp.status(None)["servers"][0];
                    server["state"] == "running"
                        && server["loading"] == true
                        && app.lsp.server_loading(path)
                })
                .unwrap_or(false)
        })
        .await,
        &format!("visual-test {label}: 偽サーバが起動して読み込み中と知らせる（素材の前提）"),
    );
}

/// 補完の「読み込み中」の印の今の状態（出ていれば（上限の 1 行か, その文））
fn completion_note(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> Option<Option<String>> {
    window
        .update(cx, |app, _, _| {
            app.lsp_completion
                .loading
                .as_ref()
                .map(|note| note.limit.clone())
        })
        .ok()
        .flatten()
}

/// 基準画像（同じ場面から 1 行だけを外した 1 枚）との差分が、1 行の矩形の内に十分あり外に無いか。
/// 返り値は（内の差分, 外の差分, 内の下限）
fn note_pixels(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
) -> (usize, usize, usize) {
    let Some((with_note, scale)) = capture_frame(any, cx) else {
        fail(&format!("visual-test {label}: フレーム採取"))
    };
    let bounds = window
        .update(cx, |app, _, _| app.lsp_completion.loading_bounds)
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {label}: 1 行の矩形が無い")));
    let saved = window
        .update(cx, |app, _, cx| {
            let saved = app.lsp_completion.loading.take();
            cx.notify();
            saved
        })
        .ok()
        .flatten();
    let Some((reference, _)) = capture_frame(any, cx) else {
        fail(&format!("visual-test {label}: 基準フレーム採取"))
    };
    let _ = window.update(cx, |app, _, cx| {
        app.lsp_completion.loading = saved;
        cx.notify();
    });
    notify_and_draw(any, window, cx);
    let viewport = window
        .update(cx, |app, _, _| app.preview_viewport_bounds(pane))
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {label}: ビューポート矩形")));
    // 影（`shadow_lg`）が縁の外へ落ちるぶんを含める（`completion-loading` 節と同じ幅）
    let margin = 10.0 + 2.0 * 15.0;
    let within = |b: &Bounds<Pixels>, m: f32, lx: f32, ly: f32| {
        lx >= f32::from(b.left()) - m
            && lx <= f32::from(b.right()) + m
            && ly >= f32::from(b.top()) - m
            && ly <= f32::from(b.bottom()) + m
    };
    let (width, height) = with_note.dimensions();
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
    // Metal の読み戻しは上下の向きがありうるので、差分が多く収まる側を採る（#812 の作法）
    let (a, b) = (count(false), count(true));
    let (inner, outer) = if a.0 >= b.0 { a } else { b };
    // 縁と 1 行の字のぶんは必ず変わる（器の周の長さの半分を下限にする）
    let need = ((f32::from(bounds.size.width) + f32::from(bounds.size.height)) * scale) as usize;
    (inner, outer, need)
}

/// visual-test `completion-loading-limit`（偽サーバの `loading`・読み込みは終わらない）。相は冒頭の ①〜④
pub(super) async fn completion_loading_limit_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "completion-loading-limit";
    const LIMIT_SECS: u64 = 2;
    inject_section_failure(LABEL);
    let work =
        std::env::temp_dir().join(format!("tako-visual-1899-{LABEL}-{}", std::process::id()));
    remove_on_fail(&work);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("visual-test completion-loading-limit の置き場");
    // 合図のファイルは作らない = 読み込みは終わらない（上限に当たるのを実時間の負荷に任せない）
    std::env::set_var("TAKO_LSP_FAKE_SCENARIO", "loading");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_MS", "0");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_UNTIL", work.join("never"));
    std::env::set_var("TAKO_LSP_FAKE_LOG", work.join("received.jsonl"));
    std::env::set_var("TAKO_LSP_COMPLETION_TIMEOUT_SECS", LIMIT_SECS.to_string());
    let source = "fn main() {\n    let total = 1;\n    \n}\n";
    let rules = serde_json::json!({ "items": [{ "label": "alpha" }, { "label": "beta" }] });
    let (pane, dir, override_env) =
        completion_scene(any, window, cx, LABEL, Some(rules), source, (3, 4)).await;
    let path = buffer_path(window, cx, pane, LABEL);
    wait_running_loading(any, window, cx, &path, LABEL).await;
    let want = tako_control::lsp::text::fill(
        tako_control::lsp::text::COMPLETION_LOADING_LIMIT_NOTE,
        &[("secs", &LIMIT_SECS.to_string())],
    );

    // ① 読み込み中に 1 文字 → 「読み込み中」の 1 行（まだ上限の文ではない）
    let typed = std::time::Instant::now();
    completion_type(any, cx, 'a');
    let loading_note = vt1860_wait(any, window, cx, Duration::from_secs(3), &|cx| {
        completion_note(window, cx) == Some(None)
    })
    .await;
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ① loading_note={loading_note} note={:?}",
        completion_note(window, cx)
    );
    check(
        loading_note,
        &format!("visual-test {LABEL} ①: 読み込み中に打つと「読み込み中」の 1 行が出る（素材の前提 = #1869）"),
    );

    // ② 上限（2 秒）に当たる → 黙って消えずに理由の 1 行へ差し替わる
    let settled = vt1860_wait(any, window, cx, Duration::from_secs(15), &|cx| {
        completion_note(window, cx) != Some(None)
    })
    .await;
    let elapsed = typed.elapsed();
    notify_and_draw(any, window, cx);
    let note = completion_note(window, cx);
    let drawn = window
        .update(cx, |app, _, _| app.lsp_completion.loading_bounds.is_some())
        .unwrap_or(false);
    let popup = window
        .update(cx, |app, _, _| app.lsp_completion.popup.is_some())
        .unwrap_or(true);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ② settled={settled} after={:.2}s note={note:?} drawn={drawn} popup={popup}",
        elapsed.as_secs_f32()
    );
    check(
        settled,
        &format!("visual-test {LABEL} ②: 上限まで待つと「読み込み中」が次の状態へ移る（素材の前提）"),
    );
    check(
        note == Some(Some(want.clone())) && drawn && !popup,
        &format!(
            "visual-test {LABEL} ②: 上限に当たっても黙って消えず、理由の 1 行が出る (#1899) ({note:?} / drawn={drawn})"
        ),
    );
    check(
        elapsed >= Duration::from_secs(LIMIT_SECS),
        &format!(
            "visual-test {LABEL} ②: 理由の 1 行は上限（{LIMIT_SECS} 秒）より前には出ない（{:.2}s）",
            elapsed.as_secs_f32()
        ),
    );
    let (inner, outer, need) = note_pixels(any, window, cx, pane, LABEL);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② pixels inner={inner} outer={outer} need={need}");
    check(
        inner >= need,
        &format!("visual-test {LABEL} ②: 理由の 1 行が描かれている（差分 {inner} < {need} px）"),
    );
    check(
        outer == 0,
        &format!("visual-test {LABEL} ②: 基準画像との差分が 1 行の矩形の外にある（{outer} px）"),
    );
    dump_both_themes(any, window, cx, "completion-loading-limit");

    // ③ 語の外へ出る（空白）と消える
    completion_type(any, cx, ' ');
    let gone = vt1860_wait(any, window, cx, Duration::from_secs(3), &|cx| {
        completion_note(window, cx).is_none()
    })
    .await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ③ space_cleared={gone}");
    check(
        gone,
        &format!("visual-test {LABEL} ③: 語の外へ出る（空白）と理由の 1 行は消える"),
    );

    // ④ 打ち直すと「読み込み中」へ戻り、また上限で理由の 1 行（前の 1 行が残り続けない）
    completion_type(any, cx, 'b');
    let again = vt1860_wait(any, window, cx, Duration::from_secs(3), &|cx| {
        completion_note(window, cx) == Some(None)
    })
    .await;
    let limited_again = vt1860_wait(any, window, cx, Duration::from_secs(15), &|cx| {
        completion_note(window, cx) == Some(Some(want.clone()))
    })
    .await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ loading_again={again} limited_again={limited_again}");
    check(
        again && limited_again,
        &format!("visual-test {LABEL} ④: 打ち直すと「読み込み中」へ戻り、また上限で理由の 1 行が出る"),
    );
    completion_type(any, cx, ' ');

    for var in [
        "TAKO_LSP_FAKE_SCENARIO",
        "TAKO_LSP_FAKE_LOADING_MS",
        "TAKO_LSP_FAKE_LOADING_UNTIL",
        "TAKO_LSP_FAKE_LOG",
        "TAKO_LSP_COMPLETION_TIMEOUT_SECS",
        "TAKO_LSP_FAKE_COMPLETION",
    ] {
        std::env::remove_var(var);
    }
    if let Some(name) = override_env {
        std::env::remove_var(name);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&work);
}

/// 語（行 `line` は 0 起点・`range` は行内 UTF-8 バイト）の矩形の左端と下端（ウィンドウ座標）
fn word_corner(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    line: usize,
    start: usize,
) -> Option<(f32, f32)> {
    window
        .update(cx, |app, _, _| {
            let layout = app.preview_text_layouts.get(&pane)?.get(line)?.clone()?;
            let at = layout.position_for_index(start)?;
            Some((f32::from(at.x), f32::from(at.y + layout.line_height())))
        })
        .ok()
        .flatten()
}

/// ホバーの「読み込み中」の 1 行の今の状態（描いていれば（きっかけ, 矩形））
fn hover_note(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) -> Option<(crate::lsp_hover_ui::HoverOrigin, Bounds<Pixels>)> {
    window
        .update(cx, |app, _, _| {
            Some((app.lsp_hover.loading_origin?, app.lsp_hover.loading_bounds?))
        })
        .ok()
        .flatten()
}

/// visual-test `hover-key-loading`（偽サーバの `loading`）。相は冒頭の ①②
pub(super) async fn hover_key_loading_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use crate::lsp_hover_ui::HoverOrigin;
    const LABEL: &str = "hover-key-loading";
    inject_section_failure(LABEL);
    let work =
        std::env::temp_dir().join(format!("tako-visual-1899-{LABEL}-{}", std::process::id()));
    remove_on_fail(&work);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("visual-test hover-key-loading の置き場");
    let gate = work.join("loading-done");
    std::env::set_var("TAKO_LSP_FAKE_SCENARIO", "loading");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_MS", "0");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_UNTIL", &gate);
    std::env::set_var("TAKO_LSP_FAKE_LOG", work.join("received.jsonl"));
    // マウスのホバーは発火させない（この節はキーの経路だけを見る）
    std::env::set_var("TAKO_LSP_HOVER_DELAY_MS", "600000");
    let source = "fn main() {\n    let total = 1;\n    let v = total;\n}\n";
    let rules = serde_json::json!([
        { "line": 1, "result": { "contents": { "kind": "markdown", "value": "# total\n\nThe running total." } } },
        { "echo": true },
    ]);
    let (pane, dir, override_env, rules_file) =
        hover_scene(any, window, cx, LABEL, rules, source).await;
    let path = buffer_path(window, cx, pane, LABEL);
    wait_running_loading(any, window, cx, &path, LABEL).await;

    // ① 読み込み中にキー → 語（2 行目の total = 8..13）の真下に「読み込み中」の 1 行
    put_cursor(window, cx, pane, 2, 10);
    notify_and_draw(any, window, cx);
    vt1860_press(any, window, cx, hover_key());
    let shown = vt1860_wait(any, window, cx, Duration::from_secs(3), &|cx| {
        hover_note(window, cx).is_some_and(|(origin, _)| origin == HoverOrigin::Explicit)
    })
    .await;
    let note = hover_note(window, cx);
    let corner = word_corner(window, cx, pane, 1, 8);
    let (header, state, still_loading) = window
        .update(cx, |app, _, _| {
            (
                app.lsp_goto_header_status(pane),
                app.lsp_hover.debug_state(),
                app.lsp.server_loading(&path),
            )
        })
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ① key={} shown={shown} note={note:?} word={corner:?} header={header:?} still_loading={still_loading} {state}",
        hover_key()
    );
    check(
        still_loading,
        &format!("visual-test {LABEL} ①: 頼んだ時点でまだ読み込み中（素材の前提。合図はまだ）"),
    );
    check(
        shown,
        &format!(
            "visual-test {LABEL} ①: 読み込み中にキーで頼むと、マウスと同じく語の真下に「読み込み中」の 1 行が出る (#1899) ({state})"
        ),
    );
    let (bounds, (word_left, word_bottom)) = match (note, corner) {
        (Some((_, bounds)), Some(corner)) => (bounds, corner),
        _ => fail(&format!("visual-test {LABEL} ①: 1 行の矩形か語の位置が無い")),
    };
    check(
        (f32::from(bounds.top()) - word_bottom).abs() < 1.0
            && (f32::from(bounds.left()) - word_left).abs() < 1.0,
        &format!(
            "visual-test {LABEL} ①: 1 行は語の頭の真下に置く（矩形 {bounds:?} / 語の左下 ({word_left}, {word_bottom})）"
        ),
    );
    check(
        header.as_ref().map(|(text, _)| text.as_str())
            == Some(crate::ui_text::preview::hover_searching()),
        &format!(
            "visual-test {LABEL} ①: 語の真下が読み込み中を言うあいだ、ヘッダは問い合わせ中の印だけ ({header:?})"
        ),
    );
    dump_both_themes(any, window, cx, "hover-key-loading");

    // ② 合図で読み込みが済む → カードへ差し替わり、1 行は消える
    std::fs::write(&gate, b"").expect("読み込みの合図");
    let carded = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.lsp_hover
                    .card
                    .as_ref()
                    .is_some_and(|c| c.line == 1 && c.origin == HoverOrigin::Explicit)
            })
            .unwrap_or(false)
    })
    .await;
    notify_and_draw(any, window, cx);
    let after = hover_note(window, cx);
    let header = window
        .update(cx, |app, _, _| app.lsp_goto_header_status(pane))
        .ok()
        .flatten();
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② card={carded} note_after={after:?} header={header:?}");
    check(
        carded && after.is_none(),
        &format!("visual-test {LABEL} ②: 読み込みが済むとカードへ差し替わり、1 行は消える ({after:?})"),
    );
    vt1860_press(any, window, cx, "escape");

    // ③ 再起動で読み込み中へ戻し（合図は別のファイル = まだ作らない）、右クリックの「ホバー情報を表示」
    //    （3 行目の total = 12..17）→ 同じく語の真下に 1 行。押した後に立つべき状態まで helper が待つ
    let menu_gate = work.join("loading-done-menu");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_UNTIL", &menu_gate);
    let _ = window.update(cx, |app, _, _| app.lsp.restart(None));
    wait_running_loading(any, window, cx, &path, LABEL).await;
    let word = vt1684_point(window, cx, pane, 2, 12..17)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ③: 3 行目の total の位置")));
    let hover_id = tako_core::lsp::menu::MenuItem::Hover.id();
    let picked = vt1899_menu(
        any,
        window,
        cx,
        word,
        Some(Duration::from_secs(20)),
        Some(hover_id),
        Duration::from_secs(3),
        &|cx| {
            hover_note(window, cx).is_some_and(|(origin, _)| origin == HoverOrigin::Explicit)
        },
        &|cx| {
            let _ = window.update(cx, |app, _, cx| {
                app.close_lsp_hover();
                cx.notify();
            });
        },
    )
    .await;
    let note = hover_note(window, cx);
    let corner = word_corner(window, cx, pane, 2, 12);
    let state = window
        .update(cx, |app, _, _| app.lsp_hover.debug_state())
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ③ menu={:?} done={} note={note:?} word={corner:?} interfered={} {state}",
        picked.drawn, picked.done, picked.interfered
    );
    picked.check_not_exhausted(&format!("{LABEL} ③"));
    check(
        picked.drawn.contains(&hover_id),
        &format!("visual-test {LABEL} ③: 識別子の右クリックに「ホバー情報を表示」が出る（素材の前提） ({:?})", picked.drawn),
    );
    let under_word = match (note, corner) {
        (Some((_, bounds)), Some((left, bottom))) => {
            (f32::from(bounds.top()) - bottom).abs() < 1.0
                && (f32::from(bounds.left()) - left).abs() < 1.0
        }
        _ => false,
    };
    check(
        picked.done && under_word,
        &format!(
            "visual-test {LABEL} ③: 右クリックの「ホバー情報を表示」で頼んでも、読み込み中は語の真下に 1 行が出る (#1899) ({note:?} / {corner:?})"
        ),
    );
    std::fs::write(&menu_gate, b"").expect("読み込みの合図");
    let carded = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.lsp_hover.card.as_ref().is_some_and(|c| c.line == 2)
            })
            .unwrap_or(false)
    })
    .await;
    notify_and_draw(any, window, cx);
    let after = hover_note(window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ③ card={carded} note_after={after:?}");
    check(
        carded && after.is_none(),
        &format!("visual-test {LABEL} ③: 読み込みが済むとカードへ差し替わり、1 行は消える ({after:?})"),
    );
    vt1860_press(any, window, cx, "escape");

    for var in [
        "TAKO_LSP_FAKE_SCENARIO",
        "TAKO_LSP_FAKE_LOADING_MS",
        "TAKO_LSP_FAKE_LOADING_UNTIL",
        "TAKO_LSP_FAKE_LOG",
        "TAKO_LSP_HOVER_DELAY_MS",
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
    let _ = std::fs::remove_dir_all(&work);
}

/// visual-test `hover-footer-1899`（偽サーバ。巨大な doc）
pub(super) async fn hover_footer_1899_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "hover-footer-1899";
    inject_section_failure(LABEL);
    std::env::set_var("TAKO_LSP_HOVER_DELAY_MS", "600000");
    // 2 行目（0 起点の 1）は 40,000 字を超える doc（カードは 16,000 字で切る）
    let big: String = (0..2500)
        .map(|i| format!("line {i} of a very long doc\n"))
        .collect();
    let total_chars = big.chars().count();
    let source = "fn main() {\n    let total = 1;\n    let v = total;\n}\n";
    let rules = serde_json::json!([
        { "line": 1, "result": { "contents": { "kind": "markdown", "value": big } } },
        { "echo": true },
    ]);
    let (pane, dir, override_env, rules_file) =
        hover_scene(any, window, cx, LABEL, rules, source).await;
    check(
        vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
            window
                .update(cx, |app, _, _| app.lsp_hover_enabled_for(pane))
                .unwrap_or(false)
        })
        .await,
        &format!("visual-test {LABEL}: 編集モードで文書がサーバへつながる（素材の前提）"),
    );
    put_cursor(window, cx, pane, 2, 10);
    notify_and_draw(any, window, cx);
    vt1860_press(any, window, cx, hover_key());
    let carded = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.lsp_hover.card.is_some() && app.lsp_hover.bounds.is_some()
            })
            .unwrap_or(false)
    })
    .await;
    for _ in 0..3 {
        notify_and_draw(any, window, cx);
    }
    let (truncated, footer, range_start) = window
        .update(cx, |app, _, _| {
            let card = app.lsp_hover.card.as_ref();
            (
                card.and_then(|c| c.truncated),
                app.lsp_hover.footer_command.clone(),
                card.map(|c| c.range.start),
            )
        })
        .unwrap_or_default();
    let want = range_start
        .map(|start| tako_control::lsp::hover::full_command(pane.as_u64(), 2, start));
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} card={carded} truncated={truncated:?} total_chars={total_chars} footer={footer:?} want={want:?}"
    );
    check(
        carded && truncated == Some(total_chars),
        &format!("visual-test {LABEL}: キーで出したカードは巨大な doc を既定の上限で切る（素材の前提） ({truncated:?})"),
    );
    check(
        footer.is_some() && footer == want,
        &format!(
            "visual-test {LABEL}: 切ったカードの脚注に人向けの全文の取り方が出る (#1899) ({footer:?} / {want:?})"
        ),
    );
    check(
        footer.as_deref().is_some_and(|c| {
            c.starts_with("tako lsp hover ") && c.ends_with(" --full") && c.contains("--line 2 ")
        }),
        &format!("visual-test {LABEL}: 脚注のコマンドは CLI の綴り（行 1 始まり・--full）({footer:?})"),
    );
    dump_both_themes(any, window, cx, "hover-footer-1899");
    vt1860_press(any, window, cx, "escape");

    std::env::remove_var("TAKO_LSP_HOVER_DELAY_MS");
    std::env::remove_var("TAKO_LSP_FAKE_COMPLETION");
    std::env::remove_var("TAKO_LSP_FAKE_HOVER");
    if let Some(name) = override_env {
        std::env::remove_var(name);
    }
    let _ = std::fs::remove_file(rules_file);
    let _ = std::fs::remove_dir_all(&dir);
}
