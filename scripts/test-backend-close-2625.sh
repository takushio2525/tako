#!/usr/bin/env bash
# test-backend-close-2625.sh — 器のペインを閉じても器の tmux サーバー全体を止めない（#2625）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#   ① 器のペインを CLI で閉じた直後から、外から 10ms おきに打つ `tmux ls`（隔離ソケット）が
#      50ms を超えない（修正前は閉じるたびに約 600ms 止まった）。閉じた器は消える
#   ② 閉じた後に PTY の親側の fd（`lsof -p <GUI>` の /dev/ptmx）・GUI の子プロセスが閉じる前と同じ数へ戻り、
#      ゾンビが無い。続けて N 回（既定 10）閉じても積み上がらない
#   ③ 普通の終了（SIGTERM）→ 起動し直しで、閉じたペインの器は戻らず（幽霊が出ない）残したペインは戻る
#   ④ 直接ペイン（器なし = TAKO_PERSIST=0）の close は従来どおり（fd・子プロセスが戻る）
#   ⑤ A/B: `TAKO_2625_LEGACY=1` で同じバイナリのまま約 600ms の止まりが再現し、②③は修正後と同じ
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットの器だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る = 他の
# `isolated-gui.sh` 系スクリプトと同じ扱い）。CI 側の担保は単体テスト
# （`tako_core::terminal` の `器のクライアントの端末は…` 3 件）と番犬
# （`crates/tako-control/tests/issue2625_backend_client_hangup_watchdog.rs`）。
#
# 使い方: bash scripts/test-backend-close-2625.sh
#        TAKO_2625_ROUNDS=3 bash scripts/test-backend-close-2625.sh   # 閉じる回数を変える
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
ROUNDS="${TAKO_2625_ROUNDS:-10}"
# 修正後に許す閉じた直後の `tmux ls` の最大（ms）と、旧挙動を「再現した」と読む下限（ms）
LIMIT_MS=50
LEGACY_MIN_MS=300

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2625-XXXXXX)"
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

# 置き場を A/B ごとに分ける（同じ dir・ソケットを使うと 2 回目が前の器を拾う）
use_dirs() {
  local root="$TMP/$1"
  export TAKO_DATA_DIR="$root/data"
  export TAKO_DISCOVERY_DIR="$root/disc"
  export TAKO_ORCHESTRATOR_DIR="$root/orch"
  export TAKO_SESSIONS_FILE="$root/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$root/panelogs"
  export TAKO_WORKERS_FILE="$root/workers.yaml"
  export TAKO_TMUX_SOCKET="tako-2625-$$-$1"
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  case "$TAKO_DATA_DIR" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;;
  esac
}

tm() { tmux -L "$TAKO_TMUX_SOCKET" "$@"; }
vessel_count() { tm ls -F '#{session_name}' 2>/dev/null | grep -c . | tr -d ' '; }
# PTY の親側の fd（macOS の lsof では /dev/ptmx として出る）
pty_count() { lsof -p "$APP_PID" 2>/dev/null | grep -c '/dev/ptmx' | tr -d ' '; }
child_count() { pgrep -P "$APP_PID" 2>/dev/null | grep -c . | tr -d ' '; }
zombie_count() {
  ps -A -o ppid= -o stat= 2>/dev/null | awk -v p="$APP_PID" '$1 == p && $2 ~ /^Z/' | grep -c . | tr -d ' '
}
pane_count() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(sum(len(t["panes"]) for t in d["tabs"]))'
}
tab_count() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
print(len(json.load(sys.stdin)["tabs"]))'
}
first_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
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

# 閉じる操作の計測（split → 出力 → 閉じる。閉じる前後に外から `tmux ls` を 10ms おきに打つ）
MEASURE="$TMP/measure.py"
cat > "$MEASURE" <<'PYEOF'
import json, subprocess, sys, threading, time

bin_, sock, rounds = sys.argv[1], sys.argv[2], int(sys.argv[3])
backend = sock != "-"


def run(argv):
    t0 = time.monotonic()
    out = subprocess.run(argv, capture_output=True, text=True)
    return (time.monotonic() - t0) * 1000, out


def tako(*a):
    return run([bin_, *a])


