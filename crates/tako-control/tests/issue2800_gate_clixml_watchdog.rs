//! 受け入れゲートの evidence が 5.1 の CLIXML を平文へ戻さない形へ戻らないことの番犬（#2800）。
//!
//! Windows の述語は境界 B1 が PowerShell へ `-EncodedCommand` で渡すので、5.1 は捕捉された stderr に cmdlet の
//! エラーレコードを CLIXML で書く。#2800 より前のゲートはそれを evidence へそのまま載せていた（導入代行の
//! `run_installer` だけが #2790 の平文化を通していた = 5.1 の stderr を捕捉する口が 2 つあり片方だけ直っていた）。
//!
//! 単体テスト（`acceptance_gates::tests_2800`）は `format_evidence` の答えを縛る。ここは**配線と 1 実装**を見る:
//!
//! 1. `execute_command` は捕捉した stderr を `format_evidence(…, !legacy_2800())` へ渡す（平文化を切って渡さない）
//! 2. `format_evidence` は stderr の範囲を `plain_stderr_window` へ通し、そこは
//!    `platform::shell::powershell_stderr_text` を呼ぶ（CLIXML の読み方をゲートの側に 2 つ目として書かない）
//! 3. Windows の CI が実物の powershell.exe（5.1）でゲートの述語を失敗させる検査を、全数より手前に名指しで回す
//! 4. `powershell_output_script` の doc が「既知の限界」（evidence が XML で読みにくい）へ戻っていない

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

const GATES: &str = "crates/tako-control/src/acceptance_gates.rs";
const SHELL: &str = "crates/tako-core/src/platform/shell.rs";
const WINDOWS_TEST: &str = "issue2800_gate_clixml_windows";

/// `fn <name>(` から次の最上位の `}` までの本文と、その始まりの行番号（1 始まり）
fn fn_body<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let head = format!("fn {name}(");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に fn {name} が無い（#2800）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 2).unwrap_or(rest.len());
    let line = src[..start].lines().count() + 1;
    (&rest[..end], line)
}

/// 本文の中で `needle` が最初に現れる行（ファイル全体の 1 始まり。無ければ本文の始まり）
fn line_in(body: &str, body_line: usize, needle: &str) -> usize {
    body.find(needle)
        .map(|i| body_line + body[..i].matches('\n').count())
        .unwrap_or(body_line)
}

#[test]
fn issue2800_ゲートの実行は捕捉したstderrを平文化へ渡す() {
    let src = read(GATES);
    let (body, line) = fn_body(&src, GATES, "execute_command");
    let call = body
        .find("format_evidence(")
        .map(|i| &body[i..])
        .and_then(|rest| rest.split_once(';').map(|(c, _)| c))
        .unwrap_or_else(|| {
            panic!("{GATES}:{line}: execute_command が format_evidence を通していない（#2800）")
        });
    assert!(
        call.contains("!legacy_2800()"),
        "{GATES}:{}: execute_command が format_evidence へ平文化の指定 `!legacy_2800()` を渡していない（#2800。\
         渡しているのは `{call}`）",
        line_in(body, line, "format_evidence(")
    );
    let (legacy, legacy_line) = fn_body(&src, GATES, "legacy_2800");
    assert!(
        legacy.contains("ISSUE2800_LEGACY_ENV") && src.contains("= \"TAKO_2800_LEGACY\";"),
        "{GATES}:{legacy_line}: A/B の門が TAKO_2800_LEGACY を読んでいない（#2800）"
    );
}

