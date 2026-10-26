#!/usr/bin/env bash
# #2412: tako が起こす claude の worker が user scope の MCP サーバーを持ち込まないこと
# （tako の MCP は使える・プロジェクトで足した MCP と .mcp.json / local scope は立つ）を、
# 隔離した tako-app（仮想ディスプレイ tako-vd）+ 実物の claude + 偽の Messages API で測る。
#
# 腕:
#   new    … この木のビルド（修正後）
#   before … /Applications/tako.app（修正前の main。BEFORE_APP で差し替えられる）
#   headless-before / headless-new … GUI を使わない腕（蓋閉じで tako-vd が描画できないとき）。隔離 tmux の
#     ペインで spawn と同じ形の起動コマンドを打つ（修正後は CLI が決めた止める名前を --settings で渡す）。
#     tako の MCP が呼べるか・spawn の応答はこの腕では見ない
#
# 測るもの（腕ごとに worker を 1 本ずつ spawn し、測り終えたら閉じる = 同時に立つ claude は 1 本）:
#   - worker の claude の子孫（コマンド名と RSS の合計）
#   - どの MCP が起動したか（偽 MCP は起動を MCP_LOG へ書く。実物は子孫のコマンド行で見る）
#   - tako の MCP が使えるか（偽 API が `…tako_show_command` を呼ぶ → 結果が返って CARD-DONE）
#
# user scope の MCP:
#   既定        … 起動を記録するだけの偽 MCP（node）を 6 つ。オフラインで決定的
#   REAL_MCP=1 … 実物（context7 / filesystem / playwright / sequential-thinking = npx・coplay-mcp = uvx）。
#                 常駐の実測用。秘密を持つもの（github / slack）は使わない。npx / uv のキャッシュは
#                 利用者のもの（npm_config_cache / UV_CACHE_DIR）を読む（設定ファイルではない）
#
# 本物の設定（~/.claude.json・設定 dir・projects.yaml）には触らない: 一時の HOME と CLAUDE_CONFIG_DIR、
# 隔離の data / orchestrator / discovery / tmux ソケット。**CI には登録しない**（実物の claude と tako-vd が要る）。
#
# 使い方: scripts/test-worker-mcp-scope-2412.sh                # 両腕・偽 MCP
#         ARMS=new REAL_MCP=1 KEEP=1 scripts/test-worker-mcp-scope-2412.sh
#         ARMS="headless-before headless-new" REAL_MCP=1 scripts/test-worker-mcp-scope-2412.sh   # 蓋閉じ
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
ARMS="${ARMS:-before new}"
REAL_MCP="${REAL_MCP:-}"
BEFORE_APP="${BEFORE_APP:-/Applications/tako.app/Contents/MacOS}"
# MCP が立ち上がるまで待つ秒数（実物の npx は初回の解決に時間がかかる）
SETTLE="${SETTLE:-}"
[ -n "$SETTLE" ] || { if [ -n "$REAL_MCP" ]; then SETTLE=45; else SETTLE=15; fi; }

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux node jq claude; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done
CLAUDE_PATH="$(command -v claude)"
CLAUDE_DIR="$(dirname "$CLAUDE_PATH")"
NODE="$(command -v node)"
REAL_HOME="$HOME"

# 本番 GUI を指す env・呼び出し元の claude の印を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX TAKO_2412_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL

isolated_gui_bins || exit 1
NEW_TAKO_BIN="$TAKO_BIN"
NEW_APP_BIN="$APP_BIN"

TMP_ROOTS=""
SOCKETS=""
FAKE_PID=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  local s d
  for s in $SOCKETS; do tmux -L "$s" kill-server >/dev/null 2>&1; done
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do
      case "$d" in /tmp/t2412-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の Messages API（どの発話にも答える。「Show this card: …」で tako_show_command を呼ぶ）
API_KEY="sk-ant-api03-fake2412fake2412fake2412fake2412fake2412fake2412AAAA"
API_TMP="$(mktemp -d /tmp/t2412-api-XXXXXX)"
TMP_ROOTS="$TMP_ROOTS $API_TMP"
FAKE_LOG="$API_TMP/fake.log" node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 >"$API_TMP/fake.out" 2>&1 &
FAKE_PID=$!
API_PORT=""
for _ in $(seq 1 50); do
  API_PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$API_TMP/fake.out" 2>/dev/null)"
  [ -n "$API_PORT" ] && break
  sleep 0.1
