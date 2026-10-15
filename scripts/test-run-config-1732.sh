#!/usr/bin/env bash
# test-run-config-1732.sh — 実行設定の引数・環境変数・作業ディレクトリ・実行前コマンドの実経路テスト
# （#1732 / FR-3.18.6）
#
# 隔離した data / tmux / HOME で**実 tako-app** を立て、
#
#   ① `tako run-config a.sh --env APP_ENV=dev --env <秘密>=… --args …` → `tako run a.sh` の
#      実行ペインの出力に env と引数が出る（引数は 1 本の文字列としてシェルが割る）
#   ② `tako run-config` と MCP `tako_run_defaults {path}` の応答が字面まで一致し、
#      MCP で保存した引数（`args`）・環境変数の削除（`env: {K: null}`）も `tako run` に効く
#   ③ `--before "false"` → 本体が走らず、実行ペインの終了コードが 1
#   ④ `--before` が成功すれば本体が走る（before → 本体の順）/ `--cwd sub` で作業ディレクトリが変わる
#      （組み込み既定の `bash ${fileBase}` は移った先から絶対パスで引ける）
#   ⑤ コンパイル系（`.c`）は生成物へ引数が渡る（cc があるときだけ）
#   ⑥ 検証: 使えない名前・無い作業ディレクトリ・`K=V` でない `--env`・設定と削除の重なりは
#      非 0 で保存しない
#   ⑦ 器あり（`TAKO_PERSIST=1` = tmux の器の中）でも env と引数が届く
#   ⑧ 診断ログ（persist.log / perf.log / GUI の stderr）に env の値・引数・実行前コマンドが 0 件
#   ⑨ A/B: `TAKO_1732_LEGACY=1` で立て直すと ① の判定が FAILED（保存しても実行に効かない）
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-run-config-1732.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
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
check_rc_nonzero() { # check_rc_nonzero <名前> <終了コード> <本文>
  if [ "$2" -ne 0 ]; then pass "$1"; else fail "$1（0 で終わった: $(printf '%s' "$3" | tr '\n' ' ' | head -c 300)）"; fi
}

TMP="$(mktemp -d /tmp/tako-1732-XXXXXX)"
# 比較は実体パスで行う（macOS の /tmp は /private/tmp へのリンク。dispatch は canonicalize する）
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1732-$$"
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

# 診断ログに出てはいけない目印（env の値・引数・実行前コマンド）
SECRET_ENV="tako1732envSECRET"
SECRET_ARG="tako1732argSECRET"
SECRET_BEFORE="tako1732beforeSECRET"

# --- 材料 ---------------------------------------------------------------------
# 探索は `.git` の段で止まるので、材料の根に置いて /tmp の置き忘れを拾わないようにする
FIX="$TMP/fixtures"
PROJ="$FIX/proj"
mkdir -p "$FIX/.git" "$PROJ/sub"
OUT="$PROJ/a.out"
# 判定は書き出したファイルで読む（実行ペインの画面は流れる）。
# 1 行目 = env 2 つ / 2 行目 = 引数を 1 つずつ [] で / 3 行目 = 作業ディレクトリ
cat > "$PROJ/a.sh" <<SH
{
  printf '%s|%s\n' "\${APP_ENV-unset}" "\${TAKO_1732_KEY-unset}"
  for a in "\$@"; do printf '[%s]' "\$a"; done; printf '\n'
  pwd -P
} > '$OUT'
SH

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

# run_file <file> <出力ファイル> → 「終了コード」と出力ファイルの中身（無ければ __NO_OUTPUT__）
# 走らなければ __NO_PANE__ <応答>
run_file() {
  local out pane st status code
  rm -f "$2"
  out="$("$TAKO_BIN" run "$1" --pane "$ROOT" --auto-close never 2>&1)"
  pane="$(field pane "$out")"
  case "$pane" in ''|None|__NOT_JSON__) echo "__NO_PANE__ $(oneline "$out")"; return ;; esac
  status=""; code=""
  for _ in $(seq 1 60); do
    st="$("$TAKO_BIN" run-interactive-status "$pane" 2>&1)"
    status="$(field status "$st")"
    [ "$status" = exited ] && { code="$(field exit_code "$st")"; break; }
    sleep 1
  done
  echo "exit=$code"
  # 同じファイルの次の実行は同じペインを差し替える（#1657）ので、ここでは閉じない
  if [ -f "$2" ]; then cat "$2"; else echo "__NO_OUTPUT__"; fi
}

