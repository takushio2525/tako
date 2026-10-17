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
//! 塞がない・同じ dir の直列・追い越しの上限）

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use tako_core::queue::{
    self as q, Admission, AdmitInput, JobWire, LimitSettings, Limits, LimitsPatch, MachineView,
    RunningJob, WaitReason, WaitingJob,
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
    estimate_mb: u64,
    used_mb: Option<u64>,
    peak_mb: u64,
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
    fn new(wire: JobWire, now: Instant) -> Self {
        let phase = if wire.child_pid.is_some() {
            Phase::Running
        } else {
            Phase::Waiting
        };
        Self {
            estimate_mb: wire.kind.default_estimate_mb(),
            granted_at_ms: wire.started_at_ms,
            wire,
            phase,
            last_seen: now,
            cancel: false,
            used_mb: None,
            peak_mb: 0,
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
        for (id, job) in a.jobs.iter_mut() {
            if !job.holds_slot() {
                continue;
            }
            let usage = view.jobs.get(id).copied();
            job.used_mb = usage.map(|u| u.memory / MIB);
            job.peak_mb = job.peak_mb.max(job.used_mb.unwrap_or(0));
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
}

/// 割り当てを判断して、入れた件を割り当て済みにする
fn decide(a: &mut Arbiter, inj: Option<&Inject>, now: Instant) {
    let (machine_view, cpus) = machine(a, inj);
    let limits: Limits = q::effective_limits(
        &a.settings,
        machine_view.map(|m| m.total_mb).filter(|t| *t > 0),
    );
    let running: Vec<RunningJob> = a
        .jobs
        .values()
        .filter(|j| j.holds_slot())
        .map(|j| RunningJob {
            id: j.wire.id,
            estimate_mb: j.estimate_mb,
            used_mb: j.used_mb,
            target_dir: j.wire.target_dir.clone(),
            stalled: j.stalled,
        })
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
            estimate_mb: j.estimate_mb,
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
            let job = Job::new(wire.clone(), now);
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
    if job.holds_slot() {
        push_recent(&mut a, &job, wire.exit_code, false);
    }
    prune(&mut a, now);
    Ok(json!({
        "state": "done",
        "known": true,
        "peak_mb": (peak > 0).then_some(peak),
    }))
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
    let running: Vec<Value> = a
        .jobs
        .values()
        .filter(|j| j.holds_slot())
        .map(|j| {
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
        assert_eq!(poll(&wire(pids[1], 2), now).unwrap()["state"], "granted");
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
}
