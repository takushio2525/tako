//! visual-test `preview-reveal-2247`（Issue #2247）: プレビューのペインの右クリックメニューの
//! 「Finder で表示」を**実 OS マウスと同じ `PlatformInput` 経路**で押すと、ファイルマネージャを開く
//! 1 回（macOS は `open -R <対象>`）が起きるか。
//!
//! **本物の Finder の窓を出さない**: 節の一時 dir に偽の `open`（呼ばれた引数を 1 行ずつ書くだけ）を
//! 置き、この GUI の `PATH` の先頭へ足す。押す前に「`PATH` で最初に当たる `open` が偽物」を確かめ、
//! 違えば押さずに落ちる（tako は `open` を名前で起こす = `os_integration::reveal`）。
//! Windows の `explorer.exe` は名前の差し替えでは受けられないので、この節は macOS だけ。
//!
//! 押す入口は 2 つ（ヘッダの右クリック = #185 / 本文の右クリック = #1684）。押す位置は描いた項目の
//! 実矩形（`pane_menu_item_rects`）。対象は 素の名前 / 空白 + 日本語の名前 / 画像・PDF・動画（本文に文字の無い
//! 種類 = 報告時の本番で開いていたもの）/ 開いた後に消えたファイル。
//! 同じ操作の他の入口（ファイルツリーの右クリック・ターミナルのパスの cmd+右クリック）は、その項目が
//! 呼ぶ関数を同じ対象で呼び、偽の `open` へ届く引数が同じかを突き合わせる（CLI / MCP は
//! `scripts/test-preview-reveal-2247.sh` が実の IPC で見る）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_2247_LEGACY=1`（修正前の作り = プレビューのメニューだけが
//! dispatch を通らず reveal を直に呼び、結果を捨てる）では ⑤ の「消えたファイルでは open を呼ばない」で FAILED
//! （無いパスへ `open -R` を黙って起こす = 実物の `open` は「無い」で終わり、画面には何も出ない）。
//! 単独実行は `TAKO_VISUAL_ONLY=preview-reveal-2247`、画像は `TAKO_VISUAL_DUMP_DIR` へ

use super::*;

/// 偽の `open` の記録を読む（1 回の呼び出し = 1 行・引数はタブ区切り。先頭の `open` は印）
fn vt2247_calls(log: &std::path::Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('\t').skip(1).map(str::to_string).collect())
        .collect()
}

/// 記録が `want` 件になるまで待ち（`open` は子プロセスなので着地は非同期）、
/// 着いたあとも少し待って**余計な呼び出しが来ない**ことまで見る
async fn vt2247_settle(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    log: &std::path::Path,
    want: usize,
) -> Vec<Vec<String>> {
    let log = log.to_path_buf();
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(5), &|_| {
        vt2247_calls(&log).len() >= want
    })
    .await;
    cx.background_executor()
        .timer(Duration::from_millis(400))
        .await;
    vt2247_calls(&log)
}

/// プレビューのヘッダ（ファイル名の上）の 1 点。テキスト領域はヘッダの下から始まる
/// （`pane_text_area_of` の会計 = 上端 + 枠 + 余白 + `PANE_TITLE_BAR`）ので、そこから引き戻す
fn vt2247_header_point(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<Point<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.pane_text_areas
                .iter()
                .find(|(p, _)| *p == pane)
                .map(|(_, b)| {
                    point(
                        b.origin.x + px(48.0),
                        b.origin.y - px(PANE_PADDING + PANE_TITLE_BAR / 2.0),
                    )
                })
        })
        .ok()
        .flatten()
}

/// 本文の 1 点（テキスト領域の左上寄り = 1 行目の上）
fn vt2247_body_point(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<Point<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.pane_text_areas
                .iter()
                .find(|(p, _)| *p == pane)
                .map(|(_, b)| point(b.origin.x + px(40.0), b.origin.y + px(12.0)))
        })
        .ok()
        .flatten()
}

/// プレビューの種類ごとの描き終わりの待ち方
#[derive(Clone, Copy, PartialEq, Eq)]
enum Vt2247Kind {
    /// Markdown / コード（行の配置が揃うまで）
    Text,
    /// PDF（ラスタライズと文字の配置が揃うまで）
    Pdf,
    /// 画像・動画（本文に文字が無い = そのファイルの状態に差し替わって数フレーム描くまで）
    Media,
}

