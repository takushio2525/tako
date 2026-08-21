//! シェル profile への PATH 追記（#868）
//!
//! ゼロスタート導入では、エージェント CLI を入れた直後にその置き場所
//! （`~/.local/bin`）が PATH に無い。ここはその 1 点だけを、
//! **冪等に・元のバイト列へ完全に戻せる形で**足す。
//!
//! ## どのファイルへ書くか（2026-08-21 実測で決定）
//!
//! 公式 docs は zsh に `~/.zshrc` を案内しているが、**tako はそれでは足りない**。
//!
//! - `tako` のコマンド解決（[`crate::platform::exe::find`]）は
//!   `$SHELL -l -c "command -v <name>"` を使う。zsh の**非対話ログインシェル**は
//!   `.zshenv` / `.zprofile` / `.zlogin` を読み、**`.zshrc` は読まない**（対話専用）
//! - ペインのシェルも `-l` 付きで起動する（[`crate::platform::shell`]）
//! - 実測（この開発機）: `.zprofile` に `.local/bin` があり `zsh -l -c` から見えていた。
//!   `.zshrc` にだけ書くと `zsh -l -c` からは見えない
//!
//! よって **ログインシェルの profile**（zsh = `.zprofile` / bash = `.bash_profile` /
//! fish = `config.fish`）へ書く。これならユーザーの端末（macOS の Terminal・iTerm2 は
//! 既定でログインシェル）と tako 自身の解決の**両方**に効く。
//!
//! ## 書き換えの規則
//!
//! ブロックの読み書きは [`crate::text_block`] に委ねる（区切り改行 1 個・
//! 元バイト列への完全復帰の不変条件はそこが唯一の実装）。本文は **ASCII だけ**で書く。

use crate::text_block::BlockMarkers;
use std::path::{Path, PathBuf};

/// PATH 追記ブロックのマーカー。シェル統合（`# >>> tako shell integration >>>`）とは別物
const BLOCK_BEGIN: &str = "# >>> tako PATH >>>";
const BLOCK_END: &str = "# <<< tako PATH <<<";

const MARKERS: BlockMarkers = BlockMarkers::new(BLOCK_BEGIN, BLOCK_END);

/// profile の書式を決めるシェル種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
}

