#!/usr/bin/env bash
# test-run-pane-followup-1778.sh — 実行ペインの続き（#1778）の実経路テスト
#
# 隔離した data / tmux で**実 tako-app** を立て、
#
#   ① `tako split --command` が失敗したペインは、画面に `__TAKO_EXIT=` を出さず
#      「[tako] 終了コード N / Enter で…」（日英どちらか）だけを出して止まる。終了コードは
#      `tako run-interactive-status` と MCP `tako_run_interactive_status` で読める
#      （MCP `tako_split_pane` で作ったペインも同じ）
#   ② 実行中のプログラム自身が `__TAKO_EXIT=7` を印字してから 0 で終わっても、確定する
#      終了コードは 0（印字が画面に出た時点ではまだ running）。`tako run --wait` も 0 で返る
#   ③ `--wait`（run / run-interactive）の上限での打ち切りは終了コード 124 で、応答は
#      `timed_out: true`。コマンドが自分で 1 / 124 で終わったときは 1
#   ④ auto_close で閉じた実行ペインのペインログのマーカーは `close:auto`
#      （GUI の終了検知で閉じた場合も `--wait` の聞き直しで閉じた場合も）。
#      手で閉じたものは従来どおり `close:dispatch(cli)` = 見分けられる
#   ⑤ エッジ: 出力が大量 / プログラムがマーカーを大量に印字 / 即座に終わる /
#      側路を用意できない（退避路 = 画面のマーカーで伝わり、`--wait` も止まらない）
#
# を実測する。**修正前のバイナリでも同じスクリプトが走る**（`TAKO_BIN=… APP_BIN=…`）ので、
# 症状（画面のマーカー / 偽の 7 / exit 1 / close:internal）はそちらの FAIL で示せる。
# 待ちはすべて外側の締め切り（`run_deadline`。打ち切りの印は **125**。124 は ③ で CLI 自身が
# 返す値なので使わない）で打ち切るので、修正前でも固まらない。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-run-pane-followup-1778.sh
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

TMP="$(mktemp -d /tmp/tako-1778-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1778-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
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
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_PANE_LOG_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 材料 ---------------------------------------------------------------------
FIX="$TMP/fixtures"
mkdir -p "$FIX"
printf '# tako:run: printf %s\n' "'ok-1778\\n'" > "$FIX/ok.sh"
printf '# tako:run: printf %s; (exit 1)\n' "'ng-1778\\n'" > "$FIX/ng1.sh"
printf '# tako:run: printf %s; (exit 124)\n' "'ng124-1778\\n'" > "$FIX/ng124.sh"
printf '# tako:run: sleep 60\n' > "$FIX/slow.sh"
# ② プログラム自身が終了マーカーを印字してから 0 で終わる
printf '# tako:run: printf %s; sleep 3\n' "'__TAKO_EXIT=7\\n'" > "$FIX/fake.sh"
# ⑤ 出力が大量 / マーカーを大量に印字 / 即座に終わる
printf '# tako:run: seq 1 200000; (exit 3)\n' > "$FIX/big.sh"
printf '# tako:run: i=0; while [ $i -lt 3000 ]; do echo __TAKO_EXIT=$i; i=$((i+1)); done; sleep 1\n' \
  > "$FIX/spam.sh"
printf '# tako:run: true\n' > "$FIX/instant.sh"

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
# 応答の**最後の** JSON（`--wait` は起動の応答のあとに結末の JSON を出す）
PY_LAST='import json,sys
raw = sys.stdin.read()
dec = json.JSONDecoder()
last, i = None, 0
while True:
    s = raw.find("{", i)
    if s < 0:
        break
    try:
        d, end = dec.raw_decode(raw[s:])
        last, i = d, s + end
    except Exception:
        i = s + 1
if last is None:
    print("__NOT_JSON__=1"); sys.exit(0)
