//! stale claude バイナリ通知バナーの文言（Issue #498）

pub fn banner_message(current: &str, spawned: &str) -> String {
    tr!(
        format!("claude {current} が利用可能です（このセッションは {spawned}）"),
        format!("claude {current} available (this session is on {spawned})")
    )
}

pub fn restart_button() -> &'static str {
    tr!("張り直す", "Restart")
}

pub fn handoff_button() -> &'static str {
    tr!("引き継ぐ", "Handoff")
}

pub fn restarting() -> &'static str {
    tr!("張り直し中...", "Restarting...")
}

pub fn restart_failed() -> &'static str {
    tr!("張り直し失敗", "Restart failed")
}
