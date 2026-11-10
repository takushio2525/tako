#!/usr/bin/env bash
# tako:run bash scripts/test-remote-keys-2765.sh
#
# test-remote-keys-2765.sh — スマホ（PWA）のクイックキー（esc / tab / ^C / ↑ / ↓）が、remote daemon の
# `POST /api/panes/<id>/input` の `{keys}` を経て**キーとして**ペインへ届くかの実経路テスト（#2765）
#
# 実 GUI（隔離 tako-app・仮想ディスプレイ tako-vd・器のペイン = TAKO_PERSIST=1）+ 実 dispatch + 実 daemon
# （`tako remote start` + 偽の tailscale + curl）で、ペインに置いた記録係（raw モードの perl が受けた 1 バイトずつを
# 16 進でファイルへ書く）に届いたバイトを読む。段は依頼の受け入れ条件:
#   keys    1 / 2. `{keys}` の Escape / C-c / Up / Down / Tab と `C-<英字>`（C-a / C-z / 大文字の C-D）と
#              空白区切りの列（`C-c C-c` = #42 の tmux send-keys と同じ綴り）が、名前の文字ではなくキーのバイトで届く
#   refuse     知らない名前・空の列は 400 で 1 バイトも届かず、直後の送信も通る（断りで IPC を捨てない）
#   dialog  4. 選択肢ダイアログの表示中は #2743（`tako send --key`）と同じ断りの文で送らない。断りの後に
#              生の ESC を含む本文（#748 の素通し）を送ると、それが最初に届く = 断った keys は 1 バイトも届いていない
#   pwa     3. PWA（Playwright の chromium。`/api/*` を実 daemon へ中継）の term ビューのクイックキーを押すと、
#              esc / tab / ^C / ↑ / ↓ がキーとして届く
#   ab      5. 同じバイナリの `TAKO_2765_LEGACY=1` の daemon では名前の文字（`Escape` = 45 73 63 61 70 65）が届き、
#              ダイアログ中の断りは 502 で IPC の接続を捨てる（直後の送信が 503）= 修正前の挙動
#
# **本番の tako / tailscale / remote には触らない**: tailscale の呼び出しは偽物へ、HOME / state / data / discovery /
# tmux は mktemp 配下と専用ソケット。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# 落とすのは自分で起こした pid とソケットだけ（pkill / killall は使わない）。
#
# 使い方: bash scripts/test-remote-keys-2765.sh   （ONLY=keys|refuse|dialog|pwa|ab で 1 段だけ・KEEP=1 で一時 dir を残す）
# **CI には登録しない**（仮想ディスプレイと実 daemon が要る）。CI 側の担保は remote.rs の単体（要求の形・断りの
# 状態コード）と番犬 `issue2765_remote_keys_watchdog`（daemon の keys の行き先・PWA のキー名が語彙にあるか）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TAKO_CLI \
  TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TAKO_2765_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
# 通った行にも実際の値（状態コード | 届いたバイトの 16 進）を出す = 実測の証拠として読める
check_eq() { if [ "$2" = "$3" ]; then pass "$1 → '${3}'"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 3; }
command -v perl >/dev/null 2>&1 || { echo "未実測: perl が無い（記録係に使う）"; exit 3; }
command -v tmux >/dev/null 2>&1 || { echo "未実測: tmux が無い（器のペインに使う）"; exit 3; }

# --- ビルド -------------------------------------------------------------------------------
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1

# `/tmp` 直下に短く取る（daemon の待ち受けと tmux が sun_path の上限に当たらない。#1441）
TMP="$(mktemp -d /tmp/tk2765-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tk2765-$$"

cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # ソケット名を明示した起動は自分で畳む（#1192。サーバーを止めてもファイルが残る）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${TMUX_SOCKET:?}"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# --- 偽の tailscale -------------------------------------------------------------------------
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
NODE="fake-2765.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE2765","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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
fake_exec_warm "$FAKE_TS"

