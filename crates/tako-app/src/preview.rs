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
use std::sync::OnceLock;

use tako_control::protocol::PreviewModeWire;
use tako_core::{SearchHit, TextBuffer};

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
    /// リンクテキスト（アクセント色で描く。URL 自体は開かない = Web ペインは FR-3.8）
    pub link: bool,
}

/// Markdown のブロック（描画単位。FR-3.3）
#[derive(Debug, Clone, PartialEq)]
pub enum MdBlock {
    Heading {
        level: u8,
        spans: Vec<MdSpan>,
    },
    Paragraph {
        spans: Vec<MdSpan>,
    },
    /// リスト項目。`marker` は "•" / "1." 等、`depth` はネスト段
    ListItem {
        depth: usize,
        marker: String,
        spans: Vec<MdSpan>,
    },
    /// コードブロック（```lang はハイライトして保持する）
    CodeBlock {
        lines: Vec<Line>,
    },
    Quote {
        spans: Vec<MdSpan>,
    },
    Rule,
}

/// 画像データ（生バイトを保持。GPUI 側で Image::from_bytes してデコードする）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub format: ImageFileFormat,
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

/// PDF データ（全ページの PNG を保持し、スクロールで閲覧）
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

/// プレビューペイン 1 枚分の状態（`TakoApp::previews` の値）
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewState {
    pub path: PathBuf,
    pub mode: PreviewMode,
    pub content: PreviewContent,
    /// 上限超過で切り詰めたか（フッタで明示する）
    pub truncated: bool,
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
    preview.truncated = false;
}

impl PreviewState {
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
            truncated: false,
        }
    }

    /// background 読み込み中のプレースホルダ（Issue #168）
    pub fn loading(path: &Path, mode: PreviewMode) -> Self {
        Self {
            path: path.to_path_buf(),
            mode,
            content: PreviewContent::Loading,
            truncated: false,
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
        None => return PreviewState::error(path, PreviewMode::Image, "対応していない画像形式"),
    };
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() > MAX_IMAGE_BYTES => PreviewState::error(
            path,
            PreviewMode::Image,
            format!(
                "画像が大きすぎる（{:.1} MB、上限 50 MB）",
                bytes.len() as f64 / 1_000_000.0
            ),
        ),
        Ok(bytes) => PreviewState {
            path: path.to_path_buf(),
            mode: PreviewMode::Image,
            content: PreviewContent::Image(ImageData { bytes, format }),
            truncated: false,
        },
        Err(e) => PreviewState::error(path, PreviewMode::Image, format!("読み込めない: {e}")),
    }
}

/// PDF の全ページをレンダリングして PreviewState を返す。
/// Core Graphics FFI で描画する（macOS のみ）
pub fn load_pdf(path: &Path, _page: usize) -> PreviewState {
    load_pdf_with_key(path, PdfRasterKey::for_view(2.0, 1.0, 612.0))
}

/// 指定した表示条件で PDF を読み込む。全処理は呼び出し側が background へ載せる。
pub fn load_pdf_with_key(path: &Path, raster_key: PdfRasterKey) -> PreviewState {
    #[cfg(target_os = "macos")]
    {
        match rasterize_pdf(path, raster_key) {
            Ok(rasterized) => {
                let text_layers = pdf_render::extract_text_layers(path, rasterized.total_pages)
                    .unwrap_or_default();
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
                    }),
                    truncated: false,
                }
            }
            Err(e) => PreviewState::error(path, PreviewMode::Pdf, e),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = raster_key;
        PreviewState::error(path, PreviewMode::Pdf, "PDF プレビューは macOS のみ対応")
    }
}

/// PDF ページ画像だけを再生成する。テキスト抽出は初回ロード時だけ行う。
pub fn rasterize_pdf(path: &Path, raster_key: PdfRasterKey) -> Result<PdfRasterizedPages, String> {
    #[cfg(target_os = "macos")]
    {
        let _span = tako_control::diag::perf_span("pdf_rasterize");
        pdf_render::render_all_pages(path, raster_key)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, raster_key);
        Err("PDF プレビューは macOS のみ対応".into())
    }
}

