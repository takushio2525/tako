#!/usr/bin/env bash
# test-remote-stop-status-offload-2630.sh — RemoteStop / RemoteStatus が GUI のメインスレッドを専有しないかの実経路テスト（#2630）
#
# 隔離した state / data ディレクトリ・偽の tailscale CLI（TAKO_TAILSCALE_BIN）で
# **実 tako-app（隔離 GUI）+ 実 daemon** を走らせ、stop と status の所要を偽の tailscale の待ちで伸ばす（負荷は作らない）:
#   - stop: daemon が畳むときに外す `serve --https=443 off` を TAKO_2630_SLOW_SECS 秒待たせる
#     （GUI の `daemon_stop` は daemon の終了を待つ = 本番の dispatch:RemoteStop 247〜349ms の区間を伸ばす）
#   - status: `tailscale status --json` を同じ秒数待たせる（`daemon_status` は 2 系統の同居の調査で最大 3 本起こす）
#   ① MCP `tako_remote_stop`（IPC → GUI の dispatch）の間に別の IPC（`tako list`）が待たされない・
#      perf.log に `dispatch:RemoteStop` の専有が出ない。応答の中身は修正前と同じ
#   ② MCP `tako_remote_status` の間も同じ（`dispatch:RemoteStatus`）
#   ③ 設定画面のリモートタブの再読み込み（`tako settings --tab remote` で切り替える = `refresh_tab_status`）の
#      間も IPC が待たされない（UI スレッドの同期の dispatch は perf.log の計測区間の外なので、IPC の往復と
#      「UI ストール」の行で見る）
#   ④ 境界: 止まっている daemon への stop（断り）・止まっている間の status は修正前と同じ中身ですぐ返る
#   ⑤ A/B（TAKO_2630_LEGACY=1）: 修正前と同じく UI スレッドで待つ（①②③ の待ちと専有が戻る）。応答の中身は ①② と同じ
# を実測する。修正前のバイナリ（TAKO_BIN / APP_BIN を差し替える）では ①②③ の行が落ちる。
#
# **本番の tailscale / serve 設定 / remote デーモン / GUI には一切触らない**
# （tailscale の呼び出しは全部この偽物へ向く。state・data・発見の置き場は mktemp 配下。
# 落とすのは自分で起こした GUI と、この置き場のバイナリで動いている daemon だけ）。
# 窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-remote-stop-status-offload-2630.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。
#   待ちの長さは TAKO_2630_SLOW_SECS（既定 3）。証拠は TAKO_2630_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2630_LEGACY TAKO_1555_LEGACY TAKO_OFFLOAD

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

SLOW="${TAKO_2630_SLOW_SECS:-3}"
SLOW_MS=$((SLOW * 1000))
# 「待たされていない」の上限（ms）。debug ビルドの CLI の起動と `tako list` の往復を含めて余裕を持たせる
QUICK_MS="${TAKO_2630_QUICK_MS:-1500}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2630-XXXXXX)"
TAG="$(basename "$TMP")"
TMUX_SOCKET="tako-iso-2630-$$"
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
# 遅延の注入を全部外す
no_slow() { echo 0 > "$TMP/slow_off"; echo 0 > "$TMP/slow_status"; }
# daemon を止める（この置き場の pid だけ）
stop_all_daemons() {
  no_slow
  "$CLI" remote stop >/dev/null 2>&1 || true
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
# `serve --https=443 off` は $TMP/slow_off 秒・`status` は $TMP/slow_status 秒待つ（区間を伸ばす注入）
FAKE_TS="$TMP/fake-tailscale"
echo '{}' > "$TMP/serve.json"
no_slow
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
NODE="fake-2630.tailfake.ts.net"
SERVE="$STATE/serve.json"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    sleep "$(cat "$STATE/slow_status" 2>/dev/null || echo 0)"
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  serve)
    shift
    case "${1:-}" in
      status) cat "$SERVE"; exit 0 ;;
      --bg)
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
export TAKO_LANG=ja
case "$STATE" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $STATE"; exit 1 ;;
esac

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'; }
field() { jq -r "$2 | if . == null then \"null\" else tostring end" "$1" 2>/dev/null; }

# MCP の stdio ブリッジで隔離 GUI のツールを 1 回呼ぶ（$1 = ツール・$2 = 引数の JSON・$3 = 結果の書き先）
mcp_call() {
  local ctl="$TMP/disc/control.json"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2630","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$(jq -r .socket "$ctl")" TAKO_TOKEN="$(jq -r .token "$ctl")" "$CLI" mcp serve \
      >"$3.raw" 2>"$3.err"
  jq -c 'select(.id == 2) | .result' "$3.raw" > "$3"
}
# MCP の結果の本文（content[0].text）
mcp_text() { jq -r '.content[0].text // ""' "$1"; }

