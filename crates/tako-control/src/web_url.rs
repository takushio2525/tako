//! Web ビューへ渡す URL の正規化と検査の 1 実装（#334 / #2937）。
//!
//! 入口（CLI / MCP の `tako web open` / `nav`・ポート提案のチップ・dock の URL 欄・アドレスバー・
//! ページの target=_blank / window.open・エラー画面の再読み込み・起動時の復元）はどれも GUI の
//! `webview::normalize_url` → `webview::validate_url` を通り、その 2 つはここ（[`normalize`] /
//! [`check`]）へ委ねる。wry へ URL を渡すのは `WebViewEntry::build_view`（生成 = `with_url`）と
//! `WebViewEntry::navigate`（読み込み = `load_url`）の 2 か所だけで、どちらも渡す直前に
//! `validate_url` を呼ぶ（番犬 `issue2937_webview_url_guard_watchdog`）。入口表は `.agent/architecture.md`
//! 「Web ビューへ渡す URL の検査（#334 / #2937）」。
//!
//! 検査は 3 段:
//! 1. **形**（#334）: 空・空白・制御文字
//! 2. **scheme の表**（#2937）: 開くのは [`OPENABLE_SCHEMES`]（http / https / file / data / about）だけ。
//!    `javascript:` / `vbscript:` は Web ビューの外から任意のスクリプトを走らせる形になるので名指しで断る
//!    （開いているページで JS を走らせる口は `tako web eval`）。それ以外の scheme は「開けない」と断る
//! 3. **解釈の確かめ**（#2937。境界 B38 `platform::webview_url`）: macOS は wry と同じ
//!    `+[NSURL URLWithString:]` を同じプロセスで先に呼び、nil なら断る（wry の `unwrap` に届かせない）
//!
//! 修正前（`javascript:alert(1)` を scheme と見ずに `https://javascript:alert(1)` へ正規化し、
//! NSURL がポートを解釈できず wry が abort する）は A/B `TAKO_2937_LEGACY=1` で戻せる。
//! 検証プロセス（隔離 GUI・セルフテスト・テスト）でだけ効く（本番の GUI を落とす形へは戻さない）

use crate::host_refusal::HostRefusal;

/// 開く scheme の表（#2937。小文字。比較は大文字小文字を区別しない = RFC 3986 §3.1）。
/// 修正前の正規化がそのまま通していた 5 つと同じ（それ以外は `https://` を前に付けていた）
pub const OPENABLE_SCHEMES: [&str; 5] = ["http", "https", "file", "data", "about"];

/// Web ビューの外からスクリプトを走らせる scheme（#2937。名指しで断る）
pub const SCRIPT_SCHEMES: [&str; 2] = ["javascript", "vbscript"];

/// A/B の env（修正前 = scheme を見ない正規化 + 形の検査だけ）
pub const LEGACY_ENV: &str = "TAKO_2937_LEGACY";

/// A/B（`TAKO_2937_LEGACY=1`）。**検証プロセスでだけ効く**（本番の GUI を abort する形へは戻さない）
pub fn legacy() -> bool {
    tako_core::paths::is_verification_process()
        && std::env::var_os(LEGACY_ENV).is_some_and(|v| v == "1")
}

