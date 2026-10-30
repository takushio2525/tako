//! Markdown / PDF プレビューのアウトラインモデル（Issue #232）。
//!
//! GPUI や PDFKit へ依存せず、ロード時に構築済みの目次を GUI・dispatch・CLI・MCP が
//! 同じ 1 始まり項目番号で参照する。

use serde::{Deserialize, Serialize};

/// アウトライン項目のジャンプ先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewOutlineTarget {
    /// Markdown の描画ブロック番号（0 始まり、内部表現）。
    MarkdownBlock { block: usize },
    /// PDF のページ番号（1 始まり）。
    PdfPage { page: usize },
    /// 編集中の Markdown の原文の行（1 始まり。#1661）。
    ///
    /// 編集モードの表示は原文の行なので、描画ブロックの番号では飛べない。
    /// 項目の並びとタイトルは描画時の目次（`MarkdownBlock`）と同じ規則で作るので、
    /// 同じ本文なら k 番目どうしが同じ見出しを指す
    SourceLine { line: usize },
}

/// プレビューのアウトライン 1 項目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewOutlineItem {
    pub title: String,
    /// Markdown は H1〜H6、PDF は PDFKit のツリー深さ（いずれも 1 始まり）。
    pub level: u8,
    pub target: PreviewOutlineTarget,
}

/// プレビューロード時に一度だけ構築するアウトライン。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewOutline {
    pub items: Vec<PreviewOutlineItem>,
}

impl PreviewOutline {
    pub fn new(items: Vec<PreviewOutlineItem>) -> Self {
        Self { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// CLI / MCP / GUI 共通の 1 始まり項目番号をジャンプ先へ解決する。
    pub fn target(&self, item: usize) -> Result<PreviewOutlineTarget, OutlineRefusal> {
        if item == 0 {
            return Err(OutlineRefusal::ItemZero);
        }
        self.items
            .get(item - 1)
            .map(|entry| entry.target)
            .ok_or(OutlineRefusal::OutOfRange {
                item,
                total: self.items.len(),
            })
    }
}

/// アウトライン項目の指定の断り（日英は #2365）。日英の対は [`OutlineRefusal::text_in`] の
/// 網羅 match の 1 か所で、日本語は #2365 より前の固定文とバイト一致
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineRefusal {
    /// `アウトライン項目は 1 以上で指定する`
    ItemZero,
    /// `アウトライン項目の範囲外: <item>（全 <total> 件）`
    OutOfRange { item: usize, total: usize },
}

impl OutlineRefusal {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::ItemZero, Ja) => "アウトライン項目は 1 以上で指定する".into(),
            (Self::ItemZero, En) => "the outline item must be 1 or greater".into(),
            (Self::OutOfRange { item, total }, Ja) => {
                format!("アウトライン項目の範囲外: {item}（全 {total} 件）")
            }
            (Self::OutOfRange { item, total }, En) => {
                format!("outline item out of range: {item} (total: {total})")
            }
        }
    }
}

impl std::fmt::Display for OutlineRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

impl std::error::Error for OutlineRefusal {}

/// `Result<_, String>` の口から `?` でそのまま返せる（文はその場の表示言語）
impl From<OutlineRefusal> for String {
    fn from(r: OutlineRefusal) -> Self {
        r.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 一始まり項目番号をジャンプ先へ解決する() {
        let outline = PreviewOutline::new(vec![
            PreviewOutlineItem {
                title: "概要".into(),
                level: 1,
                target: PreviewOutlineTarget::MarkdownBlock { block: 0 },
            },
            PreviewOutlineItem {
                title: "詳細".into(),
                level: 2,
                target: PreviewOutlineTarget::MarkdownBlock { block: 3 },
            },
        ]);
        assert_eq!(
            outline.target(2).unwrap(),
            PreviewOutlineTarget::MarkdownBlock { block: 3 }
        );
        assert_eq!(outline.target(0), Err(OutlineRefusal::ItemZero));
        assert_eq!(
            outline.target(3),
            Err(OutlineRefusal::OutOfRange { item: 3, total: 2 })
        );
    }

    /// #1661: 編集中の目次は原文の行を指す。ワイヤの形は `kind` で見分ける
    /// （CLI / MCP の呼び手は `source_line` なら行、`markdown_block` ならブロックと読む）
    #[test]
    fn 原文の行のジャンプ先はkindで見分けられる() {
        let target = PreviewOutlineTarget::SourceLine { line: 12 };
        let wire = serde_json::to_value(target).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "kind": "source_line", "line": 12 })
        );
        assert_eq!(
            serde_json::from_value::<PreviewOutlineTarget>(wire).unwrap(),
            target
        );
    }
}
