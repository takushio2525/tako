#!/usr/bin/env bash
# test-agent-probe-bounded-2951.sh — #2951: モデル一覧と stale binary の版の問い合わせが返らない CLI で固まらないことの実経路テスト
#
# `debug models` に返らない偽の codex・`models` に返らない偽の agy・`--version` に返らない偽の claude
# （どれも孫を 1 つ抱えて止まる）を置き、次を実測する（番号は Issue #2951 の受け入れ条件）:
#   2. 修正後は `tako setup models`（CLI）・MCP `tako_setup_models`・MCP `tako_stale_binary` が上限の秒数で返る。
#      モデル一覧は `failure.kind = timed_out`（`unconfirmed.waited_secs`）、版は空 = 取れない扱い。
#      打ち切った後に偽物の子も孫も残っていない（偽物が自分で書いた pid を ps で見る）。
#      修正前（`TAKO_2951_LEGACY=1`）は上限まで返らない。返る偽物では修正前後で結果が同じ
#   エッジ: codex と agy が両方返らなくても全系統の一覧は上限 1 回ぶん（並べて聞く）/ 上限の env に 0 を
#   渡しても外れない（既定の 15 秒へ落ちる）
#
# **本物の claude / codex / agy は起こさない**: 偽の claude を `cc` で作り、codex / agy はシェルの偽物。
# 偽物を置いた PATH で `command -v claude` / `codex` / `agy` が偽物を指すことを最初に確かめる。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery / HOME は mktemp 配下。GUI と CLI は砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL = #2942 のスクリプトと同じ作り）で動かす。
# MCP `tako_stale_binary` は器のペインが要るので GUI は器あり（`TAKO_PERSIST=1`・隔離ソケット）で立て、
# 器はソケットファイルまで片付ける（#2399）。落とすのは自分で起こした pid だけ。窓は仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=cli|legacy|gui（既定は全部）。
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テスト（`agent_models` の `timed_out`）と番犬
# （`crates/tako-control/tests/issue2951_agent_probe_unbounded_watchdog.rs`）。
#
# 使い方: bash scripts/test-agent-probe-bounded-2951.sh
set -uo pipefail
# 人が読む出力は表示言語に従う（#2365）。判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2951_LEGACY TAKO_SETUP_PROBE_TIMEOUT_SECS TAKO_AUTH_PROBE_TIMEOUT_SECS \
  TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR TAKO_TMUX_SOCKET
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
# 修正前が「返らない」と言い切るまで待つ秒数（修正後の既定の上限 15 秒の 2 倍）
LEGACY_WAIT="${LEGACY_WAIT:-30}"
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
contains() { case "$(cat "$1")" in *"$2"*) return 0 ;; esac; return 1; }
within() { awk -v e="$1" -v lo="$2" -v hi="$3" 'BEGIN { exit !(e >= lo && e < hi) }'; }

TMP="$(mktemp -d /tmp/tako-2951-XXXXXX)"
APP_PID=""
MCP_PID=""
SOCKETS=""
FAKE_DIR="$TMP/fake"
ALL_PIDS="$TMP/all-pids"

# 偽物を落とす（止まる前に自分で書いた pid だけ）。親から SIGTERM 無視を継ぐことがあるので SIGKILL
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
tako_exit_trap cleanup "エージェント CLI の問い合わせの上限の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy -------------------------------------------------------
for bin in cc tmux; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    unmeasured "${bin} が無い（偽の claude に cc・器に tmux が要る）"
    exit 1
  fi
