#!/usr/bin/env bash
# test-md-find-restore-1873.sh — 閲覧中の ⌘F 検索を閉じたら描画へ戻す実経路テスト（#1873）
#
# 隔離した data / tmux で**実 tako-app** を立て、CLI / MCP から次を確かめる。
#
#   ① 描画表示の md で検索欄を開き（`tako edit search --open` = GUI の ⌘F）、クエリを入れると
#      表示がエディタの行（code）へ落ちる（ヒットはエディタの行の上に描く = 意図どおり）。
#      閉じる（`--close` = GUI の Escape / ⌘F のトグル）と markdown へ戻り目次が作られる
#      （Issue の本体: 閉じても code のまま・目次は空のまま）
#   ② MCP `tako_preview_search` の `visible` が CLI と同じ dispatch を通る（応答の字面一致）
#   ③ エッジ: 未保存の編集中に開いて閉じても編集（code・未保存）を保つ / 編集を抜けた
#      未保存のセッションは閉じたら描画（本文から）へ戻る
#   ④ エッジ: コードのファイルは今までどおり（閉じても code）/ PDF は検索欄を開けない
#      （理由つきで断る・表示は pdf のまま）
#   ⑤ エッジ: 大きい md（99,000 行）は閉じた直後から markdown・描き直しは background で、
#      その間も GUI は応答する
#   ⑥ A/B: `TAKO_1873_LEGACY=1`（#1873 前の挙動）の GUI では ① が落ちる = 検出力がある
#   ⑦ 画面: visual-test 節 `md-find-restore`（⌘F / Enter / Escape を GUI のキー配送へ流す）が
#      通り、`TAKO_1873_LEGACY=1` では「描画（Markdown）へ戻り」で落ちる
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-md-find-restore-1873.sh
#   DUMP_DIR を渡すと ⑦ のフレーム（PNG）をそこへ残す。**フレームにはその機のシェルの
#   プロンプトが写るので、リポジトリ・PR・Issue へ貼らない**
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる前提の実経路テスト。
# 終了コード: 0 = 全部通った / 1 = どれか落ちた / 4 = 仮想ディスプレイを用意できず未実測
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

TMP="$(mktemp -d /tmp/tako-1873-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1873-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
echo "visual-test 入りの tako-app / tako をビルドします…"
(cd "$REPO_ROOT" && cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
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

# 応答 JSON から 1 つの値を取る（`search.visible` のように辿れる）
jget() {
  python3 -c '
import json, sys
path = sys.argv[1].split(".")
try:
    cur = json.loads(sys.stdin.read())
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for key in path:
    if isinstance(cur, dict) and key in cur:
        cur = cur[key]
    else:
        print("__MISSING__"); sys.exit(0)
print(cur if isinstance(cur, str) else json.dumps(cur, ensure_ascii=False, sort_keys=True))
' "$1"
}

# `tako` を叩いて stdout と stderr をまとめて OUT へ、終了コードを RC へ置く。
# **`$( )` の中で呼ばない**（サブシェルで立てた RC は親へ戻らない）
OUT=""
RC=0
run() {
  OUT="$("$TAKO_BIN" "$@" 2>&1)"
  RC=$?
}

# ペインの表示モード（`tako list` の preview.mode。プレビューでなければ none）
pane_mode() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
want = int(sys.argv[1])
def walk(node):
    if isinstance(node, dict):
        if node.get("id") == want and isinstance(node.get("preview"), dict):
            print(node["preview"]["mode"]); sys.exit(0)
        for v in node.values():
            walk(v)
    elif isinstance(node, list):
        for v in node:
            walk(v)
try:
    walk(json.load(sys.stdin))
except SystemExit:
    raise
except Exception:
    pass
print("none")
' "$1"
}

# 目次のタイトルを `|` でつないだもの（`tako preview-outline`。断られたら ERR）
outline_titles() {
  "$TAKO_BIN" preview-outline --pane "$1" 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("ERR"); sys.exit(0)
print("|".join(i["title"] for i in d.get("outline", [])))
'
}

