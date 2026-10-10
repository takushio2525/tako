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

/// 親 master ごとの見出し（キー: drawer.master_group。#1946）。
/// `label` はプロファイル名（あればペインのタイトルを添える）
pub fn master_group(label: &str, count: usize) -> String {
    tr!(
        format!("master {label}（{count}）"),
        format!("master {label} ({count})")
    )
}

/// 親 master が退避中であることの添え書き（キー: drawer.master_shelved。#1946）
pub fn master_shelved() -> &'static str {
    tr!("退避中", "shelved")
}

/// 親が閉じたまとまりの見出し（キー: drawer.parent_gone。#1946）
pub fn parent_gone(pane: u64, count: usize) -> String {
    tr!(
        format!("親 #{pane} は閉じた（{count}）"),
        format!("parent #{pane} closed ({count})")
    )
}

/// 親 master が分からないまとまりの見出し（キー: drawer.no_master。#1946）
pub fn no_master(count: usize) -> String {
    tr!(
        format!("親 master なし（{count}）"),
        format!("no parent master ({count})")
    )
}

/// master 自身のカードの印（キー: drawer.master_badge。#1946）
pub fn master_badge() -> &'static str {
    "master"
}

/// 器の無い退避エントリ（幽霊）の見出し（キー: drawer.ghost_title。#1946）
pub fn ghost_title() -> &'static str {
    tr!(
        "器なし（GUI 再起動で失われた）",
        "No vessel (lost on GUI restart)"
    )
}

/// 幽霊の後始末の案内（キー: drawer.ghost_hint。#1946）
pub fn ghost_hint() -> &'static str {
    tr!(
        "閉じるボタンで一覧から外せます（他の器には触れません）",
        "Remove it with the close button (other vessels are untouched)"
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
                master_group("tako", 2),
                master_shelved().to_string(),
                parent_gone(12, 1),
                no_master(3),
                ghost_title().to_string(),
                ghost_hint().to_string(),
            ]
        });
    }
}
