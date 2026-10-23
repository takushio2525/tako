//! プロセスの fd 上限（`RLIMIT_NOFILE`）を起動時に引き上げ、使用数と並べて読めるようにする（Issue #2359）
//!
//! ## 何が起きていたか
//!
//! Finder / Dock から起きた `.app` は launchd の既定（`launchctl limit maxfiles` = soft 256・
//! hard 無制限）をそのまま受け継ぎ、tako は上げていなかった。GUI は 1 ペインあたり fd を 4 本使う
//! （PTY の master・alacritty のイベントループの kqueue・子の終了を知らせる UnixStream の対）ので、
//! 本番の実測（28 ペインで 121 本）からは約 60 ペインで上限に届く。届くと IPC の接続を受けても
//! 接続ごとのスレッドの `try_clone`（dup）が EMFILE で落ちてすぐ閉じるので、CLI / MCP は届かず
//! （字面は送信の ENOTCONN / EPIPE / EINVAL か、読みの EOF =「応答が返らなかった」）、新しいペインの
//! PTY も開けない（#2002 の段 1 の注入 a2 で再現）。IPC の接続 1 本は GUI の fd 2 本（読み書き）。
//!
//! ## どう直すか
//!
//! 起動時に soft を `min(hard, OPEN_MAX, kern.maxfilesperproc)` まで上げる（hard は触らない）。
//!
//! - [`OPEN_MAX`]（10240）: macOS の setrlimit(2) が RLIMIT_NOFILE に求める上限
//!   （`rlim_cur = RLIM_INFINITY` は受けず、`min(OPEN_MAX, rlim_max)` を使えと書いてある）。
//!   hard が無制限（launchd の既定）でもここで止まる
//! - `kern.maxfilesperproc`: これを超える soft を EINVAL で断る macOS がある（Go の issue 40564）
//! - 先例: Ghostty（起動時に上げ、子の exec の前に元へ戻す）・WezTerm（`ulimit_nofile` = 既定 2048 を
//!   min(値, hard) で）・rustc の compiletest（min(kern.maxfilesperproc, hard)）
//!
//! 上げられなくても起動は続ける（記録だけ残す。[`Outcome::Failed`]）。
//!
//! ## 子へは起動時の値を渡す
//!
//! ペインの PTY の子（シェル・tmux の attach クライアント = その attach が起こす器の tmux サーバーと
//! 器のペインのシェル）は、fork の子ハンドラで**起動時の soft へ戻してから** exec する
//! （[`restore_for_child`]。構えるのは `fd_inherit::spawn_sealed`）。ユーザーのシェル・エージェントの
//! `ulimit -n` は修正前と同じ（Finder 起動なら Terminal.app と同じ 256）で、FD_SETSIZE（1024）を
//! 超えた fd を select(2) に渡す古いプログラム・soft の大きさで表を確保するプログラムの振る舞いを
//! 変えない（Ghostty #4232 と同じ判断）。tako が自分で起こす裏方（言語サーバ・git・`remote serve` の
//! daemon）は引き上げた値を受け継ぐ。
//!
//! ## Windows は対象外
//!
//! Windows のハンドルは 1 プロセス 2^24 本まで使え、既定の小さい上限が無い。Rust の std はファイル・
//! ソケット・パイプを CRT の fd ではなく HANDLE / SOCKET で扱うので、CRT の `_setmaxstdio`（既定 512）
//! にも当たらない。[`raise_at_startup`] は [`Outcome::NotApplicable`] を返し、使用数も読まない。

use std::sync::OnceLock;

/// macOS の `OPEN_MAX`（`<sys/syslimits.h>`）。setrlimit(2) が RLIMIT_NOFILE に求める上限
pub const OPEN_MAX: u64 = 10240;

/// 使用数が soft のこの割合（%）以上になったら `tako check-health` が warning を出す
pub const WARN_PERCENT: u64 = 80;

/// A/B: 立っていれば上げない（修正前の挙動）
pub const LEGACY_ENV: &str = "TAKO_2359_LEGACY";

/// 注入: `setrlimit-fail` で引き上げの setrlimit を失敗させる（起動が続き記録が残るかの検証用）
pub const INJECT_ENV: &str = "TAKO_2359_INJECT";

