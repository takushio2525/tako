#!/usr/bin/env bash
# test-drag-autoscroll-2025.sh — ドラッグ選択のオートスクロール（#310）が、送れる履歴が無いときに
# 選択の端を (0, 0) へ寄せないか（#2025）・最下部で行末へ寄せないか（#2153）の実クリック検証
#
# マウスは実 OS マウスと同じ `PlatformInput` 経路（GPUI のヒットテストとリスナー配線まで通る = #738）で
# 隔離 GUI（tako-vd・AX は使わない）の visual-test の節 `drag-autoscroll` が押す。場面は節の頭の説明:
#   ① 履歴なし ② 1 画面未満の履歴（a. 最上端まで送る b. 最上端でマウスを動かす）③ 1 画面以上の履歴
#   ④ alt screen ⑤ 最下部（履歴あり）⑥ 履歴の途中（a. 最下部まで送る b. 最下部でマウスを動かす）
# 直接ペイン（TAKO_PERSIST=0）は新旧の A/B: 新しい挙動は全段 OK・旧挙動 #2025（TAKO_2025_LEGACY=1）は
# ①・②b・⑤・⑥b だけで落ち、旧挙動 #2153（TAKO_2153_LEGACY=1 = 下へ送る側だけ旧）は ⑤・⑥b だけで落ちる
# （②a / ③ / ④ / ⑥a = #310 の送りと alt screen は旧と同じ）。
# 器のペイン（TAKO_PERSIST=1）は新しい挙動で ①④⑤ が OK になることを見て、②③ と旧挙動は観察を出す。
#
# 画像は新旧で書き出し先を分ける（$OUT/<arm>-<mode>/*.png）。**共有する前に必ず中身を目で確かめる**
# （#927。隔離 HOME と固定プロンプトで実ユーザー名・ホスト名は写らないようにしてある）。
# 選択は「窓の外で離した」形で終えるので copy-on-select は走らない（本物のクリップボードに触らない）。
# 本番 tako が配る env は最初に落とす。落とすのは自分で起こした pid と tmux ソケットだけ。
#
# 使い方: bash scripts/test-drag-autoscroll-2025.sh   （ONLY=direct|tmux / KEEP=1 で一時 dir を残す）
# **CI には登録しない**（仮想ディスプレイが要る）。CI 側の担保は tako-app の単体テスト
# （drag_scroll_tests::issue2025_* / issue2153_* と番犬 issue2025_tick_が判定を通す_watchdog・
# issue2153_tick_が下端で寄せない_watchdog）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TAKO_2025_LEGACY TAKO_2153_LEGACY
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
# CLI も隔離 GUI だけを見る（TAKO_DATA_DIR だけでは spawn 系の CLI が本番 GUI を触る）
export TAKO_ISOLATED=1

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
ONLY="${ONLY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

# --- ビルド（HOME を差し替える前に。rebase 後の古いバイナリを掴まない）---------------------
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-app --features visual-test && run_heavy cargo build -q -p tako-cli) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

TMP="$(mktemp -d /tmp/tako-2025-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
OUT="${TAKO_2025_OUT:-$TMP/shots}"
APP_PID=""
SOCKETS=""
cleanup() {
  local s
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  for s in $SOCKETS; do
    tmux -L "$s" kill-server >/dev/null 2>&1 || true
    rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$s"
  done
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# 証拠 PNG に実ユーザー名・実ホスト名を写さない（#927）
mkdir -p "$TMP/home" "$TMP/zdot" "$OUT"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 隔離 GUI を 1 本起こして終わりを待つ（arm = new / legacy（#2025）/ legacy2153、mode = direct / tmux）。
# rc は $RC、ログは $LOG
run_arm() {
  local arm="$1" mode="$2"
  local tag
  case "$arm" in
    new) tag="n" ;;
    legacy) tag="l" ;;
    *) tag="b" ;;
  esac
  tag="$tag${mode:0:1}"
  local persist=0 sock="tako-2025-$$-$tag"
  [ "$mode" = tmux ] && persist=1
  SOCKETS="$SOCKETS $sock"
  local dir="$OUT/$arm-$mode"
  mkdir -p "$dir" "$TMP/d-$tag"
  local legacy=()
  [ "$arm" = legacy ] && legacy=(TAKO_2025_LEGACY=1)
  [ "$arm" = legacy2153 ] && legacy=(TAKO_2153_LEGACY=1)
  LOG="$TMP/$arm-$mode.log"
  ISOLATED_GUI_BOUNDS=0,0,1400,880
  export TAKO_DATA_DIR="$TMP/d-$tag"
  export TAKO_DISCOVERY_DIR="$TMP/q-$tag"
  export TAKO_TMUX_SOCKET="$sock"
  launch_isolated_gui "$LOG" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
    TAKO_PERSIST="$persist" TAKO_AUTORENAME=0 \
    TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
    TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=drag-autoscroll TAKO_VISUAL_DUMP_DIR="$dir" \
    ${legacy[@]+"${legacy[@]}"} || exit $?   # 関数の中でも exit で止める（4 = 未実測）
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  RC=$?
  APP_PID=""
  grep -E 'TAKO_VISUAL_2025|TAKO_VISUAL_TEST_OK|TAKO_APP_SELF_TEST_FAILED' "$LOG" | sed 's/^/      | /' | cut -c1-400
}

