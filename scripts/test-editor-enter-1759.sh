#!/usr/bin/env bash
# test-editor-enter-1759.sh — Enter の続き（抜ける文の後ろ・リストの印）と括弧の自動閉じ（#1759）の実 GUI 経路テスト
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、
#   ① visual-test 節 `enter-1759` を新挙動と `TAKO_1759_LEGACY=1`（#1654 の挙動）の 2 回走らせる。
#      新挙動は全相が緑・旧挙動は ① の相で FAILED（同一バイナリの A/B）。打鍵は
#      `window.dispatch_keystroke` へ流すので、キーバインド → 編集の入口 → 打鍵表 / 文字入力の口 →
#      TextBuffer の全段を通る。前後の画像は新旧で別の dir へ落とす
#   ② CLI `tako edit cursor / newline / type` で同じ手順を撃ち、保存したファイルが ① の GUI の
#      打鍵の結果（`gui-*`）とバイト一致する（設計原則 5: UI でできることは AI からも同じ結果）
#   ③ MCP `tako_preview_edit` の `command`（newline / type + text）で同じ手順を撃ち、CLI とバイト一致
#   ④ 端: 設定 off で閉じない（settings.json へ残る）・文字列の中で閉じない・打つ文字の無い type・
#      上限超えの type・text を取らない command へ text
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-editor-enter-1759.sh
#   画像の置き場は TAKO_1759_SHOT_DIR（既定 target/tako-1759-shots。new/ と legacy/ に分ける）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHOT_DIR="${TAKO_1759_SHOT_DIR:-$REPO_ROOT/target/tako-1759-shots}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
# 2 つのファイルがバイト一致するか
check_same() {
  if cmp -s "$2" "$3"; then
    pass "$1"
  else
    fail "$1: $(od -c "$2" | head -2 | tr -s ' ') ≠ $(od -c "$3" | head -2 | tr -s ' ')"
  fi
}

TMP="$(mktemp -d /tmp/tako-1759-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  stop_isolated_gui "${CLI_GUI_PID:-}"
  tmux -L "${TAKO_TMUX_SOCKET:-tako-1759-$$}" kill-server >/dev/null 2>&1 || true
  # 止めた直後の GUI の子（ペインのシェル・ログの書き出し）がまだ書いていることがあるので、
  # 消し切れなければ少し待って消し直す（実測: 1 回目の rm -rf が Directory not empty で残った）
  local _
  for _ in 1 2 3 4 5; do
    rm -rf "$TMP" 2>/dev/null && break
    sleep 0.5
  done
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# visual-test 版をビルドする（HOME を差し替える前に。差し替えた後だとツールチェーンを取り直す）
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1759-$$"
# 言語サーバを配布元から取りに行かない（検証のたびに取り直さない。#1944）
export TAKO_LSP_AUTO_FETCH=0
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
rm -rf "$SHOT_DIR/new" "$SHOT_DIR/legacy"
mkdir -p "$SHOT_DIR/new" "$SHOT_DIR/legacy"

# visual-test を 1 回走らせて終わるまで待つ（ログ・画像の置き場・追加の env）
run_visual() {
  local log="$1" dump="$2"
  shift 2
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=enter-1759 \
    TAKO_VISUAL_DUMP_DIR="$dump" "$@" || exit $?
  local pid="$ISOLATED_GUI_PID"
  for _ in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
}

echo "① visual-test 節 enter-1759（新挙動）"
run_visual "$TMP/new.log" "$SHOT_DIR/new" || exit $?
grep "TAKO_VISUAL_PIXEL: enter-1759 \(python\|markdown\|rust\)" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "新挙動: 全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "新挙動: 節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi
check_eq "新挙動: 前後の画像が 6 枚落ちる" "6" "$(find "$SHOT_DIR/new" -name 'enter-1759-*.png' | wc -l | tr -d ' ')"

