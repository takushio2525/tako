//! visual-test `preview-memory`（FR-3.17 / Issue #2009）: 見えていないプレビューを手放し、
//! 表に戻したときに**見た目と操作が変わらない**か。
//!
//! 場面: 素のシェルのタブ A と、PDF（12 ページ・5 ページ目を表示）+ 大きな画像（9000x4000 =
//! デコード後 約 137 MiB。実効上限の下限 128 MiB を超える = 上限ぎりぎりの端）を開いたタブ B。
//! ① B を表に出して描き切った絵を撮る（前）。画像が実効上限に収まり、追い出されずに描ける
//! ② A へ切り替え、猶予（30 秒）を過ぎた掃除を回す → 2 枚とも手放す（デコード済み 0・
//!    PDF は表示中のページと前後以外のページ PNG を手放す・実効上限は下限へ下がる）
//! ③ B へ戻す → 作り直しが数えられ、PDF は再ラスタで全ページが戻り、デコードし直す
//! ④ 戻した絵（後）が前と同じ（プレビューのペインの矩形の画素が一致）
//! ⑤ 戻した直後の操作: 再ラスタが戻る前に遠いページ（12 ページ目）へ飛んでも、そのページが描ける
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ `preview-memory-{before,hidden,after,jump}.png` で落とす。
//!
//! A/B（`TAKO_2009_LEGACY=1` = 修正前）は ② で落ちる（見えていないプレビューを手放さない）。
//! 単独実行は `TAKO_VISUAL_ONLY=preview-memory`（`scripts/test-preview-memory-2009.sh` の visual 段）

use super::*;

const SECTION: &str = "preview-memory";

/// 色の違う矩形と文字を 1 ページずつ描いた PDF（再ラスタの前後で絵が揃うかを見る）
fn write_pages_pdf(path: &std::path::Path, pages: usize) {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        Vec::new(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut kids = Vec::new();
    for p in 0..pages {
        let (r, g, b) = (
            (p * 37 % 100) as f32 / 100.0,
            (p * 59 % 100) as f32 / 100.0,
            (p * 83 % 100) as f32 / 100.0,
        );
        let content = format!(
            "{r:.2} {g:.2} {b:.2} rg 60 380 480 300 re f 0 0 0 rg \
             BT /F1 40 Tf 72 300 Td (Page {n}) Tj ET \
             BT /F1 12 Tf 72 260 Td (tako #2009 preview memory) Tj ET",
            n = p + 1
        );
        let content_id = objs.len() + 1;
        objs.push(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .into_bytes(),
        );
        objs.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        kids.push(objs.len());
    }
    objs[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {pages} >>",
        kids.iter()
            .map(|k| format!("{k} 0 R"))
            .collect::<Vec<_>>()
            .join(" ")
    )
    .into_bytes();
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(obj);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(path, pdf)
        .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: PDF を書けない: {e}")));
}

/// 2 枚のフレームで、矩形（論理 px）の中の違う画素の数と画素数
fn diff_in(
    before: &image::RgbaImage,
    after: &image::RgbaImage,
    rect: Bounds<Pixels>,
    scale: f32,
) -> (u64, u64) {
    if before.dimensions() != after.dimensions() {
        return (u64::MAX, 0);
    }
    let (w, h) = after.dimensions();
    let x0 = ((f32::from(rect.origin.x) * scale) as u32).min(w);
    let y0 = ((f32::from(rect.origin.y) * scale) as u32).min(h);
    let x1 = ((f32::from(rect.origin.x + rect.size.width) * scale) as u32).min(w);
    let y1 = ((f32::from(rect.origin.y + rect.size.height) * scale) as u32).min(h);
    let mut changed = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if before.get_pixel(x, y) != after.get_pixel(x, y) {
                changed += 1;
            }
        }
    }
    (changed, u64::from(x1 - x0) * u64::from(y1 - y0))
}

