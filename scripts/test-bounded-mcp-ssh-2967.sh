#!/usr/bin/env bash
# test-bounded-mcp-ssh-2967.sh — #2967 / #2973: MCP の登録とリモートフォルダが起こす外部コマンドが上限で返ることの実経路テスト
#
# 返らない偽物（claude の `mcp remove` / `mcp add`・codex の `mcp list` / `mcp add`・agy の `mcp add`・
# ssh の張る形 / `-O check` / `-O exit`・sftp のバッチ）を置き、MCP と CLI が上限で返るかを A/B で見る
# （番号は Issue #2967 / #2973 の受け入れ条件）:
#   1. #2967: 返らない claude / codex / agy で MCP `tako_setup_mcp` と CLI `tako setup-mcp` が上限で返り、日英の
#      「確かめられなかった」と次の一手が出る。偽物の子と孫が残らない。錠が解けて後続の setup の書き込みが進む。
#      返る偽物では A/B で応答がバイト一致
#   2. #2973: 返らない ssh / sftp で `tako_remote_folder` の各 action が上限で返り、「応答なし」の理由と次の一手が出る。
#      偽物が残らない。返る偽物では A/B で一致
#   修正前（`TAKO_2967_LEGACY=1` / `TAKO_2973_LEGACY=1`）は返らない偽物で MCP / CLI が返らない（こちらで落とす）
#
# **本物の claude / codex / agy・ssh の接続先には触らない**: 偽の claude は `cc` で作り、codex / agy / ssh / sftp は
# シェルの偽物。ssh / sftp は偽のログインシェルが `command -v` に答える（どこへも繋がない）。偽物を置いた PATH で
# `command -v claude` / `codex` / `agy` が偽物を指すことを最初に確かめる。返らない偽物は孫（`sleep`）を抱えて止まり、
# 自分と孫の pid を書く（打ち切りが孫まで止めたかを ps で見る）。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery / HOME は mktemp 配下。GUI と CLI は砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL）で動かす。落とすのは自分で起こした pid だけ。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=cli|gui|legacy（既定は全部）・SUITE=setup|folder（既定は両方。cli は setup だけ）。
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テスト（`remote_fs` / `agent_mcp` の `issue2967_*` /
# `issue2973_*`）と番犬（`crates/tako-control/tests/issue2967_unbounded_cli_wait_watchdog.rs`）。
#
# 使い方: bash scripts/test-bounded-mcp-ssh-2967.sh
set -uo pipefail
# 人が読む出力は表示言語に従う（#2365）。判定は日本語の字面なので日本語表示へ固定する（英語の段だけ外す）
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2967_LEGACY TAKO_2973_LEGACY TAKO_2961_LEGACY \
  TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR TAKO_TMUX_BIN TAKO_TMUX_SOCKET \
  TAKO_SETUP_PROBE_TIMEOUT_SECS TAKO_REMOTE_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
