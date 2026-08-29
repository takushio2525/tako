#!/usr/bin/env bash
# test-serve-watch.sh — serve 設定の自己検査と自動 re-assert のモックテスト（#1049）
#
# 偽の tailscale CLI（TAKO_TAILSCALE_BIN）と隔離した state ディレクトリで
# 実 daemon（tako remote serve）を走らせ、「消える → 検知 → 張り直す」を実測する。
# **本番の tailscale / serve 設定 / remote デーモンには一切触らない**
# （tailscale の呼び出しは全部この偽物へ向く。state も mktemp 配下）。
#
# 使い方: bash scripts/test-serve-watch.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

check_eq() {
  # check_eq <説明> <期待> <実際>
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

check_contains() {
  # check_contains <説明> <文字列> <部分文字列>
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1049-XXXXXX")"
cleanup() {
  stop_daemon || true
  if [ -n "${PEER_PID:-}" ]; then kill "$PEER_PID" 2>/dev/null || true; fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# --- 偽 tailscale CLI ---------------------------------------------------------
# serve 設定は $TMP/serve.json に持つ（外から書き換えられる = 「消える」を再現できる）
FAKE_TS="$TMP/fake-tailscale"
echo '{}' > "$TMP/serve.json"
cat > "$FAKE_TS" <<'FAKE'
#!/usr/bin/env bash
# 偽 tailscale。2 系統の tailscaled（別ノード）の同居を再現できる:
#   flip ファイルあり → **既定探索だけ**が別ノード（B）を返す（#1049 の入れ替わり）
#   gone ファイルあり → どちらもノード B（= 公開したノードが消える）
set -u
STATE="$(dirname "$0")"
SOCK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --socket) SOCK="$2"; shift 2 ;;
    *) break ;;
  esac
done
NODE="fake-1049.tailfake.ts.net"
SERVE="$STATE/serve.json"
if [ -f "$STATE/gone" ] || { [ -f "$STATE/flip" ] && [ -z "$SOCK" ]; }; then
  NODE="fake-1049-1.tailfake.ts.net"
  SERVE="$STATE/serve-b.json"
fi
[ -f "$SERVE" ] || echo '{}' > "$SERVE"
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
        target="${3:-}"
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "$target" > "$SERVE"
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

# --- 対象バイナリ -------------------------------------------------------------
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
if [ ! -x "$TAKO_BIN" ]; then
  echo "tako CLI をビルドします（${TAKO_BIN}）…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli --quiet)
fi
[ -x "$TAKO_BIN" ] || { echo "tako CLI が見つからない: $TAKO_BIN"; exit 1; }

# --- 隔離した環境 -------------------------------------------------------------
export TAKO_REMOTE_STATE_DIR="$TMP/state"
export TAKO_TAILSCALE_BIN="$FAKE_TS"
export TAKO_1049_WATCH_SECS=2
mkdir -p "$TAKO_REMOTE_STATE_DIR"
# 本番へ書かない不変条件（万一 env が効いていなければここで落とす）
case "$TAKO_REMOTE_STATE_DIR" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $TAKO_REMOTE_STATE_DIR"; exit 1 ;;
esac

DAEMON_LOG="$TMP/daemon.log"

serve_target() { python3 -c 'import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: print(""); raise SystemExit
w=d.get("Web") or {}
for k,v in w.items():
    h=(v.get("Handlers") or {}).get("/") or {}
    print(h.get("Proxy") or ""); raise SystemExit
print("")' "${1:-$TMP/serve.json}"; }

status_field() { "$TAKO_BIN" remote status 2>/dev/null | python3 -c 'import json,sys
d=json.load(sys.stdin)
cur=d
for k in sys.argv[1].split("."):
    if isinstance(cur, dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$1"; }

start_daemon() {
  echo '{}' > "$TMP/serve.json"
  echo '{}' > "$TMP/serve-b.json"
  rm -f "$TMP/flip" "$TMP/gone"
  rm -f "$TAKO_REMOTE_STATE_DIR"/tako-remote.* 2>/dev/null || true
  # **本番と同じ起動経路**（`remote start` = spawn_daemon）を使う。
  # spawn_daemon は起動情報 JSON を読んだあと子の stdout / stderr の pipe を閉じるので、
  # 以後 daemon が println! / eprintln! を呼ぶと EPIPE で panic する（#1049 では実際に
  # 自己検査スレッドが黙って死んだ）。`remote serve` を直接叩いてログをファイルへ
  # 逃がすと**この条件が再現しない**ので、あえて本番経路を通す。
  # TAKO_ISOLATED=1 は serve_binary の解決を検証対象の自世代に固定する（/Applications へ飛ばさない）
  if ! TAKO_ISOLATED=1 "$TAKO_BIN" remote start > "$DAEMON_LOG" 2>&1; then
    echo "daemon を起動できなかった:"; cat "$DAEMON_LOG"; return 1
  fi
  for _ in $(seq 1 200); do
    if [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.url" ]; then
      if [ "${TAKO_1049_LEGACY:-}" = "1" ] || [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.serve" ]; then return 0; fi
    fi
    sleep 0.1
  done
  echo "daemon の起動を待てなかった:"; cat "$DAEMON_LOG"; return 1
}

stop_daemon() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop > /dev/null 2>&1 || true
  fi
}

# 期待する serve target に戻るまで待ち、かかった秒数を返す（戻らなければ空）
wait_restore() {
  want="$1"; limit="$2"
  start=$(date +%s)
  while [ $(( $(date +%s) - start )) -lt "$limit" ]; do
    if [ "$(serve_target)" = "$want" ]; then echo $(( $(date +%s) - start )); return 0; fi
    sleep 0.2
  done
  echo ""; return 1
}

echo "=== Test 1: 消えた serve 設定を検知して自動で張り直す ==="
start_daemon || exit 1
OURS="$(serve_target)"
check_contains "起動時に自分の到達先へ serve が張られる" "$OURS" "http://127.0.0.1:"
check_eq "起動直後の status は serve_ok=true" "true" "$(status_field serve_ok)"
echo '{}' > "$TMP/serve.json"   # 外から消す（= #1049 の症状）
check_eq "消した直後は設定が無い" "" "$(serve_target)"
ELAPSED="$(wait_restore "$OURS" 20 || true)"
if [ -n "$ELAPSED" ]; then pass "${ELAPSED} 秒で張り直された（上限 20 秒）"; else fail "20 秒経っても張り直されない"; fi
check_eq "張り直し後も serve_ok=true" "true" "$(status_field serve_ok)"
check_eq "張り直し回数が記録される" "1" "$(status_field serve_reasserts)"
AUDIT="$(cat "$TAKO_REMOTE_STATE_DIR/audit.log" 2>/dev/null || true)"
check_contains "audit.log に serve_reasserted が残る" "$AUDIT" "serve_reasserted"
check_contains "audit.log に誰が張り直したか（pid）が残る" "$AUDIT" '"pid"'
# 張り直した**あとも**検査が回り続けること（#1049: 通知の 1 行で自己検査スレッドが
# EPIPE panic して黙って死に、`stale` になるまで誰も気づけなかった）
sleep 8
AGE="$(status_field serve_checked_age_secs)"
if [ "${AGE:-999}" -le 7 ]; then pass "張り直した後も自己検査が回り続ける（最終検査 ${AGE} 秒前）"; else fail "自己検査が止まっている（最終検査 ${AGE} 秒前）"; fi
stop_daemon

echo
echo "=== Test 2 (A/B): TAKO_1049_LEGACY=1 では検知も張り直しもしない ==="
export TAKO_1049_LEGACY=1
start_daemon || exit 1
OURS="$(serve_target)"
echo '{}' > "$TMP/serve.json"
ELAPSED="$(wait_restore "$OURS" 12 || true)"
if [ -z "$ELAPSED" ]; then pass "旧挙動では張り直されない（12 秒待って復帰なし）"; else fail "旧挙動なのに ${ELAPSED} 秒で張り直された"; fi
check_eq "旧挙動では serve_ok を名乗らない" "null" "$(status_field serve_ok)"
# **無説明の null にしない**（新 CLI × 自己検査なし daemon で「黙る」を残さない）
check_eq "検査していないことが状態に出る" "unchecked" "$(status_field serve_state)"
check_contains "理由と次の一手が出る" "$(status_field serve_note)" "tako remote stop"
check_eq "検査が無いだけで劣化とは言わない" "null" "$(status_field degraded)"
stop_daemon
unset TAKO_1049_LEGACY

echo
echo "=== Test 3: ユーザーが張った設定は触らず、劣化として報告する ==="
start_daemon || exit 1
printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"fake-1049.tailfake.ts.net:443":{"Handlers":{"/":{"Proxy":"http://192.168.1.5:8080"}}}}}\n' > "$TMP/serve.json"
sleep 6
check_eq "ユーザー設定を上書きしない" "http://192.168.1.5:8080" "$(serve_target)"
check_eq "serve_ok=false" "false" "$(status_field serve_ok)"
check_eq "状態は foreign" "foreign" "$(status_field serve_state)"
check_contains "理由が出る" "$(status_field degraded.reason)" "tako 管理外"
check_contains "次の一手が出る" "$(status_field degraded.next_step)" "tailscale serve"
stop_daemon

echo
echo "=== Test 4: 生きている別プロセスが :443 を持っていたら張り合わない ==="
cat > "$TMP/peer.py" <<'PEERPY'
import socket, sys, time
s = socket.socket()
s.bind(("127.0.0.1", 0))
s.listen(8)
with open(sys.argv[1], "w") as f:
    f.write(str(s.getsockname()[1]))
while True:
    time.sleep(1)
PEERPY
python3 "$TMP/peer.py" "$TMP/peer.port" > "$TMP/peer.log" 2>&1 &
PEER_PID=$!
PEER_PORT=""
for _ in $(seq 1 50); do
  if [ -s "$TMP/peer.port" ]; then PEER_PORT="$(cat "$TMP/peer.port")"; break; fi
  sleep 0.1
done
if [ -z "$PEER_PORT" ]; then
  fail "対向プロセスのポートを取れなかった（Test 4 をスキップ）"
else
  start_daemon || exit 1
  printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"fake-1049.tailfake.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:%s"}}}}}\n' "$PEER_PORT" > "$TMP/serve.json"
  sleep 6
  check_eq "応答している相手からは奪い返さない" "http://127.0.0.1:$PEER_PORT" "$(serve_target)"
  check_eq "状態は taken_over" "taken_over" "$(status_field serve_state)"
  check_eq "張り直し回数は 0 のまま" "0" "$(status_field serve_reasserts)"
  stop_daemon
