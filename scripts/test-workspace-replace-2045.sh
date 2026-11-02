#!/bin/bash
# test-workspace-replace-2045.sh — ワークスペース横断置換（#2045）の実経路テスト
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定には触らない）:
#   ① 右パネルへ出した検索（--show）→ --replace でプレビュー（1 バイトも書かない）→ 検索の後に 1 ファイルを
#      外で書き換える → --replace --yes: 書き換わったファイルは当てずに理由（stale）を返して中身を触らず、
#      ほかは当たる（受け入れ条件 3）
#   ② 取り消し: --undo（戻せるかだけ）→ --undo --yes で開いていないファイルが退避から戻る。退避は
#      data dir の search-replace/ の下に置換前の中身で残っていた（受け入れ条件 4）
#   ③ CLI と MCP（GUI 内蔵の tako_search）が同じ答え: 同じ中身の 2 つのツリーへプレビュー → 当てる →
#      戻すをそれぞれ流し、応答（ツリーのパスと記録の番号を除く）と書き換えた後の中身が一致する（受け入れ条件 5）
#   ④ 境界: 一致 0 件 / 置換後が元と同じ / 不正な正規表現 / 無いグループ / 1 ファイルに大量一致 /
#      CRLF・BOM・Shift_JIS（文字コードを壊さない）
#   ⑤ visual-test workspace-replace-2045（置換欄 → プレビュー → 行の置換 → すべて置換 → 元に戻す。
#      受け入れ条件 6。画像は WS2045_DUMP_DIR か一時 dir の dump/）
#
# 使い方: bash scripts/test-workspace-replace-2045.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ -n "$1" ]; then pass "$2"; else fail "$2"; fi; }

TMP="$(mktemp -d /tmp/tako-2045-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERF_LOG="$TMP/perf.log"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/zdot"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
# 画像にユーザー名・ホスト名が写らないよう固定のプロンプト（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

tk() { iso_cli "$@"; }
# JSON の 1 つの値（ドット区切り。無ければ空）
jget() {
  python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(1)
for k in sys.argv[1].split("."):
    if isinstance(d, list):
        d = d[int(k)] if k.isdigit() and int(k) < len(d) else None
    else:
        d = d.get(k) if isinstance(d, dict) else None
print("" if d is None else (json.dumps(d, ensure_ascii=False) if isinstance(d, (dict, list, bool)) else d))' "$1"
}
# files[] のうち名前が NAME で終わるものの status
fstatus() {
  python3 -c '
import json, sys
d = json.load(sys.stdin)
print(next((f["status"] for f in d.get("files", []) if f["path"].endswith(sys.argv[1])), ""))' "$1"
}

# GUI 内蔵 MCP の tools/call（本文 JSON を出す。isError なら終了コード 3）
cat > "$TMP/mcp.py" <<'PY'
import json, os, sys, urllib.request
info = json.load(open(os.path.join(os.environ["TAKO_DISCOVERY_DIR"], "control.json")))
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": sys.argv[1], "arguments": json.loads(sys.argv[2])}})
req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers={
    "Authorization": "Bearer " + info["token"],
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
})
with urllib.request.urlopen(req, timeout=120) as r:
    res = json.loads(r.read())
if "error" in res:
    print(res["error"]["message"]); sys.exit(2)
print(res["result"]["content"][0]["text"])
sys.exit(3 if res["result"].get("isError") else 0)
PY
mcp() { python3 "$TMP/mcp.py" "$@"; }

# 応答を比べられる形へ（ツリーのパスを <T> へ・記録の番号と履歴の数を落とす・キーを並べる）
cat > "$TMP/norm.py" <<'PY'
import json, sys
root = sys.argv[1]
d = json.load(sys.stdin)
for k in ("undo", "history", "id", "remaining"):
    d.pop(k, None)
print(json.dumps(d, sort_keys=True, ensure_ascii=False).replace(root, "<T>"))
PY
norm() { python3 "$TMP/norm.py" "$1"; }

# 置換を試すツリー（git のリポジトリ。CRLF・BOM・Shift_JIS・バイナリを混ぜる）
make_tree() {
  local t="$1"
  mkdir -p "$t/src" "$t/.git"
  printf 'fn main() {\n    let old_2045 = 1;\n    drop(old_2045);\n}\n' > "$t/src/a.rs"
  printf 'old_2045\r\nkeep\r\nold_2045 old_2045\r\n' > "$t/src/crlf.txt"
  printf '\357\273\277old_2045 bom\n' > "$t/src/bom.txt"
  printf '\202\240 old_2045\n' > "$t/src/sjis.txt"
  printf 'old_2045\000\001' > "$t/src/bin.dat"
  printf 'nothing here\n' > "$t/src/none.txt"
}

