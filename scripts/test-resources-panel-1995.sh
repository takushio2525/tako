#!/usr/bin/env bash
# tako:run bash scripts/test-resources-panel-1995.sh
#
# test-resources-panel-1995.sh — 右パネルの resources ビューとステータスバーの区画（#1995 / エピック #1992 の S3。
# FR-2.43.15）の実経路テスト
#
# 隔離 GUI（tako-vd）を**明示の調停役**（`TAKO_QUEUE_SOCKET` + 注入 `TAKO_QUEUE_INJECT` = 同時数 1・メモリ圧 normal）
# で起こし、visual-test `resources-panel-1995` が本物の wrapper（`tako queue sleep 600`）を 3 本並べて実マウスで押す:
#   ① 区画が「実行 1 · 待ち 2」② 区画から開いたビューの行 = CLI `tako queue --json` ③ 行を押すとそのペインへ
#   ④ 段のチップ（GUI）・MCP `tako_queue`・CLI `tako queue priority --pane` が同じ答え ⑤ `tako panel --view resources`
#   と MCP `tako_panel` で開く ⑥ 取り消しの確かめ（やめる / 取り消す = 125）⑦ ライトテーマ ⑧ 実行中の取り消し・
#   空の待ち行列 ⑨ 機の値の無い OS（未対応）・メモリ圧 critical / warn / normal の区画の色（実フレームの画素）
# 終わった後に perf.log の UI ストール（32 ms 超）を数え、resources ビューの区間（periodic_prep:resources・
# dispatch:Queue・offload_prepare:Queue）の記録が無いことを見る。
#
# **本番の tako には触らない**: data / discovery / tmux は mktemp 配下とこの実行の専用ソケット。調停役は隔離 GUI
# 自身（本番の順番待ちへは並ばない）。wrapper は visual-test が起こした子だけを止める（pkill / killall は使わない）。
# **負荷は生成しない**: 待たせる処理は眠るだけ。**CI には登録しない**（実 GUI が要る）。CI 側の担保は
# `resources_panel` / `ui_text::resources` の単体テストと番犬 `issue1993_resources_ui_thread_watchdog` の 8 番。
#
# 使い方: bash scripts/test-resources-panel-1995.sh   （RES1995_DUMP_DIR=<dir> で画像の置き場を変える）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TAKO_CLI
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_1994_INJECT
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
UNMEASURED=""

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 短い一時 dir（深いと受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk1995-XXXXXX)"
DUMP_DIR="${RES1995_DUMP_DIR:-$TMP/dump}"
APP_PID=""
MASK_USER="$(id -un)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<TMP>#g"; }

cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "tk1995-$$" kill-server >/dev/null 2>&1 || true
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "resources ビュー（#1995）の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &
# 右パネル 440px + 2 枚のペイン + ステータスバーの区画が読める大きさ
ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-40,40,1360,820}"
mkdir -p "$DUMP_DIR"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tk1995-$$"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 画面の文は日本語で撮る（英語は単体テストの catalog が縛る）
printf '{"language":"ja"}' > "$TAKO_DATA_DIR/settings.json"
# 36 GB の機に見せて同時数 1（12 GB あたり 1 = 注入の total 12 GB）・空き 10 GB・normal・立ち上げ直後の待ち 0
INJECT="$TMP/inject.json"
printf '{"total_mb":12288,"available_mb":10000,"pressure":"normal","swap_used_mb":0,"cpus":8,"unmanaged":0,"regather_secs":0}\n' > "$INJECT"

echo "== 1. visual-test resources-panel-1995（隔離 GUI = 明示の調停役・本物の wrapper 3 本）=="
LOG="$TMP/visual.log"
rc=0
if launch_isolated_gui "$LOG" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=resources-panel-1995 \
  TAKO_VISUAL_DUMP_DIR="$DUMP_DIR" TAKO_1995_CLI="$TAKO_BIN" \
  TAKO_QUEUE_SOCKET="$TMP/q.sock" TAKO_QUEUE_INJECT="$INJECT"; then
  APP_PID="$ISOLATED_GUI_PID"
  for _ in $(seq 1 3000); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  grep -E "ディスプレイ指定|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED|TAKO_SELF_TEST_STATE_TIMEOUT|TAKO_RES_1995" "$LOG" \
    | mask | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$LOG"; then
    pass "visual: 区画 → ビュー → 行でフォーカス → 段（GUI / MCP / CLI）→ panel（CLI / MCP）→ 取り消しの確かめ → 空 → 未対応と色"
  else
    fail "visual-test が通らない（$(grep -m1 -E "TAKO_APP_SELF_TEST_FAILED|panicked" "$LOG" | mask)）"
  fi
else
  rc=$?
  if [ "$rc" -eq 4 ]; then
    UNMEASURED="${UNMEASURED} 隔離 GUI（tako-vd を用意できない）"
  else
    fail "隔離 GUI を起こせない（$rc）"
  fi
fi

echo "== 2. perf.log: resources ビューの区間に 32 ms 超の記録が無い =="
PERF="$TAKO_DATA_DIR/perf.log"
if [ -f "$PERF" ]; then
  HITS="$(grep -cE "periodic_prep:resources|dispatch:Queue|offload_prepare:Queue" "$PERF" || true)"
  STALLS="$(grep -c "メインスレッド専有" "$PERF" || true)"
  echo "    perf.log の行 $(wc -l < "$PERF" | tr -d ' ')・メインスレッド専有 ${STALLS}・うち resources の区間 ${HITS}"
  # 参考: 専有の上位のタグ（起動・debug ビルドの描画など resources 以外も含む）
  grep "メインスレッド専有" "$PERF" | sed -E 's/.*専有: ([^ ]+) が.*/\1/' | sort | uniq -c | sort -rn | head -5 | sed 's/^/      /'
  grep -E "periodic_prep:resources|dispatch:Queue|offload_prepare:Queue" "$PERF" | mask | sed 's/^/    /' | head -5
  if [ "${HITS:-0}" = "0" ]; then
    pass "resources の区間（periodic_prep:resources / dispatch:Queue / offload_prepare:Queue）は 32 ms を超えていない"
  else
    fail "resources の区間に 32 ms 超の記録がある（${HITS} 行）"
  fi
elif [ -z "$UNMEASURED" ]; then
  pass "perf.log が無い = 32 ms 超の区間が 1 つも無かった"
fi

echo "== 3. 画像の個人情報（ユーザー名・ホームパス）=="
if command -v tesseract >/dev/null 2>&1 && ls "$DUMP_DIR"/*.png >/dev/null 2>&1; then
  leak=0
  for f in "$DUMP_DIR"/*.png; do
    if tesseract "$f" - 2>/dev/null | grep -qiE "${MASK_USER}|/Users/"; then
      leak=1
      echo "    写り込み: $(basename "$f")"
    fi
  done
  [ "$leak" = 0 ] && pass "画像の文字（OCR）にユーザー名・ホームパスが無い" || fail "画像にユーザー名かホームパスが写っている"
else
  echo "    （tesseract が無い・画像が無い = 目視で確かめる）"
fi

echo
if [ -n "${KEEP:-}" ] || [ -n "${RES1995_DUMP_DIR:-}" ]; then
  echo "画像: $DUMP_DIR" | mask
fi
echo "PASS=$PASS FAIL=$FAIL${UNMEASURED:+ 未実測:$UNMEASURED}"
[ "$FAIL" -eq 0 ]
