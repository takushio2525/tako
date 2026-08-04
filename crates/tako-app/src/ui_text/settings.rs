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

pub fn tab_profiles() -> &'static str {
    tr!("プロファイル", "Profiles")
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

pub fn label_autosuggest() -> &'static str {
    tr!("入力予測", "Input suggestions")
}

pub fn desc_autosuggest() -> &'static str {
    tr!(
        "tako 内の zsh でコマンド履歴から続きを薄く表示する（右矢印キーで確定）",
        "Show a faded completion from shell history in tako's zsh (press the right arrow to accept)"
    )
}

pub fn label_autosuggest_tab() -> &'static str {
    tr!("Tab キーでも確定", "Accept with Tab too")
}

pub fn desc_autosuggest_tab() -> &'static str {
    tr!(
        "予測が出ていてカーソルが行末にあるときだけ Tab を確定にする（それ以外の Tab は従来の補完のまま）",
        "Make Tab accept the suggestion, but only while one is shown and the cursor sits at the end of the line (Tab keeps completing otherwise)"
    )
}

pub fn label_autosuggest_hint() -> &'static str {
    tr!("確定キーの案内", "Show how to accept")
}

pub fn desc_autosuggest_hint() -> &'static str {
    tr!(
        "予測の後ろに確定キーを薄く出す（覚えるまでの案内。既定 10 回で自動的に消える）",
        "Show the accept key faded after the suggestion — a short tutorial that disappears after 10 times by default"
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
    // #566: 確認対象は「エージェント・実行中プロセスがあるペイン」に限る
    tr!(
        "エージェントや実行中プロセスがあるペインを閉じるとき確認する",
        "Ask before closing a pane that runs an agent or a process"
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

/// #550: 隠しファイル表示（ファイルツリー）
pub fn label_show_hidden_files() -> &'static str {
    tr!("隠しファイルを表示", "Show hidden files")
}

pub fn desc_show_hidden_files() -> &'static str {
    tr!(
        "ファイルツリーにドット始まり（.git / .env 等）の項目を並べる。既定は非表示",
        "List dot-prefixed items (.git, .env, ...) in the file tree. Hidden by default"
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

// --- プロファイルタブ（Issue #721）---

pub fn prof_kind_master() -> &'static str {
    tr!("master（tako master）", "master (tako master)")
}

pub fn prof_kind_solo() -> &'static str {
    tr!("solo（tako solo）", "solo (tako solo)")
}

pub fn prof_kind_header() -> &'static str {
    tr!("プロファイルの種類", "Profile type")
}

pub fn desc_prof_kind() -> &'static str {
    tr!(
        "master はチームを率いるオーケストレーター、solo は 1 対 1 で話す単独エージェント",
        "master orchestrates a team; solo is a single agent you talk to one-on-one"
    )
}

pub fn prof_list_header() -> &'static str {
    tr!("プロファイル", "Profiles")
}

pub fn prof_empty() -> &'static str {
    tr!(
        "プロファイルがありません。名前を入れて「新規作成」してください",
        "No profiles yet. Enter a name and select Create"
    )
}

pub fn prof_new_placeholder() -> &'static str {
    tr!("新しいプロファイル名", "New profile name")
}

pub fn prof_create() -> &'static str {
    tr!("新規作成", "Create")
}

pub fn prof_duplicate() -> &'static str {
    tr!("複製", "Duplicate")
}

pub fn prof_delete_confirm() -> &'static str {
    tr!(
        "このプロファイルを削除しますか？（元に戻せません）",
        "Delete this profile? This cannot be undone."
    )
}

pub fn prof_restart_note() -> &'static str {
    tr!(
        "変更は次回の起動から有効です（実行中の master / worker には影響しません）",
        "Changes take effect on the next launch; running master / worker sessions are unaffected."
    )
}

pub fn prof_launch_label() -> &'static str {
    tr!("起動コマンド", "Launch command")
}

