#!/bin/bash
# test-lsp-format-line-diff-1899.sh — 全文を 1 つの書き換えで返す整形を行ごとへ割る（#1899）の実経路テスト
#
# 隔離 GUI（仮想ディスプレイ tako-vd）を起こし、CLI（`tako lsp format`）→ dispatch → 編集バッファ
# （`TextBuffer::apply_changes`）の経路で測る。GUI の描画は見ない（蓋を閉じた機でも測れる）:
#   ① 偽サーバの規則 `whole`（全文 1 つの答え）で 10 万行（8.8 MB）を整形: `changes` が行ごとの数（10 万）・
#      undo の履歴が行ごとの差の合計程度・その前の打鍵の履歴も残る（深さ 2）・undo 1 回で整形の前と /
#      もう 1 回で元のファイルとバイト一致・整形の結果は行ごとの答え（偽サーバの既定）とバイト一致
#   ② A/B: `TAKO_1899_LEGACY=1`（#1899 前）で ① が落ちる（1 か所・本文 2 本ぶん・その前の履歴を捨てる）
#   ③ 境界（全文 1 つの答え）: 空のファイル・末尾に改行が無い・CRLF・整形しても同じ
#   ④ 実の rust-analyzer + rustfmt（無ければ「未実測」）: 整形が rustfmt とバイト一致・undo 1 回で戻る
#
# 使い方: bash scripts/test-lsp-format-line-diff-1899.sh（実サーバの節を飛ばすなら TAKO_1899_SKIP_REAL=1）
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
# toolchain を見失う = #1869 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTFMT="$(command -v rustfmt 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
REAL_TOOLCHAIN="$(cd "$REPO_ROOT" && rustup show active-toolchain 2>/dev/null | awk '{print $1}')"

TMP="$(mktemp -d /tmp/tako-1899-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1899-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# 隔離 GUI は言語サーバを自動では起こさない（#2048）。この検証は起こす前提なので明示する
export TAKO_LSP_AUTOSTART=1

