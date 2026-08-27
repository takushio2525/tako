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
