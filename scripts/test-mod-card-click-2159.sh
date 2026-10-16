#!/usr/bin/env bash
# test-mod-card-click-2159.sh — tako mod の会話の行のカードをクリック 1 回で実行する（#2159。FR-2.42.53）の実経路テスト
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）を**一時の設定 dir** と**偽の Messages API**
# （scripts/lib/fake-messages-api.mjs。「Show this card: {JSON}」で tako の MCP の tako_show_command を呼ぶ）で
# 動かし、隔離 GUI（tako-vd・TAKO_PERSIST=0・自動リネーム off）のペインで AI がカードを出す経路を確かめる。
# 番号は Issue #2159 の受け入れ条件:
#   steps      1. 手順の A/B: 新 = 会話の行のカードの `[ run ]` をクリック 1 回（`tako click` = GUI のクリックと同じ
#                 判断とバイト列）で新しいペインと実行記録（`show-command --list` の runs = スマホ・GPUI のカードと同じ）。
#                 旧（TAKO_2159_LEGACY=1）= 会話の行にボタンが無く、カードの行をクリックしても何も起きない。
#                 帯の ctrl+x → Tab → r（3 打 + 入力欄へ戻す Esc）で初めて実行される
#   mouseoff   3. `tako mouse off`（クリックを渡さない）では会話の行にボタンを置かず、帯のホットキーで今どおり押せる
#   place      4. `cards.place=tako` と mod なしのペイン（CLAUDE_CODE_PLUGIN_DIRS を外した claude）は今の挙動
#                 （GPUI のカード帯が出る・会話の行にボタンを置かない）
#   copy       会話の行の `[ copy ]` をクリックで押すと論理文字列（157 字・改行なし）が $.ui.copy へ渡る
#              （**隔離 tmux の claude で押す** = 本物のクリップボードへ書かない。クリックは SGR の列を send-keys -H で）
#   visual     1 / 2. visual-test `mod-card-click`（実 OS と同じ PlatformInput のクリック 1 回で実行・150 ms 後の
#                 2 打目と実行中の押し直しで 2 本目が走らない・画像を書き出す）。A/B（TAKO_2159_LEGACY=1）は ① で落ちる。
#                 新旧で書き出し先を分ける
#   static     `claude plugin validate --strict` / `claude plugin test`（terminal と desktop の両 surface）
#
# **本物の設定 dir（~/.claude*）には書かない**: claude の設定 dir は mktemp 配下（認証は写さず、偽 API と
# ダミーの API キーを一時の .claude.json で承認済みにする）。前後で本物の ~/.claude*/skills・plugins の
# 一覧とハッシュを比べる。**本物のクリップボードにも書かない**: GUI のペインの claude の $.ui.copy は偽の pbcopy を
# 置いても OSC 52 で tako 経由のシステムのクリップボードへ届く（#1963）ので、Copy は隔離 tmux（set-clipboard off・
# クライアントなし）の claude でだけ押し、前後で pbpaste のハッシュを比べる。
# 本番 tako が配る env（TAKO_SOCKET / TAKO_PANE_ID / TAKO_CLI / CLAUDE_CODE_PLUGIN_DIRS 等）は最初に落とす。
# 落とすのは自分で起こした pid と tmux ソケットだけ。
#
# 使い方: bash scripts/test-mod-card-click-2159.sh   （ONLY=static|steps|mouseoff|place|copy|visual）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は mod の `claude plugin test`・
# Rust の単体テストと番犬（issue2159_mod_card_click_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_1877_S3_LEGACY \
  TAKO_1877_S7_LEGACY TAKO_1963_LEGACY TAKO_2159_LEGACY TAKO_1961_LEGACY TAKO_1881_BAND_THRESHOLD \
  TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
# 自分の claude が使っている本物の設定 dir・認証を混ぜない
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

REAL_CLAUDE="$(command -v claude 2>/dev/null || true)"
if [ -z "$REAL_CLAUDE" ]; then
  echo "未実測: PATH に claude が無い"
  exit 3
fi
command -v node >/dev/null 2>&1 || { echo "未実測: node が無い（偽 API に使う）"; exit 3; }
command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 3; }

