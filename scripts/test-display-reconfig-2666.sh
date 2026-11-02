#!/usr/bin/env bash
# test-display-reconfig-2666.sh — UI ストールの分類が画面構成の変更（macOS の画面再構成）を名指すかの実経路テスト（#2666）
#
# 蓋の開閉は検証で起こせず、tako-vd も作り直さない・モードも変えないので、画面構成の変更は
# **通知の合成**で再現する: 実 tako-app（隔離 GUI）の IPC の処理で、本物のコールバックの関数を合成の印つきで
# 呼び（`TAKO_2666_INJECT_FILE`。検証プロセスでだけ効く口。負荷は作らない）、その前後でメインスレッドを塞ぐ。
# perf.log の UI ストールの行の分類を修正後と A/B（`TAKO_2666_LEGACY=1` = #2665 の分類）で並べる:
#   ① begin → 計測区間の外で 1800ms → end: 「画面構成の変更の処理中」と名指す（A/B は「計測区間の外」）
#   ② begin → end → 外で 1800ms（本番の形 = end の後に塞がる）: 猶予の内なので名指す
#   ③ begin → 計測区間の中で 1800ms → end: 「tako の専有」と名指したまま「画面構成の変更とも重なる」と併記
#   ④ begin → 外 1200ms → 中 400ms → end: 区間が半分に届かない回は内訳を書き、残りを画面構成の変更と名指す
#   ⑤ 再構成なし（#2665 の注入 `TAKO_2665_INJECT_FILE`）: 専有 / 内訳 / 計測区間の外のまま（画面構成を書かない）
#   対照: 注入なしの 20 秒は UI ストールも再構成の行も出ない
#   どの合成も perf.log に再構成の行が 1 回 1 行出て「コールバックはメインスレッド・検証の合成」と書く
# 窓は仮想ディスプレイ（tako-vd）へ出す。置き場は mktemp 配下で、落とすのは自分で起こした GUI だけ。
#
# 使い方: bash scripts/test-display-reconfig-2666.sh
#   各場面の回数は TAKO_2666_TRIALS（既定 6。④ と ⑤ の内訳は倍）。
#   証拠（各回の perf.log と区切り）は TAKO_2666_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2666_LEGACY TAKO_2665_LEGACY TAKO_1968_LEGACY

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

TRIALS="${TAKO_2666_TRIALS:-6}"
TRIALS_2X=$((TRIALS * 2))
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2666-XXXXXX)"
INJ66="$TMP/inject-2666"
INJ65="$TMP/inject-2665"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# 注入して `tako window list` を 1 回送る（$1 = 注入のファイル・$2 = 中身。例 `begin sleep=1800 end`）
inject() {
  printf '%s\n' "$2" > "$1"
  iso_cli window list > /dev/null 2>&1
  : > "$1"
}

# perf.log の行数（区切りに使う。perf.log は追記だけ）
perf_lines() {
  if [ -f "$PERF" ]; then wc -l < "$PERF" | tr -dc '0-9'; else echo 0; fi
}

# 区切り $1〜$2 行の間に足された行
perf_between() {
  if [ "$2" -gt "$1" ]; then sed -n "$(($1 + 1)),$2p" "$PERF"; fi
}

count() { grep -c -E -e "$1" | tr -dc '0-9'; }

# 場面を $2 回回して足された行を $TMP/$1.txt へ（$3 = 注入のファイル・$4 = 中身）。
# 回の間は 4〜5 秒空ける: 再構成の猶予 2 秒を必ず越え（前の回の再構成が次の回へ重ならない）、
# 待ちの長さを掃いて監視ループの期限の位置を一巡させる（固定の待ちだと期限が毎回塞ぎの同じ位置に来て、
# ④ の「区間が半分に届かない回」を一度も踏まないことがある = #2665 の ① と同じ）
scene() {
  local name="$1" n="$2" file="$3" body="$4" a b i
  a=$(perf_lines)
  for i in $(seq 1 "$n"); do
    inject "$file" "$body"
    sleep "$(awk -v k="$i" -v n="$n" 'BEGIN { printf "%.3f", 4 + k / n }')"
  done
  # 最後の回の再構成の行（settle 300ms の後に監視スレッドが書く）を待つ
  sleep 1
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$name.txt"
}

# 1 回ぶんの GUI を起こして場面を回す（$1 = 接頭辞・$2 = all なら全場面・$3… = GUI へ渡す VAR=VAL）
run_round() {
  local r="$1" which="$2" a b
  shift 2
  PERF="$TMP/d-$r/perf.log"
  mkdir -p "$TMP/d-$r"
  : > "$INJ66"
  : > "$INJ65"
  launch_isolated_gui "$TMP/app-$r.log" TAKO_DATA_DIR="$TMP/d-$r" \
    TAKO_2666_INJECT_FILE="$INJ66" TAKO_2665_INJECT_FILE="$INJ65" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$r.log" 600 || exit 1
  # 起動直後の動きが落ち着くまで待つ
  sleep 15

  a=$(perf_lines)
  sleep 20
  b=$(perf_lines)
  perf_between "$a" "$b" > "$TMP/$r-control.txt"

  scene "$r-1" "$TRIALS" "$INJ66" "begin sleep=1800 end"
  scene "$r-3" "$TRIALS" "$INJ66" "begin span=1800 end"
  if [ "$which" = all ]; then
    scene "$r-2" "$TRIALS" "$INJ66" "begin end sleep=1800"
    scene "$r-4" "$TRIALS_2X" "$INJ66" "begin sleep=1200 span=400 end"
    scene "$r-5in" "$TRIALS" "$INJ65" "in=1800"
    scene "$r-5part" "$TRIALS_2X" "$INJ65" "out=1200 in=400"
    scene "$r-5out" "$TRIALS" "$INJ65" "out=1800"
  fi

  stop_isolated_gui "$APP_PID"
  APP_PID=""
  if [ -f "$PERF" ]; then command cp -f "$PERF" "$TMP/$r-perf.log"; fi
}

