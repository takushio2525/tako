//! 抽象境界 B12（ドキュメントレンダラ）の macOS 実装。
//!
//! ページのラスタライズは Core Graphics（`CGContextDrawPDFPage`）、テキストレイヤ・
//! 目次・リンク注釈は PDFKit（`PDFDocument` / `PDFOutline` / `PDFAnnotation`）で取る。
//!
//! **中身は #521 以前の `preview.rs` の `pdf_render` モジュールをそのまま移した**もので、
//! 実装は 1 行も変えていない（Windows 実装を足すために置き場所だけを境界の内側へ動かした）。

use std::path::Path;

use tako_core::{
    PdfLink, PdfLinkTarget, PdfLinks, PreviewOutline, PreviewOutlineItem, PreviewOutlineTarget,
};

use super::PdfCapabilities;
use crate::preview::{PdfCharBox, PdfRasterKey, PdfRasterizedPages, PdfTextLine};

/// PDFKit / Core Graphics は 4 つとも揃っている
pub(super) const CAPABILITIES: PdfCapabilities = PdfCapabilities {
    rasterize: true,
    text_layer: true,
    outline: true,
    links: true,
};

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
    fn CGBitmapContextCreateImage(context: *const core::ffi::c_void) -> *const core::ffi::c_void;
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
    raster_key: PdfRasterKey,
) -> Result<PdfRasterizedPages, String> {
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
        Ok(PdfRasterizedPages {
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
    ) -> *const core::ffi::c_void = std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
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
    ) -> *const core::ffi::c_void = std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
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
    ) -> *const core::ffi::c_void = std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
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

unsafe fn open_pdfkit_document(path: &Path) -> Result<*const core::ffi::c_void, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| "パスが UTF-8 でない".to_string())?;
    let ns_path = make_cfstring(path_str);
    if ns_path.is_null() {
        return Err("CFString 生成失敗".into());
    }
    let nsurl = msg_id(cls("NSURL"), sel_name("fileURLWithPath:"), ns_path);
    CFRelease(ns_path);
    if nsurl.is_null() {
        return Err("NSURL 生成失敗".into());
    }
    let pdf_doc_alloc = msg_no_arg(cls("PDFDocument"), sel_name("alloc"));
    if pdf_doc_alloc.is_null() {
        return Err("PDFDocument alloc 失敗".into());
    }
    let pdf_doc = msg_id(pdf_doc_alloc, sel_name("initWithURL:"), nsurl);
    if pdf_doc.is_null() {
        return Err("PDFDocument initWithURL: 失敗".into());
    }
    Ok(pdf_doc)
}

unsafe fn msg_index_for_page(
    document: *const core::ffi::c_void,
    page: *const core::ffi::c_void,
) -> usize {
    let f: unsafe extern "C" fn(
        *const core::ffi::c_void,
        *const core::ffi::c_void,
        *const core::ffi::c_void,
    ) -> usize = std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
    f(document, sel_name("indexForPage:"), page)
}

/// PDFKit の PDFOutline ツリーを平坦なクリック可能目次へ変換する。
/// ページを持たないグループ見出しは子だけを辿り、ジャンプ不能な行は UI に出さない。
pub fn extract_outline(path: &Path, total_pages: usize) -> Result<PreviewOutline, String> {
    const MAX_OUTLINE_ITEMS: usize = 5_000;
    const MAX_OUTLINE_DEPTH: u8 = 32;

    unsafe fn collect(
        document: *const core::ffi::c_void,
        parent: *const core::ffi::c_void,
        level: u8,
        total_pages: usize,
        items: &mut Vec<PreviewOutlineItem>,
    ) {
        if parent.is_null() || level > MAX_OUTLINE_DEPTH || items.len() >= MAX_OUTLINE_ITEMS {
            return;
        }
        let child_count = msg_no_arg(parent, sel_name("numberOfChildren")) as usize;
        for index in 0..child_count {
            if items.len() >= MAX_OUTLINE_ITEMS {
                break;
            }
            let child = msg_usize(parent, sel_name("childAtIndex:"), index);
            if child.is_null() {
                continue;
            }
            let label = nsstring_to_rust(msg_no_arg(child, sel_name("label")))
                .unwrap_or_default()
                .trim()
                .to_string();
            let destination = msg_no_arg(child, sel_name("destination"));
            if !label.is_empty() && !destination.is_null() {
                let page = msg_no_arg(destination, sel_name("page"));
                if !page.is_null() {
                    let page_index = msg_index_for_page(document, page);
                    if page_index < total_pages {
                        items.push(PreviewOutlineItem {
                            title: label,
                            level,
                            target: PreviewOutlineTarget::PdfPage {
                                page: page_index + 1,
                            },
                        });
                    }
                }
            }
            collect(document, child, level.saturating_add(1), total_pages, items);
        }
    }

    unsafe {
        let document = open_pdfkit_document(path)?;
        let root = msg_no_arg(document, sel_name("outlineRoot"));
        let mut items = Vec::new();
        if !root.is_null() {
            collect(document, root, 1, total_pages, &mut items);
        }
        msg_no_arg(document, sel_name("release"));
        Ok(PreviewOutline::new(items))
    }
}

