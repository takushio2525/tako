//! #1680 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、定義ジャンプの
//! 問い合わせ（`LspManager::goto`）が送るもの・受けて直すもの・後始末を測る。
//!
//! 着地（どのペインで開くか）は GUI の中でしか測れないので、ここは「答え」まで。
//! 着地の規則は `tako_core::lsp::goto::plan_landing` の単体と、`dispatch` の単体
//! （`lsp_goto_land` を MockHost で）と、GUI のセルフテストが持つ。
//!
//! 文言は `tako_control::lsp::text` の定数から読んで比べる（理由文を直書きしない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{text, DocLink, GotoError, GotoRequest, Launch, LspConfig, LspManager};
use tako_core::lsp::goto::GotoKind;
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
            "tako-1680-{label}-{}-{}",
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
        self.0.join("goto.json")
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
        "--goto".to_string(),
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

fn not_installed_config() -> LspConfig {
    LspConfig {
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

fn request(path: &Path, line: usize, column: usize, secs: u64) -> GotoRequest {
    GotoRequest {
        kind: GotoKind::Definition,
        path: path.to_path_buf(),
        line,
        column,
        timeout: Duration::from_secs(secs),
        document: None,
    }
}

fn location(path: &Path, line: u64, character: u64) -> Value {
    json!({
        "uri": tako_core::file_uri::from_path(path),
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 },
        },
    })
}

