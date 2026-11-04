#!/usr/bin/env bash
# #2727: 隔離セルフテストの項目 115（#830 のチャットの仮想化）が、混んだ機で `display=Starter` のまま
# 描画待ちの上限で切れる間欠の A/B。
#
# 項目 115 は素のシェルのペインへ 81 発話の会話を `chat_panes` へ差し込み、チャットとして描けるのを待つ。
# 修正前は差し込んだ会話を `pin_chat_fixture`（#853 の守り）で守っていなかった。待っている間に 2 秒 tick の
# 定期更新が挟まると、実 claude が動いていないペインは「チャットではない」と読まれ、`apply_chat_refresh` が
# 会話を消す。表示はスターター（素のシェル）へ戻り、描画待ちが上限で切れる（tick が挟まるかは運 = 間欠）。
# 修正後は差し込んだ update の中で pin し、定期更新をその場で 1 周回して「会話が残る」を確かめる（項目 98 と
# 同じ形）。1 周をその場で回すので、旧の腕（TAKO_2727_LEGACY=1）は時間の注入なしで決定的に落ちる。
#
# 腕（TAKO_2727_RUNS。空白区切り。既定 `legacy ok full`）:
#   legacy  旧（TAKO_2727_LEGACY=1 = pin しない）: 1 周で会話が消え、項目 115 の「チャット本文が描かれる」で
#           FAILED（`targets_include_fixture=true kept=false`・`chat_pane=false pinned=false`）= 修正前の再現
#   ok      新: 1 周しても会話が残り（`targets_include_fixture=false kept=true`）、項目 115 を通る
#           （回数は TAKO_2727_OK_RUNS。既定 3）
#   full    新: 最後まで回して TAKO_APP_SELF_TEST_OK
# full 以外は項目 115 の判定が済んだら（項目 116 の行が出たら）その回を止める。どの回も手前の #739 の項目を
# 通るので、選ぶ直前のペインの様子（`TAKO_SELF_TEST_739_LAUNCH` の pane_before）も出す（#2727 段 1）。
# `TAKO_2727_KEEP_TMP=1` はログ（腕ごとの app.log）を残して置き場を出す。
#
# 手前の項目 739 / 749 は器の競合（#2741 = 器の最後のセッションを閉じた直後に作ったペインが即死する）で
# 間欠的に落ち、その回は項目 115 まで届かない。その回は NG にも未実測にも数えずに `[#2741]` と名指し、
# 腕ごとに TAKO_2727_MAX_TRIES 回（既定 3）までやり直す（やり直し切れなければ未実測）。
#
# セルフテスト全体は項目 109（#496）を通る。#2726（PR #2738）以後は偽の claude / codex / agy で確かめるので、
# 本物のエージェントは起きない。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。腕ごとに「ディスプレイ指定 tako-vd: name で解決」を確かめ、無い回は未実測として
# 数えない。セルフテストの GUI は persist.log へ書かない（`persist_diag` が TAKO_SELF_TEST で戻る）ので、同じ文
# （`placement.log_line()`）を出す app.log の `TAKO_SELF_TEST_DISPLAY:` の行で見る。本番の tako（GUI / 設定 /
# tmux）には触らない。止める・消すのは、このスクリプトが起こした pid と、その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-selftest-115-2727.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2727_RUNS:-legacy ok full}"
OK_RUNS="${TAKO_2727_OK_RUNS:-3}"
# 1 本の上限（項目 115 までは数分。最後まで回す full は 10 分前後。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2727_RUN_LIMIT_SECS:-2400}"
MAX_TRIES="${TAKO_2727_MAX_TRIES:-3}"

PASS=0
FAIL=0
UNMEASURED=0
N2741=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# `TAKO_TMUX_SOCKET` も落とす = GUI に `tako-iso-<pid>` を自分で選ばせる（後片付けで自分の分だけ落とす）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2727-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2727_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 項目 116 に入った印（= 項目 115 の判定は済んだ）
ITEM116='TAKO_SELF_TEST_835'

