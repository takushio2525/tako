#!/usr/bin/env bash
# test-remote-recovery-offui-2976.sh — #2976: リモートフォルダの自動復帰の見張りが UI スレッドで ssh を待たないことの実経路テスト
#
# GUI の定期の tick（UI スレッド）の見張り `drive_remote_recovery`（#1040）は、ツリーに出ているホストごとに
# 生死の判定 `remote_fs::liveness`（ログインシェルで ssh を引き、`ssh -O check` を上限まで待つ）を UI スレッドで
# 待っていた。修正後は判定を背景の段へ出し、着地が結果だけを受けて復帰待ちを進める。番号は Issue #2976 の
# 受け入れ条件:
#   1. 返らない `ssh -O check` の間、修正後は `tako list` がすぐ返り perf.log に見張りの専有も UI ストールも
#      出ない。修正前（`TAKO_2976_LEGACY=1`）は `tako list` が待たされ、perf.log に専有と UI ストールが出る
#   2. 返る偽物で master が消えた（ControlPath のファイルを消す = `-O check` が 255）とき、修正前後とも
#      同じ間隔で繋ぎ直し、persist.log の復帰の行と `tako_remote_folder list` の connected が同じ
#   3. 修正後は同じホストの判定が重ならない（返らない判定の最中に次の tick が来ても 2 本目の
#      `ssh -O check` を起こさない）。偽の ssh が `-O check` の始まりを記録し、続く始まりが前の判定の上限
#      （`TAKO_REMOTE_TIMEOUT_SECS`）より後かで見る（2 秒 tick ごとに積み上がれば間隔が上限を割る）
#
# **本物の ssh の接続先・tailscale・本番の tako には触らない**: ssh / sftp はシェルの偽物で、偽のログイン
# シェルが `command -v ssh` / `sftp` に偽物を答える（どこへも繋がない）。data / discovery / HOME は mktemp 配下。
# GUI と CLI は砂場（一時 HOME・偽物だけが居る PATH）で動かす。落とすのは自分で起こした pid だけ。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 選んで回す: ONLY=fixed|legacy（既定は両方）。
# CI には登録しない（GUI が要る）。CI 側の担保は単体テスト（`ssh_folders::tests::issue2976_*`）と番犬
# （`crates/tako-control/tests/issue2956_setup_dispatch_offload_watchdog.rs` の `TAKO_APP_UI_LANDINGS`）。
#
# 使い方: bash scripts/test-remote-recovery-offui-2976.sh
set -uo pipefail
# 判定は日本語の字面なので日本語表示へ固定する
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_2976_LEGACY TAKO_2973_LEGACY TAKO_2961_LEGACY \
  TAKO_IPC_TIMEOUT_SECS TAKO_PERF_LOG CLAUDE_CONFIG_DIR TAKO_TMUX_BIN TAKO_TMUX_SOCKET \
  TAKO_SETUP_PROBE_TIMEOUT_SECS TAKO_REMOTE_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
# `ssh -O check` の上限の注入（秒。`mux_control_timeout` は `remote_timeout` が短ければそちら）
LIMIT=3
# 返らない判定の窓（秒）。最初の返らない判定の始まりからこの秒数までは、見張りの判定だけが `ssh -O check` を
# 起こす（繋ぎ直しの試行 `connect` も `-O check` を起こすが、試行は切断を観測して 5 秒後 = 最初の判定の
# 着地（LIMIT 秒）+ 5 秒より後にしか始まらない）
WINDOW=7

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

TMP="$(mktemp -d /tmp/tako-2976-XXXXXX)"
APP_PID=""
MCP_PID=""
FAKE_DIR="$TMP/fake"
FOLDER_HOST="fakehost2976"

# 偽物を落とす（止まる前に自分で書いた pid だけ）
kill_fakes() {
  local f
  for f in "$FAKE_DIR/pids"/*; do
    [ -f "$f" ] || continue
    kill -KILL "${f##*/}" >/dev/null 2>&1 || true
    rm -f "$f"
  done
}
cleanup() {
  rm -f "${FAKE_DIR:?}"/hang-*
  kill_fakes
  [ -n "$MCP_PID" ] && kill -KILL "$MCP_PID" >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "リモートフォルダの自動復帰の見張りの実経路テスト（#2976）"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の ssh / sftp / ログインシェル -------------------------------------------------
HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
# 偽の ssh。`-O check` は始まり（と止まった形の終わり）を checks.log へ記録する。ControlMaster の
# `-O check` / `-O exit` と張る形（`-N -f`）には ControlPath のファイルで答え、それ以外はどこへも繋がない。
# 止まる印があれば孫（sleep）を抱えて止まり、打ち切りの TERM で終わりを記録して抜ける
cat > "$TMP/stubs/ssh" <<SH
#!/bin/sh
stamp() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%d", time * 1000'; }
cp=""; op=""; master=0; prev=""
for a in "\$@"; do
  case "\$a" in
    ControlPath=*) cp="\${a#ControlPath=}"; cp="\${cp#\\"}"; cp="\${cp%\\"}" ;;
    -N) master=1 ;;
  esac
  [ "\$prev" = "-O" ] && op="\$a"
  prev="\$a"
