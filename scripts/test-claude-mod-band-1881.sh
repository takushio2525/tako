#!/usr/bin/env bash
# test-claude-mod-band-1881.sh — tako mod S3（#1881）の実経路テスト: Claude Code の画面の帯とサイドバー
#
# 隔離した data / discovery / tmux で**実 tako-app** を立て、**実 claude**（haiku の短いターン）に
# 帯（プロンプトの上の 1 行）とサイドバー（/tako）を描かせ、画面の capture で次を確かめる
# （番号は Issue #1881 の受け入れ条件 + master の追加条件）:
#   0. 静的: `claude plugin validate` / `claude plugin test`（terminal と desktop の両 surface）
#   1. 80 / 144 / 300 桁（行数は S0 で `↓ 2 more` に畳まれた 21 行）で帯が 1 行に収まる。
#      閾値を 0 にして（TAKO_1881_BAND_THRESHOLD=0）区切りを全部並べた最も長い帯で測る
#   2. 権限ダイアログ・AskUserQuestion の表示中に帯が隠れ、閉じた後に戻る（連続でも）
#   4. 既定の閾値では ctx / 使用制限を帯に出さない（閾値未満のとき）
#   5. /tako でサイドバーが開き、帯のトグルが $.store に保存されて claude の再起動後も保たれる。
#      tako mod band on|off（CLI の中継）でも切り替わり、tako mod に状態が出る
#   6. A/B: TAKO_1877_S3_LEGACY=1 では報告は届くが帯を描かない（同一バイナリで旧挙動）
#   エッジ: tako の外の claude（休眠）・tako が止まって報告が古くなる・worker 0 本・長いタブ名
#
# 幅は**隔離 tmux のセッションの幅**で作る（`new-session -x` / `resize-window`）。その claude には
# tako のペインと同じ env（TAKO_PANE_ID / TAKO_CLI / CLAUDE_CODE_PLUGIN_DIRS = tako が展開した mod）を
# 渡す = tako の tmux ペインの中の claude と同じ条件。既定の閾値の段は隔離 GUI の**直接ペイン**で
# 素の `claude` を打つ（tako が env を注入する本物の経路）。
#
# **本番の tako / 本番の claude 設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 落とすのは自分で起こした pid だけ（pkill / killall は使わない）。claude の設定 dir は認証のために
# 読むだけで使う。**例外は帯のトグルの $.store**（`<設定 dir>/plugins/store/tako_inline-*.json`。
# 名前は mod の名前で決まり置き場に依らない = 本番の tako mod と同じファイル）で、テストの前に退避し、
# 終わったら元へ戻す（無かったものは消す）。作業 dir は信頼済みの dir（既定は main の worktree）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 設定 dir: 既定はいまの CLAUDE_CONFIG_DIR（無ければ ~/.claude）。CCD2 に別の設定 dir を渡すと、
# 帯がそちらでも描かれるか（組織アカウントでは classic 系が mod に届かない = FR-2.42.7 の差）も見る。
# 選んで回す: ONLY=static|widths|default|ab（既定は全部）。**CI には登録しない**（実 GUI と実 claude の
# 認証が要る）。CI 側の担保は mod の `claude plugin test`・Rust の単体テストと番犬。
#
# 使い方: bash scripts/test-claude-mod-band-1881.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_1877_S3_LEGACY \
  TAKO_1881_BAND_THRESHOLD
# claude の中から回すと子セッションの印を継ぐ（transcript を保存しない等）。外す
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

TMP="$(mktemp -d /tmp/tako-1881-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1881-$$"
ROWS=21
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }

CCD="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
CCD2="${CCD2:-}"
WORKDIR="${CLAUDE_WORKDIR:-$(git -C "$REPO_ROOT" worktree list --porcelain | awk 'NR == 1 { print $2 }')}"
# 画面の採取の書き出し先（新しい挙動と旧挙動で分ける）
CAP_NEW="$TMP/capture-new"
CAP_OLD="$TMP/capture-legacy"
mkdir -p "$CAP_NEW" "$CAP_OLD"

