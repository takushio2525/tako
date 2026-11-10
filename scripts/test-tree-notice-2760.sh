#!/bin/bash
# test-tree-notice-2760.sh — ファイルツリーの断りの通知が長いパスでも上限に収まり、ツリーの行を
# 窓の外へ押し出さないか（#2760）の実経路テスト
#
# 何を確かめるか:
#   new     visual-test `tree-notice-2760` 節を既定の寸法の窓（960×600）で回す。一時 dir の深い配下の
#           ルートで ⌘C → ⌥⌘V（自分自身へ）を断らせ、通知の高さ・ツリーの最初 / 最後の行・窓の高さを
#           測る（日本語 / 英語・とても長い日本語のファイル名・ルートの外のパス）= TAKO_VISUAL_TEST_OK
#   legacy  同じバイナリで TAKO_2760_LEGACY=1（絶対パスのまま・上限なし）= 修正前の再現。
#           「①: 通知の高さが上限以内」で FAILED し、最後の行が窓の外へ出た数値が残る
# `TAKO_2760_RUNS`（空白区切り。既定 `new legacy`）で腕を選ぶ。`TAKO_2760_DUMP_DIR` を渡すと
# 窓の絵（PNG）をそこへ残す（既定は一時 dir の中 = 後片付けで消える）。`TAKO_2760_KEEP_TMP=1` は
# ログ（腕ごとの app.log と data dir）を残して置き場を出す。
#
# 使い方: bash scripts/test-tree-notice-2760.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# OS のクリップボードは名前付きペーストボード、tmux は専用ソケット、落とすのは自分で起こした pid
# だけ）。窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意
# できなければ起動せず終了コード 4 で「未実測」を返す。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_FILE_PASTEBOARD TAKO_2760_LEGACY TAKO_TMUX_SOCKET

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
RUNS="${TAKO_2760_RUNS:-new legacy}"
# 既定の寸法の窓（960×600）。tako-vd の右下寄りに置き、ほかの検証の窓と重ねない
BOUNDS="${TAKO_2760_BOUNDS:-1500,800,960,600}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2760-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-2760-$$"
PB_NAME="tako-2760-$$"
DUMP="${TAKO_2760_DUMP_DIR:-$TMP/dump}"

cleanup() {
  stop_isolated_gui "$APP_PID"
  /usr/bin/osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSPasteboard.pasteboardWithName('$PB_NAME').releaseGlobally; '{}'" >/dev/null 2>&1 || true
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  if [ -n "${TAKO_2760_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "test-tree-notice-2760"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$DUMP"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja
  TAKO_PERSIST=0 TAKO_AUTO_RENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
  TAKO_FILE_PASTEBOARD="$PB_NAME"
)

# 1 回分: run_one <ラベル> [VAR=VAL ...] → RUN_LOG / RUN_RC / RUN_DRAWN
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直す
run_one() {
  local label="$1" dir attempt
  shift
  for attempt in 1 2; do
    dir="$TMP/$label-$attempt"
    RUN_LOG="$dir/app.log"
    mkdir -p "$dir/data" "$dir/tmpdir" "$DUMP/$label"
    ISOLATED_GUI_BOUNDS="$BOUNDS"
    # GUI の一時 dir も $TMP の中へ（fixture が節の途中で落ちても後片付けで消える）
    launch_isolated_gui "$RUN_LOG" ${common_env[@]+"${common_env[@]}"} \
      TAKO_DATA_DIR="$dir/data" TMPDIR="$dir/tmpdir/" \
      TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tree-notice-2760 TAKO_VISUAL_DUMP_DIR="$DUMP/$label" \
      ${1+"$@"} || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    wait "$APP_PID"
    RUN_RC=$?
    APP_PID=""
    ISOLATED_GUI_PID=""
    ISOLATED_GUI_BOUNDS=""
    [ "$RUN_RC" != "$ISOLATED_GUI_RC_NO_DISPLAY" ] && break
    echo "    [$label] 置き先に断られて窓を開かずに終わった（終了コード ${RUN_RC}。試み ${attempt}）"
  done
  RUN_DRAWN=""
  if grep -q "ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決" "$RUN_LOG" "$dir/data/persist.log" 2>/dev/null; then
    RUN_DRAWN=1
  fi
  echo "    [$label] 窓=${BOUNDS} rc=$RUN_RC drawn=${RUN_DRAWN:-0}"
  grep -E "TAKO_VISUAL_PIXEL: tree-notice-2760|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$RUN_LOG" \
    | sed -e "s#${TMP}#<TMP>#g" | cut -c1-600 | sed 's/^/      /'
}

measured() {
  if [ -n "$RUN_DRAWN" ]; then return 0; fi
  UNMEASURED=$((UNMEASURED + 1))
  echo "  [未実測] ${1}: 「ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決」が無い"
  return 1
}
in_log() { grep -q -- "$1" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }
# 1 行の `key=値` の値（数値の比較用）。同じ札の `text=` の行もあるので、その名前を持つ行に絞る
field() { grep -E "^TAKO_VISUAL_PIXEL: tree-notice-2760 $1 (.* )?$2=" "$RUN_LOG" | tail -1 | tr ' ' '\n' | sed -n "s/^$2=//p" | head -1; }
# `a->b` の右側
after() { printf '%s' "${1##*->}"; }
le() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 <= b + 0) }' && echo 1; }
gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 > b + 0) }' && echo 1; }

