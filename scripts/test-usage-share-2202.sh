#!/usr/bin/env bash
# test-usage-share-2202.sh — usage 分析 S3（#2202 / エピック #2192。FR-2.44.8〜10）の実経路テスト
#
# 偽の会話記録（一時 dir の設定 dir）と偽の利用制限の観測（limits.jsonl）を実 CLI と隔離 GUI に読ませる
# （claude 不要・決定的）。番号は設計書 §9「S3」の受け入れ条件:
#   1. 7d / 5h の今の窓で、段の % の合計 + tako の外 = 最新の観測。区間にローカルの使用量が無い Δ は
#      「tako の外」へ、最後の観測より後の使用量は「推定」へ入る
#   3. `--by project` / `--by issue` の既定出力に 7d / 5h の % 列が出て、MCP `tako_sessions` の usage が
#      CLI と同じ値を返す
#   境界: `--project` で絞っても按分は絞る前の全行で行う（絞った段の % は絞らないときと同じ）・
#   暦の期間は延べ（scope: period）・観測の無いログインは % を出さない（available: false と注記）
#
# **本番の tako / 本物の claude の設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 会話記録は `TAKO_USAGE_CONFIG_DIRS` で一時 dir の設定 dir だけを読ませる。落とすのは自分で起こした pid
# だけ（pkill / killall は使わない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI が要る）。CI 側の担保は `tako_core::usage::share` / `report` の単体テスト。
#
# 使い方: bash scripts/test-usage-share-2202.sh
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

TMP="$(mktemp -d /tmp/tako-2202-XXXXXX)"
APP_PID=""
VHOME=""
TMUX_SOCKET="tako-2202-$$"
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
tako_exit_trap cleanup "usage 分析 S3（#2202）の実経路テスト"

# rebase 後に古いバイナリで回さない（isolated_gui_bins は無いときだけビルドする）
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app) || exit 1
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
mkdir -p "$TAKO_DATA_DIR/usage" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/cfg"
USAGE="$TAKO_DATA_DIR/usage"
# 資格情報（accountUuid だけ。ログインの鍵 = 塩付きハッシュ）
printf '{"oauthAccount":{"accountUuid":"aaaaaaaa-2202-4222-8333-444444444444"}}\n' > "$TMP/cfg/.claude.json"

NOW="$(date +%s)"
# 分を指定して「今から N 分前」の ISO（UTC）
iso_ago() {
  python3 -I -c 'import datetime,sys; t=datetime.datetime.fromtimestamp(int(sys.argv[1]) - int(sys.argv[2]) * 60, datetime.timezone.utc); print(t.strftime("%Y-%m-%dT%H:%M:%S.000Z"))' "$NOW" "$1"
}
# assistant 行 1 本（id / 会話 / 何分前 / 出力 / 読み出し / cwd）。Opus 5.5 = 出力 $20・読み出し $0.2 / 100 万
line() {
  printf '{"type":"assistant","message":{"id":"%s","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"本文"}],"usage":{"input_tokens":0,"output_tokens":%s,"cache_read_input_tokens":%s,"speed":"standard"}},"requestId":"req_%s","timestamp":"%s","sessionId":"%s","cwd":"%s","uuid":"u-%s"}\n' \
    "$1" "$4" "$5" "$1" "$(iso_ago "$3")" "$2" "$6" "$1"
}
CWD_ALPHA="/home/testuser/dev/alpha"
CWD_BETA="/home/testuser/dev/beta"
SA="$TMP/cfg/projects/-home-testuser-dev-alpha/s-alpha.jsonl"
SB="$TMP/cfg/projects/-home-testuser-dev-beta/s-beta.jsonl"
mkdir -p "$(dirname "$SA")" "$(dirname "$SB")"
# 使用量: alpha は 170 / 120 / 60 分前、beta は 100 / 52 / 8 分前（8 分前 = 最後の観測より後 = 推定）。
# 45〜30 分前はどちらも使わない（その間に 7d が 3% 増える = tako の外）
{
  line msg_a1 s-alpha 170 100000 1000000 "$CWD_ALPHA"
  line msg_a2 s-alpha 120 100000 1000000 "$CWD_ALPHA"
  line msg_a3 s-alpha 60 300000 1000000 "$CWD_ALPHA"
} > "$SA"
{
  line msg_b1 s-beta 100 50000 0 "$CWD_BETA"
  line msg_b2 s-beta 52 50000 0 "$CWD_BETA"
  line msg_b3 s-beta 8 50000 0 "$CWD_BETA"
} > "$SB"
cat > "$TAKO_SESSIONS_FILE" <<EOF
entries:
  s-alpha:
    kind: worker
    label: "#2202 区間按分"
    project: alpha
    started_at: "2026-10-16T00:00:00Z"
    last_seen_at: "2026-10-16T00:00:00Z"
