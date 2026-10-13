//! visual-test `tasks-tick`（Issue #2031）: 右パネル tasks の 2 秒ごとの一覧更新の最中も
//! **返答欄への打鍵が止まらない**か。
//!
//! 場面: 隔離の置き場へタスクを 2 件起票し、tasks ビューを開いて 1 件を展開し、返答欄に
//! フォーカスを置く。そこへ 40 ms ごとに 1 文字ずつ `dispatch_keystroke` で打ち（本番のキー入力と
//! 同じ入口 = `handle_task_comment_key`）、打鍵の間隔の最大値を測る。打鍵のループは UI スレッドで
//! 回るので、2 秒ループの tick が置き場を UI スレッドで読めば、その間だけ間隔が開く。
//! 置き場の読みは注入 `TAKO_2031_INJECT=slow:<ミリ秒>` で遅らせる（CPU は焼かない）。
//!
//! ① 打鍵の間隔の最大値が上限（既定 400 ms。`TAKO_2031_GAP_LIMIT_MS` で変えられる）未満
//! ② 打った文字が全部欄に入る ③ 打っている間に 2 秒ループの一覧更新が 3 回以上撃たれた
//! ④ 置き場の版が変わらない間は読まない（`list_poll.reads` が増えない。A/B の旧い腕では数えない）
//! 結果は `TAKO_VISUAL_2031: …` の 1 行で出す。
//!
//! A/B: `TAKO_2031_LEGACY=1`（修正前 = tick が UI スレッドの同期の dispatch で毎回読む）では
//! ① で落ちる（`scripts/test-tasks-tick-2031.sh`）。単独実行は `TAKO_VISUAL_ONLY=tasks-tick`

use super::*;

const SECTION: &str = "tasks-tick";
/// 打つ時間（2 秒ループが 3 回以上回る長さ）
const TYPE_FOR: Duration = Duration::from_millis(7_000);
/// 打鍵の間隔（人の速めの打鍵）
const KEY_EVERY_MS: u64 = 40;