launch_isolated_gui "$TMP/app.log" ZDOTDIR="$TMP/zdot" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || { echo "GUI が立たない"; exit 1; }
tk autorename off >/dev/null 2>&1 || true

echo "--- ① 出した検索へのプレビュー → 外で書き換え → 当てる（受け入れ条件 3）"
T="$TMP/tree"
make_tree "$T"
cp -R "$T" "$TMP/tree.orig"
tk search old_2045 --path "$T" --show --json --limit 0 > "$TMP/shown.json" 2>&1
echo "    検索: $(jget total < "$TMP/shown.json") 行・$(jget files_matched < "$TMP/shown.json") ファイル（バイナリ $(jget skipped.binary < "$TMP/shown.json")）"
tk search --replace new_2045 --json --limit 0 > "$TMP/preview.json" 2>&1
echo "    プレビュー: status=$(jget status < "$TMP/preview.json") will_replace=$(jget will_replace < "$TMP/preview.json") sjis=$(fstatus sjis.txt < "$TMP/preview.json")"
check "$([ "$(jget status < "$TMP/preview.json")" = preview ] && diff -r "$T" "$TMP/tree.orig" >/dev/null && echo 1)" \
  "プレビューは何も書かない（ツリーは元のまま）"
check "$([ "$(fstatus sjis.txt < "$TMP/preview.json")" = not-utf8 ] && echo 1)" \
  "UTF-8 でないファイルはプレビューの時点で当てないと言う"
# 人間向けの出力（→ 置換後 と 次の一手）
tk search --replace new_2045 > "$TMP/preview.txt" 2>&1
sed -n '1,4p' "$TMP/preview.txt" | sed 's/^/    | /'
check "$(grep -q '→ let new_2045 = 1;' "$TMP/preview.txt" && grep -q -- '--yes' "$TMP/preview.txt" && echo 1)" \
  "CLI の表示: 行の下に → 置換後・最後に --yes の案内"
# 検索の後に外で書き換える
printf 'fn main() {\n    let old_2045 = 1;\n    drop(old_2045);\n}\n// edited outside\n' > "$T/src/a.rs"
cp "$T/src/a.rs" "$TMP/a.outside"
tk search --replace new_2045 --yes --json --limit 0 > "$TMP/apply.json" 2>&1
A_REASON="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(next(f.get("reason","") for f in d["files"] if f["path"].endswith("a.rs")))' "$TMP/apply.json" 2>/dev/null)"
echo "    当てる: status=$(jget status < "$TMP/apply.json") replaced=$(jget replaced < "$TMP/apply.json") a.rs=$(fstatus a.rs < "$TMP/apply.json")（$A_REASON）"
check "$([ "$(fstatus a.rs < "$TMP/apply.json")" = stale ] && [ -n "$A_REASON" ] && cmp -s "$T/src/a.rs" "$TMP/a.outside" && echo 1)" \
  "検索の後に外で書き換わったファイルは当てずに理由を返し、中身を触らない"
check "$([ "$(jget status < "$TMP/apply.json")" = partial ] && echo 1)" "ほかのファイルは当たる（status=partial）"
check "$(cmp -s "$T/src/crlf.txt" <(printf 'new_2045\r\nkeep\r\nnew_2045 new_2045\r\n') && echo 1)" "CRLF のまま置き換わる"
check "$(cmp -s "$T/src/bom.txt" <(printf '\357\273\277new_2045 bom\n') && echo 1)" "BOM のまま置き換わる"
check "$(cmp -s "$T/src/sjis.txt" "$TMP/tree.orig/src/sjis.txt" && cmp -s "$T/src/bin.dat" "$TMP/tree.orig/src/bin.dat" && echo 1)" \
  "Shift_JIS とバイナリは 1 バイトも変わらない"

echo "--- ② 退避から戻す（受け入れ条件 4）"
BACKUPS="$(find "$TAKO_DATA_DIR/search-replace" -type f 2>/dev/null | wc -l | tr -d ' ')"
BK_CRLF="$(find "$TAKO_DATA_DIR/search-replace" -type f -name '*crlf.txt' 2>/dev/null | head -1)"
echo "    退避: $BACKUPS ファイル（data dir の search-replace/ の下）"
check "$([ -n "$BK_CRLF" ] && cmp -s "$BK_CRLF" "$TMP/tree.orig/src/crlf.txt" && echo 1)" \
  "書く前に置換前の中身を data dir の下へ退避していた"
