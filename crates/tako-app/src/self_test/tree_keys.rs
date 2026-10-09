//! visual-test `tree-keys`（FR-3.40 / Issue #1908）: ファイルツリーの行を選んでいる間の
//! ↑ / ↓ / ← / → / Enter / ⇧⌘↑ / ⇧⌘↓（Windows は Shift+Ctrl+Home / End）と、コピーの帯の
//! 残り時間の数え下ろしを**実キー**（GPUI のキー配送 = キーバインド判定 → アクション →
//! `on_key_down`）で確かめる。
//!
//! ①↑ / ↓ で選んだ行が 1 行ずつ動き（選んだ枠が移る・ファイルは開かない・フォルダは開閉しない）、
//! 先頭 / 末尾では止まる ②→ で畳んだフォルダが開き、もう一度で最初の子へ・← で親へ・
//! もう一度で畳む・空のフォルダの → は何もしない ③Enter でファイルが開き（選択はその行のまま）、
//! フォルダは開閉する ④⇧⌘↓ / ⇧⌘↑ で起点から末尾 / 先頭まで伸び（畳んだフォルダの中は入らない）、
//! ⇧↓ で縮む ⑤リモート（SSH）の行の上ではどのキーも選択を動かさない・選んでいなければキーは
//! ツリーへ向かない ⑥一定の速さのコピー（`TAKO_1895_COPY_CHUNK_DELAY_MS` が立っているときだけ）で、
//! 帯の残り時間が 150 ms ごとの読みで行き来しない。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1908_LEGACY=1` は ↓ がツリーへ向かず選択が外れるので
//! ① が FAILED になる（⑥ も「いま」までの平均の式へ戻るので逆戻りで FAILED）= 同一バイナリでの A/B。
//! 単独実行は `TAKO_VISUAL_ONLY=tree-keys`

use super::tree_multiselect::press;
use super::*;

const SECTION: &str = "tree-keys";

/// ⑥ の大きなファイル（MiB）。遅延の注入（1 MiB ごと）と合わせて十数秒で写り終わる大きさ
/// （数 GB のファイル・CPU を焼く負荷で遅くしない）
const BIG_MIB: usize = 40;

/// ⑥ で一定の速さのときに許す見積もりの逆戻り（秒）。数え下ろしは届いた進みで終わりの
/// 時刻を引き直すだけなので、OS の間隔の揺れ（数 ms）しか戻らない。#1895 の式は 1 チャンク
/// ごとに「残り ÷ 写した × 読みの間隔」だけ戻る（写し始めの 2 秒過ぎで 0.7 秒前後）
const ETA_RISE_MAX: f64 = 0.25;

