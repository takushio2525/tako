#!/usr/bin/env bash
# test-restore-rescue-641.sh — 復元の直後に全ペインが PTY 死亡しても終了しないことの実経路テスト（#641）
#
# 隔離した data / HOME / tmux で**実 tako-app** を立て、layout を作って一度終了 → 立て直して復元させ、
# **復元の直後に自分の隔離ソケットの tmux サーバーを落とす**（注入）。器が消えるので attach クライアントが
# 一斉に終わり、全ペインが PTY 死亡で閉じる（Windows の実機で起きた「復元の直後の全滅」と同じ形）。
#
#   ① legacy（`TAKO_641_LEGACY=1` = 修正前）: 全滅で GUI が終了する（「全ペイン終了（PTY 死亡）」）
#   ② fixed: 終了せず、器を使わない新しいシェルのタブ（「復元できず」）で続く。persist.log に
#      捨てた器と理由の 1 行・layout.json.rescued に復元の中身・restore-guard.json に連続 1 回
#   ③ fixed の続き: もう一度同じ全滅 → 連続 2 回 → 次の起動は復元を見送る（layout.json を
#      layout.json.rescued へ移す・記録を消す）。見送った起動も生きている
#   ④ 普通の終了は変わらない: 入力（`tako send exit`）の後の全滅は終了する
#   ⑤ 窓（30 秒）を過ぎた全滅は終了する
#   ⑥ 一部のペインだけ死んでも何もしない（残りのタブで続く・救済の行も記録も無い）
#   ⑦ 明示 close（CLI で 1 枚を閉じた = 人 / AI の操作）の後の全滅は従来どおり終了する
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は回ごとの専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# CI には登録しない（実 GUI を立てる）。CI 側の担保は `tako_control::restore_guard` の単体テスト。
#
# 使い方: bash scripts/test-restore-rescue-641.sh                 # 全部（約 3 分）
#         PARTS="legacy fixed" bash scripts/test-restore-rescue-641.sh   # 選んで回す
#         （legacy / fixed / input / late / partial / explicit）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TMUX
unset CLAUDE_CONFIG_DIR TAKO_641_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARTS="${PARTS:-legacy fixed input late partial explicit}"
# 全滅を待つ上限（秒）
DEATH_WAIT="${DEATH_WAIT:-20}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else fail "$2"; fi; }

for bin in tmux zsh; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# HOME を差し替える**前**にバイナリを決める（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

ROOT="$(mktemp -d /tmp/t641-XXXXXX)"
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local s
  for s in $SOCKETS; do
    tmux_socket_kill "$s"
  done
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した: $ROOT"
  else
    rm -rf "${ROOT:?}"
  fi
}
trap cleanup EXIT

# 回ごとの置き場（data / discovery / tmux を回で分ける = 前の回の器を拾わない）
#   new_round <名前>
new_round() {
  R="$ROOT/$1"
  mkdir -p "$R/home" "$R/zdot"
  export HOME="$R/home"
  export ZDOTDIR="$R/zdot"
  export TAKO_DATA_DIR="$R/data"
  export TAKO_DISCOVERY_DIR="$R/disc"
  export TAKO_ORCHESTRATOR_DIR="$R/orch"
  export TAKO_SESSIONS_FILE="$R/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$R/panelogs"
  export TAKO_WORKERS_FILE="$R/workers.yaml"
  export TAKO_ISOLATED=1
  SOCK="t641-$1-$$"
  SOCKETS="$SOCKETS $SOCK"
  export TAKO_TMUX_SOCKET="$SOCK"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  local d
  for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$ROOT"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done
  # 固定のプロンプト（実ユーザー名・ホスト名を画面へ出さない = #927）
  printf '%s\n' 'PS1="[test@host]$ "' > "$R/zdot/.zshrc"
  LOG="$TAKO_DATA_DIR/persist.log"
  GEN=0
}

