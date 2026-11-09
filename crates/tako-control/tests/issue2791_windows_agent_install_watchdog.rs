//! Windows の codex / agy の導入代行が「案内止まり」へ戻らないことの番犬（#2791）。
//!
//! #989 で Windows の codex / agy は `tako_can_run: false`（状態照会と公式手順の案内まで）にし、根拠の文は
//! 「実行代行は #525 の範囲」とした。#525 に専用の作業は無く、claude 以外を選んだ Windows の利用者は
//! 手で導入することになっていた。#2791 で両方の `install.ps1` を読み、ユーザー権限だけで入ることを確かめて
//! 代行へ倒した。戻る道は 4 つある。4 つとも塞ぐ（5 は実走の段の走らせ方）:
//!
//! 1. **手順の後退**: Windows のレシピの `tako_can_run` が false へ戻る・#2790 の起こし方
//!    （本文をテキストで読む `-EncodedCommand`）から外れる・起動片が子スコープ（`&`）へ戻る
//!    （agy の `install.ps1` の取得物の片付けが飛ぶ）
//! 2. **宣言のずれ**: 対応表・MCP の説明・docs・`.agent/` に「Windows で代行するのは claude だけ」
//!    「実行代行は #525 の範囲」が残る（宣言が実態より小さい = 過小申告）
//! 3. **実走の検査の抜け**: Windows の CI が本物の `install.ps1` を走らせる名指しのステップを持たない・
//!    非ブロッキングに倒れる・全数より後ろへ回る・ポリシー 3 通り / A/B / 境界のどれかが落ちる
//! 4. **偽物の検査の抜け**: 起こし方のスコープを実物の PowerShell で見る Windows のテストが無い
//! 5. **実走の段が毎回走る形**: 実走は外部（chatgpt.com / antigravity.google）から本物を取るので、条件なしで
//!    毎回走らせると外部の障害・遅延で関係の無い PR まで赤になる。導入代行に関わるパスを変えたとき
//!    （`changes` の `agent_install` = `scripts/ci-change-scope.sh` の `AGENT_INSTALL`）と定期・手動実行でだけ
//!    走らせる。偽物で見る #2790 の段は外部に依らないので毎回走らせる

use std::path::{Path, PathBuf};
use tako_core::platform::agent_install::{self, AgentKind, ScriptPassing};
use tako_core::platform::support::Platform;

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

const SCRIPT: &str = "scripts/windows/test-agent-install-2791.ps1";
const WINDOWS_TEST: &str = "crates/tako-core/tests/issue2791_agent_install_windows.rs";

/// 1. Windows の 3 系統は代行し、#2790 の 1 実装で起こし、起動片はドットソース
#[test]
fn issue2791_windowsの3系統は代行し同じ1実装で起こす() {
    let script = Path::new(r"C:\Users\winuser\AppData\Local\Temp\tako-agy-install-1.ps1");
    for agent in AgentKind::ALL {
        let r = agent_install::recipe(Platform::Windows, agent);
        let who = agent.as_str();
        assert!(
            r.tako_can_run,
            "Windows の {who} が案内止まり（tako_can_run: false）へ戻っている（#2791）"
        );
        assert_eq!(
            r.runner.passing,
            ScriptPassing::PowerShellText,
            "Windows の {who} が #2790 の起こし方から外れている"
        );
        // 符号化して載せる本文は起動片の正本そのもの
        let args = r.runner.args_for_in(script, false);
        let body = tako_core::platform::shell::powershell_script_text_command(script);
        assert_eq!(
            args.last().map(|a| a.to_string_lossy().into_owned()),
            Some(tako_core::platform::shell::encode_powershell_command(&body)),
            "Windows の {who} の起動引数が起動片の正本を載せていない"
        );
        // #2822 から本文は関数 `install.ps1` として定義し、その名前をドットソースする（スコープは同じ）
        assert!(
            body.contains("\n. install.ps1\n")
                && !body.contains("& install.ps1")
                && !body.contains("& ([scriptblock]"),
            "起動片がドットソースでない（子スコープでは agy の取得物の片付けが飛ぶ。#2791 / #2822）: {body}"
        );
    }
    // A/B の旧アーム（TAKO_2791_LEGACY=1）だけが Windows の codex / agy を案内止まりへ戻す
    for agent in [AgentKind::Codex, AgentKind::Agy] {
        assert!(!agent_install::recipe_in(Platform::Windows, agent, true).tako_can_run);
    }
    assert!(agent_install::recipe_in(Platform::Windows, AgentKind::Claude, true).tako_can_run);
}

