#!/bin/bash
# tako:run bash scripts/test-run-board-1952.sh
# test-run-board-1952.sh — `.ino` の基板とポートの選択（Issue #1952）を、この PC の Arduino IDE の同梱物
# （arduino-cli・arduino-language-server・clangd）と一時 HOME に複製した arduino:avr のコアで確かめる（隔離 GUI）
#
# 何を確かめるか（Issue #1952 の受け入れ条件）:
#   ① ドロップダウン（visual-test `run-board-1952` = 実マウス・実キー）・CLI（`tako run-config --board --port`）・
#      MCP（`tako_run_defaults` の board / port）のどれで選んでも、arduino-cli が直に書いた sketch.yaml と
#      **バイト一致**（既にある sketch.yaml のコメント・profiles を保つ形も arduino-cli の直書きと一致）
#   ② 選んだ直後に言語サーバが unconfigured から起き、`tako run --wait` が compile まで通る（基板は繋がない）
#   ③ コアが入っていない基板（arduino:renesas_uno:unor4wifi）は書かず `core install` のコマンドを示すだけ。
#      tako が起こした arduino-cli の引数を全部記録し（PATH の先頭の記録係）、`core install` が 1 度も無い
#   ④ 境界: 形の崩れた FQBN / スケッチでないファイル / 入っているコアに無い基板 / `--boards` の検索
#   ⑤ A/B: `TAKO_1952_LEGACY=1` はドロップダウンに基板の欄が無い（visual-test が ① で名指しの FAILED）・
#      本番の /Applications/tako.app（#1952 より前）の CLI には `--board` が無い
#
# **ユーザーの Arduino の設定・スケッチには書かない**: HOME を一時 dir へ差し替え（arduino-cli の既定の data dir と
# ビルドのキャッシュも一時 dir になる）、arduino:avr のコアは本物の data dir から APFS の複製（`cp -c`）で写す。
# 本物の inventory.yaml / package_index.json / library_index.json の mtime を前後で比べる（#1950 / #1951 の作法）。
#
# 使い方: bash scripts/test-run-board-1952.sh
#   TAKO_1952_DUMP_DIR=<dir>   画像の置き場（新旧で new / legacy に分ける。既定は一時 = 終わると消える）
#   Arduino IDE（/Applications/Arduino IDE.app）か arduino:avr のコアが無い機では全段を「未実測」と出す。
#   面（tako-vd）を用意できない機（蓋閉じ）では GUI の段を「未実測」と出す。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装）。落とすのは自分で起こした pid だけ。
# macOS の bash 3.2 で通す（#1499 / #1518）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_has() { # check_has <名前> <探す語> <本文>
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'$2' が無い: $(printf '%s' "$3" | head -c 300)）" ;;
  esac
}

IDE_DIR="/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources"
REAL_CLI="$IDE_DIR/arduino-cli"
REAL_ARDUINO15="${TAKO_1952_ARDUINO15:-$HOME/Library/Arduino15}"
PORT="/dev/cu.usbmodem-tako"
stamp() { # 本物の Arduino の data dir の印（無ければ空）
  local f
  for f in inventory.yaml package_index.json library_index.json; do
    [ -f "$REAL_ARDUINO15/$f" ] && stat -f '%m %N' "$REAL_ARDUINO15/$f"
  done
  true
}
STAMP_BEFORE="$(stamp)"

TMP="$(mktemp -d /tmp/tk1952.XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
DUMP="${TAKO_1952_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1952-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

echo "visual-test 版の tako-app と tako をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
mask() { sed "s#$TMP#<tmp>#g"; }

# --- 隔離の土台（HOME・コアの複製・記録係） ----------------------------------------------
mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/rbin" "$DUMP"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
printf 'export PATH="%s:$PATH"\n' "$TMP/rbin" > "$TMP/zdot/.zprofile"
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
found="$(command -v tmux 2>/dev/null)"
[ -n "$found" ] && ln -s "$found" "$TMP/rbin/tmux"

