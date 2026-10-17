#!/usr/bin/env bash
# test-mod-command-cards-1963.sh — tako mod S7-4（#1963。FR-2.42.41〜）の実経路テスト
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）を**一時の設定 dir** と**偽の Messages API**
# （scripts/lib/fake-messages-api.mjs。「Show this card: {JSON}」で tako の MCP の tako_show_command を呼ぶ）で
# 動かし、隔離 GUI（tako-vd・TAKO_PERSIST=0・自動リネーム off）のペインで AI がカードを出す経路を確かめる。
# 番号は Issue #1963 の受け入れ条件:
#   show       1. AI の tako_show_command（pane 省略）で会話の行にカード・帯に `card N` とボタンが出て、
#                 GPUI のカード帯は出ず viewport（ペインの行数）が**一瞬も**縮まない（0.1 秒ごとに採る）
#   keys       2 / 3. 全画面: 帯（ctrl+x → Tab → ホットキー）の Run で新しいペインと実行記録（スマホと同じ
#                 `show-command --list` の runs）・閉じるでカード 0 枚（帯から消え GPUI の帯も出ない）
#   mainscreen 3. main screen（CLAUDE_CODE_NO_FLICKER=0）でも同じくカードが出てキーで Run が押せる
#   stop       4. mod の報告が途切れる（mod が叩く CLI の `mod report` だけを失敗させる）と 45 秒 + 1 tick で、
#                 claude が終わる（最後の報告）と 1 tick（2 秒）で GPUI のカード帯へ戻る
#   place      5. `tako mod ui set cards.place tako` で今の挙動（GPUI の帯・mod は描かない）
#   copy       2. Copy は論理文字列（157 字・改行なし）を $.ui.copy へ。他の mod に拒まれたら
#                 `show-command --copy` へ落ちる（**隔離 tmux の claude で押す** = 本物のクリップボードへ書かない）
#   ab         A/B: TAKO_1963_LEGACY=1 で S7-4 前（GPUI の帯が出て viewport が縮む・mod は描かない）。
#                 1 の判定がこの腕で名指しで落ちる
#   visual     visual-test `mod-card`（実ピクセル。新旧で書き出し先を分ける）
#   static     `claude plugin validate --strict` / `claude plugin test`（terminal と desktop の両 surface）
#
# **本物の設定 dir（~/.claude*）には書かない**: claude の設定 dir は mktemp 配下（認証は写さず、偽 API と
# ダミーの API キーを一時の .claude.json で承認済みにする）。前後で本物の ~/.claude*/skills・plugins の
# 一覧とハッシュを比べる。**本物のクリップボードにも書かない**: tako の GUI のペインの claude の $.ui.copy は
# 偽の pbcopy を置いても OSC 52 で tako 経由のシステムのクリップボードへ届く（#1963 の探索で実測）ので、Copy は
# 隔離 tmux（set-clipboard off・クライアントなし）の claude でだけ押し、前後で pbpaste のハッシュを比べる。
# 本番 tako が配る env（TAKO_SOCKET / TAKO_PANE_ID / TAKO_CLI / CLAUDE_CODE_PLUGIN_DIRS 等）は最初に落とす。
# 落とすのは自分で起こした pid と tmux ソケットだけ。
#
# 使い方: bash scripts/test-mod-command-cards-1963.sh   （ONLY=static|show|keys|mainscreen|stop|place|copy|ab|visual）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は mod の `claude plugin test`・
# Rust の単体テストと番犬（issue1963_mod_command_cards_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_1877_S3_LEGACY \
  TAKO_1877_S7_LEGACY TAKO_1963_LEGACY TAKO_1881_BAND_THRESHOLD TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
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

TMP="$(mktemp -d /tmp/tako-1963-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
FAKE_PID=""
SAMPLER_PID=""
TMUX_SOCKET="tako-1963-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }
tmx() { tmux -u -L "$TMUX_SOCKET" "$@"; }

