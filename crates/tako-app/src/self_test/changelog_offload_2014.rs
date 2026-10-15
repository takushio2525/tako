//! visual-test `changelog-offload`（Issue #2014）: プレビューの「履歴」ボタンが UI スレッドで git を待たない。
//!
//! 場面: 一時リポジトリのファイル（`TAKO_2014_FILE`。3 コミット）のプレビュー。git はスクリプトが
//! PATH の先頭に置いた偽の git で、`TAKO_2014_ARM` のファイルがある間は変更履歴の 2 段
//! （`rev-parse --show-toplevel` / `log --follow`）だけ戻らない（起きた回数を `TAKO_2014_CALLS` へ残す）。
//! 上限は `TAKO_CHANGELOG_GIT_TIMEOUT_SECS`（スクリプトが短くする）。
//!
//! ① 武装して「履歴」ボタン（`toggle_preview_changelog`）を押す: 押した更新の UI スレッドの所要と、
//!    取っている間の UI スレッドの往復の間隔の最大が上限（既定 300 ms。`TAKO_2014_CLICK_LIMIT_MS`）未満
//! ② 取っている間の重ね押しは無視する（取りに行く git は 1 回のまま）
//! ③ 上限に達すると状態は変わらず（履歴ビューに入らない）、通知欄に打ち切りの理由が出る
//! ④ 武装を解いて押すと従来どおり履歴ビュー（3 件）が描かれ、もう一度押すと閉じる
//! 画像は `TAKO_VISUAL_DUMP_DIR` へ `changelog-offload-{timeout,on}.png` で落とす。
//! 結果は `TAKO_VISUAL_2014: …` の 1 行で出す。
//!
//! A/B（`TAKO_2014_LEGACY=1` = 修正前）は ① で落ちる（押した更新が git の戻りまで返らない。
//! スクリプトが外から武装を解くまで UI スレッドが止まる）。
//! 単独実行は `TAKO_VISUAL_ONLY=changelog-offload`（`scripts/test-changelog-offload-2014.sh` の visual 段）

use super::*;

const SECTION: &str = "changelog-offload";

fn env_path(name: &str) -> std::path::PathBuf {
    std::env::var_os(name)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: {name} が無い")))
}

