#!/usr/bin/env bash
# test-remote-start-while-starting-2613.sh — daemon を起こしている途中に打った start / stop の実経路テスト（#2613）
#
# 隔離した state / data ディレクトリ・偽の tailscale CLI（TAKO_TAILSCALE_BIN）で
# **実 tako-app（隔離 GUI）+ 実 daemon** を走らせ、#2608 の印（`<state_dir>/tako-remote.starting`）が
# 効いている間に `tako remote start`（CLI / MCP）を打つ。区間は偽の tailscale に待ちを入れて伸ばす:
# 旧版が終了時に serve を外す `serve --https=443 off`（入れ替えの停止の区間）と、新版が張る `serve --bg`
# （待ち受けを開いてから PID ファイルを書くまでの区間）。
#   ① GUI 起動時の自動復帰（#1485）が旧版を入れ替える（#2518）途中に CLI で start を打つ:
#      2 本目の daemon を起こさず断り、自動復帰の入れ替えは成る（daemon は 1 本だけ）。
#      修正前は 2 本目も旧版を止めに行き、daemon が 2 本立つ（PID ファイルに載らない 1 本が残る）
#   ② CLI の start が起こしている途中（serve を張る待ち）に 2 本目の start を打つ:
#      断り、印を消さない（区間の残りの status は starting のまま = stop を案内しない）。
#      修正前は 2 本目が印を上書きして消すので、残りの区間の status が #2455 の stop の案内へ戻る
#   ③ 断りの文（印を置いて決定的に作る）: 日本語・英語（日本語が混ざらない）・MCP（英語表示の GUI）。
#      A/B（TAKO_2613_LEGACY=1）は修正前の挙動 = 印があっても起こす
#   ④ 陽性対照: 印が効いていないとき（印が無い・書き手が死んでいる・pid の再利用・期限切れ）は起こせる
#   ⑤ 入れ替えの途中の stop / --force は断り（止めても進行中の起動が新版を起こし直す）、「起動していた」を
#      消さない。終わった後の stop は止める（陽性対照）。② の起動の途中の stop も同じく断る
# を実測する。修正前のバイナリ（TAKO_BIN / APP_BIN を差し替える）では ①〜③ と ⑤ が落ちる。
#
# **本番の tailscale / serve 設定 / remote デーモン / GUI には一切触らない**
# （tailscale の呼び出しは全部この偽物へ向く。state・data・発見の置き場は mktemp 配下。
# 落とすのは自分で起こした GUI と、この置き場のバイナリで動いている daemon だけ）。
# 窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-remote-start-while-starting-2613.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。
#   区間の長さは TAKO_2613_SLOW_SECS（serve を張る待ち。既定 6）/ TAKO_2613_SLOW_OFF_SECS（旧版が serve を
#   外す待ち。既定 4）
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2608_LEGACY TAKO_2613_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_ne() {
  if [ "$2" != "$3" ]; then pass "$1（${3}）"; else fail "$1（'${2}' 以外のはずが '${3}'）"; fi
}
check_ge() {
  if [ "$3" -ge "$2" ]; then pass "$1（${3} 回）"; else fail "$1（${2} 回以上のはずが ${3} 回）"; fi
}

SLOW="${TAKO_2613_SLOW_SECS:-6}"
SLOW_OFF="${TAKO_2613_SLOW_OFF_SECS:-4}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2613-XXXXXX)"
TAG="$(basename "$TMP")"
TMUX_SOCKET="tako-iso-2613-$$"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

STATE="$TMP/state"
BIN_DIR="$TMP/bin"
mkdir -p "$STATE" "$TMP/data" "$TMP/disc" "$BIN_DIR" "$TMP/ctl"
# 隔離モードの serve_binary() は tako-app と同じディレクトリの tako を選ぶので 2 つを並べる。
# 検査に使う CLI は差し替えの影響を受けない複製（この CLI から起こす daemon もこの複製で動く）
command cp -f "$APP_BIN" "$BIN_DIR/tako-app"
command cp -f "$TAKO_BIN" "$BIN_DIR/tako"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
APP_BIN="$BIN_DIR/tako-app"
CLI="$TMP/ctl/tako"

