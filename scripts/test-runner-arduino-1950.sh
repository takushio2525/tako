#!/bin/bash
# test-runner-arduino-1950.sh — Arduino のスケッチ（.ino）の Code Runner の実経路（Issue #1950）
#
# 隔離 GUI（仮想ディスプレイ tako-vd）を起こし、CLI `tako run` / `--list` と MCP
# `tako_run_resolve` で次を確かめる:
#
#   ① sketch.yaml のあるスケッチ → ProjectDefault の `<arduino-cli> compile`（cwd = スケッチ）と upload
#   ② 実際に compile まで通る（exit 0・`Sketch uses`）
#   ③ 基板が未選択 → 実行ペインに選び方の案内 + arduino-cli の `Missing FQBN`
#   ④ フォルダ名と .ino 名の不一致 → 改名の案内 + `main file missing`
#   ⑤ arduino-cli が見つからない（実行環境の層を外した GUI = IDE の無い PC と同じ解決）→ 導入の案内
#   ⑥ A/B: `TAKO_1950_LEGACY=1` では `.ino` が「実行コマンドが見つからない」へ戻る
#
# **ユーザーの Arduino の設定・作業フォルダには書かない**: HOME を一時 dir へ差し替え
# （arduino-cli の既定の data dir `~/Library/Arduino15`・キャッシュ `~/Library/Caches/arduino` も
# 一時 dir になる）、コアは本物の data dir から APFS の複製（`cp -c`）で一時 dir へ写して
# `ARDUINO_CONFIG_FILE` で指す。本物の `inventory.yaml` / `package_index.json` の mtime を前後で比べる。
#
# 書き込み（upload）は基板とポートの実機が要るので、ここでは解決（--list）だけを見る。
#
# 使い方: bash scripts/test-runner-arduino-1950.sh
#   arduino-cli（PATH か Arduino IDE 同梱）と arduino:avr のコアが無い機では、その段を「未実測」と出す

