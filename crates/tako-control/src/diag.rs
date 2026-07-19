//! diag — 永続化まわりの診断ログ（Issue #30）+ メインスレッド・ストール診断（Issue #168）
//!
//! Dock 起動の .app は stderr がユーザーから見えないため、レイアウト永続化
//! （保存・復元・明示削除）の結果を `<data_dir>/persist.log` に残す。
//! 「再起動したらタブが全部消えた」ときに、保存されなかったのか・復元に
//! 失敗したのか・明示削除されたのかを後から特定できるようにする。
//!
//! - 書き込みは best-effort（ログ失敗で本体機能を止めない）
//! - 肥大化防止: 上限超過で `.old` へローテート（世代は 1 つ）
//! - ペイン内容・送信テキスト・トークンは書かない（規約）

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// ローテート閾値（これを超えたら `.old` へ退避して新しいファイルを始める）
const ROTATE_BYTES: u64 = 256 * 1024;

/// 診断ログのパス（`<data_dir>/persist.log`）
pub fn persist_log_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("persist.log"))
}

/// パニックのローカル記録のパス（`<data_dir>/panic.log`。#381）
pub fn panic_log_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("panic.log"))
}

/// パニックの痕跡をローカルへ書き残す（#381: silent death の事後調査用）。
/// .app 起動は stderr が捨てられ、テレメトリ（既定 OFF）以外に記録先が無く、
/// 「クラッシュレポートも stderr も無いプロセス消滅」の原因を追えなかった。
/// テレメトリの有効 / 無効に関係なく常時書く（内容はパニックメッセージと
/// バックトレースのみ = ペイン内容・トークンは含まれない）
pub fn panic_log(msg: &str) {
    append_log(panic_log_path(), msg);
}

/// パフォーマンス診断ログのパス（`<data_dir>/perf.log`。Issue #113）。
/// `TAKO_PERF_LOG` で上書き可（Issue #168: 隔離実測で本番ログと混ざらないように）
pub fn perf_log_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAKO_PERF_LOG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    tako_core::paths::data_dir().map(|d| d.join("perf.log"))
}

/// 1 行追記する（UTC タイムスタンプ付き）。失敗は握りつぶす（診断ログの失敗で
/// 本体を巻き込まない）。呼び出し頻度は起動・終了・エラー時のみを想定し、
/// 定期保存の成功のような高頻度イベントは書かないこと
pub fn persist_log(msg: &str) {
    append_log(persist_log_path(), msg);
}

/// UI ストール・dispatch 遅延など性能異常の記録（Issue #113: 多ペイン時の無応答の
/// 犯人特定用）。**しきい値超えのときだけ**呼ぶこと（正常時は何も書かない = 高頻度
/// 呼び出し禁止は persist_log と同じ方針）。セルフテスト中はユーザーのログを汚さない
pub fn perf_log(msg: &str) {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return;
    }
    append_log(perf_log_path(), msg);
}

fn append_log(path: Option<PathBuf>, msg: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // ローテート: 上限超過なら .old へ退避（rename 失敗は無視して追記継続）
    if std::fs::metadata(&path)
        .map(|m| m.len() > ROTATE_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        // 1 行 = 1 write(2) にする。writeln! はフォーマット断片ごとに write が分かれ、
        // 複数スレッド（MCP リクエスト毎スレッド化 #84 以降）の並行書き込みで
        // 行が混線する実例が出た（#212 の perf.log で観測）
        let line = format!("[{}] {msg}\n", format_utc(now));
        let _ = f.write_all(line.as_bytes());
    }
}

/// unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`。外部クレート（chrono 等）を増やさないための
/// 自前変換（civil_from_days アルゴリズム。うるう年対応・グレゴリオ暦）
pub(crate) fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3_600,
        (tod % 3_600) / 60,
        tod % 60
    )
}

// ===== メインスレッド・ストール診断（Issue #168） =====
//
// 「アプリ全体がちょくちょく止まる」= メインスレッド（GPUI イベントループ）の
// 間欠ブロックを、**止まった瞬間に何をしていたか**ごと記録するための仕組み。
//
// - `perf_span(tag)`: 重い区間の入口で呼ぶ（RAII）。drop 時に所要がしきい値を
//   超えていれば perf.log へ記録する。ネスト可（内側の区間が現在タグになる）
// - `spawn_stall_watchdog()`: 監視スレッドを起動（GUI プロセスで 1 回）。
//   区間が 2 秒を超えて継続している（ハング級）とき、drop を待たずに 1 回記録する
// - `TAKO_PERF_VERBOSE=1`: しきい値を 16ms（1 フレーム）へ下げ、さらに 10 秒ごとに
//   タグ別の所要分布（count / p50 / p95 / p99 / max）を出力する（before/after 実測用）

