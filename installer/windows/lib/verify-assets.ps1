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
      5. zip にライセンス本文と第三者の告知（同梱表 scripts/lib/license-bundle.txt の全部）が入り、
         リポジトリの元ファイルとバイト一致する（Issue #1845 / #2305）

    インストーラーがインストール先へ置く中身は、実際にインストールしないと分からないので
    Test-TakoInstalledPayload が別に見る。こちらは CI の使い捨てランナー専用
    （実機で走らせると実インストールを上書きする）。

    命名規則の正は crates/tako-core/src/platform/release_assets.rs。
    ここは PowerShell 側の写し lib/release-assets.ps1 を経由して組む。
#>

Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot 'release-assets.ps1')

# 配布物が壊れて（切り詰められて）いないことの下限。実測は installer 約 17MB / zip 約 22MB
$TakoMinAssetBytes = 5MB

# リポジトリのルート（このファイルは installer/windows/lib/ にある）
$TakoRepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))

# 配布物へ同梱するライセンス本文と第三者の告知の表（Issue #1845 / #2305。リポジトリのルートからの相対パス）。
# **正本はこの表の 1 か所**で、ここに名前を並べ直さない（2 か所目の表を作らない）。同じ表を
# crates/tako-control/tests/license_bundle_watchdog.rs（組み立ての全箇所 = build-installer.ps1 の zip・
# tako.iss の [Files]・macOS の scripts/build-app.sh との突き合わせ）と scripts/lib/license-bundle.sh
# （release.sh --check-assets の公開後の検査）が読む
$TakoLicenseBundleTable = 'scripts/lib/license-bundle.txt'

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
    同梱表 $TakoLicenseBundleTable を読み、1 本ずつ Source（リポジトリ直下の元ファイル）/
    WindowsName（配布物の中の名前。LICENSE はメモ帳で開けるよう .txt を付ける）/ Line（表の行）を返す。

.DESCRIPTION
    表が無い・1 本も無い・「元ファイル Windows での名前」の 2 列でない行があれば、表の file:line を
    名指しして throw する（空の表のまま検査を素通りさせない）。行の読み方は
    scripts/lib/license-bundle.sh の tako_license_bundle と同じ（# で始まる行と空行は飛ばす）。
    PowerShell 5.1 でも動く書き方にする（?? や三項演算子を使わない）
#>
function Get-TakoLicenseBundle {
    $table = Join-Path $TakoRepoRoot $TakoLicenseBundleTable
    if (-not (Test-Path -LiteralPath $table -PathType Leaf)) {
        throw "ライセンスの同梱表 $TakoLicenseBundleTable が無い（$table）"
    }
    $entries = @()
    $n = 0
    foreach ($raw in @(Get-Content -LiteralPath $table -Encoding UTF8)) {
        $n++
        $t = ([string]$raw).Trim()
        if ($t -eq '' -or $t.StartsWith('#')) { continue }
        $cols = @($t -split '\s+')
        if ($cols.Count -ne 2) {
            throw "${TakoLicenseBundleTable}:${n}: 「元ファイル Windows での名前」の 2 列になっていない: $t"
        }
        $entries += [PSCustomObject]@{ Source = $cols[0]; WindowsName = $cols[1]; Line = $n }
    }
    if ($entries.Count -eq 0) {
        throw "ライセンスの同梱表 $TakoLicenseBundleTable に 1 本も無い（$table）"
    }
    $entries
}

<#
.SYNOPSIS
    展開した配布物の置き場 Dir に同梱表（Get-TakoLicenseBundle）の全部が入り、リポジトリの元ファイルと
    バイト一致することを確かめる。欠け・空・食い違いは throw する。Label は案内に出す名前。