/// PDFKit を使ってテキストレイヤを抽出する。
/// 各ページのテキストを行に分割し、行ごとの PDF 座標バウンディングボックスを取得する。
pub fn extract_text_layers(
    path: &Path,
    total_pages: usize,
) -> Result<Vec<Vec<PdfTextLine>>, String> {
    unsafe {
        let pdf_doc = open_pdfkit_document(path)?;

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
                    lines.push(PdfTextLine {
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
                                char_boxes.push(PdfCharBox {
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
                        lines.push(PdfTextLine {
                            text: line_text.to_string(),
                            bbox: [bounds.x, bounds.y, bounds.w, bounds.h],
                            char_boxes,
                        });
                    } else {
                        lines.push(PdfTextLine {
                            text: line_text.to_string(),
                            bbox: [0.0, 0.0, 0.0, 0.0],
                            char_boxes: Vec::new(),
                        });
                    }
                } else {
                    lines.push(PdfTextLine {
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

/// PDFKit のアノテーションからリンク（外部 URL / 内部ページ）を抽出する（#271）。
/// ロード時に 1 回だけ呼び、結果を PdfData.links に保持する。
pub fn extract_links(path: &Path, total_pages: usize) -> Result<PdfLinks, String> {
    unsafe {
        let pdf_doc = open_pdfkit_document(path)?;
        let mut links = Vec::new();

        for page_idx in 0..total_pages {
            let page = msg_usize(pdf_doc, sel_name("pageAtIndex:"), page_idx);
            if page.is_null() {
                continue;
            }
            let annotations = msg_no_arg(page, sel_name("annotations"));
            if annotations.is_null() {
                continue;
            }
            let count = msg_no_arg(annotations, sel_name("count")) as usize;
            for ann_idx in 0..count {
                let annotation = msg_usize(annotations, sel_name("objectAtIndex:"), ann_idx);
                if annotation.is_null() {
                    continue;
                }
                // アノテーションの bounds を取得（PDF 座標系、左下原点）
                let ann_bounds = msg_annotation_bounds(annotation);

                // linkURL（外部 URL）を試す
                let url_obj = msg_no_arg(annotation, sel_name("URL"));
                if !url_obj.is_null() {
                    let abs_string = msg_no_arg(url_obj, sel_name("absoluteString"));
                    if let Some(url) = nsstring_to_rust(abs_string) {
                        if !url.is_empty() {
                            links.push(PdfLink {
                                page_index: page_idx,
                                bbox: [ann_bounds.x, ann_bounds.y, ann_bounds.w, ann_bounds.h],
                                target: PdfLinkTarget::Url { url },
                            });
                            continue;
                        }
                    }
                }

                // destination（内部リンク）を試す
                let destination = msg_no_arg(annotation, sel_name("destination"));
                if !destination.is_null() {
                    let dest_page = msg_no_arg(destination, sel_name("page"));
                    if !dest_page.is_null() {
                        let dest_page_index = msg_index_for_page(pdf_doc, dest_page);
                        if dest_page_index < total_pages {
                            links.push(PdfLink {
                                page_index: page_idx,
                                bbox: [ann_bounds.x, ann_bounds.y, ann_bounds.w, ann_bounds.h],
                                target: PdfLinkTarget::Page {
                                    page: dest_page_index + 1,
                                },
                            });
                        }
                    }
                }
            }
        }

        msg_no_arg(pdf_doc, sel_name("release"));
        Ok(PdfLinks::new(links))
    }
}

/// PDFAnnotation の bounds（NSRect）を取得する。
#[cfg(target_arch = "aarch64")]
unsafe fn msg_annotation_bounds(annotation: *const core::ffi::c_void) -> NSRect {
    let f: unsafe extern "C" fn(*const core::ffi::c_void, *const core::ffi::c_void) -> NSRect =
        std::mem::transmute(objc_msgSend as *const core::ffi::c_void);
    f(annotation, sel_name("bounds"))
}

#[cfg(target_arch = "x86_64")]
unsafe fn msg_annotation_bounds(annotation: *const core::ffi::c_void) -> NSRect {
    extern "C" {
        fn objc_msgSend_stret(
            ret: *mut NSRect,
            receiver: *const core::ffi::c_void,
            sel: *const core::ffi::c_void,
        );
    }
    let mut result = NSRect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };
    objc_msgSend_stret(&mut result, annotation, sel_name("bounds"));
    result
}
