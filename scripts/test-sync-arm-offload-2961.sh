#!/usr/bin/env bash
# test-sync-arm-offload-2961.sh — #2961: dispatch の同期の腕で外部プロセスを待っていた要求が GUI を止めないことの実経路テスト
#
# 返らない偽物（claude の `mcp add`・tako の `setup`・git の `init`）を置いた隔離 GUI に MCP で要求を投げ、
# 答えを待っている間に別の IPC（`tako list`）が待たされるかと、perf.log の
# `メインスレッド専有: dispatch:<要求>` の有無を A/B で見る（番号は Issue #2961 の受け入れ条件）:
#   1. 修正後は要求の間も `tako list` がすぐ返り、perf.log に dispatch:<要求> の専有が出ない。
#      修正前（`TAKO_2961_LEGACY=1`）は `tako list` が要求の終わりまで待たされ、perf.log に専有が出る。
#      返る偽物では修正前後で MCP の応答の本文が同じ
#   対象の要求は組ごと（SUITE）: setup（SetupMcp / SetupRun / ConfigShare）・remote（RemoteSetup /
#   RemoteScrollback / OpenRemote）・folder（RemoteFolder の open / ls / list / pending / open-file / close）・
#   sleep（SleepGuard の蓋閉じの準備・片付け）・stale（#2966 の版の問い合わせの回数と、tako-app の
#   `apply_stale_binary_scan` = 2 秒 tick の着地が `claude --version` を UI スレッドで待たないこと）
#
# **本物の claude / codex / agy / tako setup / git は起こさない**: 偽の claude は `cc` で作り、codex / agy は
# シェルの偽物。`tako setup` の子は検証プロセスでだけ読む注入 `TAKO_2961_INJECT_SETUP_BIN` で偽物を指す
# （素のままだと /Applications の本物の `tako setup` が砂場で導入器まで起こす）。git は `TAKO_GIT_BIN` で偽物を指す。
# tailscale は `TAKO_TAILSCALE_BIN` で偽物を指し、ssh は偽のログインシェルが `command -v ssh` に偽物を答える。
# 器（tmux）は本物を隔離ソケットで使い、`TAKO_TMUX_BIN` の包みが採取の `-S -4321` のときだけ止まる。
# 偽物を置いた PATH で `command -v claude` / `codex` / `agy` が偽物を指すことを最初に確かめる。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery / HOME は mktemp 配下。GUI と CLI は砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL = #2956 のスクリプトと同じ作り）で動かす。
# 落とすのは自分で起こした pid だけ。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=gui|legacy（既定は両方）・SUITE=setup|remote|folder|sleep|stale（既定は全部）。
# **本物の sudoers / pmset には触らない**: osascript は PATH の先頭の偽物、sudoers の有無と `sudo pmset` は
# #2904 の偽の置き場（`TAKO_2904_FAKE_PMSET`。検証プロセスでだけ読む）。
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テスト（dispatch.rs の `issue2961_*`）と番犬
# （`crates/tako-control/tests/issue2956_setup_dispatch_offload_watchdog.rs` の `TARGETS`）。
#
# 使い方: bash scripts/test-sync-arm-offload-2961.sh
set -uo pipefail
# 人が読む出力は表示言語に従う（#2365）。判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2961_LEGACY TAKO_2961_INJECT_SETUP_BIN TAKO_GIT_BIN \
  TAKO_SETUP_RUN_TIMEOUT_SECS TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR \
  TAKO_TAILSCALE_BIN TAKO_TMUX_BIN TAKO_TMUX_SOCKET TAKO_2904_FAKE_PMSET TAKO_2966_LEGACY \
  TAKO_SETUP_PROBE_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
