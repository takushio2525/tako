#!/usr/bin/env bash
# test-jump-1677.sh — ジャンプ履歴（戻る / 進む）の実経路テスト（#1677 / FR-3.29）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#
#   ① 行を指定した `tako open` が「開く前にいた場所」と着地点を積む
#      （行なしの open は積まない / 同じ場所は畳む）
#   ② `tako jump back` / `forward` でプレビューの中身と着地行が変わる（ペインは増えない）
#   ③ MCP `tako_jump` が**同じ dispatch** を通り、応答が CLI と字面まで一致する
#      （list と back の両方。back は同じ位置から撃ち直して比べる）
#   ④ 閉じたペインの項目は開き直す（reopened=true・閉じる前は pane_alive=false）
#   ⑤ 消えたファイルの項目は読み飛ばして捨てる（dropped）
#   ⑥ エッジ: 空の履歴で back / forward（moved=false・終了コード 0）/ 上限 100 件で古い側が落ちる
#   ⑦ A/B: `TAKO_1677_LEGACY=1` の GUI では行指定の open が何も積まない（= この検査が
#      #1677 以前を落とせる）
#
# を実測する。GUI のキー（⌃- / ⌃⇧-）は AX を使わずに押せないので、ここでは測らない
# （`TAKO_VISUAL_ONLY=jump-keys` の visual-test 節が GPUI の打鍵経路で測る）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-jump-1677.sh
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
check_contains() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'${2}' を含まない: ${3}）" ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1677-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1677-$$"
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

# --- 材料（300 行のファイルを 3 本。着地行が画面の先頭に来る余裕を持たせる） ----
FIX="$TMP/fixtures"
mkdir -p "$FIX"
for f in a b c cap; do
  i=1
  : > "$FIX/$f.rs"
  while [ "$i" -le 300 ]; do
    echo "// $f $i" >> "$FIX/$f.rs"
    i=$((i + 1))
  done
done

# JSON から 1 つの値を取る（`py_get '<式>'`。d = 応答）
py_get() {
  python3 -c "import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print('__NOT_JSON__'); sys.exit(0)
v = $1
print(v if not isinstance(v, bool) else str(v).lower())"
}
# 履歴を「ファイル名:行」で並べる（現在位置に >）
PY_ENTRIES='import json,os,sys
d = json.load(sys.stdin)
cur = d["history"]["current"]
out = []
for i, e in enumerate(d["entries"]):
    mark = ">" if cur == i else ""
    out.append("%s%s:%s" % (mark, os.path.basename(e["path"]), e["line"]))
print(" ".join(out))'
entries() { "$TAKO_BIN" jump list --json 2>&1 | python3 -c "$PY_ENTRIES" 2>/dev/null; }
pane_count() { "$TAKO_BIN" list 2>/dev/null | grep -c '"id"'; }
preview_path() { # preview_path <pane> → そのペインのプレビューのファイル名
  "$TAKO_BIN" list 2>/dev/null | python3 -c "import json,os,sys
d = json.load(sys.stdin)
for t in d['tabs']:
    for p in t['panes']:
        if p['id'] == $1 and p.get('preview'):
            print(os.path.basename(p['preview']['path']))"
}
NORM='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True, ensure_ascii=False))'

