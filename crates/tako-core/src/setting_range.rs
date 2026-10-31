//! setting_range — CLI / MCP で明示した設定値が範囲の外のときの断り（Issue #2365）
//!
//! `preview_cache_bytes`（`tako preview-cache`）・`scrollback::validate_lines`（`tako scrollback`）・
//! `redraw_limit::validate_fps`（`tako redraw-limit`）は範囲外を黙って丸めずに理由を返す。その文の
//! 日英の対をここ 1 か所に置く（形は `user_task::TaskRefusal` と同じ (バリアント, 言語) の網羅 match）。
//! 検証の口は `Result<_, String>` のまま、文は表示（`Display`）が表示言語のグローバルに従う
//! （呼び手は dispatch だけ = 枠の `invalid parameter:` と同じ言語になる。tako-app の起動時の読み込みは
//! 文を捨てて既定へ倒す）。範囲の値は各モジュールの定数が正本

use crate::i18n::Lang;
use crate::{preview_cache, redraw_limit, scrollback};

/// 範囲外の設定値の断り
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeRefusal {
    /// プレビュー画像キャッシュの上限（MB）
    PreviewCacheMb,
    /// 直接ペインのスクロールバックの保持行数
    ScrollbackLines,
    /// フォーカスの無いペインの再描画の上限（fps）
    UnfocusedFps,
}

impl RangeRefusal {
    /// (バリアント, 言語) の網羅 match。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use preview_cache::{PREVIEW_CACHE_MAX_MB, PREVIEW_CACHE_MIN_MB};
        use redraw_limit::{
            DEFAULT_UNFOCUSED_FPS, FOCUSED_FPS, MAX_UNFOCUSED_FPS, MIN_UNFOCUSED_FPS,
        };
        use scrollback::{DEFAULT_LINES, MAX_LINES, MIN_LINES};
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::PreviewCacheMb, Ja) => {
                format!("max_mb は {PREVIEW_CACHE_MIN_MB}〜{PREVIEW_CACHE_MAX_MB} の範囲で指定する")
            }
            (Self::PreviewCacheMb, En) => {
                format!("max_mb must be between {PREVIEW_CACHE_MIN_MB} and {PREVIEW_CACHE_MAX_MB}")
            }
            (Self::ScrollbackLines, Ja) => {
                format!(
                    "lines は {MIN_LINES}〜{MAX_LINES} の範囲で指定する（既定 {DEFAULT_LINES}）"
                )
            }
            (Self::ScrollbackLines, En) => format!(
                "lines must be between {MIN_LINES} and {MAX_LINES} (default {DEFAULT_LINES})"
            ),
            (Self::UnfocusedFps, Ja) => format!(
                "unfocused_fps は {MIN_UNFOCUSED_FPS}〜{MAX_UNFOCUSED_FPS} の範囲で指定する\
                 （既定 {DEFAULT_UNFOCUSED_FPS}。フォーカス中のペインは常に {FOCUSED_FPS}）"
            ),
            (Self::UnfocusedFps, En) => format!(
                "unfocused_fps must be between {MIN_UNFOCUSED_FPS} and {MAX_UNFOCUSED_FPS} \
                 (default {DEFAULT_UNFOCUSED_FPS}; the focused pane always uses {FOCUSED_FPS})"
            ),
        }
    }
}

impl std::fmt::Display for RangeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}
