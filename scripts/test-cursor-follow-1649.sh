#!/bin/bash
# test-cursor-follow-1649.sh — 編集カーソルの追従スクロールを実機で測る（#1649）
#
# 何を測るか:
#   4,000 行のファイル（プレビューの編集上限 MAX_LINES = 5,000 行の内側）を編集モードで
#   開き、検索ヒット / 打鍵 / 矢印連打 / undo /
#   IME の変換開始のそれぞれの後に**カーソル行が可視範囲に入っているか**を、
#   器（`ListState`）の実測値（先頭可視行・末尾可視行）から読む。
#   実ピクセルではなくレイアウト値なので画面収録の権限が要らない。
#
# A/B:
#   同じバイナリを 2 回走らせる。`TAKO_1649_LEGACY=1` が #1649 前の挙動（追わない）で、
#   **旧挙動の側も節が自分で期待値（ok=false）を持つ**ので両腕とも終了コード 0 で終わる。
#   片方でも期待とずれたら節が非ゼロで落ちる = この節の検出力の実証。
#
# 実装は visual-test の節（`TAKO_VISUAL_ONLY=cursor-follow`）。CI には登録しない
# （実ディスプレイが要る）。窓は常設の仮想ディスプレイ `tako-vd` へ出す（#1141 / #1150）。
# 落とすのは自分で起こしたプロセスだけ（名前一致では殺さない）。
set -uo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO_ROOT" || exit 1
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

TMP=${TAKO_1649_TMP:-$(mktemp -d "${TMPDIR:-/tmp}/tako-1649-XXXXXX")}
mkdir -p "$TMP/home" "$TMP/zdot"
PASS=0; FAIL=0
pass() { PASS=$((PASS+1)); printf '  OK   %s\n' "$1"; }
fail() { FAIL=$((FAIL+1)); printf '  NG   %s\n' "$1"; }

cargo build -p tako-app --features visual-test --quiet || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
trap 'stop_isolated_gui' EXIT

# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# $1 = ログの置き場 / 残りは追加の env（`VAR=VAL`）
run_section() {
    local log="$1"; shift
    ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
    launch_isolated_gui "$log" \
        HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
        TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
        TAKO_DATA_DIR="$TMP/data" TAKO_DISCOVERY_DIR="$TMP/disc" \
        TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
        TAKO_TMUX_SOCKET="tako-1649-$$" \
        TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=cursor-follow \
        ${1+"$@"} || return 1
    wait "$ISOLATED_GUI_PID"
    local rc=$?
    ISOLATED_GUI_PID=""
    return $rc
}

echo "=== (A) 追従あり（既定） ==="
run_section "$TMP/run.log"
RC=$?
[ "$RC" -eq 0 ] && pass "節が終了コード 0 で終わった" || fail "終了コード ${RC}（ログ: $TMP/run.log）"
grep -q '未知の節' "$TMP/run.log" 2>/dev/null && fail "visual-test 付きでビルドされていない（節が無い）"
grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/run.log" 2>/dev/null \
    && pass "TAKO_VISUAL_TEST_OK" || fail "OK 行が出ない"
grep 'TAKO_VISUAL_PIXEL: cursor-follow' "$TMP/run.log" 2>/dev/null | sed 's/^/  観測: /'
# 7 相（edit-start / search-tail / typed / arrows / search-head / undo / ime）が出る。
# `ensure_fresh_scene` が出す premise 行は相ではないので数に入れない
PHASE_RE='cursor-follow (edit-start|search-tail|typed|arrows|search-head|undo|ime) '
PHASES=$(grep -cE "$PHASE_RE" "$TMP/run.log" 2>/dev/null)
[ "${PHASES:-0}" -eq 7 ] && pass "7 相すべてが走った" || fail "相が ${PHASES:-0} 本しか出ていない"
# 画面外へ送る相（ime の before）を除き、カーソル行は毎回可視に戻る
OKS=$(grep -cE "$PHASE_RE.*ok=true" "$TMP/run.log" 2>/dev/null)
[ "${OKS:-0}" -ge 7 ] && pass "7 相すべてで ok=true（観測 ${OKS}）" \
    || fail "ok=true が ${OKS:-0} 相しかない"
grep -q 'cursor-follow ime .*anchored=true' "$TMP/run.log" 2>/dev/null \
    && pass "画面外カーソルでも IME の下線アンカーが残る" || fail "IME のアンカーが採れていない"

echo "=== (B) TAKO_1649_LEGACY=1（#1649 前の挙動 = 追わない） ==="
run_section "$TMP/legacy.log" TAKO_1649_LEGACY=1
RC=$?
[ "$RC" -eq 0 ] && pass "節が終了コード 0 で終わった（旧挙動の側も自分で判定する）" \
    || fail "終了コード ${RC}（ログ: $TMP/legacy.log）"
grep 'TAKO_VISUAL_PIXEL: cursor-follow' "$TMP/legacy.log" 2>/dev/null | sed 's/^/  観測: /'
LPHASES=$(grep -cE "$PHASE_RE" "$TMP/legacy.log" 2>/dev/null)
[ "${LPHASES:-0}" -eq 7 ] && pass "旧挙動でも 7 相すべてが走った" \
    || fail "旧挙動の相が ${LPHASES:-0} 本しか出ていない"
LOKS=$(grep -cE "$PHASE_RE.*ok=false" "$TMP/legacy.log" 2>/dev/null)
[ "${LOKS:-0}" -eq 7 ] && pass "旧挙動は 7 相すべてで ok=false（この節の検出力）" \
    || fail "旧挙動で ok=false が ${LOKS:-0} 相しかない"
grep -q 'cursor-follow search-tail .*ok=false' "$TMP/legacy.log" 2>/dev/null \
    && pass "旧挙動では検索ヒットが画面外に残る（この節の検出力）" \
    || fail "旧挙動でも ok=true（節が追従を測っていない）"
grep -q 'cursor-follow ime .*anchored=false' "$TMP/legacy.log" 2>/dev/null \
    && pass "旧挙動では画面外カーソルで IME の下線アンカーを失う（この節の検出力）" \
    || fail "旧挙動でも anchored=true（節が IME のアンカーを測っていない）"

printf '\n結果: PASS=%d FAIL=%d（ログ: %s）\n' "$PASS" "$FAIL" "$TMP"
[ "$FAIL" -eq 0 ] || exit 1
