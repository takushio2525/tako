//! visual-test `tree-notice-2760`（Issue #2760）: ファイルツリーの断りの通知が、長いパスでも
//! 通知欄の高さの上限に収まり、ツリーの行を窓の外へ押し出さないかを**実マウス・実キー**の
//! 入口（⌘C → ⌥⌘V）から確かめる。
//!
//! 場面は一時 dir の深い配下（`<一時 dir>/tako-vt2760-<pid>/<長い名前>/<長い名前>/ws`）をルートにする。
//! ルートの最後の行の下に「上限ぶん + 少し」だけ隙間が残るよう、節が測ってから詰め物の行を足す
//! （窓の高さ・歓迎の帯の有無に左右されない）。
//!
//! ① 日本語表示で `folder/inner` を ⌘C → `x.txt` の行で ⌥⌘V（自分自身へ）を断らせ、通知の高さ・
//!    行数・ツリーの最初の行の上端・最後の行の下端・窓の高さを 1 行に出す
//!    （`TAKO_VISUAL_PIXEL: tree-notice-2760 ① …`）。通知の高さが `NOTICE_MAX_H` 以内・最後の行が
//!    窓の中・文面にルートの絶対パスが無く相対パスと理由がある・縮めた断りは上限の中に全文が収まる
//! ② 英語表示で同じ（文面の言語だけが変わる）
//! ③ とても長い日本語のファイル名（同じ名前がある = 上書きしない）の断りは、高さが上限で止まり、
//!    通知欄の中でスクロールすると末尾まで読める
//! ④ ルートの外のパス（ホーム / ホームの外）の通知も壊れない
//!
//! **A/B**: `TAKO_2760_LEGACY=1`（絶対パスのまま・上限なし）は ① の「通知の高さが上限以内」で FAILED。
//! 単独実行は `TAKO_VISUAL_ONLY=tree-notice-2760`（`scripts/test-tree-notice-2760.sh`）

use super::*;

const SECTION: &str = "tree-notice-2760";