/// 受け入れ条件（答えの側）: 別ファイルの `Location` を tako の行・桁へ直して返す。
/// 開いていなかった文書は問い合わせのあいだだけ開き、答えのあと閉じる
#[test]
fn 別ファイルの定義を行と桁へ直して返し一時的に開いた文書を閉じる() {
    let scratch = Scratch::new("other");
    let main = scratch.write("src/main.rs", "fn main() {\n    helper();\n}\n");
    // 定義の行には絵文字（UTF-16 で 2・UTF-8 で 4）がある = 桁の変換を見る
    let other = scratch.write("src/other.rs", "// x\n\n/* 😀 */ pub fn helper() {}\n");
    // `/* 😀 */ pub fn ` = UTF-16 で 3 + 2 + 3 + 8 = 16 → バイトは 3 + 4 + 3 + 8 = 18・文字は 15
    let rules = json!([{ "line": 1, "result": location(&other, 2, 16) }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let answer = manager.goto(&request(&main, 1, 5, 10)).expect("答えが来る");
    assert_eq!(answer.targets.len(), 1);
    let target = &answer.targets[0];
    assert_eq!(target.path, other);
    assert_eq!(target.line, 2);
    assert_eq!(target.column, 18, "UTF-16 の 16 は UTF-8 の 18 バイト");
    assert_eq!(target.char_column, 15, "文字数では 15");
    assert_eq!(target.excerpt, "/* 😀 */ pub fn helper() {}");
    // 送ったもの: 握手 → 一時的な didOpen（ディスクの中身）→ definition（UTF-16 の位置）→ didClose
    wait_until("didClose", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didClose".to_string())
    });
    assert_eq!(
        methods(&scratch),
        vec![
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/definition",
            "textDocument/didClose",
        ]
    );
    let messages = received(&scratch);
    assert_eq!(
        messages[2]["params"]["textDocument"]["text"],
        json!("fn main() {\n    helper();\n}\n")
    );
    let asked = &messages[3]["params"];
    assert_eq!(asked["position"], json!({ "line": 1, "character": 5 }));
    assert_eq!(
        asked["textDocument"]["uri"],
        json!(tako_core::file_uri::from_path(&main))
    );
    // 開いていた文書は 0 に戻る（一時的な lease を残さない）
    assert_eq!(manager.status(None)["documents"], json!(0));
    // 初期化の申告に定義ジャンプの 4 種がある（LocationLink を受ける）
    let caps = &messages[0]["params"]["capabilities"]["textDocument"];
    for key in [
        "definition",
        "declaration",
        "typeDefinition",
        "implementation",
    ] {
        assert_eq!(caps[key]["linkSupport"], json!(true), "{key}");
    }
    manager.shutdown_all(Duration::from_secs(2));
}

/// `LocationLink` は識別子の範囲（targetSelectionRange）へ着地する。複数なら全部返す
#[test]
fn location_link_と複数の候補() {
    let scratch = Scratch::new("links");
    let main = scratch.write("src/main.rs", "fn main() { a(); }\n");
    let a = scratch.write("src/a.rs", "/// doc\npub fn a() {}\n");
    let b = scratch.write("src/b.rs", "\n\npub fn a() {}\n");
    let rules = json!([{ "result": [
        {
            "targetUri": tako_core::file_uri::from_path(&a),
            "targetRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 13 } },
            "targetSelectionRange": { "start": { "line": 1, "character": 7 }, "end": { "line": 1, "character": 8 } },
        },
        location(&b, 2, 7),
    ] }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let answer = manager.goto(&request(&main, 0, 12, 10)).unwrap();
    let places: Vec<(PathBuf, usize, usize)> = answer
        .targets
        .iter()
        .map(|t| (t.path.clone(), t.line, t.column))
        .collect();
    assert_eq!(places, vec![(a, 1, 7), (b, 2, 7)]);
    manager.shutdown_all(Duration::from_secs(2));
}

/// `#include "foo.h"` の行に対してヘッダの `Location` を返すと、飛び先はそのヘッダ
#[test]
fn include_の行はヘッダを返す() {
    let scratch = Scratch::new("include");
    let main = scratch.write(
        "src/main.c",
        "#include \"foo.h\"\nint main(void) { return 0; }\n",
    );
    let header = scratch.write("src/foo.h", "#pragma once\nint foo(void);\n");
    let rules = json!([{ "line": 0, "uri_suffix": "main.c", "result": location(&header, 0, 0) }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let at = "#include \"".len();
    let answer = manager.goto(&request(&main, 0, at, 10)).unwrap();
    assert_eq!(answer.targets.len(), 1);
    assert_eq!(answer.targets[0].path, header);
    assert_eq!((answer.targets[0].line, answer.targets[0].column), (0, 0));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 編集中（開いている）文書はその写しで問い合わせ、didOpen / didClose を足さない
#[test]
fn 編集中の文書はそのまま問い合わせ閉じない() {
    let scratch = Scratch::new("editing");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let rules = json!([{ "result": location(&main, 0, 3) }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let mut link = DocLink::default();
    // 画面の本文はディスクと違う（未保存の編集）
    manager.sync(&mut link, true, &main, "fn main() { x }\n", 3);
    wait_until("didOpen", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didOpen".to_string())
    });
    let answer = manager.goto(&request(&main, 0, 3, 10)).unwrap();
    // 飛び先の抜粋はサーバへ送った写し（ディスクではない）から採る
    assert_eq!(answer.targets[0].excerpt, "fn main() { x }");
    let seen = methods(&scratch);
    assert_eq!(
        seen.iter().filter(|m| *m == "textDocument/didOpen").count(),
        1,
        "{seen:?}"
    );
    assert!(!seen.contains(&"textDocument/didClose".to_string()));
    assert_eq!(manager.status(None)["documents"], json!(1));
    drop(link);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 開いていない文書に渡す本文（編集セッションの全文）があればディスクの代わりに使う
#[test]
fn 渡した本文で一時的に開く() {
    let scratch = Scratch::new("document");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let mut req = request(&main, 0, 0, 10);
    req.document = Some("fn main() { unsaved }\n".into());
    let answer = manager.goto(&req).unwrap();
    assert!(answer.targets.is_empty());
    let messages = received(&scratch);
    let did_open = messages
        .iter()
        .find(|m| m["method"] == json!("textDocument/didOpen"))
        .unwrap();
    assert_eq!(
        did_open["params"]["textDocument"]["text"],
        json!("fn main() { unsaved }\n")
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件: 「見つからない」「未応答（タイムアウト）」「未導入」の 3 状態が**異なる文言**。
/// 文言は `text` の定数から組んだものと一致する（理由文を直書きしない）
#[test]
fn 見つからない_未応答_未導入は別の文言() {
    let kind = GotoKind::Definition;
    // 見つからない: サーバは null で答えた
    let scratch = Scratch::new("notfound");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let answer = manager.goto(&request(&main, 0, 3, 10)).unwrap();
    assert!(answer.targets.is_empty());
    let not_found = tako_control::lsp::goto::not_found_json(kind, answer.server);
    manager.shutdown_all(Duration::from_secs(2));

    // 未応答: 答えない規則 + 上限 1 秒
    let scratch = Scratch::new("timeout");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "silent": true }])));
    // 上限で打ち切ったことは経路（`Timeout`・起動済み）で見る（実時間の予算で比べない = #962）
    let timeout = manager.goto(&request(&main, 0, 3, 1)).unwrap_err();
    assert!(
        matches!(
            timeout,
            GotoError::Timeout {
                starting: false,
                secs: 1,
                ..
            }
        ),
        "{timeout:?}"
    );
    // 打ち切ったら取り消しを送る
    wait_until("$/cancelRequest", Duration::from_secs(5), || {
        methods(&scratch).contains(&"$/cancelRequest".to_string())
    });
    manager.shutdown_all(Duration::from_secs(2));

    // 未導入
    let scratch = Scratch::new("missing");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(not_installed_config());
    let missing = manager.goto(&request(&main, 0, 3, 5)).unwrap_err();
    let GotoError::NotInstalled {
        reason,
        install_command,
        ..
    } = &missing
    else {
        panic!("未導入のはず: {missing:?}");
    };
    let spec = servers::resolve(&main).unwrap().spec;
    assert_eq!(
        reason,
        &text::fill(text::NOT_INSTALLED_REASON, &[("program", spec.program)])
    );
    assert_eq!(*install_command, spec.install.command());

    let texts = [
        not_found["reason"].as_str().unwrap().to_string(),
        timeout.reason(kind),
        missing.reason(kind),
    ];
    assert_eq!(
        texts[0],
        text::fill(
            text::GOTO_NOT_FOUND_REASON,
            &[("kind", tako_control::lsp::goto::kind_label(kind))]
        )
    );
    assert_eq!(
        texts[1],
        text::fill(
            text::GOTO_TIMEOUT_REASON,
            &[("server", spec.id), ("secs", "1")]
        )
    );
    assert_ne!(texts[0], texts[1]);
    assert_ne!(texts[1], texts[2]);
    assert_ne!(texts[0], texts[2]);
    // 機械可読の status も 3 つ別
    let statuses = [
        not_found["status"].as_str().unwrap(),
        timeout.status(),
        missing.status(),
    ];
    assert_eq!(statuses, ["not-found", "timeout", "not-installed"]);
}

/// 起動と握手が上限までに済まない（initialize に答えない）ときも未応答で止まる（無限に待たない）
#[test]
fn 起動が終わらなければ上限で未応答() {
    let scratch = Scratch::new("slow");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "slow", &json!([])));
    let error = manager.goto(&request(&main, 0, 3, 1)).unwrap_err();
    assert!(
        matches!(error, GotoError::Timeout { starting: true, .. }),
        "{error:?}"
    );
    // 「起動が終わらなかった」は要求の未応答と別の文（読み込み中の可能性を添える）
    let spec = servers::resolve(&main).unwrap().spec;
    assert_eq!(
        error.reason(GotoKind::Definition),
        text::fill(
            text::GOTO_START_TIMEOUT_REASON,
            &[("server", spec.id), ("secs", "1")]
        )
    );
    manager.shutdown_all(Duration::from_secs(2));
}

fn config_with(scratch: &Scratch, scenario: &str, rules: &Value, extra: &[&str]) -> LspConfig {
    let mut config = config(scratch, scenario, rules);
    let mut args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--goto".to_string(),
        scratch.rules().display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    config.launcher = Arc::new(move |_spec: &ServerSpec| Launch::Found {
        plan: ChildCmd {
            program: FAKE.to_string(),
            args: args.clone(),
        },
        program_path: FAKE.to_string(),
    });
    config
}

fn definition_count(scratch: &Scratch) -> usize {
    methods(scratch)
        .iter()
        .filter(|m| *m == "textDocument/definition")
        .count()
}

/// 読み込みの前の問い合わせに**空で答える**サーバ（rust-analyzer）: 空の答えを「見つからない」と
/// 読まず、サーバが「読み込みが済んだ」（`experimental/serverStatus` の `quiescent: true`）と
/// 知らせるのを待って問い直す。初期化で知らせてもらう能力を申告している
#[test]
fn 読み込み中の空の答えは済むのを待って問い直す() {
    let scratch = Scratch::new("loading");
    let main = scratch.write("src/main.rs", "fn main() { helper(); }\n");
    let other = scratch.write("src/other.rs", "pub fn helper() {}\n");
    let rules = json!([{ "result": location(&other, 0, 7) }]);
    let manager = LspManager::new(config(&scratch, "loading", &rules));
    let answer = manager
        .goto(&request(&main, 0, 12, 10))
        .expect("答えが来る");
    assert_eq!(
        answer.targets.len(),
        1,
        "読み込みが済んでから問い直して見つかる"
    );
    assert_eq!(answer.targets[0].path, other);
    assert_eq!(
        definition_count(&scratch),
        2,
        "空の答えのあと 1 回だけ問い直す"
    );
    assert_eq!(
        received(&scratch)[0]["params"]["capabilities"]["experimental"]["serverStatusNotification"],
        json!(true)
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 読み込みが上限までに済まなければ「起動が終わらなかった」系の未応答（見つからないにしない）
#[test]
fn 読み込みが上限までに済まなければ未応答() {
    let scratch = Scratch::new("loading-slow");
    let main = scratch.write("src/main.rs", "fn main() { helper(); }\n");
    let manager = LspManager::new(config_with(
        &scratch,
        "loading",
        &json!([]),
        &["--loading-ms", "60000"],
    ));
    let error = manager.goto(&request(&main, 0, 12, 1)).unwrap_err();
    assert!(
        matches!(error, GotoError::Timeout { starting: true, .. }),
        "{error:?}"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 状態を送らないサーバ（clangd・偽サーバの normal）: 空の答えは見つからない。
/// 握手の直後だけは知らせを猶予のあいだ待ち、1 回だけ問い直す（それ以上は待たない）
#[test]
fn 状態を送らないサーバの空の答えは見つからない() {
    let scratch = Scratch::new("nostatus");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let answer = manager.goto(&request(&main, 0, 3, 10)).unwrap();
    assert!(answer.targets.is_empty());
    let asked = definition_count(&scratch);
    assert!(asked <= 2, "握手の直後の猶予で 1 回だけ問い直す: {asked}");
    // 猶予を過ぎてからの空の答えは待たずに見つからない（問い直さない）
    std::thread::sleep(tako_control::lsp::goto::STATUS_GRACE);
    let before = definition_count(&scratch);
    let again = manager.goto(&request(&main, 0, 3, 10)).unwrap();
    assert!(again.targets.is_empty());
    assert_eq!(
        definition_count(&scratch),
        before + 1,
        "猶予の後は 1 回きり"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 能力に無い種類は問い合わせずに「未対応」を返す
#[test]
fn 能力に無ければ未対応() {
    let scratch = Scratch::new("nogoto");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "no-goto", &json!([])));
    let error = manager.goto(&request(&main, 0, 3, 10)).unwrap_err();
    assert!(matches!(error, GotoError::Unsupported { .. }), "{error:?}");
    assert!(!methods(&scratch).contains(&"textDocument/definition".to_string()));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 問い合わせの途中でサーバが落ちたら「落ちた」（未応答まで待たない）
#[test]
fn 途中で落ちたら落ちたと返す() {
    let scratch = Scratch::new("crash");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "crash": true }])));
    // 未応答（10 秒の上限）まで待たずに「落ちた」で抜ける = 経路で見る
    let error = manager.goto(&request(&main, 0, 3, 10)).unwrap_err();
    assert!(matches!(error, GotoError::Crashed { .. }), "{error:?}");
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け持つサーバが無い拡張子は何も起こさない（spawn しない）
#[test]
fn 受け持つサーバが無ければ何も起こさない() {
    let scratch = Scratch::new("noserver");
    let notes = scratch.write("notes.txt", "hello\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let error = manager.goto(&request(&notes, 0, 0, 10)).unwrap_err();
    assert_eq!(error, GotoError::NoServer);
    assert!(manager.server_pids().is_empty());
    assert!(received(&scratch).is_empty());
}

/// `TAKO_1007_LEGACY=1` 相当（無効な manager）は「止めている」を返す
#[test]
fn 無効な_manager_は止めていると返す() {
    let error = LspManager::disabled()
        .goto(&request(Path::new("/x/a.rs"), 0, 0, 1))
        .unwrap_err();
    assert_eq!(error, GotoError::Disabled);
    assert_eq!(
        error.reason(GotoKind::Definition),
        text::DISABLED_REASON.text()
    );
}
