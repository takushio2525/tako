#!/usr/bin/env bash
# test-editor-keys-1652.sh — エディタの打鍵の実 GUI 経路テスト
#   （修飾キー #1652 / インデント #1654 / キー操作の残り #1742）
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、visual-test 節 `editor-keys` を
#   ① そのまま走らせる → 15 相（文書頭 / 語の左右 / 日本語の語 / ⇧ で選択 / 語の削除 /
#      smart Home / 桁の記憶 / Page Down / 文書末 / 開き括弧の直後の Enter / 選択行の Tab・⇧Tab /
#      選択の無い Tab / 表示幅の桁 / 選択中の ←→ で畳む / ⌃A・⌃E・⌃K / IME の変換中。
#      Enter と Tab の 3 相は #1654、表示幅・畳む・⌃A/E/K の 3 相は #1742）がすべて緑
#   ② A/B `TAKO_1652_LEGACY=1`（入口で修飾キー付きの打鍵を捨てる旧経路）で走らせる →
#      最初の修飾キーの相で FAILED（= 節に検出力がある）
#   ③ #1742 の 3 点を **CLI（`tako edit move` / `delete`）と MCP（`tako_preview_move` /
#      `tako_preview_delete`）** から同じ手順で撃ち、打鍵と同じ結果・CLI と MCP の応答が
#      字面まで一致することを見る（設計原則 5 の 1:1）
# を実測する。打鍵は `window.dispatch_keystroke` へ流すので、キーバインド判定 →
# `on_key_down` → 編集の入口 → 打鍵表 → TextBuffer の全段を通る。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-editor-keys-1652.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1652-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  stop_isolated_gui "${CLI_GUI_PID:-}"
  tmux -L "${TAKO_TMUX_SOCKET:-tako-1652-$$}" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# visual-test 版をビルドする（同じ target/debug/tako-app を上書きする）
echo "visual-test 版の tako-app をビルドします…"
(cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1652-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local log="$1"
  shift
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=editor-keys "$@" || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  return 0
}

echo "① editor-keys 節（修正後）"
run_section "$TMP/new.log"
grep "TAKO_VISUAL_PIXEL: editor-keys" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi

echo "② A/B: TAKO_1652_LEGACY=1（入口で修飾キー付きの打鍵を捨てる旧経路）"
run_section "$TMP/legacy.log" TAKO_1652_LEGACY=1
grep "TAKO_VISUAL_PIXEL: editor-keys" "$TMP/legacy.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test editor-keys" "$TMP/legacy.log"; then
  pass "旧経路では落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/legacy.log" | head -1 | cut -c1-120)"
else
  fail "旧経路でも落ちない（節に検出力が無い）"
fi

echo "③ #1742: CLI / MCP から同じ操作で同じ結果"
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
# 応答 JSON から 1 つの値を取る（`document` の下も辿れる）
jget() {
  python3 -c '
import json, sys
path = sys.argv[1].split(".")
try:
    cur = json.loads(sys.stdin.read())
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for key in path:
    if isinstance(cur, dict) and key in cur:
        cur = cur[key]
    else:
        print("__MISSING__"); sys.exit(0)
print(cur if isinstance(cur, str) else json.dumps(cur, ensure_ascii=False, sort_keys=True))
' "$1"
}
# ペイン ID だけは別物なので落とす（版も undo 履歴も比較に残す）
normalize() {
  python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
d.pop("pane", None)
print(json.dumps(d, ensure_ascii=False, sort_keys=True))
'
}
launch_isolated_gui "$TMP/cli.log" || exit $?
CLI_GUI_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/cli.log" || exit 1
# `tako mcp serve` は socket と token を env から読む（隔離 data の中のものを名指しする）
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
export TAKO_TOKEN
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])
')"
# 打鍵の相（visual-test）と同じ並びの行: 半角 / 全角 / タブ / 半角 / 語の行 / 全角を含む行
FIXTURE=$'0123456789\nあいう漢字\n\tlet x = 1;\n0123456789\n    let value = 1;\n    println!("日本語のテキスト");\n'
printf '%s' "$FIXTURE" > "$TMP/cli.rs"
printf '%s' "$FIXTURE" > "$TMP/mcp.rs"
CLI_PANE="$("$TAKO_BIN" open "$TMP/cli.rs" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
MCP_PANE="$("$TAKO_BIN" open "$TMP/mcp.rs" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
case "$CLI_PANE$MCP_PANE" in
  ''|*[!0-9]*) echo "プレビューペインを開けない: cli=$CLI_PANE mcp=$MCP_PANE"; exit 1 ;;
