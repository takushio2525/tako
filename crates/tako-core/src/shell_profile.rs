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
//! sh = `.profile` / fish = `config.fish`）へ書く。これならユーザーの端末（macOS の Terminal・iTerm2 は
//! 既定でログインシェル）と tako 自身の解決の**両方**に効く。
//!
//! sh を bash と分けるのは #2592 の実測（2026-11-01・一時 HOME の全 profile に印を置いて `-l -c` で起こす）:
//! bash は argv[0] の名前が `sh` だと sh モードで起き、ログインで `.profile` **だけ**を読む
//! （macOS の `/bin/sh` は bash 3.2 の sh モード。dash・sh の名前で起こした zsh も `.profile`。名前が
//! `bash` なら `/bin/sh` を指す symlink でも `.bash_profile`）。tako の問い合わせも `$SHELL` のパスのまま
//! 起こすので、判定は実行ファイルの名前で揃う
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

/// #2592 の A/B の env（`1` で名前 `sh` のシェルを bash と見なし、`.bash_profile` へ書く = #2592 前の形。
/// `sh -l` は `.profile` だけを読むので、書いた後も新しいターミナルの PATH に置き場所が見えない）
pub const ISSUE2592_LEGACY_ENV: &str = "TAKO_2592_LEGACY";

/// [`ISSUE2592_LEGACY_ENV`] が立っているか（#2592 の A/B のゲートはここ 1 か所）
pub fn issue2592_legacy() -> bool {
    std::env::var(ISSUE2592_LEGACY_ENV).is_ok_and(|v| v == "1")
}

/// profile の書式を決めるシェル種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    /// 名前が `sh` のシェル（macOS は bash の sh モード・Linux は dash ほか）。行の形は bash と同じ
    /// POSIX の `case` / `export` で、書き先だけが違う（#2592）
    Sh,
    Fish,
    PowerShell,
}

impl ShellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Sh => "sh",
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
            "bash" => Some(Self::Bash),
            "sh" if issue2592_legacy() => Some(Self::Bash),
            "sh" => Some(Self::Sh),
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
            // sh のログインが読むのは `.profile` だけ（モジュール冒頭の #2592 の実測）
            Self::Sh => ".profile",
            Self::Fish => ".config/fish/config.fish",
            // PowerShell の `$PROFILE.CurrentUserAllHosts`（#525 で実パスを解決する）
            Self::PowerShell => "Documents/PowerShell/profile.ps1",
        }
    }

    /// 書き先を移す前にこの種別の利用者へ書いていた profile（`$HOME` 相対）。`undo-path` が併せて
    /// tako のブロックを外す（移した後の書き先だけを外すと、移す前に置いたブロックが二度と消えない）。
    ///
    /// sh は #2592 より前 bash と見なして `.bash_profile` へ書いていた
    pub fn previous_profile_rel(self) -> Option<&'static str> {
        match self {
            Self::Sh => Some(Self::Bash.login_profile_rel()),
            Self::Zsh | Self::Bash | Self::Fish | Self::PowerShell => None,
        }
    }

    /// ブロックの 1 行目に置く説明。**ディレクトリの数によらず同じ 1 行**
    /// （tako が入れるのはエージェント CLI のランチャーと tako 自身の CLI。#1502）
    const NOTE: &'static str =
        "# Managed by `tako setup`. Adds tako-managed command directories to PATH.";

    /// PATH へ 1 ディレクトリ足す本文（二重追加ガード付き）。
    ///
    /// `dir_expr` は「そのシェルでディレクトリを指す式」（`$HOME/.local/bin` 等）
    fn stanza(self, dir_expr: &str) -> String {
        match self {
            // `case` は sh / bash / zsh 共通
            Self::Zsh | Self::Bash | Self::Sh => format!(
                "case \":$PATH:\" in\n\
                 \x20 *\":{dir_expr}:\"*) ;;\n\
                 \x20 *) export PATH=\"{dir_expr}:$PATH\" ;;\n\
                 esac\n"
            ),
            Self::Fish => format!(
                "if not contains {dir_expr} $PATH\n\
                 \x20   set -gx PATH {dir_expr} $PATH\n\
                 end\n"
            ),
            // `dir_expr` は**そのまま埋めない**。PowerShell 5.1 は BOM 無しの `.ps1` を
            // ANSI コードページ（日本語環境なら CP932）で読むので、非 ASCII が 1 バイトでも
            // 混ざるとユーザーの `$PROFILE` 全体がパースエラーで壊れる（実機で観測済み。
            // `shell_integration` の同じ注意書きを参照）。home 配下でないディレクトリは
            // 絶対パスが入るため、ユーザー名が非 ASCII だと現実に踏む
            Self::PowerShell => format!(
                "$__takoPathDir = {dir_expr}\n\
                 if ($env:PATH -notlike \"*$__takoPathDir*\") {{ $env:PATH = \"$__takoPathDir;\" + $env:PATH }}\n"
            ),
        }
    }

    /// PATH へ N ディレクトリ足すブロック本文（末尾は改行 1 個）。
    ///
    /// **マーカーは 1 組のまま**で、中に [`Self::stanza`] を並べる（#1502）。
    /// ディレクトリが 1 個のときの出力は説明行を除いて #1502 前と同一なので、
    /// 既存ユーザーの profile は形が変わらない
    fn block(self, dir_exprs: &[String]) -> String {
        let body: String = dir_exprs.iter().map(|e| self.stanza(e)).collect();
        format!("{BLOCK_BEGIN}\n{}\n{body}{BLOCK_END}\n", Self::NOTE)
    }

    /// そのシェルの構文で `dir` を指す式。home 配下なら `$HOME` 相対にして可搬にする
    fn dir_expr(self, dir: &Path, home: &Path) -> String {
        let rel = dir.strip_prefix(home).ok();
        match (self, rel) {
            // どちらの枝も [`crate::shell_integration::powershell_ascii_literal`] を通す
            // （非 ASCII は `[char]0xNNNN` へ逃げる）。実装は 1 本だけ持つ
            (Self::PowerShell, Some(rel)) => format!(
                "Join-Path $HOME {}",
                crate::shell_integration::powershell_ascii_literal(
                    &rel.to_string_lossy().replace('/', "\\")
                )
            ),
            (Self::PowerShell, None) => crate::shell_integration::powershell_ascii_literal(
                &dir.to_string_lossy().replace('/', "\\"),
            ),
            (_, _) => home_relative_expr(dir, home),
        }
    }
}

