<#
.SYNOPSIS
    リポジトリの .ps1 が Windows PowerShell 5.1 で「読める」か「理由を出して止まる」かを確かめる（#2329）。

.DESCRIPTION
    5.1 は BOM の無い .ps1 を ANSI コードページ（英語版 = CP1252 / 日本語版 = CP932）として読む。
    BOM の無い UTF-8 で日本語を書くと、5.1 では文字列やコメントの境目が崩れて構文エラーになる
    （CP932 では行末の改行まで食われ、構文エラーも出さずに次の行が消えることもある）。
    そこで .ps1 はどれも次のどちらかでなければならない:

      readable … 5.1 が読んだ中身が UTF-8 として読んだ中身と同じで、構文エラーが 0 件
                 （UTF-8 with BOM のファイルか、ASCII だけのファイル）
      stops    … 5.1 が 1 行目の #Requires -Version 7 を読み取れる。5.1 は構文解析のエラーより
                 先に #Requires を確かめ、ScriptRequiresUnmatchedPSVersion で理由を出して止まる

    どちらでもないファイル（broken）を名指しして終了コード 1 で終わる。

    走らせる側で確かめ方が変わる:

      pwsh 7（手元・番犬 crates/tako-control/tests/powershell51_watchdog.rs）
          5.1 の読み方を写す。BOM があれば UTF-8 で、無ければ CP1252 と CP932 の両方で復号して
          構文解析する
      Windows PowerShell 5.1（CI の windows ジョブ）
          5.1 の実物で読む。stops の側は一時 dir へ写した複製を powershell.exe -File で起こし、
          理由を出して止まるところまで見る

    どちらへ倒すか（利用者が自分の手で走らせうるものは BOM で readable、開発・CI・リリースだけが
    走らせるものは #Requires で stops）とファイルごとの判断は .agent/conventions.md の #2329 の節。
    このファイル自身も 5.1 で走るので、5.1 に無い書き方（?? や三項演算子など）は使わない。

.PARAMETER Path
    確かめる .ps1 のパス。省略時はリポジトリ全体から集める（生成物・依存物の dir は外す）。

.EXAMPLE
    pwsh -File scripts/windows/check-powershell51.ps1
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/windows/check-powershell51.ps1
#>
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Path = @()
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

# リポジトリのルート（このファイルは scripts/windows/ にある）
$TakoRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent (Split-Path -Parent $PSScriptRoot)))
# 番犬の走査と同じ（crates/tako-control/tests/powershell51_watchdog.rs の SKIP_DIRS）
$TakoSkipDirs = @('target', '.git', 'node_modules', 'dist', '.wrangler')
$TakoUtf8 = New-Object System.Text.UTF8Encoding $false

$TakoIsDesktop = $PSVersionTable.PSVersion.Major -lt 6
if (-not $TakoIsDesktop) {
    # .NET（pwsh 7）は CP932 を既定で持たないので足す。.NET Framework（5.1）は最初から持つ
    [System.Text.Encoding]::RegisterProvider([System.Text.CodePagesEncodingProvider]::Instance)
}
try {
    # 日本語の案内を CI のログで読めるようにする（5.1 の既定は OEM コードページ）
    [Console]::OutputEncoding = $TakoUtf8
} catch {
    # コンソールの無いホストでは設定できない。行の頭の機械向けの欄は ASCII なので困らない
}

function Get-TakoPs1Files([string]$Dir) {
    foreach ($item in @(Get-ChildItem -LiteralPath $Dir -Force)) {
        if ($item.PSIsContainer) {
            if ($TakoSkipDirs -cnotcontains $item.Name) { Get-TakoPs1Files $item.FullName }
        } elseif ($item.Extension -eq '.ps1' -or $item.Extension -eq '.psm1') {
            $item.FullName
        }
    }
}