/// 記録対象とするメインスレッド専有時間のしきい値（通常運転）。
/// 60fps の 2 フレーム分 = 体感で引っかかりが分かり始める長さ
const SPAN_LOG_OVER_MS: u64 = 32;
/// perf.log への専有記録のレート上限（1 秒窓あたり）。ストール洪水時のログ肥大防止
const SPAN_LOG_RATE_PER_SEC: u32 = 20;
/// ハング級とみなす区間継続時間（watchdog が drop を待たず中間報告する）
const HANG_REPORT_AFTER: Duration = Duration::from_secs(2);

struct SpanState {
    tag: Cow<'static, str>,
    started: Instant,
    hang_reported: bool,
}

struct PerfWatch {
    /// メインスレッドで現在実行中の区間（ネストは内側優先）
    current: Option<SpanState>,
    /// verbose 統計のサンプル（タグ, 所要 ms）。watchdog が 10 秒ごとに回収する
    samples: Vec<(Cow<'static, str>, u64)>,
}

static PERF_WATCH: Mutex<PerfWatch> = Mutex::new(PerfWatch {
    current: None,
    samples: Vec::new(),
});

/// verbose 実測モード（`TAKO_PERF_VERBOSE=1`）。プロセス内で 1 回だけ判定
fn perf_verbose() -> bool {
    static VERBOSE: OnceLock<bool> = OnceLock::new();
    *VERBOSE.get_or_init(|| {
        matches!(
            std::env::var("TAKO_PERF_VERBOSE").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

fn span_threshold_ms() -> u64 {
    if perf_verbose() {
        16
    } else {
        SPAN_LOG_OVER_MS
    }
}

/// poisoned でも診断を止めない（診断のロックでアプリを巻き込まない）
fn watch_lock() -> std::sync::MutexGuard<'static, PerfWatch> {
    PERF_WATCH.lock().unwrap_or_else(|e| e.into_inner())
}

/// メインスレッドの重い区間を計測する RAII ガード。
/// drop 時にしきい値超えを perf.log へ記録し、区間中は watchdog のハング検知対象になる
pub struct PerfSpan {
    tag: Cow<'static, str>,
    t0: Instant,
    log_over_ms: u64,
    prev: Option<SpanState>,
}

/// 既定しきい値（32ms、verbose 時 16ms）の計測区間を開始する
pub fn perf_span(tag: impl Into<Cow<'static, str>>) -> PerfSpan {
    perf_span_over(tag, span_threshold_ms())
}

/// しきい値を明示して計測区間を開始する
pub fn perf_span_over(tag: impl Into<Cow<'static, str>>, log_over_ms: u64) -> PerfSpan {
    let tag = tag.into();
    let t0 = Instant::now();
    let prev = {
        let mut w = watch_lock();
        let prev = w.current.take();
        w.current = Some(SpanState {
            tag: tag.clone(),
            started: t0,
            hang_reported: false,
        });
        prev
    };
    PerfSpan {
        tag,
        t0,
        log_over_ms,
        prev,
    }
}

impl Drop for PerfSpan {
    fn drop(&mut self) {
        let took = self.t0.elapsed().as_millis() as u64;
        {
            let mut w = watch_lock();
            w.current = self.prev.take();
            if perf_verbose() {
                w.samples.push((self.tag.clone(), took));
                // 暴走ガード: watchdog 不在でも無限には溜めない
                if w.samples.len() > 100_000 {
                    w.samples.drain(..50_000);
                }
            }
        }
        if took >= self.log_over_ms && span_log_rate_ok() {
            perf_log(&format!(
                "メインスレッド専有: {} が {took}ms（Issue #168 診断）",
                self.tag
            ));
        }
    }
}

/// 専有記録のレート制限（1 秒窓で SPAN_LOG_RATE_PER_SEC 行まで）
fn span_log_rate_ok() -> bool {
    static WINDOW: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
    let mut win = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match win.as_mut() {
        Some((t0, n)) if now.duration_since(*t0) < Duration::from_secs(1) => {
            if *n >= SPAN_LOG_RATE_PER_SEC {
                false
            } else {
                *n += 1;
                true
            }
        }
        _ => {
            *win = Some((now, 1));
            true
        }
    }
}

/// メインスレッド・ウォッチドッグを起動する（多重呼び出しは無視）。
/// 50ms ごとに現在区間を確認し、ハング級（2 秒超え継続）を drop を待たず記録する。
/// verbose 時は 10 秒ごとにタグ別の所要分布も出力する
pub fn spawn_stall_watchdog() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("tako-stall-watchdog".into())
        .spawn(|| {
            let mut last_stats = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(50));
                // ハング級の中間報告（ロック中に I/O しない: メッセージだけ組んで出る）
                let hang_msg = {
                    let mut w = watch_lock();
                    match w.current.as_mut() {
                        Some(cur)
                            if !cur.hang_reported && cur.started.elapsed() >= HANG_REPORT_AFTER =>
                        {
                            cur.hang_reported = true;
                            Some(format!(
                                "メインスレッド長時間専有（継続中）: {} が {}ms 経過",
                                cur.tag,
                                cur.started.elapsed().as_millis()
                            ))
                        }
                        _ => None,
                    }
                };
                if let Some(msg) = hang_msg {
                    perf_log(&msg);
                }
                if perf_verbose() && last_stats.elapsed() >= Duration::from_secs(10) {
                    last_stats = Instant::now();
                    emit_span_stats();
                }
            }
        });
}

/// verbose 統計の出力: タグごとに count / p50 / p95 / p99 / max（ms）
fn emit_span_stats() {
    let samples = std::mem::take(&mut watch_lock().samples);
    if samples.is_empty() {
        return;
    }
    let mut by_tag: std::collections::HashMap<Cow<'static, str>, Vec<u64>> =
        std::collections::HashMap::new();
    for (tag, ms) in samples {
        by_tag.entry(tag).or_default().push(ms);
    }
    let mut tags: Vec<_> = by_tag.into_iter().collect();
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    for (tag, mut v) in tags {
        v.sort_unstable();
        let pct = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
        perf_log(&format!(
            "span 統計 [{tag}] count={} p50={}ms p95={}ms p99={}ms max={}ms",
            v.len(),
            pct(0.50),
            pct(0.95),
            pct(0.99),
            v[v.len() - 1],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utcタイムスタンプが正しく変換される() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        // 2026-07-02 00:00:00 UTC = 1_782_950_400
        assert_eq!(format_utc(1_782_950_400), "2026-07-02T00:00:00Z");
        // うるう日 2024-02-29 12:34:56 UTC = 1_709_210_096
        assert_eq!(format_utc(1_709_210_096), "2024-02-29T12:34:56Z");
        // 年末境界 2023-12-31 23:59:59 UTC = 1_704_067_199
        assert_eq!(format_utc(1_704_067_199), "2023-12-31T23:59:59Z");
    }

    #[test]
    fn perf_spanのネストで現在区間が内側優先になり復元される() {
        {
            let _outer = perf_span_over("outer", u64::MAX);
            assert_eq!(
                watch_lock()
                    .current
                    .as_ref()
                    .map(|s| s.tag.as_ref().to_string()),
                Some("outer".to_string())
            );
            {
                let _inner = perf_span_over("inner", u64::MAX);
                assert_eq!(
                    watch_lock()
                        .current
                        .as_ref()
                        .map(|s| s.tag.as_ref().to_string()),
                    Some("inner".to_string())
                );
            }
            // 内側 drop で外側が復元される
            assert_eq!(
                watch_lock()
                    .current
                    .as_ref()
                    .map(|s| s.tag.as_ref().to_string()),
                Some("outer".to_string())
            );
        }
        assert!(watch_lock().current.is_none());
    }

    #[test]
    fn 並行書き込みでも行が混線しない() {
        // #212: writeln! のフォーマット断片ごとの write(2) 分割で、複数スレッドの
        // 並行 append_log がタイムスタンプ途中に割り込む実例が出た。1 行 = 1 write の回帰テスト
        let dir = std::env::temp_dir().join(format!("tako-diag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("perf.log");
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let p = path.clone();
                std::thread::spawn(move || {
                    for i in 0..50 {
                        append_log(Some(p.clone()), &format!("thread{t} メッセージ {i}"));
                    }
                })
            })
            .collect();
        for th in threads {
            th.join().unwrap();
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 8 * 50);
        for line in lines {
            // 各行が「[YYYY-MM-DDTHH:MM:SSZ] threadN メッセージ M」の完全形であること
            assert!(
                line.starts_with('[')
                    && line.len() > 22
                    && &line[21..23] == "] "
                    && line[23..].starts_with("thread"),
                "混線した行: {line:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn span_log_rate制限は1秒窓で上限を超えない() {
        // 窓内で上限まで true、その後 false（他テストとの共有 static を考慮して
        // 「上限 + 1 回叩いたら少なくとも 1 回は false」だけを保証する）
        let mut allowed = 0;
        for _ in 0..(SPAN_LOG_RATE_PER_SEC + 1) {
            if span_log_rate_ok() {
                allowed += 1;
            }
        }
        assert!(allowed <= SPAN_LOG_RATE_PER_SEC);
    }
}
