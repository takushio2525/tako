//! visual-test `fleet-resident-1997`（FR-2.43.16 / Issue #1997。エピック #1992 の S5）: fleet ビューの worker の
//! 行に常駐メモリ（子孫込み）を描き、**同じ worker の値が CLI `tako orchestrator workers` / MCP
//! `tako_orchestrator_workers` / `tako panel` の `fleet` / 画面の行で同じ**ことを確かめる。
//!
//! 呼び手のスクリプト（`scripts/test-spawn-advice-1997.sh`）がこの GUI を隔離の置き場で起こし、偽の claude
//! （起動を記録して眠るだけ）をプロファイルの PATH の先頭へ置いたプロジェクトを登録して、CLI の置き場を
//! `TAKO_1997_CLI`・プロジェクトの鍵を `TAKO_1997_PROJECT` で渡す:
//!
//! ① master のペインから CLI `orchestrator spawn` で worker を 2 本（応答に `machine` / `advice`）
//! ② fleet ビューを開き、走査の覚えに 2 本が載るまで待つ（`tako queue` の状況で走査を 2 秒ごとに急がせる）
//! ③ 走査が 30 秒ごとへ戻ってから、同じ走査の時刻のまま CLI / MCP / `tako panel` / 画面の写しを読み、
//!    worker ごとの常駐メモリが 4 経路で一致する（新しい走査が挟まった回は読み直す）
//! ④ 画像 `fleet-dark` / `fleet-light`（`TAKO_VISUAL_DUMP_DIR` の下へ `fleet-resident-1997-<場面>.png`）
//!
//! 単独実行は `TAKO_VISUAL_ONLY=fleet-resident-1997`

use super::*;
use serde_json::{json, Value};
use std::process::{Command, Stdio};

const LABEL: &str = "fleet-resident-1997";
/// 状態を待つ上限（走査は急がせた間 2 秒ごと・GUI の写しは 2 秒 tick）
const WAIT: Duration = Duration::from_secs(40);
/// 走査を急がせた後、30 秒ごとへ戻るまで（`resources::WANTED_FOR` = 10 秒 + 余裕）
const SETTLE: Duration = Duration::from_secs(12);

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

/// この GUI の受け口とトークン（発見の置き場 = この隔離 GUI が書いた control.json）
fn control() -> (String, String) {
    let info = tako_control::discovery::read()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 発見の置き場が無い")));
    (info.socket, info.token)
}

/// CLI を background で 1 回走らせる（UI スレッドを塞がない = この GUI が IPC に答えられる）
async fn run_cli(cx: &mut AsyncApp, cli: &str, args: &[&str]) -> (i32, String) {
    let cli = cli.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (socket, token) = control();
    cx.background_executor()
        .spawn(async move {
            let mut c = Command::new(&cli);
            tako_core::platform::process::no_console_window(&mut c);
            let out = c
                .args(&args)
                .stdin(Stdio::null())
                .env("TAKO_SOCKET", socket)
                .env("TAKO_TOKEN", token)
                .env_remove("TAKO_PANE_ID")
                .env_remove("TAKO_ORCHESTRATOR_ROLE")
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
            "visual-test {LABEL} {step}: `tako {}` が {code} で終わった: {}",
            args.join(" "),
            out.chars().take(200).collect::<String>()
        ));
    }
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
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1997","version":"0"}}}"#,
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
            fail(&format!("visual-test {LABEL} {step}: MCP {tool} が断った"));
        }
        let text = msg["result"]["content"][0]["text"].as_str().unwrap_or("");
        return serde_json::from_str(text).unwrap_or_else(|_| json!({ "text": text }));
    }
    fail(&format!("visual-test {LABEL} {step}: MCP {tool} の応答が無い"))
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

