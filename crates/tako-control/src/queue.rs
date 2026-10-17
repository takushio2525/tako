//! 重い処理の順番待ちの調停役（FR-2.43.6 / Issue #1994。エピック #1992 の S2）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§4・§8.3）。判断そのものは
//! tako-core の純関数（`tako_core::queue::admit`）で、ここは状態（誰が待っていて誰が走っているか）と
//! IPC の受け答えだけを持つ。
//!
//! ## 形
//!
//! - **シングルトン**（プロセスに 1 つ）。本番の GUI が起動時に [`enable`] する。隔離の GUI
//!   （`TAKO_ISOLATED=1`）・セルフテスト・セカンダリは調停しない（[`role_for`]）。
//!   隔離ごとに別の待ち行列ができると「マシン全体で絞る」にならないため。
//!   テストは `TAKO_QUEUE_SOCKET` を**明示したときだけ**隔離の調停役を立てる
//!   （GUI 無しの `tako queue serve` か、その env を付けた隔離 GUI）
//! - 受け答えは `ipc::submit` が UI の受け口へ渡す**前に** [`answer`] で行う（UI スレッドを通らない。
//!   UI が固まってもビルドの割り当ては止まらない = #1979 の型を持ち込まない）
//! - **待ち行列の正本は各 wrapper**（`tako queue <コマンド>`）。毎回の poll が 1 件の全体
//!   （[`JobWire`]）を送る upsert なので、調停役が落ちて立て直しても、元の受付時刻（待ち）と
//!   子の pid（実行中）で表が組み直る。ディスクには持たない。立ち上げ直後の [`REGATHER`] の間は
//!   誰も入れない（後から戻ってきた古い待ちに追い越されないため）
//! - 調停役はプロセスを止めない。取り消しは印を付けるだけで、子を止めるのは wrapper 自身
//! - wrapper が消えたら（pid が居ない / 心拍が [`HEARTBEAT_TIMEOUT`] 途絶えた）次の受け答えで枠を返す
//!
//! 番犬: `crates/tako-control/tests/issue1993_resources_ui_thread_watchdog.rs`（受け口の順序）と
//! `crates/tako-control/tests/issue1994_queue_ui_stall.rs`（UI の受け口を誰も読まなくても割り当てが進む）と
//! `crates/tako-control/tests/issue2243_queue_head_of_line_watchdog.rs`（target dir 待ちの先頭が空き枠を
//! 塞がない・同じ dir の直列・追い越しの上限）と
//! `crates/tako-control/tests/issue2254_queue_reservation_watchdog.rs`（ピークを過ぎた実行中が見込みを
//! 押さえ続けない・見込みは過去の記録から・本当に空きが無ければ待つ）
//!
//! ## 見込みと過去の記録（#2254）
//!
//! 受付のときに wrapper が送る鍵（[`q::HistoryKeys`]）で過去の記録（[`crate::queue_history`]）から見込みを
//! 引き（[`q::estimate`]）、終わった処理の木の最大と最大になった時点を 1 行足す。実行中の「まだ使っていない
//! 見込み」はピークを過ぎたら 0（[`q::RunningJob::pending_mb`]）。待ちの上限で諦めた件は、そのとき枠と
//! メモリを押さえていた処理を `recent` に残す（`gave_up` / `holders`）

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use tako_core::queue::{
    self as q, Admission, AdmitInput, Estimate, HistoryRow, JobWire, LimitSettings, Limits,
    LimitsPatch, MachineView, RunningJob, WaitReason, WaitingJob,
};
use tako_core::resources::{Pressure, MIB};

use crate::dispatch::DispatchError;
use crate::protocol::Request;

/// 隔離の調停役の受け口（明示したときだけ立てる。wrapper もこれがあればここだけへ繋ぐ）
pub const ENV_SOCKET: &str = "TAKO_QUEUE_SOCKET";
/// 割り当てた子へ渡す受付番号。これがある中の `tako queue` は入れ子 = 待たずに走る
pub const ENV_JOB: &str = "TAKO_QUEUE_JOB";
/// 隔離の調停役だけが読む注入（機の値・外の重い処理の件数・停滞の窓。e2e 用。JSON ファイル）
pub const ENV_INJECT: &str = "TAKO_QUEUE_INJECT";

/// 注入: `ui-stall` なら GUI 無しの調停役が「順番待ち以外の要求」に答えない
/// （UI スレッドが固まった状態の再現。e2e が「それでも割り当ては進む」を確かめる）
pub const ENV_INJECT_UI: &str = "TAKO_1994_INJECT";
/// #2243 の A/B: `TAKO_2243_LEGACY=1` で修正前の割り当て（先頭が target dir 待ちでも後ろを追い越させない
/// = 先頭詰まり）。調停役のプロセス（GUI / `tako queue serve`）の env で効く
pub const ENV_LEGACY_2243: &str = "TAKO_2243_LEGACY";

fn legacy_2243() -> bool {
    std::env::var_os(ENV_LEGACY_2243).is_some_and(|v| v == "1")
}

/// #2254 の A/B: `TAKO_2254_LEGACY=1` で修正前の見込み（区分の既定だけ・過去の記録を引かない）と予約
/// （実行中は最後まで「見込み − 今の使用量」を押さえる）。調停役のプロセスの env で効く。
/// 記録は修正前でも足す（振る舞いは変えない）
pub const ENV_LEGACY_2254: &str = "TAKO_2254_LEGACY";

fn legacy_2254() -> bool {
    std::env::var_os(ENV_LEGACY_2254).is_some_and(|v| v == "1")
}

/// 立ち上げ直後に誰も入れない長さ（wrapper の繋ぎ直しの間隔は最大 5 秒）
pub const REGATHER: Duration = Duration::from_secs(6);
/// 心拍がこれだけ途絶えた件は外す（pid の生存の判定の裏打ち。ゾンビは生きて見えるため）
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);
/// 順番待ちに件がある間、走査を 2 秒ごとに保つ長さ（受け答えのたびに延ばす）
const KEEP_SCANNING: Duration = Duration::from_secs(10);
/// 設定ファイルの mtime を見直す間隔
const SETTINGS_RECHECK: Duration = Duration::from_secs(2);
/// 終わった件を status に残す数
const RECENT_KEEP: usize = 10;

/// 調停役でないプロセスが wrapper の操作を受けたときの答え（wrapper はこれでそのまま走る）
pub const NOT_ARBITER: &str = "この tako は順番待ちの調停役ではない";

/// 調停役の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 本番の GUI（マシンに 1 つ）
    Production,
    /// `TAKO_QUEUE_SOCKET` を明示して立てた隔離の調停役（注入 `TAKO_QUEUE_INJECT` を読む）
    Explicit,
}

/// このプロセスが調停役になるか（純関数）。
///
/// `TAKO_QUEUE_SOCKET` の明示が最優先（隔離の検証）。それ以外は本番の GUI だけ:
/// 隔離（`TAKO_ISOLATED=1`）・セルフテスト・セカンダリ（多重起動の後発）は調停しない
pub fn role_for(
    explicit_socket: Option<&str>,
    isolated: bool,
    self_test: bool,
    secondary: bool,
) -> Option<Role> {
    if explicit_socket.is_some_and(|s| !s.is_empty()) {
        return Some(Role::Explicit);
    }
    if isolated || self_test || secondary {
        return None;
    }
    Some(Role::Production)
}

