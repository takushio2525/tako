#!/usr/bin/env bash
# test-remote-command-card-1724.sh — スマホからコマンドカードを実行する実経路テスト（#1724）
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+
# 実 tako-app** を走らせ、PWA と同じ HTTP を実際に叩いて
#   ① カードが一覧に出る（observe でも読める・論理文字列そのまま・未実行は null）
#   ② observe の実行は 403 で、PC 側にペインが生えない・記録も付かない
#   ③ interact の実行が PC のカードの「新規ペインで実行」と同じ経路で走る
#      （同じタブに 1 枚増える・フォーカスは動かない・画面にコマンドの出力が出る）
#   ④ **PC 側のカードも実行済みになる**（GUI が持つ保管庫を `tako show-command --list` で
#      読むと、スマホが見ているのと同じ実行ペイン・終了コードが載っている）
#   ⑤ エッジ: 実行中をもう一度押す（409・ペインは増えない）/ 実行ペインを閉じたら再実行できる /
#      PC 側でカードが閉じられた後に押す（404）/ その場で降格された直後に押す（403）/
#      失効した端末（403）/ 壊れた頼み方（400）/ 表に無い受け口（床の Manage）
#   ⑥ persist.log と監査ログに「どの端末から・どのカードを」だけが残り、本文は残らない
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / 端末登録 / tako 設定には一切触らない**
# （HOME / state / data / orchestrator は mktemp 配下、tailscale は偽物）。
# 出力に実ユーザー名・実ホームパスは入らない（#927）。
#
# 使い方: bash scripts/test-remote-command-card-1724.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（tako のペインの中から走らせると継承される）
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
    *) fail "$1（'${3}' を含まない）" ;;
  esac
}
check_not_contains() {
  case "$2" in
    *"$3"*) fail "$1（'${3}' を含んでいる）" ;;
    *) pass "$1" ;;
  esac
}

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1724-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1724-$$"
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
NODE="fake-1724.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE1724","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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
mkdir -p "$HOME" "$TMP/zdot"
# 実行ペインのシェルが実ユーザー名・ホスト名を画面に出さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
export ZDOTDIR="$TMP/zdot"
mkdir -p "$TMP/bin"
ln -sf "$TAKO_BIN" "$TMP/bin/tako"
export PATH="$TMP/bin:/usr/bin:/bin:/usr/sbin:/sbin"

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
export TAKO_AUTORENAME=0
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# #841: ループバック TCP では接続元プロセスを検証する。serve の代わりに繋ぐのは curl
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
# #1452: 承認は「呼び出し元が tako-app か」も見る。この隔離環境では curl を GUI 側として名乗る
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"
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
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。本番 GUI を触る恐れがあるので中止する"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・タブ 1 枚）"

