//! 設定画面の UI 文字列カタログ（Issue #459 / #486 / #488）
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

// --- 共通ボタン・メッセージ ---

pub fn button_reset() -> &'static str {
    tr!("リセット", "Reset")
}

pub fn button_reset_all() -> &'static str {
    tr!("全色リセット", "Reset all colors")
}

pub fn button_save_preset() -> &'static str {
    tr!("保存", "Save")
}

pub fn button_delete() -> &'static str {
    tr!("削除", "Delete")
}

pub fn button_apply() -> &'static str {
    tr!("適用", "Apply")
}

pub fn button_refresh() -> &'static str {
    tr!("更新", "Refresh")
}

pub fn button_copy() -> &'static str {
    tr!("コピー", "Copy")
}

pub fn button_check() -> &'static str {
    tr!("確認", "Check")
}

pub fn button_show() -> &'static str {
    tr!("表示", "Show")
}

pub fn error_app_gone() -> &'static str {
    tr!("本体ウィンドウが見つかりません", "Main window is gone")
}

pub fn error_number() -> &'static str {
    tr!("数値で入力してください", "Enter a number")
}

pub fn msg_loading() -> &'static str {
    tr!("読み込み中…", "Loading…")
}

pub fn msg_refreshed() -> &'static str {
    tr!("最新の状態にしました", "Refreshed")
}

pub fn msg_copied() -> &'static str {
    tr!("コピーしました", "Copied")
}

pub fn msg_reloaded() -> &'static str {
    tr!("設定を読み直しました", "Settings reloaded")
}

pub fn msg_nothing_to_save() -> &'static str {
    tr!(
        "編集していません（エディタをクリックしてから編集してください）",
        "Nothing edited (click the editor first)"
    )
}

pub fn msg_preset_saved() -> &'static str {
    tr!("プリセットを保存しました", "Preset saved")
}

pub fn msg_preset_name_required() -> &'static str {
    tr!(
        "プリセット名を入力してください",
        "Enter a preset name first"
    )
}

pub fn msg_no_presets() -> &'static str {
    tr!("保存済みプリセットはありません", "No saved presets")
}

// --- 一般タブ ---

pub fn label_language() -> &'static str {
    tr!("表示言語", "Display language")
}

pub fn desc_language() -> &'static str {
    tr!(
        "UI の表示言語。CLI の tako lang / MCP の tako_lang と同じ設定",
        "UI language. Same setting as tako lang / tako_lang"
    )
}

pub fn lang_system() -> &'static str {
    tr!("OS 既定", "System")
}

pub fn lang_ja() -> &'static str {
    tr!("日本語", "Japanese")
}

pub fn lang_en() -> &'static str {
    tr!("English", "English")
}

pub fn label_auto_rename() -> &'static str {
    tr!("AI 自動リネーム", "AI auto-rename")
}

pub fn desc_auto_rename() -> &'static str {
    tr!(
        "タブ・ペイン名を実行内容から自動で付ける",
        "Name tabs and panes automatically from their contents"
    )
}

pub fn label_port_detect() -> &'static str {
    tr!("ポート検知", "Port detection")
}

pub fn desc_port_detect() -> &'static str {
    tr!(
        "listen ポートを検知してプレビュー提案チップを出す",
        "Detect listening ports and offer a preview chip"
    )
}

pub fn label_persist() -> &'static str {
    tr!("セッション永続化", "Session persistence")
}

pub fn desc_persist() -> &'static str {
    tr!(
        "tmux バックエンドで再起動後もペインを復元する",
        "Restore panes after restart via the tmux backend"
    )
}

pub fn desc_persist_no_tmux() -> &'static str {
    tr!(
        "tmux が見つからないため構成のみ復元されます",
        "tmux not found: only the layout is restored"
    )
}

pub fn desc_persist_secondary() -> &'static str {
    tr!(
        "セカンダリモードのため復元・保存は無効です",
        "Secondary instance: restore and save are disabled"
    )
}

pub fn label_confirm_close() -> &'static str {
    tr!("Close 確認", "Close confirmation")
}

pub fn desc_confirm_close() -> &'static str {
    tr!(
        "ペインを閉じるときに確認ダイアログを出す",
        "Ask before closing a pane"
    )
}

pub fn label_telemetry() -> &'static str {
    tr!("エラーレポート", "Error reports")
}

pub fn desc_telemetry() -> &'static str {
    tr!(
        "クラッシュ情報の自動送信（既定 OFF の opt-in）",
        "Send crash reports automatically (opt-in, off by default)"
    )
}

pub fn label_limit_service() -> &'static str {
    tr!("利用制限表示", "Usage limit display")
}

