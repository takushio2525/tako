#!/bin/bash
# tako:run bash scripts/test-queue-priority-eta-1996.sh
#
# 順番待ちの優先度と終わりの見込み（#1996。エピック #1992 の S4）の実経路テスト。
#
# 隔離の調停役を GUI 無しで立て（`tako queue serve`。TAKO_QUEUE_SOCKET を明示したときだけ立つ）、本物の
# wrapper（`tako queue <コマンド>`）を並べて確かめる。**負荷は生成しない**: 待たせる処理は眠るだけ、cargo は
# 眠るビルドスクリプトだけの小さな workspace（CPU を焼かない）。
#
#   ① 段の既定（ペインの外 low・worker normal・master high・人が直接 top）で並ぶ
#   ② CLI `tako queue priority <受付番号> <段>` と MCP `tako_queue` の `priority` が同じ並びと段を返す
#   ③ 境界: 読めない段・居ない受付番号・受付番号とペインの両方・ペインの既定は GUI の UI が受ける（GUI 無しは断る）
#   ④ 端末でない cargo の `N/M` を wrapper が読み、AI に返る出力（stderr）に進みの行が混ざらない
#   ⑤ 同じコマンドの 2 回目から終わりの見込みが出る（過去の記録の所要時間）・記録に生の argv とパスが無い
#   ⑥ 端末の cargo は横取りしない（進みの行は cargo 自身が端末へ出す）
#   ⑦ 過去の記録が 512 KB を超えたら回す・読めない行は .unreadable.bak へ退避してから直る
#
# 落とすのは自分で起こした pid とソケットだけ（名前一致で殺さない）。
# 使い方: bash scripts/test-queue-priority-eta-1996.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh"

echo "tako-cli をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli) || exit 1
TAKO="$REPO_ROOT/target/debug/tako"
[ -x "$TAKO" ] || { echo "tako が無い: $TAKO"; exit 1; }

# 本番 GUI と外側の順番待ちを指す env を落とす（外側の `tako queue bash …` の枠の中でも入れ子と見なされない）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_1994_INJECT
unset CARGO_BUILD_JOBS RUST_TEST_THREADS CARGO_TERM_PROGRESS_WHEN CARGO_TERM_PROGRESS_WIDTH CARGO_TARGET_DIR

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 短い一時 dir（深いと受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk1996-XXXXXX)"
ARB_PID=""
BG_PIDS=""
cleanup() {
  [ -n "$ARB_PID" ] && kill "$ARB_PID" 2>/dev/null
  for p in $BG_PIDS; do kill "$p" 2>/dev/null; done
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR"
export TAKO_TOKEN="tk1996-$$"
SOCK="$TMP/q.sock"
INJECT="$TMP/inject.json"
HIST="$TAKO_DATA_DIR/resources/history.jsonl"
# 36 GB・空き 30 GB・normal・18 コア・立ち上げ直後の待ち 0
printf '{"total_mb":36864,"available_mb":30000,"pressure":"normal","swap_used_mb":0,"cpus":18,"unmanaged":0,"regather_secs":0}\n' > "$INJECT"

start_arbiter() {
  TAKO_QUEUE_SOCKET="$SOCK" TAKO_QUEUE_INJECT="$INJECT" "$TAKO" queue serve 2>>"$TMP/arb.log" &
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
status_json() { q --json 2>/dev/null; }
# python3 -I: 置き場の中の同名のモジュールを拾わない
py() { python3 -I -c "$@"; }

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

# 眠るだけの処理を並べる（`env …` で受付の手がかりを変える。$! = tako queue の pid = 受付番号）
enqueue_sleep() {
  local secs="$1"
  shift
  env "$@" TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sleep "$secs" </dev/null 2>>"$TMP/wrapper.err" &
  LAST_BG=$!
  BG_PIDS="$BG_PIDS $LAST_BG"
}

# 待ちの並び（受付番号:段:出どころ を空白区切り）
waiting_summary() {
  status_json | py 'import json,sys
v=json.load(sys.stdin)
print(" ".join("%s:%s:%s" % (w["id"], w["priority"], w["priority_from"]) for w in v["queue"]["waiting"]))'
}
waiting_count() { status_json | py 'import json,sys; print(len(json.load(sys.stdin)["queue"]["waiting"]))'; }
waiting_is() { [ "$(waiting_count)" = "$1" ]; }

# MCP の stdio ブリッジで 1 回呼ぶ（tools/call の本文 = text の JSON を出す）
mcp_call() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1996","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" |
    TAKO_SOCKET="$SOCK" "$TAKO" mcp serve 2>>"$TMP/mcp.err" |
    py 'import json,sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("id") == 2:
        r = m.get("result", {})
        t = (r.get("content") or [{}])[0].get("text", "")
        print(("ERROR " if r.get("isError") else "") + t)'
}

cd "$TMP" || exit 1
start_arbiter || exit 1
q limits --max-jobs 1 >/dev/null || fail "上限を 1 にできない"

echo "== ① 段の既定で並ぶ =="
enqueue_sleep 60 ; HOLD=$LAST_BG
wait_until 5 sh -c "TAKO_QUEUE_SOCKET='$SOCK' '$TAKO' queue --json 2>/dev/null | grep -q '\"phase\": \"running\"'"
enqueue_sleep 1 ; P_OUT=$LAST_BG                                              # ペインの外 → low
sleep 0.3
enqueue_sleep 1 TAKO_PANE_ID=901 TAKO_ORCHESTRATOR_ROLE=worker:tako ; P_WORKER=$LAST_BG   # worker → normal
sleep 0.3
enqueue_sleep 1 TAKO_PANE_ID=902 TAKO_ORCHESTRATOR_ROLE=master:proj ; P_MASTER=$LAST_BG   # master → high
sleep 0.3
wait_until 5 waiting_is 3
got="$(waiting_summary)"
want="$P_MASTER:high:role $P_WORKER:normal:role $P_OUT:low:role"
[ "$got" = "$want" ] && pass "master → worker → ペインの外の順（${got}）" || fail "並び ${got}（期待 ${want}）"
# 人が直接（役割の無いペインで stdin が端末）は top。端末は script(1) の疑似端末で与える
script -q /dev/null env TAKO_PANE_ID=903 TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sleep 1 >/dev/null 2>&1 &
P_HUMAN=$!
BG_PIDS="$BG_PIDS $P_HUMAN"
wait_until 5 waiting_is 4
human_line="$(status_json | py 'import json,sys
w=json.load(sys.stdin)["queue"]["waiting"][0]
print(w["priority"], w["priority_from"], w["label"])')"
case "$human_line" in
  "top role sleep 1") pass "人が端末から打った処理は top で先頭（${human_line}）" ;;
  *) fail "先頭が ${human_line}（期待 top role sleep 1）" ;;
