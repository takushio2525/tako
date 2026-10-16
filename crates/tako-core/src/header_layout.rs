// ペインヘッダの省略制御（#185 見切れ解消 + #229 狭幅メニュー集約）
//
// 幅に応じて要素を段階的に非表示にする。
// 狭幅（bg_button が消える閾値未満）では × の代わりに「...」メニューに集約し、
// メニューからバックグラウンド退避/クローズを選択できるようにする。

/// ヘッダの各要素の表示可否を幅に応じて決定する。
/// 優先度が低い要素から順に非表示にしていく。
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderVisibility {
    pub badge: bool,
    pub title: bool,
    pub role: bool,
    pub state: bool,
    pub state_elapsed: bool,
    pub workers_dropdown: bool,
    pub parent_link: bool,
    pub cwd_chip: bool,
    /// SSH の接続待ち / 失敗の表示（#1010）。**一時的だが伝わらないと困る**ので
    /// cwd チップよりずっと早い段階から出す（狭い分割でも消えない）
    pub ssh_connect: bool,
    /// 実行ペインの実行中 / 完了 / 失敗のバッジ（#1657）。**結果が伝わらないと困る**ので
    /// `ssh_connect` と同じ早い段階から出す（右へ細く割った実行ペインでも消えない）
    pub run_badge: bool,
    pub shell_info: bool,
    pub split_button: bool,
    pub bg_button: bool,
    pub close_button: bool,
    /// 狭幅時に bg/close を「...」メニューに集約する (#229)
    pub more_menu: bool,
}

impl HeaderVisibility {
    /// 利用可能な幅 (px) からヘッダ要素の表示可否を計算する。
    /// 省略順序（優先度低→高）:
    ///   shell_info → cwd_chip → workers/parent → state_elapsed →
    ///   state → role → split → bg/close (→ more_menu に集約)
    /// ssh_connect（#1010）と run_badge（#1657）は結果が伝わらないと困るので早くから出す
    /// （badge の次に残る）
    /// 幅 < 140 では bg_button/close_button の代わりに more_menu = true となり、
    /// UI 側で「...」ボタン 1 個にまとめる。
    pub fn from_width(width: f32) -> Self {
        let more_menu = width < 140.0;
        Self {
            more_menu,
            close_button: !more_menu,
            bg_button: width >= 140.0,
            badge: width >= 80.0,
            title: width >= 100.0,
            ssh_connect: width >= 120.0,
            run_badge: width >= 120.0,
            split_button: width >= 180.0,
            role: width >= 220.0,
            state: width >= 260.0,
            state_elapsed: width >= 320.0,
            parent_link: width >= 350.0,
            workers_dropdown: width >= 350.0,
            cwd_chip: width >= 450.0,
            shell_info: width >= 500.0,
        }
    }
}

/// プレビューヘッダの各要素の表示可否を幅に応じて決定する。
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewHeaderVisibility {
    pub close_button: bool,
    pub file_name: bool,
    pub file_icon: bool,
    pub path_label: bool,
    pub run_button: bool,
    /// 実行ボタン横のシェブロン（#1733。実行設定のドロップダウンの入口）。**再生ボタンと一緒に畳む**:
    /// 設定の入口なので、ボタンが見えているのにシェブロンだけ消える幅を作らない
    pub run_chevron: bool,
    /// 再生ボタンの右に淡色で出す、選んだ実行環境の短いラベル（#1733。`.venv 3.12` / `uv run`）。
    /// 補足の表示なので再生ボタンより先に畳む（編集・表示切替より後まで残さない）
    pub run_runtime_label: bool,
    pub mode_toggle: bool,
    pub edit_button: bool,
    pub save_button: bool,
    pub page_info: bool,
}

