//! 変化の出来事の購読（`Request::Events` / MCP `tako_events`）は UI の受け口を通らずに答える（#2731）
//!
//! # なぜ要るか
//!
//! 外の購読者が長ポーリング（最大 30 秒）で待つ。UI スレッドの dispatch で待てば、その間 GUI が
//! 丸ごと止まる（#1984 / #2646 / #2647 で減らしてきた専有の逆）。設計は「`ipc::submit` が
//! UI の受け口へ渡す前に答え、待つのは接続のスレッド」（`Request::Queue` = #1994 と同じ形）。
//!
//! # 何を確かめるか（注入）
//!
//! 本物の HTTP MCP サーバー（unix は IPC サーバーも）を、**受け口（UI の代役）を誰も読まない**状態で
//! 立てる（= UI スレッドが固まった状態）。そのうえで
//!
//! 1. 待っている長ポーリングが、ドメインの書き換え（`Pane::set_role`）で起きて `role_changed` を返す
//! 2. 該当が無ければ待ちが尽きて `events: []`・`cursor` は変わらない
//! 3. 上限を超えた `wait_ms` は丸めて応答に載る
//! 4. 9 本目は断らずに待たずに返し、理由が載る（8 本は積まれたら全部起きる）
//! 5. IPC と HTTP MCP の応答が同じ形・同じ値
//! 6. UI の受け口へは 1 件も流れていない
//!
//! プロセスで共有のバッファ（待ちの本数も）を使うので、検査は 1 つのテスト関数の中で順に行う

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};
use tako_control::ipc::IncomingRequest;
use tako_control::mcp::McpServer;
// 実ソケットの往復（unix）だけが使う（#1537）
#[cfg(unix)]
use tako_control::protocol::Request;
use tako_core::events::{MAX_WAITERS, MAX_WAIT_MS};
use tako_core::{Pane, PaneOrigin};

const TOKEN: &str = "tk2731-test";

/// HTTP MCP の `tools/call tako_events`（応答の本文 = ツールの text を JSON として読む）
fn mcp_events(port: u16, args: Value) -> Result<Value, String> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "tako_events", "arguments": args },
    })
    .to_string();
    let mut sock = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    write!(
        sock,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("ヘッダ終端が無い")?;
    let rpc: Value = serde_json::from_slice(&raw[split + 4..]).map_err(|e| e.to_string())?;
    let result = &rpc["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .ok_or_else(|| rpc.to_string())?;
    if result["isError"] == true {
        return Err(text.to_string());
    }
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// 別スレッドで投げ、`limit` まで待って結果を受け取る口（返らない注入でテストを止めない）
fn in_background(
    call: impl FnOnce() -> Result<Value, String> + Send + 'static,
) -> mpsc::Receiver<Result<Value, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(call());
    });
    rx
}

/// 待ちの本数が `n` になるまで回す（時間で待たない。上限は安全網）
fn until_waiters(n: usize) {
    let mut spins: u64 = 0;
    while tako_core::events::log().waiters() < n {
        spins += 1;
        assert!(
            spins < 20_000_000,
            "待ちが {n} 本にならない（いま {}）",
            tako_core::events::log().waiters()
        );
        std::thread::yield_now();
    }
}

/// 出来事が出ないペイン（並走する書き手の出来事で起きないように絞る）
const QUIET_PANE: u64 = u64::MAX - 2731;

