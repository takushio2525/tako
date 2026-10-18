#!/bin/bash
# test-workspace-search-1694.sh — ワークスペース横断検索（#1694）の実経路テスト
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定には触らない）:
#   ① 1 万ファイルのツリー（一時 dir に生成し、終わったら消す）を `tako search` で探す: 件数が
#      生成した答えと一致する（.gitignore の配下・バイナリは数えない）・所要
#   ② 検索のあいだ UI スレッドが止まらない: 走査が数秒かかる検索を打ちながら別の要求
#      （`tako scrollback` = UI スレッドで同期に答える）の応答時間を測り、最大が上限以下
#   ③ 検索のあいだのメインスレッドのスタック（`sample`）に走査の関数が居ない（ほかのスレッドには居る）
#   ④ エッジ: 0 件 / 上限で打ち切り（stopped=limit と表示）/ 壊れた正規表現（理由つきで失敗）/
#      存在しない範囲 / 読めないファイル / シンボリックリンクのループ / 検索中の再検索（前は取り消し）/
#      検索中の --clear（取り消して保持 0）
#   ⑤ visual-test `workspace-search-1694`（⇧⌘F → 打鍵 → Enter → 行の ⌘クリックで新しいペインが
#      1 枚増え該当行が見える → × で保持 0）
#
# A/B: `TAKO_OFFLOAD=0`（IPC の要求を UI スレッドで同期に処理する既存の切り分けスイッチ = 走査が
# UI スレッドで走る形）で ② が名指しで NG になる（`WS1694_AB=1 bash scripts/test-workspace-search-1694.sh`）。
#
# 使い方: bash scripts/test-workspace-search-1694.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AB="${WS1694_AB:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ -n "$1" ]; then pass "$2"; else fail "$2"; fi; }

# 「UI が止まっていない」とみなす別の要求の応答の上限（秒）
UI_REPLY_LIMIT=0.5
FILES_DIRS=100
FILES_PER_DIR=100
LINES=30

TMP="$(mktemp -d /tmp/tako-1694-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  chmod -R u+rwx "$TMP" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERF_LOG="$TMP/perf.log"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/zdot"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
# 画像にユーザー名・ホスト名が写らないよう固定のプロンプト（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# --- 1 万ファイルのツリー（git のリポジトリ。ignored/ は .gitignore・blob.bin はバイナリ）---
TREE="$TMP/tree"
perl - "$TREE" "$FILES_DIRS" "$FILES_PER_DIR" "$LINES" <<'PERL'
use strict; use warnings;
my ($root, $dirs, $per, $lines) = @ARGV;
mkdir $root; mkdir "$root/.git"; mkdir "$root/ignored";
open my $gi, '>', "$root/.gitignore" or die; print $gi "ignored/\n"; close $gi;
my $hits = 0;
for my $d (0 .. $dirs - 1) {
  my $dir = sprintf "%s/d%03d", $root, $d;
  mkdir $dir;
  for my $f (0 .. $per - 1) {
    my $n = $d * $per + $f;
    open my $fh, '>', sprintf("%s/f%03d.rs", $dir, $f) or die;
    for my $l (1 .. $lines) {
      if ($n % 10 == 0 && $l == ($n % $lines) + 1) { print $fh "    let zebra_needle = $n; // line $l\n"; $hits++; }
      else { print $fh "    // file $n line $l lorem ipsum dolor sit amet\n"; }
    }
    close $fh;
  }
}
for my $i (0 .. 499) {
  open my $fh, '>', "$root/ignored/g$i.rs" or die; print $fh "zebra_needle\n"; close $fh;
}
open my $bin, '>', "$root/blob.bin" or die; binmode $bin; print $bin "zebra_needle\0\x01\x02"; close $bin;
open my $ans, '>', "$root/../answer" or die; print $ans "$hits\n"; close $ans;
PERL
EXPECTED="$(cat "$TMP/answer")"
NFILES="$(find "$TREE" -type f -not -path '*/.git/*' | wc -l | tr -d ' ')"
echo "ツリー: $NFILES ファイル（数える答え $EXPECTED 行。ignored/ の 500 行と blob.bin は数えない）"

# `cmd` を `secs` 秒で打ち切って実行し、所要（秒）を出す。打ち切ったら "timeout"
timed() {
  local secs="$1"; shift
  local t0 t1 rc
  t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  perl -e 'alarm shift; exec @ARGV' "$secs" "$@" > "$TMP/timed.out" 2>&1
  rc=$?
  t1=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  if [ "$rc" -eq 142 ]; then echo "timeout"; else perl -e "printf '%.3f', $t1 - $t0"; fi
}
tk() { "$TAKO_BIN" "$@"; }

# --- GUI（本体）---
ENV_EXTRA=(ZDOTDIR="$TMP/zdot")
[ -n "$AB" ] && ENV_EXTRA+=(TAKO_OFFLOAD=0) && echo "A/B: TAKO_OFFLOAD=0（IPC を UI スレッドで同期に処理する形）"
launch_isolated_gui "$TMP/app.log" ${ENV_EXTRA[@]+"${ENV_EXTRA[@]}"} || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui || { echo "GUI が立たない"; exit 1; }
tk autorename off >/dev/null 2>&1 || true

