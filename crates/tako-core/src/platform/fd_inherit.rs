//! 子プロセスへ GUI の fd を渡さない（抽象境界 B27。Issue #1768）
//!
//! ## 何が起きていたか
//!
//! ペインの PTY の子（シェル・tmux クライアント・エージェント）は、GUI が
//! **CLOEXEC 無しで開いたままの fd** をそのまま受け継いでいた。実測（隔離 GUI・
//! 3000 回）では、Metal のシェーダキャッシュ（`com.apple.metal/…/functions*.data` /
//! `.list`。Apple のフレームワークが開く）が**全ペインの fd 4 / 5 に 100%** 入っていた。
//! 本番の tmux クライアント 23 本でも同じだった。子の中のスクリプトが開かずに
//! `>&4` と書けば GUI のキャッシュへ書き込めるし、ペインから起きた長生きの
//! プロセスがキャッシュのファイルを握り続ける。
//!
//! 入口が PTY に限られるのは、alacritty の PTY の起動が `pre_exec` を持つ
//! `Command` = **fork + exec** で、fork の瞬間の fd 表がまるごと子へ写るから。
//! 同じ理由で `remote serve` の daemon（`pre_exec` で `setsid`）にも入る。
//!
//! ## どう直すか: fork 後・exec 前の子の中で CLOEXEC を立てる
//!
//! [`seal_inherited_fds`] は、自分の fd 3 以上のうち CLOEXEC が無いものへ
//! `FD_CLOEXEC` を立てる。**子の中で**呼べば、exec を越えて残るのは 0 / 1 / 2 だけになる。
//!
//! - **daemon**: 既存の `pre_exec` の中から呼ぶ（`remote.rs` の `configure_daemon_child`）
//! - **PTY**: alacritty の `Command` には `pre_exec` を足す口が無いので、
//!   [`spawn_sealed`] で `tty::new` を包む。包んでいるあいだだけ `pthread_atfork` の
//!   子ハンドラが構え、**そのスレッドが起こした fork の子**で掃除を走らせる
//!   （fork は libc の `fork()` なので atfork の子ハンドラが std の子側の処理より先に走る）
//!
//! ### 親で掃く案（fork の直前に親の fd へ立てる）を採らなかった理由
//!
//! 親で掃く案は fork 子で動くコードもプロセス全体のハンドラも要らず単純だが、
//! **掃いてから fork までに別スレッドが CLOEXEC 無しで開いた fd** が残る。
//! macOS の std は `Stdio::piped()` のパイプを `pipe()` → `set_cloexec` の 2 手で作るので、
//! その 2 手のあいだに fork が当たれば入る。実測（隔離 GUI・GitLog の並行負荷・3000 回）で、
//! 親で掃く版は Metal の漏れを 3000 → 0 に塞いだが、**GUI が相方を握るパイプが 1 回入った**。
//! 子の中で掃けば、いつ開いた fd でも exec の前に閉じる側へ倒れる。
//!
//! **close ではなく CLOEXEC にする**: fork 後の子で閉じると、std が exec の失敗を
//! 親へ返すための error pipe（CLOEXEC 付き）まで閉じてしまい、exec の失敗が
//! 「成功した」に化ける。CLOEXEC なら exec の瞬間にだけ閉じ、それまでは生きている。
//! 親の fd は書き換えない（掃除は子の fd 表の写しにだけ効く）。
//!
//! ## async-signal-safe で書く
//!
//! 子の中（多スレッドのプロセスを fork した直後）で走るので、**確保しない・ロックを
//! 取らない**。列挙の器はスタックの固定長配列で、使うのはシステムコールの薄い包み
//! （`proc_pidinfo` / `fcntl` / `getrlimit` / `pthread_self`）だけ。ロックと `Once` は
//! 親だけで走る `mod arming` に閉じる。番犬 `issue1768_fd_inherit_watchdog.rs` が
//! 子で走る側に確保・ロックの字面が生えたら名指す。
//!
//! ## Windows は掃かない
//!
//! Windows のハンドルは既定で継承されない。PTY（alacritty の ConPTY）は
//! `CreateProcessW` に `bInheritHandles = FALSE` を渡し、std の `Command` も
//! 子へ渡す stdio だけを継承可能にして起こす。したがって [`seals`] は
//! Windows で偽を返し、掃除は何もしない（macOS の単体テストからも両 OS の判断を検査できる）。

