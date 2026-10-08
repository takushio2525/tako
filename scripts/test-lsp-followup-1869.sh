#!/bin/bash
# test-lsp-followup-1869.sh — LSP の続き（#1869）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1869 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① visual-test 節 completion-loading（偽サーバ = 読み込み 6 秒・その間は補完に null で即答）:
#      読み込み中に打つと「読み込み中」の 1 行が出る / 基準画像との差分がその矩形の中だけ /
#      5 キーを奪わない / 語の外へ出ると消える / 打ち足さずに待つと済んだ後に一覧が出る
#   ② A/B: `TAKO_1869_LEGACY=1`（#1869 前）で ① と ③ が名指しで落ちる
#   ③ 10 万行（9.6 MB）の整形 → undo（CLI。偽サーバの既定の整形 = 10 万行の行末の空白を消す）:
#      undo の予算（8 MiB）に収まる・その前の打鍵の履歴も残る（深さ 2）・undo 1 回で整形の前と
#      バイト一致・もう 1 回で元のファイルとバイト一致
#   ④ CLI / MCP: 読み込み中に頼むと待って一覧を返し `waited_for_loading_ms` が載る / MCP と字面一致 /
#      終わらない読み込みは上限で `status: loading`（理由・次の一手）/ `tako lsp status` の `loading`
#   ⑤ エッジ: 空の文書の整形 / 空の答え / 重なる答え（本文を触らない）/ 読み込み中にペインを閉じる
#   ⑥ 実の rust-analyzer（無ければ「未実測」と出す）: 整形が rustfmt とバイト一致して undo 1 回で戻る /
#      編集を始めた直後の CLI の補完が読み込みを待って答える / visual-test 節 completion-real（`s.le` → len）/
#      同じ節を暖機なし（TAKO_1869_REAL_NO_WARM=1 = 読み込みの最中に GUI で打つ）で回して一覧が出る
#
# 使い方: bash scripts/test-lsp-followup-1869.sh
#   画像を残すときは TAKO_1869_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）
#   実サーバの節を飛ばすときは TAKO_1869_SKIP_REAL=1
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
REAL_RUSTFMT="$(command -v rustfmt 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
REAL_TOOLCHAIN="$(cd "$REPO_ROOT" && rustup show active-toolchain 2>/dev/null | awk '{print $1}')"

