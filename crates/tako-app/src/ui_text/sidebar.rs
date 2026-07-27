//! 左サイドバー（ファイルツリー）の文言（キー: sidebar.*）

// --- コンテキストメニュー（FR-3.12 / #314。キー: sidebar.menu_*） ---

pub fn menu_copy_rel() -> &'static str {
    tr!("相対パスをコピー", "Copy relative path")
}
pub fn menu_copy_abs() -> &'static str {
    tr!("絶対パスをコピー", "Copy absolute path")
}
pub fn menu_reveal() -> &'static str {
    tr!("Finder で表示", "Reveal in Finder")
}
pub fn menu_open_term() -> &'static str {
    tr!("ターミナルで開く", "Open in terminal")
}
pub fn menu_open_default() -> &'static str {
    tr!("デフォルトアプリで開く", "Open with default app")
}
pub fn menu_open_with() -> &'static str {
    tr!("このアプリで開く...", "Open with...")
}
pub fn menu_rename() -> &'static str {
    tr!("名前変更", "Rename")
}
pub fn menu_new_file() -> &'static str {
    tr!("新しいファイル", "New file")
}
pub fn menu_new_dir() -> &'static str {
    tr!("新しいフォルダ", "New folder")
}
pub fn menu_trash() -> &'static str {
    tr!("削除", "Move to Trash")
}
pub fn menu_remove_root() -> &'static str {
    tr!("ツリーから除去", "Remove from tree")
}

// --- ヘッダのトグル（#550。キー: sidebar.hidden_*） ---

pub fn hidden_show() -> &'static str {
    tr!("隠しファイルを表示", "Show hidden files")
}
pub fn hidden_hide() -> &'static str {
    tr!("隠しファイルを隠す", "Hide hidden files")
}

// --- 新規作成のインライン入力（#559。キー: sidebar.new_*） ---

pub fn new_file_placeholder() -> &'static str {
    tr!("ファイル名", "File name")
}
pub fn new_dir_placeholder() -> &'static str {
    tr!("フォルダ名", "Folder name")
}
pub fn rename_placeholder() -> &'static str {
    tr!("新しい名前", "New name")
}

// --- プレビュー編集の通知（FR-3.5。キー: sidebar.note_*） ---

pub fn note_save_before_mode_switch() -> &'static str {
    tr!(
        "未保存の変更を保存してから表示モードを切り替えてください",
        "Save your changes before switching the view mode"
    )
}
pub fn note_external_change() -> &'static str {
    tr!(
        "外部変更を検知しました。編集中の内容を保持し、自動更新は行いません",
        "External changes detected. Your edits are kept; auto-reload is paused"
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                menu_copy_rel().to_string(),
                menu_copy_abs().to_string(),
                menu_reveal().to_string(),
                menu_open_term().to_string(),
                menu_open_default().to_string(),
                menu_open_with().to_string(),
                menu_rename().to_string(),
                menu_new_file().to_string(),
                menu_new_dir().to_string(),
                menu_trash().to_string(),
                menu_remove_root().to_string(),
                hidden_show().to_string(),
                hidden_hide().to_string(),
                new_file_placeholder().to_string(),
                new_dir_placeholder().to_string(),
                rename_placeholder().to_string(),
                note_save_before_mode_switch().to_string(),
                note_external_change().to_string(),
            ]
        });
    }
}
