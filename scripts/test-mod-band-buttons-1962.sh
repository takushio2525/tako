#!/usr/bin/env bash
# test-mod-band-buttons-1962.sh — tako mod S7-3（#1962。FR-2.42.34〜）の実経路テスト
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）を**一時の設定 dir** と**偽の Messages API**
# （scripts/lib/fake-messages-api.mjs）で動かし、隔離 GUI（tako-vd・TAKO_PERSIST=0・自動リネーム off）の
# ペインとして報告させて、画面の capture と `tako mod` / `tako limit-service --refresh` で確かめる。
# 番号は Issue #1962 の受け入れ条件:
#   widths  2. 80 / 144 / 300 桁・21 行で tako の行がボタン込みで 1 行（隔離 tmux の -x / -y）
#   coexist 1. 帯を描く他の mod と並べて両方の行が出る（読み込み順を入れ替えても）。next を呼ばない
#              他の mod が外側に居ると tako の帯は隠れ、`tako mod` の行に hidden_by_other_mod が出る
#   buttons 3. /compact のボタンをキー（ctrl+x → Tab → c）で押すと会話が圧縮され、tako の操作の
#              ボタン（split right）でペインが増える。押した結果（種類と成否）が報告に載る
#   bar     4. バーが PromptHint に出て、同じときにステータスバーから claude の区画が消える
#              （`tako limit-service --refresh` の claude.status_bar.drawn_by = mod）。報告を止める
#              （mod が叩く CLI の `mod report` だけを失敗させる）と 45 秒で tako の表示へ戻り、戻すと mod へ戻る
#   status  5. statusLine で ctx を出している構成ではバーを描かない（隔離 GUI の直接ペイン = tako が
#              画面を読める経路。数値を出さない statusLine では描く）
#   ab      6. A/B: TAKO_1877_S7_LEGACY=1 で S3 の描き方へ戻る（他の mod の帯を包まない・ボタンとバーなし・
#              ステータスバーは止めない）
#   visual  4 の絵: visual-test `mod-bar`（実ピクセル。新旧で書き出し先を分ける）
#   static  7. `claude plugin validate --strict` / `claude plugin test`（terminal と desktop の両 surface）
#
# **本物の設定 dir（~/.claude*）には書かない**: claude の設定 dir は mktemp 配下（認証は写さず、偽 API と
# ダミーの API キーを一時の .claude.json で承認済みにする）。前後で本物の ~/.claude*/skills・plugins の
# 一覧とハッシュを比べる。本番 tako が配る env（TAKO_SOCKET / TAKO_PANE_ID / TAKO_CLI /
# CLAUDE_CODE_PLUGIN_DIRS 等）は最初に落とす。落とすのは自分で起こした pid と tmux ソケットだけ。
# 画面に出る claude はシェルの rc が CLAUDE_CONFIG_DIR を書き換えても効くように `env` で起動する。
#
# 使い方: bash scripts/test-mod-band-buttons-1962.sh   （ONLY=widths|coexist|buttons|bar|status|ab|visual|static）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は mod の `claude plugin test`・
# Rust の単体テストと番犬（issue1962_mod_band_buttons_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_1877_S3_LEGACY \
  TAKO_1877_S7_LEGACY TAKO_1881_BAND_THRESHOLD TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
# 自分の claude が使っている本物の設定 dir・認証を混ぜない
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
# 部分文字列を含むか（true / false）。`$( case … ) … )` は bash 3.2 が `)` で読み違えるので関数に出す
contains() {
  case "$1" in
    *"$2"*) echo true ;;
    *) echo false ;;
  esac
}

REAL_CLAUDE="$(command -v claude 2>/dev/null || true)"
if [ -z "$REAL_CLAUDE" ]; then
  echo "未実測: PATH に claude が無い"
  exit 3
fi
command -v node >/dev/null 2>&1 || { echo "未実測: node が無い（偽 API に使う）"; exit 3; }
command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 3; }

# --- 本物の設定 dir の前後比較（skills / plugins の一覧とハッシュ。#1959 と同じ物差し）----------
REAL_NOISE='/\.in_use/|/synced/|/\.last_inuse_sweep$|/install-counts-cache\.json$'
snap_real() {
  local d
  for d in "$HOME"/.claude*/skills "$HOME"/.claude*/plugins; do
    [ -d "$d" ] || continue
    find "$d" -type f -print0 2>/dev/null | sort -z | xargs -0 shasum -a 256 2>/dev/null
    find "$d" \( -type d -o -type l \) 2>/dev/null | sort
  done | grep -Ev "$REAL_NOISE" | shasum -a 256 | cut -c1-16
}
REAL_BEFORE="$(snap_real)"

