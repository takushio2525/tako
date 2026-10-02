#!/bin/bash
# test-lsp-menu-1684.sh — 右クリックメニューの LSP 項目（FR-3.36 / #1684）の実経路テスト（隔離 GUI）
#
# 何を確かめるか（Issue #1684 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① visual-test `lsp-context-menu`: 隔離 GUI（tako-vd）で**実マウス**の右クリック → 握手を待つ 1 行 →
#      項目へ差し替わる → 「定義へ移動」を押すと定義の行へ着地 / 識別子でない位置（字下げ・記号・
#      数値・空行）・Markdown・未導入では従来のメニューのまま / コメントの中の語は ⌘ホバーと同じ扱い /
#      実ドラッグの選択の中なら「選択範囲を整形」まで / 「コードを整形」を押すと整形が当たる /
#      四隅に置いても描いた項目の実矩形がウィンドウに収まる
#   ② A/B: `TAKO_1684_LEGACY=1`（本文の右クリックで何もしない）の同じバイナリでは ① が FAILED
#   ③ 実 GUI + 実 CLI + MCP: `tako lsp menu` の答えが偽サーバの申告 4 通り（ゼロ / 定義のみ / 全部 /
#      一部）で固定値と一致・CLI と MCP が字面まで一致・`items[].args` をそのまま `tako_lsp` へ渡すと
#      押したのと同じ操作になる・識別子でない位置は not-symbol 0 件・未導入は not-installed・
#      Markdown は理由つきで断る
#   ④ 実サーバ（rust-analyzer があるとき）: visual-test `lsp-context-menu-real`
#
# 使い方: bash scripts/test-lsp-menu-1684.sh
#   実サーバの節を飛ばすときは `TAKO_1684_SKIP_REAL=1`
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意できなければ起動せず終了コード 4 で
# 「未実測」を返す。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1684-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1684-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# 実サーバの節で使う（HOME を差し替える前の rustup の置き場）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako --bin tako-lsp-fake || exit 1
FAKE="$REPO_ROOT/target/debug/tako-lsp-fake"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/tmpdir"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_1684_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP"
# GUI の一時 dir も $TMP の中へ（visual-test の fixture が節の途中で落ちても後片付けで消える）
common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja TMPDIR="$TMP/tmpdir/"
  TAKO_PERSIST=0 TAKO_AUTORENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
)

# visual-test の節を 1 回走らせる（$1 = 節 / $2 = ログ / $3 = data dir / 残りは追加の env）
run_visual() {
  local section="$1" log="$2" data="$3"
  shift 3
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$data" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" TAKO_VISUAL_DUMP_DIR="$DUMP" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test lsp-context-menu（実マウス・偽サーバ） =="
run_visual lsp-context-menu "$TMP/visual.log" "$TMP/data-visual"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: lsp-context-menu' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'

echo
echo "== ② A/B: TAKO_1684_LEGACY=1 で本文の右クリックが何もせず FAILED =="
run_visual lsp-context-menu "$TMP/legacy.log" "$TMP/data-legacy" TAKO_1684_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" | sed 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test lsp-context-menu ①: 識別子の右クリックでメニューが開く' \
  "$TMP/legacy.log" \
  && pass "落ちた理由は識別子の右クリックでメニューが開かないこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（偽サーバ。申告の組はファイルから読み、restart で替える） =="
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/data-gui"
mkdir -p "$TAKO_DATA_DIR"
for d in "$HOME" "$TAKO_DATA_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
PROVIDERS="$TMP/providers.json"
echo '{}' > "$PROVIDERS"
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
printf 'fn main() {\n    let value = helper(1);  \n}\n\nfn helper(x: i32) -> i32 {\n    x\n}\n' > "$P/src/main.rs"
printf '# Title\n\nhelper words\n' > "$P/README.md"
printf 'def run():\n    return run\n' > "$P/tool.py"
GOTO="$TMP/goto.json"
python3 - "$P/src/main.rs" > "$GOTO" <<'PY'
import json, sys, urllib.parse
uri = "file://" + urllib.parse.quote(sys.argv[1])
print(json.dumps([{ "method": "textDocument/definition", "line": 1,
  "result": { "uri": uri, "range": { "start": { "line": 4, "character": 3 }, "end": { "line": 4, "character": 9 } } } }]))
PY
launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_BIN_PYRIGHT="$TMP/no-such-pyright" \
  TAKO_LSP_FAKE_PROVIDERS="@$PROVIDERS" \
  TAKO_LSP_FAKE_GOTO="$GOTO" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
[ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数のツール名と JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1684","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        r = m.get("result")
        print(r["content"][0]["text"] if r else json.dumps(m.get("error"), ensure_ascii=False)); break'
}
open_file() { "$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]'; }
menu() { "$TAKO_BIN" lsp menu "$@" --json 2>/dev/null; }
ids() { json '",".join(i["id"] for i in d["items"])'; }
canon() { python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), sort_keys=True, ensure_ascii=False))'; }
echo "  pid=$APP_PID root=$ROOT"

