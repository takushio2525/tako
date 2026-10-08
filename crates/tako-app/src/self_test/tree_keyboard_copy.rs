//! visual-test `tree-keyboard-copy`（FR-3.39 / Issue #1895）: ファイルツリーのキーでの範囲選択・
//! ごみ箱と、大きな 1 ファイルのコピーの途中の進み具合・取り消し・残り時間の目安を**実マウス・
//! 実キーで**確かめる。
//!
//! ①⇧↓ / ⇧↑ で最後に押した行から 1 行ずつ伸びて逆へ打つと縮む（起点を越えると反対側へ）・
//! 先頭 / 末尾では何も変えない・畳んだフォルダは見出しの 1 行だけ（中は入らない）・伸ばした行に
//! 選んだ枠が出る・ファイルを開かない ②⌘⌫（Windows は Delete）で選んだもの全部がごみ箱へ・
//! 見出しのフォルダは理由を出して断る ③大きな 1 ファイルのコピー（`TAKO_1895_COPY_CHUNK_DELAY_MS`
//! が立っているときだけ）で、件数は 0 のまま**バイトが単調に増え**、帯の「取り消し」で作りかけを
//! 残さず止まり、直後にもう一度貼ると最後まで写る ④写し始めて 2 秒経つと帯の右端に残り時間の
//! 目安が出る（実矩形があり文字が描かれている）。
//!
//! **ユーザーのクリップボード・ゴミ箱は触らない**（`TAKO_FILE_PASTEBOARD` の名前付きペーストボード /
//! `TAKO_TRASH_DIR` = 一時 dir）。判定は新しい挙動を無条件に主張する。`TAKO_1895_LEGACY=1` は
//! ⇧↓ がツリーへ向かないので ① が FAILED になる = 同一バイナリでの A/B。
//! 単独実行は `TAKO_VISUAL_ONLY=tree-keyboard-copy`

use super::tree_multiselect::press;
use super::*;

const SECTION: &str = "tree-keyboard-copy";

/// ③④ の大きなファイル（MiB）。遅延の注入（1 MiB ごと）と合わせて数秒〜十数秒で写り終わる大きさ
/// （数 GB のファイル・CPU を焼く負荷で遅くしない）
const BIG_MIB: usize = 40;

