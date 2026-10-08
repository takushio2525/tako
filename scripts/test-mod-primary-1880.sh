#!/usr/bin/env bash
# test-mod-primary-1880.sh — tako mod S2（#1880）: mod の報告を一次ソースにする配線の実経路テスト
#
# 隔離した data / discovery / tmux で**実 tako-app** を立て、次を実測する（番号は Issue #1880 の受け入れ条件）:
#   1. 4 経路（`orchestrator self` / `worker_status` / #749 の自動ハンドオフの tick / チャットヘッダの
#      残量バー）が `ctx_source: "mod"` を返す
#   2. statusLine を外した claude（画面に ctx が出ない構成）でも ctx% が取れる
#   3. 報告を止める注入で 45 秒後に画面 / transcript へ落ち、理由（ctx_mod_reason）が出る
#   4. 使用制限の reset 時刻が mod の値（秒精度）になる。mod の値だけでは停止と判定しない
#   5. A/B: `TAKO_1877_S2_LEGACY=1` で mod を見ない旧挙動
#   6. classic_events=false の報告で、画面の権限ダイアログとの食い違いが warnings に出る
#   エッジ: 画面と mod の ctx の食い違い（warnings）/ `tako mod off` の理由 / 版の下限未満の理由 /
#          tako の再起動をまたいで届く報告
#
# 段は 2 種類:
#   fake   — 報告を `tako mod report` で直に送る（claude 不要・決定的）。1 / 3 / 4 / 5 / 6 / エッジ
#   claude — 実 claude（haiku）を statusLine なしで動かす。1 / 2 / 3（claude を SIGSTOP して止める）
#
# **本番の tako / 本番の claude 設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 落とすのは自分で起こした pid だけ（pkill / killall は使わない）。claude は利用者のログインシェルの
# 設定 dir を**読むだけ**で使う（認証が要るため）。statusLine は `--settings` でこのセッションだけ外す
# （設定ファイルは書かない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 選んで回す: ONLY=fake|claude（既定は両方）。**CI には登録しない**（実 GUI と実 claude の認証が要る）。
# CI 側の担保は単体テストと番犬（`crates/tako-control/tests/issue1880_mod_primary_watchdog.rs`）。
#
# 使い方: bash scripts/test-mod-primary-1880.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY
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
contains() {
  case "$1" in
    *"$2"*) echo true ;;
    *) echo false ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1880-XXXXXX)"
APP_PID=""
CLAUDE_PID=""
TMUX_SOCKET="tako-1880-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }

CCD="$("${SHELL:-/bin/zsh}" -l -c 'printf %s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"' 2>/dev/null)"
[ -n "$CCD" ] || CCD="$HOME/.claude"
INLINE_DATA="$CCD/plugins/data/tako-inline"
INLINE_EXISTED=0
[ -d "$INLINE_DATA" ] && INLINE_EXISTED=1
WORKDIR="${CLAUDE_WORKDIR:-$(git -C "$REPO_ROOT" worktree list --porcelain | awk 'NR == 1 { print $2 }')}"

cleanup() {
  # 止めた claude は戻してから落とす（SIGSTOP のまま残さない）
  if [ -n "$CLAUDE_PID" ]; then kill -CONT "$CLAUDE_PID" >/dev/null 2>&1 || true; fi
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
tako_exit_trap cleanup "tako mod S2 の実経路テスト"

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
# JSON の 1 項目（ドット区切り。数字はリストの添字・`pane=N` はペインの行）
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
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
self_f() { "$TAKO_BIN" orchestrator self --pane "$1" 2>/dev/null | jf "$2"; }
status_f() { "$TAKO_BIN" orchestrator status --pane "$1" 2>/dev/null | jf "$2"; }
# `tako read` の CLI は本文だけを出すので、input_status / mod_turn は MCP の tako_read_pane で読む
read_f() { mcp_call tako_read_pane "{\"pane\":$1}" 2>/dev/null | jf "$2"; }
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1880","version":"0"}}}' \
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
mod_f() { "$TAKO_BIN" mod --json 2>/dev/null | jf "$1"; }
lr_f() { "$TAKO_BIN" limit-resume --pane "$1" 2>/dev/null | jf "$2"; }
ui_f() { "$TAKO_BIN" ui-mode 2>/dev/null | jf "$1"; }
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}
is_eq() { [ "$("$1" "$2" "$3")" = "$4" ]; }
# 1 引数の取得関数（mod_f / ui_f）用
is_eq1() { [ "$("$1" "$2")" = "$3" ]; }
root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}
pane_state() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
want = int(sys.argv[1])
d = json.load(sys.stdin)
for tab in d.get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == want:
            print(p.get("state") or "absent"); sys.exit()
