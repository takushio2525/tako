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
//! - ダイアログ型 = キー送出を伴うので**バックグラウンド**で `respond_via` を通す
//!   （1 回あたり数百 ms のスリープが入るので UI スレッドでは走らせない）。
//!   届き方（in-process / 器越し）は **UI スレッドで**決めて手を持たせる（#1200。
//!   `TerminalSession` はスレッドを越えられないが `PaneAccess` は越えられる）

use std::collections::HashMap;

use tako_control::host::{SessionHost, TmuxHost};
use tako_control::limit_stop::ObservedLimit;
use tako_core::limit_resume::{
    self, HoldReason, LimitStop, LimitStopKind, ResumeAction, ResumeDecision, ResumeInput,
};
use tako_core::PaneId;

use crate::tick_screens::{TickScreens, TickSource};
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
    /// このエピソードで最初に上限停止を観測した時刻（unix 秒）。
    /// GUI の再起動・追跡の作り直しをまたいで**告知を最初に見た時刻**を引き継ぐ
    /// （ペインの記憶 `Pane::limit_seen`。#1710）
    pub(crate) first_seen: i64,
    /// この GUI がこのエピソードの追跡を（再）開始した時刻（unix 秒。#1710）。
    /// 追跡を始めた時点でもう解けていた上限は、ここ + 安全マージンで再開する
    pub(crate) tracked_since: i64,
    /// エピソード開始時に解決したリセット時刻（unix 秒）。告知を最初に見た時刻から解く（#1710）。
    /// **途中で更新しない**（画面に残った古いメッセージで復帰予定が後ろへずれないように）。
    /// 例外は構造化ソース（codex の rollout / tako mod）の時刻が後から届いたとき（#1710）
    pub(crate) reset_at: Option<i64>,
    /// `reset_at` が構造化ソースの時刻か（#1710。画面から解いた値は後から届いた構造化ソースの
    /// 時刻で置き換える = GUI の再起動直後は mod の報告がまだ届いていない）
    reset_from_hint: bool,
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
    pub(crate) worker_id: String,
    /// tako-app が保持しているペインへの手（#1200）。**UI スレッドで取り出しておく**
    /// （`TerminalSession` はスレッドを越えられないが、この手は越えられる）。
    /// 器が入力送出を持たない環境（psmux = Windows）ではこれが唯一の送り口
    pub(crate) access: Option<tako_core::PaneAccess>,
    /// 器越しの手（GUI が保持していないペイン。`access` が無いときの経路）
    pub(crate) backend_session: Option<String>,
}

/// 監査ログの action 名（#749 の `ctx_handoff_nudge` と同じ場所・同じ形式）
const AUDIT_ACTION: &str = "limit_autoresume";

impl TakoApp {
    /// 自動復帰の対象（有効なペイン）。2 秒 tick の段と、背景で画面を採る計画
    /// （#2647 の `tick_screens::plan`）が同じ 1 実装を引く。
    /// #1945: 退避中のペインも対象（一括で ON にした退避中の master が上限で止まったら
    /// 戻す。端末を持たない退避中 = 再起動後まだ表へ出していないペインは段の中で飛ばす）
    pub(crate) fn limit_resume_targets(&self) -> Vec<PaneId> {
        if tako_core::limit_resume_all::legacy() {
            self.workspace
                .tabs()
                .iter()
                .flat_map(|tab| tab.tree().panes())
                .filter(|p| p.limit_autoresume())
                .map(|p| p.id())
                .collect()
        } else {
            self.workspace
                .all_panes()
                .into_iter()
                .filter(|p| p.limit_autoresume())
                .map(|p| p.id())
                .collect()
        }
    }