# 表示モードと目次が期待どおりになるまで待つ（上限 $4 秒。既定 10）。待った ms を WAITED へ
WAITED=0
wait_md() { # ペイン 期待モード 期待タイトル [上限秒]
  local pane="$1" mode="$2" titles="$3" limit="${4:-10}" t0 now
  t0=$(python3 -c 'import time; print(time.time())')
  while :; do
    if [ "$(pane_mode "$pane")" = "$mode" ] && [ "$(outline_titles "$pane")" = "$titles" ]; then
      now=$(python3 -c 'import time; print(time.time())')
      WAITED=$(python3 -c "print(int((${now} - ${t0}) * 1000))")
      return 0
    fi
    now=$(python3 -c 'import time; print(time.time())')
    if python3 -c "import sys; sys.exit(0 if (${now} - ${t0}) > ${limit} else 1)"; then
      WAITED=$(python3 -c "print(int((${now} - ${t0}) * 1000))")
      return 1
    fi
    sleep 0.1
  done
}

start_gui() { # ログ 追加の env…
  local log="$1" rc=0
  shift
  launch_isolated_gui "$log" ${1+"$@"} || rc=$?
  if [ "$rc" -eq 4 ]; then
    echo "仮想ディスプレイを用意できないので未実測で止める（${ISOLATED_GUI_NO_DISPLAY_REASON:-理由不明}）"
    exit 4
  fi
  [ "$rc" -eq 0 ] || exit "$rc"
  wait_isolated_gui "$log" || exit 1
}

root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])
'
}

# 新しいタブで開いてペイン ID を返す
open_tab() { # ファイル [追加の引数…]
  local file="$1" pane
  shift
  pane="$("$TAKO_BIN" open "$file" --pane "$ROOT_PANE" --new-tab ${1+"$@"} 2>&1 | jget pane)"
  case "$pane" in
    ''|*[!0-9]*) echo "開けない: $file" >&2; return 1 ;;
  esac
  printf '%s' "$pane"
}

