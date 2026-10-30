//! PC リソースの採取と読み取りの口（FR-2.43 / Issue #1993。エピック #1992 の S1）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§2・§4.1・§8）。
//!
//! ## 形
//!
//! - 採るのは**背景スレッド**（[`start`]）: 機全体は 2 秒ごと、プロセスの走査は誰も見ていなければ 30 秒ごと
//!   （CLI / MCP の status を受けてから [`WANTED_FOR`] の間は 2 秒ごと）。
//!   OS への問い合わせは境界 B29（`tako_core::platform::resources`）、解釈は
//!   `tako_core::resources` の純関数
//! - **UI スレッドがすること**は 2 つだけ: ペインの一覧を渡す（[`publish_panes`]）と、
//!   覚えを読む（[`cached_status`]）。走査も子プロセスもしない
//! - IPC / MCP の `Request::Queue` は、`ipc::submit` が UI の受け口へ渡す**前に** [`answer`] で答える
//!   （先例: 非同期 run の分岐）。覚えが 2 秒より古ければその接続のスレッドで採り直す。
//!   UI が固まっていても答えられる（#1979 の本番ハングの型を持ち込まない）
//! - GUI が居ないときの CLI は [`machine_only`] で機の状態だけを出す
//!
//! 番犬: `crates/tako-control/tests/issue1993_resources_ui_thread_watchdog.rs`

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_core::platform::resources as b29;
use tako_core::resources::{
    self as core_res, CpuTicks, MachineStats, ManagedView, Owner, ProcKey, ProcSample, Report, MIB,
};

use crate::dispatch::DispatchError;

/// 機全体を採る間隔
pub const MACHINE_INTERVAL: Duration = Duration::from_secs(2);
/// 誰も見ていないときの走査の間隔。
///
/// 設計書 §2.2 は 5 秒としていたが、隔離 GUI（debug）の A/B で 5 秒ごとの走査は
/// 1 コアの +0.57%（走査 1 回 22〜24 ms）と見込みの 2 倍を超えた。見ていない間に要るのは
/// 「次に status を受けたとき CPU% の差分の基準があること」だけなので、30 秒へ落とす
/// （見ている間 = status の後 10 秒・順番待ちが動いている間（#1994）・resources ビュー（#1995）は 2 秒）
pub const SCAN_INTERVAL: Duration = Duration::from_secs(30);
/// 見ている間の走査の間隔（1 コアの約 0.26%）
pub const SCAN_INTERVAL_WANTED: Duration = Duration::from_secs(2);
/// status を受けてから「見ている」とみなす長さ
pub const WANTED_FOR: Duration = Duration::from_secs(10);
/// 器のペインの pid（`tmux list-panes -a` = 子プロセス）を採り直す間隔（A/B の旧挙動。#2018）
pub const BACKEND_INTERVAL: Duration = Duration::from_secs(10);
/// 器の顔ぶれが変わらなくても pid を採り直す保険の間隔（#2018）
pub const BACKEND_INSURANCE: Duration = Duration::from_secs(60);
/// status の `panes` に載せる件数（メモリの多い順）
pub const PANES_SHOWN: usize = 10;

/// CPU% の差分を取るのに要る最小の間隔（これより近い 2 回は差が粒度に負けるので前回の値を使う）
const MIN_CPU_WINDOW: Duration = Duration::from_millis(500);

/// A/B（採取の費用の比較）: 背景スレッドを立てない。status は要求のたびにその場で採る
const NO_SAMPLER_ENV: &str = "TAKO_1993_NO_SAMPLER";

/// 読む env のキー（ペインの番号・どの tako の受け口か・隔離か・順番待ちの受付番号 = #1994・
/// 共有 jobserver の在りか = #1998）
const ENV_KEYS: [&str; 5] = [
    "TAKO_PANE_ID",
    "TAKO_SOCKET",
    "TAKO_ISOLATED",
    crate::queue::ENV_JOB,
    tako_core::jobserver::ENV,
];

