#!/bin/bash
# test-lsp-format-1683.sh — 整形（#1683）の実経路テスト（隔離 GUI）
#
# 何を確かめるか（Issue #1683 の受け入れ条件のうち、CLI / MCP / メニューから GUI を通して言えるもの）:
#   ① tako lsp format → 編集バッファが整形される（保存してディスクで見る）・版が 1 つだけ進む・
#      undo 1 回で整形の前とバイト一致
#   ② 範囲の整形は範囲の外を 1 バイトも変えない（範囲はサーバの座標 = UTF-16 で送る）
#   ③ CLI と MCP が同じ答え
#   ④ GUI の入口（編集メニュー「コードを整形」= ⇧⌘I と同じアクション）も同じ 1 本を通る
#   ⑤ 保存時整形: 既定 off → on で `tako edit save` とメニューの保存（⌘S と同じアクション）が
#      整形してから保存する。自動保存は整形しない
#   ⑥ エッジ: サーバの無い種類（編集モードへ入らない）/ 未導入 / CRLF / 外部変更の競合中 /
#      待つあいだの変更（stale）/ 保存時整形の未応答（保存は止まらない）
#   ⑦ 実サーバ（rust-analyzer と rustfmt があるとき）: 実ファイルが rustfmt の出力どおりに整形され、
#      undo 1 回で戻る。構文エラーのファイル・範囲の整形の能力
#
# 使い方: bash scripts/test-lsp-format-1683.sh
#   修正前のビルドで測るときは `APP_BIN=<tako-app> TAKO_BIN=<tako>` を渡す（`tako lsp format` が無い = FAIL）
#   実サーバの節を飛ばすときは `TAKO_1683_SKIP_REAL=1`
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1683-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1683-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
FAKE="$REPO_ROOT/target/debug/tako-lsp-fake"
if [ ! -x "$FAKE" ]; then
  (cd "$REPO_ROOT" && cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
fi

# 実サーバの節で使う（HOME を差し替える前の rustup / cargo の置き場）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTFMT="$(command -v rustfmt 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/data"
mkdir -p "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DATA_DIR"

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
fmt() { "$TAKO_BIN" lsp format "$@" --json 2>/dev/null; }
save() { "$TAKO_BIN" edit save "$@" 2>&1; }
same_file() { cmp -s "$1" "$2"; }
# 期待の本文をファイルへ（printf の書式 = エスケープを解く）
expect() { printf "$2" > "$1"; }
# ペインの版（`tako edit status` の document.version）
version() { "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | json 'd["document"]["version"]'; }
# 自動保存を止める（保存の検査が 500ms の自動保存と競わないように）
autosave_off() { "$TAKO_BIN" edit autosave false --pane "$1" >/dev/null 2>&1; }
open_code() { "$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]'; }

mcp() { # 引数のツール名と JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1683","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        # 要求の変換で断られたら JSON-RPC の error（理由の文）をそのまま出す
        r = m.get("result")
        print(r["content"][0]["text"] if r else json.dumps(m.get("error"), ensure_ascii=False)); break'
}

# メニュー項目のパスをアクション名から引く（表示言語に依らない）
menu_path() {
  "$TAKO_BIN" menu list 2>/dev/null | python3 -c 'import json,sys
want = sys.argv[1]
d = json.load(sys.stdin)
def walk(items, prefix):
    for it in items:
        if it.get("kind") == "action" and it.get("action") == want:
            print(prefix + "/" + it["label"]); return True
        if it.get("kind") == "submenu" and walk(it.get("items", []), prefix + "/" + it.get("label", "")):
            return True
    return False
for m in d["menus"]:
    if walk(m["items"], m["name"]):
        break' "$1"
}

# --- 材料（偽サーバの節） -----------------------------------------------------------
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
RULES="$TMP/format.json"
cat > "$RULES" <<'JSON'
[
  { "uri_suffix": "slow.rs", "delay_ms": 1500 },
  { "uri_suffix": "silent.rs", "silent": true }
]
JSON
LOG="$TMP/fake.jsonl"

echo "== 起動（偽サーバを rust-analyzer の代わりに・pyright は未導入・保存時整形の上限 2 秒） =="
launch_isolated_gui "$TMP/app.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_BIN_PYRIGHT="$TMP/no-such-pyright" \
  TAKO_LSP_FAKE_FORMAT="$RULES" \
  TAKO_LSP_FAKE_LOG="$LOG" \
  TAKO_LSP_FORMAT_TIMEOUT_SECS=10 \
  TAKO_LSP_FORMAT_ON_SAVE_TIMEOUT_SECS=2 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
[ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
echo "  pid=$APP_PID root=$ROOT"

echo
echo "== ① 全体の整形 → 整形される・版が 1 つ進む・undo 1 回でバイト一致 =="
printf 'fn main() {  \n    let 名前 = 1;\t\n    let x = 2;   \n}\n' > "$P/src/main.rs"
cp "$P/src/main.rs" "$TMP/main.orig"
expect "$TMP/main.want" 'fn main() {\n    let 名前 = 1;\n    let x = 2;\n}\n'
A="$(open_code "$P/src/main.rs")"
"$TAKO_BIN" edit start --pane "$A" >/dev/null
autosave_off "$A"
V0="$(version "$A")"
R1="$(fmt --pane "$A")"
check_eq "status" "formatted" "$(printf '%s' "$R1" | json 'd["status"]')"
check_eq "書き換えた箇所" "3" "$(printf '%s' "$R1" | json 'd["changes"]')"
check_eq "版は 1 つだけ進む" "$((V0 + 1))" "$(printf '%s' "$R1" | json 'd["document"]["version"]')"
check_eq "保存はしない（dirty のまま）" "True" "$(printf '%s' "$R1" | json 'd["dirty"]')"
save --pane "$A" >/dev/null
if same_file "$P/src/main.rs" "$TMP/main.want"; then pass "保存したファイルは整形後の固定値"; else fail "整形後: $(od -c "$P/src/main.rs" | head -5)"; fi
"$TAKO_BIN" edit undo --pane "$A" >/dev/null
save --pane "$A" >/dev/null
if same_file "$P/src/main.rs" "$TMP/main.orig"; then pass "undo 1 回で整形の前とバイト一致"; else fail "undo 1 回で戻らない"; fi
OPTS="$(python3 -c 'import json,sys
for l in open(sys.argv[1]):
    m = json.loads(l)
    if m.get("method") == "textDocument/formatting":
        print(json.dumps(m["params"]["options"], sort_keys=True)); break' "$LOG")"
check_eq "FormattingOptions はファイルの字下げから" '{"insertSpaces": true, "tabSize": 4}' "$OPTS"

echo
echo "== ② 範囲の整形は範囲の外を 1 バイトも変えない（範囲は UTF-16 で送る） =="
printf 'let a = 1;  \nlet 😀名 = 2;  \nlet c = 3;  \nlet d = 4;  \n' > "$P/src/range.rs"
cp "$P/src/range.rs" "$TMP/range.orig"
expect "$TMP/range.want" 'let a = 1;  \nlet 😀名 = 2;\nlet c = 3;\nlet d = 4;  \n'
B="$(open_code "$P/src/range.rs")"
"$TAKO_BIN" edit start --pane "$B" >/dev/null
autosave_off "$B"
R2="$(fmt --pane "$B" --range 2:0-4:0)"
check_eq "status" "formatted" "$(printf '%s' "$R2" | json 'd["status"]')"
check_eq "応答の範囲" "2:0-4:0" "$(printf '%s' "$R2" | json '"%d:%d-%d:%d" % (d["range"]["start_line"], d["range"]["start_col"], d["range"]["end_line"], d["range"]["end_col"])')"
save --pane "$B" >/dev/null
if same_file "$P/src/range.rs" "$TMP/range.want"; then pass "範囲の中だけ整形され、外（1 行目・4 行目の行末の空白）は残る"; else fail "範囲の整形: $(cat "$P/src/range.rs")"; fi
SENT="$(python3 -c 'import json,sys
for l in open(sys.argv[1]):
    m = json.loads(l)
    if m.get("method") == "textDocument/rangeFormatting":
        r = m["params"]["range"]; print(r["start"]["line"], r["start"]["character"], r["end"]["line"], r["end"]["character"]); break' "$LOG")"
check_eq "送った範囲（LSP の 0 起点の行）" "1 0 3 0" "$SENT"
BAD="$("$TAKO_BIN" lsp format --pane "$B" --range 2:5-3:0 2>&1)"
case "$BAD" in *"文字の途中"*) pass "文字の途中の範囲は丸めずに拒否" ;; *) fail "文字の途中の範囲: $BAD" ;; esac

echo
echo "== ③ CLI と MCP が同じ答え =="
printf 'fn a() {  \n}\n' > "$P/src/mcp.rs"
C="$(open_code "$P/src/mcp.rs")"
"$TAKO_BIN" edit start --pane "$C" >/dev/null
autosave_off "$C"
RC="$(fmt --pane "$C")"
"$TAKO_BIN" edit undo --pane "$C" >/dev/null
RM="$(mcp tako_lsp "{\"action\":\"format\",\"pane\":$C}")"
pick() { json '[d["status"], d["changes"], d["server"], d["note"], d["document"]["undo_depth"]]'; }
check_eq "CLI と MCP の status / changes / server / note / undo の深さ" "$(printf '%s' "$RC" | pick)" "$(printf '%s' "$RM" | pick)"
RMR="$(mcp tako_lsp "{\"action\":\"format\",\"pane\":$C,\"line\":1,\"column\":0}")"
case "$RMR" in *"end_line"*) pass "MCP の範囲は 4 つそろえて（一部だけは拒否）" ;; *) fail "MCP の範囲の一部だけ: $RMR" ;; esac

