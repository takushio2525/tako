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

pub use crate::platform::sysload::MachineLoad;

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

/// ペインの文字コード固定の異常（諦めた / 入力の保留を上限で解いた）を残す（#2092）。
/// どちらも Windows の異常系でしか起きない（正常な固定は書かない = 高頻度にならない）。
/// 1 行の正本は `PinLogEvent::log_line`（ペイン ID・対象・pid・経過・理由だけ。送った文字は載せない）
pub fn encoding_pin_log(pane: u64, event: &tako_core::encoding_pin::PinLogEvent) {
    persist_log(&event.log_line(pane));
}

/// 送達経路の分岐を記録する（`TAKO_FLOW_DIAG=1` のときだけ。#640）。
///
/// 起動コマンドの送達は「シェル準備待ち → エコー確認 → 分離 Enter → 実行確認」を
/// 段階的に進む。どの段階で足踏みしたかを残さないと「worker が起動しない」の切り分けが
/// できない（器が入力を落としているのか、そもそも書けていないのか）。
///
/// #877 でエージェント走査（`claude agents --json`）の失敗理由もここへ出す。
/// 「CLI が見つからない」「子プロセスを起こせない」「claude が異常終了（認証切れ等）」は
/// どれも呼び出し側では同じ `None` になるので、切り分けにはこのログしか手が無い。
///
/// **送信テキスト・画面内容は出さない**（AGENTS.md の絶対ルール）。出すのは段階と回数、
/// 走査なら分類・終了コード・バイト数だけ（claude の出力そのものは載せない）
pub fn flow_log(msg: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| {
        matches!(
            std::env::var("TAKO_FLOW_DIAG").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    });
    if on {
        perf_log(msg);
    }
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

/// いまのマシンの混み具合。取得できなければ `None`（#796 / #1073）。
///
/// 実装は境界 [`crate::platform::sysload`]（unix = load average / Windows = 短い窓の
/// CPU 使用率）。**Windows では約 120ms ブロックする**ので、診断 1 行を作るとき以外から
/// 呼んではいけない
pub fn machine_load() -> Option<MachineLoad> {
    crate::platform::sysload::sample()
}

/// 混み具合を診断 1 行用の表記へ（unix は `load=6.17/5.79/5.47`、
/// Windows は `load=cpu42%/12cpu`）。取得できなければ `load=unknown`。
///
/// unix の表記は**1 文字も変えていない**（過去のログ・記録と突き合わせられるように）。
/// 純粋関数なので単体テストで両プラットフォームぶんを固定できる
pub fn format_machine_load(load: Option<MachineLoad>) -> String {
    match load {
        Some(MachineLoad::Average([one, five, fifteen])) => {
            format!("load={one:.2}/{five:.2}/{fifteen:.2}")
        }
        Some(MachineLoad::CpuBusy { percent, cpus }) => {
            format!("load=cpu{percent:.0}%/{cpus}cpu")
        }
        None => "load=unknown".to_string(),
    }
}

fn append_log(path: Option<PathBuf>, msg: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    // #2686: 終了の後片付けが消した検証起動の置き場へは書かない（行は捨てる）。下の create_dir_all が
    // 置き場を作り直すので、片付けの後の 1 行で data dir が生き返る。本番の GUI では常に偽
    if tako_core::scoped_dirs::is_retired(&path) {
        return;
    }
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

/// #1968 の A/B 用の逃げ道（`TAKO_1968_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// worker の報告を UI スレッドで同期実行する・状態照会の子プロセスの有無を UI スレッド部で
/// 数える・worker レジストリを毎回読み直す・UI ストールの分類に終わった区間を使わない。
/// 前後を同じ構成・同じ負荷で並べて測るためのもの（製品の既定は常に新しい形）
pub fn issue1968_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1968_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2665 の A/B 用の逃げ道（`TAKO_2665_LEGACY=1`）。同一バイナリのまま修正前の UI ストールの分類へ戻す:
/// 窓を「1 秒 + 遅延」（タイマーを掛けた時点まで遡る）で取り、区間の全長を数え、半分に届かなければ
/// 内訳を書かずに「計測区間の外」とする（文も修正前の [`classify_stall`] のまま）
pub fn issue2665_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2665_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2666 の A/B 用の逃げ道（`TAKO_2666_LEGACY=1`）。同一バイナリのまま修正前（#2665 の分類）へ戻す:
/// 画面構成の変更の通知を登録しない・届いた通知（検証の合成を含む）を記録しない・UI ストールの分類に
/// 画面構成の変更を使わない（記録が無いので perf.log の再構成の行も出ない）
pub fn issue2666_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2666_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 直近に終わったメインスレッドの**トップレベル**計測区間（UI ストールの分類用。#1968）。
///
/// UI ストールは 1 秒タイマーの再開遅延で測るが、その監視ループ自身が UI スレッドで走るので、
/// 記録する時点では塞いでいた区間は**必ず終わっている**（`current` は常に空）。
/// 旧実装はそのせいで、tako 自身の専有まで「再開経路（タイマー / キュー）側の遅延」と
/// 書いていた（本番の perf.log で 38 分に 331 件・計 750 秒がこの分類）。
/// 終わった区間を短い履歴に残し、遅延の窓に重なったものを分類に使う
struct EndedSpan {
    tag: Cow<'static, str>,
    ended: Instant,
    took_ms: u64,
}

/// 履歴へ残す最短の所要（これ未満は 1 秒の遅延の説明にならないので数えない）
const RECENT_SPAN_MIN_MS: u64 = 5;
/// 履歴の上限（古いものから捨てる。1 秒の窓に収まる数より十分大きい）
const RECENT_SPAN_CAP: usize = 256;

static RECENT_SPANS: Mutex<std::collections::VecDeque<EndedSpan>> =
    Mutex::new(std::collections::VecDeque::new());

/// 呼び手が [`RECENT_SPAN_MIN_MS`] 以上のときだけ呼ぶ（短い区間で `Cow` を複製しない）
fn record_ended_span(tag: Cow<'static, str>, took_ms: u64) {
    let mut q = RECENT_SPANS.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() >= RECENT_SPAN_CAP {
        q.pop_front();
    }
    q.push_back(EndedSpan {
        tag,
        ended: Instant::now(),
        took_ms,
    });
}

/// 直前の `window` に終わったトップレベル区間の要約（最長の区間・合計・件数）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSpans {
    pub longest_tag: String,
    pub longest_ms: u64,
    pub total_ms: u64,
    pub count: usize,
}

/// 直前の `window` に終わったメインスレッドのトップレベル区間を要約する（無ければ `None`）
pub fn recent_spans_within(window: Duration) -> Option<RecentSpans> {
    let q = RECENT_SPANS.lock().unwrap_or_else(|e| e.into_inner());
    let since = Instant::now().checked_sub(window)?;
    let mut out: Option<RecentSpans> = None;
    for s in q.iter().filter(|s| s.ended >= since) {
        let r = out.get_or_insert_with(|| RecentSpans {
            longest_tag: s.tag.to_string(),
            longest_ms: 0,
            total_ms: 0,
            count: 0,
        });
        r.total_ms += s.took_ms;
        r.count += 1;
        if s.took_ms > r.longest_ms {
            r.longest_ms = s.took_ms;
            r.longest_tag = s.tag.to_string();
        }
    }
    out
}

/// 遅延の間（タイマーの期限 → 記録）に重なったトップレベル区間の要約（#2665）。
///
/// 数えるのは**期限より後ろへはみ出した分**だけ。期限より前に終わった区間はタイマーの再開を
/// 遅らせていない（その間メインスレッドは空いていた）ので数えない。旧実装は窓を
/// 「1 秒 + 遅延」= タイマーを掛けた時点まで遡り、期限より前に終わった区間まで丸ごと数えて
/// 「tako の専有」と名指していた（隔離 GUI の注入で 4/4）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelaySpans {
    /// 遅延の間に重なった分が最も長い区間
    pub longest_tag: String,
    /// その区間の全長（ms。区間自身の「メインスレッド専有」の行と突き合わせられる）
    pub longest_took_ms: u64,
    /// その区間のうち遅延の間に重なった分（ms）
    pub longest_overlap_ms: u64,
    /// 遅延の間に重なった分の合計（ms。トップレベルの区間どうしは重ならないので遅延を超えない）
    pub overlap_ms: u64,
    pub count: usize,
}

impl DelaySpans {
    /// 終わった区間（タグ・終わった時刻・全長 ms）のうち `since` より後ろに重なった分を要約する
    /// （無ければ `None`）。時刻は呼び手が渡す純粋関数
    pub fn summarize<'a>(
        spans: impl IntoIterator<Item = (&'a str, Instant, u64)>,
        since: Instant,
    ) -> Option<Self> {
        let mut out: Option<Self> = None;
        for (tag, ended, took_ms) in spans {
            if ended <= since {
                continue;
            }
            let started = ended
                .checked_sub(Duration::from_millis(took_ms))
                .unwrap_or(since);
            let overlap_ms = ended.duration_since(started.max(since)).as_millis() as u64;
            if overlap_ms == 0 {
                continue;
            }
            let r = out.get_or_insert_with(|| Self {
                longest_tag: tag.to_string(),
                longest_took_ms: took_ms,
                longest_overlap_ms: 0,
                overlap_ms: 0,
                count: 0,
            });
            r.overlap_ms += overlap_ms;
            r.count += 1;
            if overlap_ms > r.longest_overlap_ms {
                r.longest_overlap_ms = overlap_ms;
                r.longest_took_ms = took_ms;
                r.longest_tag = tag.to_string();
            }
        }
        out
    }
}

/// `since`（タイマーの期限）から今までに重なったメインスレッドのトップレベル区間（#2665）
pub fn spans_overlapping_since(since: Instant) -> Option<DelaySpans> {
    let q = RECENT_SPANS.lock().unwrap_or_else(|e| e.into_inner());
    DelaySpans::summarize(
        q.iter().map(|s| (s.tag.as_ref(), s.ended, s.took_ms)),
        since,
    )
}