#   start_gui [VAR=VAL …]（回の中で何度目の起動かを GEN で数える）
start_gui() {
  GEN=$((GEN + 1))
  APPLOG="$R/app-$GEN.log"
  launch_isolated_gui "$APPLOG" "TAKO_PERSIST=1" "SHELL=/bin/zsh" "ZDOTDIR=$ZDOTDIR" \
    ${@+"$@"} || exit $?
  GUI_PID="$ISOLATED_GUI_PID"
  if ! wait_isolated_gui "$APPLOG"; then
    fail "隔離 GUI が起動しない（${R##*/} の ${GEN} 回目）"
    exit 1
  fi
}
stop_gui() {
  stop_isolated_gui "$GUI_PID"
  ISOLATED_GUI_PID=""
  sleep 1
}
# 終了するはずの GUI を後片付けの対象から外す。**終わっていなければ外さない**（外すと後片付けが
# 器のサーバーだけを落とし、生き残った GUI が救済して居残る）
forget_if_exited() {
  if kill -0 "$GUI_PID" 2>/dev/null; then
    stop_gui
  else
    ISOLATED_GUI_PID=""
  fi
}

alive() { kill -0 "$GUI_PID" 2>/dev/null && echo 1 || echo 0; }
sessions() { tmux -L "$SOCK" list-sessions -F '#{session_name}' 2>/dev/null | grep -c . | tr -d ' '; }
# この起動の pid の行だけ数える（persist_diag は末尾に [pid N] を付ける）。
# pipefail 下で grep の 1（一致なし）を `|| echo 0` で拾うと「0」が 2 行になるので wc で数える
log_count() {
  [ -f "$LOG" ] || { echo 0; return; }
  local n
  n=$(grep -F "$1" "$LOG" | grep -F "[pid $GUI_PID]" | wc -l | tr -d ' ')
  echo "${n:-0}"
}
# この起動の pid の行が 1 行以上あれば 1
has_log() { [ "$(log_count "$1")" -ge 1 ] && echo 1 || echo 0; }

# 器のセッションが n 本になるまで待つ（最大 10 秒）
wait_sessions() {
  local n="$1" i
  for i in $(seq 1 50); do
    [ "$(sessions)" -ge "$n" ] && return 0
    sleep 0.2
  done
  return 1
}

# GUI が終わる（=1）か、persist.log に pat の行が出る（=2）まで待つ。どちらも無ければ 0
wait_outcome() {
  local pat="$1" i
  for i in $(seq 1 $((DEATH_WAIT * 5))); do
    [ "$(alive)" = 0 ] && { echo 1; return; }
    [ "$(log_count "$pat")" -ge 1 ] && { echo 2; return; }
    sleep 0.2
  done
  echo 0
}

# 回の最初: 器のペインを 2 つ（タブ 2 枚）作り、SIGTERM で終える（layout.json を保存して器は残る）
#   seed [タブの枚数]
seed() {
  local tabs="${1:-2}" i
  start_gui
  i=1
  while [ "$i" -lt "$tabs" ]; do
    "$TAKO_BIN" tab new --title "t$i" >/dev/null 2>&1
    i=$((i + 1))
  done
  wait_sessions "$tabs" || fail "器のセッションが ${tabs} 本立たない（実際 $(sessions)）"
  sleep 2
  stop_gui
  local n
  n=$(grep -o '"session": *"[^"]*"' "$TAKO_DATA_DIR/layout.json" 2>/dev/null | grep -c . | tr -d ' ')
  check "$([ "$n" = "$tabs" ] && echo 1 || echo 0)" "種: layout.json に器のペイン ${tabs} 枚（実際 ${n}）"
}

# 復元の直後の全滅を注入する（自分の隔離ソケットのサーバーだけを落とす）
inject_all_exit() {
  tmux_socket_kill "$SOCK"
}

# ペインの ID（タブの `id` と混ざらないよう JSON を読む。perl の JSON::PP は macOS 同梱）
pane_ids() {
  "$TAKO_BIN" list 2>/dev/null |
    perl -MJSON::PP -0777 -ne 'my $d = decode_json($_); for my $t (@{$d->{tabs}}) { print "$_->{id}\n" for @{$t->{panes}} }'
}

# PARTS にその節があるか（pipefail の下で `echo | grep -q` を使わない = #1633 / #2272）
want() { [[ " $PARTS " == *" $1 "* ]]; }

if want legacy; then
  echo "== ① legacy（TAKO_641_LEGACY=1 = 修正前）: 復元の直後の全滅で GUI が終了する =="
  new_round legacy
  seed 2
  start_gui TAKO_641_LEGACY=1
  check "$(has_log '復元成功')" "復元した（復元成功の行）"
  inject_all_exit
  out=$(wait_outcome '復元の直後の全滅（#641）')
  check "$([ "$out" = 1 ] && echo 1 || echo 0)" "修正前は全滅で GUI が終了する（結果 ${out}: 1=終了 / 2=救済 / 0=どちらでもない）"
  check "$(has_log '全ペイン終了（PTY 死亡）')" "persist.log に「全ペイン終了（PTY 死亡）」の行"
  check "$([ ! -f "$TAKO_DATA_DIR/restore-guard.json" ] && echo 1 || echo 0)" "legacy は記録を書かない"
  forget_if_exited
