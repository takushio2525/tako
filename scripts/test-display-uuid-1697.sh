#!/usr/bin/env bash
# test-display-uuid-1697.sh — 面の名前が読めない瞬間に隔離 GUI がユーザーの画面へ落ちないことの
# 実経路テスト（#1697）
#
# tako:run: bash scripts/test-display-uuid-1697.sh
#
# 実 tako-app（隔離）を常設の仮想ディスプレイ（tako-vd）上に立て、persist.log の解決行と
# `tako check-health` の display_placement で次を実測する:
#   ① 名前が読めない瞬間（TAKO_1697_INJECT_NO_NAMES=1 = 全部の面を name=? にする）でも、
#      ensure が残した uuid で tako-vd へ解決して開く（matched_by=recorded_uuid）
#   ② 値が uuid の指定はそのまま uuid で解決する（matched_by=uuid）
#   ③ TAKO_DISPLAY 未指定（検証用の暗黙の既定）は従来どおり名前で解決する（explicit=false）
#   ④ 明示の指定がどの面にも当たらなければ窓を開かずに終了 4（理由と候補を stderr へ）
#
# ④ の「当たらない面」は **実在し得ない index（index:999）で指す**（#1760）。ヘルパは
# tako-vd 以外の面の明示を通さない（当たらないはずの名前も、その名前の面が無いことを
# 確かめられないので通さない）。index:999 は tako の当て方（uuid → 名前 → 記録 uuid →
# index）のどれにも当たらず、以前の no-such-display-1697 と同じ「明示の見失い」の道を通る
#
# **旧挙動（ユーザーの画面へ落ちる）は実 GUI で再現しない**。それは単体の番犬
# （crates/tako-core/src/platform/display.rs の #1697 節・TAKO_1697_LEGACY=1 の A/B）で固定する。
# ④ は壊れていればユーザーの画面へ窓が出る道なので、①〜③ で新しいバイナリが解決の口を
# 持つことを確かめてから最後に踏む（①〜③ のどれかが落ちたら ④ は踏まない）。
#
# tako-vd が使えない（蓋閉じで眠ったまま起こせない・未配線）なら何も起動せず終了 4
# （= 未実測。検証を「通った」と読ませない。番号は isolated-gui.sh の
# ISOLATED_GUI_RC_NO_DISPLAY にそろえる = #1744）。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（tako のペインから走らせると継承され、
# CLI が隔離インスタンスではなくユーザーの本番 GUI を触る）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_DISPLAY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
VD="$REPO_ROOT/scripts/lib/virtual-display.sh"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
check_contains() {
    case "$2" in
        *"$3"*) pass "$1" ;;
        *) fail "$1（'${3}' を含まない: ${2}）" ;;
    esac
}

# `/tmp` 直下の短いパス（IPC ソケットと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1697-XXXXXX)"
TMUX_SOCKET="tako-1697-$$"
APP_PID=""
cleanup() {
    # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
    stop_isolated_gui "$APP_PID"
    # ソケット名を明示した起動は自分で畳む（#1192）
    tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
    rm -rf "$TMP"
}
trap cleanup EXIT

isolated_gui_bins || exit 1

# --- 隔離した環境（data dir だけでは CLI が隔離 GUI を見つけられず、本番 GUI へ向かいうる）---
# HOME も一時 dir へ倒すので、面の uuid の記録（~/Library/Caches/tako/virtual-display/）も
# この中で閉じる: launch_isolated_gui の ensure がここへ書き、tako もここから読む
export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$HOME" "$TAKO_ORCHESTRATOR_DIR"

# check-health は重い（#1505 の実測で 10 秒前後）ので、ケースごとに 1 回だけ引いて使い回す
HEALTH=""
read_health() { HEALTH=$("$TAKO_BIN" check-health --json 2>/dev/null); }

# display_placement の 1 項目（直前の read_health の JSON から。無ければ空）
placement() {
    printf '%s' "$HEALTH" | /usr/bin/python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)["display_placement"]
    for k in sys.argv[1].split("."):
        v = v[k]
    print("" if v is None else (str(v).lower() if isinstance(v, bool) else v))
except Exception:
    print("")
' "$1"
}

