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

/// タブ単位で退避したタブの見出し（キー: drawer.shelved_tab_group。#1487）。
/// 中のペインは分解されていないので「ペイン数」ではなく「タブ 1 枚」として見せる
pub fn shelved_tab_group(title: &str, count: usize) -> String {
    tr!(
        format!("タブ {title}（退避中・{count} ペイン）"),
        format!("Tab {title} (shelved, {count} panes)")
    )
}

/// 退避タブをまとめて戻すボタン（キー: drawer.restore_tab。#1487）
pub fn restore_tab() -> &'static str {
    tr!("タブごと復帰", "Restore tab")
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
                shelved_tab_group("dev", 3),
                restore_tab().to_string(),
                header(5),
            ]
        });
    }
}
