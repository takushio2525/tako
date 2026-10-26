//! launch_gate — agent の起動待ち（FR-2.43.11 / Issue #2413。エピック #1992）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md` の §9.6（入口の棚卸し）と §11-7（#2413 で改めた）。
//!
//! メモリが逼迫しているあいだ（メモリ圧 warn / critical・swap の警戒線・空きの予約割れ）は、agent を
//! 起こす入口（spawn / run・再起動後の復元・`tako sessions resume`・`tako git resolve`）を待たせ、
//! 再起動後の復元は 1 本ずつ時差で起こす。**走っている agent は止めない・殺さない**（ここは起こす前の
//! 判断だけを持つ）。
//!
//! ## 形
//!
//! - 判断は `tako_core::queue::admit_launches`（= 重い処理の順番待ちと**同じ `admit`** を agent の方針の
//!   入力で呼ぶ）。ここは状態（誰が待っていて誰が立ち上がり中か）と、待っていた起動を GUI の 500 ms tick
//!   （[`tick`]）で起こすことだけを持つ
//! - 入口（UI スレッドの dispatch / 起動時の復元）は [`request`] で問う。`Now` ならその場で起こし、
//!   `Wait` なら起こし方（[`LaunchAction`]）をここへ預けて**すぐ答える**（MCP の呼び出しを固めない。
//!   master は spawn の応答・`tako_orchestrator_workers`・`worker_status`・`run_status`・`tako_queue` で追う）
//! - [`tick`] は覚え（機の値・ペインのメモリ）を読むだけで**採らない**（UI スレッドの番犬
//!   `issue1993_resources_ui_thread_watchdog` の対象）
//! - 立ち上がり中（起こしてから [`STARTUP_SECS`]、または見込みに届くまで）の agent は、まだ使っていない
//!   見込みを空きから引く（立て続けの起動を重ねない）。立ち上がりを終えたら最大メモリを過去の記録
//!   （`<data_dir>/resources/history.jsonl`。区分 `agent`）へ足し、次の見込みに使う
//! - 明示の上書きは [`start_now`]（`tako queue start <ペイン>` / MCP `tako_queue` の `start`）
//! - 本番の GUI（とセカンダリ）で有効。隔離 / セルフテストは注入 [`ENV_INJECT`] があるときだけ
//!   （逼迫は注入で再現する = 負荷を生成しない）。A/B [`ENV_LEGACY`] は修正前（待たずに起こす）

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use serde_json::{json, Value};
use tako_core::queue::{
    self as q, Admission, Estimate, HistoryRow, JobKind, LimitSettings, MachineView, Priority,
    RunningJob, WaitReason, WaitingJob,
};
use tako_core::resources::Pressure;
use tako_core::PaneId;

use crate::host::ControlHost;

/// A/B: `TAKO_2413_LEGACY=1` で修正前（逼迫中でも待たずに起こす・復元は一斉）。GUI のプロセスの env で効く
pub const ENV_LEGACY: &str = "TAKO_2413_LEGACY";
/// 隔離 / セルフテストの GUI だけが読む注入（機の値・立ち上がりの長さ。JSON ファイル）。
/// ファイルは tick のたびに（変わっていれば）読み直すので、待っている間に normal ↔ critical を往復できる
pub const ENV_INJECT: &str = "TAKO_2413_INJECT";

/// 起こしてから「立ち上がり中」とみなす長さ（秒）。この間はまだ使っていない見込みを空きから引き、
/// 復元の列を塞ぐ（= 復元の時差）。claude の立ち上がり（本体 + MCP サーバー）が落ち着くまでの実測から決めた
/// （設計書 §9.6）
pub const STARTUP_SECS: u64 = 20;
/// 待ち・立ち上がり中がある間、走査を 2 秒ごとに保つ長さ（tick のたびに延ばす）
const KEEP_SCANNING: Duration = Duration::from_secs(10);
/// 設定ファイルの mtime を見直す間隔（件があるときだけ）
const SETTINGS_RECHECK: Duration = Duration::from_secs(10);
/// 注入ファイルを見直す間隔
const INJECT_RECHECK: Duration = Duration::from_secs(1);

/// 起動待ちが有効か
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 本番（機の実際の値で判断する）
    On,
    /// 隔離 / セルフテストの GUI で注入を読む（[`ENV_INJECT`]）
    Injected,
    /// 待たせない（理由）
    Off(&'static str),
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Injected => "injected",
            Self::Off(_) => "off",
        }
    }

    fn active(self) -> bool {
        !matches!(self, Self::Off(_))
    }
}

/// このプロセスの起動待ちの形（純関数）。A/B が最優先。隔離 / セルフテストは注入があるときだけ
/// （機の実際のメモリ圧で検証用の GUI が固まらないように。重い処理の調停役が隔離で調停しないのと同じ考え方）
pub fn mode_for(isolated: bool, self_test: bool, legacy: bool, inject: Option<&str>) -> Mode {
    if legacy {
        return Mode::Off("A/B（TAKO_2413_LEGACY=1）で修正前の挙動");
    }
    if isolated || self_test {
        return match inject.filter(|s| !s.is_empty()) {
            Some(_) => Mode::Injected,
            None => Mode::Off("隔離 / セルフテストの tako（注入 TAKO_2413_INJECT が無い）"),
        };
    }
    Mode::On
}

