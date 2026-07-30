//! AI コマンド提案カード（FR-2.22 / #666）の文言（キー: command_card.*）

/// カードの既定見出し（AI が label を付けなかったとき）
pub fn heading() -> &'static str {
    tr!("実行するコマンド", "Run this command")
}
/// コピーボタン
pub fn copy() -> &'static str {
    tr!("コピー", "Copy")
}
/// コピー直後のボタン表示（成功フィードバック）
pub fn copied() -> &'static str {
    tr!("コピーしました", "Copied")
}
/// 新しいペインで実行するボタン
pub fn run() -> &'static str {
    tr!("新規ペインで実行", "Run in new pane")
}
/// 複数コマンドのときの番号ラベル
pub fn index_label(index: usize, total: usize) -> String {
    tr!(
        format!("{index}/{total} 件目"),
        format!("{index} of {total}")
    )
}
/// 実行に失敗したときの表示（新ペインを作れない等）。
/// 具体的な理由は dispatch のエラー文（日本語固定 = i18n 対象外）なので画面には出さず、
/// 診断ログへ回す。画面には「押したのに何も起きない」を避けるための一言だけ出す
pub fn run_failed() -> &'static str {
    tr!("実行できませんでした", "Could not run")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                heading().to_string(),
                copy().to_string(),
                copied().to_string(),
                run().to_string(),
                index_label(2, 3),
                run_failed().to_string(),
            ]
        });
    }
}
