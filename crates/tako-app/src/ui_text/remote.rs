//! リモート接続パネル（承認ダイアログ + 端末一覧 + 起動導線。#283 / #590）の
//! 文言（キー: remote.*）

/// role 選択肢のラベル（キー: remote.role_*。role キー自体は言語非依存）
pub fn role_label(role: &str) -> &'static str {
    match role {
        "observe" => tr!("Observe（画面閲覧のみ）", "Observe (view only)"),
        "interact" => tr!("Interact（+ 入力）", "Interact (+ input)"),
        "manage" => tr!("Manage（+ 閉じる・リサイズ）", "Manage (+ close / resize)"),
        "admin" => tr!("Admin（+ 端末管理）", "Admin (+ device management)"),
        _ => "",
    }
}

pub fn connected_count(n: usize) -> String {
    tr!(format!("{n} 接続"), format!("{n} connected"))
}
pub fn pending_count(n: usize) -> String {
    tr!(format!("承認待ち {n}"), format!("{n} pending"))
}
pub fn unnamed_device() -> &'static str {
    tr!("(名称未設定)", "(unnamed)")
}
pub fn approve_role_change_title() -> &'static str {
    tr!(
        "権限の変更を許可しますか？",
        "Allow this permission change?"
    )
}
pub fn approve_connect_title() -> &'static str {
    tr!(
        "この端末を接続許可しますか？",
        "Allow this device to connect?"
    )
}
pub fn device_name(name: &str) -> String {
    tr!(format!("端末名: {name}"), format!("Device: {name}"))
}
pub fn device_user(login: &str) -> String {
    tr!(format!("ユーザー: {login}"), format!("User: {login}"))
}
pub fn device_node(node: &str) -> String {
    tr!(format!("ノード: {node}"), format!("Node: {node}"))
}
pub fn choose_role() -> &'static str {
    tr!("許可する権限を選択:", "Choose the permission to grant:")
}
pub fn deny() -> &'static str {
    tr!("拒否", "Deny")
}
pub fn approve() -> &'static str {
    tr!("許可", "Allow")
}
pub fn no_devices() -> &'static str {
    tr!("登録された端末はありません", "No registered devices")
}
pub fn connected_suffix() -> &'static str {
    tr!(" · 接続中", " · connected")
}
pub fn revoke() -> &'static str {
    tr!("失効", "Revoke")
}
pub fn panel_title() -> &'static str {
    tr!("リモート接続端末", "Remote devices")
}
pub fn connections_now(n: usize) -> String {
    tr!(format!("{n} 接続中"), format!("{n} connected"))
}

// --- 常時表示インジケータと起動導線（#590）---

/// ステータスバーのラベル: daemon 稼働中・接続なし
pub fn indicator_idle() -> &'static str {
    tr!("リモート", "remote")
}
/// ステータスバーのラベル: daemon 停止中
pub fn indicator_off() -> &'static str {
    tr!("リモート オフ", "remote off")
}
/// ステータスバーのラベル: 起動処理中
pub fn indicator_starting() -> &'static str {
    tr!("リモート 起動中", "remote starting")
}
/// 起動パネルのタイトル
pub fn start_panel_title() -> &'static str {
    tr!("リモート接続", "Remote access")
}
/// 起動パネルの説明（何ができるのか）
pub fn start_panel_desc() -> &'static str {
    tr!(
        "スマホやタブレットからこの Mac のターミナルを見る（Tailscale 経由・tailnet 内限定）。",
        "View this Mac's terminals from your phone or tablet (via Tailscale, inside your tailnet)."
    )
}
/// 起動ボタン
pub fn start_button() -> &'static str {
    tr!("リモートを起動", "Start remote access")
}
/// 起動中のボタン表示（押せない状態）
pub fn start_button_busy() -> &'static str {
    tr!("起動中…", "Starting…")
}
/// セットアップ状態の再確認ボタン
pub fn recheck_button() -> &'static str {
    tr!("再確認", "Re-check")
}
/// セットアップ状態の確認中
pub fn setup_checking() -> &'static str {
    tr!("Tailscale の状態を確認中…", "Checking Tailscale status…")
}
/// セットアップ完了（起動できる）
pub fn setup_ready() -> &'static str {
    tr!("Tailscale の準備は完了", "Tailscale is ready")
}
/// 不足項目の見出し（起動ボタンは押せるままにするので断定形にしない。押せば
/// daemon が返す理由が出る = そちらが最終的な正）
pub fn setup_missing_header() -> &'static str {
    tr!(
        "起動に必要な設定が不足しています:",
        "Setup required before starting:"
    )
}
/// 不足項目 1 件の説明（キーは `remote_setup::check_status` の item。言語非依存）
pub fn setup_item_label(item: &str) -> &'static str {
    match item {
        "tailscale" => tr!(
            "Tailscale が未導入（App Store 版アプリ または brew install tailscale）",
            "Tailscale is not installed (App Store app or brew install tailscale)"
        ),
        "daemon" => tr!(
            "Tailscale が起動していない（Tailscale アプリを起動）",
            "Tailscale is not running (launch the Tailscale app)"
        ),
        "login" => tr!(
            "Tailscale にログインしていない（tailscale up でブラウザ認証）",
            "Not logged in to Tailscale (run tailscale up to authenticate)"
        ),
        "https" => tr!(
            "tailnet の HTTPS 証明書が未有効（管理画面で MagicDNS と HTTPS Certificates を有効化）",
            "tailnet HTTPS certificates are off (enable MagicDNS and HTTPS Certificates in the admin console)"
        ),
        "dns_name" => tr!(
            "MagicDNS 名を取得できない（tailnet の DNS 設定を確認）",
            "Cannot resolve the MagicDNS name (check the tailnet DNS settings)"
        ),
        "serve" => tr!(
            "tailscale serve に tako 管理外の設定がある（tailscale serve status で確認）",
            "tailscale serve has a non-tako configuration (check tailscale serve status)"
        ),
        // 未知のキーは黙って消さず、キーをそのまま出す（黙って失敗しない。#590）
        _ => "",
    }
}
/// セットアップ手順の案内（下のコマンドを実行する）
pub fn setup_hint() -> &'static str {
    tr!(
        "ターミナルで次を実行するとセットアップできます（クリックでコピー）:",
        "Run this in a terminal to set it up (click to copy):"
    )
}
/// セットアップコマンド（言語非依存。#322 の最簡形）
pub const SETUP_COMMAND: &str = "tako remote setup";
/// 起動失敗の見出し
pub fn start_failed() -> &'static str {
    tr!("起動に失敗しました", "Failed to start")
}
/// 接続 URL の見出し（稼働中パネル）
pub fn url_label() -> &'static str {
    tr!(
        "接続 URL（クリックでコピー）",
        "Connect URL (click to copy)"
    )
}

