#!/usr/bin/env bash
# test-pty-fd-leak-1768.sh — ペインの PTY の子が GUI の fd を受け継がないことの実経路テスト（#1768）
#
# 隔離した data / HOME / tmux で**実 tako-app** を立て、
#
#   ① GUI の中で `git` を `.output()` する要求（`tako git log` = offload スレッドで spawn）を
#      外から並行で浴びせ続ける
#   ② そのあいだにペインを連続で開いては閉じる（PTY の fork は UI スレッド）
#   ③ 開いたペインの PTY の子（シェル）ごとに `lsof` で fd 3 以上を採り、
#      自分の tty 以外を「余計な fd」として数える
#
# を回す。数えるのは 2 種類:
#
#   - **パイプ**: 相方が子の内側に居ないもの（= 他スレッドの `Stdio::piped()` を fork の
#     瞬間に掴んだ。#1768 の本題。相方を GUI が握っていれば GUI の読み出しが EOF を見ない）
#   - **その他**: GUI と同じファイルを指す REG など（Metal のシェーダキャッシュは GUI が
#     CLOEXEC 無しで開くので、競合と無関係に毎回漏れる）
#
# 余計な fd を持つ子が 1 本でもあれば終了コード 1。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-pty-fd-leak-1768.sh
#   ROUNDS=60 PANES=6 LOADERS=8 で回数と負荷を変えられる（試行 = ROUNDS × PANES）
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROUNDS=${ROUNDS:-60}
PANES=${PANES:-6}
LOADERS=${LOADERS:-8}

