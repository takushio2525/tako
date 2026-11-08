//! Windows の claude が Git Bash を要るかの判断が実測から外れないことの番犬（#2433 の 2）。
//!
//! #2433 の調査時点で、setup は Claude Code の Windows ネイティブ版が Git Bash（Git for Windows）を要るかを
//! 確かめていなかった。2026-11-09 に Windows の CI のランナーで claude 2.1.319 を Git を隠した砂場で起こし、
//! **要らない**（無ければコマンドを PowerShell で走らせ、1 ターン最後まで走る）と確かめた
//! （記録は `.agent/requirements.md` の FR-2.37.10 と `platform::agent_install` の #2433 の節）。
//! それに合わせて setup は Git Bash を段の判定に入れず、導入計画の 1 行（`InstallRecipe::shell_note`）で知らせる。
//! 外れる道は 3 つある。3 つとも塞ぐ:
//!
//! 1. **見張りの抜け**: Windows の CI が本物の claude を起こす名指しの段を持たない・非ブロッキングに倒れる・
//!    全数より後ろへ回る・条件なしで毎回走る（外部の claude.ai から取る = #2791 の段と同じ条件で絞る）
//! 2. **検査と製品のずれ**: 検査が見る計画の 1 行の語が手順の `shell_note` から外れる・Git の無い組み合わせの
//!    期待が「PowerShell で走る」（= 要らない）でなくなる・Git の名前の変更や Path を戻さない形になる
//! 3. **宣言のずれ**: docs・FR が実測と違うこと（Git Bash が要る）を言う

use std::path::{Path, PathBuf};
use tako_core::platform::agent_install::{self, AgentKind};
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

const SCRIPT: &str = "scripts/windows/test-claude-git-bash-2433.ps1";
const FAKE_API: &str = "scripts/windows/lib/fake-anthropic-2433.ps1";

/// ci.yml の windows ジョブのステップ（`      - ` で始まる行ごと。#2790 / #2791 の番犬と同じ切り方）
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

/// 1. Windows の CI が本物の claude を、製品の口で入れて、全数より手前で、条件つきのブロッキングで起こす
#[test]
fn issue2433_windowsのciがclaudeのgit_bashの要否を名指しの段で見張る() {
    let steps = windows_steps();
    let run = steps
        .iter()
        .position(|s| s.contains(SCRIPT))
        .unwrap_or_else(|| panic!("ci.yml の windows ジョブに {SCRIPT} のステップが無い（#2433）"));
    let step = &steps[run];
    assert!(
        !step.contains("continue-on-error"),
        "Git Bash の要否の検査が非ブロッキングに倒されている（#2433）:\n{step}"
    );
    assert!(
        step.contains("if: needs.changes.outputs.agent_install == 'true'"),
        "Git Bash の要否の検査が条件なしで毎回走る（外部の claude.ai から取るので #2791 と同じ条件で絞る）:\n{step}"
    );
    assert!(
        step.contains("-Tako target\\debug\\tako.exe"),
        "Git Bash の要否の検査が製品の口（tako setup bootstrap）を通らない（#2433）:\n{step}"
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
        "Git Bash の要否の検査はビルド（tako.exe）の後・全数より手前に置く（#1282 / #2790 と同じ理由）"
    );
    // 変更範囲の判定: 検査そのものと偽の API を変えた PR でも走る
    let scope = read("scripts/ci-change-scope.sh");
    for path in [SCRIPT, FAKE_API] {
        assert!(
            scope.contains(&format!("'{path}|")),
            "scripts/ci-change-scope.sh の AGENT_INSTALL に {path} が無い（変えても実走が走らない）"
        );
    }
}

