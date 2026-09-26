#!/usr/bin/env bash
# test-remote-launch-1449.sh — スマホの「+」から立てる 3 種の実経路テスト（#1449）
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+
# 実 tako-app** を走らせ、HTTP を実際に叩いて
#   ① role の gate（observe / interact では 403 / manage で通る）
#   ② ターミナル: `POST /api/tabs` だけでタブ + シェルのペインが立ち、CLI の一覧に出る
#   ③ SSH: `GET /api/ssh-hosts` が **CLI（`tako ssh-hosts`）と同じ答え**を返し、
#      `POST /api/ssh {target:"tab"}` が `tako open-in remote --target tab` と同じ形を作る
#   ④ 接続に失敗してもペインが消えず、理由が `ssh_connect` に残る（#919 / #1040）
#   ⑤ 壊れた指定でペインを増減させない / 監査ログに残る
# を実測する。master の起動そのものは #1078 の
# `scripts/test-remote-master-launch.sh` が見るので、ここでは扱わない。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定 / ~/.ssh には一切触らない**
# （tailscale の呼び出しは偽物へ、HOME / state / data / orchestrator は mktemp 配下）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141。`TAKO_ISOLATED=1` が既定で狙う）。
#
# 使い方: bash scripts/test-remote-launch-1449.sh
set -euo pipefail

# **本番 GUI を指す env を最初に落とす**。tako のペインの中から走らせると
# `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` が継承され、CLI（`tako list` /
# `tako tab new`）が隔離インスタンスではなく**ユーザーの本番 GUI** を触る
# （実測: 検証中に本番へタブが 1 枚できた）。下の起動ガードが第 2 の網
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

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1449-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1449-$$"
cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
  stop_isolated_gui "$APP_PID"
  # ソケット名を明示した起動は自分で畳む（#1192）
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
NODE="fake-1449.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE1449","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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
# **HOME ごと隔離する**: `Request::SshHosts` は `$HOME/.ssh/config` を読むので、
# 本物のホーム（= 実ホスト名。#927）を読ませない。ssh 自身も同じ config を見る
export HOME="$TMP/home"
mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
# 実在しない相手だけを並べる（TEST-NET-3 = 198.51.100.0/24 はルーティングされない）
cat > "$HOME/.ssh/config" <<'SSHCONF'
Host tako1449-unreachable
  HostName 198.51.100.9
  User tester
  Port 2222
  ConnectTimeout 2
  BatchMode yes
  StrictHostKeyChecking no

Host tako1449-alias
  User tester
SSHCONF
chmod 600 "$HOME/.ssh/config"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_REMOTE_STATE_DIR="$TMP/remote"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_PERSIST=0
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# #841: ループバック TCP では XFF を読む前に**接続元プロセス**を検証する。
# この検証をそのまま通したいので、切る（`TAKO_841_LEGACY`）のではなく
# 「この隔離環境で serve の代わりに繋いでくるのは curl」だと**名前で宣言**する。
# 所有者ゲート（uid）と実行ファイルの引き当ては本番と同じ経路を通る
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
# #1452: ペアリングの**承認**は「呼び出し元が tako-app か」も見るようになった
# （管理トークンは 0600 = 同一ユーザーなら誰でも読めるので、トークンだけでは
# GUI と CLI / AI を区別できない）。このテストは curl で承認を撃つので、
# **この隔離環境では curl を GUI 側として名乗る**（#841 の PEER_NAMES と同じ作法）
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
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
# **繋がった先が隔離インスタンスであることを確かめる**。立ち上げ直後の隔離 app は
# タブ 1 枚・ペイン 1 枚しか持たない。本番 GUI に繋がっていたらここで必ず外れる
# （外れたまま進むと本番にタブを作ってしまう = 一度やった事故）
BOOT_TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$BOOT_TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。"
  echo "本番 GUI を触る恐れがあるので中止する（TAKO_SOCKET などが残っていないか確認）。"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・タブ 1 枚）"

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
HOST_HDR="fake-1449.tailfake.ts.net"