echo "--- ① 1 万ファイルを探す"
SECS="$(timed 120 "$TAKO_BIN" search zebra_needle --path "$TREE" --json --limit 1)"
cp "$TMP/timed.out" "$TMP/one.json"
TOTAL="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["total"])' "$TMP/one.json" 2>/dev/null)"
SEARCHED="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["files_searched"], d["skipped"]["binary"], d["stopped"])' "$TMP/one.json" 2>/dev/null)"
echo "    所要 ${SECS}s・件数 ${TOTAL}・探したファイル / バイナリ / 打ち切り = $SEARCHED"
check "$([ "$TOTAL" = "$EXPECTED" ] && echo 1)" "件数が生成した答え（${EXPECTED}）と一致（.gitignore の配下 500 行を数えない）"
check "$(grep -q ' 1 None$' <<<"$SEARCHED" && echo 1)" "バイナリ 1 つを探さず、最後まで探した"

echo "--- ② 検索のあいだ UI が止まらない（別の要求の応答時間）"
# 一致しない正規表現で全ファイルを読み切る。1 万ファイルの走査は素の検索語なら 0.1 秒で終わって
# 「走査のあいだ」を測れないので、前置きが効かず NFA で照合する形（Unicode の文字クラスの反復）にして
# 走査を数秒へ延ばす（debug ビルドの実測 3.7 秒。製品の経路は同じ = 試験用の口は足さない）。
# --show で右パネルにも出す
SLOW_Q='(?i)((\w|\s){10,40}){3}qzq'
"$TAKO_BIN" search "$SLOW_Q" --regex --path "$TREE" --show --json --limit 1 > "$TMP/slow.json" 2>&1 &
SLOW_PID=$!
MAX=0
N=0
SAMPLED=""
while kill -0 "$SLOW_PID" 2>/dev/null; do
  T="$(timed 10 "$TAKO_BIN" scrollback)"
  [ "$T" = "timeout" ] && T=10
  N=$((N + 1))
  MAX="$(perl -e "printf '%.3f', ($T > $MAX ? $T : $MAX)")"
  # ③ 1 回だけメインスレッドのスタックを採る（走査のあいだ）
  if [ -z "$SAMPLED" ] && kill -0 "$SLOW_PID" 2>/dev/null; then
    sample "$APP_PID" 1 -file "$TMP/sample.txt" >/dev/null 2>&1 && SAMPLED=1
  fi
  [ "$N" -ge 200 ] && break
done
wait "$SLOW_PID"
SLOW_MS="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["elapsed_ms"])' "$TMP/slow.json" 2>/dev/null)"
echo "    走査 ${SLOW_MS}ms のあいだに別の要求を $N 回・最大 ${MAX}s（上限 ${UI_REPLY_LIMIT}s）"
check "$([ "$N" -ge 3 ] && echo 1)" "走査のあいだに別の要求を 3 回以上打てた（測れている）"
check "$(perl -e "print 1 if $MAX <= $UI_REPLY_LIMIT")" "検索のあいだも別の要求が ${UI_REPLY_LIMIT}s 以内に返る（UI スレッドが止まらない）"

echo "--- ③ メインスレッドで走査しない（sample）"
if [ -n "$SAMPLED" ]; then
  awk '/Thread_.*com.apple.main-thread/{m=1; next} /^    [0-9]+ Thread_/{m=0} m' "$TMP/sample.txt" > "$TMP/main-thread.txt"
  MAIN_SCAN="$(grep -cE 'workspace_search::search|ignore::walk' "$TMP/main-thread.txt")"
  ALL_SCAN="$(grep -cE 'workspace_search::search|ignore::walk' "$TMP/sample.txt")"
  echo "    走査の関数の出現: メインスレッド $MAIN_SCAN / プロセス全体 $ALL_SCAN"
  check "$([ "$MAIN_SCAN" -eq 0 ] && echo 1)" "メインスレッドのスタックに走査の関数が居ない"
  check "$([ "$ALL_SCAN" -gt 0 ] && echo 1)" "走査は別のスレッドで走っていた（sample が走査中を採れている）"
else
  echo "    未実測: sample を採れなかった"
fi
# perf.log: UI スレッドの専有のうち横断検索の区間（32ms を超えたものだけが記録される）
grep -E 'WorkspaceSearch|finish_offload' "$TMP/perf.log" 2>/dev/null | sed 's/^/    perf: /' | head -5
# 準備（offload_prepare:WorkspaceSearch）も、同期で走ったときの区間（dispatch:WorkspaceSearch）も見る
check "$(grep -qE 'WorkspaceSearch' "$TMP/perf.log" 2>/dev/null || echo 1)" \
  "UI スレッドの専有に横断検索の区間が 32ms を超えて現れない（perf.log に記録が無い）"

