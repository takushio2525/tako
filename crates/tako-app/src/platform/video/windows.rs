//! 抽象境界 B12（動画プレイヤー）の Windows 実装 — Media Foundation の `IMFMediaEngine`（#521）。
//!
//! Windows 8 以降に OS 同梱の再生エンジン（Edge / Films & TV と同じ土台）を
//! **フレームサーバーモード**で使う。追加の再頒布物は要らず、`windows` crate は既に
//! 依存グラフの中にいる（gpui / wry 経由。PDF が `Data_Pdf` を足したのと同じ形）。
//!
//! macOS が AVPlayer + AVPlayerItemVideoOutput という「再生器 + フレーム取り出し口」の
//! 組で成り立っているのに対し、Media Engine のフレームサーバーモードは
//! **その 2 つを 1 つのオブジェクトで兼ねる**。音声は Media Engine が既定の出力デバイスへ
//! 自分で流すので、tako 側で音を鳴らす仕組みは要らない。
//!
//! ## なぜ WIC ビットマップへ描くのか
//!
//! `TransferVideoFrame` の宛先は、DXGI デバイスマネージャを渡していれば
//! Direct3D テクスチャ、渡していなければ **`IWICBitmap`** になる。tako が GPUI へ渡すのは
//! CPU 側の BGRA バイト列（`current_bgra`）なので、D3D デバイスを持って GPU に描いてから
//! 読み戻すより、最初から CPU で読めるビットマップへ描かせるほうが素直で速い。
//! WinRT の `Windows.Media.Playback.MediaPlayer` を採らなかったのも同じ理由で、
//! あちらは宛先が `IDirect3DSurface` 固定である。
//!
//! ## 実測で分かった罠（#521 のコメントに全実測）
//!
//! **1. 初回ロードだけ極端に遅い。** プロセスで最初に開く 1 本は、デコーダ MFT の
//! 初期化ぶん **4 秒以上**かかることがある（2 回目以降は 0.3〜1.4 秒）。
//! そのため [`VideoPlayer::open`] は**ロード完了を待たない**（#484 で macOS 側が
//! `readyToPlay` を待つのをやめたのと同じ結論）。総尺・解像度・最初のフレームは
//! [`VideoPlayer::grab_frame`] が届き次第埋め、それまで [`VideoPlayer::needs_tick`] が
//! true を返してティッカーを回し続ける。
//!
//! **2. ロード完了前の設定は「速度だけ」巻き戻る。** 音量・シーク位置・ループは
//! ロード前に設定しても残るが、`SetPlaybackRate` だけはロードが
//! `defaultPlaybackRate` で上書きする。→ メタデータ到着時に貼り直す（[`VideoPlayer::sync_after_load`]）。
//!
//! **3. `OnVideoStreamTick` の戻り値は S_OK と S_FALSE を区別する必要がある。**
//! `windows` crate の高レベル API は「エラーでない HRESULT」を全部 `Ok` に畳むので、
//! **新しいフレームの有無が判らなくなる**。vtable を直接呼んで HRESULT を見る。
//!
//! **4. 終了時の ACCESS_VIOLATION は起きない。** PDF（`Windows.Data.Pdf`）では
//! 1 度でも描画すると終了処理で GPU ドライバ DLL の中で落ちるため「番人」で回避したが、
//! Media Engine では同じ問題が出なかった（`Shutdown` 無しの drop / 5 回の生成破棄 /
//! 再生中のプロセス終了、いずれも終了コード 0）。
//!
//! ## 対応コーデック
//!
//! OS が持っているものだけ（既定で H.264 / HEVC / VP9 など。AV1 等はストアの
//! 拡張機能が要ることがある）。開けない形式は `MF_MEDIA_ENGINE_EVENT_ERROR` で返るので、
//! [`VideoPlayer::open`] がその場でエラーにする。

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{implement, Interface, BSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad, WICBitmapLockRead, WICRect,
};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MFCreateAttributes, MFStartup, MFSTARTUP_FULL,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};

use crate::video_player::{
    at_end, clamp_time, sanitize_duration, seek_settled, PlaybackState, SEEK_SETTLE_TIMEOUT,
};

/// [`VideoPlayer::open`] がロード結果を待つ上限。
///
/// 実測では**エラーは 2〜233ms**（コールドで最悪 233ms）、**成功（メタデータ到着）は
/// 339〜1949ms** かかる。つまりこの長さは「壊れたファイルを同期的にエラーにするための
/// 予算」であって、成功を待ち切るためのものではない。成功が間に合わなければ待たずに返し、
/// 総尺・解像度・フレームはティッカーに任せる（罠 1）。
///
/// ## なぜ UI スレッドを待たせてよいのか
///
/// `open()` はユーザー / AI の明示操作（再生ボタン・`tako video …`）からしか呼ばれず、
/// 1 回だけである。ここで待たないと、壊れたファイルや非対応コーデックのときに
/// **MCP / CLI が成功を返して何も起きない**（ペインは黒いまま）という一番困る形になる。
///
/// ## 取りこぼす壊れ方もある
///
/// **壊れ方が軽いとこの時間内にエラーが返ってこない**。実測では 2KB まで削った mp4 は
/// 即座にエラーになるが、13KB 残っているとエンジンがもっと読んでから諦めるので
/// 上限を超え、`open()` は成功してしまう。その場合は [`VideoPlayer::grab_frame`] が
/// あとから届いたエラーを拾って `stalled` を立て、ティッカーを止める
/// （回帰テスト `遅れて届くエラーでもティッカーは止まる`）。
const OPEN_WAIT: Duration = Duration::from_millis(500);

