#!/usr/bin/env bash
# test-search-case-1653.sh — 検索・置換の「大文字小文字の区別 / 単語単位」の実 GUI 経路テスト（#1653）
#
# 隔離した data で実 tako-app を立て、
#   ① visual-test 節 `search-case` を走らせる → 検索欄のトグルを**実マウスで**押すたびに
#      件数が 3 → 5 → 4 → 2 → 3 と変わり、置換欄の Enter と全置換が型名 `Value` を残す
#      （修正前は区別できず `Value::new()` が `item::new()` になった）
#   ② 同じ手順を **CLI（`tako edit search|replace`）と MCP（`tako_preview_search|replace`）**
#      から撃ち、件数・使った条件・置換結果が期待どおりで、CLI と MCP の応答が字面まで
#      一致することを見る（設計原則 5 の 1:1）。引数を省略したときは「区別する」
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# 面を用意できなければ起動しない（終了コード 4 = 未実測。#1744）。
#
# 使い方: bash scripts/test-search-case-1653.sh
#   `SEARCH_CASE_DUMP_DIR=<dir>` を置くと ① の各段のフレーム（PNG）をそこへ残す
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

TMP="$(mktemp -d /tmp/tako-1653-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  stop_isolated_gui "${CLI_GUI_PID:-}"
  tmux -L "${TAKO_TMUX_SOCKET:-tako-1653-$$}" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# visual-test 版をビルドする（同じ target/debug/tako-app を上書きする。② もこの版で回る）
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
export TAKO_TMUX_SOCKET="tako-1653-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

echo "① search-case 節（検索欄のトグルを実マウスで押す）"
DUMP_ENV=()
if [ -n "${SEARCH_CASE_DUMP_DIR:-}" ]; then
  mkdir -p "$SEARCH_CASE_DUMP_DIR"
  DUMP_ENV=("TAKO_VISUAL_DUMP_DIR=$SEARCH_CASE_DUMP_DIR")
fi
launch_isolated_gui "$TMP/visual.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=search-case \
  ${DUMP_ENV[@]+"${DUMP_ENV[@]}"} || exit $?
VISUAL_PID="$ISOLATED_GUI_PID"
# 終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
for _ in $(seq 1 1800); do
  kill -0 "$VISUAL_PID" 2>/dev/null || break
  sleep 0.1
done
stop_isolated_gui "$VISUAL_PID"
grep "TAKO_VISUAL_1653" "$TMP/visual.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual.log"; then
  pass "トグル 4 回の件数と置換の結果がすべて期待どおり（TAKO_VISUAL_TEST_OK）"
else
  fail "search-case 節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/visual.log" | tail -5 | sed 's/^/    /'
fi

echo "② CLI / MCP から同じ手順で同じ結果（省略時は区別する）"
# 応答 JSON から 1 つの値を取る（`search.total` のように下も辿れる）
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
# ペイン ID と器の寸法（ペインごとに違う）だけを落とす
normalize() {
  python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
d.pop("pane", None)
d.pop("viewport", None)
if isinstance(d.get("search"), dict):
    d["search"].pop("viewport", None)
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
# 型名・全大文字・接尾・漢字の直後・かぎ括弧・小文字化で伸びる İ を含む本文
FIXTURE=$'let value = Value::new();\nlet values = value + VALUE;\n// 値value 「value」 İstanbul istanbul\n'
printf '%s' "$FIXTURE" > "$TMP/cli.rs"
printf '%s' "$FIXTURE" > "$TMP/mcp.rs"
CLI_PANE="$("$TAKO_BIN" open "$TMP/cli.rs" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
MCP_PANE="$("$TAKO_BIN" open "$TMP/mcp.rs" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
case "$CLI_PANE$MCP_PANE" in
  ''|*[!0-9]*) echo "プレビューペインを開けない: cli=$CLI_PANE mcp=$MCP_PANE"; exit 1 ;;
esac
"$TAKO_BIN" edit start --pane "$CLI_PANE" >/dev/null || exit 1
"$TAKO_BIN" edit start --pane "$MCP_PANE" >/dev/null || exit 1

