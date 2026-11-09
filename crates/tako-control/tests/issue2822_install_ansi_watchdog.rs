//! エージェント CLI の導入代行の断りの文に pwsh 7 の色の符号と 5.1 のインストーラ本文が、実行ペインの画面に 5.1 の
//! 包みの行が戻らないことの番犬（#2822）。
//!
//! 単体テスト（`platform::shell::tests_2822` / `platform::agent_install` の #2790 / #2791 の 2 本）は答えを縛る。
//! ここは**配線と 1 実装**を見る:
//!
//! 1. 導入代行の起動片の入口（`powershell_script_text_command`）は `legacy_2822()` の門の下でだけ #2822 前の形へ戻し、
//!    既定は本体（`powershell_installer_function_command`）を通す。本体は 1 行目で捕捉されているときだけ pwsh 7 の色を
//!    止め（`$null -ne $PSStyle` を見てから = 5.1 で止まらない）、本文を関数 `install.ps1` として定義して名前で
//!    ドットソースする（`$MyInvocation` を `$null` にしない = agy の `install.ps1` が StrictMode の下で読む）
//! 2. 実行ペインの包み（#1657 の形と #1657 前の形）は `legacy_2822()` を渡し、終了コードの片を
//!    `powershell_pane_exit_code_script` から取る（5.1 の片を 1 行目の末尾 = 利用者のコマンドより前に置く）。
//!    受け入れゲートの包み（#2812）は共有の片 `powershell_exit_code_script` のまま（字面を変えない）
//! 3. 断りの文を組むのは `setup_bootstrap::run_installer` の 1 実装（CLI と MCP がどちらも通る）で、捕捉した
//!    stderr は共有の平文化（`powershell_stderr_text`）を通す。色の符号を自前で除く 2 つ目の実装を書かない
//! 4. Windows の CI が実物の 5.1 / pwsh 7 で導入代行と実行ペインを失敗させる検査を、全数より手前に名指しで回す
//! 5. 仕様（`.agent/requirements.md` の FR-2.33.1 / FR-3.18.1）が #2822 の形を書いている

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
const INSTALL_TEST: &str = "issue2822_install_ansi_windows";
const PANE_TEST: &str = "issue2822_run_pane_windows";

/// `fn <name>(` から次の最上位の `}` までの本文と、その始まりの行番号（1 始まり）
fn fn_body<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let head = format!("fn {name}(");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に fn {name} が無い（#2822）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 2).unwrap_or(rest.len());
    let line = src[..start].matches('\n').count() + 1;
    (&rest[..end], line)
}

/// `const <name>: &str =` の値（次の `";` + 改行まで）と、その行番号
fn const_value<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let head = format!("const {name}: &str =");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に const {name} が無い（#2822）"));
    let rest = &src[start..];
    let end = rest.find("\";\n").map(|i| i + 2).unwrap_or(rest.len());
    (&rest[..end], src[..start].matches('\n').count() + 1)
}

/// 本文の中で `needle` が最初に現れる行（ファイル全体の 1 始まり。無ければ本文の始まり）
fn line_in(body: &str, body_line: usize, needle: &str) -> usize {
    body.find(needle)
        .map(|i| body_line + body[..i].matches('\n').count())
        .unwrap_or(body_line)
}

