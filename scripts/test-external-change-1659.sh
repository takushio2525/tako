#!/usr/bin/env bash
# test-external-change-1659.sh — 外部変更を検知した後の逃げ道の実経路テスト（#1659）
#
# 隔離した data / tmux で**実 tako-app** を立て、編集中のファイルをディスク側で書き換えて、
#
#   ① 保存は書かずに断り、抜け方（save --force / reload / diff）を添える。
#      `tako edit diff` はディスク → 編集中の差分を返し、何も変えない。
#      `tako edit save --force` で自分の変更が残り、`tako edit reload` でディスクの中身になる。
#      読み直しは undo 1 回で自分の変更へ戻る（Issue の本体: ペインを閉じる以外に抜けられなかった）
#   ② 自動保存 ON（既定）でも、競合のあいだは保存を試みず、知らせは 1 回だけ
#      （修正前は 500ms ごとに同じ競合を出し続けた）。ディスクの外部変更は踏み潰さない
#   ③ エッジ: 外で消された（保存 / 読み直しは断り、save --force で作り直す）/ CRLF /
#      大きいファイル（5 万行）/ 自動保存 OFF（① がそれ）
#   ④ 未保存のプレビューへ別のファイルを開くと、差し替えずに分割して開く
#      （修正前は「未保存の変更があるため別ファイルを開けない」で行き止まり）
#   ⑤ MCP `tako_preview_save` の action（overwrite / reload / diff）が CLI と同じ dispatch を通る
#   ⑥ A/B: `TAKO_1659_LEGACY=1`（#1659 前の挙動）の GUI では ② が落ちる = この検査に検出力がある
#   ⑦ 画面: visual-test 節 `external-change`（帯の 差分 / 上書き保存 / 読み直す を実マウスで押す）が
#      通り、`TAKO_1659_LEGACY=1` では「知らせは 1 回だけ」で落ちる。フレームは $DUMP_DIR へ残す
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-external-change-1659.sh
#   main など別のビルドで同じ検査を走らせるときは TAKO_BIN / APP_BIN でバイナリを差し替える
#   （その場合 ⑥ ⑦ は飛ばす = 旧ビルドには A/B の口も visual 節も無い）。
#   DUMP_DIR を渡すと ⑦ のフレーム（PNG）をそこへ残す
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
check_has() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'${2}' を含まない: ${3}）" ;;
  esac
}

# 差し替えたバイナリ（main 等）で走らせているか。A/B の口と visual 節はこのビルドにしか無い
CUSTOM_BINS=0
if [ -n "${TAKO_BIN:-}" ] || [ -n "${APP_BIN:-}" ]; then
  CUSTOM_BINS=1
fi

TMP="$(mktemp -d /tmp/tako-1659-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1659-$$"
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

# 応答 JSON から 1 つの値を取る（`conflict.notices` のように辿れる）
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

# `tako` を叩いて stdout と stderr をまとめて OUT へ、終了コードを RC へ置く（失敗の文面も読むため）。
# **`$( )` の中で呼ばない**（サブシェルで立てた RC は親へ戻らない）
OUT=""
RC=0
run() {
  OUT="$("$TAKO_BIN" "$@" 2>&1)"
  RC=$?
}

# ファイルを開いて編集を始め、ペイン ID を返す（自動保存は $2 = true / false）
open_edit() {
  local file="$1" autosave="$2" pane
  pane="$("$TAKO_BIN" open "$file" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
  case "$pane" in
    ''|*[!0-9]*) echo "開けない: $file" >&2; return 1 ;;
  esac
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null || return 1
  "$TAKO_BIN" edit autosave "$autosave" --pane "$pane" >/dev/null || return 1
  printf '%s' "$pane"
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

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
echo "  pid=$APP_PID"
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])
')"

