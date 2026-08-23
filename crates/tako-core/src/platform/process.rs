//! 子プロセス起動のプラットフォーム差（抽象境界）
//!
//! GUI プロセス（tako-app）から**コンソール系の子プロセスを起動するときの
//! ウィンドウ抑止**を閉じ込める。プロセスの停止（terminate）は制御側の
//! `tako-control::platform::process` が扱う（こちらは起動時の作法のみ）。
//!
//! ## なぜ要るか（#586）
//!
//! tako-app は release で GUI サブシステムとしてリンクされる。GUI サブシステムの
//! プロセスは自前のコンソールを持たないため、そこから **console サブシステムの
//! 子**（git / claude / tako CLI 等）を起動すると Windows が**子のために
//! コンソールウィンドウを新規作成する**。`Stdio::piped()` にしても防げない
//! （実測: 子の `GetConsoleWindow()` が非 NULL・`IsWindowVisible` = 1）。
//!
//! git タブは 2 秒ポーリングで git を叩くため、対策しないとウィンドウが
//! 明滅し続ける。`CREATE_NO_WINDOW` を付けると子はコンソールを持たなくなる
//! （実測: 子の `GetConsoleWindow()` が NULL）。
//!
//! ## 使いどころ
//!
//! **出力をパイプ / 破棄して受け取るだけの子**に付ける。
//! ペインの PTY 起動（ConPTY）は疑似コンソールへ接続するため対象外
//! （そちらは alacritty_terminal 側が面倒を見る）。

use std::process::Command;

/// 子プロセスにコンソールウィンドウを作らせない。
///
/// Windows 以外では何もしない（呼び出し側に `cfg` を書かせないための境界）。
/// 新しく外部コマンドを叩くコードを足すときは、GUI プロセス（tako-app）から
/// 到達しうるなら必ずこれを通すこと
pub fn no_console_window(cmd: &mut Command) -> &mut Command {
    imp::no_console_window(cmd)
}

/// その pid のプロセスが生きているか（**残骸の掃除の判断に使う**。#916）。
///
/// unix は `kill(pid, 0)`（権限が無くて EPERM でも「居る」= true）。
/// Windows は [`super::procinfo::snapshot`] の在籍で見る（あちらは Windows 実装が正）。
///
/// ゾンビ（終了済みで親が未刈り取り）は unix では true になる。掃除の判断としては
/// それで正しい（親がまだ居る = そのプロセス列は現役の可能性がある）。
/// 「終わったか」を待ち合わせる用途には使わないこと（`remote.rs` の `has_terminated` が
/// ゾンビ判定込みでそれを担う）
pub fn pid_alive(pid: u32) -> bool {
    imp::pid_alive(pid)
}

#[cfg(not(windows))]
mod imp {
    use std::process::Command;

    pub fn no_console_window(cmd: &mut Command) -> &mut Command {
        cmd
    }

    pub fn pid_alive(pid: u32) -> bool {
        // `kill` の第 1 引数は 0 で「自分のプロセスグループ」、負で「プロセスグループ /
        // 全プロセス」を指す**特別値**。`as` で潰すと u32::MAX が -1 になり
        // 「全プロセスへ送る」= 常に成功してしまう（テストで実際に踏んだ）。
        // pid_t の正の範囲に収まらない値は「居ない」と答える
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // SAFETY: signal 0 は送らずに存在と権限だけを確かめる呼び出し。
        // 引数は値渡しでポインタを触らない
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return true;
        }
        // 権限が無くて拒否された = 相手は居る
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// `CREATE_NO_WINDOW`（winbase.h）。コンソールを持たない親から起動された
    /// コンソールアプリに、コンソールウィンドウを与えない
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn no_console_window(cmd: &mut Command) -> &mut Command {
        cmd.creation_flags(CREATE_NO_WINDOW)
    }

    pub fn pid_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        // Windows は在籍の列挙（Toolhelp）が procinfo 側にあるのでそれを使う。
        // OpenProcess を新たに宣言せずに済み、実装は 1 か所に留まる
        super::super::procinfo::snapshot()
            .iter()
            .any(|p| p.pid == pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自分のpidは生きている() {
        assert!(pid_alive(std::process::id()));
    }

    /// `kill` の特別値（0 / 負）へ落ちないこと。ここを `as` で潰すと
    /// u32::MAX が -1 = 「全プロセス」になり、生きていない pid が生きて見える
    #[test]
    fn 特別値になる範囲外のpidは生きていない() {
        assert!(!pid_alive(u32::MAX), "pid_t の範囲外");
        assert!(!pid_alive(0), "0 はプロセスグループの指定");
        assert!(
            !pid_alive(i32::MAX as u32),
            "pid_t の上限は実在しない（macOS / Linux の pid 上限より大きい）"
        );
    }

    #[test]
    fn no_console_windowを通しても子プロセスの結果は変わらない() {
        // 境界が「ウィンドウを出さない」以外の副作用を持たないことの回帰テスト。
        // 実行するのは各プラットフォームで確実に存在するコマンドに限る
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo tako"]);
            c
        } else {
            let mut c = Command::new("echo");
            c.arg("tako");
            c
        };
        let out = no_console_window(&mut cmd)
            .output()
            .expect("子プロセスを起動できない");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "tako");
    }
}
