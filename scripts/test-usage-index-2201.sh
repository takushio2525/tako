#!/usr/bin/env bash
# test-usage-index-2201.sh — usage 分析 S2（#2201 / エピック #2192。FR-2.44.4〜7）の実経路テスト
#
# 偽の会話記録（一時 dir の設定 dir）を実 CLI と隔離 GUI に読ませる（claude 不要・決定的）。
# 番号は設計書 §9「S2」の受け入れ条件:
#   1. 分割行（仮の output）・`<synthetic>`・サブエージェント・書きかけの最終行で期待値どおり。
#      2 回目は何も読まず、追記した後は追記した分だけを読む。`--by project` / `--by issue` / `--by session`
#   3. 本文・ツールの入力・`wireToolInputs`・gitBranch・cwd・設定 dir のパスが `usage/` と persist.log に出ない。
#      隔離 GUI は起動しても索引を読まず（最初の更新は起動から 60 秒後）、その後は背景で追記分を読む。
#      MCP `tako_sessions` の `action: "usage"` が CLI と同じ内訳を返す
#   4. カタログのラベルの最初の `#N`（プロンプト由来の小さい番号ではない）が主の Issue になる
#   5. 単価表に無いモデルは `unpriced_tokens` に数えて注記が出る
#   A/B: `TAKO_2192_S2_LEGACY=1` の同じバイナリでは索引を作らない
#
# **本番の tako / 本物の claude の設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 会話記録は `TAKO_USAGE_CONFIG_DIRS` で一時 dir の設定 dir だけを読ませる（検証プロセスは指定が無ければ
# 検証用のホームを読む = 本物の `~/.claude*` は読まない）。落とすのは自分で起こした pid だけ
# （pkill / killall は使わない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue2201_usage_index_watchdog.rs`）。
#
# 使い方: bash scripts/test-usage-index-2201.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2192_S1_LEGACY TAKO_2192_S2_LEGACY TAKO_USAGE_CONFIG_DIRS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
PASS=0
FAIL=0
UNMEASURED=""

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2201-XXXXXX)"
APP_PID=""
VHOME=""
TMUX_SOCKET="tako-2201-$$"
MASK_USER="$(id -un)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<TMP>#g"; }

cleanup() {
  stop_isolated_gui "$APP_PID"
  if [ -n "$VHOME" ]; then
    case "$VHOME" in */tako-agent-config-*) rm -rf "$VHOME" ;; esac
  fi
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "usage 分析 S2（#2201）の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_USAGE_CONFIG_DIRS="$TMP/cfg"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
USAGE="$TAKO_DATA_DIR/usage"

# 本文・ツールの入力・wireToolInputs・gitBranch・cwd に入れる印（出たら名指す）
BODY="body-marker-2201"
TOOL="tool-marker-2201"
WIRE="wire-marker-2201"
BRANCH="branch-marker-2201"
CWD_ALPHA="/home/testuser/dev/alpha-marker-2201"
CWD_BETA="/home/testuser/dev/beta"
TS="$(python3 -I -c 'import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=1)).strftime("%Y-%m-%dT%H:%M:%S.000Z"))')"

