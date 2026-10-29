#!/usr/bin/env bash
# #2473: 再起動をまたいで復元した worker（器が死んで `claude … --resume <id>` / `codex resume … <id>` で
# 起こし直す経路）と `tako sessions resume` の起動引数が spawn と同じかを、隔離 GUI の実経路で並べて比べる。
#
# 段取り（腕ごとに 1 回。腕ごとに置き場・器のソケットを分ける）:
#   ① 隔離 GUI（器 = TAKO_PERSIST=1・仮想ディスプレイ tako-vd）から worker を 2 本 spawn する（claude / codex）。
#      どちらも偽物 = 起動の引数・役割・プロファイルの env を記録するだけ（本物の claude / codex・アカウントの枠は使わない）
#   ② 会話 ID が layout.json に載るのを待ち、GUI を止めて器（tmux サーバー）ごと落とす = PC の再起動
#   ③ 同じ置き場で GUI を起こし直す → 復元が `claude … --resume <id>` / `codex resume … <id>` を打つ
#   ④ claude の会話を `tako sessions resume` でもう 1 本開く（codex の会話はカタログに載らないので断られる）
#   ⑤ spawn と再開の起動引数（再開の書式 = `resume` / `--resume <id>` を除く）・役割・プロファイルの env を比べる
#
# 腕: new    … 本番の worker と同じ形のプロファイル（worker_agents.claude.args = --permission-mode auto・
#              model / effort・remote_control: true・env・worker_agents.codex の model / effort / args）。
#              再開の引数が spawn と一致し、許可スキップが付かず、--permission-mode auto が付くことを期待
#     plain  … 何も足していないプロファイル（remote_control なし・args なし）。一致し、余計な引数が付かないことを期待
#     legacy … new と同じプロファイルで TAKO_2473_LEGACY=1（修正前の復元 = カタログのメタだけ = 一致しないのが症状）
# 修正前のバイナリで `ARMS=new` を回すと ⑤ の一致が NG になる（= 修正前の症状）。
#
# 本物の設定（~/.codex・~/.claude・projects.yaml・tako の data）には触らない: 一時の HOME / CODEX_HOME /
# CLAUDE_CONFIG_DIR、隔離の data / orchestrator / discovery / tmux ソケット。**CI には登録しない**（tako-vd が要る）。
# tako-vd が描画できない（蓋閉じ）と終了コード 4 = 未実測。
#
# 使い方: scripts/test-resume-launch-args-2473.sh             # 既定 = new plain legacy
#         ARMS=new scripts/test-resume-launch-args-2473.sh    # 修正前のバイナリで症状を見る
#         KEEP=1 …（置き場を残す）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ARMS="${ARMS:-new plain legacy}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux jq lsof; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

# 本番 GUI を指す env・呼び出し元の印・本物の設定の置き場を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  TAKO_2412_LEGACY TAKO_2418_LEGACY TAKO_2447_LEGACY TAKO_2473_LEGACY TAKO_1967_LEGACY \
  CODEX_HOME OPENAI_API_KEY CLAUDE_CONFIG_DIR ZDOTDIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT

# HOME を差し替える前に建てる（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

TMP_ROOTS=""
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local d
  # 器はソケットファイルまで消す（#2399）。偽物の sleep は器と一緒に HUP で終わる
  # shellcheck disable=SC2086 # 名前の並びを語へ分ける
  tmux_socket_kill $SOCKETS
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do
      case "$d" in /tmp/t2473-* | /private/tmp/t2473-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の claude（API を呼ばない。起動の引数・役割・プロファイルの env を claude.log へ書き、
# `agents --json` に生きている自分を答える = tako が会話 ID を layout.json へ保存できる）
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
echo "ROLE=${TAKO_ORCHESTRATOR_ROLE:-} ENV=${T2473_ENV:-} ARGS=$*" >>"$FAKE_DIR/claude.log"
[ -n "$sid" ] || sid=$(printf '%08x-2473-4000-8000-%012x' "$$" "$(date +%s)")
# 会話の記録（復元は記録が実在するときだけ起こす = #652）
slug=$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')
mkdir -p "$CLAUDE_CONFIG_DIR/projects/$slug" "$FAKE_DIR/agents"
[ -f "$CLAUDE_CONFIG_DIR/projects/$slug/$sid.jsonl" ] || echo '{}' >"$CLAUDE_CONFIG_DIR/projects/$slug/$sid.jsonl"
echo "$sid|$(pwd -P)" >"$FAKE_DIR/agents/$$"
trap 'rm -f "${FAKE_DIR:?}/agents/$$"; exit 0' TERM INT HUP
echo "FAKECLAUDE-2473 $sid"
while :; do sleep 5; done
FAKE
  chmod +x "$1"
}

