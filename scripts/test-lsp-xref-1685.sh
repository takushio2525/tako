#!/bin/bash
# test-lsp-xref-1685.sh — 参照検索・呼び出し階層・型階層（#1685）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1685 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 xref-1685（偽サーバ・実マウスの経路）: 識別子の右クリックに「参照を検索」が出て、
#      押すと右パネルの references ビューに 2 行 / 別のファイルの行を ⌘クリックすると新しいペインが
#      1 枚増え、その行（90 行目）が見える / 問い合わせたペインを閉じると保持件数の口が 0
#   ② A/B: `TAKO_1685_LEGACY=1` で ① が名指しで落ちる（画像は before/ へ分ける）
#   ③ CLI / MCP（隔離 GUI へ実 CLI と `tako mcp serve`）: 参照の一覧の行が CLI と MCP で字面一致 /
#      呼び出し階層を開いて展開・折りたたみを CLI で読み書きし、往復で一致 / 行を開くと新しいペイン /
#      0 件・巨大な結果（3000 件 → 保持 2000）・未対応・サーバを止めた、が別の status /
#      問い合わせたペインを閉じると `tako lsp results` の retained が 0
#   ④ 実の rust-analyzer（`TAKO_1685_REAL=1` のときだけ。tako 自身のワークスペースを読ませるので重い:
#      build script のための cargo check が走る）: tako の実関数の references / incoming-calls が
#      grep の結果と食い違わない・supertypes は能力どおりの答え。測り終えたらサーバを止める
#
# 使い方: bash scripts/test-lsp-xref-1685.sh
#   画像を残すときは TAKO_1685_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
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

# 実の rust-analyzer は HOME を差し替える前に控える（rustup の proxy なので、HOME を変えると
# toolchain を見失う = #1681 / #1893 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

TMP="$(mktemp -d /tmp/tako-1685-XXXXXX)"
DUMP="${TAKO_1685_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1685-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# 隔離 GUI は言語サーバを自動では起こさない（#2048）。この検証は起こす前提なので明示する
export TAKO_LSP_AUTOSTART=1

# HOME を差し替える前にビルドする（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app と偽サーバをビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins visual-test || exit 1
FAKE="$(dirname "$TAKO_BIN")/tako-lsp-fake"

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# 隔離 GUI でも自動命名は claude を呼ぶので止める
export TAKO_AUTO_RENAME=0
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DUMP/after" "$DUMP/before"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 300 秒）。
# 画像は既定で after/ へ（旧挙動の腕は呼び出し側が before/ を渡す = 同じ名前で上書きしない）
run_section() {
  local log="$1" section="$2"
  shift 2
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" \
    TAKO_VISUAL_DUMP_DIR="$DUMP/after" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 3000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  return 0
}

echo
echo "== ① visual-test xref-1685（右クリック → 右パネル → 行の ⌘クリックで新しいペイン → 閉じたら 0） =="
run_section "$TMP/vt.log" xref-1685
grep "TAKO_VISUAL_PIXEL: xref-1685" "$TMP/vt.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt.log"; then
  pass "xref-1685 の全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "xref-1685 が落ちた"
  grep -E "FAILED|panicked" "$TMP/vt.log" | head -5 | sed 's/^/    /'
fi

echo
echo "== ② A/B: TAKO_1685_LEGACY=1 では ① が名指しで落ちる =="
run_section "$TMP/vt-legacy.log" xref-1685 TAKO_1685_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
grep -E "TAKO_VISUAL_PIXEL: xref-1685 ①" "$TMP/vt-legacy.log" | head -1 | sed 's/^/    旧腕: /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-legacy.log"; then
  fail "旧腕でも緑（検出力が無い）"
else
  NAMED="$(grep -E "FAILED.*xref-1685 ①|xref-1685 ①.*FAILED" "$TMP/vt-legacy.log" | head -1)"
  if [ -n "$NAMED" ]; then pass "旧腕は ① で名指しの FAILED（$(printf '%s' "$NAMED" | cut -c1-120)）"; else fail "旧腕が ① 以外で落ちた"; grep -E "FAILED|panicked" "$TMP/vt-legacy.log" | head -3 | sed 's/^/    /'; fi
fi

