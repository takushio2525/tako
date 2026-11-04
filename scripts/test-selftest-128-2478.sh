#!/usr/bin/env bash
# #2478: 隔離セルフテストの項目 128（#945 の脈動）が、混んだ機で不透明度 0.692 のまま落ちる間欠の A/B。
#
# 項目 128 は検証用ペインを作り、側路（feed_osc_bytes）で `133;A` / `133;C` を流して Running にしてから、
# 脈動が全長（6 秒）を過ぎたら不透明度 1.0 で止まるかを見る。修正前は「シェルの起動出力が落ち着くまで」を
# 固定 1500ms で待っていた。シェルの統合は `133;A` を precmd（最初のプロンプトの直前 = rc を読み終えた後）で
# 出すので、rc の読み込みが 1500ms に間に合わない回は、遅れて届いた実シェルの `133;A` が注入した Running を
# Idle へ上書きする。脈動の途中で要素ごと消えるので、不透明度は最後に計算した値で止まる（load 20 で 0.692）。
# 修正後は最初のプロンプト（`shell_marks().prompts >= 1`）を状態で待ってから注入する。
#
# rc の読み込みの遅れは注入で作る（CPU を焼く負荷は作らない）:
#   TAKO_2478_INJECT=slow-shell:<ファイル>  GUI が項目 128 の検証用ペインの ID をシェルを起こす直前にそのファイルへ
#                                書く。ここで置く .zshrc は TAKO_PANE_ID がファイルの中身と同じペインでだけ 3 秒眠る
#                                （ほかのペインは眠らない）。印を env で渡さないのは、器（tmux）越しのペインのシェルへ
#                                届く env が許可リスト（ペイン ID 等）に限られるため。そのペインでの rc の振る舞いは
#                                腕ごとに slow-shell.mode で変える（delay = 3 秒眠る / hang = 待ちの上限より長く眠る /
#                                stray = 眠らず、最初のプロンプトの 4 秒後に統合の _tako_emit で迷子の 133;A を出す）
#
# 腕（TAKO_2478_RUNS。空白区切り。既定 `legacy inject ok full`）:
#   legacy     旧（TAKO_2478_LEGACY=1）+ slow-shell: 項目 128 の「脈動の全長を過ぎたら止まる」で FAILED
#              = 修正前の再現。診断の行の late が early の最後の値（early_last）のまま止まり、state_late=Idle
#   legacy-ok  旧・注入なし: rc がすぐ読み終わる回は旧でも通る（= 落ちるかは rc の読み込みの速さ次第）。
#              機の混み具合で結果が変わる腕なので既定には入れない（見るときは TAKO_2478_RUNS に並べる）
#   inject     新 + slow-shell: 最初のプロンプトを待ってから注入するので項目 128 を通る
#   ok         新・注入なし: 項目 128 を通る（回数は TAKO_2478_OK_RUNS。既定 3）
#   full       新・注入なし: 最後まで回して TAKO_APP_SELF_TEST_OK
#   hang       新 + slow-shell（hang）: 最初のプロンプトが待ちの上限までに届かない = 新の前提の check が
#              「最初のプロンプトを出す」で、待った時間つきで FAILED（エラー経路）
#   stray      新 + slow-shell（stray）: 注入の後に実シェル側から迷子の 133;A が届く = 新の前提の check が
#              「測っている間に実行状態が動かない」で FAILED（② の「止まる」と読み違えない。エラー経路）
#   hang / stray は既定には入れない（見るときは TAKO_2478_RUNS に並べる）
# full 以外は項目 128 の判定が済んだら（項目 129 の行が出たら）その回を止める。
# `TAKO_2478_KEEP_TMP=1` はログ（腕ごとの app.log）を残して置き場を出す。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。腕ごとに「ディスプレイ指定 tako-vd: name で解決」を確かめ、無い回は未実測として
# 数えない。セルフテストの GUI は persist.log へ書かない（`persist_diag` が TAKO_SELF_TEST で戻る = 利用者の
# persist.log を汚さない）ので、同じ文（`placement.log_line()`）を出す app.log の `TAKO_SELF_TEST_DISPLAY:` の
# 行で見る。本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは、このスクリプトが起こした pid と、
# その pid のソケット `tako-iso-<pid>` だけ。
#
#   bash scripts/test-selftest-128-2478.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2478_RUNS:-legacy inject ok full}"
OK_RUNS="${TAKO_2478_OK_RUNS:-3}"
# 1 本の上限（項目 128 までは数分。最後まで回す full は 10 分前後。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2478_RUN_LIMIT_SECS:-2400}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# `TAKO_TMUX_SOCKET` も落とす = GUI に `tako-iso-<pid>` を自分で選ばせる（後片付けで自分の分だけ落とす）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2478-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2478_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# ペインの zsh が読む rc（全腕で同じ。利用者のホームの外 = isolated-gui.sh はこの rc をそのまま使う）。
# 中身は isolated-gui.sh の既定の .zshrc + 項目 128 の検証用ペインにだけ届く遅延
RC_DIR="$TMP/zdotdir"
# 眠らせるペインの ID の置き場（GUI が書く。回ごとに run_one が先に消す = 前の回の ID で眠らない）
SLOW_PANE_FILE="$TMP/slow-shell.pane"
# そのペインでの rc の振る舞い（回ごとに run_one が SLOW_MODE を書く。既定 delay）
SLOW_MODE_FILE="$TMP/slow-shell.mode"
SLOW_MODE=delay
mkdir -p "$RC_DIR"
{
  printf '%s\n' "$ISOLATED_GUI_ZSHRC"
  printf '%s\n' '# #2478 の注入: rc の読み込みが遅れた回（混んだ機）を作る。GUI が書いた ID のペインだけ振る舞いを変える'
  printf 'if [[ -n ${TAKO_PANE_ID-} && -f %s && "$(<%s)" == "$TAKO_PANE_ID" ]]; then\n' \
    "'$SLOW_PANE_FILE'" "'$SLOW_PANE_FILE'"
  printf '  case "$(<%s)" in\n' "'$SLOW_MODE_FILE'"
  printf '%s\n' \
    '    hang) sleep 300 ;;' \
    "    stray) ( sleep 4; _tako_emit \$'\\e]133;A\\a' ) &! ;;" \
    '    *) sleep 3 ;;' \
    '  esac' \
    'fi'
} > "$RC_DIR/.zshrc"

