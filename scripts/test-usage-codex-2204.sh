#!/usr/bin/env bash
# test-usage-codex-2204.sh — usage 分析 S5（#2204 / エピック #2192。FR-2.44.11〜12）の実経路テスト
#
# 偽の codex の rollout（一時 dir の CODEX_HOME）と偽の claude の会話記録を実 CLI と隔離 GUI に読ませる
# （codex / claude 不要・決定的）。番号は設計書 §9「S5」の受け入れ条件と #2204 の依頼の受け入れ条件:
#   1. token_count の累積の差分の和が最後の累積値と一致する（重複行・欠けた行・累積のリセット・書きかけの
#      最終行・壊れた行・空の rollout・token_count の無い rollout を含む）。2 回目は何も読まず、追記した分だけ読む
#   2. rate_limits の列（過去分）から区間按分ができる: 今の 7d / 5h の窓で、段の % の合計 + tako の外 = 最新の観測。
#      ローカルの使用量の無い区間の Δ は tako の外。codex は推定しない。観測は値ごとに 1 行（2 回目は増えない）
#   3. `tako usage` / MCP `tako_sessions` の usage で codex のログインの段が claude と同じ形で出る（`agent: codex`）
#   A/B: `TAKO_2192_S5_LEGACY=1` の同じバイナリでは codex を読まない
#
# **本番の tako / 本物の codex・claude の設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# rollout は `TAKO_USAGE_CODEX_HOMES`・会話記録は `TAKO_USAGE_CONFIG_DIRS` で一時 dir だけを読ませる（検証
# プロセスは指定が無ければ検証用のホームを読む = 本物の `~/.codex` / `~/.claude*` は読まない）。落とすのは
# 自分で起こした pid だけ（pkill / killall は使わない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue2204_usage_codex_watchdog.rs`）。
#
# 使い方: bash scripts/test-usage-codex-2204.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2192_S1_LEGACY TAKO_2192_S2_LEGACY TAKO_2192_S5_LEGACY \
  TAKO_USAGE_CONFIG_DIRS TAKO_USAGE_CODEX_HOMES CODEX_HOME
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

TMP="$(mktemp -d /tmp/tako-2204-XXXXXX)"
APP_PID=""
VHOME=""
TMUX_SOCKET="tako-2204-$$"
MASK_USER="$(id -un)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<TMP>#g"; }

cleanup() {
  stop_isolated_gui "$APP_PID"
  if [ -n "$VHOME" ]; then
    case "$VHOME" in */tako-agent-config-*) rm -rf "$VHOME" ;; esac
  fi
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${TMUX_SOCKET:?}"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "usage 分析 S5（#2204）の実経路テスト"

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
export TAKO_USAGE_CODEX_HOMES="$TMP/codex"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/cfg" "$TMP/codex"
USAGE="$TAKO_DATA_DIR/usage"
# codex の資格情報（account_id だけ見る。トークンの欄は印 = どこにも出ないこと）
printf '{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"tok-2204-secret","access_token":"tok-2204-secret","refresh_token":"tok-2204-secret","account_id":"acct-2204-codex"}}\n' > "$TMP/codex/auth.json"
printf '{"oauthAccount":{"accountUuid":"aaaaaaaa-2204-4222-8333-444444444444"}}\n' > "$TMP/cfg/.claude.json"

