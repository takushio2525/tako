//! 実行ペインと `split --command` の失敗したときだけ止める形で、利用者のコマンドが最上位の終端エラーになっても
//! 終了コード・案内・Enter 待ちが出る形（#2827）が戻らないことの番犬。
//!
//! 単体テスト（`platform::shell::tests_2827`）は答え（字面）を縛る。ここは**配線と 1 実装**を見る:
//!
//! 1. 実行ペインの包み（#1657 の形）の入口は `legacy_2827()` を本体へ渡し、本体は 1 行目の先頭に
//!    `powershell_unfinished_hook` の口を置いて最後に `exit $__tako_code` で抜ける。argv は `powershell_pane_args`
//!    （口で始まるときだけ `-NoExit`）から組む
//! 2. 失敗したときだけ止める形も同じ口と argv を通り、5.1 の片（#2822 と同じ）を 1 行目の末尾へ足す。
//!    Windows の入口（`imp::hold_on_failure_command`）は `legacy_2827()` を渡す
//! 3. 口は `prompt` の差し替えで、抜けるのは `finally` の `[Environment]::Exit`（`prompt` の中の `exit` は効かない）。
//!    `-NoExit` は口の字面から決める（口と食い違わない）
//! 4. **利用者のコマンドを `try` / `trap` で囲まない**（文だけを止めるエラーの後ろが走らなくなる = 意味が変わる）
//! 5. 受け入れゲートの経路（PTY 無しの包み）は口も `-NoExit` も持たない
//! 6. Windows の CI が実物の 5.1 / pwsh 7 の ConPTY で新旧を並べる検査を、全数より手前に名指しで回す
//! 7. 仕様（`.agent/requirements.md` の FR-3.18.1 / FR-2.5.14）が #2827 の形を書いている

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
const SPEC: &str = ".agent/requirements.md";
const PANE_TEST: &str = "issue2827_run_pane_terminating_windows";

/// `fn <name>(` から次の最上位の `}` までの本文と、その始まりの行番号（1 始まり）
fn fn_body<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let head = format!("fn {name}(");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に fn {name} が無い（#2827）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 2).unwrap_or(rest.len());
    let line = src[..start].matches('\n').count() + 1;
    (&rest[..end], line)
}

/// `    pub(crate) fn <name>(` から字下げの閉じ `    }` までの本文（`mod imp` の中の関数）と行番号
fn imp_fn_body<'a>(src: &'a str, rel: &str, after: &str, name: &str) -> (&'a str, usize) {
    let from = src
        .find(after)
        .unwrap_or_else(|| panic!("{rel} に {after} が無い（#2827）"));
    let head = format!("    pub(crate) fn {name}(");
    let start = from
        + src[from..]
            .find(&head)
            .unwrap_or_else(|| panic!("{rel} の {after} に fn {name} が無い（#2827）"));
    let rest = &src[start..];
    let end = rest.find("\n    }\n").map(|i| i + 6).unwrap_or(rest.len());
    (&rest[..end], src[..start].matches('\n').count() + 1)
}

/// `const <name>: &str =` の値（次の `";` + 改行まで）と、その行番号
fn const_value<'a>(src: &'a str, rel: &str, name: &str) -> (&'a str, usize) {
    let head = format!("const {name}: &str =");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に const {name} が無い（#2827）"));
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
fn issue2827_実行ペインの包みは1行目の先頭に口を置き最後にexitで抜ける() {
    let src = read(SHELL);
    let (entry, entry_line) = fn_body(&src, SHELL, "powershell_run_pane_script");
    assert!(
        entry.contains("powershell_run_pane_script_in(") && entry.contains("legacy_2827(),"),
        "{SHELL}:{entry_line}: powershell_run_pane_script が本体へ `legacy_2827()` を渡していない（#2827）"
    );
    let (body, line) = fn_body(&src, SHELL, "powershell_run_pane_script_in");
    for needle in [
        "powershell_unfinished_hook(&report, &hint, &cleanup)",
        "if legacy_2827 {",
        "{body}exit $__tako_code\\n",
    ] {
        assert!(
            body.contains(needle),
            "{SHELL}:{line}: powershell_run_pane_script_in に `{needle}` が無い = 最上位の終端エラーで包みごと止まり、\
             終了コード・案内・Enter 待ちが出ない（#2827）"
        );
    }
    // 口は 1 行目の先頭（利用者のコマンドより前・同じ 1 行目 = 行番号を変えない）: 書式の最初の `{}` が口で、
    // その後ろに #2827 前の包み（終了コードの片から始まる）がそのまま続く
    let squashed: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        squashed.contains(
            "\"{}{body}exit$__tako_code\\n\",powershell_unfinished_hook(&report,&hint,&cleanup)"
        ) && body.contains("powershell_pane_exit_code_script(command, legacy_2822)"),
        "{SHELL}:{}: 口が包みの先頭に置かれていない（#2827）",
        line_in(body, line, "powershell_unfinished_hook(")
    );
    let (argv, argv_line) = fn_body(&src, SHELL, "powershell_run_pane_command_1657");
    assert!(
        argv.contains("powershell_pane_args(&powershell_run_pane_script("),
        "{SHELL}:{argv_line}: 実行ペインの argv が powershell_pane_args（口で始まるときだけ -NoExit）を通っていない（#2827）"
    );
}