for run in $RUNS; do
  case "$run" in
    new)
      echo "--- 新（既定の窓 960×600）"
      run_one "$run"
      measured "$run" || continue
      check "$(in_log "^TAKO_VISUAL_TEST_OK")" "新: TAKO_VISUAL_TEST_OK"
      check "$(not "$(in_log "TAKO_APP_SELF_TEST_FAILED")")" "新: FAILED が無い"
      cap="$(field ①ja cap)"
      check "$(le "$(field ①ja notice_h)" "$cap")" "新: ① 通知の高さ $(field ①ja notice_h)px が上限 ${cap}px 以内"
      check "$(le "$(after "$(field ①ja last_row_bottom)")" "$(field ①ja window_h)")" \
        "新: ① 最後の行の下端 $(after "$(field ①ja last_row_bottom)")px が窓 $(field ①ja window_h)px の中"
      check "$(le "$(field ②en notice_h)" "$cap")" "新: ② 英語でも通知の高さ $(field ②en notice_h)px が上限以内"
      check "$(gt "$(field ③long scroll_max)" 0)" "新: ③ 長い断りは通知欄の中でスクロールする（$(field ③long scroll_max)px）"
      ;;
    legacy)
      echo "--- 旧（TAKO_2760_LEGACY=1 = 絶対パスのまま・上限なし。既定の窓 960×600）"
      run_one "$run" TAKO_2760_LEGACY=1
      measured "$run" || continue
      check "$(in_log "TAKO_APP_SELF_TEST_FAILED: visual-test tree-notice-2760 ①: 通知の高さが上限以内")" \
        "旧: 「①: 通知の高さが上限以内」で FAILED（修正前の再現）"
      check "$(gt "$(field ①ja notice_h)" "$(field ①ja cap)")" "旧: ① 通知の高さ $(field ①ja notice_h)px が上限 $(field ①ja cap)px を超える"
      check "$(gt "$(after "$(field ①ja last_row_bottom)")" "$(field ①ja window_h)")" \
        "旧: ① 最後の行の下端 $(after "$(field ①ja last_row_bottom)")px が窓 $(field ①ja window_h)px の外へ押し出される"
      check "$(not "$(in_log "^TAKO_VISUAL_TEST_OK")")" "旧: TAKO_VISUAL_TEST_OK にならない"
      ;;
    *)
      echo "知らない腕: ${run}（new / legacy）"
      exit 2
      ;;
  esac
done

if [ -n "${TAKO_2760_DUMP_DIR:-}" ]; then
  echo "窓の絵: ${DUMP}"
fi
echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
if [ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]; then
  tako_exit 0
fi
# 描けなかった回が残れば 4（未実測）、判定が落ちれば 1
[ "$FAIL" -eq 0 ] && exit "$ISOLATED_GUI_RC_NO_DISPLAY"
exit 1
