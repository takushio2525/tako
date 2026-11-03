//! visual-test `resources-panel-1995`（FR-2.43.15 / Issue #1995。エピック #1992 の S3）: 右パネルの
//! resources ビューとステータスバーの区画を**実マウスで**押し、CLI / MCP / GUI の 3 経路が同じ順番待ちの
//! 口（`Request::Queue` = `queue::answer` / ペインの既定は UI の dispatch）を通ることを確かめる。
//!
//! 呼び手のスクリプト（`scripts/test-resources-panel-1995.sh`）がこの GUI を**明示の調停役**
//! （`TAKO_QUEUE_SOCKET` + 注入 `TAKO_QUEUE_INJECT` = 同時数 1）で起こし、CLI の置き場を
//! `TAKO_1995_CLI` で渡す。この節は本物の wrapper（`tako queue sleep 600`）を 3 本並べ、
//! 自分が起こした子だけを止める（名前で殺さない）:
//!
//! ① A（ペイン 1）が走り B（ペイン 2）・C（ペインの外 = low）が待つ → ステータスバーの区画が
//!    「実行 1 · 待ち 2」（画像 `statusbar`）
//! ② 区画を押す → resources ビュー（画像 `view-dark`）。画面の行（受付番号・並び・段・誰）=
//!    CLI `tako queue --json` の答え
//! ③ 待ちの行を押す → そのペインへフォーカス（dispatch の `Focus`）
//! ④ C の段のチップ → 「この処理」の high（GUI）→ C が先頭。MCP `tako_queue` で low に戻す → 画面も low。
//!    B のチップ → 「ペイン 2 の既定」の low（GUI = UI の dispatch）→ ペインの模型と CLI の `pane_priority`。
//!    CLI `tako queue priority --pane 2 default` → 画面も戻る
//! ⑤ `tako panel --view resources`（CLI）と MCP `tako_panel` で開き、応答の `resources` が画面の行と同じ
//! ⑥ C の取り消し → 確かめの枠（画像 `confirm`）→ やめる（残る）→ 取り消す（C の wrapper が 125 で終わる）
//! ⑦ ライトテーマ（画像 `view-light`）
//! ⑧ 走っている A を GUI で取り消し（確かめの文が「止めて」）→ B が走る → CLI `tako queue cancel` で B →
//!    空の待ち行列（画像 `empty`）
//! ⑨ 境界（答えを置いて定期の読みを止める）: 機の値の無い OS（画像 `unsupported`）・メモリ圧 critical /
//!    warn / normal の区画の塗りの色を実フレームの画素で読む（画像 `critical`）
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ `resources-panel-1995-<場面>.png`。単独実行は
//! `TAKO_VISUAL_ONLY=resources-panel-1995`

use super::*;
use crate::resources_panel::{read_cached_snapshot, ResourcesSnapshot};
use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use tako_core::queue::Priority;
use tako_core::resources::Pressure;

const LABEL: &str = "resources-panel-1995";
/// 状態を待つ上限（wrapper の poll は 1 秒・定期の読みは 2 秒ごと）
const WAIT: Duration = Duration::from_secs(30);

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

fn probe(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, key: &str) -> Option<Bounds<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds.borrow().get(key).copied()
        })
        .ok()
        .flatten()
}

/// 描き直して、その矩形が今のフレームに在るか（前のフレームの矩形を拾わないよう消してから描く）
fn drawn(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
) -> Option<Bounds<Pixels>> {
    let _ = window.update(cx, |app, _, _| {
        app.panel_click_probe_bounds.borrow_mut().remove(key);
    });
    notify_and_draw(any, window, cx);
    probe(window, cx, key)
}

/// 実マウスで押す（矩形が無ければ名指しで落とす）
fn click(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
    step: &str,
) {
    let at = drawn(any, window, cx, key)
        .map(|b| b.center())
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} {step}: `{key}` の矩形が無い")));
    click_at(any, cx, at);
    notify_and_draw(any, window, cx);
}

