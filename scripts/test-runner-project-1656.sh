#!/usr/bin/env bash
# test-runner-project-1656.sh — Code Runner のプロジェクト既定の実経路テスト（#1656 / FR-3.18）
#
# 隔離した data / tmux / HOME で**実 tako-app** を立て、
#
#   ① CLI `tako run <file> --dry-run` が cargo プロジェクトの `.rs` を
#      `cargo run`（source = project_default・cwd = プロジェクトのルート）で解決する
#   ② MCP `tako_run_resolve` が**同じ dispatch** を通り、応答が CLI と字面まで一致する
#   ③ Issue の例の形（workspace の lib crate の `src/runner.rs`）が
#      `cargo test -p <pkg> --lib runner::` になり、`rustc` 単体へ落ちない
#   ④ 実際に走る: `tako run` で cargo プロジェクトの main.rs を実行し、
#      ペインに `cargo run` の出力が出て終了コード 0 で終わる
#   ⑤ エッジ: HOME 直下の置き忘れ `package.json` を拾わない / workspace member の bin /
#      シンボリックリンク越しのファイル（実体のルートで走る）/ 宣言が勝つ / マーカー無しは
#      従来の拡張子既定
#   ⑥ A/B: `TAKO_1656_LEGACY=1` で立て直すと同じファイルが `rustc` 単体へ戻る
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-runner-project-1656.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイ + cargo が要る）。手元で走らせる実経路テスト。
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

TMP="$(mktemp -d /tmp/tako-1656-XXXXXX)"
# 比較は実体パスで行う（macOS の /tmp は /private/tmp へのリンク。dispatch は canonicalize する）
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1656-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
# HOME を差し替えると rustup / cargo が toolchain を見失うので、**差し替える前に**実値で固定する
# （読むだけ。依存の無い fixture なのでレジストリへも書かない）
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 材料 ---------------------------------------------------------------------
# 探索は `.git` の段で止まるので、材料の根に置いて /tmp の置き忘れを拾わないようにする
FIX="$TMP/fixtures"
mkdir -p "$FIX/.git"
python3 - "$FIX" "$HOME" <<'PY'
import pathlib, sys
fix, home = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
def w(p, s=""):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(s, encoding="utf-8")
# 単独の cargo パッケージ
w(fix / "rsproj/Cargo.toml", '[package]\nname = "demo1656"\nversion = "0.1.0"\nedition = "2021"\n')
w(fix / "rsproj/src/main.rs", 'mod util;\nfn main() { println!("HELLO_1656_{}", util::word()); }\n')
w(fix / "rsproj/src/util.rs", 'pub fn word() -> &\'static str { "FROM_CARGO" }\n')
w(fix / "rsproj/src/decl.rs", "// tako:run: echo DECL_1656\n")
# Issue の例の形: workspace の lib crate + bin crate
w(fix / "ws/Cargo.toml", '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n')
w(fix / "ws/crates/core1656/Cargo.toml", '[package]\nname = "core1656"\nversion = "0.1.0"\nedition = "2021"\n')
w(fix / "ws/crates/core1656/src/lib.rs", "pub mod runner;\n")
w(fix / "ws/crates/core1656/src/runner.rs", "#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_runs() {}\n}\n")
w(fix / "ws/crates/app1656/Cargo.toml", '[package]\nname = "app1656"\nversion = "0.1.0"\nedition = "2021"\n')
w(fix / "ws/crates/app1656/src/main.rs", "fn main() {}\n")
# マーカーの無い単独ファイル
w(fix / "loose/sub/hello.rs", "fn main() {}\n")
(fix / "loose/.git").mkdir(parents=True, exist_ok=True)
# HOME 直下の置き忘れ（ホームで `npm i` を打つと出来る）
w(home / "package.json", '{"scripts":{"start":"node scratch/a.js"}}')
w(home / "scratch/a.js", "console.log(1)\n")
PY
ln -s "$FIX/rsproj" "$FIX/rslink"