    /// 自動復帰の駆動（2 秒 tick から呼ぶ）。
    /// 戻り値はバックグラウンドで実行すべきダイアログ応答ジョブ。
    /// 画面は `screens` から読む（#2647: 定期更新は背景で採った 1 枚を渡す = 画面の組み立てと
    /// 上限の判定を UI スレッドで行わない。定期更新の外は `TickScreens::Live` = その場で読む）
    pub(crate) fn drive_limit_autoresume(&mut self, screens: &TickScreens) -> Vec<LimitResumeJob> {
        // 有効なペインの収集。既定 OFF なので通常運転ではここで終わる（NFR-8）
        let targets = self.limit_resume_targets();
        if targets.is_empty() {
            if !self.limit_resume.is_empty() {
                self.limit_resume.clear();
            }
            if !self.limit_seen_absent.is_empty() {
                self.limit_seen_absent.clear();
            }
            return Vec::new();
        }
        // 無効化・消滅したペインの追跡は捨てる（ペイン ID 再利用での取り違え防止）
        self.limit_resume.retain(|id, _| targets.contains(id));
        self.limit_seen_absent.retain(|id, _| targets.contains(id));

        let now = limit_resume::now_unix();
        let tz = limit_resume::local_utc_offset();
        let seen_legacy = limit_resume::legacy_detected_base();
        let mod_now = std::time::Instant::now();
        let mod_legacy = tako_core::claude_mod::s2_legacy();
        let mut jobs = Vec::new();
        for pane in targets {
            let Some(session) = screens.source(&self.terminals, pane) else {
                continue;
            };
            let crate::tick_screens::LimitRead {
                lines,
                fingerprint,
                classified,
            } = session.limit_read();
            // #985: codex は rollout の `rate_limits.resets_at`（epoch）で解除時刻が
            // **書式にもタイムゾーンにも依存せず**分かる。画面の文言パースより確かなので
            // 読めていればそちらを採る。**停止の根拠は画面のまま**（#813 の安全条件）。
            // #1880: claude は tako mod の使用制限（アカウント単位で束ねた値。秒精度）
            let hint = self
                .codex_limits
                .get(&pane)
                .map(tako_control::limit_stop::LimitHint::from_codex)
                .or_else(|| {
                    if mod_legacy {
                        return None;
                    }
                    let limits = self.claude_mod.account_rate_limits(pane.as_u64(), mod_now);
                    (!limits.is_empty())
                        .then(|| tako_control::limit_stop::LimitHint::from_mod(&limits))
                });
            // #1710: 解除時刻は**告知を最初に見た時刻**から解く。記憶はペインの属性
            // （layout.json に載る）なので GUI の再起動・追跡の作り直しをまたぐ
            let seen = if seen_legacy {
                None
            } else {
                self.workspace
                    .pane_anywhere(pane)
                    .and_then(|p| p.limit_seen())
                    .cloned()
            };
            let observed = tako_control::limit_stop::observe_classified(
                &lines,
                classified,
                seen.as_ref(),
                now,
                tz,
                hint.as_ref(),
            );
            if !seen_legacy {
                // `session`（self.terminals の借用）が生きているのでフィールドを分けて渡す
                sync_limit_seen(
                    &mut self.workspace,
                    &mut self.limit_seen_absent,
                    pane,
                    seen.as_ref(),
                    observed.as_ref(),
                    now,
                );
            }
            let Some(observed) = observed else {
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
            let stop = &observed.stop;
            // 入力欄に人間の下書きがあるか。ダイアログ中は入力欄自体が無いので見ない
            let user_draft = matches!(stop.kind, LimitStopKind::Idle)
                && session
                    .analyze_input()
                    .is_some_and(|s| s.style.is_user_draft());

            let tracker = self
                .limit_resume
                .entry(pane)
                .or_insert_with(|| LimitResumeTracker::new(&observed, now, fingerprint));
            tracker.observe(&observed, now, fingerprint);
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
                tracked_since: tracker.tracked_since,
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
                    // バックグラウンドへ回す。到達手段が無ければ触らずに記録だけ残す。
                    // #1200: **in-process を先に見る**（器が入力送出を持たない環境では
                    // 器越しの手が無いので、ここで諦めると Windows で一生応答できない）
                    let access = SessionHost::session(self, pane).map(|s| s.access());
                    let backend_session = TmuxHost::backend_session(self, pane);
                    if access.is_none() && backend_session.is_none() {
                        let tracker = self.limit_resume.get_mut(&pane).expect("直前に挿入済み");
                        tracker.note_attempt(now, "no route (cannot respond)");
                        audit(
                            &worker_id,
                            pane,
                            "skipped: ペインへ届く手が無く、ダイアログへ応答できない",
                        );
                        continue;
                    }
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
                        worker_id,
                        access,
                        backend_session,
                    });
                }
            }
        }
        jobs
    }

    /// ペインで手起動のエージェント CLI が動いていると検出済みか（#1945）。
    /// 材料は会話の検出（claude = `claude agents --json` / codex・agy = プロセス表。
    /// #1076 / #1238 の保持規則つき）で、ここは引くだけ = 新しい走査を増やさない
    pub(crate) fn limit_resume_agent_detected(&self, pane: PaneId) -> bool {
        self.claude_resume_sessions.get(pane).is_some()
            || self.agent_resume_sessions.get(pane).is_some()
    }

    /// 検出済みのペインの集合（`limit_resume_all` の判断へ渡す。workspace を可変で
    /// 借りる前に作っておく）
    fn limit_resume_detected(&self) -> std::collections::HashSet<PaneId> {
        self.workspace
            .all_panes()
            .into_iter()
            .map(|p| p.id())
            .filter(|id| self.limit_resume_agent_detected(*id))
            .collect()
    }

    /// 全体の既定（#1945）を書き換えて settings.json へ残す。一括ボタン・CLI・MCP の
    /// どれも dispatch（`limit_resume_bulk`）からここへ来る
    pub(crate) fn set_limit_resume_all(&mut self, enabled: bool) {
        self.limit_resume_all = enabled;
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.limit_resume_all = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                // 次の起動で既定が戻ってしまうので黙らない（GUI の stderr は誰も読めない = #1399）
                self.notify_ui_op_failed(
                    crate::sidebar::NoticeArea::StatusBar,
                    crate::sidebar::NoticeArm::Issue1945,
                    crate::ui_text::pane_menu::limit_resume_all_save(),
                    None,
                    &e.to_string(),
                );
            }
        }
    }

    /// ステータスバーの一括ボタンの集計を数え直す。変わったら true（呼び出し側が再描画する）。
    ///
    /// 呼ぶのは 2 秒 tick と切り替えの直後だけ（**描画のたびには数えない**）
    pub(crate) fn refresh_limit_resume_summary(&mut self) -> bool {
        let detected = self.limit_resume_detected();
        let next =
            tako_core::limit_resume_all::summarize(&self.workspace, self.limit_resume_all, |id| {
                detected.contains(&id)
            });
        if next == self.limit_resume_summary {
            return false;
        }
        self.limit_resume_summary = next;
        true
    }

    /// 以後にエージェントになったペインへ全体の既定を当てる（#1945）。
    /// 会話の検出が更新された直後に呼ぶ（手起動の claude / codex はここでしか分からない）。
    /// 値を変えたら layout.json の保存が要るので true を返す
    pub(crate) fn adopt_limit_resume_default(&mut self) -> bool {
        if tako_core::limit_resume_all::legacy() || !self.limit_resume_all {
            return false;
        }
        let detected = self.limit_resume_detected();
        let adopted = tako_core::limit_resume_all::adopt_default(
            &mut self.workspace,
            self.limit_resume_all,
            |id| detected.contains(&id),
        );
        for pane in &adopted {
            tako_control::diag::persist_log(&format!(
                "[limit-autoresume] pane={} enabled=true 発生源 all:detected",
                pane.as_u64()
            ));
        }
        !adopted.is_empty()
    }

    /// 復元したエージェントのペインを「決定済み」にする（#1945）。
    /// 保存されていた値は人の選択なので、再起動で全体の既定へ戻さない
    pub(crate) fn mark_restored_limit_resume_decided(&mut self) {
        let detected = self.limit_resume_detected();
        tako_core::limit_resume_all::mark_restored_agents_decided(&mut self.workspace, |id| {
            detected.contains(&id)
        });
    }

    /// ステータスバーの一括ボタン（#1945）。押した向きは集計が決め
    /// （全部 ON なら OFF、それ以外は ON）、CLI / MCP と同じ dispatch を通る
    pub(crate) fn toggle_limit_resume_all(&mut self) {
        self.refresh_limit_resume_summary();
        let next = self.limit_resume_summary.next_enabled();
        if let Err(e) = tako_control::dispatch(
            self,
            tako_control::protocol::Request::LimitResume {
                pane: None,
                enabled: Some(next),
                all: Some(true),
                network: None,
            },
            tako_core::PaneOrigin::User,
        ) {
            // 押した操作の失敗は通知欄へ（#1399 の「弾いたら黙って捨てない」）
            self.notify_ui_dispatch_failed(
                crate::sidebar::NoticeArea::StatusBar,
                crate::sidebar::NoticeArm::Issue1945,
                crate::ui_text::pane_menu::limit_resume_all_toggle_op(next),
                None,
                &e,
            );
        }
        // 属性は layout.json へ載るので、ここで保存して再起動に備える
        self.save_layout();
        self.refresh_limit_resume_summary();
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
            "detected_at": t.first_seen,
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
    fn new(observed: &ObservedLimit, now: i64, fingerprint: u64) -> Self {
        let stop = &observed.stop;
        Self {
            // #1710: 告知を最初に見た時刻（記憶が無ければ now = これまでどおり）
            first_seen: observed.seen.detected_at,
            tracked_since: now,
            reset_at: stop.reset_at,
            reset_from_hint: observed.reset_from_hint,
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
    /// ただし最初に解決できていなかった場合だけは、後から読めたものを採る。
    /// 画面から解いた値は、後から届いた構造化ソースの時刻で置き換える（#1710。
    /// GUI の再起動直後の数 tick は mod の報告がまだ無く、画面の値で追跡が始まる）
    fn observe(&mut self, observed: &ObservedLimit, now: i64, fingerprint: u64) {
        let stop = &observed.stop;
        self.kind = stop.kind;
        self.message = stop.message.clone();
        if self.reset_at.is_none() {
            self.reset_at = stop.reset_at;
            self.reset_from_hint = observed.reset_from_hint;
        } else if observed.reset_from_hint
            && !self.reset_from_hint
            && !limit_resume::legacy_detected_base()
        {
            self.reset_at = stop.reset_at;
            self.reset_from_hint = true;
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
        // #1710: 追跡の開始もさかのぼらせる（リセット済みの上限は追跡開始 + マージンで動く）
        self.tracked_since -= secs;
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

    /// このエピソードで復帰を試みてよくなる時刻（unix 秒）。
    /// 追跡を始めた時点でもう解けていたら追跡の開始 + マージン（#1710）
    fn due_at(&self) -> i64 {
        limit_resume::resume_due(&self.effective_stop(), self.first_seen, self.tracked_since)
    }
}

/// バックグラウンドで実行する本体（UI スレッドから呼ばないこと）。
///
/// 1. ダイアログの構造を**下見**する（`choice` 省略 = 送信しない）
/// 2. 安全な選択肢をラベルで選ぶ（無ければ何も送らずに諦める）
/// 3. そのラベルで応答する（送出経路・検証・persist.log の監査は `respond` と同じ）
pub(crate) fn run_limit_resume_job(job: &LimitResumeJob) -> String {
    let caller = Some("limit-autoresume");
    // #1200: 届き方は in-process（UI スレッドで取り出した手）を先に使う。
    // 器が入力送出を持たない環境（psmux = Windows）では器越しの手が存在しない
    let access: Box<dyn tako_control::reach::DialogAccess> = match &job.access {
        Some(access) => Box::new(tako_control::reach::LiveDialogAccess::new(access.clone())),
        None => {
            let hint = job.backend_session.as_deref().unwrap_or_default();
            match tako_control::reach::detached_session(hint) {
                Some((session, detached)) => Box::new(
                    tako_control::reach::DetachedDialogAccess::new(session, detached),
                ),
                None => {
                    let msg = format!(
                        "dialog probe failed: {}",
                        tako_control::reach::no_detached_access_note()
                    );
                    audit(&job.worker_id, job.pane, &msg);
                    return msg;
                }
            }
        }
    };
    let respond = |choice: Option<&str>| {
        tako_control::dispatch::respond_via(access.as_ref(), job.pane.as_u64(), choice, caller)
    };
    let probe = match respond(None) {
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
    match respond(Some(label)) {
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

/// 告知の記憶（#1710）をペインへ書き戻す。規則は `limit_resume::step_seen` の 1 実装
/// （消えたまま 10 分続いたら捨てる）。書き換えはペインの属性なので、同じ tick の
/// `save_layout` が layout.json へ落とす
fn sync_limit_seen(
    workspace: &mut tako_core::Workspace,
    absent: &mut LimitSeenAbsence,
    pane: PaneId,
    current: Option<&limit_resume::LimitSeen>,
    observed: Option<&ObservedLimit>,
    now: i64,
) {
    let step = limit_resume::step_seen(
        current,
        observed.map(|o| &o.seen),
        absent.get(&pane).copied(),
        now,
    );
    match step.absent_since {
        Some(since) => {
            absent.insert(pane, since);
        }
        None => {
            absent.remove(&pane);
        }
    }
    if let Some(write) = step.write {
        if let Some(p) = workspace.pane_anywhere_mut(pane) {
            p.set_limit_seen(write);
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

/// 告知が画面から消えたのを最初に見た時刻（#1710。TakoApp のフィールド型）。
/// ペインの記憶を捨てるまでの猶予を数えるためだけの非永続の状態
pub(crate) type LimitSeenAbsence = HashMap<PaneId, i64>;

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

    /// 画面から解いた観測（告知を最初に見た時刻 = `detected_at`）
    fn seen_at(kind: LimitStopKind, reset_at: Option<i64>, detected_at: i64) -> ObservedLimit {
        ObservedLimit {
            stop: stop(kind, reset_at),
            seen: limit_resume::LimitSeen {
                notice: "0000000000000001".into(),
                detected_at,
            },
            reset_from_hint: false,
        }
    }

    /// 構造化ソース（tako mod / codex の rollout）の時刻つきの観測
    fn hinted(kind: LimitStopKind, reset_at: i64, detected_at: i64) -> ObservedLimit {
        ObservedLimit {
            reset_from_hint: true,
            ..seen_at(kind, Some(reset_at), detected_at)
        }
    }

    fn input(t: &LimitResumeTracker, stop: &LimitStop, now: i64) -> ResumeDecision {
        limit_resume::decide(&ResumeInput {
            enabled: true,
            stop: Some(stop),
            now,
            first_seen: t.first_seen,
            tracked_since: t.tracked_since,
            stable_since: t.stable_since,
            attempts: t.attempts,
            last_attempt: t.last_attempt,
            user_draft: false,
        })
    }

    #[test]
    fn issue813_リセット時刻はエピソード中に動かない() {
        let mut t = LimitResumeTracker::new(&seen_at(LimitStopKind::Idle, Some(1_000), 0), 0, 1);
        // 画面に別の（古い）時刻が出ても最初の解決を保つ
        t.observe(&seen_at(LimitStopKind::Idle, Some(90_000), 0), 10, 1);
        assert_eq!(t.reset_at, Some(1_000));
        // 最初に解決できていなければ後から読めたものを採る
        let mut t2 = LimitResumeTracker::new(&seen_at(LimitStopKind::Idle, None, 0), 0, 1);
        t2.observe(&seen_at(LimitStopKind::Idle, Some(1_000), 0), 10, 1);
        assert_eq!(t2.reset_at, Some(1_000));
    }

    /// 判断へ渡すのは追跡状態であって、毎 tick の再パースではない。
    /// これを取り違えると「画面に残った古い上限メッセージ」で復帰予定が
    /// 後ろへずれ続け、リセット時刻を過ぎても永遠に発動しない（実測で踏んだ）
    #[test]
    fn issue813_判断は追跡状態のリセット時刻を使う() {
        let mut t = LimitResumeTracker::new(&seen_at(LimitStopKind::Idle, Some(1_000), 0), 0, 1);
        // 画面からは「まだ先のリセット時刻」が読めても、
        t.observe(&seen_at(LimitStopKind::Idle, Some(90_000), 0), 10, 1);
        // 判断に渡す停止情報は最初に確定した値のまま
        assert_eq!(t.effective_stop().reset_at, Some(1_000));
        assert_eq!(t.due_at(), 1_000 + limit_resume::SAFETY_MARGIN_SECS);
        // 実際に発動できる（画面由来の 90_000 を使っていたら待ち続ける）
        let decision = input(&t, &t.effective_stop(), 2_000);
        assert_eq!(decision.action(), Some(ResumeAction::Nudge));
    }

    #[test]
    fn issue813_画面が変わると静止時刻がやり直しになる() {
        let s = seen_at(LimitStopKind::Dialog, Some(0), 100);
        let mut t = LimitResumeTracker::new(&s, 100, 1);
        assert_eq!(t.stable_since, Some(100));
        t.observe(&s, 110, 1);
        assert_eq!(t.stable_since, Some(100), "同じ画面なら静止時刻は動かない");
        t.observe(&s, 120, 2);
        assert_eq!(t.stable_since, Some(120), "画面が変わったらやり直し");
    }

    #[test]
    fn issue813_試行の記録が状態に残る() {
        let mut t = LimitResumeTracker::new(&seen_at(LimitStopKind::Idle, Some(1_000), 0), 0, 1);
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
        let mut t = LimitResumeTracker::new(&seen_at(LimitStopKind::Idle, Some(1_000), 0), 0, 1);
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

    // --- #1710: 告知の検知時刻と追跡のやり直し ---

    /// 2026-09-25 07:00 JST（Issue の実例の解除時刻）
    const RESET: i64 = 1_790_287_200;
    /// 9/24 12:31 JST（上限に当たった = 告知を最初に見た）
    const DETECTED: i64 = RESET - (18 * 3600 + 29 * 60);
    /// 9/25 12:43 JST（GUI を再起動して追跡を作り直した）
    const RESTART: i64 = RESET + 5 * 3600 + 43 * 60;

    /// GUI の再起動で作り直した追跡は、告知の検知時刻（記憶）を引き継ぎ、
    /// リセット済みなら「作り直した時刻 + マージン」に再開する
    #[test]
    fn issue1710_作り直した追跡はリセット済みなら今からマージン後に動く() {
        // 記憶の時刻から解いた解除時刻 = 9/25 07:00（もう過ぎている）
        let t = LimitResumeTracker::new(
            &seen_at(LimitStopKind::Idle, Some(RESET), DETECTED),
            RESTART,
            1,
        );
        assert_eq!(t.first_seen, DETECTED, "告知の検知時刻を引き継ぐ");
        assert_eq!(t.tracked_since, RESTART);
        assert_eq!(
            t.due_at(),
            RESTART + limit_resume::SAFETY_MARGIN_SECS,
            "過去の解除時刻ではなく、作り直した時刻 + マージン"
        );
        // 画面の静止（10 秒）を待ってもまだマージンの中
        assert_eq!(
            input(&t, &t.effective_stop(), RESTART + limit_resume::STABLE_SECS),
            ResumeDecision::Hold {
                reason: HoldReason::WaitingForReset,
                remaining_secs: limit_resume::SAFETY_MARGIN_SECS - limit_resume::STABLE_SECS,
            }
        );
        // マージンを過ぎたら発動する（翌日まで待たない）
        assert_eq!(
            input(
                &t,
                &t.effective_stop(),
                RESTART + limit_resume::SAFETY_MARGIN_SECS
            )
            .action(),
            Some(ResumeAction::Nudge)
        );
    }

    /// GUI の再起動直後は mod の報告がまだ届いておらず、画面の値で追跡が始まる。
    /// 後から届いた構造化ソースの時刻（絶対時刻）は画面の値を置き換える
    #[test]
    fn issue1710_後から届いた構造化ソースの時刻は画面の値を置き換える() {
        // 記憶が無い（告知が GUI の停止中に出た）ので画面は「明日の 7 時」へ解ける
        let mut t = LimitResumeTracker::new(
            &seen_at(LimitStopKind::Idle, Some(RESET + 86_400), RESTART),
            RESTART,
            1,
        );
        assert_eq!(
            t.due_at(),
            RESET + 86_400 + limit_resume::SAFETY_MARGIN_SECS
        );
        // 15 秒後に mod の報告が届く（resets_at = 9/25 07:00 = もう過ぎている）
        t.observe(
            &hinted(LimitStopKind::Idle, RESET, RESTART),
            RESTART + 15,
            1,
        );
        assert_eq!(t.reset_at, Some(RESET), "構造化ソースの時刻を採る");
        assert_eq!(
            t.due_at(),
            RESTART + limit_resume::SAFETY_MARGIN_SECS,
            "リセット済みなので追跡の開始 + マージン"
        );
        // 一度採った構造化ソースの時刻は、画面の値で上書きしない
        t.observe(
            &seen_at(LimitStopKind::Idle, Some(RESET + 86_400), RESTART),
            RESTART + 17,
            1,
        );
        assert_eq!(t.reset_at, Some(RESET));
        // 構造化ソースどうしでも途中では動かさない（#813 の「エピソード中に動かない」）
        t.observe(
            &hinted(LimitStopKind::Idle, RESET + 3_600, RESTART),
            RESTART + 19,
            1,
        );
        assert_eq!(t.reset_at, Some(RESET));
    }

    /// 構造化ソースの時刻で始まった追跡は今までどおり（#985 / #1880 の回帰なし）
    #[test]
    fn issue1710_構造化ソースで始まった追跡は動かない() {
        let future = RESTART + 3_600;
        let mut t =
            LimitResumeTracker::new(&hinted(LimitStopKind::Idle, future, RESTART), RESTART, 1);
        t.observe(
            &seen_at(LimitStopKind::Idle, Some(RESET), RESTART),
            RESTART + 2,
            1,
        );
        assert_eq!(t.reset_at, Some(future));
        assert_eq!(t.due_at(), future + limit_resume::SAFETY_MARGIN_SECS);
    }

    /// セルフテスト（項目 111）の注入口は追跡の開始もさかのぼらせる。
    /// さかのぼらせないと「リセット済み」の状態でもマージンぶん待ってしまう
    #[test]
    fn issue1710_さかのぼりの注入口は追跡の開始も動かす() {
        let now = RESTART;
        let mut t = LimitResumeTracker::new(&seen_at(LimitStopKind::Idle, None, now), now, 1);
        t.backdate_for_test(3_600, Some(now - 600));
        assert_eq!(t.tracked_since, now - 3_600);
        assert!(
            t.due_at() <= now,
            "注入した状態で今すぐ動けない: due={} now={now}",
            t.due_at()
        );
    }
}
