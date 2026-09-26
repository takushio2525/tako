#!/usr/bin/env bash
# test-remote-role-1452.sh — 権限リクエストと role 編集の実経路テスト（#1452）
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+
# 実 tako-app** を走らせ、HTTP と CLI と MCP を実際に叩いて
#   ① observe 端末は `/api/files` で 403（権限が足りない状態を作る）
#   ② `POST /api/pair {role, reason}` で承認待ちができ、`/api/me` が pending を返す
#      （**登録済み端末にも返る**のが #1452。以前は未登録の枝にしか無かった）
#   ③ 依頼が `tako todo`（kind=permission）へ 1 件だけ起票される（二重リクエストで増えない）
#   ④ **昇格は CLI からも MCP からも curl からも通らない**（#1452 の安全側の不変条件）
#   ⑤ tako-app として名乗った承認だけが通り、承認後は `/api/files` が 200・todo が done
#   ⑥ **降格は CLI / MCP から通る**（1:1）。監査に caller_check が残る
#   ⑦ エッジ: 現状以下の要求 / 承認前の端末削除 / daemon 再起動をまたいだ保留
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定 / ~/.ssh には一切触らない**
# （tailscale の呼び出しは偽物へ、HOME / state / data / orchestrator は mktemp 配下）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-remote-role-1452.sh
set -euo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）。
# 下の「タブ 1 枚か」の関門が第 2 の網
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}
check_not_contains() {
  case "$2" in
    *"$3"*) fail "$1（'${3}' を含んでしまった: ${2}）" ;;
    *) pass "$1" ;;
  esac
}

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1452-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1452-$$"
cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# --- 偽 tailscale CLI ---------------------------------------------------------
FAKE_TS="$TMP/fake-tailscale"
echo '{}' > "$TMP/serve.json"
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
NODE="fake-1452.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nPHONEA","Name":"phone-a.tailfake.ts.net.","Hostinfo":{"Hostname":"phone-a"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
    exit 0 ;;
  serve)
    shift
    case "${1:-}" in
      status) cat "$STATE/serve.json"; exit 0 ;;
      --bg)
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "${3:-}" > "$STATE/serve.json"
        exit 0 ;;
      --https=443) echo '{}' > "$STATE/serve.json"; exit 0 ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$FAKE_TS"

# --- 対象バイナリ -------------------------------------------------------------
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_REMOTE_STATE_DIR="$TMP/remote"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_USER_TASKS_FILE="$TMP/orch/user-tasks.yaml"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_PERSIST=0
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# #841: ループバック TCP では XFF を読む前に接続元プロセスを検証する。
# この隔離環境で serve の代わりに繋いでくるのは curl だと**名前で宣言**する
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
# #1452: 昇格は「呼び出し元が tako-app か」で判断する。ここでは実 GUI を使えないので、
# **python3 を GUI 側の名前として宣言**し、GUI 相当 = python3 / それ以外 = curl で撃ち分ける
# （curl を `tako-app` へコピーする案は macOS の署名検証で SIGKILL される = 実測）。
# 名前ゲート自体（`tako-app` / `tako-app.exe` / 大文字小文字）は remote_role の単体テストが見る。
# **実体名で宣言する**: `/opt/homebrew/bin/python3` は symlink で、接続元から引ける
# 実行ファイル名は実体側（`python3.14`）や framework の `Python` になる（実測で
# 版とビルドによって散る）ので、候補を並べて宣言する
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="python,python3,$(python3 -c 'import sys,os; print(os.path.basename(os.path.realpath(sys.executable)))')"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 起動 ---------------------------------------------------------------------
echo "=== 準備: 隔離 tako-app と daemon を起動 ==="
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
BOOT_TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$BOOT_TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。中止する。"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・タブ 1 枚）"

start_daemon() {
  "$TAKO_BIN" remote start > "$TMP/daemon.log" 2>&1 || {
    echo "daemon を起動できなかった:"; cat "$TMP/daemon.log"; exit 1; }
  for _ in $(seq 1 200); do
    [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.port" ] && break
    sleep 0.1
  done
  PORT="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || { echo "daemon の port が読めない:"; cat "$TMP/daemon.log"; exit 1; }
  ADMIN_TOKEN="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.token")"
  BASE="http://127.0.0.1:$PORT"
  for _ in $(seq 1 100); do
    [ "$(api GET /api/health)" = "200" ] && break
    sleep 0.1
  done
}