# ① の判定（A/B で同じ関数を使う）: env と引数が実行ペインまで届いたか
judge_env_args() { # judge_env_args <run_file の出力> → OK / FAILED
  case "$1" in
    *"dev|$SECRET_ENV"*"[x][y z][$SECRET_ARG]"*) echo OK ;;
    *) echo FAILED ;;
  esac
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log" || exit 1

echo
echo "== ① --env / --args を覚えて tako run の実行ペインに出る =="
R1="$(cd "$PROJ" && "$TAKO_BIN" run-config a.sh --env APP_ENV=dev --env "TAKO_1732_KEY=$SECRET_ENV" \
  --args "x 'y z' $SECRET_ARG" 2>&1)"
check_eq "① 保存先の既定はファイル（args / env）" \
  '[{"action": "set", "field": "args", "scope": "file"}, {"action": "set", "field": "env", "scope": "file"}]' \
  "$(field updated "$R1")"
check_eq "① config.env.APP_ENV の出どころは file" "file" "$(field config.env.APP_ENV.source "$R1")"
check_eq "① run-configs.json に引数が書かれた" "x 'y z' $SECRET_ARG" \
  "$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["files"][sys.argv[2]]["args"])' "$STORE" "$PROJ/a.sh" 2>&1)"
O1="$(run_file "$PROJ/a.sh" "$OUT")"
echo "  出力: $(oneline "$O1")"
check_eq "① env と引数が実行ペインまで届く" "OK" "$(judge_env_args "$O1")"
check_has "① 終了コード 0" "exit=0" "$O1"
check_has "① 作業ディレクトリは既定（ファイルの段）" "$PROJ"$'\n' "$O1"$'\n'
L1="$("$TAKO_BIN" run "$PROJ/a.sh" --list --pane "$ROOT" 2>&1)"
check_eq "① tako run --list のコマンドにも引数が載る" "bash a.sh x 'y z' $SECRET_ARG" "$(field profiles.0.command "$L1")"

echo
echo "== ② CLI と MCP tako_run_defaults が同じ 1 本を通る =="
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp_call() { # mcp_call <arguments の JSON> → 本文（text）
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1732","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_run_defaults\",\"arguments\":$1}}" \
    > "$TMP/mcp-in.jsonl"
  env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2>> "$TMP/mcp-err.log"
  python3 -c 'import json,sys
for line in open(sys.argv[1]):
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        if "error" in msg:
            print("__RPC_ERROR__ " + json.dumps(msg["error"], ensure_ascii=False)); sys.exit(0)
        content = (msg.get("result") or {}).get("content") or []
        print(content[0].get("text") if content else "__NO_CONTENT__")
        sys.exit(0)
print("__NO_RESPONSE__")' "$TMP/mcp-out.jsonl"
}
COMPACT='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), ensure_ascii=False, separators=(",", ":")))'
MCP_TEXT="$(mcp_call "{\"path\":\"$PROJ/a.sh\"}")"
CLI_TEXT="$("$TAKO_BIN" run-config "$PROJ/a.sh" 2>&1)"
printf '%s' "$CLI_TEXT" | python3 -c "$COMPACT" > "$TMP/cli.txt" 2>/dev/null
printf '%s' "$MCP_TEXT" | python3 -c "$COMPACT" > "$TMP/mcp.txt" 2>/dev/null
if [ -s "$TMP/cli.txt" ] && cmp -s "$TMP/cli.txt" "$TMP/mcp.txt"; then
  pass "② 読むだけの応答がキーの並びまでバイト一致（$(wc -c < "$TMP/cli.txt" | tr -d ' ') バイト）"
else
  fail "② CLI と MCP の応答がずれる: $(oneline "$MCP_TEXT" | head -c 300)"
fi
M2="$(mcp_call "{\"path\":\"$PROJ/a.sh\",\"args\":\"m1 $SECRET_ARG\",\"env\":{\"APP_ENV\":null}}")"
check_eq "② MCP で引数を設定・環境変数を削除した" \
  '[{"action": "reset", "field": "env", "scope": "file"}, {"action": "set", "field": "args", "scope": "file"}]' \
  "$(field updated "$M2")"