print("absent")' "$1"
}
shell_idle() { [ "$(pane_state "$1")" = idle ]; }
# 疑似の報告を 1 本送る（mod と同じ口 = `tako mod report` の stdin。送り主は TAKO_PANE_ID）
send_report() {
  local pane="$1" body="$2"
  printf '%s' "$body" | TAKO_PANE_ID="$pane" "$TAKO_BIN" mod report >/dev/null 2>&1
}
# 報告の本体（turn / ctx% / 窓 / classic_events / 使用制限の JSON 配列）
report_json() {
  local turn="$1" pct="$2" window="$3" classic="$4" limits="$5" tool="${6:-}"
  local pending=null
  [ -n "$tool" ] && pending="\"$tool\""
  printf '{"schema":1,"mod_version":"t1880","claude_version":"2.1.294","session_id":"s-1880","at":1,"model":"claude-haiku-5-5","effort":"medium","context":{"tokens":%s,"window":%s,"percent":%s},"rate_limits":%s,"turn":"%s","pending_tool":%s,"classic_events":%s,"config_dir":"/cfg-1880","ended":false}' \
    "$((pct * window / 100))" "$window" "$pct" "$limits" "$turn" "$pending" "$classic"
}
# 疑似の画面: ペインで「画面ファイルが変わったら描き直す」ループを 1 本動かし、ファイルを差し替える
# （中身は画面の**下端**へ寄せる = claude の TUI のフッターと同じ位置。末尾 8 行の走査に入る）
SCREEN_FILE="$TMP/screen.txt"
DISPLAY_SH="$TMP/display.sh"
cat > "$DISPLAY_SH" <<'EOF'
f="$1"
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
start_display() {
  /bin/cp -f "$2" "$SCREEN_FILE"
  "$TAKO_BIN" send --pane "$1" "clear; sh '$DISPLAY_SH' '$SCREEN_FILE'" >/dev/null 2>&1
  sleep 2
}
show_screen() {
  /bin/cp -f "$2" "$SCREEN_FILE.tmp" && /bin/mv -f "$SCREEN_FILE.tmp" "$SCREEN_FILE"
  sleep 1.5
}

# 疑似の画面（statusLine 風のフッター / 上限の停止画面 / 権限ダイアログ）
FOOTER41="$TMP/footer41.txt"
printf '%s\n' "⏺ 実装を進めます" "" "────────────────────────────" "❯ " "────────────────────────────" "  [Haiku 5.5 · MAX]  worker" "  ctx  41% ████░░░░░░" > "$FOOTER41"
LIMIT_SCREEN="$TMP/limit.txt"
printf '%s\n' "⏺ 実装を進めます" "  ⎿  Claude usage limit reached. Your limit will reset at 3am." "" \
  "╭──────────────────────────────────────────────────────────────────────╮" \
  "│ >                                                                    │" \
  "╰──────────────────────────────────────────────────────────────────────╯" > "$LIMIT_SCREEN"
PERM_SCREEN="$TMP/perm.txt"
printf '%s\n' "⏺ Running 1 shell command…" "────────────────────────────────────────────────" " Bash command" \
  "   echo tako1880" " Do you want to proceed?" " ❯ 1. Yes" "   2. Yes, and don't ask again for echo commands" \
  "   3. No, and tell Claude what to do differently (esc)" " Esc to cancel · Tab to amend · ctrl+e to explain" > "$PERM_SCREEN"
IDLE_SCREEN="$TMP/idle.txt"
printf '%s\n' "⏺ 完了しました" "" "────────────────────────────" "❯ " "────────────────────────────" > "$IDLE_SCREEN"

