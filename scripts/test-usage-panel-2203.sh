#!/usr/bin/env bash
# test-usage-panel-2203.sh — usage 分析 S4（#2203 / エピック #2192。FR-2.44.13〜15）の実経路テスト
#
# 偽の会話記録（一時 dir の設定 dir）・偽のカタログ・偽の利用制限の観測から CLI `tako usage` が索引を作り、
# 隔離 GUI（tako-vd）の右パネルの usage ビューを visual-test `usage-panel-2203` で実マウスで掘る:
#   1. ステータスバーの 7d を押すと usage ビューが開き、プロジェクト → Issue → 会話と掘れる（画像）
#   2. 画面の行（%・API 換算・トークン・会話数）が同じ段の CLI `tako usage --json` / 既定出力と一致する（表）
#   3. `tako usage --show`（IPC）・MCP `tako_sessions` の `show` で同じ段が開き、`tako_panel` の `usage` が返す
#   4. 境界: データが空 / 索引と観測に読めない行 / 数万の会話（UI が固まらない = 待つ間の描画の最大の間隔）。
#      A/B: `TAKO_1984_LEGACY=1`（読みを UI スレッドの同期の dispatch へ戻す）では数万の会話で描画が止まる
#
# **本番の tako / 本物の claude の設定には触らない**: data / discovery / tmux は mktemp 配下とケースごとの
# 専用ソケット、会話記録は `TAKO_USAGE_CONFIG_DIRS` で一時 dir の設定 dir だけを読ませる。落とすのは
# 自分で起こした pid だけ（pkill / killall は使わない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI が要る）。CI 側の担保は `tako_core::usage::report` / tako-app の
# `usage_panel` の単体テストと番犬（issue1984 / issue2201 / issue2036）。
#
# 使い方: bash scripts/test-usage-panel-2203.sh   （HUGE=<会話の数> で数万の会話の数を変える。既定 30000）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2192_S1_LEGACY TAKO_2192_S2_LEGACY TAKO_USAGE_CONFIG_DIRS \
  TAKO_1984_LEGACY TAKO_2203_CASE
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY="$REPO_ROOT/scripts/lib/usage-panel-2203.py"
HUGE="${HUGE:-30000}"
PASS=0
FAIL=0
UNMEASURED=""

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2203-XXXXXX)"
DUMP_DIR="${USAGE2203_DUMP_DIR:-$TMP/dump}"
APP_PID=""
VHOMES=""
SOCKETS=""
MASK_USER="$(id -un)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<TMP>#g"; }

cleanup() {
  stop_isolated_gui "$APP_PID"
  for v in $VHOMES; do
    case "$v" in */tako-agent-config-*) rm -rf "${v:?}" ;; esac
  done
  for s in $SOCKETS; do
    tmux -L "$s" kill-server >/dev/null 2>&1 || true
    rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${s:?}"
  done
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "usage 分析 S4（#2203）の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &
# 画像が読める大きさ（右パネル 520px + ペイン）
ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-40,40,1280,820}"
mkdir -p "$DUMP_DIR"

# ケースごとに置き場とソケットを分ける（2 回目の GUI が前の器を拾わない = #2157）
use_case() {
  local name="$1"
  local base="$TMP/$name"
  export TAKO_ISOLATED=1
  export TAKO_DATA_DIR="$base/data"
  export TAKO_DISCOVERY_DIR="$base/disc"
  export TAKO_ORCHESTRATOR_DIR="$base/orch"
  export TAKO_SESSIONS_FILE="$base/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$base/panelogs"
  export TAKO_WORKERS_FILE="$base/workers.yaml"
  export TAKO_TMUX_SOCKET="tako-2203-${name}-$$"
  export TAKO_USAGE_CONFIG_DIRS="$base/cfg"
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET"
  mkdir -p "$TAKO_DATA_DIR/usage" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$base/cfg"
}

