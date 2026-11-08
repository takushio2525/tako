#!/usr/bin/env bash
# test-direct-close-2803.sh — 器なしのペインを閉じたら、シェルとその子・PTY の親側の fd が残らない（#2803）
#
# 隔離した data で器なし（TAKO_PERSIST=0）の**実 tako-app** を立て、前景のジョブを変えたペインを
# split → send → close して、閉じた後に
#   ① ペインのシェル（zsh）とその子が消える（修正前は前景が `bash -c '…'` のとき zsh `?Es` / bash `?E` で
#      GUI を終了するまで残った）
#   ② GUI の PTY の親側の fd（`lsof -p <GUI>` の /dev/ptmx）が閉じる前より 1 本減る
# を実測する。前景のジョブ（Issue #2803 の対照表 + 境界）:
#   sleep 600（zsh から直接）/ zsh -c 'sleep 600; :' / bash -c 'sleep 600; :' /
#   出力し続ける bash / シェル自身が HUP を無視（trap '' HUP）/ ジョブが HUP を無視
#   （ジョブは他の端末と同じく走り続ける = nohup の形。シェルが消え fd が閉じることだけを見る。
#    残った子はこのスクリプトが控えた pid だけを止める）
# A/B: `TAKO_2803_LEGACY=1` で同じバイナリのまま bash -c の行が残る（修正前の症状）ことも確かめる。
# UI スレッドの所要は `TAKO_PERF_VERBOSE=1` の perf.log の分布（`dispatch:Close`）を両方で並べる。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下・落とすのは自分で起こした pid だけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る = 他の `isolated-gui.sh` 系スクリプトと
# 同じ扱い）。CI 側の担保は単体テスト（`tako_core::terminal` の `印を立てた端末は…` 2 件）と番犬
# （`crates/tako-control/tests/issue2803_direct_pane_hangup_watchdog.rs`）。
#
# 使い方: bash scripts/test-direct-close-2803.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
# 閉じてからシェルが消えるまで待つ上限（0.1 秒 × この回数。読み捨ての上限 2 秒 + 回線断の分を含む）
GONE_TRIES=60

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2803-XXXXXX)"
APP_PID=""
# HUP を無視して走り続ける想定の子（「pid 起動時刻」の行）。片付けで自分の控えた pid だけ止める
LEFT=""
cleanup() {
  local pid start now
  stop_isolated_gui "$APP_PID"
  while read -r pid start; do
    [ -n "$pid" ] || continue
    now=$(lstart_of "$pid")
    # 起動時刻が同じ = 控えたときと同じプロセス（pid の使い回しで別人を撃たない）
    [ -n "$now" ] && [ "$now" = "$start" ] && kill -9 "$pid" 2>/dev/null
  done <<<"$LEFT"
  rm -rf "${TMP:?}"
}
trap cleanup EXIT
# 起動時刻（空白を 1 つへ詰める。`ps` は末尾に空白を付け、`read` は付けないので素のままだと照合が外れる）
lstart_of() { ps -o lstart= -p "$1" 2>/dev/null | awk '{ $1 = $1; print }'; }

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_PERSIST=0

# 置き場を A/B ごとに分ける（同じ dir・ソケットを使うと 2 回目が前の状態を拾う）
use_dirs() {
  local root="$TMP/$1"
  export TAKO_DATA_DIR="$root/data"
  export TAKO_DISCOVERY_DIR="$root/disc"
  export TAKO_ORCHESTRATOR_DIR="$root/orch"
  export TAKO_SESSIONS_FILE="$root/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$root/panelogs"
  export TAKO_WORKERS_FILE="$root/workers.yaml"
  export TAKO_TMUX_SOCKET="tako-2803-$$-$1"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  case "$TAKO_DATA_DIR" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;;
  esac
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

# PTY の親側の fd（macOS の lsof では /dev/ptmx として出る）
pty_count() { lsof -p "$APP_PID" 2>/dev/null | grep -c '/dev/ptmx' | tr -d ' '; }
now_ms() { python3 -I -c 'import time; print(int(time.time() * 1000))'; }
first_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}
# 生きている（ゾンビでない）pid だけを並べる
alive_of() {
  local pid st out=""
  for pid in "$@"; do
    st=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')
    case "$st" in
      "" | Z*) : ;;
      *) out="$out $pid" ;;
    esac
  done
  printf '%s\n' "${out# }"
}
stats_of() { ps -o pid= -o stat= -p "$(printf '%s' "$*" | tr ' ' ',')" 2>/dev/null | awk '{ printf "%s:%s ", $1, $2 }'; }

