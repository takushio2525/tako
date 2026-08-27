//! ペインの `ssh` を検知してリモートフォルダを自動追加する（Issue #976 / #65 要件 1）の
//! 走査層。
//!
//! # 何をどこで決めるか
//!
//! | 層 | 中身 |
//! |---|---|
//! | `tako_core::ssh_detect` | コマンド行 → 宛先（純関数） |
//! | ここ | 「どのペインの配下に ssh が居るか」の判定と**再走査の間引き** |
//! | `tako-app` | 自動追加の実行（background で接続 → ルートを足す）と切断の表示 |
//!
//! # 毎 tick 走らせない（#772 / #779 / #782 の教訓）
//!
//! プロセス表の採取は子プロセス起動を伴うので、**2 秒 tick のたびには走らせない**。
//! [`should_rescan`] が見るのはメモリ上の材料だけ:
//!
//! - ペインの集合・PTY 直下の子 pid・**OSC 133 のコマンド状態**が前回と変わったか
//! - 変わっていなくても [`RESCAN_INTERVAL`] を過ぎたか（**取りこぼしの保険**）
//!
//! 対話 `ssh` はフォアグラウンドのコマンドなので、入るときに `Idle → Running`、
//! 抜けるときに `Running → Idle` へ変わる = **検知も切断もこの指紋の変化で拾える**。
//! ssh が生きている間は指紋が動かないので走査は起きない（= 常時コストがゼロ）。
//!
//! シェル統合が効いていないペイン（状態が `Unknown` のまま）だけは変化が現れないので
//! 保険の間隔に頼る。全ペインが `Idle` で追跡中のホストも無ければ、保険も走らせない
//! （アイドルの tako が `ps` を起動しない = #976 受け入れ条件の「アイドル時の増加なし」）。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use tako_core::ssh_detect::{parse_ssh_command, SkipReason};
use tako_core::CommandState;

use crate::agents::ProcessSnapshot;

/// 指紋が動かないときの再走査間隔（取りこぼしの保険）
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(60);

/// 走査対象のペイン 1 枚。指紋の材料は**すべてメモリ上にある**もので、
/// これを作るために OS へ問い合わせない
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshScanTarget {
    pub pane: u64,
    pub tab: u64,
    /// 永続化バックエンド（tmux / psmux）のセッション名。器を持つペインは
    /// シェルが器のサーバー配下に居るので、pid ではなくここから辿る
    pub backend_session: Option<String>,
    /// PTY 直下の子 pid（器を持たないペイン用）
    pub child_pid: Option<u32>,
    /// OSC 133 のコマンド状態（`Running` への遷移が「ssh に入った」の合図）
    pub state: CommandState,
}

/// ペイン配下で見つかった ssh セッション 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSsh {
    pub pane: u64,
    pub tab: u64,
    /// `ssh` / `sftp` へそのまま渡せる宛先（`host` または `user@host`）
    pub destination: String,
    pub pid: u32,
}

/// 見送った ssh（**なぜ出てこないのか**を `auto` の応答で説明するために持ち帰る）。
///
/// 理由は文字列ではなく [`SkipReason`] のまま持つ: 文言へ落とすのは表示・応答の
/// 直前にする（走査時の言語で凍結させない。#516 で踏んだ罠と同型）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSsh {
    pub pane: u64,
    pub reason: SkipReason,
}

/// 走査の結果と、次回の間引き判断に使う指紋
#[derive(Debug, Clone, Default)]
pub struct SshScanState {
    pub targets: Vec<SshScanTarget>,
    pub sessions: Vec<DetectedSsh>,
    pub skipped: Vec<SkippedSsh>,
    pub scanned_at: Option<Instant>,
    /// 走査したが argv を採れなかった（境界が argv を返せない環境 = Windows）。
    /// 「検知できない」と「ssh が居ない」を混同しないための旗
    pub argv_unavailable: bool,
}

impl SshScanState {
    /// この宛先が生きているか（最後の走査時点）
    pub fn is_live(&self, destination: &str) -> bool {
        self.sessions.iter().any(|s| s.destination == destination)
    }
}

