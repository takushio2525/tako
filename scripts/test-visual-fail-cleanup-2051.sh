#!/usr/bin/env bash
# #2051: visual-test の節が `fail()`（即 exit）で落ちた経路でも、節が登録した一時ファイル / dir が
# 消えるかを、実 GUI の節を本当に落として確かめる。落とし方は既存の A/B（同一バイナリの旧挙動）を使う
# = 判定の中身には触らない。
#
#   ① paste-guard（受け手のファイルは data dir の下）を `TAKO_2046_LEGACY=1` で回す = ② で FAILED。
#      新: `<data>/tk2046-<pid>` が残らない・`TAKO_APP_SELF_TEST_CLEANUP` に removed>=1
#      旧（`TAKO_2051_LEGACY=1` = 修正前の fail()）: 同じ dir が残る
#   ② tree-multiselect（fixture は $TMPDIR の下）を `TAKO_1867_LEGACY=1` で回す = ① で FAILED。
#      TMPDIR をこのスクリプトの一時 dir へ向け、新は `tako-vt1867-<pid>` が残らない・旧は残る
#   ③ 正常に通る節（extra-syntax）は従来どおり TAKO_VISUAL_TEST_OK で終わり、後片付けの行を出さず、
#      $TMPDIR に何も残さない（登録が正常経路を変えない）
#
# 窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は
# 終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）には触らない。
#
#   bash scripts/test-visual-fail-cleanup-2051.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# visual-test 版の tako-app が要る。HOME を差し替える前に済ませる（ツールチェーンを取り直さない）
isolated_gui_bins visual-test || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2051-XXXXXX)"
APP_PID=""
cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  rm -rf "$TMP"
}
trap cleanup EXIT

