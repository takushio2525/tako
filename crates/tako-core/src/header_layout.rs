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
    // ~ 置換
    let display = if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && path.starts_with(&home) {
            format!("~{}", &path[home.len()..])
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };

    let char_count = display.chars().count();
    if char_count <= max_chars || max_chars < 6 {
        return display;
    }

    // ファイル名（末尾コンポーネント）は必ず残す
    let parts: Vec<&str> = display.split('/').collect();
    if parts.len() <= 2 {
        // 1-2 コンポーネントなら末尾省略で十分
        let cut: String = display.chars().take(max_chars.saturating_sub(1)).collect();
        return format!("{cut}\u{2026}");
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

        let medium = PreviewHeaderVisibility::from_width(250.0);
        assert!(medium.file_name);
        assert!(medium.file_icon);
        assert!(medium.path_label);
        assert!(!medium.edit_button);

        let wide = PreviewHeaderVisibility::from_width(400.0);
        assert!(wide.edit_button);
        assert!(wide.mode_toggle);
    }
}
