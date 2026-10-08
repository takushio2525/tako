//! #1681 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、ホバーの問い合わせ
//! （`LspManager::hover`）が送るもの・受けて直すもの・古い要求の捨て方・持ち物の後始末を測る。
//!
//! カードの描画（Markdown が `render_block` を通る・四隅で見切れない・基準画像）は GUI の中でしか
//! 測れないので、ここは「答え」まで。カードの組み立ては tako-app の単体（`lsp_hover_ui`）と
//! visual-test `hover`、着地（`show`）は dispatch の単体（MockHost）が持つ。
//!
//! 文言は `tako_control::lsp::text` の定数から読んで比べる（理由文を直書きしない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, GotoError, HoverError, HoverRequest, Launch, LspConfig, LspManager,
};
use tako_core::lsp::completion::At;
use tako_core::lsp::hover::Markup;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 受け入れ条件の Markdown（見出し / コードブロック / リンク の 3 種）。tako-app の単体
/// （`lsp_hover_ui` の `markdown_の答えは_render_block_を通る`）も同じ 3 種で描画を固定する
const MARKDOWN: &str = "# Vec\n\n```rust\npub fn len(&self) -> usize\n```\n\n[docs](https://doc.rust-lang.org/std/vec/struct.Vec.html)";

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1681-{label}-{}-{}",
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

    fn spawns(&self) -> PathBuf {
        self.0.join("spawns.txt")
    }

    fn rules(&self) -> PathBuf {
        self.0.join("hover.json")
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
        "--spawns".to_string(),
        scratch.spawns().display().to_string(),
        "--hover".to_string(),
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

fn spawn_count(scratch: &Scratch) -> usize {
    std::fs::read_to_string(scratch.spawns())
        .unwrap_or_default()
        .lines()
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

/// CLI / MCP・メニューの形（取り消し合わない・開いていなければ開く）
fn explicit(path: &Path, line: usize, column: usize) -> HoverRequest {
    HoverRequest {
        path: path.to_path_buf(),
        line,
        column,
        timeout: Duration::from_secs(10),
        document: None,
        superseding: false,
        open: true,
        ticket: None,
    }
}

/// GUI のマウスの形（前の 1 つを取り消す・開いていなければ問い合わせない）
fn mouse(path: &Path, line: usize, column: usize) -> HoverRequest {
    HoverRequest {
        superseding: true,
        open: false,
        ..explicit(path, line, column)
    }
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path, text: &str, version: u64) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, text, version);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

fn server_status(manager: &LspManager) -> Value {
    manager.status(None)["servers"][0].clone()
}

/// 受け入れ条件: 偽サーバが Markdown の `Hover`（見出し / コードブロック / リンク）を返したとき、
/// 答えの本文は**1 バイトも変えずに** Markdown として届く（描画は tako-app が `render_block` で行う）。
/// 範囲は UTF-16 から tako の行・バイトへ写る
#[test]
fn markdown_の答えは本文と範囲をそのまま届ける() {
    let scratch = Scratch::new("markdown");
    let text = "fn main() {\n    let v: Vec<u8> = Vec::new();\n}\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!([{
        "line": 1,
        "result": {
            "contents": { "kind": "markdown", "value": MARKDOWN },
            "range": { "start": { "line": 1, "character": 11 }, "end": { "line": 1, "character": 14 } },
        },
    }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let answer = manager.hover(&explicit(&main, 1, 12)).expect("答えが来る");
    let content = answer.content.expect("本文がある");
    assert_eq!(content.markup, Markup::Markdown);
    assert_eq!(content.value, MARKDOWN);
    assert!(!content.truncated);
    assert_eq!(answer.range, Some((At::new(1, 11), At::new(1, 14))));
    let asked = &of_method(&scratch, "textDocument/hover")[0]["params"];
    assert_eq!(asked["position"], json!({ "line": 1, "character": 12 }));
    // 初期化の申告: Markdown を先に
    let init = &received(&scratch)[0]["params"]["capabilities"]["textDocument"]["hover"];
    assert_eq!(init["contentFormat"], json!(["markdown", "plaintext"]));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件: プレーンテキストの `Hover` はエスケープされずそのまま届く。古い形
/// （`MarkedString[]`）は Markdown として 1 本につながる
#[test]
fn 平文と古い形の答え() {
    let scratch = Scratch::new("plain");
    let text = "int main() { return a; }\n";
    let main = scratch.write("src/main.rs", text);
    let plain = "a < b && *c* `d` <br>";
    let rules = json!([
        { "line": 0, "uri_suffix": "main.rs", "result": { "contents": { "kind": "plaintext", "value": plain } } },
    ]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let content = manager
        .hover(&explicit(&main, 0, 20))
        .expect("答えが来る")
        .content
        .expect("本文がある");
    assert_eq!(content.markup, Markup::PlainText);
    assert_eq!(content.value, plain, "平文は解釈もエスケープもしない");
    manager.shutdown_all(Duration::from_secs(2));

    let scratch = Scratch::new("marked");
    let main = scratch.write("src/main.rs", text);
    let rules =
        json!([{ "result": { "contents": [{ "language": "c", "value": "int a" }, "doc"] } }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let content = manager
        .hover(&explicit(&main, 0, 20))
        .expect("答えが来る")
        .content
        .expect("本文がある");
    assert_eq!(content.markup, Markup::Markdown);
    assert_eq!(content.value, "```c\nint a\n```\n\ndoc");
    manager.shutdown_all(Duration::from_secs(2));
}

/// 空の答え（`null` / 空の本文）は「表示するものが無い」（失敗ではない）
#[test]
fn 空の答えは本文なしで返る() {
    let scratch = Scratch::new("empty");
    let text = "fn main() {}\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!([
        { "line": 0, "result": { "contents": { "kind": "markdown", "value": "  " } } },
    ]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let answer = manager.hover(&explicit(&main, 0, 3)).expect("答えが来る");
    assert_eq!(answer.content, None);
    assert_eq!(answer.range, None);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 位置は UTF-16 で送り、範囲は UTF-16 から tako の座標へ戻る（偽サーバは問われた語を自前の
/// 本文の模型で引いて返す = tako の変換とサーバの数え方が揃っているかの往復）
#[test]
fn 位置と範囲は_utf16_で往復する() {
    let scratch = Scratch::new("utf16");
    // `    let 名前 = 😀; target` = target の頭は UTF-8 で 4+4+6+3+4+2 = 23・UTF-16 で 4+4+2+3+2+2 = 17
    let text = "fn main() {\n    let 名前 = 😀; target\n}\n";
    let main = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "echo": true }])));
    let _link = open_editing(&manager, &main, text, 1);
    let column = "    let 名前 = 😀; ta".len();
    let answer = manager.hover(&explicit(&main, 1, column)).expect("答え");
    assert_eq!(answer.content.unwrap().value, "**target**");
    let start = "    let 名前 = 😀; ".len();
    assert_eq!(
        answer.range,
        Some((At::new(1, start), At::new(1, start + "target".len())))
    );
    let asked = &of_method(&scratch, "textDocument/hover")[0]["params"];
    assert_eq!(asked["position"], json!({ "line": 1, "character": 19 }));
    manager.shutdown_all(Duration::from_secs(2));
}

