#!/usr/bin/env bash
# #2757: visual-test 節 editor-font（#1772）のコードの相を、狭い窓（560×600）でも設計どおりに
# 判定できるかと、追従の退行（PR #2750 = #2737 の確かめ直し）の修正の A/B。
#
# 560×600 の窓では 16pt で行 10 以降が器の幅で 2 行に折り返し、折り返しの素材の長い行は器（381px）より
# 背の高い 650px になる。修正前は、
#   - 製品: 背の高い行の次の短い行へ ↓ すると、追従（先頭を下へ）と #2737 の確かめ直し（下げた先で可視が
#     増えて余白のために先頭を上へ）が往復し、キャレットが器の外に残った（行 3 が描かれない = `line=None`）
#   - 検査: Page の着地を 1 フレームで測り（確かめ直しの前）、歩幅を先頭から測った 1 つの値で固定し、
#     余白に FOLLOW_MARGIN を直に期待し、器より背の高い行を「丸ごと入らない = 見えない」と数えていた
# 修正後は追従が `editor_scroll::follow_cursor` の 1 実装でカーソル行を余白より優先し、検査は確かめ直しの
# 後に「押す前の可視 − 1（最低 1 行）」「着地した位置の詰めた余白」「背の高い行はキャレットの行」で判定する。
#
# 腕（TAKO_2757_RUNS。空白区切り。既定 `default wide narrow full legacy`）:
#   default  窓を渡さない（既定の 960×600）: TAKO_VISUAL_TEST_OK・NG 0
#   wide     窓 1400×880: TAKO_VISUAL_TEST_OK・NG 0
#   narrow   窓 560×600: 折り返しの ↓ で行 3 が描かれて見える・コードの相の NG 0。md の相は #2752 の
#            「前提の欠け」で名指して止まる（この幅では md の字送りを測る区間が 1 行に収まらない）
#   full     窓 560×600 + TAKO_2752_LEGACY=1（md の前提で止めず最後の 10 万行まで回す）: NG は md の比だけ
#   legacy   窓 560×600 + TAKO_2757_LEGACY=1（追従が行数だけで先頭を決める = 修正前の製品）: 行 3 が
#            描かれず（`line=None`）「折り返し: ↓ の途中でカーソル行が器の外へ出た (行 [3, 8, …」で NG
# `TAKO_2757_KEEP_TMP=1` はログ（腕ごとの app.log と data dir）を残して置き場を出す。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。腕ごとに「ディスプレイ指定 tako-vd: name で解決」を確かめ、無い回は未実測として
# 数えない（visual-test は TAKO_SELF_TEST を置かないので、面の解決は persist.log と stderr の `persist:` の行に出る）。
# 本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは、このスクリプトが起こした pid と、
# その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-editor-font-2757.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2757_RUNS:-default wide narrow full legacy}"
WIDE_BOUNDS="0,0,1400,880"
NARROW_BOUNDS="0,0,560,600"
# 1 本の上限（節は 10 万行の素材を開くので数分。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2757_RUN_LIMIT_SECS:-900}"

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
TMP="$(mktemp -d /tmp/tk2757-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2757_KEEP_TMP:-}" ]; then
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
  grep -E "TAKO_VISUAL_PIXEL: editor-font (zoom-page|font-max-page|big-page|NG)|wrapped-walk cursor=(2|3|7|8) |TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$RUN_LOG" \
    | cut -c1-400 | sed 's/^/      /'
}

# 描けなかった回は未実測（数えない）。描けた回だけ判定を数える
measured() {
  if [ -n "$RUN_DRAWN" ]; then return 0; fi
  UNMEASURED=$((UNMEASURED + 1))
  echo "  [未実測] ${1}: 「ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決」が無い"
  return 1
}

