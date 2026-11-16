#!/bin/bash
# test-lsp-band-2751.sh — 言語サーバの「未導入」の帯が、狭いペイン（幅 240px 前後）でも本文の行を全部
# 押し出さないか（#2751）の実経路テスト
#
# 何を確かめるか:
#   new     visual-test `lsp-band-2751` 節を既定の寸法の窓（960×600）で回す。.py（200 行）を編集モードで
#           開き、言語サーバの差し替え口を一時 dir の深い配下の実行できないファイルへ向ける（= 未導入・理由に
#           長いパス）。ペインを幅 240px 前後へ縮めて、本文の行・帯・帯の文の器を測る。旧の形をその場で倒した
#           同じ場面で本文の行が 0（症状の再現）・帯の中を実ホイールで送ると最後の行が読める・「入れる」を
#           実マウスで押すと取得が走り（配布元は閉じたポート = 外へ出ない）失敗の帯へ変わる・広いペインは
#           新旧の画素の差が 0 = TAKO_VISUAL_TEST_OK
#   legacy  同じバイナリで TAKO_2751_LEGACY=1（帯の文の器に上限なし）= 修正前の再現。
#           「①: 本文の行がペインの高さの半分以上描かれる」で FAILED し、本文の行が 0 の数値が残る
# `TAKO_2751_RUNS`（空白区切り。既定 `new legacy`）で腕を選ぶ。`TAKO_2751_DUMP_DIR` を渡すと
# 窓の絵（PNG）をそこへ残す（既定は一時 dir の中 = 後片付けで消える）。`TAKO_2751_KEEP_TMP=1` は
# ログ（腕ごとの app.log と data dir）を残して置き場を出す。
#
# 使い方: bash scripts/test-lsp-band-2751.sh
#
# **本番の tako / 設定・本物のファイル・本物の言語サーバの置き場には一切触らない**（data / HOME / fixture は
# mktemp 配下、言語サーバの取得先は 127.0.0.1 の閉じたポート、tmux は専用ソケット、落とすのは自分で起こした
# pid だけ）。窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意
# できなければ起動せず終了コード 4 で「未実測」を返す。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_2751_LEGACY TAKO_TMUX_SOCKET TAKO_LSP_AUTO_FETCH TAKO_LSP_FETCH_BASE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
RUNS="${TAKO_2751_RUNS:-new legacy}"
# 既定の寸法の窓（960×600）。tako-vd の右下寄りに置き、ほかの検証の窓と重ねない
BOUNDS="${TAKO_2751_BOUNDS:-1500,780,960,600}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2751-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-2751-$$"
DUMP="${TAKO_2751_DUMP_DIR:-$TMP/dump}"

cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  if [ -n "${TAKO_2751_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "test-lsp-band-2751"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$DUMP"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 言語サーバは起こす（隔離の既定は起こさない = 帯が出ない。#2048）。取得は自動では走らせず、
# 「入れる」を押したときの取得先は閉じたポート（127.0.0.1:9）= 外へ出ずにすぐ失敗する
common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja
  TAKO_PERSIST=0 TAKO_AUTO_RENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
  TAKO_LSP_AUTOSTART=1 TAKO_LSP_FETCH_BASE="http://127.0.0.1:9"
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
      TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=lsp-band-2751 TAKO_VISUAL_DUMP_DIR="$DUMP/$label" \
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
  grep -E "TAKO_VISUAL_PIXEL: lsp-band-2751|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK|TAKO_VISUAL_TEST_UNMET" "$RUN_LOG" \
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
# 1 行の `key=値` の値（数値の比較用）。札（①new など）を持つ行に絞る
field() { grep -E "^TAKO_VISUAL_PIXEL: lsp-band-2751 $1 (.* )?$2=" "$RUN_LOG" | tail -1 | tr ' ' '\n' | sed -n "s/^$2=//p" | head -1; }
ge() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 >= b + 0) }' && echo 1; }
lt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 < b + 0) }' && echo 1; }
eq() { [ -n "$1" ] && [ "$1" = "$2" ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    new)
      echo "--- 新（既定の窓 960×600）"
      run_one "$run"
      measured "$run" || continue
      check "$(in_log "^TAKO_VISUAL_TEST_OK")" "新: TAKO_VISUAL_TEST_OK"
      check "$(not "$(in_log "TAKO_APP_SELF_TEST_FAILED")")" "新: FAILED が無い"
      check "$(ge "$(field ①new code_h)" "$(field ①new half)")" \
        "新: ① 狭いペイン（幅 $(field ①width pane_w)px）で本文の行 $(field ①new lines) 行・$(field ①new code_h)px がペインの高さの半分 $(field ①new half)px 以上"
      check "$(eq "$(field ②legacy lines)" 0)" "新: ② 同じ場面で旧の形へ倒すと本文の行が $(field ②legacy lines) 行（症状の再現）"
      check "$(in_log "lsp-band-2751 ④click fetch_failed=true")" "新: ④ 実マウスで押した「入れる」が取得を起こし、失敗の帯へ変わる"
      check "$(eq "$(field ⑤wide changed)" 0)" "新: ⑤ 広いペイン（幅 $(field ⑤wide pane_w)px）の画素の差が $(field ⑤wide changed)"
      ;;
    legacy)
      echo "--- 旧（TAKO_2751_LEGACY=1 = 帯の文の器に上限なし。既定の窓 960×600）"
      run_one "$run" TAKO_2751_LEGACY=1
      measured "$run" || continue
      check "$(in_log "TAKO_APP_SELF_TEST_FAILED: visual-test lsp-band-2751 ①: 本文の行がペインの高さの半分以上描かれる")" \
        "旧: 「①: 本文の行がペインの高さの半分以上描かれる」で FAILED（修正前の再現）"
      check "$(lt "$(field ①new code_h)" "$(field ①new half)")" \
        "旧: ① 本文の行 $(field ①new lines) 行・$(field ①new code_h)px がペインの高さの半分 $(field ①new half)px に届かない"
      check "$(not "$(in_log "^TAKO_VISUAL_TEST_OK")")" "旧: TAKO_VISUAL_TEST_OK にならない"
      ;;
    *)
      echo "知らない腕: ${run}（new / legacy）"
      exit 2
      ;;
  esac
done

if [ -n "${TAKO_2751_DUMP_DIR:-}" ]; then
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
