#!/usr/bin/env bash
# tako:run bash scripts/measure-remote-poll-2539.sh 60
# measure-remote-poll-2539.sh — remote daemon の稼働中に GUI が起こす tailscale の数と CPU を測る（#2539）
#
# 隔離した state / data ディレクトリ・偽の tailscale CLI（TAKO_TAILSCALE_BIN）で**本物の daemon**
# （`tako remote start`）と隔離 GUI（仮想ディスプレイ tako-vd）を立て、GUI の 2 秒ごとの状態の更新が
# 起こす tailscale の本数 / 分と、GUI の CPU（本体と回収済みの子 = proc_pid_rusage v6）を測る。
# 偽の tailscale は起こした親（$PPID）と引数を 1 行ずつ記録するので、GUI 由来と daemon 由来を pid で分ける。
#
# 続けて `tako remote status` の出力（揺れる欄を落とした sha）と、劣化の検知の遅れを測る:
#   ① tailscale が落ちた（偽物の status / serve status が失敗する）→ status の serve_ok が false になるまで
#   ② 2 系統の同居（--socket の応答だけ別ノード）→ status の warnings に出るまで
# 劣化を見つけるのは daemon の serve の自己検査（#1049。既定 30 秒周期）と、status を引いた時点の
# 系統の調査で、GUI の周期は関わらない。修正前後で同じ秒数になることを示すための計測。
#
# **本番の tailscale / serve 設定 / remote daemon / GUI には一切触らない**（tailscale の呼び出しは全部
# 偽物へ向く。state・data は mktemp 配下。落とすのは自分で起こした daemon と GUI の pid だけ）。
#
# 使い方: bash scripts/measure-remote-poll-2539.sh [測る秒数=60]
#   TAKO_2539_LEGACY=1  GUI と CLI を修正前の引き方（1 tick に daemon_status を 2 回）へ戻す（A/B）
#   TAKO_2539_OUT=<dir> 集計と status の JSON を残す先（A/B は別の dir に分ける）
#   TAKO_2539_WARM=<秒> 測り始める前の待ち（既定 10。起動直後の 1 回きりの呼び出しを窓から外す）
set -uo pipefail
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
SECS="${1:-60}"
WARM="${TAKO_2539_WARM:-10}"
LEGACY="${TAKO_2539_LEGACY:-}"
OUT="${TAKO_2539_OUT:-}"
# ソケットのパス長の上限があるので短い置き場にする
TMP="$(mktemp -d /tmp/tk2539-XXXXXX)"
DAEMON_PID=""
SOCKET="tk2539-$$"

cleanup() {
  if [ -n "${TAKO_BIN:-}" ] && [ -s "$TMP/state/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  if [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill "$DAEMON_PID" 2>/dev/null || true
  fi
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux_socket_kill "$SOCKET"
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前に建てる（ツールチェーンを一時 HOME へ取り直さない）
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$SOCKET"
export TAKO_REMOTE_STATE_DIR="$TMP/state"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_REMOTE_STATE_DIR"
case "$TAKO_REMOTE_STATE_DIR" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $TAKO_REMOTE_STATE_DIR"; exit 1 ;;
esac
if [ -n "$LEGACY" ]; then export TAKO_2539_LEGACY="$LEGACY"; fi
if [ -n "$OUT" ]; then mkdir -p "$OUT"; fi

# --- 偽 tailscale CLI（呼ばれるたびに「親の pid と引数」を 1 行。外部コマンドは起こさない）-------
FAKE_TS="$TMP/fake-tailscale"
CALLS="$TMP/ts-calls.log"
: > "$CALLS"
echo '{}' > "$TMP/serve.json"
cat > "$FAKE_TS" <<'FAKE'
#!/bin/bash
STATE="${0%/*}"
echo "$PPID $*" >> "$STATE/ts-calls.log"
SOCK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --socket) SOCK="$2"; shift 2 ;;
    *) break ;;
  esac
done
NODE="fake-2539.tailfake.ts.net"
# 2 系統の同居の注入: --socket で名指した相手だけ別ノードとして答える
if [ -n "$SOCK" ] && [ -f "$STATE/ts-coexist" ]; then NODE="other-2539.tailfake.ts.net"; fi
SERVE="$STATE/serve.json"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    # tailscale が落ちた注入: LocalAPI に届かない
    if [ -f "$STATE/ts-down" ]; then echo "failed to connect to local tailscaled" >&2; exit 1; fi
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  serve)
    if [ -f "$STATE/ts-down" ]; then echo "failed to connect to local tailscaled" >&2; exit 1; fi
    shift
    case "${1:-}" in
      status) cat "$SERVE"; exit 0 ;;
      --bg)
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "${3:-}" > "$SERVE"
        exit 0 ;;
      --https=443)
        if [ "${2:-}" = "off" ]; then echo '{}' > "$SERVE"; exit 0; fi
        exit 1 ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$FAKE_TS"