# assistant 行 1 本（id / 会話 / モデル / 入力 / 出力 / 作成 5 分 / 作成 1 時間 / 読み出し / cwd）
line() {
  printf '{"type":"assistant","isSidechain":false,"message":{"id":"%s","model":"%s","role":"assistant","content":[{"type":"text","text":"%s"},{"type":"tool_use","name":"Bash","input":{"command":"%s"}}],"usage":{"input_tokens":%s,"output_tokens":%s,"cache_creation_input_tokens":%s,"cache_creation":{"ephemeral_5m_input_tokens":%s,"ephemeral_1h_input_tokens":%s},"cache_read_input_tokens":%s,"speed":"standard"}},"requestId":"req_%s","timestamp":"%s","sessionId":"%s","cwd":"%s","gitBranch":"feat/%s","wireToolInputs":{"x":"%s"},"uuid":"u-%s-%s"}' \
    "$1" "$3" "$BODY" "$TOOL" "$4" "$5" "$(($6 + $7))" "$6" "$7" "$8" "$1" "$TS" "$2" "$9" "$BRANCH" "$WIRE" "$1" "$5"
}
OPUS="claude-opus-5-5"
SA="$TMP/cfg/projects/-home-testuser-dev-alpha-marker-2201/s-alpha.jsonl"
SUB="$TMP/cfg/projects/-home-testuser-dev-alpha-marker-2201/s-alpha/subagents/agent-1.jsonl"
SB="$TMP/cfg/projects/-home-testuser-dev-beta/s-beta.jsonl"
mkdir -p "$(dirname "$SA")" "$(dirname "$SUB")" "$(dirname "$SB")"
PARTIAL="$(line msg_a3 s-alpha "$OPUS" 0 7000 0 0 0 "$CWD_ALPHA")"
{
  printf '{"type":"user","sessionId":"s-alpha","message":{"content":"%s の依頼"}}\n' "$BODY"
  line msg_a1 s-alpha "$OPUS" 3000 10000 100000 0 1000000 "$CWD_ALPHA"; echo
  line msg_a1 s-alpha "$OPUS" 3000 60000 100000 0 1000000 "$CWD_ALPHA"; echo
  line msg_a1 s-alpha "$OPUS" 3000 200000 100000 0 1000000 "$CWD_ALPHA"; echo
  line msg_syn s-alpha "<synthetic>" 0 0 0 0 0 "$CWD_ALPHA"; echo
  line msg_a2 s-alpha "$OPUS" 1000 50000 0 20000 0 "$CWD_ALPHA"; echo
  # 書きかけ（改行なし）の前半
  printf '%s' "${PARTIAL:0:120}"
} > "$SA"
{ line msg_as s-alpha claude-haiku-4-5-20251001 0 30000 0 0 5000000 "$CWD_ALPHA"; echo; } > "$SUB"
{ line msg_b1 s-beta claude-sonnet-9-9 7 7 0 0 0 "$CWD_BETA/sub"; echo; } > "$SB"

# 帰属の材料: カタログ（s-alpha = worker・alpha・ラベルの最初の番号 #2201、プロンプトに出る小さい #322 は使わない）・
# プロジェクト（beta は cwd の最長前方一致）・アカウント（設定 dir = univ）
cat > "$TAKO_SESSIONS_FILE" <<EOF
entries:
  s-alpha:
    kind: worker
    label: "#2201 S2 の索引 / #322 を参照"
    project: alpha
    issues: [322, 2201]
    started_at: "2026-10-16T00:00:00Z"
    last_seen_at: "2026-10-16T00:00:00Z"
EOF
cat > "$TAKO_ORCHESTRATOR_DIR/projects.yaml" <<EOF
projects:
  beta:
    cwd: $CWD_BETA
EOF
cat > "$TAKO_ORCHESTRATOR_DIR/accounts.yaml" <<EOF
accounts:
  univ:
    config_dir: $TMP/cfg
EOF