# 1 ケース分の起動（data dir をケースごとに分ける）。起動に失敗したら非ゼロ
launch_case() {
    local name=$1
    shift
    export TAKO_DATA_DIR="$TMP/$name"
    export TAKO_DISCOVERY_DIR="$TMP/$name-disc"
    mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR"
    local d
    for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
        case "$d" in
            "$TMP"/*) : ;;
            *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
        esac
    done
    launch_isolated_gui "$TMP/$name.log" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
}

echo "== 前提: 常設の仮想ディスプレイが使える =="
# 眠っているだけなら起こしてから判定する（起こすのはヘルパの 1 実装。起動ごとの ensure も
# launch_isolated_gui が通し、用意できなければ起動しない = #1744）。ここで先に止めるのは
# 面の状態（status の 1 行）を出してから未実測で終わるため
iso_ensure_display >/dev/null 2>&1 || true
if ! status=$(bash "$VD" status 2>&1); then
    echo "$status"
    echo "未実測: ${ISOLATED_GUI_DISPLAY} が使える状態ではない（眠っている / 未配線）。何も起動しない"
    exit "$ISOLATED_GUI_RC_NO_DISPLAY"
fi
echo "$status" | sed 's/^/  /'

echo "== ①: 名前が読めない瞬間でも記録済み uuid で ${ISOLATED_GUI_DISPLAY} へ解決する =="
launch_case names TAKO_1697_INJECT_NO_NAMES=1 || exit 1
if ! wait_isolated_gui "$TMP/names.log"; then
    fail "① の GUI が立たない"
    exit 1
fi
read_health
RECORDED=$(bash "$VD" recorded-uuid 2>/dev/null)
check_eq "ensure が uuid を記録している" "36" "${#RECORDED}"
check_eq "記録は隔離した HOME の中にある" "1" \
    "$(ls "$HOME/Library/Caches/tako/virtual-display/" 2>/dev/null | grep -c '\.uuid$')"
line=$(grep "ディスプレイ指定" "$TAKO_DATA_DIR/persist.log" | tail -1)
echo "  解決行: ${line}"
check_contains "persist.log の解決行が recorded_uuid で解決している" "$line" "recorded_uuid で解決"
check_contains "名前が読めない一覧を再現できている（name=?）" "$line" "name=?"
check_eq "check-health の matched_by" "recorded_uuid" "$(placement matched_by)"
resolved=$(placement resolved.uuid | tr '[:upper:]' '[:lower:]')
check_eq "解決した面の uuid が記録と一致" "$RECORDED" "$resolved"
check_eq "明示の指定として記録される" "true" "$(placement explicit)"
check_eq "拒否していない" "false" "$(placement refused)"
stop_isolated_gui "$APP_PID"; APP_PID=""

echo "== ②: 値が uuid の指定はそのまま uuid で解決する =="
launch_case uuid "TAKO_DISPLAY=$RECORDED" || exit 1
if wait_isolated_gui "$TMP/uuid.log"; then
    read_health
    check_eq "check-health の matched_by" "uuid" "$(placement matched_by)"
    check_eq "解決した面の uuid" "$RECORDED" "$(placement resolved.uuid | tr '[:upper:]' '[:lower:]')"
else
    fail "② の GUI が立たない"
fi
stop_isolated_gui "$APP_PID"; APP_PID=""

echo "== ③: TAKO_DISPLAY 未指定（暗黙の既定）は従来どおり名前で解決する =="
# 空の TAKO_DISPLAY は tako 側で未指定と同じ（explicit_request）。ヘルパの明示を打ち消す
launch_case implicit "TAKO_DISPLAY=" || exit 1
if wait_isolated_gui "$TMP/implicit.log"; then
    read_health
    check_eq "暗黙の既定として記録される" "false" "$(placement explicit)"
    check_eq "狙いは既定名" "$ISOLATED_GUI_DISPLAY" "$(placement requested)"
    check_eq "名前で解決する（回帰なし）" "name" "$(placement matched_by)"
else
    fail "③ の GUI が立たない"
fi
stop_isolated_gui "$APP_PID"; APP_PID=""

if [ "$FAIL" -gt 0 ]; then
    echo "①〜③ が落ちたので ④（壊れていればユーザーの画面へ窓が出る道）は踏まない"
    echo "PASS=${PASS} FAIL=${FAIL}"
    exit 1
fi

echo "== ④: 明示の指定がどの面にも当たらなければ窓を開かずに終了 4 =="
# 実在し得ない index で「当たらない面」を指す（当たらない面としてヘルパが通すのはこの形だけ = #1760）
launch_case miss "TAKO_DISPLAY=index:999" || exit 1
pid="$APP_PID"
# 終了を待つ（状態待ち。窓を開かない道なので IPC は立たない）
for _ in $(seq 1 100); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then
    fail "④ が終了しない（窓を開いた疑い）"
    stop_isolated_gui "$pid"
else
    wait "$pid"; rc=$?
    ISOLATED_GUI_PID=""
    check_eq "終了コードは 4（REFUSED_EXIT_CODE）" "4" "$rc"
    err=$(cat "$TMP/miss.log")
    check_contains "狙った指定がそのまま tako へ届いている" "$err" "置き先 index:999"
    check_contains "stderr に開かなかったことを出す" "$err" "窓を開かずに終了した"
    check_contains "stderr に見失った理由を出す" "$err" "どの面にも当たらない"
    check_contains "stderr に候補を出す" "$err" "[0] id="
    line=$(grep "ディスプレイ指定" "$TAKO_DATA_DIR/persist.log" | tail -1)
    check_contains "persist.log に開かなかった記録" "$line" "窓を開かずに終了する"
fi
APP_PID=""

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