impl PreviewHeaderVisibility {
    pub fn from_width(width: f32) -> Self {
        Self {
            close_button: true,
            file_name: width >= 80.0,
            file_icon: width >= 100.0,
            path_label: width >= 200.0,
            run_button: width >= 250.0,
            run_chevron: width >= 250.0,
            run_runtime_label: width >= 360.0,
            page_info: width >= 250.0,
            edit_button: width >= 300.0,
            mode_toggle: width >= 350.0,
            save_button: width >= 300.0,
        }
    }
}

/// パスの中間省略: パスが max_chars を超える場合、先頭と末尾を残して中間を省略する。
/// 例: `/Users/foo/Documents/projects/bar/src/main.rs` → `~/…/bar/src/main.rs`
pub fn truncate_path_middle(path: &str, max_chars: usize) -> String {
    // ~ 置換（正本は paths::shorten_home。#893 で 10 箇所を 1 本に寄せた）
    let display = crate::paths::shorten_home(path);

    let char_count = display.chars().count();
    if char_count <= max_chars || max_chars < 6 {
        return display;
    }

    // ファイル名（末尾コンポーネント）は必ず残す
    let parts: Vec<&str> = display.split('/').collect();
    if parts.len() <= 2 {
        // 1-2 コンポーネントなら末尾省略で十分（`…` 込みで max_chars 文字。#1790）
        return crate::text::truncate_chars(&display, max_chars);
    }

    let file_name = parts.last().unwrap_or(&"");
    let first = parts[0]; // "" or "~"

    // 先頭 + "…/" + 末尾のいくつかのコンポーネントを組み立て
    // 末尾から可能な限りコンポーネントを追加
    let prefix = if first.is_empty() { "/" } else { first };
    let ellipsis = "\u{2026}/";
    let prefix_len = prefix.chars().count();
    let ellipsis_len = 2; // "…/"

    let mid_parts = &parts[1..parts.len() - 1];
    let mut included_from = mid_parts.len(); // 末尾からどこまで含めたか

    let mut suffix_parts: Vec<&str> = vec![*file_name];
    let mut suffix_len = file_name.chars().count();

    for (i, &part) in mid_parts.iter().enumerate().rev() {
        let part_len = part.chars().count() + 1; // +1 for '/'
        if prefix_len + ellipsis_len + suffix_len + part_len <= max_chars {
            suffix_parts.push(part);
            suffix_len += part_len;
            included_from = i;
        } else {
            break;
        }
    }

    suffix_parts.reverse();

    // 全中間コンポーネントを含められた場合は省略不要
    if included_from == 0 {
        return display;
    }

    let suffix = suffix_parts.join("/");
    format!("{prefix}/{ellipsis}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_visibility_very_narrow_uses_more_menu() {
        // #229: 狭幅では close/bg の代わりに more_menu に集約
        for w in [30.0, 50.0, 79.0, 100.0, 139.0] {
            let v = HeaderVisibility::from_width(w);
            assert!(v.more_menu, "width={w}: must use more_menu");
            assert!(!v.close_button, "width={w}: close hidden in more_menu");
            assert!(!v.bg_button, "width={w}: bg hidden in more_menu");
        }
    }

    #[test]
    fn header_visibility_normal_width_no_more_menu() {
        for w in [140.0, 300.0, 500.0] {
            let v = HeaderVisibility::from_width(w);
            assert!(!v.more_menu, "width={w}: no more_menu at normal width");
            assert!(v.close_button, "width={w}: close shown normally");
            assert!(v.bg_button, "width={w}: bg shown normally");
        }
    }

    #[test]
    fn header_visibility_very_narrow() {
        let v = HeaderVisibility::from_width(60.0);
        assert!(v.more_menu);
        assert!(!v.close_button);
        assert!(!v.badge);
        assert!(!v.title);
        assert!(!v.role);
        assert!(!v.state);
        assert!(!v.split_button);
        assert!(!v.bg_button);
    }

    #[test]
    fn header_visibility_narrow() {
        let v = HeaderVisibility::from_width(150.0);
        assert!(!v.more_menu);
        assert!(v.close_button);
        assert!(v.badge);
        assert!(v.title);
        assert!(v.bg_button);
        assert!(!v.split_button);
        assert!(!v.role);
        assert!(!v.cwd_chip);
    }

    #[test]
    fn header_visibility_medium() {
        let v = HeaderVisibility::from_width(300.0);
        assert!(!v.more_menu);
        assert!(v.close_button);
        assert!(v.badge);
        assert!(v.title);
        assert!(v.bg_button);
        assert!(v.split_button);
        assert!(v.role);
        assert!(v.state);
        assert!(!v.state_elapsed);
        assert!(!v.cwd_chip);
    }

    #[test]
    fn header_visibility_full() {
        let v = HeaderVisibility::from_width(600.0);
        assert!(!v.more_menu);
        assert!(v.close_button);
        assert!(v.badge);
        assert!(v.title);
        assert!(v.bg_button);
        assert!(v.split_button);
        assert!(v.role);
        assert!(v.state);
        assert!(v.state_elapsed);
        assert!(v.workers_dropdown);
        assert!(v.parent_link);
        assert!(v.cwd_chip);
        assert!(v.shell_info);
    }

    #[test]
    fn header_visibility_progressive_hiding() {
        let narrow = HeaderVisibility::from_width(100.0);
        let medium = HeaderVisibility::from_width(300.0);
        let wide = HeaderVisibility::from_width(600.0);

        // narrow は more_menu モード（bg/close は集約）
        assert!(narrow.more_menu);
        assert!(!narrow.role);
        assert!(medium.role);
        assert!(!narrow.state);
        assert!(medium.state);

        assert!(!medium.cwd_chip);
        assert!(wide.cwd_chip);
        assert!(!medium.shell_info);
        assert!(wide.shell_info);
    }

    #[test]
    fn header_visibility_more_menu_boundary() {
        // 閾値 140 の境界テスト
        let below = HeaderVisibility::from_width(139.9);
        assert!(below.more_menu);
        assert!(!below.close_button);
        assert!(!below.bg_button);

        let at = HeaderVisibility::from_width(140.0);
        assert!(!at.more_menu);
        assert!(at.close_button);
        assert!(at.bg_button);
    }

    #[test]
    fn truncate_path_middle_short() {
        let path = "/a/b.txt";
        assert_eq!(truncate_path_middle(path, 20), path);
    }

    #[test]
    fn truncate_path_middle_long() {
        let path = "/very/long/deeply/nested/project/src/main.rs";
        let result = truncate_path_middle(path, 30);
        assert!(result.contains("main.rs"), "must keep filename: {result}");
        assert!(
            result.chars().count() <= 30,
            "must respect limit: {result} ({})",
            result.chars().count()
        );
        assert!(result.contains('\u{2026}'), "must have ellipsis: {result}");
    }

    #[test]
    fn truncate_path_middle_keeps_filename() {
        let path = "/a/b/c/d/e/very_long_filename.rs";
        let result = truncate_path_middle(path, 25);
        assert!(
            result.contains("very_long_filename.rs"),
            "must keep filename: {result}"
        );
    }

    #[test]
    fn truncate_path_middle_two_components() {
        let path = "/file.txt";
        assert_eq!(truncate_path_middle(path, 20), path);
    }

    /// 1〜2 コンポーネントのパスの末尾省略の固定値（#1790）。`…` は上限の内側
    /// （`…` 込みで `max_chars` 文字）・数えるのは `char`
    #[test]
    fn truncate_path_middle_末尾省略の固定値() {
        // ルート直下の長いファイル名（`""` と名前の 2 コンポーネント）
        let root_file = format!("/{}.txt", "あ".repeat(30));
        assert_eq!(
            truncate_path_middle(&root_file, 20),
            format!("/{}…", "あ".repeat(18))
        );
        // ちょうど上限は切らず、1 文字あふれたら `…` 込みで上限へ収める
        let exact = format!("/{}", "あ".repeat(19));
        assert_eq!(truncate_path_middle(&exact, 20), exact);
        let over = format!("/{}", "あ".repeat(20));
        assert_eq!(
            truncate_path_middle(&over, 20),
            format!("/{}…", "あ".repeat(18))
        );
        // `/` の無い 1 コンポーネント（4 バイト文字・結合文字）
        assert_eq!(
            truncate_path_middle(&"🎉".repeat(12), 10),
            format!("{}…", "🎉".repeat(9))
        );
        assert_eq!(
            truncate_path_middle(&"か\u{3099}".repeat(6), 7),
            "か\u{3099}か\u{3099}か\u{3099}…"
        );
        // `\` 区切りは割らないので Windows のパスはこの分岐に入る
        assert_eq!(
            truncate_path_middle("C:\\work\\プロジェクト\\src\\main.rs", 12),
            "C:\\work\\プロジ…"
        );
        // 上限が 6 未満なら省略しない
        assert_eq!(truncate_path_middle(&root_file, 5), root_file);
    }

    #[test]
    fn preview_header_always_shows_close() {
        for w in [30.0, 50.0, 79.0, 100.0, 500.0] {
            let v = PreviewHeaderVisibility::from_width(w);
            assert!(v.close_button, "width={w}: close must always be visible");
        }
    }

    #[test]
    fn preview_header_progressive() {
        let narrow = PreviewHeaderVisibility::from_width(90.0);
        assert!(narrow.file_name);
        assert!(!narrow.file_icon);
        assert!(!narrow.path_label);
        assert!(!narrow.run_button);

        let medium = PreviewHeaderVisibility::from_width(250.0);
        assert!(medium.file_name);
        assert!(medium.file_icon);
        assert!(medium.path_label);
        assert!(medium.run_button);
        assert!(!medium.edit_button);

        let wide = PreviewHeaderVisibility::from_width(400.0);
        assert!(wide.edit_button);
        assert!(wide.mode_toggle);
        assert!(wide.run_button);
    }

    #[test]
    fn preview_header_run_button_threshold() {
        let below = PreviewHeaderVisibility::from_width(249.0);
        assert!(!below.run_button);
        let at = PreviewHeaderVisibility::from_width(250.0);
        assert!(at.run_button);
    }

    /// #1733: 狭い幅での畳み方。実行環境のラベルが先に消え（360 未満）、再生ボタンとシェブロンは
    /// 一緒に消える（250 未満）。ボタンが見えているのにシェブロン（設定の入口）だけ消える幅は無い
    #[test]
    fn preview_header_run_dropdown_folds_label_then_button() {
        let narrow = PreviewHeaderVisibility::from_width(249.0);
        assert!(!narrow.run_button && !narrow.run_chevron && !narrow.run_runtime_label);

        let button_only = PreviewHeaderVisibility::from_width(250.0);
        assert!(button_only.run_button && button_only.run_chevron);
        assert!(
            !button_only.run_runtime_label,
            "250 では実行環境のラベルを出さない"
        );

        let below_label = PreviewHeaderVisibility::from_width(359.0);
        assert!(below_label.run_chevron && !below_label.run_runtime_label);

        let label = PreviewHeaderVisibility::from_width(360.0);
        assert!(label.run_button && label.run_chevron && label.run_runtime_label);

        // 全幅で: ラベル ⇒ ボタン、シェブロン ⇔ ボタン
        for w in 0..=1000 {
            let v = PreviewHeaderVisibility::from_width(w as f32);
            assert_eq!(
                v.run_chevron, v.run_button,
                "width={w}: シェブロンはボタンと一緒に畳む"
            );
            assert!(
                !v.run_runtime_label || v.run_button,
                "width={w}: ラベルだけ残らない"
            );
        }
    }
}
