#!/usr/bin/env bash
# test-tmux-open-quit-2634.sh — `tako tmux open` のペインの close と GUI の終了で tmux サーバーを止めない（#2634）
#
# #2625 の続き。隔離した data / tmux で**実 tako-app** を立て、外から 10ms おきに打つ `tmux ls`
# （隔離ソケット）の所要を測る:
#   ① `tako tmux open` で利用者役の tmux サーバー（隔離ソケット）へつないだペインを閉じた直後、
#      利用者役のサーバーの `tmux ls` が 50ms を超えない。器なし（TAKO_PERSIST=0 = PTY 直下の子が
#      利用者のサーバーのクライアント）と器あり（TAKO_PERSIST=1 = 器の中のクライアント）の両方。
#      利用者のセッションは残り、表示用ラッパーは消え、PTY の親側の fd・GUI の子プロセス・ゾンビが戻る
#   ② 器のペインを持つ隔離 GUI を普通に終了（SIGTERM = #777 の正規の quit）した直後、器のサーバーの
#      `tmux ls` が 50ms を超えない。終了にかかる時間を測り、器は残って attach クライアントは 0 本、
#      起動し直しでペインが戻る。器なしの tmux open のペインを開いたままの終了も利用者役のサーバーを見る
#      （終了は OS が PTY の親側を先に閉じるので修正前から止まらない = 終了の経路は変えていない。退行の見張り）
#   ③ A/B: `TAKO_2634_LEGACY=1` で同じバイナリのまま ① の器なしの止まりが再現する
# を実測する。
#
# **本番の tako / 設定・利用者の既定の tmux サーバーには一切触らない**（data / HOME は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid と自分のソケットのサーバーだけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る = 他の
# `isolated-gui.sh` 系スクリプトと同じ扱い）。CI 側の担保は番犬
# （`crates/tako-control/tests/issue2634_tmux_open_hangup_watchdog.rs`）と単体テスト。
#
# 使い方: bash scripts/test-tmux-open-quit-2634.sh
#        TAKO_2634_ROUNDS=3 bash scripts/test-tmux-open-quit-2634.sh   # 閉じる / 終了する回数を変える
#        TAKO_2634_ONLY=fixed bash scripts/test-tmux-open-quit-2634.sh # A/B の旧挙動の腕を回さない
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
ROUNDS="${TAKO_2634_ROUNDS:-5}"
ONLY="${TAKO_2634_ONLY:-}"
# 修正後に許す閉じた / 終了した直後の `tmux ls` の最大（ms）と、旧挙動を「再現した」と読む下限（ms）
LIMIT_MS=50
LEGACY_MIN_MS=300

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2634-XXXXXX)"
APP_PID=""
SOCKETS=""
cleanup() {
  local s
  stop_isolated_gui "$APP_PID"
  for s in $SOCKETS; do
    tmux -L "$s" kill-server >/dev/null 2>&1 || true
    # サーバーが落ちてもソケットファイルが残ることがある（自分の名前の 1 本だけ消す）
    rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${s:?}"
  done
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1

# 置き場を腕ごとに分ける（同じ dir・ソケットを使うと 2 回目が前の器を拾う）
use_dirs() {
  local root="$TMP/$1"
  export TAKO_DATA_DIR="$root/data"
  export TAKO_DISCOVERY_DIR="$root/disc"
  export TAKO_ORCHESTRATOR_DIR="$root/orch"
  export TAKO_SESSIONS_FILE="$root/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$root/panelogs"
  export TAKO_WORKERS_FILE="$root/workers.yaml"
  export TAKO_TMUX_SOCKET="tako-2634-$$-$1"
  USER_SOCK="tako-2634-$$-$1-user"
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET $USER_SOCK"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  case "$TAKO_DATA_DIR" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;;
  esac
}

tm() { tmux -L "$TAKO_TMUX_SOCKET" "$@"; }
um() { tmux -L "$USER_SOCK" "$@"; }
vessel_count() { tm ls -F '#{session_name}' 2>/dev/null | grep -c . | tr -d ' '; }
vessel_clients() { tm list-clients -F '#{client_pid}' 2>/dev/null | grep -c . | tr -d ' '; }
user_sessions() { um ls -F '#{session_name}' 2>/dev/null | sort | tr '\n' ' ' | sed 's/ $//'; }
# PTY の親側の fd（macOS の lsof では /dev/ptmx として出る）
pty_count() { lsof -p "$APP_PID" 2>/dev/null | grep -c '/dev/ptmx' | tr -d ' '; }
child_count() { pgrep -P "$APP_PID" 2>/dev/null | grep -c . | tr -d ' '; }
zombie_count() {
  ps -A -o ppid= -o stat= 2>/dev/null | awk -v p="$APP_PID" '$1 == p && $2 ~ /^Z/' | grep -c . | tr -d ' '
}
pane_count() {
  iso_cli list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(sum(len(t["panes"]) for t in d["tabs"]))'
}
first_pane() {
  iso_cli list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}

#   start_app <ログの名前> [VAR=VAL …]
# 蓋閉じの機では tako-vd が起動の合間に眠り、GUI が窓を開かずに終わる（#1160 の設計どおり）。
# その終わり方のときだけやり直す（launch_isolated_gui の ensure が毎回面を起こす）
start_app() {
  local name="$1" try log
  shift
  for try in 1 2 3 4 5; do
    log="$TMP/app-${name}-${try}.log"
    launch_isolated_gui "$log" ${@+"$@"} || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    if wait_isolated_gui "$log" >/dev/null; then
      return 0
    fi
    stop_isolated_gui "$APP_PID"
    APP_PID=""
    if ! grep -q '窓を開かずに終了' "$log" 2>/dev/null; then
      echo "tako-app へ接続できない:"
      tail -20 "$log"
      exit 1
    fi
    echo "  （tako-vd が眠っていて窓を開けなかった。やり直す: ${try} 回目）"
    sleep 2
  done
  echo "tako-vd を起こせず GUI を立てられない（未実測）"
  exit 4
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# 外から `tmux ls` を 10ms おきに打つ見張り（ソケットごとに 1 本のスレッド）と、
# ① tmux open のペインを閉じる / ② GUI を終了する操作の計測
MEASURE="$TMP/measure.py"
cat > "$MEASURE" <<'PYEOF'
import json, os, re, signal, subprocess, sys, threading, time


def run(argv):
    t0 = time.monotonic()
    out = subprocess.run(argv, capture_output=True, text=True)
    return (time.monotonic() - t0) * 1000, out


class Watch:
    """1 つのソケットへ `tmux ls` を 10ms おきに打ち、(打った時刻, 所要, セッション数) を貯める"""

    def __init__(self, sock, base):
        self.sock, self.base, self.events = sock, base, []
        self.stop = threading.Event()
        self.th = threading.Thread(target=self.loop)
        self.th.start()

    def loop(self):
        while not self.stop.is_set():
            at = (time.monotonic() - self.base) * 1000
            dt, o = run(["tmux", "-L", self.sock, "list-sessions", "-F", "#{session_name}"])
            self.events.append((at, dt, o.stdout.split()))
            time.sleep(0.01)

    def finish(self):
        self.stop.set()
        self.th.join()

    def worst(self, start, end):
        return max((e[1] for e in self.events if start <= e[0] <= end), default=0.0)

    def control(self, t):
        # 操作の前 1 秒 = 何もしない窓（機の混み具合で `tmux ls` がどれだけ揺れるかの対照）
        return max((e[1] for e in self.events if t - 1000 <= e[0] < t - e[1]), default=0.0)


def tako(bin_, *a):
    return run([bin_, *a])


def pane_of(out):
    try:
        return int(json.loads(out)["pane"])
    except Exception:
        m = re.search(r'"?pane"?\D*(\d+)', out)
        return int(m.group(1)) if m else None


def open_rounds(bin_, user_sock, vessel_sock, rounds, limit):
    """① tako tmux open のペインを開いて閉じる。利用者役のサーバー（と器のサーバー）を見張る"""
    _, out = tako(bin_, "list")
    p1 = json.loads(out.stdout)["tabs"][0]["panes"][0]["id"]
    worst_u, worst_v, ctrl_u, close_all, wrapper_left = [], [], [], [], 0
    for i in range(rounds):
        _, out = tako(bin_, "tmux", "open", "work", "--socket", user_sock, "--pane", str(p1))
        new = pane_of(out.stdout)
        if new is None:
            print(f"OPEN_FAILED {out.stdout.strip()} {out.stderr.strip()}", flush=True)
            sys.exit(1)
        time.sleep(1.5)
        subprocess.run(["tmux", "-L", user_sock, "send-keys", "-t", "work", "seq 1 60", "Enter"])
        time.sleep(0.3)
        base = time.monotonic()
        wu = Watch(user_sock, base)
        wv = Watch(vessel_sock, base) if vessel_sock != "-" else None
        time.sleep(1.0)
        t_close = (time.monotonic() - base) * 1000
        dt_close, out = tako(bin_, "close", "--pane", str(new))
        if out.returncode != 0:
            print(f"CLOSE_FAILED {out.stderr.strip()}", flush=True)
        time.sleep(1.0)
        wu.finish()
        if wv:
            wv.finish()
        # 窓は「close を打ってから、close が返った 1 秒後まで」
        end = t_close + dt_close + 1000
        u = wu.worst(t_close, end)
        v = wv.worst(t_close, end) if wv else 0.0
        c = wu.control(t_close)
        last = wu.events[-1][2] if wu.events else []
        left = [s for s in last if s.startswith("tako-view-")]
        wrapper_left += 1 if left else 0
        worst_u.append(u)
        worst_v.append(v)
        ctrl_u.append(c)
        close_all.append(dt_close)
        vtxt = f" 器のサーバーの最大={v:.0f}ms" if wv else ""
        print(
            f"  #{i} close={dt_close:.0f}ms 利用者役のサーバーの tmux ls の最大={u:.0f}ms"
            f"（閉じる前 1 秒の対照={c:.0f}ms）{vtxt} 残ったラッパー={len(left)}",
            flush=True,
        )
        time.sleep(0.5)
    print(
        "SUMMARY max_ls=%d min_ls=%d over=%d vessel_max=%d vessel_over=%d control_max=%d control_over=%d wrapper_left=%d close_max=%d"
        % (
            round(max(worst_u)),
            round(min(worst_u)),
            sum(1 for w in worst_u if w > limit),
            round(max(worst_v)),
            sum(1 for w in worst_v if w > limit),
            round(max(ctrl_u)),
            sum(1 for w in ctrl_u if w > limit),
            wrapper_left,
            round(max(close_all)),
        )
    )


def proc_gone(pid):
    """pid が終わったか（ゾンビ = 終わって親の回収待ちも終わった扱い）"""
    o = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
    stat = o.stdout.strip()
    return stat == "" or stat.startswith("Z")


def quit_once(pid, socks, limit):
    """② GUI へ SIGTERM（#777 の正規の quit）を送り、終わるまでの時間と各サーバーの tmux ls を測る"""
    base = time.monotonic()
    watches = [Watch(s, base) for s in socks]
    time.sleep(1.0)
    t_kill = (time.monotonic() - base) * 1000
    os.kill(pid, signal.SIGTERM)
    while not proc_gone(pid):
        if (time.monotonic() - base) * 1000 - t_kill > 15000:
            break
        time.sleep(0.005)
    t_exit = (time.monotonic() - base) * 1000
    time.sleep(1.0)
    for w in watches:
        w.finish()
    parts = []
    for s, w in zip(socks, watches):
        worst = w.worst(t_kill, t_exit + 1000)
        parts.append(
            "%s=%d control_%s=%d" % (s.rsplit("-", 1)[-1], round(worst), s.rsplit("-", 1)[-1], round(w.control(t_kill)))
        )
    print("QUIT exit_ms=%d %s" % (round(t_exit - t_kill), " ".join(parts)))


if sys.argv[1] == "open":
    open_rounds(sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5]), float(sys.argv[6]))
