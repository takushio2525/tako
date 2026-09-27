#!/bin/bash
# test-theme-reload-1763.sh — 実行中のテーマの読み直しでも、読めない色を persist.log へ残す実経路テスト（Issue #1763）
#
# 何を確かめるか:
#   #1756 で、settings.json に読めない色（`#赤色` 等）が入っていても起動は落ちず、その色だけ
#   既定へ落として理由を persist.log に残すようにした。ただし残すのは**起動時だけ**で、
#   実行中の読み直し（`reload_theme`。`tako theme` / MCP `tako_theme` / 設定画面 / タブバーの
#   トグルが呼ぶ）は警告を捨てていた。手で直した値がなぜ効かないのかを追えない。
#
#   ① 起動時の記録は従来どおり（`テーマの色上書きを無視: <キー>: <理由> [pid N]`）
#   ② 実行中に settings.json へ読めない色を手で足し、CLI（`tako theme dark`）で読み直すと、
#      その色が起動時と同じ形式で残る。起動時に記録済みの色は積み直さない
#   ③ 同じ内容のまま何度読み直しても行は増えない
#   ④ MCP（`tako mcp serve` 経由の tools/call `tako_theme`）の読み直しでも同じ
#   ⑤ 手で直して読み直すと「解消」が残り、直した色が効く
#   ⑦ `tako theme toggle` でも同じ（ライトでは dark の上書きを読まないので行にならず、
#      ダークへ戻すと起動時と同じ形式で残る）
#   ⑥ settings.json 自体が壊れた JSON でも落ちず、読み直しを重ねても行は積もらない
#      （dispatch が読み直しの前に既定値で保存し直すので、増えるのは「解消」1 行だけ）
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-theme-reload-1763.sh`
# で渡すと ② ④ ⑤ ⑦ ⑥ が NG になる（読み直しの経路が 1 行も残さない = #1763 の症状）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-theme-reload-1763.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1763-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1763-$$"
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
SETTINGS="$TAKO_DATA_DIR/settings.json"
PANIC_LOG="$TAKO_DATA_DIR/panic.log"
PERSIST_LOG="$TAKO_DATA_DIR/persist.log"
IGNORED="テーマの色上書きを無視: "
RESOLVED="テーマの色上書きの警告が解消"

gui_alive() {
  [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null && "$TAKO_BIN" list >/dev/null 2>&1
}
check_alive() {
  sleep 1
  if gui_alive; then pass "$1: GUI が生きている"; else fail "$1: GUI が落ちた"; fi
}
# persist.log のうち、指定の断片を含む行の数
count_lines() {
  local n
  n=$(grep -c -F -- "$1" "$PERSIST_LOG" 2>/dev/null)
  echo "${n:-0}"
}
# テーマの色上書きに関する行（起動時・読み直しの両方）をまとめて出す
show_theme_lines() {
  grep -F "テーマの色上書き" "$PERSIST_LOG" 2>/dev/null | sed 's/^/      /'
}
# settings.json の dark の色上書きを 1 つ書き換える（値が空なら消す）。
# **ユーザーが手で直す**のと同じく、GUI を通さずにファイルだけを書き換える
set_color() {
  python3 - "$SETTINGS" "$1" "$2" <<'PY'
import json, os, sys
p, key, value = sys.argv[1], sys.argv[2], sys.argv[3]
d = {}
if os.path.exists(p):
    try:
        d = json.load(open(p, encoding="utf-8"))
    except Exception:
        d = {}
d.setdefault("theme", "dark")
colors = d.setdefault("theme_colors", {}).setdefault("dark", {})
if value:
    colors[key] = value
else:
    colors.pop(key, None)
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
}
# `tako theme colors` の応答から色の現在値を読む
current_hex() {
  "$TAKO_BIN" theme colors 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print(((d.get("colors") or {}).get(sys.argv[1]) or {}).get("hex", "__NONE__"))' "$1"
}
# 行が起動時と同じ形式か（`テーマの色上書きを無視: <キー>: <理由> [pid <GUI の pid>]`）
check_format() {
  local label="$1" key="$2" line
  line=$(grep -F -- "${IGNORED}${key}: " "$PERSIST_LOG" 2>/dev/null | tail -1)
  case "$line" in
    "["*"] ${IGNORED}${key}: 不正な色値: "*"#RRGGBB"*" [pid ${APP_PID}]") pass "$label: 起動時と同じ形式で残る" ;;
    *) fail "$label: 形式が起動時と違う: ${line:-（行が無い）}" ;;
  esac
}
# MCP の tools/call を 1 本撃ち、isError と本文を返す
mcp_call() {
  local args="$1"
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1763","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_theme","arguments":${args}}}
JSONL
  # `mcp serve` はツール公開の判定を env だけで行う（FR-2.3.2）ので接続情報ファイルから渡す。
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  local sock token
  sock="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  token="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  if [ -z "$sock" ] || [ -z "$token" ]; then
    echo "is_error=__NO_DISCOVERY__"
    return
  fi
  env TAKO_SOCKET="$sock" TAKO_TOKEN="$token" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  python3 - "$TMP/mcp-out.jsonl" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    res = msg.get("result") or {}
    print("is_error=%s" % bool(res.get("isError") or msg.get("error")))
    sys.exit(0)
print("is_error=__NO_RESPONSE__")
PY
}

APP_LOG=""
start_gui() {
  APP_LOG="$1"
  # 面を用意できないときは起動せずに 4 が返る（#1744）。関数の中でも exit で止めて「未実測」にする
  launch_isolated_gui "$1" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$1"
}

echo "== ① 起動時の記録（settings.json に #赤色 が保存済み） =="
set_color accent '#赤色'
set_color green '#00ff00'
if ! start_gui "$TMP/app.log"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"
check_eq "① accent の行が起動時に 1 行" "1" "$(count_lines "${IGNORED}accent: ")"
check_format "①" accent
check_eq "① 正しい上書き（green）は行にならない" "0" "$(count_lines "${IGNORED}green: ")"
check_eq "① green は効く" "#00ff00" "$(current_hex green)"

echo "== ② 実行中に red へ読めない色を手で足し、CLI で読み直す =="
set_color red '#12345'
OUT="$("$TAKO_BIN" theme dark 2>&1)"
check_eq "② tako theme dark が成功で返る" "0" "$?"
check_eq "② red の行が 1 行残る（修正前は 0 行）" "1" "$(count_lines "${IGNORED}red: ")"
check_format "②" red
check_eq "② 起動時に記録済みの accent は積み直さない" "1" "$(count_lines "${IGNORED}accent: ")"
check_alive "②"

echo "== ③ 同じ内容のまま 3 回読み直す =="
BEFORE="$(count_lines "テーマの色上書き")"
for _ in 1 2 3; do "$TAKO_BIN" theme dark >/dev/null 2>&1; done
check_eq "③ テーマの色上書きの行が増えない" "$BEFORE" "$(count_lines "テーマの色上書き")"

echo "== ④ MCP の tako_theme で読み直す（blue に全角の値を手で足す） =="
set_color blue '#ｇ00000'
MCP="$(mcp_call '{"action":"set","mode":"dark"}')"
echo "  応答: $MCP"
check_eq "④ MCP が成功で返る" "is_error=False" "$MCP"
check_eq "④ blue の行が 1 行残る（修正前は 0 行）" "1" "$(count_lines "${IGNORED}blue: ")"
check_format "④" blue
check_eq "④ 既に記録済みの red は積み直さない" "1" "$(count_lines "${IGNORED}red: ")"

echo "== ⑤ 手で直して読み直す =="
set_color accent '#ff8800'
set_color red '#ff0000'
set_color blue ''
"$TAKO_BIN" theme dark >/dev/null 2>&1
check_eq "⑤ 解消の行が 3 行（accent / red / blue）" "3" "$(count_lines "$RESOLVED")"
for k in accent red blue; do
  check_eq "⑤ ${k} の解消が名指しで残る" "1" "$(count_lines "${RESOLVED}（読み直した設定では出ない）: ${k}: ")"
done
check_eq "⑤ 直した accent が効く" "#ff8800" "$(current_hex accent)"
check_eq "⑤ 直した red が効く" "#ff0000" "$(current_hex red)"
BEFORE="$(count_lines "テーマの色上書き")"
"$TAKO_BIN" theme dark >/dev/null 2>&1
check_eq "⑤ 直ったまま読み直しても行は増えない" "$BEFORE" "$(count_lines "テーマの色上書き")"

echo "== ⑦ tako theme toggle で切り替える（dark の上書きはライトでは読まない） =="
set_color accent '#赤色'
BEFORE="$(count_lines "テーマの色上書き")"
"$TAKO_BIN" theme toggle >/dev/null 2>&1
check_eq "⑦ ライトへ切り替えた: 読まれない dark の上書きは行にならない" "$BEFORE" "$(count_lines "テーマの色上書き")"
"$TAKO_BIN" theme toggle >/dev/null 2>&1
check_eq "⑦ ダークへ戻した: accent が起動時と同じ形式で残る（起動時と合わせて 2 行）" "2" "$(count_lines "${IGNORED}accent: ")"
check_format "⑦" accent

echo "== ⑥ settings.json 自体が壊れた JSON のとき =="
printf '{ "theme": "dark", "theme_colors": { "dark": { "accent": "#ff0000" ' > "$SETTINGS"
BEFORE="$(count_lines "テーマの色上書き")"
BEFORE_RESOLVED="$(count_lines "${RESOLVED}（読み直した設定では出ない）: accent: ")"
OUT="$("$TAKO_BIN" theme dark 2>&1)"
echo "  応答: rc=$? $(printf '%s' "$OUT" | tr '\n' ' ' | cut -c1-120)"
check_alive "⑥"
AFTER="$(count_lines "テーマの色上書き")"
# dispatch は読み直しの前に settings.json を既定値で保存し直す（壊れた中身は #916 の退避へ）。
# 読み直した設定には上書きが無いので、増えるのは記録済みの accent の「解消」1 行だけ
check_eq "⑥ 壊れた JSON で読み直して増えた行は 1 行" "1" "$((AFTER - BEFORE))"
check_eq "⑥ その 1 行は accent の解消" "$((BEFORE_RESOLVED + 1))" \
  "$(count_lines "${RESOLVED}（読み直した設定では出ない）: accent: ")"
grep -F "settings.json を解釈できない" "$PERSIST_LOG" 2>/dev/null | sed 's/^/      /'
if [ -f "$TAKO_DATA_DIR/settings.json.unreadable.bak" ]; then
  pass "⑥ 壊れた中身は退避されている（#916）"
else
  fail "⑥ 壊れた中身の退避が無い"
fi
for _ in 1 2 3; do "$TAKO_BIN" theme dark >/dev/null 2>&1; done
check_eq "⑥ 続けて読み直しても行は積もらない" "$AFTER" "$(count_lines "テーマの色上書き")"

echo "== 残った行（テーマの色上書き） =="
show_theme_lines

if [ -f "$PANIC_LOG" ]; then
  fail "panic.log が作られた（どこかで panic した）"
else
  pass "panic.log が作られていない"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
