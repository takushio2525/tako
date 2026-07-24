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
pub fn git_commits(n: usize) -> String {
    tr!(format!(" コミット ({n})"), format!(" Commits ({n})"))
}
pub fn git_commit_placeholder(branch: &str) -> String {
    tr!(
        format!("メッセージ (Cmd+Enter で \"{branch}\" にコミット)"),
        format!("Message (Cmd+Enter to commit on \"{branch}\")")
    )
}
pub fn git_commit_btn() -> &'static str {
    tr!("コミット", "Commit")
}

// --- ステージング UI（#487。VSCode ソース管理の 2 セクション構造） ---

/// git リポジトリではない cwd での表示（旧: 検出中のまま止まって見えた）
pub fn git_not_a_repo() -> &'static str {
    tr!(
        "このタブに git リポジトリがありません",
        "No git repository in this tab"
    )
}
pub fn git_staged_section(n: usize) -> String {
    tr!(
        format!(" ステージ済みの変更 ({n})"),
        format!(" Staged Changes ({n})")
    )
}
pub fn git_unstaged_section(n: usize) -> String {
    tr!(format!(" 変更 ({n})"), format!(" Changes ({n})"))
}
pub fn git_no_changes() -> &'static str {
    tr!("変更はありません", "No changes")
}
/// 行ごとのステージボタンの tooltip 相当ラベル
pub fn git_stage_file() -> &'static str {
    tr!("ステージ", "Stage")
}
pub fn git_unstage_file() -> &'static str {
    tr!("アンステージ", "Unstage")
}
pub fn git_stage_all() -> &'static str {
    tr!("すべてステージ", "Stage all")
}
pub fn git_unstage_all() -> &'static str {
    tr!("すべてアンステージ", "Unstage all")
}
pub fn git_refresh() -> &'static str {
    tr!("更新", "Refresh")
}
/// diff セクションの見出し（作業ツリー diff は staged / unstaged を明示する）
pub fn git_diff_unstaged(n: usize) -> String {
    tr!(
        format!(" diff: 未ステージ ({n} ファイル)"),
        format!(" diff: unstaged ({n} files)")
    )
}
pub fn git_diff_staged(n: usize) -> String {
    tr!(
        format!(" diff: ステージ済み ({n} ファイル)"),
        format!(" diff: staged ({n} files)")
    )
}
pub fn git_diff_commit(n: usize) -> String {
    tr!(
        format!(" diff: 選択コミット ({n} ファイル)"),
        format!(" diff: selected commit ({n} files)")
    )
}
/// ステージ済みがあるときのコミットボタン注記（`-a` を付けない旨）
pub fn git_commit_staged_hint(n: usize) -> String {
    tr!(
        format!("ステージ済み {n} 件をコミット"),
        format!("Commit {n} staged file(s)")
    )
}
pub fn git_commit_all_hint() -> &'static str {
    tr!(
        "追跡中の全変更をコミット（ステージ済みなし）",
        "Commit all tracked changes (nothing staged)"
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
                git_commits(10),
                git_commit_placeholder("main"),
                git_commit_btn().to_string(),
                git_not_a_repo().to_string(),
                git_staged_section(2),
                git_unstaged_section(3),
                git_no_changes().to_string(),
                git_stage_file().to_string(),
                git_unstage_file().to_string(),
                git_stage_all().to_string(),
                git_unstage_all().to_string(),
                git_refresh().to_string(),
                git_diff_unstaged(1),
                git_diff_staged(2),
                git_diff_commit(3),
                git_commit_staged_hint(2),
                git_commit_all_hint().to_string(),
            ]
        });
    }
}