/// ロード完了をあきらめる上限。これを過ぎたら [`VideoPlayer::needs_tick`] は
/// 「ロード待ち」を理由には true を返さなくなる（永久にティッカーを回さない保険）
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Media Engine が返すエラー種別（HTML5 の MediaError に対応）
fn error_message(code: u32, extended: u32) -> String {
    let kind = match code {
        1 => "読み込みが中断された",
        2 => "ネットワークエラー",
        3 => "デコードできない（コーデック非対応の可能性）",
        4 => "この形式は再生できない（OS が対応していないコーデック / 壊れたファイル）",
        5 => "暗号化された動画は再生できない",
        _ => "動画を再生できない",
    };
    if extended != 0 {
        format!("{kind}（0x{extended:08X}）")
    } else {
        kind.to_string()
    }
}

/// Media Engine からの通知を受ける COM オブジェクト。
///
/// コールバックは MF のワーカースレッドから来るので、共有するのは atomic だけにする
/// （プレイヤー本体は UI スレッドが持っており、ロックを持ち込むと素直に詰む）。
#[implement(IMFMediaEngineNotify)]
struct Notify {
    state: Arc<EngineEvents>,
}

/// ワーカースレッドから UI スレッドへ渡す最小限の事実
#[derive(Default)]
struct EngineEvents {
    /// メタデータ（総尺・解像度）が届いたか
    loaded: AtomicU32,
    /// `MF_MEDIA_ENGINE_ERR_*`。0 = エラー無し
    error: AtomicU32,
    /// エラーの拡張 HRESULT（原因の切り分け用）
    error_extended: AtomicU32,
}

impl EngineEvents {
    fn loaded(&self) -> bool {
        self.loaded.load(Ordering::Acquire) != 0
    }
    fn error(&self) -> Option<String> {
        match self.error.load(Ordering::Acquire) {
            0 => None,
            code => Some(error_message(
                code,
                self.error_extended.load(Ordering::Acquire),
            )),
        }
    }
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        if event == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32 {
            self.state.loaded.store(1, Ordering::Release);
        }
        if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            // param1 = MF_MEDIA_ENGINE_ERR_*、param2 = 拡張 HRESULT。
            // 0 が入ると「エラー無し」と区別できないので 1 へ寄せる
            self.state
                .error
                .store((param1 as u32).max(1), Ordering::Release);
            self.state.error_extended.store(param2, Ordering::Release);
        }
        Ok(())
    }
}

/// COM と Media Foundation の初期化はプロセスで 1 回だけ。
///
/// `MFShutdown` は呼ばない。GPUI の終了処理と MF のワーカースレッドの停止順を
/// 保証できないうえ、呼ばなくても実測で終了コードは 0 だった（罠 4）。
pub(super) fn ensure_media_foundation() -> Result<(), String> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| unsafe {
        // GPUI が既に COM を初期化している。戻り値は S_FALSE（初期化済み）や
        // RPC_E_CHANGED_MODE（別のアパートメント）になりうるが、どちらでも
        // Media Engine は動く（STA / MTA 双方で実測）ので無視してよい
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)
            .map_err(|e| format!("Media Foundation を初期化できない: {e}"))
    })
    .clone()
}

/// 進行中のシーク要求（macOS 実装と同じ意味）
struct SeekPending {
    target: f64,
    tolerance: f64,
    started: Instant,
}

/// Media Foundation の `IMFMediaEngine` ベースの動画プレイヤー
pub struct VideoPlayer {
    engine: IMFMediaEngine,
    /// 通知コールバックの COM オブジェクト。**engine より長く生かすために持つ**。
    ///
    /// `MF_MEDIA_ENGINE_CALLBACK` 属性へ渡した時点で参照は増えるが、
    /// 「エンジンがそれを自分の寿命ぶん保持する」とは仕様上保証されていない。
    /// 落ちてから気づく類の壊れ方（解放済みオブジェクトへのコールバック）なので、
    /// Rust 側でも 1 つ握っておく
    _notify: IMFMediaEngineNotify,
    events: Arc<EngineEvents>,
    wic: IWICImagingFactory,
    /// フレームの受け皿。解像度が判るまでは作れないので Option
    bitmap: Option<IWICBitmap>,
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
    /// Media Engine の `SetCurrentTime` も非同期なので、完了までは要求位置を
    /// current_time として見せ、つまみの巻き戻りを防ぐ（macOS と同じ扱い）
    seek_pending: Option<SeekPending>,
    /// 一時停止中でもフレームを取り直す期限（シーク直後に設定）
    refresh_deadline: Option<Instant>,
    /// ロード待ちをあきらめる時刻
    load_deadline: Instant,
    /// メタデータ到着後の貼り直しを済ませたか（罠 2）
    synced_after_load: bool,
    /// 映像トラックがあるか（メタデータ到着で確定。音声だけの mp4 は false）。
    /// 確定前は「あるつもり」で待つ
    has_video: bool,
    /// これ以上待っても何も来ないと判った（エラー / ロード時間切れ）。
    /// ティッカーの空回りを止めるためのラッチ
    stalled: bool,
    /// 末尾に到達して停止したか（次の再生で先頭へ戻す）
    pub ended: bool,
}