done
[ -n "$API_PORT" ] || { echo "偽 API が立たない"; exit 1; }
LAST20="$(printf '%s' "$API_KEY" | tail -c 20)"

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

# user scope の MCP（名前 → 定義）を JSON で出す
user_scope_json() {
  local tmp="$1" tako_bin="$2"
  if [ -n "$REAL_MCP" ]; then
    jq -n --arg tako "$tako_bin" --arg fsdir "$tmp/fsroot" '{
      tako: { type: "stdio", command: $tako, args: ["mcp", "serve"], env: {} },
      context7: { type: "stdio", command: "npx", args: ["-y", "@upstash/context7-mcp@latest"], env: {} },
      filesystem: { type: "stdio", command: "npx", args: ["-y", "@modelcontextprotocol/server-filesystem", $fsdir], env: {} },
      playwright: { type: "stdio", command: "npx", args: ["-y", "@playwright/mcp@latest"], env: {} },
      "sequential-thinking": { type: "stdio", command: "npx", args: ["-y", "@modelcontextprotocol/server-sequential-thinking"], env: {} },
      "coplay-mcp": { type: "stdio", command: "uvx", args: ["--python", ">=3.11", "coplay-mcp-server@latest"], env: {} }
    }'
  else
    local n out
    out="$(jq -n --arg tako "$tako_bin" '{ tako: { type: "stdio", command: $tako, args: ["mcp", "serve"], env: {} } }')"
    for n in context7 filesystem playwright sequential-thinking coplay-mcp GitKraken; do
      out="$(printf '%s' "$out" | jq --arg n "$n" --arg node "$NODE" --arg f "$tmp/fake-mcp.mjs" \
        '.[$n] = { type: "stdio", command: $node, args: [$f, ("user-" + $n)], env: {} }')"
    done
    printf '%s' "$out"
  fi
}

# pid の子孫（自分を含む）を「pid rss(KB) コマンド行」で並べる
descendants() {
  ps -A -o pid=,ppid=,rss=,command= | awk -v root="$1" '
    { pid[NR]=$1; ppid[NR]=$2; line[NR]=$0 }
    END {
      keep[root]=1; changed=1
      while (changed) { changed=0; for (i=1;i<=NR;i++) if (!keep[pid[i]] && keep[ppid[i]]) { keep[pid[i]]=1; changed=1 } }
      for (i=1;i<=NR;i++) if (keep[pid[i]]) print line[i]
    }'
}

# GUI の子孫から worker の claude（実物の claude の実行ファイル）を探す
worker_claude_pid() {
  # argv[0] が claude（実物は `claude --model …` の形で出る）で、`claude -p`（一時の問い合わせ）ではないもの
  descendants "$ISOLATED_GUI_PID" | awk '{ n = split($4, a, "/"); if (a[n] == "claude" && $5 != "-p") { print $1; exit } }'
}

# 腕ごとの砂場（一時の HOME・設定 dir・プロジェクト 2 つ・隔離の data / orchestrator）。
# TMP / sock / GHOME / CCD / P1 / P2 と TAKO_BIN / APP_BIN を呼び出し側へ残す
setup_sandbox() {
  local arm="$1" bin_arm="$2" resp
  TMP="$(mktemp -d /tmp/t2412-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2412-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm}（user scope の MCP: $([ -n "$REAL_MCP" ] && echo 実物 || echo 偽)）==="
  if [ "$bin_arm" = before ]; then
    TAKO_BIN="$BEFORE_APP/tako"
    APP_BIN="$BEFORE_APP/tako-app"
  else
    TAKO_BIN="$NEW_TAKO_BIN"
    APP_BIN="$NEW_APP_BIN"
  fi
  [ -x "$TAKO_BIN" ] && [ -x "$APP_BIN" ] || { bad "バイナリが無い: $APP_BIN"; return 1; }

  GHOME="$TMP/home" CCD="$TMP/ccd" P1="$TMP/p1" P2="$TMP/p2"
  mkdir -p "$GHOME" "$CCD" "$P1" "$P2" "$TMP/fsroot"
  write_fake_mcp "$TMP/fake-mcp.mjs"
  : >"$TMP/mcp.log"
  # P1: .mcp.json（プロジェクトの MCP）と local scope の MCP を持つ
  jq -n --arg node "$NODE" --arg f "$TMP/fake-mcp.mjs" \
    '{ mcpServers: { "proj-mcpjson": { type: "stdio", command: $node, args: [$f, "proj-mcpjson"], env: {} } } }' \
    >"$P1/.mcp.json"
  jq -n --arg key "$LAST20" --arg p1 "$P1" --arg p2 "$P2" --arg node "$NODE" --arg f "$TMP/fake-mcp.mjs" \
    --argjson user "$(user_scope_json "$TMP" "$TAKO_BIN")" '{
      hasCompletedOnboarding: true, theme: "dark",
      customApiKeyResponses: { approved: [$key], rejected: [] },
      mcpServers: $user,
      projects: {
        ($p1): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true,
                 enabledMcpjsonServers: ["proj-mcpjson"],
                 mcpServers: { "proj-local": { type: "stdio", command: $node, args: [$f, "proj-local"], env: {} } } },
        ($p2): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true }
      }
    }' >"$CCD/.claude.json"
  # tako の MCP の呼び出しを許可（user の settings.json が --settings と並んで効くことも見る）
  printf '%s\n' '{"permissions":{"allow":["mcp__tako"]}}' >"$CCD/settings.json"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  # worker の claude を一時の設定 dir へ向ける（tako が読む user scope もここ = 明示の config dir）
  cat >"$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  CLAUDE_CONFIG_DIR: "$CCD"
