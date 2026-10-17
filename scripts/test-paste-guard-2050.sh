#!/usr/bin/env bash
# test-paste-guard-2050.sh — 貼り付けの秘密検出の続き（#2050）の実経路テスト
#
# 実 GUI（隔離 tako-app・仮想ディスプレイ tako-vd）+ 実 dispatch + 実 daemon（`tako remote start` + 偽の
# tailscale + curl）+ 実物の Claude Code（一時の設定 dir と偽の Messages API）で測る。番号は依頼の受け入れ条件:
#   policy  2. 許可リスト・規則の無効化・警告だけの段（CLI / MCP）。境界 = 許可の値と鍵の混在・全規則の無効
#   send    4. `tako send` / MCP `tako_send_input` は止めずに届け、応答に伏せ字の警告を付ける
#   remote  4. スマホの入力（POST /api/panes/<id>/input）は確認の段なら 409 と伏せ字で送らない・
#              confirm_secret で送る・警告だけの段は 200 + 警告
#   mod     3. 実物の claude の入力欄に入れた鍵で、送信時に確認が出る（やめる → API へ届かない / 送る → 届く）・
#              警告だけの段はトースト・tako が流した値は二度聞かない
#   ab      4. 同じバイナリの `TAKO_2050_LEGACY=1` では send / remote / mod が素通し（旧挙動）
#   visual  2. GUI の貼り付け（visual-test `paste-guard` の ⑫）: 警告だけの段は確認なしで届き通知が描かれる・
#              許可した値は確認なし・混在は鍵だけで確認・無効にした規則は確認なし。旧挙動は ⑫ で FAILED。
#              画像は新旧で書き出し先を分ける（$TMP/visual-new / visual-legacy）
#   logs    5. GUI の標準出力・標準エラー・persist.log・daemon の状態 dir・偽 API のログに偽の鍵の本文が無い
#
# **本物の設定 dir（~/.claude*）・クリップボード・本番の tako / tailscale / remote には触らない**:
# claude の設定 dir は一時 dir（認証は写さず、偽 API とダミーの API キー）、daemon の状態・data dir・tmux は
# 一時 dir と専用ソケット。前後で本物の ~/.claude*/skills・plugins の一覧とハッシュを比べる。
# 偽の鍵は前置詞と本体を分けて書き、実行時に連結する（GitHub の secret scanning に当てない）。
#
# 使い方: bash scripts/test-paste-guard-2050.sh   （ONLY=policy|send|remote|mod|ab|visual で 1 段だけ）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は Rust の単体・番犬
# （issue2050_paste_guard_routes_watchdog）・mod の `claude plugin test`・PWA の e2e。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR \
  TAKO_2046_LEGACY TAKO_2050_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
check() { if [ "$1" = "1" ]; then pass "$2"; else fail "$2"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 3; }
command -v node >/dev/null 2>&1 || { echo "未実測: node が無い（偽 API に使う）"; exit 3; }

# --- 本物の設定 dir の前後比較（#1959 / #1962 と同じ物差し）-------------------------------
REAL_NOISE='/\.in_use/|/synced/|/\.last_inuse_sweep$|/install-counts-cache\.json$'
snap_real() {
  local d
  for d in "$HOME"/.claude*/skills "$HOME"/.claude*/plugins; do
    [ -d "$d" ] || continue
    find "$d" -type f -print0 2>/dev/null | sort -z | xargs -0 shasum -a 256 2>/dev/null
    find "$d" \( -type d -o -type l \) 2>/dev/null | sort
  done | grep -Ev "$REAL_NOISE" | shasum -a 256 | cut -c1-16
}
REAL_BEFORE="$(snap_real)"

# --- ビルド -------------------------------------------------------------------------------
# visual 段のために visual-test 版の GUI を作る（ほかの段もこの版で走る = #1962 と同じ）
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-app --features visual-test && run_heavy cargo build -q -p tako-cli) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# `/tmp` 直下に短く取る（daemon の待ち受けと tmux が sun_path の上限に当たらない。#1441）
TMP="$(mktemp -d /tmp/tk2050-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
FAKE_PID=""
TMUX_SOCKET="tk2050-$$"
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }
tmx() { tmux -u -L "$TMUX_SOCKET" "$@"; }

cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmx kill-server >/dev/null 2>&1 || true
  tmux -L "$TMUX_SOCKET-gui" kill-server >/dev/null 2>&1 || true
  # ソケット名を明示した起動は自分で畳む（#1192。サーバーを止めてもファイルが残る）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET" "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET-gui"
  if [ -n "$FAKE_PID" ]; then
    kill "$FAKE_PID" 2>/dev/null
    wait "$FAKE_PID" 2>/dev/null
  fi
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# 偽の鍵（前置詞と本体を分けて書く。本体は EXAMPLE を含む明らかな偽物）
FAKE_GITHUB="ghp_""EXAMPLE0123456789abcdefghijklmnopqrst"
FAKE_ANTHROPIC="sk-ant-""api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD"
FAKE_GOOGLE="AIza""EXAMPLE-abcdefghijklmnopqrstuvwxyz0"
export FAKE_GITHUB FAKE_ANTHROPIC FAKE_GOOGLE

# 偽の鍵の 8 文字の部分列がどれかファイルに出ていれば 1（伏せ字は … を挟むので当たらない）
leaks_in() {
  python3 - "$@" <<'PY'
import os, sys
data = ""
for p in sys.argv[1:]:
    if os.path.isfile(p):
        data += open(p, encoding="utf-8", errors="replace").read()
for secret in (os.environ["FAKE_GITHUB"], os.environ["FAKE_ANTHROPIC"], os.environ["FAKE_GOOGLE"]):
    body = secret[4:] if not secret.startswith("sk-ant-") else secret[7:]
    if any(body[i:i + 8] in data for i in range(len(body) - 7)):
        print(1)
        sys.exit(0)
print(0)
PY
}

# --- 隔離した環境 ---------------------------------------------------------------------------
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET-gui"
export TAKO_REMOTE_STATE_DIR="$TMP/remote"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_REMOTE_STATE_DIR"
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done
tk() { "$TAKO_BIN" "$@"; }
jget() { jq -r "$1" 2>/dev/null; }

start_app() {
  local name="$1"
  shift
  ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-0,0,1400,900}"
  launch_isolated_gui "$TMP/app-${name}.log" TAKO_PERSIST=0 ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  # 繋がった先が隔離インスタンスか（タブ 1 枚）。本番に繋がっていたら中止する
  if [ "$(tk list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  # 自動リネームは GUI の env の claude を呼ぶので切る
  tk autorename off >/dev/null 2>&1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
root_pane() { tk list 2>/dev/null | jq -r '.tabs[0].panes[0].id'; }
pane_ids() { tk list 2>/dev/null | jq -r '.tabs[].panes[].id' | sort; }
# 新しいペインを割って ID を出す（確認済みの値はペインごとなので、前の段が届けた鍵を持ち越さない）
fresh_pane() {
  local before after
  before="$(pane_ids)"
  tk split --pane "$(root_pane)" --right >/dev/null 2>&1
  after="$(pane_ids)"
  comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -1
}
pane_has() { local screen; screen="$(tk read --pane "$1" 2>/dev/null)" && grep -q -- "$2" <<<"$screen"; }
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}

# MCP（GUI 内蔵の HTTP MCP。#2046 のテストと同じ口）
mcp_call() {
  python3 - "$1" "$2" <<'PY'
import json, os, sys, urllib.request
info = json.load(open(os.path.join(os.environ["TAKO_DISCOVERY_DIR"], "control.json")))
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": sys.argv[1], "arguments": json.loads(sys.argv[2])}})
req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers={
    "Authorization": "Bearer " + info["token"], "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream"})
with urllib.request.urlopen(req, timeout=60) as r:
    resp = json.loads(r.read())
if "error" in resp:
    print(json.dumps({"rpc_error": resp["error"]["message"]}, ensure_ascii=False))
    sys.exit(0)
text = resp["result"]["content"][0]["text"]
try:
    print(json.dumps(json.loads(text), ensure_ascii=False))
except ValueError:
    print(json.dumps({"text": text}, ensure_ascii=False))
PY
}

