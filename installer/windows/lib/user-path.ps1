#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    ユーザー環境変数 Path（HKCU\Environment\Path）を読み戻す・元へ戻す 1 実装（Issue #2767）。

.DESCRIPTION
    ユーザーの Path を書くのは 2 か所ある:
      - インストーラー（tako.iss の [Code] の EnvAddPath / EnvRemovePath）
      - `tako setup` / `tako setup bootstrap path|undo-path`（境界 B23 = tako_core::platform::user_path）
    CI の検査はどちらも「書いた後にレジストリを読み戻して確かめる」。読み方と比べ方を 2 つ持たない
    ために、インストーラーの一巡（lib/verify-assets.ps1 の Test-TakoInstalledPayload）と
    setup の通しの検査（scripts/windows/test-setup-windows-2767.ps1）の両方がこのファイルを
    dot-source する。

    比べ方は tako.iss の NormalizePathEntry に揃える（前後の空白・末尾の \ を落とし、%VAR% を展開し、
    大文字小文字を区別しない）。展開は %LOCALAPPDATA% などの 3 つに限らず全部、前後の引用符も落とす
    （どちらも PATH の要素としては同じ場所を指すため）。空の要素（`a;;b` や末尾の `;`）は数えない。

    書くのは Restore-TakoUserPath（検査の前に取った値へ戻す）だけ。検査はどれも使い捨てのランナー
    （GitHub Actions）で走らせる前提で、呼び手が $env:GITHUB_ACTIONS を確かめる。
#>

Set-StrictMode -Version Latest

$TakoEnvironmentSubKey = 'Environment'

<#
.SYNOPSIS
    HKCU\Environment\Path を展開せずに読む。値が無ければ Exists = $false。
    戻り値: Exists / Raw（レジストリの文字列そのまま）/ Kind（RegistryValueKind）/ Entries（空要素を除いた並び）
#>
function Get-TakoUserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($TakoEnvironmentSubKey)
    try {
        $raw = $null
        $kind = $null
        if ($null -ne $key) {
            $raw = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($null -ne $raw) { $kind = $key.GetValueKind('Path') }
        }
        [PSCustomObject]@{
            Exists  = ($null -ne $raw)
            Raw     = [string]$raw
            Kind    = $kind
            Entries = @(Split-TakoPathList ([string]$raw))
        }
    } finally {
        if ($null -ne $key) { $key.Dispose() }
    }
}

# `;` 区切りを並びにする。空要素は捨てる（tako.iss の SplitPath と同じ）
function Split-TakoPathList([string]$Raw) {
    @($Raw -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
}

# 比べるための正規化（tako.iss の NormalizePathEntry に揃える。違いは冒頭の説明のとおり）
function ConvertTo-TakoPathKey([string]$Entry) {
    $e = [Environment]::ExpandEnvironmentVariables($Entry.Trim().Trim('"'))
    while ($e.Length -gt 1 -and $e.EndsWith('\')) { $e = $e.Substring(0, $e.Length - 1) }
    $e.ToLowerInvariant()
}

<#
.SYNOPSIS
    並び Entries のうち Dir と同じ場所を指す要素の数（0 = 入っていない / 2 以上 = 重複して足した）。
#>
function Get-TakoPathEntryCount {
    param(
        [AllowEmptyCollection()][string[]]$Entries,
        [Parameter(Mandatory = $true)][string]$Dir
    )
    $needle = ConvertTo-TakoPathKey $Dir
    @($Entries | Where-Object { (ConvertTo-TakoPathKey $_) -eq $needle }).Count
}

<#
.SYNOPSIS
    2 つの並びが同じ場所を同じ順で並べているか（正規化して比べる）。
    差があれば最初に食い違った位置の説明を返し、同じなら $null を返す。
#>
function Compare-TakoPathEntries {
    param(
        [AllowEmptyCollection()][string[]]$Expected,
        [AllowEmptyCollection()][string[]]$Actual
    )
    $e = @($Expected | ForEach-Object { ConvertTo-TakoPathKey $_ })
    $a = @($Actual | ForEach-Object { ConvertTo-TakoPathKey $_ })
    $n = [Math]::Max($e.Count, $a.Count)
    for ($i = 0; $i -lt $n; $i++) {
        $want = if ($i -lt $e.Count) { $Expected[$i] } else { '(無い)' }
        $got = if ($i -lt $a.Count) { $Actual[$i] } else { '(無い)' }
        if ($i -ge $e.Count -or $i -ge $a.Count -or $e[$i] -ne $a[$i]) {
            return "$($i + 1) 番目の要素が違う（期待 $want / 実際 $got）"
        }
    }
    $null
}

<#
.SYNOPSIS
    Get-TakoUserPath で取った値へ戻す（無かったなら値を消す）。型（REG_SZ / REG_EXPAND_SZ）も戻す。
#>
function Restore-TakoUserPath {
    param([Parameter(Mandatory = $true)]$Snapshot)
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($TakoEnvironmentSubKey)
    try {
        if ($Snapshot.Exists) {
            $key.SetValue('Path', $Snapshot.Raw, $Snapshot.Kind)
        } else {
            $key.DeleteValue('Path', $false)
        }
    } finally {
        $key.Dispose()
    }
}

<#
.SYNOPSIS
    新しくログインしたプロセスが受け取る PATH（システムの Path に続けてユーザーの Path。どちらも展開する）。
    「いま開いているターミナルには届かないが、新しく開けば届く」を検査の中で再現するのに使う。
#>
function Get-TakoFreshLoginPath {
    $machine = [Environment]::GetEnvironmentVariable('Path', [EnvironmentVariableTarget]::Machine)
    $user = [Environment]::GetEnvironmentVariable('Path', [EnvironmentVariableTarget]::User)
    (@(Split-TakoPathList $machine) + @(Split-TakoPathList $user)) -join ';'
}
