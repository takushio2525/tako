//! 実行ペイン（#453 Code Runner / #666 / `tako run-interactive`）と `split --command` の失敗したときだけ止める形
//! （#1031）で、利用者のコマンドが最上位の終端エラー（`throw` / `-ErrorAction Stop` / `$ErrorActionPreference = 'Stop'`）に
//! なっても、終了コード（非 0）が側路へ書かれ、案内と Enter 待ちが出るか（#2827）。**実際に ConPTY を起動して測る**。
//!
//! #2827 の前は終端エラーで包みのスクリプト全体が止まり、側路への書き込み・案内・入力待ちが走らなかった
//! （`code=None`、ペインはエラーを出してすぐ閉じる）。#2827 から包み（`platform::shell`）は 1 行目の先頭で `prompt` を
//! 差し替え（`powershell_unfinished_hook`）、`-NoExit` で起こす。ホストはいつもの形でエラーを表示した後で `prompt` を
//! 呼ぶので、そこで終了コード・案内・入力待ちを出す。利用者のコマンドを `try` / `trap` で囲まないので、文だけを止める
//! エラーの後ろが走る（`continue` / `nested`）・`exit 3` で抜けるといった意味は修正前と同じ = ここで新旧を並べて縛る。
//!
//! 製品の口（`platform::shell::run_pane_command` = dispatch の `run_pane` と同じ組み立て / `hold_on_failure_command` =
//! dispatch の Split の腕と同じ組み立て）でペインを立て、画面（`TerminalSession::visible_lines`）と側路を読む。シェルは
//! 製品の解決（pwsh 7 が在ればそれ、無ければ 5.1）に任せ、5.1 の回は `ProgramFiles` を pwsh 7 の無い置き場へ向けて倒す
//! （#2822 の検査と同じ）。修正前の形は同じバイナリの `TAKO_2827_LEGACY=1` で並べて取る = A/B。env を書くので
//! `run_alone!` の子の中で走らせる（#2400）。`.ps1` はテストごとの置き場へ書く（#2659）。
//!
//! CI は `.github/workflows/ci.yml` の windows ジョブが全数より手前に名指しで回す。結果の行（`TAKO2827_*`）を
//! `--nocapture` でログへ残す。
//!
//! テストの冒頭で `FreeConsole` して出荷構成（コンソールを持たない GUI プロセス）に合わせる（`encoding_conpty.rs` と同じ）。

#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use tako_core::platform::shell::{hold_on_failure_command, run_pane_command, ISSUE2827_LEGACY_ENV};
use tako_core::terminal::{SessionNotice, SpawnCommand, SpawnOptions, TerminalSession};
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

/// 起動から画面が揃うまで・Enter から終わるまでを待つ上限（ConPTY + PowerShell の起動。混んだ CI でも足りる幅の状態待ちの上限）
const START_LIMIT: Duration = Duration::from_secs(120);

/// ペインの終わり方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// 終了コードを側路へ書き、案内（`[tako]`）を出して Enter を待つ
    Hold(i32),
    /// 待たずに終わる（側路は空）
    Gone,
}

/// 何を起こすか
#[derive(Clone, Copy)]
enum Form {
    /// 実行ペイン（`run_pane_command`）へ渡すコマンド。`{ps1}` は終端エラーを投げる `.ps1` のパスに置き換える
    Pane(&'static str),
    /// `split --command` の argv（`hold_on_failure_command`）。`{ps1}` は同上
    Hold(&'static [&'static str]),
}

struct Case {
    id: &'static str,
    form: Form,
    /// 修正後の終わり方
    new: End,
    /// 修正前の終わり方（`TAKO_2827_LEGACY=1`）
    legacy: End,
    /// 新しい形の画面に読めるべき本文
    needle: &'static str,
    /// 画面にこの文字ちょうどの行があってはいけない（終端エラーの後ろの文が走っていない。5.1 のエラー表示は利用者の
    /// コマンドの行をそのまま見せるので、部分一致では見ない）
    absent: &'static [&'static str],
    /// 画面にこの文字ちょうどの行がある（その文が走った）
    ran: &'static [&'static str],
    /// 新旧で画面が同じ（終端でない失敗・成功・ネイティブの子・`exit`）
    same_screen: bool,
    /// 5.1 / pwsh 7 の新しい形の画面に読めるべき利用者のコマンドの位置（行番号は 2 行目のまま）
    position_51: Option<&'static str>,
    position_7: Option<&'static str>,
    /// 修正前の形で 5.1 に包みの行が出る（argv が `Write-Error` の `split --command`）
    wrapper_51_before: bool,
    /// 画面にこの文字が出たら Ctrl+C を送る
    interrupt_after: Option<&'static str>,
}

