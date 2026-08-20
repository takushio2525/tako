//! preview — プレビューペイン（コード / Markdown / 画像 / PDF）の読み込みと整形
//!
//! GPUI 非依存（描画は main.rs 側）。シンタックスハイライトは syntect だが、
//! 将来 tree-sitter へ差し替えられるよう [`Highlighter`] trait で抽象化する
//! （`architecture.md`「コンセプト②の実現」。ユーザー指示）。
//! Markdown は pulldown-cmark でイベントストリームをブロック列へ写す。
//! 画像は生バイトを保持し GPUI 側でデコードする（FR-3.10）。
//! PDF は macOS Core Graphics でページを RGBA にレンダリングする（FR-3.4）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use tako_control::protocol::PreviewModeWire;
use tako_core::{
    PdfLinks, PreviewOutline, PreviewOutlineItem, PreviewOutlineTarget, SearchHit, TextBuffer,
};

use crate::platform;

/// 読み込みの上限（巨大ファイルで UI を固めない。超過分は切り詰めて明示する）
pub(crate) const MAX_BYTES: usize = 1_000_000;
const MAX_LINES: usize = 5_000;

/// プレビューの表示モード（ワイヤ表現 `PreviewModeWire` と 1:1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    Code,
    Markdown,
    Image,
    Pdf,
    Video,
}

impl PreviewMode {
    pub fn to_wire(self) -> PreviewModeWire {
        match self {
            PreviewMode::Code => PreviewModeWire::Code,
            PreviewMode::Markdown => PreviewModeWire::Markdown,
            PreviewMode::Image => PreviewModeWire::Image,
            PreviewMode::Pdf => PreviewModeWire::Pdf,
            PreviewMode::Video => PreviewModeWire::Video,
        }
    }

    pub fn from_wire(wire: PreviewModeWire) -> Self {
        match wire {
            PreviewModeWire::Code => PreviewMode::Code,
            PreviewModeWire::Markdown => PreviewMode::Markdown,
            PreviewModeWire::Image => PreviewMode::Image,
            PreviewModeWire::Pdf => PreviewMode::Pdf,
            PreviewModeWire::Video => PreviewMode::Video,
        }
    }
}

/// ハイライト済みテキストの 1 区間。色はハイライタのテーマ由来（theme 非依存の生 RGB）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Option<tako_core::Rgb>,
    pub bold: bool,
    pub italic: bool,
}

/// ハイライト済みの 1 行
pub type Line = Vec<Span>;

/// Markdown のインライン 1 区間（強調・インラインコード等のスタイルフラグ付き）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MdSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// リンクなら md に書かれた遷移先（`None` = リンクではない）。
    /// 装飾（accent + 下線）は `is_some()` で判定し、⌘+クリックで開くかは
    /// `tako_core::md_links::browser_url` の判定に従う（#680）
    pub link_url: Option<String>,
}

impl MdSpan {
    /// リンクの一部か（装飾判定用）
    pub fn is_link(&self) -> bool {
        self.link_url.is_some()
    }
}

/// 表セルの配置（GFM の `:---` / `:---:` / `---:`。FR-3.3）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MdAlign {
    /// 指定なし（`---`）。左寄せで描く
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// 表の 1 セル（インライン装飾を保持する）
pub type MdCell = Vec<MdSpan>;

/// Markdown のブロック（描画単位。FR-3.3）。
///
/// 引用・リストの入れ子は「ブロックを再帰させる」のではなく、各ブロックが自分の
/// 引用深さ・リスト段を持つフラット構造で表す。1 ブロック = 1 描画要素を保てるので
/// 目次ジャンプ（Issue #232）のブロック番号がそのまま子要素の添字になる。
#[derive(Debug, Clone, PartialEq)]
pub struct MdBlock {
    pub kind: MdBlockKind,
    /// 引用のネスト深さ（0 = 引用外）。引用内のリスト・コードブロックも同じ帯に入る
    pub quote_depth: usize,
    /// リストのネスト段（1 = 最上位のリスト項目、0 = リスト外）。
    /// リスト項目に属する段落・コードブロックの字下げにも使う
    pub list_depth: usize,
}

impl MdBlock {
    fn new(kind: MdBlockKind) -> Self {
        Self {
            kind,
            quote_depth: 0,
            list_depth: 0,
        }
    }

    fn nested(kind: MdBlockKind, quote_depth: usize, list_depth: usize) -> Self {
        Self {
            kind,
            quote_depth,
            list_depth,
        }
    }
}

/// ブロックの種別
#[derive(Debug, Clone, PartialEq)]
pub enum MdBlockKind {
    Heading {
        level: u8,
        spans: Vec<MdSpan>,
    },
    Paragraph {
        spans: Vec<MdSpan>,
    },
    /// リスト項目
    ListItem {
        /// 番号付きなら表示番号、箇条書きなら None
        ordered: Option<u64>,
        /// タスクリスト（`- [ ]` / `- [x]`）なら完了フラグ
        task: Option<bool>,
        /// 同じ項目の 2 つ目以降のブロック（マーカーを描かず字下げだけ揃える）
        continuation: bool,
        spans: Vec<MdSpan>,
    },
    /// コードブロック（```lang はハイライトして保持する）
    CodeBlock {
        /// フェンスの info 文字列（言語指定なし / インデントコードは None）
        lang: Option<String>,
        lines: Vec<Line>,
    },
    /// GFM テーブル（Issue #656）
    Table {
        /// 列ごとの配置。列数の正はこの長さ
        align: Vec<MdAlign>,
        header: Vec<MdCell>,
        rows: Vec<Vec<MdCell>>,
    },
    Rule,
}

/// 画像データ（生バイトを保持。GPUI 側で Image::from_bytes してデコードする）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub format: ImageFileFormat,
    /// ヘッダだけから取得したデコード後の pixel size。SVG など取得不能時は None。
    pub pixel_size: Option<(u32, u32)>,
}

/// 対応画像フォーマット（GPUI の ImageFormat と 1:1 だが GPUI 非依存にする）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFileFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Svg,
}

/// PDF テキスト行 1 本分（ページ内で改行区切り）
#[derive(Debug, Clone, PartialEq)]
pub struct PdfCharBox {
    pub byte_range: std::ops::Range<usize>,
    /// PDF 座標系での文字バウンディングボックス [x, y, width, height]
    pub bbox: [f64; 4],
}

/// PDF テキスト行 1 本分（ページ内で改行区切り）
#[derive(Debug, Clone, PartialEq)]
pub struct PdfTextLine {
    pub text: String,
    /// PDF 座標系での行バウンディングボックス [x, y, width, height]
    /// （PDF 座標は左下原点。描画時にスクリーン座標に変換する）
    pub bbox: [f64; 4],
    /// 文字単位の矩形。ヒットテストと選択ハイライトはこれを使う
    pub char_boxes: Vec<PdfCharBox>,
}

/// PDF データ（全ページの圧縮 PNG を保持し、表示近傍だけデコードして閲覧）
#[derive(Debug, Clone, PartialEq)]
pub struct PdfData {
    /// 各ページの PNG バイト列（Core Graphics でレンダリング済み）
    pub pages: Vec<Vec<u8>>,
    pub total_pages: usize,
    /// ページごとのテキスト行（テキスト選択用。テキストレイヤがない PDF では空）
    pub text_layers: Vec<Vec<PdfTextLine>>,
    /// ページごとの PDF 座標系でのサイズ [width, height]
    pub page_sizes: Vec<[f64; 2]>,
    /// 現在の PNG を生成した表示条件。ウィンドウ scale・ズーム・幅を量子化して
    /// background 再ラスタライズと PreviewImageCache の世代判定に使う。
    pub raster_key: PdfRasterKey,
    /// ページごとの実ラスタライズ解像度 [pixel width, pixel height]。
    /// 品質検証とキャッシュ整合性の確認に使う。
    pub pixel_sizes: Vec<[u32; 2]>,
    /// PDF アノテーションから抽出したリンク一覧（#271。ロード時に 1 回構築）。
    pub links: Arc<PdfLinks>,
}

/// PDF 再ラスタライズのキャッシュキー（#231 / #234）。
///
/// 連続リサイズやピンチでキーが無制限に増えないよう、表示幅は 64 logical px、
/// device scale と zoom は 1% 単位へ量子化する。対象ピクセル幅は安全上 4096 px を上限とする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PdfRasterKey {
    pub device_scale_percent: u16,
    pub zoom_percent: u16,
    pub logical_width_bucket: u32,
}

impl PdfRasterKey {
    const WIDTH_BUCKET: u32 = 64;
    const MIN_PIXEL_WIDTH: u32 = 256;
    const MAX_PIXEL_WIDTH: u32 = 4096;

    pub fn for_view(device_scale: f32, zoom: f32, logical_width: f32) -> Self {
        let device_scale_percent = (device_scale.clamp(1.0, 4.0) * 100.0).round() as u16;
        let zoom_percent = (zoom.clamp(0.25, 4.0) * 100.0).round() as u16;
        let width = logical_width.max(1.0).ceil() as u32;
        let logical_width_bucket = width.div_ceil(Self::WIDTH_BUCKET) * Self::WIDTH_BUCKET;
        Self {
            device_scale_percent,
            zoom_percent,
            logical_width_bucket,
        }
    }

    pub fn target_pixel_width(self) -> u32 {
        let width = self.logical_width_bucket as f64
            * f64::from(self.device_scale_percent)
            * f64::from(self.zoom_percent)
            / 10_000.0;
        (width.ceil() as u32).clamp(Self::MIN_PIXEL_WIDTH, Self::MAX_PIXEL_WIDTH)
    }
}

/// background ラスタライズの戻り値。テキストレイヤは scale 非依存なので含めず再利用する。
pub struct PdfRasterizedPages {
    pub pages: Vec<Vec<u8>>,
    pub total_pages: usize,
    pub page_sizes: Vec<[f64; 2]>,
    pub pixel_sizes: Vec<[u32; 2]>,
}

/// 動画のメタ情報 + サムネイル（ffmpeg で抽出）
#[derive(Debug, Clone, PartialEq)]
pub struct VideoData {
    /// サムネイル画像（PNG バイト列。ffmpeg 未インストール時は空）
    pub thumbnail: Vec<u8>,
    /// 動画の長さ（秒。取得できなければ None）
    pub duration: Option<f64>,
    /// 解像度（幅 x 高さ。取得できなければ None）
    pub resolution: Option<(u32, u32)>,
    /// コーデック名（"h264" 等。取得できなければ None）
    pub codec: Option<String>,
    /// ファイルサイズ（バイト）
    pub file_size: u64,
}

/// チェンジログビューの 1 コミットエントリ（Issue #338）
#[derive(Debug, Clone, PartialEq)]
pub struct ChangelogEntry {
    pub commit: tako_core::GitCommit,
    /// diff 展開中なら Some（hunks）。折りたたみ中なら None
    pub expanded_diff: Option<Vec<tako_core::DiffHunk>>,
}

/// チェンジログビュー全体のデータ（Issue #338）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChangelogData {
    pub entries: Vec<ChangelogEntry>,
    /// git リポジトリのルートパス（diff 取得時に使う）
    pub repo_root: Option<std::path::PathBuf>,
    /// リポジトリ内の相対パス（diff 取得時に使う）
    pub rel_path: Option<String>,
}

/// 読み込み済みのプレビュー内容
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Code(Vec<Line>),
    Markdown(Vec<MdBlock>),
    Image(ImageData),
    Pdf(PdfData),
    Video(VideoData),
    /// background で読み込み中（Issue #168: PDF ラスタライズ / ffmpeg サムネ抽出は
    /// UI スレッドで行わない。完了時に本内容へ差し替わる）
    Loading,
    /// 読めない・バイナリ等（正常系の劣化。ペインは開いたまま理由を表示する）
    Error(String),
}

/// background ライブリロードの完成結果。テキストの元バイト列は、編集中の
/// 自己保存イベントと真の外部変更を区別するためだけに完了時まで保持する。
pub struct ReloadedPreview {
    pub state: PreviewState,
    pub source_bytes: Option<Vec<u8>>,
}

