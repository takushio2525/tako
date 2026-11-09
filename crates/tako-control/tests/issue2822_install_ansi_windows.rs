//! エージェント CLI の導入代行（`tako setup bootstrap install` = MCP `tako_setup_bootstrap`）が Windows で失敗したとき、
//! 断りの文に pwsh 7 の色の符号（ANSI の ESC 列）と 5.1 のインストーラ本文が載らないか（#2822）。
//! **実際に powershell.exe / pwsh.exe を起こして測る**。
//!
//! - pwsh 7 は導入代行の経路（`-OutputFormat Text` + stdout / stderr の捕捉）で、インストーラが host に書いたエラーを
//!   `$PSStyle` の色で飾った平文のまま stderr へ書く。CLIXML ではないので #2812 の平文化は届かない。#2822 から
//!   起動片（`platform::shell::powershell_script_text_command`）が、捕捉されているときだけ `OutputRendering` を
//!   `PlainText` にして止める
//! - 5.1 の最上位の `Write-Error` は、ドットソースした scriptblock（= インストーラ全体）を「コマンド」として
//!   本文の前に出していた。#2822 から起動片は本文を関数 `install.ps1` として定義し、その名前をドットソースする
//!
//! 製品の口（`setup_bootstrap::run_installer` = CLI と MCP がどちらも `install_for_typed` から通る 1 実装）へ
//! 偽の `install.ps1` を渡し、返った断り（`SetupRefusal::InstallerFailed`）の文を日英で読む。取得（外部）は
//! 挟まないので毎回走らせる。インタプリタは製品の解決（`powershell` → `pwsh`）に任せ、pwsh 7 の回は PATH から
//! WindowsPowerShell の置き場を外して倒す。修正前の形は同じバイナリの `TAKO_2822_LEGACY=1` で並べて取る = A/B。
//! #2812 の A/B（`TAKO_2812_LEGACY=1`）との組み合わせも 1 つの偽物で並べて残す。env を書くので
//! `run_alone!` の子の中で走らせる（#2400）。偽物はテストごとの置き場へ書く（#2659）。
//!
//! CI は `.github/workflows/ci.yml` の windows ジョブが全数より手前に名指しで回す。結果の行（`TAKO2822_*`）を
//! `--nocapture` でログへ残す（ESC はログで見えないので `<ESC>` に置き換え、数を `esc=` に出す）。

#![cfg(windows)]

use std::path::{Path, PathBuf};

use tako_control::setup_bootstrap::run_installer;
use tako_control::setup_refusal::SetupRefusal;
use tako_core::i18n::Lang;
use tako_core::platform::agent_install::{recipe, AgentKind};
use tako_core::platform::shell::{ISSUE2812_LEGACY_ENV, ISSUE2822_LEGACY_ENV};
use tako_core::platform::support::Platform;
use tako_core::test_residue::ScratchDir;

/// 1 本の偽のインストーラと、その結果に読めるべきもの
struct Case {
    id: &'static str,
    /// 冒頭（`param` / `Set-StrictMode` / 本文の印）の後ろに続ける本文
    body: &'static str,
    exit: i32,
    /// 新しい形の文に読めるべきエラーの本文（空白を落として探す。5.1 は捕捉した stderr を画面の幅で折り返す）
    needles: &'static [&'static str],
    /// 修正前の形で 5.1 にインストーラ本文が出る（最上位の `Write-Error`）
    body_51_before: bool,
    /// 修正前の形で pwsh 7 に色の符号が出る（host が書いたエラー）
    ansi_7_before: bool,
    /// 呼び出し元が起動片の呼び出し（`install.ps1`）になる = 新しい形の文に `install.ps1` が読める
    names_installer: bool,
    /// ネイティブの子が同じ stderr へ直接書いた色は変えない = 新旧で文がバイト一致
    raw_unchanged: bool,
}

/// インストーラ本文にだけある印（断りの文に出たら本文が載った証拠）
const BODY_MARK: &str = "BODYMARK-2822";

