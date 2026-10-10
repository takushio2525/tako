//! #1930 の e2e: 読み込み中にサーバが返した**空の答え**を、manager がそれを見る前に読み込みが
//! 済んでいても「本当の空」として返さない（定義ジャンプ・補完・ホバーの 3 つ）。
//!
//! ## 真因（注入で確かめた）
//!
//! 3 つとも空の答えの後に読み込みを待ち（`wait_loaded`）、最初に見た時点で済んでいれば
//! `Settled`（= 見つからない / 候補 0 件）として返していた。空は読み込みの途中に作られた古い
//! 答えでも、manager の待ち手が見る前に `quiescent: true` が処理されると最終の答えになる
//! （Windows の CI の `issue1869_lsp_followup.rs:197`。偽サーバが空を返した直後に読み込みを
//! 終える注入で 10 回とも同じ行・同じ値で落ちた）。補完なら一覧が出ず、済んでも問い直さない。
//!
//! 偽サーバの `--settle-on-empty before`（読み込みを終えてから空を送る）でこの順序をどの機でも
//! 起こし、送ったときに読み込み中と知っていた要求は 1 回だけ問い直すこと（`EmptyAnswer`）・
//! 状態を送らないサーバは変わらないこと・問い直しは 1 回で終わることを固定する。
//! A/B は `TAKO_1930_LEGACY=1`（#1930 前 = 古い空をそのまま返す）で、同じ順序で落ちる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    CompletionRequest, DocLink, GotoRequest, HoverRequest, Launch, LspConfig, LspManager,
};
use tako_core::lsp::completion::Trigger;
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::LoadingGate;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// A/B の env（`TAKO_1930_LEGACY`）を触るテストがあるので、このファイルのテストは 1 本ずつ走る
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1930-{label}-{}-{}",
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

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn log(&self) -> PathBuf {
        self.file("received.jsonl")
    }

    /// 読み込みを終わらせる合図（このファイルのテストは作らない = 終わるのは空の答えのときだけ）
    fn gate(&self) -> LoadingGate {
        LoadingGate::new(&self.0)
    }

    fn count(&self, method: &str) -> usize {
        lsp_fake_e2e::received(&self.log(), method)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 3 つの規則（補完・ホバー・定義ジャンプ）を渡した偽サーバ
fn config(scratch: &Scratch, scenario: &str, rules: &Rules, extra: Vec<String>) -> LspConfig {
    let mut args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
    ];
    for (flag, name, value) in [
        ("--completion", "completion.json", &rules.completion),
        ("--hover", "hover.json", &rules.hover),
        ("--goto", "goto.json", &rules.goto),
    ] {
        let path = scratch.file(name);
        std::fs::write(&path, value.to_string()).unwrap();
        args.push(flag.to_string());
        args.push(path.display().to_string());
    }
    args.extend(extra);
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

/// `loading` シナリオ: 読み込みは合図では終わらず、最初の空の答えの**前**に終わる
/// （読み込み中に受けた要求の空を、済んだ後に送る = manager は空を受けた時点で済んだと知っている）
fn loading_config(scratch: &Scratch, rules: &Rules) -> LspConfig {
    let mut extra = scratch.gate().args();
    extra.extend(lsp_fake_e2e::status_delay_args());
    extra.extend(lsp_fake_e2e::settle_before_empty_args());
    config(scratch, "loading", rules, extra)
}

struct Rules {
    completion: Value,
    hover: Value,
    goto: Value,
}

/// 済んだ後は 3 つとも答えが見つかる規則
fn found(other: &Path) -> Rules {
    Rules {
        completion: json!({ "items": [{ "label": "abc" }, { "label": "abd" }] }),
        hover: json!([{ "result": { "contents": { "kind": "markdown", "value": "**ab**" } } }]),
        goto: json!([{ "result": {
            "uri": tako_core::file_uri::from_path(other),
            "range": {
                "start": { "line": 0, "character": 7 },
                "end": { "line": 0, "character": 8 },
            },
        } }]),
    }
}

/// 済んだ後も 3 つとも空で答える規則
fn nothing() -> Rules {
    Rules {
        completion: json!({ "items": [] }),
        hover: json!([]),
        goto: json!([]),
    }
}

const TEXT: &str = "fn main() { ab }\n";

/// 打鍵の補完（GUI と同じ `superseding`）
fn typing(path: &Path) -> CompletionRequest {
    CompletionRequest {
        path: path.to_path_buf(),
        line: 0,
        column: 14,
        timeout: Duration::from_secs(60),
        document: None,
        trigger: Trigger::Word,
        superseding: true,
        ticket: None,
        resolve_top: 0,
    }
}

/// CLI / MCP の補完（`tako lsp completion`）
fn cli_completion(path: &Path) -> CompletionRequest {
    CompletionRequest {
        superseding: false,
        ..typing(path)
    }
}

/// CLI / MCP のホバー（`tako lsp hover`）
fn cli_hover(path: &Path) -> HoverRequest {
    HoverRequest {
        path: path.to_path_buf(),
        line: 0,
        column: 13,
        timeout: Duration::from_secs(60),
        document: None,
        superseding: false,
        open: true,
        ticket: None,
    }
}

/// CLI / MCP の定義ジャンプ（`tako lsp definition`）
fn definition(path: &Path) -> GotoRequest {
    GotoRequest {
        kind: GotoKind::Definition,
        path: path.to_path_buf(),
        line: 0,
        column: 12,
        timeout: Duration::from_secs(60),
        document: None,
    }
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, TEXT, 1);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

/// 受け入れ条件（補完）: 読み込み中に送った打鍵の要求へサーバが空（`null`）を返し、manager が
/// それを見る前に読み込みが済んでいても、1 回だけ問い直して一覧を返す
#[test]
fn 補完_読み込み中に返った空は見る前に済んでいても問い直して一覧を返す() {
    let _env = serial();
    let scratch = Scratch::new("completion");
    let main = scratch.write("src/main.rs", TEXT);
    let other = scratch.write("src/other.rs", "pub fn ab() {}\n");
    let manager = LspManager::new(loading_config(&scratch, &found(&other)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);
    let answer = manager.completion(&typing(&main)).expect("答えが来る");
    let asked = scratch.count("textDocument/completion");
    assert_eq!(
        answer.items.len(),
        2,
        "読み込みが済んだ後に問い直した一覧（問い合わせ {asked} 回）"
    );
    assert_eq!(asked, 2, "古い空の後に 1 回だけ問い直す");
    assert!(
        answer.waited_for_loading.is_some(),
        "読み込み中に頼んだと答えに載る"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件（ホバー）: CLI / MCP のホバーも同じ判定（`wait_after_empty`）を通る
#[test]
fn ホバー_読み込み中に返った空は見る前に済んでいても問い直して本文を返す() {
    let _env = serial();
    let scratch = Scratch::new("hover");
    let main = scratch.write("src/main.rs", TEXT);
    let other = scratch.write("src/other.rs", "pub fn ab() {}\n");
    let manager = LspManager::new(loading_config(&scratch, &found(&other)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);
    let answer = manager.hover(&cli_hover(&main)).expect("答えが来る");
    let asked = scratch.count("textDocument/hover");
    let content = answer
        .content
        .unwrap_or_else(|| panic!("読み込みが済んだ後に問い直した本文（問い合わせ {asked} 回）"));
    assert_eq!(content.value, "**ab**");
    assert_eq!(asked, 2, "古い空の後に 1 回だけ問い直す");
    assert!(
        answer.waited_for_loading.is_some(),
        "読み込み中に頼んだと答えに載る"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件（定義ジャンプ）: 読み込み中の `[]` も同じく古い空として問い直す
#[test]
fn 定義ジャンプ_読み込み中に返った空は見る前に済んでいても問い直して見つける() {
    let _env = serial();
    let scratch = Scratch::new("goto");
    let main = scratch.write("src/main.rs", TEXT);
    let other = scratch.write("src/other.rs", "pub fn ab() {}\n");
    let manager = LspManager::new(loading_config(&scratch, &found(&other)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);
    let answer = manager.goto(&definition(&main)).expect("答えが来る");
    let asked = scratch.count("textDocument/definition");
    assert_eq!(
        answer.targets.len(),
        1,
        "読み込みが済んだ後に問い直して見つかる（問い合わせ {asked} 回）"
    );
    assert_eq!(answer.targets[0].path, other);
    assert_eq!(asked, 2, "古い空の後に 1 回だけ問い直す");
    manager.shutdown_all(Duration::from_secs(2));
}

/// 問い直しは 1 回だけ: 済んだ後も空で答えるサーバへの問い合わせは 2 回で終わり、空を返す
/// （問い直しの 2 回目の空は本当の空。繰り返さない判定の単体は `lsp::goto` の `EmptyAnswer`）
#[test]
fn 済んだ後も空なら問い直しは_1_回で終わる() {
    let _env = serial();
    let scratch = Scratch::new("once");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(loading_config(&scratch, &nothing()));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);
    let answer = manager
        .completion(&cli_completion(&main))
        .expect("空の答え");
    assert!(answer.items.is_empty());
    assert_eq!(
        scratch.count("textDocument/completion"),
        2,
        "古い空の後に 1 回だけ問い直し、2 回目の空はそのまま返す"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 状態を送らないサーバ（`quiescent` が無い）は #1930 の前と変わらない: 握手の直後の猶予が
/// 過ぎた後の空は、3 つとも問い直さずに空で返す（猶予の中の空は今までどおり猶予を待って問い直す）
#[test]
fn 状態を送らないサーバの空は問い直さない() {
    let _env = serial();
    let scratch = Scratch::new("no-status");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, "normal", &nothing(), Vec::new()));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_running(&manager);
    // 猶予（`STATUS_GRACE`）が過ぎて「読み込み中」でなくなるのを状態で待つ
    let deadline = Instant::now() + Duration::from_secs(60);
    while manager.status(None)["servers"][0]["loading"] != json!(false) {
        assert!(Instant::now() < deadline, "猶予が過ぎない");
        std::thread::sleep(Duration::from_millis(20));
    }
    let completion = manager
        .completion(&cli_completion(&main))
        .expect("補完の答え");
    assert!(completion.items.is_empty());
    assert!(completion.waited_for_loading.is_none());
    assert_eq!(scratch.count("textDocument/completion"), 1, "補完");
    let hover = manager.hover(&cli_hover(&main)).expect("ホバーの答え");
    assert!(hover.content.is_none());
    assert_eq!(scratch.count("textDocument/hover"), 1, "ホバー");
    let goto = manager
        .goto(&definition(&main))
        .expect("定義ジャンプの答え");
    assert!(goto.targets.is_empty());
    assert_eq!(scratch.count("textDocument/definition"), 1, "定義ジャンプ");
    manager.shutdown_all(Duration::from_secs(2));
}

/// A/B の入口が効いている: `TAKO_1930_LEGACY=1` では古い空をそのまま 0 件で返す（同じ順序で
/// #1930 の前の形が落ちる = この e2e が検出力を持つ）
#[test]
fn 旧挙動は読み込み中に返った古い空をそのまま返す() {
    let _env = serial();
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("TAKO_1930_LEGACY", value),
                None => std::env::remove_var("TAKO_1930_LEGACY"),
            }
        }
    }
    let _restore = Restore(std::env::var("TAKO_1930_LEGACY").ok());
    std::env::set_var("TAKO_1930_LEGACY", "1");
    let scratch = Scratch::new("legacy");
    let main = scratch.write("src/main.rs", TEXT);
    let other = scratch.write("src/other.rs", "pub fn ab() {}\n");
    let manager = LspManager::new(loading_config(&scratch, &found(&other)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);
    let answer = manager.completion(&typing(&main)).expect("答えが来る");
    assert!(answer.items.is_empty(), "旧挙動は古い空を 0 件で返す");
    assert_eq!(scratch.count("textDocument/completion"), 1, "問い直さない");
    manager.shutdown_all(Duration::from_secs(2));
}
