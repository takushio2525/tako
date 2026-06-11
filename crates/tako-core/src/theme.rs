//! Theme — 描画色・フォントの一元管理（GPUI 非依存、FR-4）
//!
//! 全描画色は必ずこの構造体を通して引く（コードへの直書き禁止。`requirements.md` FR-4）。
//! Phase 1 はデフォルトダークテーマ 1 つだが、後の AI テーマ操作（FR-4.5 / 設計原則 5）で
//! 全項目を MCP / CLI から読み書きできるよう、最初からフィールド単位の構造化を守る。

/// テーマ用の RGB 色（sRGB、各 0–255）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `0xRRGGBB` 形式から生成する
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
        }
    }

    /// 0.0–1.0 の係数で暗くする（DIM 表現用）
    pub fn dim(self, factor: f32) -> Self {
        Self {
            r: (self.r as f32 * factor) as u8,
            g: (self.g as f32 * factor) as u8,
            b: (self.b as f32 * factor) as u8,
        }
    }
}

/// テーマ。ターミナル色 + UI クローム色 + フォントを 1 つの構造体で持つ
#[derive(Debug, Clone)]
pub struct Theme {
    // --- ターミナル色 ---
    /// デフォルト背景
    pub background: Rgb,
    /// デフォルト前景
    pub foreground: Rgb,
    /// ANSI 16 色（0–7: 通常、8–15: 明色）
    pub ansi: [Rgb; 16],
    /// カーソル（ブロック）の塗り色
    pub cursor: Rgb,
    /// カーソル上の文字色
    pub cursor_text: Rgb,
    /// 選択範囲の背景
    pub selection_background: Rgb,

    // --- UI クローム色 ---
    /// アクセントカラー（フォーカスペインの枠・アクティブタブの下線）
    pub accent: Rgb,
    /// 非フォーカスペインの枠
    pub pane_border: Rgb,
    /// タブバーの背景
    pub tab_bar_background: Rgb,
    /// アクティブタブの背景
    pub tab_active_background: Rgb,
    /// アクティブタブの文字色
    pub tab_active_foreground: Rgb,
    /// 非アクティブタブの文字色
    pub tab_inactive_foreground: Rgb,

    // --- フォント ---
    pub font_family: String,
    pub font_size: f32,
    /// 行の高さ（px）。セル高さとしても使う
    pub line_height: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_dark()
    }
}

impl Theme {
    /// デフォルトダークテーマ（Catppuccin Mocha ベース）
    pub fn default_dark() -> Self {
        Self {
            background: Rgb::from_hex(0x1e1e2e),
            foreground: Rgb::from_hex(0xcdd6f4),
            ansi: [
                Rgb::from_hex(0x45475a), // black
                Rgb::from_hex(0xf38ba8), // red
                Rgb::from_hex(0xa6e3a1), // green
                Rgb::from_hex(0xf9e2af), // yellow
                Rgb::from_hex(0x89b4fa), // blue
                Rgb::from_hex(0xf5c2e7), // magenta
                Rgb::from_hex(0x94e2d5), // cyan
                Rgb::from_hex(0xbac2de), // white
                Rgb::from_hex(0x585b70), // bright black
                Rgb::from_hex(0xf38ba8), // bright red
                Rgb::from_hex(0xa6e3a1), // bright green
                Rgb::from_hex(0xf9e2af), // bright yellow
                Rgb::from_hex(0x89b4fa), // bright blue
                Rgb::from_hex(0xf5c2e7), // bright magenta
                Rgb::from_hex(0x94e2d5), // bright cyan
                Rgb::from_hex(0xa6adc8), // bright white
            ],
            cursor: Rgb::from_hex(0xf5e0dc),
            cursor_text: Rgb::from_hex(0x1e1e2e),
            selection_background: Rgb::from_hex(0x45475a),
            accent: Rgb::from_hex(0x89b4fa),
            pane_border: Rgb::from_hex(0x313244),
            tab_bar_background: Rgb::from_hex(0x11111b),
            tab_active_background: Rgb::from_hex(0x1e1e2e),
            tab_active_foreground: Rgb::from_hex(0xcdd6f4),
            tab_inactive_foreground: Rgb::from_hex(0x6c7086),
            font_family: "Menlo".into(),
            font_size: 13.0,
            line_height: 17.0,
        }
    }

    /// 256 色パレットのインデックスを解決する。
    /// 0–15 はテーマの ANSI 色、16–231 は 6x6x6 カラーキューブ、232–255 はグレースケール
    pub fn indexed_color(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let i = index as u32 - 16;
                let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
                // xterm 標準: 0 → 0, n → 55 + 40n
                let level = |v: u32| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
                Rgb::new(level(r), level(g), level(b))
            }
            232..=255 => {
                let v = (8 + 10 * (index as u32 - 232)) as u8;
                Rgb::new(v, v, v)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hexで各成分が取れる() {
        let c = Rgb::from_hex(0x1e2e3e);
        assert_eq!((c.r, c.g, c.b), (0x1e, 0x2e, 0x3e));
    }

    #[test]
    fn indexed_colorの境界値() {
        let t = Theme::default_dark();
        // 0–15 はテーマ ANSI 色
        assert_eq!(t.indexed_color(1), t.ansi[1]);
        // 16 はキューブの原点（黒）
        assert_eq!(t.indexed_color(16), Rgb::new(0, 0, 0));
        // 231 はキューブの最大（白）
        assert_eq!(t.indexed_color(231), Rgb::new(255, 255, 255));
        // 196 = 16 + 36*5 → 純赤
        assert_eq!(t.indexed_color(196), Rgb::new(255, 0, 0));
        // グレースケールの両端
        assert_eq!(t.indexed_color(232), Rgb::new(8, 8, 8));
        assert_eq!(t.indexed_color(255), Rgb::new(238, 238, 238));
    }
}