/// 調停役の受け口を明示した値（空は未設定と同じ）
pub fn explicit_socket() -> Option<String> {
    std::env::var(ENV_SOCKET).ok().filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    /// 割り当てた（wrapper が子を起こす前）。枠は持っている
    Granted,
    Running,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Waiting => "waiting",
            Phase::Granted => "granted",
            Phase::Running => "running",
        }
    }
}

#[derive(Debug, Clone)]
struct Job {
    wire: JobWire,
    phase: Phase,
    last_seen: Instant,
    cancel: bool,
    /// 見込み（受付のときに過去の記録から決める。#2254）
    estimate: Estimate,
    used_mb: Option<u64>,
    peak_mb: u64,
    /// 最大を最後に更新した時刻（Unix ミリ秒。走り出してからの秒数を出すので wrapper の開始と同じ物差し）
    peak_raised_ms: Option<u64>,
    cpu_percent: Option<f64>,
    /// 木の CPU が下限未満になった時刻（調停役の単調ミリ秒）
    low_since: Option<u64>,
    stalled: bool,
    share: u32,
    granted_at_ms: Option<u64>,
    /// 待っている間に後ろから追い越された件数（#2243。上限は `q::max_overtaken`）
    overtaken: u32,
}

impl Job {
    fn new(wire: JobWire, now: Instant, history: &[HistoryRow]) -> Self {
        let phase = if wire.child_pid.is_some() {
            Phase::Running
        } else {
            Phase::Waiting
        };
        let estimate = if legacy_2254() {
            Estimate::default_for(wire.kind)
        } else {
            q::estimate(wire.kind, wire.history.as_ref(), history)
        };
        Self {
            estimate,
            granted_at_ms: wire.started_at_ms,
            wire,
            phase,
            last_seen: now,
            cancel: false,
            used_mb: None,
            peak_mb: 0,
            peak_raised_ms: None,
            cpu_percent: None,
            low_since: None,
            stalled: false,
            share: 0,
            overtaken: 0,
        }
    }

    /// 枠を持っているか（割り当て済み・実行中）
    fn holds_slot(&self) -> bool {
        matches!(self.phase, Phase::Granted | Phase::Running)
    }

    /// 走り出した時刻（Unix ミリ秒。子が起きる前は割り当ての時刻）
    fn started_ms(&self) -> Option<u64> {
        self.wire.started_at_ms.or(self.granted_at_ms)
    }

    /// 割り当ての判断の入力の形（`wall` = いまの Unix ミリ秒）
    fn running_view(&self, wall: u64) -> RunningJob {
        let start = self.started_ms();
        let secs_since = |ms: u64| start.map_or(0, |s| ms.saturating_sub(s) / 1000);
        RunningJob {
            id: self.wire.id,
            estimate_mb: self.estimate.mb,
            used_mb: self.used_mb,
            target_dir: self.wire.target_dir.clone(),
            stalled: self.stalled,
            peak_mb: self.peak_mb,
            ran_s: secs_since(wall),
            peak_seen_s: self.peak_raised_ms.map(secs_since),
            expected_peak_at_s: self.estimate.peak_at_s,
        }
    }

    /// 終わった処理の過去の記録の 1 行（鍵が無い = 旧い wrapper・走査の前に終わった = 最大 0 は残さない）
    fn history_row(&self, exit_code: Option<i32>, wall: u64) -> Option<HistoryRow> {
        let keys = self.wire.history.as_ref().filter(|k| !k.key.is_empty())?;
        if self.peak_mb == 0 || self.phase != Phase::Running {
            return None;
        }
        let view = self.running_view(wall);
        Some(HistoryRow {
            at: wall / 1000,
            key: keys.key.clone(),
            shape: keys.shape.clone(),
            name: keys.name.clone(),
            kind: self.wire.kind,
            peak_mb: self.peak_mb,
            peak_at_s: view.peak_seen_s,
            ran_s: view.ran_s,
            exit_code,
        })
    }
}

#[derive(Default)]
struct Arbiter {
    role: Option<Role>,
    enabled_at: Option<Instant>,
    jobs: BTreeMap<u64, Job>,
    recent: VecDeque<Value>,
    settings: LimitSettings,
    /// 設定を最後に見た時刻と、そのときの mtime
    settings_seen: Option<(Instant, Option<SystemTime>)>,
    /// 最後に取り込んだ走査の時刻
    usage_at: Option<Instant>,
    unmanaged: u32,
    /// 直近の判断（先頭が待った理由・待ちそれぞれの理由・並びを止めた理由）
    last: Admission,
    /// GUI 無しで立てた（`tako queue serve`）
    headless: bool,
    /// 機の値と論理 CPU 数を固定し、走査の覚えを読まない（**単体テストだけが立てる**。本番は常に
    /// `None`。機の実際の状態で判断が揺れないように）
    fixed_machine: Option<(Option<MachineView>, u32)>,
    /// 過去の記録（古い順。#2254）
    history: Vec<HistoryRow>,
    /// 記録の置き場（`<data_dir>/resources`。`None` = 足さない = 単体テスト・data dir が決まらない）
    history_dir: Option<PathBuf>,
}

fn arbiter() -> &'static Mutex<Arbiter> {
    static A: OnceLock<Mutex<Arbiter>> = OnceLock::new();
    A.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, Arbiter> {
    arbiter().lock().unwrap_or_else(|p| p.into_inner())
}

/// 調停役の単調ミリ秒（停滞の窓に使う。壁時計の巻き戻りに影響されない）
fn mono_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// 壁時計の Unix ミリ秒（受付・開始の時刻。wrapper と同じ物差し）
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 調停役として受け付けを始める（GUI の起動時 / `tako queue serve`）。2 回目以降は役だけ差し替える
pub fn enable(role: Role) {
    let mut a = lock();
    if a.role.is_none() {
        a.enabled_at = Some(Instant::now());
        // 過去の記録は最初の 1 回だけ読む（以後は足した行をメモリにも積む）
        a.history_dir = crate::queue_history::dir();
        a.history = a
            .history_dir
            .as_deref()
            .map(crate::queue_history::load)
            .unwrap_or_default();
    }
    a.role = Some(role);
    a.settings = crate::settings::load().resources;
    a.settings_seen = Some((Instant::now(), settings_mtime()));
}

/// いまの役（調停役でなければ `None`）
pub fn role() -> Option<Role> {
    lock().role
}

