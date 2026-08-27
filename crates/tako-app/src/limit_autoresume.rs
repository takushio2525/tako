//! 利用上限（5h / 週次）後のペイン単位の自動復帰の駆動（Issue #813）。
//!
//! 判断は `tako_core::limit_resume` の純関数、検知は `tako_control::limit_stop` が持つ。
//! ここは 2 秒 tick で材料を集めて判断へ渡し、決まった動作を実行する層。
//!
//! # 2 秒 tick を重くしない
//!
//! 既定 OFF なので、**有効なペインが 1 つも無ければ最初の 1 行で抜ける**。
//! 有効なペインがあるときだけ、そのペインの可視画面（メモリ上のグリッド）を読む。
//! 新しいポーリング・サブプロセス起動は増やさない（#772 / #779 の教訓）。
//!
//! # 動作の分担
//!
//! - idle 型 = 継続ナッジ。既存の `queue_prompt_flow`（送達確認つき。#32 / #790）へ積むだけ
//! - ダイアログ型 = キー送出を伴うので**バックグラウンド**で `respond_to_choice_dialog`
//!   を通す（1 回あたり数百 ms のスリープが入るので UI スレッドでは走らせない）

use std::collections::HashMap;

use tako_control::host::{SessionHost, TmuxHost};
use tako_core::limit_resume::{
    self, HoldReason, LimitStop, LimitStopKind, ResumeAction, ResumeDecision, ResumeInput,
};
use tako_core::PaneId;

use crate::TakoApp;

/// codex の構造化レート制限を読み直す間隔（#985）。
///
/// 短くしても意味が無い: 使用率は 1 ターンごとにしか動かず、リセット時刻は枠のあいだ不変。
/// **長くしすぎない**のは、上限に当たったペインが正確な解除時刻を早く得られるようにするため
const CODEX_LIMITS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// codex レート制限の読み直しを間引くための状態
#[derive(Debug, Clone, Default)]
pub(crate) struct CodexLimitsScan {
    last: Option<std::time::Instant>,
}

impl CodexLimitsScan {
    /// 読み直す時期か（対象が 1 つも無ければ常に false = 何も起こさない）
    pub(crate) fn due(&self, targets: usize, now: std::time::Instant) -> bool {
        targets > 0
            && self
                .last
                .is_none_or(|t| now.duration_since(t) >= CODEX_LIMITS_INTERVAL)
    }

    pub(crate) fn mark(&mut self, now: std::time::Instant) {
        self.last = Some(now);
    }
}

/// background で codex のレート制限を集める（#985）。
///
/// **プロセス起動を増やさない**のが要点: 呼び出し側が既に採った `ProcessSnapshot` を
/// 1 枚だけ使い回すので、対象が何ペインでも tmux / ps は増えない（#772 / #779 の枠組み）。
/// `lsof` は codex を見つけたペインに 1 回ずつだけ（結果は sticky）
pub(crate) fn scan_codex_limits(
    targets: &[(PaneId, String)],
    snapshot: &tako_control::agents::ProcessSnapshot,
) -> HashMap<PaneId, tako_control::codex_session::RateLimits> {
    let backends: Vec<String> = targets.iter().map(|(_, b)| b.clone()).collect();
    let threads = tako_control::codex_session::resolve_thread_ids_with(&backends, snapshot);
    let mut out = HashMap::new();
    for (pane, backend) in targets {
        let Some(tid) = threads.get(backend) else {
            continue;
        };
        if let Some(rl) =
            tako_control::codex_session::read_turn_state(tid).and_then(|st| st.rate_limits)
        {
            out.insert(*pane, rl);
        }
    }
    out
}

/// ペイン 1 つぶんの自動復帰の追跡状態（1 エピソード = 上限で止まってから復帰するまで）
#[derive(Debug, Clone)]
pub(crate) struct LimitResumeTracker {
    /// このエピソードで最初に上限停止を観測した時刻（unix 秒）
    pub(crate) first_seen: i64,
    /// エピソード開始時に解決したリセット時刻（unix 秒）。
    /// **途中で更新しない**（画面に残った古いメッセージで復帰予定が後ろへずれないように）
    pub(crate) reset_at: Option<i64>,
    /// 停止の型（ダイアログ / idle）
    pub(crate) kind: LimitStopKind,
    /// 検知の根拠になった画面上の 1 行
    pub(crate) message: String,
    /// 直近の画面指紋と、それが変わらなくなった時刻
    fingerprint: u64,
    pub(crate) stable_since: Option<i64>,
    /// このエピソードでの試行回数と直近の試行時刻
    pub(crate) attempts: u32,
    pub(crate) last_attempt: Option<i64>,
    /// バックグラウンドの復帰動作が走っている（二重起動を防ぐ）
    pub(crate) in_flight: bool,
    /// 直近の判断・結果（`tako limit-resume` の state に出す）
    pub(crate) last_outcome: Option<String>,
}

