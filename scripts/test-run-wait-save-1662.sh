#!/usr/bin/env bash
# test-run-wait-save-1662.sh — Code Runner の `--wait` の上限・GUI 側の auto_close・
# 実行前の保存の実経路テスト（#1662）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#
#   ① `tako run --wait` は上限（`TAKO_RUN_WAIT_TIMEOUT_SECS`）で「まだ実行中」を返して非 0 で
#      終わる。実行は止めない（ペインは running のまま）
#   ② env が 0 のときは既定（600 秒）へ落ちる（0 = 無制限にしない）。`--wait` は普通に終わる
#   ③ `--wait` 無しでも `--auto-close success` が成功した実行ペインを GUI 側で閉じる。
#      閉じたあとの `run-interactive-status` は控えから exited / closed=true を返す
#   ④ 失敗（非 0）は `--auto-close success` でも閉じない / `always` は閉じる
#   ⑤ `--wait --auto-close success` は GUI が先に閉じても 0 で返る
#   ⑥ プレビューで編集した**未保存の**内容が `tako run` で走る（ディスクも更新される）
#   ⑦ MCP `tako_run` も同じ（未保存の編集が走る）。CLI と MCP の応答が同じ形
#   ⑧ 保存できない（読み取り専用）ときは走らせない
#
# を実測する。**修正前のバイナリでも同じスクリプトが走る**（`TAKO_BIN=… APP_BIN=…`）ので、
# 症状（返らない / 閉じない / 古い内容が走る）はそちらの FAIL で示せる。待ちはすべて
# 外側の締め切り（`run_deadline`）で打ち切るので、修正前でも固まらない。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-run-wait-save-1662.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL
unset TAKO_RUN_WAIT_TIMEOUT_SECS

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1662-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1662-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  chmod -R u+w "$TMP" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

# 呼び出し側がバイナリを差し替えていなければ、いまのソースでビルドする
if [ -z "${TAKO_BIN:-}" ] && [ -z "${APP_BIN:-}" ]; then
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet) || exit 1
fi

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
echo "  CLI=${TAKO_BIN##*/} GUI=${APP_BIN##*/}"

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
printf '# tako:run: printf %s\n' "'ok-1662\\n'" > "$FIX/ok.sh"
printf '# tako:run: printf %s; (exit 3)\n' "'ng-1662\\n'" > "$FIX/ng.sh"
printf '# tako:run: sleep 60\n' > "$FIX/slow.sh"
printf '# tako:run: sleep 1\n' > "$FIX/quick.sh"
printf '# tako:run: /bin/sh ${file}\necho old-1662\n' > "$FIX/edit.sh"

# 応答 JSON から見たい項目だけを 1 行 1 項目で出す（最初の JSON だけを読む）
PY_FIELDS='import json,sys
raw = sys.stdin.read()
start = raw.find("{")
try:
    d, _ = json.JSONDecoder().raw_decode(raw[start:]) if start >= 0 else (None, 0)
    if d is None:
        raise ValueError("JSON が無い")
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
def walk(prefix, v):
    if isinstance(v, dict):
        for k, x in v.items():
            walk(prefix + k + ".", x)
    else:
        print("%s=%s" % (prefix[:-1], json.dumps(v, ensure_ascii=False) if isinstance(v, list) else v))
walk("", d)'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }

# タブ 0 のペイン数
pane_count() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(len(d["tabs"][0]["panes"]))'
}
# ペインの `run` 欄（list の値）。居なければ status=__GONE__
run_of() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"] == int(sys.argv[1]):
            r = p.get("run") or {}
            print("status=%s" % r.get("status"))
            print("exit_code=%s" % r.get("exit_code"))
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
# 状態で待つ: run_of の status が $2 になるまで（上限 20 秒）
wait_status() {
  local pane="$1" want="$2" i out
  for i in $(seq 1 200); do
    out="$(run_of "$pane")"
    [ "$(field status "$out")" = "$want" ] && return 0
    sleep 0.1
  done
  return 1
}
# 外側の締め切りつきで走らせる。打ち切ったら 124（修正前の「返らない」を固まらずに測る）
#   run_deadline <秒> <出力ファイル> <コマンド…>
run_deadline() {
  local secs="$1" out="$2" pid i=0
  shift 2
  "$@" > "$out" 2>&1 &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$i" -ge $((secs * 10)) ]; then
      kill "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 124
    fi
    sleep 0.1
    i=$((i + 1))
  done
  wait "$pid"
}
run_json() { # run_json <file> [引数…] → JSON（--wait なし）
  local file="$1"; shift
  "$TAKO_BIN" run "$file" --pane "$ROOT" ${1+"$@"} 2>&1
}
# MCP の tools/call を 1 回（引数は JSON）。返すのは content[0].text
mcp_call() {
  local tool="$1" args="$2"
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1662","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$tool\",\"arguments\":$args}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        c = (msg.get("result") or {}).get("content") or []
        print(c[0].get("text") if c else json.dumps(msg.get("error") or {}))
        break'
}
# プレビューで開いて編集を始め、自動保存を切る（未保存のまま残すため）。プレビューのペインを返す
open_for_edit() {
  local file="$1" out pane
  out="$("$TAKO_BIN" open "$file" --pane "$ROOT" --right 2>&1)"
  pane="$(printf '%s' "$out" | python3 -c "$PY_FIELDS" | sed -n 's/^pane=//p' | head -1)"
  [ -n "$pane" ] || { echo "open が pane を返さない: $out" >&2; return 1; }
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  "$TAKO_BIN" edit autosave false --pane "$pane" >/dev/null 2>&1
  printf '%s' "$pane"
}
dirty_of() {
  "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | python3 -c "$PY_FIELDS" | sed -n 's/^dirty=//p' | head -1
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
# 実行ペインを何枚か割るので窓を広めに取る（既定の 960x600 だと出力行が画面の外へ出る）
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
start_gui "$TMP/app.log" || exit $?
# **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"

echo
echo "== ① --wait は上限で「まだ実行中」を返して非 0（実行は止めない）=="
T0=$(date +%s)
run_deadline 15 "$TMP/wait-cap.log" \
  env TAKO_RUN_WAIT_TIMEOUT_SECS=3 "$TAKO_BIN" run "$FIX/slow.sh" --pane "$ROOT" --new-pane --wait
RC=$?
T1=$(date +%s)
echo "  rc=$RC 所要=$((T1 - T0)) 秒（外側の締め切り 15 秒・上限 3 秒）"
sed 's/^/  | /' "$TMP/wait-cap.log"
if [ "$RC" = 124 ]; then
  fail "--wait が上限で返らない（外側の締め切りで打ち切った = #1662 の症状）"
elif [ "$RC" != 0 ]; then
  pass "--wait は上限で非 0 で返る（rc=${RC}）"
else
  fail "--wait が上限なのに 0 で返った"
fi
CAP="$(python3 -c "$PY_FIELDS" < "$TMP/wait-cap.log")"
check_eq "応答は status=running" "running" "$(field status "$CAP")"
check_eq "応答は timed_out=True" "True" "$(field timed_out "$CAP")"
check_eq "応答の limit_secs は env の値" "3" "$(field limit_secs "$CAP")"
if grep -q "まだ実行中" "$TMP/wait-cap.log"; then pass "「まだ実行中」を知らせる"; else fail "「まだ実行中」の知らせが無い"; fi
SLOW_PANE="$(sed -n 's/^pane \([0-9]*\) でコマンドを実行中.*/\1/p' "$TMP/wait-cap.log" | head -1)"
if [ -n "$SLOW_PANE" ]; then
  check_eq "打ち切ったあとも実行ペインは running（止めていない）" "running" "$(field status "$(run_of "$SLOW_PANE")")"
  "$TAKO_BIN" close --pane "$SLOW_PANE" >/dev/null 2>&1
else
  fail "実行ペインの ID を採れない"
fi

echo
echo "== ② env が 0 なら既定（600 秒）へ落ち、--wait は普通に終わる =="
run_deadline 30 "$TMP/wait-zero.log" \
  env TAKO_RUN_WAIT_TIMEOUT_SECS=0 "$TAKO_BIN" run "$FIX/quick.sh" --pane "$ROOT" --new-pane --wait
RC=$?
check_eq "env 0 の --wait は 0 で返る（0 を「即打ち切り」にしない）" "0" "$RC"
if grep -q "最長 600 秒" "$TMP/wait-zero.log"; then
  pass "env 0 の上限は既定の 600 秒（0 を「無制限」にしない）"
else
  fail "env 0 の上限が既定の 600 秒と表示されない: $(tr '\n' ' ' < "$TMP/wait-zero.log")"
fi
ZPANE="$(sed -n 's/^pane \([0-9]*\) でコマンドを実行中.*/\1/p' "$TMP/wait-zero.log" | head -1)"
[ -n "$ZPANE" ] && "$TAKO_BIN" close --pane "$ZPANE" >/dev/null 2>&1

echo
echo "== ③ --wait 無しでも --auto-close success は成功した実行ペインを閉じる =="
OK="$(run_json "$FIX/ok.sh" --new-pane --auto-close success | python3 -c "$PY_FIELDS")"
OK_PANE="$(field pane "$OK")"
if wait_status "$OK_PANE" "__GONE__"; then
  pass "誰も聞きに来なくても成功で閉じた（pane ${OK_PANE}）"
else
  fail "--wait 無しの --auto-close success で閉じない（#1662 の症状。status=$(field status "$(run_of "$OK_PANE")")）"
fi
ST_CLI="$("$TAKO_BIN" run-interactive-status "$OK_PANE" 2>&1)"
ST="$(printf '%s' "$ST_CLI" | python3 -c "$PY_FIELDS")"
check_eq "閉じたあとの status は exited" "exited" "$(field status "$ST")"
check_eq "閉じたあとの exit_code は 0" "0" "$(field exit_code "$ST")"
check_eq "閉じたあとの closed は True" "True" "$(field closed "$ST")"
ST_MCP="$(mcp_call tako_run_interactive_status "{\"pane\":$OK_PANE}" | python3 -c "$PY_FIELDS")"
check_eq "MCP tako_run_interactive_status も同じ応答" "$ST" "$ST_MCP"

echo
echo "== ④ 失敗は success では閉じない / always は閉じる =="
NG="$(run_json "$FIX/ng.sh" --new-pane --auto-close success | python3 -c "$PY_FIELDS")"
NG_PANE="$(field pane "$NG")"
if wait_status "$NG_PANE" "exited"; then
  # 定期更新（2 秒ごと）を 2 回以上またいでも残っていること
  STILL=1
  for i in $(seq 1 50); do
    [ "$(field status "$(run_of "$NG_PANE")")" = "exited" ] || { STILL=0; break; }
    sleep 0.1
  done
  check_eq "失敗（exit 3）は success で閉じない（5 秒観測）" "1" "$STILL"
else
  fail "失敗の実行が exited へ確定しない"
fi
check_eq "失敗の status は closed=False" "False" "$(field closed "$("$TAKO_BIN" run-interactive-status "$NG_PANE" 2>&1 | python3 -c "$PY_FIELDS")")"
AL="$(run_json "$FIX/ng.sh" --new-pane --auto-close always | python3 -c "$PY_FIELDS")"
AL_PANE="$(field pane "$AL")"
if wait_status "$AL_PANE" "__GONE__"; then pass "always は失敗でも閉じる"; else fail "always が失敗で閉じない"; fi
"$TAKO_BIN" close --pane "$NG_PANE" >/dev/null 2>&1

echo
echo "== ⑤ --wait --auto-close success は GUI が先に閉じても 0 で返る =="
run_deadline 30 "$TMP/wait-close.log" \
  "$TAKO_BIN" run "$FIX/ok.sh" --pane "$ROOT" --new-pane --auto-close success --wait
check_eq "--wait --auto-close success は 0 で返る" "0" "$?"
if grep -q '"closed": true' "$TMP/wait-close.log"; then pass "応答に closed=true が載る"; else fail "応答に closed=true が無い: $(tr '\n' ' ' < "$TMP/wait-close.log")"; fi

echo
echo "== ⑥ 未保存の編集が tako run で走る（CLI）=="
BASE="$(pane_count)"
PREV="$(open_for_edit "$FIX/edit.sh")" || exit 1
"$TAKO_BIN" edit apply "$(printf '# tako:run: /bin/sh ${file}\necho new-cli-1662')" --pane "$PREV" >/dev/null 2>&1
check_eq "編集は未保存（dirty=True）" "True" "$(dirty_of "$PREV")"
case "$(cat "$FIX/edit.sh")" in *old-1662*) pass "ディスクはまだ古い内容" ;; *) fail "ディスクが先に書き換わっている" ;; esac
CLI_RAW="$(run_json "$FIX/edit.sh" --new-pane)"
CLI_RUN="$(printf '%s' "$CLI_RAW" | python3 -c "$PY_FIELDS")"
CLI_PANE="$(field pane "$CLI_RUN")"
wait_status "$CLI_PANE" "exited" || fail "CLI の実行が終わらない"
SCREEN="$(screen_of "$CLI_PANE")"
case "$SCREEN" in
  *new-cli-1662*) pass "CLI の tako run は未保存の編集の内容を走らせた" ;;
  *old-1662*) fail "CLI の tako run がディスク上の古い内容を走らせた（#1662 の症状）" ;;
  *) fail "CLI の実行の出力が見えない: $(printf '%s' "$SCREEN" | tr '\n' ' ')" ;;