/// どの入口からの起動か
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// `tako_orchestrator_spawn`（`run` も中で spawn する）
    Spawn,
    /// GUI の起動時の会話の復元（#1076 / #1238）。[`q::RESTORE_LANE`] の列で 1 本ずつ
    Restore,
    /// `tako sessions resume`（#112 / #1069）
    SessionResume,
    /// `tako git resolve`（#496 のコンフリクト解消エージェント）
    ConflictResolver,
}

impl LaunchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Restore => "restore",
            Self::SessionResume => "session_resume",
            Self::ConflictResolver => "conflict_resolver",
        }
    }
}

/// 起こすときの初回の依頼文
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstPrompt {
    /// spawn の初回（送達の結果を worker レジストリへ残す = `queue_spawn_prompt_flow`）
    Spawn(String),
    /// それ以外の初回（`queue_prompt_flow`）
    Follow(String),
}

/// 待っていた起動を起こすときにすること（入口ごとの起こし方をそのまま預かる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAction {
    /// 新しいペインの素のシェルへ起動コマンドを送達確認つきで打ち（`queue_command_flow`）、
    /// 続けて初回の依頼文を送る
    Command {
        command: String,
        prompt: Option<FirstPrompt>,
    },
    /// 復元: 新しいログインシェルの入力へ resume の行（改行つき）を書く
    Write(Vec<u8>),
}

/// 入口が問う 1 件
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub pane: u64,
    pub kind: LaunchKind,
    /// claude / codex / agy …（見込みの過去の記録を系統ごとに引く）
    pub agent: String,
    /// 表示名（ペインのタイトル等。persist.log には出さない）
    pub label: String,
    /// ペインの役割（`orchestrator-master` / `orchestrator-worker:…` …。並びの優先度に使う）
    pub role: Option<String>,
    pub action: LaunchAction,
}

/// [`request`] の答え
#[derive(Debug)]
pub enum Decision {
    /// 今起こす（入口がその場で起こす）
    Now(LaunchAction),
    /// 待たせた（起こし方は預かった。応答に載せる中身）
    Wait(Value),
}

/// ペインの役割から並びの優先度（master / solo が先 = 再起動後は master から戻る）
pub fn priority_for_role(role: Option<&str>) -> Priority {
    match role {
        Some(r)
            if r.starts_with("orchestrator-master")
                || r.starts_with("orchestrator-solo")
                || r == "solo"
                || r.starts_with("solo:") =>
        {
            Priority::High
        }
        _ => Priority::Normal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    /// 起こすと決めた（次の tick で起こす）。立ち上がり中に数える
    Due,
    /// 起こした
    Starting(Instant),
}

#[derive(Debug, Clone)]
struct Entry {
    kind: LaunchKind,
    agent: String,
    label: String,
    priority: Priority,
    /// 受付の時刻（Unix ミリ秒。並びに使う）
    enqueued_ms: u64,
    enqueued: Instant,
    estimate: Estimate,
    action: Option<LaunchAction>,
    phase: Phase,
    /// 明示の上書きで起こした
    forced: bool,
    /// 一度でも待った（起こしたときに記録・worker レジストリの起点を直す）
    waited: bool,
    used_mb: Option<u64>,
    peak_mb: u64,
    peak_at: Option<Instant>,
}

impl Entry {
    fn lane(&self) -> Option<String> {
        (self.kind == LaunchKind::Restore).then(|| q::RESTORE_LANE.to_string())
    }

    /// 立ち上がり中の 1 本として（`admit` の実行中の形）
    fn starting_view(&self, pane: u64, now: Instant) -> RunningJob {
        let since = match self.phase {
            Phase::Starting(at) => Some(at),
            _ => None,
        };
        let secs = |t: Instant| since.map_or(0, |s| t.saturating_duration_since(s).as_secs());
        RunningJob {
            id: pane,
            estimate_mb: self.estimate.mb,
            used_mb: self.used_mb,
            target_dir: self.lane(),
            stalled: false,
            peak_mb: self.peak_mb,
            ran_s: secs(now),
            peak_seen_s: self.peak_at.map(secs),
            expected_peak_at_s: self.estimate.peak_at_s,
        }
    }
}

/// 注入（[`ENV_INJECT`] のファイルの中身）。**機の値は注入で与え、負荷は生成しない**
#[derive(Debug, Default, Clone, Deserialize)]
struct Inject {
    total_mb: Option<u64>,
    available_mb: Option<u64>,
    pressure: Option<String>,
    swap_used_mb: Option<u64>,
    /// 立ち上がり中とみなす長さ（秒。既定 [`STARTUP_SECS`]）
    startup_secs: Option<u64>,
}

impl Inject {
    fn machine(&self) -> Option<MachineView> {
        let any = self.total_mb.is_some()
            || self.available_mb.is_some()
            || self.pressure.is_some()
            || self.swap_used_mb.is_some();
        any.then(|| MachineView {
            total_mb: self.total_mb.unwrap_or(0),
            available_mb: self.available_mb,
            pressure: self.pressure.as_deref().and_then(|p| match p {
                "normal" => Some(Pressure::Normal),
                "warn" => Some(Pressure::Warn),
                "critical" => Some(Pressure::Critical),
                _ => None,
            }),
            swap_used_mb: self.swap_used_mb,
        })
    }
}

#[derive(Default)]
struct Gate {
    /// `None` = 有効にしていない（CLI のプロセス・単体テスト）
    mode: Option<Mode>,
    entries: BTreeMap<u64, Entry>,
    /// 過去の記録のうち区分 `agent` の行（古い順）
    history: Vec<HistoryRow>,
    history_dir: Option<PathBuf>,
    settings: LimitSettings,
    settings_seen: Option<(Instant, Option<SystemTime>)>,
    inject_path: Option<PathBuf>,
    inject: Option<(Instant, Option<SystemTime>, Inject)>,
    usage_at: Option<Instant>,
    /// 直近の判断（待ちそれぞれの理由と並び）
    last: Admission,
    /// 機の値を固定する（**単体テストだけ**。本番は常に `None`）
    fixed_machine: Option<Option<MachineView>>,
    /// 立ち上がりの長さを固定する（**単体テストだけ**）
    fixed_startup: Option<Duration>,
}

#[cfg(not(test))]
fn gate() -> &'static Mutex<Gate> {
    static G: std::sync::OnceLock<Mutex<Gate>> = std::sync::OnceLock::new();
    G.get_or_init(Mutex::default)
}

