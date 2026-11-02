#!/usr/bin/env bash
# #2491: visual-test（`--features visual-test`）の出口も、成功の出口（`on_app_quit`）と同じ後片付け
# `cleanup_process_scoped` を通るかを、実 GUI の節を本当に回して確かめる。visual-test は節を回し終えると
# `exit` で抜けるので `on_app_quit` を通らず、修正前は通るたびに検証用の設定置き場
# （`$TMPDIR/tako-agent-config-<pid>`）が、器を有効にした回（`TAKO_PERSIST=1`）は器のサーバー
# `tako-iso-<pid>` とソケットファイルも残っていた。
#
#   ① 新・成功（launch-wait-band + TAKO_PERSIST=1）: TAKO_VISUAL_TEST_OK（終了コード 0）で終わり、
#      走っている間に在った器のサーバー・ソケットファイル・設定置き場が残らない。後片付けの行が出る
#   ② 旧（`TAKO_2491_LEGACY=1` = 修正前の出口）・成功: 同じ OK で、器のサーバー・ソケット・設定置き場が残る
#   ③ 失敗の出口（tree-multiselect + 既存の A/B `TAKO_1867_LEGACY=1` で FAILED・TAKO_PERSIST=1）は
#      従来どおり（#2388）: 終了コード 1 で何も残らない
#   ④ 新・知らない節の名前（exit(1) の出口）: 設定置き場が残らない・後片付けの行が出る
#   ⑤ 旧・知らない節の名前: 設定置き場が残る
#
# 回す腕は `TAKO_2491_RUNS`（空白区切り。既定 `ok legacy fail unknown unknown-legacy`）で選べる。
# `TAKO_2491_KEEP_TMP=1` は置き場を消さずに残す（診断用）。
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは
# このスクリプトが起こした pid と、その pid のソケット `tako-iso-<pid>` だけ。1 本あたり 1 分前後。
#
#   bash scripts/test-visual-exit-cleanup-2491.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2491_RUNS:-ok legacy fail unknown unknown-legacy}"
RUN_LIMIT_SECS="${TAKO_2491_RUN_LIMIT_SECS:-300}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# visual-test 版の tako-app が要る。HOME を差し替える前に済ませる（ツールチェーンを取り直さない）
isolated_gui_bins visual-test || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす。
# **`TAKO_TMUX_SOCKET` も落とす**: 明示したソケットは使い捨てではない（#770）ので、
# GUI に `tako-iso-<pid>` を自分で選ばせないとこの検査は何も見ない
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET

SOCK_DIR="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2491-XXXXXX)"
APP_PID=""
# このスクリプトが起こした GUI のソケット名（後片付けで自分の分だけ落とす）
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # 器はソケットファイルまで片付ける 1 実装で落とす（#2399。`tako` / `default` には触らない）
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2491_KEEP_TMP:-}" ]; then
    echo "置き場を残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
mkdir -p "$TAKO_ORCHESTRATOR_DIR" "$TMP/home" "$TMP/zdot"
# ペインのシェルは隔離 HOME + 固定プロンプト（ユーザー名・ホスト名を出さない = #927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 1 回分: data dir と TMPDIR を回ごとに分けて節を 1 本だけ回し、GUI が自分で終わるのを待つ
#   run_one <ラベル> <節> [VAR=VAL ...]  → RUN_PID / RUN_RC / RUN_LOG / RUN_TMPDIR / RUN_SOCK / RUN_SERVER_SEEN
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" only="$2" attempt i data
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    data="$TMP/$RUN_SEQ-$label-$attempt/data"
    RUN_TMPDIR="$TMP/$RUN_SEQ-$label-$attempt/tmpdir"
    RUN_LOG="$TMP/$RUN_SEQ-$label-$attempt/app.log"
    mkdir -p "$data" "$RUN_TMPDIR"
    # 自動命名は off（隔離でも claude を呼ぶ）
    printf '%s\n' '{"auto_rename": false, "theme": "dark"}' > "$data/settings.json"
    launch_isolated_gui "$RUN_LOG" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$only" \
      TAKO_DATA_DIR="$data" TMPDIR="$RUN_TMPDIR/" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    RUN_PID="$APP_PID"
    RUN_SOCK="tako-iso-$RUN_PID"
    OWN_SOCKS="$OWN_SOCKS $RUN_SOCK"
    # 器のサーバーが走っている間に在ったか（「残らない」が空振りでないことの裏付け）
    RUN_SERVER_SEEN=""
    for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
      kill -0 "$APP_PID" 2>/dev/null || break
      if [ -z "$RUN_SERVER_SEEN" ] && tmux -L "$RUN_SOCK" ls >/dev/null 2>&1; then
        RUN_SERVER_SEEN=1
      fi
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
  echo "    [$label] pid=$RUN_PID rc=$RUN_RC socket=$RUN_SOCK 走っている間の器=${RUN_SERVER_SEEN:-無し}"
  grep -E "TAKO_VISUAL_TEST_OK|TAKO_APP_SELF_TEST_(FAILED|EXIT_CLEANUP)|TAKO_VISUAL_ONLY: 未知" "$RUN_LOG" \
    | cut -c1-200 | sed 's/^/      /'
  echo "      器（tmux -L ${RUN_SOCK} ls）: $(tmux -L "$RUN_SOCK" ls 2>&1 | sed -n 1p | cut -c1-120)"
  echo "      ソケットファイル: $(command ls -1 "$SOCK_DIR" 2>/dev/null | grep -cx "${RUN_SOCK}=\{0,1\}") 個"
  echo "      残った一時物（\$TMPDIR の直下）:"
  command ls -1 "$RUN_TMPDIR" | sed 's/^/        /'
}

