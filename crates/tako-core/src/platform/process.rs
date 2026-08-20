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

#[cfg(not(windows))]
mod imp {
    use std::process::Command;

    pub fn no_console_window(cmd: &mut Command) -> &mut Command {
        cmd
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