/// 単体テストはスレッドごとに別の状態を持つ（dispatch の spawn のテストと並走しても、
/// 起動待ちを有効にしたテストの状態が漏れない）
#[cfg(test)]
fn gate() -> &'static Mutex<Gate> {
    thread_local! {
        static G: &'static Mutex<Gate> = Box::leak(Box::default());
    }
    G.with(|g| *g)
}

/// このスレッドの起動待ちを、機の値と立ち上がりの長さを固定して有効にする（単体テストだけ）
#[cfg(test)]
pub(crate) fn enable_for_test(machine: Option<MachineView>, startup: Duration) {
    let mut g = lock();
    *g = Gate::default();
    g.mode = Some(Mode::On);
    g.fixed_machine = Some(machine);
    g.fixed_startup = Some(startup);
}

/// 機の値を差し替える（単体テストだけ。待っている間の normal ↔ critical の往復）
#[cfg(test)]
pub(crate) fn set_machine_for_test(machine: Option<MachineView>) {
    lock().fixed_machine = Some(machine);
}

fn lock() -> std::sync::MutexGuard<'static, Gate> {
    gate().lock().unwrap_or_else(|p| p.into_inner())
}

/// GUI の起動時に有効にする（過去の記録の `agent` の行を 1 回だけ読む）
pub fn enable(mode: Mode) {
    let mut g = lock();
    if g.mode.is_none() && mode.active() {
        g.history_dir = crate::queue_history::dir();
        g.history = g
            .history_dir
            .as_deref()
            .map(crate::queue_history::load)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.kind == JobKind::Agent)
            .collect();
        g.settings = crate::settings::load().resources;
        g.settings_seen = Some((Instant::now(), settings_mtime()));
    }
    if mode == Mode::Injected {
        g.inject_path = std::env::var_os(ENV_INJECT).map(PathBuf::from);
    }
    g.mode = Some(mode);
}

/// このプロセスの形（有効にしていなければ `None`）
pub fn mode() -> Option<Mode> {
    lock().mode
}

/// A/B の env（GUI の起動時に [`mode_for`] へ渡す）
pub fn legacy() -> bool {
    std::env::var_os(ENV_LEGACY).is_some_and(|v| v == "1")
}

fn unix_ms() -> u64 {
    crate::queue::unix_ms()
}

fn settings_mtime() -> Option<SystemTime> {
    crate::settings::settings_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// 設定（予約・swap の警戒線）と注入を必要なら読み直す（件があるときだけ呼ばれる）
fn refresh_inputs(g: &mut Gate, now: Instant) {
    if g.fixed_machine.is_some() {
        return;
    }
    let due = g
        .settings_seen
        .is_none_or(|(at, _)| now.duration_since(at) >= SETTINGS_RECHECK);
    if due {
        let mtime = settings_mtime();
        if g.settings_seen.is_none_or(|(_, seen)| seen != mtime) {
            g.settings = crate::settings::load().resources;
        }
        g.settings_seen = Some((now, mtime));
    }
    // 注入（隔離 / セルフテストの検証だけ）。変わっていれば読み直す
    let Some(path) = g.inject_path.clone() else {
        return;
    };
    let recheck = g
        .inject
        .as_ref()
        .is_none_or(|(at, _, _)| now.duration_since(*at) >= INJECT_RECHECK);
    if !recheck {
        return;
    }
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let changed = g.inject.as_ref().is_none_or(|(_, seen, _)| *seen != mtime);
    let value = if changed {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Inject>(&t).ok())
            .unwrap_or_default()
    } else {
        g.inject.take().map(|(_, _, v)| v).unwrap_or_default()
    };
    g.inject = Some((now, mtime, value));
}

