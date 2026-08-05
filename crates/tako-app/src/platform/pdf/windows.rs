//! 抽象境界 B12（ドキュメントレンダラ）の Windows 実装 — `Windows.Data.Pdf` + `lopdf`（#521 / #693）。
//!
//! Windows 10 以降に OS 同梱の WinRT PDF レンダラ（Edge の PDF 表示と同じエンジン）を使う。
//! 追加の配布物は要らず、`windows` crate は既に依存グラフの中にいる（gpui / wry 経由）ので
//! feature を足すだけで済む。macOS が OS 標準の PDFKit / Core Graphics を使っているのと
//! 同じ構造になる。
//!
//! ## リンク注釈・目次・テキスト (#693)
//!
//! Windows.Data.Pdf にはリンク注釈・目次（しおり）・テキスト抽出の API が**存在しない**
//! （このレンダラができるのは「ページを画像にする」ことだけ）。そこで PDF ファイル側を
//! 直接読んで補う。ラスタライズは WinRT、それ以外は PDF の構造から組み立てる。
//!
//! - **リンク注釈**: ページの `/Annots` → `/Link` → `/A`（URI アクション）/ `/Dest`
//!   （内部ページ）を `lopdf` で読む。`/Rect` から矩形を取る
//! - **目次**: ドキュメントカタログの `/Outlines` ツリーを `lopdf` で走査
//! - **テキストレイヤ**: `pdf-extract` に content stream を解釈させ、文字ごとの
//!   変換行列・送り幅・フォントサイズを受け取って行と文字矩形へ組み直す（[`TextCollector`]）
//!
//! `lopdf` 単体では**字面すら取り出せない**（content stream の `Tj` が持つのはフォント固有の
//! バイト列で、Unicode へ落とすには標準 14 フォントの AFM 幅・`/Differences`・`ToUnicode`
//! CMap・CID の解決が要る）。そこを持っているのが `pdf-extract` の採用理由で、
//! バージョンは `lopdf` を共有させるために揃えてある（Cargo.toml のコメント参照）。
//!
//! ## 座標系
//!
//! 3 つとも **PDF 座標（左下原点）で、ページ矩形の左下を (0, 0) とした値**に揃える
//! （`preview_render::pdf_box_to_screen` がその前提で画面へ写す）。`/CropBox` があるページは
//! レンダラもそちらを描くので、原点は [`page_box_origin`] で CropBox 優先に解決して差し引く。
//!
//! ## 行の復元（テキストレイヤの限界）
//!
//! PDF に「行」は無く、字送りと位置指定の列があるだけなので [`build_lines`] が復元する。
//! macOS の PDFKit のような版面解析は持たないため、**段組は空きの広さで切り分ける近似**に
//! なる（[`LINE_JOIN_GAP_RATIO`]）。選択・ヒットテストの単位である文字矩形は
//! 1 文字ずつ実測値から作るので、この近似の影響を受けるのは行のまとまり方だけ。
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
use crate::preview::{PdfCharBox, PdfRasterKey, PdfRasterizedPages, PdfTextLine};