NOW="$(date +%s)"
# 分を指定して「今から N 分前」の ISO（UTC・ミリ秒つき。秒は刻みの頭からずらす = ターンの終わりの時刻らしく）
iso_ago() {
  python3 -I -c 'import datetime,sys; t=datetime.datetime.fromtimestamp(int(sys.argv[1]) - int(sys.argv[2]) * 60 - 17, datetime.timezone.utc); print(t.strftime("%Y-%m-%dT%H:%M:%S.250Z"))' "$NOW" "$1"
}
R5=$((NOW + 2 * 3600))   # 5h の解除（窓の開始 = 3 時間前）
R7=$((NOW + 3 * 86400))  # 7d の解除（窓の開始 = 4 日前）
CWD_ALPHA="/home/testuser/dev/alpha"
CWD_BETA="/home/testuser/dev/beta"
meta() { # 会話 cwd 何分前
  printf '{"timestamp":"%s","type":"session_meta","payload":{"id":"%s","timestamp":"%s","cwd":"%s","originator":"codex_cli_rs","cli_version":"0.150.1","source":"cli","base_instructions":{"text":"指示の印 instr-2204"},"git":{"branch":"feat/branch-2204"}}}\n' \
    "$(iso_ago "$3")" "$1" "$(iso_ago "$3")" "$2"
}
turn() { # モデル cwd 何分前
  printf '{"timestamp":"%s","type":"turn_context","payload":{"turn_id":"t","cwd":"%s","model":"%s","effort":"high","user_instructions":"instr-2204"}}\n' "$(iso_ago "$3")" "$2" "$1"
}
body() { # 何分前
  printf '{"timestamp":"%s","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"本文の印 body-2204"}]}}\n' "$(iso_ago "$1")"
}
usage_obj() { # 入力 読み出し 出力
  printf '{"input_tokens":%s,"cached_input_tokens":%s,"cache_write_input_tokens":0,"output_tokens":%s,"reasoning_output_tokens":0,"total_tokens":%s}' "$1" "$2" "$3" "$(($1 + $3))"
}
limits_obj() { # 5h の % 7d の %
  printf '{"limit_id":"codex","limit_name":null,"primary":{"used_percent":%s,"window_minutes":300,"resets_at":%s},"secondary":{"used_percent":%s,"window_minutes":10080,"resets_at":%s},"credits":null,"plan_type":"plus","rate_limit_reached_type":null}' "$1" "$R5" "$2" "$R7"
}
tc() { # 何分前 累積（入力 読み出し 出力） この回（入力 出力） 5h% 7d%
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":%s,"last_token_usage":%s,"model_context_window":258400},"rate_limits":%s}}\n' \
    "$(iso_ago "$1")" "$(usage_obj "$2" "$3" "$4")" "$(usage_obj "$5" 0 "$6")" "$(limits_obj "$7" "$8")"
}
rate_only() { # 何分前 5h% 7d%（info: null = 使用量の無い観測）
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":%s}}\n' "$(iso_ago "$1")" "$(limits_obj "$2" "$3")"
}