#[test]
fn issue2822_導入代行の起動片は関数をドットソースし捕捉中だけ色を止める() {
    let src = read(SHELL);
    let (entry, entry_line) = fn_body(&src, SHELL, "powershell_script_text_command");
    let gate = entry.find("if legacy_2822()").unwrap_or_else(|| {
        panic!("{SHELL}:{entry_line}: powershell_script_text_command に A/B の門 `if legacy_2822()` が無い（#2822）")
    });
    let legacy = entry
        .find("powershell_script_text_command_in(script, false)")
        .unwrap_or_else(|| {
            panic!(
                "{SHELL}:{entry_line}: powershell_script_text_command の旧アームが #2822 前の形\
                 （`powershell_script_text_command_in(script, false)`）を返していない"
            )
        });
    let new = entry
        .find("powershell_installer_function_command(script)")
        .unwrap_or_else(|| {
            panic!(
                "{SHELL}:{entry_line}: powershell_script_text_command の既定が本体 \
                 powershell_installer_function_command を通していない（#2822）"
            )
        });
    assert!(
        gate < legacy && legacy < new,
        "{SHELL}:{}: 旧形が `if legacy_2822()` の門の外にある（#2822）",
        line_in(entry, entry_line, "powershell_script_text_command_in(")
    );

    let (body, line) = fn_body(&src, SHELL, "powershell_installer_function_command");
    let plain = body.find("{POWERSHELL_PLAIN_WHEN_CAPTURED}\\n").unwrap_or_else(|| {
        panic!("{SHELL}:{line}: 起動片の 1 行目が色の止め（POWERSHELL_PLAIN_WHEN_CAPTURED）でない（#2822）")
    });
    let define = body.find("${{function:{name}}} = [scriptblock]::Create(").unwrap_or_else(|| {
        panic!(
            "{SHELL}:{line}: 起動片が本文を関数として定義していない。scriptblock を直にドットソースすると 5.1 の\
             最上位の Write-Error がインストーラの全文を断りの文の前に出す（#2822）"
        )
    });
    // 呼び出しは `. {name}` だけの行（`format!` の継続行なので字下げを落として行ごとに見る）
    let call = body
        .lines()
        .position(|l| l.trim() == ". {name}\\n\\")
        .map(|i| body.lines().take(i).map(|l| l.len() + 1).sum::<usize>())
        .unwrap_or_else(|| {
            panic!(
                "{SHELL}:{}: 起動片が関数をその名前だけの行でドットソースしていない（子スコープでは agy の取得物の\
                 片付けが飛ぶ = #2791・行に他の文があると 5.1 の位置の行に起動片が出る = #2822）",
                line_in(body, line, "${{function:")
            )
        });
    assert!(
        plain < define && define < call,
        "{SHELL}:{line}: 起動片の並びが「色の止め → 定義 → 呼び出し」でない（#2822）"
    );
    assert!(
        !body.contains("$MyInvocation"),
        "{SHELL}:{}: 起動片が $MyInvocation を外している。agy の install.ps1 は Set-StrictMode -Version Latest の下で\
         最上位の $MyInvocation.MyCommand を読むので止まる（#2822）",
        line_in(body, line, "$MyInvocation")
    );
    let (name, name_line) = const_value(&src, SHELL, "POWERSHELL_INSTALLER_FUNCTION");
    assert!(
        name.contains("\"install.ps1\""),
        "{SHELL}:{name_line}: 本文を定義する関数の名前が install.ps1 でない（断りの文に出る名前。#2822）"
    );
    let (value, value_line) = const_value(&src, SHELL, "POWERSHELL_PLAIN_WHEN_CAPTURED");
    for (needle, why) in [
        (
            "$null -ne $PSStyle -and",
            "5.1 には $PSStyle が無い。見ずに書くと 5.1 で止まる",
        ),
        (
            "$PSStyle.OutputRendering = 'PlainText'",
            "pwsh 7 の色の符号が断りの文へ載る",
        ),
        (
            "[Console]::IsErrorRedirected",
            "端末で対話実行するときの色まで止める",
        ),
        (
            "catch { }",
            "制約付き言語モードで書けないと起動片ごと止まる",
        ),
    ] {
        assert!(
            value.contains(needle),
            "{SHELL}:{value_line}: POWERSHELL_PLAIN_WHEN_CAPTURED に `{needle}` が無い（{why}。#2822）"
        );
    }
    let (gate_fn, gate_line) = fn_body(&src, SHELL, "legacy_2822");
    assert!(
        gate_fn.contains("ISSUE2822_LEGACY_ENV") && src.contains("= \"TAKO_2822_LEGACY\";"),
        "{SHELL}:{gate_line}: A/B の門が TAKO_2822_LEGACY を読んでいない（#2822）"
    );
}