#[test]
fn issue2800_evidenceのstderrは平文化の1実装を通す() {
    let src = read(GATES);
    let (body, line) = fn_body(&src, GATES, "format_evidence");
    assert!(
        body.contains("plain_stderr_window(stderr_trimmed, &window)"),
        "{GATES}:{line}: format_evidence が stderr の範囲を plain_stderr_window へ通していない（#2800）"
    );
    let (window, window_line) = fn_body(&src, GATES, "plain_stderr_window");
    assert!(
        window.contains("powershell_stderr_text(input)"),
        "{GATES}:{window_line}: plain_stderr_window が platform::shell::powershell_stderr_text を呼んでいない\
         （導入代行と同じ 1 実装を通す。#2800）"
    );
    // CLIXML の読み方（ストリームの要素・`_xHHHH_`）をゲートの側に 2 つ目として書かない
    for forbidden in ["<S S=", "_x000D_", "</Objs>", "clixml_"] {
        assert!(
            !window.contains(forbidden),
            "{GATES}:{}: plain_stderr_window が CLIXML を自前で読んでいる（`{forbidden}`。#2800）",
            line_in(window, window_line, forbidden)
        );
    }
    // 見出しは平文化の側の定数を使う（文字列を 2 か所で持たない）
    assert!(
        window.contains("POWERSHELL_CLIXML_MARK") && !window.contains("\"#< CLIXML\""),
        "{GATES}:{window_line}: 見出しを platform::shell::POWERSHELL_CLIXML_MARK から引いていない（#2800）"
    );
    let shell = read(SHELL);
    // 本体は #2812 から `_in`（色の符号を除くかを引数で受ける純粋関数）。入口はそれへ委ねるだけ
    let (text, text_line) = fn_body(&shell, SHELL, "powershell_stderr_text_in");
    assert!(
        text.contains("const MARK: &str = POWERSHELL_CLIXML_MARK;"),
        "{SHELL}:{text_line}: powershell_stderr_text_in が見出しを POWERSHELL_CLIXML_MARK から引いていない（#2800）"
    );
}

#[test]
fn issue2800_既知の限界の記述へ戻っていない() {
    let shell = read(SHELL);
    let start = shell
        .find("/// PTY 無しで走らせる PowerShell スクリプト（純粋関数）。")
        .expect("powershell_output_script の doc が無い");
    let doc_end = shell[start..]
        .find("fn powershell_output_script(")
        .expect("powershell_output_script が無い");
    let doc = &shell[start..start + doc_end];
    let line = shell[..start].lines().count() + 1;
    assert!(
        !doc.contains("既知の限界"),
        "{SHELL}:{line}: powershell_output_script の doc が「既知の限界」（evidence が XML で読みにくい）のまま。\
         #2800 からゲートは平文へ戻すので実態と食い違う"
    );
    assert!(
        doc.contains("powershell_stderr_text"),
        "{SHELL}:{line}: powershell_output_script の doc が平文化の口（powershell_stderr_text）を案内していない（#2800）"
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
fn issue2800_windowsのciが51でゲートを失敗させる検査を名指しで回す() {
    let steps = windows_steps();
    let gate = steps
        .iter()
        .position(|s| s.contains(&format!("--test {WINDOWS_TEST}")))
        .unwrap_or_else(|| {
            panic!("ci.yml の windows ジョブに `--test {WINDOWS_TEST}` のステップが無い（#2800）")
        });
    let step = &steps[gate];
    assert!(
        step.contains("--nocapture") && step.contains("-p tako-control"),
        "#2800 のステップが結果の行（TAKO2800_*）をログへ残していない:\n{step}"
    );
    assert!(
        !step.contains("continue-on-error") && !step.contains("if:"),
        "#2800 のステップが非ブロッキング・条件つきに倒されている（外部に依らないので毎回走らせる）:\n{step}"
    );
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    assert!(
        gate < full,
        "#2800 のステップは全数より手前に置く（#1282 と同じ理由）"
    );

    let test = read(&format!("crates/tako-control/tests/{WINDOWS_TEST}.rs"));
    for needle in [
        "#![cfg(windows)]",
        "tako_core::run_alone!(",
        "execute_gate_check_at(",
        "ISSUE2800_LEGACY_ENV",
        "\"ProgramFiles\"",
        "POWERSHELL_51_TAIL",
        "old_evidence.contains(\"#< CLIXML\")",
    ] {
        assert!(
            test.contains(needle),
            "{WINDOWS_TEST}.rs に `{needle}` が無い（#2800）"
        );
    }
}
