#!/usr/bin/env bash
# #2388: 隔離セルフテスト（`TAKO_SELF_TEST=1`）が FAILED で抜けたときも、成功の出口（`on_app_quit`）と
# 同じ後片付けを通るかを、実 GUI のセルフテストを本当に落として確かめる。落とし方は既存の注入
# `TAKO_2371_INJECT=leak`（項目 154 を落とす。前の項目が器を ON のまま戻さないので、落ちる時点で
# 器のサーバー `tako-iso-<pid>` が生きている）を使う = 項目の判定の中身には触らない。
#
#   ① 新 + 注入: FAILED（終了コード 1）で終わり、器のサーバー・ソケットファイル・検証用の設定置き場
#      （`$TMPDIR/tako-agent-config-<pid>`）が残らない。後片付けの行（TAKO_APP_SELF_TEST_EXIT_CLEANUP）が出る
#   ② 旧（`TAKO_2388_LEGACY=1` = 修正前の fail()）+ 注入: 同じ FAILED で、器のサーバーとソケットが残る
#   ③ 新 + 注入 + `TAKO_2388_INJECT=hang`（後片付けが返らない形）: FAILED の理由の行と終了コード 1 は
#      変わらず、上限（5 秒）で打ち切った旨の行を出して終わる
#   ④ 新・注入なし: TAKO_APP_SELF_TEST_OK（終了コード 0）で終わり、従来どおり何も残さない。
#      後片付けの行（失敗の出口のもの）は出さない
#
# 回す腕は `TAKO_2388_RUNS`（空白区切り。既定 `new legacy hang ok`）で選べる。修正前のバイナリを
# `APP_BIN=<path>` で渡して `TAKO_2388_RUNS=legacy` とすれば、修正前の main の再現になる（旧の腕の
# 判定 = 残る、がそのまま当たる）。`TAKO_2388_KEEP_TMP=1` はログ（腕ごとの app.log）を残して置き場を出す
# （注入より前の項目が間欠で落ちた回の診断用）。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは
# このスクリプトが起こした pid と、その pid のソケット `tako-iso-<pid>` だけ。1 本あたり 5 分前後。
#
#   bash scripts/test-selftest-failed-cleanup-2388.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2388_RUNS:-new legacy hang ok}"
# 1 本の上限（セルフテストは 5 分前後。混んだ機で伸びる分を見込む）
RUN_LIMIT_SECS="${TAKO_2388_RUN_LIMIT_SECS:-1500}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
# 起動の cwd はリポジトリの根に揃える（項目 14 の `cargo build -p tako-cli` は #1556 から根を実行ファイルの位置で辿るので、cwd は前提ではない）
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# **`TAKO_TMUX_SOCKET` も落とす**: 明示したソケットは使い捨てではない（#770）ので、
# GUI に `tako-iso-<pid>` を自分で選ばせないとこの検査は何も見ない
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

SOCK_DIR="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2388-XXXXXX)"
APP_PID=""
# このスクリプトが起こした GUI のソケット名（後片付けで自分の分だけ落とす）
OWN_SOCKS=""
# 何本目か（同じ腕を 2 度回しても置き場が混ざらないように、置き場の名前へ入れる）
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # 器はソケットファイルまで片付ける 1 実装で落とす（#2399。`tako` / `default` には触らない）
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2388_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 1 回分: TMPDIR を回ごとに分けてセルフテストを回し、GUI が自分で終わるのを待つ
#   run_one <ラベル> [VAR=VAL ...]  → RUN_PID / RUN_RC / RUN_LOG / RUN_TMPDIR / RUN_SOCK を残す
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" attempt i
  shift
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    RUN_TMPDIR="$TMP/$RUN_SEQ-$label-$attempt/tmpdir"
    RUN_LOG="$TMP/$RUN_SEQ-$label-$attempt/app.log"
    mkdir -p "$RUN_TMPDIR"
    launch_isolated_gui "$RUN_LOG" TAKO_SELF_TEST=1 TMPDIR="$RUN_TMPDIR/" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    RUN_PID="$APP_PID"
    RUN_SOCK="tako-iso-$RUN_PID"
    OWN_SOCKS="$OWN_SOCKS $RUN_SOCK"
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
    APP_PID=""
    ISOLATED_GUI_PID=""
    [ "$RUN_RC" -ne "$ISOLATED_GUI_RC_NO_DISPLAY" ] && break
    echo "    [$label] 置き先に断られて窓を開かずに終わった（終了コード ${RUN_RC}）"
    if [ "$attempt" -eq 2 ]; then
      echo "未実測: tako-vd へ窓を開けない（蓋閉じ・ディスプレイスリープ。scripts/lib/virtual-display.sh status）"
      exit "$ISOLATED_GUI_RC_NO_DISPLAY"
    fi
  done
  echo "    [$label] pid=$RUN_PID rc=$RUN_RC socket=$RUN_SOCK"
  grep -E "TAKO_APP_SELF_TEST_(FAILED|OK|EXIT_CLEANUP)" "$RUN_LOG" | cut -c1-240 | sed 's/^/      /'
  echo "      器（tmux -L ${RUN_SOCK} ls）: $(tmux -L "$RUN_SOCK" ls 2>&1 | sed -n 1p)"
  echo "      ソケットファイル: $(command ls -1 "$SOCK_DIR" 2>/dev/null | grep -cx "${RUN_SOCK}=\{0,1\}") 個"
  echo "      残った一時物（\$TMPDIR の直下）:"
  command ls -1 "$RUN_TMPDIR" | sed 's/^/        /'
}

