#!/bin/bash
# test-ipc-listener-recover-2002.sh — IPC の受け口がソケットのパスから切り離されたら、
# GUI が自分で同じパスへ張り直す実経路テスト（Issue #2002）
#
# 直した症状（2026-10-12 の本番）: GUI は生きていて accept スレッドも accept で待っているのに、
# CLI / MCP が `Connection refused` のまま戻らなかった。段 1 の切り分けで再現したのは
# 「同じパスへ別の受け口が作り直し、その受け口が後から消えた」形（下の ②）。
#
# 何を確かめるか（隔離 GUI のソケットにだけ注入する）:
#   ① 平常: check-health の ipc 節に見張り（10 秒ごと）が出る
#   ② 同じパスへ別の受け口を張って閉じる（誰も受けない残骸）→ すぐは refused・
#      届かないときの check-health が「GUI は動いているのに受け口が応答しない」と次の一手を出す →
#      GUI が 12 秒以内に張り直して CLI と MCP（stdio ブリッジ）が通る・persist.log に 1 行
#   ③ ソケットファイルを消す → 張り直す
#   ④ 生きた別の受け口がパスを握っている間は奪わない（相手のファイルに触らない・persist.log は 1 行だけ）→
#      相手が去ったら張り直す
#   ⑤ 張り直しの最中に繋ぎ続けるクライアントがいても GUI は落ちず、張り直した後は全部通る
#   ⑥ 別の受け口がパスを握ったまま GUI を終了 → 相手のソケットファイルを消さない・終了後に張り直さない
#   ⑦ A/B: `TAKO_2002_LEGACY=1` では ② の後も refused のまま・persist.log は 0 行・
#      届かないときの check-health は修正前の表示（GUI の行が無い）
#   ⑧ 正常時: IDLE_SECS 秒動かして張り直し 0 件・確認の回数・CPU 時間と電力を A/B で並べる
#
# **本番の tako / 設定には一切触らない**（data / discovery / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **負荷は作らない**（注入はソケットファイルの差し替えだけ。繋ぎ続けるクライアントは 50 ms 間隔）。
#
# 使い方: bash scripts/test-ipc-listener-recover-2002.sh   （IDLE_SECS=60 が既定。⑧ の放置時間）
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail
# check-health / `tako setup --check` は表示言語に従う（#2365）。判定は日本語の字面なので、OS の言語に
# 依らず日本語表示へ固定する（settings.json の language が system のときだけ効く）
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IDLE_SECS="${IDLE_SECS:-60}"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$2" = 1 ]; then pass "$1"; else fail "$1"; fi; }