/// ファイルの mtime + size ペア。ライブリロード時に内容変更の有無を判定する（#257）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl FileStamp {
    pub fn from_path(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            size: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// プレビューペイン 1 枚分の状態（`TakoApp::previews` の値）
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewState {
    pub path: PathBuf,
    pub mode: PreviewMode,
    pub content: PreviewContent,
    /// プレビューロード時に background で一度だけ構築した目次。
    /// render はこの完成済みデータを参照するだけで再計算しない。
    pub outline: Arc<PreviewOutline>,
    /// 上限超過で切り詰めたか（フッタで明示する）
    pub truncated: bool,
    /// ロード時のファイルスタンプ（ライブリロードの変更判定用。#257）
    pub file_stamp: Option<FileStamp>,
}

/// コードプレビューの軽量編集セッション（FR-3.5）。表示状態とは分離し、編集モードを
/// OFF にしても未保存バッファを保持する。別ファイルで差し替える前に dirty を検査できる。
#[derive(Debug, Clone)]
pub struct EditState {
    pub buffer: TextBuffer,
    pub editing: bool,
    pub message: Option<String>,
    /// 自動保存の有効状態（既定 true。config.yaml の editor.autosave で変更可能）
    pub autosave: bool,
    /// 自動保存後の表示メッセージ（タイトルバーに「保存済み」等を表示する）
    pub save_status: Option<SaveStatus>,
    /// 検索バーの表示状態
    pub search_visible: bool,
    /// 検索バー内のフォーカス先（検索フィールド or 置換フィールド）
    pub search_focus: SearchFieldFocus,
    /// 検索クエリ
    pub search_query: String,
    /// 検索フィールドのカーソル位置（バイトオフセット）
    pub search_cursor: usize,
    /// 検索ヒット一覧（検索クエリ変更時に更新）
    pub search_hits: Vec<SearchHit>,
    /// 現在フォーカス中のヒットインデックス
    pub search_index: usize,
    /// 置換テキスト
    pub replace_text: String,
    /// 置換フィールドのカーソル位置（バイトオフセット）
    pub replace_cursor: usize,
}

/// 検索バー内のフォーカス先
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFieldFocus {
    Query,
    Replace,
}

/// 自動保存の表示状態
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveStatus {
    Saved,
    Conflict,
    Error(String),
}

impl EditState {
    pub fn open(preview: &PreviewState) -> Result<Self, String> {
        if preview.truncated {
            return Err("末尾を省略した大きいファイルは安全のため編集できない".into());
        }
        if !matches!(preview.mode, PreviewMode::Code | PreviewMode::Markdown) {
            return Err("テキスト以外のプレビューは編集できない".into());
        }
        let buffer = TextBuffer::open(&preview.path).map_err(|e| e.to_string())?;
        if buffer.text().contains('\0') {
            return Err("バイナリファイルは編集できない".into());
        }
        Ok(Self {
            buffer,
            editing: true,
            message: None,
            autosave: true,
            save_status: None,
            search_visible: false,
            search_focus: SearchFieldFocus::Query,
            search_query: String::new(),
            search_cursor: 0,
            search_hits: Vec::new(),
            search_index: 0,
            replace_text: String::new(),
            replace_cursor: 0,
        })
    }

    pub fn dirty(&self) -> bool {
        self.buffer.dirty()
    }
}

/// 編集中も既存の syntect ハイライト基盤を再利用して、読み取り時と同じ色分けで
/// 表示する。`apply_editor_text` は UI スレッドから呼ばれるので、ファイルが巨大な
/// 場合は上限で切り詰められたテキストを対象にする。
pub fn apply_editor_text(preview: &mut PreviewState, edit: &EditState) {
    preview.mode = PreviewMode::Code;
    preview.content =
        PreviewContent::Code(highlighter().highlight(&preview.path, edit.buffer.text()));
    preview.outline = Arc::new(PreviewOutline::default());
    preview.truncated = false;
}

impl PreviewState {
    /// 再ハイライトの可能性がある種別か（#815 の構文セット寿命判定）。
    /// Markdown もコードブロックのハイライトで構文セットを使う
    pub fn needs_syntax(&self) -> bool {
        matches!(self.mode, PreviewMode::Code | PreviewMode::Markdown)
    }

    /// Markdown レンダリングへ切り替え可能なファイルか（目アイコントグルの表示判定）
    pub fn markdown_capable(&self) -> bool {
        is_markdown_path(&self.path)
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn error(path: &Path, mode: PreviewMode, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            mode,
            content: PreviewContent::Error(message.into()),
            outline: Arc::new(PreviewOutline::default()),
            truncated: false,
            file_stamp: None,
        }
    }

    /// background 読み込み中のプレースホルダ（Issue #168）
    pub fn loading(path: &Path, mode: PreviewMode) -> Self {
        Self {
            path: path.to_path_buf(),
            mode,
            content: PreviewContent::Loading,
            outline: Arc::new(PreviewOutline::default()),
            truncated: false,
            file_stamp: None,
        }
    }
}

pub fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
    )
}

/// 画像ファイルの拡張子判定 → フォーマット
pub fn image_format_from_path(path: &Path) -> Option<ImageFileFormat> {
    let ext = path.extension()?.to_str()?;
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some(ImageFileFormat::Png),
        "jpg" | "jpeg" => Some(ImageFileFormat::Jpeg),
        "gif" => Some(ImageFileFormat::Gif),
        "webp" => Some(ImageFileFormat::WebP),
        "svg" => Some(ImageFileFormat::Svg),
        _ => None,
    }
}

const MAX_IMAGE_BYTES: usize = 50_000_000; // 50 MB

/// 画像ファイルを読み込む（生バイト。デコードは GPUI 側）
pub fn load_image(path: &Path) -> PreviewState {
    let format = match image_format_from_path(path) {
        Some(f) => f,
        None => {
            return PreviewState::error(
                path,
                PreviewMode::Image,
                crate::ui_text::preview::unsupported_image(),
            )
        }
    };
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() > MAX_IMAGE_BYTES => PreviewState::error(
            path,
            PreviewMode::Image,
            crate::ui_text::preview::image_too_large(bytes.len() as f64 / 1_000_000.0),
        ),
        Ok(bytes) => {
            let pixel_size = image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()
                .ok()
                .and_then(|reader| reader.into_dimensions().ok());
            PreviewState {
                path: path.to_path_buf(),
                mode: PreviewMode::Image,
                content: PreviewContent::Image(ImageData {
                    bytes,
                    format,
                    pixel_size,
                }),
                outline: Arc::new(PreviewOutline::default()),
                truncated: false,
                file_stamp: FileStamp::from_path(path),
            }
        }
        Err(e) => PreviewState::error(
            path,
            PreviewMode::Image,
            crate::ui_text::preview::cannot_read(&e.to_string()),
        ),
    }
}

/// PDF の全ページを圧縮 PNG へレンダリングして PreviewState を返す。
/// 描画は抽象境界 B12（[`crate::platform::pdf`]）が OS ごとに担う
pub fn load_pdf(path: &Path, _page: usize) -> PreviewState {
    load_pdf_with_key(path, PdfRasterKey::for_view(2.0, 1.0, 612.0))
}

/// 指定した表示条件で PDF を読み込む。全処理は呼び出し側が background へ載せる。
///
/// テキストレイヤ・目次・リンクは取れないプラットフォームがある（#521 / #693）。
/// そこは空で返るのが正常系で、テキストレイヤの無い PDF（macOS でも普通にある）と
/// 同じ経路を通る
pub fn load_pdf_with_key(path: &Path, raster_key: PdfRasterKey) -> PreviewState {
    match rasterize_pdf(path, raster_key) {
        Ok(rasterized) => {
            let text_layers = platform::pdf::extract_text_layers(path, rasterized.total_pages)
                .unwrap_or_default();
            let outline =
                platform::pdf::extract_outline(path, rasterized.total_pages).unwrap_or_default();
            let links =
                platform::pdf::extract_links(path, rasterized.total_pages).unwrap_or_default();
            PreviewState {
                path: path.to_path_buf(),
                mode: PreviewMode::Pdf,
                content: PreviewContent::Pdf(PdfData {
                    pages: rasterized.pages,
                    total_pages: rasterized.total_pages,
                    text_layers,
                    page_sizes: rasterized.page_sizes,
                    raster_key,
                    pixel_sizes: rasterized.pixel_sizes,
                    links: Arc::new(links),
                }),
                outline: Arc::new(outline),
                truncated: false,
                file_stamp: FileStamp::from_path(path),
            }
        }
        Err(e) => PreviewState::error(path, PreviewMode::Pdf, e),
    }
}

/// PDF ページ画像だけを再生成する。テキスト抽出は初回ロード時だけ行う。
pub fn rasterize_pdf(path: &Path, raster_key: PdfRasterKey) -> Result<PdfRasterizedPages, String> {
    if !platform::pdf::capabilities().rasterize {
        return Err(crate::ui_text::preview::pdf_unsupported_platform().to_string());
    }
    let _span = tako_control::diag::perf_span("pdf_rasterize");
    platform::pdf::render_all_pages(path, raster_key)
}

/// 動画ファイルのプレビュー読み込み（ffmpeg でサムネイル抽出 + メタ情報取得）
pub fn load_video(path: &Path) -> PreviewState {
    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            return PreviewState::error(
                path,
                PreviewMode::Video,
                crate::ui_text::preview::cannot_read(&e.to_string()),
            );
        }
    };

    // ffprobe でメタ情報を取得
    let (duration, resolution, codec) = video_probe(path);

    // ffmpeg でサムネイル抽出（10秒時点 or 先頭フレーム）
    let thumbnail = video_thumbnail(path, duration);

    PreviewState {
        path: path.to_path_buf(),
        mode: PreviewMode::Video,
        content: PreviewContent::Video(VideoData {
            thumbnail,
            duration,
            resolution,
            codec,
            file_size,
        }),
        outline: Arc::new(PreviewOutline::default()),
        truncated: false,
        file_stamp: FileStamp::from_path(path),
    }
}

/// ffmpeg バイナリの場所（プロセス内で 1 回だけ解決してキャッシュする）。
/// .app バンドルから起動すると PATH が最小構成で Homebrew の ffmpeg が
/// 見えない（tmux_bin() と同じ問題）。同じフォールバック戦略で解決する
fn ffmpeg_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        resolve_media_bin(
            "ffmpeg",
            "FFMPEG_PATH",
            &[
                "/opt/homebrew/bin/ffmpeg",
                "/usr/local/bin/ffmpeg",
                "/opt/local/bin/ffmpeg",
            ],
        )
    })
}

/// ffprobe バイナリの場所（ffmpeg_bin() と同じ戦略）
fn ffprobe_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        resolve_media_bin(
            "ffprobe",
            "FFPROBE_PATH",
            &[
                "/opt/homebrew/bin/ffprobe",
                "/usr/local/bin/ffprobe",
                "/opt/local/bin/ffprobe",
            ],
        )
    })
}

/// 外部バイナリの解決（tmux_bin() と同じフォールバック: env → PATH → 既知パス → ログインシェル）
fn resolve_media_bin(name: &str, env_var: &str, known_paths: &[&str]) -> String {
    if let Some(bin) = std::env::var_os(env_var) {
        if !bin.is_empty() {
            return bin.to_string_lossy().into_owned();
        }
    }
    // #586: GUI プロセスから走るのでコンソールウィンドウを出させない（以下 3 箇所）
    if tako_core::platform::process::no_console_window(&mut std::process::Command::new(name))
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return name.into();
    }
    for candidate in known_paths {
        if std::path::Path::new(candidate).is_file() {
            return (*candidate).into();
        }
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        if let Ok(output) = std::process::Command::new(&shell)
            .args(["-l", "-c", &format!("command -v {name}")])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && std::path::Path::new(&path).is_file() {
                    return path;
                }
            }
        }
    }
    name.into()
}

/// ffprobe で動画のメタ情報を取得する。ffprobe が無ければすべて None
fn video_probe(path: &Path) -> (Option<f64>, Option<(u32, u32)>, Option<String>) {
    let output = tako_core::platform::process::no_console_window(&mut std::process::Command::new(
        ffprobe_bin(),
    ))
    .args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ])
    .arg(path)
    .output();
    let output = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return (None, None, None),
    };
    let json: serde_json::Value = match serde_json::from_slice(&output) {
        Ok(v) => v,
        Err(_) => return (None, None, None),
    };

    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());

    let video_stream = json["streams"].as_array().and_then(|streams| {
        streams
            .iter()
            .find(|s| s["codec_type"].as_str() == Some("video"))
    });

    let resolution = video_stream.and_then(|s| {
        let w = s["width"].as_u64()? as u32;
        let h = s["height"].as_u64()? as u32;
        Some((w, h))
    });

    let codec = video_stream
        .and_then(|s| s["codec_name"].as_str())
        .map(|s| s.to_string());

    (duration, resolution, codec)
}

/// ffmpeg でサムネイルを抽出する。seek 位置は 10 秒 or 動画の 10% or 先頭
fn video_thumbnail(path: &Path, duration: Option<f64>) -> Vec<u8> {
    let seek = match duration {
        Some(d) if d > 10.0 => "10".to_string(),
        Some(d) if d > 1.0 => format!("{:.1}", d * 0.1),
        _ => "0".to_string(),
    };
    let output = tako_core::platform::process::no_console_window(&mut std::process::Command::new(
        ffmpeg_bin(),
    ))
    .args(["-ss", &seek, "-i"])
    .arg(path)
    .args([
        "-frames:v",
        "1",
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "-vf",
        "scale='min(800,iw)':'min(600,ih)':force_original_aspect_ratio=decrease",
        "-",
    ])
    .output();
    match output {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => o.stdout,
        _ => Vec::new(),
    }
}

/// ファイルを読み込んでプレビュー状態を作る（テスト用。本番は load_fast + background highlight）
#[cfg(test)]
pub fn load(path: &Path, mode: PreviewMode) -> PreviewState {
    match mode {
        PreviewMode::Image => return load_image(path),
        PreviewMode::Pdf => return load_pdf(path, 0),
        PreviewMode::Video => return load_video(path),
        _ => {}
    }
    let (text, truncated) = match read_text(path) {
        Ok(pair) => pair,
        Err(message) => return PreviewState::error(path, mode, message),
    };
    let (content, outline) = match mode {
        PreviewMode::Markdown => {
            let (blocks, outline) = markdown_document(&text);
            (PreviewContent::Markdown(blocks), outline)
        }
        PreviewMode::Code => (
            PreviewContent::Code(highlighter().highlight(path, &text)),
            PreviewOutline::default(),
        ),
        PreviewMode::Image | PreviewMode::Pdf | PreviewMode::Video => unreachable!(),
    };
    PreviewState {
        path: path.to_path_buf(),
        mode,
        content,
        outline: Arc::new(outline),
        truncated,
        file_stamp: FileStamp::from_path(path),
    }
}

/// 高速ロード（UI スレッド用）: ファイルを読むが syntect ハイライトはスキップする。
/// Code モードは平文（色なし）を返し、呼び出し側が background で [`highlight_text`] を
/// 走らせて差し替える。Markdown の初回ロードは Issue #232 以降、本文と目次を同時に
/// background で完成させるため、本番の set_preview からはこの関数を呼ばない。
/// Image / Pdf モードは専用ローダーに委譲する。
/// 戻り値の `Option<String>` は Code モードの生テキスト（background ハイライト用）
pub fn load_fast(path: &Path, mode: PreviewMode) -> (PreviewState, Option<String>) {
    match mode {
        PreviewMode::Image => return (load_image(path), None),
        PreviewMode::Pdf => return (load_pdf(path, 0), None),
        PreviewMode::Video => return (load_video(path), None),
        _ => {}
    }
    let (text, truncated) = match read_text(path) {
        Ok(pair) => pair,
        Err(message) => {
            return (PreviewState::error(path, mode, message), None);
        }
    };
    let (content, outline, raw) = match mode {
        PreviewMode::Markdown => {
            let (blocks, outline) = markdown_document(&text);
            (PreviewContent::Markdown(blocks), outline, None)
        }
        PreviewMode::Code => {
            let lines = text.lines().map(|l| vec![plain_span(l)]).collect();
            (
                PreviewContent::Code(lines),
                PreviewOutline::default(),
                Some(text),
            )
        }
        PreviewMode::Image | PreviewMode::Pdf | PreviewMode::Video => unreachable!(),
    };
    (
        PreviewState {
            path: path.to_path_buf(),
            mode,
            content,
            outline: Arc::new(outline),
            truncated,
            file_stamp: FileStamp::from_path(path),
        },
        raw,
    )
}

