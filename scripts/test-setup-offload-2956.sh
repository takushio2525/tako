#!/usr/bin/env bash
# test-setup-offload-2956.sh — #2956: MCP の tako_setup_bootstrap / tako_setup_models が GUI を止めないことの実経路テスト
#
# 返らない偽の claude（`auth status`）・codex（`debug models`）を置いた隔離 GUI に MCP で問い合わせ、
# 答えを待っている間に別の IPC（`tako list`）が待たされるかと、perf.log の
# `メインスレッド専有: dispatch:SetupBootstrap` / `dispatch:SetupModels` の有無を A/B で見る（番号は Issue #2956 の受け入れ条件）:
#   1. 修正後は問い合わせの間も `tako list` がすぐ返り、perf.log に dispatch:SetupBootstrap / SetupModels の専有が出ない。
#      MCP の応答は今の形のまま返る（返らない claude は上限の秒数で auth_unconfirmed つき）。
#      修正前（`TAKO_2956_LEGACY=1`）は `tako list` が問い合わせの終わりまで待たされ、perf.log に専有が出る
#   返る偽物では修正前後で MCP の応答が同じ（status と models の本文を突き合わせる）
#
# **本物の claude / codex / agy は起こさない**: 偽の claude を `cc` で作り、codex / agy はシェルの偽物。
# 偽物を置いた PATH で `command -v claude` / `codex` / `agy` が偽物を指すことを最初に確かめる。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery / HOME は mktemp 配下。GUI と CLI は砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL = #2942 のスクリプトと同じ作り）で動かす。
# 落とすのは自分で起こした pid だけ。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=gui|legacy（既定は両方）。
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テスト（dispatch.rs の `issue2956_*`）と番犬
# （`crates/tako-control/tests/issue2956_setup_dispatch_offload_watchdog.rs`）。
#
# 使い方: bash scripts/test-setup-offload-2956.sh
set -uo pipefail
# 人が読む出力は表示言語に従う（#2365）。判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2956_LEGACY TAKO_2942_LEGACY TAKO_AUTH_PROBE_TIMEOUT_SECS \
  TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR
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

TMP="$(mktemp -d /tmp/tako-2956-XXXXXX)"
APP_PID=""
MCP_PID=""
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
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "setup の問い合わせを background で答えるかの実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy -------------------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR/.local/bin" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#2956 の実測用）。API を呼ばず、設定 dir も読まない */
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
    if (flag("hang-claude")) {
      record();
      for (;;) pause();
    }
    printf("{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"max\"}\n");
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
  "debug models")
    if [ -e "$FAKE_DIR/hang-codex" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    echo '{"models":[{"slug":"gpt-test","display_name":"GPT-Test","visibility":"list","priority":1,"supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}],"default_reasoning_level":"low","context_window":1000}]}' ;;
  "mcp list"*) echo '[]' ;;
