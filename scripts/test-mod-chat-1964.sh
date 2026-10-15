#!/usr/bin/env bash
# test-mod-chat-1964.sh — tako mod S7-6（#1964。FR-2.42.48〜）の実経路テスト
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）を**一時の設定 dir** と**偽の Messages API**
# （scripts/lib/fake-messages-api.mjs。「Reply with code blocks」= コードブロック 2 つの返答 /
# 「Run this command: <cmd>」= Bash の呼び出し）で動かし、隔離 GUI（tako-vd・TAKO_PERSIST=0・自動リネーム off）の
# ペインでチャット風の描画を確かめる。番号は Issue #1964 の受け入れ条件:
#   visual   1 / 4. visual-test `mod-chat`: 同じ会話を GPUI のチャット（既定 = chat.place auto）と mod の描画
#               （chat.place mod）で撮り、mod の描画の Copy をクリック・帯の「最新のコードをコピー」をキーで押す
#               （Copy は ui.copy を拒む mod を同居させて押す = 本物のクリップボードへ書かない）。
#               A/B（TAKO_1964_LEGACY=1）は ② で FAILED = 同一バイナリでの検出力
#   term     3. ターミナル表示（既定）では吹き出し・ツールの要約を描かずコードのコピーだけ
#   gui      フォールバック側（#1883 の置き換え）: GUI モードの GPUI のチャットが、accounts.yaml に無い設定 dir の
#               claude の会話を報告の設定 dir から読み、新しい発話を mod の通知（chat_seq）から 1 tick 以内に映す
#            1. chat.place mod でそのペインがターミナル表示（mod の描画）になり、吹き出し・Copy・要約が画面に出る
#            2. mod の報告が途切れる（mod が叩く CLI の `mod report` だけを失敗させる）と 45 秒 + 1 tick、
#               claude が終わると 1 tick で GPUI のチャットへ戻る
#   ab       A/B（TAKO_1964_LEGACY=1）: chat.place mod でも GPUI のチャットのまま・未登録の設定 dir の会話は読めない
#   tmux     Copy の中身（**隔離 tmux** の claude で押す = 本物のクリップボードへ書かない）: 帯のキーは最新の返答の
#               最後のコード・会話の行の Copy 1 のクリック（SGR）は 157 字・改行 0 の論理文字列。他の mod が同じ描画の口を
#               使う（next を包む）ときに外側 / 内側のどちらでも両方の描画が残る
#   static   `claude plugin validate --strict` / `claude plugin test`（scripts/check-claude-mod.sh）
#
# **本物の設定 dir（~/.claude*）には書かない**: claude の設定 dir は mktemp 配下（認証は写さず、偽 API と
# ダミーの API キーを一時の .claude.json で承認済みにする）。前後で本物の ~/.claude*/skills・plugins の一覧と
# ハッシュ・クリップボードのハッシュを比べる。本番 tako が配る env は最初に落とす。落とすのは自分で起こした pid と
# tmux ソケットだけ。
#
# 使い方: bash scripts/test-mod-chat-1964.sh   （ONLY=static|visual|term|gui|ab|tmux）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は mod の `claude plugin test`・
# Rust の単体テストと番犬（issue1964_mod_chat_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1877_S2_LEGACY TAKO_1877_S3_LEGACY \
  TAKO_1877_S7_LEGACY TAKO_1963_LEGACY TAKO_1964_LEGACY TAKO_1881_BAND_THRESHOLD TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
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
now_s() { perl -MTime::HiRes=time -e 'printf "%.1f", time'; }
since() { perl -e "printf '%.1f', $(now_s) - $1"; }

command -v claude >/dev/null 2>&1 || { echo "未実測: PATH に claude が無い"; exit 3; }
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
snap_clipboard() { /usr/bin/pbpaste 2>/dev/null | shasum -a 256 | cut -c1-16; }
REAL_BEFORE="$(snap_real)"
CLIP_BEFORE="$(snap_clipboard)"

