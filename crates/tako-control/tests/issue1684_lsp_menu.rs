//! #1684 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、右クリックメニューの
//! 能力の読み口（`LspManager::menu_capabilities_now` / `menu_capabilities`）と、出し分け
//! （`tako_core::lsp::menu::items`）を通した項目の列を測る。
//!
//! GUI の描画・クリックは visual-test `lsp-context-menu`（`scripts/test-lsp-menu-1684.sh`）が持つ。
//! ここは「サーバが申告したもの → 出る項目」と「メニューを開くだけでは握手以外を送らない」まで。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, GotoError, Launch, LspConfig, LspManager, MenuCapabilities, MenuRequest,
};
use tako_core::lsp::goto::SymbolKind;
use tako_core::lsp::menu::items;
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
            "tako-1684-{label}-{}-{}",
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
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `providers` = 偽サーバが申告する能力の組（`None` ならシナリオ `normal` の既定 = 全部あり）
fn config(scratch: &Scratch, providers: Option<&Value>) -> LspConfig {
    let mut args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
    ];
    if let Some(providers) = providers {
        args.push("--providers".to_string());
        args.push(providers.to_string());
    }
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

fn request(path: &Path) -> MenuRequest {
    MenuRequest {
        path: path.to_path_buf(),
        document: None,
        timeout: Duration::from_secs(10),
    }
}

/// 実プロセスのサーバが申告した能力 → 識別子の上・選択の中で出る項目 ID の列
fn ids_for(providers: Option<Value>) -> Vec<&'static str> {
    let scratch = Scratch::new("caps");
    let main = scratch.write("src/main.rs", "fn main() {\n    helper();\n}\n");
    let manager = LspManager::new(config(&scratch, providers.as_ref()));
    let (server, capabilities) = manager
        .menu_capabilities(&request(&main))
        .expect("握手が済んで能力が読める");
    assert_eq!(server, "rust-analyzer", "検出表で .rs を受け持つサーバ");
    let ids = items(&capabilities, Some(SymbolKind::Identifier), true)
        .into_iter()
        .map(|item| item.id())
        .collect();
    manager.shutdown_all(Duration::from_secs(5));
    ids
}

/// 受け入れ条件（実プロセス）: サーバが申告した能力の 4 通り（ゼロ / 定義のみ / 全部 / 一部）で
/// 項目 ID の列が固定値と一致する。**申告に無い項目は出ない**（過大申告しない）
#[test]
fn 実プロセスの申告4通りで項目の列が固定値と一致する() {
    assert_eq!(ids_for(Some(json!({}))), Vec::<&str>::new(), "能力ゼロ");
    assert_eq!(
        ids_for(Some(json!({ "definitionProvider": true }))),
        ["lsp-definition"],
        "定義のみ"
    );
    assert_eq!(
        ids_for(None),
        [
            "lsp-definition",
            "lsp-declaration",
            "lsp-type-definition",
            "lsp-implementation",
            "lsp-format",
            "lsp-format-selection",
        ],
        "全部あり（偽サーバの既定）"
    );
    assert_eq!(
        ids_for(Some(json!({
            "implementationProvider": { "id": "impl" },
            "documentFormattingProvider": true,
            "definitionProvider": false,
        }))),
        ["lsp-implementation", "lsp-format"],
        "一部だけ"
    );
}