#[test]
fn issue2827_失敗したときだけ止める形も同じ口とargvを通る() {
    let src = read(SHELL);
    let (body, line) = fn_body(&src, SHELL, "powershell_hold_on_failure_command");
    for needle in [
        "legacy_2827: bool",
        "powershell_unfinished_hook(&report, &hint, &cleanup)",
        "powershell_pane_exit_code_script(&invocation, false)",
        "args: powershell_pane_args(&script)",
    ] {
        assert!(
            body.contains(needle),
            "{SHELL}:{line}: powershell_hold_on_failure_command に `{needle}` が無い（#2827。argv がこのプロセスの中で\
             終端エラーになるとペインが黙って消える = #1031 の症状 / 5.1 の包みの行）"
        );
    }
    let (imp, imp_line) = imp_fn_body(
        &src,
        SHELL,
        "#[cfg(windows)]\nmod imp {",
        "hold_on_failure_command",
    );
    assert!(
        imp.contains("super::legacy_2827(),"),
        "{SHELL}:{imp_line}: Windows の hold_on_failure_command が `legacy_2827()` を渡していない（#2827）"
    );
}

#[test]
fn issue2827_口はpromptの差し替えでfinallyのenvironment_exitで抜ける() {
    let src = read(SHELL);
    let (hook, line) = fn_body(&src, SHELL, "powershell_unfinished_hook");
    for needle in [
        "POWERSHELL_UNFINISHED_HEAD",
        "if ($null -eq $__tako_code)",
        "POWERSHELL_UNFINISHED_CODE",
        "} finally {",
        "POWERSHELL_UNFINISHED_EXIT",
        "$__tako_code = $null; ",
    ] {
        assert!(
            hook.contains(needle),
            "{SHELL}:{line}: powershell_unfinished_hook に `{needle}` が無い（#2827）"
        );
    }
    assert!(
        !hook.contains("exit $"),
        "{SHELL}:{}: 口の中で `exit` を使っている（prompt の中の exit はホストに飲まれ、対話の PS> に落ちる。#2827）",
        line_in(hook, line, "exit $")
    );
    let (head, head_line) = const_value(&src, SHELL, "POWERSHELL_UNFINISHED_HEAD");
    assert!(
        head.contains("\"function global:prompt { \""),
        "{SHELL}:{head_line}: 口が prompt の差し替えでない（#2827）"
    );
    let (exit, exit_line) = const_value(&src, SHELL, "POWERSHELL_UNFINISHED_EXIT");
    assert!(
        exit.contains("[Environment]::Exit($__tako_code)") && exit.contains("Stop-Process -Id $PID"),
        "{SHELL}:{exit_line}: 口から抜ける片が [Environment]::Exit（制約付き言語モードでは Stop-Process）でない（#2827）"
    );
    let (args, args_line) = fn_body(&src, SHELL, "powershell_pane_args");
    assert!(
        args.contains("script.starts_with(POWERSHELL_UNFINISHED_HEAD)") && args.contains("\"-NoExit\""),
        "{SHELL}:{args_line}: -NoExit を口の字面から決めていない（口と -NoExit が食い違うと対話に落ちる / 口が呼ばれない。#2827）"
    );
}

