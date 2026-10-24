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
//! ペインの PTY の子（シェル・エージェント・tmux の attach クライアント）は、fork の子ハンドラで
//! **起動時の soft へ戻してから** exec する（[`restore_for_child`]。構えるのは `fd_inherit::spawn_sealed`。
//! 器の attach クライアントだけは下の #2367 の節のとおり戻さない）。ユーザーのシェル・エージェントの
//! `ulimit -n` は修正前と同じ（Finder 起動なら Terminal.app と同じ 256）で、FD_SETSIZE（1024）を
//! 超えた fd を select(2) に渡す古いプログラム・soft の大きさで表を確保するプログラムの振る舞いを
//! 変えない（Ghostty #4232 と同じ判断）。tako が自分で起こす裏方（言語サーバ・git・`remote serve` の
//! daemon）は引き上げた値を受け継ぐ。
//!
//! ## 器の tmux サーバーには引き上げた値を渡す（#2367）
//!
//! 器の tmux サーバーは attach クライアント（ペインの PTY の子）が起こすので、上のとおり戻すと
//! サーバーも起動時の soft（Finder 起動なら 256）で動く。サーバーは器の 1 ペインで fd を 3 本
//! （ペインの PTY の master・attach クライアントのソケットとその tty）使い、上限に届くと新しいペインは
//! `create window failed: fork failed: Too many open files` で作れず、器への問い合わせ（画面の読み取り・
//! 生死の確認）もすべて `server exited unexpectedly` で返らなくなる（tmux 3.6b の実測。tmux は
//! 自分の上限を変えず、ペインの子へもそのまま渡す = `spawn.c` は rlimit に触れない）。
//!
//! そこで器の attach クライアントだけは戻さず（[`with_children_raised`]。サーバーは引き上げた値を
//! 受け継ぐ）、器の中のペインの子は**この実行ファイル自身を通して**起こす（[`container_child_argv`]）。
//! tmux は残余引数が 2 個以上なら `execvp` で直に起こすので、その段（[`run_container_exec_if_requested`]）
//! が soft を起動時の値へ下げてから、tmux が自分で起こすはずだった形（`$SHELL` を `-zsh` の名で
//! ログイン起動 / `$SHELL -c <コマンド>`）を exec し直す。ユーザーのシェルの `ulimit -n` は修正前と同じ。
//! 段を通らないなら（A/B [`CONTAINER_LEGACY_ENV`]・引き上げていない・段を持たないプロセス）
//! クライアントも戻す = 修正前と同じ。**既に走っているサーバーの上限は変わらない**（上がるのは
//! 次にサーバーが起きたとき）。
//!
//! ## Windows は対象外
//!
//! Windows のハンドルは 1 プロセス 2^24 本まで使え、既定の小さい上限が無い。Rust の std はファイル・
//! ソケット・パイプを CRT の fd ではなく HANDLE / SOCKET で扱うので、CRT の `_setmaxstdio`（既定 512）
//! にも当たらない。[`raise_at_startup`] は [`Outcome::NotApplicable`] を返し、使用数も読まない。
//! 器（psmux）の上限も同じ理由で対象外（[`container_child_argv`] は常に None）。

use std::path::{Path, PathBuf};
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

/// A/B（#2367）: 立っていれば器の tmux サーバーへ引き上げた値を渡さない（修正前 = 起動時の値で起きる）
pub const CONTAINER_LEGACY_ENV: &str = "TAKO_2367_LEGACY";

/// 器の中のペインの子を起こす段の印（argv[1]。#2367）。tmux の残余引数の 2 語目に置く
pub const CONTAINER_EXEC_ARG: &str = "__tako-container-exec";

/// 器の tmux サーバーが 1 ペインに使う fd（ペインの PTY の master・attach クライアントの
/// ソケットとその tty。tmux 3.6b の lsof の実測。本番 28 ペインで 90 本 = 基礎 6 + 3 × 28）
pub const CONTAINER_FDS_PER_PANE: u64 = 3;

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

/// 器の中のペインの子を起こす段に使う実行ファイルと、置いたときのファイルの同一性（dev, inode）
struct ContainerExec {
    path: PathBuf,
    identity: Option<(u64, u64)>,
}

/// [`register_container_exec`] が置く
static CONTAINER_EXEC: OnceLock<ContainerExec> = OnceLock::new();

fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// 器の中のペインの子を起こす段に使う実行ファイルを置く（#2367。GUI の main が自分を渡す）。
///
/// 段が動くのは、その実行ファイルの main が [`run_container_exec_if_requested`] を呼ぶときだけ
/// なので、渡すのは**そう呼ぶ実行ファイル**に限る。置かないプロセス（テスト・CLI）は器の
/// tmux サーバーへ引き上げた値を渡さない = 修正前と同じ
pub fn register_container_exec(program: PathBuf) {
    let identity = file_identity(&program);
    let _ = CONTAINER_EXEC.set(ContainerExec {
        path: program,
        identity,
    });
}

/// 段に使ってよい実行ファイル。置いたときと同じファイル（dev, inode）がまだそこにあるときだけ返す:
/// 走っている間に置き換わった実行ファイル（更新・古い版へ戻した `.app`・再ビルド）は段の印を
/// 知らないかもしれず、知らない版の GUI は印を無視して**窓を開いてしまう**ので使わない
/// （その間に起こす器のペインは修正前の形 = 段を通さない）
fn container_exec_now() -> Option<&'static Path> {
    let exec = CONTAINER_EXEC.get()?;
    unchanged(exec.identity, file_identity(&exec.path)).then_some(exec.path.as_path())
}

/// 置いたときの同一性と今の同一性が同じか（どちらかが読めなければ偽 = 段を使わない側）
fn unchanged(registered: Option<(u64, u64)>, now: Option<(u64, u64)>) -> bool {
    registered.is_some() && registered == now
}

/// A/B（[`CONTAINER_LEGACY_ENV`]）が立っているか。呼ぶたびに読む（同じプロセスで A/B を並べられる）
pub fn container_legacy() -> bool {
    std::env::var(CONTAINER_LEGACY_ENV).is_ok_and(|v| !v.is_empty())
}

/// 器（tmux）の `new-session` へ渡す残余引数（#2367）。`inner` は器の中で動かすコマンド 1 本
/// （`backend::inner_command_line` が組んだもの。None = 既定のログインシェル）。
///
/// None なら段を通さない（呼び出し側は従来どおり `inner` を 1 引数で渡す）: 起動時に引き上げて
/// いない（[`raise_at_startup`] の結果が raised でない）・A/B・段の実行ファイルが置かれていない
/// か置き換わった（[`container_exec_now`]）・Windows（引き上げない OS）
pub fn container_child_argv(inner: Option<&str>) -> Option<Vec<String>> {
    container_child_argv_with(
        container_exec_now(),
        child::armed_soft(),
        container_legacy(),
        inner,
    )
}

/// [`container_child_argv`] の判断部（純粋関数）。`children_soft` は器の中のペインの子へ戻す soft
pub fn container_child_argv_with(
    program: Option<&Path>,
    children_soft: Option<u64>,
    legacy: bool,
    inner: Option<&str>,
) -> Option<Vec<String>> {
    if legacy {
        return None;
    }
    let program = program?.to_str()?;
    let soft = children_soft.filter(|s| *s > 0)?;
    let mut argv = vec![
        program.to_string(),
        CONTAINER_EXEC_ARG.to_string(),
        soft.to_string(),
    ];
    argv.extend(inner.map(str::to_string));
    Some(argv)
}

/// 器の attach クライアントの引数 `args` が、器の中のペインの子を段に通すか
/// （[`container_child_argv`] の並びを含むか）。**通すときだけ**クライアントへ引き上げた値を
/// 渡してよい（[`with_children_raised`]）= 段を通らないシェルへ引き上げた値が届く形を作らない
pub fn routes_container_children(args: &[String]) -> bool {
    routes_container_children_with(CONTAINER_EXEC.get().and_then(|e| e.path.to_str()), args)
}

/// [`routes_container_children`] の判断部（純粋関数）
pub fn routes_container_children_with(program: Option<&str>, args: &[String]) -> bool {
    let Some(program) = program else {
        return false;
    };
    args.windows(2)
        .any(|w| w[0] == program && w[1] == CONTAINER_EXEC_ARG)
}

