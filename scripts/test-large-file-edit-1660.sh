#!/bin/bash
# test-large-file-edit-1660.sh — 大きいファイルの編集（#1660）の実経路テスト（隔離 GUI・release）
#
# 何を確かめるか:
#   ① visual-test 節 `large-file-edit` を tako 自身の main.rs の写し（8 万行超）で回す。
#      偽の言語サーバ（tako-lsp-fake）をつないだ状態で、開く → 編集開始 → 全文の塗り
#      （background）→ 先頭・中央・末尾の打鍵（`dispatch_keystroke` = 実機の文字入力と
#      同じ入口）→ Enter / Backspace → ⌘Z で全部戻すと元とバイト一致 → 打って保存すると
#      ディスクと一致。1 打鍵ごとに 1 フレーム描かせて測る
#   ② 同じ節を LSP なし（`TAKO_1007_LEGACY=1`）で回す（LSP の 1 打鍵あたりの上乗せを見る）
#   ③ A/B: `TAKO_1660_SYNC_SEED=1`（編集開始で UI スレッドが全文を塗る旧経路）で回す
#      = 編集開始の UI 停止が数字で出る
#   ④ 合成の 10 万行 / 10 MB（1 行 100 バイト）でも同じ節を回す
#   ⑤ CLI と MCP: ちょうど上限（10 万行 / 10,000,000 バイト）は編集できる・上限 + 1 行 /
#      + 1 バイトは**理由と値つきで**断る（CLI と MCP で同じ文面）・`tako edit status` の
#      `limit`・改行の無い 10 MB の 1 行・CRLF の 10 万行（保存しても CR が残る）
#
# 数字は出力するだけで判定に使わない（`.agent/conventions.md`「効果を測る単体テストは
# 実時間で比べない」）。判定は「編集できる / 断る / 本文とディスクのバイト一致」だけ。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141 / #1490）。
#
# 使い方: bash scripts/test-large-file-edit-1660.sh
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

TMP="$(mktemp -d /tmp/tako-1660-XXXXXX)"
CLI_GUI_PID=""
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  stop_isolated_gui "${CLI_GUI_PID:-}"
  tmux -L "tako-1660-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# 性能を測るので release（visual-test 入り）を使う（置き場はヘルパの 1 か所が組み立てる）
