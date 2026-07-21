//! 右パネル（tmux / orch / git ビュー。#217）の文言（キー: panel.*)

// --- kill 確認（キー: panel.kill_*） ---

pub fn kill_button() -> &'static str {
    tr!("kill する", "Kill")
}
pub fn kill_cancel() -> &'static str {
    tr!("やめる", "Cancel")
}
pub fn confirm_kill_window(w: impl std::fmt::Display) -> String {
    tr!(
        format!("window {w} を kill していいですか?（中のプロセスごと終了）"),
        format!("Kill window {w}? (terminates its processes)")
    )
}
pub fn confirm_kill_session(name: &str) -> String {
    tr!(
        format!(
            "セッション {name} を kill していいですか?（中のプロセスごと終了。attach 中のペインからも消える）"
        ),
        format!("Kill session {name}? (terminates its processes and detaches all attached panes)")
    )
}
pub fn confirm_kill_pane(pane: impl std::fmt::Display) -> String {
    tr!(
        format!("ペイン {pane} を kill していいですか?（中のプロセスごと終了）"),
        format!("Kill pane {pane}? (terminates its processes)")
    )
}
pub fn confirm_kill_leftover(name: &str) -> String {
    tr!(
        format!(
            "{name} は tako の kill 漏れ残骸の可能性。kill していいですか?（中のプロセスごと終了）"
        ),
        format!("{name} looks like a leftover tako session. Kill it? (terminates its processes)")
    )
}
pub fn confirm_kill_unmanaged(name: &str) -> String {
    tr!(
        format!("管理外セッション {name} を kill していいですか?（中のプロセスごと終了）"),
        format!("Kill unmanaged session {name}? (terminates its processes)")
    )
}

// --- orch ビュー（キー: panel.orch_*） ---

pub fn orch_empty() -> &'static str {
    tr!(
        "オーケストレーターはいません（tako master で起動）",
        "No orchestrator (start one with: tako master)"
    )
}
pub fn orch_uptime_label() -> &'static str {
    tr!("稼働", "up")
}
pub fn orch_no_workers() -> &'static str {
    tr!("ワーカーなし", "No workers")
}

// --- tmux ビュー（キー: panel.tmux_*） ---

pub fn pane_count(n: impl std::fmt::Display) -> String {
    tr!(format!("{n} ペイン"), format!("{n} panes"))
}
pub fn external_badge() -> &'static str {
    tr!("外部", "external")
}
pub fn closed_tab_section() -> &'static str {
    tr!(
        "閉じたタブのターミナル（バックグラウンドで実行中）",
        "Terminals from closed tabs (still running in background)"
    )
}
pub fn closed_tab_group(title: &str, count: usize) -> String {
    tr!(
        format!("タブ {title}（閉じたタブ・{count} 件）"),
        format!("Tab {title} (closed, {count})")
    )
}

// --- git ビュー（キー: panel.git_*） ---

pub fn git_detecting() -> &'static str {
    tr!("git リポジトリを検出中…", "Detecting git repository…")
}
/// 見出しの先頭スペースはアイコンとの間隔（描画側の既存レイアウトを維持）
pub fn git_branches(n: usize) -> String {
    tr!(format!(" ブランチ ({n})"), format!(" Branches ({n})"))
}
pub fn git_changes(n: usize) -> String {
    tr!(format!(" 変更 ({n})"), format!(" Changes ({n})"))
}
pub fn git_commits(n: usize) -> String {
    tr!(format!(" コミット ({n})"), format!(" Commits ({n})"))
}
/// diff 件数の単位サフィックス（"diff (5 コミット)" = 選択コミットの diff /
/// "diff (5 ファイル)" = 作業ツリーの diff）
pub fn git_commit_tab() -> &'static str {
    tr!(" コミット", " in commit")
}
pub fn git_files_tab() -> &'static str {
    tr!(" ファイル", " files")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                kill_button().to_string(),
                kill_cancel().to_string(),
                confirm_kill_window(2),
                confirm_kill_session("dev"),
                confirm_kill_pane(7),
                confirm_kill_leftover("tako-9"),
                confirm_kill_unmanaged("misc"),
                orch_empty().to_string(),
                orch_uptime_label().to_string(),
                orch_no_workers().to_string(),
                pane_count(3),
                external_badge().to_string(),
                closed_tab_section().to_string(),
                closed_tab_group("dev", 2),
                git_detecting().to_string(),
                git_branches(2),
                git_changes(4),
                git_commits(10),
                git_commit_tab().to_string(),
                git_files_tab().to_string(),
            ]
        });
    }
}
