#!/usr/bin/env bash
# #2804: worker の状態（`tako orchestrator status` = MCP `tako_orchestrator_worker_status`）の
# `has_running_children` と、それに乗る 2 つの判定が器なしでも効くかを、隔離 GUI（仮想ディスプレイ tako-vd）の
# 実経路で器なし / 器ありの 2 腕に並べて測る。
#
#   - #1297 の倒し: 一次シグナル（`claude agents --json`）が busy でも、画面が「入力待ち + 背景作業」なら idle。
#     倒しは `has_running_children` を要求する（子の無い busy は #224 の stalled が拾う）
#   - unknown の補助: 画面から busy / idle が決まらないとき、子の有無で busy へ倒す
#
# 腕:
#   off  … 器なし（TAKO_PERSIST=0。Homebrew cask の既定。tmux は PATH にも置かない）
#   tmux … 器あり（TAKO_PERSIST=1・隔離ソケット）。修正の前後で値が変わらないことの対照
#
# 段（腕ごと。置き場・器のソケットは腕ごとに分ける）:
#   ① 背景作業つきの入力待ち: 最初のペインで偽 claude を起こす。`agents --json` には busy を答え
#      （claude 2.1.258 が Bash の背景シェルの生きている間に返す形）、画面には #1273 の形
#      （`✻ Worked for … · 1 shell still running`・空の入力欄）を描く。
#      `tako orchestrator status --pane N --session-id <ID>` が idle（`idle_despite_primary_busy: true`）を返すか。
#      同じ照会を MCP（`tako mcp serve` の `tako_orchestrator_worker_status`）でも引き、値が CLI と同じか
#   ② 画面から決まらない: 偽 claude を終え、ペインで busy / idle のどちらの印も描かない処理（alt screen に
#      「T2804 compiling 3/9」だけ）を走らせる。`tako orchestrator status --pane N`（会話 ID なし = 画面の推定）が
#      子の有無で busy へ倒すか
#   ③ シェルだけ: ② を終えた後は `has_running_children: false`
#
# `TAKO_2804_LEGACY=1` を付けて回すと器なしの腕は修正前（① は stalled・② は unknown・
# どちらも has_running_children false）を期待する。器ありの腕はどちらでも同じ値を期待する。
# 本物の claude / codex / agy は起こさない（偽物は API を呼ばない）。本番の GUI / tmux / data dir には触らない
# （HOME も一時 dir）。**CI には登録しない**（tako-vd が要る）。tako-vd が描画できない（蓋閉じ）と終了コード 4 = 未実測。
#
# 使い方: scripts/test-bare-children-2804.sh                # 既定 = off tmux
#         ARMS=off scripts/test-bare-children-2804.sh
#         TAKO_2804_LEGACY=1 scripts/test-bare-children-2804.sh
#         KEEP=1 …（置き場を残す。応答の全文は <置き場>/responses/ に入る）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
ARMS="${ARMS:-off tmux}"
# #2804 の A/B（GUI へそのまま継がせる）
LEGACY_2804="${TAKO_2804_LEGACY:-}"
SID="2804c0de-0000-4000-8000-000000002804"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux jq perl python3; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

# 本番 GUI を指す env・呼び出し元の印・本物の設定の置き場を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  CODEX_HOME OPENAI_API_KEY CLAUDE_CONFIG_DIR ZDOTDIR TAKO_2151_INJECT
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT

# HOME を差し替える前に建てる（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

TMP_ROOTS=""
SOCKETS=""
OWN_PIDS=""
cleanup() {
  local d pid
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  # 自分が起こした偽物だけを止める（名前一致の一括 kill はしない）
  for pid in $OWN_PIDS; do kill -KILL "$pid" 2>/dev/null; done
  # shellcheck disable=SC2086 # 名前の並びを語へ分ける
  tmux_socket_kill $SOCKETS
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do
      case "$d" in /tmp/t2804-* | /private/tmp/t2804-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の claude（API を呼ばない）。`agents --json` には生きている自分を busy で答え、
# 対話の起動では会話 ID を決めて画面の描き手（screen.pl）へ置き換わる（pid はそのまま = agents の置き場の名前）
write_fake_claude() {
  cat >"$1" <<'FAKE'
#!/bin/bash
case "${1:-}" in
  agents)
    printf '['
    sep=''
    for f in "$FAKE_DIR"/agents/*; do
      [ -f "$f" ] || continue
      pid=${f##*/}
      kill -0 "$pid" 2>/dev/null || { rm -f "$f"; continue; }
      IFS='|' read -r sid cwd <"$f"
      printf '%s{"pid":%s,"sessionId":"%s","cwd":"%s","model":"claude-opus-5","status":"busy","kind":"interactive"}' \
        "$sep" "$pid" "$sid" "$cwd"
      sep=','
    done
    printf ']\n'
    exit 0
    ;;
  --version | -v) echo "2.1.301 (Claude Code)"; exit 0 ;;
  -p) exit 0 ;;
