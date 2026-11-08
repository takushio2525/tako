//! 導入代行の起こし方が実行ポリシーに頼る形へ戻らないことの番犬（#2790）。
//!
//! エージェント CLI の導入代行は取得した `install.ps1` を #2790 より前は `-ExecutionPolicy Bypass -File`
//! で起こしていた。`Bypass` は Process スコープの指定なので、グループポリシーで AllSigned / Restricted を
//! 強制した管理端末では負けて導入が止まる。#2790 からは本文をテキストで読み、`-EncodedCommand` の
//! 起動片から走らせる（`platform::shell::powershell_script_text_args`。実行ポリシーの対象外）。
//!
//! 単体テスト（`agent_install.rs` の `issue2790_*`）は Windows のレシピの組み立てを縛る。ここは
//! **組み立ての外側**を見る:
//!
//! 1. 製品のコード（各 crate の `src/` のうちテストのモジュールより前）に、PowerShell へ
//!    `-File` / `-ExecutionPolicy`（略記の `-ep` / `-ex` / `-exec` も）を渡す文字列が増えていない。
//!    A/B の旧アームのように理由があって残すものは、同じ行か直前の行の `tako:ps-file-ok <理由>` で通す
//! 2. インストーラを起こす `Command` は `InstallRecipe::installer_command` の 1 実装だけが組む
//!    （`setup_bootstrap` が引数を自前で組み直す形へ戻らない）
//! 3. Windows の CI がポリシーを強制した状態の実行検査を、全数より手前に名指しで回している
//!    （ステップ・スクリプト・テストの 3 点が揃い、スクリプトは置いたポリシーを `finally` で戻す）

use std::path::{Path, PathBuf};

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

/// 例外の印（同じ行か直前の行に `tako:ps-file-ok <理由>`）
const OK_MARK: &str = "tako:ps-file-ok";

/// PowerShell の実行ポリシー・スクリプトファイルの指定（大文字小文字を問わない。略記も含む）
const FORBIDDEN_TOKENS: &[&str] = &[
    "\"-file\"",
    "\"-executionpolicy\"",
    "\"-ep\"",
    "\"-ex\"",
    "\"-exec\"",
];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// テストのモジュール（`#[cfg(test)]` の直後の行が `mod …`）が始まる行番号（0 始まり）。無ければ行数
fn test_module_start(lines: &[&str]) -> usize {
    for (i, line) in lines.iter().enumerate() {
        if line.trim() != "#[cfg(test)]" {
            continue;
        }
        let next = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
        if next.is_some_and(|l| {
            let l = l.trim_start();
            l.starts_with("mod ") || l.starts_with("pub mod ") || l.starts_with("pub(crate) mod ")
        }) {
            return i;
        }
    }
    lines.len()
}

/// 製品のコードで PowerShell へ `-File` / `-ExecutionPolicy` を渡す行（印の無いもの）を file:line で返す
fn offenders_in(rel: &str, text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let end = test_module_start(&lines);
    let mut out = Vec::new();
    for (i, line) in lines[..end].iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let Some(token) = FORBIDDEN_TOKENS.iter().find(|t| lower.contains(*t)) else {
            continue;
        };
        let marked = line.contains(OK_MARK) || (i > 0 && lines[i - 1].contains(OK_MARK));
        if !marked {
            out.push(format!("{rel}:{} {token}: {}", i + 1, line.trim()));
        }
    }
    out
}

#[test]
fn issue2790_製品のコードはpowershellへfileや実行ポリシーの指定を渡さない() {
    let root = repo_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/")
        .flatten()
    {
        rust_sources(&entry.path().join("src"), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 100,
        "走査の対象が少なすぎる（{} 本）= 置き場の取り違え",
        files.len()
    );
    let mut offenders = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        let text = std::fs::read_to_string(path).unwrap_or_default();
        offenders.extend(offenders_in(&rel, &text));
    }
    assert!(
        offenders.is_empty(),
        "製品のコードが PowerShell へ -File / -ExecutionPolicy を渡している（#2790）。\
         -ExecutionPolicy Bypass は Process スコープなので、グループポリシーで AllSigned / Restricted を\
         強制した PC では負ける。.ps1 は platform::shell::powershell_script_text_args（本文をテキストで読む \
         -EncodedCommand）で起こす。理由があって残すなら同じ行か直前の行に `{OK_MARK} <理由>`:\n{}",
        offenders.join("\n")
    );
}

/// 番犬自身の判定（印・テストのモジュール・コメント行・略記）を合成した入力で縛る
#[test]
fn issue2790_番犬の判定は印とテストのモジュールを見分ける() {
    let src = "fn a() {\n    cmd.args([\"-NoProfile\", \"-File\", path]);\n}\n\
               fn b() {\n    // tako:ps-file-ok A/B の旧アーム\n    \"-ExecutionPolicy\",\n}\n\
               fn c() { cmd.arg(\"-EP\"); } // 略記\n\
               // \"-File\" はコメントなら数えない\n\
               #[cfg(test)]\nmod tests {\n    const X: &[&str] = &[\"-File\"];\n}\n";
    let got = offenders_in("x.rs", src);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0].starts_with("x.rs:2 \"-file\""), "{got:?}");
    assert!(got[1].starts_with("x.rs:8 \"-ep\""), "{got:?}");
}

