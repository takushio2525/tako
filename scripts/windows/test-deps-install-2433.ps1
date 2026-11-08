#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Windows の任意依存（psmux / Git）の導入代行を、製品の口（tako setup deps install）で確かめる（Issue #2433）。

.DESCRIPTION
    #2433 で Windows の `tako setup deps install`（= MCP `tako_setup_deps`・設定画面のボタン）が winget で
    依存を入れるようにした（それまでは winget のコマンドを案内するだけ）。依存ごとの権限は依存表の 1 か所で宣言し、
    利用者の権限で入る psmux はそのまま、UAC が出うる Git は利用者の同意（CLI は 1 回の確認か --yes）が
    あるときだけ起こす。Windows の実機は当面使えないので、使い捨ての windows-latest ランナーで回す。

    -Mode real（外部の winget の取得元から本物を取る。関連パスの PR・main への push・定期・手動実行だけ）:
      1. 導入の前の status で psmux が未導入・can_run = true・privilege = user
      2. install --dep psmux が winget を起こして終了コード 0・installed に psmux
      3. 導入の後の status で psmux が導入済み（見つかったパスを記録）・psmux -V に答える
      4. 2 回目の install は already_installed で何もしない（冪等）
      5. Git: 一度 winget で外してから、同意なし（--yes なし・端末なし）の install は起こさず needs_consent・
         consent_required = [git] で返り、git は未導入のまま。--yes の install で winget が Git を入れ、
         status が導入済みへ変わる（ランナーは管理者で UAC の確認が出ない設定なので、確認の画面そのものは出ない）
      後片付け: psmux を外す・Git が元の置き場（%ProgramFiles%\Git\cmd）に無ければ machine で入れ直す・
      ユーザー環境変数 Path（winget が WinGet\Links を足す）を検査前の値へ戻して確かめる

    -Mode fake（外部に依らない。毎回）: 偽の winget（winget.cmd。FAKE_WINGET_EXIT の終了コードで抜け、
    呼ばれたら FAKE_WINGET_MARKER へ引数を書く）を砂場の PATH の先頭に置き、
      - 同意なしの Git は偽の winget を**起こさず** needs_consent で返る（UAC 系を起こさない）
      - 終了コードごとの断り: 0x800704C7 / 0x8A15010C = UAC で取り消し、0x80072EFD = 取得元に届かない、
        0x8A150002 = winget が古い、0x8A150006 = 途中で失敗、0 なのに見つからない = 入れたのに引けない
      - winget が無い: 案内（アプリ インストーラー）つきで installer_missing
      - A/B（TAKO_2433_LEGACY=1）: 案内だけ（not_delegable）で偽の winget を起こさない
      - 起こす引数: psmux は --scope user --silent、Git は --scope も --silent も付けない

    断りの文は CLI の枠（日本語）で見る。英語の文は単体テスト（setup_deps::tests の issue2433_*）が
    同じ関数（winget_outcome / DepSkip::text_in）へ終了コードを注入して確かめる。

    失敗は段の名前つきの `::error::` 1 行で名指しし、終了コード 1 で終わる。結果の行（TAKO2433_*）を CI のログへ残す。

.PARAMETER Mode
    real（本物の winget）/ fake（偽の winget）。

.PARAMETER Tako
    検査する tako.exe。省略時はリポジトリの target\debug\tako.exe（CI の `cargo build --workspace` の成果物）。

.EXAMPLE
    pwsh -File scripts/windows/test-deps-install-2433.ps1 -Mode fake
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('real', 'fake')][string]$Mode,
    [string]$Tako
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) { throw 'このスクリプトは Windows 専用（winget で依存を入れ、ユーザー環境変数 Path を書き換えて戻す）' }
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'GitHub Actions の使い捨てランナー専用（psmux / Git を入れたり外したりし、ユーザー環境変数 Path を書き換えて戻す。実機では走らせない）'
}

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $RepoRoot 'installer/windows/lib/user-path.ps1')

