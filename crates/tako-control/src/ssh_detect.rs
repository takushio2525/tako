//! ペインの `ssh` を検知してリモートフォルダを自動追加する（Issue #976 / #65 要件 1）の
//! 走査層。
//!
//! # 何をどこで決めるか
//!
//! | 層 | 中身 |
//! |---|---|
//! | `tako_core::ssh_detect` | コマンド行 → 宛先（純関数） |
//! | ここ | 「どのペインの配下に ssh が居るか」の判定・**再走査の間引き**・見送りログの重複抑止（#1258）・**検知の材料（`~/.ssh/config`）の採取**（#1411） |
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
//!
//! # 材料を読むのも走る tick だけ（#1411）
//!
//! 非既定ポートの `-p` を「宛先の名前そのものが持つポート」と突き合わせるために
//! `~/.ssh/config` を読む（[`tako_core::ssh_detect::ConfiguredPorts`]）。読むのは
//! [`scan`] が**実際に走査する tick** だけで、間引かれた tick は 1 行も触らない。
//! 材料を引数で受ける [`scan_in`] が本体なので、テストは実ユーザーの config に
//! 左右されない。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use tako_core::ssh_detect::{parse_ssh_command_with, DetectContext, SkipReason};
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
    /// 見送った ssh プロセスの pid。**見送りログの重複抑止（#1258）の鍵の一部**で、
    /// 「同じ ssh がまだ生きている」と「打ち直された別の ssh」を分ける
    pub pid: u32,
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

/// 見送りログの重複抑止（Issue #1258）。
///
/// # なぜ要るのか
///
/// 見送りの判定は 2 秒 tick のたびに読み直される。走査を間引いた tick では
/// [`scan`] が `prev.skipped` をそのまま複製して返す（= 「見ていない」を
/// 「消えた」と読み替えないための仕様）ので、**判定のたびに書くと同じ行が
/// ペインの ssh が生きている間ずっと積もる**（実測: 非対話 ssh が 3 時間
/// ぶら下がって 866 行 = #1258）。他の行（送達・復元・kill）が埋もれ、
/// `grep` で追う運用（#770 / #790）が壊れる。
///
/// # 何を 1 エピソードと見るか
///
/// 鍵は **(ペイン, ssh の pid, 理由)**。`pid` を鍵に入れるので:
///
/// - 同じ ssh が生きている間は何回評価しても 1 行
/// - 打ち直した（pid が替わった）ら、それは別のエピソードなので再び 1 行
/// - 同じ pid で理由が変わる形（argv の読み替え）も別の 1 行
///
/// ペイン ID はプロセス生存期間中**単調増加**（`tako_core::PaneId`）なので、
/// 閉じたペインの ID が別のペインへ回ってきて記憶が混ざることはない。
///
/// # 記憶の刈り取り
///
/// 覚えるのは**いま見送られている鍵だけ**。消えた鍵は落とす（= ssh を打ち直す
/// ループを回されても記憶が無限に伸びない）。落としたぶんは次に同じ形が
/// 現れたら再び 1 行書く = 「プロセスが替わったら再び 1 回」と同じ扱い。
///
/// 見送り → 対話 ssh へ切り替えた場合も、見送りが消えた時点で記憶が落ちるので
/// 次の見送りは新しいエピソードとして 1 行残る。
#[derive(Debug, Clone, Default)]
pub struct SshSkipLog {
    /// 既に記録した (ペイン, pid, 理由)
    logged: HashSet<(u64, u32, SkipReason)>,
}

impl SshSkipLog {
    /// **今回書くべき見送り**だけを返し、書いたことを覚える。
    ///
    /// `legacy` = `TAKO_1258_LEGACY=1`（[`legacy_1258`]）のときは抑止せず
    /// 全部返す（同一バイナリで #1258 前の挙動を再現する A/B の口）
    pub fn take_new(&mut self, skipped: &[SkippedSsh], legacy: bool) -> Vec<SkippedSsh> {
        if legacy {
            return skipped.to_vec();
        }
        let mut fresh: Vec<SkippedSsh> = Vec::new();
        let mut present: HashSet<(u64, u32, SkipReason)> = HashSet::new();
        for skip in skipped {
            let key = (skip.pane, skip.pid, skip.reason);
            // 同じ鍵が 1 回の走査に 2 度現れても書くのは 1 行
            if !present.insert(key) {
                continue;
            }
            if !self.logged.contains(&key) {
                fresh.push(skip.clone());
            }
        }
        // いま見送られていない鍵は忘れる（記憶を現状へ揃える）
        self.logged = present;
        fresh
    }