echo
echo "== ① 上書きと読み直し（自動保存 OFF）=="
F1="$TMP/one.rs"
printf 'a\nb\nc\n' > "$F1"
P1="$(open_edit "$F1" false)" || exit 1
"$TAKO_BIN" edit replace-range 2:0 2:1 MINE --pane "$P1" >/dev/null
printf 'a\nDISK\nc\n' > "$F1"
run edit save --pane "$P1"
check_eq "外部変更の後の保存は断る（終了コード非 0）" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
check_has "断った理由に上書きの抜け方が載る" "tako edit save --force" "$OUT"
check_has "断った理由に読み直しの抜け方が載る" "tako edit reload" "$OUT"
check_eq "ディスクは外の中身のまま" "$(printf 'a\nDISK\nc\n')" "$(cat "$F1")"
run edit status --pane "$P1"
check_eq "status に conflict.state=changed" changed "$(printf '%s' "$OUT" | jget conflict.state)"
run edit diff --pane "$P1"
DIFF="$OUT"
echo "  edit diff の応答: $DIFF"
check_has "diff はディスク → 編集中の向き" "-DISK\\n+MINE" "$(printf '%s' "$DIFF" | jget diff.unified | python3 -c 'import sys; print(repr(sys.stdin.read()))')"
check_eq "diff は何も変えない（ディスク）" "$(printf 'a\nDISK\nc\n')" "$(cat "$F1")"
run edit save --force --pane "$P1"
FORCED="$OUT"
echo "  edit save --force の応答: $FORCED"
check_eq "save --force が通る" 0 "$RC"
check_eq "上書きで自分の変更がディスクへ残る" "$(printf 'a\nMINE\nc\n')" "$(cat "$F1")"
check_eq "上書きの後は conflict が消える" __MISSING__ "$(printf '%s' "$FORCED" | jget conflict)"
# もう一度ぶつけて読み直す
"$TAKO_BIN" edit replace-range 1:0 1:1 A --pane "$P1" >/dev/null
printf 'a\nMINE\nc\nexternal\n' > "$F1"
run edit save --pane "$P1"
run edit reload --pane "$P1"
REVERTED="$OUT"
echo "  edit reload の応答: $REVERTED"
check_eq "reload が通る" 0 "$RC"
check_eq "reload の後は dirty=false" false "$(printf '%s' "$REVERTED" | jget dirty)"
check_eq "reload の後は conflict が消える" __MISSING__ "$(printf '%s' "$REVERTED" | jget conflict)"
run edit save --pane "$P1"
check_eq "reload の後は通常の保存が通る" 0 "$RC"
run edit undo --pane "$P1"
check_eq "読み直しは undo 1 回で自分の変更へ戻る（dirty=true）" true "$(printf '%s' "$OUT" | jget dirty)"
run edit save --pane "$P1"
check_eq "戻した自分の変更は競合せずに保存できる" "$(printf 'A\nMINE\nc\n')" "$(cat "$F1")"

echo
echo "== ② 自動保存 ON: 競合中は止まり、知らせは 1 回だけ =="
F2="$TMP/two.rs"
printf 'x\ny\nz\n' > "$F2"
P2="$(open_edit "$F2" true)" || exit 1
# 打った直後（自動保存の 500ms より前）にディスク側を書き換える
"$TAKO_BIN" edit replace-range 2:0 2:1 Y --pane "$P2" >/dev/null
printf 'x\nexternal\nz\n' > "$F2"
sleep 1.5
# 1 回の IPC ごとに自動保存の判定が回る。5 回叩いて、知らせの回数が増えないことを見る
for i in 1 2 3 4 5; do
  "$TAKO_BIN" edit status --pane "$P2" >/dev/null
  sleep 0.7
done
run edit status --pane "$P2"
S2="$OUT"
echo "  5 回叩いた後の status: $S2"
NOTICES2="$(printf '%s' "$S2" | jget conflict.notices)"
check_eq "競合中に IPC を 5 回叩いても知らせは 1 回" 1 "$NOTICES2"
check_eq "自動保存は止まっている（autosave_paused=true）" true "$(printf '%s' "$S2" | jget conflict.autosave_paused)"
check_eq "ディスクの外部変更を踏み潰さない" "$(printf 'x\nexternal\nz\n')" "$(cat "$F2")"
check_eq "編集中の内容は保持する（dirty=true）" true "$(printf '%s' "$S2" | jget dirty)"