#[test]
fn 購読は受け口のスレッドで答え_uiへ流れない() {
    // UI の受け口: 受け取るが**誰も読まない**（= UI スレッドが固まった状態）
    let (tx, rx) = futures::channel::mpsc::unbounded::<IncomingRequest>();
    let server = McpServer::start(tx.clone(), TOKEN.into()).expect("MCP を張れる");
    let port: u16 = server
        .url()
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches("/mcp")
        .parse()
        .expect("ポート");
    // 1. 待っている長ポーリングが role の書き換えで起きる
    let mut pane = Pane::new(PaneOrigin::Mcp);
    let pane_id = pane.id().as_u64();
    let since = tako_core::events::log().cursor();
    let waiting = in_background(move || {
        mcp_events(
            port,
            json!({ "since": since, "panes": [pane_id], "wait_ms": MAX_WAIT_MS }),
        )
    });
    until_waiters(1);
    pane.set_role(Some("orchestrator-master:dev".into()));
    let woke = waiting
        .recv_timeout(Duration::from_secs(20))
        .expect("role の書き換えで起きるはず")
        .expect("応答");
    assert_eq!(woke["events"].as_array().unwrap().len(), 1, "{woke}");
    let event = &woke["events"][0];
    assert_eq!(event["kind"], "role_changed");
    assert_eq!(event["pane"], pane_id);
    assert_eq!(event["data"]["to"], "orchestrator-master:dev");
    assert!(woke["cursor"].as_u64().unwrap() >= event["seq"].as_u64().unwrap());

    // 2. 該当が無ければ待ちが尽きて空・cursor は変わらない（出来事の出ないペインで絞る）
    let quiet = mcp_events(
        port,
        json!({ "since": tako_core::events::log().cursor(), "panes": [QUIET_PANE], "wait_ms": 30 }),
    )
    .expect("応答");
    assert_eq!(quiet["events"], json!([]), "{quiet}");
    assert_eq!(quiet["wait_ms"], 30);
    assert!(quiet.get("wait_skipped").is_none(), "{quiet}");

    // 3. 上限を超えた wait_ms は丸めて応答に載る（該当があるので待たない）
    let clamped = mcp_events(port, json!({ "since": 0, "wait_ms": 60_000 })).expect("応答");
    assert_eq!(clamped["wait_ms"], MAX_WAIT_MS);
    assert_eq!(clamped["wait_clamped"], true);

    // 4. 8 本が待っている間、9 本目は待たずに理由つきで返る
    let start = tako_core::events::log().cursor();
    let watchers: Vec<_> = (0..MAX_WAITERS)
        .map(|_| {
            in_background(move || {
                mcp_events(
                    port,
                    json!({ "since": start, "panes": [pane_id], "wait_ms": MAX_WAIT_MS }),
                )
            })
        })
        .collect();
    until_waiters(MAX_WAITERS);
    let ninth = mcp_events(
        port,
        json!({ "since": start, "panes": [pane_id], "wait_ms": MAX_WAIT_MS }),
    )
    .expect("9 本目も断らない");
    assert_eq!(ninth["wait_skipped"], "too_many_waiters", "{ninth}");
    assert!(ninth["wait_skipped_reason"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(ninth["events"], json!([]));
    pane.set_role(Some("orchestrator-master:review".into()));
    for watcher in watchers {
        let page = watcher
            .recv_timeout(Duration::from_secs(20))
            .expect("8 本とも起きる")
            .expect("応答");
        assert_eq!(
            page["events"][0]["data"]["to"],
            "orchestrator-master:review"
        );
    }
    assert_eq!(
        tako_core::events::log().waiters(),
        0,
        "待ち終えたら枠を返す"
    );

    // 5. IPC と HTTP MCP の応答が同じ（unix の受け口で確かめる。named pipe も同じ `ipc::submit` を通る）
    #[cfg(unix)]
    {
        let scratch = tako_core::test_residue::ScratchDir::new("ev2731");
        let socket = scratch.path().join("t.sock").to_string_lossy().into_owned();
        let _ipc =
            tako_control::ipc::IpcServer::start_at(tx.clone(), TOKEN.into(), &socket).expect("IPC");
        let fixed = tako_core::events::log().cursor();
        let args = json!({ "since": fixed.saturating_sub(3), "panes": [pane_id] });
        let via_mcp = mcp_events(port, args).expect("MCP");
        let via_ipc = ipc_events(
            &socket,
            Request::Events {
                since: Some(fixed.saturating_sub(3)),
                kinds: Vec::new(),
                panes: vec![pane_id],
                wait_ms: None,
            },
        )
        .expect("IPC");
        // cursor は並走するテストの出来事で進みうるので、それ以外の欄で比べる
        let strip = |mut v: Value| {
            v.as_object_mut().unwrap().remove("cursor");
            v
        };
        assert_eq!(strip(via_ipc), strip(via_mcp));
    }

    // 6. UI の受け口へは 1 件も流れていない
    drop(pane);
    let mut rx = rx;
    let mut reached_ui = Vec::new();
    while let Ok(incoming) = rx.try_recv() {
        reached_ui.push(incoming.request.kind_name());
    }
    assert!(reached_ui.is_empty(), "UI へ流れた: {reached_ui:?}");
}

#[cfg(unix)]
fn ipc_events(socket: &str, request: Request) -> Result<Value, String> {
    use std::io::{BufRead, BufReader};
    use tako_control::protocol::{RequestEnvelope, ResponseEnvelope};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).map_err(|e| e.to_string())?;
    let envelope = RequestEnvelope::new(1, TOKEN, request);
    writeln!(stream, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let response: ResponseEnvelope = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match response.error {
        Some(e) => Err(e.message),
        None => Ok(response.result.unwrap_or(Value::Null)),
    }
}
