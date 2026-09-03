#!/usr/bin/env bash
# test-remote-master-launch.sh — スマホからの「新しいタブ + master 起動」の実経路テスト（#1078）
#
# 偽の tailscale CLI（whois / serve / status）と隔離した state・data ディレクトリで
# **実 daemon（tako remote start）+ 実 tako-app** を走らせ、HTTP を実際に叩いて
#   ① role の gate（observe では 403 / manage で通る）
#   ② タブが本当に作られ、tako 側の一覧（= Mac 画面）に出る
#   ③ master 起動でペインの role / タブ名 / 起動コマンドが CLI 経路と同じ形になる
#   ④ 壊れた指定（存在しないタブ / 未登録プロファイル）でペインを壊さない
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定には一切触らない**
# （tailscale の呼び出しは偽物へ、state / data / orchestrator は mktemp 配下、
# claude は同梱のスタブへ向く = 実エージェントは起動しない）。
#
# 使い方: bash scripts/test-remote-master-launch.sh
set -euo pipefail

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

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1078-XXXXXX")"
APP_PID=""
cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる。progress.md の事故）
  if [ -n "$APP_PID" ]; then kill "$APP_PID" 2>/dev/null || true; fi
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
NODE="fake-1078.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    # 実測形（tailscale.rs の `parse_whois` が読むキー）。ノードは 1 台だけ返す
    printf '{"Node":{"StableID":"nFAKE1078","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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

# --- claude のスタブ（実エージェントを起動しない）-----------------------------
# preflight（daemon 側）は**ログインシェルの PATH** で claude を探すので実物を見つける。
# 実際に走るのはペインのシェルなので、プロファイルの env で PATH を差し替えれば
# 起動されるのはこのスタブになる（実 claude セッションを作らない）
STUB_BIN="$TMP/bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/claude" <<'STUB'
#!/usr/bin/env bash
echo "STUB_CLAUDE_STARTED args=$*"
# 起動したまま居座る（master ペインが即終了しない = 実物と同じ形）
sleep 120
STUB
chmod +x "$STUB_BIN/claude"

# --- 対象バイナリ -------------------------------------------------------------
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}"
if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
  echo "バイナリをビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet)
fi
for b in "$TAKO_BIN" "$APP_BIN"; do
  [ -x "$b" ] || { echo "バイナリが見つからない: $b"; exit 1; }
done

# --- 隔離した環境（daemon と app が同じ discovery / data を見る）--------------
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_REMOTE_STATE_DIR="$TMP/remote"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1078-$$"
export TAKO_PERSIST=0
export TAKO_TAILSCALE_BIN="$FAKE_TS"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
for d in "$TAKO_DATA_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 検証用プロファイル: PATH をスタブへ差し替える（実 claude を起動しない）
cat > "$TAKO_ORCHESTRATOR_DIR/profiles/t1078.yaml" <<YAML
model: null
effort: high
env:
  PATH: "$STUB_BIN:/usr/bin:/bin"
YAML

# --- 起動 ---------------------------------------------------------------------
echo "=== 準備: 隔離 tako-app と daemon を起動 ==="
"$APP_BIN" > "$TMP/app.log" 2>&1 &
APP_PID=$!
for _ in $(seq 1 200); do
  if "$TAKO_BIN" list >/dev/null 2>&1; then break; fi
  sleep 0.1
done
"$TAKO_BIN" list >/dev/null 2>&1 || { echo "tako-app へ接続できない:"; tail -20 "$TMP/app.log"; exit 1; }
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"

if ! "$TAKO_BIN" remote start > "$TMP/daemon.log" 2>&1; then
  echo "daemon を起動できなかった:"; cat "$TMP/daemon.log"; exit 1
fi
for _ in $(seq 1 200); do
  [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.port" ] && break
  sleep 0.1
done
PORT="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.port" 2>/dev/null || true)"
[ -n "$PORT" ] || { echo "daemon の port が読めない:"; cat "$TMP/daemon.log"; exit 1; }
ADMIN_TOKEN="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.token")"
pass "隔離 daemon が起動した（loopback port ${PORT}）"

BASE="http://127.0.0.1:$PORT"
HOST_HDR="fake-1078.tailfake.ts.net"

# serve 経由を模した identity ヘッダつきリクエスト（層① = XFF + XFH）
api() {
  # api <METHOD> <PATH> [BODY]
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR" \
      -H 'Content-Type: application/json' -d "$body"
  else
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR"
  fi
}
body() { cat "$TMP/body"; }
jqf() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur,list): cur=cur[int(k)]
    elif isinstance(cur,dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$TMP/body" "$1"; }

# ペアリング（PWA と同じ経路）→ Mac 画面の承認（管理 API。XFF を付けない）
pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1078\",\"role\":\"$1\"}"
}