echo
echo "== ④ GUI の入口（編集メニュー「コードを整形」= ⇧⌘I と同じアクション） =="
printf 'fn gui() {   \n}\n' > "$P/src/gui.rs"
expect "$TMP/gui.want" 'fn gui() {\n}\n'
G="$(open_code "$P/src/gui.rs")"
"$TAKO_BIN" edit start --pane "$G" >/dev/null
autosave_off "$G"
"$TAKO_BIN" focus --pane "$G" >/dev/null 2>&1
FMT_MENU="$(menu_path tako::FormatDocument)"
echo "    メニュー: $FMT_MENU"
"$TAKO_BIN" menu invoke "$FMT_MENU" >/dev/null 2>&1
DONE=""
for _ in $(seq 1 50); do
  [ "$("$TAKO_BIN" edit status --pane "$G" 2>/dev/null | json 'd["dirty"]')" = "True" ] && { DONE=1; break; }
  sleep 0.1
done
check_eq "メニューの整形で本文が変わった" "1" "${DONE:-0}"
save --pane "$G" >/dev/null
if same_file "$P/src/gui.rs" "$TMP/gui.want"; then pass "メニューの整形の結果は CLI と同じ固定値"; else fail "メニューの整形: $(cat "$P/src/gui.rs")"; fi
check_eq "メニューの整形も undo 1 件" "1" "$("$TAKO_BIN" edit status --pane "$G" 2>/dev/null | json 'd["document"]["undo_depth"]')"

