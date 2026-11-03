//! 右パネルの `resources` ビューとステータスバーの区画 —— PC の負荷（Issue #1995。エピック #1992 の S3）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md` の §7。
//!
//! ## 正本はここに無い
//!
//! 数値の正本は `tako queue` / MCP `tako_queue` の status（`tako_control::queue::answer` = 機・走査の覚え +
//! 順番待ちの調停役）。この画面は**読んだ答えを描くだけ**で、走査・子プロセス・ファイルの読み書きを
//! UI スレッドでしない（番犬 `issue1993_resources_ui_thread_watchdog` の 8 番）:
//!
//! - **ビューを見ている間**（2 秒ごと）: CLI / MCP と同じ `Request::Queue { action: "status" }` を
//!   `dispatch_off_ui` で投げる（`prepare_offload` → background の `OffloadJob::Queue` → `queue::answer`。
//!   `ipc::submit` が手前で答えるのと同じ関数）。見ている間は走査も 2 秒ごとになる（§2.2）
//! - **見ていない間**（ステータスバーの区画）: background で覚えだけを読む
//!   （[`read_cached_snapshot`] = dispatch の同期の腕と同じ `with_section(cached_status())`。走査は起こさない）
//!
//! どちらも JSON を [`ResourcesSnapshot::from_status`]（純関数）で描く形へ写し、UI はそれを読むだけ。
//!
//! ## 操作は CLI / MCP と同じ dispatch
//!
//! 行を押す = `Request::Focus`（`tako focus`）、取り消し = `Request::Queue { action: "cancel" }`
//! （`tako queue cancel`。確かめてから）、優先度 = `Request::Queue { action: "priority", id | pane }`
//! （`tako queue priority`）。どれも `dispatch_off_ui` を通るので、受付番号の操作は IPC と同じ
//! `queue::answer` が background で、ペインの既定（`queue::needs_ui`）は UI スレッドの dispatch が受ける。
//!
//! 印（メモリのバー・段のチップ・確かめの枠）は GPUI の矩形と既存の SVG で描く。絵文字は使わない。

use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, relative, svg, Context, SharedString};
use serde_json::{json, Value};
use tako_core::queue::Priority;
use tako_core::resources::Pressure;
use tako_core::PaneOrigin;

use super::*;
use crate::ui_text::resources as text;

/// 見ている間の読み直しの間隔（走査の「見ている間」の間隔 `resources::SCAN_INTERVAL_WANTED` と同じ）
const RESOURCES_REFRESH: Duration = Duration::from_secs(2);
/// 操作の結果の 1 行を出しておく長さ
const NOTICE_FOR: Duration = Duration::from_secs(10);
/// 優先度のチップに並べる段（CLI `tako queue priority` の値と同じ並び）
const PRIORITY_CHOICES: [&str; 5] = ["top", "high", "normal", "low", "default"];

// ===== 描く形（純関数で JSON から写す）=====

/// ① 機
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MachineView {
    /// (使用, 全体, 圧縮, 空き) MiB。機の値はあってもメモリの内訳が採れなければ `None`
    pub(crate) memory: Option<(u64, u64, u64, u64)>,
    pub(crate) swap: Option<(u64, u64)>,
    pub(crate) pressure: Option<Pressure>,
    pub(crate) cpu_percent: Option<f64>,
    pub(crate) cpus: u32,
    /// load（macOS のみ。Windows には無い = 出さない）
    pub(crate) load: Option<[f64; 3]>,
}

/// 終わりの見込み（status の `eta`）
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EtaView {
    pub(crate) state: String,
    pub(crate) remaining_s: Option<u64>,
    pub(crate) previous_s: Option<u64>,
    /// `N/M` だけからの見込みか
    pub(crate) from_progress: bool,
}

/// ② 実行中 / ③ 待ちの 1 件
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct JobView {
    /// 受付番号（= wrapper の pid）
    pub(crate) id: u64,
    pub(crate) label: String,
    pub(crate) pane: Option<u64>,
    pub(crate) tab: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) priority: Option<Priority>,
    pub(crate) base_priority: Option<Priority>,
    pub(crate) priority_from: Option<String>,
    pub(crate) aged: bool,
    /// 処理に付けた段（受付番号での変更・`--priority`）
    pub(crate) job_priority: Option<Priority>,
    /// ペインの優先度の既定（無ければ役割の既定）
    pub(crate) pane_priority: Option<Priority>,
    pub(crate) cpu_percent: Option<f64>,
    pub(crate) memory_mb: Option<u64>,
    pub(crate) ran_s: Option<u64>,
    pub(crate) progress: Option<(u32, u32)>,
    pub(crate) eta: Option<EtaView>,
    pub(crate) stalled: bool,
    pub(crate) cancel: bool,
    pub(crate) position: Option<u64>,
    pub(crate) waited_s: Option<u64>,
    pub(crate) start_s: Option<u64>,
    pub(crate) reason: Option<String>,
    pub(crate) reason_text: Option<String>,
}

/// 順番待ちの節
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct QueueView {
    /// この tako が調停役か（隔離 / セルフテスト / セカンダリは違う）
    pub(crate) arbiter: bool,
    pub(crate) max_jobs: Option<u64>,
    pub(crate) running: Vec<JobView>,
    pub(crate) waiting: Vec<JobView>,
    /// 順番待ちを通っていない重い処理（tako の外を含む）
    pub(crate) unmanaged: u64,
}

/// ④ 常駐の内訳の 1 ペイン
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PaneView {
    pub(crate) pane: u64,
    pub(crate) tab: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) shelved: bool,
    pub(crate) agent: Option<String>,
    pub(crate) memory_mb: u64,
    /// agent 本体 + MCP サーバー等の補助
    pub(crate) agent_mb: u64,
    pub(crate) heavy_mb: u64,
    pub(crate) cpu_percent: f64,
}

impl PaneView {
    /// agent と重い処理を除いた残り（dev サーバー・シェル等）
    pub(crate) fn other_mb(&self) -> u64 {
        self.memory_mb
            .saturating_sub(self.agent_mb)
            .saturating_sub(self.heavy_mb)
    }
}

/// 描く中身の全部（1 回の status の答え）
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ResourcesSnapshot {
    /// `None` = この OS では採れない（未対応）
    pub(crate) machine: Option<MachineView>,
    pub(crate) queue: QueueView,
    pub(crate) panes: Vec<PaneView>,
    pub(crate) panes_total: usize,
    /// (メモリ MiB, 本数, 重い処理の件数)
    pub(crate) external: Option<(u64, u64, u64)>,
    /// (走査した本数, 古さ ms)。`None` = 走査できない OS か、まだ走査していない
    pub(crate) scan: Option<(u64, u64)>,
}