$script:Stage = '準備'

function Fail([string]$Message) {
    throw "[#2433 $Mode $script:Stage] $Message"
}

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { Fail $Message }
}

if (-not $Tako) { $Tako = Join-Path $RepoRoot 'target\debug\tako.exe' }
if (-not (Test-Path -LiteralPath $Tako -PathType Leaf)) { throw "tako.exe が無い: $Tako（先に cargo build -p tako-cli）" }
$TakoExe = (Resolve-Path -LiteralPath $Tako).ProviderPath

$RunRoot = Join-Path ([System.IO.Path]::GetTempPath()) "tako-2433-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Path $RunRoot -Force | Out-Null

# 子の env。real は今の env（TAKO_* だけ外す）、fake は砂場（$Overrides で上書き）
function New-ChildEnv([hashtable]$Overrides) {
    $vars = @{}
    foreach ($entry in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
        $vars[[string]$entry.Key] = [string]$entry.Value
    }
    foreach ($k in @($vars.Keys)) {
        if ($k -like 'TAKO_*' -or $k -like 'FAKE_WINGET_*' -or $k -in @('PSModulePath')) { $vars.Remove($k) }
    }
    # 判定は日本語の字面で見る（表示言語の既定が OS の言語に従うので固定する。#2365）
    $vars['TAKO_LANG'] = 'ja'
    if ($Overrides) {
        foreach ($k in $Overrides.Keys) {
            # Windows の環境変数は大小文字を区別しないので、同じ名前の別表記を先に消す（PATH / Path）
            foreach ($existing in @($vars.Keys)) {
                if ($existing -ieq $k) { $vars.Remove($existing) }
            }
            $vars[$k] = [string]$Overrides[$k]
        }
    }
    $vars
}

# 子を起こして待つ。stdin は閉じる（端末が無い = CLI は確認を出さない）。stdout / stderr は UTF-8 で読む
function Invoke-Child {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [hashtable]$Overrides,
        [int]$TimeoutSec = 900
    )
    $psi = [System.Diagnostics.ProcessStartInfo]::new($FilePath)
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.WorkingDirectory = $RunRoot
    $psi.Environment.Clear()
    $vars = New-ChildEnv $Overrides
    foreach ($k in $vars.Keys) { $psi.Environment[$k] = $vars[$k] }

    $started = Get-Date
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        $p.StandardInput.Close()
        if (-not $p.WaitForExit($TimeoutSec * 1000)) {
            $p.Kill($true)
            Fail "$(Split-Path -Leaf $FilePath) $($Arguments -join ' ') が $TimeoutSec 秒で終わらなかった"
        }
        $p.WaitForExit()
        $result = [PSCustomObject]@{
            Code    = $p.ExitCode
            Out     = $out.Result
            Err     = $err.Result
            Seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 1)
        }
    } finally {
        $p.Dispose()
    }
    Write-Host ("   > {0} {1}  (exit {2}, {3} 秒)" -f (Split-Path -Leaf $FilePath), ($Arguments -join ' '), $result.Code, $result.Seconds)
    $result
}

function Invoke-Tako([string[]]$Arguments, [hashtable]$Overrides, [int]$TimeoutSec = 900) {
    Invoke-Child -FilePath $TakoExe -Arguments $Arguments -Overrides $Overrides -TimeoutSec $TimeoutSec
}

function Show-Result($Result, [string]$Label) {
    Write-Host "   --- $Label の stdout ---"
    $Result.Out -split "`r?`n" | ForEach-Object { Write-Host "   | $_" }
    Write-Host "   --- $Label の stderr ---"
    $Result.Err -split "`r?`n" | ForEach-Object { Write-Host "   | $_" }
}

function Assert-Exit0($Result, [string]$Label) {
    if ($Result.Code -ne 0) {
        Show-Result $Result $Label
        Fail "$Label が終了コード $($Result.Code) で終わった（期待 0）"
    }
}

