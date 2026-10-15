#!/usr/bin/env bash
# test-run-config-1731.sh — Code Runner の実行設定を覚える実経路テスト（#1731 / FR-3.18.5）
#
# 隔離した data / tmux / HOME で**実 tako-app** を立て、
#
#   ① `tako run-config a.py` は読むだけ（自動選択 = .venv・保存値なし）
#   ② `tako run-config a.py --runtime env2` → `tako run a.py` が env2 の python で走る
#      （保存先は隔離 data dir の run-configs.json・既定でプロジェクトに保存）
#   ③ `tako run-config a.py` と MCP `tako_run_defaults {path}` の応答が字面まで一致する
#   ④ 短縮（`.venv`）が 2 候補に当たると、保存せずに候補を並べてエラー
#   ⑤ 保存した env2 を消すと `tako run a.py` がエラーになり（自動の .venv へ落ちない・ペインも割らない）、
#      本文と一覧（`runtime_unresolved.next_step`）に `--reset runtime` が載る
#   ⑥ `--reset runtime` で自動（.venv）へ戻って走る
#   ⑦ プロファイル: `--profile alt` を覚えて `tako run b.py` が alt で走る / 宣言に無い名前はエラー
#   ⑧ A/B: `TAKO_1731_LEGACY=1` で立て直すと ② の判定が FAILED（保存値を読まない = .venv で走る）
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-run-config-1731.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイ + python3 が要る）。手元で走らせる実経路テスト。
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
check_has() { # check_has <名前> <含むはずの語> <本文>
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'$2' が無い: $(printf '%s' "$3" | tr '\n' ' ' | head -c 400)）" ;;
  esac
}

command -v python3 >/dev/null 2>&1 || { echo "python3 が無いので走らせられない（未実測）"; exit 1; }
SYS_PY="$(command -v python3)"

TMP="$(mktemp -d /tmp/tako-1731-XXXXXX)"
# 比較は実体パスで行う（macOS の /tmp は /private/tmp へのリンク。dispatch は canonicalize する）
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1731-$$"
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
STORE="$TAKO_DATA_DIR/run-configs.json"

# --- 材料 ---------------------------------------------------------------------
# 探索は `.git` の段で止まるので、材料の根に置いて /tmp の置き忘れを拾わないようにする
FIX="$TMP/fixtures"
mkdir -p "$FIX/.git"
# 判定は隣に書き出したファイルで読む（実行ペインの画面は流れる）
PRINT_PREFIX='import sys
tag = sys.argv[1] if len(sys.argv) > 1 else "-"
with open(__file__ + ".out", "w", encoding="utf-8") as f:
    f.write(sys.prefix + "|" + tag)
'
mkvenv() { "$SYS_PY" -m venv --without-pip "$1" || { echo "venv を作れない: $1"; exit 1; }; }
PROJ="$FIX/proj"
mkdir -p "$PROJ"
printf '%s' "$PRINT_PREFIX" > "$PROJ/a.py"
mkvenv "$PROJ/.venv"
mkvenv "$PROJ/env2"
# プロファイル 2 つ（どちらも実行環境の python で走る）
{ printf '# tako:run: ${python} ${fileBase} default\n# tako:run[alt]: ${python} ${fileBase} alt\n'
  printf '%s' "$PRINT_PREFIX"; } > "$PROJ/b.py"
# monorepo（.venv が 2 段）。**別のリポジトリ**にする（同じ `.git` の下だとプロジェクトのスコープの
# キー = `${workspaceRoot}` が proj と同じになり、④ の保存が ② の保存を上書きする = 仕様どおりの挙動）
MONO="$FIX/mono"
mkdir -p "$MONO/pkg" "$MONO/.git"
printf '%s' "$PRINT_PREFIX" > "$MONO/pkg/a.py"
mkvenv "$MONO/.venv"
mkvenv "$MONO/pkg/.venv"

