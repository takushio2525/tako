//! OS の URL ハンドラへ渡してよい文字列の判定（Issue #680 / #1376 の正本）
//!
//! tako が「開く」URL の出どころは**第三者が書ける場所**しかない: ターミナル画面
//! （[`crate::links`]）・Markdown（[`crate::md_links`]）・**PDF のリンク注釈**
//! （[`crate::pdf_links`]）・ポート検知の提案チップ。OS の既定ハンドラ（macOS の
//! `open` / Windows の `ShellExecuteW`）は **URL でない文字列も開く**ので、
//! ローカルの実行ファイルパス・UNC パス（`\\host\share\payload.exe`）・`file:` URL を
//! そのまま渡すと、クリック 1 回で任意のプログラムが起動しうる（#1376）。
//!
//! 判定は**ここ 1 か所**に置き、2 段構えで使う:
//!
//! - [`check_browser_url`]（経路側 = #1376 の (a)）: 画面 / md / PDF / 提案チップから
//!   来た URL に通す。**http / https だけ**を許す（#680 の規則がこれ）
//! - [`check_os_handler_url`]（境界 B8 側 = #1376 の (b)）: `open_url` の直前に通す保険。
//!   上に加えて **tako 自身が組み立てる OS 固有スキーム**（[`OS_HANDLER_SCHEMES`]）だけを許す
//!
//! 経路側の検査を落とした実装を書いても B8 で止まり、B8 の許可集合が広がっても経路側で
//! 止まる。**どちらかが 1 行で壊れても穴にならない**ようにこの形にしてある。

/// 開かないと決めた理由（**リンク文字列そのものは持たない**）。
///
/// 診断ログ・エラー文へ載せてよいのはこの分類だけ。URL の中身は PDF / ペインの内容に
/// 相当するので `persist.log` へ出してはならない（AGENTS.md の絶対ルール）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlBlocked {
    /// スキームが無い（`/usr/bin/x`・`\\host\share\x.exe`・`./a.md`・`#anchor`）
    NoScheme,
    /// 1 文字のスキーム = Windows のドライブレター（`C:\payload.exe`）
    DriveLetter,
    /// 許可していないスキーム（`file:` / `javascript:` / `data:` / `smb:` …）
    Scheme,
    /// 改行・タブ等の制御文字入り
    ControlChar,
    /// http / https なのにホスト部が無い（`https://` だけ、`https:///path`）
    NoHost,
}

impl UrlBlocked {
    /// 診断ログ・エラー文へ載せる短い理由（**URL は含まない**）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoScheme => "スキームが無い",
            Self::DriveLetter => "スキームではなくドライブレター",
            Self::Scheme => "許可していないスキーム",
            Self::ControlChar => "制御文字入り",
            Self::NoHost => "ホスト部が無い",
        }
    }
}

impl std::fmt::Display for UrlBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 境界 B8（`os_integration::open_url`）へ渡してよいスキームの**許可集合**（#1376）。
///
/// 棚卸し（2026-09-11 時点の全呼び出し元）:
///
/// | 呼び出し元 | 渡す URL |
/// |---|---|
/// | 画面リンク / md / PDF / 提案チップ / About・Help メニュー | `http` / `https` |
/// | `fda::open_settings`（フルディスクアクセスのパネル） | `x-apple.systempreferences:` |
/// | `sleep_guard::open_battery_settings` | `x-apple.systempreferences:` |
///
/// **増やすときは「tako 自身が組み立てた URL か」を確かめる**。第三者由来の文字列が
/// 届く経路（画面 / md / PDF / 提案チップ）は [`check_browser_url`] で http / https へ
/// 絞ってあるので、ここへ足したスキームはその経路からは届かない。
pub const OS_HANDLER_SCHEMES: &[&str] = &["http", "https", "x-apple.systempreferences"];

/// スキーム部（`:` の手前）を RFC 3986 §3.1 として読む（**読み方の正本**）。
/// 戻り値は `(scheme, rest)` で、`rest` は `:` の後ろ全部。
///
/// **1 文字のスキームは URL として扱わない**: Windows のドライブレター
/// （`C:\payload.exe`）をスキームとして通すと、ローカルの実行ファイルが「URL」として
/// OS へ渡ってしまう。RFC 上 1 文字のスキームも合法だが、実在するものは無い。
pub fn split_scheme(url: &str) -> Result<(&str, &str), UrlBlocked> {
    let (scheme, rest) = url.split_once(':').ok_or(UrlBlocked::NoScheme)?;
    if scheme.len() == 1 {
        return Err(UrlBlocked::DriveLetter);
    }
    let mut chars = scheme.chars();
    // scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return Err(UrlBlocked::NoScheme),
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return Err(UrlBlocked::NoScheme);
    }
    Ok((scheme, rest))
}

