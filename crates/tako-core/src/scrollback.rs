//! scrollback — 直接ペインのスクロールバック保持上限（Issue #818）
//!
//! 直接ペイン（tmux バックエンドを使わないペイン）の履歴は alacritty の
//! `Grid` が持つ。`Row<Cell>` は**列数ぶんのセルを常に確保**するので、
//! 履歴が飽和したペインは `行数 × 桁数 × 24 バイト`（実測で理論値と 15% 以内に
//! 一致）を保持し続ける。119 桁 10,000 行で +33 MB / ペイン。
//!
//! 既定は据え置きの 10,000 行。軽量運用（persist OFF / tmux 不在環境）向けに
//! **設定で下げられる**ようにするのがこのモジュールの役目で、値の正本
//! （既定・下限・上限・見積り）をここ 1 か所に置く。判定を UI / CLI / MCP へ
//! 散らさない。

/// 既定の保持行数（従来の固定値と同じ）
pub const DEFAULT_LINES: usize = 10_000;

/// 下限。1 画面ぶんの遡りすら残らない値は「スクロールバック無し」と変わらず、
/// ペインログ（#112）の増分取り込みも取りこぼしやすくなるので受け付けない
pub const MIN_LINES: usize = 100;

/// 上限。200 桁で 100,000 行 = 約 480 MB / ペインに達する。
/// これ以上は「設定できる」こと自体が事故なので弾く
pub const MAX_LINES: usize = 100_000;

/// `alacritty_terminal::term::cell::Cell` 1 個のバイト数。
/// `c: char` 4 + `fg` 4 + `bg` 4 + `flags: u16` 2 + `extra: Option<Arc<_>>` 8 →
/// アラインで 24（#818 の実測がこの値で理論値と一致した）
pub const BYTES_PER_CELL: usize = 24;

/// 直接ペインのスクロールバックの現在状態（CLI / MCP の応答の材料）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbackStatus {
    /// 現在の保持上限（行）
    pub lines: usize,
    /// 上限を適用している生存中の直接ペイン数
    pub panes: usize,
    /// 生存中ペインの最大桁数（見積りの材料。ペインが無ければ 0）
    pub max_cols: usize,
}

impl Default for ScrollbackStatus {
    fn default() -> Self {
        Self {
            lines: DEFAULT_LINES,
            panes: 0,
            max_cols: 0,
        }
    }
}

/// 明示指定（CLI / MCP / 設定画面）の検証。範囲外は**黙って丸めず**理由を返す。
/// ユーザーが打った値と実際に効く値がずれたまま進むほうが分かりにくい
pub fn validate_lines(lines: usize) -> Result<usize, String> {
    if !(MIN_LINES..=MAX_LINES).contains(&lines) {
        return Err(format!(
            "lines は {MIN_LINES}〜{MAX_LINES} の範囲で指定する（既定 {DEFAULT_LINES}）"
        ));
    }
    Ok(lines)
}

/// 永続ファイル由来の値の丸め。`settings.json` は手で書けるので、
/// 0 や桁違いの値が入っていても**起動は必ず成立させる**（`pane_log_config` と同じ方針）
pub fn clamp_lines(lines: usize) -> usize {
    lines.clamp(MIN_LINES, MAX_LINES)
}

/// 飽和時に 1 ペインが保持するバイト数の見積り（`行 × 桁 × 24 B`）。
/// malloc のバケット丸めと `Row` 構造体ぶんは含まないので実測はこれより
/// 1 割ほど大きい（#818 の実測: 理論 28.6 MB / 実測 +33 MB）
pub fn estimated_bytes(lines: usize, cols: usize) -> u64 {
    (lines as u64) * (cols as u64) * (BYTES_PER_CELL as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 明示指定は範囲外を理由つきで弾く() {
        assert_eq!(validate_lines(DEFAULT_LINES), Ok(DEFAULT_LINES));
        assert_eq!(validate_lines(MIN_LINES), Ok(MIN_LINES));
        assert_eq!(validate_lines(MAX_LINES), Ok(MAX_LINES));
        // 0 / 負数（CLI は usize なのでパース時に落ちる）/ 桁違いは弾く
        assert!(validate_lines(0).is_err());
        assert!(validate_lines(MIN_LINES - 1).is_err());
        assert!(validate_lines(1_000_000).is_err());
        let reason = validate_lines(1_000_000).unwrap_err();
        assert!(reason.contains("100000"), "上限を示していない: {reason}");
    }

    #[test]
    fn 永続ファイル由来の値は丸めて起動を成立させる() {
        assert_eq!(clamp_lines(0), MIN_LINES);
        assert_eq!(clamp_lines(1_000_000), MAX_LINES);
        assert_eq!(clamp_lines(2_000), 2_000);
    }

    /// #818 の実測（119 桁 10,000 行 = 理論 28.6 MB）を数式側で固定する
    #[test]
    fn 飽和時の見積りは実測の理論値と一致する() {
        let bytes = estimated_bytes(10_000, 119);
        assert_eq!(bytes, 28_560_000);
        // 上限を 1,000 行へ下げると 1 桁減る
        assert_eq!(estimated_bytes(1_000, 119), 2_856_000);
        assert_eq!(estimated_bytes(0, 119), 0);
    }
}