/// バックグラウンドで実行する復帰動作（UI スレッドでは組み立てるだけ）
pub(crate) struct LimitResumeJob {
    pub(crate) pane: PaneId,
    pub(crate) backend_session: String,
    pub(crate) worker_id: String,
}

/// 監査ログの action 名（#749 の `ctx_handoff_nudge` と同じ場所・同じ形式）
const AUDIT_ACTION: &str = "limit_autoresume";

impl TakoApp {
    /// 自動復帰の駆動（2 秒 tick から呼ぶ）。
    /// 戻り値はバックグラウンドで実行すべきダイアログ応答ジョブ
    pub(crate) fn drive_limit_autoresume(&mut self) -> Vec<LimitResumeJob> {
        // 有効なペインの収集。既定 OFF なので通常運転ではここで終わる（NFR-8）
        let targets: Vec<PaneId> = self
            .workspace
            .tabs()
            .iter()
            .flat_map(|tab| tab.tree().panes())
            .filter(|p| p.limit_autoresume())
            .map(|p| p.id())
            .collect();
        if targets.is_empty() {
            if !self.limit_resume.is_empty() {
                self.limit_resume.clear();
            }
            return Vec::new();
        }
        // 無効化・消滅したペインの追跡は捨てる（ペイン ID 再利用での取り違え防止）
        self.limit_resume.retain(|id, _| targets.contains(id));

        let now = limit_resume::now_unix();
        let tz = limit_resume::local_utc_offset();
        let mut jobs = Vec::new();
        for pane in targets {
            let Some(session) = self.terminals.get(&pane) else {
                continue;
            };
            let lines = session.visible_lines();
            let fingerprint = tako_control::limit_stop::screen_fingerprint(&lines);
            // #985: codex は rollout の `rate_limits.resets_at`（epoch）で解除時刻が
            // **書式にもタイムゾーンにも依存せず**分かる。画面の文言パースより確かなので
            // 読めていればそちらを採る。**停止の根拠は画面のまま**（#813 の安全条件）
            let hint = self
                .codex_limits
                .get(&pane)
                .map(tako_control::limit_stop::LimitHint::from_codex);
            let stop =
                tako_control::limit_stop::detect_limit_stop_with(&lines, now, tz, hint.as_ref());
            let Some(stop) = stop else {
                // 上限ではなくなった = エピソード終了。次に止まったら 1 からやり直す
                if let Some(prev) = self.limit_resume.remove(&pane) {
                    if prev.attempts > 0 {
                        audit(
                            &worker_id_of(self, pane),
                            pane,
                            &format!(
                                "recovered: pane left the usage limit state after {} attempt(s)",
                                prev.attempts
                            ),
                        );
                    }
                }
                continue;
            };
            // 入力欄に人間の下書きがあるか。ダイアログ中は入力欄自体が無いので見ない
            let user_draft = matches!(stop.kind, LimitStopKind::Idle)
                && session.analyze_input().is_some_and(|s| {
                    matches!(
                        s.style,
                        tako_core::InputStyle::User | tako_core::InputStyle::Mixed
                    )
                });

            let tracker = self
                .limit_resume
                .entry(pane)
                .or_insert_with(|| LimitResumeTracker::new(&stop, now, fingerprint));
            tracker.observe(&stop, now, fingerprint);
            if tracker.in_flight {
                continue; // バックグラウンドの応答が走っている
            }
            // 判断に渡すのは**追跡状態**（エピソード開始時に確定したリセット時刻）で、
            // 毎 tick パースし直した `stop` ではない。画面には古い上限メッセージが
            // 残るので、都度パースを渡すと復帰予定が後ろへずれ続ける
            let effective = tracker.effective_stop();
            let decision = limit_resume::decide(&ResumeInput {
                enabled: true,
                stop: Some(&effective),
                now,
                first_seen: tracker.first_seen,
                stable_since: tracker.stable_since,
                attempts: tracker.attempts,
                last_attempt: tracker.last_attempt,
                user_draft,
            });
            let Some(action) = decision.action() else {
                tracker.note_hold(decision);
                continue;
            };
            let worker_id = worker_id_of(self, pane);
            match action {
                ResumeAction::Nudge => {
                    let prompt = limit_resume::nudge_prompt();
                    self.queue_prompt_flow(pane, prompt);
                    let tracker = self.limit_resume.get_mut(&pane).expect("直前に挿入済み");
                    tracker.note_attempt(now, "nudge queued");
                    audit(
                        &worker_id,
                        pane,
                        &format!(
                            "nudge queued (attempt {}, reset_at={})",
                            tracker.attempts,
                            reset_display(tracker.reset_at)
                        ),
                    );
                }
                ResumeAction::RespondDialog => {
                    // ダイアログへの応答はキー送出（数百 ms のスリープ込み）なので
                    // バックグラウンドへ回す。到達手段が無ければ触らずに記録だけ残す
                    let Some(backend_session) = TmuxHost::backend_session(self, pane) else {
                        let tracker = self.limit_resume.get_mut(&pane).expect("直前に挿入済み");
                        tracker.note_attempt(now, "no backend session (cannot respond)");
                        audit(
                            &worker_id,
                            pane,
                            "skipped: ペインに永続バックエンドが無く、ダイアログへ応答できない",
                        );
                        continue;
                    };
                    let tracker = self.limit_resume.get_mut(&pane).expect("直前に挿入済み");
                    tracker.note_attempt(now, "responding to limit dialog");
                    tracker.in_flight = true;
                    audit(
                        &worker_id,
                        pane,
                        &format!(
                            "responding to limit dialog (attempt {}, reset_at={})",
                            tracker.attempts,
                            reset_display(tracker.reset_at)
                        ),
                    );
                    jobs.push(LimitResumeJob {
                        pane,
                        backend_session,
                        worker_id,
                    });
                }
            }
        }
        jobs
    }

