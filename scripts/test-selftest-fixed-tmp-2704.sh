#!/usr/bin/env bash
# #2704: セルフテストの節が作る一時 dir（項目 60 の tako-osc-e2e-<pid>・項目 122 の tako-st915-<a|b|x>-<pid> ほか
# remove_on_fail の登録物）が、成功・FAILED・SIGTERM のどの出口でも残らないこと、同じ TMPDIR で並列に走る
# 2 本のセルフテストが同じ名前を取り合わないことを、実 GUI で確かめる。修正前（TAKO_2704_LEGACY=1）は固定名
# （tako-osc-e2e・tako-st915-a/b/x）で作り、どの出口でも残る。
#
#   ok / legacy-ok              : セルフテストの成功（TAKO_APP_SELF_TEST_OK）
#   failed                      : 後ろの項目 154 を既存の注入 TAKO_2371_INJECT=leak で落とす
#   sigterm60 / legacy-sigterm60: 項目 60 の行（TAKO_SELF_TEST_60:）が出た直後に SIGTERM を 1 回送る
#   sigterm915                  : 項目 122 の節の途中（TAKO_SELF_TEST_915_MIGRATE:）で SIGTERM を 1 回送る
#   parallel / legacy-parallel  : 同じ TMPDIR で 2 本を同時に走らせ、両方が項目 60 を越えた時点の置き場の名前を見る
#
# 回ごとに TMPDIR を一時 dir へ向け、終わった後（と 2 秒待った後）にその直下を見る。回す腕は TAKO_2704_RUNS
# （空白区切り。既定は上の全部）。セルフテストは 1 本 5 分前後。窓は仮想ディスプレイ tako-vd の上だけ
# （scripts/lib/isolated-gui.sh）。面が無い機（蓋閉じ等）は終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）
# には触らない。止める・消すのはこのスクリプトが起こした pid と、その pid のソケットだけ。CI には登録しない。
#
#   bash scripts/test-selftest-fixed-tmp-2704.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2704_RUNS:-ok failed sigterm60 sigterm915 parallel legacy-ok legacy-sigterm60 legacy-parallel}"
RUN_LIMIT_SECS="${TAKO_2704_RUN_LIMIT_SECS:-1500}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報と置き場の指定を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET \
  TAKO_DATA_DIR TAKO_DISCOVERY_DIR TAKO_PANE_LOG_DIR TAKO_SESSIONS_FILE TAKO_REMOTE_STATE_DIR \
  TAKO_WORKERS_FILE TAKO_ORCHESTRATOR_DIR TAKO_2704_LEGACY TAKO_2686_LEGACY
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2704-XXXXXX)"
PIDS=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  local pid
  for pid in $PIDS; do kill -0 "$pid" 2>/dev/null && stop_isolated_gui "$pid"; done
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2704_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# セルフテストを 1 本起こす。LAUNCHED_PID を残す（置き先に断られたら 1 度だけやり直し、駄目なら未実測 = 4）
#   launch_one <ログ> <TMPDIR> [VAR=VAL ...]
launch_one() {
  local log="$1" tmpdir="$2" attempt rc
  shift 2
  for attempt in 1 2; do
    launch_isolated_gui "$log" TAKO_SELF_TEST=1 TMPDIR="$tmpdir/" "$@" || exit $?
    LAUNCHED_PID="$ISOLATED_GUI_PID"
    PIDS="$PIDS $LAUNCHED_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$LAUNCHED_PID tako-st-$LAUNCHED_PID"
    # 窓を開けずに即終わる（終了コード 4）ときだけやり直す
    sleep 3
    if ! kill -0 "$LAUNCHED_PID" 2>/dev/null; then
      wait "$LAUNCHED_PID" 2>/dev/null
      rc=$?
      if [ "$rc" -eq "$ISOLATED_GUI_RC_NO_DISPLAY" ]; then
        echo "    置き先に断られて窓を開かずに終わった（終了コード ${rc}）"
        [ "$attempt" -eq 2 ] && { echo "未実測: tako-vd へ窓を開けない"; exit "$ISOLATED_GUI_RC_NO_DISPLAY"; }
        continue
      fi
    fi
    return 0
  done
}

# GUI が自分で終わるまで待つ（上限を過ぎたら止める）。終了コードを RC に置く
wait_exit() {
  local pid="$1" i
  for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "    pid=$pid が時間内に終わらなかったので止める"
    stop_isolated_gui "$pid"
    RC=124
  else
    wait "$pid" 2>/dev/null
    RC=$?
  fi
}