/// 矩形の中の色の種類（空白 = 背景 1 色だけ、を見分ける）
fn distinct_colors(frame: &image::RgbaImage, rect: Bounds<Pixels>, scale: f32) -> usize {
    let (w, h) = frame.dimensions();
    let x0 = ((f32::from(rect.origin.x) * scale) as u32).min(w);
    let y0 = ((f32::from(rect.origin.y) * scale) as u32).min(h);
    let x1 = ((f32::from(rect.origin.x + rect.size.width) * scale) as u32).min(w);
    let y1 = ((f32::from(rect.origin.y + rect.size.height) * scale) as u32).min(h);
    let mut seen = std::collections::HashSet::new();
    for y in (y0..y1).step_by(3) {
        for x in (x0..x1).step_by(3) {
            seen.insert(frame.get_pixel(x, y).0);
            if seen.len() > 64 {
                return seen.len();
            }
        }
    }
    seen.len()
}

fn area_of(app: &TakoApp, pane: PaneId) -> Option<Bounds<Pixels>> {
    app.pane_text_areas
        .iter()
        .find(|(id, _)| *id == pane)
        .map(|(_, b)| *b)
}

/// 開いたプレビューを描き切るまで回す（画像のデコードは GPUI の背景で進む）。
/// `filled` の矩形がどれも空白でなく、2 枚続けて同じ絵になったら描き切ったと見る
/// （空白どうしも「同じ絵」なので、デコード待ちの間に止まらないよう矩形も見る）
async fn settle(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    filled: &[Bounds<Pixels>],
) -> Option<(image::RgbaImage, f32)> {
    let mut last: Option<image::RgbaImage> = None;
    for _ in 0..150 {
        notify_and_draw(any, window, cx);
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        let (frame, scale) = capture_frame(any, cx)?;
        let ready = filled
            .iter()
            .all(|rect| distinct_colors(&frame, *rect, scale) > 2);
        if ready && last.as_ref() == Some(&frame) {
            return Some((frame, scale));
        }
        last = Some(frame);
    }
    capture_frame(any, cx)
}

/// `rect` に絵が出るまで描いて待つ（出るまでの時間。出なければ None）
async fn first_filled(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    rect: Bounds<Pixels>,
    timeout: Duration,
) -> Option<Duration> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        notify_and_draw(any, window, cx);
        if let Some((frame, scale)) = capture_frame(any, cx) {
            if distinct_colors(&frame, rect, scale) > 2 {
                return Some(started.elapsed());
            }
        }
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
    None
}

fn dump(frame: &image::RgbaImage, name: &str) {
    if let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(name);
        if frame.save(&path).is_ok() {
            println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
        }
    }
}

