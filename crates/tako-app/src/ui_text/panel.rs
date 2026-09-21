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
/// tmux window 行のクリックが失敗したときに通知欄へ出す**操作名**（#1417）。
/// 「押した行の切り替え」と「失敗の名前」を一致させるためにここで持つ
pub fn op_select_window() -> &'static str {
    tr!("window 切替", "Switch window")
}
/// tmux セッションの「復元」ボタンを押したときに通知欄へ出す**操作名**（#1422）
pub fn op_restore_session() -> &'static str {
    tr!("セッションの復元", "Restore session")
}
/// 復元したペインの PTY を起動できなかったときの**操作名**（#1023 / #1422）
pub fn op_start_terminal() -> &'static str {
    tr!("ターミナルの起動", "Start terminal")
}
/// バックグラウンドのターミナルを画面へ戻すときの**操作名**（#1422）
pub fn op_unshelve_pane() -> &'static str {
    tr!("バックグラウンドから復帰", "Restore from background")
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

/// タブ単位で退避したタブの節見出し（キー: panel.shelved_tab_section。#1487）
pub fn shelved_tab_section() -> &'static str {
    tr!(
        "退避中のタブ（分割のまま裏で実行中）",
        "Shelved tabs (still running in background)"
    )
}

/// 退避タブ 1 枚のラベル（キー: panel.shelved_tab_group。#1487）
pub fn shelved_tab_group(title: &str, count: usize) -> String {
    tr!(
        format!("タブ {title}（退避中・{count} ペイン）"),
        format!("Tab {title} (shelved, {count} panes)")
    )
}

/// 退避タブをまとめて戻す操作名（通知欄の失敗表示にも使う。キー: panel.op_unshelve_tab。#1487）
pub fn op_unshelve_tab() -> &'static str {
    tr!("タブごと復帰", "Restore tab")
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
/// コミットメッセージ欄のプレースホルダ。
///
/// `key` は確定の打鍵（macOS = `Cmd+Enter`）。**確定は GPUI の platform 修飾を
/// 直接見ている**ので Windows では Win+Enter になり、押せる打鍵が無い。
/// そのときは `None` を渡して打鍵に触れない文にする（隣のコミットボタンが入口。#1203）
pub fn git_commit_placeholder(branch: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => tr!(
            format!("メッセージ ({key} で \"{branch}\" にコミット)"),
            format!("Message ({key} to commit on \"{branch}\")")
        ),
        None => tr!(
            format!("メッセージ (\"{branch}\" にコミット)"),
            format!("Message (commit on \"{branch}\")")
        ),
    }
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
#[allow(dead_code)]
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
/// コミットできない理由（#494。ボタン無効化だけでは理由が分からないので必ず言葉で出す）
pub fn git_commit_blocked_empty() -> &'static str {
    tr!(
        "コミットするにはメッセージを入力してください",
        "Enter a message to commit"
    )
}
pub fn git_commit_blocked_no_changes() -> &'static str {
    tr!(
        "コミットできる変更がありません",
        "No changes available to commit"
    )
}
/// 実行中（#494。二重押し防止でボタンを無効化している間の表示）
pub fn git_busy(op: &str) -> String {
    tr!(format!("{op} を実行中..."), format!("Running {op}..."))
}
/// フィードバックカードを閉じる
pub fn git_dismiss() -> &'static str {
    tr!("閉じる", "Dismiss")
}
/// メッセージが上限に達した
pub fn git_commit_message_too_long(max: usize) -> String {
    tr!(
        format!("コミットメッセージが上限（{max} バイト）に達しました"),
        format!("Commit message reached the limit ({max} bytes)")
    )
}
/// detached HEAD の表示（ブランチ名の代わり）
pub fn git_detached_head() -> &'static str {
    tr!("detached HEAD", "detached HEAD")
}

// --- ブランチ操作（#496 Part 1） ---

