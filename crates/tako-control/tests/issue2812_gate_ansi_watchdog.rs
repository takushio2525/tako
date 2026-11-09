//! 受け入れゲートの evidence に pwsh 7 の色の符号（ANSI の ESC 列）と 5.1 の包みのスクリプトの行が戻らないことの
//! 番犬（#2812）。
//!
//! 単体テスト（`platform::shell::tests_2812` / `acceptance_gates::tests_2812`）は答えを縛る。ここは**配線と 1 実装**を見る:
//!
//! 1. 平文化（`powershell_stderr_text`）は CLIXML の塊から戻した文字を `strip_ansi_escapes` へ通す（ゲートと導入代行の
//!    共有の 1 実装の中で除く。ゲートの側に 2 つ目の除き方を書かない）。入口は `!legacy_2812()` を渡す
//! 2. ゲートの包み（`powershell_output_script`）は 5.1 のときだけ `$MyInvocation` を外し、その片を利用者のコマンドより
//!    前（2 行目の末尾 = 利用者のコマンドの行番号を変えない）に置く。入口は `legacy_2812()` を渡す
//! 3. Windows の CI が実物の 5.1 / pwsh 7 でゲートの述語を失敗させる検査を、全数より手前に名指しで回す
//! 4. 仕様（`.agent/orchestrator-design.md`）が「色の符号は残る」へ戻っていない

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
const GATES: &str = "crates/tako-control/src/acceptance_gates.rs";
const SPEC: &str = ".agent/orchestrator-design.md";
const WINDOWS_TEST: &str = "issue2812_gate_ansi_windows";

/// `fn <name>(` から次の最上位の `}` までの本文と、その始まりの行番号（1 始まり）
fn fn_body<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let head = format!("fn {name}(");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に fn {name} が無い（#2812）"));
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
fn issue2812_平文化はclixmlから戻した文字の色の符号を除く() {
    let src = read(SHELL);
    let (entry, entry_line) = fn_body(&src, SHELL, "powershell_stderr_text");
    assert!(
        entry.contains("powershell_stderr_text_in(raw, !legacy_2812())"),
        "{SHELL}:{entry_line}: powershell_stderr_text が本体へ色の符号を除く指定 `!legacy_2812()` を渡していない（#2812）"
    );
    let (body, line) = fn_body(&src, SHELL, "powershell_stderr_text_in");
    let streams = body.find("clixml_streams_text(").unwrap_or_else(|| {
        panic!("{SHELL}:{line}: powershell_stderr_text_in が CLIXML の塊を clixml_streams_text で読んでいない")
    });
    let strip = body.find("strip_ansi_escapes(&streams)").unwrap_or_else(|| {
        panic!(
            "{SHELL}:{}: powershell_stderr_text_in が塊から戻した文字を strip_ansi_escapes へ通していない\
             （pwsh 7 の色の符号が evidence と導入代行の断りの文へ載る。#2812）",
            line_in(body, line, "clixml_streams_text(")
        )
    });
    assert!(
        streams < strip && body[..strip].contains("if strip_ansi"),
        "{SHELL}:{}: 色の符号を除く段が `if strip_ansi` の門の下に無い（#2812）",
        line_in(body, line, "strip_ansi_escapes(")
    );
    // 塊の外の素のテキスト（ネイティブの子の出力）には掛けない = CLIXML でない stderr は 1 バイトも変えない（#2800）
    assert_eq!(
        body.matches("strip_ansi_escapes(").count(),
        1,
        "{SHELL}:{line}: powershell_stderr_text_in が塊の外の素のテキストまで色の符号を除いている（#2800 / #2812）"
    );
    let (legacy, legacy_line) = fn_body(&src, SHELL, "legacy_2812");
    assert!(
        legacy.contains("ISSUE2812_LEGACY_ENV") && src.contains("= \"TAKO_2812_LEGACY\";"),
        "{SHELL}:{legacy_line}: A/B の門が TAKO_2812_LEGACY を読んでいない（#2812）"
    );
    // ゲートの側に 2 つ目の除き方を書かない（共有の 1 実装を通す）
    let gates = read(GATES);
    let (window, window_line) = fn_body(&gates, GATES, "plain_stderr_window");
    for forbidden in ["\\u{1b}", "\\x1b", "strip_ansi"] {
        assert!(
            !window.contains(forbidden),
            "{GATES}:{}: plain_stderr_window が色の符号を自前で除いている（`{forbidden}`。#2812）",
            line_in(window, window_line, forbidden)
        );
    }
}

