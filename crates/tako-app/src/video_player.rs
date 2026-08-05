//! 動画プレイヤーの**プラットフォーム非依存な部分**。
//!
//! OS を直接叩く再生器そのものは抽象境界 **B12**（[`crate::platform::video`]）にあり、
//! ここから `pub use` で再輸出している。呼び出し側（`main.rs` / `preview_render.rs` /
//! `drawer.rs`）は従来どおり `video_player::VideoPlayer` を使い、
//! **`#[cfg(target_os)]` を書かない**（設計原則 1）。
//!
//! | | 再生器の実体 |
//! |---|---|
//! | macOS | AVPlayer + AVPlayerItemVideoOutput（AVFoundation） |
//! | Windows | IMFMediaEngine のフレームサーバーモード（Media Foundation。#521） |
//! | その他 | 無し（開こうとすると理由つきで失敗する） |
//!
//! このファイルに残しているのは、**どちらの OS でも同じ答えになる計算**
//! （クランプ・進捗率・時刻表記・シーク完了判定・末尾判定）と、その単体テスト。
//! 両実装がここを共有するので、シークバーの当たり判定や末尾の扱いが
//! OS ごとにずれることが構造的に起きない。

pub use crate::platform::video::VideoPlayer;

/// 動画プレイヤーの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Paused,
    Playing,
}

// --- シーク・表示まわりの純粋計算（プラットフォーム非依存・単体テスト対象）---

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
/// 非有限）は None を返す。indefinite（生放送等）もここで弾かれる。
///
/// 使うのは macOS 実装だけ（Media Foundation は秒を f64 で直接返す）。それでも
/// ここに置いているのは、**中身が純粋計算で単体テストがどの OS でも回る**ため
/// （境界の内側に移すと macOS でしか検証できなくなる）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