echo
echo "=== Test 1: role の gate（observe では作らせない）==="
pair_as observe
CODE="$(api GET /api/me)"
check_eq "observe 端末として登録された" "observe" "$(jqf role)"
CODE="$(api GET /api/master/profiles)"
check_eq "プロファイル一覧は Observe で引ける" "200" "$CODE"
check_contains "検証用プロファイルが並ぶ" "$(body)" "t1078"
CODE="$(api POST /api/tabs '{}')"
check_eq "observe のタブ作成は 403" "403" "$CODE"
check_contains "理由に必要な role が出る" "$(body)" "manage"
CODE="$(api POST /api/tabs/1/master '{"profile":"t1078"}')"
check_eq "observe の master 起動は 403" "403" "$CODE"
TABS_BEFORE="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"

echo
echo "=== Test 2: manage でタブが実際に作られ、tako 側の一覧に出る ==="
pair_as manage
CODE="$(api POST /api/tabs "{\"cwd\":\"$TMP\"}")"
check_eq "manage のタブ作成は 200" "200" "$CODE"
NEW_TAB="$(jqf tab)"
NEW_PANE="$(jqf pane)"
check_contains "指定した cwd で開く" "$(jqf cwd)" "$(basename "$TMP")"
TABS_AFTER="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
check_eq "tako 側のタブが 1 つ増えた（= Mac 画面にも出る）" "$((TABS_BEFORE + 1))" "$TABS_AFTER"
FOUND="$("$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin); t=[x for x in d["tabs"] if x["id"]=='"$NEW_TAB"']
print("yes" if t else "no")')"
check_eq "作られたタブが CLI から見える" "yes" "$FOUND"

echo
echo "=== Test 3: master 起動が CLI 経路と同じ形になる ==="
CODE="$(api POST "/api/tabs/$NEW_TAB/master" '{"profile":"t1078"}')"
check_eq "master 起動は 200" "200" "$CODE"
check_eq "プロファイル名が返る" "t1078" "$(jqf profile)"
check_eq "表示用 role が CLI と同じ形" "orchestrator-master:t1078" "$(jqf role)"
check_eq "タブ名が CLI と同じ形" "master-t1078" "$(jqf tab_title)"
check_eq "系統が claude" "claude" "$(jqf agent)"
# opt-in していないプロファイルなので理由が返る（受け入れ条件 ②）
check_eq "Remote Control は off" "off" "$(jqf remote_control.state)"
check_eq "公式リンクは出ない（url を返さない）" "null" "$(jqf remote_control.url)"
check_contains "理由が出る" "$(jqf remote_control.reason)" "Remote Control"
check_contains "有効化コマンドが具体形" "$(jqf remote_control.enable_command)" "profiles set t1078 --remote-control true"
# 起動コマンド本文は応答へ入れない（env の並びが載るため）
check_eq "応答にコマンド本文を入れない" "null" "$(jqf command)"

# 実際にペインへ届いたか（role が貼られ、スタブが動き出す）
sleep 6
ROLE="$("$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"]=='"$NEW_PANE"': print(p.get("role") or ""); raise SystemExit
print("")')"
check_eq "ペインに role が貼られた" "orchestrator-master:t1078" "$ROLE"
TITLE="$("$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    if t["id"]=='"$NEW_TAB"': print(t.get("title") or ""); raise SystemExit
print("")')"
check_eq "タブ名が反映された" "master-t1078" "$TITLE"
SCREEN="$("$TAKO_BIN" read --pane "$NEW_PANE" 2>/dev/null || true)"
check_contains "起動コマンドがペインで実行された" "$SCREEN" "STUB_CLAUDE_STARTED"

echo
echo "=== Test 4: 壊れた指定でペインを壊さない ==="
PANES_BEFORE="$("$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin); print(sum(len(t["panes"]) for t in d["tabs"]))')"
CODE="$(api POST /api/tabs/abc/master '{"profile":"t1078"}')"
check_eq "タブ id が数値でなければ 400" "400" "$CODE"
CODE="$(api POST /api/tabs/99999/master '{"profile":"t1078"}')"
check_eq "存在しないタブは 404" "404" "$CODE"
CODE="$(api POST "/api/tabs/$NEW_TAB/master" '{"profile":"この名前は無い"}')"
check_eq "未登録プロファイルは 400（ペインに触る前に落ちる）" "400" "$CODE"
CODE="$(api POST /api/tabs '{"cwd":"/nope/nope/1078"}')"
check_eq "存在しないフォルダのタブ作成は 400" "400" "$CODE"
PANES_AFTER="$("$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin); print(sum(len(t["panes"]) for t in d["tabs"]))')"
check_eq "失敗でペインが増減しない" "$PANES_BEFORE" "$PANES_AFTER"

echo
echo "=== Test 5: 監査ログにパスとコマンドを書かない ==="
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
if [ -s "$AUDIT" ]; then
  check_contains "タブ作成が記録される" "$(cat "$AUDIT")" "tab_new"
  check_contains "master 起動が記録される" "$(cat "$AUDIT")" "master_launch"
  if grep -q "$TMP" "$AUDIT"; then fail "監査ログに cwd のパスが出ている"; else pass "監査ログにパスを書かない"; fi
  if grep -q "STUB_CLAUDE\|export PATH" "$AUDIT"; then fail "監査ログに起動コマンドが出ている"; else pass "監査ログにコマンドを書かない"; fi
else
  fail "監査ログが無い（${AUDIT}）"
fi

echo
echo "=== 結果: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
