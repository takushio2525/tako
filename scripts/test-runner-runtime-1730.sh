#!/usr/bin/env bash
# test-runner-runtime-1730.sh — Code Runner の実行環境の自動検出の実経路テスト（#1730 / FR-3.18.3）
#
# 隔離した data / tmux / HOME で**実 tako-app** を立て、
#
#   ① `python3 -m venv .venv` したプロジェクトで `tako run a.py`（`print(sys.prefix)`）が
#      設定なしで `.venv` の python で走る（出力の prefix が `.venv` を指す）
#   ② venv の無いプロジェクトはシステムの python（prefix が材料の外）
#   ③ `tako run a.py --list` と MCP `tako_run_resolve` の `runtimes` が字面まで一致する
#   ④ 道具: uv のプロジェクトは `uv run` で走る（uv が入っていれば）/ poetry の印だけあって
#      道具が無ければ自動では選ばずシステムへ落ち、理由が warnings に載る
#   ⑤ エッジ: 壊れた `.venv`（python が無い）/ プロジェクト外の単独 `.py` /
#      パスに空白と日本語 / Tier P のコマンドが固まる（上限で打ち切って知らせる）
#   ⑥ Tier F の所要（`probe.tier_f_ms`。目標 50 ms 以内。assert にはしない）
#   ⑦ A/B: `TAKO_1730_LEGACY=1` で立て直すと ① の判定が FAILED になる（PATH の python3 へ戻る）
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-runner-runtime-1730.sh
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

command -v python3 >/dev/null 2>&1 || { echo "python3 が無いので走らせられない（未実測）"; exit 1; }

TMP="$(mktemp -d /tmp/tako-1730-XXXXXX)"
# 比較は実体パスで行う（macOS の /tmp は /private/tmp へのリンク。dispatch は canonicalize する）
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1730-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# 道具の有無は HOME を差し替える前の実環境で見る（uv の置き場は HOME の下のこともある）
UV_BIN="$(command -v uv 2>/dev/null || true)"
SYS_PY="$(command -v python3)"

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
# uv はキャッシュと python を HOME の下へ作る（隔離 HOME の中）。python は落としてこない
export UV_PYTHON_DOWNLOADS=never
export UV_NO_PROGRESS=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 固まる道具のスタブ（pipenv はこの機械に無い前提の名前。直接の子を眠らせるので打ち切りで止まる）
STUBS="$TMP/stubs"
mkdir -p "$STUBS"
printf '#!/bin/sh\nexec sleep 30\n' > "$STUBS/pipenv"
chmod +x "$STUBS/pipenv"
export PATH="$STUBS:$PATH"

# --- 材料 ---------------------------------------------------------------------
# 探索は `.git` の段で止まるので、材料の根に置いて /tmp の置き忘れを拾わないようにする
FIX="$TMP/fixtures"
mkdir -p "$FIX/.git"
# 画面にも出すが、判定は隣に書き出したファイルで読む（実行ペインを何枚も割ると後の方は
# 小さくなり、画面の行が流れて読めない）
PRINT_PREFIX='import sys
print("PREFIX_1730=" + sys.prefix)
with open(__file__ + ".prefix", "w", encoding="utf-8") as f:
    f.write(sys.prefix)
'
mk_script() { # mk_script <dir> → <dir>/a.py
  mkdir -p "$1"
  printf '%s' "$PRINT_PREFIX" > "$1/a.py"
}
mk_script "$FIX/venvproj"
"$SYS_PY" -m venv "$FIX/venvproj/.venv" || { echo "venv を作れない"; exit 1; }
mk_script "$FIX/novenv"
mk_script "$FIX/broken"
"$SYS_PY" -m venv "$FIX/broken/.venv" >/dev/null 2>&1
rm -f "$FIX/broken/.venv/bin/python"
JP="$FIX/日本語 プロジェクト"
mk_script "$JP"
"$SYS_PY" -m venv "$JP/.venv" || { echo "空白と日本語のパスに venv を作れない"; exit 1; }
mk_script "$FIX/poetryproj"
: > "$FIX/poetryproj/poetry.lock"
mk_script "$FIX/pipenvproj"
: > "$FIX/pipenvproj/Pipfile"
mkdir -p "$HOME/Downloads"
printf '%s' "$PRINT_PREFIX" > "$HOME/Downloads/solo.py"
# Tier F のいちばん重い形: 各段に子ディレクトリが 300 ずつ（1 段で見る子の上限 256 を超える）
python3 - "$FIX/wide" <<'PYW'
import pathlib, sys
base = pathlib.Path(sys.argv[1])
cur = base
for level in ("l1", "l2", "l3"):
    for i in range(300):
        (cur / ("d%03d" % i)).mkdir(parents=True, exist_ok=True)
    cur = cur / level
    cur.mkdir(parents=True, exist_ok=True)