// Safety: Media Engine 自体はスレッドセーフだが、VideoPlayer は macOS 実装と同じく
// GPUI のメインスレッドコールバック内でのみ操作される前提で持ち回る。
// バックグラウンドスレッドからのアクセスは行わない。
unsafe impl Send for VideoPlayer {}

impl VideoPlayer {
    /// 動画ファイルからプレイヤーを初期化する。
    ///
    /// **ロード完了は待たない**（罠 1）。ただし壊れたファイル・非対応形式を
    /// 呼び出し側（MCP / CLI / 再生ボタン）へその場で返せるように、
    /// [`OPEN_WAIT`] のあいだだけ「エラーかメタデータ到着か」を待つ。
    pub fn open(path: &Path) -> Result<Self, String> {
        precheck(path)?;
        ensure_media_foundation()?;

        let events = Arc::new(EngineEvents::default());
        let (engine, notify) = create_engine(&events)?;

        unsafe {
            let url = BSTR::from(file_url(path));
            engine
                .SetSource(&url)
                .map_err(|e| format!("動画を読み込めない: {e}"))?;
        }

        // エラーは実測 2〜233ms で届く。ここで拾えれば MCP / CLI に理由を返せる
        let started = Instant::now();
        while started.elapsed() < OPEN_WAIT && !events.loaded() && events.error().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Some(message) = events.error() {
            return Err(message);
        }

        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| format!("画像バッファを用意できない: {e}"))?;

        let mut player = VideoPlayer {
            engine,
            _notify: notify,
            events,
            wic,
            bitmap: None,
            state: PlaybackState::Paused,
            duration: 0.0,
            width: 0,
            height: 0,
            current_bgra: Vec::new(),
            current_time: 0.0,
            frame_gen: 0,
            rate: 1.0,
            volume: 1.0,
            muted: false,
            looping: false,
            seek_pending: None,
            refresh_deadline: None,
            load_deadline: Instant::now() + LOAD_TIMEOUT,
            synced_after_load: false,
            has_video: true,
            stalled: false,
            ended: false,
        };