start_gui() { # start_gui <log> [VAR=VAL…]
  launch_isolated_gui "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$1" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
  echo "  pid=$APP_PID root=$ROOT"
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log"

echo
echo "== ⑥a 空の履歴 =="
OUT="$("$TAKO_BIN" jump back --pane "$ROOT" 2>&1)"; RC=$?
echo "  back: $OUT (rc=$RC)"
check_eq "空の履歴で back しても終了コード 0" "0" "$RC"
check_contains "空の履歴で back は「前の履歴は無い」" "これより前の履歴は無い" "$OUT"
OUT="$("$TAKO_BIN" jump forward --pane "$ROOT" 2>&1)"; RC=$?
check_eq "空の履歴で forward しても終了コード 0" "0" "$RC"
check_contains "空の履歴で forward は「先の履歴は無い」" "これより先の履歴は無い" "$OUT"
check_contains "空の list は積み方を案内する" "tako open <file> --line" "$("$TAKO_BIN" jump list 2>&1)"

echo
echo "== ① 行を指定した open が積む =="
# 基準ペインを名指しする（tako の外から撃つので呼び出し元ペインが無い）
PREVIEW="$("$TAKO_BIN" open "$FIX/a.rs" --pane "$ROOT" --right 2>/dev/null | py_get 'd["pane"]')"
echo "  preview pane=$PREVIEW"
check_eq "行なしの open は積まない" "" "$(entries)"
"$TAKO_BIN" open "$FIX/a.rs" --pane "$ROOT" --line 120 >/dev/null 2>&1
"$TAKO_BIN" open "$FIX/b.rs" --pane "$ROOT" --line 40 >/dev/null 2>&1
# a.rs は行なしで開いて先頭（1 行目）を見ていた → 120 へ飛ぶ → b.rs:40 へ飛ぶ。
# 2 回目の「飛ぶ前の場所」a:120 は直前の着地点と同じなので畳まれる
check_eq "開く前の場所と着地点が積まれ、同じ場所は畳まれる" "a.rs:1 a.rs:120 >b.rs:40" "$(entries)"
PANES_BEFORE="$(pane_count)"

echo
echo "== ② CLI で戻る / 進む =="
OUT="$("$TAKO_BIN" jump back --pane "$ROOT" 2>&1)"
echo "  back: $OUT"
check_contains "back は a.rs:120 へ着地する" "a.rs:120" "$OUT"
check_eq "プレビューの中身が a.rs に替わる" "a.rs" "$(preview_path "$PREVIEW")"
check_eq "戻っても履歴は積まない（位置だけ動く）" "a.rs:1 >a.rs:120 b.rs:40" "$(entries)"
OUT="$("$TAKO_BIN" jump forward --pane "$ROOT" 2>&1)"
check_contains "forward は b.rs:40 へ着地する" "b.rs:40" "$OUT"
check_eq "プレビューの中身が b.rs に戻る" "b.rs" "$(preview_path "$PREVIEW")"
check_eq "戻る / 進むでペインは増えない" "$PANES_BEFORE" "$(pane_count)"

echo
echo "== ③ MCP tako_jump と CLI の応答が一致する =="
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp_call() { # mcp_call <arguments JSON> → ツールの応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1677","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_jump\",\"arguments\":$1}}" \
    > "$TMP/mcp-in.jsonl"
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  python3 -c 'import json,sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
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
CLI_LIST="$("$TAKO_BIN" jump list --json 2>&1 | python3 -c "$NORM" 2>/dev/null)"
MCP_LIST="$(mcp_call '{"action":"list"}' | python3 -c "$NORM" 2>/dev/null)"
[ -n "$MCP_LIST" ] || echo "  (MCP の stderr) $(tr '\n' ' ' < "$TMP/mcp-err.log")"
check_eq "list: CLI と MCP の応答が字面まで一致する" "$CLI_LIST" "$MCP_LIST"
# back は状態を動かすので、同じ位置（b.rs:40）から 1 回ずつ撃って応答を比べる
CLI_BACK="$("$TAKO_BIN" jump back --pane "$ROOT" --json 2>&1 | python3 -c "$NORM" 2>/dev/null)"
"$TAKO_BIN" jump forward --pane "$ROOT" >/dev/null 2>&1
MCP_BACK="$(mcp_call "{\"action\":\"back\",\"pane\":$ROOT}" | python3 -c "$NORM" 2>/dev/null)"
echo "  CLI back: $CLI_BACK"
echo "  MCP back: $MCP_BACK"
check_eq "back: CLI と MCP の応答が字面まで一致する" "$CLI_BACK" "$MCP_BACK"
check_eq "back は moved=true" "true" "$(printf '%s' "$MCP_BACK" | py_get 'd["moved"]')"
check_eq "back の着地行は 120" "120" "$(printf '%s' "$MCP_BACK" | py_get 'd["open"]["line"]')"
"$TAKO_BIN" jump forward --pane "$ROOT" >/dev/null 2>&1

echo
echo "== ④ 閉じたペインの項目は開き直す =="
"$TAKO_BIN" close --pane "$PREVIEW" --force >/dev/null 2>&1
ALIVE="$("$TAKO_BIN" jump list --json 2>/dev/null | py_get '[e["pane_alive"] for e in d["entries"]]')"
check_eq "閉じても項目は消えず pane_alive=false になる" "[False, False, False]" "$ALIVE"
OUT="$("$TAKO_BIN" jump back --pane "$ROOT" 2>&1)"
echo "  back: $OUT"
check_contains "閉じたペインの項目へ戻ると開き直す" "開き直した" "$OUT"
REOPENED="$("$TAKO_BIN" jump list --json 2>/dev/null | py_get 'sorted(set(e["pane"] for e in d["entries"]))')"
NEWPANE="$("$TAKO_BIN" jump list --json 2>/dev/null | py_get 'd["entries"][1]["pane"]')"
check_eq "閉じたペインの項目はまとめて開き直した先へ付け替わる" "[$NEWPANE]" "$REOPENED"
check_eq "開き直した先は a.rs を表示している" "a.rs" "$(preview_path "$NEWPANE")"
OUT="$("$TAKO_BIN" jump forward --pane "$ROOT" 2>&1)"
check_contains "付け替えた後は開き直さずに進める" "b.rs:40" "$OUT"
case "$OUT" in *開き直した*) fail "付け替えた後にまた開き直している: $OUT" ;; *) pass "付け替えた後はペインを増やさない" ;; esac

