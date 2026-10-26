#!/bin/bash
# measure-hidden-delivery-2344.sh — 出力イベントがメインスレッドを何回起こしているかを腕ごとに測る（#2344）
#
# 同じ流し方・同じ秒数で、画面に出ていないペイン（裏タブ）とフォーカスの無い可視ペイン（表のタブ）の
# 配送の記録（`tako redraw-limit` の `delivery` = hops / forced / skipped）の差を取り、秒あたりで出す。
# 判定はしない（数値を出すだけ。判定はセルフテスト項目 113）。
#
# 腕:
#   new    = 今の木（既定の分類 = Wakeup / CursorBlinkingChange / MouseCursorDirty が描き直しの合図）
#   legacy = 同じバイナリの TAKO_2344_LEGACY=1（#2344 前の分類 = Wakeup だけが合図）
#   pre    = TAKO_2344_PRE_DIR に置いた修正前のバイナリ（`tako` と `tako-app`）。置いたときだけ走る
#
# 流し方は 3 つ目の引数で選ぶ:
#   lines   = 1 行 30ms で流し続ける（既定。予定時刻に合わせて sleep するので間隔は機の混み具合で流れない）
#   toggles = 本文を出さず、カーソルの形（DECSCUSR）・点滅（?12）・マウス要求（?1000）の切替だけを
#             200ms ごとに出す（見えているペインでこれまでどおり描き直されるかの確認。#2344 の 3）
# 器（tmux の永続化）は本番の既定と同じく ON。TAKO_2344_PERSIST=0 で直接ペイン（器なし）にする。
#
# 出力側は python 1 本ずつ = 1 コアも使わない量にとどめる（CPU を焼く負荷生成はしない）。
# 隔離 GUI・tako-vd・本番の tako / 設定には触らない。腕ごとに data dir / discovery / 器のソケットを分ける
# （永続化 ON で同じ dir を使うと、2 本目の起動が前の腕の配置と器を復元して数が混ざる）。
#
# 使い方: bash scripts/measure-hidden-delivery-2344.sh [秒（既定 20）] [往復（既定 2）] [lines|toggles]
set -uo pipefail
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
SECS="${1:-20}"
ROUNDS="${2:-2}"
KIND="${3:-lines}"
PERSIST="${TAKO_2344_PERSIST:-1}"
PRE_DIR="${TAKO_2344_PRE_DIR:-}"
TMP="$(mktemp -d /tmp/tako-2344m-XXXXXX)"
SOCKETS=""
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  local s
  for s in $SOCKETS; do
    tmux_socket_kill "$s"
  done
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
NEW_TAKO_BIN="$TAKO_BIN"
NEW_APP_BIN="$APP_BIN"
if [ -n "$PRE_DIR" ]; then
  for b in tako tako-app; do
    [ -x "$PRE_DIR/$b" ] || { echo "ERROR: 修正前のバイナリが無い: $PRE_DIR/$b" >&2; exit 1; }
  done
fi

export HOME="$TMP/home"
# CLI も隔離側だけを見る（data dir だけを差し替えると spawn 系が本番の GUI を触りうる）
export TAKO_ISOLATED=1
mkdir -p "$HOME"

# 1 行 30ms。予定時刻（開始 + n × 30ms）まで眠るので、1 行の処理が遅れても間隔は詰めて取り戻す
GEN_LINES='python3 -u -c "import time,itertools
t0=time.monotonic()
for i in itertools.count():
    print(\"l%d\" % i, flush=True); time.sleep(max(0.0, t0+(i+1)*0.03-time.monotonic()))"'
# 本文を出さずに形と要求だけを切り替える（200ms ごと。4 種を巡回）
GEN_TOGGLES='python3 -u -c "import sys,time,itertools
seqs=[chr(27)+\"[6 q\", chr(27)+\"[2 q\", chr(27)+\"[?12h\", chr(27)+\"[?12l\", chr(27)+\"[?1000h\", chr(27)+\"[?1000l\"]
t0=time.monotonic()
for i in itertools.count():
    sys.stdout.write(seqs[i % len(seqs)]); sys.stdout.flush(); time.sleep(max(0.0, t0+(i+1)*0.2-time.monotonic()))"'
case "$KIND" in
  toggles) GEN="$GEN_TOGGLES" ;;
  *) GEN="$GEN_LINES" ;;
esac