PY_RESOLVE='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
p = (d.get("profiles") or [{}])[0]
proj = d.get("project") or {}
for k in ("command", "cwd", "source"):
    print("%s=%s" % (k, p.get(k)))
print("kind=%s" % proj.get("kind"))
print("root=%s" % proj.get("root"))
print("workspace_root=%s" % d.get("workspace_root"))'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }
oneline() { printf '%s' "$1" | tr '\n' ' '; }

start_gui() { # start_gui <ログ> [VAR=VAL…]
  local log="$1"; shift
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || return 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; return 1; }
  echo "  pid=$APP_PID root pane=$ROOT"
}

dry() { # dry <file> → 解決の要約（1 行 1 項目）
  "$TAKO_BIN" run "$1" --dry-run --pane "$ROOT" 2>&1 | python3 -c "$PY_RESOLVE"
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log" || exit 1

echo
echo "== ① CLI --dry-run: cargo プロジェクトの main.rs =="
OUT_A="$(dry "$FIX/rsproj/src/main.rs")"
echo "  $(oneline "$OUT_A")"
check_eq "source は project_default" "project_default" "$(field source "$OUT_A")"
check_eq "コマンドは cargo run" "cargo run" "$(field command "$OUT_A")"
check_eq "cwd はプロジェクトのルート" "$FIX/rsproj" "$(field cwd "$OUT_A")"
check_eq "project.kind は cargo" "cargo" "$(field kind "$OUT_A")"
check_eq "workspace_root はルート" "$FIX/rsproj" "$(field workspace_root "$OUT_A")"
OUT_MOD="$(dry "$FIX/rsproj/src/util.rs")"
check_eq "同じパッケージのモジュールも cargo run" "cargo run" "$(field command "$OUT_MOD")"

echo
echo "== ② MCP tako_run_resolve が CLI と字面まで一致する =="
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1656","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run_resolve","arguments":{"path":"$FIX/rsproj/src/main.rs","pane":$ROOT}}}
JSONL
# `mcp serve` はツール公開の判定を env だけで行うので、接続情報ファイルから渡す（トークンは表示しない）
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
  "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
PY_MCP='import json,sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    content = (msg.get("result") or {}).get("content") or []
    print(content[0].get("text") if content else "__NO_CONTENT__")
    sys.exit(0)
print("__NO_RESPONSE__")'
MCP_TEXT="$(python3 -c "$PY_MCP" "$TMP/mcp-out.jsonl")"
CLI_TEXT="$("$TAKO_BIN" run "$FIX/rsproj/src/main.rs" --dry-run --pane "$ROOT" 2>&1)"
NORM='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True, ensure_ascii=False))'
CLI_NORM="$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM" 2>/dev/null)"
MCP_NORM="$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM" 2>/dev/null)"
echo "  CLI: $CLI_NORM"
echo "  MCP: $MCP_NORM"
[ -n "$MCP_NORM" ] || {
  echo "  (MCP の生応答) $MCP_TEXT"
  echo "  (MCP の stderr) $(tr '\n' ' ' < "$TMP/mcp-err.log")"
}
[ -n "$CLI_NORM" ] || fail "CLI の応答が JSON でない: $CLI_TEXT"
check_eq "CLI と MCP の応答が字面まで一致する" "$CLI_NORM" "$MCP_NORM"

echo
echo "== ③ Issue の例の形: workspace の lib crate =="
OUT_C="$(dry "$FIX/ws/crates/core1656/src/runner.rs")"
echo "  $(oneline "$OUT_C")"
check_eq "lib crate はそのモジュールのテスト" "cargo test -p core1656 --lib runner::" "$(field command "$OUT_C")"
check_eq "cwd はワークスペースのルート" "$FIX/ws" "$(field cwd "$OUT_C")"
case "$(field command "$OUT_C")" in
  *rustc*) fail "rustc 単体へ落ちている" ;;
  *) pass "rustc 単体へ落ちていない" ;;
esac