fi
kill "$PEER_PID" 2>/dev/null || true
PEER_PID=""

echo
echo "=== Test 5: 消され続けても上限で止まる（無限ループ防止） ==="
start_daemon || exit 1
OURS="$(serve_target)"
WIPE_UNTIL=$(( $(date +%s) + 40 ))
( while [ "$(date +%s)" -lt "$WIPE_UNTIL" ]; do echo '{}' > "$TMP/serve.json"; sleep 0.5; done ) &
WIPER_PID=$!
GAVE_UP=""
for _ in $(seq 1 60); do
  if [ "$(status_field serve_state)" = "missing" ]; then GAVE_UP=1; break; fi
  sleep 1
done
kill "$WIPER_PID" 2>/dev/null || true
wait "$WIPER_PID" 2>/dev/null || true
if [ -n "$GAVE_UP" ]; then pass "上限に達したら張り直しをやめる"; else fail "上限に達しても missing にならない"; fi
check_eq "諦めたあとは serve_ok=false" "false" "$(status_field serve_ok)"
check_contains "理由に「繰り返し消え」が出る" "$(status_field degraded.reason)" "繰り返し消え"
REASSERTS="$(status_field serve_reasserts)"
if [ "$REASSERTS" = "5" ]; then pass "張り直しは上限（5 回）で止まる"; else fail "張り直し回数が上限と違う: $REASSERTS"; fi
AUDIT="$(cat "$TAKO_REMOTE_STATE_DIR/audit.log" 2>/dev/null || true)"
check_contains "audit.log に serve_reassert_gave_up が残る" "$AUDIT" "serve_reassert_gave_up"
stop_daemon

