#!/usr/bin/env bash
# test-run-pane-reuse-1657.sh — Code Runner の実行ペインの再利用・終了の案内・バッジの実経路テスト（#1657）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#
#   ① 同じファイルを `tako run` で 5 回走らせてもタブのペイン数が変わらない
#      （`reused_from` が 1 つ前の実行ペインを指す）。`--new-pane` で 1 枚増える
#   ② 実行ペインの画面（`tako read` と器の `tmux capture-pane` の両方）に内部マーカー
#      `__TAKO_EXIT=` が出ず、「[tako] 終了コード N / Enter で…」（日英どちらか）が出る
#   ③ `tako list` の `run`（status / exit_code / outcome）と `run-interactive-status` が
#      同じ終了コードを返す。失敗（非 0）は outcome=failure
#   ④ `tako run --wait` が終了コードで返る（成功 0 / 失敗は非 0）
#   ⑤ エッジ: 実行中にもう一度走らせる（止めて走らせ直す = stopped_running=true）/
#      実行ペインを閉じたあとに走らせる（新しく分割）/ 別ファイル（別の実行ペイン）/
#      `tako:run[name]` の別プロファイル（別の実行ペイン）
#   ⑥ MCP `tako_run` が同じ dispatch を通る（CLI で作った実行ペインを差し替える）
#   ⑦ A/B: 同じバイナリを `TAKO_1657_LEGACY=1` で立てると 5 回で 5 枚積み、画面にマーカーが出る
#   ⑧ タイトルバーのバッジ（visual-test の `run-badge` 節。Metal の描画結果を読み戻すので
#      画面収録の権限は要らない）
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-run-pane-reuse-1657.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1657-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1657-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# バッジの節（⑧）は feature 付きのビルドにしか無い。CLI / MCP の節も同じバイナリで回す
(cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME" "$TMP/zdot"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
export ZDOTDIR="$TMP/zdot"
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
FIX="$TMP/fixtures"
mkdir -p "$FIX"
printf '# tako:run: printf %s\n# tako:run[other]: printf %s\n' \
  "'ok-1657\\n'" "'other-1657\\n'" > "$FIX/ok.sh"
printf '# tako:run: printf %s; (exit 3)\n' "'ng-1657\\n'" > "$FIX/ng.sh"
printf '# tako:run: sleep 60\n' > "$FIX/slow.sh"
printf '# tako:run: printf %s\n' "'b-1657\\n'" > "$FIX/b.sh"

# 応答 JSON から見たい項目だけを 1 行 1 項目で出す
PY_FIELDS='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
def walk(prefix, v):
    if isinstance(v, dict):
        for k, x in v.items():
            walk(prefix + k + ".", x)
    else:
        print("%s=%s" % (prefix[:-1], v))
walk("", d)'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }

# タブ 0 のペイン数
pane_count() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(len(d["tabs"][0]["panes"]))'
}
# ペインの `run` 欄（list の値）
run_of() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"] == int(sys.argv[1]):
            r = p.get("run") or {}
            print("status=%s" % r.get("status"))
            print("exit_code=%s" % r.get("exit_code"))
            print("outcome=%s" % r.get("outcome"))
            print("profile=%s" % r.get("profile"))
            sys.exit(0)
print("status=__GONE__")' "$1"
}
# 画面テキスト（tako read）
screen_of() {
  "$TAKO_BIN" read --pane "$1" 2>/dev/null | python3 -c 'import json,sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
    lines = d.get("lines") if isinstance(d, dict) else None
    print("\n".join(lines) if isinstance(lines, list) else (d.get("text") if isinstance(d, dict) else raw))
except Exception:
    print(raw)'
}
# 終了が確定するまで待つ（**状態で待つ**。上限は 20 秒）
wait_exited() {
  local pane="$1" i out
  for i in $(seq 1 200); do
    out="$(run_of "$pane")"
    [ "$(field status "$out")" = "exited" ] && { printf '%s' "$out"; return 0; }
    sleep 0.1
  done
  printf '%s' "$out"
  return 1
}
run_cli() { # run_cli <file> [引数…] → JSON
  local file="$1"; shift
  "$TAKO_BIN" run "$file" --pane "$ROOT" ${1+"$@"} 2>&1
}

start_gui() { # start_gui <ログ> [VAR=VAL…]
  local log="$1"; shift
  # 面を用意できなければ起動せず 4（未実測）で返る（#1744）。関数の中でも exit で止める
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || return 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; return 1; }
  echo "  pid=$APP_PID root=$ROOT"
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log" || exit $?
BASE_COUNT="$(pane_count)"
echo "  最初のペイン数=$BASE_COUNT"