/// `dir` を指す `$HOME` 相対の式（home 配下でなければ絶対パス。区切りは `/`）。
/// sh 系と fish の [`ShellKind::dir_expr`] と、種別の分からないシェルへの案内
/// （[`dirs_for_unknown_shell`]）が同じ 1 本を通る
fn home_relative_expr(dir: &Path, home: &Path) -> String {
    match dir.strip_prefix(home) {
        Ok(rel) => format!("$HOME/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => dir.to_string_lossy().replace('\\', "/"),
    }
}

/// 種別の分からないシェル（nu・tcsh 等）の利用者へ示す「PATH へ足すディレクトリ」の並び（#2557）。
///
/// 書く行と同じ `$HOME/…` の式で組み、順序を保って重複を落とす（macOS は tako CLI と
/// 3 系統のランチャーがみな `~/.local/bin` = 1 個になる）。**そのシェルの構文は推測しない**:
/// 行の形（`export` / `setenv` / `$env.PATH`）を決めるのは利用者で、ここはディレクトリだけを返す。
/// #2557 より前は `~` 短縮の表示を sh の `export PATH="…"` に包み、重複も並べていた
/// （sh / bash / zsh は二重引用符の中の `~` を展開しない = 文字どおりの `~/.local/bin` が PATH に入る。
/// tcsh は展開するが、`export` が無いので行ごと通らない。2026-10-31 実測）
pub fn dirs_for_unknown_shell(home: &Path, dirs: &[PathBuf]) -> Vec<String> {
    dedup(dirs)
        .iter()
        .map(|d| home_relative_expr(d, home))
        .collect()
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
    /// ブロックが通しているディレクトリ（**マーカーは 1 組**のまま中に並ぶ。#1502）
    pub dirs: Vec<PathBuf>,
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
    ensure_dirs_on_path_in(
        home,
        shell,
        std::slice::from_ref(&dir.to_path_buf()),
        current_path,
    )
}

/// 複数ディレクトリ版（#1502）。**マーカーブロックは 1 組**のまま、その中に
/// ディレクトリごとのガード付き追記を並べる。
///
/// `dirs` は**この環境で tako が通したいディレクトリの全部**を渡す
/// （呼び出しごとに部分集合を渡すと、前回入れた分が消える）。順序はそのまま保つ。
/// すべてが既に PATH にあるときだけ [`PathChange::AlreadyOnPath`] を返す
pub fn ensure_dirs_on_path_in(
    home: &Path,
    shell: ShellKind,
    dirs: &[PathBuf],
    current_path: Option<&str>,
) -> Result<EnsureOutcome, String> {
    ensure_dirs_on_path_in_typed(home, shell, dirs, current_path).map_err(PathBlockError::into_ja)
}

/// [`ensure_dirs_on_path_in`] の型の口（#2365。断りを表示言語で返す dispatch の経路が通る）
pub fn ensure_dirs_on_path_in_typed(
    home: &Path,
    shell: ShellKind,
    dirs: &[PathBuf],
    current_path: Option<&str>,
) -> Result<EnsureOutcome, PathBlockError> {
    let profile = home.join(shell.login_profile_rel());
    let dirs: Vec<PathBuf> = dedup(dirs);
    if dirs.is_empty() {
        return Err(PathBlockError::NoDirs);
    }
    if current_path.is_some_and(|p| dirs.iter().all(|d| path_contains(p, d))) {
        return Ok(EnsureOutcome {
            shell,
            profile,
            dirs,
            change: PathChange::AlreadyOnPath,
        });
    }
    let exprs: Vec<String> = dirs.iter().map(|d| shell.dir_expr(d, home)).collect();
    let block = shell.block(&exprs);
    let original = read_bytes(&profile)?;
    let had_block = MARKERS.present(&original);
    let updated = MARKERS.apply(&original, &block);
    if updated == original {
        return Ok(EnsureOutcome {
            shell,
            profile,
            dirs,
            change: PathChange::Unchanged,
        });
    }
    write_bytes(&profile, &updated)?;
    Ok(EnsureOutcome {
        shell,
        profile,
        dirs,
        change: if had_block {
            PathChange::Updated
        } else {
            PathChange::Installed
        },
    })
}

/// 順序を保ったまま重複を落とす（macOS は 3 系統とも `~/.local/bin` なので
/// tako CLI の置き場所と合わせても**実際に並ぶのは 1 個**になる）
fn dedup(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(dirs.len());
    for dir in dirs {
        if !out.contains(dir) {
            out.push(dir.clone());
        }
    }
    out
}

/// 置いたブロックを取り除く（元のバイト列へ完全に戻す）
pub fn remove_from_profile_in(home: &Path, shell: ShellKind) -> Result<EnsureOutcome, String> {
    remove_from_profile_in_typed(home, shell).map_err(PathBlockError::into_ja)
}

/// [`remove_from_profile_in`] の型の口（#2365）
pub fn remove_from_profile_in_typed(
    home: &Path,
    shell: ShellKind,
) -> Result<EnsureOutcome, PathBlockError> {
    remove_block_at(shell, home.join(shell.login_profile_rel()))
}

/// 書き先を移す前の profile（[`ShellKind::previous_profile_rel`]）に残ったブロックを取り除く（#2592）。
/// 移していない種別は `None`（ファイルを読みもしない）
pub fn remove_from_previous_profile_in_typed(
    home: &Path,
    shell: ShellKind,
) -> Result<Option<EnsureOutcome>, PathBlockError> {
    shell
        .previous_profile_rel()
        .map(|rel| remove_block_at(shell, home.join(rel)))
        .transpose()
}

/// `profile` から tako のブロックを取り除く（元のバイト列へ完全に戻す。無ければ触らない）
fn remove_block_at(shell: ShellKind, profile: PathBuf) -> Result<EnsureOutcome, PathBlockError> {
    let original = read_bytes(&profile)?;
    if !MARKERS.present(&original) {
        return Ok(EnsureOutcome {
            shell,
            profile,
            dirs: Vec::new(),
            change: PathChange::Absent,
        });
    }
    let updated = MARKERS.remove(&original);
    write_bytes(&profile, &updated)?;
    Ok(EnsureOutcome {
        shell,
        profile,
        dirs: Vec::new(),
        change: PathChange::Removed,
    })
}

/// profile に tako の PATH ブロックが入っているか
pub fn profile_has_block(profile: &Path) -> bool {
    std::fs::read(profile).is_ok_and(|bytes| MARKERS.present(&bytes))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, PathBlockError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(PathBlockError::Read {
            path: path.display().to_string(),
            error: e.to_string(),
        }),
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), PathBlockError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PathBlockError::CreateDir {
            path: parent.display().to_string(),
            error: e.to_string(),
        })?;
    }
    std::fs::write(path, bytes).map_err(|e| PathBlockError::Write {
        path: path.display().to_string(),
        error: e.to_string(),
    })
}

