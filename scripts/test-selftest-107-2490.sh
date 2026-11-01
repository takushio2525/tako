#!/usr/bin/env bash
# #2490: 隔離セルフテストの項目 107（#782 の再描画の判定）が間欠的に落ちていた件の A/B。
#
# 判定は「要求するなら即座に要求する」状態で出力を流し、その場の要求数を数える。修正前は
# フォーカス中の 16ms の時計（last_term_notify）だけを巻き戻し、#1979 のフォーカスの無いペインの
# 上限の時計（last_unfocused_flush）を巻き戻していなかった。直前の 1 間隔（既定 33ms）以内に
# フォーカスの無いペインの再描画が流れた回だけ、裏タブの OSC が予約へ回って要求数 0 で落ちる。
#
# 混み具合は再現できないので、その回を注入で作る（CPU を焼く負荷は作らない）:
#   TAKO_2490_INJECT=clock      判定の直前に上限の時計を「いま流した」にする
#   TAKO_2490_INJECT=piggyback  表示中ペインへ流す前にフォーカスの無いペインの再描画を 1 件溜める
#                               （表示中ペインの再描画が相乗りで流して時計が進む = FR-4.11.1 の順序）
#
# 腕（TAKO_2490_RUNS。空白区切り。既定 `legacy-clock legacy-piggyback clock piggyback nolimit ok`）:
#   legacy-clock / legacy-piggyback  旧（TAKO_2490_LEGACY=1）+ 注入: 項目 107 の
#                                    「裏タブでも OSC 由来の状態変化は再描画する」で FAILED = 修正前の再現
#   clock / piggyback                新 + 注入: 項目 107 を通る（上限の窓をまたぐ予約の判定 2 本も通る）
#   nolimit                          旧 + clock + TAKO_1979_LEGACY=1（上限なし）: 上限が効かないので旧でも通る
#   ok                               新・注入なし: 項目 107 を通る（回数は TAKO_2490_OK_RUNS。既定 4）
#   full                             新・注入なし: 最後まで回して TAKO_APP_SELF_TEST_OK
# full 以外は項目 107 の判定が済んだら（項目 108 の行が出たら）その回を止める。
# `TAKO_2490_KEEP_TMP=1` はログ（腕ごとの app.log）を残して置き場を出す。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは
# このスクリプトが起こした pid と、その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-selftest-107-2490.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2490_RUNS:-legacy-clock legacy-piggyback clock piggyback nolimit ok}"
OK_RUNS="${TAKO_2490_OK_RUNS:-4}"
# 1 本の上限（項目 107 までは数分。最後まで回す full は 5 分前後。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2490_RUN_LIMIT_SECS:-1500}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
# 起動の cwd はリポジトリの根に揃える（項目 14 の `cargo build -p tako-cli` は #1556 から根を実行ファイルの位置で辿るので、cwd は前提ではない）
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# `TAKO_TMUX_SOCKET` も落とす = GUI に `tako-iso-<pid>` を自分で選ばせる（後片付けで自分の分だけ落とす）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2490-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2490_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 項目 108 に入った印（= 項目 107 の判定は済んだ）
ITEM108='TAKO_SELF_TEST_786|TAKO_SELF_TEST_SKIPPED: 108'

# 項目 107 の判定が済んだ（= 項目 108 に入った）か、セルフテストが終わったか
item107_settled() {
  grep -qE "${ITEM108}|TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG"
}

# 項目 108 に入る前（= 項目 107 まで）の行。止めた回は止めたこと自体で #872 の
# 「最終項目より前に quit した」が FAILED として出るので、判定はこの範囲だけで見る
upto_108() { awk -v pat="$ITEM108" '$0 ~ pat {exit} {print}' "$RUN_LOG"; }

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
      if [ "$mode" = stop ] && item107_settled; then
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
  upto_108 | grep -E "TAKO_SELF_TEST_782|TAKO_SELF_TEST_2490|SKIPPED: 107|TAKO_APP_SELF_TEST_FAILED" \
    | cut -c1-240 | sed 's/^/      /'
  if [ "$mode" = full ]; then
    grep -E "^TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG" | cut -c1-240 | sed 's/^/      /'
  fi
}

failed_osc() { grep -q "TAKO_APP_SELF_TEST_FAILED: 107: 裏タブでも OSC" "$RUN_LOG" && echo 1; }
failed_any() { grep -q "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" && echo 1; }
# 項目 107 を通った = 108 に入った（または最後まで通った）・それより前に FAILED が無い
passed_107() {
  local head
  grep -qE "${ITEM108}|TAKO_APP_SELF_TEST_OK" "$RUN_LOG" || return 0
  head="$(upto_108)"
  grep -q "TAKO_APP_SELF_TEST_FAILED" <<<"$head" && return 0
  echo 1
}
deferred_ok() { grep -q "TAKO_SELF_TEST_2490: deferred req=0 reserved=true delivered=true" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    legacy-clock|legacy-piggyback)
      inject=${run#legacy-}
      echo "--- 旧（TAKO_2490_LEGACY=1）+ 注入 ${inject}"
      run_one "$run" stop TAKO_2490_LEGACY=1 TAKO_2490_INJECT="$inject"
      check "$(failed_osc)" "旧 + ${inject}: 項目 107 の「裏タブでも OSC 由来の状態変化は再描画する」で FAILED = 修正前の再現"
      check "$(grep -q "TAKO_SELF_TEST_2490: osc req=0 " "$RUN_LOG" && echo 1)" \
        "旧 + ${inject}: その場の要求数は 0（予約へ回った）"
      ;;
    clock|piggyback)
      echo "--- 新 + 注入 ${run}"
      run_one "$run" stop TAKO_2490_INJECT="$run"
      check "$(passed_107)" "新 + ${run}: 項目 107 を通る"
      check "$(deferred_ok)" "新 + ${run}: 上限の窓が閉じている間の OSC は予約へ回り、次の間隔で流れる"
      ;;
    nolimit)
      echo "--- 旧 + clock + 上限なし（TAKO_1979_LEGACY=1）"
      run_one "$run" stop TAKO_2490_LEGACY=1 TAKO_2490_INJECT=clock TAKO_1979_LEGACY=1
      check "$(passed_107)" "上限なし: 上限の時計が効かないので旧でも項目 107 を通る"
      check "$(grep -q "SKIPPED: 107 の予約の判定" "$RUN_LOG" && echo 1)" "上限なし: 予約の判定は飛ばす"
      ;;
    ok)
      i=1
      while [ "$i" -le "$OK_RUNS" ]; do
        echo "--- 新・注入なし（${i}/${OK_RUNS}）"
        run_one "ok$i" stop
        check "$(passed_107)" "新・注入なし ${i}/${OK_RUNS}: 項目 107 を通る"
        check "$(deferred_ok)" "新・注入なし ${i}/${OK_RUNS}: 予約の判定も通る"
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
      echo "知らない腕: ${run}（legacy-clock / legacy-piggyback / clock / piggyback / nolimit / ok / full）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
