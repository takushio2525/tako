#!/usr/bin/env bash
# test-editor-font-1772.sh — ⌘+ / ⌘- / ⌘0 がコードプレビュー（エディタ）の本文に効くかの実 GUI テスト（#1772）
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、次の 2 部を実測する。
#
#   ① visual-test 節 `editor-font` を**同じバイナリで 2 腕**走らせる:
#        新しい腕 = 11 相すべて緑（TAKO_VISUAL_TEST_OK）
#        旧挙動の腕（TAKO_1772_LEGACY=1 = 本文がテーマ既定の文字サイズのまま）= 名指しで FAILED
#      節が見るのは、⌘+ × 3 / ⌘- / ⌘0 / 最小 8pt・最大 32pt で、描いた行の高さ・字送りの幅・
#      実ピクセル（1 行を選んだ絵と選んでいない絵の差 = 選択の帯）が文字サイズの比で動くこと、
#      可視行数が実矩形と一致し ↓ の追従・Page Down / Up・クリック位置が正しいこと、
#      折り返し・md のレンダリング表示・別のペインだけの拡大（ペイン単位）・メニュー経路・
#      10 万行の末尾付近。
#   ② 実 CLI（`tako menu invoke`）と MCP（`tako_menu` / `tako_preview_cursor`）で拡大・既定へ
#      戻すと、編集系の応答の `viewport.visible_lines` が**同じ値で**動く（開発不変条件）。
#      旧挙動の腕では拡大しても動かない = before。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-editor-font-1772.sh
#   （修正前のビルドと比べるときは `APP_BIN=<そのビルドの tako-app>` を付ける。②だけが走る:
#    修正前のビルドには節 `editor-font` が無いので①は「節が無い」で落ちる）
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

TMP="$(mktemp -d /tmp/tako-1772-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1772-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

if [ -z "${APP_BIN:-}" ]; then
  echo "visual-test 版の tako-app をビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
fi
isolated_gui_bins || exit 1

