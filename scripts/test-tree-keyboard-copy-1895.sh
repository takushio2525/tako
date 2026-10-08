#!/bin/bash
# test-tree-keyboard-copy-1895.sh — ファイルツリーのキーでの範囲選択・ごみ箱、1 つのファイルの
# 途中での取り消し、複数のコピーを 1 つのジョブへ、帯の残り時間（FR-3.39 / #1895）の実経路テスト
#
# 何を確かめるか（Issue #1895 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① visual-test `tree-keyboard-copy` 節: 隔離 GUI（tako-vd）で**実マウス・実キー**の入口から
#      ⇧↑ / ⇧↓ で範囲が伸び縮みする（先頭・末尾・畳んだフォルダ）・⌘⌫（Windows は Delete）で
#      ごみ箱へ（見出しは断る）・大きな 1 ファイルのコピーで件数 0 のままバイトが単調に増え、
#      帯の「取り消し」で作りかけを残さず止まり、直後に貼り直すと最後まで写る・帯に残り時間
#   ② A/B: `TAKO_1895_LEGACY=1`（#1895 の前）の同じバイナリでは ① が名指しで FAILED
#   ③ 実 CLI と MCP `tako_file_op`: `tako file copy a b dst` と MCP の `paths` の応答が**字面まで
#      一致**し、走っている間は**1 つのジョブ**（paths に全部）として見え、1 つのファイルの途中で
#      バイトが進み、残り時間（`eta_secs`）が出て、片方で始めたコピーをもう片方で取り消せる
#   ④ A/B（エンジンと CLI）: `TAKO_1895_LEGACY=1` の GUI / CLI では 2 つのジョブに割れ、1 つの
#      ファイルの途中でバイトが進まない（③ の検査が名指しで落ちる）
#   ⑤ エッジ: 権限エラー（書けないフォルダへのまとめたコピー）・0 バイトのファイル・取り消しの
#      直後の再開・無いパスが混ざったまとめたコピー
#
# 使い方: bash scripts/test-tree-keyboard-copy-1895.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# OS のクリップボードは名前付きペーストボード、ゴミ箱は `TAKO_TRASH_DIR` = 一時 dir、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。大きなファイルは 16〜40 MiB を
# 一時 dir に作り（CPU を焼く負荷は使わない = 遅さは `TAKO_1895_COPY_CHUNK_DELAY_MS` の注入）、
# 終わったら消す。窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装）。
# 面を用意できなければ起動せず終了コード 4 で「未実測」を返す。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_FILE_PASTEBOARD TAKO_TRASH_DIR TAKO_1867_COPY_DELAY_MS TAKO_1867_LEGACY \
  TAKO_1895_COPY_CHUNK_DELAY_MS TAKO_1895_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1895-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
BG_PID=""
TMUX_SOCKET="tako-1895-$$"
PB_NAME="tako-1895-$$"
# 1 つのファイルの中身を写す単位（macOS は 1 MiB）ごとの遅延（ミリ秒）
CHUNK_DELAY_MS=300

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
tako_exit_trap cleanup "test-tree-keyboard-copy-1895"

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/tmpdir" "$TMP/trash"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_1895_DUMP_DIR:-$TMP/dump}"
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
    TAKO_DATA_DIR="$data" TAKO_1895_COPY_CHUNK_DELAY_MS="$CHUNK_DELAY_MS" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tree-keyboard-copy TAKO_VISUAL_DUMP_DIR="$DUMP" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test tree-keyboard-copy（実マウス・実キー） =="
run_visual "$TMP/visual.log" "$TMP/data-visual"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: tree-keyboard-copy' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'

echo
echo "== ② A/B: TAKO_1895_LEGACY=1 で ⇧↓ がツリーへ向かず FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" TAKO_1895_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test tree-keyboard-copy ①: ⇧↓ で 1 行下へ伸びる' \
  "$TMP/legacy.log" \
  && pass "落ちた理由は ⇧↓ で範囲が伸びないこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（まとめたコピー = 1 つのジョブ・途中のバイト・残り時間・取り消し） =="
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

