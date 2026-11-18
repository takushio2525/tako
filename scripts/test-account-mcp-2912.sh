#!/usr/bin/env bash
# test-account-mcp-2912.sh — #2912: アカウントの設定 dir で起こす claude（worker / master）が、その dir に
# tako の MCP の登録が無くても tako のツールを持つこと（起動の行の `--mcp-config=<file>`）を、
# 隔離した tako-app（仮想ディスプレイ tako-vd）+ 実物の claude + 偽の Messages API で測る。
#
# 腕（どちらもこの木のビルド。A/B は env）:
#   new    … 修正後
#   legacy … `TAKO_2912_LEGACY=1`（起動の行で渡さず、設定 dir の登録に頼る = 修正前）
#
# 砂場（腕ごと）:
#   - GUI の CLAUDE_CONFIG_DIR（`tako setup-mcp` が登録する 1 か所）= ccd0。tako の登録は無い
#   - アカウント acct-a = 設定 dir acc-a。tako の登録が**無い**。user scope に偽 MCP 2 つ（keepme / other）
#   - アカウント acct-b = 設定 dir acc-b。tako が**登録済み**（command はこの木の tako = 起動の行で渡す
#     入れた本体の tako と見分けられる）。二重に起きないかを見る
#   - プロジェクト p1 の mcp_servers に keepme（#2412 の絞り込み = 前後で同じか）
#   - profiles/default.yaml の master_account: acct-a（master も同じ形か）
#
# 測るもの:
#   1. worker（--account acct-a）: tako の MCP（tako_show_command）が呼べるか・子の tako のブリッジの数
#      （修正後 = 呼べる・1 本 / 修正前 = 呼べない・0 本）
#   2. worker（--account acct-b）: ブリッジは 1 本だけ（修正後は起動の行の側・修正前は登録の側）
#   3. #2412: spawn の応答の mcp.denied / kept と、起きた偽 MCP（keepme だけ）が前後で同じ
#   4. master（tako master --tab・master_account acct-a）: tako の MCP が呼べるか
#   5. check-health のアカウントごとの項目（mcp.claude.account.acct-a / acct-b）
#
# 前提の注意:
#   - claude 2.1 系は既定で MCP のツールを遅延読み込み（tool search）にし、要求の tools に載せない。偽 API は
#     tools から tako_show_command を探すので、GUI へ `ENABLE_TOOL_SEARCH=false` を渡して載せる
#   - `tako master --tab` の新しいタブは GUI の既定のフォルダ（砂場の HOME）で開く（プロファイルの cwd を渡さない =
#     #2912 の範囲外）。信頼の確認が出たままだと MCP が起きないので、砂場の HOME も信頼済みにしておく
#
# 本物の設定（~/.claude*・アカウントの設定 dir・projects.yaml）には触らない: 一時の HOME と CLAUDE_CONFIG_DIR、
# 隔離の data / orchestrator / discovery / tmux ソケット。**CI には登録しない**（実物の claude と tako-vd が要る）。
#
# 使い方: bash scripts/test-account-mcp-2912.sh            # 両腕
#         ARMS=new KEEP=1 bash scripts/test-account-mcp-2912.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ARMS="${ARMS:-new legacy}"
SETTLE="${SETTLE:-12}"

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

# 本番 GUI を指す env・呼び出し元の claude の印を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  TAKO_2412_LEGACY TAKO_2912_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL

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
      case "$d" in /tmp/t2912-* | /private/tmp/t2912-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の Messages API（「Show this card: …」で要求の tools の …tako_show_command を呼ぶ = tako の MCP が見えるか）
API_KEY="sk-ant-api03-fake2912fake2912fake2912fake2912fake2912fake2912AAAA"
API_TMP="$(mktemp -d /tmp/t2912-api-XXXXXX)"
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

# pid の子孫（自分を含む）を「pid ppid rss コマンド行」で並べる
descendants() {
  ps -A -o pid=,ppid=,rss=,command= | awk -v root="$1" '
    { pid[NR]=$1; ppid[NR]=$2; line[NR]=$0 }
    END {
      keep[root]=1; changed=1
      while (changed) { changed=0; for (i=1;i<=NR;i++) if (!keep[pid[i]] && keep[ppid[i]]) { keep[pid[i]]=1; changed=1 } }
      for (i=1;i<=NR;i++) if (keep[pid[i]]) print line[i]
    }'
}