# 秒まで刻んだ解除時刻（画面の `resets at 3am` では出せない粒度 = 受け入れ条件 4）
RESET_ISO="$(python3 -I -c 'import datetime as d; t=d.datetime.now(d.timezone.utc)+d.timedelta(hours=2); print(t.replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:07.000Z"))')"
RESET_UNIX="$(python3 -I -c 'import datetime as d, sys; print(int(d.datetime.strptime(sys.argv[1], "%Y-%m-%dT%H:%M:%S.000Z").replace(tzinfo=d.timezone.utc).timestamp()))' "$RESET_ISO")"
LIMITS_FULL="[{\"kind\":\"five_hour\",\"percent_used\":100,\"resets_at\":\"$RESET_ISO\",\"observed_at\":1},{\"kind\":\"seven_day\",\"percent_used\":40,\"resets_at\":\"2030-01-01T00:00:00.000Z\",\"observed_at\":1}]"
LIMITS_OK="[{\"kind\":\"five_hour\",\"percent_used\":12,\"resets_at\":\"$RESET_ISO\",\"observed_at\":1}]"

# --- fake: 報告を直に送る段 -----------------------------------------------------
phase_fake() {
  echo "== fake: 疑似の報告で 4 経路のうち 3 経路・落ち方・使用制限・A/B（TAKO_PERSIST=0）"
  start_app fake TAKO_PERSIST=0
  local p
  p="$(root_pane)"
  # #749 の tick の対象にする（master の role。プロファイルは無くても既定で動く）
  "$TAKO_BIN" title --pane "$p" --role "orchestrator-master:t1880" >/dev/null 2>&1
  start_display "$p" "$IDLE_SCREEN"

  # 1. 新鮮な報告 → self / worker_status / tick / read が mod を採る
  send_report "$p" "$(report_json busy 37 200000 true "$LIMITS_OK")"
  check_eq "self: ctx_source=mod" "mod" "$(self_f "$p" ctx_source)"
  check_eq "self: ctx_percent は mod の値" "37" "$(self_f "$p" ctx_percent)"
  check_eq "self: 窓は claude が答えた値（200000）" "200000" "$(self_f "$p" ctx_window)"
  check_eq "self: ctx_mod_reason=null" "null" "$(self_f "$p" ctx_mod_reason)"
  check_eq "worker_status: ctx_source=mod" "mod" "$(status_f "$p" ctx_source)"
  check_eq "worker_status: status_source=mod" "mod" "$(status_f "$p" status_source)"
  check_eq "worker_status: 画面は入力欄でも mod の busy が先" "busy" "$(status_f "$p" status)"
  check_eq "worker_status: mod_turn.effort" "medium" "$(status_f "$p" mod_turn.effort)"
  check_eq "read: mod_turn.turn=busy" "busy" "$(read_f "$p" mod_turn.turn)"
  if wait_for 10 is_eq self_f "$p" auto_handoff_tick.ctx_source mod; then
    pass "#749 の tick: ctx_source=mod（$(self_f "$p" auto_handoff_tick)）"
  else
    fail "#749 の tick が mod を採らない（$(self_f "$p" auto_handoff_tick)）"
  fi
  check_eq "rate_limits: 解除時刻は mod の値（秒精度）" "$RESET_UNIX" "$(self_f "$p" rate_limits.windows.0.resets_at)"
  check_eq "rate_limits: worker_status にも同じ値" "$RESET_UNIX" "$(status_f "$p" rate_limits.windows.0.resets_at)"

  # エッジ: 画面と mod の ctx が食い違う（画面 41% / mod 37%）→ mod を採って warnings に差
  show_screen "$p" "$FOOTER41"
  send_report "$p" "$(report_json idle 37 200000 true "$LIMITS_OK")"
  check_eq "食い違い: mod の値を採る" "37" "$(self_f "$p" ctx_percent)"
  check_eq "食い違い: ctx_screen_delta = 画面 − mod" "4" "$(self_f "$p" ctx_screen_delta)"
  check_eq "食い違い: self の warnings に出る" "true" "$(contains "$(self_f "$p" warnings)" "mod の値 37%")"
  check_eq "食い違い: worker_status の warnings にも出る" "true" "$(contains "$(status_f "$p" warnings)" "mod の値 37%")"

  # 3. 報告を止める → 45 秒は mod のまま・過ぎたら画面へ落ちて理由が出る
  echo "    （報告を止めて 46 秒待つ）"
  sleep 30
  check_eq "30 秒後はまだ mod（鮮度 45 秒）" "mod" "$(self_f "$p" ctx_source)"
  sleep 16
  check_eq "46 秒後: self は画面へ落ちる" "screen" "$(self_f "$p" ctx_source)"
  check_eq "46 秒後: self の ctx_percent は画面の値" "41" "$(self_f "$p" ctx_percent)"
  check_eq "46 秒後: ctx_mod_reason=mod_stale" "mod_stale" "$(self_f "$p" ctx_mod_reason)"
  check_eq "46 秒後: worker_status の mod_reason=mod_stale" "mod_stale" "$(status_f "$p" mod_reason)"
  check_eq "46 秒後: worker_status は mod を状態の根拠にしない" "true" "$( [ "$(status_f "$p" status_source)" != mod ] && echo true || echo false)"
  if wait_for 10 is_eq self_f "$p" auto_handoff_tick.ctx_source screen; then
    pass "46 秒後: #749 の tick も画面へ落ちる（理由 $(self_f "$p" auto_handoff_tick.ctx_mod_reason)）"
  else
    fail "46 秒後: #749 の tick が画面へ落ちない（$(self_f "$p" auto_handoff_tick)）"
  fi
  check_eq "tako mod の行も stale" "stale" "$("$TAKO_BIN" mod --json 2>/dev/null | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(next((r["state"] for r in d["panes"] if r["pane"]==int(sys.argv[1])), "absent"))' "$p")"

  # 6. classic_events=false で画面に権限ダイアログ・mod は busy → 画面を採って warnings
  show_screen "$p" "$PERM_SCREEN"
  send_report "$p" "$(report_json busy 37 200000 false "$LIMITS_OK")"
  check_eq "classic_events=false: 画面のダイアログで waiting" "waiting" "$(status_f "$p" status)"
  check_eq "classic_events=false: 食い違いが warnings に出る" "true" "$(contains "$(status_f "$p" warnings)" "classic_events=false")"
  # 逆向き: mod は permission・画面は入力欄 → read の input_status は null（入力欄は無い）
  show_screen "$p" "$IDLE_SCREEN"
  send_report "$p" "$(report_json permission 37 200000 true "$LIMITS_OK" Bash)"
  check_eq "mod=permission: worker_status は waiting" "waiting" "$(status_f "$p" status)"
  check_eq "mod=permission: 画面にダイアログが無いことが warnings に出る" "true" "$(contains "$(status_f "$p" warnings)" "画面に選択肢ダイアログが見えない")"
  check_eq "mod=permission: read の input_status は null" "null" "$(read_f "$p" input_status)"
  check_eq "mod=permission: read の mod_turn" "permission" "$(read_f "$p" mod_turn.turn)"
  # mod が idle へ戻れば入力欄は読める（null にしたのは mod が待っていた間だけ）
  send_report "$p" "$(report_json idle 37 200000 true "$LIMITS_OK")"
  check_eq "mod=idle: read の input_status は戻る" "true" "$( [ "$(read_f "$p" input_status)" != null ] && echo true || echo false)"

  # 4. 使用制限の解除時刻は mod の値（秒精度）。mod の値だけでは停止と判定しない
  "$TAKO_BIN" limit-resume on --pane "$p" >/dev/null 2>&1
  show_screen "$p" "$IDLE_SCREEN"
  send_report "$p" "$(report_json idle 37 200000 true "$LIMITS_FULL")"
  sleep 5
  check_eq "100% の報告でも画面が上限でなければ停止ではない" "null" "$(lr_f "$p" state)"
  send_report "$p" "$(report_json idle 37 200000 true "$LIMITS_FULL")"
  show_screen "$p" "$LIMIT_SCREEN"
  if wait_for 15 is_eq lr_f "$p" state.stopped_by_limit true; then
    check_eq "上限の停止の解除時刻は mod の値（${RESET_ISO}）" "$RESET_UNIX" "$(lr_f "$p" state.reset_at)"
  else
    fail "上限の画面で停止を検知しない（$(lr_f "$p" state)）"
  fi
  "$TAKO_BIN" limit-resume off --pane "$p" >/dev/null 2>&1

  # エッジ: tako mod off で作ったペインは理由 disabled
  "$TAKO_BIN" mod off >/dev/null 2>&1
  local q
  q="$("$TAKO_BIN" split --pane "$p" --down 2>/dev/null | tr -dc '0-9')"
  sleep 1
  check_eq "mod off のペイン: ctx_mod_reason=disabled" "disabled" "$(self_f "$q" ctx_mod_reason)"
  "$TAKO_BIN" mod on >/dev/null 2>&1
  stop_app

  # 5. A/B: TAKO_1877_S2_LEGACY=1 では報告があっても見ない
  echo "== fake: A/B（TAKO_1877_S2_LEGACY=1）"
  start_app ab TAKO_PERSIST=0 TAKO_1877_S2_LEGACY=1
  p="$(root_pane)"
  "$TAKO_BIN" title --pane "$p" --role "orchestrator-master:t1880" >/dev/null 2>&1
  start_display "$p" "$FOOTER41"
  send_report "$p" "$(report_json busy 37 200000 true "$LIMITS_OK")"
  check_eq "A/B: 報告は受け取る（tako mod の行は reporting）" "reporting" "$("$TAKO_BIN" mod --json 2>/dev/null | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(next((r["state"] for r in d["panes"] if r["pane"]==int(sys.argv[1])), "absent"))' "$p")"
  check_eq "A/B: self は画面（旧挙動）" "screen" "$(self_f "$p" ctx_source)"
  check_eq "A/B: self の ctx_percent は画面の値" "41" "$(self_f "$p" ctx_percent)"
  check_eq "A/B: ctx_mod_reason=legacy_env" "legacy_env" "$(self_f "$p" ctx_mod_reason)"
  check_eq "A/B: worker_status は mod を状態の根拠にしない" "true" "$( [ "$(status_f "$p" status_source)" != mod ] && echo true || echo false)"
  check_eq "A/B: rate_limits は null" "null" "$(self_f "$p" rate_limits)"
  if wait_for 10 is_eq self_f "$p" auto_handoff_tick.ctx_source screen; then
    pass "A/B: #749 の tick は画面"
  else
    fail "A/B: #749 の tick が画面でない（$(self_f "$p" auto_handoff_tick)）"
  fi
  # A/B: 上限の停止の解除時刻は画面のパース（`reset at 3am`）のまま = mod の秒精度の値ではない
  "$TAKO_BIN" limit-resume on --pane "$p" >/dev/null 2>&1
  send_report "$p" "$(report_json idle 37 200000 true "$LIMITS_FULL")"
  show_screen "$p" "$LIMIT_SCREEN"
  if wait_for 15 is_eq lr_f "$p" state.stopped_by_limit true; then
    local ab_reset
    ab_reset="$(lr_f "$p" state.reset_at)"
    check_eq "A/B: 解除時刻は画面のパース（mod の値ではない。${ab_reset}）" "true" "$( [ "$ab_reset" != "$RESET_UNIX" ] && [ "$ab_reset" != null ] && echo true || echo false)"
  else
    fail "A/B: 上限の画面で停止を検知しない（$(lr_f "$p" state)）"
  fi
  "$TAKO_BIN" limit-resume off --pane "$p" >/dev/null 2>&1
  stop_app

  # エッジ: 版の下限未満の claude（2.1.280 の実体）しか無い環境では注入せず、理由が出る
  local old
  old="$(ls -d "$HOME/.local/share/claude/versions/2.1.280" 2>/dev/null)"
  if [ -z "$old" ]; then
    unmeasured "2.1.280 の実体が無いので版の下限未満の段を飛ばした"
  else
    mkdir -p "$TMP/oldbin"
    ln -sf "$old" "$TMP/oldbin/claude"
    start_app version TAKO_PERSIST=0 "PATH=$TMP/oldbin:$PATH"
    p="$(root_pane)"
    check_eq "版の下限未満: ctx_mod_reason=claude_too_old" "claude_too_old" "$(self_f "$p" ctx_mod_reason)"
    check_eq "版の下限未満: worker_status の mod_reason=claude_too_old" "claude_too_old" "$(status_f "$p" mod_reason)"
    stop_app
  fi

  # エッジ: tako の再起動をまたいで届く報告（tmux の中で生き残る送り手 = CLI のフォールバック）
  echo "== fake: 再起動をまたぐ報告（TAKO_PERSIST=1）"
  start_app persist1 TAKO_PERSIST=1
  p="$(root_pane)"
  local loop="$TMP/reporter.sh"
  cat > "$loop" <<EOF
while :; do
  printf '%s' '$(report_json busy 52 1000000 true "$LIMITS_OK")' | "\$TAKO_CLI" mod report >/dev/null 2>&1
  sleep 2
done
EOF
  "$TAKO_BIN" send --pane "$p" "clear; sh '$loop'" >/dev/null 2>&1
  if wait_for 15 is_eq self_f "$p" ctx_source mod; then pass "送り手のループから届く（再起動前）"; else fail "再起動前に届かない（$(self_f "$p" ctx_mod_reason)）"; fi
  stop_app
  start_app persist2 TAKO_PERSIST=1
  local np found=""
  for np in $(seq 1 30); do
    found="$("$TAKO_BIN" mod --json 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
for r in d.get("panes", []):
    if r.get("state") == "reporting":
        print(r["pane"]); break' 2>/dev/null)"
    [ -n "$found" ] && break
    sleep 1
  done
  if [ -n "$found" ]; then
    check_eq "再起動後: 生き残った送り手の報告で self が mod（pane ${found}）" "mod" "$(self_f "$found" ctx_source)"
    check_eq "再起動後: ctx_percent は報告の値" "52" "$(self_f "$found" ctx_percent)"
  else
    fail "再起動後に生き残った送り手の報告が届かない"
  fi
  stop_app
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
}