/// ライブリロード用の完成版を作る。ファイル I/O・Markdown パース・syntect・
/// 画像読み込み・PDF ラスタライズのすべてを呼び出し側の background executor で行う。
pub fn load_for_reload(
    path: &Path,
    mode: PreviewMode,
    pdf_raster_key: Option<PdfRasterKey>,
) -> ReloadedPreview {
    let (state, source_bytes) = match mode {
        PreviewMode::Image => (load_image(path), None),
        PreviewMode::Pdf => (
            load_pdf_with_key(
                path,
                pdf_raster_key.unwrap_or_else(|| PdfRasterKey::for_view(2.0, 1.0, 612.0)),
            ),
            None,
        ),
        // 動画はライブリロード対象外だが、呼び出し誤りでも安全に完成状態を返す。
        PreviewMode::Video => (load_video(path), None),
        PreviewMode::Code | PreviewMode::Markdown => {
            let (text, truncated, source) = match read_text_source(path) {
                Ok(loaded) => loaded,
                Err(message) => {
                    return ReloadedPreview {
                        state: PreviewState::error(path, mode, message),
                        source_bytes: None,
                    };
                }
            };
            let (content, outline) = match mode {
                PreviewMode::Code => (
                    PreviewContent::Code(highlighter().highlight(path, &text)),
                    PreviewOutline::default(),
                ),
                PreviewMode::Markdown => {
                    let (blocks, outline) = markdown_document(&text);
                    (PreviewContent::Markdown(blocks), outline)
                }
                _ => unreachable!(),
            };
            (
                PreviewState {
                    path: path.to_path_buf(),
                    mode,
                    content,
                    outline: Arc::new(outline),
                    truncated,
                    file_stamp: FileStamp::from_path(path),
                },
                (!truncated).then_some(source),
            )
        }
    };
    ReloadedPreview {
        state,
        source_bytes,
    }
}

pub fn live_reload_supported(mode: PreviewMode) -> bool {
    matches!(
        mode,
        PreviewMode::Code | PreviewMode::Markdown | PreviewMode::Image | PreviewMode::Pdf
    )
}

/// background executor 上で呼ぶ: syntect ハイライトだけを実行して行列を返す
pub fn highlight_text(path: &Path, text: &str) -> Vec<Line> {
    highlighter().highlight(path, text)
}

/// テキストとして読む。バイナリ（NUL 含有）は明示エラー、上限超過は切り詰める
fn read_text(path: &Path) -> Result<(String, bool), String> {
    read_text_source(path).map(|(text, truncated, _)| (text, truncated))
}

/// 上限 + 1 byte だけ読み、巨大ファイルを丸ごとメモリへ載せずに省略判定する。
fn read_text_source(path: &Path) -> Result<(String, bool, Vec<u8>), String> {
    let file = std::fs::File::open(path)
        .map_err(|e| crate::ui_text::preview::cannot_read(&e.to_string()))?;
    let mut bytes = Vec::with_capacity(MAX_BYTES.min(64 * 1024) + 1);
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| crate::ui_text::preview::cannot_read(&e.to_string()))?;
    let truncated_bytes = bytes.len() > MAX_BYTES;
    bytes.truncate(MAX_BYTES);
    if bytes.contains(&0) {
        return Err(crate::ui_text::preview::binary_file().into());
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    let mut truncated = truncated_bytes;
    if text.lines().count() > MAX_LINES {
        text = text.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
        truncated = true;
    }
    Ok((text, truncated, bytes))
}

/// シンタックスハイライタの抽象（差し替え点。現実装は syntect、将来 tree-sitter）
pub trait Highlighter: Send + Sync {
    /// パス（拡張子・1 行目）から構文を推定して全行をハイライトする
    fn highlight(&self, path: &Path, text: &str) -> Vec<Line>;
    /// 言語トークン（``` の info 文字列）からのハイライト（Markdown のコードブロック用）
    fn highlight_lang(&self, lang: &str, text: &str) -> Vec<Line>;
}

/// 構文セットを載せたままにしておく猶予（最後に使ってからの長さ。#815）。
/// 編集の連続打鍵とライブリロードの連投で毎回ロードし直さないための幅で、これを過ぎたら
/// プレビューを開いたままでも手放す（描画済みの色は [`PreviewContent`] 側が持っている）。
pub const SYNTAX_IDLE_GRACE: Duration = Duration::from_secs(30);

/// 構文セットの寿命（#815）。
///
/// 実費は `SyntaxSet` の器（実測 1.04 MB / ロード 0.6〜1.2 ms）ではなく、**ハイライトした
/// 言語ごとに遅延コンパイルされる正規表現**にある（実測: Rust +5.1 MB・bash +10.9 MB・
/// Markdown +10.9 MB・TypeScript +32.0 MB。18 言語を通すと 149 MB まで積み上がる）。
/// これはセットの内側に溜まり、syntect には言語単位で捨てる API が無い。
/// つまり**捨てられる単位はセット全体だけ**なので、「借用が 1 枚も無く、最後の使用から
/// [`SYNTAX_IDLE_GRACE`] 経過したら丸ごと解放する」寿命管理にしている。
/// 実体は下のグローバル 1 個だが、テストが並列でも決定的になるようローカルに作れる形にしてある
struct SyntaxCache {
    /// 生存追跡。借用チケットが 1 枚でも生きていれば upgrade できる
    weak: Weak<SyntectHighlighter>,
    /// 猶予中の保持。これを手放しても、実際の解放は最後のチケットが落ちた時
    keep: Option<Arc<SyntectHighlighter>>,
    last_use: Option<Instant>,
}

impl SyntaxCache {
    const fn new() -> Self {
        Self {
            weak: Weak::new(),
            keep: None,
            last_use: None,
        }
    }

    /// 借りる。載っていなければここでロードする（実測 0.6〜1.2 ms）
    fn acquire(&mut self, now: Instant) -> SyntaxLease {
        self.last_use = Some(now);
        if let Some(arc) = self.weak.upgrade() {
            self.keep = Some(Arc::clone(&arc));
            return SyntaxLease(arc);
        }
        let arc = Arc::new(SyntectHighlighter::new());
        self.weak = Arc::downgrade(&arc);
        self.keep = Some(Arc::clone(&arc));
        SyntaxLease(arc)
    }

    /// 猶予を過ぎていれば保持を手放す。戻り値 = このターンで手放したか。
    /// 借用が残っていれば実解放はそのチケットが落ちた時まで自動的に待つ
    fn release_idle(&mut self, now: Instant, text_preview_open: bool) -> bool {
        if self.keep.is_none() {
            return false;
        }
        let idle = self
            .last_use
            .map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        if !syntax_release_due(idle, text_preview_open) {
            return false;
        }
        self.keep = None;
        true
    }

    /// 構文セットが今メモリに載っているか（保持していなくても、借用中なら載っている）
    fn resident(&self) -> bool {
        self.weak.strong_count() > 0
    }
}

static SYNTAX_CACHE: Mutex<SyntaxCache> = Mutex::new(SyntaxCache::new());

fn syntax_cache() -> MutexGuard<'static, SyntaxCache> {
    SYNTAX_CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// ハイライタの借用チケット（#815）。**これが生きている間は構文セットが解放されない**ので、
/// background のハイライト実行中に足元が消えることが型として起こり得ない。
pub struct SyntaxLease(Arc<SyntectHighlighter>);

impl Highlighter for SyntaxLease {
    fn highlight(&self, path: &Path, text: &str) -> Vec<Line> {
        self.0.highlight(path, text)
    }

    fn highlight_lang(&self, lang: &str, text: &str) -> Vec<Line> {
        self.0.highlight_lang(lang, text)
    }
}

/// 既定ハイライタを借りる。載っていなければここでロードする（実測 0.6〜1.2 ms）
pub fn highlighter() -> SyntaxLease {
    syntax_cache().acquire(Instant::now())
}

/// 保持を手放してよいか（純関数。テストが実時間を待たないための切り出し）
pub(crate) fn syntax_release_due(idle: Duration, text_preview_open: bool) -> bool {
    // テキストのプレビューが 1 枚も無ければ猶予を待たない（閉じた直後に返す）
    !text_preview_open || idle >= SYNTAX_IDLE_GRACE
}

/// 解放をやめて旧挙動（プロセス常駐）へ戻す逃げ道（`TAKO_815_NO_SYNTAX_RELEASE=1`）。
/// #815 の効果を同じバイナリで A/B するためのもの（#786 / #787 / #803 と同じ流儀）
pub(crate) fn syntax_release_disabled() -> bool {
    static OFF: OnceLock<bool> = OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_815_NO_SYNTAX_RELEASE").is_some())
}

/// 使っていない構文セットを手放す（2 秒 tick から呼ぶ。#815）。
/// 実際の解放は最後の [`SyntaxLease`] が落ちた時点で起こるので、ハイライト中に呼んでも安全。
/// 戻り値 = このターンで保持を手放したか
pub fn release_idle_syntax(now: Instant, text_preview_open: bool) -> bool {
    if syntax_release_disabled() {
        return false;
    }
    syntax_cache().release_idle(now, text_preview_open)
}

/// 構文セットが今メモリに載っているか（診断・セルフテスト用）
pub fn syntax_resident() -> bool {
    syntax_cache().resident()
}

/// syntect 実装（bat / delta と同系の定番。純 Rust 構成 = regex-fancy）
pub struct SyntectHighlighter {
    syntaxes: syntect::parsing::SyntaxSet,
    theme: syntect::highlighting::Theme,
}

impl SyntectHighlighter {
    fn new() -> Self {
        // bat 由来の拡張構文セット（TOML・TypeScript・Dockerfile 等 270+ 構文。#320）
        let syntaxes = two_face::syntax::extra_newlines();
        let mut themes = syntect::highlighting::ThemeSet::load_defaults().themes;
        let theme = themes
            .remove("base16-eighties.dark")
            .or_else(|| themes.into_values().next())
            .unwrap_or_default();
        Self { syntaxes, theme }
    }

    /// 読み取り / 編集で共用する構文解決。ファイル名 → 拡張子 → shebang の優先順で
    /// 構文を特定する。two-face（bat 由来）の 270+ 構文セットが基盤。
    /// ファイル名を先に試すのは CMakeLists.txt 等の拡張子が汎用（.txt）でも
    /// ファイル名で特定できるケースを拾うため。
    fn syntax_for_path<'a>(
        &'a self,
        path: &Path,
        text: &str,
    ) -> &'a syntect::parsing::SyntaxReference {
        let file_name = path.file_name().and_then(|v| v.to_str());
        // ファイル名での解決を先に試す（CMakeLists.txt・Dockerfile・Makefile 等）
        if let Some(syn) = file_name.and_then(|name| {
            self.syntaxes
                .find_syntax_by_extension(name)
                .or_else(|| self.filename_to_syntax(name))
        }) {
            return syn;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        extension
            .as_deref()
            .and_then(|ext| {
                self.syntaxes
                    .find_syntax_by_extension(ext)
                    .or_else(|| self.extension_fallback(ext))
            })
            .or_else(|| {
                text.lines()
                    .next()
                    .and_then(|line| self.syntaxes.find_syntax_by_first_line(line))
            })
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
    }

    /// ファイル名からの追加マッピング（拡張子が無い / 特殊なファイル名用）
    fn filename_to_syntax<'a>(
        &'a self,
        name: &str,
    ) -> Option<&'a syntect::parsing::SyntaxReference> {
        let mapped = match name {
            "Cargo.lock" | "Pipfile" | "Pipfile.lock" | "poetry.lock" => "toml",
            ".dockerignore" => "gitignore",
            ".editorconfig" | ".npmrc" | ".yarnrc" => "ini",
            ".eslintrc" | ".prettierrc" | ".babelrc" => "json",
            "Justfile" | "justfile" => "makefile",
            _ => return None,
        };
        self.syntaxes
            .find_syntax_by_extension(mapped)
            .or_else(|| self.syntaxes.find_syntax_by_name(mapped))
    }

    /// 拡張子からの追加フォールバック（構文セットに定義が無い拡張子用）
    fn extension_fallback<'a>(
        &'a self,
        ext: &str,
    ) -> Option<&'a syntect::parsing::SyntaxReference> {
        let mapped = match ext {
            "jsx" => "js",
            "mjs" | "cjs" => "js",
            "mts" | "cts" => "ts",
            _ => return None,
        };
        self.syntaxes.find_syntax_by_extension(mapped)
    }

    fn run(&self, syntax: &syntect::parsing::SyntaxReference, text: &str) -> Vec<Line> {
        use syntect::easy::HighlightLines;
        use syntect::util::LinesWithEndings;
        let mut hl = HighlightLines::new(syntax, &self.theme);
        // `load_defaults_newlines` の構文は改行込みの入力を前提にする。`str::lines()` で
        // 改行を落とすと shell の shebang 後などで状態遷移が閉じず、標準言語でも行全体が
        // 同じ色になる。パーサには改行を渡し、UI の 1 行要素からは末尾改行だけ除く。
        LinesWithEndings::from(text)
            .map(|line| {
                match hl.highlight_line(line, &self.syntaxes) {
                    Ok(regions) => {
                        let visible_len = line
                            .strip_suffix("\r\n")
                            .or_else(|| line.strip_suffix('\n'))
                            .map_or(line.len(), str::len);
                        let mut remaining = visible_len;
                        regions
                            .into_iter()
                            .filter_map(|(style, fragment)| {
                                if remaining == 0 {
                                    return None;
                                }
                                let len = fragment.len().min(remaining);
                                remaining -= len;
                                Some(Span {
                                    text: fragment[..len].to_string(),
                                    color: Some(tako_core::Rgb {
                                        r: style.foreground.r,
                                        g: style.foreground.g,
                                        b: style.foreground.b,
                                    }),
                                    bold: style
                                        .font_style
                                        .contains(syntect::highlighting::FontStyle::BOLD),
                                    italic: style
                                        .font_style
                                        .contains(syntect::highlighting::FontStyle::ITALIC),
                                })
                            })
                            .collect()
                    }
                    // ハイライト失敗行は素のテキストへ劣化（表示を欠けさせない）
                    Err(_) => vec![plain_span(line.trim_end_matches(['\r', '\n']))],
                }
            })
            .collect()
    }
}

