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
