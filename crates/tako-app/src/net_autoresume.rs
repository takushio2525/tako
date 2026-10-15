//! ネット復帰の即時再開（FR-2.27.17 / Issue #2112）の駆動。
//!
//! 判断は `tako_core::net_resume` の純関数、画面からの停止の切り出しと到達確認の I/O は
//! `tako_control::net_stop` が持つ。ここは 2 秒 tick で材料を集め、到達確認を 1 本だけ
//! 背景のスレッドで走らせ、通ったら再開の文面を送達確認つきの経路へ積む層。
//!
//! # どの機構に載せたか
//!
//! リミット後の自動復帰（#813 / #1945。`limit_autoresume`）と**同じ層・同じ道具**に載せた:
//! 2 秒 tick で退避中を含む全ペインを見る・送るのは `queue_prompt_flow`（送達確認つき。#32 / #790）・
//! 人の下書きは入力欄の属性で見る・口（CLI / MCP）は `tako limit-resume` / `tako_limit_resume` を
//! 共有する（`--network` / `network`）。supervisor（#401）は watch の外側ループとして書かれているが
//! 呼び出し元が無く（2026-10-15 時点）、worker 以外（master / solo / 手起動の claude）も見ないので
//! 載せられない。違いは 2 つだけ: ① 既定 ON の**全体の設定**（ペインのオプトインではない）
//! ② 時刻ではなく**回線の到達**を合図にする（到達確認は止まったペインがあるあいだだけ）
//!
//! # 2 秒 tick を重くしない
//!
//! 設定が OFF なら最初の 1 行で抜ける。ON でも読むのはエージェントのペインの画面末尾
//! （メモリ上のグリッド）だけで、プロセスは起こさない。到達確認（名前解決 + TCP 接続）は
//! **送ってよい停止が 1 つ以上あるときだけ**、1 本ずつ背景のスレッドで走る
//! （UI スレッドでは名前解決をしない。結果は 500ms ループが拾う）

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tako_control::host::SessionHost;
use tako_control::net_stop::{NetScreen, NetStop};
use tako_core::net_resume::{self, ConnErrorKind, Hold, ProbeTarget, ResumeInput};
use tako_core::PaneId;

use crate::TakoApp;

/// 1 ペインの停止（1 エピソード = 接続エラーで止まってから、その停止が画面から消えるまで）
#[derive(Debug, Clone)]
struct NetStopTracker {
    kind: ConnErrorKind,
    /// 停止の鍵（`NetStop::key`。同じ停止かどうかの正本）
    key: u64,
    /// 停止の根拠の 1 論理行（応答にだけ出す。診断ログには出さない）
    message: String,
    /// この停止を最初に見た時刻（unix 秒）。同じ鍵の停止が [`net_resume::STABLE_SECS`] 秒
    /// 続いてから送る。**画面全体の指紋では測らない**（フッターの時計・ctx% や帯は止まって
    /// いても動くので、指紋で測ると「静止」が永遠に来ない）
    first_seen: i64,
    /// この停止へ送った時刻（同じ停止に 2 回送らない）
    sent_at: Option<i64>,
    /// 直近の判断で見送った理由
    hold: Option<Hold>,
    /// persist.log へ書いた見送りの理由（同じ理由を 2 秒ごとに書かない）
    logged_hold: Option<Hold>,
    /// 選択ダイアログの裏にある停止か
    dialog: bool,
}

/// 到達確認の結果（背景のスレッドが書き、500ms ループが拾う）
type ProbeSlot = Arc<Mutex<Option<Result<Duration, String>>>>;

/// 到達確認の状態
#[derive(Debug, Default)]
struct ProbeState {
    /// 先（最初に確かめる時に env から決める。以後は同じ先を見る）
    target: Option<ProbeTarget>,
    slot: ProbeSlot,
    in_flight: bool,
    last_finished: Option<Instant>,
    /// 起動してからの到達確認の回数（「止まっていないあいだは 0 回」を外から確かめる材料）
    count: u64,
    /// 届かなかった最初の時刻（unix 秒。届いたら消える）
    outage_since: Option<i64>,
    last_at: Option<i64>,
    last_ok: Option<bool>,
    last_detail: Option<String>,
    /// 到達確認の期間中か（persist.log の「始めた / 止めた」を 1 行ずつにする）
    active: bool,
}

