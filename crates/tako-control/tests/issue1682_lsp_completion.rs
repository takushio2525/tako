//! #1682 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、補完の問い合わせ
//! （`LspManager::completion`）が送るもの・受けて直すもの・古い要求の捨て方を測る。
//!
//! 一覧の描画と打鍵（デバウンス・キーの振り分け）は GUI の中でしか測れないので、ここは「答え」まで。
//! 版の照合（[`tako_core::lsp::completion::Session`]）とキーの振り分け表は tako-core の単体、
//! `--limit` の切り方と確定（`lsp_completion_apply`）は dispatch の単体（MockHost）が持つ。
//!
//! 文言は `tako_control::lsp::text` の定数から読んで比べる（理由文を直書きしない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    CompletionError, CompletionRequest, DocLink, GotoError, Launch, LspConfig, LspManager,
};
use tako_core::lsp::completion::{At, Trigger};
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
            "tako-1682-{label}-{}-{}",
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

fn config(scratch: &Scratch, scenario: &str, rules: &Value) -> LspConfig {
    std::fs::write(scratch.rules(), rules.to_string()).unwrap();
    let args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
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

fn received(scratch: &Scratch) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn of_method(scratch: &Scratch, method: &str) -> Vec<Value> {
    received(scratch)
        .into_iter()
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

fn request(path: &Path, line: usize, column: usize, superseding: bool) -> CompletionRequest {
    CompletionRequest {
        path: path.to_path_buf(),
        line,
        column,
        timeout: Duration::from_secs(10),
        document: None,
        trigger: Trigger::Word,
        superseding,
        ticket: None,
        resolve_top: 0,
    }
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path, text: &str, version: u64) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, text, version);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

/// 受け入れ条件: 連続して 3 回要求を出したとき、偽サーバが受け取る `$/cancelRequest` が **2 件**。
///
/// 打鍵の要求（`superseding`）は次の要求が来た時点で前の要求を取り消す。偽サーバは答えを
/// 遅らせる（`delay_ms`）ので、2 本目・3 本目が出たときに前の要求はまだ答えを待っている。
/// 取り消された 2 本は `Superseded` で返り（答えを待たない）、最後の 1 本だけが答えを受ける
#[test]
fn 連続して_3_回要求を出すと取り消しは_2_件() {
    let scratch = Scratch::new("cancel");
    let text = "fn main() {\n    let x = ab\n}\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!({ "items": [{ "label": "abc" }, { "label": "abd" }], "delay_ms": 1500 });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let mut handles = Vec::new();
    for n in 1..=3 {
        let (manager, main) = (manager.clone(), main.clone());
        handles.push(std::thread::spawn(move || {
            manager.completion(&request(&main, 1, 14, true))
        }));
        // 状態で待つ: n 本目が偽サーバへ届いてから次を出す（実時間で比べない）
        wait_until(
            &format!("{n} 本目の completion"),
            Duration::from_secs(10),
            || of_method(&scratch, "textDocument/completion").len() >= n,
        );
    }
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes[0], Err(CompletionError::Superseded));
    assert_eq!(outcomes[1], Err(CompletionError::Superseded));
    let last = outcomes[2].as_ref().expect("最後の 1 本は答えを受ける");
    assert_eq!(last.items.len(), 2);
    let asked: Vec<Value> = of_method(&scratch, "textDocument/completion")
        .iter()
        .map(|m| m["id"].clone())
        .collect();
    let cancels: Vec<Value> = of_method(&scratch, "$/cancelRequest")
        .iter()
        .map(|m| m["params"]["id"].clone())
        .collect();
    assert_eq!(cancels.len(), 2, "取り消しは 2 件: {cancels:?}");
    assert_eq!(cancels, asked[..2].to_vec(), "取り消すのは前の 2 本");
    // 取り消した要求は待ちの表に残らない（遅れて届く -32800 は捨てる）
    let status = manager.status(None);
    assert_eq!(status["servers"][0]["pending_requests"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));
}

