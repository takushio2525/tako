#!/usr/bin/env bash
# test-md-edit-resume-1661.sh — Markdown の編集を抜けたら描画へ戻す実経路テスト（#1661）
#
# 隔離した data / tmux で**実 tako-app** を立て、CLI / MCP から次を確かめる。
#
#   ① 描画表示の md を編集して抜けると、表示モードが markdown へ戻り目次が作り直される。
#      書き換えた見出しが抜けた後の目次に出る（Issue の本体: code のまま・目次は空のまま）
#   ② 編集中も目次が使える（target は source_line = 原文の行）。項目へ飛ぶとキャレットが
#      その見出しの行へ行く
#   ③ エッジ: 未保存のまま抜ける（描画と目次は本文から・ディスクは変えない）/ 抜けた後の
#      `tako edit save` と `tako edit reload` でも描画のまま（修正前はここでも code へ落ちた）
#   ④ エッジ: 外部変更の競合中（#1659）に抜ける / 読み直す
#   ⑤ エッジ: 大きい md（#1660 の上限 10 万行の手前 = 99,000 行）は background で描き直し、
#      その間も GUI は応答する
#   ⑥ エッジ: コードのファイル・コード表示で開いた md は今までどおり（抜けても code）
#   ⑦ MCP: `tako_preview_outline` / `tako_preview_edit` が CLI と同じ dispatch を通る
#   ⑧ A/B: `TAKO_1661_LEGACY=1`（#1661 前の挙動）の GUI では ① が落ちる = この検査に検出力がある
#   ⑨ 画面: visual-test 節 `md-edit-resume`（実マウスで 編集 / 目次 / 編集中 を押す）が通り、
#      `TAKO_1661_LEGACY=1` では「描画（Markdown）へ戻り」で落ちる。フレームは $DUMP_DIR へ残す
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-md-edit-resume-1661.sh
#   main など別のビルドで同じ検査を走らせるときは TAKO_BIN / APP_BIN でバイナリを差し替える
#   （その場合 ⑧ ⑨ は飛ばす = 旧ビルドには A/B の口も visual 節も無い）。
#   DUMP_DIR を渡すと ⑨ のフレーム（PNG）をそこへ残す。**フレームにはその機のシェルの
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

# 差し替えたバイナリ（main 等）で走らせているか。A/B の口と visual 節はこのビルドにしか無い
CUSTOM_BINS=0
if [ -n "${TAKO_BIN:-}" ] || [ -n "${APP_BIN:-}" ]; then
  CUSTOM_BINS=1
fi

TMP="$(mktemp -d /tmp/tako-1661-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1661-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
if [ "$CUSTOM_BINS" -eq 0 ]; then
  echo "visual-test 入りの tako-app / tako をビルドします…"
  (cd "$REPO_ROOT" && cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
fi
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

# 応答 JSON から 1 つの値を取る（`document.cursor.line` のように辿れる）
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

# 目次の target の kind と行（`source_line:5|...`。markdown_block は `markdown_block`）
outline_targets() {
  "$TAKO_BIN" preview-outline --pane "$1" 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("ERR"); sys.exit(0)
out = []
for i in d.get("outline", []):
    t = i["target"]
    out.append(t["kind"] + (":%d" % t["line"] if "line" in t else ""))
print("|".join(out))
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
  launch_isolated_gui "$log" "$@" || rc=$?
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
  pane="$("$TAKO_BIN" open "$file" --pane "$ROOT_PANE" --new-tab "$@" 2>&1 | jget pane)"
  case "$pane" in
    ''|*[!0-9]*) echo "開けない: $file" >&2; return 1 ;;
  esac
  printf '%s' "$pane"
}