/// 外部ブラウザへ渡してよい URL だけを返す（#680 / #1376）。理由つき。
///
/// **http / https のみ許可する**。`javascript:` / `data:` / `vbscript:` は OS の URL
/// ハンドラへ渡すと任意コード実行になりうるし、`file:` はローカルファイルを露出させる。
/// 相対パス・アンカー（`#section`）・`mailto:` のような「ブラウザで開く対象ではない」
/// ものも弾く（呼び出し側は何もしない）。
///
/// スキームだけで判定するので、ホスト名やパスの妥当性は OS のハンドラに委ねる。
/// 前後の空白は剥がし、制御文字を含むものは拒否する。
pub fn check_browser_url(url: &str) -> Result<&str, UrlBlocked> {
    let trimmed = url.trim();
    // 改行・タブ入りの URL は引数として渡す際に扱いを誤らせるので開かない
    if trimmed.chars().any(char::is_control) {
        return Err(UrlBlocked::ControlChar);
    }
    let (scheme, rest) = split_scheme(trimmed)?;
    // スキームは ASCII の大文字小文字を区別しない（RFC 3986 §3.1）
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(UrlBlocked::Scheme);
    }
    // オーソリティを持たない形（`http:example.com`）とホスト部が空
    // （`https://` だけ、`https:///path`）は開いても意味がない
    let host = rest.strip_prefix("//").ok_or(UrlBlocked::NoHost)?;
    if host.is_empty() || host.starts_with('/') {
        return Err(UrlBlocked::NoHost);
    }
    Ok(trimmed)
}

/// 外部ブラウザへ渡してよい URL だけを返す（#680）。理由が要らない呼び出し側用。
///
/// 判定は [`check_browser_url`] が正本。`md_links::browser_url` はこれの再公開。
pub fn browser_url(url: &str) -> Option<&str> {
    check_browser_url(url).ok()
}