pub(super) async fn tree_notice_2760_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use crate::sidebar::{NOTICE_LINE_PX, NOTICE_MAX_H, NOTICE_PROBE};
    use tako_core::i18n::Lang;

    inject_section_failure(SECTION);
    // 一般のペーストボード（ユーザーのクリップボード）へ書かない（#1860 と同じ）
    if std::env::var("TAKO_FILE_PASTEBOARD")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true)
    {
        std::env::set_var(
            "TAKO_FILE_PASTEBOARD",
            format!("tako-vt2760-{}", std::process::id()),
        );
    }
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let lang_before = tako_core::i18n::lang();
    tako_core::i18n::set_lang(Lang::Ja);
    let mac = cfg!(target_os = "macos");
    let plain = Modifiers::default();
    let key = |letter: &str| {
        if mac {
            format!("cmd-{letter}")
        } else {
            format!("ctrl-{letter}")
        }
    };
    let move_key = if mac { "cmd-alt-v" } else { "ctrl-alt-v" };

    // --- 場面: 一時 dir の深い配下だけに fixture を作る（本物のファイルを書き換えない = #1811） ---
    let tmp = tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir());
    let top = tmp.join(format!("tako-vt2760-{}", std::process::id()));
    remove_on_fail(&top);
    let _ = std::fs::remove_dir_all(&top);
    let raw = top
        .join("a-long-directory-name-that-makes-the-absolute-path-wrap")
        .join("another-long-directory-name-for-the-notice-2760")
        .join("ws");
    // とても長い日本語のファイル名（199 バイト = 1 成分の上限 255 バイトの内側）
    let long_name = format!("{}.txt", "とても長い名前の資料の写し".repeat(5));
    for dir in ["folder/inner", "dst"] {
        std::fs::create_dir_all(raw.join(dir)).expect("visual-test tree-notice-2760 fixture");
    }
    let base = tako_core::platform::path::canonicalize_or_self(&raw);
    if !base.starts_with(&tmp) || base == tmp {
        fail(&format!(
            "visual-test {SECTION}: fixture が一時 dir の外 ({})",
            base.display()
        ));
    }
    let at = |rel: &str| base.join(rel);
    for (rel, body) in [
        ("folder/inner/x.txt".to_string(), "X\n"),
        (format!("folder/{long_name}"), "L\n"),
        (format!("dst/{long_name}"), "D\n"),
    ] {
        std::fs::write(at(&rel), body).expect("visual-test tree-notice-2760 fixture");
    }
    window
        .update(cx, |app, _, cx| {
            app.drawer_visible = false;
            app.panel_visible = false;
            app.filetree.visible = true;
            let tab = app.workspace.active_tab_id();
            if let Some(t) = app.workspace.get_tab_mut(tab) {
                t.add_pinned_folder(base.clone());
            }
            app.sync_filetree_roots();
            for r in app.filetree.rows() {
                if r.root && r.expanded && r.entry.path != base {
                    app.filetree.toggle_dir(&r.entry.path);
                }
            }
            app.filetree.expand_dir(&base);
            for rel in ["folder", "folder/inner"] {
                app.filetree.expand_dir(&base.join(rel));
            }
            app.tree_row_probe = true;
            app.remote_notice = None;
            app.file_clipboard = None;
            app.tree_selection = None;
            cx.notify();
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: 場面づくり")));

    // --- 観測の道具 ---
    let window_h = |cx: &mut AsyncApp| {
        window
            .update(cx, |_, w, _| w.viewport_size().height)
            .unwrap_or(px(0.0))
    };
    // ルート（ws）の行の矩形（見出しを含む）と、通知欄の矩形
    let measure = |cx: &mut AsyncApp| {
        let rects = vt1834_rows(any, window, cx);
        let rows: Vec<Bounds<Pixels>> = rects
            .iter()
            .filter(|(p, _)| p.starts_with(&base))
            .map(|(_, b)| *b)
            .collect();
        let notice = rects
            .iter()
            .find(|(p, _)| p.as_os_str() == NOTICE_PROBE)
            .map(|(_, b)| *b);
        (rows, notice)
    };
    let first_top = |rows: &[Bounds<Pixels>]| {
        rows.iter()
            .map(|b| b.top())
            .fold(None, |m: Option<Pixels>, t| Some(m.map_or(t, |m| m.min(t))))
            .unwrap_or(px(0.0))
    };
    let last_bottom = |rows: &[Bounds<Pixels>]| {
        rows.iter()
            .map(|b| b.bottom())
            .fold(px(0.0), |m, b| if b > m { b } else { m })
    };
    let row_point = |cx: &mut AsyncApp, path: &std::path::Path| {
        let rect = vt1834_rows(any, window, cx)
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b)
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test {SECTION}: 行が描かれていない（{}）",
                    path.display()
                ))
            });
        if rect.bottom() > window_h(cx) {
            fail(&format!(
                "visual-test {SECTION}: 押す行が窓の外（{} の下端 {:?}）",
                path.display(),
                rect.bottom()
            ));
        }
        vt1834_grab(rect)
    };
    let notice = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                app.remote_notice.as_ref().map(|n| n.text.clone())
            })
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    let clear_notice = |cx: &mut AsyncApp| {
        let _ = window.update(cx, |app, _, cx| {
            app.remote_notice = None;
            cx.notify();
        });
        notify_and_draw(any, window, cx);
    };
    // 通知欄の中のスクロール（縦の最大量）
    let scroll_max = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.notice_scroll.max_offset().y)
            .unwrap_or(px(0.0))
    };
    let dump = |cx: &mut AsyncApp, name: &str| {
        if let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
            if let Some((img, _)) = capture_frame(any, cx) {
                let _ = img.save(std::path::Path::new(&dir).join(format!("{SECTION}-{name}.png")));
            }
        }
    };
    let landed = Duration::from_secs(10);
    let base_text = base.display().to_string();
    let rel = |parts: &[&str]| {
        parts
            .iter()
            .fold(std::path::PathBuf::new(), |p, s| p.join(s))
            .display()
            .to_string()
    };

    // --- 前提: ルートの最後の行の下に「上限ぶん + 少し」の隙間を作る ---
    let (rows, _) = measure(cx);
    let row_h = rows.last().map(|b| b.size.height).unwrap_or(px(0.0));
    check(
        row_h > px(0.0),
        &format!("visual-test {SECTION}: ルートの行が描かれる (#2760)"),
    );
    let margin = px(4.0);
    let free = window_h(cx) - last_bottom(&rows);
    let room = free - px(NOTICE_MAX_H) - margin;
    let fill = if room > px(0.0) {
        (f32::from(room) / f32::from(row_h)).floor() as usize
    } else {
        0
    };
    for i in 0..fill {
        std::fs::write(at(&format!("zz-fill-{i:02}.txt")), "F\n")
            .expect("visual-test tree-notice-2760 fixture");
    }
    let _ = window.update(cx, |app, _, cx| {
        app.filetree.refresh_dir(&base);
        cx.notify();
    });
    let want_rows = rows.len() + fill;
    let ok = vt1860_wait(any, window, cx, landed, &|cx| {
        measure(cx).0.len() == want_rows
    })
    .await;
    let (rows, _) = measure(cx);
    let free = window_h(cx) - last_bottom(&rows);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} premise window_h={:.1} rows={} fill={fill} row_h={:.1} \
         free={:.1} cap={NOTICE_MAX_H:.1}",
        f32::from(window_h(cx)),
        rows.len(),
        f32::from(row_h),
        f32::from(free),
    );
    check(
        ok && free >= px(NOTICE_MAX_H) && free < px(NOTICE_MAX_H) + margin + row_h,
        &format!(
            "visual-test {SECTION}: 前提 = 最後の行の下に上限ぶんの隙間（{:.1}px）がある (#2760。窓が低すぎる)",
            f32::from(free)
        ),
    );
    let before_first = first_top(&rows);
    let before_last = last_bottom(&rows);

    // 自分自身へ移す ⌥⌘V を断らせる（`folder/inner` を ⌘C → `x.txt` の行で ⌥⌘V = 貼り付け先は
    // その親 = コピーした `folder/inner` 自身。tree-multiselect ⑥ と同じ操作）
    let refuse_into_self = |cx: &mut AsyncApp| {
        clear_notice(cx);
        let inner_pt = row_point(cx, &at("folder/inner"));
        tree_multiselect::press(any, window, cx, inner_pt, MouseButton::Left, plain);
        let _ = window.update(cx, |app, _, cx| {
            app.filetree.expand_dir(&base.join("folder/inner"));
            cx.notify();
        });
        notify_and_draw(any, window, cx);
        vt1860_press(any, window, cx, &key("c"));
        let x_pt = row_point(cx, &at("folder/inner/x.txt"));
        tree_multiselect::press(any, window, cx, x_pt, MouseButton::Left, plain);
        vt1860_press(any, window, cx, move_key);
    };
    let report = |cx: &mut AsyncApp, label: &str| {
        let (rows, rect) = measure(cx);
        let h = rect.map(|b| f32::from(b.size.height)).unwrap_or(0.0);
        // 本文の行数の目安（余白 10px と下の枠線 1px を除いて 1 行の高さで割る。上限で止まれば
        // 見えている行数・止まらなければ全文の行数）
        let lines = (h - 11.0) / NOTICE_LINE_PX;
        // スクロールの器そのものの高さ（旧の形は器に繋がないので 0）
        let box_h = window
            .update(cx, |app, _, _| app.notice_scroll.bounds().size.height)
            .map(f32::from)
            .unwrap_or(0.0);
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} {label} notice_h={h:.1} lines={lines:.1} cap={NOTICE_MAX_H:.1} \
             box_h={box_h:.1} first_row_top={:.1}->{:.1} last_row_bottom={:.1}->{:.1} window_h={:.1} \
             scroll_max={:.1}",
            f32::from(before_first),
            f32::from(first_top(&rows)),
            f32::from(before_last),
            f32::from(last_bottom(&rows)),
            f32::from(window_h(cx)),
            f32::from(scroll_max(cx)),
        );
        (rows, rect)
    };

    // ① 日本語表示: 自分自身へ移す ⌥⌘V を断らせる
    refuse_into_self(cx);
    let shown = vt1860_wait(any, window, cx, landed, &|cx| {
        measure(cx).1.is_some() && !notice(cx).is_empty()
    })
    .await;
    let (rows, rect) = report(cx, "①ja");
    let text = notice(cx);
    println!("TAKO_VISUAL_PIXEL: {SECTION} ①ja text={text:?}");
    dump(cx, "ja");
    check(
        shown && text.contains("自分自身"),
        &format!("visual-test {SECTION} ①: 自分自身へは移せない理由の通知が出る (#2760。{text:?})"),
    );
    let h = rect.map(|b| b.size.height).unwrap_or(px(0.0));
    check(
        h <= px(NOTICE_MAX_H + 0.5),
        &format!(
            "visual-test {SECTION} ①: 通知の高さが上限以内 (#2760。{:.1}px > {NOTICE_MAX_H:.1}px)",
            f32::from(h)
        ),
    );
    check(
        last_bottom(&rows) <= window_h(cx),
        &format!(
            "visual-test {SECTION} ①: ツリーの最後の行が窓の中に残る (#2760。下端 {:.1}px > 窓 {:.1}px)",
            f32::from(last_bottom(&rows)),
            f32::from(window_h(cx))
        ),
    );
    check(
        !text.contains(&base_text)
            && text.contains(&rel(&["folder", "inner", "x.txt"]))
            && text.contains(&rel(&["folder", "inner"])),
        &format!(
            "visual-test {SECTION} ①: 文面のパスはルートからの相対（絶対パスを載せない）(#2760。{text:?})"
        ),
    );
    check(
        scroll_max(cx) <= px(0.5),
        &format!(
            "visual-test {SECTION} ①: 縮めた断りは上限の中に全文が収まる (#2760。はみ出し {:.1}px)",
            f32::from(scroll_max(cx))
        ),
    );

    // ② 英語表示: 同じ操作で文面の言語だけが変わる
    tako_core::i18n::set_lang(Lang::En);
    refuse_into_self(cx);
    let shown = vt1860_wait(any, window, cx, landed, &|cx| {
        measure(cx).1.is_some() && notice(cx).contains("itself")
    })
    .await;
    let (rows, rect) = report(cx, "②en");
    let text = notice(cx);
    println!("TAKO_VISUAL_PIXEL: {SECTION} ②en text={text:?}");
    dump(cx, "en");
    tako_core::i18n::set_lang(Lang::Ja);
    check(
        shown,
        &format!(
            "visual-test {SECTION} ②: 英語表示で自分自身へは移せない理由が出る (#2760。{text:?})"
        ),
    );
    check(
        rect.is_some_and(|b| b.size.height <= px(NOTICE_MAX_H + 0.5))
            && last_bottom(&rows) <= window_h(cx),
        &format!("visual-test {SECTION} ②: 英語表示でも上限以内・最後の行が窓の中 (#2760)"),
    );
    check(
        !text.contains(&base_text) && text.contains(&rel(&["folder", "inner"])),
        &format!("visual-test {SECTION} ②: 英語表示でも相対パス (#2760。{text:?})"),
    );
    check(
        scroll_max(cx) <= px(0.5),
        &format!(
            "visual-test {SECTION} ②: 英語の断りも上限の中に全文が収まる (#2760。はみ出し {:.1}px)",
            f32::from(scroll_max(cx))
        ),
    );

    // ③ とても長い日本語のファイル名: 同じ名前がある所へ移す ⌥⌘V を断らせる
    clear_notice(cx);
    let long_pt = row_point(cx, &at(&format!("folder/{long_name}")));
    tree_multiselect::press(any, window, cx, long_pt, MouseButton::Left, plain);
    vt1860_press(any, window, cx, &key("c"));
    let dst_pt = row_point(cx, &at("dst"));
    tree_multiselect::press(any, window, cx, dst_pt, MouseButton::Left, plain);
    let _ = window.update(cx, |app, _, cx| {
        app.filetree.expand_dir(&base.join("dst"));
        cx.notify();
    });
    vt1860_press(any, window, cx, move_key);
    let shown = vt1860_wait(any, window, cx, landed, &|cx| {
        measure(cx).1.is_some() && notice(cx).contains("同じ名前")
    })
    .await;
    let (_, rect) = report(cx, "③long");
    let text = notice(cx);
    println!("TAKO_VISUAL_PIXEL: {SECTION} ③long text={text:?}");
    dump(cx, "long");
    check(
        shown && text.contains(&long_name),
        &format!("visual-test {SECTION} ③: 長い名前の断り（同じ名前がある）が名前ごと出る (#2760。{text:?})"),
    );
    check(
        rect.is_some_and(|b| b.size.height <= px(NOTICE_MAX_H + 0.5)),
        &format!("visual-test {SECTION} ③: 長い断りも高さは上限で止まる (#2760)"),
    );
    let max = scroll_max(cx);
    check(
        max > px(0.5),
        &format!("visual-test {SECTION} ③: 上限を超えた分は通知欄の中でスクロールできる (#2760)"),
    );
    // 末尾まで送る（スクロールの量は負の向き）
    let _ = window.update(cx, |app, _, cx| {
        app.notice_scroll.set_offset(point(px(0.0), -max));
        cx.notify();
    });
    notify_and_draw(any, window, cx);
    let offset = window
        .update(cx, |app, _, _| app.notice_scroll.offset().y)
        .unwrap_or(px(0.0));
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ③long scrolled offset={:.1} max={:.1}",
        f32::from(offset),
        f32::from(max)
    );
    dump(cx, "long-end");
    check(
        (offset + max).abs() <= px(0.5),
        &format!(
            "visual-test {SECTION} ③: 通知欄を末尾まで送れる (#2760。offset {:.1} / max {:.1})",
            f32::from(offset),
            f32::from(max)
        ),
    );

    // ④ ルートの外のパス: ホーム（ルートでなければ `~`）とホームの外（そのまま）
    let home = tako_core::paths::home_dir();
    let outside = std::path::PathBuf::from("/opt/tako-vt2760-outside/y.txt");
    let in_home = home
        .as_ref()
        .map(|h| h.join("tako-vt2760-elsewhere.txt"))
        .unwrap_or_else(|| outside.clone());
    let reason = format!("{} / {}", in_home.display(), outside.display());
    let _ = window.update(cx, |app, _, cx| {
        app.remote_notice = None;
        app.notify_tree_op_failed(
            crate::ui_text::sidebar::menu_paste_move(),
            Some(&in_home.display().to_string()),
            &reason,
        );
        cx.notify();
    });
    notify_and_draw(any, window, cx);
    let text = notice(cx);
    println!("TAKO_VISUAL_PIXEL: {SECTION} ④outside text={text:?}");
    let home_text = home.as_ref().map(|h| h.display().to_string());
    check(
        text.contains(&outside.display().to_string())
            && text.contains("tako-vt2760-elsewhere.txt")
            && home_text.is_none_or(|h| !text.contains(&h)),
        &format!(
            "visual-test {SECTION} ④: ルートの外はホームを畳み、ホームの外はそのまま (#2760。{text:?})"
        ),
    );

    // 後片付け（一時 dir の中だけ）
    tako_core::i18n::set_lang(lang_before);
    let _ = window.update(cx, |app, _, cx| {
        app.remote_notice = None;
        app.tree_selection = None;
        app.file_clipboard = None;
        app.tree_row_probe = false;
        let tab = app.workspace.active_tab_id();
        if let Some(t) = app.workspace.get_tab_mut(tab) {
            t.remove_pinned_folder(&base);
        }
        cx.notify();
    });
    let _ = std::fs::remove_dir_all(&top);
}
