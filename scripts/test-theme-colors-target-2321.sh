#!/bin/bash
# test-theme-colors-target-2321.sh — `tako theme colors --target <テーマ>` / MCP `tako_theme colors`
# の `target` が、いまのテーマに依らずそのテーマの色を返す実経路テスト（Issue #2321）
#
# 何を確かめるか:
#   #2321 以前は、CLI も MCP も `target` を dispatch へ渡しているのに `colors` が読み捨て、
#   常に settings.json のいまのテーマの色を返していた。ライトの色を確かめたいのにダークの値が返る。
#      ① いまのテーマがダーク: `--target light` はライトの既定色 + light の上書きの出どころ
#         （読める上書きは override・読めない上書きは ignored）+ light の表の warnings だけ。
#         dark の上書きは混ぜない。`--target dark` はいまのテーマの応答（target 省略）とバイト一致
#      ② MCP の tako_theme colors（target 付き）も CLI と同じ応答
#      ③ settings.json だけライトへ書き換える（GUI はダークのまま = いまのテーマが食い違う）:
#         `--target dark` / `--target light` の応答は ① とバイト一致（いまのテーマに依らない）。
#         target 省略はライトの応答 = `--target light` とバイト一致
#      ④ プリセット名の target はそのプリセット（base の既定色 + プリセットの上書き）
#      ⑤ 知らないテーマ名の target はエラー（CLI は非 0 で理由・MCP は isError）。黙ってダークへ
#         落とさない
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-theme-colors-target-2321.sh`
# で渡すと ① ③ ④ の target 付きの応答が NG になる（#2321 の症状）。
# `TAKO_2321_KEEP=<dir>` を渡すと target 省略の応答を残す（修正前後のバイト一致の突き合わせ用）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-theme-colors-target-2321.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
# 応答の組み方と dispatch の配線は単体テスト（`issue2321_*`）が CI で見る。
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
# 2 つのファイルがバイト一致か
check_same_bytes() {
  if cmp -s "$2" "$3"; then pass "$1"; else fail "$1（$(basename "$2") と $(basename "$3") が食い違う）"; fi
}

TMP="$(mktemp -d /tmp/tako-2321-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2321-$$"
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
SETTINGS="$TAKO_DATA_DIR/settings.json"
KEEP="${TAKO_2321_KEEP:-}"
[ -n "$KEEP" ] && mkdir -p "$KEEP"

# settings.json のテーマ関係を書き換える（`theme <名前>` / `dark|light <キー> <値>` /
# `preset <名前> <base> <キー> <値>`）。**ユーザーが手で直す**のと同じく、GUI を通さずにファイルだけを書く
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
elif op in ("dark", "light"):
    key, value = sys.argv[3], sys.argv[4]
    d.setdefault("theme_colors", {}).setdefault(op, {})[key] = value
elif op == "preset":
    name, base, key, value = sys.argv[3], sys.argv[4], sys.argv[5], sys.argv[6]
    preset = d.setdefault("theme_presets", {}).setdefault(name, {"base": base, "colors": {}})
    preset["base"] = base
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
# 応答のトップの項目（`theme` など。無ければ __NONE__）
top_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print(d.get(sys.argv[1], "__NONE__"))' "$1"
}
# 応答の warnings の各行の先頭のキー（`red: …` の red）を辞書順に並べる（キーが無ければ __NONE__）
warning_keys() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
w = d.get("warnings")
print("__NONE__" if w is None else ",".join(sorted(x.split(":")[0] for x in w)))'
}
# 出どころごとの色の数（`builtin=56 ignored=1 override=1` の形。比べやすいよう名前順）
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
# 2 つの応答（JSON）が値として同じか（CLI と MCP の突き合わせ。MCP の本文は整形が違いうる）
same_json() {
  python3 - "$1" "$2" <<'PY'
import json, sys
try:
    a, b = (json.load(open(p, encoding="utf-8")) for p in sys.argv[1:3])
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
print("same" if a == b else "differ")
PY
}
# MCP の tools/call を 1 本撃ち、結果の本文（JSON）を $TMP/mcp-body.json へ書く。isError を返す
mcp_call() {
  local tool="$1" args="$2"
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-2321","version":"0"}}}
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
# 応答から見る色だけを抜き出して見せる（全 58 色を貼ると読めないので）
show_response() {
  python3 -c 'import json,sys
d = json.load(sys.stdin)
keep = {k: v for k, v in (d.get("colors") or {}).items() if k in sys.argv[1:] }
out = {"theme": d.get("theme"), "colors(抜粋)": keep}
if "warnings" in d:
    out["warnings"] = d["warnings"]
print(json.dumps(out, ensure_ascii=False))' "$@" 2>/dev/null || echo "（JSON でない: $(head -c 200)）"
}
# CLI で colors を引いて $TMP/<名前>.json へ書く（target を省くなら 2 つ目を空に）
cli_colors() {
  local name="$1" target="${2-}"
  if [ -n "$target" ]; then
    "$TAKO_BIN" theme colors --target "$target" > "$TMP/$name.json" 2>&1
  else
    "$TAKO_BIN" theme colors > "$TMP/$name.json" 2>&1
  fi
}

# 既定色（Catppuccin Mocha / Latte）
DARK_BG="#1e1e2e"
DARK_ACCENT="#89b4fa"
LIGHT_BG="#eff1f5"
LIGHT_RED="#d20f39"
LIGHT_GREEN="#40a02b"

APP_LOG="$TMP/app.log"
echo "== 起動（いまのテーマはダーク・dark と light の表に読める / 読めない上書き） =="
edit_settings language ja
edit_settings theme dark
edit_settings dark accent '#赤色'
edit_settings dark green '#00ff00'
edit_settings light accent '#ff0000'
edit_settings light red '#xyz'
edit_settings preset ocean light teal '#0000ff'
edit_settings preset ocean light mauve '#12345'
# 面を用意できないときは起動せずに 4 が返る（#1744）。そのときは止めて「未実測」にする
launch_isolated_gui "$APP_LOG" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$APP_LOG"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR bin=$TAKO_BIN"

echo "== ① CLI（いまのテーマ = ダーク） =="
cli_colors a-none
cli_colors a-light light
cli_colors a-dark dark
[ -n "$KEEP" ] && command cp -f "$TMP/a-none.json" "$KEEP/none-dark.json"
echo "      tako theme colors                → $(show_response background accent red green < "$TMP/a-none.json")"
echo "      tako theme colors --target light → $(show_response background accent red green < "$TMP/a-light.json")"
check_eq "① --target light の theme" "light" "$(top_field theme < "$TMP/a-light.json")"
check_eq "① --target light の background はライトの既定色" "$LIGHT_BG" "$(color_field background hex < "$TMP/a-light.json")"
check_eq "① --target light の accent は light の上書き" "#ff0000" "$(color_field accent hex < "$TMP/a-light.json")"
check_eq "① --target light の accent の出どころ" "override" "$(color_field accent source < "$TMP/a-light.json")"
check_eq "① --target light の red（#xyz）はライトの既定色" "$LIGHT_RED" "$(color_field red hex < "$TMP/a-light.json")"
check_eq "① --target light の red の出どころ" "ignored" "$(color_field red source < "$TMP/a-light.json")"
check_eq "① --target light の green は dark の上書きを混ぜない" "$LIGHT_GREEN builtin" \
  "$(color_field green hex < "$TMP/a-light.json") $(color_field green source < "$TMP/a-light.json")"
check_eq "① --target light の内訳" "builtin=56 ignored=1 override=1" "$(source_counts < "$TMP/a-light.json")"
check_eq "① --target light の warnings は light の表のものだけ" "red" "$(warning_keys < "$TMP/a-light.json")"
check_eq "① --target dark の accent（#赤色）" "$DARK_ACCENT ignored" \
  "$(color_field accent hex < "$TMP/a-dark.json") $(color_field accent source < "$TMP/a-dark.json")"
check_eq "① --target dark の warnings" "accent" "$(warning_keys < "$TMP/a-dark.json")"
check_same_bytes "① --target dark はいまのテーマ（target 省略）の応答とバイト一致" "$TMP/a-none.json" "$TMP/a-dark.json"

echo "== ② MCP（tako_theme colors + target） =="
MCP="$(mcp_call tako_theme '{"action":"colors","target":"light"}')"
command cp -f "$TMP/mcp-body.json" "$TMP/mcp-light.json"
echo "      tako_theme colors target=light → $MCP $(show_response background accent red < "$TMP/mcp-light.json")"
check_eq "② MCP（light）が成功で返る" "is_error=False" "$MCP"
check_eq "② MCP（light）の応答が CLI と同じ" "same" "$(same_json "$TMP/a-light.json" "$TMP/mcp-light.json")"
MCP="$(mcp_call tako_theme '{"action":"colors","target":"dark"}')"
command cp -f "$TMP/mcp-body.json" "$TMP/mcp-dark.json"
check_eq "② MCP（dark）が成功で返る" "is_error=False" "$MCP"
check_eq "② MCP（dark）の応答が CLI と同じ" "same" "$(same_json "$TMP/a-dark.json" "$TMP/mcp-dark.json")"

echo "== ③ settings.json だけライト（GUI はダークのまま） =="
edit_settings theme light
"$TAKO_BIN" theme > "$TMP/status.json" 2>&1
check_eq "③ いまのテーマが食い違っている（settings=light / GUI=dark）" "light dark" \
  "$(top_field theme < "$TMP/status.json") $(top_field mode < "$TMP/status.json")"
cli_colors b-none
cli_colors b-light light
cli_colors b-dark dark
[ -n "$KEEP" ] && command cp -f "$TMP/b-none.json" "$KEEP/none-light.json"
echo "      tako theme colors --target dark → $(show_response background accent green < "$TMP/b-dark.json")"
check_eq "③ --target dark の background はダークの既定色" "$DARK_BG" "$(color_field background hex < "$TMP/b-dark.json")"
check_eq "③ --target dark の green は dark の上書き" "#00ff00 override" \
  "$(color_field green hex < "$TMP/b-dark.json") $(color_field green source < "$TMP/b-dark.json")"
check_same_bytes "③ --target dark は ① とバイト一致（いまのテーマに依らない）" "$TMP/a-dark.json" "$TMP/b-dark.json"
check_same_bytes "③ --target light は ① とバイト一致（いまのテーマに依らない）" "$TMP/a-light.json" "$TMP/b-light.json"
check_same_bytes "③ target 省略（ライト）は --target light とバイト一致" "$TMP/b-none.json" "$TMP/b-light.json"
MCP="$(mcp_call tako_theme '{"action":"colors","target":"dark"}')"
command cp -f "$TMP/mcp-body.json" "$TMP/mcp-b-dark.json"
check_eq "③ MCP（dark）の応答が CLI と同じ" "same" "$(same_json "$TMP/b-dark.json" "$TMP/mcp-b-dark.json")"

echo "== ④ プリセット名の target =="
cli_colors c-ocean ocean
echo "      tako theme colors --target ocean → $(show_response background teal mauve < "$TMP/c-ocean.json")"
check_eq "④ theme はプリセット名" "ocean" "$(top_field theme < "$TMP/c-ocean.json")"
check_eq "④ background は base（light）の既定色" "$LIGHT_BG" "$(color_field background hex < "$TMP/c-ocean.json")"
check_eq "④ teal はプリセットの上書き" "#0000ff override" \
  "$(color_field teal hex < "$TMP/c-ocean.json") $(color_field teal source < "$TMP/c-ocean.json")"
check_eq "④ mauve（#12345）は ignored" "ignored" "$(color_field mauve source < "$TMP/c-ocean.json")"
check_eq "④ light の表（accent）はプリセットへ混ぜない" "builtin" "$(color_field accent source < "$TMP/c-ocean.json")"
check_eq "④ warnings はプリセットのものだけ" "mauve" "$(warning_keys < "$TMP/c-ocean.json")"

echo "== ⑤ 知らないテーマ名の target =="
"$TAKO_BIN" theme colors --target nosuch > "$TMP/d-cli.txt" 2>&1
RC=$?
echo "      tako theme colors --target nosuch → rc=$RC $(head -c 300 "$TMP/d-cli.txt")"
check_eq "⑤ CLI は非 0 で返る" "nonzero" "$([ "$RC" -ne 0 ] && echo nonzero || echo zero)"
case "$(cat "$TMP/d-cli.txt")" in
  *nosuch*) pass "⑤ CLI の理由に名前が出る" ;;
  *) fail "⑤ CLI の理由に名前が出ない" ;;
esac
MCP="$(mcp_call tako_theme '{"action":"colors","target":"nosuch"}')"
echo "      tako_theme colors target=nosuch → $MCP $(head -c 300 "$TMP/mcp-body.json")"
check_eq "⑤ MCP は isError" "is_error=True" "$MCP"

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