# GUI を立てて接続情報を読む（$1 = data dir / $2 = ログ / 残りは追加の env）
start_gui() {
  local data="$1" log="$2"
  shift 2
  export TAKO_DATA_DIR="$data"
  mkdir -p "$TAKO_DATA_DIR"
  case "$TAKO_DATA_DIR" in "$TMP"/*) : ;; *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;; esac
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_TRASH_DIR="$TMP/trash" \
    TAKO_1895_COPY_CHUNK_DELAY_MS="$CHUNK_DELAY_MS" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || exit 1
  SOCK="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
}
mcp_call() {
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    jq -n -c --argjson args "$1" '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"tako_file_op",arguments:$args}}'
    sleep "${2:-0.5}"
  } | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | jq -c 'select(.id == 2) | .result'
}
mcp_text() { printf '%s' "$1" | jq -r '.content[0].text'; }
# 中身のあるファイル（MiB。疎なファイルにしない）。必ず $TMP の中
make_big() {
  case "$1" in "$TMP"/*) : ;; *) echo "大きなファイルが一時 dir の外: $1"; exit 1 ;; esac
  mkdir -p "$(dirname "$1")"
  head -c "$(($2 * 1048576))" /dev/urandom > "$1"
}

start_gui "$TMP/data-gui" "$TMP/gui.log"

# (a) まとめたコピーの応答が CLI と MCP で字面まで一致する（A = CLI / B = MCP）
make_fixture() {
  local dir="$1"
  case "$dir" in "$TMP"/*) : ;; *) echo "fixture が一時 dir の外: $dir"; exit 1 ;; esac
  mkdir -p "$dir/src" "$dir/folder/inner" "$dir/dst" "$dir/日本 語"
  printf 'A\n' > "$dir/src/a.txt"
  printf '# b\n' > "$dir/src/b c.md"
  : > "$dir/src/empty.txt"
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
    walk(sw)'
}
normalize_text() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  sed -e "s#${real}#<WS>#g" -e "s#${ws}#<WS>#g" -e 's/^error: //'
}
# 同じ操作列（コピー元の , 区切り | 貼り付け先）
OPS=(
  "src/a.txt,src/b c.md,folder,folder/inner/x.txt|dst"
  "src/a.txt,src/empty.txt|日本 語"
  "src/a.txt,src/nope|dst"
  "src/nope-1,src/nope-2|dst"
)
i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  IFS='|' read -r paths dest <<EOF
$spec
EOF
  args=()
  IFS=',' read -r -a rels <<EOF
$paths
EOF
  for p in ${rels[@]+"${rels[@]}"}; do args+=("$A/$p"); done
  args+=("$A/$dest")
  if out="$("$TAKO_BIN" file copy ${args[@]+"${args[@]}"} 2>"$TMP/cli-$i.err")"; then
    echo 0 > "$TMP/cli-$i.rc"
  else
    echo 1 > "$TMP/cli-$i.rc"
  fi
  if [ -n "$out" ]; then
    printf '%s' "$out" | normalize "$A" > "$TMP/cli-$i.norm"
  else
    normalize_text "$A" < "$TMP/cli-$i.err" | tr -d '\n' > "$TMP/cli-$i.norm"
  fi
  arr="$(for p in ${rels[@]+"${rels[@]}"}; do printf '%s\n' "$B/$p"; done | jq -R . | jq -s -c .)"
  res="$(mcp_call "$(jq -n -c --argjson arr "$arr" --arg dest "$B/$dest" '{op:"copy",paths:$arr,dest:$dest}')" 3)"
  if [ "$(printf '%s' "$res" | jq -r '.isError // false')" = "true" ]; then
    mcp_text "$res" | normalize_text "$B" | tr -d '\n' > "$TMP/mcp-$i.norm"
  else
    mcp_text "$res" | normalize "$B" > "$TMP/mcp-$i.norm"
  fi
  echo "  [$i] ${spec}（CLI の終了コード $(cat "$TMP/cli-$i.rc")）"
  echo "      CLI: $(cat "$TMP/cli-$i.norm")"
  echo "      MCP: $(cat "$TMP/mcp-$i.norm")"
  if [ -s "$TMP/cli-$i.norm" ] && cmp -s "$TMP/cli-$i.norm" "$TMP/mcp-$i.norm"; then
    pass "[$i] CLI と MCP の応答が字面まで一致"
  else
    fail "[$i] CLI と MCP の応答が食い違う"
  fi
done
check_eq "CLI [1] 3 件を写し、配下は親と一緒（skipped 1 件）" "3/0/1" \
  "$(jq -r '"\(.done | length)/\(.failed | length)/\(.skipped | length)"' "$TMP/cli-1.norm")"
check_eq "CLI [1] 全部写せたら終了コード 0" "0" "$(cat "$TMP/cli-1.rc")"
check_eq "CLI [2] 0 バイトのファイルも写る（bytes 0）" "0" \
  "$(jq -r '.done[] | select(.to | endswith("empty.txt")) | .bytes' "$TMP/cli-2.norm")"
check_eq "CLI [3] 一部断られたら写せたものは写し failed に理由（終了コード 1）" "1/1/1" \
  "$(jq -r '"\(.done | length)/\(.failed | length)"' "$TMP/cli-3.norm")/$(cat "$TMP/cli-3.rc")"
case "$(cat "$TMP/cli-4.norm")" in *"2 件すべて"*) pass "CLI [4] 全部断られたらエラー（理由を並べる）" ;; *) fail "CLI [4] $(cat "$TMP/cli-4.norm")" ;; esac
for ws in "$A" "$B"; do
  tag="$( [ "$ws" = "$A" ] && echo CLI || echo MCP )"
  [ -f "$ws/dst/a.txt" ] && [ -f "$ws/dst/b c.md" ] && [ -f "$ws/dst/folder/inner/x.txt" ] && [ ! -e "$ws/dst/x.txt" ] \
    && pass "$tag: まとめたコピーで 3 件が写り、配下を 2 回写さない" || fail "$tag: まとめたコピーの結果が想定と違う"
done

# (b) CLI で始めたまとめたコピーを MCP で読む: 1 つのジョブ・途中のバイト・残り時間・取り消し
BIG="$TMP/big-ws"
make_big "$BIG/one.bin" 16
make_big "$BIG/two.bin" 16
mkdir -p "$BIG/dst"
"$TAKO_BIN" file copy "$BIG/one.bin" "$BIG/two.bin" "$BIG/dst" > "$TMP/bg1.out" 2> "$TMP/bg1.err" &
BG_PID=$!
JOBS=""
PATHS=""
MID=""
ETA=""
BYTES_SEEN=""
for _ in $(seq 1 120); do
  P="$(mcp_text "$(mcp_call '{"op":"copy_progress"}' 0.3)")"
  n="$(printf '%s' "$P" | jq -r '.copies | length' 2>/dev/null)"
  if [ "${n:-0}" -ge 1 ]; then
    [ "$n" -gt "${JOBS:-0}" ] && JOBS="$n"
    PATHS="$(printf '%s' "$P" | jq -r '.copies[0].paths | length')"
    b="$(printf '%s' "$P" | jq -r '.copies[0].bytes_done')"
    e="$(printf '%s' "$P" | jq -r '.copies[0].entries_done')"
    BYTES_SEEN="$BYTES_SEEN $b"
    if [ "$e" = "0" ] && [ "$b" -gt 0 ] && [ "$b" -lt 16777216 ]; then MID="$b"; fi
    eta="$(printf '%s' "$P" | jq -r '.copies[0].eta_secs')"
    if [ "$eta" != "null" ] && [ -n "$MID" ]; then
      ETA="$eta"
      ID="$(printf '%s' "$P" | jq -r '.copies[0].id')"
      break
    fi
  fi
  sleep 0.1
done
echo "  観測: MCP copy_progress → $(printf '%s' "$P" | sed "s#${TMP}#<TMP>#g")"
echo "  観測: bytes_done の推移 →${BYTES_SEEN}"
check_eq "CLI の tako file copy a b dst は 1 つのジョブ（走っているコピーは 1 つ）" "1" "${JOBS:-0}"
check_eq "そのジョブの paths に 2 件とも載る" "2" "${PATHS:-0}"
[ -n "$MID" ] && pass "1 つのファイルの途中（件数 0）でバイトが進む（${MID} バイト）" \
  || fail "1 つのファイルの途中でバイトが進まない"
MONO=1
prev=0
for b in $BYTES_SEEN; do
  if [ "$b" -lt "$prev" ]; then MONO=0; fi
  prev="$b"
done
check_eq "バイトの進み具合は単調に増える" "1" "$MONO"
[ -n "$ETA" ] && pass "写し始めて 2 秒経つと eta_secs（残り時間の目安）が出る（${ETA} 秒）" \
  || fail "eta_secs が出ない"
CANCEL="$(mcp_text "$(mcp_call "{\"op\":\"copy_cancel\",\"name\":\"${ID:-0}\"}")")"
check_eq "MCP の copy_cancel が番号で取り消す" "[${ID:-}]" "$(printf '%s' "$CANCEL" | jq -c '.cancelled' 2>/dev/null)"
wait "$BG_PID"
RC=$?
BG_PID=""
check_eq "取り消された CLI のコピーは終了コード 1" "1" "$RC"
case "$(cat "$TMP/bg1.err")" in *取り消した*) pass "CLI のエラー文に「取り消した」" ;; *) fail "CLI のエラー文: $(cat "$TMP/bg1.err")" ;; esac
check_eq "取り消したら dst に何も残さない（作りかけの 1 ファイルも）" "0" "$(find "$BIG/dst" -mindepth 1 | wc -l | tr -d ' ')"

# (c) 逆: MCP の paths で始めたコピーを CLI で読み・取り消す
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  jq -n -c --arg a "$BIG/one.bin" --arg b "$BIG/two.bin" --arg d "$BIG/dst" \
    '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"tako_file_op",arguments:{op:"copy",paths:[$a,$b],dest:$d}}}'
  sleep 12
} | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null > "$TMP/bg2.out" &
BG_PID=$!
ID=""
for _ in $(seq 1 60); do
  P="$("$TAKO_BIN" file progress 2>/dev/null)"
  b="$(printf '%s' "$P" | jq -r '.copies[0].bytes_done // 0' 2>/dev/null)"
  if [ "${b:-0}" -gt 0 ]; then
    ID="$(printf '%s' "$P" | jq -r '.copies[0].id')"
    N="$(printf '%s' "$P" | jq -r '.copies | length')"
    break
  fi
  sleep 0.1
done
check_eq "CLI の tako file progress で MCP の paths のコピーが 1 つのジョブとして読める" "1" "${N:-0}"
CANCEL="$("$TAKO_BIN" file cancel "${ID:-0}" 2>&1)"
check_eq "CLI の tako file cancel が番号で取り消す" "[${ID:-}]" "$(printf '%s' "$CANCEL" | jq -c '.cancelled' 2>/dev/null)"
for _ in $(seq 1 50); do
  grep -q '"id":2' "$TMP/bg2.out" 2>/dev/null && break
  sleep 0.1
done
MCP_COPY="$(jq -r 'select(.id == 2) | .result.content[0].text' "$TMP/bg2.out")"
case "$MCP_COPY" in *取り消した*) pass "取り消された MCP のコピーは「取り消した」を返す" ;; *) fail "MCP のコピーの応答: $MCP_COPY" ;; esac
kill "$BG_PID" >/dev/null 2>&1 || true
wait "$BG_PID" 2>/dev/null
BG_PID=""
check_eq "MCP 側も dst に何も残さない" "0" "$(find "$BIG/dst" -mindepth 1 | wc -l | tr -d ' ')"
check_eq "取り消した後は走っているコピーが無い" '[]' "$("$TAKO_BIN" file progress | jq -c '.copies')"

echo
echo "== ⑤ エッジ: 取り消しの直後の再開・権限エラー・無いパス =="
# 取り消しの直後にもう一度写すと最後まで写る（同じ名前が空いている = 作りかけが無い）
OUT="$("$TAKO_BIN" file copy "$BIG/one.bin" "$BIG/dst" 2>&1)"
check_eq "取り消しの直後の再開は元の名前で最後まで写る" "16777216/false" \
  "$(printf '%s' "$OUT" | jq -r '"\(.bytes)/\(.renamed)"' 2>/dev/null)"
cmp -s "$BIG/one.bin" "$BIG/dst/one.bin" && pass "写した中身がコピー元と同じ" || fail "写した中身が違う"
E="$TMP/edge-ws"
mkdir -p "$E/src" "$E/locked"
printf 'a\n' > "$E/src/a.txt"
printf 'b\n' > "$E/src/b.txt"
chmod 555 "$E/locked"
OUT="$("$TAKO_BIN" file copy "$E/src/a.txt" "$E/src/b.txt" "$E/locked" 2>&1)"
RC=$?
echo "  観測: $(printf '%s' "$OUT" | sed "s#${TMP}#<TMP>#g")"
check_eq "書けないフォルダへのまとめたコピーは終了コード 1" "1" "$RC"
case "$OUT" in *"2 件すべて"*書き込めない*) pass "全部が理由つき（書き込めない）で断られる" ;; *) fail "権限エラーの文面: $OUT" ;; esac
chmod 755 "$E/locked"
check_eq "書けないフォルダに何も残さない" "0" "$(find "$E/locked" -mindepth 1 | wc -l | tr -d ' ')"
OUT="$(mcp_text "$(mcp_call '{"op":"copy","paths":[]}')")"
case "$OUT" in *"1 つ以上"*) pass "MCP: 空の paths は断る（0 件）" ;; *) fail "空の paths: $OUT" ;; esac
OUT="$(mcp_text "$(mcp_call "{\"op\":\"copy\",\"paths\":[\"$E/src/a.txt\"]}")")"
case "$OUT" in *dest*) pass "MCP: dest の無いまとめたコピーは断る" ;; *) fail "dest 無し: $OUT" ;; esac
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ④ A/B: TAKO_1895_LEGACY=1（#1895 の前）では 2 つのジョブに割れ、途中でバイトが進まない =="
# LEGACY の写し方（std::fs::copy）は 1 つのファイルの途中で止まらず一瞬で終わるので、#1867 の
# 項目ごとの遅延で「走っているジョブ」を観測できるようにする（途中のバイトは注入しても進まない）
start_gui "$TMP/data-legacy-gui" "$TMP/gui-legacy.log" TAKO_1895_LEGACY=1 TAKO_1867_COPY_DELAY_MS=1500
rm -rf "$BIG/dst"
mkdir -p "$BIG/dst"
TAKO_1895_LEGACY=1 "$TAKO_BIN" file copy "$BIG/one.bin" "$BIG/two.bin" "$BIG/dst" > /dev/null 2>&1 &
BG_PID=$!
L_PATHS=""
L_MID=""
L_ETA="false"
L_SEEN=""
# CLI が終わるまで見張る（1 件ずつ別の要求 = 別のジョブが順に載っては外れる）
while kill -0 "$BG_PID" 2>/dev/null; do
  P="$("$TAKO_BIN" file progress 2>/dev/null)"
  n="$(printf '%s' "$P" | jq -r '.copies | length' 2>/dev/null)"
  if [ "${n:-0}" -ge 1 ]; then
    k="$(printf '%s' "$P" | jq -r '.copies[0].paths | length')"
    [ "$k" -gt "${L_PATHS:-0}" ] && L_PATHS="$k"
    L_SEEN="$P"
    b="$(printf '%s' "$P" | jq -r '.copies[0].bytes_done')"
    e="$(printf '%s' "$P" | jq -r '.copies[0].entries_done')"
    if [ "$e" = "0" ] && [ "$b" -gt 0 ]; then L_MID="$b"; fi
    [ "$(printf '%s' "$P" | jq -r '.copies[0] | has("eta_secs")')" = "true" ] && L_ETA="true"
  fi
  sleep 0.1
done
P="$L_SEEN"
wait "$BG_PID" 2>/dev/null
BG_PID=""
echo "  観測: LEGACY の copy_progress → $(printf '%s' "$P" | sed "s#${TMP}#<TMP>#g")"
if [ -z "$L_PATHS" ]; then
  fail "LEGACY で走っているコピーを観測できない（A/B が成り立たない）"
elif [ "$L_PATHS" != "2" ]; then
  pass "LEGACY では「そのジョブの paths に 2 件とも載る」が落ちる（paths=${L_PATHS} = 1 件ずつ別のジョブ）"
else
  fail "LEGACY でも 1 つのジョブになった（検出力が無い）"
fi
if [ -n "$L_PATHS" ] && [ -z "$L_MID" ]; then
  pass "LEGACY では「1 つのファイルの途中でバイトが進む」が落ちる（写し終えるまで 0）"
else
  fail "LEGACY でも途中でバイトが進んだ（検出力が無い）"
fi
check_eq "LEGACY の copy_progress には eta_secs が無い（#1895 の前の形）" "false" "$L_ETA"
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -eq 0 ]; then
  tako_exit 0
fi
exit 1
