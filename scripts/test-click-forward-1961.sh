#!/usr/bin/env bash
# test-click-forward-1961.sh — TUI への左クリックの転送（#1961。FR-2.5.15）の実クリック検証
#
# クリックは実 OS マウスと同じ `PlatformInput` 経路（GPUI のヒットテストとリスナー配線まで通る = #738）で
# 隔離 GUI（tako-vd・AX は使わない）の visual-test の節が押す。番号は Issue #1961 の受け入れ条件:
#   visual  2 / 3 / 4. 合成のアプリ（受けたバイトを見せる可視化アプリ）で、直接ペイン（TAKO_PERSIST=0）と
#           器のペイン（TAKO_PERSIST=1 = send-keys -H の経路）の両方を回す（節 `click-forward`。
#           ① クリック ② ペインの外まで出るドラッグ ③ ダブルクリック ④ Option / Shift + ドラッグ = tako の選択
#           ⑤ cmd + クリック ⑥ 設定 off → on がすぐ効く ⑦ フォーカスを移すクリック ⑧ マウスを要求しないアプリ
#           ⑨ X10 の素の形式）。旧挙動（TAKO_1961_LEGACY=1）は ① で FAILED になることも確かめる
#   claude  1. 実物の Claude Code（PATH の claude）を**一時の設定 dir** と**偽の Messages API**
#           （scripts/lib/fake-messages-api.mjs）で動かし、Jump to bottom・mod の帯の compact /
#           split right・AI が出したコマンドカード（#1963）の r: run をクリックで押す
#           （節 `click-forward-claude`）。旧挙動は ① で FAILED
#
# 画像は新旧で書き出し先を分ける（$OUT/<arm>-<mode>/*.png）。**共有する前に必ず中身を目で確かめる**
# （#927。隔離 HOME と固定プロンプトで実ユーザー名・ホスト名は写らないようにしてある）。
# **本物の設定 dir（~/.claude*）には書かない**（前後で skills / plugins の一覧とハッシュを比べる）。
# 本番 tako が配る env は最初に落とす。落とすのは自分で起こした pid と tmux ソケットだけ。
# 偽 API へ向けた claude の mod の Copy ボタンは本物のクリップボードへ書くので押さない（押すのは
# Jump to bottom / compact / split right だけ）。tako の選択の場面は離すを「窓の外で離した」形で
# 終えるので copy-on-select も走らない。
#
# 使い方: bash scripts/test-click-forward-1961.sh   （ONLY=visual|claude / KEEP=1 で一時 dir を残す）
# **CI には登録しない**（仮想ディスプレイと実物の claude が要る）。CI 側の担保は tako-core の単体テストと
# 器の e2e（tmux_backend::クリックのtmux直接注入…）・番犬（issue1961_click_forward_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TAKO_1961_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
# 自分の claude が使っている本物の設定 dir・認証を混ぜない
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
# CLI も隔離 GUI だけを見る（TAKO_DATA_DIR だけでは spawn 系の CLI が本番 GUI を触る）
export TAKO_ISOLATED=1

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

# --- 本物の設定 dir の前後比較（#1962 と同じ物差し）---------------------------------------
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

# --- ビルド（HOME を差し替える前に。rebase 後の古いバイナリを掴まない）---------------------
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-app --features visual-test && run_heavy cargo build -q -p tako-cli) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

