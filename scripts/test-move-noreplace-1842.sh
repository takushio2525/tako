#!/bin/bash
# test-move-noreplace-1842.sh — 移動の上書きしない rename と最近使った場所の付け替え（FR-3.32 / #1842）の実経路テスト
#
# 何を確かめるか（Issue #1842 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① 隙間の注入（`TAKO_1842_INJECT_RACE`）: 最後の確認の後・rename の直前に同名ができても、
#      CLI `tako file move` と MCP `tako_file_op`（`op=move`）が上書きせずに断り、理由が応答に出る。
#      元のファイルも割り込んだ同名も中身が残る（ファイル・空のフォルダの両方）
#   ② A/B: `TAKO_1842_LEGACY=1`（見てから `std::fs::rename` する修正前の形）の同じバイナリでは、
#      同じ注入で割り込んだ同名が上書きされ、最近使った場所は付け替わらない
#   ③ 最近使った場所: `tako open-in dir` / MCP `tako_open_dir` で記録したフォルダ（と配下）を CLI / MCP で
#      移すと、`tako recent list` / MCP `tako_recent` の項目が新しいパスを指し、そこから開ける。
#      ファイルの移動は一覧を変えない
#
# 使い方: bash scripts/test-move-noreplace-1842.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。面を用意できなければ起動せず
# 終了コード 4 で「未実測」を返す（#1744）。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
# $2 に $3 が含まれるか（パイプを使わない = pipefail の下で取りこぼさない）
check_has() {
  if grep -qF -- "$3" <<<"$2"; then pass "$1"; else fail "$1（'${3}' が無い: ${2}）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1842-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKETS=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  local s
  for s in $TMUX_SOCKETS; do
    tmux -L "$s" kill-server >/dev/null 2>&1 || true
  done
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/zdot"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 隙間に同名を作る移動（移した後の名前で当てる）
INJECT="race.txt,race-dir,race-mcp.txt"

# 隔離 GUI を 1 つ起こし、CLI も同じ受け口へ向ける。**回ごとに data / discovery / tmux を分ける**
# （共有すると 2 回目が前の器を拾う = #2157）。$1 = 回の名前 / 残りは GUI へ渡す追加の env
start_gui() {
  local name="$1"
  shift
  local base="$TMP/$name"
  mkdir -p "$base/home" "$base/disc" "$base/orch" "$base/data"
  TMUX_SOCKETS="$TMUX_SOCKETS tako-1842-$name-$$"
  export TAKO_ISOLATED=1 HOME="$base/home" ZDOTDIR="$TMP/zdot" \
    TAKO_DISCOVERY_DIR="$base/disc" TAKO_ORCHESTRATOR_DIR="$base/orch" \
    TAKO_SESSIONS_FILE="$base/sessions.yaml" TAKO_PANE_LOG_DIR="$base/panelogs" \
    TAKO_WORKERS_FILE="$base/workers.yaml" TAKO_TMUX_SOCKET="tako-1842-$name-$$" \
    TAKO_DATA_DIR="$base/data"
  local d
  for d in "$HOME" "$TAKO_DATA_DIR"; do
    case "$d" in
      "$TMP"/*) : ;;
      *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
    esac
  done
  launch_isolated_gui "$base/gui.log" \
    HOME="$HOME" ZDOTDIR="$ZDOTDIR" TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" TAKO_ORCHESTRATOR_DIR="$TAKO_ORCHESTRATOR_DIR" \
    TAKO_SESSIONS_FILE="$TAKO_SESSIONS_FILE" TAKO_PANE_LOG_DIR="$TAKO_PANE_LOG_DIR" \
    TAKO_WORKERS_FILE="$TAKO_WORKERS_FILE" TAKO_TMUX_SOCKET="$TAKO_TMUX_SOCKET" \
    TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_1842_INJECT_RACE="$INJECT" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$base/gui.log" || exit 1
}

stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# fixture。**必ず $TMP の中**
make_fixture() {
  local ws="$1"
  case "$ws" in "$TMP"/*) : ;; *) echo "fixture が一時 dir の外: $ws"; exit 1 ;; esac
  mkdir -p "$ws/src/race-dir/inner" "$ws/dst" "$ws/proj/sub" "$ws/outer/inner"
  printf '元の中身\n' > "$ws/src/race.txt"
  printf 'MCP の元\n' > "$ws/src/race-mcp.txt"
  printf 'メモ\n' > "$ws/note.txt"
}

# MCP の tools/call を 1 回（1 本のセッション）。結果（.result）を JSON で出す
mcp_call() {
  local tool="$1" args="$2" sock out
  sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  out="$TMP/mcp-$$-$RANDOM.out"
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    jq -n -c --arg tool "$tool" --argjson args "$args" \
      '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:$tool,arguments:$args}}'
    sleep 1.5
  } | TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)" \
      "$TAKO_BIN" mcp serve 2>/dev/null > "$out"
  jq -c 'select(.id == 2) | .result' "$out"
}

# 最近使った場所のフォルダの一覧（1 行 1 パス）
recent_paths_cli() {
  "$TAKO_BIN" recent list | jq -r '.entries[] | select(.type == "directory" or .type == "repository") | .path'
}

echo "== ① 新しい形: 隙間に同名ができても上書きせずに断る（TAKO_1842_INJECT_RACE=${INJECT}） =="
start_gui new
W="$TMP/new/ws"
make_fixture "$W"

ERR="$("$TAKO_BIN" file move "$W/src/race.txt" "$W/dst" 2>&1)"
RC=$?
echo "  観測（CLI ファイル）: rc=${RC} $(sed "s#${TMP}#<TMP>#g" <<<"$ERR")"
[ "$RC" -ne 0 ] && pass "CLI: 隙間に同名ができた移動は失敗で返る" || fail "CLI: 隙間に同名ができたのに移した"
check_has "CLI: 断った理由が応答に出る" "$ERR" "移動先に同じ名前がある（上書きしない）"
check_eq "CLI: 元のファイルが元の場所に残る" "元の中身" "$(cat "$W/src/race.txt" 2>/dev/null)"
check_eq "CLI: 割り込んだ同名のファイルの中身が残る" "tako-1842-race" "$(cat "$W/dst/race.txt" 2>/dev/null)"

ERR="$("$TAKO_BIN" file move "$W/src/race-dir" "$W/dst" 2>&1)"
RC=$?
echo "  観測（CLI フォルダ）: rc=${RC} $(sed "s#${TMP}#<TMP>#g" <<<"$ERR")"
[ "$RC" -ne 0 ] && pass "CLI: 空のフォルダが割り込んだ移動も失敗で返る" || fail "CLI: 空のフォルダを置き換えて移した"
check_has "CLI: フォルダでも断った理由が出る" "$ERR" "同じ名前"
[ -d "$W/src/race-dir/inner" ] && pass "CLI: 元のフォルダが中身ごと残る" || fail "CLI: 元のフォルダが消えた"
if [ -d "$W/dst/race-dir" ] && [ ! -e "$W/dst/race-dir/inner" ]; then
  pass "CLI: 割り込んだ空のフォルダがそのまま残る"
else
  fail "CLI: 割り込んだ空のフォルダが置き換わった"
fi

RES="$(mcp_call tako_file_op "$(jq -n -c --arg p "$W/src/race-mcp.txt" --arg d "$W/dst" '{op:"move",path:$p,dest:$d}')")"
echo "  観測（MCP）: $(sed "s#${TMP}#<TMP>#g" <<<"$RES")"
check_eq "MCP: 隙間に同名ができた移動はエラーで返る" "true" "$(jq -r '.isError // false' <<<"$RES")"
check_has "MCP: 断った理由が応答に出る" "$(jq -r '.content[0].text' <<<"$RES")" "移動先に同じ名前がある（上書きしない）"
check_eq "MCP: 元のファイルが残る" "MCP の元" "$(cat "$W/src/race-mcp.txt" 2>/dev/null)"
check_eq "MCP: 割り込んだ同名の中身が残る" "tako-1842-race" "$(cat "$W/dst/race-mcp.txt" 2>/dev/null)"

echo
echo "== ③ 新しい形: 最近使った場所が移動先へ付け替わり、そこから開ける =="
# CLI: proj（と配下の sub）を記録してから proj を移す
"$TAKO_BIN" open-in dir "$W/proj/sub" --no-focus >/dev/null
"$TAKO_BIN" open-in dir "$W/proj" --no-focus >/dev/null
BEFORE="$(recent_paths_cli)"
check_has "前提: 記録した proj が一覧にある" "$BEFORE" "$W/proj"
OUT="$("$TAKO_BIN" file move "$W/proj" "$W/dst" 2>&1)"
echo "  観測（CLI フォルダの移動）: $(sed "s#${TMP}#<TMP>#g" <<<"$OUT")"
check_eq "CLI: 応答の recent_followed は 2（proj と配下の sub）" "2" "$(jq -r '.recent_followed // 0' <<<"$OUT" 2>/dev/null)"
AFTER="$(recent_paths_cli)"
echo "  観測（tako recent list）: $(tr '\n' ' ' <<<"$AFTER" | sed "s#${TMP}#<TMP>#g")"
TOP2="$(sed -n '1,2p' <<<"$AFTER")"
check_eq "CLI: 一覧の先頭 2 件が移動先を指す（並びはそのまま）" \
  "$W/dst/proj $W/dst/proj/sub" "$(tr '\n' ' ' <<<"$TOP2" | sed 's/ $//')"
if grep -qxF -- "$W/proj" <<<"$AFTER"; then fail "CLI: 古いパスが一覧に残る"; else pass "CLI: 古いパスは一覧から消える"; fi
OPEN="$("$TAKO_BIN" open-in dir "$W/dst/proj" --no-focus 2>&1)"
RC=$?
[ "$RC" -eq 0 ] && pass "CLI: 付け替えた項目のパスから開ける（open-in dir rc=0）" || fail "CLI: 付け替えた項目から開けない: ${OPEN}"
check_eq "CLI: 開き直しても一覧に重複が出ない" "1" "$(grep -cxF -- "$W/dst/proj" <<<"$(recent_paths_cli)")"

# ファイルの移動は一覧を変えない（ファイルは記録されない）
BEFORE="$(recent_paths_cli)"
OUT="$("$TAKO_BIN" file move "$W/note.txt" "$W/dst" 2>&1)"
check_eq "CLI: ファイルの移動は応答に recent_followed を載せない" "null" "$(jq -c '.recent_followed' <<<"$OUT" 2>/dev/null)"
check_eq "CLI: ファイルの移動は一覧を変えない" "$BEFORE" "$(recent_paths_cli)"

# MCP: outer（と配下の inner）を記録してから outer を移す
mcp_call tako_open_dir "$(jq -n -c --arg p "$W/outer/inner" '{path:$p,focus:false}')" >/dev/null
mcp_call tako_open_dir "$(jq -n -c --arg p "$W/outer" '{path:$p,focus:false}')" >/dev/null
RES="$(mcp_call tako_file_op "$(jq -n -c --arg p "$W/outer" --arg d "$W/dst" '{op:"move",path:$p,dest:$d}')")"
TEXT="$(jq -r '.content[0].text' <<<"$RES")"
echo "  観測（MCP フォルダの移動）: $(sed "s#${TMP}#<TMP>#g" <<<"$TEXT")"
check_eq "MCP: 応答の recent_followed は 2（outer と配下の inner）" "2" "$(jq -r '.recent_followed // 0' <<<"$TEXT" 2>/dev/null)"
RES="$(mcp_call tako_recent '{"action":"list"}')"
LIST="$(jq -r '.content[0].text' <<<"$RES" | jq -r '.entries[] | select(.type == "directory") | .path')"
echo "  観測（MCP tako_recent）: $(tr '\n' ' ' <<<"$LIST" | sed "s#${TMP}#<TMP>#g")"
check_has "MCP: 一覧が移動先の outer を指す" "$LIST" "$W/dst/outer"
check_has "MCP: 一覧が移動先の outer/inner を指す" "$LIST" "$W/dst/outer/inner"
if grep -qxF -- "$W/outer" <<<"$LIST"; then fail "MCP: 古いパスが一覧に残る"; else pass "MCP: 古いパスは一覧から消える"; fi
RES="$(mcp_call tako_open_dir "$(jq -n -c --arg p "$W/dst/outer/inner" '{path:$p,focus:false}')")"
check_eq "MCP: 付け替えた項目のパスから開ける（tako_open_dir）" "false" "$(jq -r '.isError // false' <<<"$RES")"
stop_gui

echo
echo "== ② A/B: TAKO_1842_LEGACY=1（見てから rename）では同じ注入で上書きされ、一覧は付け替わらない =="
start_gui legacy TAKO_1842_LEGACY=1
W="$TMP/legacy/ws"
make_fixture "$W"
OUT="$("$TAKO_BIN" file move "$W/src/race.txt" "$W/dst" 2>&1)"
RC=$?
echo "  観測（旧形のファイル）: rc=${RC} $(sed "s#${TMP}#<TMP>#g" <<<"$OUT")"
check_eq "旧形: 移動は成功で返る（割り込みに気づかない）" "0" "$RC"
check_eq "旧形: 割り込んだ同名のファイルが上書きされる（tako-1842-race が消える）" "元の中身" "$(cat "$W/dst/race.txt" 2>/dev/null)"
OUT="$("$TAKO_BIN" file move "$W/src/race-dir" "$W/dst" 2>&1)"
RC=$?
echo "  観測（旧形のフォルダ）: rc=${RC} $(sed "s#${TMP}#<TMP>#g" <<<"$OUT")"
[ -d "$W/dst/race-dir/inner" ] && pass "旧形: 割り込んだ空のフォルダが置き換えられる" || fail "旧形: 空のフォルダが置き換わらない（A/B が取れない）"
"$TAKO_BIN" open-in dir "$W/proj" --no-focus >/dev/null
OUT="$("$TAKO_BIN" file move "$W/proj" "$W/dst" 2>&1)"
check_eq "旧形: 応答に recent_followed が無い" "null" "$(jq -c '.recent_followed' <<<"$OUT" 2>/dev/null)"
AFTER="$(recent_paths_cli)"
check_has "旧形: 一覧は古いパスのまま" "$AFTER" "$W/proj"
OPEN="$("$TAKO_BIN" open-in dir "$W/proj" --no-focus 2>&1)"
RC=$?
echo "  観測（旧形の古い項目を開く）: rc=${RC} $(sed "s#${TMP}#<TMP>#g" <<<"$OPEN")"
[ "$RC" -ne 0 ] && pass "旧形: 古い項目からは開けない" || fail "旧形: 古い項目から開けた"
stop_gui

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
