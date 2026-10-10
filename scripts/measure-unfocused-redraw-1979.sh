#!/bin/bash
# measure-unfocused-redraw-1979.sh — フォーカスの無いペインの再描画の上限（#1979）の CPU の前後を測る
#
# 同じ release バイナリを A/B（`TAKO_1979_LEGACY=1` = 上限なし / 既定 = 30 fps）で交互に起こし、
# 同じ条件（フォーカスは何も出さないペイン・隣の 2 ペインが 1 行 10ms で出力し続ける）で
# tako-app の CPU 時間と再描画の回数を測る。判定はしない（数値を出すだけ。判定は
# `scripts/test-ui-thread-wait-1979.sh` の③が「上限以下か」で行う）。
#
# 出力側は 1 行 10ms（100 行 / 秒）の python 1 本ずつ = 1 コアも使わない量にとどめる
# （CPU を焼く負荷生成はしない）。隔離 GUI・tako-vd・本番の tako / 設定には触らない。
#
# 出力の形は 3 つ目の引数で選ぶ: `lines`（1 行 10ms で流す = 既定）/ `tui`（全画面を色付きで
# 約 37 fps で描き直す = 本番で CPU を食っていたエージェントの TUI に近い形）。
#
# **蓋を閉じた機（内蔵ディスプレイが無い）ではフレームが組まれない**: 仮想ディスプレイ上の窓の
# display link が動かず、`ペイン本体の描画` が 0 回/秒になる（2026-10-10 の実測）。そのとき出る
# CPU は描画を含まない（出力の取り込みだけ）ので、描画の CPU の前後は蓋を開けた機で測る。
# 面が描いているかはこの出力の `ペイン本体の描画` で分かる。
#
# 使い方: bash scripts/measure-unfocused-redraw-1979.sh [秒（既定 60）] [往復（既定 2）] [lines|tui]
set -uo pipefail
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SECS="${1:-60}"
ROUNDS="${2:-2}"
KIND="${3:-lines}"
TMP="$(mktemp -d /tmp/tako-1979m-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "tako-1979m-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

export TAKO_ISO_PROFILE=release
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
(cd "$REPO_ROOT" && cargo build --release -q -p tako-cli -p tako-app) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1979m-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"

# プロセスの CPU 時間（秒）。`ps -o time=` は [時:]分:秒.百分の一
cpu_secs() {
  /bin/ps -o time= -p "$1" | python3 -c '
import sys
t = sys.stdin.read().strip()
parts = [float(x) for x in t.replace("-", ":").split(":")]
s = 0.0
for p in parts: s = s * 60 + p
print(f"{s:.2f}")'
}

GEN_LINES='python3 -u -c "import time,itertools
for i in itertools.count():
    print(i, time.time(), flush=True); time.sleep(0.01)"'
# 全画面を色付きで描き直す（カーソルを左上へ戻して全行を上書き。1 コマ 27ms ≒ 37 fps）
GEN_TUI='python3 -u -c "import sys,time,shutil,itertools
for i in itertools.count():
    c,r=shutil.get_terminal_size()
    rows=[chr(27)+\"[3%dm\"%((n+i)%7+1)+(\"%08d \"%(i+n)*(c//9+1))[:c] for n in range(max(r-1,1))]
    sys.stdout.write(chr(27)+\"[H\"+(chr(27)+\"[0m\\r\\n\").join(rows)+chr(27)+\"[0m\"); sys.stdout.flush(); time.sleep(0.027)"'
case "$KIND" in
  tui) GEN="$GEN_TUI" ;;
  *) GEN="$GEN_LINES" ;;
esac

run_arm() {
  local label="$1"; shift
  # 器（tmux の永続化）は本番の既定と同じく ON（隔離 GUI の既定は OFF）
  launch_isolated_gui "$TMP/app-$label.log" TAKO_PERSIST=1 "$@" || exit $?
  wait_isolated_gui "$TMP/app-$label.log" 600 || exit 1
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
  local first
  first="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
  "$TAKO_BIN" split --pane "$first" --right -- /bin/sh -c "$GEN" >/dev/null
  "$TAKO_BIN" split --pane "$first" --down -- /bin/sh -c "$GEN" >/dev/null
  "$TAKO_BIN" focus "$first" >/dev/null
  # 測っているあいだ面を眠らせない（蓋閉じの tako-vd はアイドルで眠り、眠ると display link が
  # 止まって**描画そのものが起きない** = 描画の CPU を測れない。初回の実測で両腕の差が消えた）。
  # `caffeinate -u` はユーザー活動の宣言だけ（自分の pid だけを後で落とす）
  caffeinate -u -t $((SECS + 30)) >/dev/null 2>&1 &
  local caff=$!
  sleep 5
  local c0 c1 r0 r1 b0 b1 drawable
  c0="$(cpu_secs "$ISOLATED_GUI_PID")"
  r0="$("$TAKO_BIN" redraw-limit | python3 -c 'import json,sys; d=json.load(sys.stdin)["flushes"]; print(d["focused"] + d["unfocused"])')"
  b0="$("$TAKO_BIN" redraw-limit | python3 -c 'import json,sys; print(json.load(sys.stdin)["body_renders"])')"
  sleep "$SECS"
  c1="$(cpu_secs "$ISOLATED_GUI_PID")"
  r1="$("$TAKO_BIN" redraw-limit | python3 -c 'import json,sys; d=json.load(sys.stdin)["flushes"]; print(d["focused"] + d["unfocused"])')"
  b1="$("$TAKO_BIN" redraw-limit | python3 -c 'import json,sys; print(json.load(sys.stdin)["body_renders"])')"
  drawable="描画可能"
  "$REPO_ROOT/scripts/lib/virtual-display.sh" status 2>/dev/null | head -1 | grep -q '眠っている' && drawable="眠っていた"
  kill "$caff" 2>/dev/null
  python3 -c "
cpu = ($c1 - $c0) / $SECS * 100
fps = ($r1 - $r0) / $SECS
body = ($b1 - $b0) / $SECS
print(f'  {\"$label\":<10} tako-app CPU {cpu:5.2f}%  出力起因の再描画 {fps:5.1f} 回/秒・ペイン本体の描画 {body:5.1f} 回/秒（{$SECS} 秒・面は ${drawable}）')"
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
  sleep 3
}

echo "フォーカスの無い 2 ペインが出力し続ける間の tako-app（release・tako-vd・出力の形 ${KIND}）"
for i in $(seq 1 "$ROUNDS"); do
  run_arm "legacy-$i" TAKO_1979_LEGACY=1
  run_arm "new-$i"
done