const CASES: &[Case] = &[
    // --- 最上位の終端エラー（修正前は側路が空で、ペインがすぐ閉じた） ---
    Case {
        id: "throw",
        form: Form::Pane("throw 'pane throw 2827'; Write-Host 'AFTER-2827'"),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "pane throw 2827",
        absent: &["AFTER-2827"],
        ran: &[],
        same_screen: false,
        position_51: Some("At line:2 char:1"),
        position_7: Some("2 |  throw 'pane throw 2827'"),
        wrapper_51_before: false,
        interrupt_after: None,
    },
    Case {
        id: "stop",
        form: Form::Pane("Write-Error 'pane stop 2827' -ErrorAction Stop"),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "pane stop 2827",
        absent: &[],
        ran: &[],
        same_screen: false,
        position_51: Some("At line:2 char:1"),
        position_7: Some("2 |  Write-Error 'pane stop 2827' -ErrorAction Stop"),
        wrapper_51_before: false,
        interrupt_after: None,
    },
    Case {
        id: "cmdlet-stop",
        form: Form::Pane("Get-Item -LiteralPath 'C:\\tako-2827-no-such\\missing.txt' -ErrorAction Stop"),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "Cannot find path",
        absent: &[],
        ran: &[],
        same_screen: false,
        position_51: Some("At line:2 char:1"),
        position_7: Some("2 |  Get-Item -LiteralPath"),
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // `$ErrorActionPreference = 'Stop'` の下の非終端のエラー。後ろの文は走らない
    Case {
        id: "eap-stop",
        form: Form::Pane(
            "$ErrorActionPreference = 'Stop'; Get-Item -LiteralPath 'C:\\tako-2827-no-such\\missing.txt'; Write-Host 'AFTER-2827'",
        ),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "Cannot find path",
        absent: &["AFTER-2827"],
        ran: &[],
        same_screen: false,
        position_51: Some("At line:2 char:"),
        // pwsh 7 は長い行の端を `…` で省くので、行番号（2 行目）だけを見る
        position_7: Some("   2 |  "),
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // 境界: ネイティブの子の非 0 の後の終端エラー = 共有の片の失敗の腕と同じ規則で 7
    Case {
        id: "native-then-throw",
        form: Form::Pane("cmd /c exit 7; throw 'after native 2827'"),
        new: End::Hold(7),
        legacy: End::Gone,
        needle: "after native 2827",
        absent: &[],
        ran: &[],
        same_screen: false,
        position_51: Some("At line:2 char:"),
        position_7: Some("2 |  cmd /c exit 7; throw 'after native 2827'"),
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // `& .\x.ps1` の中の終端エラー（スクリプトの残りも、後ろの文も走らない）
    Case {
        id: "ps1-throw",
        form: Form::Pane("& '{ps1}'; Write-Host 'AFTER-2827'"),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "ps1 throw 2827",
        absent: &["PS1-AFTER-2827", "AFTER-2827"],
        ran: &["PS1-BEFORE-2827"],
        same_screen: false,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // 利用者が Ctrl+C で止めた（修正前はペインが閉じた。#2827 からは終了コード 1 と案内を出して待つ）
    Case {
        id: "ctrl-c",
        form: Form::Pane("Write-Host 'READY-2827'; Start-Sleep -Seconds 90; Write-Host 'AFTER-2827'"),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "READY-2827",
        absent: &["AFTER-2827"],
        ran: &["READY-2827"],
        same_screen: false,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: Some("READY-2827"),
    },
    // --- 修正前と同じでなければならないもの（利用者のコマンドを try / trap で囲まない = 意味を変えない） ---
    Case {
        id: "write-error",
        form: Form::Pane("Write-Error 'pane error 2827'"),
        new: End::Hold(1),
        legacy: End::Hold(1),
        needle: "pane error 2827",
        absent: &[],
        ran: &[],
        same_screen: true,
        position_51: Some("At line:2 char:1"),
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // 文だけを止めるエラー（.NET の例外）の後ろは走る
    Case {
        id: "continue",
        form: Form::Pane("[int]::Parse('x'); Write-Host 'AFTER-2827'"),
        new: End::Hold(0),
        legacy: End::Hold(0),
        needle: "AFTER-2827",
        absent: &[],
        ran: &["AFTER-2827"],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // 入れ子の関数の中の文だけを止めるエラーの後ろも走る（最上位を try / trap で囲むとここが走らなくなる）
    Case {
        id: "nested",
        form: Form::Pane(
            "function f { [int]::Parse('x'); Write-Host 'F-AFTER-2827' }; f; Write-Host 'AFTER-2827'",
        ),
        new: End::Hold(0),
        legacy: End::Hold(0),
        needle: "F-AFTER-2827",
        absent: &[],
        ran: &["F-AFTER-2827", "AFTER-2827"],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    Case {
        id: "not-found",
        form: Form::Pane("tako-2827-no-such-command"),
        new: End::Hold(1),
        legacy: End::Hold(1),
        needle: "tako-2827-no-such-command",
        absent: &[],
        ran: &[],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    Case {
        id: "native",
        form: Form::Pane("cmd /c exit 7"),
        new: End::Hold(7),
        legacy: End::Hold(7),
        needle: "",
        absent: &[],
        ran: &[],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // `$ErrorActionPreference = 'Stop'` の下のネイティブの子の非 0（終端にならない）
    Case {
        id: "eap-native",
        form: Form::Pane("$ErrorActionPreference = 'Stop'; cmd /c exit 7"),
        new: End::Hold(7),
        legacy: End::Hold(7),
        needle: "",
        absent: &[],
        ran: &[],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    Case {
        id: "ok",
        form: Form::Pane("Write-Host 'ok 2827'"),
        new: End::Hold(0),
        legacy: End::Hold(0),
        needle: "ok 2827",
        absent: &[],
        ran: &["ok 2827"],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // 利用者の `exit N` は `-NoExit` でもそのまま抜ける（口は呼ばれない）
    Case {
        id: "exit",
        form: Form::Pane("Write-Host 'bye 2827'; exit 3"),
        new: End::Gone,
        legacy: End::Gone,
        needle: "bye 2827",
        absent: &[],
        ran: &["bye 2827"],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // --- `split --command` の失敗したときだけ止める形（#1031） ---
    Case {
        id: "hold-write-error",
        form: Form::Hold(&["Write-Error", "hold error 2827"]),
        new: End::Hold(1),
        legacy: End::Hold(1),
        needle: "hold error 2827",
        absent: &[],
        ran: &[],
        same_screen: false,
        position_51: Some("At line:2 char:1"),
        position_7: None,
        wrapper_51_before: true,
        interrupt_after: None,
    },
    Case {
        id: "hold-ps1-throw",
        form: Form::Hold(&["{ps1}"]),
        new: End::Hold(1),
        legacy: End::Gone,
        needle: "ps1 throw 2827",
        absent: &["PS1-AFTER-2827"],
        ran: &["PS1-BEFORE-2827"],
        same_screen: false,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    Case {
        id: "hold-native",
        form: Form::Hold(&["cmd", "/c", "exit", "3"]),
        new: End::Hold(3),
        legacy: End::Hold(3),
        needle: "",
        absent: &[],
        ran: &[],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
    // 成功は待たずに閉じる（側路にも書かない）
    Case {
        id: "hold-ok",
        form: Form::Hold(&["cmd", "/c", "exit", "0"]),
        new: End::Gone,
        legacy: End::Gone,
        needle: "",
        absent: &[],
        ran: &[],
        same_screen: true,
        position_51: None,
        position_7: None,
        wrapper_51_before: false,
        interrupt_after: None,
    },
];

/// 包みの行（画面に出たら包みのスクリプトが「コマンド」として見えている）
const WRAPPER_LINES: &[&str] = &[
    "$global:LASTEXITCODE = $null",
    "$__tako_ok = $?",
    "Set-Content -LiteralPath",
    "[Console]::ReadLine()",
    "function global:prompt",
    "if ($__tako_code -ne 0)",
];

/// `& .\x.ps1` の中で終端エラーを投げるスクリプト（3 行目は走らない）
const PS1_BODY: &str =
    "Write-Host 'PS1-BEFORE-2827'\r\nthrow 'ps1 throw 2827'\r\nWrite-Host 'PS1-AFTER-2827'\r\n";

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
    /// 側路に書かれた終了コード（Enter の前に読む）
    code: Option<i32>,
    /// 案内（`[tako]`）を出して待った
    held: bool,
    /// 終わり方（観測した値）
    end: End,
    /// 起動から待ち（Hold）/ 終わり（Gone）までのミリ秒（参考。判定には使わない）
    took_ms: u128,
}

fn spawn_of(case: &Case, ps1: &Path, exit_file: &Path) -> SpawnCommand {
    let ps1 = ps1.display().to_string();
    match case.form {
        Form::Pane(cmd) => {
            run_pane_command(&cmd.replace("{ps1}", &ps1), "__TAKO_EXIT=", Some(exit_file))
        }
        Form::Hold(argv) => {
            let mut words = argv.iter().map(|w| w.replace("{ps1}", &ps1));
            hold_on_failure_command(
                SpawnCommand {
                    program: words.next().expect("argv[0]"),
                    args: words.collect(),
                },
                "__TAKO_EXIT=",
                Some(exit_file),
            )
        }
    }
}

/// 製品の口でペインを立て、Hold（側路 + 案内まで）か Gone（終わるまで）のどちらかに着くのを待って画面を読む。
/// Hold なら Enter を送り、ペインが終わる（対話の `PS>` に落ちない）まで待つ。着かない・Enter で終わらないときは
/// 理由を返す（1 回の CI で全ケースの事実を残すため、ここでは panic しない）
fn run(dir: &Path, ps1: &Path, tag: &str, case: &Case) -> Result<Shown, String> {
    let exit_file = dir.join(format!("{tag}-{}.code", case.id));
    let spawn = spawn_of(case, ps1, &exit_file);
    let program = spawn.program.clone();
    let started = Instant::now();
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
    .map_err(|e| format!("{tag} {}: ConPTY を起動できない: {e}", case.id))?;
    let deadline = Instant::now() + START_LIMIT;
    let mut exited = false;
    let mut interrupted = false;
    loop {
        while let Ok(ev) = rx.try_recv() {
            if let Some(SessionNotice::Exited) = session.process_event(ev) {
                exited = true;
            }
        }
        let screen = session.visible_lines().join("\n");
        if let Some(mark) = case.interrupt_after {
            if !interrupted && has_line(&screen, mark) {
                session.write(b"\x03".to_vec());
                interrupted = true;
            }
        }
        let code = std::fs::read_to_string(&exit_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        let held = code.is_some() && screen.contains("[tako]");
        if held || exited {
            let took_ms = ms_since(started);
            if held {
                // 入力待ちを Enter で抜け、ペインが終わるまで待つ（`PS>` の対話に落ちない）
                session.write(b"\r".to_vec());
                let until = Instant::now() + START_LIMIT;
                while !exited && Instant::now() < until {
                    while let Ok(ev) = rx.try_recv() {
                        if let Some(SessionNotice::Exited) = session.process_event(ev) {
                            exited = true;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let after = session.visible_lines().join("\n");
                if !exited {
                    return Err(format!(
                        "{tag} {}: Enter の後でペインが終わらない（対話に落ちた？）:\n{after}",
                        case.id
                    ));
                }
                if after
                    .lines()
                    .any(|l| l.trim_start().starts_with("PS ") || l.trim() == "PS>")
                {
                    return Err(format!(
                        "{tag} {}: 対話のプロンプトが出た:\n{after}",
                        case.id
                    ));
                }
            }
            return Ok(Shown {
                program,
                screen,
                code,
                held,
                end: match code {
                    Some(c) if held => End::Hold(c),
                    _ => End::Gone,
                },
                took_ms,
            });
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{tag} {}: ペインが {START_LIMIT:?} で着かない（exited={exited} code={code:?}）:\n{screen}",
                case.id
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 起動からのミリ秒（参考の記録だけ。判定には使わない）
fn ms_since(started: Instant) -> u128 {
    Instant::now().duration_since(started).as_millis()
}

/// 画面にこの文字ちょうどの行がある（前後の空白は見ない）
fn has_line(screen: &str, line: &str) -> bool {
    screen.lines().any(|l| l.trim() == line)
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

/// 起動の上乗せ（`-NoExit` は PSReadLine を読み込む）を比べる回数（新旧を交互に起こす。参考の記録だけ）
const TIMING_ROUNDS: usize = 5;

fn median(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

#[test]
fn issue2827_終端エラーでも終了コードと案内とenter待ちを出す() {
    if tako_core::run_alone!(issue2827_終端エラーでも終了コードと案内とenter待ちを出す)
    {
        return;
    }
    detach_own_console();
    let original_pf = std::env::var_os("ProgramFiles");
    let no_pwsh = ScratchDir::new("i2827-no-pwsh");
    let dir = ScratchDir::new("i2827-pane");
    let ps1 = dir.path().join("throw2827.ps1");
    std::fs::write(&ps1, PS1_BODY).expect(".ps1 を書けること");
    // 1 回の CI で全ケースの事実を残すため、失敗は集めて最後に一覧で落とす
    let mut fails: Vec<String> = Vec::new();
    macro_rules! check {
        ($cond:expr, $($fmt:tt)+) => {
            if !$cond {
                fails.push(format!($($fmt)+));
            }
        };
    }

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
            set_flag(ISSUE2827_LEGACY_ENV, false);
            let new = run(dir.path(), &ps1, &format!("{arm}-new"), case);
            set_flag(ISSUE2827_LEGACY_ENV, true);
            let old = run(dir.path(), &ps1, &format!("{arm}-legacy"), case);
            set_flag(ISSUE2827_LEGACY_ENV, false);
            let (new, old) = match (new, old) {
                (Ok(n), Ok(o)) => (n, o),
                (n, o) => {
                    for e in [n.err(), o.err()].into_iter().flatten() {
                        println!(
                            "TAKO2827_ERROR arm={arm} case={} {}",
                            case.id,
                            e.replace('\n', " ⏎ ")
                        );
                        fails.push(e);
                    }
                    continue;
                }
            };

            let wrapper = |s: &str| WRAPPER_LINES.iter().any(|l| s.contains(l));
            println!(
                "TAKO2827_PANE arm={arm} case={} program={} code={:?} legacy_code={:?} held={} legacy_held={} \
                 wrapper={} legacy_wrapper={} took_ms={} legacy_took_ms={}",
                case.id,
                new.program,
                new.code,
                old.code,
                new.held,
                old.held,
                wrapper(&new.screen),
                wrapper(&old.screen),
                new.took_ms,
                old.took_ms,
            );
            println!(
                "TAKO2827_PANE_SCREEN arm={arm} case={} [{}]",
                case.id,
                visible(&new.screen)
            );
            println!(
                "TAKO2827_PANE_SCREEN_LEGACY arm={arm} case={} [{}]",
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
                panic!("pwsh 7 が無い（既定の解決が 5.1）。#2827 の pwsh 7 を測れない");
            }
            // 受け入れ条件 1: 修正前は側路が空（終端エラーで包みごと止まる）、修正後は非 0 の終了コード + 案内 + 待ち
            check!(
                new.end == case.new,
                "{arm} {}: 修正後の終わり方が {:?}（期待 {:?}）:\n{}",
                case.id,
                new.end,
                case.new,
                new.screen
            );
            check!(
                old.end == case.legacy,
                "{arm} {}: 修正前の終わり方が {:?}（期待 {:?} = 前提が崩れた）:\n{}",
                case.id,
                old.end,
                case.legacy,
                old.screen
            );
            if !case.needle.is_empty() {
                check!(
                    new.screen.contains(case.needle),
                    "{arm} {}: 本文（{}）が画面に無い:\n{}",
                    case.id,
                    case.needle,
                    new.screen
                );
            }
            for line in case.ran {
                check!(
                    has_line(&new.screen, line),
                    "{arm} {}: 走るはずの文（{line}）の行が画面に無い:\n{}",
                    case.id,
                    new.screen
                );
            }
            for line in case.absent {
                check!(
                    !has_line(&new.screen, line),
                    "{arm} {}: 終端エラーの後ろの文（{line}）が走った:\n{}",
                    case.id,
                    new.screen
                );
            }
            // 包みの行（口の 1 行を含む）は画面に出ない
            for line in WRAPPER_LINES {
                check!(
                    !new.screen.contains(line),
                    "{arm} {}: 包みの行（{line}）が画面に出ている:\n{}",
                    case.id,
                    new.screen
                );
            }
            // 受け入れ条件 2: 終端でない失敗・成功・ネイティブの子・exit は画面も終了コードも修正前と同じ
            if case.same_screen {
                check!(
                    new.code == old.code,
                    "{arm} {}: 終了コードが変わった",
                    case.id
                );
                check!(
                    visible(&new.screen) == visible(&old.screen),
                    "{arm} {}: 画面が #2827 の前後で変わった:\n新: {}\n旧: {}",
                    case.id,
                    visible(&new.screen),
                    visible(&old.screen)
                );
            }
            // 終端エラーの表示はホスト自身のもの = 修正前の画面（エラーで閉じる直前）がそのまま先頭に並ぶ
            if case.legacy == End::Gone && case.new != End::Gone && case.interrupt_after.is_none() {
                let (n, o) = (visible(&new.screen), visible(&old.screen));
                check!(
                    n.starts_with(&o),
                    "{arm} {}: 終端エラーの表示が修正前と違う:\n新: {n}\n旧: {o}",
                    case.id
                );
            }
            let position = if is_51 {
                case.position_51
            } else {
                case.position_7
            };
            if let Some(position) = position {
                check!(
                    new.screen.contains(position),
                    "{arm} {}: 利用者のコマンドの位置（{position}）が画面に無い:\n{}",
                    case.id,
                    new.screen
                );
            }
            // 受け入れ条件 3: split --command の 5.1 の包みの行（修正前は出る・修正後は出ない）
            if is_51 && case.wrapper_51_before {
                check!(
                    wrapper(&old.screen),
                    "{arm} {}: 修正前の形で 5.1 の包みの行が出ない（前提が崩れた）:\n{}",
                    case.id,
                    old.screen
                );
            }
        }

        // 起動の上乗せ（参考）: 「ok」を新旧交互に起こし、案内が出るまでのミリ秒の中央値を残す
        let ok = CASES.iter().find(|c| c.id == "ok").expect("ok の行");
        let (mut new_ms, mut old_ms) = (Vec::new(), Vec::new());
        for round in 0..TIMING_ROUNDS {
            for legacy in [false, true] {
                set_flag(ISSUE2827_LEGACY_ENV, legacy);
                let tag = format!(
                    "{arm}-timing{round}-{}",
                    if legacy { "legacy" } else { "new" }
                );
                match run(dir.path(), &ps1, &tag, ok) {
                    Ok(shown) if legacy => old_ms.push(shown.took_ms),
                    Ok(shown) => new_ms.push(shown.took_ms),
                    Err(e) => fails.push(e),
                }
            }
        }
        set_flag(ISSUE2827_LEGACY_ENV, false);
        println!(
            "TAKO2827_TIMING arm={arm} new_ms={new_ms:?} legacy_ms={old_ms:?} median_new={} median_legacy={}",
            median(new_ms.clone()),
            median(old_ms.clone())
        );
    }
    match &original_pf {
        Some(v) => std::env::set_var("ProgramFiles", v),
        None => std::env::remove_var("ProgramFiles"),
    }
    assert!(
        fails.is_empty(),
        "#2827 の検査が {} 件落ちた:\n{}",
        fails.len(),
        fails.join("\n----\n")
    );
}
