#!/usr/bin/env bash
# test-bundle-install.sh — lib/bundle-install.sh のモックテスト（#1042）
#
# 一時ディレクトリの dummy .app だけを相手にする（本番の /Applications には触らない）。
# 見ているのは 5 点:
#   1. 差し替え後に中身が新版になり、.app の inode が変わらず、残骸も残らない
#   2. 置き場が空くタイミングが無い（= Dock のピンが逃げる余地が無い）
#   3. 置き場が空のときは新規に置ける
#   4. swap が使えない環境では旧挙動へ落ち、警告を出したうえで更新自体は成立する
#   5. 差し替え元が壊れていれば置き場を壊さない
set -uo pipefail
cd "$(dirname "$0")/.."
PASS=0
FAIL=0
ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1 ($2)"; FAIL=$((FAIL + 1)); }
assert_eq()      { if [[ "$2" == "$3" ]]; then ok "$1"; else ng "$1" "期待 '$3' / 実際 '$2'"; fi; }
assert_present() { if [[ -e "$2" ]]; then ok "$1"; else ng "$1" "存在しない: $2"; fi; }
assert_has()     { if grep -qF -- "$2" <<< "$3"; then ok "$1"; else ng "$1" "見つからない: $2"; fi; }

# shellcheck source=lib/bundle-install.sh
source "$PWD/scripts/lib/bundle-install.sh"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# $1 = .app のパス, $2 = 版数
mkapp() {
  mkdir -p "$1/Contents/MacOS"
  printf '%s' "$2" > "$1/Contents/version.txt"
}
verof() { cat "$1/Contents/version.txt" 2>/dev/null; }

echo "== Test 1: 既存を差し替えると新版になり、残骸が残らない =="
APPS="$TMP/t1/Applications"; mkdir -p "$APPS"
mkapp "$APPS/tako.app" 0.8.0
mkapp "$TMP/t1/new/tako.app" 0.8.1
ino_before="$(stat -f '%i' "$APPS/tako.app")"
strategy="$(install_bundle_in_place "$TMP/t1/new/tako.app" "$APPS/tako.app")"
ino_after="$(stat -f '%i' "$APPS/tako.app")"
assert_eq "Contents だけの入れ替えを選ぶ" "$strategy" "contents-swap"
assert_eq "中身が新版になっている" "$(verof "$APPS/tako.app")" "0.8.1"
leftovers="$(ls -A "$APPS" | grep -v '^tako.app$' || true)"
assert_eq "作業用ディレクトリ・退避が残らない" "$leftovers" ""
assert_eq ".app 自体の inode が変わらない" "$ino_after" "$ino_before"

echo "== Test 2: 差し替え中に置き場が空になる瞬間が無い =="
# 置き場を高頻度でサンプリングし「不在」を観測した回数を数える。
# $1 = 作業ディレクトリ, $2 = 差し替えを行うコマンド（置き場は $APPS/tako.app）
count_vacancies() {
  local work="$1"; shift
  APPS="$work/Applications"; mkdir -p "$APPS"
  mkapp "$APPS/tako.app" 0.8.0
  mkapp "$work/new/tako.app" 0.8.1
  local misses="$work/misses"
  : > "$misses"
  ( for _ in $(seq 1 20000); do [[ -e "$APPS/tako.app" ]] || echo miss >> "$misses"; done ) &
  local watcher=$!
  "$@" > /dev/null 2>&1
  wait "$watcher" 2>/dev/null
  wc -l < "$misses" | tr -d ' '
}

fixed_replace() { install_bundle_in_place "$1/new/tako.app" "$1/Applications/tako.app"; }
# 監視が本当に効いているかの対照（計器の点検）。旧来の rm -rf → cp -R と同じ形に、
# 観測できる幅の窓（50ms）を明示的に作る。dummy .app は小さくコピーが一瞬で終わるため、
# 窓を作らないと「旧手順でも 0 回」になり、対照が対照にならない
legacy_replace_with_window() {
  rm -rf "$1/Applications/tako.app"
  sleep 0.05
  cp -R "$1/new/tako.app" "$1/Applications/tako.app"
}

vac_fixed="$(count_vacancies "$TMP/t2-fixed" fixed_replace "$TMP/t2-fixed")"
vac_control="$(count_vacancies "$TMP/t2-control" legacy_replace_with_window "$TMP/t2-control")"
assert_eq "新しい手順では不在を一度も観測しない" "$vac_fixed" "0"
assert_eq "中身が新版になっている" "$(verof "$TMP/t2-fixed/Applications/tako.app")" "0.8.1"
if [[ "$vac_control" -gt 0 ]]; then
  ok "検出力: 置き場が空く手順なら観測できる（${vac_control} 回）"
else
  ng "検出力: 置き場が空く手順なら観測できる" "対照でも 0 回 = 監視が効いていない"
fi

echo "== Test 3: 置き場が空なら新規に置ける =="
APPS="$TMP/t3/Applications"; mkdir -p "$APPS"
mkapp "$TMP/t3/new/tako.app" 0.8.1
strategy="$(install_bundle_in_place "$TMP/t3/new/tako.app" "$APPS/tako.app")"
assert_eq "新規設置と報告する" "$strategy" "fresh"
assert_present "置き場に設置されている" "$APPS/tako.app"
assert_eq "中身が新版" "$(verof "$APPS/tako.app")" "0.8.1"

echo "== Test 4: swap が使えない環境では警告つきで旧挙動へ落ちる =="
APPS="$TMP/t4/Applications"; mkdir -p "$APPS"
mkapp "$APPS/tako.app" 0.8.0
mkapp "$TMP/t4/new/tako.app" 0.8.1
# 常に失敗する偽 python3 を挿す（本番の python3 にも renamex_np にも触らない）
FAKE_PY="$TMP/t4/fake-python3"
printf '#!/bin/sh\nexit 1\n' > "$FAKE_PY"; chmod +x "$FAKE_PY"
warn="$TMP/t4/warn.txt"
strategy="$(TAKO_BUNDLE_INSTALL_PYTHON="$FAKE_PY" install_bundle_in_place \
  "$TMP/t4/new/tako.app" "$APPS/tako.app" 2> "$warn")"
assert_eq "旧挙動へ落ちたと報告する" "$strategy" "move-aside"
assert_eq "更新自体は成立する" "$(verof "$APPS/tako.app")" "0.8.1"
assert_has "ピンが外れうることを伏せない" "Dock のピン留めが外れることがあります" "$(cat "$warn")"

echo "== Test 5: 差し替え元が壊れていれば置き場を壊さない =="
APPS="$TMP/t5/Applications"; mkdir -p "$APPS"
mkapp "$APPS/tako.app" 0.8.0
if install_bundle_in_place "$TMP/t5/does-not-exist.app" "$APPS/tako.app" > /dev/null 2>&1; then
  ng "存在しない差し替え元では失敗する" "成功してしまった"
else
  ok "存在しない差し替え元では失敗する"
fi
assert_eq "旧版が生きている" "$(verof "$APPS/tako.app")" "0.8.0"
leftovers="$(ls -A "$APPS" | grep -v '^tako.app$' || true)"
assert_eq "失敗しても残骸を残さない" "$leftovers" ""

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[[ $FAIL -eq 0 ]]
