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

/// テーマモード（ライト / ダーク。Issue #217）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(ThemeMode::Dark),
            "light" => Some(ThemeMode::Light),
            _ => None,
        }
    }
}

/// テーマ。ターミナル色 + UI クローム色 + フォントを 1 つの構造体で持つ
#[derive(Debug, Clone)]
pub struct Theme {
    /// このテーマのモード（ライト/ダーク切替の状態参照用）
    pub mode: ThemeMode,

    // --- ターミナル色 ---
    pub background: Rgb,
    pub foreground: Rgb,
    pub ansi: [Rgb; 16],
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection_background: Rgb,

    // --- 背景階層（Catppuccin Mocha: 暗い順） ---
    pub crust: Rgb,
    pub mantle: Rgb,
    pub surface_0: Rgb,
    pub surface_1: Rgb,
    pub surface_2: Rgb,
    pub surface_hover: Rgb,
    pub surface_highlight: Rgb,
    /// チップ・カード類の面（カンプ #1a1b27。cwd チップ / standalone カード等）
    pub chip_surface: Rgb,
    /// 退避（shelved）行の面（カンプ #161620）
    pub shelved_surface: Rgb,
    /// ドロップダウン行等の強めの hover 面（カンプ #232434）
    pub surface_hover_strong: Rgb,
    /// 失敗ペインのヘッダ面（カンプ #241b26）
    pub danger_header: Rgb,
    /// 失敗タブカードの面（カンプ #1f1a22）
    pub danger_surface: Rgb,

    // --- ボーダー階層（薄い順） ---
    /// サイドバー等の内側罫線（カンプ #21222f。border_subtle より薄い）
    pub border_inner: Rgb,
    pub border_subtle: Rgb,
    pub border_default: Rgb,
    pub border_strong: Rgb,
    pub border_heavy: Rgb,

    // --- テキスト階層（明るい順） ---
    pub text_secondary: Rgb,
    pub text_tertiary: Rgb,
    /// UI 最頻出の muted テキスト（カンプ #6c7086）
    pub text_muted: Rgb,
    pub text_faint: Rgb,
    pub text_overlay: Rgb,

    // --- アクセント色 ---
    pub accent: Rgb,
    /// 非フォーカス要素のアクセント減光版（カンプ #7f9ccc。ペイン番号バッジ等）
    pub accent_muted: Rgb,
    /// アクセント系ボーダーの減光版（カンプ #5a6a9e。ミニマップ非フォーカス枠等）
    pub accent_border_muted: Rgb,
    /// ミニマップ等の待機要素の枠（カンプ #4a5572）
    pub idle_border: Rgb,
    pub green: Rgb,
    pub red: Rgb,
    pub yellow: Rgb,
    pub teal: Rgb,
    pub mauve: Rgb,
    pub peach: Rgb,

    // --- UI クローム色（後方互換） ---
    pub pane_border: Rgb,
    pub tab_bar_background: Rgb,
    pub tab_active_background: Rgb,
    pub tab_active_foreground: Rgb,
    pub tab_inactive_foreground: Rgb,

    // --- フォント ---
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
}

impl Rgb {
    /// `#RRGGBB` 形式の文字列へ変換する
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// `#RRGGBB` 文字列を Rgb へパースする（6 桁のみ。3 桁 #RGB は非対応）
pub fn parse_hex_color(s: &str) -> Option<Rgb> {
    let s = s.trim();
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Rgb::new(r, g, b))
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_dark()
    }
}

