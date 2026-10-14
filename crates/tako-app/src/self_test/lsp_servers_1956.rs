//! visual-test `lsp-servers`（Issue #1956）: 検出表の第 2 波（texlab / tombi / sourcekit-lsp / dart /
//! yaml-language-server）を**実のサーバ**で起こし、誤りの波線とホバーのカードが出た画面を落とす。
//!
//! 素材は `scripts/test-lsp-servers-1956.sh` が用意する（サーバの置き場・ログインシェルの PATH・誤りを
//! 含むファイル）。`TAKO_1956_CASES` は 1 行 1 件 `<ファイル>\t<行>\t<桁>`（ホバーを置く位置。0 起点・
//! 桁はバイト）。受け持つサーバは検出表から引く（言語名・サーバ名をここに書かない）。
//!
//! 1 件ごとに: ① 編集モードで開く → 稼働して読み込みが済む ② 診断が 1 件以上届く（波線。`<id>-diag.png`）
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

/// 1 件（ファイル・ホバーを置く行と桁）
struct Case {
    path: std::path::PathBuf,
    line: usize,
    column: usize,
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
        // ① 稼働して読み込みが済む ② 診断が届く（状態で待つ。上限は初回の索引を見込んで 180 秒）
        let path = case.path.clone();
        let mut ready_ms = None;
        let mut diagnosed_ms = None;
        let mut last = String::new();
        while started.elapsed() < Duration::from_secs(180) {
            notify_and_draw(any, window, cx);
            let (server, count) = window
                .update(cx, |app, _, _| {
                    (
                        app.lsp_document_server(pane),
                        app.lsp.diagnostics_count(&path),
                    )
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
            if ready_ms.is_some() && diagnosed_ms.is_some() {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
        let count = window
            .update(cx, |app, _, _| app.lsp.diagnostics_count(&path))
            .unwrap_or(0);
        println!(
            "TAKO_VISUAL_PIXEL: {LABEL} id={id} ready_ms={ready_ms:?} diagnosed_ms={diagnosed_ms:?} diagnostics={count} {last}"
        );
        check(
            ready_ms.is_some(),
            &format!("visual-test {LABEL} {id}: 稼働して読み込みが済む（{last}）"),
        );
        check(
            count > 0,
            &format!("visual-test {LABEL} {id}: 誤りの診断が届く（波線）"),
        );
        notify_and_draw(any, window, cx);
        dump(any, cx, &format!("{id}-diag.png"));
        // ③ ホバー: CLI と同じ問い合わせで答えの中身を確かめてから、実 GUI のマウス経路でカードを出す
        let manager = window
            .update(cx, |app, _, _| app.lsp.clone())
            .unwrap_or_else(|_| fail("visual-test lsp-servers: manager"));
        let (line, column) = (case.line, case.column);
        let hover_path = case.path.clone();
        let asked = std::time::Instant::now();
        let answer = cx
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
        let text = answer
            .as_ref()
            .ok()
            .and_then(|a| a.content.as_ref())
            .map(|c| c.value.clone())
            .unwrap_or_default();
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