# --- 隔離した環境 ---------------------------------------------------------------------------
REAL_HOME="$HOME"
# Playwright（node）の置き場は PATH を絞る前に覚える
NODE_DIR="$(dirname "$(command -v node 2>/dev/null || echo /usr/local/bin/node)")"
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_REMOTE_STATE_DIR="$TMP/remote"
export TAKO_AUTORENAME=0
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# #841: ループバック TCP では接続元プロセスを検証する。serve の代わりに繋ぐのは curl と Playwright の中継（node）
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl,node"
# #1452: 承認は「呼び出し元が tako-app か」も見る。この隔離環境では curl を GUI 側として名乗る
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_REMOTE_STATE_DIR" "$TMP/rec"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done
# 断りの文は日本語で比べる（英語は daemon / dispatch の単体と網羅テストが縛る）
printf '{"language":"ja"}' > "$TAKO_DATA_DIR/settings.json"
tk() { "$TAKO_BIN" "$@"; }

# 選択肢ダイアログの画面（dispatch のテストの `LIMIT_DIALOG_748` と同じ形 = claude の利用制限の選択）
cat > "$TMP/dialog.txt" <<'DLG'
⏺ 実装を続けます
  ⎿  Claude usage limit reached. Your limit will reset at 3am.

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   What do you want to do?

   ❯ 1. Stop and wait for limit to reset
     2. Upgrade to Max 20x for higher session limits every month

   Enter to confirm · Esc to cancel
DLG

# --- GUI ----------------------------------------------------------------------------------
echo "=== 準備: 隔離 tako-app（器のペイン）と daemon を起動 ==="
# 器のペインで測る: PWA は器のペイン（`tmux_target` を持つ）にだけ term ビューとクイックキーを出す
ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-1460,420,1000,960}"
rc=0
launch_isolated_gui "$TMP/app.log" TAKO_PERSIST=1 || rc=$?
if [ "$rc" -eq 4 ]; then
  echo "tako-vd を用意できない（蓋が閉じている等）。実 GUI が要るので未実測で止める"
  exit 4
elif [ "$rc" -ne 0 ]; then
  echo "隔離 GUI を起こせない（${rc}）"; exit 1
fi
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
if [ "$(tk list | jq '.tabs | length')" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない。中止する"
  exit 1
fi
pane_ids() { tk list 2>/dev/null | jq -r '.tabs[].panes[].id' | sort -n; }
ROOT_PANE="$(pane_ids | sed -n 1p)"
case "$ROOT_PANE" in ''|*[!0-9]*) echo "隔離 GUI のペインが読めない（'${ROOT_PANE}'）"; exit 1 ;; esac
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・ペイン ${ROOT_PANE}）"

wait_until() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 0.1
  done
  return 1
}
has_file() { [ -e "$1" ]; }
# pipefail の下では grep -q へパイプしない（読み手が先に抜けると左辺が SIGPIPE で落ちる間欠 = #1633）
pane_shows() { local out; out="$(tk read --pane "$1" 2>/dev/null)" && [[ "$out" == *"$2"* ]]; }

