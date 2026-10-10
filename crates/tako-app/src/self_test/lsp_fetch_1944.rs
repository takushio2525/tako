//! visual-test `lsp-fetch`（FR-3.41 / Issue #1944）: 言語サーバが入っていない（まっさら）のまま .py を
//! 編集モードで開くと、tako が data dir へ取って起こし、エディタに状態が出る。押せば入る。
//!
//! 素材は `scripts/test-lsp-fetch-1944.sh` が用意する: 配布元のミラー（`TAKO_LSP_FETCH_BASE`。実の取得物を
//! 1 回取ったもの。ハッシュは表の固定値で検証される）と、ミラーを 503 にする門（`TAKO_1944_GATE` の
//! ファイル）・遅くする門（`TAKO_1944_SLOW` のファイル）。HOME は一時（node も pyright も無い）。
//!
//! 相: ①門を閉じたまま開く → 自動で取りに行って失敗 → タイトルの下の帯（見出し・理由・次の一手・
//! 「もう一度取得」）②門を開けて「もう一度取得」を**実マウスで**押す → タイトルに「取得中」（進捗）
//! ③取れたら起動 → 動作中・診断が出る（波線の数 > 0）④打つと補完の一覧が出る
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1944_LEGACY=1` は ① で名指しの FAILED（帯が出ない =
//! 黙って出ない）。画像は `TAKO_VISUAL_DUMP_DIR` へ（新旧で書き出し先を分けるのはスクリプト）

use super::*;

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

/// 状態で待つ（上限つき）: 条件が立ったら真
async fn until(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    limit: Duration,
    done: &dyn Fn(&mut AsyncApp) -> bool,
) -> bool {
    let started = std::time::Instant::now();
    loop {
        notify_and_draw(any, window, cx);
        if done(cx) {
            return true;
        }
        if started.elapsed() > limit {
            return false;
        }
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
}

/// 型の誤り（引数）と無い属性（`os.pa`）で、pyright が診断を出す本文。9 行目の末尾で打つと補完が出る
const SOURCE: &str = "import os\n\n\ndef add(a: int, b: int) -> int:\n    return a + b\n\n\ntotal: int = add(1, \"two\")\nprint(os.pa)\n";

pub(super) async fn lsp_fetch_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::Request as Req;
    use tako_core::lsp::state::ServerState;
    const LABEL: &str = "lsp-fetch";
    inject_section_failure(LABEL);
    let gate = std::env::var("TAKO_1944_GATE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            fail(&format!(
                "visual-test {LABEL}: TAKO_1944_GATE が無い（素材はスクリプトが用意する）"
            ))
        });
    check(
        gate.exists(),
        &format!("visual-test {LABEL}: 門が閉じている（ミラーが 503 = 素材の前提）"),
    );
    let anchor = ensure_fresh_scene(window, cx, LABEL).await;
    let dir = std::env::temp_dir().join(format!("tako-visual-{LABEL}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("visual-test lsp-fetch の一時ディレクトリ");
    std::fs::write(dir.join("pyproject.toml"), "[project]\nname = \"v\"\n").expect("pyproject");
    let path = dir.join("main.py");
    std::fs::write(&path, SOURCE).expect("main.py");
    let pane = window
        .update(cx, |app, _, cx| {
            let opened = tako_control::dispatch(
                app,
                Req::OpenFile {
                    pane: Some(anchor.as_u64()),
                    path: path.display().to_string(),
                    mode: Some(tako_control::protocol::PreviewModeWire::Code),
                    direction: Some(tako_control::protocol::Direction::Right),
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            )
            .expect("visual-test lsp-fetch を dispatch で開ける");
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
            .expect("visual-test lsp-fetch: 編集モードへ入れる");
            cx.notify();
            pane
        })
        .unwrap_or_else(|_| fail("visual-test lsp-fetch dispatch"));
    check(
        wait_for_preview_maps(any, window, cx, pane, false).await,
        &format!("visual-test {LABEL}: 行が描かれる"),
    );
    let server = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.lsp_document_server(pane))
            .ok()
            .flatten()
    };
    let probe = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                app.panel_click_probe_bounds
                    .borrow()
                    .get(&format!("preview-lsp-action-{}", pane.as_u64()))
                    .copied()
            })
            .ok()
            .flatten()
    };

    // ① 門が閉じている（ミラーが 503）→ 開いた時点で取りに行って失敗 → 帯とボタン
    let failed = until(any, window, cx, Duration::from_secs(60), &|cx| {
        server(cx).is_some_and(|s| s.state == ServerState::NotInstalled && s.fetch_failed)
            && probe(cx).is_some()
    })
    .await;
    dump(any, cx, "lsp-fetch-1-failed.png");
    let state = server(cx);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ① state={:?} fetch_failed={:?} reason={:?} next_step={:?} button={:?}",
        state.as_ref().map(|s| s.state.slug()),
        state.as_ref().map(|s| s.fetch_failed),
        state.as_ref().and_then(|s| s.reason.clone()),
        state.as_ref().and_then(|s| s.next_step.clone()),
        probe(cx).map(|b| (f32::from(b.size.width), f32::from(b.size.height))),
    );
    check(
        failed,
        &format!(
            "visual-test {LABEL}: ① 開いた時点で取りに行き、失敗したら帯に理由と「もう一度取得」が出る（黙って出ない状態を無くす。#1944）"
        ),
    );
    let reason = state.and_then(|s| s.reason).unwrap_or_default();
    check(
        reason.contains("503"),
        &format!("visual-test {LABEL}: ① 理由に配布元の答え（HTTP 503）が出る: {reason}"),
    );

    // ② 門を開けて「もう一度取得」を実マウスで押す → タイトルに「取得中」（進捗）
    let _ = std::fs::remove_file(&gate);
    let Some(button) = probe(cx) else {
        fail(&format!("visual-test {LABEL}: ② ボタンの矩形が無い"))
    };
    click_at(any, cx, button.center());
    let mut saw_fetching = None;
    let fetching = until(any, window, cx, Duration::from_secs(60), &|cx| {
        window
            .update(cx, |app, _, _| app.lsp_status_badge(pane))
            .ok()
            .flatten()
            .is_some_and(|(_, tone)| tone == crate::lsp_status_ui::BadgeTone::Busy)
            && server(cx).is_some_and(|s| s.fetch.as_ref().is_some_and(|p| p.done > 0))
    })
    .await;
    if fetching {
        saw_fetching = window
            .update(cx, |app, _, _| app.lsp_status_badge(pane).map(|(t, _)| t))
            .ok()
            .flatten();
        dump(any, cx, "lsp-fetch-2-fetching.png");
    }
    println!("TAKO_VISUAL_PIXEL: {LABEL} ② badge={saw_fetching:?}");
    check(
        fetching,
        &format!("visual-test {LABEL}: ② 押すとタイトルに取得中（進捗）が出る"),
    );

    // ③ 取れたら起こす → 動作中（読み込みが済む）・診断（波線）が出る
    let running = until(any, window, cx, Duration::from_secs(180), &|cx| {
        server(cx).is_some_and(|s| s.state == ServerState::Running && s.fetch.is_none())
    })
    .await;
    check(
        running,
        &format!("visual-test {LABEL}: ③ 取れたら restart を待たずに起きる"),
    );
    check(
        until(any, window, cx, Duration::from_secs(30), &|cx| {
            server(cx).is_some_and(|s| !s.loading)
        })
        .await,
        &format!("visual-test {LABEL}: ③ 読み込み中 → 動作中へ移る"),
    );
    let diagnostics = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                app.preview_edits
                    .get(&pane)
                    .and_then(|e| e.diagnostics.as_ref().map(|d| d.items.len()))
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    };
    let shown = until(any, window, cx, Duration::from_secs(120), &|cx| {
        diagnostics(cx) > 0
    })
    .await;
    let badge = window
        .update(cx, |app, _, _| app.lsp_status_badge(pane).map(|(t, _)| t))
        .ok()
        .flatten();
    dump(any, cx, "lsp-fetch-3-running.png");
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ③ diagnostics={} badge={badge:?}",
        diagnostics(cx)
    );
    check(
        shown,
        &format!("visual-test {LABEL}: ③ 診断（型の誤り・無い属性）が波線として出る"),
    );

    // ④ 9 行目の末尾（`os.pa` の後ろ）で 1 文字打つ → 補完の一覧
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            Req::PreviewCursor {
                pane: Some(pane.as_u64()),
                line: 9,
                col: "print(os.pa".len(),
                select_to_line: None,
                select_to_col: None,
                expected_version: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    // pyright は起動直後の解析の途中に空の候補を返すことがある（状態を送らないサーバなので tako は
    // 握手直後の猶予しか読み込み中と分からない）。利用者は打ち続けるので、出なければもう 1 文字打つ
    let popup = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                app.lsp_completion
                    .popup
                    .as_ref()
                    .is_some_and(|p| !p.order.is_empty())
            })
            .unwrap_or(false)
    };
    let mut typed = 0;
    let mut listed = false;
    for ch in ['t', 'h'] {
        completion_type(any, cx, ch);
        typed += 1;
        listed = until(any, window, cx, Duration::from_secs(20), &popup).await;
        if listed {
            break;
        }
    }
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ typed={typed}");
    let count = window
        .update(cx, |app, _, _| {
            app.lsp_completion.popup.as_ref().map(|p| p.order.len())
        })
        .ok()
        .flatten();
    dump(any, cx, "lsp-fetch-4-completion.png");
    println!("TAKO_VISUAL_PIXEL: {LABEL} ④ completion={count:?}");
    check(
        listed,
        &format!("visual-test {LABEL}: ④ 打つと補完の一覧が出る"),
    );
    let _ = std::fs::remove_dir_all(&dir);
}