# --- ビルド（visual-test の節を含む GUI と CLI。rebase 後の古いバイナリを掴まない）------------
(cd "$REPO_ROOT" && cargo build -q -p tako-app --features visual-test && cargo build -q -p tako-cli) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

TMP="$(mktemp -d /tmp/tako-1962-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
FAKE_PID=""
TMUX_SOCKET="tako-1962-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }
tmx() { tmux -u -L "$TMUX_SOCKET" "$@"; }

cleanup() {
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmx kill-server >/dev/null 2>&1 || true
  # GUI 側の器（TAKO_PERSIST=0 なので普段は立たない。立っていても自分のソケットだけ）
  tmux -L "$TMUX_SOCKET-gui" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET" "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET-gui"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# 画面の採取の書き出し先（新しい挙動と旧挙動で分ける）
CAP_NEW="$TMP/capture-new"
CAP_OLD="$TMP/capture-legacy"
DUMP_NEW="$TMP/visual-new"
DUMP_OLD="$TMP/visual-legacy"
mkdir -p "$CAP_NEW" "$CAP_OLD" "$DUMP_NEW" "$DUMP_OLD"

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
API_KEY="sk-ant-api03-fake1962fake1962fake1962fake1962fake1962fake1962AA"
# 設定 dir: 素（statusLine なし）・statusLine が ctx / 5h を出す・statusLine が数値を出さない
make_config() {
  local dir="$1" status_cmd="${2:-}" last20
  last20="$(printf '%s' "$API_KEY" | tail -c 20)"
  mkdir -p "$dir"
  jq -n --arg key "$last20" --arg work "$WORK" '{
    hasCompletedOnboarding: true, theme: "dark",
    customApiKeyResponses: { approved: [$key], rejected: [] },
    projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
  }' > "$dir/.claude.json"
  if [ -n "$status_cmd" ]; then
    jq -n --arg cmd "$status_cmd" '{ statusLine: { type: "command", command: $cmd } }' > "$dir/settings.json"
  fi
}
CCD="$TMP/cfg"
CCD_SL="$TMP/cfg-statusline"
CCD_SL_PLAIN="$TMP/cfg-statusline-plain"
make_config "$CCD"
make_config "$CCD_SL" "printf 'ctx 45%% 5h 12%%'"
make_config "$CCD_SL_PLAIN" "printf 'main*'"

# --- 他の mod の代役（帯に自分の行を描く。wrap = next を包む / greedy = next を呼ばない）-------------
make_other_mod() {
  local name="$1" mode="$2" dir="$TMP/mods/$1"
  mkdir -p "$dir/.claude-plugin" "$dir/hooks"
  printf '{"name":"%s","version":"0.0.1","description":"band of another mod for the #1962 check"}\n' "$name" \
    > "$dir/.claude-plugin/plugin.json"
  printf '{ "modules": ["./register.ts"] }\n' > "$dir/hooks/hooks.json"
  local label
  label="$(printf '%s' "$name" | tr '[:lower:]-' '[:upper:]_')_BAND"
  if [ "$mode" = wrap ]; then
    cat > "$dir/hooks/register.ts" <<EOF
import type { Register } from 'claude-code'
export const register: Register = on => {
  on('ui.render', { component: 'AbovePrompt' }, async (\$, e, next) => {
    if (e.props.hasSurvey) return next(e)
    const below = await next(e)
    const { Box, Text } = \$.ui.resolve(e)
    const row = Text({ children: '${label}' })
    return below.type === 'engine' ? row : Box({ flexDirection: 'column', children: [row, below] })
  }).catch((\$, e, next) => next(e))
}
EOF
  else
    cat > "$dir/hooks/register.ts" <<EOF
import type { Register } from 'claude-code'
export const register: Register = on => {
  on('ui.render', { component: 'AbovePrompt' }, async (\$, e, next) => {
    if (e.props.hasSurvey) return next(e)
    return \$.ui.resolve(e).Text({ children: '${label}' })
  }).catch((\$, e, next) => next(e))
}
EOF
  fi
  printf '%s' "$dir"
}
OTHER_WRAP="$(make_other_mod other-wrap wrap)"
OTHER_GREEDY="$(make_other_mod other-greedy greedy)"

