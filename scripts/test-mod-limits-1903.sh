#!/usr/bin/env bash
# test-mod-limits-1903.sh — tako mod S2 の続き（#1903。FR-2.42.11 / FR-2.42.18）の実経路テスト
#
# 段（ONLY=visual|fake|claude で選ぶ。既定は全部）:
#   visual — visual-test `mod-limits`: 隔離 GUI（tako-vd）の実ピクセルで、ステータスバーが mod の値
#            （42 / 18）を描き、報告が古くなると戻る。帯の絵を書き出す（新旧で dir を分ける）。
#            A/B: `TAKO_1903_LEGACY=1` の同じバイナリでは ① が FAILED
#   fake   — 実 GUI + 実 CLI。報告は mod と同じ口（`tako mod report`）で直に送る（claude 不要・決定的）:
#            1. 画面（5h 23% · 7d 46%）と mod（42.4 / 17.5）が違う状態でステータスバーが mod の値
#               （`tako limit-service --refresh` の claude）。報告が止まると 45 秒で画面の値へ戻る
#            2. 同じアカウントの放置したペイン（80%・1 時間前の観測）と動いているペイン（20%・今の観測）
#               → `orchestrator self` / `worker_status` / ステータスバーの束ねた値が動いているペインのもの
#            エッジ: mod の無いペインがフォーカス・アカウントが違うペインが混ざる
#            A/B: `TAKO_1903_LEGACY=1` で束ね方が #1880（同じ窓の大きい方 = 放置ペインの 80%）、
#                 ステータスバーは画面の値
#   claude — 実 claude（haiku の短いターン）: 使用制限の observed_at が heartbeat を 2 回またいでも
#            動かない（報告の数と at は進む）・ステータスバーが mod の値
#
# **本番の tako / 本番の claude 設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 落とすのは自分で起こした pid だけ（pkill / killall は使わない）。claude は利用者のログインシェルの
# 設定 dir を**読むだけ**で使う（認証が要るため）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI と実 claude の認証が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue1903_mod_limits_watchdog.rs`）。
#
# 使い方: bash scripts/test-mod-limits-1903.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_1903_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

TMP="$(mktemp -d /tmp/tako-1903-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1903-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g" -e "s#${TMP}#<TMP>#g"; }

CCD="$("${SHELL:-/bin/zsh}" -l -c 'printf %s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"' 2>/dev/null)"
[ -n "$CCD" ] || CCD="$HOME/.claude"
INLINE_DATA="$CCD/plugins/data/tako-inline"
INLINE_EXISTED=0
[ -d "$INLINE_DATA" ] && INLINE_EXISTED=1
WORKDIR="${CLAUDE_WORKDIR:-$(git -C "$REPO_ROOT" worktree list --porcelain | awk 'NR == 1 { print $2 }')}"
# 検証の画像（新旧で dir を分ける）
DUMP_NEW="${TAKO_1903_DUMP_DIR:-$TMP/dump}/new"
DUMP_OLD="${TAKO_1903_DUMP_DIR:-$TMP/dump}/legacy"

cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET"
  if [ "$INLINE_EXISTED" = 0 ] && [ -d "$INLINE_DATA" ]; then
    rmdir "$INLINE_DATA" 2>/dev/null || true
  fi
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "tako mod S2 の続き（#1903）の実経路テスト"

# visual-test の節を持つ GUI を作る（実 GUI の段も同じバイナリで回す = 1867 と同じ）
(cd "$REPO_ROOT" && cargo build -q -p tako-app --features visual-test && cargo build -q -p tako-cli) || exit 1
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
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DUMP_NEW" "$DUMP_OLD"

