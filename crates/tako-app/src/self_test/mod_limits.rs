//! visual-test `mod-limits`（FR-2.42.18 / Issue #1903）: 画面下のステータスバーの 5h / 7d が
//! tako mod の報告の値で**実ピクセルに**描かれるか。
//!
//! 場面: フォーカスペイン（素のシェル = 画面に 5h / 7d は無い）に mod の報告（5h 42.4% /
//! 7d 17.5%）を入れて `refresh_agent_metrics` を通す。①ステータスバーの値が mod（42 / 18・
//! 取得元 mod）②報告を古くする（受信時刻を失効の 1 秒先へ）と mod を使わず画面の値（無し）へ戻る
//! ③①と②でステータスバーの帯の絵が変わる（メーターが描かれていた）。帯の絵は
//! `TAKO_VISUAL_DUMP_DIR` へ書き出す（目で見る用。`mod-limits-fresh.png` / `mod-limits-stale.png`）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1903_LEGACY=1` は mod を見ないので ① が FAILED
//! = 同一バイナリでの A/B。単独実行は `TAKO_VISUAL_ONLY=mod-limits`

use super::*;
use std::time::Instant;

const SECTION: &str = "mod-limits";

/// ステータスバーの帯（ウィンドウ下端の `STATUS_BAR_HEIGHT`）を切り出す
fn status_strip(frame: &image::RgbaImage, scale: f32) -> image::RgbaImage {
    let h = ((STATUS_BAR_HEIGHT * scale) as u32).clamp(1, frame.height());
    image::imageops::crop_imm(frame, 0, frame.height() - h, frame.width(), h).to_image()
}

pub(super) async fn mod_limits_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let report = tako_core::claude_mod::parse_report(serde_json::json!({
        "schema": 1, "mod_version": "vt1903", "at": 1, "turn": "idle",
        "config_dir": "/vt-1903",
        "rate_limits": [
            {"kind": "five_hour", "percent_used": 42.4, "resets_at": "2030-01-01T00:00:00.000Z", "observed_at": 1},
            {"kind": "seven_day", "percent_used": 17.5, "resets_at": "2030-01-05T00:00:00.000Z", "observed_at": 1},
        ],
    }))
    .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 報告を組めない: {e}")));

    // 報告を入れてステータスバーの値を組み直す（2 秒 tick と同じ `refresh_agent_metrics`）
    let set = |cx: &mut AsyncApp, received: Instant| {
        window
            .update(cx, |app: &mut TakoApp, _, cx| {
                let root = app.workspace.active_tab().tree().focused();
                app.limit_service = tako_core::LimitService::Claude;
                app.claude_mod
                    .accept(root.as_u64(), report.clone(), received);
                app.refresh_agent_metrics();
                cx.notify();
                (
                    app.agent_metrics.limit_5h,
                    app.agent_metrics.limit_week,
                    app.claude_limit_source.map(|s| s.as_str()),
                )
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 場面づくり")))
    };
    let strip = |cx: &mut AsyncApp| {
        let (frame, scale) = capture_frame(any, cx)
            .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: フレーム")));
        status_strip(&frame, scale)
    };

    // ① 新鮮な報告 = mod の値
    let fresh = set(cx, Instant::now());
    notify_and_draw(any, window, cx);
    wait(cx, 300).await;
    let fresh_img = strip(cx);
    // ② 報告が古い = mod を使わない（このペインの画面に 5h / 7d は無い）
    let stale_at = Instant::now()
        .checked_sub(tako_core::claude_mod::FRESH_FOR + Duration::from_secs(1))
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 古い時刻を作れない")));
    let stale = set(cx, stale_at);
    notify_and_draw(any, window, cx);
    wait(cx, 300).await;
    let stale_img = strip(cx);

    let diff = if fresh_img.dimensions() == stale_img.dimensions() {
        fresh_img
            .pixels()
            .zip(stale_img.pixels())
            .filter(|(a, b)| a != b)
            .count()
    } else {
        usize::MAX
    };
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dir = std::path::Path::new(&dump);
        let _ = fresh_img.save(dir.join("mod-limits-fresh.png"));
        let _ = stale_img.save(dir.join("mod-limits-stale.png"));
    }
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} fresh={fresh:?} stale={stale:?} strip={}x{} diff_px={diff}",
        fresh_img.width(),
        fresh_img.height()
    );

    if fresh != (Some(42), Some(18), Some("mod")) {
        fail(&format!(
            "visual-test {SECTION} ①: 報告が新鮮ならステータスバーは mod の値（期待 (42, 18, mod)・実際 {fresh:?}）"
        ));
    }
    if stale.2 == Some("mod") || stale.0 == Some(42) {
        fail(&format!(
            "visual-test {SECTION} ②: 報告が古いのに mod の値のまま（{stale:?}）"
        ));
    }
    if diff == 0 {
        fail(&format!(
            "visual-test {SECTION} ③: ①と②でステータスバーの絵が変わらない（メーターが描かれていない）"
        ));
    }
}