/// 閲覧中（文書を開いていない）の右クリック: 待たない読み口は `Pending`（サーバを起こさない）。
/// 起こして待つ読み口は**問い合わせのあいだだけ**文書を開いて閉じ、握手以外の要求を送らない。
/// 猶予つき停止までのあいだは、待たない読み口がその場で能力を返す（2 回目の右クリックは待たない）
#[test]
fn 閲覧中の右クリックは握手だけして一時的に開いた文書を閉じる() {
    let scratch = Scratch::new("viewing");
    let main = scratch.write("src/main.rs", "fn main() {\n    helper();\n}\n");
    let manager = LspManager::new(config(&scratch, None));
    assert!(
        matches!(
            manager.menu_capabilities_now(&main),
            MenuCapabilities::Pending
        ),
        "まだ起きていない"
    );
    assert!(
        manager.server_pids().is_empty(),
        "待たない読み口はサーバを起こさない"
    );
    manager
        .menu_capabilities(&request(&main))
        .expect("起こして待つ");
    wait_until("didClose", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didClose".to_string())
    });
    assert_eq!(
        methods(&scratch),
        [
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didClose",
        ],
        "メニューは能力を読むだけ（定義・整形の要求は押したときに初めて送る）"
    );
    assert_eq!(manager.status(None)["documents"], json!(0));
    // 猶予のあいだは待たずに読める（同じプロジェクトの別ファイルも同じサーバ）
    let other = scratch.write("src/other.rs", "pub fn helper() {}\n");
    for path in [&main, &other] {
        match manager.menu_capabilities_now(path) {
            MenuCapabilities::Ready {
                server,
                capabilities,
            } => {
                assert_eq!(server, "rust-analyzer");
                assert_eq!(capabilities["definitionProvider"], json!(true));
            }
            other => panic!("握手済みなのに待たずに読めない: {other:?}"),
        }
    }
    // 2 回目は文書を開き直さない（待たない読み口だけで済む）
    manager.menu_capabilities(&request(&other)).unwrap();
    assert_eq!(
        methods(&scratch)
            .iter()
            .filter(|m| *m == "textDocument/didOpen")
            .count(),
        1,
        "握手済みなら一時的に開かない"
    );
    manager.shutdown_all(Duration::from_secs(5));
}

/// 編集中（文書が開いている）なら、握手が済んだ時点で待たない読み口が能力を返す
#[test]
fn 編集中の文書は握手が済めば待たずに読める() {
    let scratch = Scratch::new("editing");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(
        &scratch,
        Some(&json!({ "typeDefinitionProvider": true })),
    ));
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &main, "fn main() {}\n", 1);
    wait_until("握手", Duration::from_secs(10), || {
        matches!(
            manager.menu_capabilities_now(&main),
            MenuCapabilities::Ready { .. }
        )
    });
    let MenuCapabilities::Ready { capabilities, .. } = manager.menu_capabilities_now(&main) else {
        unreachable!()
    };
    assert_eq!(
        items(&capabilities, Some(SymbolKind::Identifier), false)
            .into_iter()
            .map(|item| item.id())
            .collect::<Vec<_>>(),
        ["lsp-type-definition"]
    );
    // 一時的な didOpen は送らない（編集セッションの 1 つだけ）
    assert_eq!(
        methods(&scratch)
            .iter()
            .filter(|m| *m == "textDocument/didOpen")
            .count(),
        1
    );
    drop(link);
    manager.shutdown_all(Duration::from_secs(5));
}

/// 未導入・受け持つサーバが無い種類は、待たない読み口でも起こして待つ読み口でも理由で返す
/// （項目を出さない = 押しても何も起きない項目を作らない）
#[test]
fn 未導入と受け持つサーバが無い種類は理由で返す() {
    let scratch = Scratch::new("missing");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let text = scratch.write("notes.txt", "plain words\n");
    let manager = LspManager::new(LspConfig {
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
    assert!(matches!(
        manager.menu_capabilities(&request(&main)),
        Err(GotoError::NotInstalled { .. })
    ));
    // 一度分かれば待たない読み口も理由を返す（右クリックのたびに待たない）
    assert!(matches!(
        manager.menu_capabilities_now(&main),
        MenuCapabilities::Failed(GotoError::NotInstalled { .. })
    ));
    assert!(matches!(
        manager.menu_capabilities_now(&text),
        MenuCapabilities::Failed(GotoError::NoServer)
    ));
    assert!(matches!(
        manager.menu_capabilities(&request(&text)),
        Err(GotoError::NoServer)
    ));
    // LSP を止めていれば disabled
    assert!(matches!(
        LspManager::disabled().menu_capabilities_now(&main),
        MenuCapabilities::Failed(GotoError::Disabled)
    ));
}