/// GUI から渡すペイン 1 つ
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneEntry {
    pub pane: u64,
    /// 所属タブの名前（退避中は由来タブの名前）
    pub tab_title: Option<String>,
    pub title: Option<String>,
    pub role: Option<String>,
    /// たまり場（退避中）か
    pub shelved: bool,
    /// 器の無いペインの PTY 直下の子（シェル）。器のペインでは `None`（直下は attach の客なので）
    pub child_pid: Option<u32>,
    /// 器（tmux / psmux）のセッション名
    pub backend_session: Option<String>,
}

/// 走査 1 回ぶんの覚え
#[derive(Debug, Clone)]
struct ProcSnapshot {
    at: Instant,
    report: Report,
    scanned: usize,
    readable: usize,
    took: Duration,
    interval: Duration,
    /// 走査の生の値（順番待ちの木の集計 = [`managed`] が読む。#1994）
    samples: Arc<Vec<ProcSample>>,
    cpu: Arc<HashMap<ProcKey, f64>>,
}

#[derive(Default)]
struct Published {
    panes: Vec<PaneEntry>,
    own_socket: Option<String>,
    machine: Option<(Instant, MachineStats)>,
    /// 機全体が採れない理由（この OS では未実装 など）
    machine_unavailable: Option<&'static str>,
    procs: Option<ProcSnapshot>,
    wanted_until: Option<Instant>,
}

/// argv / env の覚え（pid と開始時刻で引く）
#[derive(Debug, Clone, Default)]
struct CachedDetails {
    argv: Option<String>,
    pane_id: Option<String>,
    socket: Option<String>,
    isolated: bool,
    queue_job: Option<u64>,
    /// env の `CARGO_MAKEFLAGS` が指す tako の共有 jobserver の fifo（#1998）
    jobserver_fifo: Option<String>,
}

#[derive(Default)]
struct ScanState {
    cpu_ticks: Option<(Instant, CpuTicks)>,
    last_machine_cpu: Option<f64>,
    proc_cpu: HashMap<ProcKey, u64>,
    proc_cpu_at: Option<Instant>,
    last_proc_pct: HashMap<ProcKey, f64>,
    details: HashMap<ProcKey, CachedDetails>,
    reader: Option<b29::DetailsReader>,
    backend: Option<(Instant, Vec<(String, u32)>)>,
}

fn published() -> &'static Mutex<Published> {
    static P: OnceLock<Mutex<Published>> = OnceLock::new();
    P.get_or_init(Mutex::default)
}

fn scan_state() -> &'static Mutex<ScanState> {
    static S: OnceLock<Mutex<ScanState>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

