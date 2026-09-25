#!/usr/bin/env bash
# test-edit-range-1658.sh — 行・桁で指す範囲編集とカーソル操作の実経路テスト（#1658）
#
# 隔離した data / tmux で**実 tako-app** を立て、5,000 行のファイルをプレビューで開いて、
#
#   ① 1 行だけを `tako edit replace-range` で差し替える。**他の行はバイト一致**・
#      カーソルは差し替えた本文の末尾・undo 1 回で元に戻る（#1658 の本体）
#   ② `tako edit cursor` の移動と `--select-to` の選択が `tako edit status` で読める
#      （位置 / 選択 / 版 / undo_depth）
#   ③ MCP `tako_preview_edit_range` / `tako_preview_cursor` が同じ dispatch を通り、
#      応答が CLI と**字面まで一致**する（設計原則 5 の 1:1）
#   ④ 版（version）が全文置換・範囲編集・undo / redo で単調増加し、
#      `--expect-version` に古い版を渡すと**何もせず**失敗する（楽観ロック）
#   ⑤ エッジ: 範囲外の行 / 文字の途中の桁 / CRLF の CR と LF のあいだ /
#      空範囲への挿入 / 全範囲の置換
#   ⑥ 送るバイト数の実測: 1 行直すのに全文置換（apply）が何バイト要るか vs 範囲編集
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-edit-range-1658.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる前提の実経路テスト。
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

TMP="$(mktemp -d /tmp/tako-1658-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1658-$$"
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

# 応答 JSON から 1 つの値を取る（`document` の下も辿れる）
jget() {
  python3 -c '
import json, sys
path = sys.argv[1].split(".")
try:
    cur = json.loads(sys.stdin.read())
except Exception as e:
    print("__NOT_JSON__"); sys.exit(0)
for key in path:
    if isinstance(cur, dict) and key in cur:
        cur = cur[key]
    else:
        print("__MISSING__"); sys.exit(0)
# 文字列はそのまま、それ以外（null / true / false / 数値 / 入れ子）は JSON の綴りで出す
print(cur if isinstance(cur, str) else json.dumps(cur, ensure_ascii=False, sort_keys=True))
' "$1"
}

echo "== 隔離 GUI を起こす =="
launch_isolated_gui "$TMP/app.log" || exit 1
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
echo "  pid=$APP_PID"

# --- 5,000 行のファイルを作る -------------------------------------------------
BIG="$TMP/big.txt"
python3 -c '
import sys
with open(sys.argv[1], "w", encoding="utf-8") as f:
    for i in range(1, 5001):
        f.write("line %04d: the quick brown fox jumps over the lazy dog\n" % i)
' "$BIG"
BIG_BYTES=$(wc -c < "$BIG" | tr -d ' ')
echo "  5,000 行 / ${BIG_BYTES} バイト"

# tako の外から撃つので基準ペインを名指しする（省略すると呼び出し元が解けない）
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])
')"
OPENED="$("$TAKO_BIN" open "$BIG" --pane "$ROOT_PANE" --new-tab 2>&1)"
PANE="$(printf '%s' "$OPENED" | jget pane)"
case "$PANE" in
  ''|*[!0-9]*) echo "プレビューペインを開けない: $OPENED"; exit 1 ;;
esac
"$TAKO_BIN" edit start --pane "$PANE" >/dev/null || exit 1

