//! 抽象境界 B12（ドキュメントレンダラ）の Windows 実装 — `Windows.Data.Pdf` + `lopdf`（#521 / #693）。
//!
//! Windows 10 以降に OS 同梱の WinRT PDF レンダラ（Edge の PDF 表示と同じエンジン）を使う。
//! 追加の配布物は要らず、`windows` crate は既に依存グラフの中にいる（gpui / wry 経由）ので
//! feature を足すだけで済む。macOS が OS 標準の PDFKit / Core Graphics を使っているのと
//! 同じ構造になる。
//!
//! ## リンク注釈・目次・テキスト (#693)
//!
//! Windows.Data.Pdf にはリンク注釈・目次（しおり）・テキスト抽出の API が**存在しない**。
//! そこで PDF オブジェクトツリーの構造パーサ `lopdf`（pure Rust・MIT）を併用し、
//! ラスタライズは WinRT、メタデータ抽出は lopdf で補う。
//!
//! - **リンク注釈**: ページの `/Annots` → `/Link` サブタイプ → `/A`（URI アクション）/
//!   `/Dest`（内部ページ）を読む。`/Rect` から PDF 座標の矩形を取得
//! - **目次**: ドキュメントカタログの `/Outlines` ツリーを走査
//! - **テキスト抽出**: content stream のパースとフォントエンコーディングの解決が要るため
//!   lopdf 単体では困難。[`CAPABILITIES`] で `text_layer: false` を立てる
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
use lopdf::Document as LopdfDocument;
use tako_core::{
    PdfLink, PdfLinkTarget, PdfLinks, PreviewOutline, PreviewOutlineItem, PreviewOutlineTarget,
};
use windows::Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};

use super::PdfCapabilities;
use crate::preview::{PdfRasterKey, PdfRasterizedPages, PdfTextLine};