elif sys.argv[1] == "quit":
    quit_once(int(sys.argv[2]), sys.argv[3].split(","), float(sys.argv[4]))
PYEOF

# SUMMARY / QUIT 行から欄を 1 つ読む。欄名の前の空白まで合わせる（`over=` が `control_over=` に当たらない）
summary_field() { printf '%s\n' "$1" | tr ' ' '\n' | sed -n 's/^'"$2"'=\([0-9]*\)$/\1/p'; }

#   open_round <名前> <persist 0|1> [VAR=VAL …]   ① tmux open のペインの close
open_round() {
  local name="$1" persist="$2" out summary base_pty base_child after_pty after_child zombies vsock="-"
  shift 2
  use_dirs "$name"
  export TAKO_PERSIST="$persist"
  [ "$persist" = 1 ] && vsock="$TAKO_TMUX_SOCKET"
  echo "== ${name}: tako tmux open のペイン（TAKO_PERSIST=${persist}）を ${ROUNDS} 回閉じる =="
  # 利用者役の tmux サーバー（隔離ソケット・設定を読まない）
  um -f /dev/null new-session -d -s work -x 120 -y 40 || { fail "${name}: 利用者役の tmux を立てられない"; return; }
  start_app "$name" ${@+"$@"}
  sleep 3
  base_pty="$(pty_count)"
  base_child="$(child_count)"
  echo "  開く前: PTY の親側 ${base_pty} 本・GUI の子 ${base_child} 本・利用者役のセッション [$(user_sessions)]"
  out="$(python3 -I "$MEASURE" open "$TAKO_BIN" "$USER_SOCK" "$vsock" "$ROUNDS" "$LIMIT_MS")"
  printf '%s\n' "$out" | grep -v '^SUMMARY'
  summary="$(printf '%s\n' "$out" | grep '^SUMMARY')"
  sleep 1
  after_pty="$(pty_count)"
  after_child="$(child_count)"
  zombies="$(zombie_count)"
  echo "  閉じた後: PTY の親側 ${after_pty} 本・GUI の子 ${after_child} 本・ゾンビ ${zombies} 本・利用者役のセッション [$(user_sessions)]"
  eval "RESULT_${name}_MAX=$(summary_field "$summary" max_ls)"
  eval "RESULT_${name}_MIN=$(summary_field "$summary" min_ls)"
  eval "RESULT_${name}_OVER=$(summary_field "$summary" over)"
  eval "RESULT_${name}_VMAX=$(summary_field "$summary" vessel_max)"
  eval "RESULT_${name}_VOVER=$(summary_field "$summary" vessel_over)"
  eval "RESULT_${name}_CONTROL_MAX=$(summary_field "$summary" control_max)"
  eval "RESULT_${name}_CONTROL_OVER=$(summary_field "$summary" control_over)"
  check_eq "${name}: 利用者のセッションは残りラッパーは消える" "work" "$(user_sessions)"
  check_eq "${name}: 閉じた後にラッパーが残った回が無い" 0 "$(summary_field "$summary" wrapper_left)"
  check_eq "${name}: PTY の親側の fd が開く前と同じ数へ戻る" "$base_pty" "$after_pty"
  check_eq "${name}: GUI の子プロセスが開く前と同じ数へ戻る" "$base_child" "$after_child"
  check_eq "${name}: ゾンビが無い" 0 "$zombies"
  stop_app
  um kill-server >/dev/null 2>&1 || true
}

