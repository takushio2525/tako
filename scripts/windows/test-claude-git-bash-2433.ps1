#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Windows の Claude Code が Git Bash を要るかを、本物の claude を起こして確かめる（Issue #2433 の 2）。

.DESCRIPTION
    #2433 の調査時点で、tako の setup は Claude Code の Windows ネイティブ版が Git Bash（Git for Windows）を
    要るかを確かめていなかった。要るのに無い PC では「導入済み」と言った後で claude が止まるおそれがある。
    Windows の実機は当面使えないので、使い捨ての windows-latest ランナーで本物の claude を起こして測る。

    claude は公式の install.ps1 で砂場へ入れる（-Tako を渡したときは製品の口
    `tako setup bootstrap install` = MCP tako_setup_bootstrap と同じ setup_bootstrap::install_for で入れる）。
    入れるのは Git を隠した状態（Git の無い PC で入るか）。そのうえで次の組み合わせで claude を起こす:

      Git を隠す（C:\Program Files\Git の名前を一時的に変える）:
        no-git                  … PATH は Windows の標準の置き場だけ
        bash-path-env           … CLAUDE_CODE_GIT_BASH_PATH = 隠した先の bin\bash.exe（標準でない置き場）
        git-nonstandard-on-path … PATH に隠した先の cmd を足す（標準でない置き場の git が PATH にある）
        bash-path-missing       … CLAUDE_CODE_GIT_BASH_PATH = 存在しないファイル
        no-powershell-on-path   … PATH から WindowsPowerShell\v1.0 も外す
      Git を戻す:
        git-on-path             … PATH に C:\Program Files\Git\cmd を足す（Git の既定の導入と同じ）
        git-off-path            … PATH から Git を外す（Git は既定の置き場にある）
        bash-path-missing-git   … Git は既定の置き場にあり、CLAUDE_CODE_GIT_BASH_PATH は存在しないファイル

    1 つの組み合わせで起こすのは 3 回:
      1. claude --version
      2. claude -p（認証なし。本物の API へは行かない形で、起動のどこで止まるかを見る）
      3. claude -p を偽の Messages API（lib/fake-anthropic-2433.ps1。ダミーの API キー）へ向けて 1 ターン走らせる。
         偽の API はシェルのツール（Bash / PowerShell）の tool_use を返すので、claude がどのシェルのツールを
         出したか・そのツールで実際にコマンドが走ったか（走ったシェルが名乗る）までが分かる

    結果は TAKO2433_* の行と末尾の表で CI のログへ残す。期待（$Expect。2026-11-09 に claude 2.1.319 で実測）は
    「どの組み合わせでも 1 ターン走り、Git Bash が見つかれば Bash・無ければ PowerShell でコマンドが走る」
    = Git Bash は必須でない。上流が必須へ戻すと、Git の無い組み合わせが名指しで落ちる。
    no-powershell-on-path は記録だけ（claude は PATH に無くても既定の置き場の PowerShell を使う。期待に入れると
    利用者に関係の薄い上流の変化で赤になる）。

    -Tako を渡したときは各組み合わせで `tako setup bootstrap status --json` も読み、Git の有無で段が変わらない
    （Git Bash を段の判定に入れない）ことと、導入計画の shell_note が「Git for Windows は要りません」を言うことを見る。

    隔離: USERPROFILE / APPDATA / LOCALAPPDATA / TEMP は砂場、CLAUDE_CONFIG_DIR は組み合わせごとの空の dir、
    ANTHROPIC_* / CLAUDE_CODE_* / TAKO_* はランナーの env から外す。Git の名前の変更とユーザー環境変数 Path（HKCU）は
    finally で戻して、戻ったことまで確かめる。そのため GitHub Actions の使い捨てランナー以外では走らせない。

.PARAMETER Tako
    製品の口で入れて status も読むときの tako.exe。省略時は公式の install.ps1 を直接走らせ、status は読まない。

.EXAMPLE
    pwsh -File scripts/windows/test-claude-git-bash-2433.ps1 -Tako target\debug\tako.exe
#>
[CmdletBinding()]
param([string]$Tako)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用（本物の claude を入れ、Git の置き場の名前を変えて戻す）' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（C:\Program Files\Git の名前を変え、ユーザー環境変数 Path を書き換えて戻す。実機では走らせない）'
}

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $RepoRoot 'installer/windows/lib/user-path.ps1')
$FakeApi = Join-Path $PSScriptRoot 'lib/fake-anthropic-2433.ps1'