esac
check_eq "応答の saved_panes はそのプレビュー" "[$PREV]" "$(field saved_panes "$CLI_RUN")"
check_eq "保存後は dirty=False" "False" "$(dirty_of "$PREV")"
case "$(cat "$FIX/edit.sh")" in *new-cli-1662*) pass "ディスクも新しい内容になった" ;; *) fail "ディスクが古いまま" ;; esac

echo
echo "== ⑦ MCP tako_run も同じ（未保存の編集が走る）/ CLI と MCP の応答が同じ形 =="
# 実行ペインの高さを確保する（積むと出力行が画面の外へ押し出され、tako read で見えない）
"$TAKO_BIN" close --pane "$CLI_PANE" >/dev/null 2>&1
"$TAKO_BIN" edit apply "$(printf '# tako:run: /bin/sh ${file}\necho new-mcp-1662')" --pane "$PREV" >/dev/null 2>&1
check_eq "MCP の前に未保存（dirty=True）" "True" "$(dirty_of "$PREV")"
MCP_RAW="$(mcp_call tako_run "{\"path\":\"$FIX/edit.sh\",\"pane\":$ROOT,\"new_pane\":true}")"
MCP_RUN="$(printf '%s' "$MCP_RAW" | python3 -c "$PY_FIELDS")"
MCP_PANE="$(field pane "$MCP_RUN")"
wait_status "$MCP_PANE" "exited" || fail "MCP の実行が終わらない"
SCREEN="$(screen_of "$MCP_PANE")"
case "$SCREEN" in
  *new-mcp-1662*) pass "MCP tako_run は未保存の編集の内容を走らせた" ;;
  *new-cli-1662*|*old-1662*) fail "MCP tako_run がディスク上の古い内容を走らせた（#1662 の症状）" ;;
  *) fail "MCP の実行の出力が見えない: $(printf '%s' "$SCREEN" | tr '\n' ' ')" ;;
