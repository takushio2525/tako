//! Windows の codex / agy の導入代行が、公式の `irm … | iex` と同じスコープでインストーラを走らせるか（#2791）。
//! **実際に PowerShell を起こして測る**。
//!
//! #2790 の起動片は取得した `install.ps1` を子スコープ（`& ([scriptblock]::Create(…))`）で走らせていた。
//! 子スコープでは、スクリプトの最上位の変数と `$script:` の変数が別の場所を指す（`-File` はスクリプトの
//! スコープ、`irm | iex` は今のスコープで、どちらも同じ 1 つ）。agy の `install.ps1`（2026-11-08 の実物）は
//! 関数の中で `$script:stagingPayload` へ取得物のパスを書き、最上位の `finally` で素の `$stagingPayload` を
//! 読んで片付けるので、子スコープでは片付けが飛んで `%LOCALAPPDATA%\antigravity\staging\agy.exe` が残る。
//! #2791 から起動片はドットソース（`.`）で走らせる（`platform::shell::powershell_script_text_command`）。
//!
//! 偽のインストーラは agy の実物の骨組み（`$isSourced` の判定・`$script:installExitCode`・
//! `$script:stagingPayload` と `finally` の片付け・既に在れば 0）と、codex の実物の
//! `$script:guardRejected`（関数へ渡した scriptblock の中で書き、最上位で読む）を写したもの。
//! 外部の本物の `install.ps1` は走らせない（本物の実走は `scripts/windows/test-agent-install-2791.ps1`）。
//!
//! 起こし方は製品の 1 実装（`InstallRecipe::installer_command` = `setup_bootstrap::run_installer` と同じ口）
//! を通し、インタプリタは製品の候補（`powershell` → `pwsh`）のうちこの環境に在るものすべてで見る。
//! 旧形（子スコープ）は `powershell_script_text_command_in(script, true)` で組み、同じ偽物で
//! 「片付けが飛ぶ」ことを示す（偽物が壊れているのではなく、スコープのせいだと分かる）。
//!
//! CI の `scripts/windows/test-agent-install-2791.ps1` は、ふだんの全数より手前でこのテストを
//! ポリシーなし・HKCU Restricted・HKLM AllSigned の 3 通りで走らせる（`TAKO_2791_POLICY=<スコープ>:<値>`。
//! 値は結果の行へ写すだけ。ポリシーが効いていることの確かめは #2790 の検査が持つ）。
//!
//! 結果の行（`TAKO2791_*`）を `--nocapture` で CI のログへ残す。

#![cfg(windows)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tako_core::platform::agent_install::{recipe, AgentKind, InstallRecipe};
use tako_core::platform::shell::{
    encode_powershell_command, powershell_script_text_command_in, powershell_stderr_text,
};
use tako_core::platform::support::Platform;
use tako_core::test_residue::ScratchDir;

/// CI のステップが置いたポリシー（結果の行へ写すだけ）
const POLICY_ENV: &str = "TAKO_2791_POLICY";

/// 偽のインストーラが置き場に使う dir（テストが子へ渡す。本物の `$env:LOCALAPPDATA` の代わり）
const ROOT_ENV: &str = "TAKO_2791_ROOT";

/// 1 回の PowerShell の起動を待つ上限（返らない回にテストごと固まらない）
const RUN_LIMIT: Duration = Duration::from_secs(120);

/// 偽の agy の `install.ps1`（BOM なし UTF-8）。実物の骨組みを写し、置き場だけを `$env:TAKO_2791_ROOT` にした。
/// `TAKO_2791_FAIL` で途中（取得物を置いた後）で失敗する
const FAKE_AGY_INSTALLER: &str = r#"$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$hasPath = ($null -ne $MyInvocation.MyCommand) -and ($null -ne $MyInvocation.MyCommand.PSObject.Properties['Path'])
$scriptPath = if ($hasPath) { $MyInvocation.MyCommand.Path } else { $null }
$isSourced = [string]::IsNullOrEmpty($scriptPath) -or ($MyInvocation.InvocationName -eq '.')

$stagingPayload = $null
$script:installExitCode = 0

