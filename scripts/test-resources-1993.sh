#!/bin/bash
# tako:run bash scripts/test-resources-1993.sh
#
# PC リソースの計測と読み取りの口（#1993。エピック #1992 の S1）の実経路テスト。
#
# 隔離 GUI（仮想ディスプレイ tako-vd・器あり = TAKO_PERSIST=1）を立て、ペインの中で
# 「cargo という名前の眠るだけのプロセス」を起こして、`tako queue --json` が
#   ① 機の値を Activity Monitor と同じ式で返す（vm_stat / sysctl と突き合わせ）
#   ② その処理を「重い処理」として、起こしたペインへ帰属させる（器の pane_pid の子孫）
#   ③ launchd へ付け替わった子（nohup）を env の TAKO_PANE_ID でペインへ帰属させる
#   ④ GUI が居ないときは機の値だけを返し「順番待ちは無効（tako が起動していない）」を添える
# を確かめる。**負荷は生成しない**（重い処理は名前だけ cargo の sleep）。
#
# 落とすのは自分で起こした pid と自分の tmux ソケットだけ（名前一致で殺さない）。

set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 短い一時 dir（深いと IPC の受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk1993-XXXXXX)"
SOCKET="tk1993-$$"
FAKE_PIDS=""
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux_socket_kill "$SOCKET"
  for p in $FAKE_PIDS; do kill "$p" 2>/dev/null; done
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
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
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/bin"
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
  esac
done
# 「cargo」という名前で眠るだけのプロセス（argv の第 1 語が cargo になる）。
# **/bin/sleep の別名では作らない**: Apple のプラットフォームバイナリは env を他のプロセスから
# 読めない（KERN_PROCARGS2 に env が載らない = 実測で 0 キー）ので、③ の env の帰属が試せない。
# 本物の cargo / rustc / node はプラットフォームバイナリではないので読める（実測 87 キー）
printf '#include <unistd.h>\n#include <stdlib.h>\nint main(int c, char **v) { sleep(c > 1 ? atoi(v[1]) : 30); return 0; }\n' > "$TMP/sleeper.c"
cc -O2 -o "$TMP/bin/cargo" "$TMP/sleeper.c" || { echo "眠るだけの cargo をビルドできない（cc）"; exit 1; }

queue_json() { "$TAKO_BIN" queue --json 2>/dev/null; }

echo "== ④ GUI が居ないとき =="
OFF="$TMP/offline.json"
queue_json > "$OFF"
python3 - "$OFF" <<'PY' && pass "GUI が居なくても機の値と「順番待ちは無効」を返す" || fail "GUI 不在の応答（${OFF}）"
import json, sys
v = json.load(open(sys.argv[1]))
assert v["gui"] is False, v
assert v["note"] == "順番待ちは無効（tako が起動していない）", v
assert v["machine"]["memory_mb"]["total"] > 0, v
assert v["machine"]["pressure"] in ("normal", "warn", "critical"), v
PY

echo "== 隔離 GUI を起動 =="
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1200,800}
launch_isolated_gui "$TMP/app.log" TAKO_PERSIST=1 TAKO_AUTO_RENAME=0 || exit $?
wait_isolated_gui || { fail "隔離 GUI に繋がらない"; echo "PASS=$PASS FAIL=$((FAIL))"; exit 1; }

PANE="$("$TAKO_BIN" list | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["tabs"][0]["panes"][0]["id"])')"
echo "  ペイン: $PANE"
# シェルのプロンプトが出るまで待つ
for _ in $(seq 1 100); do
  screen="$("$TAKO_BIN" read --pane "$PANE" 2>/dev/null)" && grep -q '%' <<<"$screen" && break
  sleep 0.2
done
"$TAKO_BIN" send --pane "$PANE" "PATH=$TMP/bin:\$PATH cargo 600 &" >/dev/null
"$TAKO_BIN" send --pane "$PANE" "(PATH=$TMP/bin:\$PATH nohup cargo 601 >/dev/null 2>&1 &)" >/dev/null

# 眠るプロセスの pid（後始末用。自分の一時 dir の cargo だけ）
for _ in $(seq 1 50); do
  FAKE_PIDS="$(ps -axo pid=,command= | awk -v t="$TMP/bin/cargo" '$2 == t || $2 == "cargo" && $3 ~ /^60[01]$/ {print $1}' | tr '\n' ' ')"
  [ "$(echo $FAKE_PIDS | wc -w | tr -d ' ')" -ge 2 ] && break
  sleep 0.2
