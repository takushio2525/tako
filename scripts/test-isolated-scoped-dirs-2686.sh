#!/usr/bin/env bash
# #2686: 隔離 GUI（TAKO_ISOLATED=1）とセルフテストが env へ自分で当てた置き場（tako-iso-data-<pid>・
# tako-iso-pane-logs-<pid>・セッションカタログ・セルフテストの workers / orchestrator ほか。付属ファイルの
# .bak.N / .lock を含む）と本体の受け口の pid 入りのソケットファイル（tako-<pid>-0.sock）が、成功・FAILED・
# SIGTERM のどの出口でも残らないこと、利用者が明示した置き場（TAKO_DATA_DIR / TAKO_PANE_LOG_DIR）は中身が
# 残ることを、実 GUI で確かめる。回ごとに TMPDIR を一時 dir へ向け、終わった後（と 2 秒待った後 = 片付けの後の
# 書き込みで置き場が生き返らないこと）にその直下を見る。
#
#   sigterm        : 隔離 GUI に SIGTERM を 1 回送り、自分で終わるのを待つ（kill -9 しない）
#   sigterm-own    : 発見の置き場も GUI に決めさせる（helper が渡す置き場を外す = TAKO_2598_LEGACY=1 の起動）
#   explicit       : TAKO_DATA_DIR と TAKO_PANE_LOG_DIR を明示した隔離 GUI に SIGTERM → 中身が残る
#   legacy         : TAKO_2686_LEGACY=1（修正前）+ SIGTERM → 置き場が残る = 同一バイナリでの A/B
#   ok             : セルフテストの成功（TAKO_APP_SELF_TEST_OK）
#   failed         : セルフテストを後ろの項目で落とす（既存の注入 TAKO_2371_INJECT=leak = 項目 154）
#   legacy-ok / legacy-failed : 上の 2 つの A/B
#
# 項目が固定名で作る一時 dir（tako-osc-e2e・tako-st915-*）は #2704 の範囲なので数えない（一覧には出す）。
# 回す腕は TAKO_2686_RUNS（空白区切り。既定 `sigterm sigterm-own explicit legacy ok failed`）で選ぶ。
# セルフテストの腕は 1 本 5 分前後。窓は仮想ディスプレイ tako-vd の上だけ（scripts/lib/isolated-gui.sh）。
# 面が無い機（蓋閉じ等）は終了コード 4 = 未実測。本番の tako（GUI / 設定 / tmux）には触らない。止める・消すのは
# このスクリプトが起こした pid と、その pid のソケット tako-iso-<pid> / tako-st-<pid> だけ。CI には登録しない。
#
#   bash scripts/test-isolated-scoped-dirs-2686.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"

RUNS="${TAKO_2686_RUNS:-sigterm sigterm-own explicit legacy ok failed}"
RUN_LIMIT_SECS="${TAKO_2686_RUN_LIMIT_SECS:-1500}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1
cd "$REPO_ROOT" || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報と置き場の指定を落とす
# （置き場を GUI に自分で決めさせるのがこの検査の対象。明示の腕は自分で並べる）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_TMUX_SOCKET \
  TAKO_DATA_DIR TAKO_DISCOVERY_DIR TAKO_PANE_LOG_DIR TAKO_SESSIONS_FILE TAKO_REMOTE_STATE_DIR \
  TAKO_WORKERS_FILE TAKO_ORCHESTRATOR_DIR TAKO_2686_LEGACY
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2686-XXXXXX)"
APP_PID=""
OWN_SOCKS=""
RUN_SEQ=0

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # shellcheck disable=SC2086
  tmux_socket_kill $OWN_SOCKS
  if [ -n "${TAKO_2686_KEEP_TMP:-}" ]; then
    echo "ログを残した: ${TMP}"
  else
    rm -rf "${TMP:?}"
  fi
}
trap cleanup EXIT

