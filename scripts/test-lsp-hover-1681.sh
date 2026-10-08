#!/bin/bash
# test-lsp-hover-1681.sh — ホバー（型・doc のカード。#1681）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1681 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 hover（偽サーバが Markdown を返す・実 GUI のマウス経路）: 識別子に乗せると
#      カードが出る / 本文は見出し・コードブロック・リンクの 3 種 / 基準画像（カードだけを外した 1 枚）との
#      差分がカードの矩形の中だけ / カードの上では閉じず外で閉じる / 100 回出し入れしても保持件数
#      （待ち・カード・manager の待ちの表・持ち手）が増えない / 下端の行では真上へ返して窓に収める /
#      Esc はカードだけを閉じる / 補完の一覧を出しているあいだは出さない
#   ② A/B: `TAKO_1681_LEGACY=1`（マウスで問い合わせない = #1681 前）で同じ節が名指しで落ちる
#   ③ 実の rust-analyzer（visual-test 節 hover-real）: String に乗せると std の doc のカードが出る
#      （before = `TAKO_1681_LEGACY=1` では出ない。rust-analyzer が無ければ SKIPPED と出す = 未実測）
#   ④ CLI / MCP（隔離 GUI へ実 CLI と `tako mcp serve`）: Markdown / 平文 / 空 / 範囲 / MCP と字面一致 /
#      --show でカードが出る / 開いていない文書（明示の問い合わせは一時的に開いて閉じる）/ 未導入 /
#      対象外 / 文字の途中は拒否 / 人向けの体裁
#
# 使い方: bash scripts/test-lsp-hover-1681.sh
#   画像を残すときは TAKO_1681_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
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
# toolchain を見失う = #1679 / #1682 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

TMP="$(mktemp -d /tmp/tako-1681-XXXXXX)"
DUMP="${TAKO_1681_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1681-$$"
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
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DUMP"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 300 秒）
run_section() {
  local log="$1" section="$2"
  shift 2
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" \
    TAKO_VISUAL_DUMP_DIR="$DUMP" ${1+"$@"} || exit $?
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
echo "== ① visual-test 節 hover（偽サーバの Markdown・実 GUI のマウス経路） =="
run_section "$TMP/new.log" hover
grep "TAKO_VISUAL_PIXEL: hover" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi

echo
echo "== ② A/B: 旧挙動（マウスで問い合わせない）で同じ節が落ちる =="
run_section "$TMP/ab-legacy.log" hover TAKO_1681_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
grep "TAKO_VISUAL_PIXEL: hover" "$TMP/ab-legacy.log" | head -3 | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test hover: 識別子に乗せるとカードが出る" "$TMP/ab-legacy.log"; then
  pass "TAKO_1681_LEGACY=1 では落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/ab-legacy.log" | head -1 | cut -c1-140)"
else
  fail "TAKO_1681_LEGACY=1 でも落ちない（節に検出力が無い）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/ab-legacy.log" | head -1)"
fi

echo
echo "== ③ 実の rust-analyzer（visual-test 節 hover-real） =="
if [ -n "$REAL_RA" ] && [ -n "$REAL_RUSTUP_HOME" ]; then
  run_section "$TMP/real.log" hover-real \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  # before（#1681 前 = マウスで問い合わせない）: 同じ実サーバ・同じマウスでカードが出ない。
  # 画像は before/ へ分ける（同じ置き場だと after の hover-real.png をカードの無い 1 枚で上書きする）
  run_section "$TMP/real-legacy.log" hover-real TAKO_1681_LEGACY=1 \
    TAKO_VISUAL_DUMP_DIR="$DUMP/before" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test hover-real: 実サーバの doc のカードが出る" "$TMP/real-legacy.log"; then
    pass "before（TAKO_1681_LEGACY=1）では実サーバでもカードが出ない"
  else
    fail "before でもカードが出る（または別の理由で落ちた）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/real-legacy.log" | head -1)"
  fi
else
  echo "TAKO_VISUAL_1681_REAL: SKIPPED（rust-analyzer が無い）" > "$TMP/real.log"
fi
grep -E "TAKO_VISUAL_PIXEL: hover-real|TAKO_VISUAL_1681_REAL" "$TMP/real.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_1681_REAL: SKIPPED" "$TMP/real.log"; then
  echo "  [--] 未実測: rust-analyzer が PATH に無い"
elif grep -q "TAKO_VISUAL_TEST_OK" "$TMP/real.log"; then
  pass "実サーバの doc のカードが出る"
else
  fail "実サーバの節が緑にならない"
  grep -E "FAILED|panicked" "$TMP/real.log" | tail -3 | sed 's/^/    /'
fi
ls "$DUMP"/hover*.png 2>/dev/null | sed 's/^/    画像（after）: /'
ls "$DUMP"/before/hover*.png 2>/dev/null | sed 's/^/    画像（before = TAKO_1681_LEGACY=1）: /'

echo
echo "== ④ CLI / MCP（隔離 GUI） =="
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
printf 'fn main() {\n    let total = 1;\n    let 名前 = total;\n    plain_word();\n    empty_word();\n}\n' > "$P/src/main.rs"
cp "$P/src/main.rs" "$P/src/closed.rs"
printf 'x = 1\n' > "$P/src/tool.py"
printf 'plain text\n' > "$P/notes.txt"
RULES="$TMP/hover.json"
cat > "$RULES" <<'JSON'
[
  { "line": 1, "uri_suffix": "main.rs", "result": {
      "contents": { "kind": "markdown", "value": "# total\n\n```rust\nlet total: i32\n```\n\n[docs](https://doc.rust-lang.org/std/)" },
      "range": { "start": { "line": 1, "character": 8 }, "end": { "line": 1, "character": 13 } } } },
  { "line": 3, "result": { "contents": { "kind": "plaintext", "value": "a < b && *c*" } } },
  { "line": 4, "result": null },
  { "echo": true }
]
JSON
launch_isolated_gui "$TMP/app.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_BIN_PYRIGHT="$TMP/no-such-pyright" \
  TAKO_LSP_FAKE_HOVER="$RULES" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
