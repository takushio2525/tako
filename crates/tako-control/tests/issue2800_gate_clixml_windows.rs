//! 受け入れゲートの述語が Windows PowerShell 5.1 で失敗したとき、結果の stderr が CLIXML のままにならないか
//! （#2800）。**実際に powershell.exe を起こして測る**。
//!
//! ゲートの述語は境界 B1（`platform::shell::output_command`）が PowerShell へ `-EncodedCommand` で渡す。
//! 5.1 は stderr を捕捉されると cmdlet のエラーレコードを CLIXML（`#< CLIXML` + `<Objs …>`）で書く
//! （`-OutputFormat Text` も見ない = #2790 で実測）。ゲートの argv はその指定も付けないので pwsh 7 も同じく
//! CLIXML で書く（この検査の初回の CI で実測）。#2800 からは `acceptance_gates::format_evidence` が
//! 導入代行と同じ 1 実装（`platform::shell::powershell_stderr_text`）で平文へ戻してから evidence へ載せる。
//!
//! 製品の口（`set_gate_payload_at` → `execute_gate_check_at` = `tako task gate check` / MCP
//! `tako_task_gate_check` と同じ関数）を通し、シェルは製品の解決（`default_shell` = pwsh 7 が在ればそれ、
//! 無ければ 5.1）に任せる。5.1 の回は `ProgramFiles` を pwsh 7 の無い置き場へ向けて解決を 5.1 へ倒す
//! （pwsh 7 を入れていない PC と同じ）。env を書くので `run_alone!` の子の中で走らせる（#2400）。
//! 修正前の形は同じバイナリの `TAKO_2800_LEGACY=1`（捕捉したまま載せる）で並べて取る = A/B。
//!
//! CI は `.github/workflows/ci.yml` の windows ジョブが全数より手前に名指しで回す。結果の行（`TAKO2800_*`）を
//! `--nocapture` でログへ残す。

#![cfg(windows)]

use std::path::Path;

use serde_json::{json, Value};
use tako_control::acceptance_gates::{
    execute_gate_check_at, set_gate_payload_at, ISSUE2800_LEGACY_ENV,
};
use tako_core::test_residue::ScratchDir;

/// 1 本の述語と、その結果に読めるべき文字
struct Case {
    id: &'static str,
    cmd: &'static str,
    /// 終了コード（evidence の先頭の `exit N`）
    exit: i32,
    /// 5.1 の修正前の形で stderr が CLIXML になる（cmdlet のエラーレコード）。`false` はネイティブコマンド
    clixml: bool,
    /// 平文に戻した evidence に読めるべき文字（シェルに依らないもの）
    needles: &'static [&'static str],
    /// 5.1 の平文に読めるべき文字（エラーレコードの先頭の本文の行 = 末尾で切ると落ちる行）
    needles_51: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        id: "cmdlet",
        cmd: "Get-Item -LiteralPath 'C:\\tako-2800-no-such\\missing.txt'",
        exit: 1,
        clixml: true,
        needles: &["tako-2800-no-such\\missing.txt"],
        // NormalView の本文の行は `<コマンド> : <本文>`（CategoryInfo の行は `[Get-Item]`）
        needles_51: &["Get-Item : "],
    },
    Case {
        id: "throw",
        cmd: "throw 'tako gate failure 2800'",
        exit: 1,
        clixml: true,
        needles: &["tako gate failure 2800"],
        needles_51: &["tako gate failure 2800\n"],
    },
    Case {
        id: "two-errors",
        cmd: "Write-Error 'first error 2800'; Write-Error 'second error 2800'",
        exit: 1,
        clixml: true,
        // Write-Error の本文は本文の行にしか出ない（CategoryInfo は `NotSpecified: (:)`）
        needles: &["first error 2800", "second error 2800"],
        needles_51: &[],
    },
    Case {
        id: "host-then-error",
        cmd: "Write-Host 'host line 2800'; Write-Error 'after host 2800'",
        exit: 1,
        clixml: true,
        needles: &["after host 2800"],
        needles_51: &[],
    },
    Case {
        id: "native",
        cmd: "cmd /c \"echo native stderr 2800 1>&2 & exit 3\"",
        exit: 3,
        clixml: false,
        needles: &["native stderr 2800"],
        needles_51: &[],
    },
    Case {
        id: "silent",
        cmd: "cmd /c exit 5",
        exit: 5,
        clixml: false,
        needles: &[],
        needles_51: &[],
    },
];

const POWERSHELL_51_TAIL: &str = "\\windowspowershell\\v1.0\\powershell.exe";

