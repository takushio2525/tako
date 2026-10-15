//! visual-test `lsp-servers`（Issue #1956）: 検出表の第 2 波（texlab / tombi / sourcekit-lsp / dart /
//! vscode-css-language-server / vscode-html-language-server）を**実のサーバ**で起こし、誤りの波線と
//! ホバーのカードが出た画面を落とす。
//!
//! 素材は `scripts/test-lsp-servers-1956.sh` が用意する（サーバの置き場・ログインシェルの PATH・誤りを
//! 含むファイル）。`TAKO_1956_CASES` は 1 行 1 件 `<ファイル>\t<行>\t<桁>`（ホバーを置く位置。0 起点・
//! 桁はバイト）。受け持つサーバは検出表から引く（言語名・サーバ名をここに書かない）。
//! 4 列目に `unconfigured` を書いた件（#1951。`scripts/test-lsp-arduino-1951.sh` の基板の無いスケッチ）は、
//! サーバを起こさず「前提が未設定」の帯（理由・選ぶコマンド・確かめ直すボタン）が描かれた画面
//! （`<id>-unconfigured.png`）を落とす。
//! #2113（`scripts/test-lsp-sketch-headers-2113.sh` のスケッチ内の `.h` / `.cpp`）: `clean` は**誤りの無い**
//! ファイル = ホバーの答えが出た（サーバが文書を読んだ）後も波線が 0 本のまま（`<id>-clean.png`）で、
//! カードが出る。`diag` は波線だけを撮る（旧挙動の嘘のエラー。ホバーは見ない）。
//!
//! 1 件ごとに: ① 編集モードで開く → 稼働して読み込みが済む ② 診断が 1 件以上、画面の写しまで届く
//! （波線。`<id>-diag.png`）
//! ③ 実 GUI のマウス経路でホバーのカードが出る（`<id>-hover.png`）。どれかが出なければ名指しの FAILED。
//! 開いてから ① ② までの時間を `TAKO_VISUAL_PIXEL` に出す（起動時間の記録）。
//! 単独実行は `TAKO_VISUAL_ONLY=lsp-servers`

use super::*;

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）
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

/// 画面が波線を描く元（編集セッションの診断の写し）の件数
fn drawn(app: &TakoApp, pane: PaneId) -> usize {
    app.preview_edits
        .get(&pane)
        .and_then(|edit| edit.diagnostics.as_ref())
        .map_or(0, |d| d.items.len())
}

/// 1 件（ファイル・ホバーを置く行と桁・期待する状態）
struct Case {
    path: std::path::PathBuf,
    line: usize,
    column: usize,
    /// `Some("unconfigured")` = 起こす前提が未設定の帯を撮る（#1951）。`Some("clean")` = 波線が出ず
    /// ホバーが出る / `Some("diag")` = 波線だけ（#2113）。`None` = 波線とホバー
    expect: Option<String>,
}

fn cases() -> Vec<Case> {
    let raw = std::env::var("TAKO_1956_CASES").unwrap_or_else(|_| {
        fail("visual-test lsp-servers: TAKO_1956_CASES が無い（素材はスクリプトが用意する）")
    });
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let (Some(path), Some(l), Some(c)) = (parts.next(), parts.next(), parts.next()) else {
                fail(&format!("visual-test lsp-servers: 件の形が違う: {line:?}"))
            };
            Case {
                path: path.into(),
                line: l
                    .parse()
                    .unwrap_or_else(|_| fail("visual-test lsp-servers: 行")),
                column: c
                    .parse()
                    .unwrap_or_else(|_| fail("visual-test lsp-servers: 桁")),
                expect: parts.next().map(str::to_string),
            }
        })
        .collect()
}