set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
SKIP=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
skip() { SKIP=$((SKIP + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_has() { # check_has <名前> <探す語> <本文>
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'$2' が無い）" ;;
  esac
}

command -v python3 >/dev/null 2>&1 || { echo "python3 が無いので走らせられない（未実測）"; exit 1; }

# 本物の HOME の値は差し替える前に控える（コアの複製元と、触っていないことの比較に使う）
REAL_ARDUINO15="${TAKO_1950_ARDUINO15:-$HOME/Library/Arduino15}"
IDE_CLI="/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli"
REAL_CLI="$(command -v arduino-cli 2>/dev/null || true)"
[ -n "$REAL_CLI" ] || { [ -x "$IDE_CLI" ] && REAL_CLI="$IDE_CLI"; }

stamp() { # 本物の Arduino の data dir の印（無ければ空）
  local f
  for f in inventory.yaml package_index.json library_index.json; do
    [ -f "$REAL_ARDUINO15/$f" ] && stat -f '%m %N' "$REAL_ARDUINO15/$f"
  done
  true
}
STAMP_BEFORE="$(stamp)"

TMP="$(mktemp -d /tmp/tako-1950-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1950-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

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
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"

# arduino-cli の置き場は一時 dir の中だけ（config / data / user / downloads / build cache）
ARD="$TMP/arduino"
mkdir -p "$ARD/data/packages/arduino/hardware" "$ARD/data/packages/arduino/tools" \
  "$ARD/user" "$ARD/staging" "$ARD/cache"
HAVE_CORE=0
if [ -d "$REAL_ARDUINO15/packages/arduino/hardware/avr" ]; then
  cp -c -R "$REAL_ARDUINO15/packages/arduino/hardware/avr" "$ARD/data/packages/arduino/hardware/" 2>/dev/null \
    || cp -R "$REAL_ARDUINO15/packages/arduino/hardware/avr" "$ARD/data/packages/arduino/hardware/"
  for t in avr-gcc avrdude arduinoOTA; do
    [ -d "$REAL_ARDUINO15/packages/arduino/tools/$t" ] || continue
    cp -c -R "$REAL_ARDUINO15/packages/arduino/tools/$t" "$ARD/data/packages/arduino/tools/" 2>/dev/null \
      || cp -R "$REAL_ARDUINO15/packages/arduino/tools/$t" "$ARD/data/packages/arduino/tools/"
  done
  [ -d "$REAL_ARDUINO15/packages/builtin" ] && \
    { cp -c -R "$REAL_ARDUINO15/packages/builtin" "$ARD/data/packages/" 2>/dev/null \
      || cp -R "$REAL_ARDUINO15/packages/builtin" "$ARD/data/packages/"; }
  [ -f "$REAL_ARDUINO15/package_index.json" ] && cp "$REAL_ARDUINO15/package_index.json" "$ARD/data/"
  HAVE_CORE=1
fi
cat > "$ARD/arduino-cli.yaml" <<EOF
directories:
  data: $ARD/data
  user: $ARD/user
  downloads: $ARD/staging
build_cache:
  path: $ARD/cache
EOF
export ARDUINO_CONFIG_FILE="$ARD/arduino-cli.yaml"

for d in "$HOME" "$TAKO_DATA_DIR" "$ARD"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# スケッチ（根に .git を置いて探索をその中へ閉じ込める）
FIX="$TMP/sketches"
mkdir -p "$FIX/.git"
BLINK='void setup() { pinMode(LED_BUILTIN, OUTPUT); }
void loop() { digitalWrite(LED_BUILTIN, HIGH); delay(500); digitalWrite(LED_BUILTIN, LOW); delay(500); }
'
mk_sketch() { # mk_sketch <フォルダ> <ファイル名> [sketch.yaml の中身]
  mkdir -p "$FIX/$1"
  printf '%s' "$BLINK" > "$FIX/$1/$2"
  [ $# -ge 3 ] && printf '%s\n' "$3" > "$FIX/$1/sketch.yaml"
  true
}
mk_sketch Blink Blink.ino "default_fqbn: arduino:avr:uno"
mk_sketch NoBoard NoBoard.ino
mk_sketch Blink2 Blink.ino "default_fqbn: arduino:avr:uno"

PY_LIST='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
ps = d.get("profiles") or []
def prof(name):
    return next((p for p in ps if p.get("profile") == name), {})
print("source=%s" % prof("default").get("source"))
print("command=%s" % prof("default").get("command"))
print("cwd=%s" % prof("default").get("cwd"))
print("upload=%s" % prof("upload").get("command"))
print("kind=%s" % (d.get("project") or {}).get("kind"))
print("manager=%s" % (d.get("runtime") or {}).get("manager"))
print("notes=%s" % " ".join(n.get("id", "") for n in ((d.get("project") or {}).get("notes") or [])))'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }

start_gui() { # start_gui <ログ> [VAR=VAL…]
  local log="$1"; shift
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || return 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; return 1; }
  echo "  pid=$APP_PID root pane=$ROOT"
}

stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # 次の GUI はペイン ID を 1 から振り直すので、前の GUI のペインのログを読み違えないよう空にする
  rm -rf "$TAKO_PANE_LOG_DIR"
}

list() { "$TAKO_BIN" run "$1" --list --pane "$ROOT" 2>&1; }

# run_and_read <file> → 実行ペインの画面（stdout）。終了の状態は "$TMP/status" へ
run_and_read() {
  local out pane st status=""
  out="$("$TAKO_BIN" run "$1" --pane "$ROOT" --new-pane --ratio 0.8 2>&1)"
  pane="$(printf '%s' "$out" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
  if [ -z "$pane" ]; then
    echo "status=__NO_PANE__" > "$TMP/status"
    printf '%s\n' "$out"
    return
  fi
  for _ in $(seq 1 120); do
    st="$("$TAKO_BIN" run-interactive-status "$pane" 2>&1)"
    status="$(printf '%s' "$st" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin); print("%s %s" % (d.get("status"), d.get("exit_code")))
except Exception:
    pass' 2>/dev/null)"
    case "$status" in exited*) break ;; esac
    sleep 1
  done
  echo "status=$status" > "$TMP/status"
  # 画面（`tako read`）は見えている行だけなので、流れた行まで読めるペインの平文ログを読む。
  # ログが無ければ画面へ落とす
  local log
  log="$("$TAKO_BIN" logs show "$pane" --lines 200 2>/dev/null)"
  if [ -n "$log" ]; then
    printf '%s\n' "$log"
  else
    "$TAKO_BIN" read --pane "$pane" 2>/dev/null
  fi
  "$TAKO_BIN" close --pane "$pane" >/dev/null 2>&1 || true
}

echo "== 前提 =="
if [ -n "$REAL_CLI" ]; then
  echo "  arduino-cli: ${REAL_CLI}（$("$REAL_CLI" version 2>/dev/null | head -1)）"
else
  echo "  arduino-cli: 見つからない（compile の段は未実測になる）"
fi
echo "  arduino:avr のコア: $([ "$HAVE_CORE" = 1 ] && echo 一時 dir へ複製した || echo 無い)"

echo
echo "== 隔離 GUI（通常）=="
start_gui "$TMP/app.log" || exit 1

echo
echo "== ① sketch.yaml のあるスケッチ: --list =="
L1="$(list "$FIX/Blink/Blink.ino" | python3 -c "$PY_LIST")"
printf '%s\n' "$L1" | sed 's/^/    /'
check_eq "① source は project_default" "project_default" "$(field source "$L1")"
check_eq "① 種別は arduino" "arduino" "$(field kind "$L1")"
check_eq "① cwd はスケッチのフォルダ" "$FIX/Blink" "$(field cwd "$L1")"
case "$(field command "$L1")" in
  *arduino-cli*" compile") pass "① コマンドは <arduino-cli> compile" ;;
  *) fail "① コマンドが <arduino-cli> compile でない: $(field command "$L1")" ;;
