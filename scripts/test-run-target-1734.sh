#!/usr/bin/env bash
# tako:run bash scripts/test-run-target-1734.sh
# test-run-target-1734.sh — 実行先（同じペインで再実行 / 新しいペイン）を実行設定として覚える
# 実経路テスト（#1734 / FR-3.18.9。エピック #1726 の S6）
#
# 隔離した data / HOME で**実 tako-app** を立て、
#
#   ① `tako run-config a.sh --target new` → 保存先の既定はファイル・`run-configs.json` に `"target": "new"`
#   ② `tako run a.sh` を 2 回 → タブのペインが 2 枚増える（応答の `target` は new・`reused_from` は null）
#   ③ 一回限りの指定が保存値より勝つ: MCP `tako_run { new_pane: false }` は保存の new でも使い回す（増えない）/
#      `--target reuse` を保存して `tako run` 2 回は増えない / `tako run --new-pane` は保存の reuse でも 1 枚増える
#   ④ CLI と MCP が同じ 1 本: `tako run-config a.sh` と MCP `tako_run_defaults {path}` の応答がバイト一致・
#      MCP の `target` で保存した値が CLI から読める
#   ⑤ 境界: 正本に無い語（CLI `--target New` / MCP `target: "tab"`）は保存しない（非 0・ファイルは変わらない）/
#      `--reset target` で既定（出典 default）へ戻る
#   ⑥ 旧い `run-configs.json`（`target` の欄の無い #1733 までの形）をそのまま読む（退避しない・他の項目は効く・
#      実行先は既定 = 使い回す）
#   ⑦ A/B: `TAKO_1734_LEGACY=1` で立て直すと ② の判定が FAILED（保存の new が効かず 1 枚しか増えない）
#   ⑧ 画面: visual-test `run-target-1734`（ドロップダウンの実行先の行を実マウスで切り替え、実マウスの再生ボタン
#      2 回で 2 枚 / 増えない）。旧（`TAKO_1734_LEGACY=1`）は ③ の「2 枚増える」で名指しの FAILED
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下・器は使わない（TAKO_PERSIST=0）・
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-run-target-1734.sh
#         TAKO_1734_KEEP_DUMP=<dir> bash scripts/test-run-target-1734.sh   # visual-test の画像を残す（new / legacy）
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
check_rc_nonzero() { # check_rc_nonzero <名前> <終了コード> <本文>
  if [ "$2" -ne 0 ]; then pass "$1"; else fail "$1（0 で終わった: $(printf '%s' "$3" | tr '\n' ' ' | head -c 300)）"; fi
}

TMP="$(mktemp -d /tmp/tako-1734-XXXXXX)"
# 比較は実体パスで行う（macOS の /tmp は /private/tmp へのリンク。dispatch は canonicalize する）
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）。
# ⑧ の visual-test が要るので visual-test 版を作る（ふつうの起動はこの版でも同じに動く）
isolated_gui_bins visual-test || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERF_LOG="$TMP/perf.log"
# 実行ペインは器を使わない（数えるのはペインの枚数だけ。隔離 tmux の後片付けを要らなくする）
export TAKO_PERSIST=0
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/zdot"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
# 画像にユーザー名・ホスト名が写らないよう固定のプロンプト（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
STORE="$TAKO_DATA_DIR/run-configs.json"

# --- 材料 ---------------------------------------------------------------------
# 探索は `.git` の段で止まるので、材料の根に置いて /tmp の置き忘れを拾わないようにする
FIX="$TMP/fixtures"
PROJ="$FIX/proj"
mkdir -p "$FIX/.git" "$PROJ"
printf '# tako:run: echo tako-1734-a\n' > "$PROJ/a.sh"
printf '# tako:run: echo tako-1734-b\n' > "$PROJ/b.sh"

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
stored_target() { # stored_target <files|projects> <鍵> → 保存した target（無ければ None）
  python3 -c 'import json,sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    print("__NO_STORE__"); sys.exit(0)
print((d.get(sys.argv[2]) or {}).get(sys.argv[3], {}).get("target"))' "$STORE" "$1" "$2"
}