/// profile の PATH ブロックの書き換えの断り（#2365）。
///
/// 日英の対は [`PathBlockError::text_in`] の (バリアント, 言語) の網羅 match の 1 か所。日本語は #2365 より前の
/// 固定文とバイト一致（`tako-control/tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
/// `String` を返す口（[`ensure_dir_on_path_in`] / [`ensure_dirs_on_path_in`] / [`remove_from_profile_in`]）は
/// 日本語のまま（CLI の `tako setup` の段は枠が日本語）。`error` は OS のエラー文（言語に依らずそのまま載せる）、
/// パスは表記のまま埋める
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathBlockError {
    /// 通すディレクトリが 1 つも無い
    NoDirs,
    /// profile を読めない（無いのは空として扱うので、ここへは来ない）
    Read { path: String, error: String },
    /// profile の置き場（親ディレクトリ）を作れない
    CreateDir { path: String, error: String },
    /// profile へ書けない
    Write { path: String, error: String },
}

impl PathBlockError {
    /// 表示言語を明示しての文言（網羅 match = バリアントを足すと日英の両方を書くまで通らない）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::NoDirs, Ja) => "PATH へ通すディレクトリが 1 つもありません".to_string(),
            (Self::NoDirs, En) => "there is no directory to add to PATH".to_string(),
            (Self::Read { path, error }, Ja) => format!("{path} を読めません: {error}"),
            (Self::Read { path, error }, En) => format!("cannot read {path}: {error}"),
            (Self::CreateDir { path, error }, Ja) => format!("{path} を作成できません: {error}"),
            (Self::CreateDir { path, error }, En) => format!("cannot create {path}: {error}"),
            (Self::Write { path, error }, Ja) => format!("{path} へ書けません: {error}"),
            (Self::Write { path, error }, En) => format!("cannot write to {path}: {error}"),
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(crate::i18n::Lang::Ja)
    }
}