/// ファイルをこのプレビューのペインへ出し直す（同じペインを使い回す = 窓を細らせない）
async fn vt2247_show(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    path: &std::path::Path,
    kind: Vt2247Kind,
) {
    let opened = window
        .update(cx, |app, _, cx| {
            let r = tako_control::dispatch(
                app,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(pane.as_u64()),
                    path: path.display().to_string(),
                    mode: None,
                    direction: None,
                    focus: Some(false),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            );
            app.drain_pending_highlights(cx);
            cx.notify();
            r.ok().and_then(|v| v["pane"].as_u64())
        })
        .ok()
        .flatten();
    check(
        opened == Some(pane.as_u64()),
        &format!(
            "visual-test preview-reveal-2247: {} を同じペインへ出せる（{opened:?}）",
            path.display()
        ),
    );
    let drawn = match kind {
        Vt2247Kind::Text => wait_for_preview_maps(any, window, cx, pane, false).await,
        Vt2247Kind::Pdf => wait_for_preview_maps(any, window, cx, pane, true).await,
        Vt2247Kind::Media => {
            let want = path.to_path_buf();
            let shown = vt1860_wait(any, window, cx, Duration::from_secs(10), &|cx| {
                window
                    .update(cx, |app, _, _| {
                        app.previews.get(&pane).is_some_and(|s| s.path == want)
                    })
                    .unwrap_or(false)
            })
            .await;
            for _ in 0..5 {
                notify_and_draw(any, window, cx);
            }
            shown
        }
    };
    check(
        drawn,
        &format!(
            "visual-test preview-reveal-2247: {} が描かれる",
            path.display()
        ),
    );
}

/// 右クリックでメニューを開き、「Finder で表示」の実矩形を実マウスで押す。
/// 戻り値は押す前のメニューに描かれた項目
fn vt2247_press_reveal(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    at: Point<Pixels>,
    how: &str,
) -> Vec<&'static str> {
    vt1860_click(any, window, cx, at, MouseButton::Right);
    let (_, drawn) = vt1684_menu(any, window, cx).unwrap_or_else(|| {
        fail(&format!(
            "visual-test preview-reveal-2247 {how}: 右クリックでメニューが開く"
        ))
    });
    let owner = window
        .update(cx, |app, _, _| {
            app.pane_context_menu
                .as_ref()
                .map(|m| (m.pane, matches!(m.kind, PaneContextKind::Preview)))
        })
        .ok()
        .flatten();
    check(
        owner == Some((pane, true)),
        &format!(
            "visual-test preview-reveal-2247 {how}: プレビューのメニューとして開く（{owner:?}）"
        ),
    );
    let item = vt1684_item_point(window, cx, "reveal").unwrap_or_else(|| {
        fail(&format!(
            "visual-test preview-reveal-2247 {how}: 「Finder で表示」が描かれる（{drawn:?}）"
        ))
    });
    vt1860_click(any, window, cx, item, MouseButton::Left);
    let closed = window
        .update(cx, |app, _, _| app.pane_context_menu.is_none())
        .unwrap_or(false);
    check(
        closed,
        &format!("visual-test preview-reveal-2247 {how}: 押すとメニューが閉じる"),
    );
    drawn
}