O2="$(run_file "$PROJ/a.sh" "$OUT")"
echo "  出力: $(oneline "$O2")"
check_has "② MCP で消した APP_ENV は unset・残りの env は届く" "unset|$SECRET_ENV" "$O2"
check_has "② MCP で設定した引数が届く" "[m1][$SECRET_ARG]" "$O2"
M2b="$(mcp_call "{\"path\":\"$PROJ/a.sh\",\"env\":{\"A\":1}}")"
check_has "② env の値が数値なら拒否する" "__RPC_ERROR__" "$M2b"
# 以降のために戻す
"$TAKO_BIN" run-config "$PROJ/a.sh" --env APP_ENV=dev --args "x 'y z' $SECRET_ARG" >/dev/null 2>&1

echo
echo "== ③ --before \"false\" で本体が走らず終了コードが 1 =="
R3="$("$TAKO_BIN" run-config "$PROJ/a.sh" --before false 2>&1)"
check_eq "③ before を保存した" '[{"action": "set", "field": "before", "scope": "file"}]' "$(field updated "$R3")"
O3="$(run_file "$PROJ/a.sh" "$OUT")"
echo "  出力: $(oneline "$O3")"
check_has "③ 実行ペインの終了コードが 1" "exit=1" "$O3"
check_has "③ 本体は走らない（出力ファイルができない）" "__NO_OUTPUT__" "$O3"

echo
echo "== ④ before が成功すれば本体が走る / --cwd sub =="
MARK="$PROJ/before.mark"
R4="$("$TAKO_BIN" run-config "$PROJ/a.sh" --before "touch '$MARK' && true $SECRET_BEFORE" --cwd sub 2>&1)"
check_eq "④ before と cwd を保存した" \
  '[{"action": "set", "field": "cwd", "scope": "file"}, {"action": "set", "field": "before", "scope": "file"}]' \
  "$(field updated "$R4")"
rm -f "$MARK"
O4="$(run_file "$PROJ/a.sh" "$OUT")"
echo "  出力: $(oneline "$O4")"
check_has "④ 終了コード 0" "exit=0" "$O4"
[ -f "$MARK" ] && pass "④ before が走った" || fail "④ before が走っていない"
check_eq "④ env と引数も届く" "OK" "$(judge_env_args "$O4")"
check_has "④ 作業ディレクトリが sub" "$PROJ/sub" "$O4"
"$TAKO_BIN" run-config "$PROJ/a.sh" --reset before cwd >/dev/null 2>&1

echo
echo "== ⑤ コンパイル系（.c）は生成物へ引数を渡す =="
if command -v cc >/dev/null 2>&1; then
  mkdir -p "$FIX/c"
  cat > "$FIX/c/hello.c" <<C
#include <stdio.h>
int main(int argc, char **argv) {
  FILE *f = fopen("$FIX/c/hello.out", "w");
  for (int i = 1; i < argc; i++) fprintf(f, "[%s]", argv[i]);
  fprintf(f, "\n");
  fclose(f);
  return 0;
}
C
  "$TAKO_BIN" run-config "$FIX/c/hello.c" --args "c1 'c 2'" >/dev/null 2>&1
  O5="$(run_file "$FIX/c/hello.c" "$FIX/c/hello.out")"
  echo "  出力: $(oneline "$O5")"
  check_has "⑤ ./hello へ引数が渡る" "[c1][c 2]" "$O5"
else
  echo "  cc が無いので未実測"
fi

