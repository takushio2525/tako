#!/bin/bash
# test-lsp-1678.sh — LSP 基盤（#1678）の実経路テスト（隔離 GUI・偽サーバ）
#
# 何を確かめるか（Issue #1678 の受け入れ条件のうち、GUI を通して初めて言えるもの）:
#   ① 編集モードに入ると偽サーバが起きて握手し、didOpen が届く（開いただけでは起こさない）
#   ② 範囲編集（tako edit replace-range）が didChange（incremental・UTF-16 の桁・#1658 の版）で届く
#   ③ `tako lsp status --json` と MCP `tako_lsp_server` が同じ答えを返す（同じ dispatch）
#   ④ restart で起こし直す（spawn が 1 つ増え、今の本文で didOpen し直す）/ stop で止まる
#   ⑤ `tako lsp servers` が未導入のサーバに理由と導入コマンドを返す
#   ⑥ アイドル中（既定 60 秒 = IDLE_SECS）に偽サーバが受信した行数が 0（#772）
#   ⑦ 編集の結果が LSP の有無でバイト一致（偽サーバあり / 未導入 / TAKO_1007_LEGACY=1）
#   ⑧ GUI を kill -9 しても偽サーバが生き残らない（孤児を残さない）
#
# 使い方: bash scripts/test-lsp-1678.sh            （IDLE_SECS=5 で短縮できる）
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IDLE_SECS="${IDLE_SECS:-60}"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1678-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1678-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
FAKE="$(dirname "$TAKO_BIN")/tako-lsp-fake"
if [ ! -x "$FAKE" ]; then
  (cd "$REPO_ROOT" && cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
fi

export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"

FIX="$TMP/fixtures"
mkdir -p "$FIX/proj/src"
printf '[package]\nname = "p"\n' > "$FIX/proj/Cargo.toml"
# 日本語・絵文字（サロゲートペア）を含む 1 行目の後ろを直す = 桁が UTF-16 で届くかを見る
printf '// 日本😀\nfn main() {}\n' > "$FIX/proj/src/main.rs"
for copy in fake missing legacy; do
  mkdir -p "$FIX/$copy/src"
  cp "$FIX/proj/Cargo.toml" "$FIX/$copy/Cargo.toml"
  cp "$FIX/proj/src/main.rs" "$FIX/$copy/src/main.rs"
done

LOG="$TMP/fake.jsonl"
SPAWNS="$TMP/spawns.txt"
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
lines() { [ -f "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }
methods() { python3 -c 'import json,sys
for l in open(sys.argv[1]):
    try:
        m = json.loads(l).get("method")
    except Exception:
        continue
    if m: print(m)' "$LOG" 2>/dev/null | tr '\n' ' '; }
wait_for() { # 説明 秒数 コマンド…
  local what="$1" secs="$2"; shift 2
  local n=0
  while [ "$n" -lt $((secs * 10)) ]; do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.1; n=$((n + 1))
  done
  echo "  （${what} が ${secs} 秒以内に起きなかった）"
  return 1
}
state_is() { [ "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["servers"][0]["state"]')" = "$1" ]; }
log_has() { case " $(methods) " in *" $1 "*) return 0 ;; esac; return 1; }
# 待ちの条件は**関数にする**（`test "$(lines …)" -ge 2` を渡すと呼び出し時に 1 度だけ展開され、
# 条件が再評価されないまま上限まで待って素通りする）
spawns_at_least() { [ "$(lines "$SPAWNS")" -ge "$1" ]; }
expect() { # 説明 秒数 コマンド… — 待ちが外れたら FAIL として数える
  local what="$1"
  if wait_for "$@"; then pass "$what"; else fail "$what"; fi
}

start_gui() { # ログ名 追加の env…
  local name="$1"; shift
  export TAKO_DATA_DIR="$TMP/data-$name"
  mkdir -p "$TAKO_DATA_DIR"
  launch_isolated_gui "$TMP/app-$name.log" "$@" || exit 1
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$name.log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
}

open_preview() { # ファイル → プレビューペイン ID
  "$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]'
}

# 同じ編集列を当てて保存し、応答（ペイン ID を除く）を 1 行ずつ残す
edit_sequence() { # ファイル 出力
  local pane out="$2"
  pane="$(open_preview "$1")"
  {
    "$TAKO_BIN" edit start --pane "$pane" | json 'd.get("editing")'
    "$TAKO_BIN" edit replace-range 1:13 1:13 "語" --pane "$pane" | json '(d["document"]["version"], d["document"]["line_count"])'
    "$TAKO_BIN" edit replace-range 2:11 2:11 " 1 " --pane "$pane" | json '(d["document"]["version"], d["document"]["cursor"])'
    "$TAKO_BIN" edit save --pane "$pane" | json 'd.get("dirty")'
  } > "$out"
  echo "$pane"
}

echo "== A: 偽サーバあり =="
start_gui fake \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_LOG="$LOG" \
  TAKO_LSP_FAKE_SPAWNS="$SPAWNS" \
  TAKO_LSP_BIN_PYRIGHT="$TMP/no-such-pyright"
echo "  pid=$APP_PID"

echo
echo "== ① 開いただけでは起こさず、編集モードで起こす =="
PANE="$(open_preview "$FIX/proj/src/main.rs")"
sleep 1
check_eq "開いただけではサーバが 0 件" "0" "$("$TAKO_BIN" lsp status --json | json 'len(d["servers"])')"
check_eq "偽サーバは起きていない" "0" "$(lines "$SPAWNS")"
"$TAKO_BIN" edit start --pane "$PANE" >/dev/null
expect "編集モードで稼働した" 20 state_is running
expect "didOpen が届いた" 10 log_has textDocument/didOpen
check_eq "握手の順序" "initialize initialized textDocument/didOpen " "$(methods)"
STATUS="$("$TAKO_BIN" lsp status --json)"
check_eq "能力: 同期は incremental" "incremental" "$(printf '%s' "$STATUS" | json 'd["servers"][0]["text_document_sync"]')"
check_eq "位置は utf-16" "utf-16" "$(printf '%s' "$STATUS" | json 'd["servers"][0]["position_encoding"]')"
check_eq "診断を 1 件受けて保持" "1" "$(printf '%s' "$STATUS" | json 'd["servers"][0]["diagnostics"]')"

echo
echo "== ② 範囲編集が didChange で届く（UTF-16 の桁・#1658 の版） =="
VERSION="$("$TAKO_BIN" edit replace-range 1:13 1:13 "語" --pane "$PANE" | json 'd["document"]["version"]')"
expect "didChange が届いた" 10 log_has textDocument/didChange
CHANGE="$(python3 -c 'import json,sys
for l in open(sys.argv[1]):
    m = json.loads(l)
    if m.get("method") == "textDocument/didChange":
        p = m["params"]; c = p["contentChanges"][0]
        print(p["textDocument"]["version"], c["range"]["start"]["line"], c["range"]["start"]["character"], c["text"])' "$LOG")"
# `// 日本😀` = `// `(3) + 日本(2) + 😀(2) = 7 桁（UTF-8 では 13 バイト目）
check_eq "版は編集 API の版そのもの・桁は UTF-16" "$VERSION 0 7 語" "$CHANGE"

echo
echo "== ③ CLI と MCP が同じ答え =="
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # action → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1678","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp_server\",\"arguments\":{\"action\":\"$1\"}}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }
CLI_STATUS="$("$TAKO_BIN" lsp status --json)"
MCP_STATUS="$(mcp status)"
if same_json "$CLI_STATUS" "$MCP_STATUS"; then pass "status が CLI と MCP で一致"; else fail "status が食い違う: CLI=$CLI_STATUS MCP=$MCP_STATUS"; fi
CLI_SERVERS="$("$TAKO_BIN" lsp servers --json)"
MCP_SERVERS="$(mcp list)"
if same_json "$CLI_SERVERS" "$MCP_SERVERS"; then pass "servers / list が CLI と MCP で一致"; else fail "servers が食い違う"; fi

echo
echo "== ⑤ 未導入のサーバには理由と導入コマンド =="
check_eq "差し替え先が無い pyright は未導入" "False" "$(printf '%s' "$CLI_SERVERS" | json '[s for s in d["servers"] if s["id"]=="pyright"][0]["installed"]')"
check_eq "導入コマンドが載る" "npm install -g pyright" "$(printf '%s' "$CLI_SERVERS" | json '[s for s in d["servers"] if s["id"]=="pyright"][0]["install_command"]')"
check_eq "理由と次の一手が載る" "True" "$(printf '%s' "$CLI_SERVERS" | json 'all([s for s in d["servers"] if s["id"]=="pyright"][0].get(k) for k in ("reason","next_step"))')"
check_eq "偽サーバは導入済みに見える" "True" "$(printf '%s' "$CLI_SERVERS" | json '[s for s in d["servers"] if s["id"]=="rust-analyzer"][0]["installed"]')"

echo
echo "== ⑥ アイドル ${IDLE_SECS} 秒で偽サーバの受信 0 行 =="
BEFORE="$(lines "$LOG")"
sleep "$IDLE_SECS"
AFTER="$(lines "$LOG")"
echo "  受信 ${BEFORE} → ${AFTER} 行"
check_eq "アイドル中の受信" "0" "$((AFTER - BEFORE))"

echo
echo "== ④ restart で起こし直し、stop で止まる =="
"$TAKO_BIN" lsp restart >/dev/null
expect "restart で起こし直した" 20 spawns_at_least 2
expect "起こし直して稼働" 20 state_is running
REOPENED="$(python3 -c 'import json,sys
opens = [json.loads(l) for l in open(sys.argv[1]) if "didOpen" in l]
print(len(opens), "語" in opens[-1]["params"]["textDocument"]["text"])' "$LOG")"
check_eq "今の本文で didOpen し直す" "2 True" "$REOPENED"
MCP_RESTART="$(mcp restart | json 'len(d["restarted"])')"
check_eq "MCP の restart も同じ口" "1" "$MCP_RESTART"
expect "MCP の restart で起こし直した" 20 spawns_at_least 3
expect "MCP の restart の後も稼働" 20 state_is running
"$TAKO_BIN" lsp stop >/dev/null
expect "stop で止まる" 20 state_is stopped
SPAWNED="$(lines "$SPAWNS")"
"$TAKO_BIN" edit replace-range 2:0 2:0 "" --pane "$PANE" >/dev/null
"$TAKO_BIN" edit replace-range 2:0 2:0 "// z\n" --pane "$PANE" >/dev/null
sleep 1
check_eq "止めた後は打鍵しても起こさない" "$SPAWNED" "$(lines "$SPAWNS")"
"$TAKO_BIN" lsp restart >/dev/null
expect "止めた後の restart で稼働" 20 state_is running

echo
echo "== ⑦-A 偽サーバありの編集列 =="
edit_sequence "$FIX/fake/src/main.rs" "$TMP/resp-fake.txt" >/dev/null

echo
echo "== ⑧ GUI を kill -9 しても偽サーバが残らない =="
SERVER_PID="$("$TAKO_BIN" lsp status --json | json '[s["pid"] for s in d["servers"] if s["pid"]][0]')"
if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
  pass "偽サーバ pid=$SERVER_PID は生きている"
  kill -9 "$APP_PID" 2>/dev/null
  wait "$APP_PID" 2>/dev/null
  APP_PID=""
  gone() { ! kill -0 "$SERVER_PID" 2>/dev/null; }
  expect "GUI が落ちたら偽サーバも終わった（孤児なし）" 10 gone
else
  fail "偽サーバの pid を採れない"
fi
stop_isolated_gui "$APP_PID"; APP_PID=""

echo
echo "== ⑦-B サーバ未導入の編集列 =="
start_gui missing TAKO_LSP_BIN_RUST_ANALYZER="$TMP/no-such-rust-analyzer"
edit_sequence "$FIX/missing/src/main.rs" "$TMP/resp-missing.txt" >/dev/null
check_eq "未導入なら状態は not_installed" "not_installed" "$("$TAKO_BIN" lsp status --json | json 'd["servers"][0]["state"]')"
stop_isolated_gui "$APP_PID"; APP_PID=""

echo
echo "== ⑦-C TAKO_1007_LEGACY=1（LSP を丸ごと止める）の編集列 =="
start_gui legacy TAKO_1007_LEGACY=1 TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" TAKO_LSP_FAKE_SPAWNS="$TMP/spawns-legacy.txt"
edit_sequence "$FIX/legacy/src/main.rs" "$TMP/resp-legacy.txt" >/dev/null
check_eq "LEGACY は LSP を持たない" "False" "$("$TAKO_BIN" lsp status --json | json 'd["enabled"]')"
check_eq "LEGACY では偽サーバを起こさない" "0" "$(lines "$TMP/spawns-legacy.txt")"
stop_isolated_gui "$APP_PID"; APP_PID=""

echo
echo "== ⑦ 編集の結果が LSP の有無でバイト一致 =="
if cmp -s "$FIX/fake/src/main.rs" "$FIX/legacy/src/main.rs" && cmp -s "$FIX/missing/src/main.rs" "$FIX/legacy/src/main.rs"; then
  pass "保存したファイルが 3 通りでバイト一致（$(wc -c < "$FIX/legacy/src/main.rs" | tr -d ' ') バイト）"
else
  fail "保存したファイルが食い違う"
fi
if cmp -s "$TMP/resp-fake.txt" "$TMP/resp-legacy.txt" && cmp -s "$TMP/resp-missing.txt" "$TMP/resp-legacy.txt"; then
  pass "編集 API の応答（版・行数・カーソル）が 3 通りで一致"
else
  fail "編集 API の応答が食い違う"; diff "$TMP/resp-fake.txt" "$TMP/resp-legacy.txt"
fi

echo
echo "== 結果: ${PASS} PASS ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
