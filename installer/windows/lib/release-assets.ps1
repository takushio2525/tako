# release-assets.ps1 — リリースアセット命名規則の PowerShell 側の写し（#594 / #595 / #587）
#
# **判定ロジックの正は crates/tako-core/src/platform/release_assets.rs**。
# このファイルは Windows のリリーススクリプトから使うための写しであり、
# 3 者（Rust / sh / PowerShell）が一致していることは Rust 側の同期テストが機械検証する:
#
#   cargo test -p tako-core release_assets
#     - powershell_mirror_declares_same_constants   … 定数（接頭辞 / 拡張子 / 表示名）
#     - powershell_mirror_generates_identical_names … Get-TakoAssetName の生成結果
#
# 命名規則を変えるときは **Rust 側を直してからここを合わせる**。
# 片方だけ直すと上のテストが落ちる（それが狙い）。#595 の事故（リリース側と更新チェック側で
# 命名判定が食い違い、Windows クライアントが自 OS 向けアセットを掴めない）を再発させないため。
#
# 命名規則:
#   tako-<tag>-<platform>-<arch>.<ext>
#   例) tako-v0.5.13-macos-arm64.zip / tako-v0.6.0-windows-x86_64.exe
#
# 使い方（呼び出し側でドットソースする）:
#   . (Join-Path $PSScriptRoot 'lib/release-assets.ps1')
#   $name = Get-TakoAssetName -Tag v0.7.0 -Platform windows -Arch x86_64

Set-StrictMode -Version Latest

# アセット名の接頭辞
$TakoAssetPrefix = 'tako-'

# プラットフォームごとの許容拡張子。**先頭が主形式**（優先順位も Rust 側と一致させる）
$TakoAssetExtsMacos = 'zip'
$TakoAssetExtsWindows = 'exe zip'

# リリースノートのダウンロード表に出す表示名
$TakoAssetLabelMacos = 'macOS'
$TakoAssetLabelWindows = 'Windows'

# Windows の配布アーキテクチャ。Rust の Arch::X86_64.as_str() と同じトークン
# （`x64` / `amd64` のような別名は Rust 側が受け付けないので使ってはいけない）
$TakoAssetArchWindows = 'x86_64'

# 許容拡張子を空白区切りで返す
function Get-TakoAssetExtList {
    param([Parameter(Mandatory)][string]$Platform)
    switch ($Platform) {
        'macos' { return $script:TakoAssetExtsMacos }
        'windows' { return $script:TakoAssetExtsWindows }
        default { throw "未知のプラットフォーム: $Platform（macos / windows のみ）" }
    }
}

# 主形式の拡張子
function Get-TakoAssetPrimaryExt {
    param([Parameter(Mandatory)][string]$Platform)
    # 呼び出し全体を括る。括らないと -split が Get-TakoAssetExtList のパラメータとして解釈される
    ((Get-TakoAssetExtList -Platform $Platform) -split ' ')[0]
}

# 表示名
function Get-TakoAssetLabel {
    param([Parameter(Mandatory)][string]$Platform)
    switch ($Platform) {
        'macos' { return $script:TakoAssetLabelMacos }
        'windows' { return $script:TakoAssetLabelWindows }
        default { throw "未知のプラットフォーム: $Platform（macos / windows のみ）" }
    }
}

# アセット名を組み立てる。Ext 省略時は主形式
function Get-TakoAssetName {
    param(
        [Parameter(Mandatory)][string]$Tag,
        [Parameter(Mandatory)][string]$Platform,
        [Parameter(Mandatory)][string]$Arch,
        [string]$Ext
    )
    if (-not $Ext) { $Ext = Get-TakoAssetPrimaryExt -Platform $Platform }
    '{0}{1}-{2}-{3}.{4}' -f $script:TakoAssetPrefix, $Tag, $Platform, $Arch, $Ext
}

# 拡張子を除いたベース名（Inno Setup の OutputBaseFilename へ渡す形）
function Get-TakoAssetBaseName {
    param(
        [Parameter(Mandatory)][string]$Tag,
        [Parameter(Mandatory)][string]$Platform,
        [Parameter(Mandatory)][string]$Arch
    )
    '{0}{1}-{2}-{3}' -f $script:TakoAssetPrefix, $Tag, $Platform, $Arch
}

# `0.7.4` / `v0.7.4` のどちらでも `v0.7.4` に正規化する。
# アセット名に入るのは**タグ表記**（先頭 v あり）と決まっているため（release_assets.rs の命名規則）
function ConvertTo-TakoTag {
    param([Parameter(Mandatory)][string]$Version)
    if ($Version.StartsWith('v')) { return $Version }
    return "v$Version"
}
