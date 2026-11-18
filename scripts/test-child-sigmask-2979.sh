#!/usr/bin/env bash
# test-child-sigmask-2979.sh — #2979: GUI の背景の段から起こした子にも打ち切りの SIGTERM が届くことの実経路テスト
#
# GUI の背景の段（GPUI の macOS の dispatcher = GCD のスレッド）は TERM / INT / HUP ほかをブロックしていて、
# 子のシグナルマスクは起こしたスレッドから受け継がれる。修正前は背景の段から起こした子（リモートフォルダの
# 見張りの生死の判定 = `ssh -O check`。#2976 で背景へ出した）が打ち切りの SIGTERM（#2633）を受けられず、
# 猶予（1 秒）の後の SIGKILL で trap なしに消えていた。修正後は待つ子を起こす 1 実装 `probe::spawn_for_wait`
# が既定のマスク（空）で起こす。番号は Issue #2979 の受け入れ条件:
#   1. 背景の段から起こした返らない `ssh -O check` の trap に、上限（`TAKO_REMOTE_TIMEOUT_SECS`）の打ち切りで
#      TERM が届く（子と孫の sleep は上限の直後に消える）。修正前（`TAKO_2979_LEGACY=1`）は届かず、子と孫は
#      猶予の後の SIGKILL で消える
#   2. UI スレッドから起こす子（#2976 の修正前の経路 `TAKO_2976_LEGACY=1`）は、修正前後とも trap に TERM が届く
#
# **本物の ssh の接続先・tailscale・本番の tako には触らない**: ssh / sftp はシェルの偽物で、偽のログイン
# シェルが `command -v ssh` / `sftp` に偽物を答える（どこへも繋がない）。data / discovery / HOME は mktemp 配下。
# GUI と CLI は砂場（一時 HOME・偽物だけが居る PATH）で動かす。落とすのは自分で起こした pid だけ。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=bg|bg-legacy|ui|ui-legacy（既定は全部）。
# CI には登録しない（GUI が要る）。CI 側の担保は境界の単体テスト（`platform::process::tests::issue2979_*`）と
# 番犬 `crates/tako-control/tests/issue2979_child_sigmask_watchdog.rs`（GCD と同じ組をブロックしたスレッドから
# #2633 の rehash の打ち切りを走らせる実プロセスの節を含む）。
#
# 使い方: bash scripts/test-child-sigmask-2979.sh
set -uo pipefail
# 判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2976_LEGACY TAKO_2979_LEGACY TAKO_2633_LEGACY \
  TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR TAKO_TMUX_BIN TAKO_TMUX_SOCKET \
  TAKO_SETUP_PROBE_TIMEOUT_SECS TAKO_REMOTE_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
# `ssh -O check` の上限の注入（秒）
LIMIT=3
# 打ち切りの TERM から SIGKILL までの猶予（`probe::GROUP_TERM_GRACE` = 1 秒）
GRACE_MS=1000

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

TMP="$(mktemp -d /tmp/tako-2979-XXXXXX)"
APP_PID=""
MCP_PID=""
FAKE_DIR="$TMP/fake"
FOLDER_HOST="fakehost2979"

# 偽物を落とす（止まる前に自分で書いた pid だけ）
kill_fakes() {
  local f
  for f in "$FAKE_DIR/pids"/*; do
    [ -f "$f" ] || continue
    kill -KILL "${f##*/}" >/dev/null 2>&1 || true
    rm -f "$f"
  done
}
cleanup() {
  rm -f "${FAKE_DIR:?}"/hang-*
  kill_fakes
  [ -n "$MCP_PID" ] && kill -KILL "$MCP_PID" >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "背景の段から起こした子のシグナルマスクの実経路テスト（#2979）"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の ssh / sftp / ログインシェル -------------------------------------------------
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
# 偽の ssh。止まる印があれば `-O check` の始まり・孫（sleep）の pid・打ち切りの TERM を checks.log へ記録し、
# 孫を抱えて止まる。ControlMaster の `-O check` / `-O exit` と張る形（`-N -f`）には ControlPath のファイルで
# 答え、それ以外はどこへも繋がない
cat > "$TMP/stubs/ssh" <<SH
#!/bin/sh
stamp() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%d", time * 1000'; }
cp=""; op=""; master=0; prev=""
for a in "\$@"; do
  case "\$a" in
    ControlPath=*) cp="\${a#ControlPath=}"; cp="\${cp#\\"}"; cp="\${cp%\\"}" ;;
    -N) master=1 ;;
  esac
  [ "\$prev" = "-O" ] && op="\$a"
  prev="\$a"
