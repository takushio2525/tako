#!/usr/bin/env bash
# test-ssh-adopt-offui-2969.sh — #2969: ssh の検知の着地（引き取り）が UI スレッドでログインシェルを待たないことの実経路テスト
#
# 遅い偽のログインシェル（`command -v ssh` に 3 秒かけて答える）の隔離 GUI で、手打ちの ssh に見えるペイン
# （偽の ssh を起動コマンドにした分割ペイン）を作り、#976 の検知が見つけて #1446 の引き取り（`adopt_ssh_connect`）
# が走るのを待つ。perf.log の `メインスレッド専有: periodic_land:ssh_scan が Nms`（着地の区間）を A/B で見る
# （番号は Issue #2969 の受け入れ条件）:
#   3. 修正後は着地の専有が出ない（ssh の実体は走査 = background の段で引く）。修正前（`TAKO_2969_LEGACY=1`）は
#      着地でログインシェルを待って約 3 秒の専有が出る。どちらも引き取りは済む（`tako list` の
#      `ssh_connect.reconnect.source = detected`）
#
# **ssh の接続先には触らない**: 偽の ssh は `cc` で作った実行ファイル（名前が `ssh` でないと検知されない）で、
# 対話の形（`ssh <host>`）は止まったまま居続け、ControlMaster の形（`-O` / `-N`）はすぐ失敗で返る = どこへも繋がない。
# 偽のログインシェルは `command -v ssh` に偽の ssh を答える。**本番の tako にも利用者の設定にも触らない**: data /
# discovery / HOME は mktemp 配下。落とすのは自分で起こした pid だけ。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=gui|legacy（既定は両方）。CI には登録しない（GUI と cc が要る）。CI 側の担保は番犬
# （`crates/tako-control/tests/issue2956_setup_dispatch_offload_watchdog.rs` の `TAKO_APP_UI_LANDINGS`）。
#
# 使い方: bash scripts/test-ssh-adopt-offui-2969.sh
set -uo pipefail
# 人が読む出力は表示言語に従う（#2365）。判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2969_LEGACY TAKO_1446_LEGACY TAKO_976_LEGACY \
  TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR TAKO_TMUX_BIN TAKO_TMUX_SOCKET \
  TAKO_LOGIN_SHELL_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
# 偽のログインシェルが `command -v ssh` に答えるまでの秒数（上限 `login_shell_timeout` 既定 5 秒の内側）
SLOW=3
HOST="fakehost2969"

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

TMP="$(mktemp -d /tmp/tako-2969-XXXXXX)"
APP_PID=""
MCP_PID=""
FAKE_DIR="$TMP/fake"

# 偽物を落とす（自分で書いた pid だけ）
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
tako_exit_trap cleanup "ssh の検知の着地が UI スレッドで待たないかの実経路テスト（#2969）"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の ssh と遅いログインシェル ------------------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の ssh を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR" "$TMP/bin" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
cat > "$TMP/fake-ssh.c" <<'C'
/* 偽の ssh（#2969 の実測用）。どこへも繋がない */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
  int i;
  if (argc > 1 && strcmp(argv[1], "tako-fake-exec-warm") == 0) return 0;
  /* ControlMaster の形（`-O check` / `-O exit` / `-N -f`）はすぐ失敗で返る */
  for (i = 1; i < argc; i++) {
    if (strcmp(argv[i], "-O") == 0 || strcmp(argv[i], "-N") == 0) {
      fprintf(stderr, "fake ssh (#2969): no master\n");
      return 255;
    }
  }
  /* 対話の形（`ssh <host>`）は居続ける = 手で ssh を打ったペイン。pid を書く */
  {
    const char *dir = getenv("FAKE_DIR");
    if (dir) {
      char path[1024];
      snprintf(path, sizeof path, "%s/pids/%d", dir, (int)getpid());
      FILE *f = fopen(path, "w");
      if (f) fclose(f);
    }
  }
  for (;;) pause();
}
C
cc -O1 -o "$TMP/bin/ssh" "$TMP/fake-ssh.c" || exit 1
# 作りたての実行ファイルの初回 exec の検査を上限の外で済ませる（#2222）
"$TMP/bin/ssh" "$FAKE_EXEC_WARM_ARG"
# 遅い偽のログインシェル: `command -v ssh` には SLOW 秒かけて偽の ssh を答え、答えた回数を記録する。それ以外は /bin/sh
cat > "$TMP/stubs/isosh" <<SH
#!/bin/sh
if [ "\$1" = "-l" ]; then shift; fi
case "\$*" in
  *"command -v ssh"*)
    : > "$FAKE_DIR/asked-ssh-\$\$"
    /bin/sleep $SLOW
    echo "$TMP/bin/ssh"; exit 0 ;;
  *"command -v sftp"*) echo "$TMP/bin/sftp"; exit 0 ;;
esac
exec /bin/sh "\$@"
SH
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "TAKO_ISOLATED=1" "PATH=$SANDBOX_PATH" "FAKE_DIR=$FAKE_DIR"
}
check_eq "砂場のログインシェルの ssh は偽物" "$TMP/bin/ssh" \
  "$(env -i PATH="$SANDBOX_PATH" "$TMP/stubs/isosh" -l -c "command -v ssh" 2>/dev/null)"

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
mcp_call() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2969","version":"0"}}}' \
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

