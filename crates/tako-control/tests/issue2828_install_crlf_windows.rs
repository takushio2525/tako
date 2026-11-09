//! エージェント CLI の導入代行（`tako setup bootstrap install` = MCP `tako_setup_bootstrap`）が Windows で失敗したとき、
//! 断りの文（と成功したときの応答の output）に CR（0x0D）が残らないか（#2828）。**実際に powershell.exe / pwsh.exe を
//! 起こして測る**。
//!
//! PowerShell は捕捉された stdout / stderr へ改行を CRLF で書く。#2790 の平文化（`powershell_stderr_text`）が LF へ
//! 揃えるのは CLIXML を戻したときだけなので、pwsh 7 の平文の stderr と stdout の CR が文へ残り、CLI / MCP / 設定画面へ
//! 返る文の行が CR で割れていた。#2828 から `run_installer` は stdout と平文化した stderr を
//! `platform::shell::powershell_captured_text`（インタプリタが PowerShell のときだけ CRLF と単独の CR を LF へ）に通す。
//!
//! 製品の口（`setup_bootstrap::run_installer` = CLI と MCP がどちらも `install_for_typed` から通る 1 実装）へ偽の
//! `install.ps1` を渡し、返った断り（`SetupRefusal::InstallerFailed`）の文を日英で読む。インタプリタは製品の解決
//! （`powershell` → `pwsh`）に任せ、pwsh 7 の回は PATH から WindowsPowerShell の置き場を外して倒す（#2822 と同じ）。
//! 修正前の形は同じバイナリの `TAKO_2828_LEGACY=1` で並べて取る = A/B。env を書くので `run_alone!` の子の中で
//! 走らせる（#2400）。偽物はテストごとの置き場へ書く（#2659）。
//!
//! CI は `.github/workflows/ci.yml` の windows ジョブが全数より手前に名指しで回す。結果の行（`TAKO2828_*`）を
//! `--nocapture` でログへ残す（CR はログで見えないので `<CR>` に置き換え、数を `cr=` に出す）。

#![cfg(windows)]

use std::path::{Path, PathBuf};

use tako_control::setup_bootstrap::run_installer;
use tako_control::setup_refusal::SetupRefusal;
use tako_core::i18n::Lang;
use tako_core::platform::agent_install::{recipe, AgentKind};
use tako_core::platform::shell::{newlines_to_lf, ISSUE2828_LEGACY_ENV};
use tako_core::platform::support::Platform;
use tako_core::test_residue::ScratchDir;

/// 1 本の偽のインストーラと、その結果に読めるべきもの
struct Case {
    id: &'static str,
    /// 冒頭（`param` / `Set-StrictMode`）の後ろに続ける本文
    body: &'static str,
    exit: i32,
    /// 新しい形の文に読めるべき本文（空白を落として探す。5.1 は捕捉した stderr を画面の幅で折り返す）
    needles: &'static [&'static str],
    /// 修正前の形の文に CR が残る（症状が出る前提。5.1, pwsh 7 の順）。5.1 は `Write-Host` の行も host のエラーも CLIXML で
    /// stderr へ書き、平文化（`powershell_stderr_text`）が CLIXML を戻すときに stderr 全体の CRLF を LF にするので、
    /// CR が残るのは stdout へ直に書いた行（`Write-Output` / `[Console]::Out`）と単独の CR だけ
    cr_before: (bool, bool),
}

