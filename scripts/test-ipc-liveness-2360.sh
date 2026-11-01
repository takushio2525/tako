#!/bin/bash
# test-ipc-liveness-2360.sh — 起動時の IPC ソケットの生存確認が、一時的に繋がらない（backlog あふれ =
# ECONNREFUSED）生きた GUI の受け口を「死んだ残骸」と読んで奪わない実経路テスト（Issue #2360）
#
# 直した症状: 起動時の生存確認（`preferred_socket_path`）は connect 1 回だけで、繋がらなければ
# `bind` が固定パスを unlink して張り直していた。生きた GUI（A）の受け口が一時的に満杯の瞬間に、
# 同じ data dir の 2 つ目の tako-app（B）が多重起動のガード（#113 の control.json の pid・
# #177 の tmux クライアント）をすり抜けて起動すると、B が A の受け口を奪う。A は生きたまま
# パスから切り離され、A のペインのシェル（TAKO_SOCKET = 固定パス）の CLI / MCP は B へ届く。
#
# 注入（負荷は作らない）: A を SIGSTOP で止め、受け口の backlog を perl が握る接続で埋める
# （止まった A は accept しない = 次の connect は ECONNREFUSED）。その状態で B を起動する。
# B は A と同じ TAKO_DATA_DIR / TAKO_DISCOVERY_DIR を使う（起動ごとに別の dir = 前の器を拾わない）。
#
# 何を確かめるか:
#   ① ガードが効く形（control.json あり）: B はセカンダリで一時パス（修正前も同じ = 対照）
#   ② ガードが盲目（control.json が無い）: B は A の受け口を奪わずセカンダリで一時パス・
#      persist.log に #2360 の 1 行・B の check-health（JSON の ipc.fixed_path と「固定パス:」の行）が
#      busy と A の pid を出す・A を再開すると固定パスの CLI と MCP（stdio ブリッジ）が A へ届く・
#      B が先に終わっても A の受け口はそのまま（2 つ目が先に終わる境界）
#   ③ control.json が壊れている: ② と同じ
#   ④ A/B: ② に `TAKO_2360_LEGACY=1`（B だけ）= 修正前の挙動 → B が固定パスを奪い、固定パスの
#      CLI は B へ届く・A の見張り（#2002）は「張り直さない（奪わずに待つ）」→ B が去ると張り直す
#   ⑤ 本当の残骸（A を SIGKILL = pid が死んでいる・ずっと refused）: B が作り直して固定パスで立つ・
#      persist.log に「残骸を作り直す」の 1 行・B の check-health の ipc.fixed_path が dead
#   ⑥ control.json の pid が別のプロセスに再利用されている + 残骸: ⑤ と同じく作り直す
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-ipc-liveness-2360.sh` で渡すと
# ② ③ が NG になる（B が奪う = #2360 の症状）。① ④ は修正前も通る（④ は修正前のバイナリが env を
# 知らないだけで、奪う挙動はそのまま）。⑤ ⑥ は作り直す挙動は修正前も同じで、persist.log の行と
# check-health の記録だけが NG になる（修正前は残骸を作り直しても何も残さない）。
#
# **本番の tako / 設定には一切触らない**（data / discovery / HOME は mktemp 配下、tmux は専用ソケット、
# 止める・落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-ipc-liveness-2360.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail
# check-health / `tako setup --check` は表示言語に従う（#2365）。判定は日本語の字面なので、OS の言語に
# 依らず日本語表示へ固定する（settings.json の language が system のときだけ効く）
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$2" = 1 ]; then pass "$1"; else fail "$1"; fi; }

# ソケットのパスは sun_path（103 バイト）に収まる短い置き場へ（#1441 の短縮を通さず固定パスで測る）
TMP="$(mktemp -d /tmp/tk2360-XXXX)"
A_PID=""
B_PID=""
SIDE_PIDS=""
cleanup() {
  local p
  # 止めたままの A を落とすと SIGTERM が届かないので先に再開する
  [ -n "$A_PID" ] && kill -CONT "$A_PID" 2>/dev/null
  for p in $SIDE_PIDS; do kill "$p" 2>/dev/null; done
  [ -n "$B_PID" ] && stop_isolated_gui "$B_PID"
  [ -n "$A_PID" ] && stop_isolated_gui "$A_PID"
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
my %names = map { (POSIX->can($_) ? (POSIX->can($_)->() => $_) : ()) } qw(ECONNREFUSED ENOENT ENOTCONN EMFILE);
sub errname { my $e = 0 + $!; return $names{$e} // "errno$e"; }
sub conn { my ($p) = @_; socket(my $s, PF_UNIX, SOCK_STREAM, 0) or die; return connect($s, pack_sockaddr_un($p)) ? "ok" : errname(); }
sub ino { my @st = lstat($_[0]); return @st ? "$st[0]:$st[1]" : "missing"; }
my ($mode, $path, $secs) = @ARGV;
$| = 1;
if ($mode eq 'connect') { print conn($path), "\n"; }
elsif ($mode eq 'ino') { print ino($path), "\n"; }
elsif ($mode eq 'fill') {
  # 止まった受け口の backlog を埋めて握る（次の connect が ECONNREFUSED になるまで）
  my (@held, $stop);
  for (1 .. 4096) {
    socket(my $s, PF_UNIX, SOCK_STREAM, 0) or do { $stop = "socket:" . errname(); last };
    if (connect($s, pack_sockaddr_un($path))) { push @held, $s; } else { $stop = errname(); last; }
  }
  print "queued=", scalar(@held), " stop=", ($stop // "none"), "\n";
  sleep $secs;
}
PL
probe() { perl "$TMP/probe.pl" "$@"; }

# 起動ごとに data / discovery / tmux を分ける（2 回目が前の器を拾わない）。A と B は同じものを使う
arm_env() {
  local label="$1"
  ARM_DIR="$TMP/$label"
  mkdir -p "$ARM_DIR/d" "$ARM_DIR/disc" "$ARM_DIR/orch"
  export TAKO_DATA_DIR="$ARM_DIR/d"
  export TAKO_DISCOVERY_DIR="$ARM_DIR/disc"
  export TAKO_ORCHESTRATOR_DIR="$ARM_DIR/orch"
  export TAKO_SESSIONS_FILE="$ARM_DIR/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$ARM_DIR/panelogs"
  export TAKO_WORKERS_FILE="$ARM_DIR/workers.yaml"
  export TAKO_TMUX_SOCKET="tk2360-$$-$label"
  echo "$TAKO_TMUX_SOCKET" > "$ARM_DIR/tmux"
  printf '{\n  "language": "ja"\n}\n' > "$TAKO_DATA_DIR/settings.json"
  SOCK="$TAKO_DATA_DIR/tako.sock"
  PLOG="$TAKO_DATA_DIR/persist.log"
  CONTROL="$TAKO_DISCOVERY_DIR/control.json"
}

# A を起こしてタブに印を付ける（固定パスの CLI がどちらへ届いたかを印で見分ける）
launch_a() {
  launch_isolated_gui "$ARM_DIR/a.log" TAKO_IPC_WATCH_MS=1000 || exit $?
  A_PID=$ISOLATED_GUI_PID
  wait_isolated_gui "$ARM_DIR/a.log" 300 || exit 1
  "$TAKO_BIN" tab new --title MARK-A >/dev/null 2>&1
}

# B を起こし、接続情報（instances/control-<pid>.json。プライマリもセカンダリも書く）が出るまで待つ
launch_b() {
  local i
  launch_isolated_gui "$ARM_DIR/b.log" TAKO_IPC_WATCH_MS=1000 "$@" || exit $?
  B_PID=$ISOLATED_GUI_PID
  for i in $(seq 1 600); do
    [ -f "$TAKO_DISCOVERY_DIR/instances/control-$B_PID.json" ] && return 0
    kill -0 "$B_PID" 2>/dev/null || break
    sleep 0.1
  done
  echo "B の接続情報が出ない:"
  tail -20 "$ARM_DIR/b.log"
  return 1
}

stop_a() { [ -n "$A_PID" ] && { kill -CONT "$A_PID" 2>/dev/null; stop_isolated_gui "$A_PID"; }; A_PID=""; }
stop_b() { [ -n "$B_PID" ] && stop_isolated_gui "$B_PID"; B_PID=""; }

# 固定パスの CLI（A のペインのシェルが使う TAKO_SOCKET と同じ口）。トークンは data dir の共有ファイル
via_fixed() { TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" list 2>&1; }
fixed_answers_a() { local out; out=$(via_fixed); [[ "$out" == *MARK-A* ]] && echo 1; }
# その pid が固定パスの名前で張った受け口を持っているか（lsof の unix ソケットの名前）
holds_fixed() {
  local names
  names=$(lsof -nP -a -p "$1" -U -F n 2>/dev/null | sed -n 's/^n//p')
  grep -qxF "$SOCK" <<<"$names" && echo 1
}
# MCP の stdio ブリッジ（`tako mcp serve`）を固定パスへ繋ぎ、tako_list_panes に印が見えれば 1
mcp_answers_a() {
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2360","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_list_panes","arguments":{}}}'
    sleep 5
  } | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" mcp serve 2>/dev/null | perl -MJSON::PP -ne '
    my $m = eval { decode_json($_) } or next;
    next unless ($m->{id} // 0) == 2;
    my $t = $m->{result}{content}[0]{text} // "";
    print "1\n" if !$m->{result}{isError} && $t =~ /MARK-A/;'
}
# B の check-health（B の受け口へ繋ぐ = 接続情報の instances/control-<pid>.json の socket）
b_health() {
  local sock
  sock=$(perl -MJSON::PP -0ne 'print decode_json($_)->{socket}' "$TAKO_DISCOVERY_DIR/instances/control-$B_PID.json")
  TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" check-health "$@" 2>&1
}
# ipc.fixed_path を `state/pid` の 1 語で（節が無ければ null）
b_fixed_path() {
  b_health --json | perl -MJSON::PP -0ne '
    my $j = eval { decode_json($_) } or do { print "unreadable"; exit };
    my $f = $j->{ipc}{fixed_path};
    print defined $f ? "$f->{state}/" . ($f->{pid} // "null") : "null";'
}
lines() { local n; n=$(grep -c "$1" "$PLOG" 2>/dev/null); echo "${n:-0}"; }
show_log() { grep -E "$1" "$PLOG" 2>/dev/null | sed -e "s#$TMP#<TMP>#g" -e 's/^/    persist.log: /'; }

# A を止めて受け口を埋める。埋まったら $FILL に握る perl の pid
stop_and_fill_a() {
  local i
  kill -STOP "$A_PID"
  # 関数（probe）を `&` で背景にすると $! は副シェルの pid になり、kill が perl へ届かない
  perl "$TMP/probe.pl" fill "$SOCK" 300 > "$ARM_DIR/fill.txt" &
  FILL=$!
  SIDE_PIDS="$SIDE_PIDS $FILL"
  for i in $(seq 1 100); do
    [ -s "$ARM_DIR/fill.txt" ] && break
    sleep 0.1
  done
  echo "    backlog を埋めた: $(cat "$ARM_DIR/fill.txt")"
}
resume_a() {
  kill -CONT "$A_PID"
  kill "$FILL" 2>/dev/null
  wait "$FILL" 2>/dev/null
}

# ① ② ③ ④ の共通の流れ。$1 = ラベル、$2 = control.json の扱い（keep / missing / broken）、残りは B の env
contended() {
  local label="$1" control="$2" ino_a c ino_b
  shift 2
  arm_env "$label"
  launch_a
  ino_a=$(probe ino "$SOCK")
  check "A が固定パスで立つ（前提）" "$(holds_fixed "$A_PID")"
  stop_and_fill_a
  c=$(probe connect "$SOCK")
  echo "    埋めた後の connect: $c"
  check "生きた A の受け口へ繋ぐと ECONNREFUSED（注入が効いている）" "$( [ "$c" = ECONNREFUSED ] && echo 1)"
  case "$control" in
    missing) rm -f "${CONTROL:?}" ;;
    broken) printf 'not json\n' > "$CONTROL" ;;
  esac
  launch_b "$@" || exit 1
  ino_b=$(probe ino "$SOCK")
  RESULT_TAKEN=$( [ "$ino_b" != "$ino_a" ] && echo 1)
  RESULT_B_FIXED=$(holds_fixed "$B_PID")
  echo "    固定パスの実体: A=$ino_a → B の起動後=${ino_b}・B が固定パスで張った: ${RESULT_B_FIXED:-0}"
  show_log '#2360|セカンダリモード'
  resume_a
  sleep 3
  RESULT_A_PID=$A_PID
  RESULT_HEALTH_JSON=$(b_fixed_path)
  RESULT_HEALTH_LINE=$(b_health | grep '固定パス:')
  echo "    B の check-health: ipc.fixed_path=${RESULT_HEALTH_JSON}"
  [ -n "$RESULT_HEALTH_LINE" ] && echo "  ${RESULT_HEALTH_LINE}"
  RESULT_CLI_A=$(fixed_answers_a)
  RESULT_MCP_A=$(mcp_answers_a)
  echo "    A を再開した後の固定パス: CLI が A へ届く=${RESULT_CLI_A:-0}・MCP が A へ届く=${RESULT_MCP_A:-0}"
  RESULT_SECONDARY=$(grep -q "セカンダリモードで起動.*\[pid $B_PID\]" "$PLOG" 2>/dev/null && echo 1)
  RESULT_2360_KEEP=$(grep -q "奪わず.*#2360.*\[pid $B_PID\]" "$PLOG" 2>/dev/null && echo 1)
  RESULT_A_YIELD=$( [ "$(lines '張り直さない（#2002）.*奪わずに待つ')" -ge 1 ] && echo 1)
  # 2 つ目が先に終わる
  stop_b
  sleep 3
  RESULT_AFTER_B_INO=$(probe ino "$SOCK")
  RESULT_AFTER_B_CLI_A=$(fixed_answers_a)
  RESULT_INO_A=$ino_a
  RESULT_A_REBINDS=$(lines '張り直した（#2002）')
  echo "    B が先に終わった後: 実体=${RESULT_AFTER_B_INO}・固定パスの CLI が A へ届く=${RESULT_AFTER_B_CLI_A:-0}・A の張り直し=$RESULT_A_REBINDS 回"
  show_log '#2002'
  stop_a
}

echo "== ① ガードが効く形（control.json あり = #113 の pid のガード）"
contended guard keep
check "B は固定パスを奪わない" "$( [ -z "$RESULT_TAKEN" ] && [ -z "$RESULT_B_FIXED" ] && echo 1)"
check "B はセカンダリで起動した（#113）" "$RESULT_SECONDARY"
check "再開した A へ固定パスの CLI が届く" "$RESULT_CLI_A"
check "再開した A へ固定パスの MCP が届く" "$RESULT_MCP_A"

for arm in missing broken; do
  if [ "$arm" = missing ]; then
    echo "== ② ガードが盲目（control.json が無い）"
  else
    echo "== ③ control.json が壊れている"
  fi
  contended "$arm" "$arm"
  check "B は生きた A の受け口を奪わない（ファイルの実体が A のまま）" "$( [ -z "$RESULT_TAKEN" ] && echo 1)"
  check "B は固定パスで張っていない（lsof）" "$( [ -z "$RESULT_B_FIXED" ] && echo 1)"
  check "persist.log に B の「奪わず」の 1 行（#2360）" "$RESULT_2360_KEEP"
  check "B はセカンダリで起動した（固定パスの先客がガードになる）" "$RESULT_SECONDARY"
  check "B の check-health の ipc.fixed_path が busy と A の pid" \
    "$( [ "$RESULT_HEALTH_JSON" = "busy/$RESULT_A_PID" ] && echo 1)"
  check "B の check-health に「固定パス: … 奪わずに一時パスで立てた」の行" \
    "$(grep -q "奪わずに一時パスで立てた.*pid ${RESULT_A_PID}.*セカンダリ（#2360）" <<<"$RESULT_HEALTH_LINE" && echo 1)"
  check "再開した A へ固定パスの CLI が届く" "$RESULT_CLI_A"
  check "再開した A へ固定パスの MCP（stdio ブリッジ）が届く" "$RESULT_MCP_A"
  check "B が先に終わっても A の受け口はそのまま（実体が同じ・張り直し 0 回）" \
    "$( [ "$RESULT_AFTER_B_INO" = "$RESULT_INO_A" ] && [ "$RESULT_A_REBINDS" = 0 ] && echo 1)"
  check "B が先に終わった後も固定パスの CLI が A へ届く" "$RESULT_AFTER_B_CLI_A"
done

echo "== ④ A/B: ② に TAKO_2360_LEGACY=1（B だけ = 修正前の挙動）"
contended legacy missing TAKO_2360_LEGACY=1
check "旧挙動の B は生きた A の受け口を奪う（#2360 の症状の再現）" "$( [ -n "$RESULT_TAKEN" ] && [ -n "$RESULT_B_FIXED" ] && echo 1)"
check "奪われている間、固定パスの CLI は A へ届かない" "$( [ -z "$RESULT_CLI_A" ] && echo 1)"
check "旧挙動の B は check-health に何も残さない（修正前と同じ）" "$( [ "$RESULT_HEALTH_JSON" = null ] && echo 1)"
check "A の見張り（#2002）は奪い返さない（張り直さない・奪わずに待つ）" "$RESULT_A_YIELD"
check "B が去ると A が張り直して固定パスの CLI が A へ戻る（#2002）" \
  "$( [ "$RESULT_A_REBINDS" -ge 1 ] && [ -n "$RESULT_AFTER_B_CLI_A" ] && echo 1)"

# ⑤ ⑥ の共通の流れ。$1 = ラベル、$2 = control.json の扱い（dead / reused）
residue() {
  local label="$1" control="$2" ino_a ino_b sleeper=""
  arm_env "$label"
  launch_a
  ino_a=$(probe ino "$SOCK")
  # SIGKILL = 後始末をしない（ソケットファイルと接続情報が残る）
  kill -9 "$A_PID"
  wait "$A_PID" 2>/dev/null
  A_PID=""
  check "残骸: ファイルは残り、繋ぐと ECONNREFUSED（前提）" \
    "$( [ "$(probe ino "$SOCK")" = "$ino_a" ] && [ "$(probe connect "$SOCK")" = ECONNREFUSED ] && echo 1)"
  if [ "$control" = reused ]; then
    sleep 300 &
    sleeper=$!
    SIDE_PIDS="$SIDE_PIDS $sleeper"
    perl -MJSON::PP -0 -i -pe "my \$j = decode_json(\$_); \$j->{pid} = $sleeper; \$_ = encode_json(\$j)" "$CONTROL"
    echo "    control.json の pid を生きた別のプロセス（sleep）の pid へ書き換えた"
  fi
  launch_b || exit 1
  ino_b=$(probe ino "$SOCK")
  echo "    固定パスの実体: 残骸=$ino_a → B の起動後=$ino_b"
  show_log '#2360|セカンダリモード'
  check "B が残骸を作り直して固定パスで張る" "$( [ "$ino_b" != "$ino_a" ] && [ "$ino_b" != missing ] && [ -n "$(holds_fixed "$B_PID")" ] && echo 1)"
  check "B はプライマリ（セカンダリにならない）" "$(grep -q "セカンダリモードで起動.*\[pid $B_PID\]" "$PLOG" 2>/dev/null || echo 1)"
  check "固定パスの CLI が B へ届く" "$(via_fixed >/dev/null 2>&1 && echo 1)"
  check "persist.log に「残骸を作り直す」の 1 行（#2360）" "$(grep -q "残骸.*#2360.*\[pid $B_PID\]" "$PLOG" 2>/dev/null && echo 1)"
  local health
  health=$(b_fixed_path)
  echo "    B の check-health: ipc.fixed_path=${health}"
  check "B の check-health の ipc.fixed_path が dead（残骸を作り直した）" "$( [ "$health" = dead/null ] && echo 1)"
  stop_b
  [ -n "$sleeper" ] && kill "$sleeper" 2>/dev/null
}

echo "== ⑤ 本当の残骸（A を SIGKILL = control.json の pid が死んでいる）"
residue dead dead
echo "== ⑥ control.json の pid が別のプロセスに再利用されている + 残骸"
residue reused reused

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