/// `spawn` のあいだに起こす PTY の子へ、起動時の値へ戻さずに引き上げた値を渡す（`keep` が偽なら
/// そのまま呼ぶ）。器の attach クライアント用（#2367。それが起こす器の tmux サーバーが受け継ぐ）。
///
/// `fd_inherit::spawn_sealed` の**内側**で呼ぶ: 構えはそこで 1 スレッドずつに直列化され、
/// 戻すかどうかを決めるのはそのスレッドが起こした fork の子だけなので、ほかの spawn には効かない
pub fn with_children_raised<T>(keep: bool, spawn: impl FnOnce() -> T) -> T {
    if !keep {
        return spawn();
    }
    /// `spawn` が panic しても構えを解く
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            child::keep_raised(false);
        }
    }
    child::keep_raised(true);
    let _reset = Reset;
    spawn()
}

/// 器の中のペインの子の段が exec するもの（[`container_exec_plan`]）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecPlan {
    pub program: std::ffi::OsString,
    /// 子の argv[0]（ログインシェルは `-<名前>`）
    pub arg0: std::ffi::OsString,
    pub args: Vec<std::ffi::OsString>,
}

/// tmux 3.6b の `spawn.c` が自分で起こすはずだった形（純粋関数）。
///
/// `shell` は tmux が子の env に置いた `SHELL`（= `default-shell`。tmux が実行できると確かめた値。
/// 無ければ tmux と同じ `/bin/sh`）、`inner` は `-c` で渡すコマンド。None はログインシェル
/// （argv[0] を `-<名前>` にする = tmux の `execl(shell, "-zsh", NULL)` と同じ）
pub fn container_exec_plan(
    shell: Option<&std::ffi::OsStr>,
    inner: Option<&std::ffi::OsStr>,
) -> ExecPlan {
    use std::ffi::{OsStr, OsString};
    let shell = shell
        .filter(|s| !s.is_empty())
        .unwrap_or(OsStr::new("/bin/sh"));
    let name = Path::new(shell)
        .file_name()
        .filter(|n| !n.is_empty())
        .unwrap_or(shell);
    match inner {
        None => {
            let mut arg0 = OsString::from("-");
            arg0.push(name);
            ExecPlan {
                program: shell.to_os_string(),
                arg0,
                args: Vec::new(),
            }
        }
        Some(command) => ExecPlan {
            program: shell.to_os_string(),
            arg0: name.to_os_string(),
            args: vec![OsString::from("-c"), command.to_os_string()],
        },
    }
}

/// 器の中のペインの子の段（#2367）。argv[1] が [`CONTAINER_EXEC_ARG`] のときだけ動き、soft を
/// argv[2] まで下げて（上げない・hard は触らない）[`container_exec_plan`] を exec する
/// = 成功すれば戻らない。印が無ければ None（いつもの main を続ける）。exec に失敗したら理由を
/// 標準エラー（= ペインの画面）へ出し、tmux の子と同じ終了コード 1 を返す。
///
/// GUI / CLI の main の 2 文目（`mark_product_process` の直後・ほかの初期化より前）で
/// [`exec_container_child_if_requested`] を通して呼ぶ
pub fn run_container_exec_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != CONTAINER_EXEC_ARG {
        return None;
    }
    let soft = args
        .next()
        .and_then(|s| s.to_str().and_then(|s| s.parse::<u64>().ok()));
    let inner = args.next();
    Some(sys::container_exec(soft, inner))
}

/// [`run_container_exec_if_requested`] を main の 1 文で呼ぶ形。段として起きたなら exec し（戻らない）、
/// exec に失敗したらその終了コードでプロセスを終える。印が無ければ何もしない
pub fn exec_container_child_if_requested() {
    if let Some(code) = run_container_exec_if_requested() {
        std::process::exit(code);
    }
}

/// `pid`（同じユーザーのプロセス）が開いている fd の本数。読めない・Windows は None
pub fn open_fds_of(pid: u32) -> Option<u64> {
    sys::open_fds_of(pid)
}

/// 器の tmux サーバーの上限がどう決まったか（#2367。check-health と persist.log が同じ 1 実装で読む）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerLimit {
    /// 器のペインのシェルへ渡す起動時の値より大きい（GUI が引き上げた値を受け継いだ）
    Raised,
    /// GUI は引き上げたが、サーバーは起動時の値のまま = 引き上げより前（前の tako・A/B の起動）に
    /// 起きたサーバー。上がるのは次にサーバーが起きたとき（器のセッションがすべて終わった後）
    Predates,
    /// A/B（[`CONTAINER_LEGACY_ENV`]）で渡していない
    Legacy,
    /// GUI 自身が起動時に引き上げていない（hard が低い・失敗・既に十分）ので起動時の値のまま
    LaunchValue,
}