done
case "\$op" in
  check)
    if [ -e "\$FAKE_DIR/hang-ssh-check" ]; then
      echo "start \$\$ \$(stamp)" >> "\$FAKE_DIR/checks.log"
      trap 'echo "term \$\$ \$(stamp)" >> "\$FAKE_DIR/checks.log"; exit 255' TERM INT HUP
      /bin/sleep 600 &
      echo "grandchild \$\$ \$!" >> "\$FAKE_DIR/checks.log"
      : > "\$FAKE_DIR/pids/\$!"; : > "\$FAKE_DIR/pids/\$\$"
      wait
      exit 255
    fi
    echo "quick \$\$ \$(stamp) \$cp" >> "\$FAKE_DIR/checks.log"
    [ -n "\$cp" ] && [ -e "\$cp" ] && exit 0
    exit 255 ;;
  exit) [ -n "\$cp" ] && rm -f "\$cp"; exit 0 ;;
esac
if [ "\$master" = 1 ]; then
  [ -n "\$cp" ] && : > "\$cp"
  exit 0
fi
echo "fake ssh (#2979): \$*"
exit 255
SH
# 偽の sftp（バッチの `pwd` と `ls -la` に決まった形で答える。どこへも繋がない）
cat > "$TMP/stubs/sftp" <<'SH'
#!/bin/sh
while IFS= read -r line; do
  echo "sftp> $line"
  case "$line" in
    pwd) echo "Remote working directory: /home/testuser" ;;
    "ls -la"*)
      echo 'drwxr-xr-x    3 fake     staff          96 Nov 17 10:00 .'
      echo 'drwxr-xr-x    3 fake     staff          96 Nov 17 10:00 ..'
      echo '-rw-r--r--    1 fake     staff          12 Nov 17 10:00 hello.txt' ;;
  esac
done
exit 0
SH
# 偽のログインシェル: `command -v ssh` / `sftp` には偽物を答える。それ以外は /bin/sh
cat > "$TMP/stubs/isosh" <<SH
#!/bin/sh
if [ "\$1" = "-l" ]; then shift; fi
case "\$*" in
  *"command -v ssh"*) echo "$TMP/stubs/ssh"; exit 0 ;;
  *"command -v sftp"*) echo "$TMP/stubs/sftp"; exit 0 ;;
esac
exec /bin/sh "\$@"
SH
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/ssh" "$TMP/stubs/sftp" "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "TAKO_ISOLATED=1" "PATH=$SANDBOX_PATH" \
    "FAKE_DIR=$FAKE_DIR" "TAKO_REMOTE_TIMEOUT_SECS=$LIMIT"
}
# 砂場で本物の claude / codex / agy を起こさない（PATH に居ない）こと
for name in claude codex agy; do
  resolved="$(env -i PATH="$SANDBOX_PATH" /bin/sh -c "command -v $name" 2>/dev/null)"
  if [ -z "$resolved" ]; then
    pass "砂場の PATH に $name が居ない（本物を起こさない）"
  else
    fail "砂場の PATH に $name が居る（${resolved}）"
    exit 1
  fi
done
for name in ssh sftp; do
  resolved="$(env -i PATH="$SANDBOX_PATH" FAKE_DIR="$FAKE_DIR" "$TMP/stubs/isosh" -l -c "command -v $name")"
  check_eq "砂場のログインシェルの $name は偽物" "$TMP/stubs/$name" "$resolved"
done

# 隔離の data / discovery（段ごとに分ける = 前の段の器を拾わない。#2157）
use_dirs() {
  export TAKO_DATA_DIR="$TMP/$1/data"
  export TAKO_DISCOVERY_DIR="$TMP/$1/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$1/orch"
  export TAKO_SESSIONS_FILE="$TMP/$1/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$1/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$1/workers.yaml"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
}
gui_socket() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  echo "$sock"
}