TMP="$(mktemp -d /tmp/tako-1961-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
OUT="${TAKO_1961_OUT:-$TMP/shots}"
APP_PID=""
FAKE_PID=""
SOCKETS=""
cleanup() {
  local s
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  for s in $SOCKETS; do
    tmux -L "$s" kill-server >/dev/null 2>&1 || true
    rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$s"
  done
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# 証拠 PNG に実ユーザー名・実ホスト名を写さない（#927）
mkdir -p "$TMP/home" "$TMP/zdot" "$OUT"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 隔離 GUI を 1 本起こす（arm = new / legacy、mode = direct / tmux、section = 節の名前）。
# 追加の env は 5 番目以降に VAR=VAL で渡す
launch() {
  local arm="$1" mode="$2" section="$3" bounds="$4"
  shift 4
  # 短い札（データ dir の下に IPC のソケットが立つ = unix ソケットのパス長 104 字に収める）
  local kind=v
  case "$section" in *claude) kind=c ;; esac
  local tag="${arm:0:1}${mode:0:1}${kind}"
  local persist=0 sock="tako-1961-$$-$tag"
  [ "$mode" = tmux ] && persist=1
  SOCKETS="$SOCKETS $sock"
  local dir="$OUT/$arm-$mode"
  mkdir -p "$dir" "$TMP/d-$tag"
  local legacy=()
  [ "$arm" = legacy ] && legacy=(TAKO_1961_LEGACY=1)
  ISOLATED_GUI_BOUNDS="$bounds"
  export TAKO_DATA_DIR="$TMP/d-$tag"
  export TAKO_DISCOVERY_DIR="$TMP/q-$tag"
  export TAKO_TMUX_SOCKET="$sock"
  launch_isolated_gui "$TMP/$arm-$mode-$section.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
    TAKO_PERSIST="$persist" TAKO_AUTORENAME=0 \
    TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
    TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" TAKO_VISUAL_DUMP_DIR="$dir" \
    ${legacy[@]+"${legacy[@]}"} ${@+"$@"} || exit $?   # 関数の中でも exit で止める（4 = 未実測）
  APP_PID="$ISOLATED_GUI_PID"
}

# 起こした GUI の終わりを待ち、新しい挙動は OK・旧挙動は ① で FAILED を期待する
finish() {
  local arm="$1" mode="$2" section="$3" rc log="$TMP/$1-$2-$3.log"
  wait "$APP_PID"
  rc=$?
  APP_PID=""
  grep -E 'TAKO_VISUAL_1961|TAKO_VISUAL_TEST_OK|TAKO_APP_SELF_TEST_FAILED' "$log" | sed 's/^/      | /' | cut -c1-400
  if [ "$arm" = new ]; then
    if [ "$rc" = 0 ] && grep -q TAKO_VISUAL_TEST_OK "$log"; then
      pass "${section}（${mode}）: 新しい挙動で全段 OK"
    else
      fail "${section}（${mode}）: 新しい挙動で落ちた（rc=${rc}）"
      tail -5 "$log" | sed 's/^/      ! /' | cut -c1-400
    fi
  else
    if [ "$rc" != 0 ] && grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test click-forward[a-z-]* ①' "$log"; then
      pass "${section}（${mode}）: 旧挙動（TAKO_1961_LEGACY=1）は ① で落ちる = A/B の検出力"
    else
      fail "${section}（${mode}）: 旧挙動が ① で落ちない（rc=${rc}）"
    fi
  fi
}

# --- visual: 合成のアプリ -------------------------------------------------------------------
if want visual; then
  echo "== visual: 合成のアプリへの実クリック（直接ペイン / 器のペイン × 新旧）"
  for mode in direct tmux; do
    for arm in new legacy; do
      launch "$arm" "$mode" click-forward 0,0,1400,880
      finish "$arm" "$mode" click-forward
    done
  done
fi