YAML
  "$TAKO_BIN" orchestrator projects add --key p1 --cwd "$P1" --description "#2412 の検証" >/dev/null 2>&1
  "$TAKO_BIN" orchestrator projects add --key p2 --cwd "$P2" --description "#2412 の検証" >/dev/null 2>&1
  if [ "$bin_arm" = new ]; then
    # 境界: 存在しない名前は足すが警告 / 空の追加は断る
    resp="$("$TAKO_BIN" orchestrator projects mcp p1 add no-such-server 2>&1)"
    check "$(grep -q '警告: .*no-such-server' <<<"$resp" && echo 1 || echo 0)" \
      "存在しない名前を足すと警告が出る"
    "$TAKO_BIN" orchestrator projects mcp p1 remove no-such-server >/dev/null 2>&1
    local rc
    resp="$("$TAKO_BIN" orchestrator projects mcp p1 add 2>&1)"
    rc=$?
    check "$([ "$rc" -ne 0 ] && grep -q 'servers' <<<"$resp" && echo 1 || echo 0)" \
      "servers の無い add は断る（終了コード ${rc}）"
    # P1 だけに sequential-thinking を足す
    "$TAKO_BIN" orchestrator projects mcp p1 add sequential-thinking >/dev/null 2>&1
    resp="$("$TAKO_BIN" orchestrator projects mcp p1 2>&1)"
    echo "$resp" | sed 's/^/    /'
  fi
}

