#!/usr/bin/env bash
# test-measure-vd-sleep-2313.sh — measure-unfocused-redraw-1979.sh が面の眠りを読めることのモックテスト（#2313）
#
# tako:run: bash scripts/test-measure-vd-sleep-2313.sh
#
# 測定スクリプトは測り終えた時点の面の状態を `virtual-display.sh status` で読む。status は
# 使用可のときだけ 0 を返すので、終了コードの成功を前提にした以前の判定は眠っていた回を
# 一度も読めず、眠っていても「描画可能」と出していた（#1979 から）。
#
# **実ディスプレイ・器（BetterDisplay）には一切触らない**:
#   - 判定の関数（vd_drawable_word）を測定スクリプトから名前で抜き出して走らせる
#     （測定スクリプト自体は隔離 GUI をビルド・起動するので走らせない）
#   - 関数が呼ぶ係は偽物だが、中で**本物の virtual-display.sh を読んで status を走らせる**。
#     差し替えるのは画面の列挙・描画可能の判定・蓋・器の口だけ（= status の終了コードと
#     文言は本物）。CI（画面の無いランナー）でも走る
#
# 見ているのは 3 点:
#   1. 起きている面は「描画可能」、眠っている面は「眠っていた」
#   2. 器が応答しない・未接続・物理画面の名前・係が何も出さないときは「確かめられない」
#      （描画可能と言わない = 偽の緑にしない）。理由として status の 1 行目を stderr へ出す
#   3. 検出力: 以前の判定（status の成功を前提にする形）をこの偽の係に当てると、眠っていても
#      「描画可能」と出る（= このテストは #2313 の症状を再現できる）
set -uo pipefail
cd "$(dirname "$0")/.."
PASS=0
FAIL=0
ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1 ($2)"; FAIL=$((FAIL + 1)); }
assert_eq() { if [[ "$2" == "$3" ]]; then ok "$1"; else ng "$1" "期待 '$3' / 実際 '$2'"; fi; }
assert_has() { if grep -qF -- "$2" <<< "$3"; then ok "$1"; else ng "$1" "見つからない: $2 / 実際: $3"; fi; }
assert_lacks() { if grep -qF -- "$2" <<< "$3"; then ng "$1" "在ってはいけない: $2 / 実際: $3"; else ok "$1"; fi; }

MEASURE=scripts/measure-unfocused-redraw-1979.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# ── 偽の係（本物の status を、画面・器の口だけ差し替えて走らせる）──────────
STUB_DIR="$TMP/stub"
FAKE_ROOT="$TMP/root"
mkdir -p "$STUB_DIR" "$FAKE_ROOT/scripts/lib"
export REAL_VD="$PWD/scripts/lib/virtual-display.sh"
export STUB_DIR
cat > "$FAKE_ROOT/scripts/lib/virtual-display.sh" <<'EOF'
#!/bin/bash
# 本物の virtual-display.sh を関数として読み、画面・器の口だけを差し替えて status を 1 回走らせる
. "$REAL_VD"
VD_RECORD_DIR="$STUB_DIR/record"
vd_screens() { cat "$STUB_DIR/table" 2>/dev/null; }
vd_drawable() { cat "$STUB_DIR/drawable" 2>/dev/null; }
vd_clamshell_raw() { cat "$STUB_DIR/clamshell" 2>/dev/null; }
vd_backend_instances() { printf 1; }
vd_backend_responding() { [ ! -f "$STUB_DIR/backend-down" ]; }
vd_backend_list() { printf '17\t13\ttako-vd\n'; }
# 器・AX・起こす手立ては本物へ届かせない（届いたら記録して、テストが 0 回を確かめる）
vd_bd() { echo "vd_bd $*" >> "$STUB_DIR/reached.log"; return 0; }
vd_wake_displays() { echo "wake" >> "$STUB_DIR/reached.log"; return 0; }
osascript() { echo "osascript $*" >> "$STUB_DIR/reached.log"; return 1; }
vd_display_uuid() { printf 'AAAAAAAA-0000-4000-8000-%012d' "${1:-0}"; }
case "${1:-}" in
    status) vd_status ;;
    *) echo "偽の係は status だけ" >&2; exit 64 ;;
esac
EOF
chmod +x "$FAKE_ROOT/scripts/lib/virtual-display.sh"
: > "$STUB_DIR/reached.log"
printf '  |   "AppleClamshellState" = Yes\n' > "$STUB_DIR/clamshell"

t() { printf '%b' "$1"; }   # \t を実タブへ
TABLE_VD=$(t 'tako-vd\t1512\t0\t2560\t1440\t17\t0\t0')
TABLE_LCD=$(t 'Color LCD\t0\t0\t1512\t982\t1\t1\t1\ntako-vd\t1512\t0\t2560\t1440\t17\t0\t0')
set_table() { printf '%s\n' "$1" > "$STUB_DIR/table"; }
awake() { t '1\t1\t0\n17\t1\t0\n' > "$STUB_DIR/drawable"; }
asleep() { t '1\t1\t0\n17\t0\t1\n' > "$STUB_DIR/drawable"; }
status_line() { "$REPO_ROOT/scripts/lib/virtual-display.sh" status 2>/dev/null | sed -n 1p; }