esac
"$TAKO_BIN" edit start --pane "$CLI_PANE" >/dev/null || exit 1
"$TAKO_BIN" edit start --pane "$MCP_PANE" >/dev/null || exit 1

# 手順（CLI の引数 | MCP のツール名 | MCP の引数 JSON）。行は 1 始まり・桁は行内 UTF-8 バイト
STEPS=(
  "cursor 1:4|tako_preview_cursor|{\"line\":1,\"col\":4}"
  "move down|tako_preview_move|{\"movement\":\"down\"}"
  "move down|tako_preview_move|{\"movement\":\"down\"}"
  "move down|tako_preview_move|{\"movement\":\"down\"}"
  "cursor 5:4 --select-to 5:13|tako_preview_cursor|{\"line\":5,\"col\":4,\"select_to_line\":5,\"select_to_col\":13}"
  "move left|tako_preview_move|{\"movement\":\"left\"}"
  "cursor 5:4 --select-to 5:13|tako_preview_cursor|{\"line\":5,\"col\":4,\"select_to_line\":5,\"select_to_col\":13}"
  "move right|tako_preview_move|{\"movement\":\"right\"}"
  "cursor 6:6 --select-to 5:8|tako_preview_cursor|{\"line\":6,\"col\":6,\"select_to_line\":5,\"select_to_col\":8}"
  "move right|tako_preview_move|{\"movement\":\"right\"}"
  "cursor 5:13|tako_preview_cursor|{\"line\":5,\"col\":13}"
  "move line-start|tako_preview_move|{\"movement\":\"line-start\"}"
  "move line-end|tako_preview_move|{\"movement\":\"line-end\"}"
  "cursor 5:8|tako_preview_cursor|{\"line\":5,\"col\":8}"
  "delete to-line-end|tako_preview_delete|{\"motion\":\"to-line-end\"}"
  "delete to-line-end|tako_preview_delete|{\"motion\":\"to-line-end\"}"
)
# 各手順のあとの（カーソル, 選択）。打鍵の相と同じ期待値
EXPECT=(
  '{"column": 4, "line": 1}|null'
  '{"column": 6, "line": 2}|null'
  '{"column": 1, "line": 3}|null'
  '{"column": 4, "line": 4}|null'
  '{"column": 13, "line": 5}|{"end": {"column": 13, "line": 5}, "start": {"column": 4, "line": 5}}'
  '{"column": 4, "line": 5}|null'
  '{"column": 13, "line": 5}|{"end": {"column": 13, "line": 5}, "start": {"column": 4, "line": 5}}'
  '{"column": 13, "line": 5}|null'
  '{"column": 8, "line": 5}|{"end": {"column": 6, "line": 6}, "start": {"column": 8, "line": 5}}'
  '{"column": 6, "line": 6}|null'
  '{"column": 13, "line": 5}|null'
  '{"column": 0, "line": 5}|null'
  '{"column": 18, "line": 5}|null'
  '{"column": 8, "line": 5}|null'
  '{"column": 8, "line": 5}|null'
  '{"column": 8, "line": 5}|null'
)
LABELS=(
  "起点" "↓ 全角の行で表示幅の桁 4（あい|う）" "↓ タブの行で表示幅の桁 4（\\t|let）" "↓ 半角の行で桁 4"
  "選択" "選択中の left は始点へ畳む" "選択" "選択中の right は終点へ畳む"
  "後ろ向きの複数行の選択" "right は終点へ畳む" "起点" "line-start（⌃A）は桁 0"
  "line-end（⌃E）は行末" "起点" "to-line-end（⌃K）は行末まで" "行末の to-line-end は改行を消す"
)
MCP_IN="$TMP/mcp.in"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1742","version":"0"}}}' > "$MCP_IN"
CLI_OUT=()
i=0
for step in ${STEPS[@]+"${STEPS[@]}"}; do
  IFS='|' read -r cli_args tool mcp_args <<< "$step"
  # shellcheck disable=SC2086
  CLI_OUT+=("$("$TAKO_BIN" edit $cli_args --pane "$CLI_PANE" 2>&1)")
  args="$(printf '%s' "$mcp_args" | python3 -c '
