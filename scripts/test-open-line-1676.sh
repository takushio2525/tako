#!/usr/bin/env bash
# test-open-line-1676.sh — `tako open --line L [--column C]` の実経路テスト（#1676 / FR-3.27）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#
#   ① CLI `tako open <file> --line 42` が着地点（line / column / item / total_lines /
#      clamped）を返し、表示種別が code になる
#   ② MCP `tako_open_file(path, pane, line)` が**同じ dispatch** を通り、
#      応答が CLI と**字面まで一致**する（ツールは増やしていない）
#   ③ 丸め: 行数超過は末尾行へ / 行末を超える桁は行末の次へ（どちらも clamped=true）
#   ④ 境界: `--line 0` / `--line -1` / `--column` 単独はエラー（**開く前**に落ちる）
#   ⑤ md の写像: `--line` を渡すと markdown のレンダリングをやめて原文の行へ着地する
#      （行を渡さなければ従来どおり markdown）
#   ⑥ 行を持たない種別（画像）は `--line` でエラーになり、**ペインが増えない**
#   ⑦ エッジ: 空ファイル / 1 行ファイル / 存在しないファイル
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-open-line-1676.sh
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

TMP="$(mktemp -d /tmp/tako-1676-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1676-$$"
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
FIX="$TMP/fixtures"
mkdir -p "$FIX"
python3 - "$FIX" <<'PY'
import pathlib, sys
fix = pathlib.Path(sys.argv[1])
(fix / "big.rs").write_text("".join(f"// 行 {i} :: MARK_1676_{i}\n" for i in range(1, 5001)), encoding="utf-8")
(fix / "one.rs").write_text("only\n", encoding="utf-8")
(fix / "empty.rs").write_text("", encoding="utf-8")
(fix / "doc.md").write_text("# 見出し\n\n段落\n\n## 次\n\n本文\n", encoding="utf-8")
(fix / "shot.png").write_bytes(bytes([0x89]) + b"PNG\r\n\x1a\n")
PY

# 応答 JSON から見たい項目だけを 1 行 1 項目で出す
PY_OPEN='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
for k in ("pane", "mode", "created", "line", "column", "item", "total_lines", "clamped"):
    print("%s=%s" % (k, d.get(k)))'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }
oneline() { printf '%s' "$1" | tr '\n' ' '; }

echo "== 隔離 GUI を起こす =="
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

# 以降の差し替え先にする専用プレビューペインを 1 枚生やす。
# **tako の外から撃つので基準ペインを名指しする**（呼び出し元ペインが無い）
ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
[ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
FIRST_OPEN="$("$TAKO_BIN" open "$FIX/big.rs" --pane "$ROOT" --right 2>&1)"
PREVIEW="$(printf '%s' "$FIRST_OPEN" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
[ -n "$PREVIEW" ] || { echo "検証用プレビューペインを作れない: $FIRST_OPEN"; exit 1; }
echo "  preview pane=$PREVIEW"

open_line() { # open_line <file> [引数…] → JSON を標準出力
  local file="$1"; shift
  "$TAKO_BIN" open "$file" --pane "$PREVIEW" ${1+"$@"} 2>&1
}

echo
echo "== ① CLI で行を指定して開く =="
OUT_A="$(open_line "$FIX/big.rs" --line 42 | python3 -c "$PY_OPEN")"
echo "  $(oneline "$OUT_A")"
check_eq "mode は code へ倒れる" "code" "$(field mode "$OUT_A")"
check_eq "着地する行は 42" "42" "$(field line "$OUT_A")"
check_eq "item は 0 始まりで 41" "41" "$(field item "$OUT_A")"
check_eq "文書の行数を返す" "5000" "$(field total_lines "$OUT_A")"
check_eq "丸めは起きていない" "False" "$(field clamped "$OUT_A")"
check_eq "桁は渡していないので null" "None" "$(field column "$OUT_A")"

echo
echo "== ② MCP tako_open_file(line=42) が CLI と字面まで一致する =="
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1676","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_open_file","arguments":{"path":"$FIX/big.rs","pane":$PREVIEW,"line":42}}}
JSONL
# `mcp serve` はツール公開の判定を **env だけ**で行う（tako の外で 0 ツール = FR-2.3.2）。
# ペインの中からなら env が入るが、ここはスクリプトから撃つので接続情報ファイルから渡す。
# **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
  "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
PY_MCP='import json,sys
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
print("__NO_RESPONSE__")'
MCP_TEXT="$(python3 -c "$PY_MCP" "$TMP/mcp-out.jsonl")"
# CLI をもう一度同じ引数で叩き、**同じペイン・同じ引数の応答**を比べる
CLI_TEXT="$(open_line "$FIX/big.rs" --line 42)"
NORM='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True, ensure_ascii=False))'
CLI_NORM="$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM" 2>/dev/null)"
MCP_NORM="$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM" 2>/dev/null)"
echo "  CLI: $CLI_NORM"
echo "  MCP: $MCP_NORM"
[ -n "$MCP_NORM" ] || {
  echo "  (MCP の生応答) $MCP_TEXT"
  echo "  (MCP の stderr) $(tr '\n' ' ' < "$TMP/mcp-err.log")"
}
check_eq "CLI と MCP の応答が字面まで一致する" "$CLI_NORM" "$MCP_NORM"

