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
//!
//! ## 答えの順序（#1930）
//!
//! #1922 の残りと、その後に Windows の CI で出た間欠も「順序を実時間に任せていた」で、注入で
//! 確かめてからここへ寄せた:
//!
//! 4. **答えの遅れ（偽サーバの `delay_ms`）の間に次の操作が間に合う前提**（取り消し・打ち足し・
//!    重なった問い合わせ）。遅い機では答えが先に届く。答えは [`AnswerGate`]（規則の `hold_until`）で
//!    テストが「次の操作が済んだ」を確かめてから返させる
//! 5. **「送った」と「届いた」の取り違え**（`issue1684_lsp_menu.rs:262`）。manager が握手の後に
//!    didOpen を送った直後に偽サーバのログを数えていた。偽サーバのログ書き込みを 20ms 遅らせる
//!    注入（`--log-delay-ms`）で 10 回中 9 回、50ms で 10 回とも CI と同じ行で落ちる。ログで
//!    数を確かめる検査は [`stop_and_settle`] の後に数える
//! 6. **読み込み中に返った空を、manager が見る前に読み込みが済む**（`issue1869_lsp_followup.rs:197`）。
//!    これは製品側の判定が真因で、`EmptyAnswer`（`tako_control::lsp::goto`）が 1 回だけ問い直す
//!    ようにした。偽サーバの `--settle-on-empty before`（[`settle_before_empty_args`]）がこの順序を
//!    どの機でも起こす

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

/// 偽サーバの答え（補完・ホバー・整形）を待たせる扉（規則の `hold_until`。#1930）。
///
/// 作った時点で閉じている。閉じているあいだに届いた要求は [`Self::open`] まで答えない
/// （補完・ホバーは取り消されたら -32800 で答える）。開いた後に [`Self::close`] すれば、その後に
/// 届く要求をまた待たせる。答えの遅れを実時間で決めない（遅い機で次の操作より先に答えが届かない）
pub struct AnswerGate(PathBuf);

impl AnswerGate {
    /// `dir`（テストの使い捨ての置き場）の中に扉のファイルの置き場を決める
    pub fn new(dir: &Path) -> Self {
        Self::named(dir, "answer-open")
    }

    /// 扉を複数使うとき（`hold_until` の配列 = k 本目の要求が k 番目の扉を待つ。要求を 1 本ずつ返させる）
    pub fn named(dir: &Path, name: &str) -> Self {
        let gate = Self(dir.join(name));
        gate.close();
        gate
    }

    /// 規則の `hold_until` へ渡す値
    pub fn path(&self) -> String {
        self.0.display().to_string()
    }

    /// 待たせている答えを返させる（以後に届く要求もすぐ答える）
    pub fn open(&self) {
        std::fs::write(&self.0, b"").expect("答えの扉を開ける");
    }

    /// 以後に届く要求をまた待たせる
    pub fn close(&self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 偽サーバのログを数える前の区切り（#1930）: manager を止めて、偽サーバが `shutdown` を
/// 受け取ったのを確かめる。偽サーバは届いた順に 1 本で読み、ログへ書いてから答えるので、
/// `shutdown` がログに載った = **それより前に送った通知・要求はすべてログに載っている**
/// （manager が「送った」と偽サーバに「届いた」を取り違えない）
#[track_caller]
pub fn stop_and_settle(manager: &LspManager, log: &Path) {
    manager.shutdown_all(Duration::from_secs(5));
    wait_received(log, "shutdown", 1);
}

/// 偽サーバのログ書き込みを遅らせる注入（`--log-delay-ms`。#1930）。テストの状態待ちの周期
/// （20ms）より長い = 「送った」直後にログを数えれば**どの機でも必ず**落ちる順序を演じる
/// （[`stop_and_settle`] を通っていれば遅れても通る）
pub fn log_delay_args() -> Vec<String> {
    vec!["--log-delay-ms".to_string(), "50".to_string()]
}

/// 読み込み中に受けた要求の空の答えを、読み込みを終えてから送らせる（`--settle-on-empty before`。
/// #1930）。manager は空を受けた時点で必ず「済んだ」と知っている = 古い空を最終の答えにしない
/// 判定（`EmptyAnswer`）を決定的に通す
pub fn settle_before_empty_args() -> Vec<String> {
    vec!["--settle-on-empty".to_string(), "before".to_string()]
}

/// 起動の順序を入れ替える遅延の注入（`TAKO_LSP_FAKE_STATUS_DELAY_MS` と同じ）。
/// 読み込み中と知らせるのを `READY_POLL`（20ms）より遅らせる = 待たずに送れば必ず落ちる順序を
/// どの機でも演じる（[`wait_loading_known`] を通っていれば遅れても通る）
pub fn status_delay_args() -> Vec<String> {
    vec!["--status-delay-ms".to_string(), "50".to_string()]
}

/// `done` が真になるまで待つ。上限で落ちるときは `detail`（その時点の状態）を添える
/// （`#[track_caller]` = 落ちた行は待ちを呼んだテストの行になる。#1930）
#[track_caller]
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
#[track_caller]
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
#[track_caller]
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
#[track_caller]
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
