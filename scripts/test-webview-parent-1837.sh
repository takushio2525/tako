#!/bin/bash
# test-webview-parent-1837.sh — Web ビューの親（wry に渡す GPUI の描画ビュー）が閉じた窓を
# 指したまま残り、次の Web ビューの生成で tako 全体が panic で落ちていた不具合の実経路テスト（Issue #1837）
#
# 何を確かめるか:
#   wry の親にするハンドル（`window_raw_handle`）は**初回 render で 1 度だけ**採っていた。
#   その窓が閉じても採り直さないので、閉じた窓の描画ビューを指したまま残る。wry は親ビューの
#   NSWindow を `ns_view.window().unwrap()`（wkwebview/mod.rs:392）で引くので、次の Web ビューの
#   生成で panic してプロセスごと落ちる。表示中の Web ビューの位置合わせ（`set_bounds` の
#   mod.rs:1013）も同じ unwrap を持つ。踏む経路は製品のもの:
#     - `tako window new` で窓を足し、最初の窓を閉じる（下の手順）
#     - macOS で最後の窓を赤ボタンで閉じ、Dock から開き直す（セルフテスト項目 79 → 項目 155）
#      ① 最初の窓に Web ビューを開く → 窓を足す → 最初の窓を閉じる。プロセスは生きている
#      ② もう一度 `tako web open` → 生き残った窓に開ける（応答に pane）。プロセスは生きている
#      ③ 閉じた窓にあった Web ビューも生き残った窓へ付け替わる（`tako web list` の `attached`）。
#         そのペインを表示しても落ちない（`visible` が真・位置合わせが走る）
#      ④ panic.log が無い
#
# A/B（同じバイナリ）: `TAKO_1837_LEGACY=web` を渡すと親を採り直さない（修正前の採り方）。
#   このときは ② が**エラーの応答**（理由つき・非 0）になり、プロセスは落ちない = 生成前の
#   検査（#334 と同じ「wry の unwrap に届く前に断る」）だけが効く腕。
#   `bash scripts/test-webview-parent-1837.sh legacy` で回すとこの腕の期待で判定する。
# 修正前のバイナリ: `TAKO_BIN=… APP_BIN=… bash scripts/test-webview-parent-1837.sh` で渡すと
#   ② でプロセスが落ち、panic.log に `wkwebview/mod.rs:392` が残る（#1837 の症状）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-webview-parent-1837.sh [legacy]
#
# **CI には載せない**（実 GUI + 仮想ディスプレイ + WKWebView が要る）。手元で走らせる実経路テスト。
# 親の採り直しの判定と生成前の検査は単体テスト（`issue1837_*`）が CI で見る。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARM="${1:-new}"
case "$ARM" in
  new | legacy) : ;;
  *) echo "使い方: $0 [legacy]"; exit 2 ;;
esac
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1837-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1837-$$"
cleanup() {
  # 落ちたときは GUI の出力の末尾を残す（一時 dir ごと消える前に）
  if [ "$FAIL" -gt 0 ] && [ -f "${APP_LOG:-}" ]; then
    echo "--- app.log の末尾"
    tail -20 "$APP_LOG"
  fi
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# JSON（標準入力）のトップの項目（無ければ __NONE__・JSON でなければ __NOT_JSON__）
json_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
v = d.get(sys.argv[1], "__NONE__") if isinstance(d, dict) else "__NONE__"
print("null" if v is None else v)' "$1"
}
# `tako window list` の窓 ID を並びの順に（空白区切り）
window_ids() {
  "$TAKO_BIN" window list 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print(" ".join(str(w["id"]) for w in d.get("windows", [])))'
}
# `tako web list` の 1 件の項目（id で引く。無ければ __NONE__）
web_field() {
  "$TAKO_BIN" web list 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for e in d:
    if str(e.get("id")) == sys.argv[1]:
        v = e.get(sys.argv[2], "__NONE__")
        print(v if isinstance(v, str) else json.dumps(v))
        sys.exit(0)
print("__NONE__")' "$1" "$2"
}
alive() { kill -0 "$APP_PID" 2>/dev/null; }
# 条件が真になるまで 0.2 秒刻みで待つ（上限 $2 秒）。真になったら 0
wait_until() {
  local cond="$1" limit="${2:-10}" i
  for i in $(seq 1 $((limit * 5))); do
    if eval "$cond"; then return 0; fi
    sleep 0.2
  done
  return 1
}

APP_LOG="$TMP/app.log"
echo "== 起動（腕: ${ARM}） =="
if [ "$ARM" = legacy ]; then
  launch_isolated_gui "$APP_LOG" TAKO_1837_LEGACY=web || exit $?
else
  launch_isolated_gui "$APP_LOG" || exit $?
fi
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$APP_LOG"; then
  echo "GUI が立たない"; exit 1
fi