function Invoke-Install {
    $TARGET_DIR = Join-Path $env:TAKO_2791_ROOT "agy\bin"
    $binaryPath = Join-Path $TARGET_DIR "agy.exe"
    if (Test-Path $binaryPath) {
        Write-Host "Notice: 'agy.exe' is already installed at $binaryPath."
        $script:installExitCode = 0; return
    }
    $stagingDir = Join-Path $env:TAKO_2791_ROOT "antigravity\staging"
    if (-not (Test-Path $stagingDir)) { New-Item -ItemType Directory -Path $stagingDir | Out-Null }
    $localStagingPayload = Join-Path $stagingDir "agy.exe"
    $script:stagingPayload = $localStagingPayload
    [System.IO.File]::WriteAllText($localStagingPayload, "fake agy 2791")
    if ($env:TAKO_2791_FAIL) {
        Write-Error "Write Error: fake failure 2791 after staging." -ErrorAction Continue
        if ($isSourced) { throw "Fatal: Failed to install binary." } else { $script:installExitCode = 1; return }
    }
    if (-not (Test-Path $TARGET_DIR)) { New-Item -ItemType Directory -Path $TARGET_DIR | Out-Null }
    Copy-Item -Path $localStagingPayload -Destination $binaryPath -Force
    $script:installExitCode = 0
    return
}

$script:installExitCode = 0
try {
    Invoke-Install $args
} finally {
    if ($null -ne $stagingPayload -and (Test-Path $stagingPayload)) {
        Remove-Item $stagingPayload -Force
    }
}
$exitCode = $script:installExitCode
if ($exitCode -ne 0) {
    if ($isSourced) { throw "Fatal: Installation failed." } else { exit $exitCode }
}
Write-Output "fake agy installer done 2791 sourced=$isSourced"
"#;

/// 偽の codex の `install.ps1` の骨組み（実物の `[CmdletBinding()] param()` と、`Invoke-WithInstallLock` へ
/// 渡した scriptblock の中で `$script:guardRejected` を書いて最上位で読む形）。読めた値を印のファイルへ書く
const FAKE_CODEX_INSTALLER: &str = r#"[CmdletBinding()]
param(
    [string]$Release = $env:CODEX_RELEASE
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Release)) { $Release = "latest" }
$NonInteractive = $env:CODEX_NON_INTERACTIVE -match "^(?i:1|true|yes)$"

function Invoke-WithInstallLock {
    param([scriptblock]$Script)
    & $Script
}

$guardRejected = $false
Invoke-WithInstallLock -Script {
    $script:guardRejected = $true
}
$line = "release=$Release noninteractive=$NonInteractive guardRejected=$guardRejected"
[System.IO.File]::WriteAllText((Join-Path $env:TAKO_2791_ROOT "codex-marker.txt"), $line)
if ($guardRejected) { return }
Write-Output "fake codex installer reached the end 2791"
"#;

fn policy() -> Option<String> {
    std::env::var(POLICY_ENV).ok()
}

/// 製品の候補（`powershell` → `pwsh`）のうち、この環境に在るもの
fn interpreters() -> Vec<String> {
    let found: Vec<String> = recipe(Platform::Windows, AgentKind::Agy)
        .runner
        .candidates
        .iter()
        .filter_map(|name| tako_core::platform::exe::find(name))
        .collect();
    assert!(!found.is_empty(), "powershell も pwsh も見つからない");
    found
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
            powershell_stderr_text(&self.stderr).trim()
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

/// 起こし方の 2 形
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    /// 製品（#2791 から。ドットソース）
    Product,
    /// #2791 より前の子スコープ（`&`）
    LegacyChildScope,
}

/// 偽物を起こす `Command`。`program` が `None` なら製品の解決どおり（`installer_command`）
fn command_for(
    r: &InstallRecipe,
    script: &Path,
    program: Option<&str>,
    form: Form,
    root: &Path,
) -> Command {
    let mut command = match (form, program) {
        (Form::Product, None) => r.installer_command(script),
        (Form::Product, Some(program)) => {
            let mut command = Command::new(program);
            command.args(r.runner.args_for_in(script, false));
            command
        }
        (Form::LegacyChildScope, program) => {
            let program = program.expect("旧形はインタプリタを名指しして起こす");
            // 製品の起動引数のうち、符号化した本文だけを旧形（子スコープ）へ差し替える
            let mut args: Vec<std::ffi::OsString> = r.runner.args_for_in(script, false);
            let last = args.len() - 1;
            args[last] =
                encode_powershell_command(&powershell_script_text_command_in(script, true)).into();
            let mut command = Command::new(program);
            command.args(args);
            command
        }
    };
    command.env(ROOT_ENV, root);
    // cargo を起こした pwsh 7 の PSModulePath を受け継ぐと、Windows PowerShell 5.1 が pwsh 7 の置き場の
    // モジュールを拾って読めないことがある（#2790 の CI で実測）。利用者の環境（エクスプローラーから起動した
    // tako）にこの値は無いので、外して各 PowerShell の既定へ戻す
    command.env_remove("PSModulePath");
    command
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    // 置き場の名前に日本語・空白・単引用符を入れる（#2790 と同じく、起動片がパスを崩さないことも見る）
    let sub = dir.join("山田 'q' 2791");
    std::fs::create_dir_all(&sub).expect("置き場を作れない");
    let script = sub.join(name);
    std::fs::write(&script, body.as_bytes()).expect("偽のインストーラを書けない");
    script
}

