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

/// #1067: 旧プロセスが終わらず建て直しを断念したときの理由 + 次の一手。
/// **黙って諦めない**（バナーに出して手動の逃げ道を示す）
pub fn relaunch_gave_up(pid: Option<u32>) -> String {
    let target = match pid {
        Some(pid) => format!("pid {pid}"),
        None => tr!("対象のプロセス", "the target process").to_string(),
    };
    tr!(
        format!(
            "{target} が終わらないのでセッション再起動を中止しました。\
             ペインで直接終了させてから `tako session-restart --mode harness` をやり直してください"
        ),
        format!(
            "{target} did not exit, so the session restart was aborted. \
             Quit it in the pane, then run `tako session-restart --mode harness` again"
        )
    )
}
