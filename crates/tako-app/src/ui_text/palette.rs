//! コマンドパレット（cmd+K）と検索ボタンの文言（キー: palette.*）

pub fn search_placeholder() -> &'static str {
    tr!("ペイン・コマンド検索", "Search panes & commands")
}
pub fn no_match() -> &'static str {
    tr!("該当なし", "No matches")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![search_placeholder().to_string(), no_match().to_string()]
        });
    }
}
