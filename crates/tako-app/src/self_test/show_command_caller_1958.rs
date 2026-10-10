//! visual-test `show-command-caller`（FR-2.22.1 / FR-2.22.8 / Issue #1958）: pane を省いた
//! MCP `tako_show_command` のカードが、**呼び出し元ペインの実ピクセルに**描かれるか。
//!
//! 場面: 素のシェル A を下へ割って B を作り、フォーカスを B へ移す（= 呼び出し元とフォーカスを分ける）。
//! MCP のエンジン（`tako_control::mcp::handle_message`）を**呼び出し元 A**の文脈で回し、
//! `tako_show_command {commands}`（pane なし）を呼ぶ。実行係は本番と同じ dispatch。
//! ① 応答が成功で、カードの所在が A ② GUI がカード帯を A にだけ割り当てる（B には無い）
//! ③ A の帯の矩形の画素が出す前から変わり（カードが描かれた）、B の同じ高さの下端は変わらない。
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ `show-command-caller.png` で落とす。
//!
//! 修正前は ① で落ちる（変換が呼び出し元で埋めず、dispatch が「対象ペインが未指定」で断る）。
//! A/B は修正を外したビルドで同じ節を回す（`scripts/test-show-command-caller-1958.sh` の visual 段）。
//! 単独実行は `TAKO_VISUAL_ONLY=show-command-caller`

use super::*;

const SECTION: &str = "show-command-caller";

/// フレームの矩形（論理 px）の中で、2 枚の画素が違う数と矩形の画素数
fn diff_in(
    before: &image::RgbaImage,
    after: &image::RgbaImage,
    rect: Bounds<Pixels>,
    scale: f32,
) -> (u64, u64) {
    let (w, h) = after.dimensions();
    let x0 = ((f32::from(rect.origin.x) * scale) as u32).min(w);
    let y0 = ((f32::from(rect.origin.y) * scale) as u32).min(h);
    let x1 = ((f32::from(rect.origin.x + rect.size.width) * scale) as u32).min(w);
    let y1 = ((f32::from(rect.origin.y + rect.size.height) * scale) as u32).min(h);
    let mut changed = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if before.dimensions() == after.dimensions()
                && before.get_pixel(x, y) != after.get_pixel(x, y)
            {
                changed += 1;
            }
        }
    }
    (changed, u64::from(x1 - x0) * u64::from(y1 - y0))
}

/// ペインのテキスト領域（`render()` が組んだ矩形）
fn text_area(app: &TakoApp, pane: PaneId) -> Option<Bounds<Pixels>> {
    app.pane_text_areas
        .iter()
        .find(|(id, _)| *id == pane)
        .map(|(_, b)| *b)
}