pub(super) async fn tree_keys_visual(
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
            format!("tako-vt1908-{}", std::process::id()),
        );
    }
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let mac = cfg!(target_os = "macos");
    let plain = Modifiers::default();
    let (to_top, to_bottom) = if mac {
        ("cmd-shift-up", "cmd-shift-down")
    } else {
        ("ctrl-shift-home", "ctrl-shift-end")
    };
    let delay = std::env::var("TAKO_1895_COPY_CHUNK_DELAY_MS").is_ok_and(|v| !v.trim().is_empty());

    // --- 場面: 一時 dir の中だけに fixture を作る（本物のファイルを書き換えない = #1811） ---
    let tmp = tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir());
    let raw = tmp.join(format!("tako-vt1908-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&raw);
    for dir in ["keys/sub", "keys/empty", "keys/zz", "big", "dst"] {
        std::fs::create_dir_all(raw.join(dir)).expect("visual-test tree-keys fixture");
    }
    let base = tako_core::platform::path::canonicalize_or_self(&raw);
    if !base.starts_with(&tmp) || base == tmp {
        fail(&format!(
            "visual-test tree-keys: fixture が一時 dir の外 ({})",
            base.display()
        ));
    }
    let at = |rel: &str| base.join(rel);
    for (rel, body) in [
        ("keys/a.txt", "A\n"),
        ("keys/b.txt", "B\n"),
        ("keys/c.txt", "C\n"),
        ("keys/sub/x.txt", "X\n"),
        ("keys/sub/y.txt", "Y\n"),
        ("keys/zz/z.txt", "Z\n"),
    ] {
        std::fs::write(at(rel), body).expect("visual-test tree-keys fixture");
    }
    if delay {
        // 中身のあるファイル（疎なファイルにしない）
        let mut body = vec![0u8; BIG_MIB << 20];
        for (i, b) in body.iter_mut().enumerate() {
            *b = (i.wrapping_mul(2_654_435_761) >> 13) as u8;
        }
        std::fs::write(at("big/big.bin"), body).expect("visual-test tree-keys fixture");
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
            app.filetree.expand_dir(&base.join("keys"));
            app.tree_row_probe = true;
            app.remote_notice = None;
            app.file_clipboard = None;
            app.tree_selection = None;
            cx.notify();
        })
        .unwrap_or_else(|_| fail("visual-test tree-keys: 場面づくり"));

    // 観測の道具
    let row_rect = |cx: &mut AsyncApp, path: &std::path::Path| {
        vt1834_rows(any, window, cx)
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b)
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test tree-keys: 行が描かれていない（{}）",
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
                "visual-test tree-keys: 行が窓の外（{} の下端 {:?} > 窓 {:?}）",
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
    let visible = |cx: &mut AsyncApp, path: &std::path::Path| order(cx).iter().any(|p| p == path);
    let expanded = |cx: &mut AsyncApp, path: &std::path::Path| {
        window
            .update(cx, |app, _, _| {
                app.filetree
                    .rows()
                    .iter()
                    .any(|r| r.entry.path == path && r.expanded)
            })
            .unwrap_or(false)
    };
    // 見えている並びの `from` から `to` まで（両端を含む・上から順）
    let span = |order: &[std::path::PathBuf], from: &std::path::Path, to: &std::path::Path| {
        let a = order.iter().position(|p| p == from).unwrap_or(0);
        let b = order.iter().position(|p| p == to).unwrap_or(0);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        order[lo..=hi].to_vec()
    };
    let previews = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.previews.len())
            .unwrap_or(0)
    };
    let showing = |cx: &mut AsyncApp, path: &std::path::Path| {
        window
            .update(cx, |app, _, _| {
                app.previews.values().any(|p| p.path == path)
            })
            .unwrap_or(false)
    };
    let theme = window
        .update(cx, |app, _, _| app.theme.clone())
        .unwrap_or_else(|_| fail("visual-test tree-keys: テーマ"));
    let frame = |cx: &mut AsyncApp| {
        capture_frame(any, cx).unwrap_or_else(|| fail("visual-test tree-keys: フレーム"))
    };
    let count_in = |img: &image::RgbaImage, row: Bounds<Pixels>, scale: f32, color| {
        [false, true]
            .into_iter()
            .map(|flip| color_pixels_in_bounds(img, row, scale, color, 14, flip))
            .max()
            .unwrap_or(0)
    };
    let dump = |img: &image::RgbaImage, name: &str| {
        if let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
            let _ = img.save(std::path::Path::new(&dir).join(name));
        }
    };
    let landed = Duration::from_secs(10);

    // ① ↑ / ↓ で 1 行ずつ動く（行を押して開いた直後から = Finder / VS Code と同じ）
    let a_pt = row_point(cx, &at("keys/a.txt"));
    press(any, window, cx, a_pt, MouseButton::Left, plain);
    let _ = vt1860_wait(any, window, cx, landed, &|cx| {
        showing(cx, &at("keys/a.txt"))
    })
    .await;
    check(
        selected(cx) == Some(vec![at("keys/a.txt")]),
        &format!(
            "visual-test tree-keys ①: 押した行を選ぶ (#1908。{:?})",
            selected(cx)
        ),
    );
    let opened = previews(cx);
    let b_rect = row_rect(cx, &at("keys/b.txt"));
    let (before, scale) = frame(cx);
    vt1860_press(any, window, cx, "down");
    check(
        selected(cx) == Some(vec![at("keys/b.txt")]),
        &format!(
            "visual-test tree-keys ①: ↓ で 1 行下へ動く (#1908。{:?})",
            selected(cx)
        ),
    );
    let (after, _) = frame(cx);
    dump(&after, "tree-keys-1-down.png");
    let gain = count_in(&after, b_rect, scale, theme.accent).saturating_sub(count_in(
        &before,
        b_rect,
        scale,
        theme.accent,
    ));
    println!("TAKO_VISUAL_PIXEL: {SECTION} ① moved accent_gain={gain} th=120");
    check(
        gain >= 120,
        &format!("visual-test tree-keys ①: 動いた先の行に選んだ枠が出る (#1908。gain={gain})"),
    );
    check(
        previews(cx) == opened && !showing(cx, &at("keys/b.txt")),
        "visual-test tree-keys ①: ↑ / ↓ はファイルを開かない (#1908)",
    );
    vt1860_press(any, window, cx, "up");
    vt1860_press(any, window, cx, "up");
    // a.txt の上は畳んだフォルダ keys/zz（フォルダが先に並ぶ）。選ぶだけで開かない
    check(
        selected(cx) == Some(vec![at("keys/zz")]) && !expanded(cx, &at("keys/zz")),
        &format!(
            "visual-test tree-keys ①: ↑ で 1 行ずつ上へ・フォルダは開閉しない (#1908。{:?})",
            selected(cx)
        ),
    );
    // 先頭で ↑・末尾で ↓ は何も変えない（右クリックで選ぶ = 開閉しない）
    let o = order(cx);
    let first = o
        .first()
        .cloned()
        .unwrap_or_else(|| fail("visual-test tree-keys ①: 行が無い"));
    let last = o
        .last()
        .cloned()
        .unwrap_or_else(|| fail("visual-test tree-keys ①: 行が無い"));
    let first_pt = row_point(cx, &first);
    press(any, window, cx, first_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "up");
    check(
        selected(cx) == Some(vec![first.clone()]),
        &format!(
            "visual-test tree-keys ①: 先頭で ↑ は何も変えない (#1908。{:?})",
            selected(cx)
        ),
    );
    let last_pt = row_point(cx, &last);
    press(any, window, cx, last_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "down");
    check(
        selected(cx) == Some(vec![last.clone()]),
        &format!(
            "visual-test tree-keys ①: 末尾で ↓ は何も変えない (#1908。{:?})",
            selected(cx)
        ),
    );
    check(
        window
            .update(cx, |app, _, _| app.context_menu.is_none())
            .unwrap_or(false),
        "visual-test tree-keys ①: キーで操作したら右クリックメニューは畳む (#1908)",
    );

    // ② → / ← で開いて中へ・親へ・畳む。空のフォルダの → は何もしない
    let sub = at("keys/sub");
    let sub_pt = row_point(cx, &sub);
    press(any, window, cx, sub_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "right");
    check(
        expanded(cx, &sub) && visible(cx, &at("keys/sub/x.txt")),
        "visual-test tree-keys ②: 畳んだフォルダの → で開く (#1908)",
    );
    check(
        selected(cx) == Some(vec![sub.clone()]),
        &format!(
            "visual-test tree-keys ②: 開いても選択はフォルダのまま (#1908。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "right");
    check(
        selected(cx) == Some(vec![at("keys/sub/x.txt")]),
        &format!(
            "visual-test tree-keys ②: 開いたフォルダの → で最初の子へ (#1908。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "left");
    check(
        selected(cx) == Some(vec![sub.clone()]),
        &format!(
            "visual-test tree-keys ②: ファイルの ← で親のフォルダへ (#1908。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "left");
    check(
        !expanded(cx, &sub) && !visible(cx, &at("keys/sub/x.txt")),
        "visual-test tree-keys ②: 開いたフォルダの ← で畳む (#1908)",
    );
    vt1860_press(any, window, cx, "left");
    check(
        selected(cx) == Some(vec![at("keys")]),
        &format!(
            "visual-test tree-keys ②: 畳んだフォルダの ← で親へ (#1908。{:?})",
            selected(cx)
        ),
    );
    let empty = at("keys/empty");
    let empty_pt = row_point(cx, &empty);
    press(any, window, cx, empty_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "right");
    vt1860_press(any, window, cx, "right");
    check(
        expanded(cx, &empty) && selected(cx) == Some(vec![empty.clone()]),
        &format!(
            "visual-test tree-keys ②: 空のフォルダは開くだけで中へは下りない (#1908。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "left");

    // ③ Enter = 素の押下と同じ（ファイルは開く・フォルダは開閉）。行を押して開いてから
    // ↓ で動いた先を Enter で開く（キーだけで次のファイルへ = Finder / VS Code の使い方）
    let c = at("keys/c.txt");
    let a_pt = row_point(cx, &at("keys/a.txt"));
    press(any, window, cx, a_pt, MouseButton::Left, plain);
    vt1860_press(any, window, cx, "down");
    vt1860_press(any, window, cx, "down");
    check(
        selected(cx) == Some(vec![c.clone()]) && !showing(cx, &c),
        &format!(
            "visual-test tree-keys ③: ↓ で動いた先は選ぶだけで開かない (#1908。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "enter");
    let shown = vt1860_wait(any, window, cx, landed, &|cx| showing(cx, &c)).await;
    check(
        shown,
        "visual-test tree-keys ③: ファイルの上の Enter で開く (#1908)",
    );
    check(
        selected(cx) == Some(vec![c.clone()]),
        &format!(
            "visual-test tree-keys ③: 開いた後も選択はその行のまま（続けて ↑ / ↓ が効く。#1908。{:?})",
            selected(cx)
        ),
    );
    vt1860_press(any, window, cx, "up");
    check(
        selected(cx) == Some(vec![at("keys/b.txt")]),
        &format!(
            "visual-test tree-keys ③: 開いた後の ↑ もツリーへ向く (#1908。{:?})",
            selected(cx)
        ),
    );
    let zz = at("keys/zz");
    let zz_pt = row_point(cx, &zz);
    press(any, window, cx, zz_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, "enter");
    check(
        expanded(cx, &zz) && visible(cx, &at("keys/zz/z.txt")),
        "visual-test tree-keys ③: フォルダの上の Enter で開く (#1908)",
    );
    vt1860_press(any, window, cx, "enter");
    check(
        !expanded(cx, &zz),
        "visual-test tree-keys ③: もう一度 Enter で畳む (#1908)",
    );

    // ④ ⇧⌘↓ / ⇧⌘↑（Windows は Shift+Ctrl+End / Home）で端まで・畳んだフォルダの中は入らない
    let o = order(cx);
    let sub_pt = row_point(cx, &sub);
    press(any, window, cx, sub_pt, MouseButton::Right, plain);
    vt1860_press(any, window, cx, to_bottom);
    let sel = selected(cx).unwrap_or_default();
    check(
        sel == span(&o, &sub, o.last().unwrap_or(&sub)),
        &format!("visual-test tree-keys ④: {to_bottom} で末尾まで伸びる (#1908。{sel:?})"),
    );
    check(
        !sel.contains(&at("keys/zz/z.txt")) && !sel.contains(&at("keys/sub/x.txt")),
        &format!("visual-test tree-keys ④: 畳んだフォルダの中は範囲に入らない (#1908。{sel:?})"),
    );
    let (img, _) = frame(cx);
    dump(&img, "tree-keys-4-to-bottom.png");
    vt1860_press(any, window, cx, to_top);
    let sel = selected(cx).unwrap_or_default();
    check(
        sel == span(&o, o.first().unwrap_or(&sub), &sub),
        &format!(
            "visual-test tree-keys ④: {to_top} で起点（動かない）から先頭まで (#1908。{sel:?})"
        ),
    );
    vt1860_press(any, window, cx, "shift-down");
    let sel = selected(cx).unwrap_or_default();
    check(
        o.get(1).is_some_and(|second| sel == span(&o, second, &sub)),
        &format!("visual-test tree-keys ④: 続けて ⇧↓ で縮む (#1908。{sel:?})"),
    );
    // 範囲の上で ↓ = 最後に押した行の下の 1 行だけ
    vt1860_press(any, window, cx, "down");
    let sel = selected(cx).unwrap_or_default();
    check(
        sel.len() == 1 && o.get(2).is_some_and(|third| sel[0] == *third),
        &format!("visual-test tree-keys ④: 範囲の上の ↓ は 1 行だけになる (#1908。{sel:?})"),
    );

    // ⑤ リモート（SSH）の行の上ではどのキーも選択を動かさない（範囲の並びに載らない）。
    // 実の SSH は立てず、リモートの行を押したときと同じ選択の状態を置く
    let remote_path = std::path::PathBuf::from("/tako-vt1908-remote/a.txt");
    let _ = window.update(cx, |app, _, cx| {
        let tab = app.workspace.active_tab_id();
        let pane = app.focused_pane();
        app.tree_selection = Some(crate::sidebar::TreeSelection {
            path: remote_path.clone(),
            is_dir: false,
            root: false,
            remote: true,
            tab,
            pane,
            paths: vec![remote_path.clone()],
            anchor: remote_path.clone(),
        });
        cx.notify();
    });
    for key in ["down", "up", "left", "right", "enter", to_bottom] {
        vt1860_press(any, window, cx, key);
        check(
            selected(cx) == Some(vec![remote_path.clone()]),
            &format!(
                "visual-test tree-keys ⑤: リモートの行の上の {key} は選択を動かさない（ペインへも流さない。#1908。{:?}）",
                selected(cx)
            ),
        );
    }
    // 選んでいなければキーはツリーへ向かない（選択は生えない）
    vt1860_press(any, window, cx, "escape");
    vt1860_press(any, window, cx, "down");
    check(
        selected(cx).is_none(),
        "visual-test tree-keys ⑤: 選んでいないときの ↓ はツリーへ向かない (#1908)",
    );

    // ⑥ 一定の速さのコピーで、帯の残り時間が行き来しない（150 ms ごとに読む = 帯の刻み）
    if delay {
        // keys を畳んで big / dst の行を窓の中へ
        let keys = at("keys");
        let _ = window.update(cx, |app, _, cx| {
            if app
                .filetree
                .rows()
                .iter()
                .any(|r| r.entry.path == keys && r.expanded)
            {
                app.filetree.toggle_dir(&keys);
            }
            cx.notify();
        });
        notify_and_draw(any, window, cx);
        let menu_click = |cx: &mut AsyncApp, row: &std::path::Path, item: &str| {
            let pt = row_point(cx, row);
            press(any, window, cx, pt, MouseButton::Right, plain);
            let items = window
                .update(cx, |app, _, _| app.tree_menu_item_rects.borrow().clone())
                .unwrap_or_default();
            let Some((_, rect)) = items.iter().find(|(id, _)| *id == item) else {
                fail(&format!("visual-test tree-keys: メニューに {item} が無い"));
            };
            press(any, window, cx, rect.center(), MouseButton::Left, plain);
        };
        let _ = window.update(cx, |app, _, cx| {
            app.filetree.expand_dir(&at("big"));
            cx.notify();
        });
        notify_and_draw(any, window, cx);
        menu_click(cx, &at("big/big.bin"), "clip-copy");
        menu_click(cx, &at("dst"), "clip-paste");
        let snap = || {
            tako_core::file_copy::jobs()
                .list()
                .first()
                .map(|t| t.progress.snapshot())
        };
        let mut secs: Vec<f64> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(40) {
            notify_and_draw(any, window, cx);
            let Some(s) = snap() else {
                if !secs.is_empty() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                continue;
            };
            if let Some(left) = tako_core::file_copy::eta(&s) {
                secs.push(left.as_secs_f64());
                // 帯に書く文字（`render_copy_progress` と同じ組み立て）
                labels.push(crate::ui_text::sidebar::copy_eta(
                    tako_core::file_copy::eta_label(left),
                ));
            }
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
        }
        let rises: Vec<f64> = secs
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|d| *d > 1e-6)
            .collect();
        let max_rise = rises.iter().copied().fold(0.0, f64::max);
        let total_rise: f64 = rises.iter().sum();
        // 表記が「前に出ていた別の表記」へ戻った回数（「15 秒 → まもなく → 15 秒」= 1 回）
        let mut label_back = 0usize;
        let mut seen: Vec<&String> = Vec::new();
        for l in &labels {
            if seen.last() != Some(&l) {
                if seen.contains(&l) {
                    label_back += 1;
                }
                seen.push(l);
            }
        }
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ⑥ eta samples={} max_rise={max_rise:.3} total_rise={total_rise:.3} label_back={label_back} labels={:?} th={ETA_RISE_MAX}",
            secs.len(),
            seen
        );
        check(
            secs.len() >= 20,
            &format!(
                "visual-test tree-keys ⑥: 残り時間を 20 回以上読めた (#1908。{})",
                secs.len()
            ),
        );
        check(
            max_rise < ETA_RISE_MAX,
            &format!(
                "visual-test tree-keys ⑥: 一定の速さのコピーで残り時間が戻らない (#1908。max_rise={max_rise:.3} total_rise={total_rise:.3})"
            ),
        );
        let _ = vt1860_wait(any, window, cx, Duration::from_secs(40), &|_| {
            tako_core::file_copy::jobs().list().is_empty()
        })
        .await;
    } else {
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} ⑥ skipped（TAKO_1895_COPY_CHUNK_DELAY_MS 未設定 = scripts/test-tree-keys-1908.sh で見る）"
        );
    }

    // 後片付け（一時 dir の中だけ）
    let _ = window.update(cx, |app, _, cx| {
        app.tree_selection = None;
        app.file_clipboard = None;
        app.tree_row_probe = false;
        cx.notify();
    });
    let _ = std::fs::remove_dir_all(&base);
}