    /// バックグラウンドのダイアログ応答が終わったときの後始末（結果の記録）
    pub(crate) fn apply_limit_resume_result(&mut self, pane: PaneId, outcome: String) {
        if let Some(tracker) = self.limit_resume.get_mut(&pane) {
            tracker.in_flight = false;
            tracker.last_outcome = Some(outcome);
        }
    }

    /// `tako limit-resume` / `read` / `worker_status` に載せる実行状態（#813）
    pub(crate) fn limit_resume_state_json(&self, pane: PaneId) -> Option<serde_json::Value> {
        let t = self.limit_resume.get(&pane)?;
        Some(serde_json::json!({
            "stopped_by_limit": true,
            "kind": t.kind.as_str(),
            "message": t.message,
            "reset_at": t.reset_at,
            "resume_at": t.due_at(),
            "attempts": t.attempts,
            "max_attempts": limit_resume::MAX_ATTEMPTS,
            "last_attempt": t.last_attempt,
            "in_flight": t.in_flight,
            "last_outcome": t.last_outcome,
        }))
    }
}

impl LimitResumeTracker {
    fn new(stop: &LimitStop, now: i64, fingerprint: u64) -> Self {
        Self {
            first_seen: now,
            reset_at: stop.reset_at,
            kind: stop.kind,
            message: stop.message.clone(),
            fingerprint,
            stable_since: Some(now),
            attempts: 0,
            last_attempt: None,
            in_flight: false,
            last_outcome: None,
        }
    }

    /// 新しい観測を取り込む。`reset_at` はエピソード中に**動かさない**
    /// （画面に残った古い上限メッセージで復帰予定が後ろへずれるのを防ぐ）。
    /// ただし最初に解決できていなかった場合だけは、後から読めたものを採る
    fn observe(&mut self, stop: &LimitStop, now: i64, fingerprint: u64) {
        self.kind = stop.kind;
        self.message = stop.message.clone();
        if self.reset_at.is_none() {
            self.reset_at = stop.reset_at;
        }
        if fingerprint == self.fingerprint {
            if self.stable_since.is_none() {
                self.stable_since = Some(now);
            }
        } else {
            self.fingerprint = fingerprint;
            self.stable_since = Some(now);
        }
    }

    /// 観測時刻を `secs` 秒さかのぼらせ、リセット時刻を差し替える。
    ///
    /// セルフテスト（項目 111）が「リセット時刻を過ぎた状態」を作るための注入口。
    /// #749 が `HandoffNudgeTracker::new(old)` で猶予をさかのぼらせるのと同じ流儀で、
    /// **判断そのもの（`tako_core::limit_resume::decide`）には手を入れない**。
    /// 製品コードからは呼ばれないので release の挙動は変わらない
    pub(crate) fn backdate_for_test(&mut self, secs: i64, reset_at: Option<i64>) {
        self.first_seen -= secs;
        self.stable_since = self.stable_since.map(|t| t - secs);
        self.reset_at = reset_at;
    }