fn startup_window(g: &Gate) -> Duration {
    if let Some(fixed) = g.fixed_startup {
        return fixed;
    }
    let secs = g
        .inject
        .as_ref()
        .and_then(|(_, _, i)| i.startup_secs)
        .unwrap_or(STARTUP_SECS);
    Duration::from_secs(secs)
}

/// 判断に使う機の値（採らない = 背景の採取の覚えを読むだけ）
fn machine(g: &Gate) -> Option<MachineView> {
    if let Some(fixed) = g.fixed_machine {
        return fixed;
    }
    if let Some(view) = g.inject.as_ref().and_then(|(_, _, i)| i.machine()) {
        return Some(view);
    }
    crate::resources::machine_stats().and_then(|s| q::machine_view(&s))
}

/// 立ち上がり中のペインの使用量を走査の覚えから取り込む（新しい走査が来たときだけ）
fn refresh_usage(g: &mut Gate) {
    if g.fixed_machine.is_some() {
        return;
    }
    let Some((at, panes)) = crate::resources::pane_memory(g.usage_at) else {
        return;
    };
    g.usage_at = Some(at);
    for (pane, e) in g.entries.iter_mut() {
        if !matches!(e.phase, Phase::Starting(_)) {
            continue;
        }
        if let Some(mb) = panes.get(pane).copied() {
            e.used_mb = Some(mb);
            if mb > e.peak_mb {
                e.peak_mb = mb;
                e.peak_at = Some(at);
            }
        }
    }
}

/// 立ち上がりを終えた（窓を過ぎた / 見込みに届いた）件を外し、過去の記録の行を返す
fn settle(g: &mut Gate, now: Instant) -> Vec<HistoryRow> {
    let window = startup_window(g);
    let done: Vec<u64> = g
        .entries
        .iter()
        .filter(|(_, e)| match e.phase {
            Phase::Starting(at) => {
                now.saturating_duration_since(at) >= window
                    || (e.peak_mb > 0 && e.peak_mb >= e.estimate.mb)
            }
            _ => false,
        })
        .map(|(p, _)| *p)
        .collect();
    let mut rows = Vec::new();
    for pane in done {
        let Some(e) = g.entries.remove(&pane) else {
            continue;
        };
        if e.peak_mb == 0 {
            continue;
        }
        let view = e.starting_view(pane, now);
        let keys = q::agent_history_keys(&e.agent);
        rows.push(HistoryRow {
            at: unix_ms() / 1000,
            key: keys.key,
            shape: None,
            name: keys.name,
            kind: JobKind::Agent,
            peak_mb: e.peak_mb,
            peak_at_s: view.peak_seen_s,
            ran_s: view.ran_s,
            exit_code: None,
        });
    }
    rows
}

/// 判断して、入れた件を「起こす」にする（明示の上書きは判断を通さない）
fn decide(g: &mut Gate, now: Instant) {
    let machine = machine(g);
    let limits = q::effective_limits(&g.settings, machine.map(|m| m.total_mb).filter(|t| *t > 0));
    let starting: Vec<RunningJob> = g
        .entries
        .iter()
        .filter(|(_, e)| matches!(e.phase, Phase::Due | Phase::Starting(_)))
        .map(|(p, e)| e.starting_view(*p, now))
        .collect();
    let waiting: Vec<WaitingJob> = g
        .entries
        .iter()
        .filter(|(_, e)| e.phase == Phase::Waiting)
        .map(|(p, e)| WaitingJob {
            id: *p,
            estimate_mb: e.estimate.mb,
            target_dir: e.lane(),
            priority: e.priority,
            enqueued_at_ms: e.enqueued_ms,
            overtaken: 0,
        })
        .collect();
    let admission = q::admit_launches(machine, limits, &starting, &waiting);
    for pane in &admission.grants {
        if let Some(e) = g.entries.get_mut(pane) {
            e.phase = Phase::Due;
        }
    }
    g.last = admission;
}

/// 待っている件の (理由のコード, 理由の文)。並びが止まって調べなかった件は止めている理由を添える
fn reason_of(last: &Admission, pane: u64) -> (String, String) {
    let Some(own) = last.reason_of(pane) else {
        return ("pending".into(), "判断待ち".into());
    };
    match (own, last.holding_of(pane)) {
        (WaitReason::Ahead, Some(hold)) if *hold != WaitReason::Ahead => (
            hold.code().to_string(),
            format!(
                "{}（{}）",
                q::launch_reason_text(own),
                q::launch_reason_text(hold)
            ),
        ),
        _ => (own.code().to_string(), q::launch_reason_text(own)),
    }
}

