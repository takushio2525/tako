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

// --- シーク・表示まわりの純粋計算（プラットフォーム非依存・単体テスト対象）---

/// CMTime のタイムスケール（1/600 秒。NTSC / PAL 双方のフレームレートを割り切れる慣例値）
#[cfg(target_os = "macos")]
const TIMESCALE: i32 = 600;
/// シーク完了とみなす許容誤差（秒）。正確シークなら 1 フレーム以内に収束する
pub const SEEK_SETTLE_EPS: f64 = 0.05;
/// シーク完了を待つ上限（秒）。到達しないまま表示が固まるのを防ぐ保険
pub const SEEK_SETTLE_TIMEOUT: f64 = 1.5;
/// 末尾に到達したとみなす残り時間（秒）
pub const END_EPS: f64 = 0.05;
/// ドラッグ中のシークに与える許容誤差（秒）。正確シークの連打はデコードが重く
/// つまみが引っかかるため、スクラブ中だけ粗く飛ばして離した時点で正確に合わせる
pub const SCRUB_TOLERANCE: f64 = 0.15;

/// CMTime の生値から秒を求める。無効値（valid フラグ無し / timescale <= 0 /
/// 非有限）は None を返す。indefinite（生放送等）もここで弾かれる
pub fn cm_seconds(value: i64, timescale: i32, flags: u32) -> Option<f64> {
    // kCMTimeFlags_Valid = 1 << 0
    if flags & 1 == 0 || timescale <= 0 {
        return None;
    }
    let seconds = value as f64 / timescale as f64;
    if seconds.is_finite() {
        Some(seconds)
    } else {
        None
    }
}

/// 動画の長さを UI が扱える値へ正規化する。取得できない・不定長・負値はすべて
/// 0.0（= 長さ不明）に倒し、以降の割り算・クランプが破綻しないようにする
pub fn sanitize_duration(seconds: Option<f64>) -> f64 {
    match seconds {
        Some(d) if d.is_finite() && d > 0.0 => d,
        _ => 0.0,
    }
}

/// 再生位置を 0〜duration にクランプする。duration が 0（長さ不明）や
/// 非有限でも panic せず 0.0 に落ちる
pub fn clamp_time(seconds: f64, duration: f64) -> f64 {
    if !seconds.is_finite() {
        return 0.0;
    }
    let max = if duration.is_finite() && duration > 0.0 {
        duration
    } else {
        0.0
    };
    seconds.clamp(0.0, max)
}

/// シークバー上の x 座標（絶対）を再生位置（秒）へ変換する。
/// バー幅 0・長さ 0 でもゼロ除算しない
pub fn seek_seconds_at(x: f32, bar_x: f32, bar_width: f32, duration: f64) -> f64 {
    if bar_width <= 0.0 || !bar_width.is_finite() || !bar_x.is_finite() || !x.is_finite() {
        return 0.0;
    }
    let frac = ((x - bar_x) / bar_width).clamp(0.0, 1.0) as f64;
    clamp_time(frac * duration, duration)
}

/// 再生位置の進捗率（0.0〜1.0）。長さ不明なら 0.0
pub fn progress_fraction(current: f64, duration: f64) -> f32 {
    if !(duration.is_finite() && duration > 0.0 && current.is_finite()) {
        return 0.0;
    }
    (current / duration).clamp(0.0, 1.0) as f32
}