pub fn git_remote_branches(n: usize) -> String {
    tr!(format!(" リモート ({n})"), format!(" Remote ({n})"))
}
/// ブランチセクション右端の新規作成ボタン
pub fn git_branch_new() -> &'static str {
    tr!("新規", "New")
}
pub fn git_branch_new_placeholder() -> &'static str {
    tr!("新しいブランチ名", "New branch name")
}
pub fn git_branch_from(base: &str) -> String {
    tr!(format!("基点: {base}"), format!("From: {base}"))
}
pub fn git_branch_create_btn() -> &'static str {
    tr!("作成", "Create")
}
pub fn git_cancel() -> &'static str {
    tr!("キャンセル", "Cancel")
}
/// ブランチ行のマージボタン（#562: 常時薄く表示し、ホバーで強調する）
pub fn git_merge_btn() -> &'static str {
    tr!("マージ", "Merge")
}
/// ブランチセクションの操作案内（#562: マージ導線が見つけられない問題への対処）
pub fn git_branch_hint() -> &'static str {
    tr!(
        "行クリックで切替 / 右の「マージ」で現在ブランチへ取り込み",
        "Click a row to switch / \"Merge\" pulls it into the current branch"
    )
}
/// 変更ファイル行クリックでプレビューを開けなかったとき（#560）
pub fn git_preview_missing(path: &str) -> String {
    tr!(
        format!("ファイルが見つからないのでプレビューできません: {path}"),
        format!("Cannot preview: file not found: {path}")
    )
}
pub fn git_confirm_run() -> &'static str {
    tr!("実行", "Run")
}
/// 事前提示カードの見出し（#496: 黙って実行せず、何が起きるかを先に出す）
pub fn git_checkout_confirm_title(branch: &str) -> String {
    tr!(
        format!("'{branch}' へ切り替えます"),
        format!("Switch to '{branch}'")
    )
}
pub fn git_merge_confirm_title(branch: &str) -> String {
    tr!(
        format!("'{branch}' を現在のブランチへ取り込みます"),
        format!("Merge '{branch}' into the current branch")
    )
}
pub fn git_preview_changed(n: usize) -> String {
    tr!(
        format!("内容が入れ替わるファイル: {n} 件"),
        format!("Files replaced by the switch: {n}")
    )
}
pub fn git_preview_carried(n: usize) -> String {
    tr!(
        format!("切替後もそのまま残る未コミット変更: {n} 件"),
        format!("Uncommitted changes carried over: {n}")
    )
}
pub fn git_preview_blocking(n: usize) -> String {
    tr!(
        format!("切替を妨げる未コミット変更: {n} 件（先にコミットか退避が必要）"),
        format!("Uncommitted changes blocking the switch: {n} (commit or stash first)")
    )
}
/// 切替を実行できない理由（#496。一覧の内訳とは別に「だからできない」を 1 行で言う）
pub fn git_checkout_blocked() -> &'static str {
    tr!(
        "このまま切り替えると変更が失われるため実行できません",
        "Cannot switch: the change would be lost"
    )
}
pub fn git_preview_creates_local(branch: &str) -> String {
    tr!(
        format!("リモート追跡ブランチから '{branch}' を作成します"),
        format!("Creates local branch '{branch}' tracking the remote")
    )
}
pub fn git_merge_kind_label(kind: &str) -> String {
    match kind {
        "up-to-date" => tr!(
            "すでに取り込み済み（何も起きません）".to_string(),
            "Already up to date (nothing happens)".to_string()
        ),
        "fast-forward" => tr!(
            "早送り（コンフリクトは起きません）".to_string(),
            "Fast-forward (no conflicts possible)".to_string()
        ),
        "unrelated" => tr!(
            "共通の祖先がありません".to_string(),
            "No common ancestor".to_string()
        ),
        _ => tr!(
            "3-way マージ（マージコミットを作ります）".to_string(),
            "Three-way merge (creates a merge commit)".to_string()
        ),
    }
}
pub fn git_merge_incoming(n: usize) -> String {
    tr!(
        format!("取り込むコミット: {n} 件"),
        format!("Commits to merge in: {n}")
    )
}
pub fn git_merge_changed(n: usize) -> String {
    tr!(
        format!("変更されるファイル: {n} 件"),
        format!("Files changed: {n}")
    )
}
pub fn git_merge_predicted(n: usize) -> String {
    tr!(
        format!("コンフリクトの予測: {n} 件"),
        format!("Predicted conflicts: {n}")
    )
}
pub fn git_merge_no_conflict() -> &'static str {
    tr!("コンフリクトは予測されていません", "No conflicts predicted")
}
pub fn git_merge_prediction_unavailable() -> &'static str {
    tr!(
        "この git ではコンフリクトを事前予測できません",
        "This git version cannot predict conflicts"
    )
}