run_round new all
run_round legacy some TAKO_2666_LEGACY=1

stalls() { grep 'UI ストール' "$TMP/$1.txt"; }
reconfigs() { grep '画面構成の変更（macOS の画面再構成）' "$TMP/$1.txt"; }
NAMED='画面構成の変更の処理中（OS / GPUI 側）'

for r in new legacy; do
  echo
  echo "== [$r] 対照（注入なし 20 秒）=="
  check_eq "[$r] UI ストールが出ない" 0 "$(stalls "$r-control" | count .)"
  check_eq "[$r] 再構成の行が出ない" 0 "$(reconfigs "$r-control" | count .)"
done

echo
echo "== ① begin → 外 1800ms → end（$TRIALS 回）=="
check_eq "[new] 毎回記録される" "$TRIALS" "$(stalls new-1 | count .)"
# 塞ぎの後の `ipc_turn`（計測区間）が 5ms を超えた回は「内訳 + 残りは画面構成の変更」になるので両方を数える
check_eq "[new] 毎回「画面構成の変更の処理中」と名指す" "$TRIALS" "$(stalls new-1 | grep -E "$NAMED" | count '・検証の合成。#2666）')"
check_eq "[new] 「tako の専有」と名指さない" 0 "$(stalls new-1 | count 'tako の専有')"
check_eq "[new] 再構成の行が 1 回 1 行・メインスレッド・検証の合成" "$TRIALS" "$(reconfigs new-1 | count '画面 1,2・通知 begin 2 件 / end 2 件・変更 desktop_shape・コールバックはメインスレッド・検証の合成。#2666')"
check_eq "[legacy] 修正前は「計測区間の外」と書く（A/B）" "$TRIALS" "$(stalls legacy-1 | count '(遅延の間に計測区間なし = |ms は)計測区間の外')"
check_eq "[legacy] 修正前は画面構成を書かない（A/B）" 0 "$(stalls legacy-1 | count '画面構成')"
check_eq "[legacy] 修正前は再構成の行が出ない（A/B）" 0 "$(reconfigs legacy-1 | count .)"

echo
echo "== ② begin → end → 外 1800ms（$TRIALS 回）=="
check_eq "[new] 毎回記録される" "$TRIALS" "$(stalls new-2 | count .)"
check_eq "[new] 猶予の内なので毎回名指し、end から記録までを書く" "$TRIALS" "$(stalls new-2 | grep "$NAMED" | count 'end から記録まで ')"

echo
echo "== ③ begin → 中 1800ms → end（$TRIALS 回・優先の決まり）=="
check_eq "[new] 毎回「tako の専有（t2666_inject）」と名指す" "$TRIALS" "$(stalls new-3 | grep 'tako の専有' | count '最長 t2666_inject')"
check_eq "[new] 毎回「画面構成の変更とも重なる」と併記する" "$TRIALS" "$(stalls new-3 | grep 'tako の専有' | count '。画面構成の変更とも重なる: begin → end ')"
check_eq "[new] 専有の回は「画面構成の変更の処理中」と名指さない" 0 "$(stalls new-3 | count "$NAMED")"
check_eq "[legacy] 修正前は専有だけ（併記しない。A/B）" "$TRIALS" "$(stalls legacy-3 | grep 'tako の専有' | grep -v '画面構成' | count '最長 t2666_inject')"

echo
echo "== ④ begin → 外 1200ms → 中 400ms → end（$TRIALS_2X 回）=="
check_ge "[new] 区間が半分に届かない回は内訳と、残りを画面構成の変更と名指す" "$TRIALS" "$(stalls new-4 | grep '遅延の間の計測区間は計' | count "残り .*ms は$NAMED")"
check_eq "[new] 記録はどれも画面構成の変更を名指すか併記する" "$(stalls new-4 | count .)" "$(stalls new-4 | count "$NAMED|画面構成の変更とも重なる")"
check_eq "[new] 名指した回は「tako の専有」と書かない" 0 "$(stalls new-4 | grep "$NAMED" | count 'tako の専有')"

echo
echo "== ⑤ 再構成なし（#2665 の注入）=="
check_eq "[new] 中 1800ms は $TRIALS 回とも「tako の専有（ipc_turn）」" "$TRIALS" "$(stalls new-5in | grep 'tako の専有' | count 'ipc_turn')"
check_ge "[new] 外 1200ms → 中 400ms は内訳と残り（計測区間の外）" "$TRIALS" "$(stalls new-5part | grep '遅延の間の計測区間は計' | count '残り .*ms は計測区間の外')"
check_eq "[new] 外 1800ms は $TRIALS 回とも「計測区間の外」" "$TRIALS" "$(stalls new-5out | count '(遅延の間に計測区間なし = |ms は)計測区間の外')"
check_eq "[new] 再構成が無ければ画面構成を書かない" 0 "$(cat "$TMP/new-5in.txt" "$TMP/new-5part.txt" "$TMP/new-5out.txt" | count '画面構成')"

if [ -n "${TAKO_2666_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2666_EVIDENCE_DIR"
  for f in "$TMP"/new-*.txt "$TMP"/legacy-*.txt "$TMP"/*-perf.log; do
    if [ -e "$f" ]; then command cp -f "$f" "$TAKO_2666_EVIDENCE_DIR/"; fi
  done
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