// ===== 画面構成の変更（macOS の画面再構成。#2666） =====
//
// 蓋の開閉・画面スリープで内蔵ディスプレイが画面構成から出入りする瞬間に、UI が 0.5〜1.8 秒止まる。
// その塞がりは tako の計測区間の外（AppKit / GPUI の画面の当て直し・Metal の提示）なので、
// #2665 の分類では「計測区間の外か再開経路側の遅延」としか書けなかった（本番 29 件中 18 件）。
// OS の通知（`CGDisplayRegisterReconfigurationCallback` の begin / end。登録は tako-app の
// `platform::display_reconfig`）の時刻だけをここへ記録し、遅延の間に重なったら分類が名指す。
//
// 本番の統合ログ（11/01 15:03・tako-app）の実測: 再構成の通知（kCGSDisplayWillReconfigure →
// DidReconfigure2 → ReconfigureHappened）はどれもプロセスで最も若い TID（= メインスレッド）で処理され、
// DidReconfigure2（end のコールバックの時点）の約 130ms 後から 1.4 秒、そのスレッドの記録が途切れていた。
// 塞がりは end の**後**に来るので、窓は end から [`RECONFIG_AFTER_GRACE`] まで延ばす。

/// 再構成の通知の段（begin = 変更の前・end = 変更の後。画面ごとに 1 回ずつ来る）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconfigPhase {
    Begin,
    End,
}

/// 再構成の通知 1 件（コールバックが受けた値そのまま）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconfigEvent {
    pub phase: ReconfigPhase,
    /// 通知の対象の画面（macOS の `CGDirectDisplayID`）
    pub display: u32,
    /// 変更の内容（macOS の `CGDisplayChangeSummaryFlags`）
    pub flags: u32,
    /// コールバックがメインスレッドで呼ばれたか（本番で確かめるため perf.log の行に書く）
    pub main_thread: bool,
    /// 検証の合成（[`Issue2666Inject`]）か。perf.log の行と分類の文に「検証の合成」と書く
    pub synthetic: bool,
}

/// end の後、その後始末（AppKit / GPUI の画面の当て直し・全面の描き直し）として窓に含める長さ。
/// 本番の実測では end の約 130ms 後から 1.4 秒塞がっていた（期限は塞がりの頭から最大 1 秒後ろへ来る）
pub const RECONFIG_AFTER_GRACE: Duration = Duration::from_secs(2);
/// begin だけで end が来ない再構成を開いたままと見なす上限（end を取りこぼしても、以後のストールを
/// いつまでも画面構成の変更と名指さない）
pub const RECONFIG_OPEN_MAX: Duration = Duration::from_secs(10);
/// 最後の end からこれだけ次の通知が無ければ、その再構成は終わった（perf.log へ 1 行書く）。
/// 画面ごとの end は同じ瞬間に続けて来る
const RECONFIG_SETTLE: Duration = Duration::from_millis(300);
/// 履歴の上限（古いものから捨てる。窓は数秒なので数件あれば足りる）
const RECONFIG_CAP: usize = 16;
/// 1 回の再構成で覚える画面の数の上限
const RECONFIG_DISPLAYS_CAP: usize = 8;

/// 1 回の再構成（最初の begin から最後の end まで）
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconfigWindow {
    begun: Instant,
    ended: Option<Instant>,
    begins: u32,
    ends: u32,
    /// 通知の来た画面（重複なし・先頭 [`RECONFIG_DISPLAYS_CAP`] 枚まで）
    displays: Vec<u32>,
    /// end の通知の変更の内容の和
    end_flags: u32,
    /// メインスレッド以外で来た通知の数
    off_main: u32,
    synthetic: bool,
    /// perf.log へ書いたか
    reported: bool,
}

static RECONFIG: Mutex<std::collections::VecDeque<ReconfigWindow>> =
    Mutex::new(std::collections::VecDeque::new());

/// 通知 1 件を履歴へ畳む（純粋関数。時刻は呼び手が渡す）。
///
/// - begin: 開いている窓（end が未着で [`RECONFIG_OPEN_MAX`] 以内）があればその窓に数え、
///   無ければ新しい窓を始める
/// - end: 直前の窓が開いているか、end から [`RECONFIG_SETTLE`] 以内ならその窓を延ばす。
///   どちらでもなければ（登録より前に始まった再構成など）end の時刻で窓を作る
fn apply_reconfig(
    q: &mut std::collections::VecDeque<ReconfigWindow>,
    ev: &ReconfigEvent,
    now: Instant,
) {
    let off_main = u32::from(!ev.main_thread);
    let fresh = |begun: Instant| ReconfigWindow {
        begun,
        ended: None,
        begins: 0,
        ends: 0,
        displays: Vec::new(),
        end_flags: 0,
        off_main: 0,
        synthetic: false,
        reported: false,
    };
    let joins = q.back().is_some_and(|w| match (ev.phase, w.ended) {
        (ReconfigPhase::Begin, None) | (ReconfigPhase::End, None) => {
            now.saturating_duration_since(w.begun) <= RECONFIG_OPEN_MAX
        }
        (ReconfigPhase::End, Some(ended)) => {
            now.saturating_duration_since(ended) <= RECONFIG_SETTLE
        }
        (ReconfigPhase::Begin, Some(_)) => false,
    });
    if !joins {
        if q.len() >= RECONFIG_CAP {
            q.pop_front();
        }
        q.push_back(fresh(now));
    }
    let Some(w) = q.back_mut() else {
        return;
    };
    w.off_main += off_main;
    w.synthetic |= ev.synthetic;
    if !w.displays.contains(&ev.display) && w.displays.len() < RECONFIG_DISPLAYS_CAP {
        w.displays.push(ev.display);
    }
    match ev.phase {
        ReconfigPhase::Begin => w.begins += 1,
        ReconfigPhase::End => {
            w.ends += 1;
            w.end_flags |= ev.flags;
            w.ended = Some(now);
        }
    }
}

/// 画面構成の変更の通知を記録する（コールバックから呼ぶ。時刻を取って畳むだけ = I/O しない）。
/// A/B（`TAKO_2666_LEGACY=1`）は何もしない
pub fn record_display_reconfig(ev: ReconfigEvent) {
    if issue2666_legacy() {
        return;
    }
    let now = Instant::now();
    let mut q = RECONFIG.lock().unwrap_or_else(|e| e.into_inner());
    apply_reconfig(&mut q, &ev, now);
}

/// 遅延の間に重なった画面構成の変更の要約（#2666）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconfigOverlap {
    /// begin → end（ms）。end が未着なら begin から記録まで
    pub took_ms: u64,
    /// end から記録まで（ms）。end が未着なら `None`
    pub since_end_ms: Option<u64>,
    /// 遅延の間に重なった再構成の数（`took_ms` / `since_end_ms` は最後に始まったもの）
    pub count: usize,
    /// 検証の合成を含む
    pub synthetic: bool,
}

impl ReconfigOverlap {
    /// 再構成（begin・end・合成か）のうち、窓（begin → end + [`RECONFIG_AFTER_GRACE`]。end が未着なら
    /// 記録まで・ただし begin から [`RECONFIG_OPEN_MAX`] まで）が `since`（タイマーの期限）から `now`
    /// （記録）までに重なったものを要約する（無ければ `None`）。時刻は呼び手が渡す純粋関数
    pub fn summarize(
        windows: impl IntoIterator<Item = (Instant, Option<Instant>, bool)>,
        since: Instant,
        now: Instant,
    ) -> Option<Self> {
        let mut out: Option<Self> = None;
        let mut latest: Option<Instant> = None;
        for (begun, ended, synthetic) in windows {
            let reach = match ended {
                Some(e) => e + RECONFIG_AFTER_GRACE,
                None => (begun + RECONFIG_OPEN_MAX).min(now),
            };
            if begun > now || reach <= since {
                continue;
            }
            let r = out.get_or_insert(Self {
                took_ms: 0,
                since_end_ms: None,
                count: 0,
                synthetic: false,
            });
            r.count += 1;
            r.synthetic |= synthetic;
            if latest.is_none_or(|l| begun >= l) {
                latest = Some(begun);
                let end = ended.unwrap_or(now);
                r.took_ms = end.saturating_duration_since(begun).as_millis() as u64;
                r.since_end_ms = ended.map(|e| now.saturating_duration_since(e).as_millis() as u64);
            }
        }
        out
    }

    /// 分類の文に挟む中身（言語ごと）
    fn detail_in(&self, lang: tako_core::i18n::Lang) -> String {
        use tako_core::i18n::Lang::{En, Ja};
        let mut s = match (self.since_end_ms, lang) {
            (Some(after), Ja) => {
                format!("begin → end {}ms・end から記録まで {after}ms", self.took_ms)
            }
            (Some(after), En) => format!(
                "begin → end {}ms, {after}ms from end to the record",
                self.took_ms
            ),
            (None, Ja) => format!("begin から記録まで {}ms・end は未着", self.took_ms),
            (None, En) => format!("{}ms from begin to the record, no end yet", self.took_ms),
        };
        if self.count > 1 {
            s.push_str(&match lang {
                Ja => format!("（遅延の間に {} 回）", self.count),
                En => format!(" ({} of them during the delay)", self.count),
            });
        }
        if self.synthetic {
            s.push_str(match lang {
                Ja => "・検証の合成",
                En => ", synthesized for verification",
            });
        }
        s
    }
}

/// `since`（タイマーの期限）から今までに重なった画面構成の変更（#2666）
pub fn reconfig_overlapping_since(since: Instant) -> Option<ReconfigOverlap> {
    let q = RECONFIG.lock().unwrap_or_else(|e| e.into_inner());
    ReconfigOverlap::summarize(
        q.iter().map(|w| (w.begun, w.ended, w.synthetic)),
        since,
        Instant::now(),
    )
}

/// macOS の `CGDisplayChangeSummaryFlags` の名前（perf.log の行に書く。他の OS は通知を記録しない）
const RECONFIG_FLAG_NAMES: &[(u32, &str)] = &[
    (1 << 1, "moved"),
    (1 << 2, "set_main"),
    (1 << 3, "set_mode"),
    (1 << 4, "add"),
    (1 << 5, "remove"),
    (1 << 8, "enabled"),
    (1 << 9, "disabled"),
    (1 << 10, "mirror"),
    (1 << 11, "unmirror"),
    (1 << 12, "desktop_shape"),
];

/// 変更の内容の和を名前の並びにする（知らない桁は 16 進で残す・何も無ければ「なし」）
fn reconfig_flags_text(flags: u32) -> String {
    let mut names: Vec<String> = Vec::new();
    let mut rest = flags & !1;
    for &(bit, name) in RECONFIG_FLAG_NAMES {
        if rest & bit != 0 {
            names.push(name.to_string());
            rest &= !bit;
        }
    }
    if rest != 0 {
        names.push(format!("0x{rest:x}"));
    }
    if names.is_empty() {
        "なし".to_string()
    } else {
        names.join(",")
    }
}

