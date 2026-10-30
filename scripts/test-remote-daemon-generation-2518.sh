#!/usr/bin/env bash
# test-remote-daemon-generation-2518.sh — install をまたいだ remote daemon の世代の実経路テスト（#2518）
#
# 隔離した state / data ディレクトリ・偽の tailscale CLI（TAKO_TAILSCALE_BIN）・一時ディレクトリの
# .app で**本物の daemon**（`tako remote serve`）を走らせ、本物の `install_bundle_in_place`
# （contents-swap = 本番の install と同じ差し替え）で .app を入れ替えたあと、
#   ① 旧挙動（TAKO_2518_LEGACY=1）: status に世代が出ず、`remote start` は「既に起動中」で旧版が残る
#   ② 新挙動: status の serve_generation が replaced（renewable）・warnings に 1 行、
#      `remote start`（= GUI 起動時の自動復帰が呼ぶのと同じ spawn_daemon）が旧版を止めて起こし直す。
#      「起動していた」（desired）は消えない
#   ③ 現行の daemon は起こし直さない（`remote start` は「既に起動中」・pid 不変）
#   ④ #2518 より前の daemon（PID ファイルが 3 行 = 識別なし）は ctime と起動時刻で判定する
#   ⑤ 実行ファイルが消えた daemon は missing と言い、止めない（起こし直す先が無い）
# を実測する。GUI 起動時の判断（`remote_autostart::probe`）は GUI を立てずに
# crates/tako-control/tests/issue2518_remote_daemon_generation_watchdog.rs が本物のまま呼ぶ。
#
# **本番の tailscale / serve 設定 / remote デーモン / .app には一切触らない**
# （tailscale の呼び出しは全部この偽物へ向く。state・data・.app は mktemp 配下。
# 落とすのは自分で起こした daemon だけ = `tako remote stop` と記録した pid）。
#
# 使い方: bash scripts/test-remote-daemon-generation-2518.sh
#   証拠の JSON を残すなら TAKO_2518_EVIDENCE_DIR=<dir> を渡す（status の実測をそこへ書く）
set -euo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2518_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh"          # run_heavy = 重い処理を順番待ちへ（#1994）
. "$REPO_ROOT/scripts/lib/bundle-install.sh" # install_bundle_in_place = 本番の install と同じ差し替え
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_ne() {
  if [ "$2" != "$3" ]; then pass "$1"; else fail "$1（'${2}' と同じ）"; fi
}
check_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-2518-XXXXXX")"
CTL=""        # 停止・後片付けに使う CLI（差し替えの影響を受けない複製）
DAEMON_PIDS=""

# 自分で起こした daemon だけを止める（pkill / killall は本番にも当たる）
cleanup() {
  if [ -n "$CTL" ] && [ -s "$TMP/state/tako-remote.pid" ]; then
    "$CTL" remote stop >/dev/null 2>&1 || true
  fi
  local p
  for p in $DAEMON_PIDS; do
    if kill -0 "$p" 2>/dev/null; then kill "$p" 2>/dev/null || true; fi
  done
  rm -rf "${TMP:?}"
}
# 途中で死んだテストを「緑」に化けさせない（bash 3.2 の set -e + EXIT trap。#1864）
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup

# contents-swap（renamex_np）と BSD の stat を使う = macOS だけ
if [ "$(uname -s)" != "Darwin" ]; then
  echo "macOS 以外では走らせない（contents-swap は macOS の install の形）"
  tako_exit 0
fi

# --- 偽 tailscale CLI（serve 設定は $TMP/serve.json）---------------------------
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
NODE="fake-2518.tailfake.ts.net"
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

# --- 対象バイナリ（今の木から建てる。stale な target を掴まない）-----------------
echo "== ビルド =="
(cd "$REPO_ROOT" && run_heavy cargo build -p tako-cli --quiet)
TAKO_BIN="$REPO_ROOT/target/debug/tako"
[ -x "$TAKO_BIN" ] || { echo "tako CLI が見つからない: $TAKO_BIN"; exit 1; }