# GUI の子孫から、まだ測っていない実物の claude（`claude -p` の一時の問い合わせは除く）を探す
claude_pid_except() {
  local seen="$1"
  descendants "$ISOLATED_GUI_PID" | awk -v seen=" $seen " '{ n = split($4, a, "/"); if (a[n] == "claude" && $5 != "-p" && index(seen, " " $1 " ") == 0) { print $1; exit } }'
}

# CLI（この木のビルド）。腕の env を付け、HOME と claude の設定 dir も砂場へ向ける（`tako master` の登録の
# 確かめ・claude の実体の検出が本物のホームの `.claude.json` や rc を読まない）
cli() {
  env HOME="$GHOME" CLAUDE_CONFIG_DIR="$CCD0" SHELL=/bin/zsh \
    PATH="$CLAUDE_DIR:$(dirname "$NODE"):/opt/homebrew/bin:/usr/bin:/bin" \
    ${ARM_ENV:+"$ARM_ENV"} "$TAKO_BIN" "$@"
}

# 砂場（腕ごと）
setup_sandbox() {
  local arm="$1"
  TMP="$(mktemp -d /tmp/t2912-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2912-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm}${ARM_ENV:+（${ARM_ENV}）} ==="
  GHOME="$TMP/home" CCD0="$TMP/ccd0" ACC_A="$TMP/acc-a" ACC_B="$TMP/acc-b" P1="$TMP/p1"
  mkdir -p "$GHOME" "$CCD0" "$ACC_A" "$ACC_B" "$P1"
  write_fake_mcp "$TMP/fake-mcp.mjs"
  : >"$TMP/mcp.log"
  local dir user
  for dir in "$CCD0" "$ACC_A" "$ACC_B"; do
    user='{}'
    if [ "$dir" != "$CCD0" ]; then
      user="$(jq -n --arg node "$NODE" --arg f "$TMP/fake-mcp.mjs" '{
        keepme: { type: "stdio", command: $node, args: [$f, "user-keepme"], env: {} },
        other: { type: "stdio", command: $node, args: [$f, "user-other"], env: {} } }')"
    fi
    if [ "$dir" = "$ACC_B" ]; then
      user="$(printf '%s' "$user" | jq --arg tako "$TAKO_BIN" '.tako = { type: "stdio", command: $tako, args: ["mcp", "serve"], env: {} }')"
    fi
    jq -n --arg key "$LAST20" --arg p1 "$P1" --arg gh "$GHOME" --argjson user "$user" '{
        hasCompletedOnboarding: true, theme: "dark",
        customApiKeyResponses: { approved: [$key], rejected: [] },
        mcpServers: $user,
        projects: {
          ($p1): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true },
          ($gh): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
      }' >"$dir/.claude.json"
    printf '%s\n' '{"permissions":{"allow":["mcp__tako"]}}' >"$dir/settings.json"
  done

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  cat >"$TAKO_ORCHESTRATOR_DIR/accounts.yaml" <<YAML
accounts:
  acct-a:
    config_dir: "$ACC_A"
  acct-b:
    config_dir: "$ACC_B"
YAML
  cat >"$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
master_account: acct-a
YAML
  cli orchestrator projects add --key p1 --cwd "$P1" --description "#2912 の検証" >/dev/null 2>&1
  cli orchestrator projects mcp p1 add keepme >/dev/null 2>&1
}

# ペインに CARD-DONE（偽 API が tako_show_command の結果を受けた印）が出るまで待つ
card_done() {
  local pane="$1" i
  cli send --pane "$pane" 'Show this card: {"commands":["echo 2912"],"label":"t2912"}' >/dev/null 2>&1
  for i in $(seq 1 40); do
    if grep -q "CARD-DONE" <<<"$(cli read --pane "$pane" 2>/dev/null)"; then
      echo 1
      return
    fi
    sleep 0.5
  done
  echo 0
}

# claude の起動の行に `--mcp-config=` があるか（1 / 0）
argv_mcp() {
  local args
  args="$(ps -o args= -p "$1" 2>/dev/null)"
  if [[ "$args" == *"--mcp-config="* ]]; then echo 1; else echo 0; fi
}