fn snap(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> ResourcesSnapshot {
    window
        .update(cx, |app, _, _| app.resources.snapshot.clone())
        .ok()
        .flatten()
        .unwrap_or_default()
}

async fn wait_state<F>(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, step: &str, predicate: F)
where
    F: Fn(&TakoApp) -> bool,
{
    if !wait_for_app_state(window, cx, &format!("{LABEL} {step}"), WAIT, predicate).await {
        let s = snap(window, cx);
        println!(
            "TAKO_RES_1995_STATE {step}: running={:?} waiting={:?}",
            s.queue
                .running
                .iter()
                .map(|j| (j.id, j.priority))
                .collect::<Vec<_>>(),
            s.queue
                .waiting
                .iter()
                .map(|j| (j.id, j.priority, j.priority_from.clone()))
                .collect::<Vec<_>>()
        );
        fail(&format!("visual-test {LABEL} {step}: 状態が来ない"));
    }
    notify_and_draw_soon(window, cx);
}

fn notify_and_draw_soon(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    let _ = window.update(cx, |_, _, cx| cx.notify());
}

/// 待ちの行の受付番号（並び順）
fn waiting_ids(app: &TakoApp) -> Vec<u64> {
    app.resources
        .snapshot
        .as_ref()
        .map(|s| s.queue.waiting.iter().map(|j| j.id).collect())
        .unwrap_or_default()
}

fn running_ids(app: &TakoApp) -> Vec<u64> {
    app.resources
        .snapshot
        .as_ref()
        .map(|s| s.queue.running.iter().map(|j| j.id).collect())
        .unwrap_or_default()
}

fn job_priority(app: &TakoApp, id: u64) -> Option<(Priority, Option<String>)> {
    let s = app.resources.snapshot.as_ref()?;
    s.queue
        .running
        .iter()
        .chain(s.queue.waiting.iter())
        .find(|j| j.id == id)
        .and_then(|j| j.priority.map(|p| (p, j.priority_from.clone())))
}

/// この GUI の受け口とトークン（発見の置き場 = この隔離 GUI が書いた control.json）
fn control() -> (String, String) {
    let info = tako_control::discovery::read()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 発見の置き場が無い")));
    (info.socket, info.token)
}

/// 順番待ちの受け口は**必ず明示の `TAKO_QUEUE_SOCKET`**（この GUI から受け継ぐ）。外すと `tako queue` は
/// 発見の既定の置き場 = 本番の調停役へ並ぶ（#2531）ので、受け継いでいなければ止まる
fn queue_socket() -> String {
    tako_control::queue::explicit_socket().unwrap_or_else(|| {
        fail(&format!(
            "visual-test {LABEL}: TAKO_QUEUE_SOCKET が無い（本番の順番待ちへ並ばないよう止める）"
        ))
    })
}

/// 本物の wrapper（`tako queue sleep`）を起こす。調停役はこの GUI（明示の受け口 + そのトークン）
fn spawn_wrapper(cli: &str, pane: Option<PaneId>, secs: u32) -> Child {
    let (_, token) = control();
    let mut c = Command::new(cli);
    // 製品の起動と同じくコンソール窓を出さない（#628。visual-test は macOS だけで回るが規約に合わせる）
    tako_core::platform::process::no_console_window(&mut c);
    c.args(["queue", "sleep", &secs.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("TAKO_QUEUE_SOCKET", queue_socket())
        .env("TAKO_TOKEN", token)
        .env_remove("TAKO_QUEUE_JOB")
        .env_remove("TAKO_QUEUE_PRIORITY")
        .env_remove("TAKO_ORCHESTRATOR_ROLE")
        .env_remove("TAKO_SOCKET");
    match pane {
        Some(p) => {
            c.env("TAKO_PANE_ID", p.as_u64().to_string());
        }
        None => {
            c.env_remove("TAKO_PANE_ID");
        }
    }
    c.spawn()
        .unwrap_or_else(|e| fail(&format!("visual-test {LABEL}: wrapper を起こせない: {e}")))
}

/// CLI を background で 1 回走らせる（UI スレッドを塞がない = この GUI が IPC に答えられる）。
/// 受け口はこの GUI（`tako queue` は明示の順番待ちの受け口、ほかは本体の受け口）
async fn run_cli(cx: &mut AsyncApp, cli: &str, args: &[&str]) -> (i32, String) {
    let cli = cli.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (socket, token) = control();
    let queue = queue_socket();
    cx.background_executor()
        .spawn(async move {
            let mut c = Command::new(&cli);
            tako_core::platform::process::no_console_window(&mut c);
            let out = c
                .args(&args)
                .stdin(Stdio::null())
                .env("TAKO_QUEUE_SOCKET", queue)
                .env("TAKO_SOCKET", socket)
                .env("TAKO_TOKEN", token)
                .env_remove("TAKO_PANE_ID")
                .output();
            match out {
                Ok(o) => (
                    o.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&o.stdout).to_string(),
                ),
                Err(e) => (-1, e.to_string()),
            }
        })
        .await
}

async fn cli_json(cx: &mut AsyncApp, cli: &str, args: &[&str], step: &str) -> Value {
    let (code, out) = run_cli(cx, cli, args).await;
    if code != 0 {
        fail(&format!(
            "visual-test {LABEL} {step}: `tako {}` が {code} で終わった: {out}",
            args.join(" ")
        ));
    }
    // 誤りの文言は出さない（値を引用しうる = #2230）。出力の先頭を名指しの材料にする
    serde_json::from_str(&out).unwrap_or_else(|_| {
        fail(&format!(
            "visual-test {LABEL} {step}: `tako {}` の出力が JSON でない（先頭: {}）",
            args.join(" "),
            out.chars().take(120).collect::<String>()
        ))
    })
}

/// MCP の stdio ブリッジで 1 回呼ぶ（tools/call の本文 = text を JSON で返す）
async fn mcp_call(cx: &mut AsyncApp, cli: &str, tool: &str, args: Value, step: &str) -> Value {
    let (socket, token) = control();
    let cli = std::path::PathBuf::from(cli);
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": tool, "arguments": args},
    })
    .to_string();
    let lines = cx
        .background_executor()
        .spawn(async move {
            bridge_roundtrip(
                &cli,
                &[("TAKO_SOCKET", socket.as_str()), ("TAKO_TOKEN", token.as_str())],
                &[
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1995","version":"0"}}}"#,
                    &call,
                ],
            )
        })
        .await;
    for line in lines {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if msg["id"] != 2 {
            continue;
        }
        if msg["result"]["isError"] == true {
            fail(&format!(
                "visual-test {LABEL} {step}: MCP {tool} が断った: {line}"
            ));
        }
        let text = msg["result"]["content"][0]["text"].as_str().unwrap_or("");
        return serde_json::from_str(text).unwrap_or_else(|_| json!({ "text": text }));
    }
    fail(&format!(
        "visual-test {LABEL} {step}: MCP {tool} の応答が無い"
    ))
}