HOST_HDR="fake-1724.tailfake.ts.net"
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
# JSON のキーを辿る（`a.0.b`）。$1 = ファイル、$2 = 経路
jqf_in() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur,list):
        try: cur=cur[int(k)]
        except (ValueError,IndexError): cur=None; break
    elif isinstance(cur,dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$1" "$2"; }
jqf() { jqf_in "$TMP/body" "$1"; }
# PC 側の保管庫（GUI が持つカード）を CLI で読む = PC のカードが描いているのと同じもの
pc_cards() { "$TAKO_BIN" show-command --list --pane "$PANE" > "$TMP/pc.json" 2>&1; }
pc_field() { pc_cards; jqf_in "$TMP/pc.json" "$1"; }
# アクティブタブのペイン数とフォーカス
tab_panes() { "$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
tabs=d["tabs"]
tab=next((t for t in tabs if t.get("active")), tabs[0])
def walk(o, out):
    if isinstance(o, dict):
        if "surface" in o and "id" in o: out.append(o)
        for v in o.values(): walk(v, out)
    elif isinstance(o, list):
        for v in o: walk(v, out)
out=[]; walk(tab, out)
print(len(out))'; }
focused_pane() { "$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
def walk(o):
    if isinstance(o, dict):
        if o.get("focused") is True and "surface" in o: return o["id"]
        for v in o.values():
            r=walk(v)
            if r is not None: return r
    elif isinstance(o, list):
        for v in o:
            r=walk(v)
            if r is not None: return r
    return None
print(walk(d))'; }
# 実行記録が期待の状態になるまで待つ（**状態で待つ**。上限は 0.2 秒 × 回数）
wait_pc_state() {
  local want="$1" tries="${2:-150}" got=""
  for _ in $(seq 1 "$tries"); do
    got="$(pc_field "cards.0.runs.0.state")"
    [ "$got" = "$want" ] && { echo "$got"; return 0; }
    sleep 0.2
  done
  echo "$got"
  return 1
}

start_daemon() {
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
  BASE="http://127.0.0.1:$PORT"
  for _ in $(seq 1 100); do
    if [ "$(api GET /api/health)" = "200" ]; then break; fi
    sleep 0.1
  done
}

# 端末の role をその値に据える（上げる = pair + 承認 / 下げる = 管理経路。#1452 と同じゲート）
pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1724\",\"role\":\"$1\"}"
  curl -s -o /dev/null -X POST "$BASE/api/admin/devices/role" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1724\",\"role\":\"$1\"}"
  api GET /api/me >/dev/null
  ACTUAL_ROLE="$(jqf role)"
  if [ "$ACTUAL_ROLE" != "$1" ]; then
    fail "role を ${1} に据えられなかった（実際 ${ACTUAL_ROLE}）"
  fi
}

start_daemon
pass "隔離 daemon が起動した（loopback port ${PORT}）"

PANE="$(focused_pane)"
case "$PANE" in
  ''|None|*[!0-9]*) echo "対象ペインが読めない（'${PANE}'）"; exit 1 ;;
esac
MARKER="tako-1724-ok-$$"

echo
echo "=== 準備: AI がカードを出す（CLI = master の tako_show_command と同じ dispatch） ==="
"$TAKO_BIN" show-command --pane "$PANE" --label "動作確認" "printf '%s\\n' $MARKER" > "$TMP/show.json" 2>&1
CARD="$(jqf_in "$TMP/show.json" card.id)"
case "$CARD" in
  ''|null|*[!0-9]*) echo "カードを出せなかった:"; cat "$TMP/show.json"; exit 1 ;;
esac
pass "ペイン ${PANE} にカード ${CARD} を出した"

echo
echo "=== Test 1: カードが一覧に出る（observe でも読める） ==="
pair_as observe
CODE="$(api GET "/api/cards?pane=$PANE")"
check_eq "observe は一覧を引ける" "200" "$CODE"
check_eq "カードが載る" "$CARD" "$(jqf cards.0.id)"
check_eq "論理文字列そのまま" "printf '%s\\n' $MARKER" "$(jqf cards.0.commands.0)"
check_eq "ラベルが載る" "動作確認" "$(jqf cards.0.label)"
check_eq "未実行の記録は null" "null" "$(jqf cards.0.runs.0)"

echo
echo "=== Test 2: observe の実行は 403・PC に何も起きない ==="
BEFORE="$(tab_panes)"
CODE="$(api POST "/api/cards/$CARD/run" '{"index":1}')"
check_eq "observe の実行は 403" "403" "$CODE"
check_contains "足りない role を名指す" "$(cat "$TMP/body")" "interact"
check_eq "PC 側にペインが生えない" "$BEFORE" "$(tab_panes)"
check_eq "PC 側のカードに記録が付かない" "null" "$(pc_field cards.0.runs.0)"