pub(super) async fn tasks_tick_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_control::protocol::{PanelViewWire, Request};
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let _ = ensure_fresh_scene(window, cx, SECTION).await;
    let legacy = crate::tasks_panel::legacy_2031();
    let limit_ms: u128 = std::env::var("TAKO_2031_GAP_LIMIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);

    // tasks ビューを開き、タスクを起票して 1 件を展開する（起票は CLI と同じ dispatch）
    let target = window
        .update(cx, |app: &mut TakoApp, _, cx| {
            let _ = tako_control::dispatch(
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
            let mut last = None;
            for title in ["#2031 打鍵の検証 A", "#2031 打鍵の検証 B"] {
                last = tako_control::dispatch(
                    app,
                    Request::UserTask {
                        action: "add".to_string(),
                        id: None,
                        title: Some(title.to_string()),
                        body: Some("返答欄へ打ち続ける間に一覧更新が走る".to_string()),
                        kind: Some("review".to_string()),
                        status: None,
                        all: None,
                        project: Some("st2031".to_string()),
                        attachments: None,
                        copy_texts: None,
                        links: None,
                        due: None,
                        decision: None,
                        comment: None,
                        via: None,
                        pane: None,
                        caller_role: Some("orchestrator-master:st2031".to_string()),
                    },
                    PaneOrigin::Cli,
                )
                .ok()
                .and_then(|v| v["id"].as_str().map(str::to_string));
            }
            app.refresh_user_tasks();
            app.user_tasks.expanded = last.clone();
            cx.notify();
            last
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: タスクを起票できない")));
    // 前回の正本を持たせてから測る（初回の 1 回は必ず読む。④ はその後の読みを数える）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| app.spawn_user_tasks_list(cx));
    if !settle_user_tasks(window, cx).await {
        fail(&format!("visual-test {SECTION}: 一覧更新が着地しない"));
    }
    // 返答欄にフォーカスを置く（欄を押したときと同じ手順）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.clear_text_input_focus();
        app.user_tasks.expanded = Some(target.clone());
        app.user_tasks.comment_focused = true;
        app.user_tasks.comment.move_end();
        cx.notify();
    });
    wait(cx, 200).await;
    let (requested0, reads0, len0) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.user_tasks.list_poll.requested,
                app.user_tasks.list_poll.reads,
                app.user_tasks.comment.text().chars().count(),
            )
        })
        .unwrap_or((0, 0, 0));

    // 打つ（UI スレッドで回る。tick が UI スレッドで置き場を読めば、この間隔が開く）
    let started = std::time::Instant::now();
    let mut prev = started;
    let mut max_gap = Duration::ZERO;
    let mut keys = 0usize;
    while started.elapsed() < TYPE_FOR {
        let _ = any.update(cx, |_, window, cx| {
            window.dispatch_keystroke(
                Keystroke {
                    modifiers: Modifiers::default(),
                    key: "a".to_string(),
                    key_char: Some("a".to_string()),
                },
                cx,
            );
        });
        keys += 1;
        let now = std::time::Instant::now();
        max_gap = max_gap.max(now - prev);
        prev = now;
        wait(cx, KEY_EVERY_MS).await;
    }
    let _ = settle_user_tasks(window, cx).await;
    let (requested1, reads1, typed, focused) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.user_tasks.list_poll.requested,
                app.user_tasks.list_poll.reads,
                app.user_tasks.comment.text().chars().count() - len0,
                app.user_tasks.comment_focused,
            )
        })
        .unwrap_or((0, 0, 0, false));
    let ticks = requested1 - requested0;
    let reads = reads1 - reads0;
    println!(
        "TAKO_VISUAL_2031: arm={} max_gap_ms={} limit_ms={limit_ms} keys={keys} typed={typed} \
         ticks={ticks} reads={} inject={}",
        if legacy { "legacy" } else { "new" },
        max_gap.as_millis(),
        if legacy {
            "n/a".to_string()
        } else {
            reads.to_string()
        },
        std::env::var("TAKO_2031_INJECT").unwrap_or_else(|_| "none".into()),
    );

    // 片付け（次の節へフォーカスと右パネルを持ち越さない）
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.user_tasks.comment_focused = false;
        app.user_tasks.reset_form();
        app.user_tasks.collapse();
        app.panel_visible = false;
        app.panel_view = PanelView::Fleet;
        cx.notify();
    });

    check(
        max_gap.as_millis() < limit_ms,
        &format!(
            "① 一覧更新の最中も返答欄への打鍵が止まらない（#2031。打鍵の間隔の最大 {} ms / 上限 {limit_ms} ms）",
            max_gap.as_millis()
        ),
    );
    check(
        focused && typed == keys,
        &format!(
            "② 打った文字が全部欄に入る（#2031。keys={keys} typed={typed} focused={focused}）"
        ),
    );
    check(
        ticks >= 3,
        &format!("③ 打っている間に一覧更新が 3 回以上撃たれた（#2031。ticks={ticks}）"),
    );
    if !legacy {
        check(
            reads == 0,
            &format!("④ 置き場の版が変わらない間は読まない（#2031。reads={reads}）"),
        );
    }
    edges(window, cx).await;
}

/// 一覧更新を 1 回撃って着地まで待ち、(件数, 読んだ回数, 理由の有無) を返す
async fn pull(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> (usize, u64, bool) {
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| app.spawn_user_tasks_list(cx));
    if !settle_user_tasks(window, cx).await {
        fail(&format!("visual-test {SECTION}: 一覧更新が着地しない"));
    }
    window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.user_tasks.snapshot.tasks.len(),
                app.user_tasks.list_poll.reads,
                app.user_tasks.load_error.is_some(),
            )
        })
        .unwrap_or((0, 0, false))
}