# --- claude: 実 claude（statusLine なし）-------------------------------------------
claude_available() { command -v claude >/dev/null 2>&1; }
# 自分で起こした隔離 GUI の子孫で動く claude の pid（名前一致の kill はしない。
# `~/.local/bin/claude` は版の実体へのリンクなので comm は版番号になる = 引数の先頭で見分ける）
claude_pid_under() {
  ps -A -o pid=,ppid=,args= | python3 -I -c '
import sys
root = sys.argv[1]
procs = []
for line in sys.stdin:
    parts = line.split(None, 2)
    if len(parts) == 3:
        procs.append((parts[0], parts[1], parts[2]))
kids = {}
for pid, ppid, args in procs:
    kids.setdefault(ppid, []).append((pid, args))
stack = [root]
while stack:
    cur = stack.pop()
    for pid, args in kids.get(cur, []):
        if args.split(" ", 1)[0].endswith("claude") and "--model haiku" in args:
            print(pid); sys.exit()
        stack.append(pid)' "$1"
}
ask() { "$TAKO_BIN" send --pane "$1" --await-prompt "$2" >/dev/null 2>&1; }

phase_claude() {
  echo "== claude: 実 claude（haiku・statusLine なし）で 4 経路と落ち方"
  if ! claude_available; then
    unmeasured "claude が無いので実 claude の段を飛ばした"
    return
  fi
  # チャット表示の live 解決（`claude agents --json`）は CLAUDE_CONFIG_DIR を継承せず、既定 dir と
  # accounts.yaml の登録だけを走査する（#571）。ログインシェルが別の設定 dir を使うなら、隔離した
  # orchestrator dir の accounts.yaml にだけ登録する（本番の accounts.yaml には触らない）
  if [ "$CCD" != "$HOME/.claude" ]; then
    printf 'accounts:\n  t1880:\n    config_dir: %s\n' "$CCD" > "$TAKO_ORCHESTRATOR_DIR/accounts.yaml"
  fi
  start_app claude TAKO_PERSIST=0
  "$TAKO_BIN" ui-mode gui >/dev/null 2>&1
  local p
  p="$(root_pane)"
  "$TAKO_BIN" title --pane "$p" --role "orchestrator-master:t1880" >/dev/null 2>&1
  # statusLine はこのセッションだけ空にする（設定ファイルは書かない）。権限は default + Bash を必ず聞く
  "$TAKO_BIN" send --pane "$p" "cd '$WORKDIR' && claude --model haiku --permission-mode default --settings '{\"statusLine\":{\"type\":\"command\",\"command\":\"true\"},\"permissions\":{\"ask\":[\"Bash\"]}}'" >/dev/null 2>&1
  if ! wait_for 60 is_eq1 mod_f "panes.0.state" reporting; then
    fail "claude の起動で報告が届かない（$(mod_f panes.0)）"
    stop_app
    return
  fi
  ask "$p" "Reply with exactly one word: ok"
  wait_for 90 is_eq self_f "$p" ctx_source mod || true
  local screen_text
  screen_text="$("$TAKO_BIN" read --pane "$p" 2>/dev/null | jf text)"
  check_eq "2. 画面に ctx の表記が無い（statusLine なし）" "false" "$(printf '%s' "$screen_text" | grep -q -E 'ctx +[0-9]+%|[0-9]+% context' && echo true || echo false)"
  check_eq "1/2. self: ctx_source=mod" "mod" "$(self_f "$p" ctx_source)"
  echo "    （self: ctx $(self_f "$p" ctx_percent)% / 窓 $(self_f "$p" ctx_window) / model $(self_f "$p" ctx_model) / classic_events $(self_f "$p" mod_turn.classic_events) / effort $(self_f "$p" mod_turn.effort)）"
  check_eq "1. worker_status: ctx_source=mod" "mod" "$(status_f "$p" ctx_source)"
  check_eq "1. worker_status: status_source=mod / idle" "mod/idle" "$(status_f "$p" status_source)/$(status_f "$p" status)"
  if wait_for 10 is_eq self_f "$p" auto_handoff_tick.ctx_source mod; then pass "1. #749 の tick: ctx_source=mod"; else fail "1. #749 の tick が mod でない（$(self_f "$p" auto_handoff_tick)）"; fi
  if wait_for 20 is_eq1 ui_f "chat_header.$p.ctx_source" mod; then
    pass "1. チャットヘッダ: ctx_source=mod（$(ui_f "chat_header.$p")）"
  else
    fail "1. チャットヘッダが mod でない（$(ui_f chat_header) / 表示 $(ui_f "pane_display.$p") / 理由 $(ui_f "pane_display_reason.$p")）"
  fi
  check_eq "effort が mod から取れる（turn.step。classic 系の有無に依らない）" "true" "$( [ "$(self_f "$p" mod_turn.effort)" != null ] && echo true || echo false)"
  local limits
  limits="$(self_f "$p" rate_limits.windows.0.kind)"
  if [ "$limits" = null ] || [ "$limits" = absent ]; then
    unmeasured "使用制限が報告に無い（API キー利用など。rate_limits は null）"
  else
    check_eq "使用制限の解除時刻は unix 秒（mod の ISO を読んだ値）" "true" "$(self_f "$p" rate_limits.windows.0.resets_at | grep -q -E '^[0-9]{10}$' && echo true || echo false)"
  fi

  # 権限ダイアログ（ask ルール = classic_events に依らず mod が拾える）
  ask "$p" "Use the Bash tool to run exactly: echo tako1880"
  if wait_for 60 is_eq status_f "$p" status waiting; then
    pass "権限ダイアログ: worker_status=waiting（mod_turn $(status_f "$p" mod_turn.turn) / warnings $(status_f "$p" warnings)）"
  else
    fail "権限ダイアログで waiting にならない（$(status_f "$p" status) / $(status_f "$p" mod_turn)）"
  fi
  "$TAKO_BIN" orchestrator respond --pane "$p" --choice no >/dev/null 2>&1
  wait_for 60 is_eq status_f "$p" status idle || true

  # 3. 報告を止める（claude を SIGSTOP）→ 45 秒後に画面 / transcript へ落ちて理由が出る
  CLAUDE_PID="$(claude_pid_under "$APP_PID")"
  if [ -z "$CLAUDE_PID" ]; then
    unmeasured "claude の pid が引けず、報告を止める注入を飛ばした"
  else
    kill -STOP "$CLAUDE_PID"
    echo "    （claude（pid ${CLAUDE_PID}）を止めて 47 秒待つ）"
    sleep 47
    local src reason
    src="$(self_f "$p" ctx_source)"
    reason="$(self_f "$p" ctx_mod_reason)"
    check_eq "3. 止めて 47 秒後: ctx_mod_reason=mod_stale" "mod_stale" "$reason"
    check_eq "3. 止めて 47 秒後: mod 以外へ落ちる（$src / ctx_reason $(self_f "$p" ctx_reason)）" "true" "$( [ "$src" != mod ] && echo true || echo false)"
    if wait_for 10 is_eq self_f "$p" auto_handoff_tick.ctx_mod_reason mod_stale; then pass "3. #749 の tick も落ちて理由が出る"; else fail "3. tick に理由が出ない（$(self_f "$p" auto_handoff_tick)）"; fi
    kill -CONT "$CLAUDE_PID"
    CLAUDE_PID=""
    if wait_for 30 is_eq self_f "$p" ctx_source mod; then pass "再開した claude の報告で mod へ戻る"; else fail "再開後に mod へ戻らない"; fi
  fi
  ask "$p" "/exit"
  wait_for 30 shell_idle "$p" || true
  stop_app
}

want fake && phase_fake
want claude && phase_claude

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