pub(super) async fn tree_keyboard_copy_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    inject_section_failure(SECTION);
    // 一般のペーストボード（ユーザーのクリップボード）へ書かない（#1860 と同じ）
    if std::env::var("TAKO_FILE_PASTEBOARD")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true)
    {
        std::env::set_var(
            "TAKO_FILE_PASTEBOARD",
            format!("tako-vt1895-{}", std::process::id()),
        );
    }
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let mac = cfg!(target_os = "macos");
    let plain = Modifiers::default();
    let trash_key = if mac { "cmd-backspace" } else { "delete" };
    let delay = std::env::var("TAKO_1895_COPY_CHUNK_DELAY_MS").is_ok_and(|v| !v.trim().is_empty());

    // --- 場面: 一時 dir の中だけに fixture を作る（本物のファイルを書き換えない = #1811） ---
    let tmp = tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir());
    let raw = tmp.join(format!("tako-vt1895-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&raw);
    for dir in ["keys/sub", "trash-me", "big", "dst", "bin"] {
        std::fs::create_dir_all(raw.join(dir)).expect("visual-test tree-keyboard-copy fixture");
    }
    let base = tako_core::platform::path::canonicalize_or_self(&raw);
    if !base.starts_with(&tmp) || base == tmp {
        fail(&format!(
            "visual-test tree-keyboard-copy: fixture が一時 dir の外 ({})",
            base.display()
        ));
    }
    let at = |rel: &str| base.join(rel);
    for (rel, body) in [
        ("keys/a.txt", "A\n"),
        ("keys/b.txt", "B\n"),
        ("keys/c.txt", "C\n"),
        ("keys/d.txt", "D\n"),
        ("keys/sub/x.txt", "X\n"),
        ("trash-me/t1.txt", "1\n"),
        ("trash-me/t2.txt", "2\n"),
        ("trash-me/t3.txt", "3\n"),
    ] {
        std::fs::write(at(rel), body).expect("visual-test tree-keyboard-copy fixture");
    }
    if delay {
        // 中身のあるファイル（疎なファイルにしない）
        let mut body = vec![0u8; BIG_MIB << 20];
        for (i, b) in body.iter_mut().enumerate() {
            *b = (i.wrapping_mul(2_654_435_761) >> 13) as u8;
        }
        std::fs::write(at("big/big.bin"), body).expect("visual-test tree-keyboard-copy fixture");
    }
    // ごみ箱の代わり（ユーザーのゴミ箱へ fixture を入れない）
    std::env::set_var("TAKO_TRASH_DIR", at("bin"));
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
            for rel in ["keys", "trash-me", "big", "dst"] {
                app.filetree.expand_dir(&base.join(rel));
            }
            app.tree_row_probe = true;
            app.remote_notice = None;
            app.file_clipboard = None;
            app.tree_selection = None;
            cx.notify();
        })
        .unwrap_or_else(|_| fail("visual-test tree-keyboard-copy: 場面づくり"));

    // 観測の道具
    let row_rect = |cx: &mut AsyncApp, path: &std::path::Path| {
        vt1834_rows(any, window, cx)
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b)
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test tree-keyboard-copy: 行が描かれていない（{}）",
                    path.display()
                ))
            })
    };
    let row_point = |cx: &mut AsyncApp, path: &std::path::Path| {
        let rect = row_rect(cx, path);
        let height = window
            .update(cx, |_, w, _| w.viewport_size().height)
            .unwrap_or(px(0.0));
        if rect.bottom() > height {
            fail(&format!(
                "visual-test tree-keyboard-copy: 行が窓の外（{} の下端 {:?} > 窓 {:?}）",
                path.display(),
                rect.bottom(),
                height
            ));
        }
        vt1834_grab(rect)
    };
    let selected = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                app.active_tree_selection().map(|s| s.paths.clone())
            })
            .ok()
            .flatten()
    };
    let order = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                crate::sidebar::tree_visible_order(&app.filetree.rows())
            })
            .unwrap_or_default()
    };
    // 見えている並びの `from` から `to` まで（両端を含む・上から順）
    let span = |order: &[std::path::PathBuf], from: &std::path::Path, to: &std::path::Path| {
        let a = order.iter().position(|p| p == from).unwrap_or(0);
        let b = order.iter().position(|p| p == to).unwrap_or(0);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        order[lo..=hi].to_vec()
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
    };
    let previews = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.previews.len())
            .unwrap_or(0)
    };
    let menu_click = |cx: &mut AsyncApp, row: &std::path::Path, item: &str| {
        let pt = row_point(cx, row);
        press(any, window, cx, pt, MouseButton::Right, plain);
        let items = window
            .update(cx, |app, _, _| app.tree_menu_item_rects.borrow().clone())
            .unwrap_or_default();
        let Some((_, rect)) = items.iter().find(|(id, _)| *id == item) else {
            fail(&format!(
                "visual-test tree-keyboard-copy: メニューに {item} が無い"
            ));
        };
        press(any, window, cx, rect.center(), MouseButton::Left, plain);
    };
    let theme = window
        .update(cx, |app, _, _| app.theme.clone())
        .unwrap_or_else(|_| fail("visual-test tree-keyboard-copy: テーマ"));
    let frame = |cx: &mut AsyncApp| {
        capture_frame(any, cx).unwrap_or_else(|| fail("visual-test tree-keyboard-copy: フレーム"))
    };
    let count_in = |img: &image::RgbaImage, row: Bounds<Pixels>, scale: f32, color| {
        [false, true]
            .into_iter()
            .map(|flip| color_pixels_in_bounds(img, row, scale, color, 14, flip))
            .max()
            .unwrap_or(0)
    };
    let landed = Duration::from_secs(10);
    let collapse = |cx: &mut AsyncApp, rel: &str| {
        let _ = window.update(cx, |app, _, cx| {
            let path = base.join(rel);
            if app
                .filetree
                .rows()
                .iter()
                .any(|r| r.entry.path == path && r.expanded)
            {
                app.filetree.toggle_dir(&path);
            }
            cx.notify();
        });
        notify_and_draw(any, window, cx);
    };

    // ① ⇧↓ / ⇧↑ で範囲を伸ばす・縮める（`keys/sub` は畳んだまま = 見出しの 1 行だけ）
    collapse(cx, "keys/sub");
    let a_pt = row_point(cx, &at("keys/a.txt"));
    press(any, window, cx, a_pt, MouseButton::Left, plain);
    check(
        selected(cx) == Some(vec![at("keys/a.txt")]),
        &format!(
            "visual-test tree-keyboard-copy ①: 素の押下でその行だけ (#1895。{:?})",
            selected(cx)
        ),
    );
    let opened = previews(cx);
    let o = order(cx);
    let below = o
        .iter()
        .position(|p| *p == at("keys/a.txt"))
        .and_then(|i| o.get(i + 1).cloned())
        .unwrap_or_else(|| fail("visual-test tree-keyboard-copy ①: a.txt の下に行が無い"));
    let b_rect = row_rect(cx, &below);
    let (before, scale) = frame(cx);
    vt1860_press(any, window, cx, "shift-down");
    check(
        selected(cx) == Some(span(&o, &at("keys/a.txt"), &below)),
        &format!(
            "visual-test tree-keyboard-copy ①: ⇧↓ で 1 行下へ伸びる (#1895。{:?})",
            selected(cx)
        ),
    );
    let (after, _) = frame(cx);
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let _ = after.save(std::path::Path::new(&dump).join("tree-keyboard-copy-1-shift-down.png"));
    }
    let gain = count_in(&after, b_rect, scale, theme.accent).saturating_sub(count_in(
        &before,
        b_rect,
        scale,
        theme.accent,
    ));
    println!("TAKO_VISUAL_PIXEL: {SECTION} ① extended accent_gain={gain} th=120");
    check(
        gain >= 120,
        &format!(
            "visual-test tree-keyboard-copy ①: 伸ばした行に選んだ枠が出る (#1895。gain={gain})"
        ),
    );
    vt1860_press(any, window, cx, "shift-down");
    let two_below = o
        .iter()
        .position(|p| *p == below)
        .and_then(|i| o.get(i + 1).cloned())
        .unwrap_or_else(|| fail("visual-test tree-keyboard-copy ①: 2 行下が無い"));
    check(
        selected(cx) == Some(span(&o, &at("keys/a.txt"), &two_below)),
        &format!(
            "visual-test tree-keyboard-copy ①: 続けて ⇧↓ でさらに伸びる (#1895。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "shift-up");
    vt1860_press(any, window, cx, "shift-up");
    check(
        selected(cx) == Some(vec![at("keys/a.txt")]),
        &format!(
            "visual-test tree-keyboard-copy ①: ⇧↑ で縮み起点まで戻る (#1895。{:?})",
            selected(cx)
        ),
    );
    // 起点を越えると上へ伸びる。a.txt の上は畳んだ keys/sub（フォルダが先に並ぶ）= 見出しだけ
    vt1860_press(any, window, cx, "shift-up");
    let sel = selected(cx).unwrap_or_default();
    let above = o
        .iter()
        .position(|p| *p == at("keys/a.txt"))
        .and_then(|i| i.checked_sub(1))
        .map(|i| o[i].clone())
        .unwrap_or_else(|| fail("visual-test tree-keyboard-copy ①: a.txt の上に行が無い"));
    check(
        sel == span(&o, &above, &at("keys/a.txt")),
        &format!("visual-test tree-keyboard-copy ①: 起点を越えると上へ伸びる (#1895。{sel:?})"),
    );
    check(
        !sel.contains(&at("keys/sub/x.txt")),
        &format!(
            "visual-test tree-keyboard-copy ①: 畳んだフォルダの中は範囲に入らない (#1895。{sel:?})"
        ),
    );
    check(
        previews(cx) == opened,
        "visual-test tree-keyboard-copy ①: ⇧↑ / ⇧↓ はファイルを開かない (#1895)",
    );
    // 先頭（見出し）で ⇧↑・末尾で ⇧↓ は何も変えない（右クリックで選ぶ = 開閉しない）
    let first = o
        .first()
        .cloned()
        .unwrap_or_else(|| fail("visual-test tree-keyboard-copy ①: 行が無い"));
    let first_pt = row_point(cx, &first);
    press(any, window, cx, first_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "shift-up");
    check(
        selected(cx) == Some(vec![first.clone()]),
        &format!(
            "visual-test tree-keyboard-copy ①: 先頭で ⇧↑ は何も変えない (#1895。{:?})",
            selected(cx)
        ),
    );
    check(
        window
            .update(cx, |app, _, _| app.context_menu.is_none())
            .unwrap_or(false),
        "visual-test tree-keyboard-copy ①: キーで操作したら右クリックメニューは畳む (#1895)",
    );
    let last = o
        .last()
        .cloned()
        .unwrap_or_else(|| fail("visual-test tree-keyboard-copy ①: 行が無い"));
    let last_pt = row_point(cx, &last);
    press(any, window, cx, last_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "shift-down");
    check(
        selected(cx) == Some(vec![last.clone()]),
        &format!(
            "visual-test tree-keyboard-copy ①: 末尾で ⇧↓ は何も変えない (#1895。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "escape");

    // ② ⌘⌫（Windows は Delete）で選んだもの全部をごみ箱へ・見出しは断る
    clear_notice(cx);
    let t1_pt = row_point(cx, &at("trash-me/t1.txt"));
    press(any, window, cx, t1_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "shift-down");
    check(
        selected(cx) == Some(vec![at("trash-me/t1.txt"), at("trash-me/t2.txt")]),
        &format!(
            "visual-test tree-keyboard-copy ②: 右クリックで選んで ⇧↓ で 2 件 (#1895。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, trash_key);
    let ok = vt1860_wait(any, window, cx, landed, &|_| {
        at("bin/t1.txt").is_file() && at("bin/t2.txt").is_file()
    })
    .await;
    check(
        ok && !at("trash-me/t1.txt").exists() && !at("trash-me/t2.txt").exists(),
        &format!(
            "visual-test tree-keyboard-copy ②: {trash_key} で 2 件まとめてごみ箱へ (#1895。{:?})",
            notice(cx)
        ),
    );
    check(
        at("trash-me/t3.txt").is_file(),
        "visual-test tree-keyboard-copy ②: 選んでいないものは残る (#1895)",
    );
    check(
        selected(cx).is_none(),
        "visual-test tree-keyboard-copy ②: ごみ箱へ入れたら選択は外れる (#1895)",
    );
    // 選んでいなければ ⌘⌫ はツリーへ向かない（何も入らない）
    vt1860_press(any, window, cx, trash_key);
    check(
        at("trash-me/t3.txt").is_file(),
        "visual-test tree-keyboard-copy ②: 選んでいないときの打鍵はツリーへ向かない (#1895)",
    );
    // 見出し（ワークスペースのフォルダ）は理由を出して断る
    press(any, window, cx, first_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, trash_key);
    check(
        base.is_dir() && notice(cx).contains(crate::ui_text::sidebar::trash_root_refused()),
        &format!(
            "visual-test tree-keyboard-copy ②: 見出しのフォルダは理由を出して断る (#1895。{:?})",
            notice(cx)
        ),
    );
    // 1 件だけ
    clear_notice(cx);
    let t3_pt = row_point(cx, &at("trash-me/t3.txt"));
    press(any, window, cx, t3_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, trash_key);
    let ok = vt1860_wait(any, window, cx, landed, &|_| at("bin/t3.txt").is_file()).await;
    check(
        ok && !at("trash-me/t3.txt").exists(),
        &format!(
            "visual-test tree-keyboard-copy ②: 1 件でも {trash_key} でごみ箱へ (#1895。{:?})",
            notice(cx)
        ),
    );

    // ③④ 大きな 1 ファイルのコピー: 途中のバイト・取り消し・再開・残り時間
    if delay {
        clear_notice(cx);
        collapse(cx, "keys");
        collapse(cx, "trash-me");
        let total = (BIG_MIB as u64) << 20;
        menu_click(cx, &at("big/big.bin"), "clip-copy");
        menu_click(cx, &at("dst"), "clip-paste");
        let probe = std::path::PathBuf::from(crate::sidebar::COPY_CANCEL_PROBE);
        let eta_probe = std::path::PathBuf::from(crate::sidebar::COPY_ETA_PROBE);
        let snap = || {
            tako_core::file_copy::jobs()
                .list()
                .first()
                .map(|t| t.progress.snapshot())
        };
        // 件数は 0 のまま（= 1 つのファイルの途中）でバイトが進む
        let midway = vt1860_wait(any, window, cx, landed, &|cx| {
            snap().is_some_and(|s| !s.counting && s.bytes_done > 0 && s.entries_done == 0)
                && vt1834_rows(any, window, cx)
                    .iter()
                    .any(|(p, _)| *p == probe)
        })
        .await;
        check(
            midway,
            &format!(
                "visual-test tree-keyboard-copy ③: 1 つのファイルの途中でバイトが進む (#1895。{:?})",
                snap()
            ),
        );
        // 単調に増える（減らない・止まったまま帯が出続けない）
        let mut seen: Vec<u64> = Vec::new();
        let sampling = std::time::Instant::now();
        while sampling.elapsed() < Duration::from_millis(1500) {
            notify_and_draw(any, window, cx);
            if let Some(s) = snap() {
                seen.push(s.bytes_done);
            }
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
        let rises = seen.windows(2).filter(|w| w[1] > w[0]).count();
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ③ bytes samples={} rises={rises} first={:?} last={:?} total={total}",
            seen.len(),
            seen.first(),
            seen.last()
        );
        check(
            seen.windows(2).all(|w| w[0] <= w[1]),
            &format!(
                "visual-test tree-keyboard-copy ③: バイトの進み具合が減った (#1895。{:?})",
                seen.windows(2).find(|w| w[0] > w[1])
            ),
        );
        check(
            rises >= 3 && seen.last().is_some_and(|b| *b < total),
            &format!(
                "visual-test tree-keyboard-copy ③: バイトが途中で何度も進む (#1895。rises={rises} last={:?})",
                seen.last()
            ),
        );
        // ④ 写し始めて 2 秒経つと帯の右端に残り時間の目安
        let shown = vt1860_wait(any, window, cx, landed, &|cx| {
            vt1834_rows(any, window, cx)
                .iter()
                .any(|(p, _)| *p == eta_probe)
        })
        .await;
        let s = snap();
        let eta = s.and_then(|s| tako_core::file_copy::eta(&s));
        check(
            shown && eta.is_some(),
            &format!(
                "visual-test tree-keyboard-copy ④: 帯に残り時間の目安が出る (#1895。{s:?} eta={eta:?})"
            ),
        );
        let eta_rect = row_rect(cx, &eta_probe);
        let cancel_rect = row_rect(cx, &probe);
        let (img, scale) = frame(cx);
        if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
            let _ = img.save(std::path::Path::new(&dump).join("tree-keyboard-copy-4-eta.png"));
        }
        let ink = count_in(&img, eta_rect, scale, theme.text_muted);
        let width = window
            .update(cx, |_, w, _| w.viewport_size().width)
            .unwrap_or(px(0.0));
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ④ eta rect={eta_rect:?} ink={ink} th=15 label={:?} copying_for={:?}",
            eta.map(tako_core::file_copy::eta_label),
            s.map(|s| s.copying_for)
        );
        check(
            ink >= 15 && eta_rect.size.width > px(10.0) && eta_rect.right() <= width,
            &format!(
                "visual-test tree-keyboard-copy ④: 残り時間の文字が窓の中に描かれる (#1895。ink={ink} rect={eta_rect:?})"
            ),
        );
        check(
            eta_rect.top() >= cancel_rect.bottom(),
            "visual-test tree-keyboard-copy ④: 残り時間は棒の行（帯の 2 行目）にある (#1895)",
        );
        // 帯の「取り消し」で止まり、作りかけを残さない
        press(
            any,
            window,
            cx,
            cancel_rect.center(),
            MouseButton::Left,
            plain,
        );
        let stopped = vt1860_wait(any, window, cx, landed, &|_| {
            tako_core::file_copy::jobs().list().is_empty()
        })
        .await;
        check(
            stopped,
            "visual-test tree-keyboard-copy ③: 帯の「取り消し」で 1 つのファイルの途中でも止まる (#1895)",
        );
        let _ = vt1860_wait(any, window, cx, Duration::from_secs(3), &|cx| {
            !notice(cx).is_empty()
        })
        .await;
        check(
            !at("dst/big.bin").exists(),
            "visual-test tree-keyboard-copy ③: 取り消したら作りかけのファイルを残さない (#1895)",
        );
        check(
            notice(cx) == crate::ui_text::sidebar::copy_cancelled(),
            &format!(
                "visual-test tree-keyboard-copy ③: 取り消しは失敗ではなく知らせとして出る (#1895。{:?})",
                notice(cx)
            ),
        );
        check(
            std::fs::metadata(at("big/big.bin")).map(|m| m.len()).ok() == Some(total),
            "visual-test tree-keyboard-copy ③: コピー元は欠けない (#1895)",
        );
        // 取り消しの直後にもう一度貼ると最後まで写る（作りかけが名前を塞いでいない）
        clear_notice(cx);
        menu_click(cx, &at("dst"), "clip-paste");
        let again = vt1860_wait(any, window, cx, Duration::from_secs(40), &|_| {
            tako_core::file_copy::jobs().list().is_empty()
                && std::fs::metadata(at("dst/big.bin")).map(|m| m.len()).ok() == Some(total)
        })
        .await;
        check(
            again,
            &format!(
                "visual-test tree-keyboard-copy ③: 取り消しの直後にもう一度貼ると同じ名前で最後まで写る (#1895。{:?})",
                notice(cx)
            ),
        );
        check(
            std::fs::read(at("dst/big.bin")).ok() == std::fs::read(at("big/big.bin")).ok(),
            "visual-test tree-keyboard-copy ③: 写した中身がコピー元と同じ (#1895)",
        );
    } else {
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ③④ skipped（TAKO_1895_COPY_CHUNK_DELAY_MS 未設定 = 途中の進み具合は scripts/test-tree-keyboard-copy-1895.sh で見る）"
        );
    }

    // 後片付け（一時 dir の中だけ）
    std::env::remove_var("TAKO_TRASH_DIR");
    let _ = window.update(cx, |app, _, cx| {
        app.tree_selection = None;
        app.file_clipboard = None;
        app.tree_row_probe = false;
        cx.notify();
    });
    let _ = std::fs::remove_dir_all(&base);
}
