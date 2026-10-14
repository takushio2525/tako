#!/bin/bash
# tako:run bash scripts/test-queue-1992.sh
#
# 重い処理の順番待ち `tako queue <コマンド>`（#1994。エピック #1992 の S2）の実経路テスト。
#
# 隔離の調停役を GUI 無しで立て（`tako queue serve`。TAKO_QUEUE_SOCKET を明示したときだけ立つ）、
# 眠るだけの偽の重い処理を `tako queue` で並べて確かめる。**機の値は注入で与え、負荷は生成しない**
# （TAKO_QUEUE_INJECT の JSON = 全体・空き・メモリ圧・swap・コア数・tako の外の件数・停滞の窓）。
#
#   ① 順番どおりに割り当てる（同時 1 件で A → B → C・重ならない）
#   ② 同時数と取り分（同時 2 件で 3 件目は待つ・CARGO_BUILD_JOBS = 18 ÷ 実行中の数）
#   ③ 終了コードの素通し（7 / シグナルで終わった子は 143）
#   ④ 入れ子が固まらない（枠の中の tako queue は待たずに走る）
#   ⑤ 調停役を落として立て直しても順番が保たれる（実行中は子の pid、待ちは元の受付時刻で組み直す）
#   ⑥ 調停役が居なければすぐ走る（受け口が無い / 本番へは行かない）
#   ⑦ 待ちの上限で 75
#   ⑧ 取り消し（待ち = 125）
#   ⑨ 取り消し（実行中 = 子とその子孫へ SIGTERM・wrapper は 143）
#   ⑩ 停滞（木の CPU が窓のあいだ 1% 未満）は枠を返して殺さない
#   ⑪ メモリ圧 critical は止める（0 件でも）・空きメモリの予約で待つ（注入した機の値）
#   ⑫ UI の代役が答えない（固まった UI）状態でも割り当ては進む
#   ⑬ 上限の読み書き（範囲外は弾く・GUI が居なくても設定ファイルへ書ける）
#   ⑭ scripts/lib/queue.sh の run_heavy（順番待ちへ通す・入れ子と tako の無い環境はそのまま）
#
# 落とすのは自分で起こした pid とソケットだけ（名前一致で殺さない）。
# 使い方: bash scripts/test-queue-1992.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh"

echo "tako-cli をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli) || exit 1
TAKO="$REPO_ROOT/target/debug/tako"
[ -x "$TAKO" ] || { echo "tako が無い: $TAKO"; exit 1; }

# **本番 GUI と外側の順番待ちを指す env を落とす**（外側の `tako queue bash …` の枠の中で走らせても、
# このテストの wrapper が入れ子と見なされて素通りしないように）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_1994_INJECT
unset CARGO_BUILD_JOBS RUST_TEST_THREADS

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 短い一時 dir（深いと受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk1992-XXXXXX)"
ARB_PID=""
BG_PIDS=""
cleanup() {
  [ -n "$ARB_PID" ] && kill "$ARB_PID" 2>/dev/null
  for p in $BG_PIDS; do kill "$p" 2>/dev/null; done
  # 偽の重い処理が残した sleep（自分の記録した pid だけ）
  if [ -f "$TMP/log" ]; then
    awk '$1 == "sleep" {print $3}' "$TMP/log" | while read -r p; do kill "$p" 2>/dev/null; done
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR"
export TAKO_TOKEN="tk1992-$$"
SOCK="$TMP/q.sock"
INJECT="$TMP/inject.json"
LOG="$TMP/log"
: > "$LOG"
export LOG

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

# 注入する機の値（36 GB・空き 30 GB・normal・swap 0・18 コア・外 0・立ち上げ直後の待ち 0）
inject() {
  local total=36864 avail=30000 pressure=normal swap=0 unmanaged=0 regather=0 stall=600
  local kv
  for kv in "$@"; do
    case "$kv" in
      avail=*) avail="${kv#avail=}" ;;
      pressure=*) pressure="${kv#pressure=}" ;;
      swap=*) swap="${kv#swap=}" ;;
      unmanaged=*) unmanaged="${kv#unmanaged=}" ;;
      regather=*) regather="${kv#regather=}" ;;
      stall=*) stall="${kv#stall=}" ;;
    esac
  done
  printf '{"total_mb":%s,"available_mb":%s,"pressure":"%s","swap_used_mb":%s,"cpus":18,"unmanaged":%s,"regather_secs":%s,"stall_secs":%s}\n' \
    "$total" "$avail" "$pressure" "$swap" "$unmanaged" "$regather" "$stall" > "$INJECT"
}

