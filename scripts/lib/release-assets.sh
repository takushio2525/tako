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
