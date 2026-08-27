#!/usr/bin/env bash
# release-assets.sh — リリースアセット命名規則のシェル側の写し（#594 / #595）
#
# **判定ロジックの正は crates/tako-core/src/platform/release_assets.rs**。
# このファイルはリリーススクリプトから使うための写しであり、
# 両者が一致していることは Rust 側の同期テストが機械検証する:
#
#   cargo test -p tako-core release_assets
#     - shell_mirror_declares_same_constants   … 定数（接頭辞 / 拡張子 / 表示名）
#     - shell_mirror_generates_identical_names … tako_asset_name の生成結果
#
# 命名規則を変えるときは **Rust 側を直してからここを合わせる**。
# 片方だけ直すと上のテストが落ちる（それが狙い）。
#
# 命名規則:
#   tako-<tag>-<platform>-<arch>.<ext>
#   例) tako-v0.5.13-macos-arm64.zip / tako-v0.6.0-windows-x86_64.exe
#
# **bash 専用**（`source` するのは bash スクリプトだけ）。`tako_asset_is_for` の
# `[[ == ]]` パターンは zsh では同じ結果にならない（zsh で手検証すると「macOS の
# 配布物が無い」と誤って出るので、確認は `bash -c 'source …'` で行う）

# アセット名の接頭辞
TAKO_ASSET_PREFIX="tako-"

# プラットフォームごとの許容拡張子。**先頭が主形式**（優先順位も Rust 側と一致させる）
TAKO_ASSET_EXTS_MACOS="zip"
TAKO_ASSET_EXTS_WINDOWS="exe zip"

# リリースノートのダウンロード表に出す表示名
TAKO_ASSET_LABEL_MACOS="macOS"
TAKO_ASSET_LABEL_WINDOWS="Windows"

# 対応プラットフォーム（表の行順）
TAKO_ASSET_PLATFORMS="macos windows"

# tako_asset_ext_list <platform> — 許容拡張子を空白区切りで返す
tako_asset_ext_list() {
  case "$1" in
    macos)   printf '%s' "$TAKO_ASSET_EXTS_MACOS" ;;
    windows) printf '%s' "$TAKO_ASSET_EXTS_WINDOWS" ;;
    *)       return 1 ;;
  esac
}

# tako_asset_primary_ext <platform> — 主形式の拡張子
tako_asset_primary_ext() {
  local exts
  exts=$(tako_asset_ext_list "$1") || return 1
  printf '%s' "${exts%% *}"
}

# tako_asset_label <platform> — 表示名
tako_asset_label() {
  case "$1" in
    macos)   printf '%s' "$TAKO_ASSET_LABEL_MACOS" ;;
    windows) printf '%s' "$TAKO_ASSET_LABEL_WINDOWS" ;;
    *)       return 1 ;;
  esac
}

# tako_asset_name <tag> <platform> <arch> [ext] — アセット名を組み立てる
# ext 省略時は主形式
tako_asset_name() {
  local tag="$1" platform="$2" arch="$3" ext="${4:-}"
  if [[ -z "$ext" ]]; then
    ext=$(tako_asset_primary_ext "$platform") || return 1
  fi
  printf '%s%s-%s-%s.%s' "$TAKO_ASSET_PREFIX" "$tag" "$platform" "$arch" "$ext"
}

# tako_asset_is_for <file-name> <platform> — そのファイルが指定 OS 向けの配布物か
# （arch は問わない。リリース側は「どの OS の配布物が揃っているか」だけ判れば足りる）
tako_asset_is_for() {
  local name platform ext matched
  name=$(basename -- "$1")
  platform="$2"
  case "$name" in
    "${TAKO_ASSET_PREFIX}"*) ;;
    *) return 1 ;;
  esac
  matched=1
  for ext in $(tako_asset_ext_list "$platform"); do
    # tako-<tag>-<platform>-<arch>.<ext>
    if [[ "$name" == "${TAKO_ASSET_PREFIX}"*"-${platform}-"*".${ext}" ]]; then
      matched=0
      break
    fi
  done
  return $matched
}

# --- リリースの完全性（#965）------------------------------------------------
#
# リリースは macOS / Windows の配布物が**揃って初めて成立する**。片方だけ出ると、
# 欠けた OS の利用者には「更新が無い」ように見えたままバージョンだけが進む
# （更新判定は自 OS 用アセットの有無で決まる = #595）。
# 判定の正は release_assets.rs の missing_platforms / is_complete。

# tako_asset_missing_platforms <file-name...> — 配布物が 1 つも無い OS を 1 行ずつ返す
tako_asset_missing_platforms() {
  local platform name found
  for platform in $TAKO_ASSET_PLATFORMS; do
    found=1
    for name in "$@"; do
      [[ -n "$name" ]] || continue
      if tako_asset_is_for "$name" "$platform"; then
        found=0
        break
      fi
    done
    [[ $found -eq 0 ]] || echo "$platform"
  done
  return 0
}

# tako_asset_is_complete <file-name...> — 両 OS が揃っていれば 0
tako_asset_is_complete() {
  [[ -z "$(tako_asset_missing_platforms "$@")" ]]
}

# --- 動作要件（#965）--------------------------------------------------------
# リリースノートに載せる最低要件。文言の正は release_assets.rs の os_requirement()
# （日英ともに同期テストで拘束する）

TAKO_ASSET_REQ_MACOS_JA="macOS 11.0 以降 / Apple Silicon（arm64）"
TAKO_ASSET_REQ_MACOS_EN="macOS 11.0 or later / Apple Silicon (arm64)"
TAKO_ASSET_REQ_WINDOWS_JA="Windows 10 バージョン 1809（ビルド 10.0.17763）以降 / x64"
TAKO_ASSET_REQ_WINDOWS_EN="Windows 10 version 1809 (build 10.0.17763) or later / x64"

# tako_asset_requirement <platform> [ja|en] — 動作要件の文言（既定は ja）
tako_asset_requirement() {
  case "$1:${2:-ja}" in
    macos:ja)   printf '%s' "$TAKO_ASSET_REQ_MACOS_JA" ;;
    macos:en)   printf '%s' "$TAKO_ASSET_REQ_MACOS_EN" ;;
    windows:ja) printf '%s' "$TAKO_ASSET_REQ_WINDOWS_JA" ;;
    windows:en) printf '%s' "$TAKO_ASSET_REQ_WINDOWS_EN" ;;
    *)          return 1 ;;
  esac
}
