//! 抽象境界 B12（ドキュメントレンダラ）の Windows 実装 — `Windows.Data.Pdf`（#521）。
//!
//! Windows 10 以降に OS 同梱の WinRT PDF レンダラ（Edge の PDF 表示と同じエンジン）を使う。
//! 追加の配布物は要らず、`windows` crate は既に依存グラフの中にいる（gpui / wry 経由）ので
//! feature を足すだけで済む。macOS が OS 標準の PDFKit / Core Graphics を使っているのと
//! 同じ構造になる。
//!
//! ## 取れないもの
//!
//! テキストレイヤ・目次（しおり）・リンク注釈は **API 自体が存在しない**。
//! [`CAPABILITIES`] で false を立て、空の結果を返す（追跡は #693）。
//!
//! ## 単位の違い（罠）
//!
//! `PdfPage::Size` は **96 DPI のピクセル**を返す。Core Graphics の `CGPDFPageGetBoxRect` は
//! **ポイント**なので、そのまま `page_sizes` へ入れると 4/3 倍ずれる。ここで pt へ正規化する
//! （実測: MediaBox 595x842 の PDF が 793.33 x 1122.67 で返る = 96/72 倍）。
//!
//! ## 終了時の ACCESS_VIOLATION と「番人」（重要）
//!
//! このレンダラは Direct2D / Direct3D で描くため、**1 ページでもラスタライズしたあと
//! 関連 COM オブジェクトを全部解放すると、プロセス終了処理で GPU ドライバ DLL の中で
//! `0xC0000005` が出る**（実測環境では AMD の `atidxx64.dll`。ベンダ依存なので
//! 「自分の機で出ない」は保証にならない）。切り分けの実測:
//!
//! | 条件 | 終了コード |
//! |---|---|
//! | 開くだけ / ページを取るだけ（描画しない） | 0 |
//! | 描画して全部解放する | **-1073741819** |
//! | 描画後 500ms 待ってから終了 | -1073741819（時間の競合ではない） |
//! | `CoInitializeEx(MTA)` を明示 / 別スレッドで実行 | -1073741819（COM 初期化とは無関係） |
//! | **1 度描画した `PdfDocument` を 1 つだけ解放しない** | **0** |
//! | 描画していない `PdfDocument` を解放しない | -1073741819（描画済みでないと効かない） |
//!
//! そこで [`pin_renderer_device`] が、数百バイトの 1 ページ PDF をメモリ上で組み立てて
//! 16px で 1 回描画し、その `PdfDocument` を解放せずプロセス寿命まで残す。
//! これでレンダラの D3D デバイスが生き続け、**実ドキュメントは普通に解放できる**
//! （72 ページ PDF を 3 スレッドから開いて捨てても終了コード 0 を実測）。
//!
//! 常駐コストは最小構成の PDF 1 つぶん。GPUI の rev や Windows の更新でこの回避が
//! 不要になったかを確かめたいときは、`pin_renderer_device` を空にして
//! `cargo test -p tako-app pdf` 後のプロセス終了コードを見ればよい。

use std::future::IntoFuture;
use std::path::Path;
use std::sync::OnceLock;

use futures::executor::block_on;
use tako_core::{PdfLinks, PreviewOutline};
use windows::Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};

use super::PdfCapabilities;
use crate::preview::{PdfRasterKey, PdfRasterizedPages, PdfTextLine};

pub(super) const CAPABILITIES: PdfCapabilities = PdfCapabilities {
    rasterize: true,
    // 以下 3 つは Windows.Data.Pdf に API が無い（#693）
    text_layer: false,
    outline: false,
    links: false,
};

