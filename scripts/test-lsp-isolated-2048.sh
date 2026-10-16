#!/bin/bash
# test-lsp-isolated-2048.sh — 隔離 GUI では言語サーバを自動で起こさず・取らない（#2048）の実経路テスト
#
# 何を確かめるか（隔離 GUI・偽サーバ。言語サーバの有無は spawn の記録と GUI の子プロセスで見る）:
#   ① 新（明示の env なし）: .rs を編集しても言語サーバが起きない（spawn 0・GUI の子に居ない）。
#      `tako lsp status` は autostart=false・state=stopped・reason は隔離・MCP も同じ答え
#   ② 新: 明示の `tako lsp restart` で起きる（spawn 1・GUI の子に居る）
#   ③ 新 + TAKO_LSP_AUTOSTART=1: 従来どおり編集を始めた時点で起きる
#   ④ 旧（TAKO_2048_LEGACY=1）: 隔離でも編集を始めた時点で起きる（A/B）
#   ⑤ 取るほう: 起こす明示だけ（TAKO_LSP_AUTOSTART=1）では pyright を取りに行かない（取得の失敗の記録なし）、
#      旧と TAKO_LSP_AUTO_FETCH=1 は取りに行く（閉じたポートのミラー = 配布元へ出ずに network の失敗で分かる）
#   ⑥ 境界: 起こさなかった器を `tako lsp stop` したら理由は stop へ替わる・`tako lsp hover` は起こさず隔離の理由を返す
#
# 使い方: bash scripts/test-lsp-isolated-2048.sh
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。実の rust-analyzer は起こさない（偽サーバ = 検証の CPU を食わない）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE
# 呼び出し元の明示を持ち込まない（腕ごとに並べる）
unset TAKO_LSP_AUTOSTART TAKO_LSP_AUTO_FETCH TAKO_2048_LEGACY TAKO_LSP_FETCH_BASE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2048-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2048-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
echo "visual-test 版の tako-app と偽サーバをビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test \
  && run_heavy cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins || exit 1
FAKE="$(dirname "$TAKO_BIN")/tako-lsp-fake"
DUMP_NEW="${DUMP_NEW:-$TMP/dump-new}"
DUMP_OLD="${DUMP_OLD:-$TMP/dump-legacy}"

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc"
# GUI の PATH は最小にする（利用者の node / pyright を見せない = 取るかどうかだけが分かれる）
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
# CLI も同じ隔離の置き場を見る（GUI へは start_gui が並べて渡す）
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

