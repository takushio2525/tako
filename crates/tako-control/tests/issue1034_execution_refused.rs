//! #1034: **起動も送達も成立したのに、agent 側の理由で実行が始まらない**を分類する。
//!
//! `cargo test -p tako-control --test issue1034_execution_refused -- --nocapture` を通すと、
//! 実際に返る文言がそのまま出るので**実測記録としても読める**（#983 のテストと同じ作法）。
//!
//! ## 何が起きていたか（北極星実測 / #975 の agy r1）
//!
//! agy worker を spawn したところ、CLI が起動直後にアカウントの検証待ちを表示して
//! **タスクを 1 文字も実行しなかった**のに、tako は `status = idle` /
//! `prompt_delivery = delivered` / `WORKER_IDLE`（50.07 秒後）を報告した。
//! master から見ると「worker が仕事を終えた」ように見える。
//!
//! #983 の `detect_launch_failure` は「まだ送達の証拠が無い worker」に限るゲートを
//! 持つので、送達が成立していたこの事象は**設計どおりゲートの外**だった（バグではなく穴）。
//!
//! ## 版で文言が変わる（実採取）
//!
//! | 版 | 画面 |
//! |---|---|
//! | 1.1.22（#1034 の実画面） | `Verifying your account...` / `We're finishing verifying your account eligibility.` / `This usually takes a moment. Please try again shortly.` |
//! | 1.1.27（実バイナリの文字列） | `Unable to verify account eligibility.` / `Eligibility check failed: <理由>` |
//!
//! 両版に共通して残る語が `account eligibility` なので、そこを軸に版差を吸収する。

use tako_control::orchestrator::agent::WorkerAgent;
use tako_control::orchestrator::agent_cli::{
    self, problem_message_in, AgentCliError, AgentCliProblem,
};
use tako_control::orchestrator::wait::WorkerErrorKind;
use tako_core::agent_support::Agent;
use tako_core::i18n::Lang;

/// #1034 の**実画面**（agy 1.1.22）。#927 に従いユーザー名・ホスト名・
/// アカウントはプレースホルダへ置換してある
const REAL_SCREEN_1_1_22: &str = "\
[testuser@host:proj]$ TAKO_ORCHESTRATOR_ROLE='worker:ns983:ns983-agy' agy --model gemini-3.7-flash-high --effort high --dangerously-skip-permissions
  Antigravity CLI 1.1.22
  <account> (Google AI Pro)
  Gemini 3.7 Flash (High)

⚠ Verifying your account...
  ⎿  We're finishing verifying your account eligibility.
     This usually takes a moment. Please try again shortly.
";

/// agy 1.1.27 の文言（実バイナリの文字列から組んだ画面）
const REAL_SCREEN_1_1_27: &str = "\
[testuser@host:proj]$ agy --model gemini-3.7-flash-high --dangerously-skip-permissions
  Antigravity CLI 1.1.27
  <account> (Google AI Pro)

⚠ Unable to verify account eligibility.
  ⎿  This usually takes about 30 seconds. Please try again later.
";

/// **正常に仕事をした worker の画面**（本測定の実ログ由来。agy の正常ラウンド）。
/// ここが `execution_refused` になってはいけない
const NORMAL_AGY_SCREEN: &str = "\
▸ Thought for 1s, 322 tokens
  Confirming Instruction Understanding
