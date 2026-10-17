#!/bin/bash
# test-user-task-offload-2052.sh — tasks の残りの UI スレッド待ちが無いか（#2052）
#
# #2031 で右パネルの 2 秒ごとの一覧更新は background へ出たが、同じ型が 3 つ残っていた:
#   1. CLI / MCP / PWA（5 秒ごと）の `tako todo list` が UI スレッドの dispatch で置き場を 2 度読む
#   2. 完了・却下（IPC と右パネルのボタン）が UI スレッドで YAML の書き換えを待つ
#   3. コマンドパレットの `panel-tasks`（と `tako panel --view tasks`）は開いた瞬間に一覧を
#      引き直さない（最大 2 秒古い）
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / user-tasks.yaml には触らない）:
#   ① visual-test 節 tasks-offload: パレット / `tako panel` で開いた更新の中でジョブが出て、
#      着地後に閉じている間の起票が載る。完了のボタンの UI スレッドの所要と、書き換えの最中の
#      UI スレッドの往復の間隔が上限未満。エッジ（書き換え中に別の行を開いて打つ / 無い id）
#   ② A/B: `TAKO_2052_LEGACY=1`（修正前）では ① で落ちる（節に検出力がある）
#   ③ 実 CLI: 置き場の読みを注入（`TAKO_2031_INJECT=slow:<ミリ秒>`。書き換えも中で読むので遅れる。
#      CPU は焼かない）したまま、`tako todo list`（1 秒ごと = PWA より密）と `tako todo done` の
#      最中に軽い IPC（`tako scrollback`）の応答時間を測る（IPC の受け口は UI スレッド = UI が
#      止まれば応答も止まる）。perf.log（`TAKO_PERF_VERBOSE=1`）から UI スレッドの置き場の読み
#      （`user_tasks:read`）の回数を新旧の腕で並べる。エッジ: 無い id の完了 / 取得条件の誤り /
#      壊れた置き場
#
# 使い方: bash scripts/test-user-task-offload-2052.sh
#   STAGES="visual cli"（既定は両方）で段を選ぶ。INJECT_MS（既定 1500）で読みの遅さを変える。
#   tako / tako-app は isolated_gui_bins が毎回今の木から visual-test 付きで建てる
#   （最新なら cargo は何もしない = #1643）。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 自動命名は `TAKO_AUTO_RENAME=0` で止める（隔離 GUI でも claude を呼ぶ = #1968 の罠）。
# 落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INJECT_MS="${INJECT_MS:-1500}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-2052-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR"
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
ms_since() { perl -e "printf \"%d\", ($(now) - $1) * 1000"; }

# 腕ごとにデータ・置き場・perf.log を分ける（前の腕の一覧や分布を持ち越さない）
arm_env() {
  local arm="$1"
  mkdir -p "$TMP/data-$arm" "$TMP/orch-$arm"
  for d in "$TMP/data-$arm" "$TMP/orch-$arm"; do
    case "$d" in
      "$TMP"/*) : ;;
      *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
    esac
  done
  echo "TAKO_DATA_DIR=$TMP/data-$arm TAKO_ORCHESTRATOR_DIR=$TMP/orch-$arm \
TAKO_USER_TASKS_FILE=$TMP/orch-$arm/user-tasks.yaml TAKO_PERF_VERBOSE=1 \
TAKO_PERF_LOG=$TMP/perf-$arm.log TAKO_AUTO_RENAME=0 TAKO_2031_INJECT=slow:$INJECT_MS"
}

# perf.log の分布行（`span 統計 [タグ] count=N … max=Mms`）から、タグの count の和と max の最大
span_count() { grep -F "span 統計 [$2] " "$1" 2>/dev/null | sed -E 's/.*count=([0-9]+).*/\1/' | awk '{s+=$1} END{print s+0}'; }

STAGES="${STAGES:-visual cli}"
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local arm="$1" log="$2"
  shift 2
  # shellcheck disable=SC2046
  launch_isolated_gui "$log" $(arm_env "$arm") \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tasks-offload ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  return 0
}

if has_stage visual; then
  echo "① tasks-offload 節（注入: 置き場の読み ${INJECT_MS} ms）"
  run_section vnew "$TMP/vnew.log"
  grep -E "TAKO_VISUAL_2052" "$TMP/vnew.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vnew.log"; then
    pass "全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "節が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$TMP/vnew.log" | tail -5 | sed 's/^/    /'
  fi

  echo "② A/B: TAKO_2052_LEGACY=1（修正前）"
  run_section vlegacy "$TMP/vlegacy.log" TAKO_2052_LEGACY=1
  grep -E "TAKO_VISUAL_2052" "$TMP/vlegacy.log" | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: ① パレットで tasks を開いた瞬間に一覧を引き直す" "$TMP/vlegacy.log"; then
    pass "旧経路では ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/vlegacy.log" | head -1)"
  else
    fail "旧経路でも ① で落ちない（節に検出力が無い）"
    grep -E "FAILED|panicked" "$TMP/vlegacy.log" | tail -3 | sed 's/^/    /'
  fi
