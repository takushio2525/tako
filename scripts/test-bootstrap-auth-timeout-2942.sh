#!/usr/bin/env bash
# test-bootstrap-auth-timeout-2942.sh — #2942: `tako setup bootstrap status` が返らない claude で固まらないことの実経路テスト
#
# `auth status` に返らない偽の claude（孫を 1 つ抱えて止まる = Windows 実機の `claude.cmd` が `ping` を
# 抱えた形）を置き、次を実測する（番号は Issue #2942 の受け入れ条件）:
#   1. 修正後は上限の秒数で理由つきで返る（CLI の人が読む形・`--json`・`status-all`・`tako setup` の検出と
#      導入の段・MCP `tako_setup_bootstrap`）。修正前（`TAKO_2942_LEGACY=1` と、OLD_BIN に渡した修正前の
#      バイナリ）は上限まで返らない。打ち切った後に偽物の子も孫も残っていない（偽物が自分で書いた pid で見る）
#   3. 返る claude（ログイン済み / 未ログイン）では修正前後で判定が同じ
#   エッジ: 3 系統とも返らなくても `status-all` の全体は上限 1 回ぶん（並べて聞く）/ 上限の env に 0 を
#   渡しても外れない（既定の 10 秒へ落ちる）
#
# **本物の claude / codex / agy は起こさない**: 偽の claude を `cc` で作る（印のファイル `hang-claude` が
# あるときだけ `auth` で孫を作って止まる。`unauth-claude` なら未ログインを返す）。codex / agy はシェルの偽物
# （`hang-codex` / `hang-agy` で止まる）。偽物を置いた PATH で `command -v claude` が偽物を指すことを最初に確かめる。
# **本番の tako にも利用者の CLI にも触らない**: data / discovery / HOME は mktemp 配下。CLI も GUI も砂場
# （一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL・brew は偽物 = #1503 / #2255 のスクリプトと
# 同じ作り）で動かす。落とすのは自分で起こした pid だけ。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=cli|setup|legacy|old|gui（既定は全部。old は OLD_BIN があるときだけ）。
#   OLD_BIN=<修正前の tako CLI>  例: OLD_BIN=/Applications/tako.app/Contents/MacOS/tako（#2942 の修正が入る前の配布物）
# CI には登録しない（GUI と cc が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue2942_bootstrap_auth_probe_watchdog.rs`）。
#
# 使い方: bash scripts/test-bootstrap-auth-timeout-2942.sh
set -uo pipefail
# 人が読む出力は表示言語に従う（#2365）。判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2942_LEGACY TAKO_2255_LEGACY TAKO_AUTH_PROBE_TIMEOUT_SECS \
  TAKO_IPC_TIMEOUT_SECS CLAUDE_CONFIG_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
OLD_BIN="${OLD_BIN:-}"
# 修正前が「返らない」と言い切るまで待つ秒数（修正後の既定の上限 10 秒の 3 倍）
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