        // 最初のフレーム（ポスターフレーム）を取りに行く。`Play()` を呼ばなくても
        // メタデータが揃っていれば 1 枚出る（実測）。間に合わなければ
        // [`Self::needs_tick`] が true を返し続けてティッカーが拾う
        player.grab_frame();
        Ok(player)
    }

    /// 再生開始。末尾で停止している状態から押されたら先頭へ巻き戻してから再生する
    pub fn play(&mut self) {
        if self.state == PlaybackState::Playing {
            return;
        }
        if self.ended || at_end(self.current_time, self.duration) {
            self.seek(0.0);
        }
        unsafe {
            // ロードが defaultPlaybackRate で上書きするので、再生のたびに貼り直す（罠 2）
            let _ = self.engine.SetPlaybackRate(self.rate as f64);
            let _ = self.engine.Play();
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
            let _ = self.engine.Pause();
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
                let _ = self.engine.SetPlaybackRate(rate as f64);
            }
        }
    }

    /// 指定位置へシークする（秒）
    pub fn seek(&mut self, seconds: f64) {
        self.seek_with_tolerance(seconds, 0.0);
    }

    /// 許容誤差つきでシークする（秒）。
    ///
    /// Media Engine の `SetCurrentTime` は常に正確シークで、許容誤差という概念を持たない。
    /// tolerance は「ドラッグ中の粗いシークを許す」という**呼び出し側の意図**なので、
    /// シーク完了判定（[`seek_settled`]）の緩さとしてだけ使う。
    pub fn seek_with_tolerance(&mut self, seconds: f64, tolerance: f64) {
        let seconds = seek_target(seconds, self.duration);
        let tolerance = if tolerance.is_finite() {
            tolerance.max(0.0)
        } else {
            0.0
        };
        unsafe {
            let _ = self.engine.SetCurrentTime(seconds);
        }
        // シーク完了までは要求位置を見せる（実位置は非同期に追いつく）
        self.current_time = seconds;
        self.seek_pending = Some(SeekPending {
            target: seconds,
            tolerance,
            started: Instant::now(),
        });
        // 一時停止中でも新しい位置の絵に差し替える必要がある
        self.refresh_deadline = Some(Instant::now() + Duration::from_secs_f64(SEEK_SETTLE_TIMEOUT));
        self.ended = false;
    }

    /// 相対シーク（±秒。現在位置 + delta、0〜duration にクランプ）
    pub fn seek_relative(&mut self, delta: f64) {
        self.seek(self.current_time + delta);
    }

    /// ティッカーを回す必要があるか。
    ///
    /// 再生中・シーク後のフレーム取り直し待ちに加えて、**ロードと最初のフレームが
    /// 揃うまで**回す。Windows は読み込みが非同期で、初回は数秒かかることがあるため、
    /// ここを落とすと「開いたのに真っ黒のまま・総尺 0:00」で止まる（罠 1）。
    ///
    /// 「メタデータが来たか」だけを条件にすると足りない。**最初のフレームが
    /// 使えるようになるのはメタデータ到着と同時ではない**ので、そこで止めると
    /// ポスターフレームを取り逃してペインが黒いままになる。
    pub fn needs_tick(&self) -> bool {
        // これ以上何も来ないと判っているなら回さない（空回り防止）
        if self.stalled {
            return false;
        }
        if self.state == PlaybackState::Playing {
            return true;
        }
        if self
            .refresh_deadline
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            return true;
        }
        let waiting_for_first_frame = self.has_video && self.frame_gen == 0;
        (!self.synced_after_load || waiting_for_first_frame) && Instant::now() < self.load_deadline
    }

    /// 音量を設定（0.0〜1.0）
    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
        unsafe {
            let effective = if self.muted { 0.0 } else { self.volume };
            let _ = self.engine.SetVolume(effective as f64);
        }
    }

    /// ミュートのトグル
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        unsafe {
            let effective = if self.muted { 0.0 } else { self.volume };
            let _ = self.engine.SetVolume(effective as f64);
        }
    }

    /// ループ再生のトグル。
    ///
    /// Media Engine 自身の `SetLoop` は使わない。呼び出し側が `looping` フィールドを
    /// 直接書き換える経路があり（`main.rs` の `loop_on` / `loop_off`）、
    /// エンジン側の状態と二重管理になるためである。末尾での巻き戻しは
    /// [`Self::update_end_of_stream`] が macOS 実装とまったく同じ規則で行う。
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

    /// メタデータ到着後の一度きりの後始末。
    /// 総尺・解像度を取り込み、ロードに巻き戻された再生速度を貼り直す（罠 2）
    fn sync_after_load(&mut self) {
        if self.synced_after_load || !self.events.loaded() {
            return;
        }
        unsafe {
            let duration = sanitize_duration(Some(self.engine.GetDuration()));
            if duration > 0.0 {
                self.duration = duration;
            }
            let mut cx = 0u32;
            let mut cy = 0u32;
            if self
                .engine
                .GetNativeVideoSize(Some(&mut cx), Some(&mut cy))
                .is_ok()
                && cx > 0
                && cy > 0
            {
                self.width = cx;
                self.height = cy;
            }
            // 映像トラックの有無を確定する。音声だけの mp4 は永久にフレームが来ないので、
            // ここで判っておかないとティッカーが空回りする
            self.has_video = self.engine.HasVideo().as_bool();
            let _ = self.engine.SetPlaybackRate(self.rate as f64);
            let effective = if self.muted { 0.0 } else { self.volume };
            let _ = self.engine.SetVolume(effective as f64);
        }
        self.synced_after_load = true;

        // ロード完了前に受けたシーク要求は上限でクランプできていない。
        // 総尺が判ったので、行き過ぎていたら詰め直す
        if self.duration > 0.0 {
            let pending = self.seek_pending.as_ref().map(|p| (p.target, p.tolerance));
            if let Some((target, tolerance)) = pending {
                let clamped = clamp_time(target, self.duration);
                if (clamped - target).abs() > f64::EPSILON {
                    self.seek_with_tolerance(clamped, tolerance);
                }
            }
        }
    }

    /// 末尾到達の後始末。ループ中なら先頭へ戻し、そうでなければ再生状態を
    /// 停止へ落とす（macOS 実装と同じ規則）
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
                let _ = self.engine.SetPlaybackRate(self.rate as f64);
                let _ = self.engine.Play();
            }
        } else {
            unsafe {
                let _ = self.engine.Pause();
            }
            self.state = PlaybackState::Paused;
            self.ended = true;
            self.current_time = self.duration;
        }
    }

    fn grab_frame_inner(&mut self) -> bool {
        self.sync_after_load();

        // 読み込み中・再生中に落ちた場合（コーデック非対応など）は、ここで止める。
        // 放っておくとティッカーが延々と空振りする
        if let Some(message) = self.events.error() {
            if !self.stalled {
                // GUI には出す先が無い（再生ボタンの失敗も eprintln 止まり）ので、
                // せめて理由を診断ログへ残す。ペイン内容は含まないので出してよい
                eprintln!("動画の再生を中止した: {message}");
            }
            self.state = PlaybackState::Paused;
            self.refresh_deadline = None;
            self.stalled = true;
            return false;
        }
        // ロード時間切れ。ここまで来て総尺も解像度も判らないなら待っても無駄
        if !self.synced_after_load && Instant::now() >= self.load_deadline {
            self.stalled = true;
            return false;
        }

        unsafe {
            // 現在時刻を取得。シーク進行中は要求位置を維持し、実位置が追いついた
            // 時点で実位置へ切り替える（切り替えないとつまみが旧位置へ巻き戻る）
            let actual = {
                let t = self.engine.GetCurrentTime();
                if t.is_finite() && t >= 0.0 {
                    Some(t)
                } else {
                    None
                }
            };
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

            // 映像トラックが無い（音声だけの mp4）ならフレームは永久に来ない。
            // 位置と総尺の更新だけして帰る
            if !self.has_video {
                return false;
            }

            // 解像度が判ってから受け皿を作る（ロード完了前は 0x0 で作れない）。
            // `sync_after_load` は 1 回しか走らないので、そこで取り損ねていたら
            // ここで取り直す（取れないまま固定されるとフレームが永久に出ない）
            if self.width == 0 || self.height == 0 {
                let mut cx = 0u32;
                let mut cy = 0u32;
                if self
                    .engine
                    .GetNativeVideoSize(Some(&mut cx), Some(&mut cy))
                    .is_ok()
                    && cx > 0
                    && cy > 0
                {
                    self.width = cx;
                    self.height = cy;
                }
            }
            if self.bitmap.is_none() {
                if self.width == 0 || self.height == 0 {
                    return false;
                }
                self.bitmap = self
                    .wic
                    .CreateBitmap(
                        self.width,
                        self.height,
                        &GUID_WICPixelFormat32bppBGRA,
                        WICBitmapCacheOnLoad,
                    )
                    .ok();
            }
            let Some(bitmap) = self.bitmap.clone() else {
                return false;
            };

            // 新しいフレームがあるかは HRESULT でしか判らない（罠 3）。
            // S_OK = 新フレームあり / S_FALSE = 変化なし
            let mut pts = 0i64;
            let hr = (Interface::vtable(&self.engine).OnVideoStreamTick)(
                Interface::as_raw(&self.engine),
                &mut pts,
            );
            if hr.0 != 0 {
                // シーク直後は完了までフレームが来ない。取り直しは期限まで
                // 次のティックへ回す（時間切れなら諦めてティッカーを止める）
                if self
                    .refresh_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    self.refresh_deadline = None;
                }
                return false;
            }

            let rect = RECT {
                left: 0,
                top: 0,
                right: self.width as i32,
                bottom: self.height as i32,
            };
            if self
                .engine
                .TransferVideoFrame(&bitmap, None, &rect, None)
                .is_err()
            {
                return false;
            }
            if !self.copy_frame(&bitmap) {
                return false;
            }

            // シーク後の絵が届いたので取り直し要求を解除する（一時停止中の
            // ティッカーはこれで止まる）。シーク進行中なら次の絵まで続ける
            if self.seek_pending.is_none() {
                self.refresh_deadline = None;
            }
            true
        }
    }

    /// WIC ビットマップから BGRA を取り出して `current_bgra` へ移す。
    ///
    /// WIC の 1 行は `stride` バイトで、幅 × 4 とは限らない（右側にパディングが入る）。
    /// GPUI の RenderImage は詰まった BGRA を期待するので、行ごとに詰め直す。
    unsafe fn copy_frame(&mut self, bitmap: &IWICBitmap) -> bool {
        // 呼び出し経路では 0 にならないが、下の `height - 1` を安全にするため明示で弾く
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let rect = WICRect {
            X: 0,
            Y: 0,
            Width: self.width as i32,
            Height: self.height as i32,
        };
        let Ok(lock) = bitmap.Lock(&rect, WICBitmapLockRead.0 as u32) else {
            return false;
        };
        let stride = match lock.GetStride() {
            Ok(s) if s as usize >= self.width as usize * 4 => s as usize,
            _ => return false,
        };
        let mut size = 0u32;
        let mut ptr = std::ptr::null_mut();
        if lock.GetDataPointer(&mut size, &mut ptr).is_err() || ptr.is_null() {
            return false;
        }
        let row = self.width as usize * 4;
        let needed = stride * (self.height as usize - 1) + row;
        if (size as usize) < needed {
            return false;
        }
        let mut bgra = vec![0u8; row * self.height as usize];
        for y in 0..self.height as usize {
            let src = std::slice::from_raw_parts(ptr.add(y * stride), row);
            bgra[y * row..(y + 1) * row].copy_from_slice(src);
        }
        self.current_bgra = bgra;
        self.frame_gen += 1;
        true
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        unsafe {
            // Shutdown を呼ばなくても終了コードは 0 だったが（罠 4）、
            // ペインを閉じたあとも音が鳴り続けないよう明示的に止める
            let _ = self.engine.Pause();
            let _ = self.engine.Shutdown();
        }
    }
}

