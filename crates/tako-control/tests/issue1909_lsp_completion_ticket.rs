//! #1909 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、補完の打鍵の要求と説明の補いの
//! **取り消しの番号を UI スレッドで先に取る**形（ホバーの #1893 と同じ口 = manager の `enter_lane`）を測る。
//!
//! # 競合（#1893 でホバーだけ直した形）
//!
//! GUI は打鍵の要求を背景へ渡し、背景が `manager.completion` の頭で取り消しの番号を取っていた。背景が
//! 走り出す前に UI が一覧を閉じた・語の外へ出た（`cancel_completion`）と、背景は**その取り消しを
//! 追い越して**自分が最新になり、読み込みが済むまで待ち続ける（サーバが答えずに待たせる読み込みの
//! 後半なら、要求は `pending_requests` に残る）。済んだ後は取り消したはずの一覧の答えを受け取る。
//!
//! - 直した形: UI が `reserve_completion` / `reserve_resolve` で番号を先に取り、要求に載せる。背景は
//!   その番号で列へ入るので、先に出た取り消しで問い合わせずに `Superseded` で抜ける
//! - 対比の旧い形: 要求の `ticket: None`（manager が背景で取る = #1909 の前）。GUI の A/B
//!   （`TAKO_1909_LEGACY=1`）はこの形へ戻す
//! - 観測: `tako lsp status` の `inflight`（列へ入ってまだ答えを返していない数）と、サーバごとの
//!   `pending_requests`（答えを待っている要求の数）が 0 に戻るか
//!
//! 順序は実時間に任せない: 「背景が走り出す前に取り消す」はテストの手で順に呼ぶか、背景の走り出しを
//! 合図のファイルまで止める注入（`TAKO_1909_INJECT_HOLD`）で作る。読み込みの終わりは
//! `common/lsp_fake_e2e.rs`（#1922）の合図で決める。env を触るので、このファイルのテストは 1 本ずつ。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::LoadingGate;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// env（`TAKO_1909_INJECT_HOLD`）を触るテストがあるので、このファイルのテストは 1 本ずつ
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
            "tako-1909-{label}-{}-{}",
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

    /// `loading` の読み込みを終わらせる合図（作るまで読み込みは終わらない）
    fn gate(&self) -> LoadingGate {
        LoadingGate::new(&self.0)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `loading` シナリオの偽サーバ（読み込みは `scratch.gate()` を終わらせるまで続く）
fn config(scratch: &Scratch, rules: &Value) -> LspConfig {
    std::fs::write(scratch.rules(), rules.to_string()).unwrap();
    let mut args = vec![
        "--scenario".to_string(),
        "loading".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--completion".to_string(),
        scratch.rules().display().to_string(),
    ];
    args.extend(scratch.gate().args());
    args.extend(lsp_fake_e2e::status_delay_args());
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

/// 状態が立つまで待つ（上限は止まったときの保険。届けば待たずに進む）
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が 60 秒以内に起きなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `tako lsp status` の `inflight`（列へ入ってまだ答えを返していない数）
fn inflight(manager: &LspManager, lane: &str) -> u64 {
    manager.status(None)["inflight"][lane]
        .as_u64()
        .unwrap_or_else(|| panic!("status に inflight.{lane} が載る"))
}

/// サーバの待ちの表（答えを待っている要求）の数
fn pending(manager: &LspManager) -> u64 {
    manager.status(None)["servers"][0]["pending_requests"]
        .as_u64()
        .expect("pending_requests が載る")
}

fn received(scratch: &Scratch, method: &str) -> usize {
    lsp_fake_e2e::received(&scratch.log(), method)
}

/// `fn main() { abcd }` の `abcd` の中（0 行目）
const TEXT: &str = "fn main() { abcd }\n";

/// 打鍵の要求（GUI と同じ = `superseding`）。`column` は打った直後のカーソル
fn typed(path: &Path, column: usize, ticket: Option<u64>) -> CompletionRequest {
    CompletionRequest {
        path: path.to_path_buf(),
        line: 0,
        column,
        // 上限は止まったときの保険（抜けたかは答えの種類で見る = 実時間は測らない）
        timeout: Duration::from_secs(60),
        document: None,
        trigger: Trigger::Word,
        superseding: true,
        ticket,
        resolve_top: 0,
    }
}

fn rules(hold: bool) -> Value {
    json!({
        "items": [{ "label": "abcd" }, { "label": "abce" }],
        "resolve_doc": "doc of {label}",
        "hold_while_loading": hold,
    })
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, TEXT, 1);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

fn spawn(
    manager: &LspManager,
    request: CompletionRequest,
) -> std::thread::JoinHandle<Result<tako_control::lsp::CompletionAnswer, CompletionError>> {
    let manager = manager.clone();
    std::thread::spawn(move || manager.completion(&request))
}

/// 受け入れ条件 1 の本体（読み込みの前半 = サーバが即座に `null` で答える）: UI が番号を取ってから
/// 背景が走り出す前に一覧を閉じた（`cancel_completion`）なら、背景は問い合わせずに `Superseded`。
/// 番号を背景で取る旧い形は取り消しを追い越し、読み込みが済むまで `inflight` に残って、済んだ後に
/// 閉じたはずの一覧の答えを受け取る
#[test]
fn 一覧を閉じた取り消しは背景が走り出す前でも効く() {
    let _env = serial();
    let scratch = Scratch::new("closed");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, &rules(false)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);

    // UI: 番号を取って背景へ渡した → 背景が走り出す前に一覧を閉じた
    let ticket = manager.reserve_completion();
    assert!(ticket.is_some());
    manager.cancel_completion();
    let outcome = manager.completion(&typed(&main, 16, ticket));
    assert_eq!(outcome, Err(CompletionError::Superseded));
    assert_eq!(
        received(&scratch, "textDocument/completion"),
        0,
        "閉じた一覧の問い合わせはサーバへ届かない"
    );
    assert_eq!(inflight(&manager, "completion"), 0, "列に残らない");

    // 対比: 番号を背景で取る旧い形（#1909 の前 = GUI の `TAKO_1909_LEGACY=1`）
    manager.cancel_completion();
    let stale = spawn(&manager, typed(&main, 16, None));
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
    assert_eq!(
        inflight(&manager, "completion"),
        1,
        "旧い形は取り消しを追い越して読み込み待ちに残る"
    );
    scratch.gate().finish();
    let answer = stale.join().unwrap().expect("済んだ後に答えを受け取る");
    assert!(
        answer.waited_for_loading.is_some(),
        "閉じたはずの一覧の答えを、読み込みが済むまで待って受け取った"
    );
    assert_eq!(inflight(&manager, "completion"), 0);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件 1（読み込みの後半 = サーバが答えずに待たせる）: 待たされている打鍵の要求は、一覧を
/// 閉じると `$/cancelRequest` で捨てられ、待ちの表（`pending_requests`）も列（`inflight`）も 0 に戻る。
/// 旧い形で取り消しを追い越した要求は、読み込みが済むまで待ちの表に残る
#[test]
fn 答えない間に閉じた要求は待ちの表に残らない() {
    let _env = serial();
    let scratch = Scratch::new("hold");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, &rules(true)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);

    let ticket = manager.reserve_completion();
    let waiting = spawn(&manager, typed(&main, 16, ticket));
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
    assert_eq!(pending(&manager), 1, "サーバが答えずに待たせている");
    assert_eq!(inflight(&manager, "completion"), 1);
    manager.cancel_completion();
    assert_eq!(waiting.join().unwrap(), Err(CompletionError::Superseded));
    assert_eq!(pending(&manager), 0, "取り消した要求は待ちの表に残らない");
    assert_eq!(inflight(&manager, "completion"), 0);
    assert_eq!(
        received(&scratch, "$/cancelRequest"),
        1,
        "サーバにも捨てさせた"
    );

    // 対比: 取り消しの後に背景が番号を取る旧い形は、待ちの表に残る
    manager.cancel_completion();
    let stale = spawn(&manager, typed(&main, 16, None));
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 2);
    assert_eq!(
        pending(&manager),
        1,
        "旧い形は取り消したはずの要求が待ちの表に残る"
    );
    assert_eq!(inflight(&manager, "completion"), 1);
    scratch.gate().finish();
    stale.join().unwrap().expect("済んだ後に答えを受け取る");
    assert_eq!(pending(&manager), 0);
    assert_eq!(inflight(&manager, "completion"), 0);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 速い連打: 番号は打った順に UI で取るので、背景の走り出す順が逆でも**最後に打った 1 つだけ**が
/// 問い合わせて答えを受ける（後から走り出した古い打鍵はサーバへ届かずに抜ける）。番号を背景で
/// 取る旧い形では、最後に走り出したものが勝つ（打った順に依らない）
#[test]
fn 速い連打は最後に打った1つだけが答えを受ける() {
    let _env = serial();
    let scratch = Scratch::new("burst");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, &rules(false)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);

    // `a` `b` `c` `d` を打った（デバウンスが明けるたびに UI が番号を取る）
    let tickets: Vec<(usize, Option<u64>)> = (13..=16)
        .map(|column| (column, manager.reserve_completion()))
        .collect();
    // 最後の打鍵の背景が先に走り出し、読み込みの待ちへ入った
    let (last_column, last_ticket) = tickets[3];
    let last = spawn(&manager, typed(&main, last_column, last_ticket));
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
    // 前の打鍵の背景が遅れて走り出した（混んだ background executor）
    for &(column, ticket) in tickets[..3].iter().rev() {
        assert_eq!(
            manager.completion(&typed(&main, column, ticket)),
            Err(CompletionError::Superseded),
            "{column} 桁の古い打鍵は抜ける"
        );
    }
    assert_eq!(
        received(&scratch, "textDocument/completion"),
        1,
        "古い打鍵はサーバへ届かない"
    );
    assert_eq!(
        inflight(&manager, "completion"),
        1,
        "残っているのは最後の 1 つ"
    );
    scratch.gate().finish();
    let answer = last.join().unwrap().expect("最後の打鍵は答えを受ける");
    assert_eq!(answer.cursor.col, last_column, "最後に打った位置の答え");
    assert_eq!(inflight(&manager, "completion"), 0);
    assert_eq!(pending(&manager), 0);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 一覧を閉じた直後に次の語を打った: 閉じた一覧の背景が新しい打鍵より遅れて走り出しても、新しい
/// 打鍵を取り消さない（旧い形は遅れて走り出した側が新しい打鍵を取り消して勝つ）
#[test]
fn 閉じた直後の打鍵は遅れて走り出した前の要求に取り消されない() {
    let _env = serial();
    let scratch = Scratch::new("reopen");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, &rules(false)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);

    let closed = manager.reserve_completion();
    manager.cancel_completion();
    let next = manager.reserve_completion();
    let fresh = spawn(&manager, typed(&main, 16, next));
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
    assert_eq!(
        manager.completion(&typed(&main, 15, closed)),
        Err(CompletionError::Superseded),
        "閉じた一覧の要求は遅れて走り出しても抜ける"
    );
    assert_eq!(received(&scratch, "textDocument/completion"), 1);
    scratch.gate().finish();
    let answer = fresh.join().unwrap().expect("新しい打鍵は取り消されない");
    assert_eq!(answer.cursor.col, 16);
    assert!(answer.waited_for_loading.is_some());
    assert_eq!(inflight(&manager, "completion"), 0);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 説明の補い（`completionItem/resolve`）も同じ口: UI が番号を取ってから一覧を閉じたら、背景は
/// サーバへ問い合わせずに抜ける。旧い形は閉じた一覧の説明をサーバへ問い合わせる
#[test]
fn 説明の補いも閉じた取り消しを追い越さない() {
    let _env = serial();
    let scratch = Scratch::new("resolve");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, &rules(false)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);
    let item = json!({ "label": "abcd" });

    let ticket = manager.reserve_resolve();
    assert!(ticket.is_some());
    manager.cancel_completion();
    assert_eq!(
        manager.resolve_completion(&main, &item, ticket),
        Err(CompletionError::Superseded)
    );
    assert_eq!(received(&scratch, "completionItem/resolve"), 0);
    assert_eq!(inflight(&manager, "resolve"), 0);

    // 対比: 旧い形は閉じた後でも問い合わせる
    manager.cancel_completion();
    let stale = manager
        .resolve_completion(&main, &item, None)
        .expect("旧い形は問い合わせて答えを受ける");
    assert_eq!(stale["documentation"], json!("doc of abcd"));
    assert_eq!(received(&scratch, "completionItem/resolve"), 1);
    assert_eq!(inflight(&manager, "resolve"), 0);
    manager.shutdown_all(Duration::from_secs(2));
}

