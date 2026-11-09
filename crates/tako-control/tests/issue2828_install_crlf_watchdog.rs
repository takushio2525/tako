//! エージェント CLI の導入代行の断りの文（と成功したときの応答の output）に、PowerShell が書いた CR が戻らないことの番犬
//! （#2828）。
//!
//! 単体テスト（`platform::shell::tests_2828`）は答えを縛る。ここは**配線と 1 実装**を見る:
//!
//! 1. 断りの文を組む `setup_bootstrap::run_installer`（CLI と MCP がどちらも通る 1 実装）は、stdout と平文化した stderr の
//!    両方を `powershell_captured_text`（起こしたインタプリタを渡す）に通す。CR を自前で扱う 2 つ目の実装を書かない
//! 2. `powershell_captured_text` は `legacy_2828()` の門の下でだけ素通しし、インタプリタの判定は
//!    `ShellDialect::from_program` の 1 実装（bash / sh の経路 = macOS を 1 バイトも変えない）
//! 3. 受け入れゲートの evidence が通る平文化（`powershell_stderr_text`）には #2828 を入れない（ゲートの経路は不変）
//! 4. Windows の CI が実物の 5.1 / pwsh 7 で導入代行を失敗させる検査を、全数より手前に名指しで回す
//! 5. 仕様（`.agent/requirements.md` の FR-2.33.1）が #2828 の形を書いている

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

const SHELL: &str = "crates/tako-core/src/platform/shell.rs";
const BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";
const SPEC: &str = ".agent/requirements.md";
const INSTALL_TEST: &str = "issue2828_install_crlf_windows";

