//! spawn の起動保証ステートマシン（Issue #665）。
//!
//! 従来の spawn は「起動コマンドを `queue_write` で流し、プロンプトを
//! `queue_prompt_flow` で積む」だけの投げっぱなしで、どちらも**届いたかを誰も
//! 確認していなかった**。#640 では起動コマンドが 4/4 で失われ、worker が
//! 6 時間素の PowerShell のまま空回りしたのに、spawn の応答は成功だった。
//!
//! ここでは起動を段階に分け、各段階を画面から検証する:
//!
//! ```text
//! Queued → ShellReady → LaunchSent → AgentStarted → PromptSent → PromptDelivered
//!                           ↑____________|  検証できなければ再送（最大 max_attempts）
//! ```
//!
//! 重要な設計判断:
//! - **プロンプトは起動を確認してから積む**。従来は起動コマンドと同時に積んでおり、
//!   シェル初期化・raw mode 切替と競合していた（#390 の「貼り付けが吸われる」の温床）
//! - **再送はシェルプロンプトが見えているときだけ**。起動済みペインへ再送すると
//!   エージェントの入力欄へゴミが入る。判定は tako-control の純粋関数
//!   `classify_launch_screen`（GUI 非依存 = 単体テスト可能）に委ねる
//! - 段階はレジストリ（workers.yaml）へ書き、プロセス外から読めるようにする

use std::time::{Duration, Instant};

use tako_control::host::{LaunchAssuranceSpec, LaunchAssuranceStatus};
use tako_control::orchestrator::launch::{classify_launch_screen, LaunchPhase, LaunchScreen};
use tako_core::PaneId;

use super::*;

/// 起動コマンドを送ってからエージェント CLI の起動を待つ時間。
/// これを超えて確認できなければ再送する。claude / codex の実測起動時間
/// （数百 ms〜数秒）に対して十分な余裕を取りつつ、#640 の 6 時間空回りに
/// 比べれば桁違いに早く気づける値
const LAUNCH_PROBE: Duration = Duration::from_secs(12);

/// ペインのシェルが動き出すのを待つ上限。超えたら「出力なし」のまま先へ進み、
/// 起動コマンドを送ってみる（未知シェルで永久に止まらないため）
const SHELL_WAIT: Duration = Duration::from_secs(10);

/// 全体のタイムアウト。PromptFlow 側の 120 秒 + 再送 3 回ぶんの余裕
const TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// ペイン（PTY）そのものが生えるのを待つ上限。これを超えて `terminals` に
/// 現れないなら PTY 起動に失敗している（spawn 側が巻き戻し済み）
const PANE_WAIT: Duration = Duration::from_secs(20);

/// 状態機械の 1 エントリ
#[derive(Debug)]
pub(crate) struct LaunchWatch {
    pub(crate) pane: PaneId,
    spec: LaunchAssuranceSpec,
    phase: LaunchPhase,
    attempts: u32,
    created_at: Instant,
    /// 現在の段階に入った時刻（段階内タイムアウト用）
    phase_entered_at: Instant,
    detail: Option<String>,
    /// PromptFlow を積んだか（PromptSent 以降で二重に積まないため）
    prompt_queued: bool,
}

impl LaunchWatch {
    pub(crate) fn new(pane: PaneId, spec: LaunchAssuranceSpec) -> Self {
        let now = Instant::now();
        Self {
            pane,
            spec,
            phase: LaunchPhase::Queued,
            attempts: 0,
            created_at: now,
            phase_entered_at: now,
            detail: None,
            prompt_queued: false,
        }
    }

    fn enter(&mut self, phase: LaunchPhase, detail: Option<String>) {
        self.phase = phase;
        self.phase_entered_at = Instant::now();
        self.detail = detail;
        // 段階の永続記録。失敗しても状態機械は止めない（#390 の方針）
        if let Err(e) = tako_control::orchestrator::registry::record_launch_phase(
            &self.spec.worker_id,
            phase,
            self.attempts,
            self.detail.as_deref(),
        ) {
            eprintln!("warning: 起動保証の段階を記録できない: {e}");
        }
    }

    pub(crate) fn status(&self) -> LaunchAssuranceStatus {
        LaunchAssuranceStatus {
            phase: self.phase,
            attempts: self.attempts,
            elapsed_ms: self.created_at.elapsed().as_millis() as u64,
            detail: self.detail.clone(),
        }
    }

    fn is_done(&self) -> bool {
        self.phase.is_terminal()
    }
}