$TakoExe = $null
if ($Tako) {
    if (-not (Test-Path -LiteralPath $Tako -PathType Leaf)) { throw "tako.exe が無い: $Tako（先に cargo build -p tako-cli）" }
    $TakoExe = (Resolve-Path -LiteralPath $Tako).ProviderPath
}

$SysRoot = $env:SystemRoot
$PsDir = Join-Path $SysRoot 'System32\WindowsPowerShell\v1.0'
$StdPath = @((Join-Path $SysRoot 'System32'), $SysRoot, (Join-Path $SysRoot 'System32\Wbem'), $PsDir)
$GitRoot = Join-Path $env:ProgramFiles 'Git'
$GitHidden = Join-Path $env:ProgramFiles 'Git-tako2433'
$GitRootX86 = if (${env:ProgramFiles(x86)}) { Join-Path ${env:ProgramFiles(x86)} 'Git' } else { $null }

$script:Stage = '準備'
function Fail([string]$Message) { throw "[#2433 $script:Stage] $Message" }
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { Fail $Message } }

# ログへ残す 1 行（改行・連続空白を詰め、長すぎる分は切る）
function Format-OneLine([string]$Text, [int]$Max = 400) {
    $t = ($Text -replace '\s+', ' ').Trim()
    if ($t.Length -gt $Max) { $t = $t.Substring(0, $Max) + '…' }
    $t
}

# --- 砂場 ---------------------------------------------------------------------
$RunRoot = Join-Path ([System.IO.Path]::GetTempPath()) "tako-2433-$([System.IO.Path]::GetRandomFileName())"
$homeDir = Join-Path $RunRoot 'home'
$Sandbox = [pscustomobject]@{
    Root         = $RunRoot
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

function New-SandboxEnv([string[]]$PathDirs, [hashtable]$ExtraEnv) {
    $vars = @{}
    foreach ($entry in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
        $vars[[string]$entry.Key] = [string]$entry.Value
    }
    foreach ($k in @($vars.Keys)) {
        if ($k -like 'TAKO_*' -or $k -like 'ANTHROPIC_*' -or $k -like 'CLAUDE_*' -or
            $k -in @('HOME', 'CLAUDECODE', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'PSModulePath')) {
            $vars.Remove($k)
        }
    }
    $vars['USERPROFILE'] = $Sandbox.Home
    $vars['APPDATA'] = $Sandbox.AppData
    $vars['LOCALAPPDATA'] = $Sandbox.LocalAppData
    $vars['TEMP'] = $Sandbox.Tmp
    $vars['TMP'] = $Sandbox.Tmp
    $vars['PATH'] = $PathDirs -join ';'
    $vars['TAKO_LANG'] = 'ja'
    # 測っている間に claude が自分を更新しない・本物の API 以外の外向きの通信をしない
    $vars['DISABLE_AUTOUPDATER'] = '1'
    $vars['CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC'] = '1'
    if ($ExtraEnv) { foreach ($k in $ExtraEnv.Keys) { $vars[$k] = [string]$ExtraEnv[$k] } }
    $vars
}

function Invoke-Sandboxed {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string[]]$PathDirs = $StdPath,
        [hashtable]$ExtraEnv,
        [int]$TimeoutSec = 120
    )
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
    $psi.Environment.Clear()
    $vars = New-SandboxEnv $PathDirs $ExtraEnv
    foreach ($k in $vars.Keys) { $psi.Environment[$k] = $vars[$k] }

    $started = Get-Date
    $p = [System.Diagnostics.Process]::Start($psi)
    $timedOut = $false
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        $p.StandardInput.Close()
        if (-not $p.WaitForExit($TimeoutSec * 1000)) {
            $timedOut = $true
            $p.Kill($true)
        }
        $p.WaitForExit()
        $result = [PSCustomObject]@{
            Code     = if ($timedOut) { 'timeout' } else { $p.ExitCode }
            Out      = $out.Result
            Err      = $err.Result
            Seconds  = [Math]::Round(((Get-Date) - $started).TotalSeconds, 1)
            TimedOut = $timedOut
        }
    } finally {
        $p.Dispose()
    }
    Write-Host ("   > {0} {1}  (exit {2}, {3} 秒)" -f (Split-Path -Leaf $FilePath), ($Arguments -join ' '), $result.Code, $result.Seconds)
    $result
}