# mod が叩く CLI（TAKO_CLI）の薄い包み: 合図のファイルがある間だけ `mod report` を失敗させる
# （= 報告が途切れた状態。claude は生かしたまま・ボタンの押下は通す）。`kill -STOP` は 2.1.294 の
# claude がすぐ起き直すので止める手段にならない（実測: 5 秒後に STAT が S へ戻り heartbeat が続いた）
PAUSE_REPORTS="$TMP/pause-reports"
CLI_GATE="$TMP/tako-cli-gate"
cat > "$CLI_GATE" <<EOF
#!/bin/sh
if [ -e '$PAUSE_REPORTS' ] && [ "\$1" = mod ] && [ "\$2" = report ]; then
  exit 1
fi
exec '$TAKO_BIN' "\$@"
EOF
chmod +x "$CLI_GATE"

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
  # 窓は大きめ（直接ペインの claude が入力欄の下の行と statusLine まで描ける高さ）
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
pane_ids() { tk list 2>/dev/null | jq -r '.tabs[].panes[].id' | sort; }
# p を分割し、新しく生えたペインの ID を出す（split の出力の形に依らない = 一覧の差で取る）
split_new() {
  local before after
  before="$(pane_ids)"
  tk split --pane "$1" "$2" >/dev/null 2>&1
  after="$(pane_ids)"
  comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -1
}
reporting() { [ "$(row_field "$1" '.state')" = reporting ]; }
mod_dir() { mod_json | jq -r '.plugin_dir'; }

# tako のペインと同じ env で claude を隔離 tmux のセッションに立てる（設定 dir と API は一時のもの）
start_claude() {
  local name="$1" width="$2" rows="$3" pane="$4" dirs="$5" ccd="${6:-$CCD}"
  tmx kill-session -t "=$name" >/dev/null 2>&1
  tmx new-session -d -s "$name" -x "$width" -y "$rows" -c "$WORK" \
    env TAKO_ISOLATED=1 TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" \
    TAKO_ORCHESTRATOR_DIR="$TAKO_ORCHESTRATOR_DIR" CLAUDE_CONFIG_DIR="$ccd" \
    ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    TAKO_PANE_ID="$pane" TAKO_CLI="$CLI_GATE" CLAUDE_CODE_PLUGIN_DIRS="$dirs" \
    claude --model haiku
  tmx set-option -t "=$name" remain-on-exit off >/dev/null 2>&1
}
# claude を落とし、そのペインの報告が消える（session.end の最後の報告 か 45 秒の失効）まで待つ
# = 次の段の「報告が届いた」を前の claude の報告で早まらせない
stop_claude() {
  tmx kill-session -t "=$1" >/dev/null 2>&1
  local pane
  pane="$(root_pane)"
  wait_for 50 not_reporting "$pane" || true
}
not_reporting() { [ "$(row_field "$1" '.state')" != reporting ]; }
cap() { tmx capture-pane -p -t "=$1:" 2>/dev/null; }
save_cap() { cap "$1" | mask > "$2"; }
screen_has() { cap "$1" | grep -q -- "$2"; }
key() { tmx send-keys -t "=$1:" "$2"; }
type_line() {
  tmx send-keys -t "=$1:" -l "$2"
  sleep 1
  tmx send-keys -t "=$1:" Enter
}
# 1 ターン回す（偽 API の答えの語が画面に出るまで）
one_turn() {
  type_line "$1" "Reply with exactly one word: $2"
  wait_for 60 screen_has "$1" "$2"
}
# 帯の tako の行（`tako | ` を含む行）
band_lines() { cap "$1" | grep -c 'tako | ' | tr -d ' '; }
band_line() { cap "$1" | grep 'tako | ' | head -1; }
# バー（`<ラベル> <棒> <N>%`）が画面に出ているか
bar_on_screen() { cap "$1" | grep -Eq '(5h|7d|ctx) [▁▂▃▄▅▆▇█] [0-9]+%'; }
bar_gone() { ! bar_on_screen "$1"; }
wait_claude() {
  local session="$1" pane="$2"
  if ! wait_for 90 reporting "$pane"; then
    fail "claude（${session}）の報告が届かない（state=$(row_field "$pane" '.state')）"
    cap "$session" | mask | tail -15 | sed 's/^/      | /'
    return 1
  fi
}

