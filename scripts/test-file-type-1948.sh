#!/usr/bin/env bash
# test-file-type-1948.sh — 言語の同定の正本（`tako_core::file_type`）の実経路テスト（#1948 / FR-3.2）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#
#   ① CLI `tako open` の結果に `file_type`（形式の ID）と `syntax`（塗った構文名）が載る
#      （`.ino` = arduino / C++・大文字の `.INO`・ファイル名で決まる `CMakeLists.txt`・
#      表に無い拡張子・拡張子なしで 1 行目が shebang・`.jsx`・画像・Markdown の 2 表示）
#   ② MCP `tako_open_file` が**同じ dispatch** を通り、応答が CLI と**字面まで一致**する
#      （ツールは増やしていない）
#   ③ `tako edit` の応答（読み取り表示の状態取得・編集開始）にも同じ 2 つが載り、
#      MCP `tako_preview_edit` の状態取得と字面まで一致する
#   ④ A/B: `TAKO_1949_LEGACY=1` では `.ino` だけが Plain Text へ戻る（#1949 の逃げ道が
#      表へ移っても効く）。形式の ID は変わらない
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-file-type-1948.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1948-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1948-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
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
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 材料 ---------------------------------------------------------------------
# macOS の既定のファイルシステムは大文字小文字を区別しないので、大文字の `.INO` は別の dir に置く
FIX="$TMP/fixtures"
mkdir -p "$FIX/upper"
python3 - "$FIX" <<'PY'
import pathlib, sys
fix = pathlib.Path(sys.argv[1])
sketch = "void setup() {\n  pinMode(13, OUTPUT);\n}\n\nvoid loop() {\n}\n"
(fix / "sketch.ino").write_text(sketch, encoding="utf-8")
(fix / "upper" / "Sketch.INO").write_text(sketch, encoding="utf-8")
(fix / "CMakeLists.txt").write_text("cmake_minimum_required(VERSION 3.10)\nproject(demo)\n", encoding="utf-8")
(fix / "notes.unknownext").write_text("hello\n", encoding="utf-8")
(fix / "runme").write_text("#!/usr/bin/env python3\nprint(1)\n", encoding="utf-8")
(fix / "App.jsx").write_text("export const App = () => <div />;\n", encoding="utf-8")
(fix / "doc.md").write_text("# 見出し\n\n本文\n", encoding="utf-8")
(fix / "shot.png").write_bytes(bytes([0x89]) + b"PNG\r\n\x1a\n")
PY

# 応答 JSON から見たい項目だけを 1 行で出す
PY_FT='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__ %s" % e); sys.exit(0)
print("file_type=%s syntax=%s mode=%s" % (d.get("file_type", "__NO_KEY__"), d.get("syntax", "__NO_KEY__"), d.get("mode")))'
NORM='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True, ensure_ascii=False))'

# MCP を 1 回呼んで、id=2 の応答の本文を出す（`mcp_call <ツール名> <arguments の JSON>`）
mcp_call() {
  local socket token
  socket="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  token="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1948","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    > "$TMP/mcp-in.jsonl"
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  env TAKO_SOCKET="$socket" TAKO_TOKEN="$token" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  python3 -c 'import json,sys
for line in open(sys.argv[1]):
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    content = (msg.get("result") or {}).get("content") or []
    print(content[0].get("text") if content else "__NO_CONTENT__")
    sys.exit(0)
print("__NO_RESPONSE__")' "$TMP/mcp-out.jsonl"
}

# GUI を起こし、差し替え先の専用プレビューペインを 1 枚生やす（`start_gui <ログ> [VAR=VAL…]`）
start_gui() {
  local log="$1"; shift
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
  PREVIEW="$("$TAKO_BIN" open "$FIX/notes.unknownext" --pane "$ROOT" --right 2>&1 | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
  [ -n "$PREVIEW" ] || { echo "検証用プレビューペインを作れない"; exit 1; }
  echo "  pid=$APP_PID preview pane=$PREVIEW"
}

open_ft() { # open_ft <file> [引数…] → "file_type=… syntax=… mode=…"
  local file="$1"; shift
  "$TAKO_BIN" open "$file" --pane "$PREVIEW" ${1+"$@"} 2>&1 | python3 -c "$PY_FT"
}

