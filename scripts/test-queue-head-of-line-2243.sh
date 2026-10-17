#!/bin/bash
# tako:run bash scripts/test-queue-head-of-line-2243.sh
#
# `tako queue` の先頭詰まり（#2243）の実経路テスト。
#
# 先頭が「同じ target dir を使う処理が走っている」で待つ間、別の target dir の後ろの処理まで空き枠の前で
# 待たされていた。target dir だけで入れない件は後ろに追い越させる（条件と上限は
# `crates/tako-core/src/queue.rs` のモジュールの説明「追い越し」）。
#
# 隔離の調停役を GUI 無しで立て（`tako queue serve`。TAKO_QUEUE_SOCKET を明示したときだけ立つ）、
# 眠るだけの偽の重い処理を `tako queue` で並べる。target dir は env の CARGO_TARGET_DIR で与える
# （偽の処理は `sh` = 区分 build なので wrapper が target dir を推定する）。**機の値は注入で与え、
# 負荷は生成しない**（TAKO_QUEUE_INJECT）。
#
#   ① A/B: 同じ target dir の処理が走っている間、別の target dir の後ろの処理は
#      修正後 = 空き枠ですぐ走る / 修正前（調停役の env TAKO_2243_LEGACY=1）= 持ち主が終わるまで待つ
#   ② 同じ target dir が 3 本並んでも直列（受付の順・重ならない）で、別の dir は先に走る
#   ③ 枠が全部埋まっているとき: 追い越さず、待ちの表示は自分の理由（同時数の上限）を言う
#   ④ メモリ圧 warn のとき: 追い越す側も待ち、理由はメモリ圧。normal へ戻れば走る
#   ⑤ 追い越しの上限（同時数 2 × 2 巡 = 4 件）: 5 件目は待ち、持ち主が終われば先頭が先に入る
#
# 落とすのは自分で起こした pid とソケットだけ（名前一致で殺さない）。
# 使い方: bash scripts/test-queue-head-of-line-2243.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh"

echo "tako-cli をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli) || exit 1
TAKO="$REPO_ROOT/target/debug/tako"
[ -x "$TAKO" ] || { echo "tako が無い: $TAKO"; exit 1; }

# 本番 GUI と外側の順番待ちを指す env を落とす（外側の `tako queue bash …` の枠の中で走らせても、
# このテストの wrapper が入れ子と見なされて素通りしないように）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_1994_INJECT
unset TAKO_2243_LEGACY CARGO_TARGET_DIR CARGO_BUILD_JOBS RUST_TEST_THREADS

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 短い一時 dir（深いと受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk2243-XXXXXX)"
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
export TAKO_TOKEN="tk2243-$$"
SOCK="$TMP/q.sock"
INJECT="$TMP/inject.json"
LOG="$TMP/log"
: > "$LOG"
export LOG

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

# 注入する機の値（36 GB・空き 30 GB・18 コア・外 0・立ち上げ直後の待ち 0）
inject() {
  printf '{"total_mb":36864,"available_mb":30000,"pressure":"%s","swap_used_mb":0,"cpus":18,"unmanaged":0,"regather_secs":0}\n' \
    "${1:-normal}" > "$INJECT"
}

# 偽の重い処理: 開始と終了を時刻（ミリ秒）つきで記録して眠るだけ
cat > "$TMP/fake.sh" <<'SH'
#!/bin/sh
# fake.sh <名前> <秒>
ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
echo "start $1 $(ms)" >> "$LOG"
sleep "$2" &
SP=$!
echo "sleep $1 $SP" >> "$LOG"
wait "$SP"
echo "end $1 $(ms)" >> "$LOG"
SH
chmod +x "$TMP/fake.sh"

