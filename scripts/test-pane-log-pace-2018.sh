#!/bin/bash
# tako:run bash scripts/test-pane-log-pace-2018.sh
# test-pane-log-pace-2018.sh — 器のペインログの取り込みを間引いても中身が欠け・重複しないかを、
# 隔離 GUI で修正前（同じバイナリの TAKO_2018_LEGACY=1）と並べて確かめる（#2018）
#
# 何を確かめるか（どれも器 = tmux のペイン。出力させるのは常に 1 枚・1 行 10ms）:
#   ① 出力し続けている器を CLI で閉じる → ペインログが 0 番から閉じる直前の画面まで
#      欠け・重複・省略なしで続き、クローズマーカーがある（最後の増分は閉じる直前に取り込む）
#   ② 出力が止まってからシェルごと終わる（器のセッションが自ら消える）→ 全行が 1 回ずつ入る
#      （出力が止まった次の tick で取り込む）
#   ④ 器の履歴が保持上限（10000 行）で飽和した後も出し続けてから閉じる → 飽和後の行が
#      欠け・重複なく入る（飽和すると行数が増えないので、末尾の照合で新しい行を探す経路）
#   ③ 出力し続けている最中に器が自ら消える（`tako split -- <cmd>` の成功 = FR-2.5.14）→
#      0〜1499 が欠け・重複なく入り、最後の行まで届く（#2085: 器の `pane-died` フックが
#      消える前に全履歴と画面を書き出し、tako が閉じるときに読む）。#2085 の前は tmux の履歴が
#      器と一緒に消えるので最後の取り込みの後の行（#2018 で最大 10 秒）が入らなかった
#   ⑤ 成功したコマンドのペインは終わった直後に閉じる（FR-2.5.14。#2085 は remain-on-exit を
#      立てるので、閉じるのが遅れていないかを終わった時刻からの経過で測る）
#   ⑥ 失敗したコマンドのペインは閉じずに終了コードを伝える（FR-2.5.14 ②。#1031 / #1778）
#
# 新しい挙動で ①〜⑥ が通ること、旧挙動（A/B）で ① の最後の増分（TAKO_2018_LEGACY=1）・
# ③ の末尾（TAKO_2085_LEGACY=1）が抜けることを出す。
# 使い方: bash scripts/test-pane-log-pace-2018.sh [new|legacy|legacy2085|both（既定 = 3 つとも）]
#
# 隔離 GUI・tako-vd・自分の tmux ソケットだけ。落とすのは自分で起こした pid と自分のソケットだけ。
set -uo pipefail
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ARMS="${1:-both}"
TMP="$(mktemp -d /tmp/tako-2018-XXXXXX)"
SOCK_NAME="tako-2018-$$"
PASS=0
FAIL=0

cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux_socket_kill "$SOCK_NAME"
  rm -rf "$TMP"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP PIPE

export TAKO_ISO_PROFILE="${TAKO_ISO_PROFILE:-release}"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$SOCK_NAME"
mkdir -p "$HOME"