# 目次 3 件の素材（needle は Beta の節にだけある）
write_note() {
  printf '# Title\n\nintro\n\n## Alpha\n\nbody a\n\n## Beta\n\nneedle b\n' > "$1"
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
echo "  pid=$APP_PID"
ROOT_PANE="$(root_pane)"

# ① の本体（⑥ の A/B でも同じ手順を通す）。結果は CLOSED_* へ
basic_find() { # ファイル
  local file="$1" pane
  write_note "$file"
  pane="$(open_tab "$file")" || return 1
  wait_md "$pane" markdown "Title|Alpha|Beta" || true
  BASIC_PANE="$pane"
  BASIC_OPENED="$(pane_mode "$pane")/$(outline_titles "$pane")"
  run edit search --open --pane "$pane"
  OPEN_VISIBLE="$(printf '%s' "$OUT" | jget search.visible)"
  OPEN_MODE="$(pane_mode "$pane")"
  run edit search needle --pane "$pane"
  SEARCH_TOTAL="$(printf '%s' "$OUT" | jget search.total)"
  SEARCH_MODE="$(pane_mode "$pane")"
  run edit search --close --pane "$pane"
  CLOSE_RC="$RC"
  CLOSE_OUT="$OUT"
  CLOSED_VISIBLE="$(printf '%s' "$OUT" | jget search.visible)"
  # 小さい文書はその場で描き直すので、待たずに読む
  CLOSED_MODE="$(pane_mode "$pane")"
  # 旧挙動では目次が断られて非 0 になる（それも観測値）
  CLOSED_TITLES="$(outline_titles "$pane")"
  return 0
}

echo
echo "== ① 検索欄を開いて探し、閉じると描画へ戻り目次が作られる =="
basic_find "$TMP/one.md" || exit 1
P1="$BASIC_PANE"
check_eq "描画表示で開いて目次が 3 件" "markdown/Title|Alpha|Beta" "$BASIC_OPENED"
check_eq "--open で検索欄が開く（search.visible=true）" true "$OPEN_VISIBLE"
check_eq "開いただけでは描画のまま" markdown "$OPEN_MODE"
check_eq "クエリのヒットは 1 件" 1 "$SEARCH_TOTAL"
check_eq "検索欄を開いて探すとエディタの行（code）の上に描く" code "$SEARCH_MODE"
check_eq "--close が通る" 0 "$CLOSE_RC"
check_eq "--close で検索欄が閉じる（search.visible=false）" false "$CLOSED_VISIBLE"
check_eq "閉じた応答の mode は markdown" markdown "$(printf '%s' "$CLOSE_OUT" | jget mode)"
check_eq "閉じた直後（待たずに）表示モードが markdown へ戻る" markdown "$CLOSED_MODE"
check_eq "閉じた後の目次が作られる" "Title|Alpha|Beta" "$CLOSED_TITLES"
run edit status --pane "$P1"
check_eq "閉じた後は編集中ではない" false "$(printf '%s' "$OUT" | jget editing)"
# 検索欄を閉じたまま探す（従来の CLI の検索）は表示を変えない
run edit search needle --pane "$P1"
check_eq "閉じたまま探しても描画のまま（従来どおり）" markdown "$(pane_mode "$P1")"
check_eq "閉じたまま探した応答の search.visible は false" false "$(printf '%s' "$OUT" | jget search.visible)"
# 開いて探して、もう一度 --open（冪等）→ --close
run edit search --open --pane "$P1"
run edit search --open --pane "$P1"
check_eq "--open を 2 回送っても開いたまま（トグルではない）" true "$(printf '%s' "$OUT" | jget search.visible)"
run edit search --close --pane "$P1"
run edit search --close --pane "$P1"
check_eq "--close を 2 回送っても閉じたまま" "0/false/markdown" \
  "$RC/$(printf '%s' "$OUT" | jget search.visible)/$(pane_mode "$P1")"
run edit search --open --close --pane "$P1"
check_eq "--open と --close は同時に渡せない（終了コード非 0）" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"

echo
echo "== ② MCP tako_preview_search の visible（CLI と同じ dispatch）=="
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
mcp_call() { # ツール名 引数の JSON → 結果の本文（isError なら先頭に ERROR:）
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1873","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$SOCK_PATH" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" \
      "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c '
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    if "error" in msg:
        print("ERROR:" + json.dumps(msg["error"], ensure_ascii=False)); break
    res = msg.get("result", {})
    text = res.get("content", [{}])[0].get("text", "")
    print(("ERROR:" if res.get("isError") else "") + text)
'
}
# JSON を正規化（キー順・空白）して比べる
norm() { python3 -c 'import json,sys; print(json.dumps(json.loads(sys.stdin.read()), sort_keys=True, ensure_ascii=False))' 2>/dev/null || echo "__NOT_JSON__"; }
F2="$TMP/two.md"
write_note "$F2"
P2="$(open_tab "$F2")" || exit 1
wait_md "$P2" markdown "Title|Alpha|Beta" || true
M_OPEN="$(mcp_call tako_preview_search "{\"pane\":$P2,\"visible\":true}")"
check_eq "MCP の visible=true で検索欄が開く" true "$(printf '%s' "$M_OPEN" | jget search.visible)"
M_FIND="$(mcp_call tako_preview_search "{\"pane\":$P2,\"query\":\"needle\"}")"
check_eq "MCP で開いて探すと code" "1/code" \
  "$(printf '%s' "$M_FIND" | jget search.total)/$(pane_mode "$P2")"
M_CLOSE="$(mcp_call tako_preview_search "{\"pane\":$P2,\"visible\":false}")"
check_eq "MCP の visible=false で閉じると markdown へ戻る" "false/markdown/Title|Alpha|Beta" \
  "$(printf '%s' "$M_CLOSE" | jget search.visible)/$(pane_mode "$P2")/$(outline_titles "$P2")"
# 字面一致: 同じ状態から同じ操作を CLI と MCP で送り、応答 JSON が一致する
for args in "--open" "--close"; do
  case "$args" in
    --open) mcp_args="{\"pane\":$P2,\"visible\":true}" ;;
    *) mcp_args="{\"pane\":$P2,\"visible\":false}" ;;
  esac
  run edit search "$args" --pane "$P2"
  CLI_JSON="$(printf '%s' "$OUT" | norm)"
  # CLI で変えた状態を一度戻してから MCP で同じ操作を送る
  if [ "$args" = "--open" ]; then "$TAKO_BIN" edit search --close --pane "$P2" >/dev/null 2>&1; else "$TAKO_BIN" edit search --open --pane "$P2" >/dev/null 2>&1; fi
  MCP_JSON="$(mcp_call tako_preview_search "$mcp_args" | norm)"
  check_eq "CLI と MCP の応答が字面一致（${args}）" "$CLI_JSON" "$MCP_JSON"
done

