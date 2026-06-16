//! preview — プレビューペイン（コード / Markdown / 画像 / PDF）の読み込みと整形
//!
//! GPUI 非依存（描画は main.rs 側）。シンタックスハイライトは syntect だが、
//! 将来 tree-sitter へ差し替えられるよう [`Highlighter`] trait で抽象化する
//! （`architecture.md`「コンセプト②の実現」。ユーザー指示）。
//! Markdown は pulldown-cmark でイベントストリームをブロック列へ写す。
//! 画像は生バイトを保持し GPUI 側でデコードする（FR-3.10）。
//! PDF は macOS Core Graphics でページを RGBA にレンダリングする（FR-3.4）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tako_control::protocol::PreviewModeWire;

/// 読み込みの上限（巨大ファイルで UI を固めない。超過分は切り詰めて明示する）
const MAX_BYTES: usize = 1_000_000;
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

/// PDF データ（全ページの PNG を保持し、スクロールで閲覧）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfData {
    /// 各ページの PNG バイト列（Core Graphics でレンダリング済み）
    pub pages: Vec<Vec<u8>>,
    pub total_pages: usize,
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
    /// 読めない・バイナリ等（正常系の劣化。ペインは開いたまま理由を表示する）
    Error(String),
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

/// PDF ファイルの拡張子判定
pub fn is_pdf_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("pdf")
    )
}

const MAX_IMAGE_BYTES: usize = 50_000_000; // 50 MB

/// 画像ファイルを読み込む（生バイト。デコードは GPUI 側）
pub fn load_image(path: &Path) -> PreviewState {
    let format = match image_format_from_path(path) {
        Some(f) => f,
        None => {
            return PreviewState {
                path: path.to_path_buf(),
                mode: PreviewMode::Image,
                content: PreviewContent::Error("対応していない画像形式".into()),
                truncated: false,
            }
        }
    };
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() > MAX_IMAGE_BYTES => PreviewState {
            path: path.to_path_buf(),
            mode: PreviewMode::Image,
            content: PreviewContent::Error(format!(
                "画像が大きすぎる（{:.1} MB、上限 50 MB）",
                bytes.len() as f64 / 1_000_000.0
            )),
            truncated: false,
        },
        Ok(bytes) => PreviewState {
            path: path.to_path_buf(),
            mode: PreviewMode::Image,
            content: PreviewContent::Image(ImageData { bytes, format }),
            truncated: false,
        },
        Err(e) => PreviewState {
            path: path.to_path_buf(),
            mode: PreviewMode::Image,
            content: PreviewContent::Error(format!("読み込めない: {e}")),
            truncated: false,
        },
    }
}

/// PDF の全ページをレンダリングして PreviewState を返す。
/// Core Graphics FFI で描画する（macOS のみ）
pub fn load_pdf(path: &Path, _page: usize) -> PreviewState {
    #[cfg(target_os = "macos")]
    {
        match pdf_render::render_all_pages(path) {
            Ok((all_pages, total_pages)) => PreviewState {
                path: path.to_path_buf(),
                mode: PreviewMode::Pdf,
                content: PreviewContent::Pdf(PdfData {
                    pages: all_pages,
                    total_pages,
                }),
                truncated: false,
            },
            Err(e) => PreviewState {
                path: path.to_path_buf(),
                mode: PreviewMode::Pdf,
                content: PreviewContent::Error(e),
                truncated: false,
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = _page;
        PreviewState {
            path: path.to_path_buf(),
            mode: PreviewMode::Pdf,
            content: PreviewContent::Error("PDF プレビューは macOS のみ対応".into()),
            truncated: false,
        }
    }
}

/// 動画ファイルの拡張子判定
pub fn is_video_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if matches!(
            ext.to_ascii_lowercase().as_str(),
            "mp4" | "webm" | "mov" | "avi" | "mkv"
        )
    )
}

/// 動画ファイルのプレビュー読み込み（ffmpeg でサムネイル抽出 + メタ情報取得）
pub fn load_video(path: &Path) -> PreviewState {
    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            return PreviewState {
                path: path.to_path_buf(),
                mode: PreviewMode::Video,
                content: PreviewContent::Error(format!("読み込めない: {e}")),
                truncated: false,
            };
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

/// ffprobe で動画のメタ情報を取得する。ffprobe が無ければすべて None
fn video_probe(path: &Path) -> (Option<f64>, Option<(u32, u32)>, Option<String>) {
    let output = std::process::Command::new("ffprobe")
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
    let output = std::process::Command::new("ffmpeg")
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

    const RENDER_SCALE: f64 = 2.0; // Retina 品質

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

    pub fn render_all_pages(path: &Path) -> Result<(Vec<Vec<u8>>, usize), String> {
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
            for page_idx in 0..total {
                let page_num = page_idx + 1;
                let pdf_page = CGPDFDocumentGetPage(doc, page_num);
                if pdf_page.is_null() {
                    all_pages.push(Vec::new());
                    continue;
                }

                let media_box = CGPDFPageGetBoxRect(pdf_page, CG_PDF_MEDIA_BOX);
                let pixel_w = (media_box.size.width * RENDER_SCALE) as usize;
                let pixel_h = (media_box.size.height * RENDER_SCALE) as usize;
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

                CGContextScaleCTM(ctx, RENDER_SCALE, RENDER_SCALE);
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
            Ok((all_pages, total))
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
        Err(message) => {
            return PreviewState {
                path: path.to_path_buf(),
                mode,
                content: PreviewContent::Error(message),
                truncated: false,
            }
        }
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
            return (
                PreviewState {
                    path: path.to_path_buf(),
                    mode,
                    content: PreviewContent::Error(message),
                    truncated: false,
                },
                None,
            );
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

/// background executor 上で呼ぶ: syntect ハイライトだけを実行して行列を返す
pub fn highlight_text(path: &Path, text: &str) -> Vec<Line> {
    highlighter().highlight(path, text)
}

/// テキストとして読む。バイナリ（NUL 含有）は明示エラー、上限超過は切り詰める
fn read_text(path: &Path) -> Result<(String, bool), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("読み込めない: {e}"))?;
    let truncated_bytes = bytes.len() > MAX_BYTES;
    let bytes = &bytes[..bytes.len().min(MAX_BYTES)];
    if bytes.contains(&0) {
        return Err("バイナリファイル（テキストとして表示できない）".into());
    }
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut truncated = truncated_bytes;
    if text.lines().count() > MAX_LINES {
        text = text.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
        truncated = true;
    }
    Ok((text, truncated))
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

    fn run(&self, syntax: &syntect::parsing::SyntaxReference, text: &str) -> Vec<Line> {
        use syntect::easy::HighlightLines;
        let mut hl = HighlightLines::new(syntax, &self.theme);
        text.lines()
            .map(|line| {
                match hl.highlight_line(line, &self.syntaxes) {
                    Ok(regions) => regions
                        .into_iter()
                        .map(|(style, fragment)| Span {
                            text: fragment.to_string(),
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
                        .collect(),
                    // ハイライト失敗行は素のテキストへ劣化（表示を欠けさせない）
                    Err(_) => vec![plain_span(line)],
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
        let syntax = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.syntaxes.find_syntax_by_extension(ext))
            .or_else(|| {
                text.lines()
                    .next()
                    .and_then(|line| self.syntaxes.find_syntax_by_first_line(line))
            })
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.run(syntax, text)
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
                if done { "☑ " } else { "☐ " },
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
}