export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERSIST=0
mkdir -p "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/home" "$TMP/zdot"
# ペインのシェルは隔離 HOME + 固定プロンプト（ユーザー名・ホスト名を出さない = #927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 1 回分: data dir と TMPDIR を回ごとに分けて節を 1 本だけ回し、GUI が自分で終わるのを待つ
#   run_one <ラベル> <節> [VAR=VAL ...]  → RUN_DATA / RUN_TMPDIR / RUN_LOG / RUN_PID を残す
# GUI が置き先に断られて窓を開かずに終わった（終了コード 4 = ディスプレイスリープで tako-vd が
# 一覧から落ちた等）回は 1 度だけやり直し、それでも駄目なら未実測（4）で止める。
# 断られた回を数えると、節が何も作っていないのに「残らない」が OK になる
run_one() {
  local label="$1" only="$2" attempt app_rc i
  shift 2
  for attempt in 1 2; do
    RUN_DATA="$TMP/$label-$attempt/data"
    RUN_TMPDIR="$TMP/$label-$attempt/tmpdir"
    RUN_LOG="$TMP/$label-$attempt/app.log"
    mkdir -p "$RUN_DATA" "$RUN_TMPDIR"
    case "$RUN_DATA" in "$TMP"/*) : ;; *) echo "隔離されていない: $RUN_DATA"; exit 1 ;; esac
    # 自動命名は off（隔離でも claude を呼ぶ）
    printf '%s\n' '{"auto_rename": false, "theme": "dark"}' > "$RUN_DATA/settings.json"
    launch_isolated_gui "$RUN_LOG" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$only" \
      TAKO_DATA_DIR="$RUN_DATA" TMPDIR="$RUN_TMPDIR/" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" "$@" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    RUN_PID="$APP_PID"
    for i in $(seq 1 1800); do
      kill -0 "$APP_PID" 2>/dev/null || break
      sleep 0.1
    done
    app_rc=0
    if kill -0 "$APP_PID" 2>/dev/null; then
      stop_isolated_gui "$APP_PID"
    else
      wait "$APP_PID" 2>/dev/null
      app_rc=$?
    fi
    APP_PID=""
    ISOLATED_GUI_PID=""
    [ "$app_rc" -ne "$ISOLATED_GUI_RC_NO_DISPLAY" ] && break
    echo "    [$label] 置き先に断られて窓を開かずに終わった（終了コード ${app_rc}）"
    if [ "$attempt" -eq 2 ]; then
      echo "未実測: tako-vd へ窓を開けない（蓋閉じ・ディスプレイスリープ。scripts/lib/virtual-display.sh status）"
      exit "$ISOLATED_GUI_RC_NO_DISPLAY"
    fi
    # やり直しの launch_isolated_gui が起動の直前に面を起こし直す（ensure は毎回通る）
  done
  echo "    [$label] pid=$RUN_PID"
  grep -E "TAKO_APP_SELF_TEST_FAILED|TAKO_APP_SELF_TEST_CLEANUP|TAKO_VISUAL_TEST_OK" "$RUN_LOG" \
    | cut -c1-200 | sed 's/^/      /'
}

failed_in() { grep -q "TAKO_APP_SELF_TEST_FAILED" "$RUN_LOG" && echo 1; }
removed_at_least_1() {
  grep -Eq "TAKO_APP_SELF_TEST_CLEANUP: registered=[1-9][0-9]* removed=[1-9][0-9]* left=0" "$RUN_LOG" && echo 1
}
no_cleanup_line() { grep -q "TAKO_APP_SELF_TEST_CLEANUP" "$RUN_LOG" || echo 1; }

echo "--- ① paste-guard を ② で落とす（受け手は data dir の下）"
run_one pg-new paste-guard TAKO_2046_LEGACY=1
check "$(failed_in)" "新: 節が FAILED で終わる（TAKO_2046_LEGACY=1 の既存の A/B）"
check "$([ ! -e "$RUN_DATA/tk2046-$RUN_PID" ] && echo 1)" "新: 受け手の dir（data/tk2046-<pid>）が残らない"
check "$(removed_at_least_1)" "新: 後片付けの行が登録と削除を数える（left=0）"

run_one pg-legacy paste-guard TAKO_2046_LEGACY=1 TAKO_2051_LEGACY=1
check "$(failed_in)" "旧: 節が FAILED で終わる"
check "$([ -d "$RUN_DATA/tk2046-$RUN_PID" ] && echo 1)" "旧（TAKO_2051_LEGACY=1）: 受け手の dir が残る = 修正前の再現"
[ -d "$RUN_DATA/tk2046-$RUN_PID" ] && command ls "$RUN_DATA/tk2046-$RUN_PID" | sed 's/^/      残った: /'
check "$(no_cleanup_line)" "旧: 後片付けの行を出さない"

echo "--- ② tree-multiselect を ① で落とす（fixture は \$TMPDIR の下）"
run_one tm-new tree-multiselect TAKO_1867_LEGACY=1
check "$(failed_in)" "新: 節が FAILED で終わる（TAKO_1867_LEGACY=1 の既存の A/B）"
check "$([ ! -e "$RUN_TMPDIR/tako-vt1867-$RUN_PID" ] && echo 1)" "新: fixture（\$TMPDIR/tako-vt1867-<pid>）が残らない"
check "$(removed_at_least_1)" "新: 後片付けの行が登録と削除を数える（left=0）"

run_one tm-legacy tree-multiselect TAKO_1867_LEGACY=1 TAKO_2051_LEGACY=1
check "$(failed_in)" "旧: 節が FAILED で終わる"
check "$([ -d "$RUN_TMPDIR/tako-vt1867-$RUN_PID" ] && echo 1)" "旧（TAKO_2051_LEGACY=1）: fixture が残る = 修正前の再現"

echo "--- ③ 正常に通る節（extra-syntax）は変わらない"
run_one es-new extra-syntax
check "$(grep -q "TAKO_VISUAL_TEST_OK" "$RUN_LOG" && echo 1)" "新: TAKO_VISUAL_TEST_OK で終わる"
check "$(no_cleanup_line)" "新: 落ちていないので後片付けの行を出さない"
check "$([ ! -e "$RUN_TMPDIR/tako-visual-1954-$RUN_PID" ] && echo 1)" "新: 節が自分で消すので fixture が残らない"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