TMP="$(mktemp -d /tmp/tako-1768-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1768-$$"
LOADER_PIDS=()
cleanup() {
  touch "$TMP/stop" 2>/dev/null
  local p
  for p in ${LOADER_PIDS[@]+"${LOADER_PIDS[@]}"}; do
    kill "$p" 2>/dev/null
    wait "$p" 2>/dev/null
  done
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
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# `tako git log` の対象にする使い捨てのリポジトリ
REPO="$TMP/repo"
mkdir -p "$REPO" "$TMP/work"
git -C "$REPO" init -q
git -C "$REPO" -c user.name=testuser -c user.email=testuser@example.invalid \
  commit -q --allow-empty -m "fixture"

echo "== 隔離 GUI を起こす =="
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
[ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
# git log の対象ペイン（cwd = 使い捨てリポジトリ）
ANCHOR="$("$TAKO_BIN" split --pane "$ROOT" --cwd "$REPO" 2>/dev/null)"
[ -n "$ANCHOR" ] || { echo "リポジトリのペインを開けない"; exit 1; }

# GUI の直下の子のうち制御端末を持つもの（= ペインの PTY の子）の pid を 1 行 1 本で出す。
# **端末で絞る**: 負荷役が GUI に起こさせる `git` も GUI の直下の子だが、GUI には制御端末が
# 無いので `??` になる（絞らないと試行に混ざる）
pty_children() {
  ps -axo pid=,ppid=,tty= | awk -v p="$APP_PID" '$2==p && $3!="??"{print $1}' | sort -n
}
# 子の本数が期待値になるまで待つ（上限 20 秒）。なれなければ非ゼロ
wait_children() {
  local want="$1" i
  for i in $(seq 1 200); do
    [ "$(pty_children | wc -l | tr -d ' ')" -eq "$want" ] && return 0
    sleep 0.1
  done
  return 1
}

# 負荷役: GUI に `git` を `.output()` させる要求（GitLog = offload で `git` を 4 回）を
# 止まるまで撃ち続ける。CLI を毎回起こすと起動が律速で要求が細るので、発見ファイルの
# socket / token で**常時接続**し、本数ぶんのスレッドから並行で流す。
# 呼び出しの遅延は**観察として**残す（漏れたパイプの相方を GUI が握ると、
# その要求は子が死ぬまで返らない = #1768 の症状 2。合否には使わない）
LOADER_PY='import json, os, socket, sys, threading, time
disc, pane, stop, out, n_threads = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4], int(sys.argv[5])
info = json.load(open(os.path.join(disc, "control.json")))
stats = {"calls": 0, "errors": 0, "worst_s": 0.0, "over_3s": 0}
lock = threading.Lock()
def run(k):
    s = socket.socket(socket.AF_UNIX); s.connect(info["socket"]); f = s.makefile("rwb")
    i = 0
    while not os.path.exists(stop):
        i += 1
        req = {"jsonrpc": "2.0", "id": i, "token": info["token"],
               "method": "git_log", "params": {"pane": pane, "max_count": 1}}
        t = time.monotonic()
        f.write((json.dumps(req) + "\n").encode()); f.flush()
        line = f.readline()
        d = time.monotonic() - t
        with lock:
            stats["calls"] += 1
            stats["errors"] += (not line) or (b"\"error\"" in line)
            stats["worst_s"] = max(stats["worst_s"], round(d, 2))
            stats["over_3s"] += d > 3.0
        if not line:
            return
ts = [threading.Thread(target=run, args=(k,), daemon=True) for k in range(n_threads)]
for t in ts: t.start()
while not os.path.exists(stop):
    time.sleep(0.5)
    with lock: json.dump(stats, open(out, "w"))
for t in ts: t.join(timeout=15)
with lock: json.dump(stats, open(out, "w"))'
python3 -c "$LOADER_PY" "$TAKO_DISCOVERY_DIR" "$ANCHOR" "$TMP/stop" "$TMP/loader.json" "$LOADERS" &
LOADER_PIDS+=("$!")

BASE="$(pty_children | tr '\n' ' ')"
BASE_N="$(pty_children | wc -l | tr -d ' ')"
echo "== ${ROUNDS} 周 × ${PANES} ペイン（負荷役 ${LOADERS} 本・GUI の子の基準 ${BASE_N} 本）=="

# 1 周ぶんの子の fd を判定して jsonl へ 1 行 1 子で足す
JUDGE_PY='import subprocess, sys, json
gui, out = sys.argv[1], sys.argv[2]
pids = sys.argv[3:]
def table(pid_arg):
    r = subprocess.run(["lsof", "-n", "-P", "-a", "-p", pid_arg, "-d", "0-65535", "-F", "pftdn"],
                       capture_output=True, text=True).stdout
    procs = {}; cur = None; fd = None
    for line in r.splitlines():
        k, v = line[:1], line[1:]
        if k == "p": cur = procs.setdefault(v, []); fd = None
        elif k == "f": fd = {"fd": v}; cur.append(fd)
        elif fd is not None and k in "tdn": fd[k] = v
    return procs
kids = table(",".join(pids))
gui_fds = table(gui).get(gui, [])
gui_reg = {f.get("n") for f in gui_fds if f.get("t") == "REG"}
gui_pipe = {f.get("d") for f in gui_fds if f.get("t") == "PIPE"}
with open(out, "a") as o:
    for pid in pids:
        fds = kids.get(pid, [])
        ttys = {f.get("n") for f in fds if f["fd"] in ("0", "1", "2")}
        own_pipe = {f.get("d") for f in fds if f.get("t") == "PIPE"}
        extra = []
        for f in fds:
            if not f["fd"].isdigit() or int(f["fd"]) < 3:
                continue
            if f.get("t") == "CHR" and f.get("n") in ttys:
                continue  # シェルが自分で複製した制御端末（zsh の SHTTY）
            e = {"fd": int(f["fd"]), "type": f.get("t"), "name": f.get("n", "")}
            if f.get("t") == "PIPE":
                peer = (f.get("n") or "").replace("->", "")
                e["peer_in_child"] = peer in own_pipe
                e["peer_in_gui"] = peer in gui_pipe
                if e["peer_in_child"]:
                    continue  # 子が exec 後に自分で作ったパイプ
            elif f.get("t") == "REG":
                e["same_as_gui"] = f.get("n") in gui_reg
            extra.append(e)
        o.write(json.dumps({"pid": pid, "extra": extra}) + "\n")'

RESULTS="$TMP/results.jsonl"
: > "$RESULTS"
DIR_FLAGS=(--right --down)
for r in $(seq 1 "$ROUNDS"); do
  IDS=()
  for i in $(seq 1 "$PANES"); do
    id="$("$TAKO_BIN" split --pane "$ROOT" "${DIR_FLAGS[$((i % 2))]}" --cwd "$TMP/work" 2>/dev/null)"
    case "$id" in ''|*[!0-9]*) echo "  周 $r: split が ID を返さない: $id"; continue ;; esac
    IDS+=("$id")
  done
  wait_children $((BASE_N + ${#IDS[@]})) || echo "  周 $r: 子の本数が揃わない"
  NEW=()
  for p in $(pty_children); do
    case " $BASE " in *" $p "*) : ;; *) NEW+=("$p") ;; esac
  done
  # シェルが exec を終えるまで待つ（fork 直後の子は GUI の fd 表の複製のまま = 判定が早すぎる）
  for i in $(seq 1 100); do
    busy=0
    for p in ${NEW[@]+"${NEW[@]}"}; do
      case "$(ps -o comm= -p "$p" 2>/dev/null)" in *tako-app*) busy=1 ;; esac
    done
    [ "$busy" -eq 0 ] && break
    sleep 0.05
  done
  [ "${#NEW[@]}" -gt 0 ] && python3 -c "$JUDGE_PY" "$APP_PID" "$RESULTS" ${NEW[@]+"${NEW[@]}"}
  for id in ${IDS[@]+"${IDS[@]}"}; do
    "$TAKO_BIN" close --pane "$id" --force >/dev/null 2>&1
  done
  wait_children "$BASE_N" || echo "  周 $r: 閉じた子が残っている"