esac
slug=$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')
cfg="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
mkdir -p "$cfg/projects/$slug" "$FAKE_DIR/agents"
: >>"$cfg/projects/$slug/$FAKE_SID.jsonl"
echo "$FAKE_SID|$(pwd -P)" >"$FAKE_DIR/agents/$$"
exec -a claude /usr/bin/perl "$FAKE_DIR/screen.pl" claude
FAKE
  fake_exec_warm "$1"
}

# 画面の描き手（perl。実行権は要らない）。$ARGV[0] = claude なら #1273 の「背景作業つきの入力待ち」、
# neutral なら busy / idle のどちらの印も無い 1 行だけを alt screen に描き、q か ^C まで黙って待つ
write_screen() {
  cat >"$1" <<'SCREEN'
use strict;
use warnings;
my ($kind) = @ARGV;
$| = 1;
open(my $pf, '>', "$ENV{FAKE_DIR}/$kind.pid") and print $pf $$;
close $pf if $pf;
system('/bin/stty', 'raw', '-echo');
my $rule = "\xe2\x94\x80" x 45;
my @lines = $kind eq 'claude'
  ? (
    "\xe2\x8f\xba T2804 release build is running. Waiting for it to finish.",
    '',
    "\xe2\x9c\xbb Worked for 1m 11s \xc2\xb7 done 2:54 PM \xc2\xb7 1 shell still running",
    '',
    $rule,
    "\xe2\x9d\xaf ",
    $rule,
    '  [model placeholder]  worker: placeholder task',
    "  \xe2\x8f\xb5\xe2\x8f\xb5 auto mode on \xc2\xb7 1 shell \xc2\xb7 \xe2\x86\x90 for agents",
  )
  : ('T2804 compiling 3/9');
print "\e[?1049h\e[2J\e[H", join("\r\n", @lines);
# 入力欄（claude）の「❯ 」の後ろへカーソルを置く
print "\e[6;3H" if $kind eq 'claude';
while (sysread(STDIN, my $c, 1)) { last if $c eq 'q' || $c eq "\x03"; }
print "\e[?1049l";
system('/bin/stty', 'sane');
SCREEN
}

# 腕ごとの砂場
setup_sandbox() {
  local arm="$1"
  TMP="$(mktemp -d /tmp/t2804-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2804-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm} ==="
  GHOME="$TMP/home" BIN="$TMP/bin" RESP="$TMP/responses"
  FAKE_DIR="$TMP/fake"
  mkdir -p "$GHOME" "$BIN" "$RESP" "$FAKE_DIR/agents"
  write_fake_claude "$BIN/claude"
  write_screen "$FAKE_DIR/screen.pl"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  export TAKO_ISOLATED=1
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
}

