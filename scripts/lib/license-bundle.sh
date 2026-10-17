#!/usr/bin/env bash
# license-bundle.sh — 配布物に同梱するライセンス（#1845）の表の読み手と、
# 公開済み Release の zip の中身の検査（#1850。scripts/release.sh --check-assets が使う）
#
# **表の正本は scripts/lib/license-bundle.txt**（1 行 1 本: 元ファイル / Windows での名前）。
# ここにもリリーススクリプトにも名前を並べ直さない（2 か所目の表を作らない）。同じ表を
# crates/tako-control/tests/license_bundle_watchdog.rs が読んで組み立ての全箇所と突き合わせ、
# このファイルと release.sh に名前の直書きが入ったら file:line で名指しして落とす。
#
# zip の中の置き場（2026-10-18 に v0.8.37 の実物の zip で確かめた）:
#   macOS   … tako-<tag>-macos-<arch>.zip の tako.app/Contents/Resources/<元ファイル>
#             （ditto が AppleDouble の ._<名前> を隣に入れるが、見るのは本体の 1 本だけ）
#   Windows … tako-<tag>-windows-<arch>.zip の tako/<Windows での名前>
# インストーラー（.exe）は macOS から中身を開けないのでここでは見ない
# （インストール先の 3 本は release-windows.yml の Test-TakoInstalledPayload が CI で見る）。
#
# 照合先は**そのタグのファイル**（git show <tag>:<元ファイル>）。今の作業ツリーと比べると、
# 版の後でライセンスの本文を直したときに古い版が偽の不一致になる。
#
# **bash 専用**・macOS 同梱の bash 3.2 でも通す（source するのは bash スクリプトだけ）

# 表の置き場（リポジトリのルートからの相対パス）
TAKO_LICENSE_BUNDLE_TABLE="scripts/lib/license-bundle.txt"
# zip の中の置き場（上の実測を参照）
TAKO_LICENSE_ZIP_DIR_MACOS="tako.app/Contents/Resources"
TAKO_LICENSE_ZIP_DIR_WINDOWS="tako"

# tako_license_bundle <repo_root> — 表を「元ファイル<TAB>Windows での名前」の行で返す。
# 表が無い・空・2 列でない行があれば、stderr へ file:line を出して非 0
tako_license_bundle() {
  local table="$1/$TAKO_LICENSE_BUNDLE_TABLE" line n=0 rows=0 src win extra
  local skip_re='^[[:space:]]*(#|$)'
  if [[ ! -f "$table" ]]; then
    echo "エラー: ライセンスの同梱表 ${TAKO_LICENSE_BUNDLE_TABLE} が無い" >&2
    return 1
  fi
  while IFS= read -r line || [[ -n "$line" ]]; do
    n=$((n + 1))
    line=${line%$'\r'}
    [[ $line =~ $skip_re ]] && continue
    src="" win="" extra=""
    read -r src win extra <<< "$line"
    if [[ -z "$win" || -n "$extra" ]]; then
      echo "エラー: ${TAKO_LICENSE_BUNDLE_TABLE}:${n}: 「元ファイル Windows での名前」の 2 列になっていない: $line" >&2
      return 1
    fi
    printf '%s\t%s\n' "$src" "$win"
    rows=$((rows + 1))
  done < "$table"
  if [[ $rows -eq 0 ]]; then
    echo "エラー: ライセンスの同梱表 ${TAKO_LICENSE_BUNDLE_TABLE} に 1 本も無い" >&2
    return 1
  fi
}

# tako_license_zip_entry <platform> <元ファイル> <Windows での名前> — zip の中のパス
tako_license_zip_entry() {
  case "$1" in
    macos)   printf '%s/%s' "$TAKO_LICENSE_ZIP_DIR_MACOS" "$2" ;;
    windows) printf '%s/%s' "$TAKO_LICENSE_ZIP_DIR_WINDOWS" "$3" ;;
    *)       return 1 ;;
  esac
}

# tako_license_check_zip <zip> <platform> <tag> <repo_root> <work_dir>
#   zip に表の全部が入り、タグ <tag> の元ファイルとバイト一致するかを 1 本ずつ [OK] / [NG] で出す。
#   1 本でも欠け・食い違いがあれば非 0。<work_dir> へ 1 本ずつ取り出して比べる（展開はここだけ）
tako_license_check_zip() {
  local zip="$1" platform="$2" tag="$3" root="$4" work="$5"
  local bundle listing src win entry got want ng=0 got_sum want_sum
  bundle=$(tako_license_bundle "$root") || return 1
  if ! listing=$(unzip -Z1 "$zip" 2>/dev/null); then
    echo "      [NG] zip を開けない（壊れている・zip ではない）: ${zip##*/}"
    return 1
  fi
  mkdir -p "$work"
  got="$work/from-zip"
  want="$work/from-tag"
  while IFS=$'\t' read -r src win; do
    [[ -n "$src" ]] || continue
    entry=$(tako_license_zip_entry "$platform" "$src" "$win") || return 1
    # 一覧との照合は here-string（pipefail の下で grep -q へパイプしない = #1633）
    if ! grep -qxF -- "$entry" <<< "$listing"; then
      echo "      [NG] ${entry}: zip に無い（元ファイル ${src}）"
      ng=$((ng + 1))
      continue
    fi
    if ! unzip -p "$zip" "$entry" > "$got" 2>/dev/null; then
      echo "      [NG] ${entry}: zip から取り出せない"
      ng=$((ng + 1))
      continue
    fi
    if ! git -C "$root" show "${tag}:${src}" > "$want" 2>/dev/null; then
      echo "      [NG] ${entry}: タグ ${tag} に ${src} が無く照合できない"
      ng=$((ng + 1))
      continue
    fi
    if cmp -s "$got" "$want"; then
      echo "      [OK] ${entry}（タグ ${tag} の ${src} とバイト一致）"
    else
      got_sum=$(shasum -a 256 "$got")
      want_sum=$(shasum -a 256 "$want")
      echo "      [NG] ${entry}: タグ ${tag} の ${src} と中身が違う" \
        "（zip $(( $(wc -c < "$got") )) B・sha256 ${got_sum:0:12} / タグ $(( $(wc -c < "$want") )) B・sha256 ${want_sum:0:12}）"
      ng=$((ng + 1))
    fi
  done <<< "$bundle"
  rm -f "${got:?}" "${want:?}"
  [[ $ng -eq 0 ]]
}
