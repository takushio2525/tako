#!/usr/bin/env bash
# #2418: tako が起こす codex の worker が user 設定（`$CODEX_HOME/config.toml` の `[mcp_servers.*]`）の
# MCP サーバーを持ち込まないこと（tako の MCP・プロジェクトで足した MCP・`.codex/config.toml` の MCP は立つ）を、
# 実物の codex + 偽の Responses API（scripts/lib/fake-responses-api.mjs）+ 起動を記録する偽 MCP で測る。
#
# 腕:
#   headless-before / headless-new … GUI を使わない腕。隔離 tmux のペインで spawn と同じ形の起動コマンドを打つ
#     （修正前 = `TAKO_ORCHESTRATOR_ROLE=… codex`、修正後 = それに修正後の CLI（`tako orchestrator projects mcp`）が
#     決めた止める名前を worker_mcp::codex_off_args と同じ形の `-c` で足したもの。形は単体テストが固定）。
#     修正後の腕では「tako が読んだ設定と worker が読む設定がずれても codex が起動する」（目印の効き目）も見る
#   new … 隔離 GUI（仮想ディスプレイ tako-vd）から実際に spawn する。応答の `mcp`・起動した codex のコマンド行・
#     子孫・worker から tako の MCP（tako_show_command）が呼べることを見る。tako-vd が描画できない（蓋閉じ）と
#     終了コード 4 = 未実測
#   edge … 境界: user 設定に MCP が 1 つも無い / プロジェクトで足した名前が user 設定に無い / config.toml が壊れている
#
# 本物の設定（~/.codex・projects.yaml・tako の data）には触らない: 一時の HOME と CODEX_HOME、隔離の data /
# orchestrator / discovery / tmux ソケット。CLI の `projects mcp` は spawn と同じくプロファイルの `env:` の
# CODEX_HOME を読む（検証プロセスは環境変数の CODEX_HOME を読まない = #944 と同じ）。**CI には登録しない**
# （実物の codex と tako-vd が要る）。
#
# 使い方: scripts/test-codex-worker-mcp-2418.sh                         # 既定 = headless-before headless-new edge new
#         ARMS="headless-before headless-new edge" scripts/test-codex-worker-mcp-2418.sh   # 蓋閉じ
#         KEEP=1 …（置き場を残す）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ARMS="${ARMS:-headless-before headless-new edge new}"
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

# 本番 GUI を指す env・呼び出し元の印・本物の CODEX_HOME を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  TAKO_2412_LEGACY TAKO_2418_LEGACY CODEX_HOME OPENAI_API_KEY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT

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
      case "$d" in /tmp/t2418-* | /private/tmp/t2418-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の Responses API（「Show this card: …」で tako_show_command を呼ぶ。MCP の http の口への要求も記録する）
API_TMP="$(mktemp -d /tmp/t2418-api-XXXXXX)"
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

# stdio の偽 MCP 1 つぶんの TOML（名前 → 起動したら MCP_LOG へ書く印）
fake_server_toml() {
  printf '[mcp_servers.%s]\ncommand = "%s"\nargs = ["%s", "%s"]\nenv_vars = ["MCP_LOG"]\n\n' "$1" "$NODE" "$TMP/fake-mcp.mjs" "$2"
}

# codex の user 設定（偽 API へ向けた provider + user 設定の MCP）
#   tako        … tako のブリッジの登録（`tako setup-mcp` 相当。名前で常に残る。GUI の腕では一時注入が実物へ差し替える）
#   user-alpha / user-beta … 止まるべき stdio
#   user-web    … 止まるべき http（偽 API の /mcp/user-web へ繋ぎに来たら記録に残る）
#   user-keep   … p1 のプロジェクトで足す（p1 では立ち、p2 では止まる）
#   user-shared … p1 の .codex/config.toml と同名（止めない = p1 ではプロジェクト側の定義 proj-shared が立つ）
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

# 子孫から codex の本体（argv[0] が codex）を探す。第 2 引数を渡すとコマンド行にそれを含むものだけ
# （GUI は spawn の前に `codex --version` 等の短命な確認を走らせるので、worker の起動コマンドの印で絞る）
codex_pid_under() {
  descendants "$1" | awk -v want="$CODEX_PATH" -v must="${2:-}" '{ if (($4 == want || $4 == "codex") && (must == "" || index($0, must) > 0)) { print $1; exit } }'
}

