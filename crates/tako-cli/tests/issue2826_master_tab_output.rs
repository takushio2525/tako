//! 実経路: `tako master --tab` / `tako solo --tab` の出力は、起動行の `Request::Send` の応答が
//! 送達フローへ寄せた形（#2826 の `{"queued": true, "shell_wait": "first_prompt"}`）でも修正前の
//! 即時書き込みの形（`null`）でも**バイト一致**する（Issue #2826）。
//!
//! #2826 で作りたてのペインへの起動行は dispatch が送達フローへ寄せる（判断は
//! `tako_core::shell_send::shell_wait`。経路の単体テストは `dispatch::tests::issue2826_*`）。CLI 側は
//! 何も変えていないので、Send の応答の形が変わっても既定の出力（stdout / stderr）と終了コードが
//! 変わらないこと、起動行が TabNew で作ったペインへ改行つきの素の Send で届くこと（= dispatch の
//! 同じ判断を通る）をここで見る。
//!
//! **ビルドした `tako` をそのまま**叩く。GUI の代わりに要求を記録して決まった応答を返す受け口
//! （偽の GUI）を立てる。本物の claude は起こさない（PATH の先頭の偽物は呼ばれたら印を残すだけ。
//! `tako master` は起動行を送るだけで claude を自分では起こさない）。本番の data dir・GUI へ届かないよう
//! `TAKO_*` を全部外して一時の置き場と偽の GUI の口だけを渡す。unix だけ（偽の GUI が Unix domain socket）

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tako_control::protocol::ResponseEnvelope;
use tako_core::test_residue::ScratchDir;

/// 作ったタブとペイン（TabNew の応答）
const TAB: u64 = 5;
const PANE: u64 = 42;

/// 偽 claude: 呼ばれたら印を残すだけ（`tako master` の preflight は PATH で見つかるかを見る）
const FAKE_CLAUDE: &str = "#!/bin/sh\necho called >> \"$I2826_CLAUDE_LOG\"\nexit 0\n";

struct Sandbox {
    dir: ScratchDir,
}

impl Sandbox {
    fn new() -> Self {
        // 偽の GUI の socket が macOS の上限（104 バイト）に収まるよう短い名前にする
        let dir = ScratchDir::new("i2826m");
        for d in ["data", "disc", "home", "tmp", "bin", "w"] {
            std::fs::create_dir_all(dir.path().join(d)).expect("砂場を作れる");
        }
        std::fs::write(
            dir.path().join("data/settings.json"),
            "{\"language\":\"ja\"}",
        )
        .expect("settings.json を書ける");
        let claude = dir.path().join("bin/claude");
        std::fs::write(&claude, FAKE_CLAUDE).expect("偽 claude を書ける");
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755))
            .expect("実行権を付けられる");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 要求を記録し、メソッドごとに決まった `result` を返す偽の GUI。`send` は起動行の Send の応答
    fn fake_gui(&self, tag: &str, send: Value) -> (PathBuf, Arc<Mutex<Vec<Value>>>) {
        let path = self.path().join(format!("{tag}.sock"));
        let listener = UnixListener::bind(&path).expect("受け口を作れる");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut line = String::new();
                let _ =
                    BufReader::new(stream.try_clone().expect("複製できる")).read_line(&mut line);
                let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let result = match request["method"].as_str() {
                    Some("tab_new") => json!({ "tab": TAB, "pane": PANE }),
                    Some("tab_rename") => json!({ "tab": TAB, "title": "w" }),
                    Some("send") => send.clone(),
                    _ => Value::Null,
                };
                log.lock().unwrap().push(request);
                let reply = ResponseEnvelope::ok(1, result);
                let mut stream = stream;
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&reply).expect("応答を組める")
                );
            }
        });
        (path, seen)
    }

    /// `tako <args>` を偽の GUI へ向けて走らせる（出力の砂場のパスは `<SB>` へ置換）
    fn run(&self, tag: &str, args: &[&str], send: Value) -> (Out, Vec<Value>) {
        let (socket, seen) = self.fake_gui(tag, send);
        let mut command = Command::new(env!("CARGO_BIN_EXE_tako"));
        command
            .args(args)
            .current_dir(self.path().join("w"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        command
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path().join("bin").display()),
            )
            .env("SHELL", "/bin/sh")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("HOME", self.path().join("home"))
            .env("TMPDIR", self.path().join("tmp"))
            .env("TAKO_SOCKET", &socket)
            .env("TAKO_TOKEN", "t")
            .env("I2826_CLAUDE_LOG", self.path().join("claude.log"));
        let out = command.output().expect("tako が終わる");
        let real = std::fs::canonicalize(self.path())
            .expect("砂場の実体")
            .display()
            .to_string();
        let root = self.path().display().to_string();
        let norm = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .replace(&real, "<SB>")
                .replace(&root, "<SB>")
        };
        let requests = seen.lock().unwrap().clone();
        (
            Out {
                code: out.status.code().unwrap_or(-1),
                stdout: norm(&out.stdout),
                stderr: norm(&out.stderr),
            },
            requests,
        )
    }
}

#[derive(Debug, PartialEq)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 起動行の Send（TabNew で作ったペインへ改行つき・素の Send = dispatch の #2826 の判断を通る形）
fn launch_send(requests: &[Value]) -> &Value {
    let sends: Vec<&Value> = requests.iter().filter(|r| r["method"] == "send").collect();
    assert_eq!(sends.len(), 1, "起動行の Send は 1 本: {requests:?}");
    let params = &sends[0]["params"];
    assert_eq!(
        params["pane"], PANE,
        "TabNew で作ったペインへ送る: {params}"
    );
    assert_eq!(params["newline"], true, "改行つき: {params}");
    assert_eq!(params["await_prompt"], false, "素の Send: {params}");
    let methods: Vec<&str> = requests
        .iter()
        .filter_map(|r| r["method"].as_str())
        .collect();
    let tab_new = methods.iter().position(|m| *m == "tab_new");
    let send = methods.iter().position(|m| *m == "send");
    assert!(
        tab_new.is_some() && tab_new < send,
        "TabNew の後に Send: {methods:?}"
    );
    params
}

fn check(command: &str) {
    let sb = Sandbox::new();
    let args = [command, "--tab"];
    let (before, before_requests) = sb.run(&format!("{command}0"), &args, Value::Null);
    let (after, after_requests) = sb.run(
        &format!("{command}1"),
        &args,
        json!({ "queued": true, "pane": PANE, "shell_wait": "first_prompt" }),
    );
    eprintln!(
        "[2826] tako {command} --tab: code={} stdout={:?}\nstderr:\n{}",
        before.code, before.stdout, before.stderr
    );
    assert_eq!(
        before.code, 0,
        "tako {command} --tab が通らない: {before:?}"
    );
    let sent = launch_send(&before_requests);
    assert!(
        sent["text"].as_str().is_some_and(|t| !t.is_empty()),
        "起動行が空: {sent}"
    );
    assert_eq!(launch_send(&after_requests)["text"], sent["text"]);
    assert_eq!(
        before, after,
        "tako {command} --tab の出力が Send の応答の形で変わった（#2826 は CLI の出力を変えない）"
    );
    assert!(
        !sb.path().join("claude.log").exists(),
        "tako {command} が claude を自分で起こした"
    );
}

#[test]
fn master_tabの出力は送信の応答の形に依らずバイト一致() {
    check("master");
}

#[test]
fn solo_tabの出力は送信の応答の形に依らずバイト一致() {
    check("solo");
}
