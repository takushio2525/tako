#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Windows で codex / agy の導入代行を、公式の install.ps1 を本当に走らせて確かめる（Issue #2791）。

.DESCRIPTION
    #2791 で Windows の codex / agy も導入を代行するようにした（それまでは公式コマンドの案内だけ）。
    Windows の実機は当面使えないので、使い捨ての windows-latest ランナーで本物の導入を走らせる。
    確かめるのは製品の口そのもの（`tako setup bootstrap install --agent codex|agy` = MCP
    `tako_setup_bootstrap` と同じ setup_bootstrap::install_for）:

      1. 導入の前の status が next_step = install（未導入）
      2. install が公式の install.ps1 を取得して走らせ、終了コード 0・performed = true・binary が砂場の中
      3. 導入の後の status が導入済み（next_step が install でなく、binary がランチャーの置き場）
      4. 入ったランチャー（codex.exe / agy.exe）が `--version` に答える
      5. agy の取得物（%LOCALAPPDATA%\antigravity\staging\agy.exe）が残っていない（起動片のスコープ。#2791）

    これをポリシーなし・HKCU UserPolicy = Restricted・HKLM MachinePolicy = AllSigned の 3 通りで回す
    （#2790 の検査と同じ置き方。置き方・戻し方は lib/execution-policy.ps1 の 1 実装）。各回の頭で
    偽物の起こし方の検査（cargo のテスト issue2791_agent_install_windows）も同じポリシーの下で走らせる。

    ポリシーなしの回だけ、境界も見る:
      - A/B（TAKO_2791_LEGACY=1）: #2791 より前の形は導入を断り、公式コマンドを案内する
      - 既に入っている: 2 回目の install も 0 で終わる（agy は already installed を言う）
      - 導入元に届かない: HTTPS_PROXY を閉じたポートへ向けると、取得の失敗として理由つきで断る
      - 途中で失敗する: codex は存在しない版（CODEX_RELEASE=0.0.1）、agy は取得物の置き場を作れない形で、
        インストーラの失敗としてその系統の案内先（claude のページではない）つきで断り、ランチャーは置かない
      - npm が無い: 砂場の PATH は Windows の標準の置き場だけ（node / npm を載せない）

    隔離: USERPROFILE / APPDATA / LOCALAPPDATA / TEMP を回ごとの一時 dir の砂場へ向け、HOME と
    TAKO_* / CODEX_HOME / CLAUDE_CONFIG_DIR を外す。ユーザー環境変数 Path（HKCU）だけは砂場に閉じ込められない
    （codex の install.ps1 と agy.exe install が自分で書く）ので、最初に値を取り、回ごとと最後に元へ戻して
    戻ったことまで確かめる。そのため GitHub Actions の使い捨てランナー以外では走らせない。

    ポリシーを置いている間は、このスクリプト自身はモジュールやスクリプトファイルを新しく読み込まない
    （使う cmdlet のモジュールは置く前に読み込んでおく。cargo のテストは置く前に --no-run で組む）。

    失敗は段の名前つきの `::error::` 1 行で名指しし、終了コード 1 で終わる。結果の行（TAKO2791_*）を
    CI のログへ残す。

.PARAMETER Tako
    検査する tako.exe。省略時はリポジトリの target\debug\tako.exe（CI の `cargo build --workspace` の成果物）。

.EXAMPLE
    pwsh -File scripts/windows/test-agent-install-2791.ps1
#>
[CmdletBinding()]
param([string]$Tako)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用（本物の install.ps1 を走らせ、HKCU / HKLM を書き換えて戻す）' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（本物の codex / agy を入れ、グループポリシーとユーザー環境変数 Path を書き換えて戻す。実機では走らせない）'
}

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $RepoRoot 'installer/windows/lib/user-path.ps1')
. (Join-Path $PSScriptRoot 'lib/execution-policy.ps1')