impl std::fmt::Display for PathBlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

impl std::error::Error for PathBlockError {}

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

    /// #2592: 名前 `sh` は bash と分ける。sh のログインが読むのは `.profile` だけ
    /// （本物のシェルとの突き合わせは `tako-control/tests/issue2592_sh_profile_watchdog.rs`）
    #[test]
    fn shはログインで読まれるprofileを選び行の形はbashと同じ() {
        for shell in ["/bin/sh", "/usr/bin/sh", "sh"] {
            assert_eq!(
                ShellKind::from_shell_path(shell),
                Some(ShellKind::Sh),
                "{shell}"
            );
        }
        assert_eq!(ShellKind::Sh.as_str(), "sh");
        assert_eq!(ShellKind::Sh.login_profile_rel(), ".profile");
        assert_eq!(ShellKind::Sh.previous_profile_rel(), Some(".bash_profile"));
        for kind in [
            ShellKind::Zsh,
            ShellKind::Bash,
            ShellKind::Fish,
            ShellKind::PowerShell,
        ] {
            assert_eq!(kind.previous_profile_rel(), None, "{kind:?}");
        }
        let exprs = ["$HOME/.local/bin".to_string()];
        assert_eq!(
            ShellKind::Sh.block(&exprs),
            ShellKind::Bash.block(&exprs),
            "sh の行は bash と同じ POSIX の case / export"
        );
    }

    /// PATH の区切りは OS 依存（`:` / `;`）。**fixture も `std::env::join_paths` で
    /// 組む**（`:` 直書きだと Windows では 1 エントリに見えて必ず外れる。#1278）
    fn joined_path(entries: &[&str]) -> String {
        std::env::join_paths(entries.iter().map(Path::new))
            .expect("PATH を組める")
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn path判定は完全一致で行う() {
        let dir = Path::new("/home/u/.local/bin");
        assert!(path_contains(
            &joined_path(&["/usr/bin", "/home/u/.local/bin", "/bin"]),
            dir
        ));
        // 部分一致で誤検出しない
        assert!(!path_contains(
            &joined_path(&["/usr/bin", "/home/u/.local/bin2"]),
            dir
        ));
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
        let current = joined_path(&["/usr/bin", &dir.to_string_lossy()]);
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
            let block = shell.block(&[shell.dir_expr(&dir, home)]);
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

    /// **PowerShell 5.1 は BOM 無しの `.ps1` を ANSI（日本語環境なら CP932）で読む**ので、
    /// ブロックに非 ASCII が 1 バイトでも混ざるとユーザーの `$PROFILE` 全体が壊れる。
    ///
    /// 元の `全シェルのブロックが…` は home が `/home/u` の ASCII しか見ておらず、
    /// **非 ASCII のユーザー名（日本語 Windows では普通にある）を素通ししていた**。
    /// home 配下・home 配下でない の両方で escape が効くことを固定する
    #[test]
    fn powershellブロックは非asciiのパスでもasciiだけになる() {
        let home = Path::new(r"C:\Users\山田");
        for dir in [
            home.join(".local/bin"),                  // home 配下（相対部分は ASCII）
            PathBuf::from(r"D:\ツール\bin"),          // home 配下でない = 絶対パスが入る
            PathBuf::from(r"C:\Users\山田\bin ver2"), // 空白入り
        ] {
            let expr = ShellKind::PowerShell.dir_expr(&dir, home);
            let block = ShellKind::PowerShell.block(std::slice::from_ref(&expr));
            assert!(
                block.is_ascii(),
                "非 ASCII が残った（$PROFILE を壊す）: dir={} block={block}",
                dir.display()
            );
            // ASCII になった理由が「escape した」であって「落とした」でないことの確認。
            //
            // home 配下は非 ASCII 部分が `$HOME`（PowerShell が実行時に展開する）へ
            // くくり出されるので escape は要らない = `Join-Path` の形になる。
            // 絶対パスを丸ごと入れる枝だけが escape を必要とする
            let inlined_whole_path = !expr.starts_with("Join-Path");
            if inlined_whole_path && !dir.to_string_lossy().is_ascii() {
                assert!(
                    expr.contains("[char]0x"),
                    "非 ASCII が escape ではなく欠落している疑い: dir={} expr={expr}",
                    dir.display()
                );
            }
        }
    }

    /// 二重追加ガードは変数経由になっても効いていること（PowerShell）
    #[test]
    fn powershellブロックは変数経由でも二重追加ガードを持つ() {
        let home = Path::new("/home/u");
        let expr = ShellKind::PowerShell.dir_expr(&home.join(".local/bin"), home);
        let block = ShellKind::PowerShell.block(std::slice::from_ref(&expr));
        assert!(block.contains("$__takoPathDir = Join-Path $HOME"));
        assert!(block.contains("-notlike"), "二重追加ガードが消えた");
        assert!(block.contains("$env:PATH = \"$__takoPathDir;\" + $env:PATH"));
    }

    #[test]
    fn home配下でないディレクトリは絶対パスで書く() {
        let expr = ShellKind::Zsh.dir_expr(Path::new("/opt/tools/bin"), Path::new("/home/u"));
        assert_eq!(expr, "/opt/tools/bin");
    }

    /// #2557: 種別の分からないシェルへの案内は、書く行と同じ `$HOME/…` の式で・重複なしで・
    /// ディレクトリだけ（sh の `export` などの構文を推測して包まない）
    #[test]
    fn 未知のシェルへの案内は書く行と同じ式で重複なしのディレクトリだけ() {
        let home = Path::new("/home/u");
        let local = home.join(".local/bin");
        // macOS の形: tako CLI と 3 系統のランチャーがみな同じ置き場所 = 1 個
        assert_eq!(
            dirs_for_unknown_shell(
                home,
                &[local.clone(), local.clone(), local.clone(), local.clone()]
            ),
            vec!["$HOME/.local/bin".to_string()]
        );
        // 順序を保って重複だけ落とす。home 配下でなければ絶対パス
        let opt = PathBuf::from("/opt/tako/bin");
        let got = dirs_for_unknown_shell(home, &[opt.clone(), local.clone(), opt.clone()]);
        assert_eq!(
            got,
            vec!["/opt/tako/bin".to_string(), "$HOME/.local/bin".to_string()]
        );
        // 書く行（sh 系・fish）の式と同じ
        for kind in [ShellKind::Zsh, ShellKind::Bash, ShellKind::Fish] {
            assert_eq!(got[1], kind.dir_expr(&local, home), "{kind:?}");
            assert_eq!(got[0], kind.dir_expr(&opt, home), "{kind:?}");
        }
        for d in &got {
            assert!(
                !d.contains('~') && !d.contains('"') && !d.contains("PATH"),
                "{d}"
            );
        }
        // Windows の区切りでも `/` の式になる（文の表は cfg の外）
        let win_home = Path::new(r"C:\Users\winuser");
        let win_dir = PathBuf::from(r"C:\Users\winuser\.local\bin");
        assert_eq!(
            dirs_for_unknown_shell(win_home, &[win_dir]),
            if cfg!(windows) {
                vec!["$HOME/.local/bin".to_string()]
            } else {
                // unix の Path は `\` を区切りと見ないので home 配下と判定されない
                vec!["C:/Users/winuser/.local/bin".to_string()]
            }
        );
        assert!(dirs_for_unknown_shell(home, &[]).is_empty());
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

    /// #1502: tako CLI の置き場所を足してもマーカーは 1 組のまま。
    /// 1 ディレクトリのときの本文が 2 ディレクトリの前半と完全に一致することも見る
    /// （「並べただけ」= 既存ユーザーの profile の形を変えない）
    #[test]
    fn 複数ディレクトリでもマーカーブロックは一個() {
        let home = temp_home("multi");
        let a = home.join(".local/bin");
        let b = PathBuf::from("/opt/tako/bin");
        let out = ensure_dirs_on_path_in(
            &home,
            ShellKind::Zsh,
            &[a.clone(), b.clone()],
            Some("/usr/bin"),
        )
        .unwrap();
        assert_eq!(out.change, PathChange::Installed);
        assert_eq!(out.dirs, vec![a.clone(), b.clone()]);
        let text = std::fs::read_to_string(&out.profile).unwrap();
        assert_eq!(text.matches(BLOCK_BEGIN).count(), 1, "ブロックが増えている");
        assert_eq!(text.matches(BLOCK_END).count(), 1);
        assert!(text.contains("$HOME/.local/bin"));
        assert!(text.contains("/opt/tako/bin"));
        assert_eq!(
            text.matches("esac").count(),
            2,
            "ガードがディレクトリ数ぶん無い"
        );

        // 2 回目は無変更（冪等）
        let again = ensure_dirs_on_path_in(
            &home,
            ShellKind::Zsh,
            &[a.clone(), b.clone()],
            Some("/usr/bin"),
        )
        .unwrap();
        assert_eq!(again.change, PathChange::Unchanged);
        assert_eq!(std::fs::read_to_string(&out.profile).unwrap(), text);

        // 除去は 1 回で全部消える
        assert_eq!(
            remove_from_profile_in(&home, ShellKind::Zsh)
                .unwrap()
                .change,
            PathChange::Removed
        );
        assert!(!profile_has_block(&out.profile));
        cleanup(&home);
    }

    /// macOS は claude / codex / agy も tako CLI も `~/.local/bin` なので、
    /// 重複が落ちて**ブロックの中身は 1 ディレクトリのまま**になる（#1502）
    #[test]
    fn 同じディレクトリを重ねても一本にまとまる() {
        let home = temp_home("dedup");
        let dir = home.join(".local/bin");
        let out = ensure_dirs_on_path_in(
            &home,
            ShellKind::Zsh,
            &[dir.clone(), dir.clone(), dir.clone()],
            Some("/usr/bin"),
        )
        .unwrap();
        assert_eq!(out.dirs, vec![dir.clone()]);
        let text = std::fs::read_to_string(&out.profile).unwrap();
        assert_eq!(text.matches("esac").count(), 1);
        // 1 ディレクトリ版（従来 API）と本文が完全一致する
        let home2 = temp_home("dedup-single");
        let single = ensure_dir_on_path_in(
            &home2,
            ShellKind::Zsh,
            &home2.join(".local/bin"),
            Some("/usr/bin"),
        )
        .unwrap();
        let text2 = std::fs::read_to_string(&single.profile)
            .unwrap()
            .replace(&home2.display().to_string(), &home.display().to_string());
        assert_eq!(text, text2);
        cleanup(&home);
        cleanup(&home2);
    }

    /// 一部だけ PATH に在るなら書く（全部揃って初めて `AlreadyOnPath`）
    #[test]
    fn 一部しかpathに無いなら書く() {
        let home = temp_home("partial");
        let a = home.join(".local/bin");
        let b = PathBuf::from("/opt/tako/bin");
        let current = joined_path(&["/usr/bin", &a.to_string_lossy()]);
        let out = ensure_dirs_on_path_in(
            &home,
            ShellKind::Zsh,
            &[a.clone(), b.clone()],
            Some(&current),
        )
        .unwrap();
        assert_eq!(out.change, PathChange::Installed);
        let both = joined_path(&["/usr/bin", &a.to_string_lossy(), &b.to_string_lossy()]);
        let out2 = ensure_dirs_on_path_in(&home, ShellKind::Zsh, &[a, b], Some(&both)).unwrap();
        assert_eq!(out2.change, PathChange::AlreadyOnPath);
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
