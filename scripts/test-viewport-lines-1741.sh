#!/usr/bin/env bash
# test-viewport-lines-1741.sh — 可視行数が描いた行の実矩形と合うかの実 GUI テスト（#1741 / #2714）
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、visual-test 節 `viewport-lines` を
# 走らせる。節が見るのは次の 6 相で、正解は**実際に描いた行の矩形**（器に収まる行）から採る:
#   (1) 寸法: 追従が使う可視行数 = 実矩形の行数
#   (2) 下端: ↓ を押し続けると、器は「可視行数 − 余白 m」の行で動き始め、以降は下の余白が m 行
#       （m は製品と同じ `effective_margin`: 既定 3 行、狭い器では 2m + 1 行が収まるまで詰める。#2714）
#   (3) 上端: ↑ で戻ると、器が動き始めてからは上の余白が 3 行
#   (4) Page Down / Up: 歩幅は可視行数 − 1、着地のあとも余白 3 行で見える
#   (5) 文字サイズ: ⌘+ と全体の文字サイズの最大（32）・最小（8）でも (1)(2) が成り立つ
#   (6) 1 行 / 視野より短い / 折り返しのある行のファイル
# 追従を見る相は、器の高さが基準の段から動いていないこと（前提）を先に確かめ、動いていれば
# 「前提が崩れた」と名指して設計の判定を省く（#2714）。
# 打鍵は `window.dispatch_keystroke` へ流すので、キーバインド判定 → 編集の入口 → 追従の全段を通る。
#
# 腕（#2714）:
#   ① 広い窓（1400×880）                       … 緑。font-max は 12 行前後
#   ② 狭い器（960×600 = 既定の窓）               … 緑。font-max は折り返して可視 7 行以下 = 余白が詰まる器
#   ③ ② + 未導入の言語サーバの帯（注入 `rs`）     … 緑。帯が器の高さを取り font-max は可視 2 行前後
#   ④ ② を旧の設計値（TAKO_2714_LEGACY=1）で     … font-max の「動き始めた行」が名指しで落ちる
#   ⑤ ③ を旧の設計値で                          … 「引けない」（修正前は panic）が名指しで落ちる
#   ⑥ ① + 下端の途中で器を縮める（注入 `shrink` = 下を分割） … 下端の「前提が崩れた」が名指しで落ちる
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-viewport-lines-1741.sh
#   （腕を絞るなら `TAKO_1741_ARMS="1 2"`。修正前のビルドと比べるときは `APP_BIN=<そのビルドの tako-app>`）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1741-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1741-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 窓の寸法は腕ごとに固定する（器の高さが実行ごとに変わると可視行数の実測値が比べられない）
WIDE="0,0,1400,880"
NARROW="0,0,960,600"
# 未導入の言語サーバ（差し替えの口に存在しないパス = 編集を始めた直後に未導入の帯が出る）
NO_SERVER="$TMP/no-such-rust-analyzer"
ARMS="${TAKO_1741_ARMS:-1 2 3 4 5 6}"
LOG=""

