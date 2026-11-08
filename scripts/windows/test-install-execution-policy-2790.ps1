#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    グループポリシーで実行ポリシーを強制した状態で、エージェント CLI の導入代行の起こし方を確かめる（Issue #2790）。

.DESCRIPTION
    導入代行は取得した install.ps1 を #2790 より前は `-ExecutionPolicy Bypass -File` で起こしていた。
    Bypass は Process スコープの指定なので、グループポリシー（MachinePolicy / UserPolicy）で AllSigned /
    Restricted を強制した管理端末では負ける。#2790 からは本文をテキストで読んで -EncodedCommand の
    起動片から走らせる（実行ポリシーの対象外）。ここではグループポリシーのレジストリを実際に置き、

      HKLM MachinePolicy = AllSigned  （EnableScripts = 1 + ExecutionPolicy = AllSigned）
      HKCU UserPolicy    = Restricted （EnableScripts = 0 = 「スクリプトの実行を有効にする」が無効）

    の 2 通りで cargo のテスト crates/tako-core/tests/issue2790_install_execution_policy_windows.rs を
    TAKO_2790_POLICY=<スコープ>:<値> 付きで走らせる。テストは「ポリシーが効いている」を確かめたうえで、
    新しい形は偽の install.ps1 が走って印が残り、修正前の形は同じ条件で弾かれ、失敗時に案内する
    手で打つ 1 行（irm <url> | iex）は走ることを見る（外部の本物の install.ps1 は走らせない）。

    置き場は Windows PowerShell 5.1（SOFTWARE\Policies\Microsoft\Windows\PowerShell）と pwsh 7
    （SOFTWARE\Policies\Microsoft\PowerShellCore。pwsh 7 は既定で 5.1 の置き場を読まない）の両方。
    置く前の状態（鍵が在ったか・値）を取り、ケースごとに finally で戻して、戻ったことまで読み戻して確かめる。
    HKLM / HKCU のポリシーは機械全体に効くので、GitHub Actions の使い捨てランナー以外では走らせない。

    ポリシーを置いている間は、このスクリプト自身もモジュールやスクリプトファイルを読み込まない
    （.NET のレジストリ API と cargo だけを使う）。テストは置く前に --no-run で組んでおく
    （ポリシーの下で cargo の build.rs を走らせない）。

    失敗は段の名前つきの `::error::` 1 行で名指しし、終了コード 1 で終わる。

.EXAMPLE
    pwsh -File scripts/windows/test-install-execution-policy-2790.ps1
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用（グループポリシーのレジストリを置く）' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（HKLM / HKCU のグループポリシーを書き換えて戻す。実機では走らせない）'
}

$TestName = 'issue2790_install_execution_policy_windows'
# Windows PowerShell 5.1 と pwsh 7 のグループポリシーの置き場
$PolicyKeys = @(
    'SOFTWARE\Policies\Microsoft\Windows\PowerShell',
    'SOFTWARE\Policies\Microsoft\PowerShellCore'
)
$Cases = @(
    [pscustomobject]@{ Hive = 'HKLM'; Scope = 'MachinePolicy'; Value = 'AllSigned' },
    [pscustomobject]@{ Hive = 'HKCU'; Scope = 'UserPolicy'; Value = 'Restricted' }
)

$script:Stage = '準備'

function Say([string]$Message) {
    # ポリシーの窓の中でも呼べるよう、モジュールの cmdlet ではなく .NET で書く
    [Console]::WriteLine("[#2790 $script:Stage] $Message")
}

function Get-HiveRoot([string]$Hive) {
    if ($Hive -eq 'HKLM') { return [Microsoft.Win32.Registry]::LocalMachine }
    return [Microsoft.Win32.Registry]::CurrentUser
}