pass() { PASS=$((PASS + 1)); echo "  PASS: $*"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $*"; }

# ペインログの番号つきの行を数える: first last n gaps missing dups omitted closed
cat > "$TMP/seq.py" <<'PY'
import os, re, sys
d, pane, prefix = sys.argv[1], sys.argv[2], sys.argv[3]
files = sorted(f for f in os.listdir(d) if re.search(r"_pane%s(_|\.log$)" % re.escape(pane), f)) if os.path.isdir(d) else []
if not files:
    print("-1 -1 0 0 0 0 0 no")
    sys.exit(0)
text = "".join(open(os.path.join(d, f), encoding="utf-8", errors="replace").read() for f in files)
nums = [int(m.group(1)) for m in re.finditer(r"^%s(\d{7})$" % re.escape(prefix), text, re.M)]
pairs = list(zip(nums, nums[1:]))
gaps = sum(1 for a, b in pairs if b > a + 1)
missing = sum(b - a - 1 for a, b in pairs if b > a + 1)
dups = sum(1 for a, b in pairs if b <= a)
omitted = len(re.findall(r"^--- \[省略: ", text, re.M))
closed = "yes" if "--- [クローズ: " in text else "no"
print(nums[0] if nums else -1, nums[-1] if nums else -1, len(nums), gaps, missing, dups, omitted, closed)
PY
seq_of() { python3 "$TMP/seq.py" "$TAKO_PANE_LOG_DIR" "$1" "$2"; }
# クローズマーカーが書かれるまで待つ（最大 20 秒）
wait_closed() {
  local i
  for i in $(seq 1 100); do
    case "$(seq_of "$1" "$2")" in *" yes") return 0 ;; esac
    sleep 0.2
  done
  return 1
}
pane_alive() { "$TAKO_BIN" list | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if any(p["id"]==int(sys.argv[1]) for t in d["tabs"] for p in t["panes"]) else 1)' "$1"; }
wait_gone() {
  local i
  for i in $(seq 1 150); do
    pane_alive "$1" || return 0
    sleep 0.2
  done
  return 1
}
# 番号つきの行を 1 行 10ms で出す（N 行で終わる。0 なら止まらない）
gen() { # <接頭辞> <行数>
  printf 'import time, itertools\nfor i in (itertools.count() if %s == 0 else range(%s)):\n    print("%s%%07d" %% i, flush=True); time.sleep(0.01)\n' "$2" "$2" "$1"
}

# 履歴を一気に飽和させ（10500 行）、そのあと番号つきの行を 1 行 10ms で出し続ける。
# 行の長さを番号で変える（同じ長さだと飽和後に #{history_bytes} が動かず、取り込みの合図にならない）
sat_gen() {
  printf 'import sys, time, itertools
sys.stdout.write("".join("flood %%d %%s\\n" %% (i, "x" * (i %% 7)) for i in range(10500))); sys.stdout.flush()
time.sleep(3)
for i in itertools.count():
    print("T%%07d" %% i, flush=True); print("pad" + "y" * (i %% 5), flush=True); time.sleep(0.02)
'
}