esac

echo "== ② CLI と MCP の優先度変更が同じ結果 =="
cli_out="$(q priority "$P_OUT" top --json 2>&1)"
after_cli="$(waiting_summary)"
q priority "$P_OUT" default >/dev/null 2>&1
back="$(waiting_summary)"
mcp_out="$(mcp_call tako_queue "{\"action\":\"priority\",\"id\":$P_OUT,\"priority\":\"top\"}")"
after_mcp="$(waiting_summary)"
echo "    CLI → $after_cli"
echo "    MCP → $after_mcp"
[ -n "$after_cli" ] && [ "$after_cli" = "$after_mcp" ] && pass "CLI と MCP の後の並びと段が一致" || fail "CLI 後 $after_cli / MCP 後 $after_mcp"
case "$after_cli" in
  *"$P_OUT:top:job"*) pass "変えた処理は top（処理に付けた段）" ;;
  *) fail "変えた処理の段: $after_cli" ;;
esac
[ "$back" != "$after_cli" ] && case "$back" in *"$P_OUT:low:role"*) true ;; *) false ;; esac &&
  pass "default で既定（low = ペインの外）へ戻る" || fail "default の後: $back"
cli_fields="$(py 'import json,sys; v=json.loads(sys.argv[1]); print(v["priority"], v["priority_from"], v["changed"])' "$cli_out" 2>/dev/null)"
mcp_fields="$(py 'import json,sys; v=json.loads(sys.argv[1]); print(v["priority"], v["priority_from"], v["changed"])' "$mcp_out" 2>/dev/null)"
[ -n "$cli_fields" ] && [ "$cli_fields" = "$mcp_fields" ] && pass "応答の段・出どころ・changed が一致（${cli_fields}）" || fail "CLI 応答 [${cli_fields}] / MCP 応答 [${mcp_fields}]（${mcp_out}）"
# 人の読む表示に出どころが出る
human_status="$(q 2>/dev/null)"
[[ "$human_status" == *"top（処理に付けた段）"* ]] && pass "tako queue の待ちの行に段の出どころ" || fail "待ちの行に出どころが無い"