/// ネット復帰の即時再開の状態（TakoApp のフィールド）
#[derive(Debug)]
pub(crate) struct NetResumeState {
    /// 全体の設定（settings.json の `network_resume`。既定 ON）
    pub(crate) enabled: bool,
    trackers: HashMap<PaneId, NetStopTracker>,
    probe: ProbeState,
    /// ペインごとの再開の時刻（打ち切りの判断。エピソードをまたいで持つ）
    resumes: HashMap<PaneId, Vec<i64>>,
    /// ペインごとの最後に送った停止の鍵。追跡が途切れても（ペインを上へスクロールして
    /// 入力欄が画面外へ出た等）、同じ停止を見つけ直して 2 回送らないために持つ
    sent_keys: HashMap<PaneId, u64>,
    /// 直近の再開の回（応答に出す）
    last_round: Option<serde_json::Value>,
    total_resumed: u64,
}

impl NetResumeState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            trackers: HashMap::new(),
            probe: ProbeState::default(),
            resumes: HashMap::new(),
            sent_keys: HashMap::new(),
            last_round: None,
            total_resumed: 0,
        }
    }

    /// 到達確認の結果を待っているか（500ms ループが拾うかどうかの判断。ロックは取らない）
    pub(crate) fn probe_in_flight(&self) -> bool {
        self.probe.in_flight
    }
}

impl TakoApp {
    /// 2 秒 tick から呼ぶ。止まったペインを拾い直し、送ってよい停止があれば到達確認を始める
    pub(crate) fn drive_net_autoresume(&mut self) {
        if !self.net_resume.enabled {
            if !self.net_resume.trackers.is_empty() {
                self.net_resume.trackers.clear();
            }
            self.stop_net_probe_period("disabled");
            return;
        }
        let now = tako_core::limit_resume::now_unix();
        let targets: Vec<(PaneId, bool)> = self
            .workspace
            .all_panes()
            .into_iter()
            .filter(|p| {
                tako_core::limit_resume_all::is_agent_pane(
                    p,
                    self.limit_resume_agent_detected(p.id()),
                )
            })
            .map(|p| (p.id(), self.workspace.background_pane(p.id()).is_some()))
            .collect();
        // 閉じた・エージェントでなくなったペインの追跡は捨てる（ペイン ID の再利用で取り違えない）
        self.net_resume
            .trackers
            .retain(|id, _| targets.iter().any(|(t, _)| t == id));
        self.net_resume
            .resumes
            .retain(|id, _| targets.iter().any(|(t, _)| t == id));
        self.net_resume
            .sent_keys
            .retain(|id, _| targets.iter().any(|(t, _)| t == id));

        let mut ready = 0usize;
        for (pane, shelved) in targets {
            let Some(screen) = self.observe_net_stop(pane) else {
                self.net_resume.trackers.remove(&pane);
                continue;
            };
            let (stop, dialog) = screen;
            let tracker = match self.net_resume.trackers.get_mut(&pane) {
                Some(t) if t.key == stop.key => t,
                _ => {
                    // 新しい停止。ただし前に送った停止を見つけ直しただけなら送らない
                    let already = self.net_resume.sent_keys.get(&pane) == Some(&stop.key);
                    if !already {
                        tako_control::diag::persist_log(&format!(
                            "[net-resume] pane={} 接続エラーで停止 kind={} shelved={shelved}",
                            pane.as_u64(),
                            stop.kind.as_str()
                        ));
                    }
                    self.net_resume.trackers.insert(
                        pane,
                        NetStopTracker {
                            kind: stop.kind,
                            key: stop.key,
                            message: stop.message.clone(),
                            first_seen: now,
                            sent_at: already.then_some(now),
                            hold: None,
                            logged_hold: None,
                            dialog,
                        },
                    );
                    self.net_resume
                        .trackers
                        .get_mut(&pane)
                        .expect("直前に挿入した")
                }
            };
            tracker.message = stop.message;
            tracker.dialog = dialog;
            match self.net_resume_decide(pane, now) {
                Ok(()) => {
                    if let Some(t) = self.net_resume.trackers.get_mut(&pane) {
                        t.hold = None;
                    }
                    ready += 1;
                }
                Err(hold) => self.note_net_hold(pane, hold),
            }
        }
        if ready == 0 {
            self.stop_net_probe_period("nothing_to_resume");
            return;
        }
        self.maybe_start_net_probe(ready);
    }

