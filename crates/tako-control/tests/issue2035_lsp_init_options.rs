//! #2035 の e2e: 言語サーバへ初期化オプション（`initializationOptions`）を送る。
//!
//! 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、**サーバが受け取った initialize の
//! params**（偽サーバの受信ログ）で「何を送ったか」を測る。rust-analyzer の行は既定で
//! `cargo.targetDir = true`（初回の build script と `cargo check` を `target/rust-analyzer` で走らせ、
//! 利用者の `cargo build` と target のロックを取り合わない）、既定を持たない行は何も送らない。
//! 上書きは `options`（CLI `tako lsp options` / MCP `tako_lsp_server` の action=options と同じ
//! `dispatch::lsp_server_action`）で書き、次に起こしたときから効く。
//!
//! 本番の manager（`LspManager::from_env`）が settings.json を読み書きすることと A/B
//! （`TAKO_2035_LEGACY=1` = 何も送らない）は、同じテストバイナリを子プロセスで起こして測る
//! （テストの env を書き換えない = 並列のテストと打ち消し合わない）。子は rc を読まない `/bin/sh` と
//! 一時の HOME で起こす（本番の解決はログインシェル越しに偽サーバを起こすので、開発者の rc を
//! 経由させない = #2551。番犬 `issue2551_lsp_child_shell_watchdog`）。

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tako_control::dispatch::{lsp_server_action, DispatchError};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）。
/// 置き場は #1312 の `ScratchDir`（`tako-test-scratch-<pid>/` の下）: 消し損ねても終了時の後始末と
/// `tako test-residue` が拾う（#2555 の前は TMPDIR 直下の `tako-2035-*` で、どちらにも載らなかった）。
///
/// 偽サーバを起こしたテストは**`shutdown_all` で止めてから**置き場を手放す。`stop` は止め終わるのを
/// 待たずに返る（shutdown → exit は別スレッド）ので、その直後に置き場を消すと、偽サーバが `exit` を
/// 受けて `received.jsonl` を作り直し、置き場が消え残る（#2555。番犬 `issue2555_lsp_stop_then_scope_end_watchdog`）
struct Scratch(ScratchDir);

impl Scratch {
    fn new(label: &str) -> Self {
        let dir = ScratchDir::new(&format!("2035-{label}"));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn rust_file(&self) -> PathBuf {
        self.0.join("src").join("main.rs")
    }

    fn python_file(&self) -> PathBuf {
        self.0.join("tool.py")
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }

    fn data_dir(&self) -> PathBuf {
        self.0.join("data")
    }
}

fn config(scratch: &Scratch) -> LspConfig {
    let args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
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

/// サーバが受け取った initialize の params を、受け取った順に
fn initializes(log: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m.get("method").and_then(Value::as_str) == Some("initialize"))
        .map(|m| m["params"].clone())
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

/// `n` 回目の initialize を待って、その `initializationOptions`（無ければ null）
fn nth_init_options(log: &std::path::Path, n: usize) -> Value {
    wait_until(
        &format!("{n} 回目の initialize"),
        Duration::from_secs(15),
        || initializes(log).len() >= n,
    );
    initializes(log)[n - 1]
        .get("initializationOptions")
        .cloned()
        .unwrap_or(Value::Null)
}

fn set(value: Value) -> Map<String, Value> {
    serde_json::from_value(value).unwrap()
}

fn open(manager: &LspManager, path: &std::path::Path) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, "fn main() {}\n", 0);
    link
}