use super::support::Platform;

/// 掃く fd の下限（0 / 1 / 2 は子の stdio なので触らない）
pub const FIRST_SEALED_FD: i32 = 3;

/// その OS で子へ fd が漏れうる（= 掃く必要がある）か
pub fn seals(platform: Platform) -> bool {
    match platform {
        // fork + exec の子は CLOEXEC の無い fd をすべて受け継ぐ
        Platform::MacOs => true,
        // ハンドルは既定で継承されない（ConPTY は bInheritHandles = FALSE）
        Platform::Windows => false,
    }
}

/// 自分の fd 3 以上のうち CLOEXEC が無いものへ `FD_CLOEXEC` を立て、立てた本数を返す。
///
/// **async-signal-safe**（確保しない・ロックを取らない）なので、`pre_exec`
/// （fork 後・exec 前の子）の中から呼べる。Windows では何もしない（[`seals`]）
pub fn seal_inherited_fds() -> usize {
    if !seals(Platform::current()) {
        return 0;
    }
    sys::seal_all()
}

/// `spawn` の中でこのスレッドが起こした fork の子で、exec の前に
/// [`seal_inherited_fds`] を走らせる（`pre_exec` を足せない PTY の起動用）。
///
/// 構えるのは `spawn` のあいだだけで、ほかのスレッドの fork には効かない。
/// Windows ではそのまま `spawn` を呼ぶ（[`seals`]）
pub fn spawn_sealed<T>(spawn: impl FnOnce() -> T) -> T {
    if !seals(Platform::current()) {
        return spawn();
    }
    arming::spawn_sealed(spawn)
}

/// [`seal_inherited_fds`] の逃げ道（列挙できないときの総当たり）を直接走らせる。
/// 検証用に公開している（逃げ道は fd が 4096 本を超えないと通らないので、
/// ふだんの経路からは検査できない）
#[doc(hidden)]
pub fn seal_by_scan() -> usize {
    if !seals(Platform::current()) {
        return 0;
    }
    sys::scan_all()
}

/// CLOEXEC を立てるべきか（`flags` は `fcntl(F_GETFD)` の戻り値。負 = 開いていない）
pub fn needs_seal(fd: i32, flags: i32, cloexec: i32) -> bool {
    fd >= FIRST_SEALED_FD && flags >= 0 && flags & cloexec == 0
}

/// 子の中で `fds` が開いているかを調べて `FDCHK=[…]` を 1 行出す `/bin/sh` の片
/// （検証用。`{ : >&N; }` は N が開いていなければ失敗する。読み取り専用の fd でも
/// dup は通るので向きを問わない）
pub fn inherited_probe_script(fds: &[i32]) -> String {
    let list = fds
        .iter()
        .map(|fd| fd.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "open=''; for n in {list}; do if {{ : >&$n; }} 2>/dev/null; then open=\"$open $n\"; fi; done; \
         echo \"FDCHK=[${{open# }}]\""
    )
}

/// [`inherited_probe_script`] の出力から、子の中で開いていた fd を拾う（行が無ければ None）
pub fn observed_inherited(screen: &str) -> Option<Vec<i32>> {
    let line = screen.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("FDCHK=[")?;
        rest.strip_suffix(']')
    })?;
    Some(
        line.split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect(),
    )
}

/// fork 後の子でも走る側（**async-signal-safe**。確保・ロック・出力を置かない）
#[cfg(unix)]
mod sys {
    use std::sync::atomic::AtomicUsize;

    use super::{needs_seal, FIRST_SEALED_FD};

    /// 掃除を構えているスレッド（`pthread_self`。0 = 構えていない）
    pub(super) static ARMED: AtomicUsize = AtomicUsize::new(0);