impl TakoApp {
    /// 起動保証の状態機械を 1 tick 進める（`drive_prompt_flows` と同じ 500ms ループ）
    pub(crate) fn drive_launch_assurance(&mut self) {
        let mut remaining = Vec::new();
        for mut watch in std::mem::take(&mut self.launch_watches) {
            self.step_launch_watch(&mut watch);
            if !watch.is_done() {
                remaining.push(watch);
            }
        }
        self.launch_watches = remaining;
    }

    fn step_launch_watch(&mut self, watch: &mut LaunchWatch) {
        if watch.created_at.elapsed() > TOTAL_TIMEOUT {
            let detail = format!(
                "起動保証が全体タイムアウト（{} 秒）。到達段階: {}",
                TOTAL_TIMEOUT.as_secs(),
                watch.phase.describe()
            );
            eprintln!("warning: {detail}（pane={}）", watch.pane.as_u64());
            watch.enter(LaunchPhase::Failed, Some(detail));
            return;
        }

        // 画面の観測（セッションがまだ生えていなければ Queued のまま待つ）
        let Some((has_output, screen)) = self.terminals.get(&watch.pane).map(|session| {
            let lines = session.visible_lines();
            let has_output = lines.iter().any(|l| !l.trim().is_empty());
            (has_output, classify_launch_screen(&lines))
        }) else {
            // 一度動き出したペインが消えた = 閉じられた / PTY が死んだ。
            // 待ち続けても意味が無いので明確に失敗させる（master が原因を読める）
            if watch.phase != LaunchPhase::Queued {
                watch.enter(
                    LaunchPhase::Failed,
                    Some(format!(
                        "ペインが消滅した（到達段階: {}）",
                        watch.phase.describe()
                    )),
                );
            } else if watch.created_at.elapsed() > PANE_WAIT {
                // PTY 起動に失敗した（spawn 側はエラーを返してペインも巻き戻している）。
                // 全体タイムアウトまで宙ぶらりんにせず、ここで畳む
                watch.enter(
                    LaunchPhase::Failed,
                    Some("ペインが起動しなかった（PTY の起動失敗）".to_string()),
                );
            }
            return;
        };
        // PromptFlow の完了判定はセッション借用と無関係なのでここで拾っておく
        let prompt_flow_active = self.prompt_flows.iter().any(|f| f.pane == watch.pane);
        // #640 の送達確認フローが動いている間は「まだ送っている最中」なので、
        // 起動が見えなくても再送しない（二重送信でシェルへ 2 行打ち込むのを防ぐ）
        let command_flow_active = self.command_flows.iter().any(|f| f.pane == watch.pane);

        // 観測 → 次の一手を決める（副作用はこの後まとめて適用する）
        let action = decide(
            watch,
            has_output,
            &screen,
            prompt_flow_active,
            command_flow_active,
        );
        self.apply_launch_action(watch, action, screen);
    }

    fn apply_launch_action(
        &mut self,
        watch: &mut LaunchWatch,
        action: LaunchAction,
        screen: LaunchScreen,
    ) {
        match action {
            LaunchAction::Wait => {}
            LaunchAction::ShellReady => watch.enter(LaunchPhase::ShellReady, None),
            LaunchAction::SendLaunch { cancel_line } => {
                if cancel_line {
                    // 直前の入力が中途半端に残っている可能性があるので
                    // Ctrl-C で行をキャンセルしてから送り直す
                    if let Some(session) = self.terminals.get(&watch.pane) {
                        session.write(vec![0x03]);
                    }
                }
                // 送達そのものは #640 の送達確認フローに任せる（シェルの準備待ち →
                // 本文 → エコー確認 → 分離 Enter → 実行確認）。ここは「届いた結果
                // エージェントが起動したか」だけを見る
                self.queue_command_flow(watch.pane, watch.spec.command.clone());
                watch.attempts += 1;
                if watch.attempts > 1 {
                    eprintln!(
                        "warning: {} の起動を確認できないため起動コマンドを再送する（pane={} {} 回目）",
                        watch.spec.agent,
                        watch.pane.as_u64(),
                        watch.attempts
                    );
                }
                watch.enter(
                    LaunchPhase::LaunchSent,
                    Some(format!("起動コマンドを送信（{} 回目）", watch.attempts)),
                );
            }
            LaunchAction::AgentStarted => {
                let detail = format!(
                    "{} の起動を画面で確認（{} 回目の送信）",
                    watch.spec.agent, watch.attempts
                );
                watch.enter(LaunchPhase::AgentStarted, Some(detail));
            }
            LaunchAction::SendPrompt => {
                self.queue_prompt_for(watch);
                watch.enter(LaunchPhase::PromptSent, None);
            }
            LaunchAction::Delivered => watch.enter(
                LaunchPhase::PromptDelivered,
                Some("プロンプトが入力欄から消えた（送達確認）".to_string()),
            ),
            LaunchAction::Fail { reason } => {
                let msg = match (&reason, &screen) {
                    (FailReason::LaunchError, LaunchScreen::LaunchError { detail }) => format!(
                        "起動コマンドが実行できない（{}）: {detail}",
                        watch.spec.agent
                    ),
                    (FailReason::LaunchError, _) => {
                        format!("起動コマンドが実行できない（{}）", watch.spec.agent)
                    }
                    (FailReason::RetriesExhausted, _) => format!(
                        "{} の起動を {} 回試したが確認できない（画面はシェルのまま）",
                        watch.spec.agent, watch.attempts
                    ),
                };
                eprintln!("warning: {msg}（pane={}）", watch.pane.as_u64());
                if matches!(reason, FailReason::RetriesExhausted) {
                    // 起動は確認できないが、プロンプトだけでも届く可能性は残す
                    // （未知の TUI 等）。PromptFlow 側にも 15 秒の耐性がある
                    self.queue_prompt_for(watch);
                }
                watch.enter(LaunchPhase::Failed, Some(msg));
            }
        }
    }