/// 終わった再構成（または end が来ないまま [`RECONFIG_OPEN_MAX`] を過ぎたもの）を perf.log の行に
/// して取り出す（1 回の再構成につき 1 回。純粋関数）
fn reconfig_reports_at(
    q: &mut std::collections::VecDeque<ReconfigWindow>,
    now: Instant,
) -> Vec<String> {
    let mut out = Vec::new();
    for w in q.iter_mut().filter(|w| !w.reported) {
        let ready = match w.ended {
            Some(ended) => now.saturating_duration_since(ended) >= RECONFIG_SETTLE,
            None => now.saturating_duration_since(w.begun) >= RECONFIG_OPEN_MAX,
        };
        if !ready {
            continue;
        }
        w.reported = true;
        let displays = w
            .displays
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let thread = if w.off_main == 0 {
            "メインスレッド".to_string()
        } else {
            format!("メインスレッド以外 {} 件", w.off_main)
        };
        let synthetic = if w.synthetic {
            "・検証の合成"
        } else {
            ""
        };
        out.push(match w.ended {
            Some(ended) => format!(
                "画面構成の変更（macOS の画面再構成）: begin → end {}ms・画面 {displays}・\
                 通知 begin {} 件 / end {} 件・変更 {}・コールバックは{thread}{synthetic}。#2666",
                ended.saturating_duration_since(w.begun).as_millis(),
                w.begins,
                w.ends,
                reconfig_flags_text(w.end_flags),
            ),
            None => format!(
                "画面構成の変更（macOS の画面再構成）: begin から {}s 経っても end が来ない・\
                 画面 {displays}・通知 begin {} 件・コールバックは{thread}{synthetic}。#2666",
                RECONFIG_OPEN_MAX.as_secs(),
                w.begins,
            ),
        });
    }
    out
}

/// 監視スレッドが 50ms ごとに呼ぶ（ロック中に I/O しない: 行だけ組んで返す。普段は空）
pub fn take_reconfig_reports() -> Vec<String> {
    let mut q = RECONFIG.lock().unwrap_or_else(|e| e.into_inner());
    reconfig_reports_at(&mut q, Instant::now())
}

/// メインスレッドの ThreadId（`mark_main_thread` で登録）。
/// **未登録のプロセスはメインスレッドを持たない扱い**（#944）
static MAIN_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

/// 呼んだスレッドをメインスレッドとして登録する（GUI プロセスの起動時に 1 回）。
///
/// Issue #782: `perf_span` は元々スレッドを区別せず、**background executor 上の
/// 区間まで「メインスレッド専有」として perf.log に書いていた**。実際には
/// background で走っている PDF ラスタライズ（`pdf_rasterize`。#258 で background 化済み）が
/// 「メインスレッド専有 73ms」と記録され、UI ストールの容疑者に見えていた。
/// 登録後は、別スレッドの区間を watchdog の対象（= ハング容疑）にも
/// 「メインスレッド専有」の記録にも入れない
pub fn mark_main_thread() {
    *MAIN_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current().id());
}

/// 呼び出し元が**登録済みの**メインスレッドか。
///
/// Issue #944: 未登録なら真（= 全部メインスレッド扱い）だったので、
/// `mark_main_thread()` を呼ばないプロセス（テストバイナリ・CLI・daemon）が
/// background の区間まで「メインスレッド専有」として記録していた。実測では
/// ユーザーの perf.log に `メインスレッド専有: pdf_rasterize が 71ms` が 643 行
/// 溜まっており、**製品側は background executor で回している**のに
/// UI ストールの容疑者に見えていた。名乗れるのは登録したプロセスだけにする
fn on_main_thread() -> bool {
    let registered = *MAIN_THREAD.lock().unwrap_or_else(|e| e.into_inner());
    if tako_core::paths::issue944_legacy() {
        // A/B: 旧挙動（未登録なら全部メインスレッド扱い）
        return registered.is_none_or(|id| id == std::thread::current().id());
    }
    is_registered_main(registered, std::thread::current().id())
}

/// [`on_main_thread`] の純粋ロジック（登録状態と現在スレッドだけで決まる）
fn is_registered_main(
    registered: Option<std::thread::ThreadId>,
    current: std::thread::ThreadId,
) -> bool {
    registered == Some(current)
}

/// テスト専用: 呼んだスレッドを一時的にメインスレッドとして登録する（#944）。
///
/// `perf_span` の**区間追跡そのもの**を見るテストは登録が要るが、
/// 登録はプロセス全体に効くので、`Drop` で必ず元へ戻し、
/// さらに専用ロックで直列化する（並列テストが互いの登録を覗かない）
#[cfg(test)]
pub(crate) fn scoped_main_thread() -> MainThreadGuard {
    static LOCK: Mutex<()> = Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut slot = MAIN_THREAD.lock().unwrap_or_else(|e| e.into_inner());
    let prev = *slot;
    *slot = Some(std::thread::current().id());
    drop(slot);
    MainThreadGuard { prev, _lock: lock }
}

/// [`scoped_main_thread`] の後始末（登録を元へ戻す）
#[cfg(test)]
pub(crate) struct MainThreadGuard {
    prev: Option<std::thread::ThreadId>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for MainThreadGuard {
    fn drop(&mut self) {
        *MAIN_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = self.prev;
    }
}

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
    /// メインスレッドで開始した区間か（別スレッドは記録の扱いを変える）
    on_main: bool,
    /// 別の区間の内側か（UI ストールの分類では外側だけを数える = 二重に数えない。#1968）
    nested: bool,
}

/// 既定しきい値（32ms、verbose 時 16ms）の計測区間を開始する
pub fn perf_span(tag: impl Into<Cow<'static, str>>) -> PerfSpan {
    perf_span_over(tag, span_threshold_ms())
}

/// しきい値を明示して計測区間を開始する
pub fn perf_span_over(tag: impl Into<Cow<'static, str>>, log_over_ms: u64) -> PerfSpan {
    let tag = tag.into();
    let t0 = Instant::now();
    // 別スレッドの区間は watchdog の現在区間に入れない（#782: background の
    // 区間がメインスレッドのハング容疑者として記録されていた）
    let on_main = on_main_thread();
    let prev = if on_main {
        let mut w = watch_lock();
        let prev = w.current.take();
        w.current = Some(SpanState {
            tag: tag.clone(),
            started: t0,
            hang_reported: false,
        });
        prev
    } else {
        None
    };
    let nested = prev.is_some();
    PerfSpan {
        tag,
        t0,
        log_over_ms,
        prev,
        on_main,
        nested,
    }
}

impl Drop for PerfSpan {
    fn drop(&mut self) {
        let took = self.t0.elapsed().as_millis() as u64;
        if !self.on_main {
            // 別スレッドの所要は verbose 統計にだけ残す（タグに bg: を付けて
            // メインスレッドの分布と混ざらないようにする）
            if perf_verbose() {
                push_sample(Cow::Owned(format!("bg:{}", self.tag)), took);
            }
            return;
        }
        {
            let mut w = watch_lock();
            w.current = self.prev.take();
        }
        if !self.nested && took >= RECENT_SPAN_MIN_MS {
            record_ended_span(self.tag.clone(), took);
        }
        if perf_verbose() {
            push_sample(self.tag.clone(), took);
        }
        if took >= self.log_over_ms && span_log_rate_ok() {
            perf_log(&format!(
                "メインスレッド専有: {} が {took}ms（Issue #168 診断）",
                self.tag
            ));
        }
    }
}

