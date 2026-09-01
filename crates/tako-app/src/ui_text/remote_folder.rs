//! 「リモートからフォルダを開く」（#919 / #65）の文言（キー: remote_folder.*）
//!
//! 失敗の理由そのもの（何が起きたか / 次に何をすべきか）は
//! `tako_core::remote_fs::RemoteError` が日英で持っている。ここに置くのは
//! **画面の枠組み**（メニュー名・見出し・状態行・操作名）だけ

// --- メニュー・パレット -----------------------------------------------------

/// ファイルメニューの項目名。英語は Zed / VSCode の慣習に合わせる
pub fn menu_open_remote_folder() -> &'static str {
    tr!("リモートからフォルダを開く…", "Open Remote Folder…")
}

/// ホスト選択のプレースホルダ
pub fn pick_host_placeholder() -> &'static str {
    tr!(
        "SSH ホストを選ぶ（~/.ssh/config）",
        "Pick an SSH host (~/.ssh/config)"
    )
}

/// ~/.ssh/config に Host が 1 つも無いときの案内（空の一覧を黙って出さない）
pub fn no_hosts() -> &'static str {
    tr!(
        "~/.ssh/config に Host が登録されていません",
        "No Host entries found in ~/.ssh/config"
    )
}

/// フォルダ選択のプレースホルダ
pub fn pick_dir_placeholder() -> &'static str {
    tr!(
        "フォルダを選ぶ（Enter で移動）",
        "Pick a folder (Enter to descend)"
    )
}

/// 「ここを開く」項目のラベル
pub fn open_this_folder(path: &str) -> String {
    tr!(
        format!("このフォルダを開く: {path}"),
        format!("Open this folder: {path}")
    )
}

/// 1 つ上へ
pub fn parent_dir() -> &'static str {
    tr!("上のフォルダへ", "Go to parent folder")
}

/// 接続中（ホスト名つき）
pub fn connecting(host: &str) -> String {
    tr!(
        format!("{host} へ接続しています…"),
        format!("Connecting to {host}…")
    )
}

/// 接続できた（ホスト名つき）
pub fn connected(host: &str, path: &str) -> String {
    tr!(
        format!("{host} に接続しました（{path}）"),
        format!("Connected to {host} ({path})")
    )
}

/// フォルダを開いた
pub fn opened(label: &str) -> String {
    tr!(
        format!("リモートフォルダを開きました: {label}"),
        format!("Opened remote folder: {label}")
    )
}

/// フォルダを開いてターミナルも繋いだ（#1041）
pub fn opened_with_terminal(label: &str) -> String {
    tr!(
        format!("リモートフォルダを開き、ターミナルを {label} へ繋いでいます"),
        format!("Opened remote folder and connecting a terminal to {label}")
    )
}

/// フォルダは開いたがターミナルは繋がなかった（#1041。理由つき）
pub fn opened_terminal_skipped(label: &str, note: &str) -> String {
    tr!(
        format!("リモートフォルダを開きました: {label}（ターミナルは繋いでいません: {note}）"),
        format!("Opened remote folder: {label} (no terminal: {note})")
    )
}

/// フォルダを閉じた
pub fn closed(label: &str) -> String {
    tr!(
        format!("リモートフォルダを閉じました: {label}"),
        format!("Closed remote folder: {label}")
    )
}

// --- ツリーの状態行（#919: 静かな失敗を作らない） ---------------------------

pub fn row_loading() -> &'static str {
    tr!("読み込み中…", "Loading…")
}

pub fn row_empty() -> &'static str {
    tr!("（空のフォルダ）", "(empty folder)")
}

// --- SSH バッジ・自動追加（#976） -------------------------------------------

/// リモートルート行のバッジの見出し。**言語に依らない**ので `pub const`
/// （`.agent/conventions.md`「UI 文字列の i18n」の但し書き）。
/// ホスト名はこの後ろに素で並べる = 絵文字なしで「SSH のフォルダ」が読める
pub const BADGE_LABEL: &str = "SSH";

/// 検知していた ssh セッションが消えたルートに出す状態（**行は消さない**）
pub fn badge_disconnected() -> &'static str {
    tr!("切断", "offline")
}

/// ペインの ssh を検知して自動で開いたときの通知（#976）
pub fn auto_added(label: &str) -> String {
    tr!(
        format!("SSH を検知: {label} をツリーへ追加しました"),
        format!("Detected SSH: added {label} to the tree")
    )
}

/// 検知はしたが自動で開けなかったときの通知（理由つき。黙って何もしない、をしない）
pub fn auto_skipped(host: &str, reason: &str) -> String {
    tr!(
        format!("{host}: 自動追加を見送りました（{reason}）"),
        format!("{host}: skipped auto-add ({reason})")
    )
}

