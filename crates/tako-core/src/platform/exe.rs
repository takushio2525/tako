//! 実行ファイルの探索（抽象境界 B16）
//!
//! 「コマンド名から実行ファイルを見つける」ことのプラットフォーム差を閉じ込める。
//!
//! ## なぜ境界が要るか（#525）
//!
//! 従来の実装は各所で `$SHELL -l -c "command -v <name>"` を直接叩いていた。
//! **macOS ではこれでないと困る**: `.app` を Dock から起動するとプロセスの PATH が
//! 最小構成（`/usr/bin:/bin:…`）になり、Homebrew や `~/.local/bin` のコマンドが
//! 一切見つからない。ログインシェルを経由して初めてユーザーの PATH で解決できる。
//!
//! 一方 **Windows には `$SHELL` も `command -v` も無い**ので、この実装は例外なく
//! `None` を返す。実測（2026-07-27・Windows 11）では claude / git が導入済みでも
//! `tako setup` が「claude / codex / agy のいずれも見つかりません」で停止した。
//!
//! ## Windows 側の作法
//!
//! - PATH を `PATHEXT` の拡張子と組み合わせて走査する（`where.exe` と同じ意味論。
//!   外部プロセスを起こさないのでコンソールウィンドウが明滅しない）
//! - PATH に無くても**ユーザーが手で入れがちな場所**を追って探す。Windows は
//!   インストーラが PATH を書き換えても**再ログインするまで実行中プロセスへ伝播しない**。
//!   「入れたのに見つからない」を避けるための保険

/// コマンド名から実行ファイルの絶対パスを解決する。見つからなければ `None`。
///
/// 返り値はそのまま [`std::process::Command::new`] に渡せる
/// （Windows の `.cmd` / `.bat` シムも Rust 標準ライブラリが解釈する）
pub fn find(name: &str) -> Option<String> {
    imp::find(name)
}

#[cfg(unix)]
mod imp {
    /// ログインシェル経由で探す。`.app`（Dock 起動）の痩せた PATH でも
    /// ユーザーの PATH で解決できるようにするため（この経路を外すと Homebrew が全滅する）
    pub fn find(name: &str) -> Option<String> {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let output = std::process::Command::new(shell)
            .args(["-l", "-c", &format!("command -v {name}")])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }
}

#[cfg(windows)]
mod imp {
    pub fn find(name: &str) -> Option<String> {
        super::find_in_windows_path(
            name,
            &split_path_list(std::env::var_os("PATH")),
            &pathext(),
            &user_install_dirs(),
            &|p| std::path::Path::new(p).is_file(),
        )
    }