echo
echo "== ① 同じファイルを 5 回走らせても 1 枚のまま =="
PREV=""
COUNTS=""
for i in 1 2 3 4 5; do
  OUT="$(run_cli "$FIX/ok.sh" | python3 -c "$PY_FIELDS")"
  PANE="$(field pane "$OUT")"
  REUSED="$(field reused_from "$OUT")"
  COUNTS="$COUNTS $(pane_count)"
  if [ "$i" = 1 ]; then
    check_eq "1 回目は新しく分割する（reused_from=null）" "None" "$REUSED"
  else
    check_eq "$i 回目は 1 つ前の実行ペインを差し替える" "$PREV" "$REUSED"
  fi
  PREV="$PANE"
done
echo "  ペイン数の推移:$COUNTS"
check_eq "5 回走らせたあとのペイン数（最初 + 1）" "$((BASE_COUNT + 1))" "$(pane_count)"
OK_PANE="$PREV"

echo
echo "== ② 画面にマーカーが出ず、人の言葉の案内が出る =="
OK_RUN="$(wait_exited "$OK_PANE")"
check_eq "成功の実行は exited に確定する" "exited" "$(field status "$OK_RUN")"
sleep 0.3
SCREEN="$(screen_of "$OK_PANE")"
echo "  --- tako read --pane ${OK_PANE}（空行を除く）---"
printf '%s\n' "$SCREEN" | sed '/^[[:space:]]*$/d' | sed 's/^/  | /'
case "$SCREEN" in
  *__TAKO_EXIT=*) fail "tako read に内部マーカーが出ている" ;;
  *) pass "tako read に __TAKO_EXIT= が無い" ;;
esac
case "$SCREEN" in
  *"[tako] 終了コード 0"*|*"[tako] Exit code 0"*) pass "「[tako] 終了コード 0 / Enter で…」が出る" ;;
  *) fail "終了の案内が出ていない" ;;
esac
case "$SCREEN" in
  *ok-1657*) pass "実行の出力そのものは残る" ;;
  *) fail "実行の出力が見えない" ;;
esac
# 器（tmux）の実画面も見る = capture-pane の文字列
SESSIONS="$(tmux -L "$TMUX_SOCKET" list-sessions -F '#{session_name}' 2>/dev/null)"
if [ -n "$SESSIONS" ]; then
  LEAK=0
  N=0
  for s in $SESSIONS; do
    N=$((N + 1))
    if tmux -L "$TMUX_SOCKET" capture-pane -p -t "$s" 2>/dev/null | grep -q '__TAKO_EXIT='; then
      LEAK=$((LEAK + 1))
    fi
  done
  echo "  器のセッション $N 本を capture-pane で走査"
  check_eq "tmux capture-pane のどのセッションにもマーカーが無い" "0" "$LEAK"
else
  echo "  (注) この起動は器（tmux）を使っていない = capture-pane の節は省略（tako read で確認済み）"
fi

echo
echo "== ③ list の run と run-interactive-status が同じ終了コード =="
check_eq "成功: exit_code=0" "0" "$(field exit_code "$OK_RUN")"
check_eq "成功: outcome=success" "success" "$(field outcome "$OK_RUN")"
check_eq "成功: profile=default" "default" "$(field profile "$OK_RUN")"
NG="$(run_cli "$FIX/ng.sh" | python3 -c "$PY_FIELDS")"
NG_PANE="$(field pane "$NG")"
check_eq "別ファイルは別の実行ペイン（reused_from=null）" "None" "$(field reused_from "$NG")"
NG_RUN="$(wait_exited "$NG_PANE")"
check_eq "失敗: exit_code=3" "3" "$(field exit_code "$NG_RUN")"
check_eq "失敗: outcome=failure" "failure" "$(field outcome "$NG_RUN")"
ST="$("$TAKO_BIN" run-interactive-status "$NG_PANE" 2>&1 | python3 -c "$PY_FIELDS")"
check_eq "run-interactive-status も exited" "exited" "$(field status "$ST")"
check_eq "run-interactive-status も 3" "3" "$(field exit_code "$ST")"
NG_SCREEN="$(screen_of "$NG_PANE")"
case "$NG_SCREEN" in
  *"[tako] 終了コード 3"*|*"[tako] Exit code 3"*) pass "失敗の案内に終了コード 3 が載る" ;;
  *) fail "失敗の案内が出ていない: $(printf '%s' "$NG_SCREEN" | tr '\n' ' ')" ;;
esac