done
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR/.local/bin" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
: > "$ALL_PIDS"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#2951 の実測用）。API を呼ばず、設定 dir も読まない */
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
/* 孫を 1 つ作ってから、親子とも止まる（Windows 実機の claude.cmd が ping を抱えた形） */
static void hang(void) {
  pid_t child = fork();
  record();
  (void)child;
  for (;;) pause();
}
int main(int argc, char **argv) {
  if (argc > 1 && strcmp(argv[1], "tako-fake-exec-warm") == 0) return 0;
  if (argc > 1 && strcmp(argv[1], "--version") == 0) {
    if (flag("hang-claude")) hang();
    printf("2.1.294 (Claude Code)\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "auth") == 0) {
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
# 返らない形は孫（sleep）を 1 つ抱えて止まる。子と孫の pid を記録する
cat > "$TMP/stubs/codex" <<'SH'
#!/bin/sh
case "$*" in
  "login status") echo "Logged in using ChatGPT" ;;
  "debug models")
    if [ -e "$FAKE_DIR/hang-codex" ]; then
      /bin/sleep 600 &
      : > "$FAKE_DIR/pids/$!"
      : > "$FAKE_DIR/pids/$$"
      wait
      exit 0
    fi
    echo '{"models":[{"slug":"gpt-test","display_name":"GPT-Test","visibility":"list","priority":1,"supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}],"default_reasoning_level":"low","context_window":1000}]}' ;;
  "mcp list"*) echo '[]' ;;
esac
exit 0
SH
cat > "$TMP/stubs/agy" <<'SH'
#!/bin/sh
case "$*" in
  models)
    if [ -e "$FAKE_DIR/hang-agy" ]; then
      /bin/sleep 600 &
      : > "$FAKE_DIR/pids/$!"
      : > "$FAKE_DIR/pids/$$"
      wait
      exit 0
    fi
    printf 'agy-test\tAgy Test\n' ;;
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/isosh" || exit 1
# 器（tmux）は本物を使う（偽物だけの PATH に置く）
ln -s "$(command -v tmux)" "$TMP/stubs/tmux"
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

# 印を置き直す（前の問い合わせの偽物と pid の記録は先に片付ける）
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

# 砂場の env でコマンドを起こし、終わるか LIMIT 秒を過ぎるまで状態で待つ（パイプを組まない）。
#   run_case <名前> <LIMIT 秒> [VAR=VAL …] -- <引数…>
# 結果: RC・ELAPSED（秒・小数 1 桁）・HUNG（1 = 上限まで返らず、こちらで落とした）
run_case() {
  local name="$1" limit="$2" t0 pid
  shift 2
  local extra=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do extra+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) ${extra[@]+"${extra[@]}"} "$TAKO_BIN" "$@" < "${RUN_STDIN:-/dev/null}" \
    > "$TMP/out/$name.out" 2> "$TMP/out/$name.err" &
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
  local f
  for f in "$FAKE_DIR/pids"/*; do
    [ -f "$f" ] && echo "${f##*/}" >> "$ALL_PIDS"
  done
}

