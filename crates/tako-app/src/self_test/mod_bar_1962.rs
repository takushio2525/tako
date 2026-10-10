//! visual-test `mod-bar`（FR-2.42.38 / Issue #1962）: フォーカス中のペインの tako mod が Claude Code の
//! 画面に使用制限・ctx のバーを描いているとき、画面下のステータスバーから claude の 5h / 7d / ctx の
//! 区画が**実ピクセルで**消え、そうでないときは今のまま描かれるか。
//!
//! 場面: フォーカスペイン（素のシェル）に mod の報告（5h 42.4% / 7d 17.5% / ctx 33%）を入れて
//! `refresh_agent_metrics` を通す。①報告の `renders.usage_bar` = `prompt_hint`（mod が描いた）→
//! 区画を出さない ②`renders` の無い報告（S7-3 前の mod・描いていない）→ 区画を出す
//! ③①と同じ報告だが古い（受信時刻を失効の 1 秒先へ）→ 区画を出す（45 秒で戻る）
//! ④①と②・①と③でステータスバーの帯の絵が変わる（区画が描かれ / 消えている）。
//! 帯の絵は `TAKO_VISUAL_DUMP_DIR` へ書き出す（目で見る用。`mod-bar-yield.png` /
//! `mod-bar-no-renders.png` / `mod-bar-stale.png`）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1877_S7_LEGACY=1` は区画を止めないので ① が FAILED
//! = 同一バイナリでの A/B。単独実行は `TAKO_VISUAL_ONLY=mod-bar`

use super::*;
use std::time::Instant;

const SECTION: &str = "mod-bar";

/// ステータスバーの帯（ウィンドウ下端の `STATUS_BAR_HEIGHT`）を切り出す
fn status_strip(frame: &image::RgbaImage, scale: f32) -> image::RgbaImage {
    let h = ((STATUS_BAR_HEIGHT * scale) as u32).clamp(1, frame.height());
    image::imageops::crop_imm(frame, 0, frame.height() - h, frame.width(), h).to_image()
}

fn diff_px(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    if a.dimensions() != b.dimensions() {
        return usize::MAX;
    }
    a.pixels().zip(b.pixels()).filter(|(x, y)| x != y).count()
}

pub(super) async fn mod_bar_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let report = |renders: Option<serde_json::Value>| {
        let mut v = serde_json::json!({
            "schema": 1, "mod_version": "vt1962", "at": 1, "turn": "idle",
            "config_dir": "/vt-1962",
            "context": {"tokens": 330000, "window": 1000000, "percent": 33},
            "rate_limits": [
                {"kind": "five_hour", "percent_used": 42.4, "resets_at": "2030-01-01T00:00:00.000Z", "observed_at": 1},
                {"kind": "seven_day", "percent_used": 17.5, "resets_at": "2030-01-05T00:00:00.000Z", "observed_at": 1},
            ],
        });
        if let Some(r) = renders {
            v["renders"] = r;
        }
        tako_core::claude_mod::parse_report(v)
            .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 報告を組めない: {e}")))
    };
    let drawing = report(Some(serde_json::json!({
        "band": true, "usage_bar": "prompt_hint", "buttons": 1, "band_hook": true, "hint_hook": true,
    })));
    let not_drawing = report(None);

    // 報告を入れてステータスバーを組み直す（2 秒 tick と同じ `refresh_agent_metrics`）。
    // ctx のメーターは画面の値（素のシェルには無い = 0%）でも区画ごと描かれるので、絵の差で見える
    let set = |cx: &mut AsyncApp, report: &tako_core::claude_mod::ModReport, received: Instant| {
        let report = report.clone();
        window
            .update(cx, |app: &mut TakoApp, _, cx| {
                let root = app.workspace.active_tab().tree().focused();
                app.limit_service = tako_core::LimitService::Claude;
                app.claude_mod.accept(root.as_u64(), report, received);
                app.refresh_agent_metrics();
                cx.notify();
                (
                    app.claude_bar_owner.as_str(),
                    app.claude_bar_owner.yields(),
                    app.agent_metrics.limit_5h,
                )
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 場面づくり")))
    };
    let strip = |cx: &mut AsyncApp| {
        let (frame, scale) = capture_frame(any, cx)
            .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: フレーム")));
        status_strip(&frame, scale)
    };

    // ① mod がバーを描いている = 区画を出さない
    let yielded = set(cx, &drawing, Instant::now());
    notify_and_draw(any, window, cx);
    wait(cx, 300).await;
    let yield_img = strip(cx);
    // ② 描いていない（renders なし）= 区画を出す
    let shown = set(cx, &not_drawing, Instant::now());
    notify_and_draw(any, window, cx);
    wait(cx, 300).await;
    let shown_img = strip(cx);
    // ③ 描いているが報告が古い = 区画を出す
    let stale_at = Instant::now()
        .checked_sub(tako_core::claude_mod::FRESH_FOR + Duration::from_secs(1))
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 古い時刻を作れない")));
    let stale = set(cx, &drawing, stale_at);
    notify_and_draw(any, window, cx);
    wait(cx, 300).await;
    let stale_img = strip(cx);

    let (diff_shown, diff_stale) = (
        diff_px(&yield_img, &shown_img),
        diff_px(&yield_img, &stale_img),
    );
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dir = std::path::Path::new(&dump);
        let _ = yield_img.save(dir.join("mod-bar-yield.png"));
        let _ = shown_img.save(dir.join("mod-bar-no-renders.png"));
        let _ = stale_img.save(dir.join("mod-bar-stale.png"));
    }
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} yield={yielded:?} no_renders={shown:?} stale={stale:?} \
         strip={}x{} diff_no_renders_px={diff_shown} diff_stale_px={diff_stale}",
        yield_img.width(),
        yield_img.height()
    );

    if yielded != ("mod", true, Some(42)) {
        fail(&format!(
            "visual-test {SECTION} ①: mod がバーを描いているのにステータスバーの claude の区画を出す\
             （期待 (mod, true, 42) = 値は取ったまま区画だけ外す・実際 {yielded:?}）"
        ));
    }
    if shown.1 || stale.1 {
        fail(&format!(
            "visual-test {SECTION} ②③: 描いていない / 古い報告で区画を外した（no_renders={shown:?} stale={stale:?}）"
        ));
    }
    if diff_shown == 0 || diff_stale == 0 {
        fail(&format!(
            "visual-test {SECTION} ④: 区画を外したのにステータスバーの絵が変わらない（no_renders {diff_shown} px / stale {diff_stale} px）"
        ));
    }
}
