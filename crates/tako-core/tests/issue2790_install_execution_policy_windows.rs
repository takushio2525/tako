//! 実行ポリシーを強制した PC でも、エージェント CLI の導入代行がインストーラを走らせられるか（#2790）。
//! **実際に PowerShell を起こして測る**。
//!
//! 導入代行は取得した `install.ps1` を #2790 より前は `-ExecutionPolicy Bypass -File` で起こしていた。
//! `Bypass` は Process スコープの指定なので、グループポリシー（MachinePolicy / UserPolicy）で
//! AllSigned / Restricted を強制した管理端末（学校・会社の PC）では負けて導入が止まる。
//! #2790 からは本文をテキストで読み、`-EncodedCommand` の起動片から scriptblock として走らせる
//! （`platform::shell::powershell_script_text_args`。実行ポリシーが止めるのはスクリプトファイルだけ）。
//!
//! 起こし方は製品の 1 実装（`InstallRecipe::installer_command` = `setup_bootstrap::run_installer` と同じ口）
//! を通し、インタプリタは製品の候補（`powershell` → `pwsh`）のうちこの環境に在るものすべてで見る。
//! 外部の本物の `install.ps1` は走らせない（偽物は印のファイルを書くだけ）。
//!
//! ## 2 通りの走らせ方
//!
//! - ふだんの `cargo test --workspace`: ポリシーは置かない。新しい形で偽のインストーラが走ること・
//!   終了コードが `-File` と同じになること（修正前の形と突き合わせる）・読めないときに理由つきで
//!   1 になること・失敗時に案内する手で打つ 1 行（`irm <url> | iex`）が走ることを見る
//! - CI の名指しのステップ（`scripts/windows/test-install-execution-policy-2790.ps1`）: グループポリシーの
//!   レジストリ（HKLM / HKCU の `SOFTWARE\Policies\Microsoft\Windows\PowerShell` と pwsh 7 の
//!   `...\PowerShellCore`）へ AllSigned / Restricted を置き、`TAKO_2790_POLICY=<スコープ>:<値>` を付けて
//!   このテストだけを走らせる。ポリシーが効いていることを確かめたうえで、新しい形は印が残り、
//!   修正前の形は同じ条件で弾かれることを見る。置いたポリシーはそのステップの `finally` で戻す
//!
//! 結果の行（`TAKO2790_*`）を `--nocapture` で CI のログへ残す。

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tako_core::platform::agent_install::{recipe, AgentKind, InstallRecipe};
use tako_core::platform::support::Platform;
use tako_core::test_residue::ScratchDir;

/// CI のステップが置いたポリシー（`<スコープ>:<値>`。例 `MachinePolicy:AllSigned`）
const POLICY_ENV: &str = "TAKO_2790_POLICY";

/// 偽のインストーラが印を書く先（テストが子へ渡す）
const MARKER_ENV: &str = "TAKO_2790_MARKER";

/// 1 回の PowerShell の起動を待つ上限（返らない回にテストごと固まらない）
const RUN_LIMIT: Duration = Duration::from_secs(120);

/// 偽の `install.ps1`（BOM なし UTF-8）。本物（2026-11-08 の claude の `install.ps1`）と同じ
/// `param()` + `ValidatePattern` → `Set-StrictMode -Version Latest` → `$ErrorActionPreference = "Stop"` の
/// 冒頭を持ち、本文に非 ASCII・単引用符・二重引用符・バッククォート・`$`・CRLF の行を含む。
/// 何も入れずに印のファイルを書くだけ。`TAKO_2790_THROW` で終端エラー、`TAKO_2790_EXIT` で `exit N`
const FAKE_INSTALLER: &str = "param(\n    [Parameter(Position=0)]\n    [ValidatePattern('^(stable|latest|\\d+\\.\\d+\\.\\d+(-[^\\s]+)?)$')]\n    [string]$Target = \"latest\"\n)\n\nSet-StrictMode -Version Latest\n$ErrorActionPreference = \"Stop\"\r\n# 偽の install.ps1（#2790）。印を書くだけで何も入れない\n$note = \"日本語の印 'single' \"\"double\"\" `$dollar ✅\"\n$line = \"ran target=$Target note=$note\"\n[System.IO.File]::WriteAllText($env:TAKO_2790_MARKER, $line, (New-Object System.Text.UTF8Encoding $false))\nif ($env:TAKO_2790_THROW) { throw 'fake installer terminating error 2790' }\nif ($env:TAKO_2790_EXIT) { exit ([int]$env:TAKO_2790_EXIT) }\nWrite-Output 'fake installer done 2790'\n";