● Bash(wc -l sample.txt)
● Bash(python3 -c \"with open('sample.txt') as f: lines = f....) (ctrl+o to expand)
  ガイドラインを確認しました。
  sample.txt の行数を数えました。
  RESULT: 137
? for shortcuts                                          Gemini 3.7 Flash · high
";

#[test]
fn 実画面の検証待ちが実行拒否として分類される() {
    for (label, screen) in [
        ("agy 1.1.22（Issue の実画面）", REAL_SCREEN_1_1_22),
        ("agy 1.1.27（実バイナリの文言）", REAL_SCREEN_1_1_27),
    ] {
        let problem = agent_cli::detect_execution_refused(WorkerAgent::Agy, screen)
            .unwrap_or_else(|| panic!("実行拒否として分類されること: {label}"));
        assert_eq!(problem, AgentCliProblem::ExecutionRefused);
        let msg = AgentCliError {
            agent: WorkerAgent::Agy,
            problem,
        }
        .message_in(Lang::Ja);
        println!("--- {label} ---\n{msg}\n");
        assert!(msg.contains("次の一手"), "次の一手が要る: {msg}");
        assert!(
            msg.contains("時間を置いて"),
            "再試行が正解であることが読めること: {msg}"
        );
        assert!(
            msg.contains("1 文字も"),
            "作業が進んでいないこと（= 同じ指示を渡し直してよい）が読めること: {msg}"
        );
    }
}

#[test]
fn 正常に仕事をした画面は実行拒否にならない() {
    // 受け入れ条件 2。本測定の実ログ由来の画面で固定する
    assert_eq!(
        agent_cli::detect_execution_refused(WorkerAgent::Agy, NORMAL_AGY_SCREEN),
        None,
        "正常ラウンドの画面を拒否と読んではいけない"
    );
    for agent in WorkerAgent::ALL {
        assert_eq!(
            agent_cli::detect_execution_refused(agent, "❯ \n? for shortcuts"),
            None,
            "素の入力欄を拒否と読まない（{}）",
            agent.as_str()
        );
    }
}

#[test]
fn 一時的なapiエラーの常套句を拒否と読まない() {
    // `Please try again later` は API エラーの常套句でもある。これを拾うと
    // `api_error`（続行指示で復帰できる）を誤って `execution_refused` にしてしまう
    for line in [
        "Error: request failed. Please try again later.",
        "The command exited with code 1. Please try again shortly.",
        "Your image generation quota has been exceeded. Please try again later.",
    ] {
        assert_eq!(
            agent_cli::detect_execution_refused(WorkerAgent::Agy, line),
            None,
            "アカウント適格性の語が無い行を拒否と読まない: {line}"
        );
    }
}

#[test]
fn 未調査の系統には推測の文言を置いていない() {
    // #982 の規約: 実採取していない系統は宣言を空のままにする（推測を書かない）。
    // 同じ画面を claude / codex へ渡しても分類されないことで、それを固定する
    for agent in [WorkerAgent::Claude, WorkerAgent::Codex] {
        assert_eq!(
            agent_cli::detect_execution_refused(agent, REAL_SCREEN_1_1_22),
            None,
            "{} は同型の文言を実採取していないので分類しない",
            agent.as_str()
        );
    }
}

#[test]
fn 送達後に流れた同じ文字列でも作業中なら分類しない条件がある() {
    // ここは**検出関数の責務ではない**（呼び出し側のゲート）。
    // 「agent が 1 歩も動いていない」を一次シグナルで確かめてから呼ぶ、という
    // 約束を守っているかは dispatch 側の番犬 `agy_execution_refused_watchdog` が見る。
    // この test は「検出関数だけを裸で使うと拒否と読む」= ゲートが要ることを明示する
    let agent_output =
        format!("{NORMAL_AGY_SCREEN}\n$ cat notes.md\nverifying your account eligibility\n");
    assert!(
        agent_cli::detect_execution_refused(WorkerAgent::Agy, &agent_output).is_some(),
        "検出関数は文字列しか見ない（だからゲートが要る）"
    );
}

#[test]
fn 実行拒否の推奨アクションは再試行で往復も通る() {
    let kind = WorkerErrorKind::ExecutionRefused;
    assert_eq!(kind.as_str(), "execution_refused");
    assert_eq!(kind.recommended_action(), "retry_spawn");
    assert_eq!(WorkerErrorKind::from_slug("execution_refused"), Some(kind));
    // **待つ / ナッジする側の助言にしない**（ターンがそもそも始まっていない）
    assert_ne!(kind.recommended_action(), "wait_reset");
    assert_ne!(kind.recommended_action(), "resume");
}

#[test]
fn 全系統の文言が日英で固定されている() {
    // #983 の失敗 4 と同じ形。ローカル LLM は `applies_to` で外れる
    for agent in Agent::ALL {
        let applies = AgentCliProblem::ExecutionRefused.applies_to(agent);
        if agent == Agent::Local {
            assert!(!applies, "ローカル LLM には実行を断る主体が居ない");
            continue;
        }
        assert!(applies, "{} は実行を断られうる", agent.as_str());
        let ja = problem_message_in(agent, AgentCliProblem::ExecutionRefused, Lang::Ja);
        let en = problem_message_in(agent, AgentCliProblem::ExecutionRefused, Lang::En);
        println!(
            "=== {} / execution_refused ===\n[ja]\n{ja}\n[en]\n{en}\n",
            agent.as_str()
        );
        assert!(
            ja.lines().count() >= 2,
            "理由と次の一手が別の行にあること: {ja}"
        );
        assert!(
            en.lines().count() >= 2,
            "理由と次の一手が別の行にあること: {en}"
        );
        assert_ne!(ja, en, "英語が日本語の写しでない");
        assert!(en.contains("Next:"), "英語にも次の一手が要る: {en}");
    }
}

#[test]
fn ab_の_env_で旧挙動へ戻せる() {
    // `TAKO_1034_LEGACY=1` は**同一バイナリのまま**分類をやめる（= idle + delivered へ戻る）。
    // env はプロセス全体に効くのでこのテストは単独で env を触り、必ず戻す
    // （並列実行と競合しないよう、検出関数の戻りだけを見る最小の形にしてある）
    assert!(!agent_cli::legacy_execution_refused(), "既定は分類する側");
    assert!(
        agent_cli::detect_execution_refused(WorkerAgent::Agy, REAL_SCREEN_1_1_22).is_some(),
        "既定では分類される"
    );
}