const CASES: &[Case] = &[
    // host へ書いた行（stdout）と最上位の Write-Error（pwsh 7 は平文の stderr、5.1 は CLIXML）
    Case {
        id: "lines-and-error",
        body: "Write-Host 'line one 2828'\nWrite-Host 'line two 2828'\nWrite-Error 'fake error 2828'\nexit 3\n",
        exit: 3,
        needles: &["line one 2828", "line two 2828", "fake error 2828"],
        cr_before: (false, true),
    },
    // 捕まえない終端エラー（起動片が `[Console]::Error.WriteLine(($_ | Out-String).TrimEnd())` で書く複数行）
    Case {
        id: "throw",
        body: "Write-Host 'before throw 2828'\n$ErrorActionPreference = 'Stop'\nthrow 'fake throw 2828'\n",
        exit: 1,
        needles: &["before throw 2828", "fake throw 2828"],
        cr_before: (false, true),
    },
    // 単独の CR（進捗の上書き）と本文の途中の CRLF を、host を通さずに直接書く。どちらの断片も消えない
    Case {
        id: "lone-cr",
        body: "[Console]::Out.Write(\"progress 10% 2828`rprogress 100% 2828`r`nfirst half 2828`r`nsecond half 2828`r`n\")\n[Console]::Error.Write(\"err part one 2828`rerr part two 2828`r`n\")\nexit 4\n",
        exit: 4,
        needles: &[
            "progress 10% 2828",
            "progress 100% 2828",
            "first half 2828",
            "second half 2828",
            "err part one 2828",
            "err part two 2828",
        ],
        cr_before: (true, true),
    },
    // 何も出さずに非 0（境界: 断りの文に添える出力が空 = 新旧でバイト一致）
    Case {
        id: "silent",
        body: "exit 7\n",
        exit: 7,
        needles: &[],
        cr_before: (false, false),
    },
    // 成功。断りにならず、応答の output に CR が載らない
    Case {
        id: "success",
        body: "Write-Host 'ok 2828'\nWrite-Output 'done 2828'\nWrite-Output 'third 2828'\n",
        exit: 0,
        needles: &["ok 2828", "done 2828", "third 2828"],
        cr_before: (true, true),
    },
];

const POWERSHELL_51_TAIL: &str = "\\windowspowershell\\v1.0\\powershell.exe";

fn set_flag(name: &str, on: bool) {
    if on {
        std::env::set_var(name, "1");
    } else {
        std::env::remove_var(name);
    }
}

/// PATH から WindowsPowerShell の置き場を外す（製品の解決 `powershell` → `pwsh` を pwsh 7 へ倒す）
fn path_without_windows_powershell(path: &std::ffi::OsStr) -> String {
    path.to_string_lossy()
        .split(';')
        .filter(|e| !e.to_ascii_lowercase().contains("windowspowershell"))
        .collect::<Vec<_>>()
        .join(";")
}

/// 偽の `install.ps1` を書く（BOM なし UTF-8。冒頭は本物と同じ `param()` → `Set-StrictMode -Version Latest`）
fn write_fake(dir: &Path, arm: &str, case: &Case) -> PathBuf {
    let sub = dir.join(format!("{arm}-{}", case.id));
    std::fs::create_dir_all(&sub).expect("置き場を作れない");
    let script = sub.join("tako-codex-install-2828.ps1");
    let text = format!(
        "param([string]$Target = \"latest\")\nSet-StrictMode -Version Latest\n# 偽の install.ps1（#2828）\n{}",
        case.body
    );
    std::fs::write(&script, text.as_bytes()).expect("偽のインストーラを書けない");
    script
}

/// 製品の口で 1 回走らせた結果（成功なら `code = 0` で `log` は応答の output）
struct Ran {
    code: i32,
    log: String,
    /// 利用者・AI が読む文（CLI = 日本語の `into_ja`・MCP = 表示言語の `text_in`）
    text_ja: String,
    text_en: String,
}

fn run_case(script: &Path) -> Ran {
    match run_installer(AgentKind::Codex, script, false) {
        Ok(log) => Ran {
            code: 0,
            text_ja: log.clone(),
            text_en: log.clone(),
            log,
        },
        Err(refusal) => {
            let text_ja = refusal.text_in(Lang::Ja);
            let text_en = refusal.text_in(Lang::En);
            match refusal {
                SetupRefusal::InstallerFailed { code, log, .. } => Ran {
                    code,
                    log,
                    text_ja,
                    text_en,
                },
                other => panic!("InstallerFailed 以外の断り: {}", other.into_ja()),
            }
        }
    }
}

fn cr_count(s: &str) -> usize {
    s.matches('\r').count()
}

/// ログで読める形（CR は `<CR>`、改行は ` ⏎ `）。長いものは頭だけ
fn visible(s: &str) -> String {
    let v = s.replace('\r', "<CR>").replace('\n', " ⏎ ");
    if v.chars().count() > 700 {
        format!(
            "{} …({} 字)",
            v.chars().take(700).collect::<String>(),
            s.chars().count()
        )
    } else {
        v
    }
}

/// 空白（改行を含む）を落として探す（5.1 は捕捉した stderr のエラーを画面の幅で折り返す）
fn contains_loose(haystack: &str, needle: &str) -> bool {
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    squash(haystack).contains(&squash(needle))
}

