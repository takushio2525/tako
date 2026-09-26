#!/bin/bash
# test-theme-color-1756.sh — 非 ASCII の色の指定で GUI が落ちないことの実経路テスト（Issue #1756）
#
# 何を確かめるか:
#   `parse_hex_color` はバイト長で 6 を確かめてから `&hex[0..2]` で切っていたので、
#   「赤色」（3 バイト × 2 = 6 バイト）のような値が長さの検査を通り、文字の途中で切って
#   panic していた。呼び出し元は GUI プロセスの dispatch（`tako theme color` / MCP
#   `tako_theme` / 設定画面）と起動時のテーマ解決なので、**GUI ごと落ちる**。
#
#   ① 正しい色は従来どおり通る（回帰なし）
#   ② CLI から `#赤色` を渡すと理由付きのエラーが返り、GUI は生きている
#   ③ MCP（`tako mcp serve` 経由の tools/call）でも同じ
#   ④ ほかの不正な値（桁違い・全角・空）でも落ちず、保存済みの色も書き換わらない
#   ⑤ settings.json に `#赤色` が保存済みの状態から起動しても落ちない（既定色で立つ）
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-theme-color-1756.sh`
# で渡すと ② 以降が NG になり、`<data_dir>/panic.log` に「byte index 2 is not a char
# boundary」が残る（#1756 の実測）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-theme-color-1756.sh
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
check_contains() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'${2}' を含まない: ${3}）" ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1756-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1756-$$"
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

# GUI が生きているか（pid が居る + CLI が繋がる の両方）
gui_alive() {
  [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null && "$TAKO_BIN" list >/dev/null 2>&1
}
check_alive() {
  # panic が遅れて届く分を拾うため、少し待ってから見る
  sleep 1
  if gui_alive; then
    pass "$1: GUI が生きている"
  else
    fail "$1: GUI が落ちた"
    show_panic
  fi
}
# 落ちたときの証拠（panic.log と GUI の標準エラーの要点）を出す
show_panic() {
  if [ -f "$PANIC_LOG" ]; then
    echo "    panic.log の要点:"
    grep -E "panicked|char boundary|panic 発生" "$PANIC_LOG" | tail -3 | sed 's/^/      /'
  fi
  if [ -n "$APP_LOG" ] && [ -f "$APP_LOG" ]; then
    echo "    GUI の標準エラーの要点:"
    grep -E "panicked|char boundary|abort" "$APP_LOG" | tail -3 | sed 's/^/      /'
  fi
}
# 保存済みの settings.json から dark の accent 上書きを読む
saved_accent() {
  python3 - "$SETTINGS" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    print("__NO_SETTINGS__"); sys.exit(0)
print(((d.get("theme_colors") or {}).get("dark") or {}).get("accent", "__NONE__"))
PY
}
# `tako theme colors` の応答から accent の現在値を読む
current_accent_hex() {
  "$TAKO_BIN" theme colors 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print(((d.get("colors") or {}).get("accent") or {}).get("hex", "__NONE__"))'
}
APP_LOG=""
start_gui() {
  APP_LOG="$1"
  # 面を用意できないときは起動せずに 4 が返る（#1744）。関数の中でも exit で止めて「未実測」にする
  launch_isolated_gui "$1" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$1"
}

echo "== 隔離 GUI を起こす =="
if ! start_gui "$TMP/app.log"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

echo "== ① 正しい色は従来どおり通る =="
OUT="$("$TAKO_BIN" theme color accent '#ff0000' 2>&1)"
RC=$?
check_eq "① #ff0000 は成功で返る" "0" "$RC"
check_eq "① settings.json に #ff0000 が保存される" "#ff0000" "$(saved_accent)"
check_eq "① theme colors の accent が #ff0000" "#ff0000" "$(current_accent_hex)"
OUT="$("$TAKO_BIN" theme color accent '#89B4FA' 2>&1)"
check_eq "① 大文字の #89B4FA も成功で返る" "0" "$?"
check_eq "① theme colors の accent が #89b4fa（大文字小文字を問わない）" "#89b4fa" "$(current_accent_hex)"
"$TAKO_BIN" theme color accent '#ff0000' >/dev/null 2>&1

echo "== ② CLI から #赤色（6 バイトの非 ASCII）を渡す =="
OUT="$("$TAKO_BIN" theme color accent '#赤色' 2>&1)"
RC=$?
echo "  応答: $(printf '%s' "$OUT" | tr '\n' ' ')"
if [ "$RC" -ne 0 ]; then pass "② 失敗で返る（rc=${RC}）"; else fail "② 成功で返った"; fi
check_contains "② エラーに問題の文字が載る" "赤" "$OUT"
check_contains "② エラーに期待する形式が載る" "#RRGGBB" "$OUT"
check_alive "②"

echo "== ③ MCP tools/call tako_theme set-color #赤色 =="
if gui_alive; then
  cat > "$TMP/mcp-in.jsonl" <<'JSONL'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1756","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_theme","arguments":{"action":"set-color","key":"accent","value":"#赤色"}}}
JSONL
  # `mcp serve` はツール公開の判定を env だけで行う（FR-2.3.2）ので接続情報ファイルから渡す。
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  if [ -z "$MCP_SOCKET" ] || [ -z "$MCP_TOKEN" ]; then
    fail "③ 隔離先の接続情報が空（本番へ繋がないよう撃たずに止める）"
  else
    env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
      "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
    MCP="$(python3 - "$TMP/mcp-out.jsonl" <<'PY'
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
    content = res.get("content") or []
    text = content[0].get("text") if content else ((msg.get("error") or {}).get("message") or "")
    print("is_error=%s" % bool(res.get("isError") or msg.get("error")))
    print("text=%s" % text.replace("\n", " "))
    sys.exit(0)
print("is_error=__NO_RESPONSE__")
PY
)"
    echo "  応答: $(printf '%s' "$MCP" | tr '\n' ' ')"
    check_contains "③ MCP がエラーで返す" "is_error=True" "$MCP"
    check_contains "③ MCP のエラーに問題の文字が載る" "赤" "$MCP"
  fi
else
  fail "③ GUI が既に落ちているので撃てない"
fi
check_alive "③"

echo "== ④ ほかの不正な値 =="
# 最後の 1 つは「ff」と「0000」のあいだにゼロ幅空白（U+200B）を挟んだ値（コピペで紛れ込む形）。
# 見えない文字をソースへ直に置かないよう、バイト列で書く（bash 3.2 の $'' は \x を解する）
for v in '#ff00' '#ff000000' 'ff00' '#ｆｆ００００' '＃ff0000' '#' '' '#+f+f+f' $'#ff\xe2\x80\x8b0000'; do
  if ! gui_alive; then fail "④ '$v': GUI が既に落ちているので撃てない"; continue; fi
  OUT="$("$TAKO_BIN" theme color accent "$v" 2>&1)"
  RC=$?
  if [ "$RC" -ne 0 ]; then pass "④ '$v' は失敗で返る"; else fail "④ '$v' が成功で返った: $OUT"; fi
  echo "      → $(printf '%s' "$OUT" | tr '\n' ' ')"
done
check_alive "④"
check_eq "④ 不正な値は保存されず #ff0000 が残る" "#ff0000" "$(saved_accent)"

echo "== ⑤ settings.json に #赤色 が保存済みの状態から起動する =="
stop_isolated_gui "$APP_PID"
APP_PID=""
python3 - "$SETTINGS" <<'PY'
import json, os, sys
p = sys.argv[1]
d = {}
if os.path.exists(p):
    try:
        d = json.load(open(p, encoding="utf-8"))
    except Exception:
        d = {}
d.setdefault("theme", "dark")
d.setdefault("theme_colors", {}).setdefault("dark", {})
d["theme_colors"]["dark"]["accent"] = "#赤色"
d["theme_colors"]["dark"]["red"] = "#ｆｆ００００"
d["theme_colors"]["dark"]["green"] = "#00ff00"
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
check_eq "⑤ 材料: settings.json に #赤色 が入っている" "#赤色" "$(saved_accent)"
: > "$TMP/app2.log"
if start_gui "$TMP/app2.log"; then
  pass "⑤ 起動して CLI が繋がる"
  check_alive "⑤"
  HEX="$(current_accent_hex)"
  case "$HEX" in
    \#[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) pass "⑤ accent は読める色（${HEX}）で立つ" ;;
    *) fail "⑤ accent が色として読めない: $HEX" ;;
  esac
  GREEN="$("$TAKO_BIN" theme colors 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["colors"]["green"]["hex"])' 2>/dev/null)"
  check_eq "⑤ 同じファイルの正しい上書き（green）は効く" "#00ff00" "$GREEN"
  check_eq "⑤ 起動しても settings.json の元の値は消さない" "#赤色" "$(saved_accent)"
  if grep -q "#赤色" "$PERSIST_LOG" 2>/dev/null; then
    pass "⑤ 無視した値が persist.log に残る"
    grep "テーマの色上書き" "$PERSIST_LOG" | sed 's/^/      /'
  else
    fail "⑤ 無視した値が persist.log に無い"
  fi
else
  fail "⑤ 起動しない（CLI が繋がらない）"
  show_panic
fi

if [ -f "$PANIC_LOG" ]; then
  fail "panic.log が作られた（どこかで panic した）"
else
  pass "panic.log が作られていない"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
