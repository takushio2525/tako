//! macOS アプリケーションメニューの項目名（キー: menu.*。Issue #485）
//!
//! メニューは起動時に `cx.set_menus` で OS へ渡すため、言語切替時は
//! `TakoApp::render` の言語監視が `app_menus()` を再構築して貼り直す。

// --- メニュー名 -------------------------------------------------------------

/// アプリメニュー（先頭）はアプリ名そのものなので言語非依存
pub const APP: &str = "tako";

pub fn file() -> &'static str {
    tr!("ファイル", "File")
}
pub fn edit() -> &'static str {
    tr!("編集", "Edit")
}
pub fn view() -> &'static str {
    tr!("表示", "View")
}
pub fn window() -> &'static str {
    tr!("ウインドウ", "Window")
}
pub fn help() -> &'static str {
    tr!("ヘルプ", "Help")
}

// --- tako メニュー ----------------------------------------------------------

pub fn about() -> &'static str {
    tr!("tako について", "About tako")
}
pub fn check_updates() -> &'static str {
    tr!("アップデートを確認…", "Check for Updates…")
}
pub fn settings() -> &'static str {
    tr!("設定…", "Settings…")
}
pub fn services() -> &'static str {
    tr!("サービス", "Services")
}
pub fn hide_app() -> &'static str {
    tr!("tako を隠す", "Hide tako")
}
pub fn hide_others() -> &'static str {
    tr!("ほかを隠す", "Hide Others")
}
pub fn show_all() -> &'static str {
    tr!("すべてを表示", "Show All")
}
pub fn quit() -> &'static str {
    tr!("tako を終了", "Quit tako")
}

// --- ファイル ---------------------------------------------------------------

pub fn new_tab() -> &'static str {
    tr!("新規タブ", "New Tab")
}
pub fn new_window() -> &'static str {
    tr!("新規ウインドウ", "New Window")
}
pub fn split_right() -> &'static str {
    tr!("ペインを右に分割", "Split Pane Right")
}
pub fn split_down() -> &'static str {
    tr!("ペインを下に分割", "Split Pane Down")
}
pub fn open_directory() -> &'static str {
    tr!("フォルダを開く…", "Open Directory…")
}
pub fn open_repository() -> &'static str {
    tr!("リポジトリを開く…", "Open Repository…")
}
pub fn open_remote() -> &'static str {
    tr!("リモート接続…", "Open Remote…")
}
pub fn open_recent() -> &'static str {
    tr!("最近使った項目…", "Open Recent…")
}
pub fn save_preview() -> &'static str {
    tr!("プレビューを保存", "Save Preview")
}
pub fn close_pane() -> &'static str {
    tr!("ペインを閉じる", "Close Pane")
}

// --- 編集 -------------------------------------------------------------------

pub fn undo() -> &'static str {
    tr!("取り消す", "Undo")
}
pub fn redo() -> &'static str {
    tr!("やり直す", "Redo")
}
pub fn copy() -> &'static str {
    tr!("コピー", "Copy")
}
pub fn paste() -> &'static str {
    tr!("ペースト", "Paste")
}
pub fn select_all() -> &'static str {
    tr!("すべてを選択", "Select All")
}
pub fn find() -> &'static str {
    tr!("プレビュー内を検索…", "Find in Preview…")
}

// --- 表示 -------------------------------------------------------------------