/// 開く前に同期的に済ませられる検査。
///
/// Media Engine は不在ファイルでもエラーを**非同期**で返すので、判る分は先に弾いて
/// 呼び出し側（MCP / CLI）へ理由の判る失敗を返す。
fn precheck(path: &Path) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Err("ディレクトリは再生できない".into()),
        Ok(meta) if meta.len() == 0 => Err("空のファイルは再生できない".into()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("動画ファイルを開けない: {e}")),
    }
}

/// シーク要求を実際に投げる位置へ直す。
///
/// **総尺が判る前は上限でクランプできない**のが Windows 固有の事情。`clamp_time` は
/// 長さ不明（0.0）を「0 秒の動画」と見なして**すべて 0.0 に潰す**ので、そのまま通すと
/// 「開いた直後の `tako video seek 4.0` が必ず先頭に飛ぶ」という壊れ方になる
/// （macOS は `AVAsset.duration` が同期で取れるのでこの問題が無い）。
///
/// ロード前は下限だけ効かせ、総尺が判った時点で [`VideoPlayer::sync_after_load`] が
/// 行き過ぎを詰め直す。
fn seek_target(requested: f64, duration: f64) -> f64 {
    if duration > 0.0 {
        clamp_time(requested, duration)
    } else if requested.is_finite() {
        requested.max(0.0)
    } else {
        0.0
    }
}