pub fn render_all_pages(
    path: &Path,
    raster_key: PdfRasterKey,
) -> Result<PdfRasterizedPages, String> {
    pin_renderer_device();

    let doc = open_document(path)?;
    let total = doc
        .PageCount()
        .map_err(|e| format!("PDF のページ数を取得できない: {}", hresult(&e)))?
        as usize;
    if total == 0 {
        return Err("PDF にページがない".into());
    }

    let pixel_w = raster_key.target_pixel_width();
    let mut all_pages = Vec::with_capacity(total);
    let mut page_sizes = Vec::with_capacity(total);
    let mut pixel_sizes = Vec::with_capacity(total);
    // 空ページを黙って返すと「読み込み中のまま」に見えるので、最初の失敗だけ理由を残す
    let mut first_failure: Option<String> = None;

    for page_idx in 0..total {
        // 1 ページの失敗で全体を落とさない（macOS 実装と同じ縮退）。
        // 空 PNG のページは描画側が「まだ無い」として扱える
        let Ok(page) = doc.GetPage(page_idx as u32) else {
            all_pages.push(Vec::new());
            page_sizes.push([0.0, 0.0]);
            pixel_sizes.push([0, 0]);
            continue;
        };
        let Ok(size) = page.Size() else {
            all_pages.push(Vec::new());
            page_sizes.push([0.0, 0.0]);
            pixel_sizes.push([0, 0]);
            continue;
        };

        // Size は 96 DPI px。PDF 座標系（pt）へ戻す
        let pt_w = f64::from(size.Width) * 72.0 / 96.0;
        let pt_h = f64::from(size.Height) * 72.0 / 96.0;
        page_sizes.push([pt_w, pt_h]);

        let render_scale = f64::from(pixel_w) / pt_w.max(1.0);
        let pixel_h = (pt_h * render_scale).ceil() as u32;
        pixel_sizes.push([pixel_w, pixel_h]);
        if pixel_w == 0 || pixel_h == 0 {
            all_pages.push(Vec::new());
            continue;
        }

        match render_page_png(&page, pixel_w, pixel_h) {
            Ok(png) => all_pages.push(png),
            Err(error) => {
                first_failure.get_or_insert(format!("{page_idx} ページ目: {error}"));
                all_pages.push(Vec::new());
            }
        }
    }

    if let Some(reason) = first_failure {
        let failed = all_pages.iter().filter(|page| page.is_empty()).count();
        eprintln!("warning: PDF のページ描画に失敗（{failed}/{total} ページ）: {reason}");
    }

    if std::env::var_os("TAKO_PERF_VERBOSE").is_some() {
        if let (Some(logical), Some(pixels)) = (page_sizes.first(), pixel_sizes.first()) {
            eprintln!(
                "TAKO_PDF_RASTER: pages={total} logical={:.0}x{:.0} pixels={}x{} device_scale={:.2} zoom={:.2}",
                logical[0],
                logical[1],
                pixels[0],
                pixels[1],
                f32::from(raster_key.device_scale_percent) / 100.0,
                f32::from(raster_key.zoom_percent) / 100.0,
            );
        }
    }

    Ok(PdfRasterizedPages {
        pages: all_pages,
        total_pages: total,
        page_sizes,
        pixel_sizes,
    })
}

/// Windows.Data.Pdf にテキスト抽出の API は無い（#693）。
/// テキストレイヤの無い PDF は macOS でも普通にあり、描画側はその分岐を持っているので
/// エラーではなく空で返す
pub fn extract_text_layers(
    _path: &Path,
    _total_pages: usize,
) -> Result<Vec<Vec<PdfTextLine>>, String> {
    Ok(Vec::new())
}

/// しおりの API は無い（#693）。目次パネルの「ページへ移動」は `total_pages` 由来なので
/// これが空でもページ送りは効く
pub fn extract_outline(_path: &Path, _total_pages: usize) -> Result<PreviewOutline, String> {
    Ok(PreviewOutline::default())
}

/// リンク注釈の API は無い（#693）
pub fn extract_links(_path: &Path, _total_pages: usize) -> Result<PdfLinks, String> {
    Ok(PdfLinks::default())
}