/// 2. 「Windows で代行するのは claude だけ」「実行代行は #525 の範囲」が宣言・説明・docs に残っていない
#[test]
fn issue2791_windowsの代行がclaudeだけという古い宣言が残っていない() {
    const STALE: &[&str] = &[
        "実行代行は #525 の範囲",
        "running it is tracked in #525",
        "Windows で代行できるのは claude だけ",
        "Windows で tako が導入を代行できるのは claude だけ",
        "Windows の実行代行は claude だけ",
        "Windows で代わりに導入まで行うのは claude だけ",
        "Windows で tako が実際にインストーラを走らせるのは `claude` だけ",
        "Windows で実行を代行するのは claude だけ",
        "tako から実行を代行できるのは 3 系統のうち claude だけ",
    ];
    let files = [
        "crates/tako-core/src/platform/agent_install.rs",
        "crates/tako-core/src/platform/support.rs",
        "crates/tako-core/src/agent_support.rs",
        "crates/tako-control/src/mcp/catalog.rs",
        "docs/src/content/docs/agents/claude.md",
        "docs/src/content/docs/agents/codex.md",
        "docs/src/content/docs/agents/agy.md",
        "docs/src/content/docs/getting-started/index.mdx",
        "docs/src/content/docs/guides/cli-reference.md",
        "docs/src/data/windows-support.json",
        "docs/src/data/agent-support.json",
        ".agent/requirements.md",
        ".agent/commands.md",
    ];
    let mut offenders = Vec::new();
    for rel in files {
        let text = read(rel);
        for (i, line) in text.lines().enumerate() {
            for stale in STALE {
                if line.contains(stale) {
                    offenders.push(format!("{rel}:{} 「{stale}」", i + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Windows の codex / agy も導入を代行する（#2791）のに、案内止まりの宣言が残っている:\n{}",
        offenders.join("\n")
    );
    // 対応表の理由は実態（3 系統とも代行・codex / agy は CI のランナーで実走・実機は未実測）を言う
    let note = tako_core::platform::support::notes::WIN_SETUP_BOOTSTRAP_AGENTS;
    let ja = note.text_in(tako_core::i18n::Lang::Ja);
    for needle in ["3 系統とも", "#2791", "実機では未実測"] {
        assert!(
            ja.contains(needle),
            "WIN_SETUP_BOOTSTRAP_AGENTS に「{needle}」が無い: {ja}"
        );
    }
}

/// ci.yml の windows ジョブのステップ（`      - ` で始まる行ごと。#2790 の番犬と同じ切り方）
fn windows_steps() -> Vec<String> {
    let ci = read(".github/workflows/ci.yml");
    let (_, rest) = ci
        .split_once("\n  windows:")
        .expect("CI に windows ジョブが無い");
    let job: Vec<&str> = rest
        .lines()
        .skip(1)
        .take_while(|line| line.trim().is_empty() || line.starts_with("    "))
        .collect();
    let mut steps: Vec<String> = Vec::new();
    for line in job {
        if line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with("      - ") || steps.is_empty() {
            steps.push(String::new());
        }
        let step = steps.last_mut().expect("直前に 1 本ある");
        step.push_str(line);
        step.push('\n');
    }
    steps
}

/// 3. Windows の CI が本物の install.ps1 の実走を、全数より手前に名指しで、ブロッキングで回す
#[test]
fn issue2791_windowsのciが本物のインストーラの実走を名指しで回す() {
    let steps = windows_steps();
    let run = steps
        .iter()
        .position(|s| s.contains(SCRIPT))
        .unwrap_or_else(|| panic!("ci.yml の windows ジョブに {SCRIPT} のステップが無い（#2791）"));
    assert!(
        !steps[run].contains("continue-on-error"),
        "実走の検査が非ブロッキングに倒されている（#2791）:\n{}",
        steps[run]
    );
    let build = steps
        .iter()
        .position(|s| s.contains("cargo build --workspace"))
        .expect("windows ジョブにビルドのステップが無い");
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    assert!(
        build < run && run < full,
        "実走の検査はビルド（tako.exe）の後・全数より手前に置く（#1282 / #2790 と同じ理由）"
    );

    let script = read(SCRIPT);
    for (needle, why) in [
        (
            ". (Join-Path $PSScriptRoot 'lib/execution-policy.ps1')",
            "ポリシーの置き方は #2790 と同じ 1 実装",
        ),
        (
            "installer/windows/lib/user-path.ps1",
            "ユーザー環境変数 Path の読み戻しは #2767 と同じ 1 実装",
        ),
        (
            "Hive = 'HKCU'; Scope = 'UserPolicy'; Value = 'Restricted'",
            "HKCU Restricted の回",
        ),
        (
            "Hive = 'HKLM'; Scope = 'MachinePolicy'; Value = 'AllSigned'",
            "HKLM AllSigned の回",
        ),
        (
            "@('setup', 'bootstrap', 'install', '--agent', $Agent, '--json')",
            "製品の口（MCP tako_setup_bootstrap と同じ install_for）で入れる",
        ),
        ("TAKO_2791_LEGACY", "A/B の旧アームは断る"),
        ("HTTPS_PROXY", "境界: 導入元に届かない"),
        ("CODEX_RELEASE = '0.0.1'", "境界: 途中で失敗する（codex）"),
        ("antigravity\\staging\\agy.exe", "agy の取得物が残らない"),
        ("where.exe", "境界: npm が無い"),
        ("Restore-TakoUserPath -Snapshot $Before", "Path を戻す"),
        ("--test $TestName --no-run", "ポリシーを置く前に組む"),
        (
            "$TestName = 'issue2791_agent_install_windows'",
            "偽物の検査も同じポリシーの下で回す",
        ),
    ] {
        assert!(
            script.contains(needle),
            "{SCRIPT} に `{needle}` が無い（{why}。#2791）"
        );
    }
    // 置いたポリシーは finally の最初の文で戻す
    let finally = script
        .split_once("} finally {\n        # 置いたポリシーはここで必ず戻す")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("{SCRIPT}: ポリシーを finally で戻していない（#2791）"));
    assert!(
        finally
            .lines()
            .skip(1)
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.contains("if ($case.Hive)")),
        "{SCRIPT}: finally の最初の文がポリシーの戻しでない（#2791）"
    );
    let no_run = script
        .find("--test $TestName --no-run")
        .expect("テストを置く前に組んでいない");
    let set = script
        .find("            Set-Policy ")
        .expect("Set-Policy の呼び出し");
    assert!(
        no_run < set,
        "{SCRIPT}: ポリシーを置いてから cargo で組んでいる（#2791）"
    );
}

/// 4. 起こし方のスコープを実物の PowerShell で見る Windows のテストがある
#[test]
fn issue2791_起こし方のスコープを実物のpowershellで見るテストがある() {
    let test = read(WINDOWS_TEST);
    for needle in [
        "#![cfg(windows)]",
        ".installer_command(script)",
        "powershell_script_text_command_in(script, true)",
        "$script:stagingPayload",
        "$script:guardRejected",
        "TAKO_2791_POLICY",
    ] {
        assert!(
            test.contains(needle),
            "{WINDOWS_TEST} に `{needle}` が無い（#2791）"
        );
    }
}

/// 5. 実走の段は導入代行に関わるパスを変えたときと定期・手動実行でだけ走る（外部に依るので毎回は走らせない）。
///    偽物で見る #2790 の段は毎回走る
#[test]
fn issue2791_実走の段は条件つきで偽物の2790の段は毎回走る() {
    let steps = windows_steps();
    let run = steps
        .iter()
        .find(|s| s.contains(SCRIPT))
        .unwrap_or_else(|| panic!("ci.yml の windows ジョブに {SCRIPT} のステップが無い（#2791）"));
    assert!(
        run.lines()
            .any(|l| l.trim() == "if: needs.changes.outputs.agent_install == 'true'"),
        "実走の段が条件なしで毎回走る形に戻っている（外部の障害で関係の無い PR まで赤になる。#2791）。\
         `if: needs.changes.outputs.agent_install == 'true'` を付ける:\n{run}"
    );
    let fake = steps
        .iter()
        .find(|s| s.contains("scripts/windows/test-install-execution-policy-2790.ps1"))
        .expect("ci.yml の windows ジョブに #2790 の段が無い");
    assert!(
        !fake.lines().any(|l| l.trim_start().starts_with("if:")),
        "偽物で見る #2790 の段が判定で止まる（外部に依らないので毎回走らせる。#2791）:\n{fake}"
    );

    let ci = read(".github/workflows/ci.yml");
    let changes = ci
        .split_once("\n  changes:")
        .map(|(_, rest)| rest.split_once("\n  docs:").map(|(b, _)| b).unwrap_or(rest))
        .expect("ci.yml に changes ジョブが無い");
    assert!(
        changes.contains("agent_install: ${{ steps.scope.outputs.agent_install }}"),
        "changes ジョブが agent_install を出していない（#2791）"
    );
    let on = ci
        .split_once("\non:\n")
        .map(|(_, rest)| rest.split_once("\n\n").map(|(b, _)| b).unwrap_or(rest))
        .expect("ci.yml に on: が無い");
    for (needle, why) in [
        (
            "  schedule:",
            "上流の install.ps1 の形の変化を定期実行で拾う",
        ),
        ("  workflow_dispatch:", "手動でも実走を回せる"),
        ("  pull_request:", "PR の CI"),
    ] {
        assert!(
            on.lines().any(|l| l.starts_with(needle)),
            "ci.yml の on: に `{}` が無い（{why}。#2791）:\n{on}",
            needle.trim()
        );
    }

    // 判定の型: 依頼で名指しされたパスを変えたら実走を走らせる（正本は ci-change-scope.sh の AGENT_INSTALL）
    let scope = read("scripts/ci-change-scope.sh");
    let table = scope
        .split_once("AGENT_INSTALL=(")
        .and_then(|(_, rest)| rest.split_once("\n)"))
        .map(|(t, _)| t)
        .expect("ci-change-scope.sh に AGENT_INSTALL の表が無い");
    for path in [
        "crates/tako-core/src/platform/agent_install.rs",
        "crates/tako-core/src/platform/shell.rs",
        "crates/tako-control/src/setup_bootstrap.rs",
        SCRIPT,
        "scripts/windows/lib/execution-policy.ps1",
        ".github/workflows/ci.yml",
        WINDOWS_TEST,
    ] {
        assert!(
            table.contains(&format!("'{path}|")),
            "ci-change-scope.sh の AGENT_INSTALL に {path} が無い（変えても実走が走らない。#2791）"
        );
    }
    // 定期・手動実行では常に走らせる
    assert!(
        scope.contains("schedule | workflow_dispatch)")
            && scope.contains("echo \"agent_install=${AGENT}\""),
        "ci-change-scope.sh が定期・手動実行で agent_install を出していない（#2791）"
    );
}