A15="$TMP/home/Library/Arduino15"
HAVE_CORE=0
if [ -d "$REAL_ARDUINO15/packages/arduino/hardware/avr" ]; then
  mkdir -p "$A15/packages/arduino/hardware" "$A15/packages/arduino/tools"
  copy() { command cp -c -R "$1" "$2" 2>/dev/null || command cp -R "$1" "$2"; }
  # arduino:avr だけを写す（renesas_uno は写さない = 「コアが入っていない基板」の素材）
  copy "$REAL_ARDUINO15/packages/arduino/hardware/avr" "$A15/packages/arduino/hardware/"
  for t in avr-gcc avrdude arduinoOTA; do
    [ -d "$REAL_ARDUINO15/packages/arduino/tools/$t" ] && copy "$REAL_ARDUINO15/packages/arduino/tools/$t" "$A15/packages/arduino/tools/"
  done
  [ -d "$REAL_ARDUINO15/packages/builtin" ] && copy "$REAL_ARDUINO15/packages/builtin" "$A15/packages/"
  [ -f "$REAL_ARDUINO15/package_index.json" ] && command cp -f "$REAL_ARDUINO15/package_index.json" "$A15/"
  HAVE_CORE=1
fi
HAVE_IDE=0
[ -x "$REAL_CLI" ] && [ -x "$IDE_DIR/arduino-language-server" ] && [ -x "$IDE_DIR/clangd" ] && HAVE_IDE=1

# 記録係: tako が PATH の先頭で見つける arduino-cli。起こされた引数を 1 行ずつ残してから本物へ渡す
ARGV_LOG="$TMP/cli-argv.log"
: > "$ARGV_LOG"
cat > "$TMP/rbin/arduino-cli" <<SH
#!/bin/sh
printf '%s\n' "\$*" >> "$ARGV_LOG"
exec "$REAL_CLI" "\$@"
SH
chmod +x "$TMP/rbin/arduino-cli"

export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

# --- 素材（根に .git = ルートが上へ上がらない） --------------------------------------------
S="$TMP/s"
mkdir -p "$S/.git"
sketch() { # sketch <名前> → <S>/<名前>/<名前>.ino（基板なし）
  mkdir -p "$S/$1"
  cat > "$S/$1/$1.ino" <<'EOF'
void setup() {
  pinMode(LED_BUILTIN, OUTPUT);
}

void loop() {
  digitalWrite(LED_BUILTIN, HIGH);
  delay(500);
  digitalWrite(LED_BUILTIN, LOW);
  delay(500);
}
EOF
}
for n in Ref Vis VisOld Cli Mcp Core Bad Keep KeepRef; do sketch "$n"; done
# 正解: 本物の arduino-cli が直に書いた sketch.yaml（tako を通さない）
"$REAL_CLI" board attach -b arduino:avr:uno -p "$PORT" "$S/Ref" >/dev/null 2>&1
# 既にある sketch.yaml（コメント・profiles）の上書きの正解
KEEP_YAML='# 授業用の設定
profiles:
  nano:
    fqbn: arduino:avr:nano
default_fqbn: arduino:avr:nano   # 古い
'
printf '%s' "$KEEP_YAML" > "$S/Keep/sketch.yaml"
printf '%s' "$KEEP_YAML" > "$S/KeepRef/sketch.yaml"
"$REAL_CLI" board attach -b arduino:avr:mega "$S/KeepRef" >/dev/null 2>&1

start_gui() { # start_gui <data> <ログ> [VAR=VAL…]
  local data="$1" log="$2"
  shift 2
  mkdir -p "$data"
  export TAKO_DATA_DIR="$data"
  launch_isolated_gui "$log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_DATA_DIR="$data" ${1+"$@"}
}
server_field() { # server_field <ルート> <式>
  "$TAKO_BIN" lsp status --name arduino-language-server --json 2>/dev/null |
    python3 -c 'import json,sys
d = json.load(sys.stdin)
s = next((x for x in d.get("servers", []) if x.get("root") == sys.argv[1]), {})
print(eval("s" + sys.argv[2]))' "$1" "$2" 2>/dev/null
}
wait_state() { # wait_state <ルート> <状態の正規表現> <秒>
  local n=0 st
  while [ "$n" -lt $(($3 * 10)) ]; do
    st="$(server_field "$1" '.get("state")')"
    if printf '%s' "$st" | grep -Eq "^($2)\$"; then return 0; fi
    sleep 0.1
    n=$((n + 1))
  done
  return 1
}
same_bytes() { # same_bytes <名前> <ファイル> <正解>
  if [ -f "$2" ] && cmp -s "$2" "$3"; then
    pass "$1（$(wc -c < "$3" | tr -d ' ') バイト）"
  else
    fail "$1（ours=$(od -c "$2" 2>/dev/null | head -3 | tr -s ' ' | tr '\n' ' ') / ref=$(od -c "$3" | head -3 | tr -s ' ' | tr '\n' ' ')）"
  fi
}

