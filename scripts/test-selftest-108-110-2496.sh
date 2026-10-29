#!/usr/bin/env bash
# #2496: 隔離セルフテストの項目 108 / 110（#786 / #803 の再描画の判定）が、#2490 と同じ形で
# 間欠的に落ちうる件の A/B。
#
# 両項目は測り方の 1 実装 measure_output_redraw で「要求するなら即座に要求する」状態で出力を流し、
# 描き直した枚数を数える。修正前はフォーカス中の 16ms の時計（last_term_notify）だけを巻き戻し、
# #1979 のフォーカスの無いペインの上限の時計（last_unfocused_flush）を巻き戻していなかった。
# 対象は「最初に描かれた端末ペイン」。いまの項目の並びでは項目 91（#666）が残す 2 ペインの先頭 =
# フォーカス中のペインなので 16ms の経路を通り、上限の時計は効かない（= 実際に落ちた例は無い）。
# ただしそれは並び次第で、対象がフォーカスの無いペインになると、直前の 1 間隔（既定 33ms）以内に
# フォーカスの無いペインの再描画が流れた回だけ出力が予約へ回って本体が描き直らず（body=+0）、
# 「出力のあったペイン（本体）は描き直される」で落ちる。
#
# 配置と混み具合は注入で作る（CPU を焼く負荷は作らない）:
#   TAKO_2496_INJECT=clock[:108|:110]    窓を空ける直前に上限の時計を「いま流した」にする
#   TAKO_2496_INJECT=unfocus[:108|:110]  測る前に同じタブの別のペインへフォーカスを移す（対象がフォーカスの無いペイン）
#   TAKO_2496_INJECT=focus[:108|:110]    測る前に対象のペインへフォーカスを移す（対象がフォーカス中の境界）
#
# 腕（TAKO_2496_RUNS。空白区切り。既定 `legacy-108 legacy-110 clock focus nolimit ok`）:
#   legacy-108  旧（TAKO_2496_LEGACY=1）+ clock:108 + unfocus:108: 項目 108 の「出力のあったペインは描き直される」で FAILED
#   legacy-110  旧 + clock:110 + unfocus:110: 項目 108 は通り、項目 110 の「出力のあったペイン本体は描き直される」で FAILED
#   clock       新 + clock + unfocus: 項目 108 / 110 を通る
#   focus       旧 + clock + focus: 対象がフォーカス中なら 16ms の経路を通るので旧でも通る
#   nolimit     旧 + clock + unfocus + TAKO_1979_LEGACY=1（上限なし）: 上限が効かないので旧でも通る
#   ok          新・注入なし: 項目 107 / 108 / 110 を通る（回数は TAKO_2496_OK_RUNS。既定 4）
#   full        新・注入なし: 最後まで回して TAKO_APP_SELF_TEST_OK
# full 以外は項目 110 の判定が済んだら（項目 111 の行が出たら）その回を止める。
# `TAKO_2496_KEEP_TMP=1` はログ（腕ごとの app.log）を残して置き場を出す。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは
# このスクリプトが起こした pid と、その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-selftest-108-110-2496.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2496_RUNS:-legacy-108 legacy-110 clock focus nolimit ok}"
OK_RUNS="${TAKO_2496_OK_RUNS:-4}"
# 1 本の上限（項目 110 までは数分。最後まで回す full は 5 分前後。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2496_RUN_LIMIT_SECS:-1500}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
# 項目 14 がカレントディレクトリで `cargo build -p tako-cli` を回す = リポジトリの根から起こす
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# `TAKO_TMUX_SOCKET` も落とす = GUI に `tako-iso-<pid>` を自分で選ばせる（後片付けで自分の分だけ落とす）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2496-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2496_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 項目 111 に入った印（= 項目 110 の判定は済んだ）
ITEM111='TAKO_SELF_TEST_813_'

# 項目 110 の判定が済んだ（= 項目 111 に入った）か、セルフテストが終わったか
item110_settled() {
  grep -qE "${ITEM111}|TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG"
}

# 項目 111 に入る前（= 項目 110 まで）の行。止めた回は止めたこと自体で #872 の
# 「最終項目より前に quit した」が FAILED として出るので、判定はこの範囲だけで見る
upto_111() { awk -v pat="$ITEM111" '$0 ~ pat {exit} {print}' "$RUN_LOG"; }

