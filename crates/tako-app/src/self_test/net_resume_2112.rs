//! visual-test `net-resume`（FR-2.27.17 / Issue #2112）: 設定画面「一般」の「ネット復帰で再開」の
//! トグルを実マウスで押すと、GUI の実行状態・settings.json・CLI / MCP と同じ dispatch の答え
//! （`tako limit-resume --network` の `network.enabled`）の 3 つが同じ値で切り替わるか。
//!
//! ① 既定 ON で描かれる（項目の無い settings.json でも ON）
//! ② トグルを押す → OFF。実行状態・settings.json・dispatch の答えが揃う
//! ③ もう一度押す → ON に戻る
//! ④ CLI / MCP の経路（dispatch の `network`）で OFF にしてから設定画面を開き直すと OFF で描かれる
//!
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ `net-resume-settings-on.png` /
//! `net-resume-settings-off.png` / `net-resume-settings-reopened-off.png` で落とす。
//! 単独実行は `TAKO_VISUAL_ONLY=net-resume`（`scripts/test-net-resume-2112.sh` が呼ぶ）

use super::*;

const SECTION: &str = "net-resume";

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn dump(image: &image::RgbaImage, name: &str) {
    if let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(name);
        if image.save(&path).is_ok() {
            println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
        }
    }
}

fn network_req(network: Option<bool>) -> tako_control::protocol::Request {
    tako_control::protocol::Request::LimitResume {
        pane: None,
        enabled: None,
        all: network.is_none().then_some(true),
        network,
    }
}

/// CLI / MCP と同じ dispatch の答え（`network.enabled`）
fn dispatched(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, network: Option<bool>) -> bool {
    window
        .update(cx, |app, _, cx| {
            let mut result = tako_control::dispatch(app, network_req(network), PaneOrigin::Cli);
            let _ = app.after_dispatch(&mut result, false, cx);
            result
        })
        .ok()
        .and_then(Result::ok)
        .and_then(|v| v["network"]["enabled"].as_bool())
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: dispatch が通らない")))
}

fn runtime(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> bool {
    window
        .update(cx, |app, _, _| app.net_resume.enabled)
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 実行状態を読めない")))
}

/// 永続化はセルフテスト（TAKO_SELF_TEST）では止まるので、その時だけ見ない
fn persisted(want: bool) -> bool {
    std::env::var_os("TAKO_SELF_TEST").is_some()
        || tako_control::settings::load().network_resume == want
}

/// トグルの矩形の平均色（ON と OFF で描き分けられているかを画素で見る）
fn mean_rgb(frame: &image::RgbaImage, scale: f32, r: crate::form_layout::Rect) -> [f32; 3] {
    let (x0, y0) = ((r.x0 * scale) as u32, (r.y0 * scale) as u32);
    let (x1, y1) = (
        ((r.x1 * scale) as u32).min(frame.width()),
        ((r.y1 * scale) as u32).min(frame.height()),
    );
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = frame.get_pixel(x, y);
            for (i, s) in sum.iter_mut().enumerate() {
                *s += f64::from(p[i]);
            }
            n += 1.0;
        }
    }
    let n = n.max(1.0);
    [
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    ]
}

fn color_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum()
}

async fn open_settings(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) -> WindowHandle<crate::settings_window::SettingsWindow> {
    use crate::settings_window::SettingsTab;
    let _ = window.update(cx, |app, _, cx| {
        app.open_settings_window_impl(Some(SettingsTab::General), cx);
    });
    for _ in 0..40 {
        if let Some(handle) = window
            .update(cx, |app, _, _| app.settings_window_handle)
            .ok()
            .flatten()
        {
            let _ = handle.update(cx, |view, window, cx| {
                view.set_tab(SettingsTab::General, cx);
                window.resize(size(px(760.), px(600.)));
                cx.notify();
            });
            sleep(cx, 250).await;
            let _ = capture_frame(handle.into(), cx);
            return handle;
        }
        sleep(cx, 100).await;
    }
    fail(&format!("visual-test {SECTION}: 設定ウィンドウが開かない"));
}

/// トグルを画面に入れて（枠の外ならスクロールして）その実矩形を返す
async fn toggle_rect(
    settings: WindowHandle<crate::settings_window::SettingsWindow>,
    cx: &mut AsyncApp,
) -> crate::form_layout::Rect {
    use crate::settings_window::SettingsTab;
    let find = |cx: &mut AsyncApp| {
        settings
            .update(cx, |view, _, _| {
                let handle = view.scroll_handle(SettingsTab::General);
                (
                    view.vt_probe_bounds()
                        .iter()
                        .find(|(key, _)| key == "ctl:toggle-network_resume")
                        .map(|(_, r)| *r),
                    crate::settings_window::to_rect(handle.bounds()),
                    f32::from(handle.offset().y),
                )
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: トグルの実測矩形")))
    };
    let (target, view, offset) = find(cx);
    let Some(target) = target else {
        fail(&format!(
            "visual-test {SECTION}: 「ネット復帰で再開」のトグルが描かれていない"
        ));
    };
    if target.y0 >= view.y0 && target.y1 <= view.y1 {
        return target;
    }
    let want = offset - (target.y0 - view.y0 - 60.0);
    let _ = settings.update(cx, |view, _, cx| {
        view.scroll_handle(SettingsTab::General)
            .set_offset(point(px(0.), px(want)));
        cx.notify();
    });
    sleep(cx, 250).await;
    let _ = capture_frame(settings.into(), cx);
    find(cx).0.unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: スクロール後にトグルが消えた"
        ))
    })
}