echo
echo "== ③ 未保存の編集中・編集を抜けた未保存のセッション =="
F3="$TMP/three.md"
write_note "$F3"
P3="$(open_tab "$F3")" || exit 1
wait_md "$P3" markdown "Title|Alpha|Beta" || true
"$TAKO_BIN" edit start --pane "$P3" >/dev/null
"$TAKO_BIN" edit autosave false --pane "$P3" >/dev/null
"$TAKO_BIN" edit replace-range 1:2 1:7 Edited --pane "$P3" >/dev/null
"$TAKO_BIN" edit search --open --pane "$P3" >/dev/null
"$TAKO_BIN" edit search needle --pane "$P3" >/dev/null
run edit search --close --pane "$P3"
check_eq "編集中に閉じても code のまま（編集を保つ）" code "$(pane_mode "$P3")"
run edit status --pane "$P3"
check_eq "編集中・未保存のまま" "true/true" \
  "$(printf '%s' "$OUT" | jget editing)/$(printf '%s' "$OUT" | jget dirty)"
"$TAKO_BIN" edit stop --pane "$P3" >/dev/null
check_eq "未保存のまま編集を抜けると描画へ（#1661 の経路）" markdown "$(pane_mode "$P3")"
"$TAKO_BIN" edit search --open --pane "$P3" >/dev/null
"$TAKO_BIN" edit search needle --pane "$P3" >/dev/null
check_eq "未保存のセッションでも検索中は code" code "$(pane_mode "$P3")"
"$TAKO_BIN" edit search --close --pane "$P3" >/dev/null
check_eq "閉じたら描画へ戻り、目次は未保存の本文から" "markdown/Edited|Alpha|Beta" \
  "$(pane_mode "$P3")/$(outline_titles "$P3")"
run edit status --pane "$P3"
check_eq "未保存の変更は残る（dirty=true）・ディスクは書き換えない" "true/# Title" \
  "$(printf '%s' "$OUT" | jget dirty)/$(sed -n 1p "$F3")"

echo
echo "== ④ コードのファイル / PDF =="
F4="$TMP/four.rs"
printf 'fn main() {\n    let needle = 1;\n}\n' > "$F4"
P4="$(open_tab "$F4")" || exit 1
check_eq "コードは code で開く" code "$(pane_mode "$P4")"
"$TAKO_BIN" edit search --open --pane "$P4" >/dev/null
run edit search needle --pane "$P4"
check_eq "コードでも検索が当たる" 1 "$(printf '%s' "$OUT" | jget search.total)"
run edit search --close --pane "$P4"
check_eq "コードは閉じても code のまま（従来どおり）" "false/code" \
  "$(printf '%s' "$OUT" | jget search.visible)/$(pane_mode "$P4")"
F4P="$TMP/four.pdf"
python3 - "$F4P" <<'PY'
import sys
# 1 ページの最小 PDF（xref のオフセットは組み立てながら数える）
objs = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
]
out = bytearray(b"%PDF-1.4\n")
offsets = []
for i, body in enumerate(objs, 1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for off in offsets:
    out += b"%010d 00000 n \n" % off
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
open(sys.argv[1], "wb").write(bytes(out))
PY
P4P="$(open_tab "$F4P")" || exit 1
check_eq "PDF は pdf で開く" pdf "$(pane_mode "$P4P")"
run edit search --open --pane "$P4P"
check_eq "PDF は検索欄を開けない（終了コード非 0）" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
case "$OUT" in
  *テキスト以外*) pass "PDF で断る理由が出る（${OUT}）" ;;
  *) fail "PDF で断る理由が出ない（${OUT}）" ;;
esac
check_eq "PDF の表示は変わらない" pdf "$(pane_mode "$P4P")"

