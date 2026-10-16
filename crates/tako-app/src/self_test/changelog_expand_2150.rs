//! visual-test `changelog-expand`（Issue #2150）: プレビューの変更履歴のコミット行のクリックが UI スレッドで
//! git を待たない。
//!
//! 場面: 一時リポジトリのファイル（`TAKO_2150_FILE`。3 コミット）のプレビューの変更履歴。git はスクリプトが
//! PATH の先頭に置いた偽の git で、`TAKO_2150_ARM` のファイルがある間は行の diff の 2 段
//! （`diff <hash>^..<hash>` / `diff-tree --root -p`）だけ戻らない（起きた回数を `TAKO_2150_CALLS` へ残す）。
//! 上限は `TAKO_CHANGELOG_GIT_TIMEOUT_SECS`（スクリプトが短くする）。
//!
//! ① 武装していちばん新しいコミットの行を押す（`toggle_changelog_diff`）: 押した更新の UI スレッドの所要と、
//!    取っている間の UI スレッドの往復の間隔の最大が上限（既定 300 ms。`TAKO_2150_CLICK_LIMIT_MS`）未満
//! ② 取っている間の同じ行の重ね押しは無視する（取りに行く git は 1 回のまま）
//! ③ 上限に達すると状態は変わらず（行は畳んだまま）、通知欄に打ち切りの理由が出る
//! ④ 武装を解いて押すと従来どおり diff が開き、中身は上限なしの `git::diff_file_commit` とバイト一致。
//!    もう一度押すと畳む
//! 画像は `TAKO_VISUAL_DUMP_DIR` へ `changelog-expand-{timeout,on}.png` で落とす。
//! 結果は `TAKO_VISUAL_2150: …` の 1 行で出す。
//!
//! A/B（`TAKO_2150_LEGACY=1` = 修正前）は ① で落ちる（押した更新が git の戻りまで返らない。
//! スクリプトが外から武装を解くまで UI スレッドが止まる）。
//! 単独実行は `TAKO_VISUAL_ONLY=changelog-expand`（`scripts/test-changelog-expand-2150.sh` の visual 段）

use super::*;

const SECTION: &str = "changelog-expand";

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

