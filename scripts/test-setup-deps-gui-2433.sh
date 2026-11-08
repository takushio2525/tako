#!/usr/bin/env bash
# #2433: 設定画面「セットアップ」の「任意の依存ツール」の行を隔離 GUI で描いて撮る（Windows の腕を macOS の上で）。
#
# 見るもの（visual-test の節 `setup-deps-2433`）:
#   ① psmux（利用者の権限）と Git（UAC）が未導入 = ボタンが 2 つ。Git は「導入（管理者の確認あり）」と書く
#      （押すことが UAC への同意。CLI / MCP と同じ `tako_setup_deps` の install を consent=true で送る）
#   ② psmux が入った後 = psmux はパスを出してボタンを消し、Git のボタンだけが残る
#
# 一覧は検証のプロセスだけが読む `TAKO_2433_DEPS_FIXTURE`（`tako setup deps --json` の `deps` と同じ形）で
# 差し替える。節が置き場へ書いてタブを切り替えて読み直させる。**ボタンは押さない**（本物の brew / winget を起こす）。
#
# CI には登録しない（実ディスプレイが要る）。窓は常設の仮想ディスプレイ `tako-vd` へ出す（#1141 / #1150）。
# 撮るのは Metal の scene の読み戻し（画面収録の権限は要らない）。落とすのは自分で起こしたプロセスだけ。
# 共有する前に PNG の中身を目で確かめる（#927。ここは設定画面だけを撮る）
set -uo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO_ROOT" || exit 1
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

TMP=${TAKO_2433_TMP:-$(mktemp -d "${TMPDIR:-/tmp}/tako-2433-XXXXXX")}
OUT=${TAKO_2433_OUT:-$TMP/shots}
mkdir -p "$OUT" "$TMP/home" "$TMP/zdot"
PASS=0; FAIL=0
pass() { PASS=$((PASS+1)); printf '  OK   %s\n' "$1"; }
fail() { FAIL=$((FAIL+1)); printf '  NG   %s\n' "$1"; }

# visual-test の節は feature 付きビルドにしか無い。起動は必ずヘルパ経由（#1490）
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1
trap 'stop_isolated_gui' EXIT

printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

echo "=== 隔離 GUI で setup-deps-2433 節を回す ==="
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1200,800}
launch_isolated_gui "$TMP/run.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DATA_DIR="$TMP/data" TAKO_DISCOVERY_DIR="$TMP/disc" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_TMUX_SOCKET="tako-2433-$$" \
    TAKO_2433_DEPS_FIXTURE="$TMP/deps.json" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=setup-deps-2433 TAKO_VISUAL_DUMP_DIR="$OUT" || exit $?
wait "$ISOLATED_GUI_PID"
RC=$?
ISOLATED_GUI_PID=""

[ "$RC" -eq 0 ] && pass "節が終了コード 0 で終わった" || fail "終了コード ${RC}（ログ: $TMP/run.log）"
grep -q '未知の節' "$TMP/run.log" && fail "visual-test 付きでビルドされていない（節が無い）"
grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/run.log" && pass "TAKO_VISUAL_TEST_OK" || fail "OK 行が出ない"
grep 'visual-test setup-deps-2433' "$TMP/run.log" | sed 's/^/  ログ: /'

LINE=$(grep -m1 'TAKO_2433_GUI:' "$TMP/run.log")
printf '  観測: %s\n' "${LINE:-（出ていない）}"
case "$LINE" in
  *"after_install_psmux_btn=false"*) pass "導入済みの psmux にはボタンが無い" ;;
  *) fail "導入済みの psmux のボタンが消えていない" ;;
esac
for png in setup-deps-2433-both.png setup-deps-2433-psmux-installed.png; do
  [ -s "$OUT/$png" ] && pass "スクショ: $OUT/$png" || fail "$png が落ちていない"
done

echo
printf '%s PASS / %s FAIL  （出力: %s）\n' "$PASS" "$FAIL" "$OUT"
[ "$FAIL" -eq 0 ]