(cur / "a.py").write_text("print(1)\n", encoding="utf-8")
PYW
if [ -n "$UV_BIN" ]; then
  mk_script "$FIX/uvproj"
  printf '[project]\nname = "uvproj1730"\nversion = "0.1.0"\nrequires-python = ">=3.8"\ndependencies = []\n' \
    > "$FIX/uvproj/pyproject.toml"
  (cd "$FIX/uvproj" && "$UV_BIN" lock >/dev/null 2>&1) || echo "  (uv lock に失敗。uv の項目は未実測になる)"
fi

PY_LIST='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
rt = d.get("runtime") or {}
print("manager=%s" % rt.get("manager"))
print("program=%s" % " ".join(rt.get("program") or []))
print("path=%s" % rt.get("path"))
print("command=%s" % (d.get("profiles") or [{}])[0].get("command"))
print("warnings=%s" % " | ".join(d.get("warnings") or []))
p = d.get("probe") or {}
print("tier_f_ms=%s" % p.get("tier_f_ms"))
print("tier_p_ms=%s" % p.get("tier_p_ms"))
print("timeouts=%s" % " ".join(t.get("label", "") for t in (p.get("timeouts") or [])))
print("runtimes=%s" % json.dumps(d.get("runtimes"), sort_keys=True, ensure_ascii=False))'
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

list() { # list <file> [--refresh] → 一覧の要約（1 行 1 項目）
  "$TAKO_BIN" run "$@" --list --pane "$ROOT" 2>&1 | python3 -c "$PY_LIST"
}

# run_prefix <file> → 実行ペインで走った python の sys.prefix（書き出されなければ __NO_OUTPUT__）。
# 終了の状態は stderr へ `status=<状態> <終了コード>` で出す
run_prefix() {
  local out pane st status
  rm -f "$1.prefix"
  out="$("$TAKO_BIN" run "$1" --pane "$ROOT" --auto-close success 2>&1)"
  pane="$(printf '%s' "$out" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
  [ -n "$pane" ] || { echo "__NO_PANE__ $(oneline "$out")"; return; }
  status=""
  for _ in $(seq 1 60); do
    st="$("$TAKO_BIN" run-interactive-status "$pane" 2>&1)"
    status="$(printf '%s' "$st" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin); print("%s %s" % (d.get("status"), d.get("exit_code")))
except Exception:
    pass' 2>/dev/null)"
    case "$status" in exited*) break ;; esac
    sleep 1
  done
  if [ -f "$1.prefix" ]; then cat "$1.prefix"; echo; else echo "__NO_OUTPUT__"; fi
  echo "status=$status" >&2
}

# ① の判定（A/B で同じ関数を使う）: 出力の prefix が venv を指すか
judge_venv() { # judge_venv <prefix> → OK / FAILED
  if [ "$1" = "$FIX/venvproj/.venv" ]; then echo OK; else echo FAILED; fi
}

echo "== 隔離 GUI を起こす =="
# Tier P の上限は GUI の中で読む（dispatch が GUI で走る）ので、起動の env で縮める
# （固まるスタブを 3 秒で打ち切る。ログインシェルの探索は 0.35 秒前後なので足りる）
start_gui "$TMP/app.log" TAKO_RUN_PROBE_TIMEOUT_SECS=3 || exit 1

