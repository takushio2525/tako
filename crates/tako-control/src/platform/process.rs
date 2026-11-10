//! プロセス制御（抽象境界 B5 の制御側）
//!
//! 「プロセスを終わらせる」操作のプラットフォーム差を閉じ込める。
//! 検査側（列挙・親子関係・listen ポート）は `tako-core` 側の境界で扱う。
//!
//! 制御側の呼び手（`remote` の daemon・`queue_wrapper` の包み）が使う**シグナルの受け取り・転送**と
//! **daemon として切り離す**起こし方・**起こした子とその子孫を止める**取り消しの OS 差もここに置く
//! （#2860。#1538 の表の S5）。中身は移す前の呼び手の `cfg` の塊をそのまま写したもので、unix 以外の腕
//! （何もしない / 入れ物ごと止める）も移す前の挙動のまま。プロセスグループの扱いと終了シグナルの読み取りは
//! `tako-core` の呼び手も使うので `tako_core::platform::process` にある。

use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// 指定 PID の終了を要求する。
///
/// - `force = false`: 穏当な終了要求（unix は SIGTERM。Windows は下記）
/// - `force = true`: 強制終了（unix は SIGKILL / Windows は `TerminateProcess`）
///
/// 「要求を出せた」ことだけを保証する。実際に終了したかは呼び出し側でポーリングする
/// （[`has_terminated`]）。**撃つ相手の正体確認は呼び出し側の責任**（#329 / #1401 / #1616）。
///
/// # Windows に穏当な終了要求は無い（#1599）
///
/// unix の SIGTERM に当たる「任意のプロセスへ届いて、相手が後始末してから終われる要求」が
/// Windows には無い。`GenerateConsoleCtrlEvent` は**同じコンソールを共有するプロセス
/// グループ**にしか届かず（`CREATE_NO_WINDOW` で起こした daemon や別のコンソールの相手には
/// 届かない。逆に自分のグループへ撃つと呼び出し元まで落ちる）、`WM_CLOSE` は窓を持つ
/// プロセスにしか届かない。なので Windows は `force` に関わらず `TerminateProcess` で止める
/// （[`GRACEFUL_TERMINATE`] が `false`）。後始末をさせたい呼び出し側は、相手の協力が要る口
/// （remote daemon なら admin API の shutdown）を**先に**試し、これは最後の手段に使う。
///
/// # 撃たない pid
///
/// `0` と `pid_t` の範囲外は送る前に断る。unix の `kill` は `0` を「自分のプロセス
/// グループ」、`-1`（= `u32::MAX` を `as` で潰した値）を「権限の届く全プロセス」と読むので、
/// 素通しすると呼び出し元ごと周りを撃つ（生存判定の境界 `pid_alive` と同じ扱い。#1557）
pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
    imp::terminate(pid, force)
}

/// [`terminate`] で終了を要求し、`job`（`pid` が居るペインの子孫の入れ物 = `TerminalSession::descendant_job`）
/// の中に残った `pid` の子孫も止める（#2462。境界 B30 の `Job::stop_descendants_of`）。
///
/// Windows の `TerminateProcess` は子孫を道連れにせず、unix の SIGTERM と違って相手に後始末の
/// 機会も無い。エージェントの再起動（session-restart の harness）で古いエージェントが起こした
/// MCP サーバ・背景のシェルが、ペインを閉じるまで残っていた。子孫の寿命はペインを閉じるのと同じ
/// （コンソールのプロセスは止め、GUI のアプリとその子孫は残す）で、ペインのシェルとその別の子は撃たない。
///
/// 終了要求が失敗したら子孫には触れずに `Err`（呼び手は何も壊さずに断る）。unix・器のペイン・
/// A/B の `TAKO_2454_LEGACY=1`（`job` が無い）は [`terminate`] だけ（unix は SIGTERM を受けた相手が
/// 自分の子を畳む）。返り値は止めた / 残した子孫の数
pub fn terminate_with_descendants(
    pid: u32,
    force: bool,
    job: Option<&tako_core::platform::job::Job>,
) -> Result<tako_core::platform::job::Release, String> {
    terminate(pid, force)?;
    Ok(job.map_or_else(Default::default, |job| job.stop_descendants_of(pid)))
}

/// 穏当な終了要求（`terminate(pid, false)`）が強制終了と**別物**か（#1599）。
///
/// unix は SIGTERM（相手が後始末できる）と SIGKILL が別物 = `true`。
/// Windows は両方 `TerminateProcess` になる = `false`（[`terminate`] の節）
pub const GRACEFUL_TERMINATE: bool = cfg!(unix);