echo
echo "== ⑤ 保存時整形（既定 off → on）。明示的な保存だけ整形し、自動保存は整形しない =="
check_eq "既定は off" "False" "$("$TAKO_BIN" lsp format-on-save 2>/dev/null >/dev/null; mcp tako_lsp '{"action":"format-on-save"}' | json 'd["enabled"]')"
printf 'fn s() {\n}\n' > "$P/src/onsave.rs"
S="$(open_code "$P/src/onsave.rs")"
"$TAKO_BIN" edit start --pane "$S" >/dev/null
autosave_off "$S"
"$TAKO_BIN" edit replace-range 1:8 1:8 "  " --pane "$S" >/dev/null
OFF="$(save --pane "$S")"
if grep -q '{  $' "$P/src/onsave.rs"; then pass "off の保存は整形しない（行末の空白が残る）"; else fail "off なのに整形された: $(cat "$P/src/onsave.rs")"; fi
"$TAKO_BIN" lsp format-on-save on >/dev/null
check_eq "on にした値" "True" "$(mcp tako_lsp '{"action":"format-on-save"}' | json 'd["enabled"]')"
"$TAKO_BIN" edit replace-range 2:0 2:0 $'// x   \n' --pane "$S" >/dev/null
ON="$("$TAKO_BIN" edit save --pane "$S" 2>/dev/null)"
expect "$TMP/onsave.want" 'fn s() {\n// x\n}\n'
if same_file "$P/src/onsave.rs" "$TMP/onsave.want"; then pass "tako edit save は整形してから保存"; else fail "保存時整形: $(cat "$P/src/onsave.rs")"; fi
# `tako edit save` の表示は人向けなので、MCP で応答の形を見る
"$TAKO_BIN" edit replace-range 1:8 1:8 "  " --pane "$S" >/dev/null
MS="$(mcp tako_preview_save "{\"pane\":$S}")"
check_eq "MCP の保存も整形を挟む（format.status）" "formatted" "$(printf '%s' "$MS" | json 'd["format"]["status"]')"
check_eq "MCP の保存は saved" "True" "$(printf '%s' "$MS" | json 'd["saved"]')"
# メニューの保存（⌘S と同じアクション）
SAVE_MENU="$(menu_path tako::SavePreview)"
"$TAKO_BIN" edit replace-range 1:8 1:8 "   " --pane "$S" >/dev/null
"$TAKO_BIN" focus --pane "$S" >/dev/null 2>&1
"$TAKO_BIN" menu invoke "$SAVE_MENU" >/dev/null 2>&1
SAVED=""
for _ in $(seq 1 50); do
  same_file "$P/src/onsave.rs" "$TMP/onsave.want" && [ "$("$TAKO_BIN" edit status --pane "$S" 2>/dev/null | json 'd["dirty"]')" = "False" ] && { SAVED=1; break; }
  sleep 0.1
