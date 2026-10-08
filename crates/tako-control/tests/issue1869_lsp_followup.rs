//! #1869 の e2e: 言語サーバの**読み込み中**に補完を頼んだときの振る舞いを、偽の言語サーバ
//! （`tako-lsp-fake` の `loading` シナリオ）を実プロセスで起こして測る。
//!
//! ## 真因（実測。rust-analyzer 1.95.0・std だけの新しいプロジェクト）
//!
//! 起動直後に 250ms おきに `textDocument/completion` を投げると、rust-analyzer は 2 段で答える:
//!
//! ```text
//! [  330ms] serverStatus quiescent=false
//! [  337ms] id=11 → null          ← 前半: 即座に null（読み込みの前）
//! [  566ms] id=12 → null
//! [  826ms] id=13 → null
//! [ 2257ms] serverStatus quiescent=true
//! [ 2520ms] id=14..19 → items=126  ← 後半: 答えずに待たせ、済んだ直後にまとめて答える
//! ```
//!
//! tako は前半の `null` を「0 件」として受け、**打鍵の要求（`superseding`）は読み込みを待たずに
//! そのまま 0 件で返していた**（CLI / MCP の要求だけが #1680 と同じく待って問い直していた）。
//! GUI は 0 件なので一覧を出さず、読み込みが済んでも問い直さない = サーバが空を返したのを
//! **tako 側が捨てていた**。`didChange` を挟んでも -32801（ContentModified）は 1 件も出なかった。
//!
//! ここでは偽サーバに同じ 2 段を演じさせ（前半 = 既定、後半 = `hold_while_loading`）、
//! 打鍵の要求も済むのを待って問い直すこと・待つあいだに次の打鍵 / 閉じる / 文書を閉じるで
//! 抜けること・上限に当たったら `loading`（読み込み中）と分かる答えになることを固定する。
//! 状態の待ちは `wait_until`（状態で待つ。実時間の比較を持ち込まない = `.agent/conventions.md`）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    CompletionError, CompletionRequest, DocLink, Launch, LspConfig, LspManager,
};
use tako_core::lsp::completion::Trigger;
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
            "tako-1869-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }

    fn rules(&self) -> PathBuf {
        self.0.join("completion.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `loading` シナリオの偽サーバ（読み込みは `loading_ms`）
fn config(scratch: &Scratch, loading_ms: u64, rules: &Value) -> LspConfig {
    std::fs::write(scratch.rules(), rules.to_string()).unwrap();
    let args = vec![
        "--scenario".to_string(),
        "loading".to_string(),
        "--loading-ms".to_string(),
        loading_ms.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--completion".to_string(),
        scratch.rules().display().to_string(),
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

fn of_method(scratch: &Scratch, method: &str) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m.get("method").and_then(Value::as_str) == Some(method))
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

fn request(path: &Path, superseding: bool, timeout: Duration) -> CompletionRequest {
    CompletionRequest {
        path: path.to_path_buf(),
        line: 0,
        column: 14,
        timeout,
        document: None,
        trigger: Trigger::Word,
        superseding,
        resolve_top: 0,
    }
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path, text: &str) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, text, 1);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

const TEXT: &str = "fn main() { ab }\n";

fn rules() -> Value {
    json!({ "items": [{ "label": "abc" }, { "label": "abd" }] })
}

/// 受け入れ条件 2 の本体: 読み込みの前半（サーバが即座に `null` で答える）に出した**打鍵の要求**も、
/// 読み込みが済むのを待って問い直し、一覧（2 件）を返す。修正前は 0 件のまま即座に返っていた
#[test]
fn 読み込みの前半に出した打鍵の要求も済むのを待って一覧を返す() {
    let scratch = Scratch::new("early");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, 1500, &rules()));
    let _link = open_editing(&manager, &main, TEXT);
    let answer = manager
        .completion(&request(&main, true, Duration::from_secs(10)))
        .expect("答えが来る");
    let asked = of_method(&scratch, "textDocument/completion").len();
    assert_eq!(
        answer.items.len(),
        2,
        "読み込みが済んでから問い直した一覧（問い合わせ {asked} 回）"
    );
    assert!(
        asked >= 2,
        "読み込み中の null の後に問い直している（問い合わせ {asked} 回）"
    );
    assert!(
        answer.waited_for_loading.is_some(),
        "読み込みを待ったことが答えに載る"
    );
    assert!(
        !manager.server_loading(&main),
        "済んだ後は読み込み中ではない"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 読み込みの後半（サーバが答えずに待たせ、済んだ直後に答える = rust-analyzer の実測）でも、
/// 打鍵の要求は済んだ後の答えを受けて一覧を返す。送ったときにサーバ自身が読み込み中と
/// 知らせていたので、待ったことが答えに載る
#[test]
fn 読み込みの後半に待たされた要求は済んだ後の答えを返す() {
    let scratch = Scratch::new("hold");
    let main = scratch.write("src/main.rs", TEXT);
    let mut rules = rules();
    rules["hold_while_loading"] = json!(true);
    let manager = LspManager::new(config(&scratch, 1500, &rules));
    let _link = open_editing(&manager, &main, TEXT);
    let answer = manager
        .completion(&request(&main, true, Duration::from_secs(10)))
        .expect("答えが来る");
    assert_eq!(answer.items.len(), 2);
    assert_eq!(
        of_method(&scratch, "textDocument/completion").len(),
        1,
        "待たされた 1 回の答えをそのまま使う（問い直さない）"
    );
    assert!(answer.waited_for_loading.is_some());
    manager.shutdown_all(Duration::from_secs(2));
}

/// 読み込みを待っているあいだに次の打鍵が来たら、前の要求は待ちを抜けて `Superseded`
/// （古いスレッドを上限まで残さない）。次の要求は済んだ後に一覧を受ける
#[test]
fn 読み込みを待つあいだに次の打鍵が来たら前の待ちは抜ける() {
    let scratch = Scratch::new("supersede");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, 2500, &rules()));
    let _link = open_editing(&manager, &main, TEXT);
    let first = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || {
            let outcome = manager.completion(&request(&main, true, Duration::from_secs(10)));
            (outcome, manager.server_loading(&main))
        })
    };
    // 状態で待つ: 1 本目が null を受けて読み込みの待ちへ入った（= 問い合わせが 1 回届いた）
    wait_until("1 本目の completion", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/completion").is_empty()
    });
    assert!(manager.server_loading(&main), "読み込み中と分かる");
    let second = manager.completion(&request(&main, true, Duration::from_secs(10)));
    let (first, loading_when_first_returned) = first.join().unwrap();
    assert_eq!(first, Err(CompletionError::Superseded));
    assert!(
        loading_when_first_returned,
        "前の要求は読み込みが済む前に抜けている（済むまで残っていない）"
    );
    assert_eq!(second.expect("後の要求は答えを受ける").items.len(), 2);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 読み込みを待っているあいだに一覧を閉じた（`cancel_completion`）・文書を閉じた、のどちらでも
