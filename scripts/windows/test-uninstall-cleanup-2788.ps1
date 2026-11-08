#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Windows の CI で、インストール → setup → 上書きインストール → アンインストールの一巡を回し、
    tako が外へ書いた連携が外れて data dir が残ることを確かめる（Issue #2788）。

.DESCRIPTION
    段の並び:

      1. 砂場（USERPROFILE / APPDATA / LOCALAPPDATA / TEMP を一時 dir へ）でインストーラーを無人で入れる
      2. 入れた tako.exe で連携を書く: `tako setup --yes --answers -`（偽 claude への MCP 登録・指示ファイル・
         tako mod の skills・ユーザー環境変数 Path）と `tako agents sync-rules`（指示ファイルの同期ブロック）。
         codex の登録は codex が無いので、`codex mcp add` が書く形を置く。WebView2 のキャッシュも置く。
         利用者のもの（自分の MCP 登録・自分で書いた本文）も並べて置く
      3. 上書きインストール = アプリ内更新（#2434 / #2443）と同じ引数（/SILENT … /TAKOWAITPID=）。
         連携がそのまま残り、data dir が 1 バイトも変わらないこと（片付けが走らないこと）
      4. アンインストーラー（アンインストール情報の UninstallString）を無人で走らせる
      5. 連携が外れる（MCP 登録・同期ブロック・mod の skills・Path のエントリ・WebView2 のキャッシュ）。
         data dir は 1 バイトも変わらない。利用者の MCP 登録・本文・入れてある claude の置き場の Path は残る

    アンインストーラーは tako.iss の usUninstall で `{app}\tako.exe setup undo` を起こす（tako_control::setup_undo の
    1 実装）。子は親の env を継ぐので、片付けも砂場の USERPROFILE / APPDATA / LOCALAPPDATA を見る。

    ユーザー環境変数 Path（HKCU）とアンインストール情報（HKCU）だけは砂場に閉じ込められない。Path は最初の値へ
    必ず戻す（失敗しても）。GitHub Actions の使い捨てランナー以外では走らせない。

    失敗は段の名前つきの `::error::` 1 行で名指しし、終了コード 1 で終わる。

.PARAMETER Setup
    検査するインストーラー（build-installer.ps1 が組んだ .exe）。
#>
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Setup)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $RepoRoot 'installer/windows/lib/user-path.ps1')
. (Join-Path $RepoRoot 'installer/windows/lib/verify-assets.ps1')

$script:Stage = '準備'
function Fail([string]$Message) { throw "[#2788 $script:Stage] $Message" }
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { Fail $Message } }

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（実インストールの登録を上書きし、ユーザー環境変数 Path を書き換えて戻す）'
}
$SetupExe = (Resolve-Path -LiteralPath $Setup).ProviderPath

# --- 砂場 ---------------------------------------------------------------------
$Root = Join-Path ([System.IO.Path]::GetTempPath()) "tako-2788-$([System.IO.Path]::GetRandomFileName())"
$HomeDir = Join-Path $Root 'home'
$AppData = Join-Path $HomeDir 'AppData\Roaming'
$LocalAppData = Join-Path $HomeDir 'AppData\Local'
$TmpDir = Join-Path $Root 'tmp'
$InstallDir = Join-Path $Root 'Programs\tako'
$TakoExe = Join-Path $InstallDir 'tako.exe'
$DataDir = Join-Path $AppData 'tako'
$LocalTako = Join-Path $LocalAppData 'tako'
$ClaudeBin = Join-Path $HomeDir '.local\bin'
$ClaudeExe = Join-Path $ClaudeBin 'claude.exe'
$ClaudeJson = Join-Path $HomeDir '.claude.json'
$ClaudeMd = Join-Path $HomeDir '.claude\CLAUDE.md'
$CodexDir = Join-Path $HomeDir '.codex'
$CodexToml = Join-Path $CodexDir 'config.toml'
$SkillsTako = Join-Path $HomeDir '.claude\skills\tako'
$WebView2 = Join-Path $LocalTako 'WebView2'
$LegacyWebView2 = Join-Path $InstallDir 'tako-app.exe.WebView2'
$Logs = Join-Path $Root 'logs'
foreach ($d in @($AppData, $LocalAppData, $TmpDir, $ClaudeBin, $CodexDir, $Logs)) {
    New-Item -ItemType Directory -Path $d -Force | Out-Null
}
$RealDataDir = Join-Path $env:APPDATA 'tako'
$RealClaudeJson = Join-Path $env:USERPROFILE '.claude.json'
$RealDataDirExisted = Test-Path -LiteralPath $RealDataDir
$RealClaudeJsonExisted = Test-Path -LiteralPath $RealClaudeJson

