#!/bin/bash
# test-tmux-server-nofile-2367.sh — 器の tmux サーバーの fd 上限（RLIMIT_NOFILE）の実経路テスト（Issue #2367）
#
# 直した症状: GUI は起動時に fd の上限の soft を引き上げるが（#2359）、ペインの PTY の子へは起動時の soft を
# 戻すので、attach クライアントが起こす器の tmux サーバーも起動時の値（Finder 起動なら 256）で動いた。
# サーバーは器の 1 ペインで fd を 3 本使い（tmux 3.6b は自分の上限を変えない）、上限に届くと
# 新しいペインは `create window failed: fork failed: Too many open files` で作れず、器への問い合わせも
# すべて `server exited unexpectedly` で返らなくなる。
#
# 何を確かめるか（隔離 GUI にだけ上限を掛ける = APP_BIN を「ulimit を掛けて本物を exec する包み」に替える。
# #2359 と同じ作法。枯渇は**器のサーバーへ detached の window（sleep）を足して**作る = 負荷は作らない）:
#   ① 修正前（A/B TAKO_2367_LEGACY=1・soft 64・器あり）: 器のサーバーも soft 64。window で上限まで埋めると
#      tako の新しいペインが作れない（画面の字面・タブとペインの数・既存のペインが生きているか）
#   ② 修正後（同じ soft 64）: 器のサーバーの soft は GUI の引き上げた値・器のペインのシェルは 64 のまま
#      （$0 / SHLVL / SHELL も修正前と同じ）・①より多く埋めても新しいペインが作れる・
#      persist.log に 1 行・check-health の fd_limit.container（CLI の人が読む行も）
#   ③ 境界: soft 64 の器のサーバーが先に起きている所へ attach → 上がらない（limit=predates）・
#      80% まで埋めると warning と次の一手（CLI と MCP の stdio ブリッジ）
#   ④ 境界: hard 256（上げられない）→ 段を通さず、器のサーバーもシェルも 256（limit=launch_value）
#
# **本番の tako / 設定 / 本番の tmux（-L tako）には一切触らない**（data / discovery / HOME は mktemp 配下、
# tmux は起動ごとの専用ソケット、落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の
# 仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-tmux-server-nofile-2367.sh
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
# ソケットのパスは sun_path（103 バイト）に収まる短い置き場へ
TMP="$(mktemp -d /tmp/tk2367-XXXX)"
APP_PID=""
cleanup() {
  local p name sockdir
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  # tmux は kill-server の後もソケットファイルを残すことがあるので、自分の名前のものは消す
  sockdir="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
  for p in "$TMP"/*/tmux; do
    [ -f "$p" ] || continue
    name=$(cat "$p")
    tmux -L "$name" kill-server >/dev/null 2>&1
    rm -f "${sockdir:?}/${name:?}" "${sockdir:?}/${name:?}="
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

# 上限を掛けて本物を exec する包み（上限は GUI にだけ掛かる = このスクリプトのシェルは変えない）
export REAL_APP="$APP_BIN"
cat > "$TMP/app-nofile.sh" <<'SH'
#!/bin/bash
if [ -n "${NOFILE_HARD:-}" ]; then ulimit -n "$NOFILE_HARD" || exit 97; fi
if [ -n "${NOFILE_SOFT:-}" ]; then ulimit -Sn "$NOFILE_SOFT" || exit 97; fi
exec "$REAL_APP" "$@"
SH
chmod +x "$TMP/app-nofile.sh"
APP_BIN="$TMP/app-nofile.sh"

# --- 起動ごとに data / discovery / tmux を分ける（2 回目が前の器を拾わない） --------------
prepare_dirs() {
  local label="$1"
  LABEL="$label"
  local dir="$TMP/$label"
  mkdir -p "$dir/d" "$dir/disc" "$dir/orch"
  export TAKO_DATA_DIR="$dir/d"
  export TAKO_DISCOVERY_DIR="$dir/disc"
  export TAKO_ORCHESTRATOR_DIR="$dir/orch"
  export TAKO_SESSIONS_FILE="$dir/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$dir/panelogs"
  export TAKO_WORKERS_FILE="$dir/workers.yaml"
  export TAKO_TMUX_SOCKET="tk2367-$$-$label"
  echo "$TAKO_TMUX_SOCKET" > "$dir/tmux"
  printf '{\n  "language": "ja"\n}\n' > "$TAKO_DATA_DIR/settings.json"
  PLOG="$TAKO_DATA_DIR/persist.log"
  GUI_DIR="$dir"
}
start_gui() {
  launch_isolated_gui "$GUI_DIR/app.log" "$@" || exit $?
  APP_PID=$ISOLATED_GUI_PID
  wait_isolated_gui "$GUI_DIR/app.log" 300 || exit 1
}
stop_gui() { stop_isolated_gui "$APP_PID"; APP_PID=""; }
health_json() { "$TAKO_BIN" check-health --json 2>/dev/null; }
# check-health の JSON から fd_limit の値を 1 つ読む（例: soft / container.limit）
fd_field() {
  perl -MJSON::PP -0e '
    binmode STDOUT, ":encoding(UTF-8)";
    my $j = eval { decode_json(<STDIN>) } or do { print "unreadable"; exit };
    my $v = $j->{fd_limit};
    for my $k (split /\./, $ARGV[0]) { $v = ref $v eq "HASH" ? $v->{$k} : undef }
    print !defined $v ? "null"
      : JSON::PP::is_bool($v) ? ($v ? 1 : 0)
      : ref $v ? JSON::PP->new->canonical->encode($v) : $v;' "$1"
}
# fd_limit の warning の本文（器のサーバーのものだけ。無ければ空）
container_warning() {
  perl -MJSON::PP -0e '
    my $j = eval { decode_json(<STDIN>) } or exit;
    binmode STDOUT, ":encoding(UTF-8)";
    print join("\n", map { $_->{message} } grep { ($_->{check} // "") eq "fd_limit" && $_->{message} =~ /tmux/ } @{ $j->{issues} // [] });'
}
# 器のサーバー（tmux の中から読む = run-shell の子はサーバーの上限を受け継ぐ）
server_soft() { tmux -L "$TAKO_TMUX_SOCKET" run-shell 'ulimit -Sn' 2>/dev/null; }
server_pid() { tmux -L "$TAKO_TMUX_SOCKET" display -p '#{pid}' 2>/dev/null; }
server_fds() { lsof -p "$1" 2>/dev/null | awk 'NR > 1 && $4 ~ /^[0-9]/' | wc -l | tr -d ' '; }
# 器のサーバーへ detached の window（sleep）を足す。出力: "<足せた本数> <最後の誤り>"
add_windows() {
  local i err
  for i in $(seq 1 "$1"); do
    if ! err=$(tmux -L "$TAKO_TMUX_SOCKET" new-window -d 'sleep 600' 2>&1); then
      echo "$((i - 1)) $err"
      return
    fi
  done
  echo "$1"
}
# 新しいタブ（= 器の新しいペイン）を開き、ペイン ID を返す
new_pane() {
  "$TAKO_BIN" tab new 2>/dev/null | perl -MJSON::PP -0e 'my $j = eval { decode_json(<STDIN>) } or exit; print $j->{pane} // ""'
}
pane_count() { "$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -0e 'my $j = eval { decode_json(<STDIN>) } or do { print "?"; exit }; my $n = 0; $n += scalar @{ $_->{panes} // [] } for @{ $j->{tabs} // $j }; print $n'; }
# ペインのシェルへ 1 行打ち、その出力を読む（"<中身>" / unread）。出力の印は呼ぶたびに変え、
# 打った行（エコー）には印が現れない形にする（`"P<n>""=[…]"` と打つ → 出力だけが `P<n>=[…]`）。
# 呼び出しは $(…) のサブシェルなので、数はファイルに持つ（変数だと親へ戻らず毎回同じ印になる）
pane_probe() {
  local pane="$1" body="$2" i out tag n
  n=$(( $(cat "$TMP/probe-seq" 2>/dev/null || echo 0) + 1 ))
  echo "$n" > "$TMP/probe-seq"
  tag="P${n}x$$"
  for i in $(seq 1 100); do
    [ $((i % 10)) = 1 ] && "$TAKO_BIN" send --pane "$pane" "echo \"${tag}\"\"=[${body}]\"" >/dev/null 2>&1
    sleep 0.3
    out=$("$TAKO_BIN" read --pane "$pane" 2>/dev/null | grep -o "${tag}=\[[^]]*\]" | tail -n 1)
    if [ -n "$out" ]; then
      out=${out#"${tag}=["}
      echo "${out%]}"
      return
    fi
  done
  echo "unread"
}
NOFILE_CMD='$(ulimit -Sn)/$(ulimit -Hn)'
SHELL_CMD='$0/$SHLVL/${SHELL##*/}/$([[ -o login ]] && echo login)'
# 起動ごとの persist.log をスクリプトの後片付けより後まで残す（OUT_DIR を渡したときだけ）
keep_log() { [ -n "${OUT_DIR:-}" ] && [ -f "$PLOG" ] && mask < "$PLOG" > "$OUT_DIR/persist-$1.log"; }
# MCP の stdio ブリッジ（`tako mcp serve`）から tako_check_health を 1 回呼び、器のサーバーの値と warning を返す
mcp_container() {
  local sock tok
  sock=$(perl -MJSON::PP -0e 'print decode_json(<>)->{socket}' "$TAKO_DISCOVERY_DIR/control.json")
  tok=$(perl -MJSON::PP -0e 'print decode_json(<>)->{token}' "$TAKO_DISCOVERY_DIR/control.json")
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2367","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_check_health","arguments":{}}}'
    sleep 10
  } | TAKO_SOCKET="$sock" TAKO_TOKEN="$tok" "$TAKO_BIN" mcp serve 2>/dev/null | perl -MJSON::PP -ne '
    my $m = eval { decode_json($_) } or next;
    next unless ($m->{id} // 0) == 2;
    binmode STDOUT, ":encoding(UTF-8)";
    my $t = $m->{result}{content}[0]{text} // ""; my $h = eval { JSON::PP->new->decode($t) } or do { print "isError: $t\n"; next };
    my $c = $h->{fd_limit}{container} // {};
    print "state=$c->{state} open=$c->{open} soft=$c->{soft} limit=$c->{limit}\n";
    print map { "$_->{message}\n" } grep { ($_->{check} // "") eq "fd_limit" && $_->{message} =~ /tmux/ } @{ $h->{issues} // [] };'
}
mask() { sed -e "s#$TMP#<TMP>#g" -e "s#$REPO_ROOT#<REPO>#g"; }

MPP=$(sysctl -n kern.maxfilesperproc 2>/dev/null || echo 10240)
WANT=10240
[ "$MPP" -lt "$WANT" ] && WANT=$MPP
SOFT=64

echo "== ① 修正前（TAKO_2367_LEGACY=1・soft ${SOFT}・器あり）"
prepare_dirs legacy
start_gui NOFILE_SOFT=$SOFT TAKO_PERSIST=1 TAKO_2367_LEGACY=1
FIRST=$("$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -0e 'my $j = decode_json(<STDIN>); my $t = ($j->{tabs} // $j)->[0]; print $t->{panes}[0]{id} // $t->{panes}[0]{pane_id}')
OLD_NOFILE=$(pane_probe "$FIRST" "$NOFILE_CMD")
OLD_SHELL=$(pane_probe "$FIRST" "$SHELL_CMD")
S=$(server_soft)
echo "    器のサーバーの soft（tmux の中の run-shell）: ${S}・器のペインのシェル: ${OLD_NOFILE}（${OLD_SHELL}）"
check "修正前は器のサーバーも起動時の soft $SOFT" "$( [ "$S" = "$SOFT" ] && echo 1)"
check "persist.log に legacy の 1 行" "$(grep -q "legacy: TAKO_2367_LEGACY のため.*#2367" "$PLOG" && echo 1)"
R=$(add_windows 200)
OLD_FILLED=${R%% *}
echo "    window を足した結果: $R"
check "修正前は起動時の値で上限に届いて window が作れない（fork failed: Too many open files）" "$(grep -q 'fork failed: Too many open files' <<<"$R" && [ "$OLD_FILLED" -lt 200 ] && echo 1)"
SPID=$(server_pid)
echo "    器のサーバー: pid ${SPID:-?}・lsof の fd ${SPID:+$(server_fds "$SPID")} 本"
BEFORE=$(pane_count)
LOG_AT=$(wc -l < "$PLOG" | tr -d ' ')
P=$(new_pane)
sleep 2
SCREEN=$( [ -n "$P" ] && "$TAKO_BIN" read --pane "$P" 2>&1 | tr -s '\n' | tail -n 5)
AFTER=$(pane_count)
echo "    新しいタブのペイン: ${P:-（作れない）}・ペイン数 $BEFORE → ${AFTER}・画面: $(echo "$SCREEN" | tr '\n' ' ' | mask)"
echo "    その間の persist.log:"
tail -n +"$((LOG_AT + 1))" "$PLOG" | mask | sed 's/^/      /'
check "修正前は新しいペインが作れない（attach クライアントが fork failed を出して終わる / ペインが残らない）" "$( { grep -qE "fork failed|server exited unexpectedly" <<<"$SCREEN" || [ "$AFTER" = "$BEFORE" ]; } && echo 1)"
ALIVE=$(pane_probe "$FIRST" "$NOFILE_CMD")
check "既存のペインは生きている" "$( [ "$ALIVE" = "$OLD_NOFILE" ] && echo 1)"
H=$(health_json)
echo "    check-health の container: state=$(fd_field container.state <<<"$H") open=$(fd_field container.open <<<"$H") soft=$(fd_field container.soft <<<"$H") limit=$(fd_field container.limit <<<"$H") reason=$(fd_field container.reason <<<"$H")"
W=$(container_warning <<<"$H")
echo "    warning: $W"
check "check-health が器のサーバーの枯渇（か届かない）を申告する" "$(grep -qE '器の tmux サーバー.*(上限に近い|問い合わせが届かない)' <<<"$W" && echo 1)"
keep_log "$LABEL"
stop_gui

echo "== ② 修正後（同じ soft ${SOFT}・器あり）"
prepare_dirs raised
start_gui NOFILE_SOFT=$SOFT TAKO_PERSIST=1
FIRST=$("$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -0e 'my $j = decode_json(<STDIN>); my $t = ($j->{tabs} // $j)->[0]; print $t->{panes}[0]{id} // $t->{panes}[0]{pane_id}')
NEW_NOFILE=$(pane_probe "$FIRST" "$NOFILE_CMD")
NEW_SHELL=$(pane_probe "$FIRST" "$SHELL_CMD")
S=$(server_soft)
SPID=$(server_pid)
echo "    器のサーバーの soft（tmux の中の run-shell）: ${S}（pid ${SPID:-?}・ps: $(ps -o pid=,comm= -p "${SPID:-0}" 2>/dev/null | mask)）"
echo "    器のペインのシェル: ${NEW_NOFILE}（${NEW_SHELL}）・修正前: ${OLD_NOFILE}（${OLD_SHELL}）"
check "器のサーバーの soft が GUI の引き上げた値 $WANT" "$( [ "$S" = "$WANT" ] && echo 1)"
check "器のペインのシェルの ulimit -n は修正前と同じ（${OLD_NOFILE}）" "$( [ "$NEW_NOFILE" = "$OLD_NOFILE" ] && [ "$NEW_NOFILE" = "$SOFT/unlimited" ] && echo 1)"
check "器のペインのシェルの \$0 / SHLVL / SHELL / ログインも修正前と同じ（${OLD_SHELL}）" "$( [ "$NEW_SHELL" = "$OLD_SHELL" ] && echo 1)"
L=$(grep '#2367' "$PLOG")
echo "    persist.log: $L" | mask
check "persist.log に raised の 1 行（器のペインのシェルには ${SOFT}）" "$( [ "$(grep -c '#2367' "$PLOG")" = 1 ] && grep -q "soft $WANT.*raised: GUI が引き上げた値を受け継いだ。器のペインのシェルには $SOFT を渡す" <<<"$L" && echo 1)"
R=$(add_windows $((OLD_FILLED + 20)))
echo "    window を $((OLD_FILLED + 20)) 本（修正前に届いた本数 + 20）足した結果: $R"
check "修正後は同じ注入を越えても window が作れる" "$( [ "$R" = "$((OLD_FILLED + 20))" ] && echo 1)"
P=$(new_pane)
NP=$( [ -n "$P" ] && pane_probe "$P" "$NOFILE_CMD")
echo "    新しいタブのペイン: ${P:-（作れない）}・そのシェル: ${NP:-?}"
check "修正後は新しいペインが作れ、そのシェルも $SOFT" "$( [ "$NP" = "$SOFT/unlimited" ] && echo 1)"
H=$(health_json)
echo "    check-health の container: state=$(fd_field container.state <<<"$H") open=$(fd_field container.open <<<"$H") soft=$(fd_field container.soft <<<"$H") limit=$(fd_field container.limit <<<"$H") children_soft=$(fd_field container.children_soft <<<"$H")"
check "check-health の container に使用数・上限・raised・器のシェルへの値" "$( [ "$(fd_field container.state <<<"$H")" = running ] && [ "$(fd_field container.soft <<<"$H")" = "$WANT" ] && [ "$(fd_field container.limit <<<"$H")" = raised ] && [ "$(fd_field container.children_soft <<<"$H")" = "$SOFT" ] && [ "$(fd_field container.open <<<"$H")" -gt "$OLD_FILLED" ] 2>/dev/null && echo 1)"
check "引き上げた器のサーバーには warning が出ない" "$( [ -z "$(container_warning <<<"$H")" ] && echo 1)"
HT=$("$TAKO_BIN" check-health 2>/dev/null | grep '^器の tmux サーバー: ')
echo "    $HT"
check "人が読む行に器のサーバーの使用数と決まり方" "$(grep -q "^器の tmux サーバー: [0-9]* / $WANT 本（[0-9]*%）・引き上げた値を受け継いだ（器のペインのシェルには ${SOFT}）" <<<"$HT" && echo 1)"
keep_log "$LABEL"
stop_gui

echo "== ③ 境界: soft $SOFT の器のサーバーが先に起きている所へ attach"
prepare_dirs predates
# 本番の env を持たない素の env で、soft を絞ったサーバーを先に起こす（tako の conf は GUI が後から当てる）
env -i HOME="$HOME" PATH="/usr/bin:/bin:/opt/homebrew/bin" SHELL=/bin/zsh TERM=xterm-256color \
  /bin/bash -c "ulimit -Sn $SOFT && exec tmux -L '$TAKO_TMUX_SOCKET' -f /dev/null new-session -d -s pre 'sleep 600'"
start_gui NOFILE_SOFT=256 TAKO_PERSIST=1
S=$(server_soft)
echo "    器のサーバーの soft: ${S}（先に起きていたサーバー）"
check "先に起きていたサーバーの上限は変わらない（${SOFT}）" "$( [ "$S" = "$SOFT" ] && echo 1)"
L=$(grep '#2367' "$PLOG")
echo "    persist.log: $L" | mask
check "persist.log に predates（上がるのは次にサーバーが起きたとき）" "$(grep -q "predates: 引き上げより前に起きたサーバーなので起動時の値のまま。上がるのは次にサーバーが起きたとき" <<<"$L" && echo 1)"
H=$(health_json)
OPEN=$(fd_field container.open <<<"$H")
# 80% まで埋める（52 本 = 64 の 81%）。window 1 本で fd 1 本
NEED=$(( (SOFT * 80 + 99) / 100 + 1 - OPEN ))
R=$(add_windows "$NEED")
H=$(health_json)
echo "    window を $NEED 本足した: $R → open=$(fd_field container.open <<<"$H") / soft=$(fd_field container.soft <<<"$H")（$(fd_field container.percent <<<"$H")%）limit=$(fd_field container.limit <<<"$H")"
check "80% で near_exhaustion・limit=predates" "$( [ "$(fd_field container.near_exhaustion <<<"$H")" = 1 ] && [ "$(fd_field container.limit <<<"$H")" = predates ] && echo 1)"
W=$(container_warning <<<"$H")
echo "    CLI の warning: $W"
check "CLI: warning に使用数・作れなくなる理由・次の一手（ペインを閉じる・次にサーバーが起きたとき上がる）" "$(grep -q "/ $SOFT 本（[0-9]*%・残り [0-9]* 本）" <<<"$W" && grep -q 'create window failed: fork failed: Too many open files' <<<"$W" && grep -q '使っていないペインを閉じること（器の 1 ペインで約 3 本）' <<<"$W" && grep -q '上がるのは次にサーバーが起きたとき' <<<"$W" && echo 1)"
HT=$("$TAKO_BIN" check-health 2>/dev/null)
check "人が読む出力にも [warning] fd_limit の行と器のサーバーの行" "$(grep -q '^\[warning\] fd_limit: 器の tmux サーバー' <<<"$HT" && grep -q '^器の tmux サーバー: .*引き上げより前に起きたので起動時のまま' <<<"$HT" && echo 1)"
M=$(mcp_container)
echo "    MCP（stdio ブリッジ）の tako_check_health:"
echo "$M" | sed 's/^/      /'
check "MCP: 同じ値と warning" "$(grep -q "state=running .* soft=$SOFT limit=predates" <<<"$M" && grep -q '器の tmux サーバー.*上限に近い' <<<"$M" && echo 1)"
keep_log "$LABEL"
stop_gui

echo "== ④ 境界: hard 256（GUI が上げられない）"
prepare_dirs capped
start_gui NOFILE_HARD=256 TAKO_PERSIST=1
FIRST=$("$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -0e 'my $j = decode_json(<STDIN>); my $t = ($j->{tabs} // $j)->[0]; print $t->{panes}[0]{id} // $t->{panes}[0]{pane_id}')
CN=$(pane_probe "$FIRST" "$NOFILE_CMD")
S=$(server_soft)
H=$(health_json)
echo "    GUI: startup=$(fd_field startup.outcome <<<"$H")・器のサーバーの soft: ${S}・器のペインのシェル: ${CN}・limit=$(fd_field container.limit <<<"$H")"
check "上げられない機では器のサーバーもシェルも 256（段を通さない = limit=launch_value）" "$( [ "$(fd_field startup.outcome <<<"$H")" = hard_capped ] && [ "$S" = 256 ] && [ "$CN" = 256/256 ] && [ "$(fd_field container.limit <<<"$H")" = launch_value ] && echo 1)"
keep_log "$LABEL"
stop_gui

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