#[test]
fn issue2822_実行ペインの包みは51の片を1行目に置きゲートの包みは変えない() {
    let src = read(SHELL);
    // 入口が本体へ渡す引数の 1 つとして `legacy_2822()` を読む（#2827 で引数が増え、rustfmt が呼び出しを折り返すので
    // 閉じ括弧の位置までは見ない）
    for (entry_name, body_name, arg) in [
        (
            "powershell_run_pane_script",
            "powershell_run_pane_script_in",
            "legacy_2822()",
        ),
        (
            "powershell_run_script",
            "powershell_run_script_in",
            "legacy_2822()",
        ),
    ] {
        let (entry, entry_line) = fn_body(&src, SHELL, entry_name);
        assert!(
            entry.contains(&format!("{body_name}(")) && entry.contains(arg),
            "{SHELL}:{entry_line}: {entry_name} が本体へ `legacy_2822()` を渡していない（#2822）"
        );
        let (body, line) = fn_body(&src, SHELL, body_name);
        assert!(
            body.contains("powershell_pane_exit_code_script(command, legacy_2822)"),
            "{SHELL}:{line}: {body_name} が実行ペインの終了コードの片（powershell_pane_exit_code_script）を\
             通していない = 5.1 の最上位の Write-Error で包みの行が画面に出る（#2822）"
        );
        assert!(
            !body.contains("powershell_exit_code_script(command)"),
            "{SHELL}:{}: {body_name} が 5.1 の片の無い共有の片を直に使っている（#2822）",
            line_in(body, line, "powershell_exit_code_script(command)")
        );
    }
    let (pane, pane_line) = fn_body(&src, SHELL, "powershell_pane_exit_code_script");
    assert!(
        pane.contains("POWERSHELL_51_FORGET_INVOCATION")
            && pane.contains("powershell_exit_code_script_in(command, tail)"),
        "{SHELL}:{pane_line}: powershell_pane_exit_code_script が 5.1 の片を共有の片の 1 行目の末尾へ\
         渡していない（別の行に置くと利用者のコマンドの行番号がずれる。#2822）"
    );
    let (shared, shared_line) = fn_body(&src, SHELL, "powershell_exit_code_script_in");
    let lines: Vec<&str> = shared.lines().map(str::trim).collect();
    let first = lines
        .iter()
        .position(|l| *l == "\"$global:LASTEXITCODE = $null{first_line_tail}\\n\\");
    assert!(
        first.is_some_and(|i| lines.get(i + 1) == Some(&"{command}\\n\\")),
        "{SHELL}:{shared_line}: 共有の片の 1 行目の末尾に first_line_tail が無い・利用者のコマンドが 2 行目でない（#2822）"
    );
    // ゲートの包み（#2812）は 5.1 の片の無い共有の片のまま（字面を変えない = 受け入れ条件 5）
    let (gate, gate_line) = fn_body(&src, SHELL, "powershell_output_script_in");
    assert!(
        gate.contains("powershell_exit_code_script(command)")
            && !gate.contains("powershell_pane_exit_code_script"),
        "{SHELL}:{gate_line}: 受け入れゲートの包みが実行ペインの片へ寄っている（ゲートの evidence が変わる。#2812 / #2822）"
    );
    let (plain, plain_line) = fn_body(&src, SHELL, "powershell_exit_code_script");
    assert!(
        plain.contains("powershell_exit_code_script_in(command, \"\")"),
        "{SHELL}:{plain_line}: 共有の片の既定が 1 行目に何かを足している（ゲートと失敗時だけ止める形の字面が変わる。#2822）"
    );
}

