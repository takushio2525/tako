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
/// **正本は [`crate::url_guard::browser_url`]**。#1376 で PDF のリンク注釈・提案チップ・
/// 境界 B8 にも同じ規則を通すことになり、判定を 1 実装へ寄せた。ここは Markdown 経路が
/// 使ってきた名前をそのまま残すための再公開（呼び出し側は従来どおり）。
pub use crate::url_guard::browser_url;

#[cfg(test)]
mod tests {
    /// 再公開が正本と同じ答えを返すこと（実装を md_links 側へ書き戻さない）。
    /// 規則そのもののテストは [`crate::url_guard`] にある
    #[test]
    fn 再公開はurl_guardの判定と同じ() {
        for url in [
            "https://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "./relative.md",
        ] {
            assert_eq!(super::browser_url(url), crate::url_guard::browser_url(url));
        }
    }
}