# 腕ごとの砂場（一時の HOME・CODEX_HOME・プロジェクト 2 つ・隔離の data / orchestrator）
setup_sandbox() {
  local arm="$1"
  TMP="$(mktemp -d /tmp/t2418-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2418-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm} ==="
  GHOME="$TMP/home" CH="$TMP/codex-home" P1="$TMP/p1" P2="$TMP/p2"
  mkdir -p "$GHOME" "$CH" "$P1/.codex" "$P2"
  write_fake_mcp "$TMP/fake-mcp.mjs"
  : >"$TMP/mcp.log"
  # P1 は git の根（codex は cwd から git の根までの .codex/config.toml を読む）
  git -C "$P1" init -q
  {
    fake_server_toml user-shared proj-shared
    fake_server_toml proj-only proj-only
  } >"$P1/.codex/config.toml"
  write_codex_config

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  export TAKO_ISOLATED=1
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  # worker の codex を一時の CODEX_HOME へ向ける（tako が読む user 設定もここ = env 計画の export）
  cat >"$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  CODEX_HOME: "$CH"
YAML
  "$TAKO_BIN" orchestrator projects add --key p1 --cwd "$P1" --description "#2418 の検証" >/dev/null 2>&1
  "$TAKO_BIN" orchestrator projects add --key p2 --cwd "$P2" --description "#2418 の検証" >/dev/null 2>&1
  "$TAKO_BIN" orchestrator projects mcp p1 add user-keep >/dev/null 2>&1
}

# 修正後の CLI が決めた codex の止める名前（`codex の worker で止める（config.toml）: a, b`）
codex_denied_of() {
  local out line
  out="$("$TAKO_BIN" orchestrator projects mcp "$1" 2>/dev/null)"
  line="$(sed -n 's/^  codex の worker で止める（config.toml）: //p' <<<"$out")"
  [ "$line" = "（なし）" ] && line=""
  printf '%s\n' "$line" | tr ',' '\n' | sed 's/^ *//; /^$/d'
}

# worker_mcp::codex_off_args と同じ形の `-c`（http は url・ほかは command の目印。砂場で http なのは user-web だけ）
codex_off_args_of() {
  local name out=""
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    if [ "$name" = "user-web" ]; then
      out="$out -c 'mcp_servers.${name}.enabled=false' -c 'mcp_servers.${name}.url=http://127.0.0.1:9/tako-disabled'"
    else
      out="$out -c 'mcp_servers.${name}.enabled=false' -c 'mcp_servers.${name}.command=tako-disabled'"
    fi
  done
  printf '%s' "$out"
}

# 1 本の codex を隔離 tmux のペインで起こし、子孫と起動した MCP を見る。
# 呼び出し側へ TREE（子孫）・NAMES（起動した偽 MCP）・WEB（http の口へ来た要求の数）・CPID・RC（codex が落ちたら 1）を残す
run_codex_in_tmux() {
  local session="$1" cwd="$2" cmd="$3" codex_home="$4" root i
  : >"$TMP/mcp.log"
  : >"$API_TMP/fake.log"
  tmux -L "$sock" -f /dev/null new-session -d -s "$session" -x 200 -y 50 -c "$cwd" \
    "exec env -i HOME=$GHOME USER=test LANG=en_US.UTF-8 TERM=xterm-256color \
      PATH=$CODEX_DIR:$(dirname "$NODE"):/usr/bin:/bin CODEX_HOME=$codex_home MCP_LOG=$TMP/mcp.log \
      /bin/bash --noprofile --norc"
  sleep 1
  tmux -L "$sock" send-keys -t "$session" "$cmd" Enter
  root="$(tmux -L "$sock" display -p -t "$session" '#{pane_pid}')"
  CPID=""
  for i in $(seq 1 60); do
    CPID="$(codex_pid_under "$root")"
    [ -n "$CPID" ] && break
    sleep 0.5
  done
  RC=0
  if [ -z "$CPID" ]; then
    RC=1
    TREE=""
  else
    sleep "$SETTLE"
    kill -0 "$CPID" 2>/dev/null || RC=1
    TREE="$(descendants "$CPID")"
  fi
  NAMES="$(sort -u "$TMP/mcp.log" | tr '\n' ' ')"
  WEB="$(grep -c '/mcp/user-web' "$API_TMP/fake.log" | tr -d ' ')"
  SCREEN="$(tmux -L "$sock" capture-pane -p -t "$session" -S -40)"
  [ -n "$CPID" ] && kill "$CPID" 2>/dev/null
  for i in $(seq 1 20); do
    [ -n "$CPID" ] && kill -0 "$CPID" 2>/dev/null || break
    sleep 0.25
  done
  tmux -L "$sock" kill-session -t "$session" >/dev/null 2>&1
}