pub(super) const CAPABILITIES: PdfCapabilities = PdfCapabilities {
    rasterize: true,
    // content stream + font encoding が要るので lopdf 単体では困難（#693）
    text_layer: false,
    // lopdf で PDF オブジェクトツリーから読む（#693）
    outline: true,
    links: true,
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

/// テキスト抽出には content stream のパースとフォントエンコーディングの解決が必要で、
/// lopdf 単体では困難（#693）。テキストレイヤの無い PDF は macOS でも普通にあり、
/// 描画側はその分岐を持っているのでエラーではなく空で返す
pub fn extract_text_layers(
    _path: &Path,
    _total_pages: usize,
) -> Result<Vec<Vec<PdfTextLine>>, String> {
    Ok(Vec::new())
}

/// lopdf で PDF の `/Outlines` ツリー（しおり）を走査する（#693）。
/// 壊れた PDF や取れない場合はエラーではなく空を返す（macOS 実装と同じ縮退）
pub fn extract_outline(path: &Path, total_pages: usize) -> Result<PreviewOutline, String> {
    let doc = match LopdfDocument::load(path) {
        Ok(d) => d,
        Err(_) => return Ok(PreviewOutline::default()),
    };
    Ok(lopdf_extract_outline(&doc, total_pages))
}

/// lopdf でリンク注釈を抽出する（#693）。
/// `/Annots` → `/Link` サブタイプ → `/A`（URI）/ `/Dest`（内部ページ）
pub fn extract_links(path: &Path, total_pages: usize) -> Result<PdfLinks, String> {
    let doc = match LopdfDocument::load(path) {
        Ok(d) => d,
        Err(_) => return Ok(PdfLinks::default()),
    };
    Ok(lopdf_extract_links(&doc, total_pages))
}

// --- lopdf によるリンク注釈抽出 ---

/// ページの `/Annots` 配列から `/Link` サブタイプの注釈を読み、
/// `/A`（URI アクション）と `/Dest`（内部ページジャンプ）を取り出す。
fn lopdf_extract_links(doc: &LopdfDocument, total_pages: usize) -> PdfLinks {
    let pages = doc.get_pages();
    let mut links = Vec::new();

    for (&page_num, &page_id) in &pages {
        let page_index = (page_num as usize).saturating_sub(1);
        if page_index >= total_pages {
            continue;
        }
        let Ok(page_obj) = doc.get_object(page_id) else {
            continue;
        };
        let Ok(page_dict) = page_obj.as_dict() else {
            continue;
        };
        let annots = match page_dict.get(b"Annots") {
            Ok(annots_ref) => annots_ref,
            Err(_) => continue,
        };
        let annot_ids = match resolve_array(doc, annots) {
            Some(ids) => ids,
            None => continue,
        };
        for annot_ref in annot_ids {
            let annot_id = match annot_ref.as_reference() {
                Ok(id) => id,
                Err(_) => continue,
            };
            let Ok(annot_obj) = doc.get_object(annot_id) else {
                continue;
            };
            let Ok(annot_dict) = annot_obj.as_dict() else {
                continue;
            };
            if !is_link_annotation(annot_dict) {
                continue;
            }
            let bbox = match extract_rect(annot_dict) {
                Some(r) => r,
                None => continue,
            };
            if let Some(target) = extract_link_target(doc, annot_dict, &pages) {
                links.push(PdfLink {
                    page_index,
                    bbox,
                    target,
                });
            }
        }
    }
    PdfLinks::new(links)
}

/// `/Subtype` が `/Link` であるか
fn is_link_annotation(dict: &lopdf::Dictionary) -> bool {
    dict.get(b"Subtype")
        .ok()
        .and_then(|v| v.as_name().ok())
        .is_some_and(|name| name == b"Link")
}

/// `/Rect [x1, y1, x2, y2]` を `[x, y, width, height]`（PDF 座標、左下原点）に変換。
/// macOS の PDFKit と同じ形式にする
fn extract_rect(dict: &lopdf::Dictionary) -> Option<[f64; 4]> {
    let rect_obj = dict.get(b"Rect").ok()?;
    let rect_arr = rect_obj.as_array().ok()?;
    if rect_arr.len() < 4 {
        return None;
    }
    let x1 = obj_to_f64(&rect_arr[0])?;
    let y1 = obj_to_f64(&rect_arr[1])?;
    let x2 = obj_to_f64(&rect_arr[2])?;
    let y2 = obj_to_f64(&rect_arr[3])?;
    let x = x1.min(x2);
    let y = y1.min(y2);
    Some([x, y, (x2 - x1).abs(), (y2 - y1).abs()])
}

/// リンクの飛び先を取り出す。`/A`（アクション辞書）を優先し、無ければ `/Dest` を試す
fn extract_link_target(
    doc: &LopdfDocument,
    annot_dict: &lopdf::Dictionary,
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
) -> Option<PdfLinkTarget> {
    if let Ok(action) = annot_dict.get(b"A") {
        let action_dict = resolve_dict(doc, action)?;
        let action_type = action_dict.get(b"S").ok()?.as_name().ok()?;
        match action_type {
            b"URI" => {
                let uri = action_dict.get(b"URI").ok()?;
                let url = resolve_string(doc, uri)?;
                if !url.is_empty() {
                    return Some(PdfLinkTarget::Url { url });
                }
            }
            b"GoTo" => {
                let dest = action_dict.get(b"D").ok()?;
                return resolve_destination(doc, dest, pages);
            }
            _ => {}
        }
    }
    if let Ok(dest) = annot_dict.get(b"Dest") {
        return resolve_destination(doc, dest, pages);
    }
    None
}

/// `/Dest` の値（配列 or 名前 or 文字列）からページ番号を解決する。
/// 配列形式: `[page_ref /XYZ ...]` — 最初の要素がページオブジェクトへの参照
fn resolve_destination(
    doc: &LopdfDocument,
    dest: &lopdf::Object,
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
) -> Option<PdfLinkTarget> {
    match dest {
        lopdf::Object::Array(arr) if !arr.is_empty() => {
            let page_ref = arr[0].as_reference().ok()?;
            let page_num = page_id_to_number(page_ref, pages)?;
            Some(PdfLinkTarget::Page {
                page: page_num as usize,
            })
        }
        lopdf::Object::Name(name) | lopdf::Object::String(name, _) => {
            resolve_named_dest(doc, name, pages)
        }
        lopdf::Object::Reference(id) => {
            let resolved = doc.get_object(*id).ok()?;
            resolve_destination(doc, resolved, pages)
        }
        _ => None,
    }
}

/// ページオブジェクト ID から 1 始まりのページ番号を引く
fn page_id_to_number(
    page_id: lopdf::ObjectId,
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
) -> Option<u32> {
    pages
        .iter()
        .find(|(_, &id)| id == page_id)
        .map(|(&num, _)| num)
}

/// `/Names` → `/Dests` ツリーから名前付き行き先を解決する
fn resolve_named_dest(
    doc: &LopdfDocument,
    name: &[u8],
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
) -> Option<PdfLinkTarget> {
    let catalog = doc.catalog().ok()?;
    let names_ref = catalog.get(b"Names").ok()?;
    let names_dict = resolve_dict(doc, names_ref)?;
    let dests_ref = names_dict.get(b"Dests").ok()?;
    let dests_dict = resolve_dict(doc, dests_ref)?;
    lookup_name_tree(doc, &dests_dict, name, pages)
}

/// PDF の Name Tree をたどる
fn lookup_name_tree(
    doc: &LopdfDocument,
    node: &lopdf::Dictionary,
    name: &[u8],
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
) -> Option<PdfLinkTarget> {
    if let Ok(names_arr) = node.get(b"Names") {
        let arr = resolve_array(doc, names_arr)?;
        let mut i = 0;
        while i + 1 < arr.len() {
            let key = match &arr[i] {
                lopdf::Object::String(s, _) => s.as_slice(),
                lopdf::Object::Name(n) => n.as_slice(),
                _ => {
                    i += 2;
                    continue;
                }
            };
            if key == name {
                return resolve_destination(doc, &arr[i + 1], pages);
            }
            i += 2;
        }
    }
    if let Ok(kids_arr) = node.get(b"Kids") {
        let kids = resolve_array(doc, kids_arr)?;
        for kid in kids {
            let kid_dict = resolve_dict(doc, kid)?;
            if let Some(result) = lookup_name_tree(doc, &kid_dict, name, pages) {
                return Some(result);
            }
        }
    }
    None
}

// --- lopdf によるアウトライン（しおり）抽出 ---

/// `/Outlines` ツリーを走査して平坦な目次を組み立てる
fn lopdf_extract_outline(doc: &LopdfDocument, total_pages: usize) -> PreviewOutline {
    const MAX_ITEMS: usize = 5_000;
    const MAX_DEPTH: u8 = 32;

    let pages = doc.get_pages();
    let catalog = match doc.catalog() {
        Ok(c) => c,
        Err(_) => return PreviewOutline::default(),
    };
    let outlines_ref = match catalog.get(b"Outlines") {
        Ok(r) => r,
        Err(_) => return PreviewOutline::default(),
    };
    let outlines_dict = match resolve_dict(doc, outlines_ref) {
        Some(d) => d,
        None => return PreviewOutline::default(),
    };
    let first = match outlines_dict.get(b"First") {
        Ok(f) => f,
        Err(_) => return PreviewOutline::default(),
    };

    let mut items = Vec::new();
    collect_outline_items(
        doc,
        first,
        1,
        total_pages,
        &pages,
        &mut items,
        MAX_DEPTH,
        MAX_ITEMS,
    );
    PreviewOutline::new(items)
}

fn collect_outline_items(
    doc: &LopdfDocument,
    item_ref: &lopdf::Object,
    level: u8,
    total_pages: usize,
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
    items: &mut Vec<PreviewOutlineItem>,
    max_depth: u8,
    max_items: usize,
) {
    if level > max_depth || items.len() >= max_items {
        return;
    }
    let dict = match resolve_dict(doc, item_ref) {
        Some(d) => d,
        None => return,
    };
    let title = dict
        .get(b"Title")
        .ok()
        .and_then(|t| resolve_string(doc, t))
        .unwrap_or_default()
        .trim()
        .to_string();

    if !title.is_empty() {
        let page_target = resolve_outline_dest(doc, &dict, pages);
        if let Some(page) = page_target {
            if page <= total_pages {
                items.push(PreviewOutlineItem {
                    title,
                    level,
                    target: PreviewOutlineTarget::PdfPage { page },
                });
            }
        }
    }

    if let Ok(first_child) = dict.get(b"First") {
        collect_outline_items(
            doc,
            first_child,
            level.saturating_add(1),
            total_pages,
            pages,
            items,
            max_depth,
            max_items,
        );
    }

    if let Ok(next) = dict.get(b"Next") {
        collect_outline_items(
            doc,
            next,
            level,
            total_pages,
            pages,
            items,
            max_depth,
            max_items,
        );
    }
}

/// アウトライン項目の飛び先（`/Dest` または `/A` の GoTo）を 1 始まりのページ番号に解決する
fn resolve_outline_dest(
    doc: &LopdfDocument,
    dict: &lopdf::Dictionary,
    pages: &std::collections::BTreeMap<u32, lopdf::ObjectId>,
) -> Option<usize> {
    if let Ok(dest) = dict.get(b"Dest") {
        if let Some(PdfLinkTarget::Page { page }) = resolve_destination(doc, dest, pages) {
            return Some(page);
        }
    }
    if let Ok(action) = dict.get(b"A") {
        let action_dict = resolve_dict(doc, action)?;
        let action_type = action_dict.get(b"S").ok()?.as_name().ok()?;
        if action_type == b"GoTo" {
            let dest = action_dict.get(b"D").ok()?;
            if let Some(PdfLinkTarget::Page { page }) = resolve_destination(doc, dest, pages) {
                return Some(page);
            }
        }
    }
    None
}

// --- lopdf ヘルパー ---

/// 参照を解決して辞書を取り出す
fn resolve_dict<'a>(
    doc: &'a LopdfDocument,
    obj: &'a lopdf::Object,
) -> Option<&'a lopdf::Dictionary> {
    match obj {
        lopdf::Object::Dictionary(d) => Some(d),
        lopdf::Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()),
        _ => None,
    }
}

