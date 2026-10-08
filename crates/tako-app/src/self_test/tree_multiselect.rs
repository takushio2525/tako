//! visual-test `tree-multiselect`（FR-3.38 / Issue #1867）: ファイルツリーの複数選択・
//! まとめた操作・⌥⌘V・コピーの進み具合と取り消しを**実マウス・実キーで**確かめる。
//!
//! ①⌘クリック（Windows は Ctrl）で足す / 外す・最後の 1 行を外すと選択が無くなり ⌘C が
//! ペインへ戻る ②⇧クリックで範囲（開かない）③まとめて ⌘C → ⌘V（貼ったもの全部を選び直す）
//! ④選んだ行を掴むとまとめて運ぶ（D&D）⑤フォルダとその配下を同時に選んだら親だけを扱う
//! ⑥⌥⌘V = 移動として貼る（配下へは理由つきで断る・貼り付け先が選択に含まれるときは複製）
//! ⑦右クリックのごみ箱がまとめて効く（`TAKO_TRASH_DIR` = ユーザーのゴミ箱へ入れない）
//! ⑧大きなフォルダのコピーで帯に進み具合が出て、帯の「取り消し」で作りかけを残さず止まる
//! （`TAKO_1867_COPY_DELAY_MS` が立っているときだけ。小さな fixture を遅くして見る）。
//!
//! **ユーザーのクリップボードは触らない**（`TAKO_FILE_PASTEBOARD` の名前付きペーストボード）。
//! 判定は新しい挙動を無条件に主張する。`TAKO_1867_LEGACY=1` は ⌘クリックが素の押下になるので
//! ① が FAILED になる = 同一バイナリでの A/B。単独実行は `TAKO_VISUAL_ONLY=tree-multiselect`

use super::*;

const SECTION: &str = "tree-multiselect";