field() { # field <JSON のキーの並び（ドット区切り）> <JSON> → 値（無ければ None）
  printf '%s' "$2" | python3 -c 'import json,sys
keys = sys.argv[1].split(".")
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for k in keys:
    if isinstance(d, list):
        d = d[int(k)] if k.isdigit() and int(k) < len(d) else None
    elif isinstance(d, dict):
        d = d.get(k)
    else:
        d = None
print(d if not isinstance(d, (dict, list)) else json.dumps(d, ensure_ascii=False, sort_keys=True))' "$1"
}
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

pane_count() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(sum(len(t["panes"]) for t in d["tabs"]))'
}

# run_out <file> → 実行ペインで走った python の `sys.prefix|<プロファイルの印>`
# （書き出されなければ __NO_OUTPUT__ / 走らなければ __NO_PANE__ <応答>）
run_out() {
  local out pane st status
  rm -f "$1.out"
  out="$("$TAKO_BIN" run "$1" --pane "$ROOT" --auto-close success 2>&1)"
  pane="$(field pane "$out")"
  case "$pane" in ''|None|__NOT_JSON__) echo "__NO_PANE__ $(oneline "$out")"; return ;; esac
  status=""
  for _ in $(seq 1 60); do
    st="$("$TAKO_BIN" run-interactive-status "$pane" 2>&1)"
    status="$(field status "$st")"
    [ "$status" = exited ] && break
    sleep 1
  done
  if [ -f "$1.out" ]; then cat "$1.out"; echo; else echo "__NO_OUTPUT__"; fi
}

# ② の判定（A/B で同じ関数を使う）: env2 の python で走ったか
judge_env2() { # judge_env2 <出力> → OK / FAILED
  case "$1" in "$PROJ/env2|"*) echo OK ;; *) echo FAILED ;; esac
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log" || exit 1

echo
echo "== ① 読むだけ（自動選択・保存しない） =="
R1="$("$TAKO_BIN" run-config "$PROJ/a.py" 2>&1)"
check_eq "① runtime の出どころは auto" "auto" "$(field runtime.source "$R1")"
check_eq "① 自動選択は .venv" ".venv" "$(field runtime.label "$R1")"
check_eq "① プロジェクトのキーは材料の根（.git の段）" "$FIX" "$(field scope_root "$R1")"
check_eq "① 何も書き換えない" "[]" "$(field updated "$R1")"
[ -f "$STORE" ] && fail "① 読むだけで run-configs.json ができた" || pass "① run-configs.json を作らない"

echo
echo "== ② --runtime env2 を覚えて tako run が env2 で走る =="
# 相対パス（CLI を打ったシェルの cwd 基準）
R2="$(cd "$PROJ" && "$TAKO_BIN" run-config a.py --runtime env2 2>&1)"
check_eq "② 保存先の既定はプロジェクト" '[{"action": "set", "field": "runtime", "scope": "project"}]' "$(field updated "$R2")"
check_eq "② runtime の出どころは project" "project" "$(field runtime.source "$R2")"
check_eq "② 保存した実行環境は env2" "$PROJ/env2" "$(field runtime.key "$R2")"
check_eq "② run-configs.json に書かれた" "venv" \
  "$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["projects"][sys.argv[2]]["runtime"]["python"]["manager"])' "$STORE" "$FIX" 2>&1)"
O2="$(run_out "$PROJ/a.py")"
echo "  出力: $O2"
check_eq "② tako run a.py が env2 の python で走る" "OK" "$(judge_env2 "$O2")"
L2="$("$TAKO_BIN" run "$PROJ/a.py" --list --pane "$ROOT" 2>&1)"
check_eq "② tako run --list の runtime も project の env2" "project $PROJ/env2" \
  "$(field runtime.source "$L2") $(field runtime.key "$L2")"

echo
echo "== ③ CLI と MCP tako_run_defaults の応答が字面まで一致する =="
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1731","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run_defaults","arguments":{"path":"$PROJ/a.py"}}}
JSONL
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
  "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