pub fn desc_limit_service() -> &'static str {
    tr!(
        "ステータスバーに利用制限を表示するサービス",
        "Which service's usage limits show in the status bar"
    )
}

pub fn section_preview() -> &'static str {
    tr!("プレビュー", "Preview")
}

pub fn label_preview_reload() -> &'static str {
    tr!("ライブリロード", "Live reload")
}

pub fn desc_preview_reload() -> &'static str {
    tr!(
        "表示中のファイルが変わったら自動で再読み込みする",
        "Reload the previewed file when it changes on disk"
    )
}

pub fn label_preview_cache() -> &'static str {
    tr!("画像キャッシュ上限 (MiB)", "Image cache limit (MiB)")
}

pub fn desc_preview_cache() -> &'static str {
    tr!(
        "PDF・画像のデコード済みキャッシュ上限（256〜8192）",
        "Decoded PDF/image cache limit (256-8192)"
    )
}

pub fn section_logs() -> &'static str {
    tr!("ペインログ", "Pane logs")
}

pub fn label_pane_logs() -> &'static str {
    tr!("平文ログの保存", "Save plain-text logs")
}

pub fn desc_pane_logs() -> &'static str {
    tr!(
        "ペインの出力をローカルに保存する（ペイン終了後も遡れる）",
        "Store pane output locally so it survives the pane"
    )
}

pub fn label_pane_log_max() -> &'static str {
    tr!("ペインごとの上限 (MB)", "Per-pane limit (MB)")
}

pub fn desc_pane_log_max() -> &'static str {
    tr!(
        "超えたらローテーションする",
        "Rotate the log when it exceeds this size"
    )
}

pub fn label_pane_log_total() -> &'static str {
    tr!("全体の上限 (MB)", "Total limit (MB)")
}

pub fn desc_pane_log_total() -> &'static str {
    tr!(
        "超えたら古いログから削除する",
        "Delete the oldest logs when the total exceeds this size"
    )
}

// --- 外観タブ ---

pub fn label_theme() -> &'static str {
    tr!("テーマ", "Theme")
}

pub fn desc_theme() -> &'static str {
    tr!(
        "ダーク / ライト。プリセットは下の一覧から適用できる",
        "Dark or light. Presets can be applied from the list below"
    )
}

pub fn theme_dark() -> &'static str {
    tr!("ダーク", "Dark")
}

pub fn theme_light() -> &'static str {
    tr!("ライト", "Light")
}

pub fn label_color_settings() -> &'static str {
    tr!("色設定", "Colors")
}

pub fn label_preset() -> &'static str {
    tr!("プリセット", "Presets")
}

pub fn placeholder_preset_name() -> &'static str {
    tr!("プリセット名", "Preset name")
}

pub fn label_font_family() -> &'static str {
    tr!("フォント", "Font family")
}

pub fn desc_font_family() -> &'static str {
    tr!(
        "空欄で既定（Menlo）に戻る",
        "Leave empty to use the default (Menlo)"
    )
}

pub fn label_font_size() -> &'static str {
    tr!("フォントサイズ", "Font size")
}

pub fn desc_font_size() -> &'static str {
    tr!("8〜32 pt", "8-32 pt")
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

// --- Code Runner タブ ---

pub fn runner_header() -> &'static str {
    tr!("拡張子既定コマンド", "Extension default commands")
}

pub fn runner_edit_help() -> &'static str {
    tr!(
        "コマンド欄をクリックすると編集できる（Enter で確定 / Esc で取消）",
        "Click a command to edit it (Enter to apply, Esc to cancel)"
    )
}

pub fn runner_col_ext() -> &'static str {
    tr!("拡張子", "Extension")
}

pub fn runner_col_command() -> &'static str {
    tr!("コマンド", "Command")
}

pub fn runner_placeholder_command() -> &'static str {
    tr!("例: python3 ${fileBase}", "e.g. python3 ${fileBase}")
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

pub fn setup_mcp_header() -> &'static str {
    tr!("MCP 登録", "MCP registration")
}

pub fn desc_mcp() -> &'static str {
    tr!(
        "Claude Code から tako を操作できるようにする",
        "Let Claude Code drive tako"
    )
}

pub fn setup_mcp_register() -> &'static str {
    tr!("登録する", "Register")
}

pub fn msg_mcp_registered() -> &'static str {
    tr!("MCP を登録しました", "MCP server registered")
}

pub fn setup_fda_header() -> &'static str {
    tr!("フルディスクアクセス", "Full Disk Access")
}

pub fn desc_fda() -> &'static str {
    tr!(
        "許可するとフォルダアクセスの確認ダイアログが出なくなる",
        "Granting it stops the repeated folder-access prompts"
    )
}

