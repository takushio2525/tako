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
    /// Windows 実装は `OpenProcess` + `TerminateProcess`（穏当な終了は
    /// コンソール制御イベントの送出）で置き換える（B5 の Windows 実装タスク）
    pub fn terminate(_pid: u32, _force: bool) -> Result<(), String> {
        Err("プロセスの停止は Windows では未対応です".to_string())
    }
}
