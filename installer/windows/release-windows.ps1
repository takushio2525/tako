# tako:run: pwsh -File ${file}
<#
.SYNOPSIS
    tako の Windows 配布物をこの PC でビルドし、GitHub Release へ添付する。

.DESCRIPTION
    Windows の CI / リリースは GitHub Actions ではなく Windows 実機で回す（#587）。
    このスクリプトが「ビルド → 検査 → 添付」の全部を 1 コマンドで通す:

      1. 前検査（タグ・Release の実在、作業ツリー、HEAD、バージョン整合）
      2. build-installer.ps1 でインストーラーとポータブル zip を作る
      3. 配布物のスモーク検査（名前・サイズ・zip の中身・埋め込み FileVersion）
      4. gh release upload でアセットを添付する

    **既定は dry-run**。実際にアップロードするのは -Upload を明示したときだけ。

    macOS 側（scripts/release.sh と launchd の夜間ジョブ）とは独立に動く。
    同じタグに両 OS のアセットが並ぶ形で、Release を作るのは先に走った方。

    添付するアセット名は crates/tako-core/src/platform/release_assets.rs が正
    （#594 / #595）。ここは PowerShell 側の写し installer/windows/lib/release-assets.ps1
    を経由して組む:
      tako-<tag>-windows-x86_64.exe / tako-<tag>-windows-x86_64.zip

.PARAMETER Tag
    リリースタグ（例: v0.6.0 / v0.6.0-rc1）。省略時は Cargo.toml の
    workspace.package.version から `v<version>` を組み立てる。

.PARAMETER Upload
    実際に gh release upload を実行する。付けない限りアップロードはしない。

.PARAMETER CreateRelease
    対象タグの Release がまだ無いとき、prerelease として新規作成する。
    付けないと「Release が無い」で止まる（タグ違いの取り違えを防ぐため）。

.PARAMETER Force
    git の状態に関する前検査（作業ツリーが dirty / HEAD がタグと違う）を警告へ downgrade する。
    **バージョン整合と配布物の検査は Force でも緩まない**。バージョンを詐称した
    配布物をアップロードできてしまうのが一番まずいため。

.PARAMETER SkipBuild
    cargo build を省略して target/release の既存バイナリを包み直す。
    検査やアップロードだけをやり直したいときに使う。

.PARAMETER IsccPath
    ISCC.exe のパス。省略時は build-installer.ps1 が PATH と既定の導入先から探す。

.EXAMPLE
    pwsh -File installer/windows/release-windows.ps1
    pwsh -File installer/windows/release-windows.ps1 -Tag v0.6.0
    pwsh -File installer/windows/release-windows.ps1 -Tag v0.6.0 -Upload
    pwsh -File installer/windows/release-windows.ps1 -Tag v0.6.0 -Upload -CreateRelease
