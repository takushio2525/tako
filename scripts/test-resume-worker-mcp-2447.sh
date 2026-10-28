#!/usr/bin/env bash
# #2447: 再起動をまたいで復元した worker（`codex resume <id>` / `claude --resume <id>`）が、spawn と同じ
# MCP の絞り込み（#2412 / #2418）と tako の MCP の一時注入（#986）を持って戻ることを、隔離 GUI の実経路で測る。
#
# 段取り（腕ごとに 1 回。腕ごとに置き場・器のソケットを分ける）:
#   ① 隔離 GUI（器 = TAKO_PERSIST=1・仮想ディスプレイ tako-vd）から worker を 3 本 spawn する
#      - codex p1 … プロジェクトで user-keep を足した・`.codex/config.toml` に user 設定と同名の server がある
#      - codex p2 … 何も足していない・プロジェクトの設定が無い
#      - claude p1 … 偽の claude（起動の引数を記録するだけ。`agents --json` に答えて会話 ID を渡す）
#   ② codex p1 へ `tako send` で 2 通目・3 通目を送り、偽 API に届いたか（入力欄に残らないか）を見る（Issue の観察）
#   ③ 30 秒の走査で会話 ID が layout.json に載るのを待ち、GUI を止めて器（tmux サーバー）ごと落とす = PC の再起動
#   ④ 同じ置き場で GUI を起こし直す → 復元が `codex resume <id>` / `claude --resume <id>` を打つ
#   ⑤ spawn と復元の起動コマンドの MCP の断片（`-c mcp_servers.*` / `--settings`）が同じか・止めた MCP が立たないか・
#      tako の MCP ブリッジ（`tako mcp serve`）が子にいるか・復元した codex へ `tako send` が届くか
#
# 腕: new … 修正後の挙動（断片が spawn と一致することを期待）
#     legacy … 同じバイナリで TAKO_2447_LEGACY=1（修正前の復元 = 断片が無いことを期待）
# 修正前のバイナリで `ARMS=new` を回すと ⑤ の一致が NG になる（= 修正前の症状）。
#
# 本物の設定（~/.codex・~/.claude・projects.yaml・tako の data）には触らない: 一時の HOME / CODEX_HOME /
# CLAUDE_CONFIG_DIR、隔離の data / orchestrator / discovery / tmux ソケット。codex は実物 + 偽の Responses API
# （scripts/lib/fake-responses-api.mjs）、MCP は起動を記録する偽 MCP。**CI には登録しない**（実物の codex と
# tako-vd が要る）。tako-vd が描画できない（蓋閉じ）と終了コード 4 = 未実測。
#
# 使い方: scripts/test-resume-worker-mcp-2447.sh            # 既定 = new legacy
#         ARMS=new scripts/test-resume-worker-mcp-2447.sh   # 修正前のバイナリで症状を見る
#         KEEP=1 …（置き場を残す）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ARMS="${ARMS:-new legacy}"
# MCP が立ち上がるまで待つ秒数
SETTLE="${SETTLE:-10}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux node jq codex git; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done
CODEX_PATH="$(command -v codex)"
CODEX_DIR="$(dirname "$CODEX_PATH")"
NODE="$(command -v node)"
REAL_HOME="$HOME"

# 本番 GUI を指す env・呼び出し元の印・本物の設定の置き場を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  TAKO_2412_LEGACY TAKO_2418_LEGACY TAKO_2447_LEGACY CODEX_HOME OPENAI_API_KEY CLAUDE_CONFIG_DIR ZDOTDIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT

# HOME を差し替える前に建てる（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

TMP_ROOTS=""
SOCKETS=""
FAKE_PID=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  local d
  # 器はソケットファイルまで消す（#2399）
  # shellcheck disable=SC2086 # 名前の並びを語へ分ける
  tmux_socket_kill $SOCKETS
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do
      case "$d" in /tmp/t2447-* | /private/tmp/t2447-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の Responses API（「Reply with exactly one word: X」に X と答える。受けた要求を 1 行ずつ記録する）