# ── 判定の関数を測定スクリプトから名前で抜き出す ─────────────────────────
echo "== Test 1: 測定スクリプトが判定を vd_drawable_word の 1 か所で行う =="
FN_SRC=$(sed -n '/^vd_drawable_word() {$/,/^}$/p' "$MEASURE")
if [ -z "$FN_SRC" ]; then
    ng "vd_drawable_word を抜き出せる" "見つからない（関数名を変えたならこのテストも直す）"
    echo
    echo "PASS=${PASS} FAIL=${FAIL}"
    exit 1
fi
ok "vd_drawable_word を抜き出せる"
eval "$FN_SRC"
if grep -qF 'drawable="$(vd_drawable_word)"' "$MEASURE"; then
    ok "run_arm が vd_drawable_word の語を使う"
else
    ng "run_arm が vd_drawable_word の語を使う" "drawable=\"\$(vd_drawable_word)\" が無い"
fi
assert_eq "status を読むのは vd_drawable_word の 1 か所だけ" \
    "$(grep -c 'virtual-display.sh" status' "$MEASURE")" "1"
assert_has "その 1 か所は vd_drawable_word の中" 'virtual-display.sh" status' "$FN_SRC"

REPO_ROOT="$FAKE_ROOT"
# 判定を 1 回走らせ、語を word・stderr を err へ
judge() { word=$(vd_drawable_word 2>"$TMP/err"); err=$(cat "$TMP/err"); }

echo "== Test 2: 起きている面は「描画可能」、眠っている面は「眠っていた」 =="
set_table "$TABLE_VD"; awake
judge
assert_eq "起きている（status の 1 行目: $(status_line)）" "$word" "描画可能"
assert_eq "描画可能のときは stderr に何も出さない" "$err" ""
asleep
judge
assert_eq "眠っている（status の 1 行目: $(status_line)）" "$word" "眠っていた"
assert_eq "眠っていたときも stderr に何も出さない" "$err" ""

echo "== Test 3: 状態を確かめられないときは描画可能と言わない（偽の緑にしない）=="
set_table "$TABLE_VD"; awake; touch "$STUB_DIR/backend-down"
judge
assert_eq "器が応答しない（面は起きていても）" "$word" "確かめられない"
assert_has "理由に status の 1 行目を出す" "仮想ディスプレイか確かめられない" "$err"
rm -f "${STUB_DIR:?}/backend-down"
asleep; touch "$STUB_DIR/backend-down"
judge
assert_eq "器が応答しない（面は眠っている）" "$word" "確かめられない"
rm -f "${STUB_DIR:?}/backend-down"
: > "$STUB_DIR/table"; awake
judge
assert_eq "未接続" "$word" "確かめられない"
assert_has "未接続と理由を出す" "未接続" "$err"
set_table "$TABLE_LCD"; awake
export TAKO_VD_NAME="Color LCD"
judge
unset TAKO_VD_NAME
assert_eq "物理画面の名前（status は 3）" "$word" "確かめられない"
assert_has "仮想ディスプレイではないと理由を出す" "仮想ディスプレイではない" "$err"
REPO_ROOT="$TMP/no-such-root"
judge
REPO_ROOT="$FAKE_ROOT"
assert_eq "係が何も出さない（置き場に無い）" "$word" "確かめられない"
assert_has "何も出さなかったと理由を出す" "何も出さない" "$err"
for w in 描画可能 眠っていた; do
    assert_lacks "確かめられない回の stderr に「${w}」の語を混ぜない" "$w" "$err"
done

echo "== Test 4: 偽の係は器・AX・起こす手立てへ 1 度も届かない =="
assert_eq "届いた呼び出し" "$(grep -c . "$STUB_DIR/reached.log")" "0"

echo "== Test 5: 検出力（以前の判定はこの偽の係で眠っていても「描画可能」と出す）=="
# a9f8835（#1979）〜 #2313 の前まで measure-unfocused-redraw-1979.sh にあった形をそのまま写す
legacy_word() {
    local drawable vd_status
    drawable="描画可能"
    vd_status="$("$REPO_ROOT/scripts/lib/virtual-display.sh" status 2>/dev/null)" \
        && [[ "$(sed -n 1p <<<"$vd_status")" == *"眠っている"* ]] && drawable="眠っていた"
    printf '%s\n' "$drawable"
}
set_table "$TABLE_VD"; asleep
assert_eq "以前の判定は眠っていても描画可能と出す（#2313 の症状）" "$(legacy_word)" "描画可能"
judge
assert_eq "いまの判定は同じ状態で眠っていたと出す" "$word" "眠っていた"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