#[test]
fn issue2822_断りの文は1実装が組み色の符号を自前で除かない() {
    let src = read(BOOTSTRAP);
    let (body, line) = fn_body(&src, BOOTSTRAP, "run_installer");
    assert!(
        src.contains("pub fn run_installer("),
        "{BOOTSTRAP}:{line}: run_installer が公開されていない（Windows の CI の {INSTALL_TEST} が製品の口を通せない）"
    );
    for needle in [
        "r.installer_command(script)",
        "tako_core::platform::shell::powershell_stderr_text(&stderr_raw)",
    ] {
        assert!(
            body.contains(needle),
            "{BOOTSTRAP}:{line}: run_installer に `{needle}` が無い（起こし方と平文化は 1 実装を通す。#2790 / #2822）"
        );
    }
    for forbidden in ["\\u{1b}", "\\x1b", "strip_ansi", "OutputRendering"] {
        assert!(
            !body.contains(forbidden),
            "{BOOTSTRAP}:{}: run_installer が色の符号を自前で扱っている（`{forbidden}`。止めるのは起動片の 1 行目。#2822）",
            line_in(body, line, forbidden)
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
fn issue2822_windowsのciが実物の51とpwsh7で導入代行と実行ペインを失敗させる検査を名指しで回す() {
    let steps = windows_steps();
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    for (test, krate, needles) in [
        (
            INSTALL_TEST,
            "-p tako-control",
            &[
                "#![cfg(windows)]",
                "tako_core::run_alone!(",
                "run_installer(",
                "ISSUE2822_LEGACY_ENV",
                "ISSUE2812_LEGACY_ENV",
                "path_without_windows_powershell",
                "POWERSHELL_51_TAIL",
                "esc_count(text)",
                "BODY_MARK",
            ][..],
        ),
        (
            PANE_TEST,
            "-p tako-core",
            &[
                "#![cfg(windows)]",
                "tako_core::run_alone!(",
                "run_pane_command(",
                "ISSUE2822_LEGACY_ENV",
                "\"ProgramFiles\"",
                "POWERSHELL_51_TAIL",
                "WRAPPER_LINES",
                "At line:2 char:1",
            ][..],
        ),
    ] {
        let at = steps
            .iter()
            .position(|s| s.contains(&format!("--test {test}")))
            .unwrap_or_else(|| {
                panic!("ci.yml の windows ジョブに `--test {test}` のステップが無い（#2822）")
            });
        let step = &steps[at];
        assert!(
            step.contains("--nocapture") && step.contains(krate),
            "#2822 の {test} のステップが結果の行（TAKO2822_*）をログへ残していない:\n{step}"
        );
        assert!(
            !step.contains("continue-on-error") && !step.contains("if:"),
            "#2822 の {test} のステップが非ブロッキング・条件つきに倒されている（外部に依らないので毎回走らせる）:\n{step}"
        );
        assert!(
            at < full,
            "#2822 の {test} のステップは全数より手前に置く（#1282 と同じ理由）"
        );
        let body = read(&format!(
            "crates/{}/tests/{test}.rs",
            krate.trim_start_matches("-p ")
        ));
        for needle in needles {
            assert!(
                body.contains(needle),
                "{test}.rs に `{needle}` が無い（#2822）"
            );
        }
    }
}

#[test]
fn issue2822_仕様が導入代行と実行ペインの形を書いている() {
    let spec = read(SPEC);
    for (fr, needles) in [
        (
            "| FR-2.33.1 |",
            &["#2822", "install.ps1", "PlainText", "TAKO_2822_LEGACY"][..],
        ),
        (
            "| FR-3.18.1 |",
            &["#2822", "$MyInvocation", "TAKO_2822_LEGACY"][..],
        ),
    ] {
        let start = spec
            .find(fr)
            .unwrap_or_else(|| panic!("{SPEC} に {fr} の行が無い"));
        let row = &spec[start..spec[start..].find('\n').map_or(spec.len(), |i| start + i)];
        let line = spec[..start].matches('\n').count() + 1;
        for needle in needles {
            assert!(
                row.contains(needle),
                "{SPEC}:{line}: {fr} に `{needle}` が無い（#2822 の導入代行の起動片 / 実行ペインの包み）"
            );
        }
    }
}
