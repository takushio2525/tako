#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    グループポリシーの実行ポリシーを置く・置く前へ戻す 1 実装（Issue #2790 / #2791）。

.DESCRIPTION
    CI の検査（scripts/windows/test-install-execution-policy-2790.ps1 と
    scripts/windows/test-agent-install-2791.ps1）が、管理端末（学校・会社の PC）と同じ形の
    グループポリシーを置いてエージェント CLI の導入代行を確かめるのに使う。置き方と戻し方を 2 つ持たない。

    置き場は Windows PowerShell 5.1（SOFTWARE\Policies\Microsoft\Windows\PowerShell）と pwsh 7
    （SOFTWARE\Policies\Microsoft\PowerShellCore。pwsh 7 は既定で 5.1 の置き場を読まない）の両方:

      Set-Policy 'HKLM' 'AllSigned'  … EnableScripts = 1 + ExecutionPolicy = AllSigned
      Set-Policy 'HKCU' 'Restricted' … EnableScripts = 0（「スクリプトの実行を有効にする」が無効）

    置く前の状態（鍵が在ったか・2 つの値）を Get-PolicySnapshot で取り、Restore-Policy でその状態へ
    戻す（作った鍵だけを消す）。.NET のレジストリ API だけを使うので、ポリシーを置いている間も
    モジュールやスクリプトファイルを読み込まない。

    HKLM / HKCU のポリシーは機械全体に効くので、GitHub Actions の使い捨てランナー以外では使わない
    （呼び手のスクリプトが GITHUB_ACTIONS を確かめてから dot-source する）。

.EXAMPLE
    . (Join-Path $PSScriptRoot 'lib/execution-policy.ps1')
    $before = @(Get-PolicySnapshot 'HKCU')
    try { Set-Policy 'HKCU' 'Restricted'; <検査> } finally { Restore-Policy 'HKCU' $before }
#>

# Windows PowerShell 5.1 と pwsh 7 のグループポリシーの置き場
$PolicyKeys = @(
    'SOFTWARE\Policies\Microsoft\Windows\PowerShell',
    'SOFTWARE\Policies\Microsoft\PowerShellCore'
)

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

# 実効の実行ポリシーの一覧（ログ用。5.1 の実物に聞く）。1 行の文字列で返す。
# 呼び手（pwsh 7）の PSModulePath は渡さない: 5.1 が pwsh 7 の置き場の Microsoft.PowerShell.Security を拾い、
# AllSigned の下で読めずに Get-ExecutionPolicy が落ちる（#2790 の CI で実測）。ポリシーを置いた後にも呼ぶため
function Get-PolicyListText {
    $saved = $env:PSModulePath
    try {
        $env:PSModulePath = $null
        $list = & powershell.exe -NoLogo -NoProfile -Command "Get-ExecutionPolicy -List | ForEach-Object { '' + `$_.Scope + '=' + `$_.ExecutionPolicy }" 2>&1
    } finally {
        $env:PSModulePath = $saved
    }
    (@($list) | ForEach-Object { [string]$_ }) -join ', '
}
