#!/bin/bash
# test-lsp-rename-1686.sh — 名前の変更（#1686）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1686 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 rename-1686（偽サーバ・実マウスと実キー）: 右クリックの「名前の変更…」→ 欄 →
#      打って Enter で語としての value だけが変わる → undo 1 回で元へ / 別のファイルに及ぶ名前は当てない / Esc
#   ② A/B: `TAKO_1686_LEGACY=1` で ① が名指しで落ちる（画像は before/ へ分ける）
#   ③ CLI / MCP（隔離 GUI へ実 CLI と `tako mcp serve`。偽サーバ + ディスクの模型）:
#      dry-run の後にファイルがバイト一致・編集モードに入らない / CLI と MCP の応答が字面一致 /
#      --yes で当たり undo 1 回で編集バッファの差分が 0 / 別のファイルに及ぶ名前は --yes でも当てない /
#      境界（名前が空・不正な識別子・prepareRename の拒否・能力なし）
#   ④ 実の rust-analyzer（PATH に無ければ未実測）: 小さなプロジェクトで ③ と同じ手順を通し、visual-test
#      rename-1686-real（実マウス・実キー → undo）を撮る。測り終えたらサーバを止める
#
# 使い方: bash scripts/test-lsp-rename-1686.sh
#   画像を残すときは TAKO_1686_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
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
# toolchain を見失う = #1685 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

TMP="$(mktemp -d /tmp/tako-1686-XXXXXX)"
DUMP="${TAKO_1686_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1686-$$"
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
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test \
  && run_heavy cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
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
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 600 秒）。
# 画像は既定で after/ へ（旧挙動の腕は呼び出し側が before/ を渡す = 同じ名前で上書きしない）
run_section() {
  local log="$1" section="$2"
  shift 2
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" \
    TAKO_VISUAL_DUMP_DIR="$DUMP/after" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 6000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  return 0
}

echo
echo "== ① visual-test rename-1686（右クリック → 欄 → 実キーで打って Enter → undo 1 回 / 別のファイル / Esc） =="
run_section "$TMP/vt.log" rename-1686
grep "TAKO_VISUAL_PIXEL: rename-1686" "$TMP/vt.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt.log"; then
  pass "rename-1686 の全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "rename-1686 が落ちた"
  grep -E "FAILED|panicked" "$TMP/vt.log" | head -5 | sed 's/^/    /'
fi

echo
echo "== ② A/B: TAKO_1686_LEGACY=1 では ① が名指しで落ちる =="
run_section "$TMP/vt-legacy.log" rename-1686 TAKO_1686_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
grep -E "TAKO_VISUAL_PIXEL: rename-1686 ①" "$TMP/vt-legacy.log" | head -1 | sed 's/^/    旧腕: /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-legacy.log"; then
  fail "旧腕でも緑（検出力が無い）"
else
  NAMED="$(grep -E "FAILED.*rename-1686 ①|rename-1686 ①.*FAILED" "$TMP/vt-legacy.log" | head -1)"
  if [ -n "$NAMED" ]; then pass "旧腕は ① で名指しの FAILED（$(printf '%s' "$NAMED" | cut -c1-120)）"; else fail "旧腕が ① 以外で落ちた"; grep -E "FAILED|panicked" "$TMP/vt-legacy.log" | head -3 | sed 's/^/    /'; fi
fi

# --- ③ CLI / MCP（偽サーバ + ディスクの模型） ---------------------------------------
# 1 つの素材で偽サーバと実サーバの両方を通す（`value` は main.rs だけ・`helper` は other.rs にも在る）
make_project() {
  local p="$1"
  mkdir -p "$p/src"
  printf '[package]\nname = "p"\nversion = "0.1.0"\nedition = "2021"\n' > "$p/Cargo.toml"
  {
    printf 'mod other;\n'
    printf 'struct Value;\n'
    printf 'impl Value {\n'
    printf '    fn new() -> Self {\n'
    printf '        Value\n'
    printf '    }\n'
    printf '}\n'
    printf 'fn main() {\n'
    printf '    let value = Value::new();\n'
    printf '    let values = 1;\n'
    printf '    drop((value, values));\n'
    printf '    other::helper();\n'
    printf '}\n'
  } > "$p/src/main.rs"
  printf 'pub fn helper() {}\n' > "$p/src/other.rs"
}

lsp() { "$TAKO_BIN" lsp "$@" --json 2>/dev/null; }
MCP_SOCKET=""
MCP_TOKEN=""
mcp() { # 引数の JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1686","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_lsp\",\"arguments\":$1}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"]); break'
}
same_json() { python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]) == json.loads(sys.argv[2]) else 1)' "$1" "$2"; }

# GUI を立てて main.rs を開く（閲覧のまま）。SRC にペインを入れる
open_project() {
  local log="$1" p="$2"
  shift 2
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || exit 1
  MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  local root
  root="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$root" ] || { echo "ルートペインを採れない"; exit 1; }
  SRC="$("$TAKO_BIN" open "$p/src/main.rs" --pane "$root" --right 2>/dev/null | json 'd["pane"]')"
  [ -n "$SRC" ] || { echo "main.rs を開けない"; exit 1; }
}