echo
echo "== ④ 実際に走らせる: cargo run の出力と終了コード =="
RUN_OUT="$("$TAKO_BIN" run "$FIX/rsproj/src/main.rs" --pane "$ROOT" 2>&1)"
RUN_PANE="$(printf '%s' "$RUN_OUT" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
echo "  run: $(oneline "$RUN_OUT")"
if [ -z "$RUN_PANE" ]; then
  fail "実行ペインが作られない"
else
  STATUS=""
  for _ in $(seq 1 120); do
    ST="$("$TAKO_BIN" run-interactive-status "$RUN_PANE" 2>&1)"
    STATUS="$(printf '%s' "$ST" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin); print("%s %s" % (d.get("status"), d.get("exit_code")))
except Exception:
    pass' 2>/dev/null)"
    case "$STATUS" in exited*) break ;; esac
    sleep 1
  done
  check_eq "cargo run が終了コード 0 で終わる" "exited 0" "$STATUS"
  SCREEN="$("$TAKO_BIN" read --pane "$RUN_PANE" 2>&1)"
  case "$SCREEN" in
    *HELLO_1656_FROM_CARGO*) pass "ペインに cargo run の出力が出る（mod util まで解決できている）" ;;
    *) fail "ペインに出力が無い: $(oneline "$SCREEN" | tail -c 400)" ;;
  esac
  case "$SCREEN" in
    *"Running \`target"*|*"Running "*) pass "cargo がビルドして走らせている" ;;
    *) fail "cargo の実行行が見えない" ;;
  esac
fi

echo
echo "== ⑤ エッジ =="
OUT_H="$(dry "$HOME/scratch/a.js")"
echo "  HOME 直下の置き忘れ: $(oneline "$OUT_H")"
check_eq "HOME 直下の package.json はプロジェクトと読まない" "extension_default" "$(field source "$OUT_H")"
check_eq "project は null" "None" "$(field kind "$OUT_H")"
OUT_B="$(dry "$FIX/ws/crates/app1656/src/main.rs")"
check_eq "workspace member の bin は -p 付き" "cargo run -p app1656" "$(field command "$OUT_B")"
check_eq "member の cwd もワークスペースのルート" "$FIX/ws" "$(field cwd "$OUT_B")"
OUT_L="$(dry "$FIX/rslink/src/main.rs")"
check_eq "シンボリックリンク越しは実体のルートで走る" "$FIX/rsproj" "$(field cwd "$OUT_L")"
OUT_D="$(dry "$FIX/rsproj/src/decl.rs")"
check_eq "tako:run 宣言がプロジェクト既定より勝つ" "declaration" "$(field source "$OUT_D")"
check_eq "宣言のコマンド" "echo DECL_1656" "$(field command "$OUT_D")"
OUT_N="$(dry "$FIX/loose/sub/hello.rs")"
check_eq "マーカーが無ければ従来の拡張子既定" "extension_default" "$(field source "$OUT_N")"
check_eq "従来の rustc 単体" "rustc hello.rs -o hello && ./hello" "$(field command "$OUT_N")"
check_eq "workspace_root は git のルート" "$FIX/loose" "$(field workspace_root "$OUT_N")"

echo
echo "== ⑥ A/B: TAKO_1656_LEGACY=1 で立て直す =="
stop_isolated_gui "$APP_PID"
APP_PID=""
start_gui "$TMP/app-legacy.log" TAKO_1656_LEGACY=1 || exit 1
OUT_LEG="$(dry "$FIX/rsproj/src/main.rs")"
echo "  $(oneline "$OUT_LEG")"
check_eq "LEGACY では拡張子既定へ戻る" "extension_default" "$(field source "$OUT_LEG")"
check_eq "LEGACY では rustc 単体（#1656 の症状）" "rustc main.rs -o main && ./main" "$(field command "$OUT_LEG")"
check_eq "LEGACY の cwd はファイルのディレクトリ" "$FIX/rsproj/src" "$(field cwd "$OUT_LEG")"

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
