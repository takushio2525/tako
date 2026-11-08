#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

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

    インストーラーがインストール先へ置く中身と、ユーザー環境変数 Path の足し引き・アンインストールは、
    実際にインストールしないと分からないので Test-TakoInstalledPayload が別に見る（入れる → 中身 →
    Path → 起動 → アンインストール → Path が外れる の一巡。#1845 / #2767）。こちらは CI の使い捨て
    ランナー専用（実機で走らせると実インストールを上書きし、アンインストールまでする）。

    命名規則の正は crates/tako-core/src/platform/release_assets.rs。
    ここは PowerShell 側の写し lib/release-assets.ps1 を経由して組む。
#>

Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot 'release-assets.ps1')
# ユーザー環境変数 Path の読み戻し（`tako setup` の通しの検査と同じ 1 実装。#2767）
. (Join-Path $PSScriptRoot 'user-path.ps1')

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

# インストール先 Dir を指す per-user のアンインストール情報（HKCU\...\Uninstall\<AppId>_is1）。無ければ $null。
# AppId は tako.iss の 1 か所にだけ書く（ここへ写さない）ので、InstallLocation で引く
function Get-TakoUninstallEntry([string]$Dir) {
    $root = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Uninstall')
    if ($null -eq $root) { return $null }
    try {
        $want = ConvertTo-TakoPathKey $Dir
        foreach ($name in $root.GetSubKeyNames()) {
            $sub = $root.OpenSubKey($name)
            if ($null -eq $sub) { continue }
            try {
                $location = [string]$sub.GetValue('InstallLocation', '')
                if ($location -and (ConvertTo-TakoPathKey $location) -eq $want) {
                    return [PSCustomObject]@{
                        Key             = $name
                        DisplayName     = [string]$sub.GetValue('DisplayName', '')
                        UninstallString = [string]$sub.GetValue('UninstallString', '')
                    }
                }
            } finally {
                $sub.Dispose()
            }
        }
        $null
    } finally {
        $root.Dispose()
    }
}

# 出力の 1 行目（`tako 0.8.40`）から版の数値部分を取る。読めなければ $null
function Get-TakoVersionFromOutput([string]$Text) {
    $first = @($Text -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | Select-Object -First 1)
    if ($first.Count -eq 0 -or $first[0] -notmatch '^tako\s+v?([0-9][0-9A-Za-z.+-]*)\s*$') { return $null }
    Get-TakoNumericVersion $Matches[1]
}

<#
.SYNOPSIS
    インストーラーを使い捨ての置き場で一巡させて検査する（Issue #1845 / #2767）。

.DESCRIPTION
    tako.iss の [Files] が実際にインストール先へ何を置くか・[Code] がユーザー環境変数 Path を
    足して外すかは、インストーラーを動かさないと確かめられない（Inno Setup の出力は中身を一覧できる
    形式ではない）。そこで CI の windows ランナー（使い捨て）で 1 回だけ次を通す:

      1. /VERYSILENT のインストール（Path への追加のタスクを選ぶ・デスクトップのショートカットは選ばない）
      2. インストール先の中身: exe 2 本と同梱表（Get-TakoLicenseBundle）の全部（#1845）
      3. ユーザー環境変数 Path にインストール先がちょうど 1 回入り、検査前の要素は同じ順で残る（#2767）
      4. インストール先の tako.exe --version が起動し、インストーラーの FileVersion と同じ版を返す。
         新しく開いたシェル（システム + ユーザーの Path を読み直した cmd）からも `tako` が引けて動く
      5. アンインストーラー（アンインストール情報の UninstallString = unins000.exe）を無人で走らせ、
         Path からインストール先が外れて検査前と同じ並びに戻り、exe とアンインストール情報が消える

    ユーザー環境変数 Path は最初に取った値へ必ず戻す（途中で失敗しても）。

    **実機では呼ばない**（release-windows.ps1 からも呼んでいない）。同じ AppId の
    実インストールの登録（HKCU のアンインストール情報）を上書きし、アンインストールまでするため。
    GitHub Actions の外では throw する。