/// 境界 B8（`os_integration::open_url`）へ渡してよい文字列だけを返す（#1376）。
///
/// 経路側（[`check_browser_url`]）を通り抜けた場合の**保険**なので、許可集合は
/// [`OS_HANDLER_SCHEMES`] = 「http / https + tako 自身が組み立てる OS 固有スキーム」に
/// 限る。http / https は経路側と同じ厳しさ（オーソリティ必須）で通す。
pub fn check_os_handler_url(url: &str) -> Result<&str, UrlBlocked> {
    let trimmed = url.trim();
    if trimmed.chars().any(char::is_control) {
        return Err(UrlBlocked::ControlChar);
    }
    let (scheme, rest) = split_scheme(trimmed)?;
    if !OS_HANDLER_SCHEMES
        .iter()
        .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
    {
        return Err(UrlBlocked::Scheme);
    }
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        return check_browser_url(trimmed);
    }
    // OS 固有スキームはオーソリティを持たない（`x-apple.systempreferences:<パネル>`）。
    // 中身の妥当性は OS のハンドラに委ねるが、空なら開いても意味がない
    if rest.is_empty() {
        return Err(UrlBlocked::NoHost);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn httpとhttpsは開ける() {
        assert_eq!(
            browser_url("https://example.com/a?b=1#c"),
            Some("https://example.com/a?b=1#c")
        );
        assert_eq!(
            browser_url("http://localhost:5173/"),
            Some("http://localhost:5173/")
        );
        // スキームの大文字小文字は区別しない
        assert_eq!(
            browser_url("HTTPS://EXAMPLE.COM"),
            Some("HTTPS://EXAMPLE.COM")
        );
        // 前後の空白は剥がす
        assert_eq!(
            browser_url("  https://example.com  "),
            Some("https://example.com")
        );
    }

    #[test]
    fn 危険なスキームは開かない() {
        // オーソリティ無しの形（"://" が無い）
        assert_eq!(
            check_browser_url("javascript:alert(1)"),
            Err(UrlBlocked::Scheme)
        );
        assert_eq!(
            browser_url("data:text/html,<script>alert(1)</script>"),
            None
        );
        assert_eq!(browser_url("vbscript:msgbox(1)"), None);
        // "://" を持たせてもスキーム名で落ちる
        assert_eq!(browser_url("javascript://comment%0aalert(1)"), None);
        assert_eq!(browser_url("JavaScript://x/alert(1)"), None);
    }

    #[test]
    fn ブラウザで開く対象でないものは開かない() {
        assert_eq!(browser_url("file:///etc/passwd"), None);
        assert_eq!(browser_url("mailto:someone@example.com"), None);
        assert_eq!(browser_url("./relative/path.md"), None);
        assert_eq!(browser_url("/absolute/path.md"), None);
        assert_eq!(browser_url("#section-anchor"), None);
        assert_eq!(browser_url(""), None);
        // プロトコル相対（"//example.com"）は対象外（スコープ最小。#680）
        assert_eq!(browser_url("//example.com"), None);
    }

    #[test]
    fn ホスト部が空なら開かない() {
        assert_eq!(browser_url("https://"), None);
        assert_eq!(browser_url("http://"), None);
        assert_eq!(browser_url("https:///path/only"), None);
        // オーソリティを持たない http（`http:example.com`）も開かない
        assert_eq!(
            check_browser_url("http:example.com"),
            Err(UrlBlocked::NoHost)
        );
    }

    #[test]
    fn 制御文字入りは開かない() {
        assert_eq!(browser_url("https://example.com/\nevil"), None);
        assert_eq!(browser_url("https://example.com/\tevil"), None);
    }

    /// #1376 の本体。PDF のリンク注釈は**中身が任意の文字列**なので、
    /// 「URL に見えないもの」が OS の既定ハンドラへ届く形を全部固定しておく
    #[test]
    fn ローカルの実行対象は経路側でも境界側でも弾く() {
        // `file:` URL（ローカルファイルの露出 + 拡張子次第で実行）
        for url in ["file:///etc/passwd", "file:///C:/Windows/System32/cmd.exe"] {
            assert_eq!(check_browser_url(url), Err(UrlBlocked::Scheme), "{url}");
            assert_eq!(check_os_handler_url(url), Err(UrlBlocked::Scheme), "{url}");
        }
        // UNC パス（`ShellExecuteW` はこれを開ける = リモートの exe が走る）
        for url in [r"\\host\share\payload.exe", r"\\10.0.0.1\c$\x.bat"] {
            assert_eq!(check_browser_url(url), Err(UrlBlocked::NoScheme), "{url}");
            assert_eq!(
                check_os_handler_url(url),
                Err(UrlBlocked::NoScheme),
                "{url}"
            );
        }
        // ローカルパス（macOS の `open /usr/bin/x` / Windows のドライブレター）
        assert_eq!(
            check_os_handler_url("/usr/bin/osascript"),
            Err(UrlBlocked::NoScheme)
        );
        assert_eq!(
            check_os_handler_url("/Applications/Calculator.app"),
            Err(UrlBlocked::NoScheme)
        );
        assert_eq!(
            check_os_handler_url(r"C:\Windows\System32\cmd.exe"),
            Err(UrlBlocked::DriveLetter)
        );
        assert_eq!(
            check_os_handler_url(r"c:/Windows/System32/cmd.exe"),
            Err(UrlBlocked::DriveLetter)
        );
        // スクリプト系スキーム
        for url in [
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
        ] {
            assert_eq!(check_os_handler_url(url), Err(UrlBlocked::Scheme), "{url}");
        }
    }

    /// 許可集合の正本が 1 か所にあり、そこに在るものだけが境界を通る
    #[test]
    fn 境界はos固有スキームの許可集合だけ通す() {
        // 棚卸し済みの OS 固有スキーム（FDA / sleep guard が使う）
        for url in [
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
            "x-apple.systempreferences:com.apple.preference.battery",
        ] {
            assert_eq!(check_os_handler_url(url), Ok(url), "{url}");
            // 経路側（第三者由来の URL）からは届かない
            assert_eq!(check_browser_url(url), Err(UrlBlocked::Scheme), "{url}");
        }
        // 許可集合に無い OS 固有スキームは通さない
        for url in [
            "x-apple.systempreferences:",
            "ms-settings:windowsupdate",
            "smb://host/share",
            "ftp://example.com/x",
            "itms-apps://itunes.apple.com/app/id1",
        ] {
            assert!(check_os_handler_url(url).is_err(), "{url} が通っている");
        }
        // 許可集合の中身そのもの（増減したらここが落ちる）
        assert_eq!(
            OS_HANDLER_SCHEMES,
            &["http", "https", "x-apple.systempreferences"]
        );
    }

    /// http / https は境界でも経路側と同じ判定（保険が本体より緩くならない）
    #[test]
    fn 境界のhttp判定は経路側と同じ() {
        for url in [
            "https://example.com/a?b=1&c=2",
            "http://localhost:5173/",
            "https://",
            "https:///path",
            "http:example.com",
            "https://example.com/\nevil",
        ] {
            assert_eq!(
                check_os_handler_url(url),
                check_browser_url(url),
                "http / https の判定が経路側と食い違う: {url}"
            );
        }
    }

    #[test]
    fn スキームの読み方() {
        assert_eq!(
            split_scheme("https://example.com"),
            Ok(("https", "//example.com"))
        );
        assert_eq!(
            split_scheme("x-apple.systempreferences:com.apple.x"),
            Ok(("x-apple.systempreferences", "com.apple.x"))
        );
        // 1 文字 = ドライブレター
        assert_eq!(split_scheme(r"C:\x.exe"), Err(UrlBlocked::DriveLetter));
        // `:` が無い / 先頭が英字でない / スキームに使えない文字
        assert_eq!(split_scheme("/usr/bin/x"), Err(UrlBlocked::NoScheme));
        assert_eq!(split_scheme("1http://x"), Err(UrlBlocked::NoScheme));
        assert_eq!(split_scheme("ht tp://x"), Err(UrlBlocked::NoScheme));
        assert_eq!(split_scheme(":no-scheme"), Err(UrlBlocked::NoScheme));
    }

    /// 理由は分類だけを持ち、URL の中身を持ち回らない（診断ログの規約）
    #[test]
    fn 理由にリンク文字列が混ざらない() {
        let url = "file:///Users/testuser/secret.pdf";
        let reason = check_os_handler_url(url).unwrap_err();
        assert!(!reason.as_str().contains("secret"));
        assert!(!reason.to_string().contains("file"));
        assert_eq!(reason.as_str(), "許可していないスキーム");
    }
}