TMP="$(mktemp -d /tmp/tako-2942-XXXXXX)"
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
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "bootstrap の認証確認の上限の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude / codex / agy / brew ----------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR/.local/bin" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
: > "$ALL_PIDS"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#2942 の実測用）。API を呼ばず、設定 dir も読まない */
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
    printf("2.1.294 (Claude Code)\n");
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "auth") == 0) {
    if (flag("hang-claude")) hang();
    if (flag("unauth-claude")) {
      printf("{\"loggedIn\":false}\n");
      return 1;
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
    echo "Please sign in to view available models." >&2; exit 1 ;;
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
# 依存の導入（`tako setup --yes`）が本物の brew を起こさないための偽物（#1503 のスクリプトと同じ）
printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$TMP/stubs/brew"
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
chmod +x "$TMP/stubs"/*
fake_exec_warm "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/isosh" "$TMP/stubs/brew" || exit 1
SANDBOX_PATH="$HOME_DIR/.local/bin:$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ）
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$HOME_DIR/.codex" "TAKO_ISOLATED=1" \
    "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR"
}
# 砂場の PATH で `claude` が偽物を指すこと（本物を起こさない前提）
resolved="$(env -i PATH="$SANDBOX_PATH" /bin/sh -c 'command -v claude')"
check_eq "砂場の PATH の claude は偽物" "$HOME_DIR/.local/bin/claude" "$resolved"
[ "$resolved" = "$HOME_DIR/.local/bin/claude" ] || exit 1

set_flags() {
  rm -f "${FAKE_DIR:?}"/hang-* "${FAKE_DIR:?}"/unauth-*
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
# 止まった偽物（子と孫）の pid が記録されたか（記録が 0 だと「残っていない」が空振り）
fakes_recorded() {
  local f n=0
  for f in "$FAKE_DIR/pids"/*; do [ -f "$f" ] && n=$((n + 1)); done
  echo "$n"
}
expect_no_fakes() {
  local label="$1" left
  left="$(fakes_left)"
  if [ -z "$left" ]; then pass "$label: 打ち切った偽物（子と孫）は残っていない"; else fail "$label: 偽物が残っている:$left"; fi
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
    if isinstance(v, list) and k.startswith("agent="):
        v = next((r for r in v if r.get("agent") == k[6:]), None)
    elif isinstance(v, dict):
        if k not in v:
            print("absent"); sys.exit()
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
# 判定の要点（修正前後で比べる欄）
verdict() { echo "$(jf authenticated < "$1")/$(jf account_plan < "$1")/$(jf next_step < "$1")"; }

# --- 1 / 3: CLI（`tako setup bootstrap`） ---------------------------------------------
phase_cli() {
  echo "== cli: tako setup bootstrap status / --json / status-all"
  use_dirs cli

  # 3: 返る claude（ログイン済み）
  set_flags
  run_case ok 60 "$TAKO_BIN" -- setup bootstrap status --json
  check_eq "ログイン済み: 返る" "0" "$HUNG"
  check_eq "ログイン済み: 判定（認証 / プラン / 段）" "true/max/ready" "$(verdict "$TMP/out/ok.out")"
  check_eq "ログイン済み: auth_unconfirmed は null" "null" "$(jf auth_unconfirmed < "$TMP/out/ok.out")"
  run_case ok-text 60 "$TAKO_BIN" -- setup bootstrap status
  if contains "$TMP/out/ok-text.err" "認証:   済み"; then pass "ログイン済み（人が読む形）: 「認証: 済み」"; else fail "ログイン済み（人が読む形）: 「認証: 済み」が無い"; fi

  # 3: 返る claude（未ログイン）
  set_flags unauth-claude
  run_case unauth 60 "$TAKO_BIN" -- setup bootstrap status --json
  check_eq "未ログイン: 判定（認証 / プラン / 段）" "false/null/auth" "$(verdict "$TMP/out/unauth.out")"
  check_eq "未ログイン: auth_unconfirmed は null（確かめた答え）" "null" "$(jf auth_unconfirmed < "$TMP/out/unauth.out")"
  run_case unauth-text 60 "$TAKO_BIN" -- setup bootstrap status
  if contains "$TMP/out/unauth-text.err" "認証:   未ログイン"; then pass "未ログイン（人が読む形）: 「認証: 未ログイン」のまま"; else fail "未ログイン（人が読む形）: 「認証: 未ログイン」が無い"; fi

  # 1: 返らない claude（既定の上限 10 秒・人が読む形）
  set_flags hang-claude
  run_case hang-text 60 "$TAKO_BIN" -- setup bootstrap status --agent claude
  check_eq "返らない claude（既定・人が読む形）: 返る" "0" "$HUNG"
  check_eq "返らない claude（既定・人が読む形）: 終了コード 0" "0" "$RC"
  echo "    経過 ${ELAPSED} 秒（上限 10 秒）"
  sed 's/^/      /' "$TMP/out/hang-text.err"
  if contains "$TMP/out/hang-text.err" "[確認できません] claude auth status --json（10 秒応答なし）。打ち切って次へ進みます"; then
    pass "返らない claude（既定）: stderr に打ち切りの知らせ（#1503 / setup --check と同じ文面）"
  else
    fail "返らない claude（既定）: stderr に打ち切りの知らせが無い"
  fi
  if contains "$TMP/out/hang-text.err" "[警告] claude: 認証: 不明（10 秒で応答なし）"; then
    pass "返らない claude（既定）: 「認証: 不明（10 秒で応答なし）」（check-health と同じ文面）"
  else
    fail "返らない claude（既定）: 「認証: 不明」の行が無い"
  fi
  if contains "$TMP/out/hang-text.err" "未ログイン"; then fail "返らない claude: 「未ログイン」と言っている"; else pass "返らない claude: 「未ログイン」とは言わない"; fi
  check_eq "返らない claude: 偽物の子と孫の pid を記録した（2 個）" "2" "$(fakes_recorded)"
  expect_no_fakes "返らない claude（既定）"

  # 1: --json（上限を 3 秒へ）
  run_case hang-json 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- setup bootstrap status --agent claude --json
  check_eq "返らない claude（3 秒・--json）: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 3 秒）"
  if awk -v e="$ELAPSED" 'BEGIN { exit !(e >= 3 && e < 8) }'; then pass "--json: 上限の 3 秒で返る（${ELAPSED} 秒）"; else fail "--json: 経過 ${ELAPSED} 秒（上限 3 秒）"; fi
  check_eq "--json: auth_unconfirmed の名札" "claude auth status --json" "$(jf auth_unconfirmed/label < "$TMP/out/hang-json.out")"
  check_eq "--json: auth_unconfirmed の秒" "3" "$(jf auth_unconfirmed/waited_secs < "$TMP/out/hang-json.out")"
  check_eq "--json: authenticated は false（確かめられなかった）" "false" "$(jf authenticated < "$TMP/out/hang-json.out")"
  check_eq "--json: 段は auth（next_step_unconfirmed はログインシェル用で null）" "auth/null" \
    "$(jf next_step < "$TMP/out/hang-json.out")/$(jf next_step_unconfirmed < "$TMP/out/hang-json.out")"
  echo "    要点: $(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps({k: d.get(k) for k in ("agent","authenticated","next_step","auth_unconfirmed")}, ensure_ascii=False))' "$TMP/out/hang-json.out")"
  expect_no_fakes "--json"

  # エッジ: 3 系統とも返らない → status-all は並べて聞くので全体は上限 1 回ぶん
  set_flags hang-claude hang-codex hang-agy
  run_case hang-all 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- setup bootstrap status-all --json
  check_eq "3 系統とも返らない（status-all）: 返る" "0" "$HUNG"
  local a n=0
  for a in claude codex agy; do
    [ "$(jf "agents/agent=$a/auth_unconfirmed/waited_secs" < "$TMP/out/hang-all.out")" = "3" ] && n=$((n + 1))
  done
  check_eq "3 系統とも返らない: 3 系統とも auth_unconfirmed（3 秒）" "3" "$n"
  if awk -v e="$ELAPSED" 'BEGIN { exit !(e < 6) }'; then
    pass "3 系統とも返らない: 全体 ${ELAPSED} 秒（1 系統ずつなら 9 秒以上 = 上限 3 秒 × 3）"
  else
    fail "3 系統とも返らない: 全体 ${ELAPSED} 秒（並べて聞けていない）"
  fi
  expect_no_fakes "3 系統とも返らない"
  run_case hang-all-text 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- setup bootstrap status-all
  for a in claude codex agy; do
    if contains "$TMP/out/hang-all-text.err" "[警告] $a: 認証: 不明（3 秒で応答なし）"; then
      pass "status-all（人が読む形）: $a は「認証: 不明（3 秒で応答なし）」"
    else
      fail "status-all（人が読む形）: $a の「認証: 不明」の行が無い"
    fi
  done
  expect_no_fakes "status-all（人が読む形）"

  # エッジ: 上限の env に 0 を渡しても外れない（既定の 10 秒へ落ちる）
  set_flags hang-claude
  run_case hang-zero 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=0 -- setup bootstrap status --json
  check_eq "上限の env が 0: 返る（無制限にならない）" "0" "$HUNG"
  check_eq "上限の env が 0: 既定の 10 秒で打ち切る" "10" "$(jf auth_unconfirmed/waited_secs < "$TMP/out/hang-zero.out")"
  expect_no_fakes "上限の env が 0"
}

# --- 1: `tako setup` の検出（[検出] の行）と導入の段 -------------------------------------
phase_setup() {
  echo "== setup: tako setup の検出と導入の段（上限 3 秒）"
  use_dirs setup
  set_flags hang-claude
  run_case setup 120 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- setup --yes
  check_eq "tako setup: 返る" "0" "$HUNG"
  check_eq "tako setup: 終了コード 0（詰まっても完走 = #1501）" "0" "$RC"
  echo "    経過 ${ELAPSED} 秒"
  grep -n "検出\] claude\|認証: 不明\|確認できません\] claude\|ログイン\|導入状況の確認" "$TMP/out/setup.err" | sed 's/^/      /'
  if contains "$TMP/out/setup.err" "（認証: 不明（3 秒で応答なし） / プラン不明）"; then
    pass "tako setup: [検出] の行は「認証: 不明（3 秒で応答なし）」（未認証と言わない）"
  else
    fail "tako setup: [検出] の行に「認証: 不明」が無い"
  fi
  if contains "$TMP/out/setup.err" "[警告] claude: 認証: 不明（3 秒で応答なし）"; then
    pass "tako setup: 導入の段は「認証: 不明」と言って先へ進む"
  else
    fail "tako setup: 導入の段の「認証: 不明」の行が無い"
  fi
  if contains "$TMP/out/setup.err" "claude auth login"; then
    fail "tako setup: 確かめられなかったのにログインを案内した"
  else
    pass "tako setup: ログインへ人を送らない（claude auth login を案内しない）"
  fi
  if contains "$TMP/out/setup.err" "エージェント CLI の導入状況の確認"; then
    pass "tako setup: 残り作業は「エージェント CLI の導入状況の確認」"
  else
    fail "tako setup: 残り作業に導入状況の確認が無い"
  fi
  expect_no_fakes "tako setup"
}

# --- 1（A/B）: TAKO_2942_LEGACY=1 は返らない --------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_2942_LEGACY=1（修正前 = 上限なしで待つ）"
  use_dirs legacy
  set_flags hang-claude
  run_case legacy "$LEGACY_WAIT" "$TAKO_BIN" TAKO_2942_LEGACY=1 -- setup bootstrap status --agent claude
  check_eq "A/B: ${LEGACY_WAIT} 秒待っても bootstrap status が返らない（こちらで落とした）" "1" "$HUNG"
  if [ -s "$TMP/out/legacy.out" ] || [ -s "$TMP/out/legacy.err" ]; then
    fail "A/B: 何か出力した"
  else
    pass "A/B: 何も出さないまま固まっている（stdout / stderr とも 0 バイト = Issue の症状）"
  fi
  kill_fakes
  # 3: 返る claude では A/B でも判定が同じ
  set_flags
  run_case legacy-ok 60 "$TAKO_BIN" TAKO_2942_LEGACY=1 -- setup bootstrap status --json
  check_eq "A/B・ログイン済み: 判定が修正後と同じ" "true/max/ready" "$(verdict "$TMP/out/legacy-ok.out")"
  set_flags unauth-claude
  run_case legacy-unauth 60 "$TAKO_BIN" TAKO_2942_LEGACY=1 -- setup bootstrap status --json
  check_eq "A/B・未ログイン: 判定が修正後と同じ" "false/null/auth" "$(verdict "$TMP/out/legacy-unauth.out")"

  # 孫の A/B: 環境診断（#2255）はどちらでも上限つきなので、打ち切った後に孫が残るかだけが違う
  # （修正前 = 親のグループで起こし直接の子だけを kill / 修正後 = グループごと畳む）
  echo "== 孫の A/B: check-health（上限 3 秒）で打ち切った偽の claude の孫"
  set_flags hang-claude
  run_case grand-new 60 "$TAKO_BIN" TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- check-health --json
  check_eq "孫の A/B（修正後）: check-health が返る" "0" "$HUNG"
  check_eq "孫の A/B（修正後）: 偽物の子と孫の pid を記録した（2 個）" "2" "$(fakes_recorded)"
  expect_no_fakes "孫の A/B（修正後）"
  run_case grand-old 60 "$TAKO_BIN" TAKO_2942_LEGACY=1 TAKO_AUTH_PROBE_TIMEOUT_SECS=3 -- check-health --json
  check_eq "孫の A/B（TAKO_2942_LEGACY=1）: check-health が返る" "0" "$HUNG"
  local left
  left="$(fakes_left)"
  if [ -n "$left" ]; then
    pass "孫の A/B（TAKO_2942_LEGACY=1）: 修正前は孫が残る（残った pid:${left}）= 修正後との違いが測れている"
  else
    fail "孫の A/B（TAKO_2942_LEGACY=1）: 修正前でも孫が残らない（A/B が効いていない）"
  fi
  kill_fakes
}

# --- 1 / 3（修正前のバイナリ）: OLD_BIN は返らない ----------------------------------------
phase_old() {
  if [ -z "$OLD_BIN" ]; then
    unmeasured "OLD_BIN が無いので修正前のバイナリでは測っていない"
    return
  fi
  echo "== old: 修正前のバイナリ（$("$OLD_BIN" --version 2>/dev/null)）"
  use_dirs old
  set_flags hang-claude
  run_case old "$LEGACY_WAIT" "$OLD_BIN" -- setup bootstrap status --agent claude
  check_eq "修正前: ${LEGACY_WAIT} 秒待っても bootstrap status が返らない（こちらで落とした）" "1" "$HUNG"
  if [ -s "$TMP/out/old.out" ] || [ -s "$TMP/out/old.err" ]; then fail "修正前: 何か出力した"; else pass "修正前: 何も出さないまま固まっている"; fi
  kill_fakes
  set_flags
  run_case old-ok 60 "$OLD_BIN" -- setup bootstrap status --json
  check_eq "修正前・ログイン済み: 判定が修正後と同じ" "true/max/ready" "$(verdict "$TMP/out/old-ok.out")"
  set_flags unauth-claude
  run_case old-unauth 60 "$OLD_BIN" -- setup bootstrap status --json
  check_eq "修正前・未ログイン: 判定が修正後と同じ" "false/null/auth" "$(verdict "$TMP/out/old-unauth.out")"
}

# --- 1: MCP tako_setup_bootstrap（GUI 経由） -------------------------------------------
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2942","version":"0"}}}' \
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
    unmeasured "GUI を立てられない（終了コード ${rc}。tako-vd が無い / 蓋が閉じている）: MCP は未実測"
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
  echo "== gui: MCP tako_setup_bootstrap（GUI の上限は注入で 3 秒）"
  set_flags
  start_app gui TAKO_AUTH_PROBE_TIMEOUT_SECS=3 || return
  mcp_call tako_setup_bootstrap '{"action":"status"}' gui-ok 60
  check_eq "MCP・ログイン済み: 返る" "0" "$HUNG"
  check_eq "MCP・ログイン済み: 判定（認証 / プラン / 段）" "true/max/ready" "$(verdict "$TMP/out/gui-ok.out")"
  check_eq "MCP・ログイン済み: auth_unconfirmed は null" "null" "$(jf auth_unconfirmed < "$TMP/out/gui-ok.out")"

  set_flags hang-claude
  mcp_call tako_setup_bootstrap '{"action":"status","agent":"claude"}' gui-hang 60
  check_eq "MCP・返らない claude: 返る" "0" "$HUNG"
  echo "    経過 ${ELAPSED} 秒（上限 3 秒）"
  check_eq "MCP: auth_unconfirmed の名札" "claude auth status --json" "$(jf auth_unconfirmed/label < "$TMP/out/gui-hang.out")"
  check_eq "MCP: auth_unconfirmed の秒" "3" "$(jf auth_unconfirmed/waited_secs < "$TMP/out/gui-hang.out")"
  check_eq "MCP: authenticated は false" "false" "$(jf authenticated < "$TMP/out/gui-hang.out")"
  echo "    要点: $(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps({k: d.get(k) for k in ("agent","authenticated","next_step","auth_unconfirmed")}, ensure_ascii=False))' "$TMP/out/gui-hang.out" 2>/dev/null)"
  expect_no_fakes "MCP status"

  set_flags hang-claude hang-codex hang-agy
  mcp_call tako_setup_bootstrap '{"action":"status-all"}' gui-hang-all 60
  check_eq "MCP status-all・3 系統とも返らない: 返る" "0" "$HUNG"
  local a n=0
  for a in claude codex agy; do
    [ "$(jf "agents/agent=$a/auth_unconfirmed/waited_secs" < "$TMP/out/gui-hang-all.out")" = "3" ] && n=$((n + 1))
  done
  check_eq "MCP status-all: 3 系統とも auth_unconfirmed（3 秒）" "3" "$n"
  if awk -v e="$ELAPSED" 'BEGIN { exit !(e < 8) }'; then
    pass "MCP status-all: 全体 ${ELAPSED} 秒（並べて聞く）"
  else
    fail "MCP status-all: 全体 ${ELAPSED} 秒（並べて聞けていない）"
  fi
  expect_no_fakes "MCP status-all"
  stop_app

  echo "== gui legacy: TAKO_2942_LEGACY=1 の GUI"
  set_flags hang-claude
  start_app gui-legacy TAKO_2942_LEGACY=1 || return
  mcp_call tako_setup_bootstrap '{"action":"status","agent":"claude"}' gui-legacy "$LEGACY_WAIT"
  check_eq "MCP・A/B: ${LEGACY_WAIT} 秒待っても返らない" "1" "$HUNG"
  stop_app
}

want cli && phase_cli
want setup && phase_setup
want legacy && phase_legacy
want old && phase_old
want gui && phase_gui

# 1: 起こした偽物が 1 つも残っていない（全段の pid を見直す。A/B で落としたものも含む）
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