/// 2. 検査の期待が「要らない」を言い、計画の 1 行の語が手順の `shell_note` とそろい、借りた物を戻す
#[test]
fn issue2433_検査の期待と導入計画の1行が実測とそろう() {
    let script = read(SCRIPT);
    // 検査が status の install_plan.shell_note に求める語は、手順の shell_note に含まれる
    let needle = script
        .lines()
        .find_map(|l| l.trim().strip_prefix("$ShellNoteNeedle = '"))
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_else(|| panic!("{SCRIPT} に $ShellNoteNeedle が無い"));
    let note = agent_install::recipe(Platform::Windows, AgentKind::Claude)
        .shell_note
        .expect("Windows の claude の手順に shell_note が無い（#2433）");
    assert!(
        note.contains(needle),
        "{SCRIPT} の $ShellNoteNeedle「{needle}」が手順の shell_note「{note}」に無い（検査と製品がずれた）"
    );
    // Git の無い組み合わせは PowerShell で走る = Git Bash は要らない（実測の結論）
    for (case, shell) in [
        ("'no-git'", "'powershell'"),
        ("'bash-path-missing'", "'powershell'"),
        ("'bash-path-env'", "'bash'"),
        ("'git-nonstandard-on-path'", "'bash'"),
        ("'git-on-path'", "'bash'"),
        ("'git-off-path'", "'bash'"),
        ("'bash-path-missing-git'", "'bash'"),
    ] {
        assert!(
            script.lines().any(|l| {
                let t = l.trim();
                t.starts_with(case) && t.ends_with(&format!("= {shell}"))
            }),
            "{SCRIPT} の $Expect で {case} が {shell} でない（実測は #2433 / FR-2.37.10）"
        );
    }
    for (needle, why) in [
        (
            "@('setup', 'bootstrap', 'install')",
            "製品の口（MCP tako_setup_bootstrap と同じ install_for）で入れる",
        ),
        (
            "Fail \"導入後の案内に「$ShellNoteNeedle」が無い\"",
            "導入後の CLI の案内に計画の 1 行が出る",
        ),
        (
            "@('setup', 'bootstrap', 'status', '--json')",
            "各組み合わせで tako の status を読む",
        ),
        ("Hide-Git\n    Install-Claude", "Git を隠してから入れる"),
        ("try { Show-Git }", "Git の名前を finally で戻す"),
        ("Restore-TakoUserPath -Snapshot $Before", "Path を戻す"),
        (
            "Git の有無で tako の段が変わった",
            "Git Bash を段の判定に入れない",
        ),
        ("'sk-ant-tako2433-dummy'", "本物の認証情報を使わない"),
        ("lib/fake-anthropic-2433.ps1", "偽の Messages API へ向ける"),
    ] {
        assert!(
            script.contains(needle),
            "{SCRIPT} に `{needle}` が無い（{why}。#2433）"
        );
    }
}

/// 3. docs と FR が実測（Git Bash は要らない・無ければ PowerShell）を言う
#[test]
fn issue2433_docsとfrが実測と一致する() {
    let claude = read("docs/src/content/docs/agents/claude.md");
    for needle in [
        "**Git for Windows は要りません**",
        "PowerShell",
        "CLAUDE_CODE_GIT_BASH_PATH",
    ] {
        assert!(
            claude.contains(needle),
            "docs の claude のページに「{needle}」が無い（#2433）"
        );
    }
    let fr = read(".agent/requirements.md");
    let row = fr
        .lines()
        .find(|l| l.starts_with("| FR-2.37.10 |"))
        .expect("FR-2.37.10（#2433 の実測）が無い");
    for needle in ["Git Bash（Git for Windows）を要らない", "2.1.319", SCRIPT] {
        assert!(row.contains(needle), "FR-2.37.10 に「{needle}」が無い");
    }
    // 要ると言う書き方が戻っていない
    const STALE: &[&str] = &[
        "Git Bash が必須",
        "Git Bash が要ります",
        "Git for Windows が必須",
        "Git for Windows が要ります",
    ];
    let mut offenders = Vec::new();
    for rel in [
        "docs/src/content/docs/agents/claude.md",
        "docs/src/content/docs/getting-started/index.mdx",
        ".agent/requirements.md",
        "crates/tako-core/src/platform/agent_install.rs",
    ] {
        for (i, line) in read(rel).lines().enumerate() {
            for stale in STALE {
                if line.contains(stale) {
                    offenders.push(format!("{rel}:{} 「{stale}」", i + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Windows の claude は Git Bash を要らない（#2433 の実測）のに、要ると言う文がある:\n{}",
        offenders.join("\n")
    );
}
