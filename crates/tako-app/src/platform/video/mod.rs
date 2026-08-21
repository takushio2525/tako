//! 抽象境界 B12 後半（動画プレイヤー）。
//!
//! 呼び出し側（`main.rs` の `ensure_video_player` / dispatch の `video_playback` /
//! `video_seek` / `video_volume`、`preview_render` の描画）が知ってよいのは
//! [`VideoPlayer`] だけで、**`#[cfg(target_os)]` を書いてよいのは
//! このファイルの実装選択 1 箇所に限る**（`.agent/plans/2026-07-windows-port-architecture.md` 原則 1）。
//!
//! ## プラットフォームごとの実体（#521）
//!
//! | | 再生器 | フレームの取り出し口 | 音声出力 |
//! |---|---|---|---|
//! | macOS | AVPlayer | AVPlayerItemVideoOutput → CVPixelBuffer | AVPlayer |
//! | Windows | IMFMediaEngine（フレームサーバーモード） | TransferVideoFrame → IWICBitmap | Media Engine |
//! | その他 | 無し | 無し | 無し |
//!
//! どちらも **OS 標準の再生器**で、tako は追加の再頒布物を持たない（PDF が
//! `Windows.Data.Pdf` を選んだのと同じ判断。選定の比較は #521 のコメント）。
//! 対価として**再生できるコーデックは OS 依存**になる。
//!
//! ## 「同じ API が生えている」ことをどう担保するか
//!
//! [`VideoPlayer`] は trait ではなく、実装ごとの構造体を `pub use` で選ぶ形にしている。
//! 動画プレイヤーは公開フィールド（`current_bgra` / `current_time` / `looping` …）を
//! 呼び出し側が直接読み書きしており、trait にすると getter / setter が 20 個以上並んで
//! 呼び出し側を書き換えることになるからである。代わりに、
//! [`tests::どのプラットフォームでも同じapiが生えている`] が全メソッドを 1 度ずつ呼び、
//! **署名がずれたらコンパイルエラーになる**形で固定している。

#[cfg(target_os = "macos")]
mod macos;
/// 検証用 mp4 を OS のエンコーダで作る（テスト専用）
#[cfg(all(test, target_os = "windows"))]
mod test_fixture;
#[cfg(target_os = "windows")]
mod windows;

// 実装選択（cfg を書いてよい唯一の場所）
#[cfg(target_os = "macos")]
pub use macos::VideoPlayer;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::VideoPlayer;
#[cfg(target_os = "windows")]
pub use windows::VideoPlayer;

/// 動画プレイヤーを持たない環境（macOS / Windows 以外）。
///
/// 呼び出し側は `open()` のエラーだけを見ればよいので、あとは何もしない形でよい。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use crate::video_player::PlaybackState;

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

    impl VideoPlayer {
        pub fn open(_path: &std::path::Path) -> Result<Self, String> {
            Err("この環境には動画プレイヤーが無いため再生できない".into())
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// この境界を通す意味は「呼び出し側から cfg を消す」ことにある。
    /// どのプラットフォームでも同じメソッドが同じ署名で生えていることを、
    /// **実際に全部呼ぶ**ことでコンパイル時に固定する（署名がずれたらここで落ちる）
    #[test]
    fn どのプラットフォームでも同じapiが生えている() {
        let missing = std::path::Path::new("no-such-file-for-parity-check.mp4");
        // 不在ファイルなので必ず失敗する。ここで見たいのは「呼べること」だけ
        let Ok(mut player) = VideoPlayer::open(missing) else {
            return;
        };
        player.play();
        player.pause();
        player.toggle();
        player.set_rate(1.5);
        player.seek(1.0);
        player.seek_with_tolerance(1.0, 0.1);
        player.seek_relative(-1.0);
        player.set_volume(0.5);
        player.toggle_mute();
        player.toggle_loop();
        let _: bool = player.needs_tick();
        let _: bool = player.grab_frame();
        // 呼び出し側が直接読み書きする公開フィールド
        let _: (f64, f64, u32, u32, u64, f32, f32) = (
            player.duration,
            player.current_time,
            player.width,
            player.height,
            player.frame_gen,
            player.rate,
            player.volume,
        );
        let _: (bool, bool, bool) = (player.muted, player.looping, player.ended);
        let _: &[u8] = &player.current_bgra;
        player.muted = true;
        player.looping = true;
    }
}