esac
check_eq "MCP の saved_panes もそのプレビュー" "[$PREV]" "$(field saved_panes "$MCP_RUN")"
# 応答の形（キーの集合）と、実行ごとに変わらない値が CLI と一致する
same_shape="$(python3 - "$CLI_RAW" "$MCP_RAW" <<'PY'
import json, sys
def first(raw):
    s = raw.find("{")
    return json.JSONDecoder().raw_decode(raw[s:])[0]
cli, mcp = first(sys.argv[1]), first(sys.argv[2])
diff = []
if sorted(cli) != sorted(mcp):
    diff.append("keys: %s / %s" % (sorted(cli), sorted(mcp)))
for k in ["path", "profile", "command", "cwd", "source", "project", "auto_close", "saved_panes", "stopped_running"]:
    if cli.get(k) != mcp.get(k):
        diff.append("%s: %r / %r" % (k, cli.get(k), mcp.get(k)))
print("same" if not diff else "; ".join(diff))
PY
)"
check_eq "CLI と MCP の tako run の応答が同じ形・同じ値" "same" "$same_shape"

echo
echo "== ⑧ 保存できない（読み取り専用）ときは走らせない =="
"$TAKO_BIN" edit apply "$(printf '# tako:run: /bin/sh ${file}\necho readonly-1662')" --pane "$PREV" >/dev/null 2>&1
chmod 444 "$FIX/edit.sh"
BEFORE_COUNT="$(pane_count)"
"$TAKO_BIN" run "$FIX/edit.sh" --pane "$ROOT" --new-pane > "$TMP/ro.log" 2>&1
RC=$?
chmod 644 "$FIX/edit.sh"
if [ "$RC" != 0 ]; then pass "保存できないと非 0（rc=${RC}）"; else fail "保存できないのに走らせた（古い内容が走る）"; fi
if grep -q "実行前の保存に失敗" "$TMP/ro.log"; then pass "理由（実行前の保存に失敗）を返す"; else fail "理由が無い: $(tr '\n' ' ' < "$TMP/ro.log")"; fi
check_eq "実行ペインは作らない（ペイン数が変わらない）" "$BEFORE_COUNT" "$(pane_count)"
check_eq "未保存の編集はそのまま残る（dirty=True）" "True" "$(dirty_of "$PREV")"
echo "  （参考: 最初のペイン数 ${BASE}）"

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" = 0 ]