# ソケットのパスは sun_path（103 バイト）に収まる短い置き場へ（#1441 の短縮を通さず固定パスで測る）
TMP="$(mktemp -d /tmp/tk2002-XXXX)"
APP_PID=""
SIDE_PIDS=""
cleanup() {
  local p
  for p in $SIDE_PIDS; do kill "$p" 2>/dev/null; done
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  for p in "$TMP"/*/tmux; do
    [ -f "$p" ] && tmux -L "$(cat "$p")" kill-server >/dev/null 2>&1
  done
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1

# --- 注入とプローブ（perl の標準モジュールだけ） -----------------------------------
cat > "$TMP/probe.pl" <<'PL'
use strict; use warnings; use Socket; use POSIX ();
my %names = map { (POSIX->can($_) ? (POSIX->can($_)->() => $_) : ()) } qw(ECONNREFUSED ENOENT ENOTCONN);
sub errname { my $e = 0 + $!; return $names{$e} // "errno$e"; }
sub conn { my ($p) = @_; socket(my $s, PF_UNIX, SOCK_STREAM, 0) or die; return connect($s, pack_sockaddr_un($p)) ? "ok" : errname(); }
sub ino { my @st = lstat($_[0]); return @st ? "$st[0]:$st[1]" : "missing"; }
sub listen_at { my ($p) = @_; unlink $p; socket(my $l, PF_UNIX, SOCK_STREAM, 0) or die; bind($l, pack_sockaddr_un($p)) or die "bind: $!"; listen($l, 16) or die; return $l; }
my ($mode, $path, $secs) = @ARGV;
$| = 1;
if ($mode eq 'connect') { print conn($path), "\n"; }
elsif ($mode eq 'ino') { print ino($path), "\n"; }
elsif ($mode eq 'takeover') { my $l = listen_at($path); close $l; print ino($path), "\n"; }
elsif ($mode eq 'steal') { my $l = listen_at($path); print ino($path), "\n"; sleep $secs; unlink $path; }
elsif ($mode eq 'hammer') {
  my %n; my $end = time + $secs;
  while (time < $end) { $n{conn($path)}++; select(undef, undef, undef, 0.05); }
  print join(" ", map { "$_=$n{$_}" } sort keys %n), "\n";
}
PL
probe() { perl "$TMP/probe.pl" "$@"; }

# --- 起動ごとに data / discovery / tmux を分ける（2 回目が前の器を拾わない） --------------
start_gui() {
  local label="$1"; shift
  local dir="$TMP/$label"
  mkdir -p "$dir/d" "$dir/disc" "$dir/orch"
  export TAKO_DATA_DIR="$dir/d"
  export TAKO_DISCOVERY_DIR="$dir/disc"
  export TAKO_ORCHESTRATOR_DIR="$dir/orch"
  export TAKO_SESSIONS_FILE="$dir/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$dir/panelogs"
  export TAKO_WORKERS_FILE="$dir/workers.yaml"
  export TAKO_TMUX_SOCKET="tk2002-$$-$label"
  echo "$TAKO_TMUX_SOCKET" > "$dir/tmux"
  printf '{\n  "language": "ja"\n}\n' > "$TAKO_DATA_DIR/settings.json"
  SOCK="$TAKO_DATA_DIR/tako.sock"
  PLOG="$TAKO_DATA_DIR/persist.log"
  launch_isolated_gui "$dir/app.log" "$@" || exit $?
  APP_PID=$ISOLATED_GUI_PID
  wait_isolated_gui "$dir/app.log" 300 || exit 1
}
stop_gui() { stop_isolated_gui "$APP_PID"; APP_PID=""; }

health() { "$TAKO_BIN" check-health 2>&1 | sed -e "s#$TMP#<TMP>#g"; }
rebind_lines() { grep -c '張り直した（#2002）' "$PLOG" 2>/dev/null || true; }
issue_lines() { grep -c '#2002' "$PLOG" 2>/dev/null || true; }
# persist.log でその語を含む最後の行（tail は最後まで読む = pipefail で途中で抜けない）
last_line() { grep "$1" "$PLOG" | tail -n 1; }
# CLI が通るまで待つ（上限秒）。通った秒数（小数 1 桁）を返し、通らなければ空
wait_cli() {
  local limit="$1" t0 now
  t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  while :; do
    if "$TAKO_BIN" list >/dev/null 2>&1; then
      now=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
      perl -e "printf '%.1f', $now - $t0"
      return 0
    fi
    now=$(perl -MTime::HiRes=time -e 'printf "%.0f", time')
    [ "$((now - ${t0%.*}))" -ge "$limit" ] && return 1
    sleep 0.2
  done
}
# MCP の stdio ブリッジ（`tako mcp serve`）から tako_check_health を 1 回呼び、ipc.watch.rebinds を返す
mcp_rebinds() {
  local sock tok
  sock=$(perl -MJSON::PP -0e 'print decode_json(<>)->{socket}' "$TAKO_DISCOVERY_DIR/control.json")
  tok=$(perl -MJSON::PP -0e 'print decode_json(<>)->{token}' "$TAKO_DISCOVERY_DIR/control.json")
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2002","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_check_health","arguments":{}}}'
    sleep 10
  } | TAKO_SOCKET="$sock" TAKO_TOKEN="$tok" "$TAKO_BIN" mcp serve 2>/dev/null | perl -MJSON::PP -ne '
    my $m = eval { decode_json($_) } or next;
    next unless ($m->{id} // 0) == 2;
    my $t = $m->{result}{content}[0]{text} // ""; my $h = eval { JSON::PP->new->decode($t) } or do { print "isError\n"; next };
    print $h->{ipc}{watch}{rebinds} // "null", "\n";'
}
# proc_pid_rusage v6 の CPU（user + system 秒）と電力（J）
cat > "$TMP/ru.py" <<'PY'
import ctypes, sys
class RU(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16), ("f", ctypes.c_uint64 * 47), ("r", ctypes.c_uint64 * 9)]
class TB(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
tb = TB(); lib.mach_timebase_info(ctypes.byref(tb))
k = tb.numer / tb.denom / 1e9
ru = RU()
if lib.proc_pid_rusage(int(sys.argv[1]), 6, ctypes.byref(ru)) != 0:
    sys.exit(1)
print(f"{(ru.f[0] + ru.f[1]) * k:.4f} {ru.f[40] / 1e9:.4f}")
PY
ru() { python3 "$TMP/ru.py" "$1"; }

echo "== 新しいバイナリ（見張りあり・既定の間隔）"
start_gui new
echo "① 平常"
H=$(health)
echo "$H" | grep -E '^(IPC|  見張り)' | sed 's/^/    /'
check "check-health に見張り（10 秒ごと）が出る" "$(grep -q '見張り: 10 秒ごと' <<<"$H" && echo 1)"

echo "② 同じパスへ別の受け口を張って閉じる（誰も受けない残骸）"
probe hammer "$SOCK" 14 > "$TMP/hammer.txt" &
HAMMER=$!
SIDE_PIDS="$SIDE_PIDS $HAMMER"
sleep 0.5
DEAD_INO=$(probe takeover "$SOCK")
C=$(probe connect "$SOCK")
echo "    注入直後の connect: $C"
check "注入直後は refused（修正前の症状）" "$( [ "$C" = ECONNREFUSED ] && echo 1)"
H=$(health)
echo "$H" | grep -E '^(接続|IPC|  GUI|  次の一手)' | sed 's/^/    /'
check "届かないときの check-health が「GUI は動いているのに受け口が応答しない」を出す" \
  "$(grep -q 'GUI: 動いている（pid [0-9]*）のに受け口が応答しない（ソケットファイルはあるが誰も受けていない）' <<<"$H" && echo 1)"
check "次の一手（待ってもう一度・戻らなければ再起動）を出す" \
  "$(grep -q '次の一手: tako は受け口を 10 秒ごとに確かめて張り直す' <<<"$H" && echo 1)"
T=$(wait_cli 20)
echo "    CLI が通るまで: ${T:-20 秒で通らない} 秒"
check "12 秒以内に張り直して CLI が通る" "$(perl -e "print 1 if '${T:-99}' <= 12")"
NEW_INO=$(probe ino "$SOCK")
check "張り直したファイルは残骸とは別の実体" "$( [ "$NEW_INO" != "$DEAD_INO" ] && [ "$NEW_INO" != missing ] && echo 1)"
check "persist.log に張り直しが 1 行" "$( [ "$(rebind_lines)" = 1 ] && echo 1)"
grep '#2002' "$PLOG" | sed -e 's/^/    persist.log: /'
L=$(grep '張り直した（#2002）' "$PLOG")
check "その行の理由が「誰も受けていなかった」" "$(grep -q '誰も受けていなかった' <<<"$L" && echo 1)"
H=$(health)
echo "$H" | grep -E '^  見張り' | sed 's/^/    /'
check "check-health が張り直し 1 回と理由を出す" "$(grep -q '張り直し 1 回（最後: [0-9]* 秒前・ソケットファイルが別の実体に置き換わり、誰も受けていなかった）' <<<"$H" && echo 1)"
M=$(mcp_rebinds)
echo "    MCP（stdio ブリッジ）の tako_check_health: ipc.watch.rebinds=$M"
check "MCP の stdio ブリッジも通り、張り直しの記録が読める" "$( [ "$M" = 1 ] && echo 1)"

echo "⑤ 張り直しの最中に繋ぎ続けたクライアント"
wait "$HAMMER" 2>/dev/null
echo "    connect の結果: $(cat "$TMP/hammer.txt")"
check "GUI は落ちていない" "$(kill -0 "$APP_PID" 2>/dev/null && echo 1)"
C=$(probe connect "$SOCK")
check "張り直した後は繋がる" "$( [ "$C" = ok ] && echo 1)"

echo "③ ソケットファイルを消す"
rm -f "${SOCK:?}"
C=$(probe connect "$SOCK")
echo "    注入直後の connect: $C"
T=$(wait_cli 20)
echo "    CLI が通るまで: ${T:-20 秒で通らない} 秒"
check "消されても 12 秒以内に張り直す" "$(perl -e "print 1 if '${T:-99}' <= 12")"
check "persist.log の張り直しが 2 行目（理由: 消えていた）" \
  "$( [ "$(rebind_lines)" = 2 ] && grep -q '消えていた' <<<"$(last_line '張り直した（#2002）')" && echo 1)"

echo "④ 生きた別の受け口がパスを握る（14 秒 = 見張りの間隔より長く）"
BEFORE=$(issue_lines)
probe steal "$SOCK" 14 > "$TMP/steal.txt" &
STEAL=$!
SIDE_PIDS="$SIDE_PIDS $STEAL"
sleep 1
STOLEN=$(cat "$TMP/steal.txt")
sleep 11
check "握られている間は奪わない（相手のファイルの実体のまま）" "$( [ "$(probe ino "$SOCK")" = "$STOLEN" ] && echo 1)"
check "張り直していない" "$( [ "$(rebind_lines)" = 2 ] && echo 1)"
check "persist.log には「張り直さない」が 1 行だけ" \
  "$( [ "$(( $(issue_lines) - BEFORE ))" = 1 ] && grep -q '張り直さない（#2002）.*奪わずに待つ' <<<"$(last_line '#2002')" && echo 1)"
wait "$STEAL" 2>/dev/null
T=$(wait_cli 20)
echo "    相手が去ってから CLI が通るまで: ${T:-20 秒で通らない} 秒"
check "相手が去ったら 12 秒以内に張り直す" "$(perl -e "print 1 if '${T:-99}' <= 12")"

echo "⑥ 別の受け口がパスを握ったまま GUI を終了"
probe steal "$SOCK" 20 > "$TMP/steal2.txt" &
STEAL=$!
SIDE_PIDS="$SIDE_PIDS $STEAL"
sleep 1
STOLEN=$(cat "$TMP/steal2.txt")
LINES=$(rebind_lines)
stop_gui
check "終了しても相手のソケットファイルを消さない" "$( [ "$(probe ino "$SOCK")" = "$STOLEN" ] && echo 1)"
check "相手の受け口に繋がる" "$( [ "$(probe connect "$SOCK")" = ok ] && echo 1)"
check "終了の前後で張り直していない" "$( [ "$(rebind_lines)" = "$LINES" ] && echo 1)"
kill "$STEAL" 2>/dev/null; wait "$STEAL" 2>/dev/null

echo "== ⑦ A/B: TAKO_2002_LEGACY=1（修正前の挙動）"
start_gui legacy TAKO_2002_LEGACY=1
probe takeover "$SOCK" >/dev/null
# CLI も同じ env で旧表示へ戻す（両側が同じ env を読む）
H=$(export TAKO_2002_LEGACY=1; health)
echo "$H" | grep -E '^(接続|IPC|  GUI|  次の一手)' | sed 's/^/    /'
check "旧挙動の check-health には GUI の行が無い（修正前の表示）" "$(grep -q '^  GUI:' <<<"$H" || echo 1)"
T=$(wait_cli 15)
check "旧挙動は 15 秒たっても refused のまま（修正前の症状の再現）" "$( [ -z "$T" ] && [ "$(probe connect "$SOCK")" = ECONNREFUSED ] && echo 1)"
check "旧挙動の persist.log は 0 行" "$( [ "$(issue_lines)" = 0 ] && echo 1)"
stop_gui

echo "== ⑧ 正常時（${IDLE_SECS} 秒放置・A/B）"
for arm in idle-new idle-legacy; do
  if [ "$arm" = idle-new ]; then start_gui "$arm"; else start_gui "$arm" TAKO_2002_LEGACY=1; fi
  sleep 5
  R0=$(ru "$APP_PID")
  sleep "$IDLE_SECS"
  R1=$(ru "$APP_PID")
  CHECKS=$("$TAKO_BIN" check-health --json 2>/dev/null | perl -MJSON::PP -0ne 'my $w = decode_json($_)->{ipc}{watch}; print defined $w ? "$w->{checks}/$w->{rebinds}" : "null"')
  perl -e '
    my ($arm, $secs, $checks, @v) = @ARGV;
    printf "    %-11s CPU %.3f 秒（%.3f%%）・電力 %.1f mW・確認/張り直し %s\n",
      $arm, $v[2] - $v[0], ($v[2] - $v[0]) / $secs * 100, ($v[3] - $v[1]) / $secs * 1000, $checks;' \
    "$arm" "$IDLE_SECS" "$CHECKS" $R0 $R1
  if [ "$arm" = idle-new ]; then
    check "正常時の張り直しは 0 件（persist.log）" "$( [ "$(issue_lines)" = 0 ] && echo 1)"
    check "正常時も見張りは回っている（確認 $((IDLE_SECS / 10 - 1)) 回以上・張り直し 0）" \
      "$(echo "$CHECKS" | perl -ne "print 1 if m{^(\\d+)/0\$} && \$1 >= $((IDLE_SECS / 10 - 1))")"
  fi
  stop_gui
done

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