api() {
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: fake-1452.tailfake.ts.net" \
      -H 'Content-Type: application/json' -d "$body"
  else
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: fake-1452.tailfake.ts.net"
  fi
}
# 管理 API（XFF を付けない = ローカル直結）
admin() {
  local path="$1" body="${2:-{\}}"
  curl -s -o "$TMP/body" -w '%{http_code}' -X POST "$BASE$path" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' -d "$body"
}
# GUI 相当（= 表で宣言した名前のプロセス）から管理 API を叩く
gui_admin() {
  python3 - "$BASE$1" "$ADMIN_TOKEN" "${2:-{\}}" "$TMP/body" <<'PYADMIN'
import json, sys, urllib.request, urllib.error
url, token, body, out = sys.argv[1:5]
req = urllib.request.Request(url, data=body.encode(), method="POST",
                             headers={"X-Tako-Admin": token, "Content-Type": "application/json"})
try:
    with urllib.request.urlopen(req, timeout=10) as r:
        open(out, "w").write(r.read().decode()); print(r.status)
except urllib.error.HTTPError as e:
    open(out, "w").write(e.read().decode()); print(e.code)
PYADMIN
}
body() { cat "$TMP/body"; }
jqf() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur,list):
        cur = cur[int(k)] if int(k) < len(cur) else None
    elif isinstance(cur,dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$TMP/body" "$1"; }
jqs() { python3 -c 'import json,sys
d=json.loads(sys.stdin.read())
cur=d
for k in sys.argv[1].split("."):
    if isinstance(cur,list):
        cur = cur[int(k)] if int(k) < len(cur) else None
    elif isinstance(cur,dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$1"; }

start_daemon
pass "隔離 daemon が起動した（loopback port ${PORT}）"

# 端末を observe で登録する（承認は「GUI として名乗った」curl で行う）
api POST /api/pair '{"name":"phone-a","role":"observe"}' >/dev/null
gui_admin /api/admin/pair/approve '{"device_id":"nPHONEA","role":"observe"}' >/dev/null
CODE="$(api GET /api/me)"
check_eq "observe として登録された" "observe" "$(jqf role)"

echo
echo "=== Test 1: 権限が足りない状態と、その申告 ==="
CODE="$(api GET /api/files)"
check_eq "observe ではファイル参照が 403" "403" "$CODE"
CODE="$(api GET /api/me)"
check_eq "登録済みでも pending が返る（false）" "false" "$(jqf pending)"

CODE="$(api POST /api/pair '{"name":"phone-a","role":"interact","reason":"出先でログを見たい"}')"
check_eq "権限の更新をリクエストできる" "200" "$CODE"
check_eq "承認待ちになる" "pending" "$(jqf status)"
CODE="$(api GET /api/me)"
check_eq "**登録済み端末にも**承認待ちが返る（#1452 の中身）" "true" "$(jqf pending)"
check_eq "要求した role も返る" "interact" "$(jqf requested_role)"
check_eq "role はまだ上がっていない" "observe" "$(jqf role)"

echo
echo "=== Test 2: CLI の一覧に理由と現在の role が載る ==="
LIST="$("$TAKO_BIN" remote devices list)"
check_eq "保留が 1 件" "1" "$(printf '%s' "$LIST" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["pending"]))')"
check_eq "要求 role" "interact" "$(printf '%s' "$LIST" | jqs pending.0.requested_role)"
check_eq "現在の role" "observe" "$(printf '%s' "$LIST" | jqs pending.0.current_role)"
check_eq "理由" "出先でログを見たい" "$(printf '%s' "$LIST" | jqs pending.0.reason)"
check_eq "種別は昇格" "upgrade" "$(printf '%s' "$LIST" | jqs pending.0.kind)"

echo
echo "=== Test 3: 依頼がユーザータスクへ 1 件だけ起票される ==="
TODO="$("$TAKO_BIN" todo list --kind permission --json 2>/dev/null || echo '{}')"
check_eq "permission のタスクが 1 件" "1" "$(printf '%s' "$TODO" | python3 -c 'import json,sys
d=json.load(sys.stdin); print(len(d.get("tasks",[])))')"
check_contains "題に端末名と要求 role が入る" "$(printf '%s' "$TODO" | jqs tasks.0.title)" "phone-a"
check_contains "本文に理由が入る（監査ログには載せない値）" "$(printf '%s' "$TODO" | jqs tasks.0.body)" "出先でログを見たい"
# 二重リクエスト: 同じ端末・同じ要求でもう一度送っても増えない
api POST /api/pair '{"name":"phone-a","role":"interact","reason":"もう一度"}' >/dev/null
TODO2="$("$TAKO_BIN" todo list --kind permission --json 2>/dev/null || echo '{}')"
check_eq "二重リクエストでもタスクは 1 件のまま" "1" "$(printf '%s' "$TODO2" | python3 -c 'import json,sys
d=json.load(sys.stdin); print(len(d.get("tasks",[])))')"
LIST="$("$TAKO_BIN" remote devices list)"
check_eq "保留も 1 件のまま（上書き）" "1" "$(printf '%s' "$LIST" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["pending"]))')"

echo
echo "=== Test 4: 昇格は CLI / MCP / curl のどれからも通らない ==="
set +e
CLI_OUT="$("$TAKO_BIN" remote devices role nPHONEA manage 2>&1)"
CLI_CODE=$?
set -e
check_eq "CLI からの昇格は失敗する（exit 1）" "1" "$CLI_CODE"
check_contains "理由に画面での操作が案内される" "$CLI_OUT" "画面"
CODE="$(api GET /api/me)"
check_eq "CLI の昇格で role は動いていない" "observe" "$(jqf role)"

# MCP 経路（= tako-app の中の dispatch。daemon からは GUI と区別が付かない面）
read -r MCP_SOCKET MCP_TOKEN <<<"$(python3 - "$TAKO_DISCOVERY_DIR" <<'PYD'
import json, os, sys
d = sys.argv[1]
for name in sorted(os.listdir(d)):
    try:
        info = json.load(open(os.path.join(d, name)))
    except Exception:
        continue
    if info.get("socket") and info.get("token"):
        print(info["socket"], info["token"]); break
PYD
)"
MCP_OUT="$(printf '%s\n%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_remote_devices","arguments":{"action":"role","device_id":"nPHONEA","role":"manage"}}}' \
  | TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null | tail -1)"