/// ステータスバーの区画に描く分（変わったときだけ描き直すための比較の単位）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BarView {
    /// (使用, 全体) MiB。`None` = 未対応
    pub(crate) memory: Option<(u64, u64)>,
    pub(crate) pressure: Option<Pressure>,
    pub(crate) running: usize,
    pub(crate) waiting: usize,
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn priority_of(v: &Value) -> Option<Priority> {
    v.as_str().and_then(Priority::parse)
}

fn pressure_of(v: &Value) -> Option<Pressure> {
    match v.as_str()? {
        "normal" => Some(Pressure::Normal),
        "warn" => Some(Pressure::Warn),
        "critical" => Some(Pressure::Critical),
        _ => None,
    }
}

fn progress_of(v: &Value) -> Option<(u32, u32)> {
    let total = v["total"].as_u64()?;
    (total > 0).then(|| (v["done"].as_u64().unwrap_or(0) as u32, total as u32))
}

impl JobView {
    fn from_row(v: &Value) -> Self {
        let eta = v["eta"].as_object().map(|_| EtaView {
            state: v["eta"]["state"].as_str().unwrap_or("unknown").to_string(),
            remaining_s: v["eta"]["remaining_s"].as_u64(),
            previous_s: v["eta"]["previous_s"].as_u64(),
            from_progress: v["eta"]["from"] == "progress",
        });
        Self {
            id: v["id"].as_u64().unwrap_or(0),
            label: v["label"].as_str().unwrap_or("?").to_string(),
            pane: v["pane"].as_u64(),
            tab: str_of(&v["tab"]),
            title: str_of(&v["title"]),
            role: str_of(&v["role"]),
            priority: priority_of(&v["priority"]),
            base_priority: priority_of(&v["base_priority"]),
            priority_from: str_of(&v["priority_from"]),
            aged: v["aged"] == true,
            job_priority: priority_of(&v["job_priority"]),
            pane_priority: priority_of(&v["pane_priority"]),
            cpu_percent: v["cpu_percent"].as_f64(),
            memory_mb: v["memory_mb"].as_u64(),
            ran_s: v["ran_s"].as_u64(),
            progress: progress_of(&v["progress"]),
            eta,
            stalled: v["stalled"] == true,
            cancel: v["cancel"] == true,
            position: v["position"].as_u64(),
            waited_s: v["waited_s"].as_u64(),
            start_s: v["start_s"].as_u64(),
            reason: str_of(&v["reason"]),
            reason_text: str_of(&v["text"]),
        }
    }
}

impl ResourcesSnapshot {
    /// status の JSON（`tako queue --json` と同じ形）から写す（純関数。UI スレッドで呼んでよい = 読むだけ）
    pub(crate) fn from_status(v: &Value) -> Self {
        let machine = v["machine"].as_object().map(|_| {
            let m = &v["machine"];
            let mem = &m["memory_mb"];
            let n = |key: &str| mem[key].as_u64();
            let load = m["load"].as_array().and_then(|a| {
                let l: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                (l.len() == 3).then(|| [l[0], l[1], l[2]])
            });
            MachineView {
                memory: match (n("used"), n("total")) {
                    (Some(used), Some(total)) if total > 0 => Some((
                        used,
                        total,
                        n("compressed").unwrap_or(0),
                        n("available").unwrap_or(0),
                    )),
                    _ => None,
                },
                swap: m["swap_mb"]["total"]
                    .as_u64()
                    .map(|t| (m["swap_mb"]["used"].as_u64().unwrap_or(0), t)),
                pressure: pressure_of(&m["pressure"]),
                cpu_percent: m["cpu_percent"].as_f64(),
                cpus: m["cpus"].as_u64().unwrap_or(0) as u32,
                load,
            }
        });
        let q = &v["queue"];
        let rows = |key: &str| -> Vec<JobView> {
            q[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(JobView::from_row)
                .collect()
        };
        let queue = QueueView {
            arbiter: q["arbiter"] == true,
            max_jobs: q["limits"]["max_jobs"].as_u64(),
            running: rows("running"),
            waiting: rows("waiting"),
            unmanaged: q["unmanaged"].as_u64().unwrap_or(0),
        };
        let panes: Vec<PaneView> = v["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some(PaneView {
                    pane: p["pane"].as_u64()?,
                    tab: str_of(&p["tab"]),
                    title: str_of(&p["title"]),
                    role: str_of(&p["role"]),
                    shelved: p["shelved"] == true,
                    agent: str_of(&p["agent"]),
                    memory_mb: p["memory_mb"].as_u64().unwrap_or(0),
                    agent_mb: p["agent_mb"].as_u64().unwrap_or(0),
                    heavy_mb: p["heavy_mb"].as_u64().unwrap_or(0),
                    cpu_percent: p["cpu_percent"].as_f64().unwrap_or(0.0),
                })
            })
            .collect();
        let external = v["external"].as_object().map(|_| {
            let e = &v["external"];
            (
                e["memory_mb"].as_u64().unwrap_or(0),
                e["processes"].as_u64().unwrap_or(0),
                e["heavy"].as_u64().unwrap_or(0),
            )
        });
        let scan = v["scan"].as_object().map(|_| {
            (
                v["scan"]["processes"].as_u64().unwrap_or(0),
                v["scan"]["age_ms"].as_u64().unwrap_or(0),
            )
        });
        Self {
            machine,
            panes_total: v["panes_total"]
                .as_u64()
                .map_or(panes.len(), |n| n as usize),
            panes,
            queue,
            external,
            scan,
        }
    }

    pub(crate) fn bar(&self) -> BarView {
        BarView {
            memory: self
                .machine
                .and_then(|m| m.memory)
                .map(|(used, total, _, _)| (used, total)),
            pressure: self.machine.and_then(|m| m.pressure),
            running: self.queue.running.len(),
            waiting: self.queue.waiting.len(),
        }
    }

    fn job(&self, id: u64) -> Option<&JobView> {
        self.queue
            .running
            .iter()
            .chain(self.queue.waiting.iter())
            .find(|j| j.id == id)
    }
}