    /// 画面を読んで停止を切り出す。(停止, ダイアログ中か)。止まっていなければ None
    fn observe_net_stop(&self, pane: PaneId) -> Option<(NetStop, bool)> {
        let session = self.terminals.get(&pane)?;
        // 普段は画面末尾だけ。入力欄の上に会話の項目が見えないときだけ全体を読む
        let tail = session.tail_lines(tako_core::terminal::AGENT_TUI_TAIL_LINES);
        let lines = if tako_control::net_stop::needs_full_screen(&tail) {
            session.visible_lines()
        } else {
            tail
        };
        match tako_control::net_stop::detect(&lines) {
            NetScreen::Stopped(s) => Some((s, false)),
            NetScreen::Dialog(s) => Some((s, true)),
            NetScreen::Clear => None,
        }
    }

    /// 送ってよいかを判断する（材料はその場で取る = 送る直前にも同じ判断を通す）
    fn net_resume_decide(&self, pane: PaneId, now: i64) -> Result<(), Hold> {
        let Some(t) = self.net_resume.trackers.get(&pane) else {
            return Err(Hold::AlreadyResumed);
        };
        // 下書きの判定は文字の属性（dim = 提案 / それ以外 = 人の文字）まで要るので画面を組む。
        // 止まったペインに限って行う（2 秒 tick の全ペインでは組まない）
        let user_draft = !t.dialog
            && t.sent_at.is_none()
            && self
                .terminals
                .get(&pane)
                .and_then(|s| s.analyze_input())
                .is_some_and(|s| s.style.is_user_draft());
        net_resume::decide(&ResumeInput {
            now,
            stable_since: t.first_seen,
            already_sent: t.sent_at.is_some(),
            user_draft,
            dialog: t.dialog,
            delivering: self.prompt_flows.iter().any(|f| f.pane == pane),
            recent_resumes: self
                .net_resume
                .resumes
                .get(&pane)
                .map(|v| net_resume::recent_resumes(v, now))
                .unwrap_or(0),
        })
    }

    /// 見送りを記録する。persist.log へは**理由が変わったときだけ** 1 行
    /// （画面の静止待ち・送った後の待ちは毎回の通過点なので書かない）
    fn note_net_hold(&mut self, pane: PaneId, hold: Hold) {
        let Some(t) = self.net_resume.trackers.get_mut(&pane) else {
            return;
        };
        t.hold = Some(hold);
        let quiet = matches!(hold, Hold::ScreenUnstable | Hold::AlreadyResumed);
        if !quiet && t.logged_hold != Some(hold) {
            t.logged_hold = Some(hold);
            tako_control::diag::persist_log(&format!(
                "[net-resume] pane={} 見送り reason={}",
                pane.as_u64(),
                hold.as_str()
            ));
        }
    }

    /// 到達確認を始める（走っている・間隔が空いていなければ何もしない）
    fn maybe_start_net_probe(&mut self, ready: usize) {
        let now = tako_core::limit_resume::now_unix();
        let probe = &mut self.net_resume.probe;
        if probe.in_flight {
            return;
        }
        let interval = net_resume::probe_interval_secs(probe.outage_since.map(|t| now - t));
        if probe
            .last_finished
            .is_some_and(|t| t.elapsed() < Duration::from_secs(interval as u64))
        {
            return;
        }
        let target = probe
            .target
            .get_or_insert_with(net_resume::probe_target)
            .clone();
        if !probe.active {
            probe.active = true;
            tako_control::diag::persist_log(&format!(
                "[net-resume] 到達確認を始める target={} source={} 待ち {ready} 本",
                target.display(),
                target.source.as_str()
            ));
        }
        probe.in_flight = true;
        probe.count += 1;
        let slot = probe.slot.clone();
        let spawned = std::thread::Builder::new()
            .name("tako-net-probe".into())
            .spawn(move || {
                let result = tako_control::net_stop::probe(
                    &target,
                    Duration::from_millis(net_resume::PROBE_TIMEOUT_MS),
                );
                if let Ok(mut s) = slot.lock() {
                    *s = Some(result);
                }
            });
        if let Err(e) = spawned {
            probe.in_flight = false;
            tako_control::diag::persist_log(&format!(
                "[net-resume] 到達確認のスレッドを起こせない: {e}"
            ));
        }
    }