# 止まった偽物（子と孫）の記録の数
fakes_recorded() {
  local f n=0
  for f in "$FAKE_DIR/pids"/*; do [ -f "$f" ] && n=$((n + 1)); done
  echo "$n"
}
# 偽物の pid が全部いなくなるまで待つ（状態で待つ・上限 10 秒）。残った pid を出す
fakes_left() {
  local i f pid left
  for i in $(seq 1 200); do
    left=""
    for f in "$FAKE_DIR/pids"/*; do
      [ -f "$f" ] || continue
      pid="${f##*/}"
      if ps -o stat= -p "$pid" >/dev/null 2>&1; then left="$left $pid"; else rm -f "$f"; fi
    done
    [ -z "$left" ] && return 0
    sleep 0.05
  done
  echo "$left"
}
expect_no_fakes() {
  local label="$1" left
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "$label: 打ち切った偽物（子と孫）は残っていない"; else fail "$label: 偽物が残っている:$left"; fi
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

# --- 2: CLI（`tako setup models`） ----------------------------------------------------
phase_cli() {
  echo "== cli: tako setup models（修正後）"
  use_dirs cli

  # 返る偽物（A/B と突き合わせる）
  set_flags
  run_case ok 60 -- setup models --json
  check_eq "返る偽物: 返る" "0" "$HUNG"
  check_eq "返る偽物: codex は実取得（live / 先頭の id）" "true/gpt-test" \
    "$(jf agents/1/live < "$TMP/out/ok.out")/$(jf agents/1/models/0/id < "$TMP/out/ok.out")"
  check_eq "返る偽物: agy は実取得（live / 先頭の id）" "true/agy-test" \
    "$(jf agents/2/live < "$TMP/out/ok.out")/$(jf agents/2/models/0/id < "$TMP/out/ok.out")"

  # 返らない codex（上限を 3 秒へ・--json）
  set_flags hang-codex
  run_case hang-json 60 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- setup models --agent codex --json
  check_eq "返らない codex（3 秒・--json）: 返る" "0" "$HUNG"
  check_eq "返らない codex（3 秒・--json）: 終了コード 0" "0" "$RC"
  if within "$ELAPSED" 3 8; then pass "返らない codex: 上限の 3 秒で返る（${ELAPSED} 秒）"; else fail "返らない codex: 経過 ${ELAPSED} 秒（上限 3 秒）"; fi
  check_eq "返らない codex: failure.kind" "timed_out" "$(jf agents/0/failure/kind < "$TMP/out/hang-json.out")"
  check_eq "返らない codex: unconfirmed の名札" "codex debug models" "$(jf agents/0/failure/unconfirmed/label < "$TMP/out/hang-json.out")"
  check_eq "返らない codex: unconfirmed の秒" "3" "$(jf agents/0/failure/unconfirmed/waited_secs < "$TMP/out/hang-json.out")"
  check_eq "返らない codex: 候補を並べない（source）" "none" "$(jf agents/0/source < "$TMP/out/hang-json.out")"
  echo "    failure.message: $(jf agents/0/failure/message < "$TMP/out/hang-json.out" | head -1)"
  check_eq "返らない codex: 偽物の子と孫の pid を記録した（2 個）" "2" "$(fakes_recorded)"
  expect_no_fakes "返らない codex（--json）"

  # 返らない agy（既定の上限 15 秒・人が読む形）
  set_flags hang-agy
  run_case hang-text 60 -- setup models --agent agy
  check_eq "返らない agy（既定・人が読む形）: 返る" "0" "$HUNG"
  if within "$ELAPSED" 15 21; then pass "返らない agy: 既定の 15 秒で返る（${ELAPSED} 秒）"; else fail "返らない agy: 経過 ${ELAPSED} 秒（既定 15 秒）"; fi
  sed 's/^/      /' "$TMP/out/hang-text.err" | head -12
  if contains "$TMP/out/hang-text.err" "[確認できません] agy models（15 秒応答なし）。打ち切って次へ進みます"; then
    pass "返らない agy: stderr に打ち切りの知らせ（#1503 と同じ文面）"
  else
    fail "返らない agy: stderr に打ち切りの知らせが無い"
  fi
  if contains "$TMP/out/hang-text.err" "agy CLIのモデル一覧（\`agy models\`）が 15 秒で応答しなかったので打ち切りました（一覧は確かめられませんでした）"; then
    pass "返らない agy: 理由（確かめられなかった）と次の一手"
  else
    fail "返らない agy: 理由の行が無い"
  fi
  if contains "$TMP/out/hang-text.err" "ログインしていない"; then fail "返らない agy: 未ログインと言っている"; else pass "返らない agy: 未ログインとは言わない"; fi
  expect_no_fakes "返らない agy（既定）"

  # エッジ: codex と agy が両方返らない → 全系統は並べて聞くので全体は上限 1 回ぶん
  set_flags hang-codex hang-agy
  run_case hang-all 60 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- setup models --json
  check_eq "両方返らない（全系統）: 返る" "0" "$HUNG"
  check_eq "両方返らない: codex / agy とも timed_out" "timed_out/timed_out" \
    "$(jf agents/1/failure/kind < "$TMP/out/hang-all.out")/$(jf agents/2/failure/kind < "$TMP/out/hang-all.out")"
  if within "$ELAPSED" 3 6; then
    pass "両方返らない: 全体 ${ELAPSED} 秒（1 系統ずつなら 6 秒以上 = 上限 3 秒 × 2）"
  else
    fail "両方返らない: 全体 ${ELAPSED} 秒（並べて聞けていない）"
  fi
  expect_no_fakes "両方返らない"

  # エッジ: 上限の env に 0 を渡しても外れない（既定の 15 秒へ落ちる）
  set_flags hang-codex
  run_case hang-zero 60 TAKO_SETUP_PROBE_TIMEOUT_SECS=0 -- setup models --agent codex --json
  check_eq "上限の env が 0: 返る（無制限にならない）" "0" "$HUNG"
  check_eq "上限の env が 0: 既定の 15 秒で打ち切る" "15" "$(jf agents/0/failure/unconfirmed/waited_secs < "$TMP/out/hang-zero.out")"
  expect_no_fakes "上限の env が 0"
}

# --- 2: 修正前（A/B） ----------------------------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2951_LEGACY=1"
  use_dirs legacy
  set_flags
  run_case legacy-ok 60 TAKO_2951_LEGACY=1 -- setup models --json
  if [ -f "$TMP/out/ok.out" ]; then
    if cmp -s "$TMP/out/ok.out" "$TMP/out/legacy-ok.out"; then
      pass "返る偽物: tako setup models --json の出力が修正前後で同じ"
    else
      fail "返る偽物: 出力が修正前後で違う"
      diff "$TMP/out/ok.out" "$TMP/out/legacy-ok.out" | head -10 | sed 's/^/      /'
    fi
  fi
  set_flags hang-codex
  run_case legacy-hang "$LEGACY_WAIT" TAKO_2951_LEGACY=1 -- setup models --agent codex --json
  check_eq "A/B: 返らない codex で ${LEGACY_WAIT} 秒待っても返らない" "1" "$HUNG"
  check_eq "A/B: stdout は 0 バイト（何も出さずに固まる）" "0" "$(wc -c < "$TMP/out/legacy-hang.out" | tr -d ' ')"
  kill_fakes
}