/// 覚えだけから組む（**background で呼ぶ**。走査は起こさない = dispatch の同期の腕と同じ
/// `with_section(cached_status())`。調停役の錠・設定の mtime・札の数えがあるので UI スレッドでは呼ばない）
pub(crate) fn read_cached_snapshot() -> ResourcesSnapshot {
    ResourcesSnapshot::from_status(&tako_control::queue::with_section(
        tako_control::resources::cached_status(),
    ))
}

/// メモリ圧の色（normal = 通常 / warn = 黄 / critical = 赤。§7）
pub(crate) fn pressure_color(theme: &tako_core::Theme, p: Option<Pressure>) -> tako_core::Rgb {
    match p {
        Some(Pressure::Critical) => theme.red,
        Some(Pressure::Warn) => theme.yellow,
        _ => theme.accent,
    }
}

// ===== 画面の状態 =====

/// 操作の結果の 1 行
#[derive(Debug, Clone)]
pub(crate) struct ResourcesNotice {
    pub(crate) text: String,
    pub(crate) error: bool,
    pub(crate) at: Instant,
}

/// resources ビューの状態（画面の一時状態。永続しない）
#[derive(Default)]
pub(crate) struct ResourcesPanel {
    /// 届いた答え（`None` = まだ読んでいない）
    pub(crate) snapshot: Option<ResourcesSnapshot>,
    /// 直近の読みの失敗（次に届いた答えで消える）
    pub(crate) error: Option<String>,
    /// このセッションで開いたか（タブを出す条件）
    pub(crate) opened: bool,
    /// 次の機会に読み直す（`tako panel --view resources`・操作の後）
    pub(crate) pending: bool,
    /// ビューの読み（`dispatch_off_ui`）の飛んでいる数
    pub(crate) inflight: usize,
    /// ステータスバーの覚えの読み（background）が飛んでいるか
    pub(crate) bar_inflight: bool,
    pub(crate) requested: u64,
    pub(crate) applied: u64,
    pub(crate) last_fetch: Option<Instant>,
    /// 取り消しを確かめている行（受付番号）
    pub(crate) confirm_cancel: Option<u64>,
    /// 優先度の段を開いている行（受付番号）
    pub(crate) priority_open: Option<u64>,
    pub(crate) notice: Option<ResourcesNotice>,
    /// visual-test が置いた答えを定期の読みで上書きしない（境界の場面を撮るため）
    #[cfg(feature = "visual-test")]
    pub(crate) pinned: bool,
}

impl ResourcesPanel {
    /// 答えを置く。後から頼んだ読みが先に着いていたら古い答えで上書きしない。描き直しが要るかを返す
    fn apply(&mut self, seq: u64, result: Result<ResourcesSnapshot, String>) -> bool {
        #[cfg(feature = "visual-test")]
        if self.pinned {
            return false;
        }
        if seq <= self.applied {
            return false;
        }
        self.applied = seq;
        match result {
            Ok(snapshot) => {
                let changed = self.snapshot.as_ref() != Some(&snapshot) || self.error.is_some();
                // 消えた行の確かめ・段の選択は畳む（受付番号は wrapper の pid = 使い回されうる）
                if let Some(id) = self.confirm_cancel {
                    if snapshot.job(id).is_none() {
                        self.confirm_cancel = None;
                    }
                }
                if let Some(id) = self.priority_open {
                    if snapshot.job(id).is_none() {
                        self.priority_open = None;
                    }
                }
                self.snapshot = Some(snapshot);
                self.error = None;
                changed
            }
            Err(e) => {
                self.error = Some(e);
                true
            }
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.requested += 1;
        self.requested
    }

    fn notify(&mut self, text: String, error: bool) {
        self.notice = Some(ResourcesNotice {
            text,
            error,
            at: Instant::now(),
        });
    }

    /// `tako panel` の応答の `resources`（描いている行・確かめ中の取り消し・開いている段）
    pub(crate) fn state_json(&self) -> Value {
        let snap = self.snapshot.as_ref();
        let ids = |rows: Option<&Vec<JobView>>| -> Vec<u64> {
            rows.into_iter().flatten().map(|j| j.id).collect()
        };
        json!({
            "running": ids(snap.map(|s| &s.queue.running)),
            "waiting": ids(snap.map(|s| &s.queue.waiting)),
            "arbiter": snap.map(|s| s.queue.arbiter),
            "machine": snap.map(|s| if s.machine.is_some() { "ok" } else { "unsupported" }),
            "confirm_cancel": self.confirm_cancel,
            "priority_open": self.priority_open,
            "loading": self.inflight > 0,
            "error": self.error,
        })
    }
}

impl TakoApp {
    /// resources ビューを描いているか
    pub(crate) fn resources_view_shown(&self) -> bool {
        self.panel_visible && self.panel_view == PanelView::Resources
    }

    /// resources タブを出すか（開いている間か、このセッションで一度でも開いたとき）
    pub(crate) fn resources_tab_visible(&self) -> bool {
        self.panel_view == PanelView::Resources || self.resources.opened
    }

    /// resources ビューを開く（ステータスバーの区画・パレット・タブ）
    pub(crate) fn open_resources_view(&mut self, cx: &mut Context<Self>) {
        self.resources.opened = true;
        self.resources.pending = true;
        self.panel_visible = true;
        self.panel_view = PanelView::Resources;
        self.drain_resources_pending(cx);
        cx.notify();
    }

    /// 頼まれた読み直しを出す（ビューを描いているときだけ。閉じていれば開いたときに出す）
    pub(crate) fn drain_resources_pending(&mut self, cx: &mut Context<Self>) {
        if self.resources.pending && self.resources_view_shown() {
            self.spawn_resources_fetch(cx);
        }
    }