# 偽の重い処理: 開始と終了を記録して眠るだけ（sh = 区分 build。target dir は無い = 一時 dir の外に Cargo.toml が無い）
cat > "$TMP/fake.sh" <<'SH'
#!/bin/sh
# fake.sh <名前> <秒> [終了コード]
echo "start $1 jobs=${CARGO_BUILD_JOBS:-} threads=${RUST_TEST_THREADS:-} job=${TAKO_QUEUE_JOB:-}" >> "$LOG"
sleep "$2" &
SP=$!
echo "sleep $1 $SP" >> "$LOG"
wait "$SP"
echo "end $1" >> "$LOG"
exit "${3:-0}"
SH
chmod +x "$TMP/fake.sh"

start_arbiter() {
  TAKO_QUEUE_SOCKET="$SOCK" TAKO_QUEUE_INJECT="$INJECT" "$@" "$TAKO" queue serve 2>>"$TMP/arb.log" &
  ARB_PID=$!
  local i
  for i in $(seq 1 50); do
    if TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue --json 2>/dev/null | grep -q '"arbiter": true'; then
      return 0
    fi
    sleep 0.1
  done
  echo "  調停役が立たない（$TMP/arb.log）"
  return 1
}

stop_arbiter() {
  [ -n "$ARB_PID" ] && kill "$ARB_PID" 2>/dev/null
  [ -n "$ARB_PID" ] && wait "$ARB_PID" 2>/dev/null
  ARB_PID=""
}

q() { TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue "$@"; }

# バックグラウンドで並べる（$! = tako queue の pid = 受付番号）
enqueue_bg() {
  local name="$1" secs="$2" code="${3:-0}"
  TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh "$TMP/fake.sh" "$name" "$secs" "$code" 2>>"$TMP/wrapper-$name.err" &
  LAST_BG=$!
  BG_PIDS="$BG_PIDS $LAST_BG"
}

# 条件が真になるまで待つ（秒）
wait_until() {
  local limit="$1"
  shift
  local deadline=$(($(date +%s) + limit))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    "$@" && return 0
    sleep 0.2
  done
  "$@"
}

started() { grep -q "^start $1 " "$LOG"; }
ended() { grep -q "^end $1\$" "$LOG"; }
start_order() { awk '$1 == "start" {printf "%s", $2}' "$LOG"; }
# 記録から同時に走っていた数の最大
max_concurrency() {
  awk '$1 == "start" {n++; if (n > m) m = n} $1 == "end" {n--} END {print m + 0}' "$LOG"
}
reset_log() { : > "$LOG"; }