/// プロセス表を採り直す必要があるか。**メモリ上の材料だけ**で判定する。
///
/// `has_tracked` = 自動追加したルートを 1 つ以上抱えているか。抱えているなら
/// 切断の検出のために保険の間隔で見に行く（抱えていなければ何もしない）
pub fn should_rescan(
    prev: &SshScanState,
    targets: &[SshScanTarget],
    has_tracked: bool,
    now: Instant,
) -> bool {
    if targets.is_empty() {
        return false;
    }
    let Some(scanned_at) = prev.scanned_at else {
        return true; // 初回
    };
    if prev.targets != *targets {
        return true; // ペインの集合・子 pid・コマンド状態のどれかが動いた
    }
    // 指紋が動かないまま取りこぼす経路（シェル統合が無いペイン）と、
    // 切断の検出のためだけに保険を回す
    let needs_insurance = has_tracked
        || targets
            .iter()
            .any(|t| matches!(t.state, CommandState::Unknown | CommandState::Running));
    needs_insurance && now.duration_since(scanned_at) >= RESCAN_INTERVAL
}

/// ペイン配下の ssh を拾う。`snapshot` が None なら**前回の結果を保つ**
/// （走査を間引いた tick で「ssh が消えた」と誤解しないため）
pub fn scan(
    prev: &SshScanState,
    targets: Vec<SshScanTarget>,
    snapshot: Option<&ProcessSnapshot>,
    now: Instant,
) -> SshScanState {
    let Some(snapshot) = snapshot else {
        return SshScanState {
            targets,
            sessions: prev.sessions.clone(),
            skipped: prev.skipped.clone(),
            scanned_at: prev.scanned_at,
            argv_unavailable: prev.argv_unavailable,
        };
    };
    let mut sessions: Vec<DetectedSsh> = Vec::new();
    let mut skipped: Vec<SkippedSsh> = Vec::new();
    let mut seen_argv = false;
    for target in &targets {
        // 器を持つペインは器のセッションから、持たないペインは PTY 直下の子から辿る
        // （#728 と同じ二段構え。どちらか一方だけだと片方の構成で永久に空になる）
        let roots: Vec<u32> = match &target.backend_session {
            Some(session) => snapshot.pane_pids_of(session),
            None => target.child_pid.into_iter().collect(),
        };
        let mut candidates: Vec<u32> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for root in roots {
            for pid in snapshot.descendants_with_root(root) {
                if seen.insert(pid) {
                    candidates.push(pid);
                }
            }
        }
        // 手前（ペインに近い）から見るので、入れ子の ssh では外側が採れる
        // （内側の宛先はこのマシンからは届かないので外側が正しい）
        let mut found = false;
        for pid in candidates {
            let Some(argv) = snapshot.argv(pid) else {
                continue;
            };
            seen_argv = true;
            if !argv_looks_like_ssh(argv) {
                continue;
            }
            match parse_ssh_command(argv) {
                Ok(cmd) if !found => {
                    found = true;
                    sessions.push(DetectedSsh {
                        pane: target.pane,
                        tab: target.tab,
                        destination: cmd.destination,
                        pid,
                    });
                }
                Ok(_) => {}
                Err(SkipReason::NotSsh) => {}
                Err(reason) => skipped.push(SkippedSsh {
                    pane: target.pane,
                    reason,
                }),
            }
        }
    }
    SshScanState {
        targets,
        sessions,
        skipped,
        scanned_at: Some(now),
        argv_unavailable: !seen_argv,
    }
}