/// 切断を検知したときの通知
pub fn auto_disconnected(host: &str) -> String {
    tr!(
        format!("{host}: SSH が切断されました（フォルダは残します）"),
        format!("{host}: SSH disconnected (the folder stays in the tree)")
    )
}

// --- 右クリックメニュー -----------------------------------------------------

pub fn menu_copy_remote_path() -> &'static str {
    tr!("リモートのパスをコピー", "Copy remote path")
}

pub fn menu_open_ssh_pane() -> &'static str {
    tr!("このフォルダで SSH ペインを開く", "Open SSH pane here")
}

pub fn menu_reload() -> &'static str {
    tr!("再読み込み", "Reload")
}

pub fn menu_close_remote_root() -> &'static str {
    tr!("リモートフォルダを閉じる", "Close remote folder")
}

// --- プレビュー -------------------------------------------------------------

/// **書けない**リモートファイル（mode のどこにも `w` が無い）は読み取り専用（#966）
pub fn preview_read_only() -> &'static str {
    tr!(
        "このリモートファイルは書き込み権限がありません（読み取り専用）",
        "You do not have write permission for this remote file (read-only)"
    )
}

// --- 書き戻し（#966） -------------------------------------------------------

/// リモートへ押し出し中（ローカルの写しへは書けている）
pub fn preview_pushing() -> &'static str {
    tr!("リモートへ保存しています…", "Saving to the remote host…")
}

/// リモートへ書けた
pub fn preview_pushed(label: &str) -> String {
    tr!(
        format!("リモートへ保存しました: {label}"),
        format!("Saved to the remote host: {label}")
    )
}

/// 押し出せず退避した（無言で消えないことを伝える）
pub fn preview_push_pending() -> &'static str {
    tr!(
        "リモートへ送れていません（編集内容はローカルに残っています。`tako remote-folder push` で再試行）",
        "Not sent to the remote host yet (your edit is kept locally; retry with `tako remote-folder push`)"
    )
}

/// プレビューヘッダに出す「どこのファイルか」
pub fn preview_origin(label: &str) -> String {
    tr!(format!("リモート: {label}"), format!("Remote: {label}"))
}

// --- 進行状況の可視化（#1010） ---------------------------------------------

/// ツリーで読み込み中のファイル行に添える説明（アイコンだけだと意味が伝わらない）
pub fn file_loading() -> &'static str {
    tr!("読み込み中", "Loading")
}

/// ペインが SSH の接続待ちのあいだ出す文言。**ホスト名を必ず入れる**
/// （どこへ繋ごうとしているのか分からないまま待たせない）
pub fn pane_connecting(host: &str) -> String {
    tr!(
        format!("{host} へ接続中…"),
        format!("Connecting to {host}…")
    )
}

/// 接続に失敗したときに**接続中の文言と置き換える**もの（黙って消さない）。
/// `reason` は ssh 自身が出した行（読めなければ画面を見るよう促す）
pub fn pane_connect_failed(host: &str, reason: &str) -> String {
    tr!(
        format!("{host} へ接続できません: {reason}"),
        format!("Cannot connect to {host}: {reason}")
    )
}

/// 失敗表示のクリックで閉じられることの案内（ツールチップ）
pub fn pane_connect_dismiss() -> &'static str {
    tr!("クリックで閉じる", "Click to dismiss")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 日英併記かつ絵文字なし() {
        crate::ui_text::tests_support::check_ja_en(|| {
            vec![
                menu_open_remote_folder().into(),
                pick_host_placeholder().into(),
                no_hosts().into(),
                pick_dir_placeholder().into(),
                open_this_folder("/srv/app"),
                parent_dir().into(),
                connecting("host"),
                connected("host", "/srv/app"),
                opened("host:/srv/app"),
                closed("host:/srv/app"),
                row_loading().into(),
                row_empty().into(),
                // #976: SSH バッジと自動追加の通知
                badge_disconnected().into(),
                auto_added("host:/srv/app"),
                auto_skipped("host", "reason"),
                auto_disconnected("host"),
                menu_copy_remote_path().into(),
                menu_open_ssh_pane().into(),
                menu_reload().into(),
                menu_close_remote_root().into(),
                preview_read_only().into(),
                preview_pushing().into(),
                preview_pushed("host:/srv/app"),
                preview_push_pending().into(),
                preview_origin("host:/srv/app"),
                // #1010: 進行状況の可視化
                file_loading().into(),
                pane_connecting("host"),
                pane_connect_failed("host", "ssh: connect to host host port 22: timed out"),
                pane_connect_dismiss().into(),
            ]
        });
    }
}