echo "== ③ 境界 =="
out="$(q priority "$P_OUT" urgent 2>&1)"; rc=$?
[ "$rc" -ne 0 ] && grep -q "urgent" <<<"$out" && pass "読めない段は断る（rc=${rc}）" || fail "読めない段: rc=$rc $out"
out="$(q priority 4242424 high 2>&1)"; rc=$?
[ "$rc" -ne 0 ] && grep -q "4242424" <<<"$out" && pass "居ない受付番号は断る" || fail "居ない受付番号: rc=$rc $out"
out="$(q priority 1 high --pane 5 2>&1)"; rc=$?
[ "$rc" -ne 0 ] && pass "受付番号とペインの両方は断る" || fail "両方: rc=$rc $out"
out="$(mcp_call tako_queue '{"action":"priority","pane":901,"priority":"high"}')"
case "$out" in
  ERROR*"GUI が無い"*) pass "ペインの既定は UI の受け口へ渡る（GUI 無しの調停役は断る）" ;;
  *) fail "ペインの既定の変更: $out" ;;
esac
kill "$HOLD" 2>/dev/null
wait "$HOLD" "$P_OUT" "$P_WORKER" "$P_MASTER" "$P_HUMAN" 2>/dev/null
wait_until 10 waiting_is 0

echo "== ④ 非 TTY の cargo の N/M（眠るビルドスクリプトの小さな workspace）=="
WS="$TMP/sleepws"
mkdir -p "$WS"
printf '[workspace]\nmembers = ["a", "b", "c"]\nresolver = "2"\n' > "$WS/Cargo.toml"
for c in a b c; do
  mkdir -p "$WS/$c/src"
  printf '[package]\nname = "sleep_%s"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n' "$c" > "$WS/$c/Cargo.toml"
  cat > "$WS/$c/build.rs" <<'RS'
// 眠るだけのビルドスクリプト（CPU を使わずに所要時間を作る。tako #1996 の e2e）
fn main() {
    let ms: u64 = std::env::var("SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    std::thread::sleep(std::time::Duration::from_millis(ms));
    println!("cargo:rerun-if-env-changed=SLEEP_SALT");
}
RS
  echo 'pub fn f() -> u32 { 1 }' > "$WS/$c/src/lib.rs"
done
# 1 回目（記録が無い）。stderr はファイル = 端末でない（AI の Bash ツールと同じ）
(cd "$WS" && SLEEP_SALT=1 TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue cargo build >"$TMP/b1.out" 2>"$TMP/b1.err") &
B1=$!
BG_PIDS="$BG_PIDS $B1"
saw_progress=""
for i in $(seq 1 60); do
  p="$(status_json | py 'import json,sys
r=json.load(sys.stdin)["queue"]["running"]
print(next(("%s/%s %s" % (x["progress"]["done"], x["progress"]["total"], x["eta"]["state"]) for x in r if x.get("progress")), ""))' 2>/dev/null)"
  [ -n "$p" ] && saw_progress="$p"
  kill -0 "$B1" 2>/dev/null || break
  sleep 0.5
done
wait "$B1"; rc1=$?
[ "$rc1" -eq 0 ] && pass "1 回目のビルドは成功" || fail "1 回目 rc=${rc1}（$(tail -3 "$TMP/b1.err")）"
[ -n "$saw_progress" ] && pass "status の running[].progress に N/M（${saw_progress}）" || fail "N/M が status に出なかった"
b1_lines="$(tr '\r' '\n' < "$TMP/b1.err")"
if [[ "$b1_lines" == *"Building ["* ]]; then
  fail "AI に返る stderr に進みの行が混ざった"
else
  pass "stderr に進みの行が無い"
fi
grep -q "Compiling sleep_a" "$TMP/b1.err" && grep -q "Finished" "$TMP/b1.err" && pass "普通の行（Compiling / Finished）は素通し" || fail "普通の行が欠けた: $(head -c 300 "$TMP/b1.err")"
grep -q $'\r' "$TMP/b1.err" && fail "stderr に \\r が残った" || pass "進みの行の \\r も残らない"

echo "== ⑤ 同じコマンドの 2 回目から終わりの見込み =="
[ -s "$HIST" ] && pass "1 回目の記録が残った" || fail "記録が無い（${HIST}）"
if [ -s "$HIST" ]; then
  py 'import json,sys
rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
r=rows[-1]
print("    記録:", json.dumps(r, ensure_ascii=False))' "$HIST"
  grep -q '/' "$HIST" && fail "記録にパスの区切りが載った" || pass "記録に / が無い（生の argv・パスが無い）"
  grep -q "$TMP\|sleepws\|SLEEP_SALT" "$HIST" && fail "記録に一時 dir・env が載った" || pass "記録に一時 dir・env の名前が無い"
  py 'import json,sys
r=[json.loads(l) for l in open(sys.argv[1]) if l.strip()][-1]
sys.exit(0 if r.get("name")=="cargo build" and (r.get("units") or 0) > 0 and len(r.get("key",""))==16 else 1)' "$HIST" &&
    pass "表示名は cargo build・鍵は 16 桁・ユニット数 M を持つ" || fail "記録の形が違う"
fi
(cd "$WS" && SLEEP_SALT=2 TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue cargo build >"$TMP/b2.out" 2>"$TMP/b2.err") &
B2=$!
BG_PIDS="$BG_PIDS $B2"
eta2=""
human2=""
for i in $(seq 1 60); do
  e="$(status_json | py 'import json,sys
r=json.load(sys.stdin)["queue"]["running"]
print(next(("%s|%s|%s" % (x["eta"]["state"], x["eta"].get("from"), x["eta"]["text"]) for x in r), ""))' 2>/dev/null)"
  case "$e" in remaining\|history\|*) eta2="$e" ;; esac
  if [ -n "$eta2" ]; then
    # 人の読む表示（実行中の行）も同じ時点で
    human_all="$(q 2>/dev/null)"
    human2="$(printf '%s\n' "$human_all" | sed -n '/^  実行  /p')"
    break
  fi
  kill -0 "$B2" 2>/dev/null || break
  sleep 0.3