echo "== ① 最初の窓に Web ビューを開き、窓を足して最初の窓を閉じる =="
OUT1="$("$TAKO_BIN" web open 'data:text/html,<title>st1837a</title>st1837a' 2>&1)"
WEB1_PANE="$(printf '%s' "$OUT1" | json_field pane)"
WEB1_ID="$(printf '%s' "$OUT1" | json_field id)"
case "$WEB1_PANE" in
  '' | __*) fail "最初の Web ビューを開ける（応答: ${OUT1}）"; echo "前提が立たないので中止"; exit 1 ;;
  *) pass "最初の Web ビューを開ける（id=$WEB1_ID pane=${WEB1_PANE}）" ;;
esac
FIRST_WIN="$(window_ids | awk '{print $1}')"
"$TAKO_BIN" window new >/dev/null 2>&1
if wait_until '[ "$(window_ids | wc -w | tr -d " ")" = 2 ]' 10; then
  pass "窓が 2 枚になる（最初の窓 = ${FIRST_WIN}）"
else
  fail "窓が 2 枚にならない（$(window_ids)）"; exit 1
fi
# 新しい窓が描かれるのを待つ（GPUI の窓は defer で開く）
sleep 2
CLOSE_OUT="$("$TAKO_BIN" window close "$FIRST_WIN" 2>&1)"
if wait_until '[ "$(window_ids | wc -w | tr -d " ")" = 1 ]' 10; then
  pass "最初の窓を閉じられる（${CLOSE_OUT}）"
else
  fail "最初の窓が閉じない（$CLOSE_OUT / $(window_ids)）"
fi
sleep 2
if alive; then pass "窓を閉じた後もプロセスが生きている"; else fail "窓を閉じた時点でプロセスが落ちた"; fi

echo "== ② もう一度 Web ビューを開く =="
OUT2="$("$TAKO_BIN" web open 'data:text/html,<title>st1837b</title>st1837b' 2>&1)"
RC2=$?
sleep 2
WEB2_PANE="$(printf '%s' "$OUT2" | json_field pane)"
if [ "$ARM" = legacy ]; then
  # 親を採り直さない腕: 生成前の検査がエラーで断る（理由つき・非 0）
  if [ "$RC2" -ne 0 ] && [[ "$OUT2" == *"Web ビューの親"* ]]; then
    pass "親が窓から外れていればエラーで断る（rc=$RC2: ${OUT2:0:200}）"
  else
    fail "親が窓から外れているのに断らない（rc=$RC2: ${OUT2:0:300}）"
  fi
  if grep -q "Web ビューの親" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
    pass "persist.log に理由が 1 行残る（$(grep "Web ビューの親" "$TAKO_DATA_DIR/persist.log" | tail -1 | cut -c1-160)）"
  else
    fail "persist.log に理由が残らない"
  fi
else
  case "$WEB2_PANE" in
    '' | __*) fail "生き残った窓に Web ビューを開ける（rc=$RC2 応答: ${OUT2:0:300}）" ;;
    *) pass "生き残った窓に Web ビューを開ける（pane=${WEB2_PANE}）" ;;
  esac
fi
if alive; then
  pass "2 枚目の Web ビューの生成でプロセスが落ちない"
else
  fail "2 枚目の Web ビューの生成でプロセスが落ちた"
fi

echo "== ③ 閉じた窓にあった Web ビューを表示する =="
if alive; then
  ATTACHED1="$(web_field "$WEB1_ID" attached)"
  "$TAKO_BIN" focus "$WEB1_PANE" >/dev/null 2>&1
  # 隔離 GUI の窓は非アクティブなので、表示の切り替えは次の描画（tick）を待つ（#442）
  if [ "$ARM" = legacy ]; then
    sleep 3
  else
    wait_until '[ "$(web_field "$WEB1_ID" visible)" = true ]' 10
  fi
  VISIBLE1="$(web_field "$WEB1_ID" visible)"
  if [ "$ARM" = legacy ]; then
    if [ "$ATTACHED1" = false ]; then
      pass "最初の Web ビューは窓から外れたまま（採り直さない腕は付け替えない）"
    else
      fail "最初の Web ビューの attached=${ATTACHED1}（採り直さない腕は付け替えない）"
    fi
  else
    if [ "$ATTACHED1" = true ]; then pass "最初の Web ビューが生き残った窓へ付け替わる（attached=true）"; else fail "最初の Web ビューが付け替わらない（attached=${ATTACHED1}）"; fi
    if [ "$VISIBLE1" = true ]; then pass "そのペインを表示すると見える（visible=true）"; else fail "そのペインを表示しても見えない（visible=${VISIBLE1}）"; fi
  fi
  if alive; then pass "表示（位置合わせ）でプロセスが落ちない"; else fail "表示（位置合わせ）でプロセスが落ちた"; fi
fi

echo "== ④ panic の痕跡 =="
if [ -f "$TAKO_DATA_DIR/panic.log" ]; then
  fail "panic.log が残った: $(grep -m1 "panic at" "$TAKO_DATA_DIR/panic.log" | cut -c1-200)"
else
  pass "panic.log が無い"
fi

echo
echo "結果: PASS=$PASS FAIL=${FAIL}（腕: ${ARM}）"
[ "$FAIL" -eq 0 ]