    fn note_attempt(&mut self, now: i64, outcome: &str) {
        self.attempts += 1;
        self.last_attempt = Some(now);
        self.last_outcome = Some(outcome.to_string());
    }

    fn note_hold(&mut self, decision: ResumeDecision) {
        if let ResumeDecision::Hold {
            reason,
            remaining_secs,
        } = decision
        {
            // 待ちの理由は状態照会で見えれば十分（毎 tick 監査ログへ書かない）
            self.last_outcome = Some(match reason {
                HoldReason::WaitingForReset | HoldReason::RetryBackoff => {
                    format!("{}: あと {remaining_secs} 秒", reason.as_str())
                }
                other => other.as_str().to_string(),
            });
        }
    }

    /// 判断に渡す停止情報。**リセット時刻は追跡状態のもの**（エピソード開始時に確定）で、
    /// 画面から毎 tick パースし直した値ではない
    pub(crate) fn effective_stop(&self) -> LimitStop {
        LimitStop {
            kind: self.kind,
            message: self.message.clone(),
            reset_at: self.reset_at,
        }
    }

    /// このエピソードで復帰を試みてよくなる時刻（unix 秒）
    fn due_at(&self) -> i64 {
        limit_resume::due_at(&self.effective_stop(), self.first_seen)
    }
}