# --- 本物の設定 dir とクリップボードの前後比較 ----------------------------------------------
REAL_NOISE='/\.in_use/|/synced/|/\.last_inuse_sweep$|/install-counts-cache\.json$'
snap_real() {
  local d
  for d in "$HOME"/.claude*/skills "$HOME"/.claude*/plugins; do
    [ -d "$d" ] || continue
    find "$d" -type f -print0 2>/dev/null | sort -z | xargs -0 shasum -a 256 2>/dev/null
    find "$d" \( -type d -o -type l \) 2>/dev/null | sort
  done | grep -Ev "$REAL_NOISE" | shasum -a 256 | cut -c1-16
}
# 中身は読まない（ハッシュだけ比べる）
snap_clipboard() { /usr/bin/pbpaste 2>/dev/null | shasum -a 256 | cut -c1-16; }
REAL_BEFORE="$(snap_real)"
CLIP_BEFORE="$(snap_clipboard)"

# --- ビルド（visual-test の節を含む GUI と CLI。rebase 後の古いバイナリを掴まない）------------
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-app --features visual-test && run_heavy cargo build -q -p tako-cli) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

TMP="$(mktemp -d /tmp/tako-2159-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
FAKE_PID=""
TMUX_SOCKET="tako-2159-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }
tmx() { tmux -u -L "$TMUX_SOCKET" "$@"; }
OUT="${OUT_DIR:-$TMP/out}"

cleanup() {
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmx kill-server >/dev/null 2>&1 || true
  tmux -L "$TMUX_SOCKET-gui" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET" "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET-gui"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  if [ -n "${KEEP:-}" ] || [ -n "${OUT_DIR:-}" ]; then echo "（$TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
caffeinate -d -u -w $$ >/dev/null 2>&1 &

mkdir -p "$OUT/capture-new" "$OUT/capture-legacy" "$OUT/visual-new" "$OUT/visual-legacy"

# --- 偽の Messages API と一時の設定 dir ---------------------------------------------------
WORK="$TMP/work"
mkdir -p "$WORK"
FAKE_LOG="$TMP/fake.log"
: > "$FAKE_LOG"
FAKE_LOG="$FAKE_LOG" node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 > "$TMP/fake.out" 2>&1 &
FAKE_PID=$!
PORT=""
for _ in $(seq 1 50); do
  PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$TMP/fake.out" 2>/dev/null)"
  [ -n "$PORT" ] && break
  sleep 0.1
done
[ -n "$PORT" ] || { echo "偽 API が立たない"; cat "$TMP/fake.out"; exit 1; }
API_URL="http://127.0.0.1:${PORT}"
# ダミーの API キー（本物ではない。一時の .claude.json で末尾 20 字を承認済みにする）
API_KEY="sk-ant-api03-fake2159fake2159fake2159fake2159fake2159fake2159AA"
CCD="$TMP/cfg"
mkdir -p "$CCD"
jq -n --arg key "$(printf '%s' "$API_KEY" | tail -c 20)" --arg work "$WORK" --arg bin "$TAKO_BIN" '{
  hasCompletedOnboarding: true, theme: "dark",
  customApiKeyResponses: { approved: [$key], rejected: [] },
  projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } },
  mcpServers: { tako: { type: "stdio", command: $bin, args: ["mcp", "serve"], env: {} } }
}' > "$CCD/.claude.json"

# 偽の pbcopy（受け取った文字列をファイルへ。本物のクリップボードへ書かない）
FAKEBIN="$TMP/fakebin"
mkdir -p "$FAKEBIN"
printf '#!/bin/sh\ncat > %s/pbcopy.out\n' "$TMP" > "$FAKEBIN/pbcopy"
chmod +x "$FAKEBIN/pbcopy"

# 試すカード: すぐには終わらないコマンド（実行中の押し直しを見る）と、157 字・改行なしの長いコマンド（Copy）
RUN_CMD="printf 'click-2159\\n'; sleep 4"
LONG='cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long-data-dir-name ./target/debug/tako mod --json | jq ".panes[] | {pane, reason}"'
CARD_JSON="$(jq -cn --arg a "$RUN_CMD" '{commands: [$a], label: "click 2159"}')"