    /// プロンプト送達フローを積む（二重登録しない）
    fn queue_prompt_for(&mut self, watch: &mut LaunchWatch) {
        if watch.prompt_queued || watch.spec.prompt.is_empty() {
            return;
        }
        watch.prompt_queued = true;
        // 既存の送達フロー（#32 / #95 / #623）をそのまま使う。起動は確認済みなので
        // 先頭の alt_screen 待ちは即座に抜ける（入力欄が見えていれば進む実装）。
        // 信頼ダイアログの承諾・貼り付け・分離 Enter・空検証はフロー側の責務
        self.queue_prompt_flow(watch.pane, watch.spec.prompt.clone());
    }
}

/// 1 tick ぶんの判断結果。副作用（画面書き込み・フロー登録）と判断を分離することで、
/// 判断そのものを GUI なしで単体テストできるようにする
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LaunchAction {
    Wait,
    ShellReady,
    SendLaunch { cancel_line: bool },
    AgentStarted,
    SendPrompt,
    Delivered,
    Fail { reason: FailReason },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FailReason {
    /// コマンドが存在しない等。再送しても直らない
    LaunchError,
    /// 再送を使い切っても起動を確認できない
    RetriesExhausted,
}

/// 観測から次の一手を決める（純粋関数。`watch` は経過時間・段階の読み取りのみ）
fn decide(
    watch: &LaunchWatch,
    has_output: bool,
    screen: &LaunchScreen,
    prompt_flow_active: bool,
    command_flow_active: bool,
) -> LaunchAction {
    match watch.phase {
        // シェルが動き出した（何か描画された）か、待ちきれなくなったら先へ
        LaunchPhase::Queued => {
            if has_output || watch.phase_entered_at.elapsed() > SHELL_WAIT {
                LaunchAction::ShellReady
            } else {
                LaunchAction::Wait
            }
        }
        LaunchPhase::ShellReady => LaunchAction::SendLaunch { cancel_line: false },
        LaunchPhase::LaunchSent => match screen {
            LaunchScreen::AgentReady => LaunchAction::AgentStarted,
            LaunchScreen::LaunchError { .. } => LaunchAction::Fail {
                reason: FailReason::LaunchError,
            },
            // まだ送達確認フローが回っている = 送っている最中。判断を保留する
            _ if command_flow_active => LaunchAction::Wait,
            LaunchScreen::ShellPrompt | LaunchScreen::Unknown => {
                // シェルプロンプトが見えている = 起動コマンドが届いていない**確証**があるので
                // 早めに再送する。判断がつかない（Unknown）ときは逆に長く待つ:
                // 起動が遅いだけのエージェントへ再送すると、起動中の stdin へ
                // 起動コマンドを打ち込むことになる（害の方が大きい）
                let shell_idle = matches!(screen, LaunchScreen::ShellPrompt);
                let probe = if shell_idle {
                    LAUNCH_PROBE / 3
                } else {
                    LAUNCH_PROBE * 3
                };
                if watch.phase_entered_at.elapsed() <= probe {
                    LaunchAction::Wait
                } else if watch.attempts >= watch.spec.max_attempts {
                    LaunchAction::Fail {
                        reason: FailReason::RetriesExhausted,
                    }
                } else {
                    LaunchAction::SendLaunch {
                        cancel_line: shell_idle,
                    }
                }
            }
        },
        LaunchPhase::AgentStarted => LaunchAction::SendPrompt,
        // PromptFlow が終わった（= 入力欄から本文が消えた）ことをもって送達確認とする
        LaunchPhase::PromptSent => {
            if prompt_flow_active {
                LaunchAction::Wait
            } else {
                LaunchAction::Delivered
            }
        }
        LaunchPhase::PromptDelivered | LaunchPhase::Failed => LaunchAction::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchAssuranceSpec {
        LaunchAssuranceSpec {
            worker_id: String::new(),
            command: "claude --model opus".into(),
            prompt: "テストプロンプト".into(),
            agent: "claude".into(),
            max_attempts: 3,
        }
    }

    #[test]
    fn 新規は起動待ちから始まる() {
        let w = LaunchWatch::new(PaneId::from_raw(1), spec());
        assert_eq!(w.phase, LaunchPhase::Queued);
        assert_eq!(w.attempts, 0);
        assert!(!w.is_done());
        assert_eq!(w.status().phase, LaunchPhase::Queued);
    }

    #[test]
    fn 終端段階でループから外れる() {
        let mut w = LaunchWatch::new(PaneId::from_raw(1), spec());
        w.enter(LaunchPhase::PromptDelivered, None);
        assert!(w.is_done());

        let mut w = LaunchWatch::new(PaneId::from_raw(1), spec());
        w.enter(LaunchPhase::Failed, Some("だめ".into()));
        assert!(w.is_done());
        assert_eq!(w.status().detail.as_deref(), Some("だめ"));
    }

    #[test]
    fn 再送プローブはシェルプロンプト検出で短縮される() {
        // シェルプロンプトが見えている = 起動コマンドが届いていない確証があるので
        // 全体プローブを待たずに再送する。定数の関係が壊れたら気づけるようにする
        assert!(
            LAUNCH_PROBE / 3 < LAUNCH_PROBE,
            "短縮プローブは通常より短いこと"
        );
        assert!(
            TOTAL_TIMEOUT > LAUNCH_PROBE * 3 * 3,
            "判断がつかない画面でも再送 3 回を試せるだけの全体タイムアウトが要る"
        );
    }

    /// 段階と経過時間を任意に組める watch を作る
    fn at(phase: LaunchPhase, attempts: u32, ago: Duration) -> LaunchWatch {
        let mut w = LaunchWatch::new(PaneId::from_raw(1), spec());
        w.phase = phase;
        w.attempts = attempts;
        w.phase_entered_at = Instant::now() - ago;
        w
    }

    #[test]
    fn シェルの出力を見てから起動コマンドを送る() {
        let w = at(LaunchPhase::Queued, 0, Duration::ZERO);
        // まだ何も出ていない = 待つ（シェル初期化との競合を避ける）
        assert_eq!(
            decide(&w, false, &LaunchScreen::Unknown, false, false),
            LaunchAction::Wait
        );
        // 何か出た = 先へ進む
        assert_eq!(
            decide(&w, true, &LaunchScreen::Unknown, false, false),
            LaunchAction::ShellReady
        );
        // 出力が無いまま待ちきれない未知シェルでも先へ進む
        let old = at(LaunchPhase::Queued, 0, SHELL_WAIT + Duration::from_secs(1));
        assert_eq!(
            decide(&old, false, &LaunchScreen::Unknown, false, false),
            LaunchAction::ShellReady
        );
    }

    #[test]
    fn 起動を確認できたらプロンプトへ進む() {
        let w = at(LaunchPhase::LaunchSent, 1, Duration::ZERO);
        assert_eq!(
            decide(&w, true, &LaunchScreen::AgentReady, false, false),
            LaunchAction::AgentStarted
        );
        let w = at(LaunchPhase::AgentStarted, 1, Duration::ZERO);
        assert_eq!(
            decide(&w, true, &LaunchScreen::AgentReady, false, false),
            LaunchAction::SendPrompt
        );
    }

    #[test]
    fn シェルのままなら再送する() {
        // #640 の症状: 起動コマンドが届かず素の PowerShell のまま
        let w = at(
            LaunchPhase::LaunchSent,
            1,
            LAUNCH_PROBE / 3 + Duration::from_secs(1),
        );
        assert_eq!(
            decide(&w, true, &LaunchScreen::ShellPrompt, false, false),
            LaunchAction::SendLaunch { cancel_line: true }
        );
    }

    #[test]
    fn 起動確認前は待つ() {
        // プローブ時間内は再送しない（起動が遅いだけのケースを潰さない）
        let w = at(LaunchPhase::LaunchSent, 1, Duration::from_millis(500));
        assert_eq!(
            decide(&w, true, &LaunchScreen::ShellPrompt, false, false),
            LaunchAction::Wait
        );
        assert_eq!(
            decide(&w, true, &LaunchScreen::Unknown, false, false),
            LaunchAction::Wait
        );
    }

    #[test]
    fn 再送を使い切ったら失敗にする() {
        let w = at(
            LaunchPhase::LaunchSent,
            3, // max_attempts と同数
            LAUNCH_PROBE + Duration::from_secs(1),
        );
        assert_eq!(
            decide(&w, true, &LaunchScreen::ShellPrompt, false, false),
            LaunchAction::Fail {
                reason: FailReason::RetriesExhausted
            }
        );
    }

    #[test]
    fn 送達確認フローが動いている間は再送しない() {
        // #640 の送達確認フローがまだ本文を打っている最中に再送すると、
        // シェルへ 2 行ぶん打ち込むことになる（#640 と #665 の合流点）
        let w = at(
            LaunchPhase::LaunchSent,
            1,
            LAUNCH_PROBE * 3 + Duration::from_secs(1),
        );
        assert_eq!(
            decide(&w, true, &LaunchScreen::ShellPrompt, false, true),
            LaunchAction::Wait,
            "送達確認フロー稼働中は待つ"
        );
        // フローが終われば通常どおり再送する
        assert_eq!(
            decide(&w, true, &LaunchScreen::ShellPrompt, false, false),
            LaunchAction::SendLaunch { cancel_line: true }
        );
        // ただしエージェント起動・起動エラーの判定は送達確認より優先する
        assert_eq!(
            decide(&w, true, &LaunchScreen::AgentReady, false, true),
            LaunchAction::AgentStarted
        );
    }

    #[test]
    fn 判断がつかない画面へは早く再送しない() {
        // 起動が遅いだけのエージェントへ再送すると、起動中の stdin へ
        // 起動コマンドを打ち込むことになる。確証（シェルプロンプト）が
        // 無いうちは長く待つ
        let w = at(
            LaunchPhase::LaunchSent,
            1,
            LAUNCH_PROBE + Duration::from_secs(1),
        );
        assert_eq!(
            decide(&w, true, &LaunchScreen::Unknown, false, false),
            LaunchAction::Wait,
            "Unknown ではまだ待つ"
        );
        // 同じ経過時間でも、シェルプロンプトが見えていれば再送する
        assert_eq!(
            decide(&w, true, &LaunchScreen::ShellPrompt, false, false),
            LaunchAction::SendLaunch { cancel_line: true }
        );
        // 十分待っても何も出なければ再送する（未知シェルで諦めないため）
        let long = at(
            LaunchPhase::LaunchSent,
            1,
            LAUNCH_PROBE * 3 + Duration::from_secs(1),
        );
        assert_eq!(
            decide(&long, true, &LaunchScreen::Unknown, false, false),
            LaunchAction::SendLaunch { cancel_line: false }
        );
    }

    #[test]
    fn コマンドが存在しなければ再送せず即失敗する() {
        // 存在しないエージェント CLI。再送しても直らないので待たずに落とす
        let w = at(LaunchPhase::LaunchSent, 1, Duration::ZERO);
        assert_eq!(
            decide(
                &w,
                true,
                &LaunchScreen::LaunchError {
                    detail: "command not found".into()
                },
                false,
                false
            ),
            LaunchAction::Fail {
                reason: FailReason::LaunchError
            }
        );
    }

    #[test]
    fn 送達確認はpromptflowの完了を待つ() {
        let w = at(LaunchPhase::PromptSent, 1, Duration::ZERO);
        // フローが走っている間は待つ
        assert_eq!(
            decide(&w, true, &LaunchScreen::AgentReady, true, false),
            LaunchAction::Wait
        );
        // フローが消えた = 入力欄から本文が消えた = 送達確認
        assert_eq!(
            decide(&w, true, &LaunchScreen::AgentReady, false, false),
            LaunchAction::Delivered
        );
    }

    #[test]
    fn 終端段階では何もしない() {
        for phase in [LaunchPhase::PromptDelivered, LaunchPhase::Failed] {
            let w = at(phase, 1, Duration::from_secs(60));
            assert_eq!(
                decide(&w, true, &LaunchScreen::ShellPrompt, false, false),
                LaunchAction::Wait,
                "{phase:?} は追加の副作用を起こさない"
            );
        }
    }
}
