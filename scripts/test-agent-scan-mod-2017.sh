#!/usr/bin/env bash
# test-agent-scan-mod-2017.sh — 監視の走査（`claude agents --json`）を mod の報告で置き換えた効果の実経路テスト（#2017）
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）を**一時の設定 dir** と**偽の Messages API**
# （scripts/lib/fake-messages-api.mjs）で、隔離 GUI（tako-vd・器あり = TAKO_PERSIST=1・自動リネーム off）の
# ペインに 4 本立てる。走査先は本番と同じ 4 本（既定 + 一時アカウント 3。既定 config dir も一時の HOME の下）で、
# claude は走査先ごとに 1 本。master の watch 相当に 4 ペインの `orchestrator status` と `orchestrator self` を
# 5 秒ごとに叩きながら、同じバイナリの新旧（旧 = TAKO_2017_LEGACY=1）で 60 秒ずつ測る。
# 番号は Issue #2017 の受け入れ条件:
#   measure 1. tako-app の回収済みの子の CPU（proc_pid_rusage v6 の差）と、`pgrep -lf -P <tako-app>` を
#              10 Hz × 60 秒で拾った `claude agents --json` の本数。新は旧の 1/10 以下
#           2. 判定が変わらない: 4 本とも新旧で status / 会話 ID の有無が同じ（idle の claude は idle）。
#              新は agents_scan.skipped=true（reason=mod_fresh）、旧は legacy_env
#   aging   2. 報告を止める（mod が叩く CLI の `mod report` だけを失敗させる）と 20 秒で走査へ落ち
#              （reason=mod_aging）、走査はその claude のアカウントと既定だけ。戻すと走査が止まる
#   nomod   2. mod の無い claude（`tako mod off` の後に立てたペイン）が混ざると、そのペインだけ走査へ落ち
#              （reason は注入しなかった理由）、走査はそのアカウントと既定だけ（4 本ではなく 2 本）
#
# **本物の設定 dir（~/.claude*）には触らない**: HOME を一時 dir へ差し替えて GUI を立てる（既定 config dir の
# 走査も一時の HOME の下を見る）。claude の設定 dir は mktemp 配下（認証は写さず、偽 API とダミーの API キーを
# 一時の .claude.json で承認済みにする）。本番 tako が配る env は最初に落とす。落とすのは自分で起こした pid と
# 自分の tmux ソケットだけ（pkill / killall は使わない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-agent-scan-mod-2017.sh   （ONLY=measure|aging|nomod。既定は全部）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は単体テスト
# （`dispatch::tests::issue2017_*` ほか）と番犬（issue2017_agent_scan_mod_watchdog / issue1984_ui_thread_offload_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_2017_LEGACY \
  TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
ONLY="${ONLY:-}"
SECS="${SECS:-60}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

command -v claude >/dev/null 2>&1 || { echo "未実測: claude が PATH に無い"; exit 4; }
command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 4; }

TMP="$(mktemp -d /tmp/tako-2017-XXXXXX)"
APP_PID=""
FAKE_PID=""
LOAD_PID=""
TMUX_SOCKET="tk2017-$$"
REAL_HOME="$HOME"
MASK_USER="$(id -un)"
mask() { sed -e "s#${REAL_HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<tmp>#g"; }

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

