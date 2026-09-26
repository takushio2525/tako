#!/usr/bin/env bash
# test-remote-autostart-1485.sh — remote daemon の自動復帰の実経路テスト（#1485）
#
# 隔離した state / data ディレクトリで **実 tako-app（GUI）+ 実 daemon** を走らせ、
#   ① `remote start` → daemon と GUI を落とす（= Mac 再起動）→ GUI 再起動 →
#      **手を触れずに** running: true へ戻る
#   ② `remote stop` → GUI 再起動 → 立ち上がらない（desired が消えている）
#   ③ 前提が崩れた状態（serve バイナリが失敗する = tailscaled 不在に相当）で GUI 起動 →
#      理由が persist.log に出て、前提が戻ったあとのリトライで復旧する
#   ④ 諦めたときに理由が `tako remote status` に残る（`last_autostart.result=failed`）
#   ⑤ A/B（`TAKO_1485_LEGACY=1`）で旧挙動（再起動後に running: false のまま）が再現する
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定には一切触らない**
# （`TAKO_REMOTE_TEST_MODE=1` で実 serve を張らず、state / data は mktemp 配下、
# 落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ（`tako-vd`）へ出す。
#
# 使い方: bash scripts/test-remote-autostart-1485.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 で実測）。tako のペインの中から
# 走らせると `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` が継承され、
# CLI が隔離インスタンスではなく**ユーザーの本番 GUI** を触る
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1485-XXXXXX")"
APP_PID=""
TMUX_SOCKET="tako-iso-1485-$$"

# 明示 pid だけを落とす（`pkill -f tako` は本番 GUI にも当たる = memory の事故）
kill_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
kill_daemon() {
  local pid
  pid="$(head -1 "$STATE_DIR/tako-remote.pid" 2>/dev/null)"
  if [ -n "$pid" ]; then kill -9 "$pid" 2>/dev/null; fi
}
cleanup() {
  kill_app
  kill_daemon
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  rm -rf "$TMP"
}
trap cleanup EXIT

STATE_DIR="$TMP/state"
DATA_DIR="$TMP/data"
BIN_DIR="$TMP/bin"
mkdir -p "$STATE_DIR" "$DATA_DIR" "$BIN_DIR" "$TMP/disc"

echo "== ビルド =="
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# **この本は毎回ビルドし直す**（`isolated_gui_bins` は無いときだけ作るので、
# stale な target を掴んだまま「緑」になるのを防ぐ = #432 の罠）
cargo build -q -p tako-app -p tako-cli 2>&1 | tail -3
isolated_gui_bins || exit 1
# **リポジトリの target を書き換えない**（③ の注入で serve バイナリを差し替えるため、
# 検証用の複製を作ってそちらを差し替える）。隔離モードの `serve_binary()` は
# tako-app と**同じディレクトリの `tako`** を選ぶので、2 つを並べて置く
cp "$APP_BIN" "$BIN_DIR/tako-app"
cp "$TAKO_BIN" "$BIN_DIR/tako"
cp "$TAKO_BIN" "$TMP/tako-cli"   # 検査に使う CLI（注入の影響を受けない）
APP_BIN="$BIN_DIR/tako-app"   # 起動するのは複製（③ が serve バイナリを差し替える）
CLI="$TMP/tako-cli"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$DATA_DIR"
export TAKO_REMOTE_STATE_DIR="$STATE_DIR"
export TAKO_REMOTE_TEST_MODE=1
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DISCOVERY_DIR="$TMP/disc"
# 検証用のバックオフ（既定は 2/5/10/20/40 秒 = 合計 77 秒。実測を現実的な時間で回す）
export TAKO_1485_BACKOFF_SECS=2
export TAKO_1485_ATTEMPTS=8

start_app() {
  # 面を起こすのは launch_isolated_gui の中（#1490。①〜⑤ で何度も起動し直すので、
  # 蓋閉じで tako-vd が眠ると 2 回目以降が窓を開かずに終わる = #1160）
  launch_isolated_gui "$TMP/app.log" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  sleep 5
}

status_field() {
  "$CLI" remote status 2>/dev/null | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
except Exception:
    print('')
    raise SystemExit
cur = d
for k in '$1'.split('.'):
    if not isinstance(cur, dict) or k not in cur:
        print('')
        raise SystemExit
    cur = cur[k]
print(cur)
"
}

# 条件が満たされるまで待つ（最大 $2 秒）
wait_for() {
  local field="$1" want="$2" budget="$3" i
  for ((i = 0; i < budget * 2; i++)); do
    [ "$(status_field "$field")" = "$want" ] && return 0
    sleep 0.5
  done
  return 1
}

echo
echo "== ① 起動していたら、GUI 再起動で手を触れずに戻る =="
start_app
"$CLI" remote start >/dev/null 2>&1
check_eq "remote start で daemon が立つ" "True" "$(status_field running)"
check_eq "「起動していた」が記録される" "True" "$(status_field desired)"