# --- 2: MCP（隔離 GUI・器あり） --------------------------------------------------------
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2951","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-$3.jsonl"
  RUN_STDIN="$TMP/mcp-$3.jsonl" run_case "$3-raw" "$4" \
    TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" -- mcp serve
  # tools/call（id 2）の本文だけを取り出す
  perl -MJSON::PP -ne '
    my $m = eval { JSON::PP->new->utf8->decode($_) } or next;
    next unless defined $m->{id} && $m->{id} == 2;
    binmode STDOUT, ":utf8";
    for my $c (@{ $m->{result}{content} || [] }) { print $c->{text}, "\n" if $c->{type} eq "text" }
    last;' "$TMP/out/$3-raw.out" > "$TMP/out/$3.out"
}

start_app() {
  local name="$1"
  shift
  use_dirs "$name"
  local socket="t2951-$name-$$"
  SOCKETS="$SOCKETS $socket"
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_PERSIST=1 TAKO_TMUX_SOCKET="$socket" \
    ${@+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    unmeasured "GUI を立てられない（終了コード ${rc}。tako-vd が無い / 蓋が閉じている）: MCP は未実測"
    return 1
  fi
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || return 1
}
# 最初のタブの最初のペインの ID（`tako list` の JSON。stale binary の状態はペインを指して聞く）
first_pane() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  run_case "$1" 30 TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" -- list
  jf tabs/0/panes/0/id < "$TMP/out/$1.out"
}
stop_app() {
  kill_fakes
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

phase_gui() {
  echo "== gui: MCP tako_setup_models / tako_stale_binary（GUI の上限は注入で 3 秒）"
  set_flags
  start_app gui TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return

  set_flags hang-codex
  mcp_call tako_setup_models '{"agent":"codex"}' gui-models 60
  check_eq "MCP models・返らない codex: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 3 秒）"
  check_eq "MCP models: failure.kind" "timed_out" "$(jf agents/0/failure/kind < "$TMP/out/gui-models.out")"
  check_eq "MCP models: unconfirmed の秒" "3" "$(jf agents/0/failure/unconfirmed/waited_secs < "$TMP/out/gui-models.out")"
  expect_no_fakes "MCP models"

  # stale binary: 器のペインの版の確かめ（パスに版が無い偽の claude = `claude --version` が最終手段）
  local pane
  pane="$(first_pane gui-list)"
  echo "    対象のペイン: ${pane}"
  set_flags hang-claude
  mcp_call tako_stale_binary "{\"action\":\"status\",\"pane\":${pane}}" gui-stale 60
  check_eq "MCP stale_binary・返らない claude --version: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 3 秒）"
  echo "    要点: $(perl -MJSON::PP -0ne 'my $d = eval { JSON::PP->new->utf8->decode($_) } or exit; print JSON::PP->new->canonical->encode({ map { $_ => $d->{$_} } grep { exists $d->{$_} } qw(stale reason current_version spawned_version) })' "$TMP/out/gui-stale.out")"
  check_eq "MCP stale_binary: 版は空（取れない扱い）" "" "$(jf current_version < "$TMP/out/gui-stale.out")"
  # 状態の照会は今の claude の版を 2 回読む（`resolve_current_claude_binary` の覚えの中と応答の欄）ので、
  # 返らない `--version` は 2 回起き、全体は上限 × 2。どちらも子と孫の組で止まる
  local recorded
  recorded="$(fakes_recorded)"
  if [ "$recorded" -ge 2 ] && [ $((recorded % 2)) -eq 0 ]; then
    pass "MCP stale_binary: 偽物の子と孫の組の pid を記録した（${recorded} 個）"
  else
    fail "MCP stale_binary: 偽物の pid の記録が子と孫の組になっていない（${recorded} 個）"
  fi
  if within "$ELAPSED" 3 8; then pass "MCP stale_binary: 上限 3 秒 × 2 回の内で返る（${ELAPSED} 秒）"; else fail "MCP stale_binary: 経過 ${ELAPSED} 秒"; fi
  expect_no_fakes "MCP stale_binary"
  stop_app

  echo "== gui legacy: TAKO_2951_LEGACY=1 の GUI"
  set_flags
  start_app gui-legacy TAKO_2951_LEGACY=1 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return
  local pane
  pane="$(first_pane gui-legacy-list)"
  set_flags hang-claude
  mcp_call tako_stale_binary "{\"action\":\"status\",\"pane\":${pane}}" gui-legacy-stale "$LEGACY_WAIT"
  check_eq "MCP stale_binary・A/B: ${LEGACY_WAIT} 秒待っても返らない" "1" "$HUNG"
  stop_app
}

want cli && phase_cli
want legacy && phase_legacy
want gui && phase_gui

# 2: 起こした偽物が 1 つも残っていない（全段の pid を見直す。A/B で落としたものも含む）
kill_fakes
left=""
while read -r pid; do
  [ -n "$pid" ] || continue
  case "$left " in *" $pid "*) continue ;; esac
  ps -o stat= -p "$pid" >/dev/null 2>&1 && left="$left $pid"
done < "$ALL_PIDS"
recorded="$(sort -u "$ALL_PIDS" | grep -c . || true)"
if [ -z "$left" ]; then
  pass "全段: 記録した偽物の pid ${recorded} 個はどれも残っていない"
else
  fail "全段: 偽物が残っている:$left"
fi

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