#[test]
fn issue2827_利用者のコマンドをtryやtrapで囲まない() {
    let src = read(SHELL);
    // 実行ペイン・失敗したときだけ止める形・共有の片・口のどれにも、利用者のコマンドを囲む try / trap を書かない
    for name in [
        "powershell_run_pane_script_in",
        "powershell_hold_on_failure_command",
        "powershell_exit_code_script_in",
        "powershell_pane_exit_code_script",
        "powershell_unfinished_hook",
    ] {
        let (body, line) = fn_body(&src, SHELL, name);
        for forbidden in [
            "trap {",
            "trap [",
            "try {{\\n{command}",
            "try {{\\n{invocation}",
        ] {
            assert!(
                !body.contains(forbidden),
                "{SHELL}:{}: {name} が利用者のコマンドを `{forbidden}` で囲んでいる。try / trap があると文だけを止める\
                 エラー（[int]::Parse('x') やコマンドの打ち間違い）が囲いまで巻き上がり、後ろの文が走らなくなる（#2827）",
                line_in(body, line, forbidden)
            );
        }
    }
}

#[test]
fn issue2827_受け入れゲートの経路は口もnoexitも持たない() {
    let src = read(SHELL);
    for name in ["powershell_output_argv", "powershell_output_script_in"] {
        let (body, line) = fn_body(&src, SHELL, name);
        for forbidden in [
            "powershell_unfinished_hook",
            "powershell_pane_args",
            "-NoExit",
        ] {
            assert!(
                !body.contains(forbidden),
                "{SHELL}:{}: 受け入れゲートの経路（{name}）が `{forbidden}` を使っている（ゲートは終わったら親へ終了コードを\
                 返す。対話に残ると判定が止まる。#2827）",
                line_in(body, line, forbidden)
            );
        }
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
fn issue2827_windowsのciが実物の51とpwsh7のconptyで新旧を並べる検査を名指しで回す() {
    let steps = windows_steps();
    let full = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("windows ジョブに全数のテストのステップが無い");
    let at = steps
        .iter()
        .position(|s| s.contains(&format!("--test {PANE_TEST}")))
        .unwrap_or_else(|| {
            panic!("ci.yml の windows ジョブに `--test {PANE_TEST}` のステップが無い（#2827）")
        });
    let step = &steps[at];
    assert!(
        step.contains("--nocapture") && step.contains("-p tako-core"),
        "#2827 の {PANE_TEST} のステップが結果の行（TAKO2827_*）をログへ残していない:\n{step}"
    );
    assert!(
        !step.contains("continue-on-error") && !step.contains("if:"),
        "#2827 の {PANE_TEST} のステップが非ブロッキング・条件つきに倒されている（外部に依らないので毎回走らせる）:\n{step}"
    );
    assert!(
        at < full,
        "#2827 の {PANE_TEST} のステップは全数より手前に置く（#1282 と同じ理由）"
    );
    let body = read(&format!("crates/tako-core/tests/{PANE_TEST}.rs"));
    for needle in [
        "#![cfg(windows)]",
        "tako_core::run_alone!(",
        "run_pane_command(",
        "hold_on_failure_command(",
        "ISSUE2827_LEGACY_ENV",
        "\"ProgramFiles\"",
        "POWERSHELL_51_TAIL",
        "WRAPPER_LINES",
        "\"continue\"",
        "\"nested\"",
        "\"ctrl-c\"",
        "\"hold-write-error\"",
    ] {
        assert!(
            body.contains(needle),
            "{PANE_TEST}.rs に `{needle}` が無い（#2827）"
        );
    }
}

#[test]
fn issue2827_仕様が実行ペインと失敗時だけ止める形の口を書いている() {
    let spec = read(SPEC);
    for (fr, needles) in [
        (
            "| FR-3.18.1 |",
            &["#2827", "-NoExit", "prompt", "TAKO_2827_LEGACY", "Ctrl+C"][..],
        ),
        ("| FR-2.5.14 |", &["#2827", "TAKO_2827_LEGACY"][..]),
    ] {
        let start = spec
            .find(fr)
            .unwrap_or_else(|| panic!("{SPEC} に {fr} の行が無い"));
        let row = &spec[start..spec[start..].find('\n').map_or(spec.len(), |i| start + i)];
        let line = spec[..start].matches('\n').count() + 1;
        for needle in needles {
            assert!(
                row.contains(needle),
                "{SPEC}:{line}: {fr} に `{needle}` が無い（#2827 の実行ペイン / 失敗したときだけ止める形の口）"
            );
        }
    }
}