TMP="$(mktemp -d /tmp/tako-1869-XXXXXX)"
DUMP="${TAKO_1869_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1869-$$"
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
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
same_file() { cmp -s "$1" "$2"; }
version() { "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | json 'd["document"]["version"]'; }
doc() { "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | json "d[\"document\"][\"$2\"]"; }
autosave_off() { "$TAKO_BIN" edit autosave false --pane "$1" >/dev/null 2>&1; }
open_code() { "$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]'; }
completion() { "$TAKO_BIN" lsp completion "$@" --json 2>/dev/null; }

mcp() { # 引数のツール名と JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1869","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        r = m.get("result")
        print(r["content"][0]["text"] if r else json.dumps(m.get("error"), ensure_ascii=False)); break'
}

# 隔離 GUI を立てて CLI が繋がるまで待つ（引数は GUI へ渡す env）。ROOT / MCP_* を決める
start_gui() {
  local log="$1"
  shift
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
  MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
}

stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

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

# 10 万行（#1660 の上限 10 MB / 10 万行の付近 = 9.6 MB）。行末に空白 2 つ（偽サーバの整形が消す）
P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
make_big() {
  awk 'BEGIN { for (i = 0; i < 100000; i++) printf "    let value_%06d = compute(alpha, beta, gamma, delta, epsilon, zeta, eta, theta);  \n", i }' > "$1"
}

echo
echo "== ① visual-test 節 completion-loading（偽サーバ・読み込み 6 秒） =="
run_section "$TMP/loading.log" completion-loading
grep "TAKO_VISUAL_PIXEL: completion-loading" "$TMP/loading.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/loading.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/loading.log" | tail -5 | sed 's/^/    /'
fi

echo
echo "== ② A/B: TAKO_1869_LEGACY=1（#1869 前）で同じ節が名指しで落ちる =="
run_section "$TMP/loading-legacy.log" completion-loading TAKO_1869_LEGACY=1
grep "TAKO_VISUAL_PIXEL: completion-loading" "$TMP/loading-legacy.log" | head -3 | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test completion-loading" "$TMP/loading-legacy.log"; then
  pass "旧挙動では落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/loading-legacy.log" | head -1 | cut -c1-160)"
else
  fail "旧挙動でも落ちない（節に検出力が無い）"
fi

# ③ を 1 回ぶん測る。引数 = ラベル、続きは GUI へ渡す env
measure_big_format() {
  local label="$1"
  shift
  start_gui "$TMP/app-big-$label.log" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
    TAKO_LSP_FORMAT_TIMEOUT_SECS=120 ${1+"$@"}
  make_big "$P/src/big.rs"
  cp "$P/src/big.rs" "$TMP/big.orig"
  local pane t0 t1 out undo_ms
  pane="$(open_code "$P/src/big.rs")"
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null
  autosave_off "$pane"
  # 整形の前の打鍵 1 塊（これが整形の後も戻れること = 履歴が捨てられていない）
  "$TAKO_BIN" edit replace-range 1:0 1:0 "//" --pane "$pane" >/dev/null
  "$TAKO_BIN" edit save --pane "$pane" >/dev/null 2>&1
  cp "$P/src/big.rs" "$TMP/big.typed"
  t0="$(now_ms)"
  out="$("$TAKO_BIN" lsp format --pane "$pane" --json 2>/dev/null)"
  t1="$(now_ms)"
  BIG_STATUS="$(printf '%s' "$out" | json 'd["status"]')"
  BIG_CHANGES="$(printf '%s' "$out" | json 'd["changes"]')"
  BIG_DEPTH="$(doc "$pane" undo_depth)"
  BIG_BYTES="$(doc "$pane" undo_history_bytes)"
  "$TAKO_BIN" edit save --pane "$pane" >/dev/null 2>&1
  BIG_FORMATTED_BYTES="$(wc -c < "$P/src/big.rs" | tr -d ' ')"
  t0u="$(now_ms)"
  "$TAKO_BIN" edit undo --pane "$pane" >/dev/null
  undo_ms=$(( $(now_ms) - t0u ))
  "$TAKO_BIN" edit save --pane "$pane" >/dev/null 2>&1
  BIG_UNDO1="ng"; same_file "$P/src/big.rs" "$TMP/big.typed" && BIG_UNDO1="ok"
  "$TAKO_BIN" edit undo --pane "$pane" >/dev/null
  "$TAKO_BIN" edit save --pane "$pane" >/dev/null 2>&1
  BIG_UNDO2="ng"; same_file "$P/src/big.rs" "$TMP/big.orig" && BIG_UNDO2="ok"
  echo "    [$label] status=$BIG_STATUS changes=$BIG_CHANGES format=$((t1 - t0))ms undo=${undo_ms}ms undo_depth=$BIG_DEPTH undo_history_bytes=$BIG_BYTES formatted_file=${BIG_FORMATTED_BYTES}B undo1=$BIG_UNDO1 undo2=$BIG_UNDO2"
  stop_gui
}

echo
echo "== ③ 10 万行の整形 → undo（予算 8 MiB に収まり、その前の履歴も残る） =="
measure_big_format new
check_eq "整形した" "formatted" "$BIG_STATUS"
check_eq "書き換えた箇所は 10 万" "100000" "$BIG_CHANGES"
check_eq "undo の深さ = 打鍵 1 + 整形 1（その前の履歴が残る）" "2" "$BIG_DEPTH"
if [ -n "$BIG_BYTES" ] && [ "$BIG_BYTES" -le 8388608 ]; then
  pass "undo の履歴は予算内（${BIG_BYTES} <= 8388608）"
else
  fail "undo の履歴が予算を超えた（${BIG_BYTES}）"
fi
check_eq "整形後のファイルは行末の空白 2 つ × 10 万を消した長さ" "$(( $(wc -c < "$TMP/big.typed" | tr -d ' ') - 200000 ))" "$BIG_FORMATTED_BYTES"
check_eq "undo 1 回で整形の前とバイト一致" "ok" "$BIG_UNDO1"
check_eq "もう 1 回で元のファイルとバイト一致（その前の打鍵も戻れる）" "ok" "$BIG_UNDO2"
echo "  -- A/B: TAKO_1869_LEGACY=1（先頭〜末尾の 1 か所で当てる）"
measure_big_format legacy TAKO_1869_LEGACY=1
if [ "$BIG_DEPTH" != "2" ] || [ "${BIG_BYTES:-0}" -gt 8388608 ]; then
  pass "旧挙動では落ちる: undo_depth=$BIG_DEPTH undo_history_bytes=${BIG_BYTES}（予算超えで前の履歴を捨てる）"
else
  fail "旧挙動でも予算内に収まる（検査に検出力が無い）"
fi

echo
echo "== ④ CLI / MCP: 読み込み中に頼むと待って答え、終わらなければ loading =="
printf 'fn main() { ab }\n' > "$P/src/main.rs"
RULES="$TMP/completion.json"
printf '{ "items": [{ "label": "abc" }, { "label": "abd" }] }' > "$RULES"
start_gui "$TMP/app-cli.log" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_SCENARIO=loading TAKO_LSP_FAKE_LOADING_MS=3000 \
  TAKO_LSP_FAKE_COMPLETION="$RULES" TAKO_LSP_COMPLETION_TIMEOUT_SECS=20
A="$(open_code "$P/src/main.rs")"
"$TAKO_BIN" edit start --pane "$A" >/dev/null
autosave_off "$A"
LOADING_NOW="$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["servers"][0]["loading"]')"
t0="$(now_ms)"
R1="$(completion --pane "$A" --line 1 --column 14)"
t1="$(now_ms)"
echo "    読み込み中の CLI: $((t1 - t0))ms status=$(printf '%s' "$R1" | json 'd["status"]') total=$(printf '%s' "$R1" | json 'd["total"]') waited_for_loading_ms=$(printf '%s' "$R1" | json 'd.get("waited_for_loading_ms")')"
check_eq "頼んだ時点の tako lsp status は loading" "True" "$LOADING_NOW"
check_eq "読み込みを待って一覧を返す" "found" "$(printf '%s' "$R1" | json 'd["status"]')"
check_eq "候補は 2 件" "2" "$(printf '%s' "$R1" | json 'd["total"]')"
WAITED="$(printf '%s' "$R1" | json 'd.get("waited_for_loading_ms")')"
case "$WAITED" in
  ''|None) fail "答えに waited_for_loading_ms が無い（読み込み中だったと分からない）" ;;
  *) pass "答えに waited_for_loading_ms=${WAITED}（読み込み中だったと分かる）" ;;
