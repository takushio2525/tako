//! macOS AVFoundation ベースの動画プレイヤー
//!
//! AVPlayer + AVPlayerItemVideoOutput でデコード済みフレームを
//! RGBA ビットマップとして取得し、GPUI の img() で描画する。
//! PDF 描画（preview.rs pdf_render）と同じ raw objc FFI パターン。

#[cfg(target_os = "macos")]
use std::ffi::c_void;

#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[cfg(target_os = "macos")]
extern "C" {
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        num_bytes: isize,
        encoding: u32,
        is_external: bool,
    ) -> *const c_void;
}

#[cfg(target_os = "macos")]
extern "C" {
    // CoreVideo pixel buffer
    fn CVPixelBufferGetWidth(pixel_buffer: *const c_void) -> usize;
    fn CVPixelBufferGetHeight(pixel_buffer: *const c_void) -> usize;
    fn CVPixelBufferGetBytesPerRow(pixel_buffer: *const c_void) -> usize;
    fn CVPixelBufferLockBaseAddress(pixel_buffer: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pixel_buffer: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pixel_buffer: *const c_void) -> *const u8;
}

#[cfg(target_os = "macos")]
extern "C" {
    // CoreMedia time
    fn CMTimeMakeWithSeconds(seconds: f64, preferred_timescale: i32) -> CMTime;
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[cfg(target_os = "macos")]
const CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
// kCVPixelBufferLockFlags の読み取り専用フラグ
#[cfg(target_os = "macos")]
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 0x00000001;

// kCVPixelFormatType_32BGRA
#[cfg(target_os = "macos")]
const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = 0x42475241; // 'BGRA'

#[cfg(target_os = "macos")]
fn make_cfstring(s: &str) -> *const c_void {
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

/// 動画プレイヤーの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Paused,
    Playing,
}

/// macOS AVFoundation ベースの動画プレイヤー
#[cfg(target_os = "macos")]
pub struct VideoPlayer {
    player: *const c_void,       // AVPlayer
    player_item: *const c_void,  // AVPlayerItem
    video_output: *const c_void, // AVPlayerItemVideoOutput
    pub state: PlaybackState,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    /// 現在のフレーム（RGBA PNG バイト列。img() で描画する）
    pub current_frame: Vec<u8>,
    /// 現在の再生位置（秒）
    pub current_time: f64,
    /// フレーム世代カウンタ（grab_frame 成功ごとにインクリメント。描画キャッシュの無効化に使う）
    pub frame_gen: u64,
}

#[cfg(target_os = "macos")]
unsafe impl Send for VideoPlayer {}

#[cfg(target_os = "macos")]
impl VideoPlayer {
    /// 動画ファイルからプレイヤーを初期化する
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let path_str = path
            .to_str()
            .ok_or_else(|| "パスが UTF-8 でない".to_string())?;

        unsafe {
            // NSURL fileURLWithPath:
            let ns_string = msg_class_str("NSString", "stringWithUTF8String:", path_str)?;
            let url = msg_send_id(get_class("NSURL"), sel("fileURLWithPath:"), ns_string);
            if url.is_null() {
                return Err("URL を生成できない".into());
            }

            // AVPlayerItem itemWithURL:
            let player_item =
                msg_send_id(get_class("AVPlayerItem"), sel("playerItemWithURL:"), url);
            if player_item.is_null() {
                return Err("AVPlayerItem を生成できない".into());
            }
            // retain
            let _: *const c_void = msg_send_no_arg(player_item, sel("retain"));

            // AVPlayerItemVideoOutput — BGRA フォーマットで要求
            let pixel_format_key = make_cfstring("PixelFormatType");
            let format_num: *const c_void = msg_send_u32(
                get_class("NSNumber"),
                sel("numberWithUnsignedInt:"),
                K_CV_PIXEL_FORMAT_TYPE_32_BGRA,
            );
            let keys: [*const c_void; 1] = [pixel_format_key];
            let vals: [*const c_void; 1] = [format_num];
            let pixel_attrs = msg_send_dict(
                get_class("NSDictionary"),
                sel("dictionaryWithObjects:forKeys:count:"),
                vals.as_ptr(),
                keys.as_ptr(),
                1,
            );
            CFRelease(pixel_format_key);

            let video_output_class = get_class("AVPlayerItemVideoOutput");
            let video_output: *const c_void = msg_send_id(
                msg_send_no_arg(video_output_class, sel("alloc")),
                sel("initWithPixelBufferAttributes:"),
                pixel_attrs,
            );
            if video_output.is_null() {
                return Err("AVPlayerItemVideoOutput を生成できない".into());
            }

            // AVPlayerItem に VideoOutput を追加
            let _: () = msg_send_void_id(player_item, sel("addOutput:"), video_output);

            // AVPlayer playerWithPlayerItem:
            let player = msg_send_id(
                get_class("AVPlayer"),
                sel("playerWithPlayerItem:"),
                player_item,
            );
            if player.is_null() {
                return Err("AVPlayer を生成できない".into());
            }
            let _: *const c_void = msg_send_no_arg(player, sel("retain"));

            // アイテムの読み込み完了を待つ（最大 2 秒）
            let start = std::time::Instant::now();
            loop {
                let status: isize = msg_send_status(player_item, sel("status"));
                // AVPlayerItemStatusReadyToPlay = 1
                if status == 1 {
                    break;
                }
                // AVPlayerItemStatusFailed = 2
                if status == 2 {
                    return Err("動画の読み込みに失敗した".into());
                }
                if start.elapsed() > std::time::Duration::from_secs(2) {
                    break; // タイムアウトしても続行（メタ情報は取れないかもしれないが）
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            // 動画の長さを取得
            let duration_cm: CMTime = msg_send_cmtime(player_item, sel("duration"));
            let duration = if duration_cm.timescale > 0 {
                duration_cm.value as f64 / duration_cm.timescale as f64
            } else {
                0.0
            };

            // 動画のサイズを取得（AVPlayerItem.presentationSize）
            let pres_size: [f64; 2] = msg_send_cgsize(player_item, sel("presentationSize"));
            let width = pres_size[0].max(0.0) as u32;
            let height = pres_size[1].max(0.0) as u32;

            let mut player = VideoPlayer {
                player,
                player_item,
                video_output,
                state: PlaybackState::Paused,
                duration,
                width,
                height,
                current_frame: Vec::new(),
                current_time: 0.0,
                frame_gen: 0,
            };

            // 最初のフレームを取得
            player.seek(0.0);
            std::thread::sleep(std::time::Duration::from_millis(100));
            player.grab_frame();

            Ok(player)
        }
    }

    /// 再生開始
    pub fn play(&mut self) {
        if self.state == PlaybackState::Playing {
            return;
        }
        unsafe {
            let _: () = msg_send_void(self.player, sel("play"));
        }
        self.state = PlaybackState::Playing;
    }

    /// 一時停止
    pub fn pause(&mut self) {
        if self.state == PlaybackState::Paused {
            return;
        }
        unsafe {
            let _: () = msg_send_void(self.player, sel("pause"));
        }
        self.state = PlaybackState::Paused;
    }

    /// 再生/一時停止トグル
    pub fn toggle(&mut self) {
        if self.state == PlaybackState::Playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// 指定位置へシーク（秒）
    pub fn seek(&mut self, seconds: f64) {
        let seconds = seconds.clamp(0.0, self.duration);
        unsafe {
            let time = CMTimeMakeWithSeconds(seconds, 600);
            let _: () = msg_send_cmtime_arg(self.player, sel("seekToTime:"), time);
        }
        self.current_time = seconds;
    }

    /// 現在のフレームをキャプチャして current_frame に格納する。
    /// 再生中は定期的に呼ぶ（タイマー駆動）
    pub fn grab_frame(&mut self) -> bool {
        unsafe {
            // 現在時刻を取得
            let current: CMTime = msg_send_cmtime(self.player, sel("currentTime"));
            if current.timescale > 0 {
                self.current_time = current.value as f64 / current.timescale as f64;
            }

            // hasNewPixelBufferForItemTime: で確認
            let has_new: bool = msg_send_bool_cmtime(
                self.video_output,
                sel("hasNewPixelBufferForItemTime:"),
                current,
            );
            if !has_new {
                return false;
            }

            // copyPixelBufferForItemTime:itemTimeForDisplay:
            let pixel_buffer: *const c_void = msg_send_copy_pixel_buffer(
                self.video_output,
                sel("copyPixelBufferForItemTime:itemTimeForDisplay:"),
                current,
            );
            if pixel_buffer.is_null() {
                return false;
            }

            // ピクセルバッファから RGBA データを取得
            let width = CVPixelBufferGetWidth(pixel_buffer);
            let height = CVPixelBufferGetHeight(pixel_buffer);
            let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);

            CVPixelBufferLockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            let base = CVPixelBufferGetBaseAddress(pixel_buffer);

            if !base.is_null() && width > 0 && height > 0 {
                // BGRA → RGBA 変換して PNG エンコード
                let mut rgba = vec![0u8; width * height * 4];
                for y in 0..height {
                    let src_row = base.add(y * bytes_per_row);
                    let dst_offset = y * width * 4;
                    for x in 0..width {
                        let src = src_row.add(x * 4);
                        let dst = dst_offset + x * 4;
                        rgba[dst] = *src.add(2); // R ← B
                        rgba[dst + 1] = *src.add(1); // G ← G
                        rgba[dst + 2] = *src; // B ← R
                        rgba[dst + 3] = *src.add(3); // A ← A
                    }
                }

                self.width = width as u32;
                self.height = height as u32;
                self.current_frame = encode_rgba_to_png(&rgba, width as u32, height as u32);
                self.frame_gen += 1;
            }

            CVPixelBufferUnlockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            CFRelease(pixel_buffer);

            true
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for VideoPlayer {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send_void(self.player, sel("pause"));
            let _: () = msg_send_void_id(self.player_item, sel("removeOutput:"), self.video_output);
            let _: () = msg_send_void(self.video_output, sel("release"));
            let _: () = msg_send_void(self.player_item, sel("release"));
            let _: () = msg_send_void(self.player, sel("release"));
        }
    }
}

// --- 最小限の PNG エンコーダ（外部依存なし） ---

fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() + 1024);

    // PNG signature
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type = RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT — filter=None (0) の行を deflate で圧縮
    let row_bytes = width as usize * 4;
    let mut raw = Vec::with_capacity((row_bytes + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter type = None
        raw.extend_from_slice(&rgba[y * row_bytes..(y + 1) * row_bytes]);
    }
    let compressed = miniz_compress(&raw);
    write_png_chunk(&mut out, b"IDAT", &compressed);

    // IEND
    write_png_chunk(&mut out, b"IEND", &[]);

    out
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let mut hasher = Crc32::new();
    hasher.update(chunk_type);
    hasher.update(data);
    out.extend_from_slice(&hasher.finish().to_be_bytes());
}

struct Crc32(u32);
impl Crc32 {
    fn new() -> Self {
        Self(0xFFFFFFFF)
    }
    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= byte as u32;
            for _ in 0..8 {
                if self.0 & 1 != 0 {
                    self.0 = (self.0 >> 1) ^ 0xEDB88320;
                } else {
                    self.0 >>= 1;
                }
            }
        }
    }
    fn finish(self) -> u32 {
        self.0 ^ 0xFFFFFFFF
    }
}