echo
echo "== ④ tako run --wait が終了コードで返る =="
"$TAKO_BIN" run "$FIX/ok.sh" --pane "$ROOT" --wait > "$TMP/wait-ok.log" 2>&1
check_eq "成功の --wait は 0 で返る" "0" "$?"
"$TAKO_BIN" run "$FIX/ng.sh" --pane "$ROOT" --wait > "$TMP/wait-ng.log" 2>&1
WAIT_RC=$?
if [ "$WAIT_RC" != 0 ]; then pass "失敗の --wait は非 0 で返る（rc=${WAIT_RC}）"; else fail "失敗の --wait が 0 で返った"; fi
check_eq "--wait を挟んでもペインは積まない" "$((BASE_COUNT + 2))" "$(pane_count)"

echo
echo "== ⑤ エッジ =="
SLOW1="$(run_cli "$FIX/slow.sh" | python3 -c "$PY_FIELDS")"
SLOW1_PANE="$(field pane "$SLOW1")"
sleep 0.5
check_eq "実行中は running" "running" "$(field status "$(run_of "$SLOW1_PANE")")"
SLOW2="$(run_cli "$FIX/slow.sh" | python3 -c "$PY_FIELDS")"
check_eq "実行中にもう一度: 同じ実行ペインを差し替える" "$SLOW1_PANE" "$(field reused_from "$SLOW2")"
check_eq "実行中にもう一度: 止めて走らせ直した（stopped_running）" "True" "$(field stopped_running "$SLOW2")"
check_eq "実行中にもう一度: 前のペインは消えている" "__GONE__" "$(field status "$(run_of "$SLOW1_PANE")")"
SLOW2_PANE="$(field pane "$SLOW2")"
COUNT_BEFORE_CLOSE="$(pane_count)"
"$TAKO_BIN" close --pane "$SLOW2_PANE" >/dev/null 2>&1
SLOW3="$(run_cli "$FIX/slow.sh" | python3 -c "$PY_FIELDS")"
check_eq "閉じたあとに走らせると新しく分割する" "None" "$(field reused_from "$SLOW3")"
check_eq "閉じたあとの分割でペイン数は元へ戻る" "$COUNT_BEFORE_CLOSE" "$(pane_count)"
OTHER="$(run_cli "$FIX/ok.sh" --profile other | python3 -c "$PY_FIELDS")"
check_eq "tako:run[other] は別の実行ペイン" "None" "$(field reused_from "$OTHER")"
OTHER_RUN="$(wait_exited "$(field pane "$OTHER")")"
check_eq "tako:run[other] の profile=other" "other" "$(field profile "$OTHER_RUN")"
COUNT_NEW="$(pane_count)"
NEWP="$(run_cli "$FIX/ok.sh" --new-pane | python3 -c "$PY_FIELDS")"
check_eq "--new-pane は差し替えない" "None" "$(field reused_from "$NEWP")"
check_eq "--new-pane で 1 枚増える" "$((COUNT_NEW + 1))" "$(pane_count)"

echo
echo "== ⑥ MCP tako_run が同じ dispatch を通る =="
B1="$(run_cli "$FIX/b.sh" | python3 -c "$PY_FIELDS")"
B1_PANE="$(field pane "$B1")"
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1657","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run","arguments":{"path":"$FIX/b.sh","pane":$ROOT}}}
JSONL
# **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
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
        c = (msg.get("result") or {}).get("content") or []
        print(c[0].get("text") if c else "{}")
        break' "$TMP/mcp-out.jsonl")"
MCP="$(printf '%s' "$MCP_TEXT" | python3 -c "$PY_FIELDS")"
check_eq "MCP tako_run は CLI で作った実行ペインを差し替える" "$B1_PANE" "$(field reused_from "$MCP")"
TOOLS_HAS_NEW_PANE="$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' '{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}' \
  | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
  | python3 -c 'import json,sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 3:
        tools = {t["name"]: t for t in msg["result"]["tools"]}
        print("new_pane" in tools["tako_run"]["inputSchema"]["properties"])')"
check_eq "tako_run の引数に new_pane がある（ツールは増やしていない）" "True" "$TOOLS_HAS_NEW_PANE"

stop_isolated_gui "$APP_PID"
APP_PID=""
tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true