jf() {
  python3 -I -c '
import json, sys
path = [k for k in sys.argv[1].split(".") if k]
try:
    v = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for k in path:
    if isinstance(v, dict):
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
print("null" if v is None else (json.dumps(v, ensure_ascii=False, sort_keys=True) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}

mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2203","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -I -c '
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

# 隔離 GUI が作る検証用のホーム（後片付けで消す）
remember_vhome() {
  local vh="${TMPDIR:-/tmp}"
  VHOMES="$VHOMES ${vh%/}/tako-agent-config-$1"
}

# 前のケースの索引をそのまま使う（会話記録は**同じ置き場**を読ませる = 読み位置の鍵はパスの
# ハッシュなので、写すと別のファイルとして読み直しになる）
reuse_main() {
  command cp -R "$MAIN_DATA/data/usage" "$TAKO_DATA_DIR/"
  command cp "$MAIN_DATA/sessions.yaml" "$TAKO_SESSIONS_FILE"
  command cp "$MAIN_DATA/orch/projects.yaml" "$TAKO_ORCHESTRATOR_DIR/projects.yaml"
  [ -f "$MAIN_DATA/data/machine_id" ] && command cp "$MAIN_DATA/data/machine_id" "$TAKO_DATA_DIR/machine_id"
  export TAKO_USAGE_CONFIG_DIRS="$MAIN_DATA/cfg"
}

# visual-test を 1 本回してログを返す（$1 = ケース、$2 = ログ、残り = 追加の env）
run_visual() {
  local case_name="$1" log="$2"
  shift 2
  if ! launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=usage-panel-2203 \
    TAKO_VISUAL_DUMP_DIR="$DUMP_DIR" TAKO_2203_CASE="$case_name" "$@"; then
    return 4
  fi
  APP_PID="$ISOLATED_GUI_PID"
  remember_vhome "$APP_PID"
  for _ in $(seq 1 3000); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  grep -E "TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED|TAKO_USAGE_2203_(WAIT|PAYLOAD|PANEL)" "$log" \
    | mask | sed 's/^/    /'
  grep -q "TAKO_VISUAL_TEST_OK" "$log"
}

NOW="$(date +%s)"

echo "== 0. 素材: 偽の会話記録・カタログ・観測から CLI が索引を作る =="
use_case main
python3 -I "$PY" gen-main "$TAKO_USAGE_CONFIG_DIRS" "$TAKO_SESSIONS_FILE" "$TAKO_ORCHESTRATOR_DIR/projects.yaml" "$NOW"
LOGIN="$("$TAKO_BIN" usage --json 2>/dev/null | jf logins.0.login)"
check_eq "ログインの鍵は 12 桁" "12" "${#LOGIN}"
python3 -I "$PY" limits "$LOGIN" "$NOW" main > "$TAKO_DATA_DIR/usage/limits.jsonl"
TOP="$("$TAKO_BIN" usage --json 2>/dev/null)"
NAMES="$(printf '%s' "$TOP" | python3 -I -c 'import json,sys; print(" / ".join(g["name"] for g in json.load(sys.stdin)["logins"][0]["usage"]["groups"]))')"
echo "    最上段（CLI）: $NAMES"
case "$NAMES" in *"alpha"*"プロファイル: alpha"*|*"プロファイル: alpha"*"alpha"*) pass "同名のプロジェクトとプロファイルが別の行" ;; *) fail "同名のプロジェクトとプロファイルの行が無い: $NAMES" ;; esac
case "$NAMES" in *"未分類"*) pass "未分類の行がある" ;; *) fail "未分類の行が無い" ;; esac
MAIN_DATA="$TMP/main"

echo "== 1. visual-test: ステータスバーの 7d → プロジェクト → Issue → 会話・パンくず・5h・狭い幅・show =="
if run_visual main "$TMP/visual-main.log"; then
  pass "visual: 7d から開き、行で掘り、パンくずで戻り、5h に替え、最小幅で描け、show の段が tako panel に出る・画面の行 = 同じ関数の答え"
else
  rc=$?
  if [ "$rc" -eq 4 ]; then
    UNMEASURED="${UNMEASURED} 隔離 GUI（tako-vd を用意できない）"
  else
    fail "visual-test main が通らない（$(grep -m1 "TAKO_APP_SELF_TEST_FAILED" "$TMP/visual-main.log" | mask)）"
  fi
fi

echo "== 2. 画面の行と CLI（同じ段の tako usage --json と既定出力）の照合 =="
CLI_DIR="$TMP/cli"
mkdir -p "$CLI_DIR"
while IFS=$'\t' read -r tag step by window project issue limit; do
  [ "$tag" = "TAKO_USAGE_QUERY" ] || continue
  args=(--by "$by" --window "$window" --limit "$limit")
  [ -n "$project" ] && args+=(--project "$project")
  [ -n "$issue" ] && args+=(--issue "$issue")
  "$TAKO_BIN" usage --json ${args[@]+"${args[@]}"} > "$CLI_DIR/$step.json" 2>/dev/null
  "$TAKO_BIN" usage ${args[@]+"${args[@]}"} > "$CLI_DIR/$step.txt" 2>/dev/null
  echo "    ${step}: tako usage ${args[*]+${args[*]}}"
