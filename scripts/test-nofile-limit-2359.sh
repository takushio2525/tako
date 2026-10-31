#!/bin/bash
# test-nofile-limit-2359.sh — GUI の fd 上限（RLIMIT_NOFILE）の引き上げと check-health の実経路テスト（Issue #2359）
#
# 直した症状: Finder / Dock から起きた tako は launchd の既定（soft 256）のまま動き、
# 約 60 ペイン（GUI は 1 ペインで fd 4 本）で上限に届くと、IPC の接続を受けてもすぐ閉じる
# （接続ごとのスレッドの try_clone が EMFILE）ので CLI / MCP が届かなくなった
# （#2002 の段 1 の注入 a2。字面は ENOTCONN / EPIPE / EINVAL・CLI は「応答が返らなかった」）。
#
# 何を確かめるか（隔離 GUI にだけ上限を掛ける = APP_BIN を「ulimit を掛けて本物を exec する包み」に替える）:
#   ① soft 256（launchd と同じ）で起こす → GUI の soft は min(hard, 10240, kern.maxfilesperproc)・
#      check-health に使用数と上限・persist.log に 1 行・ペインのシェルの ulimit -n は 256 のまま
#   ② 器（TAKO_PERSIST=1）のペインのシェルも 256（attach が起こす tmux サーバーへ起動時の値が渡る）
#   ③ A/B: TAKO_2359_LEGACY=1 は上げない（soft 256）
#   ④ soft 160 で起こして IPC へ 200 本繋いで握る（#2002 の a2）→ 旧は CLI が届かない・
#      新は通る（引き上げた上限の範囲）。握りを放せば旧も戻る。上限に届いた GUI は accept できても
#      接続ごとのスレッドの try_clone（dup）が EMFILE で落ちて接続を閉じるので、CLI の字面は
#      閉じるのと送るのとどちらが先かで変わる（送信で ENOTCONN / EINVAL・読みが EOF =
#      「応答が返らなかった」）。5 回とも通らないことを見て、出た字面は全部残す
#   ⑤ hard 500（上げられない）で 200 本握る → check-health（CLI と MCP の stdio ブリッジ）が
#      枯渇が近い warning と次の一手を出す。**握った接続 1 本は GUI の fd 2 本**（読み書きの
#      try_clone）なので 13 + 2 × 200 = 413 本（約 83%）。残りの約 87 本は check-health 自身の探針
#      （`tako --version`・環境診断）が使う分（hard 200 で 160 本握ると探針の前に上限を越えて応答ごと返らない）
#   ⑥ setrlimit の失敗を注入（TAKO_2359_INJECT=setrlimit-fail）→ 起動は続き、persist.log と
#      check-health に失敗が残る
#
# **本番の tako / 設定には一切触らない**（data / discovery / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **負荷は作らない**（上限を縮めて枯渇させる。握る接続は 200 本・データは送らない）。
#
# 使い方: bash scripts/test-nofile-limit-2359.sh
#   修正前のバイナリでも ④ を測るなら MAIN_APP_BIN=<修正前の tako-app> を並べる（CLI は今の木のもの）
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail
# check-health / `tako setup --check` は表示言語に従う（#2365）。判定は日本語の字面なので、OS の言語に
# 依らず日本語表示へ固定する（settings.json の language が system のときだけ効く）
export TAKO_LANG=ja
# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
MAIN_APP_BIN="${MAIN_APP_BIN:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$2" = 1 ]; then pass "$1"; else fail "$1"; fi; }
# ソケットのパスは sun_path（103 バイト）に収まる短い置き場へ
TMP="$(mktemp -d /tmp/tk2359-XXXX)"
APP_PID=""
HOLD_PID=""
cleanup() {
  local p
  [ -n "$HOLD_PID" ] && kill "$HOLD_PID" 2>/dev/null
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  for p in "$TMP"/*/tmux; do
    [ -f "$p" ] && tmux_socket_kill "$(cat "$p")"
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

# 上限を掛けて本物を exec する包み（上限は GUI にだけ掛かる = このスクリプトのシェルは変えない）。
# NOFILE_HARD は soft と hard の両方（上げられない機を作る）、NOFILE_SOFT は soft だけ
export REAL_APP="$APP_BIN"
cat > "$TMP/app-nofile.sh" <<'SH'
#!/bin/bash
if [ -n "${NOFILE_HARD:-}" ]; then ulimit -n "$NOFILE_HARD" || exit 97; fi
if [ -n "${NOFILE_SOFT:-}" ]; then ulimit -Sn "$NOFILE_SOFT" || exit 97; fi
exec "$REAL_APP" "$@"
SH
chmod +x "$TMP/app-nofile.sh"
APP_BIN="$TMP/app-nofile.sh"

# --- 接続を握る（perl の標準モジュールだけ。データは送らない） ------------------------------
cat > "$TMP/hold.pl" <<'PL'
use strict; use warnings; use Socket;
my ($path, $n) = @ARGV;
my (@held, %r);
for (1 .. $n) {
  socket(my $s, PF_UNIX, SOCK_STREAM, 0) or do { $r{"socket: $!"}++; next };
  if (connect($s, pack_sockaddr_un($path))) { push @held, $s; $r{ok}++ } else { $r{"$!"}++ }
}
$| = 1;
print join(" ", map { "$_=$r{$_}" } sort keys %r), "\n";
sleep 3600;
PL
hold() {
  perl "$TMP/hold.pl" "$1" "$2" > "$TMP/hold.txt" 2>&1 &
  HOLD_PID=$!
  local i
  for i in $(seq 1 100); do
    [ -s "$TMP/hold.txt" ] && break
    sleep 0.1
  done
  sleep 1
  echo "    握った接続: $(cat "$TMP/hold.txt")"
}
release() {
  [ -n "$HOLD_PID" ] && kill "$HOLD_PID" 2>/dev/null
  wait "$HOLD_PID" 2>/dev/null
  HOLD_PID=""
}

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
  export TAKO_TMUX_SOCKET="tk2359-$$-$label"
  echo "$TAKO_TMUX_SOCKET" > "$dir/tmux"
  printf '{\n  "language": "ja"\n}\n' > "$TAKO_DATA_DIR/settings.json"
  SOCK="$TAKO_DATA_DIR/tako.sock"
  PLOG="$TAKO_DATA_DIR/persist.log"
  launch_isolated_gui "$dir/app.log" "$@" || exit $?
  APP_PID=$ISOLATED_GUI_PID
  wait_isolated_gui "$dir/app.log" 300 || exit 1
}
stop_gui() { stop_isolated_gui "$APP_PID"; APP_PID=""; }
health_json() { "$TAKO_BIN" check-health --json 2>/dev/null; }
# check-health の JSON から fd_limit の値を 1 つ読む（例: soft / startup.outcome）
fd_field() {
  perl -MJSON::PP -0e '
    my $j = eval { decode_json(<STDIN>) } or do { print "unreadable"; exit };
    my $v = $j->{fd_limit};
    for my $k (split /\./, $ARGV[0]) { $v = ref $v eq "HASH" ? $v->{$k} : undef }
    print !defined $v ? "null"
      : JSON::PP::is_bool($v) ? ($v ? 1 : 0)
      : ref $v ? JSON::PP->new->canonical->encode($v) : $v;' "$1"
}
# fd_limit の warning の本文（無ければ空）
fd_warning() {
  perl -MJSON::PP -0e '
    my $j = eval { decode_json(<STDIN>) } or exit;
    binmode STDOUT, ":encoding(UTF-8)";
    print join("\n", map { $_->{message} } grep { ($_->{check} // "") eq "fd_limit" } @{ $j->{issues} // [] });'
}
# ペインを 1 本足し、そのシェルの ulimit -Sn / -Hn を読む（"soft/hard"）
pane_nofile() {
  local tab pane i out
  tab=$("$TAKO_BIN" list | perl -MJSON::PP -0e 'my $j = decode_json(<STDIN>); my $t = ($j->{tabs} // $j)->[0]; print $t->{id} // $t->{tab_id}')
  pane=$("$TAKO_BIN" split --tab "$tab" 2>/dev/null)
  [ -n "$pane" ] || { echo "split-failed"; return; }
  for i in $(seq 1 100); do
    # 1 回目と、届かなかったとき（シェルの起動前に食われた）は 10 回に 1 回送り直す
    [ $((i % 10)) = 1 ] && "$TAKO_BIN" send --pane "$pane" 'echo "NOFILE=[$(ulimit -Sn)/$(ulimit -Hn)]"' >/dev/null 2>&1
    sleep 0.3
    out=$("$TAKO_BIN" read --pane "$pane" 2>/dev/null | grep -o 'NOFILE=\[[0-9a-z]*/[0-9a-z]*\]' | tail -n 1)
    if [ -n "$out" ]; then
      out=${out#NOFILE=[}
      echo "${out%]}"
      return
    fi
  done
  echo "unread"
}
# MCP の stdio ブリッジ（`tako mcp serve`）から tako_check_health を 1 回呼び、fd_limit の warning の本文を返す
mcp_fd_warning() {
  local sock tok
  sock=$(perl -MJSON::PP -0e 'print decode_json(<>)->{socket}' "$TAKO_DISCOVERY_DIR/control.json")
  tok=$(perl -MJSON::PP -0e 'print decode_json(<>)->{token}' "$TAKO_DISCOVERY_DIR/control.json")
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2359","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_check_health","arguments":{}}}'
    sleep 10
  } | TAKO_SOCKET="$sock" TAKO_TOKEN="$tok" "$TAKO_BIN" mcp serve 2>/dev/null | perl -MJSON::PP -ne '
    my $m = eval { decode_json($_) } or next;
    next unless ($m->{id} // 0) == 2;
    binmode STDOUT, ":encoding(UTF-8)";
    my $t = $m->{result}{content}[0]{text} // ""; my $h = eval { JSON::PP->new->decode($t) } or do { print "isError: $t\n"; next };
    print "open=$h->{fd_limit}{open} soft=$h->{fd_limit}{soft}\n";
    print map { "$_->{message}\n" } grep { ($_->{check} // "") eq "fd_limit" } @{ $h->{issues} // [] };'
}
# CLI が 1 回通るか（通れば ok・通らなければ誤りの 1 行）
cli_once() {
  local out
  if out=$("$TAKO_BIN" list 2>&1 >/dev/null); then echo ok; else echo "${out:-失敗}" | tail -n 1; fi
}
# CLI を 5 回試し、出た結果を数えて 1 行にする（例: "ok=5" / "error: …=3 | error: …=2"）
cli_tries() {
  local i
  for i in 1 2 3 4 5; do cli_once; done | sort | uniq -c | awk '{n=$1; $1=""; sub(/^ /, ""); printf "%s%s=%s", (NR>1 ? " | " : ""), $0, n} END {print ""}'
}

MPP=$(sysctl -n kern.maxfilesperproc 2>/dev/null || echo 10240)
WANT=10240
[ "$MPP" -lt "$WANT" ] && WANT=$MPP

echo "== ① soft 256（launchd と同じ）で起こす"
start_gui raise NOFILE_SOFT=256
H=$(health_json)
echo "    fd_limit: open=$(fd_field open <<<"$H") soft=$(fd_field soft <<<"$H") hard=$(fd_field hard <<<"$H") startup=$(fd_field startup <<<"$H")"
check "GUI の soft が $WANT へ上がる" "$( [ "$(fd_field soft <<<"$H")" = "$WANT" ] && echo 1)"
check "起動時の値 256・結果 raised・子へは 256" "$( [ "$(fd_field startup.soft_at_launch <<<"$H")" = 256 ] && [ "$(fd_field startup.outcome <<<"$H")" = raised ] && [ "$(fd_field startup.children_soft <<<"$H")" = 256 ] && echo 1)"
check "hard は無制限のまま（触らない）" "$( [ "$(fd_field hard_unlimited <<<"$H")" = 1 ] && echo 1)"
check "使用数が出て枯渇は近くない" "$( [ "$(fd_field open <<<"$H")" -gt 0 ] 2>/dev/null && [ "$(fd_field near_exhaustion <<<"$H")" = 0 ] && echo 1)"
L=$(grep '#2359' "$PLOG")
echo "    persist.log: $L" | sed -e "s#$TMP#<TMP>#g"
check "persist.log に引き上げが 1 行" "$( [ "$(grep -c '#2359' "$PLOG")" = 1 ] && grep -q "soft 256 → $WANT に引き上げた" <<<"$L" && echo 1)"
HT=$("$TAKO_BIN" check-health 2>/dev/null | grep '^fd: ')
echo "    $HT"
check "人が読む行に使用数と引き上げが出る" "$(grep -q "^fd: [0-9]* / $WANT 本（[0-9]*%）・起動時 256 → $WANT に引き上げ（ペインのシェルには 256）" <<<"$HT" && echo 1)"
P=$(pane_nofile)
echo "    ペインのシェル（直接の PTY）の ulimit -n: $P"
check "ペインのシェルは起動時の 256 のまま（hard は無制限）" "$( [ "$P" = 256/unlimited ] && echo 1)"
stop_gui

echo "== ② 器（TAKO_PERSIST=1）のペイン"
start_gui persist NOFILE_SOFT=256 TAKO_PERSIST=1
P=$(pane_nofile)
SPID=$(tmux -L "$TAKO_TMUX_SOCKET" display -p '#{pid}' 2>/dev/null)
echo "    器のペインのシェルの ulimit -n: ${P}（tmux サーバー pid ${SPID:-?}）"
check "器のペインのシェルも 256（tmux サーバーへ起動時の値が渡る）" "$( [ "$P" = 256/unlimited ] && echo 1)"
check "GUI 自身は引き上げている" "$( [ "$(fd_field soft <<<"$(health_json)")" = "$WANT" ] && echo 1)"
stop_gui

echo "== ③ A/B: TAKO_2359_LEGACY=1"
start_gui legacy NOFILE_SOFT=256 TAKO_2359_LEGACY=1
H=$(health_json)
echo "    fd_limit: soft=$(fd_field soft <<<"$H") startup.outcome=$(fd_field startup.outcome <<<"$H")"
check "旧挙動は soft 256 のまま（legacy）" "$( [ "$(fd_field soft <<<"$H")" = 256 ] && [ "$(fd_field startup.outcome <<<"$H")" = legacy ] && echo 1)"
check "persist.log に上げない理由が 1 行" "$(grep -q 'TAKO_2359_LEGACY のため上げない' "$PLOG" && echo 1)"
stop_gui

# ④ #2002 の a2: soft 160 で起こし、200 本繋いで握る
exhaust() {
  local label="$1" expect="$2"; shift 2
  start_gui "$label" NOFILE_SOFT=160 "$@"
  hold "$SOCK" 200
  local r
  r=$(cli_tries)
  echo "    握っている間の CLI（5 回）: $r"
  if [ "$expect" = unreachable ]; then
    check "$label: 握っている間は CLI が 5 回とも届かない" "$(! grep -q 'ok=' <<<"$r" && echo 1)"
  else
    check "$label: 握っている間も CLI が 5 回とも通る" "$( [ "$r" = ok=5 ] && echo 1)"
    local h
    h=$(health_json)
    echo "    握っている間の fd_limit: open=$(fd_field open <<<"$h") soft=$(fd_field soft <<<"$h")"
    check "$label: 200 本を握っても上限の内（open > 200・soft ${WANT}）" "$( [ "$(fd_field open <<<"$h")" -gt 200 ] 2>/dev/null && [ "$(fd_field soft <<<"$h")" = "$WANT" ] && echo 1)"
  fi
  release
  sleep 1
  r=$(cli_once)
  echo "    放した後の CLI: $r"
  check "$label: 放せば CLI が通る" "$( [ "$r" = ok ] && echo 1)"
  stop_gui
}
echo "== ④ soft 160 で 200 本握る（#2002 の a2）"
exhaust a2-legacy unreachable TAKO_2359_LEGACY=1
exhaust a2-new ok
if [ -n "$MAIN_APP_BIN" ]; then
  echo "  （修正前のバイナリ）"
  REAL_APP="$MAIN_APP_BIN" exhaust a2-main unreachable
fi

echo "== ⑤ hard 500（上げられない）で 200 本握る"
start_gui capped NOFILE_HARD=500
hold "$SOCK" 200
H=$(health_json)
echo "    fd_limit: open=$(fd_field open <<<"$H") soft=$(fd_field soft <<<"$H") percent=$(fd_field percent <<<"$H") startup.outcome=$(fd_field startup.outcome <<<"$H")"
check "上げられないと記録する（hard_capped）" "$( [ "$(fd_field startup.outcome <<<"$H")" = hard_capped ] && echo 1)"
check "枯渇が近い（near_exhaustion）" "$( [ "$(fd_field near_exhaustion <<<"$H")" = 1 ] && echo 1)"
W=$(fd_warning <<<"$H")
echo "    CLI の warning: $W"
check "CLI: warning に使用数・届かなくなる理由・次の一手（ペインを閉じる）が出る" "$(grep -q '/ 500 本（[0-9]*%・残り [0-9]* 本）' <<<"$W" && grep -q '繋がっても GUI がすぐ閉じる' <<<"$W" && grep -q '使っていないペイン・タブを閉じること' <<<"$W" && grep -q '起動時に上限を引き上げていない（hard_capped）' <<<"$W" && echo 1)"
HT=$("$TAKO_BIN" check-health 2>/dev/null)
check "人が読む出力にも [warning] fd_limit の行" "$(grep -q '^\[warning\] fd_limit: ファイル記述子（fd）の使用が上限に近い' <<<"$HT" && echo 1)"
M=$(mcp_fd_warning)
echo "    MCP（stdio ブリッジ）の tako_check_health:"
echo "$M" | sed 's/^/      /'
check "MCP: 同じ warning が出る" "$(grep -q 'ファイル記述子（fd）の使用が上限に近い' <<<"$M" && echo 1)"
release
H=$(health_json)
check "放せば warning は消える" "$( [ -z "$(fd_warning <<<"$H")" ] && echo 1)"
stop_gui

echo "== ⑥ setrlimit の失敗を注入"
start_gui inject NOFILE_SOFT=256 TAKO_2359_INJECT=setrlimit-fail
H=$(health_json)
echo "    fd_limit: soft=$(fd_field soft <<<"$H") startup.outcome=$(fd_field startup.outcome <<<"$H") error=$(fd_field startup.error <<<"$H")"
check "起動は続き、失敗を記録する（failed・soft 256 のまま）" "$( [ "$(fd_field startup.outcome <<<"$H")" = failed ] && [ "$(fd_field soft <<<"$H")" = 256 ] && echo 1)"
check "persist.log に失敗の理由が 1 行" "$(grep -q "引き上げられなかった: setrlimit($WANT)" "$PLOG" && echo 1)"
W=$(fd_warning <<<"$H")
echo "    warning: $W"
check "check-health が引き上げの失敗を warning で出す" "$(grep -q '起動時にファイル記述子（fd）の上限を引き上げられなかった' <<<"$W" && echo 1)"
P=$(pane_nofile)
check "引き上げていないのでペインのシェルも 256" "$( [ "$P" = 256/unlimited ] && echo 1)"
stop_gui

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