# 記録係のペインを作る。$1 = 名前（記録のファイル名）・$2 = 先に画面へ出すファイル（任意）。ペイン ID を出す。
# 記録係は raw・echo なしの端末で 1 バイトずつを 16 進 1 行でファイルへ書く（^C も 03 のまま届く = ISIG なし）
recorder_pane() {
  local name="$1" show="${2:-}" before after pane rec cmd
  rec="$TMP/rec/$name"
  before="$(pane_ids)"
  tk split --pane "$ROOT_PANE" --down >/dev/null 2>&1
  wait_until 100 test "$(pane_ids | wc -l)" -gt "$(printf '%s\n' "$before" | wc -l)" || return 1
  after="$(pane_ids)"
  pane="$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | sed -n 1p)"
  [ -n "$pane" ] || return 1
  cmd="stty raw -echo; exec perl -e '\$|=1; binmode STDIN; while (sysread(STDIN, my \$b, 1)) { printf \"%02x\\n\", ord \$b }' > $rec"
  [ -n "$show" ] && cmd="clear; cat $show; $cmd"
  tk send --pane "$pane" "$cmd" >/dev/null 2>&1 || return 1
  wait_until 200 has_file "$rec" || return 1
  # 記録係が読み始めるまで（ファイルはリダイレクトの時点で在る。raw は exec の前に効いている = 早く書いた分も残る）
  sleep 0.5
  printf '%s\n' "$pane"
}
rec_lines() { wc -l < "$1" | tr -d ' '; }
# 段の終わりに記録係のペインを閉じる（分割を重ねるとペインが低くなり、ダイアログの画面が収まらない）
close_pane() { tk close --pane "$1" >/dev/null 2>&1 || true; }
# 記録の n 行目より後ろ（16 進を空白で繋ぐ）
rec_since() { tail -n "+$(( $2 + 1 ))" "$1" | tr '\n' ' ' | sed 's/ $//'; }
# 新しい行が来るまで（上限 $2 × 0.1 秒）待ち、来たら 0.6 秒静かになるまで待つ
rec_settle() {
  local file="$1" base="$2" limit="${3:-50}" i last now
  for i in $(seq 1 "$limit"); do
    [ "$(rec_lines "$file")" -gt "$base" ] && break
    sleep 0.1
  done
  last="$(rec_lines "$file")"
  while :; do
    sleep 0.6
    now="$(rec_lines "$file")"
    [ "$now" = "$last" ] && break
    last="$now"
  done
}

# --- daemon -------------------------------------------------------------------------------
PORT=""
HOST_HDR="fake-2765.tailfake.ts.net"
api() {
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "http://127.0.0.1:$PORT$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR" \
      -H 'Content-Type: application/json' -d "$body"
  else
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "http://127.0.0.1:$PORT$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR"
  fi
}
# $@ = daemon へ渡す env（VAR=VAL…。A/B の段だけ使う）
start_daemon() {
  if ! env ${@+"$@"} "$TAKO_BIN" remote start > "$TMP/daemon.log" 2>&1; then
    echo "daemon を起動できない"; cat "$TMP/daemon.log"; return 1
  fi
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
    -d '{"device_id":"nFAKE2765","role":"interact"}'
  # daemon が GUI の IPC を掴むまで（List が通れば input も通る）
  for _ in $(seq 1 100); do
    [ "$(api GET /api/panes)" = "200" ] && break
    sleep 0.1
  done
}
stop_daemon() { tk remote stop >/dev/null 2>&1 || true; }
keys_body() { jq -cn --arg k "$1" '{keys:$k}'; }

# `{keys}` を 1 回送って、届いたバイトを「状態コード | 16 進」で出す。$1 = ペイン・$2 = 記録・$3 = keys
send_keys_measure() {
  local pane="$1" rec="$2" keys="$3" base code
  base="$(rec_lines "$rec")"
  code="$(api POST "/api/panes/$pane/input" "$(keys_body "$keys")")"
  rec_settle "$rec" "$base" 30
  printf '%s | %s' "$code" "$(rec_since "$rec" "$base")"
}

