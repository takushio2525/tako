//! close の worker レジストリへの記録を GUI のメインスレッドで待たない（#2657）。
//!
//! ## なぜ分けるのか
//!
//! 明示 close の記録（[`crate::orchestrator::registry::mark_closed_by_origin`]）は、workers.yaml
//! （本番で約 225 KB・200 件）を読んで YAML を解釈し、上限の無い flock（`workers.yaml.lock`）を取り、
//! 書き出して世代バックアップを回し、`sync_all`（macOS は F_FULLFSYNC）まで行う。これを UI スレッドで
//! 待つと、器の worker の close が普段 11〜37ms・裾で 47〜263ms GUI を止めていた（#2618 の段 1 の実測。
//! 本番の perf.log の `dispatch:Close` 16 件はすべて master による器の worker の close）。
//!
//! ## 書く場所
//!
//! - CLI / MCP の `Close` / `BackgroundKill`（応答を待つ相手がいる）: dispatch の offload の段
//!   （`OffloadJob::CloseRecord`）が background で [`CloseMark::write`] を呼び、**書き終わってから応答する**
//!   = close の応答の後に `tako orchestrator workers` を読めば closed、の順序は修正前と同じ
//! - GUI の × / ⌘W / タブ close / たまり場カード（応答を待つ相手がいない）: [`CloseMark::spawn`] で
//!   背景のスレッドへ投げる。終了の直前は [`flush`] で書き終わりを待つ
//! - 同期の dispatch（`TAKO_OFFLOAD=0`・直の呼び手・テスト）と A/B の旧経路（`TAKO_2657_LEGACY=1`）:
//!   呼び手のスレッドで [`CloseMark::write`] を呼ぶ（修正前と同じ）

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use tako_core::pane_log::CloseOrigin;

/// A/B: #2657 前へ戻す（close の記録を UI スレッドで同期に書く）
pub const LEGACY_ENV: &str = "TAKO_2657_LEGACY";

/// close の記録を UI スレッドで同期に書く修正前の経路か（#2657 の A/B。**門はここ 1 か所**）。
/// 要求のたびに読む（テストが `run_alone!` の子で切り替えられるよう、値を覚えない）
pub fn legacy() -> bool {
    matches!(
        std::env::var(LEGACY_ENV).ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 1 件の close の記録（どのペインを・どの発生源で閉じたか）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseMark {
    pane: u64,
    origin: CloseOrigin,
    caller_role: Option<String>,
}

impl CloseMark {
    pub fn new(pane: u64, origin: CloseOrigin, caller_role: Option<&str>) -> Self {
        Self {
            pane,
            origin,
            caller_role: caller_role.map(str::to_string),
        }
    }

    /// 記録するペイン
    pub fn pane(&self) -> u64 {
        self.pane
    }

    /// 呼び手のスレッドで書く。**UI スレッドで呼ばない**（同期の dispatch と A/B の旧経路を除く）。
    /// 失敗は close を止めない（修正前と同じく stderr へ 1 行）
    pub fn write(&self) {
        inject_record_delay();
        if let Err(e) = crate::orchestrator::registry::mark_closed_by_origin(
            self.pane,
            self.origin,
            self.caller_role.as_deref(),
        ) {
            eprintln!("warning: worker レジストリの close 記録に失敗: {e}");
        }
    }

    /// 応答を待つ相手がいない経路（GUI の close）: 背景のスレッドで書く。
    /// スレッドを立てられなければその場で書く（記録を落とさない）
    pub fn spawn(self) {
        let ticket = Pending::enter();
        let spawned = std::thread::Builder::new()
            .name("tako-close-record".into())
            .spawn(move || {
                let _ticket = ticket;
                self.write();
            });
        if let Err(e) = spawned {
            eprintln!("warning: close の記録のスレッドを立てられない（その場で書く）: {e}");
        }
    }
}

/// 背景へ投げた記録の数（[`flush`] が待つ）
static PENDING: Mutex<usize> = Mutex::new(0);
static DRAINED: Condvar = Condvar::new();

/// 背景へ投げた記録 1 件ぶんの札。書き終わり（スレッドの終わり・panic を含む）で返す
struct Pending;

impl Pending {
    fn enter() -> Self {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Pending
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        let mut n = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        if *n == 0 {
            DRAINED.notify_all();
        }
    }
}

/// 背景へ投げた記録が書き終わるまで待つ（GUI の終了の直前。最後に閉じた worker を active で残さない）。
/// `limit` で打ち切る（ロックを握ったまま返らない別プロセスで終了を止めない）。戻り値は待った長さ
pub fn flush(limit: Duration) -> Duration {
    let t0 = Instant::now();
    let mut n = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    while *n > 0 {
        let Some(left) = limit.checked_sub(t0.elapsed()) else {
            break;
        };
        n = DRAINED
            .wait_timeout(n, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    t0.elapsed()
}

/// 背景へ投げてまだ書き終わっていない記録の数
pub fn pending() -> usize {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// **#2657 の検出力の注入**（`TAKO_2657_INJECT_RECORD_MS=<ms>`）: close の記録 1 件をこの ms だけ遅らせる。
/// 負荷は作らず、遅い記録（書き手のロック待ち・遅い fsync）の区間だけを伸ばす。実経路テスト
/// `scripts/test-close-record-offload-2657.sh` が「その間に close の応答以外の IPC が待たされない」を
/// 測るのに使う。製品の既定は 0（何もしない）
fn inject_record_delay() {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ms = *MS.get_or_init(|| {
        std::env::var("TAKO_2657_INJECT_RECORD_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    });
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flushは背景の記録が書き終わるまで待つ() {
        // 札を 1 枚持ったまま別スレッドで 80ms 後に返す。flush はそれまで返らない
        let ticket = Pending::enter();
        let t0 = Instant::now();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            drop(ticket);
        });
        let waited = flush(Duration::from_secs(5));
        handle.join().unwrap();
        assert!(
            waited >= Duration::from_millis(60) && t0.elapsed() < Duration::from_secs(5),
            "flush が書き終わりを待っていない: {waited:?}"
        );
    }

    #[test]
    fn flushは上限で打ち切る() {
        let ticket = Pending::enter();
        let waited = flush(Duration::from_millis(50));
        drop(ticket);
        assert!(
            waited >= Duration::from_millis(50) && waited < Duration::from_secs(2),
            "上限で打ち切っていない: {waited:?}"
        );
    }

    #[test]
    fn 札はpanicでも返る() {
        let handle = std::thread::spawn(|| {
            let _ticket = Pending::enter();
            panic!("記録の途中の panic");
        });
        assert!(handle.join().is_err());
        // 札が返っていなければ上限（2 秒）まで待つ。他のテストの札（最長 80ms）は数えてよい
        assert!(
            flush(Duration::from_secs(2)) < Duration::from_secs(1),
            "panic したスレッドの札が返っていない"
        );
    }
}