// --- コンフリクトカード（#496 Part 2） ---

pub fn git_conflict_title(operation: &str) -> String {
    let op = match operation {
        "merging" => tr!("マージ", "merge"),
        "rebasing" => tr!("リベース", "rebase"),
        "cherry-picking" => tr!("チェリーピック", "cherry-pick"),
        "reverting" => tr!("リバート", "revert"),
        other => other,
    };
    tr!(
        format!("{op}中にコンフリクトが発生しています"),
        format!("Conflicts during {op}")
    )
}
pub fn git_conflict_branches(ours: &str, theirs: &str) -> String {
    tr!(format!("{ours} ← {theirs}"), format!("{ours} <- {theirs}"))
}
pub fn git_conflict_files(n: usize) -> String {
    tr!(
        format!("未解決ファイル ({n})"),
        format!("Unresolved files ({n})")
    )
}
/// 一覧が長いときの省略表示（カードがパネル高さを食い潰さないための上限。#496）
pub fn git_conflict_more_files(n: usize) -> String {
    tr!(format!("ほか {n} 件"), format!("and {n} more"))
}
/// 未解決ゼロ = 解決済みでコミット待ち
pub fn git_conflict_all_resolved() -> &'static str {
    tr!(
        "未解決ファイルはありません（コミットすれば完了します）",
        "No unresolved files left (commit to finish)"
    )
}
pub fn git_conflict_abort(operation: &str) -> String {
    let op = match operation {
        "merging" => tr!("マージ", "merge"),
        "rebasing" => tr!("リベース", "rebase"),
        "cherry-picking" => tr!("チェリーピック", "cherry-pick"),
        "reverting" => tr!("リバート", "revert"),
        other => other,
    };
    tr!(format!("{op}を中止"), format!("Abort {op}"))
}
pub fn git_conflict_resolve_agent() -> &'static str {
    tr!("解消エージェントを起動", "Start resolve agent")
}
/// エージェント選択ドロップダウンの見出し
pub fn git_agent_pick() -> &'static str {
    tr!("エージェントを選ぶ", "Choose an agent")
}
pub fn git_resolve_agent_started(agent: &str, pane: u64) -> String {
    tr!(
        format!("{agent} をペイン {pane} で起動しました（プロンプト投入中）"),
        format!("Started {agent} in pane {pane} (delivering prompt)")
    )
}

// --- ユーザー向けタスク（tasks ビュー。#1450 の分割 B2。キー: panel.tasks_*） ---