echo "① visual-test 節 enter-1759（TAKO_1759_LEGACY=1 = #1654 の挙動）"
run_visual "$TMP/legacy.log" "$SHOT_DIR/legacy" TAKO_1759_LEGACY=1 || exit $?
grep "TAKO_VISUAL_PIXEL: enter-1759 \(python\|markdown\|rust\)" "$TMP/legacy.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test enter-1759: ① return の後ろの Enter で 1 段浅くなる" "$TMP/legacy.log"; then
  pass "旧挙動: ① の相で落ちる（同一バイナリの A/B）"
else
  fail "旧挙動が ① で落ちない（検出力が無い）"
  grep -E "FAILED|TAKO_VISUAL_TEST_OK" "$TMP/legacy.log" | tail -3 | sed 's/^/    /'
fi
echo "  画像: $SHOT_DIR/new / $SHOT_DIR/legacy"
find "$SHOT_DIR" -name '*.png' | sort | sed 's/^/    /'

echo "② CLI から同じ手順で GUI の打鍵と同じ結果"
launch_isolated_gui "$TMP/cli.log" || exit $?
CLI_GUI_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/cli.log" || exit 1
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
export TAKO_TOKEN
# 隔離 GUI でも自動命名は claude を呼ぶので止める
"$TAKO_BIN" autorename off >/dev/null 2>&1 || true
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | jq -r '.tabs[0].panes[0].id')"

# ファイルを開いて編集モードにし、ペイン ID を返す
open_editing() {
  local pane
  pane="$("$TAKO_BIN" open "$1" --pane "$ROOT_PANE" --new-tab 2>&1 | jq -r '.pane // empty')"
  case "$pane" in
    '' | *[!0-9]*) echo "プレビューペインを開けない: $1" >&2; return 1 ;;
  esac
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null || return 1
  echo "$pane"
}
# visual-test の素材と同じ本文（enter_1759.rs の PY / MD / RS）
PY_SRC=$'def f(x):\n    if x:\n        return 1\n'
MD_SRC=$'- [x] done\n9. nine\n'
RS_SRC=$'fn main() {\n    let v = ;\n}\n'
mkdir -p "$TMP/cli" "$TMP/mcp"
for d in cli mcp; do
  printf '%s' "$PY_SRC" > "$TMP/$d/enter.py"
  printf '%s' "$MD_SRC" > "$TMP/$d/list.md"
  printf '%s' "$RS_SRC" > "$TMP/$d/pair.rs"
done

# ① Python: return の行末で Enter → y = 2
P="$(open_editing "$TMP/cli/enter.py")" || exit 1
"$TAKO_BIN" edit cursor 3:16 --pane "$P" >/dev/null
OUT="$("$TAKO_BIN" edit newline --pane "$P" 2>&1)"
check_eq "CLI ①: newline の後のカーソルは 4 行目の 4 桁（1 段浅い）" "4:4" \
  "$(printf '%s' "$OUT" | jq -r '"\(.document.cursor.line):\(.document.cursor.column)"')"
"$TAKO_BIN" edit type "y = 2" --pane "$P" >/dev/null
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_same "CLI ① の結果が GUI の打鍵とバイト一致" "$SHOT_DIR/new/gui-enter.py" "$TMP/cli/enter.py"

# ② Markdown: 9. の行末で Enter → ten → Enter → Enter（空の項目で抜ける）
P="$(open_editing "$TMP/cli/list.md")" || exit 1
"$TAKO_BIN" edit cursor 2:7 --pane "$P" >/dev/null
"$TAKO_BIN" edit newline --pane "$P" >/dev/null
"$TAKO_BIN" edit type ten --pane "$P" >/dev/null
"$TAKO_BIN" edit newline --pane "$P" >/dev/null
OUT="$("$TAKO_BIN" edit newline --pane "$P" 2>&1)"
check_eq "CLI ②: 空の項目の Enter で行頭へ（改行は増えない = 4 行目）" "4:0" \
  "$(printf '%s' "$OUT" | jq -r '"\(.document.cursor.line):\(.document.cursor.column)"')"
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_eq "CLI ②: 番号を進めて引き継ぎ、空の項目は印が消える" "- [x] done|9. nine|10. ten||" "$(tr '\n' '|' < "$TMP/cli/list.md")"
check_same "CLI ② の結果が GUI の打鍵とバイト一致" "$SHOT_DIR/new/gui-list.md" "$TMP/cli/list.md"

