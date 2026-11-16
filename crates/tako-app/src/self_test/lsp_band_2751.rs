//! visual-test `lsp-band-2751`（Issue #2751）: 言語サーバの「未導入」の帯が、狭いペイン（幅 240px 前後）でも
//! 本文の行を全部押し出さないか（帯の文の器の高さの上限 + 器の中のスクロール）を実フレームの寸法で確かめる。
//!
//! 場面: .py（200 行）を編集モードで開き、受け持つ言語サーバの差し替え口（`TAKO_LSP_BIN_<ID>`）を一時 dir の
//! 深い配下の**実行できないファイル**へ向ける = 状態は未導入で、帯の理由に長いパスが入る（#2716 の全節実行で
//! 見つかった形と同じ）。ペインは右側に開き、幅を 240px 前後へ縮める。
//!
//! ① 狭いペイン（新）: 帯・帯の文の器・本文の行を測る（`TAKO_VISUAL_PIXEL: lsp-band-2751 ①new …`）。
//!    本文の行の高さの合計がペインの高さの半分以上・あふれた帯の文の器が上限（`bar_text_max_h`）ちょうど
//!    （描画がペイン全体の高さで上限を決めている）・はみ出した分は
//!    器の中でスクロールできる（つまみが出る）・帯の外側の高さが上限の計算（`BAR_CHROME_PX`）と合う
//! ② 狭いペイン（旧の形をその場で倒す）: 同じ場面・同じ文で本文の行が 0（場面が #2751 の症状を再現する）
//! ③ 読める: 帯の文の器の上で実ホイールを回すと、次の一手の最後の行が器の中に入る（全文がその場で読める）
//! ④ 押せる: 「入れる」の実矩形が帯とペインと窓の中にある。配布元がループバック（`TAKO_LSP_FETCH_BASE`。
//!    スクリプトが閉じたポートを渡す = 外へ出ない）なら実マウスで押し、帯が「取得できませんでした」へ変わる
//!    （渡されていない全節実行では押さない = 本物の配布元から取らない）
//! ⑤ 広いペイン: 現実の長さのパスへ差し替え、新と旧の形の画素の差が 0（広いペインの見た目は変わらない）
//!
//! **A/B**: `TAKO_2751_LEGACY=1`（帯の文の器に上限なし）は ① の「本文の行がペインの高さの半分以上」で FAILED。
//! 単独実行は `TAKO_VISUAL_ONLY=lsp-band-2751`（`scripts/test-lsp-band-2751.sh`）。`TAKO_LSP_AUTOSTART=1` が前提

use super::*;

const SECTION: &str = "lsp-band-2751";

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

/// 1 フレームぶんの寸法
#[derive(Debug, Default, Clone)]
struct Shot {
    /// ペインの矩形（`pane_text_areas` から枠・余白・タイトルバーを足し戻す）
    pane: Option<Bounds<Pixels>>,
    /// 本文の list の表示範囲（list が 1 度も組まれていなければ `None`）
    list: Option<Bounds<Pixels>>,
    band: Option<Bounds<Pixels>>,
    text: Option<Bounds<Pixels>>,
    last: Option<Bounds<Pixels>>,
    thumb: Option<Bounds<Pixels>>,
    button: Option<Bounds<Pixels>>,
    /// 帯の下・ペインの中に描かれた本文の行の数と、高さの合計（ペインの下端で切った分は数えない）
    lines: usize,
    code_h: f32,
    /// 帯の文の器のはみ出し（スクロールできる高さ）と今のスクロール量
    scroll_max: f32,
    offset: f32,
}

fn f(v: Pixels) -> f32 {
    f32::from(v)
}

fn fmt_b(b: Option<Bounds<Pixels>>) -> String {
    b.map_or("none".into(), |b| {
        format!(
            "{:.1},{:.1},{:.1}x{:.1}",
            f(b.left()),
            f(b.top()),
            f(b.size.width),
            f(b.size.height)
        )
    })
}

