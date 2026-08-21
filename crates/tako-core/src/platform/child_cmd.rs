//! tako 自身が起こす子プロセスの組み立て（抽象境界 B21。#877）
//!
//! ## なぜ境界が要るか
//!
//! tako は「ユーザーの環境で CLI を 1 回走らせて出力を読む」ことを随所でやる
//! （`claude agents --json` の走査・`command -v` の探索）。**ペインで起こす PTY とは別**で、
//! こちらは短命な子プロセスなので `platform::shell`（B1）の担当外。
//!
//! 従来の実装は `$SHELL -l -c "<シェル片>"` の直書きだった。**macOS ではこれでないと困る**:
//! `.app` を Dock から起動するとプロセスの PATH が最小構成になり、Homebrew や
//! `~/.local/bin` の CLI が一切見つからない。ログインシェルを経由して初めて解決できる。
//!
//! 一方 **Windows には `SHELL` も `-l -c` も無い**ので、この形は必ず失敗する。
//! 実測（2026-08-21・Windows 11 / claude 2.1.238）:
//!
//! ```text
//! SHELL 未設定 → /bin/sh へ落ちる → CreateProcess: The system cannot find the file specified
//! SHELL=powershell.exe → -l : The term '-l' is not recognized as the name of a cmdlet…
//! ```
//!
//! ## Windows 側の作法
//!
//! Windows は**シェルを起こさない**。`platform::exe::find`（B16）が PATH と `PATHEXT` を
//! 走査して実体を返すので、それを直接起動する。ログインシェルの rc に相当するものが
//! 無いため、環境変数は `Command::env` / `env_remove` だけで**確定する**
//! （POSIX 側でシェル片へ `unset` / `export` を前置きしているのは、rc がコマンド行より
//! 先に走って `Command::env` を上書きしうるから。#500 / #512 と同型）。

/// 子プロセスの起動計画（純粋データ。**macOS 上から Windows 側の組み立てをテストできる**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCmd {
    /// [`std::process::Command::new`] に渡すプログラム
    pub program: String,
    /// 渡す引数
    pub args: Vec<String>,
}

impl ChildCmd {
    /// 実行用の [`std::process::Command`] を組む。
    ///
    /// GUI から数秒おきに呼ばれる経路があるので、コンソールウィンドウは出さない（#586）
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        super::process::no_console_window(&mut command);
        command
    }
}

/// ユーザーの環境で CLI を 1 回走らせる子プロセスの計画。解決できなければ `None`。
///
/// **同じ意図を 2 通りで受け取る**（片方しか使われない）:
///
/// | 引数 | 使う側 |
/// |---|---|
/// | `posix_snippet` | unix（`<$SHELL> -l -c <シェル片>`） |
/// | `program` / `args` | Windows（PATH で解決した実体を直接起動） |
///
/// 2 通りに分かれるのは、POSIX 側だけが「rc に勝つための env 前置き」をシェル片へ
/// 埋める必要があるため（モジュールの説明参照）。ずれると気づきにくいので、
/// 呼び出し側では**同じ式の中に並べて書く**こと
pub fn user_env_cli(posix_snippet: &str, program: &str, args: &[&str]) -> Option<ChildCmd> {
    imp::user_env_cli(posix_snippet, program, args)
}

#[cfg(unix)]
mod imp {
    use super::ChildCmd;

    pub(crate) fn user_env_cli(
        posix_snippet: &str,
        _program: &str,
        _args: &[&str],
    ) -> Option<ChildCmd> {
        Some(super::login_shell_snippet(&user_shell(), posix_snippet))
    }

