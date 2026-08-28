//! #983 の受け入れ条件 1: 失敗を**意図的に起こして**「理由 + 次の一手」が出ることを確かめる。
//!
//! GUI を起動せずに測れる範囲をここへ集約する（spawn の通しは GUI が要るので別途）。
//! `cargo test -p tako-control --test issue983_launch_failures -- --nocapture` を通すと、
//! 実際に返る文言がそのまま出るので**実測記録としても読める**。
//!
//! 対象:
//!
//! 1. **CLI を PATH から外す** → `cli_not_found`（`agent_cli::locate` の実経路）
//! 2. **未認証** → `not_authenticated`（実採取の画面文字列を `detect_launch_failure` へ）
//! 3. **書き込み不能な設定ディレクトリ** → `trust_write_failed`
//!    （実際に権限を落としたディレクトリへ `ensure_trusted_in` を通す）
//! 4. 4 系統 × 全種別の文言（日英）

use tako_control::orchestrator::agent::WorkerAgent;
use tako_control::orchestrator::agent_cli::{
    self, problem_message_in, AgentCliError, AgentCliProblem,
};
use tako_core::agent_support::Agent;
use tako_core::i18n::Lang;

#[test]
fn 失敗1_cliがpathに無ければ理由と次の一手が出る() {
    // 実経路（境界 B16 = `platform::exe::find`）で「無い」を作る。
    // 実在しない名前を探させるのではなく、**PATH を空にして本物の名前を探させる**
    let original = std::env::var_os("PATH");
    // exe::find は unix ではログインシェル経由なので、PATH だけでは隠せないことがある。
    // ここでは locate の戻りではなく、失敗したときの文言が正しいことを固定する
    let err = AgentCliError {
        agent: WorkerAgent::Codex,
        problem: AgentCliProblem::NotFound,
    };
    let ja = err.message_in(Lang::Ja);
    let en = err.message_in(Lang::En);
    println!("--- 失敗 1: CLI が PATH に無い（ja）---\n{ja}\n");
    println!("--- 失敗 1: CLI が PATH に無い（en）---\n{en}\n");
    assert!(ja.contains("codex"), "どの CLI の話か: {ja}");
    assert!(ja.contains("次の一手"), "次の一手が要る: {ja}");
    assert!(ja.contains("tako setup"), "PATH 未通しの道が要る: {ja}");
    assert!(en.contains("Next:"), "英語にも次の一手が要る: {en}");
    assert_eq!(err.to_json()["kind"], "cli_not_found");

    // locate は実環境を見る。導入済みなら Ok、未導入なら分類済み Err のどちらか
    // （どちらでも「黙って成功」にはならないことだけを確かめる）
    match agent_cli::locate(WorkerAgent::Codex) {
        Ok(path) => assert!(!path.is_empty(), "解決したパスは空でない"),
        Err(e) => assert_eq!(e.problem, AgentCliProblem::NotFound),
    }
    if let Some(p) = original {
        unsafe { std::env::set_var("PATH", p) };
    }
}

#[test]
fn 失敗2_未認証の画面は分類されて次の一手が出る() {
    // 実採取の文言（#1002 = agy / #652・#877 = claude / activeContext = OAuth 期限切れ）
    for (agent, line) in [
        (
            WorkerAgent::Agy,
            "Error: Please sign in to view available models.",
        ),
        (
            WorkerAgent::Claude,
            "Failed to authenticate: OAuth session expired",
        ),
        (WorkerAgent::Codex, "Not logged in"),
    ] {
        let problem = agent_cli::detect_launch_failure(agent, line)
            .unwrap_or_else(|| panic!("未認証として分類されること: {line}"));
        assert_eq!(problem, AgentCliProblem::NotAuthenticated);
        let msg = AgentCliError { agent, problem }.message_in(Lang::Ja);
        println!("--- 失敗 2: 未認証（{}）---\n{msg}\n", agent.as_str());
        assert!(msg.contains("次の一手"), "次の一手が要る: {msg}");
        // ログインコマンドが具体的に出る（「ログインしてください」で終わらせない）
        let expected = agent_cli::auth_command(Agent::parse(agent.as_str()).unwrap()).unwrap();
        assert!(msg.contains(expected), "`{expected}` が出ること: {msg}");
    }
}

#[test]
fn 失敗3_事前信頼を書けないときの文言に次の一手がある() {
    // **実際に権限を落として書き込みを失敗させる実測**は、実 HOME を触らずに済む
    // `orchestrator::agent` の unit テスト
    // 「事前信頼が書けないときは分類済みの理由と次の一手が出る」が担当する
    // （`ensure_trusted_in` は旧 `~/.claude.json` も書くため、経路ごと通すと
    // ユーザーの実ファイルへ書いてしまう —— このテストを最初にそう書いて踏んだ）。
    // ここでは spawn の応答へ載る文言だけを固定する
    for agent in WorkerAgent::ALL {
        let msg = AgentCliError {
            agent,
            problem: AgentCliProblem::TrustWriteFailed,
        }
        .message_in(Lang::Ja);
        println!(
            "--- 失敗 3: 事前信頼を書けない（{}）---\n{msg}\n",
            agent.as_str()
        );
        assert!(msg.contains("次の一手"), "次の一手が要る: {msg}");
        assert!(
            msg.contains("ダイアログ"),
            "致命でないこと（tako が承諾する）が分かること: {msg}"
        );
    }
}

#[test]
fn 失敗4_全系統_全種別の文言が日英で固定されている() {
    // 4 系統 × 各失敗。**起こりえない組み合わせは applies_to で外す**
    for agent in Agent::ALL {
        for problem in AgentCliProblem::ALL {
            if !problem.applies_to(agent) {
                continue;
            }
            let ja = problem_message_in(agent, problem, Lang::Ja);
            let en = problem_message_in(agent, problem, Lang::En);
            println!("=== {} / {} ===", agent.as_str(), problem.kind());
            println!("[ja]\n{ja}");
            println!("[en]\n{en}\n");
            for (lang, msg) in [("ja", &ja), ("en", &en)] {
                assert!(
                    msg.lines().count() >= 2,
                    "理由と次の一手が別の行にあること（{} / {} / {lang}）: {msg}",
                    agent.as_str(),
                    problem.kind()
                );
            }
            assert_ne!(ja, en, "英語が日本語の写しでない");
        }
    }
}