echo
echo "=== Test 6: 既定探索の相手が入れ替わっても公開したノードを見続ける（#1049 の根因） ==="
start_daemon || exit 1
OURS="$(serve_target)"
check_contains "起動時はノード A へ張る" "$OURS" "http://127.0.0.1:"
touch "$TMP/flip"   # 既定探索だけが別ノード（B）を返すようになる
sleep 6
check_eq "ノード A の設定はそのまま" "$OURS" "$(serve_target "$TMP/serve.json")"
check_eq "ノード B へ張りに行かない" "" "$(serve_target "$TMP/serve-b.json")"
check_eq "入れ替わっても serve_ok=true" "true" "$(status_field serve_ok)"
check_contains "見ている相手はノード A" "$(status_field serve_node)" "fake-1049.tailfake"
stop_daemon
check_eq "停止でノード A の設定が掃除される" "" "$(serve_target "$TMP/serve.json")"

echo
echo "=== Test 7 (A/B): 旧挙動では入れ替わり後の停止で設定が消し残る ==="
export TAKO_1049_LEGACY=1
start_daemon || exit 1
OURS="$(serve_target)"
touch "$TMP/flip"
stop_daemon
LEFT="$(serve_target "$TMP/serve.json")"
if [ "$LEFT" = "$OURS" ]; then pass "旧挙動ではノード A の設定が消し残る（${LEFT}）"; else fail "旧挙動なのに掃除された（${LEFT}）"; fi
echo '{}' > "$TMP/serve.json"
unset TAKO_1049_LEGACY

echo
echo "=== Test 8: 公開したノードがどこにも居なければ正直に劣化を出す ==="
start_daemon || exit 1
touch "$TMP/gone"
sleep 6
check_eq "serve_ok=false" "false" "$(status_field serve_ok)"
check_eq "状態は node_missing" "node_missing" "$(status_field serve_state)"
check_contains "理由にノード名が出る" "$(status_field degraded.reason)" "fake-1049.tailfake"
rm -f "$TMP/gone"
stop_daemon

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