echo
echo "=== Test 3: interact の実行が PC の「新規ペインで実行」と同じ経路で走る ==="
pair_as interact
FOCUS_BEFORE="$(focused_pane)"
CODE="$(api POST "/api/cards/$CARD/run" '{"index":1}')"
check_eq "interact の実行は通る" "200" "$CODE"
RUN_PANE="$(jqf pane)"
check_eq "応答に走り始めた記録が載る" "running" "$(jqf run.state)"
check_eq "応答の記録は実行ペインを指す" "$RUN_PANE" "$(jqf run.pane)"
check_not_contains "応答に本文を返さない" "$(cat "$TMP/body")" "$MARKER"
check_eq "同じタブに 1 枚増える" "$((BEFORE + 1))" "$(tab_panes)"
check_eq "フォーカスは動かない（手元のペインに触らない）" "$FOCUS_BEFORE" "$(focused_pane)"
SAW=""
for _ in $(seq 1 150); do
  if "$TAKO_BIN" read --pane "$RUN_PANE" 2>/dev/null | grep -q "^$MARKER"; then SAW=1; break; fi
  sleep 0.2
done
check_eq "実行ペインの画面にコマンドの出力が出る" "1" "$SAW"

echo
echo "=== Test 4: PC 側のカードも実行済みになる ==="
STATE="$(wait_pc_state exited)"
check_eq "PC 側のカードの記録が exited に確定する" "exited" "$STATE"
check_eq "PC 側の記録は同じ実行ペイン" "$RUN_PANE" "$(pc_field cards.0.runs.0.pane)"
check_eq "PC 側の終了コード" "0" "$(pc_field cards.0.runs.0.exit_code)"
api GET "/api/cards?pane=$PANE" >/dev/null
check_eq "スマホの一覧も同じ状態" "exited" "$(jqf cards.0.runs.0.state)"
check_eq "スマホの一覧も同じ終了コード" "0" "$(jqf cards.0.runs.0.exit_code)"
check_eq "回数は 1" "1" "$(jqf cards.0.runs.0.count)"

echo
echo "=== Test 5: 実行中のカードをもう一度押す ==="
"$TAKO_BIN" show-command --pane "$PANE" --label "長い処理" "sleep 600" > "$TMP/show2.json" 2>&1
LONG="$(jqf_in "$TMP/show2.json" card.id)"
CODE="$(api POST "/api/cards/$LONG/run" '{}')"
check_eq "1 回目（index 省略 = 1 件目）は通る" "200" "$CODE"
LONG_PANE="$(jqf pane)"
PANES_NOW="$(tab_panes)"
CODE="$(api POST "/api/cards/$LONG/run" '{"index":1}')"
check_eq "走っているあいだの 2 回目は 409" "409" "$CODE"
check_eq "理由の種別は still_running" "still_running" "$(jqf kind)"
check_contains "どのペインで走っているかを言う" "$(jqf error)" "ペイン $LONG_PANE"
check_eq "2 回目でペインが増えない" "$PANES_NOW" "$(tab_panes)"
# PC のボタン（CLI の --run も同じ dispatch）でも同じく断られる
if "$TAKO_BIN" show-command --run --card "$LONG" > "$TMP/pcrun.out" 2>&1; then
  fail "PC 側からの 2 回目も断られる（通ってしまった）"
else
  check_contains "PC 側からの 2 回目も同じ理由で断られる" "$(cat "$TMP/pcrun.out")" "まだ実行中"
fi
"$TAKO_BIN" close --pane "$LONG_PANE" --force > /dev/null 2>&1
api GET "/api/cards?pane=$PANE" >/dev/null
LONG_IDX="$(python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
print(next(i for i,c in enumerate(d["cards"]) if c["id"]==int(sys.argv[2])))' "$TMP/body" "$LONG")"
check_eq "実行ペインを閉じると closed に確定する" "closed" "$(jqf "cards.$LONG_IDX.runs.0.state")"
CODE="$(api POST "/api/cards/$LONG/run" '{"index":1}')"
check_eq "閉じた後は再実行できる" "200" "$CODE"
check_eq "回数が 2 へ進む" "2" "$(jqf run.count)"
"$TAKO_BIN" close --pane "$(jqf pane)" --force > /dev/null 2>&1