MCP_TEXT="$(python3 -c 'import json,sys
for line in open(sys.argv[1]):
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        content = (msg.get("result") or {}).get("content") or []
        print(content[0].get("text") if content else "__NO_CONTENT__")
        sys.exit(0)
print("__NO_RESPONSE__")' "$TMP/mcp-out.jsonl")"
CLI_TEXT="$("$TAKO_BIN" run-config "$PROJ/a.py" 2>&1)"
NORM='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True, ensure_ascii=False))'
CLI_N="$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM" 2>/dev/null)"
MCP_N="$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM" 2>/dev/null)"
[ -n "$MCP_N" ] || echo "  (MCP の生応答) $(oneline "$MCP_TEXT") / stderr: $(oneline "$(cat "$TMP/mcp-err.log")")"
[ -n "$CLI_N" ] || fail "CLI の応答が JSON でない: $(oneline "$CLI_TEXT" | head -c 300)"
check_eq "③ 応答全体が字面まで一致する（$(printf '%s' "$CLI_N" | wc -c | tr -d ' ') バイト）" "$CLI_N" "$MCP_N"
# 整形だけは違う（CLI は整形して表示・MCP の本文は 1 行の JSON）ので、キーの並びを保ったまま
# 1 行へ詰め直してバイトで比べる（並べ替えない = 並びのずれも拾う）
COMPACT='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), ensure_ascii=False, separators=(",", ":")))'
printf '%s' "$CLI_TEXT" | python3 -c "$COMPACT" > "$TMP/cli.txt" 2>/dev/null
printf '%s' "$MCP_TEXT" | python3 -c "$COMPACT" > "$TMP/mcp.txt" 2>/dev/null
if [ -s "$TMP/cli.txt" ] && cmp -s "$TMP/cli.txt" "$TMP/mcp.txt"; then
  pass "③ キーの並びも含めてバイト一致（$(wc -c < "$TMP/cli.txt" | tr -d ' ') バイト）"
else
  fail "③ キーの並びまで含めるとずれる: $(diff "$TMP/cli.txt" "$TMP/mcp.txt" | head -4 | tr '\n' ' ' | head -c 300)"
fi

echo
echo "== ④ 短縮が 2 候補に当たると保存せずに候補を並べる =="
BEFORE="$(cat "$STORE")"
E4="$("$TAKO_BIN" run-config "$MONO/pkg/a.py" --runtime .venv 2>&1)"; RC4=$?
check_eq "④ 終了コードが非 0" "1" "$([ "$RC4" -ne 0 ] && echo 1 || echo 0)"
check_has "④ 近い段の .venv を候補に並べる" "venv:$MONO/pkg/.venv" "$E4"
check_has "④ 遠い段の .venv も並べる" "venv:$MONO/.venv" "$E4"
check_eq "④ run-configs.json は変わらない" "$BEFORE" "$(cat "$STORE")"
R4="$("$TAKO_BIN" run-config "$MONO/pkg/a.py" --runtime "venv:$MONO/.venv" 2>&1)"
check_eq "④ ID なら選べる" "$MONO/.venv" "$(field runtime.key "$R4")"