/// 画面の行（受付番号・並び・段・誰）が CLI `tako queue --json` の答えと同じか
async fn same_as_cli(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, cli: &str, step: &str) {
    // 画面を CLI の時点に合わせる（2 秒ごとの読みを待たず、同じ要求を投げて着くのを待つ）
    let before = window
        .update(cx, |app, _, _| app.resources.applied)
        .unwrap_or(0);
    let _ = window.update(cx, |app, _, cx| {
        app.resources.pending = true;
        app.drain_resources_pending(cx);
    });
    wait_state(window, cx, &format!("{step} 読み直し"), move |app| {
        app.resources.applied > before && app.resources.inflight == 0
    })
    .await;
    let v = cli_json(cx, cli, &["queue", "--json"], step).await;
    let s = snap(window, cx);
    let rows = |key: &str| -> Vec<(u64, String, String, Option<String>)> {
        v["queue"][key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                (
                    r["id"].as_u64().unwrap_or(0),
                    r["priority"].as_str().unwrap_or("").to_string(),
                    r["priority_from"].as_str().unwrap_or("").to_string(),
                    r["tab"].as_str().map(str::to_string),
                )
            })
            .collect()
    };
    let screen =
        |jobs: &[crate::resources_panel::JobView]| -> Vec<(u64, String, String, Option<String>)> {
            jobs.iter()
                .map(|j| {
                    (
                        j.id,
                        j.priority.map(Priority::as_str).unwrap_or("").to_string(),
                        j.priority_from.clone().unwrap_or_default(),
                        j.tab.clone(),
                    )
                })
                .collect()
        };
    let (cli_run, cli_wait) = (rows("running"), rows("waiting"));
    let (gui_run, gui_wait) = (screen(&s.queue.running), screen(&s.queue.waiting));
    println!("TAKO_RES_1995_CLI\t{step}\trunning={cli_run:?}\twaiting={cli_wait:?}");
    println!("TAKO_RES_1995_GUI\t{step}\trunning={gui_run:?}\twaiting={gui_wait:?}");
    check(
        cli_run == gui_run,
        &format!("visual-test {LABEL} {step}: 実行中の行が CLI と同じ"),
    );
    check(
        cli_wait == gui_wait,
        &format!("visual-test {LABEL} {step}: 待ちの行（並び・段・誰）が CLI と同じ"),
    );
}