jf() {
  python3 -I -c '
import json, sys
path = [k for k in sys.argv[1].split(".") if k]
try:
    v = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for k in path:
    if k.startswith("name=") and isinstance(v, list):
        v = next((w for w in v if isinstance(w, dict) and w.get("name") == k[5:]), None)
    elif isinstance(v, dict):
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
if isinstance(v, float) and v.is_integer():
    v = int(v)
print("null" if v is None else (json.dumps(v, ensure_ascii=False, sort_keys=True) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
# 内訳の名前の並び（ログイン A）
names() { jf logins.0.usage.groups | python3 -I -c 'import json,sys; print(" / ".join(g["name"] for g in json.load(sys.stdin)))'; }
usage_json() { "$TAKO_BIN" usage --json "$@" 2>/dev/null; }
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2201","version":"0"}}}' \
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

echo "== 1. CLI（GUI 無し）: 偽の会話記録を数える =="
J="$(usage_json --by project --window all)"
check_eq "新しく数えた応答（msg_a1 / msg_a2 / msg_as / msg_b1）" "4" "$(printf '%s' "$J" | jf index.update.responses)"
check_eq "読んだ transcript" "3" "$(printf '%s' "$J" | jf index.update.read_files)"
check_eq "プロジェクトの並び（API 換算の大きい順）" "alpha / beta" "$(printf '%s' "$J" | names)"
check_eq "alpha の出力（分割行は確定値 200000 + 50000 + サブエージェント 30000）" "280000" \
  "$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.tokens.output)"
check_eq "alpha の API 換算（Opus 5.5 = 5.876 + Haiku 4.5 = 0.65）" "6.526" \
  "$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.api_usd)"
check_eq "alpha の応答数（<synthetic> と書きかけを数えない）" "3" \
  "$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.responses)"
check_eq "5. beta は単価不明（API 換算 0・unpriced 14）" "0/14" \
  "$(printf '%s' "$J" | jf logins.0.usage.groups.name=beta.api_usd)/$(printf '%s' "$J" | jf logins.0.usage.groups.name=beta.unpriced_tokens)"
check_eq "5. 単価不明のモデル" '["claude-sonnet-9-9"]' "$(printf '%s' "$J" | jf unpriced_models)"
case "$(printf '%s' "$J" | jf notes)" in
  *"単価表に無いモデル（claude-sonnet-9-9）"*) pass "5. 単価不明の注記が出る" ;;
  *) fail "5. 単価不明の注記が無い" ;;
esac
check_eq "アカウント（accounts.yaml の config_dir）" "univ" "$(usage_json --by account --window all | jf logins.0.usage.groups.0.name)"
J="$(usage_json --by issue --window all)"
check_eq "4. 主の Issue はラベルの最初の番号（#322 ではない）" "alpha #2201 / beta Issue なし" "$(printf '%s' "$J" | names)"
check_eq "--by session の先頭（会話・役割・プロジェクト・ラベル）" "s-alpha worker alpha #2201 S2 の索引 / #322 を参照" \
  "$(usage_json --by session --window all | jf logins.0.usage.groups.0.name)"
check_eq "既定（今の 7d の窓・観測なし = 直近 7 日）" "rolling" "$(usage_json | jf logins.0.usage.range.basis)"
echo "    tako usage --by issue の既定出力:"
"$TAKO_BIN" usage --by issue 2>&1 | sed -e 's/^/    | /' | mask

echo "-- 1. 2 回目は何も読まない・追記した後は追記した分だけ読む"
J="$(usage_json)"
check_eq "変わっていなければ読まない（バイト数）" "0" "$(printf '%s' "$J" | jf index.update.bytes_read)"
BEFORE="$(wc -c < "$SA" | tr -d ' ')"
REST="${PARTIAL:120}"
ADDED="$(printf '%s\n%s\n' "$REST" "$(line msg_a4 s-alpha "$OPUS" 0 1000 0 0 0 "$CWD_ALPHA")")"
printf '%s\n' "$ADDED" >> "$SA"
AFTER="$(wc -c < "$SA" | tr -d ' ')"
J="$(usage_json --window all)"
# 読み位置は書きかけの行の頭（= 前回の大きさ − 書きかけの 120 バイト）
check_eq "読んだのは読み位置から後ろだけ（書きかけの残り + 新しい応答）" "$((AFTER - BEFORE + 120))" \
  "$(printf '%s' "$J" | jf index.update.bytes_read)"
check_eq "書きかけだった msg_a3 と新しい msg_a4" "2" "$(printf '%s' "$J" | jf index.update.responses)"
check_eq "alpha の出力（+ 7000 + 1000）" "288000" "$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.tokens.output)"

echo "-- 3. 本文・パスが usage/ に出ない"
LEAK=""
for f in $(find "$USAGE" -type f); do
  for needle in "$BODY" "$TOOL" "$WIRE" "$BRANCH" "$CWD_ALPHA" "$CWD_BETA" "$TMP"; do
    if grep -qF -- "$needle" "$f" 2>/dev/null; then LEAK="${LEAK} $(basename "$f"):$(printf '%s' "$needle" | mask)"; fi
  done
done
check_eq "本文・ツールの入力・wireToolInputs・gitBranch・cwd・設定 dir の漏れ" "" "$LEAK"
check_eq "走査したファイル（cursor / keys / 月 / 書き直しの覚え ほか）" "yes" \
  "$([ "$(find "$USAGE" -type f | wc -l | tr -d ' ')" -ge 4 ] && echo yes || echo no)"
CLI_ISSUE="$(usage_json --by issue --window all | jf logins.0.usage.groups)"

echo "== 3. 隔離 GUI: 起動しても読まず、60 秒後から背景で追記分を読む・MCP が同じ内訳を返す =="
if launch_isolated_gui "$TMP/app.log"; then
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app.log" || exit 1
  VHOME="${TMPDIR:-/tmp}"
  VHOME="${VHOME%/}/tako-agent-config-${APP_PID}"
  STAMP0="$(stat -f '%z %m' "$USAGE/cursor.jsonl")"
  printf '%s\n' "$(line msg_a5 s-alpha "$OPUS" 0 2000 0 0 0 "$CWD_ALPHA")" >> "$SA"
  sleep 6
  check_eq "起動して 6 秒は索引を読まない（cursor.jsonl が変わらない）" "$STAMP0" "$(stat -f '%z %m' "$USAGE/cursor.jsonl")"
  echo "    （GUI の最初の更新 = 起動から 60 秒後を待つ）"
  for _ in $(seq 1 60); do
    grep -q 'usage: 使用量の索引が追いついた' "$TAKO_DATA_DIR/persist.log" 2>/dev/null && break
    sleep 2
  done
  check_eq "GUI の背景が追記分を読んで追いついた（persist.log）" "1" "$(grep -c 'usage: 使用量の索引が追いついた' "$TAKO_DATA_DIR/persist.log" 2>/dev/null || echo 0)"
  grep 'usage:' "$TAKO_DATA_DIR/persist.log" | sed -e 's/^/    /' | mask
  MCP="$(mcp_call tako_sessions '{"action":"usage","by":"issue","window":"all"}')"
  check_eq "MCP の alpha の出力（GUI が読んだ msg_a5 の 2000 を含む）" "290000" \
    "$(printf '%s' "$MCP" | jf "logins.0.usage.groups.name=alpha #2201.tokens.output")"
  check_eq "MCP と CLI の内訳が同じ（同じ 1 実装）" "$(usage_json --by issue --window all | jf logins.0.usage.groups)" \
    "$(printf '%s' "$MCP" | jf logins.0.usage.groups)"
  case "$CLI_ISSUE" in *"alpha #2201"*) pass "GUI の前の CLI の内訳にも alpha #2201" ;; *) fail "CLI の内訳が崩れた" ;; esac
  LEAK=""
  grep 'usage:' "$TAKO_DATA_DIR/persist.log" > "$TMP/persist-usage.log" 2>/dev/null || true
  for needle in "$BODY" "$TOOL" "$WIRE" "$BRANCH" "$CWD_ALPHA" "$TMP" "alpha" "2201"; do
    if grep -qF -- "$needle" "$TMP/persist-usage.log"; then LEAK="${LEAK} $(printf '%s' "$needle" | mask)"; fi
  done
  check_eq "persist.log の usage の行に本文・パス・プロジェクト・Issue が出ない" "" "$LEAK"
  stop_isolated_gui "$APP_PID"
  APP_PID=""
else
  UNMEASURED="隔離 GUI（tako-vd が起きない = 蓋閉じ等）"
  echo "  [--] 隔離 GUI を起こせなかった: GUI の節は未実測"
fi

echo "== A/B: TAKO_2192_S2_LEGACY=1（同じバイナリ・data dir を分ける）=="
J="$(TAKO_DATA_DIR="$TMP/data-ab" TAKO_2192_S2_LEGACY=1 "$TAKO_BIN" usage --json 2>/dev/null)"
check_eq "旧挙動は索引を作らない（index.enabled）" "false" "$(printf '%s' "$J" | jf index.enabled)"
check_eq "旧挙動は cursor.jsonl を作らない" "absent" "$([ -e "$TMP/data-ab/usage/cursor.jsonl" ] && echo present || echo absent)"

echo
[ -n "$UNMEASURED" ] && echo "未実測: $UNMEASURED"
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