pub(super) async fn show_command_caller_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let a = ensure_fresh_scene(window, cx, SECTION).await;

    // B を A の下へ割ってフォーカスを移す（呼び出し元 A はフォーカスを持たない）
    let b = window
        .update(cx, |app, _, cx| {
            let mut split = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Split {
                    pane: Some(a.as_u64()),
                    tab: None,
                    direction: Some(tako_control::protocol::Direction::Down),
                    ratio: None,
                    command: None,
                    cwd: None,
                    focus: Some(true),
                },
                PaneOrigin::Cli,
            );
            // IPC と同じ後処理（dispatch が積んだセッション起動はここで消化される）
            let _ = app.after_dispatch(&mut split, false, cx);
            cx.notify();
            let split = split
                .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 分割できない: {e}")));
            split["pane"].as_u64().map(PaneId::from_raw)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 分割の応答に pane が無い")));
    check(
        wait_for_pane_ready(window, cx, b, Duration::from_secs(15)).await,
        &format!("visual-test {SECTION}: 前提 = B のプロンプトが出る"),
    );
    for _ in 0..6 {
        notify_and_draw(any, window, cx);
        wait(cx, 50).await;
    }
    let focused_b = window
        .update(cx, |app, _, _| {
            app.workspace.active_tab().tree().focused() == b
        })
        .unwrap_or(false);
    check(
        focused_b,
        &format!("visual-test {SECTION}: 前提 = フォーカスは B（呼び出し元 A とは別）"),
    );
    let (before, scale) = capture_frame(any, cx).unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: 出す前のフレームを撮れない"
        ))
    });

    // MCP のエンジンを呼び出し元 A の文脈で回す（stdio の TAKO_PANE_ID / HTTP の X-Tako-Pane と同じ口）
    let response = window
        .update(cx, |app, _, cx| {
            // 実行係は IPC の受け口と同じ形（dispatch → 後処理）
            let mut exec = |request: tako_control::protocol::Request| {
                let mut result = tako_control::dispatch(app, request, PaneOrigin::Mcp);
                let _ = app.after_dispatch(&mut result, false, cx);
                result.map_err(|e| e.to_string())
            };
            let mut session = tako_control::mcp::McpSession {
                caller_pane: Some(a.as_u64()),
                caller_role: None,
                connected: true,
                exec: &mut exec,
            };
            let response = tako_control::mcp::handle_message(
                &serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {
                        "name": "tako_show_command",
                        "arguments": { "commands": ["echo visual-1958"], "label": "visual-1958" },
                    },
                }),
                &mut session,
            );
            cx.notify();
            response
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: MCP の応答が無い")));
    let result = &response["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or("");
    let card_pane = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v["card"]["pane"].as_u64());

    // 帯が A に割り当たり、実描画が採取されるまで描く（状態で待つ = #796）
    let mut bands = (0.0f32, false);
    for _ in 0..60 {
        notify_and_draw(any, window, cx);
        wait(cx, 50).await;
        bands = window
            .update(cx, |app, _, _| {
                (
                    app.card_bands.get(&a).map(|f| f.height).unwrap_or(0.0),
                    app.card_bands.contains_key(&b),
                )
            })
            .unwrap_or((0.0, false));
        if bands.0 > 0.0 {
            break;
        }
    }
    for _ in 0..4 {
        notify_and_draw(any, window, cx);
        wait(cx, 50).await;
    }
    let (after, _) = capture_frame(any, cx).unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: 出した後のフレームを撮れない"
        ))
    });
    // 画像は判定の前に落とす（修正前のビルドでも絵が残るように）
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dump = std::path::Path::new(&dump);
        let _ = std::fs::create_dir_all(dump);
        let path = dump.join("show-command-caller.png");
        if after.save(&path).is_ok() {
            println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
        }
    }

    // A の帯 = テキスト領域の直下・帯の高さ。B は同じ高さの下端（カードがあれば帯が来る場所）
    let (area_a, area_b) = window
        .update(cx, |app, _, _| (text_area(app, a), text_area(app, b)))
        .unwrap_or((None, None));
    let (Some(area_a), Some(area_b)) = (area_a, area_b) else {
        fail(&format!(
            "visual-test {SECTION}: A / B のテキスト領域が無い"
        ))
    };
    let band_h = px(bands.0);
    let band_a = Bounds::new(
        point(area_a.origin.x, area_a.origin.y + area_a.size.height),
        size(area_a.size.width, band_h),
    );
    let tail_b = Bounds::new(
        point(
            area_b.origin.x,
            area_b.origin.y + area_b.size.height - band_h,
        ),
        size(area_b.size.width, band_h),
    );
    let (changed_a, total_a) = diff_in(&before, &after, band_a, scale);
    let (changed_b, total_b) = diff_in(&before, &after, tail_b, scale);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} caller={} focused={} card_pane={card_pane:?} \
         is_error={:?} band_a={:.0} band_b={} changed_a={changed_a}/{total_a} \
         changed_b={changed_b}/{total_b}",
        a.as_u64(),
        b.as_u64(),
        result["isError"],
        bands.0,
        bands.1,
    );

    check(
        result["isError"] == serde_json::json!(false) && card_pane == Some(a.as_u64()),
        &format!(
            "visual-test {SECTION} ①: pane を省いた MCP の show が呼び出し元 A へ出る \
             (#1958。応答: {})",
            text.chars().take(120).collect::<String>()
        ),
    );
    check(
        bands.0 > 0.0 && !bands.1,
        &format!(
            "visual-test {SECTION} ②: カード帯は A にだけ割り当たる (band_a={:.0} band_b={})",
            bands.0, bands.1
        ),
    );
    // 帯の 1 割以上の画素が変わる（カードの面と文字）。B の下端は 1 画素も変わらない
    check(
        total_a > 0 && changed_a * 10 >= total_a && changed_b == 0,
        &format!(
            "visual-test {SECTION} ③: A の帯にカードが描かれ B は変わらない \
             (A {changed_a}/{total_a} / B {changed_b}/{total_b})"
        ),
    );

    // 後片付け（以降の節へ持ち越さない）
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::ShowCommand {
                action: Some("dismiss".into()),
                commands: Vec::new(),
                label: None,
                pane: Some(a.as_u64()),
                card: None,
                index: None,
                focus: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
}
