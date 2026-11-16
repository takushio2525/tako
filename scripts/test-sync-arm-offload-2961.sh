#!/usr/bin/env bash
# test-sync-arm-offload-2961.sh — #2961: dispatch の同期の腕で外部プロセスを待っていた要求が GUI を止めないことの実経路テスト
#
# 返らない偽物（claude の `mcp add`・tako の `setup`・git の `init`）を置いた隔離 GUI に MCP で要求を投げ、
# 答えを待っている間に別の IPC（`tako list`）が待たされるかと、perf.log の
# `メインスレッド専有: dispatch:<要求>` の有無を A/B で見る（番号は Issue #2961 の受け入れ条件）:
#   1. 修正後は要求の間も `tako list` がすぐ返り、perf.log に dispatch:<要求> の専有が出ない。
#      修正前（`TAKO_2961_LEGACY=1`）は `tako list` が要求の終わりまで待たされ、perf.log に専有が出る。
#      返る偽物では修正前後で MCP の応答の本文が同じ
#   対象の要求は段ごと: setup（SetupMcp / SetupRun / ConfigShare）
#
# **本物の claude / codex / agy / tako setup / git は起こさない**: 偽の claude は `cc` で作り、codex / agy は
# シェルの偽物。`tako setup` の子は検証プロセスでだけ読む注入 `TAKO_2961_INJECT_SETUP_BIN` で偽物を指す
# （素のままだと /Applications の本物の `tako setup` が砂場で導入器まで起こす）。git は `TAKO_GIT_BIN` で偽物を指す。
# 偽物を置いた PATH で `command -v claude` / `codex` / `agy` が偽物を指すことを最初に確かめる。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery / HOME は mktemp 配下。GUI と CLI は砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL = #2956 のスクリプトと同じ作り）で動かす。
# 落とすのは自分で起こした pid だけ。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=gui|legacy（既定は両方）。
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
  TAKO_SETUP_RUN_TIMEOUT_SECS TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
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
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

TMP="$(mktemp -d /tmp/tako-2961-XXXXXX)"
APP_PID=""
MCP_PID=""
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
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "同期の腕を background で答えるかの実経路テスト（#2961）"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy / tako / git ------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
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
    printf("2.1.294 (Claude Code)\n");
    return 0;
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
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/fake-tako" "$TMP/stubs/fake-git" \
  "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$HOME_DIR/.local/bin:$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR" "TAKO_GIT_BIN=$TMP/stubs/fake-git" \
    "TAKO_2961_INJECT_SETUP_BIN=$TMP/stubs/fake-tako"
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
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_PERSIST=0 ${@+"$@"} || rc=$?
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
  list_during "$phase・返らない claude（setup-mcp）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない claude: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"

  set_flags hang-setup
  mcp_start tako_setup '{}' "$phase-run-hang"
  list_during "$phase・返らない tako setup（setup）" "$expect" "$limit"
  mcp_wait 30
  check_eq "$phase: 返らない tako setup: 上限で MCP が返る" "0" "$MCP_HUNG"
  check_eq "$phase: 返らない tako setup: timed_out（応答の形は #1503 のまま）" "true/4" \
    "$(jf timed_out < "$TMP/out/$phase-run-hang.out")/$(jf waited_secs < "$TMP/out/$phase-run-hang.out")"

  set_flags hang-git
  rm -rf "${SHARE_DIR:?}"
  mcp_start tako_config_share "{\"action\":\"init\",\"path\":\"$SHARE_DIR\"}" "$phase-share-hang"
  list_during "$phase・返らない git（config share）" "$expect" "$limit"
  kill_fakes
  mcp_wait 30
  check_eq "$phase: 返らない git: 偽物を落とすと MCP が返る" "0" "$MCP_HUNG"
  rm -rf "${SHARE_DIR:?}"
}

report_holds() {
  local phase="$1" want_hold="$2" kind ms
  for kind in SetupMcp SetupRun ConfigShare; do
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
  grep -h "メインスレッド専有: dispatch:\(SetupMcp\|SetupRun\|ConfigShare\)" "$TAKO_DATA_DIR/perf.log" 2>/dev/null \
    | sed 's/^/      /' | head -6
}

# --- 修正後 -----------------------------------------------------------------------
phase_gui() {
  echo "== gui: 修正後（tako setup の子の上限は注入で 4 秒）"
  set_flags
  start_app gui TAKO_SETUP_RUN_TIMEOUT_SECS=4 || return
  returning_calls gui
  check_eq "gui: 返る claude: claude へ登録した" "true" "$(jf agents/0/configured < "$TMP/out/gui-mcp.out")"
  check_eq "gui: 返る tako setup: 子が答えを受け取った（completed）" "true" "$(jf completed < "$TMP/out/gui-run.out")"
  check_eq "gui: 返る git: 共有リポジトリを作った" "created" "$(jf action < "$TMP/out/gui-share.out")"
  hanging_calls gui fast 20
  report_holds gui 0
  stop_app
}

# --- 修正前（A/B） -----------------------------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2961_LEGACY=1（UI スレッドで答える）"
  set_flags
  start_app legacy TAKO_2961_LEGACY=1 TAKO_SETUP_RUN_TIMEOUT_SECS=4 || return
  returning_calls legacy
  local name
  for name in mcp run share share-status; do
    if [ ! -f "$TMP/out/gui-$name.out" ]; then
      unmeasured "返る偽物: $name は修正後の段を回していないので突き合わせない（ONLY=legacy）"
    elif cmp -s "$TMP/out/gui-$name.out" "$TMP/out/legacy-$name.out"; then
      pass "返る偽物: MCP の $name の本文が修正前後でバイト一致（$(wc -c < "$TMP/out/gui-$name.out" | tr -d ' ') バイト）"
    else
      fail "返る偽物: MCP の $name の本文が修正前後で違う"
      diff "$TMP/out/gui-$name.out" "$TMP/out/legacy-$name.out" | head -10 | sed 's/^/      /'
    fi
  done
  hanging_calls legacy blocked 6
  report_holds legacy 1
  stop_app
}

want gui && phase_gui
want legacy && phase_legacy

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