status_json() { q --json 2>/dev/null; }
json_get() { python3 -c "import json,sys; v=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

# 偽の重い処理は一時 dir から並べる（区分 build の処理は cwd の cargo workspace の target dir を持つので、
# worktree の中から並べると全件が「同じ target dir = 1 件ずつ」になって同時数を確かめられない）
cd "$TMP" || exit 1

inject
start_arbiter || exit 1
q limits --max-jobs 1 >/dev/null || { fail "上限を 1 にできない"; }

echo "== ① 順番どおり（同時 1 件）=="
reset_log
enqueue_bg A 2; PA=$LAST_BG
sleep 0.4
enqueue_bg B 1; PB=$LAST_BG
sleep 0.4
enqueue_bg C 1; PC=$LAST_BG
wait "$PA" "$PB" "$PC"
order="$(start_order)"
[ "$order" = "ABC" ] && pass "受付の順に A → B → C" || fail "開始の順が ${order}（期待 ABC）"
[ "$(max_concurrency)" = "1" ] && pass "同時に走ったのは 1 件まで" || fail "同時 $(max_concurrency) 件"

echo "== ② 同時数と取り分（同時 2 件・18 コア）=="
q limits --max-jobs 2 >/dev/null
reset_log
enqueue_bg A 3; PA=$LAST_BG
sleep 0.4
enqueue_bg B 3; PB=$LAST_BG
sleep 0.4
enqueue_bg C 1; PC=$LAST_BG
wait_until 5 started B
sleep 0.6
if ! started C; then pass "3 件目は待つ（同時 2 件）"; else fail "3 件目が待たずに走った"; fi
reason="$(status_json | json_get 'v["queue"]["blocked"]["reason"]')"
[ "$reason" = "max_jobs" ] && pass "待つ理由 = max_jobs" || fail "待つ理由 = $reason"
wait "$PA" "$PB" "$PC"
grep -q "^start A jobs=18 threads=18 job=$PA" "$LOG" && pass "1 件目の取り分 = 18（18 ÷ 1）・受付番号を TAKO_QUEUE_JOB で渡す" \
  || fail "1 件目の取り分: $(grep '^start A' "$LOG")"
grep -q "^start B jobs=9 threads=9 " "$LOG" && pass "2 件目の取り分 = 9（18 ÷ 2）" || fail "2 件目の取り分: $(grep '^start B' "$LOG")"
[ "$(max_concurrency)" = "2" ] && pass "同時に走ったのは 2 件まで" || fail "同時 $(max_concurrency) 件"
CARGO_BUILD_JOBS=3 TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh -c 'echo "user $CARGO_BUILD_JOBS $RUST_TEST_THREADS" >> "$LOG"'
grep -q "^user 3 18\$" "$LOG" && pass "利用者が決めた CARGO_BUILD_JOBS は上書きしない" || fail "利用者の env: $(grep '^user' "$LOG")"
q limits --max-jobs 1 >/dev/null

echo "== ③ 終了コードの素通し =="
q sh -c 'exit 7'; rc=$?
[ "$rc" = 7 ] && pass "終了コード 7 をそのまま返す" || fail "終了コード ${rc}（期待 7）"
q sh -c 'kill -TERM $$'; rc=$?
[ "$rc" = 143 ] && pass "シグナルで終わった子は 128 + 15 = 143" || fail "終了コード ${rc}（期待 143）"
q /nonexistent/cmd-1994 2>/dev/null; rc=$?
[ "$rc" = 127 ] && pass "起動できないコマンドは 127" || fail "終了コード ${rc}（期待 127）"
q --json >/dev/null 2>&1
n_running="$(status_json | json_get 'len(v["queue"]["running"])')"
[ "$n_running" = 0 ] && pass "起動できなかった件も枠を返している" || fail "実行中が $n_running 件残った"

echo "== ④ 入れ子が固まらない =="
t0=$(now_ms)
TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh -c "\"$TAKO\" queue sh -c 'exit 3'"; rc=$?
t1=$(now_ms)
[ "$rc" = 3 ] && [ $((t1 - t0)) -lt 5000 ] && pass "枠の中の tako queue は待たずに走る（$((t1 - t0)) ms・終了コード 3）" \
  || fail "入れ子: 終了コード ${rc}・$((t1 - t0)) ms"

echo "== ⑤ 調停役を落として立て直しても順番が保たれる =="
reset_log
enqueue_bg A 10; PA=$LAST_BG
wait_until 5 started A
enqueue_bg B 1; PB=$LAST_BG
sleep 0.4
enqueue_bg C 1; PC=$LAST_BG
sleep 1
stop_arbiter
sleep 2
if started B || started C; then fail "調停役が居ない間に待ちが走った"; else pass "調停役が居ない間も待ちは待つ（60 秒の猶予）"; fi
inject regather=4
start_arbiter || exit 1
sleep 2.8
S="$(status_json)"
echo "$S" | python3 -c '
import json, sys
v = json.load(sys.stdin)["queue"]
run = [r["id"] for r in v["running"]]
wait = [w["id"] for w in v["waiting"]]
print("  組み直し: 実行", run, "待ち", wait)
sys.exit(0 if run == [int(sys.argv[1])] and wait == [int(sys.argv[2]), int(sys.argv[3])] else 1)
' "$PA" "$PB" "$PC" && pass "立て直した調停役が実行中（子の pid）と待ち（元の受付時刻の順）を組み直す" \
  || fail "組み直し: $(echo "$S" | json_get 'v["queue"]')"
wait "$PA" "$PB" "$PC"
order="$(start_order)"
[ "$order" = "ABC" ] && pass "落とす前の順のまま A → B → C" || fail "開始の順が ${order}（期待 ABC）"
inject

echo "== ⑥ 調停役が居なければすぐ走る =="
t0=$(now_ms)
TAKO_QUEUE_SOCKET="$TMP/none.sock" "$TAKO" queue sh -c 'exit 4' 2>"$TMP/none.err"; rc=$?
t1=$(now_ms)
[ "$rc" = 4 ] && [ $((t1 - t0)) -lt 3000 ] && grep -q "tako が起動していない" "$TMP/none.err" \
  && pass "受け口が無ければ待たずに走る（$((t1 - t0)) ms・1 行だけ知らせる）" || fail "受け口なし: rc=$rc $((t1 - t0)) ms $(cat "$TMP/none.err")"
# TAKO_QUEUE_SOCKET を付けない = いつもの解決。隔離の env（発見ファイルも一時 dir）なので本番へは行かない
t0=$(now_ms)
"$TAKO" queue sh -c 'exit 5' 2>"$TMP/none2.err"; rc=$?
t1=$(now_ms)
[ "$rc" = 5 ] && [ $((t1 - t0)) -lt 3000 ] && pass "tako が居ない（いつもの解決）でも待たずに走る（$((t1 - t0)) ms）" \
  || fail "いつもの解決: rc=$rc $((t1 - t0)) ms $(cat "$TMP/none2.err")"

echo "== ⑦ 待ちの上限で 75 =="
reset_log
enqueue_bg A 5; PA=$LAST_BG
wait_until 5 started A
t0=$(now_ms)
q --max-wait 2s sh -c 'echo never >> "$LOG"' 2>"$TMP/maxwait.err"; rc=$?
t1=$(now_ms)
[ "$rc" = 75 ] && pass "上限 2 秒で 75（$((t1 - t0)) ms）" || fail "上限: rc=$rc"
grep -q "諦める" "$TMP/maxwait.err" && pass "「空いたらもう一度」の 1 行" || fail "上限の案内: $(cat "$TMP/maxwait.err")"
grep -q "^never" "$LOG" && fail "諦めた処理が走った" || pass "諦めた処理は走らない"
n_wait="$(status_json | json_get 'len(v["queue"]["waiting"])')"
[ "$n_wait" = 0 ] && pass "諦めた件は待ちから外れる" || fail "待ちに $n_wait 件残った"

echo "== ⑧ 取り消し（待ち）=="
enqueue_bg B 1; PB=$LAST_BG
wait_until 5 sh -c "TAKO_QUEUE_SOCKET='$SOCK' '$TAKO' queue --json | grep -q '\"id\": $PB'"
q cancel "$PB" >/dev/null && pass "待ちの受付番号を取り消せる" || fail "取り消し（待ち）が断られた"
wait "$PB"; rc=$?
[ "$rc" = 125 ] && pass "待っていた wrapper は 125 で終わる" || fail "取り消した待ち: rc=$rc"
started B && fail "取り消した処理が走った" || pass "取り消した処理は走らない"
q cancel 1 >/dev/null 2>"$TMP/cancel.err"; rc=$?
[ "$rc" != 0 ] && grep -q "居ない" "$TMP/cancel.err" && pass "居ない受付番号は理由つきで断る" || fail "居ない番号: rc=$rc $(cat "$TMP/cancel.err")"
wait "$PA"

echo "== ⑨ 取り消し（実行中）=="
reset_log
enqueue_bg A 60; PA=$LAST_BG
wait_until 5 grep -q "^sleep A " "$LOG"
SP="$(awk '$1 == "sleep" && $2 == "A" {print $3}' "$LOG")"
t0=$(now_ms)
q cancel "$PA" >/dev/null
wait "$PA"; rc=$?
t1=$(now_ms)
[ "$rc" = 143 ] && pass "実行中の取り消しで子は SIGTERM・wrapper は 143（$((t1 - t0)) ms）" || fail "実行中の取り消し: rc=$rc"
sleep 0.3
if kill -0 "$SP" 2>/dev/null; then fail "子孫（sleep ${SP}）が残った"; else pass "子孫（sleep）まで止まった"; fi

echo "== ⑩ 停滞は枠を返して殺さない（窓 3 秒に注入）=="
inject stall=3
reset_log
enqueue_bg A 40; PA=$LAST_BG
wait_until 5 started A
enqueue_bg B 1; PB=$LAST_BG
if wait_until 20 started B; then pass "停滞した A（CPU 0%）の枠が返り、B が入る"; else fail "B が入らない（停滞が効かない）"; fi
kill -0 "$PA" 2>/dev/null && pass "停滞した A は殺さない（走り続ける）" || fail "停滞した A が死んだ"
stalled="$(status_json | json_get '[r["stalled"] for r in v["queue"]["running"] if r["id"] == '"$PA"']')"
[ "$stalled" = "[True]" ] && pass "status に停滞の印" || fail "停滞の印: $stalled"
wait "$PB"
q cancel "$PA" >/dev/null 2>&1
wait "$PA" 2>/dev/null
inject

echo "== ⑪ メモリ圧と空きメモリ（注入した機の値）=="
inject pressure=critical
reset_log
enqueue_bg A 1; PA=$LAST_BG
sleep 2
if started A; then fail "critical なのに走った"; else pass "メモリ圧 critical は 0 件でも止める"; fi
reason="$(status_json | json_get 'v["queue"]["blocked"]["reason"]')"
[ "$reason" = "pressure_critical" ] && pass "待つ理由 = pressure_critical" || fail "待つ理由 = $reason"
inject
wait_until 5 started A && pass "normal へ戻れば走る" || fail "normal へ戻しても走らない"
wait "$PA"
q limits --max-jobs 3 >/dev/null
inject avail=6000
reset_log
enqueue_bg A 3; PA=$LAST_BG
wait_until 5 started A
enqueue_bg B 1; PB=$LAST_BG
sleep 1.5
reason="$(status_json | json_get 'v["queue"]["blocked"]["reason"]')"
if ! started B && [ "$reason" = "memory" ]; then pass "空き 5.9 GB − 見込み 3 GB × 2 < 予約 3.6 GB で待つ（理由 = memory）"; else fail "空きメモリ: started B? reason=$reason"; fi
wait "$PA" "$PB"
started B && pass "1 件目が終われば入る" || fail "B が走らなかった"
q limits --max-jobs 1 >/dev/null
inject

echo "== ⑫ UI の代役が答えない状態でも割り当ては進む =="
stop_arbiter
start_arbiter env TAKO_1994_INJECT=ui-stall || exit 1
( TAKO_SOCKET="$SOCK" "$TAKO" list >/dev/null 2>&1; echo "list-returned" >> "$LOG" ) &
LIST_PID=$!
BG_PIDS="$BG_PIDS $LIST_PID"
reset_log
enqueue_bg A 1; PA=$LAST_BG
enqueue_bg B 1; PB=$LAST_BG
wait "$PA" "$PB"
if grep -q "^list-returned" "$LOG"; then fail "UI の代役が答えている（注入が効いていない）"; else pass "UI の代役は答えない（tako list が返らない）"; fi
[ "$(start_order)" = "AB" ] && pass "それでも A → B と割り当てが進む" || fail "割り当て: $(start_order)"
kill "$LIST_PID" 2>/dev/null
stop_arbiter
start_arbiter || exit 1

echo "== ⑬ 上限の読み書き =="
q limits --max-jobs 0 >/dev/null 2>"$TMP/lim.err"; rc=$?
[ "$rc" != 0 ] && grep -q "max_jobs は 1〜64" "$TMP/lim.err" && pass "範囲外は理由つきで弾く" || fail "範囲外: rc=$rc $(cat "$TMP/lim.err")"
q limits --max-jobs 2 --reserve-gb 5 --json > "$TMP/lim.json"
python3 - "$TMP/lim.json" "$TAKO_DATA_DIR/settings.json" <<'PY' && pass "上限を書けて settings.json の resources 節に残る" || fail "上限の書き込み"
import json, sys
v = json.load(open(sys.argv[1])); s = json.load(open(sys.argv[2]))
assert v["max_jobs"] == 2 and v["reserve_gb"] == 5.0, v
assert s["resources"] == {"max_jobs": 2, "reserve_gb": 5.0}, s
PY
TAKO_QUEUE_SOCKET="$TMP/none.sock" "$TAKO" queue limits --reset --json > "$TMP/lim2.json"
python3 - "$TMP/lim2.json" "$TAKO_DATA_DIR/settings.json" <<'PY' && pass "GUI が居なくても上限を既定へ戻せる（設定ファイルへ直に書く）" || fail "GUI 不在の上限"
import json, sys
v = json.load(open(sys.argv[1])); s = json.load(open(sys.argv[2]))
assert v["configured"] == {}, v
assert "resources" not in s, s
PY
sleep 2.2
configured="$(status_json | json_get 'v["queue"]["limits"]["configured"]')"
[ "$configured" = "{}" ] && pass "調停役は設定ファイルの書き換えを読み直す（mtime）" || fail "読み直し: $configured"
q limits --max-jobs 1 >/dev/null

echo "== ⑭ run_heavy =="
mkdir -p "$TMP/pathbin"
ln -sf "$TAKO" "$TMP/pathbin/tako"
out="$(PATH="$TMP/pathbin:$PATH" TAKO_QUEUE_SOCKET="$SOCK" bash -c '. "'"$REPO_ROOT"'/scripts/lib/queue.sh"; run_heavy sh -c "echo job=\$TAKO_QUEUE_JOB"')"
case "$out" in job=[0-9]*) pass "run_heavy は順番待ちへ通す（${out}）" ;; *) fail "run_heavy: $out" ;; esac
out="$(PATH="$TMP/pathbin:$PATH" TAKO_QUEUE_SOCKET="$SOCK" TAKO_QUEUE_JOB=77 bash -c '. "'"$REPO_ROOT"'/scripts/lib/queue.sh"; run_heavy sh -c "echo job=\$TAKO_QUEUE_JOB"')"
[ "$out" = "job=77" ] && pass "入れ子（TAKO_QUEUE_JOB がある）はそのまま走らせる" || fail "入れ子: $out"
out="$(PATH="/usr/bin:/bin" bash -c '. "'"$REPO_ROOT"'/scripts/lib/queue.sh"; run_heavy sh -c "exit 6"; echo "rc=$?"')"
[ "$out" = "rc=6" ] && pass "tako の無い環境（CI）はそのまま走らせ終了コードを返す" || fail "tako なし: $out"
PATH="$TMP/pathbin:$PATH" TAKO_QUEUE_SOCKET="$SOCK" bash -c '. "'"$REPO_ROOT"'/scripts/lib/queue.sh"; run_heavy --priority low sh -c "exit 0"' \
  && pass "run_heavy --priority low" || fail "run_heavy --priority low"

echo "== 人が読む表示 =="
reset_log
: > "$TMP/wrapper-B.err"
enqueue_bg A 2; PA=$LAST_BG
sleep 0.3
enqueue_bg B 1; PB=$LAST_BG
sleep 1.2
q | sed 's/^/    /'
wait "$PA" "$PB"
echo "  wrapper の待ちの表示（非 TTY）:"
sed 's/^/    /' "$TMP/wrapper-B.err"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
