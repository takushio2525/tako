//! Markdown プレビュー内リンクモデル（Issue #680）。
//!
//! md のインラインリンク（`[text](url)`）を ⌘+クリックで外部ブラウザへ渡すための
//! 「何を開いてよいか」の判定と、CLI / MCP へ公開する 1 件分の形を持つ。
//! 当たり判定の座標（行・バイト範囲）は GPUI の実 shaping に依存するので UI 層が持つ。

use serde::{Deserialize, Serialize};

/// Markdown プレビュー内のリンク 1 件（CLI / MCP へ公開する形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdLink {
    /// リンクの表示テキスト
    pub text: String,
    /// md に書かれたリンク先（生の値。開けないものも記録する）
    pub url: String,
    /// 外部ブラウザで開けるか（[`browser_url`] が通るものだけ true）
    pub openable: bool,
    /// プレビュー上の行番号（0 始まり。テキスト選択と同じ座標系）
    pub line: usize,
}

/// 外部ブラウザへ渡してよい URL だけを返す（#680）。
///
/// **http / https のみ許可する**。`javascript:` / `data:` / `vbscript:` は OS の URL
/// ハンドラへ渡すと任意コード実行になりうるし、`file:` はローカルファイルを
/// ブラウザへ露出させる。相対パス・アンカー（`#section`）・`mailto:` のような
/// 「ブラウザで開く対象ではない」ものも `None` を返す（呼び出し側は何もしない）。
///
/// スキームだけで判定するので、ホスト名やパスの妥当性は OS のハンドラに委ねる。
/// 前後の空白は剥がし、制御文字を含むものは拒否する。
pub fn browser_url(url: &str) -> Option<&str> {
    let trimmed = url.trim();
    // スキームの区切りは "://" に限定する（`javascript:alert(1)` のような
    // オーソリティを持たない形はここで落ちる）
    let (scheme, rest) = trimmed.split_once("://")?;
    // スキームは ASCII の大文字小文字を区別しない（RFC 3986 §3.1）
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    // 改行・タブ入りの URL は引数として渡す際に扱いを誤らせるので開かない
    if trimmed.chars().any(char::is_control) {
        return None;
    }
    // ホスト部が空（"https://" だけ、"https:///path"）は開いても意味がない
    if rest.is_empty() || rest.starts_with('/') {
        return None;
    }
    Some(trimmed)
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
        assert_eq!(browser_url("javascript:alert(1)"), None);
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
    }

    #[test]
    fn 制御文字入りは開かない() {
        assert_eq!(browser_url("https://example.com/\nevil"), None);
        assert_eq!(browser_url("https://example.com/\tevil"), None);
    }
}
