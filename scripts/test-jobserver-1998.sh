#!/bin/bash
# tako:run bash scripts/test-jobserver-1998.sh
#
# cargo の全体並列の上限 = 共有 jobserver（#1998。エピック #1992 の S6）の実経路テスト。
#
# 隔離 GUI（仮想ディスプレイ tako-vd・器あり = TAKO_PERSIST=1・札の枚数は注入 TAKO_1998_INJECT_TOKENS）の
# ペインで **`tako queue` を通さない cargo** を走らせ、眠るだけのビルドスクリプトの区間（と 0.2 秒ごとの
# ps の rustc・ビルドスクリプトの本数）から同時数を数える:
#   ① 3 本並べた cargo の同時数が N + 3 以下（A/B `TAKO_1992_NO_JOBSERVER=1` では超える）
#   ② 隔離 GUI の再起動をまたいでも札が N を超えない（cargo が札を持っている間に GUI を落として立て直す）
#   ③ cargo を kill して失った札が、誰も持っていなくなってから戻る（数え直し）
#   ④ 設定 off の直後に作ったペインには配らない・fifo が消える / on で戻る
#   ⑤ N = 1 の機でも 3 本並べた同時数が N + 3 以下
#   ⑥ GUI を終えたら（札を持ちうる cargo が居なければ）fifo が消え、器のペインの cargo は従来どおり走る
#
# **負荷は生成しない**（ビルドスクリプトは眠るだけ・依存なし・--offline）。A/B だけ cargo の既定の並列で
# 小さいクレートが一度に組まれる（その瞬間の同時数を見るのがこの検査）。
# 落とすのは自分で起こした pid と自分の tmux ソケットだけ（名前一致で殺さない）。

set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL CARGO_MAKEFLAGS

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = 0 ]; then pass "$2"; else fail "$2"; fi; }

# 短い一時 dir（深いと IPC の受け口のパスが sun_path の上限を超える = #1441。空白も入らない = fifo の置き場が
# 設計書どおりの <data_dir>/resources/jobserver.fifo になる）
TMP="$(mktemp -d /tmp/tk1998-XXXXXX)"
SOCKETS=""
SAMPLER_PID=""
cleanup() {
  [ -n "$SAMPLER_PID" ] && kill "$SAMPLER_PID" 2>/dev/null
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  for s in $SOCKETS; do tmux_socket_kill "$s"; done
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1
# ペインで打つのは rustup の代理ではなく本物の cargo（③ で kill する pid を cargo そのものにする）
CARGO_BIN="$(rustup which cargo 2>/dev/null || command -v cargo)" || { echo "cargo が見つからない"; exit 1; }
command -v jq >/dev/null || { echo "jq が見つからない"; exit 1; }
# ペインの cargo は本物のツールチェーンとレジストリを使う（HOME を差し替えても取り直さない）
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/logs"
for d in "$HOME" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
  esac
done

# --- 道具 ---

# gen_ws <dir> <クレート数> <眠る ms> <区間の記録>: 眠るビルドスクリプトだけの workspace
gen_ws() {
  local dir=$1 n=$2 ms=$3 log=$4 i c members=""
  for i in $(seq 1 "$n"); do
    c="c$i"
    members="$members\"$c\","
    mkdir -p "$dir/$c/src"
    printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n' "$c" > "$dir/$c/Cargo.toml"
    : > "$dir/$c/src/lib.rs"
    cat > "$dir/$c/build.rs" <<RS
use std::io::Write;
fn now() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() }
fn main() {
    let s = now();
    std::thread::sleep(std::time::Duration::from_millis($ms));
    let e = now();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open("$log").unwrap();
    writeln!(f, "$c {s} {e}").unwrap();
}
RS
  done
  printf '[workspace]\nresolver = "2"\nmembers = [%s]\n' "${members%,}" > "$dir/Cargo.toml"
}

# overlap <区間の記録>: 同時に走った数の最大（端が触れているだけは数えない）
overlap() {
  awk '{print $2" 1"; print $3" -1"}' "$1" | sort -n -k1,1 -k2,2n | awk '{c+=$2; if(c>m)m=c} END{print m+0}'
}