echo
echo "== ⑥ 検証: 使えない指定は保存しない =="
BEFORE_STORE="$(cat "$STORE")"
E6a="$("$TAKO_BIN" run-config "$PROJ/a.sh" --env 1A=x 2>&1)"; RC=$?
check_rc_nonzero "⑥ 数字で始まる名前は非 0" "$RC" "$E6a"
E6b="$("$TAKO_BIN" run-config "$PROJ/a.sh" --env 'A;B=x' 2>&1)"; RC=$?
check_rc_nonzero "⑥ 文が混ざる名前は非 0" "$RC" "$E6b"
E6c="$("$TAKO_BIN" run-config "$PROJ/a.sh" --cwd nope 2>&1)"; RC=$?
check_rc_nonzero "⑥ 無い作業ディレクトリは非 0" "$RC" "$E6c"
E6d="$("$TAKO_BIN" run-config "$PROJ/a.sh" --env NOVALUE 2>&1)"; RC=$?
check_rc_nonzero "⑥ K=V でない --env は非 0" "$RC" "$E6d"
E6e="$("$TAKO_BIN" run-config "$PROJ/a.sh" --env A=1 --unset-env A 2>&1)"; RC=$?
check_rc_nonzero "⑥ 同じ名前の設定と削除は非 0" "$RC" "$E6e"
check_eq "⑥ run-configs.json は変わらない" "$BEFORE_STORE" "$(cat "$STORE")"

echo
echo "== ⑦ 器あり（TAKO_PERSIST=1）でも env と引数が届く =="
stop_isolated_gui "$APP_PID"
APP_PID=""
start_gui "$TMP/app-persist.log" TAKO_PERSIST=1 || exit 1
O7="$(run_file "$PROJ/a.sh" "$OUT")"
echo "  出力: $(oneline "$O7")"
SESSIONS="$(tmux -L "$TMUX_SOCKET" list-sessions 2>/dev/null | wc -l | tr -d ' ')"
if [ "${SESSIONS:-0}" -gt 0 ]; then
  pass "⑦ 器（tmux セッション）が $SESSIONS 本ある = 器の経路を通った"
else
  fail "⑦ 器が 0 本 = 器の経路に入っていない（TAKO_PERSIST が効いていない）"
fi
check_eq "⑦ 器の中でも env と引数が届く" "OK" "$(judge_env_args "$O7")"

echo
echo "== ⑧ 診断ログに env の値・引数・実行前コマンドが出ない =="
# 実行前コマンドの目印を通す（④ で保存して走らせ済み。器ありでももう 1 回）
"$TAKO_BIN" run-config "$PROJ/a.sh" --before "true $SECRET_BEFORE" >/dev/null 2>&1
O8="$(run_file "$PROJ/a.sh" "$OUT")"
check_has "⑧ 器ありで before 付きでも走る" "exit=0" "$O8"
LOGS=""
for f in "$TAKO_DATA_DIR/persist.log" "$TAKO_DATA_DIR/perf.log" "$TAKO_DATA_DIR/panic.log" \
  "$TMP/app.log" "$TMP/app-persist.log" "$TMP/mcp-err.log"; do
  [ -f "$f" ] && LOGS="$LOGS $f"
done
echo "  見たログ:$(for f in $LOGS; do printf ' %s(%s 行)' "${f##*/}" "$(wc -l < "$f" | tr -d ' ')"; done)"
[ -s "$TAKO_DATA_DIR/persist.log" ] && pass "⑧ persist.log が書かれている（0 件が grep の空振りでない）" \
  || fail "⑧ persist.log が空 = 0 件の判定に意味が無い"
for mark in "$SECRET_ENV" "$SECRET_ARG" "$SECRET_BEFORE"; do
  # shellcheck disable=SC2086
  HITS="$(grep -l -- "$mark" $LOGS 2>/dev/null | tr '\n' ' ')"
  check_eq "⑧ 目印 ${mark} が診断ログに 0 件" "" "$HITS"
done

echo
echo "== ⑨ A/B: TAKO_1732_LEGACY=1 で立て直す =="
"$TAKO_BIN" run-config "$PROJ/a.sh" --reset before >/dev/null 2>&1
ON="$(run_file "$PROJ/a.sh" "$OUT")"
check_eq "⑨ 新しい挙動では届く" "OK" "$(judge_env_args "$ON")"
stop_isolated_gui "$APP_PID"
APP_PID=""
start_gui "$TMP/app-legacy.log" TAKO_1732_LEGACY=1 || exit 1
OL="$(run_file "$PROJ/a.sh" "$OUT")"
echo "  LEGACY の出力: $(oneline "$OL")"
check_eq "⑨ LEGACY では ① の判定が FAILED（保存しても実行に効かない）" "FAILED" "$(judge_env_args "$OL")"
check_has "⑨ LEGACY の実行は env も引数も無い" "unset|unset" "$OL"

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