pub fn setup_fda_open() -> &'static str {
    tr!("システム設定を開く", "Open System Settings")
}

pub fn msg_opened_settings() -> &'static str {
    tr!("システム設定を開きました", "Opened System Settings")
}

pub fn setup_rules_header() -> &'static str {
    tr!("共通ルール同期", "Rules sync")
}

pub fn desc_rules() -> &'static str {
    tr!("正本 / 対象数", "Source / targets")
}

pub fn setup_rules_sync() -> &'static str {
    tr!("同期する", "Sync now")
}

pub fn msg_rules_synced() -> &'static str {
    tr!("共通ルールを同期しました", "Rules synced")
}

pub fn setup_changes_header() -> &'static str {
    tr!("セットアップ追従", "Setup updates")
}

pub fn desc_changes_none() -> &'static str {
    tr!("未適用の変更はありません", "No pending changes")
}

pub fn desc_changes_pending() -> &'static str {
    tr!("未適用の変更", "Pending changes")
}

pub fn setup_run_btn() -> &'static str {
    tr!("tako setup を実行", "Run tako setup")
}

pub fn msg_setup_started() -> &'static str {
    tr!(
        "新しいペインで tako setup を起動しました",
        "Started tako setup in a new pane"
    )
}

// --- スリープ防止タブ ---

pub fn sleep_mode_header() -> &'static str {
    tr!("スリープ防止モード", "Sleep prevention mode")
}

pub fn desc_sleep_mode() -> &'static str {
    tr!(
        "Mac が眠ってエージェントが止まるのを防ぐ",
        "Keep the Mac awake so agents keep running"
    )
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

pub fn desc_sleep_power() -> &'static str {
    tr!(
        "バッテリー駆動でも防止するかどうか",
        "Whether to keep awake on battery too"
    )
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

pub fn desc_sleep_lid() -> &'static str {
    tr!(
        "蓋を閉じても動かし続ける（sudoers の登録が必要）",
        "Keep running with the lid closed (requires a sudoers entry)"
    )
}

pub fn sleep_lid_install() -> &'static str {
    tr!("sudoers を登録", "Install sudoers entry")
}

pub fn sleep_lid_remove() -> &'static str {
    tr!("sudoers を解除", "Remove sudoers entry")
}

pub fn msg_lid_installed() -> &'static str {
    tr!("sudoers を登録しました", "Sudoers entry installed")
}

pub fn msg_lid_removed() -> &'static str {
    tr!("sudoers を解除しました", "Sudoers entry removed")
}

// --- リモートタブ ---

pub fn remote_daemon_header() -> &'static str {
    tr!("リモートデーモン", "Remote daemon")
}

pub fn remote_status_label() -> &'static str {
    tr!("状態", "Status")
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

pub fn msg_remote_started() -> &'static str {
    tr!("リモートを開始しました", "Remote started")
}

pub fn msg_remote_stopped() -> &'static str {
    tr!("リモートを停止しました", "Remote stopped")
}

pub fn remote_url_label() -> &'static str {
    tr!("接続 URL", "Connect URL")
}

pub fn desc_remote_url() -> &'static str {
    tr!(
        "スマホのブラウザで開く URL（トークンは伏せて表示）",
        "Open this on your phone (token is masked)"
    )
}

pub fn remote_setup_header() -> &'static str {
    tr!("セットアップ状態", "Setup status")
}

pub fn desc_remote_setup() -> &'static str {
    tr!(
        "Tailscale の導入・ログイン・HTTPS を確認する",
        "Check Tailscale install, login and HTTPS"
    )
}

pub fn remote_setup_ready() -> &'static str {
    tr!("準備できています", "ready")
}

pub fn remote_setup_not_ready() -> &'static str {
    tr!("未完了", "not ready")
}

pub fn remote_devices_header() -> &'static str {
    tr!("ペアリング済み端末", "Paired devices")
}

pub fn desc_remote_devices() -> &'static str {
    tr!(
        "登録済みの端末数を確認する",
        "Check how many devices are paired"
    )
}

// --- 高度タブ ---

pub fn advanced_editor_header() -> &'static str {
    tr!("settings.json 直接編集", "Edit settings.json directly")
}

pub fn advanced_edit_help() -> &'static str {
    tr!(
        "本文をクリックすると編集できる（⌘+Enter または 保存 で確定 / Esc で取消）",
        "Click the text to edit (Cmd+Enter or Save to apply, Esc to cancel)"
    )
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

