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
//!
//! ## 優先度と終わりの見込み（#1996）
//!
//! - 処理の段は受け答えのたびに決める（`Job::base_priority`）: 処理（受付の指定・受付番号での変更）>
//!   ペインの既定（GUI が渡すペインの一覧の `queue_priority`）> 役割の既定（一覧の役割ラベル、無ければ
//!   wrapper が env から読んだ役割）。待ちにはこちらの時計でエイジング（`q::priority::aged`）を当てる
//! - 受付番号での変更は**この表とその wrapper の両方**に残す（応答の `job_priority` を wrapper が覚え、
//!   次の poll から送る = 調停役を立て直しても戻らない。正本は wrapper のまま）。ペインの既定の変更は
//!   ペインの模型（layout.json）を書くので dispatch（UI スレッド）が受け、一覧の覚えへもすぐ写す
//! - 終わりの見込みは受付のときに過去の所要時間（`q::eta::duration_history`）を引き、wrapper が心拍で
//!   送る cargo の `N/M` と合わせて status で出す（`eta` / `progress`）。待ちには開始の見込み（`start_s`）

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use tako_core::queue::eta::{self, DurationHistory, Eta};
use tako_core::queue::priority::{self as qp, CallerRole, PriorityFrom, PriorityInput};
use tako_core::queue::{
    self as q, Admission, AdmitInput, Estimate, HistoryRow, JobWire, LimitSettings, Limits,
    LimitsPatch, MachineView, Priority, RunningJob, WaitReason, WaitingJob,
};
use tako_core::resources::{Pressure, MIB};

use crate::dispatch::{DispatchError, Reason};
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
    /// 受付番号での優先度の変更（#1996）。`Some(None)` = 既定へ戻した。wrapper が応答の
    /// `job_priority` を覚えて送ってくる（= `wire.hint.set` が同じになる）まで持つ
    priority_override: Option<Option<Priority>>,
    /// 過去の所要時間（受付のときに引く。#1996）
    durations: Option<DurationHistory>,
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
            durations: eta::duration_history(wire.history.as_ref(), history),
            priority_override: None,
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

    /// 処理に付いた段（#1996）: 受付番号での変更 → wrapper が送った指定。旧い wrapper（手がかりを送らない =
    /// S2）は送ってきた段をそのまま処理の指定とみなす（既定の normal も = S2 と同じ並び）
    fn job_setting(&self) -> Option<Priority> {
        if let Some(o) = self.priority_override {
            return o;
        }
        match &self.wire.hint {
            Some(h) => h.set,
            None => Some(self.wire.priority),
        }
    }

    /// エイジングの前の段と、その出どころ（#1996。処理 > ペイン > 役割）
    fn base_priority(&self) -> (Priority, PriorityFrom) {
        let pane = self.wire.pane.and_then(crate::resources::pane_queue_info);
        let hint = self.wire.hint.unwrap_or_default();
        let role = pane
            .as_ref()
            .and_then(|(label, _)| label.as_deref())
            .and_then(CallerRole::from_pane_role)
            .or(hint.role);
        qp::resolve(&PriorityInput {
            job: self.job_setting(),
            pane_default: pane.and_then(|(_, p)| p),
            in_pane: self.wire.pane.is_some(),
            role,
            tty: hint.tty,
        })
    }

    /// 実効の段（待ちならエイジングの後。`wall` = いまの Unix ミリ秒）
    fn effective_priority(&self, wall: u64) -> Priority {
        let (base, _) = self.base_priority();
        if self.phase == Phase::Waiting {
            qp::aged(base, wall.saturating_sub(self.wire.enqueued_at_ms))
        } else {
            base
        }
    }

    /// 走っている処理の終わりの見込み（#1996）
    fn eta(&self, wall: u64) -> Eta {
        let ran = self
            .started_ms()
            .map_or(0, |s| wall.saturating_sub(s) / 1000);
        eta::eta(ran, self.wire.progress, self.durations.as_ref())
    }

    /// 優先度の欄（status / 応答。#1996）
    fn priority_json(&self, wall: u64) -> Value {
        let (base, from) = self.base_priority();
        let effective = self.effective_priority(wall);
        json!({
            "priority": effective.as_str(),
            "base_priority": base.as_str(),
            "priority_from": from.as_str(),
            "aged": effective != base,
            "job_priority": self.job_setting().map(Priority::as_str),
        })
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
            units: self.wire.progress.map(|p| p.total).filter(|m| *m > 0),
        })
    }
}

/// JSON の欄へ値を足す（どちらも object のときだけ）
fn merge(into: &mut Value, from: Value) {
    if let (Some(dst), Value::Object(src)) = (into.as_object_mut(), from) {
        dst.extend(src);
    }
}