echo "--- ④ エッジ"
tk search no_such_needle_1694 --path "$TREE" > "$TMP/zero.out" 2>&1
check "$(grep -q '見つからなかった' "$TMP/zero.out" && echo 1)" "0 件: 見つからなかった（$(tail -1 "$TMP/zero.out")）"
tk search lorem --path "$TREE" --limit 3 > "$TMP/limit.out" 2>&1
check "$(grep -q '上限 2000 件で打ち切り' "$TMP/limit.out" && echo 1)" "巨大な結果: 上限で打ち切りと表示（$(tail -1 "$TMP/limit.out")）"
if tk search '(unclosed' --regex --path "$TREE" > "$TMP/regex.out" 2>&1; then RC=0; else RC=$?; fi
check "$([ "$RC" -ne 0 ] && grep -q '正規表現として読めない' "$TMP/regex.out" && echo 1)" "壊れた正規表現: 理由つきで失敗（rc=${RC}）"
if tk search x --path "$TMP/nope" > "$TMP/nope.out" 2>&1; then RC=0; else RC=$?; fi
check "$([ "$RC" -ne 0 ] && grep -q 'ディレクトリが存在しない' "$TMP/nope.out" && echo 1)" "存在しない範囲: 理由つきで失敗"
# 読めないファイル + シンボリックリンクのループ（親を指す）
chmod 000 "$TREE/d000/f000.rs"
ln -s .. "$TREE/d001/loop"
tk search zebra_needle --path "$TREE" --json --limit 1 > "$TMP/edge.json" 2>&1
EDGE="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["total"], d["skipped"]["unreadable"], d["stopped"])' "$TMP/edge.json" 2>/dev/null)"
echo "    読めない 1 + ループ: 件数 / 読めない / 打ち切り = $EDGE"
check "$([ "$EDGE" = "$((EXPECTED - 1)) 1 None" ] && echo 1)" "読めないファイルは数えて飛ばし、ループは辿らず重複しない"
chmod 644 "$TREE/d000/f000.rs"
rm -f "${TREE:?}/d001/loop"
# 検索中の再検索: 前の出す検索は取り消され（shown=false / stopped=cancelled）、後ろが右パネルに残る
"$TAKO_BIN" search "$SLOW_Q" --regex --path "$TREE" --show --json --limit 1 > "$TMP/first.json" 2>&1 &
FIRST=$!
sleep 0.2
tk search zebra_needle --path "$TREE" --show --json --limit 1 > "$TMP/second.json" 2>&1
wait "$FIRST"
FIRST_STATE="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["shown"], d["stopped"])' "$TMP/first.json" 2>/dev/null)"
RESULTS="$(tk search --results --json --limit 1 | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["query"], d["total"])')"
echo "    前の検索 = $FIRST_STATE / 右パネル = $RESULTS"
check "$([ "$FIRST_STATE" = "False cancelled" ] && echo 1)" "検索中の再検索: 前の検索は取り消されて置かれない"
check "$([ "$RESULTS" = "zebra_needle $EXPECTED" ] && echo 1)" "右パネルには後の検索の結果が残る"
# 検索中の --clear: 走っている検索も取り消し、保持 0
"$TAKO_BIN" search "$SLOW_Q" --regex --path "$TREE" --show --json --limit 1 > "$TMP/cleared.json" 2>&1 &
CL=$!
sleep 0.2
tk search --clear > /dev/null 2>&1
wait "$CL"
CL_STATE="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["shown"], d["stopped"])' "$TMP/cleared.json" 2>/dev/null)"
RETAINED="$(tk search --results --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["status"], d["retained"])')"
echo "    --clear の後: 走っていた検索 = $CL_STATE / 右パネル = $RETAINED"
check "$([ "$CL_STATE" = "False cancelled" ] && [ "$RETAINED" = "empty 0" ] && echo 1)" "検索中の --clear: 取り消して保持 0"
stop_isolated_gui "$APP_PID"
ISOLATED_GUI_PID=""

if [ -n "$AB" ]; then
  echo
  echo "PASS=${PASS} FAIL=${FAIL}（A/B の腕。visual 段は省く）"
  [ "$FAIL" -eq 0 ]
  exit $?
fi

echo "--- ⑤ visual-test workspace-search-1694（⇧⌘F → Enter → 行の ⌘クリック）"
DUMP_DIR="${WS1694_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP_DIR"
launch_isolated_gui "$TMP/visual.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=workspace-search-1694 \
  TAKO_VISUAL_DUMP_DIR="$DUMP_DIR" ZDOTDIR="$TMP/zdot" || exit $?
APP_PID="$ISOLATED_GUI_PID"
for _ in $(seq 1 1200); do
  kill -0 "$APP_PID" 2>/dev/null || break
  sleep 0.1
done
stop_isolated_gui "$APP_PID"
ISOLATED_GUI_PID=""
grep -E "TAKO_VISUAL_PIXEL: workspace-search-1694|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED" \
  "$TMP/visual.log" | sed "s#$TMP#<tmp>#g; s/^/    /"
check "$(grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual.log" && echo 1)" \
  "visual: ⇧⌘F → 打鍵 → Enter で 2 行・行の ⌘クリックで新しいペインが 1 枚増え 90 行目が見える・× で保持 0"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