fi

# 背景で `cmd` を走らせ、終わるまで（と最低 min_s 秒）`tako scrollback` の応答時間を測る。
# 結果は PROBE_MAX / PROBE_OVER / PROBE_N（500 ms 以上の回数）と、背景の所要 BG_MS に入れる
probe_while() {
  local min_s="$1"
  shift
  local t0 start lat bg_pid t_min
  t0=$(now)
  ( "$@" >"$TMP/bg.out" 2>&1; echo "$?" >"$TMP/bg.rc"; ms_since "$t0" >"$TMP/bg.ms" ) &
  bg_pid=$!
  PROBE_MAX=0 PROBE_OVER=0 PROBE_N=0
  t_min=$(perl -e "printf \"%.3f\", $t0 + $min_s")
  sleep 0.1
  while kill -0 "$bg_pid" 2>/dev/null || perl -e "exit !($(now) < $t_min)"; do
    start=$(now)
    "$TAKO_BIN" scrollback >/dev/null 2>&1
    lat=$(ms_since "$start")
    PROBE_N=$((PROBE_N + 1))
    [ "$lat" -gt "$PROBE_MAX" ] && PROBE_MAX=$lat
    [ "$lat" -ge 500 ] && PROBE_OVER=$((PROBE_OVER + 1))
    sleep 0.1
  done
  wait "$bg_pid" 2>/dev/null
  BG_MS=$(cat "$TMP/bg.ms" 2>/dev/null || echo "?")
  BG_RC=$(cat "$TMP/bg.rc" 2>/dev/null || echo "?")
}

# PWA の 5 秒ごとより密に（1 秒ごとに 6 回）一覧を引く
list_loop() {
  local i
  for i in 1 2 3 4 5 6; do
    "$TAKO_BIN" todo list --json >/dev/null 2>&1 || return 1
    sleep 1
  done
}

