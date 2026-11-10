//! **#2879 の番犬**: 初回起動で setup（#1510 の自動の段）を走らせる形へ戻らない。
//!
//! 11/10 の利用者の指示: 初回起動で何も自動で走らせず、「セットアップを実行」→ setup のタブで `tako setup`
//! → 対話のセットアップ → master、の流れにする（master / worker の設定や GitHub との連携は対話でしか
//! 決められないのに、#1510 の自動の段は対話を通らずに「master を起動」へ進んでいた）。
//!
//! 見るもの:
//!
//! 1. 判断（`tako_control::welcome`）: ボタン起点の形（`Flow::Guided` = 製品の既定）では、起動時の判断
//!    `startup_action` が自動の段を返さず、段の遷移 `Onboarding::apply_step` が自動の段を頼まず、
//!    ボタンの中身 `launch_for_flow` も自動の段を返さない（どの段・どの方言でも）
//! 2. 配線（`crates/tako-app/src/main.rs`）: 起動時の入口 `start_welcome_onboarding` が上の判断を通り、
//!    自動の段（`start_welcome_auto`）を判断の `Startup::Auto` の腕の外で呼ばない。自動の段の本体
//!    （`run_auto_setup`）を `start_welcome_auto` の外から呼ばない
//!
//! 外れたら `file:line` で名指す（直す場所が分からない番犬は直されない）。A/B（`TAKO_2879_LEGACY=1`）は
//! #1510 の形（`Flow::Auto`）で、そちらは従来どおり一度だけ自動の段を走らせる

use std::path::{Path, PathBuf};
use std::time::Instant;

use tako_control::setup::RunningPhase;
use tako_control::welcome::{self, Flow, Launch, NextStep, Onboarding, SetupProgress, Startup};
use tako_core::platform::agent_install::AgentKind;
use tako_core::platform::shell_dialect::ShellDialect;

const MAIN: &str = "crates/tako-app/src/main.rs";
const WELCOME: &str = "crates/tako-control/src/welcome.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// `needle` が最初に出る行を `path:line` で返す
fn locate(rel: &str, src: &str, needle: &str) -> String {
    match src.lines().position(|line| line.contains(needle)) {
        Some(i) => format!("{rel}:{}", i + 1),
        None => format!("{rel}（{needle} が見つからない）"),
    }
}

/// メソッド `fn <name>(` の本体（見出しの行から、同じ字下げの `}` まで）を (行番号, 行) で返す
fn method_body<'a>(src: &'a str, name: &str) -> Option<Vec<(usize, &'a str)>> {
    let lines: Vec<&str> = src.lines().collect();
    let head = format!("fn {name}(");
    let start = lines.iter().position(|line| {
        line.trim_start().starts_with(&head) || line.contains(&format!(" {head}"))
    })?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|line| *line == close)
        .map(|offset| start + offset)?;
    Some((start..=end).map(|i| (i + 1, lines[i])).collect())
}

/// 全段（方言ごとの中身を見るのに使う代表）
fn all_steps() -> Vec<NextStep> {
    let plan = tako_control::setup_bootstrap::plan_from(
        &tako_control::setup_bootstrap::recipe_for(AgentKind::Claude),
        Path::new("/home/testuser"),
        Path::new("/home/testuser/.codex"),
    );
    vec![
        NextStep::Setup,
        NextStep::Continue {
            agent: AgentKind::Claude,
        },
        NextStep::Running {
            phase: RunningPhase::Setup,
        },
        NextStep::Running {
            phase: RunningPhase::Dialogue,
        },
        NextStep::Install {
            agent: AgentKind::Claude,
            plan,
        },
        NextStep::Path {
            agent: AgentKind::Claude,
        },
        NextStep::PathByHand {
            agent: AgentKind::Claude,
            shell: "/bin/tcsh".into(),
            dirs: vec!["$HOME/.local/bin".into()],
        },
        NextStep::Login {
            agent: AgentKind::Claude,
            binary: None,
        },
        NextStep::Master {
            agent: AgentKind::Claude,
        },
        NextStep::Unknown { reason: "x".into() },
    ]
}

