#!/bin/bash
# test-lsp-completion-1682.sh — 補完（予測変換。#1682）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1682 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 completion（偽サーバが 1000 件返す・実 GUI の打鍵経路）: 1 文字で一覧が出る /
#      1000 件でも組む行は可視ぶんだけ / 基準画像（一覧だけを外した 1 枚）との差分が一覧の矩形の中だけ /
#      ↓ ↑ と送り / 選んだ候補の説明を resolve で補う / Esc は一覧だけを閉じる / 打ち足すとその場で絞り直す /
#      Enter・Tab の確定と undo 1 回 / ⌘F（検索バー）と同時表示の Esc の順 / 0 件 / 問い合わせ中にペインを閉じる
#   ② A/B: `TAKO_1682_LEGACY=1`（打鍵で問い合わせない = #1682 前）と
#      `TAKO_1682_NO_VIRTUAL_LIST=1`（全件ぶん element を組む）で同じ節が名指しで落ちる
#   ③ 実の rust-analyzer（visual-test 節 completion-real）: `s.le` で len が出て Enter で入る
#      （rust-analyzer が無ければ SKIPPED と出す = 未実測）
#   ④ CLI / MCP（隔離 GUI へ実 CLI と `tako mcp serve`）: --limit 20 → 20 件 + 切った件数 / MCP と字面一致 /
#      --choice で確定・undo 1 回で戻る / --resolve / 0 件 / 未導入 / 対象外 / 文字の途中は拒否
#
# 使い方: bash scripts/test-lsp-completion-1682.sh
#   画像を残すときは TAKO_1682_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
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
# toolchain を見失う = #1679 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

TMP="$(mktemp -d /tmp/tako-1682-XXXXXX)"
DUMP="${TAKO_1682_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1682-$$"
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
echo "== ① visual-test 節 completion（偽サーバ 1000 件） =="
run_section "$TMP/new.log" completion
grep "TAKO_VISUAL_PIXEL: completion" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi

echo
echo "== ② A/B: 旧挙動と全件ぶん組む形で同じ節が落ちる =="
for arm in "TAKO_1682_LEGACY=1" "TAKO_1682_NO_VIRTUAL_LIST=1"; do
  log="$TMP/ab-${arm%%=*}.log"
  run_section "$log" completion "$arm"
  grep "TAKO_VISUAL_PIXEL: completion" "$log" | head -3 | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test completion" "$log"; then
    pass "$arm では落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-140)"
  else
    fail "$arm でも落ちない（節に検出力が無い）"
  fi
done

echo
echo "== ③ 実の rust-analyzer（visual-test 節 completion-real） =="
if [ -n "$REAL_RA" ] && [ -n "$REAL_RUSTUP_HOME" ]; then
  run_section "$TMP/real.log" completion-real \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  # before（#1682 前 = 打鍵で問い合わせない）: 同じ実サーバ・同じ打鍵で一覧が出ない
  run_section "$TMP/real-legacy.log" completion-real TAKO_1682_LEGACY=1 \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test completion-real: 実サーバの候補の一覧が出る" "$TMP/real-legacy.log"; then
    pass "before（TAKO_1682_LEGACY=1）では実サーバでも一覧が出ない"
  else
    fail "before でも一覧が出る（または別の理由で落ちた）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/real-legacy.log" | head -1)"
  fi
else
  echo "TAKO_VISUAL_1682_REAL: SKIPPED（rust-analyzer が無い）" > "$TMP/real.log"
fi
grep -E "TAKO_VISUAL_PIXEL: completion-real|TAKO_VISUAL_1682_REAL" "$TMP/real.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_1682_REAL: SKIPPED" "$TMP/real.log"; then
  echo "  [--] 未実測: rust-analyzer が PATH に無い"
elif grep -q "TAKO_VISUAL_TEST_OK" "$TMP/real.log"; then
  pass "実サーバの候補が出て Enter で入る"
else
  fail "実サーバの節が緑にならない"
  grep -E "FAILED|panicked" "$TMP/real.log" | tail -3 | sed 's/^/    /'
fi
ls "$DUMP"/completion*.png 2>/dev/null | sed 's/^/    画像: /'

echo
echo "== ④ CLI / MCP（隔離 GUI） =="
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
printf 'fn main() {\n    let total = 1;\n    ca\n    let 名前 = 2;\n}\n' > "$P/src/main.rs"
printf 'x = 1\n' > "$P/src/tool.py"
printf 'plain text\n' > "$P/notes.txt"
RULES="$TMP/completion.json"
printf '{"generate": 1000, "word_edit": true, "resolve_doc": "doc of {label}"}\n' > "$RULES"
launch_isolated_gui "$TMP/app.log" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_BIN_PYRIGHT="$TMP/no-such-pyright" \
  TAKO_LSP_FAKE_COMPLETION="$RULES" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
