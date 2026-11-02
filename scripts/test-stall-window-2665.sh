#!/usr/bin/env bash
# test-stall-window-2665.sh — UI ストールの分類が「遅延の間」に重なった区間だけで決まるかの実経路テスト（#2665）
#
# 実 tako-app（隔離 GUI）の IPC の処理でメインスレッドを塞ぎ（`TAKO_2665_INJECT_FILE`。検証プロセスでだけ効く口。
# 負荷は作らない）、
# perf.log の UI ストールの行の分類を修正後と A/B（`TAKO_2665_LEGACY=1` = 修正前の窓と文）で並べる:
#   ① 計測区間の中 300ms → 区間が閉じた直後に計測区間の外 900ms（1 つの要求で途切れず）:
#      期限より前に終わった区間を「tako の専有」と名指さない（A/B は名指す = #2665 の症状）
#   ② 中 1500ms: 従来どおり「tako の専有（ipc_turn）」と名指す
#   ③ 中 3000ms: 「長時間専有（継続中）」の中間報告が出て、記録は「tako の専有」
#   ④ 外 1200ms の直後に中 400ms（1 つの要求）: 半分に届かなければ内訳と残り（計測区間の外）を書く
#      （A/B は内訳を書かずに「計測区間の外」とだけ書く）
#   対照: 注入なしの 20 秒は UI ストールが出ない
# 記録はタイマーの位相しだい（遅延が 0.5 秒以上になった回だけ記録される）。① の A/B で専有と名指すのは
# 期限が外の 900ms の頭から 0.3〜0.4 秒の位置に来た回（位相の約 1 割）なので、① は回の間の待ちを
# 掃いて位相を一巡させ、回数を多めに回す。
# 窓は仮想ディスプレイ（tako-vd）へ出す。置き場は mktemp 配下で、落とすのは自分で起こした GUI だけ。
#
# 使い方: bash scripts/test-stall-window-2665.sh
#   ① の回数は TAKO_2665_TRIALS_1（既定 40）、④ は TAKO_2665_TRIALS_4（既定 10）。
#   証拠（各回の perf.log と区切り）は TAKO_2665_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2665_LEGACY TAKO_1968_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1（${3}）"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_ge() {
  if [ "$3" -ge "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 以上のはずが ${3}）"; fi
}

TRIALS_1="${TAKO_2665_TRIALS_1:-40}"
TRIALS_4="${TAKO_2665_TRIALS_4:-10}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2665-XXXXXX)"
INJ="$TMP/inject"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# 注入して `tako window list` を 1 回送る（$1 = 注入の中身。例 `in=1500` / `in=300 after=900`）
inject() {
  printf '%s\n' "$1" > "$INJ"
  iso_cli window list > /dev/null 2>&1
  : > "$INJ"
}

# perf.log の行数（区切りに使う。perf.log は追記だけ）
perf_lines() {
  if [ -f "$PERF" ]; then wc -l < "$PERF" | tr -dc '0-9'; else echo 0; fi
}

# 区切り $1〜$2 行の間に足された行
perf_between() {
  if [ "$2" -gt "$1" ]; then sed -n "$(($1 + 1)),$2p" "$PERF"; fi
}

count() { grep -c "$1" | tr -dc '0-9'; }