_, out = tako("list")
p1 = json.loads(out.stdout)["tabs"][0]["panes"][0]["id"]
worst_all, gone_all, close_all, control_all = [], [], [], []
for i in range(rounds):
    _, out = tako("split", "--pane", str(p1), "--right")
    new = int("".join(c for c in out.stdout if c.isdigit()))
    time.sleep(1.5)
    tako("send", "--pane", str(new), "seq 1 60")
    time.sleep(0.05)
    events, stop = [], threading.Event()
    base = time.monotonic()

    def loop():
        while backend and not stop.is_set():
            at = (time.monotonic() - base) * 1000
            dt, o = run(["tmux", "-L", sock, "list-sessions", "-F", "#{session_name}"])
            events.append((at, dt, len(o.stdout.split())))
            time.sleep(0.01)

    th = threading.Thread(target=loop)
    th.start()
    # 閉じる前の 1 秒 = 何もしない窓（機の混み具合で `tmux ls` がどれだけ揺れるかの対照）
    time.sleep(1.0)
    t_close = (time.monotonic() - base) * 1000
    n_before = events[-1][2] if events else 0
    dt_close, out = tako("close", "--pane", str(new))
    if out.returncode != 0:
        print(f"CLOSE_FAILED {out.stderr.strip()}")
    time.sleep(1.0)
    stop.set()
    th.join()
    # 窓は「close を打ってから、close が返った 1 秒後まで」（機が詰まって close の応答が遅れた回も
    # 止まりを窓の外へ逃がさない）
    window = [e for e in events if t_close <= e[0] <= t_close + dt_close + 1000]
    worst = max((e[1] for e in window), default=0.0)
    control = max((e[1] for e in events if t_close - 1000 <= e[0] < t_close - e[1]), default=0.0)
    control_all.append(control)
    gone = next((e[0] + e[1] - t_close for e in window if e[2] == n_before - 1), None)
    worst_all.append(worst)
    close_all.append(dt_close)
    gone_all.append(gone)
    gone_txt = "-" if gone is None else f"{gone:.0f}ms"
    print(
        f"  #{i} close={dt_close:.0f}ms 閉じてから返った 1 秒後までの tmux ls の最大={worst:.0f}ms"
        f"（閉じる前 1 秒の対照={control:.0f}ms） 器が消えた={gone_txt}",
        flush=True,
    )
    time.sleep(0.5)
print(
    "SUMMARY max_ls=%d min_ls=%d over=%d control_max=%d control_over=%d gone_missing=%d close_max=%d"
    % (
        round(max(worst_all)),
        round(min(worst_all)),
        sum(1 for w in worst_all if w > float(sys.argv[4])),
        round(max(control_all)),
        sum(1 for w in control_all if w > float(sys.argv[4])),
        sum(1 for g in gone_all if g is None) if backend else 0,
        round(max(close_all)),
    )
)
PYEOF

# SUMMARY 行から欄を 1 つ読む。欄名の前の空白まで合わせる（`over=` が `control_over=` に当たらない）
summary_field() { printf '%s\n' "$1" | tr ' ' '\n' | sed -n 's/^'"$2"'=\([0-9]*\)$/\1/p'; }

#   backend_round <名前> [VAR=VAL …]   器のペインの close を A/B のどちらかで測る
backend_round() {
  local name="$1" out summary base_pty base_child after_pty after_child zombies p1 p2 p3
  shift
  use_dirs "$name"
  export TAKO_PERSIST=1
  echo "== ${name}: 器のペイン（TAKO_PERSIST=1）を ${ROUNDS} 回閉じる =="
  start_app "$name" ${@+"$@"}
  sleep 3
  base_pty="$(pty_count)"
  base_child="$(child_count)"
  echo "  閉じる前: PTY の親側 ${base_pty} 本・GUI の子 ${base_child} 本・器 $(vessel_count) 本"
  out="$(python3 -I "$MEASURE" "$TAKO_BIN" "$TAKO_TMUX_SOCKET" "$ROUNDS" "$LIMIT_MS")"
  printf '%s\n' "$out" | grep -v '^SUMMARY'
  summary="$(printf '%s\n' "$out" | grep '^SUMMARY')"
  sleep 1
  after_pty="$(pty_count)"
  after_child="$(child_count)"
  zombies="$(zombie_count)"
  echo "  閉じた後: PTY の親側 ${after_pty} 本・GUI の子 ${after_child} 本・ゾンビ ${zombies} 本・器 $(vessel_count) 本"
  eval "RESULT_${name}_MAX=$(summary_field "$summary" max_ls)"
  eval "RESULT_${name}_MIN=$(summary_field "$summary" min_ls)"
  eval "RESULT_${name}_OVER=$(summary_field "$summary" over)"
  eval "RESULT_${name}_CONTROL_MAX=$(summary_field "$summary" control_max)"
  eval "RESULT_${name}_CONTROL_OVER=$(summary_field "$summary" control_over)"
  check_eq "${name}: 閉じた器はすべて消える" 0 "$(summary_field "$summary" gone_missing)"
  check_eq "${name}: 器は最初の 1 本だけ残る" 1 "$(vessel_count)"
  check_eq "${name}: PTY の親側の fd が閉じる前と同じ数へ戻る" "$base_pty" "$after_pty"
  check_eq "${name}: GUI の子プロセスが閉じる前と同じ数へ戻る" "$base_child" "$after_child"
  check_eq "${name}: ゾンビが無い" 0 "$zombies"

  echo "  -- 終了と起動し直し（閉じたペインの器が幽霊にならない）--"
  p1="$(first_pane)"
  p2="$("$TAKO_BIN" split --pane "$p1" --right | tr -dc '0-9')"
  p3="$("$TAKO_BIN" split --pane "$p2" --down | tr -dc '0-9')"
  sleep 2
  "$TAKO_BIN" close --pane "$p3" >/dev/null
  sleep 3   # layout.json の保存と器の kill を待つ
  check_eq "${name}: 閉じる前の構成（3 枚 → 1 枚閉じて 2 枚）" 2 "$(pane_count)"
  stop_app
  check_eq "${name}: 終了後に残る器は残したペインの 2 本だけ" 2 "$(vessel_count)"
  start_app "${name}-restart" ${@+"$@"}
  sleep 4
  check_eq "${name}: 起動し直しでペインが 2 枚戻る" 2 "$(pane_count)"
  check_eq "${name}: タブは 1 枚（器の取り込み直しのタブが出ない）" 1 "$(tab_count)"
  check_eq "${name}: 起動し直しの後も器は 2 本" 2 "$(vessel_count)"
  stop_app
}