# 偽の codex（API を呼ばない。起動の引数・役割・env を codex.log へ書く）。tako が会話 ID を拾えるよう、
# 本物と同じく `$CODEX_HOME/thread-writer-locks/<id>.lock` を開いたまま持ち、rollout の名前の
# ファイルを置く（codex_session.rs）。argv[0] を codex にして待つ（tako は実行ファイル名で codex を見分ける）
write_fake_codex() {
  cat >"$1" <<'FAKE'
#!/bin/bash
case "${1:-}" in
  --version | -V) echo "codex-cli 0.154.0"; exit 0 ;;
esac
sid=""
if [ "${1:-}" = resume ]; then
  for a in "$@"; do sid="$a"; done
fi
echo "ROLE=${TAKO_ORCHESTRATOR_ROLE:-} ENV=${T2473_ENV:-} ARGS=$*" >>"$FAKE_DIR/codex.log"
[ -n "$sid" ] || sid=$(printf '%08x-2473-4000-8000-%012x' "$$" "$(date +%s)")
d="$CODEX_HOME/sessions/2026/10/28"
mkdir -p "$d" "$CODEX_HOME/thread-writer-locks"
[ -f "$d/rollout-2026-10-28T00-00-00-$sid.jsonl" ] || echo '{}' >"$d/rollout-2026-10-28T00-00-00-$sid.jsonl"
exec 3>>"$CODEX_HOME/thread-writer-locks/$sid.lock"
echo "FAKECODEX-2473 $sid"
exec -a codex /bin/sleep 86400
FAKE
  chmod +x "$1"
}

# 腕ごとのプロファイル（default.yaml。spawn 元のペインは master の役割を持たない = 既定のプロファイル）。
# `bypass_sandbox: false` は本番のプロファイルと同じく明示する（行が無いと #981 の移行が「#981 より前の
# ファイル」と読んで true を書き、codex の spawn にサンドボックス解除が付く = spawn 側の既存の仕様）
write_profile() {
  local arm="$1" path="$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml"
  if [ "$arm" = plain ]; then
    cat >"$path" <<YAML
env:
  CODEX_HOME: "$CH"
  CLAUDE_CONFIG_DIR: "$CC"
bypass_sandbox: false
YAML
    return
  fi
  cat >"$path" <<YAML
env:
  CODEX_HOME: "$CH"
  CLAUDE_CONFIG_DIR: "$CC"
  T2473_ENV: from-profile
bypass_sandbox: false
remote_control: true
worker_agents:
  claude:
    model: claude-fake-2473
    effort: xhigh
    args:
    - --permission-mode
    - auto
  codex:
    model: gpt-fake-2473
    effort: medium
    args:
    - --search
YAML
}

# 腕ごとの砂場
setup_sandbox() {
  local arm="$1"
  TMP="$(mktemp -d /tmp/t2473-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2473-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm} ==="
  GHOME="$TMP/home" CH="$TMP/codex-home" CC="$TMP/claude-config" P1="$TMP/p1" BIN="$TMP/bin"
  FAKE_DIR="$TMP/fake"
  mkdir -p "$GHOME" "$CH" "$CC" "$P1" "$BIN" "$FAKE_DIR/agents"
  write_fake_claude "$BIN/claude"
  write_fake_codex "$BIN/codex"
  : >"$FAKE_DIR/claude.log"
  : >"$FAKE_DIR/codex.log"
  echo '{}' >"$CC/.claude.json"
  : >"$CH/config.toml"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  export TAKO_ISOLATED=1
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  write_profile "$arm"
  "$TAKO_BIN" orchestrator projects add --key p1 --cwd "$P1" --description "#2473 の検証" >/dev/null 2>&1
}