fn settings_mtime() -> Option<SystemTime> {
    crate::settings::settings_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// 設定ファイルが手で書き換えられていたら読み直す（[`SETTINGS_RECHECK`] ごと）
fn reload_settings_if_changed(a: &mut Arbiter, now: Instant) {
    let due = a
        .settings_seen
        .is_none_or(|(at, _)| now.duration_since(at) >= SETTINGS_RECHECK);
    if !due {
        return;
    }
    let mtime = settings_mtime();
    let changed = a.settings_seen.is_none_or(|(_, seen)| seen != mtime);
    if changed {
        a.settings = crate::settings::load().resources;
    }
    a.settings_seen = Some((now, mtime));
}

// ===== 注入（隔離の調停役だけ）=====

/// `TAKO_QUEUE_INJECT` の中身。**機の値は注入で与え、負荷は生成しない**（e2e の約束）
#[derive(Debug, Default, Deserialize)]
struct Inject {
    total_mb: Option<u64>,
    available_mb: Option<u64>,
    pressure: Option<String>,
    swap_used_mb: Option<u64>,
    cpus: Option<u32>,
    /// tako の外の重い処理の件数（走査の値を置き換える）
    unmanaged: Option<u32>,
    /// 停滞の窓（秒）
    stall_secs: Option<u64>,
    /// 立ち上げ直後に誰も入れない長さ（秒）
    regather_secs: Option<u64>,
    /// 実行中の木の使用量・最大（走査の値を置き換える。表示名に `label` を含む件。#2254）
    #[serde(default)]
    jobs: Vec<InjectJob>,
}

/// 注入: 実行中の 1 件の木の値（MiB）
#[derive(Debug, Default, Deserialize)]
struct InjectJob {
    label: String,
    used_mb: Option<u64>,
    peak_mb: Option<u64>,
}

fn inject(role: Option<Role>) -> Option<Inject> {
    if role != Some(Role::Explicit) {
        return None;
    }
    let path = std::env::var(ENV_INJECT).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

impl Inject {
    fn has_machine(&self) -> bool {
        self.total_mb.is_some()
            || self.available_mb.is_some()
            || self.pressure.is_some()
            || self.swap_used_mb.is_some()
    }

    fn machine(&self) -> MachineView {
        MachineView {
            total_mb: self.total_mb.unwrap_or(0),
            available_mb: self.available_mb,
            pressure: self.pressure.as_deref().and_then(|p| match p {
                "normal" => Some(Pressure::Normal),
                "warn" => Some(Pressure::Warn),
                "critical" => Some(Pressure::Critical),
                _ => None,
            }),
            swap_used_mb: self.swap_used_mb,
        }
    }
}

/// 機の値（MiB）と論理 CPU 数。採れなければ機の値は `None`
fn machine(a: &Arbiter, inj: Option<&Inject>) -> (Option<MachineView>, u32) {
    if let Some(fixed) = a.fixed_machine {
        return fixed;
    }
    let fallback_cpus = || {
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1)
    };
    if let Some(i) = inj.filter(|i| i.has_machine()) {
        return (Some(i.machine()), i.cpus.unwrap_or_else(fallback_cpus));
    }
    let Some(stats) = crate::resources::machine_stats() else {
        return (None, inj.and_then(|i| i.cpus).unwrap_or_else(fallback_cpus));
    };
    let cpus = inj
        .and_then(|i| i.cpus)
        .or((stats.cpus > 0).then_some(stats.cpus))
        .unwrap_or_else(fallback_cpus);
    let view = stats.memory.map(|m| MachineView {
        total_mb: m.total / MIB,
        available_mb: Some(m.available / MIB),
        pressure: stats.pressure,
        swap_used_mb: stats.swap.map(|(used, _)| used / MIB),
    });
    (view, cpus)
}

// ===== 受け答え =====

/// IPC / MCP の `Request::Queue` に答える（**接続のスレッド**。`ipc::submit` の手前）。
/// status は機とプロセスの覚えが古ければこのスレッドで採り直す（S1 の `resources::answer`）
pub fn answer(request: &Request) -> Result<Value, DispatchError> {
    let Request::Queue {
        action,
        job,
        id,
        limits,
    } = request
    else {
        return Err(DispatchError::InvalidParams(
            "順番待ちの要求ではない".into(),
        ));
    };
    match action.as_deref().unwrap_or("status") {
        "status" => Ok(with_section(crate::resources::answer(None)?)),
        other => {
            // 割り当ての判断に使う機の値だけは新しくしておく（8 µs。走査はしない）
            crate::resources::refresh_machine_if_stale();
            handle(other, job.as_ref(), *id, limits.as_ref())
        }
    }
}

/// status の JSON へ順番待ちの節を足す（採らない。UI スレッドから呼んでよい）
pub fn with_section(mut status: Value) -> Value {
    status["queue"] = status_section();
    status
}

/// 順番待ちの操作（status 以外）。**採らない**（走査の覚えと機の値の覚えを読むだけ）
pub fn handle(
    action: &str,
    job: Option<&JobWire>,
    id: Option<u64>,
    limits: Option<&LimitsPatch>,
) -> Result<Value, DispatchError> {
    match action {
        "limits" => set_limits(limits.cloned().unwrap_or_default()),
        "cancel" => {
            let id = id.ok_or_else(|| {
                DispatchError::InvalidParams("cancel には受付番号（id）が要る".into())
            })?;
            cancel(id)
        }
        "enqueue" | "poll" | "started" => {
            let job = job
                .ok_or_else(|| DispatchError::InvalidParams(format!("{action} には job が要る")))?;
            poll(job, Instant::now())
        }
        "done" => {
            let job =
                job.ok_or_else(|| DispatchError::InvalidParams("done には job が要る".into()))?;
            done(job, Instant::now())
        }
        other => Err(DispatchError::InvalidParams(format!(
            "未知の action: {other}（使えるのは status / cancel / limits）"
        ))),
    }
}

fn not_arbiter(a: &Arbiter) -> Result<(), DispatchError> {
    if a.role.is_some() {
        Ok(())
    } else {
        Err(DispatchError::Operation(format!(
            "{NOT_ARBITER}（隔離 / セルフテスト / セカンダリの tako。順番待ちはせずに走らせてよい）"
        )))
    }
}

/// 居なくなった wrapper の件を外す（pid が居ない / 心拍が途絶えた）
fn prune(a: &mut Arbiter, now: Instant) {
    let gone: Vec<u64> = a
        .jobs
        .iter()
        .filter(|(id, job)| {
            let pid_gone = u32::try_from(**id)
                .map(|pid| !tako_core::platform::process::pid_alive(pid))
                .unwrap_or(true);
            pid_gone || now.duration_since(job.last_seen) > HEARTBEAT_TIMEOUT
        })
        .map(|(id, _)| *id)
        .collect();
    for id in gone {
        if let Some(job) = a.jobs.remove(&id) {
            if job.holds_slot() {
                push_recent(a, &job, None, true);
            }
        }
    }
}

fn push_recent(a: &mut Arbiter, job: &Job, exit_code: Option<i32>, lost: bool) {
    let now = unix_ms();
    let ran = job
        .wire
        .started_at_ms
        .or(job.granted_at_ms)
        .map(|s| now.saturating_sub(s) / 1000);
    let waited = job
        .granted_at_ms
        .map(|g| g.saturating_sub(job.wire.enqueued_at_ms) / 1000);
    a.recent.push_front(json!({
        "id": job.wire.id,
        "label": job.wire.label,
        "kind": job.wire.kind.as_str(),
        "exit_code": exit_code,
        "lost": lost,
        "ran_s": ran,
        "waited_s": waited,
        "peak_mb": (job.peak_mb > 0).then_some(job.peak_mb),
    }));
    a.recent.truncate(RECENT_KEEP);
}

/// 走査の覚えから実行中の木の消費・停滞・tako の外の件数を取り込む（新しい走査が来たときだけ）
fn refresh_usage(a: &mut Arbiter, inj: Option<&Inject>, now: Instant) {
    if a.fixed_machine.is_some() {
        return;
    }
    if !a.jobs.is_empty() {
        crate::resources::keep_scanning(now + KEEP_SCANNING);
    }
    let pairs: Vec<(u64, u32)> = a
        .jobs
        .keys()
        .filter_map(|id| u32::try_from(*id).ok().map(|pid| (*id, pid)))
        .collect();
    if let Some((at, view)) = crate::resources::managed(&pairs, a.usage_at) {
        a.usage_at = Some(at);
        a.unmanaged = q::count_unmanaged(&view.unmanaged);
        let window_ms = inj
            .and_then(|i| i.stall_secs)
            .unwrap_or(q::STALL_AFTER_SECS)
            * 1000;
        let t = mono_ms();
        let wall = unix_ms();
        for (id, job) in a.jobs.iter_mut() {
            if !job.holds_slot() {
                continue;
            }
            let usage = view.jobs.get(id).copied();
            job.used_mb = usage.map(|u| u.memory / MIB);
            raise_peak(job, job.used_mb.unwrap_or(0), wall);
            job.cpu_percent = usage.and_then(|u| u.cpu_percent);
            if job.phase == Phase::Running {
                job.low_since = q::track_low_cpu(job.low_since, job.cpu_percent, t);
                job.stalled = q::is_stalled(job.low_since, t, window_ms);
            }
        }
    }
    if let Some(n) = inj.and_then(|i| i.unmanaged) {
        a.unmanaged = n;
    }
    // 実行中の木の値の注入（#2254 の e2e。負荷は生成しない）
    let wall = unix_ms();
    for ij in inj.map(|i| i.jobs.as_slice()).unwrap_or_default() {
        for job in a.jobs.values_mut() {
            if !job.holds_slot() || !job.wire.label.contains(&ij.label) {
                continue;
            }
            if let Some(used) = ij.used_mb {
                job.used_mb = Some(used);
            }
            raise_peak(
                job,
                ij.peak_mb.unwrap_or(0).max(job.used_mb.unwrap_or(0)),
                wall,
            );
        }
    }
}

/// 最大を更新する（伸びたときだけ時刻を残す = ピークを過ぎたかの判定に使う。#2254）
fn raise_peak(job: &mut Job, mb: u64, wall: u64) {
    if mb > job.peak_mb {
        job.peak_mb = mb;
        job.peak_raised_ms = Some(wall);
    }
}

/// 割り当てを判断して、入れた件を割り当て済みにする
fn decide(a: &mut Arbiter, inj: Option<&Inject>, now: Instant) {
    let (machine_view, cpus) = machine(a, inj);
    let limits: Limits = q::effective_limits(
        &a.settings,
        machine_view.map(|m| m.total_mb).filter(|t| *t > 0),
    );
    let wall = unix_ms();
    let running: Vec<RunningJob> = a
        .jobs
        .values()
        .filter(|j| j.holds_slot())
        .map(|j| j.running_view(wall))
        .collect();
    let waiting = waiting_jobs(a);
    let regather = inj
        .and_then(|i| i.regather_secs)
        .map(Duration::from_secs)
        .unwrap_or(REGATHER);
    let regathering = a
        .enabled_at
        .is_some_and(|at| now.duration_since(at) < regather);
    let admission = q::admit(&AdmitInput {
        machine: machine_view,
        limits,
        running: &running,
        waiting: &waiting,
        unmanaged: a.unmanaged,
        regathering,
        overtake: !legacy_2243(),
        release_after_peak: !legacy_2254(),
    });
    let mut active = running.iter().filter(|r| !r.stalled).count();
    let granted_at = unix_ms();
    for id in &admission.grants {
        active += 1;
        if let Some(job) = a.jobs.get_mut(id) {
            job.phase = Phase::Granted;
            job.share = q::share(cpus, active);
            job.granted_at_ms = Some(granted_at);
        }
    }
    for (id, n) in &admission.overtaken {
        if let Some(job) = a.jobs.get_mut(id) {
            job.overtaken = job.overtaken.saturating_add(*n);
        }
    }
    a.last = admission;
}

fn current_limits(a: &Arbiter, inj: Option<&Inject>) -> Limits {
    let (machine_view, _) = machine(a, inj);
    q::effective_limits(
        &a.settings,
        machine_view.map(|m| m.total_mb).filter(|t| *t > 0),
    )
}

/// 待ち（取り消した件を除く）を判断の入力の形へ
fn waiting_jobs(a: &Arbiter) -> Vec<WaitingJob> {
    a.jobs
        .values()
        .filter(|j| j.phase == Phase::Waiting && !j.cancel)
        .map(|j| WaitingJob {
            id: j.wire.id,
            estimate_mb: j.estimate.mb,
            target_dir: j.wire.target_dir.clone(),
            priority: j.wire.priority,
            enqueued_at_ms: j.wire.enqueued_at_ms,
            overtaken: j.overtaken,
        })
        .collect()
}

/// 待ちの並び（受付番号）
fn waiting_order(a: &Arbiter) -> Vec<u64> {
    q::order_waiting(&waiting_jobs(a))
        .iter()
        .map(|w| w.id)
        .collect()
}

/// 待ちの 1 件が待つ理由の 1 文（status 用）。並びが止まって調べなかった件は止めている理由を添える
fn waiting_text(last: &Admission, id: u64) -> Option<String> {
    let own = last.reason_of(id)?;
    match (own, last.holding_of(id)) {
        (WaitReason::Ahead, Some(hold)) if *hold != WaitReason::Ahead => {
            Some(format!("{}（{}）", own.text(), hold.text()))
        }
        _ => Some(own.text()),
    }
}

/// wrapper の poll（受付・待ちの心拍・割り当て後の開始・実行中の心拍を兼ねる upsert）
fn poll(wire: &JobWire, now: Instant) -> Result<Value, DispatchError> {
    let mut a = lock();
    not_arbiter(&a)?;
    let inj = inject(a.role);
    reload_settings_if_changed(&mut a, now);
    prune(&mut a, now);
    let id = wire.id;
    match a.jobs.get_mut(&id) {
        Some(job) => {
            job.granted_at_ms = job.granted_at_ms.or(wire.started_at_ms);
            job.wire = wire.clone();
            job.last_seen = now;
            if wire.child_pid.is_some() {
                job.phase = Phase::Running;
            }
        }
        None => {
            let job = Job::new(wire.clone(), now, &a.history);
            a.jobs.insert(id, job);
        }
    }
    refresh_usage(&mut a, inj.as_ref(), now);
    let job = a.jobs.get(&id).cloned().expect("直前に入れた");
    if job.cancel && job.phase == Phase::Waiting {
        a.jobs.remove(&id);
        return Ok(json!({ "state": "cancelled" }));
    }
    match job.phase {
        Phase::Running => Ok(json!({
            "state": "running",
            "cancel": job.cancel,
            "stalled": job.stalled,
        })),
        Phase::Granted => Ok(granted_json(&a, &job, inj.as_ref())),
        Phase::Waiting => {
            decide(&mut a, inj.as_ref(), now);
            let job = a.jobs.get(&id).cloned().expect("待ちのまま居る");
            if job.phase == Phase::Granted {
                return Ok(granted_json(&a, &job, inj.as_ref()));
            }
            let order = waiting_order(&a);
            let position = order.iter().position(|x| *x == id).map_or(0, |p| p + 1);
            let limits = current_limits(&a, inj.as_ref());
            let running = a.jobs.values().filter(|j| j.holds_slot()).count();
            // 自分の理由（調べる前に並びが止まった件は Ahead）と、自分を実際に止めているもの
            // （Ahead なら並びを止めた理由）。`head_*` は旧い wrapper が 2 番目以降で読む欄
            let reason = a.last.reason_of(id).cloned().unwrap_or(WaitReason::Ahead);
            let head = a.last.holding_of(id).cloned();
            Ok(json!({
                "state": "waiting",
                "position": position,
                "waiting": order.len(),
                "running": running,
                "max_jobs": limits.max_jobs,
                "unmanaged": a.unmanaged,
                "reason": reason.code(),
                "text": reason.text(),
                "head_reason": head.as_ref().map(WaitReason::code),
                "head_text": head.as_ref().map(WaitReason::text),
                "max_wait_ms": u64::from(limits.max_wait_min) * 60_000,
                "estimate_mb": job.estimate.mb,
                // 枠とメモリを押さえている処理（諦めたときに wrapper が出す。#2254）
                "holders": holders(&a),
            }))
        }
    }
}

fn granted_json(a: &Arbiter, job: &Job, inj: Option<&Inject>) -> Value {
    let limits = current_limits(a, inj);
    json!({
        "state": "granted",
        "share": job.share.max(1),
        "waited_ms": job
            .granted_at_ms
            .map(|g| g.saturating_sub(job.wire.enqueued_at_ms)),
        "max_wait_ms": u64::from(limits.max_wait_min) * 60_000,
    })
}

/// wrapper の done（子が終わった / 待ちから外れた）
fn done(wire: &JobWire, now: Instant) -> Result<Value, DispatchError> {
    let mut a = lock();
    not_arbiter(&a)?;
    let inj = inject(a.role);
    refresh_usage(&mut a, inj.as_ref(), now);
    let Some(job) = a.jobs.remove(&wire.id) else {
        // 調停役の立て直しの後で登録し直す前に終わった件。知らないので何もしない
        return Ok(json!({ "state": "done", "known": false }));
    };
    let peak = job.peak_mb;
    let mut history_write = None;
    if job.holds_slot() {
        push_recent(&mut a, &job, wire.exit_code, false);
        if let Some(row) = job.history_row(wire.exit_code, unix_ms()) {
            a.history.push(row.clone());
            crate::queue_history::trim(&mut a.history);
            history_write = a.history_dir.clone().map(|dir| (dir, row));
        }
    } else if wire.exit_code == Some(q::EXIT_MAX_WAIT) {
        // 待ちの上限で諦めた: そのとき枠とメモリを押さえていた処理を残す（#2254）
        let reason = a.last.holding_of(job.wire.id).cloned();
        let holders = holders(&a);
        push_gave_up(&mut a, &job, reason.as_ref(), holders);
    }
    prune(&mut a, now);
    drop(a);
    // 記録の追記はロックの外で（ファイルの書き込みを他の受け答えに待たせない）
    if let Some((dir, row)) = history_write {
        if let Err(e) = crate::queue_history::append(&dir, &row) {
            crate::diag::persist_log(&format!("[queue] 過去の記録を足せない（{e}）"));
        }
    }
    Ok(json!({
        "state": "done",
        "known": true,
        "peak_mb": (peak > 0).then_some(peak),
    }))
}

/// 枠とメモリを押さえている処理（実行中・割り当て済み。まだ使っていない見込みの多い順。#2254）
fn holders(a: &Arbiter) -> Vec<Value> {
    let wall = unix_ms();
    let release = !legacy_2254();
    let mut rows: Vec<(u64, Value)> = a
        .jobs
        .values()
        .filter(|j| j.holds_slot())
        .map(|j| {
            let view = j.running_view(wall);
            let pending = if view.stalled {
                0
            } else {
                view.pending_mb(release)
            };
            (
                pending,
                json!({
                    "id": j.wire.id,
                    "label": j.wire.label,
                    "kind": j.wire.kind.as_str(),
                    "pane": j.wire.pane,
                    "memory_mb": j.used_mb,
                    "peak_mb": (j.peak_mb > 0).then_some(j.peak_mb),
                    "estimate_mb": j.estimate.mb,
                    "estimate_from": j.estimate.from.as_str(),
                    "pending_mb": pending,
                    "peak_passed": view.peak_passed(),
                    "stalled": j.stalled,
                }),
            )
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    rows.into_iter().map(|(_, v)| v).collect()
}

/// 待ちの上限で諦めた件を `recent` に残す（#2254）
fn push_gave_up(a: &mut Arbiter, job: &Job, reason: Option<&WaitReason>, holders: Vec<Value>) {
    a.recent.push_front(json!({
        "id": job.wire.id,
        "label": job.wire.label,
        "kind": job.wire.kind.as_str(),
        "exit_code": q::EXIT_MAX_WAIT,
        "lost": false,
        "gave_up": true,
        "waited_s": unix_ms().saturating_sub(job.wire.enqueued_at_ms) / 1000,
        "estimate_mb": job.estimate.mb,
        "reason": reason.map(WaitReason::code),
        "text": reason.map(WaitReason::text),
        "holders": holders,
    }));
    a.recent.truncate(RECENT_KEEP);
}

/// 取り消し（待ち = 次の poll で wrapper が 125 で終わる / 実行中 = 次の心拍で wrapper が子を止める）
fn cancel(id: u64) -> Result<Value, DispatchError> {
    let mut a = lock();
    not_arbiter(&a)?;
    prune(&mut a, Instant::now());
    let Some(job) = a.jobs.get_mut(&id) else {
        return Err(DispatchError::InvalidParams(format!(
            "受付番号 {id} は順番待ちに居ない（`tako queue` で一覧）"
        )));
    };
    job.cancel = true;
    Ok(json!({
        "id": id,
        "label": job.wire.label,
        "phase": job.phase.as_str(),
        "cancelled": true,
    }))
}

fn gb_value(mb: u64) -> f64 {
    (mb as f64 / 1024.0 * 10.0).round() / 10.0
}

fn limits_json(limits: &Limits, configured: &LimitSettings) -> Value {
    json!({
        "max_jobs": limits.max_jobs,
        "reserve_gb": gb_value(limits.reserve_mb),
        "swap_guard_gb": gb_value(limits.swap_guard_mb),
        "max_wait_min": limits.max_wait_min,
        "configured": configured,
    })
}

/// 上限の読み書き（`settings.json` の `resources` 節）。空の変更は読むだけ
fn set_limits(patch: LimitsPatch) -> Result<Value, DispatchError> {
    let mut settings = crate::settings::load();
    if !patch.is_empty() {
        settings
            .resources
            .apply(&patch)
            .map_err(DispatchError::InvalidParams)?;
        crate::settings::save(&settings)
            .map_err(|e| DispatchError::Operation(format!("設定を保存できない: {e}")))?;
    }
    let mut a = lock();
    a.settings = settings.resources.clone();
    a.settings_seen = Some((Instant::now(), settings_mtime()));
    let inj = inject(a.role);
    let limits = current_limits(&a, inj.as_ref());
    let mut v = limits_json(&limits, &a.settings);
    v["changed"] = json!(!patch.is_empty());
    Ok(v)
}

/// GUI が居ないときの上限の読み書き（CLI のプロセスで設定ファイルを直に変える。
/// 調停役は次に mtime を見て読み直す）
pub fn limits_offline(patch: LimitsPatch) -> Result<Value, DispatchError> {
    crate::resources::refresh_machine_if_stale();
    set_limits(patch)
}

/// 隔離の調停役を GUI 無しで立てて、殺されるまで受け付ける（`tako queue serve`。検証用）。
///
/// 受け口は [`crate::ipc::IpcServer::start_at`]（明示したパス）。順番待ちの要求は
/// `ipc::submit` の手前で答え、それ以外は「GUI が無い」で断る（UI の代役）。
/// `TAKO_1994_INJECT=ui-stall` なら代役は**答えない**（UI スレッドが固まった状態）
pub fn serve_headless(socket: &str, token: String) -> std::io::Result<()> {
    use futures::StreamExt;
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _server = crate::ipc::IpcServer::start_at(tx, token, socket)?;
    crate::resources::start();
    enable(Role::Explicit);
    lock().headless = true;
    let stall = std::env::var(ENV_INJECT_UI).is_ok_and(|v| v == "ui-stall");
    eprintln!(
        "tako queue serve: 隔離の調停役を立てた（GUI 無し{}）",
        if stall {
            "・UI の代役は答えない"
        } else {
            ""
        }
    );
    let mut held = Vec::new();
    while let Some(incoming) = futures::executor::block_on(rx.next()) {
        if stall {
            // 返事をしないまま持ち続ける（接続のスレッドは待ち続ける = 固まった UI と同じ）
            held.push(incoming);
            continue;
        }
        let _ = incoming.reply.send(Err(DispatchError::Operation(
            "GUI が無い（順番待ちの調停役だけ = tako queue serve）".into(),
        )));
    }
    Ok(())
}

/// status の `queue` 節（採らない）
pub fn status_section() -> Value {
    let mut a = lock();
    let Some(role) = a.role else {
        return json!({
            "arbiter": false,
            "reason": NOT_ARBITER,
        });
    };
    let now = Instant::now();
    reload_settings_if_changed(&mut a, now);
    prune(&mut a, now);
    let inj = inject(a.role);
    let limits = current_limits(&a, inj.as_ref());
    let wall = unix_ms();
    let release = !legacy_2254();
    let running: Vec<Value> = a
        .jobs
        .values()
        .filter(|j| j.holds_slot())
        .map(|j| {
            let view = j.running_view(wall);
            let pending = if view.stalled {
                0
            } else {
                view.pending_mb(release)
            };
            json!({
                "id": j.wire.id,
                "label": j.wire.label,
                "kind": j.wire.kind.as_str(),
                "priority": j.wire.priority.as_str(),
                "pane": j.wire.pane,
                "phase": j.phase.as_str(),
                "share": j.share,
                "memory_mb": j.used_mb,
                "peak_mb": (j.peak_mb > 0).then_some(j.peak_mb),
                // 見込みとまだ使っていない見込み（後ろの空きメモリの判定で押さえている分。#2254）
                "estimate_mb": j.estimate.mb,
                "estimate_from": j.estimate.from.as_str(),
                "pending_mb": pending,
                "peak_passed": view.peak_passed(),
                "cpu_percent": j.cpu_percent.map(|c| (c * 10.0).round() / 10.0),
                "stalled": j.stalled,
                "cancel": j.cancel,
                "ran_s": j
                    .wire
                    .started_at_ms
                    .or(j.granted_at_ms)
                    .map(|s| wall.saturating_sub(s) / 1000),
                "waited_s": j
                    .granted_at_ms
                    .map(|g| g.saturating_sub(j.wire.enqueued_at_ms) / 1000),
            })
        })
        .collect();
    let order = waiting_order(&a);
    let waiting: Vec<Value> = order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| a.jobs.get(id).map(|j| (i, j)))
        .map(|(i, j)| {
            json!({
                "id": j.wire.id,
                "position": i + 1,
                "label": j.wire.label,
                "kind": j.wire.kind.as_str(),
                "priority": j.wire.priority.as_str(),
                "pane": j.wire.pane,
                "waited_s": wall.saturating_sub(j.wire.enqueued_at_ms) / 1000,
                "reason": a.last.reason_of(j.wire.id).map(WaitReason::code),
                "text": waiting_text(&a.last, j.wire.id),
                "overtaken": j.overtaken,
                "estimate_mb": j.estimate.mb,
                "estimate_from": j.estimate.from.as_str(),
            })
        })
        .collect();
    json!({
        "arbiter": true,
        "role": match role {
            Role::Production => "production",
            Role::Explicit => "explicit",
        },
        "headless": a.headless,
        "limits": limits_json(&limits, &a.settings),
        "running": running,
        "waiting": waiting,
        "blocked": a.last.blocked.as_ref().filter(|_| !order.is_empty()).map(|b| json!({
            "reason": b.code(),
            "text": b.text(),
        })),
        "unmanaged": a.unmanaged,
        "recent": a.recent.iter().cloned().collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::queue::JobKind;

    /// 状態を空へ戻す
    fn reset_for_test(role: Option<Role>) {
        let mut a = lock();
        *a = Arbiter::default();
        a.role = role;
        // 立ち上げ直後の待ちを飛ばす（テストは別に確かめる）
        a.enabled_at = None;
        // 機の値は「採れない」（同時数だけで決める）・論理 CPU は 8 に固定
        a.fixed_machine = Some((None, 8));
    }

    /// 調停役はプロセスに 1 つの状態なので、テストは直列に回す
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static M: Mutex<()> = Mutex::new(());
        M.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 生きている pid（眠るだけの子）を受付番号にする（Windows は ping で眠る）
    fn alive_children(n: usize) -> Vec<std::process::Child> {
        (0..n)
            .map(|_| {
                let mut cmd = if cfg!(windows) {
                    let mut c = std::process::Command::new("ping");
                    c.args(["-n", "30", "127.0.0.1"]);
                    c
                } else {
                    let mut c = std::process::Command::new("sleep");
                    c.arg("30");
                    c
                };
                cmd.stdout(std::process::Stdio::null())
                    .spawn()
                    .expect("眠るだけの子を起こせる")
            })
            .collect()
    }

    fn wire(pid: u32, at: u64) -> JobWire {
        JobWire {
            id: u64::from(pid),
            enqueued_at_ms: at,
            label: format!("job {pid}"),
            kind: JobKind::Other,
            ..JobWire::default()
        }
    }

    fn set_limits_in_memory(max_jobs: u32) {
        lock().settings = LimitSettings {
            max_jobs: Some(max_jobs),
            ..LimitSettings::default()
        };
        lock().settings_seen = Some((Instant::now(), settings_mtime()));
    }

    #[test]
    fn 役は明示が最優先で本番だけが調停する() {
        assert_eq!(
            role_for(Some("/tmp/q.sock"), true, true, true),
            Some(Role::Explicit)
        );
        assert_eq!(role_for(None, false, false, false), Some(Role::Production));
        assert_eq!(
            role_for(Some(""), false, false, false),
            Some(Role::Production)
        );
        assert_eq!(role_for(None, true, false, false), None, "隔離");
        assert_eq!(role_for(None, false, true, false), None, "セルフテスト");
        assert_eq!(role_for(None, false, false, true), None, "セカンダリ");
    }

    #[test]
    fn 調停役でなければwrapperの操作を断りstatusはその旨を返す() {
        let _g = serial();
        reset_for_test(None);
        let err = handle("poll", Some(&wire(std::process::id(), 1)), None, None).unwrap_err();
        assert!(err.to_string().contains(NOT_ARBITER), "{err}");
        assert_eq!(status_section()["arbiter"], false);
    }

    #[test]
    fn 同時数で割り当て終わると次へ回る() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(3);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        let a = poll(&wire(pids[0], 100), now).unwrap();
        assert_eq!(a["state"], "granted", "{a}");
        assert!(a["share"].as_u64().unwrap() >= 1);
        let b = poll(&wire(pids[1], 200), now).unwrap();
        assert_eq!(b["state"], "waiting", "{b}");
        assert_eq!(b["position"], 1);
        assert_eq!(b["reason"], "max_jobs");
        let c = poll(&wire(pids[2], 300), now).unwrap();
        assert_eq!(c["position"], 2);
        assert_eq!(c["reason"], "ahead");
        // a が走り出して終わる
        let mut started = wire(pids[0], 100);
        started.child_pid = Some(pids[0]);
        started.started_at_ms = Some(unix_ms());
        assert_eq!(poll(&started, now).unwrap()["state"], "running");
        started.exit_code = Some(3);
        assert_eq!(done(&started, now).unwrap()["known"], true);
        // 後から来た c が先に poll しても、先に並んだ b が先
        assert_eq!(poll(&wire(pids[2], 300), now).unwrap()["state"], "waiting");
        assert_eq!(poll(&wire(pids[1], 200), now).unwrap()["state"], "granted");
        let s = status_section();
        assert_eq!(s["recent"][0]["exit_code"], 3);
        assert_eq!(s["running"].as_array().unwrap().len(), 1);
        assert_eq!(s["waiting"][0]["id"], u64::from(pids[2]));
        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }
    }

    #[test]
    fn issue2243_target_dir待ちの先頭を別のdirの後ろが追い越す() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(3);
        let mut kids = alive_children(4);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let in_dir = |pid: u32, at: u64, dir: &str| JobWire {
            target_dir: Some(dir.into()),
            ..wire(pid, at)
        };
        let now = Instant::now();
        assert_eq!(
            poll(&in_dir(pids[0], 1, "/x"), now).unwrap()["state"],
            "granted"
        );
        let a = poll(&in_dir(pids[1], 2, "/x"), now).unwrap();
        assert_eq!(
            (a["position"].as_u64(), a["reason"].as_str()),
            (Some(1), Some("target_dir")),
            "{a}"
        );
        // 実行 1/3 で空き枠がある: 別の dir の後ろは待たずに入る（修正前は「前に 1 件」で待った）
        let b = poll(&in_dir(pids[2], 3, "/y"), now).unwrap();
        assert_eq!(b["state"], "granted", "{b}");
        // 同じ dir の後ろは先頭の後ろ（自分の理由も target dir）
        let c = poll(&in_dir(pids[3], 4, "/x"), now).unwrap();
        assert_eq!(
            (c["position"].as_u64(), c["reason"].as_str()),
            (Some(2), Some("target_dir")),
            "{c}"
        );
        let s = status_section();
        assert_eq!(s["waiting"][0]["id"], u64::from(pids[1]), "{s}");
        assert_eq!(s["waiting"][0]["reason"], "target_dir");
        assert_eq!(s["waiting"][0]["overtaken"], 1, "先頭は 1 回追い越された");
        assert_eq!(
            s["waiting"][0]["text"],
            "同じ target dir を使う処理が走っている"
        );
        assert_eq!(
            s["blocked"]["reason"], "target_dir",
            "先頭が待つ理由は今まで通り"
        );
        // 持ち主が終わると先頭が入り、同じ dir の後ろは先頭の後ろで待つ
        let mut holder = in_dir(pids[0], 1, "/x");
        holder.child_pid = Some(pids[0]);
        holder.exit_code = Some(0);
        done(&holder, now).unwrap();
        assert_eq!(
            poll(&in_dir(pids[3], 4, "/x"), now).unwrap()["state"],
            "waiting"
        );
        assert_eq!(
            poll(&in_dir(pids[1], 2, "/x"), now).unwrap()["state"],
            "granted"
        );
        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }
    }

    #[test]
    fn 立て直した調停役は元の受付時刻で組み直す() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(3);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        // 立て直し直後（REGATHER の内）は、走っていた件の登録し直しも、待ちも受けるが誰も入れない
        lock().enabled_at = Some(Instant::now());
        let now = Instant::now();
        let late = poll(&wire(pids[2], 300), now).unwrap();
        assert_eq!(late["state"], "waiting");
        assert_eq!(late["reason"], "regather");
        let mut running = wire(pids[0], 100);
        running.child_pid = Some(pids[0]);
        running.started_at_ms = Some(unix_ms());
        assert_eq!(poll(&running, now).unwrap()["state"], "running");
        assert_eq!(poll(&wire(pids[1], 200), now).unwrap()["state"], "waiting");
        // 窓が明けた後: 走っている 1 件で上限。先に受け付けた 200 が先頭
        lock().enabled_at = None;
        let b = poll(&wire(pids[1], 200), now).unwrap();
        assert_eq!(
            (b["position"].as_u64(), b["reason"].as_str()),
            (Some(1), Some("max_jobs"))
        );
        running.exit_code = Some(0);
        done(&running, now).unwrap();
        assert_eq!(poll(&wire(pids[2], 300), now).unwrap()["state"], "waiting");
        assert_eq!(poll(&wire(pids[1], 200), now).unwrap()["state"], "granted");
        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }
    }

    #[test]
    fn 居なくなったwrapperの枠は返る() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        assert_eq!(poll(&wire(pids[0], 1), now).unwrap()["state"], "granted");
        assert_eq!(poll(&wire(pids[1], 2), now).unwrap()["state"], "waiting");
        kids[0].kill().unwrap();
        kids[0].wait().unwrap();
        // 「居なくなった」は生存確認（調停役と同じ `pid_alive`）が終わりを映した時点から。
        // 終わらせた直後の 1 回で決めない（#2275: Windows CI で wait() の直後も居ると読まれ
        // waiting のまま落ちた）。`now` は据え置くので心拍切れでは外れない
        assert!(
            tako_core::platform::process::wait_gone(pids[0], Duration::from_secs(10)),
            "終わらせた wrapper {} が居なくならない",
            pids[0]
        );
        let got = poll(&wire(pids[1], 2), now).unwrap();
        assert_eq!(got["state"], "granted", "{got}");
        let s = status_section();
        assert_eq!(s["recent"][0]["lost"], true, "{s}");
        let _ = kids[1].kill();
        let _ = kids[1].wait();
    }

    #[test]
    fn 取り消しは待ちを外し実行中には印を付ける() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        let mut run = wire(pids[0], 1);
        assert_eq!(poll(&run, now).unwrap()["state"], "granted");
        run.child_pid = Some(pids[0]);
        assert_eq!(poll(&wire(pids[1], 2), now).unwrap()["state"], "waiting");
        // 待ちの取り消し → 次の poll で cancelled・表から消える
        assert_eq!(cancel(u64::from(pids[1])).unwrap()["phase"], "waiting");
        assert_eq!(poll(&wire(pids[1], 2), now).unwrap()["state"], "cancelled");
        assert!(status_section()["waiting"].as_array().unwrap().is_empty());
        // 実行中の取り消し → 心拍で cancel: true（止めるのは wrapper）
        assert_eq!(cancel(u64::from(pids[0])).unwrap()["phase"], "granted");
        assert_eq!(poll(&run, now).unwrap()["cancel"], true);
        // 居ない番号は理由つきで断る
        let err = cancel(1).unwrap_err();
        assert!(err.to_string().contains("居ない"), "{err}");
        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }
    }

    #[test]
    fn 知らない件のdoneは何もしない() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        let v = done(&wire(std::process::id(), 1), Instant::now()).unwrap();
        assert_eq!(v["known"], false);
    }

    #[test]
    fn 未知のactionと欠けた引数は断る() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        assert!(matches!(
            handle("nope", None, None, None),
            Err(DispatchError::InvalidParams(_))
        ));
        assert!(matches!(
            handle("poll", None, None, None),
            Err(DispatchError::InvalidParams(_))
        ));
        assert!(matches!(
            handle("cancel", None, None, None),
            Err(DispatchError::InvalidParams(_))
        ));
    }

    // ===== #2254: 実行中の予約・過去の記録・諦めた件 =====

    /// 36 GB の機（予約 3.6 GB・同時 3）を固定する
    fn fix_machine(available_mb: u64, pressure: Pressure) {
        lock().fixed_machine = Some((
            Some(MachineView {
                total_mb: 36 * 1024,
                available_mb: Some(available_mb),
                pressure: Some(pressure),
                swap_used_mb: Some(0),
            }),
            8,
        ));
    }

    fn keys(key: &str) -> q::HistoryKeys {
        q::HistoryKeys {
            key: key.into(),
            shape: None,
            name: "cargo test".into(),
        }
    }

    /// `cargo test --workspace`（見込み 7 GB = 記録なし）を走らせ、走り出してから 600 秒たった形にする
    fn start_big(pid: u32, now: Instant) -> JobWire {
        let mut big = JobWire {
            kind: JobKind::Test,
            label: "cargo test --workspace".into(),
            history: Some(keys("big")),
            ..wire(pid, 1)
        };
        assert_eq!(poll(&big, now).unwrap()["state"], "granted");
        big.child_pid = Some(pid);
        big.started_at_ms = Some(unix_ms() - 600_000);
        assert_eq!(poll(&big, now).unwrap()["state"], "running");
        big
    }

    /// 実行中の木の値を置く（走査の代わり。`peak_at_s` = 走り出してから何秒で最大になったか）
    fn set_usage(pid: u32, used_mb: u64, peak_mb: u64, peak_at_s: u64) {
        let mut a = lock();
        let job = a.jobs.get_mut(&u64::from(pid)).expect("実行中");
        let start = job.started_ms().expect("走り出している");
        job.used_mb = Some(used_mb);
        job.peak_mb = peak_mb;
        job.peak_raised_ms = Some(start + peak_at_s * 1000);
    }

    fn small(pid: u32) -> JobWire {
        JobWire {
            kind: JobKind::Test,
            label: "cargo test --test small".into(),
            history: Some(keys("small")),
            ..wire(pid, 2)
        }
    }

    fn kill_all(kids: &mut [std::process::Child]) {
        for k in kids {
            let _ = k.kill();
            let _ = k.wait();
        }
    }

    #[test]
    fn issue2254_ピークを過ぎた実行中は後ろの小さい処理を止めない() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        // 小さい処理の過去の記録（最大 1.1 GB）
        lock().history = vec![HistoryRow {
            key: "small".into(),
            kind: JobKind::Test,
            peak_mb: 1100,
            ..HistoryRow::default()
        }];
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        fix_machine(8 * 1024, Pressure::Normal);
        start_big(pids[0], now);
        // 最大 7.3 GB（見込み 7 GB に届いた）を 240 秒で過ぎて、今 0.3 GB
        set_usage(pids[0], 300, 7475, 240);
        // 本当に空きが無い（1.1 + 予約 3.6 > 空き 4 GB）→ 待つ
        fix_machine(4 * 1024, Pressure::Normal);
        let s = poll(&small(pids[1]), now).unwrap();
        assert_eq!(
            (s["state"].as_str(), s["reason"].as_str()),
            (Some("waiting"), Some("memory")),
            "{s}"
        );
        assert_eq!(
            s["estimate_mb"], 1152,
            "見込みは過去の記録（1.1 GB を 128 MB 刻みへ）"
        );
        // メモリ圧 warn → 待つ
        fix_machine(30 * 1024, Pressure::Warn);
        let s = poll(&small(pids[1]), now).unwrap();
        assert_eq!(s["reason"], "pressure_warn", "{s}");
        // 空き 8 GB・normal: 修正前は「6.7 GB + 7 GB」を要して待った形。今はピーク後なので入る
        fix_machine(8 * 1024, Pressure::Normal);
        let s = poll(&small(pids[1]), now).unwrap();
        assert_eq!(s["state"], "granted", "{s}");
        let st = status_section();
        let big = st["running"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == "cargo test --workspace")
            .cloned()
            .unwrap_or_else(|| panic!("実行中に居ない: {st}"));
        assert_eq!(
            (
                big["pending_mb"].as_u64(),
                big["peak_passed"].as_bool(),
                big["estimate_from"].as_str()
            ),
            (Some(0), Some(true), Some("default")),
            "{big}"
        );
        kill_all(&mut kids);
    }

    #[test]
    fn issue2254_終わった処理の最大を記録して次の見込みに使う() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        let mut big = start_big(pids[0], now);
        set_usage(pids[0], 300, 7475, 240);
        big.exit_code = Some(0);
        done(&big, now).unwrap();
        let row = lock().history.last().cloned().expect("記録を足した");
        assert_eq!(
            (
                row.key.as_str(),
                row.peak_mb,
                row.peak_at_s,
                row.ran_s,
                row.exit_code
            ),
            ("big", 7475, Some(240), 600, Some(0)),
            "{row:?}"
        );
        assert_eq!(row.name, "cargo test");
        // 同じコマンドの次の回: 見込みは記録の最大（7,475 MB を 128 MB 刻みへ）・山の時刻 240 秒
        let next = JobWire {
            kind: JobKind::Test,
            history: Some(keys("big")),
            ..wire(pids[1], 3)
        };
        assert_eq!(poll(&next, now).unwrap()["state"], "granted");
        let job = lock().jobs.get(&u64::from(pids[1])).cloned().unwrap();
        assert_eq!(
            (job.estimate.mb, job.estimate.from, job.estimate.peak_at_s),
            (7552, q::EstimateFrom::History, Some(240))
        );
        // 鍵を送らない旧い wrapper・走査の前に終わった件は記録しない
        let mut old = wire(pids[1], 3);
        old.child_pid = Some(pids[1]);
        let n = lock().history.len();
        assert_eq!(poll(&old, now).unwrap()["state"], "running");
        old.exit_code = Some(0);
        done(&old, now).unwrap();
        assert_eq!(lock().history.len(), n, "鍵の無い件は記録しない");
        kill_all(&mut kids);
    }

    #[test]
    fn issue2254_待ちの上限で諦めた件は押さえていた処理を残す() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        lock().history = vec![HistoryRow {
            key: "small".into(),
            peak_mb: 1100,
            ..HistoryRow::default()
        }];
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        fix_machine(8 * 1024, Pressure::Normal);
        start_big(pids[0], now);
        // まだピークの前（最大 2 GB・今 0.3 GB・記録なし）: 7 GB − 0.3 GB を押さえる
        set_usage(pids[0], 300, 2048, 30);
        let mut w = small(pids[1]);
        let s = poll(&w, now).unwrap();
        assert_eq!(s["reason"], "memory", "{s}");
        assert!(
            s["text"]
                .as_str()
                .unwrap()
                .contains("実行中のまだ使っていない見込み 6.7 GB + この処理の見込み 1.1 GB"),
            "{s}"
        );
        assert_eq!(s["holders"][0]["pending_mb"], 7168 - 300, "{s}");
        // 諦める（wrapper は終了コード 75 を載せて外れる）
        w.exit_code = Some(q::EXIT_MAX_WAIT);
        done(&w, now).unwrap();
        let st = status_section();
        let r = &st["recent"][0];
        assert_eq!(
            (r["gave_up"].as_bool(), r["reason"].as_str()),
            (Some(true), Some("memory")),
            "{r}"
        );
        let h = &r["holders"][0];
        assert_eq!(
            (
                h["label"].as_str(),
                h["pending_mb"].as_u64(),
                h["memory_mb"].as_u64(),
                h["estimate_mb"].as_u64()
            ),
            (
                Some("cargo test --workspace"),
                Some(7168 - 300),
                Some(300),
                Some(7168)
            ),
            "{r}"
        );
        assert!(st["waiting"].as_array().unwrap().is_empty());
        // 取り消し・シグナルで外れた待ち（75 以外）は残さない
        let w2 = JobWire {
            exit_code: Some(130),
            ..small(pids[1])
        };
        poll(&small(pids[1]), now).unwrap();
        done(&w2, now).unwrap();
        assert_eq!(status_section()["recent"].as_array().unwrap().len(), 1);
        kill_all(&mut kids);
    }
}