/// マウスの要求（`superseding`）は次の要求が来た時点で前の要求を `$/cancelRequest` で取り消す。
/// 連続して 3 回乗せ直すと取り消しは 2 件で、最後の 1 本だけが答えを受ける
#[test]
fn マウスの要求は前の要求を取り消す() {
    let scratch = Scratch::new("cancel");
    let text = "fn main() { alpha }\n";
    let main = scratch.write("src/main.rs", text);
    let rules = json!([{ "echo": true, "delay_ms": 1500 }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let mut handles = Vec::new();
    for n in 1..=3 {
        let (manager, main) = (manager.clone(), main.clone());
        handles.push(std::thread::spawn(move || {
            manager.hover(&mouse(&main, 0, 13))
        }));
        // 状態で待つ: n 本目が偽サーバへ届いてから次を出す（実時間で比べない）
        wait_until(
            &format!("{n} 本目の hover"),
            Duration::from_secs(10),
            || of_method(&scratch, "textDocument/hover").len() >= n,
        );
    }
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes[0], Err(HoverError::Superseded));
    assert_eq!(outcomes[1], Err(HoverError::Superseded));
    let last = outcomes[2].as_ref().expect("最後の 1 本は答えを受ける");
    assert_eq!(last.content.as_ref().unwrap().value, "**alpha**");
    let asked: Vec<Value> = of_method(&scratch, "textDocument/hover")
        .iter()
        .map(|m| m["id"].clone())
        .collect();
    let cancels: Vec<Value> = of_method(&scratch, "$/cancelRequest")
        .iter()
        .map(|m| m["params"]["id"].clone())
        .collect();
    assert_eq!(cancels, asked[..2].to_vec(), "取り消すのは前の 2 本");
    // カードを閉じた（`cancel_hover`）ら、待っている要求も取り消す
    let handle = {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || manager.hover(&mouse(&main, 0, 13)))
    };
    wait_until("4 本目", Duration::from_secs(10), || {
        of_method(&scratch, "textDocument/hover").len() == 4
    });
    manager.cancel_hover();
    assert_eq!(handle.join().unwrap(), Err(HoverError::Superseded));
    wait_until("3 件目の取り消し", Duration::from_secs(10), || {
        of_method(&scratch, "$/cancelRequest").len() == 3
    });
    assert_eq!(server_status(&manager)["pending_requests"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));
}