pub(super) const CAPABILITIES: PdfCapabilities = PdfCapabilities {
    rasterize: true,
    // 以下 3 つは WinRT レンダラには無く、PDF を直接読んで補っている（#693）
    text_layer: true,
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

/// `pdf-extract` に content stream を解釈させ、文字ごとの位置から行を組み立てる（#693）。
///
/// テキストレイヤを持たない PDF（スキャン画像など）は macOS でも普通にあり、描画側は
/// その分岐を持っているので、取れなければエラーではなく空で返す。
pub fn extract_text_layers(
    path: &Path,
    total_pages: usize,
) -> Result<Vec<Vec<PdfTextLine>>, String> {
    let Ok(doc) = LopdfDocument::load(path) else {
        return Ok(Vec::new());
    };
    let mut result = vec![Vec::new(); total_pages];
    for (&page_num, &page_id) in &doc.get_pages() {
        let page_index = (page_num as usize).saturating_sub(1);
        if page_index >= total_pages {
            continue;
        }
        if let Some(lines) = collect_page_text(&doc, page_num, page_box_origin(&doc, page_id)) {
            result[page_index] = lines;
        }
    }
    Ok(result)
}

/// 1 ページ分のテキストを採取する。取れなければ `None`（そのページだけ空になる）。
///
/// `pdf-extract` は不正な content stream に対して panic することがある
/// （テキスト表示演算子が `Tf` より先に来ると内部の `unwrap` が外れる）。ユーザーの PDF は
/// 何が入っているか分からないので、**1 ページの panic でアプリを道連れにしない**よう捕まえる。
/// 捕まえてもプロセス側の panic フックは動くので、原因は panic.log に残る。
fn collect_page_text(
    doc: &LopdfDocument,
    page_num: u32,
    origin: (f64, f64),
) -> Option<Vec<PdfTextLine>> {
    let collected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut collector = TextCollector::new(origin);
        pdf_extract::output_doc_page(doc, &mut collector, page_num).ok()?;
        Some(collector.into_lines())
    }));
    match collected {
        Ok(lines) => lines,
        Err(_) => {
            eprintln!("warning: PDF のテキスト抽出が {page_num} ページ目で異常終了した");
            None
        }
    }
}

// --- pdf-extract によるテキストレイヤ抽出 ---

/// 1 ページあたりの採取上限。壊れた PDF や生成物の暴走で青天井にしない
const MAX_CHARS_PER_PAGE: usize = 200_000;
/// 同じ行とみなすベースラインのずれ（実効フォントサイズ比）
const SAME_BASELINE_RATIO: f64 = 0.3;
/// 同じ行として繋いでよい水平方向の空き（実効フォントサイズ比）。
/// 単語ごとに `Tm` を置き直す生成器を繋ぎ直しつつ、段組は分けたままにするための境目
const LINE_JOIN_GAP_RATIO: f64 = 6.0;
/// 空白を 1 つ補う水平方向の空き（実効フォントサイズ比）。
/// PDF は単語の区切りを空白文字ではなく字送りだけで表すことがある
const SPACE_GAP_RATIO: f64 = 0.25;
/// ベースラインから下へ伸ばす量（ディセンダ相当。実効フォントサイズ比）
const DESCENT_RATIO: f64 = 0.2;
/// 送り幅ゼロのグリフに与える最小幅（実効フォントサイズ比）。
/// 幅 0 の矩形はヒットテストの的にならず、選択から取り残されてしまう
const MIN_GLYPH_WIDTH_RATIO: f64 = 0.05;

/// 採取した 1 文字。座標はページ矩形の左下を原点とする PDF 座標
struct RawChar {
    /// ベースライン左端の x
    x: f64,
    /// ベースラインの y
    y: f64,
    /// 送り幅
    advance: f64,
    /// 実効フォントサイズ（`Tm` / CTM の伸縮を畳んだ値）
    size: f64,
    text: String,
    /// 直前に `end_line()` が来た = ここから新しい run
    line_break: bool,
}

/// `pdf-extract` から文字を受け取って溜める。
///
/// この実装が返すのは「文字と、その置かれた位置」だけで、行にまとめるのは [`build_lines`]。
struct TextCollector {
    origin: (f64, f64),
    chars: Vec<RawChar>,
    pending_break: bool,
    overflowed: bool,
}

impl TextCollector {
    fn new(origin: (f64, f64)) -> Self {
        Self {
            origin,
            chars: Vec::new(),
            pending_break: false,
            overflowed: false,
        }
    }

    fn into_lines(self) -> Vec<PdfTextLine> {
        if self.overflowed {
            eprintln!(
                "warning: PDF 1 ページの文字数が上限（{MAX_CHARS_PER_PAGE}）を超えたので打ち切った"
            );
        }
        build_lines(self.chars)
    }
}