A="$(open_file "$P/src/main.rs")"
[ -n "$A" ] || { echo "main.rs を開けない"; exit 1; }

# 申告の組を替えて起こし直し、`helper`（2 行目の 16 バイト目）を問う
ask_with() {
  printf '%s' "$1" > "$PROVIDERS"
  "$TAKO_BIN" lsp restart >/dev/null 2>&1
  menu --pane "$A" --line 2 --column 18
}
R0="$(ask_with '{}')"
check_eq "能力ゼロ: status" "ok" "$(printf '%s' "$R0" | json 'd["status"]')"
check_eq "能力ゼロ: 項目は 0 件" "" "$(printf '%s' "$R0" | ids)"
check_eq "定義のみ" "lsp-definition" "$(ask_with '{"definitionProvider":true}' | ids)"
check_eq "一部だけ（実装 + 範囲の整形。選択が無いので範囲の整形は出ない）" "lsp-implementation" \
  "$(ask_with '{"implementationProvider":{},"documentRangeFormattingProvider":true}' | ids)"
ALL='{"definitionProvider":true,"declarationProvider":true,"typeDefinitionProvider":true,"implementationProvider":true,"documentFormattingProvider":true,"documentRangeFormattingProvider":true}'
RA="$(ask_with "$ALL")"
check_eq "全部あり" "lsp-definition,lsp-declaration,lsp-type-definition,lsp-implementation,lsp-format" \
  "$(printf '%s' "$RA" | ids)"
check_eq "識別子" "helper identifier 16 22" \
  "$(printf '%s' "$RA" | json '"%s %s %d %d" % (d["symbol"]["text"], d["symbol"]["kind"], d["symbol"]["column"], d["symbol"]["end_column"])')"
check_eq "項目の位置は識別子の先頭" "16" "$(printf '%s' "$RA" | json 'd["items"][0]["args"]["column"]')"

# 選択の中なら範囲の整形まで（選択は `tako edit cursor --select-to` = GUI の選択と同じ所）
"$TAKO_BIN" edit cursor 2:4 --select-to 2:26 --pane "$A" >/dev/null 2>&1
RS="$(menu --pane "$A" --line 2 --column 18)"
check_eq "選択の中: 範囲の整形まで 6 項目" \
  "lsp-definition,lsp-declaration,lsp-type-definition,lsp-implementation,lsp-format,lsp-format-selection" \
  "$(printf '%s' "$RS" | ids)"
check_eq "範囲の整形の引数は選択そのもの" "2:4-2:26" \
  "$(printf '%s' "$RS" | json '"%d:%d-%d:%d" % tuple(d["items"][5]["args"][k] for k in ("line","column","end_line","end_column"))')"
check_eq "選択の外（1 行目）では範囲の整形は出ない" "lsp-definition,lsp-declaration,lsp-type-definition,lsp-implementation,lsp-format" \
  "$(menu --pane "$A" --line 1 --column 3 | ids)"