    /// 2 秒の定期処理から呼ぶ（**新しいタイマーを作らない**）。見ている間は CLI と同じ status を
    /// `dispatch_off_ui` で、見ていない間はステータスバーのために覚えだけを background で読む
    pub(crate) fn tick_resources(&mut self, cx: &mut Context<Self>) {
        if self.resources_view_shown() {
            let due = self
                .resources
                .last_fetch
                .is_none_or(|t| t.elapsed() >= RESOURCES_REFRESH);
            if self.resources.inflight == 0 && (self.resources.pending || due) {
                self.spawn_resources_fetch(cx);
            }
            return;
        }
        if self.resources.bar_inflight {
            return;
        }
        self.resources.bar_inflight = true;
        let seq = self.resources.next_seq();
        let task = cx.background_executor().spawn(async move {
            let _s = tako_control::diag::perf_span("resources_bar_read");
            read_cached_snapshot()
        });
        cx.spawn(async move |this, cx| {
            let snapshot = task.await;
            let _ = this.update(cx, |app: &mut TakoApp, cx| {
                app.resources.bar_inflight = false;
                let before = app.resources.snapshot.as_ref().map(ResourcesSnapshot::bar);
                let bar = snapshot.bar();
                app.resources.apply(seq, Ok(snapshot));
                // 見ていない間に描くのはステータスバーの区画だけ = 区画の値が変わったときだけ描き直す
                if before != Some(bar) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 状況を読む。CLI `tako queue` / MCP `tako_queue` と同じ要求を `dispatch_off_ui` で投げる
    /// （`queue::answer` は background。UI スレッドは要求を組んで答えを置くだけ）
    pub(crate) fn spawn_resources_fetch(&mut self, cx: &mut Context<Self>) {
        let request = queue_request("status", None, None, None);
        self.resources.pending = false;
        self.resources.inflight += 1;
        self.resources.last_fetch = Some(Instant::now());
        let seq = self.resources.next_seq();
        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {
            let panel = &mut app.resources;
            panel.inflight = panel.inflight.saturating_sub(1);
            let result = result
                .map(|v| ResourcesSnapshot::from_status(&v))
                .map_err(|e| e.to_string());
            if panel.apply(seq, result) || app.resources_view_shown() {
                cx.notify();
            }
        });
        cx.notify();
    }

    /// 行を押す = そのペインへフォーカス（`tako focus` と同じ dispatch の `Focus`。別タブならタブも）
    fn resources_focus_pane(&mut self, pane: u64, cx: &mut Context<Self>) {
        self.hover_preview = None;
        self.clear_text_input_focus();
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Focus {
                pane: Some(pane),
                direction: None,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            // この tako に無いペイン（別の tako の番号・閉じた後）
            self.resources.notify(text::op_failed(&e.to_string()), true);
        }
        cx.notify();
    }

    /// 取り消し（確かめた後。`tako queue cancel <受付番号>` と同じ要求）
    fn resources_cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        self.resources.confirm_cancel = None;
        let label = self
            .resources
            .snapshot
            .as_ref()
            .and_then(|s| s.job(id))
            .map(|j| j.label.clone())
            .unwrap_or_default();
        let request = queue_request("cancel", Some(id), None, None);
        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {
            match result {
                Ok(_) => app.resources.notify(text::cancelled(&label), false),
                Err(e) => app.resources.notify(text::op_failed(&e.to_string()), true),
            }
            app.resources.pending = true;
            app.drain_resources_pending(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// 優先度を変える（`tako queue priority <受付番号 | --pane N> <段>` と同じ要求）。
    /// `pane` を渡せばそのペインから出る処理の既定（ペインの模型 = layout.json に残る）
    fn resources_set_priority(
        &mut self,
        id: Option<u64>,
        pane: Option<u64>,
        level: &'static str,
        cx: &mut Context<Self>,
    ) {
        let request = queue_request("priority", id, pane, Some(level));
        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {
            match result {
                Ok(v) => {
                    let notice = match pane {
                        Some(p) => text::pane_priority_changed(p, level),
                        None if v["note"].is_string() => {
                            format!(
                                "{} — {}",
                                text::priority_changed(level, None),
                                text::running_note()
                            )
                        }
                        None => text::priority_changed(level, v["position"].as_u64()),
                    };
                    app.resources.notify(notice, false);
                    if pane.is_some() {
                        // ペインの既定は layout.json に残す（CLI 経由なら `after_dispatch` が書く）
                        app.save_layout();
                    }
                }
                Err(e) => app.resources.notify(text::op_failed(&e.to_string()), true),
            }
            app.resources.pending = true;
            app.drain_resources_pending(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// ステータスバーの区画（メモリの使用バー + 実行中・待ちの件数）。まだ 1 度も読んでいなければ出さない
    pub(crate) fn render_resources_segment(
        &self,
        theme: &tako_core::Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let bar = self.resources.snapshot.as_ref()?.bar();
        let active = self.resources_view_shown();
        let color = pressure_color(theme, bar.pressure);
        let tip = text::bar_tip(
            bar.memory.map(|(u, _)| u),
            bar.memory.map(|(_, t)| t),
            bar.pressure,
        );
        let tip_theme = theme.clone();
        let probes = self.panel_click_probe_bounds.clone();
        let mut seg = div()
            .id("statusbar-resources")
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h_full()
            .px(px(11.0))
            .border_l_1()
            .border_color(hsla(theme.border_subtle))
            .cursor_pointer()
            .when(active, |d| d.bg(rgba_alpha(theme.accent, 0.1)))
            .hover(|d| d.bg(rgba(theme.surface_hover)))
            .tooltip(move |_, cx| {
                cx.new(|_| crate::tab_bar::HintTooltip::new(tip.clone(), tip_theme.clone()))
                    .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if this.resources_view_shown() {
                        this.panel_visible = false;
                        cx.notify();
                    } else {
                        this.open_resources_view(cx);
                    }
                }),
            )
            .child(
                div()
                    .text_color(hsla(theme.text_muted))
                    .child(text::memory_label()),
            );
        seg = match bar.memory {
            Some((used, total)) => {
                let frac = (used as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                let fill_probes = probes.clone();
                seg.child(
                    div()
                        .w(px(48.0))
                        .h(px(6.0))
                        .rounded(px(3.0))
                        .bg(rgba(theme.surface_highlight))
                        .overflow_hidden()
                        .child(
                            div()
                                .relative()
                                .h_full()
                                .rounded(px(3.0))
                                .w(relative(frac))
                                .bg(hsla(color))
                                // 塗りの実矩形（visual-test がメモリ圧の色を実フレームの画素で読む）
                                .child(crate::tab_shape::probe_canvas(
                                    fill_probes,
                                    "statusbar-resources-fill".to_string(),
                                )),
                        ),
                )
                .child(
                    div()
                        .font_family(theme.font_family.clone())
                        .text_color(hsla(match bar.pressure {
                            Some(Pressure::Warn | Pressure::Critical) => color,
                            _ => theme.text_tertiary,
                        }))
                        .child(SharedString::from(format!(
                            "{:.0}%",
                            used as f64 * 100.0 / total.max(1) as f64
                        ))),
                )
            }
            None => seg.child(
                div()
                    .text_color(hsla(theme.text_faint))
                    .child(text::unsupported()),
            ),
        };
        if bar.running + bar.waiting > 0 {
            seg = seg.child(
                div()
                    .text_color(hsla(theme.text_muted))
                    .child(SharedString::from(text::bar_counts(bar.running, bar.waiting))),
            );
        }
        Some(seg.child(crate::tab_shape::probe_canvas(
            probes,
            "statusbar-resources".to_string(),
        )))
    }

    pub(crate) fn render_resources_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let loading = self.resources.inflight > 0;
        let probes = self.panel_click_probe_bounds.clone();
        let probe = move |key: String| crate::tab_shape::probe_canvas(probes.clone(), key);

        // --- 見出し（題・更新中・読み直し）---
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .px(px(10.0))
            .pt(px(8.0))
            .pb(px(6.0))
            .text_size(px(12.0))
            .border_b_1()
            .border_color(hsla(theme.border_inner))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(text::title()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .when(loading && self.resources.snapshot.is_some(), |d| {
                        d.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_muted))
                                .child(text::updating()),
                        )
                    })
                    .child(
                        div()
                            .id("resources-refresh")
                            .relative()
                            .p(px(3.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.surface_hover)))
                            .tooltip({
                                let tip_theme = theme.clone();
                                move |_, cx| {
                                    cx.new(|_| {
                                        crate::tab_bar::HintTooltip::new(
                                            text::refresh().to_string(),
                                            tip_theme.clone(),
                                        )
                                    })
                                    .into()
                                }
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.resources.pending = true;
                                this.drain_resources_pending(cx);
                            }))
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::REFRESH)
                                    .size(px(12.0))
                                    .text_color(hsla(theme.text_muted)),
                            )
                            .child(probe("resources-refresh".to_string())),
                    ),
            );
        let root = div()
            .id("panel-resources-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground))
            .child(header);
        let note = |t: String, color: tako_core::Rgb| {
            div()
                .px(px(10.0))
                .py(px(6.0))
                .text_size(px(11.0))
                .text_color(hsla(color))
                .child(SharedString::from(t))
        };
        let Some(snap) = self.resources.snapshot.clone() else {
            let t = match &self.resources.error {
                Some(e) => text::failed(e),
                None => text::loading().to_string(),
            };
            return root.child(note(t, theme.text_muted));
        };

        let mut body = div()
            .id("panel-resources-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .pb(px(12.0))
            .text_size(px(11.5));
        if let Some(e) = &self.resources.error {
            body = body.child(note(text::failed(e), theme.red));
        }
        if let Some(n) = self
            .resources
            .notice
            .as_ref()
            .filter(|n| n.at.elapsed() < NOTICE_FOR)
        {
            body = body.child(note(
                n.text.clone(),
                if n.error { theme.red } else { theme.teal },
            ));
        }
        let section = |title: String| {
            div()
                .px(px(10.0))
                .pt(px(10.0))
                .pb(px(4.0))
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(title))
        };
        let muted = |t: String| {
            div()
                .px(px(10.0))
                .py(px(2.0))
                .text_size(px(10.5))
                .text_color(hsla(theme.text_muted))
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(SharedString::from(t))
        };

        // --- ① 機 ---
        body = body.child(section(text::section_machine().to_string()));
        match snap.machine {
            None => body = body.child(muted(text::machine_unsupported().to_string())),
            Some(m) => {
                let color = pressure_color(&theme, m.pressure);
                body = match m.memory {
                    Some((used, total, compressed, available)) => {
                        let frac = (used as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                        body.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .px(px(10.0))
                                .py(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(6.0))
                                        .child(text::memory_label())
                                        .child(
                                            div()
                                                .font_family(theme.font_family.clone())
                                                .child(SharedString::from(text::memory_used(
                                                    used, total,
                                                ))),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(5.0))
                                        .text_size(px(10.5))
                                        .text_color(hsla(color))
                                        // メモリ圧の印（丸。GPUI の矩形で描く）
                                        .child(
                                            div()
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .rounded_full()
                                                .bg(hsla(color)),
                                        )
                                        .child(SharedString::from(text::pressure(m.pressure))),
                                ),
                        )
                        .child(
                            div().px(px(10.0)).py(px(3.0)).child(
                                div()
                                    .h(px(6.0))
                                    .w_full()
                                    .rounded(px(3.0))
                                    .bg(rgba(theme.surface_highlight))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(3.0))
                                            .w(relative(frac))
                                            .bg(hsla(color)),
                                    ),
                            ),
                        )
                        .child(muted(text::memory_detail(compressed, available, m.swap)))
                    }
                    None => body.child(muted(format!(
                        "{} {}",
                        text::memory_label(),
                        text::unsupported()
                    ))),
                };
                body = body.child(muted(text::cpu_line(m.cpu_percent, m.cpus, m.load)));
            }
        }