impl ContainerLimit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raised => "raised",
            Self::Predates => "predates",
            Self::Legacy => "legacy",
            Self::LaunchValue => "launch_value",
        }
    }
}

/// 器の tmux サーバーの soft から、上限がどう決まったかを読む（純粋関数）
pub fn classify_container(
    server_soft: u64,
    startup: Option<&Report>,
    legacy: bool,
) -> ContainerLimit {
    let raised = startup.filter(|r| r.outcome == Outcome::Raised);
    match raised.and_then(Report::children_soft) {
        Some(children) if server_soft > children => ContainerLimit::Raised,
        Some(_) if legacy => ContainerLimit::Legacy,
        Some(_) => ContainerLimit::Predates,
        None => ContainerLimit::LaunchValue,
    }
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
    /// 枯渇の直前でも読める。同じユーザーの別プロセスも読める = 器の tmux サーバー。#2367）
    fn open_fds(pid: libc::pid_t) -> Option<u64> {
        let entry = std::mem::size_of::<libc::proc_fdinfo>();
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
            open: open_fds(unsafe { libc::getpid() })?,
            soft,
            hard,
        })
    }

    pub(super) fn open_fds_of(pid: u32) -> Option<u64> {
        open_fds(libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)?)
    }

    /// 器の中のペインの子の段の本体（#2367。exec の前の子ではなく、exec された段のプロセスで走る
    /// = 確保してよい）
    pub(super) fn container_exec(soft: Option<u64>, inner: Option<std::ffi::OsString>) -> i32 {
        use std::os::unix::process::CommandExt;
        if let Some(soft) = soft {
            // 上げない（サーバーが起動時の値より小さい soft で起きていたらそのまま）・hard は触らない
            if let Ok((current, _)) = get() {
                if current > soft {
                    let _ = set_soft(soft);
                }
            }
        }
        // 段の起動の途中で開いた fd（フレームワークの初期化）をシェルへ渡さない。tmux は exec の前に
        // 3 以上を閉じている（`closefrom`）ので、シェルが受け取るのは 0 / 1 / 2 だけ = それに揃える
        crate::platform::fd_inherit::seal_inherited_fds();
        let plan =
            super::container_exec_plan(std::env::var_os("SHELL").as_deref(), inner.as_deref());
        let mut command = std::process::Command::new(&plan.program);
        // 子プロセス起動の境界の作法（#628。macOS では何もしない）
        crate::platform::process::no_console_window(&mut command);
        let err = command.arg0(&plan.arg0).args(&plan.args).exec();
        eprintln!(
            "tako: {}: {err}（#2367）",
            std::path::Path::new(&plan.program).display()
        );
        1
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

    pub(super) fn open_fds_of(_pid: u32) -> Option<u64> {
        None
    }

    /// 引き上げない OS には段が無い（[`super::container_child_argv`] が印を置かない）
    pub(super) fn container_exec(_soft: Option<u64>, _inner: Option<std::ffi::OsString>) -> i32 {
        eprintln!(
            "tako: {} はこの OS では使えない（#2367）",
            super::CONTAINER_EXEC_ARG
        );
        1
    }
}

/// fork 後の子でも走る側（**async-signal-safe**。確保・ロック・出力を置かない。
/// 番犬 `issue1768_fd_inherit_watchdog.rs` がこのモジュールの字面を検査する）
#[cfg(target_os = "macos")]
mod child {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// 子へ戻す soft（0 = 戻さない）
    static CHILD_SOFT: AtomicU64 = AtomicU64::new(0);

    /// 立っているあいだに起こす子は戻さない（器の attach クライアント。#2367。
    /// 構えるのは `with_children_raised` だけ）
    static KEEP_RAISED: AtomicBool = AtomicBool::new(false);

    pub(super) fn arm(soft: u64) {
        CHILD_SOFT.store(soft, Ordering::Release);
    }