# ペインの画面の頭（ブリッジが 0 本のときの手がかり。一時 dir は伏せる）
screen_head() {
  local scr
  scr="$(cli read --pane "$1" 2>/dev/null)"
  printf '%s\n' "$scr" | sed "s#$TMP#<tmp>#g" | grep -v '^[[:space:]]*$' | sed -n '1,12p' | sed 's/^/      | /'
}

# claude の子の tako のブリッジ（`… mcp serve`）を「実行ファイル」で並べる
bridges_of() {
  descendants "$1" | awk '$0 ~ / mcp serve/ { for (i = 4; i <= NF; i++) if ($(i+1) == "mcp" && $(i+2) == "serve") { print $i; break } }'
}

SEEN=""
measure_worker() {
  local arm="$1" acct="$2" resp pane cpid i br n av
  : >"$TMP/mcp.log"
  resp="$(cli orchestrator spawn --project p1 --account "$acct" --pane "$MASTER_PANE" --label "w-${acct}" \
    --prompt "Reply with exactly one word: READY" 2>&1)"
  pane="$(printf '%s' "$resp" | jq -r '.pane_id // empty' 2>/dev/null)"
  if [ -z "$pane" ]; then
    bad "[${arm}/${acct}] spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
    return
  fi
  printf '%s' "$resp" | jq -c '{denied: .mcp.denied, kept: .mcp.kept}' >"$TMP/plan-${acct}.json"
  echo "  [${acct}] worker pane=${pane} mcp=$(printf '%s' "$resp" | jq -c '.mcp | {mode, denied, kept, tako_config_file: (.tako_config_file // null | if . then "<data>/worker-mcp/" + (split("/") | last) else null end)}')"
  cpid=""
  for i in $(seq 1 120); do
    cpid="$(claude_pid_except "$SEEN")"
    [ -n "$cpid" ] && break
    sleep 0.5
  done
  [ -n "$cpid" ] || { bad "[${arm}/${acct}] worker の claude が見つからない"; return; }
  SEEN="$SEEN $cpid"
  sleep "$SETTLE"
  br="$(bridges_of "$cpid" | sed "s#$TAKO_BIN#<この木の tako>#; s#/Applications/tako.app/Contents/MacOS/tako#<入れた本体の tako>#")"
  n="$(printf '%s' "$br" | grep -c . | tr -d ' ')"
  av="$(argv_mcp "$cpid")"
  echo "  [${acct}] claude pid=${cpid} の起動の行の --mcp-config: $([ "$av" = 1 ] && echo あり || echo なし)・tako のブリッジ ${n} 本: $(printf '%s' "$br" | tr '\n' ' ')"
  echo "  [${acct}] 起きた偽 MCP: $(sort -u "$TMP/mcp.log" | tr '\n' ' ')"
  printf '%s\n' "$br" >"$TMP/bridges-${acct}.txt"
  sort -u "$TMP/mcp.log" | tr '\n' ' ' >"$TMP/fakemcp-${acct}.txt"
  CARD="$(card_done "$pane")"
  echo "  [${acct}] tako_show_command: $([ "$CARD" = 1 ] && echo 呼べた || echo 呼べない)"
  [ "$CARD" = 1 ] || screen_head "$pane"
  echo "${arm} ${acct} card=${CARD} bridges=${n} argv=${av}" >>"$API_TMP/summary.txt"
  cp "$TMP/plan-${acct}.json" "$API_TMP/plan-${arm}-${acct}.json"
  cp "$TMP/fakemcp-${acct}.txt" "$API_TMP/fakemcp-${arm}-${acct}.txt"
  bridges_of "$cpid" >"$API_TMP/bridges-${arm}-${acct}.txt"
  cli close --pane "$pane" --force >/dev/null 2>&1
  for i in $(seq 1 40); do
    kill -0 "$cpid" 2>/dev/null || break
    sleep 0.5
  done
}