#[test]
fn issue2790_インストーラを起こすcommandはinstaller_commandの1実装だけが組む() {
    let bootstrap = read("crates/tako-control/src/setup_bootstrap.rs");
    let body = bootstrap
        .split_once("fn run_installer(")
        .map(|(_, rest)| rest.split_once("\n}\n").map(|(b, _)| b).unwrap_or(rest))
        .expect("setup_bootstrap.rs に run_installer が無い");
    assert!(
        body.contains(".installer_command(script)"),
        "run_installer が InstallRecipe::installer_command を通していない（#2790）"
    );
    // 5.1 は -EncodedCommand + stderr の捕捉で CLIXML を書く（-OutputFormat Text を見ない = CI 実測）。
    // 平文へ戻さずに断りの文へ載せると、利用者に XML を見せる
    assert!(
        body.contains("powershell_stderr_text(&stderr_raw)"),
        "run_installer が捕捉した stderr を powershell_stderr_text で平文へ戻していない（#2790）"
    );
    for forbidden in ["runner.args_for(", "runner.candidates", "Command::new("] {
        assert!(
            !body.contains(forbidden),
            "run_installer がインストーラの起こし方を自前で組み直している（{forbidden}。#2790）"
        );
    }
    let install = read("crates/tako-core/src/platform/agent_install.rs");
    assert!(
        install.contains("runner: WINDOWS_POWERSHELL_RUNNER")
            && install.contains("passing: ScriptPassing::PowerShellText"),
        "Windows のレシピが本文をテキストで読む渡し方を使っていない（#2790）"
    );
}

/// ci.yml の windows ジョブのステップ（`      - ` で始まる行ごと）
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

const POLICY_SCRIPT: &str = "scripts/windows/test-install-execution-policy-2790.ps1";
const POLICY_TEST: &str = "issue2790_install_execution_policy_windows";
/// ポリシーの置き方・戻し方の 1 実装（#2791 で #2790 のスクリプトから出して共有した）
const POLICY_LIB: &str = "scripts/windows/lib/execution-policy.ps1";
const POLICY_LIB_DOT_SOURCE: &str = ". (Join-Path $PSScriptRoot 'lib/execution-policy.ps1')";

#[test]
fn issue2790_windowsのciがポリシーを強制した状態の実行検査を名指しで回す() {
    let steps = windows_steps();
    let policy = steps
        .iter()
        .position(|s| s.contains(POLICY_SCRIPT))
        .unwrap_or_else(|| {
            panic!("ci.yml の windows ジョブに {POLICY_SCRIPT} のステップが無い（#2790）")
        });
    assert!(
        !steps[policy].contains("continue-on-error"),
        "ポリシーの実行検査が非ブロッキングに倒されている（#2790）:\n{}",
        steps[policy]
    );
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    assert!(
        policy < full,
        "ポリシーの実行検査は全数より手前に置く（#1282 と同じ理由。#2790）"
    );
    assert!(
        !steps[full].contains("TAKO_2790_POLICY"),
        "全数のテストにポリシーの印を付けない（ポリシーは名指しのステップの中だけで置く）"
    );

    let script = read(POLICY_SCRIPT);
    for needle in [
        POLICY_LIB_DOT_SOURCE,
        "Hive = 'HKLM'; Scope = 'MachinePolicy'; Value = 'AllSigned'",
        "Hive = 'HKCU'; Scope = 'UserPolicy'; Value = 'Restricted'",
        POLICY_TEST,
        "TAKO_2790_POLICY",
    ] {
        assert!(
            script.contains(needle),
            "{POLICY_SCRIPT} に `{needle}` が無い（#2790）"
        );
    }
    // 置き方・戻し方の 1 実装は lib（#2791 の実走の検査と共有）。5.1 と pwsh 7 の両方の置き場を書く
    let lib = read(POLICY_LIB);
    for needle in [
        "SOFTWARE\\Policies\\Microsoft\\Windows\\PowerShell",
        "SOFTWARE\\Policies\\Microsoft\\PowerShellCore",
        "'EnableScripts'",
        "function Set-Policy(",
        "function Restore-Policy(",
        "function Get-PolicySnapshot(",
    ] {
        assert!(
            lib.contains(needle),
            "{POLICY_LIB} に `{needle}` が無い（#2790 / #2791）"
        );
    }
    // 置いたポリシーは finally で戻し、テストは置く前に組む
    let finally = script
        .split_once("} finally {\n        # 置いたポリシーはここで必ず戻す")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("{POLICY_SCRIPT}: ポリシーを finally で戻していない（#2790）"));
    assert!(
        finally
            .trim_start()
            .lines()
            .nth(1)
            .is_some_and(|l| l.contains("Restore-Policy")),
        "{POLICY_SCRIPT}: finally の最初の文がポリシーの戻しでない（#2790）"
    );
    let no_run = script
        .find("--test $TestName --no-run")
        .expect("テストを置く前に組んでいない");
    let set = script
        .find("        Set-Policy ")
        .expect("Set-Policy の呼び出し");
    assert!(
        no_run < set,
        "{POLICY_SCRIPT}: ポリシーを置いてから cargo で組んでいる（#2790）"
    );

    let test = read(&format!("crates/tako-core/tests/{POLICY_TEST}.rs"));
    for needle in [
        "#![cfg(windows)]",
        ".installer_command(script)",
        ".args_for_in(script, true)",
        "TAKO_2790_POLICY",
        "official_command.replace(",
    ] {
        assert!(
            test.contains(needle),
            "{POLICY_TEST}.rs に `{needle}` が無い（#2790）"
        );
    }
}
