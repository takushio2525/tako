#!/usr/bin/env bash
# test-session-scan-mod-2041.sh — 残りの `claude agents --json` 走査を mod の報告で埋めた効果の実経路テスト（#2041）
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）を**一時の設定 dir** と**偽の Messages API**
# （scripts/lib/fake-messages-api.mjs）で、隔離 GUI（tako-vd・自動リネーム off）のペインに立てる。
# 走査先は本番と同じ 4 本（既定 + 一時アカウント 3。既定 config dir も一時の HOME の下）。
# 同じバイナリの新旧（旧 = TAKO_2041_LEGACY=1）を同じ条件で測る。番号は Issue #2041 の受け入れ条件:
#   chat   1. GUI モード（`tako ui-mode gui`）で mod 入りの claude 4 本がチャット表示になっている間に
#             起こした `claude agents --json` の本数（GUI の PATH の先頭の `claude` の包みで数える）。
#             新は 0 本・旧は 30 秒ごとにアカウント数ぶん。4 本ともチャット表示（pane_display=chat・
#             chat_header のモデル）は新旧で同じ。新ではさらに mod の無い claude を 1 本足し、
#             走査がそのアカウントと既定だけに絞られ、そのペインもチャット表示になることを見る
#   clear  2. worker（workers.yaml に spawn 直後の会話 ID で登録）の claude で `/clear` すると、新は次の
#             `orchestrator status` で `agents_scan.registry_followed=true` になりレジストリが新しい会話 ID へ
#             更新され、以後 `skipped=true`・走査 0 本。旧は `session_mismatch` で走査する。
#             persist ON（器あり）と OFF（器なし = セッションカタログの 30 秒走査が止まる構成）の両方
#
# **本物の設定 dir（~/.claude*）には触らない**: HOME を一時 dir へ差し替えて GUI を立てる。claude の設定 dir は
# mktemp 配下（認証は写さず、偽 API とダミーの API キーを一時の .claude.json で承認済みにする）。最後に本物の
# ~/.claude* に検証で生まれた会話（会話 ID・作業フォルダ名）が 1 件も無いことを見る（この PC では並走中の本番の
# claude が ~/.claude を常に書くので、更新時刻の比較は物差しにならない）。本番 tako が配る env は最初に落とす。落とすのは自分で
# 起こした pid と自分の tmux ソケットだけ（pkill / killall は使わない）。窓は仮想ディスプレイ（tako-vd）へ出る。
#
# 使い方: bash scripts/test-session-scan-mod-2041.sh   （ONLY=chat|clear|clear_direct。既定は全部）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は単体テスト
# （`chat_view::tests::issue2041_*` / `dispatch::tests::issue2041_*` / `claude_mod::tests::issue2041_*`）と
# 番犬（issue2041_session_scan_mod_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_2017_LEGACY TAKO_2041_LEGACY \
  TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
SECS="${SECS:-60}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

command -v claude >/dev/null 2>&1 || { echo "未実測: claude が PATH に無い"; exit 4; }
command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 4; }

TMP="$(mktemp -d /tmp/tako-2041-XXXXXX)"
APP_PID=""
FAKE_PID=""
LOAD_PID=""
TMUX_SOCKET="tk2041-$$"
REAL_HOME="$HOME"

# 検証で生まれた会話 ID（mod の報告で見えたもの）。最後に本物の ~/.claude* に無いことを確かめる
SIDS="$TMP/sids.txt"
: > "$SIDS"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