/// 採取の器を空にして描き直し（つまみの有無が前のフレームの寸法で決まるので 2 回 + 少し待つ）、寸法を読む
async fn measure(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Shot {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds.borrow_mut().clear()
        })
        .ok();
    for _ in 0..3 {
        notify_and_draw(any, window, cx);
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
    let key = pane.as_u64();
    window
        .update(cx, |app, _, _| {
            let probes = app.panel_click_probe_bounds.borrow();
            let probe = |name: &str| probes.get(&format!("preview-lsp-{name}-{key}")).copied();
            let pane_rect =
                app.pane_text_areas
                    .iter()
                    .find(|(id, _)| *id == pane)
                    .map(|(_, area)| {
                        let inset = PANE_BORDER + PANE_PADDING;
                        let top = inset + PANE_TITLE_BAR;
                        Bounds::new(
                            point(area.left() - px(inset), area.top() - px(top)),
                            gpui::size(
                                area.size.width + px(inset * 2.0),
                                area.size.height + px(inset + top),
                            ),
                        )
                    });
            let band = probe("band");
            let (mut lines, mut code_h) = (0usize, 0.0f32);
            // 本文の list の表示範囲（list の余白を含む矩形 = 行が実際に見える範囲）。帯の下・ペインの中で切る
            let list = app
                .preview_body_lists
                .get(&pane)
                .map(|(state, _, _)| state.viewport_bounds())
                .filter(|b| f(b.size.height) > 0.0);
            if let Some(pane_rect) = pane_rect {
                let mut from = band.map_or(f(pane_rect.top()) + PANE_TITLE_BAR, |b| f(b.bottom()));
                let mut to = f(pane_rect.bottom());
                if let Some(list) = list {
                    from = from.max(f(list.top()));
                    to = to.min(f(list.bottom()));
                }
                for layout in app
                    .preview_text_layouts
                    .get(&pane)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let b = layout.bounds();
                    let seen = f(b.bottom()).min(to) - f(b.top()).max(from);
                    if seen > 0.5 {
                        lines += 1;
                        code_h += seen;
                    }
                }
            }
            let scroll = app.lsp_bar_scrolls.borrow();
            let handle = scroll.get(&pane).map(|s| s.handle_for_test());
            Shot {
                pane: pane_rect,
                list,
                band,
                text: probe("text"),
                last: probe("last"),
                thumb: probe("thumb"),
                button: probe("action"),
                lines,
                code_h,
                scroll_max: handle.as_ref().map_or(0.0, |h| f(h.max_offset().y)),
                offset: handle.as_ref().map_or(0.0, |h| f(h.offset().y)),
            }
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 寸法を読めない")))
}

fn print_shot(tag: &str, shot: &Shot) {
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} {tag} pane={} pane_h={:.1} list={} band={} band_h={:.1} text={} text_h={:.1} \
         cap={:.1} lines={} code_h={:.1} half={:.1} scroll_max={:.1} offset={:.1} last={} thumb={} button={}",
        fmt_b(shot.pane),
        shot.pane.map_or(0.0, |p| f(p.size.height)),
        fmt_b(shot.list),
        fmt_b(shot.band),
        shot.band.map_or(0.0, |b| f(b.size.height)),
        fmt_b(shot.text),
        shot.text.map_or(0.0, |b| f(b.size.height)),
        shot.pane.map_or(0.0, |p| crate::lsp_status_ui::bar_text_max_h(f(p.size.height))),
        shot.lines,
        shot.code_h,
        shot.pane.map_or(0.0, |p| f(p.size.height) / 2.0),
        shot.scroll_max,
        shot.offset,
        fmt_b(shot.last),
        fmt_b(shot.thumb),
        fmt_b(shot.button),
    );
}

/// `inner` が `outer` の中にあるか（0.5px の丸めを許す）
fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    f(inner.left()) >= f(outer.left()) - 0.5
        && f(inner.top()) >= f(outer.top()) - 0.5
        && f(inner.right()) <= f(outer.right()) + 0.5
        && f(inner.bottom()) <= f(outer.bottom()) + 0.5
}

