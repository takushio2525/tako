#!/bin/bash
# test-lsp-diagnostics-1679.sh — LSP の診断（#1679）の実経路テスト（隔離 GUI・偽サーバ）
#
# 何を確かめるか（Issue #1679 の受け入れ条件のうち、GUI を通して初めて言えるもの）:
#   ① visual-test `preview-code` 節の「診断のある画面」: 基準画像（診断の写しだけ外した同じ場面）
#      との差分が波線の帯の中にだけある / 重大度 4 段の色が実ピクセルで互いに異なる /
#      行末まで伸びる範囲と波の縦の振れ幅が切られていない（#797）/ 右パネルに
#      1 件 1 行で並ぶ / 閉じたら保持 0
#   ② A/B: `TAKO_1007_LEGACY=1` の同じバイナリでは波線が消え、① が FAILED になる
#   ③ 実 GUI + 実 CLI: 偽サーバの固定配列が `tako lsp diagnostics --json` に出て、
#      `--severity error` の境界（省略はエラー / 警告は入らない）が効き、MCP `tako_lsp`
#      （`tako mcp serve` 経由）が CLI と同じ答えを返す
#   ④ エッジ: 診断 0 件 / 行末を越える範囲 / 多バイト文字の行（UTF-16 の桁）
#   ⑤ `tako panel --show --view diagnostics` が効き、ペインを閉じたら `retained` が 0
#
# 使い方: bash scripts/test-lsp-diagnostics-1679.sh
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 面を用意できなければ起動せず終了コード 4 で「未実測」を返す（#1744）。
# 落とすのは自分で起こした pid だけ。
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

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1679-XXXXXX")"
# ⑥ の実サーバ用に、HOME を隔離する**前**の rustup / cargo の置き場を採っておく
# （rust-analyzer は rustup の proxy なので、HOME を変えると toolchain を見失う）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
APP_PID=""
TMUX_SOCKET="tako-1679-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako --bin tako-lsp-fake 2>/dev/null \
  || cargo build -q -p tako-cli -p tako-control || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
FAKE="$(dirname "$APP_BIN")/tako-lsp-fake"
[ -x "$FAKE" ] || { echo "偽サーバが無い: $FAKE"; exit 1; }

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_1679_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP"

common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot"
  TAKO_PERSIST=0 TAKO_AUTORENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
)