/// CLI / MCP の要求（`superseding: false`）は互いに取り消し合わない（1 回ずつ答えを待つ）
#[test]
fn cli_の要求は互いに取り消さない() {
    let scratch = Scratch::new("no-cancel");
    let text = "fn main() { ab }\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!({ "items": [{ "label": "abc" }], "delay_ms": 300 });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (manager, main) = (manager.clone(), main.clone());
            std::thread::spawn(move || manager.completion(&request(&main, 0, 14, false)))
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap().expect("答えが来る").items.len(), 1);
    }
    assert!(of_method(&scratch, "$/cancelRequest").is_empty());
    manager.shutdown_all(Duration::from_secs(2));
}

/// 一覧を閉じた（`cancel_completion`）ら、答えを待っている打鍵の要求を取り消す
#[test]
fn 一覧を閉じたら待っている要求を取り消す() {
    let scratch = Scratch::new("close");
    let text = "fn main() { ab }\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!({ "items": [{ "label": "abc" }], "delay_ms": 3000 });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let handle = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || manager.completion(&request(&main, 0, 14, true)))
    };
    wait_until("completion", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/completion").is_empty()
    });
    manager.cancel_completion();
    assert_eq!(handle.join().unwrap(), Err(CompletionError::Superseded));
    wait_until("$/cancelRequest", Duration::from_secs(10), || {
        of_method(&scratch, "$/cancelRequest").len() == 1
    });
    manager.shutdown_all(Duration::from_secs(2));
}

/// 問い合わせのあいだに本文が変わったら、その答えは使わない（範囲は送った本文の座標）
#[test]
fn 待つあいだに本文が変わった答えは捨てる() {
    let scratch = Scratch::new("edited");
    let text = "fn main() { ab }\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!({ "items": [{ "label": "abc" }], "delay_ms": 800 });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let mut link = open_editing(&manager, &main, text, 1);
    let handle = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || manager.completion(&request(&main, 0, 14, false)))
    };
    wait_until("completion", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/completion").is_empty()
    });
    // 答えを待つあいだに打ち足す（didChange で文書の版が進む）
    manager.sync(&mut link, true, &main, "fn main() { abc }\n", 2);
    wait_until("didChange", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/didChange").is_empty()
    });
    assert_eq!(handle.join().unwrap(), Err(CompletionError::Edited));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 答えの座標: 範囲（`textEdit`）は UTF-16 から tako の行・バイトへ、要求の位置は UTF-16 で送る。
/// 要求には `context`（きっかけ）が載る
#[test]
fn 範囲と位置は_utf16_で往復する() {
    let scratch = Scratch::new("utf16");
    // `    let 名前 = 😀; na` = UTF-8 で 4+4+6+3+4+2+2 = 25 バイト・UTF-16 で 4+4+2+3+2+2+2 = 19
    let text = "fn main() {\n    let 名前 = 😀; na\n}\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!({ "items": [{ "label": "name" }, { "label": "nap" }], "word_edit": true });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let column = "    let 名前 = 😀; na".len();
    let mut req = request(&main, 1, column, false);
    req.trigger = Trigger::Character('.');
    let answer = manager.completion(&req).expect("答えが来る");
    assert_eq!(answer.cursor, At::new(1, column));
    assert_eq!(answer.line_text, "    let 名前 = 😀; na");
    // 語の頭 = `na` の手前（UTF-8 で 23 バイト）
    let start = "    let 名前 = 😀; ".len();
    assert!(answer.items.iter().all(|i| i.start == At::new(1, start)));
    let asked = &of_method(&scratch, "textDocument/completion")[0]["params"];
    assert_eq!(asked["position"], json!({ "line": 1, "character": 19 }));
    assert_eq!(
        asked["context"],
        json!({ "triggerKind": 2, "triggerCharacter": "." })
    );
    // 初期化の申告: スニペットは申告せず、resolve へ回すのは説明だけ
    let init = &received(&scratch)[0]["params"]["capabilities"]["textDocument"]["completion"];
    assert_eq!(init["completionItem"]["snippetSupport"], json!(false));
    manager.shutdown_all(Duration::from_secs(2));
}