# --- keys: 5 キーと C-<英字> と列 ----------------------------------------------------------------
phase_keys() {
  echo "== 1 / 2. {keys} はキーのバイトで届く（名前の文字ではない）"
  local p rec
  p="$(recorder_pane keys)" || { fail "記録係のペインを作れない"; return 1; }
  rec="$TMP/rec/keys"
  check_eq "Escape" "200 | 1b" "$(send_keys_measure "$p" "$rec" Escape)"
  check_eq "C-c" "200 | 03" "$(send_keys_measure "$p" "$rec" C-c)"
  check_eq "Up" "200 | 1b 5b 41" "$(send_keys_measure "$p" "$rec" Up)"
  check_eq "Down" "200 | 1b 5b 42" "$(send_keys_measure "$p" "$rec" Down)"
  check_eq "Tab" "200 | 09" "$(send_keys_measure "$p" "$rec" Tab)"
  check_eq "C-a（C-<英字> の語彙）" "200 | 01" "$(send_keys_measure "$p" "$rec" C-a)"
  check_eq "C-z" "200 | 1a" "$(send_keys_measure "$p" "$rec" C-z)"
  check_eq "C-D（大文字も同じ制御文字）" "200 | 04" "$(send_keys_measure "$p" "$rec" C-D)"
  check_eq "空白区切りの列 \"C-c C-c\"（#42 の綴り）" "200 | 03 03" "$(send_keys_measure "$p" "$rec" "C-c C-c")"
  check_eq "Enter" "200 | 0d" "$(send_keys_measure "$p" "$rec" Enter)"
  close_pane "$p"
}

# --- refuse: 知らない名前・空の列 ------------------------------------------------------------------
phase_refuse() {
  echo "== 境界: 知らない名前・空の列は 400 で何も届かず、直後の送信も通る"
  local p rec out
  p="$(recorder_pane refuse)" || { fail "記録係のペインを作れない"; return 1; }
  rec="$TMP/rec/refuse"
  out="$(send_keys_measure "$p" "$rec" Escapee)"
  check_eq "知らない名前 Escapee" "400 | " "$out"
  check_eq "断りの文に名前が入る" "1" "$(jq -r '.error' "$TMP/body" | grep -c 'Escapee')"
  out="$(send_keys_measure "$p" "$rec" "C-c Nope")"
  check_eq "列の途中に知らない名前（先頭の C-c も送らない）" "400 | " "$out"
  out="$(send_keys_measure "$p" "$rec" "")"
  check_eq "空の列" "400 | " "$out"
  # 断りは app の答え = IPC の接続は健全（捨てると 5 秒の間 503 になる）
  check_eq "断りの直後の送信も届く" "200 | 1b" "$(send_keys_measure "$p" "$rec" Escape)"
  close_pane "$p"
}

# --- dialog: ダイアログ中の断り --------------------------------------------------------------------
# $1 = 期待する状態コード・$2 = 断りの直後の送信の期待（"200 | 1b 5a" / "503 | "）
phase_dialog() {
  local want_code="$1" want_next="$2" p rec out cli_err daemon_err
  p="$(recorder_pane dialog "$TMP/dialog.txt")" || { fail "記録係のペインを作れない"; return 1; }
  rec="$TMP/rec/dialog"
  wait_until 100 pane_shows "$p" "Stop and wait for limit" || { tk read --pane "$p" > "$TMP/dialog-screen.txt" 2>&1; fail "ダイアログの画面が出ない"; return 1; }
  # #2743 の口（CLI `tako send --key`）が同じペインで断る = 判定の前提（画面がダイアログと読まれている）
  cli_err="$(tk send --pane "$p" --key Escape 2>&1 >/dev/null)"
  check_eq "前提: tako send --key も断る（usage_limit と respond --pane ${p}）" "1 1" \
    "$(grep -c usage_limit <<<"$cli_err") $(grep -c -- "respond --pane $p" <<<"$cli_err")"
  out="$(send_keys_measure "$p" "$rec" Escape)"
  check_eq "ダイアログ中の Escape は ${want_code} で 1 バイトも届かない" "$want_code | " "$out"
  daemon_err="$(jq -r '.error' "$TMP/body")"
  check_eq "断りの文は tako send --key と同じ（daemon の文が CLI の出力に含まれる）" "1" \
    "$(case "$cli_err" in *"$daemon_err"*) echo 1 ;; *) echo 0 ;; esac)"
  # 生の ESC を含む本文は #748 の素通し = ダイアログ中でも届く。最初に届いたのがこれなら、断った keys は届いていない
  local base code
  base="$(rec_lines "$rec")"
  code="$(api POST "/api/panes/$p/input" "$(jq -cn '{text:"\u001bZ",newline:false}')")"
  rec_settle "$rec" "$base" 30
  check_eq "断りの直後の送信（生の ESC + Z）" "$want_next" "$code | $(rec_since "$rec" "$base")"
  close_pane "$p"
}