export TAKO_ISO_PROFILE=release
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
echo "release（visual-test 入り）の tako-app / tako / tako-lsp-fake をビルドします…"
(cd "$REPO_ROOT" && cargo build --release -q -p tako-cli -p tako-app --features tako-app/visual-test \
  && cargo build --release -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins || exit 1
FAKE="$(dirname "$TAKO_BIN")/tako-lsp-fake"

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1660-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 応答 JSON から 1 つの値を取る（`a.b` で辿る。無ければ __MISSING__）
jget() {
  python3 -c '
import json, sys
path = sys.argv[1].split(".")
try:
    cur = json.loads(sys.stdin.read())
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for key in path:
    if isinstance(cur, dict) and key in cur:
        cur = cur[key]
    else:
        print("__MISSING__"); sys.exit(0)
print(cur if isinstance(cur, str) else json.dumps(cur, ensure_ascii=False, sort_keys=True))
' "$1"
}

# visual-test 節を 1 回走らせて、終わるまで**状態で**待つ（上限 600 秒）
run_section() { # ログ 追加の env…
  local log="$1"
  shift
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=large-file-edit "$@" || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 6000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  grep "TAKO_VISUAL_PIXEL: large-file-edit" "$log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$log"; then
    return 0
  fi
  grep -E "FAILED|panicked|ERROR" "$log" | tail -5 | sed 's/^/    /'
  return 1
}

# fixture を作る（python で書く = 行数とバイト数を正確に揃える）
FIX="$TMP/fixtures"
mkdir -p "$FIX"
python3 - "$FIX" <<'PY'
import os, sys
fix = sys.argv[1]
def write(name, data):
    with open(os.path.join(fix, name), "wb") as f:
        f.write(data)
# 1 行 100 バイト（改行込み）× 10 万行 = ちょうど 10,000,000 バイト / 10 万行
lines = []
for i in range(100_000):
    head = f"let value_{i} = {i}; // "
    lines.append((head + "~" * (99 - len(head)) + "\n").encode())
exact = b"".join(lines)
assert len(exact) == 10_000_000 and exact.count(b"\n") == 100_000
write("exact.rs", exact)
# 上限 + 1 行（バイトは上限の内側 = 行数が理由になる）
short = b"".join(f"let v{i} = {i};\n".encode() for i in range(100_001))
write("over_lines.rs", short)
# 上限 + 1 バイト（行数は上限の内側 = 大きさが理由になる）
write("over_bytes.txt", exact[:-1] + b"x\n")
# 改行の無い 10 MB の 1 行（ちょうど上限）
write("one_line.txt", b"a" * 10_000_000)
# CRLF の 10 万行
write("crlf.txt", b"".join(f"crlf line {i}\r\n".encode() for i in range(100_000)))
PY
# ① / ② / ③ は tako の main.rs の写し（節が一時ディレクトリへ写してから触る）
MAIN_RS="$REPO_ROOT/crates/tako-app/src/main.rs"
echo "対象: $(wc -l < "$MAIN_RS" | tr -d ' ') 行 / $(wc -c < "$MAIN_RS" | tr -d ' ') バイト（crates/tako-app/src/main.rs の写し）"

echo
echo "① main.rs の写し・偽の言語サーバあり"
if run_section "$TMP/lsp.log" TAKO_1660_FILE="$MAIN_RS" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE"; then
  pass "節が緑（開く・打鍵・undo で元に戻る・保存がディスクと一致）"
else
  fail "節が緑にならない（LSP あり）"
fi
if grep -q "lsp_servers=1" "$TMP/lsp.log"; then
  pass "偽の言語サーバが起きて文書を受け持った（lsp_servers=1）"
else
  fail "偽の言語サーバが起きていない"
fi

echo
echo "② main.rs の写し・LSP なし（TAKO_1007_LEGACY=1）"
if run_section "$TMP/nolsp.log" TAKO_1660_FILE="$MAIN_RS" TAKO_1007_LEGACY=1; then
  pass "節が緑（LSP なし）"
else
  fail "節が緑にならない（LSP なし）"
fi

echo
echo "③ A/B: TAKO_1660_SYNC_SEED=1（編集開始で UI スレッドが全文を塗る旧経路）"
if run_section "$TMP/sync.log" TAKO_1660_FILE="$MAIN_RS" TAKO_1007_LEGACY=1 TAKO_1660_SYNC_SEED=1; then
  pass "旧経路でも編集自体は通る（違いは編集開始の UI 停止）"
else
  fail "旧経路で節が落ちた"
fi
if grep -q "deferred=true" "$TMP/nolsp.log" && grep -q "deferred=false" "$TMP/sync.log"; then
  pass "新経路は全文の塗りを background へ出し、旧経路は UI スレッドで塗る（deferred の差）"
else
  fail "deferred の差が出ない"
fi

echo
echo "④ 合成 10 万行 / 10,000,000 バイト"
if run_section "$TMP/synthetic.log" TAKO_1660_FILE="$FIX/exact.rs" TAKO_1007_LEGACY=1; then
  pass "10 万行 / 10 MB でも節が緑"
else
  fail "10 万行 / 10 MB で節が落ちた"
fi

echo
echo "⑤ CLI / MCP: 上限の境目・改行の無い 1 行・CRLF"
launch_isolated_gui "$TMP/cli.log" || exit $?
CLI_GUI_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/cli.log" || exit 1
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
export TAKO_TOKEN
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])
')"
open_pane() { "$TAKO_BIN" open "$1" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane; }
mcp_call() { # ツール名 引数 JSON → 本文（isError なら "ERROR:" を前に付ける）
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1660","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        res = msg.get("result", {})
        text = res.get("content", [{}])[0].get("text", "")
        print(("ERROR:" if res.get("isError") else "") + text)
'
}

# (a) ちょうど上限（10 万行 / 10,000,000 バイト）は編集できる。先頭・中央・末尾を直して保存
P="$(open_pane "$FIX/exact.rs")"
OUT="$("$TAKO_BIN" edit start --pane "$P" 2>&1)"
check_eq "ちょうど上限（10 万行 / 10 MB）は編集モードに入れる" "true" "$(printf '%s' "$OUT" | jget editing)"
check_eq "上限の内側なので応答に limit が無い" "__MISSING__" "$(printf '%s' "$OUT" | jget limit)"
for L in 1 50000 100000; do
  "$TAKO_BIN" edit replace-range "$L:0" "$L:0" "EDIT" --pane "$P" >/dev/null 2>&1
done
SAVE="$("$TAKO_BIN" edit save --pane "$P" 2>&1)"
check_eq "保存できる（dirty が落ちる）" "false" "$(printf '%s' "$SAVE" | jget dirty)"
python3 - "$FIX/exact.rs" <<'PY' && pass "保存したファイルは 3 行だけ変わり、残りはバイト一致" || fail "保存したファイルが期待と違う"
import sys
data = open(sys.argv[1], "rb").read()
lines = data.split(b"\n")
assert len(data) == 10_000_000 + 12, len(data)
for n in (0, 49_999, 99_999):
    assert lines[n].startswith(b"EDITlet value_"), (n, lines[n][:20])
    head = f"let value_{n} = {n}; // ".encode()
    assert lines[n] == b"EDIT" + head + b"~" * (99 - len(head)), n
changed = [i for i, l in enumerate(lines[:-1]) if l.startswith(b"EDIT")]
assert changed == [0, 49_999, 99_999], changed
PY
UNDO="$("$TAKO_BIN" edit undo --pane "$P" 2>&1)"
check_eq "undo が通る（末尾の EDIT が消える）" "true" "$(printf '%s' "$UNDO" | jget dirty)"
"$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1