mkdir -p "$TMP/proj/src" "$TMP/py"
printf '[package]\nname = "p"\n' > "$TMP/proj/Cargo.toml"
printf 'fn main() {}\n' > "$TMP/proj/src/main.rs"
printf 'print(1)\n' > "$TMP/py/main.py"

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
lines() { [ -f "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }
status_field() { # サーバ ID 式 — そのサーバの status の値
  "$TAKO_BIN" lsp status --json 2>/dev/null |
    json "[s for s in d['servers'] if s['id'] == '$1'][0]$2"
}
# GUI の子孫に偽サーバが居るか（自分が立てた GUI の pid から辿る。pkill / 名前一致の全体走査はしない）
fake_children() {
  local pid queue next found=0
  queue="$APP_PID"
  while [ -n "$queue" ]; do
    next=""
    for pid in $queue; do
      for c in $(pgrep -P "$pid" 2>/dev/null); do
        case "$(ps -o command= -p "$c" 2>/dev/null)" in *tako-lsp-fake*) found=$((found + 1)) ;; esac
        next="$next $c"
      done
    done
    queue="$next"
  done
  echo "$found"
}
wait_for() { # 秒数 コマンド…
  local secs="$1" n=0
  shift
  while [ "$n" -lt $((secs * 10)) ]; do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.1
    n=$((n + 1))
  done
  return 1
}
spawned() { [ "$(lines "$SPAWNS")" -ge 1 ]; }
has_child() { [ "$(fake_children)" -ge 1 ]; }
fetch_failed() { [ -n "$(status_field pyright '.get("fetch_error") or ""')" ]; }

start_gui() { # 名前 追加の env…
  local name="$1"
  shift
  export TAKO_DATA_DIR="$TMP/data-$name"
  mkdir -p "$TAKO_DATA_DIR"
  SPAWNS="$TMP/spawns-$name.txt"
  launch_isolated_gui "$TMP/app-$name.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DISCOVERY_DIR="$TMP/disc" \
    TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
    TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
    TAKO_TMUX_SOCKET="$TMUX_SOCKET" TAKO_DATA_DIR="$TAKO_DATA_DIR" \
    TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" TAKO_LSP_FAKE_SPAWNS="$SPAWNS" \
    TAKO_LSP_FAKE_LOG="$TMP/fake-$name.jsonl" \
    TAKO_LSP_FETCH_BASE="http://127.0.0.1:9" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$name.log" || { echo "GUI が立たない"; exit 1; }
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$ROOT" ] || { echo "ペインが取れない"; exit 1; }
}
stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
open_edit() { # ファイル → ペイン ID（編集モードへ入れて 1 文字打つ）
  local pane
  pane="$("$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
  [ -n "$pane" ] || return 1
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  "$TAKO_BIN" edit replace-range 1:1 1:1 " " --pane "$pane" >/dev/null 2>&1
  echo "$pane"
}

echo "== ① 新: 隔離 GUI（明示の env なし）で .rs を編集しても起きない =="
start_gui new
PANE="$(open_edit "$TMP/proj/src/main.rs")"
# 起こす版なら偽サーバは 1 秒以内に spawn する（③④ の実測）。その 3 倍を待つ
sleep 3
check_eq "spawn の記録が 0" "0" "$(lines "$SPAWNS")"
check_eq "GUI の子に偽サーバが居ない" "0" "$(fake_children)"
check_eq "status の autostart は偽" "False" "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["autostart"]')"
check_eq "器は stopped" "stopped" "$(status_field rust-analyzer '["state"]')"
check_eq "理由は隔離" "隔離起動（TAKO_ISOLATED）なので自動では起こさない" "$(status_field rust-analyzer '["reason"]')"
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_STATE="$(printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-2048","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_lsp_server","arguments":{"action":"status"}}}' |
  env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null |
  python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        t = json.loads(m["result"]["content"][0]["text"])
        print(t["autostart"], t["servers"][0]["state"], t["servers"][0]["reason"]); break' 2>/dev/null)"
check_eq "MCP tako_lsp_server も同じ答え" "False stopped 隔離起動（TAKO_ISOLATED）なので自動では起こさない" "$MCP_STATE"
echo "    次の一手: $(status_field rust-analyzer '["next_step"]')"

echo
echo "== ⑥ 境界: 起こさなかった器への問い合わせは起こさず理由を返す =="
HOVER="$("$TAKO_BIN" lsp hover --pane "$PANE" --line 1 --column 4 --json 2>/dev/null)"
check_eq "hover は unavailable" "unavailable" "$(printf '%s' "$HOVER" | json 'd.get("status")')"
check_eq "hover の理由も隔離" "隔離起動（TAKO_ISOLATED）なので自動では起こさない" "$(printf '%s' "$HOVER" | json 'd.get("reason")')"
check_eq "問い合わせでも spawn 0" "0" "$(lines "$SPAWNS")"

echo
echo "== ② 新: 明示の restart で起きる =="
"$TAKO_BIN" lsp restart >/dev/null 2>&1
if wait_for 10 spawned; then pass "restart で spawn した"; else fail "restart で spawn しない"; fi
if wait_for 10 has_child; then pass "GUI の子に偽サーバが居る（$(fake_children) 本）"; else fail "GUI の子に偽サーバが居ない"; fi
if wait_for 10 sh -c "[ \"\$(\"$TAKO_BIN\" lsp status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)[\"servers\"][0][\"state\"])')\" = running ]"; then
  pass "状態は running"
else
  fail "running にならない（$(status_field rust-analyzer '["state"]')）"
fi
stop_gui

echo
echo "== ⑥ 境界: 起こさなかった器を stop したら理由は stop へ替わる =="
start_gui stop
open_edit "$TMP/proj/src/main.rs" >/dev/null
"$TAKO_BIN" lsp stop >/dev/null 2>&1
check_eq "stop の後の理由" "tako lsp stop で止めた" "$(status_field rust-analyzer '["reason"]')"
sleep 1
check_eq "stop の後も spawn 0" "0" "$(lines "$SPAWNS")"
stop_gui