/// バックグラウンドで実行する本体（UI スレッドから呼ばないこと）。
///
/// 1. ダイアログの構造を**下見**する（`choice` 省略 = 送信しない）
/// 2. 安全な選択肢をラベルで選ぶ（無ければ何も送らずに諦める）
/// 3. そのラベルで応答する（送出経路・検証・persist.log の監査は `respond` と同じ）
pub(crate) fn run_limit_resume_job(job: &LimitResumeJob) -> String {
    use tako_control::dispatch::respond_to_choice_dialog;

    let caller = Some("limit-autoresume");
    let probe =
        match respond_to_choice_dialog(&job.backend_session, job.pane.as_u64(), None, caller) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("dialog probe failed: {e}");
                audit(&job.worker_id, job.pane, &msg);
                return msg;
            }
        };
    // 下見の時点で種別が usage_limit でなければ触らない（画面が入れ替わった）
    if probe.get("kind").and_then(|k| k.as_str()) != Some("usage_limit") {
        let msg = format!(
            "aborted: ダイアログが usage_limit ではなくなっている（kind={}）",
            probe
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("unknown")
        );
        audit(&job.worker_id, job.pane, &msg);
        return msg;
    }
    let options: Vec<(Option<u32>, String)> = probe
        .get("options")
        .and_then(|o| o.as_array())
        .map(|a| {
            a.iter()
                .map(|o| {
                    (
                        o.get("number").and_then(|n| n.as_u64()).map(|n| n as u32),
                        o.get("label")
                            .and_then(|l| l.as_str())
                            .unwrap_or("")
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let Some((_, label)) = limit_resume::safe_choice(&options) else {
        let msg = format!(
            "aborted: 安全に選べる選択肢が無い（{}）",
            options
                .iter()
                .map(|(_, l)| l.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        );
        audit(&job.worker_id, job.pane, &msg);
        return msg;
    };
    // 番号ではなくラベルで応答する（選択肢の並びが版で変わっても課金系を掴まない）
    match respond_to_choice_dialog(&job.backend_session, job.pane.as_u64(), Some(label), caller) {
        Ok(v) => {
            let resolved = v.get("resolved").and_then(|r| r.as_bool()).unwrap_or(false);
            let msg = format!("responded: 「{label}」 resolved={resolved}");
            audit(&job.worker_id, job.pane, &msg);
            msg
        }
        Err(e) => {
            let msg = format!("respond failed: {e}");
            audit(&job.worker_id, job.pane, &msg);
            msg
        }
    }
}

/// 監査記録（`<data_dir>/supervisor.log`。#749 の ctx_handoff_nudge と同じ場所）
fn audit(worker_id: &str, pane: PaneId, detail: &str) {
    tako_control::orchestrator::supervisor::audit_log(
        worker_id,
        pane.as_u64(),
        AUDIT_ACTION,
        detail,
    );
}

/// 監査ログに出す識別子（role があればそれ、無ければペイン ID）
fn worker_id_of(app: &TakoApp, pane: PaneId) -> String {
    app.workspace
        .tabs()
        .iter()
        .flat_map(|t| t.tree().panes())
        .find(|p| p.id() == pane)
        .and_then(|p| p.role().map(str::to_string))
        .unwrap_or_else(|| format!("pane:{}", pane.as_u64()))
}

fn reset_display(reset_at: Option<i64>) -> String {
    reset_at.map(|v| v.to_string()).unwrap_or("unknown".into())
}

/// 追跡状態の入れ物（TakoApp のフィールド型）
pub(crate) type LimitResumeTrackers = HashMap<PaneId, LimitResumeTracker>;

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(kind: LimitStopKind, reset_at: Option<i64>) -> LimitStop {
        LimitStop {
            kind,
            message: "Your limit will reset at 3am.".into(),
            reset_at,
        }
    }

    #[test]
    fn issue813_リセット時刻はエピソード中に動かない() {
        let first = stop(LimitStopKind::Idle, Some(1_000));
        let mut t = LimitResumeTracker::new(&first, 0, 1);
        // 画面に別の（古い）時刻が出ても最初の解決を保つ
        t.observe(&stop(LimitStopKind::Idle, Some(90_000)), 10, 1);
        assert_eq!(t.reset_at, Some(1_000));
        // 最初に解決できていなければ後から読めたものを採る
        let mut t2 = LimitResumeTracker::new(&stop(LimitStopKind::Idle, None), 0, 1);
        t2.observe(&stop(LimitStopKind::Idle, Some(1_000)), 10, 1);
        assert_eq!(t2.reset_at, Some(1_000));
    }

    /// 判断へ渡すのは追跡状態であって、毎 tick の再パースではない。
    /// これを取り違えると「画面に残った古い上限メッセージ」で復帰予定が
    /// 後ろへずれ続け、リセット時刻を過ぎても永遠に発動しない（実測で踏んだ）
    #[test]
    fn issue813_判断は追跡状態のリセット時刻を使う() {
        let first = stop(LimitStopKind::Idle, Some(1_000));
        let mut t = LimitResumeTracker::new(&first, 0, 1);
        // 画面からは「まだ先のリセット時刻」が読めても、
        t.observe(&stop(LimitStopKind::Idle, Some(90_000)), 10, 1);
        // 判断に渡す停止情報は最初に確定した値のまま
        assert_eq!(t.effective_stop().reset_at, Some(1_000));
        assert_eq!(t.due_at(), 1_000 + limit_resume::SAFETY_MARGIN_SECS);
        // 実際に発動できる（画面由来の 90_000 を使っていたら待ち続ける）
        let decision = limit_resume::decide(&ResumeInput {
            enabled: true,
            stop: Some(&t.effective_stop()),
            now: 2_000,
            first_seen: t.first_seen,
            stable_since: t.stable_since,
            attempts: 0,
            last_attempt: None,
            user_draft: false,
        });
        assert_eq!(decision.action(), Some(ResumeAction::Nudge));
    }

    #[test]
    fn issue813_画面が変わると静止時刻がやり直しになる() {
        let s = stop(LimitStopKind::Dialog, Some(0));
        let mut t = LimitResumeTracker::new(&s, 100, 1);
        assert_eq!(t.stable_since, Some(100));
        t.observe(&s, 110, 1);
        assert_eq!(t.stable_since, Some(100), "同じ画面なら静止時刻は動かない");
        t.observe(&s, 120, 2);
        assert_eq!(t.stable_since, Some(120), "画面が変わったらやり直し");
    }

    #[test]
    fn issue813_試行の記録が状態に残る() {
        let mut t = LimitResumeTracker::new(&stop(LimitStopKind::Idle, Some(1_000)), 0, 1);
        assert_eq!(t.attempts, 0);
        t.note_attempt(2_000, "nudge queued");
        assert_eq!(t.attempts, 1);
        assert_eq!(t.last_attempt, Some(2_000));
        assert_eq!(t.last_outcome.as_deref(), Some("nudge queued"));
        // 復帰予定はリセット時刻 + 安全マージン
        assert_eq!(t.due_at(), 1_000 + limit_resume::SAFETY_MARGIN_SECS);
    }

    #[test]
    fn issue813_待ちの理由は残り秒つきで状態に出る() {
        let mut t = LimitResumeTracker::new(&stop(LimitStopKind::Idle, Some(1_000)), 0, 1);
        t.note_hold(ResumeDecision::Hold {
            reason: HoldReason::WaitingForReset,
            remaining_secs: 42,
        });
        assert_eq!(
            t.last_outcome.as_deref(),
            Some("waiting_for_reset: あと 42 秒")
        );
        t.note_hold(ResumeDecision::Hold {
            reason: HoldReason::UserDraft,
            remaining_secs: 0,
        });
        assert_eq!(t.last_outcome.as_deref(), Some("user_draft"));
    }
}