# --- static: validate / test ---------------------------------------------------------------
phase_static() {
  echo "== 7. claude plugin validate --strict / test（一時の設定 dir・mod の写し）"
  local cfg="$TMP/cfg-static" m="$TMP/mod-static" out
  mkdir -p "$cfg"
  /bin/cp -R "$REPO_ROOT/crates/tako-core/claude-mod" "$m"
  rm -rf "$m/.claude-plugin/types"
  out="$(cd "$TMP" && env CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 claude plugin validate --strict "$m" 2>&1)"
  if printf '%s' "$out" | grep -q 'Validation passed'; then pass "validate --strict"; else fail "validate --strict"; printf '%s\n' "$out" | mask | tail -15; fi
  out="$(cd "$TMP" && env CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 claude plugin test "$m" 2>&1)"
  if printf '%s' "$out" | grep -Eq '^ *0 fail'; then
    pass "claude plugin test（$(printf '%s' "$out" | grep -E '^ *[0-9]+ pass' | tr -d ' ')。s7.test.ts は terminal / desktop の両 surface）"
  else
    fail "claude plugin test"
    printf '%s\n' "$out" | grep -v '^(pass)' | mask | tail -30
  fi
}

# --- 2. widths ------------------------------------------------------------------------------
phase_widths() {
  echo "== 2. 80 / 144 / 300 桁・21 行で tako の行がボタン込みで 1 行"
  local p dirs w s line cells
  p="$(root_pane)"
  dirs="$(mod_dir)"
  # 帯を混ませる: 長いペイン名・タブ名・ボタン 4 つ（既定の compact + tako の操作 + 組み込み + prompt）
  tk title --pane "$p" "orchestrator-master-pane-with-a-long-name" >/dev/null
  tk tab rename --tab "$(tk list | jq -r '.tabs[0].id')" "tako-wt-1962-working-tab-long" >/dev/null 2>&1
  tk mod ui button add split-right >/dev/null
  tk mod ui button add context >/dev/null
  tk mod ui button add prompt "please continue" --label "go on" >/dev/null
  for w in 80 144 300; do
    s="w$w"
    start_claude "$s" "$w" 21 "$p" "$dirs"
    wait_claude "$s" "$p" || continue
    if ! wait_for 30 screen_has "$s" 'tako | '; then
      fail "${w} 桁: 帯の tako の行が出ない"
      save_cap "$s" "$CAP_NEW/width-$w.txt"
      continue
    fi
    sleep 2
    save_cap "$s" "$CAP_NEW/width-$w.txt"
    line="$(band_line "$s" | sed 's/ *$//')"
    cells="$(printf '%s' "$line" | wc -m | tr -d ' ')"
    check_eq "${w} 桁: tako の行は 1 本" "1" "$(band_lines "$s")"
    check_eq "${w} 桁: ボタン（[ compact ]）が tako の行と同じ行にある" "true" "$(contains "$line" '[ compact ]')"
    check_eq "${w} 桁: 帯が畳まれていない（↓ N more が無い）" "false" \
      "$(cap "$s" | grep -q 'more' && echo true || echo false)"
    if [ "$cells" -le "$w" ]; then pass "${w} 桁: 行の桁 ${cells} ≤ ${w}"; else fail "${w} 桁: 行の桁 ${cells} > ${w}"; fi
    echo "    観測: $(printf '%s' "$line" | mask | cut -c1-200)"
    echo "    報告: band=$(row_field "$p" '.report.band | {columns, segments}' | tr -d '\n ') renders=$(row_field "$p" '.report.renders' | tr -d '\n ')"
    stop_claude "$s"
    sleep 1
  done
  tk mod ui reset >/dev/null 2>&1
}