# 区間の間 `tako list`（IPC）を打ち続け、1 回ごとの往復の ms を $2 へ 1 行ずつ書く。
# 繋がらなかった往復は速く返るので速さに数えず、$2.fail へ数を書く。
# $1 = 区間を表す pid（終わるまで続ける）。少なくとも 3 回は打つ
probe_ipc_while() {
  local busy="$1" out="$2" n=0 failed=0 t0 t1
  : > "$out"
  while kill -0 "$busy" 2>/dev/null || [ "$n" -lt 3 ]; do
    t0=$(now_ms)
    if "$CLI" list >/dev/null 2>&1; then
      t1=$(now_ms)
      echo $((t1 - t0)) >> "$out"
    else
      failed=$((failed + 1))
    fi
    n=$((n + 1))
    [ "$n" -ge 200 ] && break
    sleep 0.2
  done
  echo "$failed" > "$out.fail"
}
# 往復の最長（ms）。1 回も通らなければ大きな値（= 「待たされない」を満たさない）
max_of() {
  if [ -s "$1" ]; then sort -n "$1" | tail -1 | tr -dc '0-9'; else echo 999999; fi
}
fails_of() { tr -dc '0-9' < "$1.fail"; }
# perf.log の「メインスレッド専有: <タグ> が N ms」の行数（$1 = タグの正規表現）
perf_hits() {
  grep -cE "メインスレッド専有: $1 が [0-9]+ms" "$PERF" 2>/dev/null | tr -dc '0-9'
}
# perf.log の UI スレッドの遅れ（計測区間の外の同期の待ちはここにしか出ない）
perf_stalls() {
  grep -E "UI ストール|メインスレッド(長時間)?専有.* [0-9]{4,}ms" "$PERF" 2>/dev/null
}

# daemon を CLI で起こす（GUI を通らない = 区間の外）。起きれば 0
start_daemon() {
  no_slow
  "$CLI" remote start >"$TMP/start.out" 2>"$TMP/start.err" || return 1
  [ "$(count_daemons)" = 1 ]
}

# MCP のツールを背景で打ち、その間の IPC の往復を測る（$1 = 名前の接頭辞・$2 = ツール・$3 = 注入の置き場）
measure_mcp() {
  local p="$1" tool="$2" knob="$3" t0 t1
  echo "$SLOW" > "$TMP/$knob"
  : > "$PERF"
  t0=$(now_ms)
  mcp_call "$tool" '{}' "$TMP/$p.json" &
  local busy=$!
  # MCP の要求が GUI に届くまで少し置いてから打ち始める
  sleep 0.5
  probe_ipc_while "$busy" "$TMP/$p-ipc.txt"
  wait "$busy"
  t1=$(now_ms)
  echo $((t1 - t0)) > "$TMP/$p-ms.txt"
  no_slow
  # 区間の後の記録が perf.log へ落ちるのを待つ
  sleep 1.5
  command cp -f "$PERF" "$TMP/$p-perf.log" 2>/dev/null || : > "$TMP/$p-perf.log"
  echo "  （$tool の所要: $(cat "$TMP/$p-ms.txt")ms・その間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/$p-ipc.txt")ms）"
  grep -E "dispatch:Remote|UI ストール" "$PERF" 2>/dev/null | sed 's/^/    perf.log: /' | head -5
}

# 設定画面のリモートタブを読み直させ（一般 → リモートへ切り替える）、その間の IPC の往復を測る（$1 = 接頭辞）。
# 切り替えは `tako settings` の IPC の中で畳まれる（cx.defer は応答の前に流れる）ので、その IPC を背景で打ち、
# 打っている間と、background の読み直しが終わるまでの区間（注入の 2 回ぶん）を打ち続ける
measure_settings_reload() {
  local p="$1" t0 t1
  "$CLI" settings --tab general >/dev/null 2>&1
  sleep 1
  echo "$SLOW" > "$TMP/slow_status"
  : > "$PERF"
  (
    t0=$(now_ms)
    "$CLI" settings --tab remote >"$TMP/$p-settings.out" 2>&1
    t1=$(now_ms)
    echo $((t1 - t0)) > "$TMP/$p-settings-ms.txt"
    sleep $((SLOW * 2 + 1))
  ) &
  local busy=$!
  sleep 0.5
  probe_ipc_while "$busy" "$TMP/$p-ipc.txt"
  wait "$busy"
  no_slow
  sleep 1.5
  command cp -f "$PERF" "$TMP/$p-perf.log" 2>/dev/null || : > "$TMP/$p-perf.log"
  echo "  （tako settings --tab remote の往復: $(cat "$TMP/$p-settings-ms.txt")ms・その間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/$p-ipc.txt")ms）"
  perf_stalls | sed 's/^/    perf.log: /' | head -5
}

