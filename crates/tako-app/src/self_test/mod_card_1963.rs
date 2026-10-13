//! visual-test `mod-card`（FR-2.42.41 / Issue #1963）: tako mod がコマンドカードを Claude Code の画面に
//! 描いている（報告の `renders.cards` に載った）カードは、GPUI のカード帯へ**実ピクセルで**出ず、ペインの
//! 行数（claude の viewport）も縮まない。描いていない・報告が古い・claude が終わったときは今のまま帯が出る。
//!
//! 場面: 素のシェルのペインへカードを 1 枚出し（CLI / MCP と同じ dispatch）、mod の報告を入れて描き直す。
//! ① 報告の `renders.cards` にカードの id（2 回目の報告 = mod は応答でカードを受け取って描いた）→ 帯なし・行数そのまま
//! ② `renders.cards` の無い報告（S7-4 前の mod）→ 帯あり・行数が縮む
//! ③ ①と同じ報告だが古い（受信時刻を失効の 1 秒先へ）→ 帯あり（45 秒で GPUI の帯へ戻る）
//! ④ 新しいカードを mod がまだ拾っていない（カードの後の報告が 1 つだけ）→ 帯なし（猶予の間は viewport を縮めない）
//! ⑤ claude が終わった（最後の報告 `ended`）→ 帯あり。2 秒 tick の `refresh_mod_card_owners` が変化を知らせる
//! 帯の絵は `TAKO_VISUAL_DUMP_DIR` へ書き出す（目で見る用。`mod-card-drawn.png` / `mod-card-old-mod.png` /
//! `mod-card-stale.png` / `mod-card-ended.png`）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1963_LEGACY=1` は帯から外さないので ① が FAILED = 同一バイナリ
//! での A/B。単独実行は `TAKO_VISUAL_ONLY=mod-card`

use super::*;
use std::time::Instant;

const SECTION: &str = "mod-card";

