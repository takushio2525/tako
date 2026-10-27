#!/bin/bash
# test-webview-build-724.sh — Web ビューの生成を gpui の App の借用の外で 1 本ずつ行う経路（Issue #724）の
# 実経路テスト（macOS の隔離 GUI）
#
# 何を確かめるか:
#   Windows の wry（WebView2）は生成中に入れ子のメッセージループを回すので、App を借用したまま生成すると
#   gpui の前景タスクが二重借用で panic → abort していた（#724 の W1）。修正後の Windows は
#   `create_webview` が生成待ちを積むだけになり、常駐の生成タスクが借用の外で 1 本ずつ作る。
#   この経路（待ち行列・生成中の断り・生成中に閉じる・生成中に落ちた印 = W4）は macOS でも
#   `TAKO_724_ASYNC=1` で同じコードを通せる。生成に時間が掛かる形（Windows の最初の生成 =
#   msedgewebview2 の起動待ちのあいだも前景タスクが走る）は `TAKO_724_INJECT_BUILD_DELAY_MS` で作る。
#
# 腕:
#   sync（既定）  macOS の既定 = 同期の生成。回帰が無いこと（open の応答が ready・タイトル・
#                 data: と about:blank・close・印のファイルを書かない）
#   async         TAKO_724_ASYNC=1 + 注入 3 秒:
#     ① open A → 応答 state=queued
#     ② A の生成中に open B → creating は同時に 1 本だけ・B は queued で待つ
#     ③ 生成中の A に eval → 理由つきで断る（プロセスは落ちない）
#     ④ A・B が順に ready になり、タイトルが出る
#     ⑤ open C → 生成中にそのペインを閉じる → C は作り終えて dock に入る（pane = null）
#     ⑥ open D → 生成中に web close → D は一覧から消え、persist.log に「生成中に閉じられた」
#     ⑦ open E → 生成中（印を書いた後）に kill -9（abort の代わり）→ webview-guard.json に E の印が残る
#     ⑧ 同じ data dir で起動し直す → E は held（自動では開かない・URL は残る）・A / B は ready に戻る
#     ⑨ web nav reload --id E → E が ready になり、最初の読み込みの後に印が消える
#     ⑩ panic.log が無い
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-webview-build-724.sh [sync|async]
#
# **CI には載せない**（実 GUI + 仮想ディスプレイ + WKWebView が要る）。構造（借用の外で生成すること・
# Windows で ipc を付けないこと）は単体テスト `issue724_*` が CI で見る。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARM="${1:-sync}"
case "$ARM" in
  sync | async) : ;;
  *) echo "使い方: $0 [sync|async]"; exit 2 ;;