done
check_eq "メニューの保存（⌘S）も整形してから保存" "1" "${SAVED:-0}"
# 自動保存は整形しない（on のままでも）
"$TAKO_BIN" edit autosave true --pane "$S" >/dev/null
"$TAKO_BIN" edit replace-range 1:8 1:8 "    " --pane "$S" >/dev/null
AUTO=""
for _ in $(seq 1 40); do
  [ "$("$TAKO_BIN" edit status --pane "$S" 2>/dev/null | json 'd["dirty"]')" = "False" ] && { AUTO=1; break; }
  sleep 0.1
done
check_eq "自動保存が走った" "1" "${AUTO:-0}"
if grep -q '{    $' "$P/src/onsave.rs"; then pass "自動保存は整形しない（行末の空白がそのまま保存された）"; else fail "自動保存で整形された: $(cat "$P/src/onsave.rs")"; fi
autosave_off "$S"
# 保存時整形の未応答: 上限（2 秒）で整形をやめ、保存は止まらない
printf 'fn q() {  \n}\n' > "$P/src/silent.rs"
Q="$(open_code "$P/src/silent.rs")"
"$TAKO_BIN" edit start --pane "$Q" >/dev/null
autosave_off "$Q"
"$TAKO_BIN" edit replace-range 2:0 2:0 $'// y\n' --pane "$Q" >/dev/null
T0="$(python3 -c 'import time; print(time.time())')"
MQ="$(mcp tako_preview_save "{\"pane\":$Q}")"
T1="$(python3 -c 'import time; print(time.time())')"
check_eq "未応答でも保存する" "True" "$(printf '%s' "$MQ" | json 'd["saved"]')"
check_eq "整形の status" "timeout" "$(printf '%s' "$MQ" | json 'd["format"]["status"]')"
echo "    note: $(printf '%s' "$MQ" | json 'd["format"]["note"]')"
if grep -q '// y' "$P/src/silent.rs"; then pass "整形せずに保存された（編集はディスクにある）"; else fail "保存されていない"; fi
WAITED="$(python3 -c "print(int(($T1 - $T0) * 1000))")"
if [ "$WAITED" -lt 6000 ]; then pass "保存は上限の近くで返る（${WAITED}ms）"; else fail "保存が ${WAITED}ms 待たされた"; fi
"$TAKO_BIN" lsp format-on-save off >/dev/null