api() {
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
tab_count() { "$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))'; }
pane_count() { "$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin); print(sum(len(t["panes"]) for t in d["tabs"]))'; }
tab_title_of() { "$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    if t["id"]=='"$1"': print(t.get("title") or ""); raise SystemExit
print("")'; }
has_pane() { "$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
print("yes" if any(p["id"]=='"$1"' for t in d["tabs"] for p in t["panes"]) else "no")'; }

pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1449\",\"role\":\"$1\"}"
}

# daemon → app の IPC が繋がるまで待つ（起動直後は 503 を返す）
for _ in $(seq 1 100); do
  if [ "$(api GET /api/health)" = "200" ]; then break; fi
  sleep 0.1
done

echo
echo "=== Test 1: role の gate（3 種とも Manage 以上） ==="
# #1449 では role を **Manage 据え置き**にした（Issue の「interact 以上」案は不採用）。
# 新しいタブとプロセスを作れる = 実質シェルアクセスで、#1078 / #1080 が
# close / resize と同じ強さとして置いた判断を、UI の都合で緩めない
for role in observe interact; do
  pair_as "$role"
  CODE="$(api GET /api/me)"
  check_eq "${role} 端末として登録された" "$role" "$(jqf role)"
  CODE="$(api POST /api/tabs '{}')"
  check_eq "${role} のターミナル起動は 403" "403" "$CODE"
  check_contains "理由に必要な role が出る（${role}）" "$(body)" "manage"
  CODE="$(api GET /api/ssh-hosts)"
  check_eq "${role} の SSH ホスト一覧は 403" "403" "$CODE"
  CODE="$(api POST /api/ssh '{"host":"tako1449-alias","target":"tab"}')"
  check_eq "${role} の SSH 接続は 403" "403" "$CODE"
done
TABS_BEFORE="$(tab_count)"

echo
echo "=== Test 2: ターミナル（POST /api/tabs だけでシェルのタブが立つ） ==="
pair_as manage
CODE="$(api POST /api/tabs '{}')"
check_eq "manage のターミナル起動は 200" "200" "$CODE"
TERM_TAB="$(jqf tab)"
TERM_PANE="$(jqf pane)"
check_eq "tako 側のタブが 1 つ増えた（= Mac 画面にも出る）" "$((TABS_BEFORE + 1))" "$(tab_count)"
check_eq "立ったペインが CLI から見える" "yes" "$(has_pane "$TERM_PANE")"
# シェルが本当に動いている（プロンプトが出るまで待つ）
SHELL_OK="no"
for _ in $(seq 1 40); do
  if [ -n "$("$TAKO_BIN" read --pane "$TERM_PANE" 2>/dev/null | tr -d '[:space:]')" ]; then
    SHELL_OK="yes"; break
  fi
  sleep 0.25
done
check_eq "ペインでシェルが動いている（画面に出力が出た）" "yes" "$SHELL_OK"
# CLI の 1:1（設計原則 5）: `tako tab new` が同じ形を作る
CLI_TAB="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["tab"])')"
check_eq "CLI（tako tab new）でも同じ形のタブが作れる" "yes" "$( [ -n "$CLI_TAB" ] && echo yes || echo no)"

echo
echo "=== Test 3: SSH ホスト一覧が CLI と同じ 1 実装から来る ==="
CODE="$(api GET /api/ssh-hosts)"
check_eq "manage の SSH ホスト一覧は 200" "200" "$CODE"
HTTP_HOSTS="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["hosts"], sort_keys=True, ensure_ascii=False))' "$TMP/body")"
CLI_HOSTS="$("$TAKO_BIN" ssh-hosts | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["hosts"], sort_keys=True, ensure_ascii=False))')"
# **中身は出さない**: 隔離が壊れていた場合に実 ~/.ssh/config が出力へ載るため（#927）
if [ "$CLI_HOSTS" = "$HTTP_HOSTS" ]; then
  pass "HTTP の一覧が CLI（tako ssh-hosts）と一致する"
else
  fail "HTTP の一覧が CLI（tako ssh-hosts）と一致しない（中身は伏せる。隔離を確認）"
fi
check_contains "隔離した ~/.ssh/config の Host が並ぶ" "$HTTP_HOSTS" "tako1449-unreachable"
check_contains "HostName / user / port も PC 側と同じ形で出る" "$HTTP_HOSTS" "198.51.100.9"

echo
echo "=== Test 4: SSH ターミナル（target=tab で新しいタブが立ち、失敗しても残る） ==="
TABS_BEFORE_SSH="$(tab_count)"
CODE="$(api POST /api/ssh '{"host":"tako1449-unreachable","target":"tab"}')"
check_eq "SSH 接続要求は 200（接続の成否はこの時点では未定）" "200" "$CODE"
SSH_TAB="$(jqf tab)"
SSH_PANE="$(jqf pane)"
check_eq "開き先の語彙が #1006 のまま" "tab" "$(jqf target)"
check_eq "タブが 1 つ増えた" "$((TABS_BEFORE_SSH + 1))" "$(tab_count)"
check_eq "タブ名が CLI 経路と同じ形（ssh:<host>）" "ssh:tako1449-unreachable" "$(tab_title_of "$SSH_TAB")"
# 接続は必ず失敗する相手（TEST-NET-3）。**ペインは消えない**のが #919 / #1040 の契約
sleep 8
check_eq "接続に失敗してもペインが残る" "yes" "$(has_pane "$SSH_PANE")"
CODE="$(api GET /api/v2/panes)"
SSH_STATE="$(SSH_PANE="$SSH_PANE" python3 -c '
import json, os, sys
d = json.load(open(sys.argv[1]))
want = int(os.environ["SSH_PANE"])
for p in d["panes"]:
    if p["id"] == want:
        c = p.get("ssh_connect") or {}
        print(str(c.get("host")) + "/" + str(c.get("phase")))
        raise SystemExit
print("none")' "$TMP/body")"
check_contains "ssh_connect にホストが載る（PC 側と同じ値）" "$SSH_STATE" "tako1449-unreachable"
SCREEN="$("$TAKO_BIN" read --pane "$SSH_PANE" 2>/dev/null || true)"
check_contains "ペインに理由が残る（黙って消えない）" "$SCREEN" "tako1449-unreachable"
# CLI の 1:1（設計原則 5）: `tako open-in remote <host> --target tab` が同じ形を作る
CLI_SSH="$("$TAKO_BIN" open-in remote tako1449-alias --target tab --no-focus 2>/dev/null \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("target"))' || echo "err")"
check_eq "CLI（open-in remote --target tab）も同じ語彙で開く" "tab" "$CLI_SSH"

echo
echo "=== Test 5: 壊れた指定でペインを増減させない ==="
PANES_BEFORE="$(pane_count)"
CODE="$(api POST /api/ssh '{"host":"bad host name","target":"tab"}')"
check_eq "使えない文字のホストは 400" "400" "$CODE"
CODE="$(api POST /api/ssh '{"host":"tako1449-alias","target":"nowhere"}')"
check_eq "知らない開き先は 400" "400" "$CODE"
CODE="$(api POST /api/ssh '{"target":"tab"}')"
check_eq "ホスト無しは 400" "400" "$CODE"
CODE="$(api POST /api/tabs '{"cwd":"/nope/nope/1449"}')"
check_eq "存在しないフォルダのタブ作成は 400" "400" "$CODE"
check_eq "失敗でペインが増減しない" "$PANES_BEFORE" "$(pane_count)"

echo
echo "=== Test 6: 監査ログ ==="
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
if [ -s "$AUDIT" ]; then
  check_contains "ターミナル起動が記録される" "$(cat "$AUDIT")" "tab_new"
  check_contains "SSH 接続が記録される" "$(cat "$AUDIT")" "ssh_open"
  check_contains "接続先のホストは記録する（接続先そのもの）" "$(cat "$AUDIT")" "tako1449-unreachable"
  if grep -q "$TMP/home" "$AUDIT"; then fail "監査ログにホームのパスが出ている"; else pass "監査ログにパスを書かない"; fi
else
  fail "監査ログが無い（${AUDIT}）"
fi

echo
echo "=== 結果: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
