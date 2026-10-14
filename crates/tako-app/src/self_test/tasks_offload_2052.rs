//! visual-test `tasks-offload`（Issue #2052）: tasks の残りの UI スレッド待ち。
//!
//! 置き場の読みは注入 `TAKO_2031_INJECT=slow:<ミリ秒>` で遅らせる（書き換えも中で読むので遅れる。
//! CPU は焼かない）。右パネルの 2 秒ループの一覧更新はこの節では撃たない（`tick_user_tasks` を
//! 呼ばない。パネルを閉じている間はループも撃たない）ので、一覧に載るかどうかは入口が引き直したか
//! だけで決まる。
//!
//! ① コマンドパレットの `panel-tasks` で開いた瞬間に一覧を引き直す（開いた同じ更新の中でジョブが
//!    出る）。着地を待つと、閉じている間に CLI と同じ dispatch で起票したタスクが一覧に載る
//! ② `tako panel --show --view tasks`（IPC の受け口と同じ dispatch → 後処理）でも同じ
//! ③ 完了のボタン（`user_task_set_status`）の UI スレッドの所要が上限（既定 300 ms。
//!    `TAKO_2052_CLICK_LIMIT_MS`）未満。書き換えの最中も UI スレッドの往復の間隔の最大が上限未満。
//!    着地後は一覧から消え、開いていた行が畳まれる
//! ④ エッジ: 書き換えの最中に別の行を開いて打ちかけても、着地がそのフォームを消さない /
//!    無い id の完了は理由を出し、書き換え中の数が 0 へ戻る
//! 結果は `TAKO_VISUAL_2052: …` の 1 行で出す。
//!
//! A/B: `TAKO_2052_LEGACY=1`（修正前）では ① ② ③ で落ちる（`scripts/test-user-task-offload-2052.sh`）。
//! 単独実行は `TAKO_VISUAL_ONLY=tasks-offload`

use super::*;

const SECTION: &str = "tasks-offload";

fn add_request(title: &str) -> tako_control::protocol::Request {
    tako_control::protocol::Request::UserTask {
        action: "add".to_string(),
        id: None,
        title: Some(title.to_string()),
        body: Some("#2052 の検証".to_string()),
        kind: Some("review".to_string()),
        status: None,
        all: None,
        project: Some("st2052".to_string()),
        attachments: None,
        copy_texts: None,
        links: None,
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: None,
        caller_role: Some("orchestrator-master:st2052".to_string()),
    }
}

/// CLI と同じ dispatch で起票して id を返す
fn add_task(app: &mut TakoApp, title: &str) -> Option<String> {
    tako_control::dispatch(app, add_request(title), PaneOrigin::Cli)
        .ok()
        .and_then(|v| v["id"].as_str().map(str::to_string))
}

fn listed(app: &TakoApp, id: &str) -> bool {
    app.user_tasks.snapshot.tasks.iter().any(|t| t.id == id)
}

/// 右パネルを閉じ、閉じている間に起票する（一覧はまだ古いまま = 載っていない）。
/// 閉じる前に撃たれた一覧更新が起票の後に読むと載ってしまうので、閉じてから着地を待ち、
/// その後で起票する（閉じている間は 2 秒ループも撃たない）
async fn close_and_add(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, title: &str) -> String {
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.panel_visible = false;
        app.panel_view = PanelView::Fleet;
        cx.notify();
    });
    if !settle_user_tasks(window, cx).await {
        fail(&format!("visual-test {SECTION}: 前のジョブが着地しない"));
    }
    window
        .update(cx, |app: &mut TakoApp, _, _| add_task(app, title))
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: タスクを起票できない")))
}