/// 受け入れ条件 2 の前提: rust-analyzer の行へは既定で `cargo.targetDir = true` を送り、
/// 送った値は `tako lsp status` の各サーバの `init_options` と `options` の `running[].sent` で読める
#[test]
fn rust_analyzer_へは既定で_target_dir_を分ける指定を送る() {
    let scratch = Scratch::new("default");
    let manager = LspManager::new(config(&scratch));
    let _link = open(&manager, &scratch.rust_file());
    let sent = nth_init_options(&scratch.log(), 1);
    assert_eq!(sent, json!({"cargo": {"targetDir": true}}));
    wait_until("稼働", Duration::from_secs(10), || {
        manager.status(Some("rust-analyzer"))["servers"][0]["state"] == json!("running")
    });
    let status = manager.status(Some("rust-analyzer"));
    assert_eq!(status["servers"][0]["init_options"], sent);

    let report = lsp_server_action(Some(&manager), "options", None, None).unwrap();
    let servers = report["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1, "既定か上書きを持つ行だけ: {report}");
    let row = &servers[0];
    assert_eq!(row["id"], json!("rust-analyzer"));
    assert_eq!(
        row["options"],
        json!([{"key": "cargo.targetDir", "value": true, "source": "default", "default": true}])
    );
    assert_eq!(row["effective"], sent);
    assert_eq!(row["running"][0]["sent"], sent);
    assert_eq!(row["running"][0]["current"], json!(true));
    assert_eq!(report["legacy"], json!(false));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 既定を持たない行（pyright）へは何も送らない（変更前と同じ initialize）
#[test]
fn 既定を持たないサーバへは送らない() {
    let scratch = Scratch::new("none");
    let manager = LspManager::new(config(&scratch));
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &scratch.python_file(), "x = 1\n", 0);
    assert_eq!(nth_init_options(&scratch.log(), 1), Value::Null);
    let report = lsp_server_action(Some(&manager), "options", Some("pyright"), None).unwrap();
    assert_eq!(report["servers"][0]["options"], json!([]));
    assert_eq!(report["servers"][0]["effective"], Value::Null);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件 3: 上書きは `options` の `set` で書き、動いているサーバには次の起動から効く
/// （それまでは `running[].current` が偽）。null で既定へ戻す。表に無いキーも足せる
#[test]
fn 上書きは次の起動から効き_null_で既定へ戻る() {
    let scratch = Scratch::new("override");
    let manager = LspManager::new(config(&scratch));
    let _link = open(&manager, &scratch.rust_file());
    assert_eq!(
        nth_init_options(&scratch.log(), 1),
        json!({"cargo": {"targetDir": true}})
    );
    wait_until("稼働", Duration::from_secs(10), || {
        manager.status(None)["servers"][0]["state"] == json!("running")
    });

    let report = lsp_server_action(
        Some(&manager),
        "options",
        Some("rust-analyzer"),
        Some(&set(
            json!({"cargo.targetDir": false, "check.command": "clippy"}),
        )),
    )
    .unwrap();
    assert_eq!(report["changed"], json!(true));
    let row = &report["servers"][0];
    assert_eq!(
        row["options"],
        json!([
            {"key": "cargo.targetDir", "value": false, "source": "user", "default": true},
            {"key": "check.command", "value": "clippy", "source": "user", "default": null},
        ])
    );
    let wanted = json!({"cargo": {"targetDir": false}, "check": {"command": "clippy"}});
    assert_eq!(row["effective"], wanted);
    assert_eq!(
        row["running"][0]["current"],
        json!(false),
        "動いているサーバは古い値のまま（次の起動から効く）"
    );
    assert_eq!(
        initializes(&scratch.log()).len(),
        1,
        "書いただけでは起こし直さない"
    );

    manager.restart(Some("rust-analyzer"));
    assert_eq!(nth_init_options(&scratch.log(), 2), wanted);
    wait_until("稼働", Duration::from_secs(10), || {
        manager.status(None)["servers"][0]["state"] == json!("running")
    });
    let report = lsp_server_action(Some(&manager), "options", Some("rust-analyzer"), None).unwrap();
    assert_eq!(report["servers"][0]["running"][0]["current"], json!(true));

    // null で既定へ戻す（表に無いキーは消える）
    lsp_server_action(
        Some(&manager),
        "options",
        Some("rust-analyzer"),
        Some(&set(
            json!({"cargo.targetDir": null, "check.command": null}),
        )),
    )
    .unwrap();
    manager.restart(Some("rust-analyzer"));
    assert_eq!(
        nth_init_options(&scratch.log(), 3),
        json!({"cargo": {"targetDir": true}})
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 上書きの入口の検査（CLI / MCP / dispatch で同じ 1 実装）: サーバの ID が要る・知らない ID・
/// キーの形・options 以外の action に set は断り、断ったときは何も変えない
#[test]
fn 上書きの入口は不正を断り何も変えない() {
    let scratch = Scratch::new("reject");
    let manager = LspManager::new(config(&scratch));
    let invalid = |result: Result<Value, DispatchError>| match result {
        Err(DispatchError::InvalidParams(message)) => message,
        other => panic!("InvalidParams にならない: {other:?}"),
    };
    let message = invalid(lsp_server_action(
        Some(&manager),
        "options",
        None,
        Some(&set(json!({"cargo.targetDir": false}))),
    ));
    assert!(message.contains("--name"), "{message}");
    invalid(lsp_server_action(
        Some(&manager),
        "options",
        Some("no-such-server"),
        Some(&set(json!({"cargo.targetDir": false}))),
    ));
    invalid(lsp_server_action(
        Some(&manager),
        "options",
        Some("rust-analyzer"),
        Some(&set(json!({"cargo.targetDir": false, "bad key": 1}))),
    ));
    invalid(lsp_server_action(
        Some(&manager),
        "status",
        Some("rust-analyzer"),
        Some(&set(json!({"cargo.targetDir": false}))),
    ));
    let report = lsp_server_action(Some(&manager), "options", Some("rust-analyzer"), None).unwrap();
    assert_eq!(
        report["servers"][0]["effective"],
        json!({"cargo": {"targetDir": true}}),
        "断った上書きは当たっていない"
    );
}

// --- 本番の manager（settings.json）と A/B（子プロセスで測る）-------------------------

const PROBE_ENV: &str = "TAKO_2035_PROBE";

/// 子プロセスの中身: 本番の口（`LspManager::from_env`）で .rs を開き、送った初期化オプションを
/// 1 行で出す。`TAKO_2035_PROBE=set:<JSON>` なら先にその上書きを options で書く
#[test]
fn 本番の口で送った値を出す子プロセス() {
    let Some(mode) = std::env::var_os(PROBE_ENV) else {
        return;
    };
    let mode = mode.to_string_lossy().into_owned();
    let file = PathBuf::from(std::env::var("TAKO_2035_PROBE_FILE").unwrap());
    let log = PathBuf::from(std::env::var("TAKO_LSP_FAKE_LOG").unwrap());
    let manager = LspManager::from_env();
    if let Some(json) = mode.strip_prefix("set:") {
        let set: Map<String, Value> = serde_json::from_str(json).unwrap();
        lsp_server_action(Some(&manager), "options", Some("rust-analyzer"), Some(&set)).unwrap();
    }
    let _link = open(&manager, &file);
    let sent = nth_init_options(&log, 1);
    println!("TAKO_2035_PROBE sent={sent}");
    manager.shutdown_all(Duration::from_secs(2));
}

/// `vars` だけを立てた子プロセスで、本番の口が送った初期化オプションを測る
fn probe(scratch: &Scratch, mode: &str, vars: &[(&str, &str)]) -> Value {
    let _ = std::fs::remove_file(scratch.log());
    let home = scratch.0.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    // `--test-threads=1`: 子の出力の形を親の env（`RUST_TEST_THREADS`）に依らず 1 つに決める。
    // 直列の libtest は `test <名前> ... ` を改行せずに出すので、子の 1 行目はその後ろへつながる（#2248）
    command.args([
        "--exact",
        "本番の口で送った値を出す子プロセス",
        "--test-threads=1",
        "--nocapture",
    ]);
    for key in [
        "TAKO_ISOLATED",
        "TAKO_SELF_TEST",
        "TAKO_VISUAL_TEST",
        "TAKO_2035_LEGACY",
        "TAKO_1007_LEGACY",
        "TAKO_LSP_DIAG",
    ] {
        command.env_remove(key);
    }
    command
        .env(PROBE_ENV, mode)
        .env("TAKO_2035_PROBE_FILE", scratch.rust_file())
        .env("TAKO_DATA_DIR", scratch.data_dir())
        .env("TAKO_LSP_BIN_RUST_ANALYZER", FAKE)
        .env("TAKO_LSP_FAKE_LOG", scratch.log())
        .env("TAKO_LSP_AUTOSTART", "1")
        .env("TAKO_LSP_AUTO_FETCH", "0")
        .env("HOME", &home)
        .envs(vars.iter().copied());
    // 本番の解決は unix で `$SHELL -l -c 'exec …'` を起こす（差し替えの env の偽サーバも同じ）。
    // 開発者の rc（`pyenv rehash` は残ったロックを最大 60 秒待つ）を偽サーバの前に走らせない
    // 素のシェルにする（#2551: 全体実行でだけ 1 回目の initialize の待ちを使い切った）
    if cfg!(unix) {
        command.env("SHELL", "/bin/sh");
    }
    let output = command.output().expect("子プロセスを起こせる");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // 目印は行頭とは限らない（上の `--test-threads=1` で `test <名前> ... ` の後ろへ続く）
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("TAKO_2035_PROBE sent=").map(|(_, rest)| rest))
        .unwrap_or_else(|| {
            panic!(
                "子プロセスが値を出さない: {stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
    serde_json::from_str(line).unwrap()
}

/// 受け入れ条件 2 / 3（本番の口）: settings.json が無ければ既定、`lsp_init_options` があればそれを重ね、
/// options の set は settings.json の他のキーを残したまま書く（次の起動 = 子プロセスの起動から効く）。
/// A/B: `TAKO_2035_LEGACY=1` は何も送らない（変更前）
#[test]
fn 本番の口は_settings_json_を読み書きし_旧は送らない() {
    let scratch = Scratch::new("probe");
    let settings = scratch.data_dir().join("settings.json");
    assert_eq!(
        probe(&scratch, "read", &[]),
        json!({"cargo": {"targetDir": true}}),
        "settings.json が無ければ表の既定"
    );
    assert_eq!(
        probe(&scratch, "read", &[("TAKO_2035_LEGACY", "1")]),
        Value::Null,
        "A/B: 旧は initializationOptions を送らない"
    );

    std::fs::write(
        &settings,
        r#"{"theme": "light", "lsp_init_options": {"rust-analyzer": {"cargo.targetDir": "target/ra"}}}"#,
    )
    .unwrap();
    assert_eq!(
        probe(&scratch, "read", &[]),
        json!({"cargo": {"targetDir": "target/ra"}}),
        "手で書いた settings.json も次の起動から効く"
    );

    assert_eq!(
        probe(&scratch, r#"set:{"cargo.targetDir": false}"#, &[]),
        json!({"cargo": {"targetDir": false}})
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        written["lsp_init_options"],
        json!({"rust-analyzer": {"cargo.targetDir": false}})
    );
    assert_eq!(written["theme"], json!("light"), "他の設定は残す");

    assert_eq!(
        probe(&scratch, r#"set:{"cargo.targetDir": null}"#, &[]),
        json!({"cargo": {"targetDir": true}}),
        "null で表の既定へ戻る"
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert!(
        written.get("lsp_init_options").is_none(),
        "上書きが空になったらキーごと書かない: {written}"
    );
}