started() { case " $NAMES " in *" $1 "*) echo 1 ;; *) echo 0 ;; esac; }
not_started() { case " $NAMES " in *" $1 "*) echo 0 ;; *) echo 1 ;; esac; }

run_headless() {
  local arm="$1" bin_arm="${1#headless-}" project cwd denied args cmd
  setup_sandbox "$arm"
  for project in p1 p2; do
    [ "$bin_arm" = before ] && [ "$project" = p2 ] && continue
    cwd="$P1"
    [ "$project" = p2 ] && cwd="$P2"
    cmd="TAKO_ORCHESTRATOR_ROLE='worker:${project}:m' codex"
    if [ "$bin_arm" = new ]; then
      denied="$(codex_denied_of "$project")"
      echo "  [${project}] 止める: $(printf '%s' "$denied" | tr '\n' ' ')"
      args="$(codex_off_args_of <<<"$denied")"
      cmd="$cmd$args"
    fi
    run_codex_in_tmux "w$project" "$cwd" "$cmd" "$CH"
    echo "  [${project}] codex pid=${CPID:-なし} 子孫 $(printf '%s\n' "$TREE" | grep -c . | tr -d ' ') プロセス・起動した偽 MCP: ${NAMES:-（なし）}・http の口への要求 ${WEB}"
    printf '%s\n' "$TREE" | awk '{ rss=$3; $1=$2=$3=""; sub(/^ +/, ""); printf "    %6.1f MB  %s\n", rss/1024, substr($0, 1, 110) }' \
      | sed "s#$REAL_HOME#~#g; s#$TMP#<tmp>#g"
    check "$([ "$RC" = 0 ] && echo 1 || echo 0)" "[${arm}/${project}] codex が起動して生きている"
    check "$(started tako)" "[${arm}/${project}] tako の MCP（user 設定の登録）は立つ"
    if [ "$bin_arm" = before ]; then
      check "$(started user-alpha)" "[before] 修正前は user 設定の MCP が立つ（対照）"
      check "$([ "$WEB" -ge 1 ] && echo 1 || echo 0)" "[before] 修正前は http の user 設定にも繋ぎに行く（対照。${WEB} 件）"
    else
      check "$(not_started user-alpha)" "[${arm}/${project}] user-alpha（stdio）が立たない"
      check "$(not_started user-beta)" "[${arm}/${project}] user-beta（stdio）が立たない"
      check "$([ "$WEB" = 0 ] && echo 1 || echo 0)" "[${arm}/${project}] user-web（http）へ繋ぎに行かない（${WEB} 件）"
      if [ "$project" = p1 ]; then
        check "$(started user-keep)" "[${arm}/p1] プロジェクトで足した user-keep は立つ"
      else
        check "$(not_started user-keep)" "[${arm}/p2] 足していないプロジェクトでは user-keep が立たない"
        check "$(not_started user-shared)" "[${arm}/p2] プロジェクト設定の無い p2 では user-shared も止まる"
      fi
    fi
    if [ "$project" = p1 ]; then
      check "$(started proj-only)" "[${arm}/p1] .codex/config.toml の proj-only は立つ"
      check "$(started proj-shared)" "[${arm}/p1] user 設定と同名の .codex/config.toml 側（proj-shared）は立つ"
      check "$(not_started user-shared)" "[${arm}/p1] 同名の user 側の定義（user-shared）は立たない（プロジェクト側が勝つ）"
    fi
  done
  if [ "$bin_arm" = new ]; then
    # 目印の効き目: worker のシェルが別の CODEX_HOME（止める名前を 1 つも持たない）を読んでも起動する
    local other="$TMP/other-codex-home" bare
    mkdir -p "$other"
    sed '/^\[mcp_servers\./,$d' "$CH/config.toml" >"$other/config.toml"
    denied="$(codex_denied_of p2)"
    run_codex_in_tmux "wdrift" "$P2" "codex$(codex_off_args_of <<<"$denied")" "$other"
    check "$([ "$RC" = 0 ] && echo 1 || echo 0)" "[${arm}] 設定がずれても（止める名前が config.toml に無い）目印つきなら codex は起動する"
    # 対照: 目印を付けずに enabled=false だけだと起動しない
    bare="$(printf '%s\n' "$denied" | sed 's/.*/ -c mcp_servers.&.enabled=false/' | tr -d '\n')"
    run_codex_in_tmux "wbare" "$P2" "codex$bare" "$other"
    check "$([ "$RC" = 1 ] && echo 1 || echo 0)" "[${arm}] 対照: 目印なしの enabled=false だけでは codex が起動しない"
    printf '%s\n' "$SCREEN" | grep -i "error\|invalid" | head -2 | sed "s#$TMP#<tmp>#g; s/^/    画面: /"
  fi
}

