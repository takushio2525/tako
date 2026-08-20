#!/usr/bin/env bash
# test-launch-services.sh — lib/launch-services.sh の後始末ロジックのモックテスト（#837）
#
# 偽 lsregister を TAKO_LSREGISTER で挿し、本番の Launch Services データベースには
# 一切触らずに検証する（実 LS を触ると他のアプリの登録を壊し得る）。
#
# 見ているのは 3 点:
#   1. ビルド出力の .app を消す
#   2. 登録解除は**実体が無いパスだけ**（実体があるものは -u しても約 1 分で戻るので、
#      勝手に消さず掃除手順を提示する側に回す）
#   3. 警告文が実際に出力される（`$var（` のように全角が続く箇所で bash が UTF-8 の
#      バイトを変数名へ取り込み `set -u` で落ちる罠を踏んでいないこと）
set -uo pipefail
cd "$(dirname "$0")/.."
PASS=0
FAIL=0
ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1 ($2)"; FAIL=$((FAIL + 1)); }
assert_absent()  { if [[ ! -e "$2" ]]; then ok "$1"; else ng "$1" "まだ存在する: $2"; fi; }
assert_present() { if [[ -e "$2" ]]; then ok "$1"; else ng "$1" "存在しない: $2"; fi; }
assert_has()     { if grep -qF -- "$2" <<< "$3"; then ok "$1"; else ng "$1" "見つからない: $2"; fi; }
assert_hasnt()   { if grep -qF -- "$2" <<< "$3"; then ng "$1" "含まれてはいけない: $2"; else ok "$1"; fi; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# $1 = .app のパス, $2 = CFBundleDocumentTypes を宣言するか（yes/no）
mkapp() {
  mkdir -p "$1/Contents"
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<plist version="1.0"><dict>'
    if [[ "$2" == yes ]]; then
      echo '<key>CFBundleDocumentTypes</key><array><dict>'
      echo '<key>CFBundleTypeName</key><string>Text</string></dict></array>'
    fi
    echo '<key>CFBundleName</key><string>tako</string>'
    echo '</dict></plist>'
  } > "$1/Contents/Info.plist"
}

CANON="$TMP/Applications/tako.app"   ; mkapp "$CANON" yes   # 正本（残す・触らない）
BUILD="$TMP/repo/dist/tako.app"      ; mkapp "$BUILD" yes   # ビルド出力（消す）
OTHER="$TMP/other/dist/tako.app"     ; mkapp "$OTHER" yes   # 実体あり + 宣言あり（警告 = 候補に出る）
OLD="$TMP/dl/tako.app"               ; mkapp "$OLD"   no    # 実体あり + 宣言なし（警告 = 候補には出ない）
GHOST="$TMP/gone/dist/tako.app"                            # 実体なし（-u される）

FAKE="$TMP/lsregister"
cat > "$FAKE" <<INNER
#!/usr/bin/env bash
case "\$1" in
  -dump)
    printf '\tpath:                       %s (0x1)\n' "$CANON"
    printf '\tpath:                       %s (0x2)\n' "$BUILD"
    printf '\tpath:                       %s (0x3)\n' "$OTHER"
    printf '\tpath:                       %s (0x4)\n' "$OLD"
    printf '\tpath:                       %s (0x5)\n' "$GHOST"
    printf '\tpath:                       %s (0x6)\n' "/Applications/NotTako.app"
    ;;
  -u) echo "\$2" >> "$TMP/unregistered" ;;
  -f) echo "\$2" >> "$TMP/registered" ;;
esac
INNER
chmod +x "$FAKE"

export TAKO_LSREGISTER="$FAKE"
export LS_CANONICAL_APP="$CANON"
# shellcheck source=lib/launch-services.sh
source scripts/lib/launch-services.sh

echo ""
echo "--- Test 1: ビルド出力の後始末 ---"
: > "$TMP/unregistered"
out=$(ls_drop_build_output "$BUILD" "$TMP/repo/dist" 2>&1)
rc=$?
unreg=$(cat "$TMP/unregistered" 2>/dev/null || true)

if [[ $rc -eq 0 ]]; then ok "exit 0（set -u で落ちない）"; else ng "exit 0（set -u で落ちない）" "rc=$rc"; fi
assert_absent  "ビルド出力を削除した" "$BUILD"
assert_absent  "空になった dist を rmdir した" "$TMP/repo/dist"
assert_present "正本には触らない" "$CANON"
assert_present "他人の .app は消さない（実体あり）" "$OTHER"
assert_has   "ビルド出力を登録解除した"      "$BUILD"  "$unreg"
assert_has   "実体の無い登録を解除した"      "$GHOST"  "$unreg"
assert_hasnt "正本は登録解除しない"          "$CANON"  "$unreg"
assert_hasnt "実体のある他の .app は解除しない（約 1 分で戻るため）" "$OTHER" "$unreg"
assert_has   "警告に「候補に出る」が出る"    "候補に出る"     "$out"
assert_has   "警告に「候補には出ない」が出る" "候補には出ない" "$out"
assert_has   "警告に掃除コマンドが出る"      "rm -rf"        "$out"
assert_hasnt "tako.app 以外は列挙しない"     "NotTako.app"   "$out"

echo ""
echo "--- Test 2: 残るものが無ければ警告を出さない ---"
rm -rf "$OTHER" "$OLD"
: > "$TMP/unregistered"
mkapp "$BUILD" yes
out2=$(ls_drop_build_output "$BUILD" "$TMP/repo/dist" 2>&1)
assert_hasnt "警告なし" "警告:" "$out2"
assert_has   "実体の無い登録は解除する" "$OTHER" "$(cat "$TMP/unregistered")"

echo ""
echo "--- Test 3: lsregister が無い環境では no-op（削除だけ） ---"
mkapp "$BUILD" yes
TAKO_LSREGISTER=/nonexistent/lsregister
LSREGISTER=/nonexistent/lsregister
out3=$(ls_drop_build_output "$BUILD" "$TMP/repo/dist" 2>&1)
assert_absent "ビルド出力は消える" "$BUILD"
assert_hasnt  "登録解除の行は出ない" "登録解除" "$out3"

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail"
echo "================================"
[[ $FAIL -eq 0 ]]