/// 再生位置の時刻表記。1 時間以上の動画は h:mm:ss、それ未満は m:ss。
/// 負値・NaN は 0:00 に倒す
pub fn time_label(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// シークが完了した（プレイヤーの実位置が要求位置に追いついた）かを判定する。
/// 許容誤差内に収まるか、待ち時間が上限を超えたら完了扱いにする
pub fn seek_settled(actual: Option<f64>, target: f64, tolerance: f64, elapsed_secs: f64) -> bool {
    if elapsed_secs >= SEEK_SETTLE_TIMEOUT {
        return true;
    }
    match actual {
        Some(a) => (a - target).abs() <= SEEK_SETTLE_EPS + tolerance.max(0.0),
        None => false,
    }
}

/// ホバー時刻ツールチップの左位置（バー左端からの相対 px）。
/// バーの外へはみ出さないようにクランプする
pub fn tooltip_left(rel_x: f32, bar_width: f32, tip_width: f32) -> f32 {
    if !rel_x.is_finite() || !bar_width.is_finite() {
        return 0.0;
    }
    let max = (bar_width - tip_width).max(0.0);
    (rel_x - tip_width / 2.0).clamp(0.0, max)
}

/// 末尾に到達しているか（長さ不明なら常に false）
pub fn at_end(current: f64, duration: f64) -> bool {
    duration.is_finite() && duration > 0.0 && current >= duration - END_EPS
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
    /// 音量（0.0〜1.0）
    pub volume: f32,
    /// ミュート中か
    pub muted: bool,
    /// ループ再生が有効か
    pub looping: bool,
    /// 進行中のシークの要求位置（秒）と許容誤差・開始時刻。
    /// AVPlayer の seekToTime: は非同期で、完了までは currentTime が旧位置を返す。
    /// 完了までは要求位置を current_time として見せ、つまみの巻き戻りを防ぐ
    seek_pending: Option<SeekPending>,
    /// 一時停止中でもフレームを取り直す期限（シーク直後に設定）。
    /// ティッカーはこの期限内なら停止中のプレイヤーも回し、新しい位置の絵に
    /// 差し替える。期限を過ぎたら諦めて回さない（永久ループ防止）
    refresh_deadline: Option<std::time::Instant>,
    /// 末尾に到達して停止したか（次の再生で先頭へ戻す）
    pub ended: bool,
}

/// 進行中のシーク要求
#[cfg(target_os = "macos")]
struct SeekPending {
    target: f64,
    tolerance: f64,
    started: std::time::Instant,
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

            // アイテムの読み込み状態を軽く確認する。
            //
            // ここで readyToPlay を待ってはいけない（#484）。status の変化は
            // メインスレッドのラン ループ経由で届くため、ラン ループを止めた
            // まま sleep で待つと永久に unknown のままで、待ち時間ぶん UI が
            // 固まるだけになる。読み込み失敗だけ拾って先へ進み、実際のフレーム
            // 取得はティッカーのリトライに任せる
            let status: isize = msg_send_status(player_item, sel("status"));
            // AVPlayerItemStatusFailed = 2
            if status == 2 {
                return Err("動画の読み込みに失敗した".into());
            }

            // 動画の長さを取得する。AVPlayerItem.duration は readyToPlay に
            // なるまで indefinite（timescale=0）を返し、そのまま使うと総尺 0:00 =
            // シークバーが常に先頭という不具合になる。ローカルファイルなら
            // AVAsset.duration が同期で正しい値を返すのでそちらを優先する（#484）
            let duration = sanitize_duration(asset_duration_seconds(player_item))
                .max(sanitize_duration(item_duration_seconds(player_item)));

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
                volume: 1.0,
                muted: false,
                looping: false,
                seek_pending: None,
                refresh_deadline: None,
                ended: false,
            };

            // 最初のフレームを取得する。デコーダの準備が間に合わなければ
            // 空振りするので、しばらくティッカーで取り直せるようにしておく
            player.seek(0.0);
            std::thread::sleep(std::time::Duration::from_millis(100));
            player.grab_frame();
            player.refresh_deadline =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(5));

            Ok(player)
        }
    }

    /// 再生開始。末尾で停止している状態から押されたら先頭へ巻き戻してから再生する
    /// （AVPlayer は末尾のまま setRate: しても進まない）
    pub fn play(&mut self) {
        if self.state == PlaybackState::Playing {
            return;
        }
        if self.ended || at_end(self.current_time, self.duration) {
            self.seek(0.0);
        }
        unsafe {
            let _: () = msg_send_void_f32(self.player, sel("setRate:"), self.rate);
        }
        self.state = PlaybackState::Playing;
        self.ended = false;
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

    /// 指定位置へ正確にシークする（秒）。フレーム単位で要求位置に一致する
    pub fn seek(&mut self, seconds: f64) {
        self.seek_with_tolerance(seconds, 0.0);
    }

    /// 許容誤差つきでシークする（秒）。tolerance = 0.0 なら正確シーク。
    ///
    /// 素の `seekToTime:` は許容誤差が無限大（= 直近のキーフレームへスナップ）で、
    /// GOP の長い動画ではクリック位置から数秒ずれた場所に飛ぶ。tako は
    /// `seekToTime:toleranceBefore:toleranceAfter:` を明示的に使い、
    /// 通常は誤差ゼロ、ドラッグ中だけ粗い誤差を許してデコード負荷を下げる
    pub fn seek_with_tolerance(&mut self, seconds: f64, tolerance: f64) {
        let seconds = clamp_time(seconds, self.duration);
        let tolerance = if tolerance.is_finite() {
            tolerance.max(0.0)
        } else {
            0.0
        };
        unsafe {
            let time = CMTimeMakeWithSeconds(seconds, TIMESCALE);
            let tol = CMTimeMakeWithSeconds(tolerance, TIMESCALE);
            let _: () = msg_send_seek_tolerance(
                self.player,
                sel("seekToTime:toleranceBefore:toleranceAfter:"),
                time,
                tol,
                tol,
            );
        }
        // シーク完了までは要求位置を見せる（実位置は非同期に追いつく）
        self.current_time = seconds;
        self.seek_pending = Some(SeekPending {
            target: seconds,
            tolerance,
            started: std::time::Instant::now(),
        });
        // 一時停止中でも新しい位置の絵に差し替える必要がある
        self.refresh_deadline = Some(
            std::time::Instant::now() + std::time::Duration::from_secs_f64(SEEK_SETTLE_TIMEOUT),
        );
        self.ended = false;
    }

    /// 相対シーク（±秒。現在位置 + delta、0〜duration にクランプ）
    pub fn seek_relative(&mut self, delta: f64) {
        self.seek(self.current_time + delta);
    }

    /// ティッカーを回す必要があるか（再生中、またはシーク後のフレーム取り直し待ち）
    pub fn needs_tick(&self) -> bool {
        self.state == PlaybackState::Playing
            || self
                .refresh_deadline
                .is_some_and(|deadline| std::time::Instant::now() < deadline)
    }

    /// 音量を設定（0.0〜1.0）。AVPlayer.volume を直接操作
    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
        unsafe {
            let effective = if self.muted { 0.0 } else { self.volume };
            let _: () = msg_send_void_f32(self.player, sel("setVolume:"), effective);
        }
    }

    /// ミュートのトグル
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        unsafe {
            let effective = if self.muted { 0.0 } else { self.volume };
            let _: () = msg_send_void_f32(self.player, sel("setVolume:"), effective);
        }
    }

    /// ループ再生のトグル
    pub fn toggle_loop(&mut self) {
        self.looping = !self.looping;
    }

    /// 現在のフレームをキャプチャして current_bgra に格納する。
    /// 再生中は定期的に呼ぶ（タイマー駆動）
    pub fn grab_frame(&mut self) -> bool {
        let grabbed = self.grab_frame_inner();
        // 末尾判定は「新しいフレームが来たとき」ではなく毎回行う。
        // 末尾では新フレームが来なくなるため、ここに置かないとループも
        // 再生状態のリセットも永久に発火しない
        self.update_end_of_stream();
        grabbed
    }

    /// 末尾到達の後始末。ループ中なら先頭へ戻し、そうでなければ再生状態を
    /// 停止へ落とす（AVPlayer 側は既に rate=0 で止まっている）
    fn update_end_of_stream(&mut self) {
        if self.seek_pending.is_some() || self.state != PlaybackState::Playing {
            return;
        }
        if !at_end(self.current_time, self.duration) {
            return;
        }
        if self.looping {
            self.seek(0.0);
            unsafe {
                let _: () = msg_send_void_f32(self.player, sel("setRate:"), self.rate);
            }
        } else {
            unsafe {
                let _: () = msg_send_void(self.player, sel("pause"));
            }
            self.state = PlaybackState::Paused;
            self.ended = true;
            self.current_time = self.duration;
        }
    }

    fn grab_frame_inner(&mut self) -> bool {
        unsafe {
            // 総尺が取れていなければ取り直す（ネットワーク越し等、開いた時点では
            // まだ読み込めていないケースの自己修復。#484）
            if self.duration <= 0.0 {
                let refreshed = sanitize_duration(item_duration_seconds(self.player_item))
                    .max(sanitize_duration(asset_duration_seconds(self.player_item)));
                if refreshed > 0.0 {
                    self.duration = refreshed;
                }
            }

            // 現在時刻を取得。シーク進行中は要求位置を維持し、実位置が追いついた
            // 時点で実位置へ切り替える（切り替えないとつまみが旧位置へ巻き戻る）
            let current: CMTime = msg_send_cmtime(self.player, sel("currentTime"));
            let actual = cm_seconds(current.value, current.timescale, current.flags);
            match &self.seek_pending {
                Some(pending) => {
                    let elapsed = pending.started.elapsed().as_secs_f64();
                    if seek_settled(actual, pending.target, pending.tolerance, elapsed) {
                        self.current_time = actual.unwrap_or(pending.target);
                        self.seek_pending = None;
                    } else {
                        self.current_time = pending.target;
                    }
                }
                None => {
                    if let Some(actual) = actual {
                        self.current_time = actual;
                    }
                }
            }

            // hasNewPixelBufferForItemTime: で確認
            let has_new: bool = msg_send_bool_cmtime(
                self.video_output,
                sel("hasNewPixelBufferForItemTime:"),
                current,
            );
            if !has_new {
                // シーク直後は完了までフレームが来ない。取り直しは期限まで
                // 次のティックへ回す（時間切れなら諦めてティッカーを止める）
                if self
                    .refresh_deadline
                    .is_some_and(|deadline| std::time::Instant::now() >= deadline)
                {
                    self.refresh_deadline = None;
                }
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

            // シーク後の絵が届いたので取り直し要求を解除する（一時停止中の
            // ティッカーはこれで止まる）。シーク進行中なら次の絵まで続ける
            if self.seek_pending.is_none() {
                self.refresh_deadline = None;
            }

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

/// AVPlayerItem.duration（readyToPlay 前は indefinite = None）
#[cfg(target_os = "macos")]
unsafe fn item_duration_seconds(player_item: *const c_void) -> Option<f64> {
    let t: CMTime = msg_send_cmtime(player_item, sel("duration"));
    cm_seconds(t.value, t.timescale, t.flags)
}

/// AVPlayerItem.asset.duration（ローカルファイルなら読み込み完了前でも取れる）
#[cfg(target_os = "macos")]
unsafe fn asset_duration_seconds(player_item: *const c_void) -> Option<f64> {
    let asset = msg_send_no_arg(player_item, sel("asset"));
    if asset.is_null() {
        return None;
    }
    let t: CMTime = msg_send_cmtime(asset, sel("duration"));
    cm_seconds(t.value, t.timescale, t.flags)
}

/// `seekToTime:toleranceBefore:toleranceAfter:` 用（CMTime を 3 つ渡す）
#[cfg(target_os = "macos")]
unsafe fn msg_send_seek_tolerance(
    receiver: *const c_void,
    selector: *const c_void,
    time: CMTime,
    before: CMTime,
    after: CMTime,
) {
    let f: unsafe extern "C" fn(*const c_void, *const c_void, CMTime, CMTime, CMTime) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(receiver, selector, time, before, after);
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
    pub volume: f32,
    pub muted: bool,
    pub looping: bool,
    pub ended: bool,
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
    pub fn seek_with_tolerance(&mut self, _seconds: f64, _tolerance: f64) {}
    pub fn seek_relative(&mut self, _delta: f64) {}
    pub fn needs_tick(&self) -> bool {
        false
    }
    pub fn set_volume(&mut self, _vol: f32) {}
    pub fn toggle_mute(&mut self) {}
    pub fn toggle_loop(&mut self) {}
    pub fn grab_frame(&mut self) -> bool {
        false
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for VideoPlayer {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cm_seconds_rejects_invalid_time() {
        // 有効フラグ付き・timescale 600 → 秒に変換できる
        assert_eq!(cm_seconds(600, 600, 1), Some(1.0));
        // valid フラグ無し（indefinite 等）は None
        assert_eq!(cm_seconds(600, 600, 0), None);
        // timescale 0 でゼロ除算しない
        assert_eq!(cm_seconds(600, 0, 1), None);
    }

    #[test]
    fn sanitize_duration_falls_back_to_unknown() {
        assert_eq!(sanitize_duration(Some(30.0)), 30.0);
        assert_eq!(sanitize_duration(None), 0.0);
        assert_eq!(sanitize_duration(Some(-1.0)), 0.0);
        assert_eq!(sanitize_duration(Some(f64::NAN)), 0.0);
        assert_eq!(sanitize_duration(Some(f64::INFINITY)), 0.0);
    }

    #[test]
    fn clamp_time_handles_edges_and_zero_length() {
        assert_eq!(clamp_time(5.0, 30.0), 5.0);
        assert_eq!(clamp_time(-3.0, 30.0), 0.0);
        assert_eq!(clamp_time(99.0, 30.0), 30.0);
        // ゼロ長（長さ不明）でも panic せず 0.0 に落ちる
        assert_eq!(clamp_time(5.0, 0.0), 0.0);
        assert_eq!(clamp_time(5.0, f64::NAN), 0.0);
        assert_eq!(clamp_time(f64::NAN, 30.0), 0.0);
    }

    #[test]
    fn seek_seconds_at_maps_click_x_to_time() {
        // バー幅 200px・30 秒の動画。中央クリック = 15 秒
        assert_eq!(seek_seconds_at(200.0, 100.0, 200.0, 30.0), 15.0);
        // 左端・右端
        assert_eq!(seek_seconds_at(100.0, 100.0, 200.0, 30.0), 0.0);
        assert_eq!(seek_seconds_at(300.0, 100.0, 200.0, 30.0), 30.0);
        // バーの外は端にクランプされる
        assert_eq!(seek_seconds_at(0.0, 100.0, 200.0, 30.0), 0.0);
        assert_eq!(seek_seconds_at(9999.0, 100.0, 200.0, 30.0), 30.0);
        // 幅 0（レイアウト未確定）でゼロ除算しない
        assert_eq!(seek_seconds_at(150.0, 100.0, 0.0, 30.0), 0.0);
    }

    #[test]
    fn progress_fraction_is_bounded() {
        assert_eq!(progress_fraction(15.0, 30.0), 0.5);
        assert_eq!(progress_fraction(0.0, 30.0), 0.0);
        assert_eq!(progress_fraction(30.0, 30.0), 1.0);
        // 実位置が総尺を超えても 1.0 を超えない
        assert_eq!(progress_fraction(31.0, 30.0), 1.0);
        // 長さ不明・NaN でもつまみは先頭に留まる
        assert_eq!(progress_fraction(5.0, 0.0), 0.0);
        assert_eq!(progress_fraction(f64::NAN, 30.0), 0.0);
    }

    #[test]
    fn time_label_formats_minutes_and_hours() {
        assert_eq!(time_label(0.0), "0:00");
        assert_eq!(time_label(9.9), "0:09");
        assert_eq!(time_label(65.0), "1:05");
        assert_eq!(time_label(3600.0), "1:00:00");
        assert_eq!(time_label(3725.0), "1:02:05");
        // 負値・NaN は 0:00
        assert_eq!(time_label(-5.0), "0:00");
        assert_eq!(time_label(f64::NAN), "0:00");
    }

    #[test]
    fn seek_settled_waits_for_player_to_catch_up() {
        // 実位置が旧位置のまま = 未完了（つまみを要求位置に留める）
        assert!(!seek_settled(Some(0.0), 15.0, 0.0, 0.0));
        // 許容誤差内に入ったら完了
        assert!(seek_settled(Some(15.02), 15.0, 0.0, 0.1));
        // ドラッグ中の粗いシークは tolerance の分だけ緩い
        assert!(seek_settled(Some(15.1), 15.0, SCRUB_TOLERANCE, 0.1));
        assert!(!seek_settled(Some(15.1), 15.0, 0.0, 0.1));
        // 実位置が取れなくても時間切れなら完了扱い（固まらない保険）
        assert!(!seek_settled(None, 15.0, 0.0, 0.1));
        assert!(seek_settled(None, 15.0, 0.0, SEEK_SETTLE_TIMEOUT));
    }

    #[test]
    fn tooltip_left_stays_inside_the_bar() {
        // 中央なら中心合わせ
        assert_eq!(tooltip_left(100.0, 200.0, 40.0), 80.0);
        // 左端・右端でバーからはみ出さない
        assert_eq!(tooltip_left(0.0, 200.0, 40.0), 0.0);
        assert_eq!(tooltip_left(200.0, 200.0, 40.0), 160.0);
        // バーがツールチップより狭くても負にならない
        assert_eq!(tooltip_left(10.0, 20.0, 40.0), 0.0);
    }

    #[test]
    fn at_end_detects_last_frame_only() {
        assert!(at_end(30.0, 30.0));
        assert!(at_end(29.99, 30.0));
        assert!(!at_end(29.0, 30.0));
        // 長さ不明なら末尾判定しない（誤停止させない）
        assert!(!at_end(0.0, 0.0));
    }
}