impl Theme {
    /// 全 58 色キー名（settings.json / dispatch 共通。順序は UI カテゴリ順）
    pub const COLOR_KEYS: &[&str] = &[
        // ターミナル (21)
        "background",
        "foreground",
        "cursor",
        "cursor_text",
        "selection_background",
        "ansi_black",
        "ansi_red",
        "ansi_green",
        "ansi_yellow",
        "ansi_blue",
        "ansi_magenta",
        "ansi_cyan",
        "ansi_white",
        "ansi_bright_black",
        "ansi_bright_red",
        "ansi_bright_green",
        "ansi_bright_yellow",
        "ansi_bright_blue",
        "ansi_bright_magenta",
        "ansi_bright_cyan",
        "ansi_bright_white",
        // 背景階層 (12)
        "crust",
        "mantle",
        "surface_0",
        "surface_1",
        "surface_2",
        "surface_hover",
        "surface_highlight",
        "chip_surface",
        "shelved_surface",
        "surface_hover_strong",
        "danger_header",
        "danger_surface",
        // ボーダー (5)
        "border_inner",
        "border_subtle",
        "border_default",
        "border_strong",
        "border_heavy",
        // テキスト (5)
        "text_secondary",
        "text_tertiary",
        "text_muted",
        "text_faint",
        "text_overlay",
        // アクセント (10)
        "accent",
        "accent_muted",
        "accent_border_muted",
        "idle_border",
        "green",
        "red",
        "yellow",
        "teal",
        "mauve",
        "peach",
        // UI クローム (5)
        "pane_border",
        "tab_bar_background",
        "tab_active_background",
        "tab_active_foreground",
        "tab_inactive_foreground",
    ];

    /// 色キーのカテゴリ情報（UI アコーディオン用）
    pub const COLOR_CATEGORIES: &[(&str, &str, usize)] = &[
        ("terminal", "ターミナル", 21),
        ("background", "背景階層", 12),
        ("border", "ボーダー", 5),
        ("text", "テキスト", 5),
        ("accent", "アクセント", 10),
        ("chrome", "UI クローム", 5),
    ];

    /// キー名から色値を取得する
    pub fn color(&self, key: &str) -> Option<Rgb> {
        match key {
            "background" => Some(self.background),
            "foreground" => Some(self.foreground),
            "cursor" => Some(self.cursor),
            "cursor_text" => Some(self.cursor_text),
            "selection_background" => Some(self.selection_background),
            "ansi_black" => Some(self.ansi[0]),
            "ansi_red" => Some(self.ansi[1]),
            "ansi_green" => Some(self.ansi[2]),
            "ansi_yellow" => Some(self.ansi[3]),
            "ansi_blue" => Some(self.ansi[4]),
            "ansi_magenta" => Some(self.ansi[5]),
            "ansi_cyan" => Some(self.ansi[6]),
            "ansi_white" => Some(self.ansi[7]),
            "ansi_bright_black" => Some(self.ansi[8]),
            "ansi_bright_red" => Some(self.ansi[9]),
            "ansi_bright_green" => Some(self.ansi[10]),
            "ansi_bright_yellow" => Some(self.ansi[11]),
            "ansi_bright_blue" => Some(self.ansi[12]),
            "ansi_bright_magenta" => Some(self.ansi[13]),
            "ansi_bright_cyan" => Some(self.ansi[14]),
            "ansi_bright_white" => Some(self.ansi[15]),
            "crust" => Some(self.crust),
            "mantle" => Some(self.mantle),
            "surface_0" => Some(self.surface_0),
            "surface_1" => Some(self.surface_1),
            "surface_2" => Some(self.surface_2),
            "surface_hover" => Some(self.surface_hover),
            "surface_highlight" => Some(self.surface_highlight),
            "chip_surface" => Some(self.chip_surface),
            "shelved_surface" => Some(self.shelved_surface),
            "surface_hover_strong" => Some(self.surface_hover_strong),
            "danger_header" => Some(self.danger_header),
            "danger_surface" => Some(self.danger_surface),
            "border_inner" => Some(self.border_inner),
            "border_subtle" => Some(self.border_subtle),
            "border_default" => Some(self.border_default),
            "border_strong" => Some(self.border_strong),
            "border_heavy" => Some(self.border_heavy),
            "text_secondary" => Some(self.text_secondary),
            "text_tertiary" => Some(self.text_tertiary),
            "text_muted" => Some(self.text_muted),
            "text_faint" => Some(self.text_faint),
            "text_overlay" => Some(self.text_overlay),
            "accent" => Some(self.accent),
            "accent_muted" => Some(self.accent_muted),
            "accent_border_muted" => Some(self.accent_border_muted),
            "idle_border" => Some(self.idle_border),
            "green" => Some(self.green),
            "red" => Some(self.red),
            "yellow" => Some(self.yellow),
            "teal" => Some(self.teal),
            "mauve" => Some(self.mauve),
            "peach" => Some(self.peach),
            "pane_border" => Some(self.pane_border),
            "tab_bar_background" => Some(self.tab_bar_background),
            "tab_active_background" => Some(self.tab_active_background),
            "tab_active_foreground" => Some(self.tab_active_foreground),
            "tab_inactive_foreground" => Some(self.tab_inactive_foreground),
            _ => None,
        }
    }