/// 取っている間の UI スレッドの往復の間隔の最大（`busy` が偽になるか `timeout` まで回す）
async fn wait_while(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    timeout: Duration,
    busy: impl Fn(&TakoApp) -> bool,
) -> (Duration, Duration) {
    let started = std::time::Instant::now();
    let mut prev = started;
    let mut max_gap = Duration::ZERO;
    loop {
        let pending = window
            .update(cx, |app: &mut TakoApp, _, _| busy(app))
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

/// その行に置かれている diff（畳んでいれば None）
fn expanded(app: &TakoApp, pane: PaneId, hash: &str) -> Option<Vec<tako_core::DiffHunk>> {
    app.preview_changelogs
        .get(&pane)
        .and_then(|d| d.entry(hash))
        .and_then(|e| e.expanded_diff.clone())
}

pub(super) async fn changelog_expand_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let legacy = tako_core::probe::issue2150_legacy();
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let limit_ms: u128 = std::env::var("TAKO_2150_CLICK_LIMIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let file = env_path("TAKO_2150_FILE");
    let arm = env_path("TAKO_2150_ARM");
    let calls_file = env_path("TAKO_2150_CALLS");
    let _ = std::fs::remove_file(&arm);

    // --- 一時リポジトリのファイルを新しいタブのプレビューで開き、変更履歴へ切り替える ---
    let pane = window
        .update(cx, |app, _, cx| {
            let mut made = tako_control::dispatch(
                app,
                tako_control::protocol::Request::TabNew {
                    title: Some("cl2150".into()),
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
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.toggle_preview_changelog(pane, cx);
    });
    let _ = wait_while(window, cx, Duration::from_secs(20), |app| {
        app.preview_changelog_pending.contains(&pane)
    })
    .await;
    let (hash, repo, rel, entries) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            let data = app.preview_changelogs.get(&pane)?;
            Some((
                data.entries.first()?.commit.hash.clone(),
                data.repo_root.clone()?,
                data.rel_path.clone()?,
                data.entries.len(),
            ))
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 変更履歴が開かない")));
    let key = (pane, hash.clone());

    // ① 武装して押す（修正前は git の戻り = スクリプトが外から武装を解くまで返らない）
    let calls0 = calls(&calls_file);
    std::fs::write(&arm, "")
        .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 武装できない: {e}")));
    let _ = window.update(cx, |app: &mut TakoApp, _, _| app.remote_notice = None);
    let click_ms = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let t0 = std::time::Instant::now();
            app.toggle_changelog_diff(pane, &hash, cx);
            t0.elapsed().as_millis()
        })
        .unwrap_or(u128::MAX);
    // ② 取っている間の同じ行の重ね押し（無視されて、git は 1 回のまま）
    let (pending_after_click, still_pending) = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let pending = app.preview_changelog_diff_pending.contains(&key);
            app.toggle_changelog_diff(pane, &hash, cx);
            (pending, app.preview_changelog_diff_pending.contains(&key))
        })
        .unwrap_or((false, false));
    let (max_gap, waited) = wait_while(window, cx, Duration::from_secs(60), |app| {
        app.preview_changelog_diff_pending.contains(&key)
    })
    .await;
    let armed_calls = calls(&calls_file).saturating_sub(calls0);
    // ③ 上限に達した後: 行は畳んだまま、通知欄に理由
    let (timeout_expanded, notice) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                expanded(app, pane, &hash).is_some(),
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
    dump(any, cx, "changelog-expand-timeout.png");

    // ④ 武装を解いて押す → 従来どおり diff が開く（中身は上限なしの版とバイト一致）
    let _ = std::fs::remove_file(&arm);
    let reference = {
        let (repo, rel, hash) = (repo.clone(), rel.clone(), hash.clone());
        cx.background_executor()
            .spawn(async move { tako_core::git::diff_file_commit(&repo, &hash, &rel) })
            .await
    };
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.remote_notice = None;
        if expanded(app, pane, &hash).is_some() {
            // 修正前の腕は ① で開いている（畳んでから押し直す）
            app.toggle_changelog_diff(pane, &hash, cx);
        }
        app.toggle_changelog_diff(pane, &hash, cx);
    });
    let _ = wait_while(window, cx, Duration::from_secs(20), |app| {
        app.preview_changelog_diff_pending.contains(&key)
    })
    .await;
    let on = window
        .update(cx, |app: &mut TakoApp, _, _| expanded(app, pane, &hash))
        .ok()
        .flatten();
    let on_lines: usize = on
        .as_ref()
        .map_or(0, |h| h.iter().map(|h| h.lines.len()).sum());
    let same = on.as_ref() == Some(&reference);
    for _ in 0..5 {
        notify_and_draw(any, window, cx);
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    dump(any, cx, "changelog-expand-on.png");
    // もう一度押すと畳む（git を起こさない = 同期の腕）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.toggle_changelog_diff(pane, &hash, cx);
    });
    let _ = wait_while(window, cx, Duration::from_secs(5), |app| {
        app.preview_changelog_diff_pending.contains(&key)
    })
    .await;
    let folded = window
        .update(cx, |app: &mut TakoApp, _, _| {
            expanded(app, pane, &hash).is_none()
        })
        .unwrap_or(false);

    println!(
        "TAKO_VISUAL_2150: arm={} entries={entries} click_ms={click_ms} limit_ms={limit_ms} max_gap_ms={} \
         waited_ms={} pending={pending_after_click}/{still_pending} armed_git_calls={armed_calls} \
         timeout=expanded:{timeout_expanded}/notice:{} on=lines:{on_lines}/same_as_unbounded:{same} \
         folded={folded}",
        if legacy { "legacy" } else { "new" },
        max_gap.as_millis(),
        waited.as_millis(),
        notice.as_deref().unwrap_or("-"),
    );

    check(
        click_ms < limit_ms && max_gap.as_millis() < limit_ms,
        &format!(
            "① コミット行のクリックが UI スレッドで git を待たない（#2150。押した更新 {click_ms} ms・取っている間の\
             往復の間隔の最大 {} ms / 上限 {limit_ms} ms）",
            max_gap.as_millis()
        ),
    );
    check(
        pending_after_click && still_pending && armed_calls == 1,
        &format!(
            "② 取っている間の同じ行の重ね押しは無視する（#2150。取っている {pending_after_click} / 重ね押しの後も \
             {still_pending} / 偽の git {armed_calls} 回）"
        ),
    );
    check(
        !timeout_expanded && notice.as_deref().is_some_and(|t| t.contains("打ち切った")),
        &format!(
            "③ 上限に達すると行を畳んだまま理由を知らせる（#2150。展開 {timeout_expanded} / 通知 {}）",
            notice.as_deref().unwrap_or("-")
        ),
    );
    check(
        on_lines > 0 && same && folded,
        &format!(
            "④ 武装を解けば従来どおり diff が開き（上限なしの版とバイト一致）、押し直すと畳む（#2150。\
             {on_lines} 行 / 一致 {same} / 畳んだ {folded}）"
        ),
    );
}