/// 終了要求の呼び名（利用者へ返す文言に使う。#1599）。
///
/// unix はシグナル名そのまま（従来の文言をバイト単位で保つ）。Windows は
/// `force` に関わらず `TerminateProcess`（[`GRACEFUL_TERMINATE`]）
pub fn request_name(force: bool) -> &'static str {
    match (GRACEFUL_TERMINATE, force) {
        (true, false) => "SIGTERM",
        (true, true) => "SIGKILL",
        (false, _) => "TerminateProcess",
    }
}

/// 送る前に断る pid の説明（両 OS の腕が使う 1 実装）
fn refuse_special_pid(pid: u32) -> String {
    format!("PID {pid} は停止の対象にできない（0 と範囲外の値は、送るとプロセスグループや全プロセスを指す）")
}

/// 開いたハンドルで見た相手が**もう終わっている / 終わりかけ**か（Windows の腕の純粋部分。#1599）。
///
/// `signaled` は `WaitForSingleObject(handle, 0)` がシグナル状態を返したか、`exit_code` は
/// `GetExitCodeProcess` の結果。Microsoft の仕様では終了時に**終了コードが先に決まり、
/// その後でオブジェクトがシグナル状態になる**（#2275 で記録した順序）。その間の相手は
/// まだシグナル状態でないうえ、`TerminateProcess` は ERROR_ACCESS_DENIED で失敗する。
/// シグナル状態だけで見ると、終わりかけの相手への要求を「失敗」と答えてしまう
/// （CI の Windows で実際に割れた）。終了コードの読みは生存判定と同じ 1 実装
/// （`tako_core::platform::process::exited`）に揃える = 停止の待ち合わせ（`has_terminated`）と
/// 同じ瞬間から「終わった」と言う。
///
/// cfg で割れていないので **macOS からも Windows の腕の判定を検査できる**
#[cfg_attr(not(windows), allow(dead_code))]
fn already_ending(signaled: bool, exit_code: Option<u32>) -> bool {
    signaled || tako_core::platform::process::exited(exit_code)
}

/// プロセスが**終了済み**か（ゾンビも終了済みとして扱う）。
///
/// `kill(pid, 0)`（= `tako_core::platform::process::pid_alive`）は**ゾンビにも成功する**ので、
/// 停止の待ち合わせには生死だけでは足りない（#619 で踏んだ）。
/// 刈り取れない別プロセスから停止したときに「終了しない」と誤判定するのを防ぐ。
///
/// 呼ぶのは停止の待ち合わせ中だけ（定常のポーリング経路へは入れない。#340 の方針）
pub fn has_terminated(pid: u32) -> bool {
    terminated_via(pid).is_some()
}

/// 終了済みと読めた**理由**（#962）。
///
/// 待ち合わせが**どう抜けたか**を呼び出し側とテストが観測するための値。
/// #619 の回帰（ゾンビを生存と読む）は `Zombie` が出なくなる形で現れるので、
/// 「速く返ったか」を実時間で測る代わりにこちらを見る
/// （`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminatedVia {
    /// pid ごと居ない（親が刈り取った / そもそも消えた）
    Gone,
    /// 終了済みだが親が未刈り取り（defunct）。#619 の本題
    Zombie,
}

/// 終了済みならそう読めた理由を返す（`None` = まだ動いている）。
///
/// [`has_terminated`] の中身をそのまま分解したもので、判定は 1 実装。
/// `ps` を起こすのは「生きている」と読めたときだけ（回数も旧実装と同じ）
pub fn terminated_via(pid: u32) -> Option<TerminatedVia> {
    #[cfg(test)]
    test_inject::observation_delay();
    if !tako_core::platform::process::pid_alive(pid) {
        return Some(TerminatedVia::Gone);
    }
    #[cfg(test)]
    if test_inject::blind_to_zombies() {
        // #619 **前**の実装（ゾンビを生存と読む）を再現する注入。
        // #962 の新しいアサートの検出力（タイムアウト経路へ落ちたら落ちる）を示すために使う
        return None;
    }
    is_zombie(pid).then_some(TerminatedVia::Zombie)
}