    /// キー名で色値を設定する。存在しないキーは false を返す
    pub fn set_color(&mut self, key: &str, c: Rgb) -> bool {
        match key {
            "background" => self.background = c,
            "foreground" => self.foreground = c,
            "cursor" => self.cursor = c,
            "cursor_text" => self.cursor_text = c,
            "selection_background" => self.selection_background = c,
            "ansi_black" => self.ansi[0] = c,
            "ansi_red" => self.ansi[1] = c,
            "ansi_green" => self.ansi[2] = c,
            "ansi_yellow" => self.ansi[3] = c,
            "ansi_blue" => self.ansi[4] = c,
            "ansi_magenta" => self.ansi[5] = c,
            "ansi_cyan" => self.ansi[6] = c,
            "ansi_white" => self.ansi[7] = c,
            "ansi_bright_black" => self.ansi[8] = c,
            "ansi_bright_red" => self.ansi[9] = c,
            "ansi_bright_green" => self.ansi[10] = c,
            "ansi_bright_yellow" => self.ansi[11] = c,
            "ansi_bright_blue" => self.ansi[12] = c,
            "ansi_bright_magenta" => self.ansi[13] = c,
            "ansi_bright_cyan" => self.ansi[14] = c,
            "ansi_bright_white" => self.ansi[15] = c,
            "crust" => self.crust = c,
            "mantle" => self.mantle = c,
            "surface_0" => self.surface_0 = c,
            "surface_1" => self.surface_1 = c,
            "surface_2" => self.surface_2 = c,
            "surface_hover" => self.surface_hover = c,
            "surface_highlight" => self.surface_highlight = c,
            "chip_surface" => self.chip_surface = c,
            "shelved_surface" => self.shelved_surface = c,
            "surface_hover_strong" => self.surface_hover_strong = c,
            "danger_header" => self.danger_header = c,
            "danger_surface" => self.danger_surface = c,
            "border_inner" => self.border_inner = c,
            "border_subtle" => self.border_subtle = c,
            "border_default" => self.border_default = c,
            "border_strong" => self.border_strong = c,
            "border_heavy" => self.border_heavy = c,
            "text_secondary" => self.text_secondary = c,
            "text_tertiary" => self.text_tertiary = c,
            "text_muted" => self.text_muted = c,
            "text_faint" => self.text_faint = c,
            "text_overlay" => self.text_overlay = c,
            "accent" => self.accent = c,
            "accent_muted" => self.accent_muted = c,
            "accent_border_muted" => self.accent_border_muted = c,
            "idle_border" => self.idle_border = c,
            "green" => self.green = c,
            "red" => self.red = c,
            "yellow" => self.yellow = c,
            "teal" => self.teal = c,
            "mauve" => self.mauve = c,
            "peach" => self.peach = c,
            "pane_border" => self.pane_border = c,
            "tab_bar_background" => self.tab_bar_background = c,
            "tab_active_background" => self.tab_active_background = c,
            "tab_active_foreground" => self.tab_active_foreground = c,
            "tab_inactive_foreground" => self.tab_inactive_foreground = c,
            _ => return false,
        }
        true
    }