fn plain_span(text: &str) -> Span {
    Span {
        text: text.to_string(),
        color: None,
        bold: false,
        italic: false,
    }
}

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, path: &Path, text: &str) -> Vec<Line> {
        self.run(self.syntax_for_path(path, text), text)
    }

    fn highlight_lang(&self, lang: &str, text: &str) -> Vec<Line> {
        let syntax = self
            .syntaxes
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.run(syntax, text)
    }
}

/// パース中に持ち回る構造の状態。`flush` がここを見てブロック種別を決める
#[derive(Default)]
struct MdParseState {
    /// リストのネスト（None = 箇条書き、Some(n) = 番号付きの次番号）
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    heading: Option<u8>,
    /// 直近の `- [ ]` / `- [x]` マーカー（項目の先頭ブロックにだけ効く）
    task: Option<bool>,
    /// 今のリスト項目でまだブロックを出していない（= マーカーを描く番）
    item_pending_marker: bool,
}

/// Markdown をブロック列へパースする（FR-3.3）。GFM テーブルは表構造として保持し、
/// HTML など未対応の構造はテキストとして段落へ劣化させ、内容を落とさない。
fn parse_markdown_blocks(text: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    // Issue #656: 表を表構造として受け取る（従来は素のテキストへ潰れていた）
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(text, options);

    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut spans: Vec<MdSpan> = Vec::new();
    let (mut bold, mut italic, mut strike) = (0u32, 0u32, 0u32);
    // リンクは深さカウンタではなく遷移先のスタックで持つ（⌘+クリックで開くため。#680）。
    // md にリンクの入れ子は無いが、閉じ忘れの入力でも `last()` が現在のリンクを指す
    let mut link_urls: Vec<String> = Vec::new();
    let mut state = MdParseState::default();
    // コードブロック蓄積（lang, 本文）
    let mut code: Option<(Option<String>, String)> = None;
    // 表の蓄積（Some の間はセル単位でスパンを回収する）
    let mut table: Option<MdTableBuilder> = None;

    let push_span = |spans: &mut Vec<MdSpan>,
                     text: &str,
                     code_span: bool,
                     bold: u32,
                     italic: u32,
                     strike: u32,
                     link: Option<&str>| {
        if text.is_empty() {
            return;
        }
        spans.push(MdSpan {
            text: text.to_string(),
            bold: bold > 0,
            italic: italic > 0,
            code: code_span,
            strike: strike > 0,
            link_url: link.map(str::to_string),
        });
    };
    // 段落・見出し等の区切りで溜まったスパンをブロック化する
    fn flush(blocks: &mut Vec<MdBlock>, spans: &mut Vec<MdSpan>, state: &mut MdParseState) {
        if spans.is_empty() {
            return;
        }
        let spans = std::mem::take(spans);
        let kind = if let Some(level) = state.heading {
            MdBlockKind::Heading { level, spans }
        } else if let Some(counter) = state.lists.last() {
            let first = state.item_pending_marker;
            state.item_pending_marker = false;
            MdBlockKind::ListItem {
                ordered: counter.map(|n| n.saturating_sub(1)),
                task: if first { state.task.take() } else { None },
                continuation: !first,
                spans,
            }
        } else {
            MdBlockKind::Paragraph { spans }
        };
        blocks.push(MdBlock::nested(kind, state.quote_depth, state.lists.len()));
    }

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut spans, &mut state);
                state.heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut blocks, &mut spans, &mut state);
                state.heading = None;
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut blocks, &mut spans, &mut state);
                state.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut blocks, &mut spans, &mut state);
                state.lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut spans, &mut state);
                if let Some(Some(counter)) = state.lists.last_mut() {
                    *counter += 1;
                }
                state.item_pending_marker = true;
                state.task = None;
            }
            Event::End(TagEnd::Item) => {
                flush(&mut blocks, &mut spans, &mut state);
                // 中身が空の項目（`-` だけの行）でもマーカーは残す
                if state.item_pending_marker {
                    flush_empty_item(&mut blocks, &mut state);
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut blocks, &mut spans, &mut state);
                state.quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut blocks, &mut spans, &mut state);
                state.quote_depth = state.quote_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut blocks, &mut spans, &mut state);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .filter(|token| !token.is_empty())
                        .map(str::to_string),
                    CodeBlockKind::Indented => None,
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, body)) = code.take() {
                    let body = body.strip_suffix('\n').unwrap_or(&body);
                    let lines = highlighter().highlight_lang(lang.as_deref().unwrap_or(""), body);
                    let kind = MdBlockKind::CodeBlock { lang, lines };
                    // リスト項目の中のコードブロックも項目の続きとして字下げする
                    state.item_pending_marker = false;
                    blocks.push(MdBlock::nested(kind, state.quote_depth, state.lists.len()));
                }
            }
            Event::Start(Tag::Table(alignments)) => {
                flush(&mut blocks, &mut spans, &mut state);
                spans.clear();
                table = Some(MdTableBuilder::new(&alignments));
            }
            Event::End(TagEnd::Table) => {
                if let Some(builder) = table.take() {
                    state.item_pending_marker = false;
                    blocks.push(MdBlock::nested(
                        builder.finish(),
                        state.quote_depth,
                        state.lists.len(),
                    ));
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(builder) = table.as_mut() {
                    builder.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(builder) = table.as_mut() {
                    builder.end_row();
                    builder.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                if let Some(builder) = table.as_mut() {
                    builder.end_row();
                }
            }
            Event::Start(Tag::TableCell) => spans.clear(),
            Event::End(TagEnd::TableCell) => {
                if let Some(builder) = table.as_mut() {
                    builder.push_cell(std::mem::take(&mut spans));
                }
            }
            Event::Start(Tag::Paragraph) => {
                flush(&mut blocks, &mut spans, &mut state);
            }
            Event::End(TagEnd::Paragraph) => {
                flush(&mut blocks, &mut spans, &mut state);
            }
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => link_urls.push(dest_url.to_string()),
            Event::End(TagEnd::Link) => {
                link_urls.pop();
            }
            Event::Rule => {
                flush(&mut blocks, &mut spans, &mut state);
                blocks.push(MdBlock::new(MdBlockKind::Rule));
            }
            Event::Text(t) => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                } else {
                    push_span(
                        &mut spans,
                        &t,
                        false,
                        bold,
                        italic,
                        strike,
                        link_urls.last().map(String::as_str),
                    );
                }
            }
            Event::Code(t) => push_span(
                &mut spans,
                &t,
                true,
                bold,
                italic,
                strike,
                link_urls.last().map(String::as_str),
            ),
            Event::SoftBreak | Event::HardBreak => push_span(
                &mut spans,
                " ",
                false,
                bold,
                italic,
                strike,
                link_urls.last().map(String::as_str),
            ),
            // チェックボックスは描画側が図形で描く（絵文字全廃 #217）。ここでは状態だけ持つ
            Event::TaskListMarker(done) => state.task = Some(done),
            // HTML 等はインラインテキストとして劣化（内容を落とさない）
            Event::Html(t) | Event::InlineHtml(t) => push_span(
                &mut spans,
                &t,
                false,
                bold,
                italic,
                strike,
                link_urls.last().map(String::as_str),
            ),
            _ => {}
        }
    }
    flush(&mut blocks, &mut spans, &mut state);
    blocks
}

/// 中身が空のリスト項目（`- [ ]` だけ / `-` だけ）でもマーカー行を残す
fn flush_empty_item(blocks: &mut Vec<MdBlock>, state: &mut MdParseState) {
    let Some(counter) = state.lists.last() else {
        return;
    };
    state.item_pending_marker = false;
    blocks.push(MdBlock::nested(
        MdBlockKind::ListItem {
            ordered: counter.map(|n| n.saturating_sub(1)),
            task: state.task.take(),
            continuation: false,
            spans: Vec::new(),
        },
        state.quote_depth,
        state.lists.len(),
    ));
}

/// 等幅フォントでの表示幅の近似（全角 = 2、半角 = 1）。表の列幅の初期配分に使うだけなので
/// East Asian Width の厳密判定までは要らない（Issue #656）
pub fn md_display_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            let cp = c as u32;
            let wide = matches!(cp,
                0x1100..=0x115F      // ハングル字母
                | 0x2E80..=0x303E    // CJK 部首・記号
                | 0x3041..=0x33FF    // かな・注音・囲み CJK
                | 0x3400..=0x4DBF    // CJK 拡張 A
                | 0x4E00..=0x9FFF    // CJK 統合漢字
                | 0xA000..=0xA4CF    // イ文字
                | 0xAC00..=0xD7A3    // ハングル音節
                | 0xF900..=0xFAFF    // CJK 互換漢字
                | 0xFE30..=0xFE6F    // CJK 互換形
                | 0xFF00..=0xFF60    // 全角形
                | 0xFFE0..=0xFFE6
                | 0x1F300..=0x1FAFF  // 絵文字（幅 2 で数える）
                | 0x20000..=0x3FFFD  // CJK 拡張 B 以降
            );
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

/// 表の列幅の比（合計 1.0）。内容の表示幅から決めるが、極端な比率にならないよう
/// 1 列あたり 4〜30 文字相当へ丸め、さらにセルの左右パディング相当を全列へ一律に足す。
/// パディング分を足さないと「短いが折り返せない語」（`dark` 等）の列が痩せすぎて
/// 1 語が 2 行に割れる。狭いペインでも全列が見えるよう flex で伸縮させる前提の
/// 初期配分なので、厳密な実測幅ではなく表示幅の近似で足りる（Issue #656）
pub fn md_table_column_shares(header: &[MdCell], rows: &[Vec<MdCell>], columns: usize) -> Vec<f32> {
    if columns == 0 {
        return Vec::new();
    }
    let cell_width = |cell: &MdCell| -> usize {
        cell.iter()
            .map(|span| md_display_width(&span.text))
            .sum::<usize>()
    };
    let mut widths = vec![0usize; columns];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (index, cell) in row.iter().enumerate().take(columns) {
            widths[index] = widths[index].max(cell_width(cell));
        }
    }
    // セル左右パディング（描画側の `base * 0.6` × 2）を文字幅へ換算した分
    const PADDING_COLUMNS: f32 = 2.0;
    let weighted: Vec<f32> = widths
        .iter()
        .map(|w| (*w).clamp(4, 30) as f32 + PADDING_COLUMNS)
        .collect();
    let total: f32 = weighted.iter().sum();
    if total <= 0.0 {
        return vec![1.0 / columns as f32; columns];
    }
    weighted.iter().map(|w| w / total).collect()
}

/// 箇条書きマーカーの字形（絵文字を使わず図形で描く。#217 の絵文字全廃に従う）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdBullet {
    /// 塗りの丸（第 1 階層）
    Dot,
    /// 中抜きの丸（第 2 階層）
    Ring,
    /// 塗りの四角（第 3 階層以降）
    Square,
}

/// リストのネスト段（1 始まり）からマーカー字形を決める
pub fn md_bullet_for_depth(list_depth: usize) -> MdBullet {
    match list_depth {
        0 | 1 => MdBullet::Dot,
        2 => MdBullet::Ring,
        _ => MdBullet::Square,
    }
}

/// インライン列の中のリンクを「連結テキスト上のバイト範囲 + 遷移先」で返す（#680）。
///
/// 範囲の原点は `md_block_line_texts` が作る 1 行分のテキスト（= スパンの text を
/// 連結したもの）で、テキスト選択・ヒットテストと同じ座標系。`**強調**` 混在で
/// 1 リンクが複数スパンに割れるので、同じ遷移先が隣接していれば 1 本へ束ねる。
pub fn md_link_ranges(spans: &[MdSpan]) -> Vec<(std::ops::Range<usize>, String)> {
    let mut out: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut offset = 0usize;
    for span in spans {
        let start = offset;
        offset += span.text.len();
        let Some(url) = span.link_url.as_deref() else {
            continue;
        };
        match out.last_mut() {
            Some((range, prev)) if prev == url && range.end == start => range.end = offset,
            _ => out.push((start..offset, url.to_string())),
        }
    }
    out
}

/// コードブロックの装飾なし論理テキスト（コピー用。行は `\n` 区切り）。
///
/// ハイライトのスパン分割・色を落として元のコードへ戻す。インデントと空行は
/// そのまま残る（`md_block_line_texts` のコードブロック行と同一の文字列）
pub fn md_code_block_text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 表の組み立て。列数は必ずヘッダ（= 配置指定）の長さに正規化するので、
/// 行ごとにセル数が違う壊れた表でもグリッドがずれない（Issue #656）
struct MdTableBuilder {
    align: Vec<MdAlign>,
    header: Vec<MdCell>,
    rows: Vec<Vec<MdCell>>,
    row: Vec<MdCell>,
    in_head: bool,
}

impl MdTableBuilder {
    fn new(alignments: &[pulldown_cmark::Alignment]) -> Self {
        use pulldown_cmark::Alignment;
        Self {
            align: alignments
                .iter()
                .map(|a| match a {
                    Alignment::None => MdAlign::None,
                    Alignment::Left => MdAlign::Left,
                    Alignment::Center => MdAlign::Center,
                    Alignment::Right => MdAlign::Right,
                })
                .collect(),
            header: Vec::new(),
            rows: Vec::new(),
            row: Vec::new(),
            in_head: false,
        }
    }

    fn push_cell(&mut self, cell: MdCell) {
        self.row.push(cell);
    }

    fn end_row(&mut self) {
        let row = std::mem::take(&mut self.row);
        if row.is_empty() {
            return;
        }
        if self.in_head {
            self.header = row;
        } else {
            self.rows.push(row);
        }
    }