esac
case "$(field upload "$L1")" in
  *arduino-cli*" compile --upload") pass "① upload は <arduino-cli> compile --upload" ;;
  *) fail "① upload が違う: $(field upload "$L1")" ;;
esac
if [ -x "$IDE_CLI" ] && ! command -v arduino-cli >/dev/null 2>&1; then
  check_eq "① PATH に無く IDE 同梱だけ → arduino-ide を選ぶ" "arduino-ide" "$(field manager "$L1")"
  check_has "① コマンドは IDE 同梱のフルパス" "Arduino IDE.app" "$(field command "$L1")"
fi

echo
echo "== ① MCP tako_run_resolve も同じ解決 =="
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1950","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run_resolve","arguments":{"path":"$FIX/Blink/Blink.ino","pane":$ROOT}}}
JSONL
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
  "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
M1="$(python3 -c 'import json,sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if msg.get("id") != 2:
        continue
    for c in (msg.get("result") or {}).get("content") or []:
        if c.get("type") == "text":
            print(c["text"])
' "$TMP/mcp-out.jsonl" | python3 -c "$PY_LIST")"
check_eq "① MCP の command が CLI と同じ" "$(field command "$L1")" "$(field command "$M1")"
check_eq "① MCP の source" "project_default" "$(field source "$M1")"

echo
echo "== ② 実際に compile まで通る =="
if [ -z "$REAL_CLI" ] || [ "$HAVE_CORE" != 1 ]; then
  skip "② arduino-cli か arduino:avr のコアが無い"
else
  S2="$(run_and_read "$FIX/Blink/Blink.ino")"
  printf '%s\n' "$S2" | grep -v '^[[:space:]]*$' | tail -6 | sed 's/^/    | /'
  check_eq "② 終了コード 0" "status=exited 0" "$(cat "$TMP/status")"
  check_has "② compile の結果が出る" "Sketch uses" "$S2"
fi

