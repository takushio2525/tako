#!/bin/bash
# test-theme-colors-ignored-1830.sh — `tako theme colors` / MCP `tako_theme colors` が、読めずに
# 既定色へ置き換えた色を応答だけで見分けられる形で返す実経路テスト（Issue #1830）
#
# 何を確かめるか:
#   #1830 以前は、settings.json の上書きに読めない値（`#赤色` / `#12345`）がある色も
#   `source: "override"` のまま返していた。hex は既定色なのに出どころは上書きと書かれるので、
#   応答だけを見た人（AI）が「上書きが効いている」と読み違える。
#      ① CLI: 読めない色は `source: "ignored"`・hex は既定色。読める上書きは override・
#         上書きの無い色は builtin のまま。`warnings`（`<キー>: <理由>`）に読めない色 2 つと
#         未知のキー 1 つが載り、読める上書きは載らない
#      ② MCP の tako_theme colors も CLI と同じ colors / warnings（同じ dispatch の 1 実装）
#      ③ ライトへ切り替える（dark の上書きを読まない）と全色 builtin・warnings はキーごと消える
#      ④ プリセットの読めない色も ignored（プリセットの経路）
#      ⑤ 手で直すと ignored は消え、応答のキーは従来の 2 つ（theme / colors）に戻る
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-theme-colors-ignored-1830.sh`
# で渡すと ① ② ④ の ignored と warnings が NG になる（#1830 の症状）。③ ⑤ は修正前も通る
# （上書きを読まない・読める値だけの応答は #1830 の前後で同じ形）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-theme-colors-ignored-1830.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
# 応答の組み方そのものは dispatch の単体テスト（`theme_colorsは読めない上書きをignoredで返す`）が CI で見る。
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

TMP="$(mktemp -d /tmp/tako-1830-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1830-$$"
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

# settings.json のテーマ関係を書き換える（`theme <名前>` / `dark <キー> <値。空なら消す>` /
# `preset <名前> <キー> <値>`）。**ユーザーが手で直す**のと同じく、GUI を通さずにファイルだけを書く
edit_settings() {
  python3 - "$SETTINGS" "$@" <<'PY'
import json, os, sys
p, op = sys.argv[1], sys.argv[2]
d = {}
if os.path.exists(p):
    try:
        d = json.load(open(p, encoding="utf-8"))
    except Exception:
        d = {}
if op == "theme":
    d["theme"] = sys.argv[3]
elif op == "language":
    d["language"] = sys.argv[3]
elif op == "dark":
    key, value = sys.argv[3], sys.argv[4]
    colors = d.setdefault("theme_colors", {}).setdefault("dark", {})
    if value:
        colors[key] = value
    else:
        colors.pop(key, None)
elif op == "preset":
    name, key, value = sys.argv[3], sys.argv[4], sys.argv[5]
    preset = d.setdefault("theme_presets", {}).setdefault(name, {"base": "dark", "colors": {}})
    preset.setdefault("colors", {})[key] = value
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
}
# 応答（標準入力の JSON）から色 1 つの項目を読む（`source` / `hex`。無ければ __NONE__）
color_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print(((d.get("colors") or {}).get(sys.argv[1]) or {}).get(sys.argv[2], "__NONE__"))' "$1" "$2"
}
# 応答の warnings を辞書順の JSON 配列で出す（キーが無ければ __NONE__）
warnings_of() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
w = d.get("warnings")
print("__NONE__" if w is None else json.dumps(sorted(w), ensure_ascii=False))'
}
# 出どころごとの色の数（`builtin=55 ignored=2 override=1` の形。比べやすいよう名前順）
source_counts() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
c = {}
for v in (d.get("colors") or {}).values():
    c[v.get("source")] = c.get(v.get("source"), 0) + 1
print(" ".join("%s=%d" % (k, c[k]) for k in sorted(c)))'
}
response_keys() { python3 -c 'import json,sys; print(",".join(sorted(json.load(sys.stdin).keys())))' 2>/dev/null; }
# 2 つの応答の colors / warnings が同じか（CLI と MCP の突き合わせ）
same_colors() {
  python3 - "$1" "$2" <<'PY'
import json, sys
a, b = (json.load(open(p, encoding="utf-8")) for p in sys.argv[1:3])
print("same" if (a.get("colors"), a.get("warnings")) == (b.get("colors"), b.get("warnings")) else "differ")
PY
}
# MCP の tools/call を 1 本撃ち、結果の本文（JSON）を $TMP/mcp-body.json へ書く。isError を返す
mcp_call() {
  local tool="$1" args="$2"
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1830","version":"0"}}}
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
# 応答から読めない色・未知のキーの行を抜き出して見せる（全 58 色を貼ると読めないので）
show_response() {
  python3 -c 'import json,sys
d = json.load(sys.stdin)
keep = {k: v for k, v in (d.get("colors") or {}).items() if k in sys.argv[1:] }
out = {"theme": d.get("theme"), "colors(抜粋)": keep}
if "warnings" in d:
    out["warnings"] = d["warnings"]
print(json.dumps(out, ensure_ascii=False))' "$@" 2>/dev/null
}

# 既定のダークの accent / red（読めない値のときに出るはずの hex）
BUILTIN_ACCENT="#89b4fa"
BUILTIN_RED="#f38ba8"

APP_LOG="$TMP/app.log"
echo "== 起動（ダーク・読めない色 2 つ + 読める色 1 つ + 未知のキー 1 つ） =="
edit_settings language ja
edit_settings theme dark
edit_settings dark accent '#赤色'
edit_settings dark red '#12345'
edit_settings dark green '#00ff00'
edit_settings dark nosuch '#000000'
# 面を用意できないときは起動せずに 4 が返る（#1744）。そのときは止めて「未実測」にする
launch_isolated_gui "$APP_LOG" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$APP_LOG"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR bin=$TAKO_BIN"

echo "== ① CLI: tako theme colors =="
"$TAKO_BIN" theme colors > "$TMP/cli.json" 2>&1
echo "      tako theme colors → $(show_response accent red green background < "$TMP/cli.json")"
check_eq "① accent（#赤色）は ignored" "ignored" "$(color_field accent source < "$TMP/cli.json")"
check_eq "① accent の hex は既定色" "$BUILTIN_ACCENT" "$(color_field accent hex < "$TMP/cli.json")"
check_eq "① red（#12345）は ignored" "ignored" "$(color_field red source < "$TMP/cli.json")"
check_eq "① red の hex は既定色" "$BUILTIN_RED" "$(color_field red hex < "$TMP/cli.json")"
check_eq "① green（#00ff00）は override" "override" "$(color_field green source < "$TMP/cli.json")"
check_eq "① green の hex は上書きの値" "#00ff00" "$(color_field green hex < "$TMP/cli.json")"
check_eq "① background（上書き無し）は builtin" "builtin" "$(color_field background source < "$TMP/cli.json")"
check_eq "① 出どころの内訳（58 色）" "builtin=55 ignored=2 override=1" "$(source_counts < "$TMP/cli.json")"
CLI_WARN="$(warnings_of < "$TMP/cli.json")"
echo "      warnings → $CLI_WARN"
WARN_COUNT="$(printf '%s' "$CLI_WARN" | python3 -c 'import json,sys
try:
    print(len(json.loads(sys.stdin.read())))
except Exception:
    print(0)' 2>/dev/null)"
check_eq "① warnings は 3 件（accent / red / 未知のキー）" "3" "$WARN_COUNT"
for needle in '"accent: ' '"red: ' '"未知の色キー: nosuch"'; do
  case "$CLI_WARN" in
    *"$needle"*) pass "① warnings に ${needle} がある" ;;
    *) fail "① warnings に ${needle} が無い" ;;
  esac
done
case "$CLI_WARN" in
  *'"green: '*) fail "① 読める上書き（green）まで warnings に載っている" ;;
  *) pass "① 読める上書き（green）は warnings に載らない" ;;
esac

echo "== ② MCP: tako_theme colors =="
MCP="$(mcp_call tako_theme '{"action":"colors"}')"
command cp -f "$TMP/mcp-body.json" "$TMP/mcp.json"
echo "      tako_theme colors → $MCP $(show_response accent red green background < "$TMP/mcp.json")"
check_eq "② MCP が成功で返る" "is_error=False" "$MCP"
check_eq "② MCP の accent は ignored" "ignored" "$(color_field accent source < "$TMP/mcp.json")"
check_eq "② MCP の colors / warnings が CLI と同じ" "same" "$(same_colors "$TMP/cli.json" "$TMP/mcp.json")"

echo "== ③ ライト（dark の上書きを読まない） =="
edit_settings theme light
"$TAKO_BIN" theme colors > "$TMP/light.json" 2>&1
echo "      tako theme colors → $(show_response accent red < "$TMP/light.json")"
check_eq "③ 全色 builtin" "builtin=58" "$(source_counts < "$TMP/light.json")"
check_eq "③ warnings はキーごと出ない" "__NONE__" "$(warnings_of < "$TMP/light.json")"

echo "== ④ プリセットの読めない色 =="
edit_settings preset ocean accent '#xyz'
edit_settings preset ocean teal '#0000ff'
edit_settings theme ocean
"$TAKO_BIN" theme colors > "$TMP/preset.json" 2>&1
echo "      tako theme colors → $(show_response accent teal < "$TMP/preset.json")"
check_eq "④ プリセットの accent（#xyz）は ignored" "ignored" "$(color_field accent source < "$TMP/preset.json")"
check_eq "④ プリセットの teal は override" "override" "$(color_field teal source < "$TMP/preset.json")"
check_eq "④ プリセットの内訳（dark の上書きは読まない）" "builtin=56 ignored=1 override=1" \
  "$(source_counts < "$TMP/preset.json")"
case "$(warnings_of < "$TMP/preset.json")" in
  *'"accent: '*) pass "④ warnings に accent がある" ;;
  *) fail "④ warnings に accent が無い（$(warnings_of < "$TMP/preset.json")）" ;;
esac

echo "== ⑤ 手で直す（読める値だけ） =="
edit_settings theme dark
edit_settings dark accent '#ff0000'
edit_settings dark red ''
edit_settings dark nosuch ''
"$TAKO_BIN" theme colors > "$TMP/fixed.json" 2>&1
echo "      tako theme colors → $(show_response accent red green < "$TMP/fixed.json")"
check_eq "⑤ accent は override" "override" "$(color_field accent source < "$TMP/fixed.json")"
check_eq "⑤ red は builtin" "builtin" "$(color_field red source < "$TMP/fixed.json")"
check_eq "⑤ 内訳に ignored が無い" "builtin=56 override=2" "$(source_counts < "$TMP/fixed.json")"
check_eq "⑤ 応答のキーは従来の 2 つ" "colors,theme" "$(response_keys < "$TMP/fixed.json")"

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