SRC="$("$TAKO_BIN" open "$P/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$SRC" ] || { echo "main.rs を開けない"; exit 1; }
"$TAKO_BIN" edit start --pane "$SRC" >/dev/null
comp() { "$TAKO_BIN" lsp completion "$@" --json 2>/dev/null; }

R1="$(comp --pane "$SRC" --line 3 --column 6 --limit 20)"
check_eq "status" "found" "$(printf '%s' "$R1" | json 'd["status"]')"
check_eq "total（絞り込み後の全件）" "1000" "$(printf '%s' "$R1" | json 'd["total"]')"
check_eq "--limit 20 で 20 件" "20" "$(printf '%s' "$R1" | json 'len(d["items"])')"
check_eq "切った件数" "980" "$(printf '%s' "$R1" | json 'd["truncated"]')"
RANGE_EXPR='"%d:%d-%d:%d" % tuple(d["items"][0]["range"][k][f] for k in ("start", "end") for f in ("line", "column"))'
check_eq "範囲は語の頭から（行 3 桁 4〜6）" "3:4-3:6" "$(printf '%s' "$R1" | json "$RANGE_EXPR")"

MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数の JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1682","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp\",\"arguments\":$1}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }
M1="$(mcp "{\"action\":\"completion\",\"pane\":$SRC,\"line\":3,\"column\":6,\"limit\":20}")"
if same_json "$R1" "$M1"; then pass "CLI と MCP の答えが字面一致（20 件）"; else fail "CLI と MCP が食い違う"; fi

R2="$(comp --pane "$SRC" --line 3 --column 6 --limit 3 --resolve)"
check_eq "--resolve で返す候補の説明を補う" "doc of cand0000" "$(printf '%s' "$R2" | json 'd["items"][0]["documentation"]')"

R3="$(comp --pane "$SRC" --line 3 --column 6 --choice 3)"
check_eq "--choice で確定" "applied" "$(printf '%s' "$R3" | json 'd["status"]')"
"$TAKO_BIN" edit save --pane "$SRC" >/dev/null
check_eq "3 番目の候補が語を置き換える" "    cand0002" "$(sed -n 3p "$P/src/main.rs")"
"$TAKO_BIN" edit undo --pane "$SRC" >/dev/null
"$TAKO_BIN" edit save --pane "$SRC" >/dev/null
check_eq "undo 1 回で戻る" "    ca" "$(sed -n 3p "$P/src/main.rs")"

R4="$(comp --pane "$SRC" --line 2 --column 13)"
check_eq "合う候補が無ければ none" "none" "$(printf '%s' "$R4" | json 'd["status"]')"
PY="$("$TAKO_BIN" open "$P/src/tool.py" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
R5="$(comp --pane "$PY" --line 1 --column 1)"
check_eq "未導入" "not-installed" "$(printf '%s' "$R5" | json 'd["status"]')"
check_eq "未導入は導入コマンドを返す" "True" "$(printf '%s' "$R5" | json 'bool(d.get("install_command"))')"
TXT="$("$TAKO_BIN" open "$P/notes.txt" --pane "$ROOT" --down 2>/dev/null | json 'd["pane"]')"
R6="$(comp --pane "$TXT" --line 1 --column 1)"
check_eq "受け持つサーバが無い" "no-server" "$(printf '%s' "$R6" | json 'd["status"]')"
MID="$("$TAKO_BIN" lsp completion --pane "$SRC" --line 4 --column 9 2>&1)"
case "$MID" in *"境界"*) pass "文字の途中の桁は丸めずに拒否" ;; *) fail "文字の途中の桁: $MID" ;; esac
LIMIT0="$("$TAKO_BIN" lsp completion --pane "$SRC" --line 3 --column 6 --limit 0 2>&1)"
case "$LIMIT0" in *"limit"*) pass "--limit 0 は拒否" ;; *) fail "--limit 0: $LIMIT0" ;; esac
HUMAN="$("$TAKO_BIN" lsp completion --pane "$SRC" --line 3 --column 6 --limit 2 2>/dev/null | head -1)"
check_eq "人向けの 1 行目" "found total=1000 shown=2 truncated=998" "$HUMAN"

echo
echo "== 参考: main の CLI（/Applications の tako）には completion が無い =="
if [ -x /Applications/tako.app/Contents/MacOS/tako ]; then
  /Applications/tako.app/Contents/MacOS/tako lsp completion --line 1 --column 0 2>&1 | head -2 | sed 's/^/    /'
fi

stop_isolated_gui "$APP_PID"; APP_PID=""
echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