/// 終わりの見込みの欄（#1996）
fn eta_json(e: &Eta) -> Value {
    let (previous_s, progress) = match *e {
        Eta::Remaining {
            previous_s,
            progress,
            ..
        } => (previous_s, progress),
        Eta::Overrun {
            previous_s,
            progress,
            ..
        } => (Some(previous_s), progress),
        Eta::Unknown { progress, .. } => (None, progress),
    };
    json!({
        "state": e.code(),
        "remaining_s": e.remaining_s(),
        "previous_s": previous_s,
        "from": match e {
            Eta::Remaining { from: eta::EtaFrom::History, .. } | Eta::Overrun { .. } => Some("history"),
            Eta::Remaining { from: eta::EtaFrom::Progress, .. } => Some("progress"),
            Eta::Unknown { .. } => None,
        },
        "progress": progress,
        "text": e.text(),
    })
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
        // 壊れた行があれば退避（`.unreadable.bak`）してから読める行だけで書き直す（#1996）
        a.history = a
            .history_dir
            .as_deref()
            .map(crate::queue_history::load_and_repair)
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
    (q::machine_view(&stats), cpus)
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
        priority,
        ..
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
            handle(
                other,
                job.as_ref(),
                *id,
                limits.as_ref(),
                priority.as_deref(),
            )
        }
    }
}

/// UI スレッドの dispatch へ渡す要求か（#1996）: ペインの優先度の既定の変更はペインの模型
/// （layout.json）を書くので、`ipc::submit` の手前では答えない。それ以外の順番待ちの要求は手前で答える
pub fn needs_ui(request: &Request) -> bool {
    matches!(
        request,
        Request::Queue { action, pane: Some(_), .. } if action.as_deref() == Some("priority")
    )
}

/// status の JSON へ順番待ちの節と agent の起動待ちの節（#2413）と cargo の札の節（#1998）を足す
/// （採らない。札の空きは fifo を数えるだけ = `FIONREAD` 1 回。UI スレッドから呼んでよい）
pub fn with_section(mut status: Value) -> Value {
    status["queue"] = status_section();
    status["launches"] = crate::launch_gate::status_section();
    status["jobserver"] = crate::jobserver::status_section();
    status
}

