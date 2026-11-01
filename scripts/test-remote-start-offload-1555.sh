#!/usr/bin/env bash
# test-remote-start-offload-1555.sh — RemoteStart が GUI のメインスレッドを専有しないかの実経路テスト（#1555）
#
# 隔離した state / data ディレクトリ・偽の tailscale CLI（TAKO_TAILSCALE_BIN）で
# **実 tako-app（隔離 GUI）+ 実 daemon** を走らせ、daemon の起動を偽の tailscale の待ちで伸ばす
# （新版が張る `serve --bg` を TAKO_1555_SLOW_SECS 秒待たせる = 本番の約 10.5 秒の起動の注入。負荷は作らない）。
#   ① MCP `tako_remote_start`（IPC → GUI の dispatch）を打ち、起動を待っている間に別の IPC（`tako list`）が
#      待たされないか・perf.log に `dispatch:RemoteStart` の専有が出ないかを測る。応答の中身（欄）は修正前と同じ。
#      修正前は `tako list` が起動の終わりまで待たされ、perf.log に `dispatch:RemoteStart が N ms` が出る
#   ② 断り・失敗の経路も同じ中身で返る: 既に起動中（MCP）・tailscale が繋がっていない（子の stderr の理由）
#   ③ GUI 起動時の自動復帰（#1485）: 「起動していた」を残したまま daemon を落として GUI を起こし直し、
#      自動復帰が daemon を起こしている間も IPC が待たされない・専有が出ない
#   ④ A/B（TAKO_1555_LEGACY=1）: 修正前と同じく UI スレッドで待つ（① の専有が戻る）
#   ⑤ CLI の `tako remote start` は GUI を通らない（自分のプロセスで起こす）ので GUI の専有を作らない（対照）
#   ⑥ MCP の start を 2 本続けて打つ: background で並んでも daemon は 1 本で、2 本目は断る
#      （修正前は UI スレッドで直列 = 1 本目の後に「既に起動中」・修正後は #2613 の印で「起動の途中」）
# を実測する。修正前のバイナリ（TAKO_BIN / APP_BIN を差し替える）では ① と ② の perf.log の行が落ちる
# （③ の自動復帰は修正前から background で起こしているので通る）。
#
# **本番の tailscale / serve 設定 / remote デーモン / GUI には一切触らない**
# （tailscale の呼び出しは全部この偽物へ向く。state・data・発見の置き場は mktemp 配下。
# 落とすのは自分で起こした GUI と、この置き場のバイナリで動いている daemon だけ）。
# 窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-remote-start-offload-1555.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。
#   起動の待ちの長さは TAKO_1555_SLOW_SECS（既定 6）。証拠は TAKO_1555_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_1555_LEGACY TAKO_OFFLOAD

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_lt() {
  if [ "$3" -lt "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 未満のはずが ${3}）"; fi
}
check_ge() {
  if [ "$3" -ge "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 以上のはずが ${3}）"; fi
}

SLOW="${TAKO_1555_SLOW_SECS:-6}"
SLOW_MS=$((SLOW * 1000))
# 「待たされていない」の上限（ms）。debug ビルドの CLI の起動と `tako list` の往復を含めて余裕を持たせる
QUICK_MS="${TAKO_1555_QUICK_MS:-1500}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t1555-XXXXXX)"
TAG="$(basename "$TMP")"
TMUX_SOCKET="tako-iso-1555-$$"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

STATE="$TMP/state"
BIN_DIR="$TMP/bin"
mkdir -p "$STATE" "$TMP/data" "$TMP/disc" "$BIN_DIR" "$TMP/ctl"
# 隔離モードの serve_binary() は tako-app と同じディレクトリの tako を選ぶので 2 つを並べる。
# 検査に使う CLI は別の置き場の複製
command cp -f "$APP_BIN" "$BIN_DIR/tako-app"
command cp -f "$TAKO_BIN" "$BIN_DIR/tako"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
APP_BIN="$BIN_DIR/tako-app"
CLI="$TMP/ctl/tako"
PERF="$TMP/data/perf.log"

# この置き場のバイナリで動いている daemon の pid（置き場の名前はこの実行に固有）
live_daemons() {
  ps -axo pid=,command= | awk -v tag="$TAG" 'index($0, tag) && index($0, " remote serve") { print $1 }'
}
count_daemons() { live_daemons | grep -c . | tr -dc '0-9'; }
# daemon を止める。「起動していた」（desired）を残すなら $1 = keep（pid へ TERM を送るだけ）
stop_all_daemons() {
  if [ "${1:-}" != "keep" ]; then
    "$CLI" remote stop >/dev/null 2>&1 || true
  fi
  local p
  for p in $(live_daemons); do
    kill "$p" 2>/dev/null || true
  done
  local i=0
  while [ -n "$(live_daemons)" ] && [ "$i" -lt 100 ]; do
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
# `serve --bg` は $TMP/slow 秒待つ（起動の区間を伸ばす注入）。$TMP/down があれば接続が無効（Stopped）と答える
FAKE_TS="$TMP/fake-tailscale"
echo '{}' > "$TMP/serve.json"
echo 0 > "$TMP/slow"
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
NODE="fake-1555.tailfake.ts.net"
SERVE="$STATE/serve.json"
BACKEND="Running"
if [ -e "$STATE/down" ]; then BACKEND="Stopped"; fi
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"%s","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$BACKEND" "$NODE" "$NODE"
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
export TAKO_LANG=ja
# 自動復帰の 1 回目の前の待ち（既定 2 秒）を縮める
export TAKO_1485_BACKOFF_SECS=1
case "$STATE" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $STATE"; exit 1 ;;
esac

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'; }
field() { jq -r "$2 | if . == null then \"null\" else tostring end" "$1" 2>/dev/null; }

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

# MCP の stdio ブリッジで隔離 GUI のツールを 1 回呼ぶ（$1 = ツール・$2 = 引数の JSON・$3 = 結果の書き先）
mcp_call() {
  local ctl="$TMP/disc/control.json"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1555","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$(jq -r .socket "$ctl")" TAKO_TOKEN="$(jq -r .token "$ctl")" "$CLI" mcp serve \
      >"$3.raw" 2>"$3.err"
  jq -c 'select(.id == 2) | .result' "$3.raw" > "$3"
}

# 区間の間 `tako list`（IPC）を打ち続け、1 回ごとの往復の ms を $2 へ 1 行ずつ書く。
# $1 = 区間を表す pid（終わるまで続ける）。少なくとも 3 回は打つ
probe_ipc_while() {
  local busy="$1" out="$2" n=0 t0 t1
  : > "$out"
  while kill -0 "$busy" 2>/dev/null || [ "$n" -lt 3 ]; do
    t0=$(now_ms)
    "$CLI" list >/dev/null 2>&1
    t1=$(now_ms)
    echo $((t1 - t0)) >> "$out"
    n=$((n + 1))
    [ "$n" -ge 200 ] && break
    sleep 0.2
  done
}
max_of() { sort -n "$1" | tail -1 | tr -dc '0-9'; }
# perf.log の「メインスレッド専有: <タグ> が N ms」の行数（$1 = タグの正規表現）
perf_hits() {
  grep -cE "メインスレッド専有: $1 が [0-9]+ms" "$PERF" 2>/dev/null | tr -dc '0-9'
}
# perf.log の 1000ms 以上の専有の行（タグを問わない）
perf_long() {
  grep -E "メインスレッド(長時間)?専有.* [0-9]{4,}ms|UI ストール" "$PERF" 2>/dev/null
}

# MCP の start を背景で打ち、その間の IPC の往復を測る（$1 = 名前の接頭辞）
measure_mcp_start() {
  local p="$1" t0 t1
  echo "$SLOW" > "$TMP/slow"
  : > "$PERF"
  t0=$(now_ms)
  mcp_call tako_remote_start '{}' "$TMP/$p-start.json" &
  local busy=$!
  # MCP の要求が GUI に届くまで少し置いてから打ち始める
  sleep 1
  probe_ipc_while "$busy" "$TMP/$p-ipc.txt"
  wait "$busy"
  t1=$(now_ms)
  echo $((t1 - t0)) > "$TMP/$p-start-ms.txt"
  echo 0 > "$TMP/slow"
  # 区間の後の記録が perf.log へ落ちるのを待つ（専有の行は区間を抜けたところで書かれる）
  sleep 1
  command cp -f "$PERF" "$TMP/$p-perf.log" 2>/dev/null || : > "$TMP/$p-perf.log"
  echo "  （MCP の start の所要: $(cat "$TMP/$p-start-ms.txt")ms・その間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/$p-ipc.txt")ms）"
  echo "  （perf.log の 1000ms 以上の行: $(perf_long | grep -c . | tr -dc '0-9') 件）"
  perf_long | sed 's/^/    /' | head -5
}

echo
echo "== ① MCP tako_remote_start（IPC → GUI の dispatch）の間に別の IPC は待たされない =="
launch_isolated_gui "$TMP/app1.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui
measure_mcp_start 1
check_eq "MCP の start は成る（isError でない）" "false" "$(jq -r '.isError // false' "$TMP/1-start.json")"
jq -r '.content[0].text // ""' "$TMP/1-start.json" > "$TMP/1-start.txt"
jq -r 'keys | join(",")' "$TMP/1-start.txt" > "$TMP/1-keys.txt" 2>/dev/null
echo "  （応答の欄: $(cat "$TMP/1-keys.txt")）"
check_eq "応答に url がある" "true" "$(jq -r '(.url // "") | startswith("https://")' "$TMP/1-start.txt" 2>/dev/null)"
check_eq "応答に serve_binary がある（#432）" "true" "$(jq -r '(.serve_binary // "") != ""' "$TMP/1-start.txt" 2>/dev/null)"
check_ge "start は注入した待ちの分かかる（区間を作れている）" "$SLOW_MS" "$(cat "$TMP/1-start-ms.txt")"
check_lt "start の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/1-ipc.txt")"
check_eq "perf.log に dispatch:RemoteStart の専有が出ない" 0 "$(perf_hits 'dispatch:RemoteStart')"
check_eq "perf.log に 1000ms 以上の専有が出ない" 0 "$(perf_long | grep -c . | tr -dc '0-9')"
check_eq "daemon は 1 本" 1 "$(count_daemons)"
"$CLI" remote status >"$TMP/1-status.json" 2>/dev/null
check_eq "status は起動済み" "true" "$(field "$TMP/1-status.json" .running)"
check_eq "「起動していた」が残る（#1485 の唯一の書き手を通った）" "true" "$(field "$TMP/1-status.json" .desired)"
check_eq "audit.log に経路 dispatch の起動の要求が残る（#2533）" 1 "$(jq -c 'select(.event == "daemon_start_requested" and .via == "dispatch")' "$STATE/audit.log" 2>/dev/null | grep -c . | tr -dc '0-9')"

echo
echo "== ② 断り・失敗の経路（既に起動中 / tailscale が繋がっていない）=="
: > "$PERF"
mcp_call tako_remote_start '{}' "$TMP/2-running.json"
check_eq "既に起動中: 断る（isError）" "true" "$(field "$TMP/2-running.json" .isError)"
jq -r '.content[0].text // ""' "$TMP/2-running.json" > "$TMP/2-running.txt"
check_eq "既に起動中: 文は「既に起動中」" 1 "$(grep -c '既に起動中' "$TMP/2-running.txt" | tr -dc '0-9')"
echo "  （既に起動中の断り: $(head -c 300 "$TMP/2-running.txt" | LC_ALL=C tr '\n' ' ')）"
check_eq "既に起動中: daemon は 1 本のまま" 1 "$(count_daemons)"
stop_all_daemons
touch "$TMP/down"
mcp_call tako_remote_start '{}' "$TMP/2-down.json"
rm -f "${TMP:?}/down"
check_eq "tailscale が繋がっていない: 断る（isError）" "true" "$(field "$TMP/2-down.json" .isError)"
jq -r '.content[0].text // ""' "$TMP/2-down.json" > "$TMP/2-down.txt"
check_eq "tailscale が繋がっていない: 子の理由を伝える（Stopped）" 1 "$(grep -c 'Stopped' "$TMP/2-down.txt" | tr -dc '0-9')"
echo "  （繋がっていないときの断り: $(head -c 300 "$TMP/2-down.txt" | LC_ALL=C tr '\n' ' ')）"
check_eq "tailscale が繋がっていない: daemon を残さない" 0 "$(count_daemons)"
check_eq "断り・失敗の経路も perf.log に dispatch:RemoteStart の専有が出ない" 0 "$(perf_hits 'dispatch:RemoteStart')"
command cp -f "$PERF" "$TMP/2-perf.log" 2>/dev/null || :

echo
echo "== ③ GUI 起動時の自動復帰（#1485）が daemon を起こす間も IPC は待たされない =="
# 「起動していた」を残すため MCP で起こしてから、GUI を落とし daemon を pid で落とす
mcp_call tako_remote_start '{}' "$TMP/3-seed.json"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons keep
"$CLI" remote status >"$TMP/3-before.json" 2>/dev/null
check_eq "前提: daemon は止まっていて「起動していた」が残る" "false/true" "$(field "$TMP/3-before.json" .running)/$(field "$TMP/3-before.json" .desired)"
echo "$SLOW" > "$TMP/slow"
launch_isolated_gui "$TMP/app3.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
: > "$PERF"
if wait_status '.starting.state == "starting"' 40 "$TMP/3-window.json"; then
  pass "自動復帰が起こしている途中（印が starting）を捉えた"
else
  fail "自動復帰が起こしている途中を捉えられなかった"
fi
# 自動復帰の起動が終わるまで IPC を打ち続ける（終わりは印が消えて running になったこと）
(
  wait_status '.running == true and .starting == null' $((SLOW + 40)) "$TMP/3-after.json"
) &
WAITER=$!
probe_ipc_while "$WAITER" "$TMP/3-ipc.txt"
wait "$WAITER"
echo 0 > "$TMP/slow"
sleep 1
command cp -f "$PERF" "$TMP/3-perf.log" 2>/dev/null || :
echo "  （自動復帰の間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/3-ipc.txt")ms）"
"$CLI" remote status >"$TMP/3-after.json" 2>/dev/null
check_eq "自動復帰が起こした" "true/started" "$(field "$TMP/3-after.json" .running)/$(field "$TMP/3-after.json" .last_autostart.result)"
check_lt "自動復帰の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/3-ipc.txt")"
check_eq "自動復帰の間の perf.log に 1000ms 以上の専有が出ない" 0 "$(perf_long | grep -c . | tr -dc '0-9')"
perf_long | sed 's/^/    /' | head -5
check_eq "daemon は 1 本" 1 "$(count_daemons)"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