# --- static（ビルドの前に回せる）-------------------------------------------------------------
if want static; then
  echo "== static: claude plugin validate --strict / claude plugin test（scripts/check-claude-mod.sh）"
  if out="$(bash "$REPO_ROOT/scripts/check-claude-mod.sh" 2>&1)"; then
    pass "$(printf '%s' "$out" | grep '^結果' | head -1)"
  else
    fail "check-claude-mod.sh が落ちた"
    printf '%s\n' "$out" | tail -15 | sed 's/^/      | /'
  fi
  [ "$ONLY" = static ] && { echo "== 結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"; [ "$FAIL" -eq 0 ]; exit $?; }
fi

# --- ビルド（visual-test の節を含む GUI と CLI）--------------------------------------------------
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-app --features visual-test && run_heavy cargo build -q -p tako-cli) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

TMP="$(mktemp -d /tmp/tako-1964-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
FAKE_PID=""
TMUX_SOCKET="tako-1964-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }
tmx() { tmux -u -L "$TMUX_SOCKET" "$@"; }
OUT="${OUT_DIR:-$TMP/out}"
mkdir -p "$OUT/visual-new" "$OUT/visual-legacy" "$OUT/capture"

cleanup() {
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
API_KEY="sk-ant-api03-fake1964fake1964fake1964fake1964fake1964fake1964AA"
# ペインの claude の設定 dir。**GUI には渡さない** = accounts.yaml にも GUI の env にも無い「未登録の設定 dir」
CCD="$TMP/cfg"
mkdir -p "$CCD"
jq -n --arg key "$(printf '%s' "$API_KEY" | tail -c 20)" --arg work "$WORK" '{
  hasCompletedOnboarding: true, theme: "dark",
  customApiKeyResponses: { approved: [$key], rejected: [] },
  projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
}' > "$CCD/.claude.json"
# GUI 自身の設定 dir（claude の版の判定などが本物の ~/.claude を使わないように別の一時 dir）
GUI_CCD="$TMP/cfg-gui"
mkdir -p "$GUI_CCD"
cp "$CCD/.claude.json" "$GUI_CCD/.claude.json"

# 偽の pbcopy（受け取った文字列をファイルへ。本物のクリップボードへ書かない）
FAKEBIN="$TMP/fakebin"
mkdir -p "$FAKEBIN"
printf '#!/bin/sh\ncat > %s/pbcopy.out\n' "$TMP" > "$FAKEBIN/pbcopy"
chmod +x "$FAKEBIN/pbcopy"

# 他の mod の代役 1: ui.copy を拒む（GUI のペインで Copy を押しても本物のクリップボードへ届かない）
OTHER_REFUSE="$TMP/mods/other-refuse"
mkdir -p "$OTHER_REFUSE/.claude-plugin" "$OTHER_REFUSE/hooks"
printf '{"name":"other-refuse","version":"0.0.1","description":"refuses ui.copy for the #1964 check"}\n' > "$OTHER_REFUSE/.claude-plugin/plugin.json"
printf '{ "modules": ["./register.ts"] }\n' > "$OTHER_REFUSE/hooks/hooks.json"
cat > "$OTHER_REFUSE/hooks/register.ts" <<'EOF'
import type { Register } from 'claude-code'
export const register: Register = on => {
  on('ui.copy', () => ({ value: { isCopied: false, reason: 'refused' } })).catch(($, e, next) => next(e))
}
EOF
# 他の mod の代役 2: 同じ描画の口（発話・返答・ツールの行）を next で包み、印の行を足す
OTHER_WRAP="$TMP/mods/other-wrap"
mkdir -p "$OTHER_WRAP/.claude-plugin" "$OTHER_WRAP/hooks"
printf '{"name":"other-wrap","version":"0.0.1","description":"wraps chat rows for the #1964 check"}\n' > "$OTHER_WRAP/.claude-plugin/plugin.json"
printf '{ "modules": ["./register.ts"] }\n' > "$OTHER_WRAP/hooks/hooks.json"
cat > "$OTHER_WRAP/hooks/register.ts" <<'EOF'
import type { Register } from 'claude-code'
export const register: Register = on => {
  on('ui.render', { component: 'UserMessage' }, async ($, e, next) => {
    const below = await next(e)
    const { Box, Text } = $.ui.resolve(e)
    return Box({ flexDirection: 'column', children: [Text({ dimColor: true, children: '[other-user]' }), below] })
  }).catch(($, e, next) => next(e))
  on('ui.render', { component: 'AssistantMessage' }, async ($, e, next) => {
    const below = await next(e)
    const { Box, Text } = $.ui.resolve(e)
    return Box({ flexDirection: 'column', children: [Text({ dimColor: true, children: '[other-reply]' }), below] })
  }).catch(($, e, next) => next(e))
  on('ui.render', { component: 'ToolUse' }, async ($, e, next) => {
    const below = await next(e)
    const { Box, Text } = $.ui.resolve(e)
    return Box({ flexDirection: 'column', children: [Text({ dimColor: true, children: '[other-tool]' }), below] })
  }).catch(($, e, next) => next(e))
}
EOF

