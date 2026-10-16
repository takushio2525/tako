//! visual-test `bracket-match`（FR-3.5 / Issue #1691）: カーソルを括弧に置くと対応する括弧が
//! **実ピクセルで**強調され、対が無いときは強調が出ないか。
//!
//! 場面: 一時 dir の `brackets.rs` を dispatch の `OpenFile`（code）で開いて編集モードにする。
//! 基準のフレームはカーソルを括弧に接しない位置（1 行目の頭）に置いて撮る。
//! ① `call(` の `(` の直前へ置く（dispatch `PreviewCursor` = CLI `tako edit cursor` / MCP
//! `tako_preview_cursor` と同じ口）と、応答の `document.bracket_match` が `(` と `)` を指し、
//! 2 つの括弧の矩形の画素が基準から変わる（`(` はキャレットの 1.5px を避けて左端を除く）。
//! あいだの `a` と、別の種類の `[` は変わらない（強調は対の 2 文字だけ）
//! ② 実キーの → で `(` の直後へ動かしても同じ組（直前の括弧を採る）、もう 1 回 → で括弧から
//! 離れると強調が消え、`)` の矩形が基準と同じ画素へ戻る
//! ③ 対の無い `(` の直前へ置くと `bracket_match` は null で（同じ行の `// )` はコメントなので
//! 数えない）、その `(` の矩形も、1 行目の括弧も、文字列の中の `)` も基準と同じ画素のまま。
//!
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ `bracket-match-1-matched.png` /
//! `bracket-match-2-unmatched.png` で落とす（新旧で dir を分けて回す）。
//! `TAKO_1691_LEGACY=1` は対応括弧を探さないので ① が FAILED = 同一バイナリでの A/B。
//! 単独実行は `TAKO_VISUAL_ONLY=bracket-match`

use super::*;

const SECTION: &str = "bracket-match";

/// 場面の本文（行は 0 始まりで、1 = 対のある括弧・2 = 文字列の中の `)`・3 = 対の無い `(`。
/// 3 行目の行コメントの `)` を数えると `(` に対ができてしまう = コメントを飛ばしているかも見る）
const SOURCE: &str = "\
fn main() {
    let v = call(a, [b, c]);
    let s = \")\";
    (unmatched; // )
}
";

/// 1 行目（0 始まり）の `call(` の `(` / `a` / `[` / `)` の桁
const OPEN_COL: usize = 16;
const INNER_COL: usize = 17;
const SQUARE_COL: usize = 20;
const CLOSE_COL: usize = 26;
/// 2 行目の文字列の中の `)` の桁
const QUOTED_COL: usize = 13;
/// 3 行目の対の無い `(` の桁（同じ行の `// )` はコメント）
const LONE_COL: usize = 4;

/// キャレット（幅 1.5px）を避けるために左から削る幅（論理 px）
const CARET_GUARD: f32 = 3.0;

/// 2 枚のフレームの矩形（論理 px）の中で画素が違う数と、矩形の画素数。
/// `flip` は Metal の読み戻しの上下（#812 の作法で、変化が見つかる側を採る）
fn diff_in(
    a: &image::RgbaImage,
    b: &image::RgbaImage,
    rect: Bounds<Pixels>,
    scale: f32,
    flip: bool,
) -> (u64, u64) {
    let (w, h) = a.dimensions();
    if b.dimensions() != (w, h) {
        return (u64::MAX, 0);
    }
    let x0 = ((f32::from(rect.left()) * scale).ceil().max(0.0) as u32).min(w);
    let x1 = ((f32::from(rect.right()) * scale).floor().max(0.0) as u32).min(w);
    let t = ((f32::from(rect.top()) * scale).ceil().max(0.0) as u32).min(h);
    let bt = ((f32::from(rect.bottom()) * scale).floor().max(0.0) as u32).min(h);
    let (y0, y1) = if flip {
        (h.saturating_sub(bt), h.saturating_sub(t))
    } else {
        (t, bt)
    };
    let mut changed = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if a.get_pixel(x, y) != b.get_pixel(x, y) {
                changed += 1;
            }
        }
    }
    (changed, u64::from(x1 - x0) * u64::from(y1 - y0))
}

/// 左端を削った矩形（キャレットが括弧の左端に立つ側）
fn without_caret(rect: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(rect.origin.x + px(CARET_GUARD), rect.origin.y),
        size: gpui::size(rect.size.width - px(CARET_GUARD), rect.size.height),
    }
}