tk search --undo --json > "$TMP/undo-peek.json" 2>&1
echo "    戻せるか: status=$(jget status < "$TMP/undo-peek.json") crlf=$(fstatus crlf.txt < "$TMP/undo-peek.json")"
check "$([ "$(jget status < "$TMP/undo-peek.json")" = preview ] && [ "$(fstatus crlf.txt < "$TMP/undo-peek.json")" = restorable ] && grep -q new_2045 "$T/src/crlf.txt" && echo 1)" \
  "--undo（--yes 無し）は戻せるかだけを言い、書かない"
tk search --undo --yes --json > "$TMP/undo.json" 2>&1
echo "    戻す: status=$(jget status < "$TMP/undo.json") remaining=$(jget remaining < "$TMP/undo.json")"
check "$([ "$(jget status < "$TMP/undo.json")" = restored ] && cmp -s "$T/src/crlf.txt" "$TMP/tree.orig/src/crlf.txt" && cmp -s "$T/src/bom.txt" "$TMP/tree.orig/src/bom.txt" && cmp -s "$T/src/a.rs" "$TMP/a.outside" && echo 1)" \
  "--undo --yes で当てたファイルだけが置換前へ戻り、外で書いた a.rs はそのまま"
tk search --undo > "$TMP/undo-none.txt" 2>&1
check "$(grep -q '戻す置換が無い\|no replacement to revert' "$TMP/undo-none.txt" && echo 1)" \
  "戻す置換が無ければそう言う（$(head -1 "$TMP/undo-none.txt")）"

echo "--- ③ CLI と MCP が同じ答え（受け入れ条件 5）"
C="$TMP/cli"
M="$TMP/mcpt"
make_tree "$C"
make_tree "$M"
tk search old_2045 --path "$C" --replace 'n_$0' --regex --json --limit 0 > "$TMP/c-preview.json" 2>&1
mcp tako_search "{\"action\":\"replace\",\"query\":\"old_2045\",\"path\":\"$M\",\"replacement\":\"n_\$0\",\"regex\":true,\"limit\":0}" > "$TMP/m-preview.json" 2>&1
check "$([ "$(norm "$C" < "$TMP/c-preview.json")" = "$(norm "$M" < "$TMP/m-preview.json")" ] && [ "$(jget status < "$TMP/c-preview.json")" = preview ] && echo 1)" \
  "プレビュー: CLI と MCP の応答が一致（$(jget will_replace < "$TMP/c-preview.json")）"
tk search old_2045 --path "$C" --replace 'n_$0' --regex --yes --json --limit 0 > "$TMP/c-apply.json" 2>&1
tk search --undo --json > "$TMP/c-undo-peek.json" 2>&1
CLI_AFTER="$(cat "$C/src/a.rs" "$C/src/crlf.txt" | shasum | cut -c1-12)"
tk search --undo --yes --json > "$TMP/c-undo.json" 2>&1
mcp tako_search "{\"action\":\"replace\",\"query\":\"old_2045\",\"path\":\"$M\",\"replacement\":\"n_\$0\",\"regex\":true,\"confirm\":true,\"limit\":0}" > "$TMP/m-apply.json" 2>&1
mcp tako_search '{"action":"undo"}' > "$TMP/m-undo-peek.json" 2>&1
MCP_AFTER="$(cat "$M/src/a.rs" "$M/src/crlf.txt" | shasum | cut -c1-12)"
mcp tako_search '{"action":"undo","confirm":true}' > "$TMP/m-undo.json" 2>&1
echo "    当てた後の中身: CLI $CLI_AFTER / MCP $MCP_AFTER（a.rs: $(grep -c n_old_2045 "$TMP/cli/src/a.rs" 2>/dev/null) 行は戻した後なので 0）"
check "$([ "$(norm "$C" < "$TMP/c-apply.json")" = "$(norm "$M" < "$TMP/m-apply.json")" ] && [ "$(jget status < "$TMP/c-apply.json")" = partial ] && [ "$CLI_AFTER" = "$MCP_AFTER" ] && echo 1)" \
  "当てる: CLI と MCP の応答と書き換えた後の中身が一致（status=$(jget status < "$TMP/c-apply.json")）"