fn lock<T>(m: &'static Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// 背景の採取スレッドを立てる（GUI の起動時に 1 回。2 回目以降は何もしない）。
/// `TAKO_1993_NO_SAMPLER=1` なら立てない（A/B: 採取の費用の比較用）
pub fn start() {
    if std::env::var(NO_SAMPLER_ENV).is_ok_and(|v| v == "1") {
        return;
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("tako-resources".into())
        .spawn(|| loop {
            sampler_tick(Instant::now());
            std::thread::sleep(MACHINE_INTERVAL);
        });
    if spawned.is_err() {
        STARTED.store(false, Ordering::SeqCst);
    }
}

/// 背景スレッドの 1 周: 機全体を採り、走査の時期なら走査する
fn sampler_tick(now: Instant) {
    let mut scan = lock(scan_state());
    refresh_machine(&mut scan, now);
    let (due, interval) = {
        let p = lock(published());
        let interval = scan_interval(p.wanted_until, now);
        let due = p
            .procs
            .as_ref()
            .is_none_or(|s| now.duration_since(s.at) + Duration::from_millis(100) >= interval);
        (due, interval)
    };
    if due {
        refresh_processes(&mut scan, now, interval);
    }
}

/// 走査の間隔（純関数）: 見ている間は 2 秒、それ以外は 30 秒
pub fn scan_interval(wanted_until: Option<Instant>, now: Instant) -> Duration {
    if wanted_until.is_some_and(|until| until > now) {
        SCAN_INTERVAL_WANTED
    } else {
        SCAN_INTERVAL
    }
}

/// GUI がペインの一覧と自分の IPC の受け口を渡す（UI スレッド。2 秒 tick）。
/// **ここでは採らない**（覚えを差し替えるだけ）
pub fn publish_panes(panes: Vec<PaneEntry>, own_socket: Option<String>) {
    let mut p = lock(published());
    p.panes = panes;
    p.own_socket = own_socket;
}

/// 機全体を採って覚えへ入れる
fn refresh_machine(scan: &mut ScanState, now: Instant) {
    let Some(raw) = b29::machine() else {
        lock(published()).machine_unavailable = Some(UNAVAILABLE);
        return;
    };
    let memory = raw.vm.map(|vm| core_res::memory_breakdown(raw.total, &vm));
    if let Some(ticks) = raw.cpu {
        match scan.cpu_ticks {
            Some((at, prev)) if now.duration_since(at) >= MIN_CPU_WINDOW => {
                scan.last_machine_cpu =
                    core_res::cpu_busy_percent(prev, ticks).or(scan.last_machine_cpu);
                scan.cpu_ticks = Some((now, ticks));
            }
            Some(_) => {}
            None => scan.cpu_ticks = Some((now, ticks)),
        }
    }
    let stats = MachineStats {
        memory,
        swap: raw.swap,
        pressure: raw.pressure_level.and_then(core_res::Pressure::from_level),
        cpu_percent: scan.last_machine_cpu,
        load: raw.load,
        cpus: raw.cpus,
    };
    let mut p = lock(published());
    p.machine = Some((now, stats));
    p.machine_unavailable = None;
}

/// 採れない OS での理由（#2000 = S8 で Windows を実装する）
const UNAVAILABLE: &str = "この OS ではまだ採れない（Windows は #2000 で対応）";

/// プロセスを走査して集計を覚えへ入れる
fn refresh_processes(scan: &mut ScanState, now: Instant, interval: Duration) {
    let t0 = Instant::now();
    let Some(raws) = b29::processes() else {
        return;
    };
    let (panes, own_socket) = {
        let p = lock(published());
        (p.panes.clone(), p.own_socket.clone())
    };
    let own_uid = b29::own_uid();
    if scan.reader.is_none() {
        scan.reader = b29::DetailsReader::new();
    }

    // argv / env は新しいプロセスだけ読む（自分の uid のものだけ。他人のは読めない）
    let live: HashSet<ProcKey> = raws
        .iter()
        .map(|r| ProcKey {
            pid: r.pid,
            start: r.start,
        })
        .collect();
    scan.details.retain(|k, _| live.contains(k));
    for raw in &raws {
        let key = ProcKey {
            pid: raw.pid,
            start: raw.start,
        };
        if scan.details.contains_key(&key) || Some(raw.uid) != own_uid {
            continue;
        }
        let d = scan
            .reader
            .as_mut()
            .and_then(|r| r.read(raw.pid, &ENV_KEYS))
            .unwrap_or_default();
        scan.details.insert(
            key,
            CachedDetails {
                argv: d.argv,
                pane_id: d.env.get("TAKO_PANE_ID").cloned(),
                socket: d.env.get("TAKO_SOCKET").cloned(),
                isolated: d.env.get("TAKO_ISOLATED").is_some_and(|v| v == "1"),
                queue_job: d
                    .env
                    .get(crate::queue::ENV_JOB)
                    .and_then(|v| v.trim().parse().ok()),
                jobserver_fifo: d
                    .env
                    .get(tako_core::jobserver::ENV)
                    .and_then(|v| tako_core::jobserver::tako_fifo(v))
                    .map(str::to_string),
            },
        );
    }

    // CPU% = 前回の走査との差。近すぎる 2 回は前回の値を使う（粒度に負ける）
    let elapsed = scan.proc_cpu_at.map(|at| now.duration_since(at));
    let fresh_window = elapsed.is_some_and(|e| e >= MIN_CPU_WINDOW);
    if fresh_window || scan.proc_cpu_at.is_none() {
        let mut pct = HashMap::new();
        if let Some(e) = elapsed.filter(|_| fresh_window) {
            let elapsed_ns = e.as_nanos().min(u128::from(u64::MAX)) as u64;
            for raw in &raws {
                let key = ProcKey {
                    pid: raw.pid,
                    start: raw.start,
                };
                if let (Some(cur), Some(prev)) = (raw.cpu_ns, scan.proc_cpu.get(&key)) {
                    if let Some(p) = core_res::cpu_percent(*prev, cur, elapsed_ns) {
                        pct.insert(key, p);
                    }
                }
            }
            scan.last_proc_pct = pct;
        }
        scan.proc_cpu = raws
            .iter()
            .filter_map(|r| {
                r.cpu_ns.map(|c| {
                    (
                        ProcKey {
                            pid: r.pid,
                            start: r.start,
                        },
                        c,
                    )
                })
            })
            .collect();
        scan.proc_cpu_at = Some(now);
    }

    let samples: Vec<ProcSample> = raws
        .iter()
        .map(|raw| {
            let key = ProcKey {
                pid: raw.pid,
                start: raw.start,
            };
            let d = scan.details.get(&key);
            ProcSample {
                pid: raw.pid,
                ppid: raw.ppid,
                start: raw.start,
                name: raw.name.clone(),
                footprint: raw.footprint,
                cpu_ns: raw.cpu_ns,
                argv: d.and_then(|d| d.argv.clone()),
                env_pane: d.and_then(|d| {
                    core_res::env_pane_of(
                        d.pane_id.as_deref(),
                        d.socket.as_deref(),
                        own_socket.as_deref(),
                    )
                }),
                isolated: d.is_some_and(|d| d.isolated),
                queue_job: d.and_then(|d| d.queue_job),
                jobserver_fifo: d.and_then(|d| d.jobserver_fifo.clone()),
            }
        })
        .collect();

    let roots = pane_roots(scan, &panes, now);
    let report = core_res::summarize(&samples, &scan.last_proc_pct, &roots);
    let readable = raws.iter().filter(|r| r.footprint.is_some()).count();
    let snapshot = ProcSnapshot {
        at: now,
        report,
        scanned: raws.len(),
        readable,
        took: t0.elapsed(),
        interval,
        samples: Arc::new(samples),
        cpu: Arc::new(scan.last_proc_pct.clone()),
    };
    lock(published()).procs = Some(snapshot);
}

/// ペインの根（pid → ペイン）。器のペインは `tmux list-panes -a` の `pane_pid`
/// （子プロセス。上限つき。採り直すのは [`backend_pids_stale`] のときだけ）
fn pane_roots(scan: &mut ScanState, panes: &[PaneEntry], now: Instant) -> HashMap<u32, u64> {
    let mut roots = HashMap::new();
    for p in panes {
        if p.backend_session.is_none() {
            if let Some(pid) = p.child_pid {
                roots.insert(pid, p.pane);
            }
        }
    }
    if panes.iter().any(|p| p.backend_session.is_some()) {
        let stale = backend_pids_stale(
            scan.backend.as_ref(),
            panes,
            now,
            tako_core::pane_log::issue2018_legacy(),
        );
        if stale {
            scan.backend = Some((now, crate::agents::backend_pane_pids()));
        }
        if let Some((_, all)) = &scan.backend {
            roots.extend(backend_roots(panes, all));
        }
    }
    roots
}

/// 覚えている器の pid（`pane_pid`）を採り直すか（#2018。純関数）。
///
/// `pane_pid` は器のペインのシェルの pid で、セッションが生きている間は変わらない。
/// なので「覚えに無いセッションがある（作った直後・顔ぶれが変わった）」ときと
/// [`BACKEND_INSURANCE`] の保険だけで採り直す。修正前（`legacy`）は 30 秒ごとの走査のたびに
/// [`BACKEND_INTERVAL`] を過ぎていれば採り直していた（= 走査ごとに `tmux list-panes -a` 1 本）
fn backend_pids_stale(
    cached: Option<&(Instant, Vec<(String, u32)>)>,
    panes: &[PaneEntry],
    now: Instant,
    legacy: bool,
) -> bool {
    let Some((at, all)) = cached else {
        return true;
    };
    let age = now.duration_since(*at);
    if legacy {
        return age >= BACKEND_INTERVAL;
    }
    let missing = panes
        .iter()
        .filter_map(|p| p.backend_session.as_deref())
        .any(|session| {
            let prefix = format!("{session}:");
            !all.iter().any(|(id, _)| id.starts_with(&prefix))
        });
    missing || age >= BACKEND_INSURANCE
}

/// 器のセッション名 → そのセッションの全ペインの `pane_pid` をペインへ対応させる（純関数）。
/// `all` の ID は `session:window.pane`
pub fn backend_roots(panes: &[PaneEntry], all: &[(String, u32)]) -> HashMap<u32, u64> {
    let mut out = HashMap::new();
    for p in panes {
        let Some(session) = p.backend_session.as_deref() else {
            continue;
        };
        let prefix = format!("{session}:");
        for (id, pid) in all {
            if id.starts_with(&prefix) {
                out.insert(*pid, p.pane);
            }
        }
    }
    out
}

/// 機全体の覚え（採らない。順番待ちの調停役が読む。#1994）
pub fn machine_stats() -> Option<MachineStats> {
    lock(published()).machine.as_ref().map(|(_, m)| m.clone())
}

/// 機全体の覚えが [`MACHINE_INTERVAL`] より古ければこのスレッドで採り直す（8 µs。走査はしない）。
/// **UI スレッドでは呼ばない**（順番待ちの受け口 = 接続のスレッドが呼ぶ。#1994）
pub fn refresh_machine_if_stale() {
    let now = Instant::now();
    let stale = lock(published())
        .machine
        .as_ref()
        .is_none_or(|(at, _)| now.duration_since(*at) >= MACHINE_INTERVAL);
    if stale {
        refresh_machine(&mut lock(scan_state()), now);
    }
}

/// `until` までは走査を 2 秒ごとに保つ（順番待ちに件がある間。#1994）
pub fn keep_scanning(until: Instant) {
    let mut p = lock(published());
    if p.wanted_until.is_none_or(|w| w < until) {
        p.wanted_until = Some(until);
    }
}

/// 順番待ちの木（受付番号, wrapper の pid）の消費と、通っていない重い処理の件数（#1994）。
/// 走査の覚えが `since` より新しいときだけ返す（採らない。集計は走査 1 回ぶんの O(n)）
pub fn managed(jobs: &[(u64, u32)], since: Option<Instant>) -> Option<(Instant, ManagedView)> {
    let (at, samples, cpu, heavy) = {
        let p = lock(published());
        let snap = p.procs.as_ref()?;
        if since.is_some_and(|s| snap.at <= s) {
            return None;
        }
        (
            snap.at,
            Arc::clone(&snap.samples),
            Arc::clone(&snap.cpu),
            snap.report.heavy.clone(),
        )
    };
    Some((at, core_res::managed_view(&samples, &cpu, &heavy, jobs)))
}

/// ペインごとの木のメモリ（MiB）の覚え（**採らない**。agent の起動待ち = #2413 が、立ち上がり中の agent の
/// 使用量として読む）。走査の覚えが `since` より新しいときだけ返す
pub fn pane_memory(since: Option<Instant>) -> Option<(Instant, HashMap<u64, u64>)> {
    let p = lock(published());
    let snap = p.procs.as_ref()?;
    if since.is_some_and(|s| snap.at <= s) {
        return None;
    }
    Some((
        snap.at,
        snap.report
            .panes
            .iter()
            .map(|u| (u.pane, mb(u.memory)))
            .collect(),
    ))
}

/// `fifo` の札を持ちうるプロセス（同じ置き場の系統の fifo を指す cargo 系）の本数（#1998）。
///
/// 覚えの使い方は `tako_core::jobserver::scan_use`: 居る覚えは信じ、0 本の覚えは新しいときだけ信じる。
/// それ以外（と `fresh` = 終了処理）は**このスレッドで採り直す**（約 20 ms）。**UI スレッドでは呼ばない**
/// （札の番人の背景スレッドと終了処理だけが呼ぶ）。走査できない OS では `None`
pub fn jobserver_holders(fifo: &std::path::Path, fresh: bool) -> Option<usize> {
    use tako_core::jobserver::{holders, scan_use, ScanUse, FRESH_SCAN};
    let fifo = fifo.to_str()?;
    let own_pid = std::process::id();
    // 覚え（数えた本数, 古さ, 走査の時刻）
    let snapshot = |now: Instant| {
        lock(published()).procs.as_ref().map(|s| {
            (
                holders(&s.samples, own_pid, fifo),
                now.duration_since(s.at),
                s.at,
            )
        })
    };
    let now = Instant::now();
    let seen = snapshot(now);
    let usable = if fresh {
        seen.filter(|(_, age, _)| *age <= FRESH_SCAN)
    } else {
        seen.filter(|(n, age, _)| scan_use(Some((*n, *age))) != ScanUse::Refresh)
    };
    if let Some((n, _, _)) = usable {
        return Some(n);
    }
    let mut scan = lock(scan_state());
    // ロックを待つ間に採り直した走査があればそれを使う
    let now = Instant::now();
    if let Some((n, age, _)) = snapshot(now) {
        if age <= FRESH_SCAN {
            return Some(n);
        }
    }
    let interval = scan_interval(lock(published()).wanted_until, now);
    refresh_processes(&mut scan, now, interval);
    // 採れなかった（走査できない OS・一覧を取れない）なら古い覚えを返さない（0 本と読み違えない）
    snapshot(now)
        .filter(|(_, _, at)| *at == now)
        .map(|(n, _, _)| n)
}

/// IPC / MCP の `Request::Queue` に答える（**UI スレッドでは呼ばない**。`ipc::submit` の手前）。
/// 覚えが古ければこのスレッドで採り直す
pub fn answer(action: Option<&str>) -> Result<Value, DispatchError> {
    match action.unwrap_or("status") {
        "status" => {}
        other => {
            return Err(DispatchError::InvalidParams(format!(
                "未知の action: {other}（使えるのは status）"
            )))
        }
    }
    let now = Instant::now();
    lock(published()).wanted_until = Some(now + WANTED_FOR);
    {
        let mut scan = lock(scan_state());
        let (machine_stale, procs_stale) = {
            let p = lock(published());
            (
                p.machine
                    .as_ref()
                    .is_none_or(|(at, _)| now.duration_since(*at) >= MACHINE_INTERVAL),
                p.procs
                    .as_ref()
                    .is_none_or(|s| now.duration_since(s.at) >= SCAN_INTERVAL_WANTED),
            )
        };
        if machine_stale {
            refresh_machine(&mut scan, now);
        }
        if procs_stale {
            refresh_processes(&mut scan, now, SCAN_INTERVAL_WANTED);
        }
    }
    Ok(cached_status())
}

/// 覚えだけから status を組む（UI スレッドから呼んでよい。採らない）
pub fn cached_status() -> Value {
    let p = lock(published());
    status_json(&p, Instant::now())
}

fn mb(bytes: u64) -> u64 {
    (bytes + MIB / 2) / MIB
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn machine_json(stats: &MachineStats) -> Value {
    json!({
        "memory_mb": stats.memory.map(|m| json!({
            "total": mb(m.total),
            "used": mb(m.used),
            "app": mb(m.app),
            "wired": mb(m.wired),
            "compressed": mb(m.compressed),
            "cached": mb(m.cached),
            "available": mb(m.available),
        })),
        "swap_mb": stats.swap.map(|(used, total)| json!({ "used": mb(used), "total": mb(total) })),
        "pressure": stats.pressure.map(|p| p.as_str()),
        "cpu_percent": stats.cpu_percent.map(round1),
        "load": stats.load.map(|l| l.map(|v| (v * 100.0).round() / 100.0)),
        "cpus": stats.cpus,
    })
}

fn owner_json(owner: &Owner, panes: &HashMap<u64, &PaneEntry>) -> Value {
    match owner {
        Owner::Pane(pane) => {
            let entry = panes.get(pane);
            json!({
                "pane": pane,
                "tab": entry.and_then(|e| e.tab_title.clone()),
                "title": entry.and_then(|e| e.title.clone()),
                "role": entry.and_then(|e| e.role.clone()),
            })
        }
        Owner::External { origin } => json!({ "external": origin.clone().unwrap_or_default() }),
    }
}

fn status_json(p: &Published, now: Instant) -> Value {
    let panes_by_id: HashMap<u64, &PaneEntry> = p.panes.iter().map(|e| (e.pane, e)).collect();
    let mut out = json!({ "gui": true });
    match &p.machine {
        Some((at, stats)) => {
            let mut m = machine_json(stats);
            m["age_ms"] = json!(now.duration_since(*at).as_millis() as u64);
            out["machine"] = m;
        }
        None => {
            out["machine"] = Value::Null;
            out["machine_unavailable"] = json!(p.machine_unavailable.unwrap_or(UNAVAILABLE));
        }
    }
    let Some(snap) = &p.procs else {
        out["heavy"] = json!([]);
        out["panes"] = json!([]);
        return out;
    };
    out["heavy"] = snap
        .report
        .heavy
        .iter()
        .map(|job| {
            let mut v = owner_json(&job.owner, &panes_by_id);
            v["pid"] = json!(job.root.pid);
            v["kind"] = json!(job.kind.as_str());
            v["label"] = json!(job.label);
            v["memory_mb"] = json!(mb(job.memory));
            v["cpu_percent"] = json!(job.cpu_percent.map(round1));
            v["processes"] = json!(job.processes);
            v
        })
        .collect();
    out["panes"] = snap
        .report
        .panes
        .iter()
        .take(PANES_SHOWN)
        .map(|u| {
            let mut v = owner_json(&Owner::Pane(u.pane), &panes_by_id);
            if panes_by_id.get(&u.pane).is_some_and(|e| e.shelved) {
                v["shelved"] = json!(true);
            }
            v["agent"] = json!(u.agent);
            v["memory_mb"] = json!(mb(u.memory));
            v["agent_mb"] = json!(mb(u.agent_memory));
            v["heavy_mb"] = json!(mb(u.heavy_memory));
            v["heavy"] = json!(u.heavy);
            v["cpu_percent"] = json!(round1(u.cpu_percent));
            v["processes"] = json!(u.processes);
            v
        })
        .collect();
    out["panes_total"] = json!(snap.report.panes.len());
    out["external"] = json!({
        "memory_mb": mb(snap.report.external.memory),
        "cpu_percent": round1(snap.report.external.cpu_percent),
        "processes": snap.report.external.processes,
        "heavy": snap.report.external.heavy,
    });
    out["scan"] = json!({
        "processes": snap.scanned,
        "readable": snap.readable,
        "age_ms": now.duration_since(snap.at).as_millis() as u64,
        "interval_ms": snap.interval.as_millis() as u64,
        "took_ms": round1(snap.took.as_secs_f64() * 1000.0),
    });
    out
}

/// GUI が居ないときの CLI 用: 機の状態だけを採って返す（このプロセスで採る）。
/// 機全体の CPU% は差分が要るので、`cpu_window` だけ間を空けて 2 回採る
pub fn machine_only(cpu_window: Duration) -> Value {
    let first = b29::machine();
    let Some(first) = first else {
        return json!({ "machine": Value::Null, "machine_unavailable": UNAVAILABLE });
    };
    std::thread::sleep(cpu_window);
    let second = b29::machine().unwrap_or_else(|| first.clone());
    let cpu_percent = match (first.cpu, second.cpu) {
        (Some(a), Some(b)) => core_res::cpu_busy_percent(a, b),
        _ => None,
    };
    let stats = MachineStats {
        memory: second
            .vm
            .map(|vm| core_res::memory_breakdown(second.total, &vm)),
        swap: second.swap,
        pressure: second
            .pressure_level
            .and_then(core_res::Pressure::from_level),
        cpu_percent,
        load: second.load,
        cpus: second.cpus,
    };
    json!({ "machine": machine_json(&stats) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 走査の間隔は見ている間だけ短い() {
        let now = Instant::now();
        assert_eq!(scan_interval(None, now), SCAN_INTERVAL);
        assert_eq!(
            scan_interval(Some(now + Duration::from_secs(3)), now),
            SCAN_INTERVAL_WANTED
        );
        assert_eq!(
            scan_interval(Some(now - Duration::from_secs(1)), now),
            SCAN_INTERVAL,
            "期限が過ぎたら既定へ戻る"
        );
    }

    #[test]
    fn 器のセッションの全ペインを根にする() {
        let panes = vec![
            PaneEntry {
                pane: 7,
                backend_session: Some("tako-7".into()),
                ..PaneEntry::default()
            },
            PaneEntry {
                pane: 8,
                child_pid: Some(500),
                ..PaneEntry::default()
            },
        ];
        let all = vec![
            ("tako-7:0.0".to_string(), 100),
            ("tako-7:1.0".to_string(), 101),
            ("tako-70:0.0".to_string(), 102),
        ];
        let roots = backend_roots(&panes, &all);
        assert_eq!(roots.get(&100), Some(&7));
        assert_eq!(roots.get(&101), Some(&7), "window が増えても同じペイン");
        assert_eq!(roots.get(&102), None, "前方一致で別セッションを拾わない");
        assert_eq!(roots.get(&500), None, "器の無いペインはここでは扱わない");
    }

    /// #2018: 器の pid は顔ぶれが変わったときと 60 秒の保険でだけ採り直す（A/B は 10 秒ごと）
    #[test]
    fn 器のpidは顔ぶれが変わったときと保険でだけ採り直す() {
        let now = Instant::now();
        let pane = |session: &str| PaneEntry {
            backend_session: Some(session.into()),
            ..PaneEntry::default()
        };
        let cached = (
            now,
            vec![
                ("tako-7:0.0".to_string(), 100),
                ("tako-8:0.0".to_string(), 200),
            ],
        );
        let panes = vec![pane("tako-7"), pane("tako-8")];
        let at = |secs: u64| now + Duration::from_secs(secs);
        assert!(backend_pids_stale(None, &panes, now, false), "初回");
        assert!(!backend_pids_stale(Some(&cached), &panes, at(30), false));
        assert!(!backend_pids_stale(Some(&cached), &panes, at(59), false));
        assert!(
            backend_pids_stale(Some(&cached), &panes, at(60), false),
            "保険"
        );
        // 覚えに無いセッション（作った直後）。前方一致で別のセッションを数えない
        let more = vec![pane("tako-7"), pane("tako-9")];
        assert!(backend_pids_stale(Some(&cached), &more, at(1), false));
        let prefix = vec![pane("tako-")];
        assert!(backend_pids_stale(Some(&cached), &prefix, at(1), false));
        // 減っただけなら覚えで足りる
        assert!(!backend_pids_stale(
            Some(&cached),
            &[pane("tako-7")],
            at(1),
            false
        ));
        // A/B: 修正前は 10 秒ごと
        assert!(!backend_pids_stale(Some(&cached), &panes, at(9), true));
        assert!(backend_pids_stale(Some(&cached), &panes, at(10), true));
    }

    #[test]
    fn 未知のactionは断る() {
        let err = answer(Some("cancel")).unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)), "{err:?}");
    }

    #[test]
    fn statusは機と走査の節を返す() {
        let v = answer(None).expect("status");
        assert_eq!(v["gui"], true);
        if cfg!(target_os = "macos") {
            let m = &v["machine"];
            assert!(m["memory_mb"]["total"].as_u64().unwrap() > 0, "{v}");
            assert!(["normal", "warn", "critical"].contains(&m["pressure"].as_str().unwrap()));
            assert!(v["scan"]["processes"].as_u64().unwrap() > 0);
            assert!(v["heavy"].is_array());
            assert!(v["panes"].is_array());
        } else {
            assert!(v["machine"].is_null());
            assert!(v["machine_unavailable"].as_str().is_some());
        }
    }

    #[test]
    fn 機だけの状態はguiが居なくても採れる() {
        let v = machine_only(Duration::from_millis(50));
        if cfg!(target_os = "macos") {
            assert!(
                v["machine"]["memory_mb"]["available"].as_u64().is_some(),
                "{v}"
            );
        } else {
            assert!(v["machine"].is_null());
        }
    }
}
