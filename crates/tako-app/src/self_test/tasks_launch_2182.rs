//! visual-test `tasks-launch-2182`（Issue #2182）: 右パネルの「返す」で起票 master が居ない返答を返したとき、
//! master の起動の組み立て（`master_launch::plan` = CLI の所在確認のログインシェル）を UI スレッドで待たない。
//!
//! 場面: 起票 master（default プロファイル）が居ないタスクを 1 件起票し、右パネルの返答欄を埋めて
//! 「返す」の入口（`user_task_respond` = ボタンが呼ぶのと同じ関数）を呼ぶ。`$SHELL` はスクリプトが
//! 差し替えたスタブで、`command -v claude` のときだけ戻らない（`T2182_SHELL_MODE=hang`）。所在確認は
//! 上限（`TAKO_SETUP_PROBE_TIMEOUT_SECS`。スクリプトが短くする）で打ち切られる。
//!
//! ① 押した更新の UI スレッドの所要と、返している間の UI スレッドの往復の間隔の最大が上限
//!    （既定 300 ms。`TAKO_2182_CLICK_LIMIT_MS`）未満
//! ② 打ち切られたら空のタブを残さず、配送の失敗（master を起動できない）として記録され、欄は空のまま
//! 結果は `TAKO_VISUAL_2182: …` の 1 行で出す。
//!
//! A/B（`TAKO_2182_LEGACY=1` = 修正前）は ① で落ちる（組み立てを UI スレッドで上限まで待つ）。
//! 単独実行は `TAKO_VISUAL_ONLY=tasks-launch-2182`（`scripts/test-respond-master-launch-2182.sh` の visual 段）

use super::*;

const SECTION: &str = "tasks-launch-2182";

/// 返している間の UI スレッドの往復の間隔の最大（`busy` が偽になるか `timeout` まで回す）
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

pub(super) async fn tasks_launch_visual(
    _any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let legacy = tako_core::probe::issue2182_legacy();
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let limit_ms: u128 = std::env::var("TAKO_2182_CLICK_LIMIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    // **本番の todo に書かない**（置き場が隔離されていなければ何もしないで落ちる）
    let Some(store) = tako_control::user_tasks::store_path() else {
        fail(&format!(
            "visual-test {SECTION}: user-tasks.yaml の置き場を解決できない"
        ));
    };
    check(
        std::env::var_os("TAKO_USER_TASKS_FILE").is_some()
            || std::env::var_os("TAKO_ORCHESTRATOR_DIR").is_some(),
        &format!(
            "visual-test {SECTION}: 置き場が隔離されていない（{}）",
            store.display()
        ),
    );

    // 起票元 = default の master（名乗りの `master`）。そのペインは居ない = 返すと起こしに行く
    let (id, tabs_before) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            let added = tako_control::dispatch(
                app,
                tako_control::protocol::Request::UserTask {
                    action: "add".to_string(),
                    id: None,
                    title: Some("#2182 の右パネル".to_string()),
                    body: None,
                    kind: None,
                    status: None,
                    all: None,
                    project: None,
                    attachments: None,
                    copy_texts: None,
                    links: None,
                    due: None,
                    decision: None,
                    comment: None,
                    via: None,
                    pane: None,
                    caller_role: Some("master".to_string()),
                },
                PaneOrigin::Cli,
            );
            let id = added.ok()?["id"].as_str()?.to_string();
            Some((id, app.workspace.tabs().len()))
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 起票できない")));

    // ① 返答欄を埋めて「返す」の入口を呼ぶ
    let click_ms = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            app.user_tasks.comment.clear();
            app.user_tasks.decision = Some("approve".to_string());
            app.task_comment_insert("右パネルから起こして", cx);
            let t0 = std::time::Instant::now();
            app.user_task_respond(&id, cx);
            t0.elapsed().as_millis()
        })
        .unwrap_or(u128::MAX);
    let (max_gap, waited) = wait_while(window, cx, Duration::from_secs(90), |app| {
        app.user_tasks.send_running || !app.user_tasks.sending.is_empty()
    })
    .await;

    // ② 記録と画面
    let shown = tako_control::user_tasks::show_at(&store, &id)
        .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 記録を読めない: {e}")));
    let state = shown["delivery"]["state"]
        .as_str()
        .unwrap_or("-")
        .to_string();
    let reason = shown["delivery"]["reason"]
        .as_str()
        .unwrap_or("-")
        .to_string();
    let responses = shown["responses"].as_array().map_or(0, Vec::len);
    let (tabs_after, comment_empty) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.workspace.tabs().len(),
                app.user_tasks.comment.text().is_empty(),
            )
        })
        .unwrap_or((usize::MAX, false));

    println!(
        "TAKO_VISUAL_2182: arm={} click_ms={click_ms} limit_ms={limit_ms} max_gap_ms={} waited_ms={} \
         responses={responses} state={state} tabs={tabs_before}->{tabs_after} comment_empty={comment_empty} \
         reason={reason}",
        if legacy { "legacy" } else { "new" },
        max_gap.as_millis(),
        waited.as_millis(),
    );

    check(
        click_ms < limit_ms && max_gap.as_millis() < limit_ms,
        &format!(
            "① 右パネルの「返す」が master の起動の組み立てを UI スレッドで待たない（#2182。押した更新 {click_ms} ms・\
             返している間の往復の間隔の最大 {} ms / 上限 {limit_ms} ms）",
            max_gap.as_millis()
        ),
    );
    check(
        responses == 1
            && state == "failed"
            // 枠は組み立ての断りと同じ表示言語（#2365）
            && reason.starts_with(
                &tako_control::Reason::UserTaskMasterLaunchFailed(String::new()).to_string(),
            )
            && tabs_after == tabs_before
            && comment_empty,
        &format!(
            "② 組み立てが打ち切られたら空のタブを残さず配送の失敗として記録する（#2182。返答 {responses} 件・\
             state={state}・タブ {tabs_before}→{tabs_after}・欄が空 {comment_empty}・理由 {reason}）"
        ),
    );
}