start_arbiter() {
  TAKO_QUEUE_SOCKET="$SOCK" TAKO_QUEUE_INJECT="$INJECT" "$@" "$TAKO" queue serve 2>>"$TMP/arb.log" &
  ARB_PID=$!
  local i st
  for i in $(seq 1 50); do
    if st="$(TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue --json 2>/dev/null)" && grep -q '"arbiter": true' <<<"$st"; then
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

# バックグラウンドで並べる（$LAST_BG = tako queue の pid = 受付番号）。dir は target dir の名前
enqueue_bg() {
  local name="$1" secs="$2" dir="$3"
  CARGO_TARGET_DIR="$TMP/t-$dir" TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh "$TMP/fake.sh" "$name" "$secs" \
    2>>"$TMP/wrapper-$name.err" &
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
ended() { grep -q "^end $1 " "$LOG"; }
t_of() { awk -v k="$1" -v n="$2" '$1 == k && $2 == n {print $3}' "$LOG"; }
start_order() { awk '$1 == "start" {printf "%s ", $2}' "$LOG" | sed 's/ $//'; }
reset_log() { : > "$LOG"; }

status_json() { q --json 2>/dev/null; }
json_get() { python3 -c "import json,sys; v=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
# 待ちの 1 件の欄（受付番号で引く）
waiting_field() {
  status_json | python3 -c '
import json, sys
v = json.load(sys.stdin)["queue"]
for w in v["waiting"]:
    if w["id"] == int(sys.argv[1]):
        print(w.get(sys.argv[2]))
        break
else:
    print("absent")
' "$1" "$2"
}

# 偽の重い処理は一時 dir から並べる（cwd の Cargo.toml を拾わない。target dir は CARGO_TARGET_DIR だけで決まる）
cd "$TMP" || exit 1
inject

# ① A/B（同時 3 件・実行 1/3）
ab_run() {
  local mode="$1"
  reset_log
  : > "$TMP/wrapper-B-$mode.err"
  enqueue_bg "R-$mode" 6 X
  local PR=$LAST_BG
  wait_until 5 started "R-$mode"
  enqueue_bg "A-$mode" 1 X
  local PA=$LAST_BG
  sleep 0.5
  local t0
  t0=$(now_ms)
  enqueue_bg "B-$mode" 1 Y
  local PB=$LAST_BG
  wait_until 12 started "B-$mode"
  local tb
  tb=$(t_of start "B-$mode")
  AB_DELAY=$((tb - t0))
  AB_R_END_BEFORE_B=no
  ended "R-$mode" && [ "$(t_of end "R-$mode")" -le "$tb" ] && AB_R_END_BEFORE_B=yes
  wait "$PR" "$PA" "$PB"
  AB_A_AFTER_R=no
  [ "$(t_of start "A-$mode")" -ge "$(t_of end "R-$mode")" ] && AB_A_AFTER_R=yes
}

echo "== ① A/B: 同じ target dir が走っている間の別の target dir の後ろ（同時 3 件・実行 1/3）=="
start_arbiter env TAKO_2243_LEGACY=1 || exit 1
q limits --max-jobs 3 >/dev/null
ab_run legacy
LEGACY_DELAY=$AB_DELAY
if [ "$AB_R_END_BEFORE_B" = yes ] && [ "$LEGACY_DELAY" -ge 3000 ]; then
  pass "修正前（TAKO_2243_LEGACY=1）: 別の dir の B は持ち主 R が終わるまで待った（${LEGACY_DELAY} ms）"
else
  fail "修正前の再現: B の待ち ${LEGACY_DELAY} ms・R の終わりより後か=$AB_R_END_BEFORE_B"
fi
grep -q "前に 1 件（先頭: 同じ target dir を使う処理が走っている）" "$TMP/wrapper-B-legacy.err" \
  && pass "修正前の表示: $(head -1 "$TMP/wrapper-B-legacy.err" | sed 's/^tako queue: //')" \
  || fail "修正前の表示: $(cat "$TMP/wrapper-B-legacy.err")"
stop_arbiter

start_arbiter || exit 1
q limits --max-jobs 3 >/dev/null
ab_run new
NEW_DELAY=$AB_DELAY
if [ "$AB_R_END_BEFORE_B" = no ] && [ "$NEW_DELAY" -lt 2500 ]; then
  pass "修正後: 別の dir の B は R が走っている間に空き枠ですぐ走った（${NEW_DELAY} ms）"
else
  fail "修正後: B の待ち ${NEW_DELAY} ms・R の終わりより後か=$AB_R_END_BEFORE_B"
fi
[ ! -s "$TMP/wrapper-B-new.err" ] && pass "修正後: すぐ入ったので待ちの表示は出ない" \
  || fail "修正後の B の表示: $(cat "$TMP/wrapper-B-new.err")"
[ "$AB_A_AFTER_R" = yes ] && pass "同じ dir の A は R の終わりの後に走った（直列のまま）" \
  || fail "A が R と重なった: $(cat "$LOG")"

echo "== ② 同じ target dir が 3 本並ぶ（+ 別の dir 1 本）=="
reset_log
enqueue_bg R 4 X; PR=$LAST_BG
wait_until 5 started R
enqueue_bg A1 1 X; PA1=$LAST_BG
sleep 0.4
enqueue_bg A2 1 X; PA2=$LAST_BG
sleep 0.4
enqueue_bg B 1 Y; PB=$LAST_BG
sleep 1.5
reason1="$(waiting_field "$PA1" reason)"
reason2="$(waiting_field "$PA2" reason)"
[ "$reason1" = target_dir ] && [ "$reason2" = target_dir ] \
  && pass "待ちの表示: A1・A2 とも target_dir（$(waiting_field "$PA1" text)）" || fail "A1=$reason1 A2=$reason2"
over="$(waiting_field "$PA1" overtaken)"
[ "$over" = 1 ] && pass "A1 は B に 1 回追い越された（overtaken = 1）" || fail "A1 の overtaken = $over"
wait "$PR" "$PA1" "$PA2" "$PB"
order="$(start_order)"
[ "$order" = "R B A1 A2" ] && pass "開始の順 = R B A1 A2（B は先に走り、X は受付の順）" || fail "開始の順 = $order"
if [ "$(t_of start A1)" -ge "$(t_of end R)" ] && [ "$(t_of start A2)" -ge "$(t_of end A1)" ]; then
  pass "X の 3 本は重ならない（R → A1 → A2）"
else
  fail "X が重なった: $(cat "$LOG")"
fi

echo "== ③ 枠が全部埋まっているとき（同時 1 件）=="
q limits --max-jobs 1 >/dev/null
reset_log
: > "$TMP/wrapper-B3.err"
enqueue_bg R3 3 X; PR=$LAST_BG
wait_until 5 started R3
enqueue_bg A3 1 X; PA=$LAST_BG
sleep 0.4
enqueue_bg B3 1 Y; PB=$LAST_BG
sleep 1.5
started B3 && fail "枠が埋まっているのに B3 が走った" || pass "枠が埋まっていれば追い越さない"
r="$(waiting_field "$PB" reason)"
[ "$r" = max_jobs ] && pass "B3 の理由 = max_jobs（$(waiting_field "$PB" text)）" || fail "B3 の理由 = $r"
grep -q "— 同時数の上限（実行 1 / 上限 1）" "$TMP/wrapper-B3.err" \
  && pass "wrapper の表示も自分の理由: $(head -1 "$TMP/wrapper-B3.err" | sed 's/^tako queue: //')" \
  || fail "B3 の表示: $(cat "$TMP/wrapper-B3.err")"
wait "$PR" "$PA" "$PB"
order="$(start_order)"
[ "$order" = "R3 A3 B3" ] && pass "R3 が終われば先頭の A3 が先（開始の順 = ${order}）" || fail "開始の順 = $order"

echo "== ④ メモリ圧 warn（同時 3 件）=="
q limits --max-jobs 3 >/dev/null
reset_log
: > "$TMP/wrapper-B4.err"
enqueue_bg R4 8 X; PR=$LAST_BG
wait_until 5 started R4
inject warn
enqueue_bg A4 1 X; PA=$LAST_BG
sleep 0.4
enqueue_bg B4 1 Y; PB=$LAST_BG
sleep 1.5
r="$(waiting_field "$PB" reason)"
if ! started B4 && [ "$r" = pressure_warn ]; then pass "warn では追い越す側も待つ（理由 = pressure_warn）"; else fail "warn: started B4? 理由 = $r"; fi
grep -q "— メモリ圧 warn" "$TMP/wrapper-B4.err" && pass "wrapper の表示: $(head -1 "$TMP/wrapper-B4.err" | sed 's/^tako queue: //')" \
  || fail "B4 の表示: $(cat "$TMP/wrapper-B4.err")"
inject
wait_until 5 started B4
if started B4 && ! ended R4; then pass "normal へ戻れば R4 が走っている間に B4 が入る"; else fail "normal へ戻した後: $(cat "$LOG")"; fi
wait "$PR" "$PA" "$PB"

echo "== ⑤ 追い越しの上限（同時 2 件 → 上限 4 件）=="
q limits --max-jobs 2 >/dev/null
reset_log
enqueue_bg R5 120 X; PR=$LAST_BG
wait_until 5 started R5
enqueue_bg A5 1 X; PA=$LAST_BG
sleep 0.4
for i in 1 2 3 4; do
  enqueue_bg "O$i" 0.3 "o$i"; P=$LAST_BG
  wait_until 5 ended "O$i" || true
  wait "$P"
done
order="$(start_order)"
[ "$order" = "R5 O1 O2 O3 O4" ] && pass "上限までの 4 件は A5 を追い越して走った（${order}）" || fail "開始の順 = $order"
over="$(waiting_field "$PA" overtaken)"
[ "$over" = 4 ] && pass "A5 の overtaken = 4（上限）" || fail "A5 の overtaken = $over"
: > "$TMP/wrapper-O5.err"
enqueue_bg O5 1 o5; P5=$LAST_BG
sleep 2
started O5 && fail "上限を超えて O5 が追い越した" || pass "5 件目の O5 は枠が空いていても待つ"
r="$(waiting_field "$P5" reason)"
t="$(waiting_field "$P5" text)"
[ "$r" = ahead ] && case "$t" in *追い越しの上限*) true ;; *) false ;; esac \
  && pass "O5 の理由 = ahead（${t}）" || fail "O5 の理由 = $r / $t"
grep -q "前に 1 件（先頭: 同じ target dir を待つ処理が追い越しの上限（4 件）に達した" "$TMP/wrapper-O5.err" \
  && pass "wrapper の表示: $(head -1 "$TMP/wrapper-O5.err" | sed 's/^tako queue: //')" \
  || fail "O5 の表示: $(cat "$TMP/wrapper-O5.err")"
# 先頭が後ろより先に入ることを順で見るため、同時 1 件にしてから持ち主を終わらせる
q limits --max-jobs 1 >/dev/null
q cancel "$PR" >/dev/null
wait "$PR" "$PA" "$P5" 2>/dev/null
order="$(start_order)"
[ "$order" = "R5 O1 O2 O3 O4 A5 O5" ] && pass "持ち主が終われば先頭の A5 が O5 より先に入る（${order}）" || fail "開始の順 = $order"
q limits --max-jobs 3 >/dev/null

echo "== 人が読む表示（同時 1 件で R・A・B を並べた status）=="
q limits --max-jobs 1 >/dev/null
reset_log
enqueue_bg R 3 X; PR=$LAST_BG
wait_until 5 started R
enqueue_bg A 1 X; PA=$LAST_BG
sleep 0.4
enqueue_bg B 1 Y; PB=$LAST_BG
sleep 1.2
q | sed -n '/^順番待ち/,$p' | sed 's/^/    /'
wait "$PR" "$PA" "$PB"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
