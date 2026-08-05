//! 抽象境界 B12 前半（PDF レンダラ）。
//!
//! 呼び出し側（`preview::load_pdf_with_key` / `preview::rasterize_pdf`）が知ってよいのは
//! このモジュールの関数と [`PdfCapabilities`] だけで、**`#[cfg(target_os)]` を書いてよいのは
//! このファイルの実装選択 1 箇所に限る**（`.agent/plans/2026-07-windows-port-architecture.md` 原則 1）。
//!
//! ## プラットフォームごとの実体（#521 / #693）
//!
//! | | ラスタライズ | テキストレイヤ | 目次（しおり） | リンク注釈 |
//! |---|---|---|---|---|
//! | macOS | Core Graphics | PDFKit | PDFKit | PDFKit |
//! | Windows | Windows.Data.Pdf | pdf-extract (#693) | lopdf (#693) | lopdf (#693) |
//! | その他 | 無し | 無し | 無し | 無し |
//!
//! Windows の WinRT レンダラは**ページを画像にすることしかできない**ので、
//! 残る 3 つは #693 で PDF ファイルを直接読んで補った（`windows.rs` のモジュール doc）。
//! テキストレイヤの無い PDF（スキャン画像など）は macOS でも普通にあり、
//! 描画側（`preview_render`）はその場合の分岐を既に持っている。
//!
//! ## なぜ OS 標準のレンダラ + PDF パーサか
//!
//! macOS が PDFKit / Core Graphics（= OS 標準）を使っているのと同じ形にすると、
//! B12 が「両 OS とも OS のレンダラで描く」1 つの構造に収まる。PDFium を同梱すれば
//! 描画もテキストもリンクも一度に手に入るが、対価が `pdfium.dll` 約 6〜11 MB の配布物追加
//! （Inno Setup インストーラー・ポータブル zip・配布物検査すべてに波及）で高い。
//! `lopdf` / `pdf-extract` は pure Rust なので追加の再頒布物が要らない。
//! 選定の詳細は #521 / #693 のコメントを参照。

use std::path::Path;

use tako_core::{PdfLinks, PreviewOutline};

use crate::preview::{PdfRasterKey, PdfRasterizedPages, PdfTextLine};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

// 実装選択（cfg を書いてよい唯一の場所）
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as imp;
#[cfg(target_os = "windows")]
use windows as imp;

/// この環境の PDF レンダラで何が取れるか。
///
/// UI・診断・サポートマトリクスが「なぜ空なのか」を同じ 1 つの事実から説明できるようにする。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfCapabilities {
    /// ページを画像へ描けるか。false ならプレビューを開くこと自体ができない
    pub rasterize: bool,
    /// テキストレイヤ（選択・コピー・ヒットテスト）が取れるか。
    ///
    /// true でも**行のまとまり方は実装によって差が出る**。macOS は PDFKit の版面解析、
    /// Windows は字送りと位置指定からの復元（`windows.rs` の `build_lines`）で、
    /// 後者は段組を空きの広さで切り分ける近似になる。選択の単位である文字矩形は
    /// どちらも 1 文字ずつ実測値から作る
    pub text_layer: bool,
    /// PDF 自身の目次（しおり）が取れるか。
    /// 目次パネルの「ページへ移動」は `total_pages` 由来で、これとは独立に動く
    pub outline: bool,
    /// リンク注釈（`tako_preview_link_list` / `follow_link`）が取れるか
    pub links: bool,
}

/// この環境の PDF レンダラの能力。
pub fn capabilities() -> PdfCapabilities {
    imp::CAPABILITIES
}

/// 全ページを指定の表示条件でラスタライズして PNG バイト列にする。
///
/// 呼び出し側が background executor 上で実行する前提（UI スレッドから呼ばない）。
pub fn render_all_pages(
    path: &Path,
    raster_key: PdfRasterKey,
) -> Result<PdfRasterizedPages, String> {
    imp::render_all_pages(path, raster_key)
}

/// ページごとのテキスト行（選択・コピー用）。取れない環境では空を返す。
pub fn extract_text_layers(
    path: &Path,
    total_pages: usize,
) -> Result<Vec<Vec<PdfTextLine>>, String> {
    imp::extract_text_layers(path, total_pages)
}

/// PDF 自身の目次（しおり）。取れない環境では空を返す。
pub fn extract_outline(path: &Path, total_pages: usize) -> Result<PreviewOutline, String> {
    imp::extract_outline(path, total_pages)
}

/// リンク注釈。取れない環境では空を返す。
pub fn extract_links(path: &Path, total_pages: usize) -> Result<PdfLinks, String> {
    imp::extract_links(path, total_pages)
}

/// PDF レンダラを持たない環境（macOS / Windows 以外）。
///
/// `preview.rs` 側が `capabilities().rasterize` を見て理由つきのエラーを表示するので、
/// ここは「呼ばれたら失敗する」だけでよい。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use super::*;

    pub(super) const CAPABILITIES: PdfCapabilities = PdfCapabilities {
        rasterize: false,
        text_layer: false,
        outline: false,
        links: false,
    };

    pub(super) fn render_all_pages(
        _path: &Path,
        _raster_key: PdfRasterKey,
    ) -> Result<PdfRasterizedPages, String> {
        Err(crate::ui_text::preview::pdf_unsupported_platform().to_string())
    }

    pub(super) fn extract_text_layers(
        _path: &Path,
        _total_pages: usize,
    ) -> Result<Vec<Vec<PdfTextLine>>, String> {
        Ok(Vec::new())
    }

    pub(super) fn extract_outline(
        _path: &Path,
        _total_pages: usize,
    ) -> Result<PreviewOutline, String> {
        Ok(PreviewOutline::default())
    }

    pub(super) fn extract_links(_path: &Path, _total_pages: usize) -> Result<PdfLinks, String> {
        Ok(PdfLinks::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 能力表は「描けないのにテキストだけ取れる」のような矛盾した組み合わせを持たない。
    /// ラスタライズできない環境では他も全部 false になる
    #[test]
    fn 能力表は矛盾しない() {
        let caps = capabilities();
        if !caps.rasterize {
            assert!(!caps.text_layer && !caps.outline && !caps.links);
        }
    }

    /// この境界を通す意味は「呼び出し側から cfg を消す」ことにある。
    /// どのプラットフォームでも同じ 4 関数 + 能力照会が生えていることを型で固定する
    #[test]
    fn どのプラットフォームでも同じapiが生えている() {
        let missing = std::path::Path::new("no-such-file-for-parity-check.pdf");
        // 不在ファイルなので中身は問わない。ここで見たいのは「呼べること」だけ
        let _ = render_all_pages(missing, PdfRasterKey::for_view(2.0, 1.0, 612.0));
        let _ = extract_text_layers(missing, 0);
        let _ = extract_outline(missing, 0);
        let _ = extract_links(missing, 0);
    }
}
