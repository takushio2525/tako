//! #1678 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、
//! tako の LSP クライアントが送るもの・受けて保つもの・プロセスの寿命を測る。
//!
//! A/B（検出力の実証）: `TAKO_1007_LEGACY=1 cargo test -p tako-control --test issue1678_lsp_e2e`
//! で LSP を丸ごと止めると、ここの e2e が FAILED になる。
//!
//! アイドルの観測時間は既定 5 秒。受け入れ条件の 60 秒は
//! `TAKO_LSP_IDLE_TEST_SECS=60` を付けて同じテストで測る。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{text, DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1678-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("src").join("main.rs")
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }

    fn spawns(&self) -> PathBuf {
        self.0.join("spawns.txt")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(scratch: &Scratch, scenario: &str) -> LspConfig {
    let args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--spawns".to_string(),
        scratch.spawns().display().to_string(),
    ];
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: FAKE.to_string(),
                args: args.clone(),
            },
            program_path: FAKE.to_string(),
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn received(scratch: &Scratch) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn methods(scratch: &Scratch) -> Vec<String> {
    received(scratch)
        .iter()
        .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn spawn_pids(scratch: &Scratch) -> Vec<u32> {
    std::fs::read_to_string(scratch.spawns())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {limit:?} 以内に起きなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn server_status(manager: &LspManager) -> Value {
    manager.status(None)["servers"][0].clone()
}

fn state(manager: &LspManager) -> String {
    server_status(manager)["state"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn open(manager: &LspManager, link: &mut DocLink, path: &Path, text: &str, version: u64) {
    manager.sync(link, true, path, text, version);
}

/// 受け入れ条件: 送られたメッセージ列が initialize → initialized → didOpen → didChange →
/// shutdown → exit の順で固定値と一致する
#[test]
fn 送られたメッセージ列が固定値と一致する() {
    let scratch = Scratch::new("sequence");
    let manager = LspManager::new(config(&scratch, "normal"));
    let path = scratch.file();
    let mut link = DocLink::default();
    open(&manager, &mut link, &path, "fn main() {}\n", 0);
    assert!(
        matches!(link, DocLink::Open(_)),
        "対象の拡張子なら開く: {link:?}"
    );
    wait_until("didOpen", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didOpen".to_string())
    });
    // 同じ版を渡しても何も送らない（打鍵のない再描画で送らない）
    open(&manager, &mut link, &path, "fn main() {}\n", 0);
    open(&manager, &mut link, &path, "fn main() { }\n", 1);
    wait_until("didChange", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didChange".to_string())
    });
    manager.stop(None);
    wait_until("exit", Duration::from_secs(10), || {
        methods(&scratch).contains(&"exit".to_string())
    });
    assert_eq!(
        methods(&scratch),
        vec![
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didChange",
            "shutdown",
            "exit",
        ]
    );
    let messages = received(&scratch);
    let init = &messages[0]["params"];
    assert_eq!(
        init["capabilities"]["general"]["positionEncodings"],
        json!(["utf-16"])
    );
    assert_eq!(
        init["rootUri"],
        json!(tako_core::file_uri::from_path(&scratch.0)),
        "ルートは Cargo.toml の場所"
    );
    assert_eq!(init["processId"], json!(std::process::id()));
    let did_open = &messages[2]["params"]["textDocument"];
    assert_eq!(did_open["languageId"], json!("rust"));
    assert_eq!(did_open["version"], json!(0));
    assert_eq!(did_open["text"], json!("fn main() {}\n"));
    assert_eq!(
        did_open["uri"],
        json!(tako_core::file_uri::from_path(&path))
    );
    let did_change = &messages[3]["params"];
    assert_eq!(did_change["textDocument"]["version"], json!(1));
    assert_eq!(
        did_change["contentChanges"],
        json!([{
            "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 11 } },
            "text": " ",
        }])
    );
    assert_eq!(messages[4]["method"], json!("shutdown"));
    wait_until("止めた", Duration::from_secs(10), || {
        state(&manager) == "stopped"
    });
}

/// 受け入れ条件: 偽サーバが Content-Length を 2 回に分けて書いても 1 メッセージとして読める
#[test]
fn ヘッダを2回に分けて書かれても握手できる() {
    let scratch = Scratch::new("split");
    let manager = LspManager::new(config(&scratch, "split-header"));
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("稼働", Duration::from_secs(10), || {
        state(&manager) == "running"
    });
    let status = server_status(&manager);
    assert_eq!(status["server_info"]["name"], json!("tako-lsp-fake"));
    assert_eq!(status["text_document_sync"], json!("incremental"));
    assert_eq!(status["position_encoding"], json!("utf-16"));
    // 分割して届いた publishDiagnostics も 1 件として保持される
    wait_until("診断の受信", Duration::from_secs(10), || {
        manager.diagnostics_count(&scratch.file()) == 1
    });
    assert_eq!(status["garbage_messages"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));
}

#[test]
fn 壊れた_json_を混ぜられても握手できる() {
    let scratch = Scratch::new("garbage");
    let manager = LspManager::new(config(&scratch, "garbage"));
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("稼働", Duration::from_secs(10), || {
        state(&manager) == "running"
    });
    assert_eq!(server_status(&manager)["garbage_messages"], json!(1));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件: 再起動の上限を超えたら「諦めた」で止まり、それ以上 spawn しない
#[test]
fn 握手の直後に落ちるサーバは3回まで起こし直して諦める() {
    let scratch = Scratch::new("crash");
    let manager = LspManager::new(config(&scratch, "crash"));
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("諦めた", Duration::from_secs(20), || {
        state(&manager) == "gave_up"
    });
    // 待っても増えない
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(spawn_pids(&scratch).len(), 4, "初回 + 再起動 3 回");
    let status = server_status(&manager);
    assert_eq!(status["spawn_count"], json!(4));
    assert_eq!(status["crashes"], json!(4));
    assert_eq!(
        status["reason"],
        json!(text::fill(text::GAVE_UP_REASON, &[("count", "4")]))
    );
    assert_eq!(status["next_step"], json!(text::GAVE_UP_NEXT_STEP.text()));
    // 打鍵が続いても起こさない
    open(&manager, &mut link, &scratch.file(), "fn main() { }\n", 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(spawn_pids(&scratch).len(), 4);
    // 利用者の restart は数え直して起こす
    manager.restart(None);
    wait_until("起こし直し", Duration::from_secs(10), || {
        spawn_pids(&scratch).len() >= 5
    });
    manager.shutdown_all(Duration::from_secs(2));
}

/// エッジケース: initialize に応答しないサーバはタイムアウトで待ちを解く
#[test]
fn 応答しないサーバはタイムアウトで起こし直し上限で諦める() {
    let scratch = Scratch::new("slow");
    let mut config = config(&scratch, "slow");
    config.request_timeout = Duration::from_millis(500);
    let manager = LspManager::new(config);
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("諦めた", Duration::from_secs(30), || {
        state(&manager) == "gave_up"
    });
    let status = server_status(&manager);
    assert_eq!(spawn_pids(&scratch).len(), 4);
    assert!(
        status["last_exit"]
            .as_str()
            .is_some_and(|s| s.starts_with("initialize:")),
        "{status}"
    );
    // 待ちの表は空（タイムアウトで消えている）。プロセスはもう居ない
    assert!(status["pid"].is_null());
    for pid in spawn_pids(&scratch) {
        wait_until("偽サーバの終了", Duration::from_secs(5), || {
            !tako_core::platform::process::pid_alive(pid)
        });
    }
    // 送った initialize には取り消しが続く
    assert!(methods(&scratch).contains(&"$/cancelRequest".to_string()));
}

/// 受け入れ条件: サーバ未導入のとき status に理由 + 次の一手（導入コマンド）が入る
#[test]
fn 未導入なら理由と導入コマンドを返し探し直さない() {
    let calls = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&calls);
    let scratch = Scratch::new("missing");
    let mut config = config(&scratch, "normal");
    config.launcher = Arc::new(move |spec: &ServerSpec| {
        counter.fetch_add(1, Ordering::SeqCst);
        Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        }
    });
    let manager = LspManager::new(config);
    let mut link = DocLink::default();
    let path = scratch.file();
    open(&manager, &mut link, &path, "fn main() {}\n", 0);
    wait_until("未導入", Duration::from_secs(10), || {
        state(&manager) == "not_installed"
    });
    let spec = servers::resolve(&path).unwrap().spec;
    let status = server_status(&manager);
    assert_eq!(
        status["reason"],
        json!(text::fill(
            text::NOT_INSTALLED_REASON,
            &[("program", spec.program)]
        ))
    );
    assert_eq!(
        status["next_step"],
        json!(text::fill(
            text::NOT_INSTALLED_NEXT_STEP,
            &[("command", spec.install.command())]
        ))
    );
    assert_eq!(status["install_command"], json!(spec.install.command()));
    // 同じサーバの別のファイルは探し直さずに断る（打鍵のたびに PATH を探さない）
    let other = scratch.0.join("src").join("lib.rs");
    let mut other_link = DocLink::default();
    for version in 0..20 {
        open(&manager, &mut other_link, &other, "", version);
    }
    assert!(matches!(other_link, DocLink::Declined { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "解決は 1 回だけ");
    assert!(spawn_pids(&scratch).is_empty(), "何も起こしていない");
    // restart で記録を消して探し直す
    manager.restart(None);
    wait_until("探し直し", Duration::from_secs(10), || {
        calls.load(Ordering::SeqCst) == 2
    });
}

/// 受け入れ条件: アイドル中に偽サーバが受信した行数が 0（定期送信を作らない = #772）
#[test]
fn アイドル中は1行も送らない() {
    let idle = std::env::var("TAKO_LSP_IDLE_TEST_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let scratch = Scratch::new("idle");
    let manager = LspManager::new(config(&scratch, "normal"));
    let mut link = DocLink::default();
    let path = scratch.file();
    open(&manager, &mut link, &path, "fn main() {}\n", 0);
    wait_until("didOpen", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didOpen".to_string())
    });
    let before = received(&scratch).len();
    let ours_before = server_status(&manager)["received_messages"].clone();
    // 編集はしていないが、再描画のたびに同期は呼ばれる（同じ版なら何も送らない）
    let deadline = Instant::now() + Duration::from_secs(idle);
    while Instant::now() < deadline {
        open(&manager, &mut link, &path, "fn main() {}\n", 0);
        std::thread::sleep(Duration::from_millis(100));
    }
    let after = received(&scratch).len();
    println!("アイドル {idle} 秒: 偽サーバの受信 {before} → {after} 行");
    assert_eq!(after - before, 0, "アイドル中に送った");
    assert_eq!(server_status(&manager)["received_messages"], ours_before);
    manager.shutdown_all(Duration::from_secs(2));
}

/// サーバからの要求には既定の応答を返す（無視して黙らない）
#[test]
fn サーバからの問い合わせには既定の応答を返す() {
    let scratch = Scratch::new("ask");
    let manager = LspManager::new(config(&scratch, "ask"));
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("応答", Duration::from_secs(10), || {
        received(&scratch)
            .iter()
            .any(|m| m.get("id") == Some(&json!("cfg-1")))
    });
    let reply = received(&scratch)
        .into_iter()
        .find(|m| m.get("id") == Some(&json!("cfg-1")))
        .unwrap();
    assert_eq!(reply["result"], json!([null, null]));
    manager.shutdown_all(Duration::from_secs(2));
}

#[test]
fn 通知を大量に受けても止まらない() {
    let scratch = Scratch::new("chatty");
    let manager = LspManager::new(config(&scratch, "chatty"));
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("5000 件の受信", Duration::from_secs(20), || {
        server_status(&manager)["received_messages"]
            .as_u64()
            .is_some_and(|n| n >= 5001)
    });
    assert_eq!(state(&manager), "running");
    manager.shutdown_all(Duration::from_secs(2));
}

/// 閉じたら didClose を送って診断を捨て、猶予が過ぎたら止める
#[test]
fn 閉じたら診断を捨て猶予の後に止める() {
    let scratch = Scratch::new("close");
    let mut config = config(&scratch, "normal");
    config.idle_grace = Duration::from_millis(300);
    let manager = LspManager::new(config);
    let mut link = DocLink::default();
    let path = scratch.file();
    open(&manager, &mut link, &path, "fn main() {}\n", 0);
    wait_until("診断の受信", Duration::from_secs(10), || {
        manager.diagnostics_count(&path) == 1
    });
    // 編集モードを抜けた
    manager.sync(&mut link, false, &path, "fn main() {}\n", 0);
    assert!(matches!(link, DocLink::Unlinked));
    assert_eq!(manager.diagnostics_count(&path), 0, "閉じたら 0");
    wait_until("猶予の後の停止", Duration::from_secs(10), || {
        state(&manager) == "not_started"
    });
    let methods = methods(&scratch);
    let tail: Vec<&str> = methods
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(String::as_str)
        .collect();
    assert_eq!(tail, vec!["textDocument/didClose", "shutdown", "exit"]);
}

/// restart で世代が進み、今の本文で開き直す
#[test]
fn restart_は今の本文で開き直す() {
    let scratch = Scratch::new("restart");
    let manager = LspManager::new(config(&scratch, "normal"));
    let mut link = DocLink::default();
    let path = scratch.file();
    open(&manager, &mut link, &path, "fn main() {}\n", 0);
    wait_until("稼働", Duration::from_secs(10), || {
        state(&manager) == "running"
    });
    open(&manager, &mut link, &path, "fn main() { 1 }\n", 3);
    let before = server_status(&manager)["generation"].as_u64().unwrap();
    manager.restart(None);
    wait_until("2 回目の didOpen", Duration::from_secs(10), || {
        methods(&scratch)
            .iter()
            .filter(|m| *m == "textDocument/didOpen")
            .count()
            == 2
    });
    let status = server_status(&manager);
    assert!(status["generation"].as_u64().unwrap() > before);
    let reopened = received(&scratch)
        .into_iter()
        .filter(|m| m["method"] == json!("textDocument/didOpen"))
        .last()
        .unwrap();
    assert_eq!(
        reopened["params"]["textDocument"]["text"],
        json!("fn main() { 1 }\n")
    );
    assert_eq!(reopened["params"]["textDocument"]["version"], json!(3));
    assert_eq!(spawn_pids(&scratch).len(), 2);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 同じファイルを 2 つ目のペインで開いても LSP へは 1 回しか開かない
#[test]
fn 同じファイルの2つ目は断り1つ目を閉じたら開ける() {
    let scratch = Scratch::new("dup");
    let manager = LspManager::new(config(&scratch, "normal"));
    let path = scratch.file();
    let (mut first, mut second) = (DocLink::default(), DocLink::default());
    open(&manager, &mut first, &path, "a\n", 0);
    open(&manager, &mut second, &path, "a\n", 0);
    assert!(matches!(second, DocLink::Declined { .. }));
    drop(first);
    open(&manager, &mut second, &path, "a\n", 0);
    assert!(matches!(second, DocLink::Open(_)));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 対象外の拡張子は断る（サーバを起こさない）
#[test]
fn 対象外の拡張子は何も起こさない() {
    let scratch = Scratch::new("plain");
    let manager = LspManager::new(config(&scratch, "normal"));
    let mut link = DocLink::default();
    open(
        &manager,
        &mut link,
        &scratch.0.join("README.md"),
        "# x\n",
        0,
    );
    assert!(matches!(link, DocLink::Declined { .. }));
    assert_eq!(manager.status(None)["servers"], json!([]));
    assert!(manager.status(None)["note"].is_string());
    assert!(spawn_pids(&scratch).is_empty());
}

// --- 孤児を残さない ---------------------------------------------------------

const HOST_ENV: &str = "TAKO_1678_HOST_DIR";

/// 子プロセス側（`HOST_ENV` があるときだけ動く）: manager を立てて偽サーバを起こし、
/// その pid を書いてから眠る。親がこのプロセスを kill する
#[test]
fn 子プロセス_偽サーバを起こして眠る() {
    let Ok(dir) = std::env::var(HOST_ENV) else {
        return;
    };
    let scratch = Scratch(PathBuf::from(dir));
    let manager = LspManager::new(config(&scratch, "normal"));
    let mut link = DocLink::default();
    open(&manager, &mut link, &scratch.file(), "fn main() {}\n", 0);
    wait_until("稼働", Duration::from_secs(10), || {
        state(&manager) == "running"
    });
    let pid = manager.server_pids()[0];
    std::fs::write(scratch.0.join("server.pid"), pid.to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(60));
    std::mem::forget(scratch);
}

/// 受け入れ条件: 偽サーバを起こしたまま tako 側のプロセスを落としても、偽サーバの pid が
/// 生存しない（`pid_alive` 境界で判定）。kill -9 相当なので Drop も終了処理も走らない
#[test]
fn 親が落ちたら偽サーバも終わる() {
    if std::env::var(HOST_ENV).is_ok() {
        return;
    }
    let scratch = Scratch::new("orphan");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "子プロセス_偽サーバを起こして眠る",
            "--nocapture",
        ])
        .env(HOST_ENV, &scratch.0)
        .env_remove("TAKO_1007_LEGACY")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = scratch.0.join("server.pid");
    wait_until("偽サーバの pid", Duration::from_secs(20), || {
        pid_file.exists()
    });
    let server_pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        tako_core::platform::process::pid_alive(server_pid),
        "落とす前は生きている"
    );
    child.kill().unwrap();
    let _ = child.wait();
    wait_until("偽サーバの終了", Duration::from_secs(10), || {
        !tako_core::platform::process::pid_alive(server_pid)
    });
}