for k, v in last.items():
    print("%s=%s" % (k, v))'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }

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
# 空白と改行を落とした画面（案内は狭いペインで折り返すので、その形で探す）
squashed_of() { screen_of "$1" | tr -d ' \n\t'; }
# 「終了コード N」の案内（日英どちらか）が画面にあるか
has_hint() { # has_hint <pane> <code>
  local s
  s="$(squashed_of "$1")"
  case "$s" in
    *"[tako]終了コード${2}/Enter"*|*"[tako]Exitcode${2}/Press"*) return 0 ;;
    *) return 1 ;;
  esac
}
status_of() { "$TAKO_BIN" run-interactive-status "$1" 2>&1 | python3 -c "$PY_FIELDS"; }
# 状態で待つ（上限 20 秒）: 述語関数が真になるまで
wait_until() { # wait_until <上限秒> <関数> [引数…]
  local secs="$1" i
  shift
  for i in $(seq 1 $((secs * 10))); do
    "$@" && return 0
    sleep 0.1
  done
  return 1
}
is_exited() { [ "$(field status "$(status_of "$1")")" = "exited" ]; }
pane_gone() { ! "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
ids = [p["id"] for t in d["tabs"] for p in t["panes"]]
sys.exit(0 if int(sys.argv[1]) in ids else 1)' "$1"; }
screen_has() { screen_of "$1" | grep -qF -- "$2"; }
# 外側の締め切りつきで走らせる。打ち切ったら 125（124 は CLI の「上限で打ち切った」なので使わない）
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
      return 125
    fi
    sleep 0.1
    i=$((i + 1))
  done
  wait "$pid"
}
# `--wait` の出力から実行ペインの ID（run は「でコマンドを実行中」、run-interactive は「で対話コマンドを起動」）
pane_of_wait_log() { sed -n 's/^pane \([0-9]*\) で.*/\1/p' "$1" | head -1; }
# ペインログの最後のクローズマーカーの発生源（無ければ空）。GUI が書くので少し待つ
# （bash 3.2 はコマンド置換の中の heredoc を読み違えるので、本文は変数に置く）
PY_CLOSE='import os, re, sys
d, pane = sys.argv[1], sys.argv[2]
pat = re.compile(r"_pane%s(?![0-9])" % re.escape(pane))
best = None
try:
    names = os.listdir(d)
except OSError:
    names = []
for name in names:
    if ".log" in name and pat.search(name):
        p = os.path.join(d, name)
        if best is None or os.path.getmtime(p) > os.path.getmtime(best):
            best = p