/// 動画ファイルのプレビュー読み込み（ffmpeg でサムネイル抽出 + メタ情報取得）
pub fn load_video(path: &Path) -> PreviewState {
    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            return PreviewState::error(path, PreviewMode::Video, format!("読み込めない: {e}"));
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
        truncated: false,
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
    if std::process::Command::new(name)
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
    let output = std::process::Command::new(ffprobe_bin())
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
    let output = std::process::Command::new(ffmpeg_bin())
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

/// macOS Core Graphics を使った PDF ページレンダリング
#[cfg(target_os = "macos")]
mod pdf_render {
    use std::path::Path;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPDFDocumentCreateWithURL(url: *const core::ffi::c_void) -> *const core::ffi::c_void;
        fn CGPDFDocumentRelease(document: *const core::ffi::c_void);
        fn CGPDFDocumentGetNumberOfPages(document: *const core::ffi::c_void) -> usize;
        fn CGPDFDocumentGetPage(
            document: *const core::ffi::c_void,
            page_number: usize,
        ) -> *const core::ffi::c_void;
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    // kCGPDFMediaBox = 0
    const CG_PDF_MEDIA_BOX: i32 = 0;

    extern "C" {
        fn CGPDFPageGetBoxRect(page: *const core::ffi::c_void, box_type: i32) -> CGRect;
        fn CGColorSpaceCreateDeviceRGB() -> *const core::ffi::c_void;
        fn CGColorSpaceRelease(space: *const core::ffi::c_void);
        fn CGBitmapContextCreate(
            data: *mut u8,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *const core::ffi::c_void,
            bitmap_info: u32,
        ) -> *const core::ffi::c_void;
        fn CGContextRelease(context: *const core::ffi::c_void);
        fn CGContextSetRGBFillColor(
            context: *const core::ffi::c_void,
            red: f64,
            green: f64,
            blue: f64,
            alpha: f64,
        );
        fn CGContextFillRect(context: *const core::ffi::c_void, rect: CGRect);
        fn CGContextScaleCTM(context: *const core::ffi::c_void, sx: f64, sy: f64);
        fn CGContextDrawPDFPage(context: *const core::ffi::c_void, page: *const core::ffi::c_void);
        fn CGBitmapContextCreateImage(
            context: *const core::ffi::c_void,
        ) -> *const core::ffi::c_void;
        fn CGImageRelease(image: *const core::ffi::c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFURLCreateWithFileSystemPath(
            allocator: *const core::ffi::c_void,
            file_path: *const core::ffi::c_void,
            path_style: isize,
            is_directory: bool,
        ) -> *const core::ffi::c_void;
        fn CFRelease(cf: *const core::ffi::c_void);
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageDestinationCreateWithData(
            data: *const core::ffi::c_void,
            image_type: *const core::ffi::c_void,
            count: usize,
            options: *const core::ffi::c_void,
        ) -> *const core::ffi::c_void;
        fn CGImageDestinationAddImage(
            dest: *const core::ffi::c_void,
            image: *const core::ffi::c_void,
            properties: *const core::ffi::c_void,
        );
        fn CGImageDestinationFinalize(dest: *const core::ffi::c_void) -> bool;
    }

    extern "C" {
        fn CFDataCreateMutable(
            allocator: *const core::ffi::c_void,
            capacity: isize,
        ) -> *const core::ffi::c_void;
        fn CFDataGetBytePtr(data: *const core::ffi::c_void) -> *const u8;
        fn CFDataGetLength(data: *const core::ffi::c_void) -> isize;
    }

    extern "C" {
        fn CFStringCreateWithBytes(
            allocator: *const core::ffi::c_void,
            bytes: *const u8,
            num_bytes: isize,
            encoding: u32,
            is_external: bool,
        ) -> *const core::ffi::c_void;
    }

    // kCFStringEncodingUTF8 = 0x08000100
    const CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
    // kCFURLPOSIXPathStyle = 0
    const CF_URL_POSIX_PATH_STYLE: isize = 0;
    // kCGImageAlphaPremultipliedLast = 1 (RGBA with premultiplied alpha)
    const CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;

    fn make_cfstring(s: &str) -> *const core::ffi::c_void {
        unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                s.as_ptr(),
                s.len() as isize,
                CF_STRING_ENCODING_UTF8,
                false,
            )
        }
    }

    pub fn render_all_pages(
        path: &Path,
        raster_key: super::PdfRasterKey,
    ) -> Result<super::PdfRasterizedPages, String> {
        let path_str = path
            .to_str()
            .ok_or_else(|| "パスが UTF-8 でない".to_string())?;
        unsafe {
            let cf_path = make_cfstring(path_str);
            if cf_path.is_null() {
                return Err("CFString 生成失敗".into());
            }
            let url = CFURLCreateWithFileSystemPath(
                std::ptr::null(),
                cf_path,
                CF_URL_POSIX_PATH_STYLE,
                false,
            );
            CFRelease(cf_path);
            if url.is_null() {
                return Err("CFURL 生成失敗".into());
            }

            let doc = CGPDFDocumentCreateWithURL(url);
            CFRelease(url);
            if doc.is_null() {
                return Err("PDF を開けない".into());
            }

            let total = CGPDFDocumentGetNumberOfPages(doc);
            if total == 0 {
                CGPDFDocumentRelease(doc);
                return Err("PDF にページがない".into());
            }

            let mut all_pages = Vec::with_capacity(total);
            let mut page_sizes = Vec::with_capacity(total);
            let mut pixel_sizes = Vec::with_capacity(total);
            for page_idx in 0..total {
                let page_num = page_idx + 1;
                let pdf_page = CGPDFDocumentGetPage(doc, page_num);
                if pdf_page.is_null() {
                    all_pages.push(Vec::new());
                    page_sizes.push([0.0, 0.0]);
                    pixel_sizes.push([0, 0]);
                    continue;
                }

                let media_box = CGPDFPageGetBoxRect(pdf_page, CG_PDF_MEDIA_BOX);
                page_sizes.push([media_box.size.width, media_box.size.height]);
                let pixel_w = raster_key.target_pixel_width() as usize;
                let render_scale = pixel_w as f64 / media_box.size.width.max(1.0);
                let pixel_h = (media_box.size.height * render_scale).ceil() as usize;
                pixel_sizes.push([pixel_w as u32, pixel_h as u32]);
                if pixel_w == 0 || pixel_h == 0 {
                    all_pages.push(Vec::new());
                    continue;
                }

                let bytes_per_row = pixel_w * 4;
                let mut buffer = vec![0u8; bytes_per_row * pixel_h];
                let color_space = CGColorSpaceCreateDeviceRGB();
                let ctx = CGBitmapContextCreate(
                    buffer.as_mut_ptr(),
                    pixel_w,
                    pixel_h,
                    8,
                    bytes_per_row,
                    color_space,
                    CG_IMAGE_ALPHA_PREMULTIPLIED_LAST,
                );
                CGColorSpaceRelease(color_space);
                if ctx.is_null() {
                    all_pages.push(Vec::new());
                    continue;
                }

                CGContextSetRGBFillColor(ctx, 1.0, 1.0, 1.0, 1.0);
                CGContextFillRect(
                    ctx,
                    CGRect {
                        origin: CGPoint { x: 0.0, y: 0.0 },
                        size: CGSize {
                            width: pixel_w as f64,
                            height: pixel_h as f64,
                        },
                    },
                );

                CGContextScaleCTM(ctx, render_scale, render_scale);
                CGContextDrawPDFPage(ctx, pdf_page);

                let cg_image = CGBitmapContextCreateImage(ctx);
                CGContextRelease(ctx);

                if cg_image.is_null() {
                    all_pages.push(Vec::new());
                    continue;
                }

                let png_data = cgimage_to_png(cg_image);
                CGImageRelease(cg_image);

                all_pages.push(png_data.unwrap_or_default());
            }

            CGPDFDocumentRelease(doc);
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
            Ok(super::PdfRasterizedPages {
                pages: all_pages,
                total_pages: total,
                page_sizes,
                pixel_sizes,
            })
        }
    }

    unsafe fn cgimage_to_png(image: *const core::ffi::c_void) -> Option<Vec<u8>> {
        let png_uti = make_cfstring("public.png");
        let mutable_data = CFDataCreateMutable(std::ptr::null(), 0);
        if mutable_data.is_null() {
            CFRelease(png_uti);
            return None;
        }
        let dest = CGImageDestinationCreateWithData(mutable_data, png_uti, 1, std::ptr::null());
        CFRelease(png_uti);
        if dest.is_null() {
            CFRelease(mutable_data);
            return None;
        }

        CGImageDestinationAddImage(dest, image, std::ptr::null());
        let ok = CGImageDestinationFinalize(dest);
        CFRelease(dest);

        if !ok {
            CFRelease(mutable_data);
            return None;
        }

        let ptr = CFDataGetBytePtr(mutable_data);
        let len = CFDataGetLength(mutable_data) as usize;
        let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
        CFRelease(mutable_data);
        Some(bytes)
    }

    // --- PDFKit FFI（テキストレイヤ抽出） ---

    // クラス名を Objective-C runtime から引くだけでは PDFKit がロードされる保証がない。
    // 明示リンクしないと `objc_getClass("PDFDocument")` が null になり、テキストレイヤが
    // 常に空へ劣化する（ページ画像は CoreGraphics 側なので表示だけは成功してしまう）。
    #[link(name = "PDFKit", kind = "framework")]
    extern "C" {}

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const u8) -> *const core::ffi::c_void;
        fn sel_registerName(name: *const u8) -> *const core::ffi::c_void;
        fn objc_msgSend(
            receiver: *const core::ffi::c_void,
            selector: *const core::ffi::c_void,
            ...
        ) -> *const core::ffi::c_void;
    }

    fn cls(name: &str) -> *const core::ffi::c_void {
        let cstr = std::ffi::CString::new(name).unwrap();
        unsafe { objc_getClass(cstr.as_ptr() as *const u8) }
    }

    fn sel_name(name: &str) -> *const core::ffi::c_void {
        let cstr = std::ffi::CString::new(name).unwrap();
        unsafe { sel_registerName(cstr.as_ptr() as *const u8) }
    }

    unsafe fn msg_no_arg(
        receiver: *const core::ffi::c_void,
        sel: *const core::ffi::c_void,
    ) -> *const core::ffi::c_void {
        objc_msgSend(receiver, sel)
    }

    unsafe fn msg_id(
        receiver: *const core::ffi::c_void,
        sel: *const core::ffi::c_void,
        arg: *const core::ffi::c_void,
    ) -> *const core::ffi::c_void {
        let f: unsafe extern "C" fn(
            *const core::ffi::c_void,
            *const core::ffi::c_void,
            *const core::ffi::c_void,
        ) -> *const core::ffi::c_void =
            std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
        f(receiver, sel, arg)
    }

    unsafe fn msg_usize(
        receiver: *const core::ffi::c_void,
        sel: *const core::ffi::c_void,
        arg: usize,
    ) -> *const core::ffi::c_void {
        let f: unsafe extern "C" fn(
            *const core::ffi::c_void,
            *const core::ffi::c_void,
            usize,
        ) -> *const core::ffi::c_void =
            std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
        f(receiver, sel, arg)
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct NSRange {
        location: usize,
        length: usize,
    }

    unsafe fn msg_nsrange(
        receiver: *const core::ffi::c_void,
        sel: *const core::ffi::c_void,
        range: NSRange,
    ) -> *const core::ffi::c_void {
        let f: unsafe extern "C" fn(
            *const core::ffi::c_void,
            *const core::ffi::c_void,
            NSRange,
        ) -> *const core::ffi::c_void =
            std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
        f(receiver, sel, range)
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct NSRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    // ARM64: NSRect (32 bytes) は GPR に収まらないため objc_msgSend_stret が必要…
    // ただし ARM64 では objc_msgSend_stret は存在せず、objc_msgSend が直接返す
    // （ABI 規約: 16 bytes 超の構造体は x8 レジスタ経由で間接リターン）
    #[cfg(target_arch = "aarch64")]
    unsafe fn msg_bounds_for_page(
        selection: *const core::ffi::c_void,
        page: *const core::ffi::c_void,
    ) -> NSRect {
        let f: unsafe extern "C" fn(
            *const core::ffi::c_void,
            *const core::ffi::c_void,
            *const core::ffi::c_void,
        ) -> NSRect = std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
        f(selection, sel_name("boundsForPage:"), page)
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn msg_bounds_for_page(
        selection: *const core::ffi::c_void,
        page: *const core::ffi::c_void,
    ) -> NSRect {
        extern "C" {
            fn objc_msgSend_stret(
                ret: *mut NSRect,
                receiver: *const core::ffi::c_void,
                sel: *const core::ffi::c_void,
                arg: *const core::ffi::c_void,
            );
        }
        let mut result = NSRect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        objc_msgSend_stret(&mut result, selection, sel_name("boundsForPage:"), page);
        result
    }

    unsafe fn nsstring_to_rust(nsstr: *const core::ffi::c_void) -> Option<String> {
        if nsstr.is_null() {
            return None;
        }
        let utf8_sel = sel_name("UTF8String");
        let cstr_ptr = msg_no_arg(nsstr, utf8_sel) as *const i8;
        if cstr_ptr.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(cstr_ptr)
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// PDFKit を使ってテキストレイヤを抽出する。
    /// 各ページのテキストを行に分割し、行ごとの PDF 座標バウンディングボックスを取得する。
    pub fn extract_text_layers(
        path: &Path,
        total_pages: usize,
    ) -> Result<Vec<Vec<super::PdfTextLine>>, String> {
        let path_str = path
            .to_str()
            .ok_or_else(|| "パスが UTF-8 でない".to_string())?;
        unsafe {
            // NSURL.fileURLWithPath:
            let ns_path = make_cfstring(path_str);
            if ns_path.is_null() {
                return Err("CFString 生成失敗".into());
            }
            let nsurl = msg_id(cls("NSURL"), sel_name("fileURLWithPath:"), ns_path);
            CFRelease(ns_path);
            if nsurl.is_null() {
                return Err("NSURL 生成失敗".into());
            }

            // PDFDocument.alloc.initWithURL:
            let pdf_doc_alloc = msg_no_arg(cls("PDFDocument"), sel_name("alloc"));
            if pdf_doc_alloc.is_null() {
                return Err("PDFDocument alloc 失敗".into());
            }
            let pdf_doc = msg_id(pdf_doc_alloc, sel_name("initWithURL:"), nsurl);
            if pdf_doc.is_null() {
                return Err("PDFDocument initWithURL: 失敗".into());
            }

            let mut result = Vec::with_capacity(total_pages);
            for page_idx in 0..total_pages {
                let page = msg_usize(pdf_doc, sel_name("pageAtIndex:"), page_idx);
                if page.is_null() {
                    result.push(Vec::new());
                    continue;
                }

                // ページ全体のテキストを取得
                let ns_string = msg_no_arg(page, sel_name("string"));
                let full_text = nsstring_to_rust(ns_string).unwrap_or_default();
                if full_text.is_empty() {
                    result.push(Vec::new());
                    continue;
                }

                // 行に分割して各行のバウンディングボックスを取得
                let mut lines = Vec::new();
                let mut char_offset: usize = 0;
                for line_text in full_text.split('\n') {
                    let line_len = line_text.len();
                    if line_len == 0 {
                        lines.push(super::PdfTextLine {
                            text: String::new(),
                            bbox: [0.0, 0.0, 0.0, 0.0],
                            char_boxes: Vec::new(),
                        });
                        char_offset += 1; // '\n'
                        continue;
                    }

                    // NSString は UTF-16 なので、Rust の byte offset → UTF-16 offset に変換
                    let before = &full_text[..char_offset];
                    let utf16_start: usize = before.encode_utf16().count();
                    let utf16_len: usize = line_text.encode_utf16().count();

                    if utf16_len > 0 {
                        let range = NSRange {
                            location: utf16_start,
                            length: utf16_len,
                        };
                        let selection = msg_nsrange(page, sel_name("selectionForRange:"), range);
                        if !selection.is_null() {
                            let bounds = msg_bounds_for_page(selection, page);
                            let mut char_boxes = Vec::new();
                            let mut utf16_char_offset = 0usize;
                            for (byte_start, ch) in line_text.char_indices() {
                                let char_range = NSRange {
                                    location: utf16_start + utf16_char_offset,
                                    length: ch.len_utf16(),
                                };
                                let char_selection =
                                    msg_nsrange(page, sel_name("selectionForRange:"), char_range);
                                if !char_selection.is_null() {
                                    let char_bounds = msg_bounds_for_page(char_selection, page);
                                    char_boxes.push(super::PdfCharBox {
                                        byte_range: byte_start..byte_start + ch.len_utf8(),
                                        bbox: [
                                            char_bounds.x,
                                            char_bounds.y,
                                            char_bounds.w,
                                            char_bounds.h,
                                        ],
                                    });
                                }
                                utf16_char_offset += ch.len_utf16();
                            }
                            lines.push(super::PdfTextLine {
                                text: line_text.to_string(),
                                bbox: [bounds.x, bounds.y, bounds.w, bounds.h],
                                char_boxes,
                            });
                        } else {
                            lines.push(super::PdfTextLine {
                                text: line_text.to_string(),
                                bbox: [0.0, 0.0, 0.0, 0.0],
                                char_boxes: Vec::new(),
                            });
                        }
                    } else {
                        lines.push(super::PdfTextLine {
                            text: line_text.to_string(),
                            bbox: [0.0, 0.0, 0.0, 0.0],
                            char_boxes: Vec::new(),
                        });
                    }
                    char_offset += line_len + 1; // +1 for '\n'
                }
                result.push(lines);
            }

            // PDFDocument は autorelease pool で管理されるので明示 release
            msg_no_arg(pdf_doc, sel_name("release"));
            Ok(result)
        }
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
    let content = match mode {
        PreviewMode::Markdown => PreviewContent::Markdown(markdown_blocks(&text)),
        PreviewMode::Code => PreviewContent::Code(highlighter().highlight(path, &text)),
        PreviewMode::Image | PreviewMode::Pdf | PreviewMode::Video => unreachable!(),
    };
    PreviewState {
        path: path.to_path_buf(),
        mode,
        content,
        truncated,
    }
}

/// 高速ロード（UI スレッド用）: ファイルを読むが syntect ハイライトはスキップする。
/// Code モードは平文（色なし）を返し、呼び出し側が background で [`highlight_text`] を
/// 走らせて差し替える。Markdown は pulldown-cmark が十分速いのでそのまま完成版を返す。
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
    let (content, raw) = match mode {
        PreviewMode::Markdown => (PreviewContent::Markdown(markdown_blocks(&text)), None),
        PreviewMode::Code => {
            let lines = text.lines().map(|l| vec![plain_span(l)]).collect();
            (PreviewContent::Code(lines), Some(text))
        }
        PreviewMode::Image | PreviewMode::Pdf | PreviewMode::Video => unreachable!(),
    };
    (
        PreviewState {
            path: path.to_path_buf(),
            mode,
            content,
            truncated,
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
            let content = match mode {
                PreviewMode::Code => PreviewContent::Code(highlighter().highlight(path, &text)),
                PreviewMode::Markdown => PreviewContent::Markdown(markdown_blocks(&text)),
                _ => unreachable!(),
            };
            (
                PreviewState {
                    path: path.to_path_buf(),
                    mode,
                    content,
                    truncated,
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
    let file = std::fs::File::open(path).map_err(|e| format!("読み込めない: {e}"))?;
    let mut bytes = Vec::with_capacity(MAX_BYTES.min(64 * 1024) + 1);
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("読み込めない: {e}"))?;
    let truncated_bytes = bytes.len() > MAX_BYTES;
    bytes.truncate(MAX_BYTES);
    if bytes.contains(&0) {
        return Err("バイナリファイル（テキストとして表示できない）".into());
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

/// 既定ハイライタ（プロセス内で 1 度だけ構文セットを読む）
pub fn highlighter() -> &'static dyn Highlighter {
    static INSTANCE: OnceLock<SyntectHighlighter> = OnceLock::new();
    INSTANCE.get_or_init(SyntectHighlighter::new)
}

/// syntect 実装（bat / delta と同系の定番。純 Rust 構成 = regex-fancy）
pub struct SyntectHighlighter {
    syntaxes: syntect::parsing::SyntaxSet,
    theme: syntect::highlighting::Theme,
}

impl SyntectHighlighter {
    fn new() -> Self {
        let syntaxes = syntect::parsing::SyntaxSet::load_defaults_newlines();
        // ダーク背景（tako 既定テーマ）に合う同梱テーマ。見つからなければ任意の 1 つ
        let mut themes = syntect::highlighting::ThemeSet::load_defaults().themes;
        let theme = themes
            .remove("base16-eighties.dark")
            .or_else(|| themes.into_values().next())
            .unwrap_or_default();
        Self { syntaxes, theme }
    }

    /// 読み取り / 編集で共用する構文解決。syntect 標準セットに含まれる全構文を
    /// 拡張子・特殊ファイル名・shebang の順で解決し、標準セットに TypeScript 文法が
    /// 無い版では JavaScript 文法へ安全に劣化させる。
    fn syntax_for_path<'a>(
        &'a self,
        path: &Path,
        text: &str,
    ) -> &'a syntect::parsing::SyntaxReference {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        extension
            .as_deref()
            .and_then(|ext| self.syntaxes.find_syntax_by_extension(ext))
            .or_else(|| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .and_then(|name| self.syntaxes.find_syntax_by_extension(name))
            })
            .or_else(|| {
                text.lines()
                    .next()
                    .and_then(|line| self.syntaxes.find_syntax_by_first_line(line))
            })
            .or_else(|| {
                matches!(extension.as_deref(), Some("ts" | "tsx"))
                    .then(|| self.syntaxes.find_syntax_by_name("JavaScript"))
                    .flatten()
            })
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
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

/// Markdown をブロック列へパースする（FR-3.3）。表など未対応の構造は
/// テキストとして段落へ劣化させ、内容を落とさない
pub fn markdown_blocks(text: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(text, options);

    let mut blocks = Vec::new();
    let mut spans: Vec<MdSpan> = Vec::new();
    let (mut bold, mut italic, mut strike, mut link) = (0u32, 0u32, 0u32, 0u32);
    // リストのネスト（None = 箇条書き、Some(n) = 番号付きの次番号）
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0u32;
    let mut heading: Option<u8> = None;
    // コードブロック蓄積（lang, 本文）
    let mut code: Option<(String, String)> = None;

    let push_span = |spans: &mut Vec<MdSpan>,
                     text: &str,
                     code_span: bool,
                     bold: u32,
                     italic: u32,
                     strike: u32,
                     link: u32| {
        if text.is_empty() {
            return;
        }
        spans.push(MdSpan {
            text: text.to_string(),
            bold: bold > 0,
            italic: italic > 0,
            code: code_span,
            strike: strike > 0,
            link: link > 0,
        });
    };
    // 段落・見出し等の区切りで溜まったスパンをブロック化する
    fn flush(
        blocks: &mut Vec<MdBlock>,
        spans: &mut Vec<MdSpan>,
        heading: Option<u8>,
        lists: &[Option<u64>],
        quote_depth: u32,
    ) {
        if spans.is_empty() {
            return;
        }
        let spans = std::mem::take(spans);
        if let Some(level) = heading {
            blocks.push(MdBlock::Heading { level, spans });
        } else if let Some(counter) = lists.last() {
            blocks.push(MdBlock::ListItem {
                depth: lists.len().saturating_sub(1),
                marker: match counter {
                    Some(n) => format!("{}.", n.saturating_sub(1)),
                    None => "•".to_string(),
                },
                spans,
            });
        } else if quote_depth > 0 {
            blocks.push(MdBlock::Quote { spans });
        } else {
            blocks.push(MdBlock::Paragraph { spans });
        }
    }

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                heading = None;
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                if let Some(Some(counter)) = lists.last_mut() {
                    *counter += 1;
                }
            }
            Event::End(TagEnd::Item) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                quote_depth = quote_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, body)) = code.take() {
                    let body = body.strip_suffix('\n').unwrap_or(&body);
                    blocks.push(MdBlock::CodeBlock {
                        lines: highlighter().highlight_lang(&lang, body),
                    });
                }
            }
            Event::Start(Tag::Paragraph) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
            }
            Event::End(TagEnd::Paragraph) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
            }
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => link += 1,
            Event::End(TagEnd::Link) => link = link.saturating_sub(1),
            Event::Rule => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                blocks.push(MdBlock::Rule);
            }
            Event::Text(t) => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                } else {
                    push_span(&mut spans, &t, false, bold, italic, strike, link);
                }
            }
            Event::Code(t) => push_span(&mut spans, &t, true, bold, italic, strike, link),
            Event::SoftBreak | Event::HardBreak => {
                push_span(&mut spans, " ", false, bold, italic, strike, link)
            }
            Event::TaskListMarker(done) => push_span(
                &mut spans,
                // MD ソース表記のまま描画（絵文字全廃 #217。[x] / [ ] は mono で揃う）
                if done { "[x] " } else { "[ ] " },
                false,
                bold,
                italic,
                strike,
                link,
            ),
            // 表・HTML 等はインラインテキストとして劣化（内容を落とさない）
            Event::Html(t) | Event::InlineHtml(t) => {
                push_span(&mut spans, &t, false, bold, italic, strike, link)
            }
            _ => {}
        }
    }
    flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
    blocks
}

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
            &blocks[0],
            MdBlock::Heading { level: 1, spans } if spans[0].text == "見出し"
        ));
        let MdBlock::Paragraph { spans } = &blocks[1] else {
            panic!("段落になる: {:?}", blocks[1]);
        };
        assert!(spans.iter().any(|s| s.bold && s.text == "強調"));
        assert!(spans.iter().any(|s| s.code && s.text == "code"));
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                MdBlock::ListItem { marker, spans, .. } => {
                    Some((marker.clone(), spans[0].text.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                ("•".to_string(), "項目1".to_string()),
                ("•".to_string(), "項目2".to_string())
            ]
        );
        assert!(blocks
            .iter()
            .any(|b| matches!(b, MdBlock::CodeBlock { lines } if !lines.is_empty())));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Rule)));
    }

    #[test]
    fn 番号付きリストとネスト() {
        let blocks = markdown_blocks("1. one\n2. two\n   - sub\n");
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                MdBlock::ListItem {
                    depth,
                    marker,
                    spans,
                } => Some((*depth, marker.clone(), spans[0].text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (0, "1.".to_string(), "one".to_string()),
                (0, "2.".to_string(), "two".to_string()),
                (1, "•".to_string(), "sub".to_string()),
            ]
        );
    }

    #[test]
    fn バイナリと不在は明示エラーになる() {
        let dir = std::env::temp_dir().join(format!("tako-preview-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bin.dat");
        std::fs::write(&path, [0u8, 159, 146, 150]).unwrap();
        let state = load(&path, PreviewMode::Code);
        assert!(matches!(&state.content, PreviewContent::Error(m) if m.contains("バイナリ")));
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
                if matches!(&blocks[0], MdBlock::Heading { spans, .. }
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

    #[test]
    #[cfg(target_os = "macos")]
    fn pdfのページレンダリング() {
        let test_paths = [
            "/System/Library/Frameworks/Quartz.framework/Versions/A/Frameworks/PDFKit.framework/Versions/A/Resources/test.pdf",
            "/Library/Documentation/License.lpdf",
        ];
        // テスト用にダミー PDF があれば使う。なければスキップ
        // （CI 環境でシステム PDF の場所が保証できないため）
        let pdf_path = test_paths.iter().find(|p| Path::new(p).is_file());
        if pdf_path.is_none() {
            eprintln!("[skip] テスト用 PDF が見つからない");
            return;
        }
        let state = load(Path::new(pdf_path.unwrap()), PreviewMode::Pdf);
        match &state.content {
            PreviewContent::Pdf(data) => {
                assert!(data.total_pages > 0);
                assert!(!data.pages.is_empty());
                assert!(!data.pages[0].is_empty());
                // PNG シグネチャの確認
                assert_eq!(&data.pages[0][..4], &[0x89, 0x50, 0x4E, 0x47]);
            }
            PreviewContent::Error(e) => {
                eprintln!("[skip] PDF レンダリング失敗（環境依存）: {e}");
            }
            other => panic!("Pdf になる: {:?}", other),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn pdfテキストレイヤ抽出() {
        // 手動構築 PDF（英語テキストのみ）で extract_text_layers が動くか
        let scratchpad = std::env::temp_dir().join("tako_pdf_text_test");
        std::fs::create_dir_all(&scratchpad).ok();
        let pdf_path = scratchpad.join("test_text.pdf");

        // Helvetica 埋め込みの最小 2 行 PDF を生成。T* に使う leading は TL で明示する
        let content = b"BT /F1 14 Tf 14 TL 72 700 Td (Hello World) Tj T* (Second Line) Tj ET";
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let off1 = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let off2 = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        let off3 = pdf.len();
        pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
        let off4 = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        let off5 = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for o in [off1, off2, off3, off4, off5] {
            pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        std::fs::write(&pdf_path, &pdf).unwrap();

        let layers =
            pdf_render::extract_text_layers(&pdf_path, 1).expect("PDFKit のテキスト抽出は成功する");
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
    #[cfg(target_os = "macos")]
    fn pdfテキストなしでもクラッシュしない() {
        let scratchpad = std::env::temp_dir().join("tako_pdf_notext_test");
        std::fs::create_dir_all(&scratchpad).ok();
        let pdf_path = scratchpad.join("notext.pdf");

        // テキストレイヤのない PDF（灰色矩形のみ）
        let content = b"q 0.8 0.8 0.8 rg 100 600 200 100 re f Q";
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let off1 = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let off2 = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        let off3 = pdf.len();
        pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << >> >>\nendobj\n");
        let off4 = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for o in [off1, off2, off3, off4] {
            pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        std::fs::write(&pdf_path, &pdf).unwrap();

        // クラッシュせずに読めること
        let state = load(&pdf_path, PreviewMode::Pdf);
        match &state.content {
            PreviewContent::Pdf(data) => {
                assert_eq!(data.total_pages, 1);
                // テキストレイヤは空（またはテキストなし）
                let text_count: usize = data.text_layers.iter().map(|p| p.len()).sum();
                assert_eq!(text_count, 0, "テキストなし PDF ではテキスト行がゼロ");
            }
            PreviewContent::Error(e) => {
                eprintln!("[skip] PDF レンダリング失敗（環境依存）: {e}");
            }
            other => panic!("Pdf になる: {other:?}"),
        }

        std::fs::remove_dir_all(&scratchpad).ok();
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
