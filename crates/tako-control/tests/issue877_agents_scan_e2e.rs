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

/// `agents-auto`（`worker_status` が session_id 省略時に通る pane→session 自動解決）の経路。
///
/// 器（tmux / psmux）のセッション名を `TAKO_877_BACKEND_SESSION` で渡す。
/// そのセッションの中で claude が動いていることが前提:
///
/// ```text
/// TAKO_877_BACKEND_SESSION=tako-s1 cargo test -p tako-control \
///   --test issue877_agents_scan_e2e -- --ignored --nocapture
/// ```
///
/// ここは #877 の走査に加えて**器へのペイン問い合わせ**（`tmux_pane_pids`）にも依存するので、
/// 走査が直っても器の側で名前や引数が食い違うと `None` になる。落ちたときにどちらが原因かが
/// 分かるよう、ペイン一覧をそのまま出す
#[test]
#[ignore = "稼働中の claude を含む器のセッションが要る（実機 e2e）"]
fn バックエンドセッションからsession_idを自動解決できる() {
    let backend = std::env::var("TAKO_877_BACKEND_SESSION").unwrap_or_default();
    assert!(
        !backend.is_empty(),
        "TAKO_877_BACKEND_SESSION に、claude を動かしている器のセッション名を渡すこと"
    );
    let socket = tako_core::tmux_backend::socket_name();
    let panes = tako_control::agents::tmux_pane_pids(Some(&socket));
    println!("器のペイン一覧（socket={socket}）: {panes:?}");
    assert!(
        !panes.is_empty(),
        "器がペインを 1 つも返さない（走査ではなく器への問い合わせ側の問題）"
    );

    let sid = tako_control::agents::resolve_session_id_for_backend(&backend);
    println!("resolve_session_id_for_backend({backend}) -> {sid:?}");
    assert!(
        sid.is_some(),
        "session_id を自動解決できない（= status_source が agents-auto にならない）"
    );
}