# --- 隔離 GUI -------------------------------------------------------------------------
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET-gui"
export TAKO_ISOLATED=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
tk() { "$TAKO_BIN" "$@"; }

start_app() {
  local name="$1"
  shift
  ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-0,0,1600,1000}"
  launch_isolated_gui "$TMP/app-${name}.log" TAKO_PERSIST=0 CLAUDE_CONFIG_DIR="$CCD" \
    ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  # 自動リネームは GUI の env の claude を呼ぶ（偽 API へ向けてあるが、測定に混ぜない）
  tk autorename off >/dev/null 2>&1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

mod_json() { tk mod --json 2>/dev/null; }
row_field() { mod_json | jq -r --argjson p "$1" ".panes[] | select(.pane == \$p) | $2" 2>/dev/null; }
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}
root_pane() { tk list 2>/dev/null | jq -r '.tabs[0].panes[0].id'; }
pane_count() { tk list 2>/dev/null | jq '[.tabs[].panes[]] | length'; }
rows_of() { tk list 2>/dev/null | jq -r --argjson p "$1" '.tabs[].panes[] | select(.id == $p) | .rows'; }
reporting() { [ "$(row_field "$1" '.state')" = reporting ]; }
not_reporting() { [ "$(row_field "$1" '.state')" != reporting ]; }
mod_dir() { mod_json | jq -r '.plugin_dir'; }
read_pane() { tk read --pane "$1" 2>/dev/null; }
screen_has() { read_pane "$1" | grep -q -- "$2"; }
cards_json() { tk show-command --list --pane "$1" 2>/dev/null; }
card_owner() { cards_json "$1" | jq -r --argjson c "$2" '.cards[] | select(.id == $c) | "\(.owner.drawn_by)/\(.owner.reason)"'; }
owner_is() { [ "$(card_owner "$1" "$2")" = "$3" ]; }
rows_below() { [ "$(rows_of "$1")" -lt "$2" ] 2>/dev/null; }
run_of() { cards_json "$1" | jq -c --argjson c "$2" '.cards[] | select(.id == $c) | .runs[0] | if . == null then null else {state, exit_code, count} end'; }
pane_more_than() { [ "$(pane_count)" -gt "$1" ]; }
# GUI のペインへキーを送る（帯へのフォーカスは ctrl+x → Tab）
key() {
  tk send --pane "$1" --no-newline "$2" >/dev/null 2>&1
  sleep 0.6
}
band_press() {
  key "$1" $'\e'
  key "$1" $'\x18'
  key "$1" $'\t'
  key "$1" "$2"
}
save_pane() { read_pane "$1" | mask > "$2"; }
# 画面の文字列 $2 の中ほどのセル「行 桁」（0 始まり。全角は 2 桁）。無ければ空
cell_of() {
  read_pane "$1" | python3 -I -c '
import sys, unicodedata
label = sys.argv[1]
for row, line in enumerate(sys.stdin.read().split("\n")):
    at = line.find(label)
    if at < 0:
        continue
    width = lambda s: sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s)
    print(row, width(line[:at]) + width(label) // 2)
    break
' "$2"
}

# 隔離 GUI の直接ペインで claude を起こす（ペインの env = tako の注入はそのまま）。$2 は前に置く env、$3 は
# claude の前のコマンド（`env -u CLAUDE_CODE_PLUGIN_DIRS` = mod なし）
gui_claude() {
  local pane="$1" extra="${2:-}" pre="${3:-}"
  if reporting "$pane"; then quit_claude "$pane"; fi
  tk send --pane "$pane" "cd '$WORK' && clear && env PATH='$FAKEBIN':\$PATH CLAUDE_CONFIG_DIR='$CCD' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 $extra $pre claude --model haiku --allowedTools mcp__tako__tako_show_command" >/dev/null
  if [ -n "$pre" ]; then
    # mod なし = 報告が来ない。入力欄が出るまで待つ
    if ! wait_for 60 screen_has "$pane" '❯'; then
      fail "claude（mod なし）の入力欄が出ない"
      return 1
    fi
    sleep 2
    return 0
  fi
  if ! wait_for 90 reporting "$pane"; then
    fail "claude の報告が届かない（state=$(row_field "$pane" '.state')）"
    read_pane "$pane" | mask | tail -10 | sed 's/^/      | /'
    return 1
  fi
}
exit_claude() {
  tk send --pane "$1" --no-newline $'\x03' >/dev/null 2>&1
  sleep 0.5
  tk send --pane "$1" --no-newline $'\x03' >/dev/null 2>&1
}
quit_claude() {
  exit_claude "$1"
  wait_for 30 not_reporting "$1" || true
  sleep 1
}