# MCP の tools/call を起こして LIMIT 秒まで待つ。本文は $TMP/out/<名前>.out
#   mcp_call <ツール> <引数の JSON> <名前> <LIMIT 秒>
mcp_call() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2979","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-$3.jsonl"
  local t0 limit="$4"
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-$3.jsonl" > "$TMP/out/$3-raw.out" 2> "$TMP/out/$3.err" &
  MCP_PID=$!
  while kill -0 "$MCP_PID" 2>/dev/null; do
    if [ $(($(now_ms) - t0)) -ge $((limit * 1000)) ]; then
      kill -KILL "$MCP_PID" 2>/dev/null
      break
    fi
    sleep 0.05
  done
  wait "$MCP_PID" 2>/dev/null
  MCP_PID=""
  perl -MJSON::PP -ne '
    my $m = eval { JSON::PP->new->utf8->decode($_) } or next;
    next unless defined $m->{id} && $m->{id} == 2;
    binmode STDOUT, ":utf8";
    for my $c (@{ $m->{result}{content} || [] }) { print $c->{text}, "\n" if $c->{type} eq "text" }
    last;' "$TMP/out/$3-raw.out" > "$TMP/out/$3.out"
}

# JSON の欄（`a/b/0/c`）。無ければ absent・null は null・真偽は小文字
jf() {
  perl -MJSON::PP -e '
    local $/; my $t = <STDIN>;
    my $v = eval { JSON::PP->new->utf8->decode($t) };
    defined $v or do { print "absent\n"; exit };
    for my $k (grep { length } split m{/}, $ARGV[0]) {
      if (ref $v eq "HASH") { exists $v->{$k} or do { print "absent\n"; exit }; $v = $v->{$k} }
      elsif (ref $v eq "ARRAY" && $k =~ /^\d+$/) { $v = $v->[$k] }
      else { $v = undef }
      last unless defined $v;
    }
    binmode STDOUT, ":utf8";
    if (!defined $v) { print "null\n" }
    elsif (JSON::PP::is_bool($v)) { print(($v ? "true" : "false"), "\n") }
    elsif (ref $v) { print JSON::PP->new->canonical->encode($v), "\n" }
    else { print "$v\n" }' "$1"
}