SUITE="${SUITE:-}"
PASS=0
FAIL=0
UNMEASURED=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
suite() { [ -z "$SUITE" ] || [ "$SUITE" = "$1" ]; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

TMP="$(mktemp -d /tmp/tako-2961-XXXXXX)"
APP_PID=""
MCP_PID=""
SOCKETS=""
FAKE_DIR="$TMP/fake"
SHARE_DIR="$TMP/share"

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
tako_exit_trap cleanup "同期の腕を background で答えるかの実経路テスト（#2961）"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy / tako / git ------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
REAL_TMUX="$(command -v tmux || true)"
if [ -z "$REAL_TMUX" ]; then
  unmeasured "tmux が無い（器のスクロールバックの採取に要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR/.local/bin" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#2961 の実測用）。API を呼ばず、設定 dir も読まない */
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
int main(int argc, char **argv) {
  if (argc > 1 && strcmp(argv[1], "tako-fake-exec-warm") == 0) return 0;
  if (argc > 1 && strcmp(argv[1], "--version") == 0) {
    if (flag("hang-claude-version")) {
      /* 子と孫の組で止まる（#2942 / #2951 の打ち切りは孫まで止める）。どちらも pid を書く */
      record();
      if (fork() == 0) {
        record();
        for (;;) pause();
      }
      for (;;) pause();
    }
    printf("2.1.294 (Claude Code)\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "--serve2961") == 0) {
    /* 器のペインで「起動時のバイナリ」として動き続ける（stale の検知の相手）。pid は別の置き場へ */
    const char *dir = getenv("FAKE_DIR");
    if (dir) {
      char path[1024];
      snprintf(path, sizeof path, "%s/serve-%d", dir, (int)getpid());
      FILE *f = fopen(path, "w");
      if (f) fclose(f);
    }
    for (;;) pause();
  }
  if (argc > 1 && strcmp(argv[1], "auth") == 0) {
    printf("{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"max\"}\n");
    return 0;
  }
  if (argc > 2 && strcmp(argv[1], "mcp") == 0 && strcmp(argv[2], "add") == 0) {
    if (flag("hang-claude-mcp")) {
      record();
      for (;;) pause();
    }
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
cat > "$TMP/stubs/codex" <<'SH'
#!/bin/sh
case "$*" in
  "login status") echo "Logged in using ChatGPT" ;;
  "mcp list"*) echo '[]' ;;
esac
exit 0
SH
cat > "$TMP/stubs/agy" <<'SH'
#!/bin/sh
case "$*" in
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
# 偽の tako（`tako setup --yes --answers -` だけを受ける。注入 `TAKO_2961_INJECT_SETUP_BIN` で指す）
cat > "$TMP/stubs/fake-tako" <<'SH'
#!/bin/sh
case "$1" in
  setup)
    cat > "$FAKE_DIR/setup-answers.json"
    if [ -e "$FAKE_DIR/hang-setup" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    echo "[OK] 偽の tako setup（#2961）"
    exit 0 ;;
esac
exit 0
SH
# 偽の git（`TAKO_GIT_BIN` で指す。共有リポジトリの init / add / diff だけを決まった形で答える）
cat > "$TMP/stubs/fake-git" <<'SH'
#!/bin/sh
case "$1" in
  --version) echo "git version 2.39.0 (fake 2961)" ;;
  init)
    if [ -e "$FAKE_DIR/hang-git" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    mkdir -p .git ;;
esac
exit 0
SH
# 偽の osascript（sudoers の登録・削除の管理者の確認の代わり。PATH の先頭 = GUI が素の名前で起こす）
cat > "$TMP/stubs/osascript" <<'SH'
#!/bin/sh
if [ -e "$FAKE_DIR/hang-osascript" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
case "$*" in
  *"rm -f"*) echo "OK: sudoers 削除完了・disablesleep 解除" ;;
  *) echo "OK: sudoers 登録完了" ;;
esac
exit 0
SH
# #2904 の偽の pmset の置き場（sudoers の有無はここのファイル・`sudo pmset` はここの偽の sudo）
FAKE_PMSET="$TMP/fakepmset"
mkdir -p "$FAKE_PMSET"
printf '#!/bin/sh\necho "$*" >> "%s/sudo.log"\nexit 0\n' "$FAKE_PMSET" > "$FAKE_PMSET/sudo"
# 偽の tailscale（`TAKO_TAILSCALE_BIN` で指す。検出の `--version` が止まる / 決まった形で答える）
cat > "$TMP/stubs/tailscale" <<'SH'
#!/bin/sh
case "$1" in
  --version)
    if [ -e "$FAKE_DIR/hang-tailscale" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    printf '1.80.0\n  tailscale commit: fake2961\n' ;;
  status) echo '{"BackendState":"NeedsLogin","Self":{"DNSName":""}}' ;;
esac
exit 0
SH
# 偽の ssh。ControlMaster の `-O check` / `-O exit` と張る形（`-N -f`）には ControlPath のファイルで答え、
# それ以外（SSH ペイン）はどこへも繋がずに終わる。止まる印があればその形で止まる
cat > "$TMP/stubs/ssh" <<'SH'
#!/bin/sh
cp=""; op=""; master=0; prev=""
for a in "$@"; do
  case "$a" in
    ControlPath=*) cp="${a#ControlPath=}"; cp="${cp#\"}"; cp="${cp%\"}" ;;
    -N) master=1 ;;
  esac
  [ "$prev" = "-O" ] && op="$a"
  prev="$a"
done
hang() { : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; }
case "$op" in
  check) [ -e "$FAKE_DIR/hang-ssh-check" ] && hang; [ -n "$cp" ] && [ -e "$cp" ] && exit 0; exit 255 ;;
  exit) [ -e "$FAKE_DIR/hang-ssh-exit" ] && hang; [ -n "$cp" ] && rm -f "$cp"; exit 0 ;;
esac
if [ "$master" = 1 ]; then
  [ -e "$FAKE_DIR/hang-ssh-master" ] && hang
  [ -n "$cp" ] && : > "$cp"
  exit 0