measure_master() {
  local arm="$1" before after pane cpid i
  before="$(cli list | jq -r '[.tabs[].panes[].id | tostring] | sort | join(" ")')"
  cli master --tab >"$TMP/master.out" 2>&1
  pane=""
  for i in $(seq 1 40); do
    after="$(cli list | jq -r '[.tabs[].panes[].id | tostring] | sort | .[]')"
    pane="$(printf '%s\n' "$after" | while read -r p; do case " $before " in *" $p "*) ;; *) echo "$p" ;; esac; done | head -1)"
    [ -n "$pane" ] && break
    sleep 0.5
  done
  [ -n "$pane" ] || { bad "[${arm}/master] master のペインが立たない: $(head -c 300 "$TMP/master.out")"; return; }
  cpid=""
  for i in $(seq 1 120); do
    cpid="$(claude_pid_except "$SEEN")"
    [ -n "$cpid" ] && break
    sleep 0.5
  done
  [ -n "$cpid" ] || { bad "[${arm}/master] master の claude が見つからない"; return; }
  SEEN="$SEEN $cpid"
  sleep "$SETTLE"
  local n av
  n="$(bridges_of "$cpid" | grep -c . | tr -d ' ')"
  av="$(argv_mcp "$cpid")"
  CARD="$(card_done "$pane")"
  echo "  [master] pane=${pane} claude pid=${cpid} の起動の行の --mcp-config: $([ "$av" = 1 ] && echo あり || echo なし)・tako のブリッジ ${n} 本・tako_show_command: $([ "$CARD" = 1 ] && echo 呼べた || echo 呼べない)"
  [ "$CARD" = 1 ] || screen_head "$pane"
  echo "${arm} master card=${CARD} bridges=${n} argv=${av}" >>"$API_TMP/summary.txt"
  cli close --pane "$pane" --force >/dev/null 2>&1
  for i in $(seq 1 40); do
    kill -0 "$cpid" 2>/dev/null || break
    sleep 0.5
  done
}

measure_health() {
  local arm="$1" items
  items="$(cli check-health --json 2>/dev/null | jq -c '[.. | objects | select((.key? // "") | tostring | startswith("mcp.claude.account.")) | {key, status, summary}] | unique')"
  printf '%s\n' "$items" | jq -r '.[] | "  [check-health] \(.key) \(.status): \(.summary)"' | sed "s#$TMP#<tmp>#g"
  printf '%s' "$items" >"$API_TMP/health-${arm}.json"
}

run_arm() {
  local arm="$1"
  ARM_ENV=""
  [ "$arm" = legacy ] && ARM_ENV="TAKO_2912_LEGACY=1"
  setup_sandbox "$arm"
  # shellcheck disable=SC2086 # 空なら渡さない
  launch_isolated_gui "$TMP/app.log" "HOME=$GHOME" "CLAUDE_CONFIG_DIR=$CCD0" \
    "ANTHROPIC_BASE_URL=http://127.0.0.1:$API_PORT" "ANTHROPIC_API_KEY=$API_KEY" \
    "DISABLE_AUTOUPDATER=1" "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1" \
    "ENABLE_TOOL_SEARCH=false" "MCP_LOG=$TMP/mcp.log" "TAKO_PERSIST=0" "SHELL=/bin/zsh" "ISOLATED_GUI_NONE=1" \
    ${ARM_ENV:+"$ARM_ENV"} "PATH=$CLAUDE_DIR:$(dirname "$NODE"):/opt/homebrew/bin:/usr/bin:/bin" || exit $?
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"
    bad "隔離 GUI が起動しない"
    return
  fi
  if [ "$(cli list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  sleep 3
  MASTER_PANE="$(cli list | jq -r '.tabs[0].panes[0].id')"
  measure_worker "$arm" acct-a
  measure_worker "$arm" acct-b
  measure_master "$arm"
  measure_health "$arm"
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
}

for arm in $ARMS; do
  run_arm "$arm"
done

echo
echo "=== 判定 ==="
S="$API_TMP/summary.txt"
[ -f "$S" ] || { echo "測れた腕が無い"; exit 1; }
sed 's/^/  /' "$S"
val() { grep "^$1 $2 " "$S" | sed -n "s/.*$3=\([0-9]*\).*/\1/p" | head -1; }
case " $ARMS " in *" new "*)
  check "$([ "$(val new acct-a argv)" = 1 ] && echo 1 || echo 0)" "修正後: 登録の無いアカウントの worker の起動の行に --mcp-config がある"
  check "$([ "$(val new acct-a card)" = 1 ] && echo 1 || echo 0)" "修正後: 登録の無いアカウントの worker が tako の MCP を呼べる"
  check "$([ "$(val new acct-a bridges)" = 1 ] && echo 1 || echo 0)" "修正後: その worker の tako のブリッジは 1 本"
  check "$([ "$(val new acct-b card)" = 1 ] && echo 1 || echo 0)" "修正後: 登録済みのアカウントの worker も tako の MCP を呼べる"
  check "$([ "$(val new acct-b bridges)" = 1 ] && echo 1 || echo 0)" "修正後: 登録済みでもブリッジは 1 本（二重に起きない）"
  check "$([ "$(val new master argv)" = 1 ] && echo 1 || echo 0)" "修正後: master の起動の行に --mcp-config がある"
  check "$([ "$(val new master bridges)" = 1 ] && echo 1 || echo 0)" "修正後: master の tako のブリッジは 1 本"
  check "$([ "$(val new master card)" = 1 ] && echo 1 || echo 0)" "修正後: 登録の無いアカウントの master が tako の MCP を呼べる"
  check "$(jq -e '[.[] | select(.key == "mcp.claude.account.acct-a" and .status == "ok")] | length == 1' "$API_TMP/health-new.json" >/dev/null && echo 1 || echo 0)" \
    "修正後: check-health の acct-a は ok（起動の行で渡す）"
  ;;
