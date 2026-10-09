//! 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こす e2e の共通部品（Issue #1922。テスト専用）
//!
//! ## なぜ要るか（真因。注入で再現して確かめた）
//!
//! 読み込み中・起動中の待ちを見る e2e が、Windows の CI でだけ間欠的に落ちていた。どれも
//! 「テストが前提にした順序」を実時間の速さに任せていたのが原因で、遅い機では順序が入れ替わる。
//!
//! 1. **読み込み中と知る前に要求を送る**（#1869 / #1893 の「読み込みの後半」）。偽サーバは
//!    `initialized` を受けた時点で `quiescent: false` を送るが、manager はその知らせを reader の
//!    スレッドで受ける。起動待ち（`wait_ready`）は 20ms おき（`READY_POLL`）に見るので、知らせの
//!    処理が握手から 1 周期ぶん遅れると、manager は「読み込み中」と知る前に要求を送る。偽サーバは
//!    その要求を読み込みが済むまで待たせて答えるが、送った時点で知らなかったので答えの
//!    `waited_for_loading` は `None` になる。偽サーバの知らせを遅らせる注入
//!    （`TAKO_LSP_FAKE_STATUS_DELAY_MS`）で 1ms / 5ms / 10ms は 10 回中 0 回・15ms は 4 回・
//!    20ms 以上は 10 回とも、CI と同じ行（`issue1869_lsp_followup.rs:214` /
//!    `issue1893_lsp_hover_followup.rs:221`）で落ちる
//! 2. **読み込みが要求より先に終わる**。読み込みの長さを実時間（`--loading-ms 1500` 等）で決めると、
//!    遅い機では要求が届く前に済む。読み込みを 1ms に縮める注入で、後半の 2 本に加えて前半の 2 本と
//!    #1893 の旧挙動も落ちる（CI で落ちたのは後半の 2 本だけ = CI の真因は 1. の方）
//! 3. **起動が上限を越える**（#1680 の「見つからない・未応答・未導入」）。上限 1 秒の要求が
//!    サーバの起動ごと 1 秒で測っていたので、Windows のランナーで起動が 1 秒を越えると
//!    `starting: true` で返る。偽サーバの起動を 1.2 秒遅らせる注入
//!    （`TAKO_LSP_FAKE_START_DELAY_MS`）で `issue1680_lsp_goto.rs:329` が落ち、2.2 秒では上限 2 秒の
//!    同型（#1683 の未応答・#1869 / #1893 の「上限で loading」）も落ちる
//!
//! ## ここが持つ約束
//!
//! - 読み込みの終わりは**テストが決める**（[`LoadingGate`]。偽サーバの `--loading-until`）。
//!   要求が届いたことを偽サーバのログで確かめてから [`LoadingGate::finish`] する
//! - 読み込み中のサーバへ送る要求は、manager が**読み込み中と知った**のを待ってから送る
//!   （[`wait_loading_known`]）
//! - 上限つきの要求で「起動の後の経路」を見るなら、**起動済みを待ってから**測る
//!   （[`wait_running`]）
//!
//! 番犬 `issue1922_lsp_loading_wait_watchdog` が、LSP の e2e がこの 3 つを通っているかを
//! file:line で見る。

// 取り込むテストごとに使う関数が違う
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::LspManager;

/// 状態待ちの上限（止まったときの保険。届けば待たずに進む）
const LIMIT: Duration = Duration::from_secs(60);

/// 偽サーバの読み込みを終わらせる合図（`--loading-until` に渡すファイル）
pub struct LoadingGate(PathBuf);

impl LoadingGate {
    /// `dir`（テストの使い捨ての置き場）の中に合図のファイルを置く
    pub fn new(dir: &Path) -> Self {
        Self(dir.join("loading-done"))
    }

    /// 偽サーバへ渡す引数。読み込みの最短は 0 = 終わりは合図だけで決まる（実時間で決めない）
    pub fn args(&self) -> Vec<String> {
        vec![
            "--loading-ms".to_string(),
            "0".to_string(),
            "--loading-until".to_string(),
            self.0.display().to_string(),
        ]
    }

    /// 読み込みを終わらせる（偽サーバは合図を見てから `quiescent: true` を送る）
    pub fn finish(&self) {
        std::fs::write(&self.0, b"").expect("読み込みの合図を書ける");
    }
}

/// 起動の順序を入れ替える遅延の注入（`TAKO_LSP_FAKE_STATUS_DELAY_MS` と同じ）。
/// 読み込み中と知らせるのを `READY_POLL`（20ms）より遅らせる = 待たずに送れば必ず落ちる順序を
/// どの機でも演じる（[`wait_loading_known`] を通っていれば遅れても通る）
pub fn status_delay_args() -> Vec<String> {
    vec!["--status-delay-ms".to_string(), "50".to_string()]
}

/// `done` が真になるまで待つ。上限で落ちるときは `detail`（その時点の状態）を添える
fn wait_until(what: &str, mut done: impl FnMut() -> bool, detail: impl Fn() -> String) {
    let deadline = Instant::now() + LIMIT;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {LIMIT:?} 以内に起きなかった: {}",
            detail()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn server(manager: &LspManager) -> Value {
    manager.status(None)["servers"][0].clone()
}

/// サーバが起動して握手を終えた（`running`）まで待つ。文書を編集モードで開いてから呼ぶ
/// （握手が済んだ時点で同じロックの中で didOpen まで送る = この後の要求は起動を待たない）
pub fn wait_running(manager: &LspManager) {
    wait_until(
        "言語サーバの起動",
        || server(manager)["state"] == json!("running"),
        || server(manager).to_string(),
    );
}

/// manager がサーバの「読み込み中」（`quiescent: false`）を受け取り済みになるまで待つ。
/// 文書を編集モードで開いてから呼ぶ。
///
/// 偽サーバは `initialized` を受けて `quiescent: false` を送り、その**後に**届く didOpen へ
/// 診断を 1 件返す（知らせを送るまで次のメッセージを読まない）。manager の reader は届いた
/// 順に 1 本のスレッドで処理するので、**診断が数に載った = 知らせも処理済み**。
/// `loading` も真であること（読み込みはまだ合図していない）を合わせて見る
pub fn wait_loading_known(manager: &LspManager) {
    wait_until(
        "読み込み中の知らせを受け取る",
        || {
            let server = server(manager);
            server["state"] == json!("running")
                && server["diagnostics"].as_u64().unwrap_or(0) >= 1
                && server["loading"] == json!(true)
        },
        || server(manager).to_string(),
    );
}

/// 偽サーバのログ（`--log`）に `method` の要求が `count` 件以上届くまで待つ
pub fn wait_received(log: &Path, method: &str, count: usize) {
    wait_until(
        &format!("{method} の {count} 件目"),
        || received(log, method) >= count,
        || format!("届いたのは {} 件", received(log, method)),
    );
}

/// 偽サーバのログ（`--log`）に届いた `method` の数
pub fn received(log: &Path, method: &str) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m.get("method").and_then(Value::as_str) == Some(method))
        .count()
}
