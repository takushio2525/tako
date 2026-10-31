//! #2048 の e2e: 隔離起動（`TAKO_ISOLATED`）の manager は、文書を開いても言語サーバを起こさない。
//!
//! 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、**spawn した pid の記録**で
//! 「起きていない / 起きた」を測る。起こさなかった器は「止めた」（理由は隔離）に置かれ、
//! 明示の `restart` だけが起こす。開いた時点で起こす manager（本番 = 隔離でない起動）は従来どおり。
//!
//! env の読み（`TAKO_ISOLATED` / `TAKO_LSP_AUTOSTART` / `TAKO_LSP_AUTO_FETCH` / `TAKO_2048_LEGACY`）は
//! 同じテストバイナリを子プロセスで起こして測る（テストの env を書き換えない = 並列のテストと
//! 打ち消し合わない・作業シェルが隔離の tako の中でも結果が変わらない）。
//!
//! A/B: `TAKO_2048_LEGACY=1` は env の読みを #2048 の前へ戻す（隔離でも起こし・取る）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::{autostart_from, DEFAULT_IDLE_GRACE};
use tako_control::lsp::{fetch, text, DocLink, GotoRequest, Launch, LspConfig, LspManager};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）。
/// 置き場は #1312 の `ScratchDir`（`tako-test-scratch-<pid>/` の下）で、偽サーバを起こしたテストは
/// **`shutdown_all` で止めてから**手放す（`stop` は止め終わるのを待たない = 直後に消すと偽サーバの
/// `exit` の書き込みで置き場が消え残る。#2555）
struct Scratch(ScratchDir);

impl Scratch {
    fn new(label: &str) -> Self {
        let dir = ScratchDir::new(&format!("2048-{label}"));
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

fn config(scratch: &Scratch) -> LspConfig {
    let args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
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

fn methods(scratch: &Scratch) -> Vec<String> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn spawn_count(scratch: &Scratch) -> usize {
    std::fs::read_to_string(scratch.spawns())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
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

/// 起こさないことは「待っても spawn が 0 のまま」でしか測れない。起こす manager が握手まで
/// 終える時間（この機で 0.1 秒前後）より十分長く待つ
const QUIET: Duration = Duration::from_millis(1500);

/// 受け入れ条件 1 / 2: 隔離では開いて打っても起こさず（spawn 0・状態は止めた・理由は隔離）、
/// 明示の restart で起きて文書が届く
#[test]
fn 隔離では開いても起こさず_restart_で起きる() {
    let scratch = Scratch::new("held");
    let manager = LspManager::with_autostart(config(&scratch), None, false);
    let path = scratch.file();
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &path, "fn main() {}\n", 0);
    assert!(
        matches!(link, DocLink::Open(_)),
        "文書は登録する（restart で起きたら didOpen する）: {link:?}"
    );
    // 打鍵が続いても起こさない
    manager.sync(&mut link, true, &path, "fn main() { }\n", 1);
    std::thread::sleep(QUIET);
    assert_eq!(spawn_count(&scratch), 0, "隔離では言語サーバを起こさない");
    assert!(methods(&scratch).is_empty());

    let status = manager.status(None);
    assert_eq!(status["autostart"], json!(false));
    let server = server_status(&manager);
    assert_eq!(server["state"], json!("stopped"));
    assert_eq!(server["reason"], json!(text::HELD_REASON.text()));
    assert_eq!(server["next_step"], json!(text::HELD_NEXT_STEP.text()));
    let shown = manager
        .document_server(&path)
        .expect("受け持つサーバはある");
    assert!(shown.held, "画面は隔離の 1 行を出す");

    // 明示の問い合わせ（定義ジャンプ）でも起こさず、隔離の理由と restart を返す
    let error = manager
        .goto(&GotoRequest {
            kind: GotoKind::Definition,
            path: path.clone(),
            line: 0,
            column: 3,
            timeout: Duration::from_secs(2),
            document: None,
        })
        .expect_err("起きていないので答えられない");
    let answer = error.to_json(GotoKind::Definition);
    assert_eq!(answer["status"], json!("unavailable"));
    assert_eq!(answer["reason"], json!(text::HELD_REASON.text()));
    assert_eq!(spawn_count(&scratch), 0, "問い合わせでも起こさない");

    // 明示の restart だけが起こす（開いていた文書は最新の本文で届く）
    let restarted = manager.restart(None);
    assert_eq!(restarted["restarted"].as_array().map(Vec::len), Some(1));
    wait_until("didOpen", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didOpen".to_string())
    });
    assert_eq!(spawn_count(&scratch), 1);
    wait_until("稼働", Duration::from_secs(10), || {
        server_status(&manager)["state"] == json!("running")
    });
    assert!(server_status(&manager)["reason"].is_null());
    assert!(!manager.document_server(&path).expect("ある").held);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 隔離で起こさなかった器を明示の stop で止めたら、理由は「stop で止めた」へ替わる
#[test]
fn 隔離で起こさなかった器を_stop_したら_stop_の理由() {
    let scratch = Scratch::new("held-stop");
    let manager = LspManager::with_autostart(config(&scratch), None, false);
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &scratch.file(), "fn main() {}\n", 0);
    assert_eq!(
        server_status(&manager)["reason"],
        json!(text::HELD_REASON.text())
    );
    manager.stop(None);
    let server = server_status(&manager);
    assert_eq!(server["state"], json!("stopped"));
    assert_eq!(server["reason"], json!(text::STOPPED_REASON.text()));
    std::thread::sleep(QUIET);
    assert_eq!(spawn_count(&scratch), 0);
}

/// 受け入れ条件 4: 開いた時点で起こす manager（本番 = 隔離でない起動）は従来どおり起こす
#[test]
fn 隔離でなければ開いた時点で起こす() {
    for manager_of in [
        (|c| LspManager::with_autostart(c, None, true)) as fn(LspConfig) -> LspManager,
        LspManager::new,
    ] {
        let scratch = Scratch::new("auto");
        let manager = manager_of(config(&scratch));
        let mut link = DocLink::default();
        manager.sync(&mut link, true, &scratch.file(), "fn main() {}\n", 0);
        wait_until("didOpen", Duration::from_secs(10), || {
            methods(&scratch).contains(&"textDocument/didOpen".to_string())
        });
        assert_eq!(spawn_count(&scratch), 1);
        assert_eq!(manager.status(None)["autostart"], json!(true));
        manager.shutdown_all(Duration::from_secs(2));
    }
}

#[test]
fn 起こすかの判定は隔離と明示の_env_で決まる() {
    assert!(autostart_from(None, false), "本番の既定は起こす");
    assert!(!autostart_from(None, true), "隔離の既定は起こさない");
    assert!(autostart_from(Some("1"), true), "明示で起こす");
    assert!(autostart_from(Some(" on "), true));
    assert!(!autostart_from(Some("0"), false), "明示で止める");
    assert!(
        autostart_from(Some("garbage"), false),
        "解釈できない値は既定へ"
    );
    assert!(!autostart_from(Some(""), true));
}

// --- env の読み（子プロセスで測る） ------------------------------------------

const PROBE_ENV: &str = "TAKO_2048_PROBE";

/// 子プロセスの中身: env の読みを 1 行で出す（親から起こされたときだけ動く）
#[test]
fn env_の読みを出す子プロセス() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }
    println!(
        "TAKO_2048_PROBE autostart={} fetch={}",
        tako_control::lsp::manager::autostart_from_env(),
        fetch::auto_from_env()
    );
}