# 1 回ぶんの GUI を起こして対照と ①〜④ を回す（$1 = 接頭辞・$2… = GUI へ渡す VAR=VAL）
run_round() {
  local r="$1" i a b
  shift
  PERF="$TMP/d-$r/perf.log"
  mkdir -p "$TMP/d-$r"
  : > "$INJ"
  launch_isolated_gui "$TMP/app-$r.log" TAKO_DATA_DIR="$TMP/d-$r" TAKO_2665_INJECT_FILE="$INJ" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$r.log" 600 || exit 1
  # 起動直後の動きが落ち着くまで待つ
  sleep 15

  a=$(perf_lines)
  sleep 20
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$r-control.txt"

  a=$(perf_lines)
  for i in $(seq 1 "$TRIALS_1"); do
    inject "in=300 after=900"
    # 次の回の期限の位置をずらす。要求（1.2 秒）は必ず期限を含むので監視ループは毎回この要求の
    # 終わりで掛け直し、次の期限の位置は待ちの長さで決まる。待ちを 2〜3 秒で掃いて位相を一巡させる
    # （固定の待ちだと期限が毎回同じ位置 = 中の区間の頭に来て、A/B の症状の帯を一度も踏まない）
    sleep "$(awk -v k="$i" -v n="$TRIALS_1" 'BEGIN { printf "%.3f", 2 + k / n }')"
  done
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$r-1.txt"

  a=$(perf_lines)
  for i in 1 2 3 4 5 6; do
    inject "in=1500"
    sleep 4
  done
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$r-2.txt"

  a=$(perf_lines)
  for i in 1 2 3 4 5; do
    inject "in=3000"
    sleep 5
  done
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$r-3.txt"

  a=$(perf_lines)
  for i in $(seq 1 "$TRIALS_4"); do
    inject "out=1200 in=400"
    sleep 4
  done
  sleep 2
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$r-4.txt"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
  if [ -f "$PERF" ]; then command cp -f "$PERF" "$TMP/$r-perf.log"; fi
}

run_round new
run_round legacy TAKO_2665_LEGACY=1

stalls() { grep 'UI ストール' "$TMP/$1.txt"; }

for r in new legacy; do
  echo
  echo "== [$r] 対照（注入なし 20 秒）=="
  check_eq "[$r] UI ストールが出ない" 0 "$(stalls "$r-control" | count .)"
done

echo
echo "== ① 中 300ms → 外 900ms（$TRIALS_1 回）=="
check_ge "[new] 記録された回（遅延 0.5 秒以上）" 5 "$(stalls new-1 | count .)"
check_eq "[new] 「tako の専有」と名指さない" 0 "$(stalls new-1 | count 'tako の専有')"
check_eq "[new] 記録は「遅延の間に計測区間なし」か内訳（期限が中に来た回）" "$(stalls new-1 | count .)" "$(stalls new-1 | grep -c -e '遅延の間に計測区間なし' -e '遅延の間の計測区間は計' | tr -dc '0-9')"
check_ge "[legacy] 修正前は期限より前に終わった ipc_turn を専有と名指す（A/B）" 1 "$(stalls legacy-1 | count 'tako の専有')"

echo
echo "== ② 中 1500ms（6 回）=="
for r in new legacy; do
  check_eq "[$r] 6 回とも「tako の専有（ipc_turn）」" 6 "$(stalls "$r-2" | grep 'tako の専有' | count 'ipc_turn')"
done

echo
echo "== ③ 中 3000ms（5 回）=="
for r in new legacy; do
  check_eq "[$r] 長時間専有（継続中）の中間報告が 5 回" 5 "$(count 'メインスレッド長時間専有（継続中）: ipc_turn' < "$TMP/$r-3.txt")"
  check_eq "[$r] 記録は 5 回とも「tako の専有」" 5 "$(stalls "$r-3" | count 'tako の専有')"
done

echo
echo "== ④ 外 1200ms → 中 400ms（1 つの要求・$TRIALS_4 回）=="
check_ge "[new] 半分に届かない回は内訳と残りを書く" 5 "$(stalls new-4 | grep '遅延の間の計測区間は計' | count '残り .*ms は計測区間の外')"
check_eq "[new] 内訳の回は「tako の専有」と名指さない" 0 "$(stalls new-4 | grep '遅延の間の計測区間は計' | count 'tako の専有')"
check_eq "[new] 記録はどれも中の ipc_turn を内訳か専有で名指す" "$(stalls new-4 | count .)" "$(stalls new-4 | count '最長 ipc_turn')"
check_ge "[legacy] 修正前は内訳を書かずに「計測区間の外」とだけ書く（A/B）" 5 "$(stalls legacy-4 | grep -v '残り' | count '直前の計測区間は計 .*計測区間の外')"

if [ -n "${TAKO_2665_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2665_EVIDENCE_DIR"
  for r in new legacy; do
    for f in control.txt 1.txt 2.txt 3.txt 4.txt perf.log; do
      if [ -e "$TMP/$r-$f" ]; then command cp -f "$TMP/$r-$f" "$TAKO_2665_EVIDENCE_DIR/$r-$f"; fi
    done
  done
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