/// 先頭の語が ssh か（`parse_ssh_command` を全プロセスへ掛ける前の軽い篩）
fn argv_looks_like_ssh(argv: &str) -> bool {
    argv.split_whitespace()
        .next()
        .is_some_and(tako_core::ssh_detect::is_ssh_program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn target(pane: u64, child_pid: u32, state: CommandState) -> SshScanTarget {
        SshScanTarget {
            pane,
            tab: 1,
            backend_session: None,
            child_pid: Some(child_pid),
            state,
        }
    }

    fn snapshot(
        panes: Vec<(String, u32)>,
        parents: &[(u32, u32)],
        argv: &[(u32, &str)],
    ) -> ProcessSnapshot {
        ProcessSnapshot::from_parts_for_test(
            panes,
            parents.iter().copied().collect::<HashMap<_, _>>(),
            argv.iter()
                .map(|(pid, a)| (*pid, a.to_string()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn ペイン配下のsshを宛先つきで拾う() {
        // pane shell 100 → ssh 200
        let snap = snapshot(
            vec![],
            &[(200, 100), (100, 10)],
            &[(100, "-zsh"), (200, "ssh win")],
        );
        let state = scan(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].destination, "win");
        assert_eq!(state.sessions[0].pane, 1);
        assert!(state.is_live("win"));
        assert!(!state.argv_unavailable);
    }

    #[test]
    fn 器を持つペインは器のセッションから辿る() {
        // psmux / tmux 配下: pane shell は器のサーバーの子なので pid では辿れない
        let snap = snapshot(
            vec![("tako-1:0.0".into(), 500), ("other:0.0".into(), 900)],
            &[(600, 500), (901, 900)],
            &[(600, "ssh box"), (901, "ssh should-not-match")],
        );
        let state = scan(
            &SshScanState::default(),
            vec![SshScanTarget {
                pane: 7,
                tab: 3,
                backend_session: Some("tako-1".into()),
                child_pid: None,
                state: CommandState::Running,
            }],
            Some(&snap),
            Instant::now(),
        );
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].destination, "box");
        assert_eq!(state.sessions[0].tab, 3);
    }

    #[test]
    fn 他のペインや無関係なプロセスのsshは拾わない() {
        let snap = snapshot(
            vec![],
            // 400 は tako のペイン配下ではない（別アプリの ssh）
            &[(200, 100), (400, 300)],
            &[(200, "vim"), (400, "ssh elsewhere")],
        );
        let state = scan(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert!(state.sessions.is_empty(), "{:?}", state.sessions);
    }

    #[test]
    fn 見送った形は理由つきで持ち帰る() {
        let snap = snapshot(vec![], &[(200, 100)], &[(200, "ssh -p 2222 win")]);
        let state = scan(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert!(state.sessions.is_empty());
        assert_eq!(state.skipped.len(), 1);
        assert_eq!(state.skipped[0].reason, SkipReason::PortOverride);
    }

    #[test]
    fn 入れ子のsshは外側を採る() {
        // shell 100 → ssh outer 200 → （リモート側の ssh は見えないが）ssh inner 300
        let snap = snapshot(
            vec![],
            &[(200, 100), (300, 200)],
            &[(200, "ssh outer"), (300, "ssh inner")],
        );
        let state = scan(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].destination, "outer");
    }

    #[test]
    fn argvを採れない環境は旗が立つ() {
        // 境界が実行ファイル名しか返せない環境（Windows）= argv が空
        let snap = snapshot(vec![], &[(200, 100)], &[]);
        let state = scan(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert!(state.sessions.is_empty());
        assert!(state.argv_unavailable, "argv 不在を申告していない");
    }

    #[test]
    fn 走査を間引いた_tick_は前回の結果を保つ() {
        let prev = SshScanState {
            targets: vec![target(1, 100, CommandState::Running)],
            sessions: vec![DetectedSsh {
                pane: 1,
                tab: 1,
                destination: "win".into(),
                pid: 200,
            }],
            skipped: Vec::new(),
            scanned_at: Some(Instant::now()),
            argv_unavailable: false,
        };
        let state = scan(
            &prev,
            vec![target(1, 100, CommandState::Running)],
            None,
            Instant::now(),
        );
        // 「見ていない」を「消えた」と読み替えない
        assert!(state.is_live("win"));
        assert_eq!(state.scanned_at, prev.scanned_at);
    }

    #[test]
    fn 初回と指紋の変化だけで走査する() {
        let now = Instant::now();
        let targets = vec![target(1, 100, CommandState::Idle)];
        // 初回は走る
        assert!(should_rescan(
            &SshScanState::default(),
            &targets,
            false,
            now
        ));
        // 対象が無ければ走らない
        assert!(!should_rescan(&SshScanState::default(), &[], true, now));

        let prev = SshScanState {
            targets: targets.clone(),
            scanned_at: Some(now),
            ..Default::default()
        };
        // 変化なし・全ペイン idle・追跡なし = アイドルの tako は ps を起動しない
        assert!(!should_rescan(&prev, &targets, false, now));
        assert!(!should_rescan(
            &prev,
            &targets,
            false,
            now + RESCAN_INTERVAL + Duration::from_secs(1)
        ));
        // コマンド状態が Running へ動いたら即走る（= ssh に入った合図）
        let running = vec![target(1, 100, CommandState::Running)];
        assert!(should_rescan(&prev, &running, false, now));
        // 追跡中のルートがあれば保険が効く（切断の検出）
        assert!(!should_rescan(&prev, &targets, true, now));
        assert!(should_rescan(
            &prev,
            &targets,
            true,
            now + RESCAN_INTERVAL + Duration::from_secs(1)
        ));
        // シェル統合が無いペイン（Unknown のまま）も保険で拾う
        let unknown = vec![target(1, 100, CommandState::Unknown)];
        let prev_unknown = SshScanState {
            targets: unknown.clone(),
            scanned_at: Some(now),
            ..Default::default()
        };
        assert!(!should_rescan(&prev_unknown, &unknown, false, now));
        assert!(should_rescan(
            &prev_unknown,
            &unknown,
            false,
            now + RESCAN_INTERVAL + Duration::from_secs(1)
        ));
    }
}
