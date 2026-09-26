#!/usr/bin/env bash
# test-jump-keys-1677.sh — ジャンプ履歴のキー（⌃- / ⌃⇧-・Windows は Ctrl+Alt+← / →）の
# 実 GUI 経路テスト（#1677 / FR-3.29）
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、visual-test 節 `jump-keys` を
#   ① そのまま走らせる → 行指定で a.rs:120 → b.rs:40 と開き、端末にフォーカスを置いたまま
#      戻るキーで a.rs:120 へ・進むキー（macOS は US の `ctrl-_` と JIS の `ctrl-=` の両方）で
#      b.rs:40 へ移り、端で押しても動かないことがすべて緑
#   ② A/B `TAKO_1677_LEGACY=1`（行指定の open が何も積まない）で走らせる →
#      最初の「戻る」の相で FAILED（= 節に検出力がある）
# を実測する。打鍵は `window.dispatch_keystroke` へ流すので、キーバインド判定 →
# アクション → `jump_step` → dispatch `Jump` の全段を通る（AX は使わない = #1442）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-jump-keys-1677.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1677k-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# visual-test 版をビルドする（同じ target/debug/tako-app を上書きする）
echo "visual-test 版の tako-app をビルドします…"
(cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1677k-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local log="$1"
  shift
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=jump-keys ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  return 0
}

echo "① jump-keys 節"
run_section "$TMP/new.log"
grep "TAKO_VISUAL_PIXEL: jump-keys" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi

echo "② A/B: TAKO_1677_LEGACY=1（行指定の open が何も積まない = #1677 以前）"
run_section "$TMP/legacy.log" TAKO_1677_LEGACY=1
grep "TAKO_VISUAL_PIXEL: jump-keys" "$TMP/legacy.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test jump-keys" "$TMP/legacy.log"; then
  pass "旧経路では落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/legacy.log" | head -1 | cut -c1-120)"
else
  fail "旧経路でも落ちない（節に検出力が無い）"
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