/// 1 ペインあたりの GUI の fd（本番 28 ペインの lsof の内訳。PTY の master・kqueue・UnixStream の対）
pub const FDS_PER_PANE: u64 = 4;

/// この OS で上げる・数える対象か（macOS だけ。Windows は [`NOT_APPLICABLE_REASON`]）
pub const APPLICABLE: bool = cfg!(target_os = "macos");

/// 上限の仕組みが無い OS（Windows）で `tako check-health` が出す理由
pub const NOT_APPLICABLE_REASON: &str = "Windows のハンドルは 1 プロセス 2^24 本まで使え、\
     macOS の RLIMIT_NOFILE（Finder 起動の既定 256）に当たる小さい既定の上限が無い";

/// 起動時の引き上げの結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 引き上げた
    Raised,
    /// 起動時の soft が既に目標以上（上限を上げた端末から起こした等）なので変えない
    AlreadyEnough,
    /// hard が soft と同じで、これ以上は上げられない（`launchctl limit maxfiles 256 256` 等）
    HardCapped,
    /// getrlimit / setrlimit が失敗した（起動は続ける）
    Failed,
    /// A/B（[`LEGACY_ENV`]）で上げなかった
    Legacy,
    /// fd 上限の仕組みが無い OS（Windows）
    NotApplicable,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raised => "raised",
            Self::AlreadyEnough => "already_enough",
            Self::HardCapped => "hard_capped",
            Self::Failed => "failed",
            Self::Legacy => "legacy",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// 起動時の記録（[`raise_at_startup`] が 1 回置く）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub outcome: Outcome,
    /// 起動時の soft（= ペインの PTY の子へ渡す値）。読めなければ None
    pub soft_at_launch: Option<u64>,
    /// 記録した時点の soft（引き上げた後の値）
    pub soft: Option<u64>,
    /// hard（None = 無制限 か 読めない。[`Report::hard_unlimited`] で分ける）
    pub hard: Option<u64>,
    pub hard_unlimited: bool,
    /// 目指した soft（`min(hard, OPEN_MAX, kern.maxfilesperproc)`）
    pub target: Option<u64>,
    /// `kern.maxfilesperproc`（読めなければ None）
    pub max_per_proc: Option<u64>,
    /// 失敗の理由（[`Outcome::Failed`] のとき）
    pub error: Option<String>,
}

impl Report {
    fn not_applicable() -> Self {
        Self {
            outcome: Outcome::NotApplicable,
            soft_at_launch: None,
            soft: None,
            hard: None,
            hard_unlimited: false,
            target: None,
            max_per_proc: None,
            error: None,
        }
    }

    /// ペインの PTY の子へ渡す soft（戻さないなら None = 親と同じ値を受け継ぐ）
    pub fn children_soft(&self) -> Option<u64> {
        match self.outcome {
            Outcome::Raised => self.soft_at_launch,
            _ => self.soft,
        }
    }

    /// persist.log へ置く 1 行（中身に利用者のデータは無い）
    pub fn log_line(&self) -> String {
        let hard = hard_text(self.hard, self.hard_unlimited);
        let num = |v: Option<u64>| v.map_or_else(|| "?".to_string(), |n| n.to_string());
        let ceiling = format!(
            "hard {hard}・kern.maxfilesperproc {}・OPEN_MAX {OPEN_MAX}",
            num(self.max_per_proc)
        );
        match self.outcome {
            Outcome::Raised => format!(
                "fd の上限（RLIMIT_NOFILE）: soft {} → {} に引き上げた（{ceiling}）。\
                 ペインのシェル・tmux には起動時の {} を渡す（#2359）",
                num(self.soft_at_launch),
                num(self.soft),
                num(self.soft_at_launch),
            ),
            Outcome::AlreadyEnough => format!(
                "fd の上限（RLIMIT_NOFILE）: soft {} は目標 {} 以上なので変えない（{ceiling}）（#2359）",
                num(self.soft_at_launch),
                num(self.target),
            ),
            Outcome::HardCapped => format!(
                "fd の上限（RLIMIT_NOFILE）: hard が {hard} で soft {} から上げられない（#2359）",
                num(self.soft_at_launch),
            ),
            Outcome::Failed => format!(
                "fd の上限（RLIMIT_NOFILE）を引き上げられなかった: {}。soft {} のまま起動を続ける（{ceiling}）（#2359）",
                self.error.as_deref().unwrap_or("理由不明"),
                num(self.soft),
            ),
            Outcome::Legacy => format!(
                "fd の上限（RLIMIT_NOFILE）: {LEGACY_ENV} のため上げない（soft {}・hard {hard}）（#2359）",
                num(self.soft_at_launch),
            ),
            Outcome::NotApplicable => {
                "fd の上限（RLIMIT_NOFILE）: この OS には無いので上げない（#2359）".to_string()
            }
        }
    }
}