run_arm() {
  local arm="$1" master resp pane cpid i tree total names
  setup_sandbox "$arm" "$arm" || return
  local extra_env="ISOLATED_GUI_NONE=1"
  [ -n "$REAL_MCP" ] && extra_env="npm_config_cache=$REAL_HOME/.npm"
  launch_isolated_gui "$TMP/app.log" "HOME=$GHOME" "CLAUDE_CONFIG_DIR=$CCD" \
    "ANTHROPIC_BASE_URL=http://127.0.0.1:$API_PORT" "ANTHROPIC_API_KEY=$API_KEY" \
    "DISABLE_AUTOUPDATER=1" "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1" \
    "MCP_LOG=$TMP/mcp.log" "TAKO_PERSIST=0" "SHELL=/bin/zsh" \
    "UV_CACHE_DIR=$REAL_HOME/.cache/uv" "UV_PYTHON_INSTALL_DIR=$REAL_HOME/.local/share/uv/python" \
    "$extra_env" "PATH=$CLAUDE_DIR:$(dirname "$NODE"):/opt/homebrew/bin:/usr/bin:/bin" || exit $?
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"
    bad "隔離 GUI が起動しない"
    return
  fi
  if [ "$("$TAKO_BIN" list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  sleep 3
  master="$("$TAKO_BIN" list | jq -r '.tabs[0].panes[0].id')"

  local project
  for project in p1 p2; do
    [ "$arm" = before ] && [ "$project" = p2 ] && continue
    : >"$TMP/mcp.log"
    resp="$("$TAKO_BIN" orchestrator spawn --project "$project" --pane "$master" --label m \
      --prompt "Reply with exactly one word: READY" 2>&1)"
    pane="$(printf '%s' "$resp" | jq -r '.pane_id // empty' 2>/dev/null)"
    if [ -z "$pane" ]; then
      bad "spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
      continue
    fi
    echo "  [${project}] worker pane=${pane} mcp=$(printf '%s' "$resp" | jq -c '.mcp // "（応答に無い = 修正前）"')"
    cpid=""
    for i in $(seq 1 120); do
      cpid="$(worker_claude_pid)"
      [ -n "$cpid" ] && break
      sleep 0.5
    done
    [ -n "$cpid" ] || { bad "[${project}] worker の claude が見つからない"; continue; }
    sleep "$SETTLE"
    tree="$(descendants "$cpid")"
    total="$(printf '%s\n' "$tree" | awk '{ s += $3 } END { printf "%d", s / 1024 }')"
    echo "  [${project}] claude pid=${cpid} 子孫 $(printf '%s\n' "$tree" | wc -l | tr -d ' ') プロセス・RSS 合計 ${total} MB"
    printf '%s\n' "$tree" | awk '{ rss=$3; $1=$2=$3=""; sub(/^ +/, ""); printf "    %6.1f MB  %s\n", rss/1024, substr($0, 1, 110) }' \
      | sed "s#$REAL_HOME#~#g; s#$TMP#<tmp>#g"
    names="$(sort -u "$TMP/mcp.log" | tr '\n' ' ')"
    echo "  [${project}] 起動した偽 MCP: ${names:-（なし）}"
    printf '%s\n' "$tree" >"$TMP/tree-${project}.txt"
    echo "${arm} ${project} ${total}" >>"$API_TMP/summary.txt"

    # tako の MCP を呼ぶ（偽 API が tako_show_command を呼ぶ → 結果が来たら CARD-DONE）
    "$TAKO_BIN" send --pane "$pane" 'Show this card: {"commands":["echo 2412"],"label":"t2412"}' >/dev/null 2>&1
    local ok=0
    for i in $(seq 1 60); do
      if grep -q "CARD-DONE" <<<"$("$TAKO_BIN" read --pane "$pane" 2>/dev/null)"; then ok=1; break; fi
      sleep 0.5
    done
    check "$ok" "[${arm}/${project}] worker から tako の MCP（tako_show_command）が呼べた"

    local has_tako has_user has_proj has_local has_seq
    has_tako="$(printf '%s\n' "$tree" | grep -c ' mcp serve\|tako-mcp' | tr -d ' ')"
    if [ -n "$REAL_MCP" ]; then
      has_user="$(printf '%s\n' "$tree" | grep -c 'context7\|server-filesystem\|playwright/mcp\|coplay' | tr -d ' ')"
      has_seq="$(printf '%s\n' "$tree" | grep -c 'server-sequential-thinking' | tr -d ' ')"
    else
      has_user="$(grep -c '^user-\(context7\|filesystem\|playwright\|coplay-mcp\|GitKraken\)$' "$TMP/mcp.log" | tr -d ' ')"
      has_seq="$(grep -c '^user-sequential-thinking$' "$TMP/mcp.log" | tr -d ' ')"
    fi
    has_proj="$(grep -c '^proj-mcpjson$' "$TMP/mcp.log" | tr -d ' ')"
    has_local="$(grep -c '^proj-local$' "$TMP/mcp.log" | tr -d ' ')"
    check "$([ "$has_tako" -ge 1 ] && echo 1 || echo 0)" "[${arm}/${project}] tako の MCP ブリッジが子にいる"
    if [ "$arm" = before ]; then
      check "$([ "$has_user" -ge 1 ] && echo 1 || echo 0)" "[before] 修正前は user scope の MCP が立つ（対照）"
    else
      check "$([ "$has_user" = 0 ] && echo 1 || echo 0)" "[new/${project}] user scope の MCP が立たない（${has_user} 件）"
      if [ "$project" = p1 ]; then
        check "$([ "$has_seq" -ge 1 ] && echo 1 || echo 0)" "[new/p1] プロジェクトで足した sequential-thinking は立つ"
      else
        check "$([ "$has_seq" = 0 ] && echo 1 || echo 0)" "[new/p2] 足していないプロジェクトには sequential-thinking が立たない"
      fi
    fi
    if [ "$project" = p1 ]; then
      check "$([ "$has_proj" -ge 1 ] && echo 1 || echo 0)" "[${arm}/p1] .mcp.json の MCP は立つ"
      check "$([ "$has_local" -ge 1 ] && echo 1 || echo 0)" "[${arm}/p1] local scope の MCP は立つ"
    fi
    "$TAKO_BIN" close --pane "$pane" --force >/dev/null 2>&1
    for i in $(seq 1 40); do
      kill -0 "$cpid" 2>/dev/null || break
      sleep 0.5
    done
    kill -0 "$cpid" 2>/dev/null && { kill "$cpid" 2>/dev/null; echo "  （claude pid ${cpid} を自分で落とした）"; }
  done
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
}

# GUI を使わない腕（蓋閉じで tako-vd が描画できないとき）。隔離 tmux のペインで、spawn が組むのと
# 同じ形の起動コマンドを打つ: 修正前 = `TAKO_ORCHESTRATOR_ROLE=… claude`、修正後 = それに
# `--settings <deny ファイル>`。止める名前は修正後の CLI（`tako orchestrator projects mcp <key>`）が
# 決めたもので、ファイルの形は worker_mcp::settings_json と同じ（単体テストが固定）。
# GUI が要る確認（tako の MCP が呼べる・spawn の応答）はこの腕では見ない
run_headless() {
  local arm="$1" bin_arm="${1#headless-}" project cwd denied file cmd cpid i tree total names screen
  setup_sandbox "$arm" "$bin_arm" || return
  local cache_env=""
  [ -n "$REAL_MCP" ] && cache_env="npm_config_cache=$REAL_HOME/.npm"
  for project in p1 p2; do
    [ "$bin_arm" = before ] && [ "$project" = p2 ] && continue
    cwd="$P1"
    [ "$project" = p2 ] && cwd="$P2"
    cmd="TAKO_ORCHESTRATOR_ROLE='worker:${project}:m' claude"
    if [ "$bin_arm" = new ]; then
      denied="$("$TAKO_BIN" orchestrator projects mcp "$project" 2>/dev/null | sed -n 's/^  止める（user scope）: //p')"
      [ "$denied" = "（なし）" ] && denied=""
      file="$TMP/deny-${project}.json"
      printf '%s\n' "$denied" | tr ',' '\n' | sed 's/^ *//; /^$/d' \
        | jq -R . | jq -s '{ deniedMcpServers: map({ serverName: . }) }' >"$file"
      echo "  [${project}] 止める: ${denied:-（なし）}"
      cmd="$cmd --settings '$file'"
    fi
    : >"$TMP/mcp.log"
    tmux -L "$sock" -f /dev/null new-session -d -s "w$project" -x 200 -y 50 -c "$cwd" \
      "exec env -i HOME=$GHOME USER=test LANG=en_US.UTF-8 TERM=xterm-256color \
        PATH=$CLAUDE_DIR:$(dirname "$NODE"):/opt/homebrew/bin:/usr/bin:/bin \
        CLAUDE_CONFIG_DIR=$CCD ANTHROPIC_BASE_URL=http://127.0.0.1:$API_PORT ANTHROPIC_API_KEY=$API_KEY \
        DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 MCP_LOG=$TMP/mcp.log \
        UV_CACHE_DIR=$REAL_HOME/.cache/uv UV_PYTHON_INSTALL_DIR=$REAL_HOME/.local/share/uv/python $cache_env \
        /bin/bash --noprofile --norc"
    sleep 1
    tmux -L "$sock" send-keys -t "w$project" "$cmd" Enter
    local root
    root="$(tmux -L "$sock" display -p -t "w$project" '#{pane_pid}')"
    cpid=""
    for i in $(seq 1 120); do
      cpid="$(descendants "$root" | awk '{ n = split($4, a, "/"); if (a[n] == "claude" && $5 != "-p") { print $1; exit } }')"
      [ -n "$cpid" ] && break
      sleep 0.5
    done
    [ -n "$cpid" ] || { bad "[${arm}/${project}] claude が見つからない"; continue; }
    sleep "$SETTLE"
    tree="$(descendants "$cpid")"
    total="$(printf '%s\n' "$tree" | awk '{ s += $3 } END { printf "%d", s / 1024 }')"
    echo "  [${project}] claude pid=${cpid} 子孫 $(printf '%s\n' "$tree" | wc -l | tr -d ' ') プロセス・RSS 合計 ${total} MB"
    printf '%s\n' "$tree" | awk '{ rss=$3; $1=$2=$3=""; sub(/^ +/, ""); printf "    %6.1f MB  %s\n", rss/1024, substr($0, 1, 110) }' \
      | sed "s#$REAL_HOME#~#g; s#$TMP#<tmp>#g"
    names="$(sort -u "$TMP/mcp.log" | tr '\n' ' ')"
    echo "  [${project}] 起動した偽 MCP: ${names:-（なし）}"
    echo "${arm} ${project} ${total} $(printf '%s\n' "$tree" | wc -l | tr -d ' ')" >>"$API_TMP/summary.txt"
    # /mcp の画面（claude が繋いだ MCP の一覧）
    tmux -L "$sock" send-keys -t "w$project" "/mcp" Enter
    sleep 3
    screen="$(tmux -L "$sock" capture-pane -p -t "w$project" -S -60)"
    printf '%s\n' "$screen" | grep -E "connected|failed|pending|disabled|✔|✘|·" | sed "s#$REAL_HOME#~#g; s#$TMP#<tmp>#g; s/^/    \/mcp: /" | head -20
    tmux -L "$sock" send-keys -t "w$project" Escape

    local has_tako has_user has_proj has_local has_seq
    has_tako="$(printf '%s\n' "$tree" | grep -c ' mcp serve\|tako-mcp' | tr -d ' ')"
    if [ -n "$REAL_MCP" ]; then
      has_user="$(printf '%s\n' "$tree" | grep -c 'context7\|server-filesystem\|playwright/mcp\|coplay' | tr -d ' ')"
      has_seq="$(printf '%s\n' "$tree" | grep -c 'server-sequential-thinking' | tr -d ' ')"
    else
      has_user="$(grep -c '^user-\(context7\|filesystem\|playwright\|coplay-mcp\|GitKraken\)$' "$TMP/mcp.log" | tr -d ' ')"
      has_seq="$(grep -c '^user-sequential-thinking$' "$TMP/mcp.log" | tr -d ' ')"
    fi
    has_proj="$(grep -c '^proj-mcpjson$' "$TMP/mcp.log" | tr -d ' ')"
    has_local="$(grep -c '^proj-local$' "$TMP/mcp.log" | tr -d ' ')"
    check "$([ "$has_tako" -ge 1 ] && echo 1 || echo 0)" "[${arm}/${project}] tako の MCP ブリッジが子にいる"
    if [ "$bin_arm" = before ]; then
      check "$([ "$has_user" -ge 1 ] && echo 1 || echo 0)" "[${arm}] 修正前は user scope の MCP が立つ（対照。${has_user} 件）"
    else
      check "$([ "$has_user" = 0 ] && echo 1 || echo 0)" "[${arm}/${project}] user scope の MCP が立たない（${has_user} 件）"
      if [ "$project" = p1 ]; then
        check "$([ "$has_seq" -ge 1 ] && echo 1 || echo 0)" "[${arm}/p1] プロジェクトで足した sequential-thinking は立つ"
      else
        check "$([ "$has_seq" = 0 ] && echo 1 || echo 0)" "[${arm}/p2] 足していないプロジェクトには sequential-thinking が立たない"
      fi
    fi
    if [ "$project" = p1 ]; then
      check "$([ "$has_proj" -ge 1 ] && echo 1 || echo 0)" "[${arm}/p1] .mcp.json の MCP は立つ"
      check "$([ "$has_local" -ge 1 ] && echo 1 || echo 0)" "[${arm}/p1] local scope の MCP は立つ"
    fi
    kill "$cpid" 2>/dev/null
    for i in $(seq 1 40); do
      kill -0 "$cpid" 2>/dev/null || break
      sleep 0.5
    done
    tmux -L "$sock" kill-session -t "w$project" >/dev/null 2>&1
  done
}

for arm in $ARMS; do
  case "$arm" in
    headless-*) run_headless "$arm" ;;
    *) run_arm "$arm" ;;
  esac
done

echo
echo "=== 1 本あたりの常駐（claude と子孫の RSS 合計）==="
[ -f "$API_TMP/summary.txt" ] && sed 's/^/  /' "$API_TMP/summary.txt"
echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" = 0 ]