esac
case " $ARMS " in *" legacy "*)
  check "$([ "$(val legacy acct-a argv)" = 0 ] && echo 1 || echo 0)" "A/B 修正前: worker の起動の行に --mcp-config は無い"
  check "$([ "$(val legacy master argv)" = 0 ] && echo 1 || echo 0)" "A/B 修正前: master の起動の行にも無い"
  check "$([ "$(val legacy master bridges)" = 0 ] && echo 1 || echo 0)" "A/B 修正前: 登録の無いアカウントの master に tako のブリッジは無い"
  check "$([ "$(val legacy acct-a card)" = 0 ] && echo 1 || echo 0)" "A/B 修正前: 登録の無いアカウントの worker は tako の MCP を呼べない（元の症状）"
  check "$([ "$(val legacy acct-a bridges)" = 0 ] && echo 1 || echo 0)" "A/B 修正前: その worker に tako のブリッジは無い"
  check "$([ "$(val legacy master card)" = 0 ] && echo 1 || echo 0)" "A/B 修正前: 登録の無いアカウントの master も呼べない"
  check "$([ "$(val legacy acct-b bridges)" = 1 ] && echo 1 || echo 0)" "A/B 修正前: 登録済みのアカウントはブリッジ 1 本（対照）"
  check "$(jq -e '[.[] | select(.key == "mcp.claude.account.acct-a" and .status == "warn")] | length == 1' "$API_TMP/health-legacy.json" >/dev/null && echo 1 || echo 0)" \
    "A/B 修正前の形: check-health の acct-a は warn（登録が無い = 見えない）"
  ;;
esac
if [[ " $ARMS " == *" new "* && " $ARMS " == *" legacy "* ]]; then
  for acct in acct-a acct-b; do
    check "$(cmp -s "$API_TMP/plan-new-${acct}.json" "$API_TMP/plan-legacy-${acct}.json" && echo 1 || echo 0)" \
      "#2412: ${acct} の止める / 持ち込む名前が前後で同じ（$(cat "$API_TMP/plan-new-${acct}.json")）"
    check "$(cmp -s "$API_TMP/fakemcp-new-${acct}.txt" "$API_TMP/fakemcp-legacy-${acct}.txt" && echo 1 || echo 0)" \
      "#2412: ${acct} で起きた user scope の MCP が前後で同じ（$(cat "$API_TMP/fakemcp-new-${acct}.txt")）"
  done
  check "$([ "$(cat "$API_TMP/bridges-new-acct-b.txt")" != "$TAKO_BIN" ] && [ "$(cat "$API_TMP/bridges-legacy-acct-b.txt")" = "$TAKO_BIN" ] && echo 1 || echo 0)" \
    "登録済みのアカウント: 修正後は起動の行の側・修正前は登録の側（この木の tako）のブリッジが起きる"
fi
echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" = 0 ]