    /// 到達確認の期間を閉じる（送ってよい停止が無くなった・設定が OFF になった）
    fn stop_net_probe_period(&mut self, why: &str) {
        let probe = &mut self.net_resume.probe;
        if probe.active {
            probe.active = false;
            probe.outage_since = None;
            tako_control::diag::persist_log(&format!(
                "[net-resume] 到達確認を止めた reason={why} 累計 {} 回",
                probe.count
            ));
        }
    }

    /// 500ms ループから呼ぶ。到達確認が終わっていれば結果を取り込み、通っていれば送る
    pub(crate) fn poll_net_probe(&mut self) {
        let result = match self.net_resume.probe.slot.lock() {
            Ok(mut s) => s.take(),
            Err(_) => None,
        };
        let Some(result) = result else {
            return;
        };
        let now = tako_core::limit_resume::now_unix();
        let enabled = self.net_resume.enabled;
        let probe = &mut self.net_resume.probe;
        probe.in_flight = false;
        // 確かめているあいだに OFF にされたら結果を捨てる（OFF で送らない）
        if !enabled {
            return;
        }
        probe.last_finished = Some(Instant::now());
        probe.last_at = Some(now);
        let target = probe
            .target
            .as_ref()
            .map(ProbeTarget::display)
            .unwrap_or_default();
        match result {
            Ok(elapsed) => {
                probe.last_ok = Some(true);
                probe.last_detail = Some(format!("{}ms", elapsed.as_millis()));
                let outage = probe.outage_since.take().map(|t| now - t);
                self.resume_net_stopped(&target, elapsed, outage);
            }
            Err(e) => {
                probe.last_ok = Some(false);
                if probe.outage_since.is_none() {
                    probe.outage_since = Some(now);
                    // 断のあいだは 1 行だけ（数秒おきの失敗を全部は書かない）
                    tako_control::diag::persist_log(&format!(
                        "[net-resume] 到達できない target={target} err={e} 回線の復帰を待つ"
                    ));
                }
                probe.last_detail = Some(e);
            }
        }
    }