#[test]
fn issue2828_導入代行の文にcrが残らず本文は消えない() {
    if tako_core::run_alone!(issue2828_導入代行の文にcrが残らず本文は消えない) {
        return;
    }
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let dir = ScratchDir::new("i2828-install");
    let codex = recipe(Platform::Windows, AgentKind::Codex);
    // 1 回の CI で全ケースの事実を残すため、失敗は集めて最後に一覧で落とす
    let mut fails: Vec<String> = Vec::new();
    macro_rules! check {
        ($cond:expr, $($fmt:tt)+) => {
            if !$cond {
                fails.push(format!($($fmt)+));
            }
        };
    }

    for arm in ["ps51", "pwsh7"] {
        match arm {
            "ps51" => std::env::set_var("PATH", &original_path),
            _ => std::env::set_var("PATH", path_without_windows_powershell(&original_path)),
        }
        let program = codex
            .installer_command(Path::new("probe.ps1"))
            .get_program()
            .to_string_lossy()
            .into_owned();
        println!("TAKO2828_SHELL arm={arm} program={program}");
        let lower = program.to_ascii_lowercase();
        if arm == "ps51" {
            assert!(
                lower.ends_with(POWERSHELL_51_TAIL),
                "製品の既定の解決が 5.1 でない（{program}）"
            );
        } else {
            assert!(
                lower.ends_with("\\pwsh.exe"),
                "WindowsPowerShell を PATH から外しても pwsh 7 へ倒れない（{program}）。#2828 の pwsh 7 を測れない"
            );
        }

        let is_51 = arm == "ps51";
        for case in CASES {
            let script = write_fake(&dir, arm, case);
            set_flag(ISSUE2828_LEGACY_ENV, false);
            let new = run_case(&script);
            set_flag(ISSUE2828_LEGACY_ENV, true);
            let old = run_case(&script);
            set_flag(ISSUE2828_LEGACY_ENV, false);
            println!(
                "TAKO2828_RESULT arm={arm} case={} code={} legacy_code={} cr={} legacy_cr={}",
                case.id,
                new.code,
                old.code,
                cr_count(&new.text_ja),
                cr_count(&old.text_ja),
            );
            println!(
                "TAKO2828_TEXT arm={arm} case={} [{}]",
                case.id,
                visible(&new.text_ja)
            );
            println!(
                "TAKO2828_TEXT_LEGACY arm={arm} case={} [{}]",
                case.id,
                visible(&old.text_ja)
            );
            // 成否と終了コードは #2828 の前後で変わらない
            check!(
                new.code == case.exit,
                "{arm} {}: 終了コード {}（期待 {}）: {}",
                case.id,
                new.code,
                case.exit,
                visible(&new.text_ja)
            );
            check!(
                new.code == old.code,
                "{arm} {}: 終了コードが変わった",
                case.id
            );
            // 受け入れ条件 4: CR（0x0D）が 1 バイトも残らない（日英とも・断りの文と応答の output の両方）
            for text in [&new.text_ja, &new.text_en, &new.log] {
                check!(
                    cr_count(text) == 0,
                    "{arm} {}: 文に CR が残った（#2828）: {}",
                    case.id,
                    visible(text)
                );
            }
            // 本文は消えない
            for needle in case.needles {
                check!(
                    contains_loose(&new.text_ja, needle) && contains_loose(&new.text_en, needle),
                    "{arm} {}: 本文（{needle}）が文から消えた（#2828）: {}",
                    case.id,
                    visible(&new.text_ja)
                );
            }
            // 変えたのは改行だけ: 修正前の出力の CR を LF に揃えたものと 1 バイトも違わない
            check!(
                new.log == newlines_to_lf(&old.log),
                "{arm} {}: 改行の他まで変わった:\n新: {}\n旧: {}",
                case.id,
                visible(&new.log),
                visible(&old.log)
            );
            // A/B: 修正前の形は症状が出る（前提が崩れていないことの確かめ。版ごとに違う）
            let before = if is_51 {
                case.cr_before.0
            } else {
                case.cr_before.1
            };
            check!(
                (cr_count(&old.log) > 0) == before,
                "{arm} {}: 修正前の形の CR の有無が前提（{before}）と違う: {}",
                case.id,
                visible(&old.log)
            );
        }
    }
    std::env::set_var("PATH", &original_path);
    assert!(
        fails.is_empty(),
        "#2828 の検査が {} 件落ちた:\n{}",
        fails.len(),
        fails.join("\n----\n")
    );
}
