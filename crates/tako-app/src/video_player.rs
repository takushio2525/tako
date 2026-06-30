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
    /// 現在のフレーム（BGRA 生バイト。RenderImage に直接渡す）
    pub current_bgra: Vec<u8>,
    /// 現在の再生位置（秒）
    pub current_time: f64,
    /// フレーム世代カウンタ（grab_frame 成功ごとにインクリメント。描画キャッシュの無効化に使う）
    pub frame_gen: u64,
    /// 再生速度（0.5 / 1.0 / 1.5 / 2.0）
    pub rate: f32,
}

// Safety: AVFoundation の API（AVPlayer / AVPlayerItemVideoOutput 等）は多くが
// main-thread-only だが、VideoPlayer は GPUI のメインスレッドコールバック内
// （on_next_frame / Entity コールバック）でのみ操作される。
// バックグラウンドスレッドからのアクセスは行わない前提で Send を実装している。
// この制約が崩れる場合は Send impl を除去し、MainThread<T> 等で保護すること。
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
                current_bgra: Vec::new(),
                current_time: 0.0,
                frame_gen: 0,
                rate: 1.0,
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
            let _: () = msg_send_void_f32(self.player, sel("setRate:"), self.rate);
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

    /// 再生速度を設定（0.5 / 1.0 / 1.5 / 2.0）
    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
        if self.state == PlaybackState::Playing {
            unsafe {
                let _: () = msg_send_void_f32(self.player, sel("setRate:"), rate);
            }
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

    /// 相対シーク（±秒。現在位置 + delta、0〜duration にクランプ）
    pub fn seek_relative(&mut self, delta: f64) {
        self.seek(self.current_time + delta);
    }

    /// 現在のフレームをキャプチャして current_bgra に格納する。
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

            // ピクセルバッファから BGRA データを取得
            let width = CVPixelBufferGetWidth(pixel_buffer);
            let height = CVPixelBufferGetHeight(pixel_buffer);
            let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);

            CVPixelBufferLockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            let base = CVPixelBufferGetBaseAddress(pixel_buffer);

            if !base.is_null() && width > 0 && height > 0 {
                // BGRA データをそのままコピー（GPUI の RenderImage は BGRA を期待する）
                let row_len = width * 4;
                let mut bgra = vec![0u8; width * height * 4];
                for y in 0..height {
                    let src = std::slice::from_raw_parts(base.add(y * bytes_per_row), row_len);
                    bgra[y * row_len..(y + 1) * row_len].copy_from_slice(src);
                }

                self.width = width as u32;
                self.height = height as u32;
                self.current_bgra = bgra;
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
unsafe fn msg_send_void_f32(receiver: *const c_void, selector: *const c_void, arg: f32) {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, f32) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, arg);
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
pub struct VideoPlayer {
    pub state: PlaybackState,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub current_bgra: Vec<u8>,
    pub current_time: f64,
    pub frame_gen: u64,
    pub rate: f32,
}

#[cfg(not(target_os = "macos"))]
impl VideoPlayer {
    pub fn open(_path: &std::path::Path) -> Result<Self, String> {
        Err("動画再生は macOS でのみ対応".into())
    }
    pub fn play(&mut self) {}
    pub fn pause(&mut self) {}
    pub fn toggle(&mut self) {}
    pub fn set_rate(&mut self, _rate: f32) {}
    pub fn seek(&mut self, _seconds: f64) {}
    pub fn seek_relative(&mut self, _delta: f64) {}
    pub fn grab_frame(&mut self) -> bool {
        false
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for VideoPlayer {
    fn drop(&mut self) {}
}