echo
echo "== ① .venv のあるプロジェクトで tako run a.py =="
L1="$(list "$FIX/venvproj/a.py")"
echo "  list: manager=$(field manager "$L1") command=$(field command "$L1")"
echo "  初回の所要: tier_f_ms=$(field tier_f_ms "$L1") tier_p_ms=$(field tier_p_ms "$L1")"
check_eq "一覧の runtime は venv" "venv" "$(field manager "$L1")"
P1="$(run_prefix "$FIX/venvproj/a.py" 2>"$TMP/st1")"
echo "  出力の prefix: ${P1}（$(cat "$TMP/st1")）"
check_eq "① 出力の prefix が .venv を指す" "OK" "$(judge_venv "$P1")"
check_eq "① 終了コード 0" "status=exited 0" "$(cat "$TMP/st1")"

echo
echo "== ② venv の無いプロジェクトはシステムの python =="
L2="$(list "$FIX/novenv/a.py")"
check_eq "一覧の runtime は system" "system" "$(field manager "$L2")"
check_eq "コマンドは今と同じ（python3 a.py）" "python3 a.py" "$(field command "$L2")"
P2="$(run_prefix "$FIX/novenv/a.py" 2>"$TMP/st2")"
echo "  出力の prefix: ${P2}（$(cat "$TMP/st2")）"
case "$P2" in
  "$FIX"*|__NO_*|"") fail "② prefix が材料の中か取れない: $P2" ;;
  *) pass "② prefix は材料の外（システムの python）" ;;
esac

echo
echo "== ③ CLI --list と MCP tako_run_resolve の runtimes が字面まで一致する =="
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1730","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run_resolve","arguments":{"path":"$FIX/venvproj/a.py","pane":$ROOT}}}
JSONL
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
CLI_TEXT="$("$TAKO_BIN" run "$FIX/venvproj/a.py" --list --pane "$ROOT" 2>&1)"
# 所要（probe の ms）は回ごとに違うので、runtimes と、probe を落とした全体の 2 通りで比べる
NORM_RT='import json,sys; print(json.dumps(json.loads(sys.stdin.read())["runtimes"], sort_keys=True, ensure_ascii=False))'
NORM_ALL='import json,sys; d = json.loads(sys.stdin.read()); d.pop("probe", None); print(json.dumps(d, sort_keys=True, ensure_ascii=False))'
CLI_RT="$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM_RT" 2>/dev/null)"
MCP_RT="$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM_RT" 2>/dev/null)"
echo "  CLI runtimes: $CLI_RT"
echo "  MCP runtimes: $MCP_RT"
[ -n "$MCP_RT" ] || {
  echo "  (MCP の生応答) $MCP_TEXT"
  echo "  (MCP の stderr) $(tr '\n' ' ' < "$TMP/mcp-err.log")"
}
[ -n "$CLI_RT" ] && [ "$CLI_RT" != "[]" ] || fail "CLI の runtimes が空か JSON でない: $(oneline "$CLI_TEXT" | head -c 300)"
check_eq "③ runtimes が字面まで一致する" "$CLI_RT" "$MCP_RT"
check_eq "③ probe 以外の応答全体も一致する" \
  "$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM_ALL" 2>/dev/null)" \
  "$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM_ALL" 2>/dev/null)"

echo
echo "== ④ 道具: uv / poetry =="
if [ -n "$UV_BIN" ] && [ -f "$FIX/uvproj/uv.lock" ]; then
  L4="$(list "$FIX/uvproj/a.py")"
  echo "  list: manager=$(field manager "$L4") program=$(field program "$L4")"
  check_eq "uv のプロジェクトは uv（uv run で包む）" "uv" "$(field manager "$L4")"
  P4="$(run_prefix "$FIX/uvproj/a.py" 2>"$TMP/st4")"
  echo "  出力の prefix: ${P4}（$(cat "$TMP/st4")）"
  check_eq "uv run がプロジェクトの .venv で走る" "$FIX/uvproj/.venv" "$P4"
else
  echo "  (uv が無い / uv lock に失敗したので uv の項目は未実測)"