impl ShellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
            Self::PowerShell => "powershell",
        }
    }

    /// `$SHELL` の値（実行ファイルパス）から種別を判定する。
    /// 判定できないシェルは `None`（= 自動追記せず手順を案内する）
    pub fn from_shell_path(shell: &str) -> Option<Self> {
        let name = shell
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(shell)
            .trim_end_matches(".exe");
        match name {
            "zsh" => Some(Self::Zsh),
            "bash" | "sh" => Some(Self::Bash),
            "fish" => Some(Self::Fish),
            "pwsh" | "powershell" => Some(Self::PowerShell),
            _ => None,
        }
    }

    /// **ログインシェル**が読む profile の `$HOME` 相対パス。
    ///
    /// 対話専用のファイル（zsh の `.zshrc` / bash の `.bashrc`）は選ばない。
    /// 理由はモジュール冒頭の実測メモを参照
    pub fn login_profile_rel(self) -> &'static str {
        match self {
            Self::Zsh => ".zprofile",
            Self::Bash => ".bash_profile",
            Self::Fish => ".config/fish/config.fish",
            // PowerShell の `$PROFILE.CurrentUserAllHosts`（#525 で実パスを解決する）
            Self::PowerShell => "Documents/PowerShell/profile.ps1",
        }
    }

    /// PATH へ 1 ディレクトリ足すブロック本文（末尾は改行 1 個）。
    ///
    /// `dir_expr` は「そのシェルでディレクトリを指す式」（`$HOME/.local/bin` 等）
    fn block(self, dir_expr: &str) -> String {
        match self {
            // 二重追加を防ぐガード付き。`case` は sh / bash / zsh 共通
            Self::Zsh | Self::Bash => format!(
                "{BLOCK_BEGIN}\n\
                 # Managed by `tako setup`. Adds the Claude Code launcher directory to PATH.\n\
                 case \":$PATH:\" in\n\
                 \x20 *\":{dir_expr}:\"*) ;;\n\
                 \x20 *) export PATH=\"{dir_expr}:$PATH\" ;;\n\
                 esac\n\
                 {BLOCK_END}\n"
            ),
            Self::Fish => format!(
                "{BLOCK_BEGIN}\n\
                 # Managed by `tako setup`. Adds the Claude Code launcher directory to PATH.\n\
                 if not contains {dir_expr} $PATH\n\
                 \x20   set -gx PATH {dir_expr} $PATH\n\
                 end\n\
                 {BLOCK_END}\n"
            ),
            Self::PowerShell => format!(
                "{BLOCK_BEGIN}\n\
                 # Managed by `tako setup`. Adds the Claude Code launcher directory to PATH.\n\
                 if ($env:PATH -notlike \"*{dir_expr}*\") {{ $env:PATH = \"{dir_expr};\" + $env:PATH }}\n\
                 {BLOCK_END}\n"
            ),
        }
    }

    /// そのシェルの構文で `dir` を指す式。home 配下なら `$HOME` 相対にして可搬にする
    fn dir_expr(self, dir: &Path, home: &Path) -> String {
        let rel = dir.strip_prefix(home).ok();
        match (self, rel) {
            (Self::PowerShell, Some(rel)) => {
                format!("$HOME\\{}", rel.to_string_lossy().replace('/', "\\"))
            }
            (Self::PowerShell, None) => dir.to_string_lossy().replace('/', "\\"),
            (_, Some(rel)) => format!("$HOME/{}", rel.to_string_lossy().replace('\\', "/")),
            (_, None) => dir.to_string_lossy().replace('\\', "/"),
        }
    }
}

/// PATH 文字列に `dir` が含まれるか（区切りは OS 依存）
pub fn path_contains(path_var: &str, dir: &Path) -> bool {
    let sep = if cfg!(windows) { ';' } else { ':' };
    path_var
        .split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|entry| Path::new(entry) == dir)
}

/// 1 ファイルに対して行った（行わなかった）こと
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathChange {
    /// 新しく書いた
    Installed,
    /// 既存のブロックを現行内容へ差し替えた
    Updated,
    /// 既に同じ内容が入っていたので触っていない
    Unchanged,
    /// PATH に既にあるので何もしなかった
    AlreadyOnPath,
    /// ブロックを取り除いた
    Removed,
    /// もともと入っていなかった
    Absent,
}

impl PathChange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::AlreadyOnPath => "already_on_path",
            Self::Removed => "removed",
            Self::Absent => "absent",
        }
    }

    /// ファイルを書き換えたか
    pub fn wrote(self) -> bool {
        matches!(self, Self::Installed | Self::Updated | Self::Removed)
    }
}

/// PATH 追記の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureOutcome {
    pub shell: ShellKind,
    pub profile: PathBuf,
    pub dir: PathBuf,
    pub change: PathChange,
}

/// `dir` がログインシェルの PATH に入るよう profile へ冪等にブロックを置く。
///
/// `home` と `shell` を引数で受けるので、**隔離 HOME のテストがそのまま書ける**
/// （実ユーザーの profile を触らずに全経路を検証できる）。
///
/// `current_path` に現在の PATH を渡すと、既に通っている場合は
/// [`PathChange::AlreadyOnPath`] を返してファイルを触らない
pub fn ensure_dir_on_path_in(
    home: &Path,
    shell: ShellKind,
    dir: &Path,
    current_path: Option<&str>,
) -> Result<EnsureOutcome, String> {
    let profile = home.join(shell.login_profile_rel());
    if current_path.is_some_and(|p| path_contains(p, dir)) {
        return Ok(EnsureOutcome {
            shell,
            profile,
            dir: dir.to_path_buf(),
            change: PathChange::AlreadyOnPath,
        });
    }
    let block = shell.block(&shell.dir_expr(dir, home));
    let original = read_bytes(&profile)?;
    let had_block = MARKERS.present(&original);
    let updated = MARKERS.apply(&original, &block);
    if updated == original {
        return Ok(EnsureOutcome {
            shell,
            profile,
            dir: dir.to_path_buf(),
            change: PathChange::Unchanged,
        });
    }
    write_bytes(&profile, &updated)?;
    Ok(EnsureOutcome {
        shell,
        profile,
        dir: dir.to_path_buf(),
        change: if had_block {
            PathChange::Updated
        } else {
            PathChange::Installed
        },
    })
}