cleanup() {
  [ -n "$LOAD_PID" ] && kill "$LOAD_PID" 2>/dev/null
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${TMUX_SOCKET:?}"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: <tmp> を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP PIPE

# **HOME を差し替える前に**バイナリを決める（無ければビルド。cargo / rustup は本物の HOME に居る）
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の Messages API と一時の設定 dir ---------------------------------------------------
mkdir -p "$TMP/work" "$TMP/home"
WORK="$(cd "$TMP/work" && pwd -P)"
FAKE_LOG="$TMP/fake.log" node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 > "$TMP/fake.out" 2>&1 &
FAKE_PID=$!
PORT=""
for _ in $(seq 1 50); do
  PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$TMP/fake.out" 2>/dev/null)"
  [ -n "$PORT" ] && break
  sleep 0.1
done
[ -n "$PORT" ] || { echo "偽 API が立たない"; exit 1; }
API_URL="http://127.0.0.1:${PORT}"
# ダミーの API キー（本物ではない。一時の .claude.json で末尾 20 字を承認済みにする）
API_KEY="sk-ant-api03-fake2041fake2041fake2041fake2041fake2041fake2041AA"
make_config() {
  local dir="$1" last20
  last20="$(printf '%s' "$API_KEY" | tail -c 20)"
  mkdir -p "$dir"
  jq -n --arg key "$last20" --arg work "$WORK" '{
    hasCompletedOnboarding: true, theme: "dark",
    customApiKeyResponses: { approved: [$key], rejected: [] },
    projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
  }' > "$dir/.claude.json"
}
CFG_DEFAULT="$TMP/home/.claude"
CFG_A="$TMP/acct-a"
CFG_B="$TMP/acct-b"
CFG_C="$TMP/acct-c"
for d in "$CFG_DEFAULT" "$CFG_A" "$CFG_B" "$CFG_C"; do make_config "$d"; done

# claude の包み（GUI の PATH の先頭）: `agents --json` の起動を設定 dir の名前で 1 行ずつ記録して本物へ exec する
REAL_CLAUDE="$(command -v claude)"
CLAUDE_LOG="$TMP/claude-scans.log"
mkdir -p "$TMP/claudebin"
cat > "$TMP/claudebin/claude" <<WRAP
#!/bin/sh
case "\$*" in *"agents --json"*) basename "\${CLAUDE_CONFIG_DIR:-default}" >> '$CLAUDE_LOG' ;; esac
exec '$REAL_CLAUDE' "\$@"
WRAP
chmod +x "$TMP/claudebin/claude"
: > "$CLAUDE_LOG"
scan_log_reset() { : > "$CLAUDE_LOG"; }
# 窓の間に起こした走査の本数と走査先（`本数 名前=本数,…`）
scan_log_summary() {
  local n
  n="$(wc -l < "$CLAUDE_LOG" | tr -d ' ')"
  printf '%s %s' "$n" "$(sort "$CLAUDE_LOG" | uniq -c | awk '{printf "%s=%s,", $2, $1}')"
}

tk() { "$TAKO_BIN" "$@"; }
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_ISOLATED=1
mkdir -p "$TAKO_ORCHESTRATOR_DIR"
arm_env() {
  export TAKO_DATA_DIR="$TMP/$1/data"
  export TAKO_DISCOVERY_DIR="$TMP/$1/disc"
  export TAKO_SESSIONS_FILE="$TMP/$1/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$1/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$1/workers.yaml"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR"
}
arm_env setup
tk orchestrator accounts add acct-a --config-dir "$CFG_A" >/dev/null || { echo "アカウントを登録できない"; exit 1; }
tk orchestrator accounts add acct-b --config-dir "$CFG_B" >/dev/null || exit 1
tk orchestrator accounts add acct-c --config-dir "$CFG_C" >/dev/null || exit 1

# start_app <腕の名前> [VAR=VAL…]
start_app() {
  local name="$1"
  shift
  arm_env "$name"
  launch_isolated_gui "$TMP/app-${name}.log" TAKO_FLOW_DIAG=1 HOME="$TMP/home" \
    PATH="$TMP/claudebin:$PATH" DISABLE_AUTOUPDATER=1 ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  tk autorename off >/dev/null 2>&1
}
stop_app() {
  tk mod --json 2>/dev/null | jq -r '.panes[].report.session_id // empty' >> "$SIDS" 2>/dev/null
  [ -n "$LOAD_PID" ] && kill "$LOAD_PID" 2>/dev/null
  LOAD_PID=""
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  sleep 1
}

wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}
pane_ids() { tk list 2>/dev/null | jq -r '.tabs[].panes[].id' | sort -n; }
split_new() { tk split --pane "$1" "--$2" 2>/dev/null | tr -dc '0-9'; }
mod_row() { tk mod --json 2>/dev/null | jq -c --argjson p "$1" '.panes[] | select(.pane == $p)' 2>/dev/null; }
mod_state() { mod_row "$1" | jq -r '.state // empty'; }
mod_sid() { mod_row "$1" | jq -r '.report.session_id // empty'; }
reporting() { [ "$(mod_state "$1")" = reporting ] && [ -n "$(mod_sid "$1")" ]; }
backend_of() { tk list 2>/dev/null | jq -r --argjson p "$1" '.tabs[].panes[] | select(.id == $p) | .tmux_session // empty'; }
status_json() { tk orchestrator status --pane "$1" 2>/dev/null; }
display_of() { tk ui-mode 2>/dev/null | jq -r --arg p "$1" '.pane_display[$p] // empty'; }
is_chat() { [ "$(display_of "$1")" = chat ]; }
claude_known() { [ -n "$(tk mod --json 2>/dev/null | jq -r '.claude_version // empty')" ]; }
# claude をペインに立てる（3 つ目に nomod を渡すと TAKO_CLI を外す = mod は休眠する）
start_claude() {
  local cli=""
  [ "${3:-}" = nomod ] && cli="-u TAKO_CLI"
  tk send --pane "$1" "cd '$WORK' && clear && env $cli CLAUDE_CONFIG_DIR='$2' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 claude --model haiku" >/dev/null 2>&1
}
# ルートの横に n 本のペインを作って PANES に入れる（注入は claude の版が分かってから作ったペインだけ）
make_panes() {
  local n="$1" root prev p i
  wait_for 30 claude_known || { fail "claude の版が分からない（mod を注入できない）"; return 1; }
  root="$(pane_ids | head -1)"
  prev="$root"
  PANES=""
  for i in $(seq 1 "$n"); do
    if [ $((i % 2)) = 1 ]; then p="$(split_new "$prev" right)"; else p="$(split_new "$prev" down)"; fi
    [ -n "$p" ] || { fail "ペインを分割できない"; return 1; }
    PANES="${PANES:+$PANES }$p"
    prev="$p"
  done
  return 0
}

# --- chat: GUI モードのチャット表示 ---------------------------------------------------------
if want chat; then
  echo "== chat: GUI モードで mod 入りの claude 4 本（4 アカウント）を ${SECS} 秒ずつ"
  for arm in legacy new; do
    case "$arm" in
      legacy) start_app "chat-$arm" TAKO_PERSIST=1 TAKO_2041_LEGACY=1 ;;
      new) start_app "chat-$arm" TAKO_PERSIST=1 ;;
    esac
    tk ui-mode gui >/dev/null 2>&1
    make_panes 4 || { stop_app; continue; }
    set -- $PANES
    start_claude "$1" "$CFG_DEFAULT"
    start_claude "$2" "$CFG_A"
    start_claude "$3" "$CFG_B"
    start_claude "$4" "$CFG_C"
    ok=1
    for p in $PANES; do
      wait_for 60 reporting "$p" || { fail "$arm: pane $p の mod の報告が届かない（state=$(mod_state "$p")）"; ok=0; }
    done
    [ "$ok" = 1 ] || { stop_app; continue; }
    for p in $PANES; do
      wait_for 40 is_chat "$p" || fail "$arm: pane $p がチャット表示にならない（$(display_of "$p")）"
    done
    # 立ち上がりの走査（mod の報告が来る前）を窓に入れない
    sleep 5
    scan_log_reset
    sleep "$SECS"
    set -- $(scan_log_summary)
    eval "SCANS_$arm=\"\$1\"; WHERE_$arm=\"\${2:-}\""
    displays=""
    for p in $PANES; do displays="$displays $(display_of "$p")"; done
    models="$(tk ui-mode 2>/dev/null | jq -r '[.chat_header[] | .model // "null"] | length')"
    eval "DISP_$arm=\"\${displays# }\"; MODELS_$arm=\"\$models\""
    echo "  $arm: ${SECS} 秒に起こした走査 $1 本（${2:-なし}）・表示 ${displays# }・chat_header ${models} 件"
    if [ "$arm" = new ]; then
      echo "  -- new: mod の無い claude（acct-c）を 1 本足す → 走査はそのアカウントと既定だけ"
      P_NOMOD="$(split_new "${PANES%% *}" right)"
      start_claude "$P_NOMOD" "$CFG_C" nomod
      if wait_for 60 is_chat "$P_NOMOD"; then pass "mod の無い claude も走査でチャット表示になる（pane ${P_NOMOD}）"; else fail "mod の無い claude がチャット表示にならない（$(display_of "$P_NOMOD")）"; fi
      sleep 2
      scan_log_reset
      sleep 35
      set -- $(scan_log_summary)
      case "${2:-}" in
        acct-c=*,default=*, | default=*,acct-c=*,) pass "残りの走査は mod の無い claude のアカウント + 既定だけ（35 秒で ${1} 本: ${2}）" ;;
        *) fail "残りの走査先がおかしい（35 秒で ${1} 本: ${2:-なし}。期待 = acct-c と default だけ）" ;;
      esac
    fi
    stop_app
  done
  if [ -n "${SCANS_legacy:-}" ] && [ -n "${SCANS_new:-}" ]; then
    echo "  旧 ${SCANS_legacy} 本（${WHERE_legacy}）/ 新 ${SCANS_new} 本（${WHERE_new:-なし}）"
    if [ "$SCANS_legacy" -gt 0 ]; then pass "旧（TAKO_2041_LEGACY=1）はチャット表示で走査する（${SCANS_legacy} 本 / ${SECS} 秒）"; else fail "旧で走査が 0 本（チャット表示の更新が回っていない）"; fi
    check_eq "新は mod の報告が出ている間チャット表示で走査しない" 0 "$SCANS_new"
    check_eq "表示が新旧で同じ（4 ペイン）" "$DISP_legacy" "$DISP_new"
    check_eq "新もチャット表示（4 ペイン）" "chat chat chat chat" "$DISP_new"
    check_eq "chat_header の件数が新旧で同じ" "$MODELS_legacy" "$MODELS_new"
  fi