run_arm() { # <名前> [VAR=VAL...]
  local arm="$1"
  shift
  echo "== ${arm}（$*）"
  rm -rf "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_PANE_LOG_DIR"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  tmux_socket_kill "$SOCK_NAME"
  launch_isolated_gui "$TMP/app-$arm.log" TAKO_PERSIST=1 "$@" || exit $?
  local i
  for i in $(seq 1 300); do
    [ -f "$TAKO_DATA_DIR/token" ] && break
    sleep 0.1
  done
  local sock_path="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock_path="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  export TAKO_SOCKET="$sock_path"
  TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
  export TAKO_TOKEN
  # 空なら進めない（空の TAKO_SOCKET は既定の解決 = 本番 GUI へ落ちる）
  [ -n "$TAKO_SOCKET" ] && [ -n "$TAKO_TOKEN" ] || { echo "隔離先の socket / token が空なので止める"; exit 1; }
  wait_isolated_gui "$TMP/app-$arm.log" 600 || exit 1
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
  local first p vis r
  first="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"

  # ① 出力し続けている器を閉じる
  p="$("$TAKO_BIN" split --pane "$first" --right -- python3 -u -c "$(gen P 0)" | tr -d '[:space:]')"
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  sleep 25
  vis="$("$TAKO_BIN" read --pane "$p" 2>/dev/null | grep -o '^P[0-9]\{7\}' | tail -1 | sed 's/^P0*\([0-9]\)/\1/')"
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1
  wait_closed "$p" P || true
  r="$(seq_of "$p" P)"
  echo "  ① 閉じる直前の画面の最後 = ${vis:-?}・ペインログ（first last n gaps missing dups omitted closed）= $r"
  set -- $r
  if [ "$1" = 0 ] && [ "$4" = 0 ] && [ "$6" = 0 ] && [ "$7" = 0 ] && [ "$8" = yes ] && [ -n "$vis" ] && [ "$2" -ge "$vis" ]; then
    pass "${arm} ①: 0〜${2} が欠け・重複・省略なしで続き、閉じる直前の画面（${vis}）まで入った"
  elif [ "$arm" = legacy ]; then
    # 旧挙動は閉じる直前の取り込みをしない = 最後の tick から画面の上端までが抜ける
    # （tick の位置次第で抜けない回もあるので、旧挙動の側は判定に数えない）
    echo "  旧挙動の再現: 欠け ${5} 行（${4} か所）・重複 ${6}・省略 ${7}・最後 ${2}（画面 ${vis:-?}）"
  else
    fail "${arm} ①: 欠け ${5} 行（${4} か所）・重複 ${6}・省略 ${7}・最後 ${2}（画面 ${vis:-?}）・マーカー ${8}"
  fi

  # ② 出力が止まってからシェルごと終わる（器のセッションが自ら消える）
  p="$("$TAKO_BIN" split --pane "$first" --right -- /bin/sh -c "python3 -u -c '$(gen Q 500)'; yes '' | head -n 150; sleep 5" | tr -d '[:space:]')"
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  wait_gone "$p" || echo "  （② のペインが 30 秒で閉じなかった）"
  wait_closed "$p" Q || true
  r="$(seq_of "$p" Q)"
  echo "  ② ペインログ = $r"
  set -- $r
  if [ "$1" = 0 ] && [ "$2" = 499 ] && [ "$3" = 500 ] && [ "$4" = 0 ] && [ "$6" = 0 ] && [ "$7" = 0 ]; then
    pass "${arm} ②: 0〜499 の 500 行が 1 回ずつ入った（器が消える前に取り込んだ）"
  elif [ "$arm" = legacy ]; then
    # 旧挙動は probe と capture が別の起動 = 間に伸びた行のぶん範囲がずれる（欠け・重複）
    echo "  旧挙動の再現: first ${1}・last ${2}・n ${3}・欠け ${5}・重複 ${6}・省略 ${7}"
  else
    fail "${arm} ②: first ${1}・last ${2}・n ${3}・欠け ${5}・重複 ${6}・省略 ${7}"
  fi

  # ④ 履歴を飽和させてから出し続け、閉じる（行の長さを変える = 飽和後の合図の #{history_bytes} が動く）
  p="$("$TAKO_BIN" split --pane "$first" --right -- python3 -u -c "$(sat_gen)" | tr -d '[:space:]')"
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  sleep 22
  vis="$("$TAKO_BIN" read --pane "$p" 2>/dev/null | grep -o '^T[0-9]\{7\}' | tail -1 | sed 's/^T0*\([0-9]\)/\1/')"
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1
  wait_closed "$p" T || true
  r="$(seq_of "$p" T)"
  echo "  ④ 閉じる直前の画面の最後 = ${vis:-?}・飽和後の行 = $r"
  set -- $r
  if [ "$1" -ge 0 ] && [ "$4" = 0 ] && [ "$6" = 0 ] && [ "$8" = yes ] && [ -n "$vis" ] && [ "$2" -ge "$vis" ]; then
    pass "${arm} ④: 飽和後の ${1}〜${2} が欠け・重複なしで続き、閉じる直前の画面（${vis}）まで入った"
  elif [ "$arm" = legacy ]; then
    echo "  旧挙動の再現: 欠け ${5} 行（${4} か所）・重複 ${6}・最後 ${2}（画面 ${vis:-?}）"
  else
    fail "${arm} ④: first ${1}・欠け ${5} 行（${4} か所）・重複 ${6}・最後 ${2}（画面 ${vis:-?}）・マーカー ${8}"
  fi

  # ③ 出力し続けている最中に器が自ら消える（#2085）
  p="$("$TAKO_BIN" split --pane "$first" --right -- python3 -u -c "$(gen R 1500)" | tr -d '[:space:]')"
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  wait_gone "$p" || echo "  （③ のペインが 30 秒で閉じなかった）"
  wait_closed "$p" R || true
  r="$(seq_of "$p" R)"
  echo "  ③ ペインログ = $r"
  set -- $r
  if [ "$1" = 0 ] && [ "$2" = 1499 ] && [ "$3" = 1500 ] && [ "$4" = 0 ] && [ "$6" = 0 ] && [ "$7" = 0 ] && [ "$8" = yes ]; then
    pass "${arm} ③: 器が自ら消えても 0〜1499 の 1500 行が 1 回ずつ入り、最後の行まで届いた"
  elif [ "$arm" != new ]; then
    echo "  旧挙動の再現: 最後に入ったのは ${2}（抜けた末尾 $((1499 - ${2})) 行・途中の欠け ${5}・重複 ${6}）"
  else
    fail "${arm} ③: first ${1}・last ${2}・n ${3}・欠け ${5} 行（${4} か所）・重複 ${6}・省略 ${7}・マーカー ${8}"
  fi

  # ⑤ 成功したコマンドのペインは終わった直後に閉じる（FR-2.5.14）
  local done_at gone_at ms
  p="$("$TAKO_BIN" split --pane "$first" --right -- python3 -u -c "import time; print('S'); open('$TMP/s5-$arm', 'w').write(repr(time.time()))" | tr -d '[:space:]')"
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  for i in $(seq 1 300); do
    pane_alive "$p" || break
    sleep 0.02
  done
  gone_at="$(python3 -c 'import time; print(repr(time.time()))')"
  done_at="$(cat "$TMP/s5-$arm" 2>/dev/null || echo "$gone_at")"
  ms="$(python3 -c "print(int((float('$gone_at') - float('$done_at')) * 1000))")"
  echo "  ⑤ 終わってから閉じるまで ${ms} ms"
  if pane_alive "$p"; then
    fail "${arm} ⑤: 成功したコマンドのペインが 6 秒で閉じない"
  elif [ "$ms" -lt 2000 ]; then
    pass "${arm} ⑤: 成功したコマンドのペインは終わってから ${ms} ms で閉じた"
  else
    fail "${arm} ⑤: 成功したコマンドのペインが閉じるまで ${ms} ms かかった"
  fi

  # ⑥ 失敗したコマンドのペインは閉じずに終了コードを伝える（FR-2.5.14 ②）
  p="$("$TAKO_BIN" split --pane "$first" --right -- /bin/sh -c 'echo F; exit 3' | tr -d '[:space:]')"
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  sleep 3
  r="$("$TAKO_BIN" run-interactive-status "$p" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("exit_code"))' 2>/dev/null)"
  if pane_alive "$p" && [ "$r" = 3 ]; then
    pass "${arm} ⑥: 失敗したコマンドのペインは閉じずに終了コード 3 を伝えた"
  else
    fail "${arm} ⑥: 失敗したコマンドのペイン（生存 $(pane_alive "$p" && echo yes || echo no)・終了コード ${r:-?}）"
  fi
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1

  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
  tmux_socket_kill "$SOCK_NAME"
  unset TAKO_SOCKET TAKO_TOKEN
}

case "$ARMS" in
  new) run_arm new ;;
  legacy) run_arm legacy TAKO_2018_LEGACY=1 ;;
  legacy2085) run_arm legacy2085 TAKO_2085_LEGACY=1 ;;
  both)
    run_arm new
    run_arm legacy TAKO_2018_LEGACY=1
    run_arm legacy2085 TAKO_2085_LEGACY=1
    ;;
  *) echo "不明: ${ARMS}（new / legacy / legacy2085 / both）" >&2; exit 2 ;;
esac
echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" = 0 ]
