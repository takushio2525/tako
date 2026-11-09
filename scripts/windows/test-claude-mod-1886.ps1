#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Windows の Claude Code が tako mod を読み、mod → tako mod report → 名前付きパイプ → tako の受け取りまで
    通るかを、本物の claude を起こして見張る（Issue #1886）。

.DESCRIPTION
    tako mod（FR-2.42）はペインの env CLAUDE_CODE_PLUGIN_DIRS（Windows の区切りは ;）と、setup が置く
    <設定 dir>/skills/tako/ の写しから Claude Code に読ませる。macOS では実測済みだが、Windows の Claude Code で
    読まれるか・mod から tako の CLI（TAKO_CLI の絶対パス）を叩いて名前付きパイプの IPC で届くか・ConPTY の
    ペインへの env 注入が効くかは未実測だった（#1886）。Windows の実機は当面使えないので、使い捨ての
    windows-latest ランナーで本物の claude を起こして測る（#2433 と同じ土台）。

    このスクリプトがやること:
      1. 砂場（USERPROFILE / APPDATA / LOCALAPPDATA / TEMP）へ claude を製品の口 `tako setup bootstrap install`
         （MCP tako_setup_bootstrap と同じ setup_bootstrap::install_for）で入れ、空白と日本語を含む設定 dir へ
         `tako mod install`（MCP tako_mod の install と同じ run_action）で skills\tako の写しを置く。このとき claude の版を
         GUI と同じ bounded_claude_version で判定する（埋まらないと注入も写しも止まる）
      2. 偽の Messages API（lib/fake-anthropic-2433.ps1）を立てる（認証は使わない）
      3. crates/tako-control/tests/issue1886_claude_mod_windows.rs を --ignored で名指しで回す。テストは GUI と同じ
         部品（展開・注入の 3 段・ConPTY・名前付きパイプの IpcServer・ModHub）で組み、4 つの組み合わせ
         （env だけ / env の ; 連結 + ペインの PowerShell から打つ / skills/tako の写しだけ / 注入しない対照）で
         claude を起こして、報告の到着・mod_turn・ターン状態の遷移・/exit の ended を見る
      4. 偽の API が受けた要求の要約を TAKO1886_API の行で残す

    結果はテストの TAKO1886_* の行と末尾の表で CI のログへ残る。ユーザー環境変数 Path（HKCU。導入が足す）は
    finally で戻して、戻ったことまで確かめる。そのため GitHub Actions の使い捨てランナー以外では走らせない。

.PARAMETER Tako
    製品の口で claude を入れ、テストが TAKO_CLI として写す tako.exe。

.EXAMPLE
    pwsh -File scripts/windows/test-claude-mod-1886.ps1 -Tako target\debug\tako.exe
#>
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Tako)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用（本物の claude を入れ、ConPTY と名前付きパイプで測る）' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（ユーザー環境変数 Path を書き換えて戻す。実機では走らせない）'
}

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $RepoRoot 'installer/windows/lib/user-path.ps1')
$FakeApi = Join-Path $PSScriptRoot 'lib/fake-anthropic-2433.ps1'
$TestName = 'issue1886_windows_claude_loads_tako_mod_and_reports'

if (-not (Test-Path -LiteralPath $Tako -PathType Leaf)) { throw "tako.exe が無い: $Tako（先に cargo build -p tako-cli）" }
$TakoExe = (Resolve-Path -LiteralPath $Tako).ProviderPath

$script:Stage = '準備'
function Fail([string]$Message) { throw "[#1886 $script:Stage] $Message" }

function Format-OneLine([string]$Text, [int]$Max = 400) {
    $t = ($Text -replace '\s+', ' ').Trim()
    if ($t.Length -gt $Max) { $t = $t.Substring(0, $Max) + '…' }
    $t
}

# --- 砂場 ---------------------------------------------------------------------
$RunRoot = Join-Path ([System.IO.Path]::GetTempPath()) "tako-1886-$([System.IO.Path]::GetRandomFileName())"
$homeDir = Join-Path $RunRoot 'home'
$Sandbox = [pscustomobject]@{
    Home         = $homeDir
    AppData      = (Join-Path $homeDir 'AppData\Roaming')
    LocalAppData = (Join-Path $homeDir 'AppData\Local')
    Tmp          = (Join-Path $RunRoot 'tmp')
    Work         = (Join-Path $RunRoot 'work')
}
foreach ($d in @($Sandbox.AppData, $Sandbox.LocalAppData, $Sandbox.Tmp, $Sandbox.Work)) {
    New-Item -ItemType Directory -Path $d -Force | Out-Null
}
$ClaudeExe = Join-Path $Sandbox.Home '.local\bin\claude.exe'