start_app() {
  local name="$1"
  shift
  use_dirs "$name"
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) ${@+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    unmeasured "GUI を立てられない（終了コード ${rc}。tako-vd が無い / 蓋が閉じている）: 未実測"
    return 1
  fi
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || return 1
}
stop_app() {
  rm -f "${FAKE_DIR:?}"/hang-*
  kill_fakes
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# 条件が立つまで待つ（0.1 秒刻み・上限 秒）。  wait_until <上限秒> <コマンド…>
wait_until() {
  local limit="$1" i
  shift
  for i in $(seq 1 $((limit * 10))); do
    "$@" && return 0
    sleep 0.1
  done
  return 1
}
checks_count() {
  local n
  n="$(grep -c "^$1 " "$FAKE_DIR/checks.log" 2>/dev/null)"
  echo "${n:-0}"
}
quick_seen() { [ "$(checks_count quick)" -ge "$1" ]; }
grandchild_seen() { [ "$(checks_count grandchild)" -ge 1 ]; }

# 1 段。  phase <名前> <TERM が届くか（yes / no）> [env…]
phase() {
  local name="$1" expect="$2"
  shift 2
  echo "== ${name}（${*:-既定}）"
  rm -f "${FAKE_DIR:?}/checks.log"
  start_app "$name" ${@+"$@"} || return
  # 返る偽物でフォルダを開く（ControlPath のファイルが立つ = 見張りの判定は Live）
  mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" "$name-open" 30
  check_eq "$name: 返る偽物でフォルダを開いた（opened）" "true" "$(jf opened < "$TMP/out/$name-open.out")"
  local base
  base="$(checks_count quick)"
  if ! wait_until 20 quick_seen $((base + 2)); then
    fail "$name: 20 秒待っても見張りが ssh -O check を起こさない"
    stop_app
    return
  fi

  # 返らない `ssh -O check` を 1 本起こさせ、打ち切りの前後を状態で見る
  : > "$FAKE_DIR/hang-ssh-check"
  if ! wait_until 10 grandchild_seen; then
    fail "$name: 10 秒待っても見張りが返らない ssh -O check を起こさない"
    stop_app
    return
  fi
  # 2 本目を起こさせない（最初の 1 本だけを見る）
  rm -f "${FAKE_DIR:?}/hang-ssh-check"
  local child grandchild t0 t_child="" t_grand="" deadline now
  read -r _ child t0 < <(grep -m1 "^start " "$FAKE_DIR/checks.log")
  read -r _ _ grandchild < <(grep -m1 "^grandchild " "$FAKE_DIR/checks.log")
  # 子（sh）と孫（sleep）が消えた時刻を 50ms 刻みで拾う（上限 + 猶予 + 5 秒まで）
  deadline=$((t0 + LIMIT * 1000 + GRACE_MS + 5000))
  while :; do
    now="$(now_ms)"
    [ -z "$t_child" ] && ! kill -0 "$child" 2>/dev/null && t_child="$now"
    [ -z "$t_grand" ] && ! kill -0 "$grandchild" 2>/dev/null && t_grand="$now"
    { [ -n "$t_child" ] && [ -n "$t_grand" ]; } && break
    [ "$now" -ge "$deadline" ] && break
    sleep 0.05
  done
  local t_term
  t_term="$(awk -v p="$child" '$1 == "term" && $2 == p { print $3; exit }' "$FAKE_DIR/checks.log")"
  command cp -f "$FAKE_DIR/checks.log" "$TMP/out/$name-checks.log"
  local term_desc child_desc grand_desc
  if [ -n "$t_term" ]; then term_desc="届いた（始まりから $((t_term - t0)) ms）"; else term_desc="届かない"; fi
  if [ -n "$t_child" ]; then child_desc="$((t_child - t0)) ms"; else child_desc="消えない"; fi
  if [ -n "$t_grand" ]; then grand_desc="$((t_grand - t0)) ms"; else grand_desc="消えない"; fi
  echo "    打ち切りの TERM の trap: ${term_desc}・子（sh）が消えた: ${child_desc}・孫（sleep）が消えた: ${grand_desc}（上限 ${LIMIT} 秒・猶予 ${GRACE_MS} ms）"
  # 段の結果（yes = TERM が届いた / no = 届かない。空 = この段は測れていない）
  if [ -n "$t_term" ]; then eval "TERM_${name//-/_}=yes"; else eval "TERM_${name//-/_}=no"; fi
  # 消えた時刻の判定: TERM が届けば上限 + 猶予の手前で消え、届かなければ猶予の後（SIGKILL）
  local cut=$((LIMIT * 1000 + GRACE_MS - 300))
  case "$expect" in
    yes)
      if [ -n "$t_term" ]; then pass "$name: 打ち切りの TERM が子の trap に届く"; else fail "$name: 打ち切りの TERM が子の trap に届かない"; fi
      if [ -n "$t_child" ] && [ -n "$t_grand" ] && [ $((t_child - t0)) -lt "$cut" ] && [ $((t_grand - t0)) -lt "$cut" ]; then
        pass "$name: 子と孫は猶予の前に TERM で消える（${child_desc} / ${grand_desc}）"
      else
        fail "$name: 子と孫が猶予の前に消えない（${child_desc} / ${grand_desc}）"
      fi
      ;;
    no)
      if [ -z "$t_term" ]; then pass "$name: A/B: 打ち切りの TERM は子の trap に届かない（修正前の再現）"; else fail "$name: A/B: TERM が届いた（修正前を再現できていない）"; fi
      if [ -n "$t_child" ] && [ -n "$t_grand" ] && [ $((t_child - t0)) -ge "$cut" ] && [ $((t_grand - t0)) -ge "$cut" ]; then
        pass "$name: A/B: 子と孫は猶予の後の SIGKILL で消える（${child_desc} / ${grand_desc}）"
      else
        fail "$name: A/B: 子と孫の消え方が猶予の後の SIGKILL の形でない（${child_desc} / ${grand_desc}）"
      fi
      ;;
  esac
  stop_app
}

TERM_ui=""
TERM_ui_legacy=""
want bg && phase bg yes
want bg-legacy && phase bg-legacy no TAKO_2979_LEGACY=1
want ui && phase ui yes TAKO_2976_LEGACY=1
want ui-legacy && phase ui-legacy yes TAKO_2976_LEGACY=1 TAKO_2979_LEGACY=1

# 2. UI スレッドから起こす子は修正前後で同じ（どちらも TERM が届く）
if [ -n "$TERM_ui" ] && [ -n "$TERM_ui_legacy" ]; then
  if [ "$TERM_ui" = "$TERM_ui_legacy" ]; then
    pass "UI スレッドから起こす子は修正前後で同じ（TERM が届いたか: 修正後 ${TERM_ui} / 修正前 ${TERM_ui_legacy}）"
  else
    fail "UI スレッドから起こす子の振る舞いが修正前後で違う（修正後 ${TERM_ui} / 修正前 ${TERM_ui_legacy}）"
  fi
elif [ -z "$ONLY" ]; then
  unmeasured "UI スレッドから起こす子の修正前後の突き合わせ（片方の段が回っていない）"
fi

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
