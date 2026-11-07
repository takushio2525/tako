#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Windows の CI で `tako setup` 一式を隔離した砂場で通しに回す（Issue #2767）。

.DESCRIPTION
    macOS は `tako setup` の実経路テスト（scripts/test-setup-*.sh ほか）が CI で回っているが、
    Windows は `cargo test` だけで、setup を通しで回す検査が無かった。Windows 実機は当面使えないので、
    GUI の要らない部分を使い捨ての windows-latest ランナーで回す。段の並び:

      1. 初回起動の自動の段（GUI の welcome::run_auto_setup と同じ `tako setup --yes --answers -` +
         welcome::auto_setup_answers の JSON。写しは番犬 issue2767_windows_setup_e2e_watchdog が縛る）
      2. `tako setup --yes --answers @<file>`（明示の答え。2 回目は「変更なし」= 冪等）
      3. `tako setup --check`
      4. `tako setup bootstrap undo-path` → `path` → `undo-path`（ユーザー環境変数 Path を読み戻し、
         新しく開いたシェルから claude / tako が引けることまで見る）
      5. `tako setup-mcp`（登録済み = そのまま / 登録先が消えた = 付け替え）
      6. 境界: 未ログインでも setup は完走する（#1501）/ 不正な answers は書く前に断る

    隔離: USERPROFILE / APPDATA / LOCALAPPDATA / TEMP をランナーの一時 dir の砂場へ向け、HOME と
    TAKO_* / CLAUDE_CONFIG_DIR を外す（データの置き場は既定の解決 = %APPDATA%\tako を通す）。
    claude は scripts/windows/fake-claude.rs を rustc で組んだ偽物を砂場の `.local\bin\claude.exe`
    （公式のネイティブインストーラの置き場。PATH には載せない = 入れた直後の状態）へ置く。
    `install: false` で走らせるので、外部のインストールスクリプト（claude の install.ps1・winget）は
    走らない。偽 claude が答えなかった呼び出し（対話起動）があれば落ちる。

    ユーザー環境変数 Path（HKCU）だけは砂場に閉じ込められない（tako も OS もレジストリを読む）。
    最初に値を取り、終わったら（失敗しても）元へ戻して戻ったことまで確かめる。そのため
    GitHub Actions の使い捨てランナー以外では走らせない（Test-TakoInstalledPayload と同じ柵）。

    失敗は段の名前つきの `::error::` 1 行で名指しし、終了コード 1 で終わる。

.PARAMETER Tako
    検査する tako.exe。省略時はリポジトリの target\debug\tako.exe（CI の `cargo build --workspace` の成果物）。

.EXAMPLE
    pwsh -File scripts/windows/test-setup-windows-2767.ps1
    pwsh -File scripts/windows/test-setup-windows-2767.ps1 -Tako target\release\tako.exe
#>
[CmdletBinding()]
param([string]$Tako)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $RepoRoot 'installer/windows/lib/user-path.ps1')

# setup の要約の見出し（正本は tako-control の setup::SUMMARY_CHANGED_HEADER / SUMMARY_UNCHANGED）
$SummaryChanged = 'セットアップ結果（変更したのはこれだけです）:'
$SummaryUnchanged = 'セットアップ結果: 変更なし（前回の設定は最新）'
# 初回起動の自動の段の answers（正本は tako-control の welcome::auto_setup_answers。番犬が字面を縛る）
$AutoSetupAnswers = '{"install":false,"launch_agent":"none"}'

$script:Stage = '準備'

function Fail([string]$Message) {
    throw "[#2767 $script:Stage] $Message"
}

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { Fail $Message }
}

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用（ユーザー環境変数 Path をレジストリで確かめる）' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（ユーザー環境変数 Path を書き換えて戻す。実機では走らせない）'
}

if (-not $Tako) { $Tako = Join-Path $RepoRoot 'target\debug\tako.exe' }
if (-not (Test-Path -LiteralPath $Tako -PathType Leaf)) { throw "tako.exe が無い: $Tako（先に cargo build -p tako-cli）" }
$TakoExe = (Resolve-Path -LiteralPath $Tako).ProviderPath
$TakoDir = Split-Path -Parent $TakoExe