echo
echo "== ④ A/B（TAKO_1555_LEGACY=1）: 修正前と同じく UI スレッドで待つ =="
launch_isolated_gui "$TMP/app4.log" TAKO_1555_LEGACY=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui
measure_mcp_start 4
check_eq "A/B: start は成る" "false" "$(jq -r '.isError // false' "$TMP/4-start.json")"
jq -r '.content[0].text // ""' "$TMP/4-start.json" > "$TMP/4-start.txt"
jq -r 'keys | join(",")' "$TMP/4-start.txt" > "$TMP/4-keys.txt" 2>/dev/null
check_eq "A/B: 応答の欄は ① と同じ" "$(cat "$TMP/1-keys.txt")" "$(cat "$TMP/4-keys.txt")"
check_ge "A/B: start の間の tako list は起動の終わりまで待たされる（ms）" "$((SLOW_MS / 2))" "$(max_of "$TMP/4-ipc.txt")"
check_eq "A/B: perf.log に dispatch:RemoteStart の専有が出る" 1 "$(perf_hits 'dispatch:RemoteStart')"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

echo
echo "== ⑤ 対照: CLI の tako remote start は GUI を通らない =="
launch_isolated_gui "$TMP/app5.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui
echo "$SLOW" > "$TMP/slow"
: > "$PERF"
"$CLI" remote start >"$TMP/5-start.json" 2>"$TMP/5-start.err" &
BUSY=$!
sleep 1
probe_ipc_while "$BUSY" "$TMP/5-ipc.txt"
wait "$BUSY"
S5_RC=$?
echo 0 > "$TMP/slow"
sleep 1
check_eq "CLI の start は成る" 0 "$S5_RC"
check_lt "CLI の start の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/5-ipc.txt")"
check_eq "CLI の start は GUI の dispatch:RemoteStart を通らない" 0 "$(perf_hits 'dispatch:RemoteStart')"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

