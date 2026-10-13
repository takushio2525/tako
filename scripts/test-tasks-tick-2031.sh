#!/bin/bash
# test-tasks-tick-2031.sh — 右パネル tasks の 2 秒ごとの一覧更新が UI スレッドで置き場を読まない（#2031）
#
# 2026-10-13 の本番 perf.log に、2 秒ループの一覧更新（`tick_user_tasks`）が UI スレッドで
# user-tasks.yaml を読んで 1 回 551 ms 止まった記録がある。打鍵中に当たると返答欄ごと固まる
# （#1976 / #1984 / #1991 と同じ「UI スレッドでの I/O」の型）。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / user-tasks.yaml には触らない）:
#   ① visual-test 節 tasks-tick: 置き場の読みを注入（`TAKO_2031_INJECT=slow:<ミリ秒>`。CPU は焼かない）
#      で遅らせたまま、返答欄へ 40 ms ごとに打ち続ける。打鍵の間隔の最大値が上限未満・打った文字が
#      全部入る・版が変わらない間は読まない。エッジ（別経路が書いた直後 / 読みの最中にパネルを閉じる /
#      壊れた置き場 / 置き場が消える）も同じ節で見る
#   ② A/B: `TAKO_2031_LEGACY=1`（修正前 = tick が UI スレッドの同期の dispatch で毎回読む）では
#      ① の打鍵の間隔で落ちる（= 注入が効いていて、節に検出力がある）
#   ③ 実 CLI: 同じ注入で tasks ビューを開いたまま、軽い IPC（`tako scrollback`）の応答時間を
#      0.2 秒ごとに測る（IPC の受け口は UI スレッド = UI が止まれば応答も止まる）。perf.log
#      （`TAKO_PERF_VERBOSE=1`）の分布から、UI スレッドの一覧更新（`periodic_prep:user_tasks`）の
#      最大と、置き場の読みの回数（UI スレッド = `user_tasks:read` / background = `bg:user_tasks:read`）
#      を新旧の腕で並べる
#
# 使い方: bash scripts/test-tasks-tick-2031.sh
#   STAGES="visual cli"（既定は両方）で段を選ぶ。INJECT_MS（既定 1500）で読みの遅さを変える。
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

TMP="$(mktemp -d /tmp/tako-2031-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app をビルドします…"
(cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR"
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }

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

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local arm="$1" log="$2"
  shift 2
  # shellcheck disable=SC2046
  launch_isolated_gui "$log" $(arm_env "$arm") \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tasks-tick ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  return 0
}

# perf.log の分布行（`span 統計 [タグ] count=N … max=Mms`）から、タグの count の和と max の最大
span_count() { grep -F "span 統計 [$2] " "$1" 2>/dev/null | sed -E 's/.*count=([0-9]+).*/\1/' | awk '{s+=$1} END{print s+0}'; }
span_max() { grep -F "span 統計 [$2] " "$1" 2>/dev/null | sed -E 's/.*max=([0-9]+)ms.*/\1/' | awk 'BEGIN{m=0} {if ($1>m) m=$1} END{print m}'; }

STAGES="${STAGES:-visual cli}"
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

if has_stage visual; then
  echo "① tasks-tick 節（注入: 置き場の読み ${INJECT_MS} ms）"
  run_section vnew "$TMP/vnew.log"
  grep -E "TAKO_VISUAL_2031" "$TMP/vnew.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vnew.log"; then
    pass "全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "節が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$TMP/vnew.log" | tail -5 | sed 's/^/    /'
  fi

  echo "② A/B: TAKO_2031_LEGACY=1（修正前 = tick が UI スレッドで毎回読む）"
  run_section vlegacy "$TMP/vlegacy.log" TAKO_2031_LEGACY=1
  grep -E "TAKO_VISUAL_2031" "$TMP/vlegacy.log" | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: ① 一覧更新の最中も返答欄への打鍵が止まらない" "$TMP/vlegacy.log"; then
    pass "旧経路では ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/vlegacy.log" | head -1)"
  else
    fail "旧経路でも ① で落ちない（注入が効いていない / 節に検出力が無い）"
    grep -E "FAILED|panicked" "$TMP/vlegacy.log" | tail -3 | sed 's/^/    /'
  fi
