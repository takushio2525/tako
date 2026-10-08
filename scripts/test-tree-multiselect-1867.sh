#!/bin/bash
# test-tree-multiselect-1867.sh — ファイルツリーの複数選択・⌥⌘V・コピーの進み具合と取り消し
# （FR-3.38 / #1867）の実経路テスト
#
# 何を確かめるか（Issue #1867 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① visual-test `tree-multiselect` 節: 隔離 GUI（tako-vd）で**実マウス・実キー**の入口から
#      ⌘クリックで足す / 外す・⇧クリックで範囲・まとめて ⌘C → ⌘V・選んだ行を掴んでまとめて
#      運ぶ（D&D）・フォルダと配下を同時に選ぶ・⌥⌘V（配下へは断る / 貼り付け先が選択に含まれる）・
#      右クリックのごみ箱・大きなフォルダのコピーの帯と「取り消し」
#   ② A/B: `TAKO_1867_LEGACY=1`（⌘クリックが素の押下）の同じバイナリでは ① が FAILED になる
#   ③ 実 CLI と MCP `tako_file_op` が同じ操作をし、応答が**字面まで一致**する
#      （`paths` のまとめた要求・paste_move・copy_progress・copy_cancel。断る場合も一致）
#   ④ 走っているコピーを **CLI で始めて MCP で読み・取り消す**（逆も）: 作りかけを残さず止まる
#   ⑤ エッジ: 権限エラー（書けないフォルダへのまとめた移動は一部だけ断る）・0 件・無いパス
#
# 使い方: bash scripts/test-tree-multiselect-1867.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# OS のクリップボードは名前付きペーストボード、ゴミ箱は `TAKO_TRASH_DIR` = 一時 dir、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意できなければ起動せず終了コード 4 で
# 「未実測」を返す。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_FILE_PASTEBOARD TAKO_TRASH_DIR TAKO_1867_COPY_DELAY_MS TAKO_1867_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1867-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
BG_PID=""
TMUX_SOCKET="tako-1867-$$"
PB_NAME="tako-1867-$$"

cleanup() {
  if [ -n "$BG_PID" ]; then
    kill "$BG_PID" >/dev/null 2>&1 || true
  fi
  stop_isolated_gui "$APP_PID"
  /usr/bin/osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSPasteboard.pasteboardWithName('$PB_NAME').releaseGlobally; '{}'" >/dev/null 2>&1 || true
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  chmod -R u+rwx "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "test-tree-multiselect-1867"

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/tmpdir" "$TMP/trash"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_1867_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP"

# GUI の一時 dir も $TMP の中へ（visual-test の fixture が節の途中で落ちても後片付けで消える）
common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja TMPDIR="$TMP/tmpdir/"
  TAKO_PERSIST=0 TAKO_AUTORENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
  TAKO_FILE_PASTEBOARD="$PB_NAME"
)

