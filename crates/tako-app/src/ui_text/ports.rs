//! listen ポート検知チップ（FR-2.4.3）の文言（キー: ports.*）

pub fn listening(port: u16) -> String {
    tr!(
        format!("localhost:{port} が listen 中"),
        format!("localhost:{port} is listening")
    )
}
pub fn listening_with_process(port: u16, process: &str) -> String {
    tr!(
        format!("localhost:{port}（{process}）が listen 中"),
        format!("localhost:{port} ({process}) is listening")
    )
}
pub fn open_in_browser() -> &'static str {
    tr!("ブラウザで開く", "Open in browser")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                listening(5173),
                listening_with_process(3000, "node"),
                open_in_browser().to_string(),
            ]
        });
    }
}