EOF
cat > "$TAKO_ORCHESTRATOR_DIR/projects.yaml" <<EOF
projects:
  beta:
    cwd: $CWD_BETA
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
    elif k.startswith("kind=") and isinstance(v, list):
        v = next((w for w in v if isinstance(w, dict) and w.get("kind") == k[5:]), None)
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
usage_json() { "$TAKO_BIN" usage --json "$@" 2>/dev/null; }
# 段の % の合計 + tako の外（枠 $1）と最新の観測を「合計/観測」で返す
share_sum() {
  python3 -I -c '
import json, sys
kind = sys.argv[1]
u = json.load(sys.stdin)["logins"][0]["usage"]
ks = next(k for k in u["limit_share"] if k["kind"] == kind)
total = sum((g.get("limit_share") or {}).get(kind, {}).get("percent", 0) for g in u["groups"]) + ks["outside"]
latest = ks["latest_percent"]
print("%.2f/%.2f" % (total, latest))' "$1"
}
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2202","version":"0"}}}' \
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

echo "== 0. 観測の無いうちは % を出さない（索引を作ってログインの鍵を読む）=="
J="$(usage_json --by project)"
LOGIN="$(printf '%s' "$J" | jf logins.0.login)"
check_eq "ログインの鍵は 12 桁" "12" "${#LOGIN}"
check_eq "観測の無い 7d は available: false" "false" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.available)"
case "$(printf '%s' "$J" | jf notes)" in
  *"観測の無いログインと枠は % を出していない: ログイン A（5h / 7d）"*) pass "観測の無いログインの注記" ;;
  *) fail "観測の無いログインの注記が無い: $(printf '%s' "$J" | jf notes | mask)" ;;
esac

# 観測（limits.jsonl）: 7d は解除まで 3 日（窓の開始 = 4 日前）。150 / 90 / 45 / 30 分前に 10 / 14 / 17 / 20%。
# 5h は解除まで 2 時間（窓の開始 = 3 時間前）。100 / 40 分前に 3 / 8%。放置したペインの古い値（下がる）と
# 同じ値の重複も混ぜる
obs() { # 枠 % 解除時刻（秒）何分前
  printf '{"login":"%s","key_from":"account","kind":"%s","percent":%s,"resets_at":%s,"observed_at":%s,"recorded_at":%s,"source":"mod"}\n' \
    "$LOGIN" "$1" "$2" "$3" "$(((NOW - $4 * 60) * 1000))" "$(((NOW - $4 * 60) * 1000))"
}
R7=$((NOW + 3 * 86400))
R5=$((NOW + 2 * 3600))
{
  obs seven_day 10 "$R7" 150
  obs seven_day 14 "$R7" 90
  obs seven_day 12 "$R7" 80
  obs seven_day 17 "$R7" 45
  obs seven_day 17 "$R7" 44
  obs seven_day 20 "$R7" 30
  obs five_hour 3 "$R5" 100
  obs five_hour 8 "$R5" 40
} > "$USAGE/limits.jsonl"

echo "== 1. 今の窓: 段の % の合計 + tako の外 = 最新の観測 =="
J="$(usage_json --by project)"
check_eq "7d: 段の合計 + tako の外 / 最新の観測" "20.00/20.00" "$(printf '%s' "$J" | share_sum seven_day)"
check_eq "5h: 段の合計 + tako の外 / 最新の観測" "8.00/8.00" "$(printf '%s' "$J" | share_sum five_hour)"
check_eq "7d の範囲は今の窓" "current_window" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.scope)"
check_eq "7d の tako の外（45〜30 分前に使用なしで 3% 増えた）" "3" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.outside)"
check_eq "下がった値と同じ値の重複を捨てた（使わなかった観測）" "1" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.ignored_observations)"
EST="$(printf '%s' "$J" | jf logins.0.usage.groups.name=beta.limit_share.seven_day.estimated)"
case "$EST" in absent | null | 0) fail "beta の 8 分前の使用が推定に入らない（${EST}）" ;; *) pass "最後の観測より後の beta の使用は推定（+${EST}）" ;; esac
check_eq "alpha に推定は無い（最後の観測より後に使っていない）" "null" "$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.limit_share.seven_day.estimated)"
check_eq "段は 7d の % の大きい順" "alpha" "$(printf '%s' "$J" | jf logins.0.usage.groups.0.name)"
check_eq "係数（直近 7 日の区間から）" "true" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.coefficient.recent)"
ALPHA7="$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.limit_share.seven_day.percent)"