# ログに行が出るか GUI が終わるまで待つ。出たら 0
wait_line() {
  local log="$1" pid="$2" pattern="$3" i
  for i in $(seq 1 $((RUN_LIMIT_SECS * 5))); do
    grep -q "$pattern" "$log" 2>/dev/null && return 0
    kill -0 "$pid" 2>/dev/null || return 1
    sleep 0.2
  done
  return 1
}

# 節の一時 dir（修正後の pid 入り・修正前の固定名）の数
items_left() {
  command ls -1A "$1" 2>/dev/null | grep -cE '^(tako-osc-e2e|tako-st915-[abx])(-[0-9]+)?$'
}
# 自分の pid の名前で残った物（#2686 の置き場・ソケットを含む）の数
own_left() {
  command ls -1A "$1" 2>/dev/null | grep -cE "(^|[^0-9])${2}([^0-9]|$)"
}
listing() { command ls -1A "$1" 2>/dev/null | sed -E 's/[0-9]{3,}/<pid>/g' | sort | tr '\n' ' '; }

# 1 本を回し、出口の後の TMPDIR を見る
#   run_single <ラベル> <出口: exit|sigterm:<行>> [VAR=VAL ...]
run_single() {
  local label="$1" how="$2" dir log pid
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  dir="$TMP/$RUN_SEQ-$label"
  log="$dir/app.log"
  mkdir -p "$dir/tmpdir"
  launch_one "$log" "$dir/tmpdir" "$@"
  pid="$LAUNCHED_PID"
  case "$how" in
    sigterm:*)
      if wait_line "$log" "$pid" "${how#sigterm:}"; then
        SAW_LINE=1
        echo "    [$label] ${how#sigterm:} の時点: $(listing "$dir/tmpdir")"
        kill -TERM "$pid" 2>/dev/null
      else
        SAW_LINE=0
      fi
      ;;
  esac
  wait_exit "$pid"
  RUN_DIR="$dir/tmpdir"
  RUN_LOG="$log"
  RUN_PID="$pid"
  echo "    [$label] pid=$pid rc=$RC"
  grep -E "TAKO_APP_SELF_TEST_(FAILED|OK|EXIT_CLEANUP)" "$log" | cut -c1-200 | sed 's/^/      /'
  echo "      残った物: $(listing "$RUN_DIR")"
}