echo "== 前提 =="
echo "  Arduino IDE の同梱: $([ "$HAVE_IDE" = 1 ] && "$REAL_CLI" version 2>/dev/null | head -1 || echo 無い)"
echo "  arduino:avr のコア: $([ "$HAVE_CORE" = 1 ] && echo '一時 HOME へ複製した（renesas_uno は写さない）' || echo 無い)"
echo "  正解の sketch.yaml（arduino-cli の直書き）: $(tr '\n' '|' < "$S/Ref/sketch.yaml" 2>/dev/null)"
if [ "$HAVE_IDE" != 1 ] || [ "$HAVE_CORE" != 1 ] || [ ! -f "$S/Ref/sketch.yaml" ]; then
  unmeasured "全段: Arduino IDE の同梱か arduino:avr のコアが無い"
  echo "結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
  exit 0
fi

echo
echo "== ⑤ A/B（CLI）: 本番の /Applications/tako.app には --board が無い =="
PROD_CLI="/Applications/tako.app/Contents/MacOS/tako"
if [ -x "$PROD_CLI" ]; then
  old="$(env -u TAKO_SOCKET TAKO_DISCOVERY_DIR="$TMP/nodisc" "$PROD_CLI" run-config "$S/Bad/Bad.ino" --board arduino:avr:uno 2>&1)"
  echo "    旧: $(printf '%s' "$old" | head -2 | mask | tr '\n' ' ')"
  check_has "旧: --board を知らない（#1952 より前）" "--board" "$old"
  [ ! -f "$S/Bad/sketch.yaml" ] && pass "旧: sketch.yaml を書いていない" || fail "旧: sketch.yaml を書いた"
else
  unmeasured "⑤ CLI: /Applications/tako.app が無い"
fi

echo
echo "== ① visual-test run-board-1952（ドロップダウン・実マウス・実キー） =="
run_visual() { # run_visual <名前> <スケッチ> [VAR=VAL…]
  local name="$1" sketch_dir="$2" log rc
  shift 2
  log="$TMP/visual-$name.log"
  start_gui "$TMP/data-vis-$name" "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=run-board-1952 \
    TAKO_VISUAL_DUMP_DIR="$DUMP/$name" TAKO_1952_SKETCH="$sketch_dir" TAKO_1952_REF="$S/Ref" ${1+"$@"}
  rc=$?
  if [ "$rc" = 4 ]; then
    unmeasured "visual-test（$name）: 面（tako-vd）を用意できない"
    return 4
  fi
  [ "$rc" = 0 ] || { fail "visual-test（$name）: GUI が立たない（rc=$rc）"; return 1; }
  APP_PID="$ISOLATED_GUI_PID"
  local i
  for i in $(seq 1 240); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 1
  done
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  grep -E "TAKO_VISUAL_1952|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK|TAKO_VISUAL_DUMP_FILE|panicked" "$log" | mask | sed 's/^/    /'
  return 0
}
if run_visual new "$S/Vis"; then
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual-new.log"; then
    pass "新: ドロップダウンで選ぶ → sketch.yaml がバイト一致・言語サーバが起きる・コア無しは案内だけ"
  else
    fail "新: visual-test が通らない（$(grep -m1 TAKO_APP_SELF_TEST_FAILED "$TMP/visual-new.log" | mask)）"
  fi
  same_bytes "新: ドロップダウンの sketch.yaml = arduino-cli の直書き" "$S/Vis/sketch.yaml" "$S/Ref/sketch.yaml"
  if run_visual legacy "$S/VisOld" TAKO_1952_LEGACY=1; then
    legacy_fail="$(grep -m1 "TAKO_APP_SELF_TEST_FAILED" "$TMP/visual-legacy.log" | mask)"
    check_has "旧（TAKO_1952_LEGACY=1）: ① で基板の欄が無いと名指しで落ちる" "基板とポートの欄" "$legacy_fail"
    [ ! -f "$S/VisOld/sketch.yaml" ] && pass "旧: sketch.yaml は書かれない" || fail "旧: sketch.yaml が書かれた"
  fi