done
touch "$TMP/stop"
for p in ${LOADER_PIDS[@]+"${LOADER_PIDS[@]}"}; do wait "$p" 2>/dev/null; done
LOADER_PIDS=()

echo "== 集計 =="
python3 - "$RESULTS" "$TMP" <<'PY'
import json, sys, collections
rows = [json.loads(l) for l in open(sys.argv[1])]
pipe = [r for r in rows if any(e["type"] == "PIPE" for e in r["extra"])]
other = [r for r in rows if any(e["type"] != "PIPE" for e in r["extra"])]
kinds = collections.Counter()
for r in rows:
    for e in r["extra"]:
        kinds[(e["type"], e["name"].rsplit("/", 1)[-1] if e["type"] == "REG" else "")] += 1
print(f"  試行（判定した PTY の子）: {len(rows)}")
print(f"  余計なパイプを持った子: {len(pipe)} / {len(rows)}")
for r in pipe[:10]:
    print("    pid=%s %s" % (r["pid"], [e for e in r["extra"] if e["type"] == "PIPE"]))
print(f"  パイプ以外の余計な fd を持った子: {len(other)} / {len(rows)}")
for (t, n), c in kinds.most_common():
    print(f"    {t} {n}: {c}")
try:
    d = json.load(open(sys.argv[2] + "/loader.json"))
    print(f"  負荷役の git log 要求: {d['calls']} 回（エラー {d['errors']}・最長 {d['worst_s']} 秒・"
          f"3 秒超 {d['over_3s']} 回。観察のみ）")
except Exception as e:
    print(f"  負荷役の集計を読めない: {e}")
leaked = sum(1 for r in rows if r["extra"])
print(f"RESULT total={len(rows)} pipe_leak={len(pipe)} other_leak={len(other)} any_leak={leaked}")
sys.exit(1 if leaked else 0)
PY
