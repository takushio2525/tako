#!/usr/bin/env bash
# test-todo-expand-1479.sh — 右パネル tasks ビューの展開を CLI / MCP から操作する実経路テスト（#1479）
#
# 隔離した data / orchestrator / tmux で**実 tako-app** を立て、CLI と MCP から
#   ① `tako todo expand <id>` が**その 1 件だけ**を展開し、右パネルを開いて tasks ビューへ切り替える
#   ② `tako todo list` の応答に `expanded` が載る（画面の状態を AI から**読める**）
#   ③ 別の id を expand すると乗り換える（同時に開くのは 1 件）
#   ④ `tako todo collapse` で畳める（パネルは閉じない）
#   ⑤ 無い id は**開いたと言わない**（エラーで返り、展開は変わらない）
#   ⑥ `done` にすると畳む（消える行の下に詳細が残らない）
#   ⑦ MCP（`tako_todo` の action）が CLI と同じ値を返す（設計原則 5 の 1:1）
# を実測する。
#
# **本番の tako / 設定 / user-tasks.yaml には一切触らない**（data / orchestrator / HOME は
# mktemp 配下、tmux は専用ソケット、落とすのは自分で起こした pid だけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-todo-expand-1479.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

# `/tmp` 直下に取る（tmux と IPC が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1479-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1479-$$"
cleanup() {
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_USER_TASKS_FILE="$TMP/orch/user-tasks.yaml"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_PERSIST=0
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

jsonf() { python3 -c 'import json,sys
d = json.load(sys.stdin)
for k in sys.argv[1:]:
    if d is None:
        break
    d = d[int(k)] if isinstance(d, list) else d.get(k)
print("null" if d is None else d)' "$@"; }

echo "== 隔離 GUI を起こす =="
launch_isolated_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

echo "== 検証用のタスクを 2 件起票 =="
A="$("$TAKO_BIN" todo add "展開の検証 A" --kind review --json | jsonf id)"
B="$("$TAKO_BIN" todo add "展開の検証 B" --kind post --json | jsonf id)"
check_eq "起票できる（A）" "u-1" "$A"
check_eq "起票できる（B）" "u-2" "$B"

echo "== ① expand はその 1 件を開き、パネルを tasks ビューで開く =="
# パネルは畳んだ状態から始める（expand が自分で開けることを見る）
"$TAKO_BIN" panel --hide >/dev/null 2>&1 || true
OUT="$("$TAKO_BIN" todo expand "$A" --json)"
check_eq "expanded が要求した id" "$A" "$(printf '%s' "$OUT" | jsonf expanded)"
check_eq "パネルが開く" "True" "$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["panel_visible"])')"
check_eq "tasks ビューへ切り替わる" "tasks" "$(printf '%s' "$OUT" | jsonf panel_view)"

echo "== ② list の応答から画面の状態が読める =="
check_eq "list.expanded" "$A" "$("$TAKO_BIN" todo list --json | jsonf expanded)"

echo "== ③ 別の id を開くと乗り換える（同時に開くのは 1 件） =="
"$TAKO_BIN" todo expand "$B" >/dev/null
check_eq "乗り換える" "$B" "$("$TAKO_BIN" todo list --json | jsonf expanded)"

echo "== ④ collapse で畳む（パネルは閉じない） =="
OUT="$("$TAKO_BIN" todo collapse --json)"
check_eq "collapse で null" "null" "$(printf '%s' "$OUT" | jsonf expanded)"
check_eq "パネルは開いたまま" "True" "$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["panel_visible"])')"

echo "== ⑤ 無い id は「開いた」と言わない =="
if "$TAKO_BIN" todo expand u-999 >/dev/null 2>&1; then
  fail "無い id が通ってしまう"
else
  pass "無い id はエラーで返る"
fi
check_eq "展開は変わらない" "null" "$("$TAKO_BIN" todo list --json | jsonf expanded)"

echo "== ⑥ done にすると畳む =="
"$TAKO_BIN" todo expand "$A" >/dev/null
check_eq "開いた" "$A" "$("$TAKO_BIN" todo list --json | jsonf expanded)"
"$TAKO_BIN" todo done "$A" >/dev/null
check_eq "完了で畳む" "null" "$("$TAKO_BIN" todo list --json | jsonf expanded)"

echo "== ⑦ MCP が CLI と同じ値を返す（設計原則 5 の 1:1） =="
# MCP ブリッジは **tako の中で起動された証（TAKO_SOCKET + TAKO_TOKEN）が無いと
# ツールを 1 つも公開しない**（FR-2.3.2）。隔離インスタンスの受け口を明示して起こす
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
export TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
MCP_OUT="$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1479","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_todo\",\"arguments\":{\"action\":\"expand\",\"id\":\"$B\"}}}" \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tako_todo","arguments":{"action":"list"}}}' \
  '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tako_todo","arguments":{"action":"collapse"}}}' \
  | "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c '
import json, sys
seen = {}
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") in (2, 3, 4):
        text = msg.get("result", {}).get("content", [{}])[0].get("text", "")
        try:
            payload = json.loads(text)
        except Exception:
            payload = {}
        seen[msg["id"]] = payload.get("expanded")
print("%s %s %s" % (seen.get(2), seen.get(3), seen.get(4)))
')"
check_eq "MCP expand / list / collapse" "$B $B None" "$MCP_OUT"

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
