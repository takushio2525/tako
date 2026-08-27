//! ペインの右クリックメニューの項目名（キー: pane_menu.*）
//!
//! #813 で自動復帰のトグルを足すにあたり、それまで render へ直書きだった
//! 既存項目もここへ集約した（1 つだけ i18n 化すると英語表示で日本語が混ざるため）

use tako_control::platform::os_integration::FileManager;

pub fn copy_path() -> &'static str {
    tr!("パスをコピー", "Copy path")
}
/// ファイルマネージャの呼び名は OS ごとに違う（#617。`sidebar::menu_reveal` と同じ理由）
pub fn reveal(fm: FileManager) -> &'static str {
    match fm {
        FileManager::Finder => tr!("Finder で表示", "Reveal in Finder"),
        FileManager::Explorer => tr!("エクスプローラーで表示", "Reveal in Explorer"),
    }
}
pub fn open_default() -> &'static str {
    tr!("デフォルトアプリで開く", "Open with default app")
}
pub fn copy_cwd() -> &'static str {
    tr!("cwd をコピー", "Copy cwd")
}
/// ディレクトリ（ペインの cwd）をファイルマネージャで開く
pub fn reveal_cwd(fm: FileManager) -> &'static str {
    match fm {
        FileManager::Finder => tr!("Finder で開く", "Open in Finder"),
        FileManager::Explorer => tr!("エクスプローラーで開く", "Open in Explorer"),
    }
}
pub fn split_right() -> &'static str {
    tr!("右に分割", "Split right")
}
pub fn split_down() -> &'static str {
    tr!("下に分割", "Split down")
}
/// このペインを SSH 接続にする（#1006）。
///
/// ファイルメニューの「リモート接続…」（新しいペインを開く）とは動作が違うので、
/// **「このペインで」を文言に入れて**取り違えを防ぐ
pub fn connect_remote() -> &'static str {
    tr!("このペインでリモート接続…", "Connect this pane via SSH…")
}
pub fn background() -> &'static str {
    tr!("バックグラウンドへ", "Send to background")
}
pub fn close() -> &'static str {
    tr!("閉じる", "Close")
}

/// 利用上限後の自動復帰のトグル（#813）。現在値で文言が入れ替わる
pub fn limit_resume_toggle(enabled: bool) -> &'static str {
    if enabled {
        tr!(
            "リミット後の自動復帰を無効にする",
            "Disable auto-resume after limit"
        )
    } else {
        tr!(
            "リミット後の自動復帰を有効にする",
            "Enable auto-resume after limit"
        )
    }
}

/// ペインヘッダのインジケータの説明（#813。ホバー時のツールチップ相当）
pub fn limit_resume_indicator() -> &'static str {
    tr!(
        "リミット後の自動復帰が有効",
        "Auto-resume after limit is on"
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
                copy_path().to_string(),
                reveal(FileManager::Finder).to_string(),
                reveal(FileManager::Explorer).to_string(),
                open_default().to_string(),
                copy_cwd().to_string(),
                reveal_cwd(FileManager::Finder).to_string(),
                reveal_cwd(FileManager::Explorer).to_string(),
                split_right().to_string(),
                split_down().to_string(),
                connect_remote().to_string(),
                background().to_string(),
                close().to_string(),
                limit_resume_toggle(false).to_string(),
                limit_resume_toggle(true).to_string(),
                limit_resume_indicator().to_string(),
            ]
        });
    }
}