    /// 回線の復帰を見たので、送ってよい停止へ「続き」を積む。見送りは理由つきで残す
    fn resume_net_stopped(&mut self, target: &str, elapsed: Duration, outage: Option<i64>) {
        let now = tako_core::limit_resume::now_unix();
        let mut panes: Vec<PaneId> = self.net_resume.trackers.keys().copied().collect();
        panes.sort_by_key(|p| p.as_u64());
        let mut resumed: Vec<u64> = Vec::new();
        let mut skipped: Vec<(u64, &'static str)> = Vec::new();
        for pane in panes {
            // 送る直前に画面をもう一度読む（到達確認のあいだに人が送った・閉じた等）
            let still = self.observe_net_stop(pane).is_some_and(|(s, _)| {
                self.net_resume
                    .trackers
                    .get(&pane)
                    .is_some_and(|t| t.key == s.key)
            });
            if !still {
                self.net_resume.trackers.remove(&pane);
                continue;
            }
            match self.net_resume_decide(pane, now) {
                Ok(()) => {
                    self.queue_prompt_flow(pane, net_resume::nudge_prompt());
                    if let Some(t) = self.net_resume.trackers.get_mut(&pane) {
                        t.sent_at = Some(now);
                        t.hold = Some(Hold::AlreadyResumed);
                        self.net_resume.sent_keys.insert(pane, t.key);
                    }
                    self.net_resume.resumes.entry(pane).or_default().push(now);
                    self.net_resume.total_resumed += 1;
                    resumed.push(pane.as_u64());
                }
                // 前の回で送ったものは「見送り」ではない（数えない）
                Err(Hold::AlreadyResumed) => {}
                Err(hold) => {
                    self.note_net_hold(pane, hold);
                    skipped.push((pane.as_u64(), hold.as_str()));
                }
            }
        }
        if resumed.is_empty() && skipped.is_empty() {
            // 確かめているあいだに停止が消えた（人が送った等）。記録する回が無い
            self.stop_net_probe_period("nothing_to_resume");
            return;
        }
        // 断を見ていたら「復帰」、止まった時点でもう通っていたら「到達を確認」
        let (what, outage_text) = match outage {
            Some(s) => ("回線の復帰を検知", format!(" 断 {s} 秒")),
            None => ("到達を確認", String::new()),
        };
        tako_control::diag::persist_log(&format!(
            "[net-resume] {what} target={target} {}ms{outage_text}: 再開 {} 本 {resumed:?} / 見送り {} 本 [{}]",
            elapsed.as_millis(),
            resumed.len(),
            skipped.len(),
            skipped
                .iter()
                .map(|(p, r)| format!("{p}:{r}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        self.net_resume.last_round = Some(serde_json::json!({
            "at": now,
            "target": target,
            "elapsed_ms": elapsed.as_millis() as u64,
            "outage_secs": outage,
            "resumed": resumed,
            "skipped": skipped
                .iter()
                .map(|(p, r)| serde_json::json!({ "pane": p, "reason": r }))
                .collect::<Vec<_>>(),
        }));
        // 送ってよい停止が残っていなければ、到達確認の期間はここで閉じる
        if !self
            .net_resume
            .trackers
            .values()
            .any(|t| t.sent_at.is_none() && t.hold.is_none())
        {
            self.stop_net_probe_period("resumed");
        }
    }

    /// 全体の設定を書き換える（設定画面のトグル・CLI・MCP はすべて dispatch からここへ来る）。
    /// OFF にしたら追跡と到達確認をその場で畳む
    pub(crate) fn apply_network_resume_setting(&mut self, enabled: bool) {
        self.net_resume.enabled = enabled;
        if !enabled {
            self.net_resume.trackers.clear();
            self.stop_net_probe_period("disabled");
        }
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.network_resume = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                // 次の起動で設定が戻るので黙らない（GUI の stderr は誰も読めない = #1399）
                tako_control::diag::persist_log(&format!(
                    "[net-resume] settings.json へ書けない: {e}"
                ));
            }
        }
    }

    /// `tako limit-resume --network` / `--all` の `network` に載せる実行状態
    pub(crate) fn net_resume_status_json(&self) -> serde_json::Value {
        let now = tako_core::limit_resume::now_unix();
        let mut stopped: Vec<(u64, serde_json::Value)> = self
            .net_resume
            .trackers
            .iter()
            .map(|(pane, t)| (pane.as_u64(), self.net_tracker_json(*pane, t, now)))
            .collect();
        stopped.sort_by_key(|(p, _)| *p);
        let probe = &self.net_resume.probe;
        serde_json::json!({
            "stopped": stopped.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            "probe": {
                "target": probe.target.as_ref().map(ProbeTarget::display),
                "active": probe.active,
                "running": probe.in_flight,
                "count": probe.count,
                "last_at": probe.last_at,
                "last_ok": probe.last_ok,
                "last_detail": probe.last_detail,
                "outage_since": probe.outage_since,
            },
            "last_round": self.net_resume.last_round,
            "total_resumed": self.net_resume.total_resumed,
        })
    }

    /// 1 ペインの停止の状態（止まっていなければ None）
    pub(crate) fn net_resume_state_json(&self, pane: PaneId) -> Option<serde_json::Value> {
        let t = self.net_resume.trackers.get(&pane)?;
        Some(self.net_tracker_json(pane, t, tako_core::limit_resume::now_unix()))
    }

    fn net_tracker_json(&self, pane: PaneId, t: &NetStopTracker, now: i64) -> serde_json::Value {
        let state = match (t.sent_at, t.hold) {
            (Some(_), _) => "resumed",
            (None, None) => "waiting_network",
            (None, Some(_)) => "held",
        };
        serde_json::json!({
            "pane": pane.as_u64(),
            "kind": t.kind.as_str(),
            "message": t.message,
            "since": t.first_seen,
            "stopped_secs": now - t.first_seen,
            "state": state,
            "hold": t.hold.filter(|h| *h != Hold::AlreadyResumed).map(Hold::as_str),
            "resumed_at": t.sent_at,
            "shelved": self.workspace.background_pane(pane).is_some(),
        })
    }
}