# $Sub の途中で最初に無い鍵（無ければ $null）。戻すときはここから下を丸ごと消す（作った分だけ消す）
function Get-FirstMissing($Root, [string]$Sub) {
    $path = ''
    foreach ($part in $Sub.Split('\')) {
        $path = if ($path) { "$path\$part" } else { $part }
        $key = $Root.OpenSubKey($path)
        if ($null -eq $key) { return $path }
        $key.Dispose()
    }
    return $null
}

# 置く前の状態（鍵が在ったか・2 つの値）
function Get-PolicySnapshot([string]$Hive) {
    $root = Get-HiveRoot $Hive
    $out = @()
    foreach ($sub in $PolicyKeys) {
        $missing = Get-FirstMissing $root $sub
        $enable = $null
        $policy = $null
        if ($null -eq $missing) {
            $key = $root.OpenSubKey($sub)
            try {
                $enable = $key.GetValue('EnableScripts')
                $policy = $key.GetValue('ExecutionPolicy')
            } finally { $key.Dispose() }
        }
        $out += [pscustomobject]@{ Sub = $sub; Missing = $missing; EnableScripts = $enable; ExecutionPolicy = $policy }
    }
    return $out
}

function Format-Snapshot($Snapshot) {
    ($Snapshot | ForEach-Object {
        '{0}: missing={1} EnableScripts={2} ExecutionPolicy={3}' -f $_.Sub, $_.Missing, $_.EnableScripts, $_.ExecutionPolicy
    }) -join ' | '
}

function Set-Policy([string]$Hive, [string]$Value) {
    $root = Get-HiveRoot $Hive
    foreach ($sub in $PolicyKeys) {
        $key = $root.CreateSubKey($sub)
        try {
            if ($Value -eq 'Restricted') {
                # グループポリシーの「スクリプトの実行を有効にする」を無効にした形（EnableScripts = 0 だけ）
                $key.SetValue('EnableScripts', 0, [Microsoft.Win32.RegistryValueKind]::DWord)
                $key.DeleteValue('ExecutionPolicy', $false)
            } else {
                $key.SetValue('EnableScripts', 1, [Microsoft.Win32.RegistryValueKind]::DWord)
                $key.SetValue('ExecutionPolicy', $Value, [Microsoft.Win32.RegistryValueKind]::String)
            }
        } finally { $key.Dispose() }
    }
}

function Restore-Policy([string]$Hive, $Snapshot) {
    $root = Get-HiveRoot $Hive
    foreach ($s in $Snapshot) {
        if ($null -ne $s.Missing) {
            $root.DeleteSubKeyTree($s.Missing, $false)
            continue
        }
        $key = $root.OpenSubKey($s.Sub, $true)
        try {
            if ($null -eq $s.EnableScripts) { $key.DeleteValue('EnableScripts', $false) }
            else { $key.SetValue('EnableScripts', $s.EnableScripts, [Microsoft.Win32.RegistryValueKind]::DWord) }
            if ($null -eq $s.ExecutionPolicy) { $key.DeleteValue('ExecutionPolicy', $false) }
            else { $key.SetValue('ExecutionPolicy', $s.ExecutionPolicy, [Microsoft.Win32.RegistryValueKind]::String) }
        } finally { $key.Dispose() }
    }
}

# 実効の実行ポリシーの一覧（ログ用。5.1 の実物に聞く）
function Show-PolicyList([string]$When) {
    $list = & powershell.exe -NoLogo -NoProfile -Command "Get-ExecutionPolicy -List | ForEach-Object { '' + `$_.Scope + '=' + `$_.ExecutionPolicy }"
    Say ("{0}: {1}" -f $When, ($list -join ', '))
}

# テストは置く前に組む（ポリシーの下で cargo の build.rs を走らせない）
$script:Stage = '組み立て'
& cargo test -p tako-core --test $TestName --no-run
if ($LASTEXITCODE -ne 0) {
    [Console]::WriteLine("::error::[#2790 組み立て] cargo test --no-run が終了コード $LASTEXITCODE")
    exit 1
}
Show-PolicyList '置く前'

$failures = @()
foreach ($case in $Cases) {
    $script:Stage = '{0} {1}={2}' -f $case.Hive, $case.Scope, $case.Value
    $before = @(Get-PolicySnapshot $case.Hive)
    Say ('置く前: ' + (Format-Snapshot $before))
    $code = -1
    try {
        Set-Policy $case.Hive $case.Value
        $env:TAKO_2790_POLICY = '{0}:{1}' -f $case.Scope, $case.Value
        & cargo test -p tako-core --test $TestName -- --nocapture --test-threads=1
        $code = $LASTEXITCODE
    } finally {
        # 置いたポリシーはここで必ず戻す（テストが落ちても・例外でも）
        Restore-Policy $case.Hive $before
        $env:TAKO_2790_POLICY = $null
    }
    $after = @(Get-PolicySnapshot $case.Hive)
    $beforeText = Format-Snapshot $before
    $afterText = Format-Snapshot $after
    Say ('戻した後: ' + $afterText)
    if ($afterText -ne $beforeText) {
        $failures += "[#2790 $script:Stage] ポリシーを戻せていない（前: $beforeText / 後: $afterText）"
    }
    if ($code -ne 0) {
        $failures += "[#2790 $script:Stage] ポリシーを強制した状態のテストが終了コード $code"
    }
}
$script:Stage = '後片付け'
Show-PolicyList '戻した後'

if ($failures.Count -gt 0) {
    foreach ($f in $failures) { [Console]::WriteLine("::error::$f") }
    exit 1
}
Say '2 通り（HKLM AllSigned / HKCU Restricted）とも、新しい形は走り修正前の形は弾かれた'