start_gui() { # start_gui <ログ> [VAR=VAL…]
  local log="$1"; shift
  # 自動命名を止める（隔離 GUI でもタブ名のために claude を呼ぶ。HOME は差し替えてあるが念のため）
  launch_isolated_gui "$log" ZDOTDIR="$TMP/zdot" TAKO_AUTO_RENAME=0 ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || return 1
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; return 1; }
  echo "  pid=$APP_PID root pane=$ROOT"
}
stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# いま開いているタブ（先頭のタブ）のペインの枚数
pane_count() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(len(d["tabs"][0]["panes"]))' 2>/dev/null || echo "__NO_LIST__"
}

# run_twice <file> → 2 回走らせ、増えた枚数を ADDED・応答を RUN1 / RUN2 へ置く
# （`$(run_twice …)` で呼ぶとサブシェルになり RUN1 / RUN2 が呼び手へ戻らないので、値はグローバルで返す）
run_twice() {
  local before after
  before="$(pane_count)"
  RUN1="$("$TAKO_BIN" run "$1" --pane "$ROOT" 2>&1)"
  RUN2="$("$TAKO_BIN" run "$1" --pane "$ROOT" 2>&1)"
  after="$(pane_count)"
  ADDED=$((after - before))
}

# ② と ⑦ の判定（A/B で同じ関数を使う）: target=new を保存して 2 回で 2 枚増えたか
judge_two_panes() { # judge_two_panes <増えた枚数> → OK / FAILED
  if [ "$1" = 2 ]; then echo OK; else echo FAILED; fi
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log" || exit 1
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp_call() { # mcp_call <ツール名> <arguments の JSON> → 本文（text）か __RPC_ERROR__ / __TOOL_ERROR__
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1734","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
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
        result = msg.get("result") or {}
        content = result.get("content") or []
        text = content[0].get("text") if content else "__NO_CONTENT__"
        print(("__TOOL_ERROR__ " + text) if result.get("isError") else text)
        sys.exit(0)
print("__NO_RESPONSE__")' "$TMP/mcp-out.jsonl"
}

echo
echo "== ① --target new を覚える（既定の保存先はファイル） =="
R1="$(cd "$PROJ" && "$TAKO_BIN" run-config a.sh --target new 2>&1)"
check_eq "① 保存先の既定はファイル" '[{"action": "set", "field": "target", "scope": "file"}]' "$(field updated "$R1")"
check_eq "① config.target は new（出典 file）" '{"source": "file", "value": "new"}' "$(field config.target "$R1")"
check_eq "① run-configs.json の files に target new" "new" "$(stored_target files "$PROJ/a.sh")"

echo
echo "== ② tako run を 2 回 → ペインが 2 枚増える =="
run_twice "$PROJ/a.sh"
echo "  増えた枚数: $ADDED / 2 回目の応答: $(oneline "$RUN2" | head -c 300)"
check_eq "② target=new で 2 回 = 2 枚増える" "OK" "$(judge_two_panes "$ADDED")"
check_eq "② 応答の target は new" "new" "$(field target "$RUN2")"
check_eq "② 2 回目も分割した（reused_from は null）" "None" "$(field reused_from "$RUN2")"
check_eq "② 出典は file" "file" "$(field config_sources.target "$RUN2")"

echo
echo "== ③ 一回限りの指定が保存値より勝つ =="
B3="$(pane_count)"
M3="$(mcp_call tako_run "{\"path\":\"$PROJ/a.sh\",\"pane\":$ROOT,\"new_pane\":false}")"
A3="$(pane_count)"
check_eq "③ MCP new_pane:false は保存の new でも使い回す（増えない）" "0" "$((A3 - B3))"
check_eq "③ その応答の target は reuse" "reuse" "$(field target "$M3")"
R3="$("$TAKO_BIN" run-config "$PROJ/a.sh" --target reuse 2>&1)"
check_eq "③ --target reuse を保存（今の保存先 = ファイル）" "reuse" "$(stored_target files "$PROJ/a.sh")"
run_twice "$PROJ/a.sh"
check_eq "③ target=reuse で 2 回 = 増えない" "0" "$ADDED"
B3b="$(pane_count)"
R3b="$("$TAKO_BIN" run "$PROJ/a.sh" --pane "$ROOT" --new-pane 2>&1)"
A3b="$(pane_count)"
check_eq "③ --new-pane は保存の reuse でも 1 枚増える" "1" "$((A3b - B3b))"
check_eq "③ その応答の target は new" "new" "$(field target "$R3b")"

echo
echo "== ④ CLI と MCP tako_run_defaults が同じ 1 本を通る =="
COMPACT='import json,sys; print(json.dumps(json.loads(sys.stdin.read()), ensure_ascii=False, separators=(",", ":")))'
MCP_TEXT="$(mcp_call tako_run_defaults "{\"path\":\"$PROJ/a.sh\"}")"
CLI_TEXT="$("$TAKO_BIN" run-config "$PROJ/a.sh" 2>&1)"
printf '%s' "$CLI_TEXT" | python3 -c "$COMPACT" > "$TMP/cli.txt" 2>/dev/null
printf '%s' "$MCP_TEXT" | python3 -c "$COMPACT" > "$TMP/mcp.txt" 2>/dev/null
if [ -s "$TMP/cli.txt" ] && cmp -s "$TMP/cli.txt" "$TMP/mcp.txt"; then
  pass "④ 読むだけの応答がキーの並びまでバイト一致（$(wc -c < "$TMP/cli.txt" | tr -d ' ') バイト・config.target = $(field config.target "$CLI_TEXT")）"
else
  fail "④ CLI と MCP の応答がずれる: $(oneline "$MCP_TEXT" | head -c 300)"
fi
M4="$(mcp_call tako_run_defaults "{\"path\":\"$PROJ/a.sh\",\"target\":\"new\",\"scope\":\"project\"}")"
check_eq "④ MCP の target で保存（scope project）" '[{"action": "set", "field": "target", "scope": "project"}]' "$(field updated "$M4")"
C4="$("$TAKO_BIN" run-config "$PROJ/a.sh" 2>&1)"
check_eq "④ CLI から見てもファイルの reuse がプロジェクトの new より勝つ" '{"source": "file", "value": "reuse"}' "$(field config.target "$C4")"
check_eq "④ CLI から見える保存値（projects）" "new" "$(field saved.project.target "$C4")"

echo
echo "== ⑤ 境界: 正本に無い語は保存しない・--reset target で既定へ =="
BEFORE_STORE="$(cat "$STORE")"
E5a="$("$TAKO_BIN" run-config "$PROJ/a.sh" --target New 2>&1)"; RC=$?
check_rc_nonzero "⑤ CLI --target New は非 0" "$RC" "$E5a"
E5b="$(mcp_call tako_run_defaults "{\"path\":\"$PROJ/a.sh\",\"target\":\"tab\"}")"
case "$E5b" in
  __RPC_ERROR__*|__TOOL_ERROR__*) pass "⑤ MCP target:\"tab\" はエラー（$(printf '%s' "$E5b" | head -c 120)）" ;;
  *) fail "⑤ MCP target:\"tab\" が通った: $(oneline "$E5b" | head -c 300)" ;;