echo
echo "== ⑤ 大きい md（99,000 行）=="
F5="$TMP/big.md"
python3 -c '
import sys
with open(sys.argv[1], "w", encoding="utf-8") as f:
    for i in range(99000):
        if i % 1000 == 0:
            f.write("## Section %03d\n" % (i // 1000))
        elif i == 50500:
            f.write("the needle line\n")
        else:
            f.write("line %05d of the big markdown body\n" % i)
' "$F5"
BIG_TITLES="$(python3 -c 'print("|".join("Section %03d" % i for i in range(99)))')"
echo "  $(wc -l < "$F5" | tr -d ' ') 行 / $(wc -c < "$F5" | tr -d ' ') バイト"
P5="$(open_tab "$F5")" || exit 1
wait_md "$P5" markdown "$BIG_TITLES" 60 || true
check_eq "大きい md を描画表示で開いて目次が 99 件" 99 \
  "$(outline_titles "$P5" | awk -F'|' '{print NF}')"
"$TAKO_BIN" edit search --open --pane "$P5" >/dev/null
run edit search "the needle" --pane "$P5"
check_eq "大きい md でも検索が当たり code へ落ちる" "1/code" \
  "$(printf '%s' "$OUT" | jget search.total)/$(pane_mode "$P5")"
T0=$(python3 -c 'import time; print(time.time())')
run edit search --close --pane "$P5"
T1=$(python3 -c 'import time; print(time.time())')
CLOSE_MS=$(python3 -c "print(int((${T1} - ${T0}) * 1000))")
T0=$(python3 -c 'import time; print(time.time())')
MODE_RIGHT_AFTER="$(pane_mode "$P5")"
T1=$(python3 -c 'import time; print(time.time())')
LIST_MS=$(python3 -c "print(int((${T1} - ${T0}) * 1000))")
echo "  --close の応答 ${CLOSE_MS} ms / 直後の tako list ${LIST_MS} ms（mode=${MODE_RIGHT_AFTER}）"
check_eq "閉じた直後から表示モードは markdown（描き直しは background）" markdown "$MODE_RIGHT_AFTER"
check_eq "background で描き直している間も GUI は応答する（tako list が 1 秒以内）" 1 \
  "$([ "$LIST_MS" -lt 1000 ] && echo 1 || echo 0)"
if wait_md "$P5" markdown "$BIG_TITLES" 60; then
  pass "大きい md も目次が作り直される（待った ${WAITED} ms）"
else
  fail "大きい md の目次が作り直されない（${WAITED} ms 待った）"
fi
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ⑥ A/B: TAKO_1873_LEGACY=1（#1873 前の挙動）では ① が落ちる =="
rm -rf "$TAKO_DATA_DIR"
mkdir -p "$TAKO_DATA_DIR"
start_gui "$TMP/app-legacy.log" TAKO_1873_LEGACY=1
APP_PID="$ISOLATED_GUI_PID"
ROOT_PANE="$(root_pane)"
basic_find "$TMP/legacy.md" || exit 1
echo "  legacy: 閉じた直後の mode=${CLOSED_MODE} titles='${CLOSED_TITLES}'（新: markdown / Title|Alpha|Beta）"
check_eq "legacy では閉じても code のまま（① の検査が #1873 前を検出できる）" code "$CLOSED_MODE"
check_eq "legacy では目次が空のまま（Markdown・PDF ではないと断られる）" ERR "$CLOSED_TITLES"
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ⑦ 画面: visual-test 節 md-find-restore =="
DUMP="${DUMP_DIR:-$TMP/frames}"
mkdir -p "$DUMP"
run_visual() { # ログ フレームの置き場 追加の env…
  local log="$1" frames="$2" pid i rc=0
  shift 2
  rm -rf "$TAKO_DATA_DIR"
  mkdir -p "$TAKO_DATA_DIR" "$frames"
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=md-find-restore \
    TAKO_VISUAL_DUMP_DIR="$frames" ${1+"$@"} || rc=$?
  if [ "$rc" -eq 4 ]; then
    echo "  仮想ディスプレイを用意できないので画面は未実測"
    return 4
  fi
  [ "$rc" -eq 0 ] || return "$rc"
  pid="$ISOLATED_GUI_PID"
  for i in $(seq 1 1200); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  grep -E "TAKO_VISUAL_1873|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$log" | sed 's/^/    /'
  grep -q "TAKO_VISUAL_TEST_OK" "$log"
}
if run_visual "$TMP/visual.log" "$DUMP/new"; then
  pass "visual-test md-find-restore が通る（⌘F / Enter / Escape を GUI のキー配送へ流す）"
else
  fail "visual-test md-find-restore が落ちた"
fi
if run_visual "$TMP/visual-legacy.log" "$DUMP/legacy" TAKO_1873_LEGACY=1; then
  fail "legacy でも visual-test md-find-restore が通ってしまう（検出力が無い）"
else
  case "$(grep TAKO_APP_SELF_TEST_FAILED "$TMP/visual-legacy.log")" in
    *"描画（Markdown）へ戻り"*) pass "legacy の visual 節は「描画（Markdown）へ戻り」で落ちる" ;;
    *) fail "legacy の visual 節が別の理由で落ちた: $(grep TAKO_APP_SELF_TEST_FAILED "$TMP/visual-legacy.log")" ;;
  esac
fi
echo "  フレーム: ${DUMP}（new / legacy）"
ls "$DUMP/new" "$DUMP/legacy" 2>/dev/null | sed 's/^/    /'

echo
echo "== 結果 =="
echo "  PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