echo
echo "== ⑤ 保存した env2 を消すと自動へ落ちずにエラー =="
rm -rf "$PROJ/env2"
N0="$(pane_count)"
E5="$("$TAKO_BIN" run "$PROJ/a.py" --pane "$ROOT" 2>&1)"; RC5=$?
echo "  tako run: rc=$RC5 $(oneline "$E5" | head -c 300)"
check_eq "⑤ tako run が非 0 で終わる" "1" "$([ "$RC5" -ne 0 ] && echo 1 || echo 0)"
check_has "⑤ 本文に次の一手（--reset runtime）" "--reset runtime" "$E5"
check_eq "⑤ 実行ペインを割らない" "$N0" "$(pane_count)"
R5="$("$TAKO_BIN" run-config "$PROJ/a.py" 2>&1)"
check_eq "⑤ 一覧の runtime_unresolved.key は env2" "$PROJ/env2" "$(field runtime_unresolved.key "$R5")"
check_has "⑤ 一覧の runtime_unresolved.next_step" "--reset runtime" "$(field runtime_unresolved.next_step "$R5")"
L5="$("$TAKO_BIN" run "$PROJ/a.py" --list --pane "$ROOT" 2>&1)"
check_has "⑤ tako run --list にも runtime_unresolved.next_step" "--reset runtime" "$(field runtime_unresolved.next_step "$L5")"
E5b="$("$TAKO_BIN" run-config "$PROJ/a.py" --runtime env2 2>&1)"; RC5b=$?
check_eq "⑤ 消えた環境は選び直せない（保存しない）" "1" "$([ "$RC5b" -ne 0 ] && echo 1 || echo 0)"

echo
echo "== ⑥ --reset runtime で自動（.venv）へ戻る =="
R6="$("$TAKO_BIN" run-config "$PROJ/a.py" --reset runtime 2>&1)"
check_eq "⑥ プロジェクトから消した" '[{"action": "reset", "field": "runtime", "scope": "project"}]' "$(field updated "$R6")"
check_eq "⑥ runtime の出どころは auto" "auto .venv" "$(field runtime.source "$R6") $(field runtime.label "$R6")"
O6="$(run_out "$PROJ/a.py")"
echo "  出力: $O6"
check_eq "⑥ tako run a.py が .venv で走る" "$PROJ/.venv|-" "$O6"

echo
echo "== ⑦ プロファイルを覚える =="
O7a="$(run_out "$PROJ/b.py")"
check_eq "⑦ 何も覚えていなければ既定（default）" "$PROJ/.venv|default" "$O7a"
R7="$("$TAKO_BIN" run-config "$PROJ/b.py" --profile alt 2>&1)"
check_eq "⑦ 保存先の既定はファイル" '[{"action": "set", "field": "profile", "scope": "file"}]' "$(field updated "$R7")"
O7="$(run_out "$PROJ/b.py")"
echo "  出力: $O7"
check_eq "⑦ tako run b.py が alt で走る" "$PROJ/.venv|alt" "$O7"
E7="$("$TAKO_BIN" run-config "$PROJ/b.py" --profile nope 2>&1)"; RC7=$?
check_eq "⑦ 宣言に無い名前はエラー" "1" "$([ "$RC7" -ne 0 ] && echo 1 || echo 0)"
check_has "⑦ 選べる名前を並べる" "alt" "$E7"
E7c="$("$TAKO_BIN" run-config "$PROJ/b.py" --runtime env2 --project --file 2>&1)"; RC7c=$?
check_eq "⑦ --project と --file は同時に使えない" "1" "$([ "$RC7c" -ne 0 ] && echo 1 || echo 0)"

echo
echo "== ⑧ A/B: TAKO_1731_LEGACY=1 で立て直す =="
mkvenv "$PROJ/env2"
"$TAKO_BIN" run-config "$PROJ/a.py" --runtime env2 >/dev/null 2>&1
ON="$(run_out "$PROJ/a.py")"
check_eq "⑧ 新しい挙動では env2 で走る" "OK" "$(judge_env2 "$ON")"
stop_isolated_gui "$APP_PID"
APP_PID=""
start_gui "$TMP/app-legacy.log" TAKO_1731_LEGACY=1 || exit 1
OL="$(run_out "$PROJ/a.py")"
echo "  LEGACY の出力: $OL"
check_eq "⑧ LEGACY では ② の判定が FAILED（保存値を読まない = 自動の .venv）" "FAILED" "$(judge_env2 "$OL")"
check_eq "⑧ LEGACY の実行は自動選択の .venv" "$PROJ/.venv|-" "$OL"

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