# Mac の再起動を再現: daemon は SIGKILL（pid ファイルだけ残る = 実測の症状）・GUI も落とす
kill_daemon
kill_app
sleep 1
check_eq "再起動相当のあとは止まっている" "False" "$(status_field running)"
check_eq "意図は残っている（daemon の死で消えない）" "True" "$(status_field desired)"

start_app
if wait_for running True 30; then
  pass "**手を触れずに** running: true へ戻る"
else
  fail "自動復帰しない（#1485 の症状のまま）"
fi
check_eq "結果が記録される" "started" "$(status_field last_autostart.result)"
check_contains "persist.log に 1 行残る" "$(cat "$DATA_DIR/persist.log" 2>/dev/null)" "リモート自動復帰: 結果=started"

echo
echo "== ② stop してあれば GUI 再起動でも立ち上がらない =="
"$CLI" remote stop >/dev/null 2>&1
check_eq "stop で意図が消える" "False" "$(status_field desired)"
kill_app
: > "$DATA_DIR/persist.log"
start_app
sleep 8
check_eq "GUI を再起動しても立ち上がらない" "False" "$(status_field running)"
check_eq "意図は無いまま" "False" "$(status_field desired)"
if grep -aq "リモート自動復帰" "$DATA_DIR/persist.log" 2>/dev/null; then
  fail "意図が無い環境で記録を書いている（関係ない人の data_dir を汚す）"
else
  pass "意図が無ければ記録も残さない"
fi

echo
echo "== ③ 前提が崩れていたら理由を残し、戻ったら復旧する =="
"$CLI" remote start >/dev/null 2>&1
check_eq "もう一度 start して意図を立てる" "True" "$(status_field desired)"
kill_daemon
kill_app
sleep 1
# 前提を崩す: serve バイナリを「必ず失敗する」ものへ差し替える
# （tailscaled 不在と同じく「daemon が立ち上がれない」状態。**本番の serve は張らない**）
cp "$BIN_DIR/tako" "$TMP/tako-real-serve"
cat > "$BIN_DIR/tako" <<'STUB'
#!/bin/sh
echo "error: Tailscale が見つかりません（検証注入: 前提が崩れた状態）" >&2
exit 1
STUB
chmod +x "$BIN_DIR/tako"
: > "$DATA_DIR/persist.log"
start_app
sleep 7
LOG="$(cat "$DATA_DIR/persist.log" 2>/dev/null)"
check_contains "再試行の間も無音にしない" "$LOG" "失敗（再試行します）"
check_contains "理由が persist.log に出る" "$LOG" "Tailscale が見つかりません"
check_eq "まだ立っていない" "False" "$(status_field running)"
# 前提を戻す（tailscaled が遅れて上がった状態）
cp "$TMP/tako-real-serve" "$BIN_DIR/tako"
chmod +x "$BIN_DIR/tako"
if wait_for running True 20; then
  pass "前提が戻ったあとのリトライで復旧する"
else
  fail "前提が戻っても復旧しない"
fi
check_eq "復旧が記録される" "started" "$(status_field last_autostart.result)"
ATTEMPTS="$(status_field last_autostart.attempts)"
if [ "${ATTEMPTS:-0}" -ge 2 ]; then
  pass "何回目で成ったかが残る（attempts=${ATTEMPTS}）"
else
  fail "試行回数が残っていない（attempts=${ATTEMPTS}）"
fi

echo
echo "== ④ 諦めたら理由が status に残る =="
kill_daemon
kill_app
sleep 1
cp "$BIN_DIR/tako" "$TMP/tako-real-serve"
cat > "$BIN_DIR/tako" <<'STUB'
#!/bin/sh
echo "error: Tailscale が見つかりません（検証注入: 前提が崩れた状態）" >&2
exit 1
STUB
chmod +x "$BIN_DIR/tako"
: > "$DATA_DIR/persist.log"
TAKO_1485_ATTEMPTS=3 start_app
if wait_for last_autostart.result failed 30; then
  pass "上限まで試して諦める"
else
  fail "諦めた記録が残らない"
fi
check_eq "試行回数が残る" "3" "$(status_field last_autostart.attempts)"
check_eq "分類が残る" "spawn-failed" "$(status_field last_autostart.reason)"
check_contains "理由が残る" "$(status_field last_autostart.detail)" "Tailscale が見つかりません"
check_contains "CLI が理由を 1 行で出す" "$("$CLI" remote status 2>&1 >/dev/null)" "前回の起動状態へ自動で戻せませんでした"
cp "$TMP/tako-real-serve" "$BIN_DIR/tako"
chmod +x "$BIN_DIR/tako"

echo
echo "== ⑤ A/B（TAKO_1485_LEGACY=1）で旧挙動が再現する =="
kill_app
sleep 1
: > "$DATA_DIR/persist.log"
TAKO_1485_LEGACY=1 start_app
sleep 8
check_eq "旧挙動では戻らない（#1485 の症状）" "False" "$(status_field running)"
check_eq "意図は残ったまま" "True" "$(status_field desired)"
check_eq "降りた理由が残る" "disabled" "$(status_field last_autostart.reason)"

echo
echo "== 結果 =="
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