    /// 列挙の器（スタックの固定長。`proc_fdinfo` 1 件 = 8 バイトで 32 KiB）
    #[cfg(target_os = "macos")]
    const LISTED_MAX: usize = 4096;

    /// 総当たりの上限（fd 上限が桁違いに大きい機で、1 回の掃除が長引かないように）
    const SCAN_CEILING: libc::c_int = 1 << 16;

    pub(super) fn current_thread() -> usize {
        unsafe { libc::pthread_self() as usize }
    }

    /// `pthread_atfork` の子ハンドラ。構えたスレッドが起こした fork の子でだけ掃く
    /// （fork 後の子のスレッドは fork したスレッドの写しなので `pthread_self` が一致する）
    #[cfg(target_os = "macos")]
    pub(super) unsafe extern "C" fn on_fork_child() {
        let armed = ARMED.load(std::sync::atomic::Ordering::Acquire);
        if armed != 0 && armed == current_thread() {
            seal_all();
        }
    }

    pub(super) fn seal_all() -> usize {
        #[cfg(target_os = "macos")]
        {
            let mut listed: [libc::proc_fdinfo; LISTED_MAX] = unsafe { std::mem::zeroed() };
            if let Some(n) = list_open_fds(&mut listed) {
                return listed[..n]
                    .iter()
                    .filter(|info| seal_one(info.proc_fd))
                    .count();
            }
        }
        scan_all()
    }

    /// 開いている fd を列挙する。**入り切らなかった / 読めなかったら None**（総当たりへ倒す）。
    ///
    /// 総当たりにしないのは、fd 上限が起こし方で桁違いに変わるから（Finder 起動は 256 /
    /// 端末から起こすと継承して 100 万級 = `kern.maxfilesperproc` で頭打ちでも 13 万）。
    /// 13 万回の `fcntl` は 1 回の PTY 起動で UI スレッドを数十 ms 止める
    #[cfg(target_os = "macos")]
    fn list_open_fds(buf: &mut [libc::proc_fdinfo]) -> Option<usize> {
        let entry = std::mem::size_of::<libc::proc_fdinfo>();
        let bytes = libc::c_int::try_from(std::mem::size_of_val(buf)).ok()?;
        let got = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                buf.as_mut_ptr().cast(),
                bytes,
            )
        };
        let n = usize::try_from(got).ok()? / entry;
        // ちょうど満杯は「入り切ったか」を判別できないので総当たりへ倒す
        (got > 0 && n < buf.len()).then_some(n)
    }

    /// 3 から fd 上限まで総当たりで掃く（列挙できないときの逃げ道）
    pub(super) fn scan_all() -> usize {
        let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
        let ceiling = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
            libc::c_int::try_from(limit.rlim_cur)
                .unwrap_or(SCAN_CEILING)
                .min(SCAN_CEILING)
        } else {
            SCAN_CEILING
        };
        (FIRST_SEALED_FD..ceiling)
            .filter(|&fd| seal_one(fd))
            .count()
    }

    /// 1 本の fd へ CLOEXEC を立てる。立てたら真
    fn seal_one(fd: libc::c_int) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if !needs_seal(fd, flags, libc::FD_CLOEXEC) {
            return false;
        }
        unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) == 0 }
    }
}

/// 親だけで走る構えの側（ロック・`Once` を使ってよい）
#[cfg(unix)]
mod arming {
    use std::sync::atomic::Ordering;

    use super::sys;

    #[cfg(target_os = "macos")]
    extern "C" {
        fn pthread_atfork(
            prepare: Option<unsafe extern "C" fn()>,
            parent: Option<unsafe extern "C" fn()>,
            child: Option<unsafe extern "C" fn()>,
        ) -> libc::c_int;
    }