# --- claude: 実物の Claude Code ---------------------------------------------------------------
if want claude; then
  echo "== claude: 実物の Claude Code（一時の設定 dir + 偽 API）で Jump to bottom・帯の split right・カードの Run / 閉じる・compact"
  REAL_CLAUDE="$(command -v claude 2>/dev/null || true)"
  if [ -z "$REAL_CLAUDE" ] || ! command -v node >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
    unmeasured "claude / node / jq のどれかが無い"
  else
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
    API_URL="http://127.0.0.1:${PORT}"
    # ダミーの API キー（本物ではない。一時の .claude.json で末尾 20 字を承認済みにする）
    API_KEY="sk-ant-api03-fake1961fake1961fake1961fake1961fake1961fake1961AA"
    CCD="$TMP/cfg"
    mkdir -p "$CCD"
    # tako の MCP（`tako mcp serve` = 呼び出し元ペインは TAKO_PANE_ID）を登録 = AI がカードを出せる（#1963 と同じ）
    jq -n --arg key "$(printf '%s' "$API_KEY" | tail -c 20)" --arg work "$WORK" --arg bin "$TAKO_BIN" '{
      hasCompletedOnboarding: true, theme: "dark",
      customApiKeyResponses: { approved: [$key], rejected: [] },
      projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } },
      mcpServers: { tako: { type: "stdio", command: $bin, args: ["mcp", "serve"], env: {} } }
    }' > "$CCD/.claude.json"
    tk() { "$TAKO_BIN" "$@"; }
    reporting() { [ "$(tk mod --json 2>/dev/null | jq -r --argjson p "$1" '.panes[] | select(.pane == $p) | .state' 2>/dev/null)" = reporting ]; }
    wait_for() {
      local limit="$1" i
      shift
      for i in $(seq 1 "$limit"); do
        if "$@"; then return 0; fi
        sleep 1
      done
      return 1
    }
    for arm in new legacy; do
      [ -n "$PORT" ] || { unmeasured "偽 API が立たない"; break; }
      launch "$arm" direct click-forward-claude 0,0,1600,1000 \
        CLAUDE_CONFIG_DIR="$CCD" ANTHROPIC_BASE_URL="$API_URL" ANTHROPIC_API_KEY="$API_KEY" \
        DISABLE_AUTOUPDATER=1 TAKO_1961_READY_TEXT=LINE-80
      if ! wait_isolated_gui "$TMP/$arm-direct-click-forward-claude.log"; then
        fail "claude（${arm}）: 隔離 GUI の CLI が繋がらない"
        stop_isolated_gui "$APP_PID"
        APP_PID=""
        continue
      fi
      tk autorename off >/dev/null 2>&1
      if ! wait_for 30 sh -c "'$TAKO_BIN' mod --json | jq -e '.injecting == true' >/dev/null"; then
        unmeasured "claude（${arm}）: tako が新しいペインへ mod を注入しない"
        stop_isolated_gui "$APP_PID"
        APP_PID=""
        continue
      fi
      tk mod ui button add split-right >/dev/null
      q="$(tk tab new --focus 2>/dev/null | jq -r '.pane // empty')"
      sleep 2
      tk send --pane "$q" "cd '$WORK' && clear && env CLAUDE_CONFIG_DIR='$CCD' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 claude --model haiku --allowedTools mcp__tako__tako_show_command" >/dev/null
      if wait_for 90 reporting "$q"; then
        tk send --pane "$q" "Reply with 80 numbered lines" >/dev/null 2>&1
      else
        fail "claude（${arm}）: claude の報告が届かない"
      fi
      finish "$arm" direct click-forward-claude
    done
    grep -c '"summarize":true' "$FAKE_LOG" | sed 's/^/      | 要約の要求（compact）: /'
    grep -c '"tool":"' "$FAKE_LOG" | sed 's/^/      | AI のツール呼び出し（tako_show_command）: /'
    grep -c '"path"' "$FAKE_LOG" | sed 's/^/      | 偽 API への要求: /'
  fi
fi

REAL_AFTER="$(snap_real)"
if [ "$REAL_BEFORE" = "$REAL_AFTER" ]; then
  pass "本物の ~/.claude*/skills・plugins は前後で同じ（${REAL_AFTER}）"
else
  fail "本物の ~/.claude* が変わった（$REAL_BEFORE → ${REAL_AFTER}）"
fi
echo "PNG: $OUT"
find "$OUT" -name '*.png' | sort | sed 's/^/  /'
echo "結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 3
exit 0