import json, sys
d = json.loads(sys.stdin.read()); d["pane"] = int(sys.argv[1]); print(json.dumps(d))
' "$MCP_PANE")"
  printf '{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' \
    $((i + 10)) "$tool" "$args" >> "$MCP_IN"
  i=$((i + 1))
done
MCP_OUT="$("$TAKO_BIN" mcp serve < "$MCP_IN" 2>/dev/null)"
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
i=0
for step in ${STEPS[@]+"${STEPS[@]}"}; do
  out="${CLI_OUT[$i]}"
  want_cursor="${EXPECT[$i]%%|*}"
  want_sel="${EXPECT[$i]#*|}"
  got_cursor="$(printf '%s' "$out" | jget document.cursor | python3 -c 'import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True))' 2>/dev/null)"
  got_sel="$(printf '%s' "$out" | jget document.selection | python3 -c 'import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True))' 2>/dev/null)"
  check_eq "CLI: ${LABELS[$i]}（${step%%|*}）" "$want_cursor|$want_sel" "$got_cursor|$got_sel"
  check_eq "MCP の応答が CLI と字面一致（${step%%|*}）" \
    "$(printf '%s' "$out" | normalize)" "$(extract $((i + 10)) | normalize)"
  i=$((i + 1))
done
# ⌃K 2 回の結果を本文で見る（保存したファイルのバイト列）
"$TAKO_BIN" edit save --pane "$CLI_PANE" >/dev/null
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1742","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_save\",\"arguments\":{\"pane\":${MCP_PANE}}}}" \
  | "$TAKO_BIN" mcp serve >/dev/null 2>&1
KILLED=$'0123456789\nあいう漢字\n\tlet x = 1;\n0123456789\n    let     println!("日本語のテキスト");\n'
printf '%s' "$KILLED" > "$TMP/killed.rs"
if cmp -s "$TMP/cli.rs" "$TMP/killed.rs" && cmp -s "$TMP/mcp.rs" "$TMP/killed.rs"; then
  pass "to-line-end 2 回で行末まで消えて次の行とつながる（CLI / MCP とも本文がバイト一致）"
else
  fail "to-line-end の結果が違う: cli=$(od -c "$TMP/cli.rs" | head -3 | tr -s ' ') mcp=$(cmp "$TMP/mcp.rs" "$TMP/killed.rs" 2>&1)"
fi
# undo 1 回ずつで戻る（2 回で元のファイルとバイト一致）
U1="$("$TAKO_BIN" edit undo --pane "$CLI_PANE" 2>&1)"
check_eq "undo 1 回目で改行が戻る（行数 6 → 7）" "7" "$(printf '%s' "$U1" | jget document.line_count)"
"$TAKO_BIN" edit undo --pane "$CLI_PANE" >/dev/null
"$TAKO_BIN" edit save --pane "$CLI_PANE" >/dev/null
printf '%s' "$FIXTURE" > "$TMP/original.rs"
if cmp -s "$TMP/cli.rs" "$TMP/original.rs"; then
  pass "undo 2 回で元のファイルとバイト一致"
else
  fail "undo 2 回で元に戻らない"
fi
stop_isolated_gui "$CLI_GUI_PID"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