/// サイドバーの通知欄の文言（無ければ `None`）
fn vt2247_notice(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> Option<String> {
    window
        .update(cx, |app, _, _| {
            app.remote_notice.as_ref().map(|n| n.text.clone())
        })
        .ok()
        .flatten()
}

pub(super) async fn preview_reveal_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "preview-reveal-2247";
    inject_section_failure(LABEL);
    if !cfg!(target_os = "macos") {
        println!("TAKO_VISUAL_PIXEL: {LABEL} SKIPPED（偽の open で受けられるのは macOS だけ）");
        return;
    }
    let anchor = ensure_fresh_scene(window, cx, LABEL).await;
    let tmp = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    // fail() の経路でも消える（#2051）
    remove_on_fail(&tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("bin")).expect("visual-test preview-reveal-2247 の一時 dir");
    // 比べるのはプレビューが持つ形（dispatch が正規化した実体のパス = /private/var/… 等）
    let tmp = std::fs::canonicalize(&tmp).expect("一時 dir を正規化できる");
    let log = tmp.join("open-calls.log");
    let fake = tmp.join("bin").join("open");
    check(
        !log.display().to_string().contains('\''),
        "visual-test preview-reveal-2247: 一時 dir に引用符が無い（偽の open に埋め込む）",
    );
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\n{{ printf 'open'; for a in \"$@\"; do printf '\\t%s' \"$a\"; done; printf '\\n'; }} >> '{}'\n",
            log.display()
        ),
    )
    .expect("偽の open を書ける");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .expect("偽の open に実行権を付けられる");
    }
    let mut dirs = vec![tmp.join("bin")];
    dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::set_var(
        "PATH",
        std::env::join_paths(dirs).expect("PATH を組み直せる"),
    );
    // 本物の Finder を出さない: PATH で最初に当たる `open` が偽物でなければ押さない
    let first = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("open"))
        .find(|p| p.is_file());
    if first.as_deref() != Some(fake.as_path()) {
        fail(&format!(
            "visual-test {LABEL}: PATH の先頭の open が偽物でない（{first:?}）ので押さない"
        ));
    }

    let plain = tmp.join("reveal.md");
    let spaced = tmp.join("空白 と 日本語.md");
    let gone = tmp.join("消える.md");
    for (p, body) in [
        (&plain, "# reveal\n\n本文\n"),
        (&spaced, "# 空白 と 日本語\n\n本文\n"),
        (&gone, "# 消える\n\n本文\n"),
    ] {
        std::fs::write(p, body).expect("fixture を書ける");
    }
    // 本文に文字の無い種類（#2247 の報告時の本番で開いていたのは画像・動画・PDF）
    let png = tmp.join("画像 a.png");
    image::RgbaImage::from_fn(640, 360, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255])
    })
    .save(&png)
    .expect("fixture の画像を書ける");
    let pdf = tmp.join("資料 b.pdf");
    write_test_pdf(&pdf);
    // 中身は見ない（振り分けは拡張子 = `open_plan` の 1 実装。#1472 の素材と同じ）
    let mp4 = tmp.join("動画 c.mp4");
    std::fs::write(&mp4, b"\x00\x00\x00\x18ftypmp42").expect("fixture の動画を書ける");
    let pane = vt1684_open(any, window, cx, anchor, &plain, LABEL).await;
    let want = |p: &std::path::Path| vec!["-R".to_string(), p.display().to_string()];

    // ① ヘッダの右クリック → 「Finder で表示」 = `open -R <対象>` が 1 回
    let at = vt2247_header_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: ヘッダの位置を採れない")));
    let drawn = vt2247_press_reveal(any, window, cx, pane, at, "①");
    let calls = vt2247_settle(any, window, cx, &log, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ① header drawn={drawn:?} calls={calls:?}");
    check(
        calls == vec![want(&plain)],
        &format!("visual-test {LABEL} ①: ヘッダのメニューから open -R <対象> が 1 回呼ばれる（{calls:?}）"),
    );

    // 目視の代わりの 1 枚（メニューを開いた状態）。`TAKO_VISUAL_DUMP_DIR` があるときだけ
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        vt1860_click(any, window, cx, at, MouseButton::Right);
        notify_and_draw(any, window, cx);
        if let Some((frame, _)) = capture_frame(any, cx) {
            let _ = frame.save(std::path::Path::new(&dump).join("preview-reveal-menu.png"));
        }
        vt1684_close(any, window, cx);
    }

    // ② 本文の右クリック（#1684 の入口）でも同じ 1 回
    let _ = std::fs::write(&log, "");
    let at = vt2247_body_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ②: 本文の位置を採れない")));
    vt2247_press_reveal(any, window, cx, pane, at, "②");
    let calls = vt2247_settle(any, window, cx, &log, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② body calls={calls:?}");
    check(
        calls == vec![want(&plain)],
        &format!("visual-test {LABEL} ②: 本文のメニューからも open -R <対象> が 1 回（{calls:?}）"),
    );

    // ③ 空白 + 日本語の名前は 1 つの引数のまま渡る
    let _ = std::fs::write(&log, "");
    vt2247_show(any, window, cx, pane, &spaced, Vt2247Kind::Text).await;
    let at = vt2247_header_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ③: ヘッダの位置を採れない")));
    vt2247_press_reveal(any, window, cx, pane, at, "③");
    let calls = vt2247_settle(any, window, cx, &log, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ③ spaced calls={calls:?}");
    check(
        calls == vec![want(&spaced)],
        &format!("visual-test {LABEL} ③: 空白 + 日本語の名前が 1 つの引数で渡る（{calls:?}）"),
    );

    // ④ 画像・PDF・動画（本文に文字が無い種類）。ヘッダから開いたメニューは本文の上へ垂れるので、
    //    本文の描画が項目を覆っていないかもここで見る
    for (what, path, kind) in [
        ("画像", &png, Vt2247Kind::Media),
        ("PDF", &pdf, Vt2247Kind::Pdf),
        ("動画", &mp4, Vt2247Kind::Media),
    ] {
        let _ = std::fs::write(&log, "");
        vt2247_show(any, window, cx, pane, path, kind).await;
        let at = vt2247_header_point(window, cx, pane).unwrap_or_else(|| {
            fail(&format!(
                "visual-test {LABEL} ④ {what}: ヘッダの位置を採れない"
            ))
        });
        let how = format!("④ {what}");
        vt2247_press_reveal(any, window, cx, pane, at, &how);
        let calls = vt2247_settle(any, window, cx, &log, 1).await;
        println!("TAKO_VISUAL_PIXEL: {LABEL} ④ {what} calls={calls:?}");
        check(
            calls == vec![want(path)],
            &format!("visual-test {LABEL} ④ {what}: open -R <対象> が 1 回（{calls:?}）"),
        );
    }

    // ⑤ 開いた後に消えたファイル: open は呼ばず、理由を通知欄へ出す（黙って何も起きない、にしない）
    let _ = std::fs::write(&log, "");
    vt2247_show(any, window, cx, pane, &gone, Vt2247Kind::Text).await;
    std::fs::remove_file(&gone).expect("fixture を消せる");
    let _ = window.update(cx, |app, _, _| app.remote_notice = None);
    let at = vt2247_header_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ⑤: ヘッダの位置を採れない")));
    vt2247_press_reveal(any, window, cx, pane, at, "⑤");
    let calls = vt2247_settle(any, window, cx, &log, 0).await;
    let notice = vt2247_notice(window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑤ gone calls={calls:?} notice={notice:?}");
    check(
        calls.is_empty(),
        &format!("visual-test {LABEL} ⑤: 消えたファイルでは open を呼ばない（{calls:?}）"),
    );
    check(
        notice.as_deref().is_some_and(|n| n.contains("存在しない")),
        &format!("visual-test {LABEL} ⑤: 消えたファイルでは理由を通知欄へ出す（{notice:?}）"),
    );
    if let (Ok(dump), Some((frame, _))) = (
        std::env::var("TAKO_VISUAL_DUMP_DIR"),
        capture_frame(any, cx),
    ) {
        let _ = frame.save(std::path::Path::new(&dump).join("preview-reveal-gone.png"));
    }

    // ⑥ 同じ操作の他の入口（ツリーの右クリック・ターミナルのパスの cmd+右クリック）が呼ぶ関数でも、
    //    偽の open へ届く引数がプレビューと同じ
    let _ = std::fs::write(&log, "");
    let _ = window.update(cx, |app, _, cx| {
        app.handle_context_action("reveal", &spaced, false, cx);
    });
    let tree = vt2247_settle(any, window, cx, &log, 1).await;
    let _ = std::fs::write(&log, "");
    let _ = window.update(cx, |app, _, cx| {
        app.handle_path_link_action("reveal", &spaced, false, anchor, cx);
    });
    let link = vt2247_settle(any, window, cx, &log, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑥ tree={tree:?} link={link:?}");
    check(
        tree == vec![want(&spaced)] && link == vec![want(&spaced)],
        &format!("visual-test {LABEL} ⑥: ツリーとパスのメニューも同じ引数で 1 回（tree={tree:?} link={link:?}）"),
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
