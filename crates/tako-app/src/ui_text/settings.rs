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

// --- Code Runner タブ ---

pub fn runner_header() -> &'static str {
    tr!("拡張子既定コマンド", "Extension default commands")
}

pub fn runner_col_ext() -> &'static str {
    tr!("拡張子", "Extension")
}

pub fn runner_col_command() -> &'static str {
    tr!("コマンド", "Command")
}

pub fn runner_col_source() -> &'static str {
    tr!("ソース", "Source")
}

pub fn runner_source_builtin() -> &'static str {
    tr!("組込", "builtin")
}

pub fn runner_source_user() -> &'static str {
    tr!("ユーザー", "user")
}

pub fn runner_add_header() -> &'static str {
    tr!("新規追加", "Add new")
}

pub fn runner_add_btn() -> &'static str {
    tr!("追加", "Add")
}

pub fn runner_help_header() -> &'static str {
    tr!("変数リファレンス", "Variable reference")
}

pub fn runner_resolution_help() -> &'static str {
    tr!(
        "解決順: ファイル内 tako:run 宣言 > ユーザー既定 > 組込既定",
        "Resolution order: in-file tako:run declaration > user default > built-in default"
    )
}

pub fn runner_var_file() -> &'static str {
    tr!("ファイルの絶対パス", "Absolute path")
}

pub fn runner_var_filedir() -> &'static str {
    tr!("ファイルのディレクトリ", "File directory")
}

pub fn runner_var_filebase() -> &'static str {
    tr!("ファイル名", "File name")
}

pub fn runner_var_filenoext() -> &'static str {
    tr!("拡張子なしファイル名", "File name without extension")
}

pub fn runner_var_ext() -> &'static str {
    tr!("拡張子", "Extension")
}

// --- セットアップタブ ---

pub fn setup_agents_header() -> &'static str {
    tr!("エージェント CLI", "Agent CLIs")
}

pub fn setup_installed() -> &'static str {
    tr!("導入済み", "Installed")
}

pub fn setup_not_installed() -> &'static str {
    tr!("未導入", "Not installed")
}

pub fn setup_profiles_header() -> &'static str {
    tr!("プロファイル", "Profiles")
}

pub fn setup_mcp_header() -> &'static str {
    tr!("MCP 登録", "MCP registration")
}

pub fn setup_mcp_register() -> &'static str {
    tr!("登録する", "Register")
}

pub fn setup_fda_header() -> &'static str {
    tr!("フルディスクアクセス", "Full Disk Access")
}

pub fn setup_fda_open() -> &'static str {
    tr!("システム設定を開く", "Open System Settings")
}

pub fn setup_rules_header() -> &'static str {
    tr!("共通ルール同期", "Rules sync")
}

pub fn setup_rules_sync() -> &'static str {
    tr!("同期する", "Sync now")
}

pub fn setup_run_btn() -> &'static str {
    tr!("tako setup を実行", "Run tako setup")
}

// --- スリープ防止タブ ---

pub fn sleep_mode_header() -> &'static str {
    tr!("スリープ防止モード", "Sleep prevention mode")
}

pub fn sleep_mode_off() -> &'static str {
    tr!("オフ", "Off")
}

pub fn sleep_mode_on() -> &'static str {
    tr!("常時オン", "Always on")
}

pub fn sleep_mode_agents() -> &'static str {
    tr!("エージェント稼働中", "While agents running")
}

pub fn sleep_power_header() -> &'static str {
    tr!("電源条件", "Power condition")
}

pub fn sleep_power_ac() -> &'static str {
    tr!("AC 電源のみ", "AC power only")
}

pub fn sleep_power_always() -> &'static str {
    tr!("常時", "Always")
}

pub fn sleep_lid_header() -> &'static str {
    tr!("蓋閉じ継続", "Lid close prevention")
}

// --- リモートタブ ---

pub fn remote_daemon_header() -> &'static str {
    tr!("リモートデーモン", "Remote daemon")
}

pub fn remote_status_running() -> &'static str {
    tr!("稼働中", "Running")
}

pub fn remote_status_stopped() -> &'static str {
    tr!("停止中", "Stopped")
}

pub fn remote_start() -> &'static str {
    tr!("開始", "Start")
}

pub fn remote_stop() -> &'static str {
    tr!("停止", "Stop")
}

pub fn remote_setup_header() -> &'static str {
    tr!("セットアップ状態", "Setup status")
}

// --- 高度タブ ---

pub fn advanced_editor_header() -> &'static str {
    tr!("settings.json 直接編集", "Edit settings.json directly")
}

pub fn advanced_save() -> &'static str {
    tr!("保存", "Save")
}

pub fn advanced_reload() -> &'static str {
    tr!("再読み込み", "Reload")
}

pub fn advanced_open_finder() -> &'static str {
    tr!("Finder で表示", "Reveal in Finder")
}

pub fn advanced_related_header() -> &'static str {
    tr!("関連ファイル", "Related files")
}

pub fn advanced_parse_error() -> &'static str {
    tr!("JSON パースエラー", "JSON parse error")
}

pub fn advanced_saved() -> &'static str {
    tr!("保存しました", "Saved")
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
                runner_header().into(),
                runner_col_ext().into(),
                runner_col_command().into(),
                runner_col_source().into(),
                runner_source_builtin().into(),
                runner_source_user().into(),
                runner_add_header().into(),
                runner_add_btn().into(),
                runner_help_header().into(),
                runner_resolution_help().into(),
                runner_var_file().into(),
                runner_var_filedir().into(),
                runner_var_filebase().into(),
                runner_var_filenoext().into(),
                runner_var_ext().into(),
                setup_agents_header().into(),
                setup_installed().into(),
                setup_not_installed().into(),
                setup_profiles_header().into(),
                setup_mcp_header().into(),
                setup_mcp_register().into(),
                setup_fda_header().into(),
                setup_fda_open().into(),
                setup_rules_header().into(),
                setup_rules_sync().into(),
                setup_run_btn().into(),
                sleep_mode_header().into(),
                sleep_mode_off().into(),
                sleep_mode_on().into(),
                sleep_mode_agents().into(),
                sleep_power_header().into(),
                sleep_power_ac().into(),
                sleep_power_always().into(),
                sleep_lid_header().into(),
                remote_daemon_header().into(),
                remote_status_running().into(),
                remote_status_stopped().into(),
                remote_start().into(),
                remote_stop().into(),
                remote_setup_header().into(),
                advanced_editor_header().into(),
                advanced_save().into(),
                advanced_reload().into(),
                advanced_open_finder().into(),
                advanced_related_header().into(),
                advanced_parse_error().into(),
                advanced_saved().into(),
            ]
        });
    }
}