#[test]
fn ボタン起点の形では起動時にsetupを走らせない() {
    let welcome_src = read(WELCOME);
    let at = || locate(WELCOME, &welcome_src, "pub fn startup_action(");
    let mut failures = Vec::new();

    // ---- 1. 判断 ----
    for completed in [false, true] {
        if welcome::startup_action(Flow::Guided, completed) != Startup::Probe {
            failures.push(format!(
                "起動時の判断が自動の段を返した（setup 済み = {completed}）: {}",
                at()
            ));
        }
    }
    // 走っていない・一度も走っていない段の確かめは、導入状況（ログインシェル）すら見ない
    let probed = std::cell::Cell::new(false);
    let first = welcome::next_step_guided(&SetupProgress::default(), || {
        probed.set(true);
        Vec::new()
    });
    if first != NextStep::Setup || probed.get() {
        failures.push(format!(
            "初回起動の最初の段が「セットアップを実行」でない、またはログインシェルを起こした（{first:?}）: {}",
            locate(WELCOME, &welcome_src, "pub fn next_step_guided(")
        ));
    }
    let now = Instant::now();
    for step in all_steps() {
        let mut onboarding = Onboarding::new(Flow::Guided);
        onboarding.start_watch(now);
        if onboarding.apply_step(step.clone(), now).run_auto {
            failures.push(format!(
                "段 {} を受けて自動の段を頼んだ: {}",
                step.kind(),
                locate(WELCOME, &welcome_src, "fn apply_step_guided(")
            ));
        }
        for dialect in [ShellDialect::Posix, ShellDialect::PowerShell] {
            if welcome::launch_for_flow(&step, dialect, Flow::Guided) == Launch::Auto {
                failures.push(format!(
                    "段 {}（{dialect:?}）のボタンが自動の段: {}",
                    step.kind(),
                    locate(WELCOME, &welcome_src, "pub fn launch_for_flow(")
                ));
            }
        }
    }
    // 製品の既定はボタン起点（A/B だけが #1510 の形）
    if welcome::flow_in(false, false, false, false) != Flow::Guided {
        failures.push(format!(
            "製品の既定の形がボタン起点でない: {}",
            locate(WELCOME, &welcome_src, "pub fn flow_in(")
        ));
    }

    // ---- 2. 配線 ----
    let main_src = read(MAIN);
    match method_body(&main_src, "start_welcome_onboarding") {
        None => failures.push(format!(
            "{MAIN} に start_welcome_onboarding が無い（入口を探せない）"
        )),
        Some(body) => {
            if !body
                .iter()
                .any(|(_, line)| line.contains("welcome::startup_action("))
            {
                failures.push(format!(
                    "起動時の入口が welcome::startup_action を通らない: {MAIN}:{}",
                    body[0].0
                ));
            }
            for (no, line) in &body {
                if line.contains("start_welcome_auto(") && !line.contains("Startup::Auto") {
                    failures.push(format!(
                        "起動時の入口が判断の Startup::Auto の腕の外で自動の段を呼ぶ: {MAIN}:{no}"
                    ));
                }
                if line.contains("run_auto_setup(") {
                    failures.push(format!(
                        "起動時の入口が自動の段の本体を直に呼ぶ: {MAIN}:{no}"
                    ));
                }
            }
        }
    }
    let owner = method_body(&main_src, "start_welcome_auto")
        .map(|body| (body[0].0, body[body.len() - 1].0));
    for (i, line) in main_src.lines().enumerate() {
        let no = i + 1;
        if line.contains("run_auto_setup(")
            && !owner.is_some_and(|(from, to)| (from..=to).contains(&no))
        {
            failures.push(format!(
                "自動の段の本体（run_auto_setup）を start_welcome_auto の外から呼ぶ: {MAIN}:{no}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "#2879: 初回起動で setup を走らせる形へ戻った（11/10 の利用者の指示に反する）:\n  {}",
        failures.join("\n  ")
    );
}

/// A/B（#1510 の形）は従来どおり: 一度も最後まで走っていなければ起動時に自動の段（逆向きの確かめ。
/// 判断が常に Probe を返すだけの壊れ方 = 番犬が何も見ていない、を見分ける）
#[test]
fn ab_の旧挙動は一度だけ自動の段を走らせる() {
    assert_eq!(welcome::startup_action(Flow::Auto, false), Startup::Auto);
    assert_eq!(welcome::startup_action(Flow::Auto, true), Startup::Probe);
    assert_eq!(welcome::flow_in(false, true, false, false), Flow::Auto);
    let mut onboarding = Onboarding::new(Flow::Auto);
    assert!(
        onboarding
            .apply_step(
                NextStep::Path {
                    agent: AgentKind::Claude
                },
                Instant::now()
            )
            .run_auto,
        "#1510 の形は PATH の段で 1 回だけ自動の段を当て直す"
    );
}