#   quit_round <名前> [VAR=VAL …]   ② 器のペインを 3 枚持つ GUI の終了 → 起動し直し
quit_round() {
  local name="$1" i out p1 p2 p3 worst=0 over=0 ctrl=0 ctrl_over=0 exit_max=0 exit_sum=0 v c e
  shift
  use_dirs "$name"
  export TAKO_PERSIST=1
  echo "== ${name}: 器のペイン 3 枚の GUI を ${ROUNDS} 回終了して起動し直す =="
  start_app "${name}-0" ${@+"$@"}
  p1="$(first_pane)"
  p2="$(iso_cli split --pane "$p1" --right | tr -dc '0-9')"
  p3="$(iso_cli split --pane "$p2" --down | tr -dc '0-9')"
  sleep 2
  for i in $(seq 1 "$ROUNDS"); do
    for p in $(iso_cli list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(" ".join(str(p["id"]) for t in d["tabs"] for p in t["panes"]))'); do
      iso_cli send --pane "$p" "seq 1 60" >/dev/null 2>&1
    done
    sleep 1
    check_eq "${name} #${i}: 終了の前の器は 3 本" 3 "$(vessel_count)"
    out="$(python3 -I "$MEASURE" quit "$APP_PID" "$TAKO_TMUX_SOCKET" "$LIMIT_MS")"
    echo "  #${i} ${out}"
    # 終わった GUI を回収し、ペインのプロセスの残りを待つ（ヘルパの 1 実装）
    stop_app
    v="$(summary_field "$out" "${name}")"
    c="$(summary_field "$out" "control_${name}")"
    e="$(summary_field "$out" exit_ms)"
    [ "$v" -gt "$worst" ] && worst="$v"
    [ "$v" -gt "$LIMIT_MS" ] && over=$((over + 1))
    [ "$c" -gt "$ctrl" ] && ctrl="$c"
    [ "$c" -gt "$LIMIT_MS" ] && ctrl_over=$((ctrl_over + 1))
    [ "$e" -gt "$exit_max" ] && exit_max="$e"
    exit_sum=$((exit_sum + e))
    check_eq "${name} #${i}: 終了後も器は 3 本残る" 3 "$(vessel_count)"
    check_eq "${name} #${i}: 終了後に器の attach クライアントが残らない" 0 "$(vessel_clients)"
    start_app "${name}-${i}" ${@+"$@"}
    sleep 3
    check_eq "${name} #${i}: 起動し直しでペインが 3 枚戻る" 3 "$(pane_count)"
    check_eq "${name} #${i}: 起動し直しの後も器は 3 本" 3 "$(vessel_count)"
  done
  echo "  起動し直した GUI: PTY の親側 $(pty_count) 本・GUI の子 $(child_count) 本・ゾンビ $(zombie_count) 本"
  check_eq "${name}: 起動し直した GUI にゾンビが無い" 0 "$(zombie_count)"
  stop_app
  eval "RESULT_${name}_MAX=$worst"
  eval "RESULT_${name}_OVER=$over"
  eval "RESULT_${name}_CONTROL_MAX=$ctrl"
  eval "RESULT_${name}_CONTROL_OVER=$ctrl_over"
  eval "RESULT_${name}_EXIT_MAX=$exit_max"
  eval "RESULT_${name}_EXIT_AVG=$((exit_sum / ROUNDS))"
}

#   quit_open_round <名前> [VAR=VAL …]   器なしの tmux open のペインを開いたまま GUI を終了する（利用者役のサーバーを見る）
quit_open_round() {
  local name="$1" i out p1 v e worst=0 over=0 exit_max=0
  shift
  use_dirs "$name"
  export TAKO_PERSIST=0
  echo "== ${name}: 器なしの tmux open のペインを開いたまま GUI を ${ROUNDS} 回終了する =="
  um -f /dev/null new-session -d -s work -x 120 -y 40 || { fail "${name}: 利用者役の tmux を立てられない"; return; }
  for i in $(seq 1 "$ROUNDS"); do
    start_app "${name}-${i}" ${@+"$@"}
    p1="$(first_pane)"
    iso_cli tmux open work --socket "$USER_SOCK" --pane "$p1" >/dev/null
    sleep 1.5
    um send-keys -t work 'seq 1 60' Enter
    sleep 0.5
    case "$(user_sessions)" in
      "tako-view-work-"*" work") pass "${name} #${i}: 終了の前の利用者役のセッションは本体とラッパー" ;;
      *) fail "${name} #${i}: 終了の前の利用者役のセッションは本体とラッパー（実際 '$(user_sessions)'）" ;;
    esac
    out="$(python3 -I "$MEASURE" quit "$APP_PID" "$USER_SOCK" "$LIMIT_MS")"
    echo "  #${i} ${out}"
    stop_app
    v="$(summary_field "$out" user)"
    e="$(summary_field "$out" exit_ms)"
    [ "$v" -gt "$worst" ] && worst="$v"
    [ "$v" -gt "$LIMIT_MS" ] && over=$((over + 1))
    [ "$e" -gt "$exit_max" ] && exit_max="$e"
    # ラッパーは destroy-unattached で消え、利用者のセッションは残る
    check_eq "${name} #${i}: 終了後も利用者のセッションは残りラッパーは消える" "work" "$(user_sessions)"
  done
  um kill-server >/dev/null 2>&1 || true
  eval "RESULT_${name}_MAX=$worst"
  eval "RESULT_${name}_OVER=$over"
  eval "RESULT_${name}_EXIT_MAX=$exit_max"
}

