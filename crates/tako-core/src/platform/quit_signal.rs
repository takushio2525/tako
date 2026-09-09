//! `SIGTERM` を正規の quit（`on_app_quit` を通る終了）へ読み替える（Issue #777。元は #770）。
//!
//! GUI アプリの正規の終了経路（Cmd+Q / Dock 終了 / OS 終了）は `on_app_quit` を通り、
//! そこで layout 保存・ペインログの flush・蓋閉じ防止の解除・discovery の後片付けが走る。
//! ところが `SIGTERM` は既定でプロセスを**即死**させるため、この経路をまるごと飛ばしていた:
//!
//! ```text
//! $ kill -TERM <tako-app pid>
//! --- persist.log ---
//!    ← on_app_quit の行が無い = 終了処理を通っていない（#770 の実測）
//! ```
//!
//! 困るのは「最後の構成が失われる」こと。layout の保存は 2 秒ポーリング + 操作ごとなので、
//! `kill -TERM` / スクリプトからの停止 / 一部の OS シャットダウン経路では**直前 2 秒ぶんの
//! 分割・タブ**が復元されない。蓋閉じ防止（`disablesleep`）の解除も走らない。
//! tmux セッション自体は生き続けるので喪失ゼロ保証（#30）は破れないが、「構成だけ古い」になる。
//!
//! そこで `SIGTERM` を「quit 要求フラグを立てる」へ読み替える。フラグを見た UI 側が通常の
//! `cx.quit()` を呼ぶので、**通るのは Cmd+Q と同じ終了経路そのもの**（`on_app_quit` が
//! セッションを kill しないことは #770 の番犬が拘束している）。
//!
//! ## ウォッチドッグ（この読み替えの必須の対）
//!
//! シグナルを握るアプリは、**ハングすると `pkill` が効かなくなる**。読み替えを入れるなら
//! 「猶予を過ぎたら必ず死ぬ」保証を同時に持たなければならない。専用スレッドを 1 本立て、
//! `SIGTERM` を受けてから [`grace`]（既定 5 秒）以内に終われなければ
//! [`FORCED_EXIT_CODE`] で `std::process::exit` する。
//!
//! - 猶予は**シグナル到着から**測る。メインスレッドが固まっていて quit を撃てないとき
//!   （= まさに `pkill` したくなる状況）も同じ上限で終わる
//! - スレッドはシグナルハンドラから起こせない（スレッド生成は async-signal-safe ではない）ので、
//!   起動時に立てて 100ms 間隔でフラグを見る。既存の stall watchdog（50ms 間隔）と同型で、
//!   アイドル時のコストは無視できる
//!
//! ## Windows
//!
//! Windows に `SIGTERM` は無い。ウィンドウを閉じる経路（`taskkill` の既定・× ボタン）は
//! `WM_CLOSE` として GPUI の通常終了へ入り、`taskkill /F` は**プロセス側にフックできない**
//! 強制終了なので、どちらもこの読み替えの対象外になる。`cfg` で明示的に空実装にしてある。
//!
//! ## A/B
//!
//! `TAKO_777_LEGACY=1` で**仕掛けを一切入れない** = 修正前の本番挙動（`SIGTERM` = 即死）に戻る。
//! #770 の隔離検証用インストールも止まるので、隔離インスタンスで新旧を同一バイナリで比べられる。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// quit 要求（UI の定期チェックが消費する）
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
/// `SIGTERM` を一度でも受けたか。**消費しない**（ウォッチドッグの起点）
static SIGTERM_SEEN: AtomicBool = AtomicBool::new(false);
/// quit を UI へ渡したか。連打された `SIGTERM` で終了処理を二重に走らせないラッチ
static QUIT_DISPATCHED: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// `SIGTERM` から強制終了までの猶予（既定）。
///
/// 正常な終了処理（`on_app_quit`）に対しては桁で余る長さにし、かつ `kill` した人が
/// 諦めるまでの体感を損なわない長さにする、の 2 条件から 5 秒に置いた。
/// 実測（2026-09-09 / 隔離インスタンス・5 回）は `SIGTERM` から消えるまで中央値 287ms・
/// 最大 452ms（下の 500ms ポーリングの待ちを含む）なので 10 倍以上の余裕がある
pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);
/// 猶予を差し替える env（検証専用。ミリ秒）
pub const GRACE_ENV: &str = "TAKO_777_GRACE_MS";
/// ウォッチドッグがフラグを見る間隔。猶予の測り始めはこのぶんだけ遅れうる
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
/// ウォッチドッグによる強制終了の終了コード（128 + SIGTERM(15)。シェルの慣習に合わせる）
pub const FORCED_EXIT_CODE: i32 = 143;

/// `SIGTERM` を quit 要求へ読み替える仕掛けを入れる（プロセスに 1 度だけ）。
///
/// `on_forced_exit` は**猶予を過ぎて強制終了する直前**に呼ばれる診断ログの口。
/// 通常のスレッドから呼ばれるので何をしてもよい（シグナルハンドラからは呼ばない）。
/// `TAKO_777_LEGACY=1` と Windows では何もしない
pub fn install(on_forced_exit: fn(&str)) {
    // Windows に SIGTERM は無い（モジュール冒頭の「Windows」節）ので、起点の来ない
    // 見張りスレッドごと立てない
    if legacy() || !cfg!(unix) || INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    install_handler();
    // 猶予はここ（起動時のメインスレッド）で読む。ウォッチドッグのスレッドから読むと
    // 起動処理の `set_var`（隔離モードの一括設定）と競合しうる
    spawn_watchdog(grace(), on_forced_exit);
}

