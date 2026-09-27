#!/usr/bin/env bash
# test-virtual-display-guard.sh — lib/virtual-display.sh の見張りのモックテスト（#1150）
#
# tako:run: bash scripts/test-virtual-display-guard.sh
#
# **実ディスプレイに一切触らない**: 画面の列挙（vd_screens）・器の CLI（vd_bd）・蓋の状態
# （vd_clamshell_raw）・器の実体数（vd_backend_instances）をすべてスタブへ差し替え、
# 一時ディレクトリのテーブルだけを相手にする。CI（ランナーに画面が無い）でも走る。
#
# 見ているのは 7 点:
#   1. 同名の面が 2 枚以上になったら ensure が理由つきで止まる（#1150 の増殖）
#   2. Main 保護は「内蔵あり かつ Main が仮想」のときだけ器へ main を撃つ
#   3. 孤児の後片付けは実行条件（内蔵が NSScreen に居る + 蓋開き）を満たすまで拒否する
#      （満たさないまま器を再起動すると画面が 0 枚になり機械が眠る）
#   4. 前後比較（status --snapshot）が同じ構成に対して同じ 1 枚の絵を出す
#   5. ensure の完了条件に「面が起きている（描画可能）」が入っている（#1160）。
#      NSScreen に居るだけで通すと、面は在るのに tako から見えず検証が始まらない
#   6. ensure の締めが面の uuid を記録する（#1697）。名前が読めない瞬間に tako が
#      uuid で当てる材料で、読めなくても ensure は失敗させない・同じ値なら書かない
#   7. 名指しされた面が物理画面なら ensure が断る（#1783）。TAKO_VD_NAME に内蔵・外部モニタの
#      名前を渡しても tako-vd と見なさない（uuid の記録・Main 保護へ進む前に止まる）
set -uo pipefail
cd "$(dirname "$0")/.."
PASS=0
FAIL=0
ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1 ($2)"; FAIL=$((FAIL + 1)); }
assert_eq() { if [[ "$2" == "$3" ]]; then ok "$1"; else ng "$1" "期待 '$3' / 実際 '$2'"; fi; }
assert_has() { if grep -qF -- "$2" <<< "$3"; then ok "$1"; else ng "$1" "見つからない: $2 / 実際: $3"; fi; }
assert_lacks() { if grep -qF -- "$2" <<< "$3"; then ng "$1" "在ってはいけない: $2 / 実際: $3"; else ok "$1"; fi; }

# shellcheck source=lib/virtual-display.sh
source "$PWD/scripts/lib/virtual-display.sh"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# **記録の置き場を真っ先に一時 dir へ向ける**（#1697）。これより後ろのどの ensure も
# 本物の ~/Library/Caches へ書かない（テストから本番へ書かない）
VD_RECORD_DIR="$TMP/record"

# ── スタブ（source した本物の関数を上書きする）──────────────────────
TABLE="$TMP/table"
BD_LOG="$TMP/bd.log"
CLAMSHELL="$TMP/clamshell"
WAKE_LOG="$TMP/wake.log"
: > "$BD_LOG"
: > "$WAKE_LOG"

vd_screens() { cat "$TABLE" 2>/dev/null; }
vd_clamshell_raw() { cat "$CLAMSHELL" 2>/dev/null; }
# CoreGraphics の描画可能判定（#1160）。既定は「テーブルの全 id が active」。
# DRAWABLE に中身を置けばそれが答えになり、空文字なら「材料が読めない」を再現する
vd_drawable() {
    if [ -f "$TMP/drawable" ]; then cat "$TMP/drawable"; return 0; fi
    awk -F'\t' 'NF { printf "%s\t1\t0\n", $6 }' "$TABLE" 2>/dev/null
}
# 起こす手立て（caffeinate -u）。撃った回数だけ記録し、起きた後のテーブルへ差し替える
vd_wake_displays() {
    echo wake >> "$WAKE_LOG"
    [ -f "$TMP/drawable.after-wake" ] && cp "$TMP/drawable.after-wake" "$TMP/drawable"
    return 0
}
# 面の uuid（#1697）。既定は displayID から作る大文字のダミー（実機の JXA と同じ大文字）。
# STUB_UUID を立てればそれが答えになり、空文字なら「読めない」を再現する
vd_display_uuid() {
    if [ -n "${STUB_UUID+x}" ]; then printf '%s' "$STUB_UUID"; return 0; fi
    printf 'AAAAAAAA-0000-4000-8000-%012d' "${1:-0}"
}
vd_backend_instances() { printf '%s' "${STUB_INSTANCES:-1}"; }
vd_backend_strays() { printf '%s' "${STUB_STRAYS:-}"; }
# 器が応答するか（#1783 で聞いた回数も数える）。STUB_BACKEND_DOWN を立てれば「器に聞けない」
READY_LOG="$TMP/ready.log"
: > "$READY_LOG"
vd_backend_ready() { echo ready >> "$READY_LOG"; [ -z "${STUB_BACKEND_DOWN:-}" ]; }
# 器の仮想スクリーン一覧（#1783。tagID<TAB>displayID<TAB>name）。既定は「tako-vd が器の仮想
# スクリーン」。STUB_BACKEND_LIST を立てればそれが答えになる（空文字なら「仮想スクリーンが 0 枚」）
vd_backend_list() {
    if [ -n "${STUB_BACKEND_LIST+x}" ]; then printf '%s' "$STUB_BACKEND_LIST"; return 0; fi
    printf '17\t13\ttako-vd\n'
}
# 器へ渡した引数を記録し、「効いた後」のテーブルへ差し替える（応答文ではなく
# 読み戻しで確かめる本物の作法を、スタブ側でも再現する）
vd_bd() {
    printf '%s\n' "$*" >> "$BD_LOG"
    case "$*" in
        *-main=on*) [ -f "$TMP/table.after-main" ] && cp "$TMP/table.after-main" "$TABLE" ;;
        *restartApp*) [ -f "$TMP/table.after-restart" ] && cp "$TMP/table.after-restart" "$TABLE" ;;
    esac
    return 0
}
# 待ちは要らない（判定は全部スタブで確定している）
sleep() { :; }

