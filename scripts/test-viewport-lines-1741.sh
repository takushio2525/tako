#!/usr/bin/env bash
# test-viewport-lines-1741.sh — 可視行数が描いた行の実矩形と合うかの実 GUI テスト（#1741）
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、visual-test 節 `viewport-lines` を
# 走らせる。節が見るのは次の 6 相で、正解は**実際に描いた行の矩形**（器に収まる行）から採る:
#   (1) 寸法: 追従が使う可視行数 = 実矩形の行数
#   (2) 下端: ↓ を押し続けると、器は「可視行数 − 余白 3」の行で動き始め、以降は下の余白が 3 行
#   (3) 上端: ↑ で戻ると、器が動き始めてからは上の余白が 3 行
#   (4) Page Down / Up: 歩幅は可視行数 − 1、着地のあとも余白 3 行で見える
#   (5) 文字サイズ: ⌘+ と全体の文字サイズの最大（32）・最小（8）でも (1)(2) が成り立つ
#   (6) 1 行 / 視野より短い / 折り返しのある行のファイル
# 打鍵は `window.dispatch_keystroke` へ流すので、キーバインド判定 → 編集の入口 → 追従の全段を通る。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-viewport-lines-1741.sh
#   （修正前のビルドと比べるときは `APP_BIN=<そのビルドの tako-app>` を付ける。
#    節は修正前のコードでもコンパイルできる形にしてある）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1741-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

if [ -z "${APP_BIN:-}" ]; then
  echo "visual-test 版の tako-app をビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
fi
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1741-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 窓の寸法を固定する（器の高さが実行ごとに変わると可視行数の実測値が比べられない）
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 300 秒）
LOG="$TMP/run.log"
RC=0
launch_isolated_gui "$LOG" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=viewport-lines || RC=$?
if [ "$RC" -ne 0 ]; then
  echo "隔離 GUI を立てられない（終了コード ${RC}。4 = 面を用意できない = 未実測）"
  exit "$RC"
fi
PID="$ISOLATED_GUI_PID"
for _ in $(seq 1 3000); do
  kill -0 "$PID" 2>/dev/null || break
  sleep 0.1
done
stop_isolated_gui "$PID"

grep -E "TAKO_VISUAL_PIXEL: viewport-lines (geometry|zoom-in|font-max |font-min |wrapped truth|.* judge|NG)" "$LOG" \
  | sed 's/^/    /'
if grep -q "未知の節" "$LOG"; then
  fail "visual-test 付きでビルドされていない（節が無い）"
fi
if grep -q "TAKO_VISUAL_TEST_OK" "$LOG"; then
  pass "6 相すべて緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$LOG" | tail -5 | sed 's/^/    /'
fi
# 実矩形の可視行数と追従の可視行数が、文字サイズを変えても一致している
for label in geometry zoom-in font-max font-min; do
  line=$(grep "TAKO_VISUAL_PIXEL: viewport-lines $label " "$LOG" | head -1)
  truth=$(printf '%s' "$line" | sed -n 's/.*truth=\([0-9]*\).*/\1/p')
  follow=$(printf '%s' "$line" | sed -n 's/.*follow_lines=\([0-9]*\).*/\1/p')
  if [ -n "$truth" ] && [ "$truth" = "$follow" ]; then
    pass "$label: 追従の可視行数 = 実矩形の行数（$truth 行）"
  else
    fail "$label: 実矩形 ${truth:-?} 行 / 追従 ${follow:-?} 行"
  fi
done

# ログは終了時に `$TMP` ごと消える。残すなら TAKO_1741_KEEP=1（置き場は $TMPDIR）
if [ "${TAKO_1741_KEEP:-}" = 1 ]; then
  cp "$LOG" "${TMPDIR:-/tmp}/tako-1741-last.log" && echo "ログを ${TMPDIR:-/tmp}/tako-1741-last.log へ残した"
fi
echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