fn hard_text(hard: Option<u64>, unlimited: bool) -> String {
    match (hard, unlimited) {
        (_, true) => "無制限".to_string(),
        (Some(h), false) => h.to_string(),
        (None, false) => "?".to_string(),
    }
}

/// 目指す soft（純粋関数）。`hard` は None = 無制限
pub fn target(hard: Option<u64>, max_per_proc: Option<u64>) -> u64 {
    let mut t = OPEN_MAX;
    if let Some(h) = hard {
        t = t.min(h);
    }
    if let Some(m) = max_per_proc.filter(|m| *m > 0) {
        t = t.min(m);
    }
    t
}

/// 何をするか（純粋関数）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// soft をこの値へ上げる
    Raise(u64),
    /// 変えない（理由つき）
    Keep(Outcome),
}

pub fn plan(soft: u64, hard: Option<u64>, max_per_proc: Option<u64>) -> Plan {
    let want = target(hard, max_per_proc);
    if soft < want {
        return Plan::Raise(want);
    }
    // 目標に届いている。hard が目標を決めた（= hard が無ければもっと上げた）なら上げられない側
    let without_hard = target(None, max_per_proc);
    if hard.is_some_and(|h| h < without_hard) && soft < without_hard {
        Plan::Keep(Outcome::HardCapped)
    } else {
        Plan::Keep(Outcome::AlreadyEnough)
    }
}

/// 引き上げの 1 実装（純粋な骨組み）。`get` は (soft, hard。None = 無制限)、`set` は soft を書く
pub fn raise_with(
    legacy: bool,
    get: impl Fn() -> std::io::Result<(u64, Option<u64>)>,
    set: impl Fn(u64) -> std::io::Result<()>,
    max_per_proc: Option<u64>,
) -> Report {
    let (soft, hard) = match get() {
        Ok(v) => v,
        Err(e) => {
            return Report {
                outcome: Outcome::Failed,
                error: Some(format!("getrlimit: {e}")),
                max_per_proc,
                ..Report::not_applicable()
            }
        }
    };
    let mut report = Report {
        outcome: Outcome::AlreadyEnough,
        soft_at_launch: Some(soft),
        soft: Some(soft),
        hard,
        hard_unlimited: hard.is_none(),
        target: Some(target(hard, max_per_proc)),
        max_per_proc,
        error: None,
    };
    if legacy {
        report.outcome = Outcome::Legacy;
        return report;
    }
    match plan(soft, hard, max_per_proc) {
        Plan::Keep(outcome) => report.outcome = outcome,
        Plan::Raise(want) => match set(want) {
            Ok(()) => {
                // 書けた値は読み直す（黙って丸める実装があっても実際の値を記録する）
                report.soft = Some(get().map(|(s, _)| s).unwrap_or(want));
                report.outcome = Outcome::Raised;
            }
            Err(e) => {
                report.outcome = Outcome::Failed;
                report.error = Some(format!("setrlimit({want}): {e}"));
            }
        },
    }
    report
}

static STARTUP: OnceLock<Report> = OnceLock::new();

/// 起動時の記録（[`raise_at_startup`] を通っていなければ None = GUI 以外のプロセス）
pub fn startup_report() -> Option<&'static Report> {
    STARTUP.get()
}

