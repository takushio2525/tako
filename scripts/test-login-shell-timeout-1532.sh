#!/usr/bin/env bash
# test-login-shell-timeout-1532.sh — #1532 / #1533: ログインシェルを上限なしで待たないことの実経路テスト
#
# 偽のログインシェル（`SHELL`）を置き、印のファイルで rc の振る舞いを切り替えて次を実測する
# （番号は依頼の受け入れ条件）:
#   1. 修正後は上限で打ち切って「確認できない」を返し、他の項目は出る:
#      setup の該当段（`tako setup --check` / `tako setup bootstrap status-all` / `tako setup deps`）・
#      check-health（CLI が自分で診断する経路と、GUI に届く経路 + MCP）・`exe::find` の代表の呼び手
#      （依存の検出・GUI の `which("tako")`）。修正前（`TAKO_1532_LEGACY=1` と OLD_BIN）は固まる
#   2. 打ち切った後に偽のシェルとその孫が残っていない（偽物が自分で書いた pid で確かめる）
#   4. 普通のログインシェルでは今まで通り見つかる（claude の導入・PATH・git）
#   エッジ: 入力を待つ rc（誰も書かない FIFO を読む）も同じく打ち切る / 上限の env に 0 → 既定の 5 秒 /
#   打ち切った依存は `--dry-run` の導入計画に載らない（入れ直さない）
#
# 偽のログインシェルの rc（印のファイル）:
#   rc-sleeps: 孫（sleep 600）を起こして待つ = 眠り続ける rc（直接の子を kill しても孫が残る形）
#   rc-reads : 誰も書かない FIFO を読む = 入力を待つ rc（パスフレーズを聞く ssh-add の代役）
#   印が無ければ普通のログインシェル（PATH の先頭に LOGIN_BIN を足して /bin/sh を exec）
# 偽の claude と tako は LOGIN_BIN にだけ置く = **ログインシェルの PATH からしか見えない**
# （`.app` の痩せた PATH で Homebrew の CLI を探す本番の形と同じ）。
#
# **本物の claude / codex / agy は起こさない**（偽の claude を `cc` で作る。サブコマンドに即答して抜ける）。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery は mktemp 配下。CLI も GUI も砂場
# （一時 HOME・偽物と /usr/bin 等だけの PATH・偽の SHELL）で動かす。落とすのは自分で起こした pid だけ
# （偽物は止まる前に自分の pid を書く）。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=cli|legacy|old|gui（既定は全部。old は OLD_BIN があるときだけ）。
#   OLD_BIN=<修正前の tako CLI>  例: OLD_BIN=/Applications/tako.app/Contents/MacOS/tako（#1532 の修正が入る前の配布物）
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue1533_login_shell_timeout_watchdog.rs`）。
#
# 使い方: bash scripts/test-login-shell-timeout-1532.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1532_LEGACY TAKO_LOGIN_SHELL_TIMEOUT_SECS TAKO_IPC_TIMEOUT_SECS \
  TAKO_SETUP_PROBE_TIMEOUT_SECS TAKO_AUTH_PROBE_TIMEOUT_SECS TAKO_1503_LEGACY TAKO_2255_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CONFIG_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
OLD_BIN="${OLD_BIN:-}"
# 修正前が「返らない」と言い切るまで待つ秒数（修正後の上限 2 秒の 15 倍）
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

TMP="$(mktemp -d /tmp/tako-1532-XXXXXX)"
APP_PID=""
FAKE_DIR="$TMP/fake"
LOGIN_BIN="$TMP/loginbin"
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
tako_exit_trap cleanup "ログインシェルの上限の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽物（ログインシェル・claude・tako） ------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR" "$LOGIN_BIN" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
mkfifo "$FAKE_DIR/tty"
: > "$ALL_PIDS"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#1532 の実測用）。API を呼ばず、設定 dir も読まない */
#include <stdio.h>
#include <string.h>
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
  if (argc > 1 && strcmp(argv[1], "mcp") == 0) {
    printf("No MCP servers configured.\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "agents") == 0) printf("[]\n");
  return 0;
}
C
cc -O1 -o "$LOGIN_BIN/claude" "$TMP/fake-claude.c" || exit 1
# 作りたての実行ファイルの初回 exec の検査を上限の外で済ませる（#2222）
"$LOGIN_BIN/claude" "$FAKE_EXEC_WARM_ARG"
# 偽の tako（GUI の check-health が `which("tako")` で見つけて `--version` を聞く相手）。
# 印 `hang-tako` があるときだけ `--version` で止まる（`exec sleep` = 打ち切りの kill が届く直接の子）
cat > "$LOGIN_BIN/tako" <<'SH'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  if [ -e "$FAKE_DIR/hang-tako" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
  echo "tako 0.0.0-fake"
fi
exit 0
SH
# 偽のログインシェル。呼ばれた引数を控え、印で rc の振る舞いを変えてから /bin/sh へ渡す
cat > "$TMP/stubs/fakesh" <<'SH'
#!/bin/sh
echo "$*" >> "$FAKE_DIR/calls"
if [ -e "$FAKE_DIR/rc-sleeps" ]; then
  /bin/sleep 600 &
  : > "$FAKE_DIR/pids/$!"
  : > "$FAKE_DIR/pids/$$"
  wait
fi
if [ -e "$FAKE_DIR/rc-reads" ]; then
  : > "$FAKE_DIR/pids/$$"
  read line < "$FAKE_DIR/tty"
fi
if [ "${1:-}" = "-l" ]; then shift; fi
PATH="$LOGIN_BIN:$PATH"
export PATH
exec /bin/sh "$@"
SH
fake_exec_warm "$LOGIN_BIN/tako" "$TMP/stubs/fakesh" || exit 1
SANDBOX_PATH="$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/fakesh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR" "LOGIN_BIN=$LOGIN_BIN"
}
# rc の振る舞いを決める（引数の印だけを置く。無ければ普通のログインシェル）
set_rc() {
  rm -f "$FAKE_DIR"/rc-* "$FAKE_DIR"/hang-*
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

# 砂場の env でコマンドを起こし、終わるか LIMIT 秒を過ぎるまで状態で待つ。
#   run_case <名前> <LIMIT 秒> <バイナリ> [VAR=VAL …] -- <引数…>
# 結果: RC（終わらなければ空）・ELAPSED（秒・小数 1 桁）・HUNG（1 = 上限まで返らず、こちらで落とした）。
# 出力は $TMP/out/<名前>.out / .err。標準入力は RUN_STDIN（既定 /dev/null）
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
expect_reaped() {
  local left
  left="$(fakes_left)"
  if [ -z "$left" ]; then
    pass "$1: 打ち切った偽のシェルと孫は残っていない"
  else
    fail "$1: 偽物が残っている:$left"
  fi
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
    elif isinstance(v, list) and k.startswith("agent="):
        v = next((r for r in v if r.get("agent") == k[6:]), None)
    elif isinstance(v, list) and k.startswith("bin="):
        v = next((r for r in v if r.get("bin") == k[4:]), None)
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
items_count() { jf diagnostics/items < "$1" | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0; }
labels_of() { jf diagnostics/probe_timeouts < "$1" | python3 -I -c 'import json,sys; print(" | ".join(sorted(t["label"] for t in json.load(sys.stdin))))' 2>/dev/null; }
contains() { case "$(cat "$1")" in *"$2"*) return 0 ;; esac; return 1; }
expect_text() {
  if contains "$2" "$3"; then pass "${1}: 「${3}」"; else fail "${1}: 「${3}」が無い"; fi
}
expect_no_text() {
  if contains "$2" "$3"; then fail "${1}: 「${3}」が出ている"; else pass "${1}: 「${3}」は出ない"; fi
}
# 普通のときに在った項目のうち、$2 で欠けたもののキー（空白区切り）
missing_keys() {
  python3 -I -c '
import json, sys
def keys(path):
    try:
        d = json.load(open(path, encoding="utf-8"))
    except Exception:
        return []
    return [i["key"] for i in d.get("diagnostics", {}).get("items", [])]
ok, got = keys(sys.argv[1]), set(keys(sys.argv[2]))
print(" ".join(k for k in ok if k not in got))' "$1" "$2"
}
# 他の項目が出ているか。欠けてよいのは claude が入っているときだけ出す項目（MCP 登録・指示ファイル）だけ
# （claude を確かめられないので、その先は聞かない。頭の項目が「不明」と言っている）
expect_other_items() {
  local gone k bad=""
  gone="$(missing_keys "$2" "$3")"
  for k in $gone; do
    case "$k" in
      mcp.claude | instructions.claude) ;;
      *) bad="$bad $k" ;;
    esac
  done
  if [ -z "$bad" ]; then
    pass "${1}: 他の項目は出る（普通のとき $(items_count "$2") 件 → $(items_count "$3") 件。欠けたのは「${gone:-なし}」= claude の導入を確かめられないので、その先は聞かない）"
  else
    fail "${1}: 項目が欠けた:${bad}"
  fi
}
calls_count() { wc -l < "$FAKE_DIR/calls" 2>/dev/null | tr -d ' ' || echo 0; }

# --- 1 / 2 / 4: CLI（GUI に届かない = CLI 自身が診断する経路・setup の段） ----------------
phase_cli() {
  echo "== cli: setup の段と、GUI に届かない check-health（上限 2 秒）"
  use_dirs cli
  local LS=TAKO_LOGIN_SHELL_TIMEOUT_SECS=2

  # 4: 普通のログインシェル（回帰なし）
  set_rc
  run_case ok-health 60 "$TAKO_BIN" "$LS" -- check-health --json
  check_eq "普通: check-health が返る" "0" "$HUNG"
  check_eq "普通: claude はログインシェルの PATH で見つかり完了（ok）" "ok" \
    "$(jf diagnostics/items/key=agent_cli.claude/status < "$TMP/out/ok-health.out")"
  check_eq "普通: git はログインシェルで見つかる" "ok" "$(jf diagnostics/items/key=dep.git/status < "$TMP/out/ok-health.out")"
  check_eq "普通: 打ち切りは 0 件" "[]" "$(jf diagnostics/probe_timeouts < "$TMP/out/ok-health.out")"
  local ok_items
  ok_items="$(items_count "$TMP/out/ok-health.out")"
  echo "    項目 ${ok_items} 件 / 経過 ${ELAPSED} 秒 / ログインシェル $(calls_count) 回"
  run_case ok-status 60 "$TAKO_BIN" "$LS" -- setup bootstrap status-all --json
  check_eq "普通: bootstrap status-all の claude は導入済み" "true" "$(jf agents/agent=claude/installed < "$TMP/out/ok-status.out")"
  check_eq "普通: claude の PATH は通っている（ログインシェルが見つけた）" "true" \
    "$(jf agents/agent=claude/launcher_dir_on_path < "$TMP/out/ok-status.out")"
  check_eq "普通: 段は確かめた答え（next_step_unconfirmed は null）" "null" \
    "$(jf agents/agent=claude/next_step_unconfirmed < "$TMP/out/ok-status.out")"
  run_case ok-deps 60 "$TAKO_BIN" "$LS" -- setup deps --json
  check_eq "普通: setup deps の git は /usr/bin/git" "/usr/bin/git" "$(jf deps/bin=git/found < "$TMP/out/ok-deps.out")"

  # 1 / 2: 眠り続ける rc（孫を起こして待つ）
  set_rc rc-sleeps
  : > "$FAKE_DIR/calls"
  run_case sleeps-health 60 "$TAKO_BIN" "$LS" -- check-health --json
  check_eq "眠る rc: check-health が返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 2 秒 × ログインシェル $(calls_count) 回ぶんの問い合わせ）"
  check_eq "眠る rc: claude は warn（未導入の missing ではない）" "warn" \
    "$(jf diagnostics/items/key=agent_cli.claude/status < "$TMP/out/sleeps-health.out")"
  expect_text "眠る rc" "$TMP/out/sleeps-health.out" "claude: 導入: 不明（ログインシェルが 2 秒で応答なし）"
  check_eq "眠る rc: 検出の一覧は warn で残り作業なし" "warn|[]" \
    "$(jf diagnostics/items/key=agents_detected/status < "$TMP/out/sleeps-health.out")|$(jf diagnostics/items/key=agents_detected/remaining < "$TMP/out/sleeps-health.out")"
  expect_no_text "眠る rc" "$TMP/out/sleeps-health.out" "いずれも見つかりません"
  expect_text "眠る rc（依存）" "$TMP/out/sleeps-health.out" "git: 確認できません（ログインシェルが 2 秒で応答なし）"
  expect_text "眠る rc（tako CLI の PATH）" "$TMP/out/sleeps-health.out" "tako CLI の PATH: 確認できません（ログインシェルが 2 秒で応答なし）"
  expect_other_items "眠る rc" "$TMP/out/ok-health.out" "$TMP/out/sleeps-health.out"
  echo "    probe_timeouts: $(labels_of "$TMP/out/sleeps-health.out")"
  if labels="$(labels_of "$TMP/out/sleeps-health.out")" && grep -q "fakesh -l -c command -v claude" <<<"$labels"; then
    pass "眠る rc: probe_timeouts に何を待ったか（fakesh -l -c command -v claude）が載る"
  else
    fail "眠る rc: probe_timeouts に claude の検出が無い"
  fi
  expect_reaped "眠る rc（check-health）"

  run_case sleeps-check 60 "$TAKO_BIN" "$LS" -- setup --check
  check_eq "眠る rc: setup --check が返る" "0" "$HUNG"
  expect_text "眠る rc（setup --check）" "$TMP/out/sleeps-check.err" "[警告] claude: 導入: 不明（ログインシェルが 2 秒で応答なし）"
  expect_no_text "眠る rc（setup --check）" "$TMP/out/sleeps-check.err" "インストール"
  expect_reaped "眠る rc（setup --check）"

  run_case sleeps-status 60 "$TAKO_BIN" "$LS" -- setup bootstrap status-all --json
  check_eq "眠る rc: bootstrap status-all が返る" "0" "$HUNG"
  check_eq "眠る rc: claude の段は打ち切りで決まった（2 秒）" "2" \
    "$(jf agents/agent=claude/next_step_unconfirmed/waited_secs < "$TMP/out/sleeps-status.out")"
  run_case sleeps-status-human 60 "$TAKO_BIN" "$LS" -- setup bootstrap status-all
  expect_text "眠る rc（status-all の人向け）" "$TMP/out/sleeps-status-human.err" "[警告] claude: 確認できません（ログインシェルが 2 秒で応答なし）"
  run_case sleeps-deps 60 "$TAKO_BIN" "$LS" -- setup deps --json
  check_eq "眠る rc: setup deps の git は打ち切り（2 秒）" "2" "$(jf deps/bin=git/lookup_timeout/waited_secs < "$TMP/out/sleeps-deps.out")"
  run_case sleeps-plan 60 "$TAKO_BIN" "$LS" -- setup deps install --dry-run --json
  check_eq "眠る rc: 打ち切った git は導入計画に載らず unconfirmed で飛ばす" "unconfirmed" \
    "$(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(next((s["reason"] for s in d.get("skipped", []) if s.get("bin") == "git"), "absent"))' "$TMP/out/sleeps-plan.out" 2>/dev/null)"
  expect_reaped "眠る rc（setup の段）"
  if grep -q "実行ファイルの検出（#1533）" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
    pass "眠る rc: Option しか返さない exe::find の打ち切りは persist.log に残る"
    grep "実行ファイルの検出（#1533）" "$TAKO_DATA_DIR/persist.log" | head -1 | cut -c1-200 | sed 's/^/      /'
  else
    fail "眠る rc: persist.log に exe::find の打ち切りが無い"
  fi

  # エッジ: 入力を待つ rc（誰も書かない FIFO）
  set_rc rc-reads
  run_case reads-status 60 "$TAKO_BIN" "$LS" -- setup bootstrap status --agent claude --json
  check_eq "入力待ちの rc: 返る" "0" "$HUNG"
  check_eq "入力待ちの rc: claude の段は打ち切りで決まった（2 秒）" "2" \
    "$(jf next_step_unconfirmed/waited_secs < "$TMP/out/reads-status.out")"
  expect_reaped "入力待ちの rc"

  # エッジ: 上限の env に 0 を渡しても外れない（既定の 5 秒へ落ちる）
  set_rc rc-sleeps
  run_case zero-status 60 "$TAKO_BIN" TAKO_LOGIN_SHELL_TIMEOUT_SECS=0 -- setup bootstrap status --agent claude --json
  check_eq "上限の env が 0: 返る（無制限にならない）" "0" "$HUNG"
  check_eq "上限の env が 0: 既定の 5 秒で打ち切る" "5" "$(jf next_step_unconfirmed/waited_secs < "$TMP/out/zero-status.out")"
  echo "    経過 ${ELAPSED} 秒"
  expect_reaped "上限の env が 0"
  set_rc
}

# --- 1（A/B）: TAKO_1532_LEGACY=1 は返らない --------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_1532_LEGACY=1（修正前 = ログインシェルを上限なしで待つ）"
  use_dirs legacy
  set_rc rc-sleeps
  local args name
  for args in "check-health --json" "setup bootstrap status --agent claude --json" "setup deps --json"; do
    name="legacy-$(echo "$args" | tr ' -' '__')"
    # shellcheck disable=SC2086
    run_case "$name" "$LEGACY_WAIT" "$TAKO_BIN" TAKO_1532_LEGACY=1 TAKO_LOGIN_SHELL_TIMEOUT_SECS=2 -- $args
    check_eq "A/B: tako ${args} は ${LEGACY_WAIT} 秒待っても返らない（こちらで落とした）" "1" "$HUNG"
    if [ -s "$TMP/out/$name.out" ]; then fail "A/B: tako ${args} が何か出力した"; else pass "A/B: tako ${args} は何も出さないまま固まっている（stdout 0 バイト）"; fi
    kill_fakes
  done
  set_rc
}

# --- 1（修正前のバイナリ）: OLD_BIN は返らない --------------------------------------------
phase_old() {
  if [ -z "$OLD_BIN" ]; then
    unmeasured "OLD_BIN が無いので修正前のバイナリでは測っていない"
    return
  fi
  echo "== old: 修正前のバイナリ（$("$OLD_BIN" --version 2>/dev/null)）"
  use_dirs old
  set_rc rc-sleeps
  run_case old-health "$LEGACY_WAIT" "$OLD_BIN" -- check-health --json
  check_eq "修正前: check-health は ${LEGACY_WAIT} 秒待っても返らない（こちらで落とした）" "1" "$HUNG"
  kill_fakes
  run_case old-check "$LEGACY_WAIT" "$OLD_BIN" -- setup --check
  check_eq "修正前: setup --check は ${LEGACY_WAIT} 秒待っても返らない" "1" "$HUNG"
  kill_fakes
  set_rc
  run_case old-ok 60 "$OLD_BIN" -- check-health --json
  check_eq "修正前: 普通のログインシェルなら返る（同じ砂場で測れている）" "0" "$HUNG"
}

# --- 1 / 2: GUI 経由（finish_check_health の tako 検出と tako --version・MCP） --------------
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1532","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-in.jsonl"
  RUN_STDIN="$TMP/mcp-in.jsonl" run_case "$3-raw" "$4" "$TAKO_BIN" \
    TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" -- mcp serve
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
  echo "== gui: GUI に届く check-health と MCP tako_check_health（GUI のログインシェルの上限 2 秒・probe 3 秒）"
  set_rc
  start_app gui TAKO_LOGIN_SHELL_TIMEOUT_SECS=2 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 || return
  run_case gui-ok 90 "$TAKO_BIN" -- check-health --json
  check_eq "GUI・普通: 届いた（reason が無い）" "null" "$(jf reason < "$TMP/out/gui-ok.out")"
  check_eq "GUI・普通: tako CLI はログインシェルの PATH で見つかる" "true" "$(jf cli_in_path < "$TMP/out/gui-ok.out")"
  check_eq "GUI・普通: tako --version を読めた" "0.0.0-fake" "$(jf cli_version < "$TMP/out/gui-ok.out")"
  check_eq "GUI・普通: 打ち切りの印は null" "null|null" \
    "$(jf cli_in_path_unconfirmed < "$TMP/out/gui-ok.out")|$(jf cli_version_unconfirmed < "$TMP/out/gui-ok.out")"

  # 1 / 2: 眠り続ける rc（tako の検出が打ち切られる）
  set_rc rc-sleeps
  run_case gui-sleeps 90 "$TAKO_BIN" -- check-health --json
  check_eq "GUI・眠る rc: check-health が返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒"
  check_eq "GUI・眠る rc: tako CLI の検出は打ち切り（2 秒）" "2" "$(jf cli_in_path_unconfirmed/waited_secs < "$TMP/out/gui-sleeps.out")"
  expect_text "GUI・眠る rc" "$TMP/out/gui-sleeps.out" "tako CLI が PATH に在るかを確かめられない（ログインシェルが 2 秒で応答しない）"
  expect_no_text "GUI・眠る rc" "$TMP/out/gui-sleeps.out" "tako CLI が PATH に見つからない"
  expect_other_items "GUI・眠る rc" "$TMP/out/gui-ok.out" "$TMP/out/gui-sleeps.out"
  expect_reaped "GUI・眠る rc"

  # 1 / 2: PATH の先の tako が --version で返らない（check-health の経路の 3 か所目）
  set_rc hang-tako
  run_case gui-hang-tako 90 "$TAKO_BIN" -- check-health --json
  check_eq "GUI・tako --version が返らない: check-health が返る" "0" "$HUNG"
  check_eq "GUI・tako --version が返らない: 打ち切り（3 秒）" "3" "$(jf cli_version_unconfirmed/waited_secs < "$TMP/out/gui-hang-tako.out")"
  expect_text "GUI・tako --version が返らない" "$TMP/out/gui-hang-tako.out" "CLI バージョンを確かめられない"
  expect_reaped "GUI・tako --version が返らない"

  # MCP（同じ dispatch）
  set_rc rc-sleeps
  mcp_call tako_check_health '{}' gui-mcp 90
  check_eq "MCP・眠る rc: tako_check_health が返る" "0" "$HUNG"
  check_eq "MCP・眠る rc: cli_in_path_unconfirmed が載る（2 秒）" "2" "$(jf cli_in_path_unconfirmed/waited_secs < "$TMP/out/gui-mcp.out")"
  expect_reaped "MCP・眠る rc"
  set_rc
  stop_app

  # A/B: 修正前の GUI は返らない
  start_app gui-legacy TAKO_1532_LEGACY=1 || return
  set_rc rc-sleeps
  run_case gui-legacy "$LEGACY_WAIT" "$TAKO_BIN" -- check-health --json
  check_eq "GUI・A/B: TAKO_1532_LEGACY=1 の check-health は ${LEGACY_WAIT} 秒待っても返らない" "1" "$HUNG"
  set_rc
  stop_app
}

want cli && phase_cli
want legacy && phase_legacy
want old && phase_old
want gui && phase_gui

echo
echo "結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
