//! プロセス制御（抽象境界 B5 の制御側）
//!
//! 「プロセスを終わらせる」操作のプラットフォーム差を閉じ込める。
//! 検査側（列挙・親子関係・listen ポート）は `tako-core` 側の境界で扱う。

/// 指定 PID の終了を要求する。
///
/// - `force = false`: 穏当な終了要求（unix は SIGTERM）
/// - `force = true`: 強制終了（unix は SIGKILL）
///
/// 「要求を出せた」ことだけを保証する。実際に終了したかは呼び出し側でポーリングする
pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
    imp::terminate(pid, force)
}

#[cfg(unix)]
mod imp {
    pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        let ret = unsafe { libc::kill(pid as libc::pid_t, sig) };
        if ret != 0 {
            return Err(format!(
                "PID {pid} への signal 送信に失敗（errno: {}）",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    /// `OpenProcess` + `TerminateProcess`（#528）。
    ///
    /// **`force` は意図的に無視する**。Windows で「穏当な終了」に当たるのは
    /// `GenerateConsoleCtrlEvent` だが、これはプロセスグループ単位でしか送れず、
    /// remote デーモンは `DETACHED_PROCESS`（= コンソールを持たない）で起動するため届かない。
    /// 呼び出し側は force の有無に関わらず終了をポーリングで確認するので、
    /// 常に `TerminateProcess` に一貫させる方が挙動が読める
    /// （unix の SIGTERM → SIGKILL の段階付けに相当するものは Windows には無い）
    pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
        use std::ffi::c_void;
        // 最小権限: 停止だけができるハンドルを開く
        const PROCESS_TERMINATE: u32 = 0x0001;
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn TerminateProcess(handle: *mut c_void, exit_code: u32) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        let _ = force;
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            return Err(format!(
                "PID {pid} を停止権限つきで開けない（{}）",
                std::io::Error::last_os_error()
            ));
        }
        let ok = unsafe { TerminateProcess(handle, 1) };
        // CloseHandle が errno を上書きする前に読む
        let err = std::io::Error::last_os_error();
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return Err(format!("PID {pid} の停止に失敗（{err}）"));
        }
        Ok(())
    }
}