        // --- ② 実行中 / ③ 待ち ---
        let q = &snap.queue;
        if !q.arbiter {
            body = body.child(section(text::section_running(0, None)));
            body = body.child(muted(text::no_arbiter().to_string()));
        } else {
            body = body.child(section(text::section_running(q.running.len(), q.max_jobs)));
            if q.running.is_empty() {
                body = body.child(muted(text::running_empty().to_string()));
            }
            for job in &q.running {
                body = body.child(self.render_resources_job(job, true, &theme, cx));
            }
            if q.unmanaged > 0 {
                body = body.child(muted(text::unmanaged(q.unmanaged)));
            }
            body = body.child(section(text::section_waiting(q.waiting.len())));
            if q.waiting.is_empty() {
                body = body.child(muted(text::waiting_empty().to_string()));
            }
            for job in &q.waiting {
                body = body.child(self.render_resources_job(job, false, &theme, cx));
            }
        }

        // --- ④ 常駐の内訳 ---
        body = body.child(section(text::section_resident().to_string()));
        if snap.scan.is_none() && snap.machine.is_none() {
            body = body.child(muted(text::scan_unsupported().to_string()));
        } else if snap.scan.is_none() {
            body = body.child(muted(text::resident_empty().to_string()));
        }
        let max_mb = snap.panes.iter().map(|p| p.memory_mb).max().unwrap_or(0);
        for p in &snap.panes {
            body = body.child(self.render_resources_pane(p, max_mb, &theme, cx));
        }
        if snap.panes_total > snap.panes.len() {
            body = body.child(muted(text::more_panes(snap.panes_total - snap.panes.len())));
        }
        if let Some((mb, procs, heavy)) = snap.external {
            body = body.child(muted(text::external(mb, procs, heavy)));
        }
        if let Some((procs, age)) = snap.scan {
            body = body.child(
                div()
                    .px(px(10.0))
                    .py(px(2.0))
                    .text_size(px(10.0))
                    .text_color(hsla(theme.text_faint))
                    .child(SharedString::from(text::scan_line(procs, age))),
            );
        }
        root.child(body)
    }