echo
echo "== ⑥' 器（tmux）つきでも同じ: 差し替えと capture-pane =="
# 隔離起動の既定は器なし（`TAKO_ISOLATED` が `TAKO_PERSIST=0` を置く）。専用ソケットの
# 器で明示して立て、**器のセッションに残る実画面**（capture-pane）を見る
start_gui "$TMP/app-tmux.log" TAKO_PERSIST=1 || exit $?
TBASE="$(pane_count)"
T1="$(run_cli "$FIX/ok.sh" | python3 -c "$PY_FIELDS")"
T2="$(run_cli "$FIX/ok.sh" | python3 -c "$PY_FIELDS")"
check_eq "器つき: 2 回目は差し替える" "$(field pane "$T1")" "$(field reused_from "$T2")"
check_eq "器つき: ペインは積まない" "$((TBASE + 1))" "$(pane_count)"
T2_RUN="$(wait_exited "$(field pane "$T2")")"
check_eq "器つき: 終了コード 0 で確定する" "0" "$(field exit_code "$T2_RUN")"
sleep 0.3
TSESSIONS="$(tmux -L "$TMUX_SOCKET" list-sessions -F '#{session_name}' 2>/dev/null)"
TN=0
TLEAK=0
THINT=0
for s in $TSESSIONS; do
  TN=$((TN + 1))
  CAP="$(tmux -L "$TMUX_SOCKET" capture-pane -p -t "$s" 2>/dev/null)"
  case "$CAP" in *__TAKO_EXIT=*) TLEAK=$((TLEAK + 1)) ;; esac
  case "$CAP" in *"[tako] 終了コード 0"*|*"[tako] Exit code 0"*) THINT=$((THINT + 1)) ;; esac
done
echo "  器のセッション ${TN} 本を capture-pane で走査（案内が見えた本数=${THINT}）"
if [ "$TN" -ge 1 ]; then pass "器のセッションがある（器つきで走った）"; else fail "器のセッションが無い"; fi
check_eq "capture-pane のどのセッションにもマーカーが無い" "0" "$TLEAK"
check_eq "capture-pane に案内が出ている実行ペインは 1 本（差し替えた旧ペインの器は消えた）" "1" "$THINT"
stop_isolated_gui "$APP_PID"
APP_PID=""
tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true

echo
echo "== ⑦ A/B: TAKO_1657_LEGACY=1（同じバイナリ）は積んでマーカーを出す =="
start_gui "$TMP/app-legacy.log" TAKO_1657_LEGACY=1 || exit $?
LBASE="$(pane_count)"
LAST=""
for i in 1 2 3 4 5; do
  LAST="$(run_cli "$FIX/ok.sh" | python3 -c "$PY_FIELDS")"
done
LCOUNT="$(pane_count)"
echo "  legacy のペイン数: 最初=$LBASE → 5 回後=$LCOUNT"
check_eq "legacy は 5 回で 5 枚積む（#1657 前の症状の再現）" "$((LBASE + 5))" "$LCOUNT"
LPANE="$(field pane "$LAST")"
wait_exited "$LPANE" >/dev/null
LSCREEN="$(screen_of "$LPANE")"
case "$LSCREEN" in
  *__TAKO_EXIT=0*) pass "legacy は画面に __TAKO_EXIT=0 が出る（#1657 前の症状の再現）" ;;
  *) fail "legacy なのに画面にマーカーが無い（A/B の比較先が崩れている）" ;;
esac
stop_isolated_gui "$APP_PID"
APP_PID=""
tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true

echo
echo "== ⑧ タイトルバーのバッジ（visual-test の run-badge 節）=="
mkdir -p "$TMP/shots"
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
launch_isolated_gui "$TMP/app-visual.log" \
  TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
  TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=run-badge TAKO_VISUAL_DUMP_DIR="$TMP/shots" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait "$APP_PID"
VRC=$?
APP_PID=""
grep -E '^TAKO_VISUAL_1657|^TAKO_VISUAL_TEST_OK' "$TMP/app-visual.log" | sed 's/^/  /'
if [ "$VRC" = 4 ]; then
  echo "  (注) 起動後に面を見失い GUI が窓を開かなかった（終了コード 4。#1697）= バッジは未実測"
  fail "バッジの実ピクセルを測れていない（rc=4）"
else
  check_eq "visual-test の節が完走する" "0" "$VRC"
  if grep -q '^TAKO_VISUAL_1657: all_ok=true' "$TMP/app-visual.log"; then
    pass "成功=緑 / 失敗=赤 / 実行中=アクセントのバッジが実ピクセルで描かれている"
  else
    fail "バッジの色が期待どおりに描かれていない"
  fi
  if [ -n "${TAKO_1657_KEEP_SHOT:-}" ] && [ -f "$TMP/shots/run-badge-1657.png" ]; then
    cp "$TMP/shots/run-badge-1657.png" "$TAKO_1657_KEEP_SHOT"
    echo "  画像: ${TAKO_1657_KEEP_SHOT}（共有前に中身を目で確かめる = #927）"
  fi
fi

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" = 0 ]