start_app() {
  local name="$1"
  shift
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# --- 観測の道具 -----------------------------------------------------------------
# JSON の 1 項目（ドット区切り。数字はリストの添字・`kind=X` は windows の行）
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
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
self_f() { "$TAKO_BIN" orchestrator self --pane "$1" 2>/dev/null | jf "$2"; }
status_f() { "$TAKO_BIN" orchestrator status --pane "$1" 2>/dev/null | jf "$2"; }
# ステータスバーの 5h / 7d と取得元（`tako limit-service --refresh` = MCP tako_limit_service の refresh）
bar() { "$TAKO_BIN" limit-service --refresh 2>/dev/null | jf "claude"; }
bar_f() { "$TAKO_BIN" limit-service --refresh 2>/dev/null | jf "claude.$1"; }
bar_line() { echo "$(bar_f 5h)/$(bar_f 7d)/$(bar_f source)"; }
bar_is() { [ "$(bar_line)" = "$1" ]; }
mod_row_f() {
  "$TAKO_BIN" mod --json 2>/dev/null | python3 -I -c '
import json, sys
want, path = int(sys.argv[1]), [k for k in sys.argv[2].split(".") if k]
try:
    d = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
v = next((r for r in d.get("panes", []) if r.get("pane") == want), None)
for k in path:
    if k.startswith("kind=") and isinstance(v, list):
        v = next((w for w in v if isinstance(w, dict) and w.get("kind") == k[5:]), None)
    elif isinstance(v, dict):
        v = v.get(k)
    else:
        v = None
    if v is None:
        break
print("null" if v is None else v)' "$1" "$2"
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
root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}
split_pane() { "$TAKO_BIN" split --pane "$1" --down 2>/dev/null | tr -dc '0-9'; }
now_ms() { python3 -I -c 'import time; print(int(time.time() * 1000))'; }
# 疑似の報告を 1 本送る（mod と同じ口 = `tako mod report` の stdin。送り主は TAKO_PANE_ID）
send_report() {
  printf '%s' "$2" | TAKO_PANE_ID="$1" "$TAKO_BIN" mod report >/dev/null 2>&1
}
# 報告の本体（アカウント / 5h の % と観測時刻 / 7d の % と観測時刻。7d の % が空なら 5h だけ）
report_json() {
  local cfg="$1" five="$2" five_at="$3" week="${4:-}" week_at="${5:-}"
  local limits="{\"kind\":\"five_hour\",\"percent_used\":${five},\"resets_at\":\"2030-01-01T00:00:00.000Z\",\"observed_at\":${five_at}}"
  if [ -n "$week" ]; then
    limits="${limits},{\"kind\":\"seven_day\",\"percent_used\":${week},\"resets_at\":\"2030-01-05T00:00:00.000Z\",\"observed_at\":${week_at}}"
  fi
  printf '{"schema":1,"mod_version":"t1903","claude_version":"2.1.294","session_id":"s-1903","at":1,"model":"claude-haiku-5-5","context":{"tokens":1000,"window":200000,"percent":1},"rate_limits":[%s],"turn":"idle","classic_events":true,"config_dir":"%s","ended":false}' \
    "$limits" "$cfg"
}
# 疑似の画面（alt screen に入って、claude の statusLine 風のフッターを画面の下端へ。#217 の読み口）
SCREEN_FILE="$TMP/screen.txt"
DISPLAY_SH="$TMP/display.sh"
cat > "$DISPLAY_SH" <<'EOF'
f="$1"
printf '\033[?1049h'
last=""
while :; do
  cur="$(cat "$f" 2>/dev/null)"
  if [ "$cur" != "$last" ]; then
    rows="$(stty size 2>/dev/null | awk '{print $1}')"
    [ -n "$rows" ] || rows=40
    n="$(printf '%s\n' "$cur" | wc -l | tr -d ' ')"
    printf '\033[2J\033[H'
    i=0
    while [ "$i" -lt $((rows - n - 1)) ]; do echo; i=$((i + 1)); done
    printf '%s' "$cur"
    last="$cur"
  fi
  sleep 0.3
done
EOF
printf '%s\n' "⏺ 実装を進めます" "" "────────────────────────────" "❯ " "────────────────────────────" \
  "  [Haiku 5.5]  5h 23% · 7d 46%" > "$SCREEN_FILE"
start_display() {
  "$TAKO_BIN" send --pane "$1" "clear; sh '$DISPLAY_SH' '$SCREEN_FILE'" >/dev/null 2>&1
}

# --- visual: 実ピクセル ----------------------------------------------------------
run_visual() {
  local log="$1" dump="$2"
  shift 2
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" TAKO_PERSIST=0 TAKO_DATA_DIR="$TMP/data-visual-$$-$RANDOM" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=mod-limits TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

phase_visual() {
  echo "== visual: visual-test mod-limits（実ピクセル。帯の絵は ${DUMP_NEW#"$TMP/"}）"
  local rc=0
  run_visual "$TMP/visual.log" "$DUMP_NEW" || rc=$?
  check_eq "節が終了コード 0 で終わる" "0" "$rc"
  grep 'TAKO_VISUAL_PIXEL: mod-limits\|TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | mask | sed 's/^/    観測: /'
  check_eq "TAKO_VISUAL_TEST_OK" "true" "$(grep -q TAKO_VISUAL_TEST_OK "$TMP/visual.log" && echo true || echo false)"
  check_eq "帯の絵を新旧 2 枚書き出した" "2" "$(ls "$DUMP_NEW"/mod-limits-*.png 2>/dev/null | wc -l | tr -d ' ')"

  echo "== visual: A/B（TAKO_1903_LEGACY=1。帯の絵は ${DUMP_OLD#"$TMP/"}）"
  rc=0
  run_visual "$TMP/visual-legacy.log" "$DUMP_OLD" TAKO_1903_LEGACY=1 || rc=$?
  check_eq "A/B: 節が非ゼロで終わる" "true" "$([ "$rc" -ne 0 ] && echo true || echo false)"
  grep 'TAKO_VISUAL_PIXEL: mod-limits\|TAKO_APP_SELF_TEST_FAILED' "$TMP/visual-legacy.log" | mask | sed 's/^/    観測: /'
  check_eq "A/B: 落ちた理由はステータスバーが mod の値でないこと（①）" "true" \
    "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test mod-limits ①' "$TMP/visual-legacy.log" && echo true || echo false)"
}

# --- fake: 報告を直に送る段 -----------------------------------------------------
phase_fake() {
  echo "== fake 1: ステータスバーは mod を先に見て、報告が古くなると画面へ戻る（TAKO_PERSIST=0）"
  start_app fake TAKO_PERSIST=0
  local p q r s t0 t1 waited
  p="$(root_pane)"
  start_display "$p"
  if wait_for 15 bar_is "23/46/screen"; then
    pass "報告の前: ステータスバーは画面の値（$(bar)）"
  else
    fail "報告の前に画面の値が出ない（$(bar)）"
  fi
  send_report "$p" "$(report_json /cfg-1903-a 42.4 "$(now_ms)" 17.5 "$(now_ms)")"
  t0="$(date +%s)"
  check_eq "1. 画面（23 / 46）と違う mod（42.4 / 17.5）→ ステータスバーは mod の値" "42/18/mod" "$(bar_line)"
  check_eq "1. self の rate_limits も同じ報告から（5h 42.4）" "42.4" "$(self_f "$p" rate_limits.windows.kind=five_hour.used_percent)"
  echo "    （報告を止めて、ステータスバーが画面へ戻るまで待つ）"
  if wait_for 70 bar_is "23/46/screen"; then
    t1="$(date +%s)"
    waited=$((t1 - t0))
    pass "1. 報告が古くなると画面の値へ戻る（${waited} 秒後・$(bar)）"
    check_eq "1. 戻ったのは失効（45 秒）の後" "true" "$([ "$waited" -ge 44 ] && echo true || echo false)"
  else
    fail "1. 報告が古くなっても画面へ戻らない（$(bar)）"
  fi
  check_eq "1. 古い報告の理由（mod_reason=mod_stale）" "mod_stale" "$(self_f "$p" mod_reason)"

  echo "== fake 2: 放置したペインと動いているペインが同じアカウント → 動いているペインの値"
  q="$(split_pane "$p")"
  local now old
  now="$(now_ms)"
  old=$((now - 3600000))
  # p: 1 時間前に 80% / 7d 30% を観測したきり放置（heartbeat で鮮度だけ保っている）
  send_report "$p" "$(report_json /cfg-1903-b 80 "$old" 30 "$old")"
  # q: 動いている。同じ窓で 20%（上限の引き上げ・早めのリセットで下がった = #1880 の束ね方が誤る形）
  send_report "$q" "$(report_json /cfg-1903-b 20 "$now")"
  "$TAKO_BIN" focus "$p" >/dev/null 2>&1
  check_eq "2. self（放置したペイン）: 5h は動いているペインの最新の観測" "20" "$(self_f "$p" rate_limits.windows.kind=five_hour.used_percent)"
  check_eq "2. self（放置したペイン）: 他に観測の無い 7d は放置したペインの値" "30" "$(self_f "$p" rate_limits.windows.kind=seven_day.used_percent)"
  check_eq "2. worker_status（動いているペイン）も同じ束ね方" "20" "$(status_f "$q" rate_limits.windows.kind=five_hour.used_percent)"
  check_eq "2. ステータスバー（フォーカスは放置したペイン）も動いているペインの値" "20/30/mod" "$(bar_line)"
  # 動いているペインの値が変われば追従する（最新の観測で束ねる）
  send_report "$q" "$(report_json /cfg-1903-b 25 "$(now_ms)")"
  check_eq "2. 動いているペインの観測が進めば追従（25）" "25/30/mod" "$(bar_line)"

  echo "== fake エッジ: mod の無いペインがフォーカス・アカウントが違うペインが混ざる"
  r="$(split_pane "$q")"
  s="$(split_pane "$r")"
  send_report "$r" "$(report_json /cfg-1903-c 77 "$(now_ms)" 61 "$(now_ms)")"
  "$TAKO_BIN" focus "$r" >/dev/null 2>&1
  check_eq "エッジ: 別アカウントのペインにフォーカス → そのアカウントの値（混ぜない）" "77/61/mod" "$(bar_line)"
  check_eq "エッジ: 別アカウントの self は自分のアカウントだけ" "77" "$(self_f "$r" rate_limits.windows.kind=five_hour.used_percent)"
  "$TAKO_BIN" focus "$s" >/dev/null 2>&1
  check_eq "エッジ: mod の無いペイン（素のシェル）がフォーカス → 次に報告のあるペイン（p）のアカウント" "25/30/mod" "$(bar_line)"
  check_eq "エッジ: mod の無いペインの self は rate_limits=null・理由が出る（$(self_f "$s" mod_reason)）" \
    "null/true" "$(self_f "$s" rate_limits)/$([ "$(self_f "$s" mod_reason)" != null ] && echo true || echo false)"
  stop_app

  echo "== fake A/B: TAKO_1903_LEGACY=1（#1880 の束ね方・ステータスバーは画面だけ）"
  start_app legacy TAKO_PERSIST=0 TAKO_1903_LEGACY=1
  p="$(root_pane)"
  start_display "$p"
  q="$(split_pane "$p")"
  now="$(now_ms)"
  old=$((now - 3600000))
  send_report "$p" "$(report_json /cfg-1903-b 80 "$old" 30 "$old")"
  send_report "$q" "$(report_json /cfg-1903-b 20 "$now")"
  "$TAKO_BIN" focus "$p" >/dev/null 2>&1
  check_eq "A/B: 束ね方は同じ窓の大きい方 = 放置したペインの古い 80%（#1903 前の誤り）" "80" "$(self_f "$p" rate_limits.windows.kind=five_hour.used_percent)"
  if wait_for 15 bar_is "23/46/screen"; then
    pass "A/B: ステータスバーは mod を見ず画面の値（$(bar)）"
  else
    fail "A/B: ステータスバーが画面の値でない（$(bar)）"
  fi
  stop_app
}

# --- claude: 実 claude ------------------------------------------------------------
claude_available() { command -v claude >/dev/null 2>&1; }
ask() { "$TAKO_BIN" send --pane "$1" --await-prompt "$2" >/dev/null 2>&1; }
reports_at_least() { [ "$(mod_row_f "$1" report.reports)" -ge "$2" ] 2>/dev/null; }
has_limits() {
  local v
  v="$(mod_row_f "$1" report.rate_limits.kind=five_hour.observed_at)"
  [ -n "$v" ] && [ "$v" != null ] && [ "$v" != absent ]
}

phase_claude() {
  echo "== claude: 実 claude（haiku）で observed_at が heartbeat をまたいで動かない"
  if ! claude_available; then
    unmeasured "claude が無いので実 claude の段を飛ばした"
    return
  fi
  start_app claude TAKO_PERSIST=0
  local p obs1 obs2 at1 at2 n1 pct
  p="$(root_pane)"
  "$TAKO_BIN" send --pane "$p" "cd '$WORKDIR' && claude --model haiku" >/dev/null 2>&1
  if ! wait_for 60 has_reporting "$p"; then
    fail "claude の起動で報告が届かない（$(mod_row_f "$p" state)）"
    stop_app
    return
  fi
  ask "$p" "Reply with exactly one word: ok"
  if ! wait_for 90 has_limits "$p"; then
    unmeasured "使用制限が報告に無い（API キー利用など）ので observed_at の段を飛ばした"
    ask "$p" "/exit"
    stop_app
    return
  fi
  obs1="$(mod_row_f "$p" report.rate_limits.kind=five_hour.observed_at)"
  at1="$(mod_row_f "$p" report.at)"
  n1="$(mod_row_f "$p" report.reports)"
  echo "    （5h の observed_at=${obs1} / at=${at1} / 報告 ${n1} 本。heartbeat を 2 回待つ）"
  if wait_for 60 reports_at_least "$p" $((n1 + 2)); then
    obs2="$(mod_row_f "$p" report.rate_limits.kind=five_hour.observed_at)"
    at2="$(mod_row_f "$p" report.at)"
    check_eq "heartbeat を 2 回またいでも 5h の observed_at は動かない（値が変わっていない）" "$obs1" "$obs2"
    check_eq "報告の at は進んでいる（heartbeat は届いている）" "true" "$([ "$at2" -gt "$at1" ] 2>/dev/null && echo true || echo false)"
  else
    fail "heartbeat が届かない（報告 $(mod_row_f "$p" report.reports) 本）"
  fi
  pct="$(mod_row_f "$p" report.rate_limits.kind=five_hour.percent_used)"
  check_eq "ステータスバーは mod の値（5h ${pct}% の四捨五入）・取得元 mod" \
    "$(python3 -I -c 'import sys; print(round(float(sys.argv[1])))' "$pct")/mod" "$(bar_f 5h)/$(bar_f source)"
  ask "$p" "/exit"
  stop_app
}
has_reporting() { [ "$(mod_row_f "$1" state)" = reporting ]; }

want visual && phase_visual
want fake && phase_fake
want claude && phase_claude

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