/// 上書きの案内（最も簡単な形 = #322）
fn start_now_command(pane: u64) -> String {
    format!("tako queue start {pane}")
}

fn view_of(g: &Gate, pane: u64, now: Instant) -> Option<Value> {
    let e = g.entries.get(&pane)?;
    let base = json!({
        "kind": e.kind.as_str(),
        "agent": e.agent,
        "estimate_mb": e.estimate.mb,
        "estimate_from": e.estimate.from.as_str(),
    });
    let mut v = base;
    match e.phase {
        Phase::Waiting => {
            let order: Vec<u64> = g
                .last
                .order
                .iter()
                .copied()
                .filter(|p| g.entries.get(p).is_some_and(|x| x.phase == Phase::Waiting))
                .collect();
            let position = order.iter().position(|p| *p == pane).map(|i| i + 1);
            let (code, text) = reason_of(&g.last, pane);
            v["state"] = json!("waiting");
            v["code"] = json!(code);
            v["reason"] = json!(text);
            v["position"] = json!(position);
            v["waiting"] = json!(order.len());
            v["waited_s"] = json!(now.saturating_duration_since(e.enqueued).as_secs());
            v["start_now"] = json!(start_now_command(pane));
        }
        Phase::Due => {
            v["state"] = json!("starting");
            v["since_s"] = json!(0);
        }
        Phase::Starting(at) => {
            v["state"] = json!("starting");
            v["since_s"] = json!(now.saturating_duration_since(at).as_secs());
            v["memory_mb"] = json!(e.used_mb);
        }
    }
    if e.forced {
        v["forced"] = json!(true);
    }
    Some(v)
}

/// 入口が問う（UI スレッド）。待たせない（無効・A/B）なら即 `Now`。
/// 同じペインに前の起動待ちがあれば置き換える（同じペインへ 2 本起こさない）
pub fn request(req: LaunchRequest) -> Decision {
    let mut g = lock();
    if !g.mode.is_some_and(Mode::active) {
        return Decision::Now(req.action);
    }
    let now = Instant::now();
    refresh_inputs(&mut g, now);
    let keys = q::agent_history_keys(&req.agent);
    let estimate = q::estimate(JobKind::Agent, Some(&keys), &g.history);
    let pane = req.pane;
    g.entries.insert(
        pane,
        Entry {
            kind: req.kind,
            agent: req.agent,
            label: req.label,
            priority: priority_for_role(req.role.as_deref()),
            enqueued_ms: unix_ms(),
            enqueued: now,
            estimate,
            action: Some(req.action),
            phase: Phase::Waiting,
            forced: false,
            waited: false,
            used_mb: None,
            peak_mb: 0,
            peak_at: None,
        },
    );
    decide(&mut g, now);
    crate::resources::keep_scanning(now + KEEP_SCANNING);
    let Some(e) = g.entries.get_mut(&pane) else {
        unreachable!("直前に入れた件");
    };
    if e.phase == Phase::Due {
        e.phase = Phase::Starting(now);
        let action = e.action.take().expect("預けた起こし方");
        return Decision::Now(action);
    }
    e.waited = true;
    let kind = e.kind;
    let view = view_of(&g, pane, now).unwrap_or(Value::Null);
    crate::diag::persist_log(&format!(
        "[launch-gate] 待ち pane={pane} 入口={} 理由={} 順番={}/{}",
        kind.as_str(),
        view["code"].as_str().unwrap_or("?"),
        view["position"],
        view["waiting"],
    ));
    Decision::Wait(view)
}

/// 起こすと決まった 1 件
struct Due {
    pane: u64,
    kind: LaunchKind,
    action: LaunchAction,
    waited_s: Option<u64>,
    forced: bool,
}