/// ⑤〜⑧ エッジ: 別経路が書いた直後 / 更新中にパネルを閉じる / 壊れた置き場 / 置き場が消える
async fn edges(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    let path = tako_control::user_tasks::store_path()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 置き場を解決できない")));
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.panel_visible = true;
        app.panel_view = PanelView::Tasks;
        cx.notify();
    });
    let (n0, r0, _) = pull(window, cx).await;

    // ⑤ 別経路（CLI と同じ dispatch）が書いた直後の tick は 1 回だけ読み、一覧に載る
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::UserTask {
                action: "add".to_string(),
                id: None,
                title: Some("#2031 別経路の書き込み".to_string()),
                body: None,
                kind: None,
                status: None,
                all: None,
                project: Some("st2031".to_string()),
                attachments: None,
                copy_texts: None,
                links: None,
                due: None,
                decision: None,
                comment: None,
                via: None,
                pane: None,
                caller_role: None,
            },
            PaneOrigin::Cli,
        );
        app.tick_user_tasks(cx);
    });
    let _ = settle_user_tasks(window, cx).await;
    let (n1, r1, _) = pull(window, cx).await;
    check(
        n1 == n0 + 1 && r1 == r0 + 1,
        &format!("⑤ 別経路が書いた直後は 1 回だけ読んで一覧に載る（#2031。件数 {n0}→{n1} 読み {r0}→{r1}）"),
    );

    // ⑥ 読んでいる最中にパネルを閉じても落ちず、ジョブは片付く（閉じた後の tick は撃たない）
    let _ = std::fs::write(&path, std::fs::read(&path).unwrap_or_default());
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        // 書き直して版を変えてから撃つ（= 読みの最中）。同じ中身なので一覧は変わらない
        app.spawn_user_tasks_list(cx);
        app.panel_visible = false;
        app.tick_user_tasks(cx);
        cx.notify();
    });
    let settled = settle_user_tasks(window, cx).await;
    let (n2, inflight) = window
        .update(cx, |app: &mut TakoApp, _, _| {
            (
                app.user_tasks.snapshot.tasks.len(),
                app.user_tasks.list_poll.inflight,
            )
        })
        .unwrap_or((0, 1));
    check(
        settled && inflight == 0 && n2 == n1,
        &format!(
            "⑥ 読みの最中にパネルを閉じてもジョブが片付く（#2031。inflight={inflight} 件数 {n2}）"
        ),
    );
    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.panel_visible = true;
        cx.notify();
    });

    // ⑦ 壊れた置き場: 前回の眺めを消さずに理由を出す。直れば読み直して理由が消える
    let good = std::fs::read(&path).unwrap_or_default();
    let _ = std::fs::write(&path, "tasks: [ {{{ #2031 の壊れた置き場");
    let (n3, _, err3) = pull(window, cx).await;
    let _ = std::fs::write(&path, &good);
    let (n4, _, err4) = pull(window, cx).await;
    check(
        n3 == n1 && err3 && n4 == n1 && !err4,
        &format!(
            "⑦ 壊れた置き場は眺めを残して理由を出し、直れば戻る（#2031。壊れた: 件数 {n3} 理由 {err3} / 直した: 件数 {n4} 理由 {err4}）"
        ),
    );

    // ⑧ 置き場が消えた: 空として読み直す（前回の眺めを残さない）
    let _ = std::fs::remove_file(&path);
    let (n5, _, err5) = pull(window, cx).await;
    check(
        n5 == 0 && !err5,
        &format!("⑧ 置き場が消えたら空として読み直す（#2031。件数 {n5} 理由 {err5}）"),
    );
    println!("TAKO_VISUAL_2031_EDGES: wrote={n0}->{n1} reads={r0}->{r1} closed_inflight={inflight} broken={n3}/{err3} fixed={n4}/{err4} gone={n5}/{err5}");

    let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
        app.panel_visible = false;
        app.panel_view = PanelView::Fleet;
        cx.notify();
    });
}