pub(super) async fn bracket_match_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::Request as Req;
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    inject_section_failure(SECTION);
    let base = ensure_fresh_scene(window, cx, SECTION).await;
    let dir = std::env::temp_dir().join(format!("tako-visual-1691-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("visual-test bracket-match 一時ディレクトリ");
    let path = dir.join("brackets.rs");
    std::fs::write(&path, SOURCE).expect("visual-test bracket-match の素材");

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
                    direction: Some(tako_control::protocol::Direction::Right),
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            )
            .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 開けない: {e:?}")));
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
    check(
        wait_for_preview_maps(any, window, cx, pane, false).await,
        &format!("visual-test {SECTION}: 前提 = 座標キャッシュが揃う"),
    );
    let editing = window
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
        .unwrap_or(false);
    check(
        editing,
        &format!("visual-test {SECTION}: 前提 = 編集モードを開始できる"),
    );

    // カーソルを置いて応答の `document.bracket_match` を返す（CLI / MCP と同じ dispatch）
    let place = |cx: &mut AsyncApp, line: usize, col: usize| -> serde_json::Value {
        let reply = window
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
                r.unwrap_or_else(|e| {
                    fail(&format!(
                        "visual-test {SECTION}: カーソルを {line}:{col} へ置けない: {e:?}"
                    ))
                })
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: PreviewCursor")));
        notify_and_draw(any, window, cx);
        reply["document"]["bracket_match"].clone()
    };
    // GUI の状態（実キーの後は応答が無いので、描画が引くのと同じ `bracket_match` を読む）
    let state = |cx: &mut AsyncApp| -> Option<(usize, usize)> {
        window
            .update(cx, |app, _, _| {
                let edit = app.preview_edits.get(&pane)?;
                let found = edit.buffer.bracket_match()?;
                let [open, close] = found.ordered();
                Some((open, close))
            })
            .ok()
            .flatten()
    };
    let dump = |name: &str, frame: &image::RgbaImage| {
        if let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
            let dir = std::path::Path::new(&dir);
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(name);
            if frame.save(&path).is_ok() {
                println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
            }
        }
    };

    // 基準: 括弧に接しない位置（1 行目の頭）
    let reference_match = place(cx, 1, 0);
    check(
        reference_match.is_null(),
        &format!("visual-test {SECTION}: 前提 = 括弧に接しない位置では bracket_match が null"),
    );
    for _ in 0..3 {
        notify_and_draw(any, window, cx);
        wait(cx, 30).await;
    }
    let (reference, scale) = capture_frame(any, cx)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 基準のフレームを撮れない")));

    // 文字 1 つぶんの矩形（論理 px。行は 0 始まり）
    let cell = |cx: &mut AsyncApp, line: usize, col: usize| -> Bounds<Pixels> {
        window
            .update(cx, |app, _, _| {
                let layout = app.preview_text_layouts.get(&pane)?.get(line)?.as_ref()?;
                let left = layout.position_for_index(col)?;
                let right = layout.position_for_index(col + 1)?;
                Some(Bounds {
                    origin: left,
                    size: gpui::size(right.x - left.x, layout.line_height()),
                })
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test {SECTION}: {line} 行目の {col} 桁が描かれていない"
                ))
            })
    };

    // ① `(` の直前
    let matched = place(cx, 2, OPEN_COL);
    let expected = serde_json::json!({
        "bracket": { "line": 2, "column": OPEN_COL },
        "match": { "line": 2, "column": CLOSE_COL },
    });
    let (matched_frame, _) = capture_frame(any, cx)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: ① のフレームを撮れない")));
    dump("bracket-match-1-matched.png", &matched_frame);
    let open_box = cell(cx, 1, OPEN_COL);
    let close_box = cell(cx, 1, CLOSE_COL);
    let inner_box = cell(cx, 1, INNER_COL);
    let square_box = cell(cx, 1, SQUARE_COL);
    let flip = diff_in(&reference, &matched_frame, close_box, scale, true).0
        > diff_in(&reference, &matched_frame, close_box, scale, false).0;
    let (open_changed, open_area) = diff_in(
        &reference,
        &matched_frame,
        without_caret(open_box),
        scale,
        flip,
    );
    let (close_changed, close_area) = diff_in(&reference, &matched_frame, close_box, scale, flip);
    let (inner_changed, _) = diff_in(&reference, &matched_frame, inner_box, scale, flip);
    let (square_changed, _) = diff_in(&reference, &matched_frame, square_box, scale, flip);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ① matched={matched} open={open_changed}/{open_area} \
         close={close_changed}/{close_area} inner={inner_changed} square={square_changed} \
         flip={flip} legacy={}",
        tako_core::text_edit::bracket_match_legacy()
    );
    check(
        matched == expected,
        &format!(
            "① `(` の直前に置くと応答の bracket_match が `(` と `)` を指す（実際: {matched}）(#1691)"
        ),
    );
    // 背景で塗るので、矩形の画素の大半が変わる（字形だけが変わった、では足りない）
    check(
        open_changed * 4 >= open_area && close_changed * 4 >= close_area && close_area > 0,
        &format!(
            "① 対応する 2 つの括弧が実ピクセルで強調される（`(` {open_changed}/{open_area}・\
             `)` {close_changed}/{close_area}）(#1691)"
        ),
    );
    check(
        inner_changed == 0 && square_changed == 0,
        &format!(
            "① 強調は対の 2 文字だけ（あいだの `a` {inner_changed}・別の種類の `[` {square_changed}）(#1691)"
        ),
    );

    // ② 実キーの → で `(` の直後へ（直前の括弧を採るので同じ組）、もう 1 回で括弧から離れる
    let open_at = SOURCE.find("call(").map(|at| at + 4);
    let close_at = SOURCE.find("]);").map(|at| at + 1);
    press(any, cx, "right");
    wait(cx, 50).await;
    notify_and_draw(any, window, cx);
    let after_open = state(cx);
    check(
        after_open.is_some() && after_open == open_at.zip(close_at),
        &format!(
            "② 実キーの → で `(` の直後へ動かしても同じ組を強調する（実際: {after_open:?}）(#1691)"
        ),
    );
    press(any, cx, "right");
    wait(cx, 50).await;
    notify_and_draw(any, window, cx);
    let away = state(cx);
    let (away_frame, _) = capture_frame(any, cx)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: ② のフレームを撮れない")));
    let (away_close, _) = diff_in(&reference, &away_frame, close_box, scale, flip);
    let (away_open, _) = diff_in(&reference, &away_frame, open_box, scale, flip);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ② after_open={after_open:?} away={away:?} \
         close={away_close} open={away_open}"
    );
    check(
        away.is_none() && away_close == 0 && away_open == 0,
        &format!(
            "② 括弧から離れると強調が消え、2 つの括弧が基準と同じ画素へ戻る\
             （状態 {away:?}・`)` {away_close}・`(` {away_open}）(#1691)"
        ),
    );

    // ③ 対の無い `(` の直前
    let lone = place(cx, 4, LONE_COL);
    let (lone_frame, _) = capture_frame(any, cx)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: ③ のフレームを撮れない")));
    dump("bracket-match-2-unmatched.png", &lone_frame);
    let lone_box = cell(cx, 3, LONE_COL);
    let quoted_box = cell(cx, 2, QUOTED_COL);
    let (lone_changed, lone_area) = diff_in(
        &reference,
        &lone_frame,
        without_caret(lone_box),
        scale,
        flip,
    );
    let (quoted_changed, _) = diff_in(&reference, &lone_frame, quoted_box, scale, flip);
    let (stale_open, _) = diff_in(&reference, &lone_frame, open_box, scale, flip);
    let (stale_close, _) = diff_in(&reference, &lone_frame, close_box, scale, flip);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ③ lone={lone} lone_box={lone_changed}/{lone_area} \
         quoted={quoted_changed} stale_open={stale_open} stale_close={stale_close}"
    );
    check(
        lone.is_null(),
        &format!("③ 対の無い `(` の直前では bracket_match が null（実際: {lone}）(#1691)"),
    );
    check(
        lone_area > 0
            && lone_changed == 0
            && quoted_changed == 0
            && stale_open == 0
            && stale_close == 0,
        &format!(
            "③ 対が無いときは強調が出ない（`(` {lone_changed}/{lone_area}・文字列の `)` \
             {quoted_changed}・1 行目の括弧 {stale_open}/{stale_close}）(#1691)"
        ),
    );

    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            Req::Close {
                pane: Some(pane.as_u64()),
                force: true,
                caller_role: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    let _ = std::fs::remove_dir_all(&dir);
}