set_table() { printf '%s\n' "$1" > "$TABLE"; }
set_clamshell() { printf '  |   "AppleClamshellState" = %s\n' "$1" > "$CLAMSHELL"; }
no_clamshell() { : > "$CLAMSHELL"; }
reset_bd() {
    : > "$BD_LOG"; : > "$WAKE_LOG"; : > "$READY_LOG"
    rm -f "$TMP/table.after-main" "$TMP/table.after-restart" "$TMP/drawable" "$TMP/drawable.after-wake"
}
bd_log() { cat "$BD_LOG"; }
wake_count() { grep -c . "$WAKE_LOG" 2>/dev/null || true; }
ready_count() { grep -c . "$READY_LOG" 2>/dev/null || true; }
# 眠っている状態を作る（$1 = 起きている id を列挙。省略なら全部眠っている）
set_drawable() { printf '%s\n' "$1" > "$TMP/drawable"; }

t() { printf '%b' "$1"; }   # \t を実タブへ

# 2026-09-07 の実測（#1150 の症状。蓋閉じ・Main が仮想・孤児 2 枚）
FIX_1150=$(t 'tako-vd\t0\t0\t2560\t1440\t13\t0\t1
tako-vd-ab1141（1）\t5120\t0\t2560\t1440\t15\t0\t0
tako-vd-ab1141（2）\t2560\t0\t2560\t1440\t16\t0\t0')
# 正常（蓋開き・内蔵が Main・仮想は 1 枚）
FIX_OK=$(t 'Color LCD\t0\t0\t1512\t982\t1\t1\t1
tako-vd\t1512\t0\t2560\t1440\t13\t0\t0')
# 内蔵は居るが Main が仮想（蓋を開けた直後に起こりうる = 戻す対象）
FIX_MAIN_VIRTUAL=$(t 'Color LCD\t0\t0\t1512\t982\t1\t1\t0
tako-vd\t1512\t0\t2560\t1440\t13\t0\t1')
# 外部モニタが Main（ユーザーの構成 = 触らない）
FIX_EXTERNAL_MAIN=$(t 'Color LCD\t0\t0\t1512\t982\t1\t1\t0
External 4K\t1512\t0\t3840\t2160\t7\t0\t1
tako-vd\t5352\t0\t2560\t1440\t13\t0\t0')
# 同名が 2 枚（macOS は枝番を付けて見せる）
FIX_DUP=$(t 'tako-vd（1）\t0\t0\t2560\t1440\t13\t0\t1
tako-vd（2）\t2560\t0\t2560\t1440\t14\t0\t0')

echo "== Test 1: 枝番付きの同名を「無い」と読み違えない =="
assert_eq "全角の枝番を落とす" "$(vd_strip_index 'tako-vd-ab1141（1）')" "tako-vd-ab1141"
assert_eq "半角の枝番も落とす" "$(vd_strip_index 'tako-vd (2)')" "tako-vd"
assert_eq "枝番でない数字は残す" "$(vd_strip_index 'tako-vd2')" "tako-vd2"
rows=$(printf '%s\n' "$FIX_DUP" | vd_rows_named tako-vd | grep -c .)
assert_eq "枝番付き 2 枚を同名として数える" "$rows" "2"
# 検出力: 素の完全一致だと 0 件に見える（= 増殖に気づけない元の形）
naive=$(printf '%s\n' "$FIX_DUP" | awk -F'\t' '$1=="tako-vd"' | grep -c . || true)
assert_eq "完全一致だけでは 0 件に見える（元の見落とし方）" "$naive" "0"

echo "== Test 2: 孤児の列挙は常設の 1 枚だけ残す =="
orphans=$(printf '%s\n' "$FIX_1150" | vd_family_rows tako-vd | vd_orphan_rows tako-vd)
assert_eq "#1150 の実測で孤児は 2 枚" "$(printf '%s' "$orphans" | grep -c .)" "2"
assert_has "孤児 1 枚目を名指しする" "tako-vd-ab1141（1）" "$orphans"
assert_has "孤児 2 枚目を名指しする" "tako-vd-ab1141（2）" "$orphans"
assert_lacks "常設の 1 枚は孤児に含めない" "$(t 'tako-vd\t0\t0')" "$orphans"
dups=$(printf '%s\n' "$FIX_DUP" | vd_family_rows tako-vd | vd_orphan_rows tako-vd)
assert_eq "同名 2 枚なら 1 枚だけ残して 1 枚を孤児にする" "$(printf '%s' "$dups" | grep -c .)" "1"
others=$(printf '%s\n' "$FIX_OK" | vd_family_rows tako-vd | vd_orphan_rows tako-vd)
assert_eq "無関係な面（内蔵）は系統に入れない" "$(printf '%s' "$others" | grep -c .)" "0"

echo "== Test 3: Main 保護の判定（純関数・3 ケース）=="
plan=$(printf '%s\n' "$FIX_1150" | vd_main_protection_plan tako-vd)
assert_has "内蔵が居ない → 戻す先が無いので何もしない" "noop" "$plan"
assert_has "理由に「戻す先が無い」と書く" "戻す先が無い" "$plan"
plan=$(printf '%s\n' "$FIX_OK" | vd_main_protection_plan tako-vd)
assert_has "内蔵あり + Main が内蔵 → 何もしない" "noop" "$plan"
plan=$(printf '%s\n' "$FIX_MAIN_VIRTUAL" | vd_main_protection_plan tako-vd)
assert_eq "内蔵あり + Main が仮想 → 内蔵へ戻す" "$plan" "restore 1 Color LCD"
plan=$(printf '%s\n' "$FIX_EXTERNAL_MAIN" | vd_main_protection_plan tako-vd)
assert_has "外部モニタが Main ならユーザーの構成なので触らない" "noop" "$plan"
plan=$(printf '%s' "" | vd_main_protection_plan tako-vd)
assert_has "0 枚（スリープ中）は Main が読めない扱い" "noop" "$plan"

echo "== Test 4: 蓋の状態と実行条件（純関数）=="
assert_eq "Yes は閉じている" "$(vd_clamshell_state '  |   "AppleClamshellState" = Yes')" "closed"
assert_eq "No は開いている" "$(vd_clamshell_state '  |   "AppleClamshellState" = No')" "open"
assert_eq "キーが無い機（デスクトップ）は unknown" "$(vd_clamshell_state '')" "unknown"
gate=$(printf '%s\n' "$FIX_1150" | vd_cleanup_gate closed); rc=$?
assert_eq "蓋閉じ + 内蔵なし → block（終了 1）" "$rc" "1"
assert_has "block の理由は内蔵の不在" "内蔵ディスプレイが NSScreen に居ない" "$gate"
gate=$(printf '%s\n' "$FIX_OK" | vd_cleanup_gate closed); rc=$?
assert_eq "内蔵が居ても蓋閉じなら block" "$rc" "1"
assert_has "block の理由は蓋" "蓋が閉じている" "$gate"
gate=$(printf '%s\n' "$FIX_OK" | vd_cleanup_gate open); rc=$?
assert_eq "内蔵あり + 蓋開き → ok（終了 0）" "$rc" "0"
assert_eq "ok の中身" "$gate" "ok"
gate=$(printf '%s' "" | vd_cleanup_gate open); rc=$?
assert_eq "0 枚（スリープ中）は block" "$rc" "1"

echo "== Test 5: ensure は同名の増殖で止まる =="
reset_bd; set_table "$FIX_DUP"; set_clamshell Yes
STUB_INSTANCES=3
STUB_STRAYS="20684 /Applications/BetterDisplay.app/Contents/MacOS/BetterDisplay create -virtualScreen"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "増えていたら非ゼロで返す" "$rc" "1"
assert_has "枚数を理由に書く" "tako-vd が 2 枚ある" "$out"
assert_has "器の実体数も出す（増殖の既知原因）" "3 実体走っている" "$out"
assert_has "引数つきの実体を pid つきで名指しする" "引数つきの実体: 20684" "$out"
assert_has "下見のコマンドを案内する" "cleanup-orphans" "$out"
assert_lacks "増えているときに器へ何も撃たない" "main=on" "$(bd_log)"
STUB_INSTANCES=1
STUB_STRAYS=""

echo "== Test 6: ensure は 1 枚なら黙って通る（Main が仮想でなければ撃たない）=="
reset_bd; set_table "$FIX_OK"; set_clamshell No
out=$(vd_ensure 2>&1); rc=$?
assert_eq "正常な構成では 0 で返る" "$rc" "0"
assert_eq "器へ 1 度も撃たない" "$(bd_log)" ""
assert_eq "余計な出力を出さない" "$out" ""

echo "== Test 7: ensure の締めが Main を内蔵へ戻す（読み戻しで確かめる）=="
reset_bd; set_table "$FIX_MAIN_VIRTUAL"; set_clamshell No
printf '%s\n' "$FIX_OK" > "$TMP/table.after-main"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "戻せたら 0 で返る" "$rc" "0"
assert_has "内蔵の displayID を名指しして main を撃つ" "set -displayID=1 -main=on" "$(bd_log)"
assert_eq "撃つのは 1 回だけ" "$(bd_log | grep -c 'main=on')" "1"
assert_has "何をしたかを 1 行出す" "Main を内蔵ディスプレイ（Color LCD）へ戻します" "$out"
assert_lacks "戻せたので警告は出さない" "戻せなかった" "$out"

echo "== Test 8: 戻せなかったら黙らない（ensure 自体は成功させる）=="
reset_bd; set_table "$FIX_MAIN_VIRTUAL"; set_clamshell No
out=$(vd_ensure 2>&1); rc=$?   # after-main を置かない = 何度読み戻しても仮想が Main
assert_eq "面は用意できているので ensure は 0" "$rc" "0"
assert_has "戻せなかったことを警告する" "Main を内蔵へ戻せなかった" "$out"

echo "== Test 9: 孤児の後片付けは実行条件を満たすまで拒否する =="
reset_bd; set_table "$FIX_1150"; set_clamshell Yes
out=$(vd_cleanup_orphans --dry-run 2>&1); rc=$?
assert_eq "下見は「実行できない」で非ゼロ（終了 2）" "$rc" "2"
assert_has "孤児の枚数を出す" "孤児 2 枚" "$out"
assert_has "孤児 1 枚目を名指しする" "tako-vd-ab1141（1）" "$out"
assert_has "孤児 2 枚目を名指しする" "tako-vd-ab1141（2）" "$out"
assert_has "拒否の理由を出す" "内蔵ディスプレイが NSScreen に居ない" "$out"
assert_has "いつ実行できるかを出す" "蓋を開けて" "$out"
assert_eq "器へ何も撃たない" "$(bd_log)" ""
STUB_STRAYS="15961 /Applications/BetterDisplay.app/Contents/MacOS/BetterDisplay create -virtualScreen -name=tako-vd"
out=$(vd_cleanup_orphans 2>&1); rc=$?
assert_has "引数つきの実体が居れば pid つきで出す" "15961" "$out"
assert_has "何のための手がかりか書く" "二重に繋ぐ疑い" "$out"
STUB_STRAYS=""

echo "== Test 10: --apply でも実行条件を満たさなければ撃たない（最重要）=="
reset_bd; set_table "$FIX_1150"; set_clamshell Yes
out=$(vd_cleanup_orphans --apply 2>&1); rc=$?
assert_eq "拒否して非ゼロ（終了 2）" "$rc" "2"
assert_lacks "器の再起動を撃たない（撃つと画面が 0 枚になる）" "restartApp" "$(bd_log)"
# 内蔵が居るように見えても蓋が閉じていれば撃たない（条件は独立に見る）
reset_bd; set_table "$FIX_OK"; set_clamshell Yes
out=$(vd_cleanup_orphans --apply 2>&1); rc=$?
assert_eq "孤児が無ければ 0（何もしない）" "$rc" "0"
assert_has "孤児なしと言う" "孤児なし" "$out"
assert_eq "器へ何も撃たない" "$(bd_log)" ""

echo "== Test 11: 条件を満たせば掃除して繋ぎ直す =="
reset_bd
set_table "$(printf '%s\n%s\n' "$FIX_OK" "$(t 'tako-vd-ab1141（1）\t4072\t0\t2560\t1440\t15\t0\t0')")"
set_clamshell No
printf '%s\n' "$FIX_OK" > "$TMP/table.after-restart"
out=$(vd_cleanup_orphans --apply 2>&1); rc=$?
assert_eq "掃除できたら 0" "$rc" "0"
assert_has "器を再起動する" "set -restartApp" "$(bd_log)"
assert_has "掃除後に孤児なしを読み戻す" "孤児なし" "$out"
# 下見は条件を満たしていても何も変えない
reset_bd
set_table "$(printf '%s\n%s\n' "$FIX_OK" "$(t 'tako-vd-ab1141（1）\t4072\t0\t2560\t1440\t15\t0\t0')")"
out=$(vd_cleanup_orphans 2>&1); rc=$?
assert_eq "下見は 0 で返る" "$rc" "0"
assert_has "実行条件を満たしていると伝える" "実行条件は満たしている" "$out"
assert_eq "下見では器へ何も撃たない" "$(bd_log)" ""
out=$(vd_cleanup_orphans --nonsense 2>&1); rc=$?
assert_eq "知らない引数は非ゼロ" "$rc" "1"
assert_has "使える引数を案内する" "--dry-run" "$out"

echo "== Test 12: status --snapshot は同じ構成に同じ絵を出す（前後比較用）=="
set_table "$FIX_1150"; set_clamshell Yes
snap1=$(vd_status_snapshot)
snap2=$(vd_status_snapshot)
assert_eq "同じ構成なら同じ出力（diff がノイズを出さない）" "$snap1" "$snap2"
assert_has "蓋の状態が入る" "clamshell=closed" "$snap1"
assert_has "器の実体数が入る" "backend_instances=" "$snap1"
assert_has "Main の有無が読める" "main=1" "$snap1"
assert_has "内蔵の有無が読める" "builtin=0" "$snap1"
set_table "$FIX_OK"; set_clamshell No
snap3=$(vd_status_snapshot)
if [[ "$snap1" != "$snap3" ]]; then ok "構成が変われば出力も変わる（検出力）"; else ng "構成が変われば出力も変わる（検出力）" "同じ"; fi
assert_has "内蔵が居れば builtin=1 の行が出る" "builtin=1" "$snap3"

echo "== Test 13: 描画可能の判定は証明できるときだけ「置けない」と言う（#1160）=="
DRAW_ASLEEP=$(t '1\t0\t1\n17\t0\t1')
DRAW_AWAKE=$(t '1\t1\t0\n17\t1\t0')
DRAW_VD_ASLEEP=$(t '1\t1\t0\n17\t0\t1')
printf '%s' "$DRAW_AWAKE" | vd_id_drawable 17; rc=$?
assert_eq "active=1 なら置ける" "$rc" "0"
printf '%s' "$DRAW_ASLEEP" | vd_id_drawable 17; rc=$?
assert_eq "active=0 なら置けない" "$rc" "1"
printf '%s' "" | vd_id_drawable 17; rc=$?
assert_eq "材料が 1 行も無いときは断らない（CI / osascript 不可の機）" "$rc" "0"
printf '%s' "$DRAW_AWAKE" | vd_id_drawable 99; rc=$?
assert_eq "一覧に居ない id は置けない" "$rc" "1"
# status に出す名前（純関数）
VD_TABLE_17=$(t 'Color LCD\t0\t0\t1512\t982\t1\t1\t1\ntako-vd\t1512\t0\t2560\t1440\t17\t0\t0')
names=$(printf '%s\n' "$VD_TABLE_17" | vd_sleeping_names "$DRAW_ASLEEP")
assert_eq "両方眠っていれば両方の名前を出す" "$names" "Color LCD, tako-vd"
names=$(printf '%s\n' "$VD_TABLE_17" | vd_sleeping_names "$DRAW_VD_ASLEEP")
assert_eq "眠っている面だけを出す" "$names" "tako-vd"
names=$(printf '%s\n' "$VD_TABLE_17" | vd_sleeping_names "$DRAW_AWAKE")
assert_eq "全部起きていれば空" "$names" ""

echo "== Test 14: ensure は面が眠っていたら起こしてから成功する（#1160 の本体）=="
reset_bd; set_table "$VD_TABLE_17"; set_clamshell No
set_drawable "$DRAW_VD_ASLEEP"                      # tako-vd だけ眠っている
printf '%s\n' "$DRAW_AWAKE" > "$TMP/drawable.after-wake"   # 起こせば起きる
out=$(vd_ensure 2>&1); rc=$?
assert_eq "起こせたら 0 で返る" "$rc" "0"
assert_eq "起こす手立てを 1 回だけ撃つ" "$(wake_count)" "1"
assert_has "何をしたかを 1 行出す" "眠っています" "$out"
assert_lacks "起こせたので警告は出さない" "起こせなかった" "$out"
assert_lacks "起こすために器へは触らない" "connected" "$(bd_log)"

echo "== Test 15: 起こせなければ ensure は非ゼロ（面を用意したと嘘をつかない）=="
reset_bd; set_table "$VD_TABLE_17"; set_clamshell No
set_drawable "$DRAW_VD_ASLEEP"                      # after-wake を置かない = 起きない
out=$(vd_ensure 2>&1); rc=$?
assert_eq "起こせなければ非ゼロ" "$rc" "1"
assert_has "起こせなかったことを言う" "起こせなかった" "$out"
assert_has "この状態で何が起きるかを書く" "窓を開かずに終わる" "$out"
assert_lacks "Main 保護へ進まない（締めはここで止まる）" "main=on" "$(bd_log)"

echo "== Test 16: 起きている面には手を出さない（既定の道を変えない）=="
reset_bd; set_table "$VD_TABLE_17"; set_clamshell No
set_drawable "$DRAW_AWAKE"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "起きていれば 0" "$rc" "0"
assert_eq "起こす手立ては 1 度も撃たない" "$(wake_count)" "0"
assert_eq "余計な出力を出さない" "$out" ""

echo "== Test 17: status は「在る」と「置ける」を別に出す（#1160）=="
reset_bd; set_table "$VD_TABLE_17"; set_clamshell No
set_drawable "$DRAW_AWAKE"
out=$(vd_status 2>&1); rc=$?
assert_eq "起きていれば 0" "$rc" "0"
assert_has "使用可と出る" "使用可" "$out"
assert_has "眠っている面は「なし」" "眠っている面: なし" "$out"
set_drawable "$DRAW_VD_ASLEEP"
out=$(vd_status 2>&1); rc=$?
assert_eq "眠っていれば非ゼロ" "$rc" "1"
assert_has "眠っていると出る" "眠っている" "$out"
assert_has "tako から見えないと書く" "tako から見えない" "$out"
assert_has "眠っている面を名指しする" "眠っている面: tako-vd" "$out"
assert_lacks "使用可とは言わない" "使用可" "$out"

echo "== Test 18: 前後比較（--snapshot）は眠りで差分を出さない（#1160）=="
set_table "$VD_TABLE_17"; set_clamshell No
set_drawable "$DRAW_AWAKE"; snap_awake=$(vd_status_snapshot)
set_drawable "$DRAW_ASLEEP"; snap_asleep=$(vd_status_snapshot)
assert_eq "眠っただけでは構成の絵が変わらない" "$snap_awake" "$snap_asleep"
assert_lacks "スナップショットに眠りを入れない" "asleep" "$snap_awake"

echo "== Test 19: ensure の締めが面の uuid を記録する（#1697）=="
reset_bd; set_table "$VD_TABLE_17"; set_clamshell No
set_drawable "$DRAW_AWAKE"; rm -rf "$VD_RECORD_DIR"; unset STUB_UUID
out=$(vd_ensure 2>&1); rc=$?
assert_eq "記録しても ensure は 0" "$rc" "0"
assert_eq "記録は黙って行う（既定の道の出力を増やさない）" "$out" ""
assert_eq "記録は小文字の名前のファイル" "$(ls "$VD_RECORD_DIR")" "tako-vd.uuid"
assert_eq "tako-vd の displayID の uuid を小文字で残す" \
    "$(cat "$VD_RECORD_DIR/tako-vd.uuid")" "aaaaaaaa-0000-4000-8000-000000000017"
assert_eq "recorded-uuid が読み返す" "$(vd_recorded_uuid)" "aaaaaaaa-0000-4000-8000-000000000017"
# 同じ値なら書かない（冪等。mtime ではなく中身を別の値へ置いて、上書きされないことで見る）
printf '%s\n' "aaaaaaaa-0000-4000-8000-000000000017" > "$VD_RECORD_DIR/tako-vd.uuid"
touch -t 200001010000 "$VD_RECORD_DIR/tako-vd.uuid"
vd_ensure >/dev/null 2>&1
assert_eq "同じ値なら書き直さない" \
    "$(ls -l -T "$VD_RECORD_DIR/tako-vd.uuid" 2>/dev/null | awk '{print $9}')" "2000"
# 面が作り直されて uuid が変わったら書き直す
STUB_UUID="CCCCCCCC-0000-4000-8000-000000000099"
vd_ensure >/dev/null 2>&1
assert_eq "uuid が変わったら書き直す" "$(vd_recorded_uuid)" "cccccccc-0000-4000-8000-000000000099"
# uuid が読めなければ記録しないが ensure は失敗させない（面は用意できている）
rm -rf "$VD_RECORD_DIR"; STUB_UUID=""
out=$(vd_ensure 2>&1); rc=$?
assert_eq "uuid が読めなくても ensure は 0" "$rc" "0"
assert_has "読めなかったことは言う" "uuid を読めなかった" "$out"
assert_eq "読めなければ記録しない" "$(ls "$VD_RECORD_DIR" 2>/dev/null)" ""
unset STUB_UUID
# 形の崩れた記録は「無い」と同じ
mkdir -p "$VD_RECORD_DIR"; printf 'not-a-uuid\n' > "$VD_RECORD_DIR/tako-vd.uuid"
vd_recorded_uuid >/dev/null 2>&1; rc=$?
assert_eq "崩れた記録は非ゼロ" "$rc" "1"
assert_eq "uuid の形（純関数）" "$(vd_is_uuid 'aaaaaaaa-0000-4000-8000-000000000017' && echo y)" "y"
assert_eq "uuid でない形は弾く" "$(vd_is_uuid 'tako-vd' || echo n)" "n"
# 眠っていて ensure が止まる道では記録しない（締めは起きているかの確認で止まる）
reset_bd; rm -rf "$VD_RECORD_DIR"; set_drawable "$DRAW_VD_ASLEEP"
vd_ensure >/dev/null 2>&1
assert_eq "ensure が止まる道では記録しない" "$(ls "$VD_RECORD_DIR" 2>/dev/null)" ""

echo "== Test 20: status は記録済み uuid といまの面の一致を出す（#1697）=="
reset_bd; set_table "$VD_TABLE_17"; set_clamshell No; set_drawable "$DRAW_AWAKE"
rm -rf "$VD_RECORD_DIR"
assert_has "記録が無ければそう言う" "記録 uuid: なし" "$(vd_status 2>&1)"
vd_ensure >/dev/null 2>&1
out=$(vd_status 2>&1)
assert_has "記録といまの面が一致" "aaaaaaaa…（いまの面と一致）" "$out"
assert_lacks "uuid を全桁は出さない（貼り付けで広まらないように）" "000000000017" "$out"
STUB_UUID="CCCCCCCC-0000-4000-8000-000000000099"
assert_has "作り直された面とは不一致と出す" "不一致" "$(vd_status 2>&1)"
unset STUB_UUID

echo "== Test 21: 仮想ディスプレイかの判定（純関数。#1783）=="
LIST_VD=$(t '17\t13\ttako-vd')
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict tako-vd "$LIST_VD" 1); rc=$?
assert_eq "器の一覧に居る非内蔵の面は virtual（終了 0）" "$v/$rc" "virtual/0"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict "Color LCD" "$LIST_VD" 1); rc=$?
assert_eq "内蔵は physical（終了 1）" "$rc" "1"
assert_has "理由は内蔵（CoreGraphics）" "内蔵ディスプレイ（CGDisplayIsBuiltin）" "$v"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict "Color LCD" "" 0); rc=$?
assert_eq "内蔵は器の一覧を読まなくても physical" "$rc" "1"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict "Color LCD" "$(t '17\t13\tColor LCD')" 1); rc=$?
assert_eq "器に同じ名前の仮想スクリーンがあっても内蔵は内蔵" "$rc" "1"
v=$(printf '%s\n' "$FIX_EXTERNAL_MAIN" | vd_virtual_verdict "External 4K" "$LIST_VD" 1); rc=$?
assert_eq "器の一覧に無い外部モニタは physical" "$rc" "1"
assert_has "理由は器の一覧に無いこと" "仮想スクリーン一覧に External 4K が無い" "$v"
v=$(printf '%s\n' "$FIX_EXTERNAL_MAIN" | vd_virtual_verdict "External 4K" "" 0); rc=$?
assert_eq "器の一覧を読めなければ unknown（終了 2）" "$rc" "2"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict tako-vd "" 1); rc=$?
assert_eq "器が応答して仮想スクリーンが 0 枚なら physical" "$rc" "1"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict tako-vd "$(t '17\t\ttako-vd')" 1); rc=$?
assert_eq "displayID が空の行でも名前を読み違えない" "$v/$rc" "virtual/0"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict tako-vd "$(t '17\t13\ttako-vd-other')" 1); rc=$?
assert_eq "前方一致では通さない（完全一致）" "$rc" "1"
v=$(printf '%s\n' "$FIX_OK" | vd_virtual_verdict no-such "$LIST_VD" 1); rc=$?
assert_eq "OS の一覧に居ない名前は unknown" "$rc" "2"
v=$(printf '%s\n' "$FIX_DUP" | vd_virtual_verdict tako-vd "$LIST_VD" 1); rc=$?
assert_eq "枝番付き（増殖）も器の面と読む（増殖の検査は vd_assert_single の仕事）" "$v/$rc" "virtual/0"
FIX_JA=$(t '検証 用 vd\t0\t0\t2560\t1440\t21\t0\t1')
v=$(printf '%s\n' "$FIX_JA" | vd_virtual_verdict "検証 用 vd" "$(t '17\t21\t検証 用 vd')" 1); rc=$?
assert_eq "空白・日本語を含む名前でも一致で読む" "$v/$rc" "virtual/0"