esac
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-724-XXXXXX)"
APP_PID=""
APP_LOG=""
TMUX_SOCKET="tako-724-$$"
cleanup() {
  if [ "$FAIL" -gt 0 ] && [ -f "${APP_LOG:-}" ]; then
    echo "--- app.log の末尾"
    tail -20 "$APP_LOG"
  fi
  stop_isolated_gui "$APP_PID"
  tmux_socket_kill "$TMUX_SOCKET"
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
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
GUARD="$TAKO_DATA_DIR/webview-guard.json"

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
        print("null" if v is None else (v if isinstance(v, str) else json.dumps(v)))
        sys.exit(0)
print("__NONE__")' "$1" "$2"
}
# `tako web list` で URL に $1 を含む 1 件の項目 $2
web_field_by_url() {
  "$TAKO_BIN" web list 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for e in d:
    if sys.argv[1] in str(e.get("url", "")):
        v = e.get(sys.argv[2], "__NONE__")
        print("null" if v is None else (v if isinstance(v, str) else json.dumps(v)))
        sys.exit(0)
print("__NONE__")' "$1" "$2"
}
# `tako web list` の state ごとの件数（例: "creating=1 queued=1 ready=2"）
web_states() {
  "$TAKO_BIN" web list 2>/dev/null | python3 -c 'import json,sys,collections
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
c = collections.Counter(str(e.get("state")) for e in d)
print(" ".join("%s=%d" % (k, c[k]) for k in sorted(c)))'
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
# Web ビューを開いて id と pane を OPEN_ID / OPEN_PANE / OPEN_STATE に入れる
open_web() {
  local out
  out="$("$TAKO_BIN" web open "$1" 2>&1)"
  OPEN_ID="$(printf '%s' "$out" | json_field id)"
  OPEN_PANE="$(printf '%s' "$out" | json_field pane)"
  OPEN_STATE="$(printf '%s' "$out" | json_field state)"
  OPEN_RAW="$out"
}
launch() {
  APP_LOG="$TMP/app-$1.log"
  shift
  launch_isolated_gui "$APP_LOG" TAKO_PERSIST=1 "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  if ! wait_isolated_gui "$APP_LOG"; then
    echo "GUI が立たない"; exit 1
  fi
}

URL_A='data:text/html,<title>t724a</title>a724'
URL_B='data:text/html,<title>t724b</title>b724'
URL_C='data:text/html,<title>t724c</title>c724'
URL_D='data:text/html,<title>t724d</title>d724'
URL_E='data:text/html,<title>t724e</title>e724'

if [ "$ARM" = sync ]; then
  echo "== 起動（腕: sync = macOS の既定の同期の生成） =="
  launch sync
  open_web "$URL_A"
  case "$OPEN_ID" in
    '' | __*) fail "Web ビューを開ける（応答: ${OPEN_RAW:0:300}）"; exit 1 ;;
  esac
  if [ "$OPEN_STATE" = ready ]; then
    pass "open の応答が state=ready（同期で作り終えて返る。id=$OPEN_ID pane=${OPEN_PANE}）"
  else
    fail "open の応答の state=${OPEN_STATE}（同期の生成なら ready）"
  fi
  ID_A="$OPEN_ID"
  if wait_until '[ "$(web_field "$ID_A" title)" = t724a ]' 15; then
    pass "タイトルが出る（t724a）"
  else
    fail "タイトルが出ない（$(web_field "$ID_A" title)）"
  fi
  open_web 'about:blank'
  ID_BLANK="$OPEN_ID"
  if [ "$OPEN_STATE" = ready ] && alive; then
    pass "about:blank を開ける（id=${ID_BLANK}）"
  else
    fail "about:blank を開けない（${OPEN_RAW:0:300}）"
  fi
  "$TAKO_BIN" web close --id "$ID_BLANK" >/dev/null 2>&1
  if wait_until '[ "$(web_field "$ID_BLANK" state)" = __NONE__ ]' 10; then
    pass "web close で一覧から消える"
  else
    fail "web close しても残る"
  fi
  sleep 3
  if [ -e "$GUARD" ]; then
    fail "同期の生成なのに印のファイルを書いた（$(cat "$GUARD")）"
  else
    pass "同期の生成は印のファイルを書かない"
  fi