fi

echo
echo "== ①②③④ CLI / MCP（隔離 GUI の受け口へ。ここは画面を撮らない） =="
start_gui "$TMP/data-cli" "$TMP/cli-gui.log"
grc=$?
if [ "$grc" = 4 ]; then
  unmeasured "CLI / MCP の段: 面（tako-vd）を用意できない（GUI の受け口が要る）"
else
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/cli-gui.log" || { echo "GUI が立たない"; exit 1; }
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"

  # 読み: 基板とポートの実効値と候補（background で board list / listall を聞く）
  R="$("$TAKO_BIN" run-config "$S/Cli/Cli.ino" 2>&1)"
  echo "    読み: $(printf '%s' "$R" | json 'json.dumps({k: d["board"][k] for k in ("fqbn","port","installed_boards","core_installed")})')"
  check_eq "読み: 未選択（fqbn null）" "None" "$(printf '%s' "$R" | json 'd["board"]["fqbn"]')"
  check_eq "読み: 入っているコアの基板の数（arduino:avr = 27）" "27" "$(printf '%s' "$R" | json 'd["board"]["installed_boards"]')"
  check_eq "読み: ポートの一覧を聞いた（配列）" "True" "$(printf '%s' "$R" | json 'isinstance(d["board"]["ports"], list)')"
  L="$("$TAKO_BIN" run "$S/Cli/Cli.ino" --list --boards nano --pane "$ROOT" 2>&1)"
  check_has "④ --boards nano で探せる（tako run --list）" "arduino:avr:nano" "$(printf '%s' "$L" | json 'd["board"]["matches"]')"

  # 言語サーバ: 開いて unconfigured → CLI で選ぶ → 起きる
  pane="$("$TAKO_BIN" open "$S/Cli/Cli.ino" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  if wait_state "$S/Cli" "unconfigured" 20; then
    pass "② 基板が無いので言語サーバは unconfigured"
  else
    fail "② unconfigured にならない（$(server_field "$S/Cli" '.get("state")')）"
  fi
  : > "$ARGV_LOG"
  t0="$(python3 -c 'import time; print(time.time())')"
  W="$("$TAKO_BIN" run-config "$S/Cli/Cli.ino" --board arduino:avr:uno --port "$PORT" 2>&1)"
  t1="$(python3 -c 'import time; print(time.time())')"
  echo "    書き: $(printf '%s' "$W" | json 'json.dumps({"attached": d.get("attached"), "lsp": d.get("lsp")})' | mask)（$(python3 -c "print(round($t1 - $t0, 2))") 秒）"
  echo "    起こした arduino-cli: $(tr '\n' '|' < "$ARGV_LOG" | mask)"
  same_bytes "① CLI の sketch.yaml = arduino-cli の直書き" "$S/Cli/sketch.yaml" "$S/Ref/sketch.yaml"
  if wait_state "$S/Cli" "starting|running" 30; then
    pass "② CLI で選んだ直後に言語サーバが起きる（$(server_field "$S/Cli" '.get("state")')）"
  else
    fail "② 言語サーバが起きない（$(server_field "$S/Cli" '.get("state")') / $(server_field "$S/Cli" '.get("reason")')）"
  fi
  # compile（基板は繋がない。ビルドのキャッシュは一時 HOME の下）
  "$TAKO_BIN" run "$S/Cli/Cli.ino" --pane "$ROOT" --wait > "$TMP/run.log" 2>&1
  rrc=$?
  echo "    tako run --wait: rc=$rrc $(tail -3 "$TMP/run.log" | mask | tr '\n' ' ' | head -c 300)"
  check_eq "② tako run が compile まで通る（終了コード）" "0" "$rrc"

  # MCP
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1952","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run_defaults","arguments":{"path":"$S/Mcp/Mcp.ino","board":"arduino:avr:uno","port":"$PORT"}}}
JSONL
  MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  echo "    MCP: $(python3 -c 'import json,sys