cleanup() {
  [ -n "$SAMPLER_PID" ] && kill "$SAMPLER_PID" 2>/dev/null
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmx kill-server >/dev/null 2>&1 || true
  tmux -L "$TMUX_SOCKET-gui" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET" "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET-gui"
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
caffeinate -d -u -w $$ >/dev/null 2>&1 &

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
API_KEY="sk-ant-api03-fake1963fake1963fake1963fake1963fake1963fake1963AA"
# 設定 dir: tako の MCP（`tako mcp serve` = 呼び出し元ペインは TAKO_PANE_ID）を user スコープで登録
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

# 他の mod の代役: ui.copy を拒む（tako の Copy が CLI へ落ちる経路を作る）
OTHER_REFUSE="$TMP/mods/other-refuse"
mkdir -p "$OTHER_REFUSE/.claude-plugin" "$OTHER_REFUSE/hooks"
printf '{"name":"other-refuse","version":"0.0.1","description":"refuses ui.copy for the #1963 check"}\n' \
  > "$OTHER_REFUSE/.claude-plugin/plugin.json"
printf '{ "modules": ["./register.ts"] }\n' > "$OTHER_REFUSE/hooks/hooks.json"
cat > "$OTHER_REFUSE/hooks/register.ts" <<'EOF'
import type { Register } from 'claude-code'
export const register: Register = on => {
  on('ui.copy', () => ({ value: { isCopied: false, reason: 'refused' } })).catch(($, e, next) => next(e))
}
EOF

# mod が叩く CLI（TAKO_CLI）の薄い包み: 合図のファイルがある間だけ `mod report` を失敗させる（= 報告が途切れた
# 状態。claude は生かしたまま）。`show-command --copy` は叩いた引数を控えて成功を返す（GUI がシステムの
# クリップボードへ書くので本物を呼ばない。dispatch のコピー自体は Rust の単体テストが見る）
PAUSE_REPORTS="$TMP/pause-reports"
GATE_LOG="$TMP/gate.log"
CLI_GATE="$TMP/tako-cli-gate"
cat > "$CLI_GATE" <<EOF
#!/bin/sh
if [ -e '$PAUSE_REPORTS' ] && [ "\$1" = mod ] && [ "\$2" = report ]; then
  exit 1
fi
if [ "\$1" = show-command ] && [ "\$2" = --copy ]; then
  echo "\$*" >> '$GATE_LOG'
  echo '{"copied":true}'
  exit 0
fi
exec '$TAKO_BIN' "\$@"
EOF
chmod +x "$CLI_GATE"

# 試すカード: 157 字・改行なしの長いコマンド（設計書 §9.2 と同じ長さ）と、本物の改行を含む複数行のコマンド
LONG='cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long-data-dir-name ./target/debug/tako mod --json | jq ".panes[] | {pane, reason}"'
MULTI="printf 'run-1963\\n' \\
  && true"
CARD_JSON="$(jq -cn --arg a "$LONG" --arg b "$MULTI" '{commands: [$a, $b], label: "build and check"}')"

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
rows_equal() { [ "$(rows_of "$1")" = "$2" ]; }
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

# 隔離 GUI の直接ペインで claude を起こす（ペインの env = tako の注入はそのまま。mod が叩く CLI だけ包みへ）
gui_claude() {
  local pane="$1" extra="${2:-}"
  # 前の段の claude が残っていれば終える（前の段が途中で落ちても、次の段がその入力欄へ打ち込まない）
  if reporting "$pane"; then quit_claude "$pane"; fi
  tk send --pane "$pane" "cd '$WORK' && clear && env PATH='$FAKEBIN':\$PATH TAKO_CLI='$CLI_GATE' CLAUDE_CONFIG_DIR='$CCD' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 $extra claude --model haiku --allowedTools mcp__tako__tako_show_command" >/dev/null
  if ! wait_for 90 reporting "$pane"; then
    fail "claude の報告が届かない（state=$(row_field "$pane" '.state')）"
    read_pane "$pane" | mask | tail -10 | sed 's/^/      | /'
    return 1
  fi
}
# claude を終える（ctrl+c 2 回 = session.end の最後の報告が届く）
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

# 0.1 秒ごとにペインの行数を採る（GPUI のカード帯が一瞬でも出れば行数が縮む）
start_sampler() {
  local pane="$1" out="$2"
  : > "$out"
  (
    while :; do
      rows_of "$pane" >> "$out"
      sleep 0.1
    done
  ) &
  SAMPLER_PID=$!
}
stop_sampler() {
  [ -n "$SAMPLER_PID" ] && kill "$SAMPLER_PID" 2>/dev/null
  wait "$SAMPLER_PID" 2>/dev/null
  SAMPLER_PID=""
}
min_rows() { grep -E '^[0-9]+$' "$1" | sort -n | head -1; }

# AI にカードを出させる（偽 API が tako_show_command を呼ぶ）。出たカードの id を CARD に
show_card() {
  local pane="$1" json="$2"
  local before after
  before="$(cards_json "$pane" | jq -r '[.cards[].id] | max // 0')"
  # 文を入れてから Enter を別に送る（main screen は文と改行を 1 度に受けると貼り付けとして改行を入れる）
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

# --- static ---------------------------------------------------------------------------------
phase_static() {
  echo "== static: claude plugin validate --strict / test（一時の設定 dir・mod の写し）"
  local cfg="$TMP/cfg-static" m="$TMP/mod-static" out
  mkdir -p "$cfg"
  /bin/cp -R "$REPO_ROOT/crates/tako-core/claude-mod" "$m"
  rm -rf "$m/.claude-plugin/types"
  out="$(cd "$TMP" && env CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 claude plugin validate --strict "$m" 2>&1)"
  if grep -q 'Validation passed' <<<"$out"; then pass "validate --strict"; else fail "validate --strict"; printf '%s\n' "$out" | mask | tail -15; fi
  out="$(cd "$TMP" && env CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 claude plugin test "$m" 2>&1)"
  if grep -Eq '^ *0 fail' <<<"$out"; then
    pass "claude plugin test（$(printf '%s' "$out" | grep -E '^ *[0-9]+ pass' | tr -d ' ')。cards.test.ts は terminal / desktop の両 surface）"
  else
    fail "claude plugin test"
    printf '%s\n' "$out" | grep -v '^(pass)' | mask | tail -30
  fi
}

# --- 1. show --------------------------------------------------------------------------------
# 1 の判定（A/B の腕でも同じ関数を回し、旧挙動で名指しで落ちることを見る）
judge_show() {
  local label="$1" pane="$2" rows0="$3" samples="$4" capdir="$5" min band card_line
  min="$(min_rows "$samples")"
  save_pane "$pane" "$capdir/show-$label.txt"
  band="$(read_pane "$pane" | grep 'tako | ' | tail -1)"
  card_line="$(read_pane "$pane" | grep -E "tako +card ${CARD}( |$)" | head -1)"
  echo "    観測: 帯 $(printf '%s' "$band" | sed 's/  *$//' | mask | cut -c1-160)"
  echo "    観測: 行数 前 ${rows0} / 採取の最小 ${min} / 後 $(rows_of "$pane")（採取 $(grep -c . "$samples") 回）・持ち主 $(card_owner "$pane" "$CARD")"
  check_eq "${label}: 会話の行にカード（tako  card ${CARD}）" "true" "$([ -n "$card_line" ] && echo true || echo false)"
  check_eq "${label}: 帯に card ${CARD} (1/2) とボタン（y: copy / r: run）" "true true true" \
    "$(contains "$band" "card ${CARD} (1/2)") $(contains "$band" 'y: copy') $(contains "$band" 'r: run')"
  check_eq "${label}: GPUI のカード帯は一瞬も出ない（行数の最小 = 前）" "$rows0" "$min"
  check_eq "${label}: 持ち主は mod（show-command --list の owner）" "mod/drawn" "$(card_owner "$pane" "$CARD")"
  check_eq "${label}: 報告の renders.cards に載る" "true" \
    "$(row_field "$pane" ".report.renders.cards // [] | index($CARD) != null")"
}

phase_show() {
  echo "== 1. AI の tako_show_command（pane 省略）で会話の行と帯にカード・GPUI の帯は出ず viewport が縮まない（全画面）"
  local p rows0 t0 t1
  p="$(root_pane)"
  gui_claude "$p" || return 1
  rows0="$(rows_of "$p")"
  start_sampler "$p" "$TMP/rows-show.txt"
  t0="$(date +%s)"
  show_card "$p" "$CARD_JSON" || { stop_sampler; return 1; }
  wait_for 15 owner_is "$p" "$CARD" mod/drawn || true
  t1="$(date +%s)"
  sleep 3
  stop_sampler
  echo "    観測: カードを頼んでから mod が描いたと言うまで $((t1 - t0)) 秒"
  judge_show "new" "$p" "$rows0" "$TMP/rows-show.txt" "$CAP_NEW"
  SHOW_PANE="$p"
}

# --- 2 / 3. keys（全画面）---------------------------------------------------------------------
phase_keys() {
  echo "== 2 / 3. 全画面: 帯のキーで Run（新しいペイン + 実行記録）・閉じるでカード 0 枚"
  local p before run state
  p="${SHOW_PANE:-}"
  if [ -z "$p" ] || [ -z "${CARD:-}" ]; then
    unmeasured "show の段のカードが無い（ONLY=keys は show と一緒に回す）"
    return 0
  fi
  before="$(pane_count)"
  # 次（2 件目 = 複数行のコマンド）→ 実行
  band_press "$p" n
  if wait_for 10 sh -c "'$TAKO_BIN' read --pane $p | grep -q 'card $CARD (2/2)'"; then pass "次で 2 件目へ（帯に card $CARD (2/2)）"; else fail "次が効かない"; fi
  key "$p" r
  if wait_for 20 sh -c "[ \$('$TAKO_BIN' list | jq '[.tabs[].panes[]] | length') -gt $before ]"; then
    pass "Run で新しいペインが立った（$before → $(pane_count)）"
  else
    fail "Run でペインが増えない"
  fi
  wait_for 20 sh -c "'$TAKO_BIN' show-command --list --pane $p | jq -e '.cards[] | select(.id == $CARD) | .runs[1].state == \"exited\"' >/dev/null" || true
  run="$(cards_json "$p" | jq -c --argjson c "$CARD" '.cards[] | select(.id == $c) | .runs[1] | {state, exit_code, count}')"
  check_eq "実行記録（show-command --list の runs[1] = スマホと同じ記録）" '{"state":"exited","exit_code":0,"count":1}' "$run"
  check_eq "報告の last_press は card_run で成功" "card_run/true" "$(row_field "$p" '.report.last_press | "\(.kind)/\(.ok)"')"
  # 会話の行の実行記録は次の報告（最大 15 秒）で届く
  if wait_for 20 screen_has "$p" '→ pane [0-9]* exited 0'; then pass "会話の行に実行記録（→ pane N exited 0）"; else fail "会話の行に実行記録が出ない"; fi
  save_pane "$p" "$CAP_NEW/keys-run.txt"
  # 実行ペインを閉じてから、閉じる（x）でカード 0 枚
  tk close --pane "$(cards_json "$p" | jq -r --argjson c "$CARD" '.cards[] | select(.id == $c) | .runs[1].pane')" --force >/dev/null 2>&1
  sleep 2
  local rows_before_close
  rows_before_close="$(rows_of "$p")"
  band_press "$p" x
  if wait_for 10 sh -c "[ \$('$TAKO_BIN' show-command --list --pane $p | jq '.cards | length') = 0 ]"; then pass "閉じるでカード 0 枚（store から消えた）"; else fail "閉じるでカードが消えない"; fi
  if wait_for 20 sh -c "! '$TAKO_BIN' read --pane $p | grep 'tako | ' | grep -q 'card '"; then pass "帯から card が消えた"; else fail "帯に card が残る"; fi
  check_eq "カード 0 枚: GPUI の帯も出ない（行数そのまま）" "$rows_before_close" "$(rows_of "$p")"
  # 帯を描き直した後の報告（1 秒の flush）を待つ
  wait_for 10 sh -c "'$TAKO_BIN' mod --json | jq -e '.panes[] | select(.pane == $p) | .report.renders.cards == []' >/dev/null" || true
  check_eq "報告の renders.cards は空" "[]" "$(row_field "$p" '.report.renders.cards' | tr -d ' \n')"
  quit_claude "$p"
}

# --- 3. main screen -------------------------------------------------------------------------
phase_mainscreen() {
  echo "== 3. main screen（CLAUDE_CODE_NO_FLICKER=0）でもカードが出て、帯のキーで Run が押せる"
  local p rows0 before one
  p="$(root_pane)"
  gui_claude "$p" "CLAUDE_CODE_NO_FLICKER=0" || return 1
  one="$(jq -cn '{commands: ["printf main-1963"], label: "main screen"}')"
  rows0="$(rows_of "$p")"
  start_sampler "$p" "$TMP/rows-main.txt"
  show_card "$p" "$one" || { stop_sampler; return 1; }
  wait_for 15 owner_is "$p" "$CARD" mod/drawn || true
  sleep 3
  stop_sampler
  save_pane "$p" "$CAP_NEW/mainscreen.txt"
  check_eq "main screen: 持ち主は mod" "mod/drawn" "$(card_owner "$p" "$CARD")"
  check_eq "main screen: GPUI のカード帯は一瞬も出ない" "$rows0" "$(min_rows "$TMP/rows-main.txt")"
  check_eq "main screen: 会話の行にカード" "true" "$(screen_has "$p" "tako *card ${CARD}" && echo true || echo false)"
  before="$(pane_count)"
  band_press "$p" r
  if wait_for 20 sh -c "[ \$('$TAKO_BIN' list | jq '[.tabs[].panes[]] | length') -gt $before ]"; then
    pass "main screen: 帯のキー（ctrl+x → Tab → r）で Run が押せた"
  else
    fail "main screen: Run が押せない"
    read_pane "$p" | mask | tail -8 | sed 's/^/      | /'
  fi
  wait_for 10 sh -c "'$TAKO_BIN' show-command --list --pane $p | jq -e '.cards[0].runs[0] != null' >/dev/null" || true
  check_eq "main screen: 実行記録が付いた" "true" "$(cards_json "$p" | jq '.cards[0].runs[0] != null')"
  tk close --pane "$(cards_json "$p" | jq -r '.cards[0].runs[0].pane')" --force >/dev/null 2>&1
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
  quit_claude "$p"
}

# --- 4. stop --------------------------------------------------------------------------------
phase_stop() {
  echo "== 4. mod が止まると GPUI のカード帯へ戻る（報告が途切れる = 45 秒 + 1 tick / claude が終わる = 1 tick）"
  local p rows0 t0 t1
  p="$(root_pane)"
  gui_claude "$p" || return 1
  rows0="$(rows_of "$p")"
  show_card "$p" "$(jq -cn '{commands: ["echo stop-1963"]}')" || return 1
  wait_for 15 owner_is "$p" "$CARD" mod/drawn || true
  check_eq "止める前: 持ち主は mod・行数そのまま" "mod/drawn ${rows0}" "$(card_owner "$p" "$CARD") $(rows_of "$p")"
  # (a) 報告を止める（mod が叩く CLI の `mod report` だけを失敗させる）
  : > "$PAUSE_REPORTS"
  t0="$(date +%s)"
  if wait_for 70 rows_below "$p" "$rows0"; then
    t1="$(date +%s)"
    pass "報告を止めて $((t1 - t0)) 秒で GPUI のカード帯が出た（持ち主 $(card_owner "$p" "$CARD")・行数 ${rows0} → $(rows_of "$p")）"
    if [ $((t1 - t0)) -ge 30 ] && [ $((t1 - t0)) -le 64 ]; then pass "戻るまでの秒数は鮮度（45 秒）+ 最後の報告からの間隔 + 1 tick の範囲"; else fail "戻るまでの秒数が鮮度と合わない（$((t1 - t0)) 秒）"; fi
  else
    fail "報告を止めても GPUI のカード帯が出ない（持ち主 $(card_owner "$p" "$CARD")）"
  fi
  save_pane "$p" "$CAP_NEW/stop-stale.txt"
  rm -f "$PAUSE_REPORTS"
  if wait_for 30 rows_equal "$p" "$rows0"; then pass "報告が戻ると mod へ戻り、行数も戻った（$(card_owner "$p" "$CARD")）"; else fail "報告が戻っても mod へ戻らない（$(card_owner "$p" "$CARD") rows=$(rows_of "$p")）"; fi
  # (b) claude を終える（最後の報告 = tako はそのペインの報告を即座に捨てる）
  exit_claude "$p"
  t0="$(perl -MTime::HiRes=time -e 'printf "%.1f", time')"
  local i shrunk=""
  for i in $(seq 1 100); do
    if rows_below "$p" "$rows0"; then
      shrunk="$(perl -MTime::HiRes=time -e 'printf "%.1f", time')"
      break
    fi
    sleep 0.1
  done
  if [ -n "$shrunk" ]; then
    local dt
    dt="$(perl -e "printf '%.1f', $shrunk - $t0")"
    pass "claude を終えて ${dt} 秒で GPUI のカード帯が出た（持ち主 $(card_owner "$p" "$CARD")）"
    if perl -e "exit !($dt <= 4.5)"; then pass "1 tick（2 秒）+ claude の終了処理の範囲（≤ 4.5 秒）"; else fail "claude の終了から ${dt} 秒（1 tick より遅い）"; fi
  else
    fail "claude を終えても GPUI のカード帯が出ない（持ち主 $(card_owner "$p" "$CARD")）"
  fi
  save_pane "$p" "$CAP_NEW/stop-ended.txt"
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
  wait_for 30 not_reporting "$p" || true
}

# --- 5. place -------------------------------------------------------------------------------
phase_place() {
  echo "== 5. cards.place=tako で今の挙動（GPUI のカード帯が出る・mod は描かない）"
  local p rows0
  p="$(root_pane)"
  tk mod ui set cards.place tako >/dev/null || fail "tako mod ui set cards.place tako が通らない"
  gui_claude "$p" || return 1
  rows0="$(rows_of "$p")"
  show_card "$p" "$CARD_JSON" || return 1
  if wait_for 10 rows_below "$p" "$rows0"; then pass "GPUI のカード帯が出た（行数 ${rows0} → $(rows_of "$p")）"; else fail "GPUI のカード帯が出ない"; fi
  sleep 3
  save_pane "$p" "$CAP_NEW/place-tako.txt"
  check_eq "持ち主は tako（place_tako）" "tako/place_tako" "$(card_owner "$p" "$CARD")"
  check_eq "帯に card を描かない" "false" "$(read_pane "$p" | grep 'tako | ' | grep -q 'card ' && echo true || echo false)"
  check_eq "会話の行をカードに描き替えない" "false" "$(screen_has "$p" "tako *card ${CARD}" && echo true || echo false)"
  check_eq "報告に renders.cards を載せない（描かせていない）" "null" "$(row_field "$p" '.report.renders.cards')"
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
  quit_claude "$p"
  tk mod ui reset >/dev/null 2>&1
}

# --- 2. copy（隔離 tmux）------------------------------------------------------------------------
start_tmux_claude() {
  local name="$1" pane="$2" dirs="$3"
  tmx kill-session -t "=$name" >/dev/null 2>&1
  tmx new-session -d -s "$name" -x 144 -y 30 -c "$WORK" \
    env PATH="$FAKEBIN:$PATH" TAKO_ISOLATED=1 TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" \
    TAKO_ORCHESTRATOR_DIR="$TAKO_ORCHESTRATOR_DIR" CLAUDE_CONFIG_DIR="$CCD" \
    ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    TAKO_PANE_ID="$pane" TAKO_CLI="$CLI_GATE" CLAUDE_CODE_PLUGIN_DIRS="$dirs" \
    claude --model haiku
  # tmux へ届く OSC 52 をどこへも渡さない（クライアントも付けない）
  tmx set-option -g set-clipboard off >/dev/null 2>&1
  tmx set-option -t "=$name" remain-on-exit off >/dev/null 2>&1
}
tcap() { tmx capture-pane -p -t "=$1:" 2>/dev/null; }
tscreen_has() { tcap "$1" | grep -q -- "$2"; }
tkey() {
  tmx send-keys -t "=$1:" "$2"
  sleep 0.6
}
tband_press() {
  tkey "$1" Escape
  tkey "$1" C-x
  tkey "$1" Tab
  tkey "$1" "$2"
}
phase_copy() {
  echo "== 2. Copy は論理文字列を \$.ui.copy へ（157 字・改行なし）・拒まれたら show-command --copy へ（隔離 tmux）"
  local p dirs s copied
  p="$(root_pane)"
  dirs="$(mod_dir)"
  CARD="$(tk show-command --pane "$p" "$LONG" | jq -r '.card.id')"
  s="copy"
  start_tmux_claude "$s" "$p" "$dirs"
  if ! wait_for 60 tscreen_has "$s" "card $CARD"; then
    fail "tmux の claude の帯に card $CARD が出ない"
    tcap "$s" | mask | tail -8 | sed 's/^/      | /'
    return 1
  fi
  rm -f "$TMP/pbcopy.out"
  tband_press "$s" y
  wait_for 10 test -f "$TMP/pbcopy.out" || true
  copied="$(cat "$TMP/pbcopy.out" 2>/dev/null)"
  check_eq "Copy で渡った文字列は論理文字列そのまま" "$LONG" "$copied"
  check_eq "長さ 157 字・改行 0" "157 0" "$(printf '%s' "$copied" | wc -m | tr -d ' ') $(printf '%s' "$copied" | tr -cd '\n' | wc -c | tr -d ' ')"
  wait_for 5 sh -c "'$TAKO_BIN' mod --json | jq -e '.panes[] | select(.pane == $p) | .report.last_press.kind == \"card_copy\"' >/dev/null" || true
  check_eq "報告の last_press は card_copy で成功（CLI へは落ちない）" "card_copy/true/0" \
    "$(row_field "$p" '.report.last_press | "\(.kind)/\(.ok)"')/$(grep -c . "$GATE_LOG" 2>/dev/null || echo 0)"
  tcap "$s" | mask > "$CAP_NEW/copy.txt"
  tmx kill-session -t "=$s" >/dev/null 2>&1
  wait_for 30 not_reporting "$p" || true
  # 他の mod が ui.copy を拒む → tako の CLI（show-command --copy）へ落ちる
  s="refuse"
  start_tmux_claude "$s" "$p" "$OTHER_REFUSE:$dirs"
  if ! wait_for 60 tscreen_has "$s" "card $CARD"; then
    fail "（拒む mod と同居）帯に card $CARD が出ない"
    return 1
  fi
  rm -f "$TMP/pbcopy.out"
  tband_press "$s" y
  wait_for 10 test -s "$GATE_LOG" || true
  check_eq "拒まれたら show-command --copy --card N --index 1 を叩く" "show-command --copy --card $CARD --index 1" "$(tail -1 "$GATE_LOG" 2>/dev/null)"
  check_eq "拒まれたので pbcopy は呼ばれない" "false" "$([ -e "$TMP/pbcopy.out" ] && echo true || echo false)"
  wait_for 5 sh -c "'$TAKO_BIN' mod --json | jq -e '.panes[] | select(.pane == $p) | .report.last_press.kind == \"card_copy_cli\"' >/dev/null" || true
  check_eq "報告の last_press は card_copy_cli で成功" "card_copy_cli/true" "$(row_field "$p" '.report.last_press | "\(.kind)/\(.ok)"')"
  tmx kill-session -t "=$s" >/dev/null 2>&1
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
  wait_for 30 not_reporting "$p" || true
}

# --- A/B ------------------------------------------------------------------------------------
phase_ab() {
  echo "== A/B: TAKO_1963_LEGACY=1 で S7-4 前（同一バイナリ）。1 の判定が名指しで落ちる"
  stop_app
  start_app legacy TAKO_1963_LEGACY=1
  check_eq "tako mod の cards.legacy" "true" "$(mod_json | jq -r '.cards.legacy')"
  local p rows0 before_fail
  p="$(root_pane)"
  gui_claude "$p" || return 1
  rows0="$(rows_of "$p")"
  start_sampler "$p" "$TMP/rows-ab.txt"
  show_card "$p" "$CARD_JSON" || { stop_sampler; return 1; }
  sleep 5
  stop_sampler
  # 新しい挙動の判定（judge_show）をこの腕で回し、落ちた項目を数える（落ちるのが期待）
  before_fail="$FAIL"
  echo "    （以下は新しい挙動の判定を旧挙動に当てたもの。[NG] が出るのが期待）"
  judge_show "legacy" "$p" "$rows0" "$TMP/rows-ab.txt" "$CAP_OLD"
  local named=$((FAIL - before_fail))
  FAIL="$before_fail"
  if [ "$named" -ge 4 ]; then pass "A/B: 新しい挙動の判定が ${named} 項目で名指しで落ちた"; else fail "A/B: 新しい挙動の判定が落ちない（${named} 項目）"; fi
  check_eq "A/B: 持ち主は tako（legacy_env）" "tako/legacy_env" "$(card_owner "$p" "$CARD")"
  check_eq "A/B: GPUI のカード帯が出て行数が縮む" "true" "$(rows_below "$p" "$rows0" && echo true || echo false)"
  tk show-command --dismiss --pane "$p" >/dev/null 2>&1
  quit_claude "$p"
  stop_app
  start_app main
}

# --- visual ---------------------------------------------------------------------------------
run_visual() {
  local log="$1" dump="$2"
  shift 2
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" TAKO_PERSIST=0 TAKO_DATA_DIR="$TMP/data-visual-$$-$RANDOM" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=mod-card TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID"
  wait "$pid"
  local rc=$?
  ISOLATED_GUI_PID=""
  return $rc
}
phase_visual() {
  echo "== 1 / 4 の絵: visual-test mod-card（実ピクセル。新しい挙動は ${DUMP_NEW#"$TMP/"}）"
  local rc=0
  run_visual "$TMP/visual.log" "$DUMP_NEW" || rc=$?
  check_eq "節が終了コード 0 で終わる" "0" "$rc"
  grep 'TAKO_VISUAL_PIXEL: mod-card\|TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | mask | sed 's/^/    観測: /'
  check_eq "絵を 4 枚書き出した" "4" "$(ls "$DUMP_NEW"/mod-card-*.png 2>/dev/null | wc -l | tr -d ' ')"
  echo "== 絵: A/B（TAKO_1963_LEGACY=1。旧挙動は ${DUMP_OLD#"$TMP/"}）"
  rc=0
  run_visual "$TMP/visual-legacy.log" "$DUMP_OLD" TAKO_1963_LEGACY=1 || rc=$?
  check_eq "A/B: 節が非ゼロで終わる" "true" "$([ "$rc" -ne 0 ] && echo true || echo false)"
  check_eq "A/B: 落ちた理由は mod が描いたカードを GPUI の帯へ出したこと（①）" "true" \
    "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test mod-card ①' "$TMP/visual-legacy.log" && echo true || echo false)"
}

# --- 実行 ----------------------------------------------------------------------------------------
echo "claude: $(claude --version 2>/dev/null | head -1) / 作業 dir: $(printf '%s' "$TMP" | mask)"
want static && phase_static
if want show || want keys || want mainscreen || want stop || want place || want copy || want ab; then
  start_app main
  echo "  隔離 GUI: pid $APP_PID / mod: $(mod_json | jq -c '{injecting, claude_version, cards}' | mask)"
  if want show || want keys; then phase_show; fi
  want keys && phase_keys
  want mainscreen && phase_mainscreen
  want stop && phase_stop
  want place && phase_place
  want copy && phase_copy
  want ab && phase_ab
  stop_app
fi
want visual && phase_visual

REAL_AFTER="$(snap_real)"
check_eq "本物の ~/.claude*/skills・plugins は前後で同じ（${REAL_BEFORE}）" "$REAL_BEFORE" "$REAL_AFTER"
check_eq "本物のクリップボードは前後で同じ（中身は読まずハッシュだけ）" "$CLIP_BEFORE" "$(snap_clipboard)"
echo "採取: $(printf '%s' "$CAP_NEW" | mask) / $(printf '%s' "$CAP_OLD" | mask)・絵: $(printf '%s' "$DUMP_NEW" | mask) / $(printf '%s' "$DUMP_OLD" | mask)"
echo "== 結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