pub(super) async fn mod_card_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let pane = ensure_fresh_scene(window, cx, SECTION).await;
    let report = |cards: Option<Vec<u64>>, ended: bool| {
        let mut v = serde_json::json!({
            "schema": 1, "mod_version": "vt1963", "at": 1, "turn": "idle",
            "context": {"tokens": 60000, "window": 1000000, "percent": 6},
            "rate_limits": [], "ended": ended,
        });
        if let Some(cards) = cards {
            v["renders"] = serde_json::json!({
                "band": true, "buttons": 1, "band_hook": true, "hint_hook": true, "cards": cards,
            });
        }
        tako_core::claude_mod::parse_report(v)
            .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 報告を組めない: {e}")))
    };
    // カードを 1 枚出す（CLI / MCP と同じ dispatch 経路）。戻り値はカードの id
    let show = |cx: &mut AsyncApp, command: &str| -> u64 {
        window
            .update(cx, |app: &mut TakoApp, _, cx| {
                let v = tako_control::dispatch(
                    app,
                    tako_control::protocol::Request::ShowCommand {
                        action: None,
                        commands: vec![command.to_string()],
                        label: Some("visual-test #1963".into()),
                        pane: Some(pane.as_u64()),
                        card: None,
                        index: None,
                        focus: None,
                    },
                    PaneOrigin::Mcp,
                )
                .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: カードを出せない: {e}")));
                cx.notify();
                v["card"]["id"].as_u64().unwrap_or(0)
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: カード")))
    };
    // 報告を順に入れる（受信時刻つき）。最後に 2 秒 tick と同じ判定の更新を通す
    let feed = |cx: &mut AsyncApp, reports: Vec<(tako_core::claude_mod::ModReport, Instant)>| {
        window
            .update(cx, |app: &mut TakoApp, _, cx| {
                app.claude_mod.forget_pane(pane.as_u64());
                for (r, at) in reports {
                    app.claude_mod.accept(pane.as_u64(), r, at);
                }
                let changed = app.refresh_mod_card_owners();
                cx.notify();
                changed
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 報告")))
    };
    // 帯の高さ・ペインの行数・カード帯から外したカード
    let geom = |cx: &mut AsyncApp| {
        window
            .update(cx, |app: &mut TakoApp, _, _| {
                let area = app
                    .pane_text_areas
                    .iter()
                    .find(|(id, _)| *id == pane)
                    .map(|(_, b)| *b);
                let rows = app.terminals.get(&pane).map(|s| s.size().1).unwrap_or(0);
                let band = app.card_bands.get(&pane).map(|f| f.height).unwrap_or(0.0);
                let by_mod =
                    tako_control::claude_mod::cards_drawn_by_mod(app, pane, Instant::now());
                (area, rows, band, by_mod, app.theme.crust)
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 寸法")))
    };
    let draw = |cx: &mut AsyncApp| {
        for _ in 0..4 {
            notify_and_draw(any, window, cx);
        }
    };
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));

    // 基準（カードなし）
    draw(cx);
    wait(cx, 300).await;
    let (base_area, base_rows, base_band, _, card_fill) = geom(cx);
    let base_area =
        base_area.unwrap_or_else(|| fail(&format!("visual-test {SECTION}: ペイン領域")));

    // カード帯が出うる矩形（カードの無いときのテキスト領域の下 4 割 + パディング）。帯の高さの上限は領域の
    // 34%（下駄つきでも 50%）なので、帯が出ればこの中に収まる
    let height = f32::from(base_area.size.height);
    let band_bounds = Bounds::new(
        point(base_area.left(), base_area.bottom() - px(height * 0.4)),
        size(base_area.size.width, px(height * 0.4 + PANE_PADDING)),
    );
    let shot = |cx: &mut AsyncApp| {
        capture_frame(any, cx).unwrap_or_else(|| fail(&format!("visual-test {SECTION}: フレーム")))
    };

    let id = show(cx, "cargo build -p tako-cli && echo visual-test-1963");
    // ① mod が描いた（カードの後に 2 回報告 = 応答でカードを受け取り、次の報告で renders.cards に載せた）
    let t = Instant::now();
    let changed_drawn = feed(
        cx,
        vec![
            (report(Some(vec![]), false), t),
            (report(Some(vec![id]), false), t + Duration::from_millis(1)),
        ],
    );
    draw(cx);
    wait(cx, 300).await;
    let drawn = geom(cx);
    let (drawn_img, scale) = shot(cx);

    // ② S7-4 前の mod（renders.cards なし）= 帯あり
    let changed_old = feed(
        cx,
        vec![
            (report(None, false), t + Duration::from_millis(2)),
            (report(None, false), t + Duration::from_millis(3)),
        ],
    );
    draw(cx);
    wait(cx, 300).await;
    let old = geom(cx);
    let (old_img, _) = shot(cx);

    // ③ 描いているが報告が古い = 帯あり
    let stale_at = Instant::now()
        .checked_sub(tako_core::claude_mod::FRESH_FOR + Duration::from_secs(1))
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 古い時刻を作れない")));
    feed(
        cx,
        vec![
            (report(Some(vec![]), false), stale_at),
            (
                report(Some(vec![id]), false),
                stale_at + Duration::from_millis(1),
            ),
        ],
    );
    draw(cx);
    wait(cx, 300).await;
    let stale = geom(cx);
    let (stale_img, _) = shot(cx);

    // ④ 新しいカードを mod がまだ拾っていない（カードの後の報告は 1 つだけ。1 つ前はカードより前）
    window
        .update(cx, |app: &mut TakoApp, _, _| {
            app.command_cards.dismiss(Some(pane), None)
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: カードを閉じる")));
    let before_card = Instant::now();
    wait(cx, 20).await;
    let fresh_id = show(cx, "npm test");
    feed(
        cx,
        vec![
            (report(Some(vec![]), false), before_card),
            (report(Some(vec![]), false), Instant::now()),
        ],
    );
    draw(cx);
    wait(cx, 300).await;
    let pending = geom(cx);

    // ⑤ claude が終わった（最後の報告）= 帯あり・tick が変化を知らせる
    feed(
        cx,
        vec![
            (report(Some(vec![]), false), before_card),
            (report(Some(vec![fresh_id]), false), Instant::now()),
        ],
    );
    let changed_ended = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            app.claude_mod
                .accept(pane.as_u64(), report(None, true), Instant::now());
            cx.notify();
            app.refresh_mod_card_owners()
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 最後の報告")));
    draw(cx);
    wait(cx, 300).await;
    let ended = geom(cx);
    let (ended_img, _) = shot(cx);

    // 地色の数え方の上下の向きは、帯が必ず出ている②の絵で決める（#703 と同じ。帯の無い絵で多い方を
    // 採ると、窓の上端のタブバー等の同じ色を数えて偽陽性になる）
    let flip = color_pixels_in_bounds(&old_img, band_bounds, scale, card_fill, 6, true)
        > color_pixels_in_bounds(&old_img, band_bounds, scale, card_fill, 6, false);
    let card_px = |frame: &image::RgbaImage| {
        color_pixels_in_bounds(frame, band_bounds, scale, card_fill, 6, flip)
    };
    let (drawn_px, old_px, stale_px, ended_px) = (
        card_px(&drawn_img),
        card_px(&old_img),
        card_px(&stale_img),
        card_px(&ended_img),
    );

    // 後片付け（次の節へカードを残さない）
    window
        .update(cx, |app: &mut TakoApp, _, cx| {
            app.command_cards.dismiss(Some(pane), None);
            app.claude_mod.forget_pane(pane.as_u64());
            cx.notify();
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 後片付け")));

    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dir = std::path::Path::new(&dump);
        let _ = std::fs::create_dir_all(dir);
        let _ = drawn_img.save(dir.join("mod-card-drawn.png"));
        let _ = old_img.save(dir.join("mod-card-old-mod.png"));
        let _ = stale_img.save(dir.join("mod-card-stale.png"));
        let _ = ended_img.save(dir.join("mod-card-ended.png"));
    }
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} base=(rows={base_rows} band={base_band:.1}) \
         drawn=(rows={} band={:.1} by_mod={:?} card_px={drawn_px} changed={changed_drawn}) \
         old_mod=(rows={} band={:.1} by_mod={:?} card_px={old_px} changed={changed_old}) \
         stale=(rows={} band={:.1} by_mod={:?} card_px={stale_px}) \
         pending=(rows={} band={:.1} by_mod={:?}) \
         ended=(rows={} band={:.1} by_mod={:?} card_px={ended_px} changed={changed_ended})",
        drawn.1,
        drawn.2,
        drawn.3,
        old.1,
        old.2,
        old.3,
        stale.1,
        stale.2,
        stale.3,
        pending.1,
        pending.2,
        pending.3,
        ended.1,
        ended.2,
        ended.3,
    );

    if !(drawn.2 == 0.0 && drawn.1 == base_rows && drawn.3.contains(&id) && drawn_px < old_px) {
        fail(&format!(
            "visual-test {SECTION} ①: mod が描いたカードなのに GPUI のカード帯が出ている / 行数が縮んだ\
             （期待 band=0・rows={base_rows}・by_mod に {id}・帯の地色が②より少ない。実際 band={:.1} rows={} \
             by_mod={:?} card_px={drawn_px} / ② {old_px}）",
            drawn.2, drawn.1, drawn.3
        ));
    }
    if !(old.2 > 0.0 && old.1 < base_rows && old.3.is_empty() && old_px > 2000) {
        fail(&format!(
            "visual-test {SECTION} ②: カードを描かない mod なのに GPUI のカード帯が出ない\
             （band={:.1} rows={} by_mod={:?} card_px={old_px}）",
            old.2, old.1, old.3
        ));
    }
    if !(stale.2 > 0.0 && stale.3.is_empty() && stale_px > 2000) {
        fail(&format!(
            "visual-test {SECTION} ③: 古い報告で GPUI のカード帯を外した（band={:.1} by_mod={:?} card_px={stale_px}）",
            stale.2, stale.3
        ));
    }
    if !(pending.2 == 0.0 && pending.1 == base_rows && pending.3.contains(&fresh_id)) {
        fail(&format!(
            "visual-test {SECTION} ④: mod がまだ拾っていない新しいカードで GPUI のカード帯を出した（viewport が一瞬縮む）\
             （band={:.1} rows={} by_mod={:?}）",
            pending.2, pending.1, pending.3
        ));
    }
    if !(ended.2 > 0.0 && ended.3.is_empty() && ended_px > 2000 && changed_ended) {
        fail(&format!(
            "visual-test {SECTION} ⑤: claude が終わったのに GPUI のカード帯へ戻らない / tick が変化を知らせない\
             （band={:.1} by_mod={:?} card_px={ended_px} changed={changed_ended}）",
            ended.2, ended.3
        ));
    }
}