# --- policy: 段・許可リスト・規則の無効化（CLI / MCP）-------------------------------------
phase_policy() {
  echo "== 2. 許可リスト・規則の無効化・警告だけの段（実 GUI の dispatch）"
  local st out id settings
  settings="$TAKO_DATA_DIR/settings.json"
  st="$(tk paste-guard)"
  check_eq "既定は確認の段・許可なし・全規則" "confirm 0 0" \
    "$(printf '%s' "$st" | jq -r '"\(.mode) \(.allow | length) \(.disabled_rules | length)"')"
  out="$(tk paste-guard warn)"
  check_eq "tako paste-guard warn で警告だけの段（settings.json に残る）" "warn true warn" \
    "$(printf '%s' "$out" | jq -r '"\(.mode) \(.enabled)"') $(jq -r '.paste_guard_policy.mode // "-"' "$settings")"
  out="$(printf 'これ: %s\n' "$FAKE_GITHUB" | tk paste-guard check)"
  check_eq "警告だけの段の判定は verdict=warn" "warn" "$(printf '%s' "$out" | jget .verdict)"
  out="$(tk paste-guard on)"
  check_eq "tako paste-guard on で確認の段へ戻る（既定は書き出さない）" "confirm -" \
    "$(printf '%s' "$out" | jget .mode) $(jq -r '.paste_guard_policy.mode // "-"' "$settings")"

  # 許可リスト（標準入力）: 指紋と伏せ字だけを持ち、許可した値は当たらない
  out="$(printf '%s' "$FAKE_GOOGLE" | tk paste-guard allow)"
  id="$(printf '%s' "$out" | jget '.allowed[0].id')"
  check_eq "allow: 当たりを許可リストへ（google_api_key・ID 12 文字）" "google_api_key 12" \
    "$(printf '%s' "$out" | jget '.allowed[0].rule') ${#id}"
  check_eq "settings.json に指紋（16 進 64 文字）を書き、生の値は書かない" "64 0" \
    "$(jq -r '.paste_guard_policy.allow[0].fingerprint | length' "$settings") $(leaks_in "$settings")"
  out="$(printf 'GOOGLE=%s\n' "$FAKE_GOOGLE" | tk paste-guard check)"
  check_eq "許可した値は当たらない（allowed=1）" "false 1" "$(printf '%s' "$out" | jq -r '"\(.hit) \(.allowed)"')"
  # 境界 1: 許可の値と鍵が 1 回の貼り付けに混在 → 鍵だけ
  out="$(printf '%s\n%s\n' "$FAKE_GOOGLE" "$FAKE_GITHUB" | tk paste-guard check)"
  check_eq "境界: 許可の値と鍵の混在は鍵だけが当たる" "github_token 2 1" \
    "$(printf '%s' "$out" | jq -r '"\(.findings | map(.rule) | join(",")) \(.findings[0].line) \(.allowed)"')"

  # 規則の無効化（CLI）
  out="$(tk paste-guard rule github_token off)"
  check_eq "rule github_token off" '["github_token"]' "$(printf '%s' "$out" | jq -c .disabled_rules)"
  out="$(printf '%s' "$FAKE_GITHUB" | tk paste-guard check)"
  check_eq "無効にした規則は当たらない（disabled=1・ほかの段も拾い直さない）" "false 1" \
    "$(printf '%s' "$out" | jq -r '"\(.hit) \(.disabled)"')"
  check_eq "知らない規則名は断る（終了コード非 0）" "1" "$(tk paste-guard rule nope off >/dev/null 2>&1 && echo 0 || echo 1)"

  # 境界 2: 規則を全部無効（MCP の rules）→ 鍵だらけでも当たらない
  local all_off all_on
  all_off="$(printf '%s' "$st" | jq -c '[.rule_names[] | {key: ., value: false}] | from_entries')"
  all_on="$(printf '%s' "$st" | jq -c '[.rule_names[] | {key: ., value: true}] | from_entries')"
  out="$(mcp_call tako_settings "{\"action\":\"paste_guard\",\"rules\":$all_off}")"
  check_eq "MCP rules で全規則を無効" "19" "$(printf '%s' "$out" | jq '.disabled_rules | length')"
  out="$(mcp_call tako_settings "$(jq -cn --arg t "$FAKE_ANTHROPIC $FAKE_GITHUB password=Zq9-EXAMPLE-pw" '{action:"paste_guard",text:$t}')")"
  check_eq "境界: 全規則を無効にすると何も当たらない" "false pass" \
    "$(printf '%s' "$out" | jq -r '"\(.check.hit) \(.check.verdict)"')"
  out="$(mcp_call tako_settings "{\"action\":\"paste_guard\",\"rules\":$all_on,\"unallow\":\"$id\"}")"
  check_eq "MCP で全規則を戻し、許可を外す" "0 0" \
    "$(printf '%s' "$out" | jq -r '"\(.disabled_rules | length) \(.allow | length)"')"
  out="$(printf '%s' "$FAKE_GOOGLE" | tk paste-guard check)"
  check_eq "外した値はまた当たる" "true" "$(printf '%s' "$out" | jget .hit)"
  printf '%s' "$out$st" > "$TMP/policy-out.json"
  check_eq "応答に生の値が載らない" "0" "$(leaks_in "$TMP/policy-out.json")"
}

