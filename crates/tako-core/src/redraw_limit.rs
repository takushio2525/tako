//! redraw_limit — フォーカスの無いペインの出力による再描画の上限（Issue #1979）
//!
//! 本番の tako-app の CPU 約 12% の正体は、エージェントの出力による描画（約 37 fps）
//! だった（蓋を閉じて描画が止まると約 5%）。出力を見ているのはたいていフォーカス中の
//! 1 枚で、隣に並べた worker のペインや裏のタブの題名の点滅を 60 fps で描き直す理由は薄い。
//!
//! そこで**フォーカスの無いペイン**（同じタブの別のペイン・裏のタブの題名 / OSC の変化）の
//! 出力による再描画だけに上限を設ける。**フォーカス中のペインは今のまま**（約 60 fps =
//! 16ms 間隔）。上限は設定で変えられ（`settings.json` の `unfocused_redraw_fps`）、
//! CLI `tako redraw-limit [fps]` / MCP `tako_scrollback` の `unfocused_fps` から読み書きできる。
//!
//! 値の正本（既定・下限・上限・間隔の換算）をここ 1 か所に置く。UI / CLI / MCP に散らさない
//! （`scrollback` と同じ形）。

use std::time::Duration;

/// フォーカス中のペインの再描画の上限（fps）。従来の固定値（16ms 間隔）と同じで、変えない
pub const FOCUSED_FPS: u32 = 60;

/// フォーカスの無いペインの再描画の既定の上限（fps）。
///
/// 根拠: Issue #1979 の判断（30 fps 以下）。テキストの流れと spinner の動きは 30 fps でも
/// 途切れて見えない（Claude Code の spinner の 1 コマは約 100ms）
pub const DEFAULT_UNFOCUSED_FPS: u32 = 30;

/// 下限。1 fps = 1 秒に 1 回は描き直す（それ未満は「映っているのに止まっている」に見える）
pub const MIN_UNFOCUSED_FPS: u32 = 1;

/// 上限。フォーカス中と同じ 60 fps を超えて描いても、フォーカス中より速くはならない
pub const MAX_UNFOCUSED_FPS: u32 = FOCUSED_FPS;

/// 明示指定（CLI / MCP）の検証。範囲外は**黙って丸めず**理由を返す
/// （打った値と効く値がずれたまま進むほうが分かりにくい。`scrollback::validate_lines` と同じ）
pub fn validate_fps(fps: u32) -> Result<u32, String> {
    if !(MIN_UNFOCUSED_FPS..=MAX_UNFOCUSED_FPS).contains(&fps) {
        return Err(format!(
            "unfocused_fps は {MIN_UNFOCUSED_FPS}〜{MAX_UNFOCUSED_FPS} の範囲で指定する\
             （既定 {DEFAULT_UNFOCUSED_FPS}。フォーカス中のペインは常に {FOCUSED_FPS}）"
        ));
    }
    Ok(fps)
}

/// 永続ファイル由来の値の丸め。`settings.json` は手で書けるので、どんな値でも
/// **起動は必ず成立させる**。0 は「書いていない」とみなして既定へ、上限超えは上限へ
pub fn clamp_fps(fps: u32) -> u32 {
    match fps {
        0 => DEFAULT_UNFOCUSED_FPS,
        fps => fps.min(MAX_UNFOCUSED_FPS),
    }
}

/// fps → 再描画の最小間隔。60 fps は従来の 16ms と同じになるよう、ミリ秒で切り捨てる
pub fn interval(fps: u32) -> Duration {
    Duration::from_millis(u64::from(1000 / clamp_fps(fps)))
}

/// 再描画の上限の現在状態（CLI / MCP の応答の材料）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedrawLimitStatus {
    /// フォーカスの無いペインの上限（fps）
    pub unfocused_fps: u32,
    /// フォーカス中のペインの出力による再描画の回数（起動からの累計）
    pub focused_flushes: u64,
    /// フォーカスの無いペインの出力による再描画の回数（起動からの累計）
    pub unfocused_flushes: u64,
    /// ペイン本体が実際に描かれた回数（起動からの累計。上の 2 つは「描いて」と頼んだ回数で、
    /// こちらはフレームの中で本当に描いた回数 = 面が眠っていれば増えない）
    pub body_renders: u64,
    /// A/B（`TAKO_1979_LEGACY=1`）で上限を設けていないか
    pub legacy: bool,
}

impl Default for RedrawLimitStatus {
    fn default() -> Self {
        Self {
            unfocused_fps: DEFAULT_UNFOCUSED_FPS,
            focused_flushes: 0,
            unfocused_flushes: 0,
            body_renders: 0,
            legacy: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 明示指定は範囲外を理由つきで弾く() {
        assert_eq!(
            validate_fps(DEFAULT_UNFOCUSED_FPS),
            Ok(DEFAULT_UNFOCUSED_FPS)
        );
        assert_eq!(validate_fps(MIN_UNFOCUSED_FPS), Ok(MIN_UNFOCUSED_FPS));
        assert_eq!(validate_fps(MAX_UNFOCUSED_FPS), Ok(MAX_UNFOCUSED_FPS));
        for bad in [0, MAX_UNFOCUSED_FPS + 1, u32::MAX] {
            let err = validate_fps(bad).expect_err("範囲外は弾く");
            assert!(err.contains("1〜60"), "範囲を言う: {err}");
        }
    }

    #[test]
    fn 設定ファイルの値はどんな値でも起動を止めない() {
        assert_eq!(clamp_fps(0), DEFAULT_UNFOCUSED_FPS, "0 は書いていない扱い");
        assert_eq!(clamp_fps(1), 1);
        assert_eq!(clamp_fps(30), 30);
        assert_eq!(clamp_fps(61), MAX_UNFOCUSED_FPS);
        assert_eq!(clamp_fps(u32::MAX), MAX_UNFOCUSED_FPS);
    }

    #[test]
    fn 間隔はfpsの逆数でフォーカス中と同じ60は従来の16ms() {
        assert_eq!(interval(FOCUSED_FPS), Duration::from_millis(16));
        assert_eq!(interval(DEFAULT_UNFOCUSED_FPS), Duration::from_millis(33));
        assert_eq!(interval(1), Duration::from_millis(1000));
        // 範囲外を渡しても 0 除算・0 間隔にならない
        assert_eq!(interval(0), interval(DEFAULT_UNFOCUSED_FPS));
        assert_eq!(interval(u32::MAX), Duration::from_millis(16));
        assert!(interval(DEFAULT_UNFOCUSED_FPS) > interval(FOCUSED_FPS));
    }
}