done
case "\$op" in
  check)
    if [ -e "\$FAKE_DIR/hang-ssh-check" ]; then
      echo "start \$\$ \$(stamp)" >> "\$FAKE_DIR/checks.log"
      trap 'echo "term \$\$" >> "\$FAKE_DIR/checks.log"; echo "end \$\$ \$(stamp)" >> "\$FAKE_DIR/checks.log"; exit 255' TERM INT HUP
      /bin/sleep 600 & : > "\$FAKE_DIR/pids/\$!"; : > "\$FAKE_DIR/pids/\$\$"
      wait
      exit 255
    fi
    echo "quick \$\$ \$(stamp) \$cp" >> "\$FAKE_DIR/checks.log"
    [ -n "\$cp" ] && [ -e "\$cp" ] && exit 0
    exit 255 ;;
  exit) [ -n "\$cp" ] && rm -f "\$cp"; exit 0 ;;
esac
if [ "\$master" = 1 ]; then
  echo "master \$\$ \$(stamp)" >> "\$FAKE_DIR/checks.log"
  [ -n "\$cp" ] && : > "\$cp"
  exit 0
fi
echo "fake ssh (#2976): \$*"
exit 255
SH
# 偽の sftp（バッチの `pwd` と `ls -la` に決まった形で答える。どこへも繋がない）
cat > "$TMP/stubs/sftp" <<'SH'
#!/bin/sh
while IFS= read -r line; do
  echo "sftp> $line"
  case "$line" in
    pwd) echo "Remote working directory: /home/testuser" ;;
    "ls -la"*)
      echo 'drwxr-xr-x    3 fake     staff          96 Nov 17 10:00 .'
      echo 'drwxr-xr-x    3 fake     staff          96 Nov 17 10:00 ..'
      echo '-rw-r--r--    1 fake     staff          12 Nov 17 10:00 hello.txt' ;;
  esac
done
exit 0
SH
# 偽のログインシェル: `command -v ssh` / `sftp` には偽物を答え、聞かれた回数を数える。それ以外は /bin/sh
cat > "$TMP/stubs/isosh" <<SH
#!/bin/sh
if [ "\$1" = "-l" ]; then shift; fi
case "\$*" in
  *"command -v ssh"*) echo x >> "\$FAKE_DIR/loginsh-ssh.log"; echo "$TMP/stubs/ssh"; exit 0 ;;
  *"command -v sftp"*) echo "$TMP/stubs/sftp"; exit 0 ;;
esac
exec /bin/sh "\$@"
SH
# 実行権は空打ちの補助が付ける（素の chmod にしない = #2222 / #2228）
fake_exec_warm "$TMP/stubs/ssh" "$TMP/stubs/sftp" "$TMP/stubs/isosh" || exit 1
SANDBOX_PATH="$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
sandbox_env() {
  echo "HOME=$HOME_DIR" "SHELL=$TMP/stubs/isosh" "TAKO_ISOLATED=1" "PATH=$SANDBOX_PATH" \
    "FAKE_DIR=$FAKE_DIR" "TAKO_REMOTE_TIMEOUT_SECS=$LIMIT"
}
# 砂場で本物の claude / codex / agy を起こさない（PATH に居ない）こと
for name in claude codex agy; do
  resolved="$(env -i PATH="$SANDBOX_PATH" /bin/sh -c "command -v $name" 2>/dev/null)"
  if [ -z "$resolved" ]; then
    pass "砂場の PATH に $name が居ない（本物を起こさない）"
  else
    fail "砂場の PATH に $name が居る（${resolved}）"
    exit 1
  fi
done
for name in ssh sftp; do
  resolved="$(env -i PATH="$SANDBOX_PATH" FAKE_DIR="$FAKE_DIR" "$TMP/stubs/isosh" -l -c "command -v $name")"
  check_eq "砂場のログインシェルの $name は偽物" "$TMP/stubs/$name" "$resolved"