SUITE="${SUITE:-}"
PASS=0
FAIL=0
UNMEASURED=0
# 上限の注入（秒）。setup の問い合わせ（`TAKO_SETUP_PROBE_TIMEOUT_SECS`）と ssh / sftp（`TAKO_REMOTE_TIMEOUT_SECS`）
LIMIT=3

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
suite() { [ -z "$SUITE" ] || [ "$SUITE" = "$1" ]; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

TMP="$(mktemp -d /tmp/tako-2967-XXXXXX)"
APP_PID=""
MCP_PID=""
SOCKETS=""
FAKE_DIR="$TMP/fake"

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
  kill_fakes
  [ -n "$MCP_PID" ] && kill -KILL "$MCP_PID" >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"
  local s
  # 器はソケットファイルまで消す（#2399）
  for s in $SOCKETS; do tmux_socket_kill "$s"; done
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "MCP の登録とリモートフォルダの上限の実経路テスト（#2967 / #2973）"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy / ssh / sftp -------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR/.local/bin" "$HOME_DIR/.codex" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#2967 の実測用）。API を呼ばず、設定 dir も読まない */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static int flag(const char *name) {
  const char *dir = getenv("FAKE_DIR");
  char path[1024];
  if (!dir) return 0;
  snprintf(path, sizeof path, "%s/%s", dir, name);
  return access(path, F_OK) == 0;
}
static void record(void) {
  const char *dir = getenv("FAKE_DIR");
  if (dir) {
    char path[1024];
    snprintf(path, sizeof path, "%s/pids/%d", dir, (int)getpid());
    FILE *f = fopen(path, "w");
    if (f) fclose(f);
  }
}
/* 子と孫の組で止まる（打ち切りは孫まで止める = #2942）。どちらも pid を書く */
static void hang(void) {
  record();
  if (fork() == 0) {
    record();
    for (;;) pause();
  }
  for (;;) pause();
}
int main(int argc, char **argv) {
  if (argc > 1 && strcmp(argv[1], "tako-fake-exec-warm") == 0) return 0;
  if (argc > 1 && strcmp(argv[1], "--version") == 0) {
    printf("2.1.294 (Claude Code)\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "auth") == 0) {
    printf("{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"max\"}\n");
    return 0;
  }
  if (argc > 2 && strcmp(argv[1], "mcp") == 0 && strcmp(argv[2], "remove") == 0) {
    if (flag("hang-claude-remove")) hang();
    printf("No MCP server found with name: tako\n");
    return 1;
  }
  if (argc > 2 && strcmp(argv[1], "mcp") == 0 && strcmp(argv[2], "add") == 0) {
    if (flag("hang-claude-add")) hang();
    printf("Added stdio MCP server tako\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "mcp") == 0) {
    printf("No MCP servers configured.\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "agents") == 0) printf("[]\n");
  return 0;
}
C
cc -O1 -o "$HOME_DIR/.local/bin/claude" "$TMP/fake-claude.c" || exit 1
# 作りたての実行ファイルの初回 exec の検査を上限の外で済ませる（#2222）
"$HOME_DIR/.local/bin/claude" "$FAKE_EXEC_WARM_ARG"
# シェルの偽物の止まり方（孫の sleep を抱えて待つ。自分と孫の pid を書く）
HANG_SH='hang() { /bin/sleep 600 & : > "$FAKE_DIR/pids/$!"; : > "$FAKE_DIR/pids/$$"; wait; exit 0; }'
cat > "$TMP/stubs/codex" <<SH
#!/bin/sh
$HANG_SH
case "\$*" in
  "login status") echo "Logged in using ChatGPT" ;;
  "mcp list"*) [ -e "\$FAKE_DIR/hang-codex-list" ] && hang; echo '[]' ;;
  "mcp add"*) [ -e "\$FAKE_DIR/hang-codex-add" ] && hang; echo "Added global MCP server 'tako'." ;;
esac
exit 0
SH
cat > "$TMP/stubs/agy" <<SH
#!/bin/sh
$HANG_SH
case "\$*" in
  "mcp add"*) [ -e "\$FAKE_DIR/hang-agy-add" ] && hang; echo "added tako" ;;
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
# 偽の ssh。ControlMaster の `-O check` / `-O exit` と張る形（`-N -f`）には ControlPath のファイルで答え、
# それ以外（SSH ペイン）はどこへも繋がずに終わる。止まる印があればその形で止まる
cat > "$TMP/stubs/ssh" <<SH
#!/bin/sh
$HANG_SH
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
  check) [ -e "\$FAKE_DIR/hang-ssh-check" ] && hang; [ -n "\$cp" ] && [ -e "\$cp" ] && exit 0; exit 255 ;;
  exit) [ -e "\$FAKE_DIR/hang-ssh-exit" ] && hang; [ -n "\$cp" ] && rm -f "\$cp"; exit 0 ;;
esac
if [ "\$master" = 1 ]; then
  [ -e "\$FAKE_DIR/hang-ssh-master" ] && hang
  [ -n "\$cp" ] && : > "\$cp"
  exit 0
