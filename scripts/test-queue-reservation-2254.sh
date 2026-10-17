#!/bin/bash
# tako:run bash scripts/test-queue-reservation-2254.sh
#
# `tako queue` の実行中の予約（#2254）の実経路テスト。
#
# 走っている処理の「まだ使っていない見込み」が、ピークを過ぎた後も「見込み − 今の使用量」に戻り続け、
# 数百 MB しか使っていない `cargo test --workspace` が 7 GB 近くを押さえて後ろを 30 分止めていた。
# ピークを過ぎたら押さえを返し、見込みは過去の記録（`<data_dir>/resources/history.jsonl`）から引く
# （条件は `crates/tako-core/src/queue.rs` のモジュールの説明「見込みと『まだ使っていない見込み』」）。
#
# 隔離の調停役を GUI 無しで立て（`tako queue serve`。TAKO_QUEUE_SOCKET を明示したときだけ立つ）、
# 眠るだけの偽の `cargo` を `tako queue` で走らせる。**機の値と実行中の木の使用量・最大は注入で与え、
# 負荷は生成しない**（TAKO_QUEUE_INJECT の `jobs`）。36 GB の機の既定 = 同時 3・予約 3.6 GB。
#
#   ① A/B（調停役の env TAKO_2254_LEGACY=1 = 修正前）:
#      a. 小さい `cargo test --test small` を 1 回走らせて過去の記録を作る（最大 1.1 GB）
#      b. `cargo test --workspace`（見込み 7 GB）を走らせる（ピーク前: 最大 2 GB・今 0.3 GB）
#      c. ピーク前に後ろへ小さい処理 → 両方とも空きメモリ不足で待って諦める（75。押さえていた処理と量を出す）
#      d. ピークを過ぎて使用量が下がった（最大 7.3 GB・今 0.3 GB）後に小さい処理 →
#         修正前 = 「必要 13.7 GB」で待って諦める / 修正後 = すぐ入る
#   ② 本当に空きが無いとき（修正後）: 空きが実際に足りない・メモリ圧 warn は今まで通り待つ
#   ③ 諦めた件の表示: wrapper の stderr・`tako queue`（CLI）・MCP `tako_queue`（`tako mcp serve` 経由）に
#      押さえていた処理と量が出る
#
# 落とすのは自分で起こした pid とソケットだけ（名前一致で殺さない）。
# 使い方: bash scripts/test-queue-reservation-2254.sh

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
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_QUEUE_MAX_WAIT
unset TAKO_1994_INJECT TAKO_2243_LEGACY TAKO_2254_LEGACY CARGO_TARGET_DIR CARGO_BUILD_JOBS RUST_TEST_THREADS

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 短い一時 dir（深いと受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk2254-XXXXXX)"
ARB_PID=""
BG_PIDS=""
cleanup() {
  [ -n "$ARB_PID" ] && kill "$ARB_PID" 2>/dev/null
  for p in $BG_PIDS; do kill "$p" 2>/dev/null; done
  # 偽の cargo が残した sleep（自分の記録した pid だけ）
  if [ -f "$TMP/log" ]; then
    awk '$1 == "sleep" {print $3}' "$TMP/log" | while read -r p; do kill "$p" 2>/dev/null; done
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
mkdir -p "$TAKO_DISCOVERY_DIR" "$TMP/bin" "$TMP/work"
export TAKO_TOKEN="tk2254-$$"
SOCK="$TMP/q.sock"
INJECT="$TMP/inject.json"
LOG="$TMP/log"
: > "$LOG"
export LOG

# 偽の cargo: 開始と終了を記録して FAKE_SECS 秒眠るだけ（区分は argv の `cargo test` で test になる）
cat > "$TMP/bin/cargo" <<'SH'
#!/bin/sh
echo "start ${FAKE_NAME:-?}" >> "$LOG"
sleep "${FAKE_SECS:-1}" &
SP=$!
echo "sleep ${FAKE_NAME:-?} $SP" >> "$LOG"
wait "$SP"
echo "end ${FAKE_NAME:-?}" >> "$LOG"
SH
chmod +x "$TMP/bin/cargo"

# 注入: 機（36 GB・空き・メモリ圧）と実行中の木（`--workspace` = 大きい処理 / `--test small` = 小さい処理）
inject() {
  local avail="$1" pressure="$2" big_peak="$3"
  printf '{"total_mb":36864,"available_mb":%s,"pressure":"%s","swap_used_mb":0,"cpus":18,"unmanaged":0,"regather_secs":0,"jobs":[{"label":"--workspace","used_mb":300,"peak_mb":%s},{"label":"--test small","used_mb":900,"peak_mb":1100}]}\n' \
    "$avail" "$pressure" "$big_peak" > "$INJECT"
}

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
  rm -f "$SOCK"
}

q() { TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue "$@"; }

# 前景で走らせる: run_fg <名前> <秒> [tako queue のオプション…] -- <cargo の引数…>
# $RC = 終了コード・$DUR = かかった秒・stderr は $TMP/w-<名前>.err
run_fg() {
  local name="$1" secs="$2"
  shift 2
  local opts=""
  while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
    opts="$opts $1"
    shift
  done
  shift
  local t0
  t0=$(date +%s)
  # shellcheck disable=SC2086
  (cd "$TMP/work" && PATH="$TMP/bin:$PATH" FAKE_NAME="$name" FAKE_SECS="$secs" CARGO_TARGET_DIR="$TMP/t-$name" \
    TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue $opts cargo "$@" 2>"$TMP/w-$name.err")
  RC=$?
  DUR=$(($(date +%s) - t0))
}

# 背景で走らせる（$LAST_BG = tako queue の pid = 受付番号）
run_bg() {
  local name="$1" secs="$2"
  shift 2
  (cd "$TMP/work" && PATH="$TMP/bin:$PATH" FAKE_NAME="$name" FAKE_SECS="$secs" CARGO_TARGET_DIR="$TMP/t-$name" \
    TAKO_QUEUE_SOCKET="$SOCK" exec "$TAKO" queue cargo "$@" 2>"$TMP/w-$name.err") &
  LAST_BG=$!
  BG_PIDS="$BG_PIDS $LAST_BG"
}

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

status_json() { q --json 2>/dev/null; }

# 実行中の大きい処理（--workspace）の最大が注入の値になったか
big_peak_is() {
  status_json | python3 -c '
import json, sys
q = json.load(sys.stdin)["queue"]
want = int(sys.argv[1])
sys.exit(0 if any("--workspace" in r["label"] and r.get("peak_mb") == want and r.get("memory_mb") == 300
                  for r in q["running"]) else 1)
' "$1"
}

# MCP の tako_queue（status）を `tako mcp serve` 経由で叩いて queue 節を返す（GUI と同じ dispatch の 1 実装）
mcp_queue() {
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tako_queue","arguments":{}}}' \
    | TAKO_SOCKET="$SOCK" "$TAKO" mcp serve 2>/dev/null \
    | python3 -c '
import json, sys
resp = json.loads(sys.stdin.readline())
text = resp["result"]["content"][0]["text"]
print(json.dumps(json.loads(text)["queue"], ensure_ascii=False))
'
}