# この置き場のバイナリで動いている daemon の pid（置き場の名前はこの実行に固有）
live_daemons() {
  ps -axo pid=,command= | awk -v tag="$TAG" 'index($0, tag) && index($0, " remote serve") { print $1 }'
}
count_daemons() { live_daemons | grep -c . | tr -dc '0-9'; }
stop_all_daemons() {
  "$CLI" remote stop >/dev/null 2>&1 || true
  local p
  for p in $(live_daemons); do
    kill "$p" 2>/dev/null || true
  done
  local i=0
  while [ -n "$(live_daemons)" ] && [ "$i" -lt 50 ]; do
    sleep 0.1
    i=$((i + 1))
  done
  rm -f "${STATE:?}/tako-remote.pid" "${STATE:?}/tako-remote.port" "${STATE:?}/tako-remote.starting"
}
cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  stop_all_daemons
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# --- 偽 tailscale CLI（serve 設定は $TMP/serve.json）---------------------------
# `serve --bg` は $TMP/slow 秒、`serve --https=443 off` は $TMP/slow_off 秒待つ（区間を伸ばす注入）
FAKE_TS="$TMP/fake-tailscale"
echo '{}' > "$TMP/serve.json"
echo 0 > "$TMP/slow"
echo 0 > "$TMP/slow_off"
cat > "$FAKE_TS" <<'FAKE'
#!/usr/bin/env bash
set -u
STATE="$(dirname "$0")"
while [ $# -gt 0 ]; do
  case "$1" in
    --socket) shift 2 ;;
    *) break ;;
  esac
done
NODE="fake-2613.tailfake.ts.net"
SERVE="$STATE/serve.json"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  serve)
    shift
    case "${1:-}" in
      status) cat "$SERVE"; exit 0 ;;
      --bg)
        sleep "$(cat "$STATE/slow" 2>/dev/null || echo 0)"
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "${3:-}" > "$SERVE"
        exit 0 ;;
      --https=443)
        if [ "${2:-}" = "off" ]; then
          sleep "$(cat "$STATE/slow_off" 2>/dev/null || echo 0)"
          echo '{}' > "$SERVE"
          exit 0
        fi
        exit 1 ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$FAKE_TS"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_REMOTE_STATE_DIR="$STATE"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_TAILSCALE_BIN="$FAKE_TS"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# 断りの文は表示言語に従うので、日本語の段は日本語を明示する（英語の段は TAKO_LANG=en を並べる）
export TAKO_LANG=ja
# 自動復帰の 1 回目の前の待ち（既定 2 秒）を縮める
export TAKO_1485_BACKOFF_SECS=1
case "$STATE" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $STATE"; exit 1 ;;
esac

MARKER="$STATE/tako-remote.starting"
pid_now() { head -1 "$STATE/tako-remote.pid" 2>/dev/null | tr -dc '0-9'; }
field() { jq -r "$2 | if . == null then \"null\" else tostring end" "$1" 2>/dev/null; }
# 日本語（ASCII の外のバイト）を含む行の数
non_ascii_lines() { perl -ne '$n++ if /[^\x00-\x7f]/; END { print $n + 0 }' "$1"; }
STOP_GUIDED='(.warnings // []) | any(test("tako remote stop"))'

# status が jq の条件を満たすまで読む（$1 = 条件・$2 = 上限の秒・$3 = 最後の読みの書き先）。満たせば 0
wait_status() {
  local cond="$1" budget="$2" out="$3" start
  start=$(date +%s)
  while [ $(($(date +%s) - start)) -lt "$budget" ]; do
    "$CLI" remote status >"$out" 2>/dev/null
    if jq -e "$cond" "$out" >/dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  return 1
}

# 旧版の daemon を起こし、実行ファイルを同じパスのまま別のファイルへ差し替える（install の形。#2518）
start_stale_daemon() {
  echo 0 > "$TMP/slow"
  echo 0 > "$TMP/slow_off"
  if ! "$BIN_DIR/tako" remote start >"$TMP/stale.json" 2>"$TMP/stale.err"; then
    echo "daemon を起動できなかった:"; cat "$TMP/stale.err"; exit 1
  fi
  command cp -f "$TAKO_BIN" "$BIN_DIR/tako.new"
  command mv -f "$BIN_DIR/tako.new" "$BIN_DIR/tako"
}