# AI にカードを出させる（偽 API が tako_show_command を呼ぶ）。出たカードの id を CARD に
show_card() {
  local pane="$1" json="$2"
  local before after
  before="$(cards_json "$pane" | jq -r '[.cards[].id] | max // 0')"
  tk send --pane "$pane" --no-newline "Show this card: $json" >/dev/null
  sleep 1
  tk send --pane "$pane" --no-newline $'\r' >/dev/null
  if ! wait_for 40 screen_has "$pane" 'CARD-DONE'; then
    fail "AI の tako_show_command が終わらない"
    read_pane "$pane" | mask | tail -12 | sed 's/^/      | /'
    CARD=""
    return 1
  fi
  after="$(cards_json "$pane" | jq -r '[.cards[].id] | max // 0')"
  if [ "$after" = "$before" ] || [ "$after" = 0 ]; then
    fail "tako の store にカードが増えない"
    CARD=""
    return 1
  fi
  CARD="$after"
}

# 後片付け: カードの実行ペインを閉じ、カードを閉じる
close_runs() {
  local p="$1" rp
  for rp in $(cards_json "$p" | jq -r '.cards[].runs[]? | select(. != null) | .pane'); do
    tk close --pane "$rp" --force >/dev/null 2>&1
  done
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
}

# --- static ---------------------------------------------------------------------------------
phase_static() {
  echo "== static: claude plugin validate --strict / test（一時の設定 dir・mod の写し）"
  local cfg="$TMP/cfg-static" m="$TMP/mod-static" out
  mkdir -p "$cfg"
  /bin/cp -R "$REPO_ROOT/crates/tako-core/claude-mod" "$m"
  rm -rf "$m/.claude-plugin/types"
  out="$(cd "$TMP" && env CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 claude plugin validate --strict "$m" 2>&1)"
  if printf '%s' "$out" | grep -q 'Validation passed'; then pass "validate --strict"; else fail "validate --strict"; printf '%s\n' "$out" | mask | tail -15; fi
  out="$(cd "$TMP" && env CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 claude plugin test "$m" 2>&1)"
  if printf '%s' "$out" | grep -Eq '^ *0 fail'; then
    pass "claude plugin test（$(printf '%s' "$out" | grep -E '^ *[0-9]+ pass' | tr -d ' ')。cards.test.ts は terminal / desktop の両 surface）"
  else
    fail "claude plugin test"
    printf '%s\n' "$out" | grep -v '^(pass)' | mask | tail -30
  fi
}