cleanup() {
  [ -n "$LOAD_PID" ] && kill "$LOAD_PID" 2>/dev/null
  stop_isolated_gui "$APP_PID"
  tmux_socket_kill "$TMUX_SOCKET"
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
# claude は作業フォルダを実パス（/tmp → /private/tmp）で見るので、信頼もその形で書く
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
API_KEY="sk-ant-api03-fake2017fake2017fake2017fake2017fake2017fake2017AA"
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
# 走査先 4 = 既定（一時の HOME の下）+ 明示 3（本番と同じ本数）
CFG_DEFAULT="$TMP/home/.claude"
CFG_A="$TMP/acct-a"
CFG_B="$TMP/acct-b"
CFG_C="$TMP/acct-c"
for d in "$CFG_DEFAULT" "$CFG_A" "$CFG_B" "$CFG_C"; do make_config "$d"; done

# mod が叩く CLI（TAKO_CLI）の薄い包み: `$PAUSE_DIR/<ペイン ID>` がある間だけ `mod report` を失敗させる
# （= そのペインの報告が途切れた状態。claude は生かしたまま。`kill -STOP` は claude がすぐ起き直す = #1962）
PAUSE_DIR="$TMP/pause"
mkdir -p "$PAUSE_DIR"
CLI_GATE="$TMP/tako-cli-gate"
cat > "$CLI_GATE" <<EOF
#!/bin/sh
if [ -e "$PAUSE_DIR/\${TAKO_PANE_ID:-none}" ] && [ "\$1" = mod ] && [ "\$2" = report ]; then
  exit 1
fi
exec '$TAKO_BIN' "\$@"
EOF
chmod +x "$CLI_GATE"

# ログインシェルの包み（GUI の `$SHELL`）: `agents --json` を含む起動だけを数えて本物の zsh へ渡す
# （一時の HOME では rc が無く zsh がすぐ exec するので、10 Hz の標本では zsh の行を拾えない）。
# 名前は zsh のまま（ペインのシェル統合が名前でシェルを見分ける）
SHELL_LOG="$TMP/login-scans.log"
mkdir -p "$TMP/shellbin"
cat > "$TMP/shellbin/zsh" <<WRAP
#!/bin/sh
case "\$*" in *"agents --json"*) echo scan >> '$SHELL_LOG' ;; esac
exec /bin/zsh "\$@"
WRAP
chmod +x "$TMP/shellbin/zsh"

# claude の包み（GUI の PATH の先頭）: `agents --json` の起動を 1 行ずつ記録して本物へ exec する。
# 直接起動の走査は 0.15 秒で終わり、10 Hz の標本では取りこぼしうるので、本数と走査先はこちらで数える
# （記録は設定 dir の名前だけ = default / acct-a / acct-b / acct-c）
REAL_CLAUDE="$(command -v claude)"
CLAUDE_LOG="$TMP/claude-scans.log"
mkdir -p "$TMP/claudebin"
cat > "$TMP/claudebin/claude" <<WRAP
#!/bin/sh
case "\$*" in *"agents --json"*) basename "\${CLAUDE_CONFIG_DIR:-default}" >> '$CLAUDE_LOG' ;; esac
exec '$REAL_CLAUDE' "\$@"
WRAP
chmod +x "$TMP/claudebin/claude"
# 窓の間に起こした走査の本数と走査先（`本数 名前=本数…`）
scan_log_reset() { : > "$CLAUDE_LOG"; }
scan_log_summary() {
  local n
  n="$(wc -l < "$CLAUDE_LOG" | tr -d ' ')"
  printf '%s %s' "$n" "$(sort "$CLAUDE_LOG" | uniq -c | awk '{printf "%s=%s,", $2, $1}')"
}