# 項目 129 に入った印（= 項目 128 の判定は済んだ）
ITEM129='TAKO_SELF_TEST_961'

item128_settled() {
  grep -qE "${ITEM129}|TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG"
}

# 項目 129 に入る前（= 項目 128 まで）の行。止めた回は止めたこと自体で #872 の
# 「最終項目より前に quit した」が FAILED として出るので、判定はこの範囲だけで見る
upto_129() { awk -v pat="$ITEM129" '$0 ~ pat {exit} {print}' "$RUN_LOG"; }

# 1 回分: TMPDIR を回ごとに分けてセルフテストを回す
#   run_one <ラベル> <full か stop> [VAR=VAL ...]  → RUN_RC / RUN_LOG / RUN_DRAWN / RUN_DISPLAY を残す
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" mode="$2" attempt i stopped dir
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    dir="$TMP/$RUN_SEQ-$label-$attempt"
    RUN_LOG="$dir/app.log"
    mkdir -p "$dir/tmpdir"
    rm -f "${SLOW_PANE_FILE:?}"
    printf '%s\n' "$SLOW_MODE" > "$SLOW_MODE_FILE"
    launch_isolated_gui "$RUN_LOG" TAKO_SELF_TEST=1 TMPDIR="$dir/tmpdir/" ZDOTDIR="$RC_DIR" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$APP_PID"
    stopped=""
    for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
      kill -0 "$APP_PID" 2>/dev/null || break
      if [ "$mode" = stop ] && item128_settled; then
        stopped=1
        break
      fi
      sleep 0.1
    done
    RUN_RC=0
    if [ -n "$stopped" ]; then
      stop_isolated_gui "$APP_PID"
      RUN_RC=stopped
    elif kill -0 "$APP_PID" 2>/dev/null; then
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
  # 描けたか: 面の解決の行（persist.log と同じ文）が tako-vd の名前で解決している
  RUN_DRAWN=""
  RUN_DISPLAY="$(grep -m1 "^TAKO_SELF_TEST_DISPLAY: " "$RUN_LOG" | cut -c1-160)"
  if grep -q "^TAKO_SELF_TEST_DISPLAY: ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決" "$RUN_LOG"; then
    RUN_DRAWN=1
  fi
  echo "    [$label] rc=$RUN_RC drawn=${RUN_DRAWN:-0} ${RUN_DISPLAY}"
  upto_129 | grep -E "TAKO_SELF_TEST_945|STATE_TIMEOUT.*128|SKIPPED: 128|TAKO_APP_SELF_TEST_FAILED" \
    | cut -c1-600 | sed 's/^/      /'
  if [ "$mode" = full ]; then
    grep -E "^TAKO_APP_SELF_TEST_(FAILED|OK)" "$RUN_LOG" | cut -c1-240 | sed 's/^/      /'
  fi
}