/// ゲートを 1 つ置いて回し、述語の id → evidence を返す（製品の口そのもの）
fn run_gate(dir: &Path, task: &str) -> Vec<(String, String, String)> {
    let path = dir.join(format!("acceptance_gates-{task}.yaml"));
    let criteria: Vec<Value> = CASES
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

#[test]
fn issue2800_ゲートの述語の51の失敗はstderrが平文で読める() {
    if tako_core::run_alone!(issue2800_ゲートの述語の51の失敗はstderrが平文で読める)
    {
        return;
    }
    let original_pf = std::env::var_os("ProgramFiles");
    // pwsh 7 の無い置き場（`<ProgramFiles>\PowerShell\7\pwsh.exe` が在らない = 解決が 5.1 へ倒れる）
    let no_pwsh = ScratchDir::new("i2800-no-pwsh");
    let dir = ScratchDir::new("i2800-gate");
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
        println!("TAKO2800_SHELL arm={arm} program={program}");
        if arm == "ps51" {
            assert!(
                program.to_ascii_lowercase().ends_with(POWERSHELL_51_TAIL),
                "pwsh 7 の無い置き場で 5.1 へ倒れない（{program}）"
            );
            ps51_program = program.clone();
        } else if program == ps51_program {
            println!("TAKO2800_SKIP arm=default pwsh 7 が無い（5.1 の回と同じシェル）");
            continue;
        }
        let is_51 = arm == "ps51";

        std::env::remove_var(ISSUE2800_LEGACY_ENV);
        let new = run_gate(&dir, &format!("{arm}-new"));
        std::env::set_var(ISSUE2800_LEGACY_ENV, "1");
        let old = run_gate(&dir, &format!("{arm}-legacy"));
        std::env::remove_var(ISSUE2800_LEGACY_ENV);

        for case in CASES {
            let (_, status, evidence) = find(&new, case.id);
            let (_, old_status, old_evidence) = find(&old, case.id);
            println!(
                "TAKO2800_EVIDENCE arm={arm} case={} status={status} [{evidence}]",
                case.id
            );
            println!(
                "TAKO2800_EVIDENCE_LEGACY arm={arm} case={} status={old_status} [{old_evidence}]",
                case.id
            );
            // 合否は平文化の前後で変わらない（終了コードで決まる）
            assert_eq!(status, "failed", "{arm} {}: {evidence}", case.id);
            assert_eq!(status, old_status, "{arm} {}", case.id);
            let head = format!("exit {}", case.exit);
            assert!(
                evidence == &head || evidence.starts_with(&format!("{head}; ")),
                "{arm} {}: 終了コードが違う: {evidence}",
                case.id
            );
            for junk in ["#< CLIXML", "<Objs", "<S S=", "_x000D_"] {
                assert!(
                    !evidence.contains(junk),
                    "{arm} {}: 結果の stderr に {junk} が残っている（#2800）: {evidence}",
                    case.id
                );
            }
            for needle in case.needles {
                assert!(
                    evidence.contains(needle),
                    "{arm} {}: 元のエラーの文（{needle}）が読めない: {evidence}",
                    case.id
                );
            }
            if is_51 {
                for needle in case.needles_51 {
                    assert!(
                        evidence.contains(needle),
                        "{arm} {}: エラーレコードの本文の行（{needle:?}）が落ちた: {evidence}",
                        case.id
                    );
                }
            }
            // A/B: 修正前の形は cmdlet のエラーを CLIXML のまま載せていた（#2800 の症状）。ゲートの argv は
            // `-OutputFormat Text` を付けないので 5.1 だけでなく pwsh 7 も CLIXML で書く（#2800 の CI で実測）
            if case.clixml {
                assert!(
                    old_evidence.contains("#< CLIXML"),
                    "{arm} {}: 修正前の形で CLIXML が出ない（前提が崩れた）: {old_evidence}",
                    case.id
                );
            }
            if !case.clixml {
                // ネイティブコマンドの素の stderr は #2800 前と 1 バイトも変えない
                assert_eq!(
                    evidence, old_evidence,
                    "{arm} {}: CLIXML でない出力が変わった",
                    case.id
                );
            }
        }
        // Write-Host の文字: 修正前の evidence に読めていたものは平文化した後も読める（情報を落とさない）
        let (_, _, host_new) = find(&new, "host-then-error");
        let (_, _, host_old) = find(&old, "host-then-error");
        let (in_new, in_old) = (
            host_new.contains("host line 2800"),
            host_old.contains("host line 2800"),
        );
        println!("TAKO2800_HOST arm={arm} in_new={in_new} in_legacy={in_old}");
        assert!(
            in_new || !in_old,
            "{arm}: Write-Host の文字が平文化で落ちた（修正前は読めた）: {host_new}"
        );
    }
}