# --- 1. coexist -----------------------------------------------------------------------------
# 読み込み順は CLAUDE_CODE_PLUGIN_DIRS の並び（先が外側 = 設計書 §9.5）
coexist_one() {
  local label="$1" dirs="$2" other="$3" other_shown="$4" want_tako="$5" capdir="$6" p s
  p="$(root_pane)"
  s="co-$label"
  start_claude "$s" 144 50 "$p" "$dirs"
  wait_claude "$s" "$p" || return 1
  # どちらかの帯の行が出るまで待つ（出るべきでない側は出ないことを見る）
  wait_for 30 screen_has "$s" '_BAND\|tako | ' || true
  sleep 3
  save_cap "$s" "$capdir/coexist-$label.txt"
  check_eq "${label}: 他の mod の行（${other}）が出るか = ${other_shown}" "$other_shown" "$(screen_has "$s" "$other" && echo true || echo false)"
  check_eq "${label}: tako の行が出るか = ${want_tako}" "$want_tako" "$(screen_has "$s" 'tako | ' && echo true || echo false)"
  echo "    観測: $(cap "$s" | grep -E 'tako \| |_BAND' | mask | cut -c1-120 | tr '\n' '/')"
  LAST_ROW_BAND="$(row_field "$p" '.band.state')"
  stop_claude "$s"
  sleep 1
}

phase_coexist() {
  echo "== 1. 帯を描く他の mod と並ぶ（読み込み順を入れ替えても）"
  local dirs
  dirs="$(mod_dir)"
  coexist_one "tako-outer" "$dirs:$OTHER_WRAP" OTHER_WRAP_BAND true true "$CAP_NEW"
  coexist_one "other-outer" "$OTHER_WRAP:$dirs" OTHER_WRAP_BAND true true "$CAP_NEW"
  # next を呼ばない他の mod が内側 = tako が包むので両方出る
  coexist_one "greedy-inner" "$dirs:$OTHER_GREEDY" OTHER_GREEDY_BAND true true "$CAP_NEW"
  # next を呼ばない他の mod が外側 = tako の帯は隠れ、tako mod に理由が出る
  coexist_one "greedy-outer" "$OTHER_GREEDY:$dirs" OTHER_GREEDY_BAND true false "$CAP_NEW"
  check_eq "greedy-outer: tako mod の行に hidden_by_other_mod" "hidden_by_other_mod" "$LAST_ROW_BAND"
}

# --- 3. buttons ------------------------------------------------------------------------------
phase_buttons() {
  echo "== 3. /compact のボタンをキーで押すと圧縮され、tako の操作のボタンでペインが増える"
  local p s before after summaries press dirs
  p="$(root_pane)"
  dirs="$(mod_dir)"
  tk mod ui button add split-right --hotkey s >/dev/null
  s="btn"
  start_claude "$s" 144 21 "$p" "$dirs"
  wait_claude "$s" "$p" || return 1
  one_turn "$s" APRICOT || fail "1 ターン目の答えが出ない"
  wait_for 20 screen_has "$s" '\[ split right \]' || fail "帯に split right のボタンが出ない"
  save_cap "$s" "$CAP_NEW/buttons-before.txt"
  summaries="$(grep -c '"summarize":true' "$FAKE_LOG" | tr -d ' ')"
  # 帯へフォーカス（ctrl+x → Tab）→ ホットキー c = /compact
  key "$s" C-x
  sleep 0.5
  key "$s" Tab
  sleep 1
  key "$s" c
  if wait_for 60 sh -c "[ \$(grep -c '\"summarize\":true' '$FAKE_LOG') -gt $summaries ]"; then
    pass "/compact のボタンで要約の要求が API へ出た"
  else
    fail "/compact のボタンで要約の要求が出ない"
  fi
  if wait_for 30 screen_has "$s" 'Compacted'; then
    pass "画面に圧縮の結果が出た（$(cap "$s" | grep 'Compacted' | head -1 | mask | sed 's/^[ ⎿]*//' | cut -c1-80)）"
  else
    fail "画面に圧縮の結果が出ない"
  fi
  sleep 2
  save_cap "$s" "$CAP_NEW/buttons-compact.txt"
  press="$(row_field "$p" '.report.last_press | "\(.kind)/\(.ok)"')"
  check_eq "報告の last_press は slash で成功" "slash/true" "$press"
  # tako の操作: split right（hotkey s）
  key "$s" Escape
  sleep 1
  before="$(pane_count)"
  key "$s" C-x
  sleep 0.5
  key "$s" Tab
  sleep 1
  key "$s" s
  if wait_for 20 sh -c "[ \$('$TAKO_BIN' list 2>/dev/null | jq '[.tabs[].panes[]] | length') -gt $before ]"; then
    after="$(pane_count)"
    pass "split right のボタンでペインが増えた（${before} → ${after}）"
  else
    fail "split right のボタンでペインが増えない（${before} のまま）"
  fi
  wait_for 20 sh -c "'$TAKO_BIN' mod --json | jq -e '.panes[] | select(.pane == $p) | .report.last_press.kind == \"tako\"' >/dev/null" || true
  press="$(row_field "$p" '.report.last_press | "\(.kind)/\(.ok)"')"
  check_eq "報告の last_press は tako で成功" "tako/true" "$press"
  check_eq "報告にボタンのラベル・コマンドが載らない（last_press のキーは kind / ok / at だけ）" "at,kind,ok" \
    "$(row_field "$p" '.report.last_press | keys | join(",")')"
  stop_claude "$s"
  tk mod ui reset >/dev/null 2>&1
}