/// GUI と同じ順序を注入で作る（visual-test が頼る注入そのものの確認）: 背景は要求を受けて列へ入り
/// （`inflight` = 1）、合図まで止まる。その間に UI が一覧を閉じ、合図で走り出す。先に取った番号なら
/// サーバへ届かずに抜け、旧い形（背景で取る）なら取り消しを追い越して読み込み待ちに残る
#[test]
fn 背景の走り出しを止める注入でも先に取った番号は取り消しで抜ける() {
    let _env = serial();
    let scratch = Scratch::new("inject");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(config(&scratch, &rules(false)));
    let _link = open_editing(&manager, &main);
    lsp_fake_e2e::wait_loading_known(&manager);

    for (label, legacy) in [("fixed", false), ("legacy", true)] {
        let hold = scratch.0.join(format!("hold-{label}"));
        std::env::set_var("TAKO_1909_INJECT_HOLD", &hold);
        let ticket = if legacy {
            None
        } else {
            manager.reserve_completion()
        };
        let background = spawn(&manager, typed(&main, 16, ticket));
        // 背景が列へ入って合図を待っている
        wait_until("背景が列へ入る", || {
            inflight(&manager, "completion") == 1
        });
        manager.cancel_completion();
        std::fs::write(&hold, b"").unwrap();
        if !legacy {
            assert_eq!(background.join().unwrap(), Err(CompletionError::Superseded));
            assert_eq!(received(&scratch, "textDocument/completion"), 0);
            assert_eq!(inflight(&manager, "completion"), 0);
            continue;
        }
        // 旧い形: 取り消しを追い越してサーバへ問い合わせ、読み込みの待ちに残る
        lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
        assert_eq!(inflight(&manager, "completion"), 1);
        scratch.gate().finish();
        background
            .join()
            .unwrap()
            .expect("済んだ後に答えを受け取る");
        assert_eq!(inflight(&manager, "completion"), 0);
    }
    std::env::remove_var("TAKO_1909_INJECT_HOLD");
    manager.shutdown_all(Duration::from_secs(2));
}
