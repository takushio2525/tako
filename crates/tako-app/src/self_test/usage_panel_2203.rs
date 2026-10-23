//! visual-test `usage-panel-2203`（FR-2.44.13〜15 / Issue #2203。エピック #2192 の S4）: 右パネルの
//! usage ビューを**実マウスで**開いて掘り、画面の行が `tako usage` と同じ関数の答えどおりかを確かめる。
//!
//! 材料（偽の会話記録・カタログ・観測と、それから CLI が作った索引）は呼び手のスクリプト
//! （`scripts/test-usage-panel-2203.sh`）が `TAKO_DATA_DIR` の下に置く。この節は本物の data dir を
//! 読まない・書かない。場面は `TAKO_2203_CASE` で選ぶ:
//!
//! - `main`（既定）:
//!   ① ステータスバーの 7d のメーターを実マウスで押す → usage ビューが 7d・最上段で開く（画像 `statusbar` /
//!   `top`）。画面の行 = 同じ問い合わせを別に `usage_payload` へ投げた答えを同じ書式で組んだもの（字面一致）
//!   ② 先頭のプロジェクトの行を押す → Issue の段（画像 `issue`）③ 先頭の Issue の行を押す → 会話の段
//!   （画像 `session`）④ パンくずの最上段を押して戻る → 期間の 5h を押す（画像 `top-5h`）
//!   ⑤ 狭い幅（最小幅 264px）で会話の段（画像 `narrow`）⑥ `show`（`tako usage --show` と同じ dispatch の
//!   経路 = `prepare_offload`）で Issue の段を開く → `tako panel` の `usage` が同じ段を返す（画像 `show`）
//! - `empty` / `broken` / `huge`: メーターから開いて答えが届くまで待ち、その間の描画の最大の間隔
//!   （UI スレッドが止まれば伸びる）と答えまでの所要を出す（画像 `<case>`）
//!
//! 行は `TAKO_USAGE_ROW` で 1 行ずつ出す（スクリプトが CLI の答えと照合して表にする）。
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ `usage-panel-2203-<場面>.png`。単独実行は
//! `TAKO_VISUAL_ONLY=usage-panel-2203`

use super::*;
use crate::usage_panel::{login_views, UsageLoginView, UsageQuery};
use serde_json::{json, Value};

const LABEL: &str = "usage-panel-2203";

fn dump(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp, name: &str) {
    notify_and_draw(any, window, cx);
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    let Some((frame, _)) = capture_frame(any, cx) else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("{LABEL}-{name}.png"));
    if frame.save(&path).is_ok() {
        println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
    }
}

fn probe_center(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
) -> Option<Point<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds
                .borrow()
                .get(key)
                .map(|b| b.center())
        })
        .ok()
        .flatten()
}

/// 実マウスで押す（矩形が無ければ名指しで落とす）
fn click_probe(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
    step: &str,
) {
    notify_and_draw(any, window, cx);
    let at = probe_center(window, cx, key)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} {step}: `{key}` の矩形が無い")));
    click_at(any, cx, at);
    notify_and_draw(any, window, cx);
}

/// ステータスバーのメーターを出す（隔離 GUI に claude のペインは無いので、mod の報告の代わりに
/// メーターの値だけを置く。2 秒の定期処理が戻すので押す直前に毎回置く）
fn show_meters(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    let _ = window.update(cx, |app, _, cx| {
        app.limit_service = tako_core::LimitService::Claude;
        app.agent_metrics.limit_5h = Some(12);
        app.agent_metrics.limit_week = Some(38);
        cx.notify();
    });
}