# 表示用のパス（ルートの下ならルートからの相対・区切りは /）
function Get-TakoDisplayPath([string]$Full) {
    $prefix = $TakoRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if ($Full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $Full = $Full.Substring($prefix.Length)
    }
    $Full.Replace('\', '/')
}

function Test-TakoSameText([string]$A, [string]$B) {
    [string]::Equals($A, $B, [System.StringComparison]::Ordinal)
}

# #Requires -Version の主版数（無ければ 0）
function Get-TakoRequiredMajor($Ast) {
    $req = $Ast.ScriptRequirements
    if ($null -eq $req) { return 0 }
    if ($null -eq $req.RequiredPSVersion) { return 0 }
    $req.RequiredPSVersion.Major
}

function New-TakoVerdict([string]$Verdict, [string]$Detail) {
    [pscustomobject]@{ Verdict = $Verdict; Detail = $Detail }
}

# pwsh 7 で 5.1 の読み方を写す
function Test-TakoEmulated([string]$Full) {
    $bytes = [System.IO.File]::ReadAllBytes($Full)
    $hasBom = ($bytes.Length -ge 3) -and ($bytes[0] -eq 0xEF) -and ($bytes[1] -eq 0xBB) -and ($bytes[2] -eq 0xBF)
    $start = 0
    if ($hasBom) { $start = 3 }
    $utf8 = $TakoUtf8.GetString($bytes, $start, $bytes.Length - $start)

    $views = New-Object System.Collections.Generic.List[object]
    if ($hasBom) {
        $views.Add([pscustomobject]@{ Name = 'bom'; Text = $utf8 })
    } else {
        foreach ($cp in 1252, 932) {
            $views.Add([pscustomobject]@{ Name = "cp$cp"; Text = [System.Text.Encoding]::GetEncoding($cp).GetString($bytes) })
        }
    }

    $parts = New-Object System.Collections.Generic.List[string]
    $stops = 0
    $clean = 0
    foreach ($v in $views) {
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseInput($v.Text, [ref]$tokens, [ref]$errors)
        $major = Get-TakoRequiredMajor $ast
        $same = Test-TakoSameText $v.Text $utf8
        $part = '{0}:errors={1}' -f $v.Name, @($errors).Count
        if ($major -gt 0) { $part += ',requires=' + $major }
        if (-not $same) { $part += ',text-differs' }
        $parts.Add($part)
        if ($major -ge 6) { $stops++ }
        if (($major -lt 6) -and (@($errors).Count -eq 0) -and $same) { $clean++ }
    }
    $detail = $parts -join ' '
    if ($stops -eq $views.Count) { return New-TakoVerdict 'stops' $detail }
    if ($clean -eq $views.Count) { return New-TakoVerdict 'readable' $detail }
    New-TakoVerdict 'broken' $detail
}

# 5.1 の実物で読む。#Requires で止まる側は複製を起こして確かめる
function Test-TakoReal([string]$Full) {
    $bytes = [System.IO.File]::ReadAllBytes($Full)
    $hasBom = ($bytes.Length -ge 3) -and ($bytes[0] -eq 0xEF) -and ($bytes[1] -eq 0xBB) -and ($bytes[2] -eq 0xBF)
    $start = 0
    if ($hasBom) { $start = 3 }
    $utf8 = $TakoUtf8.GetString($bytes, $start, $bytes.Length - $start)

    # スクリプトとして起こすときと同じ読み方（BOM が無ければ ANSI）で読んだ中身を構文解析する
    # （ExternalScriptInfo.ScriptContents は実行時に読む中身そのもの）
    $info = Get-Command -Name $Full -CommandType ExternalScript
    $text = $info.ScriptContents
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
    $major = Get-TakoRequiredMajor $ast
    $same = Test-TakoSameText $text $utf8
    $detail = 'enc={0} errors={1}' -f $info.OriginalEncoding.WebName, @($errors).Count
    if ($major -gt 0) { $detail += ' requires=' + $major }
    if (-not $same) { $detail += ' text-differs' }

    if ($major -lt 6) {
        if ((@($errors).Count -eq 0) -and $same) { return New-TakoVerdict 'readable' $detail }
        return New-TakoVerdict 'broken' $detail
    }

    # 一時 dir の複製を起こす（#Requires が効かず走り出しても、リポジトリの外なので何も組まない）
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ('tako-ps51-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir | Out-Null
    try {
        $copy = Join-Path $dir (Split-Path -Leaf $Full)
        Copy-Item -LiteralPath $Full -Destination $copy
        $out = Join-Path $dir 'stdout.txt'
        $err = Join-Path $dir 'stderr.txt'
        $exe = Join-Path $PSHOME 'powershell.exe'
        $argv = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $copy))
        $p = Start-Process -FilePath $exe -ArgumentList $argv -RedirectStandardOutput $out -RedirectStandardError $err -NoNewWindow -PassThru
        # 終了コードを取れるよう、終わる前にハンドルを掴んでおく（Start-Process -PassThru の既知の罠）
        $null = $p.Handle
        if (-not $p.WaitForExit(120000)) {
            $p.Kill()
            return New-TakoVerdict 'broken' ($detail + ' timeout')
        }
        $said = ((Get-Content -LiteralPath $err -Raw -ErrorAction SilentlyContinue) + ' ' + (Get-Content -LiteralPath $out -Raw -ErrorAction SilentlyContinue))
        $said = ($said -replace '\s+', ' ').Trim()
        $detail += ' exit=' + $p.ExitCode
        # 5.1 の既定の表示（NormalView）は FullyQualifiedErrorId まで出す。出なくても文面で拾う
        if (($p.ExitCode -ne 0) -and ($said -match 'ScriptRequiresUnmatchedPSVersion|"#requires"')) {
            # 理由の 1 文目をログへ残す（ScriptRequiresException の文面）
            $reason = $said
            if ($reason.Length -gt 200) { $reason = $reason.Substring(0, 200) }
            return New-TakoVerdict 'stops' ($detail + ' said: ' + $reason)
        }
        $said200 = $said
        if ($said200.Length -gt 200) { $said200 = $said200.Substring(0, 200) }
        return New-TakoVerdict 'broken' ($detail + ' said: ' + $said200)
    } finally {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$mode = 'emulate'
if ($TakoIsDesktop) { $mode = 'real' }
Write-Output ('PS51 HOST {0} {1} {2}' -f $PSVersionTable.PSVersion, $PSVersionTable.PSEdition, $mode)

$targets = @()
if ($Path.Count -gt 0) {
    foreach ($p in $Path) { $targets += [System.IO.Path]::GetFullPath($p) }
} else {
    $targets = @(Get-TakoPs1Files $TakoRoot | Sort-Object)
}
if ($targets.Count -eq 0) {
    Write-Output 'PS51 SUMMARY files=0'
    Write-Output '確かめる .ps1 が 1 本も無い（走査の起点を確かめる）'
    exit 1
}

$counts = @{ readable = 0; stops = 0; broken = 0 }
$broken = New-Object System.Collections.Generic.List[string]
foreach ($full in $targets) {
    if ($TakoIsDesktop) { $r = Test-TakoReal $full } else { $r = Test-TakoEmulated $full }
    $shown = Get-TakoDisplayPath $full
    $counts[$r.Verdict]++
    if ($r.Verdict -eq 'broken') { $broken.Add($shown) }
    Write-Output ('PS51 {0} {1} {2}' -f $r.Verdict, $shown, $r.Detail)
}
Write-Output ('PS51 SUMMARY files={0} readable={1} stops={2} broken={3}' -f $targets.Count, $counts.readable, $counts.stops, $counts.broken)
if ($broken.Count -gt 0) {
    Write-Output ('Windows PowerShell 5.1 で読めず、理由を出して止まりもしない .ps1 がある: {0}' -f ($broken -join ', '))
    Write-Output '利用者が自分の手で走らせうるものは UTF-8 with BOM で保存し、開発・CI・リリースだけが走らせるものは'
    Write-Output '1 行目を #Requires -Version 7.0 にする（.agent/conventions.md の #2329 の節）'
    exit 1
}
exit 0
