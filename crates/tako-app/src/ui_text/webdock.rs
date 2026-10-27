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
/// 生成待ち・生成中のペインの本文（#724。Windows は生成を 1 本ずつ行う）
pub fn creating() -> &'static str {
    tr!("Web ビューを準備しています…", "Preparing the web view…")
}
/// 保留のペインの本文（#724 の W4）
pub fn held() -> &'static str {
    tr!(
        "前回この Web ビューを開いている途中で tako が終了したため、自動では開いていません",
        "tako quit while this web view was opening last time, so it was not reopened automatically"
    )
}
/// まだ wry の Web ビューが無いあいだの断り（#724。CLI / MCP の eval・focus 等の応答）。
/// `state` は `web list` の `state`（queued / creating / held / failed）
pub fn not_ready(id: u64, state: &str) -> String {
    tr!(
        format!(
            "Web ビュー {id} はまだ開いていない（state = {state}。web list の state が ready になってから。\
             held / failed は navigate で開き直す）"
        ),
        format!(
            "Web view {id} is not open yet (state = {state}). Wait until its state in web list is ready; \
             reopen a held or failed one with navigate"
        )
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
                url_placeholder().to_string(),
                open().to_string(),
                reload().to_string(),
                load_failed().to_string(),
                creating().to_string(),
                held().to_string(),
                not_ready(3, "creating"),
            ]
        });
    }
}