# --- 帯のトグルの $.store を退避・復元する（本番の tako mod と同じファイル）-----------------
STORE_BACKUP="$TMP/store-backup"
store_dirs() {
  printf '%s\n' "$CCD"
  [ -n "$CCD2" ] && [ "$CCD2" != "$CCD" ] && printf '%s\n' "$CCD2"
  return 0
}
# 設定 dir ごとに「前からあった store / plugins/store / 空の inline dir」を控える
backup_stores() {
  local i=0 ccd f
  while IFS= read -r ccd; do
    i=$((i + 1))
    mkdir -p "$STORE_BACKUP/$i"
    printf '%s' "$ccd" > "$STORE_BACKUP/$i/ccd"
    [ -d "$ccd/plugins/store" ] && : > "$STORE_BACKUP/$i/had-store-dir"
    [ -d "$ccd/plugins/data/tako-inline" ] && : > "$STORE_BACKUP/$i/had-inline"
    for f in "$ccd"/plugins/store/tako_inline-*.json; do
      [ -f "$f" ] && /bin/cp -f "$f" "$STORE_BACKUP/$i/"
    done
  done <<EOF
$(store_dirs)
EOF
}
restore_stores() {
  local d ccd f b
  for d in "$STORE_BACKUP"/*; do
    [ -f "$d/ccd" ] || continue
    ccd="$(cat "$d/ccd")"
    for f in "$ccd"/plugins/store/tako_inline-*.json; do
      [ -f "$f" ] || continue
      b="$d/$(basename "$f")"
      if [ -f "$b" ]; then /bin/cp -f "$b" "$f"; else rm -f "$f"; fi
    done
    [ -f "$d/had-store-dir" ] || rmdir "$ccd/plugins/store" 2>/dev/null || true
    [ -f "$d/had-inline" ] || rmdir "$ccd/plugins/data/tako-inline" 2>/dev/null || true
  done
}

FAKE_PID=""
cleanup() {
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET"
  restore_stores
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "tako mod の帯の実経路テスト"

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
backup_stores

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

# --- 観測の道具 ------------------------------------------------------------------
mod_json() { "$TAKO_BIN" mod --json 2>/dev/null; }
mod_field() {
  mod_json | python3 -I -c '
import json, sys
path = sys.argv[1].split(".")
try:
    v = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for k in path:
    if k.startswith("pane="):
        want = int(k[5:])
        rows = v if isinstance(v, list) else v.get("panes", [])
        v = next((r for r in rows if r.get("pane") == want), None)
    elif isinstance(v, dict):
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
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
field_is() { [ "$(mod_field "$1")" = "$2" ]; }
root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}
root_tab() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["id"])'
}
claude_available() { command -v claude >/dev/null 2>&1; }

tmx() { tmux -L "$TMUX_SOCKET" "$@"; }
# tako のペインと同じ env で claude を隔離 tmux のセッションに立てる（pane が空 = tako の外）
start_claude() {
  local name="$1" width="$2" ccd="$3" pane="$4"
  shift 4
  local mod_dir
  mod_dir="$(mod_field plugin_dir)"
  tmx kill-session -t "=$name" >/dev/null 2>&1
  if [ -n "$pane" ]; then
    tmx new-session -d -s "$name" -x "$width" -y "$ROWS" -c "$WORKDIR" \
      -e "TAKO_ISOLATED=1" -e "TAKO_DATA_DIR=$TAKO_DATA_DIR" -e "TAKO_DISCOVERY_DIR=$TAKO_DISCOVERY_DIR" \
      -e "TAKO_ORCHESTRATOR_DIR=$TAKO_ORCHESTRATOR_DIR" -e "CLAUDE_CONFIG_DIR=$ccd" \
      -e "TAKO_PANE_ID=$pane" -e "TAKO_CLI=$TAKO_BIN" -e "CLAUDE_CODE_PLUGIN_DIRS=$mod_dir" \
      claude --model haiku ${@+"$@"}
  else
    tmx new-session -d -s "$name" -x "$width" -y "$ROWS" -c "$WORKDIR" \
      -e "TAKO_ISOLATED=1" -e "TAKO_DATA_DIR=$TAKO_DATA_DIR" -e "TAKO_DISCOVERY_DIR=$TAKO_DISCOVERY_DIR" \
      -e "CLAUDE_CONFIG_DIR=$ccd" -e "TAKO_CLI=$TAKO_BIN" -e "CLAUDE_CODE_PLUGIN_DIRS=$mod_dir" \
      claude --model haiku ${@+"$@"}
  fi
  tmx set-option -t "=$name" remain-on-exit off >/dev/null 2>&1
}
cap() { tmx capture-pane -p -t "=$1:" 2>/dev/null; }
# 帯の行（`tako | ` を含む行）の数
band_count() { cap "$1" | grep -c 'tako | ' | tr -d ' '; }
band_line() { cap "$1" | grep 'tako | ' | head -1; }
band_present() { [ "$(band_count "$1")" = 1 ]; }
band_absent() { [ "$(band_count "$1")" = 0 ]; }
# 端末の桁数（東アジアの全角 = 2 桁）
cells() {
  python3 -I -c '
import sys, unicodedata
s = sys.argv[1]
print(sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s))' "$1"
}
type_line() {
  tmx send-keys -t "=$1:" -l "$2"
  sleep 1
  tmx send-keys -t "=$1:" Enter
}
key() { tmx send-keys -t "=$1:" "$2"; }
save_cap() { cap "$1" | mask > "$2"; }
screen_has() { cap "$1" | grep -q -- "$2"; }

# claude の起動を待つ（そのペインの報告が届き、帯の材料が返るまで）
wait_claude() {
  local session="$1" pane="$2"
  if ! wait_for 90 field_is "panes.pane=$pane.state" reporting; then
    fail "claude（${session}）の報告が届かない（state=$(mod_field "panes.pane=$pane.state")）"
    cap "$session" | mask | tail -15 | sed 's/^/      | /'
    return 1
  fi
}
# 1 ターン回す（答えの語が画面に出るまで）
one_turn() {
  local session="$1" word="$2"
  type_line "$session" "Reply with exactly one word: ${word}"
  wait_for 90 screen_has "$session" "$word" || return 1
  sleep 3
}

# --- 0. 静的（validate / test）---------------------------------------------------------
phase_static() {
  echo "== 0. claude plugin validate / test（terminal と desktop）"
  if ! claude_available; then
    unmeasured "claude が無いので validate / test を飛ばした"
    return
  fi
  local cfg="$TMP/ccfg" out
  mkdir -p "$cfg"
  out="$(CLAUDE_CONFIG_DIR="$cfg" claude plugin validate "$REPO_ROOT/crates/tako-core/claude-mod" 2>&1)"
  if printf '%s' "$out" | grep -q 'Validation passed'; then pass "claude plugin validate"; else fail "claude plugin validate"; printf '%s\n' "$out" | mask | tail -20; fi
  check_eq "帯・ペイン・/tako の登録がある（validate の申告）" "true" \
    "$(printf '%s' "$out" | grep -q 'ui.render{component=AbovePrompt}, ui.render{component=Pane, requestId=tako}, command.run{command=tako}' && echo true || echo false)"
  if printf '%s' "$out" | grep -q 'gating hook without .catch'; then fail "ゲートになるフックに .catch の無いものがある"; else pass "ゲートになるフックはすべて .catch 付き"; fi
  out="$(CLAUDE_CONFIG_DIR="$cfg" claude plugin test "$REPO_ROOT/crates/tako-core/claude-mod" 2>&1)"
  if printf '%s' "$out" | grep -q ' 0 fail'; then pass "claude plugin test（$(printf '%s' "$out" | grep -E '^ *[0-9]+ pass' | tr -d ' ')）"; else fail "claude plugin test"; printf '%s\n' "$out" | mask | tail -30; fi
  local dirty
  # 未追跡のファイル（型定義 `.claude-plugin/types/` など）を残さない（テストのソースは除く）
  dirty="$(git -C "$REPO_ROOT" status --porcelain --untracked-files=all -- crates/tako-core/claude-mod | grep '^??' | grep -v '/tests/[^/]*\.test\.ts$' | head -1)"
  check_eq "validate / test がソースツリーへ生成物を残さない" "" "$dirty"
}

# --- 1 / 2 / 5. 幅・ダイアログ・トグル（閾値 0 = 区切りを全部並べる）-----------------------
phase_widths() {
  echo "== 1 / 2 / 5. 幅・ダイアログ・トグル（TAKO_1881_BAND_THRESHOLD=0）"
  start_app widths TAKO_PERSIST=0 TAKO_1881_BAND_THRESHOLD=0
  if ! claude_available; then
    unmeasured "claude が無いので実 claude の段を飛ばした"
    stop_app
    return
  fi
  local p tab w1 w2
  p="$(root_pane)"
  tab="$(root_tab)"
  # 長いペイン名・タブ名（帯は 24 桁で切り詰める）と、worker 2 本（唯一の master への寄せ規則）
  "$TAKO_BIN" title --pane "$p" --role orchestrator-master "オーケストレーターの master ペイン（長い名前の切り詰め）" >/dev/null 2>&1
  "$TAKO_BIN" tab rename --tab "$tab" "tako-wt-1881 の帯の検証タブ（これも長いタブ名）" >/dev/null 2>&1
  w1="$("$TAKO_BIN" split --pane "$p" --right 2>/dev/null | tr -dc '0-9')"
  w2="$("$TAKO_BIN" split --pane "$p" --down 2>/dev/null | tr -dc '0-9')"
  "$TAKO_BIN" title --pane "$w1" --role worker:a "w1-question" >/dev/null 2>&1
  "$TAKO_BIN" title --pane "$w2" --role worker:b "w2-idle" >/dev/null 2>&1
  # w1 は質問待ち（mod の報告を模す = claude を立てずに要注意を 1 つ作る。45 秒で失効するので回し続ける）
  (
    while :; do
      printf '%s' '{"schema":1,"mod_version":"t","at":1,"turn":"question","pending_tool":"AskUserQuestion","rate_limits":[],"ended":false}' \
        | TAKO_PANE_ID="$w1" "$TAKO_BIN" mod report >/dev/null 2>&1
      sleep 10
    done
  ) &
  FAKE_PID=$!

  # 権限ダイアログを出すため、このセッションだけ default モード + Bash を必ず聞く（設定ファイルは書かない）
  start_claude band 300 "$CCD" "$p" --permission-mode default --settings '{"permissions":{"ask":["Bash"]}}'
  wait_claude band "$p" || { stop_app; return; }
  if one_turn band ok1881; then pass "1 ターン回った（ctx / 使用制限が報告に載る）"; else fail "1 ターンが回らない"; fi
  wait_for 30 band_present band || true

  # 1. 300 → 144 → 80 桁（行数 21）
  local cols line width segs want_kinds kind label ok more
  for cols in 300 144 80; do
    tmx resize-window -t "=band:" -x "$cols" -y "$ROWS" >/dev/null 2>&1
    sleep 4
    save_cap band "$CAP_NEW/band-${cols}.txt"
    check_eq "${cols} 桁: 帯はちょうど 1 行" "1" "$(band_count band)"
    line="$(band_line band | sed -e 's/[[:space:]]*$//')"
    width="$(cells "$line")"
    if [ "$width" -le "$cols" ]; then pass "${cols} 桁: 帯の行は ${width} 桁（<= ${cols}）"; else fail "${cols} 桁: 帯の行が ${width} 桁で溢れた"; fi
    more="$(cap band | grep -c -E '↓ [0-9]+ more' | tr -d ' ')"
    check_eq "${cols} 桁: 帯が畳まれない（↓ N more が出ない）" "0" "$more"
    # tako mod の行（mod が描いたと言っている区切り）と画面を突き合わせる = 折り返していない
    wait_for 5 field_is "panes.pane=$p.report.band.columns" "$((cols - 5))" || true
    segs="$(mod_field "panes.pane=$p.report.band.segments")"
    echo "    ${cols} 桁: columns=$(mod_field "panes.pane=$p.report.band.columns") segments=${segs}"
    printf '    | %s\n' "$line" | mask
    ok=true
    for kind in workers attention ctx five_hour seven_day tab; do
      case "$segs" in *"\"$kind\""*) ;; *) continue ;; esac
      case "$kind" in
        workers) label="worker 2" ;;
        attention) label="要注意 1" ;;
        ctx) label="ctx " ;;
        five_hour) label="5h " ;;
        seven_day) label="7d " ;;
        tab) label="タブ " ;;
      esac
      case "$line" in *"$label"*) ;; *) ok=false; echo "      （${kind} = '${label}' が帯の行に無い）" ;; esac
    done
    check_eq "${cols} 桁: mod が描いたと報告した区切りが全部同じ行にある" "true" "$ok"
  done
  # 300 桁は全部、80 桁は優先度の低いもの（タブ名 → worker 数）から落ちる
  tmx resize-window -t "=band:" -x 300 -y "$ROWS" >/dev/null 2>&1
  sleep 4
  segs="$(mod_field "panes.pane=$p.report.band.segments")"
  check_eq "300 桁ではタブ名・worker 数・要注意・ctx が全部出る" "true" \
    "$(python3 -I -c 'import json,sys; s=json.loads(sys.argv[1]); print(str(all(k in s for k in ["tab","workers","attention","ctx"])).lower())' "$segs")"

  # 2. ダイアログの表示中は隠れ、閉じたら戻る（144 桁。権限 → 質問 → 権限 2 連続）
  tmx resize-window -t "=band:" -x 144 -y "$ROWS" >/dev/null 2>&1
  sleep 3
  type_line band "Use the Bash tool to run exactly: echo tako1881a"
  if wait_for 60 screen_has band "Do you want to proceed"; then
    save_cap band "$CAP_NEW/dialog-permission.txt"
    check_eq "権限ダイアログの表示中は帯が無い" "0" "$(band_count band)"
    key band Escape
    if wait_for 60 band_present band; then pass "権限ダイアログを閉じた後に帯が戻る"; else fail "権限ダイアログの後に帯が戻らない"; save_cap band "$CAP_NEW/dialog-permission-after.txt"; fi
  else
    fail "権限ダイアログが出ない"
    cap band | mask | tail -15 | sed 's/^/      | /'
  fi
  wait_for 60 field_is "panes.pane=$p.report.turn" idle || true
  type_line band "Call the AskUserQuestion tool right now (do not answer in plain text) to ask me which color I prefer, with exactly two options: red and blue."
  if wait_for 60 field_is "panes.pane=$p.report.turn" question; then
    sleep 2
    save_cap band "$CAP_NEW/dialog-question.txt"
    check_eq "AskUserQuestion の表示中は帯が無い" "0" "$(band_count band)"
    key band Escape
    if wait_for 60 band_present band; then pass "質問を閉じた後に帯が戻る"; else fail "質問の後に帯が戻らない"; fi
  else
    fail "AskUserQuestion が出ない（turn=$(mod_field "panes.pane=$p.report.turn")）"
  fi
  wait_for 60 field_is "panes.pane=$p.report.turn" idle || true
  type_line band "Use the Bash tool twice, one call after the other: first run exactly: echo tako1881b ; then run exactly: echo tako1881c"
  local n
  for n in 1 2; do
    if wait_for 60 screen_has band "Do you want to proceed"; then
      check_eq "連続するダイアログ ${n} 本目の表示中は帯が無い" "0" "$(band_count band)"
      # 1 本目は承認（Enter = 既定の Yes）、2 本目は拒否
      if [ "$n" = 1 ]; then key band Enter; else key band Escape; fi
      sleep 2
    else
      fail "連続するダイアログの ${n} 本目が出ない"
    fi
  done
  if wait_for 60 band_present band; then pass "連続するダイアログの後に帯が戻る"; else fail "連続するダイアログの後に帯が戻らない"; fi
  save_cap band "$CAP_NEW/dialog-after.txt"
  wait_for 60 field_is "panes.pane=$p.report.turn" idle || true

  # 5. トグル: /tako band off → 再起動しても隠れたまま → tako mod band on（中継）で戻る
  type_line band "/tako band off"
  if wait_for 20 band_absent band; then pass "/tako band off で帯が消える"; else fail "/tako band off で帯が消えない"; fi
  wait_for 10 field_is "panes.pane=$p.report.band.hidden" true || true
  check_eq "tako mod の行に band.hidden=true が出る" "true" "$(mod_field "panes.pane=$p.report.band.hidden")"
  local store
  store="$(ls "$CCD"/plugins/store/tako_inline-*.json 2>/dev/null | head -1)"
  check_eq "トグルが \$.store に保存される" "true" \
    "$( [ -n "$store" ] && python3 -I -c 'import json,sys; print(str(json.load(open(sys.argv[1]))["band"]["hidden"]).lower())' "$store" 2>/dev/null || echo absent)"
  # /tako でサイドバー（幅を問わずコマンドから開ける）
  type_line band "/tako"
  if wait_for 20 screen_has band "w1-question"; then
    pass "/tako でサイドバーが開き、worker が並ぶ"
    save_cap band "$CAP_NEW/sidebar.txt"
    check_eq "サイドバーに要注意（質問待ち）が出る" "true" "$(cap band | grep -q '質問待ち' && echo true || echo false)"
    check_eq "サイドバーのトグルは「帯を出す」（隠しているので）" "true" "$(cap band | grep -q '帯を出す' && echo true || echo false)"
    key band Escape
  else
    fail "/tako でサイドバーが開かない"
    cap band | mask | tail -20 | sed 's/^/      | /'
  fi
  # claude を終わらせて立て直す（$.store に残ったトグルが効く）
  type_line band "/exit"
  sleep 5
  start_claude band 144 "$CCD" "$p"
  wait_claude band "$p" || { stop_app; return; }
  sleep 6
  check_eq "再起動した claude でも帯は隠れたまま" "0" "$(band_count band)"
  check_eq "再起動後の tako mod の行も band.hidden=true" "true" "$(mod_field "panes.pane=$p.report.band.hidden")"
  "$TAKO_BIN" mod band on >/dev/null 2>&1
  check_eq "tako mod band on の中継が status に出る" "false" "$(mod_field band.request.hidden)"
  if wait_for 30 band_present band; then pass "tako mod band on（中継）で帯が戻る（次の報告 = 最大 15 秒）"; else fail "tako mod band on で帯が戻らない"; fi
  wait_for 10 field_is "panes.pane=$p.report.band.hidden" false || true
  check_eq "戻した後の tako mod の行は band.hidden=false" "false" "$(mod_field "panes.pane=$p.report.band.hidden")"
  "$TAKO_BIN" mod 2>/dev/null | mask | sed 's/^/    /'
  type_line band "/exit"
  sleep 3
  kill "$FAKE_PID" 2>/dev/null
  FAKE_PID=""
  stop_app
}

# --- 4. 既定の閾値・直接ペイン・worker 0 本・設定 dir 2 つ・tako の外・報告が古い -------------
phase_default() {
  echo "== 4. 既定の閾値（直接ペインの素の claude）・エッジ"
  start_app default TAKO_PERSIST=0
  if ! claude_available; then
    unmeasured "claude が無いので実 claude の段を飛ばした"
    stop_app
    return
  fi
  local p screen line pct
  p="$(root_pane)"
  # 直接ペインで素の claude（tako が env を注入する本物の経路。設定 dir はペインのシェルが決める）
  "$TAKO_BIN" send --pane "$p" "cd '$WORKDIR' && claude --model haiku" >/dev/null 2>&1
  if ! wait_for 90 field_is "panes.pane=$p.state" reporting; then
    fail "直接ペインの claude の報告が届かない"
    stop_app
    return
  fi
  "$TAKO_BIN" send --pane "$p" --await-prompt "Reply with exactly one word: ok1881d" >/dev/null 2>&1
  wait_for 90 field_is "panes.pane=$p.report.last_turn.reason" answer || true
  sleep 3
  screen="$("$TAKO_BIN" read --pane "$p" 2>/dev/null | python3 -I -c 'import json,sys
raw=sys.stdin.read()
try: print(json.loads(raw).get("text",""))
except Exception: print(raw)')"
  printf '%s\n' "$screen" | mask > "$CAP_NEW/direct-default.txt"
  line="$(printf '%s\n' "$screen" | grep 'tako | ' | head -1)"
  check_eq "直接ペインの claude に帯が 1 行出る" "1" "$(printf '%s\n' "$screen" | grep -c 'tako | ' | tr -d ' ')"
  printf '    | %s\n' "$line" | mask
  pct="$(mod_field "panes.pane=$p.report.context.percent")"
  echo "    （ctx ${pct}% / 使用制限 $(mod_field "panes.pane=$p.report.rate_limits")）"
  # 閾値（80%）未満なら ctx / 使用制限は帯に出さない
  local expect_ctx expect_limit
  expect_ctx="$(python3 -I -c 'import sys; v=sys.argv[1]; print("true" if v not in ("null","absent","") and float(v) >= 80 else "false")' "$pct")"
  expect_limit="$(mod_json | python3 -I -c '
import json, sys
p = int(sys.argv[1]); d = json.load(sys.stdin)
row = next((r for r in d.get("panes", []) if r.get("pane") == p), {})
lim = (row.get("report") or {}).get("rate_limits") or []
print("true" if any(l.get("percent_used", 0) >= 80 for l in lim) else "false")' "$p")"
  check_eq "ctx が閾値未満なら帯に ctx が無い（閾値以上なら有る）" "$expect_ctx" "$(contains "$line" "ctx ")"
  check_eq "使用制限が閾値未満なら帯に 5h / 7d が無い（閾値以上なら有る）" "$expect_limit" \
    "$( (contains "$line" "5h "; contains "$line" "7d ") | grep -q true && echo true || echo false)"
  check_eq "worker 0 本なら帯に worker 数を出さない" "false" "$( (contains "$line" "worker"; contains "$line" "workers") | grep -q true && echo true || echo false)"

  # 2 つ目の設定 dir（組織 / 個人）でも描かれるか
  if [ -n "$CCD2" ] && [ "$CCD2" != "$CCD" ]; then
    local q
    q="$("$TAKO_BIN" split --pane "$p" --down 2>/dev/null | tr -dc '0-9')"
    start_claude ccd2 144 "$CCD2" "$q"
    if wait_claude ccd2 "$q" && wait_for 30 band_present ccd2; then
      pass "CCD2 の設定 dir の claude にも帯が出る（classic_events=$(mod_field "panes.pane=$q.report.classic_events")）"
    else
      fail "CCD2 の設定 dir の claude に帯が出ない"
    fi
    save_cap ccd2 "$CAP_NEW/ccd2.txt"
    type_line ccd2 "/exit"
  else
    unmeasured "CCD2（2 つ目の設定 dir）を渡していないので、設定 dir の差は見ていない"
  fi
  local own_classic
  own_classic="$(mod_field "panes.pane=$p.report.classic_events")"
  echo "    （直接ペインの claude の classic_events=${own_classic}）"

  # tako の外の claude（TAKO_PANE_ID が無い）: mod は読み込まれるが休眠 = 帯を描かない
  start_claude outside 144 "$CCD" ""
  sleep 20
  save_cap outside "$CAP_NEW/outside.txt"
  check_eq "tako の外の claude には帯が出ない（休眠）" "0" "$(band_count outside)"
  type_line outside "/exit"

  # 報告が古い: tako を止めると 45 秒で帯が消える（claude はそのまま動く）
  start_claude stale 144 "$CCD" "$p"
  wait_claude stale "$p" || true
  wait_for 30 band_present stale || true
  check_eq "tako が動いている間は帯がある" "1" "$(band_count stale)"
  "$TAKO_BIN" send --pane "$p" "/exit" >/dev/null 2>&1
  stop_app
  if wait_for 70 band_absent stale; then pass "tako が止まると帯が消える（応答が 45 秒途切れた）"; else fail "tako が止まっても帯が残る"; fi
  save_cap stale "$CAP_NEW/stale.txt"
  type_line stale "/exit"
}

# --- 6. A/B（TAKO_1877_S3_LEGACY=1）---------------------------------------------------
phase_ab() {
  echo "== 6. A/B（TAKO_1877_S3_LEGACY=1 = 帯を描かない旧挙動）"
  start_app ab TAKO_PERSIST=0 TAKO_1877_S3_LEGACY=1
  check_eq "status に A/B が出る（band.legacy）" "true" "$(mod_field band.legacy)"
  if ! claude_available; then
    unmeasured "claude が無いので A/B の実 claude の段を飛ばした"
    stop_app
    return
  fi
  local p
  p="$(root_pane)"
  start_claude ab 144 "$CCD" "$p"
  if wait_claude ab "$p"; then pass "A/B でも報告は届く（S1 / S2 はそのまま）"; fi
  sleep 15
  save_cap ab "$CAP_OLD/legacy.txt"
  check_eq "A/B では帯を描かない" "0" "$(band_count ab)"
  check_eq "A/B の行は band.shown=false" "false" "$(mod_field "panes.pane=$p.report.band.shown")"
  type_line ab "/exit"
  sleep 3
  stop_app
}

want static && phase_static
want widths && phase_widths
want default && phase_default
want ab && phase_ab

echo
echo "画面の採取: 新しい挙動 ${CAP_NEW} / 旧挙動 ${CAP_OLD}（KEEP=1 で残す）" | mask
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