# --- 4. bar ---------------------------------------------------------------------------------
drawn_by() { tk limit-service --refresh 2>/dev/null | jq -r '.claude.status_bar.drawn_by'; }
drawn_by_is() { [ "$(drawn_by)" = "$1" ]; }
phase_bar() {
  echo "== 4. バーが PromptHint に出て、同じときにステータスバーから claude の区画が消え、報告が止まると 45 秒で戻る"
  local p s t0 t1 dirs
  p="$(root_pane)"
  dirs="$(mod_dir)"
  tk focus "$p" >/dev/null 2>&1
  wait_for 50 drawn_by_is tako || true
  check_eq "claude が居ないとき: ステータスバーは tako が出す" "tako" "$(drawn_by)"
  s="bar"
  start_claude "$s" 144 21 "$p" "$dirs"
  wait_claude "$s" "$p" || return 1
  one_turn "$s" MANGO || fail "1 ターン目の答えが出ない"
  if wait_for 30 bar_on_screen "$s"; then
    pass "バーが入力欄の下の行に出た: $(cap "$s" | grep -Eo '(5h|7d|ctx) [▁▂▃▄▅▆▇█] [0-9]+%.*' | head -1)"
  else
    fail "バーが出ない"
  fi
  save_cap "$s" "$CAP_NEW/bar.txt"
  if wait_for 20 drawn_by_is mod; then
    pass "同じときのステータスバー: claude の区画は mod（$(tk limit-service --refresh | jq -c '.claude.status_bar')）"
  else
    fail "ステータスバーが mod に任せない（$(tk limit-service --refresh | jq -c '.claude')）"
  fi
  check_eq "tako mod の status_bar も mod" "mod" "$(mod_json | jq -r '.status_bar.drawn_by')"
  check_eq "報告の renders.usage_bar は prompt_hint" "prompt_hint" "$(row_field "$p" '.report.renders.usage_bar')"
  # 報告を止める（mod の `tako mod report` だけを失敗させる = claude は動いたまま）→ 45 秒で tako へ戻る
  : > "$PAUSE_REPORTS"
  t0="$(date +%s)"
  if wait_for 70 drawn_by_is tako; then
    t1="$(date +%s)"
    pass "報告を止めて $((t1 - t0)) 秒で tako の表示へ戻った（理由 $(tk limit-service --refresh | jq -r '.claude.status_bar.reason')・行 $(row_field "$p" '"\(.state) \(.report.age_ms)ms"')）"
    if [ $((t1 - t0)) -ge 30 ] && [ $((t1 - t0)) -le 62 ]; then pass "戻るまでの秒数は鮮度（45 秒）+ 最後の報告からの間隔の範囲"; else fail "戻るまでの秒数が 45 秒の鮮度と合わない（$((t1 - t0)) 秒）"; fi
  else
    fail "報告を止めても tako の表示へ戻らない（行 $(row_field "$p" '"\(.state) \(.report.age_ms)ms"')）"
  fi
  # mod 側も応答が 45 秒途切れると材料を捨ててバーを消す（ステータスバーへ戻った値と二重に出さない）
  if wait_for 10 bar_gone "$s"; then pass "報告が途切れた間は mod もバーを消す"; else fail "報告が途切れてもバーが残る"; fi
  save_cap "$s" "$CAP_NEW/bar-stale.txt"
  rm -f "$PAUSE_REPORTS"
  if wait_for 30 drawn_by_is mod; then pass "報告が戻ると mod へ戻った"; else fail "報告が戻っても mod へ戻らない"; fi
  # フォーカスを外す（シェルのペインへ）→ tako
  local q
  q="$(split_new "$p" --right)"
  tk focus "$q" >/dev/null 2>&1
  if wait_for 10 drawn_by_is tako; then pass "シェルのペインへフォーカスするとステータスバーは tako に戻る"; else fail "フォーカスを外しても mod のまま"; fi
  tk close --pane "$q" --force >/dev/null 2>&1
  tk focus "$p" >/dev/null 2>&1
  stop_claude "$s"
}