echo "tako-app と偽サーバをビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
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
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
same_file() { cmp -s "$1" "$2"; }
doc() { "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | json "d[\"document\"][\"$2\"]"; }
autosave_off() { "$TAKO_BIN" edit autosave false --pane "$1" >/dev/null 2>&1; }
open_code() { "$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]'; }
fmt() { "$TAKO_BIN" lsp format --pane "$1" --json 2>/dev/null; }
save() { "$TAKO_BIN" edit save --pane "$1" >/dev/null 2>&1; }
undo() { "$TAKO_BIN" edit undo --pane "$1" >/dev/null; }
# 開いて編集を始める（自動保存は止める = 保存の検査が 500ms の自動保存と競わない）
open_edit() {
  local pane
  pane="$(open_code "$1")"
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null
  autosave_off "$pane"
  echo "$pane"
}
# 偽サーバの既定の整形（行末のスペースとタブを消す。改行は CRLF も保つ）を当てた期待値
expect_trailing() {
  python3 -c 'import re,sys
s = open(sys.argv[1], "rb").read()
open(sys.argv[2], "wb").write(re.sub(rb"[ \t]+(?=\r\n|\n|\Z)", b"", s))' "$1" "$2"
}

start_gui() {
  local log="$1"
  shift
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; exit 1; }
}

stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

P="$TMP/proj"
mkdir -p "$P/src"
printf '[package]\nname = "p"\n' > "$P/Cargo.toml"
# 名前が -whole.rs で終わるファイルは全文 1 つの答え・それ以外は偽サーバの既定（行ごとの答え）
RULES="$TMP/format.json"
printf '[{ "uri_suffix": "-whole.rs", "whole": true }]' > "$RULES"
# 10 万行（#1660 の上限 10 MB / 10 万行の付近 = 8.8 MB）。行末に空白 2 つ（偽サーバの整形が消す）
make_big() {
  awk 'BEGIN { for (i = 0; i < 100000; i++) printf "    let value_%06d = compute(alpha, beta, gamma, delta, epsilon, zeta, eta, theta);  \n", i }' > "$1"
}

# ① / ② を 1 回ぶん測る。引数 = ラベル、続きは GUI へ渡す env
measure_big() {
  local label="$1" pane line
  shift
  start_gui "$TMP/app-big-$label.log" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
    TAKO_LSP_FAKE_FORMAT="$RULES" TAKO_LSP_FORMAT_TIMEOUT_SECS=120 ${1+"$@"}
  make_big "$P/src/big-whole.rs"
  command cp -f "$P/src/big-whole.rs" "$TMP/big.orig"
  command cp -f "$P/src/big-whole.rs" "$P/src/big-line.rs"
  pane="$(open_edit "$P/src/big-whole.rs")"
  # 整形の前の打鍵 1 塊（これが整形の後も戻れること = 履歴が捨てられていない）
  "$TAKO_BIN" edit replace-range 1:0 1:0 "//" --pane "$pane" >/dev/null
  save "$pane"
  command cp -f "$P/src/big-whole.rs" "$TMP/big.typed"
  out="$(fmt "$pane")"
  BIG_STATUS="$(printf '%s' "$out" | json 'd["status"]')"
  BIG_CHANGES="$(printf '%s' "$out" | json 'd["changes"]')"
  BIG_DEPTH="$(doc "$pane" undo_depth)"
  BIG_BYTES="$(doc "$pane" undo_history_bytes)"
  save "$pane"
  command cp -f "$P/src/big-whole.rs" "$TMP/big.formatted"
  undo "$pane"
  save "$pane"
  BIG_UNDO1="ng"; same_file "$P/src/big-whole.rs" "$TMP/big.typed" && BIG_UNDO1="ok"
  undo "$pane"
  save "$pane"
  BIG_UNDO2="ng"; same_file "$P/src/big-whole.rs" "$TMP/big.orig" && BIG_UNDO2="ok"
  # 同じ本文を行ごとの答え（偽サーバの既定）で整形した結果と比べる
  line="$(open_edit "$P/src/big-line.rs")"
  "$TAKO_BIN" edit replace-range 1:0 1:0 "//" --pane "$line" >/dev/null
  BIG_LINE_CHANGES="$(fmt "$line" | json 'd["changes"]')"
  save "$line"
  BIG_SAME="ng"; same_file "$P/src/big-line.rs" "$TMP/big.formatted" && BIG_SAME="ok"
  echo "    [$label] 全文 1 つの答え: status=$BIG_STATUS changes=$BIG_CHANGES undo_depth=$BIG_DEPTH undo_history_bytes=$BIG_BYTES undo1=$BIG_UNDO1 undo2=$BIG_UNDO2 / 行ごとの答え: changes=$BIG_LINE_CHANGES 本文一致=$BIG_SAME"
  stop_gui
}

echo
echo "== ① 10 万行の全文 1 つの答え → 行ごとの差だけを積む（CLI） =="
measure_big new
check_eq "整形した" "formatted" "$BIG_STATUS"
check_eq "当てた箇所は行ごとの数（10 万）" "100000" "$BIG_CHANGES"
check_eq "undo の深さ = 打鍵 1 + 整形 1（その前の履歴が残る）" "2" "$BIG_DEPTH"
# 行ごとの差（2 バイト）+ 箇所の表（24 バイト）× 10 万 + 打鍵の塊ぶんの余裕
LIMIT=$((100000 * 26 + 4096))
if [ -n "$BIG_BYTES" ] && [ "$BIG_BYTES" -le "$LIMIT" ]; then
  pass "undo の履歴は行ごとの差の合計程度（${BIG_BYTES} <= ${LIMIT}）"
else
  fail "undo の履歴が行ごとの差の合計を超えた（${BIG_BYTES:-?} > ${LIMIT}）"
fi
check_eq "undo 1 回で整形の前とバイト一致" "ok" "$BIG_UNDO1"
check_eq "もう 1 回で元のファイルとバイト一致" "ok" "$BIG_UNDO2"
check_eq "整形の結果は行ごとの答えとバイト一致" "ok" "$BIG_SAME"
check_eq "行ごとの答えの箇所の数も 10 万" "100000" "$BIG_LINE_CHANGES"

echo
echo "== ② A/B: TAKO_1899_LEGACY=1（#1899 前）で ① が落ちる =="
measure_big legacy TAKO_1899_LEGACY=1
if [ "$BIG_CHANGES" = "1" ] && [ "${BIG_BYTES:-0}" -gt "$LIMIT" ]; then
  pass "旧挙動では落ちる: changes=$BIG_CHANGES undo_depth=$BIG_DEPTH undo_history_bytes=$BIG_BYTES"
else
  fail "旧挙動でも行ごとに収まる（検査に検出力が無い）: changes=$BIG_CHANGES bytes=${BIG_BYTES:-?}"
fi
check_eq "旧挙動でも整形の結果は同じ（変わるのは割り方だけ）" "ok" "$BIG_SAME"

echo
echo "== ③ 境界（全文 1 つの答え） =="
start_gui "$TMP/app-edge.log" TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
  TAKO_LSP_FAKE_FORMAT="$RULES" TAKO_LSP_FORMAT_TIMEOUT_SECS=60
printf '' > "$P/src/empty-whole.rs"
printf 'fn a() {  \n    x();  \n}  ' > "$P/src/nonl-whole.rs"
printf 'fn a() { \r\n    x(); \r\n}\r\n' > "$P/src/crlf-whole.rs"
printf 'fn a() {\n    x();\n}\n' > "$P/src/same-whole.rs"
printf '// 名前😀  \nfn 名前() {\t\n}\n' > "$P/src/mb-whole.rs"
for name in empty nonl crlf same mb; do
  f="$P/src/$name-whole.rs"
  command cp -f "$f" "$TMP/$name.orig"
  expect_trailing "$f" "$TMP/$name.want"
  pane="$(open_edit "$f")"
  out="$(fmt "$pane")"
  status="$(printf '%s' "$out" | json 'd["status"]')"
  save "$pane"
  echo "    $name: status=$status changes=$(printf '%s' "$out" | json 'd.get("changes")') undo_depth=$(doc "$pane" undo_depth)"
  if same_file "$f" "$TMP/$name.want"; then pass "$name: 期待値とバイト一致"; else fail "$name: 期待値と違う: $(od -c "$f" | head -3)"; fi
  if same_file "$TMP/$name.orig" "$TMP/$name.want"; then
    check_eq "$name: 変わらないので unchanged" "unchanged" "$status"
    continue
  fi
  check_eq "$name: 整形した" "formatted" "$status"
  undo "$pane"
  save "$pane"
  if same_file "$f" "$TMP/$name.orig"; then pass "$name: undo 1 回で戻る"; else fail "$name: undo 1 回で戻らない"; fi
done
stop_gui

echo
echo "== ④ 実の rust-analyzer + rustfmt =="
if [ -n "${TAKO_1899_SKIP_REAL:-}" ]; then
  echo "  [--] 未実測: TAKO_1899_SKIP_REAL=1"
elif [ -z "$REAL_RA" ] || [ -z "$REAL_RUSTFMT" ] || [ -z "$REAL_RUSTUP_HOME" ]; then
  echo "  [--] 未実測: rust-analyzer か rustfmt が PATH に無い"
else
  R="$TMP/real"
  mkdir -p "$R/src"
  printf '[package]\nname = "real"\nversion = "0.1.0"\nedition = "2021"\n[workspace]\n' > "$R/Cargo.toml"
  # 50 関数のうち 5 つに 1 つだけ崩れたまま（ほかは rustfmt の出力 = ほとんど整形済みのファイル）
  python3 -c 'import sys
for i in range(50):
    print(f"fn f{i}(a:u32,b:u32)->u32{{\nlet x=a+b;  \n  if x>3{{return x*2;}}\n\n\n    let long_name_{i} = some_function(alpha_value_{i}, beta_value_{i}, gamma_value_{i}, delta_value_{i});\nx+long_name_{i}\n}}")
print("fn some_function(a:u32,b:u32,c:u32,d:u32)->u32{a+b+c+d}\nfn main(){}")' > "$TMP/raw.rs"
  command cp -f "$TMP/raw.rs" "$TMP/raw-fmt.rs"
  RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN" \
    "$REAL_RUSTFMT" --edition 2021 "$TMP/raw-fmt.rs" || echo "  rustfmt が整形済みの形を作れない"
  python3 -c 'import sys
def chunks(path):
    out = []
    for line in open(path).read().splitlines(keepends=True):
        if line.startswith("fn ") or not out:
            out.append("")
        out[-1] += line
    return out
raw, fmt = chunks(sys.argv[1]), chunks(sys.argv[2])
assert len(raw) == len(fmt), (len(raw), len(fmt))
open(sys.argv[3], "w").write("".join(r if i % 5 == 2 else f for i, (r, f) in enumerate(zip(raw, fmt))))' \
    "$TMP/raw.rs" "$TMP/raw-fmt.rs" "$R/src/main.rs"
  command cp -f "$R/src/main.rs" "$TMP/real.orig"
  command cp -f "$R/src/main.rs" "$TMP/real.want"
  RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN" \
    "$REAL_RUSTFMT" --edition 2021 "$TMP/real.want" || echo "  rustfmt が期待値を作れない"
  start_gui "$TMP/app-real.log" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" RUSTUP_TOOLCHAIN="$REAL_TOOLCHAIN" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" TAKO_LSP_FORMAT_TIMEOUT_SECS=120
  RP="$(open_edit "$R/src/main.rs")"
  RR="$(fmt "$RP")"
  echo "    整形: status=$(printf '%s' "$RR" | json 'd["status"]') changes=$(printf '%s' "$RR" | json 'd["changes"]') undo_depth=$(doc "$RP" undo_depth) undo_history_bytes=$(doc "$RP" undo_history_bytes)"
  check_eq "実サーバの整形" "formatted" "$(printf '%s' "$RR" | json 'd["status"]')"
  save "$RP"
  if same_file "$R/src/main.rs" "$TMP/real.want"; then pass "実ファイルが rustfmt の出力とバイト一致"; else fail "rustfmt と違う: $(diff "$TMP/real.want" "$R/src/main.rs" | head -10)"; fi
  check_eq "整形は undo 1 件" "1" "$(doc "$RP" undo_depth)"
  undo "$RP"
  save "$RP"
  if same_file "$R/src/main.rs" "$TMP/real.orig"; then pass "undo 1 回で整形の前とバイト一致（実サーバ）"; else fail "実サーバで undo 1 回で戻らない"; fi
  stop_gui
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