#   direct_round   直接ペイン（器なし）の close は従来どおり
direct_round() {
  local out base_pty base_child after_pty after_child zombies
  use_dirs direct
  export TAKO_PERSIST=0
  echo "== direct: 直接ペイン（TAKO_PERSIST=0）を 3 回閉じる =="
  start_app direct
  sleep 3
  base_pty="$(pty_count)"
  base_child="$(child_count)"
  out="$(python3 -I "$MEASURE" "$TAKO_BIN" - 3 "$LIMIT_MS")"
  printf '%s\n' "$out" | grep -v '^SUMMARY'
  sleep 1
  after_pty="$(pty_count)"
  after_child="$(child_count)"
  zombies="$(zombie_count)"
  echo "  閉じる前: PTY の親側 ${base_pty} 本・GUI の子 ${base_child} 本 / 閉じた後: ${after_pty} 本・${after_child} 本・ゾンビ ${zombies} 本"
  check_eq "direct: 器を作らない" 0 "$(vessel_count)"
  check_eq "direct: PTY の親側の fd が閉じる前と同じ数へ戻る" "$base_pty" "$after_pty"
  check_eq "direct: GUI の子プロセスが閉じる前と同じ数へ戻る" "$base_child" "$after_child"
  check_eq "direct: ゾンビが無い" 0 "$zombies"
  stop_app
}

backend_round fixed
backend_round legacy TAKO_2625_LEGACY=1
direct_round

echo "== A/B（閉じてから close が返った 1 秒後までの tmux ls の最大。${ROUNDS} 回）=="
echo "  修正後: 最大 ${RESULT_fixed_MAX}ms・最小 ${RESULT_fixed_MIN}ms（${LIMIT_MS}ms 超え ${RESULT_fixed_OVER} 回）・閉じる前の対照の最大 ${RESULT_fixed_CONTROL_MAX}ms（${LIMIT_MS}ms 超え ${RESULT_fixed_CONTROL_OVER} 回）"
echo "  TAKO_2625_LEGACY=1: 最大 ${RESULT_legacy_MAX}ms・最小 ${RESULT_legacy_MIN}ms（${LIMIT_MS}ms 超え ${RESULT_legacy_OVER} 回）・閉じる前の対照の最大 ${RESULT_legacy_CONTROL_MAX}ms（${LIMIT_MS}ms 超え ${RESULT_legacy_CONTROL_OVER} 回）"
check_eq "修正後: 閉じた直後の tmux ls が ${LIMIT_MS}ms を超えない" 0 "$RESULT_fixed_OVER"
if [ "$RESULT_legacy_MIN" -ge "$LEGACY_MIN_MS" ]; then
  pass "A/B: TAKO_2625_LEGACY=1 では毎回 ${LEGACY_MIN_MS}ms 以上止まる（修正前の症状が同じバイナリで出る）"
else
  fail "A/B: TAKO_2625_LEGACY=1 で止まらない回がある（最小 ${RESULT_legacy_MIN}ms）"
fi

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