# --- 5. statusLine ---------------------------------------------------------------------------
# 隔離 GUI の直接ペインで claude を動かす（tako がそのペインの画面を読める = statusLine の数値を見る経路）
gui_claude() {
  local pane="$1" ccd="$2"
  tk send --pane "$pane" "cd '$WORK' && clear && env CLAUDE_CONFIG_DIR='$ccd' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 claude --model haiku" >/dev/null
}
read_pane() { tk read --pane "$1" 2>/dev/null; }
gui_bar() { read_pane "$1" | grep -Eq '(5h|7d|ctx) [▁▂▃▄▅▆▇█] [0-9]+%'; }
phase_status() {
  echo "== 5. statusLine で ctx を出している構成ではバーを描かない（隔離 GUI の直接ペイン）"
  local p q r s
  p="$(root_pane)"
  if ! wait_for 30 sh -c "'$TAKO_BIN' mod --json | jq -e '.injecting == true' >/dev/null"; then
    unmeasured "tako が新しいペインへ mod を注入しない（$(mod_json | jq -c '.reason')）"
    return 0
  fi
  for spec in "statusline:$CCD_SL" "plain-statusline:$CCD_SL_PLAIN" "no-statusline:$CCD"; do
    local label="${spec%%:*}" ccd="${spec#*:}"
    # 1 枚ペインの新しいタブ（窓いっぱいの高さ = claude が入力欄の下の行と statusLine を描ける）
    q="$(tk tab new --focus 2>/dev/null | jq -r '.pane // empty')"
    [ -n "$q" ] || { fail "${label}: 新しいタブを作れない"; continue; }
    sleep 2
    gui_claude "$q" "$ccd"
    if ! wait_for 90 reporting "$q"; then
      fail "${label}: claude の報告が届かない（$(row_field "$q" '.state') / injected=$(row_field "$q" '.injected')）"
      read_pane "$q" | mask | tail -8 | sed 's/^/      | /'
      tk close --pane "$q" --force >/dev/null 2>&1
      continue
    fi
    tk send --pane "$q" "Reply with exactly one word: KIWI" >/dev/null 2>&1
    wait_for 60 sh -c "'$TAKO_BIN' read --pane $q 2>/dev/null | grep -q KIWI" || fail "${label}: 答えが出ない"
    sleep 20
    read_pane "$q" | mask > "$CAP_NEW/status-$label.txt"
    s="$(row_field "$q" '.report.status_line')"
    r="$(row_field "$q" '.report.renders.usage_bar // "none"')"
    case "$label" in
      statusline)
        check_eq "${label}: 報告の status_line" "true" "$s"
        check_eq "${label}: 画面に statusLine の数値（ctx 45%）" "true" "$(read_pane "$q" | grep -q 'ctx 45%' && echo true || echo false)"
        check_eq "${label}: バーを描かない（renders.usage_bar）" "none" "$r"
        check_eq "${label}: 画面にバーが無い" "false" "$(gui_bar "$q" && echo true || echo false)"
        ;;
      plain-statusline)
        check_eq "${label}: 報告の status_line" "true" "$s"
        check_eq "${label}: 数値を出さない statusLine ではバーを描く" "prompt_hint" "$r"
        check_eq "${label}: 画面にバーがある" "true" "$(gui_bar "$q" && echo true || echo false)"
        ;;
      no-statusline)
        check_eq "${label}: 報告の status_line" "false" "$s"
        check_eq "${label}: バーを描く" "prompt_hint" "$r"
        check_eq "${label}: 画面にバーがある" "true" "$(gui_bar "$q" && echo true || echo false)"
        ;;
    esac
    tk send --pane "$q" --no-newline $'\x03' >/dev/null 2>&1
    sleep 0.5
    tk send --pane "$q" --no-newline $'\x03' >/dev/null 2>&1
    sleep 1
    tk close --pane "$q" --force >/dev/null 2>&1
    sleep 1
  done
}