# 1 回分。RUN_PID / RUN_RC / RUN_LOG / RUN_TMPDIR / RUN_DIR を残す。
#   run_one <ラベル> <sigterm|selftest> [VAR=VAL ...]
# 置き先に断られて窓を開かずに終わった（終了コード 4）回は 1 度だけやり直し、駄目なら未実測（4）
run_one() {
  local label="$1" mode="$2" attempt i
  shift 2
  RUN_SEQ=$((RUN_SEQ + 1))
  for attempt in 1 2; do
    RUN_DIR="$TMP/$RUN_SEQ-$label-$attempt"
    RUN_TMPDIR="$RUN_DIR/tmpdir"
    RUN_LOG="$RUN_DIR/app.log"
    mkdir -p "$RUN_TMPDIR"
    if [ "$mode" = sigterm ] && [ -n "${OWN_DISCOVERY:-}" ]; then
      # 発見の置き場を渡さない起動（helper の #2598 の前の形）。**起動の 1 回だけ**に効かせる
      # （待ちの iso_cli まで旧の形にすると、呼び手の env のまま叩く）
      TAKO_2598_LEGACY=1 launch_isolated_gui "$RUN_LOG" TMPDIR="$RUN_TMPDIR/" "$@" || exit $?
    elif [ "$mode" = sigterm ]; then
      launch_isolated_gui "$RUN_LOG" TMPDIR="$RUN_TMPDIR/" "$@" || exit $?
    else
      launch_isolated_gui "$RUN_LOG" TAKO_SELF_TEST=1 TMPDIR="$RUN_TMPDIR/" "$@" || exit $?
    fi
    APP_PID="$ISOLATED_GUI_PID"
    RUN_PID="$APP_PID"
    OWN_SOCKS="$OWN_SOCKS tako-iso-$RUN_PID tako-st-$RUN_PID"
    RUN_RC=""
    if [ "$mode" = sigterm ]; then
      # 発見の置き場を GUI が決めた腕は、その置き場で繋ぐ
      [ -n "$ISOLATED_GUI_DISCOVERY_DIR" ] || ISOLATED_GUI_DISCOVERY_DIR="$RUN_TMPDIR/tako-iso-discovery-$RUN_PID"
      if wait_isolated_gui "$RUN_LOG" 600; then
        # 最初のペインのシェルがペインログを書くまで待ってから止める
        sleep 3
        kill -TERM "$APP_PID" 2>/dev/null
        for i in $(seq 1 200); do
          kill -0 "$APP_PID" 2>/dev/null || break
          sleep 0.05
        done
      fi
    else
      for i in $(seq 1 $((RUN_LIMIT_SECS * 10))); do
        kill -0 "$APP_PID" 2>/dev/null || break
        sleep 0.1
      done
    fi
    if kill -0 "$APP_PID" 2>/dev/null; then
      echo "    [$label] 時間内に終わらなかったので止める"
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
  echo "    [$label] pid=$RUN_PID rc=$RUN_RC"
  grep -E "TAKO_APP_SELF_TEST_(FAILED|OK|EXIT_CLEANUP)" "$RUN_LOG" | cut -c1-240 | sed 's/^/      /'
  echo "      TMPDIR の直下に残った物:"
  command ls -1A "$RUN_TMPDIR" | sed "s/$RUN_PID/<pid>/g" | sed 's/^/        /'
}

# 自分の pid の名前で残った物の数（#2704 の固定名の一時 dir は数えない）
own_left() {
  command ls -1A "$RUN_TMPDIR" 2>/dev/null | grep -cE "^(tako-(iso|st)-[a-z-]+-${RUN_PID}(\.yaml)?(\..*)?|tako-${RUN_PID}-[0-9]+\.sock|tako-agent-config-${RUN_PID})$"
}
# 置き場の名前で残った物（ラベルを並べる）
left_names() {
  command ls -1A "$RUN_TMPDIR" 2>/dev/null | grep -E "${RUN_PID}" | sed "s/$RUN_PID/<pid>/g" | tr '\n' ' '
}
none_left() { [ "$(own_left)" = 0 ] && echo 1; }
# 片付けの後の書き込みで生き返らない（2 秒待ってもう一度見る）
still_none_after_wait() { sleep 2; [ "$(own_left)" = 0 ] && echo 1; }

