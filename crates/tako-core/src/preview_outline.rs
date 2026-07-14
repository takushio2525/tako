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
    pub fn target(&self, item: usize) -> Result<PreviewOutlineTarget, String> {
        if item == 0 {
            return Err("アウトライン項目は 1 以上で指定する".into());
        }
        self.items
            .get(item - 1)
            .map(|entry| entry.target)
            .ok_or_else(|| {
                format!(
                    "アウトライン項目の範囲外: {item}（全 {} 件）",
                    self.items.len()
                )
            })
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
        assert!(outline.target(0).is_err());
        assert!(outline.target(3).is_err());
    }
}
