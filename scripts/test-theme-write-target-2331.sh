#!/bin/bash
# test-theme-write-target-2331.sh — `tako theme color <キー> <値> --target <テーマ>` /
# `tako theme color <キー> --reset --target <テーマ>` / `tako theme reset-colors --target <テーマ>`
# （MCP `tako_theme` の set-color / reset-color / reset-colors）の実経路テスト（Issue #2331）
#
# 何を確かめるか:
#   #2331 以前は、書き込みの 3 操作が target の名前を colors（#2321）と同じ検証に通さず、
#   知らない名前を GUI のいまのモードの表へ落としていた。打ち間違えた `--target` で、
#   いまのテーマの上書きが書き換わる・消える。
#      ① 知らない名前（nosuch / 空文字 / 大文字のプリセット名 OCEAN）の target: CLI は非 0 で、
#         出力は `tako theme colors --target <同じ名前>` とバイト一致（同じ文言）。settings.json は
#         1 バイトも変わらない（前後の cmp）
#      ② MCP の 3 操作も知らない名前は isError で、settings.json は変わらない
#      ③ 知っている名前（大小・前後の空白を問わないビルトイン名 / プリセット名）は従来どおり
#         そのテーマの表へ書く・表から消す
#      ④ target 省略は従来どおり settings.json の theme の表へ（theme が知らない名前なら GUI の
#         モードの表へ）。応答と書いた結果を `TAKO_2331_KEEP` へ残し、修正前とバイトで突き合わせる
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-theme-write-target-2331.sh`
# で渡すと ① ② が NG になる（#2331 の症状）。③ ④ は修正前後とも OK（変えていない挙動）。
# `TAKO_2331_KEEP=<dir>` を渡すと ③ ④ の応答と書いた結果を残す（修正前後のバイト一致の突き合わせ用）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-theme-write-target-2331.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
# 検証の配線は単体テスト（`issue2331_*`）が CI で見る。
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

TMP="$(mktemp -d /tmp/tako-2331-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2331-$$"
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
BASELINE="$TMP/baseline.json"
KEEP="${TAKO_2331_KEEP:-}"
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
# settings.json（引数でほかのファイルも）のテーマ関係だけを 1 行の JSON で（キー順をそろえる。比べる・見せる用）
theme_part() {
  python3 - "${1:-$SETTINGS}" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
keep = {k: d.get(k) for k in ("theme", "theme_colors", "theme_presets")}
print(json.dumps(keep, ensure_ascii=False, sort_keys=True))
PY
}
# settings.json の 1 か所（`dark accent` / `preset ocean teal` / `table light`）。無ければ __NONE__
settings_at() {
  python3 - "$SETTINGS" "$@" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
op = sys.argv[2]
if op == "preset":
    v = ((d.get("theme_presets") or {}).get(sys.argv[3]) or {}).get("colors", {}).get(sys.argv[4])
elif op == "table":
    v = (d.get("theme_colors") or {}).get(sys.argv[3])
    v = None if v is None else json.dumps(v, sort_keys=True)
else:
    v = ((d.get("theme_colors") or {}).get(op) or {}).get(sys.argv[3])
print("__NONE__" if v is None else v)
PY
}
# 基準の settings.json へ戻す（ケースごとに戻し、1 操作の効きだけを見る）。
# GUI は settings.json を監視しないので、書き換えはそのまま次の操作の読み直しに効く
restore_baseline() { command cp -f "$BASELINE" "$SETTINGS"; }
# MCP の tools/call を 1 本撃ち、結果の本文を $TMP/mcp-body.json へ書く。isError を返す
mcp_call() {
  local tool="$1" args="$2"
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-2331","version":"0"}}}
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
# 応答（CLI の出力 + rc）を KEEP へ残す（修正前後の突き合わせ用）。settings.json のテーマ関係も添える
keep_case() {
  local name="$1" out="$2" rc="$3"
  [ -n "$KEEP" ] || return 0
  { cat "$out"; echo "rc=$rc"; echo "settings=$(theme_part)"; } > "$KEEP/$name.txt"
}

echo "== 起動（いまのテーマはダーク・dark と light の表とプリセット ocean に上書き） =="
edit_settings language ja
edit_settings theme dark
edit_settings dark accent '#111111'
edit_settings dark green '#00ff00'
edit_settings light accent '#ff0000'
edit_settings preset ocean light teal '#0000ff'
command cp -f "$SETTINGS" "$BASELINE"
APP_LOG="$TMP/app.log"
# 面を用意できないときは起動せずに 4 が返る（#1744）。そのときは止めて「未実測」にする
launch_isolated_gui "$APP_LOG" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$APP_LOG"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR bin=$TAKO_BIN"
"$TAKO_BIN" theme > "$TMP/status.json" 2>&1
check_eq "前提: いまのテーマはダーク（GUI もダーク）" '"theme": "dark" "mode": "dark"' \
  "$(python3 -c 'import json,sys; d=json.load(sys.stdin); print("\"theme\": \"%s\" \"mode\": \"%s\"" % (d.get("theme"), d.get("mode")))' < "$TMP/status.json" 2>/dev/null)"

echo "== ① 知らない名前の target（CLI） =="
# 3 操作 × 3 つの名前。1 操作ごとに基準へ戻し、その操作が settings.json を変えたかを cmp で見る
for target in nosuch "" OCEAN; do
  "$TAKO_BIN" theme colors --target "$target" > "$TMP/colors-err.txt" 2>&1
  for op in set reset reset-all; do
    restore_baseline
    case "$op" in
      set) label="color accent #123456"
        "$TAKO_BIN" theme color accent '#123456' --target "$target" > "$TMP/out.txt" 2>&1 ;;
      reset) label="color green --reset"
        "$TAKO_BIN" theme color green --reset --target "$target" > "$TMP/out.txt" 2>&1 ;;
      reset-all) label="reset-colors"
        "$TAKO_BIN" theme reset-colors --target "$target" > "$TMP/out.txt" 2>&1 ;;
    esac
    rc=$?
    what="$label --target \"$target\""
    echo "      $what → rc=$rc $(head -c 200 "$TMP/out.txt")"
    check_eq "① $what は非 0" "nonzero" "$([ "$rc" -ne 0 ] && echo nonzero || echo zero)"
    if cmp -s "$TMP/colors-err.txt" "$TMP/out.txt"; then
      pass "① $what の出力は colors --target \"$target\" とバイト一致"
    else
      fail "① $what の出力が colors --target \"$target\"（$(head -c 200 "$TMP/colors-err.txt")）と食い違う"
    fi
    if cmp -s "$BASELINE" "$SETTINGS"; then
      pass "① $what で settings.json は 1 バイトも変わらない"
    else
      fail "① $what で settings.json が変わった"
      echo "         前: $(theme_part "$BASELINE")"
      echo "         後: $(theme_part)"
    fi
  done
done

echo "== ② 知らない名前の target（MCP） =="
for args in \
  '{"action":"set-color","key":"accent","value":"#123456","target":"nosuch"}' \
  '{"action":"reset-color","key":"green","target":"nosuch"}' \
  '{"action":"reset-colors","target":"nosuch"}' \
  '{"action":"set-color","key":"accent","value":"#123456","target":""}'; do
  restore_baseline
  MCP="$(mcp_call tako_theme "$args")"
  echo "      tako_theme $args → $MCP $(head -c 200 "$TMP/mcp-body.json")"
  check_eq "② MCP $args は isError" "is_error=True" "$MCP"
  if cmp -s "$BASELINE" "$SETTINGS"; then
    pass "② MCP $args で settings.json は 1 バイトも変わらない"
  else
    fail "② MCP $args で settings.json が変わった → $(theme_part)"
  fi
done

echo "== ③ 知っている名前の target（従来どおり書く） =="
restore_baseline
"$TAKO_BIN" theme color accent '#123456' --target LIGHT > "$TMP/out.txt" 2>&1
rc=$?
keep_case known-set-LIGHT "$TMP/out.txt" "$rc"
check_eq "③ --target LIGHT は light の表へ（dark は不変）" "0 #123456 #111111" \
  "$rc $(settings_at light accent) $(settings_at dark accent)"
restore_baseline
"$TAKO_BIN" theme color accent '#654321' --target ' light ' > "$TMP/out.txt" 2>&1
rc=$?
keep_case known-set-space-light "$TMP/out.txt" "$rc"
check_eq "③ --target ' light ' は light の表へ" "0 #654321 #111111" \
  "$rc $(settings_at light accent) $(settings_at dark accent)"
restore_baseline
"$TAKO_BIN" theme color mauve '#abcdef' --target ocean > "$TMP/out.txt" 2>&1
rc=$?
keep_case known-set-ocean "$TMP/out.txt" "$rc"
check_eq "③ --target ocean はプリセットへ（dark の表へは入れない）" "0 #abcdef __NONE__" \
  "$rc $(settings_at preset ocean mauve) $(settings_at dark mauve)"
restore_baseline
"$TAKO_BIN" theme color teal --reset --target ocean > "$TMP/out.txt" 2>&1
rc=$?
keep_case known-reset-ocean "$TMP/out.txt" "$rc"
check_eq "③ color teal --reset --target ocean はプリセットから消す" "0 __NONE__" \
  "$rc $(settings_at preset ocean teal)"
restore_baseline
"$TAKO_BIN" theme color accent --reset --target Dark > "$TMP/out.txt" 2>&1
rc=$?
keep_case known-reset-Dark "$TMP/out.txt" "$rc"
check_eq "③ color accent --reset --target Dark は dark から消す（light は不変）" "0 __NONE__ #ff0000" \
  "$rc $(settings_at dark accent) $(settings_at light accent)"
restore_baseline
"$TAKO_BIN" theme reset-colors --target light > "$TMP/out.txt" 2>&1
rc=$?
keep_case known-reset-all-light "$TMP/out.txt" "$rc"
check_eq "③ reset-colors --target light は light の表を消す（dark は残る）" "0 __NONE__" \
  "$rc $(settings_at table light)"
check_eq "③ reset-colors --target light の後の dark の表" '{"accent": "#111111", "green": "#00ff00"}' \
  "$(settings_at table dark)"
restore_baseline
MCP="$(mcp_call tako_theme '{"action":"set-color","key":"accent","value":"#0a0b0c","target":"light"}')"
check_eq "③ MCP set-color target=light は light の表へ" "is_error=False #0a0b0c #111111" \
  "$MCP $(settings_at light accent) $(settings_at dark accent)"

echo "== ④ target 省略（従来どおり。修正前後の突き合わせは TAKO_2331_KEEP） =="
restore_baseline
"$TAKO_BIN" theme color red '#222222' > "$TMP/out.txt" 2>&1
rc=$?
keep_case omit-set-dark "$TMP/out.txt" "$rc"
check_eq "④ theme=dark の color red は dark の表へ" "0 #222222" "$rc $(settings_at dark red)"
"$TAKO_BIN" theme color red --reset > "$TMP/out.txt" 2>&1
rc=$?
keep_case omit-reset-dark "$TMP/out.txt" "$rc"
check_eq "④ theme=dark の color red --reset は dark から消す" "0 __NONE__" "$rc $(settings_at dark red)"
"$TAKO_BIN" theme reset-colors > "$TMP/out.txt" 2>&1
rc=$?
keep_case omit-reset-all-dark "$TMP/out.txt" "$rc"
check_eq "④ theme=dark の reset-colors は dark の表を消す（light は残る）" "0 __NONE__ #ff0000" \
  "$rc $(settings_at table dark) $(settings_at light accent)"
# settings.json の theme が知らない名前（手で書いた・プリセットを消した）: GUI のモード（ダーク）の表へ。
# target を省いたときのこの落とし方は #2331 で変えない
restore_baseline
edit_settings theme solarized
"$TAKO_BIN" theme color red '#333333' > "$TMP/out.txt" 2>&1
rc=$?
keep_case omit-set-unknown-theme "$TMP/out.txt" "$rc"
check_eq "④ theme=solarized（知らない名前）の color red は GUI のモードの表（dark）へ" "0 #333333 __NONE__" \
  "$rc $(settings_at dark red) $(settings_at light red)"
restore_baseline
edit_settings theme ocean
"$TAKO_BIN" theme color red '#444444' > "$TMP/out.txt" 2>&1
rc=$?
keep_case omit-set-preset "$TMP/out.txt" "$rc"
check_eq "④ theme=ocean の color red はプリセットへ" "0 #444444 __NONE__" \
  "$rc $(settings_at preset ocean red) $(settings_at dark red)"

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