# --- 1. steps（手順の A/B）-------------------------------------------------------------------
# 新: カードの [ run ] を 1 回クリック → 新しいペイン + 実行記録。操作の数を STEPS に
steps_new() {
  local p="$1" capdir="$2" before cell
  before="$(pane_count)"
  cell="$(cell_of "$p" '[ run ]')"
  save_pane "$p" "$capdir/steps-before.txt"
  if [ -z "$cell" ]; then
    fail "会話の行のカードに [ run ] が無い"
    return 1
  fi
  pass "会話の行のカードに [ run ]（行 桁 = ${cell}）・帯の r: run も残る（$(read_pane "$p" | grep -c 'r: run') 行）"
  # Claude Code は描かれて 300 ms 以内のクリックを捨てる（mount の直後を避ける）
  sleep 2
  # shellcheck disable=SC2086
  set -- $cell
  tk click --pane "$p" --row "$1" --col "$2" >/dev/null || fail "tako click が通らない"
  STEPS=1
  if wait_for 15 pane_more_than "$before"; then
    pass "クリック 1 回で新しいペインが立った（$before → $(pane_count)）"
  else
    fail "クリック 1 回では新しいペインが立たない"
    return 1
  fi
}
# 旧: カードの行をクリックしても何も起きない → 帯の ctrl+x → Tab → r で実行。操作の数を STEPS に
steps_legacy() {
  local p="$1" capdir="$2" before cell
  before="$(pane_count)"
  save_pane "$p" "$capdir/steps-before.txt"
  check_eq "旧: 会話の行のカードに [ run ] が無い" "" "$(cell_of "$p" '[ run ]')"
  # 新しい版で [ run ] が出る所（カードの案内の行）をクリックしても何も起きない
  cell="$(cell_of "$p" 'in the band (ctrl+x')"
  if [ -n "$cell" ]; then
    sleep 2
    # shellcheck disable=SC2086
    set -- $cell
    tk click --pane "$p" --row "$1" --col "$2" >/dev/null
    sleep 3
    check_eq "旧: カードの行をクリックしても実行されない（ペイン数そのまま）" "$before" "$(pane_count)"
  else
    unmeasured "旧: カードの案内の行が見つからない"
  fi
  band_press "$p" r
  STEPS=3
  if wait_for 15 pane_more_than "$before"; then
    pass "旧: 帯の ctrl+x → Tab → r（3 打。入力欄へ戻す Esc を除く）で初めて実行された"
  else
    fail "旧: 帯のキーで Run が押せない"
    return 1
  fi
}
steps_arm() {
  local arm="$1" capdir="$2" p run before_runs
  p="$(root_pane)"
  gui_claude "$p" || return 1
  show_card "$p" "$CARD_JSON" || return 1
  wait_for 15 owner_is "$p" "$CARD" mod/drawn || true
  wait_for 10 screen_has "$p" "card ${CARD}" || true
  check_eq "${arm}: 持ち主は mod（会話の行と帯に描いた）" "mod/drawn" "$(card_owner "$p" "$CARD")"
  if [ "$arm" = new ]; then steps_new "$p" "$capdir" || return 1; else steps_legacy "$p" "$capdir" || return 1; fi
  wait_for 10 sh -c "'$TAKO_BIN' show-command --list --pane $p | jq -e '.cards[] | select(.id == $CARD) | .runs[0] != null' >/dev/null" || true
  run="$(run_of "$p" "$CARD")"
  echo "    観測: ${arm} の実行記録 ${run}・操作 ${STEPS} 回"
  check_eq "${arm}: 実行記録は 1 回（show-command --list の runs[0].count = スマホ・GPUI のカードと同じ記録）" "1" "$(printf '%s' "$run" | jq -r '.count')"
  local rp title
  rp="$(cards_json "$p" | jq -r --argjson c "$CARD" '.cards[] | select(.id == $c) | .runs[0].pane')"
  title="$(tk list | jq -r --argjson q "$rp" '.tabs[].panes[] | select(.id == $q) | .title // ""')"
  check_eq "${arm}: 実行ペインのタイトルは GPUI のカードの実行と同じ (>) 接頭辞" "true" "$(case "$title" in '(>) '*) echo true ;; *) echo false ;; esac)"
  check_eq "${arm}: 実行ペインは手元のペインと同じタブ" "true" \
    "$(tk list | jq --argjson a "$p" --argjson b "$rp" '[.tabs[] | select([.panes[].id] | (index($a) != null and index($b) != null))] | length == 1')"
  wait_for 20 sh -c "'$TAKO_BIN' show-command --list --pane $p | jq -e '.cards[] | select(.id == $CARD) | .runs[0].state == \"exited\"' >/dev/null" || true
  check_eq "${arm}: 実行が終わって exited 0" '{"state":"exited","exit_code":0,"count":1}' "$(run_of "$p" "$CARD")"
  save_pane "$p" "$capdir/steps-after.txt"
  eval "STEPS_${arm}=$STEPS"
  close_runs "$p"
  quit_claude "$p"
}
phase_steps() {
  echo "== 1. 手順の A/B: 新 = カードの [ run ] をクリック 1 回 / 旧（TAKO_2159_LEGACY=1）= 帯の ctrl+x → Tab → r"
  STEPS_new=""
  STEPS_legacy=""
  steps_arm new "$OUT/capture-new"
  stop_app
  start_app steps-legacy TAKO_2159_LEGACY=1
  check_eq "A/B: tako mod の cards.click は偽・click_legacy は真" "false/true" "$(mod_json | jq -r '"\(.cards.click)/\(.cards.click_legacy)"')"
  steps_arm legacy "$OUT/capture-legacy"
  stop_app
  start_app main
  echo "    観測: カードが出てから実行されるまでのユーザーの操作 = 新 ${STEPS_new:-?} 回（クリック）/ 旧 ${STEPS_legacy:-?} 回（ctrl+x → Tab → r）"
  check_eq "手順の差: 新はクリック 1 回・旧はキー 3 打" "1/3" "${STEPS_new:-?}/${STEPS_legacy:-?}"
}