done < <(grep "^TAKO_USAGE_QUERY" "$TMP/visual-main.log" 2>/dev/null)
if python3 -I "$PY" check "$TMP/visual-main.log" "$CLI_DIR" > "$TMP/check.txt"; then
  pass "画面の行（%・API 換算・トークン・会話）が CLI の JSON と既定出力の行に一致（$(grep CHECKED "$TMP/check.txt")）"
else
  fail "画面の行と CLI が食い違う（$(grep CHECKED "$TMP/check.txt")）"
fi
grep "^  |" "$TMP/check.txt"

echo "== 3. tako usage --show / MCP の show / tako_panel の usage（IPC の実経路）=="
use_case show
reuse_main
OUT="$("$TAKO_BIN" usage --show 2>&1)"
check_eq "tako が動いていなければ --show は理由を言って止まる" \
  "tako が動いていないので右パネルには出せない（--show を外すとこのまま読める）" "$(printf '%s' "$OUT" | tail -1)"
if launch_isolated_gui "$TMP/app-show.log"; then
  APP_PID="$ISOLATED_GUI_PID"
  remember_vhome "$APP_PID"
  wait_isolated_gui "$TMP/app-show.log" || exit 1
  J="$("$TAKO_BIN" usage --show --json --project project:alpha --by issue 2>/dev/null)"
  check_eq "--show の答えに shown: true" "true" "$(printf '%s' "$J" | jf shown)"
  check_eq "--show の答えの絞り込み" "project:alpha" "$(printf '%s' "$J" | jf filter.project)"
  CLI_LOCAL="$("$TAKO_BIN" usage --json --project project:alpha --by issue 2>/dev/null | jf logins.0.usage.groups)"
  check_eq "--show の答え（GUI 経由）の段 = ローカルの CLI の段" "$CLI_LOCAL" "$(printf '%s' "$J" | jf logins.0.usage.groups)"
  P="$(mcp_call tako_panel '{}')"
  check_eq "tako_panel: view = usage" "usage" "$(printf '%s' "$P" | jf view)"
  check_eq "tako_panel: usage の段（by / project / depth）" "issue project:alpha 2" \
    "$(printf '%s' "$P" | jf usage.by) $(printf '%s' "$P" | jf usage.project) $(printf '%s' "$P" | jf usage.depth)"
  M="$(mcp_call tako_sessions '{"action":"usage","by":"session","project":"project:alpha","issue":"2202","show":true}')"
  check_eq "MCP show: shown" "true" "$(printf '%s' "$M" | jf shown)"
  check_eq "MCP show: #2202 の会話は 2 本" "2" "$(printf '%s' "$M" | jf logins.0.usage.total.sessions)"
  P="$(mcp_call tako_panel '{}')"
  check_eq "MCP show の後の tako_panel: session / 2202 / 2 段" "session 2202 2" \
    "$(printf '%s' "$P" | jf usage.by) $(printf '%s' "$P" | jf usage.issue) $(printf '%s' "$P" | jf usage.depth)"
  BAD="$("$TAKO_BIN" usage --show --issue abc 2>&1)"
  case "$BAD" in *"読めない issue"*) pass "読めない --issue は断る" ;; *) fail "読めない --issue を断らない: $BAD" ;; esac
  P="$(mcp_call tako_panel '{}')"
  check_eq "断った show は画面の段を動かさない" "session 2202" "$(printf '%s' "$P" | jf usage.by) $(printf '%s' "$P" | jf usage.issue)"
  P="$(mcp_call tako_panel '{"view":"fleet"}')"
  P="$(mcp_call tako_panel '{"view":"usage"}')"
  check_eq "tako panel --view usage で戻ると段は残っている" "usage session" "$(printf '%s' "$P" | jf view) $(printf '%s' "$P" | jf usage.by)"
  stop_isolated_gui "$APP_PID"
  APP_PID=""
else
  UNMEASURED="${UNMEASURED} 隔離 GUI の IPC（tako-vd を用意できない）"
fi

echo "== 4a. 境界: データが空 =="
use_case empty
if run_visual empty "$TMP/visual-empty.log"; then
  case "$(grep '^TAKO_USAGE_2203_PAYLOAD' "$TMP/visual-empty.log")" in
    *"logins=0"*) pass "空: ログイン 0 で答えが届き「まだ観測も使用量もありません」を出す" ;;
    *) fail "空の答えが想定と違う" ;;
  esac