esac
check_eq "済んだ後の tako lsp status は loading でない" "False" "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["servers"][0]["loading"]')"
R2="$(completion --pane "$A" --line 1 --column 14)"
check_eq "済んだ後は待った欄を載せない（いつもの形）" "None" "$(printf '%s' "$R2" | json 'd.get("waited_for_loading_ms")')"
M2="$(mcp tako_lsp "{\"action\":\"completion\",\"pane\":$A,\"line\":1,\"column\":14}")"
check_eq "MCP と CLI の候補が字面一致" \
  "$(printf '%s' "$R2" | json 'json.dumps(d["items"], sort_keys=True)')" \
  "$(printf '%s' "$M2" | json 'json.dumps(d["items"], sort_keys=True)')"
stop_gui

echo "  -- 終わらない読み込み（上限 3 秒）"
start_gui "$TMP/app-never.log" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_SCENARIO=loading TAKO_LSP_FAKE_LOADING_MS=600000 \
  TAKO_LSP_FAKE_COMPLETION="$RULES" TAKO_LSP_COMPLETION_TIMEOUT_SECS=3
B="$(open_code "$P/src/main.rs")"
"$TAKO_BIN" edit start --pane "$B" >/dev/null
autosave_off "$B"
R3="$(completion --pane "$B" --line 1 --column 14)"
echo "    status=$(printf '%s' "$R3" | json 'd["status"]') reason=$(printf '%s' "$R3" | json 'd["reason"]')"
check_eq "上限まで読み込み中なら status: loading" "loading" "$(printf '%s' "$R3" | json 'd["status"]')"
check_eq "次の一手が載る" "True" "$(printf '%s' "$R3" | json 'bool(d.get("next_step"))')"
M3="$(mcp tako_lsp "{\"action\":\"completion\",\"pane\":$B,\"line\":1,\"column\":14}")"
check_eq "MCP も loading" "loading" "$(printf '%s' "$M3" | json 'd["status"]')"
check_eq "tako lsp status の loading は真のまま" "True" "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'd["servers"][0]["loading"]')"