# CLI と MCP が字面まで一致する
RC_CLI="$(menu --pane "$A" --line 2 --column 18 | canon)"
RC_MCP="$(mcp tako_lsp "{\"action\":\"menu\",\"pane\":$A,\"line\":2,\"column\":18}" | canon)"
check_eq "CLI と MCP の答えが字面まで一致" "$RC_CLI" "$RC_MCP"

# items[].args をそのまま tako_lsp へ渡すと押したのと同じ操作になる
DEF_ARGS="$(printf '%s' "$RS" | json 'json.dumps(d["items"][0]["args"])')"
GOT="$(mcp tako_lsp "$DEF_ARGS")"
check_eq "定義へ移動の args → found" "found 5" \
  "$(printf '%s' "$GOT" | json '"%s %s" % (d["status"], d["locations"][0]["line"])')"
FMT_ARGS="$(printf '%s' "$RS" | json 'json.dumps(d["items"][4]["args"])')"
FMT="$(mcp tako_lsp "$FMT_ARGS")"
check_eq "コードを整形の args → formatted" "formatted" "$(printf '%s' "$FMT" | json 'd["status"]')"

# 人向けの表示は押したときと同じコマンドを出す
HUMAN="$("$TAKO_BIN" lsp menu --pane "$A" --line 2 --column 18 2>/dev/null)"
case "$HUMAN" in
  *"lsp-definition"*"tako lsp definition --pane $A --line 2 --column 16"*) pass "人向けの表示に押したときと同じコマンド" ;;
  *) fail "人向けの表示: $HUMAN" ;;
esac

# 識別子でない位置: 字下げ・記号・数値・空行
for pos in "2 1" "2 14" "2 23" "4 0"; do
  set -- $pos
  R="$(menu --pane "$A" --line "$1" --column "$2")"
  check_eq "識別子でない位置 $1:$2 は not-symbol 0 件" "not-symbol " \
    "$(printf '%s' "$R" | json '"%s %s" % (d["status"], ",".join(i["id"] for i in d["items"]))')"
done
BAD="$("$TAKO_BIN" lsp menu --pane "$A" --line 9 --column 0 2>&1)"
case "$BAD" in *"行目は無い"*) pass "無い行は丸めずに拒否" ;; *) fail "無い行: $BAD" ;; esac

# 未導入（pyright の差し替え口が実行できないもの）・Markdown
B="$(open_file "$P/tool.py")"
R="$(menu --pane "$B" --line 1 --column 5)"
check_eq "未導入は not-installed 0 件" "not-installed " \
  "$(printf '%s' "$R" | json '"%s %s" % (d["status"], ",".join(i["id"] for i in d["items"]))')"
M="$(open_file "$P/README.md")"
RM="$("$TAKO_BIN" lsp menu --pane "$M" --line 3 --column 0 2>&1)"
case "$RM" in *"コード表示のプレビューではない"*) pass "Markdown のレンダリング表示は理由つきで断る" ;; *) fail "Markdown: $RM" ;; esac

stop_isolated_gui "$APP_PID"
APP_PID=""

echo
if [ -n "${TAKO_1684_SKIP_REAL:-}" ]; then
  echo "== ④ 実の rust-analyzer: TAKO_1684_SKIP_REAL で飛ばした =="
elif [ -z "$REAL_RA" ]; then
  echo "== ④ 実の rust-analyzer: 無いので未実測 =="
else
  echo "== ④ 実の rust-analyzer（visual-test lsp-context-menu-real） =="
  run_visual lsp-context-menu-real "$TMP/real.log" "$TMP/data-real" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  RC=$?
  check_eq "節が終了コード 0 で終わる" "0" "$RC"
  if grep -q 'TAKO_VISUAL_1684_REAL: SKIPPED' "$TMP/real.log"; then
    fail "SKIPPED（差し替え口を渡したのに見つからない）"
  elif grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/real.log"; then
    pass "実の rust-analyzer で右クリック → 項目 → 定義へ移動"
  else
    fail "OK 行が出ない"
    grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/real.log" | sed 's/^/    /'
  fi
  grep 'TAKO_VISUAL_PIXEL: lsp-context-menu-real' "$TMP/real.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
fi

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
