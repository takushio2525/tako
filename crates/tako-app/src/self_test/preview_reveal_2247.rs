//! visual-test `preview-reveal-2247`（Issue #2247）: 「Finder で表示」をどの入口から押しても、
//! tako 自身から NSWorkspace（`activateFileViewerSelectingURLs:`）で Finder を前面へ出す 1 実装
//! （dispatch `FileOpKind::Reveal` → `os_integration::reveal` → `platform::finder_reveal`）へ
//! ちょうど 1 回届くか。
//!
//! **本物の Finder の窓を出さない**: この節は自分の env を書き換えない（#2400）。起こす側
//! （`scripts/test-preview-reveal-2247.sh`）が GUI の env に次の 2 つを渡し、押す前に両方が
//! 効いていることを確かめる（欠けていれば押さずに落ちる）:
//! - `TAKO_REVEAL_RECORD=<dir>/reveal.log` = NSWorkspace へ渡す直前の URL を 1 行書くだけで Finder を開かない
//! - `PATH` の先頭の `<dir>/bin/open` = 偽の `open`（旧の形 `open -R` の引数を `<dir>/open-calls.log` へ書くだけ）
//!
//! 押す入口: プレビューのヘッダ（#185）/ 本文（#1684）の右クリックを**実 OS マウスと同じ
//! `PlatformInput` 経路**で押す（素の名前 / 空白 + 日本語の名前 / 画像・PDF・動画 = 報告時の本番で
//! 開いていた種類 / 開いた後に消えたファイル）。押す位置は描いた項目の実矩形（`pane_menu_item_rects`）。
//! 同じ操作の他の入口（ファイルツリーの右クリック・ターミナルのパスの cmd+右クリック・右パネルの添付・
//! 設定画面の「設定ファイルを Finder で表示」）は、その項目が呼ぶ関数を同じ対象で呼ぶ
//! （CLI / MCP はスクリプトが実の IPC で見る）。Windows の `explorer.exe` は受けられないので macOS だけ。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_2247_LEGACY=1`（修正前 = 子の `open -R`・プレビューの
//! メニューは結果を捨てる）では ① の「NSWorkspace へ 1 回」で FAILED（偽の open にだけ届く）。
//! 単独実行は `TAKO_VISUAL_ONLY=preview-reveal-2247`、画像は `TAKO_VISUAL_DUMP_DIR` へ

use super::*;

/// 「Finder で表示」の実体へ届いた呼び出しの記録の置き場（起こす側が env で渡す）
struct Vt2247Probe {
    /// NSWorkspace へ渡す直前の記録（`TAKO_REVEAL_RECORD`）
    record: std::path::PathBuf,
    /// 偽の `open` の記録（旧の形 = `open -R`）
    open_log: std::path::PathBuf,
}

impl Vt2247Probe {
    /// 届いた呼び出し。新 = `NSWorkspace <パス> yield=<0|1>` / 旧 = `open -R <パス>`
    /// （新・旧の順に並べる。1 回の押下は 1 行）
    fn calls(&self) -> Vec<String> {
        let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();
        let native = read(&self.record);
        let legacy = read(&self.open_log);
        let native = native.lines().map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            let mark = tako_control::platform::os_integration::REVEAL_RECORD_MARK;
            match cols.as_slice() {
                [m, path, yielded] if *m == mark => format!("NSWorkspace {path} {yielded}"),
                _ => format!("（読めない記録）{l}"),
            }
        });
        let legacy = legacy
            .lines()
            .map(|l| l.split('\t').collect::<Vec<_>>().join(" "));
        native.chain(legacy).collect()
    }

    fn reset(&self) {
        let _ = std::fs::write(&self.record, "");
        let _ = std::fs::write(&self.open_log, "");
    }
}

/// 記録が `want` 件になるまで待ち（旧の形の `open` は子プロセスなので着地は非同期）、
/// 着いたあとも少し待って**余計な呼び出しが来ない**ことまで見る。読んだら空にする
async fn vt2247_settle(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    probe: &Vt2247Probe,
    want: usize,
) -> Vec<String> {
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(5), &|_| {
        probe.calls().len() >= want
    })
    .await;
    cx.background_executor()
        .timer(Duration::from_millis(400))
        .await;
    let calls = probe.calls();
    probe.reset();
    calls
}

