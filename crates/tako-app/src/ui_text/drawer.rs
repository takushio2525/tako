//! たまり場ドロワー（バックグラウンド退避。FR-2.15）の文言（キー: drawer.*）

pub fn confirm_destroy() -> &'static str {
    tr!("完全に破棄?", "Destroy permanently?")
}
pub fn empty() -> &'static str {
    tr!(
        "バックグラウンドのターミナルはありません",
        "No background terminals"
    )
}

/// 閉じたタブ由来グループのラベル（キー: drawer.closed_tab_group）
pub fn closed_tab_group(title: &str) -> String {
    tr!(
        format!("{title}（閉じたタブ）"),
        format!("{title} (closed tab)")
    )
}

/// タブ別グループ見出し（キー: drawer.tab_group）
pub fn tab_group(title: &str, count: usize) -> String {
    tr!(
        format!("タブ {title}（{count}）"),
        format!("Tab {title} ({count})")
    )
}

/// ドロワーヘッダー（キー: drawer.header）
pub fn header(total: usize) -> String {
    tr!(
        format!("バックグラウンドのターミナル（{total}）"),
        format!("Background terminals ({total})")
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
                confirm_destroy().to_string(),
                empty().to_string(),
                closed_tab_group("build"),
                tab_group("dev", 3),
                header(5),
            ]
        });
    }
}