    /// 子ハンドラを登録できたか（1 回だけ登録する。外す手段は無いが、構えていなければ
    /// 何もしないので、ほかの fork には原子変数を 1 回読む手間しか掛けない）
    #[cfg(target_os = "macos")]
    fn handler_registered() -> bool {
        static REGISTER: std::sync::Once = std::sync::Once::new();
        static REGISTERED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        REGISTER.call_once(|| {
            let rc = unsafe { pthread_atfork(None, None, Some(sys::on_fork_child)) };
            REGISTERED.store(rc == 0, Ordering::Release);
        });
        REGISTERED.load(Ordering::Acquire)
    }

    /// glibc の `pthread_atfork` は libc_nonshared 経由の包みで素のリンクに載らないので、
    /// macOS 以外の unix（tako の対象外）は登録しない
    #[cfg(not(target_os = "macos"))]
    fn handler_registered() -> bool {
        false
    }

    pub(super) fn spawn_sealed<T>(spawn: impl FnOnce() -> T) -> T {
        if !handler_registered() {
            // 子ハンドラを置けない環境: 親で掃いてから起こす
            // （掃いてから fork までに別スレッドが開いた fd の隙間は残る）
            sys::seal_all();
            return spawn();
        }
        // 構えは 1 スレッドずつ（`ARMED` は 1 枠。ペインの PTY は UI スレッドで起きるので
        // 製品では奪い合わない。並走するテストは直列になる）
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        /// `spawn` が panic しても構えを解く
        struct Disarm;
        impl Drop for Disarm {
            fn drop(&mut self) {
                sys::ARMED.store(0, Ordering::Release);
            }
        }
        sys::ARMED.store(sys::current_thread(), Ordering::Release);
        let _disarm = Disarm;
        spawn()
    }
}

#[cfg(not(unix))]
mod sys {
    /// Windows では呼ばれない（[`super::seals`] が偽）。ハンドルは既定で継承されない
    pub(super) fn seal_all() -> usize {
        0
    }

    pub(super) fn scan_all() -> usize {
        0
    }
}

#[cfg(not(unix))]
mod arming {
    /// Windows では呼ばれない（[`super::seals`] が偽）
    pub(super) fn spawn_sealed<T>(spawn: impl FnOnce() -> T) -> T {
        spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 掃くのはmacosだけでwindowsは掃かない() {
        assert!(seals(Platform::MacOs));
        assert!(!seals(Platform::Windows), "ConPTY はハンドルを継承しない");
    }

    #[test]
    fn stdioと開いていないfdとcloexec付きには立てない() {
        const CLOEXEC: i32 = 1;
        assert!(!needs_seal(0, 0, CLOEXEC), "0 は子の stdin");
        assert!(!needs_seal(2, 0, CLOEXEC), "2 は子の stderr");
        assert!(!needs_seal(3, -1, CLOEXEC), "開いていない fd");
        assert!(!needs_seal(4, CLOEXEC, CLOEXEC), "既に CLOEXEC");
        assert!(needs_seal(3, 0, CLOEXEC));
        assert!(
            needs_seal(4, 2, CLOEXEC),
            "CLOEXEC 以外のビットが立っていても立てる"
        );
    }

    #[test]
    fn 検査の片の出力を読める() {
        assert_eq!(observed_inherited("$ \nFDCHK=[]\n"), Some(vec![]));
        assert_eq!(observed_inherited("FDCHK=[4 5]"), Some(vec![4, 5]));
        assert_eq!(observed_inherited("  FDCHK=[12]  "), Some(vec![12]));
        assert_eq!(observed_inherited("まだ出ていない"), None);
        let script = inherited_probe_script(&[4, 12]);
        assert!(script.contains("for n in 4 12;"), "{script}");
        assert!(script.contains("FDCHK=["), "{script}");
    }

    #[cfg(unix)]
    #[test]
    fn stdioには触らない() {
        let cloexec_of = |fd: i32| unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC;
        let before: Vec<i32> = (0..3).map(cloexec_of).collect();
        seal_inherited_fds();
        let after: Vec<i32> = (0..3).map(cloexec_of).collect();
        assert_eq!(before, after, "0 / 1 / 2 の CLOEXEC は変えない");
    }
}