#>
[CmdletBinding()]
param(
    [string]$Tag,
    [switch]$Upload,
    [switch]$CreateRelease,
    [switch]$Force,
    [switch]$SkipBuild,
    [string]$IsccPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# 外部コマンドの非ゼロ終了は $LASTEXITCODE で明示的に見る（gh の「無い」は例外にしたくない）
$PSNativeCommandUseErrorActionPreference = $false

# アセット命名規則の写し（正は crates/tako-core/src/platform/release_assets.rs）
. (Join-Path $PSScriptRoot 'lib/release-assets.ps1')

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$outDir = Join-Path $repoRoot 'dist/windows'
# 配布物が壊れて（切り詰められて）いないことの下限。実測は installer 約 15MB / zip 約 20MB
$minAssetBytes = 5MB

$problems = [System.Collections.Generic.List[string]]::new()
function Add-Problem([string]$message) { $problems.Add($message) }
function Add-Relaxable([string]$message) {
    if ($Force) { Write-Warning "$message（-Force のため続行）" } else { Add-Problem $message }
}

function Get-CargoVersion {
    $cargoToml = Join-Path $repoRoot 'Cargo.toml'
    $inWorkspacePackage = $false
    foreach ($line in Get-Content -LiteralPath $cargoToml) {
        if ($line -match '^\s*\[') { $inWorkspacePackage = $line -match '^\s*\[workspace\.package\]' ; continue }
        if ($inWorkspacePackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    throw "Cargo.toml から workspace.package.version を読めなかった"
}

# タグ形式（v0.6.0 / v0.6.0-rc1）から数値部分だけを取り出す。
# 埋め込みリソースの FileVersion は数値しか持てないのでこちらと突き合わせる
function Get-NumericVersion([string]$version) { (($version -replace '^v', '') -split '-', 2)[0] }

# FileVersion は "0.5.12" とも "0.5.12.0" とも読め、さらに空白詰めで返ることがある
# （Inno Setup が作る setup exe が実際にそう: "0.5.12              "）。
# 空白を落として 4 桁へ揃えてから比べる
function Get-NormalizedVersion([string]$version) {
    $parts = @(($version -replace '\s', '') -split '\.')
    while ($parts.Count -lt 4) { $parts += '0' }
    ($parts[0..3] -join '.')
}
function Test-SameVersion([string]$a, [string]$b) {
    (Get-NormalizedVersion $a) -eq (Get-NormalizedVersion $b)
}

# --- タグの決定 ----------------------------------------------------------------

$cargoVersion = Get-CargoVersion
if (-not $Tag) {
    $Tag = "v$cargoVersion"
    Write-Host "-- タグ未指定。Cargo.toml から $Tag を使う"
}
if ($Tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$') {
    throw "タグの形式が不正: $Tag（期待: v0.6.0 / v0.6.0-rc1）"
}
$tagVersion = $Tag -replace '^v', ''
$numericVersion = Get-NumericVersion $Tag

$mode = if ($Upload) { 'UPLOAD' } else { 'DRY RUN（-Upload 未指定。アップロードはしない）' }
Write-Host ''
Write-Host "== tako Windows リリース =="
Write-Host "   タグ     : $Tag"
Write-Host "   モード   : $mode"
Write-Host ''

# --- 前検査 --------------------------------------------------------------------

Write-Host '-- 前検査'

if (-not (Get-Command 'gh' -ErrorAction SilentlyContinue)) {
    Add-Problem 'gh（GitHub CLI）が見つからない。winget install --id GitHub.cli で導入する'
} else {
    gh auth status 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Add-Problem 'gh が未認証。gh auth login を実行する' }
}

# バージョン整合。ここが食い違うと exe に埋まる FileVersion がタグと別物になる。
# -Force でも緩めない（緩めても後段の配布物検査で必ず落ちる）
if ($cargoVersion -ne $tagVersion) {
    Add-Problem @"
Cargo.toml のバージョンがタグと違う: Cargo.toml=$cargoVersion / タグ=$tagVersion
exe に埋まる FileVersion は Cargo.toml が正なので、このまま作るとタグと違う版数を名乗る
配布物ができる。タグの commit を取り出してから実行する:
  git fetch --tags && git checkout $Tag
"@
}

git rev-parse --verify --quiet "refs/tags/$Tag^{commit}" 2>&1 | Out-Null
$localTagExists = ($LASTEXITCODE -eq 0)
$remoteTagLine = git ls-remote --tags origin "refs/tags/$Tag" 2>$null
$remoteTagExists = -not [string]::IsNullOrWhiteSpace($remoteTagLine)

if (-not $localTagExists) {
    if ($remoteTagExists) {
        Add-Problem "タグ $Tag がローカルに無い（リモートにはある）。git fetch --tags で取り込む"
    } else {
        Add-Problem "タグ $Tag が存在しない。先に macOS 側の scripts/release.sh でタグを打つ"
    }
} else {
    $tagCommit = (git rev-parse "refs/tags/$Tag^{commit}").Trim()
    $headCommit = (git rev-parse HEAD).Trim()
    if ($tagCommit -ne $headCommit) {
        Add-Relaxable "HEAD がタグ $Tag の commit ではない（HEAD=$($headCommit.Substring(0,8)) / タグ=$($tagCommit.Substring(0,8))）。git checkout $Tag してから実行する"
    }
}

$dirty = @(git status --porcelain | Where-Object { $_.Trim() })
if ($dirty.Count -gt 0) {
    Add-Relaxable "作業ツリーが clean でない（$($dirty.Count) 件）。git status を確認して commit / stash する"
}

$releaseExists = $false
if (Get-Command 'gh' -ErrorAction SilentlyContinue) {
    gh release view $Tag --json tagName 2>&1 | Out-Null
    $releaseExists = ($LASTEXITCODE -eq 0)
}
if ($releaseExists) {
    Write-Host "   Release $Tag は既にある。アセットを追加する"
} elseif ($CreateRelease) {
    Write-Host "   Release $Tag は無い。-CreateRelease のため prerelease として作る"
    if (-not $remoteTagExists) {
        # gh release create はリモートにタグが無いと既定ブランチの先頭でタグを勝手に作る
        Add-Problem "タグ $Tag がリモートに無い。この状態で Release を作ると gh が既定ブランチにタグを作ってしまう。先に git push origin $Tag する"
    }
} else {
    Add-Problem "Release $Tag が無い。macOS 側の scripts/release.sh が先に作るのを待つか、-CreateRelease を付けて prerelease として作る"
}

if ($problems.Count -gt 0) {
    Write-Host ''
    Write-Host '== 前検査に失敗 ==' -ForegroundColor Red
    foreach ($p in $problems) { Write-Host "   [NG] $p" -ForegroundColor Red }
    exit 1
}
Write-Host '   [OK] 前検査を通過'

# --- ビルド --------------------------------------------------------------------

# 名前は直書きせず命名規則の写しから組む（#595: リリース側と更新チェック側の食い違い防止）
$setupName = Get-TakoAssetName -Tag $Tag -Platform 'windows' -Arch $TakoAssetArchWindows
$zipName = Get-TakoAssetName -Tag $Tag -Platform 'windows' -Arch $TakoAssetArchWindows -Ext 'zip'
$setupExe = Join-Path $outDir $setupName
$zipPath = Join-Path $outDir $zipName

# 前回の生成物が残っていると「作られた」の検査が意味を失うので先に消す
foreach ($stale in @($setupExe, $zipPath)) {
    if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Force }
}

Write-Host ''
Write-Host '-- 配布物をビルド（build-installer.ps1）'
$buildArgs = @{ Version = $Tag; OutDir = $outDir }
if ($SkipBuild) { $buildArgs['SkipBuild'] = $true }
if ($IsccPath) { $buildArgs['IsccPath'] = $IsccPath }
& (Join-Path $PSScriptRoot 'build-installer.ps1') @buildArgs

# --- 配布物のスモーク検査 --------------------------------------------------------
# ここは -Force でも緩めない。壊れた / 版数を詐称した配布物を上げないための最後の関門

Write-Host ''
Write-Host '-- 配布物を検査'
$assets = @($setupExe, $zipPath)

foreach ($asset in $assets) {
    $name = Split-Path -Leaf $asset
    if (-not (Test-Path -LiteralPath $asset)) { throw "生成されていない: $name" }
    $size = (Get-Item -LiteralPath $asset).Length
    if ($size -lt $minAssetBytes) {
        throw "$name が小さすぎる（$size bytes < $minAssetBytes bytes）。ビルドが途中で壊れている可能性がある"
    }
    Write-Host ("   [OK] {0,-40} {1,12:N0} bytes" -f $name, $size)
}

# インストーラー自身の版数（.iss の VersionInfoVersion 由来）
$setupVersion = Get-NormalizedVersion (Get-Item -LiteralPath $setupExe).VersionInfo.FileVersion
if (-not (Test-SameVersion $setupVersion $numericVersion)) {
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
        $exeVersion = Get-NormalizedVersion (Get-Item -LiteralPath $p).VersionInfo.FileVersion
        if (-not (Test-SameVersion $exeVersion $numericVersion)) {
            throw "zip 内 $exe の FileVersion がタグと違う: $exeVersion（期待 $numericVersion）。Windows ホストでビルドしたか確認する"
        }
        Write-Host "   [OK] zip 内 $exe の FileVersion = $exeVersion"
    }
} finally {
    Remove-Item -LiteralPath $inspect -Recurse -Force -ErrorAction SilentlyContinue
}

