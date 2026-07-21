//! 設定画面の UI 文字列カタログ（Issue #459）
#![allow(dead_code)]

pub fn window_title() -> &'static str {
    tr!("tako 設定", "tako Settings")
}

pub fn tab_general() -> &'static str {
    tr!("一般", "General")
}

pub fn tab_appearance() -> &'static str {
    tr!("外観", "Appearance")
}

pub fn tab_runner() -> &'static str {
    tr!("Code Runner", "Code Runner")
}

pub fn tab_setup() -> &'static str {
    tr!("セットアップ", "Setup")
}

pub fn tab_sleep() -> &'static str {
    tr!("スリープ防止", "Sleep Guard")
}

pub fn tab_remote() -> &'static str {
    tr!("リモート", "Remote")
}

pub fn tab_advanced() -> &'static str {
    tr!("高度", "Advanced")
}

// --- 一般タブ ---

pub fn label_language() -> &'static str {
    tr!("表示言語", "Display language")
}

pub fn label_auto_rename() -> &'static str {
    tr!("AI 自動リネーム", "AI auto-rename")
}

pub fn label_port_detect() -> &'static str {
    tr!("ポート検知", "Port detection")
}

pub fn label_persist() -> &'static str {
    tr!("セッション永続化", "Session persistence")
}

pub fn label_confirm_close() -> &'static str {
    tr!("Close 確認", "Close confirmation")
}

pub fn label_telemetry() -> &'static str {
    tr!("エラーレポート", "Error reports")
}

pub fn label_limit_service() -> &'static str {
    tr!("利用制限表示", "Usage limit display")
}

pub fn label_preview_reload() -> &'static str {
    tr!("ライブリロード", "Live reload")
}

pub fn label_preview_cache() -> &'static str {
    tr!("画像キャッシュ上限 (MiB)", "Image cache limit (MiB)")
}

pub fn label_pane_logs() -> &'static str {
    tr!("ペインログ", "Pane logs")
}

// --- 外観タブ ---

pub fn label_theme() -> &'static str {
    tr!("テーマ", "Theme")
}

pub fn label_color_settings() -> &'static str {
    tr!("色設定", "Color settings")
}

pub fn label_preset() -> &'static str {
    tr!("プリセット", "Preset")
}

pub fn label_font() -> &'static str {
    tr!("フォント", "Font")
}

pub fn button_save_preset() -> &'static str {
    tr!("保存", "Save")
}

pub fn button_delete() -> &'static str {
    tr!("削除", "Delete")
}

pub fn button_reset() -> &'static str {
    tr!("リセット", "Reset")
}

pub fn button_reset_all() -> &'static str {
    tr!("全色リセット", "Reset all colors")
}

pub fn placeholder_coming_soon() -> &'static str {
    tr!("準備中", "Coming soon")
}

pub fn category_terminal() -> &'static str {
    tr!("ターミナル", "Terminal")
}

pub fn category_background() -> &'static str {
    tr!("背景階層", "Background layers")
}

pub fn category_border() -> &'static str {
    tr!("ボーダー", "Borders")
}

pub fn category_text() -> &'static str {
    tr!("テキスト", "Text")
}

pub fn category_accent() -> &'static str {
    tr!("アクセント", "Accent")
}

pub fn category_chrome() -> &'static str {
    tr!("UI クローム", "UI Chrome")
}

pub fn label_font_family() -> &'static str {
    tr!("ファミリー", "Family")
}

pub fn label_font_size() -> &'static str {
    tr!("サイズ", "Size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_texts_have_both_languages_and_no_emoji() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                window_title().into(),
                tab_general().into(),
                tab_appearance().into(),
                tab_runner().into(),
                tab_setup().into(),
                tab_sleep().into(),
                tab_remote().into(),
                tab_advanced().into(),
                label_language().into(),
                label_auto_rename().into(),
                label_port_detect().into(),
                label_persist().into(),
                label_confirm_close().into(),
                label_telemetry().into(),
                label_limit_service().into(),
                label_preview_reload().into(),
                label_preview_cache().into(),
                label_pane_logs().into(),
                label_theme().into(),
                label_color_settings().into(),
                label_preset().into(),
                label_font().into(),
                button_save_preset().into(),
                button_delete().into(),
                button_reset().into(),
                button_reset_all().into(),
                placeholder_coming_soon().into(),
                category_terminal().into(),
                category_background().into(),
                category_border().into(),
                category_text().into(),
                category_accent().into(),
                category_chrome().into(),
                label_font_family().into(),
                label_font_size().into(),
            ]
        });
    }
}
