#!/bin/bash
# test-lsp-completion-cancel-1909.sh — 補完の打鍵の取り消しの番号（#1909）の実経路テスト（隔離 GUI・偽サーバ）
#
# 何を確かめるか（Issue #1909 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 completion-cancel（偽サーバの loading・読み込み中の補完は答えずに待たせる）:
#      背景の走り出しを合図まで止める注入（TAKO_1909_INJECT_HOLD）で「背景が走り出す前に語の外へ出る」
#      順序を作り、閉じた後に走り出した背景が問い合わせずに抜けて inflight / pending_requests が 0 /
#      閉じた直後の打鍵は取り消されず、読み込みが済むと一覧が出る
#   ② A/B: TAKO_1909_LEGACY=1（GUI が番号を背景で取る = #1909 の前）で同じ節の ③ が名指しで落ちる
#      （取り消したはずの要求がサーバへ届き、待ちの表に残る）
#   ③ CLI / MCP: `tako lsp status --json` と MCP `tako_lsp_server`（action=status）に同じ `inflight`
#      （completion / resolve / hover）が載る
#
# 使い方: bash scripts/test-lsp-completion-cancel-1909.sh
#   画像を残すときは TAKO_1909_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1909-XXXXXX)"
DUMP="${TAKO_1909_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1909-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app と偽サーバをビルドします…"
(cd "$REPO_ROOT" && cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test \
  && cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DUMP"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

mcp() { # 引数のツール名と JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1909","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        r = m.get("result")
        print(r["content"][0]["text"] if r else json.dumps(m.get("error"), ensure_ascii=False)); break'
}

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 300 秒）
run_section() {
  local log="$1" section="$2"
  shift 2
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" \
    TAKO_VISUAL_DUMP_DIR="$DUMP" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 3000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  return 0
}

echo
echo "== ① visual-test 節 completion-cancel（偽サーバ・背景の走り出しを合図まで止める注入） =="
run_section "$TMP/cancel.log" completion-cancel
grep "TAKO_VISUAL_PIXEL: completion-cancel" "$TMP/cancel.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/cancel.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/cancel.log" | tail -5 | sed 's/^/    /'
fi

echo
echo "== ② A/B: TAKO_1909_LEGACY=1（GUI が番号を背景で取る = #1909 の前）で同じ節の ③ が名指しで落ちる =="
run_section "$TMP/cancel-legacy.log" completion-cancel TAKO_1909_LEGACY=1
grep "TAKO_VISUAL_PIXEL: completion-cancel" "$TMP/cancel-legacy.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test completion-cancel ③" "$TMP/cancel-legacy.log"; then
  pass "旧い形では落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/cancel-legacy.log" | head -1 | cut -c1-200)"
else
  fail "旧い形でも ③ で落ちない（節に検出力が無い）"
  grep -E "FAILED|panicked|ERROR" "$TMP/cancel-legacy.log" | tail -5 | sed 's/^/    /'
fi

echo
echo "== ③ CLI / MCP: tako lsp status と tako_lsp_server（status）に同じ inflight が載る =="
launch_isolated_gui "$TMP/app-cli.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-cli.log" || exit 1
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
CLI_INFLIGHT="$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'json.dumps(d["inflight"], sort_keys=True)')"
MCP_INFLIGHT="$(mcp tako_lsp_server '{"action":"status"}' | json 'json.dumps(d["inflight"], sort_keys=True)')"
echo "    CLI: ${CLI_INFLIGHT}"
echo "    MCP: ${MCP_INFLIGHT}"
check_eq "CLI の inflight（何も問い合わせていない = すべて 0）" '{"completion": 0, "hover": 0, "resolve": 0}' "$CLI_INFLIGHT"
check_eq "MCP の inflight が CLI と字面一致" "$CLI_INFLIGHT" "$MCP_INFLIGHT"
stop_isolated_gui "$APP_PID"
APP_PID=""

if [ -n "${TAKO_1909_DUMP_DIR:-}" ]; then
  echo
  echo "画像: $DUMP"
  ls "$DUMP" 2>/dev/null | sed 's/^/    /'
fi
echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