# 腕ごとに隔離の置き場を分ける（前の腕の data / 接続情報を次の腕が拾わない）
use_arm() {
  local arm="$1"
  export HOME="$TMP/$arm/home"
  export TAKO_ISOLATED=1
  export TAKO_DATA_DIR="$TMP/$arm/data"
  export TAKO_DISCOVERY_DIR="$TMP/$arm/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$arm/orch"
  export TAKO_SESSIONS_FILE="$TMP/$arm/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$arm/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$arm/workers.yaml"
  export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
  mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  local d
  for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
    case "$d" in
      "$TMP"/*) : ;;
      *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
    esac
  done
}

# 窓の寸法を固定する（器の高さが実行ごとに変わると可視行数の実測値が比べられない）
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 400 秒）
run_section() { # run_section <腕の名前> <ログ> [VAR=VAL …]
  local arm="$1" log="$2" rc=0 pid _
  shift 2
  use_arm "$arm"
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=editor-font ${1+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "隔離 GUI を立てられない（終了コード ${rc}。4 = 面を用意できない = 未実測）"
    exit "$rc"
  fi
  pid="$ISOLATED_GUI_PID"
  for _ in $(seq 1 4000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
}

# ①だけを飛ばす逃げ道（②の実 CLI / MCP だけを見直すとき。TAKO_1772_SKIP_SECTION=1）
NEW_LOG="$TMP/new.log"
OLD_LOG="$TMP/legacy.log"
if [ "${TAKO_1772_SKIP_SECTION:-}" != 1 ]; then
echo "== ① visual-test 節 editor-font（新しい腕） =="
run_section new "$NEW_LOG"
grep -E "TAKO_VISUAL_PIXEL: editor-font (arm|base|zoom-in |pixel|zoom-out|zoom-reset|font-m..|wrapped|markdown|other-pane|menu-|big-|.*walk|NG|ok)" "$NEW_LOG" \
  | sed 's/^/    /'
if grep -q "未知の節" "$NEW_LOG"; then
  fail "visual-test 付きでビルドされていない / 節 editor-font が無い"
elif grep -q "TAKO_VISUAL_TEST_OK" "$NEW_LOG"; then
  pass "新しい腕: 11 相すべて緑（TAKO_VISUAL_TEST_OK）"
else
  fail "新しい腕: 節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$NEW_LOG" | tail -5 | sed 's/^/    /'
fi

echo
echo "== ① visual-test 節 editor-font（旧挙動の腕 TAKO_1772_LEGACY=1） =="
run_section legacy "$OLD_LOG" TAKO_1772_LEGACY=1
grep -E "TAKO_VISUAL_PIXEL: editor-font (arm|base|zoom-in |pixel|markdown|NG)" "$OLD_LOG" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test editor-font (#1772)" "$OLD_LOG"; then
  pass "旧挙動の腕: 本文が文字サイズを読まないことを名指しで検出（FAILED）"
else
  fail "旧挙動の腕が落ちない（節に検出力が無い）"
  tail -5 "$OLD_LOG" | sed 's/^/    /'
fi
for needle in "⌘+: 行の高さ" "⌘+: 字送りの幅" "実ピクセル: 選択の帯の幅" "md: ⌘+ で見出し"; do
  if grep -q "editor-font NG ${needle}" "$OLD_LOG"; then
    pass "旧挙動の腕で「${needle}」が落ちる"
  else
    fail "旧挙動の腕で「${needle}」が落ちない"
  fi
done
fi

# ---------------------------------------------------------------------------
# ② 実 CLI / MCP
# ---------------------------------------------------------------------------
FIX="$TMP/fixtures"
mkdir -p "$FIX"
python3 - "$FIX" <<'PY'
import pathlib, sys
fix = pathlib.Path(sys.argv[1])
(fix / "long.rs").write_text("".join(f"    let row_{i:03} = {i};\n" for i in range(400)), encoding="utf-8")
PY

field_visible() { # 編集系の応答 JSON から viewport.visible_lines を採る
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(""); sys.exit(0)
print((d.get("viewport") or {}).get("visible_lines", ""))'
}
cli_visible() { "$TAKO_BIN" edit cursor 1:0 --pane "$PREVIEW" 2>/dev/null | field_visible; }
# メニュー経路は非同期（render → defer → dispatch_action）なので、値が動くまで状態で待つ
wait_visible_change() { # wait_visible_change <前の値> → 動いた後の値（動かなければ前の値）
  local before="$1" now _
  now="$before"
  for _ in $(seq 1 50); do
    now="$(cli_visible)"
    [ -n "$now" ] && [ "$now" != "$before" ] && break
    sleep 0.1
  done
  printf '%s' "$now"
}
mcp_call() { # mcp_call <出力> <tools/call の JSON 行…>
  local out="$1" sock tok
  shift
  : > "$TMP/mcp-in.jsonl"
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1772","version":"0"}}}' >> "$TMP/mcp-in.jsonl"
  printf '%s\n' "$@" >> "$TMP/mcp-in.jsonl"
  # `mcp serve` はツール公開の判定を **env だけ**で行う（tako の外で 0 ツール = FR-2.3.2）。
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  sock="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
  tok="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
  env TAKO_SOCKET="$sock" TAKO_TOKEN="$tok" "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$out" 2> "$TMP/mcp-err.log"
}
mcp_text() { # mcp_text <出力> <id> → その id の応答本文
  python3 - "$1" "$2" <<'PY'
import json, sys
want = int(sys.argv[2])
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != want:
        continue
    content = (msg.get("result") or {}).get("content") or []
    print(content[0].get("text") if content else "__NO_CONTENT__")
    sys.exit(0)
print("__NO_RESPONSE__")
PY
}

cli_arm() { # cli_arm <腕の名前> [VAR=VAL …]
  local arm="$1" root first zin zreset v0 v1 v2 m1 v3
  shift
  use_arm "$arm"
  echo
  echo "== ② 実 CLI / MCP（${arm}） =="
  launch_isolated_gui "$TMP/$arm.app.log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/$arm.app.log" || exit 1
  root="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
  first="$("$TAKO_BIN" open "$FIX/long.rs" --pane "$root" --right 2>&1)"
  PREVIEW="$(printf '%s' "$first" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
  [ -n "$PREVIEW" ] || { echo "検証用プレビューペインを作れない: $first"; exit 1; }
  "$TAKO_BIN" edit start --pane "$PREVIEW" >/dev/null 2>&1
  "$TAKO_BIN" focus "$PREVIEW" >/dev/null 2>&1
  # メニューの項目名は表示言語で変わるので、アクション名から引く（1 行 1 項目）
  "$TAKO_BIN" menu list > "$TMP/$arm.menu.json" 2>/dev/null
  pick_menu() { # pick_menu <アクション名> → 「メニュー名/項目名」
    python3 - "$TMP/$arm.menu.json" "$1" <<'PY2'
import json, sys
d = json.load(open(sys.argv[1]))
def walk(items, prefix):
    for it in items or []:
        label = it.get("label") or it.get("name") or ""
        path = prefix + "/" + label if prefix else label
        if it.get("action") == sys.argv[2]:
            print(path)
            sys.exit(0)
        walk(it.get("items"), path)
for m in d.get("menus", []):
    walk(m.get("items"), m.get("name", ""))
PY2
  }
  zin="$(pick_menu tako::ZoomIn)"
  zreset="$(pick_menu tako::ResetZoom)"
  echo "  preview pane=$PREVIEW zoom_in='$zin' reset='$zreset'"
  if [ -z "$zin" ] || [ -z "$zreset" ]; then
    fail "${arm}: メニューに「文字を大きく」「文字サイズを戻す」が見つからない"
    stop_isolated_gui "$APP_PID"; APP_PID=""
    return
  fi
  v0="$(cli_visible)"
  "$TAKO_BIN" menu invoke "$zin" >/dev/null && "$TAKO_BIN" menu invoke "$zin" >/dev/null && "$TAKO_BIN" menu invoke "$zin" >/dev/null
  v1="$(wait_visible_change "$v0")"
  # 3 回目まで効き切るのを待つ（1 回ずつ値が動くので、止まるまで見る）
  for _ in $(seq 1 10); do
    sleep 0.2
    v3="$(cli_visible)"
    [ "$v3" = "$v1" ] && break
    v1="$v3"
  done
  "$TAKO_BIN" menu invoke "$zreset" >/dev/null
  v2="$(wait_visible_change "$v1")"
  echo "  CLI: visible_lines 既定=$v0 → menu invoke 拡大 ×3=$v1 → 戻す=$v2"
  # MCP: 同じ 3 回の拡大（tako_menu）→ 可視行数（tako_preview_cursor）
  mcp_call "$TMP/$arm.mcp1.jsonl" \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_menu\",\"arguments\":{\"action\":\"invoke\",\"path\":\"$zin\"}}}" \
    "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_menu\",\"arguments\":{\"action\":\"invoke\",\"path\":\"$zin\"}}}" \
    "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_menu\",\"arguments\":{\"action\":\"invoke\",\"path\":\"$zin\"}}}"
  echo "  MCP tako_menu invoke: $(mcp_text "$TMP/$arm.mcp1.jsonl" 2 | tr '\n' ' ')"
  wait_visible_change "$v2" >/dev/null
  sleep 1
  mcp_call "$TMP/$arm.mcp2.jsonl" \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_cursor\",\"arguments\":{\"pane\":$PREVIEW,\"line\":1,\"col\":0}}}" \
    "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_menu\",\"arguments\":{\"action\":\"invoke\",\"path\":\"$zreset\"}}}"
  m1="$(mcp_text "$TMP/$arm.mcp2.jsonl" 2 | field_visible)"
  echo "  MCP: tako_menu 拡大 ×3 → tako_preview_cursor の visible_lines=$m1"
  if [ "$arm" = cli-new ]; then
    if [ -n "$v0" ] && [ -n "$v1" ] && [ "$v1" -lt "$v0" ]; then
      pass "CLI の拡大で可視行数が減る（$v0 → $v1 行）"
    else
      fail "CLI の拡大で可視行数が減らない（$v0 → ${v1}）"
    fi
    if [ "$v2" = "$v0" ]; then pass "CLI の「文字サイズを戻す」で既定の $v0 行へ戻る"; else fail "CLI で戻らない（$v2 / 既定 ${v0}）"; fi
    if [ -n "$m1" ] && [ "$m1" = "$v1" ]; then
      pass "MCP の拡大 ×3 は CLI と同じ可視行数（$m1 行）"
    else
      fail "MCP の拡大 ×3 が CLI と違う（MCP $m1 / CLI ${v1}）"
    fi
    NEW_V0="$v0"; NEW_V1="$v1"
  else
    if [ -n "$v0" ] && [ "$v1" = "$v0" ]; then
      pass "旧挙動の腕: 拡大しても可視行数が変わらない（$v0 → $v1 行 = #1772 の症状）"
    else
      fail "旧挙動の腕で可視行数が動いた（$v0 → ${v1}）"
    fi
    if [ -n "${NEW_V0:-}" ] && [ "$v0" = "$NEW_V0" ]; then
      pass "既定の文字サイズでは新旧の可視行数が同じ（$v0 行 = 壊れていない側は動かさない）"
    else
      fail "既定の文字サイズで新旧の可視行数が違う（旧 $v0 / 新 ${NEW_V0:-?}）"
    fi
  fi
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

NEW_V0=""
NEW_V1=""
cli_arm cli-new
cli_arm cli-legacy TAKO_1772_LEGACY=1

# ログは終了時に `$TMP` ごと消える。残すなら TAKO_1772_KEEP=1（置き場は $TMPDIR）
if [ "${TAKO_1772_KEEP:-}" = 1 ] && [ -f "$NEW_LOG" ]; then
  /bin/cp -f "$NEW_LOG" "${TMPDIR:-/tmp}/tako-1772-new.log"
  /bin/cp -f "$OLD_LOG" "${TMPDIR:-/tmp}/tako-1772-legacy.log"
  echo "ログを ${TMPDIR:-/tmp}/tako-1772-{new,legacy}.log へ残した"
fi
echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