$TestName = 'issue2791_agent_install_windows'
$Cases = @(
    [pscustomobject]@{ Name = 'ポリシーなし'; Hive = $null; Scope = $null; Value = $null },
    [pscustomobject]@{ Name = 'HKCU UserPolicy=Restricted'; Hive = 'HKCU'; Scope = 'UserPolicy'; Value = 'Restricted' },
    [pscustomobject]@{ Name = 'HKLM MachinePolicy=AllSigned'; Hive = 'HKLM'; Scope = 'MachinePolicy'; Value = 'AllSigned' }
)
# 系統ごとの案内先（正本は tako_core::platform::agent_install の troubleshoot_url）と公式の 1 行
$Agents = [ordered]@{
    codex = [pscustomobject]@{ Troubleshoot = 'https://learn.chatgpt.com/docs/codex/cli'; Official = 'irm https://chatgpt.com/codex/install.ps1 | iex'; Url = 'https://chatgpt.com/codex/install.ps1' }
    agy   = [pscustomobject]@{ Troubleshoot = 'https://antigravity.google/docs/cli/troubleshooting'; Official = 'irm https://antigravity.google/cli/install.ps1 | iex'; Url = 'https://antigravity.google/cli/install.ps1' }
}
# 砂場の PATH = Windows の標準の置き場だけ（node / npm / pwsh 7 / git を載せない = npm が無い環境）
$SysRoot = $env:SystemRoot
$SandboxPath = @(
    (Join-Path $SysRoot 'System32'),
    $SysRoot,
    (Join-Path $SysRoot 'System32\Wbem'),
    (Join-Path $SysRoot 'System32\WindowsPowerShell\v1.0')
) -join ';'

$script:Stage = '準備'

function Fail([string]$Message) {
    throw "[#2791 $script:Stage] $Message"
}

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { Fail $Message }
}

if (-not $Tako) { $Tako = Join-Path $RepoRoot 'target\debug\tako.exe' }
if (-not (Test-Path -LiteralPath $Tako -PathType Leaf)) { throw "tako.exe が無い: $Tako（先に cargo build -p tako-cli）" }
$TakoExe = (Resolve-Path -LiteralPath $Tako).ProviderPath

# 使う cmdlet のモジュールはポリシーを置く前に読み込む（pwsh 7 は AllSigned の下で型データを読めない
# モジュールがある = #2790 の実測。読み込み済みなら置いた後も使える）
Import-Module Microsoft.PowerShell.Management, Microsoft.PowerShell.Utility -ErrorAction Stop
$null = '{}' | ConvertFrom-Json

# --- 砂場 ---------------------------------------------------------------------
$RunRoot = Join-Path ([System.IO.Path]::GetTempPath()) "tako-2791-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Path $RunRoot -Force | Out-Null

function New-Sandbox([string]$Name) {
    $root = Join-Path $RunRoot $Name
    # `$home` は PowerShell の読み取り専用の自動変数なので別の名前で持つ
    $homeDir = Join-Path $root 'home'
    $sb = [pscustomobject]@{
        Root         = $root
        Home         = $homeDir
        AppData      = (Join-Path $homeDir 'AppData\Roaming')
        LocalAppData = (Join-Path $homeDir 'AppData\Local')
        Tmp          = (Join-Path $root 'tmp')
    }
    foreach ($d in @($sb.AppData, $sb.LocalAppData, $sb.Tmp)) {
        New-Item -ItemType Directory -Path $d -Force | Out-Null
    }
    $sb
}