esac
check_eq "⑤ run-configs.json は変わらない" "$BEFORE_STORE" "$(cat "$STORE")"
R5="$("$TAKO_BIN" run-config "$PROJ/a.sh" --reset target 2>&1)"
check_eq "⑤ --reset target で既定（出典 default）" '{"source": "default", "value": null}' "$(field config.target "$R5")"
check_eq "⑤ files にも projects にも残らない" "None|None" \
  "$(stored_target files "$PROJ/a.sh")|$(stored_target projects "$FIX")"

echo
echo "== ⑥ target の欄の無い旧い run-configs.json をそのまま読む =="
stop_gui
python3 - "$STORE" "$PROJ/b.sh" <<'PY'
import json, sys
store, b = sys.argv[1], sys.argv[2]
# #1733 までの版が書いた形（target の欄が無い）
old = {"version": 1, "files": {b: {"args": "--old", "updated_at": "2026-10-16T00:00:00Z"}}}
with open(store, "w") as f:
    f.write(json.dumps(old, ensure_ascii=False, indent=2) + "\n")
PY
start_gui "$TMP/app-old.log" || exit 1
R6="$("$TAKO_BIN" run-config "$PROJ/b.sh" 2>&1)"
check_eq "⑥ 旧い形の引数が効く（出典 file）" '{"source": "file", "value": "--old"}' "$(field config.args "$R6")"
check_eq "⑥ 実行先は既定" '{"source": "default", "value": null}' "$(field config.target "$R6")"
UNREADABLE="$(ls "$TAKO_DATA_DIR" | grep -c unreadable)"
check_eq "⑥ 読めないとして退避していない（.unreadable.bak が 0 件）" "0" "$UNREADABLE"
run_twice "$PROJ/b.sh"
check_eq "⑥ 実行先が既定なので 2 回で 1 枚（使い回す）" "1" "$ADDED"

