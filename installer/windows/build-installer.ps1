<#
.SYNOPSIS
    tako の Windows 配布物（インストーラー + ポータブル zip）を作る。

.DESCRIPTION
    1. アイコン（assets/icon/tako.ico）が無ければ make-icon.ps1 で生成する
    2. cargo build --release で tako-app.exe / tako.exe を作る
    3. ISCC で installer/windows/tako.iss をコンパイルする
    4. ポータブル zip を作る

    出力（dist/windows/、.gitignore 済み）:
      tako-<tag>-windows-x86_64.exe   インストーラー（主形式）
      tako-<tag>-windows-x86_64.zip   ポータブル版

    アセット名の命名規則の正は crates/tako-core/src/platform/release_assets.rs
    （#594 / #595）で、ここは PowerShell 側の写し
    installer/windows/lib/release-assets.ps1 を経由して組み立てる。
    名前を直書きすると更新チェック側（update_checker）と食い違い、
    「自 OS 向けアセットを掴めない / 掴めない更新を通知する」事故になる。

    配布物の作り方をここ 1 本に集約するため、リリース手順
    （installer/windows/release-windows.ps1）はこのスクリプトを呼ぶ。

.PARAMETER Version
    配布物の名前に入るバージョン。省略時は Cargo.toml の workspace.package.version。
    リリース時はタグをそのまま渡す（例: v0.6.0 → tako-v0.6.0-windows-x86_64.exe）。
    先頭に `v` が無ければアセット名を組む時点で補う（命名規則はタグ表記が正）。

.PARAMETER IsccPath
    ISCC.exe のパス。省略時は PATH → 既定のインストール先の順に探す。

.PARAMETER OutDir
    配布物の出力先。省略時は dist/windows。リリース以外の配布物（テスター向けプレビュー等）を
    リリース用の成果物と混ぜたくないときに分ける。

.PARAMETER BinDir
    同梱する tako-app.exe / tako.exe の置き場。省略時は target/release。
    --target 付きでビルドした場合や、別の場所に用意した成果物を包む場合に指定する。
    -SkipBuild と併用するのが主な用途だが、$env:CARGO_TARGET_DIR を別の場所へ向けて
    ビルドさせるときは -SkipBuild なしでも要る（cargo の出力先が target/release では
    なくなるため。worktree でリリースビルドを回すときがこれに当たる）。