# mod が叩く CLI（TAKO_CLI）の薄い包み: 合図のファイルがある間だけ `mod report` を失敗させる（= 報告が途切れた状態。
# claude は生かしたまま）
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
LONG='cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long-data-dir-name ./target/debug/tako mod --json | jq ".panes[] | {pane, reason}"'

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
  rm -rf "$TAKO_DATA_DIR" && mkdir -p "$TAKO_DATA_DIR"
  ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-0,0,1400,900}"
  launch_isolated_gui "$TMP/app-${name}.log" TAKO_PERSIST=0 CLAUDE_CONFIG_DIR="${GUI_CFG:-$GUI_CCD}" \
    ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  tk autorename off >/dev/null 2>&1
}
stop_app() {
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
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
reporting() { [ "$(row_field "$1" '.state')" = reporting ]; }
not_reporting() { [ "$(row_field "$1" '.state')" != reporting ]; }
mod_dir() { mod_json | jq -r '.plugin_dir'; }
read_pane() { tk read --pane "$1" 2>/dev/null; }
screen_has() { read_pane "$1" | grep -Eq -- "$2"; }
# 吹き出し = `╮` で終わる行と `╰` の行の間（枠の中）に発話と `│`（他の mod の行が中に包まれていても見つける）
has_bubble() { read_pane "$1" | awk -v said="$2" '/╮ *$/ { inside = 1; next } /╰/ { inside = 0 } inside && index($0, said) && /│/ { found = 1 } END { exit !found }'; }
display_of() { tk ui-mode 2>/dev/null | jq -r --arg p "$1" '"\(.pane_display[$p])/\(.pane_display_reason[$p].reason // "-")"'; }
display_is() { [ "$(display_of "$1")" = "$2" ]; }
display_not() { [ "$(display_of "$1")" != "$2" ]; }
chat_owner() { row_field "$1" '.chat | "\(.drawn_by)/\(.reason)"'; }
# 今あてている組（bubble/code_copy/tool_summary/notify。無ければ null）
renders_chat() { row_field "$1" '.report.renders.chat // null | if . == null then "null" else "\(.bubble)/\(.code_copy)/\(.tool_summary)/\(.notify)" end'; }
renders_is() { [ "$(renders_chat "$1")" = "$2" ]; }
chat_list() { tk chat copy --list --pane "$1" 2>/dev/null; }
chat_has() { chat_list "$1" | jq -e --arg w "$2" '.messages | any(.preview | contains($w))' >/dev/null 2>&1; }
save_pane() { read_pane "$1" | mask > "$2"; }
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
# 文を入れてから Enter を別に送る（文と改行を 1 度に受けると貼り付けとして改行を入れることがある）
say() {
  tk send --pane "$1" --no-newline "$2" >/dev/null
  sleep 1
  tk send --pane "$1" --no-newline $'\r' >/dev/null
}

# 隔離 GUI の直接ペインで claude を起こす（ペインの env = tako の注入はそのまま。mod が叩く CLI だけ包みへ）
gui_claude() {
  local pane="$1" extra="${2:-}" plugins="${3:-}"
  if reporting "$pane"; then quit_claude "$pane"; fi
  # API キーはペインの env（GUI の env を継ぐ）にある = 打つ文に載せない（鍵らしき文字の確認 #2050 に掛けない）
  tk send --pane "$pane" "cd '$WORK' && clear && env PATH='$FAKEBIN':\$PATH TAKO_CLI='$CLI_GATE' CLAUDE_CONFIG_DIR='$CCD' ANTHROPIC_BASE_URL='$API_URL' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 $extra claude --model haiku $plugins --permission-mode default --allowedTools Bash" >/dev/null
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
# 2 ターン（Bash の呼び出し → コードブロックの返答）を流す。最新の返答にコードがある = 帯の「最新のコードを
# コピー」が出る順にする
two_turns() {
  local pane="$1"
  say "$pane" "Run this command: touch made-1964.txt && echo made"
  wait_for 40 screen_has "$pane" 'RUN-DONE' || { fail "Bash の呼び出しが終わらない"; read_pane "$pane" | mask | tail -8 | sed 's/^/      | /'; return 1; }
  say "$pane" "Reply with code blocks"
  wait_for 40 screen_has "$pane" 'print\("hello from tako"\)' || { fail "コードブロックの返答が出ない"; return 1; }
}

# --- visual（受け入れ 1 / 4: 並べた画像・クリックとキー）--------------------------------------------
run_visual() {
  local arm="$1" dump="$2"
  shift 2
  # 絵の比較は GPUI のチャットが会話を読めることが前提なので、ここだけペインの設定 dir を GUI にも渡す
  # （未登録の設定 dir の扱いは gui / ab の段が見る。A/B の腕が ② より前の前提で落ちないように）
  GUI_CFG="$CCD" start_app "visual-$arm" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=mod-chat TAKO_VISUAL_DUMP_DIR="$dump" ${@+"$@"}
  local p
  p="$(root_pane)"
  tk ui-mode gui >/dev/null
  # ui.copy を拒む mod を外側に同居させる（Copy を押しても本物のクリップボードへ届かない）
  if gui_claude "$p" "" "--plugin-dir '$OTHER_REFUSE'"; then
    two_turns "$p" || true
  fi
  local rc=0
  wait "$APP_PID" || rc=$?
  APP_PID=""
  return $rc
}
phase_visual() {
  echo "== 1 / 4. visual-test mod-chat: 同じ会話の GPUI のチャットと mod の描画を撮る・Copy をクリック / キーで押す"
  local rc=0 log="$TMP/app-visual-new.log"
  run_visual new "$OUT/visual-new" || rc=$?
  grep 'TAKO_VISUAL_1964\|TAKO_APP_SELF_TEST_FAILED' "$log" | mask | sed 's/^/    観測: /' | cut -c1-400
  check_eq "節が終了コード 0 で終わる（①〜④）" "0" "$rc"
  check_eq "絵を 2 枚書き出した（GPUI のチャット / mod の描画）" "2" "$(ls "$OUT/visual-new"/mod-chat-*.png 2>/dev/null | wc -l | tr -d ' ')"
  echo "== A/B: TAKO_1964_LEGACY=1（同一バイナリ）は ② で落ちる"
  rc=0
  log="$TMP/app-visual-legacy.log"
  run_visual legacy "$OUT/visual-legacy" TAKO_1964_LEGACY=1 || rc=$?
  grep 'TAKO_VISUAL_1964\|TAKO_APP_SELF_TEST_FAILED' "$log" | mask | sed 's/^/    観測: /' | cut -c1-400
  check_eq "A/B: 節が非ゼロで終わる" "true" "$([ "$rc" -ne 0 ] && echo true || echo false)"
  check_eq "A/B: 落ちた理由は chat.place mod でもチャットを mod に任せないこと（②）" "true" \
    "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test mod-chat ②' "$log" && echo true || echo false)"
}

# --- term（受け入れ 3）・gui（フォールバック側・受け入れ 1 / 2）------------------------------------------
phase_term_gui() {
  local p t0 dt before after
  p="$(root_pane)"
  if want term; then
    echo "== 3. ターミナル表示（既定）では吹き出し・ツールの要約を描かず、コードのコピーだけ"
    check_eq "表示モードの既定は terminal" "terminal" "$(tk ui-mode | jq -r '.ui_mode')"
    gui_claude "$p" || return 1
    two_turns "$p" || return 1
    sleep 2
    save_pane "$p" "$OUT/capture/term.txt"
    check_eq "発話は吹き出しにしない" "false" "$(has_bubble "$p" 'Reply with code blocks' && echo true || echo false)"
    check_eq "コードブロックの下に Copy（コピー 1 bash / コピー 2 python）" "true true" \
      "$(screen_has "$p" '(コピー|Copy) 1 bash' && echo true || echo false) $(screen_has "$p" '(コピー|Copy) 2 python' && echo true || echo false)"
    check_eq "ツールは要約にしない（エンジンの畳んだ行のまま）" "false" "$(screen_has "$p" 'Bash touch made-1964.txt' && echo true || echo false)"
    check_eq "報告の renders.chat はコードのコピーだけ・通知しない" \
      'false/true/false/false' "$(renders_chat "$p")"
    check_eq "持ち主は tako（mode_terminal）" "tako/mode_terminal" "$(chat_owner "$p")"
  fi
  want gui || return 0
  if ! reporting "$p"; then gui_claude "$p" || return 1; two_turns "$p" || return 1; fi
  echo "== フォールバック側（#1883 の置き換え）: GUI モードの GPUI のチャットが未登録の設定 dir の会話を読み、通知で映す"
  tk ui-mode gui >/dev/null
  if wait_for 10 display_is "$p" "chat/-"; then pass "GUI モードの既定（chat.place auto）は GPUI のチャット"; else fail "GPUI のチャットにならない（$(display_of "$p")）"; fi
  check_eq "持ち主は tako（place_auto = 判断点が決まるまで）" "tako/place_auto" "$(chat_owner "$p")"
  if wait_for 15 chat_has "$p" "RUN-DONE"; then
    pass "accounts.yaml にも GUI の env にも無い設定 dir の claude の会話を GPUI のチャットが読めた（$(chat_list "$p" | jq '.total') 件）"
  else
    fail "未登録の設定 dir の会話を読めない（$(chat_list "$p" 2>&1 | head -c 200)）"
  fi
  if wait_for 20 sh -c "'$TAKO_BIN' mod --json | jq -e '.panes[] | select(.pane == $p) | .report.renders.chat.notify == true' >/dev/null"; then
    pass "GUI モードの GPUI のチャットのペインでは mod が通知する（renders.chat.notify）"
  else
    fail "通知に切り替わらない（$(renders_chat "$p")）"
  fi
  before="$(row_field "$p" '.report.reports')"
  t0="$(now_s)"
  say "$p" "Reply with exactly one word: NOTIFY1964"
  local reflected=""
  for _ in $(seq 1 100); do
    if chat_has "$p" "NOTIFY1964"; then reflected="$(since "$t0")"; break; fi
    sleep 0.2
  done
  sleep 3
  after="$(row_field "$p" '.report.reports')"
  if [ -n "$reflected" ]; then
    pass "新しい発話が送ってから ${reflected} 秒で GPUI のチャットに出た（このターンの報告 $((after - before)) 回）"
    if perl -e "exit !($reflected <= 6.0)"; then pass "通知（1 秒の flush）+ 1 tick（2 秒）+ 偽 API の往復の範囲（≤ 6 秒）"; else fail "映るまでが遅い（${reflected} 秒）"; fi
  else
    fail "新しい発話が GPUI のチャットに出ない"
  fi
  echo "== 1. chat.place mod でそのペインがターミナル表示（mod の描画）になり、吹き出し・Copy・要約が画面に出る"
  tk mod ui set chat.place mod >/dev/null || fail "tako mod ui set chat.place mod が通らない"
  t0="$(now_s)"
  if wait_for 40 display_is "$p" "terminal/mod_chat"; then
    pass "$(since "$t0") 秒でターミナル表示（理由 mod_chat・持ち主 $(chat_owner "$p")）"
  else
    fail "ターミナル表示にならない（$(display_of "$p") / $(chat_owner "$p")）"
  fi
  wait_for 20 has_bubble "$p" "Reply with code blocks" || true
  # mod は gui の組を受け取った次の報告（1 秒の flush）で renders.chat を改める
  wait_for 10 renders_is "$p" 'true/true/true/false' || true
  save_pane "$p" "$OUT/capture/gui-mod.txt"
  check_eq "画面に吹き出し・Copy・ツールの要約" "true true true" \
    "$(has_bubble "$p" 'Reply with code blocks' && echo true || echo false) $(screen_has "$p" '(コピー|Copy) 1 bash' && echo true || echo false) $(screen_has "$p" 'Bash touch made-1964.txt' && echo true || echo false)"
  check_eq "報告の renders.chat は gui の組（通知しない = GPUI のチャットは裏）" \
    'true/true/true/false' "$(renders_chat "$p")"
  echo "== 2. mod が止まると GPUI のチャットへ戻る（報告が途切れる = 45 秒 + 1 tick / claude が終わる = 1 tick）"
  # 報告が途切れた後の GPUI のチャットは `claude agents --json` の走査で会話を引く = 走査が知る設定 dir
  # （accounts.yaml）でないと会話を引けず、チャットにならない（mod が無ければ未登録の設定 dir は見えない）。
  # ここは普段の構成（登録済みの設定 dir）で測るため、隔離した orchestrator dir の accounts.yaml にだけ登録する
  printf 'accounts:\n  t1964:\n    config_dir: %s\n' "$CCD" > "$TAKO_ORCHESTRATOR_DIR/accounts.yaml"
  : > "$PAUSE_REPORTS"
  t0="$(now_s)"
  local back=""
  for _ in $(seq 1 350); do
    if display_is "$p" "chat/-"; then back="$(since "$t0")"; break; fi
    sleep 0.2
  done
  if [ -n "$back" ]; then
    local age
    age="$(row_field "$p" '.report.age_ms')"
    pass "報告を止めて ${back} 秒で GPUI のチャットへ戻った（戻った時点の最後の報告からの経過 ${age} ms・持ち主 $(chat_owner "$p")）"
    if [ "${age:-0}" -ge 45000 ] && [ "${age:-0}" -le 48500 ]; then pass "失効（45 秒）から 1 tick 以内"; else fail "失効と合わない（${age} ms）"; fi
  else
    fail "報告を止めても GPUI のチャットへ戻らない（$(display_of "$p")）"
  fi
  rm -f "$PAUSE_REPORTS"
  if wait_for 30 display_is "$p" "terminal/mod_chat"; then pass "報告が戻ると mod の描画へ戻る"; else fail "報告が戻っても mod の描画へ戻らない（$(display_of "$p")）"; fi
  exit_claude "$p"
  t0="$(now_s)"
  back=""
  for _ in $(seq 1 100); do
    if display_not "$p" "terminal/mod_chat"; then back="$(since "$t0")"; break; fi
    sleep 0.1
  done
  if [ -n "$back" ]; then
    pass "claude を終えて ${back} 秒で mod の描画をやめた（表示 $(display_of "$p")）"
    if perl -e "exit !($back <= 4.5)"; then pass "1 tick（2 秒）+ claude の終了処理の範囲（≤ 4.5 秒）"; else fail "claude の終了から ${back} 秒（1 tick より遅い）"; fi
  else
    fail "claude を終えても mod の描画のまま（$(display_of "$p")）"
  fi
  wait_for 30 not_reporting "$p" || true
  rm -f "$TAKO_ORCHESTRATOR_DIR/accounts.yaml"
  tk mod ui reset >/dev/null 2>&1
  tk ui-mode terminal >/dev/null 2>&1
}

# --- ab（A/B: TAKO_1964_LEGACY=1）---------------------------------------------------------------
phase_ab() {
  echo "== A/B: TAKO_1964_LEGACY=1（同一バイナリ）= chat.place mod でも GPUI のチャットのまま・未登録の設定 dir は読めない"
  stop_app
  start_app legacy TAKO_1964_LEGACY=1
  local p
  p="$(root_pane)"
  check_eq "tako mod の chat.legacy" "true" "$(mod_json | jq -r '.chat.legacy')"
  tk ui-mode gui >/dev/null
  tk mod ui set chat.place mod >/dev/null
  gui_claude "$p" || return 1
  two_turns "$p" || return 1
  sleep 20
  save_pane "$p" "$OUT/capture/legacy.txt"
  check_eq "A/B: 持ち主は tako（legacy_env）" "tako/legacy_env" "$(chat_owner "$p")"
  check_eq "A/B: mod はチャットを描かない（renders.chat なし・吹き出し / Copy なし）" "null false false" \
    "$(renders_chat "$p") $(has_bubble "$p" 'Reply with code blocks' && echo true || echo false) $(screen_has "$p" '(コピー|Copy) 1' && echo true || echo false)"
  check_eq "A/B: 未登録の設定 dir の会話は GPUI のチャットに出ない（#1964 前）" "false" "$(chat_has "$p" 'RUN-DONE' && echo true || echo false)"
  quit_claude "$p"
  stop_app
}

# --- tmux（Copy の中身・他の mod との共存）--------------------------------------------------------
start_tmux_claude() {
  local name="$1" pane="$2" dirs="$3" plugins="${4:-}"
  tmx kill-session -t "=$name" >/dev/null 2>&1
  # shellcheck disable=SC2086
  tmx new-session -d -s "$name" -x 144 -y 44 -c "$WORK" \
    env PATH="$FAKEBIN:$PATH" TAKO_ISOLATED=1 TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" \
    TAKO_ORCHESTRATOR_DIR="$TAKO_ORCHESTRATOR_DIR" CLAUDE_CONFIG_DIR="$CCD" \
    ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    TAKO_PANE_ID="$pane" TAKO_CLI="$CLI_GATE" CLAUDE_CODE_PLUGIN_DIRS="$dirs" \
    claude --model haiku $plugins --permission-mode default --allowedTools Bash
  # tmux へ届く OSC 52 をどこへも渡さない（クライアントも付けない）
  tmx set-option -g set-clipboard off >/dev/null 2>&1
}
tcap() { tmx capture-pane -p -t "=$1:" 2>/dev/null; }
tscreen_has() { tcap "$1" | grep -Eq -- "$2"; }
tkey() {
  tmx send-keys -t "=$1:" "$2"
  sleep 0.6
}
tsay() {
  tmx send-keys -t "=$1:" -l "$2"
  sleep 0.6
  tmx send-keys -t "=$1:" Enter
}
tbubble() { tcap "$1" | awk -v said="$2" '/╮ *$/ { inside = 1; next } /╰/ { inside = 0 } inside && index($0, said) && /│/ { found = 1 } END { exit !found }'; }
phase_tmux() {
  echo "== Copy の中身（隔離 tmux の claude）と、他の mod が同じ描画の口を使うときの共存"
  local p dirs s row copied
  p="$(root_pane)"
  dirs="$(mod_dir)"
  tk ui-mode gui >/dev/null
  tk mod ui set chat.place mod >/dev/null
  for order in outer inner; do
    s="wrap-$order"
    if [ "$order" = outer ]; then
      # --plugin-dir は env の注入より外側（読み込み順 = 外から --plugin-dir → CLAUDE_CODE_PLUGIN_DIRS）
      start_tmux_claude "$s" "$p" "$dirs" "--plugin-dir $OTHER_WRAP"
    else
      start_tmux_claude "$s" "$p" "$dirs:$OTHER_WRAP"
    fi
    if ! wait_for 60 reporting "$p"; then fail "（${order}）tmux の claude の報告が届かない"; continue; fi
    tsay "$s" "Reply with code blocks"
    wait_for 40 tscreen_has "$s" 'print\("hello from tako"\)' || true
    tsay "$s" "Run this command: touch made-$order.txt && echo made"
    wait_for 40 tscreen_has "$s" 'RUN-DONE' || true
    wait_for 25 tbubble "$s" "Run this command" || true
    tcap "$s" | mask > "$OUT/capture/tmux-$order.txt"
    check_eq "（他の mod が ${order}）他の mod の印が発話・返答・ツールの行に残る" "true true true" \
      "$(tscreen_has "$s" '\[other-user\]' && echo true || echo false) $(tscreen_has "$s" '\[other-reply\]' && echo true || echo false) $(tscreen_has "$s" '\[other-tool\]' && echo true || echo false)"
    check_eq "（他の mod が ${order}）tako の吹き出し・Copy・要約も出る" "true true true" \
      "$(tbubble "$s" 'Run this command' && echo true || echo false) $(tscreen_has "$s" '(コピー|Copy) 1 bash' && echo true || echo false) $(tscreen_has "$s" "Bash touch made-$order.txt" && echo true || echo false)"
    if [ "$order" = inner ]; then
      # 帯の「最新のコードをコピー」をキーで → 最新の返答（RUN-DONE）にはコードが無いので、もう一度コードの返答を出す
      tsay "$s" "Reply with code blocks"
      wait_for 40 sh -c "tmux -u -L '$TMUX_SOCKET' capture-pane -p -t '=$s:' | grep -Eq 'o: (最後のコードをコピー|copy last code)'" || true
      rm -f "$TMP/pbcopy.out"
      tkey "$s" Escape
      tkey "$s" C-x
      tkey "$s" Tab
      tkey "$s" o
      wait_for 10 test -f "$TMP/pbcopy.out" || true
      check_eq "帯のキー（ctrl+x → Tab → o）は最新の返答の最後のコードを写す" 'print("hello from tako")' "$(cat "$TMP/pbcopy.out" 2>/dev/null)"
      tkey "$s" Escape
      # 会話の行の Copy 1 を SGR のクリックで押す（描かれて 300 ms 以内のクリックは捨てられるので待ってから）
      sleep 1
      row="$(tcap "$s" | grep -nE '(コピー|Copy) 1 bash' | tail -1 | cut -d: -f1)"
      rm -f "$TMP/pbcopy.out"
      if [ -n "$row" ]; then
        tmx send-keys -t "=$s:" -l $'\e[<0;6;'"$row"'M'
        sleep 0.1
        tmx send-keys -t "=$s:" -l $'\e[<0;6;'"$row"'m'
        wait_for 10 test -f "$TMP/pbcopy.out" || true
      fi
      copied="$(cat "$TMP/pbcopy.out" 2>/dev/null)"
      check_eq "会話の行の Copy 1 のクリックは論理文字列そのまま" "$LONG" "$copied"
      check_eq "長さ 157 字・改行 0（画面は折り返していても）" "157 0" \
        "$(printf '%s' "$copied" | wc -m | tr -d ' ') $(printf '%s' "$copied" | tr -cd '\n' | wc -c | tr -d ' ')"
      wait_for 5 sh -c "'$TAKO_BIN' mod --json | jq -e '.panes[] | select(.pane == $p) | .report.last_press.kind == \"code_copy\"' >/dev/null" || true
      check_eq "報告の last_press は code_copy で成功（本文は載せない）" "code_copy/true/false" \
        "$(row_field "$p" '.report.last_press | "\(.kind)/\(.ok)"')/$(mod_json | grep -q 'cargo build' && echo true || echo false)"
    fi
    tmx kill-session -t "=$s" >/dev/null 2>&1
    wait_for 30 not_reporting "$p" || true
  done
  tk mod ui reset >/dev/null 2>&1
  tk ui-mode terminal >/dev/null 2>&1
}

# --- 実行 ----------------------------------------------------------------------------------------
echo "claude: $(claude --version 2>/dev/null | head -1) / 作業 dir: $(printf '%s' "$TMP" | mask)"
want visual && phase_visual
if want term || want gui || want tmux; then
  start_app main
  echo "  隔離 GUI: pid $APP_PID / mod: $(mod_json | jq -c '{injecting, claude_version, chat}' | mask)"
  if want term || want gui; then phase_term_gui; fi
  want tmux && phase_tmux
  stop_app
fi
want ab && phase_ab

REAL_AFTER="$(snap_real)"
check_eq "本物の ~/.claude*/skills・plugins は前後で同じ（${REAL_BEFORE}）" "$REAL_BEFORE" "$REAL_AFTER"
check_eq "本物のクリップボードは前後で同じ（中身は読まずハッシュだけ）" "$CLIP_BEFORE" "$(snap_clipboard)"
echo "採取: $(printf '%s' "$OUT/capture" | mask)・絵: $(printf '%s' "$OUT" | mask)/visual-{new,legacy}"
find "$OUT" -name '*.png' | sort | mask | sed 's/^/  /'
grep -c '"path"' "$FAKE_LOG" | sed 's/^/  偽 API への要求: /'
echo "== 結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