check_contains "MCP からの昇格も断られる" "$MCP_OUT" "画面"
CODE="$(api GET /api/me)"
check_eq "MCP の昇格でも role は動いていない" "observe" "$(jqf role)"

# 管理トークンを読んで直接叩いた（= GUI ではないプロセス）
CODE="$(admin /api/admin/devices/role '{"device_id":"nPHONEA","role":"manage"}')"
check_eq "curl からの昇格は 403" "403" "$CODE"
check_eq "理由コード" "upgrade_requires_gui" "$(jqf kind)"
CODE="$(admin /api/admin/pair/approve '{"device_id":"nPHONEA","role":"manage"}')"
check_eq "curl からの承認（昇格）も 403" "403" "$CODE"
check_eq "承認側の理由コードも同じ" "upgrade_requires_gui" "$(jqf kind)"
CODE="$(api GET /api/me)"
check_eq "どの経路でも role は observe のまま" "observe" "$(jqf role)"

echo
echo "=== Test 5: tako-app として名乗った承認だけが通る ==="
CODE="$(gui_admin /api/admin/pair/approve '{"device_id":"nPHONEA","role":"interact"}')"
check_eq "GUI 相当の承認は 200" "200" "$CODE"
CODE="$(api GET /api/me)"
check_eq "role が上がった" "interact" "$(jqf role)"
check_eq "承認待ちは消えた" "false" "$(jqf pending)"
CODE="$(api GET /api/files)"
check_eq "ファイル参照が通るようになった" "200" "$CODE"
TODO3="$("$TAKO_BIN" todo list --kind permission --json 2>/dev/null || echo '{}')"
check_eq "未完了の permission タスクは 0 件（承認で畳まれた）" "0" "$(printf '%s' "$TODO3" | python3 -c 'import json,sys
d=json.load(sys.stdin); print(len(d.get("tasks",[])))')"
TODO_ALL="$("$TAKO_BIN" todo list --kind permission --all --json 2>/dev/null || echo '{}')"
check_eq "タスクは done になっている" "done" "$(printf '%s' "$TODO_ALL" | jqs tasks.0.status)"

echo
echo "=== Test 6: 降格は CLI / MCP から通る（1:1）==="
"$TAKO_BIN" remote devices role nPHONEA observe > "$TMP/demote.json"
check_eq "CLI の降格は成功する" "downgrade" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["change"])' "$TMP/demote.json")"
CODE="$(api GET /api/me)"
check_eq "role が下がった" "observe" "$(jqf role)"
CODE="$(api GET /api/files)"
check_eq "ファイル参照はまた 403" "403" "$CODE"