    fn finish(mut self) -> MdBlockKind {
        // 未確定の行（End(TableRow) が来ない壊れた入力）も取りこぼさない
        self.end_row();
        let columns = self
            .align
            .len()
            .max(self.header.len())
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(1);
        self.align.resize(columns, MdAlign::None);
        let fit = |mut row: Vec<MdCell>| {
            row.resize(columns, Vec::new());
            row
        };
        MdBlockKind::Table {
            align: self.align,
            header: fit(self.header),
            rows: self.rows.into_iter().map(fit).collect(),
        }
    }
}

/// Markdown 本文とアウトラインを 1 回のロードで完成させる（Issue #232）。
/// アウトラインの対象は描画ブロック番号なので、クリック時に再パースせず直接スクロールできる。
fn markdown_document(text: &str) -> (Vec<MdBlock>, PreviewOutline) {
    let blocks = parse_markdown_blocks(text);
    let items = blocks
        .iter()
        .enumerate()
        .filter_map(|(block, entry)| {
            let MdBlockKind::Heading { level, spans } = &entry.kind else {
                return None;
            };
            let title = spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
                .trim()
                .to_string();
            (!title.is_empty()).then_some(PreviewOutlineItem {
                title,
                level: *level,
                target: PreviewOutlineTarget::MarkdownBlock { block },
            })
        })
        .collect();
    (blocks, PreviewOutline::new(items))
}

/// Markdown ブロックだけを必要とする経路（アウトライン不要のとき）。
///
/// アップデート詳細画面のリリースノート（#690）と単体テストが使う。
/// **md のパースはこの 1 本（`pulldown-cmark`）が正**で、表示側が独自に解くことはしない。
pub fn markdown_blocks(text: &str) -> Vec<MdBlock> {
    parse_markdown_blocks(text)
}

/// 表示品質確認用の代表 Markdown（Issue #656）。全ブロック種別・全見出しレベル・
/// 配置指定つき表・ネスト引用・深いリスト・タスクリストを 1 本に収めてある。
/// 単体テストと visual-test（実ピクセル検査）が同じ内容を見るため定数で共有する。
#[cfg(any(test, feature = "visual-test"))]
pub const MARKDOWN_SHOWCASE: &str = include_str!("../resources/fixtures/markdown-showcase.md");

#[cfg(test)]
mod tests {
    use super::*;

