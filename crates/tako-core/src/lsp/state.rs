//! 言語サーバ 1 つのライフサイクル（純粋な遷移関数。#1678）
//!
//! ```text
//! [未起動] --(編集モードで開いた)--> [起動中]
//! [起動中] --(実行ファイルが無い)--> [未導入]
//! [起動中] --(initialize の応答)--> [稼働]
//! [起動中 / 稼働] --(プロセスが死んだ)--> [クラッシュ] --(待ってから)--> [起動中]
//!                                          └-(上限を超えた)--> [諦めた]
//! [稼働] --(最後の文書を閉じて猶予が過ぎた)--> [停止中] --(shutdown / exit)--> [未起動]
//! [どこでも] --(利用者の stop)--> [停止中] --> [止めた]
//! [どこでも] --(利用者の restart)--> [起動中]（数えた回数は 0 へ戻す）
//! ```
//!
//! プロセスの操作（spawn / kill / 待ち）は tako-control の `lsp::manager` が持ち、
//! ここは「次の状態と、やるべきこと」だけを返す。

/// 状態（`tako lsp status` の `state` に出る）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// 一度も起こしていない / 猶予後に止めた
    NotStarted,
    /// spawn から initialize の応答まで
    Starting,
    /// 握手が済んで文書を同期している
    Running,
    /// shutdown → exit を送っている途中
    Stopping,
    /// 利用者が止めた（restart まで自動では起こさない）
    Stopped,
    /// 実行ファイルが見つからない（理由 + 導入コマンドを案内する）
    NotInstalled,
    /// 落ちた。待ってから起こし直す
    Crashed,
    /// 再起動の上限を超えた（restart まで起こさない）
    GaveUp,
}

impl ServerState {
    /// 機械可読の綴り（応答 JSON の `state`）
    pub fn slug(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::NotInstalled => "not_installed",
            Self::Crashed => "crashed",
            Self::GaveUp => "gave_up",
        }
    }

    pub const ALL: [Self; 8] = [
        Self::NotStarted,
        Self::Starting,
        Self::Running,
        Self::Stopping,
        Self::Stopped,
        Self::NotInstalled,
        Self::Crashed,
        Self::GaveUp,
    ];

    /// プロセスが居る（居るはずの）状態か
    pub fn has_process(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// 起きたこと
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// 対象の文書を開いた（編集モードに入った）
    Open,
    /// 実行ファイルが見つからなかった
    NotFound,
    /// initialize の応答が来た
    Initialized,
    /// プロセスが死んだ（spawn の失敗・initialize の失敗 / タイムアウトを含む）
    Exited,
    /// クラッシュ後の待ちが明けた
    RestartDue,
    /// 最後の文書を閉じてから猶予が過ぎた
    IdleTimeout,
    /// 停止（shutdown / exit / kill）が済んだ
    StopFinished,
    /// 利用者の stop
    UserStop,
    /// 利用者の restart
    UserRestart,
}

/// 遷移に伴ってやること
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// 起こす（実行ファイルの解決 → spawn → initialize）
    Spawn,
    /// いまのプロセスを止めてから起こす
    Respawn,
    /// 止める（shutdown → exit → 期限を過ぎたら kill）
    Shutdown,
    /// `delay_ms` 待ってから [`Event::RestartDue`] を入れる
    ScheduleRestart {
        attempt: u32,
        delay_ms: u64,
    },
}

/// 再起動の上限（#813 の「3 回で打ち切り」と同じ作法。無限再起動でマシンを焼かない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// これを超えて落ちたら諦める
    pub max_restarts: u32,
    /// 1 回目の待ち。以降は倍々（指数バックオフ）
    pub base_delay_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            base_delay_ms: 1000,
        }
    }
}

/// 状態と、落ちた回数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifecycle {
    pub state: ServerState,
    /// 利用者の restart 以降に落ちた回数。**握手が済んでも 0 へ戻さない**
    /// （握手の直後に落ちる壊れ方で、上限が永久に来なくなる）
    pub crashes: u32,
    /// 利用者の stop で止めている途中か（停止が済んだ先を「止めた」にする）
    pub held: bool,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            state: ServerState::NotStarted,
            crashes: 0,
            held: false,
        }
    }
}

impl Lifecycle {
    /// 1 つ遷移させ、やるべきことを返す。意味の無い組み合わせは何もしない
    pub fn step(self, event: Event, policy: RestartPolicy) -> (Self, Action) {
        use ServerState as S;
        let keep = (self, Action::None);
        match (self.state, event) {
            (_, Event::UserRestart) => {
                let next = Self {
                    state: S::Starting,
                    crashes: 0,
                    held: false,
                };
                let action = if self.state.has_process() {
                    Action::Respawn
                } else {
                    Action::Spawn
                };
                (next, action)
            }
            (S::Stopped | S::NotInstalled | S::GaveUp, Event::UserStop) => keep,
            (state, Event::UserStop) => {
                let action = if state.has_process() {
                    Action::Shutdown
                } else {
                    Action::None
                };
                let next_state = if state.has_process() {
                    S::Stopping
                } else {
                    S::Stopped
                };
                let next = Self {
                    held: true,
                    ..self.with(next_state)
                };
                (next, action)
            }
            (S::NotStarted, Event::Open) => (self.with(S::Starting), Action::Spawn),
            (S::Starting, Event::NotFound) => (self.with(S::NotInstalled), Action::None),
            (S::Starting, Event::Initialized) => (self.with(S::Running), Action::None),
            (S::Starting | S::Running, Event::Exited) => {
                let crashes = self.crashes.saturating_add(1);
                if crashes > policy.max_restarts {
                    (
                        Self {
                            state: S::GaveUp,
                            crashes,
                            ..self
                        },
                        Action::None,
                    )
                } else {
                    let shift = (crashes - 1).min(16);
                    (
                        Self {
                            state: S::Crashed,
                            crashes,
                            ..self
                        },
                        Action::ScheduleRestart {
                            attempt: crashes,
                            delay_ms: policy.base_delay_ms.saturating_mul(1 << shift),
                        },
                    )
                }
            }
            (S::Crashed, Event::RestartDue) => (self.with(S::Starting), Action::Spawn),
            (S::Running, Event::IdleTimeout) => (self.with(S::Stopping), Action::Shutdown),
            // 利用者の stop の途中で終わったら「止めた」、猶予の停止なら「未起動」
            (S::Stopping, Event::StopFinished) => {
                let state = if self.held { S::Stopped } else { S::NotStarted };
                (self.with(state), Action::None)
            }
            _ => keep,
        }
    }