API_TMP="$(mktemp -d /tmp/t2447-api-XXXXXX)"
TMP_ROOTS="$TMP_ROOTS $API_TMP"
FAKE_LOG="$API_TMP/fake.log" node "$REPO_ROOT/scripts/lib/fake-responses-api.mjs" 0 >"$API_TMP/fake.out" 2>&1 &
FAKE_PID=$!
API_PORT=""
for _ in $(seq 1 50); do
  API_PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$API_TMP/fake.out" 2>/dev/null)"
  [ -n "$API_PORT" ] && break
  sleep 0.1
done
[ -n "$API_PORT" ] || { echo "偽 API が立たない"; exit 1; }

# 起動を MCP_LOG へ書くだけの偽 MCP（stdio・initialize / tools/list に答える）
write_fake_mcp() {
  cat >"$1" <<'JS'
import fs from 'node:fs'
const name = process.argv[2] ?? 'noname'
if (process.env.MCP_LOG) fs.appendFileSync(process.env.MCP_LOG, `${name}\n`)
let buf = ''
process.stdin.on('data', d => {
  buf += d
  let i
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1)
    let msg; try { msg = JSON.parse(line) } catch { continue }
    if (msg.id === undefined) continue
    let result = {}
    if (msg.method === 'initialize') result = { protocolVersion: msg.params?.protocolVersion ?? '2024-11-05', capabilities: { tools: {} }, serverInfo: { name, version: '0.0.1' } }
    else if (msg.method === 'tools/list') result = { tools: [{ name: 'ping', description: 'ping', inputSchema: { type: 'object', properties: {} } }] }
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }) + '\n')
  }
})
JS
}

# 偽の claude（API を呼ばない。起動の引数を launch.log へ書き、`agents --json` に生きている自分を答える）
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
      printf '%s{"pid":%s,"sessionId":"%s","cwd":"%s","model":"claude-opus-5","status":"idle","kind":"interactive"}' \
        "$sep" "$pid" "$sid" "$cwd"
      sep=','
    done
    printf ']\n'
    exit 0
    ;;
  --version | -v) echo "2.1.301 (Claude Code)"; exit 0 ;;
  -*) : ;;
  *) exit 0 ;;
esac
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
echo "$(date +%s) ROLE=${TAKO_ORCHESTRATOR_ROLE:-} ARGS=$*" >>"$FAKE_DIR/launch.log"
[ -n "$sid" ] || sid=$(printf '%08x-2447-4000-8000-%012x' "$$" "$(date +%s)")
# 会話の記録（復元は記録が実在するときだけ起こす = #652）
slug=$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')
mkdir -p "$CLAUDE_CONFIG_DIR/projects/$slug" "$FAKE_DIR/agents"
[ -f "$CLAUDE_CONFIG_DIR/projects/$slug/$sid.jsonl" ] || echo '{}' >"$CLAUDE_CONFIG_DIR/projects/$slug/$sid.jsonl"
echo "$sid|$(pwd -P)" >"$FAKE_DIR/agents/$$"
trap 'rm -f "${FAKE_DIR:?}/agents/$$"; exit 0' TERM INT HUP
echo "FAKECLAUDE-2447 $sid"
while :; do sleep 5; done
FAKE
  chmod +x "$1"
}

# stdio の偽 MCP 1 つぶんの TOML（名前 → 起動したら MCP_LOG へ書く印）
fake_server_toml() {
  printf '[mcp_servers.%s]\ncommand = "%s"\nargs = ["%s", "%s"]\nenv_vars = ["MCP_LOG"]\n\n' "$1" "$NODE" "$TMP/fake-mcp.mjs" "$2"
}
# claude の user scope の偽 MCP 1 つぶんの JSON
fake_server_json() {
  printf '"%s": {"command": "%s", "args": ["%s", "%s"]}' "$1" "$NODE" "$TMP/fake-mcp.mjs" "$2"
}

# codex の user 設定（#2418 のスクリプトと同じ形。tako = ブリッジの登録の代わりの偽 MCP）
write_codex_config() {
  {
    cat <<TOML
model = "fake-model"
model_provider = "fake"
approval_policy = "never"
check_for_update_on_startup = false

[model_providers.fake]
name = "fake"
base_url = "http://127.0.0.1:${API_PORT}/v1"
wire_api = "responses"
requires_openai_auth = false

[projects."${P1}"]
trust_level = "trusted"

[projects."${P2}"]
trust_level = "trusted"

[mcp_servers.user-web]
url = "http://127.0.0.1:${API_PORT}/mcp/user-web"

TOML
    fake_server_toml tako tako
    fake_server_toml user-alpha user-alpha
    fake_server_toml user-beta user-beta
    fake_server_toml user-keep user-keep
    fake_server_toml user-shared user-shared
  } >"$CH/config.toml"
}