fi

# --- clear: /clear の後のレジストリの会話 ID ---------------------------------------------------
# write_worker <ペイン> <器の名前（無ければ空）> <会話 ID>
write_worker() {
  {
    echo "workers:"
    echo "  '1':"
    echo "    label: clear-2041"
    echo "    project: tako"
    echo "    agent: claude"
    echo "    pane: $1"
    [ -n "$2" ] && echo "    tmux_session: $2"
    echo "    session_id: $3"
    echo "    spawned_at: '2026-10-15T00:00:00Z'"
    echo "    status: active"
    echo "next_id: 1"
  } > "$TAKO_WORKERS_FILE"
}
registry_sid() { grep -E '^ *session_id:' "$TAKO_WORKERS_FILE" 2>/dev/null | head -1 | sed 's/.*session_id: *//; s/^'"'"'//; s/'"'"'$//'; }
sid_changed() { local s; s="$(mod_sid "$1")"; [ -n "$s" ] && [ "$s" != "$2" ]; }

run_clear() {
  local label="$1" persist="$2"
  echo "== ${label}: worker の claude で /clear（TAKO_PERSIST=${persist}）"
  for arm in legacy new; do
    case "$arm" in
      legacy) start_app "${label}-$arm" TAKO_PERSIST="$persist" TAKO_2041_LEGACY=1 ;;
      new) start_app "${label}-$arm" TAKO_PERSIST="$persist" ;;
    esac
    make_panes 1 || { stop_app; continue; }
    P="$PANES"
    start_claude "$P" "$CFG_A"
    wait_for 60 reporting "$P" || { fail "$arm: mod の報告が届かない（state=$(mod_state "$P")）"; stop_app; continue; }
    S1="$(mod_sid "$P")"
    echo "$S1" >> "$SIDS"
    BACKEND="$(backend_of "$P")"
    if [ "$persist" = 1 ] && [ -z "$BACKEND" ]; then fail "$arm: persist ON なのに器の名前が無い"; fi
    if [ "$persist" = 0 ] && [ -n "$BACKEND" ]; then fail "$arm: persist OFF なのに器がある（${BACKEND}）"; fi
    write_worker "$P" "$BACKEND" "$S1"
    before="$(status_json "$P" | jq -c '.agents_scan')"
    check_eq "$arm: /clear の前（レジストリ = 報告の会話）は走査しない" '{"reason":"mod_fresh","skipped":true}' "$(printf '%s' "$before" | jq -cS .)"
    # /clear（同じ claude のプロセスで会話 ID が替わる。mod は次の報告で新しい ID を運ぶ）
    tk send --pane "$P" "/clear" >/dev/null 2>&1
    if ! wait_for 30 sid_changed "$P" "$S1"; then fail "$arm: /clear の後に報告の会話 ID が替わらない"; stop_app; continue; fi
    S2="$(mod_sid "$P")"
    scan_log_reset
    first="$(status_json "$P")"
    # master の watch 相当（5 秒ごと）を 25 秒
    scans_json="$(printf '%s' "$first" | jq -c '.agents_scan')"
    for _ in 1 2 3 4 5; do
      sleep 5
      scans_json="$scans_json $(status_json "$P" | jq -c '.agents_scan')"
    done
    set -- $(scan_log_summary)
    n_scans="$1"
    last="$(status_json "$P")"
    echo "  $arm: 1 回目 agents_scan=$(printf '%s' "$first" | jq -c '.agents_scan') status=$(printf '%s' "$first" | jq -r '.status') resolved=$( [ "$(printf '%s' "$first" | jq -r '.resolved_session_id')" = "$S2" ] && echo 新しい会話 || echo 古い会話/なし)"
    echo "        以後の agents_scan: $scans_json"
    echo "        25 秒に起こした走査 ${n_scans} 本（${2:-なし}）・レジストリ = $( [ "$(registry_sid)" = "$S2" ] && echo 新しい会話 || echo 古い会話 )"
    case "$arm" in
      new)
        check_eq "new: 1 回目で追従（registry_followed）・走査なし" '{"reason":"mod_fresh","registry_followed":true,"skipped":true}' "$(printf '%s' "$first" | jq -cS '.agents_scan')"
        check_eq "new: 1 回目の会話 ID は /clear 後の会話" "$S2" "$(printf '%s' "$first" | jq -r '.resolved_session_id')"
        check_eq "new: 1 回目の状態は報告のターン" idle "$(printf '%s' "$first" | jq -r '.status')"
        check_eq "new: レジストリが新しい会話 ID へ更新される" "$S2" "$(registry_sid)"
        check_eq "new: 以後は追従なしで skipped=true" '{"reason":"mod_fresh","skipped":true}' "$(printf '%s' "$last" | jq -cS '.agents_scan')"
        check_eq "new: /clear 後の監視 25 秒に起こした走査" 0 "$n_scans"
        ;;
      legacy)
        check_eq "legacy: 1 回目は session_mismatch で走査へ落ちる" '{"reason":"session_mismatch","skipped":false}' "$(printf '%s' "$first" | jq -cS '.agents_scan')"
        if [ "$n_scans" -gt 0 ]; then pass "legacy: /clear 後の監視で走査する（25 秒で ${n_scans} 本）"; else fail "legacy: 走査が 0 本（旧挙動を再現できていない）"; fi
        if [ "$persist" = 0 ]; then
          check_eq "legacy（器なし）: レジストリは古い会話のまま（カタログの走査が止まる構成）" "$S1" "$(registry_sid)"
          check_eq "legacy（器なし）: 最後も session_mismatch" '{"reason":"session_mismatch","skipped":false}' "$(printf '%s' "$last" | jq -cS '.agents_scan')"
        fi
        ;;
    esac
    stop_app
  done
}

if want clear; then run_clear clear 1; fi
if want clear_direct; then run_clear clear_direct 0; fi

# 本物の設定 dir に検証の会話（transcript `<会話 ID>.jsonl`）と作業フォルダの projects の dir が無い
n_sids="$(sort -u "$SIDS" | grep -c . | tr -d ' ')"
leaks=0
while read -r sid; do
  [ -n "$sid" ] || continue
  leaks=$((leaks + $(find "$REAL_HOME"/.claude* -maxdepth 3 -name "${sid}.jsonl" 2>/dev/null | wc -l)))
done < <(sort -u "$SIDS")
leaks=$((leaks + $(find "$REAL_HOME"/.claude*/projects -maxdepth 1 -name "*tako-2041*" 2>/dev/null | wc -l)))
if [ "$n_sids" -gt 0 ]; then
  check_eq "本物の ~/.claude* に検証の会話が書かれていない（検証の会話 ${n_sids} 件）" 0 "$leaks"
else
  fail "検証の会話 ID を 1 件も採れていない（本物の設定 dir の検査が空振り）"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