pub(super) async fn preview_memory_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let legacy = tako_core::preview_cache::issue2009_legacy();
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let tab_a = window
        .update(cx, |app, _, _| app.workspace.active_tab().id())
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: タブ A が無い")));

    // --- 素材 ---
    let dir = std::env::temp_dir().join(format!("tako-2009-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let pdf = dir.join("pages12.pdf");
    write_pages_pdf(&pdf, 12);
    let big = dir.join("big9000.png");
    // デコード後 9000 x 4000 x 4 = 137 MiB（実効上限の下限 128 MiB を超える）
    let img = image::RgbaImage::from_fn(9000, 4000, |x, y| {
        image::Rgba([
            (x / 36 % 256) as u8,
            (y / 16 % 256) as u8,
            ((x + y) / 50 % 256) as u8,
            255,
        ])
    });
    img.save(&big)
        .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 画像を書けない: {e}")));
    drop(img);

    // --- タブ B: PDF（右）+ 画像（下） ---
    let (tab_b, pdf_pane, img_pane) = window
        .update(cx, |app, _, cx| {
            let mut made = tako_control::dispatch(
                app,
                tako_control::protocol::Request::TabNew {
                    title: Some("B".into()),
                    focus: Some(true),
                    cwd: None,
                },
                PaneOrigin::User,
            );
            let _ = app.after_dispatch(&mut made, false, cx);
            let made = made.ok()?;
            let tab = TabId::from_raw(made["tab"].as_u64()?);
            let shell = made["pane"].as_u64()?;
            let open =
                |app: &mut TakoApp,
                 cx: &mut Context<TakoApp>,
                 path: &std::path::Path,
                 direction: Option<tako_control::protocol::Direction>| {
                    let mut opened = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(shell),
                            path: path.display().to_string(),
                            mode: None,
                            direction,
                            focus: None,
                            new_tab: false,
                            line: None,
                            column: None,
                        },
                        PaneOrigin::Cli,
                    );
                    let _ = app.after_dispatch(&mut opened, false, cx);
                    app.drain_pending_preview_loads(cx);
                    opened
                        .ok()
                        .and_then(|v| v["pane"].as_u64())
                        .map(PaneId::from_raw)
                };
            let pdf_pane = open(
                app,
                cx,
                &pdf,
                Some(tako_control::protocol::Direction::Right),
            )?;
            let img_pane = open(app, cx, &big, Some(tako_control::protocol::Direction::Down))?;
            cx.notify();
            Some((tab, pdf_pane, img_pane))
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {SECTION}: タブ B と 2 枚のプレビューを開けない"
            ))
        });
    wait_for_preview_state(window, cx, Duration::from_secs(20), |app| {
        matches!(
            app.previews.get(&pdf_pane).map(|s| &s.content),
            Some(preview::PreviewContent::Pdf(d)) if d.pages.iter().all(|png| !png.is_empty())
        ) && matches!(
            app.previews.get(&img_pane).map(|s| &s.content),
            Some(preview::PreviewContent::Image(_))
        )
    })
    .await
    .unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: PDF / 画像が読み込まれない"
        ))
    });
    // 5 ページ目へ（手放すときに「表示中のページと前後」以外を手放す相手を作る）
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::PreviewView {
                pane: Some(pdf_pane.as_u64()),
                zoom: None,
                zoom_in: false,
                zoom_out: false,
                reset: false,
                page: Some(5),
                pan_x: None,
                pan_y: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    // ① 描き切った絵（前）。矩形は 1 度描いてから読む（描画が組む）
    let _ = settle(any, window, cx, &[]).await;
    let (pdf_area, img_area) = window
        .update(cx, |app, _, _| {
            (area_of(app, pdf_pane), area_of(app, img_pane))
        })
        .unwrap_or((None, None));
    let (Some(pdf_area), Some(img_area)) = (pdf_area, img_area) else {
        fail(&format!("visual-test {SECTION}: プレビューの矩形が無い"))
    };
    let (before, scale) = settle(any, window, cx, &[pdf_area, img_area])
        .await
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 前の絵を撮れない")));
    dump(&before, "preview-memory-before.png");
    let (budget, img_cached) = window
        .update(cx, |app, _, _| {
            (
                app.preview_image_lru.budget_bytes(),
                app.preview_memory_stats(std::time::Instant::now())
                    .panes
                    .iter()
                    .any(|m| m.pane == img_pane.as_u64() && m.decoded_bytes > 0),
            )
        })
        .unwrap_or((0, false));
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} before pdf_colors={} img_colors={} budget_mb={} img_cached={img_cached}",
        distinct_colors(&before, pdf_area, scale),
        distinct_colors(&before, img_area, scale),
        budget / 1024 / 1024
    );
    check(
        distinct_colors(&before, pdf_area, scale) > 2
            && distinct_colors(&before, img_area, scale) > 2,
        &format!("visual-test {SECTION}: 前提 = PDF と画像が描かれている"),
    );
    check(
        img_cached && budget >= 9000 * 4000 * 4,
        &format!(
            "visual-test {SECTION}: ① 下限を超える画像が実効上限に収まり、追い出されずに描ける \
             (budget={} MiB)",
            budget / 1024 / 1024
        ),
    );

    // ② A へ切り替え、猶予を過ぎた掃除を回す
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::TabSelect {
                tab: tab_a.as_u64(),
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    let _ = settle(any, window, cx, &[]).await;
    let released = window
        .update(cx, |app, _, cx| {
            let now = std::time::Instant::now();
            app.sweep_preview_memory(now);
            let later =
                now + Duration::from_secs(tako_core::preview_cache::HIDDEN_RELEASE_AFTER_SECS + 1);
            let changed = app.sweep_preview_memory(later);
            cx.notify();
            let stats = app.preview_memory_stats(later);
            let pdf_released_pages = match app.previews.get(&pdf_pane).map(|s| &s.content) {
                Some(preview::PreviewContent::Pdf(d)) => d.released.iter().filter(|r| **r).count(),
                _ => 0,
            };
            (changed, stats, pdf_released_pages)
        })
        .ok();
    let Some((changed, stats, pdf_released_pages)) = released else {
        fail(&format!("visual-test {SECTION}: 掃除を回せない"))
    };
    let (hidden, _) = settle(any, window, cx, &[])
        .await
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 隠した後の絵を撮れない")));
    dump(&hidden, "preview-memory-hidden.png");
    let pending = window
        .update(cx, |app, _, _| app.pending_preview_image_evictions.len())
        .unwrap_or(usize::MAX);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} hidden legacy={legacy} changed={changed} released_total={} \
         used={} effective_mb={} pdf_released_pages={pdf_released_pages} pending_evictions={pending}",
        stats.released_total,
        stats.used_bytes,
        stats.effective_max_bytes / 1024 / 1024,
    );
    let both_released = [pdf_pane, img_pane].iter().all(|p| {
        stats
            .panes
            .iter()
            .any(|m| m.pane == p.as_u64() && m.released && m.decoded_bytes == 0 && !m.visible)
    });
    check(
        both_released && stats.used_bytes == 0,
        &format!(
            "visual-test {SECTION}: ② 見えていない 2 枚を手放す（デコード済み 0。used={}）",
            stats.used_bytes
        ),
    );
    check(
        pdf_released_pages == 12 - 3,
        &format!(
            "visual-test {SECTION}: ② PDF は表示中のページと前後の 3 枚だけ残す（手放した {pdf_released_pages} 枚）"
        ),
    );
    check(
        stats.effective_max_bytes == tako_core::preview_cache::EFFECTIVE_FLOOR_MB * 1024 * 1024,
        &format!(
            "visual-test {SECTION}: ② 何も見えていなければ実効上限は下限（{} MiB）",
            stats.effective_max_bytes / 1024 / 1024
        ),
    );
    check(
        pending == 0,
        &format!(
            "visual-test {SECTION}: ② 手放した画像は次の描画で GPUI から外れる（残り {pending}）"
        ),
    );
    // ③ B へ戻す → 作り直し
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::TabSelect {
                tab: tab_b.as_u64(),
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    let restored_in = wait_for_preview_state(window, cx, Duration::from_secs(20), |app| {
        let pdf_back = matches!(
            app.previews.get(&pdf_pane).map(|s| &s.content),
            Some(preview::PreviewContent::Pdf(d)) if !d.has_released_pages()
                && d.pages.iter().all(|png| !png.is_empty())
        );
        pdf_back
            && !app.preview_released.contains(&pdf_pane)
            && !app.preview_released.contains(&img_pane)
    })
    .await;
    for _ in 0..4 {
        notify_and_draw(any, window, cx);
    }
    let (after, _) = settle(any, window, cx, &[pdf_area, img_area])
        .await
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 戻した絵を撮れない")));
    dump(&after, "preview-memory-after.png");
    let restored_total = window
        .update(cx, |app, _, _| app.preview_restored_total)
        .unwrap_or(0);
    let (pdf_diff, pdf_px) = diff_in(&before, &after, pdf_area, scale);
    let (img_diff, img_px) = diff_in(&before, &after, img_area, scale);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} after restored_in_ms={} restored_total={restored_total} \
         pdf_diff={pdf_diff}/{pdf_px} img_diff={img_diff}/{img_px}",
        restored_in.map_or(-1, |d| d.as_millis() as i64)
    );
    check(
        restored_in.is_some() && restored_total >= 2,
        &format!(
            "visual-test {SECTION}: ③ 戻すと作り直しが数えられ、PDF の全ページが再ラスタで戻る \
             (restored_total={restored_total})"
        ),
    );
    check(
        pdf_diff == 0 && img_diff == 0,
        &format!(
            "visual-test {SECTION}: ④ 戻した絵が前と同じ（PDF {pdf_diff} / 画像 {img_diff} 画素が違う）"
        ),
    );

    // ⑤ 戻した直後の操作: もう一度手放してから戻し、再ラスタが戻る前に 12 ページ目へ飛ぶ
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::TabSelect {
                tab: tab_a.as_u64(),
            },
            PaneOrigin::Cli,
        );
        let now = std::time::Instant::now();
        app.sweep_preview_memory(now);
        app.sweep_preview_memory(
            now + Duration::from_secs(tako_core::preview_cache::HIDDEN_RELEASE_AFTER_SECS + 1),
        );
        cx.notify();
    });
    let _ = settle(any, window, cx, &[]).await;
    let jumped = window
        .update(cx, |app, _, cx| {
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::TabSelect {
                    tab: tab_b.as_u64(),
                },
                PaneOrigin::Cli,
            );
            let jump = tako_control::dispatch(
                app,
                tako_control::protocol::Request::PreviewView {
                    pane: Some(pdf_pane.as_u64()),
                    zoom: None,
                    zoom_in: false,
                    zoom_out: false,
                    reset: false,
                    page: Some(12),
                    pan_x: None,
                    pan_y: None,
                },
                PaneOrigin::Cli,
            );
            cx.notify();
            jump.is_ok()
        })
        .unwrap_or(false);
    // 戻してから画像（デコードし直し）が描けるまでの時間（手放した代わりに待つ時間）
    let img_back = first_filled(any, window, cx, img_area, Duration::from_secs(20)).await;
    let raster_back = wait_for_preview_state(window, cx, Duration::from_secs(20), |app| {
        matches!(
            app.previews.get(&pdf_pane).map(|s| &s.content),
            Some(preview::PreviewContent::Pdf(d)) if !d.has_released_pages()
        )
    })
    .await;
    let (jump_frame, _) = settle(any, window, cx, &[pdf_area, img_area])
        .await
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 飛んだ後の絵を撮れない")));
    dump(&jump_frame, "preview-memory-jump.png");
    let page_now = window
        .update(cx, |app, _, _| {
            app.preview_views
                .get(&pdf_pane)
                .map(|v| v.page)
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let jump_colors = distinct_colors(&jump_frame, pdf_area, scale);
    let jump_img_colors = distinct_colors(&jump_frame, img_area, scale);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} jump ok={jumped} page={page_now} raster_back_ms={} \
         img_back_ms={} colors={jump_colors} img_colors={jump_img_colors}",
        raster_back.map_or(-1, |d| d.as_millis() as i64),
        img_back.map_or(-1, |d| d.as_millis() as i64)
    );
    check(
        jumped && page_now == 12 && raster_back.is_some() && jump_colors > 2,
        &format!(
            "visual-test {SECTION}: ⑤ 戻した直後（再ラスタ中）に遠いページへ飛んでも、そのページが描ける \
             (page={page_now} colors={jump_colors})"
        ),
    );
    check(
        img_back.is_some() && jump_img_colors > 2,
        &format!(
            "visual-test {SECTION}: ⑤ 2 度目に戻しても画像がデコードし直されて描ける \
             (img_colors={jump_img_colors})"
        ),
    );

    let _ = std::fs::remove_dir_all(&dir);
}
