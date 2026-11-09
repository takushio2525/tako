//! 受け入れゲートの述語が Windows で失敗したとき、結果の evidence に pwsh 7 の色の符号（ANSI の ESC 列）と
//! 5.1 の包みのスクリプトの行が載らないか（#2812）。**実際に powershell.exe / pwsh.exe を起こして測る**。
//!
//! - pwsh 7 はゲートの経路（`-EncodedCommand` + stderr の捕捉・`-OutputFormat` なし）でエラーの文を `$PSStyle` の
//!   色で飾ったまま CLIXML へ直列化する（`_x001B_`）。#2800 の平文化で ESC のバイトに戻っていたので、#2812 から
//!   同じ 1 実装（`platform::shell::powershell_stderr_text`）が CLIXML から戻した文字の色の符号を除く
//! - 5.1 の最上位の `Write-Error` は `$MyInvocation`（= ゲートの包み全体）を「コマンド」として本文の前に出していた。
//!   #2812 から包み（`platform::shell::powershell_output_script`）が 5.1 のときだけ `$MyInvocation` を外す
//!
//! 製品の口（`set_gate_payload_at` → `execute_gate_check_at` = `tako task gate check` / MCP
//! `tako_task_gate_check` と同じ関数）を通し、シェルは製品の解決（`default_shell` = pwsh 7 が在ればそれ、
//! 無ければ 5.1）に任せる。5.1 の回は `ProgramFiles` を pwsh 7 の無い置き場へ向けて解決を 5.1 へ倒す
//! （#2800 の検査と同じ）。修正前の形は同じバイナリの `TAKO_2812_LEGACY=1` で並べて取る = A/B。
//! #2800 の A/B（`TAKO_2800_LEGACY=1`）との組み合わせも 1 つの述語で並べて残す。env を書くので
//! `run_alone!` の子の中で走らせる（#2400）。
//!
//! CI は `.github/workflows/ci.yml` の windows ジョブが全数より手前に名指しで回す。結果の行（`TAKO2812_*`）を
//! `--nocapture` でログへ残す（ESC はログで見えないので `<ESC>` に置き換え、数を `esc=` に出す）。

#![cfg(windows)]

use std::path::Path;

use serde_json::{json, Value};
use tako_control::acceptance_gates::{
    execute_gate_check_at, set_gate_payload_at, ISSUE2800_LEGACY_ENV,
};
use tako_core::platform::shell::ISSUE2812_LEGACY_ENV;
use tako_core::test_residue::ScratchDir;

/// 1 本の述語と、その結果に読めるべき文字
struct Case {
    id: &'static str,
    cmd: &'static str,
    exit: i32,
    /// 新しい形の evidence に読めるべき文字（エラーの本文）
    needles: &'static [&'static str],
    /// 5.1 の新しい形に読めるべき利用者のコマンドの位置
    position_51: Option<&'static str>,
    /// pwsh 7 の新しい形に読めるべき利用者のコマンドの位置
    position_7: Option<&'static str>,
    /// 修正前の形で 5.1 に包みの行が出る（最上位の `Write-Error`）
    wrapper_51_before: bool,
    /// 修正前の形で pwsh 7 に色の符号が出る（ConciseView のエラーの文）
    ansi_7_before: bool,
    /// 塊の外の素のテキスト（`[Console]::Error` の直書き）の色の符号は残す = 新旧で evidence がバイト一致
    raw_unchanged: bool,
}

const CASES: &[Case] = &[
    Case {
        id: "cmdlet",
        cmd: "Get-Item -LiteralPath 'C:\\tako-2812-no-such\\missing.txt'",
        exit: 1,
        needles: &["Cannot find path 'C:\\tako-2812-no-such\\missing.txt'"],
        position_51: Some("At line:4 char:1\n+ Get-Item -LiteralPath"),
        position_7: Some("   4 |  Get-Item -LiteralPath"),
        wrapper_51_before: false,
        ansi_7_before: true,
        raw_unchanged: false,
    },
    Case {
        id: "two-errors",
        cmd: "Write-Error 'first error 2812'; Write-Error 'second error 2812'",
        exit: 1,
        needles: &["first error 2812", "second error 2812"],
        position_51: Some("At line:4 char:1\n+ Write-Error 'first error 2812'"),
        position_7: None,
        wrapper_51_before: true,
        ansi_7_before: true,
        raw_unchanged: false,
    },
    Case {
        id: "stop",
        cmd: "Write-Error 'stop 2812' -ErrorAction Stop",
        exit: 1,
        needles: &["stop 2812"],
        position_51: Some("At line:4 char:1\n+ Write-Error 'stop 2812' -ErrorAction Stop"),
        position_7: Some("   4 |  Write-Error 'stop 2812' -ErrorAction Stop"),
        wrapper_51_before: true,
        ansi_7_before: true,
        raw_unchanged: false,
    },
    Case {
        id: "throw",
        cmd: "throw 'tako gate failure 2812'",
        exit: 1,
        needles: &["tako gate failure 2812"],
        position_51: Some("At line:4 char:1\n+ throw 'tako gate failure 2812'"),
        position_7: Some("   4 |  throw 'tako gate failure 2812'"),
        wrapper_51_before: false,
        ansi_7_before: true,
        raw_unchanged: false,
    },
    Case {
        id: "raw-esc",
        cmd: "[Console]::Error.WriteLine([char]27 + '[33mraw 2812' + [char]27 + '[0m'); exit 4",
        exit: 4,
        needles: &["raw 2812"],
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        ansi_7_before: false,
        raw_unchanged: true,
    },
    Case {
        id: "silent",
        cmd: "cmd /c exit 5",
        exit: 5,
        needles: &[],
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        ansi_7_before: false,
        raw_unchanged: true,
    },
];