/// 待ち合わせの注入口（**テストビルド限定**。製品バイナリには 1 バイトも入らない）。
///
/// #962 の A/B で「混んだ機で `/bin/ps` が秒単位ブロックする」（= 実測 2.27 / 5.10 /
/// 10.07 秒の原因）と「#619 前の誤判定」を、同じバイナリの中で再現するための口
#[cfg(test)]
mod test_inject {
    /// `TAKO_962_INJECT_DELAY_MS`: 1 回の観測をこの ms だけ遅らせる
    /// （fork+exec した `/bin/ps` が詰まる実機の代わり）
    pub(super) fn observation_delay() {
        let Some(ms) = std::env::var("TAKO_962_INJECT_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    /// `TAKO_962_INJECT=blind_zombie`: ゾンビを生存と読む（= #619 前の実装）
    pub(super) fn blind_to_zombies() -> bool {
        std::env::var("TAKO_962_INJECT").is_ok_and(|v| v == "blind_zombie")
    }
}

/// ゾンビ（終了済みだが親が未刈り取り）か。
/// 待ち合わせの前提をテストで固定するために公開している
pub fn is_zombie(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let mut cmd = std::process::Command::new("/bin/ps");
        cmd.args(["-p", &pid.to_string(), "-o", "stat="])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        // この枝は unix 限定なので実質 no-op だが、境界 B14 を素通りしない形に揃えておく
        // （#628 / #586 の番犬が「抑止していない起動」として数えないため）
        tako_core::platform::process::no_console_window(&mut cmd)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().starts_with('Z'))
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// SIGTERM / SIGINT を受けたら `shutdown` を立てる（remote daemon の停止の受け口。#590 / #2860）。
///
/// 先にシグナルマスクを空に戻す（[`reset_signal_mask`]。以後に spawn するスレッドは作成元の
/// マスクを継承するので、ハンドラの設置より前・スレッドの生成より前に戻す）。ハンドラはフラグを
/// 立てるだけ（async-signal-safe）で、200ms おきに見る見張りのスレッド（`signal-watcher`）が
/// `shutdown` へ写す。見張りを立てられなければ `Err`（daemon は起動を中止する）。
/// unix 以外は何もしない（受けるシグナルが無い。daemon は admin API の shutdown で止める）
pub fn watch_shutdown_signals(shutdown: &Arc<AtomicBool>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::sync::atomic::Ordering;
        use std::sync::atomic::Ordering::Relaxed;
        // #590: **シグナルマスクを空に戻す**。以後に spawn するスレッドは作成元の
        // マスクを継承するので、ハンドラ設置より前・スレッド生成より前のここで戻す
        reset_signal_mask();
        let _ = unsafe {
            libc::signal(
                libc::SIGTERM,
                signal_handler as *const () as libc::sighandler_t,
            )
        };
        let _ = unsafe {
            libc::signal(
                libc::SIGINT,
                signal_handler as *const () as libc::sighandler_t,
            )
        };
        static SHUTDOWN_FLAG: AtomicBool = AtomicBool::new(false);
        extern "C" fn signal_handler(_: libc::c_int) {
            SHUTDOWN_FLAG.store(true, Relaxed);
        }

        // シグナル待ちスレッド
        let shutdown_clone = shutdown.clone();
        std::thread::Builder::new()
            .name("signal-watcher".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if SHUTDOWN_FLAG.load(Relaxed) {
                    shutdown_clone.store(true, Ordering::Relaxed);
                    break;
                }
            })?;
    }
    #[cfg(not(unix))]
    let _ = shutdown;
    Ok(())
}

/// シグナルマスクを空に戻す（#590）。
///
/// シグナルマスクは fork + exec をまたいで継承される。GUI（tako-app = GPUI / AppKit）
/// から起動した daemon は SIGTERM / SIGHUP がブロックされたまま生まれ、実測では
/// ハンドラを設置しても発火せず（SIGHUP の既定動作すら起きず）、`tako remote stop` と
/// GUI の kill switch（全遮断）が無言で失敗していた。daemon 側（[`watch_shutdown_signals`]）と
/// プロセス起動側（[`configure_daemon_child`]）の両方で戻すことで、どの経路から起動しても
/// 停止できることを保証する
#[cfg(unix)]
fn reset_signal_mask() {
    unsafe {
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
    }
}