# GUI を起こす（腕の env + 器 ON・起動待ちは旧挙動 = 復元をその場で打つ）。起こせなければ UNMEASURED=1 で 1
start_gui() {
  local log="$1"
  shift
  if ! launch_isolated_gui "$log" "HOME=$GHOME" "SHELL=/bin/zsh" "TAKO_PERSIST=1" \
    "FAKE_DIR=$FAKE_DIR" "CODEX_HOME=$CH" "CLAUDE_CONFIG_DIR=$CC" \
    "TAKO_2413_LEGACY=1" "PATH=$BIN:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" ${@+"$@"}; then
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

# layout.json に保存された会話 ID（codex = agent_resume.id・claude = claude_session_id）
saved_id() {
  jq -r --arg a "$1" '
    if $a == "claude" then [.. | objects | .claude_session_id? | strings]
    else [.. | objects | .agent_resume? | objects | select(.agent == $a) | .id | strings] end
    | .[0] // empty' "$TAKO_DATA_DIR/layout.json" 2>/dev/null
}

# 記録の n 行目（1 始まり）。無ければ空
nth_line() { sed -n "${2}p" "$1" 2>/dev/null; }

# 記録 1 行から「再開の書式」を外した引数（codex = 先頭の resume と末尾の ID・claude = --resume <ID>）
args_of() {
  local line="$1" agent="$2" args
  args="${line#* ARGS=}"
  if [ "$agent" = codex ]; then
    printf '%s' "$args" | sed -E 's/^resume //; s/ [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$//'
  else
    printf '%s' "$args" | sed -E 's/ --resume [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$//'
  fi
}
role_of() { local x="${1#ROLE=}"; printf '%s' "${x%% ENV=*}"; }
env_of() { local x="${1#* ENV=}"; printf '%s' "${x%% ARGS=*}"; }

# 引数の有無（語の並びとして含むか）
has() { case " $1 " in *" $2 "*) echo あり ;; *) echo なし ;; esac; }
# 値付きの引数の値（`--model X` の X）
val() { local rest; case " $1 " in *" $2 "*) rest="${1#*"$2" }"; printf '%s' "${rest%% *}" ;; *) printf '%s' '-' ;; esac; }

# codex の effort（`-c model_reasoning_effort=X`）。無ければ -
effort_of() {
  local v
  v="$(printf '%s' "$1" | sed -n 's/.*\(model_reasoning_effort=[a-z]*\).*/\1/p')"
  printf '%s' "${v:--}"
}
# 正規表現に当たる箇所の数（pipefail の下でも件数だけを見る）
count_of() { printf '%s' "$1" | grep -o "$2" | wc -l | tr -d ' '; }

# 比較表の 1 行
row() {
  local name="$1" s="$2" r="$3" mark
  [ "$s" = "$r" ] && mark=一致 || mark=不一致
  printf '    | %-44s | %-22s | %-22s | %s |\n' "$name" "$s" "$r" "$mark"
}

# spawn と再開の 1 組を表にして、一致を判定する。$4 = 再開の経路の名前
compare() {
  local agent="$1" spawn="$2" resume="$3" label="$4" sa ra
  sa="$(args_of "$spawn" "$agent")"
  ra="$(args_of "$resume" "$agent")"
  echo "  [${agent} / ${label}]"
  echo "    spawn: $(printf '%s' "$spawn" | sed "s#$TMP#<tmp>#g" | cut -c1-300)"
  echo "    再開 : $(printf '%s' "$resume" | sed "s#$TMP#<tmp>#g" | cut -c1-300)"
  printf '    | %-44s | %-22s | %-22s | %s |\n' 引数 spawn 再開 判定
  row "TAKO_ORCHESTRATOR_ROLE" "$(role_of "$spawn")" "$(role_of "$resume")"
  row "プロファイルの env（T2473_ENV）" "$(env_of "$spawn")" "$(env_of "$resume")"
  row "--model" "$(val "$sa" --model)" "$(val "$ra" --model)"
  if [ "$agent" = claude ]; then
    row "--effort" "$(val "$sa" --effort)" "$(val "$ra" --effort)"
    row "--permission-mode（agent_args）" "$(val "$sa" --permission-mode)" "$(val "$ra" --permission-mode)"
    row "--remote-control" "$(has "$sa" --remote-control)" "$(has "$ra" --remote-control)"
    row "--dangerously-skip-permissions" "$(has "$sa" --dangerously-skip-permissions)" "$(has "$ra" --dangerously-skip-permissions)"
    row "--settings（#2412）" "$(has "$sa" --settings)" "$(has "$ra" --settings)"
  else
    row "-c model_reasoning_effort" "$(effort_of "$sa")" "$(effort_of "$ra")"
    row "--search（agent_args）" "$(has "$sa" --search)" "$(has "$ra" --search)"
    row "--dangerously-bypass-approvals-and-sandbox" "$(has "$sa" --dangerously-bypass-approvals-and-sandbox)" "$(has "$ra" --dangerously-bypass-approvals-and-sandbox)"
    row "-c mcp_servers.tako.*（#986）" "$(count_of "$sa" 'mcp_servers\.tako\.')" "$(count_of "$ra" 'mcp_servers\.tako\.')"
  fi
  if [ "$sa" = "$ra" ] && [ "$(role_of "$spawn")" = "$(role_of "$resume")" ] && [ "$(env_of "$spawn")" = "$(env_of "$resume")" ]; then
    MATCH=1
  else
    MATCH=0
    echo "    引数の差（< spawn / > 再開）:"
    # shellcheck disable=SC2086 # 引数を語へ分けて 1 行 1 つに並べる
    diff <(printf '%s\n' $sa) <(printf '%s\n' $ra) | sed -n "/^[<>]/{s#$TMP#<tmp>#g; s/^/      /; p;}"
  fi
}