pub fn command_palette() -> &'static str {
    tr!("コマンドパレット…", "Command Palette…")
}
pub fn toggle_sidebar() -> &'static str {
    tr!("ファイルツリーを開閉", "Toggle File Tree")
}
pub fn toggle_drawer() -> &'static str {
    tr!("バックグラウンドドロワーを開閉", "Toggle Background Drawer")
}
pub fn panel() -> &'static str {
    tr!("パネル", "Panel")
}
pub fn panel_fleet() -> &'static str {
    tr!("fleet ビュー", "Fleet View")
}
pub fn panel_orch() -> &'static str {
    tr!("orch ビュー", "Orch View")
}
pub fn panel_git() -> &'static str {
    tr!("git ビュー", "Git View")
}
pub fn zoom_in() -> &'static str {
    tr!("文字を大きく", "Zoom In")
}
pub fn zoom_out() -> &'static str {
    tr!("文字を小さく", "Zoom Out")
}
pub fn reset_zoom() -> &'static str {
    tr!("文字サイズを戻す", "Reset Zoom")
}
pub fn toggle_theme() -> &'static str {
    tr!("ライト / ダークを切替", "Toggle Light/Dark Theme")
}
/// 言語切替は両言語でネイティブ表記を併記する（palette::cmd_label と同方針。
/// 英語側に「日本語」を含む意図的な例外のため訳し漏れ検査の対象外）
pub fn switch_language() -> &'static str {
    tr!(
        "表示言語を切替（日本語 / English）",
        "Switch Language (日本語 / English)"
    )
}
pub fn toggle_fullscreen() -> &'static str {
    tr!("フルスクリーンを切替", "Toggle Full Screen")
}

// --- ウインドウ -------------------------------------------------------------

pub fn minimize() -> &'static str {
    tr!("しまう", "Minimize")
}
pub fn zoom_window() -> &'static str {
    tr!("拡大 / 縮小", "Zoom")
}
pub fn next_tab() -> &'static str {
    tr!("次のタブ", "Next Tab")
}
pub fn prev_tab() -> &'static str {
    tr!("前のタブ", "Previous Tab")
}
pub fn select_pane() -> &'static str {
    tr!("ペインを選択", "Select Pane")
}
pub fn focus_left() -> &'static str {
    tr!("左のペイン", "Pane on the Left")
}
pub fn focus_right() -> &'static str {
    tr!("右のペイン", "Pane on the Right")
}
pub fn focus_up() -> &'static str {
    tr!("上のペイン", "Pane Above")
}
pub fn focus_down() -> &'static str {
    tr!("下のペイン", "Pane Below")
}

// --- ヘルプ -----------------------------------------------------------------

pub fn documentation() -> &'static str {
    tr!("tako ドキュメント", "tako Documentation")
}
pub fn report_issue() -> &'static str {
    tr!("問題を報告", "Report an Issue")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                file().to_string(),
                edit().to_string(),
                view().to_string(),
                window().to_string(),
                help().to_string(),
                about().to_string(),
                check_updates().to_string(),
                settings().to_string(),
                services().to_string(),
                hide_app().to_string(),
                hide_others().to_string(),
                show_all().to_string(),
                quit().to_string(),
                new_tab().to_string(),
                new_window().to_string(),
                split_right().to_string(),
                split_down().to_string(),
                open_directory().to_string(),
                open_repository().to_string(),
                open_remote().to_string(),
                open_recent().to_string(),
                save_preview().to_string(),
                close_pane().to_string(),
                undo().to_string(),
                redo().to_string(),
                copy().to_string(),
                paste().to_string(),
                select_all().to_string(),
                find().to_string(),
                command_palette().to_string(),
                toggle_sidebar().to_string(),
                toggle_drawer().to_string(),
                panel().to_string(),
                panel_fleet().to_string(),
                panel_orch().to_string(),
                panel_git().to_string(),
                zoom_in().to_string(),
                zoom_out().to_string(),
                reset_zoom().to_string(),
                toggle_theme().to_string(),
                toggle_fullscreen().to_string(),
                minimize().to_string(),
                zoom_window().to_string(),
                next_tab().to_string(),
                prev_tab().to_string(),
                select_pane().to_string(),
                focus_left().to_string(),
                focus_right().to_string(),
                focus_up().to_string(),
                focus_down().to_string(),
                documentation().to_string(),
                report_issue().to_string(),
                // switch_language は意図的にネイティブ表記併記のため対象外（上記コメント）
            ]
        });
    }
}