/// 包みのスクリプトの行（5.1 の修正前の形で本文の前に出ていたもの）
const WRAPPER_LINES: &[&str] = &[
    "[Console]::OutputEncoding",
    "$ProgressPreference",
    "$global:LASTEXITCODE = $null",
    "$__tako_ok = $?",
    "exit $__tako_code",
];

const POWERSHELL_51_TAIL: &str = "\\windowspowershell\\v1.0\\powershell.exe";

fn set_flag(name: &str, on: bool) {
    if on {
        std::env::set_var(name, "1");
    } else {
        std::env::remove_var(name);
    }
}

/// ゲートを 1 つ置いて回し、述語の id → (status, evidence) を返す（製品の口そのもの）
fn run_gate(dir: &Path, task: &str, cases: &[&Case]) -> Vec<(String, String, String)> {
    let path = dir.join(format!("acceptance_gates-{task}.yaml"));
    let criteria: Vec<Value> = cases
        .iter()
        .map(|c| json!({ "id": c.id, "kind": { "type": "command", "cmd": c.cmd } }))
        .collect();
    set_gate_payload_at(&path, task, &Value::Array(criteria).to_string(), None)
        .expect("ゲートを置けない");
    let result = execute_gate_check_at(&path, task, false).expect("ゲートを回せない");
    result["criteria"]
        .as_array()
        .expect("criteria")
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap_or_default().to_string(),
                c["status"].as_str().unwrap_or_default().to_string(),
                c["evidence"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn find<'a>(got: &'a [(String, String, String)], id: &str) -> &'a (String, String, String) {
    got.iter()
        .find(|(i, _, _)| i == id)
        .unwrap_or_else(|| panic!("述語 {id} の結果が無い: {got:?}"))
}

fn esc_count(s: &str) -> usize {
    s.matches('\u{1b}').count()
}

/// ログで読める形（ESC は `<ESC>`、改行は ` ⏎ `）
fn visible(s: &str) -> String {
    s.replace('\u{1b}', "<ESC>").replace('\n', " ⏎ ")
}