    fn split_path_list(value: Option<std::ffi::OsString>) -> Vec<String> {
        value
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default()
            .split(';')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// `PATHEXT` は通常 `.COM;.EXE;.BAT;.CMD;…`。並び順がそのまま優先順位になる
    /// （`.exe` が `.cmd` シムより先に来るのはこの順序のおかげ）
    fn pathext() -> Vec<String> {
        let configured = split_path_list(std::env::var_os("PATHEXT"));
        if configured.is_empty() {
            [".COM", ".EXE", ".BAT", ".CMD"]
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            configured
        }
    }

    /// PATH に載っていなくても探しに行く場所。
    /// 「インストールしたのに再ログインしていない」ケースを拾うための保険
    fn user_install_dirs() -> Vec<String> {
        let mut dirs = Vec::new();
        let mut push = |base: Option<std::ffi::OsString>, rel: &str| {
            if let Some(base) = base.filter(|b| !b.is_empty()) {
                dirs.push(std::path::Path::new(&base).join(rel).display().to_string());
            }
        };
        let home = || std::env::var_os("USERPROFILE");
        // claude ネイティブインストーラ
        push(home(), ".local\\bin");
        // scoop
        push(home(), "scoop\\shims");
        // npm のグローバルシム（claude / agy を npm で入れた場合）
        push(std::env::var_os("APPDATA"), "npm");
        // winget が張るシム
        push(std::env::var_os("LOCALAPPDATA"), "Microsoft\\WinGet\\Links");
        // Git for Windows の既定インストール先
        push(std::env::var_os("ProgramFiles"), "Git\\cmd");
        dirs
    }
}

/// Windows の PATH 探索（純粋関数。**macOS 上でもテストできる**ようにしてある）。
///
/// 各ディレクトリについて「名前そのまま → `PATHEXT` の各拡張子」の順に見る。
/// ディレクトリを外側・拡張子を内側にするのが Windows の探索順で、
/// これを逆にすると PATH の後方にある `.exe` が前方の `.cmd` に勝ってしまう
#[cfg_attr(not(windows), allow(dead_code))]
fn find_in_windows_path(
    name: &str,
    path_dirs: &[String],
    pathext: &[String],
    extra_dirs: &[String],
    is_file: &dyn Fn(&str) -> bool,
) -> Option<String> {
    // 区切りを含む場合はコマンド名ではなくパス指定。PATH 探索の対象外
    if name.contains('\\') || name.contains('/') {
        return is_file(name).then(|| name.to_string());
    }
    for dir in path_dirs.iter().chain(extra_dirs.iter()) {
        let base = dir.trim_end_matches(['\\', '/']);
        if base.is_empty() {
            continue;
        }
        let bare = format!("{base}\\{name}");
        if is_file(&bare) {
            return Some(bare);
        }
        for ext in pathext {
            // `PATHEXT` は慣習的に大文字（`.EXE`）。Windows のパスは大小を区別しないので
            // 解決には影響しないが、そのまま連結すると `git.EXE` という見慣れない
            // パスを表示することになるため小文字へ寄せる
            let candidate = format!("{bare}{}", ext.to_ascii_lowercase());
            if is_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn ext() -> Vec<String> {
        dirs(&[".COM", ".EXE", ".BAT", ".CMD"])
    }

    #[test]
    fn pathextを補って実行ファイルを見つける() {
        let got = find_in_windows_path(
            "git",
            &dirs(&["C:\\Program Files\\Git\\cmd"]),
            &ext(),
            &[],
            &|p| p == "C:\\Program Files\\Git\\cmd\\git.exe",
        );
        assert_eq!(got.as_deref(), Some("C:\\Program Files\\Git\\cmd\\git.exe"));
    }

    #[test]
    fn 同じディレクトリではpathextの順序が優先度になる() {
        // npm シム（.cmd）と実体（.exe）が同居しても .exe を選ぶ
        let got = find_in_windows_path("claude", &dirs(&["C:\\bin"]), &ext(), &[], &|p| {
            p == "C:\\bin\\claude.exe" || p == "C:\\bin\\claude.cmd"
        });
        assert_eq!(got.as_deref(), Some("C:\\bin\\claude.exe"));
    }

    #[test]
    fn pathに無ければユーザー導入先を追って探す() {
        // PATH 更新が実行中プロセスへ伝播していないケース
        let got = find_in_windows_path(
            "claude",
            &dirs(&["C:\\Windows\\System32"]),
            &ext(),
            &dirs(&["C:\\Users\\u\\.local\\bin"]),
            &|p| p == "C:\\Users\\u\\.local\\bin\\claude.exe",
        );
        assert_eq!(
            got.as_deref(),
            Some("C:\\Users\\u\\.local\\bin\\claude.exe")
        );
    }

    #[test]
    fn pathの前方が後方より優先される() {
        let got = find_in_windows_path(
            "psmux",
            &dirs(&["C:\\first", "C:\\second"]),
            &ext(),
            &[],
            &|p| p.ends_with("psmux.exe"),
        );
        assert_eq!(got.as_deref(), Some("C:\\first\\psmux.exe"));
    }

    #[test]
    fn 拡張子つきの名前もそのまま解決できる() {
        let got = find_in_windows_path("psmux.exe", &dirs(&["C:\\bin"]), &ext(), &[], &|p| {
            p == "C:\\bin\\psmux.exe"
        });
        assert_eq!(got.as_deref(), Some("C:\\bin\\psmux.exe"));
    }

    #[test]
    fn 見つからなければnone() {
        let got = find_in_windows_path("nope", &dirs(&["C:\\bin"]), &ext(), &[], &|_| false);
        assert_eq!(got, None);
    }

    #[test]
    fn 区切りを含む名前はpath探索の対象にしない() {
        let found = find_in_windows_path(
            "C:\\tools\\psmux.exe",
            &dirs(&["C:\\bin"]),
            &ext(),
            &[],
            &|p| p == "C:\\tools\\psmux.exe",
        );
        assert_eq!(found.as_deref(), Some("C:\\tools\\psmux.exe"));
        let missing = find_in_windows_path(
            "C:\\tools\\psmux.exe",
            &dirs(&["C:\\bin"]),
            &ext(),
            &[],
            &|_| false,
        );
        assert_eq!(missing, None);
    }

    /// 実環境で必ず存在するコマンドを引けること（両プラットフォームの実装が動く証明）
    #[test]
    fn 実環境の基本コマンドを解決できる() {
        let name = if cfg!(windows) { "cmd" } else { "sh" };
        let found = find(name);
        assert!(found.is_some(), "{name} を解決できない");
        assert!(
            std::path::Path::new(found.as_deref().unwrap()).is_file(),
            "解決結果が実ファイルでない: {found:?}"
        );
    }
}