    /// ② / ③ の 1 件（押すとそのペインへ。段のチップ・取り消しは行の中の別のボタン）
    fn render_resources_job(
        &self,
        job: &JobView,
        running: bool,
        theme: &tako_core::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = job.id;
        let kind = if running { "run" } else { "wait" };
        let probes = self.panel_click_probe_bounds.clone();
        let probe = move |key: String| crate::tab_shape::probe_canvas(probes.clone(), key);
        let prio_open = self.resources.priority_open == Some(id);
        let confirming = self.resources.confirm_cancel == Some(id);
        let level = job.priority.map_or("-", Priority::as_str);
        let chip_color = match job.priority {
            Some(Priority::Top) => theme.red,
            Some(Priority::High) => theme.peach,
            Some(Priority::Low) => theme.text_faint,
            _ => theme.text_muted,
        };
        let small_button = |key: String, label: SharedString, color: tako_core::Rgb| {
            div()
                .id(SharedString::from(key.clone()))
                .relative()
                .flex_none()
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(hsla(theme.border_subtle))
                .text_size(px(10.5))
                .text_color(hsla(color))
                .cursor_pointer()
                .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                .child(label)
        };

        // 1 行目: 順位・段のチップ・何（ラベル）・取り消し
        let mut head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .when(!running, |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(theme.font_family.clone())
                        .text_size(px(10.5))
                        .text_color(hsla(theme.text_muted))
                        .child(SharedString::from(text::position(
                            job.position.unwrap_or(0),
                        ))),
                )
            })
            .child(
                small_button(
                    format!("resources-prio-{id}"),
                    SharedString::from(level),
                    chip_color,
                )
                .when(prio_open, |d| d.border_color(hsla(theme.accent)))
                .tooltip({
                    let tip_theme = theme.clone();
                    move |_, cx| {
                        cx.new(|_| {
                            crate::tab_bar::HintTooltip::new(
                                text::priority_tip().to_string(),
                                tip_theme.clone(),
                            )
                        })
                        .into()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.resources.confirm_cancel = None;
                    this.resources.priority_open =
                        (this.resources.priority_open != Some(id)).then_some(id);
                    cx.notify();
                }))
                .child(probe(format!("resources-prio-{id}"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(theme.font_family.clone())
                    .text_size(px(11.0))
                    .child(SharedString::from(job.label.clone())),
            );
        if job.cancel {
            head = head.child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.yellow))
                    .child(text::cancelling()),
            );
        } else {
            head = head.child(
                small_button(
                    format!("resources-cancel-{id}"),
                    SharedString::from(text::cancel()),
                    theme.text_muted,
                )
                .tooltip({
                    let tip_theme = theme.clone();
                    move |_, cx| {
                        cx.new(|_| {
                            crate::tab_bar::HintTooltip::new(
                                text::cancel_tip().to_string(),
                                tip_theme.clone(),
                            )
                        })
                        .into()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.resources.priority_open = None;
                    this.resources.confirm_cancel =
                        (this.resources.confirm_cancel != Some(id)).then_some(id);
                    cx.notify();
                }))
                .child(probe(format!("resources-cancel-{id}"))),
            );
        }

        // 2 行目: 誰 / 3 行目: 数値（実行中 = CPU% · メモリ · 経過・進み・見込み / 待ち = 待ち時間・開始の見込み・理由）
        let who = text::owner(
            job.pane,
            job.tab.as_deref(),
            job.title.as_deref(),
            job.role.as_deref(),
        );
        let mut details: Vec<String> = Vec::new();
        if running {
            details.push(text::running_detail(
                job.cpu_percent,
                job.memory_mb,
                job.ran_s,
            ));
            if let Some(eta) = &job.eta {
                details.push(text::eta(
                    &eta.state,
                    eta.remaining_s,
                    eta.previous_s,
                    if eta.from_progress {
                        job.progress
                    } else {
                        None
                    },
                    job.ran_s.unwrap_or(0),
                ));
            }
            if job.stalled {
                details.push(text::stalled().to_string());
            }
        } else {
            details.push(text::waited(job.waited_s.unwrap_or(0)));
            if let Some(s) = job.start_s {
                details.push(text::start_eta(s));
            }
        }
        if let Some(from) = text::priority_from(job.priority_from.as_deref(), job.aged, job.base_priority)
        {
            details.push(from);
        }
        let line = |t: String, color: tako_core::Rgb| {
            div()
                .text_size(px(10.5))
                .text_color(hsla(color))
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(SharedString::from(t))
        };
        let mut row = div()
            .id(SharedString::from(format!("resources-row-{kind}-{id}")))
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .mx(px(4.0))
            .px(px(6.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .when(job.pane.is_some(), |d| {
                let pane = job.pane.unwrap_or(0);
                d.cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .tooltip({
                        let tip_theme = theme.clone();
                        move |_, cx| {
                            cx.new(|_| {
                                crate::tab_bar::HintTooltip::new(
                                    text::focus_tip().to_string(),
                                    tip_theme.clone(),
                                )
                            })
                            .into()
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.resources_focus_pane(pane, cx);
                    }))
            })
            .child(head)
            .child(line(who, theme.text_secondary));
        // 進みのバー（cargo の N/M。#1996 の材料）
        if let Some((done, total)) = job.progress.filter(|_| running) {
            let frac = (done as f32 / total.max(1) as f32).clamp(0.0, 1.0);
            row = row.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .h(px(3.0))
                            .rounded(px(2.0))
                            .bg(rgba(theme.surface_highlight))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(frac))
                                    .bg(hsla(theme.accent)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.font_family.clone())
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(format!("{done}/{total}"))),
                    ),
            );
        }
        row = row.child(line(details.join(" · "), theme.text_muted));
        if !running {
            if let Some(reason) = text::wait_reason(job.reason.as_deref(), job.reason_text.as_deref())
            {
                row = row.child(line(reason, theme.text_faint));
            }
        }

        // 段の選択（この処理 / ペインの既定）
        if prio_open {
            let chips = |scope: &'static str,
                         current: Option<&'static str>,
                         pane: Option<u64>,
                         title: String| {
                let mut r = div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex_none()
                            .w(px(84.0))
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(SharedString::from(title)),
                    );
                for choice in PRIORITY_CHOICES {
                    let selected = current == Some(choice);
                    let key = format!("resources-prio-{scope}-{id}-{choice}");
                    r = r.child(
                        div()
                            .id(SharedString::from(key.clone()))
                            .relative()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(hsla(if selected {
                                theme.accent
                            } else {
                                theme.border_subtle
                            }))
                            .when(selected, |d| d.bg(rgba(theme.accent_muted)))
                            .text_size(px(10.0))
                            .text_color(hsla(if selected {
                                theme.foreground
                            } else {
                                theme.text_muted
                            }))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.resources.priority_open = None;
                                let (job_id, target) = match pane {
                                    Some(p) => (None, Some(p)),
                                    None => (Some(id), None),
                                };
                                this.resources_set_priority(job_id, target, choice, cx);
                            }))
                            .child(SharedString::from(if choice == "default" {
                                text::priority_default()
                            } else {
                                choice
                            }))
                            .child(probe(key)),
                    );
                }
                r
            };
            let job_current = match job.priority_from.as_deref() {
                Some("job") => job.job_priority.map(Priority::as_str),
                _ => Some("default"),
            };
            let mut menu = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .mt(px(2.0))
                .p(px(6.0))
                .rounded(px(5.0))
                .border_1()
                .border_color(hsla(theme.border_subtle))
                .bg(rgba(theme.surface_0))
                .child(chips(
                    "job",
                    job_current,
                    None,
                    text::priority_job_row().to_string(),
                ));
            if let Some(pane) = job.pane {
                let pane_current = job.pane_priority.map_or("default", Priority::as_str);
                menu = menu.child(chips(
                    "pane",
                    Some(pane_current),
                    Some(pane),
                    text::priority_pane_row(pane),
                ));
            }
            row = row.child(menu);
        }

        // 取り消しの確かめ（行の中の枠。ダイアログは出さない）
        if confirming {
            let label = job.label.clone();
            row = row.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .mt(px(2.0))
                    .p(px(6.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(hsla_alpha(theme.red, 0.5))
                    .bg(rgba_alpha(theme.red, 0.08))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(10.5))
                            .child(SharedString::from(text::cancel_confirm(&label, running))),
                    )
                    .child(
                        small_button(
                            format!("resources-cancel-yes-{id}"),
                            SharedString::from(text::cancel_yes()),
                            theme.red,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.resources_cancel(id, cx);
                        }))
                        .child(probe(format!("resources-cancel-yes-{id}"))),
                    )
                    .child(
                        small_button(
                            format!("resources-cancel-no-{id}"),
                            SharedString::from(text::cancel_no()),
                            theme.text_muted,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.resources.confirm_cancel = None;
                            cx.notify();
                        }))
                        .child(probe(format!("resources-cancel-no-{id}"))),
                    ),
            );
        }
        row.child(probe(format!("resources-row-{kind}-{id}")))
    }

    /// ④ の 1 ペイン（押すとそのペインへ）
    fn render_resources_pane(
        &self,
        p: &PaneView,
        max_mb: u64,
        theme: &tako_core::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let pane = p.pane;
        let mut who = text::owner(
            Some(pane),
            p.tab.as_deref(),
            p.title.as_deref(),
            p.role.as_deref(),
        );
        if p.shelved {
            who = format!("{who} · {}", text::shelved());
        }
        let frac = if max_mb > 0 {
            (p.memory_mb as f32 / max_mb as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        div()
            .id(SharedString::from(format!("resources-pane-{pane}")))
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .mx(px(4.0))
            .px(px(6.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|d| d.bg(rgba(theme.surface_hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.resources_focus_pane(pane, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(SharedString::from(who)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.font_family.clone())
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(text::size(p.memory_mb))),
                    ),
            )
            .child(
                div()
                    .h(px(3.0))
                    .w_full()
                    .rounded(px(2.0))
                    .bg(rgba(theme.surface_highlight))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(frac))
                            .bg(hsla(theme.teal)),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_muted))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(text::resident_detail(
                        p.agent.as_deref(),
                        p.agent_mb,
                        p.heavy_mb,
                        p.other_mb(),
                        p.cpu_percent,
                    ))),
            )
            .child(crate::tab_shape::probe_canvas(
                self.panel_click_probe_bounds.clone(),
                format!("resources-pane-{pane}"),
            ))
    }
}