/// 帯が出るまで（未導入の状態 + 理由の文 + 帯の実矩形）待つ
async fn wait_band(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    label: &str,
    want: &dyn Fn(&tako_control::lsp::DocumentServer) -> bool,
) -> tako_control::lsp::DocumentServer {
    use tako_core::lsp::state::ServerState;
    let key = format!("preview-lsp-band-{}", pane.as_u64());
    let ok = wait_for_app_state(window, cx, label, Duration::from_secs(30), |app| {
        app.lsp_document_server(pane)
            .is_some_and(|s| s.state == ServerState::NotInstalled && s.reason.is_some() && want(&s))
            && app.panel_click_probe_bounds.borrow().contains_key(&key)
    })
    .await;
    notify_and_draw(any, window, cx);
    let server = window
        .update(cx, |app, _, _| app.lsp_document_server(pane))
        .ok()
        .flatten();
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} {label} band={ok} state={:?} fetch_failed={:?} reason_len={}",
        server.as_ref().map(|s| s.state),
        server.as_ref().map(|s| s.fetch_failed),
        server
            .as_ref()
            .and_then(|s| s.reason.as_ref())
            .map_or(0, |r| r.chars().count())
    );
    check(
        ok,
        &format!(
            "visual-test {SECTION} {label}: 未導入の帯が出る（state={:?}）",
            server.as_ref().map(|s| s.state)
        ),
    );
    server.unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION} {label}: 受け持つサーバが無い"
        ))
    })
}

/// ペインの実幅（`pane_text_areas` のテキスト領域へ枠 + 余白を足し戻す）
fn full_width(app: &TakoApp, pane: PaneId) -> Option<f32> {
    app.pane_text_areas
        .iter()
        .find(|(id, _)| *id == pane)
        .map(|(_, a)| f(a.size.width) + (PANE_BORDER + PANE_PADDING) * 2.0)
}

/// ペインの幅を `want` px 前後へ。この場面は左右 2 枚（`anchor` の右に `pane`）なので、2 枚の実幅の和
/// （= 仕切りの親の幅）から取り分を 1 回で決めて縮め、幅が着くまで状態で待つ（描きながら）
async fn set_width(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    anchor: PaneId,
    pane: PaneId,
    want: f32,
) -> f32 {
    let label = format!("{SECTION} 幅 {want}px");
    let drawn = wait_for_drawn_state(window, any, cx, &label, Duration::from_secs(10), |app| {
        full_width(app, anchor).is_some() && full_width(app, pane).is_some()
    })
    .await;
    let total = window
        .update(cx, |app, _, _| {
            Some(full_width(app, anchor)? + full_width(app, pane)?)
        })
        .ok()
        .flatten()
        .filter(|_| drawn)
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {SECTION}: ペインの矩形が無い（pane_text_areas）"
            ))
        });
    let share = (want / total).clamp(0.05, 0.95);
    window
        .update(cx, |app, _, cx| {
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Resize {
                    pane: Some(pane.as_u64()),
                    axis: tako_control::protocol::Axis::X,
                    delta: None,
                    share: Some(share),
                },
                PaneOrigin::Cli,
            );
            cx.notify();
        })
        .ok();
    wait_for_drawn_state(window, any, cx, &label, Duration::from_secs(10), |app| {
        full_width(app, pane).is_some_and(|w| (w - want).abs() <= 12.0)
    })
    .await;
    window
        .update(cx, |app, _, _| full_width(app, pane))
        .ok()
        .flatten()
        .unwrap_or(0.0)
}