# visual-test の節を 1 回走らせる（$1 = ログ / 残りは追加の env）
run_visual() {
  local log="$1"; shift
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$TMP/data-visual" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=preview-code TAKO_VISUAL_DUMP_DIR="$DUMP" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test preview-code（診断のある画面） =="
run_visual "$TMP/visual.log"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log" && pass "TAKO_VISUAL_TEST_OK" \
  || { fail "OK 行が出ない"; grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed 's/^/    /'; }
grep 'TAKO_VISUAL_PIXEL: preview-code diagnostics' "$TMP/visual.log" | sed 's/^/  観測: /'
grep -q 'preview-code diagnostics .*outside=0 ' "$TMP/visual.log" \
  && pass "基準画像との差分は波線の帯の中だけ" || fail "帯の外に差分がある"
grep -q 'preview-code diagnostics closed retained=(0, 0) edit_left=false' "$TMP/visual.log" \
  && pass "閉じたら保持 0（GUI の中の口）" || fail "閉じても保持が残る"

echo
echo "== ② A/B: TAKO_1007_LEGACY=1 で波線が消えて FAILED =="
run_visual "$TMP/legacy.log" TAKO_1007_LEGACY=1
RC=$?
[ "$RC" -ne 0 ] && pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）" \
  || fail "LEGACY でも節が通った（検出力が無い）"
grep 'TAKO_VISUAL_PIXEL: preview-code diagnostics\|TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" \
  | sed 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test 診断: .*波線が描かれていない' "$TMP/legacy.log" \
  && pass "落ちた理由は波線が無いこと" || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③〜⑤ 実 GUI + 実 CLI + MCP（偽サーバの固定配列） =="
FIX="$TMP/proj"
mkdir -p "$FIX/src"
printf '[package]\nname = "p"\n' > "$FIX/Cargo.toml"
# 日本語・絵文字（サロゲートペア）の行 / 行末を越える範囲 / 重大度の省略
printf 'fn main() {\n    let 日本 = "😀";\n    let x: i32 = "s";\n}\n' > "$FIX/src/main.rs"
cat > "$TMP/diagnostics.json" <<'JSON'
[
 {"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}},"severity":4,"message":"hint"},
 {"range":{"start":{"line":1,"character":8},"end":{"line":1,"character":10}},"severity":2,"message":"unused variable","source":"rustc","code":"unused_variables"},
 {"range":{"start":{"line":1,"character":13},"end":{"line":1,"character":17}},"severity":3,"message":"info"},
 {"range":{"start":{"line":2,"character":17},"end":{"line":2,"character":20}},"severity":1,"message":"mismatched types","source":"rustc","code":"E0308"},
 {"range":{"start":{"line":2,"character":20},"end":{"line":2,"character":99}},"severity":1,"message":"past end"},
 {"range":{"start":{"line":3,"character":0},"end":{"line":3,"character":1}},"message":"no severity","code":7}
]
JSON
mkdir -p "$TMP/empty/src"
cp "$FIX/Cargo.toml" "$TMP/empty/Cargo.toml"
printf 'fn main() {}\n' > "$TMP/empty/src/main.rs"

# CLI も同じ隔離の受け口を見る（GUI へ渡した env を CLI 側にも置く）
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/data-gui"
mkdir -p "$TAKO_DATA_DIR"
launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} TAKO_DATA_DIR="$TAKO_DATA_DIR" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" TAKO_LSP_FAKE_DIAGNOSTICS="$TMP/diagnostics.json" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
PANE="$("$TAKO_BIN" open "$FIX/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$PANE" ] || { echo "プレビューを開けない"; exit 1; }

NOT_EDITING="$("$TAKO_BIN" lsp diagnostics --pane "$PANE" --json | json 'd["documents"][0].get("reason") is not None and d["documents"][0]["editing"] == False')"
check_eq "編集モードでないペインは理由を返す" "True" "$NOT_EDITING"

"$TAKO_BIN" edit start --pane "$PANE" >/dev/null
for _ in $(seq 1 100); do
  N="$("$TAKO_BIN" lsp diagnostics --json | json 'd["total"]')"
  [ "$N" = "6" ] && break
  sleep 0.1
done
check_eq "偽サーバの 6 件が一覧に出る" "6" "${N:-}"

"$TAKO_BIN" lsp diagnostics --severity error --json > "$TMP/cli-error.json"
GOT="$(json '[(x["range"]["start"]["line"], x["range"]["start"]["column"], x["range"]["end"]["column"], x.get("code")) for x in d["documents"][0]["diagnostics"]]' < "$TMP/cli-error.json")"
check_eq "--severity error の固定値（省略はエラー / 警告は入らない / 行末で丸める）" \
  "[(3, 17, 20, 'E0308'), (3, 20, 21, None), (4, 0, 1, '7')]" "$GOT"
WARN="$("$TAKO_BIN" lsp diagnostics --severity warning --json | json '[(x["severity"], x["range"]["start"]["column"], x["range"]["end"]["column"]) for x in d["documents"][0]["diagnostics"] if x["range"]["start"]["line"] == 2]')"
check_eq "--severity warning は警告を含み情報を含まない・日本語の桁は UTF-8 バイト" \
  "[('warning', 8, 14)]" "$WARN"
TEXT_OUT="$("$TAKO_BIN" lsp diagnostics --severity error | sed -n 2p)"
case "$TEXT_OUT" in
  *"3:17-3:20"*"error"*"mismatched types"*"(rustc E0308)"*) pass "人向けの表示（行:桁-行:桁 + 重大度 + 出所）" ;;
  *) fail "人向けの表示が崩れた: $TEXT_OUT" ;;
esac

# MCP（stdio ブリッジ）でも同じ答え。ブリッジは **tako の中で起動された証（TAKO_SOCKET +
# TAKO_TOKEN）が無いとツールを 1 つも公開しない**（FR-2.3.2）ので、隔離インスタンスの受け口を明示する
SOCK="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
MCP_OUT="$(
  export TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_lsp","arguments":{"severity":"error"}}}'
    sleep 2
  } | "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"])
'
)"
if [ -n "$MCP_OUT" ]; then
  echo "$MCP_OUT" > "$TMP/mcp-error.json"
  SAME="$(python3 -c '
import json, sys
a = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
print(a["documents"] == b["documents"] and a["total"] == b["total"])' "$TMP/cli-error.json" "$TMP/mcp-error.json" 2>/dev/null)"
  check_eq "MCP tako_lsp が CLI と同じ答えを返す" "True" "$SAME"
else
  fail "MCP tako_lsp の応答が取れない（受け口: ${SOCK:-なし}）"
fi