echo
echo "== ③-a 外で消された =="
F3="$TMP/three.rs"
printf 'keep\n' > "$F3"
P3="$(open_edit "$F3" false)" || exit 1
"$TAKO_BIN" edit replace-range 1:0 1:4 mine --pane "$P3" >/dev/null
rm -f "$F3"
run edit save --pane "$P3"
check_eq "消されたファイルの保存は断る" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
check_has "断った理由に「作り直す」が載る" "作り直す" "$OUT"
check_eq "通常の保存は作り直さない" 0 "$([ -e "$F3" ] && echo 1 || echo 0)"
run edit diff --pane "$P3"
check_eq "diff は state=deleted" deleted "$(printf '%s' "$OUT" | jget diff.state)"
run edit reload --pane "$P3"
check_eq "消されたファイルは読み直せない（断る）" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
run edit save --force --pane "$P3"
check_eq "save --force で作り直す" "mine" "$(cat "$F3" 2>/dev/null)"

echo
echo "== ③-b CRLF =="
F4="$TMP/four.txt"
printf 'a\r\nb\r\n' > "$F4"
P4="$(open_edit "$F4" false)" || exit 1
"$TAKO_BIN" edit replace-range 1:0 1:1 A --pane "$P4" >/dev/null
printf 'a\r\nb\r\nexternal\r\n' > "$F4"
run edit reload --pane "$P4"
R4="$OUT"
check_eq "CRLF のファイルを読み直すと改行コードは CRLF のまま" crlf "$(printf '%s' "$R4" | jget document.line_ending)"
"$TAKO_BIN" edit cursor 4:0 --pane "$P4" >/dev/null
"$TAKO_BIN" edit replace-range 4:0 4:0 tail --pane "$P4" >/dev/null
"$TAKO_BIN" edit newline --pane "$P4" >/dev/null
run edit save --pane "$P4"
check_eq "読み直した後の改行も CRLF で保存される" \
  "$(printf 'a\r\nb\r\nexternal\r\ntail\r\n' | od -An -tx1 | tr -d ' \n')" \
  "$(od -An -tx1 < "$F4" | tr -d ' \n')"

echo
echo "== ③-c 大きいファイル（5 万行）=="
F5="$TMP/big.txt"
python3 -c '
import sys
with open(sys.argv[1], "w", encoding="utf-8") as f:
    for i in range(1, 50001):
        f.write("line %05d: the quick brown fox jumps over the lazy dog\n" % i)
' "$F5"
echo "  $(wc -c < "$F5" | tr -d ' ') バイト"
P5="$(open_edit "$F5" false)" || exit 1
"$TAKO_BIN" edit replace-range 25001:0 25001:4 LINE --pane "$P5" >/dev/null
python3 -c '
import sys
p = sys.argv[1]
lines = open(p, encoding="utf-8").read().split("\n")
lines[24999] = "external change at 25000"
open(p, "w", encoding="utf-8").write("\n".join(lines))
' "$F5"
T0=$(python3 -c 'import time; print(time.time())')
run edit diff --pane "$P5"
D5="$OUT"
T1=$(python3 -c 'import time; print(time.time())')
echo "  diff: added=$(printf '%s' "$D5" | jget diff.added) removed=$(printf '%s' "$D5" | jget diff.removed) $(python3 -c "print('%.0f ms' % ((${T1} - ${T0}) * 1000))")"
check_eq "5 万行でも差分は変わった 2 行だけ（追加 2 / 削除 2）" "2/2" \
  "$(printf '%s' "$D5" | jget diff.added)/$(printf '%s' "$D5" | jget diff.removed)"
run edit reload --pane "$P5"
R5="$OUT"
check_eq "5 万行の読み直しが通る" 0 "$RC"
HIST="$(printf '%s' "$R5" | jget document.undo_history_bytes)"
echo "  読み直した後の undo_history_bytes=$HIST"
check_eq "読み直しは全文を履歴に積まない（64 KiB 未満）" 1 "$([ "${HIST:-999999999}" -lt 65536 ] 2>/dev/null && echo 1 || echo 0)"