pub(super) async fn lsp_band_2751_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use crate::lsp_status_ui::{bar_text_max_h, legacy_2751, set_legacy_2751};
    use tako_control::protocol::Request as Req;

    inject_section_failure(SECTION);
    // 節を抜けるときは起動時の腕へ戻す（`TAKO_2751_LEGACY=1` の回は旧のまま = 新の判定が落ちる）
    let env_legacy = legacy_2751();
    let anchor = ensure_fresh_scene(window, cx, SECTION).await;

    // --- 場面: 一時 dir の深い配下だけに fixture を作る（本物のファイル・言語サーバの置き場に触らない = #1811） ---
    let tmp = tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir());
    let top = tmp.join(format!("tako-vt2751-{}", std::process::id()));
    remove_on_fail(&top);
    let _ = std::fs::remove_dir_all(&top);
    std::fs::create_dir_all(&top).expect("visual-test lsp-band-2751 の一時 dir");
    let ws = top.join("ws");
    std::fs::create_dir_all(&ws).expect("visual-test lsp-band-2751 の ws");
    std::fs::write(ws.join("pyproject.toml"), "[project]\nname = \"v\"\n").expect("pyproject");
    let file = ws.join("main.py");
    let source: String = (1..=200)
        .map(|i| format!("value_{i:03} = {i}  # 行 {i}\n"))
        .collect();
    std::fs::write(&file, source).expect("main.py");
    // 差し替え口が指す「実行できないファイル」: 長いパス（帯の理由に全文が入る）と現実の長さのパス
    let mut deep = top.clone();
    for n in 1..=6 {
        deep = deep.join(format!(
            "a-long-directory-name-for-the-language-server-band-that-keeps-wrapping-in-a-narrow-pane-{n:02}"
        ));
    }
    std::fs::create_dir_all(&deep).expect("visual-test lsp-band-2751 の深い dir");
    let long_bin = deep.join("pyright-langserver");
    let short_bin = top.join("pyright-langserver");
    for bin in [&long_bin, &short_bin] {
        std::fs::write(bin, "not a server\n").expect("実行できないファイル");
    }
    let server_id = vt_lsp_server_of(&file);
    vt_lsp_override(window, cx, server_id, Some(&long_bin));

    let pane = window
        .update(cx, |app, _, cx| {
            app.drawer_visible = false;
            app.panel_visible = false;
            let opened = tako_control::dispatch(
                app,
                Req::OpenFile {
                    pane: Some(anchor.as_u64()),
                    path: file.display().to_string(),
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
            let pane = PaneId::from_raw(opened["pane"].as_u64().expect("OpenFile 応答の pane"));
            let _ = app.workspace.active_tab_mut().tree_mut().focus(pane);
            tako_control::dispatch(
                app,
                Req::PreviewEdit {
                    pane: Some(pane.as_u64()),
                    enabled: Some(true),
                },
                PaneOrigin::Cli,
            )
            .unwrap_or_else(|e| {
                fail(&format!(
                    "visual-test {SECTION}: 編集モードへ入れない: {e:?}"
                ))
            });
            cx.notify();
            pane
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 場面づくり")));
    let narrow_w = set_width(any, window, cx, anchor, pane, 240.0).await;
    let long_text = long_bin.display().to_string();
    wait_band(any, window, cx, pane, "①band", &|s| {
        s.reason.as_ref().is_some_and(|r| r.contains(&long_text))
    })
    .await;
    println!("TAKO_VISUAL_PIXEL: {SECTION} ①width pane_w={narrow_w:.1} server={server_id}");
    check(
        (200.0..=280.0).contains(&narrow_w),
        &format!("visual-test {SECTION}: 場面の前提 = ペインの幅が 240px 前後（{narrow_w:.1}px）"),
    );

    // ① 狭いペイン（起動時の腕。既定 = 新）
    let new = measure(any, window, cx, pane).await;
    print_shot("①new", &new);
    dump(any, cx, "narrow-new.png");
    let pane_rect = new
        .pane
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION} ①: ペインの矩形が無い")));
    let pane_h = f(pane_rect.size.height);
    check(
        new.code_h >= pane_h / 2.0 - 1.0,
        &format!(
            "visual-test {SECTION} ①: 本文の行がペインの高さの半分以上描かれる（{} 行・{:.1}px / 半分 {:.1}px）",
            new.lines,
            new.code_h,
            pane_h / 2.0
        ),
    );
    let band = new
        .band
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION} ①: 帯の実矩形が無い")));
    let text = new.text.unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION} ①: 帯の文の器の実矩形が無い"
        ))
    });
    // あふれている帯の器は上限ちょうどまで使う = 描画がペイン全体の高さ（`pane_height_of_area`）で上限を
    // 決めている（テキスト領域の高さのまま割ると 54px ぶん小さくなり、広いペインの帯まで削る）
    let cap = bar_text_max_h(pane_h);
    check(
        (f(text.size.height) - cap).abs() <= 1.0,
        &format!(
            "visual-test {SECTION} ①: あふれた帯の文の器はペインの高さから決めた上限ちょうど（{:.1}px / 上限 {cap:.1}px）",
            f(text.size.height)
        ),
    );
    check(
        new.scroll_max > 0.5 && new.thumb.is_some(),
        &format!(
            "visual-test {SECTION} ①: はみ出した文は器の中でスクロールでき、つまみが出る（{:.1}px・thumb={}）",
            new.scroll_max,
            fmt_b(new.thumb)
        ),
    );
    // 上限の計算（`bar_text_max_h`）が前提にする帯の外側の高さと、実際の帯の外側が合う
    let chrome = f(band.size.height) - f(text.size.height);
    println!("TAKO_VISUAL_PIXEL: {SECTION} ①chrome measured={chrome:.1}");
    check(
        chrome <= crate::lsp_status_ui::BAR_CHROME_PX + 1.0,
        &format!(
            "visual-test {SECTION} ①: 帯の外側の高さ {chrome:.1}px が上限の計算の前提（{:.1}px）以内",
            crate::lsp_status_ui::BAR_CHROME_PX
        ),
    );

    // ② 旧の形（上限なし）へその場で倒す: 同じ場面・同じ文で本文の行が 0 = 場面が #2751 の症状を再現する
    set_legacy_2751(true);
    let old = measure(any, window, cx, pane).await;
    print_shot("②legacy", &old);
    dump(any, cx, "narrow-legacy.png");
    set_legacy_2751(env_legacy);
    check(
        old.lines == 0,
        &format!(
            "visual-test {SECTION} ②: 旧の形では本文の行が 0（場面が #2751 の症状を再現する。{} 行・{:.1}px）",
            old.lines, old.code_h
        ),
    );

    // ③ 読める: 帯の文の器の上で実ホイールを回すと、最後の行（次の一手）が器の中に入る
    let before = measure(any, window, cx, pane).await;
    let text = before.text.unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION} ③: 帯の文の器の実矩形が無い"
        ))
    });
    let last_before = before.last;
    window
        .update(cx, |_, win, cx| {
            win.dispatch_event(
                gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: text.center(),
                    delta: ScrollDelta::Pixels(point(px(0.0), px(-4000.0))),
                    ..ScrollWheelEvent::default()
                }),
                cx,
            );
        })
        .ok();
    let after = measure(any, window, cx, pane).await;
    print_shot("③scrolled", &after);
    dump(any, cx, "narrow-scrolled.png");
    let last = after
        .last
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION} ③: 最後の行の実矩形が無い")));
    check(
        last_before.is_some_and(|b| !inside(b, text)),
        &format!(
            "visual-test {SECTION} ③: 送る前は最後の行が器の外（{} / 器 {}）",
            fmt_b(last_before),
            fmt_b(Some(text))
        ),
    );
    check(
        inside(last, after.text.unwrap_or(text)),
        &format!(
            "visual-test {SECTION} ③: ホイールで送ると最後の行（次の一手）が器の中に入る = 全文がその場で読める（{} / 器 {}）",
            fmt_b(Some(last)),
            fmt_b(after.text)
        ),
    );
    check(
        after.offset <= -(after.scroll_max - 0.5) && after.lines == before.lines,
        &format!(
            "visual-test {SECTION} ③: 器は末尾まで送られ、本文の行は動かない（offset {:.1} / {:.1}・行 {} → {}）",
            after.offset, after.scroll_max, before.lines, after.lines
        ),
    );
    let thumb_moved = match (before.thumb, after.thumb) {
        (Some(a), Some(b)) => f(b.top()) > f(a.top()) + 0.5,
        _ => false,
    };
    check(
        thumb_moved,
        &format!(
            "visual-test {SECTION} ③: つまみが下へ動く（{} → {}）",
            fmt_b(before.thumb),
            fmt_b(after.thumb)
        ),
    );

    // ④ 押せる: 「入れる」の実矩形が帯・ペイン・窓の中にある
    let window_size = window
        .update(cx, |_, w, _| w.viewport_size())
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 窓の寸法")));
    let window_rect = Bounds::new(point(px(0.0), px(0.0)), window_size);
    let button = after.button.unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION} ④: 「入れる」の実矩形が無い"
        ))
    });
    check(
        inside(button, band) && inside(button, pane_rect) && inside(button, window_rect),
        &format!(
            "visual-test {SECTION} ④: 「入れる」が帯・ペイン・窓の中に見えている（{} / 帯 {} / ペイン {}）",
            fmt_b(Some(button)),
            fmt_b(Some(band)),
            fmt_b(Some(pane_rect))
        ),
    );
    let loopback = std::env::var("TAKO_LSP_FETCH_BASE").is_ok_and(|base| {
        let base = base.trim();
        base.starts_with("http://127.0.0.1:") || base.starts_with("http://localhost:")
    });
    if loopback {
        click_at(any, cx, button.center());
        let failed = wait_for_app_state(window, cx, "④install", Duration::from_secs(30), |app| {
            app.lsp_document_server(pane)
                .is_some_and(|s| s.fetch_failed && s.fetch.is_none())
        })
        .await;
        let pressed = measure(any, window, cx, pane).await;
        print_shot("④pressed", &pressed);
        dump(any, cx, "narrow-pressed.png");
        println!("TAKO_VISUAL_PIXEL: {SECTION} ④click fetch_failed={failed}");
        check(
            failed,
            &format!("visual-test {SECTION} ④: 実マウスで押した「入れる」が取得を起こし、閉じた配布元で失敗の帯へ変わる"),
        );
        check(
            pressed.offset.abs() < 0.5 && pressed.code_h >= f(pressed.pane.unwrap_or(pane_rect).size.height) / 2.0 - 1.0,
            &format!(
                "visual-test {SECTION} ④: 文が変わった帯は頭から見せ、本文の行は半分以上のまま（offset {:.1}・{:.1}px）",
                pressed.offset, pressed.code_h
            ),
        );
    } else {
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ④click skipped（TAKO_LSP_FETCH_BASE がループバックでない = 本物の配布元から取らない）"
        );
    }

    // ⑤ 広いペイン: 現実の長さのパスへ差し替え、新と旧の形の画素の差が 0
    vt_lsp_override(window, cx, server_id, Some(&short_bin));
    let wide_w = set_width(any, window, cx, anchor, pane, 720.0).await;
    let short_text = short_bin.display().to_string();
    wait_band(any, window, cx, pane, "⑤band", &|s| {
        s.reason
            .as_ref()
            .is_some_and(|r| r.contains(&short_text) || s.fetch_failed)
    })
    .await;
    set_legacy_2751(false);
    let wide = measure(any, window, cx, pane).await;
    print_shot("⑤wide-new", &wide);
    let wide_new = capture_frame(any, cx);
    dump(any, cx, "wide-new.png");
    set_legacy_2751(true);
    let wide_old = measure(any, window, cx, pane).await;
    print_shot("⑤wide-legacy", &wide_old);
    let wide_legacy = capture_frame(any, cx);
    dump(any, cx, "wide-legacy.png");
    set_legacy_2751(env_legacy);
    let wide_rect = wide
        .pane
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION} ⑤: ペインの矩形が無い")));
    let changed = match (wide_new.as_ref(), wide_legacy.as_ref()) {
        (Some((a, scale)), Some((b, _))) => changed_pixels_in_bounds(a, b, &[wide_rect], *scale),
        _ => fail(&format!("visual-test {SECTION} ⑤: フレームを撮れない")),
    };
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ⑤wide pane_w={wide_w:.1} changed={changed} band_h={:.1}/{:.1} lines={}/{}",
        wide.band.map_or(0.0, |b| f(b.size.height)),
        wide_old.band.map_or(0.0, |b| f(b.size.height)),
        wide.lines,
        wide_old.lines
    );
    check(
        wide_w >= 600.0,
        &format!("visual-test {SECTION} ⑤: 場面の前提 = 広いペイン（{wide_w:.1}px）"),
    );
    check(
        wide.scroll_max < 0.5 && wide.thumb.is_none(),
        &format!(
            "visual-test {SECTION} ⑤: 広いペインの帯は上限に届かない（はみ出し {:.1}px・thumb={}）",
            wide.scroll_max,
            fmt_b(wide.thumb)
        ),
    );
    check(
        changed == 0,
        &format!("visual-test {SECTION} ⑤: 広いペインの見た目は旧の形と同じ（画素の差 {changed}）"),
    );

    vt_lsp_override(window, cx, server_id, None);
    let _ = std::fs::remove_dir_all(&top);
}
