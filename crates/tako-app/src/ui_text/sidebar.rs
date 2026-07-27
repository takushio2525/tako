//! 左サイドバー（ファイルツリー）の文言（キー: sidebar.*）

// --- コンテキストメニュー（FR-3.12 / #314。キー: sidebar.menu_*） ---

pub fn menu_copy_rel() -> &'static str {
    tr!("相対パスをコピー", "Copy relative path")
}
pub fn menu_copy_abs() -> &'static str {
    tr!("絶対パスをコピー", "Copy absolute path")
}
/// ファイルマネージャの名前は OS ごとに違う（#617）。
/// 「Finder で表示」を Windows で出すと、押しても何も起きないうえに何を指すか伝わらない
pub fn menu_reveal() -> &'static str {
    if cfg!(windows) {
        tr!("エクスプローラーで表示", "Reveal in Explorer")
    } else {
        tr!("Finder で表示", "Reveal in Finder")
    }
}

/// ディレクトリ（ペインの cwd）をファイルマネージャで開く
pub fn menu_reveal_dir() -> &'static str {
    if cfg!(windows) {
        tr!("エクスプローラーで開く", "Open in Explorer")
    } else {
        tr!("Finder で開く", "Open in Finder")
    }
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
/// ごみ箱の呼び名も OS ごとに違う。**この操作が復元可能であること**を
/// ラベルで約束しているので、実装（B8 の `move_to_trash`）と表記を必ず揃える（#617）
pub fn menu_trash() -> &'static str {
    if cfg!(windows) {
        tr!("ごみ箱に移動", "Move to Recycle Bin")
    } else {
        tr!("削除", "Move to Trash")
    }
}
pub fn menu_remove_root() -> &'static str {
    tr!("ツリーから除去", "Remove from tree")
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
                menu_reveal_dir().to_string(),
                menu_open_term().to_string(),
                menu_open_default().to_string(),
                menu_open_with().to_string(),
                menu_rename().to_string(),
                menu_new_file().to_string(),
                menu_new_dir().to_string(),
                menu_trash().to_string(),
                menu_remove_root().to_string(),
                note_save_before_mode_switch().to_string(),
                note_external_change().to_string(),
            ]
        });
    }
}