const CASES: &[Case] = &[
    Case {
        id: "top-stop",
        body: "$ErrorActionPreference = 'Stop'\nWrite-Error 'fake top-level error 2822'\nexit 1\n",
        exit: 1,
        needles: &["fake top-level error 2822"],
        body_51_before: true,
        ansi_7_before: false,
        names_installer: true,
        raw_unchanged: false,
    },
    Case {
        id: "top-continue",
        body: "Write-Error 'fake top-level error 2822'\nexit 3\n",
        exit: 3,
        needles: &["fake top-level error 2822"],
        body_51_before: true,
        ansi_7_before: true,
        names_installer: true,
        raw_unchanged: false,
    },
    Case {
        id: "func-continue",
        body: "function Invoke-Install { Write-Error 'fake function error 2822' }\nInvoke-Install\nexit 4\n",
        exit: 4,
        needles: &["fake function error 2822", "Invoke-Install"],
        body_51_before: false,
        ansi_7_before: true,
        names_installer: false,
        raw_unchanged: false,
    },
    Case {
        id: "func-stop",
        body: "$ErrorActionPreference = 'Stop'\nfunction Invoke-Install { Write-Error 'fake function error 2822' }\nInvoke-Install\n",
        exit: 1,
        needles: &["fake function error 2822"],
        body_51_before: false,
        ansi_7_before: false,
        names_installer: false,
        raw_unchanged: false,
    },
    Case {
        id: "throw",
        body: "$ErrorActionPreference = 'Stop'\nthrow 'fake throw 2822'\n",
        exit: 1,
        needles: &["fake throw 2822"],
        body_51_before: false,
        ansi_7_before: false,
        names_installer: false,
        raw_unchanged: false,
    },
    Case {
        id: "cmdlet",
        body: "$ErrorActionPreference = 'Stop'\nGet-Item -LiteralPath 'C:\\tako-2822-no-such\\missing.txt'\n",
        exit: 1,
        needles: &["Cannot find path 'C:\\tako-2822-no-such\\missing.txt'"],
        body_51_before: false,
        ansi_7_before: false,
        names_installer: false,
        raw_unchanged: false,
    },
    // 何も出さずに非 0（境界: 断りの文に添える出力が空）
    Case {
        id: "silent",
        body: "exit 7\n",
        exit: 7,
        needles: &[],
        body_51_before: false,
        ansi_7_before: false,
        names_installer: false,
        raw_unchanged: true,
    },
    // ネイティブの子が自分で書いた色（#2800 / #2812 と同じく 1 バイトも変えない）
    Case {
        id: "native-esc",
        body: "cmd /c \"echo $([char]27)[33mnative 2822$([char]27)[0m 1>&2\"\nexit 5\n",
        exit: 5,
        needles: &["native 2822"],
        body_51_before: false,
        ansi_7_before: false,
        names_installer: false,
        raw_unchanged: true,
    },
    // 成功（色つきの Write-Host を含む）。断りにならず、応答の output に色が載らない
    Case {
        id: "success",
        body: "Write-Host 'ok 2822' -ForegroundColor Green\nWrite-Output 'done 2822'\n",
        exit: 0,
        needles: &["ok 2822", "done 2822"],
        body_51_before: false,
        ansi_7_before: false,
        names_installer: false,
        raw_unchanged: false,
    },
];