/// 偽のインストーラが書く印（本文の非 ASCII・引用符が崩れずに走った証拠）
const EXPECTED_MARKER: &str = "ran target=latest note=日本語の印 'single' \"double\" $dollar ✅";

/// ステップが置いたポリシー（スコープ, 値）。ふだんの全数では `None`
fn enforced_policy() -> Option<(String, String)> {
    let raw = std::env::var(POLICY_ENV).ok()?;
    let (scope, value) = raw
        .split_once(':')
        .unwrap_or_else(|| panic!("{POLICY_ENV}={raw} は <スコープ>:<値> の形"));
    Some((scope.to_string(), value.to_string()))
}

fn claude_recipe() -> InstallRecipe {
    recipe(Platform::Windows, AgentKind::Claude)
}

/// 製品の候補（`powershell` → `pwsh`）のうち、この環境に在るもの
fn interpreters() -> Vec<String> {
    let found: Vec<String> = claude_recipe()
        .runner
        .candidates
        .iter()
        .filter_map(|name| tako_core::platform::exe::find(name))
        .collect();
    assert!(!found.is_empty(), "powershell も pwsh も見つからない");
    found
}

/// 偽のインストーラを置く（テストごとの置き場。#2659）。置き場の名前に日本語・空白・単引用符を入れて、
/// 起動片がパスを ASCII だけの式（`powershell_ascii_literal`）で正しく埋めることも実物で見る
fn write_fake(dir: &Path) -> PathBuf {
    let sub = dir.join("山田 'q' 2790");
    std::fs::create_dir_all(&sub).expect("置き場を作れない");
    let script = sub.join("tako-claude-install-2790.ps1");
    std::fs::write(&script, FAKE_INSTALLER.as_bytes()).expect("偽のインストーラを書けない");
    script
}

struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn summary(&self) -> String {
        format!(
            "code={:?} stdout=[{}] stderr=[{}]",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// 期限つきで走らせて終了コードと出力を取る
fn run(mut command: Command, what: &str) -> Ran {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|e| panic!("{what} を起こせない: {e}"));
    let mut out = child.stdout.take().expect("stdout");
    let mut err = child.stderr.take().expect("stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + RUN_LIMIT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what} が {RUN_LIMIT:?} 以内に返らない");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ran {
        code: status.code(),
        stdout: String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned(),
    }
}

/// 新しい形（製品の口）で起こす `Command`。`program` を与えたらそのインタプリタで、無ければ製品の解決どおり
fn new_form(script: &Path, program: Option<&str>, marker: &Path) -> Command {
    let mut command = match program {
        None => claude_recipe().installer_command(script),
        Some(program) => {
            let mut command = Command::new(program);
            command.args(claude_recipe().runner.args_for_in(script, false));
            command
        }
    };
    command.env(MARKER_ENV, marker);
    command
}

/// 修正前の形（`-NoLogo -NoProfile -ExecutionPolicy Bypass -File <path>`。A/B の旧アームと同じ組み立て）
fn legacy_form(script: &Path, program: &str, marker: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .args(claude_recipe().runner.args_for_in(script, true))
        .env(MARKER_ENV, marker);
    command
}

/// 空白（改行を含む）を落として探す。5.1 は捕捉した stderr のエラーを画面の幅で折り返すことがあり、
/// 長いパスが行をまたいでも名指しを読めるようにする
fn contains_loose(haystack: &str, needle: &str) -> bool {
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    squash(haystack).contains(&squash(needle))
}

fn read_marker(marker: &Path) -> Option<String> {
    std::fs::read(marker)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// ステップが置いたポリシーが**実際に効いている**ことを確かめる（効いていないまま新旧を比べると偽の緑になる）
fn assert_policy_effective(program: &str, scope: &str, value: &str) {
    let mut command = Command::new(program);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-Command",
        "'EFFECTIVE=' + (Get-ExecutionPolicy); Get-ExecutionPolicy -List | ForEach-Object { 'LIST=' + $_.Scope + '=' + $_.ExecutionPolicy }",
    ]);
    let ran = run(command, "Get-ExecutionPolicy");
    println!("TAKO2790_POLICY program={program} {}", ran.summary());
    assert!(
        ran.stdout.contains(&format!("EFFECTIVE={value}")),
        "{program}: 置いたポリシー {scope}={value} が効いていない: {}",
        ran.summary()
    );
    assert!(
        ran.stdout.contains(&format!("LIST={scope}={value}")),
        "{program}: {scope} に {value} が見えない: {}",
        ran.summary()
    );
}