# --- pwa: PWA のクイックキー ----------------------------------------------------------------------
phase_pwa() {
  echo "== 3. PWA の term ビューのクイックキー → daemon → ペイン"
  local p rec pw_rc=0 got
  if [ ! -d "$REPO_ROOT/web/tako-remote/node_modules" ]; then
    fail "web/tako-remote/node_modules が無い（cd web/tako-remote && npm ci）"
    return 1
  fi
  p="$(recorder_pane pwa)" || { fail "記録係のペインを作れない"; return 1; }
  rec="$TMP/rec/pwa"
  (
    cd "$REPO_ROOT/web/tako-remote" &&
    HOME="$REAL_HOME" PATH="$NODE_DIR:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin" \
    TAKO_PWA_PORT="${TAKO_PWA_PORT:-5276}" \
    TAKO_2765_LIVE_BASE="http://127.0.0.1:$PORT" TAKO_2765_LIVE_HOST="$HOST_HDR" TAKO_2765_LIVE_PANE="$p" \
    npx playwright test e2e/quick-keys-2765-live.spec.js --reporter=line --output="$TMP/pw-out"
  ) > "$TMP/pw.log" 2>&1 || pw_rc=$?
  grep -E "passed|failed|skipped|Error|TAKO_2765_KEY" "$TMP/pw.log" | sed 's/^/    /' | sed -n '1,12p'
  check_eq "Playwright（chromium）の live spec が通る" "0" "$pw_rc"
  check_eq "押したボタンの状態コード（esc tab ^C ↑ ↓）" "200 200 200 200 200" \
    "$(sed -n 's/^.*TAKO_2765_KEY //p' "$TMP/pw.log" | jq -r '.status' 2>/dev/null | tr '\n' ' ' | sed 's/ $//')"
  rec_settle "$rec" 0 30
  got="$(rec_since "$rec" 0)"
  check_eq "ペインに届いたバイト（esc tab ^C ↑ ↓ の順）" "1b 09 03 1b 5b 41 1b 5b 42" "$got"
  close_pane "$p"
}

if want keys || want refuse || want dialog || want pwa; then
  start_daemon || { fail "daemon が立たない"; exit 1; }
  want keys && phase_keys
  want refuse && phase_refuse
  if want dialog; then
    echo "== 4. ダイアログ中は #2743 と同じ断り（文言）で送らない"
    phase_dialog 400 "200 | 1b 5a"
  fi
  want pwa && phase_pwa
  stop_daemon
fi

if want ab; then
  echo "== 5. A/B: TAKO_2765_LEGACY=1 の daemon は名前の文字を打ち、ダイアログの断りで IPC を捨てる（修正前）"
  start_daemon TAKO_2765_LEGACY=1 || { fail "daemon が立たない"; exit 1; }
  p="$(recorder_pane legacy)" || { fail "記録係のペインを作れない"; exit 1; }
  rec="$TMP/rec/legacy"
  check_eq "旧挙動: Escape は文字の Escape" "200 | 45 73 63 61 70 65" "$(send_keys_measure "$p" "$rec" Escape)"
  check_eq "旧挙動: C-c は文字の C-c" "200 | 43 2d 63" "$(send_keys_measure "$p" "$rec" C-c)"
  close_pane "$p"
  phase_dialog 502 "503 | "
  stop_daemon
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