/// 起動時に 1 回呼ぶ（GUI の main）。2 回目以降は最初の記録を返す
pub fn raise_at_startup() -> &'static Report {
    STARTUP.get_or_init(|| {
        let report = sys::raise();
        if report.outcome == Outcome::Raised {
            if let Some(soft) = report.soft_at_launch {
                child::arm(soft);
            }
        }
        report
    })
}

/// A/B（[`LEGACY_ENV`]）が立っているか
pub fn legacy() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| !v.is_empty())
}

/// いまの使用数と上限
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// 開いている fd の本数
    pub open: u64,
    pub soft: u64,
    /// None = 無制限
    pub hard: Option<u64>,
}

impl Usage {
    /// soft に対する使用率（%・切り捨て）
    pub fn percent(&self) -> u64 {
        if self.soft == 0 {
            return 100;
        }
        self.open.saturating_mul(100) / self.soft
    }

    /// 枯渇が近い（[`WARN_PERCENT`] 以上）
    pub fn near_exhaustion(&self) -> bool {
        self.open.saturating_mul(100) >= self.soft.saturating_mul(WARN_PERCENT)
    }

    /// 上限までの残り本数
    pub fn remaining(&self) -> u64 {
        self.soft.saturating_sub(self.open)
    }
}

/// いまの使用数と上限（読めない OS・失敗は None）
pub fn usage() -> Option<Usage> {
    sys::usage()
}

/// fork 後・exec 前の子で、soft を起動時の値へ戻す（引き上げていなければ何もしない）。
///
/// **async-signal-safe**（確保しない・ロックを取らない。原子変数を読み getrlimit / setrlimit を
/// 呼ぶだけ）。`fd_inherit` の fork の子ハンドラから呼ぶ。失敗しても子の起動は止めない
pub fn restore_for_child() {
    child::restore();
}

