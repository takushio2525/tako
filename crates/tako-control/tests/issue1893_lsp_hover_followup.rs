//! #1893 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、ホバーの続きを測る。
//!
//! - マウスの要求（`superseding: true`）も、サーバの読み込み中は済むのを待って問い直す（補完の
//!   打鍵 = #1869 と同じ manager の待ち）。前半の即答の `null`・後半の「答えずに待たせる」の両方
//! - 待つあいだに次のマウスの要求・カードを閉じる（`cancel_hover`）・文書を閉じるで抜ける
//! - 読み込みが上限まで終わらなければ `loading`（CLI / MCP も同じ）
//! - A/B（`TAKO_1893_LEGACY=1`）はマウスの要求が待たずに空で終わる（#1893 の前）
//! - 右クリックメニューの「ホバー情報を表示」はサーバの申告（`hoverProvider`）があるときだけ
//!
//! 読み込みの再現は偽サーバの `loading` シナリオの遅延（`--loading-ms`）だけで行う（CPU を焼く
//! 負荷は使わない）。env を読む A/B があるので、このファイルのテストは [`ENV_LOCK`] で 1 本ずつ走る。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, GotoError, HoverError, HoverRequest, Launch, LspConfig, LspManager, MenuRequest,
};
use tako_core::lsp::goto::SymbolKind;
use tako_core::lsp::menu::items;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// env（`TAKO_1893_LEGACY`）を触るテストがあるので、このファイルのテストは 1 本ずつ
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
            "tako-1893-{label}-{}-{}",
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
        self.0.join("hover.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 偽サーバ（`scenario`。`loading` なら読み込みは `loading_ms`）。ホバーの規則は `rules`
fn config(scratch: &Scratch, scenario: &str, loading_ms: u64, rules: &Value) -> LspConfig {
    std::fs::write(scratch.rules(), rules.to_string()).unwrap();
    let args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--loading-ms".to_string(),
        loading_ms.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
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

const TEXT: &str = "fn main() { total }\n";

/// `total`（0 行目の 12..17）の上で問う
fn mouse(path: &Path, timeout: Duration) -> HoverRequest {
    HoverRequest {
        path: path.to_path_buf(),
        line: 0,
        column: 13,
        timeout,
        document: None,
        superseding: true,
        open: false,
        ticket: None,
    }
}

fn explicit(path: &Path, timeout: Duration) -> HoverRequest {
    HoverRequest {
        superseding: false,
        open: true,
        ..mouse(path, timeout)
    }
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, TEXT, 1);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

/// どの位置にも `**total**` の Markdown を返す規則（`hold` なら読み込みの後半 = 答えずに待たせる）
fn rules(hold: bool) -> Value {
    json!([{
        "result": { "contents": { "kind": "markdown", "value": "**total**" } },
        "hold_while_loading": hold,
    }])
}

/// 受け入れ条件 4 の本体: 読み込みの前半（サーバが即座に `null` で答える）に出した**マウスの要求**も、
/// 読み込みが済むのを待って問い直し、本文を返す。#1893 の前は `null` を受けて即座に空で終わっていた
#[test]
fn 読み込みの前半に乗せたマウスの要求も済むのを待って本文を返す() {
    let _env = serial();
    let scratch = Scratch::new("early");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, "loading", 1500, &rules(false)));
    let _link = open_editing(&manager, &main);
    let started = Instant::now();
    let answer = manager
        .hover(&mouse(&main, Duration::from_secs(10)))
        .expect("答えが来る");
    let asked = of_method(&scratch, "textDocument/hover").len();
    let content = answer.content.expect("読み込みが済んでから問い直した本文");
    assert_eq!(content.value, "**total**");
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
    println!(
        "early: {:.2}s asked={asked} waited={:?}",
        started.elapsed().as_secs_f32(),
        answer.waited_for_loading
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 読み込みの後半（サーバが答えずに待たせ、済んだ直後に答える = rust-analyzer の実測）でも、
/// マウスの要求は済んだ後の答えを受ける（問い直さない = 1 回）
#[test]
fn 読み込みの後半に待たされたマウスの要求は済んだ後の答えを返す() {
    let _env = serial();
    let scratch = Scratch::new("hold");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, "loading", 1500, &rules(true)));
    let _link = open_editing(&manager, &main);
    let answer = manager
        .hover(&mouse(&main, Duration::from_secs(10)))
        .expect("答えが来る");
    assert_eq!(answer.content.expect("本文").value, "**total**");
    assert_eq!(of_method(&scratch, "textDocument/hover").len(), 1);
    assert!(answer.waited_for_loading.is_some());
    manager.shutdown_all(Duration::from_secs(2));
}