/// ファイルを読み、WinRT のメモリストリーム経由で `PdfDocument` を開く。
///
/// `StorageFile` 経由でも開けるが、ブローカー越しのアクセスは長いパス・ネットワークパス・
/// 権限で落ち方が増える。ここは自前で読んでからストリームに載せることで、
/// 「読めない」（io エラー）と「PDF として不正」（WinRT エラー）を切り分けられるようにする。
fn open_document(path: &Path) -> Result<PdfDocument, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("PDF を読み込めない: {e}"))?;
    if bytes.is_empty() {
        return Err("PDF を開けない".into());
    }
    load_document_from_bytes(&bytes).map_err(|e| {
        // 破損・非 PDF はここに来る。HRESULT を添えて診断できるようにする
        format!("PDF を開けない（{}）", hresult(&e))
    })
}

fn load_document_from_bytes(bytes: &[u8]) -> windows::core::Result<PdfDocument> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0)?)?;
    writer.WriteBytes(bytes)?;
    block_on(writer.StoreAsync()?.into_future())?;
    block_on(writer.FlushAsync()?.into_future())?;
    // DetachStream しないと DataWriter の Drop がストリームを閉じる
    writer.DetachStream()?;
    stream.Seek(0)?;
    block_on(PdfDocument::LoadFromStreamAsync(&stream)?.into_future())
}

/// 1 ページを PNG バイト列へ。`PdfPageRenderOptions` の既定エンコーダが PNG
/// （実測: BitmapEncoderId = 27949969-876A-41D7-9447-568F6A35A4DC）なので変換は要らない
fn render_page_png(page: &PdfPage, pixel_w: u32, pixel_h: u32) -> Result<Vec<u8>, String> {
    render_page_png_inner(page, pixel_w, pixel_h)
        .map_err(|e| format!("PDF ページの描画に失敗（{}）", hresult(&e)))
}

fn render_page_png_inner(
    page: &PdfPage,
    pixel_w: u32,
    pixel_h: u32,
) -> windows::core::Result<Vec<u8>> {
    let out = InMemoryRandomAccessStream::new()?;
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(pixel_w)?;
    options.SetDestinationHeight(pixel_h)?;
    block_on(
        page.RenderWithOptionsToStreamAsync(&out, &options)?
            .into_future(),
    )?;

    let len = out.Size()?;
    // 4GiB 超の PNG は現実に無い。read 側の u32 API に載る範囲へ落とす
    let len = u32::try_from(len).unwrap_or(u32::MAX);
    let reader = DataReader::CreateDataReader(&out.GetInputStreamAt(0)?)?;
    block_on(reader.LoadAsync(len)?.into_future())?;
    let mut buffer = vec![0u8; len as usize];
    reader.ReadBytes(&mut buffer)?;
    Ok(buffer)
}

/// レンダラの D3D デバイスをプロセス寿命まで固定する（モジュール doc の「番人」）。
///
/// 失敗しても PDF 表示自体は動くので、握りつぶして進む（終了時 AV のリスクだけが残る）。
///
/// `TAKO_PDF_NO_DEVICE_PIN=1` で無効化できる。**回避がまだ要るかを確かめるため**の口で、
/// 「PDF を 1 枚描いたプロセスの終了コード」を番人あり / なしで見比べられる
/// （なし = `-1073741819` なら回避はまだ効いている。両方 0 になったら消してよい）。
fn pin_renderer_device() {
    static PINNED: OnceLock<()> = OnceLock::new();
    PINNED.get_or_init(|| {
        if std::env::var_os("TAKO_PDF_NO_DEVICE_PIN").is_some() {
            eprintln!("warning: TAKO_PDF_NO_DEVICE_PIN により終了時 AV の回避を無効化した");
            return;
        }
        if let Ok(doc) = load_document_from_bytes(&minimal_pdf()) {
            if let Ok(page) = doc.GetPage(0) {
                // 1 度でも描画しないとデバイスが作られず、固定の意味が無い（実測）
                let _ = render_page_png_inner(&page, 16, 16);
            }
            // 意図的に解放しない。詳細はモジュール doc
            std::mem::forget(doc);
        }
    });
}