# 1 回分: TMPDIR を回ごとに分けてセルフテストを回す
#   run_one <ラベル> <full か stop> [VAR=VAL ...]  → RUN_RC / RUN_LOG を残す
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" mode="$2" attempt i stopped
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    local dir="$TMP/$RUN_SEQ-$label-$attempt"
    RUN_LOG="$dir/app.log"
    mkdir -p "$dir/tmpdir"
    launch_isolated_gui "$RUN_LOG" TAKO_SELF_TEST=1 TMPDIR="$dir/tmpdir/" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$APP_PID"
    stopped=""
    for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
      kill -0 "$APP_PID" 2>/dev/null || break
      if [ "$mode" = stop ] && item110_settled; then
        stopped=1
        break
      fi
      sleep 0.1
    done
    RUN_RC=0
    if [ -n "$stopped" ]; then
      stop_isolated_gui "$APP_PID"
      RUN_RC=stopped
    elif kill -0 "$APP_PID" 2>/dev/null; then
      echo "    [$label] ${RUN_LIMIT_SECS} 秒で終わらなかったので止める"
      stop_isolated_gui "$APP_PID"
      RUN_RC=124
    else
      wait "$APP_PID" 2>/dev/null
      RUN_RC=$?
    fi
    tmux_socket_kill "tako-iso-$APP_PID"
    APP_PID=""
    ISOLATED_GUI_PID=""
    [ "$RUN_RC" != "$ISOLATED_GUI_RC_NO_DISPLAY" ] && break
    echo "    [$label] 置き先に断られて窓を開かずに終わった（終了コード ${RUN_RC}）"
    if [ "$attempt" -eq 2 ]; then
      echo "未実測: tako-vd へ窓を開けない（蓋閉じ・ディスプレイスリープ。scripts/lib/virtual-display.sh status）"
      exit "$ISOLATED_GUI_RC_NO_DISPLAY"
    fi
  done
  echo "    [$label] rc=$RUN_RC"
  upto_111 | grep -E "TAKO_SELF_TEST_(2490: osc|786|803)|SKIPPED: 10[78]|SKIPPED: 110|TAKO_APP_SELF_TEST_FAILED" \
    | cut -c1-400 | sed 's/^/      /'
  if [ "$mode" = full ]; then
    grep -E "^TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG" | cut -c1-240 | sed 's/^/      /'
  fi
}