    /// 覚えている鍵の数（テストが**量**で性質を固定するための口。#1167 の規約）
    pub fn remembered(&self) -> usize {
        self.logged.len()
    }
}

/// #1258 の A/B。`TAKO_1258_LEGACY=1` で**修正前の挙動**（見送りを判定のたびに
/// `persist.log` へ書く）を同一バイナリで再現する
pub fn legacy_1258() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1258_LEGACY").is_some())
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
    // #1411: 材料（`~/.ssh/config`）を読むのは**走査が実際に起きる tick だけ**。
    // 間引かれた tick はここで戻るので、2 秒ごとにファイルを触らない
    // （走る tick でも読むのは 1 ファイル分 = 同じ tick の
    // `ProcessSnapshot::capture()` が起こす `ps` より軽い）
    let Some(snapshot) = snapshot else {
        return carried_over(prev, targets);
    };
    scan_in(
        prev,
        targets,
        Some(snapshot),
        now,
        &DetectContext::current(),
    )
}

/// 走査を間引いた tick の戻り（**前回の結果をそのまま持ち越す**。
/// 「ssh が消えた」と誤解しないため）
fn carried_over(prev: &SshScanState, targets: Vec<SshScanTarget>) -> SshScanState {
    SshScanState {
        targets,
        sessions: prev.sessions.clone(),
        skipped: prev.skipped.clone(),
        scanned_at: prev.scanned_at,
        argv_unavailable: prev.argv_unavailable,
    }
}

