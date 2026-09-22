#!/usr/bin/env bash
# #1536: UI の絵文字を描画プリミティブ（SVG）へ置き換えた箇所が、実ピクセルで
# 描かれていることを隔離 GUI で確かめる。
#
# 見るもの:
#   1. 右パネル fleet の BG ペイン行のドラッグ用グリップ（`⠿` → `ui_icon::GRIP`）
#   2. 同じ行の個別復帰ボタン（`⬆` → `ui_icon::UNSHELVE`）
#   3. プレビュー検索の置換欄の印（`↔` → `ui_icon::SWAP`。**この PR で新設したアセット**）
#
# なぜ実画面が要るか: SVG のパス定数を足しても `EMBEDDED_ASSETS` へ登録し忘れると
# GPUI は**無言で何も描かない**（#562 で実在した）。ソースからは
# 「絵文字が消えた」と「アイコンが出ている」を区別できない。
#
# 実装は visual-test の節（`TAKO_VISUAL_ONLY=no-emoji`）。Metal の scene を読み戻すので
# **画面収録権限が要らない**（`screencapture -l` はこの機では
# "could not create image from window" で撮れない = 実測）。
#
# CI には登録しない（実ディスプレイが要る）。窓は常設の仮想ディスプレイ `tako-vd` へ出す
# （#1141 / #1150）。落とすのは自分で起こしたプロセスだけ。
#
# **PNG をそのまま共有しない**（#927）: 左のターミナルペインには実行した機の
# シェルプロンプト（ユーザー名・ホスト名）が写る。このスクリプトは隔離 HOME と
# 固定プロンプトを渡して防いでいるが、共有前に必ず中身を目で確かめること。
set -uo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO_ROOT" || exit 1
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

TMP=${TAKO_1536_TMP:-$(mktemp -d "${TMPDIR:-/tmp}/tako-1536-XXXXXX")}
OUT=${TAKO_1536_OUT:-$TMP/shots}
mkdir -p "$OUT" "$TMP/home" "$TMP/zdot"
PASS=0; FAIL=0
pass() { PASS=$((PASS+1)); printf '  OK   %s\n' "$1"; }
fail() { FAIL=$((FAIL+1)); printf '  NG   %s\n' "$1"; }

# visual-test の節は feature 付きビルドにしか無い（素のビルドだと「未知の節」で exit 1）
cargo build -p tako-app --features visual-test --quiet || exit 1

# 起動は必ずヘルパ経由（#1490）。仮想ディスプレイの ensure・置き先・バイナリの決定は
# `scripts/lib/isolated-gui.sh` の 1 実装が持つので、ここで手書きしない
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
trap 'stop_isolated_gui' EXIT

# 証拠 PNG に実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

echo "=== 隔離 GUI で no-emoji 節を回す ==="
# 節は自分で `exit 0` して終わるので、起こしてから終わるのを待つ
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
launch_isolated_gui "$TMP/run.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DATA_DIR="$TMP/data" TAKO_DISCOVERY_DIR="$TMP/disc" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_TMUX_SOCKET="tako-1536-$$" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=no-emoji TAKO_VISUAL_DUMP_DIR="$OUT" || exit 1
wait "$ISOLATED_GUI_PID"
RC=$?
ISOLATED_GUI_PID=""

[ "$RC" -eq 0 ] && pass "節が終了コード 0 で終わった" || fail "終了コード ${RC}（ログ: $TMP/run.log）"
grep -q '未知の節' "$TMP/run.log" && fail "visual-test 付きでビルドされていない（節が無い）"
grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/run.log" && pass "TAKO_VISUAL_TEST_OK" || fail "OK 行が出ない"

LINE=$(grep -m1 'TAKO_VISUAL_1536:' "$TMP/run.log")
printf '  観測: %s\n' "${LINE:-（出ていない）}"
case "$LINE" in
  *"restore_rect=true"*) pass "復帰ボタンの実矩形が登録された（BG ペイン行が描かれた）" ;;
  *) fail "復帰ボタンの矩形が無い（行が出ていない）" ;;
esac
# `painted=Some((色数, 幅, 高さ))`。単色なら「登録されただけで何も描いていない」
COLORS=$(printf '%s' "$LINE" | sed -n 's/.*painted=Some((\([0-9][0-9]*\),.*/\1/p')
if [ -n "$COLORS" ] && [ "$COLORS" -ge 3 ]; then
  pass "復帰ボタンの矩形に $COLORS 色ある（アイコンが実際に塗られている）"
else
  fail "復帰ボタンの矩形が単色（SVG が描かれていない = EMBEDDED_ASSETS 漏れ?）"
fi
[ -s "$OUT/no-emoji-panel-preview.png" ] && pass "スクショ: $OUT/no-emoji-panel-preview.png" \
  || fail "PNG が落ちていない"

echo
printf '%s PASS / %s FAIL  （出力: %s）\n' "$PASS" "$FAIL" "$OUT"
[ "$FAIL" -eq 0 ]