# --- send: tako send / MCP tako_send_input --------------------------------------------------
phase_send() {
  local legacy="${1:-}" p err out got got_b
  p="$(root_pane)"
  if [ -z "$legacy" ]; then
    echo "== 4. tako send / MCP tako_send_input は止めずに届け、応答に伏せ字の警告"
  else
    echo "== 4 の A/B: TAKO_2050_LEGACY=1 の send は警告を付けない"
  fi
  err="$(tk send --pane "$p" "echo SEND2050${legacy}A $FAKE_GITHUB" 2>&1 >/dev/null)"
  if wait_for 10 pane_has "$p" "SEND2050${legacy}A"; then got=1; else got=0; fi
  out="$(mcp_call tako_send_input "$(jq -cn --argjson p "$p" --arg t "echo SEND2050${legacy}B $FAKE_ANTHROPIC" '{pane:$p,text:$t}')")"
  if wait_for 10 pane_has "$p" "SEND2050${legacy}B"; then got_b=1; else got_b=0; fi
  printf '%s\n%s\n' "$err" "$out" > "$TMP/send-out${legacy}.txt"
  if [ -z "$legacy" ]; then
    check "$([ "$got" = 1 ] && echo 1)" "CLI: 止めずにペインへ届く"
    check "$(grep -q '\[paste-guard\] verdict=warn github_token ghp_…qrst' <<<"$err" && echo 1)" \
      "CLI: 標準エラーに伏せ字の警告 1 行（$(printf '%s' "$err" | grep paste-guard | head -1)）"
    check "$([ "$got_b" = 1 ] && echo 1)" "MCP: 止めずにペインへ届く"
    check_eq "MCP: 応答に paste_guard（warn・anthropic_api_key・伏せ字）" "warn anthropic_api_key sk-ant-…ABCD" \
      "$(printf '%s' "$out" | jq -r '"\(.paste_guard.verdict) \(.paste_guard.findings[0].rule) \(.paste_guard.findings[0].masked)"')"
    check_eq "send の応答に生の値が載らない" "0" "$(leaks_in "$TMP/send-out${legacy}.txt")"
    check "$([ "$(tk paste-guard | jq '.routes.send.warned')" -ge 2 ] && echo 1)" "経路の累計 routes.send.warned が進む"
  else
    check "$([ "$got" = 1 ] && [ "$got_b" = 1 ] && echo 1)" "旧挙動: 届く"
    check "$(grep -q 'paste-guard' <<<"$err" || echo 1)" "旧挙動: CLI に警告が出ない（素通し）"
    check_eq "旧挙動: MCP の応答に paste_guard が無い" "null" "$(printf '%s' "$out" | jq -r '.paste_guard')"
  fi
}

# --- remote: スマホの入力（実 daemon + 偽 tailscale + curl）--------------------------------
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
NODE="fake-2050.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE2050","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# 隔離環境で serve の代わりに繋いでくるのは curl（#841 / #1452 と同じ作法）
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"
PORT=""
start_daemon() {
  tk remote start > "$TMP/daemon.log" 2>&1 || { echo "daemon を起動できない"; cat "$TMP/daemon.log"; return 1; }
  for _ in $(seq 1 200); do
    [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.port" ] && break
    sleep 0.1
  done
  PORT="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || { echo "daemon の port が読めない"; return 1; }
  for _ in $(seq 1 100); do
    [ "$(api GET /api/health)" = "200" ] && break
    sleep 0.1
  done
  api POST /api/pair '{"name":"iPhone","role":"interact"}' >/dev/null
  curl -s -o /dev/null -X POST "http://127.0.0.1:$PORT/api/admin/pair/approve" \
    -H "X-Tako-Admin: $(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.token")" -H 'Content-Type: application/json' \
    -d '{"device_id":"nFAKE2050","role":"interact"}'
}
stop_daemon() { tk remote stop >/dev/null 2>&1 || true; }
api() {
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "http://127.0.0.1:$PORT$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: fake-2050.tailfake.ts.net" \
      -H 'Content-Type: application/json' -d "$body"
  else
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "http://127.0.0.1:$PORT$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: fake-2050.tailfake.ts.net"
  fi
}
input_body() { jq -cn --arg t "$1" '{text:$t,newline:true}'; }