# 印を置く（$1 = 時刻・$2 = 書き手の pid・$3 = 入れ替えなら旧版の pid）
write_marker() {
  if [ -n "${3:-}" ]; then
    printf '{"at":%s,"pid":%s,"via":"autostart","previous_pid":%s}\n' "$1" "$2" "$3" > "$MARKER"
  else
    printf '{"at":%s,"pid":%s,"via":"autostart"}\n' "$1" "$2" > "$MARKER"
  fi
}

# MCP の stdio ブリッジで隔離 GUI のツールを 1 回呼ぶ（$1 = ツール・$2 = 引数の JSON・$3 = 結果の書き先）
mcp_call() {
  local ctl="$TMP/disc/control.json"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2613","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$(jq -r .socket "$ctl")" TAKO_TOKEN="$(jq -r .token "$ctl")" "$CLI" mcp serve \
      >"$3.raw" 2>"$3.err"
  jq -c 'select(.id == 2) | .result' "$3.raw" > "$3"
}

echo
echo "== ① 自動復帰が旧版を入れ替える途中に CLI で tako remote start =="
start_stale_daemon
PID1="$(pid_now)"
"$CLI" remote status >"$TMP/1-before.json" 2>/dev/null
check_eq "差し替え後の daemon は旧版" "replaced" "$(field "$TMP/1-before.json" .serve_generation.state)"
echo "$SLOW_OFF" > "$TMP/slow_off"
echo "$SLOW" > "$TMP/slow"
launch_isolated_gui "$TMP/app1.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if wait_status '.starting.state == "replacing"' 40 "$TMP/1-window.json"; then
  pass "入れ替えの途中（印が replacing）を捉えた（旧版の PID $(field "$TMP/1-window.json" .starting.previous_pid)）"
else
  fail "入れ替えの途中を捉えられなかった"
fi
# 検査用の複製（GUI と別のパス）からの start と、GUI と同じパスの tako からの start（本番の形 =
# CLI も GUI も /Applications の tako を serve に解決する）を続けて打つ
"$CLI" remote start >"$TMP/1-other.out" 2>"$TMP/1-other.err"
S1_OTHER_RC=$?
echo "  （区間内の start（別のパスの CLI）: 終了コード ${S1_OTHER_RC}・stderr: $(head -c 600 "$TMP/1-other.err" | LC_ALL=C tr '\n' ' ')）"
"$BIN_DIR/tako" remote start >"$TMP/1-start.out" 2>"$TMP/1-start.err"
S1_RC=$?
echo "  （区間内の start（GUI と同じパスの CLI）: 終了コード ${S1_RC}・stderr: $(head -c 600 "$TMP/1-start.err" | LC_ALL=C tr '\n' ' ')）"
wait_status '.running == true and .starting == null and .last_autostart.result != null' $((SLOW + SLOW_OFF + 40)) "$TMP/1-after.json"
# 遅れて立ち上がる 2 本目も数えられるよう、serve を張る待ちの分だけ置いてから数える
sleep "$((SLOW + 2))"
"$CLI" remote status >"$TMP/1-after.json" 2>/dev/null
ps -axo pid=,command= | awk -v tag="$TAG" 'index($0, tag) && index($0, " remote serve")' > "$TMP/1-daemons.txt"
check_eq "daemon は 1 本だけ（2 本目を起こさない）" 1 "$(count_daemons)"
check_ne "区間内の start は断る（終了コード）" 0 "$S1_RC"
check_eq "断りは入れ替えの途中と言う" 1 "$(grep -c '入れ替えている途中' "$TMP/1-start.err" | tr -dc '0-9')"
check_eq "断りは stop を案内しない" 0 "$(grep -c 'tako remote stop' "$TMP/1-start.err" | tr -dc '0-9')"
check_ne "別のパスの CLI の start も断る（終了コード）" 0 "$S1_OTHER_RC"
check_eq "別のパスの CLI の断りも入れ替えの途中と言う" 1 "$(grep -c '入れ替えている途中' "$TMP/1-other.err" | tr -dc '0-9')"
check_eq "別のパスの CLI の断りも stop を案内しない（入れ替えの途中に止めさせない）" 0 "$(grep -c 'tako remote stop' "$TMP/1-other.err" | tr -dc '0-9')"
check_eq "自動復帰の入れ替えは成る" "replaced-generation" "$(field "$TMP/1-after.json" .last_autostart.reason)"
check_eq "新しい daemon が動いている" "true" "$(field "$TMP/1-after.json" .running)"
PID2="$(pid_now)"
if [ -n "$PID2" ] && [ "$PID2" != "$PID1" ]; then pass "pid が替わった（${PID1} → ${PID2}）"; else fail "pid が替わっていない（${PID1} → ${PID2}）"; fi
check_eq "PID ファイルの daemon だけが動いている" "$PID2" "$(live_daemons | tr -d ' ' | head -1)"
check_eq "audit.log に断った記録が残る（2 回の start）" 2 "$(jq -c 'select(.event == "daemon_start_refused" and .state == "replacing" and .starter_via == "autostart")' "$STATE/audit.log" 2>/dev/null | grep -c . | tr -dc '0-9')"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