// --- 起動 ⇔ 停止トグル（#615）---

/// 停止ボタン（起動ボタンと対。押すと確認を出す = kill switch の入口）
pub fn stop_button() -> &'static str {
    tr!("リモートを停止", "Stop remote access")
}
/// 停止処理中のボタン表示（押せない状態）
pub fn stop_button_busy() -> &'static str {
    tr!("停止中…", "Stopping…")
}
/// 停止確認の見出し
pub fn stop_confirm_title() -> &'static str {
    tr!("リモートを停止しますか？", "Stop remote access?")
}
/// 停止確認の本文（接続中の端末が切断されることを件数つきで伝える）。
/// 0 件でも「何も起きない」わけではない（登録端末は次から繋げなくなる）ので明示する
pub fn stop_confirm_body(connected: usize) -> String {
    if connected == 0 {
        tr!(
            "いま接続中の端末はありません。停止するとすべての接続を受け付けなくなります。"
                .to_string(),
            "No devices are connected right now. Stopping will refuse all connections.".to_string()
        )
    } else {
        tr!(
            format!("接続中の端末 {connected} 台が切断されます。"),
            format!("{connected} connected device(s) will be disconnected.")
        )
    }
}
/// 停止確認の実行ボタン
pub fn stop_confirm_yes() -> &'static str {
    tr!("停止する", "Stop")
}
/// 停止失敗の見出し
pub fn stop_failed() -> &'static str {
    tr!("停止に失敗しました", "Failed to stop")
}
/// ステータスバーのラベル: 停止処理中
pub fn indicator_stopping() -> &'static str {
    tr!("リモート 停止中", "remote stopping")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                role_label("observe").to_string(),
                role_label("interact").to_string(),
                role_label("manage").to_string(),
                role_label("admin").to_string(),
                connected_count(2),
                pending_count(1),
                unnamed_device().to_string(),
                approve_role_change_title().to_string(),
                approve_connect_title().to_string(),
                device_name("iPhone"),
                device_user("user"),
                device_node("node"),
                choose_role().to_string(),
                deny().to_string(),
                approve().to_string(),
                no_devices().to_string(),
                connected_suffix().to_string(),
                revoke().to_string(),
                panel_title().to_string(),
                connections_now(1),
                // #590 の起動導線
                indicator_idle().to_string(),
                indicator_off().to_string(),
                indicator_starting().to_string(),
                start_panel_title().to_string(),
                start_panel_desc().to_string(),
                start_button().to_string(),
                start_button_busy().to_string(),
                recheck_button().to_string(),
                setup_checking().to_string(),
                setup_ready().to_string(),
                setup_missing_header().to_string(),
                setup_item_label("tailscale").to_string(),
                setup_item_label("daemon").to_string(),
                setup_item_label("login").to_string(),
                setup_item_label("https").to_string(),
                setup_item_label("dns_name").to_string(),
                setup_item_label("serve").to_string(),
                setup_hint().to_string(),
                start_failed().to_string(),
                url_label().to_string(),
                // #615 の停止トグル
                stop_button().to_string(),
                stop_button_busy().to_string(),
                stop_confirm_title().to_string(),
                stop_confirm_body(0),
                stop_confirm_body(2),
                stop_confirm_yes().to_string(),
                stop_failed().to_string(),
                indicator_stopping().to_string(),
            ]
        });
    }

    /// 未知の item キーは空文字を返す（呼び出し側がキーそのものを出す。#590）
    #[test]
    fn setup_item_labelは未知キーで空を返す() {
        assert!(setup_item_label("unknown_item").is_empty());
    }
}