# sampler_start <出力>: 0.2 秒ごとに、この検査の target dir の rustc とビルドスクリプトの本数を書く
sampler_start() {
  local out=$1
  : > "$out"
  (
    while :; do
      ps -axww -o command= | grep -F "$TMP/t-" | grep -cE '(^|/)rustc |build-script-build' >> "$out"
      sleep 0.2
    done
  ) &
  SAMPLER_PID=$!
}
sampler_stop() {
  [ -n "$SAMPLER_PID" ] && kill "$SAMPLER_PID" 2>/dev/null
  wait "$SAMPLER_PID" 2>/dev/null
  SAMPLER_PID=""
}
max_of() { sort -n "$1" | tail -1; }

status() { "$TAKO_BIN" queue --json 2>/dev/null; }
js() { status | jq -r ".jobserver.$1 // empty"; }

# wait_until <秒> <コマンド…>: 成り立つまで 0.2 秒ごとに見る
wait_until() {
  local limit=$1 i
  shift
  for i in $(seq 1 $((limit * 5))); do
    "$@" && return 0
    sleep 0.2
  done
  return 1
}
free_is() { [ "$(js free)" = "$1" ]; }
free_below() { local f; f="$(js free)"; [ -n "$f" ] && [ "$f" -lt "$1" ]; }

wait_prompt() {
  local p=$1 i
  for i in $(seq 1 100); do
    "$TAKO_BIN" read --pane "$p" 2>/dev/null | grep -q '%' && return 0
    sleep 0.2
  done
  return 1
}
first_pane() { "$TAKO_BIN" list | jq -r '.tabs[0].panes[0].id'; }
new_pane() {
  local p
  p="$("$TAKO_BIN" split --pane "$(first_pane)" 2>/dev/null | tr -d '[:space:]')"
  case "$p" in '' | *[!0-9]*) return 1 ;; esac
  wait_prompt "$p" || return 1
  echo "$p"
}
run_in() { "$TAKO_BIN" send --pane "$1" "$2" >/dev/null; }

# cargo_cmd <workspace> <tag>: ペインで打つ cargo（取り分の CARGO_BUILD_JOBS は外す = 包まない cargo の既定）
cargo_cmd() {
  echo "cd '$1' && env -u CARGO_BUILD_JOBS CARGO_TARGET_DIR='$TMP/t-$2' '$CARGO_BIN' build --offline -q; echo \$? > '$TMP/done-$2'"
}
wait_done() {
  local tag
  for tag in "$@"; do
    wait_until 300 test -s "$TMP/done-$tag" || return 1
  done
}

# launch <名前> [VAR=VAL…]: 名前ごとに data dir と器のソケットを分けて隔離 GUI を立てる
launch() {
  local name=$1
  shift
  export TAKO_DATA_DIR="$TMP/d-$name"
  export TAKO_TMUX_SOCKET="tk1998-$$-$name"
  case " $SOCKETS " in *" $TAKO_TMUX_SOCKET "*) : ;; *) SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET" ;; esac
  mkdir -p "$TAKO_DATA_DIR"
  FIFO="$TAKO_DATA_DIR/resources/jobserver.fifo"
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1200,800}
  launch_isolated_gui "$TMP/app-$name.log" TAKO_PERSIST=1 TAKO_AUTO_RENAME=0 "$@" || exit $?
  wait_isolated_gui "$TMP/app-$name.log" || { fail "隔離 GUI（$name）に繋がらない"; exit 1; }
  wait_prompt "$(first_pane)" || { fail "最初のペインのプロンプトが出ない（$name）"; exit 1; }
}