# ③ Rust: ; の手前で f(x → ) が自動で入る。) で飛び越す（本文も版も変えない）
P="$(open_editing "$TMP/cli/pair.rs")" || exit 1
"$TAKO_BIN" edit cursor 2:12 --pane "$P" >/dev/null
OUT="$("$TAKO_BIN" edit type "f(x" --pane "$P" 2>&1)"
check_eq "CLI ③: f(x の後のカーソルは ) の手前・auto_close が効いている" "2:15 true" \
  "$(printf '%s' "$OUT" | jq -r '"\(.document.cursor.line):\(.document.cursor.column) \(.document.auto_close)"')"
V1="$(printf '%s' "$OUT" | jq -r '.document.version')"
OUT="$("$TAKO_BIN" edit type ")" --pane "$P" 2>&1)"
check_eq "CLI ③: ) は飛び越す（カーソル 16 桁・版は進まない）" "2:16 $V1" \
  "$(printf '%s' "$OUT" | jq -r '"\(.document.cursor.line):\(.document.cursor.column) \(.document.version)"')"
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_same "CLI ③ の結果が GUI の打鍵とバイト一致" "$SHOT_DIR/new/gui-pair.rs" "$TMP/cli/pair.rs"

echo "③ MCP から同じ手順で CLI と同じ結果"
MP_PY="$(open_editing "$TMP/mcp/enter.py")" || exit 1
MP_MD="$(open_editing "$TMP/mcp/list.md")" || exit 1
MP_RS="$(open_editing "$TMP/mcp/pair.rs")" || exit 1
MCP_IN="$TMP/mcp.in"
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1759","version":"0"}}}'
  id=10
  call() {
    printf '{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' "$id" "$1" "$2"
    id=$((id + 1))
  }
  call tako_preview_cursor "{\"pane\":$MP_PY,\"line\":3,\"col\":16}"
  call tako_preview_edit "{\"pane\":$MP_PY,\"command\":\"newline\"}"
  call tako_preview_edit "{\"pane\":$MP_PY,\"command\":\"type\",\"text\":\"y = 2\"}"
  call tako_preview_save "{\"pane\":$MP_PY}"
  call tako_preview_cursor "{\"pane\":$MP_MD,\"line\":2,\"col\":7}"
  call tako_preview_edit "{\"pane\":$MP_MD,\"command\":\"newline\"}"
  call tako_preview_edit "{\"pane\":$MP_MD,\"command\":\"type\",\"text\":\"ten\"}"
  call tako_preview_edit "{\"pane\":$MP_MD,\"command\":\"newline\"}"
  call tako_preview_edit "{\"pane\":$MP_MD,\"command\":\"newline\"}"
  call tako_preview_save "{\"pane\":$MP_MD}"
  call tako_preview_cursor "{\"pane\":$MP_RS,\"line\":2,\"col\":12}"
  call tako_preview_edit "{\"pane\":$MP_RS,\"command\":\"type\",\"text\":\"f(x)\"}"
  call tako_preview_save "{\"pane\":$MP_RS}"
} > "$MCP_IN"
MCP_OUT="$("$TAKO_BIN" mcp serve < "$MCP_IN" 2>/dev/null)"
check_eq "MCP の呼び出しがすべて成功（isError なし）" "0" \
  "$(printf '%s\n' "$MCP_OUT" | jq -s '[.[] | select(.id >= 10) | select(.result.isError == true or .error != null)] | length')"
for f in enter.py list.md pair.rs; do
  check_same "MCP の $f が CLI とバイト一致" "$TMP/cli/$f" "$TMP/mcp/$f"
done

