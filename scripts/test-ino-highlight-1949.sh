#!/bin/bash
# test-ino-highlight-1949.sh — `.ino`（Arduino のスケッチ）を C++ で塗る（#1949）を visual-test で確かめる
#
# 何を確かめるか:
#   ① visual-test 節 ino-highlight が緑: `.ino` が複数の構文色で塗られ、同じ中身の `.cpp` と
#      span が 1 つ残らず一致する（= C++ の構文）。構文セット（#815）は開いたまま放置すると
#      猶予（30 秒）の後に手放し、閉じたら待たずに手放す。live ヒープの増減を出す
#   ② A/B: `TAKO_1949_LEGACY=1`（`.ino` を Plain Text で塗る = #1949 以前）では ① の色で落ちる
#
# 使い方: bash scripts/test-ino-highlight-1949.sh [画像の置き場]
#   画像（ino-highlight.png）は 置き場/new と 置き場/legacy へ分けて落とす。
#   置き場を省くと一時 dir に落として終了時に消す。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。節は猶予（30 秒）を実時間で待つので 1 回 1 分ほどかかる。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1949v-XXXXXX)"
DUMP_ROOT="${1:-$TMP/dump}"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app をビルドします…"
(cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1949v-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
  esac
done

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local log="$1" dump="$2"
  shift 2
  mkdir -p "$dump"
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=ino-highlight \
    TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  return 0
}

echo "① ino-highlight 節"
run_section "$TMP/new.log" "$DUMP_ROOT/new"
grep -E "TAKO_VISUAL_(PIXEL|HEAP): ino-highlight|TAKO_VISUAL_DUMP_FILE" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR|SKIPPED" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi

echo "② A/B: TAKO_1949_LEGACY=1（.ino を Plain Text で塗る = #1949 以前）"
run_section "$TMP/legacy.log" "$DUMP_ROOT/legacy" TAKO_1949_LEGACY=1
grep -E "TAKO_VISUAL_PIXEL: ino-highlight|TAKO_VISUAL_DUMP_FILE" "$TMP/legacy.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: ① .ino が複数の構文色で塗られる" "$TMP/legacy.log"; then
  pass "旧経路では ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/legacy.log" | head -1)"
else
  fail "旧経路でも ① で落ちない（節に検出力が無い）"
  grep -E "FAILED|panicked" "$TMP/legacy.log" | tail -3 | sed 's/^/    /'
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