echo
echo "== ⑥ MCP の start を 2 本続けて打つ（background で並ぶ。daemon は 1 本・2 本目は断る）=="
launch_isolated_gui "$TMP/app6.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui
echo "$SLOW" > "$TMP/slow"
mcp_call tako_remote_start '{}' "$TMP/6-first.json" &
FIRST=$!
# 1 本目が印を置いて子を起こしている途中に 2 本目を打つ
if wait_status '.starting.state == "starting"' 30 "$TMP/6-window.json"; then
  pass "1 本目が起こしている途中（印が starting）を捉えた"
else
  fail "1 本目が起こしている途中を捉えられなかった"
fi
mcp_call tako_remote_start '{}' "$TMP/6-second.json"
wait "$FIRST"
echo 0 > "$TMP/slow"
jq -r '.content[0].text // ""' "$TMP/6-second.json" > "$TMP/6-second.txt"
echo "  （2 本目: $(head -c 300 "$TMP/6-second.txt" | LC_ALL=C tr '\n' ' ')）"
check_eq "1 本目は成る" "false" "$(jq -r '.isError // false' "$TMP/6-first.json")"
check_eq "2 本目は断る（isError）" "true" "$(field "$TMP/6-second.json" .isError)"
check_eq "2 本目の断りは起動の途中か既に起動中と言う" 1 "$(grep -cE '起動している途中|既に起動中' "$TMP/6-second.txt" | tr -dc '0-9')"
check_eq "2 本目の断りは stop を案内しない" 0 "$(grep -c 'tako remote stop' "$TMP/6-second.txt" | tr -dc '0-9')"
check_eq "daemon は 1 本" 1 "$(count_daemons)"
stop_isolated_gui "$APP_PID"
APP_PID=""
stop_all_daemons

# 証拠を残す（TAKO_1555_EVIDENCE_DIR=<dir>）
if [ -n "${TAKO_1555_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_1555_EVIDENCE_DIR"
  for f in 1-start.txt 1-keys.txt 1-ipc.txt 1-start-ms.txt 1-perf.log 2-running.txt 2-down.txt 2-perf.log \
    3-window.json 3-after.json 3-ipc.txt 3-perf.log 4-start.txt 4-keys.txt 4-ipc.txt 4-start-ms.txt 4-perf.log 5-ipc.txt \
    6-first.json 6-second.txt; do
    if [ -e "$TMP/$f" ]; then command cp -f "$TMP/$f" "$TAKO_1555_EVIDENCE_DIR/"; fi
  done
  jq -c 'select(.event | test("daemon_(start|stop)_(requested|refused)"))' "$STATE/audit.log" > "$TAKO_1555_EVIDENCE_DIR/audit-start-stop.jsonl" 2>/dev/null
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