# 砂場の env（PATH は Windows の標準の置き場だけ）
function New-SandboxEnv($Sandbox, [hashtable]$ExtraEnv) {
    $vars = @{}
    foreach ($entry in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
        $vars[[string]$entry.Key] = [string]$entry.Value
    }
    foreach ($k in @($vars.Keys)) {
        if ($k -like 'TAKO_*' -or $k -like 'CODEX_*' -or $k -in @('HOME', 'CLAUDE_CONFIG_DIR', 'CLAUDECODE', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'PSModulePath')) {
            $vars.Remove($k)
        }
    }
    # PSModulePath は外す: このスクリプト（pwsh 7）の値には pwsh 7 のモジュールの置き場が入っていて、tako が起こす
    # Windows PowerShell 5.1 がそこから読もうとして落ちることがある（#2790 の CI で実測）。エクスプローラーから
    # 起動した tako にはこの値が無いので、外して各 PowerShell の既定へ戻すのが利用者の環境と同じ形
    $vars['USERPROFILE'] = $Sandbox.Home
    $vars['APPDATA'] = $Sandbox.AppData
    $vars['LOCALAPPDATA'] = $Sandbox.LocalAppData
    $vars['TEMP'] = $Sandbox.Tmp
    $vars['TMP'] = $Sandbox.Tmp
    $vars['PATH'] = $SandboxPath
    # 判定は日本語の字面で見る（表示言語の既定が OS の言語に従うので固定する。#2365）
    $vars['TAKO_LANG'] = 'ja'
    if ($ExtraEnv) { foreach ($k in $ExtraEnv.Keys) { $vars[$k] = [string]$ExtraEnv[$k] } }
    $vars
}

# 子を起こして待つ。stdout / stderr は UTF-8 で読む（tako はパイプへ UTF-8 で書く）
function Invoke-Sandboxed {
    param(
        [Parameter(Mandatory = $true)]$Sandbox,
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [hashtable]$ExtraEnv,
        [int]$TimeoutSec = 900
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
    $psi.WorkingDirectory = $Sandbox.Home
    $psi.Environment.Clear()
    $vars = New-SandboxEnv $Sandbox $ExtraEnv
    foreach ($k in $vars.Keys) { $psi.Environment[$k] = $vars[$k] }

    $started = Get-Date
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        $p.StandardInput.Close()
        if (-not $p.WaitForExit($TimeoutSec * 1000)) {
            $p.Kill($true)
            Fail "$(Split-Path -Leaf $FilePath) $($Arguments -join ' ') が $TimeoutSec 秒で終わらなかった"
        }
        $p.WaitForExit()
        $result = [PSCustomObject]@{
            Code    = $p.ExitCode
            Out     = $out.Result
            Err     = $err.Result
            Seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 1)
        }
    } finally {
        $p.Dispose()
    }
    Write-Host ("   > {0} {1}  (exit {2}, {3} 秒)" -f (Split-Path -Leaf $FilePath), ($Arguments -join ' '), $result.Code, $result.Seconds)
    $result
}

function Invoke-Tako($Sandbox, [string[]]$Arguments, [hashtable]$ExtraEnv, [int]$TimeoutSec = 900) {
    Invoke-Sandboxed -Sandbox $Sandbox -FilePath $TakoExe -Arguments $Arguments -ExtraEnv $ExtraEnv -TimeoutSec $TimeoutSec
}

function Show-Result($Result, [string]$Label) {
    Write-Host "   --- $Label の stdout ---"
    $Result.Out -split "`r?`n" | ForEach-Object { Write-Host "   | $_" }
    Write-Host "   --- $Label の stderr ---"
    $Result.Err -split "`r?`n" | ForEach-Object { Write-Host "   | $_" }
}

function Assert-Exit0($Result, [string]$Label) {
    if ($Result.Code -ne 0) {
        Show-Result $Result $Label
        Fail "$Label が終了コード $($Result.Code) で終わった（期待 0）"
    }
}

function Assert-Refused($Result, [string]$Label, [string[]]$Needles, [string[]]$Forbidden = @()) {
    $text = $Result.Out + "`n" + $Result.Err
    if ($Result.Code -eq 0) {
        Show-Result $Result $Label
        Fail "$Label が終了コード 0 で終わった（断るはず）"
    }
    foreach ($n in $Needles) {
        if (-not $text.Contains($n)) {
            Show-Result $Result $Label
            Fail "$Label の断りに「$n」が無い"
        }
    }
    foreach ($n in $Forbidden) {
        if ($text.Contains($n)) {
            Show-Result $Result $Label
            Fail "$Label の断りに「$n」が混ざっている"
        }
    }
}

