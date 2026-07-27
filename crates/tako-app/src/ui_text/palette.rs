//! コマンドパレット（cmd+K）と検索ボタンの文言（キー: palette.*）

pub fn search_placeholder() -> &'static str {
    tr!("ペイン・コマンド検索", "Search panes & commands")
}
pub fn no_match() -> &'static str {
    tr!("該当なし", "No matches")
}

/// 固定コマンドの表示ラベル（キー: palette.cmd_<id>）
pub fn cmd_label(id: &str) -> &'static str {
    match id {
        // #549: 初期設定・オーケストレーションの導線（初回起動バナーと同じ入口）
        "run-setup" => tr!("セットアップを実行", "Run setup"),
        "run-master" => tr!("master を起動", "Start master"),
        "open-settings" => tr!("設定を開く", "Open settings"),
        "new-tab" => tr!("新しいタブ", "New tab"),
        "toggle-theme" => tr!("テーマをライト/ダーク切替", "Toggle light/dark theme"),
        "toggle-files" => tr!("ファイルツリーを開閉", "Toggle file tree"),
        "toggle-drawer" => tr!("バックグラウンドドロワーを開閉", "Toggle background drawer"),
        "panel-fleet" => tr!("fleet パネルを開く", "Open fleet panel"),
        "panel-orch" => tr!("orch パネルを開く", "Open orch panel"),
        "panel-git" => tr!("git パネルを開く", "Open git panel"),
        "split-right" => tr!("ペインを右に分割", "Split pane right"),
        "split-down" => tr!("ペインを下に分割", "Split pane down"),
        // #552: いまのタブ名を固定して自動リネームの上書きを止める
        "pin-tab-title" => tr!("このタブ名を固定", "Pin this tab name"),
        // 言語切替は両言語でネイティブ表記を併記（切替先を字面で探せるように）。
        // 英語側に「日本語」を含む意図的な例外のため、訳し漏れ検査の対象外
        "toggle-language" => tr!(
            "表示言語を切替（日本語 / English）",
            "Switch language (日本語 / English)"
        ),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                search_placeholder().to_string(),
                no_match().to_string(),
                cmd_label("run-setup").to_string(),
                cmd_label("run-master").to_string(),
                cmd_label("open-settings").to_string(),
                cmd_label("new-tab").to_string(),
                cmd_label("toggle-theme").to_string(),
                cmd_label("toggle-files").to_string(),
                cmd_label("toggle-drawer").to_string(),
                cmd_label("panel-fleet").to_string(),
                cmd_label("panel-orch").to_string(),
                cmd_label("panel-git").to_string(),
                cmd_label("split-right").to_string(),
                cmd_label("split-down").to_string(),
                cmd_label("pin-tab-title").to_string(),
                // toggle-language は意図的にネイティブ表記併記のため対象外（上記コメント）
            ]
        });
    }
}
