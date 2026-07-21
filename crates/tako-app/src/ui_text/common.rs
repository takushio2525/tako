//! 複数の UI が共有する汎用文言（キー: common.*）

pub fn yes() -> &'static str {
    tr!("はい", "Yes")
}
pub fn no() -> &'static str {
    tr!("いいえ", "No")
}
pub fn cancel() -> &'static str {
    tr!("キャンセル", "Cancel")
}
pub fn restore() -> &'static str {
    tr!("復帰", "Restore")
}
pub fn update() -> &'static str {
    tr!("更新", "Update")
}

/// タイトル未確定ペインの既定タイトル（キー: common.terminal_fallback_title）
pub fn terminal_fallback_title() -> &'static str {
    tr!("ターミナル", "Terminal")
}

/// 経過秒の相対表記（tmux ビューの作成日時等。キー: common.format_age）
pub fn format_age(seconds: i64) -> String {
    let s = seconds.max(0);
    tr!(
        match s {
            s if s < 60 => format!("{s} 秒前"),
            s if s < 3600 => format!("{} 分前", s / 60),
            s if s < 86400 => format!("{} 時間前", s / 3600),
            s => format!("{} 日前", s / 86400),
        },
        match s {
            s if s < 60 => format!("{s}s ago"),
            s if s < 3600 => format!("{}m ago", s / 60),
            s if s < 86400 => format!("{}h ago", s / 3600),
            s => format!("{}d ago", s / 86400),
        }
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
                yes().to_string(),
                no().to_string(),
                cancel().to_string(),
                restore().to_string(),
                update().to_string(),
                terminal_fallback_title().to_string(),
                format_age(5),
                format_age(90),
                format_age(7200),
                format_age(200_000),
            ]
        });
    }
}