echo
echo "== ⑤ エッジ =="
# 読み込み中にペインを閉じる: 待っている CLI の問い合わせは上限を待たずに抜ける
completion --pane "$B" --line 1 --column 14 > "$TMP/close.json" &
CLOSE_PID=$!
sleep 0.5
t0="$(now_ms)"
"$TAKO_BIN" close --pane "$B" --force >/dev/null 2>&1
wait "$CLOSE_PID"
t1="$(now_ms)"
echo "    閉じてから答えまで $((t1 - t0))ms status=$(json 'd["status"]' < "$TMP/close.json")"
case "$(json 'd["status"]' < "$TMP/close.json")" in
  loading) fail "閉じても上限まで待った（loading）" ;;
  '') fail "答えが無い" ;;
  *) pass "閉じたら待ちを抜ける（$(json 'd["status"]' < "$TMP/close.json")）" ;;
esac
stop_gui

FORMAT_RULES="$TMP/format.json"
cat > "$FORMAT_RULES" <<'JSON'
[
  { "uri_suffix": "noop.rs", "result": [] },
  { "uri_suffix": "overlap.rs", "result": [
    { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "X" },
    { "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 4 } }, "newText": "Y" }
  ] },
  { "uri_suffix": "empty.rs", "result": [
    { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": "fn main() {}\n" }
  ] }
]
JSON
start_gui "$TMP/app-edge.log" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" TAKO_LSP_FAKE_FORMAT="$FORMAT_RULES"
for name in noop overlap empty; do
  case "$name" in
    noop) printf 'fn a() {}\n' > "$P/src/$name.rs" ;;
    overlap) printf 'abcdef\n' > "$P/src/$name.rs" ;;
    empty) : > "$P/src/$name.rs" ;;
  esac
  cp "$P/src/$name.rs" "$TMP/$name.orig"
  E="$(open_code "$P/src/$name.rs")"
  "$TAKO_BIN" edit start --pane "$E" >/dev/null
  autosave_off "$E"
  V0="$(version "$E")"
  RE="$("$TAKO_BIN" lsp format --pane "$E" --json 2>/dev/null)"
  ST="$(printf '%s' "$RE" | json 'd["status"]')"
  echo "    $name: status=$ST version=${V0}→$(version "$E") undo_depth=$(doc "$E" undo_depth)"
  case "$name" in
    noop)
      check_eq "空の答えは unchanged" "unchanged" "$ST"
      check_eq "空の答えは版を進めない" "$V0" "$(version "$E")" ;;
    overlap)
      check_eq "重なる答えは invalid-edits" "invalid-edits" "$ST"
      check_eq "重なる答えは版も履歴も進めない" "$V0/0" "$(version "$E")/$(doc "$E" undo_depth)" ;;
    empty)
      check_eq "空の文書の整形" "formatted" "$ST"
      "$TAKO_BIN" edit save --pane "$E" >/dev/null 2>&1
      check_eq "空の文書へ入った" "fn main() {}" "$(cat "$P/src/$name.rs")"
      "$TAKO_BIN" edit undo --pane "$E" >/dev/null
      "$TAKO_BIN" edit save --pane "$E" >/dev/null 2>&1
      if same_file "$P/src/$name.rs" "$TMP/$name.orig"; then pass "undo 1 回で空へ戻る"; else fail "空へ戻らない"; fi ;;
  esac
