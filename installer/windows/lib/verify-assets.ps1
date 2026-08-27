<#
.SYNOPSIS
    Windows 配布物のスモーク検査（#587 / #965）。

.DESCRIPTION
    「壊れた / 版数を詐称した配布物を Release へ上げない」ための最後の関門。
    実機リリース（release-windows.ps1）と CI（.github/workflows/release-windows.yml）の
    **両方から同じ 1 実装を呼ぶ**。片方だけが検査する形にすると、生成場所によって
    通る基準が変わってしまう。

    検査するもの:
      1. インストーラーとポータブル zip が命名規則どおりの名前で存在する
      2. どちらも下限サイズを超えている（途中で切れた配布物を弾く）
      3. インストーラーの FileVersion がタグの数値部分と一致する
      4. zip を展開でき、中の tako-app.exe / tako.exe の FileVersion も一致する
         （= Windows ホストでビルドされている。クロスビルドではリソースが埋まらない）

    命名規則の正は crates/tako-core/src/platform/release_assets.rs。
    ここは PowerShell 側の写し lib/release-assets.ps1 を経由して組む。
#>

Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot 'release-assets.ps1')

# 配布物が壊れて（切り詰められて）いないことの下限。実測は installer 約 17MB / zip 約 22MB
$TakoMinAssetBytes = 5MB

# タグ形式（v0.6.0 / v0.6.0-rc1 / v0.7.9-win.1）から数値部分だけを取り出す。
# 埋め込みリソースの FileVersion は数値しか持てないのでこちらと突き合わせる
function Get-TakoNumericVersion([string]$version) { (($version -replace '^v', '') -split '-', 2)[0] }

# FileVersion は "0.5.12" とも "0.5.12.0" とも読め、さらに空白詰めで返ることがある
# （Inno Setup が作る setup exe が実際にそう: "0.5.12              "）。
# 空白を落として 4 桁へ揃えてから比べる
function Get-TakoNormalizedVersion([string]$version) {
    $parts = @(($version -replace '\s', '') -split '\.')
    while ($parts.Count -lt 4) { $parts += '0' }
    ($parts[0..3] -join '.')
}

function Test-TakoSameVersion([string]$a, [string]$b) {
    (Get-TakoNormalizedVersion $a) -eq (Get-TakoNormalizedVersion $b)
}

<#
.SYNOPSIS
    OutDir の配布物を検査し、検査したファイルのパスを返す。問題があれば throw する。
#>
function Test-TakoWindowsAssets {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$OutDir
    )

    $numericVersion = Get-TakoNumericVersion $Tag
    $setupName = Get-TakoAssetName -Tag $Tag -Platform 'windows' -Arch $TakoAssetArchWindows
    $zipName = Get-TakoAssetName -Tag $Tag -Platform 'windows' -Arch $TakoAssetArchWindows -Ext 'zip'
    $setupExe = Join-Path $OutDir $setupName
    $zipPath = Join-Path $OutDir $zipName

    foreach ($asset in @($setupExe, $zipPath)) {
        $name = Split-Path -Leaf $asset
        if (-not (Test-Path -LiteralPath $asset)) { throw "生成されていない: $name" }
        $size = (Get-Item -LiteralPath $asset).Length
        if ($size -lt $TakoMinAssetBytes) {
            throw "$name が小さすぎる（$size bytes < $TakoMinAssetBytes bytes）。ビルドが途中で壊れている可能性がある"
        }
        Write-Host ("   [OK] {0,-40} {1,12:N0} bytes" -f $name, $size)
    }

    # インストーラー自身の版数（.iss の VersionInfoVersion 由来）
    $setupVersion = Get-TakoNormalizedVersion (Get-Item -LiteralPath $setupExe).VersionInfo.FileVersion
    if (-not (Test-TakoSameVersion $setupVersion $numericVersion)) {
        throw "インストーラーの FileVersion がタグと違う: $setupVersion（期待 $numericVersion）"
    }
    Write-Host "   [OK] インストーラーの FileVersion = $setupVersion"

    # zip を展開して、実際に配る exe の中身を見る（zip が開けることの確認も兼ねる）
    $inspect = Join-Path ([System.IO.Path]::GetTempPath()) "tako-release-check-$([System.IO.Path]::GetRandomFileName())"
    try {
        Expand-Archive -LiteralPath $zipPath -DestinationPath $inspect -Force
        foreach ($exe in 'tako-app.exe', 'tako.exe') {
            # zip の中は tako/ 直下（build-installer.ps1 の staging 構成）
            $p = Join-Path $inspect (Join-Path 'tako' $exe)
            if (-not (Test-Path -LiteralPath $p)) { throw "zip に $exe が入っていない" }
            $exeVersion = Get-TakoNormalizedVersion (Get-Item -LiteralPath $p).VersionInfo.FileVersion
            if (-not (Test-TakoSameVersion $exeVersion $numericVersion)) {
                throw "zip 内 $exe の FileVersion がタグと違う: $exeVersion（期待 $numericVersion）。Windows ホストでビルドしたか確認する"
            }
            Write-Host "   [OK] zip 内 $exe の FileVersion = $exeVersion"
        }
    } finally {
        Remove-Item -LiteralPath $inspect -Recurse -Force -ErrorAction SilentlyContinue
    }

    [PSCustomObject]@{ Setup = $setupExe; Zip = $zipPath }
}
