//! PDF プレビュー内リンクモデル（Issue #271）。
//!
//! PDFKit アノテーションから抽出した外部 URL / 内部ページリンクを保持する。
//! ロード時に 1 回だけ構築し、render は完成済みデータを参照するだけ。
//! ヒットテストはビューポート座標変換後の矩形で行う。

use serde::{Deserialize, Serialize};

/// PDF リンクのジャンプ先。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PdfLinkTarget {
    /// 外部 URL（ブラウザで開く）。
    Url { url: String },
    /// 内部リンク（PDF 内の別ページへジャンプ。1 始まり）。
    Page { page: usize },
}

/// PDF アノテーションから抽出した 1 リンク。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfLink {
    /// リンクが属するページ（0 始まり、内部表現）。
    pub page_index: usize,
    /// PDF 座標系でのリンク矩形 [x, y, width, height]（左下原点）。
    pub bbox: [f64; 4],
    /// リンク先。
    pub target: PdfLinkTarget,
}

/// ページごとのリンク一覧。ロード時に 1 回構築する。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PdfLinks {
    pub links: Vec<PdfLink>,
}

impl PdfLinks {
    pub fn new(links: Vec<PdfLink>) -> Self {
        Self { links }
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// 指定ページのリンクだけを返す（0 始まり）。
    pub fn links_for_page(&self, page_index: usize) -> impl Iterator<Item = &PdfLink> {
        self.links
            .iter()
            .filter(move |l| l.page_index == page_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ページ別フィルタが正しく動く() {
        let links = PdfLinks::new(vec![
            PdfLink {
                page_index: 0,
                bbox: [10.0, 20.0, 100.0, 15.0],
                target: PdfLinkTarget::Url {
                    url: "https://example.com".into(),
                },
            },
            PdfLink {
                page_index: 1,
                bbox: [30.0, 40.0, 80.0, 12.0],
                target: PdfLinkTarget::Page { page: 3 },
            },
            PdfLink {
                page_index: 0,
                bbox: [50.0, 60.0, 90.0, 14.0],
                target: PdfLinkTarget::Url {
                    url: "https://example.org".into(),
                },
            },
        ]);
        assert_eq!(links.links_for_page(0).count(), 2);
        assert_eq!(links.links_for_page(1).count(), 1);
        assert_eq!(links.links_for_page(2).count(), 0);
    }

    #[test]
    fn 空リンクが正しく判定される() {
        let links = PdfLinks::default();
        assert!(links.is_empty());
        assert_eq!(links.len(), 0);
    }
}