echo
echo "== ② CLI の start が serve を張っている途中に 2 本目の start =="
echo 0 > "$TMP/slow_off"
echo "$SLOW" > "$TMP/slow"
"$CLI" remote start >"$TMP/2-first.out" 2>"$TMP/2-first.err" &
FIRST=$!
if wait_status '.starting.state == "starting" and .starting.listener != null' 30 "$TMP/2-window.json"; then
  pass "起動の途中（新しい daemon の待ち受けが応答・PID ファイルはまだ）を捉えた"
else
  fail "起動の途中を捉えられなかった"
fi
"$CLI" remote start >"$TMP/2-second.out" 2>"$TMP/2-second.err"
S2_RC=$?
"$CLI" remote stop >"$TMP/2-stop.out" 2>"$TMP/2-stop.err"
S2_STOP_RC=$?
echo "  （区間内の stop: 終了コード ${S2_STOP_RC}・stderr: $(head -c 600 "$TMP/2-stop.err" | LC_ALL=C tr '\n' ' ')）"
echo "  （区間内の start: 終了コード ${S2_RC}・stderr: $(head -c 600 "$TMP/2-second.err" | LC_ALL=C tr '\n' ' ')）"
# 区間の残りを読む（1 本目が終わるまで）
mkdir -p "$TMP/2-reads"
N=0
PIDLESS=0
STARTING=0
while kill -0 "$FIRST" 2>/dev/null && [ "$N" -lt 200 ]; do
  N=$((N + 1))
  "$CLI" remote status >"$TMP/2-reads/$N.json" 2>/dev/null
  if jq -e '.running == false and (.pidless_listener != null or ('"$STOP_GUIDED"'))' "$TMP/2-reads/$N.json" >/dev/null 2>&1; then PIDLESS=$((PIDLESS + 1)); fi
  if jq -e '.starting.state == "starting"' "$TMP/2-reads/$N.json" >/dev/null 2>&1; then STARTING=$((STARTING + 1)); fi
  sleep 0.2
done
wait "$FIRST"
FIRST_RC=$?
echo "  （区間の残りを ${N} 回読んだ）"
check_ne "区間内の 2 本目の start は断る（終了コード）" 0 "$S2_RC"
check_eq "断りは起動の途中と言う" 1 "$(grep -c '起動している途中' "$TMP/2-second.err" | tr -dc '0-9')"
check_eq "区間の残りの status は stop を案内しない（印が消されていない）" 0 "$PIDLESS"
check_ge "区間の残りの status は起動の途中と答え続ける" 1 "$STARTING"
check_ne "区間内の stop は断る（終了コード）" 0 "$S2_STOP_RC"
check_eq "stop の断りは起動の途中なので止めていないと言う" 1 "$(grep -c '止めていません' "$TMP/2-stop.err" | tr -dc '0-9')"
check_eq "stop の断りは kill / 記録の削除を案内しない" 0 "$(grep -c 'kill \|rm ' "$TMP/2-stop.err" | tr -dc '0-9')"
check_eq "1 本目の start は成る" 0 "$FIRST_RC"
check_eq "daemon は 1 本だけ" 1 "$(count_daemons)"
if [ -e "$MARKER" ]; then fail "終わった後に印のファイルが残っている"; else pass "終わった後に印のファイルは残らない"; fi
stop_all_daemons