done
wait "$B2"
[ -n "$eta2" ] && pass "2 回目は過去の所要時間から見込み（${eta2#*|*|}）" || fail "2 回目に見込みが出ない"
echo "    tako queue の表示: $human2"
case "$human2" in *"あと約"*"前回"*) pass "人の読む行に「あと約 N（前回 M）」" ;; *) [ -z "$eta2" ] || fail "人の読む行に見込みが無い" ;; esac

echo "== ⑥ 端末の cargo は横取りしない =="
# script(1) の疑似端末は幅 0 で、そのままだと cargo 自身も進みを出さない（2026-11-03 実測）ので幅を与える
(cd "$WS" && SLEEP_SALT=3 SLEEP_MS=1000 TAKO_QUEUE_SOCKET="$SOCK" TAKO_BIN="$TAKO" \
  script -q "$TMP/tty.typescript" sh -c 'stty cols 100 rows 30; exec "$TAKO_BIN" queue cargo build' \
  </dev/null >/dev/null 2>&1)
tty_lines="$(tr '\r' '\n' < "$TMP/tty.typescript")"
if [[ "$tty_lines" == *"Building"* ]]; then
  pass "端末では cargo 自身の進みがそのまま見える"
else
  fail "端末の出力に cargo の進みが無い（横取りした？）"
fi

echo "== ⑦ 記録の回しと壊れた行の退避 =="
# 上限（512 KB）を超えた読める記録を置いてから 1 回走らせる → history.1.jsonl へ回る
py 'import json,sys
row={"at":1,"key":"0123456789abcdef","name":"cargo build","kind":"build","peak_mb":1,"ran_s":1,"exit_code":0}
line=json.dumps(row)+"\n"
with open(sys.argv[1],"a") as f:
    f.write(line*(530*1024//len(line)+1))' "$HIST"
(cd "$WS" && SLEEP_SALT=4 TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue cargo build >/dev/null 2>&1)
[ -f "$TAKO_DATA_DIR/resources/history.1.jsonl" ] && [ "$(wc -c < "$HIST")" -lt 4096 ] &&
  pass "512 KB を超えたら history.1.jsonl へ回して新しく始める" || fail "回っていない（$(ls -l "$TAKO_DATA_DIR/resources")）"
# 壊れた行を挟んで調停役を立て直す → 退避してから読める行だけに直る
printf '{"key":\n' >> "$HIST"
cp "$HIST" "$TMP/broken.copy"
stop_arbiter
start_arbiter || exit 1
[ -f "$HIST.unreadable.bak" ] && cmp -s "$HIST.unreadable.bak" "$TMP/broken.copy" &&
  pass "元の中身はそのまま .unreadable.bak へ" || fail "退避が無い・中身が違う"
bad="$(py 'import json,sys
n=0
for l in open(sys.argv[1]):
    if l.strip():
        try: json.loads(l)
        except Exception: n+=1
print(n)' "$HIST")"
[ "$bad" = "0" ] && pass "history.jsonl は読める行だけ" || fail "読めない行が $bad 行残った"
mig="$(TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" migrate status --schema queue_history 2>&1)"
[[ "$mig" == *"queue_history"* ]] && pass "tako migrate status に queue_history が載る" || fail "migrate status: $mig"

stop_arbiter
echo
echo "結果: OK $PASS / NG $FAIL"
[ "$FAIL" -eq 0 ]