run_edge() {
  local out
  setup_sandbox edge
  # 1) user 設定に MCP が 1 つも無い
  sed '/^\[mcp_servers\./,$d' "$CH/config.toml" >"$CH/config.toml.new" && /bin/mv -f "$CH/config.toml.new" "$CH/config.toml"
  out="$("$TAKO_BIN" orchestrator projects mcp p2 2>&1)"
  check "$(grep -q '^  codex の worker で止める（config.toml）: （なし）$' <<<"$out" && echo 1 || echo 0)" \
    "[edge] user 設定に MCP が無ければ止める名前は（なし）"
  # 2) プロジェクトで足した名前が user 設定に無い（足すが警告・止める名前は変わらない）
  write_codex_config
  out="$("$TAKO_BIN" orchestrator projects mcp p2 add no-such-server 2>&1)"
  check "$(grep -q "警告: codex: .*no-such-server" <<<"$out" && echo 1 || echo 0)" \
    "[edge] user 設定に無い名前を足すと codex の警告が出る"
  check "$(grep -q '^  codex の worker で止める（config.toml）: user-alpha, user-beta, user-keep, user-shared, user-web$' <<<"$out" && echo 1 || echo 0)" \
    "[edge] 足した名前が見つからなくても止める名前は変わらない"
  # 3) config.toml が壊れている（tako は何も足さずに警告・codex 自身も起動しない）
  printf '[mcp_servers.a\ncommand = \n' >"$CH/config.toml"
  out="$("$TAKO_BIN" orchestrator projects mcp p2 2>&1)"
  check "$(grep -q '^  codex の worker で止める（config.toml）: （なし）$' <<<"$out" && grep -q '警告: codex: codex の設定を読めない' <<<"$out" && echo 1 || echo 0)" \
    "[edge] 壊れた config.toml なら何も止めずに警告する"
  run_codex_in_tmux "wbroken" "$P2" "codex" "$CH"
  check "$([ "$RC" = 1 ] && echo 1 || echo 0)" "[edge] 壊れた config.toml では codex 自身も起動しない（tako の -c の有無に依らない）"
}