# GUI を起こす（腕の env）。起こせなければ UNMEASURED=1 で 1
start_gui() {
  local log="$1" arm="$2" persist=0
  local -a arm_env
  if [ "$arm" = tmux ]; then
    persist=1
    arm_env=("PATH=$BIN:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
  else
    arm_env=("PATH=$BIN:/usr/bin:/bin:/usr/sbin:/sbin")
  fi
  [ -n "$LEGACY_2804" ] && arm_env+=("TAKO_2804_LEGACY=$LEGACY_2804")
  if ! launch_isolated_gui "$log" "HOME=$GHOME" "SHELL=/bin/zsh" "TAKO_PERSIST=$persist" \
    "FAKE_DIR=$FAKE_DIR" "FAKE_SID=$SID" ${arm_env[@]+"${arm_env[@]}"}; then
    echo "  未実測: 隔離 GUI を起こせない（tako-vd が描画できない = 蓋閉じ等）"
    UNMEASURED=1
    return 1
  fi
  if ! wait_isolated_gui "$log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"
    ISOLATED_GUI_PID=""
    echo "  未実測: 隔離 GUI が CLI に応答しない"
    UNMEASURED=1
    return 1
  fi
  return 0
}

# 置き場のパスを伏せて 1 行に縮める
mask() { sed "s#$TMP#<tmp>#g" | tr '\n' ' ' | cut -c1-"${1:-240}"; }

# 条件が真になるまで待つ（$1 = 秒・残りは評価する式）
wait_until() {
  local secs="$1" i
  shift
  for i in $(seq 1 $((secs * 2))); do
    eval "$*" && return 0
    sleep 0.5
  done
  return 1
}

# 描き手の pid（$1 = claude / neutral。生きていなければ空）
screen_pid() {
  local pid
  pid="$(cat "$FAKE_DIR/$1.pid" 2>/dev/null)"
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && echo "$pid"
  return 0
}

# 状態照会（CLI。応答の全文は置き場へ・要点を 1 行）。$1 = 段の名前・残りは CLI の引数
status_of() {
  local tag="$1" out
  shift
  out="$(iso_cli orchestrator status "$@" 2>&1)"
  printf '%s\n' "$out" >"$RESP/status-$tag.json"
  STATUS_JSON="$out"
  echo "    status($tag): $(jq -c '{status, status_source, has_running_children, idle_despite_primary_busy, background_work, stalled: (.stalled.detail // null)}' <<<"$out" 2>/dev/null | mask 400)"
  [ -n "$(jq -r '.status // empty' <<<"$out" 2>/dev/null)" ] || echo "      応答: $(printf '%s' "$out" | mask 300)"
}

# 同じ照会を MCP で（`tako mcp serve` の `tako_orchestrator_worker_status`）。$1 = arguments の JSON
mcp_status() {
  local sock token
  sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  token="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2804","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_orchestrator_worker_status\",\"arguments\":$1}}" |
    env TAKO_ISOLATED=1 TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" TAKO_SOCKET="$sock" TAKO_TOKEN="$token" \
      "$TAKO_BIN" mcp serve 2>/dev/null |
    python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        for c in msg.get("result", {}).get("content", []):
            if c.get("type") == "text":
                print(c["text"])'
}

# 応答の欄（無ければ「なし」）
field() { jq -r "if has(\"$1\") then .$1 | tostring else \"なし\" end" <<<"$2" 2>/dev/null; }

run_arm() {
  local arm="$1" pane legacy=0 got want cli mcp k same
  [ -n "$LEGACY_2804" ] && [ "$arm" != tmux ] && legacy=1
  setup_sandbox "$arm"
  start_gui "$TMP/app.log" "$arm" || return
  if [ "$(iso_cli list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  echo "  persist: $(iso_cli persist 2>&1 | jq -c '{enabled, available, backend, reason}' 2>/dev/null | mask 300)"
  sleep 3
  pane="$(iso_cli list | jq -r '.tabs[0].panes[0].id')"
  echo "  pane: ${pane}・tmux_session: $(iso_cli list | jq -r '.tabs[0].panes[0].tmux_session // "なし"')"

  # ---- ① 背景作業つきの入力待ち（#1297 の倒し）----
  iso_cli send --pane "$pane" 'claude' >/dev/null 2>&1
  wait_until 60 '[ -n "$(screen_pid claude)" ]'
  local cpid
  cpid="$(screen_pid claude)"
  OWN_PIDS="$OWN_PIDS $cpid"
  check "$([ -n "$cpid" ] && echo 1 || echo 0)" "[${arm}] 偽 claude がペインで起きた（pid ${cpid:-なし}）"
  echo "    木: $(ps -o pid=,ppid=,command= -p "${cpid:-1}" 2>/dev/null | mask 160)"
  # 画面が描けるまで（背景作業の申告を読めるまで）待つ
  wait_until 30 '[ "$(iso_cli orchestrator status --pane "$pane" --session-id "$SID" 2>/dev/null | jq -r ".background_work // empty")" != "" ]'
  status_of busy-with-background --pane "$pane" --session-id "$SID"
  check "$([ "$(field status_source "$STATUS_JSON")" = agents ] && echo 1 || echo 0)" \
    "[${arm}] 一次シグナルは偽 claude の agents（$(field status_source "$STATUS_JSON")）"
  if [ "$legacy" = 1 ]; then
    check "$([ "$(field has_running_children "$STATUS_JSON")" = false ] && [ "$(field status "$STATUS_JSON")" = stalled ] && echo 1 || echo 0)" \
      "[${arm}][旧 #2804] 子を数えず、背景作業つきの入力待ちが stalled へ倒れる（status=$(field status "$STATUS_JSON")・has_running_children=$(field has_running_children "$STATUS_JSON")）"
  else
    check "$([ "$(field has_running_children "$STATUS_JSON")" = true ] && echo 1 || echo 0)" \
      "[${arm}] agent の生きている間の has_running_children は true（$(field has_running_children "$STATUS_JSON")）"
    check "$([ "$(field status "$STATUS_JSON")" = idle ] && [ "$(field idle_despite_primary_busy "$STATUS_JSON")" = true ] && echo 1 || echo 0)" \
      "[${arm}] #1297 の倒し: 一次シグナルが busy でも入力待ち + 背景作業なら idle（status=$(field status "$STATUS_JSON")・idle_despite_primary_busy=$(field idle_despite_primary_busy "$STATUS_JSON")）"
  fi
  # 同じ照会を MCP で（CLI と同じ値か）
  cli="$STATUS_JSON"
  mcp="$(mcp_status "{\"pane_id\":${pane},\"session_id\":\"${SID}\"}")"
  printf '%s\n' "$mcp" >"$RESP/status-mcp.json"
  same=1
  for k in status status_source has_running_children idle_despite_primary_busy background_work; do
    [ "$(field "$k" "$cli")" = "$(field "$k" "$mcp")" ] || { same=0; echo "    MCP の ${k}: $(field "$k" "$mcp")（CLI: $(field "$k" "$cli")）"; }
  done
  check "$same" "[${arm}] MCP（tako_orchestrator_worker_status）も CLI と同じ値（has_running_children=$(field has_running_children "$mcp")・status=$(field status "$mcp")）"
  iso_cli send --pane "$pane" 'q' >/dev/null 2>&1
  wait_until 20 '[ -z "$(screen_pid claude)" ]' || kill -KILL "$cpid" 2>/dev/null

  # ---- ② 画面から決まらない（unknown の補助）----
  sleep 1
  iso_cli send --pane "$pane" "/usr/bin/perl \"\$FAKE_DIR/screen.pl\" neutral" >/dev/null 2>&1
  wait_until 30 '[ -n "$(screen_pid neutral)" ]'
  local npid
  npid="$(screen_pid neutral)"
  OWN_PIDS="$OWN_PIDS $npid"
  check "$([ -n "$npid" ] && echo 1 || echo 0)" "[${arm}] 画面に印の無い処理がペインで起きた（pid ${npid:-なし}）"
  wait_until 20 'iso_cli read --pane "$pane" 2>/dev/null | grep -q "T2804 compiling"'
  status_of unknown-screen --pane "$pane"
  check "$([ "$(field status_source "$STATUS_JSON")" = screen ] && echo 1 || echo 0)" \
    "[${arm}] 一次シグナルが無く画面の推定（$(field status_source "$STATUS_JSON")）"
  if [ "$legacy" = 1 ]; then
    check "$([ "$(field has_running_children "$STATUS_JSON")" = false ] && [ "$(field status "$STATUS_JSON")" = unknown ] && echo 1 || echo 0)" \
      "[${arm}][旧 #2804] 子を数えず、画面から決まらない状態は unknown のまま（status=$(field status "$STATUS_JSON")）"
  else
    check "$([ "$(field has_running_children "$STATUS_JSON")" = true ] && [ "$(field status "$STATUS_JSON")" = busy ] && echo 1 || echo 0)" \
      "[${arm}] unknown の補助: 画面から決まらないとき子の有無で busy へ倒す（status=$(field status "$STATUS_JSON")・has_running_children=$(field has_running_children "$STATUS_JSON")）"
  fi

  # ---- ③ シェルだけ ----
  iso_cli send --pane "$pane" 'q' >/dev/null 2>&1
  wait_until 20 '[ -z "$(screen_pid neutral)" ]' || kill -KILL "$npid" 2>/dev/null
  sleep 2
  status_of shell-only --pane "$pane"
  got="$(field has_running_children "$STATUS_JSON")"
  want=false
  check "$([ "$got" = "$want" ] && echo 1 || echo 0)" "[${arm}] agent が終わってシェルだけのペインは has_running_children ${want}（${got}）"

  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
  tmux_socket_kill "$sock"
}

for arm in $ARMS; do
  case "$arm" in
    off | tmux) run_arm "$arm" ;;
    *) echo "知らない腕: $arm"; exit 2 ;;
  esac
done

echo
echo "PASS=${PASS} FAIL=${FAIL}$([ "$UNMEASURED" = 1 ] && echo '（未実測の腕あり）')"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 4
exit 0