/// 関係する env を全部外し、`vars` だけを立てた子プロセスで読みを測る
fn probe(vars: &[(&str, &str)]) -> (bool, bool) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    // `--test-threads=1`: 子の出力の形を親の env（`RUST_TEST_THREADS`）に依らず 1 つに決める。
    // 直列の libtest は `test <名前> ... ` を改行せずに出すので、子の 1 行目はその後ろへつながる（#2248）
    command.args([
        "--exact",
        "env_の読みを出す子プロセス",
        "--test-threads=1",
        "--nocapture",
    ]);
    for key in [
        "TAKO_ISOLATED",
        "TAKO_SELF_TEST",
        "TAKO_VISUAL_TEST",
        "TAKO_LSP_AUTOSTART",
        "TAKO_LSP_AUTO_FETCH",
        "TAKO_2048_LEGACY",
    ] {
        command.env_remove(key);
    }
    command.env(PROBE_ENV, "1").envs(vars.iter().copied());
    let output = command.output().expect("子プロセスを起こせる");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // 目印は行頭とは限らない（上の `--test-threads=1` で `test <名前> ... ` の後ろへ続く）
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("TAKO_2048_PROBE ").map(|(_, rest)| rest))
        .unwrap_or_else(|| panic!("子プロセスが読みを出さない: {stdout}"));
    let flag = |name: &str| line.contains(&format!("{name}=true"));
    (flag("autostart"), flag("fetch"))
}

/// 受け入れ条件 1 / 2 / 4（env の読み）: 本番は起こして取る、隔離は起こさず取らない、
/// 明示の env で隔離でも起こす・取る。A/B の `TAKO_2048_LEGACY=1` は隔離でも起こして取る
#[test]
fn env_の読みは本番では変わらず隔離だけ止まる() {
    assert_eq!(probe(&[]), (true, true), "本番（隔離でない）は起こして取る");
    assert_eq!(
        probe(&[("TAKO_ISOLATED", "1")]),
        (false, false),
        "隔離は起こさず取らない"
    );
    assert_eq!(
        probe(&[("TAKO_ISOLATED", "0")]),
        (true, true),
        "偽の値は隔離と読まない"
    );
    assert_eq!(
        probe(&[("TAKO_ISOLATED", "1"), ("TAKO_LSP_AUTOSTART", "1")]),
        (true, false),
        "明示で起こす（取るのは別の明示）"
    );
    assert_eq!(
        probe(&[
            ("TAKO_ISOLATED", "1"),
            ("TAKO_LSP_AUTOSTART", "1"),
            ("TAKO_LSP_AUTO_FETCH", "1"),
        ]),
        (true, true)
    );
    assert_eq!(
        probe(&[("TAKO_SELF_TEST", "1")]),
        (true, false),
        "セルフテストだけ（隔離でない）は従来どおり起こして取らない"
    );
    assert_eq!(
        probe(&[("TAKO_ISOLATED", "1"), ("TAKO_2048_LEGACY", "1")]),
        (true, true),
        "A/B: 旧は隔離でも起こして取る"
    );
}

/// 確かめるのは manager の外の話だが、本番の GUI が env を読む口を通って作られていること
/// （`LspManager::from_env`）を番犬にする: GUI が `new` / `with_fetch` で作ると隔離が効かない
#[test]
fn gui_は_env_を読む口で_manager_を作る() {
    let main = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tako-app/src/main.rs");
    let source = std::fs::read_to_string(&main).expect("tako-app の main.rs を読める");
    assert!(
        source.contains("lsp: tako_control::lsp::LspManager::from_env(),"),
        "TakoApp の言語サーバは LspManager::from_env() で作る（#2048）"
    );
}
