//! 実行ペイン（#453 Code Runner / #666 コマンド提案カード / `tako run-interactive`）の PowerShell の包みの行が、
//! 5.1 で利用者のコマンドの最上位の `Write-Error` の前に画面へ出ないか（#2822）。**実際に ConPTY を起動して測る**。
//!
//! 5.1 の `Write-Error` はエラーレコードの呼び出し元をその場の `$MyInvocation` にし、エラーの表示（NormalView）は
//! それが Script なら本文を「コマンド」として前に付ける。実行ペインの最上位ではそれが包み全体（終了コードの判定・
//! 側路への書き込み・案内・入力待ち）になり、ペインの画面に本文の前に並んでいた。#2822 から包み
//! （`platform::shell::powershell_run_pane_script`）は受け入れゲート（#2812）と同じ片を 1 行目の末尾に置いて
//! 5.1 のときだけ `$MyInvocation` を外す。利用者のコマンドは 2 行目のまま（位置の行番号を変えない）。
//!
//! 製品の口（`platform::shell::run_pane_command` = dispatch の `run_pane` と同じ組み立て）でペインを立て、画面
//! （`TerminalSession::visible_lines`）を読む。シェルは製品の解決（pwsh 7 が在ればそれ、無ければ 5.1）に任せ、
//! 5.1 の回は `ProgramFiles` を pwsh 7 の無い置き場へ向けて倒す（#2812 の検査と同じ）。修正前の形は同じバイナリの
//! `TAKO_2822_LEGACY=1` で並べて取る = A/B。env を書くので `run_alone!` の子の中で走らせる（#2400）。
//!
//! CI は `.github/workflows/ci.yml` の windows ジョブが全数より手前に名指しで回す。結果の行（`TAKO2822_PANE*`）を
//! `--nocapture` でログへ残す。
//!
//! テストの冒頭で `FreeConsole` して出荷構成（コンソールを持たない GUI プロセス）に合わせる（`encoding_conpty.rs` と同じ）。

#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use tako_core::platform::shell::{run_pane_command, ISSUE2822_LEGACY_ENV};
use tako_core::terminal::{SessionNotice, SpawnOptions, TerminalSession};
use tako_core::test_residue::ScratchDir;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
}

fn detach_own_console() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        FreeConsole();
    });
}

/// 起動から画面が揃うまでを待つ上限（ConPTY + PowerShell の起動。混んだ CI でも足りる幅の状態待ちの上限）
const START_LIMIT: Duration = Duration::from_secs(120);

/// 1 本の利用者のコマンド
struct Case {
    id: &'static str,
    cmd: &'static str,
    /// 側路に書かれる終了コード（`None` = 終端エラーで包みごと止まり、側路へ書かれない = #2822 の前から）
    exit: Option<i32>,
    /// 画面に読めるべきエラーの本文
    needle: &'static str,
    /// 修正前の形で 5.1 に包みの行が出る（最上位の `Write-Error`）
    wrapper_51_before: bool,
    /// 5.1 の新しい形に読めるべき利用者のコマンドの位置
    position_51: Option<&'static str>,
    /// pwsh 7 に読めるべき利用者のコマンドの位置（新旧で同じ）
    position_7: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        id: "write-error",
        cmd: "Write-Error 'pane error 2822'",
        exit: Some(1),
        needle: "pane error 2822",
        wrapper_51_before: true,
        position_51: Some("At line:2 char:1"),
        position_7: None,
    },
    Case {
        id: "stop",
        cmd: "Write-Error 'pane stop 2822' -ErrorAction Stop",
        exit: None,
        needle: "pane stop 2822",
        wrapper_51_before: true,
        position_51: Some("At line:2 char:1"),
        position_7: Some("2 |  Write-Error 'pane stop 2822' -ErrorAction Stop"),
    },
    Case {
        id: "cmdlet",
        cmd: "Get-Item -LiteralPath 'C:\\tako-2822-no-such\\missing.txt'",
        exit: Some(1),
        needle: "Cannot find path",
        wrapper_51_before: false,
        position_51: Some("At line:2 char:1"),
        position_7: Some("2 |  Get-Item -LiteralPath"),
    },
    // 境界: エラーの文を出さずに非 0（ネイティブ）。画面も終了コードも新旧で同じ
    Case {
        id: "native",
        cmd: "cmd /c exit 7",
        exit: Some(7),
        needle: "",
        wrapper_51_before: false,
        position_51: None,
        position_7: None,
    },
];

