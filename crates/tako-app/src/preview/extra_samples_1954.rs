//! #1954 の検査用の本文（単体 `syntax_resolution_tests` と visual-test `extra-syntax` が同じものを開く）。
//! two-face に構文が無かった PowerShell の 3 つの拡張子（スクリプト・モジュール・マニフェスト）。
//! 授業・Windows の自動化でよく見る書き方（コメントのヘルプ・param・class / enum・ヒアドキュメント・
//! 正規表現の `-match`・ハッシュテーブル・スプラッティング・ヒアドキュメントの中の C#）を入れてある

/// スクリプト（`.ps1`）
pub(crate) const PS_SCRIPT: &str = r#"#Requires -Version 5.1
using namespace System.Collections.Generic

<#
.SYNOPSIS
    ビルド成果物を集めて zip にまとめる（tako の色分けの見本）
.PARAMETER Path
    集める元のフォルダ
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateNotNullOrEmpty()]
    [string] $Path,

    [int] $Retry = 3,
    [switch] $Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

enum Level {
    Info = 0
    Warn = 1
    Error = 2
}

class Logger {
    [string] $Name
    hidden [List[string]] $Lines = [List[string]]::new()

    Logger([string] $name) {
        $this.Name = $name
    }

    [void] Write([Level] $level, [string] $message) {
        $stamp = Get-Date -Format 'HH:mm:ss'
        $this.Lines.Add("[$stamp] [$level] $($this.Name): $message")
    }
}

function Get-Artifact {
    [OutputType([System.IO.FileInfo[]])]
    param(
        [string] $Root,
        [string[]] $Include = @('*.exe', '*.dll')
    )
    process {
        Get-ChildItem -Path $Root -Recurse -File -Include $Include |
            Where-Object { $_.Length -gt 1kb -and $_.Name -notmatch '^~\$' } |
            Sort-Object -Property LastWriteTime -Descending
    }
}

$log = [Logger]::new('pack')
$items = Get-Artifact -Root $Path
$total = ($items | Measure-Object -Property Length -Sum).Sum
$log.Write([Level]::Info, "files: $($items.Count) / bytes: $total")

$settings = @{
    Destination = Join-Path $env:TEMP 'artifacts.zip'
    Force       = $Force.IsPresent
    Level       = 0x1F
}

for ($i = 1; $i -le $Retry; $i++) {
    try {
        Compress-Archive -Path $items.FullName @settings
        break
    }
    catch [System.IO.IOException] {
        $log.Write([Level]::Warn, "retry $i : $($_.Exception.Message)")
        Start-Sleep -Milliseconds (200 * $i)
    }
    finally {
        Write-Verbose "attempt $i done"
    }
}

switch -Regex ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64|x64' { $arch = 'x64' }
    'ARM64'     { $arch = 'arm64' }
    default     { $arch = 'unknown' }
}

$report = @"
Name : $($log.Name)
Arch : $arch
Lines: $($log.Lines.Count)
"@


if ($report -match 'Arch\s*:\s*(?<arch>\w+)') {
    Write-Host "arch = $($Matches['arch'])" -ForegroundColor Green
}
elseif ($null -eq $arch) {
    throw [System.InvalidOperationException]::new('no arch')
}

$squares = 1..10 | ForEach-Object { $_ * $_ } | Where-Object { $_ % 2 -eq 0 }
$log.Lines | Out-File -FilePath "$PSScriptRoot\pack.log" -Encoding utf8
exit 0
"#;

/// モジュール（`.psm1`）
pub(crate) const PS_MODULE: &str = r#"Set-StrictMode -Version Latest

function ConvertTo-Slug {
    [CmdletBinding()]
    param([Parameter(ValueFromPipeline)] [string] $Text)
    process {
        ($Text.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-')
    }
}

function Get-Greeting([string] $Name = 'world') {
    "Hello, $Name! ($(Get-Date -Format yyyy))"
}

Export-ModuleMember -Function ConvertTo-Slug, Get-Greeting
"#;

/// マニフェスト（`.psd1` = ハッシュテーブルの式）
pub(crate) const PS_MANIFEST: &str = r#"@{
    RootModule        = 'Slug.psm1'
    ModuleVersion     = '1.2.0'
    GUID              = '00000000-0000-4000-8000-000000000000'
    Author            = 'tako'
    PowerShellVersion = '5.1'
    FunctionsToExport = @('ConvertTo-Slug', 'Get-Greeting')
    PrivateData       = @{
        PSData = @{ Tags = @('text', 'slug') }
    }
}
"#;

/// ヒアドキュメントの中の C#（`Add-Type -TypeDefinition`）。two-face の C# へ結んだ参照を通る
/// （上流の定義は Sublime Text 4 の `extends` で書いた別ファイルを名指すので、build.rs が
/// two-face の C# へ向け直している）
pub(crate) const PS_CSHARP_HEREDOC: &str = r#"$csharp = @'
using System;
public static class Hasher {
    public static string Hex(byte[] data) {
        return BitConverter.ToString(data).Replace("-", "");
    }
}
'@
Add-Type -TypeDefinition $csharp -Language CSharp
"#;