impl pdf_extract::OutputDev for TextCollector {
    fn begin_page(
        &mut self,
        _page_num: u32,
        _media_box: &pdf_extract::MediaBox,
        _art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    /// `trm` は `Tsm × Tm × CTM`（**フォントサイズを含まない**。pdf-extract は別引数で渡す）。
    /// その平行移動成分がグリフ原点 = ベースライン左端で、単位は PDF ユーザー空間。
    fn output_character(
        &mut self,
        trm: &pdf_extract::Transform,
        width: f64,
        spacing: f64,
        font_size: f64,
        text: &str,
    ) -> Result<(), pdf_extract::OutputError> {
        if self.chars.len() >= MAX_CHARS_PER_PAGE {
            self.overflowed = true;
            return Ok(());
        }
        // 制御文字は行の意味を壊すだけなので落とす（\u{0} を吐く PDF がある）
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if text.is_empty() {
            return Ok(());
        }
        // 行列の線形部から縦横の伸縮を取り出す。回転していても長さとして正しい値になる
        let x_scale = trm.m11.hypot(trm.m12);
        let y_scale = trm.m21.hypot(trm.m22);
        // 送り幅はテキスト空間で width * font_size + spacing。横方向の伸縮だけ掛ける
        let advance = (width * font_size + spacing) * x_scale;
        let size = font_size * y_scale;
        self.chars.push(RawChar {
            x: trm.m31 - self.origin.0,
            y: trm.m32 - self.origin.1,
            advance,
            size,
            text,
            line_break: std::mem::take(&mut self.pending_break),
        });
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    fn end_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    /// `Tm` / `Td` / `TD` / `T*` で呼ばれる = 位置指定が入った合図
    fn end_line(&mut self) -> Result<(), pdf_extract::OutputError> {
        self.pending_break = true;
        Ok(())
    }
}

/// 採取した文字を行へ組み直す。
///
/// ①`end_line()` で run に切り、②同じベースラインで横に続く run を繋ぎ直す、の 2 段。
/// ②が要るのは単語ごとに位置指定を置き直す生成器があるためで、それをやると今度は段組が
/// 1 行に化けるので、空きが [`LINE_JOIN_GAP_RATIO`] を超えたら繋がない。
fn build_lines(chars: Vec<RawChar>) -> Vec<PdfTextLine> {
    // ① 位置指定で run へ分割
    let mut runs: Vec<Vec<RawChar>> = Vec::new();
    for ch in chars {
        if ch.line_break || runs.is_empty() {
            runs.push(Vec::new());
        }
        if let Some(run) = runs.last_mut() {
            run.push(ch);
        }
    }
    runs.retain(|run| !run.is_empty());
    // TJ の負オフセットで前後することがあるので run 内も x 昇順に均す
    for run in &mut runs {
        run.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    // 上の行から、同じ行なら左から右へ
    runs.sort_by(|a, b| b[0].y.total_cmp(&a[0].y).then(a[0].x.total_cmp(&b[0].x)));

    // ② 同じベースラインで近い run を繋ぐ
    let mut lines: Vec<Vec<RawChar>> = Vec::new();
    for run in runs {
        match lines.last_mut() {
            Some(last) if can_join(last, &run) => last.extend(run),
            _ => lines.push(run),
        }
    }
    lines.into_iter().filter_map(finish_line).collect()
}

/// 直前の行の続きとして繋いでよいか
fn can_join(last: &[RawChar], run: &[RawChar]) -> bool {
    let (Some(tail), Some(head)) = (last.last(), run.first()) else {
        return false;
    };
    let size = tail.size.max(head.size).max(f64::EPSILON);
    if (tail.y - head.y).abs() > SAME_BASELINE_RATIO * size {
        return false;
    }
    // 少しの重なりは許す（カーニングや装飾で前の字へ食い込むことがある）
    let gap = head.x - (tail.x + tail.advance);
    (-0.5 * size..=LINE_JOIN_GAP_RATIO * size).contains(&gap)
}

/// 1 行分の文字から [`PdfTextLine`] を作る。
///
/// 文字矩形は**必ず 1 Unicode スカラーにつき 1 つ**にする（選択とヒットテストが
/// バイト位置で引くため）。合字が複数文字へ展開されたときは送り幅を等分する。
fn finish_line(mut chars: Vec<RawChar>) -> Option<PdfTextLine> {
    chars.sort_by(|a, b| a.x.total_cmp(&b.x));

    let mut text = String::new();
    let mut char_boxes: Vec<PdfCharBox> = Vec::new();
    let mut prev_right: Option<f64> = None;

    for ch in &chars {
        let height = ch.size.max(f64::EPSILON);
        let bottom = ch.y - DESCENT_RATIO * ch.size;
        let width = ch.advance.abs().max(ch.size * MIN_GLYPH_WIDTH_RATIO);

        // 字送りだけで表された単語区切りを空白として補う
        if let Some(right) = prev_right {
            let gap = ch.x - right;
            if gap > SPACE_GAP_RATIO * ch.size && !ch.text.starts_with(' ') {
                let start = text.len();
                text.push(' ');
                char_boxes.push(PdfCharBox {
                    byte_range: start..text.len(),
                    bbox: [right, bottom, gap, height],
                });
            }
        }

        // 合字などで 1 グリフが複数文字になることがあるので等分する
        let count = ch.text.chars().count().max(1);
        let each = width / count as f64;
        for (i, c) in ch.text.chars().enumerate() {
            let start = text.len();
            text.push(c);
            char_boxes.push(PdfCharBox {
                byte_range: start..text.len(),
                bbox: [ch.x + each * i as f64, bottom, each, height],
            });
        }
        prev_right = Some(ch.x + width);
    }

    if text.trim().is_empty() {
        return None;
    }
    // 行の外接矩形は文字矩形の和
    let left = char_boxes.iter().fold(f64::MAX, |a, b| a.min(b.bbox[0]));
    let right = char_boxes
        .iter()
        .fold(f64::MIN, |a, b| a.max(b.bbox[0] + b.bbox[2]));
    let bottom = char_boxes.iter().fold(f64::MAX, |a, b| a.min(b.bbox[1]));
    let top = char_boxes
        .iter()
        .fold(f64::MIN, |a, b| a.max(b.bbox[1] + b.bbox[3]));
    Some(PdfTextLine {
        text,
        bbox: [left, bottom, right - left, top - bottom],
        char_boxes,
    })
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
        let origin = page_box_origin(doc, page_id);
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
            let bbox = match extract_rect(doc, annot_dict, origin) {
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

/// `/Rect [x1, y1, x2, y2]` を `[x, y, width, height]`（PDF 座標・左下原点）に変換する。
/// 角の順序は仕様上どちらでもよいので min / max で正規化し、ページ矩形の左下を原点に揃える
fn extract_rect(
    doc: &LopdfDocument,
    dict: &lopdf::Dictionary,
    origin: (f64, f64),
) -> Option<[f64; 4]> {
    let rect_arr = resolve_array(doc, dict.get(b"Rect").ok()?)?;
    if rect_arr.len() < 4 {
        return None;
    }
    let x1 = obj_to_f64(&rect_arr[0])?;
    let y1 = obj_to_f64(&rect_arr[1])?;
    let x2 = obj_to_f64(&rect_arr[2])?;
    let y2 = obj_to_f64(&rect_arr[3])?;
    Some([
        x1.min(x2) - origin.0,
        y1.min(y2) - origin.1,
        (x2 - x1).abs(),
        (y2 - y1).abs(),
    ])
}

/// ページ矩形の左下。`/CropBox` があればそちらを優先する（レンダラも CropBox を描くので、
/// 注釈やテキストの座標を画像へ重ねるにはこの原点でそろえる必要がある）。
/// この 2 つは親の `/Pages` から継承できるので `/Parent` を辿る
fn page_box_origin(doc: &LopdfDocument, page_id: lopdf::ObjectId) -> (f64, f64) {
    for key in [b"CropBox".as_slice(), b"MediaBox".as_slice()] {
        if let Some(rect) = inherited_rect(doc, page_id, key) {
            return (rect[0].min(rect[2]), rect[1].min(rect[3]));
        }
    }
    (0.0, 0.0)
}

/// ページ属性を `/Parent` を辿りながら引く。壊れた PDF の循環参照で止まらないよう深さを切る
fn inherited_rect(doc: &LopdfDocument, page_id: lopdf::ObjectId, key: &[u8]) -> Option<[f64; 4]> {
    const MAX_DEPTH: usize = 32;
    let mut current = page_id;
    for _ in 0..MAX_DEPTH {
        let dict = doc.get_object(current).ok()?.as_dict().ok()?;
        if let Some(arr) = dict.get(key).ok().and_then(|v| resolve_array(doc, v)) {
            let values: Vec<f64> = arr.iter().take(4).filter_map(obj_to_f64).collect();
            if let [x1, y1, x2, y2] = values[..] {
                return Some([x1, y1, x2, y2]);
            }
        }
        current = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
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
    lookup_name_tree(doc, dests_dict, name, pages)
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
            if let Some(result) = lookup_name_tree(doc, kid_dict, name, pages) {
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

    let walk = OutlineWalk {
        doc,
        total_pages,
        pages: &pages,
        max_depth: MAX_DEPTH,
        max_items: MAX_ITEMS,
    };
    let mut items = Vec::new();
    collect_outline_items(&walk, first, 1, &mut items);
    PreviewOutline::new(items)
}

/// `/Outlines` の走査中ずっと変わらないもの。
/// 再帰の引数として持ち回ると、可変な「今どこか」（項目・深さ・結果）が埋もれる
struct OutlineWalk<'a> {
    doc: &'a LopdfDocument,
    total_pages: usize,
    pages: &'a std::collections::BTreeMap<u32, lopdf::ObjectId>,
    max_depth: u8,
    max_items: usize,
}

fn collect_outline_items(
    walk: &OutlineWalk<'_>,
    item_ref: &lopdf::Object,
    level: u8,
    items: &mut Vec<PreviewOutlineItem>,
) {
    let (doc, pages) = (walk.doc, walk.pages);
    if level > walk.max_depth || items.len() >= walk.max_items {
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
        if let Some(page) = resolve_outline_dest(doc, dict, pages) {
            if page <= walk.total_pages {
                items.push(PreviewOutlineItem {
                    title,
                    level,
                    target: PreviewOutlineTarget::PdfPage { page },
                });
            }
        }
    }

    if let Ok(first_child) = dict.get(b"First") {
        collect_outline_items(walk, first_child, level.saturating_add(1), items);
    }
    if let Ok(next) = dict.get(b"Next") {
        collect_outline_items(walk, next, level, items);
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

    /// 不在ファイルはパニックせず空で返る（描画側が正常系として扱えるように）
    #[test]
    fn テキストレイヤは不在ファイルで空を返す() {
        let path = Path::new("no-such.pdf");
        assert!(extract_text_layers(path, 3).unwrap().is_empty());
    }

    /// **文字矩形が実際の版面と合っているか**を、位置の分かっている PDF で数値検証する。
    /// ここがずれるとテキスト選択が 1 行ずれた場所を掴む
    #[test]
    fn テキストの文字矩形が版面の実座標と一致する() {
        let dir = std::env::temp_dir().join("tako_pdf_win_text_coord_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("text.pdf");
        // Helvetica 12pt を (72, 700) に置く。Helvetica の 'H' は 722/1000 em なので
        // 1 文字目の送りは 12 * 0.722 = 8.664pt になる
        std::fs::write(
            &path,
            pdf_with_content("BT /F1 12 Tf 72 700 Td (Hello) Tj ET"),
        )
        .unwrap();

        let layers = extract_text_layers(&path, 1).unwrap();
        assert_eq!(layers.len(), 1);
        let lines = &layers[0];
        assert_eq!(lines.len(), 1, "1 行に組まれる: {lines:?}");
        assert_eq!(lines[0].text, "Hello");

        let boxes = &lines[0].char_boxes;
        assert_eq!(boxes.len(), 5, "1 文字 1 矩形");
        // 1 文字目 'H' はベースライン (72, 700) から始まる
        assert!(
            (boxes[0].bbox[0] - 72.0).abs() < 0.5,
            "先頭の x が 72pt: got {}",
            boxes[0].bbox[0]
        );
        // 下端はベースラインからディセンダぶん下（12 * 0.2 = 2.4pt）
        assert!(
            (boxes[0].bbox[1] - (700.0 - 2.4)).abs() < 0.5,
            "下端がベースライン - ディセンダ: got {}",
            boxes[0].bbox[1]
        );
        assert!(
            (boxes[0].bbox[2] - 8.664).abs() < 0.5,
            "'H' の送り幅が AFM の 722/1000 em: got {}",
            boxes[0].bbox[2]
        );
        assert!(
            (boxes[0].bbox[3] - 12.0).abs() < 0.5,
            "高さがフォントサイズ: got {}",
            boxes[0].bbox[3]
        );
        // 2 文字目は 1 文字目の送りぶん右にある（等間隔ではなく字幅どおり）
        assert!(
            (boxes[1].bbox[0] - (72.0 + 8.664)).abs() < 0.5,
            "2 文字目が 'H' の幅ぶん右: got {}",
            boxes[1].bbox[0]
        );
        // 文字矩形は左から右へ単調に進む
        assert!(
            boxes.windows(2).all(|w| w[1].bbox[0] >= w[0].bbox[0]),
            "x が単調増加: {boxes:?}"
        );
        // バイト範囲が行テキストを隙間なく覆う
        let covered: String = boxes
            .iter()
            .map(|b| &lines[0].text[b.byte_range.clone()])
            .collect();
        assert_eq!(covered, lines[0].text, "バイト範囲が行全体を覆う");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 字送りだけで表された単語区切りが空白になり、離れた行は別の行になる
    #[test]
    fn 空白の補完と行の分離() {
        let dir = std::env::temp_dir().join("tako_pdf_win_text_lines_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lines.pdf");
        // 1 行目: 空白文字を置かず Td で離して 2 語を並べる（PDF でよくある表現）
        // 2 行目: TL + T* で 20pt 下げる
        std::fs::write(
            &path,
            pdf_with_content(
                "BT /F1 12 Tf 20 TL 72 700 Td (Alpha) Tj 60 0 Td (Beta) Tj \
                 -60 0 Td T* (Gamma) Tj ET",
            ),
        )
        .unwrap();

        let layers = extract_text_layers(&path, 1).unwrap();
        let lines = &layers[0];
        assert_eq!(lines.len(), 2, "2 行に分かれる: {lines:?}");
        assert_eq!(
            lines[0].text, "Alpha Beta",
            "同じベースラインの 2 語は空白で繋がる"
        );
        assert_eq!(lines[1].text, "Gamma");
        // 上の行が先に来る（PDF の y は上ほど大きい）
        assert!(lines[0].bbox[1] > lines[1].bbox[1], "行が上から順に並ぶ");
        // 補完した空白にも矩形があり、選択が途切れない
        assert_eq!(
            lines[0].char_boxes.len(),
            lines[0].text.chars().count(),
            "1 文字 1 矩形（補完した空白を含む）"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// テキストの無い PDF（図形だけ）では行がゼロになる
    #[test]
    fn テキストのないpdfは行を返さない() {
        let dir = std::env::temp_dir().join("tako_pdf_win_text_none_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shape.pdf");
        std::fs::write(&path, pdf_with_content("0 0 1 rg 10 10 100 100 re f")).unwrap();

        let layers = extract_text_layers(&path, 1).unwrap();
        assert_eq!(layers.len(), 1);
        assert!(layers[0].is_empty(), "テキスト行はゼロ: {:?}", layers[0]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 612x792 の 1 ページ PDF を content stream から組む（Helvetica を /F1 に持つ）
    fn pdf_with_content(content: &str) -> Vec<u8> {
        let mut pdf = Vec::with_capacity(1024);
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(5);

        offsets.push(pdf.len());
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
              /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content.as_bytes());
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );

        let xref_at = pdf.len();
        let size = offsets.len() + 1;
        pdf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in &offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
                .as_bytes(),
        );
        pdf
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