# status の応答を比べられる形へ（時刻・pid・ポートなど実行ごとに変わる値を落とす）
normalize_status() {
  jq -S 'del(.pid, .url, .endpoint, .serve_target, .serve_actual, .socket, .serve_binary, .serve_generation, .last_autostart, .desired_since, .serve_checked_at, .serve_checked_age_secs, .peer_verification)' 2>/dev/null
}

# 1 回ぶんの GUI を起こして ①〜④ を回す（$1 = 接頭辞・$2… = GUI へ渡す VAR=VAL）
run_round() {
  local r="$1"
  shift
  launch_isolated_gui "$TMP/app-$r.log" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui

  echo
  echo "== [$r] ① MCP tako_remote_stop の間に別の IPC は待たされない =="
  if start_daemon; then pass "[$r] 前提: daemon を起こした"; else fail "[$r] 前提: daemon を起こせない（$(head -c 300 "$TMP/start.err")）"; fi
  measure_mcp "$r-stop" tako_remote_stop slow_off
  mcp_text "$TMP/$r-stop.json" > "$TMP/$r-stop.txt"
  echo "  （応答: $(head -c 300 "$TMP/$r-stop.txt" | LC_ALL=C tr '\n' ' ')）"
  check_eq "[$r] stop は成る（isError でない）" "false" "$(jq -r '.isError // false' "$TMP/$r-stop.json")"
  check_eq "[$r] stop の応答は stopped / via=shutdown" "true/shutdown" "$(jq -r '"\(.stopped)/\(.via)"' "$TMP/$r-stop.txt" 2>/dev/null)"
  check_ge "[$r] stop は注入した待ちの分かかる（区間を作れている）" "$SLOW_MS" "$(cat "$TMP/$r-stop-ms.txt")"
  check_eq "[$r] daemon は残らない" 0 "$(count_daemons)"

  echo
  echo "== [$r] ② MCP tako_remote_status（daemon 稼働中）の間に別の IPC は待たされない =="
  if start_daemon; then pass "[$r] 前提: daemon を起こした"; else fail "[$r] 前提: daemon を起こせない"; fi
  measure_mcp "$r-status" tako_remote_status slow_status
  mcp_text "$TMP/$r-status.json" > "$TMP/$r-status.txt"
  normalize_status < "$TMP/$r-status.txt" > "$TMP/$r-status.norm"
  check_eq "[$r] status は成る（isError でない）" "false" "$(jq -r '.isError // false' "$TMP/$r-status.json")"
  check_eq "[$r] status は running" "true" "$(field "$TMP/$r-status.txt" .running)"
  check_ge "[$r] status は注入した待ちの分かかる（区間を作れている）" "$SLOW_MS" "$(cat "$TMP/$r-status-ms.txt")"

  echo
  echo "== [$r] ③ 設定画面のリモートタブの再読み込みの間に別の IPC は待たされない =="
  measure_settings_reload "$r-settings"
  check_eq "[$r] 設定画面を開けた（tako settings の応答）" 1 "$(grep -c '"ok"' "$TMP/$r-settings-settings.out" | tr -dc '0-9')"

  echo
  echo "== [$r] ④ 境界: 止まっている daemon への stop・止まっている間の status =="
  stop_all_daemons
  : > "$PERF"
  mcp_call tako_remote_stop '{}' "$TMP/$r-stop-idle.json"
  mcp_text "$TMP/$r-stop-idle.json" > "$TMP/$r-stop-idle.txt"
  check_eq "[$r] 止まっている daemon への stop は断る（isError）" "true" "$(field "$TMP/$r-stop-idle.json" .isError)"
  echo "  （断り: $(head -c 200 "$TMP/$r-stop-idle.txt" | LC_ALL=C tr '\n' ' ')）"
  mcp_call tako_remote_status '{}' "$TMP/$r-status-idle.json"
  mcp_text "$TMP/$r-status-idle.json" | normalize_status > "$TMP/$r-status-idle.norm"
  check_eq "[$r] 止まっている間の status は running=false" "false" "$(field "$TMP/$r-status-idle.norm" .running)"
  # 止める（--force）の断りの経路も同じ口を通る
  mcp_call tako_remote_stop '{"force":true}' "$TMP/$r-force-idle.json"
  mcp_text "$TMP/$r-force-idle.json" > "$TMP/$r-force-idle.txt"
  check_eq "[$r] 止まっている daemon への stop --force も断る（isError）" "true" "$(field "$TMP/$r-force-idle.json" .isError)"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
  stop_all_daemons
}