/// この OS で tako が Finder へ前面を譲れるか（`yieldActivation…` は macOS 14 から）
fn vt2247_expect_yield() -> bool {
    tako_control::telemetry::os_version()
        .and_then(|v| {
            v.strip_prefix("macOS ")
                .and_then(|rest| rest.split('.').next())
                .and_then(|major| major.parse::<u32>().ok())
        })
        .is_some_and(|major| major >= 14)
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
        println!(
            "TAKO_VISUAL_PIXEL: {LABEL} SKIPPED（記録の口と偽の open で受けられるのは macOS だけ）"
        );
        return;
    }
    // 本物の Finder を出さない: 記録の口と偽の open の両方が効いていなければ押さない
    let Some(record) = std::env::var_os("TAKO_REVEAL_RECORD")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
    else {
        fail(&format!(
            "visual-test {LABEL}: TAKO_REVEAL_RECORD が無い（押すと本物の Finder が出るので押さない。\
             scripts/test-preview-reveal-2247.sh から走らせる）"
        ));
    };
    let probe_dir = record
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let probe = Vt2247Probe {
        record: record.clone(),
        open_log: probe_dir.join("open-calls.log"),
    };
    let fake = probe_dir.join("bin").join("open");
    let first = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("open"))
        .find(|p| p.is_file());
    if first.as_deref() != Some(fake.as_path()) {
        fail(&format!(
            "visual-test {LABEL}: PATH の先頭の open が偽物（{}）でない（{first:?}）ので押さない",
            fake.display()
        ));
    }
    probe.reset();

    let anchor = ensure_fresh_scene(window, cx, LABEL).await;
    let tmp = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    // fail() の経路でも消える（#2051）
    remove_on_fail(&tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("visual-test preview-reveal-2247 の一時 dir");
    // 比べるのはプレビューが持つ形（dispatch が正規化した実体のパス = /private/var/… 等）
    let tmp = std::fs::canonicalize(&tmp).expect("一時 dir を正規化できる");

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
    // この GUI は前面のアプリなので、macOS 14 以降なら Finder へ前面を譲ってから頼む
    let yielded = format!("yield={}", u8::from(vt2247_expect_yield()));
    let want = |p: &std::path::Path| vec![format!("NSWorkspace {} {yielded}", p.display())];

    // ① ヘッダの右クリック → 「Finder で表示」 = NSWorkspace へ 1 回（子の open -R は起こさない）
    let at = vt2247_header_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: ヘッダの位置を採れない")));
    let drawn = vt2247_press_reveal(any, window, cx, pane, at, "①");
    let calls = vt2247_settle(any, window, cx, &probe, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ① header drawn={drawn:?} calls={calls:?}");
    check(
        calls == want(&plain),
        &format!("visual-test {LABEL} ①: ヘッダのメニューから NSWorkspace へ 1 回（{calls:?}）"),
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
    let at = vt2247_body_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ②: 本文の位置を採れない")));
    vt2247_press_reveal(any, window, cx, pane, at, "②");
    let calls = vt2247_settle(any, window, cx, &probe, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② body calls={calls:?}");
    check(
        calls == want(&plain),
        &format!("visual-test {LABEL} ②: 本文のメニューからも NSWorkspace へ 1 回（{calls:?}）"),
    );

    // ③ 空白 + 日本語の名前は崩れずに 1 つの URL で渡る
    vt2247_show(any, window, cx, pane, &spaced, Vt2247Kind::Text).await;
    let at = vt2247_header_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ③: ヘッダの位置を採れない")));
    vt2247_press_reveal(any, window, cx, pane, at, "③");
    let calls = vt2247_settle(any, window, cx, &probe, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ③ spaced calls={calls:?}");
    check(
        calls == want(&spaced),
        &format!("visual-test {LABEL} ③: 空白 + 日本語の名前が崩れずに渡る（{calls:?}）"),
    );

    // ④ 画像・PDF・動画（本文に文字が無い種類）。ヘッダから開いたメニューは本文の上へ垂れるので、
    //    本文の描画が項目を覆っていないかもここで見る
    for (what, path, kind) in [
        ("画像", &png, Vt2247Kind::Media),
        ("PDF", &pdf, Vt2247Kind::Pdf),
        ("動画", &mp4, Vt2247Kind::Media),
    ] {
        vt2247_show(any, window, cx, pane, path, kind).await;
        let at = vt2247_header_point(window, cx, pane).unwrap_or_else(|| {
            fail(&format!(
                "visual-test {LABEL} ④ {what}: ヘッダの位置を採れない"
            ))
        });
        let how = format!("④ {what}");
        vt2247_press_reveal(any, window, cx, pane, at, &how);
        let calls = vt2247_settle(any, window, cx, &probe, 1).await;
        println!("TAKO_VISUAL_PIXEL: {LABEL} ④ {what} calls={calls:?}");
        check(
            calls == want(path),
            &format!("visual-test {LABEL} ④ {what}: NSWorkspace へ 1 回（{calls:?}）"),
        );
    }

    // ⑤ 開いた後に消えたファイル: 何も呼ばず、理由を通知欄へ出す（黙って何も起きない、にしない）
    vt2247_show(any, window, cx, pane, &gone, Vt2247Kind::Text).await;
    std::fs::remove_file(&gone).expect("fixture を消せる");
    let _ = window.update(cx, |app, _, _| app.remote_notice = None);
    let at = vt2247_header_point(window, cx, pane)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ⑤: ヘッダの位置を採れない")));
    vt2247_press_reveal(any, window, cx, pane, at, "⑤");
    let calls = vt2247_settle(any, window, cx, &probe, 0).await;
    let notice = vt2247_notice(window, cx);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑤ gone calls={calls:?} notice={notice:?}");
    check(
        calls.is_empty(),
        &format!("visual-test {LABEL} ⑤: 消えたファイルでは Finder を呼ばない（{calls:?}）"),
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

    // ⑥ 同じ操作の他の入口（ツリーの右クリック・ターミナルのパスの cmd+右クリック・右パネルの添付）が
    //    呼ぶ関数でも、同じ実体へ同じ URL で 1 回
    let _ = window.update(cx, |app, _, cx| {
        app.handle_context_action("reveal", &spaced, false, cx);
    });
    let tree = vt2247_settle(any, window, cx, &probe, 1).await;
    let _ = window.update(cx, |app, _, cx| {
        app.handle_path_link_action("reveal", &spaced, false, anchor, cx);
    });
    let link = vt2247_settle(any, window, cx, &probe, 1).await;
    let _ = window.update(cx, |app, _, cx| {
        app.user_task_reveal(&spaced.display().to_string(), cx);
    });
    let task = vt2247_settle(any, window, cx, &probe, 1).await;
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑥ tree={tree:?} link={link:?} task={task:?}");
    check(
        tree == want(&spaced) && link == want(&spaced) && task == want(&spaced),
        &format!(
            "visual-test {LABEL} ⑥: ツリー・パスのメニュー・右パネルの添付も同じ実体へ 1 回\
             （tree={tree:?} link={link:?} task={task:?}）"
        ),
    );

    // ⑦ 設定画面の「設定ファイルを Finder で表示」（ボタンが呼ぶ関数）も同じ実体へ
    let _ = window.update(cx, |app, _, cx| {
        app.open_settings_window_impl(Some(crate::settings_window::SettingsTab::Advanced), cx);
    });
    let mut settings = None;
    for _ in 0..40 {
        settings = window
            .update(cx, |app, _, _| app.settings_window_handle)
            .ok()
            .flatten();
        if settings.is_some() {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    let Some(settings) = settings else {
        fail(&format!("visual-test {LABEL} ⑦: 設定ウィンドウが開かない"));
    };
    let file = tako_control::settings::settings_path().unwrap_or_else(|| {
        fail(&format!(
            "visual-test {LABEL} ⑦: 設定ファイルの置き場が無い"
        ))
    });
    // 隔離 GUI の data dir に設定ファイルがまだ無ければ置く（中身は今の設定 = 何も変えない）
    if !file.exists() {
        tako_control::settings::save(&tako_control::settings::load()).unwrap_or_else(|e| {
            fail(&format!(
                "visual-test {LABEL} ⑦: 設定ファイルを置けない: {e}"
            ))
        });
    }
    let _ = settings.update(cx, |view, _, cx| view.reveal_settings_file(cx));
    let from_settings = vt2247_settle(any, window, cx, &probe, 1).await;
    let _ = settings.update(cx, |_, window, _| window.remove_window());
    let _ = window.update(cx, |app, _, _| app.settings_window_handle = None);
    println!("TAKO_VISUAL_PIXEL: {LABEL} ⑦ settings calls={from_settings:?}");
    check(
        from_settings == want(&file),
        &format!("visual-test {LABEL} ⑦: 設定画面からも同じ実体へ 1 回（{from_settings:?}）"),
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
