#!/usr/bin/env bash
# test-usage-limits-2200.sh — usage 分析 S1（#2200 / エピック #2192。FR-2.44）の実経路テスト
#
# 隔離 GUI（tako-vd）+ 実 CLI。報告は mod と同じ口（`tako mod report` の stdin）で直に送る
# （claude 不要・決定的）。番号は設計書 §9「S1」の受け入れ条件:
#   1. % か解除時刻が変わったときだけ `limits.jsonl` が 1 行増える（同じ値の heartbeat・同じログインの
#      別の設定 dir の同じ値・放置したペインの古い観測では増えない）
#   2. 同じログインの 2 つの設定 dir が同じ鍵・別のログインは別の鍵。既定の設定 dir の 2 通り
#      （`CLAUDE_CONFIG_DIR` 無し / `=~/.claude`）が資格情報のファイルどおりに別の鍵へ入る。
#      `.claude.json` が無い・読めないときは設定 dir の鍵へ落ちる。鍵の式（塩付きハッシュ）は
#      この側でも計算して突き合わせる
#   3. 生の accountUuid（とその断片）・メール・設定 dir のパスが `usage/` と persist.log に出ない
#   4. 起動しても `limits.jsonl` を読まない（atime が古いまま）→ 最初の追記の前に末尾から覚える
#      （atime が進み、前回と同じ値は書かない）。`tako usage` と MCP `tako_sessions` の
#      `action: "usage"` が同じ要約を返す
#   5. A/B: `TAKO_2192_S1_LEGACY=1` の同じバイナリでは書かない（置き場も作らない）
#
# **本番の tako / 本物の claude の設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 資格情報は偽物を一時 dir と隔離 GUI の検証用ホーム（`$TMPDIR/tako-agent-config-<GUI の pid>`。
# `CLAUDE_CONFIG_DIR` 無しの claude が読む `~/.claude.json` の隔離先）に置く。落とすのは自分で起こした
# pid だけ（pkill / killall は使わない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue2200_usage_watchdog.rs`）。
#
# 使い方: bash scripts/test-usage-limits-2200.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_2192_S1_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_ne() {
  if [ "$2" != "$3" ]; then pass "$1"; else fail "$1（どちらも '${2}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2200-XXXXXX)"
APP_PID=""
VHOME=""
TMUX_SOCKET="tako-2200-$$"
TMUX_SOCKET_AB="tako-2200ab-$$"
MASK_USER="$(id -un)"
MASK_TMPDIR="${TMPDIR:-/tmp}"
MASK_TMPDIR="${MASK_TMPDIR%/}"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<TMP>#g" -e "s#${MASK_TMPDIR}#<TMPDIR>#g"; }

cleanup() {
  stop_isolated_gui "$APP_PID"
  # 検証用ホームは自分の GUI の pid の分だけ消す（製品バイナリの検証起動は自分では消さない）
  if [ -n "$VHOME" ]; then
    case "$VHOME" in */tako-agent-config-*) rm -rf "$VHOME" ;; esac
  fi
  for s in "$TMUX_SOCKET" "$TMUX_SOCKET_AB"; do
    tmux -L "$s" kill-server >/dev/null 2>&1 || true
    rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${s:?}"
  done
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "usage 分析 S1（#2200）の実経路テスト"

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
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
USAGE="$TAKO_DATA_DIR/usage"
LIMITS="$USAGE/limits.jsonl"
SESSIONS="$USAGE/sessions.jsonl"

