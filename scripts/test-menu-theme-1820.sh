#!/bin/bash
# test-menu-theme-1820.sh — メニュー項目を tako menu invoke で実行でき、tako theme の応答に
# 読めない色の警告が載る実経路テスト（Issue #1820）
#
# 何を確かめるか:
#   1. メニューの項目名に `/` があると `tako menu invoke` のパス区切りと衝突して名指しできない
#      （「ライト / ダークを切替」→ `メニュー項目 '…' が見つかりません`）。項目名は
#      `tako menu list` からアクション名で引くので、どの版のバイナリでも同じ手順になる
#      ① 日本語: テーマ切替をフルパス・項目名だけの両方で invoke → 実際にテーマが変わる
#      ② 日本語: 表示言語の切替を invoke → 実際に英語へ変わる（旧「（日本語 / English）」）
#      ③ 英語: テーマ切替・表示言語の切替を invoke → 実際に変わる
#      ④ macOS: 「拡大・縮小」を invoke できる（旧「拡大 / 縮小」）
#      ⑤ MCP の tako_menu invoke でも同じ
#   2. `tako theme` / MCP `tako_theme` の応答に `warnings`（無視した色と理由）が載る
#      ⑥ 読めない色が 2 つ: status の warnings が 2 件で、persist.log の行の本体と一致する
#      ⑦ MCP の tako_theme status も CLI と同じ warnings
#      ⑧ ライトへ切り替える（dark の上書きを読まない）と warnings はキーごと消え、戻すと戻る
#      ⑨ 手で直して読み直すと warnings はキーごと消える（応答のキーは従来の 4 つ）
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-menu-theme-1820.sh`
# で渡すと ① ② ③ の言語切替 ④ ⑤ ⑥ ⑦ ⑧ が NG になる（#1820 の症状）。③ のテーマ切替は
# 修正前も通る（英語の "Toggle Light/Dark Theme" は `/` の前後に空白が無く、分割した
# パスの末尾一致で偶然届く）ので、ラベルに `/` を置かない規則そのものは単体の番犬が見る。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-menu-theme-1820.sh
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

TMP="$(mktemp -d /tmp/tako-1820-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1820-$$"
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
SETTINGS="$TAKO_DATA_DIR/settings.json"
PANIC_LOG="$TAKO_DATA_DIR/panic.log"
PERSIST_LOG="$TAKO_DATA_DIR/persist.log"

# settings.json の値を書き換える（`set <キー> <値>` / `color <キー> <値。空なら消す>`）。
# **ユーザーが手で直す**のと同じく、GUI を通さずにファイルだけを書き換える
edit_settings() {
  python3 - "$SETTINGS" "$@" <<'PY'
import json, os, sys
p, op, key, value = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
d = {}
if os.path.exists(p):
    try:
        d = json.load(open(p, encoding="utf-8"))
    except Exception:
        d = {}
if op == "set":
    d[key] = value
else:
    colors = d.setdefault("theme_colors", {}).setdefault("dark", {})
    if value:
        colors[key] = value
    else:
        colors.pop(key, None)
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
}
# JSON（標準入力）の 1 項目を取り出す（`mode` / `resolved` など。無ければ __NONE__）
json_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
v = d.get(sys.argv[1], "__NONE__")
print(json.dumps(v, ensure_ascii=False) if isinstance(v, (list, dict)) else v)' "$1"
}
theme_mode() { "$TAKO_BIN" theme 2>/dev/null | json_field mode; }
lang_resolved() { "$TAKO_BIN" lang 2>/dev/null | json_field resolved; }
# `tako menu list` からアクション名で項目を引き、「メニュー名<TAB>項目名」を返す
menu_item() {
  "$TAKO_BIN" menu list 2>/dev/null | python3 -c 'import json,sys
want = sys.argv[1]
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
def walk(prefix, items):
    for it in items:
        if it.get("kind") == "action" and it.get("action") == want:
            print(prefix + "\t" + it["label"]); sys.exit(0)
        if it.get("kind") == "submenu":
            walk(prefix + "/" + it["label"], it.get("items", []))
for m in d.get("menus", []):
    walk(m["name"], m.get("items", []))' "$1"
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
other_mode() { if [ "$1" = "dark" ]; then echo light; else echo dark; fi; }
# invoke して成功で返り、テーマが実際に反転するか
invoke_theme() {
  local label="$1" query="$2" before after out rc
  before="$(theme_mode)"
  out="$("$TAKO_BIN" menu invoke "$query" 2>&1)"
  rc=$?
  echo "      tako menu invoke '$query' → rc=$rc $out"
  check_eq "$label: invoke が成功で返る" "0" "$rc"
  after="$(wait_value "$(other_mode "$before")" theme_mode)"
  check_eq "$label: テーマが実際に変わる（$before → ）" "$(other_mode "$before")" "$after"
}
# MCP の tools/call を 1 本撃ち、結果の本文（JSON）を $TMP/mcp-body.json へ書く。isError を返す
mcp_call() {
  local tool="$1" args="$2"
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1820","version":"0"}}}
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
# persist.log の「テーマの色上書きを無視: <本体> [pid N]」から、まだ解消していない本体を
# 並べる（= GUI が適用中のテーマで無視している色。比べやすいよう辞書順）
persist_warnings() {
  python3 - "$PERSIST_LOG" <<'PY'
import json, re, sys
ignored, resolved = "テーマの色上書きを無視: ", "テーマの色上書きの警告が解消（読み直した設定では出ない）: "
live = []
try:
    lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
except FileNotFoundError:
    lines = []
for line in lines:
    body = re.sub(r" \[pid \d+\]$", "", line)
    for head, add in ((ignored, True), (resolved, False)):
        at = body.find(head)
        if at < 0:
            continue
        w = body[at + len(head):]
        if add:
            live.append(w)
        elif w in live:
            live.remove(w)
print(json.dumps(sorted(live), ensure_ascii=False))
PY
}
sorted_json() { python3 -c 'import json,sys; print(json.dumps(sorted(json.loads(sys.stdin.read() or "[]")), ensure_ascii=False))'; }
response_keys() { python3 -c 'import json,sys; print(",".join(sorted(json.load(sys.stdin).keys())))'; }

APP_LOG="$TMP/app.log"
echo "== 起動（日本語・ダーク・読めない色 2 つ + 正しい色 1 つ） =="
edit_settings set language ja
edit_settings set theme dark
edit_settings color accent '#赤色'
edit_settings color red '#12345'
edit_settings color green '#00ff00'
# 面を用意できないときは起動せずに 4 が返る（#1744）。そのときは止めて「未実測」にする
launch_isolated_gui "$APP_LOG" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$APP_LOG"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR bin=$TAKO_BIN"
check_eq "起動: 表示言語は日本語" "ja" "$(lang_resolved)"

echo "== ① 日本語: テーマ切替を invoke =="
ITEM="$(menu_item tako::ToggleTheme)"
MENU="${ITEM%%$'\t'*}"
LABEL="${ITEM#*$'\t'}"
echo "  メニューの項目: '$MENU' / '$LABEL'"
invoke_theme "① フルパス" "$MENU/$LABEL"
invoke_theme "① 項目名だけ" "$LABEL"

echo "== ② 日本語: 表示言語の切替を invoke =="
ITEM="$(menu_item tako::SwitchLanguage)"
MENU="${ITEM%%$'\t'*}"
LABEL="${ITEM#*$'\t'}"
echo "  メニューの項目: '$MENU' / '$LABEL'"
OUT="$("$TAKO_BIN" menu invoke "$MENU/$LABEL" 2>&1)"
RC=$?
echo "      tako menu invoke '$MENU/$LABEL' → rc=$RC $OUT"
check_eq "② invoke が成功で返る" "0" "$RC"
check_eq "② 表示言語が実際に英語へ変わる" "en" "$(wait_value en lang_resolved)"

echo "== ③ 英語: テーマ切替・表示言語の切替を invoke =="
# 言語を変えるとメニュー定義は次の描画で貼り直される。どちらの経路で英語になっても
# 以降を英語で揃えるため、ここで明示的に英語へ寄せて貼り直しを待つ
"$TAKO_BIN" lang en >/dev/null 2>&1
view_name() { menu_item tako::ToggleTheme | cut -f1; }
check_eq "③ メニューが英語で組み直される" "View" "$(wait_value View view_name)"
ITEM="$(menu_item tako::ToggleTheme)"
MENU="${ITEM%%$'\t'*}"
LABEL="${ITEM#*$'\t'}"
echo "  メニューの項目: '$MENU' / '$LABEL'"
invoke_theme "③ フルパス" "$MENU/$LABEL"
invoke_theme "③ 項目名だけ" "$LABEL"
ITEM="$(menu_item tako::SwitchLanguage)"
MENU="${ITEM%%$'\t'*}"
LABEL="${ITEM#*$'\t'}"
echo "  メニューの項目: '$MENU' / '$LABEL'"
OUT="$("$TAKO_BIN" menu invoke "$LABEL" 2>&1)"
RC=$?
echo "      tako menu invoke '$LABEL' → rc=$RC $OUT"
check_eq "③ 表示言語の切替の invoke が成功で返る" "0" "$RC"
check_eq "③ 表示言語が実際に日本語へ戻る" "ja" "$(wait_value ja lang_resolved)"

# 以降は日本語のメニューで見る（修正前に `/` を含んでいたのは日本語側の文言が多い）。
# ③ の言語切替が効かなかった版でも同じ条件から始まるよう、明示的に寄せて貼り直しを待つ
"$TAKO_BIN" lang ja >/dev/null 2>&1
jp_view() { menu_item tako::ToggleTheme | cut -f1; }
check_eq "日本語のメニューへ組み直される" "表示" "$(wait_value "表示" jp_view)"

if [ "$(uname -s)" = "Darwin" ]; then
  echo "== ④ macOS: 「拡大・縮小」を invoke =="
  ITEM="$(menu_item tako::ZoomWindow)"
  LABEL="${ITEM#*$'\t'}"
  echo "  メニューの項目: '${ITEM%%$'\t'*}' / '$LABEL'"
  OUT="$("$TAKO_BIN" menu invoke "$LABEL" 2>&1)"
  RC=$?
  echo "      tako menu invoke '$LABEL' → rc=$RC $OUT"
  check_eq "④ invoke が成功で返る" "0" "$RC"
  # 窓の寸法の変化はアニメーションと面の大きさに左右されるので、ここでは解決までを見る
  # （発火先に on_action があることは単体の番犬が固定している）
  "$TAKO_BIN" menu invoke "$LABEL" >/dev/null 2>&1
fi

echo "== ⑤ MCP の tako_menu invoke =="
LABEL="$(menu_item tako::ToggleTheme | cut -f2)"
BEFORE="$(theme_mode)"
MCP="$(mcp_call tako_menu "{\"action\":\"invoke\",\"path\":\"$LABEL\"}")"
echo "      tako_menu invoke '$LABEL' → $MCP $(cat "$TMP/mcp-body.json")"
check_eq "⑤ MCP が成功で返る" "is_error=False" "$MCP"
check_eq "⑤ テーマが実際に変わる（$BEFORE → ）" "$(other_mode "$BEFORE")" \
  "$(wait_value "$(other_mode "$BEFORE")" theme_mode)"
# 以降はダークで始める（dark の上書きが読まれる状態）
if [ "$(theme_mode)" != "dark" ]; then "$TAKO_BIN" theme dark >/dev/null 2>&1; fi

echo "== ⑥ tako theme の応答に warnings（読めない色 2 つ） =="
STATUS="$("$TAKO_BIN" theme 2>&1)"
echo "      tako theme → $STATUS"
RESP="$(printf '%s' "$STATUS" | json_field warnings | sorted_json 2>/dev/null)"
LOGGED="$(persist_warnings)"
echo "      persist.log（未解消の行の本体）→ $LOGGED"
count_of() { python3 -c 'import json,sys; print(len(json.loads(sys.stdin.read() or "[]")))' 2>/dev/null; }
check_eq "⑥ persist.log の未解消は 2 件（accent / red）" "2" "$(printf '%s' "$LOGGED" | count_of)"
check_eq "⑥ warnings が 2 件" "2" "$(printf '%s' "$RESP" | count_of)"
check_eq "⑥ warnings と persist.log の行の本体が一致する" "$LOGGED" "$RESP"
for k in accent red; do
  case "$RESP" in
    *"\"$k: "*) pass "⑥ $k が理由つきで載る" ;;
    *) fail "⑥ $k が載らない" ;;
  esac
done
case "$RESP" in
  *'"green: '*) fail "⑥ 正しい上書き（green）まで載っている" ;;
  *) pass "⑥ 正しい上書き（green）は載らない" ;;
esac

echo "== ⑦ MCP の tako_theme status =="
MCP="$(mcp_call tako_theme '{"action":"status"}')"
echo "      tako_theme status → $MCP $(cat "$TMP/mcp-body.json")"
check_eq "⑦ MCP が成功で返る" "is_error=False" "$MCP"
check_eq "⑦ persist.log と同じ warnings（CLI とも同じ）" "$LOGGED" \
  "$(json_field warnings < "$TMP/mcp-body.json" | sorted_json 2>/dev/null)"

echo "== ⑧ ライトへ切り替える → 戻す =="
OUT="$("$TAKO_BIN" theme toggle 2>&1)"
echo "      tako theme toggle → $OUT"
check_eq "⑧ ライトでは dark の上書きを読まないので warnings はキーごと無い" \
  "available,mode,presets,theme" "$(printf '%s' "$OUT" | response_keys)"
OUT="$("$TAKO_BIN" theme toggle 2>&1)"
echo "      tako theme toggle → $OUT"
BACK="$(printf '%s' "$OUT" | json_field warnings | sorted_json 2>/dev/null)"
check_eq "⑧ ダークへ戻すと warnings が 2 件戻る" "2" "$(printf '%s' "$BACK" | count_of)"
check_eq "⑧ 戻した後も persist.log と一致する" "$(persist_warnings)" "$BACK"

echo "== ⑨ 手で直して読み直す（警告 0 件） =="
edit_settings color accent '#ff8800'
edit_settings color red ''
OUT="$("$TAKO_BIN" theme dark 2>&1)"
echo "      tako theme dark → $OUT"
check_eq "⑨ warnings はキーごと無い（応答のキーは従来の 4 つ）" \
  "available,mode,presets,theme" "$(printf '%s' "$OUT" | response_keys)"
check_eq "⑨ persist.log でも未解消の警告は 0 件" "[]" "$(persist_warnings)"
MCP="$(mcp_call tako_theme '{"action":"status"}')"
echo "      tako_theme status → $MCP $(cat "$TMP/mcp-body.json")"
check_eq "⑨ MCP の応答も warnings を持たない" "available,mode,presets,theme" \
  "$(response_keys < "$TMP/mcp-body.json")"

if [ -f "$PANIC_LOG" ]; then
  fail "panic.log が作られた（どこかで panic した）"
else
  pass "panic.log が作られていない"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