function Assert-Refused($Result, [string]$Label, [string[]]$Needles) {
    $text = $Result.Out + "`n" + $Result.Err
    if ($Result.Code -eq 0) {
        Show-Result $Result $Label
        Fail "$Label が終了コード 0 で終わった（断るはず）"
    }
    foreach ($n in $Needles) {
        if (-not $text.Contains($n)) {
            Show-Result $Result $Label
            Fail "$Label の断りに「$n」が無い"
        }
    }
    # 断りの 1 行目を CI のログへ残す（日本語の文そのもの）
    $first = @($Result.Err -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | Select-Object -First 1)
    Write-Host ("TAKO2433_REFUSAL label={0} exit={1} | {2}" -f $Label, $Result.Code, ($first -join ''))
}

function ConvertFrom-TakoJson($Result, [string]$Label) {
    try {
        $Result.Out | ConvertFrom-Json -Depth 32
    } catch {
        Show-Result $Result $Label
        Fail "$Label の stdout が JSON として読めない: $($_.Exception.Message)"
    }
}

# status の 1 件を読んで 1 行残す
function Get-DepStatus([string]$Bin, [string]$When, [hashtable]$Overrides) {
    $r = Invoke-Tako @('setup', 'deps', '--json') $Overrides 180
    Assert-Exit0 $r "status（$When）"
    $j = ConvertFrom-TakoJson $r "status（$When）"
    $dep = @($j.deps | Where-Object { $_.bin -eq $Bin })
    Assert-That ($dep.Count -eq 1) "status に $Bin の行が無い: $($r.Out)"
    $d = $dep[0]
    $found = if ($null -eq $d.found) { 'null' } else { [string]$d.found }
    Write-Host ("TAKO2433_STATUS mode={0} bin={1} when={2} found=[{3}] can_run={4} privilege={5} installer=[{6}]" -f `
            $Mode, $Bin, $When, $found, $d.can_run, $d.privilege, $d.installer_found)
    $d
}

function Get-Skip($Json, [string]$Bin) {
    @($Json.skipped | Where-Object { $_.bin -eq $Bin }) | Select-Object -First 1
}

# ---------------------------------------------------------------- 偽の winget
function Test-Fake {
    $sys = $env:SystemRoot
    $fakeDir = Join-Path $RunRoot 'fake-winget'
    $pf = Join-Path $RunRoot 'pf'
    $home2433 = Join-Path $RunRoot 'home'
    foreach ($d in @($fakeDir, $pf, (Join-Path $home2433 'AppData\Roaming'), (Join-Path $home2433 'AppData\Local'))) {
        New-Item -ItemType Directory -Path $d -Force | Out-Null
    }
    $marker = Join-Path $RunRoot 'winget-called.txt'
    # 呼ばれたら引数を印へ書き、指定の終了コードで抜ける（負の値も 32 ビットのまま返る）
    Set-Content -LiteralPath (Join-Path $fakeDir 'winget.cmd') -Encoding ascii -Value @(
        '@echo off',
        'echo fake-winget %*',
        '>>"%FAKE_WINGET_MARKER%" echo %*',
        'exit /b %FAKE_WINGET_EXIT%'
    )
    $sysPath = @((Join-Path $sys 'System32'), $sys, (Join-Path $sys 'System32\Wbem'), (Join-Path $sys 'System32\WindowsPowerShell\v1.0')) -join ';'
    # 砂場: git / psmux が見えない（PATH・%ProgramFiles%\Git\cmd・WinGet\Links・レジストリの PATH の読み直しを塞ぐ）
    $base = @{
        PATH                 = "$fakeDir;$sysPath"
        ProgramFiles         = $pf
        USERPROFILE          = $home2433
        APPDATA              = (Join-Path $home2433 'AppData\Roaming')
        LOCALAPPDATA         = (Join-Path $home2433 'AppData\Local')
        TAKO_2428_LEGACY     = '1'
        FAKE_WINGET_MARKER   = $marker
        FAKE_WINGET_EXIT     = '0'
    }
    function With([hashtable]$Extra) {
        $h = $base.Clone()
        foreach ($k in $Extra.Keys) { $h[$k] = $Extra[$k] }
        $h
    }
    function Reset-Marker { Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue }
    function Get-Marker { if (Test-Path -LiteralPath $marker) { (Get-Content -LiteralPath $marker -Raw).Trim() } else { $null } }

    $script:Stage = '前提（砂場で git / psmux が見えない・偽の winget を見る）'
    foreach ($bin in @('git', 'psmux')) {
        $d = Get-DepStatus $bin 'fake-before' $base
        Assert-That ($null -eq $d.found) "砂場で $bin が見えている: $($d.found)"
        Assert-That ([string]$d.installer_found -like "$fakeDir*") "偽の winget を見ていない: $($d.installer_found)"
        Assert-That ($d.can_run -eq $true) "$bin の can_run が true でない（#2433 は Windows の winget も代行する）"
    }
    $git = Get-DepStatus 'git' 'fake-before' $base
    Assert-That ($git.privilege -eq 'elevated') "git の privilege が elevated でない: $($git.privilege)"
    $psmux = Get-DepStatus 'psmux' 'fake-before' $base
    Assert-That ($psmux.privilege -eq 'user') "psmux の privilege が user でない: $($psmux.privilege)"

    $script:Stage = '同意なしの Git は winget を起こさない'
    Reset-Marker
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'git', '--json') $base
    Assert-Exit0 $r '同意なしの install --dep git'
    $j = ConvertFrom-TakoJson $r '同意なしの install --dep git'
    $skip = Get-Skip $j 'git'
    Assert-That ($null -ne $skip -and $skip.reason -eq 'needs_consent') "同意なしの Git が needs_consent で返らない: $($r.Out)"
    Assert-That (@($j.consent_required) -contains 'git') "consent_required に git が無い: $($r.Out)"
    Assert-That ($null -eq (Get-Marker)) "同意なしなのに winget が起こされた: $(Get-Marker)"
    Write-Host ("TAKO2433_CONSENT mode=fake bin=git consent=false winget_called=false reason={0} | {1}" -f $skip.reason, $skip.detail)

    $script:Stage = '同意なしの全部（psmux だけ起こし Git は残す）'
    Reset-Marker
    $r = Invoke-Tako @('setup', 'deps', 'install', '--json', '--dry-run') $base
    Assert-Exit0 $r '同意なしの install --dry-run'
    $j = ConvertFrom-TakoJson $r '同意なしの install --dry-run'
    $planned = @($j.planned | ForEach-Object { $_.bin })
    Assert-That (($planned -contains 'psmux') -and -not ($planned -contains 'git')) "計画に psmux だけが載らない: $($planned -join ',')"
    Assert-That ($null -eq (Get-Marker)) 'dry-run なのに winget が起こされた'

    $script:Stage = '起こす引数（psmux = --scope user --silent / Git = 付けない）'
    Reset-Marker
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'psmux') (With @{ FAKE_WINGET_EXIT = '0' })
    # 0 で抜けても偽物は何も置かないので「入れたのに引けない」で断る
    Assert-Refused $r '0 なのに引けない psmux' @('は正常終了しましたが psmux が見つかりません')
    $args1 = Get-Marker
    Write-Host "TAKO2433_ARGS bin=psmux | $args1"
    Assert-That ($args1 -like '*--id marlocarlo.psmux*--scope user --silent*--disable-interactivity*') "psmux の引数が違う: $args1"
    Reset-Marker
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'git', '--yes') (With @{ FAKE_WINGET_EXIT = '0' })
    Assert-Refused $r '0 なのに引けない git' @('は正常終了しましたが git が見つかりません')
    $args2 = Get-Marker
    Write-Host "TAKO2433_ARGS bin=git | $args2"
    Assert-That ($args2 -like '*--id Git.Git --exact --source winget --disable-interactivity*') "Git の引数が違う: $args2"
    Assert-That (-not ($args2 -like '*--silent*') -and -not ($args2 -like '*--scope*')) "Git に --silent / --scope が付いた: $args2"

    # 終了コードごとの断り（負の値は 32 ビットの HRESULT をそのまま int で渡す）
    $cases = @(
        @{ Name = 'UAC で取り消し（runas）'; Bin = 'git'; Code = [int]0x800704C7; Needles = @('git の導入は管理者の確認（UAC）で取り消されました') },
        @{ Name = 'UAC で取り消し（インストーラ）'; Bin = 'git'; Code = [int]0x8A15010C; Needles = @('git の導入は管理者の確認（UAC）で取り消されました') },
        @{ Name = '取得元に届かない'; Bin = 'psmux'; Code = [int]0x80072EFD; Needles = @('取得元に届きませんでした（exit 0x80072EFD）', 'fake-winget') },
        @{ Name = 'winget が古い'; Bin = 'psmux'; Code = [int]0x8A150002; Needles = @('winget が tako の渡す引数を受け付けませんでした', 'アプリ インストーラー') },
        @{ Name = '途中で失敗'; Bin = 'psmux'; Code = [int]0x8A150006; Needles = @('が失敗しました（exit 0x8A150006）') },
        @{ Name = 'winget は入っていると言うが引けない'; Bin = 'psmux'; Code = [int]0x8A15002B; Needles = @('は正常終了しましたが psmux が見つかりません') }
    )
    foreach ($c in $cases) {
        $script:Stage = "断り: $($c.Name)"
        Reset-Marker
        $extra = @('setup', 'deps', 'install', '--dep', $c.Bin)
        if ($c.Bin -eq 'git') { $extra += '--yes' }
        $r = Invoke-Tako $extra (With @{ FAKE_WINGET_EXIT = [string]$c.Code })
        Assert-Refused $r $c.Name $c.Needles
        Assert-That ($null -ne (Get-Marker)) "$($c.Name): 偽の winget が起こされていない"
    }

    $script:Stage = 'winget が無い'
    $noWinget = With @{ PATH = $sysPath }
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'psmux', '--json') $noWinget
    Assert-Exit0 $r 'winget が無い install'
    $j = ConvertFrom-TakoJson $r 'winget が無い install'
    $skip = Get-Skip $j 'psmux'
    Assert-That ($null -ne $skip -and $skip.reason -eq 'installer_missing') "winget が無いのに installer_missing で返らない: $($r.Out)"
    Assert-That ($skip.detail.Contains('winget が見つかりません') -and $skip.detail.Contains('アプリ インストーラー')) "winget が無いときの案内が違う: $($skip.detail)"
    Write-Host ("TAKO2433_REFUSAL label=winget が無い exit=0 | {0}" -f $skip.detail)

    $script:Stage = 'A/B（TAKO_2433_LEGACY=1 = 案内だけ）'
    Reset-Marker
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'psmux', '--json') (With @{ TAKO_2433_LEGACY = '1' })
    Assert-Exit0 $r '旧形の install'
    $j = ConvertFrom-TakoJson $r '旧形の install'
    $skip = Get-Skip $j 'psmux'
    Assert-That ($null -ne $skip -and $skip.reason -eq 'not_delegable') "旧形が not_delegable で返らない: $($r.Out)"
    Assert-That ($null -eq (Get-Marker)) '旧形なのに winget が起こされた'
    Write-Host ("TAKO2433_LEGACY reason={0} | {1}" -f $skip.reason, $skip.detail)
}

# ---------------------------------------------------------------- 本物の winget
function Get-GitCmd { Join-Path $env:ProgramFiles 'Git\cmd\git.exe' }

function Invoke-Winget([string[]]$Arguments, [int]$TimeoutSec = 900) {
    $w = (Get-Command winget -ErrorAction Stop).Source
    Invoke-Child -FilePath $w -Arguments $Arguments -TimeoutSec $TimeoutSec
}

function Test-Real {
    $script:Stage = 'winget'
    $v = Invoke-Winget @('--version') 60
    Write-Host ("TAKO2433_WINGET version=[{0}] path=[{1}]" -f $v.Out.Trim(), (Get-Command winget).Source)

    $script:Stage = 'psmux: 導入の前'
    $before = Get-DepStatus 'psmux' 'before' $null
    Assert-That ($null -eq $before.found) "ランナーに psmux が既に在る: $($before.found)（未導入から入れる前提が崩れた）"
    Assert-That ($before.can_run -eq $true) 'psmux の can_run が true でない（winget を見つけていない？）'
    Assert-That ($before.privilege -eq 'user') "psmux の privilege が user でない: $($before.privilege)"

    $script:Stage = 'psmux: 導入'
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'psmux', '--json')
    Assert-Exit0 $r 'install --dep psmux'
    $j = ConvertFrom-TakoJson $r 'install --dep psmux'
    $installed = @($j.installed | Where-Object { $_.bin -eq 'psmux' })
    Assert-That ($installed.Count -eq 1) "installed に psmux が無い: $($r.Out)"
    Write-Host ("TAKO2433_INSTALL mode=real bin=psmux seconds={0} path=[{1}] command=[{2}]" -f $r.Seconds, $installed[0].path, (@($j.planned)[0].command))

    $script:Stage = 'psmux: 導入の後'
    $after = Get-DepStatus 'psmux' 'after' $null
    Assert-That ($null -ne $after.found) '導入の後も status の psmux が見つからない'
    $ver = Invoke-Child -FilePath ([string]$after.found) -Arguments @('-V') -TimeoutSec 60
    Write-Host ("TAKO2433_VERSION bin=psmux exit={0} [{1}]" -f $ver.Code, (($ver.Out + ' ' + $ver.Err) -replace '\s+', ' ').Trim())

    $script:Stage = 'psmux: 2 回目（冪等）'
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'psmux', '--json')
    Assert-Exit0 $r '2 回目の install --dep psmux'
    $j = ConvertFrom-TakoJson $r '2 回目の install --dep psmux'
    $skip = Get-Skip $j 'psmux'
    Assert-That ($null -ne $skip -and $skip.reason -eq 'already_installed') "2 回目が already_installed で返らない: $($r.Out)"
    Write-Host ("TAKO2433_AGAIN bin=psmux reason={0}" -f $skip.reason)

    $script:Stage = 'Git: 導入済みなら同意を求めない'
    $g = Get-DepStatus 'git' 'before' $null
    Assert-That ($null -ne $g.found) 'ランナーに Git が無い（外して入れ直す前提が崩れた）'
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'git', '--json')
    Assert-Exit0 $r '導入済みの install --dep git'
    $j = ConvertFrom-TakoJson $r '導入済みの install --dep git'
    Assert-That ((Get-Skip $j 'git').reason -eq 'already_installed') "導入済みの Git に同意を求めた: $($r.Out)"
    Assert-That (@($j.consent_required).Count -eq 0) "導入済みなのに consent_required が空でない: $($r.Out)"

    $script:Stage = 'Git: 外す'
    $u = Invoke-Winget @('uninstall', '--id', 'Git.Git', '--exact', '--source', 'winget', '--silent', '--disable-interactivity', '--accept-source-agreements')
    Write-Host ("TAKO2433_GIT_UNINSTALL exit={0} seconds={1}" -f $u.Code, $u.Seconds)
    Assert-That ($u.Code -eq 0) "winget uninstall Git.Git が終了コード $($u.Code)"
    $g = Get-DepStatus 'git' 'removed' $null
    Assert-That ($null -eq $g.found) "外したのに git が見える: $($g.found)"
    Assert-That ($g.privilege -eq 'elevated') "git の privilege が elevated でない: $($g.privilege)"

    $script:Stage = 'Git: 同意なしは起こさない'
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'git', '--json')
    Assert-Exit0 $r '同意なしの install --dep git'
    $j = ConvertFrom-TakoJson $r '同意なしの install --dep git'
    $skip = Get-Skip $j 'git'
    Assert-That ($null -ne $skip -and $skip.reason -eq 'needs_consent') "同意なしの Git が needs_consent で返らない: $($r.Out)"
    Assert-That (@($j.consent_required) -contains 'git') "consent_required に git が無い: $($r.Out)"
    $g = Get-DepStatus 'git' 'no-consent' $null
    Assert-That ($null -eq $g.found) "同意なしなのに git が入った: $($g.found)"
    Write-Host ("TAKO2433_CONSENT mode=real bin=git consent=false installed=false | {0}" -f $skip.detail)

    $script:Stage = 'Git: --yes（同意）で入る'
    $r = Invoke-Tako @('setup', 'deps', 'install', '--dep', 'git', '--yes', '--json')
    Assert-Exit0 $r 'install --dep git --yes'
    $j = ConvertFrom-TakoJson $r 'install --dep git --yes'
    $installed = @($j.installed | Where-Object { $_.bin -eq 'git' })
    Assert-That ($installed.Count -eq 1) "installed に git が無い: $($r.Out)"
    Write-Host ("TAKO2433_INSTALL mode=real bin=git seconds={0} path=[{1}] command=[{2}]" -f $r.Seconds, $installed[0].path, (@($j.planned)[0].command))
    $g = Get-DepStatus 'git' 'consented' $null
    Assert-That ($null -ne $g.found) '同意して入れた後も status の git が見つからない'
}

$Before = Get-TakoUserPath
Write-Host "== 任意依存の導入代行（Windows・#2433・$Mode）=="
Write-Host "   tako.exe : $TakoExe"
Write-Host "   検査前のユーザー環境変数 Path: 要素 $($Before.Entries.Count) 個"

$failures = @()
try {
    if ($Mode -eq 'fake') { Test-Fake } else { Test-Real }
} catch {
    $failures += if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { [string]$_ }
} finally {
    $script:Stage = '後片付け'
    if ($Mode -eq 'real') {
        # 入れた psmux を外す（検査の前に無かったもの）
        $u = Invoke-Winget @('uninstall', '--id', 'marlocarlo.psmux', '--exact', '--source', 'winget', '--silent', '--disable-interactivity', '--accept-source-agreements')
        Write-Host ("TAKO2433_CLEANUP psmux_uninstall_exit={0}" -f $u.Code)
        # Git は元の置き場（machine）に無ければ入れ直す（後の段が PATH の git を使う）
        if (-not (Test-Path -LiteralPath (Get-GitCmd) -PathType Leaf)) {
            $i = Invoke-Winget @('install', '--id', 'Git.Git', '--exact', '--source', 'winget', '--scope', 'machine', '--silent', '--disable-interactivity', '--accept-source-agreements', '--accept-package-agreements')
            Write-Host ("TAKO2433_CLEANUP git_reinstall_machine_exit={0}" -f $i.Code)
        }
        if (-not (Test-Path -LiteralPath (Get-GitCmd) -PathType Leaf)) {
            $failures += "[#2433 後片付け] Git を元の置き場へ戻せなかった: $(Get-GitCmd)"
        }
    }
    Restore-TakoUserPath -Snapshot $Before
    $afterPath = Get-TakoUserPath
    if ($afterPath.Raw -ne $Before.Raw -or $afterPath.Exists -ne $Before.Exists) {
        $failures += "[#2433 後片付け] ユーザー環境変数 Path を検査前へ戻せなかった（前 $($Before.Raw) / 後 $($afterPath.Raw)）"
    }
    Remove-Item -LiteralPath $RunRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) {
    foreach ($f in $failures) { [Console]::WriteLine("::error::$f") }
    exit 1
}
Write-Host "== #2433 $Mode : 期待どおり =="
exit 0