/// 走らせる組（ラベル, インタプリタ）。製品の解決どおり（`None`）と、在るインタプリタそれぞれ
fn product_runs() -> Vec<(String, Option<String>)> {
    let mut runs = vec![("product(installer_command)".to_string(), None)];
    runs.extend(interpreters().into_iter().map(|p| (p.clone(), Some(p))));
    runs
}

/// 受け入れ条件 2: 製品の起こし方で agy の偽物が最後まで走り、取得物（staging）が片付く。
/// 旧形（子スコープ）では同じ偽物で取得物が残る = スコープの違いが原因だと分かる
#[test]
fn issue2791_agyのインストーラは取得物を片付けて終わる() {
    let dir = ScratchDir::new("i2791-agy");
    let script = write_script(&dir, "tako-agy-install-2791.ps1", FAKE_AGY_INSTALLER);
    let r = recipe(Platform::Windows, AgentKind::Agy);
    for (i, (label, program)) in product_runs().into_iter().enumerate() {
        let root = dir.join(format!("root-new-{i}"));
        std::fs::create_dir_all(&root).expect("root");
        let ran = run(
            command_for(&r, &script, program.as_deref(), Form::Product, &root),
            &label,
        );
        let staging = root.join("antigravity").join("staging").join("agy.exe");
        let binary = root.join("agy").join("bin").join("agy.exe");
        println!(
            "TAKO2791_AGY_NEW policy={:?} program={label} staging_left={} binary={} {}",
            policy(),
            staging.exists(),
            binary.exists(),
            ran.summary()
        );
        assert_eq!(ran.code, Some(0), "{label}: {}", ran.summary());
        assert!(
            binary.exists(),
            "{label}: 置き場に agy.exe が無い: {}",
            ran.summary()
        );
        assert!(
            !staging.exists(),
            "{label}: 取得物（staging）が残った = $script: と最上位の変数が別のスコープを指している（#2791）: {}",
            ran.summary()
        );
        // irm | iex と同じく、スクリプトのパスを持たない（= isSourced）
        assert!(
            ran.stdout
                .contains("fake agy installer done 2791 sourced=True"),
            "{label}: {}",
            ran.summary()
        );
    }
    for (i, program) in interpreters().iter().enumerate() {
        let root = dir.join(format!("root-old-{i}"));
        std::fs::create_dir_all(&root).expect("root");
        let ran = run(
            command_for(&r, &script, Some(program), Form::LegacyChildScope, &root),
            program,
        );
        let staging = root.join("antigravity").join("staging").join("agy.exe");
        println!(
            "TAKO2791_AGY_LEGACY policy={:?} program={program} staging_left={} {}",
            policy(),
            staging.exists(),
            ran.summary()
        );
        assert_eq!(ran.code, Some(0), "{program}: {}", ran.summary());
        assert!(
            staging.exists(),
            "{program}: 旧形（子スコープ）でも取得物が片付いた = 偽物がスコープの違いを再現していない: {}",
            ran.summary()
        );
    }
}

/// 境界: 途中で失敗したら終了コード 1（呼び手の `InstallerFailed`）で、理由が平文で stderr に出て、
/// 取得物は片付く（`finally` の片付けは失敗の経路でも効く）
#[test]
fn issue2791_agyのインストーラが途中で失敗しても理由つきで1になり取得物は片付く() {
    let dir = ScratchDir::new("i2791-agy-fail");
    let script = write_script(&dir, "tako-agy-install-2791.ps1", FAKE_AGY_INSTALLER);
    let r = recipe(Platform::Windows, AgentKind::Agy);
    for (i, (label, program)) in product_runs().into_iter().enumerate() {
        let root = dir.join(format!("root-{i}"));
        std::fs::create_dir_all(&root).expect("root");
        let mut command = command_for(&r, &script, program.as_deref(), Form::Product, &root);
        command.env("TAKO_2791_FAIL", "1");
        let ran = run(command, &label);
        let staging = root.join("antigravity").join("staging").join("agy.exe");
        let plain = powershell_stderr_text(&ran.stderr);
        println!(
            "TAKO2791_AGY_FAIL policy={:?} program={label} staging_left={} {}",
            policy(),
            staging.exists(),
            ran.summary()
        );
        assert_eq!(ran.code, Some(1), "{label}: {}", ran.summary());
        assert!(
            plain.contains("Fatal: Failed to install binary."),
            "{label}: 理由が stderr に出ていない: {}",
            ran.summary()
        );
        assert!(
            !plain.contains("<Objs"),
            "{label}: 平文へ戻らない: {}",
            ran.summary()
        );
        assert!(
            !staging.exists(),
            "{label}: 失敗の経路で取得物が残った（#2791）: {}",
            ran.summary()
        );
        assert!(
            !root.join("agy").join("bin").join("agy.exe").exists(),
            "{label}: 失敗したのに置き場へ置いた"
        );
    }
}

