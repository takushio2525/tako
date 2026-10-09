#!/bin/bash
# test-lsp-hover-1893.sh — ホバーの続き（#1893）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1893 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 hover-1893（偽サーバ・実マウス / 実キーの経路）: 識別子の右クリックに
#      「ホバー情報を表示」が移動の後・整形の前に出て押すとカード / カードが出ている間の右クリックで
#      閉じる / 識別子でない所には出ない / キー（⇧⌘H・Windows は Ctrl+Shift+H）でカード・本文は
#      変わらない・巨大な doc はカードでは 16,000 字 / 補完の一覧が出ている間のキーは一覧を閉じてカード
#   ② visual-test 節 hover-loading（偽サーバの読み込みの遅延 = 負荷は使わない）: 読み込み中に乗せると
#      「読み込み中」の 1 行（基準画像との差分は矩形の外 0 px）→ 済むとカード / 外れると消えて待ちも
#      残らない / 終わらなければ上限で消えて loading
#   ③ A/B: `TAKO_1893_LEGACY=1` で ① と ② が名指しで落ちる（画像は before/ へ分ける）
#   ④ 回帰: #1684 の lsp-context-menu（項目が 1 つ増えた並び）と #1681 の hover が緑のまま
#   ⑤ 実の rust-analyzer（visual-test 節 hover-loading-real）: 暖機なしで読み込みの最中に乗せると
#      済んだらカード・キーでもカード（before = `TAKO_1893_LEGACY=1` ではカードが出ない）
#   ⑥ CLI / MCP（隔離 GUI へ実 CLI と `tako mcp serve`）: 16,000 字を超える doc で既定は切る /
#      --full（MCP limit=0）で全文 / --limit N（MCP limit=N）で N 字 / CLI と MCP が字面一致 /
#      メニューの items に hover（args をそのまま MCP へ渡すとカードが出る）/ 申告の無いサーバでは出ない /
#      終わらない読み込みは CLI も MCP も loading
#
# 使い方: bash scripts/test-lsp-hover-1893.sh
#   画像を残すときは TAKO_1893_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
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
# toolchain を見失う = #1679 / #1682 / #1681 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

TMP="$(mktemp -d /tmp/tako-1893-XXXXXX)"
DUMP="${TAKO_1893_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1893-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

echo "visual-test 版の tako-app と偽サーバをビルドします…"
(cd "$REPO_ROOT" && cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test \
  && cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins || exit 1
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

# 節が緑か（名前・ログ）
section_green() {
  local name="$1" log="$2"
  grep "TAKO_VISUAL_PIXEL: $name" "$log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$log"; then
    pass "$name の全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "$name が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$log" | tail -5 | sed 's/^/    /'
  fi
}

# 旧挙動の腕が期待どおりの相で名指しで落ちたか
section_named() {
  local name="$1" log="$2" want="$3"
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test $want" "$log"; then
    pass "$name は TAKO_1893_LEGACY=1 で名指しで落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-150)"
  else
    fail "$name が TAKO_1893_LEGACY=1 で期待の相で落ちない（検出力が無い）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1)"
  fi
}

echo
echo "== ① visual-test 節 hover-1893（右クリックの項目・キー・補完中のキー・巨大な doc） =="
run_section "$TMP/menu-key.log" hover-1893
section_green hover-1893 "$TMP/menu-key.log"

echo
echo "== ② visual-test 節 hover-loading（偽サーバの読み込みの遅延） =="
run_section "$TMP/loading.log" hover-loading
section_green hover-loading "$TMP/loading.log"

echo
echo "== ③ A/B: 旧挙動（TAKO_1893_LEGACY=1）で同じ節が名指しで落ちる =="
run_section "$TMP/menu-key-legacy.log" hover-1893 TAKO_1893_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
section_named hover-1893 "$TMP/menu-key-legacy.log" "hover-1893 ①: 識別子の右クリックに「ホバー情報を表示」が出る"
run_section "$TMP/loading-legacy.log" hover-loading TAKO_1893_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
section_named hover-loading "$TMP/loading-legacy.log" "hover-loading ①: 読み込み中に乗せると「読み込み中」の 1 行が出る"
# キーの A/B: 旧腕はキーを張らないので、キーの相だけを見る（メニューの相の前に落ちるので別の腕で）
grep -E "TAKO_VISUAL_PIXEL: hover-1893 ①" "$TMP/menu-key-legacy.log" | head -1 | sed 's/^/    旧腕: /'

echo
echo "== ④ 回帰: #1684 lsp-context-menu・#1681 hover =="
run_section "$TMP/ctx.log" lsp-context-menu
section_green lsp-context-menu "$TMP/ctx.log"
run_section "$TMP/hover.log" hover
section_green hover "$TMP/hover.log"

echo
echo "== ⑤ 実の rust-analyzer（visual-test 節 hover-loading-real・暖機なし） =="
if [ -n "$REAL_RA" ] && [ -n "$REAL_RUSTUP_HOME" ]; then
  run_section "$TMP/real.log" hover-loading-real \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  # before（#1893 前 = マウスの要求は読み込みを待たない）: 同じ実サーバ・同じマウスで出ない
  run_section "$TMP/real-legacy.log" hover-loading-real TAKO_1893_LEGACY=1 \
    TAKO_VISUAL_DUMP_DIR="$DUMP/before" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  grep -E "TAKO_VISUAL_PIXEL: hover-loading-real" "$TMP/real.log" | sed 's/^/    after:  /'
  grep -E "TAKO_VISUAL_PIXEL: hover-loading-real" "$TMP/real-legacy.log" | sed 's/^/    before: /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/real.log"; then
    pass "実サーバ: 読み込みの最中に乗せたマウスに、済んだらカードが出る・キーでも出る"
  else
    fail "実サーバの節が緑にならない"
    grep -E "FAILED|panicked" "$TMP/real.log" | tail -3 | sed 's/^/    /'
  fi
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test hover-loading-real: 読み込みの最中に乗せたマウスに" "$TMP/real-legacy.log"; then
    pass "before（TAKO_1893_LEGACY=1）では実サーバでもカードが出ない"
  else
    fail "before でもカードが出る（または別の理由で落ちた）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/real-legacy.log" | head -1)"
  fi