# `tako redraw-limit` の JSON から 1 ペインの「hops forced skipped hidden」と、全体の再描画の要求回数を 1 行で
read_delivery() {
  "$TAKO_BIN" redraw-limit | python3 -c '
import json, sys
d = json.load(sys.stdin)
want = {int(x) for x in sys.argv[1:]}
rows = {r["pane"]: r for r in d.get("delivery", [])}
out = []
for p in sorted(want):
    r = rows.get(p, {"hops": 0, "forced": 0, "skipped": 0, "hidden": False})
    out.append("%d:%d:%d:%d:%d" % (p, r["hops"], r["forced"], r["skipped"], 1 if r["hidden"] else 0))
f = d["flushes"]
print(" ".join(out), f["focused"] + f["unfocused"])
' "$@"
}

run_arm() {
  local label="$1" bin_dir="$2"; shift 2
  local dir="$TMP/$label"
  local sock="tako-2344m-$$-$label"
  SOCKETS="$SOCKETS $sock"
  mkdir -p "$dir/data" "$dir/disc" "$dir/orch"
  if [ -n "$bin_dir" ]; then
    TAKO_BIN="$bin_dir/tako"
    APP_BIN="$bin_dir/tako-app"
  else
    TAKO_BIN="$NEW_TAKO_BIN"
    APP_BIN="$NEW_APP_BIN"
  fi
  export TAKO_DATA_DIR="$dir/data" TAKO_DISCOVERY_DIR="$dir/disc" TAKO_ORCHESTRATOR_DIR="$dir/orch"
  export TAKO_SESSIONS_FILE="$dir/sessions.yaml" TAKO_PANE_LOG_DIR="$dir/panelogs"
  export TAKO_WORKERS_FILE="$dir/workers.yaml" TAKO_TMUX_SOCKET="$sock"
  launch_isolated_gui "$dir/app.log" TAKO_PERSIST="$PERSIST" "$@" || exit $?
  wait_isolated_gui "$dir/app.log" 600 || exit 1
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
  local first visible hidden_tab hidden
  first="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
  # 表のタブ: フォーカスは何も出さない最初のペインに置いたまま、隣の可視ペインへ流す
  visible="$("$TAKO_BIN" split --pane "$first" --right -- /bin/sh -c "$GEN")"
  # 裏タブ: `tab new` は切り替えない（= 裏のまま）。その中で分割して流す
  hidden_tab="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])')"
  hidden="$("$TAKO_BIN" split --pane "$hidden_tab" --down -- /bin/sh -c "$GEN")"
  "$TAKO_BIN" focus "$first" >/dev/null
  # 測っているあいだ面を眠らせない（自分の pid だけを後で落とす）
  caffeinate -u -t $((SECS + 30)) >/dev/null 2>&1 &
  local caff=$!
  sleep 5
  local s0 s1
  s0="$(read_delivery "$visible" "$hidden")"
  sleep "$SECS"
  s1="$(read_delivery "$visible" "$hidden")"
  kill "$caff" 2>/dev/null
  python3 - "$label" "$SECS" "$visible" "$hidden" "$s0" "$s1" <<'PY'
import sys
label, secs, visible, hidden, s0, s1 = sys.argv[1], float(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5], sys.argv[6]
def parse(s):
    parts = s.split()
    rows = {}
    for p in parts[:-1]:
        pane, hops, forced, skipped, hid = (int(x) for x in p.split(":"))
        rows[pane] = (hops, forced, skipped, hid)
    return rows, int(parts[-1])
r0, f0 = parse(s0)
r1, f1 = parse(s1)
for name, pane in (("裏タブ", hidden), ("可視", visible)):
    a, b = r0[pane], r1[pane]
    hops, forced, skipped = ((b[i] - a[i]) / secs for i in range(3))
    print(f"  {label:<10} {name}（pane {pane}・hidden={bool(b[3])}）  往復 {hops:6.2f} 回/秒（うち必ず渡す {forced:6.2f}）  省いた {skipped:6.2f} 回/秒")
print(f"  {label:<10} 出力起因の再描画の要求（全体） {(f1 - f0) / secs:6.2f} 回/秒")
PY
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
  tmux_socket_kill "$sock"
  sleep 3
}

echo "出力イベントの配送（tako-vd・debug・流し方 ${KIND}・器 $([ "$PERSIST" = 1 ] && echo ON || echo OFF)・${SECS} 秒）"
for i in $(seq 1 "$ROUNDS"); do
  [ -n "$PRE_DIR" ] && run_arm "pre-$i" "$PRE_DIR"
  run_arm "legacy-$i" "" TAKO_2344_LEGACY=1
  run_arm "new-$i" ""
done
