//! tako のユーザーデータ配置先（シェル統合スクリプト・接続情報ファイル等）

use std::path::PathBuf;

/// ユーザーのホームディレクトリ。macOS / Linux は `$HOME`、Windows は `%USERPROFILE%`。
///
/// **ホーム解決はここが唯一の入口**（#870）。以前は `terminal.rs`（正しい形）と
/// `links.rs`（`HOME` 決め打ち）の 2 か所にあり、後者が Windows で必ず `None` になって
/// ターミナルリンクの `~/` が無反応だった。同じ意味論を 2 回書くと**片方だけ直る**ので、
/// 参照する側は必ずこれを通す（番犬テスト `ホーム解決の入口がpathsだけである` が固定）。
///
/// `cfg` を持たないのは、`HOME` → `USERPROFILE` の順で見れば**どちらの OS でも正しい**ため
/// （unix に `USERPROFILE` は無く、Windows に `HOME` は通常無い。Git Bash 等が `HOME` を
/// 立てている Windows では利用者の意図どおりそちらが優先される）。取得できなければ None
pub fn home_dir() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

/// [`home_dir`] の純粋ロジック（テスト用に env 参照と分離）。
/// `$HOME` を優先し、無ければ `%USERPROFILE%`。どちらも空なら None
pub(crate) fn home_from(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    home.filter(|dir| !dir.is_empty())
        .or(userprofile)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// tako のデータディレクトリ。
/// macOS: `~/Library/Application Support/tako`、その他 unix: `$XDG_DATA_HOME/tako`
/// （無ければ `~/.local/share/tako`）、Windows: `%APPDATA%\tako`
/// （無ければ `%USERPROFILE%\AppData\Roaming\tako`）。
/// `TAKO_DATA_DIR` で上書き可能（隔離検証用。#177 / #112: 本番の layout.json /
/// settings.json / token / persist.log に一切触れない起動を 1 変数で作れる）
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TAKO_DATA_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    default_data_dir()
}

fn default_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join("Library/Application Support/tako"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| PathBuf::from(h).join(".local/share"))
            })
            .map(|d| d.join("tako"))
    }
    // Windows のローミングプロファイル。%APPDATA% は通常セットされているが、
    // サービス起動など環境が痩せている場合に備えて %USERPROFILE% から組み立てる経路も持つ
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .filter(|h| !h.is_empty())
                    .map(|h| PathBuf::from(h).join("AppData").join("Roaming"))
            })
            .map(|d| d.join("tako"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    #[allow(non_snake_case)]
    fn ホームは_HOME_を優先し空は無視する() {
        // HOME があればそれを使う
        assert_eq!(
            home_from(
                Some(OsString::from("/Users/foo")),
                Some(OsString::from("C:\\u"))
            ),
            Some(PathBuf::from("/Users/foo"))
        );
        // HOME 無し → USERPROFILE（Windows）
        assert_eq!(
            home_from(None, Some(OsString::from("C:\\Users\\foo"))),
            Some(PathBuf::from("C:\\Users\\foo"))
        );
        // 空文字は無視（親 cwd 継承へフォールバック）
        assert_eq!(home_from(Some(OsString::new()), None), None);
        assert_eq!(home_from(None, None), None);
        assert_eq!(home_from(None, Some(OsString::new())), None);
    }

    /// **空の `HOME` が `USERPROFILE` を隠さない**こと（#870）。
    ///
    /// 統合前の `terminal.rs` 版は `home.or(userprofile).filter(空でない)` の順だったので、
    /// `HOME=`（空）が立っている Windows では USERPROFILE を見ずに None へ落ちていた。
    /// 「ホームが取れない」は `~/` のリンクが無反応になる #870 と同じ症状なので、
    /// 空は**次の候補へ進む**形に揃えてある
    #[test]
    #[allow(non_snake_case)]
    fn 空の_HOME_は_USERPROFILE_を隠さない() {
        assert_eq!(
            home_from(
                Some(OsString::new()),
                Some(OsString::from("C:\\Users\\foo"))
            ),
            Some(PathBuf::from("C:\\Users\\foo"))
        );
    }

    /// ホーム解決の入口が `paths` だけであること（番犬。#870）。
    ///
    /// 同じ意味論を 2 か所に書くと**片方だけ直る**（`links.rs` が `HOME` 決め打ちのまま
    /// 残って Windows の `~/` が無反応だった、というのがこの Issue そのもの）。
    /// ホームを組み立てる目的で `HOME` / `USERPROFILE` を直接読むのはここだけにする
    #[test]
    fn ホーム解決の入口がpathsだけである() {
        for (name, src) in [
            ("links.rs", include_str!("links.rs")),
            ("terminal.rs", include_str!("terminal.rs")),
        ] {
            // 走査するのは**env を読む形**だけ（散文に USERPROFILE と書いてあっても
            // それは実装ではないので拾わない）
            for needle in [
                "var_os(\"HOME\")",
                "var(\"HOME\")",
                "var_os(\"USERPROFILE\")",
                "var(\"USERPROFILE\")",
            ] {
                assert!(
                    !src.contains(needle),
                    "{name} が {needle} を直接読んでいる（ホーム解決は paths::home_dir へ寄せる。#870）"
                );
            }
        }
    }
}