pub(super) async fn net_resume_visual(
    _any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    // ① 既定 ON
    check(
        runtime(window, cx) && dispatched(window, cx, None),
        &format!("visual-test {SECTION} ①: 既定 ON（実行状態と dispatch の答え）"),
    );
    let settings = open_settings(window, cx).await;
    let settings_any: AnyWindowHandle = settings.into();
    let rect = toggle_rect(settings, cx).await;
    let on_color = match capture_frame(settings_any, cx) {
        Some((frame, scale)) => {
            dump(&frame, "net-resume-settings-on.png");
            mean_rgb(&frame, scale, rect)
        }
        None => fail(&format!("visual-test {SECTION} ①: 設定画面を撮れない")),
    };
    let press = |cx: &mut AsyncApp| {
        click_at(
            settings_any,
            cx,
            point(px((rect.x0 + rect.x1) / 2.), px((rect.y0 + rect.y1) / 2.)),
        );
    };

    // ② 押す → OFF
    press(cx);
    sleep(cx, 300).await;
    let off_color = match capture_frame(settings_any, cx) {
        Some((frame, scale)) => {
            dump(&frame, "net-resume-settings-off.png");
            mean_rgb(&frame, scale, rect)
        }
        None => fail(&format!("visual-test {SECTION} ②: 設定画面を撮れない")),
    };
    let drawn = color_distance(on_color, off_color);
    let (rt, file, wire) = (
        runtime(window, cx),
        persisted(false),
        dispatched(window, cx, None),
    );
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} off runtime={rt} persisted={file} dispatch={wire} \
         toggle_on={on_color:?} toggle_off={off_color:?} distance={drawn:.1}"
    );
    check(
        !rt && file && !wire && drawn > 30.0,
        &format!(
            "visual-test {SECTION} ②: トグルで OFF（実行状態={rt} settings.json={file} dispatch={wire} \
             トグルの色の差={drawn:.1}）"
        ),
    );

    // ③ もう一度押す → ON
    press(cx);
    sleep(cx, 300).await;
    let (rt, file, wire) = (
        runtime(window, cx),
        persisted(true),
        dispatched(window, cx, None),
    );
    println!("TAKO_VISUAL_PIXEL: {SECTION} on runtime={rt} persisted={file} dispatch={wire}");
    check(
        rt && file && wire,
        &format!(
            "visual-test {SECTION} ③: もう一度押すと ON（実行状態={rt} settings.json={file} dispatch={wire}）"
        ),
    );
    let _ = settings_any.update(cx, |_, w, _| w.remove_window());
    // remove_window は閉じる時の後始末（on_window_should_close）を通らないので、
    // 古い手を消してから開き直す（他の visual-test の節と同じ作法）
    let _ = window.update(cx, |app, _, _| app.settings_window_handle = None);
    sleep(cx, 300).await;

    // ④ CLI / MCP の経路で OFF → 開き直した設定画面が OFF で描かれる
    let off = dispatched(window, cx, Some(false));
    check(
        !off && !runtime(window, cx) && persisted(false),
        &format!("visual-test {SECTION} ④: dispatch の network=false で OFF"),
    );
    let settings = open_settings(window, cx).await;
    let rect = toggle_rect(settings, cx).await;
    let reopened = match capture_frame(settings.into(), cx) {
        Some((frame, scale)) => {
            dump(&frame, "net-resume-settings-reopened-off.png");
            mean_rgb(&frame, scale, rect)
        }
        None => fail(&format!("visual-test {SECTION} ④: 設定画面を撮れない")),
    };
    // OFF の色に近く、ON の色から遠い = OFF で描かれている
    let (to_off, to_on) = (
        color_distance(reopened, off_color),
        color_distance(reopened, on_color),
    );
    println!("TAKO_VISUAL_PIXEL: {SECTION} reopened to_off={to_off:.1} to_on={to_on:.1}");
    check(
        std::env::var_os("TAKO_SELF_TEST").is_some() || to_off < to_on,
        &format!(
            "visual-test {SECTION} ④: 開き直した設定画面が OFF を描く (OFF との差={to_off:.1} / ON との差={to_on:.1})"
        ),
    );
    let any: AnyWindowHandle = settings.into();
    let _ = any.update(cx, |_, w, _| w.remove_window());
    let _ = window.update(cx, |app, _, _| app.settings_window_handle = None);
    // 後の節へ影響を残さない（既定へ戻す）
    dispatched(window, cx, Some(true));
}