/// verbose 統計へサンプルを 1 件積む（暴走ガード付き）
fn push_sample(tag: Cow<'static, str>, took: u64) {
    let mut w = watch_lock();
    w.samples.push((tag, took));
    // 暴走ガード: watchdog 不在でも無限には溜めない
    if w.samples.len() > 100_000 {
        w.samples.drain(..50_000);
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

// ===== スケジューラ遅延の観測と、UI ストールの原因分類（#643） =====
//
// 「UI ストール」は foreground executor の再開遅延で測っている。ところが Windows では
// タイマーが WinRT スレッドプール、再開がメインスレッドのキューという 2 段構えなので、
// **遅れたのがどちらなのか区別できない**。実測（2026-07-29 の Windows 実機 perf.log）では
// 98 件中 94 件が perf_span と共起せず、tako の専有では説明できなかった
// （#168 のときは 1021 件すべてが dispatch のスパンと共起していた = 対照的）。
//
// そこで watchdog の**素の OS スレッド**自身の sleep 超過を「スケジューラ遅延」として
// 記録する。この値はメインスレッドがどれだけ塞がっても・スレッドプールがどれだけ
// 詰まっても伸びない（伸びるのはマシン全体が過負荷のときだけ）ので、
// 「マシンが重い」と「tako が専有している」を切り分ける物差しになる。
/// 前回の取り出し以降に観測したスケジューラ遅延のピーク（ms）
static SCHED_LAG_PEAK_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// watchdog スレッドの sleep 超過を記録する（ピークだけ保持）
fn record_scheduler_lag(over: Duration) {
    SCHED_LAG_PEAK_MS.fetch_max(over.as_millis() as u64, Ordering::Relaxed);
}

/// 前回の呼び出し以降に観測したスケジューラ遅延のピークを取り出してリセットする。
/// UI ストール監視ループが 1 秒ごとに呼ぶ想定（= 直近 1 秒のピークが取れる）
pub fn take_scheduler_lag_peak() -> Duration {
    Duration::from_millis(SCHED_LAG_PEAK_MS.swap(0, Ordering::Relaxed))
}

/// いまメインスレッドで走っている計測区間（タグ, 経過 ms）。無ければ `None`
pub fn current_span_snapshot() -> Option<(String, u64)> {
    let w = watch_lock();
    w.current.as_ref().map(|s| {
        (
            s.tag.as_ref().to_string(),
            s.started.elapsed().as_millis() as u64,
        )
    })
}

/// スケジューラ遅延がこの値以上なら「マシン全体が過負荷」と断じる。
/// 素の OS スレッドの 50ms sleep が 0.5 秒を超えて返らないのは、
/// tako 側の事情では起こせない（= CPU が他所で飽和している証拠）
const SCHED_OVERLOAD_OVER: Duration = Duration::from_millis(500);

/// UI ストールとして記録する再開遅延の下限（#113）
pub const STALL_REPORT_OVER: Duration = Duration::from_millis(500);

/// UI ストール監視ループの 1 周の判定（#113 / #643 / #1968 / #2665 / #2666）。記録すべきなら perf.log の
/// 1 行（日本語）を返す。
///
/// - `armed`: 監視ループがタイマーを掛けた時刻
/// - `period`: タイマーの長さ（期限 = `armed + period`）
///
/// 監視ループ自身が UI スレッドで走るので、記録する時点で塞いでいた区間は終わっている（#1968）。
/// 分類に使うのは**期限から今まで**（= 遅延の間）に重なった区間の、重なった分だけ（#2665）と、
/// 同じ遅延の間に重なった画面構成の変更（#2666）。
/// スケジューラ遅延のピークは毎周回取り出す（記録するときだけ取ると古いピークが混ざる）
pub fn stall_line(armed: Instant, period: Duration) -> Option<String> {
    let lag = armed.elapsed().saturating_sub(period);
    let sched_lag = take_scheduler_lag_peak();
    if lag < STALL_REPORT_OVER {
        return None;
    }
    let running = current_span_snapshot();
    let span = running.as_ref().map(|(tag, ms)| (tag.as_str(), *ms));
    if issue2665_legacy() {
        // A/B: 修正前の窓（1 秒 + 遅延 = タイマーを掛けた時点まで遡る）・区間の全長・修正前の文
        let recent = (!issue1968_legacy())
            .then(|| recent_spans_within(period + lag))
            .flatten();
        return Some(classify_stall(lag, sched_lag, span, recent.as_ref()));
    }
    // #1968 の A/B（`TAKO_1968_LEGACY=1`）は修正前と同じく終わった区間を使わない
    let delay = (!issue1968_legacy())
        .then(|| spans_overlapping_since(armed + period))
        .flatten();
    // #2666 の A/B（`TAKO_2666_LEGACY=1`）は画面構成の変更を見ない（= #2665 の分類）
    let reconfig = (!issue2666_legacy())
        .then(|| reconfig_overlapping_since(armed + period))
        .flatten();
    Some(
        StallCause::classify(lag, sched_lag, span, delay, reconfig)
            .text_in(tako_core::i18n::Lang::Ja),
    )
}

/// UI ストールの原因の分類（#643 / #1968 / #2665 / #2666）。実測値だけから決め、根拠の無い断定はしない
/// （原因を騙ると次の調査が丸ごと無駄になる）。文は [`StallCause::text_in`]（perf.log は日本語）。
///
/// **名指す順（#2666 で決めた優先の決まり）**: 過負荷（素の OS スレッドまで遅れた）> 継続中の計測区間 >
/// 遅延の半分以上を説明する計測区間（tako の専有）> 画面構成の変更 > 内訳 / 計測区間の外。
/// 計測区間は「tako のどのコードがメインスレッドを持っていたか」の直接の実測なので、画面構成の変更より
/// 先に名指す。ただし画面構成の変更が遅延に重なっていれば、上の 3 つでも**併記する**（変更が塞がりの
/// 引き金のことがある = 描き直しの `render` など。片方を捨てると次の調査が因果を辿れない）。
/// 計測区間が半分に届かないときは、残りを画面構成の変更の処理中と名指し、区間の内訳も書く
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StallCause {
    /// 素の OS スレッドの sleep まで遅れた = マシン全体の過負荷（tako の専有ではない）
    Overload {
        lag: Duration,
        sched_lag: Duration,
        reconfig: Option<ReconfigOverlap>,
    },
    /// 記録の時点でメインスレッドの計測区間が走っていた = tako の専有
    Running {
        lag: Duration,
        tag: String,
        ms: u64,
        reconfig: Option<ReconfigOverlap>,
    },
    /// 遅延の間に重なった計測区間が遅延の半分以上を説明する = tako の専有
    Owned {
        lag: Duration,
        spans: DelaySpans,
        reconfig: Option<ReconfigOverlap>,
    },
    /// 遅延の間に画面構成の変更（macOS の画面再構成）が重なり、計測区間は半分に届かない
    /// = 画面構成の変更の処理中（OS / GPUI 側）。区間があれば内訳も書く（#2666）
    DisplayReconfig {
        lag: Duration,
        reconfig: ReconfigOverlap,
        spans: Option<DelaySpans>,
    },
    /// 遅延の間に計測区間はあるが半分に届かない。内訳を書き、残りは計測区間の外とする
    /// （騙らない・捨てない。#2665）
    Partial { lag: Duration, spans: DelaySpans },
    /// 遅延の間に計測区間が無い = 計測区間の外か再開経路（タイマー / キュー）側の遅延
    Unmeasured { lag: Duration },
}

impl StallCause {
    /// 実測値から分類する（名指す順は [`StallCause`] の説明）。
    ///
    /// - `lag`: foreground executor の再開遅延
    /// - `sched_lag`: 素の OS スレッドの sleep 超過（[`take_scheduler_lag_peak`]）
    /// - `span`: 記録時点でメインスレッドが走らせていた計測区間
    /// - `delay`: 遅延の間に重なった区間（[`spans_overlapping_since`]）
    /// - `reconfig`: 遅延の間に重なった画面構成の変更（[`reconfig_overlapping_since`]）
    pub fn classify(
        lag: Duration,
        sched_lag: Duration,
        span: Option<(&str, u64)>,
        delay: Option<DelaySpans>,
        reconfig: Option<ReconfigOverlap>,
    ) -> Self {
        if sched_lag >= SCHED_OVERLOAD_OVER {
            return Self::Overload {
                lag,
                sched_lag,
                reconfig,
            };
        }
        if let Some((tag, ms)) = span {
            return Self::Running {
                lag,
                tag: tag.to_string(),
                ms,
                reconfig,
            };
        }
        match (delay, reconfig) {
            (Some(spans), reconfig)
                if spans.overlap_ms.saturating_mul(2) >= lag.as_millis() as u64 =>
            {
                Self::Owned {
                    lag,
                    spans,
                    reconfig,
                }
            }
            (spans, Some(reconfig)) => Self::DisplayReconfig {
                lag,
                reconfig,
                spans,
            },
            (Some(spans), None) => Self::Partial { lag, spans },
            (None, None) => Self::Unmeasured { lag },
        }
    }

    /// tako の専有と名指すか（過負荷・画面構成の変更・計測区間の外・内訳だけのときは偽）
    pub fn names_tako(&self) -> bool {
        matches!(self, Self::Running { .. } | Self::Owned { .. })
    }

    /// 画面構成の変更を原因として名指すか（併記だけのときは偽。#2666）
    pub fn names_reconfig(&self) -> bool {
        matches!(self, Self::DisplayReconfig { .. })
    }

    /// 遅延に重なった画面構成の変更（名指し・併記のどちらでも。#2666）
    pub fn reconfig(&self) -> Option<&ReconfigOverlap> {
        match self {
            Self::Overload { reconfig, .. }
            | Self::Running { reconfig, .. }
            | Self::Owned { reconfig, .. } => reconfig.as_ref(),
            Self::DisplayReconfig { reconfig, .. } => Some(reconfig),
            Self::Partial { .. } | Self::Unmeasured { .. } => None,
        }
    }

    /// 表示言語を明示しての 1 行。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語の過負荷・継続中は #2665 より前と、
    /// 画面構成の変更が重ならない行は #2666 より前とバイト一致
    pub fn text_in(&self, lang: tako_core::i18n::Lang) -> String {
        use tako_core::i18n::Lang::{En, Ja};
        let lag = match self {
            Self::Overload { lag, .. }
            | Self::Running { lag, .. }
            | Self::Owned { lag, .. }
            | Self::DisplayReconfig { lag, .. }
            | Self::Partial { lag, .. }
            | Self::Unmeasured { lag } => *lag,
        };
        let head = match lang {
            Ja => format!(
                "UI ストール: foreground executor の再開が {:.2}s 遅延",
                lag.as_secs_f64()
            ),
            En => format!(
                "UI stall: the foreground executor resumed {:.2}s late",
                lag.as_secs_f64()
            ),
        };
        let lag_ms = (lag.as_millis() as u64).max(1);
        let body = match (self, lang) {
            (Self::Overload { sched_lag, .. }, Ja) => format!(
                "素の OS スレッドの sleep も {:.2}s 超過 = マシン全体の過負荷。\
                 tako のメインスレッド専有ではない",
                sched_lag.as_secs_f64()
            ),
            (Self::Overload { sched_lag, .. }, En) => format!(
                "a plain OS thread's sleep also overran by {:.2}s = the whole machine \
                 is overloaded, not tako holding the main thread",
                sched_lag.as_secs_f64()
            ),
            (Self::Running { tag, ms, .. }, Ja) => {
                format!("OS スレッドは正常。メインスレッドで {tag} が {ms}ms 継続中 = tako の専有")
            }
            (Self::Running { tag, ms, .. }, En) => format!(
                "OS threads are fine; {tag} has been running on the main thread for \
                 {ms}ms = held by tako"
            ),
            (Self::Owned { spans: s, .. }, Ja) => format!(
                "OS スレッドは正常。遅延の間にメインスレッドの計測区間が計 {}ms・{} 件\
                 （最長 {}: 全長 {}ms のうち遅延の間 {}ms）= tako の専有。#1968 / #2665",
                s.overlap_ms, s.count, s.longest_tag, s.longest_took_ms, s.longest_overlap_ms
            ),
            (Self::Owned { spans: s, .. }, En) => format!(
                "OS threads are fine; measured main-thread spans during the delay total \
                 {}ms over {} (longest: {}, {}ms of its {}ms) = held by tako. #1968 / #2665",
                s.overlap_ms, s.count, s.longest_tag, s.longest_overlap_ms, s.longest_took_ms
            ),
            (
                Self::DisplayReconfig {
                    reconfig: r,
                    spans: None,
                    ..
                },
                Ja,
            ) => format!(
                "OS スレッドは正常・遅延の間に計測区間なし = 画面構成の変更の処理中（OS / GPUI 側）: \
                 {}。#2666",
                r.detail_in(Ja)
            ),
            (
                Self::DisplayReconfig {
                    reconfig: r,
                    spans: None,
                    ..
                },
                En,
            ) => format!(
                "OS threads are fine; no measured span during the delay = processing a display \
                 configuration change (OS / GPUI side): {}. #2666",
                r.detail_in(En)
            ),
            (
                Self::DisplayReconfig {
                    reconfig: r,
                    spans: Some(s),
                    ..
                },
                Ja,
            ) => format!(
                "OS スレッドは正常。遅延の間の計測区間は計 {}ms・{} 件 = 遅延の {}%\
                 （最長 {}: 全長 {}ms のうち遅延の間 {}ms）。残り {}ms は画面構成の変更の処理中\
                 （OS / GPUI 側）: {}。#2666",
                s.overlap_ms,
                s.count,
                s.overlap_ms * 100 / lag_ms,
                s.longest_tag,
                s.longest_took_ms,
                s.longest_overlap_ms,
                lag_ms.saturating_sub(s.overlap_ms),
                r.detail_in(Ja)
            ),
            (
                Self::DisplayReconfig {
                    reconfig: r,
                    spans: Some(s),
                    ..
                },
                En,
            ) => format!(
                "OS threads are fine; measured spans during the delay total {}ms over {} \
                 = {}% of the delay (longest: {}, {}ms of its {}ms); the remaining {}ms is \
                 processing a display configuration change (OS / GPUI side): {}. #2666",
                s.overlap_ms,
                s.count,
                s.overlap_ms * 100 / lag_ms,
                s.longest_tag,
                s.longest_overlap_ms,
                s.longest_took_ms,
                lag_ms.saturating_sub(s.overlap_ms),
                r.detail_in(En)
            ),
            (Self::Partial { spans: s, .. }, Ja) => format!(
                "OS スレッドは正常。遅延の間の計測区間は計 {}ms・{} 件 = 遅延の {}%\
                 （最長 {}: 全長 {}ms のうち遅延の間 {}ms）。残り {}ms は計測区間の外か\
                 再開経路（タイマー / キュー）側の遅延。#643 / #2665",
                s.overlap_ms,
                s.count,
                s.overlap_ms * 100 / lag_ms,
                s.longest_tag,
                s.longest_took_ms,
                s.longest_overlap_ms,
                lag_ms.saturating_sub(s.overlap_ms)
            ),
            (Self::Partial { spans: s, .. }, En) => format!(
                "OS threads are fine; measured spans during the delay total {}ms over {} \
                 = {}% of the delay (longest: {}, {}ms of its {}ms); the remaining {}ms is \
                 outside measured spans or in the resume path (timer / queue). #643 / #2665",
                s.overlap_ms,
                s.count,
                s.overlap_ms * 100 / lag_ms,
                s.longest_tag,
                s.longest_overlap_ms,
                s.longest_took_ms,
                lag_ms.saturating_sub(s.overlap_ms)
            ),
            (Self::Unmeasured { .. }, Ja) => "OS スレッドは正常・遅延の間に計測区間なし = \
                 計測区間の外か再開経路（タイマー / キュー）側の遅延。#643 / #2665"
                .to_string(),
            (Self::Unmeasured { .. }, En) => {
                "OS threads are fine; no measured span during the delay = outside \
                 measured spans or in the resume path (timer / queue). #643 / #2665"
                    .to_string()
            }
        };
        // #2666: 計測区間で名指した行にも、重なった画面構成の変更を併記する（名指しの腕は本文に書いた）
        let also = match self {
            Self::Overload { reconfig, .. }
            | Self::Running { reconfig, .. }
            | Self::Owned { reconfig, .. } => reconfig.as_ref(),
            Self::DisplayReconfig { .. } | Self::Partial { .. } | Self::Unmeasured { .. } => None,
        };
        match (also, lang) {
            (None, Ja) => format!("{head}（{body}）"),
            (None, En) => format!("{head} ({body})"),
            (Some(r), Ja) => format!(
                "{head}（{body}。画面構成の変更とも重なる: {}。#2666）",
                r.detail_in(Ja)
            ),
            (Some(r), En) => format!(
                "{head} ({body}; also overlaps a display configuration change: {}. #2666)",
                r.detail_in(En)
            ),
        }
    }
}

/// 修正前（#2665 の A/B `TAKO_2665_LEGACY=1`）の UI ストールの分類（#643 / #1968）。
/// `recent` は「1 秒 + 遅延」の窓に**終わった**区間の全長の要約（期限より前に終わった区間も数える）。
///
/// - `ui_lag`: foreground executor の再開遅延
/// - `sched_lag`: 素の OS スレッドの sleep 超過（[`take_scheduler_lag_peak`]）
/// - `span`: 記録時点でメインスレッドが走らせていた計測区間
pub fn classify_stall(
    ui_lag: Duration,
    sched_lag: Duration,
    span: Option<(&str, u64)>,
    recent: Option<&RecentSpans>,
) -> String {
    let head = format!(
        "UI ストール: foreground executor の再開が {:.2}s 遅延",
        ui_lag.as_secs_f64()
    );
    if sched_lag >= SCHED_OVERLOAD_OVER {
        return format!(
            "{head}（素の OS スレッドの sleep も {:.2}s 超過 = マシン全体の過負荷。\
             tako のメインスレッド専有ではない）",
            sched_lag.as_secs_f64()
        );
    }
    if let Some((tag, ms)) = span {
        return format!(
            "{head}（OS スレッドは正常。メインスレッドで {tag} が {ms}ms 継続中 = tako の専有）"
        );
    }
    // #1968: 遅延の窓に終わった区間が遅延の半分以上を説明するなら tako の専有
    if let Some(r) = recent.filter(|r| r.total_ms.saturating_mul(2) >= ui_lag.as_millis() as u64) {
        return format!(
            "{head}（OS スレッドは正常。直前にメインスレッドの計測区間が計 {}ms・{} 件 \
             （最長 {} が {}ms）= tako の専有。#1968）",
            r.total_ms, r.count, r.longest_tag, r.longest_ms
        );
    }
    let measured = recent.map(|r| r.total_ms).unwrap_or(0);
    format!(
        "{head}（OS スレッドは正常・直前の計測区間は計 {measured}ms = \
         計測区間の外か再開経路（タイマー / キュー）側の遅延。#643 / #1968）"
    )
}

/// ハング級（[`HANG_REPORT_AFTER`] を超えて継続中）の区間の中間報告を、区間ごとに 1 回だけ取り出す。
/// 監視スレッドが 50ms ごとに呼ぶ（ロック中に I/O しない: 文だけ組んで返す）
pub fn take_hang_report() -> Option<String> {
    let mut w = watch_lock();
    match w.current.as_mut() {
        Some(cur) if !cur.hang_reported && cur.started.elapsed() >= HANG_REPORT_AFTER => {
            cur.hang_reported = true;
            Some(format!(
                "メインスレッド長時間専有（継続中）: {} が {}ms 経過",
                cur.tag,
                cur.started.elapsed().as_millis()
            ))
        }
        _ => None,
    }
}

/// #2665 の注入の口の env（[`Issue2665Inject`]。検証プロセスでだけ読む）
pub const ISSUE2665_INJECT_FILE_ENV: &str = "TAKO_2665_INJECT_FILE";

/// #2665 の検証専用の注入（`TAKO_2665_INJECT_FILE`）。env が指すファイルの中身
/// （`out=<ms>` / `in=<ms>` / `after=<ms>`。空白区切りで並べてよい）に従い、`tako window list` の
/// 処理でメインスレッドを塞ぐ: `out` は計測区間の**外**（IPC の受信ループで `ipc_turn` の前）、
/// `in` は計測区間の**中**（`ipc_turn` の内側）、`after` は `ipc_turn` の区間が閉じた**直後**の外。
/// 並べたものは out → in → after の順に途切れず続く（監視ループは間に割り込めない）。
/// **検証プロセス（`TAKO_ISOLATED` 等。`tako_core::paths::is_verification_process`）でだけ効く**:
/// 本番の GUI は env があっても読まない（UI スレッドを外から止められる口を本番に開けない）。
/// env が無ければ何も読まない。隔離 GUI で UI ストールの分類を実経路で確かめるためのもの
/// （`scripts/test-stall-window-2665.sh`）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Issue2665Inject {
    pub out_of_span: Option<Duration>,
    pub in_span: Option<Duration>,
    pub after_span: Option<Duration>,
}

impl Issue2665Inject {
    /// ファイルの中身を読む（読めない語は無視する）
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for word in text.split_whitespace() {
            let Some((key, ms)) = word.split_once('=') else {
                continue;
            };
            let Ok(ms) = ms.parse::<u64>() else {
                continue;
            };
            match key {
                "out" => out.out_of_span = Some(Duration::from_millis(ms)),
                "in" => out.in_span = Some(Duration::from_millis(ms)),
                "after" => out.after_span = Some(Duration::from_millis(ms)),
                _ => {}
            }
        }
        out
    }

    /// 注入の対象の要求（`tako window list`）なら、env が指すファイルを読んで注入を決める
    /// （検証プロセスでなければ常に何もしない）
    pub fn for_request(target: bool) -> Self {
        static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
        if !target {
            return Self::default();
        }
        FILE.get_or_init(|| {
            if !tako_core::paths::is_verification_process() {
                return None;
            }
            std::env::var_os(ISSUE2665_INJECT_FILE_ENV).map(PathBuf::from)
        })
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Self::parse(&text))
        .unwrap_or_default()
    }

    /// 計測区間の外で塞ぐ（IPC の受信ループで `ipc_turn` の前に呼ぶ）
    pub fn block_out_of_span(&self) {
        if let Some(d) = self.out_of_span {
            std::thread::sleep(d);
        }
    }

    /// 計測区間の中で塞ぐ（`ipc_turn` の区間の内側で呼ぶ）
    pub fn block_in_span(&self) {
        if let Some(d) = self.in_span {
            std::thread::sleep(d);
        }
    }

    /// 計測区間の直後に外で塞ぐ（`ipc_turn` の区間が閉じた直後に呼ぶ）
    pub fn block_after_span(&self) {
        if let Some(d) = self.after_span {
            std::thread::sleep(d);
        }
    }
}