item115_settled() {
  grep -qE "${ITEM116}|TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG"
}

# 項目 116 に入る前（= 項目 115 まで）の行。止めた回は止めたこと自体で #872 の
# 「最終項目より前に quit した」が FAILED として出るので、判定はこの範囲だけで見る
upto_116() { awk -v pat="$ITEM116" '$0 ~ pat {exit} {print}' "$RUN_LOG"; }

# 1 回分: TMPDIR を回ごとに分けてセルフテストを回す
#   run_one <ラベル> <full か stop> [VAR=VAL ...]  → RUN_RC / RUN_LOG / RUN_DRAWN を残す
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" mode="$2" attempt i stopped dir display
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    dir="$TMP/$RUN_SEQ-$label-$attempt"
    RUN_LOG="$dir/app.log"
    mkdir -p "$dir/tmpdir"
    launch_isolated_gui "$RUN_LOG" TAKO_SELF_TEST=1 TMPDIR="$dir/tmpdir/" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$APP_PID"
    stopped=""
    for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
      kill -0 "$APP_PID" 2>/dev/null || break
      if [ "$mode" = stop ] && item115_settled; then
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
  # 描けたか: 面の解決の行（persist.log と同じ文）が tako-vd の名前で解決している
  RUN_DRAWN=""
  display="$(grep -m1 "^TAKO_SELF_TEST_DISPLAY: " "$RUN_LOG" | cut -c1-160)"
  if grep -q "^TAKO_SELF_TEST_DISPLAY: ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決" "$RUN_LOG"; then
    RUN_DRAWN=1
  fi
  echo "    [$label] rc=$RUN_RC drawn=${RUN_DRAWN:-0} ${display}"
  upto_116 | grep -E "TAKO_SELF_TEST_739_LAUNCH|TAKO_SELF_TEST_2727_115|TAKO_SELF_TEST_830_NOTDRAWN|SKIPPED: 115|TAKO_APP_SELF_TEST_FAILED" \
    | cut -c1-600 | sed 's/^/      /'
  if [ "$mode" = full ]; then
    grep -E "^TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG" | cut -c1-240 | sed 's/^/      /'
  fi
}

# 描けなかった回は未実測（数えない）。描けた回だけ判定を数える
measured() {
  if [ -n "$RUN_DRAWN" ]; then return 0; fi
  UNMEASURED=$((UNMEASURED + 1))
  echo "  [未実測] ${1}: 「ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決」が無い"
  return 1
}

failed_with() { grep -q "TAKO_APP_SELF_TEST_FAILED: $1" "$RUN_LOG" && echo 1; }
failed_any() { grep -q "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" && echo 1; }
line2727() { upto_116 | grep "^TAKO_SELF_TEST_2727_115: " | tail -1; }
has() { grep -q -- "$1" <<<"$(line2727)" && echo 1; }
# 項目 116 より前の行に一致があるか（pipefail の下で `upto_116 | grep -q` と書かない = 早く抜けた grep が
# 読み手を失った awk を SIGPIPE で落とし、判定が偽になる）
in_head() { grep -q -- "$1" <<<"$(upto_116)" && echo 1; }
# 項目 115 まで届いた = 1 周の行が出ている（手前の範囲外の項目で止まった回は数えない）
reached_115() { [ -n "$(line2727)" ] && echo 1; }
# 項目 115 を通った = 1 周の行が出ていて、項目 116 より前に FAILED が無い
passed_115() {
  local head
  head="$(upto_116)"
  grep -q "^TAKO_SELF_TEST_2727_115: " <<<"$head" || return 0
  grep -q "TAKO_APP_SELF_TEST_FAILED" <<<"$head" && return 0
  echo 1
}
not() { [ -z "$1" ] && echo 1; }
# 手前の項目 739 / 749 が器の競合（#2741）の形で落ちた回か（落ちた項目の番号を出す）。
#   749: 「検証用ペインが消えた」
#   739: 選ぶ処理が偽（`selected=false` = 選んだ時点でそのペインのシェルが無い。`pane_before` の欄で
#        シェルとツリーの有無を見る）
stopped_by_2741() {
  local head
  head="$(upto_116)"
  if grep -q "TAKO_APP_SELF_TEST_FAILED: #749: 検証用ペインが消えた" <<<"$head"; then
    echo 749
  elif grep -q "TAKO_APP_SELF_TEST_FAILED: .*(#739)" <<<"$head" \
    && grep -q "^TAKO_SELF_TEST_739_LAUNCH: .*selected=false" <<<"$head"; then
    echo 739
  fi
}

