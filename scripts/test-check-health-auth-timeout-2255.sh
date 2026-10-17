#!/usr/bin/env bash
# test-check-health-auth-timeout-2255.sh — #2255: check-health の認証確認が返らない CLI で固まらないことの実経路テスト
#
# 返らない偽の `claude auth status --json` を置き、次を実測する（番号は Issue #2255 の受け入れ条件）:
#   1. 修正後は数秒で他の項目と「認証: 不明（N 秒で応答なし）」を返す（CLI の人が読む形・`--json`・MCP
#      `tako_check_health`）。修正前（`TAKO_2255_LEGACY=1` と、OLD_BIN に渡した修正前のバイナリ）は返らない
#   2. 打ち切った後に偽の子プロセスが残っていない（偽物が自分で書いた pid で確かめる）
#   4. 偽の claude が即答するときは今まで通りの認証状態（認証済み / プラン）が出る
#   エッジ: 3 系統とも返らなくても全体は上限 1 回ぶん（並べて聞く）/ 上限の env に 0 を渡しても外れない（既定へ落ちる）
#
# **本物の claude / codex / agy は起こさない**: 偽の claude を `cc` で作る（サブコマンドに即答して抜ける。
# 印のファイル `hang-claude` があるときだけ `auth` で止まる）。codex / agy はシェルの偽物（`hang-codex` /
# `hang-agy` で止まる。止まるときは `exec sleep` = 打ち切りの kill が届く直接の子）。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery は mktemp 配下。CLI も GUI も砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL = #1505 / #1891 のスクリプトと同じ作り）で動かす。
# 落とすのは自分で起こした pid だけ（偽物は止まる前に自分の pid を書く）。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=cli|gui|legacy|old（既定は全部。old は OLD_BIN があるときだけ）。
#   OLD_BIN=<修正前の tako CLI>  例: OLD_BIN=/Applications/tako.app/Contents/MacOS/tako（#2255 の修正が入る前の配布物）
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue2255_diagnostics_auth_probe_watchdog.rs`）。
#
# 使い方: bash scripts/test-check-health-auth-timeout-2255.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2255_LEGACY TAKO_AUTH_PROBE_TIMEOUT_SECS TAKO_IPC_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
OLD_BIN="${OLD_BIN:-}"
# 修正前が「返らない」と言い切るまで待つ秒数（修正後の上限 10 秒の 3 倍）
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

TMP="$(mktemp -d /tmp/tako-2255-XXXXXX)"
APP_PID=""
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
  stop_isolated_gui "$APP_PID"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "check-health の認証確認の上限の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy ---------------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR/.local/bin" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
: > "$ALL_PIDS"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#2255 の実測用）。API を呼ばず、設定 dir も読まない */
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
static void hang(void) {
  const char *dir = getenv("FAKE_DIR");
  if (dir) {
    char path[1024];
    snprintf(path, sizeof path, "%s/pids/%d", dir, (int)getpid());
    FILE *f = fopen(path, "w");
    if (f) fclose(f);
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
    if (flag("hang-claude")) hang();
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
  "login status")
    if [ -e "$FAKE_DIR/hang-codex" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    echo "Not logged in"; exit 1 ;;
  "mcp list"*) echo '[]' ;;
esac
exit 0
SH
cat > "$TMP/stubs/agy" <<'SH'
#!/bin/sh
case "$*" in
  models)
    if [ -e "$FAKE_DIR/hang-agy" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    printf 'fake-model\tFake Model\n' ;;
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$HOME_DIR/.local/bin:$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR"
}
set_hang() {
  rm -f "$FAKE_DIR"/hang-*
  local name
  for name in "$@"; do : > "$FAKE_DIR/hang-$name"; done
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

# 砂場の env でコマンドを起こし、終わるか LIMIT 秒を過ぎるまで状態で待つ。
#   run_case <名前> <LIMIT 秒> <バイナリ> [VAR=VAL …] -- <引数…>
# 結果: RC（終わらなければ空）・ELAPSED（秒・小数 1 桁）・HUNG（1 = 上限まで返らず、こちらで落とした）。
# 出力は $TMP/out/<名前>.out / .err。標準入力は RUN_STDIN（既定 /dev/null）。
# パイプを組まない（落とすのは起こした 1 プロセス = env が exec した tako だけで済むように）
run_case() {
  local name="$1" limit="$2" bin="$3" t0 pid
  shift 3
  local extra=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do extra+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) ${extra[@]+"${extra[@]}"} "$bin" "$@" < "${RUN_STDIN:-/dev/null}" \
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
    sleep 0.1
  done
  wait "$pid" 2>/dev/null
  local rc=$?
  [ "$HUNG" = 1 ] || RC="$rc"
  ELAPSED="$(awk -v ms=$(($(now_ms) - t0)) 'BEGIN { printf "%.1f", ms / 1000 }')"
  # 偽物の pid を控える（後で「残っていない」を確かめる）
  local f
  for f in "$FAKE_DIR/pids"/*; do
    [ -f "$f" ] && echo "${f##*/}" >> "$ALL_PIDS"
  done
}

# 偽物の pid が全部いなくなるまで待つ（状態で待つ・上限 10 秒）。残った pid を出す
fakes_left() {
  local i f pid left
  for i in $(seq 1 100); do
    left=""
    for f in "$FAKE_DIR/pids"/*; do
      [ -f "$f" ] || continue
      pid="${f##*/}"
      if ps -o stat= -p "$pid" >/dev/null 2>&1; then left="$left $pid"; else rm -f "$f"; fi
    done
    [ -z "$left" ] && return 0
    sleep 0.1
  done
  echo "$left"
}

jf() {
  python3 -I -c '
import json, sys
path = [k for k in sys.argv[1].split("/") if k]
try:
    v = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for k in path:
    if isinstance(v, list) and k.startswith("key="):
        v = next((r for r in v if r.get("key") == k[4:]), None)
    elif isinstance(v, dict):
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
# diagnostics の項目の数（他の項目が出ているか）
items_count() { jf diagnostics/items < "$1" | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0; }
contains() { case "$(cat "$1")" in *"$2"*) return 0 ;; esac; return 1; }

# 修正後の 1 回ぶんの判定（人が読む形 / --json / MCP で共通の観点）
expect_unknown() {
  local label="$1" file="$2" secs="$3"
  if contains "$file" "認証: 不明（${secs} 秒で応答なし）"; then
    pass "$label: 「認証: 不明（${secs} 秒で応答なし）」が出る"
  else
    fail "$label: 「認証: 不明（${secs} 秒で応答なし）」が無い"
  fi
}

# --- 1 / 2 / 4: CLI（GUI に届かない = CLI 自身が環境診断を回す経路） -------------------
phase_cli() {
  echo "== cli: GUI に届かない check-health（CLI が環境診断を回す）"
  use_dirs cli
  # 4: 即答する偽の claude（回帰なし）
  set_hang
  run_case ok-json 60 "$TAKO_BIN" -- check-health --json
  check_eq "即答: 返る" "0" "${HUNG}"
  check_eq "即答: claude は完了（ok）" "ok" "$(jf diagnostics/items/key=agent_cli.claude/status < "$TMP/out/ok-json.out")"
  if contains "$TMP/out/ok-json.out" "claude: ~/.local/bin/claude（認証済み / max）"; then
    pass "即答: 認証済み / max が今まで通り出る（${ELAPSED} 秒）"
  else
    fail "即答: 認証の行が今まで通りでない"
  fi
  check_eq "即答: 打ち切りは 0 件" "[]" "$(jf diagnostics/probe_timeouts < "$TMP/out/ok-json.out")"
  local ok_items
  ok_items="$(items_count "$TMP/out/ok-json.out")"

  # 1: 返らない claude（既定の上限 10 秒・人が読む形）
  set_hang claude
  run_case hang-text 60 "$TAKO_BIN" -- check-health
  check_eq "返らない claude（既定）: check-health が返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 10 秒）"
  expect_unknown "返らない claude（既定・人が読む形）" "$TMP/out/hang-text.out" 10
  grep -n "認証: 不明\|エージェント CLI の導入\|\[検出\] claude" "$TMP/out/hang-text.out" | sed 's/^/      /'
  if contains "$TMP/out/hang-text.err" "[確認できません] claude auth status --json（10 秒応答なし）"; then
    pass "返らない claude（既定）: stderr に打ち切りの知らせ（#1503 の文面）"
  else
    fail "返らない claude（既定）: stderr に打ち切りの知らせが無い"
  fi
  local left
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "返らない claude（既定）: 打ち切った偽の claude は残っていない"; else fail "偽の claude が残っている:$left"; fi

  # 1: --json（上限を注入で 3 秒へ）
  run_case hang-json 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- check-health --json
  check_eq "返らない claude（3 秒・--json）: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 3 秒）"
  expect_unknown "返らない claude（3 秒・--json）" "$TMP/out/hang-json.out" 3
  check_eq "--json: agent_cli.claude は warn" "warn" "$(jf diagnostics/items/key=agent_cli.claude/status < "$TMP/out/hang-json.out")"
  check_eq "--json: probe_timeouts の名札" "claude auth status --json" "$(jf diagnostics/probe_timeouts/0/label < "$TMP/out/hang-json.out")"
  check_eq "--json: probe_timeouts の秒" "3" "$(jf diagnostics/probe_timeouts/0/waited_secs < "$TMP/out/hang-json.out")"
  check_eq "--json: 他の項目も同じ数だけ出る" "$ok_items" "$(items_count "$TMP/out/hang-json.out")"
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "--json: 打ち切った偽の claude は残っていない"; else fail "偽の claude が残っている:$left"; fi

  # エッジ: 3 系統とも返らない → 並べて聞くので全体は上限 1 回ぶん
  set_hang claude codex agy
  run_case hang-all 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- check-health --json
  check_eq "3 系統とも返らない: 返る" "0" "$HUNG"
  check_eq "3 系統とも返らない: 打ち切りは 3 件" "3" \
    "$(jf diagnostics/probe_timeouts < "$TMP/out/hang-all.out" | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin)))')"
  if awk -v e="$ELAPSED" 'BEGIN { exit !(e < 6) }'; then
    pass "3 系統とも返らない: 全体 ${ELAPSED} 秒（1 系統ずつなら 9 秒以上 = 上限 3 秒 × 3）"
  else
    fail "3 系統とも返らない: 全体 ${ELAPSED} 秒（並べて聞けていない）"
  fi
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "3 系統とも返らない: 偽物（claude / codex / agy）は残っていない"; else fail "偽物が残っている:$left"; fi

  # エッジ: 上限の env に 0 を渡しても外れない（既定の 10 秒へ落ちる）
  set_hang claude
  run_case hang-zero 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=0 -- check-health --json
  check_eq "上限の env が 0: 返る（無制限にならない）" "0" "$HUNG"
  check_eq "上限の env が 0: 既定の 10 秒で打ち切る" "10" "$(jf diagnostics/probe_timeouts/0/waited_secs < "$TMP/out/hang-zero.out")"
  fakes_left >/dev/null
}

# --- 1（A/B）: TAKO_2255_LEGACY=1 は返らない --------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2255_LEGACY=1（修正前 = 上限なしで 1 系統ずつ）"
  use_dirs legacy
  set_hang claude
  run_case legacy "$LEGACY_WAIT" "$TAKO_BIN" TAKO_2255_LEGACY=1 -- check-health --json
  check_eq "A/B: ${LEGACY_WAIT} 秒待っても check-health が返らない（こちらで落とした）" "1" "$HUNG"
  if [ -s "$TMP/out/legacy.out" ]; then fail "A/B: 何か出力した"; else pass "A/B: 何も出力しないまま固まっている（stdout 0 バイト）"; fi
  kill_fakes
}

# --- 1（修正前のバイナリ）: OLD_BIN は返らない --------------------------------------------
phase_old() {
  if [ -z "$OLD_BIN" ]; then
    unmeasured "OLD_BIN が無いので修正前のバイナリでは測っていない"
    return
  fi
  echo "== old: 修正前のバイナリ（$("$OLD_BIN" --version 2>/dev/null)）"
  use_dirs old
  set_hang claude
  run_case old "$LEGACY_WAIT" "$OLD_BIN" -- check-health --json
  check_eq "修正前: ${LEGACY_WAIT} 秒待っても check-health が返らない（こちらで落とした）" "1" "$HUNG"
  kill_fakes
  set_hang
  run_case old-ok 60 "$OLD_BIN" -- check-health --json
  check_eq "修正前: 即答する claude なら返る（同じ砂場で測れている）" "0" "$HUNG"
}

# --- 1 / 2 / 4: GUI 経由（CLI が届く経路と MCP） ------------------------------------------
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2255","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-in.jsonl"
  RUN_STDIN="$TMP/mcp-in.jsonl" run_case "$3-raw" "$4" "$TAKO_BIN" \
    TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" -- mcp serve
  # tools/call（id 2）の本文だけを取り出す
  python3 -I -c '
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        for c in msg.get("result", {}).get("content", []):
            if c.get("type") == "text":
                print(c["text"])
        break' "$TMP/out/$3-raw.out" > "$TMP/out/$3.out"
}

start_app() {
  local name="$1"
  shift
  use_dirs "$name"
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_PERSIST=0 ${@+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    unmeasured "GUI を立てられない（終了コード ${rc}。tako-vd が無い / 蓋が閉じている）: GUI 経由と MCP は未実測"
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

phase_gui() {
  echo "== gui: GUI に届く check-health と MCP tako_check_health（GUI の上限は既定の 10 秒）"
  set_hang
  start_app gui || return
  run_case gui-ok 60 "$TAKO_BIN" -- check-health --json
  check_eq "GUI・即答: 届いた（connected 相当 = reason が無い）" "null" "$(jf reason < "$TMP/out/gui-ok.out")"
  if contains "$TMP/out/gui-ok.out" "claude: ~/.local/bin/claude（認証済み / max）"; then
    pass "GUI・即答: 認証済み / max が今まで通り出る"
  else
    fail "GUI・即答: 認証の行が今まで通りでない"
  fi

  set_hang claude
  run_case gui-hang-json 60 "$TAKO_BIN" -- check-health --json
  check_eq "GUI・返らない claude（--json）: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 10 秒）"
  check_eq "GUI・--json: GUI が答えた（届かない扱いではない）" "null" "$(jf reason < "$TMP/out/gui-hang-json.out")"
  expect_unknown "GUI・返らない claude（--json）" "$TMP/out/gui-hang-json.out" 10
  check_eq "GUI・--json: probe_timeouts の名札" "claude auth status --json" "$(jf diagnostics/probe_timeouts/0/label < "$TMP/out/gui-hang-json.out")"
  local left
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "GUI・--json: 打ち切った偽の claude は残っていない（GUI が刈り取った）"; else fail "偽の claude が残っている:$left"; fi

  run_case gui-hang-text 60 "$TAKO_BIN" -- check-health
  check_eq "GUI・返らない claude（人が読む形）: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒"
  expect_unknown "GUI・返らない claude（人が読む形）" "$TMP/out/gui-hang-text.out" 10
  fakes_left >/dev/null

  mcp_call tako_check_health '{}' gui-mcp 60
  check_eq "MCP tako_check_health・返らない claude: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒"
  expect_unknown "MCP tako_check_health・返らない claude" "$TMP/out/gui-mcp.out" 10
  check_eq "MCP: probe_timeouts の名札" "claude auth status --json" "$(jf diagnostics/probe_timeouts/0/label < "$TMP/out/gui-mcp.out")"
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "MCP: 打ち切った偽の claude は残っていない"; else fail "偽の claude が残っている:$left"; fi
  stop_app

  echo "== gui legacy: TAKO_2255_LEGACY=1 の GUI（CLI の応答の上限は注入で 20 秒）"
  set_hang claude
  start_app gui-legacy TAKO_2255_LEGACY=1 || return
  run_case gui-legacy "$LEGACY_WAIT" "$TAKO_BIN" TAKO_2255_LEGACY=1 TAKO_IPC_TIMEOUT_SECS=20 -- check-health --json
  check_eq "GUI・A/B: ${LEGACY_WAIT} 秒待っても返らない（応答の上限 20 秒の後、CLI 自身の診断でも固まる）" "1" "$HUNG"
  mcp_call tako_check_health '{}' gui-legacy-mcp "$LEGACY_WAIT"
  check_eq "MCP・A/B: ${LEGACY_WAIT} 秒待っても返らない" "1" "$HUNG"
  stop_app
}

want cli && phase_cli
want legacy && phase_legacy
want old && phase_old
want gui && phase_gui

# 2: 起こした偽物が 1 つも残っていない（全段の pid を見直す）
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