echo
echo "== ③ 新 + TAKO_LSP_AUTOSTART=1: 従来どおり編集を始めた時点で起きる =="
start_gui explicit TAKO_LSP_AUTOSTART=1
T0=$(python3 -c 'import time; print(time.time())')
open_edit "$TMP/proj/src/main.rs" >/dev/null
if wait_for 10 spawned; then
  pass "編集を始めた時点で spawn（$(python3 -c "import time; print(round(time.time() - $T0, 2))") 秒）"
else
  fail "spawn しない"
fi
if wait_for 10 has_child; then pass "GUI の子に偽サーバが居る"; else fail "GUI の子に偽サーバが居ない"; fi
check_eq "status の autostart は真" "True" "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["autostart"]')"
echo
echo "== ⑤ 取るほう（起こす明示だけ）: pyright を取りに行かない =="
open_edit "$TMP/py/main.py" >/dev/null
if wait_for 10 sh -c "[ \"\$(\"$TAKO_BIN\" lsp status --json 2>/dev/null | python3 -c 'import json,sys; print([s for s in json.load(sys.stdin)[\"servers\"] if s[\"id\"] == \"pyright\"][0][\"state\"])')\" = not_installed ]"; then
  pass "pyright は not_installed"
else
  fail "pyright が not_installed にならない（$(status_field pyright '["state"]')）"
fi
check_eq "取得の失敗の記録が無い（取りに行っていない）" "" "$(status_field pyright '.get("fetch_error") or ""')"
check_eq "status の fetch.auto は偽" "False" "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["fetch"]["auto"]')"
stop_gui

echo
echo "== ⑤ 取るほう（TAKO_LSP_AUTO_FETCH=1）: 取りに行く（閉じたポートのミラーで network の失敗） =="
start_gui fetch TAKO_LSP_AUTOSTART=1 TAKO_LSP_AUTO_FETCH=1
open_edit "$TMP/py/main.py" >/dev/null
if wait_for 20 fetch_failed; then
  pass "取りに行った（fetch_error=$(status_field pyright '.get("fetch_error")')）"
else
  fail "取りに行かない（$(status_field pyright '["state"]')）"
fi
stop_gui

echo
echo "== ④ 旧（TAKO_2048_LEGACY=1）: 隔離でも編集を始めた時点で起きて取る =="
start_gui legacy TAKO_2048_LEGACY=1
open_edit "$TMP/proj/src/main.rs" >/dev/null
if wait_for 10 spawned; then pass "旧は明示なしでも spawn する（A/B）"; else fail "旧が spawn しない"; fi
if wait_for 10 has_child; then pass "旧は GUI の子に偽サーバが居る"; else fail "旧の GUI の子に偽サーバが居ない"; fi
open_edit "$TMP/py/main.py" >/dev/null
if wait_for 20 fetch_failed; then
  pass "旧は明示なしでも取りに行く（fetch_error=$(status_field pyright '.get("fetch_error")')）"
else
  fail "旧が取りに行かない（$(status_field pyright '["state"]')）"
fi
stop_gui

echo
echo "== ⑦ 画面（visual-test 節 lsp-held-2048）: 新はタイトルの 1 行だけ・旧は名指しの FAILED =="
run_visual() { # 名前 画像の置き場 追加の env…
  local name="$1" dump="$2" i
  shift 2
  export TAKO_DATA_DIR="$TMP/data-vis-$name"
  mkdir -p "$TAKO_DATA_DIR" "$dump"
  launch_isolated_gui "$TMP/vis-$name.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=lsp-held-2048 \
    TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  for i in $(seq 1 1200); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_gui
  grep "TAKO_VISUAL_PIXEL: lsp-held-2048\|TAKO_VISUAL_DUMP_FILE\|TAKO_APP_SELF_TEST_FAILED" "$TMP/vis-$name.log" |
    sed "s|$TMP|\$TMP|g; s/^/    /"
}
run_visual new "$DUMP_NEW"
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vis-new.log"; then
  pass "新: 節が緑（起こさない・1 行・帯なし）"
else
  fail "新: 節が緑にならない"
fi
run_visual legacy "$DUMP_OLD" TAKO_2048_LEGACY=1
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test lsp-held-2048: 隔離なのに言語サーバを起こした" "$TMP/vis-legacy.log"; then
  pass "旧: 名指しの FAILED（隔離なのに言語サーバを起こした = A/B）"
else
  fail "旧: 名指しの FAILED にならない"
fi
# 画像はスクリプトの後片付けで消えるので、残したいときは DUMP_NEW / DUMP_OLD を外の置き場へ向ける

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