# 導入だけを砂場の env で起こす（テストの cargo はランナーの env のまま = ツールチェーンを取り直さない）
function Invoke-Sandboxed([string]$FilePath, [string[]]$Arguments, [int]$TimeoutSec = 900, [hashtable]$ExtraEnv) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($FilePath)
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.WorkingDirectory = $Sandbox.Work
    foreach ($k in @($psi.Environment.Keys)) {
        # PSModulePath は外す（pwsh 7 の置き場を受け継いだ 5.1 は標準モジュールを読めず、install.ps1 の Get-FileHash が無くなる）
        if ($k -like 'TAKO_*' -or $k -like 'ANTHROPIC_*' -or $k -like 'CLAUDE_*' -or $k -in @('HOME', 'CLAUDECODE', 'PSModulePath')) {
            [void]$psi.Environment.Remove($k)
        }
    }
    $psi.Environment['USERPROFILE'] = $Sandbox.Home
    $psi.Environment['APPDATA'] = $Sandbox.AppData
    $psi.Environment['LOCALAPPDATA'] = $Sandbox.LocalAppData
    $psi.Environment['TEMP'] = $Sandbox.Tmp
    $psi.Environment['TMP'] = $Sandbox.Tmp
    $psi.Environment['TAKO_LANG'] = 'ja'
    $psi.Environment['DISABLE_AUTOUPDATER'] = '1'
    if ($ExtraEnv) { foreach ($k in $ExtraEnv.Keys) { $psi.Environment[$k] = [string]$ExtraEnv[$k] } }
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        $p.StandardInput.Close()
        if (-not $p.WaitForExit($TimeoutSec * 1000)) { $p.Kill($true); Fail "$FilePath $($Arguments -join ' ') が $TimeoutSec 秒で終わらない" }
        $p.WaitForExit()
        [pscustomobject]@{ Code = $p.ExitCode; Out = $out.Result; Err = $err.Result }
    } finally {
        $p.Dispose()
    }
}

# --- 偽の Messages API --------------------------------------------------------
function Start-FakeApi([string]$LogDir) {
    $tcp = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $tcp.Start()
    $port = $tcp.LocalEndpoint.Port
    $tcp.Stop()
    $ready = Join-Path $LogDir 'ready'
    New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
    $p = Start-Process -FilePath (Get-Process -Id $PID).Path -PassThru -NoNewWindow `
        -ArgumentList @('-NoProfile', '-File', $FakeApi, '-Port', $port, '-LogDir', $LogDir, '-ReadyFile', $ready) `
        -RedirectStandardOutput (Join-Path $LogDir 'server.out') -RedirectStandardError (Join-Path $LogDir 'server.err')
    $deadline = (Get-Date).AddSeconds(20)
    while (-not (Test-Path -LiteralPath $ready)) {
        if ($p.HasExited -or (Get-Date) -gt $deadline) {
            $err = Get-Content -LiteralPath (Join-Path $LogDir 'server.err') -Raw -ErrorAction SilentlyContinue
            Fail "偽の API が立たない（exit $(if ($p.HasExited) { $p.ExitCode } else { 'running' })）: $err"
        }
        Start-Sleep -Milliseconds 200
    }
    [pscustomobject]@{ Process = $p; Port = $port; LogDir = $LogDir }
}

$Before = Get-TakoUserPath
Write-Host '== Windows の Claude Code が tako mod を読み、報告が tako へ届くか（#1886）=='
Write-Host "   OS        : $([System.Environment]::OSVersion.VersionString)"
Write-Host "   導入      : tako setup bootstrap install（$TakoExe）"