    /// 上書きマップを適用する。未知キー・不正 hex は無視して警告文字列で返す
    pub fn apply_overrides(
        &mut self,
        overrides: &std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for (key, hex) in overrides {
            match parse_hex_color(hex) {
                Some(c) => {
                    if !self.set_color(key, c) {
                        warnings.push(format!("未知の色キー: {key}"));
                    }
                }
                None => {
                    warnings.push(format!("不正な色値: {key}={hex}（#RRGGBB 形式が必要）"));
                }
            }
        }
        warnings
    }

    /// モードに対応するテーマを返す（Issue #217 ライト/ダーク切替）
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::default_dark(),
            ThemeMode::Light => Self::default_light(),
        }
    }

    /// デフォルトダークテーマ（Catppuccin Mocha ベース）
    pub fn default_dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            background: Rgb::from_hex(0x1e1e2e), // Base
            foreground: Rgb::from_hex(0xcdd6f4), // Text primary
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
            cursor: Rgb::from_hex(0xf5e0dc), // Rosewater
            cursor_text: Rgb::from_hex(0x1e1e2e),
            selection_background: Rgb::from_hex(0x45475a),

            // 背景階層
            crust: Rgb::from_hex(0x11111b),
            mantle: Rgb::from_hex(0x181825),
            surface_0: Rgb::from_hex(0x1b1c28),
            surface_1: Rgb::from_hex(0x1c1d2b),
            surface_2: Rgb::from_hex(0x20212f),
            surface_hover: Rgb::from_hex(0x1f2030),
            surface_highlight: Rgb::from_hex(0x313244),
            chip_surface: Rgb::from_hex(0x1a1b27),
            shelved_surface: Rgb::from_hex(0x161620),
            surface_hover_strong: Rgb::from_hex(0x232434),
            danger_header: Rgb::from_hex(0x241b26),
            danger_surface: Rgb::from_hex(0x1f1a22),

            // ボーダー階層
            border_inner: Rgb::from_hex(0x21222f),
            border_subtle: Rgb::from_hex(0x26273a),
            border_default: Rgb::from_hex(0x2a2b3c),
            border_strong: Rgb::from_hex(0x2b2c3e),
            border_heavy: Rgb::from_hex(0x34354a),

            // テキスト階層
            text_secondary: Rgb::from_hex(0xbac2de),
            text_tertiary: Rgb::from_hex(0xa6adc8),
            text_muted: Rgb::from_hex(0x6c7086),
            text_faint: Rgb::from_hex(0x585b70),
            text_overlay: Rgb::from_hex(0x45475a),

            // アクセント色
            accent: Rgb::from_hex(0x89b4fa), // Blue
            accent_muted: Rgb::from_hex(0x7f9ccc),
            accent_border_muted: Rgb::from_hex(0x5a6a9e),
            idle_border: Rgb::from_hex(0x4a5572),
            green: Rgb::from_hex(0xa6e3a1),
            red: Rgb::from_hex(0xf38ba8),
            yellow: Rgb::from_hex(0xf9e2af),
            teal: Rgb::from_hex(0x94e2d5),
            mauve: Rgb::from_hex(0xcba6f7),
            peach: Rgb::from_hex(0xfab387),

            // UI クローム
            pane_border: Rgb::from_hex(0x2a2b3c), // border_default に合わせた
            tab_bar_background: Rgb::from_hex(0x181825), // Mantle（旧 Crust→スペック準拠）
            tab_active_background: Rgb::from_hex(0x26273a), // カンプのピル型 active タブ面（#217）
            tab_active_foreground: Rgb::from_hex(0xcdd6f4),
            tab_inactive_foreground: Rgb::from_hex(0x6c7086),

            font_family: "Menlo".into(),
            font_size: 13.0,
            line_height: 17.0,
        }
    }

    /// デフォルトライトテーマ（Catppuccin Latte ベース。Issue #217）。
    /// カンプ（ダーク = Mocha 実値）に対応する同一デザインシステムのライト版として、
    /// Latte の対応階層をダーク側と同じ相対関係で割り当てる
    pub fn default_light() -> Self {
        Self {
            mode: ThemeMode::Light,
            background: Rgb::from_hex(0xeff1f5), // Base
            foreground: Rgb::from_hex(0x4c4f69), // Text
            ansi: [
                Rgb::from_hex(0xbcc0cc), // black (Surface1)
                Rgb::from_hex(0xd20f39), // red
                Rgb::from_hex(0x40a02b), // green
                Rgb::from_hex(0xdf8e1d), // yellow
                Rgb::from_hex(0x1e66f5), // blue
                Rgb::from_hex(0xea76cb), // magenta (Pink)
                Rgb::from_hex(0x179299), // cyan (Teal)
                Rgb::from_hex(0x5c5f77), // white (Subtext1)
                Rgb::from_hex(0x9ca0b0), // bright black (Overlay0)
                Rgb::from_hex(0xd20f39), // bright red
                Rgb::from_hex(0x40a02b), // bright green
                Rgb::from_hex(0xdf8e1d), // bright yellow
                Rgb::from_hex(0x1e66f5), // bright blue
                Rgb::from_hex(0xea76cb), // bright magenta
                Rgb::from_hex(0x179299), // bright cyan
                Rgb::from_hex(0x6c6f85), // bright white (Subtext0)
            ],
            cursor: Rgb::from_hex(0xdc8a78), // Rosewater
            cursor_text: Rgb::from_hex(0xeff1f5),
            selection_background: Rgb::from_hex(0xccd0da),

            // 背景階層（ダークの「暗い順」をライトでは「明るい外殻 → 沈む面」で対応）
            crust: Rgb::from_hex(0xdce0e8),
            mantle: Rgb::from_hex(0xe6e9ef),
            surface_0: Rgb::from_hex(0xe2e5ec),
            surface_1: Rgb::from_hex(0xe4e7ee),
            surface_2: Rgb::from_hex(0xdde1e9),
            surface_hover: Rgb::from_hex(0xe0e3eb),
            surface_highlight: Rgb::from_hex(0xccd0da),
            chip_surface: Rgb::from_hex(0xe8ebf1),
            shelved_surface: Rgb::from_hex(0xdcdfe7),
            surface_hover_strong: Rgb::from_hex(0xd9dde6),
            danger_header: Rgb::from_hex(0xf2dee2),
            danger_surface: Rgb::from_hex(0xf4e4e8),

            // ボーダー階層
            border_inner: Rgb::from_hex(0xdfe3ea),
            border_subtle: Rgb::from_hex(0xd5d9e2),
            border_default: Rgb::from_hex(0xcfd4de),
            border_strong: Rgb::from_hex(0xcbd0db),
            border_heavy: Rgb::from_hex(0xbfc4d1),

            // テキスト階層
            text_secondary: Rgb::from_hex(0x5c5f77), // Subtext1
            text_tertiary: Rgb::from_hex(0x6c6f85),  // Subtext0
            text_muted: Rgb::from_hex(0x8c8fa1),     // Overlay1
            text_faint: Rgb::from_hex(0x9ca0b0),     // Overlay0
            text_overlay: Rgb::from_hex(0xacb0be),   // Surface2

            // アクセント色
            accent: Rgb::from_hex(0x1e66f5), // Blue
            accent_muted: Rgb::from_hex(0x5a86e0),
            accent_border_muted: Rgb::from_hex(0x8fa6d9),
            idle_border: Rgb::from_hex(0xaab3c7),
            green: Rgb::from_hex(0x40a02b),
            red: Rgb::from_hex(0xd20f39),
            yellow: Rgb::from_hex(0xdf8e1d),
            teal: Rgb::from_hex(0x179299),
            mauve: Rgb::from_hex(0x8839ef),
            peach: Rgb::from_hex(0xfe640b),

            // UI クローム
            pane_border: Rgb::from_hex(0xcfd4de),
            tab_bar_background: Rgb::from_hex(0xe6e9ef), // Mantle
            tab_active_background: Rgb::from_hex(0xd5d9e2), // ピル型 active タブ面（ダークと同相対）
            tab_active_foreground: Rgb::from_hex(0x4c4f69),
            tab_inactive_foreground: Rgb::from_hex(0x8c8fa1),

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
    fn theme_modeのparseとas_strが往復する() {
        assert_eq!(ThemeMode::parse("dark"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::parse("LIGHT"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::parse(" light "), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::parse("solarized"), None);
        assert_eq!(
            ThemeMode::parse(ThemeMode::Dark.as_str()),
            Some(ThemeMode::Dark)
        );
        assert_eq!(
            ThemeMode::parse(ThemeMode::Light.as_str()),
            Some(ThemeMode::Light)
        );
    }

    #[test]
    fn for_modeが対応テーマを返す() {
        assert_eq!(Theme::for_mode(ThemeMode::Dark).mode, ThemeMode::Dark);
        assert_eq!(Theme::for_mode(ThemeMode::Light).mode, ThemeMode::Light);
        // ダークはカンプ実値（Mocha Base）、ライトは Latte Base
        assert_eq!(
            Theme::for_mode(ThemeMode::Dark).background,
            Rgb::from_hex(0x1e1e2e)
        );
        assert_eq!(
            Theme::for_mode(ThemeMode::Light).background,
            Rgb::from_hex(0xeff1f5)
        );
    }

    #[test]
    fn parse_hex_colorの基本() {
        assert_eq!(parse_hex_color("#ff0000"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(parse_hex_color("#00ff00"), Some(Rgb::new(0, 255, 0)));
        assert_eq!(parse_hex_color("89b4fa"), Some(Rgb::new(0x89, 0xb4, 0xfa)));
        assert_eq!(
            parse_hex_color(" #1e1e2e "),
            Some(Rgb::new(0x1e, 0x1e, 0x2e))
        );
        assert_eq!(parse_hex_color("#fff"), None); // 3桁は非対応
        assert_eq!(parse_hex_color("#gggggg"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn rgb_to_hexの往復() {
        let c = Rgb::new(0x89, 0xb4, 0xfa);
        assert_eq!(c.to_hex(), "#89b4fa");
        assert_eq!(parse_hex_color(&c.to_hex()), Some(c));
    }

    #[test]
    fn color_keysと_color_set_colorが完全一致する() {
        let mut t = Theme::default_dark();
        for key in Theme::COLOR_KEYS {
            assert!(t.color(key).is_some(), "color({key}) が None");
            assert!(
                t.set_color(key, Rgb::new(1, 2, 3)),
                "set_color({key}) が false"
            );
            assert_eq!(
                t.color(key),
                Some(Rgb::new(1, 2, 3)),
                "{key} の set/get 不一致"
            );
        }
        assert_eq!(Theme::COLOR_KEYS.len(), 58, "色キーは 58 個");
    }

    #[test]
    fn apply_overridesは正常値を適用し不正値を警告する() {
        let mut t = Theme::default_dark();
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("accent".into(), "#ff0000".into());
        overrides.insert("unknown_key".into(), "#000000".into());
        overrides.insert("background".into(), "not-hex".into());
        let warnings = t.apply_overrides(&overrides);
        assert_eq!(t.accent, Rgb::new(255, 0, 0));
        assert_ne!(t.background, Rgb::from_hex(0x000000)); // 不正 hex は適用されない
        assert_eq!(warnings.len(), 2);
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