done
rm -f "${FAKE_DIR:?}/loginsh-ssh.log"

# 隔離の data / discovery（段ごとに分ける = 前の段の器を拾わない。#2157）
use_dirs() {
  export TAKO_DATA_DIR="$TMP/$1/data"
  export TAKO_DISCOVERY_DIR="$TMP/$1/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$1/orch"
  export TAKO_SESSIONS_FILE="$TMP/$1/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$1/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$1/workers.yaml"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
}
gui_socket() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  echo "$sock"
}

# GUI へ繋ぐ CLI を起こし、終わるか LIMIT 秒を過ぎるまで状態で待つ（パイプを組まない）。
#   run_case <名前> <LIMIT 秒> -- <引数…>
# 結果: RC・ELAPSED（秒・小数 2 桁）・HUNG（1 = 上限まで返らず、こちらで落とした）
run_case() {
  local name="$1" limit="$2" t0 pid
  shift 2
  [ "${1:-}" = "--" ] && shift
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" "$@" < /dev/null > "$TMP/out/$name.out" 2> "$TMP/out/$name.err" &
  pid=$!
  HUNG=0
  RC=""
  while kill -0 "$pid" 2>/dev/null; do
    if [ $(($(now_ms) - t0)) -ge $((limit * 1000)) ]; then
      HUNG=1
      kill -KILL "$pid" 2>/dev/null
      break
    fi
    sleep 0.02
  done
  wait "$pid" 2>/dev/null
  local rc=$?
  [ "$HUNG" = 1 ] || RC="$rc"
  ELAPSED="$(awk -v ms=$(($(now_ms) - t0)) 'BEGIN { printf "%.2f", ms / 1000 }')"
}

# MCP の tools/call を起こして LIMIT 秒まで待つ。結果: MCP_HUNG。本文は $TMP/out/<名前>.out
#   mcp_call <ツール> <引数の JSON> <名前> <LIMIT 秒>
mcp_call() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2976","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-$3.jsonl"
  local t0 limit="$4"
  t0="$(now_ms)"
  # shellcheck disable=SC2046
  env $(sandbox_env) TAKO_SOCKET="$(gui_socket)" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-$3.jsonl" > "$TMP/out/$3-raw.out" 2> "$TMP/out/$3.err" &
  MCP_PID=$!
  MCP_HUNG=0
  while kill -0 "$MCP_PID" 2>/dev/null; do
    if [ $(($(now_ms) - t0)) -ge $((limit * 1000)) ]; then
      MCP_HUNG=1
      kill -KILL "$MCP_PID" 2>/dev/null
      break
    fi
    sleep 0.05
  done
  wait "$MCP_PID" 2>/dev/null
  MCP_PID=""
  perl -MJSON::PP -ne '
    my $m = eval { JSON::PP->new->utf8->decode($_) } or next;
    next unless defined $m->{id} && $m->{id} == 2;
    binmode STDOUT, ":utf8";
    for my $c (@{ $m->{result}{content} || [] }) { print $c->{text}, "\n" if $c->{type} eq "text" }
    last;' "$TMP/out/$3-raw.out" > "$TMP/out/$3.out"
}

# JSON の欄（`a/b/0/c`）。無ければ absent・null は null・真偽は小文字
jf() {
  perl -MJSON::PP -e '
    local $/; my $t = <STDIN>;
    my $v = eval { JSON::PP->new->utf8->decode($t) };
    defined $v or do { print "absent\n"; exit };
    for my $k (grep { length } split m{/}, $ARGV[0]) {
      if (ref $v eq "HASH") { exists $v->{$k} or do { print "absent\n"; exit }; $v = $v->{$k} }
      elsif (ref $v eq "ARRAY" && $k =~ /^\d+$/) { $v = $v->[$k] }
      else { $v = undef }
      last unless defined $v;
    }
    binmode STDOUT, ":utf8";
    if (!defined $v) { print "null\n" }
    elsif (JSON::PP::is_bool($v)) { print(($v ? "true" : "false"), "\n") }
    elsif (ref $v) { print JSON::PP->new->canonical->encode($v), "\n" }
    else { print "$v\n" }' "$1"
}