# (b) 上限 + 1 行は理由と値つきで断る（CLI と MCP で同じ文面）
P="$(open_pane "$FIX/over_lines.rs")"
CLI_ERR="$("$TAKO_BIN" edit start --pane "$P" 2>&1)"
echo "    CLI: $CLI_ERR"
case "$CLI_ERR" in
  *"100,001 行（上限 100,000 行）"*) pass "CLI の edit start が行数と上限を言って断る" ;;
  *) fail "CLI の edit start が理由と値を言わない: $CLI_ERR" ;;
esac
MCP_ERR="$(mcp_call tako_preview_edit "{\"pane\":$P,\"enabled\":true}")"
echo "    MCP: $MCP_ERR"
case "$MCP_ERR" in
  ERROR:*"100,001 行（上限 100,000 行）"*) pass "MCP の tako_preview_edit も同じ理由と値で断る（isError）" ;;
  *) fail "MCP が理由と値を言わない: $MCP_ERR" ;;
esac
check_eq "CLI と MCP の断りの文面が一致" "${CLI_ERR#error: }" "${MCP_ERR#ERROR:}"
STATUS="$("$TAKO_BIN" edit status --pane "$P" 2>&1)"
check_eq "edit status の limit.reason" "lines" "$(printf '%s' "$STATUS" | jget limit.reason)"
check_eq "edit status の limit.lines（数えた行数）" "100001" "$(printf '%s' "$STATUS" | jget limit.lines)"
check_eq "edit status の limit.max_lines" "100000" "$(printf '%s' "$STATUS" | jget limit.max_lines)"
MCP_STATUS="$(mcp_call tako_preview_edit "{\"pane\":$P}")"
check_eq "MCP の状態（enabled 省略）も同じ limit を返す" \
  "$(printf '%s' "$STATUS" | jget limit)" "$(printf '%s' "$MCP_STATUS" | jget limit)"
"$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1

# (c) 上限 + 1 バイトは大きさを理由に断る
P="$(open_pane "$FIX/over_bytes.txt")"
CLI_ERR="$("$TAKO_BIN" edit start --pane "$P" 2>&1)"
echo "    CLI: $CLI_ERR"
case "$CLI_ERR" in
  *"10 MB（上限 10 MB）"*) pass "上限 + 1 バイトは大きさと上限を言って断る" ;;
  *) fail "大きさの理由が出ない: $CLI_ERR" ;;
esac
STATUS="$("$TAKO_BIN" edit status --pane "$P" 2>&1)"
check_eq "edit status の limit.reason（大きさ）" "bytes" "$(printf '%s' "$STATUS" | jget limit.reason)"
check_eq "edit status の limit.bytes（実寸）" "10000001" "$(printf '%s' "$STATUS" | jget limit.bytes)"
"$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1

# (d) 改行の無い 10 MB の 1 行（ちょうど上限 = 編集できる）
P="$(open_pane "$FIX/one_line.txt")"
T0=$(python3 -c 'import time; print(time.time())')
OUT="$("$TAKO_BIN" edit start --pane "$P" 2>&1)"
check_eq "改行の無い 10 MB の 1 行は編集モードに入れる" "true" "$(printf '%s' "$OUT" | jget editing)"
"$TAKO_BIN" edit replace-range "1:10000000" "1:10000000" "END" --pane "$P" >/dev/null 2>&1
"$TAKO_BIN" edit save --pane "$P" >/dev/null 2>&1
T1=$(python3 -c 'import time; print(time.time())')
echo "    1 行 10 MB: 編集開始 → 末尾へ 3 文字 → 保存 に $(python3 -c "print(round(($T1 - $T0) * 1000))") ms（CLI の往復込み）"
python3 - "$FIX/one_line.txt" <<'PY' && pass "1 行 10 MB の末尾へ足して保存した内容がバイト一致" || fail "1 行 10 MB の保存内容が違う"
import sys
data = open(sys.argv[1], "rb").read()
assert data == b"a" * 10_000_000 + b"END", len(data)
PY
"$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1

# (e) CRLF の 10 万行: 中央の行を直して保存しても CR が残る（新しい改行も CRLF）
P="$(open_pane "$FIX/crlf.txt")"
"$TAKO_BIN" edit start --pane "$P" >/dev/null 2>&1
"$TAKO_BIN" edit replace-range "50000:0" "50000:0" "X" --pane "$P" >/dev/null 2>&1
"$TAKO_BIN" edit cursor "50000:1" --pane "$P" >/dev/null 2>&1
"$TAKO_BIN" edit newline --pane "$P" >/dev/null 2>&1
"$TAKO_BIN" edit save --pane "$P" >/dev/null 2>&1
python3 - "$FIX/crlf.txt" <<'PY' && pass "CRLF の 10 万行: 保存後も全行 CRLF・直した行だけ変わる" || fail "CRLF の保存内容が違う"
import sys
data = open(sys.argv[1], "rb").read()
assert data.count(b"\r\n") == 100_001, data.count(b"\r\n")
assert data.count(b"\n") == 100_001
lines = data.split(b"\r\n")
assert lines[49_999] == b"X", lines[49_999]
assert lines[50_000] == b"crlf line 49999", lines[50_000]
PY
"$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1
stop_isolated_gui "$CLI_GUI_PID"
CLI_GUI_PID=""

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