phase_remote() {
  local legacy="${1:-}" p code
  p="$(root_pane)"
  start_daemon || { fail "daemon が立たない"; return 1; }
  if [ -n "$legacy" ]; then
    echo "== 4 の A/B: TAKO_2050_LEGACY=1 のスマホの入力は確かめずに届く"
    code="$(api POST "/api/panes/$p/input" "$(input_body "echo REMOTE2050L $FAKE_GITHUB")")"
    check_eq "旧挙動: 200 で届く（確認なし）" "200 null" "$code $(jq -r '.paste_guard' "$TMP/body")"
    wait_for 10 pane_has "$p" "REMOTE2050L" && pass "旧挙動: ペインへ届く" || fail "旧挙動: ペインへ届かない"
    stop_daemon
    return 0
  fi
  echo "== 4. スマホの入力は確認の段なら送らずに伏せ字を返し、確かめたら送る"
  code="$(api POST "/api/panes/$p/input" "$(input_body "echo REMOTE2050A $FAKE_GITHUB")")"
  cp "$TMP/body" "$TMP/remote-409.json"
  check_eq "確認の段: 409 と伏せ字（github_token ghp_…qrst）" "409 confirm github_token ghp_…qrst" \
    "$code $(jq -r '"\(.paste_guard.verdict) \(.paste_guard.findings[0].rule) \(.paste_guard.findings[0].masked)"' "$TMP/body")"
  check_eq "409 の応答に生の値が載らない" "0" "$(leaks_in "$TMP/remote-409.json")"
  sleep 2
  pane_has "$p" "REMOTE2050A" && fail "確認の段: 送っていないのにペインへ届いた" || pass "確認の段: ペインへ 1 文字も届かない"
  code="$(api POST "/api/panes/$p/input" "$(jq -cn --arg t "echo REMOTE2050A $FAKE_GITHUB" '{text:$t,newline:true,confirm_secret:true}')")"
  check_eq "confirm_secret で送り直すと 200" "200" "$code"
  wait_for 10 pane_has "$p" "REMOTE2050A" && pass "確かめた入力はペインへ届く" || fail "確かめた入力がペインへ届かない"
  tk paste-guard warn >/dev/null
  code="$(api POST "/api/panes/$p/input" "$(input_body "echo REMOTE2050B $FAKE_GITHUB")")"
  check_eq "警告だけの段: 200 と伏せ字の警告" "200 warn ghp_…qrst" \
    "$code $(jq -r '"\(.paste_guard.verdict) \(.paste_guard.findings[0].masked)"' "$TMP/body")"
  wait_for 10 pane_has "$p" "REMOTE2050B" && pass "警告だけの段: ペインへ届く" || fail "警告だけの段: ペインへ届かない"
  tk paste-guard on >/dev/null
  code="$(api POST "/api/panes/$p/input" "$(input_body "echo REMOTE2050C plain")")"
  check_eq "当たらない入力は 200 だけ（警告なし）" "200 null" "$code $(jq -r '.paste_guard' "$TMP/body")"
  check_eq "経路の累計 routes.remote（prompted / warned）" "1 1" \
    "$(tk paste-guard | jq -r '"\(.routes.remote.prompted) \(.routes.remote.warned)"')"
  stop_daemon
}

