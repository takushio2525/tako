#!/bin/bash
# tako:run bash scripts/test-run-dropdown-1733.sh
# test-run-dropdown-1733.sh — 実行ボタン横のドロップダウン（#1733）の実画面の検証
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定には触らない）:
#   新: visual-test `run-dropdown-1733` が通る（中身は crates/tako-app/src/self_test/run_dropdown_1733.rs の
#       ①〜⑧ = RunResolve との字面一致・一覧と Run のコマンドの一致・実マウスで開く・候補を選ぶと
#       run-configs.json と次の Run に効く・欄で直す・空と不正な環境変数名は保存されない・出典タグ・境界 2 つ）
#   旧（A/B）: `TAKO_1733_LEGACY=1`（一覧を #1733 以前の同期の解決で作る）は ① の「一覧と Run の
#       コマンドが一致する」で名指しの FAILED になる（一覧が `python3 a.py` のまま）
#
# 画像は新旧で置き場を分ける（`$TAKO_1733_KEEP_DUMP/new` と `/legacy`。未指定なら一時 dir と一緒に消える）。
#
# 使い方: bash scripts/test-run-dropdown-1733.sh
#         TAKO_1733_KEEP_DUMP=<dir> bash scripts/test-run-dropdown-1733.sh   # 画像を残す
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1733-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERF_LOG="$TMP/perf.log"
# 実行ペインは器を使わない（隔離 tmux の後片付けを要らなくする）
export TAKO_PERSIST=0
# 材料（偽の venv と a.py / b.py）は一時 dir の下（画面に出るパスを短くする・後で消える）
export TMPDIR="$TMP/t"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/zdot" "$TMPDIR"
# 画像にユーザー名・ホスト名が写らないよう固定のプロンプト（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 1 回ぶん: $1 = 名前（new / legacy）・残りは GUI へ渡す VAR=VAL
run_visual() {
  local name="$1"; shift
  local dump="$TMP/dump-$name"
  mkdir -p "$TMP/data-$name"
  launch_isolated_gui "$TMP/$name.log" ZDOTDIR="$TMP/zdot" TAKO_DATA_DIR="$TMP/data-$name" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=run-dropdown-1733 TAKO_VISUAL_DUMP_DIR="$dump" "$@" || exit $?
  local vpid="$ISOLATED_GUI_PID"
  local i
  for i in $(seq 1 240); do
    kill -0 "$vpid" 2>/dev/null || break
    sleep 1
  done
  stop_isolated_gui "$vpid"
  ISOLATED_GUI_PID=""
  grep -E "TAKO_VISUAL_1733|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK|TAKO_VISUAL_DUMP_FILE|panicked" \
    "$TMP/$name.log" | sed 's/^/    /'
  if [ -n "${TAKO_1733_KEEP_DUMP:-}" ] && [ -d "$dump" ]; then
    mkdir -p "$TAKO_1733_KEEP_DUMP/$name"
    cp "$dump"/*.png "$TAKO_1733_KEEP_DUMP/$name"/ 2>/dev/null
    echo "    画像: $TAKO_1733_KEEP_DUMP/$name"
  fi
}

echo "== 新: visual-test run-dropdown-1733 =="
run_visual new
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log" && ! grep -q "TAKO_APP_SELF_TEST_FAILED" "$TMP/new.log"; then
  pass "新: ①〜⑧ が通る（RunResolve と一致・一覧と Run が一致・選択 / 欄 / 出典タグが保存へ効く）"
else
  fail "新: visual-test が通らない"
fi
if grep -q "run-dropdown-1733-open.png" "$TMP/new.log"; then
  pass "新: 開いたメニューの画像を書き出した"
else
  fail "新: 画像が書き出されていない（TAKO_VISUAL_DUMP_DIR）"
fi

echo "== 旧（A/B）: TAKO_1733_LEGACY=1 =="
run_visual legacy TAKO_1733_LEGACY=1
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test run-dropdown-1733 ①: 一覧の走らせるコマンドと Run のコマンドが一致する" "$TMP/legacy.log"; then
  pass "旧: 一覧と Run のコマンドのずれ（① の一致）で名指しの FAILED"
else
  fail "旧: ① の一致で落ちていない（A/B の検出力が無い）"
fi

echo
echo "結果: OK $PASS / NG $FAIL"
[ "$FAIL" -eq 0 ]