for line in open(sys.argv[1]):
    m = json.loads(line)
    if m.get("id") == 2:
        r = m.get("result") or {}
        print(("isError " if r.get("isError") else "") + (r.get("content") or [{}])[0].get("text", "")[:160].replace("\n", " "))' "$TMP/mcp-out.jsonl" 2>/dev/null | mask)"
  same_bytes "① MCP の sketch.yaml = arduino-cli の直書き" "$S/Mcp/sketch.yaml" "$S/Ref/sketch.yaml"

  # 既にある sketch.yaml（コメント・profiles）を arduino-cli と同じ形で上書きする
  K="$("$TAKO_BIN" run-config "$S/Keep/Keep.ino" --board arduino:avr:mega 2>&1)"
  same_bytes "④ 既にある sketch.yaml の上書きも arduino-cli の直書きと一致（コメント・profiles を保つ）" "$S/Keep/sketch.yaml" "$S/KeepRef/sketch.yaml"
  check_has "④ default_profile が勝つ注意が出る" "default_profile" "$(printf '%s' "$K" | json 'd["board"]["notes"]')"

  # コアが入っていない基板: 書かず、導入のコマンドを示すだけ
  : > "$ARGV_LOG"
  C="$("$TAKO_BIN" run-config "$S/Core/Core.ino" --board arduino:renesas_uno:unor4wifi 2>&1)"
  crc=$?
  echo "    コア無し: rc=$crc $(printf '%s' "$C" | mask | tr '\n' ' ' | head -c 400)"
  echo "    起こした arduino-cli: $(tr '\n' '|' < "$ARGV_LOG" | mask)"
  [ "$crc" != 0 ] && pass "③ コアが無い基板は断る（rc=$crc）" || fail "③ コアが無い基板を受けた"
  check_has "③ 導入のコマンドを示す" "core install arduino:renesas_uno" "$C"
  [ ! -f "$S/Core/sketch.yaml" ] && pass "③ sketch.yaml を書いていない" || fail "③ sketch.yaml を書いた"
  if grep -q "core install" "$ARGV_LOG"; then fail "③ arduino-cli に core install が渡った"; else pass "③ arduino-cli に core install が 1 度も渡っていない（$(wc -l < "$ARGV_LOG" | tr -d ' ') 回の呼び出し）"; fi
  if grep -q "^board attach" "$ARGV_LOG"; then fail "③ board attach を起こした"; else pass "③ board attach も起こしていない"; fi

  # 境界
  B1="$("$TAKO_BIN" run-config "$S/Bad/Bad.ino" --board foo 2>&1)"
  check_has "④ 形の崩れた FQBN は断る" "FQBN" "$B1"
  B2="$("$TAKO_BIN" run-config "$S/Bad/Bad.ino" --board arduino:avr:unoo 2>&1)"
  check_has "④ 入っているコアに無い基板は断る（近い候補）" "arduino:avr:uno" "$B2"
  printf 'print(1)\n' > "$S/a.py"
  B3="$("$TAKO_BIN" run-config "$S/a.py" --board arduino:avr:uno 2>&1)"
  check_has "④ スケッチでないファイルは断る" ".ino" "$B3"
  [ ! -f "$S/Bad/sketch.yaml" ] && pass "④ 断ったスケッチに sketch.yaml が無い" || fail "④ 断ったのに sketch.yaml がある"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
fi

echo
echo "== 本物の Arduino の data dir を変えていない =="
check_eq "inventory.yaml / package_index.json / library_index.json の mtime が前後で同じ" "$STAMP_BEFORE" "$(stamp)"

echo
echo "結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" = 0 ]