/// 境界: 既に入っていれば何もせず 0（agy の実物の冪等の形）。2 回目も同じ起こし方で通る
#[test]
fn issue2791_agyが既に入っていれば何もせず0で終わる() {
    let dir = ScratchDir::new("i2791-agy-again");
    let script = write_script(&dir, "tako-agy-install-2791.ps1", FAKE_AGY_INSTALLER);
    let r = recipe(Platform::Windows, AgentKind::Agy);
    let root = dir.join("root");
    std::fs::create_dir_all(&root).expect("root");
    for round in 1..=2 {
        let ran = run(
            command_for(&r, &script, None, Form::Product, &root),
            "product",
        );
        println!(
            "TAKO2791_AGY_AGAIN policy={:?} round={round} {}",
            policy(),
            ran.summary()
        );
        assert_eq!(ran.code, Some(0), "round {round}: {}", ran.summary());
        if round == 2 {
            assert!(
                ran.stdout.contains("is already installed")
                    || ran.stderr.contains("is already installed"),
                "2 回目は既に在ると言って終わる: {}",
                ran.summary()
            );
        }
    }
}

/// codex の骨組み: `[CmdletBinding()] param()` がドットソースでも束縛され（`$env:CODEX_RELEASE` の既定）、
/// レシピの `CODEX_NON_INTERACTIVE=1` が届き、関数へ渡した scriptblock の中の `$script:guardRejected` が
/// 最上位から見える（`-File` / `irm | iex` と同じ）。旧形（子スコープ）では見えない
#[test]
fn issue2791_codexのインストーラはparamとscriptスコープとenvを公式と同じに受ける() {
    let dir = ScratchDir::new("i2791-codex");
    let script = write_script(&dir, "tako-codex-install-2791.ps1", FAKE_CODEX_INSTALLER);
    let r = recipe(Platform::Windows, AgentKind::Codex);
    let runs: Vec<(String, Option<String>, Form)> = product_runs()
        .into_iter()
        .map(|(label, program)| (label, program, Form::Product))
        .chain(
            interpreters()
                .into_iter()
                .map(|p| (format!("{p}(legacy)"), Some(p), Form::LegacyChildScope)),
        )
        .collect();
    for (i, (label, program, form)) in runs.into_iter().enumerate() {
        let root = dir.join(format!("root-{i}"));
        std::fs::create_dir_all(&root).expect("root");
        let mut command = command_for(&r, &script, program.as_deref(), form, &root);
        // 旧形の比較でも製品と同じ env を渡す（レシピのデータ）
        for (key, value) in r.installer_env {
            command.env(key, value);
        }
        command.env("CODEX_RELEASE", "0.167.0");
        let ran = run(command, &label);
        let marker = std::fs::read_to_string(root.join("codex-marker.txt")).unwrap_or_default();
        println!(
            "TAKO2791_CODEX policy={:?} program={label} form={form:?} marker=[{marker}] {}",
            policy(),
            ran.summary()
        );
        assert_eq!(ran.code, Some(0), "{label}: {}", ran.summary());
        assert!(
            marker.starts_with("release=0.167.0 noninteractive=True "),
            "{label}: param の既定か非対話の env が届いていない: [{marker}]"
        );
        // 製品（ドットソース）は -File / irm | iex と同じく見える。子スコープ（旧形）だけ見えない
        let (expected, like) = match form {
            Form::Product => ("guardRejected=True", "-File / irm | iex"),
            Form::LegacyChildScope => ("guardRejected=False", "子スコープ"),
        };
        assert!(
            marker.ends_with(expected),
            "{label}: $script: の変数の見え方が {like} と違う: [{marker}]"
        );
    }
}