else
  echo "  [--] 未実測: rust-analyzer が PATH に無い（TAKO_VISUAL_1893_REAL: SKIPPED）"
fi
ls "$DUMP"/after/hover*.png 2>/dev/null | sed 's/^/    画像（after）: /'
ls "$DUMP"/before/hover*.png 2>/dev/null | sed 's/^/    画像（before = TAKO_1893_LEGACY=1）: /'

echo
echo "== ⑥ CLI / MCP（隔離 GUI） =="
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
printf 'fn main() {\n    let total = 1;\n    let v = total;\n}\n' > "$P/src/main.rs"
RULES="$TMP/hover.json"
python3 - "$RULES" <<'PY'
import json, sys
big = "".join("line %d of a very long doc\n" % i for i in range(2500))
json.dump([
    {"line": 2, "result": {"contents": {"kind": "markdown", "value": big}}},
    {"echo": True},
], open(sys.argv[1], "w"))
PY
PROVIDERS="$TMP/providers.json"
echo '{"definitionProvider":true,"hoverProvider":true,"documentFormattingProvider":true}' > "$PROVIDERS"
launch_isolated_gui "$TMP/app.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_HOVER="$RULES" \
  TAKO_LSP_FAKE_PROVIDERS="@$PROVIDERS" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
SRC="$("$TAKO_BIN" open "$P/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$SRC" ] || { echo "main.rs を開けない"; exit 1; }
"$TAKO_BIN" edit start --pane "$SRC" >/dev/null
hov() { "$TAKO_BIN" lsp hover "$@" --json 2>/dev/null; }

MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数の JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1893","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp\",\"arguments\":$1}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }
TOTAL="$(python3 -c 'print(sum(len("line %d of a very long doc\n" % i) for i in range(2500)))')"
echo "    巨大な doc: $TOTAL 字"

R1="$(hov --pane "$SRC" --line 3 --column 14)"
check_eq "既定は切る（truncated）" "True" "$(printf '%s' "$R1" | json 'd["truncated"]')"
check_eq "既定の上限は 16,000 字以内" "True" "$(printf '%s' "$R1" | json 'len(d["contents"]) <= 16000')"
check_eq "切る前の字数" "$TOTAL" "$(printf '%s' "$R1" | json 'd["total_chars"]')"
check_eq "注記が全文の取り方を言う" "True" "$(printf '%s' "$R1" | json '"--full" in d["note"]')"
M1="$(mcp "{\"action\":\"hover\",\"pane\":$SRC,\"line\":3,\"column\":14}")"
if same_json "$R1" "$M1"; then pass "CLI と MCP の答えが字面一致（既定）"; else fail "CLI と MCP が食い違う（既定）"; fi

R2="$(hov --pane "$SRC" --line 3 --column 14 --full)"
check_eq "--full は全文" "$TOTAL" "$(printf '%s' "$R2" | json 'len(d["contents"])')"
check_eq "--full は印を付けない" "False" "$(printf '%s' "$R2" | json '"truncated" in d')"
M2="$(mcp "{\"action\":\"hover\",\"pane\":$SRC,\"line\":3,\"column\":14,\"limit\":0}")"
if same_json "$R2" "$M2"; then pass "CLI --full と MCP limit=0 の答えが字面一致（全文 $TOTAL 字）"; else fail "CLI --full と MCP limit=0 が食い違う"; fi