# --- mod: 実物の claude の送信時の確認 ---------------------------------------------------------
WORK="$TMP/work"
mkdir -p "$WORK"
CCD="$TMP/cfg"
FAKE_LOG="$TMP/fake.log"
API_KEY="sk-ant-api03-fake2050fake2050fake2050fake2050fake2050fake2050AAAA"
start_fake_api() {
  : > "$FAKE_LOG"
  # 目印 = 偽の GitHub の鍵の本体の一部。偽 API は「最後の発話に目印があったか」の真偽だけを書く
  FAKE_LOG="$FAKE_LOG" FAKE_MARK="EXAMPLE0123456789abcdef" \
    node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 > "$TMP/fake.out" 2>&1 &
  FAKE_PID=$!
  API_PORT=""
  for _ in $(seq 1 50); do
    API_PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$TMP/fake.out" 2>/dev/null)"
    [ -n "$API_PORT" ] && break
    sleep 0.1
  done
  [ -n "$API_PORT" ] || { echo "偽 API が立たない"; return 1; }
  local last20
  last20="$(printf '%s' "$API_KEY" | tail -c 20)"
  mkdir -p "$CCD"
  jq -n --arg key "$last20" --arg work "$WORK" '{
    hasCompletedOnboarding: true, theme: "dark",
    customApiKeyResponses: { approved: [$key], rejected: [] },
    projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
  }' > "$CCD/.claude.json"
}
start_claude() {
  local name="$1" pane="$2" dirs="$3"
  tmx kill-session -t "=$name" >/dev/null 2>&1
  tmx new-session -d -s "$name" -x 160 -y 50 -c "$WORK" \
    env TAKO_ISOLATED=1 TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" \
    TAKO_ORCHESTRATOR_DIR="$TAKO_ORCHESTRATOR_DIR" CLAUDE_CONFIG_DIR="$CCD" \
    ANTHROPIC_BASE_URL="http://127.0.0.1:$API_PORT" ANTHROPIC_API_KEY="$API_KEY" DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    TAKO_PANE_ID="$pane" TAKO_CLI="$TAKO_BIN" CLAUDE_CODE_PLUGIN_DIRS="$dirs" \
    claude --model haiku
}
cap() { tmx capture-pane -p -t "=$1:" 2>/dev/null; }
screen_has() { local screen; screen="$(cap "$1")" && grep -q -- "$2" <<<"$screen"; }
type_line() {
  tmx send-keys -t "=$1:" -l "$2"
  sleep 1
  tmx send-keys -t "=$1:" Enter
}
key() { tmx send-keys -t "=$1:" "$2"; }
marked_count() { grep -c '"marked":true' "$FAKE_LOG" | tr -d ' '; }
reporting() { [ "$(tk mod --json 2>/dev/null | jq -r --argjson p "$1" '.panes[] | select(.pane == $p) | .state')" = reporting ]; }
dialog_up() {
  local screen
  screen="$(cap "$1")" && grep -q 'ghp_…qrst' <<<"$screen" && grep -Eq "Don't send|やめる" <<<"$screen"
}
CAP_DIR="$TMP/capture"
mkdir -p "$CAP_DIR"