echo "== 3. 既定出力に 7d / 5h の % 列が出る =="
TEXT="$("$TAKO_BIN" usage --by issue 2>&1)"
printf '%s\n' "$TEXT" | sed -e 's/^/    | /' | mask
HEAD75="$(printf '%6s%6s  %6s%6s' 7d '' 5h '')"
HEAD57="$(printf '%6s%6s  %6s%6s' 5h '' 7d '')"
case "$TEXT" in *"${HEAD75}（% は今の窓のうち各段が使った分・+ は推定）"*) pass "% 列の見出し（7d が左）" ;; *) fail "% 列の見出しが無い" ;; esac
case "$TEXT" in *"alpha #2202"*) pass "--by issue の段（alpha #2202）" ;; *) fail "--by issue の段が無い" ;; esac
case "$TEXT" in *"tako の外（区間にローカルの使用量が無いのに増えた分）"*) pass "tako の外の行" ;; *) fail "tako の外の行が無い" ;; esac
case "$TEXT" in *"7d の按分: 今の窓 20.0% = 段 17.0 + tako の外 3.0"*) pass "7d の按分の要約（観測 = 段 + tako の外）" ;; *) fail "7d の按分の要約が無い" ;; esac
case "$("$TAKO_BIN" usage --by project 2>&1)" in *"%"*"alpha  API 換算"*) pass "--by project にも % 列" ;; *) fail "--by project に % 列が無い" ;; esac

echo "== 境界: 絞り込み・暦の期間・5h の窓 =="
J="$(usage_json --by project --project alpha)"
check_eq "--project alpha の alpha の 7d は絞らないときと同じ（按分は絞る前の全行）" "$ALPHA7" \
  "$(printf '%s' "$J" | jf logins.0.usage.groups.name=alpha.limit_share.seven_day.percent)"
check_eq "--project alpha の段への配分は alpha だけ" "$ALPHA7" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.allocated)"
J="$(usage_json --by project --window day)"
check_eq "暦の期間は延べ（scope: period）" "period" "$(printf '%s' "$J" | jf logins.0.usage.limit_share.kind=seven_day.scope)"
J="$(usage_json --by project --window 5h)"
check_eq "--window 5h の 5h の合計 / 最新の観測" "8.00/8.00" "$(printf '%s' "$J" | share_sum five_hour)"
check_eq "--window 5h の段は 5h の % の大きい順（主の枠）" "$(printf '%s' "$J" | python3 -I -c 'import json,sys; g=json.load(sys.stdin)["logins"][0]["usage"]["groups"]; print(max(g, key=lambda x: x["limit_share"]["five_hour"]["percent"]+x["limit_share"]["five_hour"].get("estimated",0))["name"])')" \
  "$(printf '%s' "$J" | jf logins.0.usage.groups.0.name)"
case "$("$TAKO_BIN" usage --window 5h 2>&1)" in *"${HEAD57}（% は今の窓"*) pass "--window 5h は 5h の列が左" ;; *) fail "--window 5h の列の並び" ;; esac
CLI_GROUPS="$(usage_json --by issue | jf logins.0.usage.groups)"
# 問い合わせの時刻（to）は呼ぶたびに違うので除いて比べる
no_to() { python3 -I -c 'import json,sys; v=json.load(sys.stdin); [k.pop("to", None) for k in v]; print(json.dumps(v, sort_keys=True))'; }
CLI_SHARE="$(usage_json --by issue | jf logins.0.usage.limit_share | no_to)"

echo "== 3. 隔離 GUI: MCP tako_sessions の usage が CLI と同じ % を返す =="
if launch_isolated_gui "$TMP/app.log"; then
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app.log" || exit 1
  VHOME="${TMPDIR:-/tmp}"
  VHOME="${VHOME%/}/tako-agent-config-${APP_PID}"
  MCP="$(mcp_call tako_sessions '{"action":"usage","by":"issue"}')"
  check_eq "MCP の段（% 込み）が CLI と同じ" "$CLI_GROUPS" "$(printf '%s' "$MCP" | jf logins.0.usage.groups)"
  check_eq "MCP の枠ごとの按分が CLI と同じ（問い合わせの時刻 to を除く）" "$CLI_SHARE" "$(printf '%s' "$MCP" | jf logins.0.usage.limit_share | no_to)"
  check_eq "MCP の 7d の tako の外" "3" "$(printf '%s' "$MCP" | jf logins.0.usage.limit_share.kind=seven_day.outside)"
  stop_isolated_gui "$APP_PID"
  APP_PID=""
else
  UNMEASURED="隔離 GUI（tako-vd が起きない = 蓋閉じ等）"
  echo "  [--] 隔離 GUI を起こせなかった: MCP の節は未実測"
fi

echo
[ -n "$UNMEASURED" ] && echo "未実測: $UNMEASURED"
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