# three <回の名前> <クレート数> <眠る ms>: 3 枚のペインで包まない cargo を並べ、区間の最大と ps の最大を返す
three() {
  local round=$1 n=$2 ms=$3 log="$TMP/logs/$1.log" panes p tag i=0
  panes="$(first_pane) $(new_pane) $(new_pane)"
  sampler_start "$TMP/logs/$round.ps"
  for p in $panes; do
    i=$((i + 1))
    tag="$round-$i"
    gen_ws "$TMP/ws-$tag" "$n" "$ms" "$log"
    run_in "$p" "$(cargo_cmd "$TMP/ws-$tag" "$tag")"
  done
  wait_done "$round-1" "$round-2" "$round-3" || fail "$round: cargo が 5 分で終わらない"
  sampler_stop
  for i in 1 2 3; do
    [ "$(cat "$TMP/done-$round-$i")" = 0 ] || fail "$round-$i: cargo が失敗した（$(cat "$TMP/done-$round-$i")）"
  done
  THREE_RUNS="$(wc -l < "$log" | tr -d ' ')"
  THREE_OVERLAP="$(overlap "$log")"
  THREE_PS="$(max_of "$TMP/logs/$round.ps")"
}

# =====================================================================
echo "== A: 札 4 枚（TAKO_1998_INJECT_TOKENS=4） =="
N=4
# 器のサーバーを注入なしで先に立てておく（本番の器は注入しない旧版の起動から何日も続いている。
# サーバーのグローバル環境に CARGO_MAKEFLAGS が無いので、`-e` で固定しないと新しいペインへ届かない）
PRE_SOCKET="tk1998-$$-a"
SOCKETS="$SOCKETS $PRE_SOCKET"
env -u CARGO_MAKEFLAGS tmux -L "$PRE_SOCKET" -f /dev/null new-session -d -s pre1998 "sleep 900"
tmux -L "$PRE_SOCKET" show-environment -g CARGO_MAKEFLAGS >/dev/null 2>&1
check $((1 - $?)) "器のサーバーは CARGO_MAKEFLAGS を持たずに立っている"
launch a TAKO_1998_INJECT_TOKENS=$N
wait_until 10 free_is $N
check $? "起動で札を $N 枚にした（free=$(js free)・state=$(js state)）"
[ "$(js path)" = "$FIFO" ] && [ -p "$FIFO" ]
check $? "fifo は <data_dir>/resources/jobserver.fifo（$(js path)）"
P1="$(first_pane)"
run_in "$P1" "printf 'CMF=[%s]\\n' \"\$CARGO_MAKEFLAGS\" > '$TMP/env-a.txt'"
wait_until 10 test -s "$TMP/env-a.txt"
grep -qxF "CMF=[-j$N --jobserver-auth=fifo:$FIFO]" "$TMP/env-a.txt"
check $? "ペインの env は CARGO_MAKEFLAGS（$(cat "$TMP/env-a.txt")）"
run_in "$P1" "printf 'MF=[%s]\\n' \"\$MAKEFLAGS\" > '$TMP/env-a-mf.txt'"
wait_until 10 test -s "$TMP/env-a-mf.txt"
grep -qxF "MF=[]" "$TMP/env-a-mf.txt"
check $? "MAKEFLAGS は配らない（$(cat "$TMP/env-a-mf.txt")）"

echo "-- ① 包まない cargo を 3 本（5 クレート × 1.5 秒） --"
three on 5 1500
echo "  ビルドスクリプト $THREE_RUNS 本・区間の最大 $THREE_OVERLAP・ps（rustc + ビルドスクリプト）の最大 $THREE_PS"
[ "$THREE_RUNS" = 15 ] && [ "$THREE_OVERLAP" -le $((N + 3)) ] && [ "$THREE_PS" -le $((N + 3)) ]
check $? "① 同時数 ≤ N + 3 = $((N + 3))（区間 $THREE_OVERLAP・ps $THREE_PS）"
ON_OVERLAP=$THREE_OVERLAP
wait_until 10 free_is $N
check $? "① 終われば札は $N 枚へ戻る（free=$(js free)）"