else
  fail "visual-test empty が通らない（$(grep -m1 "TAKO_APP_SELF_TEST_FAILED" "$TMP/visual-empty.log" | mask)）"
fi

echo "== 4b. 境界: 索引と観測に読めない行 =="
use_case broken
reuse_main
JUNK="$(python3 -I "$PY" break "$TAKO_DATA_DIR/usage")"
echo "    読めない行を $JUNK 行混ぜた"
if run_visual broken "$TMP/visual-broken.log"; then
  P="$(grep '^TAKO_USAGE_2203_PAYLOAD' "$TMP/visual-broken.log")"
  case "$P" in
    *"logins=1"*"unreadable=0"*) fail "読めない行を数えていない: $P" ;;
    *"logins=1"*"unreadable="*) pass "壊れた行を飛ばして答え、読めない行の数を出す（$(printf '%s' "$P" | cut -f3)）" ;;
    *) fail "壊れた索引の答えが想定と違う: $P" ;;
  esac
else
  fail "visual-test broken が通らない（$(grep -m1 "TAKO_APP_SELF_TEST_FAILED" "$TMP/visual-broken.log" | mask)）"
fi

echo "== 4c. 境界: 数万の会話（${HUGE} 本）=="
use_case huge
T0="$(perl -MTime::HiRes=time -e 'printf "%.1f", time')"
python3 -I "$PY" gen-huge "$TAKO_USAGE_CONFIG_DIRS" "$TAKO_ORCHESTRATOR_DIR/projects.yaml" "$NOW" "$HUGE"
T1="$(perl -MTime::HiRes=time -e 'printf "%.1f", time')"
HLOGIN="$("$TAKO_BIN" usage --json 2>/dev/null | jf logins.0.login)"
T2="$(perl -MTime::HiRes=time -e 'printf "%.1f", time')"
python3 -I "$PY" limits "$HLOGIN" "$NOW" huge > "$TAKO_DATA_DIR/usage/limits.jsonl"
echo "    生成 $(perl -e "printf '%.1f', $T1 - $T0") 秒・CLI の索引作り（debug）$(perl -e "printf '%.1f', $T2 - $T1") 秒"
HS="$("$TAKO_BIN" usage --json --window all --by login 2>/dev/null | jf logins.0.usage.total.sessions)"
check_eq "索引の会話の数" "$HUGE" "$HS"
gap_of() { grep "^TAKO_USAGE_2203_WAIT" "$1" | sed -n 's/.*max_draw_gap_ms=\([0-9]*\).*/\1/p' | sort -n | tail -1; }
if run_visual huge "$TMP/visual-huge.log"; then
  GAP="$(gap_of "$TMP/visual-huge.log")"
  grep "^TAKO_USAGE_2203_WAIT" "$TMP/visual-huge.log" | sed 's/^/    /'
  if [ -n "$GAP" ] && [ "$GAP" -le 500 ]; then
    pass "数万の会話でも待つ間の描画の最大の間隔 ${GAP} ms（≤ 500 ms = UI が固まらない）"
  else
    fail "数万の会話で描画が ${GAP:-?} ms 止まった"
  fi
  echo "    A/B: TAKO_1984_LEGACY=1（読みを UI スレッドの同期の dispatch へ戻す）"
  run_visual huge "$TMP/visual-huge-legacy.log" TAKO_1984_LEGACY=1 >/dev/null 2>&1
  LGAP="$(gap_of "$TMP/visual-huge-legacy.log")"
  grep "^TAKO_USAGE_2203_WAIT" "$TMP/visual-huge-legacy.log" | sed 's/^/    legacy /'
  if [ -n "$LGAP" ] && [ -n "$GAP" ] && [ "$LGAP" -gt "$GAP" ]; then
    pass "A/B: 旧経路は描画が ${LGAP} ms 止まる（新 ${GAP} ms）"
  else
    fail "A/B の差が出ない（旧 ${LGAP:-?} ms / 新 ${GAP:-?} ms）"
  fi
else
  fail "visual-test huge が通らない（$(grep -m1 "TAKO_APP_SELF_TEST_FAILED" "$TMP/visual-huge.log" | mask)）"
fi

echo
echo "画像: $DUMP_DIR" | mask
[ -n "$UNMEASURED" ] && echo "未実測:$UNMEASURED"
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
