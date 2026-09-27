#!/bin/bash
# test-lsp-goto-1680.sh — 定義ジャンプ（#1680）の実経路テスト（隔離 GUI・偽サーバ）
#
# 何を確かめるか（Issue #1680 の受け入れ条件のうち、CLI / MCP から GUI を通して言えるもの）:
#   ① 別ファイルの定義 → 新しいペインが 1 枚増え、その行へ着地する
#   ② 同じ定義へもう一度 → ペインは増えない（使い回し）
#   ③ 同じファイルの定義 → 同じペインのまま（読み直さない）。編集モードも抜けない
#   ④ `#include "foo.h"` の行 → ヘッダが新しいペインで開く
#   ⑤ `tako jump back` で問い合わせた位置へ戻る（ジャンプ履歴へ積まれている）
#   ⑥ 候補が複数 → status=choose と一覧、`--choice 2` でその場所へ
#   ⑦ 見つからない / 未応答（上限つき）/ 未導入 が別の status と別の理由
#   ⑧ CLI `tako lsp definition` と MCP `tako_lsp` が同じ答え（`--open none` で比べる）
#   ⑨ 多バイト文字（日本語・絵文字）の後ろの識別子は UTF-16 の桁で問い合わせる
#   ⑩ 未応答を待っているあいだも他の CLI は待たされない（UI をブロックしない）
#
# 使い方: bash scripts/test-lsp-goto-1680.sh
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

TMP="$(mktemp -d /tmp/tako-1680-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1680-$$"
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
export TAKO_DATA_DIR="$TMP/data"
mkdir -p "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DATA_DIR"

# --- 材料 -----------------------------------------------------------------------
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
printf -- '-xc\n' > "$P/compile_flags.txt"
{
  printf 'fn main() {\n'
  printf '    helper();\n'
  printf '    local();\n'
  printf '    multi();\n'
  printf '    let 名前 = 😀; target();\n'
  printf '    slow();\n'
  printf '}\n'
  n=7; while [ "$n" -lt 40 ]; do printf '// pad %s\n' "$n"; n=$((n + 1)); done
  printf 'fn local() {}\n'
} > "$P/src/main.rs"
{
  n=0; while [ "$n" -lt 60 ]; do
    case "$n" in
      10) printf 'pub fn multi() {} // a\n' ;;
      20) printf 'pub fn multi() {} // b\n' ;;
      30) printf 'pub fn helper() {}\n' ;;
      45) printf 'pub fn target() {}\n' ;;
      *) printf '// other %s\n' "$n" ;;
    esac
    n=$((n + 1))
  done
} > "$P/src/other.rs"
printf '#include "foo.h"\nint main(void) { return foo(); }\n' > "$P/src/main.c"
printf '#pragma once\nint foo(void);\n' > "$P/src/foo.h"
printf 'x = 1\n' > "$P/src/tool.py"
# 実パス（macOS の /var → /private/var）で URI を組む = tako の応答と同じ綴り
REAL="$(cd "$P" && pwd -P)"

LOG="$TMP/fake.jsonl"
RULES="$TMP/goto.json"
python3 - "$REAL" "$RULES" <<'PY'
import json, sys, urllib.parse
real, out = sys.argv[1], sys.argv[2]
def loc(rel, line, ch):
    uri = "file://" + urllib.parse.quote(f"{real}/{rel}")
    return {"uri": uri, "range": {"start": {"line": line, "character": ch}, "end": {"line": line, "character": ch + 1}}}
rules = [
    {"uri_suffix": "main.rs", "line": 1, "result": loc("src/other.rs", 30, 7)},
    {"uri_suffix": "main.rs", "line": 2, "result": loc("src/main.rs", 39, 3)},
    {"uri_suffix": "main.rs", "line": 3, "result": [loc("src/other.rs", 10, 7), loc("src/other.rs", 20, 7)]},
    {"uri_suffix": "main.rs", "line": 4, "result": loc("src/other.rs", 45, 7)},
    {"uri_suffix": "main.rs", "line": 5, "silent": True},
    {"uri_suffix": "main.c", "line": 0, "result": loc("src/foo.h", 0, 0)},
]
json.dump(rules, open(out, "w"))
PY

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
panes() { "$TAKO_BIN" list 2>/dev/null | json 'sum(len(t["panes"]) for t in d["tabs"])'; }
goto() { "$TAKO_BIN" lsp definition "$@" --json 2>/dev/null; }

echo "== 起動（偽サーバを rust-analyzer / clangd の代わりに・pyright は未導入・上限 3 秒） =="
launch_isolated_gui "$TMP/app.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_BIN_CLANGD="$FAKE" \
  TAKO_LSP_BIN_PYRIGHT="$TMP/no-such-pyright" \
  TAKO_LSP_FAKE_GOTO="$RULES" \
  TAKO_LSP_FAKE_LOG="$LOG" \
  TAKO_LSP_GOTO_TIMEOUT_SECS=3 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
[ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
SRC="$("$TAKO_BIN" open "$P/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$SRC" ] || { echo "main.rs を開けない"; exit 1; }
echo "  pid=$APP_PID root=$ROOT source=$SRC"

echo
echo "== ① 別ファイルの定義 → 新しいペイン =="
BEFORE="$(panes)"
R1="$(goto --pane "$SRC" --line 2 --column 4)"
check_eq "status" "found" "$(printf '%s' "$R1" | json 'd["status"]')"
check_eq "landing" "new-pane" "$(printf '%s' "$R1" | json 'd["landing"]')"
check_eq "ペインが 1 枚増える" "$((BEFORE + 1))" "$(panes)"
check_eq "着地した行（1 始まり）" "31" "$(printf '%s' "$R1" | json 'd["open"]["line"]')"
check_eq "飛び先は other.rs" "True" "$(printf '%s' "$R1" | json 'd["open"]["path"].endswith("other.rs")')"
TARGET="$(printf '%s' "$R1" | json 'd["open"]["pane"]')"

echo
echo "== ② 同じ定義へもう一度 → 使い回し =="
R2="$(goto --pane "$SRC" --line 2 --column 4)"
check_eq "landing" "reused" "$(printf '%s' "$R2" | json 'd["landing"]')"
check_eq "同じペイン" "$TARGET" "$(printf '%s' "$R2" | json 'd["open"]["pane"]')"
check_eq "ペイン数は増えない" "$((BEFORE + 1))" "$(panes)"

echo
echo "== ⑤ tako jump back で問い合わせた位置へ =="
BACK="$("$TAKO_BIN" jump back --pane "$SRC" --json 2>/dev/null)"
check_eq "戻った先は問い合わせたペイン" "$SRC" "$(printf '%s' "$BACK" | json 'd["open"]["pane"]')"
check_eq "戻った先は main.rs" "True" "$(printf '%s' "$BACK" | json 'd["open"]["path"].endswith("main.rs")')"

echo
echo "== ③ 同じファイルの定義 → 同じペイン・読み直さない・編集モードのまま =="
"$TAKO_BIN" edit start --pane "$SRC" >/dev/null
R3="$(goto --pane "$SRC" --line 3 --column 4)"
check_eq "landing" "same-pane" "$(printf '%s' "$R3" | json 'd["landing"]')"
check_eq "読み直さない" "False" "$(printf '%s' "$R3" | json 'd["open"]["reloaded"]')"
check_eq "着地した行" "40" "$(printf '%s' "$R3" | json 'd["open"]["line"]')"
check_eq "ペイン数は増えない" "$((BEFORE + 1))" "$(panes)"
check_eq "編集モードは抜けていない" "True" "$("$TAKO_BIN" edit status --pane "$SRC" 2>/dev/null | json 'd.get("editing")')"
"$TAKO_BIN" edit stop --pane "$SRC" >/dev/null 2>&1 || true

echo
echo "== ⑥ 候補が複数 → 一覧、--choice 2 でその場所へ =="
R6="$(goto --pane "$SRC" --line 4 --column 4)"
check_eq "status" "choose" "$(printf '%s' "$R6" | json 'd["status"]')"
check_eq "候補は 2 か所" "2" "$(printf '%s' "$R6" | json 'len(d["locations"])')"
check_eq "開かない" "False" "$(printf '%s' "$R6" | json '"open" in d')"
R6B="$(goto --pane "$SRC" --line 4 --column 4 --choice 2)"
check_eq "選んだ候補の行" "21" "$(printf '%s' "$R6B" | json 'd["open"]["line"]')"
check_eq "使い回し" "$TARGET" "$(printf '%s' "$R6B" | json 'd["open"]["pane"]')"
CHOICE_OUT="$("$TAKO_BIN" lsp definition --pane "$SRC" --line 4 --column 4 --choice 9 2>&1)"
case "$CHOICE_OUT" in *"候補"*) pass "候補の外の --choice は拒否（$(printf '%s' "$CHOICE_OUT" | head -1)）" ;; *) fail "候補の外の --choice: $CHOICE_OUT" ;; esac

echo
echo "== ⑨ 多バイト文字の後ろの識別子は UTF-16 の桁で問い合わせる =="
# `    let 名前 = 😀; ` = UTF-8 で 4+4+6+3+4+2 = 23 バイト、UTF-16 で 4+4+2+3+2+2 = 17
R9="$(goto --pane "$SRC" --line 5 --column 23 --open none)"
check_eq "status" "found" "$(printf '%s' "$R9" | json 'd["status"]')"
ASKED="$(python3 -c 'import json,sys
last = None
for l in open(sys.argv[1]):
    m = json.loads(l)
    if m.get("method") == "textDocument/definition" and m["params"]["position"]["line"] == 4:
        last = m["params"]["position"]["character"]