echo
echo "== ⑦ A/B: TAKO_1734_LEGACY=1 で立て直す =="
"$TAKO_BIN" run-config "$PROJ/a.sh" --target new >/dev/null 2>&1
run_twice "$PROJ/a.sh"
check_eq "⑦ 新しい挙動では 2 枚増える" "OK" "$(judge_two_panes "$ADDED")"
stop_gui
start_gui "$TMP/app-legacy.log" TAKO_1734_LEGACY=1 || exit 1
check_eq "⑦ LEGACY でも保存値は読める（保存の口は同じ）" "new" "$(stored_target files "$PROJ/a.sh")"
run_twice "$PROJ/a.sh"
echo "  LEGACY の増えた枚数: $ADDED"
check_eq "⑦ LEGACY では ② の判定が FAILED（保存の new が効かない）" "FAILED" "$(judge_two_panes "$ADDED")"
stop_gui

echo
echo "== ⑧ 画面: visual-test run-target-1734（新 / 旧） =="
run_visual() { # run_visual <名前 new / legacy> [VAR=VAL…]
  local name="$1"; shift
  local dump="$TMP/dump-$name"
  mkdir -p "$TMP/data-$name" "$TMP/t-$name"
  launch_isolated_gui "$TMP/vt-$name.log" ZDOTDIR="$TMP/zdot" TAKO_AUTO_RENAME=0 TAKO_DATA_DIR="$TMP/data-$name" \
    TMPDIR="$TMP/t-$name" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=run-target-1734 \
    TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  local vpid="$ISOLATED_GUI_PID"
  local i
  for i in $(seq 1 240); do
    kill -0 "$vpid" 2>/dev/null || break
    sleep 1
  done
  stop_isolated_gui "$vpid"
  ISOLATED_GUI_PID=""
  grep -E "TAKO_VISUAL_1734|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK|TAKO_VISUAL_DUMP_FILE|panicked" \
    "$TMP/vt-$name.log" | sed 's/^/    /'
  if [ -n "${TAKO_1734_KEEP_DUMP:-}" ] && [ -d "$dump" ]; then
    mkdir -p "$TAKO_1734_KEEP_DUMP/$name"
    command cp -f "$dump"/*.png "$TAKO_1734_KEEP_DUMP/$name"/ 2>/dev/null
    echo "    画像: $TAKO_1734_KEEP_DUMP/$name"
  fi
}
run_visual new
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-new.log" && ! grep -q "TAKO_APP_SELF_TEST_FAILED" "$TMP/vt-new.log"; then
  pass "⑧ 新: 実行先の行を実マウスで切り替え、再生ボタン 2 回で 2 枚 / 増えない・出典タグと × が効く"
else
  fail "⑧ 新: visual-test が通らない"
fi
if grep -q "run-target-1734-two.png" "$TMP/vt-new.log"; then
  pass "⑧ 新: 画像を書き出した"
else
  fail "⑧ 新: 画像が書き出されていない（TAKO_VISUAL_DUMP_DIR）"
fi
run_visual legacy TAKO_1734_LEGACY=1
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test run-target-1734 ③: target=new で再生ボタン 2 回 = ペインが 2 枚増える" "$TMP/vt-legacy.log"; then
  pass "⑧ 旧: 再生ボタン 2 回で 2 枚（③）で名指しの FAILED"
else
  fail "⑧ 旧: ③ で落ちていない（A/B の検出力が無い）"
fi

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