/// GUI の 500 ms tick（UI スレッド）。覚えを読むだけで採らない。
/// 閉じられたペインを外し・立ち上がりを終えた件を記録し・待ちを判断して、起こすと決めた件を起こす
pub fn tick(host: &mut dyn ControlHost) {
    let (due, rows, dir) = {
        let mut g = lock();
        if g.entries.is_empty() {
            return;
        }
        let now = Instant::now();
        let gone: Vec<u64> = g
            .entries
            .keys()
            .copied()
            .filter(|p| {
                host.workspace()
                    .pane_anywhere(PaneId::from_raw(*p))
                    .is_none()
            })
            .collect();
        for pane in gone {
            if let Some(e) = g.entries.remove(&pane) {
                if e.phase == Phase::Waiting {
                    crate::diag::persist_log(&format!(
                        "[launch-gate] 起動待ちのペインが閉じられた pane={pane} 入口={}",
                        e.kind.as_str()
                    ));
                }
            }
        }
        refresh_inputs(&mut g, now);
        refresh_usage(&mut g);
        let rows = settle(&mut g, now);
        if g.mode.is_some_and(Mode::active) {
            decide(&mut g, now);
        } else {
            // 有効でなくなった（A/B の切り替え等）なら預かっている起動はすべて起こす（取り残さない）
            for e in g.entries.values_mut() {
                if e.phase == Phase::Waiting {
                    e.phase = Phase::Due;
                }
            }
        }
        let mut due = Vec::new();
        for (pane, e) in g.entries.iter_mut() {
            if e.phase != Phase::Due {
                continue;
            }
            e.phase = Phase::Starting(now);
            if let Some(action) = e.action.take() {
                due.push(Due {
                    pane: *pane,
                    kind: e.kind,
                    action,
                    waited_s: e
                        .waited
                        .then(|| now.saturating_duration_since(e.enqueued).as_secs()),
                    forced: e.forced,
                });
            }
        }
        if !g.entries.is_empty() {
            crate::resources::keep_scanning(now + KEEP_SCANNING);
        }
        if !rows.is_empty() {
            g.history.extend(rows.iter().cloned());
            crate::queue_history::trim(&mut g.history);
        }
        (due, rows, g.history_dir.clone())
    };
    // 過去の記録へ足す（ファイルの追記は UI スレッドの外で）
    if let (false, Some(dir)) = (rows.is_empty(), dir) {
        std::thread::spawn(move || {
            for row in rows {
                if let Err(e) = crate::queue_history::append(&dir, &row) {
                    crate::diag::persist_log(&format!("[launch-gate] 過去の記録へ足せない: {e}"));
                }
            }
        });
    }
    for d in due {
        perform(host, d.pane, d.action);
        if let Some(waited) = d.waited_s {
            let how = if d.forced {
                "上書きで起動"
            } else {
                "起動"
            };
            crate::diag::persist_log(&format!(
                "[launch-gate] {how} pane={} 入口={} 待ち={waited}s",
                d.pane,
                d.kind.as_str()
            ));
            // worker レジストリの起点（送達の猶予 = spawned_at）を起こした時刻へ直す。
            // 待った分で「依頼文が届いていない」と誤って言わない（#390 の猶予は起動からの秒数）
            if d.kind == LaunchKind::Spawn {
                if let Err(e) = crate::orchestrator::registry::record_launch_started(d.pane) {
                    eprintln!("warning: worker レジストリの起点を直せない: {e}");
                }
            }
        }
    }
}

/// 預かった起こし方を実行する（入口がその場で起こすときと同じ手順）
pub fn perform(host: &mut dyn ControlHost, pane: u64, action: LaunchAction) {
    let id = PaneId::from_raw(pane);
    match action {
        LaunchAction::Command { command, prompt } => {
            host.queue_command_flow(id, command);
            match prompt {
                Some(FirstPrompt::Spawn(p)) => host.queue_spawn_prompt_flow(id, p),
                Some(FirstPrompt::Follow(p)) => host.queue_prompt_flow(id, p),
                None => {}
            }
        }
        LaunchAction::Write(bytes) => {
            if let Some(session) = host.session(id) {
                session.write(bytes);
            }
        }
    }
}

/// 明示の上書き（`tako queue start <ペイン>` / MCP `tako_queue` の `start`。接続のスレッドから呼ぶ）。
/// 次の tick（500 ms 以内）で起こす。呼んだこと自体を同意とみなす
pub fn start_now(pane: u64) -> Result<Value, String> {
    let mut g = lock();
    let now = Instant::now();
    let (code, text) = reason_of(&g.last, pane);
    let Some(e) = g.entries.get_mut(&pane) else {
        return Err(format!(
            "ペイン {pane} に起動待ちは無い（起こした後か、閉じられた。待っている起動は `tako queue` の launches）"
        ));
    };
    let was_waiting = e.phase == Phase::Waiting;
    if was_waiting {
        e.phase = Phase::Due;
        e.forced = true;
    }
    Ok(json!({
        "pane": pane,
        "kind": e.kind.as_str(),
        "label": e.label,
        "state": if was_waiting { "starting" } else { "already_started" },
        "waited_s": now.saturating_duration_since(e.enqueued).as_secs(),
        "was": was_waiting.then(|| json!({ "code": code, "reason": text })),
    }))
}

/// ペインの起動待ちの状態（応答の `launch_gate`。無ければ `None` = 待っていない・立ち上がりも終えた）
pub fn view(pane: u64) -> Option<Value> {
    let g = lock();
    view_of(&g, pane, Instant::now())
}

/// ペインの起動を待たせているか（起こすと決めた直後を含む = まだ agent が居ない）
pub fn is_waiting(pane: u64) -> bool {
    lock()
        .entries
        .get(&pane)
        .is_some_and(|e| matches!(e.phase, Phase::Waiting | Phase::Due))
}