DAY="$TMP/codex/sessions/2026/10/17"
mkdir -p "$DAY"
RA="$DAY/rollout-2026-10-17T00-00-00-thread-alpha.jsonl"
RB="$DAY/rollout-2026-10-17T00-00-01-thread-beta.jsonl"
# alpha: 累積 1000 → （重複）→ 3000 → （欠けた行）→ 7000 = 差の和 7000
{
  meta thread-alpha "$CWD_ALPHA" 175
  turn gpt-5.5 "$CWD_ALPHA" 174
  body 173
  tc 170 900 300 100 900 100 2 10
  tc 169 900 300 100 900 100 2 10
  tc 120 2700 1500 300 1800 200 4 12
  tc 60 6200 4000 800 3000 300 6 15
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"token_count","info":{"total_tok\n' "$(iso_ago 59)"
} > "$RA"
# beta: 累積 2000 → 2500 → リセット（300）→ 800 = 2500 + 800。最後は書きかけ（改行なし）
{
  meta thread-beta "$CWD_BETA" 105
  turn gpt-5.6-sol "$CWD_BETA" 104
  tc 100 1800 0 200 1800 200 5 13
  tc 52 2200 0 300 400 100 7 16
  tc 40 250 0 50 250 50 7 17
  tc 30 700 0 100 450 50 8 18
  # 30 分前の後に使用量の無いまま増えた（20 → 10 分前の区間 = tako の外）
  rate_only 20 9 19
  rate_only 10 10 21
  printf '%s' "$(tc 3 900 0 120 200 20 10 21)"
} > "$RB"
: > "$DAY/rollout-2026-10-17T00-00-02-thread-empty.jsonl"
{
  meta thread-quiet "$CWD_ALPHA" 90
  turn gpt-5.5 "$CWD_ALPHA" 90
  body 89
} > "$DAY/rollout-2026-10-17T00-00-03-thread-quiet.jsonl"
# claude の会話記録（同じ形で並ぶことの比較用・観測なし）
mkdir -p "$TMP/cfg/projects/-home-testuser-dev-alpha"
printf '{"type":"assistant","message":{"id":"msg_c1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"本文"}],"usage":{"input_tokens":0,"output_tokens":1000,"cache_read_input_tokens":100000,"speed":"standard"}},"requestId":"req_c1","timestamp":"%s","sessionId":"s-claude","cwd":"%s","uuid":"u-c1"}\n' \
  "$(iso_ago 50)" "$CWD_ALPHA" > "$TMP/cfg/projects/-home-testuser-dev-alpha/s-claude.jsonl"
cat > "$TAKO_ORCHESTRATOR_DIR/projects.yaml" <<EOF
projects:
  alpha:
    cwd: $CWD_ALPHA
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
    elif k.startswith("agent=") and isinstance(v, list):
        v = next((w for w in v if isinstance(w, dict) and w.get("agent") == k[6:]), None)
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
# codex のログインの段の % の合計 + tako の外（枠 $1）と最新の観測を「合計/観測」で返す
share_sum() {
  python3 -I -c '
import json, sys
kind = sys.argv[1]
l = next(x for x in json.load(sys.stdin)["logins"] if x.get("agent") == "codex")
u = l["usage"]
ks = next(k for k in u["limit_share"] if k["kind"] == kind)
total = sum((g.get("limit_share") or {}).get(kind, {}).get("percent", 0) for g in u["groups"]) + ks["outside"]
print("%.2f/%.2f" % (total, ks["latest_percent"]))' "$1"
}
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2204","version":"0"}}}' \
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
codex_rows() { python3 -I -c 'import json,sys; print(sum(1 for l in open(sys.argv[1]) if json.loads(l).get("source") == "codex"))' "$USAGE/limits.jsonl" 2>/dev/null || echo 0; }

echo "== A/B: TAKO_2192_S5_LEGACY=1 では codex を読まない =="
J="$(TAKO_2192_S5_LEGACY=1 usage_json --window all --by session)"
check_eq "A/B: codex の rollout を数えない" "0" "$(printf '%s' "$J" | jf index.update.codex_files)"
check_eq "A/B: codex のログインが出ない" "null" "$(printf '%s' "$J" | jf logins.agent=codex)"
check_eq "A/B: claude の会話記録は読む" "claude" "$(printf '%s' "$J" | jf logins.0.agent)"

echo "== 1. 累積の差分の和 = 最後の累積（重複・欠けた行・リセット・書きかけ・壊れた行・空・token_count 無し）=="
J="$(usage_json --window all --by session --limit 50)"
check_eq "codex の rollout 4 本を見つけた" "4" "$(printf '%s' "$J" | jf index.update.codex_files)"
check_eq "alpha の合計 = 最後の累積 6200 + 800" "7000" \
  "$(printf '%s' "$J" | python3 -I -c 'import json,sys; l=next(x for x in json.load(sys.stdin)["logins"] if x.get("agent")=="codex"); print(next(g["total_tokens"] for g in l["usage"]["groups"] if g["key"].get("session")=="thread-alpha"))')"
check_eq "beta の合計 = リセット前の 2500 + 後の 800（書きかけの最終行は数えない）" "3300" \
  "$(printf '%s' "$J" | python3 -I -c 'import json,sys; l=next(x for x in json.load(sys.stdin)["logins"] if x.get("agent")=="codex"); print(next(g["total_tokens"] for g in l["usage"]["groups"] if g["key"].get("session")=="thread-beta"))')"
check_eq "応答の数（alpha 3 + beta 4。重複行と使用量の無い観測は数えない）" "7" "$(printf '%s' "$J" | jf logins.agent=codex.usage.total.responses)"
check_eq "codex のログインの鍵は account_id から（key_from: account）" "account" "$(printf '%s' "$J" | jf logins.agent=codex.key_from)"
check_eq "codex のモデルは単価表に無い（API 換算 0・単価不明に数える）" "10300" "$(printf '%s' "$J" | jf logins.agent=codex.usage.total.unpriced_tokens)"
case "$(printf '%s' "$J" | jf unpriced_models)" in *gpt-5.6-sol*) pass "単価表に無いモデルの注記（gpt-5.6-sol）" ;; *) fail "単価表に無いモデルの注記が無い" ;; esac
ROWS1="$(codex_rows)"
J2="$(usage_json --window all --by session)"
check_eq "2 回目は何も読まない" "0" "$(printf '%s' "$J2" | jf index.update.bytes_read)"
check_eq "2 回目は観測を書き足さない" "$ROWS1" "$(codex_rows)"
# 書きかけの最終行を書き終えた: 差（900 + 120 − 800）だけ足す
printf '\n' >> "$RB"
J3="$(usage_json --window all --by session --limit 50)"
check_eq "書き終えた最終行の差だけ足した（beta 3300 → 3520）" "3520" \
  "$(printf '%s' "$J3" | python3 -I -c 'import json,sys; l=next(x for x in json.load(sys.stdin)["logins"] if x.get("agent")=="codex"); print(next(g["total_tokens"] for g in l["usage"]["groups"] if g["key"].get("session")=="thread-beta"))')"
check_eq "追記した後に読んだ rollout は 1 本" "1" "$(printf '%s' "$J3" | jf index.update.read_files)"

