#!/usr/bin/env bash
# #2752: visual-test 節 editor-font（#1772）の md の字送りの検査が、窓の寸法に依存しないかの A/B。
#
# 節は md のプレビューで ⌘+ × 3（13 → 16pt）の前後に、見出し・段落の行の高さと段落の字送りの幅を測る。
# 修正前は字送りを段落の**末尾**まで測っていたので、窓が狭い（既定の 960×600）と拡大した段落が
# 2 行目へ折り返し、末尾の x = 2 行目の x を測って比が崩れた（406.99 → 77.06。行の高さは比どおり）。
# `scripts/test-editor-font-1772.sh` は窓を 1400×880 に固定していたので、その経路では見えなかった。
# 修正後は段落の先頭の決まった区間（1 行目に収まる部分）で測り、その区間が拡大の前後とも 1 行に
# 収まっていることを前提として確かめる。前提が崩れたら比の失敗ではなく「前提の欠け」で止まる。
#
# 腕（TAKO_2752_RUNS。空白区切り。既定 `default wide narrow legacy`）:
#   default  窓を渡さない（既定の 960×600）: TAKO_VISUAL_TEST_OK・md の区間は 1 行に収まる（one_line=true->true）
#   wide     窓 1400×880（test-editor-font-1772.sh の寸法）: TAKO_VISUAL_TEST_OK
#   narrow   窓をさらに狭める（TAKO_2752_NARROW_BOUNDS。既定 0,0,680,600 = 13pt の区間は収まり 16pt で折り返す
#            幅。560 まで狭めるとコードの相も窓の幅で NG を出すので、md の前提だけが崩れる幅にしてある）: 前提の欠けで FAILED
#            （比の失敗「md: ⌘+ で見出し…」ではない）= 前提の確かめの注入
#   legacy   既定の窓 + TAKO_2752_LEGACY=1（末尾で測る・前提を確かめない）: 修正前と同じく
#            「md: ⌘+ で見出し…」で FAILED（測った末尾は拡大で 2 行目 = one_line=true->false）
# `TAKO_2752_KEEP_TMP=1` はログ（腕ごとの app.log と data dir）を残して置き場を出す。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。腕ごとに「ディスプレイ指定 tako-vd: name で解決」を確かめ、無い回は未実測として
# 数えない（visual-test は TAKO_SELF_TEST を置かないので、面の解決は persist.log と stderr の `persist:` の行に出る）。
# 本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは、このスクリプトが起こした pid と、
# その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-editor-font-2752.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2752_RUNS:-default wide narrow legacy}"
WIDE_BOUNDS="0,0,1400,880"
NARROW_BOUNDS="${TAKO_2752_NARROW_BOUNDS:-0,0,680,600}"
# 1 本の上限（節は 10 万行の素材を開くので数分。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2752_RUN_LIMIT_SECS:-900}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins visual-test || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# `TAKO_TMUX_SOCKET` も落とす = GUI に `tako-iso-<pid>` を自分で選ばせる（後片付けで自分の分だけ落とす）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2752-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2752_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 1 回分: data dir と TMPDIR を回ごとに分けて節を回す
#   run_one <ラベル> <窓（x,y,w,h。空 = 渡さない）> [VAR=VAL ...]  → RUN_RC / RUN_LOG / RUN_DRAWN を残す
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" bounds="$2" attempt i dir display
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  ISOLATED_GUI_BOUNDS="$bounds"
  for attempt in 1 2; do
    dir="$TMP/$RUN_SEQ-$label-$attempt"
    RUN_LOG="$dir/app.log"
    RUN_DATA="$dir/data"
    mkdir -p "$dir/tmpdir" "$RUN_DATA"
    launch_isolated_gui "$RUN_LOG" "TAKO_DATA_DIR=$RUN_DATA" TAKO_AUTO_RENAME=0 \
      TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=editor-font TMPDIR="$dir/tmpdir/" ${1+"$@"} || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$APP_PID"
    for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
      kill -0 "$APP_PID" 2>/dev/null || break
      sleep 0.1
    done
    RUN_RC=0
    if kill -0 "$APP_PID" 2>/dev/null; then
      echo "    [$label] ${RUN_LIMIT_SECS} 秒で終わらなかったので止める"
      stop_isolated_gui "$APP_PID"
      RUN_RC=124
    else
      wait "$APP_PID" 2>/dev/null
      RUN_RC=$?
    fi
    tmux_socket_kill "tako-iso-$APP_PID"
    APP_PID=""
    ISOLATED_GUI_PID=""
    [ "$RUN_RC" != "$ISOLATED_GUI_RC_NO_DISPLAY" ] && break
    echo "    [$label] 置き先に断られて窓を開かずに終わった（終了コード ${RUN_RC}）"
    if [ "$attempt" -eq 2 ]; then
      echo "未実測: tako-vd へ窓を開けない（蓋閉じ・ディスプレイスリープ。scripts/lib/virtual-display.sh status）"
      exit "$ISOLATED_GUI_RC_NO_DISPLAY"
    fi
  done
  ISOLATED_GUI_BOUNDS=""
  # 描けたか: 面の解決の行が tako-vd の名前で解決している（stderr の `persist:` か persist.log）
  RUN_DRAWN=""
  display="$(grep -h "ディスプレイ指定 " "$RUN_LOG" "$RUN_DATA/persist.log" 2>/dev/null | sed -n 1p | cut -c1-160)"
  if grep -q "ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決" "$RUN_LOG" "$RUN_DATA/persist.log" 2>/dev/null; then
    RUN_DRAWN=1
  fi
  echo "    [$label] 窓=${bounds:-既定} rc=$RUN_RC drawn=${RUN_DRAWN:-0} ${display}"
  grep -E "TAKO_VISUAL_PIXEL: editor-font (arm|base |wrapped|markdown|NG)|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$RUN_LOG" \
    | cut -c1-400 | sed 's/^/      /'
}