    /// 子へ戻す soft（構えていなければ None。親だけが読む）
    pub(super) fn armed_soft() -> Option<u64> {
        let soft = CHILD_SOFT.load(Ordering::Acquire);
        (soft != 0).then_some(soft)
    }

    pub(super) fn keep_raised(on: bool) {
        KEEP_RAISED.store(on, Ordering::Release);
    }

    pub(super) fn restore() {
        let soft = CHILD_SOFT.load(Ordering::Acquire);
        if soft == 0 || KEEP_RAISED.load(Ordering::Acquire) {
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

    pub(super) fn armed_soft() -> Option<u64> {
        None
    }

    pub(super) fn keep_raised(_on: bool) {}

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

    fn raised_256() -> Report {
        let soft = Cell::new(256);
        raise_with(
            false,
            || Ok((soft.get(), None)),
            |v| {
                soft.set(v);
                Ok(())
            },
            MPP,
        )
    }

    #[test]
    fn 器の中の子の段はログインシェルとコマンドで並びが違う() {
        let exe = Path::new("/Applications/tako.app/Contents/MacOS/tako-app");
        assert_eq!(
            container_child_argv_with(Some(exe), Some(256), false, None),
            Some(vec![
                exe.display().to_string(),
                CONTAINER_EXEC_ARG.to_string(),
                "256".to_string(),
            ]),
            "既定のログインシェル = 3 語（tmux は 2 語以上を execvp で直に起こす）"
        );
        assert_eq!(
            container_child_argv_with(
                Some(exe),
                Some(256),
                false,
                Some("'/bin/zsh' '-l' '-c' 'claude'")
            ),
            Some(vec![
                exe.display().to_string(),
                CONTAINER_EXEC_ARG.to_string(),
                "256".to_string(),
                "'/bin/zsh' '-l' '-c' 'claude'".to_string(),
            ]),
            "明示コマンドは 4 語目に 1 本のまま渡す（段が $SHELL -c へ渡す）"
        );
    }

    #[test]
    fn 器の中の子の段を通さない条件() {
        let exe = Path::new("/x/tako-app");
        assert_eq!(
            container_child_argv_with(Some(exe), Some(256), true, None),
            None,
            "A/B"
        );
        assert_eq!(
            container_child_argv_with(None, Some(256), false, None),
            None,
            "段の実行ファイルが置かれていない（テスト・CLI）"
        );
        assert_eq!(
            container_child_argv_with(Some(exe), None, false, None),
            None,
            "引き上げていない（子へ戻す値が無い）"
        );
        assert_eq!(
            container_child_argv_with(Some(exe), Some(0), false, None),
            None
        );
    }

    #[test]
    fn 段を通す並びのときだけクライアントへ引き上げた値を渡す() {
        let exe = "/x/tako-app";
        let wrapped: Vec<String> = [
            "-u",
            "-L",
            "tako",
            "-f",
            "/tmp/tmux-backend.conf",
            "new-session",
            "-A",
            "-D",
            "-s",
            "tako-1",
            exe,
            CONTAINER_EXEC_ARG,
            "256",
        ]
        .map(String::from)
        .to_vec();
        assert!(routes_container_children_with(Some(exe), &wrapped));
        assert!(
            !routes_container_children_with(None, &wrapped),
            "段を置いていないプロセスは渡さない"
        );
        assert!(
            !routes_container_children_with(Some("/y/other"), &wrapped),
            "置いた実行ファイルと違う並びは段とみなさない"
        );
        let plain: Vec<String> = [
            "-u",
            "-L",
            "tako",
            "-f",
            "/tmp/tmux-backend.conf",
            "new-session",
            "-A",
            "-D",
            "-s",
            "tako-1",
        ]
        .map(String::from)
        .to_vec();
        assert!(
            !routes_container_children_with(Some(exe), &plain),
            "修正前の並び"
        );
        let reattach: Vec<String> = ["-u", "-L", "tako", "attach-session", "-t", "=tako-1"]
            .map(String::from)
            .to_vec();
        assert!(!routes_container_children_with(Some(exe), &reattach));
    }

    #[test]
    fn 段はtmuxが起こすはずだった形をexecする() {
        use std::ffi::{OsStr, OsString};
        let login = container_exec_plan(Some(OsStr::new("/bin/zsh")), None);
        assert_eq!(login.program, OsString::from("/bin/zsh"));
        assert_eq!(
            login.arg0,
            OsString::from("-zsh"),
            "tmux と同じく -<名前> でログイン起動"
        );
        assert!(login.args.is_empty());
        let fish = container_exec_plan(Some(OsStr::new("/opt/homebrew/bin/fish")), None);
        assert_eq!(fish.arg0, OsString::from("-fish"));
        let command =
            container_exec_plan(Some(OsStr::new("/bin/zsh")), Some(OsStr::new("echo hi")));
        assert_eq!(
            command.arg0,
            OsString::from("zsh"),
            "-c のときは名前だけ（tmux と同じ）"
        );
        assert_eq!(
            command.args,
            vec![OsString::from("-c"), OsString::from("echo hi")]
        );
        let missing = container_exec_plan(None, None);
        assert_eq!(
            missing.program,
            OsString::from("/bin/sh"),
            "SHELL が無ければ tmux と同じ /bin/sh"
        );
        assert_eq!(missing.arg0, OsString::from("-sh"));
        assert_eq!(
            container_exec_plan(Some(OsStr::new("")), None).program,
            OsString::from("/bin/sh")
        );
    }

    #[test]
    fn 器のサーバーの上限がどう決まったかを読む() {
        let raised = raised_256();
        assert_eq!(
            classify_container(OPEN_MAX, Some(&raised), false),
            ContainerLimit::Raised
        );
        assert_eq!(
            classify_container(256, Some(&raised), false),
            ContainerLimit::Predates,
            "GUI は上げたのにサーバーは起動時の値 = 前から居たサーバー"
        );
        assert_eq!(
            classify_container(256, Some(&raised), true),
            ContainerLimit::Legacy
        );
        let capped = raise_with(false, || Ok((256, Some(256))), |_| panic!("書かない"), MPP);
        assert_eq!(
            classify_container(256, Some(&capped), false),
            ContainerLimit::LaunchValue,
            "hard が soft と同じで上げられない"
        );
        assert_eq!(
            classify_container(256, None, false),
            ContainerLimit::LaunchValue
        );
        for (limit, s) in [
            (ContainerLimit::Raised, "raised"),
            (ContainerLimit::Predates, "predates"),
            (ContainerLimit::Legacy, "legacy"),
            (ContainerLimit::LaunchValue, "launch_value"),
        ] {
            assert_eq!(limit.as_str(), s);
        }
    }

    #[cfg(unix)]
    #[test]
    fn 置き換わった実行ファイルは段に使わない() {
        let dir = std::env::temp_dir().join(format!("tako-fdlimit-2367-{}", std::process::id()));
        assert!(dir.starts_with(std::env::temp_dir()) && dir != std::env::temp_dir());
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("tako-app");
        std::fs::write(&exe, b"v1").unwrap();
        let registered = file_identity(&exe);
        assert!(
            unchanged(registered, file_identity(&exe)),
            "同じファイルなら使う"
        );
        // 更新と同じ形（別のファイルを書いて rename で差し替える）
        let next = dir.join("tako-app.new");
        std::fs::write(&next, b"v2").unwrap();
        std::fs::rename(&next, &exe).unwrap();
        assert!(
            !unchanged(registered, file_identity(&exe)),
            "置き換わった（inode が変わった）なら使わない"
        );
        std::fs::remove_file(&exe).unwrap();
        assert!(
            !unchanged(registered, file_identity(&exe)),
            "消えたなら使わない"
        );
        assert!(!unchanged(None, None), "置いたときに読めなければ使わない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 段の印が無ければいつものmainを続ける() {
        // テストのプロセスの argv[1] は段の印ではない
        assert_eq!(run_container_exec_if_requested(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn 別のプロセスの使用数も数える() {
        let mine = open_fds_of(std::process::id()).expect("自分は読める");
        assert!(mine >= 3, "{mine}");
        assert_eq!(open_fds_of(0), None);
        assert_eq!(open_fds_of(u32::MAX), None, "居ない pid");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn windowsでは上げず使用数も読まない() {
        assert_eq!(raise_at_startup().outcome, Outcome::NotApplicable);
        assert_eq!(usage(), None);
        // #2367: 器（psmux）の中の子も段を通さない
        assert_eq!(container_child_argv(None), None);
        assert_eq!(open_fds_of(std::process::id()), None);
    }
}