/// `fn <name>(` / `fn <name><'a>(` から次の最上位の `}` までの本文と、その始まりの行番号（1 始まり）
fn fn_body<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let start = [format!("fn {name}("), format!("fn {name}<")]
        .iter()
        .find_map(|head| src.find(head.as_str()))
        .unwrap_or_else(|| panic!("{rel} に fn {name} が無い（#2828）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 2).unwrap_or(rest.len());
    let line = src[..start].matches('\n').count() + 1;
    (&rest[..end], line)
}

/// 本文の中で `needle` が最初に現れる行（ファイル全体の 1 始まり。無ければ本文の始まり）
fn line_in(body: &str, body_line: usize, needle: &str) -> usize {
    body.find(needle)
        .map(|i| body_line + body[..i].matches('\n').count())
        .unwrap_or(body_line)
}

#[test]
fn issue2828_断りの文はstdoutとstderrを1実装の改行の揃えに通す() {
    let src = read(BOOTSTRAP);
    let (body, line) = fn_body(&src, BOOTSTRAP, "run_installer");
    for needle in [
        "powershell_captured_text(&shell, &stdout_raw)",
        "tako_core::platform::shell::powershell_stderr_text(&stderr_raw)",
        "powershell_captured_text(&shell, &stderr_plain)",
    ] {
        assert!(
            body.contains(needle),
            "{BOOTSTRAP}:{line}: run_installer に `{needle}` が無い = 断りの文に PowerShell の CR が残る（#2828）"
        );
    }
    for forbidden in ["'\\r'", "\"\\r\\n\"", "\"\\r\"", "newlines_to_lf"] {
        assert!(
            !body.contains(forbidden),
            "{BOOTSTRAP}:{}: run_installer が改行を自前で扱っている（`{forbidden}`。揃えるのは \
             platform::shell::powershell_captured_text の 1 実装。#2828）",
            line_in(body, line, forbidden)
        );
    }
}

#[test]
fn issue2828_揃えるのはpowershellのときだけでゲートの平文化には入れない() {
    let src = read(SHELL);
    let (body, line) = fn_body(&src, SHELL, "powershell_captured_text");
    for needle in [
        "legacy_2828()",
        "ShellDialect::from_program(program) != Some(ShellDialect::PowerShell)",
        "newlines_to_lf(text)",
    ] {
        assert!(
            body.contains(needle),
            "{SHELL}:{line}: powershell_captured_text に `{needle}` が無い（#2828。インタプリタの判定は 1 実装・\
             bash / sh の経路は借用のまま）"
        );
    }
    let (lf, lf_line) = fn_body(&src, SHELL, "newlines_to_lf");
    assert!(
        lf.contains(".replace(\"\\r\\n\", \"\\n\").replace('\\r', \"\\n\")"),
        "{SHELL}:{lf_line}: newlines_to_lf が CRLF と単独の CR の両方を LF にしていない（単独の CR を捨てると本文が消えうる。#2828）"
    );
    for name in ["powershell_stderr_text", "powershell_stderr_text_in"] {
        let (gate, gate_line) = fn_body(&src, SHELL, name);
        for forbidden in ["powershell_captured_text", "newlines_to_lf", "legacy_2828"] {
            assert!(
                !gate.contains(forbidden),
                "{SHELL}:{}: 受け入れゲートの evidence も通る {name} に #2828 を入れている（`{forbidden}`。ゲートの経路は不変）",
                line_in(gate, gate_line, forbidden)
            );
        }
    }
    assert!(
        src.contains("pub const ISSUE2828_LEGACY_ENV: &str = \"TAKO_2828_LEGACY\";"),
        "{SHELL}: A/B の env が TAKO_2828_LEGACY でない（#2828）"
    );
}

/// ci.yml の windows ジョブのステップ（`      - ` で始まる行ごと。コメント行は除く）
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

#[test]
fn issue2828_windowsのciが実物の51とpwsh7で導入代行を失敗させる検査を名指しで回す() {
    let steps = windows_steps();
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    let at = steps
        .iter()
        .position(|s| s.contains(&format!("--test {INSTALL_TEST}")))
        .unwrap_or_else(|| {
            panic!("ci.yml の windows ジョブに `--test {INSTALL_TEST}` のステップが無い（#2828）")
        });
    let step = &steps[at];
    assert!(
        step.contains("--nocapture") && step.contains("-p tako-control"),
        "#2828 の {INSTALL_TEST} のステップが結果の行（TAKO2828_*）をログへ残していない:\n{step}"
    );
    assert!(
        !step.contains("continue-on-error") && !step.contains("if:"),
        "#2828 の {INSTALL_TEST} のステップが非ブロッキング・条件つきに倒されている（外部に依らないので毎回走らせる）:\n{step}"
    );
    assert!(
        at < full,
        "#2828 の {INSTALL_TEST} のステップは全数より手前に置く（#1282 と同じ理由）"
    );
    let body = read(&format!("crates/tako-control/tests/{INSTALL_TEST}.rs"));
    for needle in [
        "#![cfg(windows)]",
        "tako_core::run_alone!(",
        "run_installer(",
        "ISSUE2828_LEGACY_ENV",
        "path_without_windows_powershell",
        "POWERSHELL_51_TAIL",
        "cr_count(",
        "newlines_to_lf(&old.log)",
        "\"lone-cr\"",
    ] {
        assert!(
            body.contains(needle),
            "{INSTALL_TEST}.rs に `{needle}` が無い（#2828）"
        );
    }
}

#[test]
fn issue2828_仕様が導入代行の改行の揃え方を書いている() {
    let spec = read(SPEC);
    let fr = "| FR-2.33.1 |";
    let start = spec
        .find(fr)
        .unwrap_or_else(|| panic!("{SPEC} に {fr} の行が無い"));
    let row = &spec[start..spec[start..].find('\n').map_or(spec.len(), |i| start + i)];
    let line = spec[..start].matches('\n').count() + 1;
    for needle in [
        "#2828",
        "CR",
        "LF",
        "TAKO_2828_LEGACY",
        "powershell_captured_text",
    ] {
        assert!(
            row.contains(needle),
            "{SPEC}:{line}: {fr} に `{needle}` が無い（#2828 の導入代行の改行の揃え方）"
        );
    }
}