echo
echo "== ③ 断りの文（印を置いて作る）と A/B =="
echo 0 > "$TMP/slow"
NOW=$(date +%s)
write_marker "$NOW" "$$" ""
command cp -f "$MARKER" "$TMP/3-marker.before"
"$CLI" remote start >"$TMP/3-ja.out" 2>"$TMP/3-ja.err"
check_ne "日本語: 断る（終了コード）" 0 "$?"
check_eq "日本語: 起動の途中と言う" 1 "$(grep -c '起動している途中' "$TMP/3-ja.err" | tr -dc '0-9')"
check_eq "日本語: daemon を起こさない" 0 "$(count_daemons)"
if cmp -s "$MARKER" "$TMP/3-marker.before"; then pass "日本語: 印を書き換えない"; else fail "日本語: 印が書き換わった / 消えた"; fi
stop_all_daemons
write_marker "$NOW" "$$" 4242
TAKO_LANG=en "$CLI" remote start >"$TMP/3-en.out" 2>"$TMP/3-en.err"
check_ne "英語: 断る（終了コード）" 0 "$?"
check_eq "英語: 日本語が混ざらない（行数）" 0 "$(non_ascii_lines "$TMP/3-en.err")"
check_eq "英語: 入れ替えの途中と言う" 1 "$(grep -c 'is being replaced' "$TMP/3-en.err" | tr -dc '0-9')"
check_eq "英語: daemon を起こさない" 0 "$(count_daemons)"
echo "  （英語の断り: $(head -c 600 "$TMP/3-en.err" | LC_ALL=C tr '\n' ' ')）"
stop_all_daemons
write_marker "$NOW" "$$" 4242
TAKO_2613_LEGACY=1 "$CLI" remote start >"$TMP/3-legacy.out" 2>"$TMP/3-legacy.err"
check_eq "A/B（TAKO_2613_LEGACY=1）: 印があっても起こす（修正前の挙動）" 0 "$?"
check_eq "A/B: daemon が立つ" 1 "$(count_daemons)"
stop_all_daemons

echo
echo "== ③' MCP（英語表示の GUI）=="
launch_isolated_gui "$TMP/app3.log" TAKO_LANG=en || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui
write_marker "$(date +%s)" "$$" 4242
mcp_call tako_remote_start '{}' "$TMP/3-mcp.json"
check_eq "MCP: 断る（isError）" "true" "$(field "$TMP/3-mcp.json" .isError)"
jq -r '.content[0].text // ""' "$TMP/3-mcp.json" > "$TMP/3-mcp.txt"
check_eq "MCP: 日本語が混ざらない（行数）" 0 "$(non_ascii_lines "$TMP/3-mcp.txt")"
check_eq "MCP: 入れ替えの途中と言う" 1 "$(grep -c 'is being replaced' "$TMP/3-mcp.txt" | tr -dc '0-9')"
check_eq "MCP: daemon を起こさない" 0 "$(count_daemons)"
echo "  （MCP の断り: $(head -c 600 "$TMP/3-mcp.txt" | LC_ALL=C tr '\n' ' ')）"
stop_all_daemons
mcp_call tako_remote_start '{}' "$TMP/3-mcp-ok.json"
check_eq "MCP の陽性対照: 印が無ければ起こす" "1" "$(count_daemons)"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

echo
echo "== ④ 陽性対照: 印が効いていなければ起こせる =="
"$CLI" remote start >/dev/null 2>"$TMP/4-none.err"
check_eq "印が無い: 起こせる" 0 "$?"
check_eq "印が無い: daemon が立つ" 1 "$(count_daemons)"
stop_all_daemons
sleep 0 &
DEAD=$!
wait "$DEAD"
write_marker "$(date +%s)" "$DEAD" 4242
"$CLI" remote start >/dev/null 2>"$TMP/4-dead.err"
check_eq "書き手が死んだ印: 起こせる" 0 "$?"
check_eq "書き手が死んだ印: daemon が立つ" 1 "$(count_daemons)"
stop_all_daemons
OLD_AT=$(($(date +%s) - 60))
sleep 2
sleep 60 &
REUSED=$!
write_marker "$OLD_AT" "$REUSED" ""
"$CLI" remote start >/dev/null 2>"$TMP/4-reused.err"
check_eq "印より後に起動したプロセス（pid の再利用）: 起こせる" 0 "$?"
kill "$REUSED" 2>/dev/null
wait "$REUSED" 2>/dev/null
stop_all_daemons
write_marker "$(($(date +%s) - 200))" "$$" ""
"$CLI" remote start >/dev/null 2>"$TMP/4-expired.err"
check_eq "期限切れの印: 起こせる" 0 "$?"
check_eq "期限切れの印: daemon が立つ" 1 "$(count_daemons)"
stop_all_daemons