# 手順（CLI の引数 | MCP のツール名 | MCP の引数 JSON | 見る値のパス | 期待値 | 見出し）
STEPS=(
  "search value|tako_preview_search|{\"query\":\"value\"}|search.total,search.case_sensitive,search.whole_word|5,true,false|query だけ = 区別する（Value / VALUE は当たらない）"
  "search -i|tako_preview_search|{\"case_sensitive\":false}|search.total,search.case_sensitive,search.whole_word|7,false,false|query 省略で条件だけ変える（区別しない）"
  "search -w|tako_preview_search|{\"whole_word\":true}|search.total,search.case_sensitive,search.whole_word|5,false,true|単語単位を足す（values / 値value が外れ、区別しないは引き継ぐ）"
  "search --direction prev|tako_preview_search|{\"direction\":\"prev\"}|search.total,search.case_sensitive,search.whole_word|5,false,true|条件も省略 = 今の条件を引き継ぐ"
  "search value|tako_preview_search|{\"query\":\"value\"}|search.total,search.case_sensitive,search.whole_word|5,true,false|新しい query は既定から組み直す"
  "search istanbul -i|tako_preview_search|{\"query\":\"istanbul\",\"case_sensitive\":false}|search.total|1|İstanbul は区別しなくても istanbul に当たらない（U+0307 が挟まる）"
  "search İstanbul|tako_preview_search|{\"query\":\"İstanbul\"}|search.total|1|İ そのものは区別して当たる"
  "search i|tako_preview_search|{\"query\":\"i\"}|search.total|1|1 文字のクエリ（区別するので İ は当たらず istanbul の i だけ）"
  "search 値 -w|tako_preview_search|{\"query\":\"値\",\"whole_word\":true}|search.total|0|漢字は単語の文字（値value の値は単語単位で当たらない）"
  "replace value item --all|tako_preview_replace|{\"query\":\"value\",\"replacement\":\"item\",\"all\":true}|replace.replaced,replace.case_sensitive,replace.whole_word|5,true,false|全置換の省略 = 区別する"
  "replace VALUE item -i -w|tako_preview_replace|{\"query\":\"VALUE\",\"replacement\":\"item\",\"case_sensitive\":false,\"whole_word\":true}|replace.replaced,replace.case_sensitive,replace.whole_word|1,false,true|1 件置換で条件を指定（先頭から最初の Value だけ）"
)
MCP_IN="$TMP/mcp.in"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1653","version":"0"}}}' > "$MCP_IN"
CLI_OUT=()
i=0
for step in ${STEPS[@]+"${STEPS[@]}"}; do
  IFS='|' read -r cli_args tool mcp_args _ _ _ <<< "$step"
  # 1 件置換の起点をそろえる（両ペインとも先頭）
  case "$cli_args" in
    replace*) "$TAKO_BIN" edit cursor 1:0 --pane "$CLI_PANE" >/dev/null 2>&1 ;;
  esac
  # shellcheck disable=SC2086
  CLI_OUT+=("$("$TAKO_BIN" edit $cli_args --pane "$CLI_PANE" 2>&1)")
  args="$(printf '%s' "$mcp_args" | python3 -c '
import json, sys
d = json.loads(sys.stdin.read()); d["pane"] = int(sys.argv[1]); print(json.dumps(d, ensure_ascii=False))
' "$MCP_PANE")"
  case "$cli_args" in
    replace*)
      printf '{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":"tako_preview_cursor","arguments":{"pane":%s,"line":1,"col":0}}}\n' \
        $((i + 500)) "$MCP_PANE" >> "$MCP_IN" ;;
  esac
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
values_of() {
  local out="$1" paths="$2" got="" p
  local IFS=','
  for p in $paths; do
    got="${got:+$got,}$(printf '%s' "$out" | jget "$p")"
  done
  printf '%s' "$got"
}
i=0
for step in ${STEPS[@]+"${STEPS[@]}"}; do
  IFS='|' read -r cli_args _ _ paths want label <<< "$step"
  out="${CLI_OUT[$i]}"
  check_eq "CLI: ${label}（tako edit ${cli_args}）" "$want" "$(values_of "$out" "$paths")"
  check_eq "MCP の応答が CLI と字面一致（${cli_args}）" \
    "$(printf '%s' "$out" | normalize)" "$(extract $((i + 10)) | normalize)"
  i=$((i + 1))
done
# 置換の結果を本文で見る（保存したファイルのバイト列）
"$TAKO_BIN" edit save --pane "$CLI_PANE" >/dev/null
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1653","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_save\",\"arguments\":{\"pane\":${MCP_PANE}}}}" \
  | "$TAKO_BIN" mcp serve >/dev/null 2>&1
REPLACED=$'let item = item::new();\nlet items = item + VALUE;\n// 値item 「item」 İstanbul istanbul\n'
printf '%s' "$REPLACED" > "$TMP/replaced.rs"
if cmp -s "$TMP/cli.rs" "$TMP/replaced.rs" && cmp -s "$TMP/mcp.rs" "$TMP/replaced.rs"; then
  pass "置換後の本文がバイト一致（区別する全置換で Value / VALUE が残り、指定した 1 件だけ Value が変わる）"
else
  fail "置換後の本文が期待と違う"
  printf '    cli: %s\n' "$(cat "$TMP/cli.rs" | head -3 | tr '\n' '|')"
  printf '    mcp: %s\n' "$(cat "$TMP/mcp.rs" | head -3 | tr '\n' '|')"
fi
# 空のクエリ（エッジケース）
check_eq "空のクエリは 0 件（CLI）" "0" "$("$TAKO_BIN" edit search "" --pane "$CLI_PANE" 2>&1 | jget search.total)"

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