fi

# 腕 1 本: GUI を起こして tasks ビューを開き、IPC の応答時間を測る
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
  # 起票は CLI から（IPC の dispatch。起票そのものの読みは #2031 の範囲外なので測る前に済ませる）
  "$TAKO_BIN" todo add "#2031 の実測" >/dev/null 2>&1 || fail "[$arm] tako todo add が通らない"
  "$TAKO_BIN" panel --show --view tasks >/dev/null 2>&1 || fail "[$arm] tako panel --view tasks が通らない"
  # 準備（IPC の dispatch の読み）が入った分布の窓（10 秒ごと）を吐き終えてから測る
  local perf="$TMP/perf-$arm.log" mark
  sleep 11
  mark=$(wc -l < "$perf" 2>/dev/null | tr -d ' ')
  mark=${mark:-0}
  local start end lat max=0 n=0 over=0 t_end
  t_end=$(perl -e "printf \"%.3f\", $(now) + 12")
  while perl -e "exit !($(now) < $t_end)"; do
    start=$(now)
    "$TAKO_BIN" scrollback >/dev/null 2>&1
    end=$(now)
    lat=$(perl -e "printf \"%d\", ($end - $start) * 1000")
    n=$((n + 1))
    [ "$lat" -gt "$max" ] && max=$lat
    [ "$lat" -ge 500 ] && over=$((over + 1))
    sleep 0.2
  done
  # 分布は 10 秒ごとに出るので、もう 1 回ぶん待ってから止める
  sleep 11
  stop_isolated_gui "$ISOLATED_GUI_PID"
  # 測定区間より後に出た分布の行だけを数える
  tail -n +"$((mark + 1))" "$perf" > "$TMP/perf-$arm.window.log" 2>/dev/null
  local win="$TMP/perf-$arm.window.log" ui_reads
  ui_reads=$(span_count "$win" user_tasks:read)
  echo "    [$arm] IPC の応答: ${n} 回・最大 ${max} ms・500 ms 以上 ${over} 回"
  echo "    [$arm] perf.log（測定区間）: periodic_prep:user_tasks 最大 $(span_max "$win" periodic_prep:user_tasks) ms・" \
    "$(span_count "$win" periodic_prep:user_tasks) 回 / 置き場の読み UI スレッド ${ui_reads} 回・background" \
    "$(span_count "$win" bg:user_tasks:read) 回"
  eval "CLI_MAX_$arm=$max CLI_OVER_$arm=$over CLI_UI_READS_$arm=$ui_reads"
}

if has_stage cli; then
  echo "③ 実 CLI: tasks ビューを開いたまま 12 秒、IPC の応答時間（注入: 置き場の読み ${INJECT_MS} ms）"
  measure_cli cnew
  measure_cli clegacy TAKO_2031_LEGACY=1
  if [ "${CLI_MAX_cnew:-99999}" -lt 500 ]; then
    pass "新しい形: IPC の応答の最大 ${CLI_MAX_cnew} ms（UI スレッドが一覧更新で止まらない）"
  else
    fail "新しい形でも IPC の応答が ${CLI_MAX_cnew:-?} ms（UI スレッドが止まっている）"
  fi
  if [ "${CLI_OVER_clegacy:-0}" -ge 1 ]; then
    pass "旧い形: 500 ms 以上の応答が ${CLI_OVER_clegacy} 回・最大 ${CLI_MAX_clegacy} ms（注入が効いている）"
  else
    fail "旧い形でも IPC が止まらない（注入が効いていない）"
  fi
  if [ "${CLI_UI_READS_cnew:-1}" -eq 0 ] && [ "${CLI_UI_READS_clegacy:-0}" -ge 1 ]; then
    pass "UI スレッドで置き場を読んだ回数（測定区間）: 新 0 回 / 旧 ${CLI_UI_READS_clegacy} 回"
  else
    fail "UI スレッドの読みの回数が想定と違う（新 ${CLI_UI_READS_cnew:-?} 回 / 旧 ${CLI_UI_READS_clegacy:-?} 回）"
  fi
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
