#!/bin/bash
# tako:run bash scripts/test-ipc-timeout-2123.sh
#
# tako CLI の IPC の 1 往復に時間切れが入った（#2123）ことの実経路テスト。
#
# 接続を受けたまま返事をしない GUI（#2002 のメモリ圧 critical・#1976 の UI スレッドの固まり）に当たると、
# 修正前の CLI は応答の読み取りで永久に待っていた。#2121 で夜間リリースの重い処理（run_heavy）が
# `tako queue` へ並ぶようになったので、固まった夜は夜間リリースがそこで止まり続ける。
#
# **偽の受け口**（perl。接続を受けて返事をしない / 要求を読んで応答を半分だけ書いて止まる / accept しない）と
# **隔離の調停役**（`tako queue serve`。止めるのは自分で起こした pid だけ）で確かめる。
# 本番の GUI・本番の順番待ちには一切触れない（接続先は env で一時 dir のソケットだけを指す）。
#
#   ① A/B（夜間と同じ条件 = run_heavy・low・3h・/bin/bash -euo pipefail・既定の上限）:
#      修正前（TAKO_2123_LEGACY=1）は固まる（45 秒で打ち切って判定）/ 修正後は 30 秒で時間切れ → 直に走る
#   ② 黙り方の違い（上限 2 秒）: 返事をしない・応答を半分で止める・accept しない・env の TAKO_SOCKET 経由
#   ③ 同じ roundtrip を通る他の CLI（list / orchestrator report / queue の状況）が同じ文言で返る
#   ④ 進捗が届いている長い順番待ち（既定の上限のまま 70 秒 = 1 往復の上限 30 秒・見失いの猶予 60 秒より長い）は切られない
#   ⑤ 待ちの途中で調停役が固まる（SIGSTOP）→ 見失いの猶予（60 秒）で直に走る
#   ⑥ 実行中に調停役が固まる → 子は走りきり、tako queue は子の終わりから 5 秒 × 2 往復の内に終わる
#
# 使い方: bash scripts/test-ipc-timeout-2123.sh（/bin/bash = 3.2 でも通す）。所要 約 5 分

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
QUEUE_SH="$REPO_ROOT/scripts/lib/queue.sh"
. "$QUEUE_SH"

echo "tako-cli をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli) || exit 1
TAKO="$REPO_ROOT/target/debug/tako"
[ -x "$TAKO" ] || { echo "tako が無い: $TAKO"; exit 1; }

# 本番 GUI と外側の順番待ちを指す env を落とす（このシェルは本番の GUI に繋がっている）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_QUEUE_MAX_WAIT
unset TAKO_NO_QUEUE TAKO_QUEUE_FALLBACKS TAKO_IPC_TIMEOUT_SECS TAKO_2123_LEGACY
unset _TAKO_QUEUE_SUPPORTED _TAKO_QUEUE_BIN CARGO_BUILD_JOBS RUST_TEST_THREADS

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
expect_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待: $2 / 実際: $3）"; fi; }
expect_has() { case "$3" in *"$2"*) pass "$1" ;; *) fail "$1（「$2」が無い: $3）" ;; esac; }
expect_lacks() { case "$3" in *"$2"*) fail "$1（「$2」が在る: $3）" ;; *) pass "$1" ;; esac; }
# 秒数が範囲 [lo, hi] に入るか
expect_within() {
  if [ "$2" -ge "$3" ] && [ "$2" -le "$4" ]; then pass "$1（${2} 秒）"; else fail "$1（${2} 秒・期待 ${3}〜${4} 秒）"; fi
}