#   run_case <名前> <期待> <前景のコマンド>
# 期待: gone = シェルも子も消える / job = シェルは消え、HUP を無視する子は走り続ける /
#       stays = シェルが残る（A/B の旧挙動の再現）
run_case() {
  local name="$1" expect="$2" cmd="$3" before after shell kids new base_pty t0 gone_ms i shell_left kids_left after_pty pid
  before=$(pgrep -P "$APP_PID" | sort)
  new=$("$TAKO_BIN" split --pane "$FIRST" --right | tr -dc '0-9')
  sleep 2
  after=$(pgrep -P "$APP_PID" | sort)
  shell=$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -1)
  if [ -z "$new" ] || [ -z "$shell" ]; then
    fail "${name}: ペインを作れない（pane='${new}' shell='${shell}'）"
    return
  fi
  "$TAKO_BIN" send --pane "$new" "$cmd" >/dev/null
  sleep 1.5
  kids=$(pgrep -P "$shell" | tr '\n' ' ')
  kids="${kids% }"
  base_pty=$(pty_count)
  t0=$(now_ms)
  "$TAKO_BIN" close --pane "$new" >/dev/null
  gone_ms="-"
  for i in $(seq 1 "$GONE_TRIES"); do
    if [ -z "$(alive_of "$shell")" ]; then
      gone_ms="$(($(now_ms) - t0))ms"
      break
    fi
    sleep 0.1
  done
  # fd は PTY のスレッドが `Pty` を落としたときに閉じる（シェルの回収の直後）
  sleep 0.5
  shell_left=$(alive_of "$shell")
  # shellcheck disable=SC2086
  kids_left=$(alive_of $kids)
  after_pty=$(pty_count)
  echo "  ${name}: 前景=${cmd}"
  echo "    shell=${shell} 子=[${kids}] ptmx ${base_pty} → ${after_pty}・シェルが消えるまで ${gone_ms}・残り=[$(stats_of $shell_left $kids_left)]"
  case "$expect" in
    gone)
      check_eq "${name}: シェルが消える" "" "$shell_left"
      check_eq "${name}: シェルの子が消える" "" "$kids_left"
      check_eq "${name}: PTY の親側の fd が 1 本減る" "$((base_pty - 1))" "$after_pty"
      ;;
    job)
      check_eq "${name}: シェルが消える" "" "$shell_left"
      check_eq "${name}: PTY の親側の fd が 1 本減る" "$((base_pty - 1))" "$after_pty"
      if [ -n "$kids_left" ]; then
        echo "    （HUP を無視する子 ${kids_left} は他の端末と同じく走り続ける。片付けでこのスクリプトが止める）"
      fi
      ;;
    stays)
      if [ -n "$shell_left" ] && [ "$after_pty" = "$base_pty" ]; then
        pass "${name}: シェルが残り PTY の親側の fd も閉じない（修正前の症状）"
      else
        fail "${name}: 修正前の症状が出ない（残り='${shell_left}' ptmx ${base_pty} → ${after_pty}）"
      fi
      ;;
  esac
  for pid in $kids_left; do
    LEFT="${LEFT}${pid} $(lstart_of "$pid")
"
  done
}

# perf.log の verbose 分布から `dispatch:Close` と `ipc_turn` の行を写す（10 秒ごとに出るので待ってから読む）
show_close_stats() {
  local label="$1" log="$TAKO_DATA_DIR/perf.log"
  sleep 11
  echo "  ${label}: UI スレッドの所要（perf.log の verbose 分布）"
  grep -E 'span 統計 \[(dispatch:Close|ipc_turn)\]' "$log" 2>/dev/null | sed 's/^/    /'
  echo "    UI ストールの行: $(grep -c 'UI ストール' "$log" 2>/dev/null | tr -d ' ') 行"
}

echo "== fixed: 器なしのペイン（TAKO_PERSIST=0）を前景のジョブごとに閉じる =="
use_dirs fixed
start_app fixed TAKO_PERF_VERBOSE=1
sleep 2
FIRST="$(first_pane)"
run_case "直接の sleep" gone "sleep 600"
run_case "zsh -c" gone "zsh -c 'sleep 600; :'"
run_case "bash -c" gone "bash -c 'sleep 600; :'"
run_case "出力し続ける bash" gone "bash -c 'while :; do seq 1 200; sleep 0.01; done'"
run_case "HUP を無視するシェル" gone "trap '' HUP"
run_case "HUP を無視するジョブ" job "bash -c \"trap '' HUP; sleep 600; :\""
show_close_stats "fixed"
stop_app

echo "== legacy: TAKO_2803_LEGACY=1（同じバイナリで修正前の閉じ方）=="
use_dirs legacy
start_app legacy TAKO_2803_LEGACY=1 TAKO_PERF_VERBOSE=1
sleep 2
FIRST="$(first_pane)"
run_case "legacy 直接の sleep" gone "sleep 600"
run_case "legacy bash -c" stays "bash -c 'sleep 600; :'"
show_close_stats "legacy"
stop_app

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