export TAKO_TAILSCALE_BIN="$FAKE_TS"

# --- proc_pid_rusage v6（sudo 不要）: 本体 user / sys・回収済みの子 user / sys（ns）・エネルギー（nJ）---
cat > "$TMP/ru.py" <<'PY'
import ctypes, sys
class RU(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16), ("f", ctypes.c_uint64 * 47), ("r", ctypes.c_uint64 * 9)]
class TB(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
tb = TB()
lib.mach_timebase_info(ctypes.byref(tb))
k = tb.numer / tb.denom
ru = RU()
if lib.proc_pid_rusage(int(sys.argv[1]), 6, ctypes.byref(ru)) != 0:
    sys.exit(1)
print(int(ru.f[0] * k), int(ru.f[1] * k), int(ru.f[10] * k), int(ru.f[11] * k), ru.f[40])
PY

# status の欄を読む（ドット区切りのキー）
field_of() {
  python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur, dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else (json.dumps(cur, ensure_ascii=False) if isinstance(cur,(list,dict)) else cur)))' "$1" "$2"
}
# 揺れる欄（pid・時刻・経過秒・実行ファイルの置き場と識別・待ち受けのポート）を落として並べ直した JSON の sha
cat > "$TMP/norm.py" <<'PY'
import json, sys, hashlib, re
VOLATILE = {"pid", "serve_checked_at", "serve_checked_age_secs", "serve_generation", "serve_binary",
            "at", "started_at", "desired_since"}
def strip(v):
    if isinstance(v, dict): return {k: strip(x) for k, x in v.items() if k not in VOLATILE}
    if isinstance(v, list): return [strip(x) for x in v]
    if isinstance(v, str): return re.sub(r"127\.0\.0\.1:\d+", "127.0.0.1:<port>", v)
    return v
d = strip(json.load(open(sys.argv[1])))
s = json.dumps(d, ensure_ascii=False, sort_keys=True)
open(sys.argv[2], "w").write(s + "\n")
print(hashlib.sha256(s.encode()).hexdigest()[:16])
PY
status_sha() { python3 "$TMP/norm.py" "$1" "$2"; }

echo "== daemon を起こす（偽 tailscale・隔離 state）=="
if ! "$TAKO_BIN" remote start >"$TMP/start.json" 2>"$TMP/start.err"; then
  echo "daemon を起動できなかった:"; cat "$TMP/start.err"; exit 1
fi
for _ in $(seq 1 300); do
  [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.url" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.serve" ] && break
  sleep 0.1
done
DAEMON_PID="$(head -1 "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" 2>/dev/null | tr -dc '0-9')"
[ -n "$DAEMON_PID" ] || { echo "daemon の pid を読めない"; cat "$TMP/start.err"; exit 1; }
echo "    daemon pid=$DAEMON_PID"

echo "== 隔離 GUI を起こす（legacy=${LEGACY:-0}）=="
launch_isolated_gui "$TMP/app.log" ${LEGACY:+TAKO_2539_LEGACY=$LEGACY} || exit $?
wait_isolated_gui "$TMP/app.log" 600 || exit 1
GUI="$ISOLATED_GUI_PID"
echo "    GUI pid=${GUI}（$("$REPO_ROOT/scripts/lib/virtual-display.sh" status 2>/dev/null | sed -n 1p)）"
sleep "$WARM"

snap() { echo "$(wc -l < "$CALLS" | tr -d ' ') $(python3 "$TMP/ru.py" "$GUI")"; }
A="$(snap)"
T0="$(python3 -c 'import time; print(time.time())')"
sleep "$SECS"
B="$(snap)"
T1="$(python3 -c 'import time; print(time.time())')"