# 描けなかった回は未実測（数えない）。描けた回だけ判定を数える
measured() {
  if [ -n "$RUN_DRAWN" ]; then return 0; fi
  UNMEASURED=$((UNMEASURED + 1))
  echo "  [未実測] ${1}: 「ディスプレイ指定 ${ISOLATED_GUI_DISPLAY}: name で解決」が無い"
  return 1
}

failed_with() { grep -q "TAKO_APP_SELF_TEST_FAILED: $1" "$RUN_LOG" && echo 1; }
failed_any() { grep -q "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" && echo 1; }
line945() { upto_129 | grep "^TAKO_SELF_TEST_945: " | tail -1; }
# 項目 128 を通った = 診断の行と再開の行が出ていて、項目 129 より前に FAILED が無い
passed_128() {
  local head
  head="$(upto_129)"
  grep -q "^TAKO_SELF_TEST_945: " <<<"$head" || return 0
  grep -q "^TAKO_SELF_TEST_945_RESTART: " <<<"$head" || return 0
  grep -q "TAKO_APP_SELF_TEST_FAILED" <<<"$head" && return 0
  echo 1
}
has() { grep -q -- "$1" <<<"$(line945)" && echo 1; }
# 最初のプロンプトまで約 3 秒（2900ms 以上）待った新の腕の診断。波括弧を含む正規表現は `"$( … )"` の中に
# 直に書かない（macOS の bash 3.2 が波括弧展開して語を割る）
SETTLE_3S='settle=(29[0-9]{2}|[3-9][0-9]{3}|[0-9]{5,})ms\(legacy_2478=false\)'
not() { [ -z "$1" ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    legacy)
      echo "--- 旧（TAKO_2478_LEGACY=1）+ slow-shell"
      run_one "$run" stop TAKO_2478_LEGACY=1 TAKO_2478_INJECT="slow-shell:$SLOW_PANE_FILE"
      measured "$run" || continue
      check "$(failed_with "128: 脈動の全長を過ぎたら止まる")" \
        "旧 + slow-shell: 項目 128 の「脈動の全長を過ぎたら止まる」で FAILED = 修正前の再現"
      check "$(has "early_last=\([0-9.]*\) late=\1\.\.\1(spread 0.000)")" \
        "旧 + slow-shell: late の 5 回が early の最後の値のまま（Running を外れて要素が消え、以後は脈動が計算されない）"
      check "$(has "state_late=Some(Idle)")" "旧 + slow-shell: 測った時点でペインは Idle（注入した Running が上書きされた）"
      check "$(has "prompts=0->1->2 ")" \
        "旧 + slow-shell: 注入の時点でシェルの最初のプロンプトはまだ（0）、測るまでに実シェルの 133;A が届いた（1→2）"
      ;;
    legacy-ok)
      echo "--- 旧（TAKO_2478_LEGACY=1）・注入なし"
      run_one "$run" stop TAKO_2478_LEGACY=1
      measured "$run" || continue
      check "$(passed_128)" "旧・注入なし: rc がすぐ読み終わる回は旧でも項目 128 を通る"
      check "$(has "prompts=1->2->2 ")" "旧・注入なし: 注入の前に最初のプロンプトが届いていた（1500ms に間に合った）"
      ;;
    inject)
      echo "--- 新 + slow-shell"
      run_one "$run" stop TAKO_2478_INJECT="slow-shell:$SLOW_PANE_FILE"
      measured "$run" || continue
      check "$(passed_128)" "新 + slow-shell: 最初のプロンプトを待ってから注入するので項目 128 を通る"
      check "$(grep -qE "$SETTLE_3S" <<<"$(line945)" && echo 1)" \
        "新 + slow-shell: 最初のプロンプトまで約 3 秒待った（遅延が効いている。起点のずれを見込んで 2900ms 以上）"
      check "$(has "prompts=1->2->2 state_late=Some(Running)")" "新 + slow-shell: 測っている間に実行状態が動かない"
      ;;
    ok)
      i=1
      while [ "$i" -le "$OK_RUNS" ]; do
        echo "--- 新・注入なし（${i}/${OK_RUNS}）"
        run_one "ok$i" stop
        if measured "ok$i"; then
          check "$(passed_128)" "新・注入なし ${i}/${OK_RUNS}: 項目 128 を通る"
          check "$(has "prompts=1->2->2 state_late=Some(Running)")" "新・注入なし ${i}/${OK_RUNS}: 測っている間に実行状態が動かない"
        fi
        i=$((i + 1))
      done
      ;;
    hang)
      echo "--- 新 + slow-shell（hang = 最初のプロンプトが上限までに届かない）"
      SLOW_MODE=hang
      run_one "$run" stop TAKO_2478_INJECT="slow-shell:$SLOW_PANE_FILE"
      SLOW_MODE=delay
      measured "$run" || continue
      check "$(failed_with "128: 検証用ペインのシェルが最初のプロンプトを出す")" \
        "hang: 新の前提の check が「最初のプロンプトを出す」で FAILED（待った時間つき）"
      check "$(grep -q 'TAKO_SELF_TEST_STATE_TIMEOUT: label="128: 検証用ペインのシェルの最初のプロンプト"' "$RUN_LOG" && echo 1)" \
        "hang: 上限で諦めた診断（STATE_TIMEOUT）が何を待ったかを名指す"
      check "$(not "$(failed_with "128: 脈動の全長を過ぎたら止まる")")" "hang: ② の「止まる」とは読み違えない"
      ;;
    stray)
      echo "--- 新 + slow-shell（stray = 注入の後に実シェル側から迷子の 133;A が届く）"
      SLOW_MODE=stray
      run_one "$run" stop TAKO_2478_INJECT="slow-shell:$SLOW_PANE_FILE"
      SLOW_MODE=delay
      measured "$run" || continue
      check "$(failed_with "128: 測っている間に検証用ペインの実行状態が実シェルの印で動かない")" \
        "stray: 新の前提の check が「測っている間に実行状態が動かない」で FAILED"
      check "$(has "prompts=1->2->3 state_late=Some(Idle)")" "stray: 注入の後に印が 1 つ増え、Idle へ戻されたことが診断に出る"
      check "$(not "$(failed_with "128: 脈動の全長を過ぎたら止まる")")" "stray: ② の「止まる」とは読み違えない"
      ;;
    full)
      echo "--- 新・注入なし（最後まで）"
      run_one full full
      measured "$run" || continue
      check "$(passed_128)" "最後まで: 項目 128 を通る"
      check "$(grep -q "^TAKO_APP_SELF_TEST_OK" "$RUN_LOG" && echo 1)" "最後まで: TAKO_APP_SELF_TEST_OK"
      check "$(not "$(failed_any)")" "最後まで: FAILED が無い"
      ;;
    *)
      echo "知らない腕: ${run}（legacy / legacy-ok / inject / ok / full / hang / stray）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ] && [ "$UNMEASURED" -eq 0 ]