# ③ と ④ が共有する手順（$1 = 見出しの接頭辞・$2 = プロジェクト）
rename_steps() {
  local tag="$1" p="$2"
  local before after
  before="$(sha "$p/src/main.rs")"
  echo
  echo "== $tag-1 dry-run（--yes なし）: 件数と場所を返し、ファイルはバイト一致・編集モードに入らない =="
  D1="$(lsp rename --pane "$SRC" --line 9 --column 8 --new-name item)"
  check_eq "status / applicable / applied" "dry-run True False" \
    "$(printf '%s' "$D1" | json 'd["status"]+" "+str(d["applicable"])+" "+str(d["applied"])')"
  check_eq "ファイル数・箇所数" "1 2" "$(printf '%s' "$D1" | json 'str(d["file_count"])+" "+str(d["edits"])')"
  check_eq "今の名前（placeholder。範囲だけを返すサーバは範囲の字面）" "value" "$(printf '%s' "$D1" | json 'd["placeholder"]')"
  check_eq "場所（行:桁:前）= Value::new と values を含まない" "9:8:value 11:10:value" \
    "$(printf '%s' "$D1" | json '" ".join(str(x["line"])+":"+str(x["column"])+":"+x["old"] for x in d["files"][0]["places"])')"
  after="$(sha "$p/src/main.rs")"
  check_eq "dry-run の後に main.rs がバイト一致（sha256）" "$before" "$after"
  check_eq "編集モードに入らない" "False" "$("$TAKO_BIN" edit status --pane "$SRC" 2>/dev/null | json 'str(d["editing"])')"
  "$TAKO_BIN" lsp rename --pane "$SRC" --line 9 --column 8 --new-name item | sed 's/^/    /'
  MCP="$(mcp "{\"action\":\"rename\",\"pane\":$SRC,\"line\":9,\"column\":8,\"new_name\":\"item\"}")"
  if same_json "$D1" "$MCP"; then pass "dry-run が CLI と MCP で字面一致"; else fail "dry-run が食い違う: CLI=$D1 MCP=$MCP"; fi

  echo
  echo "== $tag-2 --yes: 編集バッファへ当たり（保存しない）、undo 1 回で差分 0 =="
  Y="$(lsp rename --pane "$SRC" --line 9 --column 8 --new-name item --yes)"
  check_eq "status / applied / changes / undo_depth" "renamed True 2 1" \
    "$(printf '%s' "$Y" | json 'd["status"]+" "+str(d["applied"])+" "+str(d["changes"])+" "+str(d["document"]["undo_depth"])')"
  check_eq "ディスクは書き換えない" "$before" "$(sha "$p/src/main.rs")"
  DIFF="$("$TAKO_BIN" edit diff --pane "$SRC" 2>/dev/null)"
  check_eq "編集バッファの差分（- / + の行）" "-    let value = Value::new();|+    let item = Value::new();|-    drop((value, values));|+    drop((item, values));" \
    "$(printf '%s' "$DIFF" | python3 -c 'import json,sys; d=json.load(sys.stdin); print("|".join(l for l in d["diff"]["unified"].splitlines() if l[:1] in "+-" and l[:3] not in ("+++","---")))' 2>/dev/null)"
  "$TAKO_BIN" edit undo --pane "$SRC" >/dev/null 2>&1
  check_eq "undo 1 回で差分 0（編集バッファがディスクとバイト一致）" "True 0" \
    "$("$TAKO_BIN" edit diff --pane "$SRC" 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(str(d["diff"]["identical"])+" "+str(len([l for l in d["diff"]["unified"].splitlines() if l[:1] in "+-" and l[:3] not in ("+++","---")])))' 2>/dev/null)"

  echo
  echo "== $tag-3 別のファイルに及ぶ名前（helper = other.rs にも在る）は --yes でも当てない =="
  O="$(lsp rename --pane "$SRC" --line 12 --column 11 --new-name assist --yes)"
  check_eq "status / blocked / applied" "not-applied other-files False" \
    "$(printf '%s' "$O" | json 'd["status"]+" "+d["blocked"]+" "+str(d["applied"])')"
  check_eq "ファイル数" "2" "$(printf '%s' "$O" | json 'str(d["file_count"])')"
  echo "    理由: $(printf '%s' "$O" | json 'd["reason"]')"
  echo "    次の一手: $(printf '%s' "$O" | json 'd["next_step"]')"
  check_eq "編集バッファは変わらない（差分 0）" "True 0" \
    "$("$TAKO_BIN" edit diff --pane "$SRC" 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(str(d["diff"]["identical"])+" "+str(len([l for l in d["diff"]["unified"].splitlines() if l[:1] in "+-" and l[:3] not in ("+++","---")])))' 2>/dev/null)"
  check_eq "other.rs もバイト一致" "pub fn helper() {}" "$(cat "$p/src/other.rs")"

  echo
  echo "== $tag-4 境界: 名前が空・不正な識別子・名前を変えられない位置（行頭の空白） =="
  E="$("$TAKO_BIN" lsp rename --pane "$SRC" --line 9 --column 8 --new-name '' 2>&1)"
  echo "    空: $E"
  case "$E" in *new_name*) pass "空の名前はサーバへ問う前に断る" ;; *) fail "空の名前を断らない: $E" ;; esac
  R="$(lsp rename --pane "$SRC" --line 9 --column 8 --new-name 1abc)"
  check_eq "不正な識別子は refused（サーバの文つき）" "refused" "$(printf '%s' "$R" | json 'd["status"]')"
  echo "    理由: $(printf '%s' "$R" | json 'd["reason"]')"
  K="$(lsp rename --pane "$SRC" --line 9 --column 2 --new-name item --yes)"
  echo "    行頭の空白: status=$(printf '%s' "$K" | json 'd["status"]') reason=$(printf '%s' "$K" | json 'd.get("reason","")')"
  check_eq "名前を変えられない位置は rejected（prepareRename の断り = rename を送らない・当てない）" "rejected False" \
    "$(printf '%s' "$K" | json 'd["status"]+" "+str(d["applied"])')"
  check_eq "どの境界の後も main.rs はバイト一致" "$before" "$(sha "$p/src/main.rs")"
}

