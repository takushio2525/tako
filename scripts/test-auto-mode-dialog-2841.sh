#!/usr/bin/env bash
# #2841: claude の auto mode の確認「Auto mode can't check actions in this session」を、本物の Claude Code と
# 偽の Messages API で出し、隔離 GUI（仮想ディスプレイ tako-vd）の実経路で master から見えて答えられるかを測る。
#
# 構成: 本物の claude（既定は PATH の claude。CLAUDE_BIN で版を指定）を一時の CLAUDE_CONFIG_DIR + 偽の
# Messages API（scripts/lib/fake-messages-api.mjs・ダミーの API キー = 本物の認証は使わない）へ繋ぎ、
# 隔離 GUI のペインで `--permission-mode auto` で起こす。偽の API はゲートウェイ（ANTHROPIC_BASE_URL）なので
# auto mode の安全確認が届かず、確認が要る最初の操作（`Run this command: python3 -c ...`）でこの確認が出る。
#
# 段（腕 new = 修正後）:
#   ① 確認が出たら `tako orchestrator status` が waiting + choice_dialog（kind=auto_mode_unchecked・キーつきの 3 択・
#      note = 原因と答え方）を返す。同じ照会を MCP（`tako mcp serve` の tako_orchestrator_worker_status）でも引く
#   ② `tako orchestrator watch` が WORKER_DIALOG（auto_mode_unchecked）と案内の note を出す
#   ③ 確認の表示中の `tako send` は断る（入力欄が無い）
#   ④ `tako orchestrator respond`（下見）→ `--choice "stay in auto mode"` が Enter を 1 つ送って解消し、
#      続く操作ごとの承認を watch が WORKER_PERMISSION で出し、`--choice yes` で先へ進む（RUN-DONE）
#   ⑤ 新しい会話で MCP の tako_orchestrator_respond（choice=cancel）が Esc を送ってターンが止まる（auto mode のまま）
#   ⑥ 次の操作で出た同じ確認へ CLI の `--choice leave` が shift+Tab（BTab）を送り accept edits へ移る
# 腕 legacy（TAKO_2841_LEGACY=1 を GUI へ）: ① の照会が choice_dialog=null で waiting にならず、respond は断り、
#   watch は WORKER_DIALOG を出さない（修正前の「master から見えない」）
#
# 本番の GUI / tmux / data dir / claude の設定 dir には触らない（HOME も一時 dir・claude の設定は一時の
# CLAUDE_CONFIG_DIR）。**CI には登録しない**（本物の claude と tako-vd が要る）。tako-vd が描画できない
# （蓋閉じ）と終了コード 4 = 未実測。
#
# 使い方: scripts/test-auto-mode-dialog-2841.sh            # 既定 = new legacy
#         ARMS=new scripts/test-auto-mode-dialog-2841.sh
#         CLAUDE_BIN=~/.local/share/claude/versions/2.1.322 …（版を指定）
#         KEEP=1 …（置き場を残す。応答の全文は <置き場>/responses/ に入る）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ARMS="${ARMS:-new legacy}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in jq node python3; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done
# 本物の claude の実体（HOME を差し替える前に引く。symlink は実体へ）
CLAUDE_BIN="${CLAUDE_BIN:-$(command -v claude 2>/dev/null)}"
[ -n "$CLAUDE_BIN" ] && [ -x "$CLAUDE_BIN" ] || { echo "未実測: claude が無い（CLAUDE_BIN で指定できる）"; exit 4; }
CLAUDE_BIN="$(python3 -I -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$CLAUDE_BIN")"
CLAUDE_VERSION="$("$CLAUDE_BIN" --version 2>/dev/null | head -1)"

# 本番 GUI を指す env・呼び出し元の印・本物の設定の置き場を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL ZDOTDIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT \
  CLAUDE_CODE_BRIDGE_SESSION_ID CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_MESSAGING_TOKEN \
  CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_EXECPATH CLAUDE_PID CLAUDE_EFFORT
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

# HOME を差し替える前に建てる（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