echo
echo "== ④ 未保存のプレビューへ別のファイルを開く =="
F6="$TMP/six.rs"
F7="$TMP/seven.rs"
printf 'six\n' > "$F6"
printf 'seven\n' > "$F7"
P6="$(open_edit "$F6" false)" || exit 1
"$TAKO_BIN" edit replace-range 1:0 1:3 SIX --pane "$P6" >/dev/null
run open "$F7" --pane "$P6"
O7="$OUT"
echo "  open の応答: $O7"
check_eq "開ける（行き止まりにならない）" 0 "$RC"
check_eq "未保存のペインを差し替えずに残す（kept_unsaved）" "$P6" "$(printf '%s' "$O7" | jget kept_unsaved)"
O7_PANE="$(printf '%s' "$O7" | jget pane)"
case "$O7_PANE" in
  ''|*[!0-9]*) fail "別のペインで開く（応答に pane が無い: ${O7}）" ;;
  *) check_eq "別のペインで開く" 1 "$([ "$O7_PANE" != "$P6" ] && echo 1 || echo 0)" ;;
esac
run edit status --pane "$P6"
check_eq "未保存のペインの変更は残っている" true "$(printf '%s' "$OUT" | jget dirty)"

echo
echo "== ⑤ MCP tako_preview_save の action =="
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
export TAKO_TOKEN
mcp_call() { # ツール名 引数の JSON → 結果の本文（isError なら先頭に ERROR:）
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1659","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c '
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
F8="$TMP/eight.rs"
printf 'a\nb\nc\n' > "$F8"
P8="$(open_edit "$F8" false)" || exit 1
"$TAKO_BIN" edit replace-range 2:0 2:1 MINE --pane "$P8" >/dev/null
printf 'a\nDISK\nc\n' > "$F8"
M_SAVE="$(mcp_call tako_preview_save "{\"pane\":$P8}")"
echo "  MCP save（競合中）: $M_SAVE"
check_has "MCP の保存も断り、action の抜け方を添える" "action=overwrite" "$M_SAVE"
M_DIFF="$(mcp_call tako_preview_save "{\"pane\":$P8,\"action\":\"diff\"}")"
echo "  MCP action=diff: $M_DIFF"
run edit diff --pane "$P8"
C_DIFF="$OUT"
normalize() {
  python3 -c '
import json, sys
d = json.loads(sys.stdin.read())
d.pop("pane", None)
print(json.dumps(d, ensure_ascii=False, sort_keys=True))
'
}
# 両方が失敗して同じ空になる形で緑にならないよう、先に中身があることを見る
check_eq "MCP action=diff が差分を返す（state=changed）" changed "$(printf '%s' "$M_DIFF" | jget diff.state)"
check_eq "MCP action=diff の応答が CLI の edit diff と字面まで一致" \
  "$(printf '%s' "$C_DIFF" | normalize)" "$(printf '%s' "$M_DIFF" | normalize)"
M_OVER="$(mcp_call tako_preview_save "{\"pane\":$P8,\"action\":\"overwrite\"}")"
echo "  MCP action=overwrite: $M_OVER"
check_eq "MCP action=overwrite で上書きされる" "$(printf 'a\nMINE\nc\n')" "$(cat "$F8")"
"$TAKO_BIN" edit replace-range 1:0 1:1 A --pane "$P8" >/dev/null
printf 'a\nMINE\nc\nmcp\n' > "$F8"
M_RELOAD="$(mcp_call tako_preview_save "{\"pane\":$P8,\"action\":\"reload\"}")"
echo "  MCP action=reload: $M_RELOAD"
check_eq "MCP action=reload で読み直す（reverted=true）" true "$(printf '%s' "$M_RELOAD" | jget reverted)"
check_eq "MCP action=reload の後は dirty=false" false "$(printf '%s' "$M_RELOAD" | jget dirty)"
unset TAKO_SOCKET TAKO_TOKEN

stop_isolated_gui "$APP_PID"
APP_PID=""

if [ "$CUSTOM_BINS" -eq 1 ]; then
  echo
  echo "（差し替えたバイナリなので ⑥ A/B と ⑦ 画面は飛ばす）"
else
  echo
  echo "== ⑥ A/B: TAKO_1659_LEGACY=1（#1659 前の挙動）では ② が落ちる =="
  rm -rf "$TAKO_DATA_DIR"
  mkdir -p "$TAKO_DATA_DIR"
  start_gui "$TMP/app-legacy.log" TAKO_1659_LEGACY=1
  APP_PID="$ISOLATED_GUI_PID"
  ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])