P="$TMP/proj"
make_project "$P"
PROVIDERS="$TMP/providers.json"
printf '{"renameProvider":{"prepareProvider":true},"hoverProvider":true}' > "$PROVIDERS"
printf '[]' > "$TMP/rename.json"
echo
echo "== ③ 起動（偽サーバを rust-analyzer の代わりに・ディスクの模型で other.rs も知る） =="
open_project "$TMP/app.log" "$P" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_RENAME="$TMP/rename.json" \
  TAKO_LSP_FAKE_DISK_MODEL=1 \
  TAKO_LSP_FAKE_PROVIDERS="@$PROVIDERS"
echo "  pid=$APP_PID source=$SRC"
rename_steps "③" "$P"

echo
echo "== ③-5 能力なし（renameProvider を申告しないサーバ）は unsupported で問わない =="
printf '{"hoverProvider":true}' > "$PROVIDERS"
"$TAKO_BIN" lsp restart >/dev/null 2>&1
U="$(lsp rename --pane "$SRC" --line 9 --column 8 --new-name item)"
check_eq "status" "unsupported" "$(printf '%s' "$U" | json 'd["status"]')"
echo "    理由: $(printf '%s' "$U" | json 'd["reason"]')"
"$TAKO_BIN" lsp stop >/dev/null 2>&1
stop_isolated_gui "$APP_PID"; APP_PID=""

# --- ④ 実の rust-analyzer ---------------------------------------------------------
echo
if [ -z "$REAL_RA" ]; then
  echo "  [--] 未実測: rust-analyzer が PATH に無い"
else
  R4="$TMP/real"
  make_project "$R4"
  echo "== ④ 実の rust-analyzer（$(RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" "$REAL_RA" --version 2>/dev/null | head -1)） =="
  open_project "$TMP/real.log" "$R4" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" CARGO_BUILD_JOBS=2 \
    TAKO_LSP_RENAME_TIMEOUT_SECS=300
  # 読み込みが済むまで待つ（初回は cargo metadata と std の索引）
  T0="$(date +%s)"
  for i in $(seq 1 300); do
    W="$(lsp rename --pane "$SRC" --line 9 --column 8 --new-name item | json 'd["status"]')"
    [ "$W" = "dry-run" ] && break
    sleep 1
  done
  echo "    暖機: $(($(date +%s) - T0)) 秒（最後の status=${W}）"
  rename_steps "④" "$R4"
  "$TAKO_BIN" lsp stop >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"; APP_PID=""

  echo
  echo "== ④-5 visual-test rename-1686-real（実マウス・実キー → undo 1 回） =="
  run_section "$TMP/vt-real.log" rename-1686-real \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" CARGO_BUILD_JOBS=2 \
    TAKO_LSP_RENAME_TIMEOUT_SECS=300
  grep -E "TAKO_VISUAL_PIXEL: rename-1686-real|TAKO_VISUAL_1686_REAL|TAKO_VISUAL_DUMP_FILE: .*rename-1686-real" "$TMP/vt-real.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-real.log" && ! grep -q "TAKO_VISUAL_1686_REAL: SKIPPED" "$TMP/vt-real.log"; then
    pass "rename-1686-real の全相が緑"
  else
    fail "rename-1686-real が落ちた / SKIPPED"
    grep -E "FAILED|panicked|SKIPPED" "$TMP/vt-real.log" | head -5 | sed 's/^/    /'
  fi
fi

echo
echo "画像: ${DUMP}（after = 新しい挙動 / before = TAKO_1686_LEGACY=1）"
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