run_arm() {
  local arm="$1" legacy="" master resp pane_cl pane_cx i id_cl id_cx
  setup_sandbox "$arm"
  [ "$arm" = legacy ] && legacy="TAKO_2473_LEGACY=1"

  # ---- ① spawn ----
  start_gui "$TMP/app1.log" ${legacy:+"$legacy"} || return
  if [ "$("$TAKO_BIN" list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  sleep 3
  master="$("$TAKO_BIN" list | jq -r '.tabs[0].panes[0].id')"
  resp="$("$TAKO_BIN" orchestrator spawn --project p1 --pane "$master" --label cl --agent claude \
    --prompt 'hello 2473' 2>&1)"
  pane_cl="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  resp="$("$TAKO_BIN" orchestrator spawn --project p1 --pane "$master" --label cx --agent codex \
    --prompt 'hello 2473' 2>&1)"
  pane_cx="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  echo "  spawn: claude=${pane_cl:-失敗} codex=${pane_cx:-失敗}"
  if [ -z "$pane_cl" ] || [ -z "$pane_cx" ]; then
    bad "[${arm}] spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
    stop_gui
    return
  fi
  for i in $(seq 1 60); do
    [ -n "$(nth_line "$FAKE_DIR/claude.log" 1)" ] && [ -n "$(nth_line "$FAKE_DIR/codex.log" 1)" ] && break
    sleep 0.5
  done
  check "$([ -n "$(nth_line "$FAKE_DIR/claude.log" 1)" ] && [ -n "$(nth_line "$FAKE_DIR/codex.log" 1)" ] && echo 1 || echo 0)" \
    "[${arm}] spawn した 2 本の起動を偽物が記録した"

  # ---- ② 会話 ID の保存を待って器ごと落とす ----
  for i in $(seq 1 90); do
    [ -n "$(saved_id claude)" ] && [ -n "$(saved_id codex)" ] && break
    sleep 1
  done
  id_cl="$(saved_id claude)"
  id_cx="$(saved_id codex)"
  check "$([ -n "$id_cl" ] && [ -n "$id_cx" ] && echo 1 || echo 0)" \
    "[${arm}] 会話 ID が layout.json に保存された（claude ${id_cl:-なし} / codex ${id_cx:-なし}）"
  stop_gui
  tmux_socket_kill "$sock"

  # ---- ③ 起こし直す = 復元 ----
  start_gui "$TMP/app2.log" ${legacy:+"$legacy"} || return
  for i in $(seq 1 120); do
    [ -n "$(nth_line "$FAKE_DIR/claude.log" 2)" ] && [ -n "$(nth_line "$FAKE_DIR/codex.log" 2)" ] && break
    sleep 0.5
  done
  grep '復元の内訳' "$TAKO_DATA_DIR/persist.log" 2>/dev/null | tail -2 | sed 's/^.*復元/    復元/' | cut -c1-200
  local spawn_cl spawn_cx restore_cl restore_cx
  spawn_cl="$(nth_line "$FAKE_DIR/claude.log" 1)"
  spawn_cx="$(nth_line "$FAKE_DIR/codex.log" 1)"
  restore_cl="$(nth_line "$FAKE_DIR/claude.log" 2)"
  restore_cx="$(nth_line "$FAKE_DIR/codex.log" 2)"
  check "$([ -n "$restore_cl" ] && [ -n "$restore_cx" ] && echo 1 || echo 0)" \
    "[${arm}] 復元が claude / codex を起こし直した"

  # ---- ④ tako sessions resume（claude）。codex の会話はカタログの本体に載らないので断られる ----
  master="$("$TAKO_BIN" list | jq -r '[.tabs[].panes[] | select((.role // "") | startswith("orchestrator-worker") | not)] | .[0].id // empty')"
  resp="$("$TAKO_BIN" sessions resume "$id_cl" --pane "$master" 2>&1)"
  for i in $(seq 1 60); do
    [ -n "$(nth_line "$FAKE_DIR/claude.log" 3)" ] && break
    sleep 0.5
  done
  local sessions_cl
  sessions_cl="$(nth_line "$FAKE_DIR/claude.log" 3)"
  echo "  tako sessions resume（claude）: $(printf '%s' "$resp" | sed -n '1p' | sed "s#$TMP#<tmp>#g" | cut -c1-160)"
  echo "  tako sessions resume（codex）: $("$TAKO_BIN" sessions resume "$id_cx" --pane "$master" 2>&1 | head -1 | cut -c1-160)"

  # ---- ⑤ 並べて比べる ----
  local m_cl m_cx m_sr
  compare claude "$spawn_cl" "$restore_cl" "GUI の復元"
  m_cl=$MATCH
  compare codex "$spawn_cx" "$restore_cx" "GUI の復元"
  m_cx=$MATCH
  compare claude "$spawn_cl" "$sessions_cl" "tako sessions resume"
  m_sr=$MATCH

  local ra_cl ra_cx
  ra_cl="$(args_of "$restore_cl" claude)"
  ra_cx="$(args_of "$restore_cx" codex)"
  check "$([ "$(has "$ra_cl" --dangerously-skip-permissions)" = なし ] && echo 1 || echo 0)" \
    "[${arm}] 復元した claude に許可スキップ（--dangerously-skip-permissions）が付かない"
  check "$([ "$(has "$ra_cx" --dangerously-bypass-approvals-and-sandbox)" = なし ] && echo 1 || echo 0)" \
    "[${arm}] 復元した codex にサンドボックス解除（--dangerously-bypass-approvals-and-sandbox）が付かない"
  check "$m_sr" "[${arm}] tako sessions resume の claude は spawn と同じ引数（#1967）"
  case "$arm" in
    new)
      check "$m_cl" "[new] GUI の復元の claude は spawn と同じ引数・役割・env"
      check "$m_cx" "[new] GUI の復元の codex は spawn と同じ引数・役割・env"
      check "$([ "$(val "$ra_cl" --permission-mode)" = auto ] && echo 1 || echo 0)" \
        "[new] 復元した claude に --permission-mode auto が付く"
      ;;
    plain)
      check "$m_cl" "[plain] GUI の復元の claude は spawn と同じ引数・役割・env"
      check "$m_cx" "[plain] GUI の復元の codex は spawn と同じ引数・役割・env"
      check "$([ "$(has "$ra_cl" --remote-control)" = なし ] && [ "$(has "$ra_cl" --permission-mode)" = なし ] && echo 1 || echo 0)" \
        "[plain] プロファイルに無いもの（--remote-control / --permission-mode）は付かない"
      ;;
    legacy)
      check "$([ "$m_cl" = 0 ] && [ "$(has "$ra_cl" --permission-mode)" = なし ] && echo 1 || echo 0)" \
        "[legacy] 修正前の復元の claude は spawn と違い --permission-mode が落ちる（症状）"
      check "$([ "$m_cx" = 0 ] && echo 1 || echo 0)" \
        "[legacy] 修正前の復元の codex は spawn と違う（症状）"
      ;;
  esac
  stop_gui
  tmux_socket_kill "$sock"
}

for arm in $ARMS; do
  case "$arm" in
    new | plain | legacy) run_arm "$arm" ;;
    *) echo "知らない腕: $arm"; exit 2 ;;
  esac
done

echo
echo "PASS=${PASS} FAIL=${FAIL}$([ "$UNMEASURED" = 1 ] && echo '（未実測の腕あり）')"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 4
exit 0