/// `Request::Queue` を組む（CLI `tako queue` の要求と同じ形）
fn queue_request(
    action: &str,
    id: Option<u64>,
    pane: Option<u64>,
    priority: Option<&str>,
) -> tako_control::protocol::Request {
    tako_control::protocol::Request::Queue {
        action: Some(action.to_string()),
        job: None,
        id,
        limits: None,
        pane,
        priority: priority.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Value {
        json!({
            "gui": true,
            "machine": {
                "memory_mb": {"total": 36864, "used": 24000, "app": 15000, "wired": 4000,
                              "compressed": 5000, "cached": 6000, "available": 12864},
                "swap_mb": {"used": 1024, "total": 2048},
                "pressure": "warn",
                "cpu_percent": 23.4,
                "load": [4.12, 3.9, 3.51],
                "cpus": 18,
                "age_ms": 300,
            },
            "heavy": [],
            "panes": [
                {"pane": 7, "tab": "dev", "title": "w1", "role": "orchestrator-worker:tako",
                 "agent": "claude", "memory_mb": 3400, "agent_mb": 2100, "heavy_mb": 800,
                 "heavy": 1, "cpu_percent": 12.0, "processes": 30},
                {"pane": 9, "shelved": true, "memory_mb": 100, "agent_mb": 0, "heavy_mb": 0,
                 "cpu_percent": 0.0, "processes": 2},
            ],
            "panes_total": 5,
            "external": {"memory_mb": 8200, "cpu_percent": 3.0, "processes": 412, "heavy": 1},
            "scan": {"processes": 1320, "readable": 1300, "age_ms": 1200, "interval_ms": 2000, "took_ms": 11.0},
            "queue": {
                "arbiter": true,
                "limits": {"max_jobs": 3},
                "running": [{
                    "id": 4242, "label": "cargo test -p tako-core", "pane": 7, "tab": "dev",
                    "title": "w1", "role": "orchestrator-worker:tako", "pane_priority": "low",
                    "memory_mb": 1300, "cpu_percent": 120.5, "ran_s": 200, "stalled": false,
                    "cancel": false, "progress": {"done": 34, "total": 120},
                    "eta": {"state": "remaining", "remaining_s": 180, "previous_s": 420,
                            "from": "history", "progress": {"done": 34, "total": 120}, "text": "x"},
                    "priority": "normal", "base_priority": "normal", "priority_from": "role",
                    "aged": false, "job_priority": null,
                }],
                "waiting": [{
                    "id": 4343, "position": 1, "label": "cargo build", "pane": null,
                    "waited_s": 130, "reason": "max_jobs", "text": "同時数の上限（実行 1 / 上限 1）",
                    "start_s": 240, "priority": "high", "base_priority": "normal",
                    "priority_from": "job", "aged": true, "job_priority": "normal",
                }],
                "unmanaged": 2,
            },
        })
    }

    #[test]
    fn statusの答えを描く形へ写す() {
        let s = ResourcesSnapshot::from_status(&status());
        let m = s.machine.expect("machine");
        assert_eq!(m.memory, Some((24000, 36864, 5000, 12864)));
        assert_eq!(m.swap, Some((1024, 2048)));
        assert_eq!(m.pressure, Some(Pressure::Warn));
        assert_eq!(m.cpus, 18);
        assert_eq!(m.load, Some([4.12, 3.9, 3.51]));
        assert!(s.queue.arbiter);
        assert_eq!(s.queue.max_jobs, Some(3));
        assert_eq!(s.queue.unmanaged, 2);
        let r = &s.queue.running[0];
        assert_eq!((r.id, r.pane, r.progress), (4242, Some(7), Some((34, 120))));
        assert_eq!(r.tab.as_deref(), Some("dev"));
        assert_eq!(r.pane_priority, Some(Priority::Low));
        assert_eq!(r.priority, Some(Priority::Normal));
        assert_eq!(r.eta.as_ref().map(|e| e.from_progress), Some(false));
        let w = &s.queue.waiting[0];
        assert_eq!((w.id, w.position, w.pane), (4343, Some(1), None));
        assert_eq!(w.job_priority, Some(Priority::Normal));
        assert!(w.aged);
        assert_eq!(w.reason.as_deref(), Some("max_jobs"));
        assert_eq!(s.panes.len(), 2);
        assert_eq!(s.panes_total, 5);
        assert_eq!(s.panes[0].other_mb(), 500, "3400 − agent 2100 − 重い処理 800");
        assert!(s.panes[1].shelved);
        assert_eq!(s.external, Some((8200, 412, 1)));
        assert_eq!(s.scan, Some((1320, 1200)));
        assert_eq!(
            s.bar(),
            BarView {
                memory: Some((24000, 36864)),
                pressure: Some(Pressure::Warn),
                running: 1,
                waiting: 1,
            }
        );
    }

    /// Windows（機の値も走査も無い）: 機は「未対応」、区画のメモリも未対応。順番待ちは出る
    #[test]
    fn 値が無いosは未対応として写す() {
        let v = json!({
            "gui": true,
            "machine": null,
            "machine_unavailable": "この OS では未実装",
            "heavy": [],
            "panes": [],
            "queue": {"arbiter": true, "limits": {"max_jobs": 1}, "running": [], "waiting": [], "unmanaged": 0},
        });
        let s = ResourcesSnapshot::from_status(&v);
        assert!(s.machine.is_none());
        assert!(s.scan.is_none());
        assert!(s.external.is_none());
        assert!(s.queue.arbiter);
        assert_eq!(s.bar().memory, None);
        assert_eq!((s.bar().running, s.bar().waiting), (0, 0));
    }

    /// 調停役でない tako（隔離・セカンダリ）と、壊れた・欠けた答え（旧い GUI の答え）でも落ちない
    #[test]
    fn 調停役でない答えと欠けた答え() {
        let s = ResourcesSnapshot::from_status(&json!({
            "machine": {"memory_mb": null, "pressure": "critical", "cpus": 4},
            "queue": {"arbiter": false, "reason": "この tako は順番待ちの調停役ではない"},
        }));
        assert!(!s.queue.arbiter);
        assert!(s.queue.running.is_empty());
        let m = s.machine.expect("機の値はある");
        assert_eq!(m.memory, None, "メモリの内訳が採れない");
        assert_eq!(m.pressure, Some(Pressure::Critical));
        assert_eq!(s.bar().memory, None);
        let empty = ResourcesSnapshot::from_status(&json!(null));
        assert_eq!(empty, ResourcesSnapshot::default());
        // 行の欠けた欄（旧い調停役 = 誰・段の欄が無い）
        let s = ResourcesSnapshot::from_status(&json!({
            "queue": {"arbiter": true, "running": [{"id": 1}], "waiting": [{"id": 2, "label": "x"}]},
        }));
        assert_eq!(s.queue.running[0].label, "?");
        assert_eq!(s.queue.running[0].priority, None);
        assert_eq!(s.queue.waiting[0].position, None);
    }

    /// 後から頼んだ読みが先に着いたら古い答えで上書きしない・消えた行の確かめは畳む
    #[test]
    fn 古い答えで上書きせず消えた行の確かめを畳む() {
        let mut p = ResourcesPanel::default();
        let a = p.next_seq();
        let b = p.next_seq();
        let snap = ResourcesSnapshot::from_status(&status());
        assert!(p.apply(b, Ok(snap.clone())));
        assert!(!p.apply(a, Ok(ResourcesSnapshot::default())), "古い答え");
        assert_eq!(p.snapshot.as_ref(), Some(&snap));
        p.confirm_cancel = Some(4343);
        p.priority_open = Some(4242);
        let c = p.next_seq();
        let mut gone = snap.clone();
        gone.queue.waiting.clear();
        p.apply(c, Ok(gone));
        assert_eq!(p.confirm_cancel, None, "待ちから消えた行の確かめは畳む");
        assert_eq!(p.priority_open, Some(4242), "残っている行はそのまま");
        let d = p.next_seq();
        assert!(p.apply(d, Err("x".into())));
        assert!(p.snapshot.is_some(), "失敗しても前の答えは残す");
        assert_eq!(p.error.as_deref(), Some("x"));
    }

    #[test]
    fn メモリ圧の色はnormal_warn_critical() {
        let theme = tako_core::Theme::default_dark();
        assert_eq!(pressure_color(&theme, Some(Pressure::Critical)), theme.red);
        assert_eq!(pressure_color(&theme, Some(Pressure::Warn)), theme.yellow);
        assert_eq!(pressure_color(&theme, Some(Pressure::Normal)), theme.accent);
        assert_eq!(pressure_color(&theme, None), theme.accent);
    }
}