esac
exit 0
SH
cat > "$TMP/stubs/agy" <<'SH'
#!/bin/sh
case "$*" in
  models) printf 'agy-test\tAgy Test\n' ;;
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$HOME_DIR/.local/bin:$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR"
}
# 砂場の PATH で claude / codex / agy が偽物を指すこと（本物を起こさない前提）
for name in claude codex agy; do
  resolved="$(env -i PATH="$SANDBOX_PATH" /bin/sh -c "command -v $name")"
  case "$resolved" in
    "$HOME_DIR"/* | "$TMP"/*) pass "砂場の PATH の $name は偽物（${resolved}）" ;;
    *) fail "砂場の PATH の $name が偽物でない（${resolved}）"; exit 1 ;;
  esac
done

# 印を置き直す（前の問い合わせの偽物と pid の記録は先に片付ける = 次の「止まった」の判定を古い記録で満たさない）
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
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2956","version":"0"}}}' \
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

# 返らない偽物が止まったこと（自分の pid を書いた = GUI が問い合わせを起こした）を状態で待つ（上限 20 秒）
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

# 問い合わせの間に `tako list` が待たされるか（期待 = fast / blocked）。
# 返らない偽物が問い合わせで止まったのを確かめてから 1 回だけ叩く
list_during() {
  local label="$1" expect="$2" limit="$3"
  if ! wait_fake_started; then
    fail "$label: 20 秒待っても偽物が問い合わせで止まらない（GUI が問い合わせを起こしていない）"
    return
  fi
  run_case "list-$label" "$limit" -- list
  echo "    問い合わせの最中の tako list: ${ELAPSED} 秒（HUNG=${HUNG}）"
  case "$expect" in
    fast)
      if [ "$HUNG" = 0 ] && awk -v e="$ELAPSED" 'BEGIN { exit !(e < 1.5) }'; then
        pass "$label: 問い合わせの最中も tako list がすぐ返る（${ELAPSED} 秒）"
      else
        fail "$label: 問い合わせの最中に tako list が待たされた（${ELAPSED} 秒・HUNG=${HUNG}）"
      fi
      ;;
    blocked)
      if [ "$HUNG" = 1 ] || awk -v e="$ELAPSED" 'BEGIN { exit !(e >= 2.0) }'; then
        pass "$label: tako list が問い合わせの終わりまで待たされる（${ELAPSED} 秒・HUNG=$HUNG = GUI が止まっている）"
      else
        fail "$label: tako list がすぐ返った（${ELAPSED} 秒。A/B なのに止まっていない）"
      fi
      ;;
  esac
}

# --- 修正後 -----------------------------------------------------------------------
phase_gui() {
  echo "== gui: 修正後（認証の問い合わせの上限は注入で 3 秒）"
  set_flags
  start_app gui TAKO_AUTH_PROBE_TIMEOUT_SECS=3 || return

  # 返る偽物: 応答の形（A/B と突き合わせる）
  mcp_call tako_setup_bootstrap '{"action":"status"}' gui-status 60
  check_eq "返る claude: MCP の status が返る" "0" "$MCP_HUNG"
  check_eq "返る claude: 判定（認証 / 段）" "true/ready" \
    "$(jf authenticated < "$TMP/out/gui-status.out")/$(jf next_step < "$TMP/out/gui-status.out")"
  mcp_call tako_setup_models '{}' gui-models 60
  check_eq "返る codex / agy: MCP の models が返る" "0" "$MCP_HUNG"
  check_eq "返る codex: 実取得（live / 先頭の id）" "true/gpt-test" \
    "$(jf agents/1/live < "$TMP/out/gui-models.out")/$(jf agents/1/models/0/id < "$TMP/out/gui-models.out")"

  # 返らない claude: 問い合わせの最中も GUI が答える
  set_flags hang-claude
  mcp_start tako_setup_bootstrap '{"action":"status","agent":"claude"}' gui-hang
  list_during "修正後・返らない claude（bootstrap）" fast 20
  mcp_wait 60
  check_eq "返らない claude: MCP が返る" "0" "$MCP_HUNG"
  echo "    MCP の応答まで ${MCP_ELAPSED} 秒（上限 3 秒）"
  check_eq "返らない claude: auth_unconfirmed の秒（応答の形は #2942 のまま）" "3" \
    "$(jf auth_unconfirmed/waited_secs < "$TMP/out/gui-hang.out")"

  # 返らない codex（`debug models`。#2951 の前は上限なし = 偽物を落とすまで返らない）
  set_flags hang-codex
  mcp_start tako_setup_models '{"agent":"codex"}' gui-models-hang
  list_during "修正後・返らない codex（models）" fast 20
  # 応答が返らなくても GUI は答え続ける。偽物を落とせば background のジョブも終わる
  kill_fakes
  mcp_wait 30
  check_eq "返らない codex: 偽物を落とすと MCP が返る（background のジョブが終わる）" "0" "$MCP_HUNG"
  check_eq "返らない codex: 一覧は実取得できない（live = false）" "false" "$(jf agents/0/live < "$TMP/out/gui-models-hang.out")"

  local b m
  b="$(perf_hold_ms SetupBootstrap)"
  m="$(perf_hold_ms SetupModels)"
  echo "    perf.log: dispatch:SetupBootstrap の専有の最大 ${b}ms / dispatch:SetupModels ${m}ms"
  grep -h "SetupBootstrap\|SetupModels" "$TAKO_DATA_DIR/perf.log" 2>/dev/null | sed 's/^/      /' | head -5
  check_eq "perf.log に dispatch:SetupBootstrap の専有が出ない" "0" "$b"
  check_eq "perf.log に dispatch:SetupModels の専有が出ない" "0" "$m"
  stop_app
}

# --- 修正前（A/B） -----------------------------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2956_LEGACY=1（UI スレッドで答える）"
  set_flags
  start_app legacy TAKO_2956_LEGACY=1 TAKO_AUTH_PROBE_TIMEOUT_SECS=3 || return

  mcp_call tako_setup_bootstrap '{"action":"status"}' legacy-status 60
  mcp_call tako_setup_models '{}' legacy-models 60
  if [ -f "$TMP/out/gui-status.out" ]; then
    if cmp -s "$TMP/out/gui-status.out" "$TMP/out/legacy-status.out"; then
      pass "返る偽物: MCP の status の本文が修正前後で同じ"
    else
      fail "返る偽物: MCP の status の本文が修正前後で違う"
      diff "$TMP/out/gui-status.out" "$TMP/out/legacy-status.out" | head -10 | sed 's/^/      /'
    fi
    if cmp -s "$TMP/out/gui-models.out" "$TMP/out/legacy-models.out"; then
      pass "返る偽物: MCP の models の本文が修正前後で同じ"
    else
      fail "返る偽物: MCP の models の本文が修正前後で違う"
      diff "$TMP/out/gui-models.out" "$TMP/out/legacy-models.out" | head -10 | sed 's/^/      /'
    fi
  fi

  set_flags hang-claude
  mcp_start tako_setup_bootstrap '{"action":"status","agent":"claude"}' legacy-hang
  list_during "修正前・返らない claude（bootstrap）" blocked 20
  mcp_wait 60
  echo "    MCP の応答まで ${MCP_ELAPSED} 秒"

  set_flags hang-codex
  mcp_start tako_setup_models '{"agent":"codex"}' legacy-models-hang
  list_during "修正前・返らない codex（models）" blocked 6
  kill_fakes
  mcp_wait 30

  local b m
  b="$(perf_hold_ms SetupBootstrap)"
  m="$(perf_hold_ms SetupModels)"
  echo "    perf.log: dispatch:SetupBootstrap の専有の最大 ${b}ms / dispatch:SetupModels ${m}ms"
  grep -h "メインスレッド専有: dispatch:Setup" "$TAKO_DATA_DIR/perf.log" 2>/dev/null | sed 's/^/      /' | head -5
  if [ "$b" -ge 2500 ]; then pass "A/B: perf.log に dispatch:SetupBootstrap の専有（${b}ms）"; else fail "A/B: dispatch:SetupBootstrap の専有が出ない（${b}ms）"; fi
  if [ "$m" -ge 2500 ]; then pass "A/B: perf.log に dispatch:SetupModels の専有（${m}ms）"; else fail "A/B: dispatch:SetupModels の専有が出ない（${m}ms）"; fi
  stop_app
}

want gui && phase_gui
want legacy && phase_legacy

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