pub fn prof_path_label() -> &'static str {
    tr!("保存先", "Saved to")
}

pub fn prof_warnings_header() -> &'static str {
    tr!("確認が必要な設定", "Settings that need attention")
}

pub fn prof_broken() -> &'static str {
    tr!(
        "この yaml は読み込めません。壊れた設定を上書きしないよう編集を止めています（高度タブか CLI で修正してください）",
        "This YAML cannot be parsed. Editing is disabled so the broken file is not overwritten; fix it via the Advanced tab or the CLI."
    )
}

pub fn prof_section_master() -> &'static str {
    tr!(
        "master（あなたが話す相手）",
        "Master (the agent you talk to)"
    )
}

pub fn prof_section_worker() -> &'static str {
    tr!(
        "worker（master が立てる子エージェント）",
        "Worker (children spawned by master)"
    )
}

pub fn prof_section_agent() -> &'static str {
    tr!("エージェント別の worker 設定", "Per-agent worker settings")
}

pub fn prof_section_projects() -> &'static str {
    tr!("担当プロジェクト", "Assigned projects")
}

pub fn prof_section_env() -> &'static str {
    tr!("環境変数", "Environment variables")
}

pub fn prof_section_other() -> &'static str {
    tr!("その他", "Other")
}

pub fn prof_label_agent() -> &'static str {
    tr!("エージェント", "Agent")
}

pub fn desc_prof_master_agent() -> &'static str {
    tr!(
        "master を起動する CLI（agy は master 非対応）",
        "CLI that runs master (agy cannot act as master)"
    )
}

pub fn desc_prof_worker_agent() -> &'static str {
    tr!(
        "spawn 時に種別を指定しなかった worker が使う CLI",
        "CLI used by workers spawned without an explicit agent"
    )
}

pub fn prof_label_model() -> &'static str {
    tr!("モデル", "Model")
}

pub fn desc_prof_model() -> &'static str {
    tr!(
        "空欄 = その CLI の既定モデル（どのプランでも起動する。推奨）",
        "Empty = that CLI's default model (works on any plan; recommended)"
    )
}

pub fn prof_label_effort() -> &'static str {
    tr!("思考の深さ（effort）", "Thinking effort")
}

pub fn prof_label_policy() -> &'static str {
    tr!("worker のモデル決定", "How workers pick a model")
}

pub fn prof_policy_inherit() -> &'static str {
    tr!("master と同じ", "Same as master")
}

pub fn prof_policy_delegate() -> &'static str {
    tr!("master が都度選ぶ", "Master decides per task")
}

pub fn prof_policy_fixed() -> &'static str {
    tr!("下の値で固定", "Fixed to the value below")
}

pub fn prof_label_worker_model() -> &'static str {
    tr!("worker のモデル", "Worker model")
}

pub fn desc_prof_worker_model() -> &'static str {
    tr!(
        "「下の値で固定」のときに使われる",
        "Used when workers are fixed to a single model"
    )
}

pub fn prof_label_worker_effort() -> &'static str {
    tr!("worker の思考の深さ", "Worker thinking effort")
}

pub fn prof_label_master_account() -> &'static str {
    tr!("master のアカウント", "Master account")
}

pub fn prof_label_worker_account() -> &'static str {
    tr!("worker のアカウント", "Worker account")
}

pub fn desc_prof_account() -> &'static str {
    tr!(
        "登録済みアカウントから選ぶ（未設定 = 既定の資格情報）",
        "Pick a registered account (Not set = default credentials)"
    )
}

pub fn prof_no_accounts() -> &'static str {
    tr!(
        "登録済みアカウントがありません（`tako orchestrator accounts add` で追加）",
        "No accounts registered (add one with `tako orchestrator accounts add`)"
    )
}

