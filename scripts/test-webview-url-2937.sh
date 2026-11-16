#!/bin/bash
# test-webview-url-2937.sh — Web ビュー（wry）が解釈できない・開かせるべきでない URL を、GUI を落とさずに
# 理由つきで断るか（#2937）の実経路テスト
#
# 修正前は `tako web open 'javascript:alert(2936)'` が正規化で `https://javascript:alert(2936)`
# （ポートが数字でない形）になり、wry の `NSURL::URLWithString(…).unwrap()` が GUI ごと abort した。
#
# 何を確かめるか（腕ごとに隔離 GUI を 1 本起こす）:
#   cli-new     今開ける形（data: / about:blank / file: / localhost:<port> / http://127.0.0.1:<port> = ポート
#               提案のチップの形）を `tako web open` で開ける → `tako web open 'javascript:alert(2937)'` と
#               `tako web nav 'javascript:alert(1)'` は理由つきで断り（終了コードは非 0）、GUI は生きている
#               （pid が居て `tako web list` が答える）
#   cli-legacy  同じバイナリで TAKO_2937_LEGACY=1（修正前の正規化 + 形の検査だけ）= 修正前の再現。
#               今開ける形は同じく開けるが、`javascript:` で GUI が wry の panic で abort する（pid が消える）
#   visual      visual-test `webview-url-2937` 節（dispatch・dock の URL 欄・アドレスバーの入口）= TAKO_VISUAL_TEST_OK
# `TAKO_2937_RUNS`（空白区切り。既定 `cli-new cli-legacy visual`）で腕を選ぶ。`TAKO_2937_DUMP_DIR` を渡すと
# visual の窓の絵（PNG）をそこへ残す（既定は一時 dir の中 = 後片付けで消える）。`TAKO_2937_KEEP_TMP=1` は
# ログ（腕ごとの app.log と data dir）を残して置き場を出す。
#
# 使い方: bash scripts/test-webview-url-2937.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / 開く file: のページは mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。開くのは data: / about: / file: と
# 何も待ち受けていない 127.0.0.1 のポートだけ（ネットワークへ出ない）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意できなければ起動せず終了コード 4 で「未実測」を返す。
# **CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_2937_LEGACY TAKO_TMUX_SOCKET

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
RUNS="${TAKO_2937_RUNS:-cli-new cli-legacy visual}"
# tako-vd の右下寄りに置き、ほかの検証の窓と重ねない
BOUNDS="${TAKO_2937_BOUNDS:-1500,800,960,600}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2937-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-2937-$$"
DUMP="${TAKO_2937_DUMP_DIR:-$TMP/dump}"
# 何も待ち受けていないポート（ポート提案のチップの形 `http://localhost:<port>` と同じ。読み込みは失敗するだけ）
PORT=5$(( $$ % 9000 + 1000 ))

cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  if [ -n "${TAKO_2937_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "test-webview-url-2937"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/pages" "$DUMP"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
printf '<!doctype html><title>t2937</title><p>file page</p>\n' > "$TMP/pages/index.html"

common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja
  TAKO_PERSIST=0 TAKO_AUTO_RENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
)

# 1 回分の起動: launch_one <ラベル> [VAR=VAL ...] → RUN_DIR / RUN_LOG / APP_PID
# （面を用意できなければ launch_isolated_gui が終了コード 4 = 未実測で返すので、そのまま止める）
launch_one() {
  local label="$1"
  shift
  RUN_DIR="$TMP/$label"
  RUN_LOG="$RUN_DIR/app.log"
  mkdir -p "$RUN_DIR/data" "$RUN_DIR/tmpdir" "$DUMP/$label"
  ISOLATED_GUI_BOUNDS="$BOUNDS"
  launch_isolated_gui "$RUN_LOG" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$RUN_DIR/data" TMPDIR="$RUN_DIR/tmpdir/" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  ISOLATED_GUI_BOUNDS=""
}

# 起こした GUI が終わるのを待ち、終了コードを RUN_RC に置く
finish_one() {
  wait "$APP_PID"
  RUN_RC=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
}

in_log() { grep -q -- "$1" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }
alive() { kill -0 "$APP_PID" 2>/dev/null && echo 1; }
# 文字列 $1 が $2 を含めば 1（pipefail の下で `printf | grep -q` を使わない = #1633 / #2272）
has() { case "$1" in *"$2"*) echo 1 ;; esac; }

# 今開ける形を順に開く。開けた数を OPENED、最初に開けた Web ビューの ID を FIRST_ID に置く
open_openable() {
  local url out rc
  OPENED=0
  FIRST_ID=""
  for url in "data:text/html,<title>t2937</title>ok" "about:blank" "file://$TMP/pages/index.html" \
    "localhost:$PORT" "http://127.0.0.1:$PORT"; do
    out=$(iso_cli web open "$url" 2>&1)
    rc=$?
    echo "      web open $(printf '%s' "$url" | sed -e "s#${TMP}#<TMP>#g") → rc=$rc $(printf '%s' "$out" | tr '\n' ' ' | sed -e "s#${TMP}#<TMP>#g" | cut -c1-160)"
    [ "$rc" = 0 ] && OPENED=$((OPENED + 1))
    if [ "$rc" = 0 ] && [ -z "$FIRST_ID" ]; then
      FIRST_ID=$(printf '%s' "$out" | sed -n 's/.*"id": *\([0-9][0-9]*\).*/\1/p' | head -1)
    fi
  done
}