# --- 3. mouseoff ----------------------------------------------------------------------------
phase_mouseoff() {
  echo "== 3. tako mouse off（クリックを渡さない）: 会話の行にボタンを置かず、帯のホットキーで今どおり押せる"
  local p before
  p="$(root_pane)"
  tk mouse off >/dev/null || fail "tako mouse off が通らない"
  check_eq "tako mod の cards.click は偽" "false" "$(mod_json | jq -r '.cards.click')"
  gui_claude "$p" || { tk mouse on >/dev/null; return 1; }
  show_card "$p" "$CARD_JSON" || { tk mouse on >/dev/null; return 1; }
  wait_for 15 owner_is "$p" "$CARD" mod/drawn || true
  wait_for 10 screen_has "$p" "card ${CARD}" || true
  save_pane "$p" "$OUT/capture-new/mouseoff.txt"
  check_eq "会話の行に [ run ] / [ copy ] を置かない" "/" "$(cell_of "$p" '[ run ]')/$(cell_of "$p" '[ copy ]')"
  check_eq "会話の行の案内は帯のキー（#1963 の文言）" "true" "$(screen_has "$p" 'in the band (ctrl+x → Tab): y: copy / r: run' && echo true || echo false)"
  before="$(pane_count)"
  band_press "$p" r
  if wait_for 15 pane_more_than "$before"; then pass "帯の ctrl+x → Tab → r で新しいペインが立った"; else fail "帯のキーで Run が押せない"; fi
  wait_for 10 sh -c "'$TAKO_BIN' show-command --list --pane $p | jq -e '.cards[] | select(.id == $CARD) | .runs[0] != null' >/dev/null" || true
  check_eq "実行記録は 1 回" "1" "$(run_of "$p" "$CARD" | jq -r '.count')"
  close_runs "$p"
  quit_claude "$p"
  tk mouse on >/dev/null
  check_eq "tako mouse on で cards.click が真に戻る" "true" "$(mod_json | jq -r '.cards.click')"
}

# --- 4. place / mod なし ---------------------------------------------------------------------
place_check() {
  local label="$1" p="$2" rows0="$3" reason="$4"
  if wait_for 10 rows_below "$p" "$rows0"; then pass "${label}: GPUI のカード帯が出た（行数 ${rows0} → $(rows_of "$p")）"; else fail "${label}: GPUI のカード帯が出ない"; fi
  sleep 3
  save_pane "$p" "$OUT/capture-new/${label}.txt"
  check_eq "${label}: 持ち主は tako（${reason}）" "tako/${reason}" "$(card_owner "$p" "$CARD")"
  check_eq "${label}: 会話の行に [ run ] を置かない" "" "$(cell_of "$p" '[ run ]')"
}
phase_place() {
  echo "== 4. cards.place=tako と mod なしのペインは今の挙動（GPUI のカード帯・会話の行にボタンなし）"
  local p rows0
  p="$(root_pane)"
  tk mod ui set cards.place tako >/dev/null || fail "tako mod ui set cards.place tako が通らない"
  gui_claude "$p" || return 1
  rows0="$(rows_of "$p")"
  show_card "$p" "$CARD_JSON" || return 1
  place_check place-tako "$p" "$rows0" place_tako
  check_eq "place-tako: 報告に renders.cards を載せない（描かせていない）" "null" "$(row_field "$p" '.report.renders.cards')"
  close_runs "$p"
  quit_claude "$p"
  tk mod ui reset >/dev/null 2>&1
  # mod なし（ペインの CLAUDE_CODE_PLUGIN_DIRS を外す = mod が読まれず報告も来ない）
  gui_claude "$p" "" "env -u CLAUDE_CODE_PLUGIN_DIRS" || return 1
  rows0="$(rows_of "$p")"
  show_card "$p" "$CARD_JSON" || return 1
  place_check no-mod "$p" "$rows0" no_fresh_report
  close_runs "$p"
  exit_claude "$p"
  sleep 2
}