# visual-test の節を 1 回走らせる（$1 = ログ / $2 = data dir / 残りは追加の env）
run_visual() {
  local log="$1" data="$2"
  shift 2
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$data" TAKO_1867_COPY_DELAY_MS=60 \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tree-multiselect TAKO_VISUAL_DUMP_DIR="$DUMP" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test tree-multiselect（実マウス・実キー） =="
run_visual "$TMP/visual.log" "$TMP/data-visual"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: tree-multiselect' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'

echo
echo "== ② A/B: TAKO_1867_LEGACY=1 で ⌘クリックが素の押下になり FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" TAKO_1867_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test tree-multiselect ①: ⌘クリックで 2 行目を足せる' \
  "$TMP/legacy.log" \
  && pass "落ちた理由は ⌘クリックで行を足せないこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（paths・paste_move・copy_progress・copy_cancel） =="
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/data-gui"
mkdir -p "$TAKO_DATA_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TMP/trash"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_TRASH_DIR="$TMP/trash" TAKO_1867_COPY_DELAY_MS=60 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1

# 同じ形の fixture を 2 つ（A = CLI / B = MCP）。**必ず $TMP の中**
make_fixture() {
  local dir="$1"
  case "$dir" in "$TMP"/*) : ;; *) echo "fixture が一時 dir の外: $dir"; exit 1 ;; esac
  mkdir -p "$dir/src" "$dir/folder/inner" "$dir/dst" "$dir/日本 語"
  printf 'A\n' > "$dir/src/a.txt"
  printf '# b\n' > "$dir/src/b c.md"
  printf 'C\n' > "$dir/src/c.txt"
  printf 'X\n' > "$dir/folder/inner/x.txt"
}
A="$TMP/cli-ws"
B="$TMP/mcp-ws"
make_fixture "$A"
make_fixture "$B"

normalize() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  jq -S -c --arg ws "$ws" --arg real "$real" '
    def sw: if type == "string" then (split($real) | join("<WS>") | split($ws) | join("<WS>")) else . end;
    walk(sw) | walk(if type == "object" and has("followed") then .followed |= map(.pane = "<PANE>") else . end)'
}
normalize_text() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  sed -e "s#${real}#<WS>#g" -e "s#${ws}#<WS>#g" -e 's/^error: //'
}

# 同じ操作列（op|paths（, 区切り。空 = 無し）|dest|name）
OPS=(
  "clipboard_copy|src/a.txt,src/b c.md,src/c.txt||"
  "clipboard|dst||"
  "paste|dst||"
  "clipboard_copy|folder||"
  "paste_move|folder/inner||"
  "paste_move|日本 語||"
  "clipboard_cut|dst/a.txt,dst/c.txt||"
  "paste|日本 語||"
  "move|dst/b c.md,src/a.txt|日本 語/folder|"
  "move|src/nope-1,src/nope-2|dst|"
  "move|日本 語/folder,日本 語/folder/inner|dst|"
  "trash|src/c.txt,dst/folder/inner/x.txt||"
  "trash|src/nope||"
  "copy_progress|||"
  "copy_cancel|||"
  "copy_cancel|||999999"
)
# CLI の引数（| 区切り）
cli_args() {
  local op="$1" paths="$2" dest="$3" name="$4" ws="$5" args="" p
  local IFS=','
  local list=""
  for p in $paths; do list="${list}|$ws/$p"; done
  case "$op" in
    clipboard_copy) args="file|clipboard|copy${list}" ;;
    clipboard_cut) args="file|clipboard|cut${list}" ;;
    clipboard) args="file|clipboard|show${list}" ;;
    paste) args="file|paste${list}" ;;
    paste_move) args="file|paste|--move${list}" ;;
    move) args="file|move${list}|$ws/$dest" ;;
    trash) args="file|trash${list}" ;;
    copy_progress) args="file|progress" ;;
    copy_cancel) args="file|cancel${name:+|$name}" ;;
  esac
  echo "$args"
}
# MCP の引数（1 件 = path / 複数 = paths。CLI の振り分けと同じ）
mcp_args() {
  local op="$1" paths="$2" dest="$3" name="$4" ws="$5" n
  local IFS=','
  # shellcheck disable=SC2086
  set -- $paths
  n=$#
  local arr
  arr="$(for p in "$@"; do printf '%s\n' "$ws/$p"; done | jq -R . | jq -s -c .)"
  jq -n -c --arg op "$op" --argjson arr "$arr" --argjson n "$n" --arg dest "$dest" --arg ws "$ws" --arg name "$name" '
    {op: $op}
    + (if $n == 1 then {path: $arr[0]} elif $n > 1 then {paths: $arr} else {} end)
    + (if $dest != "" then {dest: ($ws + "/" + $dest)} else {} end)
    + (if $name != "" then {name: $name} else {} end)'
}

i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  IFS='|' read -r op paths dest name <<EOF
$spec
EOF
  i=$((i + 1))
  IFS='|' read -r -a args <<EOF
$(cli_args "$op" "$paths" "$dest" "$name" "$A")
EOF
  if out="$("$TAKO_BIN" ${args[@]+"${args[@]}"} 2>"$TMP/cli-$i.err")"; then
    printf '%s' "$out" | normalize "$A" > "$TMP/cli-$i.norm"
    echo 0 > "$TMP/cli-$i.rc"
  else
    echo 1 > "$TMP/cli-$i.rc"
    if [ -n "$out" ]; then
      # まとめた操作の一部失敗 = 結果を出してから終了コード 1
      printf '%s' "$out" | normalize "$A" > "$TMP/cli-$i.norm"
    else
      normalize_text "$A" < "$TMP/cli-$i.err" | tr -d '\n' > "$TMP/cli-$i.norm"
    fi
  fi
done

SOCK="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  i=0
  for spec in ${OPS[@]+"${OPS[@]}"}; do
    IFS='|' read -r op paths dest name <<EOF
$spec
EOF
    i=$((i + 1))
    ARGS="$(mcp_args "$op" "$paths" "$dest" "$name" "$B")"
    jq -n -c --argjson id "$((i + 1))" --argjson args "$ARGS" \
      '{jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:"tako_file_op",arguments:$args}}'
    # 順に処理させる（応答を待ってから次 = CLI と同じ並び）
    sleep 0.6
  done
  # path も paths も無い trash は断る
  printf '%s\n' '{"jsonrpc":"2.0","id":99,"method":"tools/call","params":{"name":"tako_file_op","arguments":{"op":"trash"}}}'
  sleep 1
} | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null > "$TMP/mcp.out"
i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  jq -c --argjson id "$((i + 1))" 'select(.id == $id) | .result' "$TMP/mcp.out" > "$TMP/mcp-$i.raw"
  if [ "$(jq -r '.isError // false' "$TMP/mcp-$i.raw")" = "true" ]; then
    jq -r '.content[0].text' "$TMP/mcp-$i.raw" | normalize_text "$B" | tr -d '\n' > "$TMP/mcp-$i.norm"
  else
    jq -r '.content[0].text' "$TMP/mcp-$i.raw" | normalize "$B" > "$TMP/mcp-$i.norm"
  fi
done

echo "  --- CLI と MCP の応答（正規化後の字面） ---"
i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  echo "  [$i] ${spec}"
  echo "      CLI: $(cat "$TMP/cli-$i.norm")"
  echo "      MCP: $(cat "$TMP/mcp-$i.norm")"
  if [ -s "$TMP/cli-$i.norm" ] && cmp -s "$TMP/cli-$i.norm" "$TMP/mcp-$i.norm"; then
    pass "[$i] CLI と MCP の応答が字面まで一致"
  else
    fail "[$i] CLI と MCP の応答が食い違う"
  fi
done
for ws in "$A" "$B"; do
  tag="$( [ "$ws" = "$A" ] && echo CLI || echo MCP )"
  [ -d "$ws/dst/folder" ] && [ ! -e "$ws/folder" ] \
    && pass "$tag: paste_move とまとめた移動で folder が dst へ移っている" || fail "$tag: folder の行き先が想定と違う"
  [ -f "$ws/日本 語/a.txt" ] && [ -f "$ws/日本 語/c.txt" ] && [ ! -e "$ws/dst/a.txt" ] \
    && pass "$tag: まとめた切り取りの貼り付けで 2 件が移る" || fail "$tag: まとめた切り取りが移っていない"
  [ -f "$ws/日本 語/folder/b c.md" ] || [ -f "$ws/dst/folder/b c.md" ] \
    && pass "$tag: まとめた移動（D&D と同じ）で移る" || fail "$tag: まとめた移動が移っていない"
done
[ -f "$TMP/trash/c.txt" ] && [ -f "$TMP/trash/x.txt" ] && [ -f "$TMP/trash/c.txt 2" ] \
  && pass "trash はゴミ箱（の代わりの一時 dir）へ入る・同名は番号を付ける（上書きしない）" \
  || fail "ゴミ箱の中身が想定と違う: $(ls "$TMP/trash" | tr '\n' ' ')"
check_eq "CLI [1] 3 件まとめて載る" "3" "$(jq -r '.paths | length' "$TMP/cli-1.norm")"
case "$(cat "$TMP/cli-5.norm")" in *配下*) pass "CLI [5] ⌥⌘V を自分の配下へは理由つきで断る" ;; *) fail "CLI [5] $(cat "$TMP/cli-5.norm")" ;; esac
check_eq "CLI [6] paste_move は移動（mode = cut）" "cut" "$(jq -r '.mode' "$TMP/cli-6.norm")"
check_eq "CLI [9] まとめた移動は 2 件とも移す" "2" "$(jq -r '.done | length' "$TMP/cli-9.norm")"
case "$(cat "$TMP/cli-10.norm")" in *"2 件すべて"*) pass "CLI [10] 全部断られたらエラー（理由を並べる）" ;; *) fail "CLI [10] $(cat "$TMP/cli-10.norm")" ;; esac
check_eq "CLI [11] 配下を重ねたら親だけを移し skipped に載せる" "1/1" \
  "$(jq -r '"\(.done | length)/\(.skipped | length)"' "$TMP/cli-11.norm")"
check_eq "CLI [12] まとめたごみ箱は 2 件" "2" "$(jq -r '.done | length' "$TMP/cli-12.norm")"
check_eq "CLI [14] 走っているコピーが無ければ copies は空" '{"copies":[]}' "$(cat "$TMP/cli-14.norm")"
check_eq "CLI [15] 取り消すものが無ければ cancelled は空" '{"cancelled":[]}' "$(cat "$TMP/cli-15.norm")"
case "$(cat "$TMP/cli-16.norm")" in *走っていない*) pass "CLI [16] 無い番号の取り消しは理由つきで断る" ;; *) fail "CLI [16] $(cat "$TMP/cli-16.norm")" ;; esac
MCP_NOPATH="$(jq -r 'select(.id == 99) | (.error.message // .result.content[0].text)' "$TMP/mcp.out")"
case "$MCP_NOPATH" in *path*) pass "MCP: path も paths も無い trash は断る" ;; *) fail "MCP: path 無しの trash: $MCP_NOPATH" ;; esac

echo
echo "== ④ 走っているコピーを CLI で始めて MCP で読み・取り消す（逆も） =="
BIG="$TMP/big-ws"
mkdir -p "$BIG/big" "$BIG/dst"
for n in $(seq -w 1 40); do printf '0123456789%.0s' 1 2 3 4 5 6 7 8 9 10 > "$BIG/big/f$n.txt"; done
mcp_call() {
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    jq -n -c --argjson args "$1" '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"tako_file_op",arguments:$args}}'
    sleep 0.5
  } | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | jq -r 'select(.id == 2) | .result.content[0].text'
}
# 1 回目: CLI でコピーを始め、MCP で進み具合を読んで取り消す
"$TAKO_BIN" file copy "$BIG/big" "$BIG/dst" > "$TMP/bg1.out" 2> "$TMP/bg1.err" &
BG_PID=$!
ID=""
for _ in $(seq 1 50); do
  PROGRESS="$(mcp_call '{"op":"copy_progress"}')"
  DONE="$(printf '%s' "$PROGRESS" | jq -r '.copies[0].entries_done // 0' 2>/dev/null)"
  if [ "${DONE:-0}" -gt 2 ]; then
    ID="$(printf '%s' "$PROGRESS" | jq -r '.copies[0].id')"
    break
  fi
  sleep 0.1
done
echo "  観測: MCP copy_progress → $(printf '%s' "$PROGRESS" | sed "s#${TMP}#<TMP>#g")"
[ -n "$ID" ] && pass "MCP の copy_progress で CLI のコピーの進み具合（件数・バイト）が読める" \
  || fail "copy_progress にコピーが載らない"
check_eq "母数は数えた量（フォルダ + 40 件・4000 バイト）" "41/4000" \
  "$(printf '%s' "$PROGRESS" | jq -r '"\(.copies[0].entries_total)/\(.copies[0].bytes_total)"')"
CANCEL="$(mcp_call "{\"op\":\"copy_cancel\",\"name\":\"$ID\"}")"
check_eq "MCP の copy_cancel が番号で取り消す" "[$ID]" "$(printf '%s' "$CANCEL" | jq -c '.cancelled')"
wait "$BG_PID"
RC=$?
BG_PID=""
check_eq "取り消された CLI のコピーは終了コード 1" "1" "$RC"
case "$(cat "$TMP/bg1.err")" in *取り消した*) pass "CLI のエラー文に「取り消した」" ;; *) fail "CLI のエラー文: $(cat "$TMP/bg1.err")" ;; esac
[ ! -e "$BIG/dst/big" ] && pass "取り消したら作りかけを残さない（dst/big が無い）" || fail "作りかけが残っている"
check_eq "コピー元は全部残る" "40" "$(find "$BIG/big" -type f | wc -l | tr -d ' ')"
# 2 回目: MCP（tools/call）でコピーを始め、CLI で読んで取り消す
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  jq -n -c --arg p "$BIG/big" --arg d "$BIG/dst" '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"tako_file_op",arguments:{op:"copy",path:$p,dest:$d}}}'
  sleep 6
} | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null > "$TMP/bg2.out" &
BG_PID=$!
ID=""
for _ in $(seq 1 50); do
  PROGRESS="$("$TAKO_BIN" file progress 2>/dev/null)"
  DONE="$(printf '%s' "$PROGRESS" | jq -r '.copies[0].entries_done // 0' 2>/dev/null)"
  if [ "${DONE:-0}" -gt 2 ]; then
    ID="$(printf '%s' "$PROGRESS" | jq -r '.copies[0].id')"
    break
  fi
  sleep 0.1
done
[ -n "$ID" ] && pass "CLI の tako file progress で MCP のコピーが読める" || fail "tako file progress に載らない"
CANCEL="$("$TAKO_BIN" file cancel "$ID" 2>&1)"
check_eq "CLI の tako file cancel が番号で取り消す" "[$ID]" "$(printf '%s' "$CANCEL" | jq -c '.cancelled' 2>/dev/null)"
wait "$BG_PID" 2>/dev/null
BG_PID=""
MCP_COPY="$(jq -r 'select(.id == 2) | .result.content[0].text' "$TMP/bg2.out")"
case "$MCP_COPY" in *取り消した*) pass "取り消された MCP のコピーは「取り消した」を返す" ;; *) fail "MCP のコピーの応答: $MCP_COPY" ;; esac
[ ! -e "$BIG/dst/big" ] && pass "MCP 側も作りかけを残さない" || fail "作りかけが残っている（MCP）"
check_eq "取り消した後は走っているコピーが無い" '[]' "$("$TAKO_BIN" file progress | jq -c '.copies')"

echo
echo "== ⑤ エッジ: 権限エラー・0 件・無いパス =="
E="$TMP/edge-ws"
mkdir -p "$E/src" "$E/locked" "$E/dst"
printf 'a\n' > "$E/src/a.txt"
printf 'l\n' > "$E/locked/l.txt"
chmod 555 "$E/locked"
# 書けないフォルダの中身は移せない（一部だけ断る = 結果を出して終了コード 1）
OUT="$("$TAKO_BIN" file move "$E/src/a.txt" "$E/locked/l.txt" "$E/dst" 2>"$TMP/edge.err")"
RC=$?
echo "  観測: $(printf '%s' "$OUT" | sed "s#${TMP}#<TMP>#g") / $(sed "s#${TMP}#<TMP>#g" "$TMP/edge.err")"
check_eq "権限エラーが混ざったまとめた移動は終了コード 1" "1" "$RC"
check_eq "移せたものは移す（1 件）・断ったものは failed（1 件）" "1/1" \
  "$(printf '%s' "$OUT" | jq -r '"\(.done | length)/\(.failed | length)"' 2>/dev/null)"
[ -f "$E/locked/l.txt" ] && [ -f "$E/dst/a.txt" ] && pass "断ったものは元の場所・移せたものは移動先" || fail "権限エラーの後の状態が想定と違う"
chmod 755 "$E/locked"
OUT="$("$TAKO_BIN" file clipboard copy "$E/src/nope" "$E/dst" 2>&1)"
case "$OUT" in *パスが存在しない*) pass "無いパスが混ざったまとめたコピーは置かない" ;; *) fail "無いパス: $OUT" ;; esac
OUT="$(mcp_call "{\"op\":\"trash\",\"paths\":[]}")"
case "$OUT" in *"1 つ以上"*) pass "MCP: 空の paths は断る（0 件）" ;; *) fail "空の paths: $OUT" ;; esac
OUT="$(mcp_call "{\"op\":\"copy_cancel\",\"name\":\"abc\"}")"
case "$OUT" in *番号*) pass "MCP: 番号でない name の取り消しは断る（CLI は引数の型で先に断る）" ;; *) fail "name=abc: $OUT" ;; esac

stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== before（配布中の版 = この変更の前）の観測 =="
OLD="/Applications/tako.app/Contents/MacOS/tako"
if [ -x "$OLD" ]; then
  OLD_HELP="$(env -u TAKO_SOCKET "$OLD" file progress 2>&1 | head -1)"
  echo "  観測: 配布中の tako file progress → ${OLD_HELP}"
  case "$OLD_HELP" in *unrecognized*|*unexpected*|*error*) pass "before: 配布中の CLI には file progress が無い" ;; *) fail "before: 配布中の CLI に file progress がある？" ;; esac
else
  echo "  未実測: 配布中の tako が無い"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -eq 0 ]; then
  tako_exit 0
fi
exit 1