fi
echo "fake ssh (#2961): $*"
exit 255
SH
# 偽の sftp（バッチの `pwd` と `ls -la` に決まった形で答える。どこへも繋がない）
cat > "$TMP/stubs/sftp" <<'SH'
#!/bin/sh
[ -e "$FAKE_DIR/hang-sftp" ] && { : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; }
while IFS= read -r line; do
  echo "sftp> $line"
  case "$line" in
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
# 器の tmux の包み（本物へ渡す。スクロールバックの採取 `-S -4321` だけ止まる）
cat > "$TMP/stubs/tmux-wrap" <<SH
#!/bin/sh
for a in "\$@"; do
  if [ "\$a" = "-4321" ] && [ -e "\$FAKE_DIR/hang-tmux" ]; then : > "\$FAKE_DIR/pids/\$\$"; exec /bin/sleep 600; fi
done
exec "$REAL_TMUX" "\$@"
SH
# 偽のログインシェル: `command -v ssh` には偽の ssh を答える（止まる印があれば止まる）。それ以外は /bin/sh
cat > "$TMP/stubs/isosh" <<SH
#!/bin/sh
if [ "\$1" = "-l" ]; then shift; fi
case "\$*" in
  *"command -v ssh"*)
    if [ -e "\$FAKE_DIR/hang-shell" ]; then : > "\$FAKE_DIR/pids/\$\$"; exec /bin/sleep 600; fi
    echo "$TMP/stubs/ssh"; exit 0 ;;
  *"command -v sftp"*) echo "$TMP/stubs/sftp"; exit 0 ;;
esac
exec /bin/sh "\$@"
SH
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/fake-tako" "$TMP/stubs/fake-git" \
  "$TMP/stubs/tailscale" "$TMP/stubs/ssh" "$TMP/stubs/sftp" "$TMP/stubs/tmux-wrap" "$TMP/stubs/isosh" \
  "$TMP/stubs/osascript" "$FAKE_PMSET/sudo" || exit 1