/// 置いたブロックを取り除く（元のバイト列へ完全に戻す）
pub fn remove_from_profile_in(home: &Path, shell: ShellKind) -> Result<EnsureOutcome, String> {
    let profile = home.join(shell.login_profile_rel());
    let original = read_bytes(&profile)?;
    if !MARKERS.present(&original) {
        return Ok(EnsureOutcome {
            shell,
            profile,
            dir: PathBuf::new(),
            change: PathChange::Absent,
        });
    }
    let updated = MARKERS.remove(&original);
    write_bytes(&profile, &updated)?;
    Ok(EnsureOutcome {
        shell,
        profile,
        dir: PathBuf::new(),
        change: PathChange::Removed,
    })
}

/// profile に tako の PATH ブロックが入っているか
pub fn profile_has_block(profile: &Path) -> bool {
    std::fs::read(profile).is_ok_and(|bytes| MARKERS.present(&bytes))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{} を読めません: {e}", path.display())),
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("{} を作成できません: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{} へ書けません: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-shell-profile-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **一時ディレクトリ配下であることを確認してから消す**
    /// （実ユーザーのホームを消した事故の再発防止）
    fn cleanup(dir: &Path) {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ以外を消そうとした: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn シェル種別を実行ファイルパスから判定する() {
        assert_eq!(ShellKind::from_shell_path("/bin/zsh"), Some(ShellKind::Zsh));
        assert_eq!(
            ShellKind::from_shell_path("/opt/homebrew/bin/fish"),
            Some(ShellKind::Fish)
        );
        assert_eq!(
            ShellKind::from_shell_path("/usr/local/bin/bash"),
            Some(ShellKind::Bash)
        );
        assert_eq!(
            ShellKind::from_shell_path(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(ShellKind::from_shell_path("/usr/bin/nu"), None);
    }

    /// 実測（モジュール冒頭）に基づく固定。`.zshrc` を選ぶと
    /// `zsh -l -c` から見えず tako が自分で入れた CLI を見つけられない
    #[test]
    fn zshはログインシェルが読むzprofileを選ぶ() {
        assert_eq!(ShellKind::Zsh.login_profile_rel(), ".zprofile");
        assert_ne!(ShellKind::Zsh.login_profile_rel(), ".zshrc");
        assert_eq!(ShellKind::Bash.login_profile_rel(), ".bash_profile");
        assert_ne!(ShellKind::Bash.login_profile_rel(), ".bashrc");
    }

    #[test]
    fn path判定は完全一致で行う() {
        let dir = Path::new("/home/u/.local/bin");
        assert!(path_contains("/usr/bin:/home/u/.local/bin:/bin", dir));
        // 部分一致で誤検出しない
        assert!(!path_contains("/usr/bin:/home/u/.local/bin2", dir));
        assert!(!path_contains("", dir));
    }

    #[test]
    fn 二回実行してもブロックは一個で内容も同じ() {
        let home = temp_home("idempotent");
        let dir = home.join(".local/bin");
        let first = ensure_dir_on_path_in(&home, ShellKind::Zsh, &dir, Some("/usr/bin")).unwrap();
        assert_eq!(first.change, PathChange::Installed);
        let after_first = std::fs::read(&first.profile).unwrap();

        let second = ensure_dir_on_path_in(&home, ShellKind::Zsh, &dir, Some("/usr/bin")).unwrap();
        assert_eq!(second.change, PathChange::Unchanged);
        assert_eq!(std::fs::read(&second.profile).unwrap(), after_first);

        let text = String::from_utf8(after_first).unwrap();
        assert_eq!(text.matches(BLOCK_BEGIN).count(), 1, "ブロックが増えている");
        assert!(
            text.contains("$HOME/.local/bin"),
            "home 相対で書かれていない"
        );
        cleanup(&home);
    }

    #[test]
    fn 既存内容を保ったまま追記し除去で完全に戻る() {
        let home = temp_home("roundtrip");
        let profile = home.join(".zprofile");
        std::fs::write(&profile, "# user's own\nexport FOO=1\n").unwrap();
        let original = std::fs::read(&profile).unwrap();

        let dir = home.join(".local/bin");
        ensure_dir_on_path_in(&home, ShellKind::Zsh, &dir, Some("/usr/bin")).unwrap();
        let text = std::fs::read_to_string(&profile).unwrap();
        assert!(text.starts_with("# user's own\nexport FOO=1\n"));

        let removed = remove_from_profile_in(&home, ShellKind::Zsh).unwrap();
        assert_eq!(removed.change, PathChange::Removed);
        assert_eq!(
            std::fs::read(&profile).unwrap(),
            original,
            "元へ戻っていない"
        );
        cleanup(&home);
    }

    #[test]
    fn 既にpathにあるならファイルを触らない() {
        let home = temp_home("already");
        let dir = home.join(".local/bin");
        let current = format!("/usr/bin:{}", dir.display());
        let out = ensure_dir_on_path_in(&home, ShellKind::Zsh, &dir, Some(&current)).unwrap();
        assert_eq!(out.change, PathChange::AlreadyOnPath);
        assert!(!out.profile.exists(), "profile を作ってしまっている");
        cleanup(&home);
    }

    #[test]
    fn 全シェルのブロックが二重追加ガードを持ちasciiだけで書かれる() {
        let home = Path::new("/home/u");
        let dir = home.join(".local/bin");
        for shell in [
            ShellKind::Zsh,
            ShellKind::Bash,
            ShellKind::Fish,
            ShellKind::PowerShell,
        ] {
            let block = shell.block(&shell.dir_expr(&dir, home));
            assert!(
                block.is_ascii(),
                "{}: 非 ASCII が混ざっている",
                shell.as_str()
            );
            assert!(block.ends_with('\n'));
            assert!(
                block.contains("$HOME"),
                "{}: home 相対になっていない",
                shell.as_str()
            );
            // 二重追加ガード（既に PATH にあれば足さない）を必ず持つ
            let guarded = block.contains("case ")
                || block.contains("not contains")
                || block.contains("-notlike");
            assert!(guarded, "{}: 二重追加ガードが無い", shell.as_str());
        }
    }

    #[test]
    fn home配下でないディレクトリは絶対パスで書く() {
        let expr = ShellKind::Zsh.dir_expr(Path::new("/opt/tools/bin"), Path::new("/home/u"));
        assert_eq!(expr, "/opt/tools/bin");
    }

    #[test]
    fn fishはネストしたprofileの親ディレクトリを作る() {
        let home = temp_home("fish");
        let dir = home.join(".local/bin");
        let out = ensure_dir_on_path_in(&home, ShellKind::Fish, &dir, Some("/usr/bin")).unwrap();
        assert_eq!(out.change, PathChange::Installed);
        assert!(out.profile.ends_with("config.fish"));
        assert!(out.profile.is_file());
        cleanup(&home);
    }

    #[test]
    fn ブロックが無いprofileへの除去は無変更() {
        let home = temp_home("absent");
        let out = remove_from_profile_in(&home, ShellKind::Zsh).unwrap();
        assert_eq!(out.change, PathChange::Absent);
        cleanup(&home);
    }
}