pub fn prof_no_projects() -> &'static str {
    tr!(
        "登録済みプロジェクトがありません（`tako orchestrator projects add` で追加）",
        "No projects registered (add one with `tako orchestrator projects add`)"
    )
}

pub fn desc_prof_projects() -> &'static str {
    tr!(
        "選んだプロジェクトが master の system prompt に注入される（複数選択可）",
        "Selected projects are injected into the master system prompt (multi-select)"
    )
}

pub fn prof_option_default() -> &'static str {
    tr!("既定", "Default")
}

pub fn prof_option_unset() -> &'static str {
    tr!("未設定", "Not set")
}

pub fn prof_agent_target() -> &'static str {
    tr!("設定するエージェント", "Agent to configure")
}

pub fn desc_prof_agent_target() -> &'static str {
    tr!(
        "この種別の worker だけに適用される設定",
        "Settings applied only to workers of this kind"
    )
}

pub fn prof_label_skip_permissions() -> &'static str {
    tr!("許可プロンプトをスキップ", "Skip permission prompts")
}

pub fn desc_prof_skip_permissions() -> &'static str {
    tr!(
        "worker が承認ダイアログで止まらなくなる（codex / agy は既定でオン）",
        "Keeps workers from stalling on approval dialogs (on by default for codex / agy)"
    )
}

pub fn prof_label_agent_args() -> &'static str {
    tr!("追加の CLI 引数", "Extra CLI arguments")
}

pub fn desc_prof_agent_args() -> &'static str {
    tr!(
        "スペース区切り（上級者向け）",
        "Space separated (for advanced users)"
    )
}

pub fn prof_effort_ignored() -> &'static str {
    tr!(
        "agy に effort 指定はありません（モデル名に含まれます）",
        "agy has no effort setting (it is part of the model name)"
    )
}

pub fn prof_label_tab_naming() -> &'static str {
    tr!("タブ名の命名規則", "Tab naming convention")
}

pub fn desc_prof_tab_naming() -> &'static str {
    tr!(
        "自動命名の指示（空欄 = 既定の規則）",
        "Instruction for automatic naming (empty = default rule)"
    )
}

pub fn prof_section_handoff() -> &'static str {
    tr!("自動ハンドオフ", "Automatic handoff")
}

pub fn prof_label_ctx_threshold() -> &'static str {
    tr!("引き継ぎ閾値", "Handoff threshold")
}

pub fn desc_prof_ctx_threshold() -> &'static str {
    tr!(
        "この ctx 使用率（%）を超えたら後任 master に引き継ぐ。50〜60、空欄 = 既定 60",
        "Hand off to a successor master above this context usage (%). 50-60, empty = default 60"
    )
}

pub fn prof_label_auto_handoff() -> &'static str {
    tr!("引き継ぎを自動で促す", "Prompt for handoff automatically")
}

pub fn desc_prof_auto_handoff() -> &'static str {
    tr!(
        "閾値を超えたら tako が master へ引き継ぎ開始を指示する（OFF でも手動の引き継ぎは使える）",
        "tako tells the master to start the handoff once the threshold is crossed (manual handoff still works when off)"
    )
}

pub fn prof_ctx_threshold_range() -> &'static str {
    tr!(
        "引き継ぎ閾値は 50〜60 の数字で指定してください",
        "The handoff threshold must be a number between 50 and 60"
    )
}

pub fn prof_env_add() -> &'static str {
    tr!("追加", "Add")
}

pub fn prof_env_masked() -> &'static str {
    tr!("（値は非表示）", "(value hidden)")
}

pub fn desc_prof_env() -> &'static str {
    tr!(
        "master と worker に注入される。値は画面にもログにも出さない",
        "Injected into master and workers. Values are never shown on screen or in logs."
    )
}

pub fn prof_model_1m_warning() -> &'static str {
    tr!(
        "1M コンテキスト版は Max / API プラン限定です（Pro では起動できません）",
        "The 1M-context model requires a Max / API plan (it will not start on Pro)."
    )
}

