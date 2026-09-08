//! tako のユーザーデータ配置先（シェル統合スクリプト・接続情報ファイル等）

use std::path::{Path, PathBuf};

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

/// ホーム配下のパスを表示用に `~` 表記へ縮める。
///
/// **表示短縮の正本はここ 1 本**（#893）。以前は同じ規則が 10 か所
/// （`header_layout.rs` / `status_bar.rs` / `sidebar.rs` / `starter.rs`（2 箇所）/
/// `main.rs`（2 箇所）/ tako-cli の `setup.rs` / `context_budget.rs` /
/// `setup_bootstrap.rs`）に書かれていて、3 つの欠けを別々に持っていた:
///
/// 1. ホーム解決が `HOME` 決め打ちなので **Windows では 1 つも縮まらない**（#870 と同型）
/// 2. ホーム自身の扱いが揃っていない（`~` を出す実装と `~/` を出す実装が混在）
/// 3. 文字列の前方一致なので、ホームが `/Users/alice` のとき
///    `/Users/alice2/dev` が `~2/dev` へ化ける
pub fn shorten_home(path: &str) -> String {
    shorten_home_with(path, home_dir().as_deref())
}

/// [`shorten_home`] の純粋ロジック（env 参照と分離）。
///
/// ホームを引数で受けるので **macOS から Windows のパスを検査できる**
/// （#515 / #905 と同じ作法）。区切りは元の文字をそのまま残すので
/// `C:\Users\winuser\dev` は `~\dev` になる（表示用なので OS の見た目に合わせる）。
/// ホームが解決できない・空・ルート（`/`）のときは縮めない
pub fn shorten_home_with(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    let home = home.to_string_lossy();
    // 末尾の区切りは無視（`C:\Users\winuser\` と `C:\Users\winuser` を同じに扱う）。
    // ルート（`/` だけ）は縮めても読みやすくならないので、trim の結果が空なら諦める
    let home = home.trim_end_matches(is_path_separator);
    if home.is_empty() {
        return path.to_string();
    }
    let Some(rest) = path.strip_prefix(home) else {
        return path.to_string();
    };
    match rest.chars().next() {
        // ホームそのもの
        None => "~".to_string(),
        // 区切りで続く = ホーム配下。区切りは元の文字を残す（`~/dev` / `~\dev`）
        Some(c) if is_path_separator(c) => format!("~{rest}"),
        // 途中一致（`/Users/alice2`）は別のディレクトリなので縮めない
        Some(_) => path.to_string(),
    }
}

/// パスの区切り。`std::path::is_separator` は cfg で変わり、macOS から
/// Windows のパスを検査できないので自前で持つ（`\` と `/` の両方を区切りとみなす）
fn is_path_separator(c: char) -> bool {
    c == '/' || c == '\\'
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

    /// `~` 短縮の正本（#893）。
    ///
    /// ホームを引数で受ける形にしてあるので、**macOS 上から Windows のパスも検査できる**。
    /// 移送前は 10 か所に散っていて、ホーム自身の扱い（`~` と `~/`）と
    /// 前方一致の食い込み（`/Users/alice2` → `~2`）が実装ごとに違っていた
    #[test]
    fn ホーム短縮は両osの区切りを扱う() {
        let mac = PathBuf::from("/Users/testuser");
        let win = PathBuf::from("C:\\Users\\winuser");

        // 配下は `~` + 残り（区切りは元の文字をそのまま残す）
        assert_eq!(
            shorten_home_with("/Users/testuser/dev/tako", Some(&mac)),
            "~/dev/tako"
        );
        assert_eq!(
            shorten_home_with("C:\\Users\\winuser\\dev\\tako", Some(&win)),
            "~\\dev\\tako"
        );
        // Windows は `/` 区切りも通る
        assert_eq!(
            shorten_home_with(
                "C:/Users/winuser/dev",
                Some(&PathBuf::from("C:/Users/winuser"))
            ),
            "~/dev"
        );

        // ホームそのものは `~`（旧 status_bar / sidebar は `~/` を出していた）
        assert_eq!(shorten_home_with("/Users/testuser", Some(&mac)), "~");
        assert_eq!(shorten_home_with("C:\\Users\\winuser", Some(&win)), "~");
        // 末尾に区切りが付いたホームでも同じ
        assert_eq!(
            shorten_home_with(
                "/Users/testuser/dev",
                Some(&PathBuf::from("/Users/testuser/"))
            ),
            "~/dev"
        );

        // 途中一致は別のディレクトリなので縮めない（旧実装は `~2/dev` に化けていた）
        assert_eq!(
            shorten_home_with("/Users/alice2/dev", Some(&PathBuf::from("/Users/alice"))),
            "/Users/alice2/dev"
        );
        // ホーム外・ホーム未解決・ホームがルートのときは素のまま
        assert_eq!(shorten_home_with("/opt/tako", Some(&mac)), "/opt/tako");
        assert_eq!(
            shorten_home_with("/Users/testuser/dev", None),
            "/Users/testuser/dev"
        );
        assert_eq!(shorten_home_with("/dev", Some(&PathBuf::from("/"))), "/dev");
        // ホームがルート直下（1 階層）でも同じ規則
        assert_eq!(
            shorten_home_with("/root/dev", Some(&PathBuf::from("/root"))),
            "~/dev"
        );
        assert_eq!(
            shorten_home_with("/root", Some(&PathBuf::from("/root"))),
            "~"
        );
        // 空パス・ホームだけが空
        assert_eq!(shorten_home_with("", Some(&mac)), "");
        assert_eq!(shorten_home_with("/opt", Some(&PathBuf::from(""))), "/opt");
    }

    /// `HOME` と `USERPROFILE` の**両方が無い**環境ではホームを名乗らない。
    /// ここで空文字や `/` を返すと、その先で `~` 相対のパスを組み立ててしまう
    #[test]
    #[allow(non_snake_case)]
    fn HOMEもUSERPROFILEも無ければホームは無い() {
        assert_eq!(home_from(None, None), None);
        assert_eq!(
            shorten_home_with("/Users/testuser/dev", home_from(None, None).as_deref()),
            "/Users/testuser/dev"
        );
    }
}
