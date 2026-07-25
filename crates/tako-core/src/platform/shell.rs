//! 既定シェルの解決（抽象境界 B1）
//!
//! 「ペインで何を起動するか」「明示コマンドをどう包むか」のプラットフォーム差を閉じ込める。
//! PTY そのもの（unix pty / ConPTY）は `alacritty_terminal::tty` が吸収するため、
//! ここでは扱わない（設計 §2.2）。

use crate::terminal::SpawnCommand;

/// 既定シェル。`None` を返すとペインを spawn できない
pub(crate) fn default_shell() -> Option<SpawnCommand> {
    imp::default_shell()
}

/// 明示コマンド（`tako split -- <command>` 等）を、ユーザーの環境で実行される形に包む
pub fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
    imp::login_shell_command(command)
}

#[cfg(unix)]
mod imp {
    use super::*;

    /// unix では alacritty に `None` を渡さず**ここで明示解決する**。
    ///
    /// alacritty の既定（None）は macOS で setuid root の `login` ラッパ経由になり、
    /// ペイン close 時の `Pty::drop` が `kill(login, SIGHUP)` を権限エラーで失敗（返り値無視）
    /// → `child.wait()` が永久ブロック → master fd・signal fd・IO スレッド・login プロセスが
    /// **close のたびにリーク**する。fd 枯渇で PTY 生成が失敗し日常使用でアプリが死ぬ
    /// （2026-06-11 常用報告の根本原因）。本家 alacritty はウィンドウ close = プロセス終了の
    /// ため顕在化しないが、tako はペイン単位でセッションを破棄するので直撃する。
    /// ユーザー権限のシェルを直接 spawn すれば SIGHUP が届き wait も即返る。
    /// `-l` でログインシェル動作（profile 読み込み）は維持する
    pub(crate) fn default_shell() -> Option<SpawnCommand> {
        Some(SpawnCommand {
            program: user_shell(),
            args: vec!["-l".into()],
        })
    }

    /// .app（Dock 起動）のプロセス環境は PATH が最小構成（/usr/bin:/bin:…）のため、
    /// コマンドを直接 exec すると Homebrew の `tmux` や `npm` が見つからず PTY 生成に
    /// 失敗する（2026-06-12 実機リグレッション）。`$SHELL -l -c "<コマンド>"` にして
    /// ユーザーの PATH・環境変数で実行する
    pub(crate) fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
        SpawnCommand {
            program: user_shell(),
            args: vec![
                "-l".into(),
                "-c".into(),
                crate::tmux_backend::shell_quoted(&command),
            ],
        }
    }

    fn user_shell() -> String {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    pub(crate) fn default_shell() -> Option<SpawnCommand> {
        Some(SpawnCommand {
            program: super::resolve_windows_shell(
                env_string("ProgramFiles"),
                env_string("SystemRoot"),
                env_string("ComSpec"),
                &|p| std::path::Path::new(p).exists(),
            ),
            // 引数は付けない。実機で banner の見え方を確認してから調整する（#517）
            args: Vec::new(),
        })
    }

    /// Windows の `CreateProcess` は PATH 探索を行うため、単一コマンドはそのまま起動できる。
    /// 複合シェル構文（`a && b` 等）の取り扱いはシェル統合と合わせて詰める（#525）
    pub(crate) fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
        command
    }

    /// 非 UTF-8 の環境変数値は解決に使えないため落とす（後段のフォールバックへ回す）
    fn env_string(key: &str) -> Option<String> {
        std::env::var_os(key).and_then(|v| v.into_string().ok())
    }
}

/// Windows の既定シェル解決（純粋関数。**macOS 上でもテストできる**ようにしてある）。
///
/// 優先順は「新しい PowerShell → 同梱の Windows PowerShell → `%ComSpec%` → `cmd.exe`」。
/// PowerShell を優先するのは、Windows の既定ターミナル体験に合わせるため
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_shell(
    program_files: Option<String>,
    system_root: Option<String>,
    com_spec: Option<String>,
    exists: &dyn Fn(&str) -> bool,
) -> String {
    // PowerShell 7 以降（別途インストール）
    if let Some(pf) = program_files.as_deref().filter(|s| !s.is_empty()) {
        let pwsh = format!("{pf}\\PowerShell\\7\\pwsh.exe");
        if exists(&pwsh) {
            return pwsh;
        }
    }
    // Windows 同梱の Windows PowerShell 5.1
    if let Some(root) = system_root.as_deref().filter(|s| !s.is_empty()) {
        let ps = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        if exists(&ps) {
            return ps;
        }
    }
    // 最後の砦。%ComSpec% は通常 cmd.exe を指す
    com_spec
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "cmd.exe".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pf() -> Option<String> {
        Some("C:\\Program Files".into())
    }
    fn sr() -> Option<String> {
        Some("C:\\Windows".into())
    }

    #[test]
    fn pwsh7があればそれを最優先する() {
        let got = resolve_windows_shell(
            pf(),
            sr(),
            Some("C:\\Windows\\system32\\cmd.exe".into()),
            &|_p| true,
        );
        assert_eq!(got, "C:\\Program Files\\PowerShell\\7\\pwsh.exe");
    }

    #[test]
    fn pwsh7が無ければ同梱のpowershellへ落ちる() {
        let got = resolve_windows_shell(pf(), sr(), None, &|p| !p.contains("PowerShell\\7"));
        assert_eq!(
            got,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
    }

    #[test]
    fn powershellが無ければcomspecを使う() {
        let got = resolve_windows_shell(
            pf(),
            sr(),
            Some("C:\\Windows\\system32\\cmd.exe".into()),
            &|_p| false,
        );
        assert_eq!(got, "C:\\Windows\\system32\\cmd.exe");
    }

    #[test]
    fn 環境変数が空でもcmdへ落ちる() {
        // サービス起動などで環境が痩せていても spawn 不能（None）にはしない
        let got =
            resolve_windows_shell(Some(String::new()), None, Some(String::new()), &|_p| false);
        assert_eq!(got, "cmd.exe");
    }
}