/// `resolve_top` は絞り込み後の上位だけの説明を `completionItem/resolve` で補う
#[test]
fn 上位の候補だけ説明を補う() {
    let scratch = Scratch::new("resolve");
    let text = "fn main() { c }\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!({ "generate": 30, "resolve_doc": "doc of {label}" });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let mut req = request(&main, 0, 13, false);
    req.resolve_top = 3;
    let answer = manager.completion(&req).expect("答えが来る");
    assert!(answer.resolvable);
    assert_eq!(answer.items.len(), 30);
    let documented: Vec<&str> = answer
        .items
        .iter()
        .filter_map(|i| i.documentation.as_deref())
        .collect();
    assert_eq!(
        documented,
        ["doc of cand0000", "doc of cand0001", "doc of cand0002"]
    );
    assert_eq!(of_method(&scratch, "completionItem/resolve").len(), 3);
    // GUI の口（選んだ 1 件）: 元の JSON を渡すと説明つきで返る
    let one = manager
        .resolve_completion(&main, &answer.items[7].raw, None)
        .expect("resolve");
    assert_eq!(one["documentation"], json!("doc of cand0007"));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 能力に補完が無い / 未導入 / 受け持つサーバが無い は別の失敗で返す（無言で空にしない）
#[test]
fn 能力なし_未導入_対象外は区別して返す() {
    let scratch = Scratch::new("errors");
    let text = "fn main() { a }\n";
    let main = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "no-completion", &json!({})));
    let _link = open_editing(&manager, &main, text, 1);
    match manager.completion(&request(&main, 0, 13, false)) {
        Err(CompletionError::Query(GotoError::Unsupported { .. })) => {}
        other => panic!("能力なしが Unsupported にならない: {other:?}"),
    }
    manager.shutdown_all(Duration::from_secs(2));

    let missing = LspManager::new(LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(|spec: &ServerSpec| Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy::default(),
        raw_log_dir: None,
    });
    let error = missing
        .completion(&request(&main, 0, 13, false))
        .expect_err("未導入");
    assert_eq!(error.status(), "not-installed");
    let value = error.to_json();
    assert!(value["install_command"]
        .as_str()
        .is_some_and(|c| !c.is_empty()));

    let plain = scratch.write("notes.unknown-ext", "abc\n");
    assert_eq!(
        missing.completion(&request(&plain, 0, 1, false)),
        Err(CompletionError::Query(GotoError::NoServer))
    );
}

/// 同じ文書への問い合わせが 2 本重なっても、先に終わった側の一時の持ち手が文書を閉じない
/// （編集モードでない文書 = ディスクから一時的に開く）
#[test]
fn 一時的に開いた文書は重なった問い合わせが終わるまで閉じない() {
    let scratch = Scratch::new("transient");
    let main = scratch.write("src/main.rs", "fn main() { ab }\n");
    let rules = json!({ "items": [{ "label": "abc" }], "delay_ms": 400 });
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let first = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || manager.completion(&request(&main, 0, 14, false)))
    };
    wait_until("1 本目", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/completion").is_empty()
    });
    let second = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || manager.completion(&request(&main, 0, 14, false)))
    };
    assert!(first.join().unwrap().is_ok());
    assert!(second.join().unwrap().is_ok(), "2 本目も答えを受ける");
    wait_until("didClose", Duration::from_secs(10), || {
        of_method(&scratch, "textDocument/didClose").len() == 1
    });
    assert_eq!(of_method(&scratch, "textDocument/didOpen").len(), 1);
    assert_eq!(manager.status(None)["documents"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));
}