done
echo "  眠る cargo: $FAKE_PIDS"

echo "== ①〜③ GUI 経由 =="
ON="$TMP/online.json"
# 1 回目は覚えが無いのでその場で採る。2 回目（2 秒後）で CPU% の差分が入る
queue_json > /dev/null
sleep 2.2
queue_json > "$ON"
VMSTAT="$TMP/vm_stat.txt"
vm_stat > "$VMSTAT"
python3 - "$ON" "$VMSTAT" "$(sysctl -n hw.memsize)" "$(sysctl -n kern.memorystatus_vm_pressure_level)" "$PANE" <<'PY'
import json, re, sys
v = json.load(open(sys.argv[1]))
vm = open(sys.argv[2]).read()
total = int(sys.argv[3]); level = int(sys.argv[4]); pane = int(sys.argv[5])
page = int(re.search(r"page size of (\d+) bytes", vm).group(1))
def pages(label):
    return int(re.search(re.escape(label) + r":\s+(\d+)", vm).group(1))
app = (pages("Anonymous pages") - pages("Pages purgeable")) * page
wired = pages("Pages wired down") * page
comp = pages("Pages occupied by compressor") * page
used_mb = (app + wired + comp) / 1048576
m = v["machine"]["memory_mb"]
ok = []
def check(name, cond, detail):
    print(("  [OK] " if cond else "  [NG] ") + name + "（" + detail + "）")
    ok.append(cond)
check("① メモリの全体が hw.memsize と一致", m["total"] == round(total / 1048576), f'{m["total"]} MB')
# 2 つの採取の間にもメモリは動くので 3% の幅で見る
check("① 使用（app + wired + 圧縮）が vm_stat の式と一致", abs(m["used"] - used_mb) <= used_mb * 0.03,
      f'tako {m["used"]} MB / vm_stat {used_mb:.0f} MB')
check("① 空き = 全体 − 使用", m["available"] == m["total"] - m["used"] or abs(m["available"] - (m["total"] - m["used"])) <= 1,
      f'{m["available"]} MB')
names = {1: "normal", 2: "warn", 4: "critical"}
check("① メモリ圧が sysctl と一致", v["machine"]["pressure"] == names[level], f'{v["machine"]["pressure"]} / level {level}')
check("① CPU% が出ている（2 回目の差分）", v["machine"]["cpu_percent"] is not None, str(v["machine"]["cpu_percent"]))
heavy = v["heavy"]
mine = [h for h in heavy if h["label"].startswith("cargo 60")]
labels = sorted(h["label"] for h in mine)
check("② 眠る cargo が 2 件とも重い処理（build）に載る", labels == ["cargo 600", "cargo 601"] and all(h["kind"] == "build" for h in mine),
      json.dumps(mine, ensure_ascii=False))
by = {h["label"]: h for h in mine}
check("② 器のペインの子孫（cargo 600）が起こしたペインへ帰属する", by.get("cargo 600", {}).get("pane") == pane,
      json.dumps(by.get("cargo 600"), ensure_ascii=False))
check("③ launchd へ付け替わった子（cargo 601）が env の TAKO_PANE_ID でペインへ帰属する",
      by.get("cargo 601", {}).get("pane") == pane, json.dumps(by.get("cargo 601"), ensure_ascii=False))
row = [p for p in v["panes"] if p["pane"] == pane]
check("② ペインの行に重い処理 2 件が数えられる", bool(row) and row[0]["heavy"] == 2, json.dumps(row, ensure_ascii=False))
check("走査の節（本数・読めた本数・経過）", v["scan"]["processes"] > 100 and v["scan"]["readable"] > 0,
      json.dumps(v["scan"]))
sys.exit(0 if all(ok) else 1)
PY
if [ $? -eq 0 ]; then pass "①〜③ の突き合わせ"; else fail "①〜③ の突き合わせ（${ON}）"; cp "$ON" /tmp/tk1993-online-last.json 2>/dev/null; fi

echo "== 人が読む表示 =="
"$TAKO_BIN" queue | sed 's/^/    /'

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