/// 読み込みを待っているあいだに次のマウスの要求が来た・カードを閉じた（`cancel_hover`）・文書を
/// 閉じた、のどれでも待ちを抜ける（読み込みが済むのを待たない = 古いスレッドを上限まで残さない）
#[test]
fn 読み込みを待つあいだに乗せ直す_閉じると待ちを抜ける() {
    let _env = serial();
    let scratch = Scratch::new("abandon");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, "loading", 600_000, &rules(false)));
    let mut link = open_editing(&manager, &main);
    // 上限は長く・読み込みは終わらせない: 待ちを抜けたことは答えの種類（`Superseded` / `Closed`。
    // 抜けなければ上限で `Loading`）で分かる（実時間は測らない）
    let spawn = |manager: &LspManager| {
        let (manager, main) = (manager.clone(), main.clone());
        std::thread::spawn(move || manager.hover(&mouse(&main, Duration::from_secs(60))))
    };
    // 次のマウスの要求（別の語へ移った）
    let first = spawn(&manager);
    wait_until("1 本目の hover", Duration::from_secs(10), || {
        !of_method(&scratch, "textDocument/hover").is_empty()
    });
    assert!(manager.server_loading(&main), "読み込み中と分かる");
    let second = spawn(&manager);
    assert_eq!(first.join().unwrap(), Err(HoverError::Superseded));
    // カードを閉じた / 語から外れた
    wait_until("2 本目の hover", Duration::from_secs(10), || {
        of_method(&scratch, "textDocument/hover").len() >= 2
    });
    manager.cancel_hover();
    assert_eq!(second.join().unwrap(), Err(HoverError::Superseded));
    // 文書を閉じた（編集モードを抜けた・ペインを閉じた）
    let third = spawn(&manager);
    wait_until("3 本目の hover", Duration::from_secs(10), || {
        of_method(&scratch, "textDocument/hover").len() >= 3
    });
    manager.sync(&mut link, false, &main, TEXT, 2);
    assert_eq!(
        third.join().unwrap(),
        Err(HoverError::Query(GotoError::Closed))
    );
    // 待ちの表は空（取り消した要求が残っていない）
    assert_eq!(
        manager.status(None)["servers"][0]["pending_requests"],
        json!(0)
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 取り消しの番号の競合: GUI は番号を UI スレッドで先に取る（`reserve_hover` → 要求の `ticket`）。
/// 背景が走り出す前に語から外れた（`cancel_hover`）なら、背景は読み込みを待たずに `Superseded`。
/// 番号を背景で取る形（`ticket: None`）だと、取り消しを追い越して自分が最新になり、読み込みを上限まで
/// 待って `Loading` で終わる（visual-test `hover-loading` の ③ で実測した形 = この対比で固定する）。
/// 読み込みは終わらせない = 答えの種類だけで区別できる（実時間は測らない = #1220 / #962 の規約）
#[test]
fn 先に取った番号は背景が走り出す前の取り消しで抜ける() {
    let _env = serial();
    let scratch = Scratch::new("ticket");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, "loading", 600_000, &rules(false)));
    let _link = open_editing(&manager, &main);
    // UI: 番号を取って背景へ渡す前に、語から外れて取り消した
    let ticket = manager.reserve_hover();
    assert!(ticket.is_some());
    manager.cancel_hover();
    let outcome = manager.hover(&HoverRequest {
        ticket,
        ..mouse(&main, Duration::from_secs(3))
    });
    assert_eq!(
        outcome,
        Err(HoverError::Superseded),
        "読み込みを待たずに抜ける"
    );
    // 対比: 番号を背景で取る形は、先の取り消しを追い越して読み込みを上限まで待つ
    manager.cancel_hover();
    let overtaken = manager.hover(&mouse(&main, Duration::from_secs(3)));
    assert!(
        matches!(overtaken, Err(HoverError::Loading { .. })),
        "背景で取ると取り消しを追い越して上限まで待つ: {overtaken:?}"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 上限まで読み込みが終わらなければ、マウスも明示（CLI / MCP）も `loading`（前半の空・後半の
/// 待たせるの両方）。`to_json` は status / server / 理由 / 次の一手を持つ
#[test]
fn 読み込みが終わらなければ上限で_loading_を返す() {
    let _env = serial();
    for hold in [false, true] {
        let scratch = Scratch::new("never");
        let main = scratch.write("src/main.rs", TEXT);
        let manager = LspManager::new(config(&scratch, "loading", 600_000, &rules(hold)));
        let _link = open_editing(&manager, &main);
        for request in [
            mouse(&main, Duration::from_secs(2)),
            explicit(&main, Duration::from_secs(2)),
        ] {
            let outcome = manager.hover(&request);
            let Err(error) = outcome else {
                panic!("上限で loading になる（hold={hold}）: {outcome:?}");
            };
            assert!(
                matches!(error, HoverError::Loading { secs: 2, .. }),
                "hold={hold} superseding={}: {error:?}",
                request.superseding
            );
            let value = error.to_json();
            assert_eq!(value["status"], json!("loading"));
            assert_eq!(value["action"], json!("hover"));
            assert!(value["server"].is_string());
            assert!(!value["reason"].as_str().unwrap_or("").is_empty());
            assert!(!value["next_step"].as_str().unwrap_or("").is_empty());
        }
        manager.shutdown_all(Duration::from_secs(2));
    }
}

/// A/B: `TAKO_1893_LEGACY=1` のマウスの要求は #1893 の前 = 読み込み中の `null` で待たずに空で終わる
/// （明示の要求は #1681 から待つので変わらない）。新しい検査はこの腕で名指しで落ちる
#[test]
fn 旧挙動のマウスの要求は読み込みを待たずに空で終わる() {
    let _env = serial();
    let scratch = Scratch::new("legacy");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, "loading", 3000, &rules(false)));
    let _link = open_editing(&manager, &main);
    std::env::set_var("TAKO_1893_LEGACY", "1");
    let legacy = manager.hover(&mouse(&main, Duration::from_secs(10)));
    std::env::remove_var("TAKO_1893_LEGACY");
    let legacy = legacy.expect("答えは来る");
    assert_eq!(legacy.content, None, "旧挙動は空で終わる（カードが出ない）");
    assert_eq!(of_method(&scratch, "textDocument/hover").len(), 1);
    // 同じサーバの同じ読み込み中に、新しい挙動は待って本文を返す
    let new = manager
        .hover(&mouse(&main, Duration::from_secs(10)))
        .expect("答えが来る");
    assert_eq!(new.content.expect("本文").value, "**total**");
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件 1（実プロセス）: 右クリックメニューの「ホバー情報を表示」はサーバが
/// `hoverProvider` を申告したときだけ出る（`no-hover` では出ない = #1684 の出し分けと同じ）
#[test]
fn 右クリックメニューのホバーの項目は申告があるときだけ() {
    let _env = serial();
    let ids = |scenario: &str| {
        let scratch = Scratch::new(scenario);
        let main = scratch.write("src/main.rs", TEXT);
        let manager = LspManager::new(config(&scratch, scenario, 0, &json!([])));
        let (_, capabilities) = manager
            .menu_capabilities(&MenuRequest {
                path: main,
                document: None,
                timeout: Duration::from_secs(10),
            })
            .expect("握手が済んで能力が読める");
        let ids: Vec<&str> = items(&capabilities, Some(SymbolKind::Identifier), false)
            .into_iter()
            .map(|item| item.id())
            .collect();
        manager.shutdown_all(Duration::from_secs(2));
        ids
    };
    let normal = ids("normal");
    assert!(normal.contains(&"lsp-hover"), "{normal:?}");
    // 並び: 移動 → 情報 → 整形
    let hover_at = normal.iter().position(|id| *id == "lsp-hover").unwrap();
    assert_eq!(normal[hover_at - 1], "lsp-implementation", "{normal:?}");
    assert_eq!(normal[hover_at + 1], "lsp-format", "{normal:?}");
    let no_hover = ids("no-hover");
    assert!(!no_hover.contains(&"lsp-hover"), "{no_hover:?}");
    assert!(no_hover.contains(&"lsp-definition"), "{no_hover:?}");
}