in_log() { grep -q -- "$1" "$RUN_LOG" && echo 1; }
# 固定文字列で探す（`[` などを含む NG の行）
in_log_f() { grep -qF -- "$1" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }
# 折り返しの ↓ の行 <N> 回目の着地（カーソル行 N）の診断行
walk_line() { grep "^TAKO_VISUAL_PIXEL: editor-font wrapped-walk cursor=$1 " "$RUN_LOG" | tail -1; }
walk_has() { grep -q -- "$2" <<<"$(walk_line "$1")" && echo 1; }
# コードの相の NG（md の相の NG を除く）の数
code_ng() { grep "^TAKO_VISUAL_PIXEL: editor-font NG " "$RUN_LOG" | grep -vc "editor-font NG md: "; }
no_code_ng() { [ "$(code_ng)" = "0" ] && echo 1; }
all_walk_seen() {
  local total seen
  total="$(grep -c "^TAKO_VISUAL_PIXEL: editor-font wrapped-walk cursor=" "$RUN_LOG")"
  seen="$(grep -c "^TAKO_VISUAL_PIXEL: editor-font wrapped-walk cursor=.* seen=true" "$RUN_LOG")"
  [ "$total" -gt 0 ] && [ "$total" = "$seen" ] && echo 1
}
MD_PREMISE="TAKO_APP_SELF_TEST_FAILED: visual-test editor-font (#2752): 前提の欠け: md の字送りを測る区間"
RATIO="md: ⌘+ で見出し / 段落の行の高さ・段落の字送りが文字サイズの比で動かない"
WALK_NG="editor-font NG 折り返し: ↓ の途中でカーソル行が器の外へ出た (行 [3, 8,"

green_arm() { # green_arm <ラベル>
  check "$(in_log "^TAKO_VISUAL_TEST_OK")" "$1: TAKO_VISUAL_TEST_OK"
  check "$(not "$(in_log "TAKO_APP_SELF_TEST_FAILED")")" "$1: FAILED が無い"
  check "$(no_code_ng)" "$1: コードの相の NG が 0"
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
      echo "--- 窓 560×600（md の相は #2752 の前提の欠けで止まる）"
      run_one "$run" "$NARROW_BOUNDS"
      measured "$run" || continue
      check "$(walk_has 2 "tall)=Some(.*, true)) seen=true")" "狭い窓: 器より背の高い行 2 はキャレットの行が見えている"
      check "$(walk_has 3 "line(top,bottom,tall)=Some(")" "狭い窓: 背の高い行の次の行 3 が描かれる（修正前は line=None）"
      check "$(all_walk_seen)" "狭い窓: 折り返しの ↓ でカーソル（の行）が毎回見えている"
      check "$(no_code_ng)" "狭い窓: コードの相の NG が 0（Page の着地・歩幅・余白・font-max・折り返し）"
      check "$(in_log "$MD_PREMISE")" "狭い窓: md の相は「前提の欠け」で名指して止まる（#2752）"
      ;;
    full)
      echo "--- 窓 560×600 + TAKO_2752_LEGACY=1（md の前提で止めず 10 万行まで回す）"
      run_one "$run" "$NARROW_BOUNDS" TAKO_2752_LEGACY=1
      measured "$run" || continue
      check "$(in_log "^TAKO_VISUAL_PIXEL: editor-font big-page-down ")" "最後まで: 10 万行の Page Down まで届く"
      check "$(no_code_ng)" "最後まで: コードの相の NG が 0（10 万行の余白は着地した位置の詰めた余白）"
      check "$(in_log "TAKO_APP_SELF_TEST_FAILED: visual-test editor-font (#1772): ${RATIO}")" \
        "最後まで: 落ちるのは md の比（TAKO_2752_LEGACY の旧の測り方）だけ"
      ;;
    legacy)
      echo "--- 旧（窓 560×600 + TAKO_2757_LEGACY=1 = 追従が行数だけで先頭を決める）"
      run_one "$run" "$NARROW_BOUNDS" TAKO_2757_LEGACY=1
      measured "$run" || continue
      check "$(walk_has 3 "line(top,bottom,tall)=None seen=false")" "旧: 背の高い行の次の行 3 が描かれない（キャレットが器の外）"
      check "$(in_log_f "$WALK_NG")" "旧: 「折り返し: ↓ の途中でカーソル行が器の外へ出た (行 [3, 8, …」で NG"
      ;;
    *)
      echo "知らない腕: ${run}（default / wide / narrow / full / legacy）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]