/// 修飾つきの押下を**実 OS マウスと同じ `PlatformInput` 経路**で流す（#1860 の
/// `vt1860_click` の修飾つき版。押下の捕捉フェーズ → 行の押下 → クリックの順に配送される）
pub(super) fn press(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    at: Point<Pixels>,
    button: MouseButton,
    modifiers: Modifiers,
) {
    let send = |cx: &mut AsyncApp, input: gpui::PlatformInput| {
        let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
    };
    send(
        cx,
        gpui::PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers,
        }),
    );
    send(
        cx,
        gpui::PlatformInput::MouseDown(MouseDownEvent {
            button,
            position: at,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
    );
    send(
        cx,
        gpui::PlatformInput::MouseUp(MouseUpEvent {
            button,
            position: at,
            modifiers,
            click_count: 1,
        }),
    );
    notify_and_draw(any, window, cx);
}

pub(super) async fn tree_multiselect_visual(
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
            format!("tako-vt1867-{}", std::process::id()),
        );
    }
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let mac = cfg!(target_os = "macos");
    let plain = Modifiers::default();
    let primary = if mac {
        Modifiers {
            platform: true,
            ..Modifiers::default()
        }
    } else {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    };
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let key = |letter: &str| {
        if mac {
            format!("cmd-{letter}")
        } else {
            format!("ctrl-{letter}")
        }
    };
    let move_key = if mac { "cmd-alt-v" } else { "ctrl-alt-v" };

    // --- 場面: 一時 dir の中だけに fixture を作る（本物のファイルを書き換えない = #1811） ---
    let tmp = tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir());
    let raw = tmp.join(format!("tako-vt1867-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&raw);
    for dir in ["src", "folder/inner", "dst", "big", "trash-me", "bin"] {
        std::fs::create_dir_all(raw.join(dir)).expect("visual-test tree-multiselect fixture");
    }
    let base = tako_core::platform::path::canonicalize_or_self(&raw);
    if !base.starts_with(&tmp) || base == tmp {
        fail(&format!(
            "visual-test tree-multiselect: fixture が一時 dir の外 ({})",
            base.display()
        ));
    }
    let at = |rel: &str| base.join(rel);
    for (rel, body) in [
        ("src/a.txt", "A\n"),
        ("src/b.txt", "B\n"),
        ("src/c.txt", "C\n"),
        ("folder/inner/x.txt", "X\n"),
        ("trash-me/t1.txt", "1\n"),
        ("trash-me/t2.txt", "2\n"),
    ] {
        std::fs::write(at(rel), body).expect("visual-test tree-multiselect fixture");
    }
    const BIG: usize = 30;
    for i in 0..BIG {
        std::fs::write(at(&format!("big/f{i:02}.txt")), "0123456789".repeat(10))
            .expect("visual-test tree-multiselect fixture");
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
            for rel in ["src", "folder", "folder/inner", "dst", "trash-me"] {
                app.filetree.expand_dir(&base.join(rel));
            }
            app.tree_row_probe = true;
            app.remote_notice = None;
            app.file_clipboard = None;
            app.tree_selection = None;
            cx.notify();
        })
        .unwrap_or_else(|_| fail("visual-test tree-multiselect: 場面づくり"));

    // 観測の道具
    let row_point = |cx: &mut AsyncApp, path: &std::path::Path| {
        let rect = vt1834_rows(any, window, cx)
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b)
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test tree-multiselect: 行が描かれていない（{}）",
                    path.display()
                ))
            });
        // 行の矩形はスクロールの外でも採れる = 押しても届かない（場面の行が多すぎる）
        let height = window
            .update(cx, |_, w, _| w.viewport_size().height)
            .unwrap_or(px(0.0));
        if rect.bottom() > height {
            fail(&format!(
                "visual-test tree-multiselect: 行が窓の外（{} の下端 {:?} > 窓 {:?}。場面のフォルダを畳むこと）",
                path.display(),
                rect.bottom(),
                height
            ));
        }
        (vt1834_grab(rect), rect)
    };
    // 関係ないフォルダを畳む（行を窓の中へ収める）
    let collapse_except = |cx: &mut AsyncApp, keep: &[&str]| {
        let _ = window.update(cx, |app, _, cx| {
            for r in app.filetree.rows() {
                let kept = keep.iter().any(|k| r.entry.path == base.join(k));
                if !r.root && r.remote.is_none() && r.expanded && !kept {
                    app.filetree.toggle_dir(&r.entry.path);
                }
            }
            cx.notify();
        });
        notify_and_draw(any, window, cx);
    };
    let selected = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                app.active_tree_selection().map(|s| s.paths.clone())
            })
            .ok()
            .flatten()
    };
    let clip = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.file_clipboard.clone())
            .ok()
            .flatten()
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
    let expand = |cx: &mut AsyncApp, rel: &str| {
        let _ = window.update(cx, |app, _, cx| {
            app.filetree.expand_dir(&base.join(rel));
            cx.notify();
        });
        notify_and_draw(any, window, cx);
    };
    let theme = window
        .update(cx, |app, _, _| app.theme.clone())
        .unwrap_or_else(|_| fail("visual-test tree-multiselect: テーマ"));
    let frame = |cx: &mut AsyncApp| {
        capture_frame(any, cx).unwrap_or_else(|| fail("visual-test tree-multiselect: フレーム"))
    };
    let count_in = |img: &image::RgbaImage, row: Bounds<Pixels>, scale: f32, color| {
        [false, true]
            .into_iter()
            .map(|flip| color_pixels_in_bounds(img, row, scale, color, 14, flip))
            .max()
            .unwrap_or(0)
    };
    let landed = Duration::from_secs(10);
    let paths = |rels: &[&str]| rels.iter().map(|r| at(r)).collect::<Vec<_>>();

    // ① ⌘クリックで足す / 外す
    let (a_pt, _) = row_point(cx, &at("src/a.txt"));
    press(any, window, cx, a_pt, MouseButton::Left, plain);
    check(
        selected(cx) == Some(paths(&["src/a.txt"])),
        &format!(
            "visual-test tree-multiselect ①: 素の押下でその行だけ (#1867。{:?})",
            selected(cx)
        ),
    );
    let opened = previews(cx);
    let (b_pt, b_row) = row_point(cx, &at("src/b.txt"));
    let (before, scale) = frame(cx);
    press(any, window, cx, b_pt, MouseButton::Left, primary);
    check(
        selected(cx) == Some(paths(&["src/a.txt", "src/b.txt"])),
        &format!(
            "visual-test tree-multiselect ①: ⌘クリックで 2 行目を足せる (#1867。{:?})",
            selected(cx)
        ),
    );
    let (after, _) = frame(cx);
    let gain = count_in(&after, b_row, scale, theme.accent).saturating_sub(count_in(
        &before,
        b_row,
        scale,
        theme.accent,
    ));
    println!("TAKO_VISUAL_PIXEL: {SECTION} ① added accent_gain={gain} th=120");
    check(
        gain >= 120,
        &format!("visual-test tree-multiselect ①: 足した行に選んだ枠が出る (#1867。gain={gain})"),
    );
    check(
        previews(cx) == opened,
        "visual-test tree-multiselect ①: ⌘クリックはファイルを開かない（選ぶだけ）(#1867)",
    );
    press(any, window, cx, b_pt, MouseButton::Left, primary);
    check(
        selected(cx) == Some(paths(&["src/a.txt"])),
        "visual-test tree-multiselect ①: もう一度 ⌘クリックで外れる (#1867)",
    );
    let (a_pt, _) = row_point(cx, &at("src/a.txt"));
    press(any, window, cx, a_pt, MouseButton::Left, primary);
    check(
        selected(cx).is_none(),
        "visual-test tree-multiselect ①: 最後の 1 行を外すと何も選んでいない (#1867)",
    );
    vt1860_press(any, window, cx, &key("c"));
    check(
        clip(cx).is_none(),
        "visual-test tree-multiselect ①: 0 件選択の ⌘C はツリーへ向かない (#1867)",
    );

    // ② ⇧クリックで範囲（開かない）
    press(any, window, cx, a_pt, MouseButton::Left, plain);
    let opened = previews(cx);
    let (c_pt, _) = row_point(cx, &at("src/c.txt"));
    press(any, window, cx, c_pt, MouseButton::Left, shift);
    check(
        selected(cx) == Some(paths(&["src/a.txt", "src/b.txt", "src/c.txt"])),
        &format!(
            "visual-test tree-multiselect ②: ⇧クリックで起点からの範囲 (#1867。{:?})",
            selected(cx)
        ),
    );
    check(
        previews(cx) == opened,
        "visual-test tree-multiselect ②: ⇧クリックはファイルを開かない (#1867)",
    );

    // ③ まとめて ⌘C → ⌘V（貼ったもの全部を選び直す）
    vt1860_press(any, window, cx, &key("c"));
    check(
        clip(cx).is_some_and(|c| {
            c.mode == tako_core::file_clipboard::ClipMode::Copy
                && c.paths == paths(&["src/a.txt", "src/b.txt", "src/c.txt"])
        }),
        &format!(
            "visual-test tree-multiselect ③: ⌘C で 3 件まとめて載る (#1867。{:?})",
            clip(cx)
        ),
    );
    let (dst_pt, _) = row_point(cx, &at("dst"));
    press(any, window, cx, dst_pt, MouseButton::Left, plain);
    expand(cx, "dst");
    vt1860_press(any, window, cx, &key("v"));
    let ok = vt1860_wait(any, window, cx, landed, &|_| {
        ["dst/a.txt", "dst/b.txt", "dst/c.txt"]
            .iter()
            .all(|r| at(r).is_file())
    })
    .await;
    check(
        ok,
        "visual-test tree-multiselect ③: ⌘V で 3 件まとめて複製される (#1867)",
    );
    let ok = vt1860_wait(any, window, cx, landed, &|cx| {
        selected(cx) == Some(paths(&["dst/a.txt", "dst/b.txt", "dst/c.txt"]))
    })
    .await;
    check(
        ok,
        &format!(
            "visual-test tree-multiselect ③: 貼ったもの全部を選び直す (#1867。{:?})",
            selected(cx)
        ),
    );

    // ④ 選んだ行を掴むとまとめて運ぶ（D&D）
    let (grab, _) = row_point(cx, &at("dst/b.txt"));
    let (folder_pt, _) = row_point(cx, &at("folder"));
    let drag = vt1834_drag(any, window, cx, grab, folder_pt);
    check(
        drag.dragging
            && drag.hover.as_ref().is_some_and(|h| {
                h.dest == at("folder") && h.verdict == tako_core::file_move::DropVerdict::Move
            }),
        &format!(
            "visual-test tree-multiselect ④: 選んだ行を掴んでフォルダの行へ運べる (#1867。{:?})",
            drag.hover
        ),
    );
    let ok = vt1860_wait(any, window, cx, landed, &|_| {
        ["folder/a.txt", "folder/b.txt", "folder/c.txt"]
            .iter()
            .all(|r| at(r).is_file())
            && !at("dst/a.txt").exists()
    })
    .await;
    check(
        ok,
        "visual-test tree-multiselect ④: 選んだ 3 件がまとめて移る (#1867)",
    );
    check(
        selected(cx) == Some(paths(&["folder/a.txt", "folder/b.txt", "folder/c.txt"])),
        &format!(
            "visual-test tree-multiselect ④: 移した先を選び直す (#1867。{:?})",
            selected(cx)
        ),
    );

    // ⑤ フォルダとその配下を同時に選んだら親だけを扱う
    let (inner_pt, _) = row_point(cx, &at("folder/inner"));
    press(any, window, cx, inner_pt, MouseButton::Left, plain);
    expand(cx, "folder/inner");
    let (x_pt, _) = row_point(cx, &at("folder/inner/x.txt"));
    press(any, window, cx, x_pt, MouseButton::Left, primary);
    check(
        selected(cx) == Some(paths(&["folder/inner", "folder/inner/x.txt"])),
        &format!(
            "visual-test tree-multiselect ⑤: フォルダと配下を同時に選べる (#1867。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, &key("c"));
    check(
        clip(cx).is_some_and(|c| c.paths == paths(&["folder/inner"])),
        &format!(
            "visual-test tree-multiselect ⑤: 配下は親と一緒に扱う（クリップボードは親だけ）(#1867。{:?})",
            clip(cx)
        ),
    );

    // ⑥ ⌥⌘V = 移動として貼る
    // 自分自身の中へは理由つきで断る（コピーしたまま = 別の場所へ貼り直せる）。
    // x.txt の行で貼る = その親（folder/inner）へ = コピーした folder/inner 自身
    clear_notice(cx);
    let (x_pt, _) = row_point(cx, &at("folder/inner/x.txt"));
    press(any, window, cx, x_pt, MouseButton::Left, plain);
    vt1860_press(any, window, cx, move_key);
    check(
        notice(cx).contains("自分自身"),
        &format!(
            "visual-test tree-multiselect ⑥: 自分自身へは移動として貼れない理由が出る (#1867。{:?})",
            notice(cx)
        ),
    );
    check(
        at("folder/inner/x.txt").is_file() && clip(cx).is_some(),
        "visual-test tree-multiselect ⑥: 断ったら何も動かずクリップボードは残る (#1867)",
    );
    // dst へ移動として貼る
    let (dst_pt, _) = row_point(cx, &at("dst"));
    press(any, window, cx, dst_pt, MouseButton::Left, plain);
    expand(cx, "dst");
    vt1860_press(any, window, cx, move_key);
    let ok = vt1860_wait(any, window, cx, landed, &|_| {
        at("dst/inner/x.txt").is_file() && !at("folder/inner").exists()
    })
    .await;
    check(
        ok,
        &format!(
            "visual-test tree-multiselect ⑥: ⌥⌘V でコピーしたものが移る (#1867。{:?})",
            notice(cx)
        ),
    );
    check(
        clip(cx).is_none(),
        "visual-test tree-multiselect ⑥: 移し終えたらクリップボードは空 (#1867)",
    );
    // 貼り付け先が選択に含まれるとき: dst と folder を選んで ⌘C → dst の行で ⌘V =
    // dst は自分の親へ複製・folder は dst の中へ
    let (folder_pt, _) = row_point(cx, &at("folder"));
    press(any, window, cx, folder_pt, MouseButton::Left, plain);
    expand(cx, "folder");
    let (dst_pt, _) = row_point(cx, &at("dst"));
    press(any, window, cx, dst_pt, MouseButton::Left, primary);
    vt1860_press(any, window, cx, &key("c"));
    check(
        clip(cx).is_some_and(|c| c.paths.len() == 2),
        &format!(
            "visual-test tree-multiselect ⑥: dst と folder の 2 件が載る (#1867。{:?})",
            clip(cx)
        ),
    );
    press(any, window, cx, dst_pt, MouseButton::Left, plain);
    expand(cx, "dst");
    vt1860_press(any, window, cx, &key("v"));
    let naming = tako_core::file_copy::CopyNaming::current();
    let dup = at(&tako_core::file_copy::copy_name("dst", true, 1, naming));
    let ok = vt1860_wait(any, window, cx, landed, &|_| {
        dup.join("inner/x.txt").is_file() && at("dst/folder/a.txt").is_file()
    })
    .await;
    check(
        ok,
        &format!(
            "visual-test tree-multiselect ⑥: 貼り付け先自身は親へ複製・ほかはその中へ (#1867。{})",
            dup.display()
        ),
    );
    // 着地（UI スレッドの続き）で貼ったもの全部が選び直されるまで待つ
    // （ファイルができた時点ではまだ着地していない = 次の押下の選択を後から上書きされる）
    let ok = vt1860_wait(any, window, cx, landed, &|cx| {
        selected(cx).is_some_and(|s| s.contains(&dup) && s.contains(&at("dst/folder")))
    })
    .await;
    check(
        ok,
        &format!(
            "visual-test tree-multiselect ⑥: 貼ったもの全部を選び直す (#1867。{:?})",
            selected(cx)
        ),
    );

    // ⑦ 右クリックのごみ箱がまとめて効く
    collapse_except(cx, &["trash-me"]);
    let (t1_pt, _) = row_point(cx, &at("trash-me/t1.txt"));
    press(any, window, cx, t1_pt, MouseButton::Left, plain);
    let (t2_pt, _) = row_point(cx, &at("trash-me/t2.txt"));
    press(any, window, cx, t2_pt, MouseButton::Left, primary);
    let (t1_pt, _) = row_point(cx, &at("trash-me/t1.txt"));
    press(any, window, cx, t1_pt, MouseButton::Right, plain);
    let (menu_open, kept, items) = window
        .update(cx, |app, _, _| {
            (
                app.context_menu.is_some(),
                app.active_tree_selection().map(|s| s.paths.clone()),
                app.tree_menu_item_rects.borrow().clone(),
            )
        })
        .unwrap_or((false, None, Vec::new()));
    check(
        menu_open && kept == Some(paths(&["trash-me/t1.txt", "trash-me/t2.txt"])),
        &format!(
            "visual-test tree-multiselect ⑦: 選んだ行の右クリックは選択を保つ (#1867。{kept:?})"
        ),
    );
    let Some((_, trash_rect)) = items.iter().find(|(id, _)| *id == "trash") else {
        fail("visual-test tree-multiselect ⑦: メニューにごみ箱が無い");
    };
    press(
        any,
        window,
        cx,
        trash_rect.center(),
        MouseButton::Left,
        plain,
    );
    let ok = vt1860_wait(any, window, cx, landed, &|_| {
        at("bin/t1.txt").is_file() && at("bin/t2.txt").is_file()
    })
    .await;
    check(
        ok && !at("trash-me/t1.txt").exists() && !at("trash-me/t2.txt").exists(),
        &format!(
            "visual-test tree-multiselect ⑦: 2 件まとめてごみ箱へ (#1867。{:?})",
            notice(cx)
        ),
    );

    // ⑧ 大きなフォルダのコピーの進み具合と取り消し
    if std::env::var("TAKO_1867_COPY_DELAY_MS").is_ok_and(|v| !v.trim().is_empty()) {
        clear_notice(cx);
        collapse_except(cx, &[]);
        // 右クリックで「コピー」（素の押下はフォルダを開いて行が増える）
        let (big_pt, _) = row_point(cx, &at("big"));
        press(any, window, cx, big_pt, MouseButton::Right, plain);
        let items = window
            .update(cx, |app, _, _| app.tree_menu_item_rects.borrow().clone())
            .unwrap_or_default();
        let Some((_, copy_rect)) = items.iter().find(|(id, _)| *id == "clip-copy") else {
            fail("visual-test tree-multiselect ⑧: メニューにコピーが無い");
        };
        press(
            any,
            window,
            cx,
            copy_rect.center(),
            MouseButton::Left,
            plain,
        );
        let (src_pt, _) = row_point(cx, &at("src"));
        press(any, window, cx, src_pt, MouseButton::Left, plain);
        expand(cx, "src");
        vt1860_press(any, window, cx, &key("v"));
        // 帯が出て、件数が途中まで進んだところを撮る
        let probe = std::path::PathBuf::from(crate::sidebar::COPY_CANCEL_PROBE);
        let midway = vt1860_wait(any, window, cx, landed, &|cx| {
            let snap = tako_core::file_copy::jobs()
                .list()
                .first()
                .map(|t| t.progress.snapshot());
            snap.is_some_and(|s| !s.counting && s.entries_done > 2)
                && vt1834_rows(any, window, cx)
                    .iter()
                    .any(|(p, _)| *p == probe)
        })
        .await;
        let snap = tako_core::file_copy::jobs()
            .list()
            .first()
            .map(|t| t.progress.snapshot());
        check(
            midway,
            &format!(
                "visual-test tree-multiselect ⑧: 大きなコピーで帯が出て件数が進む (#1867。{snap:?})"
            ),
        );
        let cancel_rect = vt1834_rows(any, window, cx)
            .into_iter()
            .find(|(p, _)| *p == probe)
            .map(|(_, b)| b)
            .unwrap_or_else(|| fail("visual-test tree-multiselect ⑧: 取り消しが描かれていない"));
        // 帯の棒（accent）が描かれている = 進み具合が見える
        let (img, scale) = frame(cx);
        let band = Bounds::new(
            point(px(0.0), cancel_rect.top()),
            // #1895 で棒の行が残り時間の枠ぶん高くなった（棒はその行の縦の中央）
            size(cancel_rect.right(), cancel_rect.size.height + px(24.0)),
        );
        let bar = count_in(&img, band, scale, theme.accent);
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ⑧ progress entries={}/{} bytes={}/{} bar_accent={bar} th=20",
            snap.map_or(0, |s| s.entries_done),
            snap.map_or(0, |s| s.entries_total),
            snap.map_or(0, |s| s.bytes_done),
            snap.map_or(0, |s| s.bytes_total),
        );
        check(
            bar >= 20,
            &format!("visual-test tree-multiselect ⑧: 帯に進み具合の棒が出る (#1867。bar={bar})"),
        );
        check(
            snap.is_some_and(|s| {
                s.entries_total == BIG as u64 + 1 && s.bytes_total == 100 * BIG as u64
            }),
            &format!(
                "visual-test tree-multiselect ⑧: 件数とバイトの母数が数えた量 (#1867。{snap:?})"
            ),
        );
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
            "visual-test tree-multiselect ⑧: 帯の「取り消し」でコピーが止まる (#1867)",
        );
        let _ = vt1860_wait(any, window, cx, Duration::from_secs(3), &|cx| {
            !notice(cx).is_empty()
        })
        .await;
        check(
            !at("src/big").exists(),
            "visual-test tree-multiselect ⑧: 取り消したら作りかけを残さない (#1867)",
        );
        let shown = window
            .update(cx, |app, _, _| {
                app.remote_notice
                    .as_ref()
                    .map(|n| (n.text.clone(), n.is_error))
            })
            .ok()
            .flatten();
        check(
            shown.as_ref().is_some_and(|(text, is_error)| {
                !is_error && text == crate::ui_text::sidebar::copy_cancelled()
            }),
            &format!(
                "visual-test tree-multiselect ⑧: 取り消しは失敗ではなく知らせとして出る (#1867。{shown:?})"
            ),
        );
        let left = std::fs::read_dir(at("big")).map(|d| d.count()).unwrap_or(0);
        check(
            left == BIG,
            &format!("visual-test tree-multiselect ⑧: コピー元は全部残る (#1867。{left})"),
        );
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ⑧ cancelled notice={:?}",
            notice(cx)
        );
    } else {
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ⑧ skipped（TAKO_1867_COPY_DELAY_MS 未設定 = 進み具合は scripts/test-tree-multiselect-1867.sh で見る）"
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