# --- copy（隔離 tmux）--------------------------------------------------------------------------
start_tmux_claude() {
  local name="$1" pane="$2" dirs="$3"
  tmx kill-session -t "=$name" >/dev/null 2>&1
  tmx new-session -d -s "$name" -x 144 -y 40 -c "$WORK" \
    env PATH="$FAKEBIN:$PATH" TAKO_ISOLATED=1 TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" \
    TAKO_ORCHESTRATOR_DIR="$TAKO_ORCHESTRATOR_DIR" CLAUDE_CONFIG_DIR="$CCD" \
    ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    TAKO_PANE_ID="$pane" TAKO_CLI="$TAKO_BIN" CLAUDE_CODE_PLUGIN_DIRS="$dirs" \
    claude --model haiku --allowedTools mcp__tako__tako_show_command
  # tmux へ届く OSC 52 をどこへも渡さない（クライアントも付けない）
  tmx set-option -g set-clipboard off >/dev/null 2>&1
  tmx set-option -t "=$name" remain-on-exit off >/dev/null 2>&1
}
tcap() { tmx capture-pane -p -t "=$1:" 2>/dev/null; }
tscreen_has() { tcap "$1" | grep -q -- "$2"; }
tcell_of() {
  tcap "$1" | python3 -I -c '
import sys, unicodedata
label = sys.argv[1]
for row, line in enumerate(sys.stdin.read().split("\n")):
    at = line.find(label)
    if at < 0:
        continue
    width = lambda s: sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s)
    print(row, width(line[:at]) + width(label) // 2)
    break
' "$2"
}
# SGR のクリック（押す・離す）を tmux の器のペインへ直接（#1961 の器のペインと同じ send-keys -H）
tclick() {
  local s="$1" row="$2" col="$3" seq hex
  for seq in "$(printf '\033[<0;%d;%dM' $((col + 1)) $((row + 1)))" "$(printf '\033[<0;%d;%dm' $((col + 1)) $((row + 1)))"; do
    hex="$(printf '%s' "$seq" | od -An -tx1 | tr -s ' \n' ' ')"
    # shellcheck disable=SC2086
    tmx send-keys -t "=$s:" -H $hex
    sleep 0.05
  done
}
phase_copy() {
  echo "== 会話の行の [ copy ] をクリックで押すと論理文字列（157 字・改行なし）が \$.ui.copy へ（隔離 tmux）"
  local p dirs s cell copied
  p="$(root_pane)"
  dirs="$(mod_dir)"
  s="copy"
  start_tmux_claude "$s" "$p" "$dirs"
  if ! wait_for 60 tscreen_has "$s" '❯'; then
    fail "tmux の claude の入力欄が出ない"
    return 1
  fi
  sleep 3
  tmx send-keys -t "=$s:" -l "Show this card: $(jq -cn --arg a "$LONG" '{commands: [$a], label: "copy 2159"}')"
  sleep 1
  tmx send-keys -t "=$s:" Enter
  if ! wait_for 40 tscreen_has "$s" 'CARD-DONE'; then
    fail "tmux の claude: AI の tako_show_command が終わらない"
    tcap "$s" | mask | tail -10 | sed 's/^/      | /'
    return 1
  fi
  if ! wait_for 30 tscreen_has "$s" '\[ copy \]'; then
    fail "tmux の claude: 会話の行のカードに [ copy ] が出ない"
    tcap "$s" | mask | tail -14 | sed 's/^/      | /'
    return 1
  fi
  sleep 2
  cell="$(tcell_of "$s" '[ copy ]')"
  tcap "$s" | mask > "$OUT/capture-new/copy.txt"
  rm -f "$TMP/pbcopy.out"
  # shellcheck disable=SC2086
  set -- $cell
  tclick "$s" "$1" "$2"
  wait_for 10 test -f "$TMP/pbcopy.out" || true
  copied="$(cat "$TMP/pbcopy.out" 2>/dev/null)"
  check_eq "[ copy ] のクリックで渡った文字列は論理文字列そのまま" "$LONG" "$copied"
  check_eq "長さ 157 字・改行 0" "157 0" "$(printf '%s' "$copied" | wc -m | tr -d ' ') $(printf '%s' "$copied" | tr -cd '\n' | wc -c | tr -d ' ')"
  tmx kill-session -t "=$s" >/dev/null 2>&1
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
  wait_for 30 not_reporting "$p" || true
}