/// #2666 の注入の口の env（[`Issue2666Inject`]。検証プロセスでだけ読む）
pub const ISSUE2666_INJECT_FILE_ENV: &str = "TAKO_2666_INJECT_FILE";

/// [`Issue2666Inject`] の 1 手
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Issue2666Step {
    /// 画面ごとに begin の通知を合成する
    Begin,
    /// 画面ごとに end の通知を合成する
    End,
    /// 計測区間の外でメインスレッドを塞ぐ
    Sleep(Duration),
    /// 計測区間（`t2666_inject`）の中でメインスレッドを塞ぐ
    Span(Duration),
}

/// #2666 の検証専用の注入（`TAKO_2666_INJECT_FILE`）。蓋の開閉は検証で起こせず、tako-vd も触らないので、
/// 画面構成の変更は**通知の合成**で再現する。env が指すファイルの中身（空白区切りの手の並び:
/// `begin` / `end` / `sleep=<ms>` / `span=<ms>` / `displays=<n>`（合成する画面の数・既定 2））を、
/// `tako window list` の処理でメインスレッド上に書いた順に行う（例 `begin sleep=1800 end`）。
/// begin / end は呼び手が渡す合成の口（tako-app の `platform::display_reconfig::synthesize` = 本物の
/// コールバックの関数を呼ぶ）を通る。
/// **検証プロセス（`TAKO_ISOLATED` 等。`tako_core::paths::is_verification_process`）でだけ効く**:
/// 本番の GUI は env があっても読まない（UI スレッドを外から止められる口を本番に開けない。#2665 と同じ）。
/// env が無ければ何も読まない（`scripts/test-display-reconfig-2666.sh`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue2666Inject {
    pub steps: Vec<Issue2666Step>,
    pub displays: u32,
}