fi
echo "fake ssh (#2967): \$*"
exit 255
SH
# 偽の sftp（バッチの `pwd` と `ls -la` に決まった形で答える。どこへも繋がない）
cat > "$TMP/stubs/sftp" <<SH
#!/bin/sh
$HANG_SH
[ -e "\$FAKE_DIR/hang-sftp" ] && hang
while IFS= read -r line; do
  echo "sftp> \$line"
  case "\$line" in
    pwd) echo "Remote working directory: /home/testuser" ;;
    "ls -la"*)
      echo 'drwxr-xr-x    3 fake     staff          96 Nov 17 10:00 .'
      echo 'drwxr-xr-x    3 fake     staff          96 Nov 17 10:00 ..'
      echo '-rw-r--r--    1 fake     staff          12 Nov 17 10:00 hello.txt'
      echo 'drwxr-xr-x    2 fake     staff          64 Nov 17 10:00 sub' ;;
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
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/ssh" "$TMP/stubs/sftp" "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$HOME_DIR/.local/bin:$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR" "TAKO_SETUP_PROBE_TIMEOUT_SECS=$LIMIT" \
    "TAKO_REMOTE_TIMEOUT_SECS=$LIMIT"
}
# 砂場の PATH で claude / codex / agy が偽物を指すこと（本物を起こさない前提）
for name in claude codex agy; do
  resolved="$(env -i PATH="$SANDBOX_PATH" /bin/sh -c "command -v $name")"
  case "$resolved" in
    "$HOME_DIR"/* | "$TMP"/*) pass "砂場の PATH の $name は偽物（${resolved}）" ;;
    *) fail "砂場の PATH の $name が偽物でない（${resolved}）"; exit 1 ;;
  esac
done
# ログインシェル（SHELL）の答えも偽物（ssh / sftp）
for name in ssh sftp; do
  resolved="$(env -i PATH="$SANDBOX_PATH" "$TMP/stubs/isosh" -l -c "command -v $name")"
  check_eq "砂場のログインシェルの $name は偽物" "$TMP/stubs/$name" "$resolved"
done

# 印を置き直す（前の要求の偽物と pid の記録は先に片付ける = 次の判定を古い記録で満たさない）
set_flags() {
  kill_fakes
  rm -f "${FAKE_DIR:?}"/hang-*
  local name
  for name in "$@"; do : > "$FAKE_DIR/$name"; done
}

# 記録した偽物の pid の数（RECORDED）と、6 秒待っても生きている数（ALIVE）。打ち切りは TERM → 猶予 1 秒 → KILL。
# GUI の定期の見張り（#1040 の `drive_remote_recovery`）も開いたフォルダへ `ssh -O check` を起こすので、
# 印を消した後に見張りが起こした偽物（上限で打ち切られる途中のもの）も待てる長さにする
fakes_left() {
  local f i alive
  RECORDED=0
  for f in "$FAKE_DIR/pids"/*; do [ -f "$f" ] && RECORDED=$((RECORDED + 1)); done
  for i in $(seq 1 60); do
    alive=0
    for f in "$FAKE_DIR/pids"/*; do
      [ -f "$f" ] || continue
      kill -0 "${f##*/}" 2>/dev/null && alive=$((alive + 1))
    done
    [ "$alive" = 0 ] && break
    sleep 0.1
  done
  ALIVE="$alive"
}
# 打ち切りで偽物の子と孫が残らないこと（記録が 2 個以上 = 子と孫の組が止まっていた）。
# 先に印を消す（以後に起きる偽物は止まらない = 判定の後から増えない）
check_fakes_gone() {
  rm -f "${FAKE_DIR:?}"/hang-*
  fakes_left
  if [ "$RECORDED" -ge 2 ] && [ "$ALIVE" = 0 ]; then
    pass "$1: 打ち切った偽物の子と孫が残らない（記録 ${RECORDED} 個・生存 0）"
  else
    fail "$1: 偽物が残る / 止まっていない（記録 ${RECORDED} 個・生存 ${ALIVE}）"
    ps -o pid,pgid,command -p "$(ls "$FAKE_DIR/pids" | tr '\n' ',' | sed 's/,$//')" 2>/dev/null | sed 's/^/      /'
  fi
}
# 上限で返った（HUNG=0 かつ上限 + 余裕の秒数より短い）
check_bounded() {
  local label="$1" hung="$2" elapsed="$3" max="$4"
  if [ "$hung" = 0 ] && awk -v e="$elapsed" -v m="$max" 'BEGIN { exit !(e < m) }'; then
    pass "$label: 上限で返る（${elapsed} 秒・上限 ${LIMIT} 秒）"
  else
    fail "$label: 上限で返らない（${elapsed} 秒・HUNG=${hung}）"
  fi
}
check_has() {
  if grep -qF -- "$2" "$3"; then pass "$1"; else fail "$1（'$2' が無い）"; sed 's/^/      /' "$3" | head -8; fi
}

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

