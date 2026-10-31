#!/usr/bin/env bash
# test-remote-status-starting-2608.sh — daemon を起こしている途中の status の実経路テスト（#2608）
#
# 隔離した state / data ディレクトリ・偽の tailscale CLI（TAKO_TAILSCALE_BIN）で
# **実 tako-app（隔離 GUI）+ 実 daemon** を走らせ、GUI 起動時の自動復帰（#1485）が旧版の daemon を
# 新版へ入れ替える（#2518）途中に `tako remote status` を読み続ける。区間は偽の tailscale の
# `serve --bg` に待ちを入れて伸ばす（新しい daemon は待ち受けを開いてから serve を張り終えて
# PID ファイルを書く = その間が #2608 の区間。#2466 の実測は 8 秒）。
#   ① 入れ替えの途中: status は `starting`（replacing）を返し、`pidless_listener` も `tako remote stop` の
#      案内も出さない（JSON の warnings と CLI の stderr の両方）。同じ区間を A/B（TAKO_2608_LEGACY=1）の
#      読み手で読むと #2608 前の答え（pidless_listener + stop の案内）= 症状が同じ実行で再現する
#   ② 入れ替えが終わったら `starting` は消え、印のファイルも残らない
#   ③ 陽性対照: 本当に PID ファイルの無い待ち受け（#2455）は今までどおり pidless_listener と stop の案内
#   ④ GUI が入れ替えの途中で落ちたら「起動中」のまま残らない（書き手の死で印が効かない）
#   ⑤ 入れ替えが失敗したら「起動中」のまま残らない（印は spawn_daemon を抜けると消える）
#   ⑥ 「起動していた」が無い（desired=false）なら GUI を起こしても何も起こさず `starting` も出ない
# を実測する。修正前のバイナリ（TAKO_BIN / APP_BIN を差し替える）では ① が落ちる。
#
# **本番の tailscale / serve 設定 / remote デーモン / GUI には一切触らない**
# （tailscale の呼び出しは全部この偽物へ向く。state・data・発見の置き場は mktemp 配下。
# 落とすのは自分で起こした GUI と、この state で起きた daemon（audit.log に残る pid）だけ）。
# 窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-remote-status-starting-2608.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。区間の長さは TAKO_2608_SLOW_SECS（既定 8）
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2608_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_ge() {
  if [ "$3" -ge "$2" ]; then pass "$1（${3} 回）"; else fail "$1（${2} 回以上のはずが ${3} 回）"; fi
}

SLOW="${TAKO_2608_SLOW_SECS:-8}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2608-XXXXXX)"
TMUX_SOCKET="tako-iso-2608-$$"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

STATE="$TMP/state"
BIN_DIR="$TMP/bin"
mkdir -p "$STATE" "$TMP/data" "$TMP/disc" "$BIN_DIR" "$TMP/ctl"
# 隔離モードの serve_binary() は tako-app と同じディレクトリの tako を選ぶので 2 つを並べる。
# 検査に使う CLI は差し替えの影響を受けない複製
command cp -f "$APP_BIN" "$BIN_DIR/tako-app"
command cp -f "$TAKO_BIN" "$BIN_DIR/tako"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
APP_BIN="$BIN_DIR/tako-app"
CLI="$TMP/ctl/tako"

# この state で起きた daemon の pid（audit.log の起動の行）。後片付けはこれと自分の GUI だけ
my_daemon_pids() {
  jq -r 'select(.event == "daemon_startup_env" or .event == "serve_set") | .pid' "$STATE/audit.log" 2>/dev/null | sort -u
}
cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  "$CLI" remote stop >/dev/null 2>&1 || true
  local p
  for p in $(my_daemon_pids); do
    if kill -0 "$p" 2>/dev/null; then kill "$p" 2>/dev/null || true; fi
  done
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# --- 偽 tailscale CLI（serve 設定は $TMP/serve.json）---------------------------
# `serve --bg` は $TMP/slow 秒待つ（区間を伸ばす注入）。$TMP/fail が在れば待った後に失敗する
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
NODE="fake-2608.tailfake.ts.net"
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
        if [ -e "$STATE/fail" ]; then echo "serve を張れません（検証の注入）" >&2; exit 1; fi
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "${3:-}" > "$SERVE"
        exit 0 ;;
      --https=443)
        if [ "${2:-}" = "off" ]; then echo '{}' > "$SERVE"; exit 0; fi
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
# 自動復帰の 1 回目の前の待ち（既定 2 秒）を縮める
export TAKO_1485_BACKOFF_SECS=1
case "$STATE" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $STATE"; exit 1 ;;
esac

pid_now() { head -1 "$STATE/tako-remote.pid" 2>/dev/null | tr -dc '0-9'; }
field() { jq -r "$2 | if . == null then \"null\" else tostring end" "$1" 2>/dev/null; }