# pid の子孫（自分を含む）を「pid ppid rss(KB) コマンド行」で並べる
descendants() {
  ps -A -o pid=,ppid=,rss=,command= | awk -v root="$1" '
    { pid[NR]=$1; ppid[NR]=$2; line[NR]=$0 }
    END {
      keep[root]=1; changed=1
      while (changed) { changed=0; for (i=1;i<=NR;i++) if (!keep[pid[i]] && keep[ppid[i]]) { keep[pid[i]]=1; changed=1 } }
      for (i=1;i<=NR;i++) if (keep[pid[i]]) print line[i]
    }'
}

# 器（隔離 tmux）のサーバーの pid
tmux_server_pid() { tmux -L "$sock" display -p '#{pid}' 2>/dev/null; }

# 器の子孫から codex の本体（argv[0] が codex）を探す。第 1 引数のペインの器のセッションに絞る
codex_pids_under_server() {
  local server
  server="$(tmux_server_pid)"
  [ -n "$server" ] || return 0
  descendants "$server" | awk -v want="$CODEX_PATH" '{ if ($4 == want || $4 == "codex") print $1 }'
}

# コマンド行の MCP の断片（`mcp_servers.…` の -c と `--settings <パス>`）を 1 行 1 つに並べ替える
mcp_fragments() {
  tr ' ' '\n' <<<"$1" | awk '
    prev == "--settings" { print "--settings " $0 }
    /^mcp_servers\./ { print }
    { prev = $0 }' | sort
}

# 腕ごとの砂場
setup_sandbox() {
  local arm="$1"
  TMP="$(mktemp -d /tmp/t2447-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2447-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm} ==="
  GHOME="$TMP/home" CH="$TMP/codex-home" CC="$TMP/claude-config" P1="$TMP/p1" P2="$TMP/p2" BIN="$TMP/bin"
  FAKE_DIR="$TMP/fake"
  mkdir -p "$GHOME" "$CH" "$CC" "$P1/.codex" "$P2" "$BIN" "$FAKE_DIR/agents"
  write_fake_mcp "$TMP/fake-mcp.mjs"
  write_fake_claude "$BIN/claude"
  : >"$TMP/mcp.log"
  : >"$FAKE_DIR/launch.log"
  # P1 は git の根（codex は cwd から git の根までの .codex/config.toml を読む）
  git -C "$P1" init -q
  {
    fake_server_toml user-shared proj-shared
    fake_server_toml proj-only proj-only
  } >"$P1/.codex/config.toml"
  write_codex_config
  # claude の user scope（tako = ブリッジの登録・user-alpha = 止まる・user-keep = p1 で足す・
  # user-shared = p1 の .mcp.json と同名 = 止めない）
  printf '{"mcpServers": {%s, %s, %s, %s}}\n' \
    "$(printf '"tako": {"command": "tako", "args": ["mcp", "serve"]}')" \
    "$(fake_server_json user-alpha c-user-alpha)" \
    "$(fake_server_json user-keep c-user-keep)" \
    "$(fake_server_json user-shared c-user-shared)" >"$CC/.claude.json"
  printf '{"mcpServers": {%s}}\n' "$(fake_server_json user-shared c-proj-shared)" >"$P1/.mcp.json"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  export TAKO_ISOLATED=1
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  # worker の codex / claude を一時の置き場へ向ける（tako が読む user 設定もここ = env 計画の export）
  cat >"$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  CODEX_HOME: "$CH"
  CLAUDE_CONFIG_DIR: "$CC"
YAML
  "$TAKO_BIN" orchestrator projects add --key p1 --cwd "$P1" --description "#2447 の検証" >/dev/null 2>&1
  "$TAKO_BIN" orchestrator projects add --key p2 --cwd "$P2" --description "#2447 の検証" >/dev/null 2>&1
  "$TAKO_BIN" orchestrator projects mcp p1 add user-keep >/dev/null 2>&1
}