"$TAKO_BIN" panel --show --view diagnostics >/dev/null
VIEW="$("$TAKO_BIN" panel 2>/dev/null | json 'd.get("view") or d.get("panel_view")')"
check_eq "tako panel --view diagnostics が効く" "diagnostics" "$VIEW"

# エッジ: 診断 0 件の文書（偽サーバは空配列を publish する）
cp "$TMP/diagnostics.json" "$TMP/diagnostics.saved.json"
echo '[]' > "$TMP/diagnostics.json"
PANE2="$("$TAKO_BIN" open "$TMP/empty/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
"$TAKO_BIN" edit start --pane "$PANE2" >/dev/null
for _ in $(seq 1 100); do
  EMPTY="$("$TAKO_BIN" lsp diagnostics --pane "$PANE2" --json | json '(d["documents"][0].get("version") is not None, d["documents"][0]["diagnostics"], d["documents"][0].get("reason"), d["documents"][0]["counts"]["error"])')"
  case "$EMPTY" in "(True, [], None, 0)") break ;; esac
  sleep 0.1
done
check_eq "診断 0 件は空の配列（publish 済み・理由は付かない）" "(True, [], None, 0)" "${EMPTY:-}"

# 閉じたら保持 0（#830 の機序）
"$TAKO_BIN" close --pane "$PANE" --force >/dev/null 2>&1 || "$TAKO_BIN" close --pane "$PANE" >/dev/null 2>&1
"$TAKO_BIN" close --pane "$PANE2" --force >/dev/null 2>&1 || "$TAKO_BIN" close --pane "$PANE2" >/dev/null 2>&1
RETAINED="$("$TAKO_BIN" lsp diagnostics --json | json '(d["retained"]["diagnostics"], d["retained"]["documents"], d["documents"])')"
check_eq "ペインを閉じたら retained が 0" "(0, 0, [])" "$RETAINED"

stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ⑥ 実サーバ（rust-analyzer）の診断が実ファイルで出る =="
if [ -z "$REAL_RA" ] || [ -z "$REAL_RUSTUP_HOME" ]; then
  echo "  （rust-analyzer が無いので飛ばす。実機目視は .agent/manual-checks.md の手順で）"
else
  REAL="$TMP/real"
  mkdir -p "$REAL/src"
  printf '[package]\nname = "real1679"\nversion = "0.1.0"\nedition = "2021"\n' > "$REAL/Cargo.toml"
  printf 'fn main() {\n    let x: i32 = "s";\n    let 日本 = 1;\n}\n' > "$REAL/src/main.rs"
  export TAKO_DATA_DIR="$TMP/data-real"
  mkdir -p "$TAKO_DATA_DIR"
  launch_isolated_gui "$TMP/real.log" ${common_env[@]+"${common_env[@]}"} TAKO_DATA_DIR="$TAKO_DATA_DIR" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" \
    CARGO_HOME="$REAL_CARGO_HOME" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/real.log" || exit 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  RPANE="$("$TAKO_BIN" open "$REAL/src/main.rs" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
  "$TAKO_BIN" edit start --pane "$RPANE" >/dev/null
  RERR=""
  for _ in $(seq 1 180); do
    RERR="$("$TAKO_BIN" lsp diagnostics --pane "$RPANE" --severity error --json | json 'len(d["documents"][0]["diagnostics"])')"
    [ "${RERR:-0}" -ge 1 ] 2>/dev/null && break
    sleep 0.5
  done
  "$TAKO_BIN" lsp status --json | json '[(s["id"], s["state"], (s.get("server_info") or {}).get("name")) for s in d["servers"]]' | sed 's/^/  観測: status /'
  # pull 型（textDocument/diagnostic）をサーバが申告していても、tako は申告しないので push で届く
  "$TAKO_BIN" lsp status --json | json '[("diagnosticProvider" in (s.get("capabilities") or {})) for s in d["servers"]]' | sed 's/^/  観測: サーバの pull 型の申告 = /'
  "$TAKO_BIN" lsp diagnostics --pane "$RPANE" | sed 's/^/  観測: /'
  [ "${RERR:-0}" -ge 1 ] 2>/dev/null && pass "rust-analyzer のエラーが一覧に出る（${RERR} 件）" \
    || fail "rust-analyzer のエラーが 90 秒以内に出ない"
  "$TAKO_BIN" panel --show --view diagnostics >/dev/null
  sleep 1
fi

echo
echo "ダンプ: $DUMP"
ls "$DUMP" 2>/dev/null | sed 's/^/  /'
echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