/// ペインの PTY の子へ戻す soft を直接構える（検証用に公開。0 で外す）。
/// 製品は [`raise_at_startup`] が引き上げたときだけ構える
#[doc(hidden)]
pub fn arm_children(soft: u64) {
    child::arm(soft);
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{raise_with, Report, Usage};

    fn get() -> std::io::Result<(u64, Option<u64>)> {
        let mut lim: libc::rlimit = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let hard = (lim.rlim_max != libc::RLIM_INFINITY).then_some(lim.rlim_max);
        Ok((lim.rlim_cur, hard))
    }

    fn set_soft(soft: u64) -> std::io::Result<()> {
        let mut lim: libc::rlimit = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        lim.rlim_cur = soft;
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn max_per_proc() -> Option<u64> {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        let rc = unsafe {
            libc::sysctlbyname(
                c"kern.maxfilesperproc".as_ptr(),
                (&mut value as *mut libc::c_int).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && value > 0).then_some(value as u64)
    }

    pub(super) fn raise() -> Report {
        let inject_fail = std::env::var(super::INJECT_ENV).is_ok_and(|v| v == "setrlimit-fail");
        raise_with(
            super::legacy(),
            get,
            |soft| {
                if inject_fail {
                    return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
                }
                set_soft(soft)
            },
            max_per_proc(),
        )
    }

    /// 開いている fd の本数（`proc_pidinfo(PROC_PIDLISTFDS)`。fd を 1 本も開かずに数える =
    /// 枯渇の直前でも読める）
    fn open_fds() -> Option<u64> {
        let entry = std::mem::size_of::<libc::proc_fdinfo>();
        let pid = unsafe { libc::getpid() };
        let hint =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        let mut cap = usize::try_from(hint).ok()? / entry + 64;
        // 数えている間に増えて入り切らなければ器を広げて読み直す
        for _ in 0..4 {
            let mut buf: Vec<libc::proc_fdinfo> = Vec::with_capacity(cap);
            let bytes = libc::c_int::try_from(cap * entry).ok()?;
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    buf.as_mut_ptr().cast(),
                    bytes,
                )
            };
            let n = usize::try_from(got).ok()? / entry;
            if got > 0 && n < cap {
                return Some(n as u64);
            }
            cap *= 2;
        }
        None
    }

    pub(super) fn usage() -> Option<Usage> {
        let (soft, hard) = get().ok()?;
        Some(Usage {
            open: open_fds()?,
            soft,
            hard,
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    use super::{Report, Usage};

    pub(super) fn raise() -> Report {
        Report::not_applicable()
    }

    pub(super) fn usage() -> Option<Usage> {
        None
    }
}

/// fork 後の子でも走る側（**async-signal-safe**。確保・ロック・出力を置かない。
/// 番犬 `issue1768_fd_inherit_watchdog.rs` がこのモジュールの字面を検査する）
#[cfg(target_os = "macos")]
mod child {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 子へ戻す soft（0 = 戻さない）
    static CHILD_SOFT: AtomicU64 = AtomicU64::new(0);

    pub(super) fn arm(soft: u64) {
        CHILD_SOFT.store(soft, Ordering::Release);
    }

    pub(super) fn restore() {
        let soft = CHILD_SOFT.load(Ordering::Acquire);
        if soft == 0 {
            return;
        }
        let mut lim: libc::rlimit = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 || lim.rlim_cur <= soft {
            return;
        }
        lim.rlim_cur = soft;
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
    }
}

#[cfg(not(target_os = "macos"))]
mod child {
    pub(super) fn arm(_soft: u64) {}

    pub(super) fn restore() {}
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// この機の `kern.maxfilesperproc`（macOS 26 の実測）
    const MPP: Option<u64> = Some(138_240);

    #[test]
    fn 目標はhardとopen_maxとmaxfilesperprocの小さい方() {
        assert_eq!(
            target(None, MPP),
            OPEN_MAX,
            "hard が無制限（launchd の既定）でも OPEN_MAX で止まる"
        );
        assert_eq!(target(Some(1000), MPP), 1000);
        assert_eq!(
            target(None, Some(4096)),
            4096,
            "kern.maxfilesperproc が小さい機（Go の issue 40564）"
        );
        assert_eq!(target(None, None), OPEN_MAX);
        assert_eq!(target(None, Some(0)), OPEN_MAX, "読めた値が 0 なら使わない");
    }

    #[test]
    fn finder起動の256は上げる() {
        assert_eq!(plan(256, None, MPP), Plan::Raise(OPEN_MAX));
    }

    #[test]
    fn hardがsoftと同じなら上げられない() {
        assert_eq!(plan(256, Some(256), MPP), Plan::Keep(Outcome::HardCapped));
        assert_eq!(plan(200, Some(200), MPP), Plan::Keep(Outcome::HardCapped));
    }

    #[test]
    fn hardが目標より小さくても上げられるところまで上げる() {
        assert_eq!(plan(256, Some(1000), MPP), Plan::Raise(1000));
    }

    #[test]
    fn 既に足りていれば下げない() {
        assert_eq!(
            plan(1_048_576, None, MPP),
            Plan::Keep(Outcome::AlreadyEnough),
            "上限を上げた端末から起こした（claude の中の 1048576）"
        );
        assert_eq!(
            plan(OPEN_MAX, Some(OPEN_MAX), MPP),
            Plan::Keep(Outcome::AlreadyEnough)
        );
    }

    #[test]
    fn 引き上げは書けた値を読み直して記録し子へは起動時の値を渡す() {
        let soft = Cell::new(256);
        let r = raise_with(
            false,
            || Ok((soft.get(), None)),
            |v| {
                soft.set(v);
                Ok(())
            },
            MPP,
        );
        assert_eq!(r.outcome, Outcome::Raised);
        assert_eq!(r.soft_at_launch, Some(256));
        assert_eq!(r.soft, Some(OPEN_MAX));
        assert_eq!(r.target, Some(OPEN_MAX));
        assert!(r.hard_unlimited);
        assert_eq!(r.children_soft(), Some(256), "子へは起動時の値");
        assert!(r.log_line().contains("soft 256 → 10240 に引き上げた"));
    }

    #[test]
    fn setrlimitが失敗しても理由を記録して元の値のまま続ける() {
        let r = raise_with(
            false,
            || Ok((256, None)),
            |_| Err(std::io::Error::from_raw_os_error(22)),
            MPP,
        );
        assert_eq!(r.outcome, Outcome::Failed);
        assert_eq!(r.soft, Some(256), "失敗したら起動時の値のまま");
        assert!(
            r.error
                .as_deref()
                .is_some_and(|e| e.starts_with("setrlimit(10240): ")),
            "{:?}",
            r.error
        );
        assert_eq!(r.children_soft(), Some(256));
        assert!(r.log_line().contains("引き上げられなかった"));
    }

    #[test]
    fn getrlimitが失敗したら値は不明のまま失敗を記録する() {
        let r = raise_with(
            false,
            || Err(std::io::Error::from_raw_os_error(22)),
            |_| panic!("読めないのに書かない"),
            MPP,
        );
        assert_eq!(r.outcome, Outcome::Failed);
        assert_eq!(r.soft_at_launch, None);
        assert_eq!(r.children_soft(), None);
        assert!(r
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("getrlimit: ")));
    }

    #[test]
    fn legacyは上げない() {
        let r = raise_with(
            true,
            || Ok((256, None)),
            |_| panic!("A/B では書かない"),
            MPP,
        );
        assert_eq!(r.outcome, Outcome::Legacy);
        assert_eq!(r.soft, Some(256));
        assert!(r.log_line().contains(LEGACY_ENV));
    }

    #[test]
    fn hardが同じなら書かずに記録する() {
        let r = raise_with(false, || Ok((256, Some(256))), |_| panic!("書かない"), MPP);
        assert_eq!(r.outcome, Outcome::HardCapped);
        assert!(!r.hard_unlimited);
        assert!(r
            .log_line()
            .contains("hard が 256 で soft 256 から上げられない"));
    }

    #[test]
    fn 使用率は80パーセントから枯渇が近い() {
        let u = |open, soft| Usage {
            open,
            soft,
            hard: None,
        };
        assert!(!u(204, 256).near_exhaustion(), "79.6%");
        assert!(u(205, 256).near_exhaustion(), "80.1%");
        assert_eq!(u(121, 256).percent(), 47, "本番の実測（28 ペイン）");
        assert_eq!(u(121, 256).remaining(), 135);
        assert_eq!(u(300, 256).remaining(), 0, "超えていても負にならない");
        assert!(u(0, 0).near_exhaustion(), "soft 0 は枯渇とみなす");
        assert_eq!(u(0, 0).percent(), 100);
    }

    #[test]
    fn 記録の1行は結果ごとに組める() {
        let base = raise_with(false, || Ok((256, None)), |_| Ok(()), MPP);
        for outcome in [
            Outcome::Raised,
            Outcome::AlreadyEnough,
            Outcome::HardCapped,
            Outcome::Failed,
            Outcome::Legacy,
            Outcome::NotApplicable,
        ] {
            let line = Report {
                outcome,
                ..base.clone()
            }
            .log_line();
            assert!(line.ends_with("（#2359）"), "{outcome:?}: {line}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn 使用数は自分の開いたfdを数える() {
        const OPENED: u64 = 200;
        // CI の soft が 256 でも開き切れるよう、足りなければ hard の範囲で上げる（下げない）
        let mut lim: libc::rlimit = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
        if lim.rlim_cur < 1024 && lim.rlim_max > lim.rlim_cur {
            lim.rlim_cur = lim.rlim_max.min(1024);
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
        }
        let before = usage().expect("macOS では読める");
        let opened: Vec<i32> = (0..OPENED)
            .map(|_| unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) })
            .collect();
        let after = usage().expect("macOS では読める");
        for fd in &opened {
            unsafe { libc::close(*fd) };
        }
        assert!(opened.iter().all(|fd| *fd >= 0), "{OPENED} 本とも開ける");
        // 並走するテストの開け閉めは 50 本までの揺れとして許す（数え漏れなら増分が 0 に近い）
        assert!(
            after.open >= before.open + OPENED - 50,
            "before={before:?} after={after:?}"
        );
        assert!(after.soft > 0);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn windowsでは上げず使用数も読まない() {
        assert_eq!(raise_at_startup().outcome, Outcome::NotApplicable);
        assert_eq!(usage(), None);
    }
}