#[test]
fn issue2812_ゲートの包みは51でだけ呼び出し元を外す() {
    let src = read(SHELL);
    let (entry, entry_line) = fn_body(&src, SHELL, "powershell_output_script");
    assert!(
        entry.contains("powershell_output_script_in(command, legacy_2812())"),
        "{SHELL}:{entry_line}: powershell_output_script が本体へ `legacy_2812()` を渡していない（#2812）"
    );
    let (body, line) = fn_body(&src, SHELL, "powershell_output_script_in");
    let progress = "$ProgressPreference = 'SilentlyContinue'{forget_invocation}\\n";
    let at = body.find(progress).unwrap_or_else(|| {
        panic!(
            "{SHELL}:{line}: powershell_output_script_in が 5.1 の `$MyInvocation` を外す片を 2 行目の末尾\
             （`$ProgressPreference` の行）に置いていない。利用者のコマンドより後ろでは最上位の Write-Error が\
             包みを拾ったまま・別の行に置くと位置の行番号がずれる（#2812）"
        )
    });
    let command = body
        .find("powershell_exit_code_script(command)")
        .unwrap_or_else(|| {
            panic!("{SHELL}:{line}: powershell_output_script_in が終了コードの共有の片を通していない（#935）")
        });
    assert!(
        at < command,
        "{SHELL}:{}: `$MyInvocation` を外す片が利用者のコマンドより後ろにある（#2812）",
        line_in(body, line, "{forget_invocation}")
    );
    let decl = src
        .find("const POWERSHELL_51_FORGET_INVOCATION")
        .unwrap_or_else(|| panic!("{SHELL} に POWERSHELL_51_FORGET_INVOCATION が無い（#2812）"));
    let decl_line = src[..decl].matches('\n').count() + 1;
    let rest = &src[decl..];
    // 値は `"; if …` で始まるので、終わりは `";` + 改行で探す
    let value = &rest[..rest.find("\";\n").map(|i| i + 2).unwrap_or(rest.len())];
    for needle in [
        "$MyInvocation = $null",
        "$PSVersionTable.PSVersion.Major -le 5",
    ] {
        assert!(
            value.contains(needle),
            "{SHELL}:{decl_line}: POWERSHELL_51_FORGET_INVOCATION に `{needle}` が無い（#2812。pwsh 7 で外すと\
             エラーの表示が長くなり、5.1 で外さないと包みの行が出る）"
        );
    }
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
fn issue2812_windowsのciが実物の51とpwsh7でゲートを失敗させる検査を名指しで回す() {
    let steps = windows_steps();
    let gate = steps
        .iter()
        .position(|s| s.contains(&format!("--test {WINDOWS_TEST}")))
        .unwrap_or_else(|| {
            panic!("ci.yml の windows ジョブに `--test {WINDOWS_TEST}` のステップが無い（#2812）")
        });
    let step = &steps[gate];
    assert!(
        step.contains("--nocapture") && step.contains("-p tako-control"),
        "#2812 のステップが結果の行（TAKO2812_*）をログへ残していない:\n{step}"
    );
    assert!(
        !step.contains("continue-on-error") && !step.contains("if:"),
        "#2812 のステップが非ブロッキング・条件つきに倒されている（外部に依らないので毎回走らせる）:\n{step}"
    );
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    assert!(
        gate < full,
        "#2812 のステップは全数より手前に置く（#1282 と同じ理由）"
    );

    let test = read(&format!("crates/tako-control/tests/{WINDOWS_TEST}.rs"));
    for needle in [
        "#![cfg(windows)]",
        "tako_core::run_alone!(",
        "execute_gate_check_at(",
        "ISSUE2812_LEGACY_ENV",
        "ISSUE2800_LEGACY_ENV",
        "\"ProgramFiles\"",
        "POWERSHELL_51_TAIL",
        "esc_count(evidence)",
        "WRAPPER_LINES",
    ] {
        assert!(
            test.contains(needle),
            "{WINDOWS_TEST}.rs に `{needle}` が無い（#2812）"
        );
    }
}

#[test]
fn issue2812_仕様が色の符号は残るへ戻っていない() {
    let spec = read(SPEC);
    let start = spec
        .find("**stderr の CLIXML を平文へ戻す（#2800）**")
        .unwrap_or_else(|| panic!("{SPEC} にゲートの stderr の平文化の節が無い"));
    let end = spec[start..]
        .find("\n\n")
        .map(|i| start + i)
        .unwrap_or(spec.len());
    let section = &spec[start..end];
    let line = spec[..start].matches('\n').count() + 1;
    assert!(
        !section.contains("色の符号は残る"),
        "{SPEC}:{line}: ゲートの節が「pwsh 7 の色の符号は残る」のまま。#2812 から平文化が除く"
    );
    for needle in ["#2812", "TAKO_2812_LEGACY", "$MyInvocation"] {
        assert!(
            section.contains(needle),
            "{SPEC}:{line}: ゲートの節に `{needle}` が無い（#2812 の色の符号と 5.1 の包みの行）"
        );
    }
}