# --- visual ---------------------------------------------------------------------------------
run_visual() {
  local arm="$1" dump="$2"
  shift 2
  start_app "visual-$arm" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=mod-card-click TAKO_VISUAL_DUMP_DIR="$dump" ${@+"$@"}
  local p
  p="$(root_pane)"
  if gui_claude "$p"; then
    show_card "$p" "$CARD_JSON" || true
  fi
  local rc=0
  wait "$APP_PID" || rc=$?
  APP_PID=""
  return $rc
}
phase_visual() {
  echo "== 1 / 2. visual-test mod-card-click: 実 OS と同じクリック 1 回で実行・2 打目と実行中の押し直しで 2 本目が走らない"
  local rc=0 log="$TMP/app-visual-new.log"
  run_visual new "$OUT/visual-new" || rc=$?
  grep 'TAKO_VISUAL_2159\|TAKO_APP_SELF_TEST_FAILED' "$log" | mask | sed 's/^/    観測: /' | cut -c1-400
  check_eq "節が終了コード 0 で終わる（①〜④）" "0" "$rc"
  check_eq "絵を 4 枚書き出した（前 / 後 / 断った / 終わった）" "4" "$(ls "$OUT/visual-new"/mod-card-click-*.png 2>/dev/null | wc -l | tr -d ' ')"
  echo "== A/B: TAKO_2159_LEGACY=1（同一バイナリ）は ① で落ちる"
  rc=0
  log="$TMP/app-visual-legacy.log"
  run_visual legacy "$OUT/visual-legacy" TAKO_2159_LEGACY=1 || rc=$?
  grep 'TAKO_VISUAL_2159\|TAKO_APP_SELF_TEST_FAILED' "$log" | mask | sed 's/^/    観測: /' | cut -c1-400
  check_eq "A/B: 節が非ゼロで終わる" "true" "$([ "$rc" -ne 0 ] && echo true || echo false)"
  check_eq "A/B: 落ちた理由は会話の行のカードに [ run ] が無いこと（①）" "true" \
    "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test mod-card-click ①' "$log" && echo true || echo false)"
}

# --- 実行 ----------------------------------------------------------------------------------------
echo "claude: $(claude --version 2>/dev/null | head -1) / 作業 dir: $(printf '%s' "$TMP" | mask)"
want static && phase_static
if want steps || want mouseoff || want place || want copy; then
  start_app main
  echo "  隔離 GUI: pid $APP_PID / mod: $(mod_json | jq -c '{injecting, claude_version, cards}' | mask)"
  want steps && phase_steps
  want mouseoff && phase_mouseoff
  want place && phase_place
  want copy && phase_copy
  stop_app
fi
want visual && phase_visual

REAL_AFTER="$(snap_real)"
check_eq "本物の ~/.claude*/skills・plugins は前後で同じ（${REAL_BEFORE}）" "$REAL_BEFORE" "$REAL_AFTER"
check_eq "本物のクリップボードは前後で同じ（中身は読まずハッシュだけ）" "$CLIP_BEFORE" "$(snap_clipboard)"
echo "採取: $(printf '%s' "$OUT" | mask)/capture-{new,legacy}・絵: $(printf '%s' "$OUT" | mask)/visual-{new,legacy}"
find "$OUT" -name '*.png' | sort | mask | sed 's/^/  /'
echo "== 結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