echo
echo "== ⑥ エッジ =="
# サーバの無い種類: 編集モードへ入らない
printf 'a  \n' > "$P/notes.txt"
N="$(open_code "$P/notes.txt")"
RN="$(fmt --pane "$N")"
check_eq "サーバの無い種類は no-server" "no-server" "$(printf '%s' "$RN" | json 'd["status"]')"
check_eq "編集モードへ入らない" "False" "$("$TAKO_BIN" edit status --pane "$N" 2>/dev/null | json 'd["editing"]')"
# 未導入（pyright）
printf 'x = 1  \n' > "$P/tool.py"
Y="$(open_code "$P/tool.py")"
RY="$(fmt --pane "$Y")"
check_eq "未導入は not-installed" "not-installed" "$(printf '%s' "$RY" | json 'd["status"]')"
check_eq "導入コマンドを返す" "npm install -g pyright" "$(printf '%s' "$RY" | json 'd["install_command"]')"
# CRLF
printf 'fn c() {  \r\n    x();  \r\n}\r\n' > "$P/src/crlf.rs"
printf 'fn c() {\r\n    x();\r\n}\r\n' > "$TMP/crlf.want"
W="$(open_code "$P/src/crlf.rs")"
"$TAKO_BIN" edit start --pane "$W" >/dev/null
autosave_off "$W"
check_eq "CRLF の整形" "formatted" "$(fmt --pane "$W" | json 'd["status"]')"
save --pane "$W" >/dev/null
if same_file "$P/src/crlf.rs" "$TMP/crlf.want"; then pass "CRLF は CRLF のまま（行末の空白だけ消える）"; else fail "CRLF: $(od -c "$P/src/crlf.rs" | head -3)"; fi
# 待つあいだの変更（stale）: 偽サーバは slow.rs に 1.5 秒待ってから答える
printf 'fn w() {  \n}\n' > "$P/src/slow.rs"
L="$(open_code "$P/src/slow.rs")"
"$TAKO_BIN" edit start --pane "$L" >/dev/null
autosave_off "$L"
( fmt --pane "$L" > "$TMP/slow.json" ) &
WAITER=$!
sleep 0.5
"$TAKO_BIN" edit replace-range 2:0 2:0 $'// typed\n' --pane "$L" >/dev/null
wait "$WAITER"
check_eq "待つあいだに打ったら当てない" "stale" "$(json 'd["status"]' < "$TMP/slow.json")"
save --pane "$L" >/dev/null
printf 'fn w() {  \n// typed\n}\n' > "$TMP/slow.want"
if same_file "$P/src/slow.rs" "$TMP/slow.want"; then pass "本文は打った編集だけ（整形は混ざらない）"; else fail "stale のあと: $(cat "$P/src/slow.rs")"; fi
# 外部変更の競合中（#1659）: 整形はバッファへ当たり、保存は競合で断られる（保存時整形も挟まない）
printf 'fn e() {  \n}\n' > "$P/src/ext.rs"
E="$(open_code "$P/src/ext.rs")"
"$TAKO_BIN" edit start --pane "$E" >/dev/null
autosave_off "$E"
# 未編集のバッファは外の変更へ追従する（#1659）ので、先に自分の編集を入れて競合にする
"$TAKO_BIN" edit replace-range 2:0 2:0 $'// mine\n' --pane "$E" >/dev/null
sleep 0.3
printf 'fn e() {}\n// outside\n' > "$P/src/ext.rs"
sleep 1
check_eq "競合中でも整形はバッファへ当たる" "formatted" "$(fmt --pane "$E" | json 'd["status"]')"
"$TAKO_BIN" lsp format-on-save on >/dev/null
EXT="$(save --pane "$E")"
case "$EXT" in *"--force"*) pass "保存は競合で断られ、抜け方を返す" ;; *) fail "競合中の保存: $EXT" ;; esac
check_eq "外のファイルは書き換わっていない" "// outside" "$(tail -1 "$P/src/ext.rs")"
"$TAKO_BIN" lsp format-on-save off >/dev/null
check_eq "設定はセルフテスト外なので settings.json へ書かれた" "False" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("lsp_format_on_save"))' "$TAKO_DATA_DIR/settings.json")"

stop_isolated_gui "$APP_PID"; APP_PID=""

echo
echo "== ⑦ 実サーバ（rust-analyzer + rustfmt） =="
if [ -n "${TAKO_1683_SKIP_REAL:-}" ]; then
  echo "  未実測: TAKO_1683_SKIP_REAL=1"
elif [ -z "$REAL_RA" ] || [ -z "$REAL_RUSTFMT" ]; then
  echo "  未実測: rust-analyzer か rustfmt が PATH に無い"