#[test]
fn issue2812_ゲートのevidenceに色の符号と包みの行が載らない() {
    if tako_core::run_alone!(issue2812_ゲートのevidenceに色の符号と包みの行が載らない)
    {
        return;
    }
    let original_pf = std::env::var_os("ProgramFiles");
    let no_pwsh = ScratchDir::new("i2812-no-pwsh");
    let dir = ScratchDir::new("i2812-gate");
    let all: Vec<&Case> = CASES.iter().collect();
    let mut ps51_program = String::new();

    for arm in ["ps51", "default"] {
        match arm {
            "ps51" => std::env::set_var("ProgramFiles", no_pwsh.as_os_str()),
            _ => match &original_pf {
                Some(v) => std::env::set_var("ProgramFiles", v),
                None => std::env::remove_var("ProgramFiles"),
            },
        }
        let program = tako_core::platform::shell::output_command("x")
            .get_program()
            .to_string_lossy()
            .into_owned();
        println!("TAKO2812_SHELL arm={arm} program={program}");
        if arm == "ps51" {
            assert!(
                program.to_ascii_lowercase().ends_with(POWERSHELL_51_TAIL),
                "pwsh 7 の無い置き場で 5.1 へ倒れない（{program}）"
            );
            ps51_program = program.clone();
        } else if program == ps51_program {
            panic!(
                "pwsh 7 が無い（既定の解決が 5.1 と同じ）。#2812 の pwsh 7 の色の符号を測れない"
            );
        }
        let is_51 = arm == "ps51";

        set_flag(ISSUE2800_LEGACY_ENV, false);
        set_flag(ISSUE2812_LEGACY_ENV, false);
        let new = run_gate(&dir, &format!("{arm}-new"), &all);
        set_flag(ISSUE2812_LEGACY_ENV, true);
        let old = run_gate(&dir, &format!("{arm}-legacy"), &all);
        set_flag(ISSUE2812_LEGACY_ENV, false);

        for case in CASES {
            let (_, status, evidence) = find(&new, case.id);
            let (_, old_status, old_evidence) = find(&old, case.id);
            println!(
                "TAKO2812_ESC arm={arm} case={} new={} legacy={}",
                case.id,
                esc_count(evidence),
                esc_count(old_evidence)
            );
            println!(
                "TAKO2812_EVIDENCE arm={arm} case={} status={status} [{}]",
                case.id,
                visible(evidence)
            );
            println!(
                "TAKO2812_EVIDENCE_LEGACY arm={arm} case={} status={old_status} [{}]",
                case.id,
                visible(old_evidence)
            );
            // 合否と終了コードは #2812 の前後で変わらない（判定は終了コードで決まる）
            assert_eq!(status, "failed", "{arm} {}: {evidence}", case.id);
            assert_eq!(status, old_status, "{arm} {}", case.id);
            let head = format!("exit {}", case.exit);
            for e in [evidence, old_evidence] {
                assert!(
                    e == &head || e.starts_with(&format!("{head}; ")),
                    "{arm} {}: 終了コードが違う: {e}",
                    case.id
                );
            }
            for needle in case.needles {
                assert!(
                    evidence.contains(needle),
                    "{arm} {}: エラーの本文（{needle}）が消えた（#2812）: {evidence}",
                    case.id
                );
            }
            if case.raw_unchanged {
                // 塊の外の素のテキストは 1 バイトも変えない（色の符号も残す）
                assert_eq!(
                    evidence, old_evidence,
                    "{arm} {}: 素の出力が変わった",
                    case.id
                );
                continue;
            }
            // 受け入れ条件 1: ESC（0x1B）が 1 バイトも残らない
            assert_eq!(
                esc_count(evidence),
                0,
                "{arm} {}: evidence に色の符号（ESC）が残っている（#2812）: {}",
                case.id,
                visible(evidence)
            );
            // 受け入れ条件 2: 包みのスクリプトの行が載らない
            for line in WRAPPER_LINES {
                assert!(
                    !evidence.contains(line),
                    "{arm} {}: 包みのスクリプトの行（{line}）が evidence に出ている（#2812）: {evidence}",
                    case.id
                );
            }
            // 受け入れ条件 4: 利用者のコマンドの位置は残る
            let position = if is_51 {
                case.position_51
            } else {
                case.position_7
            };
            if let Some(position) = position {
                assert!(
                    evidence.contains(position),
                    "{arm} {}: 利用者のコマンドの位置（{position:?}）が消えた（#2812）: {evidence}",
                    case.id
                );
            }
            // A/B: 修正前の形は症状が出る（前提が崩れていないことの確かめ）
            if is_51 && case.wrapper_51_before {
                assert!(
                    old_evidence.contains("$__tako_ok = $?"),
                    "{arm} {}: 修正前の形で包みの行が出ない（前提が崩れた）: {old_evidence}",
                    case.id
                );
            }
            if !is_51 && case.ansi_7_before {
                assert!(
                    esc_count(old_evidence) > 0,
                    "{arm} {}: 修正前の形で色の符号が出ない（前提が崩れた）: {old_evidence}",
                    case.id
                );
            }
        }

        // #2800 の A/B との組み合わせ（最上位の Write-Error の 1 本で 4 通りを並べて残す）
        let two = CASES
            .iter()
            .find(|c| c.id == "two-errors")
            .expect("two-errors");
        for (l2800, l2812) in [(false, false), (false, true), (true, false), (true, true)] {
            set_flag(ISSUE2800_LEGACY_ENV, l2800);
            set_flag(ISSUE2812_LEGACY_ENV, l2812);
            let got = run_gate(&dir, &format!("{arm}-mix-{l2800}-{l2812}"), &[two]);
            set_flag(ISSUE2800_LEGACY_ENV, false);
            set_flag(ISSUE2812_LEGACY_ENV, false);
            let (_, _, e) = find(&got, two.id);
            println!(
                "TAKO2812_MIX arm={arm} legacy2800={l2800} legacy2812={l2812} esc={} clixml={} wrapper={} [{}]",
                esc_count(e),
                e.contains("#< CLIXML"),
                e.contains("$__tako_ok"),
                visible(&e.chars().take(240).collect::<String>())
            );
            // 平文化を切ると XML のまま（ESC のバイトは無い）。包みは #2812 の門だけが決める（5.1）
            assert_eq!(e.contains("#< CLIXML"), l2800, "{arm} {l2800} {l2812}: {e}");
            if is_51 {
                assert_eq!(
                    e.contains("$__tako_ok"),
                    l2812,
                    "{arm} {l2800} {l2812}: {e}"
                );
            }
            if l2800 || !l2812 {
                assert_eq!(esc_count(e), 0, "{arm} {l2800} {l2812}: {}", visible(e));
            }
        }
    }
}