/// マウスの要求は**開いていない文書でサーバを起こさない**（設計書 §16-2。乗せただけで
/// rust-analyzer が起きない）。明示の要求は問い合わせのあいだだけ開いて閉じる
#[test]
fn マウスは開いていない文書でサーバを起こさず_明示の要求は一時的に開く() {
    let scratch = Scratch::new("not-open");
    let main = scratch.write("src/main.rs", "fn main() { alpha }\n");
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "echo": true }])));
    assert!(!manager.has_document(&main));
    assert_eq!(
        manager.hover(&mouse(&main, 0, 13)),
        Err(HoverError::NotOpen)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(spawn_count(&scratch), 0, "マウスでサーバを起こさない");
    // 明示の要求: ディスクの本文で一時的に開き、答えのあと閉じる
    let answer = manager.hover(&explicit(&main, 0, 13)).expect("答え");
    assert_eq!(answer.content.unwrap().value, "**alpha**");
    wait_until("didClose", Duration::from_secs(10), || {
        of_method(&scratch, "textDocument/didClose").len() == 1
    });
    assert_eq!(manager.status(None)["documents"], json!(0));
    assert!(!manager.has_document(&main));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件: ホバーを 100 回出し入れしても保持件数が増え続けない。
///
/// GUI の 1 回の「出し入れ」 = マウスの要求 1 本 + カードを閉じる（`cancel_hover`）。100 回の後で、
/// 待ちの表（`pending_requests`）は 0・文書は 1・持ち手（`views`）は編集セッションの 1 つだけ
/// （マウスの要求が加わった持ち手が 1 つも残らない）、偽サーバが受けた要求はちょうど 100 本
#[test]
fn 百回出し入れしても保持件数は増えない() {
    let scratch = Scratch::new("hundred");
    let text = "fn main() { alpha }\n";
    let main = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "echo": true }])));
    let _link = open_editing(&manager, &main, text, 1);
    // 1 回目で握手まで済ませてから基準を採る
    manager.hover(&mouse(&main, 0, 13)).expect("1 回目");
    manager.cancel_hover();
    let before = server_status(&manager);
    for n in 0..100 {
        let answer = manager.hover(&mouse(&main, 0, 13 + n % 3));
        assert!(answer.is_ok(), "{n} 回目: {answer:?}");
        manager.cancel_hover();
    }
    let after = server_status(&manager);
    assert_eq!(after["pending_requests"], json!(0));
    assert_eq!(after["documents"], before["documents"]);
    assert_eq!(after["views"], before["views"]);
    assert_eq!(after["views"], json!(1), "編集セッションの持ち手だけ");
    assert_eq!(of_method(&scratch, "textDocument/hover").len(), 101);
    // 答えを受けた要求は取り消さない（閉じたときには待っている要求が無い）
    assert!(of_method(&scratch, "$/cancelRequest").is_empty());
    manager.shutdown_all(Duration::from_secs(2));
}

/// 能力に無い / 未応答 / 落ちた / 未導入 / 対象外 は別の失敗で返す（無言で空にしない）
#[test]
fn 能力なし_未応答_落ちた_未導入_対象外は区別して返す() {
    let scratch = Scratch::new("errors");
    let text = "fn main() { a }\n";
    let main = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "no-hover", &json!([])));
    let _link = open_editing(&manager, &main, text, 1);
    match manager.hover(&explicit(&main, 0, 12)) {
        Err(HoverError::Query(GotoError::Unsupported { .. })) => {}
        other => panic!("能力なしが Unsupported にならない: {other:?}"),
    }
    manager.shutdown_all(Duration::from_secs(2));

    let scratch = Scratch::new("silent");
    let main = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "silent": true }])));
    let _link = open_editing(&manager, &main, text, 1);
    // 状態を送らないサーバは握手の直後の猶予（`STATUS_GRACE`）のあいだ「読み込み中」とみなす
    // （#1893 で補完と同じく、その間の上限は `loading`）。未応答の区別は猶予が明けてから測る
    wait_until(
        "握手の直後の猶予が明ける",
        Duration::from_secs(10),
        || {
            let status = server_status(&manager);
            status["state"] == json!("running") && status["loading"] == json!(false)
        },
    );
    let mut request = explicit(&main, 0, 12);
    request.timeout = Duration::from_secs(1);
    match manager.hover(&request) {
        Err(HoverError::Query(GotoError::Timeout { .. })) => {}
        other => panic!("未応答が Timeout にならない: {other:?}"),
    }
    assert_eq!(server_status(&manager)["pending_requests"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));

    let scratch = Scratch::new("crash");
    let main = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "crash": true }])));
    let _link = open_editing(&manager, &main, text, 1);
    match manager.hover(&explicit(&main, 0, 12)) {
        Err(HoverError::Query(GotoError::Crashed { .. })) => {}
        other => panic!("落ちたが Crashed にならない: {other:?}"),
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
    let error = missing.hover(&explicit(&main, 0, 12)).expect_err("未導入");
    assert_eq!(error.status(), "not-installed");
    assert!(error.to_json()["install_command"]
        .as_str()
        .is_some_and(|c| !c.is_empty()));
    let plain = scratch.write("notes.unknown-ext", "abc\n");
    assert_eq!(
        missing.hover(&explicit(&plain, 0, 1)),
        Err(HoverError::Query(GotoError::NoServer))
    );
    // LSP を止めている manager は Disabled
    assert_eq!(
        LspManager::disabled().hover(&explicit(&main, 0, 1)),
        Err(HoverError::Query(GotoError::Disabled))
    );
}