else
  R="$TMP/real"
  mkdir -p "$R/src"
  printf '[package]\nname = "real"\nversion = "0.1.0"\nedition = "2021"\n' > "$R/Cargo.toml"
  printf 'fn main(){let x=vec![1,2,3];for i in x.iter(){println!("{}",i);}\n    let  名前 =  "日本語";println!("{}",名前);}\n\nfn   other( a:i32 ,b :i32)->i32{a+b}\n' > "$R/src/main.rs"
  cp "$R/src/main.rs" "$TMP/real.orig"
  cp "$R/src/main.rs" "$TMP/real.want"
  RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" "$REAL_RUSTFMT" --edition 2021 "$TMP/real.want" || echo "  rustfmt が期待値を作れない"
  printf 'fn broken( {\n    let x =  1;\n' > "$R/src/broken.rs"
  launch_isolated_gui "$TMP/app-real.log" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
    TAKO_LSP_FORMAT_TIMEOUT_SECS=120 || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-real.log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  RP="$(open_code "$R/src/main.rs")"
  "$TAKO_BIN" edit start --pane "$RP" >/dev/null
  autosave_off "$RP"
  T0="$(python3 -c 'import time; print(time.time())')"
  RR="$(fmt --pane "$RP")"
  T1="$(python3 -c 'import time; print(time.time())')"
  echo "    1 回目（起動と握手を含む）: $(python3 -c "print(int(($T1 - $T0) * 1000))")ms status=$(printf '%s' "$RR" | json 'd["status"]') server=$(printf '%s' "$RR" | json 'd["server"]') changes=$(printf '%s' "$RR" | json 'd["changes"]')"
  check_eq "実サーバの整形" "formatted" "$(printf '%s' "$RR" | json 'd["status"]')"
  save --pane "$RP" >/dev/null
  if same_file "$R/src/main.rs" "$TMP/real.want"; then pass "実ファイルが rustfmt の出力とバイト一致"; else fail "rustfmt と違う: $(diff "$TMP/real.want" "$R/src/main.rs" | head -10)"; fi
  "$TAKO_BIN" edit undo --pane "$RP" >/dev/null
  save --pane "$RP" >/dev/null
  if same_file "$R/src/main.rs" "$TMP/real.orig"; then pass "undo 1 回で整形の前とバイト一致（実サーバ）"; else fail "実サーバで undo 1 回で戻らない"; fi
  # 2 回目（稼働中）と GUI の入口
  "$TAKO_BIN" focus --pane "$RP" >/dev/null 2>&1
  "$TAKO_BIN" menu invoke "$(menu_path tako::FormatDocument)" >/dev/null 2>&1
  DONE=""
  for _ in $(seq 1 100); do
    [ "$("$TAKO_BIN" edit status --pane "$RP" 2>/dev/null | json 'd["dirty"]')" = "True" ] && { DONE=1; break; }
    sleep 0.1
  done
  check_eq "メニューの整形（実サーバ）で本文が変わった" "1" "${DONE:-0}"
  check_eq "メニューの整形も undo 1 件" "1" "$("$TAKO_BIN" edit status --pane "$RP" 2>/dev/null | json 'd["document"]["undo_depth"]')"
  save --pane "$RP" >/dev/null
  if same_file "$R/src/main.rs" "$TMP/real.want"; then pass "メニューの整形も rustfmt とバイト一致"; else fail "メニューの整形（実サーバ）: $(diff "$TMP/real.want" "$R/src/main.rs" | head -5)"; fi
  RANGE="$(fmt --pane "$RP" --range 1:0-2:0)"
  echo "    範囲の整形: status=$(printf '%s' "$RANGE" | json 'd["status"]') reason=$(printf '%s' "$RANGE" | json 'd.get("reason")')"
  case "$(printf '%s' "$RANGE" | json 'd["status"]')" in
    unsupported|formatted|unchanged) pass "範囲の整形は能力に応じて答える" ;;
    *) fail "範囲の整形（実サーバ）: $RANGE" ;;
  esac
  BP="$(open_code "$R/src/broken.rs")"
  "$TAKO_BIN" edit start --pane "$BP" >/dev/null
  autosave_off "$BP"
  BR="$(fmt --pane "$BP")"
  echo "    構文エラー: status=$(printf '%s' "$BR" | json 'd["status"]') note=$(printf '%s' "$BR" | json 'd.get("note")') reason=$(printf '%s' "$BR" | json 'd.get("reason")')"
  case "$(printf '%s' "$BR" | json 'd["status"]')" in
    unchanged|server-error) pass "構文エラーのファイルは本文を変えずに理由を返す" ;;
    *) fail "構文エラー: $BR" ;;
  esac
  check_eq "構文エラーのファイルは変わらない" "False" "$("$TAKO_BIN" edit status --pane "$BP" 2>/dev/null | json 'd["dirty"]')"
  stop_isolated_gui "$APP_PID"; APP_PID=""
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