# 偽の資格情報（ログインは 3 つ。メールと本文に当たるキーも入れて、出ないことを見る）
UUID_A="a2200000-aaaa-4aaa-8aaa-000000000001"
UUID_B="b2200000-bbbb-4bbb-8bbb-000000000002"
UUID_C="c2200000-cccc-4ccc-8ccc-000000000003"
EMAIL="usage-2200@example.com"
SALT="5a175a175a175a175a175a175a175a17"
claude_json() {
  mkdir -p "$1"
  printf '{"projects":{"/x":{"lastPrompt":"body-marker-2200"}},"oauthAccount":{"accountUuid":"%s","emailAddress":"%s"}}' \
    "$2" "$EMAIL" > "$1/.claude.json"
}
# 鍵の式（tako_core::usage::salted_key と同じ）をこの側でも計算する
key_of() {
  python3 -I -c '
import hashlib, sys
domain, salt, value = sys.argv[1:4]
h = hashlib.sha256()
for part in (b"tako.usage.login.v1\n", domain.encode(), b"\n", salt.encode(), b"\n", value.encode()):
    h.update(part)
print(h.hexdigest()[:12])' "$1" "$SALT" "$2"
}
lines_of() { if [ -f "$1" ]; then grep -c . "$1"; else echo 0; fi; }
atime_of() { stat -f %a "$1" 2>/dev/null || echo absent; }
# sessions.jsonl の 1 欄（後ろの行が正）
session_f() {
  python3 -I -c '
import json, sys
want, field = sys.argv[2], sys.argv[3]
v = "absent"
try:
    for line in open(sys.argv[1], encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        row = json.loads(line)
        if row.get("session_id") == want:
            v = row.get(field)
except FileNotFoundError:
    pass
print(v)' "$SESSIONS" "$1" "$2"
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
    if k.startswith("kind=") and isinstance(v, list):
        v = next((w for w in v if isinstance(w, dict) and w.get("kind") == k[5:]), None)
    elif k.startswith("label=") and isinstance(v, list):
        v = next((w for w in v if isinstance(w, dict) and w.get("label") == k[6:]), None)
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
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2200","version":"0"}}}' \
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
root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}
split_pane() { "$TAKO_BIN" split --pane "$1" --down 2>/dev/null | tr -dc '0-9'; }
# 疑似の報告を 1 本送る（mod と同じ口。送り主は TAKO_PANE_ID）
send_report() { printf '%s' "$2" | TAKO_PANE_ID="$1" "$TAKO_BIN" mod report >/dev/null 2>&1; }
R5A="2026-10-17T03:20:00.000Z"
R5B="2026-10-17T08:20:00.000Z"
R7="2026-10-20T22:00:00.000Z"
# 報告の本体（会話 / 設定 dir（空 = 既定）/ 7d の % と観測時刻 / 5h の % と解除時刻と観測時刻（任意））
report_json() {
  local session="$1" cfg="$2" week="$3" week_at="$4" five="${5:-}" five_reset="${6:-}" five_at="${7:-}"
  local limits="{\"kind\":\"seven_day\",\"percent_used\":${week},\"resets_at\":\"${R7}\",\"observed_at\":${week_at}}"
  if [ -n "$five" ]; then
    limits="{\"kind\":\"five_hour\",\"percent_used\":${five},\"resets_at\":\"${five_reset}\",\"observed_at\":${five_at}},${limits}"
  fi
  local cfg_part=""
  [ -n "$cfg" ] && cfg_part=",\"config_dir\":\"${cfg}\""
  printf '{"schema":1,"mod_version":"t2200","claude_version":"2.1.301","session_id":"%s","at":1,"turn":"idle","rate_limits":[%s]%s,"ended":false}' \
    "$session" "$limits" "$cfg_part"
}
# 行数が期待に達するまで待つ（定期処理は 2 秒ごと）。届かなければ最後の数
wait_lines() {
  local file="$1" want="$2" i n=0
  for i in $(seq 1 40); do
    n="$(lines_of "$file")"
    [ "$n" -ge "$want" ] && break
    sleep 0.25
  done
  echo "$n"
}
# 「増えない」を見るときは定期処理を 2 周待つ
settle() { sleep 4.5; }

T0=1792150000000
T1=1792150600000
T2=1792151200000

echo "== 準備: 塩と前回の観測（ログイン A の 7d 40%）を置き、atime を古くする =="
mkdir -p "$USAGE"
printf '%s' "$SALT" > "$TAKO_DATA_DIR/machine_id"
KEY_A="$(key_of account "$UUID_A")"
KEY_B="$(key_of account "$UUID_B")"
KEY_C="$(key_of account "$UUID_C")"
R7_SECS="$(python3 -I -c 'import calendar, time; print(calendar.timegm(time.strptime("2026-10-20T22:00:00", "%Y-%m-%dT%H:%M:%S")))')"
printf '{"login":"%s","key_from":"account","kind":"seven_day","percent":40.0,"resets_at":%s,"observed_at":%s,"recorded_at":%s,"source":"mod"}\n' \
  "$KEY_A" "$R7_SECS" "$T0" "$T0" > "$LIMITS"
touch -a -t 202001010000 "$LIMITS"
ATIME_BEFORE="$(atime_of "$LIMITS")"

echo "== 新挙動: 隔離 GUI を起動 =="
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
VHOME="${TMPDIR:-/tmp}"
VHOME="${VHOME%/}/tako-agent-config-${APP_PID}"

echo "-- 4. 起動しても limits.jsonl を読まない"
settle
check_eq "起動して定期処理が 2 周回っても atime が古いまま（読んでいない）" "$ATIME_BEFORE" "$(atime_of "$LIMITS")"
check_eq "会話の対応はまだ無い" "0" "$(lines_of "$SESSIONS")"

# 偽の資格情報: cfg1 / cfg2 = A、cfg3 = C、検証用ホームの ~/.claude.json = A・~/.claude/.claude.json = B、
# missing = 無し、broken = 壊れた JSON
claude_json "$TMP/cfg1" "$UUID_A"
claude_json "$TMP/cfg2" "$UUID_A"
claude_json "$TMP/cfg3" "$UUID_C"
claude_json "$VHOME" "$UUID_A"
claude_json "$VHOME/.claude" "$UUID_B"
mkdir -p "$TMP/missing" "$TMP/broken"
printf '{ broken' > "$TMP/broken/.claude.json"

P1="$(root_pane)"
P2="$(split_pane "$P1")"
[ -n "$P1" ] && [ -n "$P2" ] || { fail "ペインを用意できない（P1=${P1} P2=${P2}）"; exit 1; }

echo "-- 1. 値が変わったときだけ 1 行（前回の観測と同じ値は書かない = 末尾から覚えた）"
send_report "$P1" "$(report_json s-a1 "$TMP/cfg1" 40 "$T0")"
check_eq "初めての会話の対応が 1 行" "1" "$(wait_lines "$SESSIONS" 1)"
settle
check_eq "前回（起動前）と同じ 7d 40% は書かない" "1" "$(lines_of "$LIMITS")"
check_ne "最初の追記の前に末尾を読んだ（atime が進んだ）" "$ATIME_BEFORE" "$(atime_of "$LIMITS")"
for _ in 1 2 3; do send_report "$P1" "$(report_json s-a1 "$TMP/cfg1" 40 "$T0")"; done
send_report "$P1" "$(report_json s-a1 "$TMP/cfg1" 41 "$T1")"
check_eq "heartbeat 3 回の後に % が変わった分だけ 1 行（40 → 41）" "2" "$(wait_lines "$LIMITS" 2)"
send_report "$P1" "$(report_json s-a1 "$TMP/cfg1" 41 "$T1" 10 "$R5A" "$T1")"
check_eq "5h が初めて出た = 1 行" "3" "$(wait_lines "$LIMITS" 3)"
send_report "$P1" "$(report_json s-a1 "$TMP/cfg1" 41 "$T1" 0 "$R5B" "$T2")"
check_eq "解除時刻だけが変わった（新しい窓）= 1 行" "4" "$(wait_lines "$LIMITS" 4)"
# 同じログインの別の設定 dir（cfg2）・同じ値 → 書かない。放置したペインの古い観測（39%・T0 より前）→ 書かない
send_report "$P2" "$(report_json s-a2 "$TMP/cfg2" 41 "$T1")"
send_report "$P2" "$(report_json s-a2 "$TMP/cfg2" 39 "$((T0 - 1000))")"
send_report "$P2" "$(report_json s-a2 "$TMP/cfg2" 41 "$T1")"
send_report "$P2" "$(report_json s-a2 "$TMP/cfg2" 39 "$((T0 - 1000))")"
settle
check_eq "同じログインの別の設定 dir の同じ値・放置したペインの古い値では増えない" "4" "$(lines_of "$LIMITS")"

echo "-- 2. ログインの鍵"
send_report "$P2" "$(report_json s-c "$TMP/cfg3" 41 "$T1")"
send_report "$P2" "$(report_json s-none "" 41 "$T1")"
send_report "$P2" "$(report_json s-env "$VHOME/.claude" 41 "$T1")"
send_report "$P2" "$(report_json s-missing "$TMP/missing" 41 "$T1")"
send_report "$P2" "$(report_json s-broken "$TMP/broken" 41 "$T1")"
wait_lines "$SESSIONS" 7 >/dev/null
check_eq "鍵の式がこの側の計算と一致（cfg1 = ログイン A）" "$KEY_A" "$(session_f s-a1 login)"
check_eq "同じログインの 2 つの設定 dir（cfg1 / cfg2）が同じ鍵" "$(session_f s-a1 login)" "$(session_f s-a2 login)"
check_eq "別のログイン（cfg3）は別の鍵 = ログイン C の式" "$KEY_C" "$(session_f s-c login)"
check_eq "CLAUDE_CONFIG_DIR 無し = ~/.claude.json（A）" "$KEY_A" "$(session_f s-none login)"
check_eq "=~/.claude = ~/.claude/.claude.json（B）" "$KEY_B" "$(session_f s-env login)"
check_ne "既定の設定 dir の 2 通りが別の鍵" "$(session_f s-none login)" "$(session_f s-env login)"
check_eq "資格情報が無い → 設定 dir の鍵" "config_dir" "$(session_f s-missing key_from)"
check_eq "資格情報が壊れている → 設定 dir の鍵" "config_dir" "$(session_f s-broken key_from)"
check_ne "無い / 壊れているは別の設定 dir = 別の鍵" "$(session_f s-missing login)" "$(session_f s-broken login)"
check_eq "観測は C / B / missing / broken の 4 ログインぶん増える（A の同じ値は増えない）" "8" "$(wait_lines "$LIMITS" 8)"

echo "-- 3. 生の識別子・メール・設定 dir が usage/ と persist.log の usage の行に出ない"
# persist.log は usage の行だけを見る（同じ報告で #1959 が「設定 dir へ skills/tako を置いた」を
# 置き場のパスつきで書くのは既存の設計 = この Issue の対象外）
grep 'usage:' "$TAKO_DATA_DIR/persist.log" > "$TMP/persist-usage.log" 2>/dev/null || true
LEAK=""
for f in "$LIMITS" "$SESSIONS" "$TMP/persist-usage.log"; do
  for needle in "$UUID_A" "$UUID_B" "$UUID_C" "${UUID_A%%-*}" "$EMAIL" "body-marker-2200" "$TMP" "$VHOME"; do
    if grep -qF -- "$needle" "$f" 2>/dev/null; then LEAK="${LEAK} $(basename "$f"):$(printf '%s' "$needle" | mask)"; fi
  done
done
check_eq "生の値・パスの漏れ" "" "$LEAK"
check_eq "persist.log の usage の行は件数だけ（1 行）" "1" "$(grep -c 'usage:' "$TAKO_DATA_DIR/persist.log" 2>/dev/null || echo 0)"
grep 'usage:' "$TAKO_DATA_DIR/persist.log" | sed -e 's/^/    /' | mask

echo "-- 4. tako usage と MCP tako_sessions action=usage"
CLI_JSON="$("$TAKO_BIN" usage --json 2>/dev/null)"
MCP_JSON="$(mcp_call tako_sessions '{"action":"usage"}')"
check_eq "観測の点数（CLI）" "8" "$(printf '%s' "$CLI_JSON" | jf observations)"
check_eq "ログインの数（A / C / B / missing / broken）" "5" "$(printf '%s' "$CLI_JSON" | jf logins | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin)))')"
check_eq "ログイン A の 7d の最新" "41" "$(printf '%s' "$CLI_JSON" | jf "logins.label=A.windows.kind=seven_day.used_percent")"
check_eq "ログイン A の 5h の最新（新しい窓の 0%）" "0" "$(printf '%s' "$CLI_JSON" | jf "logins.label=A.windows.kind=five_hour.used_percent")"
check_eq "ログイン A の鍵" "$KEY_A" "$(printf '%s' "$CLI_JSON" | jf "logins.label=A.login")"
check_eq "ログイン A の会話（s-a1 / s-a2 / s-none）" "3" "$(printf '%s' "$CLI_JSON" | jf "logins.label=A.sessions")"
check_eq "MCP の要約は CLI と同じ" "$(printf '%s' "$CLI_JSON" | jf logins)" "$(printf '%s' "$MCP_JSON" | jf logins)"
check_eq "MCP の recording" "true" "$(printf '%s' "$MCP_JSON" | jf recording)"
echo "    tako usage の既定出力:"
"$TAKO_BIN" usage 2>&1 | sed -e 's/^/    | /' | mask
cp "$LIMITS" "$TMP/limits-new.jsonl"
stop_isolated_gui "$APP_PID"
APP_PID=""
case "$VHOME" in */tako-agent-config-*) rm -rf "$VHOME" ;; esac
VHOME=""