fi
L5="$(list "$FIX/poetryproj/a.py")"
echo "  poetry の印だけ: manager=$(field manager "$L5") warnings=$(field warnings "$L5")"
check_eq "poetry が無ければ自動では選ばずシステムへ" "system" "$(field manager "$L5")"
case "$(field warnings "$L5")" in
  *poetry*) pass "poetry が見つからない理由が warnings に載る" ;;
  *) fail "warnings に poetry の理由が無い" ;;
esac

echo
echo "== ⑤ エッジ =="
LB="$(list "$FIX/broken/a.py")"
echo "  壊れた venv: manager=$(field manager "$LB") warnings=$(field warnings "$LB")"
check_eq "壊れた .venv は自動で選ばない" "system" "$(field manager "$LB")"
case "$(field warnings "$LB")" in
  *.venv*) pass "壊れた理由が warnings に載る" ;;
  *) fail "warnings に壊れた理由が無い" ;;
esac
PB="$(run_prefix "$FIX/broken/a.py" 2>"$TMP/stb")"
check_eq "壊れた .venv のプロジェクトもシステムの python で走る" "status=exited 0" "$(cat "$TMP/stb")"

LS="$(list "$HOME/Downloads/solo.py")"
check_eq "プロジェクト外の単独 .py はシステム" "system" "$(field manager "$LS")"
PS="$(run_prefix "$HOME/Downloads/solo.py" 2>"$TMP/sts")"
check_eq "単独 .py も走る" "status=exited 0" "$(cat "$TMP/sts")"

LJ="$(list "$JP/a.py")"
check_eq "空白と日本語のパスでも venv を選ぶ" "venv" "$(field manager "$LJ")"
PJ="$(run_prefix "$JP/a.py" 2>"$TMP/stj")"
echo "  出力の prefix: ${PJ}（$(cat "$TMP/stj")）"
check_eq "空白と日本語のパスの venv で走る" "$JP/.venv" "$PJ"

# 固まる道具: 一覧（Tier P）が上限で打ち切って返り、知らせを載せる
T0=$(date +%s)
LH="$(list "$FIX/pipenvproj/a.py")"
T1=$(date +%s)
echo "  固まる pipenv: $((T1 - T0)) 秒で返った / timeouts=$(field timeouts "$LH")"
case "$(field timeouts "$LH")" in
  *pipenv*) pass "固まった問い合わせを打ち切って timeouts に載せる" ;;
  *) fail "timeouts に打ち切りが無い: $(oneline "$LH")" ;;
esac
case "$(field warnings "$LH")" in
  *"確認できません"*) pass "打ち切りの知らせが warnings に載る" ;;
  *) fail "warnings に打ち切りの知らせが無い" ;;
esac
if [ $((T1 - T0)) -lt 30 ]; then pass "スタブの 30 秒を待たずに返る"; else fail "上限が効いていない（$((T1 - T0)) 秒）"; fi

echo
echo "== ⑥ Tier F の所要（目標 50 ms 以内。assert にはしない） =="
for f in "$FIX/venvproj/a.py" "$FIX/novenv/a.py" "$JP/a.py" "$HOME/Downloads/solo.py" \
  "$FIX/wide/l1/l2/l3/a.py"; do
  for i in 1 2 3; do
    L="$(list "$f")"
    printf '  %s #%s tier_f_ms=%s tier_p_ms=%s\n' "${f#"$TMP"/}" "$i" "$(field tier_f_ms "$L")" "$(field tier_p_ms "$L")"
  done
done

echo
echo "== ⑦ A/B: TAKO_1730_LEGACY=1 で立て直す =="
stop_isolated_gui "$APP_PID"
APP_PID=""
start_gui "$TMP/app-legacy.log" TAKO_1730_LEGACY=1 || exit 1
LL="$(list "$FIX/venvproj/a.py")"
check_eq "LEGACY では runtime が null" "None" "$(field manager "$LL")"
PL="$(run_prefix "$FIX/venvproj/a.py" 2>"$TMP/stl")"
echo "  LEGACY の出力の prefix: ${PL}（$(cat "$TMP/stl")）"
check_eq "LEGACY では ① の判定が FAILED（PATH の python3 へ戻る）" "FAILED" "$(judge_venv "$PL")"

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