for run in $RUNS; do
  case "$run" in
    ok | legacy-ok)
      echo "--- 成功の出口（${run}）"
      if [ "$run" = ok ]; then run_single ok exit; else run_single legacy-ok exit TAKO_2704_LEGACY=1; fi
      check "$(grep -q '^TAKO_APP_SELF_TEST_OK' "$RUN_LOG" && [ "$RC" = 0 ] && echo 1)" "${run}: TAKO_APP_SELF_TEST_OK・終了コード 0"
      if [ "$run" = ok ]; then
        check "$([ "$(items_left "$RUN_DIR")" = 0 ] && echo 1)" "成功: 節の一時 dir が残らない"
        check "$([ "$(own_left "$RUN_DIR" "$RUN_PID")" = 0 ] && echo 1)" "成功: 自分の pid の名前の物が残らない"
        sleep 2
        check "$([ "$(items_left "$RUN_DIR")" = 0 ] && echo 1)" "成功: 2 秒後も生き返らない"
      else
        check "$([ "$(items_left "$RUN_DIR")" = 4 ] && echo 1)" "旧の成功: 固定名の 4 つ（tako-osc-e2e・tako-st915-a/b/x）が残る = 修正前の再現"
      fi
      ;;
    failed)
      echo "--- FAILED の出口（項目 154 を TAKO_2371_INJECT=leak で落とす）"
      run_single failed exit TAKO_2371_INJECT=leak
      check "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: 項目 154' "$RUN_LOG" && [ "$RC" = 1 ] && echo 1)" "FAILED: 項目 154 の理由の行・終了コード 1"
      check "$([ "$(items_left "$RUN_DIR")" = 0 ] && echo 1)" "FAILED: 節の一時 dir が残らない"
      check "$([ "$(own_left "$RUN_DIR" "$RUN_PID")" = 0 ] && echo 1)" "FAILED: 自分の pid の名前の物が残らない"
      check "$(grep -qE 'TAKO_APP_SELF_TEST_EXIT_CLEANUP: .* items=[0-9]+ left=none' "$RUN_LOG" && echo 1)" "FAILED: 後片付けの行が items= と left=none を出す"
      ;;
    sigterm60 | legacy-sigterm60 | sigterm915)
      case "$run" in
        sigterm60) echo "--- SIGTERM の出口（項目 60 の後）"; run_single sigterm60 "sigterm:TAKO_SELF_TEST_60:" ;;
        legacy-sigterm60) echo "--- SIGTERM の出口（項目 60 の後・修正前）"; run_single legacy-sigterm60 "sigterm:TAKO_SELF_TEST_60:" TAKO_2704_LEGACY=1 ;;
        sigterm915) echo "--- SIGTERM の出口（項目 122 の節の途中）"; run_single sigterm915 "sigterm:TAKO_SELF_TEST_915_MIGRATE:" ;;
      esac
      check "$([ "$SAW_LINE" = 1 ] && echo 1)" "${run}: 狙った行の後に SIGTERM を送れた"
      if [ "$run" = legacy-sigterm60 ]; then
        check "$([ -d "$RUN_DIR/tako-osc-e2e" ] && echo 1)" "旧の SIGTERM: 固定名の tako-osc-e2e が残る = 修正前の再現"
      else
        check "$([ "$(items_left "$RUN_DIR")" = 0 ] && echo 1)" "${run}: 節の一時 dir が残らない"
        check "$([ "$(own_left "$RUN_DIR" "$RUN_PID")" = 0 ] && echo 1)" "${run}: 自分の pid の名前の物が残らない"
      fi
      ;;
    parallel | legacy-parallel)
      echo "--- 同じ TMPDIR で 2 本を並列に（${run}）"
      RUN_SEQ=$((RUN_SEQ + 1))
      dir="$TMP/$RUN_SEQ-$run"
      mkdir -p "$dir/tmpdir" "$dir/a" "$dir/b"
      extra=""
      [ "$run" = legacy-parallel ] && extra="TAKO_2704_LEGACY=1"
      # shellcheck disable=SC2086
      launch_one "$dir/a/app.log" "$dir/tmpdir" $extra
      pid_a="$LAUNCHED_PID"
      # shellcheck disable=SC2086
      launch_one "$dir/b/app.log" "$dir/tmpdir" $extra
      pid_b="$LAUNCHED_PID"
      saw_a=0
      saw_b=0
      wait_line "$dir/a/app.log" "$pid_a" "TAKO_SELF_TEST_60:" && saw_a=1
      wait_line "$dir/b/app.log" "$pid_b" "TAKO_SELF_TEST_60:" && saw_b=1
      mid="$(command ls -1A "$dir/tmpdir" 2>/dev/null)"
      echo "    両方が項目 60 を越えた時点: $(listing "$dir/tmpdir")"
      check "$([ "$saw_a$saw_b" = 11 ] && echo 1)" "${run}: 2 本とも項目 60 を越えた（pid=${pid_a} / ${pid_b}）"
      if [ "$run" = parallel ]; then
        check "$(grep -qx "tako-osc-e2e-$pid_a" <<<"$mid" && grep -qx "tako-osc-e2e-$pid_b" <<<"$mid" && echo 1)" \
          "並列: 項目 60 の dir が 2 本それぞれの pid の名前で別々"
        check "$(grep -qx 'tako-osc-e2e' <<<"$mid" || echo 1)" "並列: 固定名の tako-osc-e2e を作らない（取り合わない）"
      else
        check "$(grep -qx 'tako-osc-e2e' <<<"$mid" && [ "$(grep -c '^tako-osc-e2e-' <<<"$mid")" = 0 ] && echo 1)" \
          "旧の並列: 2 本が同じ固定名の tako-osc-e2e を共有する = 取り合いの再現"
      fi
      wait_exit "$pid_a"
      rc_a=$RC
      wait_exit "$pid_b"
      rc_b=$RC
      echo "    終了コード: a=$rc_a b=$rc_b"
      grep -hE "TAKO_APP_SELF_TEST_(FAILED|OK)" "$dir/a/app.log" "$dir/b/app.log" | cut -c1-160 | sed 's/^/      /'
      echo "      残った物: $(listing "$dir/tmpdir")"
      if [ "$run" = parallel ]; then
        check "$([ "$(items_left "$dir/tmpdir")" = 0 ] && echo 1)" "並列: 2 本とも終わった後に節の一時 dir が残らない"
        check "$([ "$(own_left "$dir/tmpdir" "$pid_a")" = 0 ] && [ "$(own_left "$dir/tmpdir" "$pid_b")" = 0 ] && echo 1)" \
          "並列: どちらの pid の名前の物も残らない"
      fi
      ;;
    *)
      echo "知らない腕: ${run}"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