echo "== 5. A/B: TAKO_2192_S1_LEGACY=1（同じバイナリ・dir とソケットを分ける）=="
export TAKO_DATA_DIR="$TMP/data-ab"
export TAKO_DISCOVERY_DIR="$TMP/disc-ab"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch-ab"
export TAKO_SESSIONS_FILE="$TMP/sessions-ab.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs-ab"
export TAKO_WORKERS_FILE="$TMP/workers-ab.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET_AB"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
launch_isolated_gui "$TMP/app-ab.log" TAKO_2192_S1_LEGACY=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-ab.log" || exit 1
VHOME="${TMPDIR:-/tmp}"
VHOME="${VHOME%/}/tako-agent-config-${APP_PID}"
P1="$(root_pane)"
send_report "$P1" "$(report_json s-ab "$TMP/cfg1" 50 "$T1")"
send_report "$P1" "$(report_json s-ab "$TMP/cfg1" 51 "$T2")"
settle
check_eq "旧挙動は usage/ を作らない" "absent" "$([ -e "$TAKO_DATA_DIR/usage" ] && echo present || echo absent)"
check_eq "旧挙動の要約は recording=false" "false" "$(TAKO_2192_S1_LEGACY=1 "$TAKO_BIN" usage --json 2>/dev/null | jf recording)"

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