# GUI を起こす（腕の env + 器 ON）。起こせなければ UNMEASURED=1 で 1 を返す
start_gui() {
  local log="$1"
  shift
  if ! launch_isolated_gui "$log" "HOME=$GHOME" "SHELL=/bin/zsh" "TAKO_PERSIST=1" \
    "MCP_LOG=$TMP/mcp.log" "FAKE_DIR=$FAKE_DIR" "CODEX_HOME=$CH" "CLAUDE_CONFIG_DIR=$CC" \
    "TAKO_2413_LEGACY=1" "PATH=$BIN:$CODEX_DIR:$(dirname "$NODE"):/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" ${@+"$@"}; then
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

stop_gui() {
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
}

# 画面に「答えの行」（その語だけの行）が出るまで待つ。出たら 1
wait_answer() {
  local pane="$1" word="$2" i screen
  for i in $(seq 1 60); do
    # pipefail の下で `X | grep -q` にしない（grep が先に抜けると左辺が SIGPIPE で落ちる = #1633）
    screen="$("$TAKO_BIN" read --pane "$pane" 2>/dev/null)"
    if grep -Eq "^[^A-Za-z0-9]*${word}[[:space:]]*\$" <<<"$screen"; then
      echo 1
      return
    fi
    sleep 0.5
  done
  echo 0
}

# 偽 API が受けた応答の要求の数
api_requests() { grep -c '/v1/responses' "$API_TMP/fake.log" 2>/dev/null | tr -d ' '; }

# `tako send` で 1 通送り、偽 API に届いて答えが画面に出たかを見る（Issue の観察）
send_and_observe() {
  local pane="$1" word="$2" label="$3" before after got
  before="$(api_requests)"
  "$TAKO_BIN" send --pane "$pane" "Reply with exactly one word: ${word}" >/dev/null 2>&1
  got="$(wait_answer "$pane" "$word")"
  after="$(api_requests)"
  echo "  [${label}] tako send の ${word}: 偽 API の要求 ${before} → ${after}・答えの行 $([ "$got" = 1 ] && echo あり || echo なし)"
  if [ "$got" != 1 ]; then
    echo "    画面（末尾）:"
    "$TAKO_BIN" read --pane "$pane" 2>/dev/null | sed '/^[[:space:]]*$/d' | tail -8 | sed "s#$TMP#<tmp>#g; s/^/      /"
  fi
  check "$got" "[${label}] tako send の ${word} が送信されて答えが返る（入力欄に残らない）"
}

# codex から tako の MCP（tako_show_command）を呼ばせ、道具の一覧に本物のブリッジの tako_show_command が
# 出るか（user 設定の偽 tako は ping しか持たない）・呼んだ結果の判定（偽 API の記録の out=…）を見る。
# 第 3 引数 = 道具の一覧に出ることを期待するか（1 / 0）。結果は LAST_CARD に置く（spawn と復元で比べる）
card_check() {
  local pane="$1" label="$2" want="$3" before listed i new result=""
  before="$(grep -c . "$API_TMP/fake.log" 2>/dev/null | tr -d ' ')"
  "$TAKO_BIN" send --pane "$pane" 'Show this card: {"commands":["echo 2447"],"label":"t2447"}' >/dev/null 2>&1
  for i in $(seq 1 120); do
    new="$(tail -n +"$((before + 1))" "$API_TMP/fake.log")"
    result="$(sed -n 's/.* out=\([a-z]*\) .*/\1/p' <<<"$new" | tail -1)"
    [ -n "$result" ] && break
    # 道具が無い（NO-TAKO-TOOL）なら関数の結果は来ない
    [ "$want" = 0 ] && [ -n "$new" ] && [ "$i" -ge 20 ] && break
    sleep 0.5
  done
  listed="$(grep -c 'mcp__tako__tako_show_command' <<<"$new" | tr -d ' ')"
  echo "  [${label}] tako の MCP: 道具の一覧に tako_show_command ${listed} 回・呼んだ結果 ${result:-（呼んでいない）}"
  if [ "$want" = 1 ]; then
    check "$([ "$listed" -ge 1 ] && echo 1 || echo 0)" "[${label}] 道具の一覧に本物のブリッジの tako_show_command が出る（一時注入が効いている）"
  else
    check "$([ "$listed" = 0 ] && echo 1 || echo 0)" "[${label}] 道具の一覧に tako_show_command が出ない（一時注入が無い = 対照）"
  fi
  LAST_CARD="${result:-none}"
}

# layout.json に保存された会話 ID の数（codex = agent_resume.id・claude = claude_session_id）
saved_ids() {
  local n
  n="$(jq -r --arg a "$1" '
    if $a == "claude" then [.. | objects | .claude_session_id? | strings]
    else [.. | objects | .agent_resume? | objects | select(.agent == $a) | .id | strings] end
    | length' "$TAKO_DATA_DIR/layout.json" 2>/dev/null)"
  echo "${n:-0}"
}

run_arm() {
  local arm="$1" legacy="" master resp pane_c1 pane_c2 pane_cl i n pid cmd
  local spawn_c1="" spawn_c2="" spawn_cl="" restore_c1="" restore_c2="" restore_cl=""
  setup_sandbox "$arm"
  [ "$arm" = legacy ] && legacy="TAKO_2447_LEGACY=1"

  # ---- ① spawn ----
  start_gui "$TMP/app1.log" ${legacy:+"$legacy"} || return
  if [ "$("$TAKO_BIN" list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  sleep 3
  master="$("$TAKO_BIN" list | jq -r '.tabs[0].panes[0].id')"
  # codex は 1 本ずつ起こす（一時の CODEX_HOME の sqlite を 2 本が同時に作ると、片方が
  # `table … already exists` で落ちる = codex-cli 0.154.0 の初回の移行の競合。tako の挙動ではない）
  resp="$("$TAKO_BIN" orchestrator spawn --project p1 --pane "$master" --label c1 --agent codex \
    --prompt 'Reply with exactly one word: FIRST2447' 2>&1)"
  pane_c1="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  [ -n "$pane_c1" ] && wait_answer "$pane_c1" FIRST2447 >/dev/null
  resp="$("$TAKO_BIN" orchestrator spawn --project p2 --pane "$master" --label c2 --agent codex \
    --prompt 'Reply with exactly one word: FIRST2447' 2>&1)"
  pane_c2="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  resp="$("$TAKO_BIN" orchestrator spawn --project p1 --pane "$master" --label cl --agent claude \
    --prompt 'hello' 2>&1)"
  pane_cl="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  echo "  spawn: codex p1=${pane_c1:-失敗} codex p2=${pane_c2:-失敗} claude p1=${pane_cl:-失敗}"
  if [ -z "$pane_c1" ] || [ -z "$pane_c2" ] || [ -z "$pane_cl" ]; then
    bad "[${arm}] spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
    stop_gui
    return
  fi
  # spawn した codex のコマンド行（器の子孫から。p1 / p2 は cwd で分ける）
  for i in $(seq 1 120); do
    n="$(codex_pids_under_server | grep -c . | tr -d ' ')"
    [ "$n" -ge 2 ] && break
    sleep 0.5
  done
  for pid in $(codex_pids_under_server); do
    cmd="$(ps -o command= -p "$pid")"
    case "$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p')" in
      "$P1") spawn_c1="$cmd" ;;
      "$P2") spawn_c2="$cmd" ;;
    esac
  done
  for i in $(seq 1 60); do
    spawn_cl="$(grep -v -- '--resume' "$FAKE_DIR/launch.log" | tail -1)"
    [ -n "$spawn_cl" ] && break
    sleep 0.5
  done
  check "$([ -n "$spawn_c1" ] && [ -n "$spawn_c2" ] && [ -n "$spawn_cl" ] && echo 1 || echo 0)" \
    "[${arm}] spawn した 3 本の起動コマンドが採れた"
  echo "    claude の spawn: $(printf '%s' "$spawn_cl" | sed "s#$TMP#<tmp>#g; s/^[0-9]* //" | cut -c1-260)"
  check "$(wait_answer "$pane_c1" FIRST2447)" "[${arm}] codex p1 が spawn の依頼に答えた"
  check "$(wait_answer "$pane_c2" FIRST2447)" "[${arm}] codex p2 が spawn の依頼に答えた"

  # ---- ② tako send の 2 通目・3 通目（Issue の観察） ----
  send_and_observe "$pane_c1" SECOND2447 "${arm}/spawn 後"
  send_and_observe "$pane_c1" THIRD2447 "${arm}/spawn 後"
  # 対照: spawn した codex から tako の MCP を呼ぶ（復元後の同じ確認と比べる）
  card_check "$pane_c1" "${arm}/spawn 後" 1
  local spawn_card="$LAST_CARD"

  # ---- ③ 会話 ID の保存を待って器ごと落とす ----
  for i in $(seq 1 90); do
    [ "$(saved_ids codex)" -ge 2 ] && [ "$(saved_ids claude)" -ge 1 ] && break
    sleep 1
  done
  echo "  layout.json の会話 ID: codex $(saved_ids codex) 本・claude $(saved_ids claude) 本"
  if [ "$(saved_ids codex)" -lt 2 ]; then
    for i in "$pane_c1" "$pane_c2"; do
      echo "    ペイン ${i} の画面（末尾）:"
      "$TAKO_BIN" read --pane "$i" 2>/dev/null | sed '/^[[:space:]]*$/d' | tail -6 | sed "s#$TMP#<tmp>#g; s/^/      /"
    done
  fi
  check "$([ "$(saved_ids codex)" -ge 2 ] && [ "$(saved_ids claude)" -ge 1 ] && echo 1 || echo 0)" \
    "[${arm}] 会話 ID が layout.json に保存された"
  stop_gui
  tmux_socket_kill "$sock"
  for i in $(seq 1 40); do
    [ -z "$(codex_pids_under_server)" ] && break
    sleep 0.5
  done
  : >"$TMP/mcp.log"
  : >"$API_TMP/fake.log"

  # ---- ④ 起こし直す = 復元 ----
  start_gui "$TMP/app2.log" ${legacy:+"$legacy"} || return
  for i in $(seq 1 120); do
    n="$(codex_pids_under_server | grep -c . | tr -d ' ')"
    [ "$n" -ge 2 ] && grep -q -- '--resume' "$FAKE_DIR/launch.log" && break
    sleep 0.5
  done
  sleep "$SETTLE"
  for pid in $(codex_pids_under_server); do
    cmd="$(ps -o command= -p "$pid")"
    case "$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p')" in
      "$P1") restore_c1="$cmd" ;;
      "$P2") restore_c2="$cmd" ;;
    esac
  done
  restore_cl="$(grep -- '--resume' "$FAKE_DIR/launch.log" | tail -1)"
  grep '復元の内訳' "$TAKO_DATA_DIR/persist.log" 2>/dev/null | tail -2 | sed 's/^.*復元/    復元/' | cut -c1-200
  check "$([ -n "$restore_c1" ] && [ -n "$restore_c2" ] && [ -n "$restore_cl" ] && echo 1 || echo 0)" \
    "[${arm}] 復元した 3 本（codex resume ×2・claude --resume）の起動コマンドが採れた"

  # ---- ⑤ spawn と復元の MCP の断片 ----
  local label spawn_cmd restore_cmd sf rf
  for label in c1 c2 cl; do
    eval "spawn_cmd=\$spawn_$label restore_cmd=\$restore_$label"
    sf="$(mcp_fragments "$spawn_cmd")"
    rf="$(mcp_fragments "$restore_cmd")"
    echo "  [${label}] spawn の MCP の断片 $(grep -c . <<<"$sf" | tr -d ' ') 個・復元 $(grep -c . <<<"$rf" | tr -d ' ') 個"
    echo "    復元の起動: $(printf '%s' "$restore_cmd" | sed "s#$REAL_HOME#~#g; s#$TMP#<tmp>#g; s/^[0-9]* //" | cut -c1-260)"
    if [ "$arm" = legacy ]; then
      check "$([ -n "$sf" ] && [ -z "$rf" ] && echo 1 || echo 0)" \
        "[legacy/${label}] 修正前の復元は MCP の断片を持たない（spawn は持つ = 対照）"
    else
      check "$([ -n "$sf" ] && [ "$sf" = "$rf" ] && echo 1 || echo 0)" \
        "[${arm}/${label}] 復元の MCP の断片が spawn と同じ"
      [ "$sf" = "$rf" ] || diff <(printf '%s\n' "$sf") <(printf '%s\n' "$rf") | sed "s#$TMP#<tmp>#g; s/^/      /"
    fi
  done
  check "$([ -n "$restore_c1" ] && grep -q 'codex resume' <<<"$restore_c1" && echo 1 || echo 0)" \
    "[${arm}] codex の復元は codex resume の経路"

  # 復元した codex の MCP の子（偽 MCP の起動記録・http の口・tako のブリッジ）
  local names web tree=""
  names="$(sort -u "$TMP/mcp.log" | tr '\n' ' ')"
  web="$(grep -c '/mcp/user-web' "$API_TMP/fake.log" | tr -d ' ')"
  for pid in $(codex_pids_under_server); do tree="$tree$(descendants "$pid")"$'\n'; done
  echo "  復元後に起動した偽 MCP: ${names:-（なし）}・http の口への要求 ${web}"
  local started_alpha started_tako bridge
  started_alpha="$(case " $names " in *" user-alpha "*) echo 1 ;; *) echo 0 ;; esac)"
  started_tako="$(case " $names " in *" tako "*) echo 1 ;; *) echo 0 ;; esac)"
  bridge="$(grep -q ' mcp serve' <<<"$tree" && echo 1 || echo 0)"
  if [ "$arm" = legacy ]; then
    check "$started_alpha" "[legacy] 修正前の復元では user 設定の user-alpha が立つ（対照）"
    check "$([ "$bridge" = 0 ] && echo 1 || echo 0)" "[legacy] 修正前の復元では tako の MCP の一時注入が無い（対照）"
  else
    check "$([ "$started_alpha" = 0 ] && echo 1 || echo 0)" "[${arm}] 復元した codex で user-alpha（stdio）が立たない"
    check "$(case " $names " in *" user-beta "*) echo 0 ;; *) echo 1 ;; esac)" "[${arm}] 復元した codex で user-beta（stdio）が立たない"
    check "$([ "$web" = 0 ] && echo 1 || echo 0)" "[${arm}] 復元した codex が user-web（http）へ繋ぎに行かない（${web} 件）"
    check "$(case " $names " in *" user-keep "*) echo 1 ;; *) echo 0 ;; esac)" "[${arm}] p1 で足した user-keep は立つ"
    check "$(case " $names " in *" proj-shared "*) echo 1 ;; *) echo 0 ;; esac)" "[${arm}] p1 の .codex/config.toml の同名（proj-shared）は立つ"
    check "$([ "$started_tako" = 0 ] && [ "$bridge" = 1 ] && echo 1 || echo 0)" \
      "[${arm}] tako の MCP は一時注入のブリッジ（tako mcp serve）で立つ（user 設定の登録ではない）"
  fi

  # 復元した codex へ tako send が届くか
  local restored_c1
  restored_c1="$("$TAKO_BIN" list | jq -r '[.tabs[].panes[] | select((.role // "") | endswith("p1:c1"))] | .[0].id // empty')"
  if [ -n "$restored_c1" ]; then
    send_and_observe "$restored_c1" AFTER2447 "${arm}/復元後"
    # 復元した codex から tako の MCP が呼べるか（#986 の一時注入。user 設定の偽 tako には
    # tako_show_command が無いので、注入が無ければ道具の一覧に tako の名前空間が出ない）
    card_check "$restored_c1" "${arm}/復元後" "$([ "$arm" = legacy ] && echo 0 || echo 1)"
    # 呼んだ結果は合否に入れない（参考の表示だけ）: MCP の呼び出しに承認が要るかは codex が決める
    # （approval_policy = never で要ると判断すると `requires approval` で断る）。tako の -c を通さない
    # codex 単体でも新規起動で断られることがあり、tako が渡す引数では決まらない（#2447 の報告）
    [ "$arm" != legacy ] && echo "  （参考）tako の MCP を呼んだ結果: spawn ${spawn_card} / 復元 ${LAST_CARD}"
  else
    bad "[${arm}] 復元した codex p1 のペインが見つからない"
  fi
  stop_gui
  tmux_socket_kill "$sock"
}

for arm in $ARMS; do
  case "$arm" in
    new | legacy) run_arm "$arm" ;;
    *) echo "知らない腕: $arm"; exit 2 ;;
  esac
done

echo
echo "PASS=${PASS} FAIL=${FAIL}$([ "$UNMEASURED" = 1 ] && echo '（未実測の腕あり）')"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 4
exit 0