/// [`scan`] の中身（**検知の材料を引数で受ける**ので `~/.ssh/config` も env も読まない）。
/// テストと A/B はこちらを呼ぶ（実ユーザーの config に結果が左右されない）
pub fn scan_in(
    prev: &SshScanState,
    targets: Vec<SshScanTarget>,
    snapshot: Option<&ProcessSnapshot>,
    now: Instant,
    ctx: &DetectContext,
) -> SshScanState {
    let Some(snapshot) = snapshot else {
        return carried_over(prev, targets);
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
            match parse_ssh_command_with(argv, ctx) {
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
                    pid,
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

    /// テストの走査（**実ユーザーの `~/.ssh/config` を読まない**。
    /// 材料なし = #1411 以前と同じ「非既定ポートは全部見送る」物差し）
    fn scan_t(
        prev: &SshScanState,
        targets: Vec<SshScanTarget>,
        snapshot: Option<&ProcessSnapshot>,
        now: Instant,
    ) -> SshScanState {
        scan_in(prev, targets, snapshot, now, &DetectContext::strict())
    }

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
        let state = scan_t(
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
        let state = scan_t(
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
        let state = scan_t(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert!(state.sessions.is_empty(), "{:?}", state.sessions);
    }

    /// tako 自身が開いた SSH ペインの実コマンド行（#1411 の実測と同じ形）。
    /// ControlPath は macOS の既定 data_dir（**空白を含む**）で組む
    fn self_opened_argv(host: &str, port: u16) -> String {
        let dir = std::path::Path::new("/Users/testuser/Library/Application Support/tako");
        let cp = tako_core::remote_fs::control_path_option(&tako_core::remote_fs::control_path_in(
            Some(dir),
            host,
        ));
        format!(
            "/usr/bin/ssh -o {cp} -o ControlMaster=auto -o ControlPersist=600 \
             -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 \
             -p {port} {host}"
        )
    }

    #[test]
    fn tako自身が開いたsshペインは材料があればsessionsに載る() {
        let argv = self_opened_argv("win", 2222);
        let snap = snapshot(vec![], &[(200, 100)], &[(200, argv.as_str())]);
        let targets = vec![target(1, 100, CommandState::Running)];
        let ctx =
            DetectContext::with_configured(tako_core::ssh_detect::ConfiguredPorts::from_pairs(&[
                ("win", 2222),
            ]));
        let state = scan_in(
            &SshScanState::default(),
            targets.clone(),
            Some(&snap),
            Instant::now(),
            &ctx,
        );
        assert_eq!(state.skipped, Vec::new(), "見送りが残っている");
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].destination, "win");
        assert!(state.is_live("win"));

        // 材料が無ければ従来どおり見送る（#1411 以前 = Issue の実測）
        let legacy = scan_in(
            &SshScanState::default(),
            targets,
            Some(&snap),
            Instant::now(),
            &DetectContext::legacy(),
        );
        assert!(legacy.sessions.is_empty());
        assert_eq!(legacy.skipped.len(), 1);
        // 空白で割れた ControlPath の続きを宛先と読むので理由は `RemoteCommand`
        // （ポートを 22 にしても起きる = #1411 の 2 つ目の原因）
        assert_eq!(legacy.skipped[0].reason, SkipReason::RemoteCommand);
    }

    #[test]
    fn 見送った形は理由つきで持ち帰る() {
        let snap = snapshot(vec![], &[(200, 100)], &[(200, "ssh -p 2222 win")]);
        let state = scan_t(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert!(state.sessions.is_empty());
        assert_eq!(state.skipped.len(), 1);
        assert_eq!(state.skipped[0].reason, SkipReason::PortOverride);
        // #1258: どの ssh プロセスの見送りかまで持ち帰る（重複抑止の鍵）
        assert_eq!(state.skipped[0].pid, 200);
    }

    #[test]
    fn 入れ子のsshは外側を採る() {
        // shell 100 → ssh outer 200 → （リモート側の ssh は見えないが）ssh inner 300
        let snap = snapshot(
            vec![],
            &[(200, 100), (300, 200)],
            &[(200, "ssh outer"), (300, "ssh inner")],
        );
        let state = scan_t(
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
        let state = scan_t(
            &SshScanState::default(),
            vec![target(1, 100, CommandState::Running)],
            Some(&snap),
            Instant::now(),
        );
        assert!(state.sessions.is_empty());
        assert!(state.argv_unavailable, "argv 不在を申告していない");
    }

    fn skip(pane: u64, pid: u32, reason: SkipReason) -> SkippedSsh {
        SkippedSsh { pane, pid, reason }
    }

    #[test]
    fn 同じ見送りを何回評価してもログは一行だけ() {
        // #1258: 2 秒 tick × 3 時間 = 5400 回ぶんの評価を 60 回で代表させる
        // （走査を間引いた tick でも `state.skipped` は前回ぶんが載っている）
        let skipped = vec![skip(1627, 4242, SkipReason::RemoteCommand)];
        let mut log = SshSkipLog::default();
        let mut lines = 0usize;
        for _ in 0..60 {
            lines += log.take_new(&skipped, false).len();
        }
        assert_eq!(
            lines, 1,
            "同じ ssh の見送りで {lines} 行書いている（#1258）"
        );
        assert_eq!(log.remembered(), 1, "記憶が現状より膨らんでいる");
    }

    #[test]
    fn sshを打ち直したら再び一行だけ書く() {
        let mut log = SshSkipLog::default();
        let first = vec![skip(1627, 4242, SkipReason::RemoteCommand)];
        assert_eq!(log.take_new(&first, false).len(), 1);
        assert_eq!(log.take_new(&first, false).len(), 0);
        // 同じペイン・同じ理由でも pid が替わったら別のエピソード
        let second = vec![skip(1627, 5151, SkipReason::RemoteCommand)];
        let fresh = log.take_new(&second, false);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].pid, 5151);
        assert_eq!(log.take_new(&second, false).len(), 0);
        // 消えた pid の記憶は落ちている（ループで打ち直されても伸びない）
        assert_eq!(log.remembered(), 1, "記憶が刈り取られていない");
    }

    #[test]
    fn 理由やペインが変わったら別の一行として書く() {
        let mut log = SshSkipLog::default();
        let both = vec![
            skip(1, 200, SkipReason::RemoteCommand),
            // 同じ pid で理由が変わる形（argv の読み替え）も別の 1 行
            skip(1, 200, SkipReason::PortOverride),
            // 別のペインは当然別の 1 行
            skip(2, 200, SkipReason::RemoteCommand),
        ];
        assert_eq!(log.take_new(&both, false).len(), 3);
        assert_eq!(log.take_new(&both, false).len(), 0);
        // 1 回の走査に同じ鍵が 2 度現れても書くのは 1 行
        let dup = vec![
            skip(9, 300, SkipReason::NoShell),
            skip(9, 300, SkipReason::NoShell),
        ];
        assert_eq!(log.take_new(&dup, false).len(), 1);
    }

    #[test]
    fn 対話sshへ切り替えると記憶が落ちて次の見送りが残る() {
        let mut log = SshSkipLog::default();
        let skipped = vec![skip(1627, 4242, SkipReason::RemoteCommand)];
        assert_eq!(log.take_new(&skipped, false).len(), 1);
        // `ssh host` へ入り直す = 見送りが消える（`sessions` 側で拾われる）
        assert!(log.take_new(&[], false).is_empty());
        assert_eq!(log.remembered(), 0, "見送りが消えても記憶が残っている");
        // 同じ pid が返ってくる（= 一時的に読めなかった）形でも 1 行で済む
        let fresh = log.take_new(&skipped, false);
        assert_eq!(fresh.len(), 1);
        assert_eq!(log.take_new(&skipped, false).len(), 0);
    }

    #[test]
    fn 間引いた_tick_で持ち越された見送りも一行に畳まれる() {
        // #1258 の温床そのものを再現する: 走査は 60 秒に 1 回しか走らないが、
        // `scan` は間引いた tick で `prev.skipped` を持ち越すので、**2 秒 tick の
        // たびに `state.skipped` には同じ見送りが載っている**
        let snap = snapshot(
            vec![],
            &[(200, 100)],
            &[(
                200,
                "ssh -o ConnectTimeout=30 win powershell -File probe.ps1",
            )],
        );
        let targets = vec![target(1, 100, CommandState::Running)];
        let mut state = SshScanState::default();
        let mut log = SshSkipLog::default();
        let mut lines = 0usize;
        let mut carried = 0usize;
        let t0 = Instant::now();
        // 2 秒 tick を 60 回 = 実測（3 時間）と同じ形を 2 分ぶんで代表させる
        for tick in 0..60u64 {
            let now = t0 + Duration::from_secs(2 * tick);
            let rescan = should_rescan(&state, &targets, false, now);
            state = scan_t(&state, targets.clone(), rescan.then_some(&snap), now);
            carried += state.skipped.len();
            lines += log.take_new(&state.skipped, false).len();
        }
        assert_eq!(state.skipped[0].reason, SkipReason::RemoteCommand);
        // 持ち越しは 60 tick ぶん載り続ける（= 素朴に書くと 60 行になっていた）
        assert_eq!(carried, 60, "持ち越しの前提が変わっている");
        assert_eq!(
            lines, 1,
            "持ち越された見送りで {lines} 行書いている（#1258）"
        );
    }

    #[test]
    fn 見送りログの抑止は同一バイナリで旧挙動へ戻せる() {
        // A/B: `TAKO_1258_LEGACY=1` を付けて同じテストバイナリを走らせると
        // 「判定のたびに書く」= #1258 の症状が再現する
        let legacy = legacy_1258();
        let skipped = vec![skip(1627, 4242, SkipReason::RemoteCommand)];
        let mut log = SshSkipLog::default();
        let mut lines = 0usize;
        for _ in 0..60 {
            lines += log.take_new(&skipped, legacy).len();
        }
        // 報告用（`-- --nocapture` で A/B の実数がそのまま読める）
        eprintln!(
            "[#1258 A/B] arm={} 60 回評価 -> {lines} 行",
            if legacy { "legacy" } else { "default" }
        );
        if legacy {
            assert_eq!(lines, 60, "TAKO_1258_LEGACY=1 で旧挙動が再現していない");
        } else {
            assert_eq!(lines, 1, "既定で {lines} 行書いている（#1258）");
        }
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
        let state = scan_t(
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
