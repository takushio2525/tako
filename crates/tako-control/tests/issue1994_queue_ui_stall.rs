//! 順番待ちの割り当ては UI の受け口を誰も読まなくても進む（#1994。エピック #1992 の S2）
//!
//! # なぜ要るか
//!
//! 2026-10-10 の本番ハング（#1979）では UI スレッドが 10 分止まった。順番待ちの調停役が
//! UI スレッドの dispatch で答えていたら、その間ビルドの割り当ても全部止まり、待っている
//! `tako queue` は上限（30 分）まで寝る。設計（`.agent/plans/2026-10-resource-manager.md` §4.1）は
//! 「`ipc::submit` が UI の受け口へ渡す前に答える」。
//!
//! # 何を確かめるか（注入）
//!
//! 本物の IPC サーバー（[`IpcServer::start_at`]）を、**受け口（UI の代役）を誰も読まない**状態で立てる
//! （= UI スレッドが固まった状態）。そのうえで
//!
//! 1. 順番待ちと関係ない要求（`list`）は返らない（= 確かに UI が止まっている）
//! 2. 1 件目は割り当てられ、2 件目は同時数の上限で待つ
//! 3. 1 件目が終われば 2 件目が割り当てられる
//! 4. 状況（`status`）も返り、順番待ちの節に実行中が載る
//!
//! 機の値は注入（`TAKO_QUEUE_INJECT`）で与える（負荷は生成しない）。
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_control::ipc::{IncomingRequest, IpcServer};
use tako_control::protocol::{Request, RequestEnvelope, ResponseEnvelope};
use tako_core::queue::{JobKind, JobWire};

const TOKEN: &str = "tk1994-test";

fn roundtrip(socket: &str, request: Request) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
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

fn queue(action: &str, job: Option<JobWire>) -> Request {
    Request::Queue {
        action: Some(action.into()),
        job,
        id: None,
        limits: None,
    }
}

fn wire(pid: u32, at: u64) -> JobWire {
    JobWire {
        id: u64::from(pid),
        enqueued_at_ms: at,
        label: format!("sleep {pid}"),
        kind: JobKind::Other,
        ..JobWire::default()
    }
}

/// 短い一時 dir（sun_path の上限 104 バイトに収める）
fn short_dir() -> PathBuf {
    let dir = PathBuf::from(format!("/tmp/tk1994-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn uiの受け口が固まっても割り当ては進む() {
    let dir = short_dir();
    let inject = dir.join("inject.json");
    // 機: 12 GB（既定の同時数 = 1）・空きは十分・メモリ圧 normal・外の重い処理 0・立ち上げ直後の待ち 0
    std::fs::write(
        &inject,
        json!({
            "total_mb": 12 * 1024, "available_mb": 10 * 1024, "pressure": "normal",
            "swap_used_mb": 0, "cpus": 8, "unmanaged": 0, "regather_secs": 0,
        })
        .to_string(),
    )
    .unwrap();
    std::env::set_var("TAKO_QUEUE_INJECT", &inject);
    std::env::set_var("TAKO_QUEUE_SOCKET", dir.join("q.sock"));

    // UI の受け口: 受け取るが**誰も読まない**（= UI スレッドが固まった状態）
    let (tx, rx) = futures::channel::mpsc::unbounded::<IncomingRequest>();
    let socket = dir.join("q.sock").to_string_lossy().into_owned();
    let _server = IpcServer::start_at(tx, TOKEN.into(), &socket).expect("受け口を張れる");
    tako_control::queue::enable(tako_control::queue::Role::Explicit);

    // 1. 順番待ちと関係ない要求は返らない（確かに UI が止まっている）
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let s2 = socket.clone();
    std::thread::spawn(move || {
        let _ = done_tx.send(roundtrip(&s2, Request::List));
    });
    assert!(
        done_rx.recv_timeout(Duration::from_secs(2)).is_err(),
        "UI の受け口を読まないので list は返らないはず（注入が効いていない）"
    );

    // 生きた pid（wrapper の代役）を 2 本
    let mut a = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut b = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();

    // 2. 1 件目は割り当て、2 件目は同時数で待つ
    let first = roundtrip(&socket, queue("enqueue", Some(wire(a.id(), 100)))).unwrap();
    assert_eq!(first["state"], "granted", "{first}");
    assert_eq!(first["share"], 8, "取り分 = 8 コア ÷ 実行 1");
    let second = roundtrip(&socket, queue("enqueue", Some(wire(b.id(), 200)))).unwrap();
    assert_eq!(second["state"], "waiting", "{second}");
    assert_eq!(second["reason"], "max_jobs");

    // 4. 状況も返る（順番待ちの節に実行中 1・待ち 1）
    let status = roundtrip(&socket, queue("status", None)).unwrap();
    assert_eq!(status["queue"]["arbiter"], true, "{status}");
    assert_eq!(status["queue"]["running"].as_array().unwrap().len(), 1);
    assert_eq!(status["queue"]["waiting"].as_array().unwrap().len(), 1);

    // 3. 1 件目が終われば 2 件目
    let mut started = wire(a.id(), 100);
    started.child_pid = Some(a.id());
    assert_eq!(
        roundtrip(&socket, queue("started", Some(started.clone()))).unwrap()["state"],
        "running"
    );
    started.exit_code = Some(0);
    roundtrip(&socket, queue("done", Some(started))).unwrap();
    let again = roundtrip(&socket, queue("poll", Some(wire(b.id(), 200)))).unwrap();
    assert_eq!(again["state"], "granted", "{again}");

    // 固まった UI の受け口には順番待ちの要求が 1 件も流れていない（届いたのは list だけ）
    let mut rx = rx;
    let mut reached_ui = Vec::new();
    while let Ok(incoming) = rx.try_recv() {
        reached_ui.push(incoming.request.kind_name());
    }
    assert_eq!(reached_ui, vec![Request::List.kind_name()]);
    for c in [&mut a, &mut b] {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