failed_154() { grep -q "TAKO_APP_SELF_TEST_FAILED: 項目 154" "$RUN_LOG" && echo 1; }
server_left() { tmux -L "$RUN_SOCK" ls >/dev/null 2>&1 && echo 1; }
socket_left() { { [ -e "$SOCK_DIR/$RUN_SOCK" ] || [ -e "$SOCK_DIR/$RUN_SOCK=" ]; } && echo 1; }
agent_home_left() { [ -e "$RUN_TMPDIR/tako-agent-config-$RUN_PID" ] && echo 1; }
exit_cleanup_line() { grep -q "TAKO_APP_SELF_TEST_EXIT_CLEANUP" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    new)
      echo "--- ① 新 + 注入（項目 154 を落とす）"
      run_one new TAKO_2371_INJECT=leak
      check "$(failed_154)" "新: 項目 154 の理由の行で FAILED になる"
      check "$([ "$RUN_RC" -eq 1 ] && echo 1)" "新: 終了コードは 1（従来どおり）"
      check "$(not "$(server_left)")" "新: 器のサーバー（${RUN_SOCK}）が残らない"
      check "$(not "$(socket_left)")" "新: ソケットファイルが残らない"
      check "$(not "$(agent_home_left)")" "新: 検証用の設定置き場（tako-agent-config-<pid>）が残らない"
      check "$(grep -q "TAKO_APP_SELF_TEST_EXIT_CLEANUP: backend=$RUN_SOCK .*left=none" "$RUN_LOG" && echo 1)" \
        "新: 後片付けの行が落としたソケットと left=none を出す"
      ;;
    legacy)
      echo "--- ② 旧（TAKO_2388_LEGACY=1）+ 注入"
      run_one legacy TAKO_2371_INJECT=leak TAKO_2388_LEGACY=1
      check "$(failed_154)" "旧: 項目 154 の理由の行で FAILED になる"
      check "$([ "$RUN_RC" -eq 1 ] && echo 1)" "旧: 終了コードは 1"
      check "$(server_left)" "旧: 器のサーバー（${RUN_SOCK}）が残る = 修正前の再現"
      check "$(socket_left)" "旧: ソケットファイルが残る"
      check "$(agent_home_left)" "旧: 検証用の設定置き場が残る"
      check "$(not "$(exit_cleanup_line)")" "旧: 後片付けの行を出さない"
      # 残った器は次の腕を待たずに落とす（自分の pid のソケットだけ）
      tmux_socket_kill "$RUN_SOCK"
      ;;
    hang)
      echo "--- ③ 新 + 注入 + 後片付けが返らない（TAKO_2388_INJECT=hang）"
      run_one hang TAKO_2371_INJECT=leak TAKO_2388_INJECT=hang
      check "$(failed_154)" "固まる: 項目 154 の理由の行は残る"
      check "$([ "$RUN_RC" -eq 1 ] && echo 1)" "固まる: 終了コードは 1 のまま"
      check "$(grep -q "TAKO_APP_SELF_TEST_EXIT_CLEANUP: .*上限 5s で終わらなかった" "$RUN_LOG" && echo 1)" \
        "固まる: 上限で打ち切った旨の行を出して終わる"
      tmux_socket_kill "$RUN_SOCK"
      ;;
    ok)
      echo "--- ④ 新・注入なし（成功の出口は変わらない）"
      run_one ok
      check "$(grep -q "^TAKO_APP_SELF_TEST_OK" "$RUN_LOG" && echo 1)" "成功: TAKO_APP_SELF_TEST_OK で終わる"
      check "$([ "$RUN_RC" -eq 0 ] && echo 1)" "成功: 終了コードは 0"
      check "$(not "$(server_left)")" "成功: 器のサーバーが残らない"
      check "$(not "$(socket_left)")" "成功: ソケットファイルが残らない"
      check "$(not "$(agent_home_left)")" "成功: 検証用の設定置き場が残らない"
      check "$(not "$(exit_cleanup_line)")" "成功: 失敗の出口の後片付けの行は出さない"
      ;;
    *)
      echo "知らない腕: ${run}（new / legacy / hang / ok）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