#>
function Test-TakoInstalledPayload {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Setup)

    if ($env:GITHUB_ACTIONS -ne 'true') {
        throw 'Test-TakoInstalledPayload は GitHub Actions の使い捨てランナー専用（実機では実インストールを上書きし、アンインストールまでするので呼ばない）'
    }
    $work = Join-Path ([System.IO.Path]::GetTempPath()) "tako-install-check-$([System.IO.Path]::GetRandomFileName())"
    $dir = Join-Path $work 'tako'
    $log = Join-Path $work 'setup.log'
    $uninstallLog = Join-Path $work 'uninstall.log'
    New-Item -ItemType Directory -Path $work -Force | Out-Null
    $before = Get-TakoUserPath
    if ((Get-TakoPathEntryCount -Entries $before.Entries -Dir $dir) -ne 0) {
        throw "検査前からユーザー環境変数 Path にインストール先 $dir が入っている"
    }
    $savedPath = $env:PATH
    try {
        # 1. 入れる。Path への追加は選ぶ（#2767。戻すのは finally）・デスクトップのショートカットは選ばない。
        # 何も動いていないはずだが、/NOCLOSEAPPLICATIONS で他のプロセスを閉じにいかせない
        $arguments = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /NOICONS /NOCLOSEAPPLICATIONS ' +
            "/MERGETASKS=`"addtopath,!desktopicon`" /DIR=`"$dir`" /LOG=`"$log`""
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

        # 2. 中身
        Write-TakoPayloadListing -Dir $dir -Label 'インストール先'
        foreach ($exe in 'tako-app.exe', 'tako.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $dir $exe) -PathType Leaf)) {
                throw "インストール先に $exe が置かれていない"
            }
        }
        Test-TakoLicenseBundle -Dir $dir -Label 'インストール先'

        # 3. ユーザー環境変数 Path（tako.iss の EnvAddPath。ペインの外の `tako` はこれに依存する）
        $installed = Get-TakoUserPath
        $count = Get-TakoPathEntryCount -Entries $installed.Entries -Dir $dir
        if ($count -ne 1) {
            throw "インストール後のユーザー環境変数 Path にインストール先 $dir が $count 回入っている（期待 1 回。tako.iss の addtopath / EnvAddPath）。いまの値: $($installed.Raw)"
        }
        $others = @($installed.Entries | Where-Object { (Get-TakoPathEntryCount -Entries @($_) -Dir $dir) -eq 0 })
        $diff = Compare-TakoPathEntries -Expected $before.Entries -Actual $others
        if ($null -ne $diff) { throw "インストールでインストール先以外のユーザー環境変数 Path が変わった: $diff" }
        Write-Host "   [OK] ユーザー環境変数 Path にインストール先が 1 回入った（他の要素 $($before.Entries.Count) 個はそのまま・$($installed.Kind)）"

        # 4. 起動。版はインストーラー自身の FileVersion（.iss の VersionInfoVersion）と突き合わせる
        $setupVersion = Get-TakoNormalizedVersion (Get-Item -LiteralPath $Setup).VersionInfo.FileVersion
        $cli = Join-Path $dir 'tako.exe'
        $out = (& $cli --version 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "インストール先の tako.exe --version が終了コード $LASTEXITCODE で失敗した: $out" }
        $got = Get-TakoVersionFromOutput $out
        if (-not $got -or -not (Test-TakoSameVersion $got $setupVersion)) {
            throw "インストール先の tako.exe --version の版がインストーラーと違う: $($out.Trim())（インストーラーの FileVersion $setupVersion）"
        }
        Write-Host "   [OK] インストール先の tako.exe --version = $($out.Trim())"
        # 新しく開いたシェル = システム + ユーザーの Path を読み直した cmd（いまのプロセスの PATH には届かない）
        $env:PATH = Get-TakoFreshLoginPath
        $where = (& $env:ComSpec /d /c where tako 2>&1 | Out-String)
        $hits = @($where -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | ForEach-Object { ConvertTo-TakoPathKey $_ })
        if ($hits -notcontains (ConvertTo-TakoPathKey $cli)) {
            throw "新しく開いたシェルから tako が $cli に解決されない（ユーザー環境変数 Path の追加が効いていない）: $($where.Trim())"
        }
        $fresh = (& $env:ComSpec /d /c tako --version 2>&1 | Out-String)
        $freshExit = $LASTEXITCODE
        $env:PATH = $savedPath
        if ($freshExit -ne 0 -or -not (Get-TakoVersionFromOutput $fresh)) {
            throw "新しく開いたシェルから tako --version が動かない（exit $freshExit）: $($fresh.Trim())"
        }
        Write-Host "   [OK] 新しく開いたシェルから tako が引けて動く（$($fresh.Trim())）"

        # 5. アンインストール。アンインストーラーは自分の写しを一時 dir で起こし直して元は先に抜ける。
        # 写しはファイルとアンインストール情報を消した**後**で [Code] の usPostUninstall（EnvRemovePath）を
        # 走らせるので、「消えた」だけを待つと Path を読むのが早すぎる（初回の CI で実際に当たった）。
        # 4 つ（exe・アンインストーラー・アンインストール情報・Path の要素）がすべて消えるまで待ち、
        # 期限を過ぎたら残ったものを名指す
        $entry = Get-TakoUninstallEntry $dir
        if ($null -eq $entry) { throw "インストール先 $dir のアンインストール情報（HKCU の Uninstall）が無い" }
        $uninstaller = $entry.UninstallString.Trim().Trim('"')
        if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) { throw "アンインストーラーが無い: $($entry.UninstallString)" }
        $proc = Start-Process -FilePath $uninstaller -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$uninstallLog`"" -PassThru
        $null = $proc.Handle
        if (-not $proc.WaitForExit(300000)) {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            throw 'アンインストーラーが 5 分で終わらなかった'
        }
        if ($proc.ExitCode -ne 0) {
            if (Test-Path -LiteralPath $uninstallLog) { Get-Content -LiteralPath $uninstallLog | ForEach-Object { Write-Host $_ } }
            throw "アンインストーラーが終了コード $($proc.ExitCode) で失敗した"
        }
        $deadline = (Get-Date).AddSeconds(120)
        while ($true) {
            $removed = Get-TakoUserPath
            $left = [ordered]@{
                'tako.exe'                       = (Test-Path -LiteralPath $cli)
                'アンインストーラー'             = (Test-Path -LiteralPath $uninstaller)
                'アンインストール情報'           = ($null -ne (Get-TakoUninstallEntry $dir))
                'ユーザー環境変数 Path の要素'   = ((Get-TakoPathEntryCount -Entries $removed.Entries -Dir $dir) -ne 0)
            }
            $remaining = @($left.Keys | Where-Object { $left[$_] })
            if ($remaining.Count -eq 0) { break }
            if ((Get-Date) -gt $deadline) {
                if (Test-Path -LiteralPath $uninstallLog) { Get-Content -LiteralPath $uninstallLog | ForEach-Object { Write-Host $_ } }
                if ($left['ユーザー環境変数 Path の要素'] -and $remaining.Count -eq 1) {
                    throw "アンインストールが終わって 120 秒たってもユーザー環境変数 Path にインストール先 $dir が残っている（tako.iss の EnvRemovePath）。いまの値: $($removed.Raw)"
                }
                throw "アンインストールが 120 秒で終わらなかった（残っているもの: $($remaining -join ' / ')）"
            }
            Start-Sleep -Milliseconds 500
        }
        Write-Host '   [OK] アンインストールで exe・アンインストーラー・アンインストール情報・Path の要素が消えた'
        $diff = Compare-TakoPathEntries -Expected $before.Entries -Actual $removed.Entries
        if ($null -ne $diff) { throw "アンインストール後のユーザー環境変数 Path が検査前と違う: $diff" }
        Write-Host "   [OK] アンインストールでユーザー環境変数 Path からインストール先が外れ、検査前の並びに戻った"
    } finally {
        $env:PATH = $savedPath
        Restore-TakoUserPath -Snapshot $before
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