check "$([ "$(norm "$C" < "$TMP/c-undo-peek.json")" = "$(norm "$M" < "$TMP/m-undo-peek.json")" ] && [ "$(norm "$C" < "$TMP/c-undo.json")" = "$(norm "$M" < "$TMP/m-undo.json")" ] && echo 1)" \
  "戻す: CLI と MCP の応答が一致（status=$(jget status < "$TMP/c-undo.json")）"
check "$(diff -r "$C" "$TMP/tree.orig" >/dev/null && diff -r "$M" "$TMP/tree.orig" >/dev/null && echo 1)" \
  "戻した後はどちらのツリーも元の中身と一致"

echo "--- ④ 境界"
E="$TMP/edge"
make_tree "$E"
tk search absent_2045 --path "$E" --replace x --yes --json > "$TMP/e-none.json" 2>&1
check "$([ "$(jget status < "$TMP/e-none.json")" = not-applied ] && [ "$(jget targets < "$TMP/e-none.json")" = 0 ] && echo 1)" "一致 0 件: not-applied・対象 0"
tk search '(old_2045)' --regex --path "$E" --replace '$1' --yes --json > "$TMP/e-same.json" 2>&1
check "$([ "$(fstatus a.rs < "$TMP/e-same.json")" = unchanged ] && diff -r "$E" "$TMP/tree.orig" >/dev/null && echo 1)" "置換後が元と同じ: unchanged・何も書かない"
if tk search '(old' --regex --path "$E" --replace x --yes > "$TMP/e-regex.txt" 2>&1; then RC=0; else RC=$?; fi
check "$([ "$RC" -ne 0 ] && grep -q '正規表現として読めない' "$TMP/e-regex.txt" && echo 1)" "不正な正規表現: 理由つきで失敗（rc=$RC）"
if tk search '(old)_2045' --regex --path "$E" --replace '$2' --yes > "$TMP/e-group.txt" 2>&1; then RC=0; else RC=$?; fi
check "$([ "$RC" -ne 0 ] && grep -q '\$2' "$TMP/e-group.txt" && diff -r "$E" "$TMP/tree.orig" >/dev/null && echo 1)" "無いグループ（\$2）: 理由つきで失敗し何も書かない（$(head -1 "$TMP/e-group.txt")）"
perl -e 'print "$_ old_2045 old_2045 old_2045\n" for 1..1500' > "$E/src/many.txt"
tk search old_2045 --path "$E/src" --replace z --yes --json --limit 1 > "$TMP/e-many.json" 2>&1
MANY="$(grep -c 'z z z$' "$E/src/many.txt")"
echo "    大量一致: status=$(jget status < "$TMP/e-many.json") replaced=$(jget replaced < "$TMP/e-many.json")・置き換わった行 $MANY / 1500"
check "$([ "$MANY" = 1500 ] && [ "$(grep -c old_2045 "$E/src/many.txt")" = 0 ] && echo 1)" "1 ファイルに 4500 箇所: 全部置き換わる"
tk search --undo --yes --json > /dev/null 2>&1
check "$(! grep -q 'z z z' "$E/src/many.txt" && [ "$(grep -c old_2045 "$E/src/many.txt")" = 1500 ] && echo 1)" "大量一致も 1 回で戻る"
stop_isolated_gui "$APP_PID"
ISOLATED_GUI_PID=""

echo "--- ⑤ visual-test workspace-replace-2045（置換欄 → プレビュー → 行の置換 → すべて置換 → 元に戻す）"
DUMP_DIR="${WS2045_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP_DIR"
launch_isolated_gui "$TMP/visual.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=workspace-replace-2045 \
  TAKO_VISUAL_DUMP_DIR="$DUMP_DIR" ZDOTDIR="$TMP/zdot" || exit $?
APP_PID="$ISOLATED_GUI_PID"
for _ in $(seq 1 1200); do
  kill -0 "$APP_PID" 2>/dev/null || break
  sleep 0.1
done
stop_isolated_gui "$APP_PID"
ISOLATED_GUI_PID=""
grep -E "TAKO_VISUAL_PIXEL: workspace-replace-2045|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED|visual-test workspace-replace-2045" \
  "$TMP/visual.log" | sed "s#$TMP#<tmp>#g; s/^/    /"
check "$(grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual.log" && echo 1)" \
  "visual: 置換欄に打つとプレビューが並び、行の置換 → すべて置換 → 元に戻す ×2 でディスクが期待どおりに動く"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