pub(super) async fn lsp_servers_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::Request as Req;
    use tako_core::lsp::state::ServerState;
    const LABEL: &str = "lsp-servers";
    inject_section_failure(LABEL);
    let cases = cases();
    check(
        !cases.is_empty(),
        "visual-test lsp-servers: 件が 1 つ以上ある",
    );
    for case in cases {
        let id = tako_core::lsp::servers::resolve_in(tako_core::lsp::servers::SERVERS, &case.path)
            .map(|r| r.spec.id)
            .unwrap_or_else(|| {
                fail(&format!(
                    "visual-test {LABEL}: {} を受け持つ行が検出表に無い",
                    case.path.display()
                ))
            });
        let anchor = ensure_fresh_scene(window, cx, LABEL).await;
        let started = std::time::Instant::now();
        let pane = window
            .update(cx, |app, _, cx| {
                let opened = tako_control::dispatch(
                    app,
                    Req::OpenFile {
                        pane: Some(anchor.as_u64()),
                        path: case.path.display().to_string(),
                        mode: Some(tako_control::protocol::PreviewModeWire::Code),
                        direction: Some(tako_control::protocol::Direction::Right),
                        focus: Some(true),
                        new_tab: false,
                        line: None,
                        column: None,
                    },
                    PaneOrigin::Cli,
                )
                .expect("visual-test lsp-servers を dispatch で開ける");
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
                .expect("visual-test lsp-servers: 編集モードへ入れる");
                cx.notify();
                pane
            })
            .unwrap_or_else(|_| fail("visual-test lsp-servers dispatch"));
        check(
            wait_for_preview_maps(any, window, cx, pane, false).await,
            &format!("visual-test {LABEL} {id}: 行が描かれる"),
        );
        // ① 稼働して読み込みが済む ② 診断が届く（状態で待つ。上限は初回の索引を見込んで 180 秒）。
        // 診断と問い合わせは**エディタが持っているパス**で引く（`/tmp` と `/private/tmp` のように、
        // 渡したパスと文書の URI が symlink の分だけ違うことがある）
        let path = window
            .update(cx, |app, _, _| {
                app.preview_edits
                    .get(&pane)
                    .map(|edit| edit.buffer.path().to_path_buf())
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| case.path.clone());
        if case.expect.as_deref() == Some("unconfigured") {
            unconfigured_bar(any, window, cx, pane, id, started).await;
            continue;
        }
        let clean = case.expect.as_deref() == Some("clean");
        let mut ready_ms = None;
        let mut diagnosed_ms = None;
        let mut last = String::new();
        while started.elapsed() < Duration::from_secs(180) {
            notify_and_draw(any, window, cx);
            // 波線は manager の表ではなく、画面が持つ写し（`apply_lsp_diagnostics` が差し替える
            // `edit.diagnostics`）から描く。写しに届いたのを待ってから撮る（表だけを見ると、
            // 写しが差し替わる前のフレームを撮りうる = 実測で HTML の 1 枚がそうなった）
            let (server, count) = window
                .update(cx, |app, _, _| {
                    (app.lsp_document_server(pane), drawn(app, pane))
                })
                .unwrap_or((None, 0));
            if let Some(server) = &server {
                last = format!(
                    "state={:?} loading={} reason={:?}",
                    server.state, server.loading, server.reason
                );
                if server.state == ServerState::Running && !server.loading && ready_ms.is_none() {
                    ready_ms = Some(started.elapsed().as_millis());
                }
            }
            if count > 0 && diagnosed_ms.is_none() {
                diagnosed_ms = Some(started.elapsed().as_millis());
            }
            if ready_ms.is_some() && (diagnosed_ms.is_some() || clean) {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
        let (count, table) = window
            .update(cx, |app, _, _| {
                (drawn(app, pane), app.lsp.diagnostics_count(&path))
            })
            .unwrap_or((0, 0));
        println!(
            "TAKO_VISUAL_PIXEL: {LABEL} id={id} ready_ms={ready_ms:?} diagnosed_ms={diagnosed_ms:?} drawn={count} table={table} {last}"
        );
        check(
            ready_ms.is_some(),
            &format!("visual-test {LABEL} {id}: 稼働して読み込みが済む（{last}）"),
        );
        if !clean {
            check(
                count > 0,
                &format!("visual-test {LABEL} {id}: 誤りの診断が届く（波線）"),
            );
            notify_and_draw(any, window, cx);
            dump(any, cx, &format!("{id}-diag.png"));
        }
        if case.expect.as_deref() == Some("diag") {
            continue;
        }
        // ③ ホバー: CLI と同じ問い合わせで答えの中身を確かめてから、実 GUI のマウス経路でカードを出す
        let manager = window
            .update(cx, |app, _, _| app.lsp.clone())
            .unwrap_or_else(|_| fail("visual-test lsp-servers: manager"));
        let (line, column) = (case.line, case.column);
        let asked = std::time::Instant::now();
        // 読み込みの直後は空で答えるサーバがある（スキーマ・索引が後から揃う）。
        // 中身が出るまで 0.5 秒おきに問い直す（上限 15 秒。出ればすぐ抜ける）
        let mut answer = Err(tako_control::lsp::HoverError::NotOpen);
        let mut text = String::new();
        for _ in 0..30 {
            let manager = manager.clone();
            let hover_path = path.clone();
            answer = cx
                .background_executor()
                .spawn(async move {
                    manager.hover(&tako_control::lsp::HoverRequest {
                        path: hover_path,
                        line,
                        column,
                        timeout: Duration::from_secs(60),
                        document: None,
                        superseding: false,
                        open: true,
                        ticket: None,
                    })
                })
                .await;
            text = answer
                .as_ref()
                .ok()
                .and_then(|a| a.content.as_ref())
                .map(|c| c.value.clone())
                .unwrap_or_default();
            if !text.is_empty() {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
        }
        println!(
            "TAKO_VISUAL_PIXEL: {LABEL} id={id} hover={} chars={} in {}ms head={:?}",
            answer
                .as_ref()
                .map_or_else(|e| e.status().to_string(), |_| "ok".into()),
            text.chars().count(),
            asked.elapsed().as_millis(),
            text.lines().next().unwrap_or_default()
        );
        check(
            !text.is_empty(),
            &format!("visual-test {LABEL} {id}: ホバーの答えに中身がある"),
        );
        if clean {
            clean_after_answer(any, window, cx, pane, &path, id).await;
        }
        let Some(at) = hover_point(window, cx, pane, line, column) else {
            fail(&format!(
                "visual-test {LABEL} {id}: ホバーの位置が描かれていない"
            ))
        };
        let (shown, retried) = hover_show(any, window, cx, at, 600).await;
        println!("TAKO_VISUAL_PIXEL: {LABEL} id={id} card={shown:?} retried={retried}");
        check(
            shown.is_some(),
            &format!("visual-test {LABEL} {id}: ホバーのカードが出る"),
        );
        dump(any, cx, &format!("{id}-hover.png"));
        // 次の件へ移る前にカードを閉じる（語でもカードでもない所へ動かす）
        hover_move(any, cx, point(px(2.0), px(2.0)));
        notify_and_draw(any, window, cx);
    }
}

/// 誤りの無いファイル（#2113 の `clean`）: サーバが文書を読んだ（ホバーの答えが出た）後、診断が届くのを
/// 待つ時間（[`CLEAN_SETTLE`]）のあいだ画面の写しも表も 0 件のまま。その画面を撮る
async fn clean_after_answer(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    path: &std::path::Path,
    id: &str,
) {
    const LABEL: &str = "lsp-servers";
    let since = std::time::Instant::now();
    let mut most = (0, 0);
    while since.elapsed() < CLEAN_SETTLE {
        notify_and_draw(any, window, cx);
        let now = window
            .update(cx, |app, _, _| {
                (drawn(app, pane), app.lsp.diagnostics_count(path))
            })
            .unwrap_or((0, 0));
        most = (most.0.max(now.0), most.1.max(now.1));
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} id={id} clean_settle_ms={} drawn_max={} table_max={}",
        CLEAN_SETTLE.as_millis(),
        most.0,
        most.1
    );
    check(
        most == (0, 0),
        &format!("visual-test {LABEL} {id}: 誤りの無いファイルに波線が出ない（{most:?}）"),
    );
    notify_and_draw(any, window, cx);
    dump(any, cx, &format!("{id}-clean.png"));
}

/// `clean` の件で、答えが出てから波線が出ないのを見続ける時間（ALS は clangd の診断を答えの後に
/// まとめて送ることがある = 余裕を持たせる）
const CLEAN_SETTLE: Duration = Duration::from_secs(8);

/// 起こす前提が未設定（#1951）の件: 状態が unconfigured になり、帯（理由・次の一手・確かめ直すボタン）が
/// 描かれるまで待って撮る。ボタンの矩形（`preview-lsp-action-<pane>`）が描画で記録されたら帯が出ている
async fn unconfigured_bar(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    id: &str,
    started: std::time::Instant,
) {
    use tako_core::lsp::state::ServerState;
    const LABEL: &str = "lsp-servers";
    let probe = format!("preview-lsp-action-{}", pane.as_u64());
    let mut seen = None;
    let mut last = String::new();
    while started.elapsed() < Duration::from_secs(60) {
        notify_and_draw(any, window, cx);
        let (server, drawn_bar) = window
            .update(cx, |app, _, _| {
                (
                    app.lsp_document_server(pane),
                    app.panel_click_probe_bounds.borrow().contains_key(&probe),
                )
            })
            .unwrap_or((None, false));
        if let Some(server) = &server {
            last = format!(
                "state={:?} reason={:?} next_step={:?}",
                server.state, server.reason, server.next_step
            );
            if server.state == ServerState::Unconfigured
                && server.reason.is_some()
                && server.next_step.is_some()
                && drawn_bar
            {
                seen = Some(started.elapsed().as_millis());
                break;
            }
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    println!("TAKO_VISUAL_PIXEL: {LABEL} id={id} unconfigured_ms={seen:?} {last}");
    check(
        seen.is_some(),
        &format!("visual-test {LABEL} {id}: 前提が未設定の帯が描かれる（{last}）"),
    );
    dump(any, cx, &format!("{id}-unconfigured.png"));
}