phase_mod() {
  local legacy="${1:-}" p dirs s before
  # 前の段（send / remote）が root のペインへ同じ偽の鍵を届けている = そのペインでは確認済み。
  # mod 段は新しいペインを名乗る
  p="$(fresh_pane)"
  [ -n "$p" ] || { fail "mod 段のペインを割れない"; return 1; }
  dirs="$(tk mod --json | jq -r '.plugin_dir')"
  s="m2050${legacy}"
  start_claude "$s" "$p" "$dirs"
  if ! wait_for 90 reporting "$p"; then
    fail "claude（${s}）の mod の報告が届かない"
    cap "$s" | mask | tail -15 | sed 's/^/      | /'
    return 1
  fi
  if [ -n "$legacy" ]; then
    echo "== 3 の A/B: TAKO_2050_LEGACY=1 では mod の送信時の確認が出ない"
    before="$(marked_count)"
    type_line "$s" "Reply with exactly one word: LEGACYSENT key $FAKE_GITHUB"
    wait_for 40 screen_has "$s" '⏺ LEGACYSENT' && pass "旧挙動: 確認なしで送られ、答えが出る" || fail "旧挙動: 答えが出ない"
    check "$([ "$(marked_count)" -gt "$before" ] && echo 1)" "旧挙動: 鍵が偽 API へ届く（marked）"
    cap "$s" | mask > "$CAP_DIR/mod-legacy.txt"
    tmx kill-session -t "=$s" >/dev/null 2>&1
    return 0
  fi
  echo "== 3. 実物の claude の入力欄に入れた鍵で、送信時に確認が出る"
  # 1) やめる（既定の選択）→ API へ届かない
  before="$(marked_count)"
  type_line "$s" "Reply with exactly one word: DROPPED key $FAKE_GITHUB"
  if wait_for 20 dialog_up "$s"; then
    pass "送信時に確認が出る（伏せ字 ghp_…qrst と「やめる」）"
  else
    fail "送信時の確認が出ない"
  fi
  cap "$s" | mask > "$CAP_DIR/mod-confirm.txt"
  key "$s" Enter
  sleep 4
  cap "$s" | mask > "$CAP_DIR/mod-dropped.txt"
  check_eq "やめると偽 API へ 1 回も届かない（marked の数が増えない）" "$before" "$(marked_count)"
  screen_has "$s" '⏺ DROPPED' && fail "やめたのに答えが出た" || pass "やめた送信の答えは出ない"
  check "$(screen="$(cap "$s")" && grep -Eq 'not sent because|送りませんでした' <<<"$screen" && echo 1)" "やめた理由が画面に出る"
  # やめた文は入力欄へ戻る（落とした文は会話に残らないので、画面に残っていれば入力欄。
  # 入力欄の ❯ の後ろは普通の空白ではないので、文の頭から見る）
  check "$(screen_has "$s" 'Reply with exactly one word: DROPPED key' && echo 1)" \
    "やめた文が入力欄へ戻る（鍵を消して送り直せる）"
  # 戻った下書きを消す（Ctrl+U）
  key "$s" C-u
  sleep 1
  # 2) 送る → 届く
  type_line "$s" "Reply with exactly one word: CONFIRMED key $FAKE_GITHUB"
  wait_for 20 dialog_up "$s" || fail "2 回目の確認が出ない"
  key "$s" Down
  sleep 0.5
  key "$s" Enter
  if wait_for 40 screen_has "$s" '⏺ CONFIRMED'; then pass "送るを選ぶと送られ、答えが出る"; else fail "送るを選んでも答えが出ない"; fi
  check "$([ "$(marked_count)" -gt "$before" ] && echo 1)" "送ると鍵が偽 API へ届く（marked）"
  cap "$s" | mask > "$CAP_DIR/mod-confirmed.txt"
  # 3) 警告だけの段 → 確認なしで送り、トースト
  tk paste-guard warn >/dev/null
  type_line "$s" "Reply with exactly one word: WARNED key $FAKE_ANTHROPIC"
  if wait_for 15 screen_has "$s" 'sk-ant-…ABCD'; then pass "警告だけの段: トーストに伏せ字が出る"; else fail "警告だけの段: トーストが出ない"; fi
  cap "$s" | mask > "$CAP_DIR/mod-warned.txt"
  wait_for 40 screen_has "$s" '⏺ WARNED' && pass "警告だけの段: 止めずに送られる" || fail "警告だけの段: 答えが出ない"
  tk paste-guard on >/dev/null
  # 4) tako が流した値は二度聞かない（AI の送信で同じペインへ届けた鍵）
  local acked
  acked="$(tk paste-guard | jq '.routes.mod.acknowledged')"
  tk send --pane "$p" "echo ACK2050 $FAKE_GITHUB" >/dev/null 2>&1
  type_line "$s" "Reply with exactly one word: ACKED key $FAKE_GITHUB"
  if wait_for 40 screen_has "$s" '⏺ ACKED'; then pass "tako が流した値は確認なしで送られる"; else fail "tako が流した値でも止まった"; fi
  dialog_up "$s" && fail "tako が流した値で確認が出た" || pass "tako が流した値で確認が出ない"
  check "$([ "$(tk paste-guard | jq '.routes.mod.acknowledged')" -gt "$acked" ] && echo 1)" "経路の累計 routes.mod.acknowledged が進む"
  check "$([ "$(tk paste-guard | jq '.routes.mod.prompted')" -ge 2 ] && echo 1)" "経路の累計 routes.mod.prompted が 2 以上"
  cap "$s" | mask > "$CAP_DIR/mod-acked.txt"
  tmx kill-session -t "=$s" >/dev/null 2>&1
}