print(last)' "$LOG")"
check_eq "送った桁は UTF-16" "17" "$ASKED"
MID="$("$TAKO_BIN" lsp definition --pane "$SRC" --line 5 --column 9 --json 2>&1)"
case "$MID" in *"境界"*) pass "文字の途中の桁は丸めずに拒否" ;; *) fail "文字の途中の桁: $MID" ;; esac

echo
echo "== ④ #include のヘッダ → 新しいペイン =="
CPANE="$("$TAKO_BIN" open "$P/src/main.c" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
BEFORE4="$(panes)"
R4="$(goto --pane "$CPANE" --line 1 --column 10)"
check_eq "landing" "new-pane" "$(printf '%s' "$R4" | json 'd["landing"]')"
check_eq ".h が開く" "True" "$(printf '%s' "$R4" | json 'd["open"]["path"].endswith("foo.h")')"
check_eq "ペインが 1 枚増える" "$((BEFORE4 + 1))" "$(panes)"

echo
echo "== ⑦ 見つからない / 未応答 / 未導入 を区別する =="
NF="$(goto --pane "$SRC" --line 1 --column 3 --open none)"
PY="$("$TAKO_BIN" open "$P/src/tool.py" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
NI="$(goto --pane "$PY" --line 1 --column 0 --open none)"
# ⑩ 未応答を待つあいだ（上限 3 秒）も他の CLI は待たされない
( goto --pane "$SRC" --line 6 --column 4 --open none > "$TMP/timeout.json" ) &
WAITER=$!
sleep 0.5
T0="$(python3 -c 'import time; print(time.time())')"
"$TAKO_BIN" list >/dev/null 2>&1
T1="$(python3 -c 'import time; print(time.time())')"
wait "$WAITER"
TO="$(cat "$TMP/timeout.json")"
check_eq "見つからない" "not-found" "$(printf '%s' "$NF" | json 'd["status"]')"
check_eq "未応答" "timeout" "$(printf '%s' "$TO" | json 'd["status"]')"
check_eq "未導入" "not-installed" "$(printf '%s' "$NI" | json 'd["status"]')"
REASONS="$(printf '%s\n%s\n%s\n' "$(printf '%s' "$NF" | json 'd["reason"]')" "$(printf '%s' "$TO" | json 'd["reason"]')" "$(printf '%s' "$NI" | json 'd["reason"]')")"
echo "$REASONS" | sed 's/^/    /'
check_eq "3 つの理由は互いに違う" "3" "$(printf '%s\n' "$REASONS" | sort -u | grep -c .)"
check_eq "未導入は導入コマンドを返す" "npm install -g pyright" "$(printf '%s' "$NI" | json 'd["install_command"]')"
LIST_MS="$(python3 -c "print(int(($T1 - $T0) * 1000))")"
echo "    未応答を待つあいだの tako list: ${LIST_MS}ms"
if [ "$LIST_MS" -lt 1500 ]; then pass "問い合わせ中も UI は止まらない（${LIST_MS}ms）"; else fail "tako list が ${LIST_MS}ms 待たされた"; fi

echo
echo "== ⑧ CLI と MCP が同じ答え（--open none） =="
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数の JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1680","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp\",\"arguments\":$1}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }
for spec in "2 4" "4 4" "1 3"; do
  set -- $spec
  CLI="$(goto --pane "$SRC" --line "$1" --column "$2" --open none)"
  MCP="$(mcp "{\"action\":\"definition\",\"pane\":$SRC,\"line\":$1,\"column\":$2,\"open\":\"none\"}")"
  if same_json "$CLI" "$MCP"; then pass "行 $1 桁 $2 が CLI と MCP で字面一致（status=$(printf '%s' "$CLI" | json 'd["status"]')）"; else fail "行 $1 桁 $2 が食い違う: CLI=$CLI MCP=$MCP"; fi
done
# MCP から開く（CLI と同じ着地の規則）
M1="$(mcp "{\"action\":\"definition\",\"pane\":$SRC,\"line\":2,\"column\":4}")"
check_eq "MCP の着地も使い回し" "reused" "$(printf '%s' "$M1" | json 'd["landing"]')"

echo
echo "== 後始末: 一時的に開いた文書は閉じている =="
check_eq "LSP の文書数（編集モードは抜けた）" "0" "$("$TAKO_BIN" lsp status --json | json 'd["documents"]')"
stop_isolated_gui "$APP_PID"; APP_PID=""

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
