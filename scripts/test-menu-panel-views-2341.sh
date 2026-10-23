#!/bin/bash
# test-menu-panel-views-2341.sh — メニューの「表示 > パネル」から tasks / usage のビューを
# 開ける実経路テスト（Issue #2341）
#
# 何を確かめるか（隔離 GUI に `tako menu list` / `tako menu invoke` / MCP `tako_menu` を撃つ）:
#   ① 日本語: 「表示/パネル」の並びが fleet / orch / git / tasks / usage の 5 項目
#      （ラベルは「X ビュー」、アクションは tako::Show<X>Panel）
#   ② パネルを閉じた状態から「表示/パネル/tasks ビュー」を invoke → パネルが開いて tasks
#   ③ 同じ項目をもう一度 invoke → パネルが閉じる（fleet / orch / git・パレットと同じトグル）
#   ④ fleet を開いた状態から項目名だけ「usage ビュー」を invoke → usage へ切り替わり、
#      `tako panel` の `usage`（開いている段）が null でなくなる
#   ⑤ 英語: 「View/Panel」の並びが "Fleet View" … "Usage View"。閉じた状態から
#      "View/Panel/Tasks View"、続けて "Usage View" を invoke → それぞれのビューが開く
#   ⑥ MCP の tako_menu invoke でも同じ（「表示/パネル/usage ビュー」）
#   ⑦ 無い項目（「表示/パネル/logs ビュー」）は失敗で返り、パネルの状態を変えない
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-menu-panel-views-2341.sh`
# で渡すと ① ② ③ ④ ⑤ ⑥ が NG になる（#2341 の症状 = メニューに tasks / usage が無い）。
# ⑦ は修正前も通る（無い項目を断る挙動は変えていない）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-menu-panel-views-2341.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2341-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2341-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
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
PANIC_LOG="$TAKO_DATA_DIR/panic.log"
printf '{\n  "language": "ja"\n}\n' > "$TAKO_DATA_DIR/settings.json"

# JSON（標準入力）の 1 項目を取り出す（無ければ __NONE__、null は __NULL__）
json_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
v = d.get(sys.argv[1], "__NONE__")
if v is None:
    print("__NULL__")
elif isinstance(v, bool):
    print("true" if v else "false")
else:
    print(json.dumps(v, ensure_ascii=False) if isinstance(v, (list, dict)) else v)' "$1"
}
# 「開いているか:ビュー」の 1 語（パネルの状態を 1 回の読みで比べる）
panel_state() {
  "$TAKO_BIN" panel 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print("%s:%s" % ("open" if d.get("visible") else "closed", d.get("view")))'
}
panel_openness() { panel_state | cut -d: -f1; }
usage_level() {
  "$TAKO_BIN" panel 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print("null" if d.get("usage") is None else "present")'
}
lang_resolved() { "$TAKO_BIN" lang 2>/dev/null | json_field resolved; }
# `tako menu list` の「<メニュー名>/<サブメニュー名>」の項目を `項目名=アクション` のカンマ区切りで返す
submenu_items() {
  "$TAKO_BIN" menu list 2>/dev/null | python3 -c 'import json,sys
menu, sub = sys.argv[1], sys.argv[2]
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for m in d.get("menus", []):
    if m.get("name") != menu:
        continue
    for it in m.get("items", []):
        if it.get("kind") == "submenu" and it.get("label") == sub:
            print(",".join("%s=%s" % (c.get("label"), c.get("action"))
                           for c in it.get("items", []) if c.get("kind") == "action"))
            sys.exit(0)
print("__NO_SUBMENU__")' "$1" "$2"
}
# 値が期待どおりになるまで待つ（メニュー操作は描画の中で消化されるので非同期）
wait_value() {
  local want="$1" getter="$2" i v=""
  for i in $(seq 1 40); do
    v="$($getter)"
    [ "$v" = "$want" ] && { echo "$v"; return 0; }
    sleep 0.25
  done
  echo "$v"
  return 1
}
# invoke して成功で返り、パネルの状態が期待へ変わるか
invoke_panel() {
  local label="$1" query="$2" want="$3" out rc
  out="$("$TAKO_BIN" menu invoke "$query" 2>&1)"
  rc=$?
  echo "      tako menu invoke '$query' → rc=$rc $out"
  check_eq "$label: invoke が成功で返る" "0" "$rc"
  check_eq "$label: パネルが $want になる" "$want" "$(wait_value "$want" panel_state)"
}
# MCP の tools/call を 1 本撃ち、結果の本文（JSON）を $TMP/mcp-body.json へ書く。isError を返す
mcp_call() {
  local tool="$1" args="$2"
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-2341","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"${tool}","arguments":${args}}}
JSONL
  # `mcp serve` はツール公開の判定を env だけで行う（FR-2.3.2）ので接続情報ファイルから渡す。
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  local sock token
  sock="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  token="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  if [ -z "$sock" ] || [ -z "$token" ]; then
    echo "is_error=__NO_DISCOVERY__"
    return
  fi
  env TAKO_SOCKET="$sock" TAKO_TOKEN="$token" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  python3 - "$TMP/mcp-out.jsonl" "$TMP/mcp-body.json" <<'PY'
import json, sys
out = open(sys.argv[2], "w", encoding="utf-8")
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    res = msg.get("result") or {}
    text = "".join(c.get("text", "") for c in res.get("content", []) if c.get("type") == "text")
    out.write(text)
    print("is_error=%s" % bool(res.get("isError") or msg.get("error")))
    sys.exit(0)
print("is_error=__NO_RESPONSE__")
PY
}

APP_LOG="$TMP/app.log"
echo "== 起動（日本語） =="
# 面を用意できないときは起動せずに 4 が返る（#1744）。そのときは止めて「未実測」にする
launch_isolated_gui "$APP_LOG" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$APP_LOG"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR bin=$TAKO_BIN"
check_eq "起動: 表示言語は日本語" "ja" "$(lang_resolved)"

JA_WANT="fleet ビュー=tako::ShowFleetPanel,orch ビュー=tako::ShowOrchPanel,git ビュー=tako::ShowGitPanel,tasks ビュー=tako::ShowTasksPanel,usage ビュー=tako::ShowUsagePanel"
EN_WANT="Fleet View=tako::ShowFleetPanel,Orch View=tako::ShowOrchPanel,Git View=tako::ShowGitPanel,Tasks View=tako::ShowTasksPanel,Usage View=tako::ShowUsagePanel"

echo "== ① 日本語: 「表示/パネル」の並び =="
ITEMS="$(submenu_items "表示" "パネル")"
echo "      tako menu list の 表示/パネル → $ITEMS"
check_eq "① 5 項目（fleet / orch / git / tasks / usage）が並ぶ" "$JA_WANT" "$ITEMS"

echo "== ② 閉じた状態から tasks ビュー =="
"$TAKO_BIN" panel --hide >/dev/null 2>&1
check_eq "② 前提: パネルは閉じている" "closed" "$(wait_value closed panel_openness)"
invoke_panel "② フルパス" "表示/パネル/tasks ビュー" "open:tasks"

echo "== ③ 同じ項目をもう一度（トグル） =="
invoke_panel "③ もう一度" "表示/パネル/tasks ビュー" "closed:tasks"

echo "== ④ fleet から usage ビュー（項目名だけ） =="
"$TAKO_BIN" panel --show --view fleet >/dev/null 2>&1
check_eq "④ 前提: fleet が開いている" "open:fleet" "$(wait_value open:fleet panel_state)"
check_eq "④ 前提: usage の段はまだ無い" "null" "$(usage_level)"
invoke_panel "④ 項目名だけ" "usage ビュー" "open:usage"
check_eq "④ usage の段が開いている（tako panel の usage）" "present" "$(wait_value present usage_level)"

echo "== ⑤ 英語: 「View/Panel」の並びと invoke =="
"$TAKO_BIN" lang en >/dev/null 2>&1
en_items() { submenu_items "View" "Panel"; }
ITEMS="$(wait_value "$EN_WANT" en_items)"
echo "      tako menu list の View/Panel → $ITEMS"
check_eq "⑤ 英語の 5 項目が並ぶ" "$EN_WANT" "$ITEMS"
"$TAKO_BIN" panel --hide >/dev/null 2>&1
check_eq "⑤ 前提: パネルは閉じている" "closed" "$(wait_value closed panel_openness)"
invoke_panel "⑤ フルパス" "View/Panel/Tasks View" "open:tasks"
invoke_panel "⑤ 項目名だけ" "Usage View" "open:usage"
"$TAKO_BIN" lang ja >/dev/null 2>&1
ja_items() { submenu_items "表示" "パネル"; }
check_eq "日本語のメニューへ組み直される" "$JA_WANT" "$(wait_value "$JA_WANT" ja_items)"

echo "== ⑥ MCP の tako_menu invoke =="
"$TAKO_BIN" panel --show --view git >/dev/null 2>&1
check_eq "⑥ 前提: git が開いている" "open:git" "$(wait_value open:git panel_state)"
MCP="$(mcp_call tako_menu '{"action":"invoke","path":"表示/パネル/usage ビュー"}')"
echo "      tako_menu invoke '表示/パネル/usage ビュー' → $MCP $(cat "$TMP/mcp-body.json")"
check_eq "⑥ MCP が成功で返る" "is_error=False" "$MCP"
check_eq "⑥ usage が開く" "open:usage" "$(wait_value open:usage panel_state)"

echo "== ⑦ 無い項目は断る =="
BEFORE="$(panel_state)"
OUT="$("$TAKO_BIN" menu invoke "表示/パネル/logs ビュー" 2>&1)"
RC=$?
echo "      tako menu invoke '表示/パネル/logs ビュー' → rc=$RC $OUT"
if [ "$RC" -ne 0 ]; then pass "⑦ 失敗で返る（rc=${RC}）"; else fail "⑦ 無い項目なのに成功で返った"; fi
sleep 1
check_eq "⑦ パネルの状態は変わらない" "$BEFORE" "$(panel_state)"

if [ -f "$PANIC_LOG" ]; then
  fail "panic.log が作られた（どこかで panic した）"
else
  pass "panic.log が作られていない"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