echo
echo "== ⑤ 自動復帰が旧版を入れ替える途中の tako remote stop / --force =="
AUDIT_BEFORE=$(wc -l < "$STATE/audit.log" | tr -dc '0-9')
start_stale_daemon
echo "$SLOW_OFF" > "$TMP/slow_off"
echo "$SLOW" > "$TMP/slow"
launch_isolated_gui "$TMP/app5.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if wait_status '.starting.state == "replacing"' 40 "$TMP/5-window.json"; then
  pass "入れ替えの途中（印が replacing）を捉えた"
else
  fail "入れ替えの途中を捉えられなかった"
fi
"$BIN_DIR/tako" remote stop >"$TMP/5-stop.out" 2>"$TMP/5-stop.err"
S5_RC=$?
"$BIN_DIR/tako" remote stop --force >"$TMP/5-force.out" 2>"$TMP/5-force.err"
S5_FORCE_RC=$?
echo "  （区間内の stop: 終了コード ${S5_RC}・stdout: $(LC_ALL=C tr '\n' ' ' < "$TMP/5-stop.out")・stderr: $(head -c 600 "$TMP/5-stop.err" | LC_ALL=C tr '\n' ' ')）"
"$CLI" remote status >"$TMP/5-just-after.json" 2>/dev/null
check_ne "区間内の stop は断る（終了コード）" 0 "$S5_RC"
check_eq "stop の断りは入れ替えの途中なので止めていないと言う" 1 "$(grep -c '止めていません' "$TMP/5-stop.err" | tr -dc '0-9')"
check_ne "区間内の --force も断る（終了コード）" 0 "$S5_FORCE_RC"
check_eq "断った stop は「起動していた」を消さない" "true" "$(field "$TMP/5-just-after.json" .desired)"
wait_status '.running == true and .starting == null and .last_autostart.result != null' $((SLOW + SLOW_OFF + 40)) "$TMP/5-after.json"
check_eq "入れ替えは成る" "replaced-generation" "$(field "$TMP/5-after.json" .last_autostart.reason)"
check_eq "daemon は 1 本" 1 "$(count_daemons)"
"$BIN_DIR/tako" remote stop >"$TMP/5-stop-after.out" 2>"$TMP/5-stop-after.err"
check_eq "陽性対照: 終わった後の stop は止める（終了コード）" 0 "$?"
"$CLI" remote status >"$TMP/5-stopped.json" 2>/dev/null
check_eq "陽性対照: 止まっている" "false" "$(field "$TMP/5-stopped.json" .running)"
check_eq "陽性対照: 「起動していた」も消える" "false" "$(field "$TMP/5-stopped.json" .desired)"
check_eq "陽性対照: daemon は 0 本" 0 "$(count_daemons)"
check_eq "audit.log に stop を断った記録が残る（stop と --force）" 2 "$(tail -n +"$((AUDIT_BEFORE + 1))" "$STATE/audit.log" | jq -c 'select(.event == "daemon_stop_refused" and .state == "replacing" and .starter_via == "autostart")' 2>/dev/null | grep -c . | tr -dc '0-9')"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

# 証拠を残す（TAKO_2613_EVIDENCE_DIR=<dir>）
if [ -n "${TAKO_2613_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2613_EVIDENCE_DIR"
  for f in 1-window.json 1-other.err 1-start.err 1-start.out 1-after.json 1-daemons.txt 2-window.json 2-second.err 2-stop.err 3-ja.err 3-en.err 3-mcp.json 3-legacy.err 5-stop.out 5-stop.err 5-force.err 5-just-after.json 5-after.json; do
    if [ -e "$TMP/$f" ]; then command cp -f "$TMP/$f" "$TAKO_2613_EVIDENCE_DIR/"; fi
  done
  if [ -d "$TMP/2-reads" ]; then command cp -Rf "$TMP/2-reads" "$TAKO_2613_EVIDENCE_DIR/2-reads"; fi
  jq -c 'select(.event | test("daemon_(start|stop)_(requested|refused)"))' "$STATE/audit.log" > "$TAKO_2613_EVIDENCE_DIR/audit-start-stop.jsonl" 2>/dev/null
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