echo
echo "=== Test 6: PC 側でカードが閉じられた後に押す ==="
"$TAKO_BIN" show-command --dismiss --card "$LONG" > /dev/null 2>&1
PANES_NOW="$(tab_panes)"
CODE="$(api POST "/api/cards/$LONG/run" '{"index":1}')"
check_eq "閉じられたカードの実行は 404" "404" "$CODE"
check_eq "理由の種別は not_found" "not_found" "$(jqf kind)"
check_eq "ペインは増えない" "$PANES_NOW" "$(tab_panes)"

echo
echo "=== Test 7: 端末の権限がその場で降格された直後に押す ==="
"$TAKO_BIN" show-command --pane "$PANE" "printf 'x\\n'" > "$TMP/show3.json" 2>&1
DEMOTE="$(jqf_in "$TMP/show3.json" card.id)"
pair_as observe
PANES_NOW="$(tab_panes)"
CODE="$(api POST "/api/cards/$DEMOTE/run" '{"index":1}')"
check_eq "降格した直後の実行は 403" "403" "$CODE"
check_eq "ペインは増えない" "$PANES_NOW" "$(tab_panes)"
check_eq "PC 側の記録も付かない" "null" "$(pc_field "cards.1.runs.0")"

echo
echo "=== Test 8: 頼み方の不備と表に無い受け口 ==="
pair_as interact
CODE="$(api POST "/api/cards/$DEMOTE/run" '{"index":9}')"
check_eq "範囲外の番号は 400" "400" "$CODE"
CODE="$(api POST "/api/cards/$DEMOTE/run" '{"index":"one"}')"
check_eq "番号でない index は 400" "400" "$CODE"
CODE="$(api GET "/api/cards")"
check_eq "pane の無い一覧は 400" "400" "$CODE"
CODE="$(api GET "/api/cards?pane=999999")"
check_eq "無いペインの一覧は 404" "404" "$CODE"
CODE="$(api POST "/api/cards/$DEMOTE/show" '{"commands":["echo injected"]}')"
check_eq "interact はカードを作る受け口で 403（床の Manage）" "403" "$CODE"
CODE="$(api POST "/api/cards/$DEMOTE/copy" '{}')"
check_eq "interact は PC のクリップボードを書く受け口で 403" "403" "$CODE"
pair_as manage
CODE="$(api POST "/api/cards/$DEMOTE/show" '{"commands":["echo injected"]}')"
check_eq "manage でも表に無い受け口は 404（受け口そのものが無い）" "404" "$CODE"
check_not_contains "本文を渡してもカードは増えない" "$(pc_cards; cat "$TMP/pc.json")" "echo injected"
CODE="$(api GET "/api/cards/$DEMOTE/run")"
check_eq "メソッド違いは 405" "405" "$CODE"

echo
echo "=== Test 9: 失効した端末 ==="
curl -s -o /dev/null -X POST "$BASE/api/admin/devices/revoke" \
  -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"device_id":"nFAKE1724"}'
CODE="$(api POST "/api/cards/$DEMOTE/run" '{"index":1}')"
check_eq "失効した端末の実行は 403" "403" "$CODE"

echo
echo "=== Test 10: 診断ログと監査に本文を出さない ==="
PERSIST="$TAKO_DATA_DIR/persist.log"
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
check_contains "persist.log に端末・カード・番号・結果が残る" "$(cat "$PERSIST" 2>/dev/null)" \
  "リモートからコマンドカードを実行: 端末=nFAKE1724 カード=$CARD 番号=1 結果=ok"
check_contains "断った実行も残る（409）" "$(cat "$PERSIST" 2>/dev/null)" \
  "カード=$LONG 番号=1 結果=still_running"
check_contains "監査に card_run が残る" "$(cat "$AUDIT" 2>/dev/null)" "card_run"
for f in "$PERSIST" "$AUDIT"; do
  if grep -q "$MARKER" "$f" 2>/dev/null || grep -q "sleep 600" "$f" 2>/dev/null; then
    fail "$(basename "$f") にコマンド本文が漏れている"
  else
    pass "$(basename "$f") にコマンド本文が漏れていない"
  fi
done

echo
echo "================================"
echo "  PASS: $PASS  /  FAIL: $FAIL"
echo "================================"
[ "$FAIL" -eq 0 ]