/// 最小限の deflate 圧縮（stored blocks のみ。高品質な圧縮は不要 — フレーム更新速度が優先）
fn miniz_compress(input: &[u8]) -> Vec<u8> {
    // zlib header (deflate, no dict, default compression)
    let mut out = vec![0x78, 0x01];

    // stored blocks（65535 バイト以下のチャンクに分割）
    let chunks: Vec<&[u8]> = input.chunks(65535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == chunks.len() - 1;
        out.push(if is_last { 0x01 } else { 0x00 }); // BFINAL + BTYPE=00(stored)
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }

    // Adler-32
    let adler = adler32(input);
    out.extend_from_slice(&adler.to_be_bytes());

    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// --- Objective-C runtime FFI ヘルパー（既存の PDF 描画と同じパターン） ---

#[cfg(target_os = "macos")]
extern "C" {
    fn objc_getClass(name: *const u8) -> *const c_void;
    fn sel_registerName(name: *const u8) -> *const c_void;
    fn objc_msgSend(receiver: *const c_void, selector: *const c_void, ...) -> *const c_void;
}

#[cfg(target_os = "macos")]
fn get_class(name: &str) -> *const c_void {
    let cstr = std::ffi::CString::new(name).unwrap();
    unsafe { objc_getClass(cstr.as_ptr() as *const u8) }
}

#[cfg(target_os = "macos")]
fn sel(name: &str) -> *const c_void {
    let cstr = std::ffi::CString::new(name).unwrap();
    unsafe { sel_registerName(cstr.as_ptr() as *const u8) }
}

#[cfg(target_os = "macos")]
unsafe fn msg_class_str(
    class_name: &str,
    sel_name: &str,
    arg: &str,
) -> Result<*const c_void, String> {
    let cstr = std::ffi::CString::new(arg).map_err(|_| "CString 変換失敗".to_string())?;
    let cls = get_class(class_name);
    let selector = sel(sel_name);
    // ARM64 では variadic 呼び出しの引数がスタックに置かれるため、
    // 型付き関数ポインタ経由で呼ぶ（レジスタ渡しを保証）
    let f: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> *const c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    let result = f(cls, selector, cstr.as_ptr() as *const c_void);
    if result.is_null() {
        return Err(format!("{class_name} {sel_name} が null を返した"));
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_id(
    receiver: *const c_void,
    selector: *const c_void,
    arg: *const c_void,
) -> *const c_void {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> *const c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, arg)
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_u32(
    receiver: *const c_void,
    selector: *const c_void,
    arg: u32,
) -> *const c_void {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, u32) -> *const c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, arg)
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_dict(
    receiver: *const c_void,
    selector: *const c_void,
    objects: *const *const c_void,
    keys: *const *const c_void,
    count: usize,
) -> *const c_void {
    let f: unsafe extern "C" fn(
        *const c_void,
        *const c_void,
        *const *const c_void,
        *const *const c_void,
        usize,
    ) -> *const c_void = std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, objects, keys, count)
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_no_arg(receiver: *const c_void, selector: *const c_void) -> *const c_void {
    objc_msgSend(receiver, selector)
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_void(receiver: *const c_void, selector: *const c_void) {
    objc_msgSend(receiver, selector);
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_void_id(receiver: *const c_void, selector: *const c_void, arg: *const c_void) {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, arg);
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_status(receiver: *const c_void, selector: *const c_void) -> isize {
    objc_msgSend(receiver, selector) as isize
}

// CMTime を返すメッセージは構造体戻り値なので objc_msgSend_stret が必要（x86_64）。
// ARM64 では objc_msgSend で返せる（16バイト以下の構造体は GPR に入る）
#[cfg(target_os = "macos")]
#[cfg(target_arch = "aarch64")]
unsafe fn msg_send_cmtime(receiver: *const c_void, selector: *const c_void) -> CMTime {
    // ARM64: objc_msgSend が CMTime（24 bytes）を返せる
    let f: unsafe extern "C" fn(*const c_void, *const c_void) -> CMTime =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector)
}

#[cfg(target_os = "macos")]
#[cfg(target_arch = "x86_64")]
unsafe fn msg_send_cmtime(receiver: *const c_void, selector: *const c_void) -> CMTime {
    extern "C" {
        fn objc_msgSend_stret(ret: *mut CMTime, receiver: *const c_void, selector: *const c_void);
    }
    let mut result = std::mem::zeroed();
    objc_msgSend_stret(&mut result, receiver, selector);
    result
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_cmtime_arg(receiver: *const c_void, selector: *const c_void, time: CMTime) {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, CMTime) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, time);
}

// CGSize を返す（[width, height] として受け取る）
#[cfg(target_os = "macos")]
#[cfg(target_arch = "aarch64")]
unsafe fn msg_send_cgsize(receiver: *const c_void, selector: *const c_void) -> [f64; 2] {
    let f: unsafe extern "C" fn(*const c_void, *const c_void) -> [f64; 2] =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector)
}

#[cfg(target_os = "macos")]
#[cfg(target_arch = "x86_64")]
unsafe fn msg_send_cgsize(receiver: *const c_void, selector: *const c_void) -> [f64; 2] {
    extern "C" {
        fn objc_msgSend_stret(ret: *mut [f64; 2], receiver: *const c_void, selector: *const c_void);
    }
    let mut result = [0.0f64; 2];
    objc_msgSend_stret(&mut result, receiver, selector);
    result
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_bool_cmtime(
    receiver: *const c_void,
    selector: *const c_void,
    time: CMTime,
) -> bool {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, CMTime) -> bool =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, time)
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_copy_pixel_buffer(
    receiver: *const c_void,
    selector: *const c_void,
    time: CMTime,
) -> *const c_void {
    let f: unsafe extern "C" fn(
        *const c_void,
        *const c_void,
        CMTime,
        *mut CMTime,
    ) -> *const c_void = std::mem::transmute(objc_msgSend as *const c_void);
    let mut out_time = std::mem::zeroed();
    f(receiver, selector, time, &mut out_time)
}

// Windows スタブ（コンパイルは通る）
#[cfg(not(target_os = "macos"))]
pub struct VideoPlayer;

#[cfg(not(target_os = "macos"))]
impl VideoPlayer {
    pub fn open(_path: &std::path::Path) -> Result<Self, String> {
        Err("動画再生は macOS でのみ対応".into())
    }
    pub fn play(&mut self) {}
    pub fn pause(&mut self) {}
    pub fn toggle(&mut self) {}
    pub fn seek(&mut self, _seconds: f64) {}
    pub fn grab_frame(&mut self) -> bool {
        false
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for VideoPlayer {
    fn drop(&mut self) {}
}