# --- アップロード ---------------------------------------------------------------

Write-Host ''
if (-not $Upload) {
    Write-Host '== DRY RUN 完了。ここから先は実行していない ==' -ForegroundColor Yellow
    if (-not $releaseExists) { Write-Host "   （未実行）gh release create $Tag --title $Tag --prerelease --generate-notes" }
    Write-Host "   （未実行）gh release upload $Tag ""$setupExe"" ""$zipPath"" --clobber"
    Write-Host ''
    Write-Host '   実際にアップロードするには -Upload を付けて実行する:'
    $rerun = "     pwsh -File installer/windows/release-windows.ps1 -Tag $Tag -Upload"
    if (-not $releaseExists) { $rerun += ' -CreateRelease' }
    if ($SkipBuild) { $rerun += ' -SkipBuild' }
    Write-Host $rerun
    exit 0
}

if (-not $releaseExists) {
    Write-Host "-- Release $Tag を prerelease として作成"
    gh release create $Tag --title $Tag --prerelease --generate-notes
    if ($LASTEXITCODE -ne 0) { throw "gh release create に失敗 (exit $LASTEXITCODE)" }
}

Write-Host "-- Release $Tag へアセットを添付"
gh release upload $Tag $setupExe $zipPath --clobber
if ($LASTEXITCODE -ne 0) { throw "gh release upload に失敗 (exit $LASTEXITCODE)" }

Write-Host ''
Write-Host '== 完了 =='
$uploaded = gh release view $Tag --json assets --jq '.assets[] | "   " + .name + "  " + .url'
if ($LASTEXITCODE -eq 0) { $uploaded | ForEach-Object { Write-Host $_ } }