/// メーターを実マウスで押して usage ビューを開く（定期処理がメーターを消した瞬間に当たったら押し直す）
fn open_from_meter(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    meter: &str,
    step: &str,
) {
    for _ in 0..5 {
        show_meters(window, cx);
        notify_and_draw(any, window, cx);
        click_probe(any, window, cx, &format!("statusbar-limit-{meter}"), step);
        let opened = window
            .update(cx, |app, _, _| {
                app.panel_visible && app.panel_view == PanelView::Usage
            })
            .unwrap_or(false);
        if opened {
            return;
        }
        // 切り替えのメニューへ落ちていたら閉じて押し直す
        let _ = window.update(cx, |app, _, _| {
            app.limit_service_menu_open = false;
            app.limit_service_menu_anchor = None;
        });
    }
    fail(&format!(
        "visual-test {LABEL} {step}: ステータスバーの {meter} を押しても usage ビューが開かない"
    ));
}

/// いまの段の答えが届くまで待つ。待つ間の描画の最大の間隔（ms）と所要（ms）を返す
async fn wait_result(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    limit: Duration,
    step: &str,
) -> (u128, u128) {
    let start = std::time::Instant::now();
    let mut last = start;
    let mut max_gap = 0u128;
    loop {
        notify_and_draw(any, window, cx);
        let now = std::time::Instant::now();
        max_gap = max_gap.max(now.duration_since(last).as_millis());
        last = now;
        let ready = window
            .update(cx, |app, _, _| {
                let q = app.usage.query();
                app.usage.inflight == 0
                    && !app.usage.pending
                    && app.usage.result.as_ref().is_some_and(|r| {
                        r.query.by == q.by
                            && r.query.window == q.window
                            && r.query.project == q.project
                            && r.query.issue == q.issue
                    })
            })
            .unwrap_or(false);
        if ready {
            // 描画を 1 回通して `rendered` を最新にする
            notify_and_draw(any, window, cx);
            return (max_gap, start.elapsed().as_millis());
        }
        if start.elapsed() > limit {
            fail(&format!(
                "visual-test {LABEL} {step}: usage の答えが {} 秒以内に届かない",
                limit.as_secs()
            ));
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

/// いまの段を読み直す操作そのものの所要（ms。UI スレッドの占有 = 旧経路ならここで集計する）と、
/// 答えを待つ間の描画の最大の間隔・所要
async fn timed_refetch(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    limit: Duration,
    step: &str,
) -> (u128, u128, u128) {
    let started = std::time::Instant::now();
    let _ = window.update(cx, |app, _, cx| {
        app.usage.pending = true;
        app.drain_usage_pending(cx);
    });
    let call = started.elapsed().as_millis();
    let (gap, took) = wait_result(any, window, cx, limit, step).await;
    (call, gap, took)
}

/// いまの段の問い合わせと描いた行
fn snapshot(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) -> (UsageQuery, Vec<UsageLoginView>, Option<Value>, i64) {
    window
        .update(cx, |app, _, _| {
            let r = app.usage.result.as_ref();
            (
                app.usage.query(),
                app.usage.rendered.clone(),
                r.and_then(|r| r.value.as_ref().ok().cloned()),
                r.map_or(0, |r| r.at_unix),
            )
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {LABEL}: 状態を読めない")))
}

/// 段の行を 1 行ずつ出す（スクリプトが CLI と照合する）
fn print_rows(step: &str, query: &UsageQuery, views: &[UsageLoginView]) {
    // 空の欄は `-`（シェルの `read` はタブの連続を 1 つの区切りに潰す）
    let field = |v: &Option<String>| {
        v.clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into())
    };
    println!(
        "TAKO_USAGE_QUERY\t{step}\t{}\t{}\t{}\t{}\t{}",
        field(&query.by),
        field(&query.window),
        field(&query.project),
        field(&query.issue),
        query.limit.unwrap_or(crate::usage_panel::USAGE_ROWS),
    );
    for (li, v) in views.iter().enumerate() {
        println!(
            "TAKO_USAGE_LOGIN\t{step}\t{li}\t{}\t{}\t{}",
            v.title,
            v.limits
                .iter()
                .map(|(k, p)| format!("{k} {p}"))
                .collect::<Vec<_>>()
                .join(" "),
            v.total.as_deref().unwrap_or("")
        );
        for r in &v.rows {
            println!(
                "TAKO_USAGE_ROW\t{step}\t{li}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                serde_json::to_string(&r.key).unwrap_or_default(),
                r.name,
                r.main.as_deref().unwrap_or("-"),
                r.estimated.as_deref().unwrap_or(""),
                r.secondary.as_deref().unwrap_or(""),
                r.usd,
                r.tokens,
                r.sessions,
            );
        }
        if let Some(o) = &v.outside {
            println!("TAKO_USAGE_OUTSIDE\t{step}\t{li}\t{o}");
        }
    }
}

/// 画面の行が、同じ問い合わせを別に `usage_payload`（CLI `tako usage` と同じ関数）へ投げた答えを
/// 同じ書式で組んだものと字面一致するか
async fn same_as_function(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, step: &str) -> usize {
    let (query, rendered, _, at) = snapshot(window, cx);
    let level = window
        .update(cx, |app, _, _| app.usage.current())
        .unwrap_or_else(|_| fail("level"));
    let q = query.clone();
    let fresh =
        cx.background_executor()
            .spawn(async move {
                tako_control::usage::usage_payload(&q, tako_control::usage::Refresh::Skip)
            })
            .await
            .unwrap_or_else(|e| {
                fail(&format!(
                    "visual-test {LABEL} {step}: usage_payload が断った: {e}"
                ))
            });
    let offset = tako_core::platform::clock::local_utc_offset();
    let expected = window
        .update(cx, |_, _, _| login_views(&fresh, &level, at, offset))
        .unwrap_or_default();
    print_rows(step, &query, &rendered);
    if rendered != expected {
        println!("TAKO_USAGE_2203_DIFF {step}: rendered={rendered:#?}\nexpected={expected:#?}");
        fail(&format!(
            "visual-test {LABEL} {step}: 画面の行が同じ関数の答えと違う"
        ));
    }
    rendered.iter().map(|v| v.rows.len()).sum()
}

pub(super) async fn usage_panel_2203_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let case = std::env::var("TAKO_2203_CASE").unwrap_or_else(|_| "main".into());
    println!("TAKO_VISUAL_2203: case={case}");
    let _ = window.update(cx, |app, _, _| {
        app.drawer_visible = false;
        app.panel_visible = false;
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Welcome {
                action: Some("dismiss".into()),
            },
            PaneOrigin::Cli,
        );
    });
    let wait_limit = Duration::from_secs(if case == "huge" { 120 } else { 30 });

    if case != "main" {
        dump(any, window, cx, &format!("{case}-before"));
        open_from_meter(any, window, cx, "7d", "①");
        let _ = window.update(cx, |app, _, cx| {
            app.panel_width = 480.0;
            cx.notify();
        });
        wait_result(any, window, cx, wait_limit, &case).await;
        let (call, gap, took) = timed_refetch(any, window, cx, wait_limit, &case).await;
        println!(
            "TAKO_USAGE_2203_WAIT\t{case}\tfetch_call_ms={call}\tmax_draw_gap_ms={gap}\tanswer_ms={took}"
        );
        let (query, rendered, payload, _) = snapshot(window, cx);
        print_rows(&case, &query, &rendered);
        if let Some(p) = &payload {
            println!(
                "TAKO_USAGE_2203_PAYLOAD\t{case}\tlogins={}\tunreadable={}\tindex_sessions={}",
                p["logins"].as_array().map_or(0, Vec::len),
                p["unreadable_lines"].as_u64().unwrap_or(0),
                p["index"]["sessions"].as_u64().unwrap_or(0),
            );
        }
        dump(any, window, cx, &case);
        // 数万の会話: 掘った先（Issue → 会話 = 1 プロジェクトに数百の会話）でも同じ
        if case == "huge" {
            for (step, row) in [("huge-issue", "②"), ("huge-session", "③")] {
                click_probe(any, window, cx, "usage-row-0", row);
                wait_result(any, window, cx, wait_limit, step).await;
                let (call, gap, took) = timed_refetch(any, window, cx, wait_limit, step).await;
                println!(
                    "TAKO_USAGE_2203_WAIT\t{step}\tfetch_call_ms={call}\tmax_draw_gap_ms={gap}\tanswer_ms={took}"
                );
                let (query, rendered, _, _) = snapshot(window, cx);
                print_rows(step, &query, &rendered);
                dump(any, window, cx, step);
            }
        }
        return;
    }

    // ① ステータスバーの 7d → 最上段（プロジェクト）
    show_meters(window, cx);
    dump(any, window, cx, "statusbar");
    open_from_meter(any, window, cx, "7d", "①");
    let _ = window.update(cx, |app, _, cx| {
        app.panel_width = 520.0;
        cx.notify();
    });
    let state = window
        .update(cx, |app, _, _| {
            (app.usage.window().to_string(), app.usage.levels().len())
        })
        .unwrap_or_default();
    check(
        state == ("7d".to_string(), 1),
        &format!("visual-test {LABEL} ①: 7d・最上段で開く（実際 {state:?}）"),
    );
    let (gap, took) = wait_result(any, window, cx, wait_limit, "①").await;
    println!("TAKO_USAGE_2203_WAIT\ttop\tmax_draw_gap_ms={gap}\tanswer_ms={took}");
    let rows = same_as_function(window, cx, "top").await;
    check(
        rows >= 3,
        &format!("visual-test {LABEL} ①: 最上段に行が 3 つ以上（{rows}）"),
    );
    dump(any, window, cx, "top");

    // ② 先頭のプロジェクトの行 → Issue の段
    click_probe(any, window, cx, "usage-row-0", "②");
    let q = window
        .update(cx, |app, _, _| app.usage.query())
        .unwrap_or_default();
    check(
        q.by.as_deref() == Some("issue") && q.project.is_some(),
        &format!("visual-test {LABEL} ②: 行を押すと Issue の段（実際 {q:?}）"),
    );
    wait_result(any, window, cx, wait_limit, "②").await;
    let rows = same_as_function(window, cx, "issue").await;
    check(
        rows >= 1,
        &format!("visual-test {LABEL} ②: Issue の行がある（{rows}）"),
    );
    dump(any, window, cx, "issue");

    // ③ 先頭の Issue の行 → 会話の段
    click_probe(any, window, cx, "usage-row-0", "③");
    let q = window
        .update(cx, |app, _, _| app.usage.query())
        .unwrap_or_default();
    check(
        q.by.as_deref() == Some("session") && q.issue.is_some(),
        &format!("visual-test {LABEL} ③: Issue の行を押すと会話の段（実際 {q:?}）"),
    );
    wait_result(any, window, cx, wait_limit, "③").await;
    let rows = same_as_function(window, cx, "session").await;
    check(
        rows >= 1,
        &format!("visual-test {LABEL} ③: 会話の行がある（{rows}）"),
    );
    let leaf = window
        .update(cx, |app, _, _| {
            app.usage
                .rendered
                .iter()
                .all(|v| v.rows.iter().all(|r| !r.drillable))
        })
        .unwrap_or(false);
    check(
        leaf,
        &format!("visual-test {LABEL} ③: 会話の行は行き止まり"),
    );
    dump(any, window, cx, "session");

    // ⑤ 狭い幅（最小幅）でも同じ段が描ける
    let _ = window.update(cx, |app, _, cx| {
        app.panel_width = 1.0; // 最小幅へ丸まる（`set_panel` と同じ規則は dispatch 経由で確かめる）
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Panel {
                visible: Some(true),
                width: Some(1.0),
                view: None,
                filetree: None,
                sidebar_width: None,
                show_hidden: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    let width = window
        .update(cx, |app, _, _| app.panel_width)
        .unwrap_or_default();
    check(
        (width - crate::PANEL_MIN_WIDTH).abs() < 0.5,
        &format!("visual-test {LABEL} ⑤: 最小幅へ丸まる（{width}）"),
    );
    dump(any, window, cx, "narrow");

    // ④ パンくずの最上段へ戻る → 5h
    let _ = window.update(cx, |app, _, cx| {
        app.panel_width = 520.0;
        cx.notify();
    });
    click_probe(any, window, cx, "usage-crumb-0", "④");
    let depth = window
        .update(cx, |app, _, _| app.usage.levels().len())
        .unwrap_or(0);
    check(
        depth == 1,
        &format!("visual-test {LABEL} ④: パンくずで最上段へ戻る（{depth}）"),
    );
    click_probe(any, window, cx, "usage-window-5h", "④");
    wait_result(any, window, cx, wait_limit, "④").await;
    let (q, ..) = snapshot(window, cx);
    check(
        q.window.as_deref() == Some("5h"),
        &format!("visual-test {LABEL} ④: 期間が 5h（{q:?}）"),
    );
    same_as_function(window, cx, "top-5h").await;
    dump(any, window, cx, "top-5h");

    // ⑥ `tako usage --show` と同じ経路（`prepare_offload` の show の腕）で Issue の段を開く
    let project =
        std::env::var("TAKO_2203_SHOW_PROJECT").unwrap_or_else(|_| "project:alpha".into());
    let request = tako_control::protocol::Request::Sessions {
        action: "usage".into(),
        id: None,
        role: None,
        project: Some(project.clone()),
        // 画面の既定の行数と同じ（答えと画面の行を字面で比べる）
        limit: Some(crate::usage_panel::USAGE_ROWS),
        pane: None,
        tab: None,
        direction: None,
        by: Some("issue".into()),
        window: None,
        account: None,
        issue: None,
        show: Some(true),
    };
    let shown: std::rc::Rc<std::cell::RefCell<Option<Value>>> = Default::default();
    let sink = shown.clone();
    let _ = window.update(cx, |app, _, cx| {
        app.dispatch_off_ui(request, PaneOrigin::Cli, cx, move |_, r, _| {
            *sink.borrow_mut() = Some(r.unwrap_or_else(|e| json!({ "error": e.to_string() })));
        });
    });
    let start = std::time::Instant::now();
    while shown.borrow().is_none() && start.elapsed() < wait_limit {
        notify_and_draw(any, window, cx);
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    let answer = shown.borrow().clone().unwrap_or(Value::Null);
    check(
        answer["shown"] == true,
        &format!(
            "visual-test {LABEL} ⑥: show の答えに shown: true（{}）",
            answer["shown"]
        ),
    );
    wait_result(any, window, cx, wait_limit, "⑥").await;
    let panel = window
        .update(cx, |app, _, _| {
            tako_control::dispatch(
                app,
                tako_control::protocol::Request::Panel {
                    visible: None,
                    width: None,
                    view: None,
                    filetree: None,
                    sidebar_width: None,
                    show_hidden: None,
                },
                PaneOrigin::Cli,
            )
        })
        .ok()
        .and_then(Result::ok)
        .unwrap_or(Value::Null);
    println!("TAKO_USAGE_2203_PANEL\t{}", panel["usage"]);
    check(
        panel["view"] == "usage"
            && panel["usage"]["by"] == "issue"
            && panel["usage"]["project"] == project.as_str()
            && panel["usage"]["window"] == "7d"
            && panel["usage"]["depth"] == 2,
        &format!(
            "visual-test {LABEL} ⑥: tako panel の usage が show した段（{}）",
            panel["usage"]
        ),
    );
    // show の答え（IPC が返すもの）と画面の行が同じ
    let rendered = window
        .update(cx, |app, _, _| app.usage.rendered.clone())
        .unwrap_or_default();
    let level = window
        .update(cx, |app, _, _| app.usage.current())
        .unwrap_or_else(|_| fail("level"));
    let at = snapshot(window, cx).3;
    let from_answer = login_views(
        &answer,
        &level,
        at,
        tako_core::platform::clock::local_utc_offset(),
    );
    check(
        rendered == from_answer,
        &format!("visual-test {LABEL} ⑥: show の答えと画面の行が同じ"),
    );
    print_rows("show", &snapshot(window, cx).0, &rendered);
    dump(any, window, cx, "show");
}