fi

if want fixed; then
  echo "== ② fixed: 復元の直後の全滅で終了せず、新しいシェルのタブで続く =="
  new_round fixed
  seed 2
  start_gui
  inject_all_exit
  out=$(wait_outcome '復元の直後の全滅（#641）')
  check "$([ "$out" = 2 ] && echo 1 || echo 0)" "修正後は終了せず救済する（結果 ${out}: 1=終了 / 2=救済 / 0=どちらでもない）"
  sleep 3
  check "$(alive)" "救済の 3 秒後も GUI は生きている"
  grep -F '復元の直後の全滅（#641）' "$LOG" | sed "s#$ROOT#<tmp>#g; s/^/      /"
  check "$(has_log '捨てた器 2:')" "救済の行に捨てた器 2 本の名前"
  check "$([ "$(log_count '全ペイン終了（PTY 死亡）')" = 0 ] && echo 1 || echo 0)" "「全ペイン終了」の行は出ない"
  check "$("$TAKO_BIN" list 2>/dev/null | grep -c '復元できず' | awk '{print ($1>=1)}')" "タブ「復元できず」がある"
  n=$(grep -o '"session": *"[^"]*"' "$TAKO_DATA_DIR/layout.json.rescued" 2>/dev/null | grep -c . | tr -d ' ')
  check "$([ "$n" = 2 ] && echo 1 || echo 0)" "layout.json.rescued に復元に使った器のペイン 2 枚（実際 ${n}）"
  check "$(grep -c '"count": 1' "$TAKO_DATA_DIR/restore-guard.json" 2>/dev/null | awk '{print ($1>=1)}')" "restore-guard.json は連続 1 回"
  check "$([ "$(sessions)" = 0 ] && echo 1 || echo 0)" "救済のシェルは器を使わない（隔離ソケットにサーバーが立たない）"
  "$TAKO_BIN" persist 2>/dev/null > "$R/persist.json"
  check "$(grep -c '"rescued": *true' "$R/persist.json" | awk '{print ($1>=1)}')" "tako persist の restore_guard.rescued = true"
  # 救済したタブで打てる（器なしのシェルが生きている）
  pid1=$(pane_ids | sed -n '1p')
  "$TAKO_BIN" send --pane "$pid1" "echo RESCUE-SHELL-OK" >/dev/null 2>&1
  sleep 1
  check "$("$TAKO_BIN" read --pane "$pid1" 2>/dev/null | grep -c 'RESCUE-SHELL-OK' | awk '{print ($1>=1)}')" "救済のシェルへ送って画面に出る"
  stop_gui

  echo "== ③ 同じ全滅がもう一度 → 連続 2 回 → 次の起動は復元を見送る =="
  start_gui
  inject_all_exit
  out=$(wait_outcome '復元の直後の全滅（#641）')
  check "$([ "$out" = 2 ] && echo 1 || echo 0)" "2 回目も救済する（結果 ${out}）"
  check "$(has_log '連続 2 回目 = 次の起動は復元を見送る')" "救済の行に「連続 2 回目 = 次の起動は復元を見送る」"
  check "$(grep -c '"count": 2' "$TAKO_DATA_DIR/restore-guard.json" 2>/dev/null | awk '{print ($1>=1)}')" "restore-guard.json は連続 2 回"
  sleep 1
  stop_gui
  command cp -f "$TAKO_DATA_DIR/layout.json" "$R/layout.before-skip.json"
  start_gui
  check "$(has_log '復元見送り（#641）')" "3 回目の起動は復元を見送る（persist.log）"
  grep -F '復元見送り（#641）' "$LOG" | sed "s#$ROOT#<tmp>#g; s/^/      /"
  check "$([ "$(log_count '復元成功')" = 0 ] && echo 1 || echo 0)" "見送った起動に「復元成功」の行は無い"
  check "$(cmp -s "$R/layout.before-skip.json" "$TAKO_DATA_DIR/layout.json.rescued" && echo 1 || echo 0)" "見送る前の layout.json が layout.json.rescued にバイト一致で残る"
  check "$([ -f "$TAKO_DATA_DIR/layout.json.rescued.bak.1" ] && echo 1 || echo 0)" "前の退避は layout.json.rescued.bak.1 へ回る"
  check "$([ ! -f "$TAKO_DATA_DIR/restore-guard.json" ] && echo 1 || echo 0)" "見送りは 1 回だけ（記録を消した）"
  sleep 3
  check "$(alive)" "見送った起動も生きている"
  "$TAKO_BIN" recover > "$R/recover.txt" 2>&1
  check "$(grep -c 'layout.json.rescued : 1 タブ' "$R/recover.txt" | awk '{print ($1>=1)}')" "tako recover の一覧に layout.json.rescued"
  sed "s#$ROOT#<tmp>#g; s/^/      /" "$R/recover.txt" | grep -F 'rescued'
  stop_gui