run_gui() {
  local resp pane master cpid i tree mcp cmdline ok
  setup_sandbox gui
  if ! launch_isolated_gui "$TMP/app.log" "HOME=$GHOME" "SHELL=/bin/zsh" "TAKO_PERSIST=0" \
    "MCP_LOG=$TMP/mcp.log" "PATH=$CODEX_DIR:$(dirname "$NODE"):/opt/homebrew/bin:/usr/bin:/bin"; then
    echo "  未実測: 隔離 GUI を起こせない（tako-vd が描画できない = 蓋閉じ等）"
    UNMEASURED=1
    return
  fi
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"
    ISOLATED_GUI_PID=""
    echo "  未実測: 隔離 GUI が CLI に応答しない"
    UNMEASURED=1
    return
  fi
  if [ "$("$TAKO_BIN" list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  sleep 3
  master="$("$TAKO_BIN" list | jq -r '.tabs[0].panes[0].id')"
  : >"$TMP/mcp.log"
  : >"$API_TMP/fake.log"
  # 最初の依頼で tako の MCP を呼ばせる（偽 API が tako_show_command を呼ぶ → 結果が来たら CARD-DONE）。
  # spawn の依頼は送達確認つきで届く（2 通目を `tako send` で送ると codex の入力欄に残って送信されないことがあった）
  resp="$("$TAKO_BIN" orchestrator spawn --project p1 --pane "$master" --label m --agent codex \
    --prompt 'Show this card: {"commands":["echo 2418"],"label":"t2418"}' 2>&1)"
  pane="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  if [ -z "$pane" ]; then
    bad "spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
    return
  fi
  mcp="$(jq -c '.mcp' <<<"$resp")"
  echo "  [p1] worker pane=${pane} mcp=$(sed "s#$TMP#<tmp>#g" <<<"$mcp")"
  check "$(jq -e '.mode == "tako_only"' <<<"$mcp" >/dev/null && echo 1 || echo 0)" "[gui] 応答の mcp.mode が tako_only"
  check "$(jq -e '.denied == ["user-alpha","user-beta","user-web"]' <<<"$mcp" >/dev/null && echo 1 || echo 0)" \
    "[gui] 応答の mcp.denied が user-alpha / user-beta / user-web"
  check "$(jq -e '.kept == ["user-keep"] and .shadowed == ["user-shared"]' <<<"$mcp" >/dev/null && echo 1 || echo 0)" \
    "[gui] 応答の mcp.kept = user-keep・shadowed = user-shared"
  check "$(jq -e '.config_file | endswith("/codex-home/config.toml")' <<<"$mcp" >/dev/null && echo 1 || echo 0)" \
    "[gui] 応答の mcp.config_file がプロファイルの CODEX_HOME の config.toml"
  cpid=""
  for i in $(seq 1 120); do
    cpid="$(codex_pid_under "$ISOLATED_GUI_PID" "mcp_servers.tako")"
    [ -n "$cpid" ] && break
    sleep 0.5
  done
  [ -n "$cpid" ] || { bad "[gui] worker の codex が見つからない"; return; }
  sleep "$SETTLE"
  cmdline="$(ps -o command= -p "$cpid")"
  check "$(grep -q 'mcp_servers.user-alpha.enabled=false' <<<"$cmdline" && grep -q 'mcp_servers.user-web.url=http://127.0.0.1:9/tako-disabled' <<<"$cmdline" && echo 1 || echo 0)" \
    "[gui] 起動した codex のコマンド行に止める -c と目印が載っている"
  check "$(grep -q 'mcp_servers.user-keep.enabled\|mcp_servers.user-shared.enabled\|mcp_servers.tako.enabled' <<<"$cmdline" && echo 0 || echo 1)" \
    "[gui] 足した名前・同名・tako には -c が付かない"
  tree="$(descendants "$cpid")"
  NAMES="$(sort -u "$TMP/mcp.log" | tr '\n' ' ')"
  echo "  [p1] codex pid=${cpid} 子孫 $(printf '%s\n' "$tree" | grep -c . | tr -d ' ') プロセス・起動した偽 MCP: ${NAMES:-（なし）}"
  printf '%s\n' "$tree" | awk '{ rss=$3; $1=$2=$3=""; sub(/^ +/, ""); printf "    %6.1f MB  %s\n", rss/1024, substr($0, 1, 110) }' \
    | sed "s#$REAL_HOME#~#g; s#$TMP#<tmp>#g"
  check "$(grep -q ' mcp serve' <<<"$tree" && echo 1 || echo 0)" "[gui] tako の MCP ブリッジ（一時注入）が子にいる"
  check "$(not_started user-alpha)" "[gui] user-alpha が立たない"
  check "$(not_started user-beta)" "[gui] user-beta が立たない"
  WEB="$(grep -c '/mcp/user-web' "$API_TMP/fake.log" | tr -d ' ')"
  check "$([ "$WEB" = 0 ] && echo 1 || echo 0)" "[gui] user-web へ繋ぎに行かない（${WEB} 件）"
  check "$(started user-keep)" "[gui] プロジェクトで足した user-keep は立つ"
  check "$(started proj-shared)" "[gui] 同名の .codex/config.toml 側（proj-shared）は立つ"
  check "$(started proj-only)" "[gui] .codex/config.toml の proj-only は立つ"
  ok=0
  for i in $(seq 1 60); do
    if grep -q "CARD-DONE" <<<"$("$TAKO_BIN" read --pane "$pane" 2>/dev/null)"; then ok=1; break; fi
    sleep 0.5
  done
  check "$ok" "[gui] worker から tako の MCP（tako_show_command）が呼べた"
  if [ "$ok" != 1 ]; then
    echo "  画面（末尾）:"
    "$TAKO_BIN" read --pane "$pane" 2>/dev/null | sed '/^[[:space:]]*$/d' | tail -15 | sed "s#$TMP#<tmp>#g; s/^/    /"
    echo "  偽 API が受けた要求:"
    cut -c1-200 "$API_TMP/fake.log" | sed 's/^/    /'
  fi
  "$TAKO_BIN" close --pane "$pane" --force >/dev/null 2>&1
  for i in $(seq 1 40); do
    kill -0 "$cpid" 2>/dev/null || break
    sleep 0.5
  done
  kill -0 "$cpid" 2>/dev/null && { kill "$cpid" 2>/dev/null; echo "  （codex pid ${cpid} を自分で落とした）"; }
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
}

for arm in $ARMS; do
  case "$arm" in
    headless-*) run_headless "$arm" ;;
    edge) run_edge ;;
    new) run_gui ;;
    *) echo "知らない腕: $arm"; exit 2 ;;
  esac
done

echo
echo "PASS=${PASS} FAIL=${FAIL}$([ "$UNMEASURED" = 1 ] && echo '（GUI の腕は未実測）')"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 4
exit 0