pub fn prof_name_required() -> &'static str {
    tr!("プロファイル名を入力してください", "Enter a profile name")
}

pub fn prof_cancel() -> &'static str {
    tr!("キャンセル", "Cancel")
}

#[cfg(test)]
// 注意: このクレートは #[test] の展開がコンパイラの recursion limit ぎりぎりで、
// テストを 1 本増やす / 1 本に詰め込むだけで `recursion limit reached while
// expanding #[test]` になる（limit を上げると今度は rustc がスタックオーバーフロー
// する。#486 で実測）。カタログの検証は 2 本に分けてこの範囲へ収めている
mod tests {
    use super::*;

    #[test]
    fn 日英併記かつ絵文字なし_前半() {
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
                label_language().into(),
                desc_language().into(),
                lang_system().into(),
                lang_ja().into(),
                lang_en().into(),
                label_auto_rename().into(),
                desc_auto_rename().into(),
                label_port_detect().into(),
                desc_port_detect().into(),
                label_autosuggest().into(),
                desc_autosuggest().into(),
                label_autosuggest_tab().into(),
                desc_autosuggest_tab().into(),
                label_autosuggest_hint().into(),
                desc_autosuggest_hint().into(),
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
                label_theme().into(),
                desc_theme().into(),
                label_show_hidden_files().into(),
                desc_show_hidden_files().into(),
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
                runner_header().into(),
            ]
        });
    }

    #[test]
    fn 日英併記かつ絵文字なし_後半() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
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
                // プロファイルタブ（#721）
                tab_profiles().into(),
                prof_kind_master().into(),
                prof_kind_solo().into(),
                prof_kind_header().into(),
                desc_prof_kind().into(),
                prof_list_header().into(),
                prof_empty().into(),
                prof_new_placeholder().into(),
                prof_create().into(),
                prof_duplicate().into(),
                prof_delete_confirm().into(),
                prof_restart_note().into(),
                prof_launch_label().into(),
                prof_path_label().into(),
                prof_warnings_header().into(),
                prof_broken().into(),
                prof_section_master().into(),
                prof_section_worker().into(),
                prof_section_agent().into(),
                prof_section_projects().into(),
                prof_section_env().into(),
                prof_section_other().into(),
                prof_label_agent().into(),
                desc_prof_master_agent().into(),
                desc_prof_worker_agent().into(),
                prof_label_model().into(),
                desc_prof_model().into(),
                prof_label_effort().into(),
                prof_label_policy().into(),
                prof_policy_inherit().into(),
                prof_policy_delegate().into(),
                prof_policy_fixed().into(),
                prof_label_worker_model().into(),
                desc_prof_worker_model().into(),
                prof_label_worker_effort().into(),
                prof_label_master_account().into(),
                prof_label_worker_account().into(),
                desc_prof_account().into(),
                prof_no_accounts().into(),
                prof_no_projects().into(),
                desc_prof_projects().into(),
                prof_option_default().into(),
                prof_option_unset().into(),
                prof_agent_target().into(),
                desc_prof_agent_target().into(),
                prof_label_skip_permissions().into(),
                desc_prof_skip_permissions().into(),
                prof_label_agent_args().into(),
                desc_prof_agent_args().into(),
                prof_effort_ignored().into(),
                prof_label_tab_naming().into(),
                desc_prof_tab_naming().into(),
                prof_section_handoff().into(),
                prof_label_ctx_threshold().into(),
                desc_prof_ctx_threshold().into(),
                prof_label_auto_handoff().into(),
                desc_prof_auto_handoff().into(),
                prof_ctx_threshold_range().into(),
                prof_env_add().into(),
                prof_env_masked().into(),
                desc_prof_env().into(),
                prof_model_1m_warning().into(),
                prof_name_required().into(),
                prof_cancel().into(),
            ]
        });
    }
}