# 砂場の env で CLI を起こし、終わるか LIMIT 秒を過ぎるまで状態で待つ（パイプを組まない）。
#   run_cli <名前> <LIMIT 秒> [env の追加…] -- <引数…>
# 結果: RC・ELAPSED（秒・小数 1 桁）・HUNG（1 = 上限まで返らず、こちらで落とした）。GUI へは繋がない
run_cli() {
  local name="$1" limit="$2" t0 pid
  shift 2
  local extra=""
  while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do extra="$extra $1"; shift; done
  [ "${1:-}" = "--" ] && shift
  t0="$(now_ms)"
  # shellcheck disable=SC2046,SC2086
  env $(sandbox_env) $extra "$TAKO_BIN" "$@" < /dev/null > "$TMP/out/$name.out" 2> "$TMP/out/$name.err" &
  pid=$!
  HUNG=0
  RC=""
  while kill -0 "$pid" 2>/dev/null; do
    if [ $(($(now_ms) - t0)) -ge $((limit * 1000)) ]; then
      HUNG=1
      kill -KILL "$pid" 2>/dev/null
      break
    fi
    sleep 0.05
  done
  wait "$pid" 2>/dev/null
  local rc=$?
  [ "$HUNG" = 1 ] || RC="$rc"
  ELAPSED="$(awk -v ms=$(($(now_ms) - t0)) 'BEGIN { printf "%.1f", ms / 1000 }')"
}