/// 番人用の最小 PDF（1 ページ・数百バイト）。外部ファイルに依存しないよう毎回組み立てる
fn minimal_pdf() -> Vec<u8> {
    const CONTENT: &[u8] = b"0 0 1 rg 4 4 8 8 re f";
    let mut pdf = Vec::with_capacity(512);
    pdf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(4);
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Contents 4 0 R /Resources << >> >>\nendobj\n",
    );
    offsets.push(pdf.len());
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", CONTENT.len()).as_bytes());
    pdf.extend_from_slice(CONTENT);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let xref_at = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n").as_bytes(),
    );
    pdf
}

/// 診断用に HRESULT を 16 進で残す。WinRT のメッセージは空のことがある
fn hresult(error: &windows::core::Error) -> String {
    let message = error.message();
    if message.trim().is_empty() {
        format!("{:#010x}", error.code().0)
    } else {
        format!("{:#010x}: {message}", error.code().0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 番人が「1 ページ PDF を作って描ける」ことを確かめる。ここが壊れると
    /// 終了時 AV の回避が黙って効かなくなる（回避が効いているかはプロセス終了コードで見る）
    #[test]
    fn 番人用の最小pdfは描画できる() {
        let doc = load_document_from_bytes(&minimal_pdf()).expect("最小 PDF を開ける");
        assert_eq!(doc.PageCount().unwrap(), 1);
        let page = doc.GetPage(0).unwrap();
        let png = render_page_png_inner(&page, 16, 16).expect("描画できる");
        assert_eq!(
            &png[..4],
            &[0x89, 0x50, 0x4E, 0x47],
            "既定エンコーダが PNG である"
        );
    }

    /// `PdfPage::Size` は 96DPI px なので、pt へ戻さないと macOS と 4/3 ずれる
    #[test]
    fn ページサイズはptへ正規化される() {
        // MediaBox 16x16 pt の最小 PDF
        let dir = std::env::temp_dir().join("tako_pdf_win_size_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("minimal.pdf");
        std::fs::write(&path, minimal_pdf()).unwrap();

        let rendered =
            render_all_pages(&path, PdfRasterKey::for_view(1.0, 1.0, 612.0)).expect("描画できる");
        assert_eq!(rendered.total_pages, 1);
        let [w, h] = rendered.page_sizes[0];
        assert!(
            (w - 16.0).abs() < 0.01 && (h - 16.0).abs() < 0.01,
            "pt へ正規化されている: got {w}x{h}（px のままなら 21.33）"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 壊れた PDF・不在・空はパニックせず理由つきの Err になる
    #[test]
    fn 異常なpdfはエラーになる() {
        let dir = std::env::temp_dir().join("tako_pdf_win_error_test");
        std::fs::create_dir_all(&dir).unwrap();
        let key = PdfRasterKey::for_view(1.0, 1.0, 612.0);

        let missing = dir.join("no-such.pdf");
        assert!(render_all_pages(&missing, key).is_err(), "不在");

        let broken = dir.join("broken.pdf");
        std::fs::write(&broken, b"this is definitely not a pdf").unwrap();
        assert!(render_all_pages(&broken, key).is_err(), "非 PDF");

        let empty = dir.join("empty.pdf");
        std::fs::write(&empty, b"").unwrap();
        assert!(render_all_pages(&empty, key).is_err(), "空");

        // 途中で切れた PDF（xref が壊れる）
        let truncated = dir.join("truncated.pdf");
        let full = minimal_pdf();
        std::fs::write(&truncated, &full[..full.len() / 2]).unwrap();
        assert!(render_all_pages(&truncated, key).is_err(), "切り詰め");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 縮退している 3 つは Err ではなく空を返す（描画側が正常系として扱えるように）
    #[test]
    fn 取れない情報はエラーではなく空で返る() {
        let path = Path::new("no-such.pdf");
        assert!(extract_text_layers(path, 3).unwrap().is_empty());
        assert!(extract_outline(path, 3).unwrap().is_empty());
        assert!(extract_links(path, 3).unwrap().is_empty());
    }
}