echo
echo "=== Test 7: 監査に caller_check が残る ==="
AUDIT="$(cat "$TAKO_REMOTE_STATE_DIR/audit.log")"
check_contains "拒否が記録されている" "$AUDIT" '"event":"role_change_denied"'
check_contains "拒否の呼び出し元の分類" "$AUDIT" '"caller_check":"not_gui"'
check_contains "許可も記録されている" "$AUDIT" '"event":"role_change_allowed"'
check_contains "許可の呼び出し元の分類" "$AUDIT" '"caller_check":"gui"'
check_not_contains "**理由の本文は監査へ載せない**（FR-6.8）" "$AUDIT" "出先でログを見たい"

echo
echo "=== Test 8: エッジ ==="
# ① 要求 role が現状以下 → 保留を作らない
CODE="$(api POST /api/pair '{"name":"phone-a","role":"observe"}')"
check_eq "同じ role の要求は already_registered" "already_registered" "$(jqf status)"
api POST /api/pair '{"name":"phone-a","role":"interact"}' >/dev/null
CODE="$(api GET /api/me)"
check_eq "昇格の要求は保留になる" "true" "$(jqf pending)"

# ② 承認前に端末を削除する → 保留も一緒に消える
"$TAKO_BIN" remote devices revoke nPHONEA >/dev/null
LIST="$("$TAKO_BIN" remote devices list)"
check_eq "端末が消えた" "0" "$(printf '%s' "$LIST" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["devices"]))')"
check_eq "保留も消えた" "0" "$(printf '%s' "$LIST" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["pending"]))')"
CODE="$(api GET /api/me)"
check_eq "端末は未登録に戻る" "false" "$(jqf registered)"

# ③ daemon 再起動をまたぐ: 授権（保留）は消えるが、記録（タスク）は残る
api POST /api/pair '{"name":"phone-a","role":"observe"}' >/dev/null
gui_admin /api/admin/pair/approve '{"device_id":"nPHONEA","role":"observe"}' >/dev/null
api POST /api/pair '{"name":"phone-a","role":"manage","reason":"再起動をまたぐ確認"}' >/dev/null
"$TAKO_BIN" remote stop >/dev/null 2>&1
start_daemon
LIST="$("$TAKO_BIN" remote devices list)"
check_eq "再起動で保留は消える（安全側）" "0" "$(printf '%s' "$LIST" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["pending"]))')"
TODO4="$("$TAKO_BIN" todo list --kind permission --json 2>/dev/null || echo '{}')"
check_eq "記録（タスク）は残る" "1" "$(printf '%s' "$TODO4" | python3 -c 'import json,sys
d=json.load(sys.stdin); print(len(d.get("tasks",[])))')"
# 残ったタスクを見て、保留が無くても画面から直接与えられる（= GUI 相当の role 変更）
CODE="$(gui_admin /api/admin/devices/role '{"device_id":"nPHONEA","role":"manage"}')"
check_eq "保留が無くても GUI からは与えられる" "200" "$CODE"
check_eq "方向は昇格" "upgrade" "$(jqf change)"
CODE="$(api GET /api/me)"
check_eq "role が上がった" "manage" "$(jqf role)"

# ④ タスクを done にしても権限は動かない（正本の 2 層分け）
"$TAKO_BIN" remote devices role nPHONEA observe >/dev/null
TID="$("$TAKO_BIN" todo list --kind permission --all --json 2>/dev/null | python3 -c 'import json,sys
d=json.load(sys.stdin); ts=d.get("tasks",[]); print(ts[0]["id"] if ts else "")')"
if [ -n "$TID" ]; then
  "$TAKO_BIN" todo done "$TID" >/dev/null 2>&1 || true
  CODE="$(api GET /api/me)"
  check_eq "タスクを done にしても role は動かない" "observe" "$(jqf role)"
else
  fail "タスク id を引けなかった"
fi

echo
echo "=== Test 9: A/B（TAKO_1452_LEGACY=1 で旧挙動）==="
"$TAKO_BIN" remote stop >/dev/null 2>&1
TAKO_1452_LEGACY=1 start_daemon
api POST /api/pair '{"name":"phone-a","role":"interact","reason":"legacy 腕"}' >/dev/null
CODE="$(api GET /api/me)"
check_eq "legacy では登録済み端末に pending を返さない（旧症状の再現）" "null" "$(jqf pending)"
LIST="$("$TAKO_BIN" remote devices list)"
check_eq "保留そのものは作られる（作法は #283 のまま）" "1" "$(printf '%s' "$LIST" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["pending"]))')"
# **安全側のゲートは legacy でも外れない**
CODE="$(admin /api/admin/devices/role '{"device_id":"nPHONEA","role":"manage"}')"
check_eq "legacy でも curl からの昇格は 403" "403" "$CODE"

echo
echo "================================"
echo "PASS: $PASS / FAIL: $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
