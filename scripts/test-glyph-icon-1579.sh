#!/usr/bin/env bash
# #1579: 絵文字ではないグリフ（`×` U+00D7 / `⎇` U+2387 / `●` U+25CF）を印代わりに
# 使っていた箇所を描画プリミティブへ替えた。その印が**実ピクセルで描かれている**ことを
# 隔離 GUI で確かめる。
#
# 見るもの（ピン留めプレビューのタイトルバー = ライブ印と閉じるが並ぶ唯一の場所）:
#   1. ライブ印（`● LIVE` → 図形の丸 + 文字ラベル `LIVE`）
#   2. 閉じる（`×` → `svg().path(ui_icon::CLOSE)`）
#
# なぜ実画面が要るか: SVG のパス定数を使っても `EMBEDDED_ASSETS` へ登録されていなければ
# GPUI は**無言で何も描かない**（#562 で実在した）。ソースからは「グリフが消えた」と
# 「印が出ている」を区別できない。#1536 の `no-emoji` 節と同じ形。
#
# `ui_asset!` を外す A/B（アセット登録が効いていることの裏取り）:
#   1. `crates/tako-app/src/file_icons.rs` の `ui_asset!("close"),` を消す
#   2. このスクリプトを回す → `close_icon` の色数が 1 に落ち、`live_dot` は落ちない
#      （図形の丸はアセットを通らないため）
#   3. 消した行を戻す
#
# 実装は visual-test の節（`TAKO_VISUAL_ONLY=glyph-icon`）。Metal の scene を読み戻すので
# **画面収録権限が要らない**。CI には登録しない（実ディスプレイが要る）。
# 窓は常設の仮想ディスプレイ `tako-vd` へ出す（#1141 / #1150）。
# 落とすのは自分で起こしたプロセスだけ。
#
# **PNG をそのまま共有しない**（#927）: 背後のターミナルペインにはシェルプロンプト
# （ユーザー名・ホスト名）が写る。このスクリプトは隔離 HOME と固定プロンプトを渡して
# 防いでいるが、共有前に必ず中身を目で確かめること。
set -uo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO_ROOT" || exit 1
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

TMP=${TAKO_1579_TMP:-$(mktemp -d "${TMPDIR:-/tmp}/tako-1579-XXXXXX")}
OUT=${TAKO_1579_OUT:-$TMP/shots}
mkdir -p "$OUT" "$TMP/home" "$TMP/zdot"
PASS=0; FAIL=0
pass() { PASS=$((PASS+1)); printf '  OK   %s\n' "$1"; }
fail() { FAIL=$((FAIL+1)); printf '  NG   %s\n' "$1"; }

# visual-test の節は feature 付きビルドにしか無い（素のビルドだと「未知の節」で exit 1）。
# **CLI を先に建てる**: `isolated_gui_bins` は 2 本のどちらかが無いと
# `cargo build -p tako-cli -p tako-app`（feature 無し）を走らせるので、順番を逆にすると
# feature 付きで建てた tako-app を素のビルドで上書きされる（新しい worktree で実際に踏んだ）
cargo build -p tako-cli --quiet || exit 1
cargo build -p tako-app --features visual-test --quiet || exit 1

# 起動は必ずヘルパ経由（#1490）。仮想ディスプレイの ensure・置き先・バイナリの決定は
# `scripts/lib/isolated-gui.sh` の 1 実装が持つので、ここで手書きしない
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
trap 'stop_isolated_gui' EXIT

# 証拠 PNG に実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

echo "=== 隔離 GUI で glyph-icon 節を回す ==="
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
launch_isolated_gui "$TMP/run.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DATA_DIR="$TMP/data" TAKO_DISCOVERY_DIR="$TMP/disc" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_TMUX_SOCKET="tako-1579-$$" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=glyph-icon TAKO_VISUAL_DUMP_DIR="$OUT" || exit 1
wait "$ISOLATED_GUI_PID"
RC=$?
ISOLATED_GUI_PID=""

[ "$RC" -eq 0 ] && pass "節が終了コード 0 で終わった" || fail "終了コード ${RC}（ログ: $TMP/run.log）"
grep -q '未知の節' "$TMP/run.log" && fail "visual-test 付きでビルドされていない（節が無い）"
grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/run.log" && pass "TAKO_VISUAL_TEST_OK" || fail "OK 行が出ない"

LINE=$(grep -m1 'TAKO_VISUAL_1579:' "$TMP/run.log")
printf '  観測: %s\n' "${LINE:-（出ていない）}"

# `live_dot=Some((色数, 幅, 高さ))` / `close_icon=Some((色数, 幅, 高さ))`。
# 単色なら「実矩形は登録されたが何も描いていない」
colors_of() { printf '%s' "$LINE" | sed -n "s/.*$1=Some((\([0-9][0-9]*\),.*/\1/p"; }

LIVE=$(colors_of live_dot)
if [ -n "$LIVE" ] && [ "$LIVE" -ge 2 ]; then
  pass "ライブ印の矩形に $LIVE 色ある（図形の丸が実際に塗られている）"
else
  fail "ライブ印の矩形が単色または未登録（live_dot=${LIVE:-なし}）"
fi

CLOSE=$(colors_of close_icon)
if [ -n "$CLOSE" ] && [ "$CLOSE" -ge 2 ]; then
  pass "閉じるの矩形に $CLOSE 色ある（CLOSE の SVG が実際に塗られている）"
else
  fail "閉じるの矩形が単色または未登録（close_icon=${CLOSE:-なし}。EMBEDDED_ASSETS 漏れ?）"
fi

[ -s "$OUT/glyph-icon-pin-titlebar.png" ] && pass "スクショ: $OUT/glyph-icon-pin-titlebar.png" \
  || fail "PNG が落ちていない"

echo
printf '%s PASS / %s FAIL  （出力: %s）\n' "$PASS" "$FAIL" "$OUT"
[ "$FAIL" -eq 0 ]
