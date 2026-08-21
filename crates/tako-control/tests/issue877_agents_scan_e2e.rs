//! #877 の実機 e2e: エージェント走査（`claude agents --json`）が実 claude を観測し、
//! `worker_status` が使うのと同じ経路で状態を引けること。
//!
//! **稼働中の claude 対話セッションが要る**ので `--ignored` で明示実行する:
//!
//! ```text
//! cargo test -p tako-control --test issue877_agents_scan_e2e -- --ignored --nocapture
//! ```
//!
//! Windows でこれが通ることが #877 の受け入れそのもの（修正前は走査自体が失敗して
//! `list_agents` が `Err` を返し、`query_agent_status` は必ず `unknown` だった）。
//! 認証は要らない: 実測（2026-08-21 / Windows 11 / claude 2.1.238）では
//! `Not logged in` の TUI でも `agents --json` には `status: idle` で載る

/// 走査 → `session_id` → 状態取得までを実物で通す
#[test]
#[ignore = "稼働中の claude 対話セッションが要る（実機 e2e）"]
fn 実claudeを走査してagentsソースの状態を取れる() {
    let agents = match tako_control::agents::list_agents() {
        Ok(agents) => agents,
        Err(e) => panic!("走査が失敗した: {e}（#877 の症状。TAKO_FLOW_DIAG=1 で理由が出る）"),
    };
    println!("走査結果: {} 件", agents.len());
    for a in &agents {
        println!(
            "  session_id={:?} status={:?} kind={:?} pid={:?} cwd={:?}",
            a["session_id"], a["status"], a["kind"], a["pid"], a["cwd"]
        );
    }
    assert!(
        !agents.is_empty(),
        "稼働中の claude が観測できない（claude を対話で起動してから実行する）"
    );

    // `worker_status` が `status_source = agents` のときに呼ぶのと同じ関数
    let sid = agents[0]["session_id"]
        .as_str()
        .expect("session_id が引けない");
    let status = tako_control::orchestrator::query_agent_status(sid);
    println!(
        "query_agent_status({sid}) -> status={:?} ctx_percent={:?}",
        status.status, status.ctx_percent
    );
    assert_ne!(
        status.status, "unknown",
        "agents ソースの状態が取れない（走査は成功したが session_id が引けていない）"
    );
}