# --- visual: GUI の貼り付けの ⑫（visual-test paste-guard）------------------------------------
run_visual() {
  local label="$1" dump="$2" log="$3"
  shift 3
  mkdir -p "$dump" "$TMP/home" "$TMP/zdot"
  # 画像にユーザー名・ホスト名を写さない（#927）: 隔離 HOME + 固定プロンプト
  printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
  launch_isolated_gui "$log" TAKO_PERSIST=0 TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=paste-guard \
    TAKO_VISUAL_DUMP_DIR="$dump" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  for _ in $(seq 1 1800); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  echo "    [$label]"
  grep -E "TAKO_VISUAL_PIXEL: paste-guard warning|⑫|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$log" \
    | sed 's/^/      /' | mask | head -12
}

phase_visual() {
  echo "== 2 の実画面: GUI の貼り付け（visual-test paste-guard ⑫）"
  run_visual new "$TMP/visual-new" "$TMP/visual-new.log"
  check "$(grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual-new.log" && echo 1)" \
    "visual: ①〜⑫ が通る（警告だけの段は確認なしで届き通知が描かれる・許可・混在・規則の無効化）"
  check "$([ -s "$TMP/visual-new/paste-guard-warning.png" ] && echo 1)" "visual: 通知の画像（paste-guard-warning.png）を書き出した"
  echo "== 2 の実画面の A/B: TAKO_2050_LEGACY=1 は ⑫ で FAILED"
  run_visual legacy "$TMP/visual-legacy" "$TMP/visual-legacy.log" TAKO_2050_LEGACY=1
  check "$(grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test paste-guard ⑫' "$TMP/visual-legacy.log" && echo 1)" \
    "visual 旧挙動: ⑫（警告だけの段）で FAILED になる"
  check "$(grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual-legacy.log" || echo 1)" "visual 旧挙動: 通らない"
}

# --- 実行 -----------------------------------------------------------------------------------
REAL_CLAUDE="$(command -v claude 2>/dev/null || true)"
echo "claude: $( [ -n "$REAL_CLAUDE" ] && claude --version 2>/dev/null | head -1) / 作業 dir: $(printf '%s' "$TMP" | mask)"
if want mod || want ab; then
  if [ -z "$REAL_CLAUDE" ]; then
    unmeasured "PATH に claude が無い（mod の段を飛ばす）"
  else
    start_fake_api || exit 1
  fi
fi
if want policy || want send || want remote || want mod; then
  start_app new
  want policy && phase_policy
  want send && phase_send
  want remote && phase_remote
  want mod && [ -n "$REAL_CLAUDE" ] && phase_mod
  stop_app
fi
if want ab; then
  start_app legacy TAKO_2050_LEGACY=1
  check_eq "旧挙動のバイナリ（legacy_2050=true）" "true" "$(tk paste-guard | jget .legacy_2050)"
  phase_send L
  phase_remote L
  [ -n "$REAL_CLAUDE" ] && phase_mod L
  stop_app
fi
want visual && phase_visual

echo "== 5. 診断ログに偽の鍵の本文が無い"
check_eq "GUI の標準出力・標準エラー（app-*.log・visual-*.log）" "0" "$(leaks_in "$TMP"/app-*.log "$TMP"/visual-*.log)"
check_eq "persist.log" "0" "$(leaks_in "$TAKO_DATA_DIR/persist.log")"
if want send; then
  check "$(grep -q '貼り付けの確認: send:warn' "$TAKO_DATA_DIR/persist.log" 2>/dev/null && echo 1)" \
    "persist.log には規則名と件数の 1 行（send:warn）が残る"
fi
check_eq "daemon の状態 dir・ログ" "0" "$(leaks_in "$TMP/daemon.log" $(find "$TAKO_REMOTE_STATE_DIR" -type f 2>/dev/null))"
check_eq "偽 API のログ（真偽だけ）" "0" "$(leaks_in "$FAKE_LOG")"
grep '貼り付けの確認' "$TAKO_DATA_DIR/persist.log" 2>/dev/null | sed 's/^[^]]*\] *//' | sort | uniq -c | sed 's/^/    /' | head -12

REAL_AFTER="$(snap_real)"
check_eq "本物の ~/.claude*/skills・plugins は前後で同じ（${REAL_BEFORE}）" "$REAL_BEFORE" "$REAL_AFTER"
echo "採取: $(printf '%s' "$CAP_DIR" | mask)（KEEP=1 で残す）"
echo "== 結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