/// 待っている起動の 1 行（人が読む。CLI の `orchestrator run` / `tako queue` が出す）。待っていなければ `None`
pub fn wait_line(view: &Value) -> Option<String> {
    if view["state"] != "waiting" {
        return None;
    }
    Some(format!(
        "agent の起動を待っている（{}・順番 {}/{}）。圧が戻ると起きる。今すぐ起こすには: {}",
        view["reason"].as_str().unwrap_or("?"),
        view["position"],
        view["waiting"],
        view["start_now"]
            .as_str()
            .unwrap_or("tako queue start <ペイン>"),
    ))
}

/// `tako queue`（status）の `launches` 節
pub fn status_section() -> Value {
    let g = lock();
    let now = Instant::now();
    let Some(mode) = g.mode else {
        return json!({ "mode": "off", "reason": "この tako は起動待ちを持たない" });
    };
    let mut waiting: Vec<(usize, Value)> = Vec::new();
    let mut starting: Vec<Value> = Vec::new();
    for (pane, e) in &g.entries {
        let Some(mut v) = view_of(&g, *pane, now) else {
            continue;
        };
        v["pane"] = json!(pane);
        v["label"] = json!(e.label);
        v["priority"] = json!(e.priority.as_str());
        if e.phase == Phase::Waiting {
            let pos = v["position"].as_u64().unwrap_or(u64::MAX) as usize;
            waiting.push((pos, v));
        } else {
            starting.push(v);
        }
    }
    waiting.sort_by_key(|(p, _)| *p);
    let mut out = json!({
        "mode": mode.as_str(),
        "waiting": waiting.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "starting": starting,
        "startup_secs": startup_window(&g).as_secs(),
    });
    if let Mode::Off(reason) = mode {
        out["reason"] = json!(reason);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024;

    fn machine(pressure: Pressure) -> Option<MachineView> {
        Some(MachineView {
            total_mb: 36 * GIB,
            available_mb: Some(20 * GIB),
            pressure: Some(pressure),
            swap_used_mb: Some(0),
        })
    }

    fn req(pane: u64, kind: LaunchKind, role: Option<&str>) -> LaunchRequest {
        LaunchRequest {
            pane,
            kind,
            agent: "claude".into(),
            label: format!("pane {pane}"),
            role: role.map(str::to_string),
            action: LaunchAction::Command {
                command: format!("claude #{pane}"),
                prompt: Some(FirstPrompt::Spawn("依頼".into())),
            },
        }
    }

    fn waiting(d: Decision) -> Value {
        match d {
            Decision::Wait(v) => v,
            Decision::Now(a) => panic!("待たずに起こした: {a:?}"),
        }
    }

    #[test]
    fn 形はab_隔離_注入で決まる() {
        assert_eq!(mode_for(false, false, false, None), Mode::On);
        assert!(matches!(mode_for(false, false, true, None), Mode::Off(_)));
        assert!(matches!(mode_for(true, false, false, None), Mode::Off(_)));
        assert!(matches!(
            mode_for(false, true, false, Some("")),
            Mode::Off(_)
        ));
        assert_eq!(
            mode_for(true, false, false, Some("/x.json")),
            Mode::Injected
        );
        // 本番は注入を読まない
        assert_eq!(mode_for(false, false, false, Some("/x.json")), Mode::On);
        // A/B は注入より強い（同じ注入で修正前を再現する）
        assert!(matches!(
            mode_for(true, false, true, Some("/x.json")),
            Mode::Off(_)
        ));
    }

    #[test]
    fn 有効にしていなければ待たずに起こす() {
        // このスレッドの状態は空（CLI のプロセス・他のテストと同じ）
        assert!(matches!(
            request(req(1, LaunchKind::Spawn, None)),
            Decision::Now(_)
        ));
        assert!(!is_waiting(1));
        assert_eq!(status_section()["mode"], "off");
    }

    #[test]
    fn warnとcriticalで待ち_理由と順番と上書きの案内を返す() {
        enable_for_test(machine(Pressure::Warn), Duration::from_secs(60));
        let v = waiting(request(req(10, LaunchKind::Spawn, None)));
        assert_eq!(
            (
                v["state"].as_str(),
                v["code"].as_str(),
                v["position"].as_u64()
            ),
            (Some("waiting"), Some("pressure_warn"), Some(1))
        );
        assert!(v["reason"].as_str().unwrap().contains("normal へ戻るまで"));
        assert_eq!(v["start_now"], "tako queue start 10");
        let v = waiting(request(req(11, LaunchKind::Spawn, None)));
        assert_eq!(
            (v["position"].as_u64(), v["waiting"].as_u64()),
            (Some(2), Some(2))
        );
        assert!(is_waiting(10) && is_waiting(11));
        set_machine_for_test(machine(Pressure::Critical));
        let v = waiting(request(req(12, LaunchKind::Spawn, None)));
        assert_eq!(v["code"], "pressure_critical");
        let s = status_section();
        assert_eq!(s["waiting"].as_array().map(Vec::len), Some(3));
        assert_eq!(s["waiting"][0]["pane"], 10);
    }

    #[test]
    fn 明示の上書きは逼迫中でも起こす() {
        enable_for_test(machine(Pressure::Critical), Duration::from_secs(60));
        waiting(request(req(20, LaunchKind::Spawn, None)));
        let r = start_now(20).expect("待っている件");
        assert_eq!(r["state"], "starting");
        assert_eq!(r["was"]["code"], "pressure_critical");
        // 起こすと決めた（まだ agent は居ない）= 待ちの印は残り、状態は立ち上がり
        assert!(is_waiting(20));
        assert_eq!(view(20).unwrap()["state"], "starting");
        assert!(start_now(99).is_err(), "待っていないペイン");
        assert_eq!(start_now(20).unwrap()["state"], "already_started");
    }

    #[test]
    fn 復元は1本ずつでmasterが先_spawnは列を追い越す() {
        enable_for_test(machine(Pressure::Normal), Duration::from_secs(60));
        // 1 本目は即座に起こす（その場で書く）
        assert!(matches!(
            request(req(1, LaunchKind::Restore, Some("orchestrator-worker:p"))),
            Decision::Now(_)
        ));
        let v = waiting(request(req(
            2,
            LaunchKind::Restore,
            Some("orchestrator-worker:p"),
        )));
        assert_eq!(v["code"], "target_dir");
        assert!(v["reason"].as_str().unwrap().contains("1 本ずつ"));
        // 後から来た master の復元が worker より先に並ぶ
        waiting(request(req(
            3,
            LaunchKind::Restore,
            Some("orchestrator-master:x"),
        )));
        assert_eq!(view(3).unwrap()["position"], 1);
        assert_eq!(view(2).unwrap()["position"], 2);
        // spawn は復元の列の待ちを追い越して起きる
        assert!(matches!(
            request(req(4, LaunchKind::Spawn, None)),
            Decision::Now(_)
        ));
    }

    #[test]
    fn 立ち上がりを終えた件は外れ_次の復元が起きる() {
        enable_for_test(machine(Pressure::Normal), Duration::ZERO);
        assert!(matches!(
            request(req(1, LaunchKind::Restore, None)),
            Decision::Now(_)
        ));
        waiting(request(req(2, LaunchKind::Restore, None)));
        // 立ち上がりの長さ 0 = 次の判断で 1 本目は外れ、2 本目が「起こす」になる
        let mut g = lock();
        let now = Instant::now();
        let rows = settle(&mut g, now);
        assert!(rows.is_empty(), "最大 0 の件は記録しない");
        decide(&mut g, now);
        assert!(!g.entries.contains_key(&1));
        assert_eq!(g.entries[&2].phase, Phase::Due);
    }

    #[test]
    fn 立ち上がりの最大を過去の記録の形で残す() {
        enable_for_test(machine(Pressure::Normal), Duration::ZERO);
        assert!(matches!(
            request(req(5, LaunchKind::Spawn, None)),
            Decision::Now(_)
        ));
        let mut g = lock();
        let e = g.entries.get_mut(&5).unwrap();
        e.peak_mb = 420;
        e.used_mb = Some(400);
        let rows = settle(&mut g, Instant::now());
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            (r.kind, r.peak_mb, r.name.as_str()),
            (JobKind::Agent, 420, "claude")
        );
        assert_eq!(r.key, q::agent_history_keys("claude").key);
        // 生の起動コマンド・ラベルは記録に載らない
        let line = serde_json::to_string(r).unwrap();
        assert!(!line.contains("pane 5") && !line.contains("#5"), "{line}");
    }

    #[test]
    fn 待っている間に圧が往復しても_normalの間に起こす() {
        enable_for_test(machine(Pressure::Critical), Duration::from_secs(60));
        waiting(request(req(7, LaunchKind::Spawn, None)));
        let mut g = lock();
        let now = Instant::now();
        g.fixed_machine = Some(machine(Pressure::Normal));
        decide(&mut g, now);
        assert_eq!(g.entries[&7].phase, Phase::Due);
        // 起こすと決めた後に critical へ戻っても取り消さない（決めた起動は起こす）
        g.fixed_machine = Some(machine(Pressure::Critical));
        decide(&mut g, now);
        assert_eq!(g.entries[&7].phase, Phase::Due);
    }

    #[test]
    fn 役割から優先度() {
        assert_eq!(
            priority_for_role(Some("orchestrator-master")),
            Priority::High
        );
        assert_eq!(
            priority_for_role(Some("orchestrator-solo:fast")),
            Priority::High
        );
        assert_eq!(
            priority_for_role(Some("orchestrator-worker:tako:x")),
            Priority::Normal
        );
        assert_eq!(priority_for_role(None), Priority::Normal);
        // 会話の再開（sessions resume）が付ける solo の綴り
        assert_eq!(priority_for_role(Some("solo:fast")), Priority::High);
        assert_eq!(priority_for_role(Some("solo")), Priority::High);
        assert_eq!(
            priority_for_role(Some("conflict-resolver")),
            Priority::Normal
        );
    }
}