echo
echo "== ③ 丸め =="
OUT_C="$(open_line "$FIX/big.rs" --line 999999 | python3 -c "$PY_OPEN")"
echo "  超過: $(oneline "$OUT_C")"
check_eq "行数超過は末尾行へ丸める" "5000" "$(field line "$OUT_C")"
check_eq "丸めたことを知らせる" "True" "$(field clamped "$OUT_C")"
OUT_COL="$(open_line "$FIX/big.rs" --line 1 --column 999 | python3 -c "$PY_OPEN")"
echo "  桁超過: $(oneline "$OUT_COL")"
# `// 行 1 :: MARK_1676_1` = 21 文字 → 行末の次は 22
check_eq "行末を超える桁は行末の次へ丸める" "22" "$(field column "$OUT_COL")"
check_eq "桁の丸めも clamped に出る" "True" "$(field clamped "$OUT_COL")"

echo
echo "== ④ 境界（0 / 負値 / 桁だけ） =="
ERR_ZERO="$(open_line "$FIX/big.rs" --line 0)"
check_contains "--line 0 はエラー" "行番号は 1 始まり" "$ERR_ZERO"
ERR_NEG="$(open_line "$FIX/big.rs" --line -1)"
check_contains "--line -1 はエラー" "行番号は 1 始まり" "$ERR_NEG"
ERR_COL0="$(open_line "$FIX/big.rs" --line 1 --column 0)"
check_contains "--column 0 はエラー" "桁番号は 1 始まり" "$ERR_COL0"
ERR_ONLY="$(open_line "$FIX/big.rs" --column 3)"
check_contains "--column 単独はエラー" "line" "$ERR_ONLY"

echo
echo "== ⑤ md の写像 =="
OUT_MD="$(open_line "$FIX/doc.md" | python3 -c "$PY_OPEN")"
check_eq "行を渡さなければ markdown のまま" "markdown" "$(field mode "$OUT_MD")"
OUT_MDL="$(open_line "$FIX/doc.md" --line 5 | python3 -c "$PY_OPEN")"
echo "  $(oneline "$OUT_MDL")"
check_eq "行を渡すとソース表示へ倒れる" "code" "$(field mode "$OUT_MDL")"
check_eq "原文 5 行目 = item 4（ブロック番号ではない）" "4" "$(field item "$OUT_MDL")"

echo
echo "== ⑥ 行を持たない種別 =="
PANES_BEFORE="$("$TAKO_BIN" list 2>/dev/null | grep -c '"id"')"
ERR_PNG="$(open_line "$FIX/shot.png" --line 1)"
PANES_AFTER="$("$TAKO_BIN" list 2>/dev/null | grep -c '"id"')"
check_contains "画像に --line はエラー" "テキスト" "$ERR_PNG"
check_eq "落ちてもペインは増えない" "$PANES_BEFORE" "$PANES_AFTER"

echo
echo "== ⑦ エッジ =="
OUT_EMPTY="$(open_line "$FIX/empty.rs" --line 1 | python3 -c "$PY_OPEN")"
echo "  空ファイル: $(oneline "$OUT_EMPTY")"
check_eq "空ファイルでも行 1 は範囲内" "1" "$(field line "$OUT_EMPTY")"
check_eq "空ファイルの行数は 0" "0" "$(field total_lines "$OUT_EMPTY")"
check_eq "空ファイルの行 1 は丸めではない" "False" "$(field clamped "$OUT_EMPTY")"
OUT_ONE="$(open_line "$FIX/one.rs" --line 9 | python3 -c "$PY_OPEN")"
echo "  1 行ファイル: $(oneline "$OUT_ONE")"
check_eq "1 行ファイルは行 1 へ丸める" "1" "$(field line "$OUT_ONE")"
check_eq "丸めたことを知らせる" "True" "$(field clamped "$OUT_ONE")"
ERR_MISSING="$(open_line "$FIX/nope.rs" --line 1)"
check_contains "存在しないファイルはエラー" "ファイルを開けない" "$ERR_MISSING"

echo
echo "== 結果: PASS=$PASS FAIL=$FAIL =="
[ "$FAIL" -eq 0 ]