done
stop_gui

echo
echo "== ⑥ 実の rust-analyzer =="
if [ -n "${TAKO_1869_SKIP_REAL:-}" ]; then
  echo "  [--] 未実測: TAKO_1869_SKIP_REAL=1"
elif [ -z "$REAL_RA" ] || [ -z "$REAL_RUSTFMT" ] || [ -z "$REAL_RUSTUP_HOME" ]; then
  echo "  [--] 未実測: rust-analyzer か rustfmt が PATH に無い"
else
  R="$TMP/real"
  mkdir -p "$R/src"
  printf '[package]\nname = "real"\nversion = "0.1.0"\nedition = "2021"\n' > "$R/Cargo.toml"
  printf 'fn main(){let x=vec![1,2,3];for i in x.iter(){println!("{}",i);}\n    let  名前 =  "日本語";println!("{}",名前);}\n\nfn   other( a:i32 ,b :i32)->i32{a+b}\n' > "$R/src/main.rs"
  cp "$R/src/main.rs" "$TMP/real.orig"
  cp "$R/src/main.rs" "$TMP/real.want"
  RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN" \
    "$REAL_RUSTFMT" --edition 2021 "$TMP/real.want" || echo "  rustfmt が期待値を作れない"
  # 補完の素材は別のクレートの main.rs（クレートに属さないファイルには rust-analyzer は null で答える =
  # 読み込みの問題と区別できない）
  RC_DIR="$TMP/realc"
  mkdir -p "$RC_DIR/src"
  printf '[package]\nname = "realc"\nversion = "0.1.0"\nedition = "2021"\n' > "$RC_DIR/Cargo.toml"
  printf 'fn main() {\n    let s = String::new();\n    s.le\n}\n' > "$RC_DIR/src/main.rs"
  start_gui "$TMP/app-real.log" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
    TAKO_LSP_FORMAT_TIMEOUT_SECS=120 TAKO_LSP_COMPLETION_TIMEOUT_SECS=120
  # 編集を始めた直後（起動・読み込みの最中）の CLI の補完: 読み込みを待って答える
  CP="$(open_code "$RC_DIR/src/main.rs")"
  "$TAKO_BIN" edit start --pane "$CP" >/dev/null
  autosave_off "$CP"
  t0="$(now_ms)"
  RC="$(completion --pane "$CP" --line 3 --column 8 --limit 5)"
  t1="$(now_ms)"
  echo "    編集を始めた直後の補完: $((t1 - t0))ms status=$(printf '%s' "$RC" | json 'd["status"]') waited_for_loading_ms=$(printf '%s' "$RC" | json 'd.get("waited_for_loading_ms")') 先頭=$(printf '%s' "$RC" | json '[i["label"] for i in d.get("items", [])][:3]')"
  check_eq "実サーバの補完（読み込みの最中に頼んでも答える）" "found" "$(printf '%s' "$RC" | json 'd["status"]')"
  check_eq "len が候補に在る" "True" "$(printf '%s' "$RC" | json 'any(i["label"].startswith("len") for i in d["items"])')"
  # 整形
  RP="$(open_code "$R/src/main.rs")"
  "$TAKO_BIN" edit start --pane "$RP" >/dev/null
  autosave_off "$RP"
  RR="$("$TAKO_BIN" lsp format --pane "$RP" --json 2>/dev/null)"
  echo "    整形: status=$(printf '%s' "$RR" | json 'd["status"]') changes=$(printf '%s' "$RR" | json 'd["changes"]') undo_depth=$(doc "$RP" undo_depth) undo_history_bytes=$(doc "$RP" undo_history_bytes)"
  check_eq "実サーバの整形" "formatted" "$(printf '%s' "$RR" | json 'd["status"]')"
  "$TAKO_BIN" edit save --pane "$RP" >/dev/null 2>&1
  if same_file "$R/src/main.rs" "$TMP/real.want"; then pass "実ファイルが rustfmt の出力とバイト一致"; else fail "rustfmt と違う: $(diff "$TMP/real.want" "$R/src/main.rs" | head -10)"; fi
  check_eq "整形は undo 1 件" "1" "$(doc "$RP" undo_depth)"
  "$TAKO_BIN" edit undo --pane "$RP" >/dev/null
  "$TAKO_BIN" edit save --pane "$RP" >/dev/null 2>&1
  if same_file "$R/src/main.rs" "$TMP/real.orig"; then pass "undo 1 回で整形の前とバイト一致（実サーバ）"; else fail "実サーバで undo 1 回で戻らない"; fi
  stop_gui
  # GUI の打鍵経路（#1682 の節をそのまま回す = 従来どおり動く）
  run_section "$TMP/real-visual.log" completion-real \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" \
    RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN"
  grep -E "TAKO_VISUAL_PIXEL: completion-real|TAKO_VISUAL_1682_REAL" "$TMP/real-visual.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/real-visual.log"; then
    pass "実サーバの候補が GUI の打鍵で出て Enter で入る（completion-real）"
  else
    fail "completion-real が緑にならない"
    grep -E "FAILED|panicked" "$TMP/real-visual.log" | tail -3 | sed 's/^/    /'
  fi
  # 読み込みの最中に GUI で打つ（暖機を省く）: 打ち足さずに待てば一覧が出る
  run_section "$TMP/real-nowarm.log" completion-real TAKO_1869_REAL_NO_WARM=1 \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" \
    RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN"
  grep -E "TAKO_VISUAL_PIXEL: completion-real (loading_at_typing|shown|accepted)" "$TMP/real-nowarm.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/real-nowarm.log"; then
    pass "読み込みの最中に打っても、済んだ後に実サーバの候補が出て Enter で入る"
  else
    fail "読み込みの最中に打つと一覧が出ない: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/real-nowarm.log" | head -1)"
  fi
  # 旧挙動（観測。打鍵が読み込みの前半に当たれば一覧は出ず、後半に当たれば出る = 時機で変わる）
  run_section "$TMP/real-nowarm-legacy.log" completion-real TAKO_1869_REAL_NO_WARM=1 TAKO_1869_LEGACY=1 \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" \
    RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN"
  grep -E "TAKO_VISUAL_PIXEL: completion-real (loading_at_typing|shown)" "$TMP/real-nowarm-legacy.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/real-nowarm-legacy.log"; then
    echo "  [--] 観測（旧挙動 TAKO_1869_LEGACY=1）: 一覧が出た（打鍵が読み込みの後半に当たった）"
  else
    echo "  [--] 観測（旧挙動 TAKO_1869_LEGACY=1）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/real-nowarm-legacy.log" | head -1)"
  fi
fi

if [ -n "${TAKO_1869_DUMP_DIR:-}" ]; then
  echo
  echo "画像: $DUMP"
  ls "$DUMP" 2>/dev/null | sed 's/^/    /'
fi
echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
