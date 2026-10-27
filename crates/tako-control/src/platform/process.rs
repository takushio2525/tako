//! プロセス制御（抽象境界 B5 の制御側）
//!
//! 「プロセスを終わらせる」操作のプラットフォーム差を閉じ込める。
//! 検査側（列挙・親子関係・listen ポート）は `tako-core` 側の境界で扱う。

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
        fn CloseHandle(object: Handle) -> i32;
    }

    /// 開いたハンドルの相手がもう終わっているか（待たずに 1 回だけ問う）
    ///
    /// # Safety
    /// `handle` は `SYNCHRONIZE` 付きで開いた有効なハンドル
    unsafe fn exited(handle: Handle) -> bool {
        WaitForSingleObject(handle, 0) == WAIT_OBJECT_0
    }

    pub fn terminate(pid: u32, _force: bool) -> Result<(), String> {
        // 0 は System Idle Process。開けもしないが、意味を unix の腕と揃えて先に断る
        if pid == 0 {
            return Err(super::refuse_special_pid(pid));
        }
        // SAFETY: OpenProcess の戻りは null を検査し、復帰経路すべてで CloseHandle する。
        // 以降の API はこのハンドルと値渡しの引数しか受け取らない
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                // 居ない（ERROR_INVALID_PARAMETER）/ 権限が無い（ERROR_ACCESS_DENIED）
                return Err(format!(
                    "PID {pid} を開けないので停止できない（{}）",
                    std::io::Error::last_os_error()
                ));
            }
            let outcome = if exited(handle) {
                // もう終わっている（誰かがハンドルを持つ間はオブジェクトが残る = unix のゾンビ
                // 相当）。unix の kill がゾンビへ成功するのと揃え、要求は果たされている扱い
                Ok(())
            } else if TerminateProcess(handle, EXIT_CODE) != 0 {
                Ok(())
            } else {
                let err = std::io::Error::last_os_error();
                // 終わりかけの相手への TerminateProcess は ERROR_ACCESS_DENIED で失敗する。
                // 開いてから撃つまでに終わったのなら要求は果たされている
                if exited(handle) {
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
    /// 腕が先に終わったかを問う）。どちらも `Child` を持ったまま = pid は再利用されない
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
}