function Show-Result($Result, [string]$Label) {
    Write-Host "   --- $Label の stdout ---"
    $Result.Out -split "`r?`n" | Select-Object -First 40 | ForEach-Object { Write-Host "   | $_" }
    Write-Host "   --- $Label の stderr ---"
    $Result.Err -split "`r?`n" | Select-Object -First 40 | ForEach-Object { Write-Host "   | $_" }
}

# --- Git を隠す / 戻す ----------------------------------------------------------
$script:GitHiddenNow = $false
function Hide-Git {
    if ($GitRootX86 -and (Test-Path -LiteralPath $GitRootX86)) {
        Fail "32bit の Git も在る（$GitRootX86）。隠す対象が 2 つになるので測る前提が崩れている"
    }
    Assert-That (Test-Path -LiteralPath (Join-Path $GitRoot 'bin\bash.exe')) "ランナーに Git の bash.exe が無い: $GitRoot"
    Rename-Item -LiteralPath $GitRoot -NewName (Split-Path -Leaf $GitHidden)
    $script:GitHiddenNow = $true
    Assert-That (-not (Test-Path -LiteralPath $GitRoot)) "Git を隠せていない: $GitRoot"
    Write-Host "   Git を隠した: $GitRoot -> $GitHidden"
}
function Show-Git {
    if ($script:GitHiddenNow -and (Test-Path -LiteralPath $GitHidden) -and -not (Test-Path -LiteralPath $GitRoot)) {
        Rename-Item -LiteralPath $GitHidden -NewName (Split-Path -Leaf $GitRoot)
    }
    $script:GitHiddenNow = $false
    Assert-That (Test-Path -LiteralPath (Join-Path $GitRoot 'bin\bash.exe')) "Git を戻せていない: $GitRoot"
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
function Stop-FakeApi($Fake) {
    if ($Fake -and -not $Fake.Process.HasExited) { Stop-Process -Id $Fake.Process.Id -Force -ErrorAction SilentlyContinue }
}
function Read-Lines([string]$Path) {
    if (Test-Path -LiteralPath $Path) { @(Get-Content -LiteralPath $Path -Encoding utf8) } else { @() }
}

# --- 導入 ---------------------------------------------------------------------
function Install-Claude {
    $script:Stage = '導入（Git を隠した状態）'
    if ($TakoExe) {
        # 人が打つ形（--json なし）で入れて、導入後の案内に計画の 1 行が出ることも見る
        $r = Invoke-Sandboxed -FilePath $TakoExe -Arguments @('setup', 'bootstrap', 'install') -TimeoutSec 900
        if ($r.Code -ne 0) { Show-Result $r 'tako setup bootstrap install'; Fail "tako setup bootstrap install が終了コード $($r.Code)" }
        $said = $r.Out + "`n" + $r.Err
        if (-not ($said.Contains('インストールが完了しました') -and $said.Contains($ShellNoteNeedle))) {
            Show-Result $r 'tako setup bootstrap install'
            Fail "導入後の案内に「$ShellNoteNeedle」が無い"
        }
    } else {
        $r = Invoke-Sandboxed -FilePath (Join-Path $PsDir 'powershell.exe') -TimeoutSec 900 `
            -Arguments @('-NoProfile', '-Command', 'irm https://claude.ai/install.ps1 | iex')
        if ($r.Code -ne 0) { Show-Result $r 'irm https://claude.ai/install.ps1 | iex'; Fail "公式の install.ps1 が終了コード $($r.Code)" }
    }
    $tail = @(($r.Out + "`n" + $r.Err) -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | Select-Object -Last 8)
    foreach ($line in $tail) { Write-Host "TAKO2433_INSTALL_OUTPUT | $(Format-OneLine $line)" }
    Assert-That (Test-Path -LiteralPath $ClaudeExe -PathType Leaf) "claude.exe が置き場に無い: $ClaudeExe"
    Write-Host ("TAKO2433_INSTALL via={0} exit={1} seconds={2} git=hidden" -f $(if ($TakoExe) { 'tako' } else { 'install.ps1' }), $r.Code, $r.Seconds)
}

# --- 1 つの組み合わせ ---------------------------------------------------------
function Test-Case($Case) {
    $script:Stage = $Case.Name
    $pathDirs = @($StdPath | Where-Object { -not ($Case.PSObject.Properties['DropPs'] -and $Case.DropPs -and $_ -eq $PsDir) }) + @($Case.PathExtra)
    $envText = ($Case.Env.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '
    Write-Host "-- $($Case.Name)（Git: $($Case.Git)）"
    Write-Host ("TAKO2433_CASE case={0} git={1} path_extra=[{2}] ps_on_path={3} env=[{4}]" -f `
            $Case.Name, $Case.Git, (@($Case.PathExtra) -join ';'), ($pathDirs -contains $PsDir), $envText)

    $cfg = Join-Path $RunRoot "cfg-$($Case.Name)"
    New-Item -ItemType Directory -Path $cfg -Force | Out-Null
    $baseEnv = @{ CLAUDE_CONFIG_DIR = $cfg }
    foreach ($k in $Case.Env.Keys) { $baseEnv[$k] = $Case.Env[$k] }

    # 1. --version
    $v = Invoke-Sandboxed -FilePath $ClaudeExe -Arguments @('--version') -PathDirs $pathDirs -ExtraEnv $baseEnv -TimeoutSec 60
    $version = Format-OneLine ($v.Out + ' ' + $v.Err) 160
    Write-Host ("TAKO2433_VERSION case={0} exit={1} [{2}]" -f $Case.Name, $v.Code, $version)

    # 2. 認証なしの -p
    $na = Invoke-Sandboxed -FilePath $ClaudeExe -Arguments @('-p', 'tako2433 probe') -PathDirs $pathDirs -ExtraEnv $baseEnv -TimeoutSec 90
    Write-Host ("TAKO2433_NOAUTH case={0} exit={1} [{2}]" -f $Case.Name, $na.Code, (Format-OneLine ($na.Out + ' | ' + $na.Err)))

    # 3. 偽の API で 1 ターン
    $fake = Start-FakeApi (Join-Path $RunRoot "api-$($Case.Name)")
    try {
        $apiEnv = @{}
        foreach ($k in $baseEnv.Keys) { $apiEnv[$k] = $baseEnv[$k] }
        $apiEnv['ANTHROPIC_API_KEY'] = 'sk-ant-tako2433-dummy'
        $apiEnv['ANTHROPIC_BASE_URL'] = "http://127.0.0.1:$($fake.Port)"
        $run = Invoke-Sandboxed -FilePath $ClaudeExe -PathDirs $pathDirs -ExtraEnv $apiEnv -TimeoutSec 150 `
            -Arguments @('--debug', '--allowedTools', 'Bash,PowerShell', '-p', 'tako2433 probe')
    } finally {
        Stop-FakeApi $fake
    }
    $requests = Read-Lines (Join-Path $fake.LogDir 'requests.log')
    $toolResults = Read-Lines (Join-Path $fake.LogDir 'tool-results.log')
    $shellTools = (@($requests | Where-Object { $_ -match 'shell_tools=\[([^\]]*)\]' } | ForEach-Object { $Matches[1] }) | Where-Object { $_ } | Select-Object -Unique) -join '|'
    $ran = @($toolResults | Where-Object { $_ -match 'TAKO2433_SHELL (\S+)' } | ForEach-Object { $Matches[1] }) | Select-Object -First 1
    $stdout = Format-OneLine $run.Out 200
    Write-Host ("TAKO2433_FAKEAPI case={0} exit={1} requests={2} shell_tools=[{3}] ran=[{4}] stdout=[{5}]" -f `
            $Case.Name, $run.Code, $requests.Count, $shellTools, $ran, $stdout)
    foreach ($line in $requests) { Write-Host "TAKO2433_REQUEST case=$($Case.Name) | $line" }
    foreach ($line in $toolResults) { Write-Host "TAKO2433_TOOL_RESULT case=$($Case.Name) | $(Format-OneLine $line 300)" }
    # シェルの検出にまつわる debug の行（stderr と debug の置き場の両方から拾う）
    $debugFiles = @(Get-ChildItem -LiteralPath (Join-Path $cfg 'debug') -Filter '*.txt' -File -ErrorAction SilentlyContinue)
    $debugText = @($run.Err -split "`r?`n") + @($debugFiles | ForEach-Object { Get-Content -LiteralPath $_.FullName -Encoding utf8 })
    $shellLines = @($debugText | Where-Object { $_ -match '(?i)git.?bash|bash\.exe|GIT_BASH|powershell|shell' } | Select-Object -First 12)
    foreach ($line in $shellLines) { Write-Host "TAKO2433_DEBUG case=$($Case.Name) | $(Format-OneLine $line 300)" }

    $status = $null
    if ($TakoExe) {
        $st = Invoke-Sandboxed -FilePath $TakoExe -Arguments @('setup', 'bootstrap', 'status', '--json') -PathDirs $pathDirs -ExtraEnv $Case.Env -TimeoutSec 180
        if ($st.Code -ne 0) { Show-Result $st 'status'; Fail "tako setup bootstrap status が終了コード $($st.Code)" }
        $status = $st.Out | ConvertFrom-Json -Depth 32
        Write-Host ("TAKO2433_STATUS case={0} next_step={1}" -f $Case.Name, $status.next_step)
    }

    [pscustomobject]@{
        Name        = $Case.Name
        Git         = $Case.Git
        Version     = $version
        NoAuthExit  = $na.Code
        NoAuthText  = Format-OneLine ($na.Out + ' | ' + $na.Err) 200
        ApiExit     = $run.Code
        ShellTools  = $shellTools
        Ran         = [string]$ran
        Status      = $status
        Run         = $run
    }
}

$Cases = @(
    [pscustomobject]@{ Name = 'no-git'; Git = 'hidden'; PathExtra = @(); Env = @{} },
    [pscustomobject]@{ Name = 'bash-path-env'; Git = 'hidden'; PathExtra = @(); Env = @{ CLAUDE_CODE_GIT_BASH_PATH = (Join-Path $GitHidden 'bin\bash.exe') } },
    [pscustomobject]@{ Name = 'git-nonstandard-on-path'; Git = 'hidden'; PathExtra = @((Join-Path $GitHidden 'cmd')); Env = @{} },
    [pscustomobject]@{ Name = 'bash-path-missing'; Git = 'hidden'; PathExtra = @(); Env = @{ CLAUDE_CODE_GIT_BASH_PATH = (Join-Path $RunRoot 'nope\bash.exe') } },
    [pscustomobject]@{ Name = 'no-powershell-on-path'; Git = 'hidden'; PathExtra = @(); DropPs = $true; Env = @{} },
    [pscustomobject]@{ Name = 'git-on-path'; Git = 'present'; PathExtra = @((Join-Path $GitRoot 'cmd')); Env = @{} },
    [pscustomobject]@{ Name = 'git-off-path'; Git = 'present'; PathExtra = @(); Env = @{} },
    [pscustomobject]@{ Name = 'bash-path-missing-git'; Git = 'present'; PathExtra = @(); Env = @{ CLAUDE_CODE_GIT_BASH_PATH = (Join-Path $RunRoot 'nope\bash.exe') } }
)

# 期待: 走ったシェル（偽の API は Bash を出されたら Bash を、無ければ PowerShell を使わせる）。$null は記録だけ
$Expect = @{
    'no-git'                  = 'powershell'
    'bash-path-env'           = 'bash'
    'git-nonstandard-on-path' = 'bash'
    'bash-path-missing'       = 'powershell'
    'no-powershell-on-path'   = $null
    'git-on-path'             = 'bash'
    'git-off-path'            = 'bash'
    'bash-path-missing-git'   = 'bash'
}
$ShellNoteNeedle = 'Git for Windows は要りません'

# 1 つの組み合わせの結果が期待どおりか。外れたら理由の文を返す（合っていれば $null）
function Test-Expectation($Result) {
    $want = $Expect[$Result.Name]
    if ($Result.Version -notmatch '\(Claude Code\)') { return "--version が答えない: $($Result.Version)" }
    if ($null -eq $want) { return $null }
    if ($Result.ApiExit -ne 0) {
        return "1 ターン走らない（偽の API の -p が exit $($Result.ApiExit)。stdout: $(Format-OneLine $Result.Run.Out 200) / stderr: $(Format-OneLine $Result.Run.Err 300)）"
    }
    if ($Result.Ran -ne $want) {
        return "コマンドが走ったシェルが $want でない（走った: [$($Result.Ran)]・出されたツール: [$($Result.ShellTools)]）"
    }
    if ($want -eq 'powershell' -and ($Result.ShellTools -split '[|,]') -contains 'Bash') {
        return "Git Bash が無いのに Bash のツールが出た（$($Result.ShellTools)）"
    }
    if ($TakoExe) {
        $plan = $Result.Status.install_plan
        if (-not ([string]$plan.shell_note).Contains($ShellNoteNeedle)) {
            return "status の install_plan.shell_note が「$ShellNoteNeedle」を言わない: $($plan.shell_note)"
        }
    }
    $null
}

$Before = Get-TakoUserPath
Write-Host '== Windows の Claude Code が Git Bash を要るか（#2433 の 2）=='
Write-Host "   OS        : $([System.Environment]::OSVersion.VersionString)"
Write-Host "   砂場      : $RunRoot"
Write-Host "   Git       : $GitRoot（bash.exe: $(Test-Path -LiteralPath (Join-Path $GitRoot 'bin\bash.exe'))）"
Write-Host "   Git (x86) : $GitRootX86（在る: $(if ($GitRootX86) { Test-Path -LiteralPath $GitRootX86 } else { $false })）"
Write-Host "   pwsh 7    : $((Get-Command pwsh -ErrorAction SilentlyContinue).Source)"
Write-Host "   導入      : $(if ($TakoExe) { "tako setup bootstrap install（$TakoExe）" } else { 'irm https://claude.ai/install.ps1 | iex' })"

$failures = @()
$results = @()
try {
    Hide-Git
    Install-Claude
    foreach ($case in $Cases) {
        if ($case.Git -eq 'present' -and $script:GitHiddenNow) { Show-Git; Write-Host "   Git を戻した: $GitRoot" }
        try {
            $results += Test-Case $case
        } catch {
            $failures += $(if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { [string]$_ })
        }
    }
} catch {
    $failures += $(if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { [string]$_ })
} finally {
    $script:Stage = '後片付け'
    try { Show-Git } catch { $failures += $_.Exception.Message }
    Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($RunRoot, [System.StringComparison]::OrdinalIgnoreCase) } catch { $false }
    } | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    Restore-TakoUserPath -Snapshot $Before
    $afterPath = Get-TakoUserPath
    if ($afterPath.Raw -ne $Before.Raw -or $afterPath.Exists -ne $Before.Exists) {
        $failures += "[#2433 後片付け] ユーザー環境変数 Path を検査前へ戻せなかった"
    }
}

$script:Stage = '判定'
Write-Host ''
Write-Host '== 表（#2433 の 2）=='
Write-Host '| 組み合わせ | Git | --version | 認証なしの -p | 偽の API の -p | 出されたシェルのツール | 走ったシェル | tako の段 |'
Write-Host '|---|---|---|---|---|---|---|---|'
foreach ($r in $results) {
    $step = if ($r.Status) { [string]$r.Status.next_step } else { '-' }
    Write-Host ("| {0} | {1} | {2} | exit {3} | exit {4} | {5} | {6} | {7} |" -f $r.Name, $r.Git, $r.Version, $r.NoAuthExit, $r.ApiExit, $r.ShellTools, $r.Ran, $step)
}
foreach ($r in $results) {
    $why = Test-Expectation $r
    if ($why) { $failures += "[#2433 $($r.Name)] $why" }
}
$missing = @($Cases | Where-Object { $_.Name -notin @($results | ForEach-Object { $_.Name }) } | ForEach-Object { $_.Name })
if ($missing.Count -gt 0) { $failures += "[#2433 判定] 結果の無い組み合わせ: $($missing -join ', ')" }
# Git の有無で tako の段が変わらない（Git Bash を段の判定に入れない）
if ($TakoExe) {
    $steps = @($results | ForEach-Object { [string]$_.Status.next_step } | Select-Object -Unique)
    if ($steps.Count -ne 1) {
        $failures += "[#2433 判定] Git の有無で tako の段が変わった: $(@($results | ForEach-Object { "$($_.Name)=$($_.Status.next_step)" }) -join ' ')"
    }
}
if ($failures.Count -eq 0) {
    Write-Host "TAKO2433_VERDICT git_bash_required=false claude=[$(@($results | ForEach-Object { $_.Version } | Select-Object -Unique) -join ',')] cases=$($results.Count)"
}

Remove-Item -LiteralPath $RunRoot -Recurse -Force -ErrorAction SilentlyContinue
if ($failures.Count -gt 0) {
    foreach ($f in $failures) { [Console]::WriteLine("::error::$f") }
    exit 1
}
exit 0