for run in $RUNS; do
  case "$run" in
    sigterm)
      echo "--- ① SIGTERM（置き場は GUI が決める・helper が発見の置き場を渡す）"
      run_one sigterm sigterm
      check "$([ "$RUN_RC" = 0 ] && echo 1)" "SIGTERM: 自分で終わる（終了コード 0）"
      check "$(none_left)" "SIGTERM: 自分の pid の置き場・ソケットが残らない（残り: $(left_names)）"
      check "$(still_none_after_wait)" "SIGTERM: 2 秒後も生き返らない"
      ;;
    sigterm-own)
      echo "--- ② SIGTERM（発見の置き場も GUI が決める）"
      OWN_DISCOVERY=1
      run_one sigterm-own sigterm
      OWN_DISCOVERY=""
      check "$([ "$RUN_RC" = 0 ] && echo 1)" "SIGTERM（発見も GUI）: 自分で終わる（終了コード 0）"
      check "$(none_left)" "SIGTERM（発見も GUI）: tako-iso-discovery-<pid> を含めて残らない（残り: $(left_names)）"
      ;;
    explicit)
      echo "--- ③ 明示した置き場（TAKO_DATA_DIR / TAKO_PANE_LOG_DIR）は中身が残る"
      EXPLICIT_DATA="$TMP/explicit-data"
      EXPLICIT_LOGS="$TMP/explicit-logs"
      mkdir -p "$EXPLICIT_DATA" "$EXPLICIT_LOGS"
      run_one explicit sigterm TAKO_DATA_DIR="$EXPLICIT_DATA" TAKO_PANE_LOG_DIR="$EXPLICIT_LOGS"
      check "$([ "$RUN_RC" = 0 ] && echo 1)" "明示: 自分で終わる（終了コード 0）"
      check "$([ -s "$EXPLICIT_DATA/persist.log" ] && echo 1)" "明示: TAKO_DATA_DIR の persist.log が残る"
      check "$([ -e "$EXPLICIT_DATA/token" ] && echo 1)" "明示: TAKO_DATA_DIR の token が残る"
      check "$([ -n "$(command ls -A "$EXPLICIT_LOGS" 2>/dev/null)" ] && echo 1)" "明示: TAKO_PANE_LOG_DIR のペインログが残る"
      echo "      明示の data dir: $(command ls -1A "$EXPLICIT_DATA" | tr '\n' ' ')"
      echo "      明示のペインログ: $(command ls -1A "$EXPLICIT_LOGS" | wc -l | tr -d ' ') 個"
      check "$(none_left)" "明示: GUI が決めた残りの置き場（セッションほか）は残らない（残り: $(left_names)）"
      ;;
    legacy)
      echo "--- ④ 修正前（TAKO_2686_LEGACY=1）+ SIGTERM"
      run_one legacy sigterm TAKO_2686_LEGACY=1
      check "$([ -d "$RUN_TMPDIR/tako-iso-data-$RUN_PID" ] && echo 1)" "旧: tako-iso-data-<pid> が残る = 修正前の再現"
      check "$([ -d "$RUN_TMPDIR/tako-iso-pane-logs-$RUN_PID" ] && echo 1)" "旧: tako-iso-pane-logs-<pid> が残る"
      ;;
    ok | legacy-ok)
      echo "--- ⑤ セルフテストの成功（${run}）"
      if [ "$run" = ok ]; then run_one ok selftest; else run_one legacy-ok selftest TAKO_2686_LEGACY=1; fi
      check "$(grep -q "^TAKO_APP_SELF_TEST_OK" "$RUN_LOG" && echo 1)" "${run}: TAKO_APP_SELF_TEST_OK で終わる"
      check "$([ "$RUN_RC" = 0 ] && echo 1)" "${run}: 終了コードは 0"
      if [ "$run" = ok ]; then
        check "$(none_left)" "成功: 自分の pid の置き場・付属ファイル・ソケットが残らない（残り: $(left_names)）"
        check "$(still_none_after_wait)" "成功: 2 秒後も生き返らない"
      else
        check "$([ "$(own_left)" -gt 0 ] && echo 1)" "旧の成功: 置き場が残る = 修正前の再現（残り: $(left_names)）"
      fi
      ;;
    failed | legacy-failed)
      echo "--- ⑥ セルフテストを後ろの項目（154）で落とす（${run}）"
      if [ "$run" = failed ]; then
        run_one failed selftest TAKO_2371_INJECT=leak
      else
        run_one legacy-failed selftest TAKO_2371_INJECT=leak TAKO_2686_LEGACY=1
      fi
      check "$(grep -q "TAKO_APP_SELF_TEST_FAILED: 項目 154" "$RUN_LOG" && echo 1)" "${run}: 項目 154 の理由の行で FAILED になる"
      check "$([ "$RUN_RC" = 1 ] && echo 1)" "${run}: 終了コードは 1（従来どおり）"
      if [ "$run" = failed ]; then
        check "$(none_left)" "FAILED: 自分の pid の置き場・付属ファイル・ソケットが残らない（残り: $(left_names)）"
        check "$(grep -qE "TAKO_APP_SELF_TEST_EXIT_CLEANUP: backend=tako-iso-$RUN_PID .*ipc_socket=1 scoped=[1-9][0-9]* left=none" "$RUN_LOG" && echo 1)" \
          "FAILED: 後片付けの行がソケット 1 本・置き場の数と left=none を出す"
        check "$(still_none_after_wait)" "FAILED: 2 秒後も生き返らない"
      else
        check "$([ "$(own_left)" -gt 0 ] && echo 1)" "旧の FAILED: 置き場が残る = 修正前の再現（残り: $(left_names)）"
      fi
      ;;
    *)
      echo "知らない腕: ${run}（sigterm / sigterm-own / explicit / legacy / ok / failed / legacy-ok / legacy-failed）"
      exit 2
      ;;
  esac
done

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