/// ローカルパスを Media Engine が受け取る `file:///` URL にする。
///
/// `SetSource` は URL しか解釈しないので、区切りを `/` に直して前置きを付ける。
/// UNC パス（`\\host\share\...`）は `file://host/share/...` になる。
///
/// ## 拡張長パス接頭辞（`\\?\`）を必ず落とす（重要）
///
/// tako はプレビューのパスを `canonicalize` して持つので、**実際に渡ってくるのは
/// ほぼ `\\?\C:\...` の形**である。素直に区切りだけ直すと `file://?/C:/...` になり、
/// URL の authority が `?` になって「そんなファイルは無い」で必ず失敗する
/// （隔離実測でこれを踏んだ。単体テストは canonicalize 前のパスを渡していたので通っていた）。
///
/// ## 逆にエスケープは要らない
///
/// 空白 / 日本語 / `#` を含むファイル名は**そのまま渡して再生できる**ことを実測した。
/// 余計にパーセントエンコードすると、こちらの方が壊れる。
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    // `\\?\UNC\host\share\...` は UNC パスの拡張長表記
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        return format!("file://{rest}");
    }
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    if let Some(rest) = text.strip_prefix("//") {
        format!("file://{rest}")
    } else {
        format!("file:///{text}")
    }
}

/// 通知コールバックを結び付けた Media Engine を作る。
///
/// コールバックの COM オブジェクトも一緒に返す（呼び出し側が engine より長く握るため）
fn create_engine(
    events: &Arc<EngineEvents>,
) -> Result<(IMFMediaEngine, IMFMediaEngineNotify), String> {
    unsafe {
        let notify: IMFMediaEngineNotify = Notify {
            state: events.clone(),
        }
        .into();
        let factory: IMFMediaEngineClassFactory =
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("動画再生エンジンを作成できない: {e}"))?;

        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2)
            .map_err(|e| format!("動画再生エンジンの設定を作成できない: {e}"))?;
        let attrs = attrs.ok_or_else(|| "動画再生エンジンの設定が空".to_string())?;
        attrs
            .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
            .map_err(|e| format!("動画再生エンジンの通知を設定できない: {e}"))?;
        // フレームサーバーモードの出力形式。GPUI が期待する BGRA に合わせる
        attrs
            .SetUINT32(
                &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            )
            .map_err(|e| format!("動画再生エンジンの出力形式を設定できない: {e}"))?;

        // 再生ウィンドウも DComp ビジュアルも渡さない = フレームサーバーモード
        let engine = factory
            .CreateInstance(0, &attrs)
            .map_err(|e| format!("動画再生エンジンを初期化できない: {e}"))?;
        Ok((engine, notify))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不在・ディレクトリ・空ファイルは OS の再生器へ渡す前に弾く
    /// （渡すと非同期のエラーイベント待ちになり、理由が返せない）
    #[test]
    fn 開く前の検査は不在とディレクトリと空を弾く() {
        let dir = std::env::temp_dir().join("tako-video-precheck-test");
        let _ = std::fs::create_dir_all(&dir);
        let empty = dir.join("empty.mp4");
        std::fs::write(&empty, b"").unwrap();
        let nonempty = dir.join("some.mp4");
        std::fs::write(&nonempty, b"not really a video").unwrap();

        assert!(precheck(&dir.join("no-such.mp4")).is_err());
        assert!(precheck(&dir).is_err());
        assert!(precheck(&empty).is_err());
        // 中身の妥当性はここでは見ない（OS の再生器の仕事）
        assert!(precheck(&nonempty).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `SetSource` は URL しか解釈しない。区切りの向きを間違えると
    /// 「ファイルが無い」で必ず失敗するので、変換規則をテストで固定する
    #[test]
    fn ローカルパスはfile_urlになる() {
        assert_eq!(
            file_url(Path::new(r"C:\Users\x\clip.mp4")),
            "file:///C:/Users/x/clip.mp4"
        );
        // 空白・`+`・日本語・`#` はそのまま通す（エスケープすると逆に壊れると実測）
        assert_eq!(
            file_url(Path::new(r"D:\a b\c+d.mp4")),
            "file:///D:/a b/c+d.mp4"
        );
        assert_eq!(
            file_url(Path::new(r"C:\動画\hash#tag.mp4")),
            "file:///C:/動画/hash#tag.mp4"
        );
        // UNC パスはホスト名が権限部に来る
        assert_eq!(
            file_url(Path::new(r"\\host\share\clip.mp4")),
            "file://host/share/clip.mp4"
        );
    }

    /// **拡張長パス接頭辞（`\\?\`）を落とすこと**。
    ///
    /// tako はプレビューのパスを canonicalize して持つのでこの形が実際に渡ってくる。
    /// 落とさないと `file://?/C:/...` になり authority が `?` で必ず失敗する
    /// （隔離実測で踏んだ実バグの回帰テスト。単体テストは canonicalize 前のパスを
    /// 渡していたので取り逃していた）。
    #[test]
    fn 拡張長パス接頭辞は落ちる() {
        assert_eq!(
            file_url(Path::new(r"\\?\C:\Users\x\clip.mp4")),
            "file:///C:/Users/x/clip.mp4"
        );
        assert_eq!(
            file_url(Path::new(r"\\?\UNC\host\share\clip.mp4")),
            "file://host/share/clip.mp4"
        );
        // 接頭辞つき / 無しで同じ URL になる = 正規化されている
        assert_eq!(
            file_url(Path::new(r"\\?\D:\a b\clip.mp4")),
            file_url(Path::new(r"D:\a b\clip.mp4"))
        );
    }

    /// ロード完了前のシークが先頭へ潰れないこと。
    ///
    /// 総尺不明（0.0）で `clamp_time` をそのまま通すと全部 0.0 になり、
    /// 「開いた直後の `tako video seek 4.0` が必ず先頭へ飛ぶ」壊れ方をする
    #[test]
    fn 総尺不明のシークは先頭へ潰れない() {
        // 総尺不明: 要求値をそのまま通す（負値だけ 0 へ）
        assert_eq!(seek_target(4.0, 0.0), 4.0);
        assert_eq!(seek_target(0.0, 0.0), 0.0);
        assert_eq!(seek_target(-3.0, 0.0), 0.0);
        assert_eq!(seek_target(f64::NAN, 0.0), 0.0);
        assert_eq!(seek_target(f64::INFINITY, 0.0), 0.0);
        // 総尺が判っていれば従来どおりクランプする
        assert_eq!(seek_target(4.0, 6.0), 4.0);
        assert_eq!(seek_target(99.0, 6.0), 6.0);
        assert_eq!(seek_target(-1.0, 6.0), 0.0);
    }

    /// エラー文言は種別と拡張コードの両方を出す（切り分けに要る）
    #[test]
    fn エラー文言は種別と拡張コードを含む() {
        assert!(error_message(4, 0xC00D36C4).contains("再生できない"));
        assert!(error_message(4, 0xC00D36C4).contains("0xC00D36C4"));
        assert!(error_message(3, 0).contains("デコード"));
        assert!(!error_message(3, 0).contains("0x"));
        // 未知のコードでも panic しない
        assert!(!error_message(99, 0).is_empty());
    }

    /// 実際に mp4 を開いて再生・シーク・音量が動くこと。
    ///
    /// 素材は **ffmpeg を使わず OS の Media Foundation エンコーダで生成**する
    /// （`tests/support/video_fixture.rs`）。CI の Windows ランナーでも同じ手が使える
    #[test]
    fn 実mp4を開いて再生シーク音量が動く() {
        let Some(path) = crate::platform::video::test_fixture::sample_mp4() else {
            eprintln!("検証用 mp4 を生成できないためスキップ");
            return;
        };
        // **canonicalize した形で開く**。tako はプレビューのパスをこの形で持っており
        // （Windows では `\\?\C:\...`）、生パスだけで通していると
        // `file_url` の接頭辞バグを取り逃す（実際に隔離実測で踏んだ）
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let mut player = VideoPlayer::open(&path).expect("mp4 を開けること");

        // ロード完了まで待つ（初回はデコーダ初期化ぶん時間がかかる。罠 1）
        let start = Instant::now();
        while player.needs_tick() && start.elapsed() < Duration::from_secs(20) {
            player.grab_frame();
            if player.duration > 0.0 && player.frame_gen > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            player.duration > 1.0,
            "総尺が取れること: {}",
            player.duration
        );
        assert!(player.width > 0 && player.height > 0, "解像度が取れること");
        assert_eq!(
            player.current_bgra.len(),
            player.width as usize * player.height as usize * 4,
            "フレームが幅 x 高さ x 4 バイトで詰まっていること"
        );

        // 音量
        player.set_volume(0.42);
        assert!((player.volume - 0.42).abs() < 1e-6);
        player.set_volume(1.5);
        assert_eq!(player.volume, 1.0, "1.0 を超えた要求はクランプされること");
        // このあと実際に再生するので、テスト実行中に音を出さないよう 0 にしておく
        player.set_volume(0.0);
        assert_eq!(player.volume, 0.0);

        // 再生 → 位置が進む
        player.play();
        assert_eq!(player.state, PlaybackState::Playing);
        let before = player.current_time;
        let start = Instant::now();
        while player.current_time <= before + 0.1
            && !player.stalled
            && start.elapsed() < Duration::from_secs(10)
        {
            player.grab_frame();
            std::thread::sleep(Duration::from_millis(20));
        }
        if player.stalled {
            // OS が再生を拒んだ（音声出力デバイスが無い CI ランナー等）。
            // 「開いてフレームを取る」ところまでは上で検証済みなので、
            // ここから先は環境が再生できるときだけ意味を持つ
            eprintln!(
                "OS が再生できない環境のため以降をスキップ: {:?}",
                player.events.error()
            );
            return;
        }
        assert!(
            player.current_time > before,
            "再生で位置が進むこと: {before} -> {}",
            player.current_time
        );

        // 一時停止 → 位置が止まる
        player.pause();
        assert_eq!(player.state, PlaybackState::Paused);
        let paused_at = player.current_time;
        std::thread::sleep(Duration::from_millis(300));
        player.grab_frame();
        assert!(
            (player.current_time - paused_at).abs() < 0.3,
            "一時停止で位置が進まないこと: {paused_at} -> {}",
            player.current_time
        );

        // シーク → 絵が変わる
        let before_frame = player.current_bgra.clone();
        let target = (player.duration * 0.75).min(player.duration - 0.2);
        player.seek(target);
        assert!((player.current_time - target).abs() < 0.01);
        let start = Instant::now();
        while player.current_bgra == before_frame && start.elapsed() < Duration::from_secs(10) {
            player.grab_frame();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(player.current_bgra, before_frame, "シークで絵が変わること");
    }

    /// 音声トラックだけの mp4 でも開けて、**ティッカーが空回りしない**こと。
    ///
    /// フレームは永久に来ないので、`needs_tick()` が「最初のフレーム待ち」を理由に
    /// true を返し続けると 30 秒ぶん無駄に回る（`has_video` を見ていないと起きる）
    #[test]
    fn 音声だけのmp4はフレーム待ちで空回りしない() {
        let Some(path) = crate::platform::video::test_fixture::audio_only_mp4() else {
            eprintln!("検証用 mp4 を生成できないためスキップ");
            return;
        };
        let mut player = VideoPlayer::open(&path).expect("音声だけの mp4 も開けること");

        // メタデータ到着まで回す
        let start = Instant::now();
        while !player.synced_after_load && start.elapsed() < Duration::from_secs(20) {
            player.grab_frame();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(player.synced_after_load, "メタデータが届くこと");
        assert!(
            player.duration > 1.0,
            "総尺が取れること: {}",
            player.duration
        );
        assert!(!player.has_video, "映像トラックが無いと判定されること");
        assert_eq!(player.frame_gen, 0, "フレームは来ないこと");
        assert!(
            !player.needs_tick(),
            "一時停止中はティッカーを回さないこと（空回り防止）"
        );
    }

    /// 動画として読めないファイルは「`open()` が失敗する」か
    /// 「開けても再生に至らず停止する」かの**どちらかになる**
    /// （黙って真っ黒のまま回り続けない）。
    ///
    /// **どちらになるかは環境とタイミング次第**である。Media Engine のエラーは非同期で、
    /// 判定が [`OPEN_WAIT`] と競争になる（単体で走らせれば数〜200ms で返るが、
    /// テストを並列実行して CPU が埋まっていると 500ms を超える）。
    /// そこで「どちらか」を契約として固定する。`stalled` が立たないと
    /// ティッカーが [`LOAD_TIMEOUT`] ぶん空回りしてしまう。
    ///
    /// 同期的に判る入力（不在 / ディレクトリ / 空）のほうは
    /// [`tests::開く前の検査は不在とディレクトリと空を弾く`] が厳密に見ている。
    #[test]
    fn 読めないファイルは開けないか再生に至らないかのどちらかになる() {
        let Some(good) = crate::platform::video::test_fixture::sample_mp4() else {
            eprintln!("検証用 mp4 を生成できないためスキップ");
            return;
        };
        let dir = good.parent().unwrap();
        let bytes = std::fs::read(&good).unwrap();

        // 冒頭だけ残した mp4 / 半端に切った mp4 / そもそも中身が動画でないファイル
        let mut cases: Vec<(String, Vec<u8>)> = vec![(
            "notavideo.mp4".to_string(),
            b"this is not a video at all".to_vec(),
        )];
        for cut in [2048usize, bytes.len() / 8] {
            cases.push((
                format!("partial-{cut}.mp4"),
                bytes[..cut.min(bytes.len())].to_vec(),
            ));
        }

        for (name, content) in cases {
            let path = dir.join(&name);
            std::fs::write(&path, &content).unwrap();

            let Ok(mut player) = VideoPlayer::open(&path) else {
                // open() の時点でエラーになった = 望ましい側
                continue;
            };
            let start = Instant::now();
            while !player.stalled && start.elapsed() < Duration::from_secs(20) {
                player.grab_frame();
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                player.stalled,
                "{name}: 遅れて届いたエラーで停止すること（ティッカーの空回り防止）"
            );
            assert!(!player.needs_tick(), "{name}: 以後回さないこと");
            assert_eq!(player.frame_gen, 0, "{name}: フレームは出ないこと");
        }
    }
}