# MCP の tools/call を起こして LIMIT 秒まで待つ。結果: MCP_HUNG・MCP_ELAPSED。本文は $TMP/out/<名前>.out
#   mcp_call <ツール> <引数の JSON> <名前> <LIMIT 秒>
mcp_call() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2967","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-$3.jsonl"
  local t0 limit="$4"
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-$3.jsonl" > "$TMP/out/$3-raw.out" 2> "$TMP/out/$3.err" &
  MCP_PID=$!
  MCP_HUNG=0
  while kill -0 "$MCP_PID" 2>/dev/null; do
    if [ $(($(now_ms) - t0)) -ge $((limit * 1000)) ]; then
      MCP_HUNG=1
      kill -KILL "$MCP_PID" 2>/dev/null
      break
    fi
    sleep 0.05
  done
  wait "$MCP_PID" 2>/dev/null
  MCP_PID=""
  MCP_ELAPSED="$(awk -v ms=$(($(now_ms) - t0)) 'BEGIN { printf "%.1f", ms / 1000 }')"
  # tools/call（id 2）の本文だけを取り出す
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
  local socket="t2967-$name-$$"
  SOCKETS="$SOCKETS $socket"
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_TMUX_SOCKET="$socket" \
    ${@+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    unmeasured "GUI を立てられない（終了コード ${rc}。tako-vd が無い / 蓋が閉じている）: 未実測"
    return 1
  fi
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || return 1
}
stop_app() {
  kill_fakes
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# 返る偽物の本文を A/B で突き合わせる（`<a>-<名前>.out` と `<b>-<名前>.out`）
compare_bodies() {
  local a="$1" b="$2" name
  shift 2
  for name in "$@"; do
    if [ ! -f "$TMP/out/$a-$name.out" ] || [ ! -f "$TMP/out/$b-$name.out" ]; then
      unmeasured "返る偽物: $name は片方の段を回していないので突き合わせない"
    elif cmp -s "$TMP/out/$a-$name.out" "$TMP/out/$b-$name.out"; then
      pass "返る偽物: $name の本文が修正前後でバイト一致（$(wc -c < "$TMP/out/$a-$name.out" | tr -d ' ') バイト）"
    else
      fail "返る偽物: $name の本文が修正前後で違う"
      diff "$TMP/out/$a-$name.out" "$TMP/out/$b-$name.out" | head -10 | sed 's/^/      /'
    fi
  done
}

# --- CLI（GUI なし。`tako setup-mcp` は手元で dispatch の 1 実装を呼ぶ） ------------------------
phase_cli() {
  echo "== cli: tako setup-mcp（上限 ${LIMIT} 秒の注入）"
  use_dirs cli
  set_flags
  run_cli cli-ok 30 -- setup-mcp
  check_eq "cli: 返る偽物: tako setup-mcp が終わる（終了コード）" "0" "$RC"
  run_cli cli-legacy-ok 30 TAKO_2967_LEGACY=1 -- setup-mcp
  compare_bodies cli cli-legacy ok
  # 標準エラーの人向けの行も同じ（偽物の答えは同じ）
  if cmp -s "$TMP/out/cli-ok.err" "$TMP/out/cli-legacy-ok.err"; then
    pass "返る偽物: tako setup-mcp の標準エラーが修正前後でバイト一致"
  else
    fail "返る偽物: tako setup-mcp の標準エラーが修正前後で違う"
    diff "$TMP/out/cli-ok.err" "$TMP/out/cli-legacy-ok.err" | head -10 | sed 's/^/      /'
  fi

  local spec flag agent step
  for spec in "hang-claude-remove claude remove" "hang-claude-add claude add" \
    "hang-codex-list codex list" "hang-codex-add codex add" "hang-agy-add agy add"; do
    # shellcheck disable=SC2086
    set -- $spec
    flag="$1" agent="$2" step="$3"
    set_flags "$flag"
    run_cli "cli-$flag" 20 -- setup-mcp --agent "$agent"
    check_bounded "cli: 返らない $agent mcp $step" "$HUNG" "$ELAPSED" $((LIMIT + 3))
    check_eq "cli: 返らない $agent mcp $step: 失敗の終了コード（登録を確かめられなかった）" "1" "$RC"
    check_has "cli: 返らない $agent mcp $step: 日本語の理由" \
      "$agent mcp $step が $LIMIT 秒で応答しなかったので打ち切りました（MCP の登録は確かめられませんでした）" \
      "$TMP/out/cli-$flag.err"
    check_has "cli: 返らない $agent mcp $step: 次の一手" "次の一手: 同じコマンド（${agent} mcp ${step}）を端末で手で実行" \
      "$TMP/out/cli-$flag.err"
    check_fakes_gone "cli: 返らない $agent mcp $step"
  done

  # 英語表示
  set_flags hang-claude-add
  run_cli cli-en 20 TAKO_LANG=en -- setup-mcp --agent claude
  check_bounded "cli: 英語表示・返らない claude mcp add" "$HUNG" "$ELAPSED" $((LIMIT + 3))
  check_has "cli: 英語表示: 理由" "claude mcp add did not respond within $LIMIT seconds" "$TMP/out/cli-en.err"
  check_has "cli: 英語表示: 次の一手" "Next step: run the same command (claude mcp add) by hand" "$TMP/out/cli-en.err"
  check_fakes_gone "cli: 英語表示"

  # 全系統（既定）: codex の一覧だけ返らなければ codex は理由つきの skip、claude / agy は登録まで進む
  set_flags hang-codex-list
  run_cli cli-all 20 -- setup-mcp
  check_bounded "cli: 全系統・codex だけ返らない" "$HUNG" "$ELAPSED" $((LIMIT + 3))
  check_eq "cli: 全系統: codex は timed_out の skip" "true/timed_out" \
    "$(jf agents/1/skipped < "$TMP/out/cli-all.out")/$(jf agents/1/error_kind < "$TMP/out/cli-all.out")"
  check_eq "cli: 全系統: claude と agy は登録まで進む" "true/true" \
    "$(jf agents/0/configured < "$TMP/out/cli-all.out")/$(jf agents/2/configured < "$TMP/out/cli-all.out")"
  check_fakes_gone "cli: 全系統・codex だけ返らない"

  # A/B: 修正前は返らない
  set_flags hang-claude-add
  run_cli cli-legacy-hang 8 TAKO_2967_LEGACY=1 -- setup-mcp --agent claude
  check_eq "cli: A/B（TAKO_2967_LEGACY=1）: 返らない claude mcp add で tako setup-mcp が 8 秒返らない" "1" "$HUNG"
  kill_fakes
}

# --- GUI（MCP） -----------------------------------------------------------------------
FOLDER_HOST="fakehost2967"
# 返る偽物で 1 回ずつ答えさせ、本文を <段>-<名前>.out へ残す（A/B の突き合わせ用）
returning_calls() {
  local phase="$1"
  set_flags
  if suite setup; then
    mcp_call tako_setup_mcp '{}' "$phase-mcp" 30
    check_eq "$phase: 返る偽物: MCP の setup-mcp が返る" "0" "$MCP_HUNG"
  fi
  if suite folder; then
    mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" "$phase-fopen" 30
    check_eq "$phase: 返る ssh / sftp: MCP の remote folder（open）が返る" "0" "$MCP_HUNG"
    mcp_call tako_remote_folder "{\"action\":\"ls\",\"host\":\"$FOLDER_HOST\"}" "$phase-fls" 30
    mcp_call tako_remote_folder '{"action":"list"}' "$phase-flist" 30
    mcp_call tako_remote_folder "{\"action\":\"open-file\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser/hello.txt\"}" "$phase-ffile" 30
    mcp_call tako_remote_folder "{\"action\":\"close\",\"host\":\"$FOLDER_HOST\"}" "$phase-fclose" 30
    check_eq "$phase: 返る ssh: MCP の remote folder（close）が返る" "0" "$MCP_HUNG"
  fi
}

phase_gui() {
  echo "== gui: 修正後（上限 ${LIMIT} 秒の注入）"
  set_flags
  start_app gui || return
  returning_calls gui
  if suite setup; then
    check_eq "gui: 返る偽物: claude へ登録した" "true" "$(jf agents/0/configured < "$TMP/out/gui-mcp.out")"
    local spec flag agent step
    for spec in "hang-claude-remove claude remove" "hang-claude-add claude add" \
      "hang-codex-list codex list" "hang-agy-add agy add"; do
      # shellcheck disable=SC2086
      set -- $spec
      flag="$1" agent="$2" step="$3"
      set_flags "$flag"
      mcp_call tako_setup_mcp "{\"agent\":\"$agent\"}" "gui-$flag" 20
      check_bounded "gui: MCP tako_setup_mcp・返らない $agent mcp $step" "$MCP_HUNG" "$MCP_ELAPSED" $((LIMIT + 3))
      check_has "gui: 返らない $agent mcp $step: 理由と次の一手" \
        "$agent mcp $step が $LIMIT 秒で応答しなかったので打ち切りました（MCP の登録は確かめられませんでした）。次の一手:" \
        "$TMP/out/gui-$flag.out"
      check_fakes_gone "gui: 返らない $agent mcp $step"
      # 錠が解けている: 後続の setup の書き込み（返る偽物の setup-mcp）がすぐ進む
      set_flags
      mcp_call tako_setup_mcp '{"agent":"claude"}' "gui-$flag-after" 20
      if [ "$MCP_HUNG" = 0 ] && awk -v e="$MCP_ELAPSED" 'BEGIN { exit !(e < 2.0) }'; then
        pass "gui: 返らない $agent mcp $step の後: 後続の setup の書き込みがすぐ進む（${MCP_ELAPSED} 秒 = 錠が解けている）"
      else
        fail "gui: 返らない $agent mcp $step の後: 後続の setup の書き込みが待たされる（${MCP_ELAPSED} 秒・HUNG=${MCP_HUNG}）"
      fi
    done
  fi
  if suite folder; then
    check_eq "gui: 返る ssh / sftp: フォルダを開いた（opened / entries）" "true/2" \
      "$(jf opened < "$TMP/out/gui-fopen.out")/$(jf entries < "$TMP/out/gui-fopen.out")"
    check_eq "gui: 返る ssh: list の connected / connection_error" "true/absent" \
      "$(jf tabs/0/remote_folders/0/connected < "$TMP/out/gui-flist.out")/$(jf tabs/0/remote_folders/0/connection_error < "$TMP/out/gui-flist.out")"
    # 開き直す（返らない段で使う）
    set_flags
    mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" gui-freopen 30
    local nr="ssh / sftp が上限の時間内に応答しませんでした（待つのを打ち切りました）"
    local next="同じ ssh / sftp を端末で手で実行して、何を待って止まるか"

    set_flags hang-sftp
    mcp_call tako_remote_folder "{\"action\":\"ls\",\"host\":\"$FOLDER_HOST\"}" gui-fls-hang 20
    check_bounded "gui: 返らない sftp（ls）" "$MCP_HUNG" "$MCP_ELAPSED" $((LIMIT + 3))
    check_has "gui: 返らない sftp（ls）: 応答なしの理由" "$nr" "$TMP/out/gui-fls-hang.out"
    check_has "gui: 返らない sftp（ls）: 次の一手と詳細" "sftp -b - ($LIMIT s)" "$TMP/out/gui-fls-hang.out"
    check_has "gui: 返らない sftp（ls）: 次の一手" "$next" "$TMP/out/gui-fls-hang.out"
    check_fakes_gone "gui: 返らない sftp（ls）"

    set_flags hang-sftp
    mcp_call tako_remote_folder "{\"action\":\"open-file\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser/hello.txt\"}" gui-ffile-hang 20
    check_bounded "gui: 返らない sftp（open-file）" "$MCP_HUNG" "$MCP_ELAPSED" $((LIMIT + 3))
    check_has "gui: 返らない sftp（open-file）: 応答なしの理由" "$nr" "$TMP/out/gui-ffile-hang.out"
    check_fakes_gone "gui: 返らない sftp（open-file）"

    # 開いたフォルダへの `ssh -O check` は GUI の定期の見張り（UI スレッド = #1040）も起こすので、要求の受け口
    # （UI スレッドの準備）が見張りの打ち切りまで待たされうる。上限 2 回ぶんまでを「上限で返る」と見る
    set_flags hang-ssh-check
    mcp_call tako_remote_folder '{"action":"list"}' gui-flist-hang 20
    check_bounded "gui: 返らない ssh -O check（list）" "$MCP_HUNG" "$MCP_ELAPSED" $((LIMIT * 2 + 3))
    check_eq "gui: 返らない ssh -O check（list）: connected / 種別" "false/no_response" \
      "$(jf tabs/0/remote_folders/0/connected < "$TMP/out/gui-flist-hang.out")/$(jf tabs/0/remote_folders/0/connection_error/kind < "$TMP/out/gui-flist-hang.out")"
    check_eq "gui: 返らない ssh -O check（list）: 詳細" "ssh -O check ($LIMIT s)" \
      "$(jf tabs/0/remote_folders/0/connection_error/detail < "$TMP/out/gui-flist-hang.out")"
    check_has "gui: 返らない ssh -O check（list）: 次の一手" "$next" "$TMP/out/gui-flist-hang.out"
    check_fakes_gone "gui: 返らない ssh -O check（list）"

    # 張る ssh が返らない: open は `-O check`（返る = 死んでいる）→ 張り直しで止まる
    set_flags hang-ssh-master
    mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"${FOLDER_HOST}b\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" gui-fopen-hang 20
    check_bounded "gui: 返らない ssh -N -f（open）" "$MCP_HUNG" "$MCP_ELAPSED" $((LIMIT + 3))
    check_has "gui: 返らない ssh -N -f（open）: 応答なしの理由と詳細" "ssh -N -f ($LIMIT s)" "$TMP/out/gui-fopen-hang.out"
    check_fakes_gone "gui: 返らない ssh -N -f（open）"

    set_flags hang-ssh-exit
    mcp_call tako_remote_folder "{\"action\":\"close\",\"host\":\"$FOLDER_HOST\"}" gui-fclose-hang 20
    check_bounded "gui: 返らない ssh -O exit（close）" "$MCP_HUNG" "$MCP_ELAPSED" $((LIMIT + 3))
    check_eq "gui: 返らない ssh -O exit（close）: 外したフォルダと master_errors" "1/$FOLDER_HOST/no_response" \
      "$(perl -MJSON::PP -e 'local $/; my $v = eval { decode_json(<STDIN>) }; print scalar(@{$v->{closed} || []})' < "$TMP/out/gui-fclose-hang.out")/$(jf master_errors/0/host < "$TMP/out/gui-fclose-hang.out")/$(jf master_errors/0/kind < "$TMP/out/gui-fclose-hang.out")"
    check_fakes_gone "gui: 返らない ssh -O exit（close）"
  fi
  stop_app
}

# --- 修正前（A/B） -----------------------------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2967_LEGACY=1 TAKO_2973_LEGACY=1（上限なしで待つ）"
  set_flags
  start_app legacy TAKO_2967_LEGACY=1 TAKO_2973_LEGACY=1 || return
  returning_calls legacy
  suite setup && compare_bodies gui legacy mcp
  suite folder && compare_bodies gui legacy fopen fls flist ffile fclose
  if suite setup; then
    set_flags hang-claude-add
    mcp_call tako_setup_mcp '{"agent":"claude"}' legacy-claude-hang 8
    check_eq "legacy: A/B: 返らない claude mcp add で MCP tako_setup_mcp が 8 秒返らない" "1" "$MCP_HUNG"
    kill_fakes
    set_flags hang-codex-list
    mcp_call tako_setup_mcp '{"agent":"codex"}' legacy-codex-hang 8
    check_eq "legacy: A/B: 返らない codex mcp list で MCP tako_setup_mcp が 8 秒返らない" "1" "$MCP_HUNG"
    kill_fakes
  fi
  if suite folder; then
    set_flags
    mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" legacy-freopen 30
    set_flags hang-sftp
    mcp_call tako_remote_folder "{\"action\":\"ls\",\"host\":\"$FOLDER_HOST\"}" legacy-fls-hang 8
    check_eq "legacy: A/B: 返らない sftp で remote folder（ls）が 8 秒返らない" "1" "$MCP_HUNG"
    kill_fakes
    set_flags hang-ssh-check
    mcp_call tako_remote_folder '{"action":"list"}' legacy-flist-hang 8
    check_eq "legacy: A/B: 返らない ssh -O check で remote folder（list）が 8 秒返らない" "1" "$MCP_HUNG"
    kill_fakes
  fi
  stop_app
}

if want cli && suite setup; then phase_cli; fi
want gui && phase_gui
want legacy && phase_legacy

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