# --- ③ CLI / MCP（偽サーバ） -------------------------------------------------------
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
{
  printf 'mod other;\n'
  printf 'fn main() {\n'
  printf '    other::helper();\n'
  printf '    zero();\n'
  printf '    huge();\n'
  printf '}\n'
  printf 'fn zero() {}\n'
  printf 'fn huge() {}\n'
} > "$P/src/main.rs"
{
  n=0; while [ "$n" -lt 3000 ]; do
    case "$n" in
      0) printf 'pub fn helper() {}\n' ;;
      40) printf 'fn caller() { helper(); }\n' ;;
      70) printf 'fn top() { caller(); }\n' ;;
      *) printf '// other %s\n' "$n" ;;
    esac
    n=$((n + 1))
  done
} > "$P/src/other.rs"
# 実パス（macOS の /var → /private/var）で URI を組む = tako の応答と同じ綴り
REAL="$(cd "$P" && pwd -P)"
RULES="$TMP/xref.json"
python3 - "$REAL" "$RULES" <<'PY'
import json, sys, urllib.parse
real, out = sys.argv[1], sys.argv[2]
def uri(rel):
    return "file://" + urllib.parse.quote(f"{real}/{rel}")
def loc(rel, line, ch):
    return {"uri": uri(rel), "range": {"start": {"line": line, "character": ch}, "end": {"line": line, "character": ch + 6}}}
def item(name, rel, line, ch):
    r = {"start": {"line": line, "character": ch}, "end": {"line": line, "character": ch + len(name)}}
    return {"name": name, "kind": 12, "uri": uri(rel), "range": r, "selectionRange": r, "data": {"n": name}}
site = lambda line, ch: [{"start": {"line": line, "character": ch}, "end": {"line": line, "character": ch + 6}}]
rules = [
    {"method": "textDocument/references", "uri_suffix": "main.rs", "line": 2,
     "result": [loc("src/main.rs", 2, 11), loc("src/other.rs", 0, 7), loc("src/other.rs", 40, 14)]},
    {"method": "textDocument/references", "uri_suffix": "main.rs", "line": 3, "result": []},
    {"method": "textDocument/references", "uri_suffix": "main.rs", "line": 4, "generate": 3000},
    {"method": "textDocument/prepareCallHierarchy", "result": [item("helper", "src/other.rs", 0, 7)]},
    {"method": "callHierarchy/incomingCalls", "item_name": "helper",
     "result": [{"from": item("caller", "src/other.rs", 40, 3), "fromRanges": site(40, 14)}]},
    {"method": "callHierarchy/incomingCalls", "item_name": "caller",
     "result": [{"from": item("top", "src/other.rs", 70, 3), "fromRanges": site(70, 11)}]},
]
json.dump(rules, open(out, "w"))
PY

panes() { "$TAKO_BIN" list 2>/dev/null | json 'sum(len(t["panes"]) for t in d["tabs"])'; }
lsp() { "$TAKO_BIN" lsp "$@" --json 2>/dev/null; }
# 申告する能力の組（偽サーバは起動のたびに読む = 書き換えて restart すると能力が変わる）
PROVIDERS="$TMP/providers.json"
printf '{"referencesProvider":true,"callHierarchyProvider":true,"typeHierarchyProvider":true}' > "$PROVIDERS"

echo
echo "== ③ 起動（偽サーバを rust-analyzer の代わりに・参照と階層を申告） =="
launch_isolated_gui "$TMP/app.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_XREF="$RULES" \
  TAKO_LSP_FAKE_PROVIDERS="@$PROVIDERS" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
[ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
SRC="$("$TAKO_BIN" open "$P/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$SRC" ] || { echo "main.rs を開けない"; exit 1; }
echo "  pid=$APP_PID root=$ROOT source=$SRC"

echo
echo "== ③-1 参照の一覧（CLI）と右パネル =="
R1="$(lsp references --pane "$SRC" --line 3 --column 11 --show)"
check_eq "status" "found" "$(printf '%s' "$R1" | json 'd["status"]')"
check_eq "一覧の行（パス末尾:行:抜粋）" \
  "main.rs:3:other::helper(); | other.rs:1:pub fn helper() {} | other.rs:41:fn caller() { helper(); }" \
  "$(printf '%s' "$R1" | json '" | ".join(i["path"].rsplit("/",1)[1]+":"+str(i["line"])+":"+i["text"] for i in d["items"])')"
check_eq "右パネルは references ビュー" "True references" \
  "$("$TAKO_BIN" panel 2>/dev/null | json 'str(d["visible"])+" "+d["view"]')"
check_eq "保持件数" "1" "$(lsp results | json 'd["retained"]')"
"$TAKO_BIN" lsp references --pane "$SRC" --line 3 --column 11 | sed 's/^/    /'

echo
echo "== ③-2 CLI と MCP が同じ答え =="
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数の JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1685","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp\",\"arguments\":$1}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }
CLI="$(lsp references --pane "$SRC" --line 3 --column 11)"
MCP="$(mcp "{\"action\":\"references\",\"pane\":$SRC,\"line\":3,\"column\":11}")"
if same_json "$CLI" "$MCP"; then pass "references が CLI と MCP で字面一致"; else fail "references が食い違う: CLI=$CLI MCP=$MCP"; fi
if same_json "$(lsp results)" "$(mcp '{"action":"results"}')"; then pass "results が CLI と MCP で字面一致"; else fail "results が食い違う"; fi

echo
echo "== ③-3 行を開く → 別のファイルは新しいペインが 1 枚増えてその行 =="
BEFORE="$(panes)"
O1="$(lsp open-result --item 3)"
check_eq "landing" "new-pane" "$(printf '%s' "$O1" | json 'd["landing"]')"
check_eq "ペインが 1 枚増える" "$((BEFORE + 1))" "$(panes)"
check_eq "着地した行" "41" "$(printf '%s' "$O1" | json 'd["open"]["line"]')"
O2="$(lsp open-result --item 2)"
check_eq "同じファイルの別の行は使い回し" "reused" "$(printf '%s' "$O2" | json 'd["landing"]')"
check_eq "ペイン数は増えない" "$((BEFORE + 1))" "$(panes)"

echo
echo "== ③-4 呼び出し階層: 展開・折りたたみを CLI で読み書きし往復で一致 =="
H1="$(lsp incoming-calls --pane "$SRC" --line 3 --column 11)"
check_eq "status" "found" "$(printf '%s' "$H1" | json 'd["status"]')"
check_eq "根を 1 段開いた形" "[1] helper,caller" \
  "$(printf '%s' "$H1" | json 'str(d["expanded"])+" "+",".join(n["name"] for n in d["nodes"])')"
check_eq "呼び出し元は呼び出しの位置（41 行目 14 桁）" "41:14" "$(printf '%s' "$H1" | json 'str(d["nodes"][1]["line"])+":"+str(d["nodes"][1]["column"])')"
H2="$(lsp expand --item 2)"
check_eq "子を問い合わせて開く" "[1, 2] helper,caller,top" \
  "$(printf '%s' "$H2" | json 'str(d["expanded"])+" "+",".join(n["name"] for n in d["nodes"])')"
check_eq "読み直しても同じ（往復）" "$(printf '%s' "$H2" | json 'd["nodes"]')" "$(lsp results | json 'd["nodes"]')"
lsp collapse --item 1 >/dev/null
check_eq "閉じた状態を読み直す" "[2] 1" "$(lsp results | json 'str(d["expanded"])+" "+str(len(d["nodes"]))')"
lsp expand --item 1 >/dev/null
check_eq "開き直すと子の開き具合は残る" "[1, 2] 3" "$(lsp results | json 'str(d["expanded"])+" "+str(len(d["nodes"]))')"
"$TAKO_BIN" lsp results | sed 's/^/    /'

echo
echo "== ③-5 エッジ: 0 件・巨大な結果・階層の対象でない位置・未対応・サーバを止めた =="
Z="$(lsp references --pane "$SRC" --line 4 --column 4)"
check_eq "0 件は none（理由つき）" "none True" "$(printf '%s' "$Z" | json 'd["status"]+" "+str(bool(d["reason"]))')"
B="$(lsp references --pane "$SRC" --line 5 --column 4)"
check_eq "3000 件 → 保持 2000・既定で 100 行・切った数" "3000 100 2900" \
  "$(printf '%s' "$B" | json 'str(d["total"])+" "+str(d["shown"])+" "+str(d["truncated"])')"
check_eq "保持は上限まで" "2000" "$(lsp results --limit 0 | json 'len(d["items"])')"
S="$(lsp supertypes --pane "$SRC" --line 3 --column 11)"
check_eq "型階層の規則が無い（null）= 空の答え" "none" "$(printf '%s' "$S" | json 'd["status"]')"
# 参照だけを申告するサーバ（呼び出し階層・型階層を持たない）へ起こし直す
printf '{"referencesProvider":true}' > "$PROVIDERS"
"$TAKO_BIN" lsp restart >/dev/null 2>&1
U="$(lsp supertypes --pane "$SRC" --line 3 --column 11)"
check_eq "能力に無い型階層は unsupported（送らない）" "unsupported" "$(printf '%s' "$U" | json 'd["status"]')"
echo "    理由: $(printf '%s' "$U" | json 'd["reason"]')"
check_eq "呼び出し階層も unsupported" "unsupported" \
  "$(lsp incoming-calls --pane "$SRC" --line 3 --column 11 | json 'd["status"]')"
check_eq "参照は申告どおり答える" "found" "$(lsp references --pane "$SRC" --line 3 --column 11 | json 'd["status"]')"
"$TAKO_BIN" lsp stop >/dev/null 2>&1
D="$(lsp references --pane "$SRC" --line 3 --column 11)"
check_eq "止めたサーバは unavailable（前の結果は残る）" "unavailable 1" \
  "$(printf '%s' "$D" | json 'd["status"]')"" $(lsp results | json 'd["retained"]')"
"$TAKO_BIN" lsp restart >/dev/null 2>&1

echo
echo "== ③-6 問い合わせたペインを閉じる → 保持件数 0 =="
"$TAKO_BIN" close --pane "$SRC" --force >/dev/null 2>&1
check_eq "retained" "0" "$(lsp results | json 'd["retained"]')"
check_eq "retained_rows" "0" "$(lsp results | json 'd["retained_rows"]')"
stop_isolated_gui "$APP_PID"; APP_PID=""

# --- ④ 実の rust-analyzer（tako 自身） ----------------------------------------------
echo
if [ "${TAKO_1685_REAL:-0}" != "1" ]; then
  echo "  [--] 未実測: 実の rust-analyzer は TAKO_1685_REAL=1 のときだけ（tako 全体を読ませるので重い）"
elif [ -z "$REAL_RA" ]; then
  echo "  [--] 未実測: rust-analyzer が PATH に無い"
else
  echo "== ④ 実の rust-analyzer で tako 自身の関数（tako_core::lsp::xref::symbol_kind_name） =="
  FILE="$REPO_ROOT/crates/tako-core/src/lsp/xref.rs"
  LINE="$(grep -n '^pub fn symbol_kind_name' "$FILE" | cut -d: -f1)"
  COL="$(grep '^pub fn symbol_kind_name' "$FILE" | python3 -c 'import sys; print(sys.stdin.read().index("symbol_kind_name"))')"
  launch_isolated_gui "$TMP/real.log" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" CARGO_BUILD_JOBS=2 \
    TAKO_LSP_XREF_TIMEOUT_SECS=900 || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/real.log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  SRC="$("$TAKO_BIN" open "$FILE" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
  T0="$(date +%s)"
  RR="$(lsp references --pane "$SRC" --line "$LINE" --column "$COL" --limit 0)"
  echo "    references: $(($(date +%s) - T0)) 秒（初回はワークスペースの読み込みを含む）"
  "$TAKO_BIN" lsp references --pane "$SRC" --line "$LINE" --column "$COL" | sed 's/^/    /'
  GOT="$(printf '%s' "$RR" | json '"\n".join(sorted(i["path"].split("/crates/",1)[1]+":"+str(i["line"]) for i in d["items"]))')"
  WANT="$(cd "$REPO_ROOT" && grep -rn --include='*.rs' -E '(^|[^_a-z])symbol_kind_name\(' crates \
    | grep -v '^\s*//' | awk -F: '{print $1":"$2}' | sed 's|^crates/||' | sort)"
  echo "    rust-analyzer: $(printf '%s\n' "$GOT" | grep -c .) 件 / grep（呼び出しと定義の行）: $(printf '%s\n' "$WANT" | grep -c .) 件"
  if [ "$GOT" = "$WANT" ]; then pass "references が grep と一致"; else
    fail "references が grep と食い違う"; diff <(printf '%s\n' "$WANT") <(printf '%s\n' "$GOT") | sed 's/^/      /'
  fi
  IC="$(lsp incoming-calls --pane "$SRC" --line "$LINE" --column "$COL" --limit 0)"
  "$TAKO_BIN" lsp results | sed 's/^/    /'
  check_eq "incoming-calls は found" "found" "$(printf '%s' "$IC" | json 'd["status"]')"
  CALLERS="$(printf '%s' "$IC" | json '",".join(sorted(set(n["name"] for n in d["nodes"][1:])))')"
  echo "    呼び出し元: $CALLERS"
  ST="$(lsp supertypes --pane "$SRC" --line "$LINE" --column "$COL")"
  echo "    supertypes: status=$(printf '%s' "$ST" | json 'd["status"]') reason=$(printf '%s' "$ST" | json 'd.get("reason","")')"
  "$TAKO_BIN" lsp status --json | json '"    能力: references="+str(d["servers"][0]["capabilities"].get("referencesProvider"))+" callHierarchy="+str(d["servers"][0]["capabilities"].get("callHierarchyProvider"))+" typeHierarchy="+str(d["servers"][0]["capabilities"].get("typeHierarchyProvider"))'
  # 測り終えたら rust-analyzer を止める（メモリを食う）
  "$TAKO_BIN" lsp stop >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"; APP_PID=""
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