fi

if want input; then
  echo "== ④ 入力の後の全滅（exit で最後のシェルを閉じた）は従来どおり終了する =="
  new_round input
  seed 1
  start_gui
  p=$(pane_ids | sed -n '1p')
  "$TAKO_BIN" send --pane "$p" "exit" >/dev/null 2>&1
  out=$(wait_outcome '復元の直後の全滅（#641）')
  check "$([ "$out" = 1 ] && echo 1 || echo 0)" "exit の後は終了する（結果 ${out}）"
  check "$(has_log '起動の後に入力があったので普通の終了として扱う')" "「全ペイン終了」の行に入力ありの注記"
  check "$([ ! -f "$TAKO_DATA_DIR/restore-guard.json" ] && echo 1 || echo 0)" "記録は無い"
  forget_if_exited
fi

if want late; then
  echo "== ⑤ 窓（30 秒）を過ぎてからの全滅は従来どおり終了する =="
  new_round late
  seed 2
  start_gui
  sleep 33
  inject_all_exit
  out=$(wait_outcome '復元の直後の全滅（#641）')
  check "$([ "$out" = 1 ] && echo 1 || echo 0)" "窓を過ぎた全滅は終了する（結果 ${out}）"
  check "$(has_log '復元の直後（30 秒）を過ぎていた')" "「全ペイン終了」の行に窓を過ぎた注記"
  forget_if_exited
fi

if want partial; then
  echo "== ⑥ 一部のペインだけ死んでも何もしない =="
  new_round partial
  seed 2
  start_gui
  victim=$(tmux -L "$SOCK" list-sessions -F '#{session_name}' 2>/dev/null | sed -n '1p')
  tmux -L "$SOCK" kill-session -t "=$victim" 2>/dev/null
  sleep 6
  check "$(alive)" "GUI は生きている"
  check "$([ "$(log_count '復元の直後の全滅（#641）')" = 0 ] && echo 1 || echo 0)" "救済の行は出ない"
  check "$([ "$(sessions)" = 1 ] && echo 1 || echo 0)" "残りの器は 1 本のまま（実際 $(sessions)）"
  check "$([ ! -f "$TAKO_DATA_DIR/restore-guard.json" ] && echo 1 || echo 0)" "記録は無い"
  stop_gui
fi

if want explicit; then
  # 最後のタブの最後のペインの close は CLI / MCP からは断られる（dispatch の LastPane。アプリ終了に
  # 等しい操作は UI の ⌘W だけ）ので、ここは「CLI で 1 枚を明示 close した後の全滅」を見る
  echo "== ⑦ 明示 close（人 / AI の操作）の後の全滅は従来どおり終了する =="
  new_round explicit
  seed 2
  start_gui
  p=$(pane_ids | tail -1)
  "$TAKO_BIN" close --pane "$p" --force >/dev/null 2>&1
  check "$([ "$(pane_ids | grep -c .)" = 1 ] && echo 1 || echo 0)" "CLI の明示 close でタブが 1 枚になった"
  inject_all_exit
  out=$(wait_outcome '復元の直後の全滅（#641）')
  check "$([ "$out" = 1 ] && echo 1 || echo 0)" "明示 close の後の全滅は終了する（結果 ${out}）"
  check "$(has_log '起動の後に入力があったので普通の終了として扱う')" "「全ペイン終了」の行に入力ありの注記"
  check "$([ ! -f "$TAKO_DATA_DIR/restore-guard.json" ] && echo 1 || echo 0)" "記録は無い"
  forget_if_exited
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