    fn is_pdf_path(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some(ext) if ext.eq_ignore_ascii_case("pdf")
        )
    }

    fn is_video_path(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some(ext) if matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp4" | "webm" | "mov" | "avi" | "mkv"
            )
        )
    }

    #[test]
    fn rustコードがハイライトされる() {
        let dir = std::env::temp_dir().join(format!("tako-preview-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.rs");
        std::fs::write(&path, "fn main() {\n    let x = 1;\n}\n").unwrap();
        let state = load(&path, PreviewMode::Code);
        let PreviewContent::Code(lines) = &state.content else {
            panic!("Code になる: {:?}", state.content);
        };
        assert_eq!(lines.len(), 3);
        // キーワード `fn` が複数スパンに分かれ、色が付く
        assert!(lines[0].len() > 1, "1 行目が複数スパンに分かれる");
        assert!(lines[0].iter().any(|s| s.color.is_some()));
        assert_eq!(
            lines[0].iter().map(|s| s.text.as_str()).collect::<String>(),
            "fn main() {"
        );
        assert!(!state.truncated);
        assert!(!state.markdown_capable());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdownがブロックへパースされる() {
        let text = "# 見出し\n\n本文 **強調** と `code`。\n\n- 項目1\n- 項目2\n\n```rust\nfn f() {}\n```\n\n---\n";
        let blocks = markdown_blocks(text);
        assert!(matches!(
            &blocks[0].kind,
            MdBlockKind::Heading { level: 1, spans } if spans[0].text == "見出し"
        ));
        let MdBlockKind::Paragraph { spans } = &blocks[1].kind else {
            panic!("段落になる: {:?}", blocks[1]);
        };
        assert!(spans.iter().any(|s| s.bold && s.text == "強調"));
        assert!(spans.iter().any(|s| s.code && s.text == "code"));
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match &b.kind {
                MdBlockKind::ListItem { ordered, spans, .. } => {
                    Some((*ordered, spans[0].text.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![(None, "項目1".to_string()), (None, "項目2".to_string())]
        );
        assert!(blocks
            .iter()
            .any(|b| matches!(&b.kind, MdBlockKind::CodeBlock { lang, lines }
                if lang.as_deref() == Some("rust") && !lines.is_empty())));
        assert!(blocks.iter().any(|b| matches!(b.kind, MdBlockKind::Rule)));
    }

    /// Issue #656: GFM テーブルが表構造として保持され、配置指定も残る
    #[test]
    fn gfmテーブルが表構造へパースされる() {
        let text = "| 左 | 中央 | 右 |\n|:---|:---:|---:|\n| a | `b` | **c** |\n| d |  | f |\n";
        let blocks = markdown_blocks(text);
        let table = blocks
            .iter()
            .find_map(|b| match &b.kind {
                MdBlockKind::Table {
                    align,
                    header,
                    rows,
                } => Some((align, header, rows)),
                _ => None,
            })
            .expect("表ブロックになる");
        let (align, header, rows) = table;
        assert_eq!(align, &vec![MdAlign::Left, MdAlign::Center, MdAlign::Right]);
        assert_eq!(header.len(), 3);
        assert_eq!(header[1][0].text, "中央");
        assert_eq!(rows.len(), 2);
        // セル内のインライン装飾は保持される
        assert!(rows[0][1].iter().any(|s| s.code && s.text == "b"));
        assert!(rows[0][2].iter().any(|s| s.bold && s.text == "c"));
        // 空セルは空スパン列（列数はヘッダに合わせて正規化）
        assert_eq!(rows[1].len(), 3);
        assert!(rows[1][1].is_empty());
    }

    /// 行ごとにセル数が違う壊れた表でも列数を正規化してグリッドを崩さない
    #[test]
    fn 列数が揃わない表は正規化される() {
        let text = "| a | b | c |\n|---|---|---|\n| 1 |\n| 1 | 2 | 3 | 4 |\n";
        let blocks = markdown_blocks(text);
        let MdBlockKind::Table {
            align,
            header,
            rows,
        } = &blocks
            .iter()
            .find(|b| matches!(b.kind, MdBlockKind::Table { .. }))
            .expect("表ブロックになる")
            .kind
        else {
            unreachable!()
        };
        let columns = align.len();
        assert_eq!(header.len(), columns);
        for row in rows {
            assert_eq!(row.len(), columns, "全行が同じ列数へ揃う");
        }
    }

    /// タスクリストはテキストではなく状態として持つ（描画側が図形で描く）
    #[test]
    fn タスクリストは完了状態を持つ() {
        let blocks = markdown_blocks("- [x] done\n- [ ] todo\n- plain\n");
        let tasks: Vec<_> = blocks
            .iter()
            .filter_map(|b| match &b.kind {
                MdBlockKind::ListItem { task, spans, .. } => {
                    Some((*task, spans[0].text.trim().to_string()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            tasks,
            vec![
                (Some(true), "done".to_string()),
                (Some(false), "todo".to_string()),
                (None, "plain".to_string()),
            ]
        );
        // 旧実装は "[x] " をテキストへ混ぜていた。混ざっていないことを固定する
        assert!(blocks.iter().all(|b| match &b.kind {
            MdBlockKind::ListItem { spans, .. } => !spans.iter().any(|s| s.text.contains("[x]")),
            _ => true,
        }));
    }

    /// 引用のネストと、引用の中のリスト・コードブロックが引用深さを持つ
    #[test]
    fn 引用の深さが各ブロックに載る() {
        let blocks = markdown_blocks("> 外側\n>\n> > 内側\n>\n> - 引用内リスト\n\n平文\n");
        let depths: Vec<_> = blocks
            .iter()
            .map(|b| {
                (
                    b.quote_depth,
                    match &b.kind {
                        MdBlockKind::Paragraph { spans } => spans[0].text.clone(),
                        MdBlockKind::ListItem { spans, .. } => spans[0].text.clone(),
                        other => format!("{other:?}"),
                    },
                )
            })
            .collect();
        assert_eq!(depths[0], (1, "外側".to_string()));
        assert_eq!(depths[1], (2, "内側".to_string()));
        assert_eq!(depths[2], (1, "引用内リスト".to_string()));
        assert_eq!(depths[3], (0, "平文".to_string()));
    }

    /// リスト項目の 2 つ目以降のブロックはマーカーを重複させない
    #[test]
    fn リスト項目内の継続ブロックはマーカーを出さない() {
        let blocks = markdown_blocks("- 一段落目\n\n  二段落目\n\n  ```sh\n  ls\n  ```\n");
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match &b.kind {
                MdBlockKind::ListItem {
                    continuation,
                    spans,
                    ..
                } => Some((*continuation, spans[0].text.clone(), b.list_depth)),
                _ => None,
            })
            .collect();
        assert_eq!(items[0], (false, "一段落目".to_string(), 1));
        assert_eq!(items[1], (true, "二段落目".to_string(), 1));
        // 項目内のコードブロックもリスト段を持ち、字下げが揃う
        assert!(blocks
            .iter()
            .any(|b| matches!(&b.kind, MdBlockKind::CodeBlock { .. }) && b.list_depth == 1));
    }

    #[test]
    fn md_display_widthは全角を2で数える() {
        assert_eq!(md_display_width("dark"), 4);
        assert_eq!(md_display_width("既定値"), 6);
        assert_eq!(md_display_width("UI テーマ"), 3 + 6);
        // 記号・アクセント付きラテンは半角扱い
        assert_eq!(md_display_width("café"), 4);
        assert_eq!(md_display_width(""), 0);
    }

    /// Issue #656: 短くて折り返せない列（`dark`）が痩せて 1 語 2 行に割れないこと。
    /// パディング相当を足さない旧実装ではこの列の取り分が足りなかった
    #[test]
    fn md_table_column_sharesは短い列にも取り分を残す() {
        let cell = |text: &str| -> MdCell {
            vec![MdSpan {
                text: text.to_string(),
                ..MdSpan::default()
            }]
        };
        let header = vec![cell("コマンド"), cell("既定値"), cell("説明")];
        let rows = vec![
            vec![
                cell("tako autosuggest"),
                cell("dark"),
                cell("UI テーマの切替。引数なしで現在値"),
            ],
            vec![cell("tako lang"), cell("auto"), cell("表示言語")],
        ];
        let shares = md_table_column_shares(&header, &rows, 3);
        assert_eq!(shares.len(), 3);
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // 450px 相当の表幅で「dark」(4 文字 ≒ 34px) + パディング 16px が入ること
        let table_width = 450.0;
        let value_column = shares[1] * table_width;
        assert!(
            value_column >= 50.0,
            "既定値の列が痩せすぎ: {value_column:.1}px"
        );
        // 一番広い列が全体を占有しない（他列が読める幅を保つ）
        assert!(shares[2] < 0.7, "説明の列が占有しすぎ: {}", shares[2]);
        // 列数だけ渡して中身が空でも均等割りで返る
        let empty = md_table_column_shares(&[], &[], 3);
        assert_eq!(empty.len(), 3);
        assert!((empty[0] - 1.0 / 3.0).abs() < 1e-5);
        assert!(md_table_column_shares(&[], &[], 0).is_empty());
    }

    #[test]
    fn md_bullet_for_depthは階層で字形を変える() {
        assert_eq!(md_bullet_for_depth(1), MdBullet::Dot);
        assert_eq!(md_bullet_for_depth(2), MdBullet::Ring);
        assert_eq!(md_bullet_for_depth(3), MdBullet::Square);
        assert_eq!(md_bullet_for_depth(9), MdBullet::Square);
    }

    /// 言語指定なしフェンスは lang=None（描画側が等幅の素表示にする）
    #[test]
    fn 言語指定なしコードフェンス() {
        let blocks = markdown_blocks("```\nplain text\n```\n");
        assert!(blocks.iter().any(|b| matches!(
            &b.kind,
            MdBlockKind::CodeBlock { lang: None, lines } if !lines.is_empty()
        )));
    }

    /// 代表 md（フィクスチャ）に全要素が含まれ、全部パースできること
    #[test]
    fn 代表フィクスチャに全ブロック種別が含まれる() {
        let blocks = markdown_blocks(MARKDOWN_SHOWCASE);
        let mut seen = std::collections::BTreeSet::new();
        let mut heading_levels = std::collections::BTreeSet::new();
        for block in &blocks {
            seen.insert(match &block.kind {
                MdBlockKind::Heading { level, .. } => {
                    heading_levels.insert(*level);
                    "heading"
                }
                MdBlockKind::Paragraph { .. } => "paragraph",
                MdBlockKind::ListItem { .. } => "list",
                MdBlockKind::CodeBlock { .. } => "code",
                MdBlockKind::Table { .. } => "table",
                MdBlockKind::Rule => "rule",
            });
        }
        for kind in ["heading", "paragraph", "list", "code", "table", "rule"] {
            assert!(seen.contains(kind), "{kind} がフィクスチャに無い");
        }
        assert_eq!(
            heading_levels,
            (1u8..=6).collect(),
            "H1〜H6 すべてがフィクスチャに要る"
        );
        // 引用・タスク・言語なしフェンス・配置指定つき表も揃っていること
        assert!(blocks.iter().any(|b| b.quote_depth >= 2), "ネスト引用");
        assert!(blocks.iter().any(|b| b.list_depth >= 3), "深いネストリスト");
        assert!(blocks
            .iter()
            .any(|b| matches!(&b.kind, MdBlockKind::ListItem { task: Some(_), .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(&b.kind, MdBlockKind::CodeBlock { lang: None, .. })));
        assert!(blocks.iter().any(|b| matches!(
            &b.kind,
            MdBlockKind::Table { align, .. }
                if align.contains(&MdAlign::Center) && align.contains(&MdAlign::Right)
        )));
        // #680: ⌘+クリックの検証に必要な「開けるリンク」と「開けないリンク」が両方要る
        let urls: Vec<String> = blocks
            .iter()
            .flat_map(|b| match &b.kind {
                MdBlockKind::Heading { spans, .. }
                | MdBlockKind::Paragraph { spans }
                | MdBlockKind::ListItem { spans, .. } => md_link_ranges(spans),
                MdBlockKind::Table { header, rows, .. } => std::iter::once(header)
                    .chain(rows.iter())
                    .flat_map(|row| row.iter())
                    .flat_map(|cell| md_link_ranges(cell))
                    .collect(),
                _ => Vec::new(),
            })
            .map(|(_, url)| url)
            .collect();
        assert!(
            urls.iter()
                .filter(|u| tako_core::md_links::browser_url(u).is_some())
                .count()
                >= 2,
            "開ける http(s) リンクが 2 本以上要る: {urls:?}"
        );
        for expected in ["#", "./", "javascript:"] {
            assert!(
                urls.iter().any(|u| u.starts_with(expected)),
                "開けないリンク（{expected}）がフィクスチャに無い: {urls:?}"
            );
        }
    }

    #[test]
    fn markdownアウトラインは重複見出しを別位置へ保持する() {
        let (blocks, outline) =
            markdown_document("# 概要\n\n本文\n\n## 詳細\n\n説明\n\n## 詳細\n\n再掲\n");
        assert_eq!(blocks.len(), 6);
        assert_eq!(outline.items.len(), 3);
        assert_eq!(outline.items[0].title, "概要");
        assert_eq!(outline.items[0].level, 1);
        assert_eq!(
            outline.items[0].target,
            PreviewOutlineTarget::MarkdownBlock { block: 0 }
        );
        assert_eq!(outline.items[1].title, "詳細");
        assert_eq!(outline.items[2].title, "詳細");
        assert_ne!(outline.items[1].target, outline.items[2].target);
    }

    #[test]
    fn markdown見出しなしは空アウトラインになる() {
        let (_, outline) = markdown_document("本文だけ\n\n- 項目\n");
        assert!(outline.is_empty());
    }

    #[test]
    fn 番号付きリストとネスト() {
        let blocks = markdown_blocks("1. one\n2. two\n   - sub\n");
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match &b.kind {
                MdBlockKind::ListItem { ordered, spans, .. } => {
                    Some((b.list_depth, *ordered, spans[0].text.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (1, Some(1), "one".to_string()),
                (1, Some(2), "two".to_string()),
                (2, None, "sub".to_string()),
            ]
        );
    }

    /// 開始番号を指定した番号付きリスト（`5.` から始まる）でも表示番号が続く
    #[test]
    fn 番号付きリストの開始番号を尊重する() {
        let blocks = markdown_blocks("5. five\n6. six\n");
        let numbers: Vec<_> = blocks
            .iter()
            .filter_map(|b| match &b.kind {
                MdBlockKind::ListItem { ordered, .. } => *ordered,
                _ => None,
            })
            .collect();
        assert_eq!(numbers, vec![5, 6]);
    }

    #[test]
    fn バイナリと不在は明示エラーになる() {
        let dir = std::env::temp_dir().join(format!("tako-preview-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bin.dat");
        std::fs::write(&path, [0u8, 159, 146, 150]).unwrap();
        let state = load(&path, PreviewMode::Code);
        assert!(
            matches!(&state.content, PreviewContent::Error(m) if m == crate::ui_text::preview::binary_file())
        );
        let state = load(&dir.join("no-such.txt"), PreviewMode::Code);
        assert!(matches!(&state.content, PreviewContent::Error(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ライブリロードは完成状態を作り巨大ファイルを上限で止める() {
        let dir = std::env::temp_dir().join(format!("tako-preview-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        std::fs::write(&path, "# 変更後\n").unwrap();

        let loaded = load_for_reload(&path, PreviewMode::Markdown, None);
        assert!(matches!(
            &loaded.state.content,
            PreviewContent::Markdown(blocks)
                if matches!(&blocks[0].kind, MdBlockKind::Heading { spans, .. }
                    if spans[0].text == "変更後")
        ));
        assert_eq!(
            loaded.source_bytes.as_deref(),
            Some("# 変更後\n".as_bytes())
        );

        std::fs::write(&path, vec![b'x'; MAX_BYTES + 128]).unwrap();
        let huge = load_for_reload(&path, PreviewMode::Code, None);
        assert!(huge.state.truncated);
        assert!(huge.source_bytes.is_none());
        assert!(matches!(huge.state.content, PreviewContent::Code(_)));

        std::fs::remove_file(&path).unwrap();
        let deleted = load_for_reload(&path, PreviewMode::Markdown, None);
        assert!(matches!(deleted.state.content, PreviewContent::Error(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ライブリロード対象はテキスト・画像・pdfに限る() {
        assert!(live_reload_supported(PreviewMode::Code));
        assert!(live_reload_supported(PreviewMode::Markdown));
        assert!(live_reload_supported(PreviewMode::Image));
        assert!(live_reload_supported(PreviewMode::Pdf));
        assert!(!live_reload_supported(PreviewMode::Video));
    }

    /// 性能計測（通常テストでは走らせない）: `cargo test -p tako-app --release -- --ignored --nocapture perf_`
    #[test]
    #[ignore]
    fn perf_ハイライト計測() {
        use std::time::Instant;
        let src_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");

        let t0 = Instant::now();
        let hl = highlighter();
        let init = t0.elapsed();
        eprintln!("[perf] SyntaxSet+Theme ロード: {:?}", init);

        let text = std::fs::read_to_string(&src_path).unwrap();
        let lines = text.lines().count().min(MAX_LINES);
        let capped: String = text.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");

        let t1 = Instant::now();
        let out = hl.highlight(&src_path, &capped);
        eprintln!(
            "[perf] highlight main.rs（{} 行）: {:?}（{} 行出力）",
            lines,
            t1.elapsed(),
            out.len()
        );

        // 2 回目（SyntaxSet ロード済み）の load() 全体 = 旧同期経路
        let t2 = Instant::now();
        let state = load(&src_path, PreviewMode::Code);
        eprintln!(
            "[perf] load() 同期全体: {:?} truncated={}",
            t2.elapsed(),
            state.truncated
        );

        // load_fast = UI スレッドが払うコスト（ファイル読み + 平文化のみ）
        let t2b = Instant::now();
        let (fast_state, raw) = load_fast(&src_path, PreviewMode::Code);
        eprintln!(
            "[perf] load_fast() UI コスト: {:?} truncated={} raw={}bytes",
            t2b.elapsed(),
            fast_state.truncated,
            raw.as_ref().map(|s| s.len()).unwrap_or(0)
        );

        // Markdown: このリポジトリの requirements.md（大きめの実物）
        let md_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.agent/requirements.md");
        if md_path.is_file() {
            let md = std::fs::read_to_string(&md_path).unwrap();
            let t3 = Instant::now();
            let blocks = markdown_blocks(&md);
            eprintln!(
                "[perf] markdown_blocks requirements.md（{} bytes）: {:?}（{} ブロック）",
                md.len(),
                t3.elapsed(),
                blocks.len()
            );
        }
    }

    #[test]
    fn markdown判定はパス拡張子から() {
        assert!(is_markdown_path(Path::new("/a/README.md")));
        assert!(is_markdown_path(Path::new("/a/B.Markdown")));
        assert!(!is_markdown_path(Path::new("/a/main.rs")));
    }

    #[test]
    fn 画像フォーマット判定() {
        assert_eq!(
            image_format_from_path(Path::new("/a/icon.png")),
            Some(ImageFileFormat::Png)
        );
        assert_eq!(
            image_format_from_path(Path::new("/a/photo.JPG")),
            Some(ImageFileFormat::Jpeg)
        );
        assert_eq!(
            image_format_from_path(Path::new("/a/anim.gif")),
            Some(ImageFileFormat::Gif)
        );
        assert_eq!(
            image_format_from_path(Path::new("/a/modern.webp")),
            Some(ImageFileFormat::WebP)
        );
        assert_eq!(
            image_format_from_path(Path::new("/a/vector.svg")),
            Some(ImageFileFormat::Svg)
        );
        assert_eq!(image_format_from_path(Path::new("/a/main.rs")), None);
    }

    #[test]
    fn pdf判定() {
        assert!(is_pdf_path(Path::new("/a/doc.pdf")));
        assert!(is_pdf_path(Path::new("/a/DOC.PDF")));
        assert!(!is_pdf_path(Path::new("/a/main.rs")));
    }

    #[test]
    fn pdfラスタキーはretina表示幅を実ピクセルへ変換する() {
        let key = PdfRasterKey::for_view(2.0, 1.0, 930.0);
        assert_eq!(key.logical_width_bucket, 960);
        assert_eq!(key.target_pixel_width(), 1920);

        let zoomed = PdfRasterKey::for_view(2.0, 1.5, 930.0);
        assert_eq!(zoomed.target_pixel_width(), 2880);
        assert_ne!(key, zoomed);
    }

    #[test]
    fn pdfラスタキーは連続リサイズを64px単位へ量子化する() {
        let a = PdfRasterKey::for_view(2.0, 1.0, 901.0);
        let b = PdfRasterKey::for_view(2.0, 1.0, 950.0);
        let c = PdfRasterKey::for_view(2.0, 1.0, 970.0);
        assert_eq!(a, b);
        assert_ne!(b, c);
    }

    #[test]
    fn 動画ファイル判定() {
        assert!(is_video_path(Path::new("/a/clip.mp4")));
        assert!(is_video_path(Path::new("/a/CLIP.MP4")));
        assert!(is_video_path(Path::new("/a/v.webm")));
        assert!(is_video_path(Path::new("/a/v.mov")));
        assert!(is_video_path(Path::new("/a/v.avi")));
        assert!(is_video_path(Path::new("/a/v.mkv")));
        assert!(!is_video_path(Path::new("/a/main.rs")));
        assert!(!is_video_path(Path::new("/a/photo.png")));
    }

    #[test]
    fn 不在動画ファイルはエラー() {
        let state = load(Path::new("/tmp/no-such-video.mp4"), PreviewMode::Video);
        assert_eq!(state.mode, PreviewMode::Video);
        assert!(matches!(&state.content, PreviewContent::Error(_)));
    }

    #[test]
    fn 存在する動画ファイルはvideoモードになる() {
        let dir = std::env::temp_dir().join(format!("tako-preview-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // ダミーファイル（ffmpeg は動かないが file_size は取れる）
        let path = dir.join("test.mp4");
        std::fs::write(&path, b"dummy-video-content").unwrap();
        let state = load(&path, PreviewMode::Video);
        assert_eq!(state.mode, PreviewMode::Video);
        match &state.content {
            PreviewContent::Video(data) => {
                assert_eq!(data.file_size, 19);
                // ffmpeg/ffprobe が無い環境ではサムネイル空・メタ情報 None
                // （テスト環境に ffmpeg がある場合はダミーなのでやはり空/None）
            }
            other => panic!("Video になる: {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 画像ファイルの読み込み() {
        let dir = std::env::temp_dir().join(format!("tako-preview-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 最小の有効な PNG（1x1 透明ピクセル）
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, // RGBA
            0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, // IDAT
            0x78, 0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, // data
            0x27, 0xDE, 0xFC, // checksum
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND
            0xAE, 0x42, 0x60, 0x82,
        ];
        let path = dir.join("test.png");
        std::fs::write(&path, &png_bytes).unwrap();
        let state = load(&path, PreviewMode::Image);
        assert_eq!(state.mode, PreviewMode::Image);
        match &state.content {
            PreviewContent::Image(data) => {
                assert_eq!(data.format, ImageFileFormat::Png);
                assert_eq!(data.bytes, png_bytes);
            }
            other => panic!("Image になる: {:?}", other),
        }
        // 不在ファイルはエラー
        let state = load(&dir.join("no-such.png"), PreviewMode::Image);
        assert!(matches!(&state.content, PreviewContent::Error(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// テスト用 PDF を組み立てる。`contents` の 1 要素が 1 ページのコンテンツストリームで、
    /// 各ページから Helvetica を `/F1` で参照できる。
    ///
    /// #521 以前は macOS のシステム PDF を探して無ければ skip していたが、
    /// それだと「レンダラが動く」ことを CI でも Windows でも確かめられない
    fn build_test_pdf(contents: &[&str]) -> Vec<u8> {
        let n = contents.len();
        // 番号割り当て: 1=Catalog / 2=Pages / 3..=2+n=Page / 3+n..=2+2n=Contents / 3+2n=Font
        let font_num = 3 + 2 * n;
        let mut objects: Vec<Vec<u8>> = Vec::new();
        objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + i)).collect();
        objects.push(
            format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.trim_end()).into_bytes(),
        );
        for i in 0..n {
            objects.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                     /Resources << /Font << /F1 {font_num} 0 R >> >> >>",
                    3 + n + i
                )
                .into_bytes(),
            );
        }
        for content in contents {
            objects.push(
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
                .into_bytes(),
            );
        }
        objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());

        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = pdf.len();
        let size = objects.len() + 1;
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

    fn write_test_pdf(dir_name: &str, file: &str, contents: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, build_test_pdf(contents)).unwrap();
        path
    }

    /// PDF レンダラを持つ環境（macOS / Windows）でだけ意味のある検査。
    /// それ以外では「理由つきエラーになる」ことだけを確かめる
    fn skip_unless_rasterizable() -> bool {
        if crate::platform::pdf::capabilities().rasterize {
            return false;
        }
        eprintln!("[skip] この環境には PDF レンダラが無い");
        true
    }

    #[test]
    fn pdfのページレンダリング() {
        if skip_unless_rasterizable() {
            return;
        }
        let pdf_path = write_test_pdf(
            "tako_pdf_render_test",
            "render.pdf",
            &[
                "BT /F1 24 Tf 72 700 Td (Page One) Tj ET 0.2 0.4 0.8 rg 72 400 200 100 re f",
                "BT /F1 24 Tf 72 700 Td (Page Two) Tj ET 0.8 0.2 0.2 rg 72 400 200 100 re f",
                "BT /F1 24 Tf 72 700 Td (Page Three) Tj ET",
            ],
        );

        let state = load(&pdf_path, PreviewMode::Pdf);
        match &state.content {
            PreviewContent::Pdf(data) => {
                assert_eq!(data.total_pages, 3, "全ページを数える");
                assert_eq!(data.pages.len(), 3, "全ページをラスタライズする");
                for (i, page) in data.pages.iter().enumerate() {
                    assert!(!page.is_empty(), "{i} ページ目が空でない");
                    assert_eq!(&page[..4], &[0x89, 0x50, 0x4E, 0x47], "{i} ページ目が PNG");
                }
                // MediaBox は 612x792 pt。px @96DPI のまま入っていれば 816x1056 になる
                for size in &data.page_sizes {
                    assert!(
                        (size[0] - 612.0).abs() < 0.5 && (size[1] - 792.0).abs() < 0.5,
                        "page_sizes は PDF 座標系（pt）: got {size:?}"
                    );
                }
                // pixel_sizes は raster_key の目標幅と一致し、縦はアスペクト比どおり
                let expected_w = data.raster_key.target_pixel_width();
                for pixels in &data.pixel_sizes {
                    assert_eq!(pixels[0], expected_w, "目標ピクセル幅どおり");
                    let expected_h = (792.0 * f64::from(expected_w) / 612.0).ceil() as u32;
                    assert!(
                        pixels[1].abs_diff(expected_h) <= 1,
                        "縦はアスペクト比どおり: got {pixels:?}, want ~{expected_h}"
                    );
                }
            }
            other => panic!("Pdf になる: {other:?}"),
        }
        std::fs::remove_dir_all(pdf_path.parent().unwrap()).ok();
    }

    /// ズームを上げると実ラスタライズ解像度が上がる（ぼやけた拡大にならない）
    #[test]
    fn pdfのズームでラスタライズ解像度が上がる() {
        if skip_unless_rasterizable() {
            return;
        }
        let pdf_path = write_test_pdf(
            "tako_pdf_zoom_test",
            "zoom.pdf",
            &["BT /F1 24 Tf 72 700 Td (Zoom) Tj ET"],
        );

        let low = rasterize_pdf(&pdf_path, PdfRasterKey::for_view(1.0, 1.0, 612.0)).unwrap();
        let high = rasterize_pdf(&pdf_path, PdfRasterKey::for_view(1.0, 3.0, 612.0)).unwrap();
        assert!(
            high.pixel_sizes[0][0] > low.pixel_sizes[0][0] * 2,
            "3 倍ズームで横解像度が伸びる: {} -> {}",
            low.pixel_sizes[0][0],
            high.pixel_sizes[0][0]
        );
        assert!(
            high.pages[0].len() > low.pages[0].len(),
            "高解像度側の PNG が大きい"
        );
        // 論理サイズ（pt）はズームに依らない
        assert_eq!(low.page_sizes[0], high.page_sizes[0]);
        std::fs::remove_dir_all(pdf_path.parent().unwrap()).ok();
    }

    /// 壊れた PDF・0 ページ PDF はパニックせず理由つきの Error になる
    #[test]
    fn 壊れたpdfと0ページpdfはエラーになる() {
        let dir = std::env::temp_dir().join("tako_pdf_broken_test");
        std::fs::create_dir_all(&dir).unwrap();

        let broken = dir.join("broken.pdf");
        std::fs::write(&broken, b"%PDF-1.4\nthis is not a valid pdf body").unwrap();
        assert!(
            matches!(
                load(&broken, PreviewMode::Pdf).content,
                PreviewContent::Error(_)
            ),
            "壊れた PDF はエラー"
        );

        let zero_pages = dir.join("zero.pdf");
        std::fs::write(&zero_pages, build_test_pdf(&[])).unwrap();
        assert!(
            matches!(
                load(&zero_pages, PreviewMode::Pdf).content,
                PreviewContent::Error(_)
            ),
            "0 ページ PDF はエラー"
        );

        let missing = dir.join("no-such.pdf");
        assert!(
            matches!(
                load(&missing, PreviewMode::Pdf).content,
                PreviewContent::Error(_)
            ),
            "不在ファイルはエラー"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// テキストレイヤ・目次・リンクは取れないプラットフォームがある（#693）。
    /// 取れない側でも Err ではなく空が返り、描画側が正常系として扱えること
    #[test]
    fn 取れない付加情報は空で返る() {
        let caps = crate::platform::pdf::capabilities();
        let pdf_path = write_test_pdf(
            "tako_pdf_caps_test",
            "caps.pdf",
            &["BT /F1 14 Tf 72 700 Td (Hello) Tj ET"],
        );
        if !caps.text_layer {
            let layers = crate::platform::pdf::extract_text_layers(&pdf_path, 1).unwrap();
            assert!(layers.iter().all(|page| page.is_empty()));
        }
        if !caps.outline {
            assert!(crate::platform::pdf::extract_outline(&pdf_path, 1)
                .unwrap()
                .is_empty());
        }
        if !caps.links {
            assert!(crate::platform::pdf::extract_links(&pdf_path, 1)
                .unwrap()
                .is_empty());
        }
        std::fs::remove_dir_all(pdf_path.parent().unwrap()).ok();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn pdfテキストレイヤ抽出() {
        // 手動構築 PDF（英語テキストのみ）で extract_text_layers が動くか。
        // T* に使う leading は TL で明示する
        let pdf_path = write_test_pdf(
            "tako_pdf_text_test",
            "test_text.pdf",
            &["BT /F1 14 Tf 14 TL 72 700 Td (Hello World) Tj T* (Second Line) Tj ET"],
        );
        let scratchpad = pdf_path.parent().unwrap().to_path_buf();

        let layers = crate::platform::pdf::extract_text_layers(&pdf_path, 1)
            .expect("PDFKit のテキスト抽出は成功する");
        assert_eq!(layers.len(), 1, "1 ページ分");
        let page = &layers[0];
        assert!(page.len() >= 2, "2 行のテキストがある: {page:?}");
        let all_text: String = page
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all_text.contains("Hello World"),
            "Hello World を含む: got {all_text:?}"
        );
        assert!(all_text.contains("Second Line"));
        // 全行の bbox と各文字矩形が非ゼロで、表示座標への変換元として使えること
        assert!(
            page.iter().all(|line| line.bbox[2] > 0.0
                && line.bbox[3] > 0.0
                && line.char_boxes.len() == line.text.chars().count()
                && line
                    .char_boxes
                    .iter()
                    .all(|char_box| char_box.bbox[2] > 0.0 && char_box.bbox[3] > 0.0)),
            "全行・全文字の bbox の幅・高さが正: {page:?}"
        );
        assert!(
            page[0]
                .char_boxes
                .iter()
                .zip(&page[1].char_boxes)
                .any(|(first, second)| first.bbox[1] != second.bbox[1]),
            "2 行の文字矩形は異なる y 座標を持つ"
        );

        std::fs::remove_dir_all(&scratchpad).ok();
    }

    #[test]
    fn pdfテキストなしでもクラッシュしない() {
        if skip_unless_rasterizable() {
            return;
        }
        // テキストレイヤのない PDF（灰色矩形のみ）
        let pdf_path = write_test_pdf(
            "tako_pdf_notext_test",
            "notext.pdf",
            &["q 0.8 0.8 0.8 rg 100 600 200 100 re f Q"],
        );

        // クラッシュせずに読めること
        let state = load(&pdf_path, PreviewMode::Pdf);
        match &state.content {
            PreviewContent::Pdf(data) => {
                assert_eq!(data.total_pages, 1);
                // テキストレイヤは空（またはテキストなし）
                let text_count: usize = data.text_layers.iter().map(|p| p.len()).sum();
                assert_eq!(text_count, 0, "テキストなし PDF ではテキスト行がゼロ");
            }
            other => panic!("Pdf になる: {other:?}"),
        }

        std::fs::remove_dir_all(pdf_path.parent().unwrap()).ok();
    }

    fn 色数(lines: &[Line]) -> usize {
        lines
            .iter()
            .flat_map(|line| line.iter())
            .filter_map(|span| span.color)
            .map(|color| (color.r, color.g, color.b))
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    #[test]
    fn 読み取りと編集で標準言語セットのシンタックスハイライトを使う() {
        let scratchpad = std::env::temp_dir().join("tako_editor_highlight_test");
        std::fs::create_dir_all(&scratchpad).ok();
        let fixtures = [
            ("sample.rs", "fn main() { let answer = 42; }\n"),
            (
                "sample.py",
                "def greet(name):\n    return f\"Hello {name}\"\n",
            ),
            (
                "sample.cpp",
                "#include <iostream>\nint main() { return 0; }\n",
            ),
            ("sample.js", "const answer = () => 42;\n"),
            ("sample.ts", "const answer: number = 42;\n"),
            (
                "sample.sh",
                "#!/bin/sh\nfor value in one two; do echo \"$value\"; done\n",
            ),
        ];

        for (name, source) in fixtures {
            let path = scratchpad.join(name);
            std::fs::write(&path, source).unwrap();

            let mut preview = load(&path, PreviewMode::Code);
            let read_colors = match &preview.content {
                PreviewContent::Code(lines) => 色数(lines),
                other => panic!("{name} の読み取り表示は Code になる: {other:?}"),
            };
            assert!(read_colors > 1, "{name} の読み取り表示に複数の構文色が付く");

            let edit = EditState::open(&preview).expect("編集を開始できる");
            apply_editor_text(&mut preview, &edit);
            let edit_colors = match &preview.content {
                PreviewContent::Code(lines) => 色数(lines),
                other => panic!("{name} の編集表示は Code になる: {other:?}"),
            };
            assert!(edit_colors > 1, "{name} の編集表示に複数の構文色が付く");
            assert_eq!(
                edit_colors, read_colors,
                "読み取りと編集で同じ構文判定を使う"
            );
        }

        std::fs::remove_dir_all(&scratchpad).ok();
    }
}

#[cfg(test)]
mod syntax_resolution_tests {
    use super::*;

    fn assert_syntax(filename: &str, expected: &str) {
        let hl = SyntectHighlighter::new();
        let path = std::path::Path::new(filename);
        let syn = hl.syntax_for_path(path, "");
        assert_eq!(
            syn.name, expected,
            "{filename} は {expected} に解決されるべき（実際: {}）",
            syn.name
        );
    }

    #[test]
    fn data_formats() {
        assert_syntax("Cargo.toml", "TOML");
        assert_syntax("Cargo.lock", "TOML");
        assert_syntax("pyproject.toml", "TOML");
        assert_syntax("test.yaml", "YAML");
        assert_syntax("test.yml", "YAML");
        assert_syntax("test.json", "JSON");
        assert_syntax("test.ini", "INI");
        assert_syntax(".env", "DotENV");
        assert_syntax("test.csv", "Separated Values");
        assert_syntax("test.xml", "XML");
    }

    #[test]
    fn systems_languages() {
        assert_syntax("test.rs", "Rust");
        assert_syntax("test.c", "C");
        assert_syntax("test.cpp", "C++");
        assert_syntax("test.go", "Go");
        assert_syntax("test.swift", "Swift");
        assert_syntax("test.kt", "Kotlin");
        assert_syntax("test.java", "Java");
    }

    #[test]
    fn web_languages() {
        assert_syntax("test.js", "JavaScript");
        assert_syntax("test.jsx", "JavaScript");
        assert_syntax("test.mjs", "JavaScript");
        assert_syntax("test.ts", "TypeScript");
        assert_syntax("test.tsx", "TypeScriptReact");
        assert_syntax("test.html", "HTML");
        assert_syntax("test.css", "CSS");
        assert_syntax("test.php", "PHP");
    }

    #[test]
    fn scripting_languages() {
        assert_syntax("test.py", "Python");
        assert_syntax("test.rb", "Ruby");
        assert_syntax("test.lua", "Lua");
        assert_syntax("test.sh", "Bourne Again Shell (bash)");
    }

    #[test]
    fn build_and_config_files() {
        assert_syntax("Dockerfile", "Dockerfile");
        assert_syntax("Makefile", "Makefile");
        assert_syntax("CMakeLists.txt", "CMake");
        assert_syntax("test.cmake", "CMake");
        assert_syntax(".gitignore", "Git Ignore");
        assert_syntax(".editorconfig", "INI");
        assert_syntax("test.sql", "SQL");
        assert_syntax("test.diff", "Diff");
        assert_syntax("test.md", "Markdown");
    }

    #[test]
    fn filename_based_fallbacks() {
        assert_syntax("Cargo.lock", "TOML");
        assert_syntax("Pipfile", "TOML");
        assert_syntax(".gitattributes", "Git Attributes");
        assert_syntax(".dockerignore", "Git Ignore");
        assert_syntax(".eslintrc", "JSON");
        assert_syntax("Justfile", "Makefile");
    }

    #[test]
    fn shebang_detection() {
        let hl = SyntectHighlighter::new();
        let path = std::path::Path::new("script");
        let syn = hl.syntax_for_path(path, "#!/bin/bash\necho hello");
        assert_ne!(syn.name, "Plain Text", "shebang で構文が特定される");
    }

    /// #815 で構文セットの寿命に手を入れたので、#320 の対応言語が縮退していないことを
    /// **登録済み拡張子の全数**で押さえる（軽い既定セットへ落とす案を採ると必ず落ちる）
    #[test]
    fn 登録済み拡張子は全数が構文へ解決する() {
        let hl = SyntectHighlighter::new();
        let mut checked = 0usize;
        let mut missing: Vec<String> = Vec::new();
        for syntax in hl.syntaxes.syntaxes() {
            for ext in &syntax.file_extensions {
                checked += 1;
                if hl.syntaxes.find_syntax_by_extension(ext).is_none() {
                    missing.push(ext.clone());
                }
            }
        }
        assert!(missing.is_empty(), "解決できない拡張子: {missing:?}");
        // two-face（bat 由来）の規模。syntect 同梱の既定セットは 75 構文 / 拡張子も
        // 半分以下なので、セットを差し替えたらこの下限で気づける
        assert!(
            hl.syntaxes.syntaxes().len() >= 200,
            "構文数が縮退している: {}",
            hl.syntaxes.syntaxes().len()
        );
        assert!(checked >= 550, "拡張子の登録数が縮退している: {checked}");
    }
}

/// Issue #815: 構文セットの寿命（使っている間だけ載せ、使い終われば手放す）。
///
/// グローバルは並列テストで他のテストと衝突するため、判定はローカルの
/// [`SyntaxCache`] に対して行う（同じ実装をグローバルが薄く包んでいる）。
#[cfg(test)]
mod syntax_lifetime_tests {
    use super::*;

    #[test]
    fn 解放判定はプレビューの有無と猶予で決まる() {
        // プレビューが 1 枚も無ければ猶予を待たない
        assert!(syntax_release_due(Duration::ZERO, false));
        // 開いている間は猶予まで保持する（編集の連続打鍵で毎回ロードし直さない）
        assert!(!syntax_release_due(Duration::ZERO, true));
        assert!(!syntax_release_due(
            SYNTAX_IDLE_GRACE - Duration::from_millis(1),
            true
        ));
        assert!(syntax_release_due(SYNTAX_IDLE_GRACE, true));
    }

    #[test]
    fn 借用中は保持を手放しても解放されない() {
        let mut cache = SyntaxCache::new();
        let now = Instant::now();
        let lease = cache.acquire(now);
        assert!(cache.resident(), "借りた直後は載っている");

        // プレビュー 0 枚 = 即時解放の条件でも、借用が生きている間は解放されない
        assert!(cache.release_idle(now, false), "保持は手放す");
        assert!(
            cache.resident(),
            "借用チケットが生きている間は構文セットが残る（background ハイライトの足元）"
        );

        // ハイライトは借用中ずっと成立する
        let lines = lease.highlight(Path::new("a.rs"), "fn main() {}\n");
        assert_eq!(lines.len(), 1);

        drop(lease);
        assert!(!cache.resident(), "最後の借用が落ちた時点で解放される");
    }

    #[test]
    fn 猶予内は載せたまま_猶予を過ぎたら手放す() {
        let mut cache = SyntaxCache::new();
        let now = Instant::now();
        drop(cache.acquire(now));
        assert!(cache.resident());

        // プレビューを開いたまま・猶予内 = 保持（再ハイライトが速い）
        assert!(!cache.release_idle(now + Duration::from_secs(1), true));
        assert!(cache.resident());

        // 猶予を過ぎたら開いたままでも手放す（借用は無いので即解放）
        assert!(cache.release_idle(now + SYNTAX_IDLE_GRACE, true));
        assert!(!cache.resident());
        // 2 回目は「もう手放している」ので false（tick が毎回仕事をしない）
        assert!(!cache.release_idle(now + SYNTAX_IDLE_GRACE, true));
    }

    #[test]
    fn 解放後に借り直しても同じ色が出る() {
        let mut cache = SyntaxCache::new();
        let now = Instant::now();
        let sample = "fn main() {\n    let x = 1; // コメント\n}\n";

        let before = cache.acquire(now).highlight(Path::new("sample.rs"), sample);
        assert!(cache.release_idle(now, false));
        assert!(!cache.resident(), "解放されている");

        let after = cache.acquire(now).highlight(Path::new("sample.rs"), sample);
        assert!(cache.resident(), "借り直しでロードし直される");
        assert_eq!(before, after, "再ロード後も色・区切りが一致する");
        // 色が実際に付いていること（両方とも素のテキストでは検査にならない）
        assert!(
            before.iter().flatten().any(|s| s.color.is_some()),
            "ハイライトの色が付いている"
        );
    }

    #[test]
    fn 借用が重なっても構文セットは1つ() {
        let mut cache = SyntaxCache::new();
        let now = Instant::now();
        let a = cache.acquire(now);
        let b = cache.acquire(now);
        assert!(
            Arc::ptr_eq(&a.0, &b.0),
            "同時に借りたチケットは同じ構文セットを指す（二重ロードしない）"
        );
        drop(a);
        assert!(cache.resident(), "1 枚残っていれば載ったまま");
        drop(b);
        assert!(cache.release_idle(now, false));
        assert!(!cache.resident());
    }

    #[test]
    fn グローバル経路も借用と解放が成立する() {
        // グローバルは他のテストと共有なので、「借りたら載る」だけを見る
        // （解放の判定はローカル cache 側のテストが担う）
        let lease = highlighter();
        assert!(syntax_resident(), "借りている間は載っている");
        let lines = lease.highlight(Path::new("g.rs"), "fn main() {}\n");
        assert_eq!(lines.len(), 1);
    }
}

/// Issue #669: コードプレビューの構文色がライトテーマの面で読めること。
///
/// サンプル値ではなく**実ハイライタの出力を全走査**して検証する（構文セットや
/// syntect の既定テーマが変わっても穴が空かないように）。
#[cfg(test)]
mod light_theme_contrast_tests {
    use super::*;
    use tako_core::theme::{Theme, ThemeMode};

    const RUST_SAMPLE: &str = r#"use std::collections::HashMap;

/// ドキュメントコメント
pub fn main() -> Result<(), String> {
    let mut map: HashMap<&str, u32> = HashMap::new();
    map.insert("answer", 42);
    if let Some(v) = map.get("answer") {
        println!("{v} {:?}", 3.14_f32);
    }
    Ok(())
}
"#;

    const PYTHON_SAMPLE: &str = r#"import os
from typing import Optional

# コメント
class Greeter:
    """docstring"""

    def __init__(self, name: str = "world") -> None:
        self.name = name

    def greet(self) -> Optional[str]:
        return f"hello {self.name}" if os.environ.get("OK") else None
"#;

    const CPP_SAMPLE: &str = r#"#include <string>
#include <vector>

// コメント
namespace demo {
template <typename T>
class Box {
 public:
  explicit Box(T value) : value_(std::move(value)) {}
  const T& get() const noexcept { return value_; }

 private:
  T value_;
};
}  // namespace demo
"#;

    /// ハイライト結果に現れる構文色をすべて集める
    fn syntax_colors(filename: &str, text: &str) -> Vec<tako_core::Rgb> {
        let hl = SyntectHighlighter::new();
        let mut colors: Vec<tako_core::Rgb> = hl
            .highlight(Path::new(filename), text)
            .iter()
            .flat_map(|line| line.iter())
            .filter_map(|span| span.color)
            .collect();
        colors.sort_by_key(|c| (c.r, c.g, c.b));
        colors.dedup();
        colors
    }

    const SAMPLES: [(&str, &str); 3] = [
        ("sample.rs", RUST_SAMPLE),
        ("sample.py", PYTHON_SAMPLE),
        ("sample.cpp", CPP_SAMPLE),
    ];

    /// 受け入れ条件 1: ライトの実描画面（コードプレビュー本体 = `background`、
    /// Markdown のコードブロック = `mantle`）に対して全構文色が 4.5:1 以上
    #[test]
    fn 代表言語の構文色がライトの実描画面で読める明度になる() {
        let light = Theme::for_mode(ThemeMode::Light);
        for (name, text) in SAMPLES {
            let colors = syntax_colors(name, text);
            assert!(colors.len() > 1, "{name} に複数の構文色が付く");
            for (surface_name, surface) in
                [("background", light.background), ("mantle", light.mantle)]
            {
                for raw in &colors {
                    let after = light.adapt_syntax_color(*raw).contrast_ratio(surface);
                    assert!(
                        after >= 4.5,
                        "{name} の {} が {surface_name} で AA 未達: 変換前 {:.2}:1 → 変換後 {after:.2}:1",
                        raw.to_hex(),
                        raw.contrast_ratio(surface)
                    );
                }
            }
        }
    }

    /// 受け入れ条件 2: ダークテーマの色は 1 ビットも変わらない
    #[test]
    fn ダークテーマの構文色は変換されない() {
        let dark = Theme::for_mode(ThemeMode::Dark);
        for (name, text) in SAMPLES {
            for raw in syntax_colors(name, text) {
                assert_eq!(
                    dark.adapt_syntax_color(raw),
                    raw,
                    "{name} の {} がダークで変換された",
                    raw.to_hex()
                );
            }
        }
    }

    /// エッジ: 構文情報が無い入力（プレーンテキスト / 不明拡張子 / 空ファイル）でも
    /// 変換が破綻せず、色が付くならライトで読めること
    #[test]
    fn 構文情報のない入力でも破綻しない() {
        let light = Theme::for_mode(ThemeMode::Light);
        for (name, text) in [
            ("notes.txt", "ただの平文\nsecond line\n"),
            ("data.unknownext", "no syntax for this\n"),
            ("empty.rs", ""),
            ("empty.txt", ""),
        ] {
            let lines = SyntectHighlighter::new().highlight(Path::new(name), text);
            // 空ファイルは 0 行、平文は行数どおり（表示が欠けない）
            assert_eq!(lines.len(), text.lines().count(), "{name} の行数");
            for raw in syntax_colors(name, text) {
                let after = light
                    .adapt_syntax_color(raw)
                    .contrast_ratio(light.background);
                assert!(
                    after >= 4.5,
                    "{name} の {} が AA 未達: {after:.2}:1",
                    raw.to_hex()
                );
            }
        }
    }
    // --- #680: リンクの遷移先保持とコードブロックのコピー本文 ---

    /// パースしたブロック列から (行内テキスト, リンク範囲) を取り出す小道具
    fn spans_of(md: &str) -> Vec<MdSpan> {
        let blocks = markdown_blocks(md);
        blocks
            .into_iter()
            .flat_map(|b| match b.kind {
                MdBlockKind::Heading { spans, .. }
                | MdBlockKind::Paragraph { spans }
                | MdBlockKind::ListItem { spans, .. } => spans,
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn リンクは遷移先を保持する() {
        let spans = spans_of("見て [tako](https://github.com/takushio2525/tako) ね\n");
        let link: Vec<_> = spans.iter().filter(|s| s.is_link()).collect();
        assert_eq!(link.len(), 1, "リンクスパンは 1 つ");
        assert_eq!(link[0].text, "tako");
        assert_eq!(
            link[0].link_url.as_deref(),
            Some("https://github.com/takushio2525/tako")
        );
        // リンク外のスパンには遷移先が漏れない
        assert!(spans
            .iter()
            .filter(|s| !s.is_link())
            .all(|s| s.link_url.is_none()));
    }

    #[test]
    fn リンク範囲は連結テキスト上のバイト範囲になる() {
        let spans = spans_of("あ [tako](https://example.com) い\n");
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        let ranges = md_link_ranges(&spans);
        assert_eq!(ranges.len(), 1);
        let (range, url) = &ranges[0];
        assert_eq!(url, "https://example.com");
        // 日本語混在でもバイト範囲がリンク文字列そのものを指す
        assert_eq!(&text[range.clone()], "tako");
    }

    #[test]
    fn リンク内の装飾は一本の範囲へ束ねる() {
        let spans = spans_of("[**太字**と普通](https://example.com)\n");
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        // 装飾で 2 スパンに割れている前提を明示（束ねの意味がある入力か）
        assert!(spans.iter().filter(|s| s.is_link()).count() >= 2);
        let ranges = md_link_ranges(&spans);
        assert_eq!(ranges.len(), 1, "同じ遷移先の隣接スパンは 1 本になる");
        assert_eq!(&text[ranges[0].0.clone()], "太字と普通");
    }

    #[test]
    fn 隣接する別リンクは別範囲になる() {
        let spans = spans_of("[a](https://a.example)[b](https://b.example)\n");
        let ranges = md_link_ranges(&spans);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].1, "https://a.example");
        assert_eq!(ranges[1].1, "https://b.example");
        assert_eq!(
            ranges[0].0.end, ranges[1].0.start,
            "範囲は隣接して重ならない"
        );
    }

    #[test]
    fn 相対パスやアンカーも遷移先として保持する() {
        // 保持はする（一覧に出す）。開くかどうかは md_links::browser_url が決める
        let spans = spans_of("[章へ](#section) [ファイル](./doc/readme.md)\n");
        let urls: Vec<_> = spans.iter().filter_map(|s| s.link_url.as_deref()).collect();
        assert_eq!(urls, vec!["#section", "./doc/readme.md"]);
        assert!(urls
            .iter()
            .all(|u| tako_core::md_links::browser_url(u).is_none()));
    }

    #[test]
    fn リンクの無い文書では範囲が空() {
        let spans = spans_of("ただの段落 `code` **強調**\n");
        assert!(md_link_ranges(&spans).is_empty());
    }

    #[test]
    fn コードブロックのコピー本文は空行とインデントを保つ() {
        let md = "```python\ndef f():\n    return 1\n\n\nprint(f())\n```\n";
        let blocks = markdown_blocks(md);
        let MdBlockKind::CodeBlock { lines, .. } = &blocks[0].kind else {
            panic!("コードブロックがある: {blocks:?}");
        };
        assert_eq!(
            md_code_block_text(lines),
            "def f():\n    return 1\n\n\nprint(f())"
        );
    }

    #[test]
    fn 空のコードブロックのコピー本文は空文字列() {
        let blocks = markdown_blocks("```\n```\n");
        let MdBlockKind::CodeBlock { lines, .. } = &blocks[0].kind else {
            panic!("コードブロックがある: {blocks:?}");
        };
        assert_eq!(md_code_block_text(lines), "");
    }

    #[test]
    fn コードブロック内のリンク風文字列はリンクにならない() {
        // ``` の中は素のテキスト = ⌘+クリックの対象にしない（コピー対象のみ）
        let blocks = markdown_blocks("```\nsee [x](https://example.com)\n```\n");
        let MdBlockKind::CodeBlock { lines, .. } = &blocks[0].kind else {
            panic!("コードブロックがある");
        };
        assert_eq!(md_code_block_text(lines), "see [x](https://example.com)");
    }

    #[test]
    fn 表セル内のリンクも範囲が取れる() {
        let md = "| 名前 | 先 |\n| --- | --- |\n| tako | [repo](https://example.com/r) |\n";
        let blocks = markdown_blocks(md);
        let MdBlockKind::Table { rows, .. } = &blocks[0].kind else {
            panic!("表がある: {blocks:?}");
        };
        let cell = &rows[0][1];
        let ranges = md_link_ranges(cell);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].1, "https://example.com/r");
        let text: String = cell.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(&text[ranges[0].0.clone()], "repo");
    }
}