/// 包みの行（5.1 の修正前の形で本文の前に出ていたもの）
const WRAPPER_LINES: &[&str] = &[
    "$global:LASTEXITCODE = $null",
    "$__tako_ok = $?",
    "Set-Content -LiteralPath",
    "[Console]::ReadLine()",
];

const POWERSHELL_51_TAIL: &str = "\\windowspowershell\\v1.0\\powershell.exe";

fn set_flag(name: &str, on: bool) {
    if on {
        std::env::set_var(name, "1");
    } else {
        std::env::remove_var(name);
    }
}

/// 1 回のペインの結果
struct Shown {
    program: String,
    screen: String,
    code: Option<i32>,
}

/// 製品の口でペインを立て、終わるまで（側路の終了コード + 案内 / 終端エラーの本文）待って画面を読む
fn run_pane(dir: &Path, tag: &str, case: &Case) -> Shown {
    let exit_file = dir.join(format!("{tag}-{}.code", case.id));
    let spawn = run_pane_command(case.cmd, "__TAKO_EXIT=", Some(&exit_file));
    let program = spawn.program.clone();
    let (mut session, mut rx) = TerminalSession::spawn(
        200,
        60,
        SpawnOptions {
            command: Some(spawn),
            cwd: Some(dir.to_path_buf()),
            env: Vec::new(),
            scrollback_lines: None,
        },
    )
    .expect("ConPTY を起動できること");
    let deadline = Instant::now() + START_LIMIT;
    let mut exited = false;
    loop {
        while let Ok(ev) = rx.try_recv() {
            if let Some(SessionNotice::Exited) = session.process_event(ev) {
                exited = true;
            }
        }
        let screen = session.visible_lines().join("\n");
        let code = std::fs::read_to_string(&exit_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        let done = match case.exit {
            // 案内（[tako] 終了コード / Exit code）まで描かれた = 包みが最後まで走った
            Some(_) => code.is_some() && screen.contains("[tako]"),
            // 終端エラー: 包みごと止まる。本文が描かれ、シェルが終わるまで
            None => screen.contains(case.needle) && exited,
        };
        if done {
            // 入力待ちを Enter で抜けて閉じる（終端エラーの回はもう終わっている）
            if case.exit.is_some() {
                session.write(b"\r".to_vec());
            }
            return Shown {
                program,
                screen,
                code,
            };
        }
        assert!(
            Instant::now() < deadline,
            "{tag} {}: ペインが {START_LIMIT:?} で終わらない（exited={exited} code={code:?}）:\n{screen}",
            case.id
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// ログで読める形（空行を落とし、改行は ` ⏎ `）。pwsh が遅いときだけ出すプロファイルの読み込み時間の行
/// （`Loading personal and system profiles took 612ms.`。値が回ごとに揺れる）は比べる対象から外す
fn visible(s: &str) -> String {
    s.lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && !l.starts_with("Loading personal and system profiles took"))
        .collect::<Vec<_>>()
        .join(" ⏎ ")
}

#[test]
fn issue2822_実行ペインの画面に51の包みの行が出ない() {
    if tako_core::run_alone!(issue2822_実行ペインの画面に51の包みの行が出ない) {
        return;
    }
    detach_own_console();
    let original_pf = std::env::var_os("ProgramFiles");
    let no_pwsh = ScratchDir::new("i2822-no-pwsh");
    let dir = ScratchDir::new("i2822-pane");

    for arm in ["ps51", "default"] {
        match arm {
            "ps51" => std::env::set_var("ProgramFiles", no_pwsh.as_os_str()),
            _ => match &original_pf {
                Some(v) => std::env::set_var("ProgramFiles", v),
                None => std::env::remove_var("ProgramFiles"),
            },
        }
        let is_51 = arm == "ps51";
        for case in CASES {
            set_flag(ISSUE2822_LEGACY_ENV, false);
            let new = run_pane(dir.path(), &format!("{arm}-new"), case);
            set_flag(ISSUE2822_LEGACY_ENV, true);
            let old = run_pane(dir.path(), &format!("{arm}-legacy"), case);
            set_flag(ISSUE2822_LEGACY_ENV, false);

            println!(
                "TAKO2822_PANE arm={arm} case={} program={} code={:?} legacy_code={:?} wrapper={} legacy_wrapper={}",
                case.id,
                new.program,
                new.code,
                old.code,
                WRAPPER_LINES.iter().any(|l| new.screen.contains(l)),
                WRAPPER_LINES.iter().any(|l| old.screen.contains(l)),
            );
            println!(
                "TAKO2822_PANE_SCREEN arm={arm} case={} [{}]",
                case.id,
                visible(&new.screen)
            );
            println!(
                "TAKO2822_PANE_SCREEN_LEGACY arm={arm} case={} [{}]",
                case.id,
                visible(&old.screen)
            );
            let program = new.program.to_ascii_lowercase();
            if is_51 {
                assert!(
                    program.ends_with(POWERSHELL_51_TAIL),
                    "pwsh 7 の無い置き場で 5.1 へ倒れない（{}）",
                    new.program
                );
            } else if program.ends_with(POWERSHELL_51_TAIL) {
                panic!("pwsh 7 が無い（既定の解決が 5.1）。#2822 の pwsh 7 の不変を測れない");
            }
            // 終了コードは #2822 の前後で変わらない
            assert_eq!(new.code, case.exit, "{arm} {}: {}", case.id, new.screen);
            assert_eq!(new.code, old.code, "{arm} {}", case.id);
            if !case.needle.is_empty() {
                assert!(
                    new.screen.contains(case.needle),
                    "{arm} {}: エラーの本文が画面から消えた（#2822）:\n{}",
                    case.id,
                    new.screen
                );
            }
            // 受け入れ条件 3: 包みの行が画面に出ない
            for line in WRAPPER_LINES {
                assert!(
                    !new.screen.contains(line),
                    "{arm} {}: 包みの行（{line}）が画面に出ている（#2822）:\n{}",
                    case.id,
                    new.screen
                );
            }
            if is_51 {
                // 利用者のコマンドの位置は 2 行目のまま
                if let Some(position) = case.position_51 {
                    assert!(
                        new.screen.contains(position),
                        "{arm} {}: 利用者のコマンドの位置（{position}）が画面に無い（#2822）:\n{}",
                        case.id,
                        new.screen
                    );
                }
                // A/B: 修正前の形は症状が出る（前提が崩れていないことの確かめ）
                if case.wrapper_51_before {
                    assert!(
                        old.screen.contains("$__tako_ok = $?"),
                        "{arm} {}: 修正前の形で包みの行が出ない（前提が崩れた）:\n{}",
                        case.id,
                        old.screen
                    );
                }
            } else {
                // pwsh 7 は外さない（版で分ける）: 画面は新旧で同じ
                assert_eq!(
                    visible(&new.screen),
                    visible(&old.screen),
                    "{arm} {}: pwsh 7 の画面が #2822 の前後で変わった",
                    case.id
                );
                if let Some(position) = case.position_7 {
                    assert!(
                        new.screen.contains(position),
                        "{arm} {}: 利用者のコマンドの位置（{position}）が画面に無い:\n{}",
                        case.id,
                        new.screen
                    );
                }
            }
        }
    }
    match &original_pf {
        Some(v) => std::env::set_var("ProgramFiles", v),
        None => std::env::remove_var("ProgramFiles"),
    }
}
