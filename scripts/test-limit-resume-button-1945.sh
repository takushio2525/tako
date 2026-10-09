#!/usr/bin/env bash
# test-limit-resume-button-1945.sh — ステータスバーの自動復帰の一括ボタンを実マウスで押す（#1945）
#
# 実装は visual-test の節（`TAKO_VISUAL_ONLY=limit-resume-all`）。合成マウスで押して、
# 退避中を含む全エージェントのペインが揃うこと・表示が 3 状態（全部 OFF / 全部 ON / 一部）で
# 切り替わることを見て、各状態の実フレームを PNG で残す（Metal の scene を読み戻すので
# 画面収録権限が要らない = #1536 と同じ）。
#
#   新: bash scripts/test-limit-resume-button-1945.sh            → $OUT/new/*.png
#   旧: TAKO_1945_LEGACY=1 bash scripts/test-limit-resume-button-1945.sh
#       → ボタンを描かない（#1945 前の画面）ので ① で FAILED になり、$OUT/legacy/ に旧の 1 枚が残る
#
# CI には登録しない（実ディスプレイが要る）。窓は常設の仮想ディスプレイ `tako-vd` へ出す
# （#1141 / #1150）。落とすのは自分で起こしたプロセスだけ。
#
# **PNG をそのまま共有しない**（#927）: 隔離 HOME と固定プロンプトを渡して防いでいるが、
# 共有前に必ず中身を目で確かめること。
set -uo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO_ROOT" || exit 1
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

ARM=new
[ "${TAKO_1945_LEGACY:-}" = "1" ] && ARM=legacy
TMP=${TAKO_1945_TMP:-$(mktemp -d "${TMPDIR:-/tmp}/tako-1945b-XXXXXX")}
OUT=${TAKO_1945_OUT:-$TMP/shots}/$ARM
mkdir -p "$OUT" "$TMP/home" "$TMP/zdot"

# visual-test の節は feature 付きビルドにしか無い（素のビルドだと「未知の節」で exit 1）
cargo build -p tako-app --features visual-test --quiet || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
trap 'stop_isolated_gui' EXIT

# 証拠 PNG に実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

echo "=== 隔離 GUI で limit-resume-all 節を回す（${ARM}）==="
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
launch_isolated_gui "$TMP/run-$ARM.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DATA_DIR="$TMP/data-$ARM" TAKO_DISCOVERY_DIR="$TMP/disc" \
    TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_TMUX_SOCKET="tako-1945b-$$" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=limit-resume-all TAKO_VISUAL_DUMP_DIR="$OUT" || exit $?
wait "$ISOLATED_GUI_PID"
RC=$?
ISOLATED_GUI_PID=""

grep -E 'TAKO_VISUAL_1945|TAKO_VISUAL_TEST_OK|TAKO_APP_SELF_TEST_FAILED' "$TMP/run-$ARM.log"
echo "PNG: $OUT"
ls "$OUT"
exit "$RC"