open_round opendirect 0
open_round openvessel 1
quit_round quit
quit_open_round quitview
if [ "$ONLY" != fixed ]; then
  open_round opendirectlegacy 0 TAKO_2634_LEGACY=1
fi

echo "== まとめ（操作の直後の tmux ls の最大。${ROUNDS} 回。対照 = 操作の前 1 秒の最大）=="
echo "  ① 器なしの tmux open の close: 利用者役 最大 ${RESULT_opendirect_MAX}ms・最小 ${RESULT_opendirect_MIN}ms（${LIMIT_MS}ms 超え ${RESULT_opendirect_OVER} 回）・対照 ${RESULT_opendirect_CONTROL_MAX}ms"
echo "  ① 器ありの tmux open の close: 利用者役 最大 ${RESULT_openvessel_MAX}ms（${LIMIT_MS}ms 超え ${RESULT_openvessel_OVER} 回）・器 最大 ${RESULT_openvessel_VMAX}ms（${RESULT_openvessel_VOVER} 回）・対照 ${RESULT_openvessel_CONTROL_MAX}ms"
echo "  ② 終了: 器 最大 ${RESULT_quit_MAX}ms（${LIMIT_MS}ms 超え ${RESULT_quit_OVER} 回）・対照 ${RESULT_quit_CONTROL_MAX}ms・終了の所要 最大 ${RESULT_quit_EXIT_MAX}ms / 平均 ${RESULT_quit_EXIT_AVG}ms"
echo "  ② 器なしの tmux open のペインを開いたままの終了: 利用者役 最大 ${RESULT_quitview_MAX}ms（${LIMIT_MS}ms 超え ${RESULT_quitview_OVER} 回）・終了の所要 最大 ${RESULT_quitview_EXIT_MAX}ms"
check_eq "① 器なしの tmux open の close の直後の tmux ls が ${LIMIT_MS}ms を超えない" 0 "$RESULT_opendirect_OVER"
check_eq "① 器ありの tmux open の close の直後の tmux ls（利用者役）が ${LIMIT_MS}ms を超えない" 0 "$RESULT_openvessel_OVER"
check_eq "① 器ありの tmux open の close の直後の tmux ls（器）が ${LIMIT_MS}ms を超えない" 0 "$RESULT_openvessel_VOVER"
check_eq "② 終了の直後の器の tmux ls が ${LIMIT_MS}ms を超えない" 0 "$RESULT_quit_OVER"
check_eq "② 器なしの tmux open のペインを開いたままの終了の直後の利用者役の tmux ls が ${LIMIT_MS}ms を超えない" 0 "$RESULT_quitview_OVER"
if [ "$ONLY" != fixed ]; then
  echo "  TAKO_2634_LEGACY=1: ① 器なし 最大 ${RESULT_opendirectlegacy_MAX}ms・最小 ${RESULT_opendirectlegacy_MIN}ms（${LIMIT_MS}ms 超え ${RESULT_opendirectlegacy_OVER} 回）"
  if [ "$RESULT_opendirectlegacy_MIN" -ge "$LEGACY_MIN_MS" ]; then
    pass "A/B: TAKO_2634_LEGACY=1 では ① が毎回 ${LEGACY_MIN_MS}ms 以上止まる（修正前の症状が同じバイナリで出る）"
  else
    fail "A/B: TAKO_2634_LEGACY=1 で ① が止まらない回がある（最小 ${RESULT_opendirectlegacy_MIN}ms）"
  fi
fi

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