/// 子の終わりを待つ（終了コード）
async fn wait_exit(cx: &mut AsyncApp, child: &mut Child, step: &str) -> i32 {
    let started = std::time::Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return status.code().unwrap_or(-1);
        }
        if started.elapsed() > WAIT {
            fail(&format!("visual-test {LABEL} {step}: wrapper が終わらない"));
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}

fn set_theme(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, mode: &str) {
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Theme {
                action: Some("set".into()),
                mode: Some(mode.into()),
                target: None,
                key: None,
                value: None,
                name: None,
                font_family: None,
                font_size: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
}

/// 答えを置いて定期の読みを止める（境界の場面）
fn pin(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, status: Value) {
    let _ = window.update(cx, |app, _, cx| {
        app.resources.pinned = true;
        app.resources.snapshot = Some(ResourcesSnapshot::from_status(&status));
        app.resources.error = None;
        cx.notify();
    });
}

/// 区画の塗りの中央の画素（RGB）
fn fill_pixel(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> [u8; 3] {
    let bounds = drawn(any, window, cx, "statusbar-resources-fill")
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ⑨: 区画の塗りの矩形が無い")));
    let Some((frame, scale)) = capture_frame(any, cx) else {
        fail(&format!("visual-test {LABEL} ⑨: フレームを採れない"));
    };
    let c = bounds.center();
    let x = ((f32::from(c.x) * scale) as u32).min(frame.width().saturating_sub(1));
    let y = ((f32::from(c.y) * scale) as u32).min(frame.height().saturating_sub(1));
    let p = frame.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn near(a: [u8; 3], b: tako_core::Rgb) -> bool {
    a[0].abs_diff(b.r) <= 24 && a[1].abs_diff(b.g) <= 24 && a[2].abs_diff(b.b) <= 24
}

fn machine_status(pressure: Option<&str>, unsupported: bool) -> Value {
    let queue = json!({
        "arbiter": true, "limits": {"max_jobs": 1}, "running": [], "waiting": [], "unmanaged": 0,
    });
    if unsupported {
        return json!({
            "gui": true, "machine": null, "machine_unavailable": "unsupported",
            "heavy": [], "panes": [], "queue": queue,
        });
    }
    json!({
        "gui": true,
        "machine": {
            "memory_mb": {"total": 36864, "used": 30000, "app": 20000, "wired": 4000,
                          "compressed": 6000, "cached": 2000, "available": 6864},
            "swap_mb": {"used": 2048, "total": 4096},
            "pressure": pressure, "cpu_percent": 88.0, "load": [12.0, 11.0, 9.5], "cpus": 18,
        },
        "heavy": [], "panes": [],
        "scan": {"processes": 1200, "readable": 1100, "age_ms": 500},
        "external": {"memory_mb": 4000, "processes": 300, "heavy": 0},
        "queue": queue,
    })
}

pub(super) async fn resources_panel_1995_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let cli = std::env::var("TAKO_1995_CLI")
        .unwrap_or_else(|_| fail(&format!("visual-test {LABEL}: TAKO_1995_CLI が無い")));
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
    set_theme(window, cx, "dark");
    let p1 = ensure_fresh_scene(window, cx, LABEL).await;
    let p2 = window
        .update(cx, |app, _, cx| {
            let mut r = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Split {
                    pane: Some(p1.as_u64()),
                    tab: None,
                    direction: Some(tako_control::protocol::Direction::Right),
                    ratio: None,
                    command: None,
                    cwd: None,
                    focus: Some(false),
                },
                PaneOrigin::Cli,
            );
            let _ = app.after_dispatch(&mut r, false, cx);
            r.ok()
                .and_then(|v| v["pane"].as_u64())
                .map(PaneId::from_raw)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 分割できない")));
    println!(
        "TAKO_RES_1995_PANES\tp1={}\tp2={}",
        p1.as_u64(),
        p2.as_u64()
    );
    // 区画の右に並ぶ ctx / tmux の分も入る幅
    let _ = window.update(cx, |app, _, cx| {
        app.panel_width = 440.0;
        cx.notify();
    });

    // ① wrapper を 3 本: A（ペイン 1）が走り、B（ペイン 2）・C（ペインの外 = low）が待つ
    let mut children: Vec<Child> = Vec::new();
    let a = spawn_wrapper(&cli, Some(p1), 600);
    let id_a = u64::from(a.id());
    children.push(a);
    wait_state(window, cx, "① A が走る", move |app| {
        running_ids(app).contains(&id_a)
    })
    .await;
    let b = spawn_wrapper(&cli, Some(p2), 600);
    let id_b = u64::from(b.id());
    children.push(b);
    wait_state(window, cx, "① B が待つ", move |app| {
        waiting_ids(app).contains(&id_b)
    })
    .await;
    let c = spawn_wrapper(&cli, None, 600);
    let id_c = u64::from(c.id());
    children.push(c);
    wait_state(window, cx, "① C が待つ", move |app| {
        waiting_ids(app) == vec![id_b, id_c]
    })
    .await;
    println!("TAKO_RES_1995_JOBS\ta={id_a}\tb={id_b}\tc={id_c}");
    // 見ていない間（ビューを閉じている）でも区画は背景の覚えの読みで「実行 1 · 待ち 2」
    let bar = snap(window, cx).bar();
    check(
        !window
            .update(cx, |app, _, _| app.resources_view_shown())
            .unwrap_or(true),
        &format!("visual-test {LABEL} ①: ビューは閉じたまま（区画は覚えだけを読む）"),
    );
    check(
        (bar.running, bar.waiting) == (1, 2),
        &format!("visual-test {LABEL} ①: 区画の件数が 実行 1 · 待ち 2"),
    );
    check(
        bar.memory.is_some() == cfg!(target_os = "macos"),
        &format!("visual-test {LABEL} ①: 区画のメモリ（macOS は値・それ以外は未対応）"),
    );
    check(
        drawn(any, window, cx, "statusbar-resources").is_some(),
        &format!("visual-test {LABEL} ①: ステータスバーに区画が描かれる"),
    );
    dump(any, window, cx, "statusbar");

    // ② 区画を押す → resources ビュー
    click(any, window, cx, "statusbar-resources", "②");
    check(
        window
            .update(cx, |app, _, _| app.resources_view_shown())
            .unwrap_or(false),
        &format!("visual-test {LABEL} ②: 区画を押すと resources ビューが開く"),
    );
    same_as_cli(window, cx, &cli, "②").await;
    for key in [
        format!("resources-row-run-{id_a}"),
        format!("resources-row-wait-{id_b}"),
        format!("resources-row-wait-{id_c}"),
    ] {
        check(
            drawn(any, window, cx, &key).is_some(),
            &format!("visual-test {LABEL} ②: 行 `{key}` が描かれる"),
        );
    }
    let a_row = snap(window, cx)
        .queue
        .running
        .first()
        .cloned()
        .unwrap_or_default();
    check(
        a_row.pane == Some(p1.as_u64()) && a_row.tab.is_some(),
        &format!("visual-test {LABEL} ②: 実行中の行の誰（ペイン・タブ）"),
    );
    dump(any, window, cx, "view-dark");

    // ③ 待ちの行を押す → そのペインへ（dispatch の Focus）
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Focus {
                pane: Some(p1.as_u64()),
                direction: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    click(any, window, cx, &format!("resources-row-wait-{id_b}"), "③");
    let focused = window
        .update(cx, |app, _, _| app.focused_pane())
        .unwrap_or(p1);
    println!(
        "TAKO_RES_1995_FOCUS\tclicked=B\tfocused={}",
        focused.as_u64()
    );
    check(
        focused == p2,
        &format!("visual-test {LABEL} ③: 待ちの行を押すとそのペイン（B = ペイン 2）へ移る"),
    );

    // ④ 段のチップ（GUI）→ C を high（この処理）→ 先頭へ
    click(
        any,
        window,
        cx,
        &format!("resources-prio-{id_c}"),
        "④ チップ",
    );
    check(
        window
            .update(cx, |app, _, _| app.resources.priority_open == Some(id_c))
            .unwrap_or(false),
        &format!("visual-test {LABEL} ④: チップを押すと段の選択が開く"),
    );
    dump(any, window, cx, "priority");
    click(
        any,
        window,
        cx,
        &format!("resources-prio-job-{id_c}-high"),
        "④ high",
    );
    wait_state(window, cx, "④ C が high で先頭", move |app| {
        waiting_ids(app) == vec![id_c, id_b]
            && job_priority(app, id_c) == Some((Priority::High, Some("job".into())))
    })
    .await;
    same_as_cli(window, cx, &cli, "④ GUI の high").await;
    // MCP で low に戻す（受付番号の口 = IPC の手前の `queue::answer`）
    let r = mcp_call(
        cx,
        &cli,
        "tako_queue",
        json!({"action": "priority", "id": id_c, "priority": "low"}),
        "④ MCP",
    )
    .await;
    println!("TAKO_RES_1995_MCP_PRIORITY\t{r}");
    wait_state(window, cx, "④ MCP の low が画面へ", move |app| {
        waiting_ids(app) == vec![id_b, id_c]
            && job_priority(app, id_c) == Some((Priority::Low, Some("job".into())))
    })
    .await;
    same_as_cli(window, cx, &cli, "④ MCP の low").await;
    // B のチップ → 「ペイン 2 の既定」の low（GUI = UI の dispatch がペインの模型へ）
    click(
        any,
        window,
        cx,
        &format!("resources-prio-{id_b}"),
        "④ B のチップ",
    );
    click(
        any,
        window,
        cx,
        &format!("resources-prio-pane-{id_b}-low"),
        "④ ペインの low",
    );
    wait_state(window, cx, "④ B がペインの既定 low", move |app| {
        job_priority(app, id_b) == Some((Priority::Low, Some("pane".into())))
    })
    .await;
    let model = window
        .update(cx, |app, _, _| {
            app.workspace
                .pane_anywhere(p2)
                .and_then(|p| p.queue_priority())
        })
        .ok()
        .flatten();
    check(
        model == Some(Priority::Low),
        &format!("visual-test {LABEL} ④: ペイン 2 の模型の優先度の既定が low"),
    );
    let v = cli_json(cx, &cli, &["queue", "--json"], "④ CLI").await;
    let b_cli = v["queue"]["waiting"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["id"].as_u64() == Some(id_b))
        .cloned()
        .unwrap_or_default();
    println!("TAKO_RES_1995_PANE_PRIORITY\t{b_cli}");
    check(
        b_cli["pane_priority"] == "low" && b_cli["priority_from"] == "pane",
        &format!("visual-test {LABEL} ④: CLI の status も pane_priority = low・出どころ pane"),
    );
    // CLI でペインの既定を戻す → 画面も戻る
    let p2s = p2.as_u64().to_string();
    let (code, out) = run_cli(cx, &cli, &["queue", "priority", "--pane", &p2s, "default"]).await;
    println!(
        "TAKO_RES_1995_CLI_PANE_DEFAULT\tcode={code}\t{}",
        out.trim()
    );
    check(
        code == 0,
        &format!("visual-test {LABEL} ④: CLI `tako queue priority --pane N default` が通る"),
    );
    wait_state(window, cx, "④ CLI の default が画面へ", move |app| {
        job_priority(app, id_b).is_some_and(|(_, from)| from.as_deref() != Some("pane"))
    })
    .await;
    same_as_cli(window, cx, &cli, "④ CLI の default").await;

    // ⑤ `tako panel --view resources`（CLI）と MCP `tako_panel` で開く
    let _ = window.update(cx, |app, _, cx| {
        app.panel_visible = false;
        cx.notify();
    });
    let v = cli_json(
        cx,
        &cli,
        &["panel", "--show", "--view", "resources"],
        "⑤ CLI",
    )
    .await;
    println!("TAKO_RES_1995_PANEL_CLI\t{}", v["resources"]);
    check(
        v["view"] == "resources" && v["visible"] == true,
        &format!("visual-test {LABEL} ⑤: CLI の応答の view が resources"),
    );
    check(
        window
            .update(cx, |app, _, _| app.resources_view_shown())
            .unwrap_or(false),
        &format!("visual-test {LABEL} ⑤: CLI `tako panel --view resources` で画面が開く"),
    );
    let _ = window.update(cx, |app, _, cx| {
        app.panel_visible = false;
        cx.notify();
    });
    let m = mcp_call(
        cx,
        &cli,
        "tako_panel",
        json!({"visible": true, "view": "resources"}),
        "⑤ MCP",
    )
    .await;
    println!("TAKO_RES_1995_PANEL_MCP\t{}", m["resources"]);
    check(
        m["view"] == "resources"
            && window
                .update(cx, |app, _, _| app.resources_view_shown())
                .unwrap_or(false),
        &format!("visual-test {LABEL} ⑤: MCP `tako_panel` の view=resources で画面が開く"),
    );
    let ids = |v: &Value, key: &str| -> Vec<u64> {
        v["resources"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect()
    };
    wait_state(window, cx, "⑤ 開いた後の読み", move |app| {
        app.resources.inflight == 0 && !app.resources.pending
    })
    .await;
    let state = window
        .update(cx, |app, _, _| app.resources.state_json())
        .unwrap_or_default();
    let gui_wait: Vec<u64> = state["waiting"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    check(
        ids(&m, "waiting")
            .iter()
            .all(|id| [id_b, id_c].contains(id))
            && gui_wait.len() == 2,
        &format!("visual-test {LABEL} ⑤: 応答の resources が画面の行と同じ受付番号"),
    );

    // ⑥ C の取り消し: 確かめの枠 → やめる（残る）→ 取り消す（125 で終わる）
    click(
        any,
        window,
        cx,
        &format!("resources-cancel-{id_c}"),
        "⑥ 取り消し",
    );
    check(
        window
            .update(cx, |app, _, _| app.resources.confirm_cancel == Some(id_c))
            .unwrap_or(false),
        &format!("visual-test {LABEL} ⑥: 取り消しを押すと確かめの枠が開く（まだ取り消さない）"),
    );
    dump(any, window, cx, "confirm");
    click(
        any,
        window,
        cx,
        &format!("resources-cancel-no-{id_c}"),
        "⑥ やめる",
    );
    check(
        window
            .update(cx, |app, _, _| app.resources.confirm_cancel.is_none())
            .unwrap_or(false),
        &format!("visual-test {LABEL} ⑥: やめると枠が閉じる"),
    );
    same_as_cli(window, cx, &cli, "⑥ やめた後").await;
    check(
        window
            .update(cx, |app, _, _| waiting_ids(app).contains(&id_c))
            .unwrap_or(false),
        &format!("visual-test {LABEL} ⑥: やめたら C は待ちに残る"),
    );
    click(
        any,
        window,
        cx,
        &format!("resources-cancel-{id_c}"),
        "⑥ 取り消し 2",
    );
    click(
        any,
        window,
        cx,
        &format!("resources-cancel-yes-{id_c}"),
        "⑥ 取り消す",
    );
    let code_c = wait_exit(cx, &mut children[2], "⑥ C").await;
    println!("TAKO_RES_1995_CANCEL\tC\texit={code_c}");
    check(
        code_c == 125,
        &format!("visual-test {LABEL} ⑥: 取り消した待ちの wrapper は 125 で終わる"),
    );
    wait_state(window, cx, "⑥ C が消える", move |app| {
        waiting_ids(app) == vec![id_b]
    })
    .await;
    same_as_cli(window, cx, &cli, "⑥ 取り消した後").await;

    // ⑦ ライトテーマ
    set_theme(window, cx, "light");
    dump(any, window, cx, "view-light");
    set_theme(window, cx, "dark");

    // ⑧ 走っている A を GUI で取り消す → B が走る → CLI で B を取り消す → 空
    click(any, window, cx, &format!("resources-cancel-{id_a}"), "⑧ A");
    click(
        any,
        window,
        cx,
        &format!("resources-cancel-yes-{id_a}"),
        "⑧ A 取り消す",
    );
    let code_a = wait_exit(cx, &mut children[0], "⑧ A").await;
    println!("TAKO_RES_1995_CANCEL\tA\texit={code_a}");
    check(
        code_a != 0,
        &format!("visual-test {LABEL} ⑧: 取り消した実行中の wrapper は止まる（0 以外）"),
    );
    wait_state(window, cx, "⑧ B が走る", move |app| {
        running_ids(app) == vec![id_b] && waiting_ids(app).is_empty()
    })
    .await;
    let ids_b = id_b.to_string();
    let (code, out) = run_cli(cx, &cli, &["queue", "cancel", &ids_b]).await;
    println!("TAKO_RES_1995_CLI_CANCEL\tcode={code}\t{}", out.trim());
    let code_b = wait_exit(cx, &mut children[1], "⑧ B").await;
    println!("TAKO_RES_1995_CANCEL\tB\texit={code_b}");
    wait_state(window, cx, "⑧ 空", move |app| {
        running_ids(app).is_empty() && waiting_ids(app).is_empty()
    })
    .await;
    same_as_cli(window, cx, &cli, "⑧ 空").await;
    let bar = snap(window, cx).bar();
    check(
        bar.running + bar.waiting == 0,
        &format!("visual-test {LABEL} ⑧: 空の待ち行列では区画に件数を出さない"),
    );
    dump(any, window, cx, "empty");
    // 背景の覚えの読みと、ビューの読み（CLI と同じ status）が同じ待ち行列を返す
    let cached = cx
        .background_executor()
        .spawn(async { read_cached_snapshot() })
        .await;
    check(
        cached.queue.running.is_empty() && cached.queue.waiting.is_empty() && cached.queue.arbiter,
        &format!("visual-test {LABEL} ⑧: 覚えの読みも空（調停役）"),
    );

    // ⑨ 境界: 機の値の無い OS（未対応）と、メモリ圧ごとの区画の色
    pin(window, cx, machine_status(None, true));
    check(
        drawn(any, window, cx, "statusbar-resources-fill").is_none()
            && drawn(any, window, cx, "statusbar-resources").is_some(),
        &format!("visual-test {LABEL} ⑨: 機の値が無いと区画はバーの代わりに未対応"),
    );
    dump(any, window, cx, "unsupported");
    let theme = window
        .update(cx, |app, _, _| app.theme.clone())
        .unwrap_or_else(|_| fail("theme"));
    for (p, want, name) in [
        ("critical", theme.red, "critical"),
        ("warn", theme.yellow, "warn"),
        ("normal", theme.accent, "normal"),
    ] {
        pin(window, cx, machine_status(Some(p), false));
        let got = fill_pixel(any, window, cx);
        println!(
            "TAKO_RES_1995_COLOR\t{p}\tgot=#{:02x}{:02x}{:02x}\twant=#{:02x}{:02x}{:02x}",
            got[0], got[1], got[2], want.r, want.g, want.b
        );
        check(
            near(got, want),
            &format!("visual-test {LABEL} ⑨: メモリ圧 {p} の区画の塗りの色"),
        );
        let bar = snap(window, cx).bar();
        check(
            bar.pressure
                == Some(match p {
                    "critical" => Pressure::Critical,
                    "warn" => Pressure::Warn,
                    _ => Pressure::Normal,
                }),
            &format!("visual-test {LABEL} ⑨: 置いた答えのメモリ圧が {p}"),
        );
        if name == "critical" {
            dump(any, window, cx, "critical");
        }
    }
    let _ = window.update(cx, |app, _, _| {
        app.resources.pinned = false;
    });
    for mut c in children {
        let _ = c.kill();
        let _ = c.wait();
    }
}