# --- 計測の道具（python は標準ライブラリだけ）------------------------------------------
cat > "$TMP/ru.py" <<'PY'
# proc_pid_rusage v6: 回収済みの子の user + system（秒）
import ctypes, sys
class RU(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16), ("f", ctypes.c_uint64 * 47), ("r", ctypes.c_uint64 * 9)]
class TB(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
tb = TB(); lib.mach_timebase_info(ctypes.byref(tb))
k = tb.numer / tb.denom / 1e9
ru = RU()
if lib.proc_pid_rusage(int(sys.argv[1]), 6, ctypes.byref(ru)) != 0:
    sys.exit(1)
f = ru.f
print(f"{(f[10] + f[11]) * k:.4f} {(f[0] + f[1]) * k:.4f}")
PY
cat > "$TMP/scan.py" <<'PY'
# tako-app の直下の子を 10 Hz で読み、`claude agents --json` の一意な pid と起動のまとまり（1 秒以内）を数える
import subprocess, sys, time
pid, dur = sys.argv[1], float(sys.argv[2])
starts = {}
t0 = time.time()
while time.time() - t0 < dur:
    out = subprocess.run(["pgrep", "-lf", "-P", pid], capture_output=True, text=True).stdout
    for line in out.splitlines():
        p, _, cmd = line.partition(" ")
        if "agents --json" in cmd and p not in starts:
            starts[p] = (time.time() - t0, " -l " in cmd or cmd.split()[0].endswith("sh"))
    time.sleep(0.1)
ts = sorted(t for t, _ in starts.values())
bursts = [t for i, t in enumerate(ts) if i == 0 or t - ts[i - 1] > 1.0]
sizes = []
for b in bursts:
    sizes.append(sum(1 for t in ts if b <= t < b + 1.0))
login = sum(1 for _, l in starts.values() if l)
print(f"{len(starts)} {len(bursts)} {max(sizes) if sizes else 0} {login}")
PY

tk() { "$TAKO_BIN" "$@"; }
# 腕ごとの隔離（data / sessions / workers は腕ごと = 前の腕のペインを復元しない。アカウントは共有）
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

start_app() {
  local name="$1"
  shift
  arm_env "$name"
  launch_isolated_gui "$TMP/app-${name}.log" TAKO_PERSIST=1 TAKO_FLOW_DIAG=1 HOME="$TMP/home" \
    SHELL="$TMP/shellbin/zsh" PATH="$TMP/claudebin:$PATH" DISABLE_AUTOUPDATER=1 ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  # 自動リネームは GUI の env の claude を呼ぶ（測定に混ぜない）
  tk autorename off >/dev/null 2>&1
}
stop_app() {
  [ -n "$LOAD_PID" ] && kill "$LOAD_PID" 2>/dev/null
  LOAD_PID=""
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  # 器ごと claude を落とす（次の腕は新しい claude で同じ条件にする）
  tmux_socket_kill "$TMUX_SOCKET"
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
# p を分割して新しいペインの ID を出す（`tako split` は新ペイン ID を出力する）
split_new() { tk split --pane "$1" "--$2" 2>/dev/null | tr -dc '0-9'; }
mod_state() { tk mod --json 2>/dev/null | jq -r --argjson p "$1" '.panes[] | select(.pane == $p) | .state' 2>/dev/null; }
reporting() { [ "$(mod_state "$1")" = reporting ]; }
status_json() { tk orchestrator status --pane "$1" 2>/dev/null; }
scan_reason() { status_json "$1" | jq -r '.agents_scan.reason // "null"'; }
reason_is() { [ "$(scan_reason "$1")" = "$2" ]; }
has_sid() { [ "$(status_json "$1" | jq -r '.resolved_session_id != null')" = true ]; }
# claude をペインに立てる（設定 dir と API は一時のもの。TAKO_CLI は包み = 報告を止められる）。
# 3 つ目に nomod を渡すと TAKO_CLI を外す = `tako mod off` のペインと同じ（mod は TAKO_CLI が無ければ
# 休眠する。外さないと、設定 dir の skills/tako = #1959 の写しから読まれた mod が報告してしまう）
start_claude() {
  local cli="TAKO_CLI='$CLI_GATE'"
  [ "${3:-}" = nomod ] && cli="-u TAKO_CLI"
  tk send --pane "$1" "cd '$WORK' && clear && env $cli CLAUDE_CONFIG_DIR='$2' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 claude --model haiku" >/dev/null 2>&1
}
# master の watch 相当（4 ペインの status + self を 5 秒ごと）
start_load() {
  (
    while :; do
      for p in "$@"; do tk orchestrator status --pane "$p" >/dev/null 2>&1; done
      tk orchestrator self --pane "$1" >/dev/null 2>&1
      sleep 5
    done
  ) &
  LOAD_PID=$!
}
# 60 秒の窓: 回収済みの子の CPU・走査の本数（一意な pid / まとまり / 1 まとまりの最大本数 / ログインシェル）
measure_window() {
  local before after
  : > "$SHELL_LOG"
  scan_log_reset
  before="$(python3 -I "$TMP/ru.py" "$APP_PID")"
  WINDOW="$(python3 -I "$TMP/scan.py" "$APP_PID" "$SECS")"
  after="$(python3 -I "$TMP/ru.py" "$APP_PID")"
  LOGIN_SCANS="$(wc -l < "$SHELL_LOG" | tr -d ' ')"
  LAUNCHED="$(scan_log_summary)"
  CHILD_CPU="$(python3 -I -c 'import sys; a=sys.argv[1].split(); b=sys.argv[2].split(); print(f"{float(b[0])-float(a[0]):.2f} {float(b[1])-float(a[1]):.2f}")' "$before" "$after")"
}

# 4 ペインに 4 アカウントの claude を立て、mod の報告が届くまで待つ（PANES に ID を入れる）
claude_known() { [ -n "$(tk mod --json 2>/dev/null | jq -r '.claude_version // empty')" ]; }
setup_four() {
  local root p1 p2 p3 p4
  # 注入は claude の版が分かってから作ったペインだけ（PATH の先頭の包みはパスから版を読めないので、
  # 起動の直後に作られるルートペインは注入されないことがある）。ルートはただのシェルのまま残す
  wait_for 30 claude_known || { fail "claude の版が分からない（mod を注入できない）"; return 1; }
  root="$(pane_ids | head -1)"
  p1="$(split_new "$root" right)"
  p2="$(split_new "$p1" right)"
  p3="$(split_new "$p1" down)"
  p4="$(split_new "$p2" down)"
  PANES="$p1 $p2 $p3 $p4"
  start_claude "$p1" "$CFG_DEFAULT"
  start_claude "$p2" "$CFG_A"
  start_claude "$p3" "$CFG_B"
  start_claude "$p4" "$CFG_C"
  for p in $PANES; do
    wait_for 60 reporting "$p" || { fail "pane $p の mod の報告が届かない（state=$(mod_state "$p")）"; return 1; }
  done
  return 0
}
judge_line() {
  local p out=""
  for p in $PANES; do
    out="$out $(status_json "$p" | jq -r '"\(.status)/\(.resolved_session_id != null)"')"
  done
  printf '%s' "${out# }"
}

# --- measure: 新旧で同じ条件の 60 秒 ------------------------------------------------------
if want measure; then
  echo "== measure: 4 アカウント × mod 入りの claude 4 本 + master の監視相当（${SECS} 秒ずつ）"
  for arm in legacy new; do
    case "$arm" in
      legacy) start_app "$arm" TAKO_2017_LEGACY=1 ;;
      new) start_app "$arm" ;;
    esac
    setup_four || { stop_app; continue; }
    # 監視の負荷を掛けてから窓を開く（最初の走査の山を窓に入れない）
    # shellcheck disable=SC2086
    start_load $PANES
    sleep 6
    measure_window
    eval "WIN_$arm=\"\$WINDOW\"; CPU_$arm=\"\$CHILD_CPU\"; LOGIN_$arm=\"\$LOGIN_SCANS\"; LAUNCH_$arm=\"\$LAUNCHED\""
    eval "JUDGE_$arm=\"\$(judge_line)\""
    eval "SCAN_$arm=\"\$(status_json "${PANES%% *}" | jq -c '.agents_scan')\""
    echo "  $arm: 起こした走査 ${LAUNCHED}（pgrep の標本: 一意な pid / まとまり / 1 まとまりの最大 = ${WINDOW% *}）・ログインシェル経由 ${LOGIN_SCANS} 本・回収済みの子の CPU / 本体の CPU（秒 / ${SECS} 秒）= $CHILD_CPU"
    stop_app
  done
  # shellcheck disable=SC2154
  if [ -n "${WIN_legacy:-}" ] && [ -n "${WIN_new:-}" ]; then
    set -- $LAUNCH_legacy; L_SCANS=$1
    set -- $LAUNCH_new; N_SCANS=$1
    set -- $CPU_legacy; L_CPU=$1
    set -- $CPU_new; N_CPU=$1
    echo "  旧 ${L_SCANS} 本 / 新 ${N_SCANS} 本・子の CPU 旧 ${L_CPU} 秒 / 新 ${N_CPU} 秒"
    if [ "$L_SCANS" -gt 0 ]; then pass "旧（TAKO_2017_LEGACY=1）は監視で走査する（${L_SCANS} 本 / ${SECS} 秒）"; else fail "旧で走査が 0 本（負荷が掛かっていない）"; fi
    if [ $((N_SCANS * 10)) -le "$L_SCANS" ]; then pass "新の走査は旧の 1/10 以下（${N_SCANS} ≦ ${L_SCANS} / 10）"; else fail "新の走査が旧の 1/10 を超える（${N_SCANS} / ${L_SCANS}）"; fi
    if python3 -I -c 'import sys; sys.exit(0 if float(sys.argv[2]) * 10 <= float(sys.argv[1]) else 1)' "$L_CPU" "$N_CPU"; then
      pass "回収済みの子の CPU は旧の 1/10 以下（${N_CPU} ≦ ${L_CPU} / 10）"
    else
      fail "回収済みの子の CPU が旧の 1/10 を超える（${N_CPU} / ${L_CPU}）"
    fi
    if [ "${LOGIN_legacy:-0}" -gt 0 ]; then pass "旧の走査はログインシェル経由（${LOGIN_legacy} 本）"; else fail "旧でログインシェルの走査が数えられない（包みが効いていない）"; fi
    check_eq "新はログインシェルを経由しない" 0 "${LOGIN_new:-x}"
    check_eq "判定が新旧で同じ（status / 会話 ID の有無。4 ペイン）" "$JUDGE_legacy" "$JUDGE_new"
    check_eq "新の agents_scan" '{"reason":"mod_fresh","skipped":true}' "$(printf '%s' "$SCAN_new" | jq -cS .)"
    check_eq "旧の agents_scan" '{"reason":"legacy_env","skipped":false}' "$(printf '%s' "$SCAN_legacy" | jq -cS .)"
  fi