# `tako list` の中で、宛先 HOST の ssh を引き取ったペインの `ssh_connect.reconnect.source`（無ければ空）
adopted_source() {
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" list < /dev/null 2>/dev/null | perl -MJSON::PP -e '
      local $/; my $v = eval { JSON::PP->new->utf8->decode(<STDIN>) } or exit;
      my @stack = ($v);
      while (@stack) {
        my $x = pop @stack;
        if (ref $x eq "HASH") {
          if (ref $x->{ssh_connect} eq "HASH" && ($x->{ssh_connect}{host} // "") eq $ARGV[0]) {
            print(($x->{ssh_connect}{reconnect}{source} // ""), "\n"); exit;
          }
          push @stack, values %$x;
        } elsif (ref $x eq "ARRAY") { push @stack, @$x }
      }' "$HOST"
}

# perf.log の「メインスレッド専有: periodic_land:ssh_scan が Nms」の最大の N（無ければ 0）
land_hold_ms() {
  local log="$TAKO_DATA_DIR/perf.log"
  [ -f "$log" ] || { echo 0; return; }
  perl -ne 'BEGIN { $m = 0 } $m = $1 if /メインスレッド専有: periodic_land:ssh_scan が (\d+)ms/ && $1 > $m; END { print "$m\n" }' "$log"
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
  kill_fakes
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# 偽の ssh（対話の形）が起きて pid を書いたか（パイプを組まない = pipefail の下で読み手が早く抜けない）
fake_started() {
  local out
  out="$(command ls "$FAKE_DIR/pids" 2>/dev/null)"
  [ -n "$out" ]
}

# 手打ちの ssh に見えるペインを作り、引き取りまで待つ（上限 30 秒）。結果: ADOPTED（source）・HOLD_MS・ASKED（ログインシェルに聞いた回数）
adopt_case() {
  local phase="$1" i
  rm -f "${FAKE_DIR:?}"/asked-ssh-*
  mcp_call tako_split_pane "{\"command\":[\"$TMP/bin/ssh\",\"$HOST\"],\"focus\":false}" "$phase-split" 30
  for i in $(seq 1 100); do
    fake_started && break
    sleep 0.1
  done
  if ! fake_started; then
    fail "$phase: 偽の ssh のペインが起きない（$(head -c 300 "$TMP/out/$phase-split.out")）"
    return 1
  fi
  ADOPTED=""
  for i in $(seq 1 300); do
    ADOPTED="$(adopted_source)"
    [ -n "$ADOPTED" ] && break
    sleep 0.1
  done
  # 着地の専有の行は区間の終わりに書かれる。引き取りを読めた時点で書き終わっている
  sleep 0.5
  HOLD_MS="$(land_hold_ms)"
  ASKED="$(command ls "$FAKE_DIR" | grep -c '^asked-ssh-' || true)"
  echo "    引き取り: source=${ADOPTED:-（なし）}・着地 periodic_land:ssh_scan の専有の最大 ${HOLD_MS}ms・ログインシェルに聞いた回数 ${ASKED}"
  grep -h "periodic_land:ssh_scan\|UI ストール" "$TAKO_DATA_DIR/perf.log" 2>/dev/null | sed 's/^/      /' | head -4
}

phase_gui() {
  echo "== gui: 修正後（ログインシェルは ${SLOW} 秒かけて答える）"
  start_app gui || return
  adopt_case gui || { stop_app; return; }
  check_eq "gui: 手打ちの ssh を引き取った（ssh_connect.reconnect.source）" "detected" "$ADOPTED"
  check_eq "gui: 着地（UI スレッド）がログインシェルを待たない（periodic_land:ssh_scan の専有）" "0" "$HOLD_MS"
  if [ "${ASKED:-0}" -ge 1 ]; then
    pass "gui: ssh の実体は走査（background）が引いた（ログインシェルに聞いた回数 ${ASKED}）"
  else
    fail "gui: ssh の実体を引いていない（ログインシェルに聞いた回数 ${ASKED:-0}）"
  fi
  # 引き取った後の tick（ssh が生きている間）はログインシェルを起こさない
  rm -f "${FAKE_DIR:?}"/asked-ssh-*
  sleep 8
  check_eq "gui: 引き取り済みのペインだけの tick はログインシェルを起こさない（8 秒の間に聞いた回数）" "0" \
    "$(command ls "$FAKE_DIR" | grep -c '^asked-ssh-' || true)"
  stop_app
}

phase_legacy() {
  echo "== legacy: TAKO_2969_LEGACY=1（着地でログインシェルに聞く）"
  start_app legacy TAKO_2969_LEGACY=1 || return
  adopt_case legacy || { stop_app; return; }
  check_eq "legacy: 手打ちの ssh を引き取った（ssh_connect.reconnect.source）" "detected" "$ADOPTED"
  if [ "${HOLD_MS:-0}" -ge $(((SLOW - 1) * 1000)) ]; then
    pass "legacy: A/B: 着地が UI スレッドでログインシェルを待つ（periodic_land:ssh_scan ${HOLD_MS}ms）"
  else
    fail "legacy: A/B: 着地の専有が出ない（${HOLD_MS:-0}ms）"
  fi
  stop_app
}

want gui && phase_gui
want legacy && phase_legacy

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