pub(super) async fn tasks_offload_visual(
    _any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::{PanelViewWire, Request};
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let legacy = tako_core::probe::issue2052_legacy();
    let limit_ms: u128 = std::env::var("TAKO_2052_CLICK_LIMIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    // 前提: 一度開いて一覧を持たせる（閉じている間に増えたぶんが「古い一覧に無い」を作る）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.panel_visible = true;
        app.panel_view = PanelView::Tasks;
        app.refresh_user_tasks();
        cx.notify();
    });

    // ① コマンドパレットの `panel-tasks`
    let p_id = close_and_add(window, cx, "#2052 パレットで開く").await;
    let (p_before, p_issued) = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let before = listed(app, &p_id);
            let requested = app.user_tasks.list_poll.requested;
            app.palette_execute(PaletteItem::Command("tasks", "panel-tasks"), cx);
            (
                before,
                app.panel_visible
                    && app.panel_view == PanelView::Tasks
                    && app.user_tasks.list_poll.requested > requested,
            )
        })
        .unwrap_or((true, false));
    let _ = settle_user_tasks(window, cx).await;
    let p_listed = window
        .update(cx, |app: &mut TakoApp, _, _| listed(app, &p_id))
        .unwrap_or(false);

    // ② `tako panel --show --view tasks`（IPC の受け口と同じ: dispatch → 後処理）
    let t_id = close_and_add(window, cx, "#2052 tako panel で開く").await;
    let (t_before, t_issued) = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let before = listed(app, &t_id);
            let requested = app.user_tasks.list_poll.requested;
            let mut result = tako_control::dispatch(
                app,
                Request::Panel {
                    visible: Some(true),
                    width: None,
                    view: Some(PanelViewWire::Tasks),
                    filetree: None,
                    sidebar_width: None,
                    show_hidden: None,
                },
                PaneOrigin::Cli,
            );
            let _ = app.after_dispatch(&mut result, false, cx);
            (
                before,
                result.is_ok() && app.user_tasks.list_poll.requested > requested,
            )
        })
        .unwrap_or((true, false));
    let _ = settle_user_tasks(window, cx).await;
    let t_listed = window
        .update(cx, |app: &mut TakoApp, _, _| listed(app, &t_id))
        .unwrap_or(false);

    // ③ 完了のボタン: UI スレッドの所要と、書き換えの最中の UI スレッドの往復の間隔
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.user_tasks.expanded = Some(p_id.clone());
        cx.notify();
    });
    let click_ms = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let t0 = std::time::Instant::now();
            app.user_task_set_status(&p_id, "done", cx);
            t0.elapsed().as_millis()
        })
        .unwrap_or(u128::MAX);
    let started = std::time::Instant::now();
    let mut prev = started;
    let mut max_gap = Duration::ZERO;
    loop {
        let idle = window
            .update(cx, |app: &mut TakoApp, _, _| {
                app.user_tasks.status_inflight == 0 && app.user_tasks.list_poll.inflight == 0
            })
            .unwrap_or(true);
        let now = std::time::Instant::now();
        max_gap = max_gap.max(now - prev);
        prev = now;
        if idle || started.elapsed() > Duration::from_secs(20) {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    let (done_gone, collapsed) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (!listed(app, &p_id), app.user_tasks.expanded.is_none())
        })
        .unwrap_or((false, false));

    // ④ エッジ a: 書き換えの最中に別の行（t）を開いて打ちかけても、着地がそのフォームを消さない
    let draft = "#2052 打ちかけ";
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.user_tasks.expanded = None;
        app.user_tasks.reset_form();
        let other = add_task(app, "#2052 書き換え中に閉じる");
        if let Some(other) = other {
            app.user_tasks.expanded = Some(other.clone());
            app.user_task_set_status(&other, "dismiss", cx);
        }
        // 着地より先に別の行を開いて打つ（旧い腕は書き換えが済んでから来る = 同じ結果）
        app.user_tasks.toggle(&t_id);
        app.user_tasks.comment.set_text(draft.to_string());
        cx.notify();
    });
    let _ = settle_user_tasks(window, cx).await;
    let (kept_open, kept_draft) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.user_tasks.expanded.as_deref() == Some(t_id.as_str()),
                app.user_tasks.comment.text() == draft,
            )
        })
        .unwrap_or((false, false));

    // ④ エッジ b: 無い id の完了は理由を出し、書き換え中の数が 0 へ戻る（開いている行は残す）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.remote_notice = None;
        app.user_task_set_status("t-2052-無い", "done", cx);
    });
    let _ = settle_user_tasks(window, cx).await;
    let (missing_notice, missing_inflight, missing_open) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.remote_notice
                    .as_ref()
                    .is_some_and(|n| n.is_error && n.text.contains("t-2052-無い")),
                app.user_tasks.status_inflight,
                app.user_tasks.expanded.as_deref() == Some(t_id.as_str()),
            )
        })
        .unwrap_or((false, 1, false));

    println!(
        "TAKO_VISUAL_2052: arm={} palette=issued:{p_issued}/listed:{p_listed}(before:{p_before}) \
         panel=issued:{t_issued}/listed:{t_listed}(before:{t_before}) click_ms={click_ms} \
         limit_ms={limit_ms} max_gap_ms={} done_gone={done_gone} collapsed={collapsed} \
         race=open:{kept_open}/draft:{kept_draft} missing=notice:{missing_notice}/inflight:{missing_inflight}/open:{missing_open} \
         inject={}",
        if legacy { "legacy" } else { "new" },
        max_gap.as_millis(),
        std::env::var("TAKO_2031_INJECT").unwrap_or_else(|_| "none".into()),
    );

    // 片付け（次の節へフォーカスと右パネルを持ち越さない）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.user_tasks.collapse();
        app.remote_notice = None;
        app.panel_visible = false;
        app.panel_view = PanelView::Fleet;
        cx.notify();
    });

    check(
        !p_before && p_issued && p_listed,
        &format!(
            "① パレットで tasks を開いた瞬間に一覧を引き直す（#2052。前 {p_before} / 開いた更新でジョブ {p_issued} / 着地後に載る {p_listed}）"
        ),
    );
    check(
        !t_before && t_issued && t_listed,
        &format!(
            "② tako panel で tasks を開いた瞬間に一覧を引き直す（#2052。前 {t_before} / ジョブ {t_issued} / 着地後に載る {t_listed}）"
        ),
    );
    check(
        click_ms < limit_ms && max_gap.as_millis() < limit_ms,
        &format!(
            "③ 完了のボタンが UI スレッドで書き換えを待たない（#2052。押した更新 {click_ms} ms・最中の往復の間隔の最大 {} ms / 上限 {limit_ms} ms）",
            max_gap.as_millis()
        ),
    );
    check(
        done_gone && collapsed,
        &format!("③ 着地後は一覧から消え、開いていた行が畳まれる（#2052。消えた {done_gone} / 畳んだ {collapsed}）"),
    );
    check(
        kept_open && kept_draft,
        &format!("④ 書き換えの最中に開いた別の行と打ちかけを着地が消さない（#2052。開いたまま {kept_open} / 打ちかけ {kept_draft}）"),
    );
    check(
        missing_notice && missing_inflight == 0 && missing_open,
        &format!(
            "④ 無い id の完了は理由を出し、書き換え中の数が 0 へ戻る（#2052。理由 {missing_notice} / 数 {missing_inflight} / 開いたまま {missing_open}）"
        ),
    );
}