echo "-- ③ cargo を kill して札を持ち逃げさせる --"
gen_ws "$TMP/ws-kill" 6 2500 "$TMP/logs/kill.log"
# `a && b &` だと $! はサブシェルになるので、背景へ回すのは env（cargo を exec する）だけ
run_in "$P1" "cd '$TMP/ws-kill' && { env -u CARGO_BUILD_JOBS CARGO_TARGET_DIR='$TMP/t-kill' '$CARGO_BIN' build --offline -q & echo \$! > '$TMP/kill.pid'; }"
wait_until 60 free_below $N
check $? "③ 札が貸し出された（free=$(js free)）"
wait_until 10 free_below $((N - 1))
KPID="$(cat "$TMP/kill.pid" 2>/dev/null)"
kill -9 "$KPID" 2>/dev/null
check $? "③ cargo（pid $KPID）を kill -9"
sleep 0.5
LEAK_FREE="$(js free)"
echo "  kill 直後の空き $LEAK_FREE / $N（孤児のビルドスクリプトは眠り終えるまで札を持ちうる）"
wait_until 60 free_is $N
check $? "③ 誰も持っていなくなってから札が $N 枚へ戻る（free=$(js free)）"
RECOUNT="$(status | jq -c '.jobserver.last_recount')"
echo "  直近の数え直し: $RECOUNT"
if [ "$LEAK_FREE" -lt "$N" ]; then
  [ "$(echo "$RECOUNT" | jq -r '.why')" = idle_short ]
  check $? "③ 数え直しの記録（足した理由 idle_short）"
fi

echo "-- ④ 設定 off / on --"
"$TAKO_BIN" queue limits --jobserver off >/dev/null
wait_until 5 test ! -e "$FIFO"
check $? "④ off で fifo が置き場から消える"
[ "$(js state)" = off ] && [ "$(js code)" = setting ]
check $? "④ status は off（code=$(js code)）"
P_OFF="$(new_pane)"
run_in "$P_OFF" "printf 'CMF=[%s]\\n' \"\$CARGO_MAKEFLAGS\" > '$TMP/env-off.txt'"
wait_until 10 test -s "$TMP/env-off.txt"
grep -qxF "CMF=[]" "$TMP/env-off.txt"
check $? "④ off の直後に作ったペインには配らない（$(cat "$TMP/env-off.txt")）"
grep -q '"jobserver": false' "$TAKO_DATA_DIR/settings.json" 2>/dev/null
check $? "④ settings.json の resources.jobserver=false"
"$TAKO_BIN" queue limits --jobserver on >/dev/null
wait_until 10 free_is $N
check $? "④ on で作り直して札 $N 枚（free=$(js free)）"
grep -q '"jobserver"' "$TAKO_DATA_DIR/settings.json" 2>/dev/null
check $((1 - $?)) "④ 既定（on）へ戻すと設定ファイルからキーが消える"
P_ON="$(new_pane)"
run_in "$P_ON" "printf 'CMF=[%s]\\n' \"\$CARGO_MAKEFLAGS\" > '$TMP/env-on.txt'"
wait_until 10 test -s "$TMP/env-on.txt"
grep -qxF "CMF=[-j$N --jobserver-auth=fifo:$FIFO]" "$TMP/env-on.txt"
check $? "④ on の後に作ったペインには配る"

echo "-- ⑥ GUI を終えると fifo が消え、器のペインの cargo は従来どおり --"
stop_isolated_gui
grep -q "cargo の札の fifo: removed" "$TAKO_DATA_DIR/persist.log" 2>/dev/null
check $? "⑥ 終了処理で fifo を消した（persist.log）"
test ! -e "$FIFO"
check $? "⑥ fifo が置き場に無い"
# GUI の居ない器のペイン（env に CARGO_MAKEFLAGS が残っている = セッションに固定した値を持つもの）で cargo を走らせる
TPANE=""
for s in $(tmux -L "$TAKO_TMUX_SOCKET" list-sessions -F '#{session_name}' 2>/dev/null); do
  if tmux -L "$TAKO_TMUX_SOCKET" show-environment -t "$s" CARGO_MAKEFLAGS 2>/dev/null | grep -qF "fifo:$FIFO"; then
    TPANE="$s"
    break
  fi
done
[ -n "$TPANE" ]
check $? "⑥ 器のセッションに CARGO_MAKEFLAGS が固定されている（$TPANE）"
gen_ws "$TMP/ws-noui" 3 1500 "$TMP/logs/noui.log"
tmux -L "$TAKO_TMUX_SOCKET" send-keys -t "$TPANE" \
  "cd '$TMP/ws-noui' && printf 'CMF=[%s]\\n' \"\$CARGO_MAKEFLAGS\" > '$TMP/env-noui.txt' && CARGO_BUILD_JOBS=3 CARGO_TARGET_DIR='$TMP/t-noui' '$CARGO_BIN' build --offline -q; echo \$? > '$TMP/done-noui'" Enter