failed_with() { grep -q "TAKO_APP_SELF_TEST_FAILED: $1" "$RUN_LOG" && echo 1; }
failed_any() { grep -q "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" && echo 1; }
# 項目 107 / 108 / 110 を通った = 3 つの判定の行が出ていて、項目 111 より前に FAILED が無い
# （SKIPPED で判定の行が出なかった回は通ったと数えない）
passed_upto_110() {
  local head
  head="$(upto_111)"
  grep -q "TAKO_SELF_TEST_2490: osc req=" <<<"$head" || return 0
  grep -q "TAKO_SELF_TEST_786: " <<<"$head" || return 0
  grep -q "TAKO_SELF_TEST_803: " <<<"$head" || return 0
  grep -q "TAKO_APP_SELF_TEST_FAILED" <<<"$head" && return 0
  echo 1
}
# 測った窓の行（`TAKO_SELF_TEST_786_WINDOW` / `_803_WINDOW`）に、対象がフォーカスの無いペインで
# 上限の時計が閉じていた（注入どおり）試行がある
window_unfocused_closed() {
  grep -E "TAKO_SELF_TEST_$1_WINDOW:" "$RUN_LOG" \
    | grep -E "focused=false unfocused_ago_ms=([0-9]|[12][0-9]|3[0-2]) " >/dev/null && echo 1
}
window_focused() {
  grep -E "TAKO_SELF_TEST_$1_WINDOW:" "$RUN_LOG" | grep -E "focused=true " >/dev/null && echo 1
}
not() { [ -z "$1" ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    legacy-108)
      echo "--- 旧（TAKO_2496_LEGACY=1）+ clock:108 + unfocus:108"
      run_one "$run" stop TAKO_2496_LEGACY=1 TAKO_2496_INJECT=clock:108,unfocus:108
      check "$(failed_with "108: 出力のあったペインは描き直される")" \
        "旧 + clock:108: 項目 108 の「出力のあったペインは描き直される」で FAILED = 修正前の再現"
      check "$(grep -q "TAKO_SELF_TEST_786: drawn=[0-9]* output=(panes +0 " "$RUN_LOG" && echo 1)" \
        "旧 + clock:108: 出力のあったペインの本体が描き直っていない（panes +0 = 予約へ回った）"
      check "$(window_unfocused_closed 786)" "旧 + clock:108: 対象はフォーカスの無いペインで、上限の窓が閉じていた"
      ;;
    legacy-110)
      echo "--- 旧（TAKO_2496_LEGACY=1）+ clock:110 + unfocus:110"
      run_one "$run" stop TAKO_2496_LEGACY=1 TAKO_2496_INJECT=clock:110,unfocus:110
      check "$(grep -q "TAKO_SELF_TEST_786: " "$RUN_LOG" && not "$(failed_with "108")")" \
        "旧 + clock:110: 項目 108 は注入していないので通る"
      check "$(failed_with "110: 出力のあったペイン本体は描き直される")" \
        "旧 + clock:110: 項目 110 の「出力のあったペイン本体は描き直される」で FAILED = 修正前の再現"
      check "$(grep -q "TAKO_SELF_TEST_803: output=(body +0 " "$RUN_LOG" && echo 1)" \
        "旧 + clock:110: 出力のあったペインの本体が描き直っていない（body +0 = 予約へ回った）"
      check "$(window_unfocused_closed 803)" "旧 + clock:110: 対象はフォーカスの無いペインで、上限の窓が閉じていた"
      ;;
    clock)
      echo "--- 新 + clock + unfocus"
      run_one "$run" stop TAKO_2496_INJECT=clock,unfocus
      check "$(passed_upto_110)" "新 + clock: 項目 107 / 108 / 110 を通る"
      check "$(window_unfocused_closed 786)" "新 + clock: 108 の対象はフォーカスの無いペインで、注入で上限の窓が閉じていた"
      check "$(window_unfocused_closed 803)" "新 + clock: 110 の対象はフォーカスの無いペインで、注入で上限の窓が閉じていた"
      ;;
    focus)
      echo "--- 旧 + clock + focus（対象がフォーカス中）"
      run_one "$run" stop TAKO_2496_LEGACY=1 TAKO_2496_INJECT=clock,focus
      check "$(passed_upto_110)" "旧 + clock + focus: 対象がフォーカス中なら 16ms の経路なので旧でも通る"
      check "$(window_focused 786)" "旧 + clock + focus: 108 の対象はフォーカス中"
      check "$(window_focused 803)" "旧 + clock + focus: 110 の対象はフォーカス中"
      ;;
    nolimit)
      echo "--- 旧 + clock + unfocus + 上限なし（TAKO_1979_LEGACY=1）"
      run_one "$run" stop TAKO_2496_LEGACY=1 TAKO_2496_INJECT=clock,unfocus TAKO_1979_LEGACY=1
      check "$(passed_upto_110)" "上限なし: 上限の時計が効かないので旧でも項目 108 / 110 を通る"
      ;;
    ok)
      i=1
      while [ "$i" -le "$OK_RUNS" ]; do
        echo "--- 新・注入なし（${i}/${OK_RUNS}）"
        run_one "ok$i" stop
        check "$(passed_upto_110)" "新・注入なし ${i}/${OK_RUNS}: 項目 107 / 108 / 110 を通る"
        i=$((i + 1))
      done
      ;;
    full)
      echo "--- 新・注入なし（最後まで）"
      run_one full full
      check "$(grep -q "^TAKO_APP_SELF_TEST_OK" "$RUN_LOG" && echo 1)" "最後まで: TAKO_APP_SELF_TEST_OK"
      check "$(not "$(failed_any)")" "最後まで: FAILED が無い"
      ;;
    *)
      echo "知らない腕: ${run}（legacy-108 / legacy-110 / clock / focus / nolimit / ok / full）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