    fn with(self, state: ServerState) -> Self {
        Self { state, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ServerState as S;

    fn policy() -> RestartPolicy {
        RestartPolicy::default()
    }

    fn run(events: &[Event]) -> (Lifecycle, Vec<Action>) {
        let mut lc = Lifecycle::default();
        let mut actions = Vec::new();
        for &event in events {
            let (next, action) = lc.step(event, policy());
            lc = next;
            actions.push(action);
        }
        (lc, actions)
    }

    #[test]
    fn 開いて握手すれば稼働() {
        let (lc, actions) = run(&[Event::Open, Event::Initialized]);
        assert_eq!(lc.state, S::Running);
        assert_eq!(actions, vec![Action::Spawn, Action::None]);
    }

    #[test]
    fn 実行ファイルが無ければ未導入で止まり開き直しても起こさない() {
        let (lc, actions) = run(&[Event::Open, Event::NotFound, Event::Open]);
        assert_eq!(lc.state, S::NotInstalled);
        assert_eq!(actions, vec![Action::Spawn, Action::None, Action::None]);
    }

    /// 受け入れ条件: 再起動の上限を超えたら「諦めた」で止まり、それ以上 spawn しない
    #[test]
    fn 上限を超えて落ちたら諦めて以後_spawn_しない() {
        let mut lc = Lifecycle::default();
        let mut spawns = 0;
        let mut delays = Vec::new();
        let mut apply = |lc: &mut Lifecycle, event| {
            let (next, action) = lc.step(event, policy());
            *lc = next;
            match action {
                Action::Spawn | Action::Respawn => spawns += 1,
                Action::ScheduleRestart { delay_ms, .. } => delays.push(delay_ms),
                _ => {}
            }
        };
        apply(&mut lc, Event::Open);
        // 握手の直後に落ちる壊れ方を 10 回繰り返しても、起こすのは最初 + 3 回だけ
        for _ in 0..10 {
            apply(&mut lc, Event::Initialized);
            apply(&mut lc, Event::Exited);
            apply(&mut lc, Event::RestartDue);
            apply(&mut lc, Event::Open);
        }
        assert_eq!(lc.state, S::GaveUp);
        assert_eq!(lc.crashes, 4);
        assert_eq!(spawns, 1 + 3, "初回 + 再起動 3 回");
        assert_eq!(delays, vec![1000, 2000, 4000], "待ちは倍々");
    }

    #[test]
    fn 利用者の_restart_は回数を戻して起こし直す() {
        let (lc, _) = run(&[
            Event::Open,
            Event::Exited,
            Event::RestartDue,
            Event::Exited,
            Event::RestartDue,
            Event::Exited,
            Event::RestartDue,
            Event::Exited,
        ]);
        assert_eq!(lc.state, S::GaveUp);
        let (lc, action) = lc.step(Event::UserRestart, policy());
        assert_eq!(
            (lc.state, lc.crashes, action),
            (S::Starting, 0, Action::Spawn)
        );
        // 動いているものの restart は止めてから起こす
        let running = Lifecycle {
            state: S::Running,
            crashes: 2,
            held: false,
        };
        assert_eq!(
            running.step(Event::UserRestart, policy()).1,
            Action::Respawn
        );
    }

    #[test]
    fn 猶予が過ぎたら止めて未起動へ戻る() {
        let (lc, actions) = run(&[Event::Open, Event::Initialized, Event::IdleTimeout]);
        assert_eq!(lc.state, S::Stopping);
        assert_eq!(actions.last(), Some(&Action::Shutdown));
        // 停止中に死んだのはクラッシュではない
        assert_eq!(lc.step(Event::Exited, policy()).0.state, S::Stopping);
        assert_eq!(
            lc.step(Event::StopFinished, policy()).0.state,
            S::NotStarted
        );
    }

    #[test]
    fn 利用者の_stop_は開き直しても起こさない() {
        let (lc, actions) = run(&[Event::Open, Event::Initialized, Event::UserStop]);
        assert_eq!((lc.state, actions[2]), (S::Stopping, Action::Shutdown));
        let (stopped, _) = lc.step(Event::StopFinished, policy());
        assert_eq!(stopped.state, S::Stopped);
        assert_eq!(stopped.step(Event::Open, policy()), (stopped, Action::None));
        // プロセスの無い状態の stop はその場で「止めた」
        let idle = Lifecycle::default();
        assert_eq!(idle.step(Event::UserStop, policy()).0.state, S::Stopped);
    }

    #[test]
    fn 綴りはすべて異なる() {
        let mut slugs: Vec<_> = ServerState::ALL.iter().map(|s| s.slug()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), ServerState::ALL.len());
    }
}