impl Default for Issue2666Inject {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            displays: 2,
        }
    }
}

impl Issue2666Inject {
    /// ファイルの中身を読む（読めない語は無視する）
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for word in text.split_whitespace() {
            let ms = |v: &str| v.parse::<u64>().ok().map(Duration::from_millis);
            match word.split_once('=') {
                None if word == "begin" => out.steps.push(Issue2666Step::Begin),
                None if word == "end" => out.steps.push(Issue2666Step::End),
                Some(("sleep", v)) => out.steps.extend(ms(v).map(Issue2666Step::Sleep)),
                Some(("span", v)) => out.steps.extend(ms(v).map(Issue2666Step::Span)),
                Some(("displays", v)) => {
                    if let Ok(n) = v.parse::<u32>() {
                        out.displays = n.clamp(1, 16);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// 注入の対象の要求（`tako window list`）なら、env が指すファイルを読んで注入を決める
    /// （検証プロセスでなければ常に何もしない）
    pub fn for_request(target: bool) -> Self {
        static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
        if !target {
            return Self::default();
        }
        FILE.get_or_init(|| {
            tako_core::paths::is_verification_process()
                .then(|| std::env::var_os(ISSUE2666_INJECT_FILE_ENV).map(PathBuf::from))
                .flatten()
        })
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Self::parse(&text))
        .unwrap_or_default()
    }

    /// 手を順に行う（`synthesize(段, 画面)` が通知の合成。IPC の受信ループで `ipc_turn` の前に呼ぶ）
    pub fn run(&self, synthesize: impl Fn(ReconfigPhase, u32)) {
        for step in &self.steps {
            match *step {
                Issue2666Step::Begin => {
                    (1..=self.displays).for_each(|d| synthesize(ReconfigPhase::Begin, d));
                }
                Issue2666Step::End => {
                    (1..=self.displays).for_each(|d| synthesize(ReconfigPhase::End, d));
                }
                Issue2666Step::Sleep(d) => std::thread::sleep(d),
                Issue2666Step::Span(d) => {
                    let _span = perf_span("t2666_inject");
                    std::thread::sleep(d);
                }
            }
        }
    }
}

/// メインスレッド・ウォッチドッグを起動する（多重呼び出しは無視）。
/// 50ms ごとに現在区間を確認し、ハング級（2 秒超え継続）を drop を待たず記録する。
/// 同時に自分自身の sleep 超過を [`record_scheduler_lag`] へ積む（#643）。
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
                let tick = Instant::now();
                std::thread::sleep(Duration::from_millis(50));
                // #643: 自分の遅れ = マシン全体の混み具合。tako 側の専有では伸びない
                record_scheduler_lag(tick.elapsed().saturating_sub(Duration::from_millis(50)));
                if let Some(msg) = take_hang_report() {
                    perf_log(&msg);
                }
                // #2666: 終わった画面構成の変更を 1 回につき 1 行（コールバックの側では I/O しない）
                for line in take_reconfig_reports() {
                    perf_log(&line);
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

    // --- #944: メインスレッド未登録のプロセスは「メインスレッド専有」と名乗らない ---

    #[test]
    fn メインスレッド未登録なら専有として記録しない() {
        let current = std::thread::current().id();
        let other = std::thread::spawn(|| std::thread::current().id())
            .join()
            .expect("別スレッドの id");

        // 未登録（テストバイナリ・CLI・daemon の既定）: 誰もメインスレッドではない
        assert!(!is_registered_main(None, current));
        assert!(!is_registered_main(None, other));
        // 登録済み（GUI プロセスの main）: 登録したスレッドだけが真
        assert!(is_registered_main(Some(current), current));
        assert!(!is_registered_main(Some(other), current));
    }

    #[test]
    fn このテストプロセスはメインスレッドを登録していない() {
        // `mark_main_thread()` は tako-app の main() でしか呼ばない。
        // テストバイナリで真になっていたら #944 の誤記録が復活している。
        // `scoped_main_thread()` を使うテストと同じロックを取って直列化する
        // （`--test-threads=1` だと同じスレッドで前後するため）
        // ロックを持ったまま「元の登録」を見る（drop 後に見ると並列テストと競合する）
        let guard = scoped_main_thread();
        assert_eq!(
            guard.prev, None,
            "テストバイナリがメインスレッドを名乗っている（#944）"
        );
    }

    // --- #643: UI ストールの原因分類 ---

    #[test]
    fn os_スレッドまで遅れていればマシン全体の過負荷と断じる() {
        // 素の OS スレッドの 50ms sleep が 3 秒超過 = tako 側の事情では起こせない
        let msg = classify_stall(
            Duration::from_millis(5500),
            Duration::from_millis(3000),
            // 計測区間が走っていても、OS スレッドの遅れが勝つ（そちらが上流の原因）
            Some(("render", 40)),
            None,
        );
        assert!(msg.contains("マシン全体の過負荷"), "{msg}");
        assert!(msg.contains("5.50s"), "{msg}");
        assert!(msg.contains("3.00s"), "{msg}");
    }

    #[test]
    fn os_スレッドが正常で計測区間が走っていれば_tako_の専有と分かる() {
        let msg = classify_stall(
            Duration::from_millis(2000),
            Duration::from_millis(10),
            Some(("dispatch:GitLog", 1900)),
            None,
        );
        assert!(msg.contains("tako の専有"), "{msg}");
        assert!(msg.contains("dispatch:GitLog"), "{msg}");
        assert!(msg.contains("1900ms"), "{msg}");
        assert!(!msg.contains("マシン全体の過負荷"), "{msg}");
    }

    #[test]
    fn どちらでもなければ再開経路の遅延として原因を騙らない() {
        let msg = classify_stall(
            Duration::from_millis(4000),
            Duration::from_millis(20),
            None,
            None,
        );
        assert!(msg.contains("再開経路"), "{msg}");
        assert!(!msg.contains("tako の専有"), "{msg}");
        assert!(!msg.contains("マシン全体の過負荷"), "{msg}");
    }

    #[test]
    fn 終わった区間が遅延の半分以上を説明すれば_tako_の専有と分かる() {
        // #1968: 監視ループは UI スレッドで走るので、記録の時点で区間は終わっている
        let r = RecentSpans {
            longest_tag: "offload_prepare:OrchestratorWorkerStatus".into(),
            longest_ms: 700,
            total_ms: 1100,
            count: 3,
        };
        let msg = classify_stall(
            Duration::from_millis(2000),
            Duration::from_millis(10),
            None,
            Some(&r),
        );
        assert!(msg.contains("tako の専有"), "{msg}");
        assert!(
            msg.contains("offload_prepare:OrchestratorWorkerStatus"),
            "{msg}"
        );
        assert!(msg.contains("1100ms"), "{msg}");
        // 半分に届かなければ騙らない（計測の外 / 再開経路のまま。合計は書く）
        let small = RecentSpans {
            total_ms: 900,
            ..r.clone()
        };
        let msg = classify_stall(
            Duration::from_millis(2000),
            Duration::from_millis(10),
            None,
            Some(&small),
        );
        assert!(!msg.contains("tako の専有"), "{msg}");
        assert!(msg.contains("900ms"), "{msg}");
    }

    #[test]
    fn 終わった区間の履歴は外側だけを数える() {
        let _main = scoped_main_thread();
        let tag = format!("test1968-{}", std::process::id());
        {
            let _outer = perf_span(format!("{tag}-outer"));
            {
                let _inner = perf_span(format!("{tag}-inner"));
                std::thread::sleep(Duration::from_millis(12));
            }
            std::thread::sleep(Duration::from_millis(12));
        }
        let q = RECENT_SPANS.lock().unwrap_or_else(|e| e.into_inner());
        let mine: Vec<&EndedSpan> = q.iter().filter(|s| s.tag.starts_with(&tag)).collect();
        assert_eq!(mine.len(), 1, "外側の 1 件だけ");
        assert!(mine[0].tag.ends_with("-outer"));
        assert!(mine[0].took_ms >= 24, "{}", mine[0].took_ms);
    }

    #[test]
    fn 過負荷判定の境界はちょうど_500ms() {
        let ui = Duration::from_secs(3);
        let just_under = classify_stall(
            ui,
            SCHED_OVERLOAD_OVER - Duration::from_millis(1),
            None,
            None,
        );
        let exactly = classify_stall(ui, SCHED_OVERLOAD_OVER, None, None);
        assert!(!just_under.contains("マシン全体の過負荷"), "{just_under}");
        assert!(exactly.contains("マシン全体の過負荷"), "{exactly}");
    }

    // --- #2665: 遅延の間に重なった分だけで分類する ---

    /// (タグ, 終わった時刻 = 期限からのずれ ms, 全長 ms) を期限 `deadline` の前後に並べる
    fn spans_at(
        deadline: Instant,
        spans: &[(&'static str, i64, u64)],
    ) -> Vec<(&'static str, Instant, u64)> {
        spans
            .iter()
            .map(|&(tag, end_ms, took)| {
                let ended = if end_ms >= 0 {
                    deadline + Duration::from_millis(end_ms as u64)
                } else {
                    deadline - Duration::from_millis(end_ms.unsigned_abs())
                };
                (tag, ended, took)
            })
            .collect()
    }

    #[test]
    fn 期限より前に終わった区間は遅延の間に数えない() {
        let deadline = Instant::now() + Duration::from_secs(5);
        // 期限の 400ms 前に終わった 300ms の区間（#2665 の注入「中 300ms → 外 900ms」の形）
        let s = spans_at(deadline, &[("ipc_turn", -400, 300)]);
        assert_eq!(DelaySpans::summarize(s, deadline), None);
    }

    #[test]
    fn 期限をまたぐ区間ははみ出した分だけ数える() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let s = spans_at(
            deadline,
            &[("ipc_turn", 1000, 1500), ("periodic_prep", 1300, 200)],
        );
        let d = DelaySpans::summarize(s, deadline).expect("重なりがある");
        assert_eq!(d.longest_tag, "ipc_turn");
        assert_eq!(d.longest_took_ms, 1500);
        assert_eq!(d.longest_overlap_ms, 1000);
        assert_eq!(d.overlap_ms, 1200);
        assert_eq!(d.count, 2);
    }

    #[test]
    fn 遅延の半分以上を説明すれば専有_届かなければ内訳と残りを書く() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let owned = StallCause::classify(
            Duration::from_millis(1000),
            Duration::from_millis(10),
            None,
            DelaySpans::summarize(spans_at(deadline, &[("ipc_turn", 1000, 1500)]), deadline),
            None,
        );
        assert!(owned.names_tako(), "{owned:?}");
        let ja = owned.text_in(tako_core::i18n::Lang::Ja);
        assert!(ja.contains("tako の専有"), "{ja}");
        assert!(ja.contains("全長 1500ms のうち遅延の間 1000ms"), "{ja}");

        // 外 1200ms（期限は 300ms の位置）の直後に中 400ms = 遅延 1.3 秒のうち 400ms だけ測れている
        let partial = StallCause::classify(
            Duration::from_millis(1300),
            Duration::from_millis(10),
            None,
            DelaySpans::summarize(spans_at(deadline, &[("ipc_turn", 1300, 400)]), deadline),
            None,
        );
        assert!(!partial.names_tako(), "{partial:?}");
        let ja = partial.text_in(tako_core::i18n::Lang::Ja);
        assert!(!ja.contains("tako の専有"), "騙らない: {ja}");
        assert!(ja.contains("遅延の 30%"), "{ja}");
        assert!(ja.contains("残り 900ms は計測区間の外"), "捨てない: {ja}");
        let en = partial.text_in(tako_core::i18n::Lang::En);
        assert!(en.contains("30% of the delay"), "{en}");
        assert!(en.contains("remaining 900ms"), "{en}");
        assert!(!en.contains("held by tako"), "{en}");

        let none = StallCause::classify(
            Duration::from_millis(600),
            Duration::from_millis(10),
            None,
            None,
            None,
        );
        assert_eq!(
            none,
            StallCause::Unmeasured {
                lag: Duration::from_millis(600)
            }
        );
        assert!(none
            .text_in(tako_core::i18n::Lang::Ja)
            .contains("遅延の間に計測区間なし = 計測区間の外"));
    }

    #[test]
    fn 過負荷と継続中の日本語は修正前とバイト一致() {
        let lag = Duration::from_millis(1234);
        for (sched, span) in [
            (SCHED_OVERLOAD_OVER, None),
            (Duration::from_millis(10), Some(("dispatch:Mod", 812))),
        ] {
            assert_eq!(
                StallCause::classify(lag, sched, span, None, None)
                    .text_in(tako_core::i18n::Lang::Ja),
                classify_stall(lag, sched, span, None)
            );
        }
    }

    #[test]
    fn 分類の文は日英どちらも頭と数値を持つ() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let d = || DelaySpans::summarize(spans_at(deadline, &[("render", 500, 900)]), deadline);
        let lag = Duration::from_millis(2000);
        let causes = [
            StallCause::classify(lag, SCHED_OVERLOAD_OVER, None, None, None),
            StallCause::classify(lag, Duration::ZERO, Some(("render", 700)), None, None),
            StallCause::classify(Duration::from_millis(800), Duration::ZERO, None, d(), None),
            StallCause::classify(lag, Duration::ZERO, None, d(), None),
            StallCause::classify(lag, Duration::ZERO, None, None, None),
            StallCause::classify(
                lag,
                Duration::ZERO,
                None,
                None,
                Some(overlap(71, Some(1545))),
            ),
        ];
        let kinds: Vec<_> = causes.iter().map(std::mem::discriminant).collect();
        for (i, k) in kinds.iter().enumerate() {
            assert!(!kinds[..i].contains(k), "6 種を 1 つずつ: {causes:?}");
        }
        // 併記・内訳つき・end 未着・回数・合成の文も同じ頭と英語だけの英文を持つ
        let mut causes = causes.to_vec();
        let mut many = overlap(900, None);
        many.count = 2;
        many.synthetic = true;
        causes.extend([
            StallCause::classify(lag, SCHED_OVERLOAD_OVER, None, None, Some(many.clone())),
            StallCause::classify(
                lag,
                Duration::ZERO,
                Some(("render", 700)),
                None,
                Some(many.clone()),
            ),
            StallCause::classify(
                Duration::from_millis(800),
                Duration::ZERO,
                None,
                d(),
                Some(many.clone()),
            ),
            StallCause::classify(lag, Duration::ZERO, None, d(), Some(many)),
        ]);
        for c in &causes {
            let ja = c.text_in(tako_core::i18n::Lang::Ja);
            let en = c.text_in(tako_core::i18n::Lang::En);
            assert!(
                ja.starts_with("UI ストール: foreground executor の再開が"),
                "{ja}"
            );
            assert!(
                en.starts_with("UI stall: the foreground executor resumed"),
                "{en}"
            );
            assert!(
                !en.chars().any(|ch| ('\u{3040}'..='\u{30ff}').contains(&ch)),
                "{en}"
            );
        }
    }

    // --- #2666: 画面構成の変更 ---

    /// 要約を直に組む（begin → end `took` ms・end から記録まで `since_end` ms・1 回・合成でない）
    fn overlap(took: u64, since_end: Option<u64>) -> ReconfigOverlap {
        ReconfigOverlap {
            took_ms: took,
            since_end_ms: since_end,
            count: 1,
            synthetic: false,
        }
    }

    fn ev(phase: ReconfigPhase, display: u32, flags: u32) -> ReconfigEvent {
        ReconfigEvent {
            phase,
            display,
            flags,
            main_thread: true,
            synthetic: false,
        }
    }

    #[test]
    fn 画面ごとの_begin_と_end_は_1_回の再構成に畳む() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut q = std::collections::VecDeque::new();
        // 本番の並び: begin が画面ぶん続き、71ms 後に end が画面ぶん続く
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 1, 1), ms(0));
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 6, 1), ms(0));
        apply_reconfig(&mut q, &ev(ReconfigPhase::End, 1, 1 << 9), ms(71));
        apply_reconfig(&mut q, &ev(ReconfigPhase::End, 6, 1 << 2), ms(72));
        assert_eq!(q.len(), 1, "{q:?}");
        let w = &q[0];
        assert_eq!((w.begins, w.ends), (2, 2));
        assert_eq!((w.begun, w.ended), (ms(0), Some(ms(72))));
        assert_eq!(w.end_flags, (1 << 9) | (1 << 2));

        // end の後の begin は次の再構成
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 1, 1), ms(100));
        assert_eq!(q.len(), 2);
        // end が settle を過ぎてから来たら、登録より前に始まった再構成として end の時刻で窓を作る
        apply_reconfig(&mut q, &ev(ReconfigPhase::End, 1, 0), ms(150));
        apply_reconfig(&mut q, &ev(ReconfigPhase::End, 1, 0), ms(150 + 301));
        assert_eq!(q.len(), 3);
        assert_eq!((q[2].begun, q[2].begins), (ms(451), 0));
        // 開いたまま OPEN_MAX を過ぎた begin には数えない
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 1, 1), ms(1_000));
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 1, 1), ms(1_000 + 10_001));
        assert_eq!(q.len(), 5);
        // メインスレッド以外・合成は窓に残る
        let mut off = ev(ReconfigPhase::End, 2, 0);
        off.main_thread = false;
        off.synthetic = true;
        apply_reconfig(&mut q, &off, ms(11_002));
        assert_eq!((q[4].off_main, q[4].synthetic), (1, true));
        // 上限で古いものから捨てる
        for i in 0..40u64 {
            apply_reconfig(
                &mut q,
                &ev(ReconfigPhase::Begin, 1, 1),
                ms(20_000 + i * 1_000),
            );
            apply_reconfig(
                &mut q,
                &ev(ReconfigPhase::End, 1, 0),
                ms(20_000 + i * 1_000 + 10),
            );
        }
        assert_eq!(q.len(), RECONFIG_CAP);
    }

    #[test]
    fn 遅延の窓に重なる再構成だけを数え_end_の後は猶予まで延ばす() {
        let deadline = Instant::now() + Duration::from_secs(30);
        let at = |ms: i64| {
            if ms >= 0 {
                deadline + Duration::from_millis(ms as u64)
            } else {
                deadline - Duration::from_millis(ms.unsigned_abs())
            }
        };
        let now = at(1_000);
        let sum = |w: &[(i64, Option<i64>)]| {
            ReconfigOverlap::summarize(
                w.iter().map(|&(b, e)| (at(b), e.map(at), false)),
                deadline,
                now,
            )
        };
        // begin → end が遅延の中（合成の begin → 塞ぐ → end）
        assert_eq!(sum(&[(100, Some(900))]), Some(overlap(800, Some(100))));
        // end が期限より前でも猶予（2 秒）の内なら数える（本番: end の後に 1.4 秒塞がった）
        assert_eq!(sum(&[(-600, Some(-500))]), Some(overlap(100, Some(1_500))));
        // 猶予の外（end + 2 秒 <= 期限）は数えない
        assert_eq!(sum(&[(-2_100, Some(-2_000))]), None);
        assert!(sum(&[(-2_100, Some(-1_999))]).is_some(), "境界の内側");
        // end が未着: 記録まで処理中（ただし begin から OPEN_MAX まで）
        assert_eq!(sum(&[(-300, None)]), Some(overlap(1_300, None)));
        assert_eq!(sum(&[(-10_000, None)]), None, "OPEN_MAX ちょうどで閉じる");
        assert!(sum(&[(-9_999, None)]).is_some());
        // 複数なら回数と最後に始まったもの
        let two = sum(&[(-700, Some(-600)), (200, Some(300))]).expect("2 回");
        assert_eq!(
            (two.count, two.took_ms, two.since_end_ms),
            (2, 100, Some(700))
        );
        // 記録より後の begin は数えない（純粋関数の境界）
        assert_eq!(sum(&[(1_001, Some(1_002))]), None);
    }

    #[test]
    fn 再構成は終わってから_1_回だけ_perf_log_の行になる() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut q = std::collections::VecDeque::new();
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 1, 1), ms(0));
        apply_reconfig(&mut q, &ev(ReconfigPhase::Begin, 6, 1), ms(0));
        apply_reconfig(
            &mut q,
            &ev(ReconfigPhase::End, 1, (1 << 9) | (1 << 5)),
            ms(71),
        );
        assert!(reconfig_reports_at(&mut q, ms(71)).is_empty(), "end の途中");
        assert!(
            reconfig_reports_at(&mut q, ms(71 + 299)).is_empty(),
            "settle の内"
        );
        let lines = reconfig_reports_at(&mut q, ms(71 + 300));
        assert_eq!(
            lines,
            vec![
                "画面構成の変更（macOS の画面再構成）: begin → end 71ms・画面 1,6・通知 begin 2 件 / end 1 件・\
                 変更 remove,disabled・コールバックはメインスレッド。#2666"
                    .to_string()
            ]
        );
        assert!(
            reconfig_reports_at(&mut q, ms(5_000)).is_empty(),
            "1 回だけ"
        );

        // end が来ないまま OPEN_MAX を過ぎたら 1 回（メインスレッド以外・合成も書く）
        let mut off = ev(ReconfigPhase::Begin, 1, 1);
        off.main_thread = false;
        off.synthetic = true;
        apply_reconfig(&mut q, &off, ms(6_000));
        assert!(reconfig_reports_at(&mut q, ms(6_000 + 9_999)).is_empty());
        let lines = reconfig_reports_at(&mut q, ms(16_000));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("begin から 10s 経っても end が来ない"),
            "{lines:?}"
        );
        assert!(
            lines[0].contains("コールバックはメインスレッド以外 1 件・検証の合成"),
            "{lines:?}"
        );
        assert!(reconfig_reports_at(&mut q, ms(30_000)).is_empty());
    }

    #[test]
    fn 変更の内容は名前で書き_知らない桁は_16_進で残す() {
        assert_eq!(reconfig_flags_text(0), "なし");
        assert_eq!(reconfig_flags_text(1), "なし", "begin の桁は書かない");
        assert_eq!(reconfig_flags_text((1 << 4) | (1 << 8)), "add,enabled");
        assert_eq!(
            reconfig_flags_text((1 << 12) | (1 << 20)),
            "desktop_shape,0x100000"
        );
    }

    #[test]
    fn 優先の決まり_計測区間で名指し再構成は併記_届かなければ再構成を名指す() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let lag = Duration::from_millis(1600);
        let r = || Some(overlap(71, Some(1545)));
        let ja = |c: &StallCause| c.text_in(tako_core::i18n::Lang::Ja);

        // 過負荷・継続中・専有は計測を名指したまま、再構成を併記する
        let over = StallCause::classify(lag, SCHED_OVERLOAD_OVER, None, None, r());
        let run = StallCause::classify(lag, Duration::ZERO, Some(("render", 900)), None, r());
        let owned = StallCause::classify(
            lag,
            Duration::ZERO,
            None,
            DelaySpans::summarize(spans_at(deadline, &[("render", 1600, 1700)]), deadline),
            r(),
        );
        for (c, says) in [
            (&over, "マシン全体の過負荷"),
            (&run, "render が 900ms 継続中 = tako の専有"),
            (&owned, "= tako の専有。#1968 / #2665"),
        ] {
            let text = ja(c);
            assert!(!c.names_reconfig(), "{c:?}");
            assert_eq!(c.reconfig(), r().as_ref());
            assert!(text.contains(says), "{text}");
            assert!(
                text.ends_with(
                    "。画面構成の変更とも重なる: begin → end 71ms・end から記録まで 1545ms。#2666）"
                ),
                "併記: {text}"
            );
        }
        assert!(run.names_tako() && owned.names_tako() && !over.names_tako());

        // 計測区間が半分に届かない: 残りを画面構成の変更の処理中と名指し、内訳も書く
        let partial = StallCause::classify(
            lag,
            Duration::ZERO,
            None,
            DelaySpans::summarize(spans_at(deadline, &[("render", 1600, 400)]), deadline),
            r(),
        );
        assert!(
            partial.names_reconfig() && !partial.names_tako(),
            "{partial:?}"
        );
        let text = ja(&partial);
        assert!(text.contains("計 400ms・1 件 = 遅延の 25%"), "{text}");
        assert!(
            text.contains("残り 1200ms は画面構成の変更の処理中（OS / GPUI 側）: begin → end 71ms"),
            "{text}"
        );
        assert!(!text.contains("tako の専有"), "{text}");

        // 計測区間なし: 画面構成の変更の処理中
        let none = StallCause::classify(lag, Duration::ZERO, None, None, r());
        assert_eq!(
            ja(&none),
            "UI ストール: foreground executor の再開が 1.60s 遅延（OS スレッドは正常・遅延の間に計測区間なし = \
             画面構成の変更の処理中（OS / GPUI 側）: begin → end 71ms・end から記録まで 1545ms。#2666）"
        );
        assert_eq!(
            none.text_in(tako_core::i18n::Lang::En),
            "UI stall: the foreground executor resumed 1.60s late (OS threads are fine; no measured \
             span during the delay = processing a display configuration change (OS / GPUI side): \
             begin → end 71ms, 1545ms from end to the record. #2666)"
        );

        // 再構成が無ければ #2665 の分類のまま（併記もしない）
        for (c, kind) in [
            (
                StallCause::classify(lag, Duration::ZERO, None, None, None),
                "外",
            ),
            (
                StallCause::classify(
                    lag,
                    Duration::ZERO,
                    None,
                    DelaySpans::summarize(spans_at(deadline, &[("render", 1600, 400)]), deadline),
                    None,
                ),
                "内訳",
            ),
        ] {
            assert!(
                !c.names_reconfig() && c.reconfig().is_none(),
                "{kind}: {c:?}"
            );
            assert!(!ja(&c).contains("画面構成"), "{kind}: {}", ja(&c));
        }
    }

    #[test]
    fn 再構成の注入は手を書いた順に読む() {
        assert_eq!(Issue2666Inject::parse(""), Issue2666Inject::default());
        assert_eq!(Issue2666Inject::default().displays, 2);
        let inj = Issue2666Inject::parse("begin sleep=1800 span=400 end displays=3 x=1 sleep=? \n");
        assert_eq!(
            inj,
            Issue2666Inject {
                steps: vec![
                    Issue2666Step::Begin,
                    Issue2666Step::Sleep(Duration::from_millis(1800)),
                    Issue2666Step::Span(Duration::from_millis(400)),
                    Issue2666Step::End,
                ],
                displays: 3,
            }
        );
        // 合成は画面ごと・書いた順
        let seen = std::cell::RefCell::new(Vec::new());
        Issue2666Inject::parse("begin sleep=1 end displays=2")
            .run(|phase, d| seen.borrow_mut().push((phase, d)));
        assert_eq!(
            seen.into_inner(),
            vec![
                (ReconfigPhase::Begin, 1),
                (ReconfigPhase::Begin, 2),
                (ReconfigPhase::End, 1),
                (ReconfigPhase::End, 2),
            ]
        );
        // 対象外の要求はファイルを読まない
        assert_eq!(
            Issue2666Inject::for_request(false),
            Issue2666Inject::default()
        );
    }

    #[test]
    fn 注入のファイルは外と中の_ms_を読む() {
        assert_eq!(Issue2665Inject::parse(""), Issue2665Inject::default());
        assert_eq!(
            Issue2665Inject::parse("out=1200 in=400\n"),
            Issue2665Inject {
                out_of_span: Some(Duration::from_millis(1200)),
                in_span: Some(Duration::from_millis(400)),
                after_span: None,
            }
        );
        assert_eq!(
            Issue2665Inject::parse("in=300 junk out=x after=900"),
            Issue2665Inject {
                out_of_span: None,
                in_span: Some(Duration::from_millis(300)),
                after_span: Some(Duration::from_millis(900)),
            }
        );
        // 対象外の要求はファイルを読まない
        assert_eq!(
            Issue2665Inject::for_request(false),
            Issue2665Inject::default()
        );
    }

    #[test]
    fn スケジューラ遅延のピークは取り出しでリセットされる() {
        // 他テストとの相互作用を避けるため、まず溜まっているぶんを捨てる
        let _ = take_scheduler_lag_peak();
        record_scheduler_lag(Duration::from_millis(120));
        record_scheduler_lag(Duration::from_millis(40)); // ピークは下がらない
        record_scheduler_lag(Duration::from_millis(300));
        assert_eq!(take_scheduler_lag_peak(), Duration::from_millis(300));
        // 取り出したら次の窓はゼロから始まる
        assert_eq!(take_scheduler_lag_peak(), Duration::ZERO);
    }

    #[test]
    fn perf_spanのネストで現在区間が内側優先になり復元される() {
        // 区間追跡はメインスレッドでだけ働く（#944）ので、このテストの間だけ登録する
        let _main = scoped_main_thread();
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
                    && line.as_bytes()[21..23] == *b"] "
                    && line.as_bytes()[23..].starts_with(b"thread"),
                "混線した行: {line:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 表記は**両プラットフォームぶんを macOS からも固定する**（#1073）。
    /// unix 側の書式は 1 文字も変えていない（過去のログと突き合わせられるように）
    #[test]
    fn 混み具合の表記は取得可否と指標の種類で決まる() {
        assert_eq!(
            format_machine_load(Some(MachineLoad::Average([6.171, 5.79, 5.4]))),
            "load=6.17/5.79/5.40"
        );
        assert_eq!(
            format_machine_load(Some(MachineLoad::CpuBusy {
                percent: 41.6,
                cpus: 12
            })),
            "load=cpu42%/12cpu"
        );
        assert_eq!(format_machine_load(None), "load=unknown");
    }

    /// **どの OS でも `load=unknown` が既定にならない**こと（#1073 の主目的）。
    /// Windows は境界が CPU 使用率を返すので、ここは cfg で分岐しない
    #[test]
    fn この環境では混み具合が採れる() {
        let shown = format_machine_load(machine_load());
        assert_ne!(shown, "load=unknown", "この OS で混み具合が採れていない");
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