echo "④ 端"
# 設定 off: 応答・settings.json の両方へ残り、( を打っても閉じない
OUT="$("$TAKO_BIN" edit auto-close off 2>&1)"
check_eq "auto-close off の応答" "false" "$(printf '%s' "$OUT" | jq -r '.auto_close')"
check_eq "settings.json に editor_auto_close=false が残る" "false" "$(jq -r '.editor_auto_close' "$TAKO_DATA_DIR/settings.json" 2>/dev/null)"
printf 'fn f() {\n    g\n}\n' > "$TMP/off.rs"
P="$(open_editing "$TMP/off.rs")" || exit 1
"$TAKO_BIN" edit cursor 2:5 --pane "$P" >/dev/null
OUT="$("$TAKO_BIN" edit type "(" --pane "$P" 2>&1)"
check_eq "off の間は ( だけ入る（document.auto_close=false）" "false" "$(printf '%s' "$OUT" | jq -r '.document.auto_close')"
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_eq "off の本文" "fn f() {|    g(|}|" "$(tr '\n' '|' < "$TMP/off.rs")"
check_eq "auto-close（状態だけ）は off のまま" "false" "$("$TAKO_BIN" edit auto-close 2>&1 | jq -r '.auto_close')"
"$TAKO_BIN" edit auto-close on >/dev/null
check_eq "on へ戻すと開いている編集にも効く" "true" "$("$TAKO_BIN" edit status --pane "$P" 2>&1 | jq -r '.document.auto_close')"
# 文字列の中では閉じない（直後は空白 = 次の文字の規則では閉じてよい位置）
printf 'let s = "a  b";\n' > "$TMP/str.rs"
P="$(open_editing "$TMP/str.rs")" || exit 1
"$TAKO_BIN" edit cursor 1:11 --pane "$P" >/dev/null
"$TAKO_BIN" edit type "(" --pane "$P" >/dev/null
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_eq "文字列の中の ( は閉じない" 'let s = "a ( b";' "$(cat "$TMP/str.rs")"
# Markdown（文書）は on でも閉じない
printf 'see \n' > "$TMP/doc.md"
P="$(open_editing "$TMP/doc.md")" || exit 1
OUT="$("$TAKO_BIN" edit cursor 1:4 --pane "$P" 2>&1)"
check_eq "Markdown は document.auto_close=false" "false" "$(printf '%s' "$OUT" | jq -r '.document.auto_close')"
# 打つ文字の無い type / 上限超えの type / text を取らない command は断り、本文は変えない
P="$(open_editing "$TMP/cli/pair.rs")" || exit 1
V0="$("$TAKO_BIN" edit status --pane "$P" 2>&1 | jq -r '.document.version')"
ERR="$("$TAKO_BIN" edit type "" --pane "$P" 2>&1)"; RC=$?
check_eq "空の type は失敗で終わる" "1" "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
LONG="$(printf 'x%.0s' $(seq 1 1001))"
ERR="$("$TAKO_BIN" edit type "$LONG" --pane "$P" 2>&1)"; RC=$?
check_eq "1001 文字の type は理由つきで断る" "1|1" "$([ "$RC" -ne 0 ] && echo 1 || echo 0)|$(printf '%s' "$ERR" | grep -c '1000 文字まで')"
MCP_ERR="$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1759","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_edit\",\"arguments\":{\"pane\":${P},\"command\":\"newline\",\"text\":\"x\"}}}" \
  "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_edit\",\"arguments\":{\"pane\":${P},\"command\":\"type\"}}}" \
  | "$TAKO_BIN" mcp serve 2>/dev/null)"
check_eq "MCP: text を取らない command へ text は isError" "true" "$(printf '%s\n' "$MCP_ERR" | jq -r 'select(.id == 2) | .result.isError')"
check_eq "MCP: text の無い type は isError" "true" "$(printf '%s\n' "$MCP_ERR" | jq -r 'select(.id == 3) | .result.isError')"
check_eq "断った操作は版を進めない" "$V0" "$("$TAKO_BIN" edit status --pane "$P" 2>&1 | jq -r '.document.version')"
stop_isolated_gui "$CLI_GUI_PID"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