/// 参照を解決して配列を取り出す
fn resolve_array<'a>(
    doc: &'a LopdfDocument,
    obj: &'a lopdf::Object,
) -> Option<&'a Vec<lopdf::Object>> {
    match obj {
        lopdf::Object::Array(a) => Some(a),
        lopdf::Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_array().ok()),
        _ => None,
    }
}

/// PDF オブジェクトから文字列を取り出す（Name / String / 参照を解決）
fn resolve_string(doc: &LopdfDocument, obj: &lopdf::Object) -> Option<String> {
    match obj {
        lopdf::Object::String(bytes, _) => {
            if bytes.starts_with(&[0xFE, 0xFF]) {
                // UTF-16BE BOM 付き
                let u16s: Vec<u16> = bytes[2..]
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16(&u16s).ok()
            } else {
                Some(String::from_utf8_lossy(bytes).into_owned())
            }
        }
        lopdf::Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        lopdf::Object::Reference(id) => {
            let resolved = doc.get_object(*id).ok()?;
            resolve_string(doc, resolved)
        }
        _ => None,
    }
}

/// 数値（Integer / Real）を f64 に変換
fn obj_to_f64(obj: &lopdf::Object) -> Option<f64> {
    match obj {
        lopdf::Object::Integer(i) => Some(*i as f64),
        lopdf::Object::Real(f) => Some(*f as f64),
        _ => None,
    }
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

    /// テキストレイヤは取れないが Err ではなく空を返す
    #[test]
    fn テキストレイヤは空で返る() {
        let path = Path::new("no-such.pdf");
        assert!(extract_text_layers(path, 3).unwrap().is_empty());
    }

    /// 壊れた PDF や不在ファイルでリンク・アウトラインはパニックせず空を返す
    #[test]
    fn 異常なpdfでリンクとアウトラインは空で返る() {
        let dir = std::env::temp_dir().join("tako_pdf_win_link_error_test");
        std::fs::create_dir_all(&dir).unwrap();

        let missing = dir.join("no-such.pdf");
        assert!(extract_links(&missing, 1).unwrap().is_empty());
        assert!(extract_outline(&missing, 1).unwrap().is_empty());

        let broken = dir.join("broken.pdf");
        std::fs::write(&broken, b"not a pdf").unwrap();
        assert!(extract_links(&broken, 1).unwrap().is_empty());
        assert!(extract_outline(&broken, 1).unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// リンク入り PDF を自前生成し、lopdf で URI リンクと内部リンクの両方が読めることを検証
    #[test]
    fn リンク注釈を抽出できる() {
        let dir = std::env::temp_dir().join("tako_pdf_win_links_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("with_links.pdf");
        std::fs::write(&path, pdf_with_links()).unwrap();

        let links = extract_links(&path, 2).unwrap();
        assert!(!links.is_empty(), "リンクが 1 つ以上ある");

        let url_links: Vec<_> = links
            .links
            .iter()
            .filter(|l| matches!(&l.target, PdfLinkTarget::Url { .. }))
            .collect();
        assert!(
            !url_links.is_empty(),
            "URI リンクが 1 つ以上ある: {:?}",
            links.links
        );
        if let PdfLinkTarget::Url { url } = &url_links[0].target {
            assert_eq!(url, "https://example.com/");
        }

        let page_links: Vec<_> = links
            .links
            .iter()
            .filter(|l| matches!(&l.target, PdfLinkTarget::Page { .. }))
            .collect();
        assert!(
            !page_links.is_empty(),
            "内部リンクが 1 つ以上ある: {:?}",
            links.links
        );
        if let PdfLinkTarget::Page { page } = &page_links[0].target {
            assert_eq!(*page, 2, "2 ページ目への内部リンク");
        }

        // 矩形が妥当な値を持つ
        for link in &links.links {
            assert!(link.bbox[2] > 0.0, "幅 > 0");
            assert!(link.bbox[3] > 0.0, "高さ > 0");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// アウトライン入り PDF でしおりが読めることを検証
    #[test]
    fn アウトラインを抽出できる() {
        let dir = std::env::temp_dir().join("tako_pdf_win_outline_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("with_outline.pdf");
        std::fs::write(&path, pdf_with_outline()).unwrap();

        let outline = extract_outline(&path, 2).unwrap();
        assert!(!outline.is_empty(), "アウトラインが空でない");
        let items = outline.items;
        assert!(items.len() >= 2, "2 項目以上ある: {:?}", items);
        assert_eq!(items[0].title, "Chapter 1");
        assert_eq!(items[1].title, "Chapter 2");
        if let PreviewOutlineTarget::PdfPage { page } = &items[0].target {
            assert_eq!(*page, 1);
        }
        if let PreviewOutlineTarget::PdfPage { page } = &items[1].target {
            assert_eq!(*page, 2);
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// リンク注釈を含む 2 ページ PDF を生成。
    /// ページ 1: URI リンク（https://example.com/）+ 内部リンク（2 ページ目へ）
    fn pdf_with_links() -> Vec<u8> {
        // obj 1: Catalog
        // obj 2: Pages
        // obj 3: Page 1（リンク注釈あり）
        // obj 4: Page 2
        // obj 5: Page 1 の Contents
        // obj 6: Page 2 の Contents
        // obj 7: URI リンク注釈
        // obj 8: 内部リンク注釈
        let content1 = b"BT /F1 12 Tf 50 700 Td (Page 1) Tj ET";
        let content2 = b"BT /F1 12 Tf 50 700 Td (Page 2) Tj ET";

        let mut pdf = Vec::with_capacity(2048);
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(8);

        // 1: Catalog
        offsets.push(pdf.len());
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // 2: Pages
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n",
        );

        // 3: Page 1（Annots 付き）
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Contents 5 0 R /Annots [7 0 R 8 0 R] \
              /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>\nendobj\n",
        );

        // 4: Page 2
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Contents 6 0 R \
              /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>\nendobj\n",
        );

        // 5: Contents for page 1
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!("5 0 obj\n<< /Length {} >>\nstream\n", content1.len()).as_bytes(),
        );
        pdf.extend_from_slice(content1);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // 6: Contents for page 2
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!("6 0 obj\n<< /Length {} >>\nstream\n", content2.len()).as_bytes(),
        );
        pdf.extend_from_slice(content2);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // 7: URI Link annotation
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"7 0 obj\n<< /Type /Annot /Subtype /Link /Rect [50 680 200 700] \
              /A << /S /URI /URI (https://example.com/) >> >>\nendobj\n",
        );

        // 8: Internal link annotation (GoTo page 2)
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"8 0 obj\n<< /Type /Annot /Subtype /Link /Rect [50 650 200 670] \
              /Dest [4 0 R /XYZ 0 792 0] >>\nendobj\n",
        );

        let xref_at = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    /// アウトライン入り 2 ページ PDF を生成
    fn pdf_with_outline() -> Vec<u8> {
        // obj 1: Catalog（Outlines 付き）
        // obj 2: Pages
        // obj 3: Page 1
        // obj 4: Page 2
        // obj 5: Page 1 Contents
        // obj 6: Page 2 Contents
        // obj 7: Outlines root
        // obj 8: Outline item "Chapter 1"
        // obj 9: Outline item "Chapter 2"
        let content = b"BT /F1 12 Tf 50 700 Td (text) Tj ET";

        let mut pdf = Vec::with_capacity(2048);
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(9);

        // 1: Catalog
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Outlines 7 0 R >>\nendobj\n",
        );

        // 2: Pages
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n",
        );

        // 3: Page 1
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R \
              /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>\nendobj\n",
        );

        // 4: Page 2
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R \
              /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>\nendobj\n",
        );

        // 5: Contents 1
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!("5 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // 6: Contents 2
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!("6 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // 7: Outlines root
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"7 0 obj\n<< /Type /Outlines /First 8 0 R /Last 9 0 R /Count 2 >>\nendobj\n",
        );

        // 8: Outline item 1
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"8 0 obj\n<< /Title (Chapter 1) /Parent 7 0 R /Next 9 0 R \
              /Dest [3 0 R /XYZ 0 792 0] >>\nendobj\n",
        );

        // 9: Outline item 2
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"9 0 obj\n<< /Title (Chapter 2) /Parent 7 0 R /Prev 8 0 R \
              /Dest [4 0 R /XYZ 0 792 0] >>\nendobj\n",
        );

        let xref_at = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }
}