/// 受け入れ条件 2 の前半: 新しい形はポリシーを強制した状態でも偽のインストーラを走らせて印を残す
/// （ふだんの全数では既定のポリシーで同じことを見る）
#[test]
fn issue2790_新しい起こし方は偽のインストーラを走らせて印を残す() {
    let policy = enforced_policy();
    let dir = ScratchDir::new("i2790-new");
    let script = write_fake(&dir);
    let mut programs: Vec<Option<String>> = vec![None];
    programs.extend(interpreters().into_iter().map(Some));
    for (i, program) in programs.iter().enumerate() {
        let label = program.as_deref().unwrap_or("product(installer_command)");
        if let (Some(program), Some((scope, value))) = (program, &policy) {
            assert_policy_effective(program, scope, value);
        }
        let marker = dir.join(format!("marker-new-{i}.txt"));
        let ran = run(new_form(&script, program.as_deref(), &marker), label);
        let got = read_marker(&marker);
        println!(
            "TAKO2790_NEW policy={:?} program={label} marker={got:?} {}",
            policy,
            ran.summary()
        );
        assert_eq!(ran.code, Some(0), "{label}: {}", ran.summary());
        assert_eq!(
            got.as_deref(),
            Some(EXPECTED_MARKER),
            "{label}: 偽のインストーラが走っていないか本文が化けた: {}",
            ran.summary()
        );
        assert!(
            ran.stdout.contains("fake installer done 2790"),
            "{label}: {}",
            ran.summary()
        );
    }
}

/// 受け入れ条件 2 の後半: 修正前の形（`-ExecutionPolicy Bypass -File`）はポリシーを強制した状態で弾かれる。
/// ポリシーを置かないふだんの全数では**同じ偽物が修正前の形でも走る**ことを見る（= 弾かれるのは
/// 偽物が壊れているからではなく、ポリシーのせいだと分かる）
#[test]
fn issue2790_修正前の起こし方はポリシーを強制した状態で弾かれる() {
    let policy = enforced_policy();
    let dir = ScratchDir::new("i2790-legacy");
    let script = write_fake(&dir);
    for (i, program) in interpreters().iter().enumerate() {
        let marker = dir.join(format!("marker-legacy-{i}.txt"));
        let ran = run(legacy_form(&script, program, &marker), program);
        let got = read_marker(&marker);
        println!(
            "TAKO2790_LEGACY policy={:?} program={program} marker={got:?} {}",
            policy,
            ran.summary()
        );
        match &policy {
            Some((scope, value)) => {
                assert_ne!(
                    ran.code,
                    Some(0),
                    "{program}: {scope}={value} の下で修正前の形が通ってしまった: {}",
                    ran.summary()
                );
                assert!(
                    got.is_none(),
                    "{program}: {scope}={value} の下で修正前の形が偽のインストーラを走らせた: {}",
                    ran.summary()
                );
            }
            None => {
                assert_eq!(
                    ran.code,
                    Some(0),
                    "{program}: ポリシーが無いのに修正前の形が走らない: {}",
                    ran.summary()
                );
                assert!(got.is_some(), "{program}: {}", ran.summary());
            }
        }
    }
}

/// 終了コードは `-File` で走らせたときと同じ（`exit N` → N・終端エラー → 1・最後まで → 0）。
/// ポリシーを置かないときは修正前の形と 1 件ずつ突き合わせる
#[test]
fn issue2790_終了コードはfileで走らせたときと同じ() {
    let policy = enforced_policy();
    let dir = ScratchDir::new("i2790-exit");
    let script = write_fake(&dir);
    let cases: [(&str, Option<(&str, &str)>, i32); 3] = [
        ("exit7", Some(("TAKO_2790_EXIT", "7")), 7),
        ("throw", Some(("TAKO_2790_THROW", "1")), 1),
        ("done", None, 0),
    ];
    for program in interpreters() {
        for (name, env, expected) in cases {
            let marker = dir.join(format!("marker-exit-{name}.txt"));
            let mut command = new_form(&script, Some(&program), &marker);
            if let Some((key, value)) = env {
                command.env(key, value);
            }
            let ran = run(command, name);
            println!(
                "TAKO2790_EXIT case={name} program={program} {}",
                ran.summary()
            );
            assert_eq!(
                ran.code,
                Some(expected),
                "{program} {name}: {}",
                ran.summary()
            );
            if name == "throw" {
                assert!(
                    contains_loose(&ran.stderr, "fake installer terminating error 2790"),
                    "{program}: 終端エラーの中身が stderr に出ていない: {}",
                    ran.summary()
                );
                // `-OutputFormat Text` を外すと、-EncodedCommand + stderr のリダイレクトでエラーが
                // CLIXML になり、GUI / MCP の代行の断りの文へ XML のまま載る
                assert!(
                    !ran.stderr.contains("#< CLIXML"),
                    "{program}: 捕捉したエラーが CLIXML になっている（平文でない）: {}",
                    ran.summary()
                );
            }
            if policy.is_none() {
                let mut legacy = legacy_form(&script, &program, &marker);
                if let Some((key, value)) = env {
                    legacy.env(key, value);
                }
                let old = run(legacy, name);
                println!(
                    "TAKO2790_EXIT_LEGACY case={name} program={program} {}",
                    old.summary()
                );
                assert_eq!(
                    old.code,
                    ran.code,
                    "{program} {name}: 修正前の形と終了コードが違う（新 {} / 旧 {}）",
                    ran.summary(),
                    old.summary()
                );
            }
        }
    }
}