wait_done noui
grep -qF "fifo:$FIFO" "$TMP/env-noui.txt"
check $? "⑥ 器のペインの env は消えた fifo を指したまま（$(cat "$TMP/env-noui.txt")）"
[ "$(cat "$TMP/done-noui")" = 0 ] && [ "$(overlap "$TMP/logs/noui.log")" -ge 2 ]
check $? "⑥ cargo は成功し自前の並列で走る（終了 $(cat "$TMP/done-noui")・重なり $(overlap "$TMP/logs/noui.log")）"

# =====================================================================
echo "== B: A/B（TAKO_1992_NO_JOBSERVER=1） =="
launch b TAKO_1992_NO_JOBSERVER=1 TAKO_1998_INJECT_TOKENS=$N
[ "$(js state)" = off ] && [ "$(js code)" = legacy ] && [ ! -e "$FIFO" ]
check $? "A/B は fifo を作らず配らない（code=$(js code)）"
three ab 5 1500
echo "  ビルドスクリプト $THREE_RUNS 本・区間の最大 $THREE_OVERLAP・ps の最大 $THREE_PS"
[ "$THREE_OVERLAP" -gt $((N + 3)) ]
check $? "① A/B では N + 3 = $((N + 3)) を超える（区間 $THREE_OVERLAP / 札あり $ON_OVERLAP）"
stop_isolated_gui

# =====================================================================
echo "== R: 再起動をまたぐ（札 2 枚） =="
N=2
launch r TAKO_1998_INJECT_TOKENS=$N
wait_until 10 free_is $N
PR="$(first_pane)"
gen_ws "$TMP/ws-r" 10 2500 "$TMP/logs/restart.log"
run_in "$PR" "$(cargo_cmd "$TMP/ws-r" r)"
wait_until 60 free_below $N
check $? "② 札が貸し出された（free=$(js free)）"
stop_isolated_gui
grep -q "cargo の札の fifo: kept" "$TAKO_DATA_DIR/persist.log" 2>/dev/null && [ -p "$FIFO" ]
check $? "② cargo が札を持っている間の終了は fifo を残す"
launch r TAKO_1998_INJECT_TOKENS=$N
: > "$TMP/logs/restart.free"
while [ ! -s "$TMP/done-r" ]; do
  js free >> "$TMP/logs/restart.free"
  sleep 0.3
done
wait_until 15 free_is $N
check $? "② 終われば札は $N 枚（free=$(js free)）"
js free >> "$TMP/logs/restart.free"
RMAX="$(grep -E '^[0-9]+$' "$TMP/logs/restart.free" | sort -n | tail -1)"
echo "  再起動後の空きの最大 $RMAX（$(grep -cE '^[0-9]+$' "$TMP/logs/restart.free") 回）・区間の最大 $(overlap "$TMP/logs/restart.log")"
[ "$(cat "$TMP/done-r")" = 0 ] && [ -n "$RMAX" ] && [ "$RMAX" -le "$N" ]
check $? "② 再起動をまたいでも札は N = $N を超えない（空きの最大 $RMAX）"
[ "$(overlap "$TMP/logs/restart.log")" -le $((N + 1)) ]
check $? "② 1 本の cargo の同時数 ≤ N + 1 = $((N + 1))"
stop_isolated_gui

# =====================================================================
echo "== N1: 札 1 枚 =="
N=1
launch n1 TAKO_1998_INJECT_TOKENS=$N
wait_until 10 free_is $N
check $? "⑤ 札 1 枚（free=$(js free)）"
three n1 4 1200
echo "  ビルドスクリプト $THREE_RUNS 本・区間の最大 $THREE_OVERLAP・ps の最大 $THREE_PS"
[ "$THREE_RUNS" = 12 ] && [ "$THREE_OVERLAP" -le $((N + 3)) ] && [ "$THREE_PS" -le $((N + 3)) ]
check $? "⑤ N = 1 の機でも同時数 ≤ N + 3 = $((N + 3))"
stop_isolated_gui

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