echo "== 2. 過去の rate_limits から区間按分（今の 7d / 5h の窓）=="
J="$(usage_json --by project)"
check_eq "7d: 段の合計 + tako の外 / 最新の観測" "21.00/21.00" "$(printf '%s' "$J" | share_sum seven_day)"
check_eq "5h: 段の合計 + tako の外 / 最新の観測" "10.00/10.00" "$(printf '%s' "$J" | share_sum five_hour)"
check_eq "7d の tako の外（20 → 10 分前に使用量なしで 2% 増えた）" "2" "$(printf '%s' "$J" | jf logins.agent=codex.usage.limit_share.kind=seven_day.outside)"
check_eq "5h の tako の外" "1" "$(printf '%s' "$J" | jf logins.agent=codex.usage.limit_share.kind=five_hour.outside)"
check_eq "codex は推定しない（使用量と観測が同じ行に載る）" "0" "$(printf '%s' "$J" | jf logins.agent=codex.usage.limit_share.kind=seven_day.estimated)"
check_eq "観測は今の窓（current_window）" "current_window" "$(printf '%s' "$J" | jf logins.agent=codex.usage.limit_share.kind=seven_day.scope)"
check_eq "観測は値ごとに 1 行（5h 8 値 + 7d 9 値。重複行・同じ値の後の報告は書かない）" "17" "$(codex_rows)"
for name in alpha beta; do
  P="$(printf '%s' "$J" | jf "logins.agent=codex.usage.groups.name=${name}.limit_share.seven_day.percent")"
  case "$P" in null | absent | 0) fail "$name に 7d の % が配られない（${P}）" ;; *) pass "$name に 7d の % が配られた（${P}%）" ;; esac
done

echo "== 3. tako usage の既定出力: codex のログインが claude と同じ形で並ぶ =="
TEXT="$("$TAKO_BIN" usage --by project 2>&1)"
printf '%s\n' "$TEXT" | sed -e 's/^/    | /' | mask
case "$TEXT" in *"（codex・鍵 "*) pass "codex のログインの見出し（codex・鍵）" ;; *) fail "codex のログインの見出しが無い" ;; esac
case "$TEXT" in *"7d の按分: 今の窓 21.0% = 段 19.0 + tako の外 2.0・1% あたり トークン"*) pass "7d の按分の要約（1% あたりのトークン）" ;; *) fail "7d の按分の要約が無い" ;; esac
case "$TEXT" in *"codex は rollout の token_count"*) pass "codex の読み方の注記" ;; *) fail "codex の読み方の注記が無い" ;; esac
shape() { # ログインの欄と usage の欄（段の行は値の無い欄を省くので、欄の名前の和で見る）
  python3 -I -c 'import json,sys; l=next(x for x in json.load(sys.stdin)["logins"] if x.get("agent")==sys.argv[1]); print(sorted(k for k in l.keys() if not k.endswith("observed_at")), sorted(l["usage"].keys()), sorted(l["usage"]["limit_share"][0].keys() & {"kind","available","scope","observed","allocated","outside","estimated"}))' "$1"
}
check_eq "claude と codex のログインの形が同じ（欄・usage・limit_share）" "$(printf '%s' "$J" | shape claude)" "$(printf '%s' "$J" | shape codex)"
check_eq "claude と codex のログインは別の鍵" "2" "$(printf '%s' "$J" | python3 -I -c 'import json,sys; print(len({l["login"] for l in json.load(sys.stdin)["logins"]}))')"
# 本文・指示・cwd・ブランチ・トークン・account_id が索引に出ない
LEAK=""
for needle in body-2204 instr-2204 branch-2204 tok-2204-secret acct-2204-codex "$CWD_ALPHA" "$TMP/codex"; do
  if grep -rqF "$needle" "$USAGE" 2>/dev/null; then LEAK="$LEAK $needle"; fi
done
check_eq "本文・指示・cwd・ブランチ・資格情報が usage/ に出ない" "" "$LEAK"
CLI_GROUPS="$(usage_json --by issue | jf logins.agent=codex.usage.groups)"
no_to() { python3 -I -c 'import json,sys; v=json.load(sys.stdin); [k.pop("to", None) for k in v]; print(json.dumps(v, sort_keys=True))'; }
CLI_SHARE="$(usage_json --by issue | jf logins.agent=codex.usage.limit_share | no_to)"

echo "== 3. 隔離 GUI: MCP tako_sessions の usage が CLI と同じ codex の段を返す =="
if launch_isolated_gui "$TMP/app.log"; then
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app.log" || exit 1
  VHOME="${TMPDIR:-/tmp}"
  VHOME="${VHOME%/}/tako-agent-config-${APP_PID}"
  MCP="$(mcp_call tako_sessions '{"action":"usage","by":"issue"}')"
  check_eq "MCP の codex のログイン" "codex" "$(printf '%s' "$MCP" | jf logins.agent=codex.agent)"
  check_eq "MCP の codex の段（% 込み）が CLI と同じ" "$CLI_GROUPS" "$(printf '%s' "$MCP" | jf logins.agent=codex.usage.groups)"
  check_eq "MCP の codex の枠ごとの按分が CLI と同じ（問い合わせの時刻 to を除く）" "$CLI_SHARE" "$(printf '%s' "$MCP" | jf logins.agent=codex.usage.limit_share | no_to)"
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