/// 巨大な doc: manager は**全文**を返し（#1893）、切るのは出口（CLI / MCP は `limit` = 省略で
/// 16,000 字・0 で全文。カードはいつも 16,000 字 = tako-app の単体）。切ったら印と全文の取り方の注記
#[test]
fn 巨大な_doc_は全文で届き出口で切る() {
    let scratch = Scratch::new("huge");
    let text = "fn main() { a }\n";
    let main = scratch.write("src/main.rs", text);
    let big: String = (0..2000)
        .map(|i| format!("line {i} of the doc\n"))
        .collect();
    let rules = json!([{ "result": { "contents": { "kind": "markdown", "value": big } } }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let _link = open_editing(&manager, &main, text, 1);
    let content = manager
        .hover(&explicit(&main, 0, 12))
        .expect("答え")
        .content
        .expect("本文");
    assert!(!content.truncated, "manager は切らない");
    assert_eq!(content.value, big);
    assert_eq!(content.total_chars, big.chars().count());
    // 出口（CLI / MCP の応答）: 省略 = 既定の上限で切る / 0 = 全文
    let cut = tako_control::lsp::hover::found_json("s", &content, None, None);
    assert_eq!(cut["truncated"], json!(true));
    let shown = cut["contents"].as_str().unwrap();
    assert!(shown.chars().count() <= tako_core::lsp::hover::MAX_CHARS);
    assert!(big.starts_with(shown));
    let full = tako_control::lsp::hover::found_json("s", &content, None, Some(0));
    assert_eq!(full["contents"], json!(big));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 実の rust-analyzer と握手して `String` の上で std の doc が届く（手動実行。CI には入れない =
/// 導入が重い・答えが環境依存）。`cargo test -p tako-control --test issue1681_lsp_hover -- --ignored`。
/// rust-analyzer が PATH に無ければ何もせずに終わる（SKIPPED と出す）
#[test]
#[ignore = "実の rust-analyzer が要る（導入が重く答えが環境依存 = CI に入れない）。手で --ignored を付けて走らせる"]
fn 実の_rust_analyzer_が_doc_を返す() {
    let Some(program) = tako_core::platform::exe::find("rust-analyzer") else {
        println!("SKIPPED: rust-analyzer が無い");
        return;
    };
    let scratch = Scratch::new("real");
    std::fs::write(
        scratch.0.join("Cargo.toml"),
        "[package]\nname = \"real\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let text = "fn main() {\n    let s = String::new();\n    println!(\"{}\", s.len());\n}\n";
    let main = scratch.write("src/main.rs", text);
    let program = program.to_string();
    let manager = LspManager::new(LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: program.clone(),
                args: Vec::new(),
            },
            program_path: program.clone(),
        }),
        request_timeout: Duration::from_secs(120),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy::default(),
        raw_log_dir: None,
    });
    let _link = open_editing(&manager, &main, text, 1);
    let started = Instant::now();
    let mut request = explicit(&main, 1, 13);
    request.timeout = Duration::from_secs(120);
    let answer = manager.hover(&request).expect("実サーバが答える");
    let content = answer.content.expect("String の doc がある");
    println!(
        "real rust-analyzer: {:.1}s kind={} chars={} range={:?}\n{}",
        started.elapsed().as_secs_f32(),
        content.markup.slug(),
        content.total_chars,
        answer.range,
        content
            .value
            .lines()
            .take(12)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(content.markup, Markup::Markdown);
    assert!(content.value.contains("String"), "{}", content.value);
    assert!(
        content.value.contains("```rust"),
        "コードブロックで型を返す"
    );
    // `String` の範囲（行 1 の `    let s = ` の後ろ = 桁 12〜18）
    assert_eq!(answer.range, Some((At::new(1, 12), At::new(1, 18))));
    manager.shutdown_all(Duration::from_secs(5));
}