.EXAMPLE
    pwsh -File installer/windows/build-installer.ps1
    pwsh -File installer/windows/build-installer.ps1 -Version v0.6.0
    pwsh -File installer/windows/build-installer.ps1 -SkipBuild -BinDir target/x86_64-pc-windows-msvc/release

    # worktree でテスター向けプレビューを作る（target を共有し、出力先を分ける）
    $env:CARGO_TARGET_DIR = 'C:\path\to\shared-target'
    pwsh -File installer/windows/build-installer.ps1 -Version v0.5.13-win.1 `
      -BinDir "$env:CARGO_TARGET_DIR\release" -OutDir C:\path\to\dist\v0.5.13-win.1
#>
[CmdletBinding()]
param(
    [string]$Version,
    [string]$IsccPath,
    [string]$OutDir,
    [string]$BinDir,
    [switch]$SkipBuild,
    [switch]$SkipZip
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# アセット命名規則の写し（正は crates/tako-core/src/platform/release_assets.rs）
. (Join-Path $PSScriptRoot 'lib/release-assets.ps1')

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$iconPath = Join-Path $repoRoot 'assets/icon/tako.ico'
$issPath = Join-Path $PSScriptRoot 'tako.iss'
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'dist/windows' }
if (-not $BinDir) { $BinDir = Join-Path $repoRoot 'target/release' }

function Resolve-Version {
    $cargoToml = Join-Path $repoRoot 'Cargo.toml'
    $inWorkspacePackage = $false
    foreach ($line in Get-Content -LiteralPath $cargoToml) {
        if ($line -match '^\s*\[') { $inWorkspacePackage = $line -match '^\s*\[workspace\.package\]' ; continue }
        if ($inWorkspacePackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    throw "Cargo.toml から workspace.package.version を読めなかった"
}

function Resolve-Iscc {
    if ($IsccPath) {
        if (-not (Test-Path -LiteralPath $IsccPath)) { throw "ISCC が見つからない: $IsccPath" }
        return $IsccPath
    }
    $cmd = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    )
    foreach ($c in $candidates) { if ($c -and (Test-Path -LiteralPath $c)) { return $c } }
    throw @"
ISCC.exe（Inno Setup コンパイラ）が見つからない。次のどれかで導入する:
  winget install --id JRSoftware.InnoSetup --scope user
  choco install innosetup
または -IsccPath で明示する。
"@
}

if (-not $Version) { $Version = Resolve-Version }

# アセット名に入るのはタグ表記（先頭 v あり）。-Version に 0.7.4 が来ても v0.7.4 へ寄せる
$assetTag = ConvertTo-TakoTag -Version $Version
$assetBase = Get-TakoAssetBaseName -Tag $assetTag -Platform 'windows' -Arch $TakoAssetArchWindows
$setupName = Get-TakoAssetName -Tag $assetTag -Platform 'windows' -Arch $TakoAssetArchWindows
$zipName = Get-TakoAssetName -Tag $assetTag -Platform 'windows' -Arch $TakoAssetArchWindows -Ext 'zip'

Write-Host "== tako Windows 配布物ビルド (version=$Version / tag=$assetTag) =="

if (-not (Test-Path -LiteralPath $iconPath)) {
    Write-Host '-- アイコンを生成'
    # .ps1 の呼び出しは $LASTEXITCODE を更新しないので、失敗は例外（ErrorActionPreference=Stop）と
    # 生成物の実在で見る
    & (Join-Path $PSScriptRoot 'make-icon.ps1')
    if (-not (Test-Path -LiteralPath $iconPath)) { throw "アイコン生成に失敗: $iconPath が作られなかった" }
}

if (-not $SkipBuild) {
    # -win.N タグからビルド番号を抽出し、update_checker が自バイナリの版数を正確に
    # 埋め込めるようにする（#723）。build.rs が TAKO_WIN_NUM を TAKO_FULL_VERSION へ反映
    if ($Version -match '-win\.(\d+)') {
        $env:TAKO_WIN_NUM = $Matches[1]
        Write-Host "-- TAKO_WIN_NUM=$($env:TAKO_WIN_NUM)（$Version から抽出）"
    } else {
        $env:TAKO_WIN_NUM = $null
    }
    Write-Host '-- cargo build --release -p tako-app -p tako-cli'
    Push-Location $repoRoot
    try {
        & cargo build --release -p tako-app -p tako-cli
        if ($LASTEXITCODE -ne 0) { throw "cargo build に失敗 (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
        $env:TAKO_WIN_NUM = $null
    }
}

# 2 本が揃っていることを確認する。tako.exe が欠けたまま配ると
# ペイン内の `tako` も dispatch の resolve_tako_binary() も動かない
foreach ($exe in 'tako-app.exe', 'tako.exe') {
    $p = Join-Path $binDir $exe
    if (-not (Test-Path -LiteralPath $p)) { throw "ビルド成果物が無い: $p" }
    Write-Host ("   {0,-14} {1,10:N0} bytes" -f $exe, (Get-Item -LiteralPath $p).Length)
}

if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }

$iscc = Resolve-Iscc
Write-Host "-- ISCC: $iscc"
# BinDir は .iss 側の既定（リポジトリ相対の target/release）を上書きするため絶対パスで渡す
$binDirFull = (Resolve-Path -LiteralPath $binDir).ProviderPath
& $iscc "/DAppVersion=$Version" "/DAssetBaseName=$assetBase" "/DBinDir=$binDirFull" "/O$OutDir" $issPath
if ($LASTEXITCODE -ne 0) { throw "ISCC に失敗 (exit $LASTEXITCODE)" }

$setupExe = Join-Path $OutDir $setupName
if (-not (Test-Path -LiteralPath $setupExe)) { throw "インストーラーが生成されていない: $setupExe" }

if (-not $SkipZip) {
    Write-Host '-- ポータブル zip'
    $zipPath = Join-Path $OutDir $zipName
    $staging = Join-Path ([System.IO.Path]::GetTempPath()) "tako-portable-$([System.IO.Path]::GetRandomFileName())"
    $payload = Join-Path $staging 'tako'
    New-Item -ItemType Directory -Path $payload -Force | Out-Null
    try {
        # tako-app.exe と tako.exe は必ず同じ階層に置く
        Copy-Item -LiteralPath (Join-Path $binDir 'tako-app.exe') -Destination $payload
        Copy-Item -LiteralPath (Join-Path $binDir 'tako.exe') -Destination $payload
        Copy-Item -LiteralPath $iconPath -Destination $payload
        Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $payload 'LICENSE.txt')
        if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
        Compress-Archive -Path $payload -DestinationPath $zipPath -CompressionLevel Optimal
    } finally {
        Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ''
Write-Host '== 完成 =='
# 部分一致ではなく「今回作った名前」で絞る（過去世代の配布物を混ぜて報告しない）
Get-ChildItem -LiteralPath $OutDir -File |
    Where-Object { $_.Name -eq $setupName -or $_.Name -eq $zipName } |
    ForEach-Object { "   {0,-40} {1,12:N0} bytes" -f $_.Name, $_.Length }