fi

# --- aging / nomod（新のみ）-----------------------------------------------------------
if want aging || want nomod; then
  start_app edge
  if setup_four; then
    set -- $PANES
    P_DEFAULT=$1
    P_B=$3
    # shellcheck disable=SC2086
    start_load $PANES
    if want aging; then
      echo "== aging: acct-b の claude の報告を止める → 20 秒で走査へ落ち、戻すと止まる"
      : > "$PAUSE_DIR/$P_B"
      if wait_for 40 reason_is "$P_B" mod_aging; then pass "報告が止まると reason=mod_aging で走査へ落ちる"; else fail "mod_aging にならない（$(scan_reason "$P_B")）"; fi
      check_eq "報告が続いているペインは走査しない" mod_fresh "$(scan_reason "$P_DEFAULT")"
      scan_log_reset
      sleep 12
      set -- $(scan_log_summary)
      case "${2:-}" in
        acct-b=*,default=*, | default=*,acct-b=*,) pass "走査は止まった claude のアカウント + 既定だけ（12 秒で ${1} 本: ${2}）" ;;
        *) fail "走査先がおかしい（12 秒で ${1} 本: ${2:-なし}。期待 = acct-b と default だけ）" ;;
      esac
      rm -f "${PAUSE_DIR:?}/${P_B:?}"
      if wait_for 30 reason_is "$P_B" mod_fresh; then pass "報告が戻ると走査しない（mod_fresh）"; else fail "mod_fresh へ戻らない（$(scan_reason "$P_B")）"; fi
      sleep 6
      scan_log_reset
      sleep 12
      set -- $(scan_log_summary)
      check_eq "報告が戻った後の 12 秒に起こした走査" 0 "$1"
    fi
    if want nomod; then
      echo "== nomod: mod の無い claude（tako mod off の後に立てたペイン）が混ざる"
      tk mod off >/dev/null 2>&1
      P_NOMOD="$(split_new "$P_DEFAULT" right)"
      tk mod on >/dev/null 2>&1
      start_claude "$P_NOMOD" "$CFG_C" nomod
      kill "$LOAD_PID" 2>/dev/null
      # shellcheck disable=SC2086
      start_load $PANES $P_NOMOD
      # 5 本目の claude の起動を待つ（走査の答えは鮮度 5 秒なので、起動の直後は前の答えが返りうる）
      if ! wait_for 40 has_sid "$P_NOMOD"; then
        echo "  （pane $P_NOMOD の画面の末尾）"
        tk read --pane "$P_NOMOD" 2>/dev/null | tail -6 | mask
      fi
      reason="$(scan_reason "$P_NOMOD")"
      case "$reason" in
        mod_fresh | null) fail "mod の無い claude が走査へ落ちない（reason=${reason}）" ;;
        *) pass "mod の無い claude は走査へ落ちる（reason=${reason}）" ;;
      esac
      check_eq "mod の無い claude の会話 ID は走査で解ける" true "$(status_json "$P_NOMOD" | jq -r '.resolved_session_id != null')"
      check_eq "mod の有る claude は走査しないまま" mod_fresh "$(scan_reason "$P_DEFAULT")"
      scan_log_reset
      sleep 12
      set -- $(scan_log_summary)
      case "${2:-}" in
        acct-c=*,default=*, | default=*,acct-c=*,) pass "走査は mod の無い claude のアカウント + 既定だけ（12 秒で ${1} 本: ${2}。旧は 4 走査先ずつ）" ;;
        *) fail "走査先がおかしい（12 秒で ${1} 本: ${2:-なし}。期待 = acct-c と default だけ）" ;;
      esac
      skipped="$(grep -c '前段ガードで 2/4 件の Node 起動を省いた' "$TAKO_DATA_DIR/perf.log" 2>/dev/null || echo 0)"
      if [ "${skipped:-0}" -ge 1 ]; then pass "perf.log に絞った走査（4 本中 2 本を省いた）が ${skipped} 回"; else unmeasured "perf.log に絞った走査の行が無い（TAKO_FLOW_DIAG の置き場を確認）"; fi
    fi
  fi
  stop_app
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
