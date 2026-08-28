//! 読み込み中インジケータ（#1010）
//!
//! **無限アニメーションを新設しない**（#945 / #1012 で退治している系統）。GPUI の
//! `AnimationElement` は動いているあいだ毎フレーム `request_animation_frame()` を
//! 呼ぶので、`repeat()` を「終わるか分からない状態」に紐づけると
//! **永久にフレームを要求し続ける**（#786 / #801 / #803 で削った毎フレームの固定費が
//! 復活する）。ここは有限回で終わる 1 本のアニメーションにして、
//! 回り終わったあとは**静止した弧**として残す（読み込み中であること自体は伝わる）。
//!
//! 出る条件そのものが状態（読み込み中 / 接続中）なので、終われば要素ごと消える。
//! 次に読み込みが始まったときは element state が作り直されて回転もやり直しになる。

use std::time::Duration;

use gpui::{
    percentage, svg, Animation, AnimationExt, AnyElement, ElementId, Hsla, IntoElement, Pixels,
    Styled, Transformation,
};

/// 1 回転の長さ
const TURN: Duration = Duration::from_millis(900);
/// 何回転で止めるか。SFTP の 1 往復は実測 1〜2 秒（#966）なので、
/// 通常の読み込みは回っているあいだに終わる
const TURNS: u32 = 12;

/// 回り続ける上限（= これを過ぎると静止する）
pub(crate) fn spin_total() -> Duration {
    TURN * TURNS
}

/// 回転が 1 フレーム計算された回数（セルフテスト用。単調増加）
static SPIN_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 最後に計算した回転角（0.0〜TURNS。f32 のビット列）
static SPIN_LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn record(turns: f32) -> f32 {
    use std::sync::atomic::Ordering::Relaxed;
    SPIN_FRAMES.fetch_add(1, Relaxed);
    SPIN_LAST.store(turns.to_bits(), Relaxed);
    turns
}

/// 観測値（計算フレーム数, 最後の回転角）。
///
/// 「時間を空けて描き直しても角度が動かない」= 完了していて**フレーム要求も
/// 止まっている**、と言い切れる（#945 の `dot_pulse_probe` と同じ作法。
/// 画面の有無に依存せず A/B が取れる）
pub(crate) fn spin_probe() -> (u64, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        SPIN_FRAMES.load(Relaxed),
        f32::from_bits(SPIN_LAST.load(Relaxed)),
    )
}

/// 回る弧。`id` は同時に複数出るので呼び出し側が一意にする
pub(crate) fn spinner(id: impl Into<ElementId>, size: Pixels, color: Hsla) -> AnyElement {
    svg()
        .path(crate::file_icons::ui_icon::SPINNER)
        .size(size)
        .flex_none()
        .text_color(color)
        .with_animation(id, Animation::new(spin_total()), |el, t| {
            // `t` は 0.0〜1.0 なので回転数を掛ける。**両端が整数回転**になるので
            // 止まった瞬間に角度が飛ばない。
            //
            // `percentage()` は **0.0〜1.0 しか受け取らない**（範囲外は gpui の
            // `debug_assert!` で panic = アプリごと abort する。実機の初回描画で
            // 踏んだ）ので、渡すのは 1 回転ぶんの端数だけにする
            let turns = record(t * TURNS as f32);
            el.with_transformation(Transformation::rotate(percentage(turns.fract())))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **無限アニメーションを新設しない**（#945 / #1012）の機械検査。
    /// `repeat()` を書き足すとここで落ちる
    #[test]
    fn 回転に無限リピートを使っていない() {
        let src = include_str!("spinner.rs");
        // 自分自身の検査文字列に当たらないよう、テストモジュールより前だけを見る
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            !body.contains(".repeat()"),
            "スピナーに `repeat()` を使うと、読み込みが終わるまで毎フレーム \
             `request_animation_frame()` を呼び続ける（#945 / #1012 で退治した系統）"
        );
    }

    /// `percentage()` は 0.0〜1.0 の外で panic する（gpui の `debug_assert!`）。
    /// **実機の初回描画でアプリごと abort した**ので、端数変換を機械検査で固定する
    #[test]
    fn 回転角はpercentageの受け取れる範囲に収まる() {
        for i in 0..=1000 {
            let t = i as f32 / 1000.0;
            let frac = (t * TURNS as f32).fract();
            assert!(
                (0.0..=1.0).contains(&frac),
                "t={t} frac={frac} が percentage() の範囲外"
            );
        }
    }

    #[test]
    fn 有限回で止まる長さになっている() {
        // 無限（repeat）にしないための不変条件。SFTP 1 往復（実測 1〜2 秒）より
        // 十分長く、かつ「永久に回る」ことはない
        assert_eq!(spin_total(), Duration::from_millis(900) * 12);
        assert!(spin_total() >= Duration::from_secs(5));
        assert!(spin_total() <= Duration::from_secs(60));
    }
}
