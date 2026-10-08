#!/bin/bash
# test-highlight-wait-1890.sh — 大きいファイルの visual-test 節が background の構文の塗りを
# 「状態で」待つこと（#1890）の実経路テスト（隔離 GUI・release・visual-test 入り）
#
# 何が起きていたか:
#   visual-test 節 large-file-decor / large-file-edit（#1660）は、読み取り表示の塗りと
#   編集開始の全文の塗り（どちらも background の syntect）の戻りを「3000 回 / 6000 回 × 10ms」の
#   回数の窓で待っていた。窓の長さは release の塗り（10 MB で 7.8 秒）だけを見て決めていたので、
#   debug（visual-test 入り）では塗り終える前に窓を使い切り、large-file-decor は節が入った
#   deecfc9 の時点から debug の単独実行では一度も通っていなかった。
#
# 何を確かめるか（塗りの遅れは製品側の注入 TAKO_1890_INJECT で作る = CPU を焼かない）:
#   ① 遅れの注入（slow:<DELAY_MS>。旧の 2 つの窓より長い）でも large-file-decor が LF / CRLF とも緑。
#      診断行 TAKO_VISUAL_1890 の waited が遅れ以上 = 本当に遅れの向こうまで待った
#   ② A/B: ① + TAKO_1890_LEGACY=view（旧の 3000 回の窓）→「lf: 読み取り表示が塗られる」で FAILED
#   ③ A/B: ① + TAKO_1890_LEGACY=seed（旧の 6000 回の窓）→「lf: 全文の塗りが戻って揃う」で FAILED
#   ④ 塗りを捨てる注入（drop）→ 上限まで待たずに「lf: 読み取り表示が塗られる」で FAILED
#      （outcome=Settled = 状態で待っても検出力は落ちない）
#   ⑤ 同じ遅れの注入で large-file-edit（tako 自身の main.rs の写し）も緑
#   ⑥ 注入なしの large-file-decor（素の release・偽の言語サーバあり）が緑
#
# 判定は「緑 / 名指しの FAILED」と診断行の outcome だけ。実時間は判定に使わない
# （waited と遅れの大小は「注入が効いた」ことの確認で、待ちの長さの合否ではない）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141 / #1490）。
#
# 使い方: bash scripts/test-highlight-wait-1890.sh
#   DELAY_MS（既定 150000）= 遅れの注入の長さ。旧の窓（release の実測で view 約 37〜46 秒 / seed 約 75〜92 秒）より長くする
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DELAY_MS="${DELAY_MS:-150000}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1890-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "tako-1890-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# #1660 と同じく release（visual-test 入り）を使う（置き場はヘルパの 1 か所が組み立てる）
export TAKO_ISO_PROFILE=release
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
echo "release（visual-test 入り）の tako-app / tako / tako-lsp-fake をビルドします…"
(cd "$REPO_ROOT" && cargo build --release -q -p tako-cli -p tako-app --features tako-app/visual-test \
  && cargo build --release -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins || exit 1
FAKE="$(dirname "$TAKO_BIN")/tako-lsp-fake"

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1890-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# visual-test 節を 1 回走らせて、終わるまで**状態で**待つ（上限 1200 秒 = 遅れの注入 4 回ぶんを含む）。
# 節は `SECTION`（既定 large-file-decor）で選ぶ。緑なら 0
run_section() { # ログ 追加の env…
  local log="$1" section="${SECTION:-large-file-decor}"
  shift
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" "$@" || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 12000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  grep -E "TAKO_VISUAL_1890|TAKO_SELF_TEST_STATE_TIMEOUT|TAKO_APP_SELF_TEST_FAILED" "$log" \
    | cut -c1-240 | sed 's/^/    /'
  grep -q "TAKO_VISUAL_TEST_OK" "$log"
}

# 診断行（`TAKO_VISUAL_1890: label="<label>" stage=<stage> outcome=<X> waited=<秒>s …`）から 1 つ採る
diag_field() { # ログ label stage 欄
  grep "TAKO_VISUAL_1890: label=\"$2\" stage=$3 " "$1" | head -1 | tr ' ' '\n' \
    | sed -n "s/^$4=//p" | head -1
}
DELAY_S=$((DELAY_MS / 1000))
# waited（小数の秒）が遅れ以上か
waited_past_delay() { # ログ label stage
  local waited
  waited="$(diag_field "$1" "$2" "$3" waited)"
  waited="${waited%s}"
  [ -n "$waited" ] && [ "${waited%.*}" -ge "$DELAY_S" ]
}
LSP_ENV="TAKO_LSP_BIN_RUST_ANALYZER=$FAKE"
DIAG_ENV="TAKO_LSP_FAKE_DIAGNOSTICS=$TMP/decor-diagnostics.json"
MAIN_RS="$REPO_ROOT/crates/tako-app/src/main.rs"

echo
echo "① 塗りを ${DELAY_MS} ms 遅らせても large-file-decor が緑（LF / CRLF）"
if run_section "$TMP/slow.log" "$LSP_ENV" "$DIAG_ENV" TAKO_1890_INJECT="slow:$DELAY_MS"; then
  pass "節が緑（遅れの向こうまで待った）"
else
  fail "節が緑にならない（遅れの注入）"
fi
for CASE in lf crlf; do
  for STAGE in view seed; do
    if [ "$(diag_field "$TMP/slow.log" "large-file-decor $CASE" "$STAGE" outcome)" = "Landed" ] \
      && waited_past_delay "$TMP/slow.log" "large-file-decor $CASE" "$STAGE"; then
      pass "$CASE の $STAGE は遅れ以上待って揃った（outcome=Landed waited=$(diag_field "$TMP/slow.log" "large-file-decor $CASE" "$STAGE" waited)）"
    else
      fail "$CASE の $STAGE が遅れの向こうで揃っていない"
    fi
  done
done

echo
echo "② A/B: ① + TAKO_1890_LEGACY=view（旧の 3000 回の窓）"
if run_section "$TMP/legacy-view.log" "$LSP_ENV" "$DIAG_ENV" TAKO_1890_INJECT="slow:$DELAY_MS" TAKO_1890_LEGACY=view; then
  fail "旧の窓でも通った（遅れが窓より短い？ DELAY_MS を伸ばす）"
elif grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test large-file-decor lf: 読み取り表示が塗られる" "$TMP/legacy-view.log"; then
  pass "旧の窓は「lf: 読み取り表示が塗られる」で FAILED（塗りはまだ走っていた = outcome=$(diag_field "$TMP/legacy-view.log" "large-file-decor lf" view outcome)）"
else
  fail "旧の窓の落ち方が違う"
fi

echo
echo "③ A/B: ① + TAKO_1890_LEGACY=seed（旧の 6000 回の窓）"
if run_section "$TMP/legacy-seed.log" "$LSP_ENV" "$DIAG_ENV" TAKO_1890_INJECT="slow:$DELAY_MS" TAKO_1890_LEGACY=seed; then
  fail "旧の窓でも通った（遅れが窓より短い？ DELAY_MS を伸ばす）"
elif grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test large-file-decor lf: 全文の塗りが戻って揃う" "$TMP/legacy-seed.log"; then
  pass "旧の窓は「lf: 全文の塗りが戻って揃う」で FAILED（outcome=$(diag_field "$TMP/legacy-seed.log" "large-file-decor lf" seed outcome)）"
else
  fail "旧の窓の落ち方が違う"
fi

echo
echo "④ 塗りを捨てる注入（drop）は上限まで待たずに落ちる"
if run_section "$TMP/drop.log" "$LSP_ENV" "$DIAG_ENV" TAKO_1890_INJECT=drop; then
  fail "塗りが捨てられたのに通った"
elif grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test large-file-decor lf: 読み取り表示が塗られる" "$TMP/drop.log" \
  && [ "$(diag_field "$TMP/drop.log" "large-file-decor lf" view outcome)" = "Settled" ]; then
  pass "「lf: 読み取り表示が塗られる」で FAILED（outcome=Settled waited=$(diag_field "$TMP/drop.log" "large-file-decor lf" view waited) / 上限 $(diag_field "$TMP/drop.log" "large-file-decor lf" view budget)）"
else
  fail "捨てた塗りの落ち方が違う"
fi

echo
echo "⑤ 同じ遅れの注入で large-file-edit（main.rs の写し）も緑"
if SECTION=large-file-edit run_section "$TMP/edit.log" TAKO_1660_FILE="$MAIN_RS" TAKO_1007_LEGACY=1 TAKO_1890_INJECT="slow:$DELAY_MS"; then
  pass "節が緑"
else
  fail "節が緑にならない（large-file-edit）"
fi
for STAGE in view seed; do
  if [ "$(diag_field "$TMP/edit.log" "large-file-edit" "$STAGE" outcome)" = "Landed" ] \
    && waited_past_delay "$TMP/edit.log" "large-file-edit" "$STAGE"; then
    pass "large-file-edit の $STAGE は遅れ以上待って揃った"
  else
    fail "large-file-edit の $STAGE が遅れの向こうで揃っていない"
  fi
done

echo
echo "⑥ 注入なしの large-file-decor（素の release）"
if run_section "$TMP/plain.log" "$LSP_ENV" "$DIAG_ENV"; then
  pass "節が緑"
else
  fail "節が緑にならない（注入なし）"
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