visual_ok() { grep -q "^TAKO_VISUAL_TEST_OK" "$RUN_LOG" && echo 1; }
failed() { grep -q "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" && echo 1; }
server_left() { tmux -L "$RUN_SOCK" ls >/dev/null 2>&1 && echo 1; }
socket_left() { { [ -e "$SOCK_DIR/$RUN_SOCK" ] || [ -e "$SOCK_DIR/$RUN_SOCK=" ]; } && echo 1; }
agent_home_left() { [ -e "$RUN_TMPDIR/tako-agent-config-$RUN_PID" ] && echo 1; }
exit_cleanup_line() { grep -q "TAKO_APP_SELF_TEST_EXIT_CLEANUP" "$RUN_LOG" && echo 1; }
not() { [ -z "$1" ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    ok)
      echo "--- ① 新・成功の出口（launch-wait-band + TAKO_PERSIST=1）"
      run_one ok launch-wait-band TAKO_PERSIST=1
      check "$(visual_ok)" "新: TAKO_VISUAL_TEST_OK で終わる"
      check "$([ "$RUN_RC" -eq 0 ] && echo 1)" "新: 終了コードは 0（従来どおり）"
      check "$RUN_SERVER_SEEN" "新: 走っている間は器のサーバー（${RUN_SOCK}）が在った（残らないの裏付け）"
      check "$(not "$(server_left)")" "新: 器のサーバーが残らない"
      check "$(not "$(socket_left)")" "新: ソケットファイルが残らない"
      check "$(not "$(agent_home_left)")" "新: 検証用の設定置き場（tako-agent-config-<pid>）が残らない"
      check "$(grep -q "TAKO_APP_SELF_TEST_EXIT_CLEANUP: backend=$RUN_SOCK .*left=none" "$RUN_LOG" && echo 1)" \
        "新: 後片付けの行が落としたソケットと left=none を出す"
      check "$(awk '/TAKO_APP_SELF_TEST_EXIT_CLEANUP/ { c = NR } /^TAKO_VISUAL_TEST_OK/ { o = NR } END { if (c && o && c < o) print 1 }' "$RUN_LOG")" \
        "新: 成功の行は後片付けの後に出る"
      ;;
    legacy)
      echo "--- ② 旧（TAKO_2491_LEGACY=1）・成功の出口（launch-wait-band + TAKO_PERSIST=1）"
      run_one legacy launch-wait-band TAKO_PERSIST=1 TAKO_2491_LEGACY=1
      check "$(visual_ok)" "旧: TAKO_VISUAL_TEST_OK で終わる"
      check "$([ "$RUN_RC" -eq 0 ] && echo 1)" "旧: 終了コードは 0"
      check "$(server_left)" "旧: 器のサーバー（${RUN_SOCK}）が残る = 修正前の再現"
      check "$(socket_left)" "旧: ソケットファイルが残る"
      check "$(agent_home_left)" "旧: 検証用の設定置き場が残る"
      check "$(not "$(exit_cleanup_line)")" "旧: 後片付けの行を出さない"
      # 残った器は次の腕を待たずに落とす（自分の pid のソケットだけ）
      tmux_socket_kill "$RUN_SOCK"
      ;;
    fail)
      echo "--- ③ 失敗の出口（tree-multiselect + TAKO_1867_LEGACY=1 + TAKO_PERSIST=1）は従来どおり"
      run_one fail tree-multiselect TAKO_PERSIST=1 TAKO_1867_LEGACY=1
      check "$(failed)" "失敗: FAILED で終わる（既存の A/B）"
      check "$([ "$RUN_RC" -eq 1 ] && echo 1)" "失敗: 終了コードは 1"
      check "$(not "$(visual_ok)")" "失敗: 成功の行は出さない"
      check "$(not "$(server_left)")" "失敗: 器のサーバーが残らない"
      check "$(not "$(socket_left)")" "失敗: ソケットファイルが残らない"
      check "$(not "$(agent_home_left)")" "失敗: 設定置き場が残らない"
      check "$(grep -q "TAKO_APP_SELF_TEST_EXIT_CLEANUP: backend=$RUN_SOCK .*left=none" "$RUN_LOG" && echo 1)" \
        "失敗: 後片付けの行は 1 本だけで left=none"
      check "$([ "$(grep -c "TAKO_APP_SELF_TEST_EXIT_CLEANUP" "$RUN_LOG")" -eq 1 ] && echo 1)" \
        "失敗: 後片付けを二重に走らせない"
      ;;
    unknown)
      echo "--- ④ 新・知らない節の名前（exit(1) の出口）"
      run_one unknown no-such-section-2491
      check "$(grep -q "TAKO_VISUAL_ONLY: 未知の節 'no-such-section-2491'" "$RUN_LOG" && echo 1)" "新: 未知の節の案内を出す"
      check "$([ "$RUN_RC" -eq 1 ] && echo 1)" "新: 終了コードは 1（従来どおり）"
      check "$(not "$(visual_ok)")" "新: 成功の行は出さない"
      check "$(not "$(agent_home_left)")" "新: 設定置き場が残らない"
      check "$(exit_cleanup_line)" "新: 後片付けの行が出る"
      ;;
    unknown-legacy)
      echo "--- ⑤ 旧（TAKO_2491_LEGACY=1）・知らない節の名前"
      run_one unknown-legacy no-such-section-2491 TAKO_2491_LEGACY=1
      check "$([ "$RUN_RC" -eq 1 ] && echo 1)" "旧: 終了コードは 1"
      check "$(agent_home_left)" "旧: 設定置き場が残る = 修正前の再現"
      check "$(not "$(exit_cleanup_line)")" "旧: 後片付けの行を出さない"
      ;;
    *)
      echo "知らない腕: ${run}（ok / legacy / fail / unknown / unknown-legacy）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
