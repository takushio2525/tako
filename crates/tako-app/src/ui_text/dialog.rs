//! ペイン・タブの close 確認ダイアログの文言（FR-2.2.6 / #346。キー: dialog.*）

pub fn close_pane_question() -> &'static str {
    tr!("このペインを閉じますか？", "Close this pane?")
}
pub fn close_tab_question() -> &'static str {
    tr!("このタブを閉じますか？", "Close this tab?")
}

/// 失われるものの列挙行（区切りも言語別: 「、」/ ", "）
pub fn close_loses(parts: &[String]) -> String {
    tr!(
        format!("閉じると失われるもの: {}", parts.join("、")),
        format!("Closing loses: {}", parts.join(", "))
    )
}

pub fn lost_running_process() -> &'static str {
    tr!("実行中のプロセス", "a running process")
}
pub fn lost_busy_worker() -> &'static str {
    tr!("稼働中の worker", "a busy worker")
}
pub fn lost_tmux_session() -> &'static str {
    tr!("tmux セッション", "a tmux session")
}
pub fn lost_panes(n: usize) -> String {
    tr!(format!("{n} ペイン"), format!("{n} panes"))
}
pub fn lost_running(n: usize) -> String {
    tr!(
        format!("{n} 個の実行中プロセス"),
        format!("{n} running processes")
    )
}
pub fn lost_workers(n: usize) -> String {
    tr!(
        format!("{n} 個の稼働中 worker"),
        format!("{n} busy workers")
    )
}
pub fn lost_tmux(n: usize) -> String {
    tr!(
        format!("{n} 個の tmux セッション"),
        format!("{n} tmux sessions")
    )
}

pub fn cancel_esc() -> &'static str {
    tr!("キャンセル (Esc)", "Cancel (Esc)")
}
pub fn close_enter() -> &'static str {
    tr!("閉じる (Enter)", "Close (Enter)")
}
pub fn close_skip_hint() -> &'static str {
    tr!(
        "⌘クリックで確認なしで閉じる",
        "⌘click closes without confirmation"
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
                close_pane_question().to_string(),
                close_tab_question().to_string(),
                close_loses(&["a".to_string(), "b".to_string()]),
                lost_running_process().to_string(),
                lost_busy_worker().to_string(),
                lost_tmux_session().to_string(),
                lost_panes(2),
                lost_running(1),
                lost_workers(3),
                lost_tmux(1),
                cancel_esc().to_string(),
                close_enter().to_string(),
                close_skip_hint().to_string(),
            ]
        });
    }
}