# 子（インストーラー・アンインストーラー・tako.exe）は親の env を継ぐので、このプロセスの env を砂場へ向ける
$SandboxEnv = [ordered]@{
    USERPROFILE  = $HomeDir
    APPDATA      = $AppData
    LOCALAPPDATA = $LocalAppData
    TEMP         = $TmpDir
    TMP          = $TmpDir
    TAKO_LANG    = 'ja'
}
$Removed = @('HOME', 'CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'CLAUDECODE')
$SavedEnv = @{}
foreach ($k in @($SandboxEnv.Keys) + $Removed) { $SavedEnv[$k] = [System.Environment]::GetEnvironmentVariable($k) }
$SavedTako = @{}
foreach ($entry in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
    if ([string]$entry.Key -like 'TAKO_*') { $SavedTako[[string]$entry.Key] = [string]$entry.Value }
}

function Enter-Sandbox {
    foreach ($k in $SavedTako.Keys) { [System.Environment]::SetEnvironmentVariable($k, $null) }
    foreach ($k in $Removed) { [System.Environment]::SetEnvironmentVariable($k, $null) }
    foreach ($k in $SandboxEnv.Keys) { [System.Environment]::SetEnvironmentVariable($k, $SandboxEnv[$k]) }
}

function Exit-Sandbox {
    foreach ($k in $SavedEnv.Keys) { [System.Environment]::SetEnvironmentVariable($k, $SavedEnv[$k]) }
    foreach ($k in $SavedTako.Keys) { [System.Environment]::SetEnvironmentVariable($k, $SavedTako[$k]) }
}

# 子を起こして待つ（終了コードを返す）
function Invoke-Waited([string]$File, [string]$Arguments, [string]$Label, [int]$TimeoutSec = 300) {
    $p = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru
    $null = $p.Handle
    if (-not $p.WaitForExit($TimeoutSec * 1000)) {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        Fail "$Label が $TimeoutSec 秒で終わらなかった"
    }
    Write-Host ("   > {0}（exit {1}）" -f $Label, $p.ExitCode)
    $p.ExitCode
}

# 子を起こす（引数は ArgumentList で 1 つずつ渡す = `--` も素のまま届く。stdout / stderr は UTF-8）
function Invoke-Native([string]$File, [string[]]$Arguments, [string]$Stdin) {
    $name = Split-Path -Leaf $File
    $psi = [System.Diagnostics.ProcessStartInfo]::new($File)
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.WorkingDirectory = $HomeDir
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if ($Stdin) { $p.StandardInput.Write($Stdin) }
    $p.StandardInput.Close()
    if (-not $p.WaitForExit(600000)) { $p.Kill($true); Fail "$name $($Arguments -join ' ') が終わらなかった" }
    $p.WaitForExit()
    $r = [PSCustomObject]@{ Code = $p.ExitCode; Out = $out.Result; Err = $err.Result }
    $p.Dispose()
    Write-Host "   > $name $($Arguments -join ' ')（exit $($r.Code)）"
    if ($r.Code -ne 0) {
        ($r.Out + $r.Err) -split "`r?`n" | ForEach-Object { Write-Host "   | $_" }
        Fail "$name $($Arguments -join ' ') が終了コード $($r.Code) で終わった"
    }
    $r
}

function Invoke-Tako([string[]]$Arguments, [string]$Stdin) { Invoke-Native $TakoExe $Arguments $Stdin }

# data dir の全ファイル（相対パス → SHA256）。1 バイトでも変われば違う
function Get-DataSnapshot {
    $map = [ordered]@{}
    if (Test-Path -LiteralPath $DataDir) {
        Get-ChildItem -LiteralPath $DataDir -Recurse -File | Sort-Object FullName | ForEach-Object {
            $map[$_.FullName.Substring($DataDir.Length)] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        }
    }
    $map
}

function Assert-DataUnchanged($Before, [string]$Why) {
    $after = Get-DataSnapshot
    $diff = Compare-Object @($Before.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) `
        @($after.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" })
    if ($diff) {
        $diff | ForEach-Object { Write-Host "   | $($_.SideIndicator) $($_.InputObject)" }
        Fail "data dir（$DataDir）が変わった（$Why）"
    }
    Write-Host "   [OK] data dir は 1 バイトも変わらない（$($after.Count) ファイル。$Why）"
}

function Get-McpServers([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return @() }
    $json = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -Depth 32
    if (-not ($json.PSObject.Properties.Name -contains 'mcpServers') -or $null -eq $json.mcpServers) { return @() }
    @($json.mcpServers.PSObject.Properties.Name)
}

function Assert-IntegrationsPresent([string]$Why) {
    Assert-That ((Get-McpServers $ClaudeJson) -contains 'tako') "~/.claude.json に tako の MCP 登録が無い（$Why）"
    Assert-That ((Get-Content -LiteralPath $ClaudeMd -Raw).Contains('<!-- BEGIN SYNCED COMMON RULES -->')) "CLAUDE.md に同期ブロックが無い（$Why）"
    Assert-That ((Get-Content -LiteralPath $CodexToml -Raw).Contains('[mcp_servers.tako]')) "codex の config.toml に tako の登録が無い（$Why）"
    Assert-That (Test-Path -LiteralPath (Join-Path $SkillsTako '.tako-managed')) "tako mod の skills（$SkillsTako）が無い（$Why）"
    Assert-That (Test-Path -LiteralPath $WebView2) "WebView2 のキャッシュ（$WebView2）が無い（$Why）"
    $n = Get-TakoPathEntryCount -Entries (Get-TakoUserPath).Entries -Dir $InstallDir
    Assert-That ($n -eq 1) "ユーザー環境変数 Path に $InstallDir が $n 回入っている（期待 1 回。$Why）"
    Write-Host "   [OK] 連携が入っている（MCP 登録・同期ブロック・mod の skills・Path・WebView2。$Why）"
}

$Before = Get-TakoUserPath
Write-Host '== アンインストールの片付けの一巡（Windows・#2788）=='
Write-Host "   インストーラー: $SetupExe"
Write-Host "   砂場          : $Root"

$failed = $null
$installed = $false
try {
    # 偽 claude は砂場へ入る前に組む（USERPROFILE を差し替えた後の rustc は rustup のツールチェーンを見失う）
    $script:Stage = '偽 claude を組む'
    $fakeSrc = Join-Path $PSScriptRoot 'fake-claude.rs'
    & rustc --edition 2021 -C debuginfo=0 -o $ClaudeExe $fakeSrc
    Assert-That ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $ClaudeExe)) "rustc で $fakeSrc を組めなかった"

    Enter-Sandbox

    # --- 1. インストール ---------------------------------------------------------
    $script:Stage = '1. インストール'
    Write-Host "-- $script:Stage"
    $code = Invoke-Waited $SetupExe ('/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /NOICONS /NOCLOSEAPPLICATIONS ' +
        "/MERGETASKS=`"addtopath,!desktopicon`" /DIR=`"$InstallDir`" /LOG=`"$(Join-Path $Logs 'install.log')`"") 'インストール'
    Assert-That ($code -eq 0) "インストーラーが終了コード $code で失敗した"
    $installed = $true
    Assert-That (Test-Path -LiteralPath $TakoExe) "インストール先に tako.exe が無い: $TakoExe"

    # --- 2. setup と連携 ---------------------------------------------------------
    $script:Stage = '2. setup で連携を書く'
    Write-Host "-- $script:Stage"
    $null = Invoke-Tako @('setup', '--yes', '--answers', '-') '{"install":false,"launch_agent":"none"}'
    $userText = Get-Content -LiteralPath $ClaudeMd -Raw
    # 利用者のもの: 自分の MCP 登録（偽 claude の mcp add = 本物と同じ ~/.claude.json の形）
    $null = Invoke-Native $ClaudeExe @('mcp', 'add', '--scope', 'user', '--transport', 'stdio', 'mine', '--', 'C:\Windows\System32\where.exe')
    $rules = Join-Path $Root 'rules.md'
    Set-Content -LiteralPath $rules -Encoding utf8NoBOM -Value "共通ルール（#2788 の検査）"
    $null = Invoke-Tako @('agents', 'sync-rules', '--source', $rules)
    # codex は入っていないので、`codex mcp add` + tako の env_vars が書く形を置く（利用者の登録も並べる）
    $codexMine = "model = `"gpt-5`"`n`n[mcp_servers.mine]`ncommand = `"npx`"`nargs = [`"my-server`"]`n"
    $takoToml = ($TakoExe -replace '\\', '\\')
    Set-Content -LiteralPath $CodexToml -Encoding utf8NoBOM -NoNewline -Value ($codexMine +
        "`n[mcp_servers.tako]`nenv_vars = [`"TAKO_SOCKET`", `"TAKO_TOKEN`"]`ncommand = `"$takoToml`"`nargs = [`"mcp`", `"serve`"]`n")
    # WebView2 のキャッシュ（新しい置き場と、入れた先の隣の旧い置き場）
    foreach ($d in @((Join-Path $WebView2 'EBWebView\Default'), (Join-Path $LegacyWebView2 'EBWebView'))) {
        New-Item -ItemType Directory -Path $d -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $d 'cache-data') -Value 'cache'
    }
    Set-Content -LiteralPath (Join-Path $DataDir 'keep-me.txt') -Value 'データは残す'
    Assert-IntegrationsPresent 'setup の後'
    $dataBefore = Get-DataSnapshot

    # --- 3. 上書きインストール（アプリ内更新の経路）-----------------------------------
    # tako の installer_update::installer_args と同じ形（/TAKORELAUNCH は GUI を起こすので渡さない。
    # /TAKOWAITPID は居ない pid = すぐ進む）。アンインストーラーを起こさない = 片付けは走らない
    $script:Stage = '3. 上書きインストール（アプリ内更新の経路）'
    Write-Host "-- $script:Stage"
    $code = Invoke-Waited $SetupExe ('/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /SP- ' +
        "/LOG=`"$(Join-Path $Logs 'update.log')`" /TAKOWAITPID=0") '上書きインストール'
    Assert-That ($code -eq 0) "上書きインストールが終了コード $code で失敗した"
    Assert-That (Test-Path -LiteralPath $TakoExe) "上書きインストールの後に tako.exe が無い（入れた先が変わった？）"
    Assert-IntegrationsPresent '上書きインストールの後（片付けは走らない）'
    Assert-DataUnchanged $dataBefore '上書きインストールの後'

    # --- 4. 外すものの見立て（dry-run = 何も変えない）。アンインストーラーは出力を捨てるので、
    # 同じ 1 実装の判断をここで CI のログへ残す（何が外れる予定で、何を理由つきで残すか）
    $script:Stage = '4. 外すものの見立て（tako setup undo --dry-run --json）'
    Write-Host "-- $script:Stage"
    $plan = (Invoke-Tako @('setup', 'undo', '--dry-run', '--json')).Out | ConvertFrom-Json -Depth 32
    foreach ($item in @($plan.items | Where-Object { $_.outcome -ne 'absent' })) {
        $why = if ($item.PSObject.Properties.Name -contains 'reason') { "（$($item.reason.code): $($item.reason.message)）" } else { '' }
        Write-Host "   $($item.outcome) $($item.kind) $($item.path)$why"
    }
    Write-Host "   summary: $($plan.summary | ConvertTo-Json -Compress)"
    foreach ($item in @($plan.items | Where-Object { $_.kind -eq 'mcp' })) { Write-Host "   mcp の項目: $($item | ConvertTo-Json -Compress)" }
    Write-Host "   env: CODEX_HOME=[$env:CODEX_HOME] HOME=[$env:HOME] USERPROFILE=[$env:USERPROFILE]"
    Write-Host "   codex の config.toml: $CodexToml（在る: $(Test-Path -LiteralPath $CodexToml)）"
    $codexBytes = [System.IO.File]::ReadAllBytes($CodexToml)
    Write-Host "   codex の config.toml の先頭: $(($codexBytes | Select-Object -First 8 | ForEach-Object { '{0:X2}' -f $_ }) -join ' ')・CR を含む: $($codexBytes -contains 13)"
    Assert-That (@($plan.items | Where-Object { $_.kind -eq 'mcp' -and $_.agent -eq 'codex' -and $_.outcome -eq 'would_remove' }).Count -eq 1) `
        'dry-run の見立てで codex の MCP 登録が would_remove でない（上の行の理由を見る）'
    Assert-DataUnchanged $dataBefore 'dry-run の後'

    # --- 5. アンインストール ------------------------------------------------------
    $script:Stage = '5. アンインストール'
    Write-Host "-- $script:Stage"
    $entry = Get-TakoUninstallEntry $InstallDir
    Assert-That ($null -ne $entry) "インストール先 $InstallDir のアンインストール情報（UninstallString）が無い"
    $uninstaller = $entry.UninstallString.Trim().Trim('"')
    $uninstallLog = Join-Path $Logs 'uninstall.log'
    $code = Invoke-Waited $uninstaller "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$uninstallLog`"" 'アンインストール'
    Assert-That ($code -eq 0) "アンインストーラーが終了コード $code で失敗した"
    # アンインストーラーは自分の写しを一時 dir で起こし直して元は先に抜ける（#2767）。消えるまで待つ
    $deadline = (Get-Date).AddSeconds(120)
    while ((Test-Path -LiteralPath $TakoExe) -or ($null -ne (Get-TakoUninstallEntry $InstallDir)) -or
        ((Get-TakoPathEntryCount -Entries (Get-TakoUserPath).Entries -Dir $InstallDir) -ne 0)) {
        if ((Get-Date) -gt $deadline) { Fail 'アンインストールが 120 秒で終わらなかった' }
        Start-Sleep -Milliseconds 500
    }
    $installed = $false
    $line = @(Get-Content -LiteralPath $uninstallLog | Where-Object { $_ -match 'tako setup undo' })
    $line | ForEach-Object { Write-Host "   アンインストールのログ: $_" }
    Assert-That ($line.Count -ge 1 -and ($line -join "`n") -match 'finished \(exit 0\)') `
        'アンインストールのログに tako setup undo の終了（exit 0）が無い（usUninstall で起こしていない）'

    # --- 6. 連携が外れ、データと利用者のものは残る -----------------------------------
    $script:Stage = '6. 片付けの結果'
    Write-Host "-- $script:Stage"
    $servers = Get-McpServers $ClaudeJson
    Assert-That (-not ($servers -contains 'tako')) "~/.claude.json に tako の MCP 登録が残った: $($servers -join ', ')"
    Assert-That ($servers -contains 'mine') "利用者の MCP 登録 mine が消えた: $($servers -join ', ')"
    Assert-That ((Get-Content -LiteralPath $ClaudeMd -Raw) -eq $userText) 'CLAUDE.md が同期の前の本文へ戻らない（利用者の本文が変わった・ブロックが残った）'
    Assert-That (-not (Test-Path -LiteralPath (Join-Path $CodexDir 'AGENTS.md'))) '同期が作った AGENTS.md（ブロックだけ）が残った'
    $codexAfter = Get-Content -LiteralPath $CodexToml -Raw
    Assert-That ($codexAfter -eq $codexMine) "codex の config.toml が tako の表だけを外した形でない:`n$codexAfter"
    Assert-That (-not (Test-Path -LiteralPath $SkillsTako)) "tako mod の skills（$SkillsTako）が残った"
    Assert-That (-not (Test-Path -LiteralPath $WebView2)) "WebView2 のキャッシュ（$WebView2）が残った"
    Assert-That (-not (Test-Path -LiteralPath $LocalTako)) "空になった $LocalTako が残った"
    Assert-That (-not (Test-Path -LiteralPath $LegacyWebView2)) "入れた先の隣の旧い WebView2（$LegacyWebView2）が残った"
    $now = Get-TakoUserPath
    Assert-That ((Get-TakoPathEntryCount -Entries $now.Entries -Dir $ClaudeBin) -eq 1) `
        "入れてある claude の置き場（$ClaudeBin）が Path から外れた（共有の置き場は残す）"
    Write-Host '   [OK] MCP 登録・同期ブロック・mod の skills・WebView2 のキャッシュ・Path の入れた先が外れた'
    Write-Host '   [OK] 利用者の MCP 登録・本文・claude の置き場の Path は残った'
    Assert-DataUnchanged $dataBefore 'アンインストールの後'
    Assert-That (Test-Path -LiteralPath (Join-Path $DataDir 'keep-me.txt')) 'data dir の利用者のファイルが消えた'

    $script:Stage = '隔離'
    Assert-That ((Test-Path -LiteralPath $RealDataDir) -eq $RealDataDirExisted) "砂場の外のデータの置き場 $RealDataDir を作った"
    Assert-That ((Test-Path -LiteralPath $RealClaudeJson) -eq $RealClaudeJsonExisted) "砂場の外の $RealClaudeJson を作った"
} catch {
    $failed = $_
} finally {
    Exit-Sandbox
    if ($installed) {
        # 途中で落ちたら入れたものを外す（次の段へ残さない）
        $entry = Get-TakoUninstallEntry $InstallDir
        if ($null -ne $entry) {
            Start-Process -FilePath $entry.UninstallString.Trim().Trim('"') -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -Wait -ErrorAction SilentlyContinue
        }
    }
    if ($failed) {
        foreach ($log in Get-ChildItem -LiteralPath $Logs -File -ErrorAction SilentlyContinue) {
            Write-Host "   --- $($log.Name) ---"
            Get-Content -LiteralPath $log.FullName | Select-Object -Last 60 | ForEach-Object { Write-Host "   | $_" }
        }
    }
    Restore-TakoUserPath -Snapshot $Before
    $after = Get-TakoUserPath
    if ($after.Raw -ne $Before.Raw -or $after.Exists -ne $Before.Exists) {
        Write-Host "::error::[#2788 後片付け] ユーザー環境変数 Path を検査前へ戻せなかった（前 $($Before.Raw) / 後 $($after.Raw)）"
        if (-not $failed) { $failed = 'Path を戻せなかった' }
    }
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failed) {
    $message = if ($failed -is [System.Management.Automation.ErrorRecord]) { $failed.Exception.Message } else { [string]$failed }
    Write-Host "::error::$message"
    exit 1
}
Write-Host '== 片付けの一巡は全段 OK（ユーザー環境変数 Path は検査前へ戻した）=='
exit 0