# 描けなかった回は未実測（数えない）。描けた回だけ判定を数える
measured() {
  if [ -n "$RUN_DRAWN" ]; then return 0; fi
  UNMEASURED=$((UNMEASURED + 1))
  echo "  [未実測] ${1}: 「ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決」が無い"
  return 1
}

md_line() { grep "^TAKO_VISUAL_PIXEL: editor-font markdown " "$RUN_LOG" | tail -1; }
has() { grep -q -- "$1" <<<"$(md_line)" && echo 1; }
in_log() { grep -q -- "$1" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }
PREMISE="TAKO_APP_SELF_TEST_FAILED: visual-test editor-font (#2752): 前提の欠け: md の字送りを測る区間"
RATIO="md: ⌘+ で見出し / 段落の行の高さ・段落の字送りが文字サイズの比で動かない"

green_arm() { # green_arm <ラベル>
  check "$(has "legacy_2752=false one_line=true->true")" "$1: md の字送りを測る区間が拡大の前後とも 1 行に収まる"
  check "$(in_log "^TAKO_VISUAL_TEST_OK")" "$1: TAKO_VISUAL_TEST_OK"
  check "$(not "$(in_log "TAKO_APP_SELF_TEST_FAILED")")" "$1: FAILED が無い"
}

for run in $RUNS; do
  case "$run" in
    default)
      echo "--- 既定の窓（960×600。窓を渡さない）"
      run_one "$run" ""
      measured "$run" && green_arm "既定の窓"
      ;;
    wide)
      echo "--- 窓 1400×880"
      run_one "$run" "$WIDE_BOUNDS"
      measured "$run" && green_arm "窓 1400×880"
      ;;
    narrow)
      echo "--- 窓をさらに狭める（${NARROW_BOUNDS} = 前提の確かめの注入）"
      run_one "$run" "$NARROW_BOUNDS"
      measured "$run" || continue
      check "$(has "legacy_2752=false one_line=.*->false")" "狭い窓: 拡大すると測る区間が 1 行に収まらない（注入が効いた）"
      check "$(in_log "$PREMISE")" "狭い窓: 「前提の欠け」で名指して止まる"
      check "$(not "$(in_log "TAKO_APP_SELF_TEST_FAILED: .*${RATIO}")")" "狭い窓: 比の失敗（${RATIO}）として落ちない"
      check "$(not "$(in_log "^TAKO_VISUAL_TEST_OK")")" "狭い窓: TAKO_VISUAL_TEST_OK にならない"
      ;;
    legacy)
      echo "--- 旧（TAKO_2752_LEGACY=1 = 段落の末尾で測る・前提を確かめない。既定の窓）"
      run_one "$run" "" TAKO_2752_LEGACY=1
      measured "$run" || continue
      check "$(has "legacy_2752=true one_line=true->false")" "旧: 測った段落の末尾が拡大で 2 行目へ折り返す"
      check "$(in_log "TAKO_APP_SELF_TEST_FAILED: visual-test editor-font (#1772): ${RATIO}")" \
        "旧: 修正前と同じく「${RATIO}」で FAILED"
      check "$(not "$(in_log "$PREMISE")")" "旧: 前提の確かめは走らない"
      ;;
    *)
      echo "知らない腕: ${run}（default / wide / narrow / legacy）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]