reason = ""
if best:
    for line in open(best, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if line.startswith("--- [クローズ: ") and line.endswith("] ---"):
            body = line[len("--- [クローズ: "):-len("] ---")]
            reason = body.rsplit(" ", 1)[0]
print(reason)'
close_reason_of() {
  local pane="$1" i r
  for i in $(seq 1 50); do
    r="$(python3 -c "$PY_CLOSE" "$TAKO_PANE_LOG_DIR" "$pane")"
    if [ -n "$r" ]; then printf '%s' "$r"; return 0; fi
    sleep 0.1
  done
  printf ''
}
# MCP の tools/call を 1 回（引数は JSON）。返すのは content[0].text
mcp_call() {
  local tool="$1" args="$2"
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1778","version":"0"}}}' \
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
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
start_gui "$TMP/app.log" || exit $?
# **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"

echo
echo "== ① split --command の失敗は案内だけを出し、終了コードは側路から読める =="
SP="$("$TAKO_BIN" split --pane "$ROOT" --down -- /bin/sh -c 'echo split-1778; exit 7' 2>/dev/null | tr -dc '0-9')"
[ -n "$SP" ] || { fail "split が pane を返さない"; SP=0; }
if wait_until 20 has_hint "$SP" 7; then
  pass "画面に「終了コード 7 / Enter で閉じる」の案内が出る（pane ${SP}）"
else
  fail "画面に「終了コード 7」の案内が無い: $(screen_of "$SP" | tr '\n' ' ')"
fi
if screen_has "$SP" "__TAKO_EXIT="; then
  fail "画面に内部マーカー __TAKO_EXIT= が出ている（#1778 の症状）: $(screen_of "$SP" | grep -F __TAKO_EXIT | head -1)"
else
  pass "画面に内部マーカー __TAKO_EXIT= が出ない"
fi
if screen_has "$SP" "split-1778"; then pass "コマンドの出力は残る"; else fail "コマンドの出力が見えない"; fi
ST_CLI_RAW="$("$TAKO_BIN" run-interactive-status "$SP" 2>&1)"
ST_CLI="$(printf '%s' "$ST_CLI_RAW" | python3 -c "$PY_FIELDS")"
echo "  CLI run-interactive-status: $(printf '%s' "$ST_CLI_RAW" | tr -d '\n' | tr -s ' ')"
check_eq "CLI: status は exited" "exited" "$(field status "$ST_CLI")"
check_eq "CLI: exit_code は 7" "7" "$(field exit_code "$ST_CLI")"
check_eq "CLI: 保持のペインは閉じない（closed=False）" "False" "$(field closed "$ST_CLI")"
ST_MCP_RAW="$(mcp_call tako_run_interactive_status "{\"pane\":$SP}")"
echo "  MCP tako_run_interactive_status: $(printf '%s' "$ST_MCP_RAW" | tr -d '\n' | tr -s ' ')"
ST_MCP="$(printf '%s' "$ST_MCP_RAW" | python3 -c "$PY_FIELDS")"
check_eq "MCP も CLI と同じ応答" "$ST_CLI" "$ST_MCP"
"$TAKO_BIN" close --pane "$SP" >/dev/null 2>&1
# MCP の tako_split_pane で作ったペインも同じ（CLI と同じ dispatch を通る）
MSP_RAW="$(mcp_call tako_split_pane "{\"pane\":$ROOT,\"direction\":\"down\",\"command\":[\"/bin/sh\",\"-c\",\"exit 5\"]}")"
MSP="$(printf '%s' "$MSP_RAW" | python3 -c "$PY_FIELDS" | sed -n 's/^pane=//p' | head -1)"
if [ -n "$MSP" ] && wait_until 20 has_hint "$MSP" 5; then
  pass "MCP tako_split_pane の失敗も案内が出る（pane ${MSP}）"
else
  fail "MCP tako_split_pane の失敗に案内が無い: ${MSP_RAW} / $(screen_of "${MSP:-0}" | tr '\n' ' ')"
fi
if [ -n "$MSP" ] && screen_has "$MSP" "__TAKO_EXIT="; then
  fail "MCP の split の画面に内部マーカーが出ている"
else
  pass "MCP の split の画面に内部マーカーが出ない"
fi
check_eq "MCP の split の終了コードも読める" "5" "$(field exit_code "$(printf '%s' "$(mcp_call tako_run_interactive_status "{\"pane\":${MSP:-0}}")" | python3 -c "$PY_FIELDS")")"
[ -n "$MSP" ] && "$TAKO_BIN" close --pane "$MSP" >/dev/null 2>&1
# 成功した split --command は従来どおり閉じる（包みが常時 hold にならない）
OKSP="$("$TAKO_BIN" split --pane "$ROOT" --down -- /bin/sh -c 'echo ok-split; exit 0' 2>/dev/null | tr -dc '0-9')"
if [ -n "$OKSP" ] && wait_until 20 pane_gone "$OKSP"; then
  pass "成功した split --command のペインは閉じる"
else
  fail "成功した split --command のペインが残っている（pane ${OKSP}）"
fi

echo
echo "== ② プログラムが印字した __TAKO_EXIT=7 で偽の確定をしない =="
RI_RAW="$("$TAKO_BIN" run-interactive "printf '__TAKO_EXIT=7\\n'; sleep 3" --pane "$ROOT" --auto-close never 2>&1)"
RI="$(printf '%s' "$RI_RAW" | python3 -c "$PY_FIELDS" | sed -n 's/^pane=//p' | head -1)"
[ -n "$RI" ] || { fail "run-interactive が pane を返さない: $RI_RAW"; RI=0; }
if wait_until 20 screen_has "$RI" "__TAKO_EXIT=7"; then
  DURING="$(status_of "$RI")"
  echo "  印字が画面に出た時点の status=$(field status "$DURING") exit_code=$(field exit_code "$DURING")"
  check_eq "印字が出た時点ではまだ running（印字で確定しない）" "running" "$(field status "$DURING")"
else
  fail "プログラムの印字が画面に出ない"
fi
if wait_until 20 is_exited "$RI"; then
  END="$(status_of "$RI")"
  check_eq "確定した終了コードはプログラムの本当の値 0" "0" "$(field exit_code "$END")"
  LIST_RUN="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"] == int(sys.argv[1]):
            r = p.get("run") or {}
            print("%s/%s" % (r.get("exit_code"), r.get("outcome")))' "$RI")"
  check_eq "tako list の run も 0 / success（バッジと同じ値）" "0/success" "$LIST_RUN"
else
  fail "run-interactive が終わらない"
fi
"$TAKO_BIN" close --pane "$RI" >/dev/null 2>&1
# tako run --wait も同じ（CLI の終了コードまで偽の 7 に引きずられない）
run_deadline 40 "$TMP/fake-wait.log" "$TAKO_BIN" run "$FIX/fake.sh" --pane "$ROOT" --new-pane --wait
RC=$?
FAKE_END="$(python3 -c "$PY_LAST" < "$TMP/fake-wait.log")"
echo "  tako run fake.sh --wait: rc=$RC exit_code=$(field exit_code "$FAKE_END")"
check_eq "tako run --wait は 0 で返る" "0" "$RC"
check_eq "tako run --wait の exit_code は 0" "0" "$(field exit_code "$FAKE_END")"
FP="$(pane_of_wait_log "$TMP/fake-wait.log")"
[ -n "$FP" ] && "$TAKO_BIN" close --pane "$FP" >/dev/null 2>&1

echo
echo "== ③ --wait の上限での打ち切りは 124（コマンドの失敗 1 と分ける）=="
run_deadline 20 "$TMP/cap-run.log" \
  env TAKO_RUN_WAIT_TIMEOUT_SECS=3 "$TAKO_BIN" run "$FIX/slow.sh" --pane "$ROOT" --new-pane --wait
RC=$?
CAP="$(python3 -c "$PY_LAST" < "$TMP/cap-run.log")"
echo "  tako run slow.sh --wait（上限 3 秒）: rc=$RC timed_out=$(field timed_out "$CAP")"
sed -n 's/^error: /  | error: /p' "$TMP/cap-run.log"
check_eq "tako run --wait の打ち切りは rc=124" "124" "$RC"
check_eq "応答は timed_out=True" "True" "$(field timed_out "$CAP")"
check_eq "応答は status=running（実行は止めない）" "running" "$(field status "$CAP")"
CP="$(pane_of_wait_log "$TMP/cap-run.log")"
[ -n "$CP" ] && "$TAKO_BIN" close --pane "$CP" >/dev/null 2>&1
run_deadline 20 "$TMP/cap-ri.log" \
  env TAKO_RUN_WAIT_TIMEOUT_SECS=3 "$TAKO_BIN" run-interactive "sleep 60" --pane "$ROOT" --wait
RC=$?
CAP="$(python3 -c "$PY_LAST" < "$TMP/cap-ri.log")"
echo "  tako run-interactive --wait（上限 3 秒）: rc=$RC timed_out=$(field timed_out "$CAP")"
check_eq "tako run-interactive --wait の打ち切りも rc=124" "124" "$RC"
check_eq "run-interactive の応答も timed_out=True" "True" "$(field timed_out "$CAP")"
CP="$(pane_of_wait_log "$TMP/cap-ri.log")"
[ -n "$CP" ] && "$TAKO_BIN" close --pane "$CP" >/dev/null 2>&1
run_deadline 40 "$TMP/ng1.log" "$TAKO_BIN" run "$FIX/ng1.sh" --pane "$ROOT" --new-pane --wait
RC=$?
check_eq "コマンドが自分で exit 1 したら rc=1 のまま" "1" "$RC"
check_eq "その exit_code は 1" "1" "$(field exit_code "$(python3 -c "$PY_LAST" < "$TMP/ng1.log")")"
NP="$(pane_of_wait_log "$TMP/ng1.log")"
[ -n "$NP" ] && "$TAKO_BIN" close --pane "$NP" >/dev/null 2>&1
run_deadline 40 "$TMP/ng124.log" "$TAKO_BIN" run "$FIX/ng124.sh" --pane "$ROOT" --new-pane --wait
RC=$?
check_eq "コマンドが自分で exit 124 しても rc=1（打ち切りと混ざらない）" "1" "$RC"
check_eq "その exit_code は 124" "124" "$(field exit_code "$(python3 -c "$PY_LAST" < "$TMP/ng124.log")")"
NP="$(pane_of_wait_log "$TMP/ng124.log")"
[ -n "$NP" ] && "$TAKO_BIN" close --pane "$NP" >/dev/null 2>&1

echo
echo "== ④ auto_close で閉じたペインのペインログは close:auto（手で閉じたものと見分ける）=="
AC_RAW="$("$TAKO_BIN" run "$FIX/ok.sh" --pane "$ROOT" --new-pane --auto-close success 2>&1)"
AC="$(printf '%s' "$AC_RAW" | python3 -c "$PY_FIELDS" | sed -n 's/^pane=//p' | head -1)"
if [ -n "$AC" ] && wait_until 20 pane_gone "$AC"; then
  R="$(close_reason_of "$AC")"
  echo "  GUI の終了検知で閉じた pane ${AC} のクローズマーカー: ${R}"
  check_eq "GUI の終了検知で閉じた実行ペイン（pane ${AC}）のマーカー" "close:auto" "$R"
else
  fail "--auto-close success の実行ペインが閉じない（pane ${AC}）"
fi
run_deadline 40 "$TMP/ac-wait.log" \
  "$TAKO_BIN" run "$FIX/ok.sh" --pane "$ROOT" --new-pane --auto-close success --wait
RC=$?
AW="$(pane_of_wait_log "$TMP/ac-wait.log")"
check_eq "--wait --auto-close success は 0 で返る" "0" "$RC"
if [ -n "$AW" ] && wait_until 20 pane_gone "$AW"; then
  R="$(close_reason_of "$AW")"
  echo "  --wait 経由で閉じた pane ${AW} のクローズマーカー: ${R}"
  check_eq "--wait 経由で閉じた実行ペイン（pane ${AW}）のマーカー" "close:auto" "$R"
else
  fail "--wait --auto-close success の実行ペインが閉じない（pane ${AW}）"
fi
MAN_RAW="$("$TAKO_BIN" run "$FIX/ng1.sh" --pane "$ROOT" --new-pane 2>&1)"
MAN="$(printf '%s' "$MAN_RAW" | python3 -c "$PY_FIELDS" | sed -n 's/^pane=//p' | head -1)"
wait_until 20 is_exited "$MAN" || fail "手で閉じる実行ペインが終わらない"
"$TAKO_BIN" close --pane "$MAN" >/dev/null 2>&1
R="$(close_reason_of "$MAN")"
echo "  手で閉じた pane ${MAN} のクローズマーカー: ${R}"
check_eq "手で閉じた実行ペイン（pane ${MAN}）は close:dispatch(cli)" "close:dispatch(cli)" "$R"

echo
echo "== ⑤ エッジ: 大量出力 / マーカーの大量印字 / 即座に終わる =="
run_deadline 60 "$TMP/big.log" "$TAKO_BIN" run "$FIX/big.sh" --pane "$ROOT" --new-pane --wait
RC=$?
BIG_END="$(python3 -c "$PY_LAST" < "$TMP/big.log")"
check_eq "20 万行を出して exit 3: rc=1" "1" "$RC"
check_eq "20 万行を出して exit 3: exit_code=3" "3" "$(field exit_code "$BIG_END")"
BP="$(pane_of_wait_log "$TMP/big.log")"
if [ -n "$BP" ] && has_hint "$BP" 3; then pass "大量出力のあとも案内が出る"; else fail "大量出力のあとに案内が無い"; fi
[ -n "$BP" ] && "$TAKO_BIN" close --pane "$BP" >/dev/null 2>&1
run_deadline 60 "$TMP/spam.log" "$TAKO_BIN" run "$FIX/spam.sh" --pane "$ROOT" --new-pane --wait
RC=$?
SPAM_END="$(python3 -c "$PY_LAST" < "$TMP/spam.log")"
echo "  __TAKO_EXIT=0..2999 を印字して 0 で終わる: rc=$RC exit_code=$(field exit_code "$SPAM_END")"
check_eq "マーカーを 3000 行印字しても exit_code=0" "0" "$(field exit_code "$SPAM_END")"
check_eq "その rc は 0" "0" "$RC"
SPP="$(pane_of_wait_log "$TMP/spam.log")"
[ -n "$SPP" ] && "$TAKO_BIN" close --pane "$SPP" >/dev/null 2>&1
run_deadline 30 "$TMP/instant.log" "$TAKO_BIN" run "$FIX/instant.sh" --pane "$ROOT" --new-pane --wait
RC=$?
check_eq "即座に終わる（true）: rc=0" "0" "$RC"
check_eq "即座に終わる（true）: exit_code=0" "0" "$(field exit_code "$(python3 -c "$PY_LAST" < "$TMP/instant.log")")"
IP="$(pane_of_wait_log "$TMP/instant.log")"
[ -n "$IP" ] && "$TAKO_BIN" close --pane "$IP" >/dev/null 2>&1
QSP="$("$TAKO_BIN" split --pane "$ROOT" --down -- /bin/sh -c 'exit 5' 2>/dev/null | tr -dc '0-9')"
if [ -n "$QSP" ] && wait_until 20 is_exited "$QSP"; then
  check_eq "即座に失敗する split --command（exit 5）も読める" "5" "$(field exit_code "$(status_of "$QSP")")"
else
  fail "即座に失敗する split --command の終了コードが読めない"
fi
[ -n "$QSP" ] && "$TAKO_BIN" close --pane "$QSP" >/dev/null 2>&1

echo
echo "== ⑤ エッジ: 側路を用意できない（退避路 = 画面のマーカーで伝わる）=="
# 置き場を「ファイル」にしてディレクトリを作れなくする（prepare が None を返す）
rm -rf "$TAKO_DATA_DIR/run-exit"
: > "$TAKO_DATA_DIR/run-exit"
FSP="$("$TAKO_BIN" split --pane "$ROOT" --down -- /bin/sh -c 'exit 6' 2>/dev/null | tr -dc '0-9')"
if [ -n "$FSP" ] && wait_until 20 is_exited "$FSP"; then
  check_eq "側路が無い split --command の終了コードは画面のマーカーから読める" "6" "$(field exit_code "$(status_of "$FSP")")"
  if screen_has "$FSP" "__TAKO_EXIT=6"; then pass "退避路では画面にマーカーが出る（読む口がある）"; else fail "退避路なのに画面にマーカーが無い"; fi
else
  fail "側路が無い split --command の終了コードが読めない"
fi
[ -n "$FSP" ] && "$TAKO_BIN" close --pane "$FSP" >/dev/null 2>&1
run_deadline 40 "$TMP/fallback.log" "$TAKO_BIN" run "$FIX/ng1.sh" --pane "$ROOT" --new-pane --wait
RC=$?
check_eq "側路が無くても tako run --wait は止まらず rc=1" "1" "$RC"
check_eq "その exit_code は 1" "1" "$(field exit_code "$(python3 -c "$PY_LAST" < "$TMP/fallback.log")")"
FBP="$(pane_of_wait_log "$TMP/fallback.log")"
[ -n "$FBP" ] && "$TAKO_BIN" close --pane "$FBP" >/dev/null 2>&1
rm -f "$TAKO_DATA_DIR/run-exit"

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" = 0 ]