start_app() {
  local name="$1"
  shift
  use_dirs "$name"
  local rc=0
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) ${@+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    unmeasured "GUI を立てられない（終了コード ${rc}。tako-vd が無い / 蓋が閉じている）: 未実測"
    return 1
  fi
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || return 1
}
stop_app() {
  rm -f "${FAKE_DIR:?}"/hang-*
  kill_fakes
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# 条件が立つまで待つ（0.1 秒刻み・上限 秒）。  wait_until <上限秒> <コマンド…>
wait_until() {
  local limit="$1" i
  shift
  for i in $(seq 1 $((limit * 10))); do
    "$@" && return 0
    sleep 0.1
  done
  return 1
}
checks_count() {
  local n
  n="$(grep -c "^$1 " "$FAKE_DIR/checks.log" 2>/dev/null)"
  echo "${n:-0}"
}
quick_seen() { [ "$(checks_count quick)" -ge "$1" ]; }
hang_seen() { [ "$(checks_count start)" -ge 1 ]; }
recovered_seen() { grep -q "リモートフォルダを自動復帰: $FOLDER_HOST" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; }

# 1 段（修正後 / 修正前）。  phase <名前> [env…]
phase() {
  local name="$1"
  shift
  echo "== $name"
  rm -f "${FAKE_DIR:?}/checks.log" "${FAKE_DIR:?}/loginsh-ssh.log"
  start_app "$name" ${@+"$@"} || return
  # 返る偽物でフォルダを開く（ControlPath のファイルが立つ = 見張りの判定は Live）
  mcp_call tako_remote_folder "{\"action\":\"open\",\"host\":\"$FOLDER_HOST\",\"path\":\"/home/testuser\",\"terminal\":false,\"focus\":false}" "$name-open" 30
  check_eq "$name: 返る偽物でフォルダを開いた（opened）" "true" "$(jf opened < "$TMP/out/$name-open.out")"
  # 見張りが 2 秒 tick で判定を起こすのを待つ（開いた後の quick が 3 本 = 3 tick 以上）
  local base
  base="$(checks_count quick)"
  if wait_until 20 quick_seen $((base + 3)); then
    pass "$name: 見張りが tick ごとに ssh -O check を起こす（開いた後に quick $(( $(checks_count quick) - base )) 本）"
  else
    fail "$name: 20 秒待っても見張りが ssh -O check を起こさない"
  fi

  # --- 2. 返る偽物で master が消えた → 繋ぎ直す ---------------------------------------
  local cpfile t_rm t_rec
  # ControlPath は偽の ssh が見張りの `-O check` の引数から記録したもの（data dir の下か、長すぎれば短い置き場 = #2855）
  cpfile="$(awk '/^quick /{p=$4} END{print p}' "$FAKE_DIR/checks.log" 2>/dev/null)"
  if [ -z "$cpfile" ] || [ ! -e "$cpfile" ]; then
    fail "$name: ControlPath のファイルが見つからない（偽の master が立っていない）"
  else
    t_rm="$(now_ms)"
    rm -f "${cpfile:?}"
    if wait_until 30 recovered_seen; then
      t_rec="$(now_ms)"
      RECOVER_SECS="$(awk -v ms=$((t_rec - t_rm)) 'BEGIN { printf "%.1f", ms / 1000 }')"
      grep -h "リモートフォルダを自動復帰: $FOLDER_HOST" "$TAKO_DATA_DIR/persist.log" | sed 's/^[^]]*\] *//' | head -1 > "$TMP/out/$name-recover.line"
      pass "$name: master が消えてから ${RECOVER_SECS} 秒で繋ぎ直した（$(cat "$TMP/out/$name-recover.line")）"
      eval "RECOVER_${name}=\$RECOVER_SECS"
    else
      fail "$name: 30 秒待っても繋ぎ直さない（persist.log に復帰の行が無い）"
    fi
    [ -e "$cpfile" ] && pass "$name: 繋ぎ直しで ControlPath が戻った" || fail "$name: 繋ぎ直しの後も ControlPath が無い"
    mcp_call tako_remote_folder '{"action":"list"}' "$name-list" 30
    check_eq "$name: 繋ぎ直しの後の list の connected" "true" \
      "$(jf tabs/0/remote_folders/0/connected < "$TMP/out/$name-list.out")"
  fi

  # --- 1 / 3. 返らない `ssh -O check` -----------------------------------------------------
  local perf="$TAKO_DATA_DIR/perf.log" off t0 t_end max=0 calls=0 blocked=0 e
  sleep 2
  # perf.log は最初の記録で作られる（記録が無ければ無い = 0 バイトから読む）
  off=0
  [ -f "$perf" ] && off="$(wc -c < "$perf" | tr -d ' ')"
  : > "$FAKE_DIR/hang-ssh-check"
  if ! wait_until 10 hang_seen; then
    fail "$name: 10 秒待っても見張りが返らない ssh -O check を起こさない"
    stop_app
    return
  fi
  t0="$(awk '/^start /{print $3; exit}' "$FAKE_DIR/checks.log")"
  t_end=$((t0 + WINDOW * 1000))
  # 窓の間 `tako list` を叩き続け、いちばん長い往復を見る
  while [ "$(now_ms)" -lt "$t_end" ]; do
    run_case "$name-list-rtt" 8 -- list
    calls=$((calls + 1))
    [ "$HUNG" = 1 ] && blocked=$((blocked + 1))
    e="${ELAPSED/./}"
    e=$((10#$e * 10))
    [ "$e" -gt "$max" ] && max="$e"
    sleep 0.2
  done
  LIST_MAX="$(awk -v ms="$max" 'BEGIN { printf "%.2f", ms / 1000 }')"
  echo "    返らない ssh -O check の窓（${WINDOW} 秒）の tako list: ${calls} 回・最長 ${LIST_MAX} 秒・打ち切り ${blocked} 回"
  # 窓の後、判定が上限で返るまで待ってから perf.log を読む
  sleep $((LIMIT + 1))
  tail -c +$((off + 1)) "$perf" 2>/dev/null > "$TMP/out/$name-perf.tail"
  local hold stall
  hold="$(perl -ne 'BEGIN { $m = 0 } $m = $1 if /メインスレッド専有: periodic_(?:prep:remote_recovery|land:remote_liveness) が (\d+)ms/ && $1 > $m; END { print "$m\n" }' "$TMP/out/$name-perf.tail")"
  stall="$(grep -c "UI ストール" "$TMP/out/$name-perf.tail" 2>/dev/null)"
  stall="${stall:-0}"
  echo "    perf.log（窓から後）: 見張りの専有の最大 ${hold}ms・UI ストール ${stall} 行"
  grep -h "periodic_prep:remote_recovery\|periodic_land:remote_liveness\|UI ストール" "$TMP/out/$name-perf.tail" | sed 's/^/      /' | head -6
  # 窓の間に始まった `ssh -O check` の始まりの間隔（窓の外の start は数えない）。判定は上限
  # （LIMIT 秒）まで返らないので、判定の最中の印が効いていれば次の start は前の start から LIMIT 秒以上
  # 後になる（効いていなければ 2 秒 tick ごとに積み上がる = 間隔が LIMIT 未満）。終わり（end）は
  # 判定の重なりの材料にしない: 打ち切りは SIGTERM を 1 回送って判定を返し、子の後始末は刈り取り係が
  # 持つ（背景のスレッドから起こした子は SIGTERM を受けられず、猶予の後の SIGKILL で trap なしに消える
  # = end が残らない。#2979）。trap が TERM を受けた本数は参考に出す
  perl -e '
    my ($t0, $t1, $gap) = @ARGV[0, 1, 2];
    my (%s, %e, %term);
    open my $f, "<", $ARGV[3] or die;
    while (<$f>) {
      my ($k, $p, $t) = split;
      $s{$p} = $t if $k eq "start";
      $e{$p} = $t if $k eq "end";
      $term{$p} = 1 if $k eq "term";
    }
    my @p = sort { $s{$a} <=> $s{$b} } grep { $s{$_} >= $t0 && $s{$_} <= $t1 } keys %s;
    my $over = 0;
    for my $i (1 .. $#p) { $over++ if $s{$p[$i]} - $s{$p[$i - 1]} < $gap; }
    printf "%d %d %d\n", scalar(@p), $over, scalar(grep { $term{$_} } @p);
    for my $i (0 .. $#p) {
      my $q = $p[$i];
      printf STDERR "      start %+d ms%s・%s\n", $s{$q} - $t0,
        ($i ? sprintf("（前から %d ms）", $s{$q} - $s{$p[$i - 1]}) : ""),
        (defined $e{$q} ? sprintf("TERM の trap で終了 %+d ms", $e{$q} - $t0)
          : $term{$q} ? "TERM を受けた" : "TERM の trap の記録なし（SIGKILL で消えたか、まだ生きている）");
    }' \
    "$t0" "$t_end" "$((LIMIT * 1000 - 150))" "$FAKE_DIR/checks.log" > "$TMP/out/$name-overlap"
  local nchecks over nterm
  read -r nchecks over nterm < "$TMP/out/$name-overlap"
  # 段ごとの記録を残す（次の段が checks.log を作り直す）
  command cp -f "$FAKE_DIR/checks.log" "$TMP/out/$name-checks.log"
  echo "    窓の間に始まった返らない ssh -O check: ${nchecks} 本・前の判定の上限（${LIMIT} 秒）の内に始まったもの ${over} 本"
  echo "    （参考）そのうち打ち切りの SIGTERM を trap で受けた本数: ${nterm}"
  case "$name" in
    fixed)
      if [ "$blocked" = 0 ] && awk -v e="$LIST_MAX" 'BEGIN { exit !(e < 0.5) }'; then
        pass "fixed: 返らない ssh -O check の間も tako list がすぐ返る（最長 ${LIST_MAX} 秒・${calls} 回）"
      else
        fail "fixed: 返らない ssh -O check の間に tako list が待たされた（最長 ${LIST_MAX} 秒・打ち切り ${blocked} 回）"
      fi
      check_eq "fixed: perf.log に見張りの専有が出ない" "0" "$hold"
      check_eq "fixed: perf.log に UI ストールが出ない" "0" "$stall"
      if [ "$over" = 0 ] && [ "$nchecks" -ge 1 ] && [ "$nchecks" -le 2 ]; then
        pass "fixed: 同じホストの判定が重ならない（${WINDOW} 秒の窓に ${nchecks} 本・どれも前の判定の上限 ${LIMIT} 秒の後 = 判定の最中は次の tick が起こさない）"
      else
        fail "fixed: 判定が重なった / 本数が合わない（${nchecks} 本・前の判定の上限の内に始まったもの ${over} 本）"
      fi
      ;;
    legacy)
      if [ "$blocked" -ge 1 ] || awk -v e="$LIST_MAX" 'BEGIN { exit !(e >= 2.0) }'; then
        pass "legacy: A/B: 返らない ssh -O check の間 tako list が待たされる（最長 ${LIST_MAX} 秒・打ち切り ${blocked} 回）"
      else
        fail "legacy: A/B: tako list が待たされない（最長 ${LIST_MAX} 秒。A/B なのに止まっていない）"
      fi
      if [ "$hold" -ge $((LIMIT * 1000 - 200)) ]; then
        pass "legacy: A/B: perf.log に見張りの専有（${hold}ms）"
      else
        fail "legacy: A/B: perf.log に見張りの専有が出ない（${hold}ms）"
      fi
      if [ "$stall" -ge 1 ]; then pass "legacy: A/B: perf.log に UI ストール（${stall} 行）"; else fail "legacy: A/B: UI ストールが出ない"; fi
      ;;
  esac
  local lsh
  lsh="$(wc -l < "$FAKE_DIR/loginsh-ssh.log" 2>/dev/null | tr -d ' ')"
  echo "    （参考）この段でログインシェルへ command -v ssh を聞いた回数: ${lsh:-0}"
  stop_app
}

RECOVER_fixed=""
RECOVER_legacy=""
want fixed && phase fixed
want legacy && phase legacy TAKO_2976_LEGACY=1

# 2. 繋ぎ直しの振る舞いが修正前後で同じ（復帰の行の本文と、かかった秒数の差が tick 2 本ぶん以内）
if [ -f "$TMP/out/fixed-recover.line" ] && [ -f "$TMP/out/legacy-recover.line" ]; then
  if cmp -s "$TMP/out/fixed-recover.line" "$TMP/out/legacy-recover.line"; then
    pass "修正前後で persist.log の復帰の行が同じ（$(cat "$TMP/out/fixed-recover.line")）"
  else
    fail "修正前後で persist.log の復帰の行が違う"
    diff "$TMP/out/fixed-recover.line" "$TMP/out/legacy-recover.line" | sed 's/^/      /'
  fi
  if awk -v a="$RECOVER_fixed" -v b="$RECOVER_legacy" 'BEGIN { d = a - b; if (d < 0) d = -d; exit !(d <= 4.0) }'; then
    pass "修正前後で繋ぎ直すまでの秒数が同じ間隔（修正後 ${RECOVER_fixed} 秒・修正前 ${RECOVER_legacy} 秒・差は tick 2 本ぶん以内）"
  else
    fail "修正前後で繋ぎ直すまでの秒数が違う（修正後 ${RECOVER_fixed} 秒・修正前 ${RECOVER_legacy} 秒）"
  fi
elif [ -z "$ONLY" ]; then
  unmeasured "修正前後の繋ぎ直しの突き合わせ（片方の段が回っていない）"
fi

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