/// 一覧が空（**未完了が 0 件**）のときの 1 行
pub fn tasks_empty() -> &'static str {
    tr!("人がやることはありません", "Nothing is waiting for you")
}
/// 絞り込みの結果だけが 0 件（未完了はある）ときの 1 行。
/// 「タスクが無い」と「絞り込みで消えた」を言い分ける（押しても無言にしない）
pub fn tasks_empty_filtered() -> &'static str {
    tr!(
        "この絞り込みに当てはまるものはありません",
        "No tasks match this filter"
    )
}
pub fn tasks_open_count(n: usize) -> String {
    tr!(format!("{n} 件待ち"), format!("{n} waiting"))
}
pub fn tasks_filter_all() -> &'static str {
    tr!("すべて", "All")
}
/// 完了・却下も一覧に出すトグル（既定は未完了のみ）
pub fn tasks_show_done() -> &'static str {
    tr!("完了も見る", "Show completed")
}
pub fn tasks_kind(kind: &str) -> String {
    match kind {
        "review" => tr!("レビュー", "Review"),
        "confirm" => tr!("確認", "Confirm"),
        "permission" => tr!("許可", "Permission"),
        "post" => tr!("投稿", "Post"),
        "other" => tr!("その他", "Other"),
        other => other,
    }
    .to_string()
}
pub fn tasks_body_empty() -> &'static str {
    tr!("（説明はありません）", "(no description)")
}
pub fn tasks_attachments() -> &'static str {
    tr!("添付", "Attachments")
}
/// 起票時には在ったが**今は無い**添付（起票時に実在を問わない設計の裏返し）
pub fn tasks_attachment_missing() -> &'static str {
    tr!("見つかりません", "Missing")
}
pub fn tasks_open_preview() -> &'static str {
    tr!("プレビューで開く", "Open preview")
}
/// 添付が tako の中で**どのプレビューで開くか**（#1472）。
/// 綴りの正本は `tako_core::open_plan::PreviewRoute::as_str`（画面は拡張子表を持たない）
pub fn tasks_attachment_route(route: &str) -> String {
    match route {
        "image" => tr!("画像", "Image"),
        "video" => tr!("動画", "Video"),
        "pdf" => tr!("PDF", "PDF"),
        "markdown" => tr!("Markdown", "Markdown"),
        "code" => tr!("テキスト", "Text"),
        other => other,
    }
    .to_string()
}
/// サムネイルを出さなかった理由（#1472）。**行は押せるまま**なので行き止まりにしない
pub fn tasks_thumb_skipped() -> &'static str {
    tr!("サムネイルなし", "No thumbnail")
}
pub fn tasks_copy_texts() -> &'static str {
    tr!("コピー用のテキスト", "Text to copy")
}
pub fn tasks_copy_button() -> &'static str {
    tr!("コピー", "Copy")
}
pub fn tasks_copied(label: &str) -> String {
    if label.is_empty() {
        tr!("コピーしました".to_string(), "Copied".to_string())
    } else {
        tr!(
            format!("「{label}」をコピーしました"),
            format!("Copied: {label}")
        )
    }
}
pub fn tasks_links() -> &'static str {
    tr!("リンク", "Links")
}
pub fn tasks_due(due: &str) -> String {
    tr!(format!("期限 {due}"), format!("Due {due}"))
}
pub fn tasks_respond_heading() -> &'static str {
    tr!("返答", "Respond")
}
pub fn tasks_decision(decision: &str) -> String {
    match decision {
        "approve" => tr!("承認", "Approve"),
        "reject" => tr!("却下", "Reject"),
        "needs_change" => tr!("直してほしい", "Needs change"),
        "answered" => tr!("回答", "Answered"),
        other => other,
    }
    .to_string()
}
pub fn tasks_comment_placeholder() -> &'static str {
    tr!("コメント（任意）", "Comment (optional)")
}
pub fn tasks_send() -> &'static str {
    tr!("返す", "Send")
}
/// 判断を選ぶまで「返す」を押せない理由（誤爆防止をユーザーに言う）
pub fn tasks_pick_decision() -> &'static str {
    tr!("判断を選ぶと返せます", "Pick a decision to send")
}
pub fn tasks_comment_limit(max: usize) -> String {
    tr!(
        format!("コメントはここまでです（上限 {max} 文字）"),
        format!("Comment is at its limit ({max} characters)")
    )
}
pub fn tasks_thread() -> &'static str {
    tr!("やりとり", "Thread")
}
pub fn tasks_done() -> &'static str {
    tr!("完了", "Done")
}
pub fn tasks_dismiss() -> &'static str {
    tr!("却下", "Dismiss")
}
/// 配送の状態。**`sent` を「届いた」と書かない**（B1 と同じ物差し。
/// 分からないものを分かったことにしない）
pub fn tasks_delivery(state: &str) -> String {
    match state {
        "sent" => tr!("送信済み（確認待ち）", "Sent (awaiting confirmation)"),
        "launched" => tr!("master を起動しました", "Launched master"),
        "delivered" => tr!("master に届きました", "Delivered to master"),
        "failed" => tr!("届きませんでした", "Delivery failed"),
        other => other,
    }
    .to_string()
}
pub fn tasks_delivery_label() -> &'static str {
    tr!("配送", "Delivery")
}
/// やりとりの時刻（**相対**で出す）。絶対時刻はタイムゾーンの解釈が要るうえ、
/// スレッドで知りたいのは「さっきか、ずっと前か」なので相対に倒す
pub fn tasks_ago(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        return tr!("たった今".to_string(), "just now".to_string());
    }
    if secs < 3600 {
        let n = secs / 60;
        return tr!(format!("{n} 分前"), format!("{n} min ago"));
    }
    if secs < 86_400 {
        let n = secs / 3600;
        return tr!(format!("{n} 時間前"), format!("{n} h ago"));
    }
    let n = secs / 86_400;
    tr!(format!("{n} 日前"), format!("{n} d ago"))
}