# 1 つの調停役で ① の a〜d を回す（$1 = legacy / fixed。残りは調停役の前置き）
ab_run() {
  local mode="$1"
  shift
  export TAKO_DATA_DIR="$TMP/data-$mode"
  mkdir -p "$TAKO_DATA_DIR"
  inject 8192 normal 2048
  start_arbiter "$@" || return 1

  # a. 過去の記録を作る（単独なので最低限の前進で入る）
  run_fg "seed-$mode" 3 -- test --test small
  local rows
  rows=$(python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
print(" ".join("%s:%s" % (r["name"], r["peak_mb"]) for r in rows))
' "$TAKO_DATA_DIR/resources/history.jsonl" 2>/dev/null)
  if [ "$RC" -eq 0 ] && [ "$rows" = "cargo test:1100" ]; then
    pass "[$mode] a. 過去の記録を 1 行足した（resources/history.jsonl = ${rows}）"
  else
    fail "[$mode] a. 記録: RC=$RC rows=$rows"
  fi

  # b. 大きい処理（ピーク前: 最大 2 GB・今 0.3 GB）
  run_bg "big-$mode" 90 test --workspace
  BIG=$LAST_BG
  if wait_until 10 big_peak_is 2048; then
    pass "[$mode] b. cargo test --workspace が走っている（最大 2.0 GB・今 300 MB = ピーク前）"
  else
    fail "[$mode] b. 大きい処理の注入が効かない: $(status_json | head -c 600)"
  fi

  # c. ピーク前の後ろ: どちらも待って諦める（押さえていた処理と量を出す）
  run_fg "pre-$mode" 1 --max-wait 5s -- test --test small
  if [ "$RC" -eq 75 ] && grep -q "押さえていた処理: $BIG cargo test --workspace（使用 300 MB・最大 2.0 GB・まだ使っていない見込み 6.7 GB" "$TMP/w-pre-$mode.err"; then
    pass "[$mode] c. ピーク前は待って 75（$(grep '押さえていた処理' "$TMP/w-pre-$mode.err" | sed 's/^tako queue: //')）"
  else
    fail "[$mode] c. ピーク前: RC=$RC $(cat "$TMP/w-pre-$mode.err")"
  fi

  # d. ピークを過ぎて使用量が下がった（最大 7.3 GB・今 0.3 GB）
  inject 8192 normal 7475
  wait_until 10 big_peak_is 7475 || fail "[$mode] d. 最大 7.3 GB の注入が効かない"
  run_fg "post-$mode" 1 --max-wait 6s -- test --test small
  POST_RC=$RC
  POST_DUR=$DUR
  cp "$TMP/w-post-$mode.err" "$TMP/keep-post-$mode.err"
  status_json > "$TMP/keep-status-$mode.json"
  mcp_queue > "$TMP/keep-mcp-$mode.json"
  q > "$TMP/keep-cli-$mode.txt" 2>&1
}

finish_mode() {
  q cancel "$BIG" >/dev/null 2>&1
  wait "$BIG" 2>/dev/null
  stop_arbiter
}

echo "== ① A/B: ピークを過ぎて使用量が下がった実行中の後ろの小さい処理（36 GB・空き 8 GB・予約 3.6 GB）=="
ab_run legacy env TAKO_2254_LEGACY=1 || exit 1
if [ "$POST_RC" -eq 75 ] && grep -q "待っていた理由: 空きメモリ不足（空き 8.0 GB・必要 13.7 GB = 実行中のまだ使っていない見込み 6.7 GB + この処理の見込み 7.0 GB・予約 3.6 GB）" "$TMP/keep-post-legacy.err"; then
  pass "修正前（TAKO_2254_LEGACY=1）: ピーク後も 6.7 GB を押さえ、小さい処理（見込み = test の既定 7 GB）は ${POST_DUR} 秒待って 75"
else
  fail "修正前の再現: RC=$POST_RC $(cat "$TMP/keep-post-legacy.err")"
fi
echo "    --- 修正前の wrapper の stderr（d）---"
sed 's/^/    /' "$TMP/keep-post-legacy.err"
finish_mode

ab_run fixed || exit 1
if [ "$POST_RC" -eq 0 ] && [ "$POST_DUR" -le 3 ] && grep -q "^end post-fixed" "$LOG"; then
  pass "修正後: ピークを過ぎた実行中は押さえず、小さい処理（見込み = 過去の記録 1.1 GB）は ${POST_DUR} 秒で走って 0"
else
  fail "修正後: RC=$POST_RC DUR=$POST_DUR $(cat "$TMP/keep-post-fixed.err")"
fi
python3 -c '
import json, sys
q = json.load(open(sys.argv[1]))
big = [r for r in q["running"] if "--workspace" in r["label"]][0]
ok = big["pending_mb"] == 0 and big["peak_passed"] is True and big["estimate_mb"] == 7168
print("pending_mb=%s peak_passed=%s estimate_mb=%s estimate_from=%s" % (big["pending_mb"], big["peak_passed"], big["estimate_mb"], big["estimate_from"]))
sys.exit(0 if ok else 1)
' "$TMP/keep-mcp-fixed.json" > "$TMP/mcp-fixed.txt" \
  && pass "修正後の MCP tako_queue: 実行中の cargo test --workspace は $(cat "$TMP/mcp-fixed.txt")" \
  || fail "修正後の MCP: $(cat "$TMP/mcp-fixed.txt") $(head -c 600 "$TMP/keep-mcp-fixed.json")"

echo "== ② 本当に空きが無いとき（修正後・大きい処理はピーク後のまま）=="
inject 4000 normal 7475
run_fg "short-fixed" 1 --max-wait 4s -- test --test small
if [ "$RC" -eq 75 ] && grep -q "待っていた理由: 空きメモリ不足（空き 3.9 GB・必要 1.1 GB・予約 3.6 GB）" "$TMP/w-short-fixed.err"; then
  pass "空き 3.9 GB < 1.1 GB + 予約 3.6 GB: 待って 75（押さえは 0 = $(grep '押さえていた処理' "$TMP/w-short-fixed.err" | sed 's/^tako queue: //')）"
else
  fail "空きが足りないのに入った / 理由が違う: RC=$RC $(cat "$TMP/w-short-fixed.err")"
fi
inject 30000 warn 7475
run_fg "warn-fixed" 1 --max-wait 4s -- test --test small
if [ "$RC" -eq 75 ] && grep -q "待っていた理由: メモリ圧 warn" "$TMP/w-warn-fixed.err"; then
  pass "メモリ圧 warn: 空き 29 GB でも実行中が居る間は待って 75"
else
  fail "メモリ圧 warn で入った / 理由が違う: RC=$RC $(cat "$TMP/w-warn-fixed.err")"
fi
inject 8192 normal 7475
run_fg "again-fixed" 1 --max-wait 4s -- test --test small
[ "$RC" -eq 0 ] && pass "normal・空き 8 GB へ戻せば入る（対照）" || fail "戻しても入らない: RC=$RC $(cat "$TMP/w-again-fixed.err")"
finish_mode

echo "== ③ 諦めた件の表示（修正前の d: CLI の tako queue と MCP の tako_queue）=="
if grep -q "直近の諦め: .*cargo test --test small" "$TMP/keep-cli-legacy.txt" \
  && grep -q "押さえていた処理: .*cargo test --workspace（使用 300 MB・最大 7.3 GB・まだ使っていない見込み 6.7 GB・見込み 7.0 GB（区分の既定）・ピーク後）" "$TMP/keep-cli-legacy.txt"; then
  pass "CLI の tako queue に諦めた件と押さえていた処理が出る"
else
  fail "CLI: $(sed -n '/^順番待ち/,$p' "$TMP/keep-cli-legacy.txt")"
fi
sed -n '/^順番待ち/,$p' "$TMP/keep-cli-legacy.txt" | sed 's/^/    /'
python3 -c '
import json, sys
q = json.load(open(sys.argv[1]))
g = [r for r in q["recent"] if r.get("gave_up")][0]
h = g["holders"][0]
print("recent[gave_up]: label=%s reason=%s holders[0]: label=%s memory_mb=%s pending_mb=%s estimate_mb=%s" % (
    g["label"], g["reason"], h["label"], h["memory_mb"], h["pending_mb"], h["estimate_mb"]))
sys.exit(0 if (g["reason"] == "memory" and "--workspace" in h["label"] and h["pending_mb"] == 6868) else 1)
' "$TMP/keep-mcp-legacy.json" > "$TMP/mcp-legacy.txt" \
  && pass "MCP tako_queue: $(cat "$TMP/mcp-legacy.txt")" \
  || fail "MCP: $(cat "$TMP/mcp-legacy.txt") $(head -c 800 "$TMP/keep-mcp-legacy.json")"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
