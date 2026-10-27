//! 新しいペインがレジストリの PATH を読み直すことの実測（#2428）。**実際に ConPTY を起動して測る**。
//!
//! tako のペインは tako-app の起動時の env を継ぐので、tako を開いたままインストーラが
//! ユーザー環境変数 `Path` へ足した置き場所は、#2428 まで新しいタブに載らなかった
//! （setup が出す「次の 1 手」`claude auth login` が tako の再起動まで通らない）。
//!
//! ## 測り方
//!
//! 「起動したあとでレジストリに増えた」状態を、**レジストリを書かずに**作る: レジストリにある
//! エントリを 1 つ選び、それを抜いた PATH で自分のテストバイナリを子として起こし直す
//! （子から見ると「起動時の PATH に無く、レジストリにだけ在る」）。子は実物の ConPTY ペインを起こし、
//! ペインの中の PowerShell に `$env:Path` をファイルへ書かせて、
//!
//! - 抜いたエントリがペインの PATH に戻っている（= 読み直した）
//! - 子の起動時の PATH が先頭にそのまま残っている（= 起動時にしか無い項目を落とさない）
//! - `TAKO_2428_LEGACY=1` の子では戻らない（= A/B の旧アームで差が出る = この測り方に検出力がある）
//!
//! を確かめる。親のプロセスの env は書き換えない（#2400。子へは `Command::env` で渡す）。
//! レジストリは読むだけで、書かない。
//!
//! Windows 以外ではコンパイルしない（unix のペインはログインシェルとして profile を読み直すので、
//! この経路を通らない）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tako_core::platform::user_path;
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

/// 子として起こし直したときだけ立つ（親が渡す）
const CHILD_ENV: &str = "TAKO_2428_CHILD";
/// 子へ「抜いたエントリ」を渡す
const DROPPED_ENV: &str = "TAKO_2428_DROPPED";
/// 子へ「ペインの PATH を書き出すファイル」を渡す（ペインはこの env を継ぐ）
const OUT_ENV: &str = "TAKO_2428_OUT";
/// 子が最後に出す目印（行の途中から拾う = #2248）
const MARK: &str = "TAKO2428_CHILD_DONE";

fn lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn system_root() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string()))
}

/// 抜いても子とペインが動けるエントリか（system32 と PowerShell の置き場所は抜かない）
fn droppable(expanded: &str) -> bool {
    let lower = expanded.to_ascii_lowercase();
    let root = system_root().display().to_string().to_ascii_lowercase();
    !lower.starts_with(&root) && !lower.contains("powershell") && !lower.contains('%')
}

/// レジストリ（マシン → ユーザー）から、抜いて測るエントリを 1 つ選ぶ（展開済みの形）
fn pick_registry_entry() -> String {
    let registry = user_path::registry_paths();
    assert!(
        registry.machine.is_some(),
        "マシンの Path をレジストリから読めない（Windows には必ず在る）"
    );
    let picked = [registry.machine.as_deref(), registry.user.as_deref()]
        .into_iter()
        .flatten()
        .flat_map(user_path::split_entries)
        .map(|entry| user_path::expand_in(entry, &lookup))
        .find(|expanded| droppable(expanded));
    picked.expect("抜いて測れるエントリがレジストリに 1 つも無い")
}