echo
echo "== ⑥ 1 行直すのに送るバイト数 =="
TARGET_LINE=2500
NEW_TEXT="line 2500: REPLACED BY tako edit replace-range"
OLD_LINE="$(sed -n "${TARGET_LINE}p" "$BIG")"
OLD_LEN=${#OLD_LINE}
# 全文置換（apply）が送る本文 = ファイル全文
echo "  edit apply       : ${BIG_BYTES} バイト（本文全文）"
RANGE_BYTES=$(printf '%s' "${TARGET_LINE}:0 ${TARGET_LINE}:${OLD_LEN} ${NEW_TEXT}" | wc -c | tr -d ' ')
echo "  edit replace-range: ${RANGE_BYTES} バイト（位置 + 置換文字列）"
if [ "$RANGE_BYTES" -lt "$BIG_BYTES" ]; then
  pass "範囲編集のほうが送るバイト数が少ない（$((BIG_BYTES / RANGE_BYTES)) 分の 1）"
else
  fail "範囲編集のほうが大きい"
fi

echo
echo "== ① 1 行だけを差し替える =="
BEFORE_VER="$("$TAKO_BIN" edit status --pane "$PANE" | jget document.version)"
EDITED="$("$TAKO_BIN" edit replace-range "${TARGET_LINE}:0" "${TARGET_LINE}:${OLD_LEN}" "$NEW_TEXT" --pane "$PANE" 2>&1)"
check_eq "差し替え後の版が 1 つ進む" "$((BEFORE_VER + 1))" "$(printf '%s' "$EDITED" | jget document.version)"
check_eq "カーソル行" "$TARGET_LINE" "$(printf '%s' "$EDITED" | jget document.cursor.line)"
check_eq "カーソル桁（差し替えた本文の末尾）" "${#NEW_TEXT}" "$(printf '%s' "$EDITED" | jget document.cursor.column)"
check_eq "行数は変わらない" "5001" "$(printf '%s' "$EDITED" | jget document.line_count)"

"$TAKO_BIN" edit save --pane "$PANE" >/dev/null || fail "保存できない"
# 期待値は**生成規則から作り直す**（open より前の写しを持ち回らない）
EXPECT="$TMP/expect.txt"
python3 -c '
import sys
line = int(sys.argv[2]); text = sys.argv[3]
out = []
for i in range(1, 5001):
    out.append(text if i == line else "line %04d: the quick brown fox jumps over the lazy dog" % i)
open(sys.argv[1], "w", encoding="utf-8").write("\n".join(out) + "\n")
' "$EXPECT" "$TARGET_LINE" "$NEW_TEXT"
if cmp -s "$BIG" "$EXPECT"; then
  pass "差し替えた 1 行以外は 4,999 行すべてバイト一致"
else
  fail "他の行が変わった: $(cmp "$BIG" "$EXPECT" 2>&1 | head -1)"
fi

UNDONE="$("$TAKO_BIN" edit undo --pane "$PANE" 2>&1)"
check_eq "undo 1 回で戻る（undone）" "true" "$(printf '%s' "$UNDONE" | jget undone)"
"$TAKO_BIN" edit save --pane "$PANE" >/dev/null
python3 -c '
out = ["line %04d: the quick brown fox jumps over the lazy dog" % i for i in range(1, 5001)]
open(__import__("sys").argv[1], "w", encoding="utf-8").write("\n".join(out) + "\n")
' "$TMP/original.txt"
if cmp -s "$BIG" "$TMP/original.txt"; then
  pass "undo 後の保存で元のファイルとバイト一致"
else
  fail "undo で元に戻らない"
fi

echo
echo "== ② カーソルと選択 =="
MOVED="$("$TAKO_BIN" edit cursor 10:5 --select-to 12:3 --pane "$PANE" 2>&1)"
check_eq "カーソルは select_to 側" "12" "$(printf '%s' "$MOVED" | jget document.cursor.line)"
check_eq "選択の起点" "10" "$(printf '%s' "$MOVED" | jget document.selection.start.line)"
check_eq "選択の終端桁" "3" "$(printf '%s' "$MOVED" | jget document.selection.end.column)"
STATUS="$("$TAKO_BIN" edit status --pane "$PANE" 2>&1)"
check_eq "status でも同じ位置が読める" "12" "$(printf '%s' "$STATUS" | jget document.cursor.line)"
check_eq "status に undo_depth が載る" "0" "$(printf '%s' "$STATUS" | jget document.undo_depth)"
CURSOR_ONLY="$("$TAKO_BIN" edit cursor 7 --pane "$PANE" 2>&1)"
check_eq "桁を省くと行頭" "0" "$(printf '%s' "$CURSOR_ONLY" | jget document.cursor.column)"
check_eq "選択は解ける" "null" "$(printf '%s' "$CURSOR_ONLY" | jget document.selection)"

echo
echo "== ④ 版の単調増加と楽観ロック =="
V0="$("$TAKO_BIN" edit status --pane "$PANE" | jget document.version)"
V1="$("$TAKO_BIN" edit replace-range 3:0 3:0 "X" --pane "$PANE" | jget document.version)"
APPLY_TEXT="$(printf '%s\n%s\n' '全文を置き換える' '2 行目')"
V2="$("$TAKO_BIN" edit apply "$APPLY_TEXT" --pane "$PANE" | jget document.version)"
V3="$("$TAKO_BIN" edit undo --pane "$PANE" | jget document.version)"
V4="$("$TAKO_BIN" edit redo --pane "$PANE" | jget document.version)"
if [ "$V1" -gt "$V0" ] && [ "$V2" -gt "$V1" ] && [ "$V3" -gt "$V2" ] && [ "$V4" -gt "$V3" ]; then
  pass "版が単調増加する（${V0} → ${V1} → ${V2} → ${V3} → ${V4}）"
else
  fail "版が単調増加しない（${V0} ${V1} ${V2} ${V3} ${V4}）"
fi
STALE_OUT="$("$TAKO_BIN" edit replace-range 1:0 1:0 "Z" --expect-version "$V0" --pane "$PANE" 2>&1)"
STALE_RC=$?
if [ "$STALE_RC" -ne 0 ]; then
  pass "古い版を指定すると失敗する"
else
  fail "古い版でも通ってしまう: $STALE_OUT"
fi
check_eq "拒否したので版は動かない" "$V4" "$("$TAKO_BIN" edit status --pane "$PANE" | jget document.version)"
FRESH="$("$TAKO_BIN" edit replace-range 1:0 1:0 "Z" --expect-version "$V4" --pane "$PANE" 2>&1)"
check_eq "現在の版を指定すれば通る" "$((V4 + 1))" "$(printf '%s' "$FRESH" | jget document.version)"

echo
echo "== ⑤ エッジケース =="
edge_reject() {
  local label="$1"; shift
  local out
  out="$("$@" 2>&1)"
  if [ $? -ne 0 ]; then pass "$label"; else fail "${label}（通ってしまった: ${out}）"; fi
}
BEFORE_EDGE="$("$TAKO_BIN" edit status --pane "$PANE" | jget document.version)"
edge_reject "範囲外の行を拒否" "$TAKO_BIN" edit replace-range 9999:0 9999:1 "X" --pane "$PANE"
edge_reject "0 行目を拒否" "$TAKO_BIN" edit replace-range 0:0 1:0 "X" --pane "$PANE"
# 多バイト文字の途中の桁
MULTIBYTE_TEXT="$(printf '%s\n%s\n' 'あいう' 'マルチバイトの行')"
"$TAKO_BIN" edit apply "$MULTIBYTE_TEXT" --pane "$PANE" >/dev/null
edge_reject "文字の途中の桁を拒否" "$TAKO_BIN" edit replace-range 1:1 1:3 "X" --pane "$PANE"
edge_reject "行の長さを超えた桁を拒否" "$TAKO_BIN" edit replace-range 1:10 1:10 "X" --pane "$PANE"
# CRLF ファイル
CRLF="$TMP/crlf.txt"
printf 'abc\r\ndef\r\n' > "$CRLF"
CRLF_OPEN="$("$TAKO_BIN" open "$CRLF" --pane "$ROOT_PANE" --new-tab 2>&1)"
CPANE="$(printf '%s' "$CRLF_OPEN" | jget pane)"
"$TAKO_BIN" edit start --pane "$CPANE" >/dev/null
check_eq "CRLF を検出する" "crlf" "$("$TAKO_BIN" edit status --pane "$CPANE" | jget document.line_ending)"
edge_reject "CR と LF のあいだの桁を拒否" "$TAKO_BIN" edit replace-range 1:4 1:4 "X" --pane "$CPANE"
"$TAKO_BIN" edit replace-range 1:0 1:3 "XY" --pane "$CPANE" >/dev/null
"$TAKO_BIN" edit save --pane "$CPANE" >/dev/null
if [ "$(od -c < "$CRLF" | head -1 | tr -s ' ')" = "$(printf 'XY\r\ndef\r\n' | od -c | head -1 | tr -s ' ')" ]; then
  pass "既存行の CRLF が 1 バイトも変わらない"
else
  fail "CRLF が壊れた: $(od -c < "$CRLF" | head -2)"
fi
# 空範囲への挿入 / 全範囲の置換
"$TAKO_BIN" edit replace-range 2:1 2:1 "ZZ" --pane "$CPANE" >/dev/null
"$TAKO_BIN" edit save --pane "$CPANE" >/dev/null
check_eq "空範囲は挿入になる" "$(printf 'XY\r\ndZZef\r\n')" "$(cat "$CRLF")"
LAST="$("$TAKO_BIN" edit status --pane "$CPANE" | jget document.line_count)"
ALL_TEXT="$(printf '%s\n' 'all replaced')"
"$TAKO_BIN" edit replace-range 1:0 "${LAST}:0" "$ALL_TEXT" --pane "$CPANE" >/dev/null
"$TAKO_BIN" edit save --pane "$CPANE" >/dev/null
check_eq "全範囲の置換" "all replaced" "$(cat "$CRLF")"

echo
echo "== ③ MCP が CLI と同じ応答を返す =="
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
export TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
# **同じ中身のファイルを 2 枚**開き、片方は CLI から・片方は MCP から同じ手順を撃つ。
# 初期状態を揃えないと undo 履歴の深さが食い違い、応答の字面が比べられない
# **末尾の改行を落とさない**（`$( )` は末尾改行を剥がすので、MCP へ送る JSON の
# 本文と 1 バイトずれて応答の line_count / bytes が食い違う）
MCP_BODY=$'one\ntwo\nthree\n'
printf 'one\ntwo\nthree\n' > "$TMP/cli.txt"
printf 'one\ntwo\nthree\n' > "$TMP/mcp.txt"
CLI_PANE="$("$TAKO_BIN" open "$TMP/cli.txt" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
MCP_PANE="$("$TAKO_BIN" open "$TMP/mcp.txt" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
"$TAKO_BIN" edit start --pane "$CLI_PANE" >/dev/null
"$TAKO_BIN" edit start --pane "$MCP_PANE" >/dev/null
"$TAKO_BIN" edit apply "$MCP_BODY" --pane "$CLI_PANE" >/dev/null
CLI_RANGE="$("$TAKO_BIN" edit replace-range 2:0 2:3 "TWO" --pane "$CLI_PANE")"
CLI_CURSOR="$("$TAKO_BIN" edit cursor 1:1 --select-to 3:2 --pane "$CLI_PANE")"
MCP_OUT="$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1658","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_apply\",\"arguments\":{\"pane\":${MCP_PANE},\"text\":\"one\\ntwo\\nthree\\n\"}}}" \
  "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_edit_range\",\"arguments\":{\"pane\":${MCP_PANE},\"start_line\":2,\"start_col\":0,\"end_line\":2,\"end_col\":3,\"text\":\"TWO\"}}}" \
  "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_cursor\",\"arguments\":{\"pane\":${MCP_PANE},\"line\":1,\"col\":1,\"select_to_line\":3,\"select_to_col\":2}}}" \
  | "$TAKO_BIN" mcp serve 2>/dev/null)"
extract() {
  printf '%s' "$MCP_OUT" | python3 -c '
import json, sys
want = int(sys.argv[1])
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == want:
        text = msg.get("result", {}).get("content", [{}])[0].get("text", "")
        try:
            print(json.dumps(json.loads(text), ensure_ascii=False, sort_keys=True))
        except Exception:
            print(text)
' "$1"
}
# ペイン ID だけは別物なので落とす（**版も undo 履歴も比較に残す**）
normalize() {
  python3 -c '
import json, sys
d = json.loads(sys.stdin.read())
d.pop("pane", None)
print(json.dumps(d, ensure_ascii=False, sort_keys=True))
'
}
MCP_RANGE="$(extract 3)"
MCP_CURSOR="$(extract 4)"
check_eq "範囲編集の応答が CLI と一致" \
  "$(printf '%s' "$CLI_RANGE" | normalize)" "$(printf '%s' "$MCP_RANGE" | normalize)"
check_eq "カーソルの応答が CLI と一致" \
  "$(printf '%s' "$CLI_CURSOR" | normalize)" "$(printf '%s' "$MCP_CURSOR" | normalize)"

echo
echo "== 結果 =="
echo "  PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