    /// `platform::shell` と同じ解決（あちらは PTY 用なので `-l` 付きの
    /// [`crate::terminal::SpawnCommand`] を返す。ここが要るのは実行ファイル名だけ）
    fn user_shell() -> String {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

#[cfg(windows)]
mod imp {
    use super::ChildCmd;

    pub(crate) fn user_env_cli(
        _posix_snippet: &str,
        program: &str,
        args: &[&str],
    ) -> Option<ChildCmd> {
        Some(super::direct_program(
            &crate::platform::exe::find(program)?,
            args,
        ))
    }
}

/// ログインシェルへシェル片を渡す形（純粋関数）。
///
/// `-l` はユーザーの profile を読ませるため（`.app` の痩せた PATH 対策）で、
/// **この形が macOS の従来の挙動そのもの**。1 バイトも変えない
#[cfg_attr(not(unix), allow(dead_code))]
fn login_shell_snippet(shell: &str, snippet: &str) -> ChildCmd {
    ChildCmd {
        program: shell.to_string(),
        args: vec!["-l".into(), "-c".into(), snippet.to_string()],
    }
}

/// 解決済みの実体を直接起動する形（純粋関数。**macOS 上でもテストできる**）
#[cfg_attr(not(windows), allow(dead_code))]
fn direct_program(resolved: &str, args: &[&str]) -> ChildCmd {
    ChildCmd {
        program: resolved.to_string(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posixはログインシェルへシェル片を渡す() {
        let got = login_shell_snippet("/bin/zsh", "unset CLAUDE_CONFIG_DIR; claude agents --json");
        assert_eq!(got.program, "/bin/zsh");
        assert_eq!(
            got.args,
            vec![
                "-l".to_string(),
                "-c".to_string(),
                "unset CLAUDE_CONFIG_DIR; claude agents --json".to_string(),
            ]
        );
    }

    #[test]
    fn windowsは実体を直接起動しシェル片を使わない() {
        let got = direct_program(
            "C:\\Users\\u\\.local\\bin\\claude.exe",
            &["agents", "--json"],
        );
        assert_eq!(got.program, "C:\\Users\\u\\.local\\bin\\claude.exe");
        assert_eq!(got.args, vec!["agents".to_string(), "--json".to_string()]);
        // 前置き（unset / export）はシェルが無いので現れない。env は Command::env が正
        assert!(
            !got.args
                .iter()
                .any(|a| a.contains("unset") || a.contains("export")),
            "Windows 側の argv に POSIX の env 前置きが混ざっている: {:?}",
            got.args
        );
    }

    #[test]
    fn 引数が無くても組める() {
        let got = direct_program("claude.exe", &[]);
        assert_eq!(got.program, "claude.exe");
        assert!(got.args.is_empty());
    }

    /// unix の実機では従来どおりログインシェル経由（`.app` の痩せた PATH 対策）。
    /// ここを直接起動へ倒すと Homebrew / npm 導入の CLI が Dock 起動で全滅する
    #[cfg(unix)]
    #[test]
    fn unixの実機ではログインシェル経由になる() {
        let plan = user_env_cli("echo tako877", "echo", &["tako877"]).expect("計画を組める");
        assert_eq!(
            plan.args,
            vec![
                "-l".to_string(),
                "-c".to_string(),
                "echo tako877".to_string()
            ],
            "ログインシェルへシェル片を渡す形が崩れている: {plan:?}"
        );
    }

    /// **Windows の実機でしか検出できない不変条件**（#877）: シェルを起こさない。
    ///
    /// 従来は `$SHELL -l -c <片>` の直書きで、`SHELL` 未設定なら `/bin/sh` へ落ちて
    /// CreateProcess が失敗し、`SHELL=powershell.exe` でも `-l` が不明な引数になった
    /// （2026-08-21 実機実測）。macOS のゲートは全部緑のままなのでここで固定する
    #[cfg(windows)]
    #[test]
    fn windowsの実機ではposixシェルを経由しない() {
        let plan = user_env_cli("echo tako877", "cmd", &["/C", "echo tako877"])
            .expect("cmd は必ず解決できる");
        assert!(
            !plan.args.iter().any(|a| a == "-l" || a == "-c"),
            "POSIX シェルの引数が残っている: {plan:?}"
        );
        assert!(
            plan.program.to_ascii_lowercase().contains("cmd"),
            "実体が PATH から解決されていない: {}",
            plan.program
        );
        assert_eq!(
            plan.args,
            vec!["/C".to_string(), "echo tako877".to_string()],
            "引数がそのまま渡っていない: {plan:?}"
        );
    }

    /// PATH に無いコマンドは解決できない（Windows）。呼び出し側が「走査失敗」として
    /// 画面推定へフォールバックできるよう `None` を返す
    #[cfg(windows)]
    #[test]
    fn windowsで実体が無ければnoneを返す() {
        assert!(user_env_cli("true", "tako-no-such-cli-877", &[]).is_none());
    }

    /// この環境で必ず解決できるコマンドを実際に走らせる（両プラットフォームの実装が動く証明）
    #[test]
    fn 実環境で子プロセスを起こせる() {
        let (snippet, program, args) = if cfg!(windows) {
            ("echo tako877", "cmd", vec!["/C", "echo tako877"])
        } else {
            ("echo tako877", "echo", vec!["tako877"])
        };
        let plan = user_env_cli(snippet, program, &args).expect("計画を組めない");
        let out = plan.command().output().expect("子プロセスを起こせない");
        assert!(out.status.success(), "終了コードが非ゼロ: {:?}", out.status);
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("tako877"),
            "出力が届いていない: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