run_round new
echo
echo "== 修正後（既定）の判定 =="
check_lt "① stop の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-stop-ipc.txt")"
check_eq "① stop の間の tako list は 1 回も失敗しない" 0 "$(fails_of "$TMP/new-stop-ipc.txt")"
PERF="$TMP/new-stop-perf.log"
check_eq "① perf.log に dispatch:RemoteStop の専有が出ない" 0 "$(perf_hits 'dispatch:RemoteStop')"
check_lt "② status の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-status-ipc.txt")"
check_eq "② status の間の tako list は 1 回も失敗しない" 0 "$(fails_of "$TMP/new-status-ipc.txt")"
PERF="$TMP/new-status-perf.log"
check_eq "② perf.log に dispatch:RemoteStatus の専有が出ない" 0 "$(perf_hits 'dispatch:RemoteStatus')"
check_lt "③ tako settings --tab remote の往復（ms。読み直しを待たずに返る）" "$QUICK_MS" "$(tr -dc '0-9' < "$TMP/new-settings-settings-ms.txt")"
check_lt "③ 再読み込みの間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-settings-ipc.txt")"
check_eq "③ 再読み込みの間の tako list は 1 回も失敗しない" 0 "$(fails_of "$TMP/new-settings-ipc.txt")"
PERF="$TMP/new-settings-perf.log"
check_eq "③ perf.log に dispatch:RemoteStatus の専有が出ない" 0 "$(perf_hits 'dispatch:RemoteStatus')"
check_eq "③ perf.log に UI ストールが出ない" 0 "$(perf_stalls | grep -c . | tr -dc '0-9')"
PERF="$TMP/data/perf.log"

run_round legacy TAKO_2630_LEGACY=1
echo
echo "== A/B（TAKO_2630_LEGACY=1）の判定: 修正前と同じく UI スレッドで待つ =="
check_ge "A/B ① stop の間の tako list は待たされる（ms）" "$((SLOW_MS / 2))" "$(max_of "$TMP/legacy-stop-ipc.txt")"
PERF="$TMP/legacy-stop-perf.log"
check_eq "A/B ① perf.log に dispatch:RemoteStop の専有が出る" 1 "$(perf_hits 'dispatch:RemoteStop')"
check_ge "A/B ② status の間の tako list は待たされる（ms）" "$((SLOW_MS / 2))" "$(max_of "$TMP/legacy-status-ipc.txt")"
PERF="$TMP/legacy-status-perf.log"
check_eq "A/B ② perf.log に dispatch:RemoteStatus の専有が出る" 1 "$(perf_hits 'dispatch:RemoteStatus')"
check_ge "A/B ③ tako settings --tab remote は読み直しの終わりまで待たされる（ms）" "$SLOW_MS" "$(tr -dc '0-9' < "$TMP/legacy-settings-settings-ms.txt")"
check_ge "A/B ③ 再読み込みの間の tako list は待たされる（ms）" "$((SLOW_MS / 2))" "$(max_of "$TMP/legacy-settings-ipc.txt")"
PERF="$TMP/legacy-settings-perf.log"
check_eq "A/B ③ perf.log に dispatch:RemoteStatus の専有が出る" 1 "$(perf_hits 'dispatch:RemoteStatus')"
PERF="$TMP/data/perf.log"

echo
echo "== 応答の中身は修正前（A/B）と同じ =="
check_eq "① stop の応答" "$(cat "$TMP/legacy-stop.txt")" "$(cat "$TMP/new-stop.txt")"
check_eq "② status の応答（実行ごとに変わる値を落として）" "$(cat "$TMP/legacy-status.norm")" "$(cat "$TMP/new-status.norm")"
check_eq "④ 止まっている daemon への stop の断り" "$(cat "$TMP/legacy-stop-idle.txt")" "$(cat "$TMP/new-stop-idle.txt")"
check_eq "④ 止まっている daemon への stop --force の断り" "$(cat "$TMP/legacy-force-idle.txt")" "$(cat "$TMP/new-force-idle.txt")"
check_eq "④ 止まっている間の status" "$(cat "$TMP/legacy-status-idle.norm")" "$(cat "$TMP/new-status-idle.norm")"

# 証拠を残す（TAKO_2630_EVIDENCE_DIR=<dir>）
if [ -n "${TAKO_2630_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2630_EVIDENCE_DIR"
  for r in new legacy; do
    for f in stop.txt stop-ipc.txt stop-ms.txt stop-perf.log status.txt status.norm status-ipc.txt status-ms.txt \
      status-perf.log settings-ipc.txt settings-settings-ms.txt settings-perf.log stop-idle.txt force-idle.txt status-idle.norm; do
      if [ -e "$TMP/$r-$f" ]; then command cp -f "$TMP/$r-$f" "$TAKO_2630_EVIDENCE_DIR/"; fi
    done
  done
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