pub fn advanced_open_editor() -> &'static str {
    tr!("エディタで開く", "Open in editor")
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
    fn 日英併記かつ絵文字なし_common() {
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
                button_reset().into(),
                button_reset_all().into(),
                button_save_preset().into(),
                button_delete().into(),
                button_apply().into(),
                button_refresh().into(),
                button_copy().into(),
                button_check().into(),
                button_show().into(),
                error_app_gone().into(),
                error_number().into(),
                msg_loading().into(),
                msg_refreshed().into(),
                msg_copied().into(),
                msg_reloaded().into(),
                msg_nothing_to_save().into(),
                msg_preset_saved().into(),
                msg_preset_name_required().into(),
                msg_no_presets().into(),
            ]
        });
    }

    #[test]
    fn 日英併記かつ絵文字なし_general() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                label_language().into(),
                desc_language().into(),
                lang_system().into(),
                lang_ja().into(),
                lang_en().into(),
                label_auto_rename().into(),
                desc_auto_rename().into(),
                label_port_detect().into(),
                desc_port_detect().into(),
                label_persist().into(),
                desc_persist().into(),
                desc_persist_no_tmux().into(),
                desc_persist_secondary().into(),
                label_confirm_close().into(),
                desc_confirm_close().into(),
                label_telemetry().into(),
                desc_telemetry().into(),
                label_limit_service().into(),
                desc_limit_service().into(),
                section_preview().into(),
                label_preview_reload().into(),
                desc_preview_reload().into(),
                label_preview_cache().into(),
                desc_preview_cache().into(),
                section_logs().into(),
                label_pane_logs().into(),
                desc_pane_logs().into(),
                label_pane_log_max().into(),
                desc_pane_log_max().into(),
                label_pane_log_total().into(),
                desc_pane_log_total().into(),
            ]
        });
    }

    #[test]
    fn 日英併記かつ絵文字なし_appearance() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                label_theme().into(),
                desc_theme().into(),
                theme_dark().into(),
                theme_light().into(),
                label_color_settings().into(),
                label_preset().into(),
                placeholder_preset_name().into(),
                label_font_family().into(),
                desc_font_family().into(),
                label_font_size().into(),
                desc_font_size().into(),
                category_terminal().into(),
                category_background().into(),
                category_border().into(),
                category_text().into(),
                category_accent().into(),
                category_chrome().into(),
            ]
        });
    }

    #[test]
    fn 日英併記かつ絵文字なし_runner() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                runner_header().into(),
                runner_edit_help().into(),
                runner_col_ext().into(),
                runner_col_command().into(),
                runner_placeholder_command().into(),
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
            ]
        });
    }

    #[test]
    fn 日英併記かつ絵文字なし_setup_sleep() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                setup_agents_header().into(),
                setup_installed().into(),
                setup_not_installed().into(),
                setup_mcp_header().into(),
                desc_mcp().into(),
                setup_mcp_register().into(),
                msg_mcp_registered().into(),
                setup_fda_header().into(),
                desc_fda().into(),
                setup_fda_open().into(),
                msg_opened_settings().into(),
                setup_rules_header().into(),
                desc_rules().into(),
                setup_rules_sync().into(),
                msg_rules_synced().into(),
                setup_changes_header().into(),
                desc_changes_none().into(),
                desc_changes_pending().into(),
                setup_run_btn().into(),
                msg_setup_started().into(),
                sleep_mode_header().into(),
                desc_sleep_mode().into(),
                sleep_mode_off().into(),
                sleep_mode_on().into(),
                sleep_mode_agents().into(),
                sleep_power_header().into(),
                desc_sleep_power().into(),
                sleep_power_ac().into(),
                sleep_power_always().into(),
                sleep_lid_header().into(),
                desc_sleep_lid().into(),
                sleep_lid_install().into(),
                sleep_lid_remove().into(),
                msg_lid_installed().into(),
                msg_lid_removed().into(),
            ]
        });
    }

    #[test]
    fn 日英併記かつ絵文字なし_remote_advanced() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                remote_daemon_header().into(),
                remote_status_label().into(),
                remote_status_running().into(),
                remote_status_stopped().into(),
                remote_start().into(),
                remote_stop().into(),
                msg_remote_started().into(),
                msg_remote_stopped().into(),
                remote_url_label().into(),
                desc_remote_url().into(),
                remote_setup_header().into(),
                desc_remote_setup().into(),
                remote_setup_ready().into(),
                remote_setup_not_ready().into(),
                remote_devices_header().into(),
                desc_remote_devices().into(),
                advanced_editor_header().into(),
                advanced_edit_help().into(),
                advanced_save().into(),
                advanced_reload().into(),
                advanced_open_finder().into(),
                advanced_open_editor().into(),
                advanced_related_header().into(),
                advanced_parse_error().into(),
                advanced_saved().into(),
            ]
        });
    }
}