# --- 砂場 ---------------------------------------------------------------------
$Root = Join-Path ([System.IO.Path]::GetTempPath()) "tako-2767-$([System.IO.Path]::GetRandomFileName())"
$HomeDir = Join-Path $Root 'home'
$AppData = Join-Path $HomeDir 'AppData\Roaming'
$LocalAppData = Join-Path $HomeDir 'AppData\Local'
$TmpDir = Join-Path $Root 'tmp'
$ClaudeBin = Join-Path $HomeDir '.local\bin'
$ClaudeExe = Join-Path $ClaudeBin 'claude.exe'
$DataDir = Join-Path $AppData 'tako'
foreach ($d in @($AppData, $LocalAppData, $TmpDir, $ClaudeBin)) {
    New-Item -ItemType Directory -Path $d -Force | Out-Null
}

# 本物の置き場（砂場の外）へ書いていないことを最後に確かめる
$RealDataDir = Join-Path $env:APPDATA 'tako'
$RealClaudeJson = Join-Path $env:USERPROFILE '.claude.json'
$RealDataDirExisted = Test-Path -LiteralPath $RealDataDir
$RealClaudeJsonExisted = Test-Path -LiteralPath $RealClaudeJson

# 砂場の env。PATH はランナーのまま（偽 claude の置き場は載せない = 入れた直後の状態）
function New-SandboxEnv([string]$PathValue) {
    $vars = @{}
    foreach ($entry in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
        $vars[[string]$entry.Key] = [string]$entry.Value
    }
    foreach ($k in @($vars.Keys)) {
        if ($k -like 'TAKO_*' -or $k -in @('HOME', 'CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'CLAUDECODE')) { $vars.Remove($k) }
    }
    $vars['USERPROFILE'] = $HomeDir
    $vars['APPDATA'] = $AppData
    $vars['LOCALAPPDATA'] = $LocalAppData
    $vars['TEMP'] = $TmpDir
    $vars['TMP'] = $TmpDir
    # 判定は日本語の字面で見る（表示言語の既定が OS の言語に従うので固定する。#2365）
    $vars['TAKO_LANG'] = 'ja'
    if ($PathValue) { $vars['PATH'] = $PathValue }
    $vars
}

# 子を起こして待つ。stdout / stderr は UTF-8 で読む（tako はパイプへ UTF-8 で書く）
function Invoke-Sandboxed {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$Stdin,
        [string]$PathValue,
        [int]$TimeoutSec = 600
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
    $psi.WorkingDirectory = $HomeDir
    $psi.Environment.Clear()
    $vars = New-SandboxEnv $PathValue
    foreach ($k in $vars.Keys) { $psi.Environment[$k] = $vars[$k] }

    $started = Get-Date
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        if ($Stdin) { $p.StandardInput.Write($Stdin) }
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

function Invoke-Tako([string[]]$Arguments, [string]$Stdin) {
    Invoke-Sandboxed -FilePath $TakoExe -Arguments $Arguments -Stdin $Stdin
}

# 失敗したときに読む材料（tako の出力の全文・偽 claude の呼ばれ方）
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

function Assert-Contains($Result, [string]$Needle, [string]$Label, [string]$Why) {
    if (-not ($Result.Err.Contains($Needle) -or $Result.Out.Contains($Needle))) {
        Show-Result $Result $Label
        Fail "$Label の出力に「$Needle」が無い（$Why）"
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

# 偽 claude が答えなかった呼び出し（= 対話起動）が無いこと
function Assert-NoAgentLaunch {
    $log = Join-Path $HomeDir 'fake-claude-unexpected.log'
    if (Test-Path -LiteralPath $log) {
        Get-Content -LiteralPath $log | ForEach-Object { Write-Host "   | $_" }
        Fail '偽 claude が答えない呼び出し（対話起動）があった。setup は agent を対話起動しない（#262 / #2285）'
    }
}

# ~/.claude.json の tako の登録（無ければ $null）
function Get-ClaudeMcpEntry {
    $path = Join-Path $HomeDir '.claude.json'
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $json = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json -Depth 32
    if (-not ($json.PSObject.Properties.Name -contains 'mcpServers')) { return $null }
    $servers = $json.mcpServers
    if ($null -eq $servers -or -not ($servers.PSObject.Properties.Name -contains 'tako')) { return $null }
    $servers.tako
}

function Assert-McpRegisteredToTako([string]$Why) {
    $entry = Get-ClaudeMcpEntry
    Assert-That ($null -ne $entry) "砂場の ~/.claude.json に tako の MCP 登録が無い（$Why）"
    Assert-That ([string]::Equals([string]$entry.command, $TakoExe, [System.StringComparison]::OrdinalIgnoreCase)) `
        "MCP の登録先が検査中の tako.exe でない: $($entry.command)（期待 $TakoExe。$Why）"
    Assert-That ((@($entry.args) -join ' ') -eq 'mcp serve') "MCP の登録の引数が mcp serve でない: $(@($entry.args) -join ' ')"
}

# ユーザー環境変数 Path に dir がちょうど 1 回入っているか（入っていない / 重複を名指す）
function Assert-UserPathHas([string]$Dir, [string]$Why) {
    $now = Get-TakoUserPath
    $n = Get-TakoPathEntryCount -Entries $now.Entries -Dir $Dir
    Assert-That ($n -eq 1) "ユーザー環境変数 Path に $Dir が $n 回入っている（期待 1 回。$Why）。いまの値: $($now.Raw)"
}

function Assert-UserPathLacks([string]$Dir, [string]$Why) {
    $now = Get-TakoUserPath
    $n = Get-TakoPathEntryCount -Entries $now.Entries -Dir $Dir
    Assert-That ($n -eq 0) "ユーザー環境変数 Path に $Dir が残っている（$n 回。$Why）。いまの値: $($now.Raw)"
}

# 検査の前から在った要素が、同じ順で残っていること（tako の足し引きが他を触らない）
function Assert-UserPathKeepsOthers([string]$Why) {
    $now = Get-TakoUserPath
    $others = @($now.Entries | Where-Object {
            (Get-TakoPathEntryCount -Entries @($_) -Dir $ClaudeBin) -eq 0 -and
            (Get-TakoPathEntryCount -Entries @($_) -Dir $TakoDir) -eq 0
        })
    $diff = Compare-TakoPathEntries -Expected $Before.Entries -Actual $others
    Assert-That ($null -eq $diff) "tako が足した以外のユーザー環境変数 Path が変わった: $diff（$Why）"
}

# 新しく開いたシェル（システム + ユーザーの Path を読み直した cmd）から name が引けるか
function Assert-FreshShellFinds([string]$Name, [string]$Expected) {
    $r = Invoke-Sandboxed -FilePath $env:ComSpec -Arguments @('/d', '/c', 'where', $Name) -PathValue (Get-TakoFreshLoginPath)
    $hits = @($r.Out -split "`r?`n" | Where-Object { $_ -ne '' })
    $found = @($hits | Where-Object { [string]::Equals($_.Trim(), $Expected, [System.StringComparison]::OrdinalIgnoreCase) })
    if ($found.Count -eq 0) {
        Show-Result $r "where $Name"
        Fail "新しく開いたシェルから $Name が $Expected に解決されない（ユーザー環境変数 Path の追加が効いていない）"
    }
}

$Before = Get-TakoUserPath
Write-Host "== tako setup の通し検査（Windows・#2767）=="
Write-Host "   tako.exe : $TakoExe"
Write-Host "   砂場     : $Root"
Write-Host "   検査前のユーザー環境変数 Path: 要素 $($Before.Entries.Count) 個（$($Before.Kind)）"

$failed = $null
try {
    # --- 偽 claude ------------------------------------------------------------
    $script:Stage = '偽 claude を組む'
    $fakeSrc = Join-Path $PSScriptRoot 'fake-claude.rs'
    & rustc --edition 2021 -C debuginfo=0 -o $ClaudeExe $fakeSrc
    Assert-That ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $ClaudeExe)) "rustc で $fakeSrc を組めなかった（exit $LASTEXITCODE）"
    Assert-That ((Get-TakoPathEntryCount -Entries $Before.Entries -Dir $ClaudeBin) -eq 0) "検査前から $ClaudeBin が Path に入っている（砂場が新しくない）"
    Assert-That ((Get-TakoPathEntryCount -Entries $Before.Entries -Dir $TakoDir) -eq 0) "検査前から $TakoDir が Path に入っている（前の検査の残りか）"

    # --- 1. 初回起動の自動の段 ----------------------------------------------------
    $script:Stage = '1. 初回起動の自動の段（tako setup --yes --answers -）'
    Write-Host "-- $script:Stage"
    $r = Invoke-Tako @('setup', '--yes', '--answers', '-') $AutoSetupAnswers
    Assert-Exit0 $r '自動の段'
    Assert-Contains $r $SummaryChanged '自動の段' '初回は変更の要約が出る（GUI の welcome::parse_auto_summary が読む見出し）'
    Assert-That (Test-Path -LiteralPath $DataDir -PathType Container) "データの置き場が %APPDATA%\tako（$DataDir）にできていない"
    $config = @(Get-ChildItem -LiteralPath $DataDir -Recurse -File -Filter 'config.yaml' |
            Where-Object { (Get-Content -LiteralPath $_.FullName -Raw) -match '(?m)^\s+completed:\s*true\s*$' })
    Assert-That ($config.Count -ge 1) "setup の完了の印（config.yaml の setup.completed: true）が $DataDir の下に無い"
    Assert-That (Test-Path -LiteralPath (Join-Path $HomeDir '.claude\CLAUDE.md') -PathType Leaf) '指示ファイル ~/.claude/CLAUDE.md が作られていない'
    Assert-UserPathHas $TakoDir 'setup の tako CLI の段（#1502）が tako.exe の置き場を足す'
    Assert-UserPathHas $ClaudeBin 'setup の PATH の段が入れた直後の claude の置き場を足す'
    Assert-UserPathKeepsOthers 'setup は Path の末尾へ足すだけ'
    Assert-McpRegisteredToTako '検出した claude へ setup が登録する'
    Assert-NoAgentLaunch

    # --- 2. 明示の答え ----------------------------------------------------------
    $script:Stage = '2. tako setup --yes --answers @<file>'
    Write-Host "-- $script:Stage"
    $answersFile = Join-Path $Root 'answers.json'
    Set-Content -LiteralPath $answersFile -Encoding utf8NoBOM -Value (@{
            selected_agent = 'claude'
            provider_plans = @{ claude = 'max' }
            sleep_guard    = @{ mode = 'while-agents-running'; power = 'ac-only' }
            install        = $false
            launch_agent   = 'none'
        } | ConvertTo-Json -Depth 4)
    $r = Invoke-Tako @('setup', '--yes', '--answers', "@$answersFile")
    Assert-Exit0 $r '明示の答えの setup'
    Assert-Contains $r '[input] setup agent: claude' '明示の答えの setup' 'answers の selected_agent を使う'
    Assert-Contains $r '[input] claude プラン: max' '明示の答えの setup' 'answers の provider_plans を使う'
    Assert-Contains $r $SummaryChanged '明示の答えの setup' 'プランの変更が要約に出る'
    $r = Invoke-Tako @('setup', '--yes', '--answers', "@$answersFile")
    Assert-Exit0 $r '2 回目の setup'
    Assert-Contains $r $SummaryUnchanged '2 回目の setup' '同じ答えの 2 回目は何も変えない（冪等）'
    Assert-UserPathHas $TakoDir '2 回目でも重複して足さない'
    Assert-UserPathHas $ClaudeBin '2 回目でも重複して足さない'
    Assert-NoAgentLaunch

    # --- 3. 環境チェック ----------------------------------------------------------
    $script:Stage = '3. tako setup --check'
    Write-Host "-- $script:Stage"
    $r = Invoke-Tako @('setup', '--check')
    Assert-Exit0 $r 'setup --check'
    Assert-Contains $r 'tako セットアップ 環境チェック' 'setup --check' '環境チェックの見出し（HealthText::SetupCheckTitle）'
    Assert-Contains $r '[OK] tako CLI の PATH' 'setup --check' 'setup が足した Path を診断も「使える」と読む（#1502 / #1505）'
    Assert-NoAgentLaunch

    # --- 4. bootstrap undo-path → path → undo-path ----------------------------------
    $script:Stage = '4. tako setup bootstrap undo-path / path'
    Write-Host "-- $script:Stage"
    $r = Invoke-Tako @('setup', 'bootstrap', 'undo-path', '--json')
    Assert-Exit0 $r 'undo-path'
    $j = ConvertFrom-TakoJson $r 'undo-path'
    Assert-That ($j.change -eq 'removed') "undo-path の change が removed でない: $($j.change)"
    Assert-UserPathLacks $ClaudeBin 'undo-path は claude の置き場を外す'
    Assert-UserPathLacks $TakoDir 'undo-path は tako CLI の置き場も外す（#1502）'
    Assert-UserPathKeepsOthers 'undo-path は他の要素を触らない'

    $r = Invoke-Tako @('setup', 'bootstrap', 'undo-path', '--json')
    Assert-Exit0 $r '2 回目の undo-path'
    $j = ConvertFrom-TakoJson $r '2 回目の undo-path'
    Assert-That ($j.change -eq 'absent') "外し終えた後の undo-path の change が absent でない: $($j.change)"

    $r = Invoke-Tako @('setup', 'bootstrap', 'path', '--json')
    Assert-Exit0 $r 'bootstrap path'
    $j = ConvertFrom-TakoJson $r 'bootstrap path'
    Assert-That ($j.change -eq 'installed') "bootstrap path の change が installed でない: $($j.change)"
    Assert-That ($j.profile -eq 'HKCU\Environment\Path') "bootstrap path の書き先がユーザー環境変数 Path でない: $($j.profile)"
    Assert-UserPathHas $ClaudeBin 'bootstrap path が claude の置き場を足す'
    Assert-UserPathHas $TakoDir 'bootstrap path が tako CLI の置き場も足す（#1502）'
    Assert-UserPathKeepsOthers 'bootstrap path は末尾へ足すだけ'
    Assert-FreshShellFinds 'claude' $ClaudeExe
    Assert-FreshShellFinds 'tako' $TakoExe
    $fresh = Invoke-Sandboxed -FilePath $env:ComSpec -Arguments @('/d', '/c', 'claude', '--version') -PathValue (Get-TakoFreshLoginPath)
    Assert-That ($fresh.Code -eq 0 -and $fresh.Out.Contains('(Claude Code)')) "新しく開いたシェルから claude --version が動かない（exit $($fresh.Code)）: $($fresh.Out)$($fresh.Err)"

    $r = Invoke-Tako @('setup', 'bootstrap', 'path', '--json')
    Assert-Exit0 $r '2 回目の bootstrap path'
    $j = ConvertFrom-TakoJson $r '2 回目の bootstrap path'
    Assert-That ($j.change -eq 'already_on_path') "2 回目の bootstrap path の change が already_on_path でない: $($j.change)"
    Assert-UserPathHas $ClaudeBin '2 回目でも重複して足さない'

    $r = Invoke-Tako @('setup', 'bootstrap', 'undo-path', '--json')
    Assert-Exit0 $r '最後の undo-path'
    Assert-UserPathLacks $ClaudeBin '最後の undo-path'
    Assert-UserPathLacks $TakoDir '最後の undo-path'
    $diff = Compare-TakoPathEntries -Expected $Before.Entries -Actual (Get-TakoUserPath).Entries
    Assert-That ($null -eq $diff) "path → undo-path の後のユーザー環境変数 Path が検査前と違う: $diff"
    Assert-NoAgentLaunch

    # --- 5. setup-mcp ------------------------------------------------------------
    $script:Stage = '5. tako setup-mcp'
    Write-Host "-- $script:Stage"
    $r = Invoke-Tako @('setup-mcp')
    Assert-Exit0 $r 'setup-mcp（登録済み）'
    $j = ConvertFrom-TakoJson $r 'setup-mcp（登録済み）'
    $claude = @($j.agents | Where-Object { $_.agent -eq 'claude' })
    Assert-That ($claude.Count -eq 1) 'setup-mcp の応答に claude の行が無い'
    Assert-That (-not $claude[0].configured) '登録済みなのに setup-mcp が登録し直した（configured=true）'
    Assert-McpRegisteredToTako '登録済みの setup-mcp は登録を保つ'

    # 登録先の tako が消えた（アンインストール・移動の後）→ 付け替える
    $gone = Join-Path $Root 'gone\tako.exe'
    $claudeJson = Join-Path $HomeDir '.claude.json'
    $json = Get-Content -LiteralPath $claudeJson -Raw | ConvertFrom-Json -Depth 32
    $json.mcpServers.tako.command = $gone
    Set-Content -LiteralPath $claudeJson -Encoding utf8NoBOM -Value ($json | ConvertTo-Json -Depth 32)
    $r = Invoke-Tako @('setup-mcp')
    Assert-Exit0 $r 'setup-mcp（登録先が消えた）'
    $j = ConvertFrom-TakoJson $r 'setup-mcp（登録先が消えた）'
    $claude = @($j.agents | Where-Object { $_.agent -eq 'claude' })
    Assert-That ($claude.Count -eq 1 -and $claude[0].repaired) "登録先が消えたのに setup-mcp が付け替えなかった（repaired=$($claude[0].repaired)）"
    Assert-That ($claude[0].old_command -eq $gone) "付け替える前の登録先が応答に無い: $($claude[0].old_command)"
    Assert-McpRegisteredToTako '消えた登録先を検査中の tako.exe へ付け替える'
    Assert-NoAgentLaunch

    # --- 6. 境界 -----------------------------------------------------------------
    $script:Stage = '6. 境界（未ログイン・不正な answers）'
    Write-Host "-- $script:Stage"
    Set-Content -LiteralPath (Join-Path $HomeDir 'fake-claude-state') -Value 'unauth' -Encoding ascii
    $r = Invoke-Tako @('setup', '--yes', '--answers', '-') $AutoSetupAnswers
    Assert-Exit0 $r '未ログインの setup'
    Assert-Contains $r 'claude は未認証です' '未ログインの setup' '未ログインは残り作業にして止まらない（#1501）'
    Assert-NoAgentLaunch
    Remove-Item -LiteralPath (Join-Path $HomeDir 'fake-claude-state') -Force

    $profilesBefore = @(Get-ChildItem -LiteralPath $DataDir -Recurse -File | ForEach-Object { "$($_.FullName)|$($_.Length)|$($_.LastWriteTimeUtc.Ticks)" })
    $r = Invoke-Tako @('setup', '--yes', '--answers', '{"selected_agent":"bogus"}')
    Assert-That ($r.Code -ne 0) '不正な selected_agent を setup が受け付けた（終了コード 0）'
    Assert-Contains $r 'bogus' '不正な answers' '断りの文が不正な値を名指す'
    $profilesAfter = @(Get-ChildItem -LiteralPath $DataDir -Recurse -File | ForEach-Object { "$($_.FullName)|$($_.Length)|$($_.LastWriteTimeUtc.Ticks)" })
    Assert-That ((Compare-Object $profilesBefore $profilesAfter | Measure-Object).Count -eq 0) '不正な answers で断ったのにデータの置き場のファイルが変わった（書く前に断る）'

    # --- 砂場の外へ書いていない ----------------------------------------------------
    $script:Stage = '隔離'
    Assert-That ((Test-Path -LiteralPath $RealDataDir) -eq $RealDataDirExisted) "砂場の外のデータの置き場 $RealDataDir を作った（隔離が外れている）"
    Assert-That ((Test-Path -LiteralPath $RealClaudeJson) -eq $RealClaudeJsonExisted) "砂場の外の $RealClaudeJson を作った（隔離が外れている）"
} catch {
    $failed = $_
} finally {
    # ユーザー環境変数 Path を検査前へ戻し、戻ったことまで確かめる（失敗しても必ず）
    Restore-TakoUserPath -Snapshot $Before
    $after = Get-TakoUserPath
    if ($after.Raw -ne $Before.Raw -or $after.Exists -ne $Before.Exists) {
        Write-Host "::error::[#2767 後片付け] ユーザー環境変数 Path を検査前へ戻せなかった（前 $($Before.Raw) / 後 $($after.Raw)）"
        if (-not $failed) { $failed = 'Path を戻せなかった' }
    }
    $calls = Join-Path $HomeDir 'fake-claude-calls.log'
    if ($failed -and (Test-Path -LiteralPath $calls)) {
        Write-Host '   --- 偽 claude の呼ばれ方 ---'
        Get-Content -LiteralPath $calls | ForEach-Object { Write-Host "   | $_" }
    }
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failed) {
    $message = if ($failed -is [System.Management.Automation.ErrorRecord]) { $failed.Exception.Message } else { [string]$failed }
    Write-Host "::error::$message"
    exit 1
}
Write-Host '== 通し検査は全段 OK（ユーザー環境変数 Path は検査前へ戻した）=='
exit 0