# 旧版の daemon を CLI で起こし、実行ファイルを同じパスのまま別のファイルへ差し替える（install の形）
start_stale_daemon() {
  echo 0 > "$TMP/slow"
  rm -f "${TMP:?}/fail"
  if ! "$BIN_DIR/tako" remote start >"$TMP/start.json" 2>"$TMP/start.err"; then
    echo "daemon を起動できなかった:"; cat "$TMP/start.err"; exit 1
  fi
  command cp -f "$TAKO_BIN" "$BIN_DIR/tako.new"
  command mv -f "$BIN_DIR/tako.new" "$BIN_DIR/tako"
}

# status を読み続ける（$1 = 書き先・$2 = 上限の秒・$3 = 止める条件の jq）。
# 1 回ごとに修正後の読み手（JSON と stderr）と A/B の読み手（TAKO_2608_LEGACY=1）の両方で読む
sample() {
  local dir="$1" budget="$2" stop_when="$3" n=0 start
  mkdir -p "$dir"
  start=$(date +%s)
  while [ $(($(date +%s) - start)) -lt "$budget" ]; do
    n=$((n + 1))
    "$CLI" remote status >"$dir/$n.json" 2>"$dir/$n.err"
    TAKO_2608_LEGACY=1 "$CLI" remote status >"$dir/$n.legacy.json" 2>/dev/null
    if jq -e "$stop_when" "$dir/$n.json" >/dev/null 2>&1; then break; fi
    sleep 0.2
  done
  echo "$n"
}
# 読みのうち jq の条件に当たる数（$1 = 置き場・$2 = 接尾辞・$3 = 条件）
count_reads() {
  local dir="$1" suffix="$2" cond="$3" f c=0
  for f in "$dir"/*"$suffix"; do
    [ -e "$f" ] || continue
    case "$suffix" in
      .json) case "$f" in *.legacy.json) continue ;; esac ;;
    esac
    if jq -e "$cond" "$f" >/dev/null 2>&1; then c=$((c + 1)); fi
  done
  echo "$c"
}
STOP_GUIDED='(.warnings // []) | any(test("tako remote stop"))'

echo
echo "== ① 自動復帰が旧版を入れ替える途中の status =="
start_stale_daemon
PID1="$(pid_now)"
"$CLI" remote status >"$TMP/before.json" 2>/dev/null
check_eq "差し替え後の daemon は旧版" "replaced" "$(field "$TMP/before.json" .serve_generation.state)"
echo "$SLOW" > "$TMP/slow"
launch_isolated_gui "$TMP/app1.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
W1="$TMP/w1"
READS="$(sample "$W1" $((SLOW + 40)) '.running == true and .pid != '"${PID1:-0}"' and .starting == null and .last_autostart.reason == "replaced-generation"')"
echo "  （status を ${READS} 回読んだ）"
check_ge "途中の読みで新しい daemon の待ち受けが応答している間を「入れ替え中」と答える" 1 \
  "$(count_reads "$W1" .json '.running == false and .starting.state == "replacing" and .starting.listener != null')"
check_eq "途中のどの読みも pidless_listener を返さない" 0 "$(count_reads "$W1" .json '.pidless_listener != null')"
check_eq "途中のどの読みも warnings で tako remote stop を案内しない" 0 "$(count_reads "$W1" .json "$STOP_GUIDED")"
ERR_STOP=0
for f in "$W1"/*.err; do
  if grep -q "tako remote stop" "$f" 2>/dev/null; then ERR_STOP=$((ERR_STOP + 1)); fi
done
check_eq "CLI の stderr も tako remote stop を案内しない" 0 "$ERR_STOP"
ERR_WAIT=0
for f in "$W1"/*.err; do
  if grep -q "入れ替えている途中" "$f" 2>/dev/null; then ERR_WAIT=$((ERR_WAIT + 1)); fi
done
check_ge "CLI の stderr は入れ替えの途中と言う" 1 "$ERR_WAIT"
check_ge "A/B（TAKO_2608_LEGACY=1）の読み手は同じ区間で pidless_listener + stop の案内（#2608 の症状）" 1 \
  "$(count_reads "$W1" .legacy.json ".pidless_listener != null and ($STOP_GUIDED)")"

echo
echo "== ② 入れ替えが終わったら starting は消える =="
"$CLI" remote status >"$TMP/after.json" 2>/dev/null
PID2="$(pid_now)"
check_eq "新しい daemon が動いている" "true" "$(field "$TMP/after.json" .running)"
if [ -n "$PID2" ] && [ "$PID2" != "$PID1" ]; then pass "pid が替わった（${PID1} → ${PID2}）"; else fail "pid が替わっていない（${PID1} → ${PID2}）"; fi
check_eq "入れ替えた記録が残る" "replaced-generation" "$(field "$TMP/after.json" .last_autostart.reason)"
check_eq "終わった後は starting を返さない" "null" "$(field "$TMP/after.json" .starting)"
if [ -e "$STATE/tako-remote.starting" ]; then fail "印のファイルが残っている"; else pass "印のファイルは残らない"; fi
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ③ 陽性対照: 本当に PID ファイルの無い待ち受け（#2455）=="
rm -f "${STATE:?}/tako-remote.pid"
"$CLI" remote status >"$TMP/pidless.json" 2>/dev/null
check_eq "pidless_listener を返す" "true" "$(jq -r '.pidless_listener != null' "$TMP/pidless.json")"
check_eq "tako remote stop を案内する" "true" "$(jq -r "$STOP_GUIDED" "$TMP/pidless.json")"
check_eq "starting は返さない" "null" "$(field "$TMP/pidless.json" .starting)"
"$CLI" remote stop >/dev/null 2>&1
"$CLI" remote status >"$TMP/stopped.json" 2>/dev/null
check_eq "stale 経路の stop で止まる（後片付け）" "null" "$(field "$TMP/stopped.json" .pidless_listener)"

echo
echo "== ④ GUI が入れ替えの途中で落ちたら「起動中」のまま残らない =="
start_stale_daemon
echo "$SLOW" > "$TMP/slow"
launch_isolated_gui "$TMP/app2.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
READS="$(sample "$TMP/w4" $((SLOW + 30)) '.starting.listener != null')"
check_eq "入れ替えの途中（新しい daemon の待ち受けが応答）を捉えた" "true" "$(jq -r '.starting.listener != null' "$TMP/w4/$READS.json" 2>/dev/null)"
# 落ちる形を作る（自分の GUI の pid だけを SIGKILL。ペインは落とす前に控えて後で片付ける）
PANES="$(iso_pane_processes "$APP_PID")"
kill -9 "$APP_PID" 2>/dev/null
wait "$APP_PID" 2>/dev/null
APP_PID=""
iso_reap_panes "$PANES"
"$CLI" remote status >"$TMP/crashed.json" 2>/dev/null
check_eq "書き手が落ちた後は starting を返さない" "null" "$(field "$TMP/crashed.json" .starting)"
if [ "$(jq -r '.running' "$TMP/crashed.json")" = "true" ]; then
  pass "（新しい daemon は PID ファイルまで書き終えていた）"
else
  # 新しい daemon はまだ serve を張っている途中 = 待ち受けが応答している。見守る書き手が居ないので
  # #2455 と同じ答え（止める判断は人に返す）
  check_eq "書き手が落ちた後の待ち受けは #2455 の案内へ戻る" "true" "$(jq -r '.pidless_listener != null and ('"$STOP_GUIDED"')' "$TMP/crashed.json")"
fi
"$CLI" remote stop >/dev/null 2>&1
sleep "$SLOW"
"$CLI" remote stop >/dev/null 2>&1

echo
echo "== ⑤ 入れ替えが失敗したら「起動中」のまま残らない =="
start_stale_daemon
echo 2 > "$TMP/slow"
: > "$TMP/fail"
launch_isolated_gui "$TMP/app3.log" TAKO_1485_ATTEMPTS=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
READS="$(sample "$TMP/w5" 40 '.last_autostart.result == "failed"')"
check_eq "自動復帰は失敗を記録した" "failed" "$(field "$TMP/w5/$READS.json" .last_autostart.result)"
check_ge "失敗する前の途中は「入れ替え中」と答えていた" 1 "$(count_reads "$TMP/w5" .json '.starting.state == "replacing"')"
check_eq "失敗した後は starting を返さない" "null" "$(field "$TMP/w5/$READS.json" .starting)"
if [ -e "$STATE/tako-remote.starting" ]; then fail "失敗した後に印のファイルが残っている"; else pass "失敗した後に印のファイルは残らない"; fi
stop_isolated_gui "$APP_PID"
APP_PID=""
rm -f "${TMP:?}/fail"

echo
echo "== ⑥ 「起動していた」が無ければ何も起こさず starting も出ない =="
"$CLI" remote stop >/dev/null 2>&1
"$CLI" remote status >"$TMP/nodesired.json" 2>/dev/null
check_eq "desired は消えている" "false" "$(field "$TMP/nodesired.json" .desired)"
launch_isolated_gui "$TMP/app4.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
sleep 6
"$CLI" remote status >"$TMP/nodesired2.json" 2>/dev/null
check_eq "GUI を起こしても立ち上がらない" "false" "$(field "$TMP/nodesired2.json" .running)"
check_eq "starting を返さない" "null" "$(field "$TMP/nodesired2.json" .starting)"
if [ -e "$STATE/tako-remote.starting" ]; then fail "印のファイルが在る"; else pass "印のファイルは無い"; fi
stop_isolated_gui "$APP_PID"
APP_PID=""

# 証拠を残す（TAKO_2608_EVIDENCE_DIR=<dir>）。① の読みの全部と各段の status
if [ -n "${TAKO_2608_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2608_EVIDENCE_DIR"
  command cp -Rf "$W1" "$TAKO_2608_EVIDENCE_DIR/replacing-reads"
  for f in before after pidless crashed nodesired2; do
    if [ -e "$TMP/$f.json" ]; then command cp -f "$TMP/$f.json" "$TAKO_2608_EVIDENCE_DIR/"; fi
  done
  if [ -d "$TMP/w5" ]; then command cp -Rf "$TMP/w5" "$TAKO_2608_EVIDENCE_DIR/failed-reads"; fi
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