fn calls(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

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

/// 取っている間の UI スレッドの往復の間隔の最大（取り終えるか `timeout` まで回す）
async fn wait_pending(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    timeout: Duration,
) -> (Duration, Duration) {
    let started = std::time::Instant::now();
    let mut prev = started;
    let mut max_gap = Duration::ZERO;
    loop {
        let pending = window
            .update(cx, |app: &mut TakoApp, _, _| {
                app.preview_changelog_pending.contains(&pane)
            })
            .unwrap_or(false);
        let now = std::time::Instant::now();
        max_gap = max_gap.max(now - prev);
        prev = now;
        if !pending || started.elapsed() > timeout {
            return (max_gap, started.elapsed());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

pub(super) async fn changelog_offload_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let legacy = tako_core::probe::issue2014_legacy();
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let limit_ms: u128 = std::env::var("TAKO_2014_CLICK_LIMIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let file = env_path("TAKO_2014_FILE");
    let arm = env_path("TAKO_2014_ARM");
    let calls_file = env_path("TAKO_2014_CALLS");

    // --- 一時リポジトリのファイルを新しいタブのプレビューで開く ---
    let pane = window
        .update(cx, |app, _, cx| {
            let mut made = tako_control::dispatch(
                app,
                tako_control::protocol::Request::TabNew {
                    title: Some("cl2014".into()),
                    focus: Some(true),
                    cwd: None,
                },
                PaneOrigin::User,
            );
            let _ = app.after_dispatch(&mut made, false, cx);
            let shell = made.ok()?["pane"].as_u64()?;
            let mut opened = tako_control::dispatch(
                app,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(shell),
                    path: file.display().to_string(),
                    mode: None,
                    direction: Some(tako_control::protocol::Direction::Right),
                    focus: None,
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            );
            let _ = app.after_dispatch(&mut opened, false, cx);
            app.drain_pending_preview_loads(cx);
            cx.notify();
            opened
                .ok()
                .and_then(|v| v["pane"].as_u64())
                .map(PaneId::from_raw)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: プレビューを開けない")));
    wait_for_preview_state(window, cx, Duration::from_secs(20), |app| {
        app.previews.contains_key(&pane)
    })
    .await
    .unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: プレビューが読み込まれない"
        ))
    });

    // ① 武装して押す（修正前は git の戻り = スクリプトが外から武装を解くまで返らない）
    let calls0 = calls(&calls_file);
    std::fs::write(&arm, "")
        .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 武装できない: {e}")));
    let _ = window.update(cx, |app: &mut TakoApp, _, _| app.remote_notice = None);
    let click_ms = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let t0 = std::time::Instant::now();
            app.toggle_preview_changelog(pane, cx);
            t0.elapsed().as_millis()
        })
        .unwrap_or(u128::MAX);
    // ② 取っている間の重ね押し（無視されて、git は 1 回のまま）
    let (pending_after_click, still_pending) = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let pending = app.preview_changelog_pending.contains(&pane);
            app.toggle_preview_changelog(pane, cx);
            (pending, app.preview_changelog_pending.contains(&pane))
        })
        .unwrap_or((false, false));
    let (max_gap, waited) = wait_pending(window, cx, pane, Duration::from_secs(60)).await;
    let armed_calls = calls(&calls_file).saturating_sub(calls0);
    // ③ 上限に達した後: 状態は変わらず、通知欄に理由
    let (timeout_active, notice) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.preview_changelogs.contains_key(&pane),
                app.remote_notice
                    .as_ref()
                    .filter(|n| n.is_error)
                    .map(|n| n.text.clone()),
            )
        })
        .unwrap_or((true, None));
    notify_and_draw(any, window, cx);
    cx.background_executor()
        .timer(Duration::from_millis(200))
        .await;
    dump(any, cx, "changelog-offload-timeout.png");

    // ④ 武装を解いて押す → 従来どおり履歴ビュー
    let _ = std::fs::remove_file(&arm);
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.remote_notice = None;
        if app.preview_changelogs.contains_key(&pane) {
            // 修正前の腕は ① で開いている（閉じてから押し直す）
            app.toggle_preview_changelog(pane, cx);
        }
        app.toggle_preview_changelog(pane, cx);
    });
    let _ = wait_pending(window, cx, pane, Duration::from_secs(20)).await;
    let (on_active, on_entries, on_repo) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            let data = app.preview_changelogs.get(&pane);
            (
                data.is_some(),
                data.map_or(0, |d| d.entries.len()),
                data.is_some_and(|d| d.repo_root.is_some()),
            )
        })
        .unwrap_or((false, 0, false));
    for _ in 0..5 {
        notify_and_draw(any, window, cx);
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    dump(any, cx, "changelog-offload-on.png");
    // もう一度押すと閉じる
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.toggle_preview_changelog(pane, cx);
    });
    let _ = wait_pending(window, cx, pane, Duration::from_secs(5)).await;
    let off = window
        .update(cx, |app: &mut TakoApp, _, _| {
            !app.preview_changelogs.contains_key(&pane)
        })
        .unwrap_or(false);

    println!(
        "TAKO_VISUAL_2014: arm={} click_ms={click_ms} limit_ms={limit_ms} max_gap_ms={} \
         waited_ms={} pending={pending_after_click}/{still_pending} armed_git_calls={armed_calls} \
         timeout=active:{timeout_active}/notice:{} on=active:{on_active}/entries:{on_entries}/repo:{on_repo} \
         off={off}",
        if legacy { "legacy" } else { "new" },
        max_gap.as_millis(),
        waited.as_millis(),
        notice.as_deref().unwrap_or("-"),
    );

    check(
        click_ms < limit_ms && max_gap.as_millis() < limit_ms,
        &format!(
            "① 「履歴」ボタンが UI スレッドで git を待たない（#2014。押した更新 {click_ms} ms・取っている間の\
             往復の間隔の最大 {} ms / 上限 {limit_ms} ms）",
            max_gap.as_millis()
        ),
    );
    check(
        pending_after_click && still_pending && armed_calls == 1,
        &format!(
            "② 取っている間の重ね押しは無視する（#2014。取っている {pending_after_click} / 重ね押しの後も \
             {still_pending} / 偽の git {armed_calls} 回）"
        ),
    );
    check(
        !timeout_active && notice.as_deref().is_some_and(|t| t.contains("打ち切った")),
        &format!(
            "③ 上限に達すると状態を変えずに理由を知らせる（#2014。履歴ビュー {timeout_active} / 通知 {}）",
            notice.as_deref().unwrap_or("-")
        ),
    );
    check(
        on_active && on_entries == 3 && on_repo && off,
        &format!(
            "④ 武装を解けば従来どおり履歴ビュー（3 件）が開き、押し直すと閉じる（#2014。開いた {on_active} / \
             {on_entries} 件 / リポジトリ {on_repo} / 閉じた {off}）"
        ),
    );
}
