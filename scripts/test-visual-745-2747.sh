#!/usr/bin/env bash
# #2747: visual-test #745（チャットの md テーブル。`TAKO_VISUAL_ONLY=chat-table`）が注入した会話を pin せずに
# 待っていた形の A/B。
#
# 節はフォーカス中のペインへ会話を差し込み、プレビューの読み込み（最大 5 秒）と各状態の 400ms を待ってから
# 表を測る。修正前は会話を `pin_chat_fixture`（#853 の守り）で守っておらず、待っている間に 2 秒 tick の定期更新が
# 挟まると、実 claude の居ないペインは「チャットではない」と読まれ `apply_chat_refresh` が会話を消す。しかも
# 修正前の表の検査では落ちない: 消える前に 1 度描かれた表の配置（`chat_text_index` / `chat_body_lists`）が残り、
# 3 状態とも会話が無いまま古い配置を測って TAKO_VISUAL_TEST_OK になる（偽の PASS。実測で narrow でもチャットの
# 幅が絞られない）。tick が挟まるかは運。
# 修正後は差し込んだ update の中で pin し、定期更新をその場で 1 周回して「会話が残る」を確かめ（#2727 の
# 項目 115 と同じ形）、各状態でも「表を測る時点で会話が残っている」を見る。1 周をその場で回すので、旧の腕
# （TAKO_2747_LEGACY=1 = pin だけを戻す）は時間の注入なしで決定的に落ちる。
#
# 腕（TAKO_2747_RUNS。空白区切り。既定 `legacy ok`）:
#   legacy  旧（TAKO_2747_LEGACY=1 = pin しない）: 1 周で会話が消え（`targets_include_fixture=true kept=false`）、
#           「visual-test #745: 定期更新が注入した会話を消さない」で FAILED（表を測る前 = TAKO_VISUAL_TEST_OK に
#           ならない）= 修正前の形を名指して落とす
#   ok      新: 1 周しても会話が残り（`targets_include_fixture=false kept=true`）、3 状態とも会話がある
#           （`chat_pane=true`）まま TAKO_VISUAL_TEST_OK（回数は TAKO_2747_OK_RUNS。既定 3）
# `TAKO_2747_KEEP_TMP=1` はログ（腕ごとの app.log と data dir）を残して置き場を出す。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。腕ごとに「ディスプレイ指定 tako-vd: name で解決」を確かめ、無い回は未実測として
# 数えない（visual-test は TAKO_SELF_TEST を置かないので、面の解決は persist.log と stderr の `persist:` の行に出る）。
# 本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは、このスクリプトが起こした pid と、
# その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-visual-745-2747.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2747_RUNS:-legacy ok}"
OK_RUNS="${TAKO_2747_OK_RUNS:-3}"
# 1 本の上限（節は 1 分前後。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2747_RUN_LIMIT_SECS:-900}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins visual-test || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# `TAKO_TMUX_SOCKET` も落とす = GUI に `tako-iso-<pid>` を自分で選ばせる（後片付けで自分の分だけ落とす）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2747-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2747_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 1 回分: data dir と TMPDIR を回ごとに分けて節を回す
#   run_one <ラベル> [VAR=VAL ...]  → RUN_RC / RUN_LOG / RUN_DRAWN を残す
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" attempt i dir display
  shift
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    dir="$TMP/$RUN_SEQ-$label-$attempt"
    RUN_LOG="$dir/app.log"
    RUN_DATA="$dir/data"
    mkdir -p "$dir/tmpdir" "$RUN_DATA"
    launch_isolated_gui "$RUN_LOG" "TAKO_DATA_DIR=$RUN_DATA" TAKO_AUTO_RENAME=0 \
      TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=chat-table TMPDIR="$dir/tmpdir/" ${1+"$@"} || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$APP_PID"
    for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
      kill -0 "$APP_PID" 2>/dev/null || break
      sleep 0.1
    done
    RUN_RC=0
    if kill -0 "$APP_PID" 2>/dev/null; then
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
  # 描けたか: 面の解決の行が tako-vd の名前で解決している（stderr の `persist:` か persist.log）
  RUN_DRAWN=""
  display="$(grep -h "ディスプレイ指定 " "$RUN_LOG" "$RUN_DATA/persist.log" 2>/dev/null | sed -n 1p | cut -c1-160)"
  if grep -q "ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決" "$RUN_LOG" "$RUN_DATA/persist.log" 2>/dev/null; then
    RUN_DRAWN=1
  fi
  echo "    [$label] rc=$RUN_RC drawn=${RUN_DRAWN:-0} ${display}"
  grep -E "TAKO_VISUAL_2747_745|TAKO_VISUAL_PIXEL: chat-table|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$RUN_LOG" \
    | cut -c1-300 | sed 's/^/      /'
}

# 描けなかった回は未実測（数えない）。描けた回だけ判定を数える
measured() {
  if [ -n "$RUN_DRAWN" ]; then return 0; fi
  UNMEASURED=$((UNMEASURED + 1))
  echo "  [未実測] ${1}: 「ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決」が無い"
  return 1
}

line2747() { grep "^TAKO_VISUAL_2747_745: " "$RUN_LOG" | tail -1; }
has() { grep -q -- "$1" <<<"$(line2747)" && echo 1; }
in_log() { grep -q -- "$1" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }
# 3 状態（dark / light / narrow）の検査の行が揃い、どれも会話がある
pixels_with_chat() {
  local lines
  lines="$(grep "^TAKO_VISUAL_PIXEL: chat-table " "$RUN_LOG")"
  [ "$(grep -c " chat_pane=true" <<<"$lines")" -eq 3 ] && echo 1
}

for run in $RUNS; do
  case "$run" in
    legacy)
      echo "--- 旧（TAKO_2747_LEGACY=1 = pin しない）"
      run_one "$run" TAKO_2747_LEGACY=1
      measured "$run" || continue
      check "$(has "legacy=true targets_include_fixture=true kept=false")" \
        "旧: 定期更新の 1 周がこのペインを読みに行き、注入した会話を消した"
      check "$(in_log "TAKO_APP_SELF_TEST_FAILED: visual-test #745: 定期更新が注入した会話を消さない (#2747 / #853。targets_include_fixture=true / kept=false)")" \
        "旧: 「定期更新が注入した会話を消さない」で FAILED = 修正前の形を名指す"
      check "$(not "$(in_log "^TAKO_VISUAL_TEST_OK")")" \
        "旧: 会話が無いまま表を測って TAKO_VISUAL_TEST_OK にならない（修正前の偽の PASS が起きない）"
      ;;
    ok)
      i=1
      while [ "$i" -le "$OK_RUNS" ]; do
        echo "--- 新（${i}/${OK_RUNS}）"
        run_one "ok$i"
        if measured "ok$i"; then
          check "$(has "legacy=false targets_include_fixture=false kept=true")" \
            "新 ${i}/${OK_RUNS}: 定期更新の 1 周はこのペインを読みに行かず、会話が残る"
          check "$(pixels_with_chat)" "新 ${i}/${OK_RUNS}: 3 状態とも会話があるまま表を測る"
          check "$(in_log "^TAKO_VISUAL_TEST_OK")" "新 ${i}/${OK_RUNS}: TAKO_VISUAL_TEST_OK"
          check "$(not "$(in_log "TAKO_APP_SELF_TEST_FAILED")")" "新 ${i}/${OK_RUNS}: FAILED が無い"
        fi
        i=$((i + 1))
      done
      ;;
    *)
      echo "知らない腕: ${run}（legacy / ok）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]