/// デーモン子プロセスの起動条件を整える（「daemon として切り離す」口。#590 / #2860）。
///
/// - `setsid`: プロセスグループから切り離し、親（tmux セッション等）の終了に
///   巻き添えで死なないようにする
/// - シグナルマスクを空に戻す: 親（GUI = GPUI / AppKit）がブロックしている
///   シグナルは exec をまたいで継承される。戻さないと生まれた daemon が SIGTERM を
///   受け取れず、`tako remote stop` と GUI の kill switch が無言で失敗する
/// - GUI の fd を受け継がない（#1768）: GUI が CLOEXEC 無しで開いた fd（Metal の
///   シェーダキャッシュ等）へ CLOEXEC を立て、長生きする daemon に握らせない。
///   子の中で掃くので、掃いてから fork までに別スレッドが開いた fd の隙間も無い
///
/// （`pre_exec` は fork 後・exec 前の子プロセスで走るので、ここで呼ぶ
/// `setsid` / `pthread_sigmask` / `seal_inherited_fds` はどれも async-signal-safe）。
/// Windows は何もしない（unix の setsid 相当が無い。コンソール窓の抑止は呼び手が
/// `tako_core::platform::process::no_console_window` で付ける）
pub fn configure_daemon_child(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                reset_signal_mask();
                tako_core::platform::fd_inherit::seal_inherited_fds();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// シグナルの受け取りと転送（`tako queue` の包み。#1994 / #2860）。
///
/// unix は INT / TERM / HUP / QUIT を受けて溜め（`install` / `take`）、包んだ子へそのまま
/// 送る（`forward`）。Windows は何もしない = Ctrl+C はコンソールの全員に届く
pub mod signal_relay {
    #[cfg(unix)]
    mod imp {
        use std::sync::atomic::{AtomicU32, Ordering};

        static PENDING: AtomicU32 = AtomicU32::new(0);
        const WATCHED: [libc::c_int; 4] =
            [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

        extern "C" fn on_signal(sig: libc::c_int) {
            // async-signal-safe: アトミックへ立てるだけ
            if (0..32).contains(&sig) {
                PENDING.fetch_or(1 << sig, Ordering::SeqCst);
            }
        }

        pub fn install() {
            for sig in WATCHED {
                // SAFETY: ハンドラはアトミックの更新だけをする async-signal-safe な関数
                unsafe {
                    libc::signal(sig, on_signal as *const () as libc::sighandler_t);
                }
            }
        }

        pub fn take() -> Vec<i32> {
            let bits = PENDING.swap(0, Ordering::SeqCst);
            (0..32).filter(|s| bits & (1 << s) != 0).collect()
        }

        pub fn forward(pid: u32, sig: i32) {
            let Ok(pid) = libc::pid_t::try_from(pid) else {
                return;
            };
            if pid > 0 {
                // SAFETY: 自分が起こした子の pid へシグナルを送るだけ
                unsafe {
                    libc::kill(pid, sig);
                }
            }
        }
    }

    #[cfg(not(unix))]
    mod imp {
        pub fn install() {}
        pub fn take() -> Vec<i32> {
            Vec::new()
        }
        pub fn forward(_pid: u32, _sig: i32) {}
    }

    pub use imp::{forward, install, take};
}

/// 起こした子とその子孫を止める（`tako queue` の取り消し。#1994 / #2462 / #2860）
pub mod child_tree {
    use std::process::Child;

    use tako_core::platform::job::KillScope;

    /// unix: 子とその子孫（libproc の親子表）へ SIGTERM（`force` なら SIGKILL）。
    /// 親子表が採れなければ子だけ。入れ物（`KillScope`）は unix では空なので見ない
    #[cfg(unix)]
    pub fn terminate(child: &mut Child, _descendants: &mut KillScope, force: bool) {
        let root = child.id();
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        let mut targets = vec![root];
        if let Some(procs) = tako_core::platform::resources::processes() {
            let mut frontier = vec![root];
            while let Some(p) = frontier.pop() {
                for r in procs.iter().filter(|r| r.ppid == p && r.pid != p) {
                    if !targets.contains(&r.pid) {
                        targets.push(r.pid);
                        frontier.push(r.pid);
                    }
                }
            }
        }
        for pid in targets {
            if let Ok(pid) = libc::pid_t::try_from(pid) {
                if pid > 0 {
                    // SAFETY: 自分が起こした子とその子孫へ送るだけ
                    unsafe {
                        libc::kill(pid, sig);
                    }
                }
            }
        }
    }

    /// Windows: 子とその子孫を入れ物（Job Object。境界 B30。#2462）ごと止める。寿命はペインを
    /// 閉じるのと同じ（コンソールのプロセスは止め、GUI のアプリとその子孫は残す）。穏当な終了要求は
    /// 無いので `force` は見ない（1 回目で止まる。2 回目の入れ物は空）。入れ物が無い回
    /// （入れられなかった・A/B の `TAKO_2454_LEGACY=1`）は子だけ（孫は残る = #2462 の前）
    #[cfg(not(unix))]
    pub fn terminate(child: &mut Child, descendants: &mut KillScope, _force: bool) {
        descendants.stop();
        let _ = child.kill();
    }
}

#[cfg(unix)]
mod imp {
    pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
        // `as` で潰さない: u32::MAX は -1 =「全プロセス」、0 は「自分のプロセスグループ」
        let target = match libc::pid_t::try_from(pid) {
            Ok(p) if p > 0 => p,
            _ => return Err(super::refuse_special_pid(pid)),
        };
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: 引数は値渡しで、ポインタを触らない。対象は正の pid 1 つだけ
        let ret = unsafe { libc::kill(target, sig) };
        if ret != 0 {
            return Err(format!(
                "PID {pid} への signal 送信に失敗（errno: {}）",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// Windows の腕（#1599）。`OpenProcess` + `TerminateProcess`。
///
/// 開いたハンドルを持つ間はその pid が再利用されないので、「終わったか」の確認と撃つ操作は
/// **同じハンドル**で行う（開いてから撃つまでに別のプロセスへ入れ替わることは無い）。
/// 開く前の正体確認とのあいだの窓は unix の `kill` と同じく残る（呼び出し側が確認直後に呼ぶ）
#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    /// `PROCESS_TERMINATE`（winnt.h）。`TerminateProcess` に要る権利
    const PROCESS_TERMINATE: u32 = 0x0001;
    /// `SYNCHRONIZE`（winnt.h）。終わったかを `WaitForSingleObject` で問うのに要る権利
    const SYNCHRONIZE: u32 = 0x0010_0000;
    /// `PROCESS_QUERY_LIMITED_INFORMATION`（winnt.h）。終了コードを問うのに要る権利
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x0000_1000;
    /// `WAIT_OBJECT_0`。プロセスのオブジェクトがシグナル状態 = もう終わっている
    const WAIT_OBJECT_0: u32 = 0;
    /// 終わらせたプロセスの終了コード。`STILL_ACTIVE`（259）でなければ生存判定
    /// （`tako_core::platform::process::exited`。#2275）が終了済みと読む
    const EXIT_CODE: u32 = 1;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    /// 開いたハンドルの相手がもう終わっている / 終わりかけか（待たずに 1 回だけ問う）。
    /// 判定は純関数 [`super::already_ending`]
    ///
    /// # Safety
    /// `handle` は `SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION` 付きで開いた有効なハンドル
    unsafe fn ending(handle: Handle) -> bool {
        let signaled = WaitForSingleObject(handle, 0) == WAIT_OBJECT_0;
        let mut code = 0u32;
        let exit_code = (GetExitCodeProcess(handle, &mut code) != 0).then_some(code);
        super::already_ending(signaled, exit_code)
    }

    pub fn terminate(pid: u32, _force: bool) -> Result<(), String> {
        // 0 は System Idle Process。開けもしないが、意味を unix の腕と揃えて先に断る
        if pid == 0 {
            return Err(super::refuse_special_pid(pid));
        }
        // SAFETY: OpenProcess の戻りは null を検査し、復帰経路すべてで CloseHandle する。
        // 以降の API はこのハンドルと値渡しの引数しか受け取らない
        unsafe {
            let handle = OpenProcess(
                PROCESS_TERMINATE | SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if handle.is_null() {
                // 居ない（ERROR_INVALID_PARAMETER）/ 権限が無い（ERROR_ACCESS_DENIED）
                return Err(format!(
                    "PID {pid} を開けないので停止できない（{}）",
                    std::io::Error::last_os_error()
                ));
            }
            let outcome = if ending(handle) {
                // もう終わっている（誰かがハンドルを持つ間はオブジェクトが残る = unix のゾンビ
                // 相当）か、終わりかけ。unix の kill がゾンビへ成功するのと揃え、要求は果たされている扱い
                Ok(())
            } else if TerminateProcess(handle, EXIT_CODE) != 0 {
                Ok(())
            } else {
                let err = std::io::Error::last_os_error();
                // 終わりかけの相手への TerminateProcess は ERROR_ACCESS_DENIED で失敗する。
                // 開いてから撃つまでに終わり始めたのなら要求は果たされている
                if ending(handle) {
                    Ok(())
                } else {
                    Err(format!("PID {pid} の TerminateProcess に失敗（{err}）"))
                }
            };
            CloseHandle(handle);
            outcome
        }
    }
}

/// 停止の実プロセス検査（#1599）。**撃つのは自分で起こした子だけ**。
///
/// CI の Windows ジョブが名指しで回す（`.github/workflows/ci.yml` の「停止の実行検査」）。
/// `TerminateProcess` の腕は macOS では 1 行も実行できないので、ここが Windows の証拠になる
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// 眠るだけの子（Windows は ping で眠る）。起動は境界の抑止を通す（#628 の番犬の数え方）
    fn spawn_sleeper() -> Child {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("ping", &["-n", "60", "127.0.0.1"])
        } else {
            ("sleep", &["60"])
        };
        let mut cmd = Command::new(program);
        cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
        tako_core::platform::process::no_console_window(&mut cmd)
            .spawn()
            .expect("眠るだけの子を起こせる")
    }

    /// すぐ終わる子
    fn spawn_quick() -> Child {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "exit 0"])
        } else {
            ("true", &[])
        };
        let mut cmd = Command::new(program);
        cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
        tako_core::platform::process::no_console_window(&mut cmd)
            .spawn()
            .expect("すぐ終わる子を起こせる")
    }

    /// 子が**その終了要求で**終わったか（実時間ではなく終了の印で見る = #1220）。
    /// 眠る子が自然に終わった（= 要求が効かなかった）なら、印がこの値にならない
    fn ended_by_request(status: std::process::ExitStatus, force: bool) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            let expected = if force { libc::SIGKILL } else { libc::SIGTERM };
            status.signal() == Some(expected)
        }
        #[cfg(windows)]
        {
            let _ = force;
            // `imp::EXIT_CODE`。ping の宛先は 127.0.0.1 なので、自然に終われば必ず 0
            status.code() == Some(1)
        }
    }

    /// 受け入れ条件 1: **自分で起こした子を実際に止める**（両 OS・穏当 / 強制の両方）。
    ///
    /// 「要求を出せた（Ok）」だけでなく、子の終了の印が**この要求のもの**であること
    /// （unix はシグナル番号、Windows は `TerminateProcess` に渡した終了コード）を見る。
    /// 未実装の腕（常に Err）はここで落ちる
    #[test]
    fn terminateは自分で起こした子を実際に止める() {
        for force in [false, true] {
            let mut child = spawn_sleeper();
            let pid = child.id();
            assert!(
                tako_core::platform::process::pid_alive(pid),
                "force={force}: 起こした子 {pid} が居ない"
            );
            let sent = terminate(pid, force);
            if let Err(e) = &sent {
                // 撃てなかったときも子は残さない（以降の assert で落ちる前に片付ける）
                let _ = child.kill();
                let _ = child.wait();
                panic!("force={force}: 自分の子 {pid} を止められない: {e}");
            }
            let status = child.wait().expect("看取れる");
            assert!(
                ended_by_request(status, force),
                "force={force}: 子 {pid} の終了の印が {} の要求のものではない: {status:?}",
                request_name(force)
            );
            // `child` はまだ持っている = Windows はオブジェクトが残り pid が再利用されない
            assert!(
                has_terminated(pid),
                "force={force}: 止めた子 {pid} を終了済みと読めない"
            );
        }
    }

    /// エッジケース: **もう終わった相手**へ撃っても何も起こさず、要求は果たされている扱い。
    ///
    /// unix は刈り取る前のゾンビ（`kill` は成功する）、Windows は終わったが
    /// ハンドルが残っているオブジェクト（`TerminateProcess` は ACCESS_DENIED で失敗するので、
    /// 腕が先に終わったかを問う）。どちらも `Child` を持ったまま = pid は再利用されない。
    ///
    /// 待ち合わせは `has_terminated`（Windows は終了コード）なので、**終了コードは決まったが
    /// まだシグナル状態でない**窓に当たりうる。腕がシグナル状態だけで見ていた版は、この窓で
    /// Err を返して CI の Windows で割れた（#1599 の 2 回目の run）。判定の向きは
    /// `already_ending` の単体が決定的に固定する
    #[test]
    fn terminateは終わった自分の子へは何も起こさずokを返す() {
        let mut child = spawn_quick();
        let pid = child.id();
        // 刈り取らずに終わるのを待つ（unix はゾンビ / Windows は終了コードで読む）
        let deadline = Instant::now() + Duration::from_secs(20);
        while !has_terminated(pid) {
            assert!(Instant::now() < deadline, "すぐ終わる子 {pid} が終わらない");
            std::thread::sleep(Duration::from_millis(20));
        }
        for force in [false, true] {
            assert_eq!(
                terminate(pid, force),
                Ok(()),
                "force={force}: 終わった自分の子 {pid} への要求を失敗扱いにした"
            );
        }
        let status = child.wait().expect("看取れる");
        assert!(
            status.success(),
            "終わった子の終了の印を書き換えた（撃ったことになっている）: {status:?}"
        );
    }

    /// エッジケース: `0` と範囲外は**送る前に**断る。
    ///
    /// **修正前の版でこのテストを走らせてはいけない**: unix の旧実装は `pid as pid_t` で
    /// `u32::MAX` を -1 へ潰し、`kill(-1, SIGTERM)` = 権限の届く全プロセスへ撃つ
    #[test]
    fn terminateは特別値のpidを撃たずに断る() {
        let err = terminate(0, false).expect_err("0 は撃たない");
        assert!(
            err.contains("対象にできない"),
            "送る前に断っていない: {err}"
        );
        for pid in [u32::MAX, i32::MAX as u32 + 1] {
            let err = terminate(pid, false).expect_err("範囲外は撃たない");
            if cfg!(unix) {
                // pid_t へ入らない値は送る前に断る（-1 / 負へ潰さない）
                assert!(err.contains("対象にできない"), "pid {pid}: {err}");
            } else {
                // Windows の pid は DWORD。開けない = 何も撃っていない
                assert!(err.contains("開けない"), "pid {pid}: {err}");
            }
        }
    }

    /// エッジケース: **居ない pid** は Err（撃てたことにしない）。
    ///
    /// unix だけで見る: 99_999_999 は macOS（上限 99,998）でも Linux（上限 2^22）でも
    /// 割り当てられない値。Windows は pid の下位 2 ビットを無視して開くうえ上限が
    /// 文書化されていないので、**任意の数値の pid へは撃たない**（自分の子だけを撃つ規則）
    #[cfg(unix)]
    #[test]
    fn terminateは居ないpidをerrで返す() {
        let err = terminate(99_999_999, true).expect_err("居ない pid は撃てない");
        assert!(err.contains("失敗"), "{err}");
    }

    /// Windows の腕の「もう終わっている / 終わりかけ」の判定（純関数。macOS でも回る）
    #[test]
    fn already_endingは終了コードが決まった時点で終わりかけと読む() {
        use tako_core::platform::process::STILL_ACTIVE;
        assert!(already_ending(true, None), "シグナル状態 = 終わっている");
        assert!(
            already_ending(false, Some(1)),
            "終了コードが決まってまだシグナル状態でない窓（CI の Windows で割れた形）"
        );
        assert!(already_ending(false, Some(0)), "正常終了のコードも同じ");
        assert!(
            !already_ending(false, Some(STILL_ACTIVE)),
            "走っている相手を終わったと読まない（撃たずに Ok を返す嘘になる）"
        );
        assert!(
            !already_ending(false, None),
            "終了コードを読めないときは終わったと言えない"
        );
    }

    /// 呼び名は [`GRACEFUL_TERMINATE`] から決まる（Windows に穏当な終了要求は無い）
    #[test]
    fn request_nameはosの終了要求の名前を返す() {
        if cfg!(unix) {
            assert_eq!(request_name(false), "SIGTERM");
            assert_eq!(request_name(true), "SIGKILL");
        } else {
            assert_eq!(request_name(false), "TerminateProcess");
            assert_eq!(request_name(true), "TerminateProcess");
        }
    }

    /// #590: シグナルマスクのリセットが効くこと。
    /// GUI（GPUI / AppKit）から起動した daemon は SIGTERM がブロックされたまま
    /// 生まれ、`tako remote stop` と kill switch が無言で失敗していた
    #[cfg(unix)]
    #[test]
    fn reset_signal_maskはブロック中のsigtermを解除する() {
        // 現在のスレッドのマスクに SIGTERM が入っているか
        fn sigterm_blocked() -> bool {
            unsafe {
                let mut cur: libc::sigset_t = std::mem::zeroed();
                libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut cur);
                libc::sigismember(&cur, libc::SIGTERM) == 1
            }
        }
        unsafe {
            let mut block: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut block);
            libc::sigaddset(&mut block, libc::SIGTERM);
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut old);
            assert!(sigterm_blocked(), "前提: SIGTERM をブロックできている");
            reset_signal_mask();
            assert!(
                !sigterm_blocked(),
                "reset_signal_mask 後は SIGTERM のブロックが解除される"
            );
            // テストスレッドのマスクを元に戻す（他テストへ影響させない）
            libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
        }
    }

    /// #590: `configure_daemon_child` を通した子プロセスは、親が SIGTERM を
    /// ブロックしていても**ブロックを継承しない**。
    ///
    /// 判定は子（sh）に自分自身へ SIGTERM を送らせる方式:
    /// ブロックが継承されていれば死なず "SURVIVED" を出力し、
    /// 解除されていれば既定動作で死んで何も出力しない。
    /// `reset_signal_mask()` の呼び出しを消すとこのテストは失敗する
    #[cfg(unix)]
    #[test]
    fn configure_daemon_childは親のブロックを子へ継承しない() {
        let run = |configure: bool| -> String {
            let mut cmd = Command::new("/bin/sh");
            cmd.args(["-c", "kill -s TERM $$; echo SURVIVED"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            if configure {
                configure_daemon_child(&mut cmd);
            }
            let out = cmd.output().expect("sh を起動できる");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        unsafe {
            let mut block: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut block);
            libc::sigaddset(&mut block, libc::SIGTERM);
            let mut old: libc::sigset_t = std::mem::zeroed();
            // このスレッドで SIGTERM をブロックする（GUI = 親の状態を再現）
            libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut old);
            // 素の spawn: ブロックが継承されるので SIGTERM で死なない（前提の確認）
            let inherited = run(false);
            // configure 済み: マスクが空に戻るので SIGTERM で死ぬ
            let configured = run(true);
            libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
            assert_eq!(
                inherited, "SURVIVED",
                "前提: 素の spawn では親のブロックが子へ継承される"
            );
            assert_eq!(
                configured, "",
                "configure_daemon_child 後は SIGTERM が届いて子が終了する"
            );
        }
    }

    /// #1768: `configure_daemon_child` を通した daemon の子は、GUI が CLOEXEC 無しで
    /// 開いた fd（Metal のシェーダキャッシュ等）を受け継がない。
    ///
    /// 掃除は子の中（`pre_exec`）で走るので、**親の fd は CLOEXEC 無しのまま残る**。
    /// 測っているあいだに親の fd へ CLOEXEC が立った回（親で掃除を直接呼ぶ誰かが
    /// 並走した）は、子で見えないのがどちらの手柄か分からないので、開き直して測り直す
    #[cfg(unix)]
    #[test]
    fn configure_daemon_childの子はguiがcloexec無しで開いたfdを受け継がない() {
        use tako_core::platform::fd_inherit::{inherited_probe_script, observed_inherited};
        let site = include_str!("process.rs")
            .lines()
            .position(|l| l.starts_with("pub fn configure_daemon_child("))
            .map_or_else(|| "?".to_string(), |i| (i + 1).to_string());
        let cloexec = |fd: i32| unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC != 0;
        let run = |fd: i32, configure: bool| -> Option<Vec<i32>> {
            let mut cmd = Command::new("/bin/sh");
            cmd.args(["-c", &inherited_probe_script(&[fd])])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            if configure {
                configure_daemon_child(&mut cmd);
            }
            let out = cmd.output().expect("sh を起動できる");
            observed_inherited(&String::from_utf8_lossy(&out.stdout))
        };
        for _ in 0..5 {
            let fd = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
            assert!(fd >= 3, "/dev/null を開ける: {fd}");
            let raw = run(fd, false);
            let configured = run(fd, true);
            let parent_untouched = !cloexec(fd);
            unsafe { libc::close(fd) };
            if !parent_untouched {
                continue;
            }
            assert_eq!(
                raw,
                Some(vec![fd]),
                "前提: 素の spawn では CLOEXEC 無しの fd が子へ見える"
            );
            assert_eq!(
                configured,
                Some(vec![]),
                "crates/tako-control/src/platform/process.rs:{site}: daemon の子が CLOEXEC 無しの fd {fd} を\
                 受け継いだ。configure_daemon_child の pre_exec で \
                 platform::fd_inherit::seal_inherited_fds() を通していない（#1768）"
            );
            return;
        }
        panic!("5 回とも別のテストが親で fd を掃いたので、子の中の掃除を測れなかった");
    }
}