ok_log() { [ "$RC" = 0 ] && grep -q TAKO_VISUAL_TEST_OK "$LOG"; }
failed_line() { grep 'TAKO_APP_SELF_TEST_FAILED: visual-test drag-autoscroll' "$LOG" | head -1; }

if want direct; then
  echo "== direct: 直接ペイン × 新旧"
  run_arm new direct
  if ok_log; then
    pass "直接ペイン: 新しい挙動で全段 OK（①〜⑥）"
  else
    fail "直接ペイン: 新しい挙動で落ちた（rc=${RC}）"
    tail -5 "$LOG" | sed 's/^/      ! /' | cut -c1-400
  fi
  run_arm legacy direct
  line="$(failed_line)"
  # 旧挙動は ①・②b・⑤・⑥b で落ち、②a / ③ / ④ / ⑥a は通る（落ちた段の一覧に「②a」「③」「④」「⑥a」が無い）
  if [ "$RC" != 0 ] && grep -q '①' <<<"$line" && grep -q '②b' <<<"$line" \
    && grep -q '⑤' <<<"$line" && grep -q '⑥b' <<<"$line" \
    && ! grep -q -e '②a' -e '③' -e '④' -e '⑥a' <<<"$line"; then
    pass "直接ペイン: 旧挙動 #2025（TAKO_2025_LEGACY=1）は ①・②b・⑤・⑥b だけで落ちる = A/B の検出力"
  else
    fail "直接ペイン: 旧挙動 #2025 の落ち方が想定と違う（rc=${RC}・${line:-FAILED 行なし}）"
  fi
  run_arm legacy2153 direct
  line="$(failed_line)"
  # 下へ送る側だけ旧 = ⑤ と ⑥b だけで落ち、上端（①②）・③④・⑥a は通る
  if [ "$RC" != 0 ] && grep -q '⑤' <<<"$line" && grep -q '⑥b' <<<"$line" \
    && ! grep -q -e '①' -e '②' -e '③' -e '④' -e '⑥a' <<<"$line"; then
    pass "直接ペイン: 旧挙動 #2153（TAKO_2153_LEGACY=1）は ⑤・⑥b だけで落ちる = A/B の検出力"
  else
    fail "直接ペイン: 旧挙動 #2153 の落ち方が想定と違う（rc=${RC}・${line:-FAILED 行なし}）"
  fi
fi

if want tmux; then
  echo "== tmux: 器のペイン（新しい挙動は ①④⑤ を判定・旧挙動は観察）"
  run_arm new tmux
  if ok_log; then
    pass "器のペイン: 新しい挙動で ①④⑤ が OK"
  else
    fail "器のペイン: 新しい挙動で落ちた（rc=${RC}）"
    tail -5 "$LOG" | sed 's/^/      ! /' | cut -c1-400
  fi
  run_arm legacy tmux
  echo "      （観察）器のペインの旧挙動: rc=${RC}・$(failed_line | cut -c1-300)"
fi

echo "PNG: $OUT"
find "$OUT" -name '*.png' | sort | sed 's/^/  /'
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ] || exit 1
exit 0