/// quit が要求されているか（UI 側の定期チェックから引く）。
///
/// **プロセスを通じて true を返すのは 1 度だけ**。`SIGTERM` を連打されても終了処理は
/// 一度しか走らない（2 発目以降は握りつぶし、間に合わなければウォッチドッグが終わらせる）
pub fn take_quit_request() -> bool {
    if !QUIT_REQUESTED.swap(false, Ordering::SeqCst) {
        return false;
    }
    !QUIT_DISPATCHED.swap(true, Ordering::SeqCst)
}

/// 修正前の挙動（`SIGTERM` = 即死）へ戻す A/B スイッチ
pub fn legacy() -> bool {
    std::env::var("TAKO_777_LEGACY").as_deref() == Ok("1")
}

/// `SIGTERM` から強制終了までの猶予。`TAKO_777_GRACE_MS` で差し替えられる（検証専用）。
/// 解釈できない値・0 は既定へ落とす（猶予 0 = 読み替えが無意味になるため）
pub fn grace() -> Duration {
    std::env::var(GRACE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_GRACE)
}

/// `SIGTERM` を受けて quit へ読み替えたときの診断行（書き手とテストで 1 実装を共有する）
pub fn received_log_line() -> String {
    "SIGTERM: 正規の quit へ読み替えた（#777。on_app_quit を通って layout を保存してから終了する）"
        .to_string()
}

/// 猶予を過ぎて強制終了するときの診断行（同上）
pub fn forced_exit_log_line(grace: Duration) -> String {
    format!(
        "SIGTERM: 猶予 {}ms を過ぎても終了処理が終わらないので強制終了する（#777。\
         ハング中でも kill が効くためのウォッチドッグ。終了コード {FORCED_EXIT_CODE}）",
        grace.as_millis()
    )
}

/// ウォッチドッグ本体。`SIGTERM` を待ち、猶予を過ぎてもプロセスが生きていたら終わらせる。
/// **正常に終了できた場合はプロセスごと消えるので、このスレッドは何もしない**
fn spawn_watchdog(grace: Duration, on_forced_exit: fn(&str)) {
    let _ = std::thread::Builder::new()
        .name("tako-quit-watchdog".into())
        .spawn(move || {
            while !SIGTERM_SEEN.load(Ordering::SeqCst) {
                std::thread::sleep(WATCH_INTERVAL);
            }
            std::thread::sleep(grace);
            on_forced_exit(&forced_exit_log_line(grace));
            std::process::exit(FORCED_EXIT_CODE);
        });
}

#[cfg(unix)]
fn install_handler() {
    // シグナルハンドラでやってよいのは async-signal-safe な操作だけ。
    // ここではアトミックなフラグ設定しかしない（ログも割り当ても行わない）
    extern "C" fn on_sigterm(_sig: libc::c_int) {
        SIGTERM_SEEN.store(true, Ordering::SeqCst);
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn install_handler() {
    // Windows に SIGTERM は無い（モジュール冒頭の「Windows」節）。
    // ウォッチドッグだけが立っても起点が来ないので、ここは意図的に空
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// グローバルなフラグを触るテストは直列化する（同一プロセスで並列実行されるため）
    static SERIAL: Mutex<()> = Mutex::new(());

    fn reset() {
        QUIT_REQUESTED.store(false, Ordering::SeqCst);
        SIGTERM_SEEN.store(false, Ordering::SeqCst);
        QUIT_DISPATCHED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn 初期状態ではquit要求が立っていない() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        assert!(!take_quit_request());
    }

    #[test]
    fn quit要求は一度だけ消費される() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
        assert!(take_quit_request());
        assert!(
            !take_quit_request(),
            "2 度目は false = quit を二重に撃たない"
        );
    }

    #[test]
    fn sigterm連打でも終了処理は一度しか走らない() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        // 1 発目 → UI が消費 → 2 発目（同じハンドラがもう一度フラグを立てる）
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
        assert!(take_quit_request(), "1 発目は quit を撃つ");
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
        assert!(
            !take_quit_request(),
            "2 発目は握りつぶす（終了処理の二重実行を防ぐ。間に合わなければウォッチドッグが終わらせる）"
        );
    }

    #[test]
    fn 猶予の既定と差し替え() {
        // env は並列テストと干渉するので、解釈だけを純粋に確かめる
        assert_eq!(DEFAULT_GRACE, Duration::from_secs(5));
        assert_eq!(FORCED_EXIT_CODE, 143, "128 + SIGTERM(15)");
        assert_eq!(GRACE_ENV, "TAKO_777_GRACE_MS");
    }

    #[test]
    fn 診断行に番号と根拠が入る() {
        assert!(received_log_line().contains("#777"));
        assert!(received_log_line().contains("on_app_quit"));
        let forced = forced_exit_log_line(Duration::from_millis(1500));
        assert!(forced.contains("1500ms"), "猶予の実値を出す: {forced}");
        assert!(forced.contains("143"), "終了コードを出す: {forced}");
    }
}