# 読み取り表示の塗りが戻るまで待つ（`tako edit status` から `highlighting` が消える = #1916）
wait_painted() {
  local i
  for i in $(seq 1 100); do
    if ! "$TAKO_BIN" edit status --pane "$PREVIEW" 2>/dev/null | grep -q '"highlighting"'; then
      return 0
    fi
    sleep 0.2
  done
  return 1
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log"

echo
echo "== ① CLI tako open の結果に file_type / syntax が載る =="
expect_open() { # expect_open <説明> <期待> <file> [引数…]
  local label="$1" want="$2" file="$3"; shift 3
  local got
  got="$(open_ft "$file" ${1+"$@"})"
  check_eq "$label" "$want" "$got"
}
expect_open ".ino は arduino / C++" "file_type=arduino syntax=C++ mode=code" "$FIX/sketch.ino"
expect_open "大文字の .INO も同じ" "file_type=arduino syntax=C++ mode=code" "$FIX/upper/Sketch.INO"
expect_open "CMakeLists.txt はファイル名で cmake（色は two-face の CMake）" "file_type=cmake syntax=CMake mode=code" "$FIX/CMakeLists.txt"
expect_open "表に無い拡張子は file_type が null・平文" "file_type=None syntax=Plain Text mode=code" "$FIX/notes.unknownext"
expect_open "拡張子なしは 1 行目（shebang）で Python" "file_type=None syntax=Python mode=code" "$FIX/runme"
expect_open ".jsx は javascriptreact / JavaScript" "file_type=javascriptreact syntax=JavaScript mode=code" "$FIX/App.jsx"
expect_open "画像はコード表示でないので syntax が null" "file_type=png syntax=None mode=image" "$FIX/shot.png"
expect_open "Markdown のレンダリング表示は syntax が null" "file_type=markdown syntax=None mode=markdown" "$FIX/doc.md"
expect_open "Markdown をコード表示にすると Markdown で塗る" "file_type=markdown syntax=Markdown mode=code" "$FIX/doc.md" --mode code

echo
echo "== ② MCP tako_open_file が CLI と字面まで一致する =="
MCP_TEXT="$(mcp_call tako_open_file "{\"path\":\"$FIX/sketch.ino\",\"pane\":$PREVIEW}")"
CLI_TEXT="$("$TAKO_BIN" open "$FIX/sketch.ino" --pane "$PREVIEW" 2>&1)"
CLI_NORM="$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM" 2>/dev/null)"
MCP_NORM="$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM" 2>/dev/null)"
echo "  CLI: $CLI_NORM"
echo "  MCP: $MCP_NORM"
check_eq "MCP の応答にも file_type / syntax が載る" "file_type=arduino syntax=C++ mode=code" "$(printf '%s' "$MCP_TEXT" | python3 -c "$PY_FT")"
check_eq "CLI と MCP の応答が字面まで一致する" "$CLI_NORM" "$MCP_NORM"

echo
echo "== ③ tako edit の応答にも載る =="
if wait_painted; then pass "読み取り表示の塗りが戻った"; else fail "塗りが 20 秒で戻らない"; fi
STATUS="$("$TAKO_BIN" edit status --pane "$PREVIEW" 2>&1)"
check_eq "状態取得（読み取り表示）" "file_type=arduino syntax=C++ mode=None" "$(printf '%s' "$STATUS" | python3 -c "$PY_FT")"
MCP_STATUS="$(mcp_call tako_preview_edit "{\"pane\":$PREVIEW}")"
check_eq "MCP tako_preview_edit の状態取得と字面まで一致する" \
  "$(printf '%s' "$STATUS" | python3 -c "$NORM" 2>/dev/null)" \
  "$(printf '%s' "$MCP_STATUS" | python3 -c "$NORM" 2>/dev/null)"
STARTED="$("$TAKO_BIN" edit start --pane "$PREVIEW" 2>&1)"
check_eq "編集開始の応答（編集の塗り）" "file_type=arduino syntax=C++ mode=None" "$(printf '%s' "$STARTED" | python3 -c "$PY_FT")"
"$TAKO_BIN" edit stop --pane "$PREVIEW" >/dev/null 2>&1 || true

echo
echo "== ④ A/B: TAKO_1949_LEGACY=1 では .ino だけが Plain Text へ戻る =="
stop_isolated_gui "$APP_PID"
APP_PID=""
start_gui "$TMP/app-legacy.log" TAKO_1949_LEGACY=1
expect_open "旧挙動: .ino は Plain Text（ID は arduino のまま）" "file_type=arduino syntax=Plain Text mode=code" "$FIX/sketch.ino"
expect_open "旧挙動でも .jsx は変わらない" "file_type=javascriptreact syntax=JavaScript mode=code" "$FIX/App.jsx"

echo
echo "== 結果: PASS=$PASS FAIL=$FAIL =="
[ "$FAIL" -eq 0 ]
