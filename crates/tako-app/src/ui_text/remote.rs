//! リモート接続パネル（承認ダイアログ + 端末一覧。#283）の文言（キー: remote.*）

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
pub fn stop_all() -> &'static str {
    tr!(
        "すべての接続を遮断（remote stop）",
        "Stop all connections (remote stop)"
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
                stop_all().to_string(),
            ]
        });
    }
}
