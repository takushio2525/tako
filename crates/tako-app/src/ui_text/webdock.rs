//! Web ビューペイン + dock（FR-3.8 / #155）の文言（キー: webdock.*）

pub fn url_placeholder() -> &'static str {
    tr!(
        "URL を入力して Enter（例: example.com）",
        "Enter a URL and press Enter (e.g. example.com)"
    )
}
pub fn open() -> &'static str {
    tr!("開く", "Open")
}
pub fn reload() -> &'static str {
    tr!("再読み込み", "Reload")
}
pub fn load_failed() -> &'static str {
    tr!("ページの読み込みに失敗しました", "Failed to load the page")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                url_placeholder().to_string(),
                open().to_string(),
                reload().to_string(),
                load_failed().to_string(),
            ]
        });
    }
}