echo
echo "== ⑤ 消えたファイルの項目は読み飛ばす =="
"$TAKO_BIN" open "$FIX/c.rs" --pane "$ROOT" --line 5 >/dev/null 2>&1
rm -f "$FIX/b.rs"
OUT="$("$TAKO_BIN" jump back --pane "$ROOT" 2>&1)"
echo "  back: $OUT"
check_contains "消えたファイルは読み飛ばしたと知らせる" "読み飛ばした（ファイルが無い）" "$OUT"
check_contains "その先の a.rs:120 へ着地する" "a.rs:120" "$OUT"
check_eq "消えたファイルの項目は履歴から捨てる" "a.rs:1 >a.rs:120 c.rs:5" "$(entries)"

echo
echo "== ⑥b 上限（100 件）で古い側から落ちる =="
i=1
while [ "$i" -le 105 ]; do
  "$TAKO_BIN" open "$FIX/cap.rs" --pane "$ROOT" --line "$i" >/dev/null 2>&1
  i=$((i + 1))
done
LEN="$("$TAKO_BIN" jump list --json 2>/dev/null | py_get 'd["history"]["len"]')"
FIRST="$("$TAKO_BIN" jump list --json 2>/dev/null | py_get 'd["entries"][0]["line"]')"
LAST="$("$TAKO_BIN" jump list --json 2>/dev/null | py_get 'd["entries"][-1]["line"]')"
check_eq "履歴は 100 件で頭打ちになる" "100" "$LEN"
check_eq "末尾は最後に飛んだ cap.rs:105" "105" "$LAST"
check_eq "古い側から落ちる（先頭は cap.rs:6）" "6" "$FIRST"

stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ⑦ A/B: TAKO_1677_LEGACY=1 では積まない =="
start_gui "$TMP/app-legacy.log" TAKO_1677_LEGACY=1
"$TAKO_BIN" open "$FIX/a.rs" --pane "$ROOT" --right --line 10 >/dev/null 2>&1
"$TAKO_BIN" open "$FIX/c.rs" --pane "$ROOT" --line 20 >/dev/null 2>&1
check_eq "LEGACY では行指定の open でも何も積まれない" "" "$(entries)"
OUT="$("$TAKO_BIN" jump back --pane "$ROOT" 2>&1)"
check_contains "LEGACY では戻れない（= ①②の検査は #1677 以前を落とせる）" "これより前の履歴は無い" "$OUT"

echo
echo "== 結果: PASS=$PASS FAIL=$FAIL =="
[ "$FAIL" -eq 0 ]