# 短い一時 dir（深いと受け口のパスが sun_path の上限を超える = #1441）
TMP="$(mktemp -d /tmp/tk2123-XXXXXX)"
FAKE_PIDS=""
ARB_PID=""
BG_PIDS=""
cleanup() {
  # wait で刈り取る（bash がジョブの「Terminated」を出さないように）
  for p in $FAKE_PIDS; do kill "$p" 2>/dev/null; wait "$p" 2>/dev/null; done
  if [ -n "$ARB_PID" ]; then
    kill -CONT "$ARB_PID" 2>/dev/null
    kill "$ARB_PID" 2>/dev/null
  fi
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
export TAKO_TOKEN="tk2123-$$"
LOG="$TMP/log"
: > "$LOG"
export LOG

now() { perl -MTime::HiRes=time -e 'printf "%.1f\n", time'; }
# 経過秒（整数・四捨五入）
since() { perl -e 'printf "%d\n", $ARGV[1] - $ARGV[0] + 0.5' "$1" "$(now)"; }

# 偽の受け口を立てる: fake <名前> <silent|partial|noaccept>。戻りの $FAKE_SOCK が接続先
#   silent   = 接続を受けて要求も読まず、返事をしないまま持ち続ける（固まった GUI）
#   partial  = 要求を 1 行読み、応答を半分だけ書いて止まる（少しずつ届く応答で上限が延びないこと）
#   noaccept = listen だけして accept しない（受け口のスレッドが詰まった。接続は backlog に積まれる）
fake() {
  FAKE_SOCK="$TMP/$1.sock"
  perl -MIO::Socket::UNIX -e '
    my ($path, $mode) = @ARGV;
    unlink $path;
    my $l = IO::Socket::UNIX->new(Type => SOCK_STREAM(), Local => $path, Listen => 64) or die "listen: $!";
    open(my $f, ">", "$path.ready"); close $f;
    if ($mode eq "noaccept") { sleep 3600; exit 0 }
    my @keep;
    while (my $c = $l->accept) {
      push @keep, $c;
      if ($mode eq "partial") {
        my $line = <$c>;
        $c->autoflush(1);
        print $c "{\"jsonrpc\":\"2.0\",\"id\":1,";
      }
    }
  ' "$FAKE_SOCK" "$2" 2>>"$TMP/fake.log" &
  FAKE_PIDS="$FAKE_PIDS $!"
  local i
  for i in $(seq 1 50); do
    [ -f "$FAKE_SOCK.ready" ] && return 0
    sleep 0.1
  done
  echo "  偽の受け口が立たない（$TMP/fake.log）"
  return 1
}

# 上限の秒数で打ち切って走らせる（打ち切られたら 142 = 128 + SIGALRM）
with_alarm() {
  local secs="$1"
  shift
  perl -e 'alarm shift; exec @ARGV or exit 127' "$secs" "$@"
}

# 上限の秒数で打ち切る（プロセスグループごと = run_heavy の中の tako queue も残さない。打ち切ったら 142）。
# 新しいグループを作ってそこだけへ TERM → 1 秒で KILL（自分が起こしたプロセスだけ）
with_alarm_group() {
  perl -e '
    my $secs = shift;
    my $pid = fork() // die "fork: $!";
    if ($pid == 0) { setpgrp(0, 0); exec @ARGV or exit 127 }
    $SIG{ALRM} = sub { kill "TERM", -$pid; sleep 1; kill "KILL", -$pid; waitpid($pid, 0); exit 142 };
    alarm $secs;
    waitpid($pid, 0);
    exit(($? & 127) ? 128 + ($? & 127) : $? >> 8);
  ' "$@"
}

# 夜間と同じ条件で run_heavy を 1 回呼ぶ（.app の代わりにこのビルドの tako を候補にする。PATH に tako は置かない）
heavy() {
  env PATH="/usr/bin:/bin" TAKO_QUEUE_FALLBACKS="$TAKO" TAKO_QUEUE_PRIORITY=low TAKO_QUEUE_MAX_WAIT=3h \
    /bin/bash -euo pipefail -c '. "$1"; shift; run_heavy "$@"' _ "$QUEUE_SH" "$@"
}

echo "== ① A/B: 夜間と同じ条件の run_heavy が、返事をしない受け口に当たる =="
fake night silent || exit 1
NIGHT="$FAKE_SOCK"
# 修正前は run_heavy ごと 45 秒で打ち切る（heavy と同じ env を perl の alarm の下で組む）
t0="$(now)"
rc=0
out="$(PATH="/usr/bin:/bin" TAKO_QUEUE_FALLBACKS="$TAKO" TAKO_QUEUE_PRIORITY=low TAKO_QUEUE_MAX_WAIT=3h \
  TAKO_QUEUE_SOCKET="$NIGHT" TAKO_2123_LEGACY=1 \
  with_alarm_group 45 \
  /bin/bash -euo pipefail -c '. "$1"; shift; run_heavy "$@"' _ "$QUEUE_SH" sh -c 'echo ran; exit 3' 2>"$TMP/legacy.err")" || rc=$?
el="$(since "$t0")"
expect_eq "修正前（TAKO_2123_LEGACY=1）: 45 秒で打ち切られる（= 固まっていた）" "142" "$rc"
expect_eq "修正前: コマンドは走らない" "" "$out"
expect_within "修正前: 打ち切りまで待ち続けた" "$el" 44 50
t0="$(now)"
rc=0
out="$(TAKO_QUEUE_SOCKET="$NIGHT" heavy sh -c 'echo ran; exit 3' 2>"$TMP/new.err")" || rc=$?
el="$(since "$t0")"
expect_eq "修正後: コマンドが走る" "ran" "$out"
expect_eq "修正後: コマンドの終了コードがそのまま返る" "3" "$rc"
expect_within "修正後: 既定の上限（30 秒）で時間切れ → 直に走る" "$el" 29 36
expect_has "修正後: 応答しないので並ばずに走らせたと 1 行知らせる" "tako が 30 秒応答しないので、順番待ちせずに走らせる" "$(cat "$TMP/new.err")"

echo "== ② 黙り方の違い（TAKO_IPC_TIMEOUT_SECS=2）=="
for mode in silent partial noaccept; do
  fake "s-$mode" "$mode" || exit 1
  t0="$(now)"
  rc=0
  out="$(TAKO_QUEUE_SOCKET="$FAKE_SOCK" TAKO_IPC_TIMEOUT_SECS=2 with_alarm 20 "$TAKO" queue sh -c 'echo ran; exit 4' 2>"$TMP/$mode.err")" || rc=$?
  el="$(since "$t0")"
  expect_eq "$mode: コマンドが走って終了コード 4" "ran/4" "$out/$rc"
  expect_within "$mode: 上限（2 秒）で時間切れ" "$el" 1 5
  expect_has "$mode: 時間切れとして知らせる（途中で切れた扱いにしない）" "2 秒応答しないので" "$(cat "$TMP/$mode.err")"
done
# 順番待ちの受け口を明示しない = いつもの解決（env の TAKO_SOCKET → 発見ファイル）の経路
t0="$(now)"
rc=0
out="$(TAKO_SOCKET="$NIGHT" TAKO_IPC_TIMEOUT_SECS=2 with_alarm 20 "$TAKO" queue sh -c 'echo ran; exit 0' 2>"$TMP/env.err")" || rc=$?
el="$(since "$t0")"
expect_eq "TAKO_SOCKET 経由: コマンドが走る" "ran/0" "$out/$rc"
expect_within "TAKO_SOCKET 経由: 上限（2 秒）で時間切れ（残りの候補へ回って伸びない）" "$el" 1 5
rc=0
TAKO_SOCKET="$NIGHT" TAKO_2123_LEGACY=1 with_alarm 6 "$TAKO" queue sh -c 'echo ran' >/dev/null 2>&1 || rc=$?
expect_eq "TAKO_SOCKET 経由の修正前: 固まる（6 秒で打ち切り）" "142" "$rc"
rc=0
out="$(TAKO_QUEUE_SOCKET="$NIGHT" TAKO_IPC_TIMEOUT_SECS=0 with_alarm 4 "$TAKO" queue sh -c 'echo ran' 2>/dev/null)" || rc=$?
expect_eq "TAKO_IPC_TIMEOUT_SECS=0 は上限なし（4 秒で打ち切り）" "142/" "$rc/$out"

echo "== ③ 同じ roundtrip を通る他の CLI =="
t0="$(now)"
rc=0
err="$(TAKO_SOCKET="$NIGHT" TAKO_IPC_TIMEOUT_SECS=2 with_alarm 20 "$TAKO" list 2>&1 >/dev/null)" || rc=$?
el="$(since "$t0")"
expect_eq "tako list: 失敗の終了コード（1）で返る" "1" "$rc"
expect_within "tako list: 上限（2 秒）で返る" "$el" 1 5
expect_has "tako list: 時間切れの文言（秒数と上書きの口）" "tako アプリが 2 秒応答しない" "$err"
expect_has "tako list: 上書きの env を案内する" "TAKO_IPC_TIMEOUT_SECS" "$err"
rc=0
err="$(TAKO_SOCKET="$NIGHT" TAKO_IPC_TIMEOUT_SECS=2 with_alarm 20 "$TAKO" orchestrator report --pane 1 2>&1 >/dev/null)" || rc=$?
expect_eq "tako orchestrator report: 失敗で返る" "1" "$rc"
expect_has "tako orchestrator report: 同じ文言" "tako アプリが 2 秒応答しない" "$err"
rc=0
TAKO_SOCKET="$NIGHT" TAKO_2123_LEGACY=1 with_alarm 6 "$TAKO" list >/dev/null 2>&1 || rc=$?
expect_eq "tako list の修正前: 固まる（6 秒で打ち切り）" "142" "$rc"
rc=0
st="$(TAKO_QUEUE_SOCKET="$NIGHT" TAKO_IPC_TIMEOUT_SECS=2 with_alarm 20 "$TAKO" queue --json 2>/dev/null)" || rc=$?
expect_eq "tako queue（状況）: 機の値をこのプロセスで採って 0 で返る" "0" "$rc"
expect_has "tako queue（状況）: GUI は居ない扱い" '"gui": false' "$st"
expect_has "tako queue（状況）: 応答しないことを添える" "2 秒応答しない" "$st"

echo "== ④ 進捗が届いている長い順番待ちは切られない（既定の上限のまま）=="
SOCK="$TMP/q.sock"
INJECT="$TMP/inject.json"
# 注入する機の値（負荷は生成しない）
printf '{"total_mb":36864,"available_mb":30000,"pressure":"normal","swap_used_mb":0,"cpus":18,"unmanaged":0,"regather_secs":0,"stall_secs":600}\n' > "$INJECT"
TAKO_QUEUE_SOCKET="$SOCK" TAKO_QUEUE_INJECT="$INJECT" "$TAKO" queue serve 2>>"$TMP/arb.log" &
ARB_PID=$!
up=no
for i in $(seq 1 50); do
  if st="$(TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue --json 2>/dev/null)" \
    && grep -q '"arbiter": true' <<<"$st"; then up=yes; break; fi
  sleep 0.1
done
[ "$up" = yes ] || { echo "  調停役が立たない（$TMP/arb.log）"; exit 1; }
TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue limits --max-jobs 1 >/dev/null || fail "上限を 1 にできない"

# 偽の重い処理: 開始と終了を記録して眠るだけ
cat > "$TMP/fake.sh" <<'SH'
#!/bin/sh
echo "start $1 $(perl -MTime::HiRes=time -e 'printf "%.1f", time')" >> "$LOG"
sleep "$2" &
SP=$!
echo "sleep $1 $SP" >> "$LOG"
wait "$SP"
echo "end $1 $(perl -MTime::HiRes=time -e 'printf "%.1f", time')" >> "$LOG"
exit "${3:-0}"
SH
chmod +x "$TMP/fake.sh"
stamp() { awk -v k="$1" -v n="$2" '$1 == k && $2 == n {print $3}' "$LOG"; }
# 一時 dir から並べる（worktree の中から並べると target dir の区分で全件 1 件ずつになる）
cd "$TMP" || exit 1

TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh "$TMP/fake.sh" A 72 2>"$TMP/A.err" &
PA=$!
BG_PIDS="$BG_PIDS $PA"
sleep 1
TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh "$TMP/fake.sh" B 0 2>"$TMP/B.err" &
PB=$!
BG_PIDS="$BG_PIDS $PB"
wait "$PA"
wait "$PB"
rc=$?
a_end="$(stamp end A)"
b_start="$(stamp start B)"
waited="$(perl -e 'printf "%d", $ARGV[1] - $ARGV[0]' "$(stamp start A)" "$b_start")"
expect_eq "B は待ち終えて 0 で終わる" "0" "$rc"
if perl -e 'exit !($ARGV[1] >= $ARGV[0])' "$a_end" "$b_start"; then pass "B は A の終わりの後に走った（待ち ${waited} 秒 > 1 往復の上限 30 秒・猶予 60 秒）"; else fail "B が A の終わる前に走った（A の終わり ${a_end} / B の開始 ${b_start}）"; fi
expect_lacks "B は応答しない・見失ったと判定していない" "応答しない" "$(cat "$TMP/B.err")"
expect_lacks "B は順番待ちを抜けていない" "順番待ちせずに走らせる" "$(cat "$TMP/B.err")"

echo "== ⑤ 待ちの途中で調停役が固まる（SIGSTOP）→ 見失いの猶予で直に走る =="
TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh "$TMP/fake.sh" C 300 2>"$TMP/C.err" &
PC=$!
BG_PIDS="$BG_PIDS $PC"
sleep 1
TAKO_QUEUE_SOCKET="$SOCK" TAKO_IPC_TIMEOUT_SECS=2 "$TAKO" queue sh "$TMP/fake.sh" D 0 2>"$TMP/D.err" &
PD=$!
BG_PIDS="$BG_PIDS $PD"
sleep 3
kill -STOP "$ARB_PID"
t0="$(now)"
rc=0
with_alarm 120 /bin/sh -c 'while kill -0 "$1" 2>/dev/null; do sleep 0.5; done' _ "$PD" || rc=$?
el="$(since "$t0")"
wait "$PD"
drc=$?
expect_eq "D は固まった調停役を待ち続けない（120 秒の内に終わる）" "0" "$rc"
expect_eq "D は直に走って 0 で終わる" "0" "$drc"
# 上側 = 止めてから最初の poll（≦ 1 秒）+ 猶予 60 秒 + 1 周（往復 2 秒 + 繋ぎ直し最大 5 秒）+ 余裕
expect_within "D は見失いの猶予（60 秒）で走る" "$el" 58 75
expect_has "D は応答しないまま 60 秒たったと知らせる" "順番待ちの調停役が応答しないまま 60 秒たったので、待たずに走らせる" "$(cat "$TMP/D.err")"
kill -CONT "$ARB_PID"
kill "$PC" 2>/dev/null
wait "$PC" 2>/dev/null

echo "== ⑥ 実行中に調停役が固まる → 子は走りきり、すぐ終わる =="
TAKO_QUEUE_SOCKET="$SOCK" "$TAKO" queue sh "$TMP/fake.sh" E 4 7 2>"$TMP/E.err" &
PE=$!
BG_PIDS="$BG_PIDS $PE"
for i in $(seq 1 50); do
  grep -q '^start E ' "$LOG" && break
  sleep 0.1
done
kill -STOP "$ARB_PID"
wait "$PE"
erc=$?
done_at="$(now)"
e_end="$(stamp end E)"
lag="$(perl -e 'printf "%d", $ARGV[1] - $ARGV[0] + 0.5' "$e_end" "$done_at")"
kill -CONT "$ARB_PID"
expect_eq "子の終了コード（7）がそのまま返る" "7" "$erc"
expect_within "tako queue は子の終わりから心拍と done の上限（5 秒 × 2）の内に終わる" "$lag" 0 11

kill "$ARB_PID" 2>/dev/null
wait "$ARB_PID" 2>/dev/null
ARB_PID=""

echo
echo "結果: OK ${PASS} / NG ${FAIL}"
[ "$FAIL" -eq 0 ]