measure_cli() {
  local arm="$1"
  shift
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/cli-$arm.log" $(arm_env "$arm") ${1+"$@"} || exit $?
  if ! wait_isolated_gui "$TMP/cli-$arm.log"; then
    fail "[$arm] GUI が立たない"
    stop_isolated_gui "$ISOLATED_GUI_PID"
    return 1
  fi
  # 起票は測る前に済ませる（起票そのものは #2052 の範囲外）。右パネルは開かない
  # （2 秒ループの一覧更新を混ぜない = 測るのは IPC の経路だけ）
  local ids=() i id
  for i in 1 2 3; do
    id=$("$TAKO_BIN" todo add "#2052 の実測 $i" --json 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = decode_json(<STDIN>); print $j->{id} // ""')
    [ -n "$id" ] && ids+=("$id")
  done
  [ "${#ids[@]}" -eq 3 ] || fail "[$arm] tako todo add が通らない（${#ids[@]} 件）"
  local perf="$TMP/perf-$arm.log" mark
  sleep 11
  mark=$(wc -l < "$perf" 2>/dev/null | tr -d ' ')
  mark=${mark:-0}

  # (a) 一覧（1 秒ごとに 6 回）の最中の IPC の応答
  probe_while 6 list_loop
  eval "LIST_MAX_$arm=$PROBE_MAX LIST_OVER_$arm=$PROBE_OVER"
  echo "    [$arm] tako todo list ×6 の最中: tako scrollback ${PROBE_N} 回・最大 ${PROBE_MAX} ms・500 ms 以上 ${PROBE_OVER} 回（一覧 6 回の所要 ${BG_MS} ms・rc=${BG_RC}）"

  # (b) 完了の最中の IPC の応答
  probe_while 1 "$TAKO_BIN" todo done "${ids[0]:-u-0}"
  eval "DONE_MAX_$arm=$PROBE_MAX DONE_OVER_$arm=$PROBE_OVER"
  echo "    [$arm] tako todo done の最中: tako scrollback ${PROBE_N} 回・最大 ${PROBE_MAX} ms・500 ms 以上 ${PROBE_OVER} 回（done の所要 ${BG_MS} ms・rc=${BG_RC}）"
  local status
  status=$("$TAKO_BIN" todo list --all --json 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = decode_json(<STDIN>); my ($t) = grep { $_->{id} eq $ARGV[0] } @{$j->{tasks}}; print $t ? $t->{status} : "missing"' "${ids[0]:-u-0}")
  eval "DONE_STATUS_$arm=$status"

  # (c) エッジ: 無い id の完了 / 取得条件の誤り / 壊れた置き場（新しい腕だけ: 理由が返り UI は止まらない）
  if [ "$arm" = "cnew" ]; then
    local t0 out rc
    t0=$(now)
    out=$("$TAKO_BIN" todo done "u-2052-none" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ] && grep -q "見つからない" <<<"$out"; then
      pass "[$arm] 無い id の完了は理由つきで失敗する（rc=${rc}・$(ms_since "$t0") ms）"
    else
      fail "[$arm] 無い id の完了が失敗にならない（rc=$rc: ${out}）"
    fi
    t0=$(now)
    out=$("$TAKO_BIN" todo list --status bogus 2>&1)
    rc=$?
    local took
    took=$(ms_since "$t0")
    if [ "$rc" -ne 0 ] && [ "$took" -lt "$INJECT_MS" ]; then
      pass "[$arm] 取得条件の誤りは置き場を読まずに即座に断る（rc=${rc}・${took} ms < 注入 ${INJECT_MS} ms）"
    else
      fail "[$arm] 取得条件の誤りの扱いが想定と違う（rc=${rc}・${took} ms: ${out}）"
    fi
    local store="$TMP/orch-$arm/user-tasks.yaml"
    cp "$store" "$TMP/store.bak"
    printf 'tasks: [ {{{ 壊れた\n' > "$store"
    probe_while 1 "$TAKO_BIN" todo list
    if [ "$BG_RC" != "0" ] && grep -q "パースに失敗" "$TMP/bg.out" && [ "$PROBE_MAX" -lt 500 ]; then
      pass "[$arm] 壊れた置き場は 0 件へ丸めず失敗し、その最中も IPC は止まらない（最大 ${PROBE_MAX} ms）"
    else
      fail "[$arm] 壊れた置き場の扱いが想定と違う（rc=${BG_RC}・最大 ${PROBE_MAX} ms: $(head -c 200 "$TMP/bg.out")）"
    fi
    cp "$TMP/store.bak" "$store"
  fi

  sleep 11
  stop_isolated_gui "$ISOLATED_GUI_PID"
  tail -n +"$((mark + 1))" "$perf" > "$TMP/perf-$arm.window.log" 2>/dev/null
  local win="$TMP/perf-$arm.window.log"
  echo "    [$arm] perf.log（測定区間）: 置き場の読み UI スレッド $(span_count "$win" user_tasks:read) 回・background $(span_count "$win" bg:user_tasks:read) 回"
  eval "UI_READS_$arm=$(span_count "$win" user_tasks:read)"
}

if has_stage cli; then
  echo "③ 実 CLI: 一覧と完了の最中の IPC の応答時間（注入: 置き場の読み ${INJECT_MS} ms）"
  measure_cli cnew
  measure_cli clegacy TAKO_2052_LEGACY=1
  if [ "${LIST_MAX_cnew:-99999}" -lt 500 ] && [ "${DONE_MAX_cnew:-99999}" -lt 500 ]; then
    pass "新しい形: 一覧の最中 最大 ${LIST_MAX_cnew} ms・完了の最中 最大 ${DONE_MAX_cnew} ms（UI スレッドが止まらない）"
  else
    fail "新しい形でも IPC が止まる（一覧 ${LIST_MAX_cnew:-?} ms・完了 ${DONE_MAX_cnew:-?} ms）"
  fi
  if [ "${LIST_OVER_clegacy:-0}" -ge 1 ] && [ "${DONE_OVER_clegacy:-0}" -ge 1 ]; then
    pass "旧い形: 一覧の最中 最大 ${LIST_MAX_clegacy} ms（500 ms 以上 ${LIST_OVER_clegacy} 回）・完了の最中 最大 ${DONE_MAX_clegacy} ms（注入が効いている）"
  else
    fail "旧い形でも IPC が止まらない（注入が効いていない。一覧 ${LIST_MAX_clegacy:-?} ms・完了 ${DONE_MAX_clegacy:-?} ms）"
  fi
  if [ "${DONE_STATUS_cnew:-}" = "done" ] && [ "${DONE_STATUS_clegacy:-}" = "done" ]; then
    pass "完了は両方の腕で置き場へ書かれる（status=done）"
  else
    fail "完了が書かれていない（新 ${DONE_STATUS_cnew:-?} / 旧 ${DONE_STATUS_clegacy:-?}）"
  fi
  if [ "${UI_READS_cnew:-1}" -eq 0 ] && [ "${UI_READS_clegacy:-0}" -ge 1 ]; then
    pass "UI スレッドで置き場を読んだ回数（測定区間）: 新 0 回 / 旧 ${UI_READS_clegacy} 回"
  else
    fail "UI スレッドの読みの回数が想定と違う（新 ${UI_READS_cnew:-?} 回 / 旧 ${UI_READS_clegacy:-?} 回）"
  fi
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