/// 読めない・PowerShell でないものは理由つきで 1（呼び手の `InstallerFailed` に出る）
#[test]
fn issue2790_読めないスクリプトは理由つきで1で終わる() {
    let dir = ScratchDir::new("i2790-unreadable");
    let missing = dir.join("missing-2790.ps1");
    let html = dir.join("html-2790.ps1");
    std::fs::write(
        &html,
        b"<!DOCTYPE html>\n<html><body>403 Forbidden</body></html>\n",
    )
    .expect("HTML を書けない");
    for program in interpreters() {
        // HTML は 5.1 では scriptblock を組む段（構文エラー）、pwsh 7 では走らせた段（`<` というコマンドが
        // 無い）で止まる。どちらもエラーの文が先頭の `<` を引くので、それを名指しの印にする
        for (name, script, needle) in [
            ("missing", &missing, "missing-2790.ps1"),
            ("html", &html, "<"),
        ] {
            let marker = dir.join(format!("marker-{name}.txt"));
            let ran = run(new_form(script, Some(&program), &marker), name);
            println!(
                "TAKO2790_UNREADABLE case={name} program={program} {}",
                ran.summary()
            );
            assert_eq!(ran.code, Some(1), "{program} {name}: {}", ran.summary());
            assert!(
                !ran.stderr.contains("#< CLIXML"),
                "{program} {name}: 捕捉したエラーが CLIXML になっている: {}",
                ran.summary()
            );
            assert!(
                contains_loose(&ran.stderr, needle),
                "{program} {name}: 理由が stderr に出ていない: {}",
                ran.summary()
            );
        }
    }
}

/// 偽のインストーラを 1 本だけ返すローカルの HTTP（`irm <url> | iex` の案内の 1 行を、URL だけ差し替えて
/// 走らせるため。外へは出ない）。スレッドはテストのプロセスが終わるまで残る
fn serve_fake_installer() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ローカルの HTTP を開けない");
    let port = listener.local_addr().expect("port").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&chunk[..n]),
                }
            }
            let body = FAKE_INSTALLER.as_bytes();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}/install.ps1")
}

/// 失敗したときに案内する手で打つ 1 行（公式の `irm <url> | iex`）も、ポリシーを強制した状態で走る。
/// 案内の文字列は製品のレシピのものを使い、取得元の URL だけをローカルの偽物へ差し替える
#[test]
fn issue2790_手で打つ案内の1行もポリシーの下で走る() {
    let policy = enforced_policy();
    let dir = ScratchDir::new("i2790-guide");
    let url = serve_fake_installer();
    let r = claude_recipe();
    let line = r.source.official_command.replace(r.source.url, &url);
    assert_ne!(line, r.source.official_command, "案内に取得元の URL が無い");
    for (i, program) in interpreters().iter().enumerate() {
        let marker = dir.join(format!("marker-guide-{i}.txt"));
        let mut command = Command::new(program);
        // 利用者がプロンプトへ打つのと同じく、コマンドとして渡す（スクリプトファイルは作らない）
        command
            .args(["-NoLogo", "-NoProfile", "-Command", &line])
            .env(MARKER_ENV, &marker);
        let ran = run(command, &line);
        let got = read_marker(&marker);
        println!(
            "TAKO2790_GUIDE policy={:?} program={program} line=[{line}] marker={got:?} {}",
            policy,
            ran.summary()
        );
        assert_eq!(ran.code, Some(0), "{program}: {}", ran.summary());
        assert_eq!(
            got.as_deref(),
            Some(EXPECTED_MARKER),
            "{program}: 案内の 1 行で偽のインストーラが走っていない: {}",
            ran.summary()
        );
    }
}