#>
function Test-TakoLicenseBundle {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Dir,
        [Parameter(Mandatory = $true)][string]$Label
    )

    foreach ($entry in @(Get-TakoLicenseBundle)) {
        $at = "${TakoLicenseBundleTable}:$($entry.Line)"
        $src = Join-Path $TakoRepoRoot $entry.Source
        if (-not (Test-Path -LiteralPath $src -PathType Leaf)) {
            throw "リポジトリに $($entry.Source) が無く $Label の $($entry.WindowsName) と照合できない（$at の行）"
        }
        $p = Join-Path $Dir $entry.WindowsName
        if (-not (Test-Path -LiteralPath $p -PathType Leaf)) {
            throw "$Label に $($entry.WindowsName) が入っていない（リポジトリの $($entry.Source) を同梱すること。$at の行。Issue #1845）"
        }
        $size = (Get-Item -LiteralPath $p).Length
        if ($size -le 0) { throw "$Label の $($entry.WindowsName) が空" }
        $got = (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash
        $want = (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash
        if ($got -ne $want) {
            throw "$Label の $($entry.WindowsName) がリポジトリの $($entry.Source) と一致しない（SHA-256 $got / 期待 $want）"
        }
        Write-Host ("   [OK] {0} の {1,-24} {2,10:N0} bytes（リポジトリの {3} と一致）" -f $Label, $entry.WindowsName, $size, $entry.Source)
    }
}

# 置き場の中身を 1 ファイル 1 行で出す（CI のログに生成物の一覧を残す）
function Write-TakoPayloadListing([string]$Dir, [string]$Label) {
    Write-Host "   $Label の中身:"
    $root = (Resolve-Path -LiteralPath $Dir).ProviderPath
    Get-ChildItem -LiteralPath $root -Recurse -File | Sort-Object FullName | ForEach-Object {
        Write-Host ("     {0,-32} {1,12:N0} bytes" -f [System.IO.Path]::GetRelativePath($root, $_.FullName), $_.Length)
    }
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
        Write-TakoPayloadListing -Dir (Join-Path $inspect 'tako') -Label 'zip の tako/'
        Test-TakoLicenseBundle -Dir (Join-Path $inspect 'tako') -Label 'zip'
    } finally {
        Remove-Item -LiteralPath $inspect -Recurse -Force -ErrorAction SilentlyContinue
    }

    [PSCustomObject]@{ Setup = $setupExe; Zip = $zipPath }
}

<#
.SYNOPSIS
    インストーラーを使い捨ての置き場へ無人インストールし、置かれたファイルを検査する（Issue #1845）。

.DESCRIPTION
    tako.iss の [Files] が実際にインストール先へ何を置くかは、インストーラーを動かさないと
    確かめられない（Inno Setup の出力は中身を一覧できる形式ではない）。そこで CI の windows
    ランナー（使い捨て）で /VERYSILENT のインストールを 1 回走らせ、exe 2 本と
    同梱表（Get-TakoLicenseBundle）の全部が置かれたことを見る。

    **実機では呼ばない**（release-windows.ps1 からも呼んでいない）。同じ AppId の
    実インストールの登録（HKCU のアンインストール情報）を上書きするため。
    GitHub Actions の外では throw する。
#>
function Test-TakoInstalledPayload {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Setup)

    if ($env:GITHUB_ACTIONS -ne 'true') {
        throw 'Test-TakoInstalledPayload は GitHub Actions の使い捨てランナー専用（実機では実インストールを上書きするので呼ばない）'
    }
    $work = Join-Path ([System.IO.Path]::GetTempPath()) "tako-install-check-$([System.IO.Path]::GetRandomFileName())"
    $dir = Join-Path $work 'tako'
    $log = Join-Path $work 'setup.log'
    New-Item -ItemType Directory -Path $work -Force | Out-Null
    try {
        # PATH への追加とデスクトップのショートカットは選ばない（ランナーの環境を触らない）。
        # 何も動いていないはずだが、/NOCLOSEAPPLICATIONS で他のプロセスを閉じにいかせない
        $arguments = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /NOICONS /NOCLOSEAPPLICATIONS ' +
            "/MERGETASKS=`"!addtopath,!desktopicon`" /DIR=`"$dir`" /LOG=`"$log`""
        $proc = Start-Process -FilePath $Setup -ArgumentList $arguments -PassThru
        # 終了後に ExitCode を読めるよう、ハンドルを先に掴んでおく
        $null = $proc.Handle
        if (-not $proc.WaitForExit(300000)) {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            throw "インストーラーが 5 分で終わらなかった: $(Split-Path -Leaf $Setup)"
        }
        if ($proc.ExitCode -ne 0) {
            if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log | ForEach-Object { Write-Host $_ } }
            throw "インストーラーが終了コード $($proc.ExitCode) で失敗した"
        }
        Write-Host "   [OK] 無人インストールが終了コード 0 で終わった"
        Write-TakoPayloadListing -Dir $dir -Label 'インストール先'
        foreach ($exe in 'tako-app.exe', 'tako.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $dir $exe) -PathType Leaf)) {
                throw "インストール先に $exe が置かれていない"
            }
        }
        Test-TakoLicenseBundle -Dir $dir -Label 'インストール先'
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