R3="$(hov --pane "$SRC" --line 3 --column 14 --limit 500)"
check_eq "--limit 500 は 500 字以内" "True" "$(printf '%s' "$R3" | json 'len(d["contents"]) <= 500 and d["truncated"]')"
M3="$(mcp "{\"action\":\"hover\",\"pane\":$SRC,\"line\":3,\"column\":14,\"limit\":500}")"
if same_json "$R3" "$M3"; then pass "CLI --limit 500 と MCP limit=500 の答えが字面一致"; else fail "--limit 500 と limit=500 が食い違う"; fi
HUMAN_LINES="$("$TAKO_BIN" lsp hover --pane "$SRC" --line 3 --column 14 --full 2>/dev/null | wc -l | tr -d ' ')"
check_eq "人向けの --full は見出し 1 行 + 本文 2500 行" "2501" "$HUMAN_LINES"
BOTH="$("$TAKO_BIN" lsp hover --pane "$SRC" --line 3 --column 14 --full --limit 9 2>&1)"
case "$BOTH" in *"cannot be used with"*) pass "--full と --limit は同時に指定できない" ;; *) fail "--full と --limit の同時指定: $BOTH" ;; esac

MENU="$("$TAKO_BIN" lsp menu --pane "$SRC" --line 2 --column 9 --json 2>/dev/null)"
check_eq "メニューに hover が出る（定義 → ホバー → 整形）" "lsp-definition,lsp-hover,lsp-format" \
  "$(printf '%s' "$MENU" | json '",".join(i["id"] for i in d["items"])')"
ARGS="$(printf '%s' "$MENU" | json 'json.dumps([i for i in d["items"] if i["id"] == "lsp-hover"][0]["args"])' 2>/dev/null)"
[ -n "$ARGS" ] || ARGS="$(printf '%s' "$MENU" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps([i for i in d["items"] if i["id"]=="lsp-hover"][0]["args"]))')"
echo "    items[hover].args = $ARGS"
M4="$(mcp "$ARGS")"
check_eq "args をそのまま MCP へ渡すとカードが出る（押したのと同じ）" "True" "$(printf '%s' "$M4" | json 'd["shown"]')"
MM="$(mcp "{\"action\":\"menu\",\"pane\":$SRC,\"line\":2,\"column\":9}")"
if same_json "$MENU" "$MM"; then pass "メニューの CLI と MCP が字面一致"; else fail "メニューの CLI と MCP が食い違う"; fi

# 申告の無いサーバ: 能力の組を差し替えて再起動
echo '{"definitionProvider":true,"documentFormattingProvider":true}' > "$PROVIDERS"
"$TAKO_BIN" lsp restart >/dev/null 2>&1
MENU2=""
for _ in $(seq 1 100); do
  MENU2="$("$TAKO_BIN" lsp menu --pane "$SRC" --line 2 --column 9 --json 2>/dev/null)"
  case "$(printf '%s' "$MENU2" | json 'd["status"]')" in ok) break ;; esac
  sleep 0.1
done
check_eq "申告の無いサーバではメニューに hover が出ない" "lsp-definition,lsp-format" \
  "$(printf '%s' "$MENU2" | json '",".join(i["id"] for i in d["items"])')"
stop_isolated_gui "$APP_PID"; APP_PID=""

# 終わらない読み込み: CLI も MCP も上限で loading（上限は env で 2 秒に縮める）。合図のファイルを
# 作らない = 読み込みは終わらない（実時間の長さで「終わらない」を演じない。#1930）
launch_isolated_gui "$TMP/app-loading.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_HOVER="$RULES" \
  TAKO_LSP_FAKE_SCENARIO=loading TAKO_LSP_FAKE_LOADING_MS=0 TAKO_LSP_FAKE_LOADING_UNTIL="$TMP/loading-never" \
  TAKO_LSP_HOVER_TIMEOUT_SECS=2 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-loading.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
SRC="$("$TAKO_BIN" open "$P/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
"$TAKO_BIN" edit start --pane "$SRC" >/dev/null
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
R5="$(hov --pane "$SRC" --line 2 --column 9)"
check_eq "終わらない読み込みは上限で loading" "loading" "$(printf '%s' "$R5" | json 'd["status"]')"
M5="$(mcp "{\"action\":\"hover\",\"pane\":$SRC,\"line\":2,\"column\":9}")"
if same_json "$R5" "$M5"; then pass "loading の CLI と MCP が字面一致"; else fail "loading の CLI と MCP が食い違う: $M5"; fi
printf '%s' "$R5" | json 'd["reason"]' | sed 's/^/    reason: /'
stop_isolated_gui "$APP_PID"; APP_PID=""

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