# stale の検知の相手（起動時のバイナリ）は今の claude と別のパスに置いた同じ偽物
mkdir -p "$TMP/old"
command cp -f "$HOME_DIR/.local/bin/claude" "$TMP/old/claude"
"$TMP/old/claude" "$FAKE_EXEC_WARM_ARG"
SANDBOX_PATH="$HOME_DIR/.local/bin:$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR" "TAKO_GIT_BIN=$TMP/stubs/fake-git" \
    "TAKO_2961_INJECT_SETUP_BIN=$TMP/stubs/fake-tako" "TAKO_TAILSCALE_BIN=$TMP/stubs/tailscale" \
    "TAKO_TMUX_BIN=$TMP/stubs/tmux-wrap" "TAKO_2904_FAKE_PMSET=$FAKE_PMSET"
}
# 砂場の PATH で claude / codex / agy が偽物を指すこと（本物を起こさない前提）
for name in claude codex agy; do
  resolved="$(env -i PATH="$SANDBOX_PATH" /bin/sh -c "command -v $name")"
  case "$resolved" in
    "$HOME_DIR"/* | "$TMP"/*) pass "砂場の PATH の $name は偽物（${resolved}）" ;;
    *) fail "砂場の PATH の $name が偽物でない（${resolved}）"; exit 1 ;;
  esac
done

# 印を置き直す（前の要求の偽物と pid の記録は先に片付ける = 次の「止まった」の判定を古い記録で満たさない）
set_flags() {
  kill_fakes
  rm -f "${FAKE_DIR:?}"/hang-*
  local name
  for name in "$@"; do : > "$FAKE_DIR/$name"; done
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

# 砂場の env で GUI の CLI を起こし、終わるか LIMIT 秒を過ぎるまで状態で待つ（パイプを組まない）。
#   run_case <名前> <LIMIT 秒> -- <引数…>
# 結果: RC・ELAPSED（秒・小数 1 桁）・HUNG（1 = 上限まで返らず、こちらで落とした）
run_case() {
  local name="$1" limit="$2" t0 pid
  shift 2
  [ "${1:-}" = "--" ] && shift
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" "$@" < /dev/null > "$TMP/out/$name.out" 2> "$TMP/out/$name.err" &
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

# MCP の tools/call を背景で起こす（`mcp_wait` で終わりを待つ）。
#   mcp_start <ツール> <引数の JSON> <名前>
mcp_start() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2961","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-$3.jsonl"
  MCP_NAME="$3"
  MCP_T0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-$3.jsonl" > "$TMP/out/$3-raw.out" 2> "$TMP/out/$3.err" &
  MCP_PID=$!
}
# 背景の MCP の終わりを LIMIT 秒まで待つ。結果: MCP_HUNG（1 = 返らず落とした）・MCP_ELAPSED。本文は $TMP/out/<名前>.out
mcp_wait() {
  local limit="$1"
  MCP_HUNG=0
  while kill -0 "$MCP_PID" 2>/dev/null; do
    if [ $(($(now_ms) - MCP_T0)) -ge $((limit * 1000)) ]; then
      MCP_HUNG=1
      kill -KILL "$MCP_PID" 2>/dev/null
      break
    fi
    sleep 0.05
  done
  wait "$MCP_PID" 2>/dev/null
  MCP_PID=""
  MCP_ELAPSED="$(awk -v ms=$(($(now_ms) - MCP_T0)) 'BEGIN { printf "%.1f", ms / 1000 }')"
  # tools/call（id 2）の本文だけを取り出す
  perl -MJSON::PP -ne '
    my $m = eval { JSON::PP->new->utf8->decode($_) } or next;
    next unless defined $m->{id} && $m->{id} == 2;
    binmode STDOUT, ":utf8";
    for my $c (@{ $m->{result}{content} || [] }) { print $c->{text}, "\n" if $c->{type} eq "text" }
    last;' "$TMP/out/$MCP_NAME-raw.out" > "$TMP/out/$MCP_NAME.out"
}
mcp_call() {
  mcp_start "$1" "$2" "$3"
  mcp_wait "$4"
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

# perf.log の「メインスレッド専有: dispatch:<要求> が Nms」の最大の N（無ければ 0）
perf_hold_ms() {
  local log="$TAKO_DATA_DIR/perf.log"
  [ -f "$log" ] || { echo 0; return; }
  perl -ne 'BEGIN { $m = 0 } $m = $1 if /メインスレッド専有: dispatch:'"$1"' が (\d+)ms/ && $1 > $m; END { print "$m\n" }' "$log"
}

start_app() {
  local name="$1"
  shift
  use_dirs "$name"
  # 器あり（スクロールバックの採取は器のペインが要る）・隔離ソケット
  local socket="t2961-$name-$$"
  SOCKETS="$SOCKETS $socket"
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_PERSIST=1 TAKO_TMUX_SOCKET="$socket" \
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

# 返らない偽物が止まったこと（自分の pid を書いた = GUI が要求で偽物を起こした）を状態で待つ（上限 20 秒）
wait_fake_started() {
  local i f
  for i in $(seq 1 400); do
    for f in "$FAKE_DIR/pids"/*; do
      [ -f "$f" ] && return 0
    done
    sleep 0.05
  done
  return 1
}

# 要求の間に `tako list` が待たされるか（期待 = fast / blocked）。
# 返らない偽物が止まったのを確かめてから 1 回だけ叩く
list_during() {
  local label="$1" expect="$2" limit="$3"
  if ! wait_fake_started; then
    fail "$label: 20 秒待っても偽物が止まらない（GUI が偽物を起こしていない）"
    return
  fi
  run_case "list-$label" "$limit" -- list
  echo "    要求の最中の tako list: ${ELAPSED} 秒（HUNG=${HUNG}）"
  case "$expect" in
    fast)
      if [ "$HUNG" = 0 ] && awk -v e="$ELAPSED" 'BEGIN { exit !(e < 1.5) }'; then
        pass "$label: 要求の最中も tako list がすぐ返る（${ELAPSED} 秒）"
      else
        fail "$label: 要求の最中に tako list が待たされた（${ELAPSED} 秒・HUNG=${HUNG}）"
      fi
      ;;
    blocked)
      if [ "$HUNG" = 1 ] || awk -v e="$ELAPSED" 'BEGIN { exit !(e >= 2.0) }'; then
        pass "$label: tako list が要求の終わりまで待たされる（${ELAPSED} 秒・HUNG=$HUNG = GUI が止まっている）"
      else
        fail "$label: tako list がすぐ返った（${ELAPSED} 秒。A/B なのに止まっていない）"
      fi
      ;;
  esac
}

# 返る偽物で 1 回ずつ答えさせ、本文を <段>-<名前>.out へ残す（A/B の突き合わせ用）
returning_calls() {
  local phase="$1"
  set_flags
  rm -rf "${SHARE_DIR:?}"
  mcp_call tako_setup_mcp '{}' "$phase-mcp" 60
  check_eq "$phase: 返る claude: MCP の setup-mcp が返る" "0" "$MCP_HUNG"
  mcp_call tako_setup '{}' "$phase-run" 60
  check_eq "$phase: 返る tako setup: MCP の setup が返る" "0" "$MCP_HUNG"
  mcp_call tako_config_share "{\"action\":\"init\",\"path\":\"$SHARE_DIR\"}" "$phase-share" 60
  check_eq "$phase: 返る git: MCP の config share（init）が返る" "0" "$MCP_HUNG"
  mcp_call tako_config_share '{"action":"status"}' "$phase-share-status" 60
  rm -rf "${SHARE_DIR:?}"
}

# 返らない偽物の 3 本（期待 = fast / blocked）。setup は上限 4 秒の注入で返る・setup-mcp と git は偽物を落とすと返る
hanging_calls() {
  local phase="$1" expect="$2" limit="$3"
  set_flags hang-claude-mcp
  mcp_start tako_setup_mcp '{"agent":"claude"}' "$phase-mcp-hang"
  list_during "${phase}・返らない claude（setup-mcp）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない claude: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-setup
  mcp_start tako_setup '{}' "$phase-run-hang"
  list_during "${phase}・返らない tako setup（setup）" "$expect" "$limit"
  mcp_wait 30
  check_eq "$phase: 返らない tako setup: 上限で MCP が返る" "0" "$MCP_HUNG"
  check_eq "$phase: 返らない tako setup: timed_out（応答の形は #1503 のまま）" "true/4" \
    "$(jf timed_out < "$TMP/out/$phase-run-hang.out")/$(jf waited_secs < "$TMP/out/$phase-run-hang.out")"

  set_flags hang-git
  rm -rf "${SHARE_DIR:?}"
  mcp_start tako_config_share "{\"action\":\"init\",\"path\":\"$SHARE_DIR\"}" "$phase-share-hang"
  list_during "${phase}・返らない git（config share）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない git: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"
  rm -rf "${SHARE_DIR:?}"
}

# 最初のタブの最初のペインの ID（`tako list` の JSON。スクロールバックはペインを指して採る）
first_pane() {
  run_case "$1" 30 -- list
  jf tabs/0/panes/0/id < "$TMP/out/$1.out"
}

# remote 系: 返る偽物で 1 回ずつ答えさせる（A/B の突き合わせ用）
returning_remote_calls() {
  local phase="$1" pane
  set_flags
  mcp_call tako_remote_setup '{"action":"check"}' "$phase-rsetup" 60
  check_eq "$phase: 返る tailscale: MCP の remote setup（check）が返る" "0" "$MCP_HUNG"
  pane="$(first_pane "$phase-list0")"
  echo "    採取するペイン: ${pane}"
  mcp_call tako_remote_scrollback "{\"pane_id\":\"${pane}\",\"lines\":4321}" "$phase-scroll" 60
  check_eq "$phase: 返る tmux: MCP のスクロールバックが返る" "0" "$MCP_HUNG"
  mcp_call tako_open_remote '{"host":"tako-2961-nohost.invalid","focus":false}' "$phase-open" 60
  check_eq "$phase: 返るログインシェル: MCP の open remote が返る" "0" "$MCP_HUNG"
}

# remote 系: 返らない偽物の 3 本（期待 = fast / blocked）。どれも上限つき（tailscale 10 秒・器 5 秒・ログインシェル 5 秒）
hanging_remote_calls() {
  local phase="$1" expect="$2" limit="$3" pane
  pane="$(first_pane "$phase-list1")"
  set_flags hang-tailscale
  mcp_start tako_remote_setup '{"action":"check"}' "$phase-rsetup-hang"
  list_during "${phase}・返らない tailscale（remote setup）" "$expect" "$limit"
  mcp_wait 40
  check_eq "$phase: 返らない tailscale: 上限で MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-tmux
  mcp_start tako_remote_scrollback "{\"pane_id\":\"${pane}\",\"lines\":4321}" "$phase-scroll-hang"
  list_during "${phase}・返らない器の採取（scrollback）" "$expect" "$limit"
  mcp_wait 30
  check_eq "$phase: 返らない器の採取: 上限で MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-shell
  mcp_start tako_open_remote '{"host":"tako-2961-nohost.invalid","focus":false}' "$phase-open-hang"
  list_during "${phase}・返らないログインシェル（open remote）" "$expect" "$limit"
  mcp_wait 30
  check_eq "$phase: 返らないログインシェル: 上限で MCP が返る（ペインは素の ssh で立つ）" "0" "$MCP_HUNG"
}

# folder 系: 返る偽物で 1 回ずつ答えさせる（A/B の突き合わせ用）。最後にフォルダを開き直して残す（止まる段で使う）
FOLDER_HOST="fakehost2961"
returning_folder_calls() {
  local phase="$1"
  set_flags
  mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"focus\":false}" "$phase-fopen" 60
  check_eq "$phase: 返る ssh / sftp: MCP の remote folder（open）が返る" "0" "$MCP_HUNG"
  mcp_call tako_remote_folder "{\"action\":\"ls\",\"host\":\"$FOLDER_HOST\"}" "$phase-fls" 60
  mcp_call tako_remote_folder '{"action":"list"}' "$phase-flist" 60
  mcp_call tako_remote_folder '{"action":"pending"}' "$phase-fpending" 60
  mcp_call tako_remote_folder "{\"action\":\"open-file\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser/hello.txt\"}" "$phase-ffile" 60
  mcp_call tako_remote_folder "{\"action\":\"close\",\"host\":\"$FOLDER_HOST\"}" "$phase-fclose" 60
  check_eq "$phase: 返る ssh: MCP の remote folder（close）が返る" "0" "$MCP_HUNG"
  # 止まる段のために開き直す（突き合わせない）
  mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" "$phase-freopen" 60
}

# folder 系: 返らない偽物の 4 本（期待 = fast / blocked）。sftp と `ssh -O` は上限なしなので偽物を落として返す
hanging_folder_calls() {
  local phase="$1" expect="$2" limit="$3"
  set_flags hang-sftp
  mcp_start tako_remote_folder "{\"action\":\"ls\",\"host\":\"$FOLDER_HOST\"}" "$phase-fls-hang"
  list_during "${phase}・返らない sftp（remote folder ls）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない sftp（ls）: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-sftp
  mcp_start tako_remote_folder "{\"action\":\"open-file\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser/hello.txt\"}" "$phase-ffile-hang"
  list_during "${phase}・返らない sftp（remote folder open-file）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない sftp（open-file）: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-ssh-check
  mcp_start tako_remote_folder '{"action":"list"}' "$phase-flist-hang"
  list_during "${phase}・返らない ssh -O check（remote folder list）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない ssh -O check（list）: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-ssh-exit
  mcp_start tako_remote_folder "{\"action\":\"close\",\"host\":\"$FOLDER_HOST\"}" "$phase-fclose-hang"
  list_during "${phase}・返らない ssh -O exit（remote folder close）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない ssh -O exit（close）: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"
  check_eq "$phase: close の応答（外したフォルダ）" "[\"$FOLDER_HOST:/home/testuser\"]" \
    "$(jf closed < "$TMP/out/$phase-fclose-hang.out")"
}

# sleep 系: 返る偽物（osascript）で 1 回ずつ答えさせる（A/B の突き合わせ用）。sudoers は偽の置き場
returning_sleep_calls() {
  local phase="$1"
  set_flags
  rm -f "${FAKE_PMSET:?}/sudoers"
  mcp_call tako_sleep_guard '{"action":"install-lid-sleep"}' "$phase-sinstall" 60
  check_eq "$phase: 返る osascript: MCP の sleep guard（install-lid-sleep）が返る" "0" "$MCP_HUNG"
  : > "$FAKE_PMSET/sudoers"
  mcp_call tako_sleep_guard '{"action":"remove-lid-sleep"}' "$phase-sremove" 60
  check_eq "$phase: 返る osascript: MCP の sleep guard（remove-lid-sleep）が返る" "0" "$MCP_HUNG"
  rm -f "${FAKE_PMSET:?}/sudoers"
}

# sleep 系: 返らない osascript（管理者の確認を待つ形）の 2 本
hanging_sleep_calls() {
  local phase="$1" expect="$2" limit="$3"
  set_flags hang-osascript
  rm -f "${FAKE_PMSET:?}/sudoers"
  mcp_start tako_sleep_guard '{"action":"install-lid-sleep"}' "$phase-sinstall-hang"
  list_during "${phase}・返らない osascript（sleep guard install-lid-sleep）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない osascript（install）: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-osascript
  : > "$FAKE_PMSET/sudoers"
  mcp_start tako_sleep_guard '{"action":"remove-lid-sleep"}' "$phase-sremove-hang"
  list_during "${phase}・返らない osascript（sleep guard remove-lid-sleep）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない osascript（remove）: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"
  rm -f "${FAKE_PMSET:?}/sudoers"
}

# 記録した偽物の pid の数（返らない `claude --version` の子と孫）
fake_pid_count() {
  local n=0 f
  for f in "$FAKE_DIR/pids"/*; do [ -f "$f" ] && n=$((n + 1)); done
  echo "$n"
}

# stale 系 1（#2966）: 器のペインの状態の照会が今の claude の版を何回読むか（返らない `claude --version`・上限 3 秒）
stale_status_call() {
  local phase="$1" pane="$2"
  set_flags
  # 今の claude の覚え（5 秒）が切れるのを待ってから聞く（起動時の解決の覚えに当たると読まない）
  sleep 6
  set_flags hang-claude-version
  mcp_call tako_stale_binary "{\"action\":\"status\",\"pane\":${pane}}" "$phase-stale" 40
  STALE_PIDS="$(fake_pid_count)"
  STALE_ELAPSED="$MCP_ELAPSED"
  echo "    状態の照会: ${MCP_ELAPSED} 秒・偽の claude --version の pid の記録 ${STALE_PIDS} 個（子と孫の組 = 2 個で 1 回）"
  kill_fakes
}

# stale 系 2: 2 秒 tick の着地（`apply_stale_binary_scan`）。器のペインを worker にして別のパスの偽の claude を
# 走らせ、版の読み（返らない `claude --version`・上限 3 秒）が UI スレッドで起きるかを perf.log で見る
stale_landing_case() {
  local phase="$1" pane="$2" i
  set_flags
  mcp_call tako_set_title "{\"pane\":${pane},\"role\":\"worker\"}" "$phase-role" 30
  mcp_call tako_send_input "{\"pane\":${pane},\"text\":\"$TMP/old/claude --serve2961\"}" "$phase-serve" 30
  for i in $(seq 1 100); do
    ls "$FAKE_DIR"/serve-* >/dev/null 2>&1 && break
    sleep 0.1
  done
  if ! ls "$FAKE_DIR"/serve-* >/dev/null 2>&1; then
    fail "$phase: 器のペインで偽の claude（起動時のバイナリ）が起動しない"
    return
  fi
  set_flags hang-claude-version
  # 走査は 2 秒ごと。今の claude と起動時のバイナリの版の読み（各 3 秒）の後に着地する
  for i in $(seq 1 300); do
    [ -f "$TAKO_DATA_DIR/perf.log" ] && grep -q "periodic_land:stale_binary" "$TAKO_DATA_DIR/perf.log" && break
    [ "$(fake_pid_count)" -ge 4 ] && [ "$i" -ge 150 ] && break
    sleep 0.1
  done
  STALE_LAND_MS="$(perl -ne 'BEGIN { $m = 0 } $m = $1 if /メインスレッド専有: periodic_land:stale_binary が (\d+)ms/ && $1 > $m; END { print "$m\n" }' "$TAKO_DATA_DIR/perf.log" 2>/dev/null)"
  echo "    着地（periodic_land:stale_binary）の専有の最大 ${STALE_LAND_MS:-0}ms・偽の claude --version の pid の記録 $(fake_pid_count) 個"
  grep -h "periodic_land:stale_binary" "$TAKO_DATA_DIR/perf.log" 2>/dev/null | sed 's/^/      /' | head -3
  mcp_call tako_stale_binary "{\"action\":\"status\",\"pane\":${pane}}" "$phase-stale-land" 40
  kill_fakes
}

report_holds() {
  local phase="$1" want_hold="$2" kind ms kinds="${3:-SetupMcp SetupRun ConfigShare}"
  for kind in $kinds; do
    ms="$(perf_hold_ms "$kind")"
    echo "    perf.log: dispatch:$kind の専有の最大 ${ms}ms"
    if [ "$want_hold" = 0 ]; then
      check_eq "$phase: perf.log に dispatch:$kind の専有が出ない" "0" "$ms"
    elif [ "$ms" -ge 2500 ]; then
      pass "$phase: A/B: perf.log に dispatch:$kind の専有（${ms}ms）"
    else
      fail "$phase: A/B: dispatch:$kind の専有が出ない（${ms}ms）"
    fi
  done
  grep -h "メインスレッド専有: dispatch:\(SetupMcp\|SetupRun\|ConfigShare\|RemoteSetup\|RemoteScrollback\|OpenRemote\|RemoteFolder\|SleepGuard\)" \
    "$TAKO_DATA_DIR/perf.log" 2>/dev/null | sed 's/^/      /' | head -12
}

# 返る偽物の本文を修正前後で突き合わせる（`names` は <段>-<名前>.out の名前）
compare_bodies() {
  local name
  for name in "$@"; do
    if [ ! -f "$TMP/out/gui-$name.out" ]; then
      unmeasured "返る偽物: $name は修正後の段を回していないので突き合わせない（ONLY=legacy）"
    elif cmp -s "$TMP/out/gui-$name.out" "$TMP/out/legacy-$name.out"; then
      pass "返る偽物: MCP の $name の本文が修正前後でバイト一致（$(wc -c < "$TMP/out/gui-$name.out" | tr -d ' ') バイト）"
    else
      fail "返る偽物: MCP の $name の本文が修正前後で違う"
      diff "$TMP/out/gui-$name.out" "$TMP/out/legacy-$name.out" | head -10 | sed 's/^/      /'
    fi
  done
}

# --- 修正後 -----------------------------------------------------------------------
phase_gui() {
  echo "== gui: 修正後（tako setup の子の上限は注入で 4 秒）"
  set_flags
  start_app gui TAKO_SETUP_RUN_TIMEOUT_SECS=4 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return
  if suite setup; then
    returning_calls gui
    check_eq "gui: 返る claude: claude へ登録した" "true" "$(jf agents/0/configured < "$TMP/out/gui-mcp.out")"
    check_eq "gui: 返る tako setup: 子が答えを受け取った（completed）" "true" "$(jf completed < "$TMP/out/gui-run.out")"
    check_eq "gui: 返る git: 共有リポジトリを作った" "created" "$(jf action < "$TMP/out/gui-share.out")"
    hanging_calls gui fast 20
    report_holds gui 0
  fi
  if suite remote; then
    returning_remote_calls gui
    check_eq "gui: 返る tailscale: 偽物を検出した" "tailscale/ok" "$(jf items/0/item < "$TMP/out/gui-rsetup.out")/$(jf items/0/status < "$TMP/out/gui-rsetup.out")"
    check_eq "gui: 返る器の採取: 行を採った" "true" \
      "$(perl -MJSON::PP -e 'local $/; my $v = eval { decode_json(<STDIN>) }; print((ref $v->{lines} eq "ARRAY" && @{$v->{lines}} > 0) ? "true" : "false")' < "$TMP/out/gui-scroll.out")"
    check_eq "gui: 返るログインシェル: 偽の ssh でペインを立てた" "tako-2961-nohost.invalid" "$(jf host < "$TMP/out/gui-open.out")"
    hanging_remote_calls gui fast 20
    report_holds gui 0 "RemoteSetup RemoteScrollback OpenRemote"
  fi
  if suite folder; then
    returning_folder_calls gui
    check_eq "gui: 返る ssh / sftp: フォルダを開いた（opened / entries）" "true/2" \
      "$(jf opened < "$TMP/out/gui-fopen.out")/$(jf entries < "$TMP/out/gui-fopen.out")"
    check_eq "gui: 返る ssh: 開いたフォルダは繋がっている（list の connected）" "true" \
      "$(jf tabs/0/remote_folders/0/connected < "$TMP/out/gui-flist.out")"
    hanging_folder_calls gui fast 20
    report_holds gui 0 "RemoteFolder"
  fi
  if suite sleep; then
    returning_sleep_calls gui
    check_eq "gui: 返る osascript: 蓋閉じ継続の準備（result / lid_sleep_mode）" "OK: sudoers 登録完了/while-agents-running" \
      "$(jf result < "$TMP/out/gui-sinstall.out")/$(jf lid_sleep_mode < "$TMP/out/gui-sinstall.out")"
    hanging_sleep_calls gui fast 20
    report_holds gui 0 "SleepGuard"
  fi
  if suite stale; then
    local pane
    pane="$(first_pane gui-stale-list)"
    stale_status_call gui "$pane"
    check_eq "gui: #2966: 返らない claude --version を 1 回だけ起こす（子と孫の組 = 2 個）" "2" "$STALE_PIDS"
    if awk -v e="$STALE_ELAPSED" 'BEGIN { exit !(e < 5.5) }'; then
      pass "gui: #2966: 状態の照会は上限 1 回分で返る（${STALE_ELAPSED} 秒・上限 3 秒）"
    else
      fail "gui: #2966: 状態の照会が上限 1 回分で返らない（${STALE_ELAPSED} 秒）"
    fi
    stale_landing_case gui "$pane"
    check_eq "gui: 着地（apply_stale_binary_scan）が UI スレッドで claude --version を待たない" "0" "${STALE_LAND_MS:-0}"
    check_eq "gui: 着地は stale を検知している（状態の照会の stale）" "true" "$(jf stale < "$TMP/out/gui-stale-land.out")"
  fi
  stop_app
}

# --- 修正前（A/B） -----------------------------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2961_LEGACY=1（UI スレッドで答える）"
  set_flags
  start_app legacy TAKO_2961_LEGACY=1 TAKO_SETUP_RUN_TIMEOUT_SECS=4 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return
  if suite setup; then
    returning_calls legacy
    compare_bodies mcp run share share-status
    hanging_calls legacy blocked 6
    report_holds legacy 1
  fi
  if suite remote; then
    returning_remote_calls legacy
    compare_bodies rsetup scroll open
    hanging_remote_calls legacy blocked 6
    report_holds legacy 1 "RemoteSetup RemoteScrollback OpenRemote"
  fi
  if suite folder; then
    returning_folder_calls legacy
    compare_bodies fopen fls flist fpending ffile fclose
    hanging_folder_calls legacy blocked 6
    report_holds legacy 1 "RemoteFolder"
  fi
  if suite sleep; then
    returning_sleep_calls legacy
    compare_bodies sinstall sremove
    hanging_sleep_calls legacy blocked 6
    report_holds legacy 1 "SleepGuard"
  fi
  stop_app
  if suite stale; then
    # #2966 の A/B は別の GUI（`TAKO_2966_LEGACY=1`。#2961 の A/B とは門が別）
    start_app legacy2966 TAKO_2966_LEGACY=1 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return
    local pane
    pane="$(first_pane legacy2966-stale-list)"
    stale_status_call legacy2966 "$pane"
    check_eq "legacy2966: A/B: 返らない claude --version を 2 回起こす（子と孫の組 × 2 = 4 個）" "4" "$STALE_PIDS"
    if awk -v e="$STALE_ELAPSED" 'BEGIN { exit !(e >= 5.5) }'; then
      pass "legacy2966: A/B: 状態の照会は上限 2 回分かかる（${STALE_ELAPSED} 秒）"
    else
      fail "legacy2966: A/B: 状態の照会が上限 2 回分かからない（${STALE_ELAPSED} 秒）"
    fi
    stop_app
    # #2961 の A/B（着地で版を読む修正前の形）
    start_app legacystale TAKO_2961_LEGACY=1 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return
    pane="$(first_pane legacystale-list)"
    stale_landing_case legacystale "$pane"
    if [ "${STALE_LAND_MS:-0}" -ge 2500 ]; then
      pass "legacystale: A/B: 着地が UI スレッドで claude --version を待つ（periodic_land:stale_binary ${STALE_LAND_MS}ms）"
    else
      fail "legacystale: A/B: 着地の専有が出ない（${STALE_LAND_MS:-0}ms）"
    fi
    stop_app
  fi
}

want gui && phase_gui
want legacy && phase_legacy

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