# 目次 3 件の素材（Beta は 9 行目）
write_note() {
  printf '# Title\n\nintro\n\n## Alpha\n\nbody a\n\n## Beta\n\nbody b\n' > "$1"
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
echo "  pid=$APP_PID"
ROOT_PANE="$(root_pane)"

# ① の本体（⑧ の A/B でも同じ手順を通す）。結果は RESUME_MODE / RESUME_TITLES へ
basic_resume() { # ファイル
  local file="$1" pane
  write_note "$file"
  pane="$(open_tab "$file")" || return 1
  wait_md "$pane" markdown "Title|Alpha|Beta" || true
  BASIC_PANE="$pane"
  BASIC_OPENED="$(pane_mode "$pane")/$(outline_titles "$pane")"
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null
  BASIC_EDITING_MODE="$(pane_mode "$pane")"
  "$TAKO_BIN" edit replace-range 1:2 1:7 Renamed --pane "$pane" >/dev/null
  "$TAKO_BIN" edit save --pane "$pane" >/dev/null
  "$TAKO_BIN" edit stop --pane "$pane" >/dev/null
  # 小さい文書はその場で描き直すので、待たずに読む
  RESUME_MODE="$(pane_mode "$pane")"
  # 旧挙動では目次が断られて非 0 になる（それも観測値）ので、終了コードは手順の成否にしない
  RESUME_TITLES="$(outline_titles "$pane")"
  return 0
}

echo
echo "== ① 編集して抜けると描画へ戻り、目次が作り直される =="
F1="$TMP/one.md"
basic_resume "$F1" || exit 1
P1="$BASIC_PANE"
check_eq "描画表示で開いて目次が 3 件" "markdown/Title|Alpha|Beta" "$BASIC_OPENED"
check_eq "編集中の表示は code" code "$BASIC_EDITING_MODE"
check_eq "抜けた直後（待たずに）表示モードが markdown へ戻る" markdown "$RESUME_MODE"
check_eq "書き換えた見出しが抜けた後の目次に出る" "Renamed|Alpha|Beta" "$RESUME_TITLES"
run edit status --pane "$P1"
check_eq "抜けた後は編集中ではない" false "$(printf '%s' "$OUT" | jget editing)"

echo
echo "== ② 編集中も目次が使える（原文の行へ飛ぶ）=="
"$TAKO_BIN" edit start --pane "$P1" >/dev/null
check_eq "編集中の目次は原文の行（source_line）を指す" \
  "source_line:1|source_line:5|source_line:9" "$(outline_targets "$P1")"
check_eq "編集中の目次のタイトルは描画の目次と同じ" "Renamed|Alpha|Beta" "$(outline_titles "$P1")"
run preview-outline --pane "$P1" --item 3
check_eq "項目 3 へ飛ぶと selected は source_line:9" "source_line/9" \
  "$(printf '%s' "$OUT" | jget selected.kind)/$(printf '%s' "$OUT" | jget selected.line)"
run edit status --pane "$P1"
check_eq "飛んだ後のキャレットは 9 行目" 9 "$(printf '%s' "$OUT" | jget document.cursor.line)"
run preview-outline --pane "$P1" --item 9
check_eq "範囲外の項目は断る（終了コード非 0）" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
"$TAKO_BIN" edit stop --pane "$P1" >/dev/null
check_eq "抜けたら描画の目次（markdown_block）へ戻る" \
  "markdown_block|markdown_block|markdown_block" "$(outline_targets "$P1")"

echo
echo "== ③ 未保存のまま抜ける / 抜けた後の保存・読み直し =="
F3="$TMP/three.md"
write_note "$F3"
P3="$(open_tab "$F3")" || exit 1
wait_md "$P3" markdown "Title|Alpha|Beta" || true
"$TAKO_BIN" edit start --pane "$P3" >/dev/null
"$TAKO_BIN" edit autosave false --pane "$P3" >/dev/null
"$TAKO_BIN" edit replace-range 9:3 9:7 Unsaved --pane "$P3" >/dev/null
"$TAKO_BIN" edit stop --pane "$P3" >/dev/null
check_eq "未保存のまま抜けても markdown へ戻る" markdown "$(pane_mode "$P3")"
check_eq "未保存の見出しも目次に出る（本文から描く）" "Title|Alpha|Unsaved" "$(outline_titles "$P3")"
run edit status --pane "$P3"
check_eq "未保存の変更は残っている（dirty=true）" true "$(printf '%s' "$OUT" | jget dirty)"
check_eq "ディスクは書き換えない" "## Beta" "$(sed -n 9p "$F3")"
run edit save --pane "$P3"
check_eq "抜けた後の保存が通る" 0 "$RC"
check_eq "抜けた後に保存しても markdown のまま（修正前は code へ落ちた）" markdown "$(pane_mode "$P3")"
check_eq "保存した見出しがディスクにある" "## Unsaved" "$(sed -n 9p "$F3")"
# もう一度未保存の変更を作って抜け、読み直す（変更を捨てる）
"$TAKO_BIN" edit replace-range 9:3 9:10 Again --pane "$P3" >/dev/null
"$TAKO_BIN" edit stop --pane "$P3" >/dev/null
check_eq "2 回目の未保存も目次に出る" "Title|Alpha|Again" "$(outline_titles "$P3")"
run edit reload --pane "$P3"
check_eq "抜けた後の読み直しが通る" 0 "$RC"
check_eq "抜けた後に読み直しても markdown のまま（修正前は code へ落ちた）" markdown "$(pane_mode "$P3")"
check_eq "読み直した目次はディスクの見出し" "Title|Alpha|Unsaved" "$(outline_titles "$P3")"

echo
echo "== ④ 外部変更の競合中（#1659）に抜ける =="
F4="$TMP/four.md"
write_note "$F4"
P4="$(open_tab "$F4")" || exit 1
wait_md "$P4" markdown "Title|Alpha|Beta" || true
"$TAKO_BIN" edit start --pane "$P4" >/dev/null
"$TAKO_BIN" edit autosave false --pane "$P4" >/dev/null
"$TAKO_BIN" edit replace-range 5:3 5:8 Mine --pane "$P4" >/dev/null
printf '# Title\n\nintro\n\n## Disk\n\nbody a\n\n## Beta\n\nbody b\n' > "$F4"
CONFLICT=""
for i in $(seq 1 50); do
  CONFLICT="$("$TAKO_BIN" edit status --pane "$P4" 2>/dev/null | jget conflict.state)"
  [ "$CONFLICT" = changed ] && break
  sleep 0.1
done
check_eq "外部変更を競合として検知する" changed "$CONFLICT"
"$TAKO_BIN" edit stop --pane "$P4" >/dev/null
check_eq "競合中に抜けても markdown へ戻る" markdown "$(pane_mode "$P4")"
check_eq "競合中の描画は自分の本文から（Mine）" "Title|Mine|Beta" "$(outline_titles "$P4")"
run edit status --pane "$P4"
check_eq "抜けても競合は残る（conflict.state=changed）" changed "$(printf '%s' "$OUT" | jget conflict.state)"
run edit reload --pane "$P4"
check_eq "競合中の読み直しが通る" 0 "$RC"
check_eq "読み直した後も markdown のまま" markdown "$(pane_mode "$P4")"
check_eq "読み直した目次はディスクの見出し（Disk）" "Title|Disk|Beta" "$(outline_titles "$P4")"

echo
echo "== ⑤ 大きい md（99,000 行 = 上限 10 万行の手前）=="
F5="$TMP/big.md"
python3 -c '
import sys
with open(sys.argv[1], "w", encoding="utf-8") as f:
    for i in range(99000):
        if i % 1000 == 0:
            f.write("## Section %03d\n" % (i // 1000))
        else:
            f.write("line %05d of the big markdown body\n" % i)
' "$F5"
BIG_TITLES="$(python3 -c 'print("|".join("Section %03d" % i for i in range(99)))')"
BIG_RENAMED="$(python3 -c 'print("|".join(("Renamed %03d" if i == 50 else "Section %03d") % i for i in range(99)))')"
echo "  $(wc -l < "$F5" | tr -d ' ') 行 / $(wc -c < "$F5" | tr -d ' ') バイト"
P5="$(open_tab "$F5")" || exit 1
wait_md "$P5" markdown "$BIG_TITLES" 60 || true
check_eq "大きい md を描画表示で開いて目次が 99 件" 99 \
  "$(outline_titles "$P5" | awk -F'|' '{print NF}')"
run edit start --pane "$P5"
check_eq "大きい md の編集を始められる" 0 "$RC"
check_eq "編集中の目次も 99 件（原文の行）" 99 "$(outline_targets "$P5" | tr '|' '\n' | grep -c '^source_line:')"
"$TAKO_BIN" edit replace-range 50001:3 50001:10 Renamed --pane "$P5" >/dev/null
"$TAKO_BIN" edit autosave false --pane "$P5" >/dev/null
T0=$(python3 -c 'import time; print(time.time())')
"$TAKO_BIN" edit stop --pane "$P5" >/dev/null
T1=$(python3 -c 'import time; print(time.time())')
STOP_MS=$(python3 -c "print(int((${T1} - ${T0}) * 1000))")
T0=$(python3 -c 'import time; print(time.time())')
MODE_RIGHT_AFTER="$(pane_mode "$P5")"
T1=$(python3 -c 'import time; print(time.time())')
LIST_MS=$(python3 -c "print(int((${T1} - ${T0}) * 1000))")
echo "  edit stop の応答 ${STOP_MS} ms / 直後の tako list ${LIST_MS} ms（mode=${MODE_RIGHT_AFTER}）"
check_eq "抜けた直後から表示モードは markdown（描き直しは background）" markdown "$MODE_RIGHT_AFTER"
check_eq "background で描き直している間も GUI は応答する（tako list が 1 秒以内）" 1 \
  "$([ "$LIST_MS" -lt 1000 ] && echo 1 || echo 0)"
if wait_md "$P5" markdown "$BIG_RENAMED" 60; then
  pass "大きい md も書き換えた見出しで目次が作り直される（待った ${WAITED} ms）"
else
  fail "大きい md の目次が作り直されない（${WAITED} ms 待った）"
fi

echo
echo "== ⑥ コードのファイル・コード表示の md は今までどおり =="
F6="$TMP/six.rs"
printf 'fn main() {\n    println!("six");\n}\n' > "$F6"
P6="$(open_tab "$F6")" || exit 1
check_eq "コードは code で開く" code "$(pane_mode "$P6")"
"$TAKO_BIN" edit start --pane "$P6" >/dev/null
"$TAKO_BIN" edit replace-range 2:14 2:17 SIX --pane "$P6" >/dev/null
"$TAKO_BIN" edit save --pane "$P6" >/dev/null
"$TAKO_BIN" edit stop --pane "$P6" >/dev/null
check_eq "コードは抜けても code のまま" code "$(pane_mode "$P6")"
check_eq "コードには目次が無い（断る）" ERR "$(outline_titles "$P6")"
F6M="$TMP/six.md"
write_note "$F6M"
P6M="$(open_tab "$F6M" --mode code)" || exit 1
check_eq "md をコード表示で開ける" code "$(pane_mode "$P6M")"
"$TAKO_BIN" edit start --pane "$P6M" >/dev/null
"$TAKO_BIN" edit stop --pane "$P6M" >/dev/null
check_eq "コード表示から編集した md は抜けても code のまま（元のモードへ戻す）" code "$(pane_mode "$P6M")"

echo
echo "== ⑦ MCP tako_preview_outline / tako_preview_edit =="
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
mcp_call() { # ツール名 引数の JSON → 結果の本文（isError なら先頭に ERROR:）
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1661","version":"0"}}}' \
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
F7="$TMP/seven.md"
write_note "$F7"
P7="$(open_tab "$F7")" || exit 1
wait_md "$P7" markdown "Title|Alpha|Beta" || true
M_START="$(mcp_call tako_preview_edit "{\"pane\":$P7,\"enabled\":true}")"
check_eq "MCP で編集を始められる" true "$(printf '%s' "$M_START" | jget editing)"
M_OUTLINE="$(mcp_call tako_preview_outline "{\"pane\":$P7,\"item\":2}")"
check_eq "MCP の目次も編集中は source_line（項目 2 = 5 行目）" "source_line/5" \
  "$(printf '%s' "$M_OUTLINE" | jget selected.kind)/$(printf '%s' "$M_OUTLINE" | jget selected.line)"
"$TAKO_BIN" edit replace-range 5:3 5:8 ViaMcp --pane "$P7" >/dev/null
M_STOP="$(mcp_call tako_preview_edit "{\"pane\":$P7,\"enabled\":false}")"
check_eq "MCP で抜けられる" false "$(printf '%s' "$M_STOP" | jget editing)"
check_eq "MCP で抜けても markdown へ戻る" markdown "$(pane_mode "$P7")"
M_AFTER="$(mcp_call tako_preview_outline "{\"pane\":$P7}")"
check_eq "MCP の目次に書き換えた見出しが出る" "Title|ViaMcp|Beta" \
  "$(printf '%s' "$M_AFTER" | python3 -c 'import json,sys; print("|".join(i["title"] for i in json.load(sys.stdin)["outline"]))' 2>/dev/null)"
stop_isolated_gui "$APP_PID"
APP_PID=""

if [ "$CUSTOM_BINS" -eq 1 ]; then
  echo
  echo "（差し替えたバイナリなので ⑧ A/B と ⑨ 画面は飛ばす）"
else
  echo
  echo "== ⑧ A/B: TAKO_1661_LEGACY=1（#1661 前の挙動）では ① が落ちる =="
  rm -rf "$TAKO_DATA_DIR"
  mkdir -p "$TAKO_DATA_DIR"
  start_gui "$TMP/app-legacy.log" TAKO_1661_LEGACY=1
  APP_PID="$ISOLATED_GUI_PID"
  ROOT_PANE="$(root_pane)"
  basic_resume "$TMP/legacy.md" || exit 1
  echo "  legacy: 抜けた直後の mode=${RESUME_MODE} titles='${RESUME_TITLES}'（新: markdown / Renamed|Alpha|Beta）"
  check_eq "legacy では抜けても code のまま（① の検査が #1661 前を検出できる）" code "$RESUME_MODE"
  check_eq "legacy では目次が空のまま（Markdown・PDF ではないと断られる）" ERR "$RESUME_TITLES"
  stop_isolated_gui "$APP_PID"
  APP_PID=""

  echo
  echo "== ⑨ 画面: visual-test 節 md-edit-resume =="
  DUMP="${DUMP_DIR:-$TMP/frames}"
  mkdir -p "$DUMP"
  run_visual() { # ログ 追加の env…
    local log="$1" pid i rc=0
    shift
    rm -rf "$TAKO_DATA_DIR"
    mkdir -p "$TAKO_DATA_DIR"
    launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=md-edit-resume \
      TAKO_VISUAL_DUMP_DIR="$DUMP" "$@" || rc=$?
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
    grep -E "TAKO_VISUAL_1661|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$log" | sed 's/^/    /'
    grep -q "TAKO_VISUAL_TEST_OK" "$log"
  }
  if run_visual "$TMP/visual.log"; then
    pass "visual-test md-edit-resume が通る（編集 / 目次 / 編集中 を実マウスで押す）"
  else
    fail "visual-test md-edit-resume が落ちた"
  fi
  if run_visual "$TMP/visual-legacy.log" TAKO_1661_LEGACY=1; then
    fail "legacy でも visual-test md-edit-resume が通ってしまう（検出力が無い）"
  else
    case "$(grep TAKO_APP_SELF_TEST_FAILED "$TMP/visual-legacy.log")" in
      *"描画（Markdown）へ戻り"*) pass "legacy の visual 節は「描画（Markdown）へ戻り」で落ちる" ;;
      *) fail "legacy の visual 節が別の理由で落ちた: $(grep TAKO_APP_SELF_TEST_FAILED "$TMP/visual-legacy.log")" ;;
    esac
  fi
  echo "  フレーム: $DUMP"
  ls "$DUMP" 2>/dev/null | sed 's/^/    /'
fi

echo
echo "== 結果 =="
echo "  PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