# 1 つの腕を回す: run_arm <ラベル> <full か stop> [VAR=VAL ...]
# 項目 115 まで届いた回で 0 を返す。手前の 739 / 749 が #2741 で落ちた回は NG に数えずに名指して
# MAX_TRIES 回までやり直す。描けなかった回・別の理由で手前で止まった回・やり直し切れなかった回は未実測で 1
run_arm() {
  local label="$1" mode="$2" try=1 which
  shift 2
  while :; do
    run_one "$label" "$mode" "$@"
    measured "$label" || return 1
    [ -n "$(reached_115)" ] && return 0
    which="$(stopped_by_2741)"
    if [ -z "$which" ]; then
      UNMEASURED=$((UNMEASURED + 1))
      echo "  [未実測] ${label}: 項目 115 まで届かなかった（$(grep -m1 "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" | cut -c1-160)）"
      return 1
    fi
    N2741=$((N2741 + 1))
    echo "  [#2741] ${label}: 手前の項目 ${which} が器の競合（#2741）で落ちた = NG に数えない（${try}/${MAX_TRIES}）"
    if [ "$try" -ge "$MAX_TRIES" ]; then
      UNMEASURED=$((UNMEASURED + 1))
      echo "  [未実測] ${label}: ${MAX_TRIES} 回とも #2741 で項目 115 まで届かなかった"
      return 1
    fi
    try=$((try + 1))
  done
}

for run in $RUNS; do
  case "$run" in
    legacy)
      echo "--- 旧（TAKO_2727_LEGACY=1 = pin しない）"
      run_arm "$run" stop TAKO_2727_LEGACY=1 || continue
      check "$(failed_with "115: チャット本文が描かれる")" \
        "旧: 項目 115 の「チャット本文が描かれる」で FAILED = 修正前の再現"
      check "$(has "legacy=true targets_include_fixture=true kept=false")" \
        "旧: 定期更新の 1 周がこのペインを読みに行き、注入した会話を消した"
      check "$(in_head "TAKO_SELF_TEST_830_NOTDRAWN: .* chat_pane=false pinned=false")" \
        "旧: 描画待ちが切れた時点で会話は無く、pin もされていない"
      ;;
    ok)
      i=1
      while [ "$i" -le "$OK_RUNS" ]; do
        echo "--- 新（${i}/${OK_RUNS}）"
        if run_arm "ok$i" stop; then
          check "$(passed_115)" "新 ${i}/${OK_RUNS}: 項目 115 を通る"
          check "$(has "legacy=false targets_include_fixture=false kept=true")" \
            "新 ${i}/${OK_RUNS}: 定期更新の 1 周はこのペインを読みに行かず、会話が残る"
        fi
        i=$((i + 1))
      done
      ;;
    full)
      echo "--- 新（最後まで）"
      run_arm full full || continue
      check "$(passed_115)" "最後まで: 項目 115 を通る"
      check "$(grep -q "^TAKO_APP_SELF_TEST_OK" "$RUN_LOG" && echo 1)" "最後まで: TAKO_APP_SELF_TEST_OK"
      check "$(not "$(failed_any)")" "最後まで: FAILED が無い"
      ;;
    *)
      echo "知らない腕: ${run}（legacy / ok / full）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED #2741 で手前で止まった回=${N2741}（NG に数えない）"
[ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]