else
  DELAY_MS=3000
  echo "== 起動（腕: async = TAKO_724_ASYNC=1・生成の注入 ${DELAY_MS}ms） =="
  launch async1 TAKO_724_ASYNC=1 TAKO_724_INJECT_BUILD_DELAY_MS="$DELAY_MS"

  echo "== ① open A は queued で返る =="
  open_web "$URL_A"
  ID_A="$OPEN_ID"
  case "$ID_A" in
    '' | __*) fail "Web ビューを開ける（応答: ${OPEN_RAW:0:300}）"; exit 1 ;;
  esac
  if [ "$OPEN_STATE" = queued ]; then
    pass "open の応答が state=queued（生成は生成タスクが後で行う。id=$ID_A pane=${OPEN_PANE}）"
  else
    fail "open の応答の state=${OPEN_STATE}（queued のはず）"
  fi

  echo "== ② A の生成中に B を開く =="
  wait_until '[ "$(web_field "$ID_A" state)" = creating ]' 5
  open_web "$URL_B"
  ID_B="$OPEN_ID"
  STATES="$(web_states)"
  if [ "$(web_field "$ID_A" state)" = creating ] && [ "$(web_field "$ID_B" state)" = queued ]; then
    pass "A が creating のあいだ B は queued で待つ（${STATES}）"
  else
    fail "A=$(web_field "$ID_A" state) B=$(web_field "$ID_B" state)（A が creating・B が queued のはず）"
  fi
  case "$STATES" in
    *creating=1*) pass "creating は同時に 1 本だけ（${STATES}）" ;;
    *) fail "creating が 1 本でない（${STATES}）" ;;
  esac

  echo "== ③ 生成中の A に eval =="
  EVAL_OUT="$("$TAKO_BIN" web eval 'document.title' --id "$ID_A" 2>&1)"
  EVAL_RC=$?
  if [ "$EVAL_RC" -ne 0 ] && [[ "$EVAL_OUT" == *"まだ開いていない"* ]]; then
    pass "生成中の eval は理由つきで断る（rc=$EVAL_RC: ${EVAL_OUT:0:160}）"
  else
    fail "生成中の eval が断られない（rc=$EVAL_RC: ${EVAL_OUT:0:300}）"
  fi
  if alive; then pass "生成中の eval でプロセスが落ちない"; else fail "生成中の eval で落ちた"; exit 1; fi

  echo "== ④ A・B が順に ready =="
  if wait_until '[ "$(web_field "$ID_A" state)" = ready ]' 15; then
    B_WHEN_A="$(web_field "$ID_B" state)"
    pass "A が ready になる（そのとき B=${B_WHEN_A}）"
    case "$B_WHEN_A" in
      queued | creating) pass "B は A の後に作られる（${B_WHEN_A}）" ;;
      *) fail "B が A と同時に作られた（${B_WHEN_A}）" ;;
    esac
  else
    fail "A が ready にならない（$(web_field "$ID_A" state)）"
  fi
  if wait_until '[ "$(web_field "$ID_B" state)" = ready ]' 15; then pass "B も ready になる"; else fail "B が ready にならない"; fi
  if wait_until '[ "$(web_field "$ID_A" title)" = t724a ] && [ "$(web_field "$ID_B" title)" = t724b ]' 15; then
    pass "A・B のタイトルが出る"
  else
    fail "タイトルが出ない（A=$(web_field "$ID_A" title) B=$(web_field "$ID_B" title)）"
  fi

  echo "== ⑤ 生成中にペインを閉じる =="
  open_web "$URL_C"
  ID_C="$OPEN_ID"
  PANE_C="$OPEN_PANE"
  wait_until '[ "$(web_field "$ID_C" state)" = creating ]' 5
  "$TAKO_BIN" close --pane "$PANE_C" --force >/dev/null 2>&1
  if wait_until '[ "$(web_field "$ID_C" state)" = ready ]' 15; then
    PANE_AFTER="$(web_field "$ID_C" pane)"
    if [ "$PANE_AFTER" = null ]; then
      pass "生成中にペインを閉じた Web ビューは作り終えて dock に入る（pane=null）"
    else
      fail "生成中にペインを閉じたのに pane=${PANE_AFTER}"
    fi
  else
    fail "生成中にペインを閉じた Web ビューが ready にならない（$(web_field "$ID_C" state)）"
  fi
  if alive; then pass "生成中にペインを閉じても落ちない"; else fail "生成中にペインを閉じて落ちた"; exit 1; fi

  echo "== ⑥ 生成中に web close =="
  open_web "$URL_D"
  ID_D="$OPEN_ID"
  wait_until '[ "$(web_field "$ID_D" state)" = creating ]' 5
  "$TAKO_BIN" web close --id "$ID_D" >/dev/null 2>&1
  sleep "$(( DELAY_MS / 1000 + 2 ))"
  if [ "$(web_field "$ID_D" state)" = __NONE__ ]; then
    pass "生成中に web close した Web ビューは一覧から消えたまま"
  else
    fail "生成中に web close した Web ビューが残る（$(web_field "$ID_D" state)）"
  fi
  if grep -q "生成中に閉じられた" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
    pass "persist.log に 1 行残る（$(grep "生成中に閉じられた" "$TAKO_DATA_DIR/persist.log" | tail -1 | cut -c1-120)）"
  else
    fail "persist.log に「生成中に閉じられた」が無い"
  fi
  if alive; then pass "生成中の web close で落ちない"; else fail "生成中の web close で落ちた"; exit 1; fi
  if [ -e "$GUARD" ] && wait_until '[ ! -e "$GUARD" ]' 20; then
    pass "生成し終えた Web ビューの印は消える（平常時は webview-guard.json が無い）"
  elif [ ! -e "$GUARD" ]; then
    pass "生成し終えた Web ビューの印は消える（平常時は webview-guard.json が無い）"
  else
    fail "印が残り続ける（$(cat "$GUARD")）"
  fi

  echo "== ⑦ 生成中に落ちる（kill -9 = abort の代わり） =="
  open_web "$URL_E"
  ID_E="$OPEN_ID"
  PANE_E="$OPEN_PANE"
  wait_until '[ "$(web_field "$ID_E" state)" = creating ]' 5
  # layout.json に E のペインが載るのを待つ（載らなくても印から dock の保留で戻る = URL は失わない）
  wait_until 'grep -q e724 "$TAKO_DATA_DIR/layout.json" 2>/dev/null' 2
  if grep -q e724 "$GUARD" 2>/dev/null; then
    pass "生成を始める前に印を書いている（$(tr -d '\n ' < "$GUARD" | cut -c1-160)）"
  else
    fail "生成中なのに印が無い（$(cat "$GUARD" 2>/dev/null)）"
  fi
  kill -9 "$APP_PID" 2>/dev/null
  wait "$APP_PID" 2>/dev/null
  APP_PID=""
  if grep -q e724 "$GUARD" 2>/dev/null; then
    pass "落ちた後も印が残る"
  else
    fail "落ちた後に印が残らない"
  fi

  echo "== ⑧ 起動し直す =="
  launch async2 TAKO_724_ASYNC=1
  if wait_until '[ "$(web_field_by_url e724 state)" = held ]' 15; then
    pass "生成中に落ちた Web ビューは自動では開かず held（pane=$(web_field_by_url e724 pane)・落ちる前の pane=${PANE_E}）"
  else
    fail "生成中に落ちた Web ビューの state=$(web_field_by_url e724 state)（held のはず）"
  fi
  if wait_until '[ "$(web_field_by_url a724 state)" = ready ] && [ "$(web_field_by_url b724 state)" = ready ]' 20; then
    pass "ほかの Web ビュー（A / B）は従来どおり開き直す"
  else
    fail "A / B が開き直らない（A=$(web_field_by_url a724 state) B=$(web_field_by_url b724 state)）"
  fi
  if grep -q "前回の生成中に tako が終了したので自動では開かない" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
    pass "persist.log に保留の 1 行が残る"
  else
    fail "persist.log に保留の 1 行が無い"
  fi

  echo "== ⑨ 保留を開く =="
  ID_E2="$(web_field_by_url e724 id)"
  NAV_OUT="$("$TAKO_BIN" web nav reload --id "$ID_E2" 2>&1)"
  if wait_until '[ "$(web_field "$ID_E2" state)" = ready ]' 15; then
    pass "web nav reload で保留の Web ビューが開く（${NAV_OUT:0:120}）"
  else
    fail "保留の Web ビューが開かない（state=$(web_field "$ID_E2" state) 応答: ${NAV_OUT:0:200}）"
  fi
  if wait_until '[ ! -e "$GUARD" ]' 25; then
    pass "開いて最初の読み込みが済むと印が消える"
  else
    fail "開いた後も印が残る（$(cat "$GUARD" 2>/dev/null)）"
  fi
fi

echo "== ⑩ panic の痕跡 =="
if alive; then pass "プロセスが生きている"; else fail "プロセスが落ちている"; fi
if [ -f "$TAKO_DATA_DIR/panic.log" ]; then
  fail "panic.log が残った: $(grep -m1 "panic at" "$TAKO_DATA_DIR/panic.log" | cut -c1-200)"
else
  pass "panic.log が無い"
fi

echo
echo "結果: PASS=$PASS FAIL=${FAIL}（腕: ${ARM}）"
[ "$FAIL" -eq 0 ]