# 一時の .app（本番の /Applications/tako.app と同じ形 = トップレベルは Contents だけ）。
# 隔離モードの serve_binary() は CLI 自身を選ぶので、.app の中の CLI で起こした daemon は
# .app の中の実行ファイルで動く（本番の daemon と同じ）
APP="$TMP/Applications/tako.app"
APP_TAKO="$APP/Contents/MacOS/tako"
mkdir -p "$APP/Contents/MacOS"
command cp -f "$TAKO_BIN" "$APP_TAKO"
# 次の版（中身は同じでも別のファイル = install の ditto が作るのと同じ）
make_new_app() {
  local dest="$1"
  rm -rf "${dest:?}"
  mkdir -p "$dest/Contents/MacOS"
  command cp -f "$TAKO_BIN" "$dest/Contents/MacOS/tako"
}
CTL="$TMP/ctl/tako"
mkdir -p "$TMP/ctl"
command cp -f "$TAKO_BIN" "$CTL"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_REMOTE_STATE_DIR="$TMP/state"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_TAILSCALE_BIN="$FAKE_TS"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_DISCOVERY_DIR"
case "$TAKO_REMOTE_STATE_DIR" in
  "$TMP"/*) : ;;
  *) echo "state ディレクトリが隔離されていない: $TAKO_REMOTE_STATE_DIR"; exit 1 ;;
esac
PID_FILE="$TAKO_REMOTE_STATE_DIR/tako-remote.pid"
EVIDENCE="${TAKO_2518_EVIDENCE_DIR:-}"
if [ -n "$EVIDENCE" ]; then mkdir -p "$EVIDENCE"; fi

# status の欄を読む（$1 = 使う CLI・$2 = ドット区切りのキー）
field_of() {
  python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur, dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$1" "$2"
}
# status を 1 回取ってファイルへ（$1 = 使う CLI・$2 = 書き先・残りは env の前置き）
take_status() {
  local cli="$1" out="$2"
  shift 2
  env "$@" "$cli" remote status >"$out" 2>"$out.err"
}
daemon_pid() { head -1 "$PID_FILE" 2>/dev/null | tr -dc '0-9'; }

# daemon の起動を待つ（URL と serve の自己検査の結果が書かれるまで）
wait_daemon_up() {
  local i
  for i in $(seq 1 300); do
    if [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.url" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.serve" ]; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}
# pid の終了を待つ（$1 = pid）
wait_gone() {
  local i
  for i in $(seq 1 100); do
    kill -0 "$1" 2>/dev/null || return 0
    sleep 0.1
  done
  return 1
}
# PID ファイルに記録された起動時刻より、壁時計の秒が進むまで待つ（ctime の比較を秒で確かめるため）
wait_past_start() {
  local start i
  start="$(sed -n 3p "$PID_FILE" | tr -dc '0-9')"
  for i in $(seq 1 30); do
    [ "$(date +%s)" -gt "$start" ] && return 0
    sleep 0.1
  done
  return 1
}
remember_pid() { DAEMON_PIDS="$DAEMON_PIDS $1"; }

echo
echo "== ③ 起動と現行の判定（現行は起こし直さない）=="
if ! "$APP_TAKO" remote start >"$TMP/start1.json" 2>"$TMP/start1.err"; then
  echo "daemon を起動できなかった:"; cat "$TMP/start1.err"; exit 1
fi
wait_daemon_up || { echo "daemon の起動を待てなかった"; cat "$TMP/start1.err"; exit 1; }
PID1="$(daemon_pid)"
remember_pid "$PID1"
check_contains "PID ファイルの 4 行目に実行ファイルの識別が残る" "$(sed -n 4p "$PID_FILE")" "exe-v1:"
take_status "$APP_TAKO" "$TMP/s1.json"
check_eq "起動直後は current" "current" "$(field_of "$TMP/s1.json" serve_generation.state)"
check_eq "起動直後は立て直し対象でない" "false" "$(field_of "$TMP/s1.json" serve_generation.renewable)"
check_eq "起動直後の判定は識別に基づく" "stamp" "$(field_of "$TMP/s1.json" serve_generation.basis)"
RC=0
"$APP_TAKO" remote start >"$TMP/start-again.json" 2>"$TMP/start-again.err" || RC=$?
check_ne "現行の daemon に start は失敗で返る" "0" "$RC"
check_contains "理由は「既に起動中」" "$(cat "$TMP/start-again.err")" "既に起動中"
check_eq "現行の daemon は起こし直さない（pid 不変）" "$PID1" "$(daemon_pid)"

echo
echo "== install（本物の install_bundle_in_place = contents-swap）=="
INO_BEFORE="$(stat -f %i "$APP_TAKO")"
make_new_app "$TMP/build1/tako.app"
STRATEGY="$(install_bundle_in_place "$TMP/build1/tako.app" "$APP")"
check_eq "差し替えは contents-swap" "contents-swap" "$STRATEGY"
INO_AFTER="$(stat -f %i "$APP_TAKO")"
check_ne "同じパスのまま実行ファイルが別のファイルになった" "$INO_BEFORE" "$INO_AFTER"
if kill -0 "$PID1" 2>/dev/null; then pass "旧版の daemon（PID ${PID1}）は動き続けている"; else fail "旧版の daemon が落ちた"; fi

echo
echo "== ① 旧挙動（TAKO_2518_LEGACY=1）: 旧版のまま残る =="
take_status "$APP_TAKO" "$TMP/s-legacy.json" TAKO_2518_LEGACY=1
check_eq "旧挙動の status は世代を出さない" "null" "$(field_of "$TMP/s-legacy.json" serve_generation)"
check_eq "旧挙動でも serve_binary のパスは同じ（パスの文字列では見分けられない）" "$APP_TAKO" "$(field_of "$TMP/s-legacy.json" serve_binary)"
RC=0
TAKO_2518_LEGACY=1 "$APP_TAKO" remote start >"$TMP/start-legacy.json" 2>"$TMP/start-legacy.err" || RC=$?
check_ne "旧挙動の start は失敗で返る" "0" "$RC"
check_contains "旧挙動の start は「既に起動中」" "$(cat "$TMP/start-legacy.err")" "既に起動中"
check_eq "旧挙動では旧版の daemon が残る（pid 不変）" "$PID1" "$(daemon_pid)"

echo
echo "== ② 新挙動: status が旧版と言い、start（= 自動復帰の spawn_daemon）が入れ替える =="
take_status "$APP_TAKO" "$TMP/s2.json"
if [ -n "$EVIDENCE" ]; then command cp -f "$TMP/s2.json" "$EVIDENCE/status-replaced.json"; fi
check_eq "差し替え後は replaced" "replaced" "$(field_of "$TMP/s2.json" serve_generation.state)"
check_eq "立て直し対象" "true" "$(field_of "$TMP/s2.json" serve_generation.renewable)"
check_eq "判定は識別に基づく" "stamp" "$(field_of "$TMP/s2.json" serve_generation.basis)"
check_contains "warnings に旧版の 1 行" "$(field_of "$TMP/s2.json" warnings)" "旧版のまま動いています"
check_contains "CLI は旧版を stderr にも出す" "$(cat "$TMP/s2.json.err")" "旧版のまま動いています"
check_eq "「起動していた」は記録されている" "true" "$(field_of "$TMP/s2.json" desired)"
if ! "$APP_TAKO" remote start >"$TMP/start-renew.json" 2>"$TMP/start-renew.err"; then
  fail "start が旧版を入れ替えられなかった: $(cat "$TMP/start-renew.err")"
else
  pass "start が成功で返る"
fi
if [ -n "$EVIDENCE" ]; then command cp -f "$TMP/start-renew.json" "$EVIDENCE/start-renewed.json"; fi
check_eq "応答に入れ替えた旧版の pid" "$PID1" "$(field_of "$TMP/start-renew.json" renewed.previous_pid)"
check_contains "CLI は入れ替えたことを言う" "$(cat "$TMP/start-renew.err")" "新版で起動し直しました"
wait_daemon_up || fail "入れ替え後の daemon の起動を待てなかった"
PID2="$(daemon_pid)"
remember_pid "$PID2"
check_ne "新しい daemon が立った" "$PID1" "$PID2"
if wait_gone "$PID1"; then pass "旧版の daemon（PID ${PID1}）は止まった"; else fail "旧版の daemon が残っている"; fi
take_status "$APP_TAKO" "$TMP/s3.json"
if [ -n "$EVIDENCE" ]; then command cp -f "$TMP/s3.json" "$EVIDENCE/status-after-renew.json"; fi
check_eq "入れ替え後は current" "current" "$(field_of "$TMP/s3.json" serve_generation.state)"
check_eq "入れ替えても「起動していた」は消えない" "true" "$(field_of "$TMP/s3.json" desired)"
check_contains "新しい daemon は今の実行ファイルの識別を残す" "$(sed -n 4p "$PID_FILE")" ":$INO_AFTER:"

echo
echo "== ④ #2518 より前の daemon（PID ファイルが 3 行）は ctime と起動時刻で判定する =="
# 識別の行を落として、記録の無い古い daemon の PID ファイルを再現する
head -3 "$PID_FILE" >"$TMP/pid3" && command cp -f "$TMP/pid3" "$PID_FILE"
take_status "$APP_TAKO" "$TMP/s4.json"
check_eq "記録が無くても、起動より前に置かれたファイルなら current" "current" "$(field_of "$TMP/s4.json" serve_generation.state)"
check_eq "判定は ctime に基づく" "ctime" "$(field_of "$TMP/s4.json" serve_generation.basis)"
wait_past_start || fail "起動時刻の秒が進まない"
make_new_app "$TMP/build2/tako.app"
install_bundle_in_place "$TMP/build2/tako.app" "$APP" >/dev/null
take_status "$APP_TAKO" "$TMP/s5.json"
if [ -n "$EVIDENCE" ]; then command cp -f "$TMP/s5.json" "$EVIDENCE/status-replaced-legacy-pidfile.json"; fi
check_eq "起動後に置かれたファイルなら replaced" "replaced" "$(field_of "$TMP/s5.json" serve_generation.state)"
check_eq "判定は ctime に基づく（差し替え後）" "ctime" "$(field_of "$TMP/s5.json" serve_generation.basis)"
if "$APP_TAKO" remote start >"$TMP/start-renew2.json" 2>"$TMP/start-renew2.err"; then
  pass "記録の無い旧版も start で入れ替わる"
else
  fail "記録の無い旧版を入れ替えられなかった: $(cat "$TMP/start-renew2.err")"
fi
wait_daemon_up || fail "入れ替え後の daemon の起動を待てなかった"
PID3="$(daemon_pid)"
remember_pid "$PID3"
check_ne "記録の無い旧版の後に新しい daemon が立った" "$PID2" "$PID3"
check_contains "入れ替えた daemon は識別を残す（2 回目からは識別で判定する）" "$(sed -n 4p "$PID_FILE")" "exe-v1:"

echo
echo "== ⑤ 実行ファイルが消えた daemon は missing と言い、止めない =="
rm -rf "${APP:?}"
take_status "$CTL" "$TMP/s6.json"
check_eq "実行ファイルが無ければ missing" "missing" "$(field_of "$TMP/s6.json" serve_generation.state)"
check_eq "missing は立て直し対象にしない" "false" "$(field_of "$TMP/s6.json" serve_generation.renewable)"
check_contains "missing も warnings に出す" "$(field_of "$TMP/s6.json" warnings)" "もうありません"
if kill -0 "$PID3" 2>/dev/null; then pass "missing の daemon は止めていない"; else fail "missing の daemon が止まった"; fi

"$CTL" remote stop >/dev/null 2>&1 || true
if wait_gone "$PID3"; then pass "後片付け: 自分の daemon を止めた"; else fail "後片付けで daemon が止まらない"; fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