SRC="$("$TAKO_BIN" open "$P/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$SRC" ] || { echo "main.rs を開けない"; exit 1; }
"$TAKO_BIN" edit start --pane "$SRC" >/dev/null
hov() { "$TAKO_BIN" lsp hover "$@" --json 2>/dev/null; }

R1="$(hov --pane "$SRC" --line 2 --column 10)"
check_eq "Markdown の status" "found" "$(printf '%s' "$R1" | json 'd["status"]')"
check_eq "種類" "markdown" "$(printf '%s' "$R1" | json 'd["kind"]')"
check_eq "本文はそのまま（1 行目）" "# total" "$(printf '%s' "$R1" | json 'd["contents"].splitlines()[0]')"
RANGE_EXPR='"%d:%d-%d:%d" % tuple(d["range"][k][f] for k in ("start", "end") for f in ("line", "column"))'
check_eq "範囲（行 2 桁 8〜13）" "2:8-2:13" "$(printf '%s' "$R1" | json "$RANGE_EXPR")"

MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数の JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1681","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp\",\"arguments\":$1}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }
M1="$(mcp "{\"action\":\"hover\",\"pane\":$SRC,\"line\":2,\"column\":10}")"
if same_json "$R1" "$M1"; then pass "CLI と MCP の答えが字面一致（Markdown）"; else fail "CLI と MCP が食い違う: $M1"; fi

R2="$(hov --pane "$SRC" --line 4 --column 6)"
check_eq "平文の種類" "plaintext" "$(printf '%s' "$R2" | json 'd["kind"]')"
check_eq "平文はエスケープしない" "a < b && *c*" "$(printf '%s' "$R2" | json 'd["contents"]')"
M2="$(mcp "{\"action\":\"hover\",\"pane\":$SRC,\"line\":4,\"column\":6}")"
if same_json "$R2" "$M2"; then pass "CLI と MCP の答えが字面一致（平文）"; else fail "CLI と MCP が食い違う（平文）"; fi
R3="$(hov --pane "$SRC" --line 5 --column 6)"
check_eq "空の答えは none" "none" "$(printf '%s' "$R3" | json 'd["status"]')"
# 日本語（UTF-16 で 2 字）の後ろの語: 位置の往復（偽サーバが自前の本文で語を引いて返す）
COL="$(python3 -c 'print(len("    let 名前 = to".encode()))')"
R4="$(hov --pane "$SRC" --line 3 --column "$COL")"
check_eq "日本語の後ろの語（UTF-16 の往復）" "**total**" "$(printf '%s' "$R4" | json 'd["contents"]')"

R5="$(hov --pane "$SRC" --line 2 --column 10 --show)"
check_eq "--show でカードが出る" "True" "$(printf '%s' "$R5" | json 'd["shown"]')"

CLOSED="$("$TAKO_BIN" open "$P/src/closed.rs" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
R6="$(hov --pane "$CLOSED" --line 2 --column 10)"
check_eq "編集していない文書も明示の問い合わせは答える" "**total**" "$(printf '%s' "$R6" | json 'd["contents"]')"
DOCS="$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["servers"][0]["documents"]')"
check_eq "一時的に開いた文書は答えのあと閉じる（開いているのは編集中の 1 つ）" "1" "$DOCS"
PENDING="$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["servers"][0]["pending_requests"]')"
check_eq "待ちの表は空" "0" "$PENDING"

PY="$("$TAKO_BIN" open "$P/src/tool.py" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
R7="$(hov --pane "$PY" --line 1 --column 0)"
check_eq "未導入" "not-installed" "$(printf '%s' "$R7" | json 'd["status"]')"
check_eq "未導入は導入コマンドを返す" "True" "$(printf '%s' "$R7" | json 'bool(d.get("install_command"))')"
TXT="$("$TAKO_BIN" open "$P/notes.txt" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
R8="$(hov --pane "$TXT" --line 1 --column 0)"
check_eq "受け持つサーバが無い" "no-server" "$(printf '%s' "$R8" | json 'd["status"]')"
MID="$("$TAKO_BIN" lsp hover --pane "$SRC" --line 3 --column 9 2>&1)"
case "$MID" in *"境界"*) pass "文字の途中の桁は丸めずに拒否" ;; *) fail "文字の途中の桁: $MID" ;; esac
HUMAN="$("$TAKO_BIN" lsp hover --pane "$SRC" --line 2 --column 10 2>/dev/null | head -2 | tr '\n' '|')"
check_eq "人向けの体裁（見出し + 本文をそのまま）" "found kind=markdown range=2:8-2:13|# total|" "$HUMAN"

echo
echo "== 参考: main の CLI（/Applications の tako）には hover が無い =="
if [ -x /Applications/tako.app/Contents/MacOS/tako ]; then
  /Applications/tako.app/Contents/MacOS/tako lsp hover --line 1 --column 0 2>&1 | head -2 | sed 's/^/    /'
fi

stop_isolated_gui "$APP_PID"; APP_PID=""
echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