/// workers の応答から（ペイン → 常駐メモリ）
fn resident_of(workers: &Value, panes: &[u64]) -> Vec<Option<u64>> {
    panes
        .iter()
        .map(|p| {
            workers["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| w["pane"].as_u64() == Some(*p))
                .and_then(|w| w["resident"]["memory_mb"].as_u64())
        })
        .collect()
}

/// `tako panel` の `fleet` から（ペイン → 常駐メモリ）
fn fleet_of(fleet: &Value, panes: &[u64]) -> Vec<Option<u64>> {
    panes
        .iter()
        .map(|p| {
            fleet["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| w["pane"].as_u64() == Some(*p))
                .and_then(|w| w["resident_mb"].as_u64())
        })
        .collect()
}

pub(super) async fn fleet_resident_1997_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let cli = std::env::var("TAKO_1997_CLI")
        .unwrap_or_else(|_| fail(&format!("visual-test {LABEL}: TAKO_1997_CLI が無い")));
    let project = std::env::var("TAKO_1997_PROJECT").unwrap_or_else(|_| "t1997".into());
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
    let master = ensure_fresh_scene(window, cx, LABEL).await;
    let _ = window.update(cx, |app, _, _| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Title {
                pane: Some(master.as_u64()),
                title: Some("t1997".into()),
                role: Some(format!("orchestrator-master:{project}")),
            },
            PaneOrigin::Cli,
        );
    });

    // ① worker を 2 本（CLI の spawn。応答に machine / advice）
    let master_arg = master.as_u64().to_string();
    let mut workers: Vec<u64> = Vec::new();
    for label in ["a", "b"] {
        let prompt = format!("#1997 {label}");
        let s = cli_json(
            cx,
            &cli,
            &[
                "orchestrator",
                "spawn",
                "--project",
                &project,
                "--prompt",
                &prompt,
                "--pane",
                &master_arg,
                "--label",
                label,
            ],
            &format!("① spawn {label}"),
        )
        .await;
        check(
            s["machine"].is_object() && s["advice"].is_array(),
            &format!("visual-test {LABEL} ① spawn の応答に machine / advice（{label}）"),
        );
        let pane = s["pane_id"]
            .as_u64()
            .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ① spawn の pane_id が無い")));
        println!(
            "TAKO_1997_SPAWN\t{label}\tpane={pane}\tmachine={}\tadvice={}",
            s["machine"], s["advice"]
        );
        workers.push(pane);
    }

    // ② fleet を開き、走査の覚えに 2 本が載るまで（status で走査を 2 秒ごとに急がせる）
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Panel {
                visible: Some(true),
                width: Some(420.0),
                view: Some(tako_control::protocol::PanelViewWire::Fleet),
                filetree: None,
                sidebar_width: None,
                show_hidden: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    let _ = run_cli(cx, &cli, &["queue", "--json"]).await;
    let want = workers.clone();
    if !wait_for_app_state(window, cx, &format!("{LABEL} ② 走査に載る"), WAIT, move |app| {
        want.iter().all(|p| {
            let id = PaneId::from_raw(*p);
            let role = app
                .workspace
                .tabs()
                .iter()
                .find_map(|t| t.tree().get(id))
                .and_then(|p| p.role().map(str::to_string));
            app.worker_resident_mb(id, role.as_deref()).is_some()
        })
    })
    .await
    {
        fail(&format!(
            "visual-test {LABEL} ② worker の常駐メモリが走査に載らない"
        ));
    }

    // ③ 走査が 30 秒ごとへ戻ってから、同じ走査のまま 4 経路で読む
    cx.background_executor().timer(SETTLE).await;
    let mut matched = None;
    for attempt in 1..=4 {
        let before = window
            .update(cx, |app, _, _| app.pane_resident_at)
            .ok()
            .flatten();
        let gui = window
            .update(cx, |app, _, _| app.fleet_state_json())
            .unwrap_or(Value::Null);
        let cli_workers = cli_json(cx, &cli, &["orchestrator", "workers"], "③ CLI").await;
        let mcp_workers = mcp_call(cx, &cli, "tako_orchestrator_workers", json!({}), "③ MCP").await;
        let panel = cli_json(cx, &cli, &["panel"], "③ tako panel").await;
        let after = window
            .update(cx, |app, _, _| app.pane_resident_at)
            .ok()
            .flatten();
        // 写しが最新の走査か（新しい走査が来ていて GUI がまだ写していなければ CLI と食い違いうる）
        let latest = tako_control::resources::pane_resident(None).map(|(at, _)| at);
        let a = fleet_of(&gui, &workers);
        let b = resident_of(&cli_workers, &workers);
        let c = resident_of(&mcp_workers, &workers);
        let d = fleet_of(&panel["fleet"], &workers);
        println!(
            "TAKO_1997_RESIDENT\tattempt={attempt}\tgui={a:?}\tcli={b:?}\tmcp={c:?}\tpanel={d:?}\tcli_age={}\tmcp_age={}\tpanel_age={}",
            cli_workers["resident_age_s"], mcp_workers["resident_age_s"], panel["fleet"]["resident_age_s"]
        );
        let same_scan = before.is_some() && before == after && latest == after;
        if same_scan {
            matched = Some((a, b, c, d));
            break;
        }
        cx.background_executor()
            .timer(Duration::from_secs(3))
            .await;
    }
    let Some((a, b, c, d)) = matched else {
        fail(&format!(
            "visual-test {LABEL} ③ 4 回とも読みの間に新しい走査が挟まった"
        ));
    };
    check(
        a.iter().all(Option::is_some),
        &format!("visual-test {LABEL} ③ 画面の行に 2 本とも常駐メモリ（{a:?}）"),
    );
    check(
        a == b && b == c && c == d,
        &format!(
            "visual-test {LABEL} ③ 同じ worker の常駐メモリが 4 経路で一致（画面 {a:?} / CLI {b:?} / MCP {c:?} / panel {d:?}）"
        ),
    );
    check(
        window
            .update(cx, |app, _, _| app.fleet_state_json()["shown"] == true)
            .unwrap_or(false),
        &format!("visual-test {LABEL} ③ fleet を見ている"),
    );

    // ④ 画像
    dump(any, window, cx, "fleet-dark");
    set_theme(window, cx, "light");
    dump(any, window, cx, "fleet-light");
    set_theme(window, cx, "dark");
    println!("TAKO_1997_FLEET_OK\tworkers={workers:?}\tresident={a:?}");
}