/// 待ちを抜ける（読み込みが済むのを待たない）
#[test]
fn 読み込みを待つあいだに閉じたら待ちを抜ける() {
    let scratch = Scratch::new("close");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, 600_000, &rules()));
    let mut link = open_editing(&manager, &main, TEXT);
    // 一覧を閉じた
    let waiting = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || {
            manager.completion(&request(&main, true, Duration::from_secs(60)))
        })
    };
    wait_until("1 本目の completion", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/completion").is_empty()
    });
    manager.cancel_completion();
    assert_eq!(waiting.join().unwrap(), Err(CompletionError::Superseded));
    // 文書を閉じた（編集モードを抜けた・ペインを閉じた）
    let waiting = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || {
            manager.completion(&request(&main, true, Duration::from_secs(60)))
        })
    };
    wait_until("2 本目の completion", Duration::from_secs(10), || {
        of_method(&scratch, "textDocument/completion").len() >= 2
    });
    manager.sync(&mut link, false, &main, TEXT, 2);
    let closed = waiting.join().unwrap();
    assert!(
        matches!(
            closed,
            Err(CompletionError::Query(tako_control::lsp::GotoError::Closed))
        ),
        "{closed:?}"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件 2（CLI / MCP）: 上限まで読み込みが終わらなければ `loading` と分かる答え
/// （打鍵の要求も同じ）。`tako lsp status` の `loading` も真
#[test]
fn 読み込みが終わらなければ上限で_loading_を返す() {
    let scratch = Scratch::new("never");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, 600_000, &rules()));
    let _link = open_editing(&manager, &main, TEXT);
    for superseding in [false, true] {
        let outcome = manager.completion(&request(&main, superseding, Duration::from_secs(2)));
        let Err(error) = outcome else {
            panic!("上限で loading になる: {outcome:?}");
        };
        assert!(
            matches!(error, CompletionError::Loading { secs: 2, .. }),
            "{error:?}"
        );
        let value = error.to_json();
        assert_eq!(value["status"], json!("loading"));
        assert!(value["server"].is_string());
    }
    let status = manager.status(None);
    assert_eq!(status["servers"][0]["loading"], json!(true), "{status}");
    // 後半（答えずに待たせる）で上限に当たっても `loading`
    let scratch = Scratch::new("never-hold");
    let main = scratch.write("src/main.rs", TEXT);
    let mut rules = rules();
    rules["hold_while_loading"] = json!(true);
    let held = LspManager::new(config(&scratch, 600_000, &rules));
    let _link = open_editing(&held, &main, TEXT);
    let outcome = held.completion(&request(&main, false, Duration::from_secs(2)));
    assert!(
        matches!(outcome, Err(CompletionError::Loading { .. })),
        "{outcome:?}"
    );
    manager.shutdown_all(Duration::from_secs(2));
    held.shutdown_all(Duration::from_secs(2));
}

/// 読み込みが済んだ後は `tako lsp status` の `loading` が偽になり、補完は待たずに答える
/// （待った欄も載らない = いつもの答えの形）
#[test]
fn 読み込みが済んだ後は待たずに答える() {
    let scratch = Scratch::new("settled");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, 300, &rules()));
    let _link = open_editing(&manager, &main, TEXT);
    wait_until("読み込みが済む", Duration::from_secs(10), || {
        manager.status(None)["servers"][0]["loading"] == json!(false)
            && manager.status(None)["servers"][0]["state"] == json!("running")
    });
    assert!(!manager.server_loading(&main));
    let answer = manager
        .completion(&request(&main, true, Duration::from_secs(10)))
        .expect("答えが来る");
    assert_eq!(answer.items.len(), 2);
    assert_eq!(answer.waited_for_loading, None);
    manager.shutdown_all(Duration::from_secs(2));
}