function ConvertFrom-TakoJson($Result, [string]$Label) {
    try {
        $Result.Out | ConvertFrom-Json -Depth 32
    } catch {
        Show-Result $Result $Label
        Fail "$Label の stdout が JSON として読めない: $($_.Exception.Message)"
    }
}

# status を読んで 1 行残す。status は入っていれば認証を上限なしで問い合わせる（codex login status /
# agy models）ので、返らない回にステップごと固まらないよう 1 回の上限を短めに置く
function Get-AgentStatus($Sandbox, [string]$Agent, [string]$When, [string]$CaseName, [hashtable]$ExtraEnv) {
    $r = Invoke-Tako $Sandbox @('setup', 'bootstrap', 'status', '--agent', $Agent, '--json') $ExtraEnv 180
    Assert-Exit0 $r "status --agent $Agent（$When）"
    $j = ConvertFrom-TakoJson $r "status --agent $Agent（$When）"
    $binary = if ($null -eq $j.binary) { '' } else { [string]$j.binary }
    Write-Host ("TAKO2791_STATUS case={0} agent={1} when={2} next_step={3} binary=[{4}] launcher=[{5}]" -f `
            $CaseName, $Agent, $When, $j.next_step, $binary, $j.install_plan.launcher)
    $j
}

# 1 系統を入れて確かめる（受け入れ条件 1・2 の芯）
function Test-AgentInstall($Sandbox, [string]$Agent, [string]$CaseName) {
    $script:Stage = "$CaseName / $Agent"
    $before = Get-AgentStatus $Sandbox $Agent 'before' $CaseName
    Assert-That ($before.next_step -eq 'install') "導入の前の next_step が install でない: $($before.next_step)（ランナーに $Agent が在る？）"
    Assert-That ($before.install_plan.can_run -eq $true) "install_plan.can_run が true でない（#2791 は Windows の $Agent も代行する）"
    $launcher = [string]$before.install_plan.launcher
    Assert-That ($launcher.StartsWith($Sandbox.Home, [System.StringComparison]::OrdinalIgnoreCase)) "ランチャーの置き場が砂場の外: $launcher"

    $r = Invoke-Tako $Sandbox @('setup', 'bootstrap', 'install', '--agent', $Agent, '--json')
    Assert-Exit0 $r "install --agent $Agent"
    $j = ConvertFrom-TakoJson $r "install --agent $Agent"
    Assert-That ($j.performed -eq $true) "install の performed が true でない"
    Assert-That ([string]$j.binary -and ([string]$j.binary).StartsWith($Sandbox.Home, [System.StringComparison]::OrdinalIgnoreCase)) "install の binary が砂場の中でない: $($j.binary)"
    Write-Host ("TAKO2791_INSTALL case={0} agent={1} seconds={2} binary=[{3}]" -f $CaseName, $Agent, $r.Seconds, $j.binary)
    $tail = @(([string]$j.output) -split "`r?`n" | Where-Object { $_ -ne '' } | Select-Object -Last 6)
    foreach ($line in $tail) { Write-Host "TAKO2791_INSTALL_OUTPUT agent=$Agent | $line" }
    Assert-That (-not ([string]$j.output).Contains('<Objs')) 'install の output に CLIXML が残っている（powershell_stderr_text が効いていない）'

    # 公式インストーラ自身がユーザー環境変数 Path へ置き場を足したか（記録だけ。tako の path の段が要るかの材料）
    $launcherDir = Split-Path -Parent $launcher
    $inUserPath = Get-TakoPathEntryCount -Entries (Get-TakoUserPath).Entries -Dir $launcherDir
    Write-Host ("TAKO2791_USERPATH case={0} agent={1} launcher_dir_in_user_path={2} dir=[{3}]" -f $CaseName, $Agent, $inUserPath, $launcherDir)

    $after = Get-AgentStatus $Sandbox $Agent 'after' $CaseName
    Assert-That ($after.next_step -ne 'install') "導入の後も next_step が install のまま"
    Assert-That ($null -ne $after.binary) "導入の後の status に binary が無い"
    Assert-That (Test-Path -LiteralPath $launcher -PathType Leaf) "ランチャーが置き場に無い: $launcher"

    # 入ったものが実行できる（codex は --version に答える。agy の --version の振る舞いは公式に書かれて
    # いないので記録だけにする）
    try {
        $v = Invoke-Sandboxed -Sandbox $Sandbox -FilePath $launcher -Arguments @('--version') -TimeoutSec 60
        $version = (($v.Out + ' ' + $v.Err) -replace '\s+', ' ').Trim()
        Write-Host ("TAKO2791_VERSION case={0} agent={1} exit={2} [{3}]" -f $CaseName, $Agent, $v.Code, $version)
        if ($Agent -eq 'codex') { Assert-Exit0 $v "$Agent --version" }
    } catch {
        if ($Agent -eq 'codex') { throw }
        Write-Host ("TAKO2791_VERSION case={0} agent={1} 記録だけ: {2}" -f $CaseName, $Agent, $_.Exception.Message)
    }

    if ($Agent -eq 'agy') {
        $staging = Join-Path $Sandbox.LocalAppData 'antigravity\staging\agy.exe'
        Write-Host ("TAKO2791_STAGING case={0} staging_left={1} path=[{2}]" -f $CaseName, (Test-Path -LiteralPath $staging), $staging)
        Assert-That (-not (Test-Path -LiteralPath $staging)) "agy の取得物が残った: $staging（起動片が子スコープで、install.ps1 の片付けが飛んだ。#2791）"
    }
}

# ポリシーなしの回だけの境界（A/B・既に入っている・届かない・途中で失敗・npm が無い）
function Test-Boundaries($Installed) {
    $script:Stage = '境界: npm が無い'
    $w = Invoke-Sandboxed -Sandbox $Installed -FilePath (Join-Path $SysRoot 'System32\where.exe') -Arguments @('npm') -TimeoutSec 60
    Write-Host ("TAKO2791_NPM where_exit={0} out=[{1}]" -f $w.Code, ($w.Out.Trim()))
    Assert-That ($w.Code -ne 0) "砂場の PATH に npm が見える（npm が無い環境で入ることを確かめられない）: $($w.Out)"

    $script:Stage = '境界: 既に入っている'
    foreach ($agent in $Agents.Keys) {
        $r = Invoke-Tako $Installed @('setup', 'bootstrap', 'install', '--agent', $agent, '--json')
        Assert-Exit0 $r "2 回目の install --agent $agent"
        $j = ConvertFrom-TakoJson $r "2 回目の install --agent $agent"
        Write-Host ("TAKO2791_AGAIN agent={0} seconds={1} performed={2}" -f $agent, $r.Seconds, $j.performed)
        if ($agent -eq 'agy') {
            Assert-That (([string]$j.output).Contains('already installed')) "2 回目の agy が already installed を言わない: $($j.output)"
        }
    }

    foreach ($agent in $Agents.Keys) {
        $info = $Agents[$agent]

        $script:Stage = "境界: A/B（TAKO_2791_LEGACY=1）/ $agent"
        $sb = New-Sandbox "legacy-$agent"
        $legacy = @{ TAKO_2791_LEGACY = '1' }
        $r = Invoke-Tako $sb @('setup', 'bootstrap', 'install', '--agent', $agent) $legacy
        Assert-Refused $r "旧形の install --agent $agent" @('この環境では tako が自動インストールを代行できません', $info.Official)
        $st = Get-AgentStatus $sb $agent 'legacy' 'A/B' $legacy
        Assert-That ($st.install_plan.can_run -eq $false) "旧形の status の can_run が false でない"
        Assert-That (-not (Test-Path -LiteralPath ([string]$st.install_plan.launcher))) '旧形が断ったのにランチャーが置かれた'
        Write-Host ("TAKO2791_LEGACY agent={0} exit={1} refused=true" -f $agent, $r.Code)

        $script:Stage = "境界: 導入元に届かない / $agent"
        $sb = New-Sandbox "unreachable-$agent"
        # curl.exe は HTTPS_PROXY を読む（Windows の環境変数は大小文字を区別しない）。9 番 = 閉じたポート
        $proxy = @{ HTTPS_PROXY = 'http://127.0.0.1:9'; HTTP_PROXY = 'http://127.0.0.1:9' }
        $r = Invoke-Tako $sb @('setup', 'bootstrap', 'install', '--agent', $agent) $proxy
        Assert-Refused $r "届かない install --agent $agent" @('インストーラを取得できませんでした', $info.Url)
        $st = Get-AgentStatus $sb $agent 'unreachable' '届かない'
        Assert-That ($st.next_step -eq 'install') "届かなかったのに next_step が install でない: $($st.next_step)"
        Assert-That (-not (Test-Path -LiteralPath ([string]$st.install_plan.launcher))) '届かないのにランチャーが置かれた'
        Write-Host ("TAKO2791_UNREACHABLE agent={0} exit={1}" -f $agent, $r.Code)
    }

    $script:Stage = '境界: 途中で失敗する / codex（存在しない版）'
    $sb = New-Sandbox 'midway-codex'
    $r = Invoke-Tako $sb @('setup', 'bootstrap', 'install', '--agent', 'codex') @{ CODEX_RELEASE = '0.0.1' }
    Assert-Refused $r '存在しない版の install --agent codex' @('Codex CLI のインストーラが失敗しました', $Agents['codex'].Troubleshoot, '0.0.1') @('claude', '<Objs')
    Assert-That (-not (Test-Path -LiteralPath (Join-Path $sb.LocalAppData 'Programs\OpenAI\Codex\bin\codex.exe'))) '失敗したのに codex のランチャーが置かれた'
    Write-Host ("TAKO2791_MIDWAY agent=codex exit={0}" -f $r.Code)

    $script:Stage = '境界: 途中で失敗する / agy（取得物の置き場を作れない）'
    $sb = New-Sandbox 'midway-agy'
    # %LOCALAPPDATA%\antigravity をファイルにしておくと、install.ps1 は staging の dir を作れずに止まる
    Set-Content -LiteralPath (Join-Path $sb.LocalAppData 'antigravity') -Value 'not a directory 2791' -Encoding ascii
    $r = Invoke-Tako $sb @('setup', 'bootstrap', 'install', '--agent', 'agy')
    Assert-Refused $r '置き場を作れない install --agent agy' @('Antigravity CLI のインストーラが失敗しました', $Agents['agy'].Troubleshoot, 'staging') @('claude', '<Objs')
    Assert-That (-not (Test-Path -LiteralPath (Join-Path $sb.LocalAppData 'agy\bin\agy.exe'))) '失敗したのに agy のランチャーが置かれた'
    Write-Host ("TAKO2791_MIDWAY agent=agy exit={0}" -f $r.Code)
}

# 回の終わりに砂場の中で動いているもの（入れた CLI が起こした子）を止めてから消す
function Remove-Sandboxes {
    Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($RunRoot, [System.StringComparison]::OrdinalIgnoreCase) } catch { $false }
    } | ForEach-Object {
        Write-Host "   砂場の中で動いていたプロセスを止める: $($_.Id) $($_.Path)"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
    Get-ChildItem -LiteralPath $RunRoot -Force -ErrorAction SilentlyContinue | ForEach-Object {
        Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# テストは置く前に組む（ポリシーの下で cargo の build.rs を走らせない）
$script:Stage = '組み立て'
& cargo test -p tako-core --test $TestName --no-run
if ($LASTEXITCODE -ne 0) {
    [Console]::WriteLine("::error::[#2791 組み立て] cargo test --no-run が終了コード $LASTEXITCODE")
    exit 1
}

$Before = Get-TakoUserPath
Write-Host "== codex / agy の導入代行の実走（Windows・#2791）=="
Write-Host "   tako.exe : $TakoExe"
Write-Host "   砂場     : $RunRoot"
Write-Host "   砂場の PATH: $SandboxPath"
Write-Host "   検査前のユーザー環境変数 Path: 要素 $($Before.Entries.Count) 個"
Write-Host "   実効の実行ポリシー（置く前）: $(Get-PolicyListText)"

$failures = @()
foreach ($case in $Cases) {
    $script:Stage = $case.Name
    Write-Host "-- $($case.Name)"
    $snapshot = $null
    if ($case.Hive) {
        $snapshot = @(Get-PolicySnapshot $case.Hive)
        Write-Host ('   置く前: ' + (Format-Snapshot $snapshot))
    }
    try {
        if ($case.Hive) {
            Set-Policy $case.Hive $case.Value
            Write-Host "   実効の実行ポリシー（置いた後）: $(Get-PolicyListText)"
            $env:TAKO_2791_POLICY = '{0}:{1}' -f $case.Scope, $case.Value
        }
        # 偽物の起こし方の検査（同じポリシーの下。組み立ては済んでいる）
        & cargo test -p tako-core --test $TestName -- --nocapture --test-threads=1
        if ($LASTEXITCODE -ne 0) { Fail "cargo test --test $TestName が終了コード $LASTEXITCODE" }

        # 本物の導入（codex → agy の順に同じ砂場へ）
        $sb = New-Sandbox ('case-{0}' -f [array]::IndexOf($Cases, $case))
        foreach ($agent in $Agents.Keys) {
            Test-AgentInstall $sb $agent $case.Name
        }
        if (-not $case.Hive) { Test-Boundaries $sb }
    } catch {
        $message = if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { [string]$_ }
        $failures += $message
    } finally {
        # 置いたポリシーはここで必ず戻す（テストが落ちても・例外でも）
        if ($case.Hive) {
            Restore-Policy $case.Hive $snapshot
            $env:TAKO_2791_POLICY = $null
            $after = @(Get-PolicySnapshot $case.Hive)
            if ((Format-Snapshot $after) -ne (Format-Snapshot $snapshot)) {
                $failures += "[#2791 $($case.Name)] ポリシーを戻せていない（前: $(Format-Snapshot $snapshot) / 後: $(Format-Snapshot $after)）"
            }
        }
        # codex の install.ps1 と agy.exe install が書いたユーザー環境変数 Path を回ごとに戻す
        Restore-TakoUserPath -Snapshot $Before
        Remove-Sandboxes
    }
}

$script:Stage = '後片付け'
$afterPath = Get-TakoUserPath
if ($afterPath.Raw -ne $Before.Raw -or $afterPath.Exists -ne $Before.Exists) {
    $failures += "[#2791 後片付け] ユーザー環境変数 Path を検査前へ戻せなかった（前 $($Before.Raw) / 後 $($afterPath.Raw)）"
}
Write-Host "   実効の実行ポリシー（戻した後）: $(Get-PolicyListText)"
Remove-Item -LiteralPath $RunRoot -Recurse -Force -ErrorAction SilentlyContinue

if ($failures.Count -gt 0) {
    foreach ($f in $failures) { [Console]::WriteLine("::error::$f") }
    exit 1
}
Write-Host '== 3 通り（ポリシーなし / HKCU Restricted / HKLM AllSigned）とも codex / agy が公式の install.ps1 で入った =='
exit 0