# --- 6. A/B ----------------------------------------------------------------------------------
phase_ab() {
  echo "== 6. A/B: TAKO_1877_S7_LEGACY=1 で S3 の描き方へ戻る（同一バイナリ）"
  stop_app
  start_app legacy TAKO_1877_S7_LEGACY=1
  check_eq "tako mod の band.s7_legacy" "true" "$(mod_json | jq -r '.band.s7_legacy')"
  local p dirs s
  p="$(root_pane)"
  dirs="$(mod_dir)"
  # 旧挙動では内側の他の mod の帯が消える（S3 の不具合の再現 = 新しい挙動の判定がここで名指しで落ちる）
  coexist_one "legacy-tako-outer" "$dirs:$OTHER_WRAP" OTHER_WRAP_BAND false true "$CAP_OLD"
  s="ab"
  start_claude "$s" 144 21 "$p" "$dirs"
  wait_claude "$s" "$p" || return 1
  one_turn "$s" PEACH || true
  sleep 3
  save_cap "$s" "$CAP_OLD/ab.txt"
  check_eq "A/B: 帯にボタンを描かない" "false" "$(screen_has "$s" '\[ compact \]' && echo true || echo false)"
  check_eq "A/B: バーを描かない" "false" "$(bar_on_screen "$s" && echo true || echo false)"
  check_eq "A/B: 報告に renders を載せない" "null" "$(row_field "$p" '.report.renders')"
  check_eq "A/B: ステータスバーは止めない（legacy_env）" "tako/legacy_env" \
    "$(tk limit-service --refresh | jq -r '.claude.status_bar | "\(.drawn_by)/\(.reason)"')"
  stop_claude "$s"
  stop_app
  start_app main
}

# --- visual ------------------------------------------------------------------------------------
run_visual() {
  local log="$1" dump="$2"
  shift 2
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" TAKO_PERSIST=0 TAKO_DATA_DIR="$TMP/data-visual-$$-$RANDOM" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=mod-bar TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID"
  wait "$pid"
  local rc=$?
  ISOLATED_GUI_PID=""
  return $rc
}
phase_visual() {
  echo "== 4 の絵: visual-test mod-bar（実ピクセル。新しい挙動は ${DUMP_NEW#"$TMP/"}）"
  local rc=0
  run_visual "$TMP/visual.log" "$DUMP_NEW" || rc=$?
  check_eq "節が終了コード 0 で終わる" "0" "$rc"
  grep 'TAKO_VISUAL_PIXEL: mod-bar\|TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | mask | sed 's/^/    観測: /'
  check_eq "帯の絵を 3 枚書き出した" "3" "$(ls "$DUMP_NEW"/mod-bar-*.png 2>/dev/null | wc -l | tr -d ' ')"
  echo "== 4 の絵: A/B（TAKO_1877_S7_LEGACY=1。旧挙動は ${DUMP_OLD#"$TMP/"}）"
  rc=0
  run_visual "$TMP/visual-legacy.log" "$DUMP_OLD" TAKO_1877_S7_LEGACY=1 || rc=$?
  check_eq "A/B: 節が非ゼロで終わる" "true" "$([ "$rc" -ne 0 ] && echo true || echo false)"
  check_eq "A/B: 落ちた理由は区画を出したこと（①）" "true" \
    "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test mod-bar ①' "$TMP/visual-legacy.log" && echo true || echo false)"
}

# --- 実行 ----------------------------------------------------------------------------------------
echo "claude: $(claude --version 2>/dev/null | head -1) / 作業 dir: $(printf '%s' "$TMP" | mask)"
want static && phase_static
if want widths || want coexist || want buttons || want bar || want status || want ab; then
  start_app main
  echo "  隔離 GUI: pid $APP_PID / mod: $(mod_json | jq -c '{injecting, claude_version, plugin_dir}' | mask)"
  want widths && phase_widths
  want coexist && phase_coexist
  want buttons && phase_buttons
  want bar && phase_bar
  want status && phase_status
  want ab && phase_ab
  stop_app
fi
want visual && phase_visual

REAL_AFTER="$(snap_real)"
check_eq "本物の ~/.claude*/skills・plugins は前後で同じ（${REAL_BEFORE}）" "$REAL_BEFORE" "$REAL_AFTER"
echo "採取: $(printf '%s' "$CAP_NEW" | mask) / $(printf '%s' "$CAP_OLD" | mask)・絵: $(printf '%s' "$DUMP_NEW" | mask) / $(printf '%s' "$DUMP_OLD" | mask)"
echo "== 結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