/// `input` の先頭が `scheme:` の形ならその scheme（打たれた綴りのまま）。
///
/// scheme の文法（RFC 3986 §3.1・1 文字はドライブレターとして scheme と見ない）は
/// `tako_core::url_guard::split_scheme` の読み方の正本を通す（#1376 と読み方を 2 つにしない）。
/// その形のうち、次は scheme と見ない（修正前と同じく `https://` / `http://` を前に付けて開く）:
/// - `host:port`（`localhost:3000`・`example.com:8080/x`。`:` の後ろが `/` `?` `#` か終わりまで数字だけ）
/// - `user:pass@host`（`:` の後ろの `/` `?` `#` までに `@` がある = 利用者情報つきのホスト）
///
/// ただし開く scheme の表・スクリプトの scheme の名前と一致するもの、`:` の後ろが `//` のものは
/// 常に scheme（`javascript:1` を host:port と読んで https で開かない）
pub fn scheme_of(input: &str) -> Option<&str> {
    let (head, rest) = tako_core::url_guard::split_scheme(input).ok()?;
    let named = |list: &[&str]| list.iter().any(|s| head.eq_ignore_ascii_case(s));
    if named(&OPENABLE_SCHEMES) || named(&SCRIPT_SCHEMES) {
        return Some(head);
    }
    if rest.starts_with("//") {
        return Some(head);
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.bytes().all(|b| b.is_ascii_digit()) || authority.contains('@') {
        return None;
    }
    Some(head)
}

/// 打たれた文字列を Web ビューへ渡す URL に正規化する（アドレスバー・dock・CLI の省略記法）。
///
/// scheme の付いた入力は**そのまま**返す（開けない scheme は [`check`] が名指しで断る。修正前は
/// 表に無い scheme に `https://` を前に付けていた = `javascript:alert(1)` が
/// `https://javascript:alert(1)` になって wry が abort した。#2937）。scheme の無い入力は、
/// 開発サーバー（localhost / 127.0.0.1 / 0.0.0.0）なら `http://`、それ以外は `https://` を前に付ける。
/// ドットも空白も無い単語でも検索はしない（ローカル開発が主用途）
pub fn normalize(input: &str) -> String {
    normalize_with(input, legacy())
}

/// [`normalize`] の本体（`legacy` = A/B の修正前の腕）
pub fn normalize_with(input: &str, legacy: bool) -> String {
    let s = input.trim();
    let has_scheme = if legacy {
        // 修正前: 5 つの接頭辞だけを scheme と見た（大文字小文字を区別した）
        s.starts_with("http://")
            || s.starts_with("https://")
            || s.starts_with("file://")
            || s.starts_with("data:")
            || s.starts_with("about:")
    } else {
        scheme_of(s).is_some()
    };
    if has_scheme {
        return s.to_string();
    }
    // localhost:3000 / 127.0.0.1:8080 のような開発サーバー指定は http に倒す
    if s.starts_with("localhost") || s.starts_with("127.0.0.1") || s.starts_with("0.0.0.0") {
        return format!("http://{s}");
    }
    format!("https://{s}")
}

/// 正規化済みの URL を wry へ渡す前に検査する（3 段は冒頭の説明）。
/// wry へ URL を渡す 2 か所（生成・読み込み）の直前はすべてここを通る
pub fn check(url: &str) -> Result<(), HostRefusal> {
    check_with(url, legacy(), crate::platform::webview_url::webview_parses)
}

/// [`check`] の本体。`legacy` = A/B の修正前の腕（形の検査だけ）、`parses` = 解釈の確かめ（境界 B38）
pub fn check_with(
    url: &str,
    legacy: bool,
    parses: impl Fn(&str) -> bool,
) -> Result<(), HostRefusal> {
    // 1. 形（#334）
    if url.is_empty() {
        return Err(HostRefusal::UrlEmpty);
    }
    if url.chars().any(|c| c.is_ascii_whitespace()) {
        return Err(HostRefusal::UrlHasWhitespace(url.to_string()));
    }
    if url.bytes().any(|b| b < 0x20) {
        return Err(HostRefusal::UrlHasControlChar(url.to_string()));
    }
    if legacy {
        return Ok(());
    }
    // 2. scheme の表（#2937）
    if let Some(scheme) = scheme_of(url) {
        let lower = scheme.to_ascii_lowercase();
        if SCRIPT_SCHEMES.contains(&lower.as_str()) {
            return Err(HostRefusal::UrlScriptScheme(lower));
        }
        if !OPENABLE_SCHEMES.contains(&lower.as_str()) {
            return Err(HostRefusal::UrlSchemeUnsupported(lower));
        }
    }
    // 3. 解釈の確かめ（#2937。境界 B38）
    if !parses(url) {
        return Err(HostRefusal::UrlUnparsable(url.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解釈の確かめを常に通す（OS に依らない規則だけを見る）
    fn any(_: &str) -> bool {
        true
    }

    fn checked(input: &str) -> Result<(), HostRefusal> {
        check_with(&normalize_with(input, false), false, any)
    }

    /// 修正前の正規化 + 修正前の検査（A/B の旧の腕）
    fn checked_legacy(input: &str) -> Result<(), HostRefusal> {
        check_with(&normalize_with(input, true), true, any)
    }

    /// 修正前に開けた形（入口から打たれる綴りで）。修正前後のどちらでも通り、正規化の結果も変わらない
    const OPENABLE_INPUTS: [(&str, &str); 14] = [
        ("https://example.com", "https://example.com"),
        ("http://localhost:3000", "http://localhost:3000"),
        (
            "https://example.com/path?q=1&r=2#frag",
            "https://example.com/path?q=1&r=2#frag",
        ),
        ("localhost:5173", "http://localhost:5173"),
        ("localhost", "http://localhost"),
        ("127.0.0.1:8080", "http://127.0.0.1:8080"),
        ("0.0.0.0:8000", "http://0.0.0.0:8000"),
        ("example.com", "https://example.com"),
        ("example.com:8443/x", "https://example.com:8443/x"),
        ("  docs.rs/wry ", "https://docs.rs/wry"),
        ("user:pass@example.com", "https://user:pass@example.com"),
        (
            "file:///tmp/tako-2937/index.html",
            "file:///tmp/tako-2937/index.html",
        ),
        ("data:text/html,hello", "data:text/html,hello"),
        ("about:blank", "about:blank"),
    ];

    #[test]
    fn 今開ける形は修正前後で同じurlへ正規化され通る() {
        for (input, normalized) in OPENABLE_INPUTS {
            assert_eq!(normalize_with(input, false), normalized, "新: {input}");
            assert_eq!(normalize_with(input, true), normalized, "旧: {input}");
            assert_eq!(checked(input), Ok(()), "新で断った: {input}");
            assert_eq!(checked_legacy(input), Ok(()), "旧で断った: {input}");
        }
        // ポート提案のチップの形（`accept_port_suggestion` の format と同じ）
        let chip = format!("http://localhost:{}", 5173);
        assert_eq!(checked(&chip), Ok(()));
    }

    #[test]
    fn javascriptのurlは名指しで断る() {
        for input in [
            "javascript:alert(2936)",
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "  javascript:void(0)  ",
            "javascript:1",
            "javascript:alert('a@b')",
            "javascript://%0aalert(1)",
            "vbscript:msgbox(1)",
        ] {
            let err = checked(input).expect_err(input);
            assert!(
                matches!(err, HostRefusal::UrlScriptScheme(ref s) if s == "javascript" || s == "vbscript"),
                "{input}: {err:?}"
            );
        }
    }

    #[test]
    fn 修正前はjavascriptをhttpsのホストとして正規化していた() {
        // #2937 の実測の形。旧の腕は wry へ `https://javascript:alert(2936)` を渡して abort した
        assert_eq!(
            normalize_with("javascript:alert(2936)", true),
            "https://javascript:alert(2936)"
        );
        assert_eq!(checked_legacy("javascript:alert(2936)"), Ok(()));
        // 新の腕は scheme のまま残す（check が名指しで断る）
        assert_eq!(
            normalize_with("javascript:alert(2936)", false),
            "javascript:alert(2936)"
        );
    }

    #[test]
    fn 表に無いschemeは開けないと断る() {
        for (input, scheme) in [
            ("mailto:someone", "mailto"),
            ("ftp://example.com/x", "ftp"),
            ("blob:https://example.com/uuid", "blob"),
            ("view-source:https://example.com", "view-source"),
            ("foo:bar", "foo"),
            ("Custom+App.1:open", "custom+app.1"),
        ] {
            assert_eq!(
                checked(input),
                Err(HostRefusal::UrlSchemeUnsupported(scheme.into())),
                "{input}"
            );
        }
    }

    #[test]
    fn schemeの大文字小文字は区別しない() {
        assert_eq!(
            normalize_with("HTTPS://EXAMPLE.COM", false),
            "HTTPS://EXAMPLE.COM"
        );
        assert_eq!(checked("HTTPS://EXAMPLE.COM"), Ok(()));
        assert_eq!(checked("About:blank"), Ok(()));
    }

    #[test]
    fn hostとportの形はschemeと読まない() {
        assert_eq!(scheme_of("localhost:3000"), None);
        assert_eq!(scheme_of("example.com:8080/x"), None);
        assert_eq!(scheme_of("example.com:8080?q=1"), None);
        assert_eq!(scheme_of("localhost:"), None);
        assert_eq!(scheme_of("user:pass@example.com"), None);
        assert_eq!(scheme_of("127.0.0.1:8080"), None);
        assert_eq!(scheme_of("[::1]:3000"), None);
        assert_eq!(scheme_of("example.com/a:b"), None);
        assert_eq!(scheme_of("https://example.com"), Some("https"));
        assert_eq!(scheme_of("about:blank"), Some("about"));
        assert_eq!(scheme_of("ftp://example.com"), Some("ftp"));
        assert_eq!(scheme_of("javascript:1"), Some("javascript"));
        // 1 文字はドライブレター（`url_guard::split_scheme` の読み方）
        assert_eq!(scheme_of("c:x"), None);
    }

    #[test]
    fn 形の検査は修正前と同じ() {
        for (url, want) in [
            ("", HostRefusal::UrlEmpty),
            (
                "https://github .com",
                HostRefusal::UrlHasWhitespace("https://github .com".into()),
            ),
            (
                "https://example.com/\there",
                HostRefusal::UrlHasWhitespace("https://example.com/\there".into()),
            ),
            (
                "https://example.com/\x01bad",
                HostRefusal::UrlHasControlChar("https://example.com/\x01bad".into()),
            ),
        ] {
            assert_eq!(
                check_with(url, false, any),
                Err(want.clone()),
                "新: {url:?}"
            );
            assert_eq!(check_with(url, true, any), Err(want), "旧: {url:?}");
        }
    }

    #[test]
    fn 解釈の確かめが断った形は理由つきで返す() {
        let never = |_: &str| false;
        assert_eq!(
            check_with("https://example.com:abc", false, never),
            Err(HostRefusal::UrlUnparsable("https://example.com:abc".into()))
        );
        // 旧の腕は確かめない（修正前は wry へ渡して abort した）
        assert_eq!(check_with("https://example.com:abc", true, never), Ok(()));
        // 形・scheme の断りが先（確かめる前に理由が決まる）
        assert_eq!(
            check_with("javascript:alert(1)", false, never),
            Err(HostRefusal::UrlScriptScheme("javascript".into()))
        );
    }

    #[test]
    fn 本物の確かめで修正前にwryを落とした形を断る() {
        // 境界 B38 の本物（macOS = NSURL。それ以外は確かめない = 通る）
        let real = crate::platform::webview_url::webview_parses;
        assert_eq!(check_with("https://example.com", false, real), Ok(()));
        assert_eq!(check_with("http://localhost:3000", false, real), Ok(()));
        // 修正前に wry を落とした形（scheme の表は通るが NSURL が nil を返す）
        for typed in ["https://example.com:abc", "exa|mple.com", "https://[::1"] {
            let url = normalize_with(typed, false);
            let got = check_with(&url, false, real);
            if cfg!(target_os = "macos") {
                assert_eq!(got, Err(HostRefusal::UrlUnparsable(url.clone())), "{typed}");
            } else {
                assert_eq!(got, Ok(()), "{typed}");
            }
        }
        // 今開ける形は本物の確かめも通る
        for (input, _) in OPENABLE_INPUTS {
            let url = normalize_with(input, false);
            assert_eq!(check_with(&url, false, real), Ok(()), "{input}");
        }
    }
}