# 腕を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 300 秒）
# $1 = 腕の名前（ログ名）/ $2 = 窓の矩形 / 残り = 追加の env（`VAR=VAL`）
run_arm() {
  local name="$1" bounds="$2" rc=0 pid
  shift 2
  LOG="$TMP/$name.log"
  ISOLATED_GUI_BOUNDS="$bounds"
  launch_isolated_gui "$LOG" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=viewport-lines ${1+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "隔離 GUI を立てられない（終了コード ${rc}。4 = 面を用意できない = 未実測）"
    exit "$rc"
  fi
  pid="$ISOLATED_GUI_PID"
  for _ in $(seq 1 3000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  grep -E "TAKO_VISUAL_PIXEL: viewport-lines (arm|geometry|zoom-in|font-max |font-min |wrapped truth|.* judge|NG|inject)" "$LOG" \
    | sed 's/^/    /'
  if grep -q "未知の節" "$LOG"; then
    fail "$name: visual-test 付きでビルドされていない（節が無い）"
  fi
  if grep -q "panicked" "$LOG"; then
    fail "$name: プロセスが panic した（$(grep -m1 -A1 panicked "$LOG" | tr '\n' ' ')）"
  fi
}

# 実矩形の可視行数と追従の可視行数が、文字サイズを変えても一致している
same_counts() {
  local label line truth follow
  for label in geometry zoom-in font-max font-min; do
    line=$(grep "TAKO_VISUAL_PIXEL: viewport-lines $label " "$LOG" | head -1)
    truth=$(printf '%s' "$line" | sed -n 's/.*truth=\([0-9]*\).*/\1/p')
    follow=$(printf '%s' "$line" | sed -n 's/.*follow_lines=\([0-9]*\).*/\1/p')
    if [ -n "$truth" ] && [ "$truth" = "$follow" ]; then
      pass "$1 $label: 追従の可視行数 = 実矩形の行数（$truth 行）"
    else
      fail "$1 $label: 実矩形 ${truth:-?} 行 / 追従 ${follow:-?} 行"
    fi
  done
}

# font-max の判定行の可視行数と余白（`VISIBLE` / `MARGIN` に入れる。読めなければ空）
font_max_judge() {
  local line
  line=$(grep "TAKO_VISUAL_PIXEL: viewport-lines font-max judge" "$LOG" | head -1)
  VISIBLE=$(printf '%s' "$line" | sed -n 's/.*visible=\([0-9]*\).*/\1/p')
  MARGIN=$(printf '%s' "$line" | sed -n 's/.*margin=\([0-9]*\).*/\1/p')
}

green() {
  if grep -q "TAKO_VISUAL_TEST_OK" "$LOG"; then
    pass "$1: 6 相すべて緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "$1: 節が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$LOG" | tail -5 | sed 's/^/    /'
  fi
}

# 落ちる腕: 緑にならず、FAILED の行に $2（名指しの文）が入っている
named_fail() {
  local failed
  failed=$(grep "TAKO_APP_SELF_TEST_FAILED" "$LOG")
  if grep -q "TAKO_VISUAL_TEST_OK" "$LOG"; then
    fail "$1: 緑になった（落ちるはずの腕。検出力が無い）"
  elif [[ "$failed" == *"$2"* ]]; then
    pass "$1: 名指しで落ちる（$2）"
  else
    fail "$1: 名指しの文「$2」が FAILED の行に無い"
    grep -E "FAILED|panicked" "$LOG" | tail -3 | sed 's/^/    /'
  fi
}

for arm in $ARMS; do
  case "$arm" in
    1)
      echo "=== ① 広い窓（1400×880） ==="
      run_arm wide "$WIDE"
      green "①"
      same_counts "①"
      ;;
    2)
      echo "=== ② 狭い器（960×600） ==="
      run_arm narrow "$NARROW"
      green "②"
      same_counts "②"
      font_max_judge
      if [ "${VISIBLE:-99}" -le 7 ] && [ "${MARGIN:-3}" -lt 3 ]; then
        pass "② font-max は可視 ${VISIBLE} 行 = 余白が ${MARGIN} 行へ詰まる器を判定した"
      else
        fail "② font-max が余白の詰まる器にならない（visible=${VISIBLE:-?} margin=${MARGIN:-?}）"
      fi
      ;;
    3)
      echo "=== ③ 狭い器 + 未導入の言語サーバの帯（注入 rs） ==="
      run_arm band "$NARROW" TAKO_LSP_AUTOSTART=1 \
        TAKO_LSP_BIN_RUST_ANALYZER="$NO_SERVER" TAKO_2714_INJECT=rs
      green "③"
      font_max_judge
      if [ "${VISIBLE:-99}" -le 2 ]; then
        pass "③ font-max は可視 ${VISIBLE} 行（余白 ${MARGIN:-?} 行）= 修正前は引き算が溢れた器でも判定が通る"
      else
        fail "③ 帯で可視 2 行以下の器にならない（visible=${VISIBLE:-?}）"
      fi
      ;;
    4)
      echo "=== ④ ② を旧の設計値で（TAKO_2714_LEGACY=1） ==="
      run_arm legacy-narrow "$NARROW" TAKO_2714_LEGACY=1
      named_fail "④" "font-max: 器が動き始めた行"
      ;;
    5)
      echo "=== ⑤ ③ を旧の設計値で（TAKO_2714_LEGACY=1） ==="
      run_arm legacy-band "$NARROW" TAKO_LSP_AUTOSTART=1 \
        TAKO_LSP_BIN_RUST_ANALYZER="$NO_SERVER" TAKO_2714_LEGACY=1
      named_fail "⑤" "行から余白 3 行を引けない"
      ;;
    6)
      echo "=== ⑥ 広い窓 + 下端の途中で器を縮める（注入 shrink） ==="
      run_arm shrink "$WIDE" TAKO_2714_INJECT=shrink
      named_fail "⑥" "bottom-edge: 前提が崩れた"
      ;;
    *) echo "知らない腕: $arm"; exit 2 ;;
  esac
done

# ログは終了時に `$TMP` ごと消える。残すなら TAKO_1741_KEEP=1（置き場は $TMPDIR）
if [ "${TAKO_1741_KEEP:-}" = 1 ]; then
  keep="${TMPDIR:-/tmp}/tako-1741-last"
  mkdir -p "$keep" && command cp -f "$TMP"/*.log "$keep"/ && echo "ログを $keep へ残した"
fi
echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