/// `dropped` を抜いた PATH（並びと表記は起動時の値のまま）
fn path_without(dropped: &str) -> String {
    let process = std::env::var("PATH").expect("テストプロセスの PATH");
    process
        .split(';')
        .filter(|entry| {
            let entry = entry.trim();
            !entry.is_empty() && !user_path::entry_matches_in(entry, Path::new(dropped), &lookup)
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// 子を起こして目印まで待つ（上限つき。#1503）。戻り値は子の標準出力 + 標準エラー
fn run_child(child_path: &str, dropped: &str, legacy: bool) -> String {
    let exe = std::env::current_exe().expect("自分のテストバイナリ");
    let mut command = Command::new(exe);
    command
        .args([
            "--exact",
            "issue2428_child_writes_pane_path",
            "--include-ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PATH", child_path)
        .env(CHILD_ENV, "1")
        .env(DROPPED_ENV, dropped)
        .env_remove("TAKO_2428_LEGACY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if legacy {
        command.env("TAKO_2428_LEGACY", "1");
    }
    let mut child = command.spawn().expect("子を起こせる");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if child.try_wait().expect("子の状態").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("子が 120 秒で終わらない（legacy={legacy}）");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().expect("子の出力");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "子が失敗した（legacy={legacy}）:\n{text}"
    );
    text
}

/// 子の報告（`MARK new_pane_has_dropped=<bool> keeps_process_prefix=<bool>`）を読む
fn parse_report(text: &str) -> (bool, bool) {
    let line = text
        .lines()
        .find_map(|line| line.find(MARK).map(|at| &line[at..]))
        .unwrap_or_else(|| panic!("子が目印を出していない:\n{text}"));
    let flag = |name: &str| {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .map(|v| v == "true")
            .unwrap_or_else(|| panic!("{name} が報告に無い: {line}"))
    };
    (flag("new_pane_has_dropped"), flag("keeps_process_prefix"))
}

#[test]
fn 新しいペインはレジストリのpathを読み直し起動時のpathを落とさない() {
    let dropped = pick_registry_entry();
    let child_path = path_without(&dropped);
    assert!(
        !user_path::contains_entry_in(&child_path, Path::new(&dropped), &lookup),
        "子の起動時の PATH から抜けていない"
    );

    let current = parse_report(&run_child(&child_path, &dropped, false));
    assert_eq!(
        current,
        (true, true),
        "新しいペインの PATH に、レジストリにだけ在るエントリが戻っていない / 起動時の値が先頭に残っていない"
    );

    // A/B の旧アームでは戻らない = この測り方が #2428 の差を拾えている
    let legacy = parse_report(&run_child(&child_path, &dropped, true));
    assert_eq!(
        legacy,
        (false, true),
        "旧アームでもエントリが戻った（測り方に検出力が無い）"
    );
}

/// 子の本体（親が `--exact` で起こすときだけ走る）。実物の ConPTY ペインを起こし、
/// ペインの中の PowerShell に `$env:Path` をファイルへ書かせて、親へ 1 行で報告する
#[test]
#[ignore = "issue2428 の親テストが子として起こすときだけ走る"]
fn issue2428_child_writes_pane_path() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let dropped = std::env::var(DROPPED_ENV).expect("親が抜いたエントリ");
    let process = std::env::var("PATH").expect("子の起動時の PATH");
    let out = std::env::temp_dir().join(format!("tako-2428-pane-path-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);

    let powershell = system_root().join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let (session, rx) = TerminalSession::spawn(
        120,
        40,
        SpawnOptions {
            command: Some(SpawnCommand {
                program: powershell.display().to_string(),
                args: vec![
                    "-NoLogo".into(),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    format!("[IO.File]::WriteAllText($env:{OUT_ENV}, $env:Path)"),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            // 書き出し先はペインの env として渡す（子のプロセスの env は書き換えない）
            env: vec![(OUT_ENV.to_string(), out.display().to_string())],
            scrollback_lines: None,
        },
    )
    .expect("ConPTY を起動できること");

    let deadline = Instant::now() + Duration::from_secs(60);
    let pane_path = loop {
        if let Ok(text) = std::fs::read_to_string(&out) {
            if !text.is_empty() {
                break text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "ペインが 60 秒で PATH を書き出さない"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(session);
    drop(rx);
    let _ = std::fs::remove_file(&out);

    let has_dropped = user_path::contains_entry_in(&pane_path, Path::new(&dropped), &lookup);
    let base = process.trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    let keeps_prefix = pane_path.starts_with(base);
    println!("{MARK} new_pane_has_dropped={has_dropped} keeps_process_prefix={keeps_prefix}");
}