# GUI が終わるまで最大 15 秒待つ（終われば 1）
gone_within() {
  local i
  for i in $(seq 1 150); do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
      echo 1
      return 0
    fi
    sleep 0.1
  done
  return 0
}

for run in $RUNS; do
  case "$run" in
    cli-new)
      echo "--- 新（CLI の実経路）"
      launch_one "$run"
      if ! wait_isolated_gui "$RUN_LOG"; then
        bad "新: GUI へ繋がらない"
        stop_isolated_gui "$APP_PID"
        APP_PID=""
        continue
      fi
      open_openable
      check "$([ "$OPENED" = 5 ] && echo 1)" "新: 今開ける形 5 つがすべて開ける（${OPENED}/5）"
      out=$(iso_cli web open 'javascript:alert(2937)' 2>&1)
      rc=$?
      echo "      web open javascript:alert(2937) → rc=$rc $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-240)"
      check "$([ "$rc" != 0 ] && echo 1)" "新: web open javascript: は非 0 で終わる（rc=${rc}）"
      check "$(has "$out" 'javascript: の URL は Web ビューで開かない')" "新: web open javascript: の応答に理由がある"
      check "$(has "$out" 'tako web eval')" "新: 応答に次の一手（tako web eval）がある"
      out=$(iso_cli web nav 'javascript:alert(1)' --id "${FIRST_ID:-1}" 2>&1)
      rc=$?
      echo "      web nav javascript:alert(1) --id ${FIRST_ID:-1} → rc=$rc $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"
      check "$([ "$rc" != 0 ] && has "$out" 'javascript: の URL は Web ビューで開かない')" \
        "新: web nav javascript: も理由つきで断る"
      out=$(iso_cli web open 'https://example.com:abc' 2>&1)
      rc=$?
      echo "      web open https://example.com:abc → rc=$rc $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"
      check "$([ "$rc" != 0 ] && has "$out" 'URL を解釈できない')" \
        "新: NSURL が解釈できない形（ポートが数字でない）を理由つきで断る"
      sleep 1
      check "$(alive)" "新: 断った後も GUI の pid が居る"
      list_rc=1
      iso_cli web list >/dev/null 2>&1 && list_rc=0
      check "$([ "$list_rc" = 0 ] && echo 1)" "新: 断った後も web list が答える"
      check "$(not "$(in_log 'panicked at')")" "新: GUI のログに panic が無い"
      stop_isolated_gui "$APP_PID"
      APP_PID=""
      ;;
    cli-legacy)
      echo "--- 旧（TAKO_2937_LEGACY=1 = 修正前の正規化 + 形の検査だけ）"
      launch_one "$run" TAKO_2937_LEGACY=1
      if ! wait_isolated_gui "$RUN_LOG"; then
        bad "旧: GUI へ繋がらない"
        stop_isolated_gui "$APP_PID"
        APP_PID=""
        continue
      fi
      open_openable
      check "$([ "$OPENED" = 5 ] && echo 1)" "旧: 今開ける形 5 つがすべて開ける（${OPENED}/5。修正前後で同じ）"
      out=$(iso_cli web open 'javascript:alert(2937)' 2>&1)
      rc=$?
      echo "      web open javascript:alert(2937) → rc=$rc $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-240)"
      check "$(gone_within)" "旧: web open javascript: で GUI が終わる（修正前の再現）"
      finish_one
      echo "      GUI の終了コード=$RUN_RC"
      grep -E "panicked at|fatal runtime error" "$RUN_LOG" | sed -e "s#${HOME}#<HOME>#g" | cut -c1-200 | sed 's/^/      /'
      check "$(in_log 'wkwebview/mod.rs')" "旧: 落ちた場所は wry の wkwebview（NSURL の unwrap）"
      ;;
    visual)
      echo "--- visual-test webview-url-2937（dispatch・dock・アドレスバー）"
      launch_one "$run" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=webview-url-2937 \
        TAKO_VISUAL_DUMP_DIR="$DUMP/$run"
      finish_one
      echo "    [$run] rc=$RUN_RC"
      grep -E "TAKO_VISUAL_PIXEL: webview-url-2937|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$RUN_LOG" \
        | sed -e "s#${TMP}#<TMP>#g" | cut -c1-400 | sed 's/^/      /'
      if [ "$RUN_RC" = "$ISOLATED_GUI_RC_NO_DISPLAY" ]; then
        UNMEASURED=$((UNMEASURED + 1))
        echo "  [未実測] visual: 置き先に断られて窓を開かずに終わった"
        continue
      fi
      check "$(in_log '^TAKO_VISUAL_TEST_OK')" "visual: TAKO_VISUAL_TEST_OK"
      check "$(not "$(in_log 'TAKO_APP_SELF_TEST_FAILED')")" "visual: FAILED が無い"
      check "$(in_log 'TAKO_VISUAL_PIXEL: webview-url-2937 OK')" "visual: 節が最後まで走った"
      ;;
    *)
      echo "知らない腕: ${run}（cli-new / cli-legacy / visual）"
      exit 2
      ;;
  esac
done

if [ -n "${TAKO_2937_DUMP_DIR:-}" ]; then
  echo "窓の絵: ${DUMP}"
fi
echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
if [ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]; then
  tako_exit 0
fi
# 描けなかった回が残れば 4（未実測）、判定が落ちれば 1
[ "$FAIL" -eq 0 ] && exit "$ISOLATED_GUI_RC_NO_DISPLAY"
exit 1