echo "== Test 22: ensure は物理画面の名前で断る（#1783 の本体）=="
reset_bd; set_table "$FIX_OK"; set_clamshell No; set_drawable "$(t '1\t1\t0\n13\t1\t0')"
rm -rf "$VD_RECORD_DIR"
VD_NAME="Color LCD"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "内蔵の名前は終了コード VD_RC_NOT_VIRTUAL（3）" "$rc" "$VD_RC_NOT_VIRTUAL"
assert_eq "終了コードの値" "$VD_RC_NOT_VIRTUAL" "3"
assert_eq "理由は 1 行" "$(printf '%s\n' "$out" | grep -c .)" "1"
assert_has "理由の行は ERROR と名前と #1783" "ERROR: Color LCD は仮想ディスプレイではない" "$out"
assert_has "理由に #1783" "#1783" "$out"
assert_eq "内蔵の名前なら器に聞かずに断る" "$(ready_count)" "0"
assert_lacks "Main 保護へ進まない（物理画面を仮想と読んで main を撃たない）" "main=on" "$(bd_log)"
assert_eq "物理画面の uuid を記録しない" "$(ls "$VD_RECORD_DIR" 2>/dev/null)" ""
assert_eq "起こす手立ても撃たない" "$(wake_count)" "0"
reset_bd; set_table "$FIX_EXTERNAL_MAIN"; set_drawable "$(t '1\t1\t0\n7\t1\t0\n13\t1\t0')"
VD_NAME="External 4K"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "器の一覧に無い外部モニタも 3" "$rc" "3"
assert_has "理由は器の一覧に無いこと" "仮想スクリーン一覧に External 4K が無い" "$out"
assert_eq "物理画面の uuid を記録しない（外部）" "$(ls "$VD_RECORD_DIR" 2>/dev/null)" ""
reset_bd; STUB_BACKEND_DOWN=1
out=$(vd_ensure 2>&1); rc=$?
assert_eq "器に聞けなければ確かめられない = 1（通さない）" "$rc" "1"
assert_has "確かめられないと言う" "確かめられないので使わない" "$out"
assert_eq "確かめられないときも記録しない" "$(ls "$VD_RECORD_DIR" 2>/dev/null)" ""
reset_bd; set_table "$FIX_OK"; VD_NAME="Color LCD"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "器に聞けなくても内蔵は 3（器に頼らず決まる）" "$rc" "3"
unset STUB_BACKEND_DOWN
# 画面が 1 枚だけ（蓋閉じで tako-vd だけ）の構成は従来どおり通る
reset_bd; set_table "$(t 'tako-vd\t0\t0\t2560\t1440\t6\t0\t1')"; set_drawable "$(t '6\t1\t0')"
VD_NAME=tako-vd
out=$(vd_ensure 2>&1); rc=$?
assert_eq "tako-vd だけの 1 枚構成は 0" "$rc" "0"
assert_lacks "tako-vd では断らない" "仮想ディスプレイではない" "$out"
# 画面が内蔵 1 枚だけでその名前を渡されたら 3
reset_bd; set_table "$(t 'Color LCD\t0\t0\t1512\t982\t1\t1\t1')"; set_drawable "$(t '1\t1\t0')"
VD_NAME="Color LCD"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "内蔵 1 枚だけの機でその名前は 3" "$rc" "3"
# 器が作った空白・日本語の名前は通る（TAKO_VD_NAME の上書き自体は壊さない）
reset_bd; set_table "$FIX_JA"; set_drawable "$(t '21\t1\t0')"; rm -rf "$VD_RECORD_DIR"
STUB_BACKEND_LIST=$(t '17\t21\t検証 用 vd')
VD_NAME="検証 用 vd"
out=$(vd_ensure 2>&1); rc=$?
assert_eq "器が作った空白・日本語の名前は 0" "$rc" "0"
assert_eq "その面の uuid は記録する" "$(ls "$VD_RECORD_DIR" 2>/dev/null)" "検証 用 vd.uuid"
unset STUB_BACKEND_LIST
VD_NAME=tako-vd
rm -rf "$VD_RECORD_DIR"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