/// 順番待ちの操作（status 以外）。**採らない**（走査の覚えと機の値の覚えを読むだけ）
pub fn handle(
    action: &str,
    job: Option<&JobWire>,
    id: Option<u64>,
    limits: Option<&LimitsPatch>,
    priority: Option<&str>,
) -> Result<Value, DispatchError> {
    match action {
        // #2413: agent の起動待ちを今すぐ起こす（id = 待っているペイン）。起動待ちは調停役とは別に
        // 持つ（隔離の注入でも効く）ので、調停役かどうかを問わない
        "start" => {
            let pane = id.ok_or_else(|| {
                DispatchError::InvalidParams(
                    "start には起動を待っているペインの番号（id）が要る".into(),
                )
            })?;
            crate::launch_gate::start_now(pane).map_err(DispatchError::Operation)
        }
        "limits" => set_limits(limits.cloned().unwrap_or_default()),
        "cancel" => {
            let id = id.ok_or_else(|| {
                DispatchError::InvalidParams("cancel には受付番号（id）が要る".into())
            })?;
            cancel(id)
        }
        // #1996: 受付番号の処理の優先度（ペインの既定は dispatch の `queue_pane_priority`）
        "priority" => {
            let setting = parse_priority_setting(priority)?;
            let id = id.ok_or_else(|| Reason::QueuePriorityTargetMissing.invalid())?;
            set_job_priority(id, setting)
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
            "未知の action: {other}（使えるのは status / cancel / priority / limits / start）"
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

/// 割り当ての判断の材料（[`decide`] と spawn の応答の助言 = [`heavy_headroom`] が同じ 1 本を通す。#1997）
struct DecideInputs {
    machine: Option<MachineView>,
    cpus: u32,
    limits: Limits,
    running: Vec<RunningJob>,
    waiting: Vec<WaitingJob>,
    regathering: bool,
    unmanaged: u32,
}

impl DecideInputs {
    /// [`q::admit`] の入力（重い処理の方針はここ 1 か所）
    fn admit_input(&self) -> AdmitInput<'_> {
        AdmitInput {
            machine: self.machine,
            limits: self.limits,
            running: &self.running,
            waiting: &self.waiting,
            unmanaged: self.unmanaged,
            regathering: self.regathering,
            overtake: !legacy_2243(),
            release_after_peak: !legacy_2254(),
            // 重い処理は終われば空くので、外の負荷だけで永久に待たせない（agent の起動待ち = #2413 は false）
            min_progress: true,
        }
    }
}

fn decide_inputs(a: &Arbiter, inj: Option<&Inject>, now: Instant) -> DecideInputs {
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
    let regather = inj
        .and_then(|i| i.regather_secs)
        .map(Duration::from_secs)
        .unwrap_or(REGATHER);
    DecideInputs {
        machine: machine_view,
        cpus,
        limits,
        running,
        waiting: waiting_jobs(a),
        regathering: a
            .enabled_at
            .is_some_and(|at| now.duration_since(at) < regather),
        unmanaged: a.unmanaged,
    }
}

/// 割り当てを判断して、入れた件を割り当て済みにする
fn decide(a: &mut Arbiter, inj: Option<&Inject>, now: Instant) {
    let inputs = decide_inputs(a, inj, now);
    let (cpus, running) = (inputs.cpus, &inputs.running);
    let admission = q::admit(&inputs.admit_input());
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

/// 待ち（取り消した件を除く）を判断の入力の形へ。段は処理 > ペイン > 役割で決めてエイジングを当てた実効の段
/// （#1996。時計はここで読む = 並びの純関数は時計を持たない）
fn waiting_jobs(a: &Arbiter) -> Vec<WaitingJob> {
    let wall = unix_ms();
    a.jobs
        .values()
        .filter(|j| j.phase == Phase::Waiting && !j.cancel)
        .map(|j| WaitingJob {
            id: j.wire.id,
            estimate_mb: j.estimate.mb,
            target_dir: j.wire.target_dir.clone(),
            priority: j.effective_priority(wall),
            enqueued_at_ms: j.wire.enqueued_at_ms,
            overtaken: j.overtaken,
        })
        .collect()
}

/// 待ちの 1 件が始まるまでの見込み（秒。#1996）: 並びで前に居る件と、枠を持つ実行中（停滞を除く）の
/// 全件に見込みがあるときだけ（`q::eta::start_after`）
fn start_estimate(a: &Arbiter, order: &[u64], id: u64, limits: &Limits) -> Option<u64> {
    let wall = unix_ms();
    let me = a.jobs.get(&id)?;
    let holders: Vec<&Job> = a
        .jobs
        .values()
        .filter(|j| j.holds_slot() && !j.stalled)
        .collect();
    let running: Vec<Option<u64>> = holders.iter().map(|j| j.eta(wall).remaining_s()).collect();
    let ahead: Vec<Option<u64>> = order
        .iter()
        .take_while(|x| **x != id)
        .filter_map(|x| a.jobs.get(x))
        .map(|j| j.durations.map(|d| d.expected_s))
        .collect();
    // 同じ target dir の実行中が居れば、その dir が空くまで始まらない
    let floor = me.wire.target_dir.as_ref().and_then(|dir| {
        holders
            .iter()
            .filter(|j| j.wire.target_dir.as_ref() == Some(dir))
            .map(|j| j.eta(wall).remaining_s())
            .max()
            .flatten()
    });
    let slots = limits.max_jobs.saturating_sub(a.unmanaged).max(1);
    eta::start_after(slots, &running, &ahead, floor)
}

/// `priority` の段を読む（`default` = 既定へ戻す = `None`）
fn parse_priority_setting(priority: Option<&str>) -> Result<Option<Priority>, DispatchError> {
    let text = priority.ok_or_else(|| Reason::QueuePriorityLevelMissing.invalid())?;
    qp::parse_setting(text)
        .ok_or_else(|| Reason::QueuePrioritySettingUnknown(text.to_string()).invalid())
}

/// 受付番号の処理の優先度を変える（#1996。`setting` = `None` で既定 = ペイン > 役割へ戻す）。
/// 待ちなら次の判断から並びに効く。走っている処理は止めない（横取りしない）
pub fn set_job_priority(id: u64, setting: Option<Priority>) -> Result<Value, DispatchError> {
    let mut a = lock();
    not_arbiter(&a)?;
    prune(&mut a, Instant::now());
    let wall = unix_ms();
    let Some(job) = a.jobs.get_mut(&id) else {
        return Err(Reason::QueueJobUnknown(id).invalid());
    };
    let before = job.effective_priority(wall);
    job.priority_override = Some(setting);
    let after = job.effective_priority(wall);
    let mut v = json!({
        "id": id,
        "label": job.wire.label,
        "phase": job.phase.as_str(),
        "changed": before != after,
    });
    merge(&mut v, job.priority_json(wall));
    if job.phase != Phase::Waiting {
        v["note"] = json!("走っている処理は止めない（優先度は待ちの並びにだけ効く）");
    }
    let order = waiting_order(&a);
    if let Some(pos) = order.iter().position(|x| *x == id) {
        v["position"] = json!(pos + 1);
    }
    Ok(v)
}

/// ペインの優先度の既定が変わった後の、そのペインから出ている処理（#1996。dispatch がペインに付けた直後に呼ぶ。
/// 調停役でなければ空 = ペインの既定は layout.json に残り、次に調停役になった GUI が使う）
pub fn pane_jobs(pane: u64) -> Vec<Value> {
    let a = lock();
    if a.role.is_none() {
        return Vec::new();
    }
    let wall = unix_ms();
    let order = waiting_order(&a);
    a.jobs
        .values()
        .filter(|j| j.wire.pane == Some(pane))
        .map(|j| {
            let mut v = json!({
                "id": j.wire.id,
                "label": j.wire.label,
                "phase": j.phase.as_str(),
                "position": order.iter().position(|x| *x == j.wire.id).map(|p| p + 1),
            });
            merge(&mut v, j.priority_json(wall));
            v
        })
        .collect()
}

/// 行の「誰」（タブ・ペイン名・役割・ペインの優先度の既定。#1995）。ペインの外の処理は何も足さない。
/// ロックの順は調停役 → ペインの一覧（`Job::base_priority` と同じ向き）
fn owner_fields(job: &Job) -> Value {
    job.wire
        .pane
        .map_or_else(|| json!({}), crate::resources::pane_owner_fields)
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
            // 受付番号での変更を wrapper が覚えて送ってきたら、こちらで持つのをやめる（#1996）
            if job
                .priority_override
                .is_some_and(|o| wire.hint.is_some_and(|h| h.set == o))
            {
                job.priority_override = None;
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
            // 受付番号での変更を wrapper が覚える（#1996。調停役を立て直しても戻らない）
            "job_priority": job.job_setting().map(Priority::as_str),
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
            let start = start_estimate(&a, &order, id, &limits);
            let wall = unix_ms();
            let mut v = json!({
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
                // 開始の見込み（#1996。前に並ぶ全件と実行中の全件に見込みがあるときだけ）
                "start_s": start,
                "start_text": start.map(eta::start_text),
            });
            merge(&mut v, job.priority_json(wall));
            Ok(v)
        }
    }
}

fn granted_json(a: &Arbiter, job: &Job, inj: Option<&Inject>) -> Value {
    let limits = current_limits(a, inj);
    json!({
        "state": "granted",
        "job_priority": job.job_setting().map(Priority::as_str),
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
    let Some(mut job) = a.jobs.remove(&wire.id) else {
        // 調停役の立て直しの後で登録し直す前に終わった件。知らないので何もしない
        return Ok(json!({ "state": "done", "known": false }));
    };
    // 最後の進み（M = ユニットの総数）を記録へ（#1996）
    if wire.progress.is_some() {
        job.wire.progress = wire.progress;
    }
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
        // cargo の共有 jobserver（#1998）。キーが無ければ既定の on
        "jobserver": configured.jobserver_on(),
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
        // 札の番人へすぐ反映する（GUI が居ないときの書き込みは番人が mtime で拾う）
        if patch.jobserver.is_some() || patch.reset {
            crate::jobserver::apply_setting(settings.resources.jobserver_on());
        }
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

/// spawn の応答の `machine` / `advice`（#1997）のうち順番待ちの側
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeavyHeadroom {
    /// 枠を持つ実行中（停滞を含む = status の `running` と同じ数え方）
    pub running: u32,
    pub waiting: u32,
    /// いまビルド 1 件（区分の既定の見込み）を受け付けたら待つ理由（入るなら `None`）
    pub next: Option<WaitReason>,
}

/// spawn の応答の順番待ちの側（#1997。UI スレッドの dispatch の spawn が呼ぶ）。調停役でなければ `None`。
/// **採らない・書き換えない**（覚えを読むだけ。判断の材料は [`decide`] と同じ [`decide_inputs`]、
/// 問い方は `q::headroom::probe` = 同じ `admit` の待ちの最後に仮の 1 件）
pub fn heavy_headroom() -> Option<HeavyHeadroom> {
    let a = lock();
    a.role?;
    let inj = inject(a.role);
    let inputs = decide_inputs(&a, inj.as_ref(), Instant::now());
    let estimate = Estimate::default_for(q::JobKind::Build).mb;
    Some(HeavyHeadroom {
        running: inputs.running.len() as u32,
        waiting: inputs.waiting.len() as u32,
        next: q::headroom::probe(&inputs.admit_input(), estimate),
    })
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
            let mut v = json!({
                "id": j.wire.id,
                "label": j.wire.label,
                "kind": j.wire.kind.as_str(),
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
                // 進みと終わりの見込み（#1996）
                "progress": j.wire.progress,
                "eta": eta_json(&j.eta(wall)),
            });
            merge(&mut v, j.priority_json(wall));
            merge(&mut v, owner_fields(j));
            v
        })
        .collect();
    let order = waiting_order(&a);
    let waiting: Vec<Value> = order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| a.jobs.get(id).map(|j| (i, j)))
        .map(|(i, j)| {
            let start = start_estimate(&a, &order, j.wire.id, &limits);
            let mut v = json!({
                "id": j.wire.id,
                "position": i + 1,
                "label": j.wire.label,
                "kind": j.wire.kind.as_str(),
                "pane": j.wire.pane,
                "waited_s": wall.saturating_sub(j.wire.enqueued_at_ms) / 1000,
                "reason": a.last.reason_of(j.wire.id).map(WaitReason::code),
                "text": waiting_text(&a.last, j.wire.id),
                "overtaken": j.overtaken,
                "estimate_mb": j.estimate.mb,
                "estimate_from": j.estimate.from.as_str(),
                // 過去の所要時間と開始の見込み（#1996）
                "expected_s": j.durations.map(|d| d.expected_s),
                "start_s": start,
                "start_text": start.map(eta::start_text),
            });
            merge(&mut v, j.priority_json(wall));
            merge(&mut v, owner_fields(j));
            v
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

/// 調停役とペインの一覧の覚え（どちらもプロセスに 1 つ）に触るテストの直列化（dispatch のテストも取る。#1996）
#[cfg(test)]
pub(crate) fn test_serial() -> std::sync::MutexGuard<'static, ()> {
    static M: Mutex<()> = Mutex::new(());
    M.lock().unwrap_or_else(|p| p.into_inner())
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
        drop(a);
        // ペインの一覧（優先度の既定と役割の材料。#1996）も空へ
        crate::resources::publish_panes(Vec::new(), None);
    }

    /// 調停役はプロセスに 1 つの状態なので、テストは直列に回す
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        test_serial()
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
        let err = handle("poll", Some(&wire(std::process::id(), 1)), None, None, None).unwrap_err();
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
            handle("nope", None, None, None, None),
            Err(DispatchError::InvalidParams(_))
        ));
        assert!(matches!(
            handle("poll", None, None, None, None),
            Err(DispatchError::InvalidParams(_))
        ));
        assert!(matches!(
            handle("cancel", None, None, None, None),
            Err(DispatchError::InvalidParams(_))
        ));
    }

    /// #1997: spawn の応答の順番待ちの助言は、実際に同じ形の 1 件を受け付けたときの答えと一致する
    /// （同じ `decide_inputs` の入力の `admit`）。調停役でなければ `None`
    #[test]
    fn issue1997_順番待ちの助言は実際の受け付けと同じ答え() {
        let _g = serial();
        reset_for_test(None);
        assert_eq!(heavy_headroom(), None, "調停役でない");
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let kids = alive_children(2);
        let pid = |i: usize| kids[i].id();
        let build = |p: u32| JobWire {
            kind: JobKind::Build,
            ..wire(p, 1)
        };
        // 空いている: 入る見込み → 実際に入る
        let h = heavy_headroom().expect("調停役");
        assert_eq!((h.running, h.waiting, h.next.clone()), (0, 0, None));
        assert_eq!(
            poll(&build(pid(0)), Instant::now()).unwrap()["state"],
            "granted"
        );
        // 同時数 1 が埋まった: 待つ見込み（max_jobs）→ 実際に待つ（同じ理由）
        let h = heavy_headroom().expect("調停役");
        assert_eq!(h.running, 1);
        assert_eq!(h.next.as_ref().map(WaitReason::code), Some("max_jobs"));
        let r = poll(&build(pid(1)), Instant::now()).unwrap();
        assert_eq!(r["state"], "waiting", "{r}");
        assert_eq!(
            lock()
                .last
                .reason_of(u64::from(pid(1)))
                .map(WaitReason::code),
            Some("max_jobs")
        );
        let h = heavy_headroom().expect("調停役");
        assert_eq!((h.running, h.waiting), (1, 1));
        for mut k in kids {
            let _ = k.kill();
            let _ = k.wait();
        }
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

    // ===== #1996: 優先度と終わりの見込み =====

    const MIN_MS: u64 = 60 * 1000;

    /// 受付の時刻は今の時刻 + `at` ミリ秒（小さい値をそのまま使うと 1970 年からの待ち = 全件が high へ上がる）
    fn hinted(
        pid: u32,
        at: u64,
        pane: Option<u64>,
        role: Option<CallerRole>,
        tty: bool,
    ) -> JobWire {
        JobWire {
            enqueued_at_ms: unix_ms() + at,
            pane,
            hint: Some(q::PriorityHint {
                set: None,
                tty,
                role,
            }),
            ..wire(pid, at)
        }
    }

    fn pane_entry(
        pane: u64,
        role: Option<&str>,
        prio: Option<Priority>,
    ) -> crate::resources::PaneEntry {
        crate::resources::PaneEntry {
            pane,
            role: role.map(str::to_string),
            queue_priority: prio,
            ..crate::resources::PaneEntry::default()
        }
    }

    /// 枠 1 を実行中で埋める（後の受付はすべて待つ）
    fn occupy(pid: u32, now: Instant) {
        let mut run = wire(pid, 1);
        assert_eq!(poll(&run, now).unwrap()["state"], "granted");
        run.child_pid = Some(pid);
        run.started_at_ms = Some(unix_ms());
        assert_eq!(poll(&run, now).unwrap()["state"], "running");
    }

    fn waiting_ids() -> Vec<u64> {
        status_section()["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_u64().unwrap())
            .collect()
    }

    fn waiting_of(id: u64) -> Value {
        status_section()["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    #[test]
    fn issue1996_段は処理とペインと役割の順で決まり変更で並びが変わる() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(6);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let id = |i: usize| u64::from(pids[i]);
        let now = Instant::now();
        occupy(pids[0], now);
        crate::resources::publish_panes(
            vec![
                pane_entry(10, Some("orchestrator-master:proj"), None),
                pane_entry(11, Some("orchestrator-worker:tako:l"), None),
                pane_entry(12, None, Some(Priority::Low)),
            ],
            None,
        );
        // A = ペインの外（low）/ B = worker（normal）/ C = master（high）/
        // D = ペインの既定 low（端末から打っても既定が勝つ）/ E = 役割の無いペインで端末から（top）
        poll(&hinted(pids[1], 2, None, None, true), now).unwrap();
        poll(&hinted(pids[2], 3, Some(11), None, false), now).unwrap();
        poll(&hinted(pids[3], 4, Some(10), None, false), now).unwrap();
        poll(&hinted(pids[4], 5, Some(12), None, true), now).unwrap();
        poll(&hinted(pids[5], 6, Some(13), None, true), now).unwrap();
        assert_eq!(waiting_ids(), vec![id(5), id(3), id(2), id(1), id(4)]);
        let from: Vec<String> = [5, 3, 2, 1, 4]
            .iter()
            .map(|i| {
                let w = waiting_of(id(*i));
                format!(
                    "{}/{}",
                    w["priority"].as_str().unwrap(),
                    w["priority_from"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(
            from,
            [
                "top/role",
                "high/role",
                "normal/role",
                "low/role",
                "low/pane"
            ]
        );

        // 受付番号で変える（処理に付けた段がペイン・役割より強い）
        let r = set_job_priority(id(1), Some(Priority::Top)).unwrap();
        assert_eq!(
            (
                r["priority"].as_str(),
                r["priority_from"].as_str(),
                r["changed"].as_bool()
            ),
            (Some("top"), Some("job"), Some(true)),
            "{r}"
        );
        assert_eq!(r["position"], 1, "同じ top の中では受付の古い A が先: {r}");
        assert_eq!(waiting_ids(), vec![id(1), id(5), id(3), id(2), id(4)]);
        // ペインの既定（dispatch が写す覚え）を変えると、そのペインの既定の処理が動く
        crate::resources::set_pane_queue_priority(11, Some(Priority::High));
        assert_eq!(
            waiting_ids(),
            vec![id(1), id(5), id(2), id(3), id(4)],
            "B（受付 3）が C（受付 4）の前へ"
        );
        assert_eq!(pane_jobs(11)[0]["priority_from"], "pane");
        // 既定へ戻す（ペインの外 = low。受付の古い A が D より先）
        let r = set_job_priority(id(1), None).unwrap();
        assert_eq!(
            (r["priority"].as_str(), r["priority_from"].as_str()),
            (Some("low"), Some("role")),
            "{r}"
        );
        assert_eq!(waiting_ids(), vec![id(5), id(2), id(3), id(1), id(4)]);
        // 同じ段への変更は changed = false・走っている処理には印だけ
        let r = set_job_priority(id(5), Some(Priority::Top)).unwrap();
        assert_eq!(r["changed"], false, "{r}");
        let r = set_job_priority(id(0), Some(Priority::Low)).unwrap();
        assert_eq!(r["phase"], "running");
        assert!(r["note"].as_str().unwrap().contains("止めない"), "{r}");
        // 居ない受付番号・段の無い / 読めない値は断る
        let e = set_job_priority(424242, Some(Priority::High)).unwrap_err();
        assert!(e.to_string().contains("424242"), "{e}");
        assert!(matches!(
            handle("priority", None, Some(id(1)), None, None),
            Err(DispatchError::InvalidParams(_))
        ));
        let e = handle("priority", None, Some(id(1)), None, Some("urgent")).unwrap_err();
        assert!(e.to_string().contains("urgent"), "{e}");
        let e = handle("priority", None, None, None, Some("high")).unwrap_err();
        assert!(
            e.to_string().contains("pane"),
            "受付番号もペインも無い: {e}"
        );
        kill_all(&mut kids);
        reset_for_test(None);
    }

    #[test]
    fn issue1996_待ち15分ごとに1段上がりtopへは上げない() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(4);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let id = |i: usize| u64::from(pids[i]);
        let now = Instant::now();
        occupy(pids[0], now);
        let wall = unix_ms();
        // A = ペインの外（low）を 31 分前に受付 → high / B = worker（normal）を 1 分前 → normal のまま /
        // C = 端末から（top）を今
        let at = |w: JobWire, ms: u64| JobWire {
            enqueued_at_ms: ms,
            ..w
        };
        let worker = hinted(pids[2], 0, Some(13), Some(CallerRole::Worker), false);
        poll(
            &at(hinted(pids[1], 0, None, None, false), wall - 31 * MIN_MS),
            now,
        )
        .unwrap();
        poll(&at(worker, wall - MIN_MS), now).unwrap();
        poll(&at(hinted(pids[3], 0, Some(13), None, true), wall), now).unwrap();
        assert_eq!(waiting_ids(), vec![id(3), id(1), id(2)]);
        let a = waiting_of(id(1));
        assert_eq!(
            (
                a["priority"].as_str(),
                a["base_priority"].as_str(),
                a["aged"].as_bool()
            ),
            (Some("high"), Some("low"), Some(true)),
            "{a}"
        );
        // 何時間待っても top へは上がらない（人が直接打った処理の前へ割り込まない）
        lock().jobs.get_mut(&id(1)).unwrap().wire.enqueued_at_ms = wall - 600 * MIN_MS;
        assert_eq!(waiting_ids(), vec![id(3), id(1), id(2)]);
        assert_eq!(waiting_of(id(1))["priority"], "high");
        kill_all(&mut kids);
        reset_for_test(None);
    }

    #[test]
    fn issue1996_受付番号での変更はwrapperが覚えるまで持ち立て直しても戻らない() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        occupy(pids[0], now);
        let mut w = hinted(pids[1], 2, None, None, false);
        poll(&w, now).unwrap();
        set_job_priority(u64::from(pids[1]), Some(Priority::High)).unwrap();
        // まだ覚えていない wrapper の poll でも変更は残り、応答で教える
        let r = poll(&w, now).unwrap();
        assert_eq!(
            (r["priority"].as_str(), r["job_priority"].as_str()),
            (Some("high"), Some("high")),
            "{r}"
        );
        crate::queue_wrapper::adopt_job_priority(&mut w, &r);
        assert_eq!(w.hint.unwrap().set, Some(Priority::High));
        poll(&w, now).unwrap();
        assert_eq!(
            lock().jobs[&u64::from(pids[1])].priority_override,
            None,
            "wrapper が送ってきたら持つのをやめる"
        );
        // 調停役を立て直しても、wrapper が送る段で戻る
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        occupy(pids[0], now);
        let r = poll(&w, now).unwrap();
        assert_eq!(
            (r["priority"].as_str(), r["priority_from"].as_str()),
            (Some("high"), Some("job")),
            "{r}"
        );
        kill_all(&mut kids);
        reset_for_test(None);
    }

    #[test]
    fn issue1996_旧いwrapperは送ってきた段をそのまま使う() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        let mut kids = alive_children(2);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let now = Instant::now();
        occupy(pids[0], now);
        crate::resources::publish_panes(
            vec![pane_entry(10, Some("orchestrator-master"), None)],
            None,
        );
        // S2 の wrapper: 手がかりを送らず、既定の normal（master のペインでも high にしない = S2 と同じ並び）
        let old = JobWire {
            pane: Some(10),
            ..wire(pids[1], unix_ms())
        };
        let r = poll(&old, now).unwrap();
        assert_eq!(
            (r["priority"].as_str(), r["priority_from"].as_str()),
            (Some("normal"), Some("job")),
            "{r}"
        );
        kill_all(&mut kids);
        reset_for_test(None);
    }

    #[test]
    fn issue1996_同じ鍵の2回目から終わりと開始の見込みが出る() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        set_limits_in_memory(1);
        // 1 回目の記録（10 分・成功）
        lock().history = vec![HistoryRow {
            key: "eta".into(),
            name: "cargo build".into(),
            kind: JobKind::Build,
            peak_mb: 100,
            ran_s: 600,
            exit_code: Some(0),
            ..HistoryRow::default()
        }];
        let mut kids = alive_children(4);
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        let id = |i: usize| u64::from(pids[i]);
        let now = Instant::now();
        let k = || Some(keys("eta"));
        // 2 回目が走って 1 分
        let mut run = JobWire {
            history: k(),
            ..wire(pids[0], 1)
        };
        poll(&run, now).unwrap();
        run.child_pid = Some(pids[0]);
        run.started_at_ms = Some(unix_ms() - 60_000);
        poll(&run, now).unwrap();
        let st = status_section();
        let eta = &st["running"][0]["eta"];
        assert_eq!(
            (
                eta["state"].as_str(),
                eta["from"].as_str(),
                eta["previous_s"].as_u64()
            ),
            (Some("remaining"), Some("history"), Some(600)),
            "{eta}"
        );
        assert!(
            (535..=540).contains(&eta["remaining_s"].as_u64().unwrap()),
            "{eta}"
        );
        assert_eq!(eta["text"], "あと約 9 分（前回 10 分）");
        // 待ち: W（同じ鍵）→ 実行中の残りの後 / X（記録なし）→ W の見込みの後 / Y → 前の X に見込みが無いので順位だけ
        poll(
            &JobWire {
                history: k(),
                ..wire(pids[1], 2)
            },
            now,
        )
        .unwrap();
        poll(&wire(pids[2], 3), now).unwrap();
        poll(
            &JobWire {
                history: k(),
                ..wire(pids[3], 4)
            },
            now,
        )
        .unwrap();
        let w = waiting_of(id(1));
        let start = w["start_s"].as_u64().unwrap();
        assert!((535..=540).contains(&start), "{w}");
        assert_eq!(w["expected_s"], 600);
        let x = waiting_of(id(2));
        assert!(
            (1135..=1140).contains(&x["start_s"].as_u64().unwrap()),
            "{x}"
        );
        assert_eq!(x["start_text"], "開始まで約 19 分");
        assert!(
            waiting_of(id(3))["start_s"].is_null(),
            "前に見込みの無い件が居る"
        );
        // 前回より長い（2 回目が 11 分）
        lock().jobs.get_mut(&id(0)).unwrap().wire.started_at_ms = Some(unix_ms() - 660_000);
        let eta = status_section()["running"][0]["eta"].clone();
        assert_eq!(eta["state"], "overrun", "{eta}");
        assert_eq!(
            eta["text"],
            "前回より長くかかっている（経過 11 分・前回 10 分）"
        );
        // 記録の無い処理: 進みがあれば線形、無ければ見込みなし
        {
            let mut a = lock();
            let j = a.jobs.get_mut(&id(0)).unwrap();
            j.durations = None;
            j.wire.started_at_ms = Some(unix_ms() - 60_000);
        }
        assert_eq!(status_section()["running"][0]["eta"]["state"], "unknown");
        lock().jobs.get_mut(&id(0)).unwrap().wire.progress =
            Some(q::Progress { done: 5, total: 10 });
        let eta = status_section()["running"][0]["eta"].clone();
        assert_eq!(
            (eta["state"].as_str(), eta["from"].as_str()),
            (Some("remaining"), Some("progress")),
            "{eta}"
        );
        assert_eq!(status_section()["running"][0]["progress"]["total"], 10);
        kill_all(&mut kids);
        reset_for_test(None);
    }

    #[test]
    fn issue1996_記録の行はハッシュと表示名だけでユニット数を持つ() {
        let _g = serial();
        reset_for_test(Some(Role::Production));
        let dir = tako_core::test_residue::ScratchDir::new("queue-1996-history");
        lock().history_dir = Some(dir.to_path_buf());
        let mut kids = alive_children(1);
        let pid = kids[0].id();
        let now = Instant::now();
        let argv: Vec<String> = [
            "env",
            "SECRET_TOKEN=abc123",
            "/opt/tools/bin/cargo",
            "test",
            "--manifest-path",
            "/home/someone/proj/Cargo.toml",
            "--test",
            "issue_private_name",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut w = crate::queue_wrapper::build_wire(
            &argv,
            &crate::queue_wrapper::Options::default(),
            &dir,
        );
        w.id = u64::from(pid);
        assert_eq!(poll(&w, now).unwrap()["state"], "granted");
        w.child_pid = Some(pid);
        w.started_at_ms = Some(unix_ms() - 30_000);
        poll(&w, now).unwrap();
        set_usage(pid, 100, 200, 10);
        w.progress = Some(q::Progress { done: 8, total: 9 });
        w.exit_code = Some(0);
        done(&w, now).unwrap();
        let text = std::fs::read_to_string(dir.join(crate::queue_history::FILE)).unwrap();
        let row: HistoryRow = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(crate::queue_history::row_leak(&row), None, "{text}");
        assert_eq!(
            (row.name.as_str(), row.units),
            ("cargo test", Some(9)),
            "{text}"
        );
        assert!((30..=31).contains(&row.ran_s), "{text}");
        for leaked in [
            "/",
            "someone",
            "abc123",
            "SECRET",
            "issue_private_name",
            "manifest",
        ] {
            assert!(!text.contains(leaked), "{leaked}: {text}");
        }
        kill_all(&mut kids);
        reset_for_test(None);
    }
}