$failures = @()
$fake = $null
$testExit = $null
try {
    $script:Stage = '導入'
    $r = Invoke-Sandboxed -FilePath $TakoExe -Arguments @('setup', 'bootstrap', 'install')
    $tail = @(($r.Out + "`n" + $r.Err) -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | Select-Object -Last 4)
    foreach ($line in $tail) { Write-Host "TAKO1886_INSTALL_OUTPUT | $(Format-OneLine $line)" }
    if ($r.Code -ne 0) { Fail "tako setup bootstrap install が終了コード $($r.Code)" }
    if (-not (Test-Path -LiteralPath $ClaudeExe -PathType Leaf)) { Fail "claude.exe が置き場に無い: <home>\.local\bin\claude.exe" }
    Write-Host "TAKO1886_INSTALL exit=$($r.Code)"

    # skills/tako の写し: 製品の口 `tako mod install`（MCP tako_mod の install と同じ run_action）で、空白と日本語を
    # 含む設定 dir へ置く。claude の版は GUI と同じ bounded_claude_version（PATH・~\.local\bin・PE の版情報・
    # claude --version）で決まる = Windows で版が埋まらないと写しも注入も止まる（OffReason::ClaudeUnknown）
    $script:Stage = 'tako mod install'
    $SkillsCfg = Join-Path $RunRoot 'tako 1886 データ\cfg skills-cli'
    New-Item -ItemType Directory -Path $SkillsCfg -Force | Out-Null
    $userBin = Join-Path $Sandbox.Home '.local\bin'
    $m = Invoke-Sandboxed -FilePath $TakoExe -Arguments @('mod', 'install', '--json') -TimeoutSec 180 `
        -ExtraEnv @{ CLAUDE_CONFIG_DIR = $SkillsCfg; PATH = "$userBin;$env:SystemRoot\System32;$env:SystemRoot" }
    $masked = ($m.Out -replace [regex]::Escape($RunRoot), '<run>') -replace [regex]::Escape($RunRoot.Replace('\', '\\')), '<run>'
    Write-Host "TAKO1886_MOD_INSTALL exit=$($m.Code) stderr=[$(Format-OneLine $m.Err 300)]"
    foreach ($line in @($masked -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | Select-Object -First 60)) { Write-Host "TAKO1886_MOD_INSTALL | $line" }
    if ($m.Code -ne 0) { Fail "tako mod install が終了コード $($m.Code)" }
    $installed = $m.Out | ConvertFrom-Json -Depth 32
    if (-not $installed.claude_version) { Fail 'tako mod install の claude_version が空（Windows で claude の版を判定できない = 注入も写しも止まる）' }
    if (-not (Test-Path -LiteralPath (Join-Path $SkillsCfg 'skills\tako\.tako-managed') -PathType Leaf)) {
        Fail '設定 dir に skills\tako の写し（管理印 .tako-managed）が置かれていない'
    }
    Write-Host "TAKO1886_MOD_INSTALL claude_version=$($installed.claude_version) min=$($installed.min_claude_version)"

    $script:Stage = '偽の API'
    $fake = Start-FakeApi (Join-Path $RunRoot 'api')

    $script:Stage = '実測'
    $env:TAKO_1886_CLAUDE_EXE = $ClaudeExe
    $env:TAKO_1886_TAKO_EXE = $TakoExe
    $env:TAKO_1886_API_URL = "http://127.0.0.1:$($fake.Port)"
    $env:TAKO_1886_HOME = $Sandbox.Home
    $env:TAKO_1886_SKILLS_CFG = $SkillsCfg
    Push-Location $RepoRoot
    try {
        & cargo test -p tako-control --test issue1886_claude_mod_windows -- --ignored --exact $TestName --nocapture --test-threads=1
        $testExit = $LASTEXITCODE
    } finally {
        Pop-Location
        foreach ($k in @('TAKO_1886_CLAUDE_EXE', 'TAKO_1886_TAKO_EXE', 'TAKO_1886_API_URL', 'TAKO_1886_HOME', 'TAKO_1886_SKILLS_CFG')) {
            Remove-Item -LiteralPath "Env:$k" -ErrorAction SilentlyContinue
        }
    }
    if ($testExit -ne 0) { $failures += "[#1886 実測] $TestName が終了コード $testExit（上の TAKO1886_* の行と表を見る）" }
} catch {
    $failures += $(if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { [string]$_ })
} finally {
    $script:Stage = '後片付け'
    if ($fake -and -not $fake.Process.HasExited) { Stop-Process -Id $fake.Process.Id -Force -ErrorAction SilentlyContinue }
    if ($fake) {
        $log = Join-Path $fake.LogDir 'requests.log'
        if (Test-Path -LiteralPath $log) {
            foreach ($line in @(Get-Content -LiteralPath $log -Encoding utf8)) { Write-Host "TAKO1886_API | $line" }
        }
    }
    # 砂場の中で起きたプロセス（claude・tako の CLI）だけを止める
    Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($RunRoot, [System.StringComparison]::OrdinalIgnoreCase) } catch { $false }
    } | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    Restore-TakoUserPath -Snapshot $Before
    $afterPath = Get-TakoUserPath
    if ($afterPath.Raw -ne $Before.Raw -or $afterPath.Exists -ne $Before.Exists) {
        $failures += '[#1886 後片付け] ユーザー環境変数 Path を検査前へ戻せなかった'
    }
}

Remove-Item -LiteralPath $RunRoot -Recurse -Force -ErrorAction SilentlyContinue
if ($failures.Count -gt 0) {
    foreach ($f in $failures) { [Console]::WriteLine("::error::$f") }
    exit 1
}
Write-Host 'TAKO1886_SCRIPT ok'
exit 0