API_KEY="sk-ant-api03-fake2841fake2841fake2841fake2841fake2841fake2841AA"
TMP_ROOTS=""
SOCKETS=""
OWN_PIDS=""
cleanup() {
  local d pid
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  # 自分が起こした偽の API だけを止める（名前一致の一括 kill はしない）
  for pid in $OWN_PIDS; do kill "$pid" 2>/dev/null; done
  # shellcheck disable=SC2086 # 名前の並びを語へ分ける
  tmux_socket_kill $SOCKETS
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do
      case "$d" in /tmp/t2841-* | /private/tmp/t2841-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 腕ごとの砂場（置き場・偽の API・claude の一時の設定 dir）
setup_sandbox() {
  local arm="$1" i
  TMP="$(mktemp -d /tmp/t2841-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2841-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm}（${CLAUDE_VERSION}）==="
  GHOME="$TMP/home" RESP="$TMP/responses" WORK="$TMP/work" CCD="$TMP/cfg"
  mkdir -p "$GHOME" "$RESP" "$WORK" "$CCD"

  FAKE_LOG="$TMP/fake.log" node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 >"$TMP/fake.out" 2>&1 &
  OWN_PIDS="$OWN_PIDS $!"
  PORT=""
  for i in $(seq 1 50); do
    PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$TMP/fake.out" 2>/dev/null)"
    [ -n "$PORT" ] && break
    sleep 0.1
  done
  [ -n "$PORT" ] || { echo "偽の API が立たない: $(cat "$TMP/fake.out")"; exit 1; }
  API_URL="http://127.0.0.1:${PORT}"
  # 一時の設定 dir: オンボーディング済み・ダミーの鍵を承認済み・作業 dir を信頼済み（本物の認証は写さない）
  jq -n --arg key "$(printf '%s' "$API_KEY" | tail -c 20)" --arg work "$WORK" '{
    hasCompletedOnboarding: true, theme: "dark",
    customApiKeyResponses: { approved: [$key], rejected: [] },
    projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
  }' >"$CCD/.claude.json"

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
  local log="$1" arm="$2"
  local -a arm_env=()
  [ "$arm" = legacy ] && arm_env+=("TAKO_2841_LEGACY=1")
  # GUI 自身が claude を起こす口（版の判定・自動リネーム）も一時の設定 dir と偽の API へ向ける
  if ! launch_isolated_gui "$log" "HOME=$GHOME" "SHELL=/bin/zsh" "TAKO_PERSIST=0" \
    "CLAUDE_CONFIG_DIR=$CCD" "ANTHROPIC_BASE_URL=$API_URL" "ANTHROPIC_API_KEY=$API_KEY" \
    "DISABLE_AUTOUPDATER=1" ${arm_env[@]+"${arm_env[@]}"}; then
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

# 置き場のパス・偽の API のポートを伏せて 1 行に縮める
mask() { sed -e "s#$TMP#<tmp>#g" -e "s#127.0.0.1:${PORT}#127.0.0.1:<port>#g" | tr '\n' ' ' | cut -c1-"${1:-240}"; }

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

# MCP の 1 回の呼び出し（`tako mcp serve`）。$1 = ツール名・$2 = arguments の JSON。text の中身を出す
mcp_call() {
  local sock token
  sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  token="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2841","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" |
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

# 応答の欄（jq の式。null・欄が無いときだけ「なし」。jq の `//` は false も「無い」と読むので使わない）
field() { jq -r "($1) | if . == null then \"なし\" else tostring end" <<<"$2" 2>/dev/null; }

# $2 に $1 が含まれるか（pipefail の下で出力をパイプで grep -q へ渡さない = 早く抜ける読み手で書き手が落ちない）
has() { grep -qF -- "$1" <<<"$2"; }

# ペインの画面（素の文字）
screen_of() { iso_cli read --pane "$1" 2>/dev/null; }

# ペインの画面に $2 が出るまで待つ（$3 秒）。式は wait_until の中で評価されるので、位置引数ではなく
# この関数の local（bash は呼び先から見える）で渡す
wait_screen() {
  local ws_pane="$1" ws_text="$2"
  wait_until "$3" 'has "$ws_text" "$(screen_of "$ws_pane")"'
}

# 状態照会の choice_dialog.kind が $2 になるまで待つ（$3 秒。local の渡し方は wait_screen と同じ）
wait_dialog_kind() {
  local wd_pane="$1" wd_kind="$2"
  wait_until "$3" '[ "$(iso_cli orchestrator status --pane "$wd_pane" 2>/dev/null | jq -r ".choice_dialog.kind // empty")" = "$wd_kind" ]'
}

# ペインで claude を auto mode で起こし、入力欄が出るまで待つ
start_claude() {
  local pane="$1"
  iso_cli send --pane "$pane" "cd '$WORK' && clear && env CLAUDE_CONFIG_DIR='$CCD' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 '$CLAUDE_BIN' --model haiku --permission-mode auto" >/dev/null 2>&1
  wait_screen "$pane" "auto mode on" 60
}

# 確認が要る操作を頼む（偽の API が Bash の tool_use を返す）
ask_action() {
  iso_cli send --pane "$1" "Run this command: python3 -c \"print($2)\"" >"$RESP/send-$2.json" 2>&1
}

run_new() {
  local pane out st mcp watch_out keys rc
  pane="$(iso_cli list | jq -r '.tabs[0].panes[0].id')"
  start_claude "$pane"
  if ! has "auto mode on" "$(screen_of "$pane")"; then
    bad "[new] ペインの claude が auto mode で起きない: $(screen_of "$pane" | tail -6 | mask 300)"
    return
  fi
  pass "[new] ペインの claude（${CLAUDE_VERSION}）が auto mode で起きた"

  # ---- ① 状態照会 ----
  ask_action "$pane" 2841
  wait_dialog_kind "$pane" auto_mode_unchecked 60
  st="$(iso_cli orchestrator status --pane "$pane" 2>&1)"
  printf '%s\n' "$st" >"$RESP/status-dialog.json"
  echo "    status: $(jq -c '{status, kind: .choice_dialog.kind, options: [.choice_dialog.options[] | "\(.label) [\(.key)]"], highlighted: .choice_dialog.highlighted, cursor_visible: .choice_dialog.cursor_visible, permission_dialog}' <<<"$st" 2>/dev/null | mask 400)"
  echo "    title: $(field '.choice_dialog.title' "$st" | mask 300)"
  check "$([ "$(field .status "$st")" = waiting ] && [ "$(field .choice_dialog.kind "$st")" = auto_mode_unchecked ] && echo 1 || echo 0)" \
    "[new] ① status が waiting + choice_dialog.kind=auto_mode_unchecked（status=$(field .status "$st")・kind=$(field .choice_dialog.kind "$st")）"
  keys="$(jq -r '[.choice_dialog.options[].key] | join(",")' <<<"$st" 2>/dev/null)"
  check "$([ "$keys" = "Enter,BTab,Escape" ] && [ "$(field .choice_dialog.highlighted "$st")" = 0 ] && echo 1 || echo 0)" \
    "[new] ① 選択肢のキーは Enter,BTab,Escape・既定（highlighted）は 0 = stay in auto mode（keys=${keys}）"
  check "$(has "stay in auto mode" "$(field .choice_dialog.note "$st")" && echo 1 || echo 0)" \
    "[new] ① choice_dialog.note に原因と答え方が載る（$(field .choice_dialog.note "$st" | cut -c1-80)…）"
  mcp="$(mcp_call tako_orchestrator_worker_status "{\"pane_id\":${pane}}")"
  printf '%s\n' "$mcp" >"$RESP/status-mcp.json"
  check "$([ "$(field .status "$mcp")" = waiting ] && [ "$(field .choice_dialog.kind "$mcp")" = auto_mode_unchecked ] && [ "$(jq -c .choice_dialog.options <<<"$mcp")" = "$(jq -c .choice_dialog.options <<<"$st")" ] && echo 1 || echo 0)" \
    "[new] ① MCP の tako_orchestrator_worker_status も同じ（status=$(field .status "$mcp")・kind=$(field .choice_dialog.kind "$mcp")）"

  # ---- ② watch ----
  watch_out="$(iso_cli orchestrator watch --pane "$pane" --timeout 30 2>&1)"
  printf '%s\n' "$watch_out" >"$RESP/watch-dialog.txt"
  printf '%s\n' "$watch_out" | sed -n '1,12p' | mask 2000 | sed 's/^/    watch: /'
  echo
  check "$(has "WORKER_DIALOG: tako:${pane} (auto_mode_unchecked)" "$(head -1 <<<"$watch_out")" && echo 1 || echo 0)" \
    "[new] ② watch が WORKER_DIALOG: tako:${pane} (auto_mode_unchecked) を出す"
  check "$(has "stay in auto mode" "$(grep -F "note:" <<<"$watch_out")" && has "[BTab]" "$watch_out" && echo 1 || echo 0)" \
    "[new] ② watch に選択肢のキー（[BTab] ほか）と原因・答え方の note が載る"

  # ---- ③ 送信の断り ----
  out="$(iso_cli send --pane "$pane" "hello" 2>&1)"
  rc=$?
  check "$([ "$rc" -ne 0 ] && has "stay in auto mode" "$out" && echo 1 || echo 0)" \
    "[new] ③ 確認の表示中の send は選択肢つきで断る（終了コード ${rc}・$(printf '%s' "$out" | mask 160)）"

  # ---- ④ respond（下見 → stay → 操作ごとの承認 → yes）----
  out="$(iso_cli orchestrator respond --pane "$pane" 2>&1)"
  printf '%s\n' "$out" >"$RESP/respond-preview.json"
  check "$([ "$(field .responded "$out")" = false ] && [ "$(field .kind "$out")" = auto_mode_unchecked ] && echo 1 || echo 0)" \
    "[new] ④ respond の下見は何も送らず構造を返す（responded=$(field .responded "$out")）"
  out="$(iso_cli orchestrator respond --pane "$pane" --choice "stay in auto mode" 2>&1)"
  printf '%s\n' "$out" >"$RESP/respond-stay.json"
  check "$([ "$(field .resolved "$out")" = true ] && [ "$(jq -c .keys_sent <<<"$out" 2>/dev/null)" = '["Enter"]' ] && echo 1 || echo 0)" \
    "[new] ④ respond --choice \"stay in auto mode\" が Enter を 1 つ送り解消（resolved=$(field .resolved "$out")・keys_sent=$(jq -c .keys_sent <<<"$out" 2>/dev/null)）"
  wait_dialog_kind "$pane" permission 30
  watch_out="$(iso_cli orchestrator watch --pane "$pane" --timeout 30 2>&1)"
  printf '%s\n' "$watch_out" >"$RESP/watch-permission.txt"
  check "$(has "WORKER_PERMISSION: tako:${pane}" "$(head -1 <<<"$watch_out")" && echo 1 || echo 0)" \
    "[new] ④ 留まった後の操作ごとの承認は WORKER_PERMISSION（$(printf '%s\n' "$watch_out" | head -1)）"
  out="$(iso_cli orchestrator respond --pane "$pane" --choice yes 2>&1)"
  printf '%s\n' "$out" >"$RESP/respond-yes.json"
  wait_screen "$pane" "RUN-DONE" 30
  check "$([ "$(field .resolved "$out")" = true ] && has "RUN-DONE" "$(screen_of "$pane")" && echo 1 || echo 0)" \
    "[new] ④ respond --choice yes で操作が通り先へ進む（keys_sent=$(jq -c .keys_sent <<<"$out" 2>/dev/null)・RUN-DONE）"

  # ---- ⑤ 新しい会話で MCP の respond（cancel = Esc）----
  iso_cli send --pane "$pane" "/exit" >/dev/null 2>&1
  wait_until 30 '! has "auto mode on" "$(screen_of "$pane" | tail -3)"'
  start_claude "$pane"
  ask_action "$pane" 2842
  wait_dialog_kind "$pane" auto_mode_unchecked 60
  out="$(mcp_call tako_orchestrator_respond "{\"pane_id\":${pane},\"choice\":\"cancel\"}")"
  printf '%s\n' "$out" >"$RESP/respond-mcp-cancel.json"
  wait_screen "$pane" "Interrupted" 20
  st="$(screen_of "$pane")"
  check "$([ "$(field .resolved "$out")" = true ] && [ "$(jq -c .keys_sent <<<"$out" 2>/dev/null)" = '["Escape"]' ] && has "Interrupted" "$st" && has "auto mode on" "$(tail -3 <<<"$st")" && echo 1 || echo 0)" \
    "[new] ⑤ MCP の tako_orchestrator_respond（cancel）が Esc を送りターンが止まる・auto mode のまま（keys_sent=$(jq -c .keys_sent <<<"$out" 2>/dev/null)）"

  # ---- ⑥ 次の操作で同じ確認 → CLI の leave（shift+Tab）----
  # 偽の API は中断された tool_result を含む次の発話には道具を呼ばずに答えるので、1 往復挟む
  iso_cli send --pane "$pane" "Reply with exactly one word: PING2841" >/dev/null 2>&1
  wait_screen "$pane" "PING2841" 30
  ask_action "$pane" 2843
  wait_dialog_kind "$pane" auto_mode_unchecked 60
  check "$([ "$(iso_cli orchestrator status --pane "$pane" 2>/dev/null | jq -r '.choice_dialog.kind // empty')" = auto_mode_unchecked ] && echo 1 || echo 0)" \
    "[new] ⑥ cancel の後の次の操作で同じ確認がまた出る"
  out="$(iso_cli orchestrator respond --pane "$pane" --choice leave 2>&1)"
  printf '%s\n' "$out" >"$RESP/respond-leave.json"
  wait_screen "$pane" "accept edits on" 20
  check "$([ "$(field .resolved "$out")" = true ] && [ "$(jq -c .keys_sent <<<"$out" 2>/dev/null)" = '["BTab"]' ] && has "accept edits on" "$(screen_of "$pane" | tail -3)" && echo 1 || echo 0)" \
    "[new] ⑥ respond --choice leave が shift+Tab（BTab）を送り accept edits へ移る（keys_sent=$(jq -c .keys_sent <<<"$out" 2>/dev/null)）"
  iso_cli send --pane "$pane" "/exit" >/dev/null 2>&1
}

run_legacy() {
  local pane st out watch_out
  pane="$(iso_cli list | jq -r '.tabs[0].panes[0].id')"
  start_claude "$pane"
  if ! has "auto mode on" "$(screen_of "$pane")"; then
    bad "[legacy] ペインの claude が auto mode で起きない"
    return
  fi
  ask_action "$pane" 2841
  # 修正前は照会に種別が出ないので、画面に確認が描かれるまで待つ
  wait_screen "$pane" "Enter to stay in auto mode" 60
  st="$(iso_cli orchestrator status --pane "$pane" 2>&1)"
  printf '%s\n' "$st" >"$RESP/status-dialog.json"
  echo "    status: $(jq -c '{status, status_source, choice_dialog, permission_dialog}' <<<"$st" 2>/dev/null | mask 300)"
  check "$([ "$(field .choice_dialog "$st")" = なし ] && [ "$(field .status "$st")" != waiting ] && echo 1 || echo 0)" \
    "[legacy] 修正前は確認の画面でも choice_dialog=null・waiting にならない（status=$(field .status "$st")）"
  out="$(iso_cli orchestrator respond --pane "$pane" --choice 1 2>&1)"
  check "$([ "$(field .resolved "$out")" = true ] && echo 0 || echo 1)" \
    "[legacy] 修正前の respond は答えられない（$(printf '%s' "$out" | mask 140)）"
  watch_out="$(iso_cli orchestrator watch --pane "$pane" --timeout 15 2>&1)"
  printf '%s\n' "$watch_out" >"$RESP/watch-dialog.txt"
  check "$(has "WORKER_DIALOG" "$watch_out" && echo 0 || echo 1)" \
    "[legacy] 修正前の watch は WORKER_DIALOG を出さない（$(printf '%s\n' "$watch_out" | head -1 | mask 120)）"
  # 片付け: 修正前は送信の断りも無いので生のキーで閉じる
  iso_cli send --pane "$pane" --key Escape >/dev/null 2>&1
  sleep 1
  iso_cli send --pane "$pane" "/exit" >/dev/null 2>&1
}

run_arm() {
  local arm="$1"
  setup_sandbox "$arm"
  start_gui "$TMP/app.log" "$arm" || return
  if [ "$(iso_cli list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  sleep 2
  case "$arm" in
    new) run_new ;;
    legacy) run_legacy ;;
  esac
  echo "    偽の API が受けた要求: $(wc -l <"$TMP/fake.log" | tr -d ' ') 件（Bash の tool_use $(grep -c '"tool":"Bash"' "$TMP/fake.log") 件）"
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
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