')"
  FL="$TMP/legacy.rs"
  printf 'x\ny\nz\n' > "$FL"
  PL="$(open_edit "$FL" true)" || exit 1
  "$TAKO_BIN" edit replace-range 2:0 2:1 Y --pane "$PL" >/dev/null
  printf 'x\nexternal\nz\n' > "$FL"
  sleep 1.5
  for i in 1 2 3 4 5; do
    "$TAKO_BIN" edit status --pane "$PL" >/dev/null
    sleep 0.7
  done
  NL="$("$TAKO_BIN" edit status --pane "$PL" 2>&1 | jget conflict.notices)"
  echo "  legacy: 5 回叩いた後の notices=${NL}（新: 1）"
  check_eq "legacy では知らせが積み上がる（② の検査が #1659 前を検出できる）" 1 \
    "$([ "${NL:-0}" -gt 1 ] 2>/dev/null && echo 1 || echo 0)"
  # ④ の検査も #1659 前を検出できる（未保存のペインへ別のファイルを開くと断られる）
  FL2="$TMP/legacy-other.rs"
  printf 'other\n' > "$FL2"
  "$TAKO_BIN" edit autosave false --pane "$PL" >/dev/null
  run open "$FL2" --pane "$PL"
  echo "  legacy: 未保存のペインへ open → rc=$RC / $OUT"
  check_has "legacy では未保存のペインへ別のファイルを開けない（④ の検査が #1659 前を検出できる）" \
    "別ファイルを開けない" "$OUT"
  stop_isolated_gui "$APP_PID"
  APP_PID=""

  echo
  echo "== ⑦ 画面: visual-test 節 external-change =="
  DUMP="${DUMP_DIR:-$TMP/frames}"
  mkdir -p "$DUMP"
  run_visual() { # ログ 追加の env…
    local log="$1" pid i rc=0
    shift
    rm -rf "$TAKO_DATA_DIR"
    mkdir -p "$TAKO_DATA_DIR"
    launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=external-change \
      TAKO_VISUAL_DUMP_DIR="$DUMP" "$@" || rc=$?
    if [ "$rc" -eq 4 ]; then
      echo "  仮想ディスプレイを用意できないので画面は未実測"
      return 4
    fi
    [ "$rc" -eq 0 ] || return "$rc"
    pid="$ISOLATED_GUI_PID"
    for i in $(seq 1 1800); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    stop_isolated_gui "$pid"
    grep -E "TAKO_VISUAL_1659|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$log" | sed 's/^/    /'
    grep -q "TAKO_VISUAL_TEST_OK" "$log"
  }
  if run_visual "$TMP/visual.log"; then
    pass "visual-test external-change が通る（帯・差分・上書き保存・読み直す を実マウスで押す）"
  else
    fail "visual-test external-change が落ちた"
  fi
  if run_visual "$TMP/visual-legacy.log" TAKO_1659_LEGACY=1; then
    fail "legacy でも visual-test external-change が通ってしまう（検出力が無い）"
  else
    check_has "legacy の visual 節は「知らせは 1 回だけ」で落ちる" "知らせは 1 回だけ" \
      "$(grep TAKO_APP_SELF_TEST_FAILED "$TMP/visual-legacy.log")"
  fi
  echo "  フレーム: $DUMP"
  ls "$DUMP" 2>/dev/null | sed 's/^/    /'
fi

echo
echo "== 結果 =="
echo "  PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