# 窓の中の呼び出しを親で分ける
set -- $A; N0=$1
set -- $B; N1=$1
sed -n "$((N0 + 1)),${N1}p" "$CALLS" > "$TMP/window.log"
python3 - "$A" "$B" "$T0" "$T1" "$GUI" "$DAEMON_PID" "$TMP/window.log" "$LEGACY" > "$TMP/summary.txt" <<'PY'
import sys, collections
a = [int(x) for x in sys.argv[1].split()]
b = [int(x) for x in sys.argv[2].split()]
t = float(sys.argv[4]) - float(sys.argv[3])
gui, daemon = sys.argv[5], sys.argv[6]
by = collections.Counter()
args = collections.Counter()
for line in open(sys.argv[7]):
    ppid, _, rest = line.strip().partition(" ")
    who = "gui" if ppid == gui else ("daemon" if ppid == daemon else "other")
    by[who] += 1
    if who == "gui":
        args[rest] += 1
per_min = lambda n: n * 60.0 / t
d = [b[i] - a[i] for i in range(len(a))]
print(f"窓 {t:.1f} 秒（legacy={sys.argv[8] or 0}）")
print(f"tailscale の起動: GUI 由来 {by['gui']} 本 = {per_min(by['gui']):.1f} 本/分・daemon 由来 {by['daemon']} 本・その他 {by['other']} 本")
for k, v in sorted(args.items(), key=lambda kv: -kv[1]):
    print(f"    GUI: {v:4d} 本  tailscale {k}")
self_ms = (d[1] + d[2]) / 1e6
child_ms = (d[3] + d[4]) / 1e6
print(f"GUI の CPU: 本体 {self_ms:.0f} ms = {self_ms * 60 / t:.0f} ms/分・回収済みの子 {child_ms:.0f} ms = {child_ms * 60 / t:.0f} ms/分・合計 {(self_ms + child_ms) * 60 / t:.0f} ms/分")
print(f"GUI のエネルギー（本体）: {d[5] / 1e6:.0f} mJ = {d[5] / 1e6 * 60 / t:.0f} mJ/分")
PY
cat "$TMP/summary.txt"

echo "== 表示の材料（揺れる欄を落とした sha）=="
"$TAKO_BIN" remote status > "$TMP/status.json" 2>"$TMP/status.err"
echo "    tako remote status sha=$(status_sha "$TMP/status.json" "$TMP/status.norm.json")"
# GUI の状態の更新が受け取る admin state と同じもの（daemon の稼働中は admin API の /api/admin/state）
"$TAKO_BIN" remote devices list > "$TMP/devices.json" 2>"$TMP/devices.err"
echo "    admin state（tako remote devices list）sha=$(status_sha "$TMP/devices.json" "$TMP/devices.norm.json")"

# 劣化の注入から status が言うまでの秒数（$1 = 注入のファイル・$2 = 判定の python 式）
detect_delay() {
  local flag="$1" expr="$2" start now i
  touch "$TMP/$flag"
  start="$(python3 -c 'import time; print(time.time())')"
  for i in $(seq 1 900); do
    "$TAKO_BIN" remote status > "$TMP/st-$flag.json" 2>/dev/null
    if python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if ($expr) else 1)" "$TMP/st-$flag.json" 2>/dev/null; then
      now="$(python3 -c 'import time; print(time.time())')"
      python3 -c "print(f'{$now - $start:.1f}')"
      rm -f "${TMP:?}/${flag:?}"
      return 0
    fi
    sleep 0.2
  done
  rm -f "${TMP:?}/${flag:?}"
  echo "検知せず（180 秒）"
}
echo "== 劣化の検知の遅れ =="
echo "    ① tailscale が落ちた → serve_ok=false まで: $(detect_delay ts-down 'd.get("serve_ok") is False') 秒（上限は daemon の自己検査の周期 = 既定 30 秒）"
# 落ちた状態から戻るのを待つ（次の自己検査で serve_ok=true へ戻る）
for _ in $(seq 1 400); do
  "$TAKO_BIN" remote status > "$TMP/st-back.json" 2>/dev/null
  [ "$(field_of "$TMP/st-back.json" serve_ok)" = "true" ] && break
  sleep 0.2
done
echo "    ② 2 系統の同居 → warnings に出るまで: $(detect_delay ts-coexist 'any("2 系統" in w for w in d.get("warnings", []))') 秒"

if [ -n "$OUT" ]; then
  command cp -f "$TMP/summary.txt" "$TMP/status.json" "$TMP/status.norm.json" "$TMP/devices.json" \
    "$TMP/devices.norm.json" "$TMP/window.log" "$OUT/"
  command cp -f "$TMP/st-ts-coexist.json" "$OUT/status-coexist.json" 2>/dev/null || true
  command cp -f "$TMP/st-ts-down.json" "$OUT/status-down.json" 2>/dev/null || true
  echo "    書き出し: $OUT"
fi