/// 起動片の行（#2822 の前後のどちらの形でも、断りの文に出たら包みが載った証拠）
const LAUNCHER_LINES: &[&str] = &[
    "[scriptblock]::Create",
    "ReadAllText",
    "OutputRendering",
    "${function:",
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
    let script = sub.join("tako-codex-install-2822.ps1");
    let text = format!(
        "param([string]$Target = \"latest\")\nSet-StrictMode -Version Latest\n# {BODY_MARK} 偽の install.ps1（#2822）\n{}",
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

fn esc_count(s: &str) -> usize {
    s.matches('\u{1b}').count()
}

/// ログで読める形（ESC は `<ESC>`、改行は ` ⏎ `）。長いものは頭だけ
fn visible(s: &str) -> String {
    let v = s.replace('\u{1b}', "<ESC>").replace('\n', " ⏎ ");
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
fn issue2822_導入代行の失敗の文に色の符号とインストーラ本文が載らない() {
    if tako_core::run_alone!(issue2822_導入代行の失敗の文に色の符号とインストーラ本文が載らない)
    {
        return;
    }
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let dir = ScratchDir::new("i2822-install");
    let codex = recipe(Platform::Windows, AgentKind::Codex);

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
        println!("TAKO2822_SHELL arm={arm} program={program}");
        let lower = program.to_ascii_lowercase();
        if arm == "ps51" {
            assert!(
                lower.ends_with(POWERSHELL_51_TAIL),
                "製品の既定の解決が 5.1 でない（{program}）"
            );
        } else {
            assert!(
                lower.ends_with("\\pwsh.exe"),
                "WindowsPowerShell を PATH から外しても pwsh 7 へ倒れない（{program}）。#2822 の pwsh 7 を測れない"
            );
        }
        let is_51 = arm == "ps51";

        for case in CASES {
            let script = write_fake(&dir, arm, case);
            set_flag(ISSUE2822_LEGACY_ENV, false);
            let new = run_case(&script);
            set_flag(ISSUE2822_LEGACY_ENV, true);
            let old = run_case(&script);
            set_flag(ISSUE2822_LEGACY_ENV, false);
            println!(
                "TAKO2822_RESULT arm={arm} case={} code={} legacy_code={} esc={} legacy_esc={} body={} legacy_body={}",
                case.id,
                new.code,
                old.code,
                esc_count(&new.text_ja),
                esc_count(&old.text_ja),
                new.text_ja.contains(BODY_MARK),
                old.text_ja.contains(BODY_MARK),
            );
            println!(
                "TAKO2822_TEXT arm={arm} case={} [{}]",
                case.id,
                visible(&new.text_ja)
            );
            println!(
                "TAKO2822_TEXT_LEGACY arm={arm} case={} [{}]",
                case.id,
                visible(&old.text_ja)
            );
            // 成否と終了コードは #2822 の前後で変わらない（呼び手は終了コードで失敗の種類を分ける）
            assert_eq!(new.code, case.exit, "{arm} {}: {}", case.id, new.text_ja);
            assert_eq!(new.code, old.code, "{arm} {}", case.id);
            // 受け入れ条件 6: エラーの本文は消えていない（応答の output / 断りの文の両方）
            for needle in case.needles {
                assert!(
                    contains_loose(&new.text_ja, needle) && contains_loose(&new.text_en, needle),
                    "{arm} {}: エラーの本文（{needle}）が文から消えた（#2822）: {}",
                    case.id,
                    new.text_ja
                );
            }
            if case.raw_unchanged {
                // ネイティブの子の出力・空の出力は 1 バイトも変えない（色の符号も残す）
                assert_eq!(new.log, old.log, "{arm} {}: 素の出力が変わった", case.id);
                continue;
            }
            // 受け入れ条件 1: ESC（0x1B）が 1 バイトも残らない（日英とも）
            for text in [&new.text_ja, &new.text_en] {
                assert_eq!(
                    esc_count(text),
                    0,
                    "{arm} {}: 断りの文に色の符号（ESC）が残っている（#2822）: {}",
                    case.id,
                    visible(text)
                );
            }
            // 受け入れ条件 2: インストーラ本文と起動片の行が載らない
            assert!(
                !new.text_ja.contains(BODY_MARK),
                "{arm} {}: 断りの文にインストーラ本文が出ている（#2822）: {}",
                case.id,
                visible(&new.text_ja)
            );
            for line in LAUNCHER_LINES {
                assert!(
                    !new.text_ja.contains(line),
                    "{arm} {}: 断りの文に起動片の行（{line}）が出ている（#2822）: {}",
                    case.id,
                    visible(&new.text_ja)
                );
            }
            if case.names_installer {
                // 最上位の Write-Error の呼び出し元はインストーラ（5.1 `install.ps1 : <本文>` / pwsh 7 `install.ps1: `）
                assert!(
                    new.text_ja.contains("install.ps1"),
                    "{arm} {}: 最上位のエラーの出どころ（install.ps1）が読めない: {}",
                    case.id,
                    visible(&new.text_ja)
                );
            }
            // A/B: 修正前の形は症状が出る（前提が崩れていないことの確かめ）
            if is_51 && case.body_51_before {
                assert!(
                    old.text_ja.contains(BODY_MARK),
                    "{arm} {}: 修正前の形でインストーラ本文が出ない（前提が崩れた）: {}",
                    case.id,
                    visible(&old.text_ja)
                );
            }
            if !is_51 && case.ansi_7_before {
                assert!(
                    esc_count(&old.text_ja) > 0,
                    "{arm} {}: 修正前の形で色の符号が出ない（前提が崩れた）: {}",
                    case.id,
                    visible(&old.text_ja)
                );
            }
        }

        // #2812 の A/B との組み合わせ（最上位の非終端の Write-Error の 1 本で 4 通りを並べて残す）
        let case = CASES
            .iter()
            .find(|c| c.id == "top-continue")
            .expect("top-continue");
        let script = write_fake(&dir, &format!("{arm}-mix"), case);
        for (l2812, l2822) in [(false, false), (false, true), (true, false), (true, true)] {
            set_flag(ISSUE2812_LEGACY_ENV, l2812);
            set_flag(ISSUE2822_LEGACY_ENV, l2822);
            let got = run_case(&script);
            set_flag(ISSUE2812_LEGACY_ENV, false);
            set_flag(ISSUE2822_LEGACY_ENV, false);
            let esc = esc_count(&got.text_ja);
            let body = got.text_ja.contains(BODY_MARK);
            println!(
                "TAKO2822_MIX arm={arm} legacy2812={l2812} legacy2822={l2822} code={} esc={esc} body={body}",
                got.code
            );
            // 色の符号（pwsh 7）とインストーラ本文（5.1）は #2822 の門だけが決める（#2812 は導入代行の経路に届かない）
            if is_51 {
                assert_eq!(body, l2822, "{arm} 2812={l2812} 2822={l2822}");
            } else {
                assert_eq!(esc > 0, l2822, "{arm} 2812={l2812} 2822={l2822}");
            }
            assert_eq!(got.code, case.exit, "{arm} 2812={l2812} 2822={l2822}");
        }
    }
    std::env::set_var("PATH", &original_path);
}