/// 一覧そのものが引けなかったとき（**画面を空にして黙らない**）
pub fn tasks_load_failed(reason: &str) -> String {
    tr!(
        format!("タスクを読めませんでした: {reason}"),
        format!("Could not load tasks: {reason}")
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
                op_select_window().to_string(),
                op_restore_session().to_string(),
                op_start_terminal().to_string(),
                op_unshelve_pane().to_string(),
                closed_tab_section().to_string(),
                closed_tab_group("dev", 2),
                shelved_tab_section().to_string(),
                shelved_tab_group("dev", 2),
                op_unshelve_tab().to_string(),
                git_detecting().to_string(),
                git_branches(2),
                git_commits(10),
                git_commit_placeholder("main", tests_support::mac_modifier_enter().as_deref()),
                git_commit_placeholder("main", None),
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
                git_commit_blocked_empty().to_string(),
                git_commit_blocked_no_changes().to_string(),
                git_busy("commit"),
                git_dismiss().to_string(),
                git_commit_message_too_long(4096),
                git_detached_head().to_string(),
                // #496: ブランチ操作 + コンフリクトカード
                git_remote_branches(2),
                git_branch_new().to_string(),
                git_branch_new_placeholder().to_string(),
                git_branch_from("main"),
                git_branch_create_btn().to_string(),
                git_cancel().to_string(),
                git_merge_btn().to_string(),
                git_confirm_run().to_string(),
                git_checkout_confirm_title("feat"),
                git_merge_confirm_title("feat"),
                git_preview_changed(3),
                git_preview_carried(1),
                git_preview_blocking(2),
                git_preview_creates_local("release"),
                git_checkout_blocked().to_string(),
                git_merge_kind_label("three-way"),
                git_merge_kind_label("fast-forward"),
                git_merge_kind_label("up-to-date"),
                git_merge_kind_label("unrelated"),
                git_merge_incoming(2),
                git_merge_changed(4),
                git_merge_predicted(1),
                git_merge_no_conflict().to_string(),
                git_merge_prediction_unavailable().to_string(),
                git_conflict_title("merging"),
                git_conflict_branches("main", "feat"),
                git_conflict_files(2),
                git_conflict_more_files(3),
                git_conflict_all_resolved().to_string(),
                git_conflict_abort("merging"),
                git_conflict_resolve_agent().to_string(),
                git_agent_pick().to_string(),
                git_resolve_agent_started("claude", 7),
                // #1450 B2: ユーザー向けタスク
                tasks_empty().to_string(),
                tasks_empty_filtered().to_string(),
                tasks_open_count(3),
                tasks_filter_all().to_string(),
                tasks_show_done().to_string(),
                tasks_kind("review"),
                tasks_kind("confirm"),
                tasks_kind("permission"),
                tasks_kind("post"),
                tasks_kind("other"),
                tasks_body_empty().to_string(),
                tasks_attachments().to_string(),
                tasks_attachment_missing().to_string(),
                tasks_open_preview().to_string(),
                // #1472: 添付の種別ラベルとサムネイルの但し書き
                tasks_attachment_route("image"),
                tasks_attachment_route("video"),
                tasks_attachment_route("pdf"),
                tasks_attachment_route("markdown"),
                tasks_attachment_route("code"),
                tasks_thumb_skipped().to_string(),
                tasks_copy_texts().to_string(),
                tasks_copy_button().to_string(),
                tasks_copied("tags"),
                tasks_copied(""),
                tasks_links().to_string(),
                tasks_due("2026-09-20"),
                tasks_respond_heading().to_string(),
                tasks_decision("approve"),
                tasks_decision("reject"),
                tasks_decision("needs_change"),
                tasks_decision("answered"),
                tasks_comment_placeholder().to_string(),
                tasks_send().to_string(),
                tasks_pick_decision().to_string(),
                tasks_comment_limit(4000),
                tasks_thread().to_string(),
                tasks_done().to_string(),
                tasks_dismiss().to_string(),
                tasks_delivery("sent"),
                tasks_delivery("launched"),
                tasks_delivery("delivered"),
                tasks_delivery("failed"),
                tasks_delivery_label().to_string(),
                tasks_ago(5),
                tasks_ago(120),
                tasks_ago(7200),
                tasks_ago(200_000),
                tasks_load_failed("no data dir"),
            ]
        });
    }
}
