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

/// #1967: 起動し直したエージェントがすぐ終わった / 送り届けられなかったときの理由 + 次の一手。
/// `reason` は `session_restart::RelaunchFailure` の綴り（`last_restart.reason` と同じ）
pub fn relaunch_failed(reason: &str, exit_code: Option<i32>) -> String {
    let code = exit_code.map_or_else(String::new, |c| format!(" (exit {c})"));
    match reason {
        "conversation_not_found" => tr!(
            format!(
                "再開した claude が会話を見つけられずに終わりました{code}。\
                 `tako sessions list` で会話の記録を確かめ、`tako sessions resume <id>` で開き直してください"
            ),
            format!(
                "The resumed claude could not find the conversation and exited{code}. \
                 Check the record with `tako sessions list`, then reopen it with `tako sessions resume <id>`"
            )
        ),
        "command_flow_timeout" => tr!(
            "シェルへ再開コマンドを送り届けられませんでした。ペインの状態を確かめて `tako session-restart --mode harness` をやり直してください"
                .to_string(),
            "The resume command could not be delivered to the shell. Check the pane, then run `tako session-restart --mode harness` again"
                .to_string()
        ),
        _ => tr!(
            format!(
                "再開したエージェントがすぐ終わりました{code}。\
                 ペインの表示で理由を確かめて `tako session-restart --mode harness` をやり直してください"
            ),
            format!(
                "The resumed agent exited right away{code}. \
                 Check the pane for the reason, then run `tako session-restart --mode harness` again"
            )
        ),
    }
}