echo
echo "== ③ 基板が未選択 =="
L3="$(list "$FIX/NoBoard/NoBoard.ino" | python3 -c "$PY_LIST")"
check_has "③ --list の notes に board_not_selected" "board_not_selected" "$(field notes "$L3")"
if [ -z "$REAL_CLI" ]; then
  skip "③ 実行ペインの表示（arduino-cli が無い）"
else
  S3="$(run_and_read "$FIX/NoBoard/NoBoard.ino")"
  printf '%s\n' "$S3" | grep -v '^[[:space:]]*$' | head -8 | sed 's/^/    | /'
  check_has "③ 案内の行が出る" "[tako] " "$S3"
  check_has "③ 選ぶコマンドが出る" "board attach -b" "$S3"
  check_has "③ arduino-cli 自身の理由も残る" "Missing FQBN" "$S3"
  check_eq "③ 終了コードは arduino-cli のまま（1）" "status=exited 1" "$(cat "$TMP/status")"
fi

echo
echo "== ④ フォルダ名と .ino 名の不一致 =="
L4="$(list "$FIX/Blink2/Blink.ino" | python3 -c "$PY_LIST")"
check_has "④ --list の notes に sketch_name_mismatch" "sketch_name_mismatch" "$(field notes "$L4")"
if [ -z "$REAL_CLI" ]; then
  skip "④ 実行ペインの表示（arduino-cli が無い）"
else
  S4="$(run_and_read "$FIX/Blink2/Blink.ino")"
  printf '%s\n' "$S4" | grep -v '^[[:space:]]*$' | head -6 | sed 's/^/    | /'
  check_has "④ 改名の案内が出る" "Blink2.ino" "$S4"
  check_has "④ arduino-cli 自身の理由も残る" "main file missing" "$S4"
fi
stop_gui

echo
echo "== ⑤ arduino-cli が見つからない（実行環境の層を外した GUI = IDE の無い PC と同じ解決）=="
if command -v arduino-cli >/dev/null 2>&1; then
  skip "⑤ この機は PATH に arduino-cli がある（実行ペインのシェルで見つかるので案内は出ない形）"
else
  start_gui "$TMP/app-1730legacy.log" TAKO_1730_LEGACY=1 || exit 1
  L5="$(list "$FIX/Blink/Blink.ino" | python3 -c "$PY_LIST")"
  check_eq "⑤ コマンドは名前のまま" "arduino-cli compile" "$(field command "$L5")"
  check_has "⑤ --list の notes に cli_not_found" "cli_not_found" "$(field notes "$L5")"
  S5="$(run_and_read "$FIX/Blink/Blink.ino")"
  printf '%s\n' "$S5" | grep -v '^[[:space:]]*$' | head -6 | sed 's/^/    | /'
  check_has "⑤ 導入の案内が出る" "arduino-ide" "$S5"
  check_eq "⑤ 終了コードは command not found（127）" "status=exited 127" "$(cat "$TMP/status")"
  stop_gui
fi

echo
echo "== ⑥ A/B: TAKO_1950_LEGACY=1 では .ino が解決できない（#1950 以前）=="
start_gui "$TMP/app-1950legacy.log" TAKO_1950_LEGACY=1 || exit 1
L6="$(list "$FIX/Blink/Blink.ino")"
printf '%s\n' "$L6" | head -2 | sed 's/^/    | /'
# 理由の文は表示言語に従う（#2365）ので英語表示の文も受ける
case "$L6" in
  *"実行コマンドが見つからない"*|*"run command not found"*) pass "⑥ 旧挙動は「実行コマンドが見つからない」" ;;
  *) fail "⑥ 旧挙動は「実行コマンドが見つからない」（'実行コマンドが見つからない' が無い）" ;;
esac
stop_gui

echo
echo "== 本物の Arduino の data dir に触れていない =="
STAMP_AFTER="$(stamp)"
check_eq "inventory.yaml / package_index.json / library_index.json の mtime が前後で同じ" "$STAMP_BEFORE" "$STAMP_AFTER"

echo
echo "== 結果: OK $PASS / NG $FAIL / 未実測 $SKIP =="
[ "$FAIL" -eq 0 ]
