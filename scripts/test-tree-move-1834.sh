#!/bin/bash
# test-tree-move-1834.sh — ファイルツリーの D&D による移動（FR-3.32 / #1834）の実経路テスト
#
# 何を確かめるか（Issue #1834 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① visual-test `tree-move` 節: 隔離 GUI（tako-vd）で**実マウスの入口**からツリーの行を
#      ドラッグし、ファイル・フォルダが移る / 落とし先の札と移せない札が実ピクセルで出る /
#      編集中（未保存）のペインが付け替わり、待っても「外で削除された」にならない /
#      言語サーバ（偽）へ旧 URI の didClose と新 URI の didOpen が届く / リモートの行 /
#      ペインへの既存の D&D（分割して開く・パスを入れる）が壊れていない
#   ② A/B: `TAKO_1834_LEGACY=1`（付け替えを外す）の同じバイナリでは ① が FAILED になる
#   ③ 実 CLI `tako file move` と MCP `tako_file_op`（`op=move`）が同じ移動をし、
#      応答が**字面まで一致**する（パスとペイン番号だけを置き換えて比べる）。
#      断る場合（同名 / 自分の配下 / 自分自身）の理由も一致する
#   ④ 開いているファイル（未保存の編集中・フォルダの配下）が CLI の移動でも付け替わる
#   ⑤ エッジ: 別のボリューム（hdiutil の一時イメージ）/ シンボリックリンク /
#      日本語と空白のパス / 同じ場所 / dest の省略（MCP）
#
# 使い方: bash scripts/test-tree-move-1834.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。面を用意できなければ起動せず
# 終了コード 4 で「未実測」を返す（#1744）。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1834-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1834-$$"
MNT=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  if [ -n "$MNT" ]; then
    hdiutil detach "$MNT" -quiet -force >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako --bin tako-lsp-fake || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
FAKE="$(dirname "$APP_BIN")/tako-lsp-fake"
[ -x "$FAKE" ] || { echo "偽サーバが無い: $FAKE"; exit 1; }

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_1834_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP"

common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot"
  TAKO_PERSIST=0 TAKO_AUTORENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
)

# visual-test の節を 1 回走らせる（$1 = ログ / $2 = data dir / 残りは追加の env）
run_visual() {
  local log="$1" data="$2"
  shift 2
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$data" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tree-move TAKO_VISUAL_DUMP_DIR="$DUMP" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test tree-move（実マウスのドラッグ + 偽の言語サーバ） =="
run_visual "$TMP/visual.log" "$TMP/data-visual" \
  TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" TAKO_LSP_FAKE_LOG="$TMP/lsp.log"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: tree-move' "$TMP/visual.log" | sed 's/^/  観測: /'
grep -q 'tree-move ⑨ lsp swapped=true' "$TMP/visual.log" \
  && pass "言語サーバへ旧 URI の didClose と新 URI の didOpen が届く" \
  || fail "言語サーバの文書が付け替わらない"

echo
echo "== ② A/B: TAKO_1834_LEGACY=1 で付け替えが外れて FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" TAKO_1834_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" | sed 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test tree-move ①: 編集中のペインが新しいパスへ付け替わり' \
  "$TMP/legacy.log" \
  && pass "落ちた理由は編集中のペインが付け替わらないこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③〜⑤ 実 GUI + 実 CLI + MCP =="
# CLI も同じ隔離の受け口を見る（GUI へ渡した env を CLI 側にも置く）
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/data-gui"
mkdir -p "$TAKO_DATA_DIR"
for d in "$HOME" "$TAKO_DATA_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1

# 同じ形の fixture を 2 つ（A = CLI / B = MCP）。**必ず $TMP の中**
make_fixture() {
  local dir="$1"
  case "$dir" in "$TMP"/*) : ;; *) echo "fixture が一時 dir の外: $dir"; exit 1 ;; esac
  mkdir -p "$dir/src" "$dir/folder/inner" "$dir/dst" "$dir/taken" "$dir/日本 語"
  printf 'A\n' > "$dir/src/a.txt"
  printf '# b\n' > "$dir/src/b.md"
  printf 'C\n' > "$dir/src/c.txt"
  printf 'E\n' > "$dir/src/e.txt"
  printf 'X\n' > "$dir/folder/inner/x.txt"
  printf '既存\n' > "$dir/taken/a.txt"
}
A="$TMP/cli-ws"
B="$TMP/mcp-ws"
make_fixture "$A"
make_fixture "$B"

ROOT="$("$TAKO_BIN" list 2>/dev/null | jq -r '.tabs[0].panes[0].id')"
case "$ROOT" in ''|*[!0-9]*) echo "基準ペインが取れない"; exit 1 ;; esac

# a.txt を編集中・未保存（自動保存 OFF）、x.txt を表示だけで開く
open_pair() {
  local ws="$1" pa px
  pa="$("$TAKO_BIN" open "$ws/src/a.txt" --pane "$ROOT" --down 2>/dev/null | jq -r '.pane')"
  "$TAKO_BIN" edit start --pane "$pa" >/dev/null
  "$TAKO_BIN" edit autosave false --pane "$pa" >/dev/null
  "$TAKO_BIN" edit apply "$(printf 'A\nunsaved')" --pane "$pa" >/dev/null
  px="$("$TAKO_BIN" open "$ws/folder/inner/x.txt" --pane "$pa" --right 2>/dev/null | jq -r '.pane')"
  echo "$pa $px"
}
read -r A_EDIT A_VIEW <<EOF
$(open_pair "$A")
EOF
read -r B_EDIT B_VIEW <<EOF
$(open_pair "$B")
EOF
echo "  CLI 側のペイン: 編集 $A_EDIT / 表示 ${A_VIEW}、MCP 側: 編集 ${B_EDIT} / 表示 ${B_VIEW}"
AUTOSAVE="$("$TAKO_BIN" edit autosave --pane "$A_EDIT" | jq -r '.autosave')"
check_eq "前提: 編集ペインの自動保存が OFF（未保存のまま移す）" "false" "$AUTOSAVE"

# 1 件の応答を字面比較用に正規化する（パス → <WS>、ペイン番号 → <PANE>）
normalize() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  jq -S -c --arg ws "$ws" --arg real "$real" '
    def sw: if type == "string" then (split($real) | join("<WS>") | split($ws) | join("<WS>")) else . end;
    walk(sw) | if type == "object" and has("followed") then .followed |= map(.pane = "<PANE>") else . end'
}
normalize_text() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  sed -e "s#${real}#<WS>#g" -e "s#${ws}#<WS>#g" -e 's/^error: //'
}

# 同じ操作列（「移す元 / 移動先」の相対パス）
OPS=(
  "src/a.txt|dst"
  "folder|dst"
  "dst/a.txt|taken"
  "dst/folder|dst/folder/inner"
  "dst|dst"
  "src/b.md|日本 語"
  "src/e.txt|src"
)

# CLI（A）
i=0
for op in ${OPS[@]+"${OPS[@]}"}; do
  src="${op%%|*}"
  dst="${op#*|}"
  i=$((i + 1))
  if out="$("$TAKO_BIN" file move "$A/$src" "$A/$dst" 2>"$TMP/cli-$i.err")"; then
    printf '%s' "$out" | normalize "$A" > "$TMP/cli-$i.norm"
  else
    normalize_text "$A" < "$TMP/cli-$i.err" | tr -d '\n' > "$TMP/cli-$i.norm"
  fi
done

# MCP（B）: 1 本のセッションで同じ操作列を投げる
SOCK="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  i=0
  for op in ${OPS[@]+"${OPS[@]}"}; do
    src="${op%%|*}"
    dst="${op#*|}"
    i=$((i + 1))
    jq -n -c --argjson id "$((i + 1))" --arg path "$B/$src" --arg dest "$B/$dst" \
      '{jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:"tako_file_op",arguments:{op:"move",path:$path,dest:$dest}}}'
    # 順に処理させる（応答を待ってから次の移動 = CLI と同じ並び）
    sleep 0.4
  done
  jq -n -c --arg path "$B/src/c.txt" \
    '{jsonrpc:"2.0",id:99,method:"tools/call",params:{name:"tako_file_op",arguments:{op:"move",path:$path}}}'
  sleep 2
} | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)" \
    "$TAKO_BIN" mcp serve 2>/dev/null > "$TMP/mcp.out"
i=0
for op in ${OPS[@]+"${OPS[@]}"}; do
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
for op in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  echo "  [$i] ${op%%|*} → ${op#*|}"
  echo "      CLI: $(cat "$TMP/cli-$i.norm")"
  echo "      MCP: $(cat "$TMP/mcp-$i.norm")"
  if [ -s "$TMP/cli-$i.norm" ] && cmp -s "$TMP/cli-$i.norm" "$TMP/mcp-$i.norm"; then
    pass "[$i] CLI と MCP の応答が字面まで一致"
  else
    fail "[$i] CLI と MCP の応答が食い違う"
  fi
done

check_eq "CLI [1] ファイルを移す" '{"followed":[{"from":"<WS>/src/a.txt","pane":"<PANE>","to":"<WS>/dst/a.txt"}],"from":"<WS>/src/a.txt","kind":"file","moved":true,"to":"<WS>/dst/a.txt"}' "$(cat "$TMP/cli-1.norm")"
check_eq "CLI [2] フォルダを移すと配下の表示ペインが付け替わる" '{"followed":[{"from":"<WS>/folder/inner/x.txt","pane":"<PANE>","to":"<WS>/dst/folder/inner/x.txt"}],"from":"<WS>/folder","kind":"dir","moved":true,"to":"<WS>/dst/folder"}' "$(cat "$TMP/cli-2.norm")"
case "$(cat "$TMP/cli-3.norm")" in
  *同じ名前*) pass "CLI [3] 同名は理由つきで断る" ;;
  *) fail "CLI [3] 同名の断り方: $(cat "$TMP/cli-3.norm")" ;;
esac
check_eq "同名の移動先を上書きしない" "既存" "$(cat "$A/taken/a.txt")"
case "$(cat "$TMP/cli-4.norm")" in
  *配下*) pass "CLI [4] 自分の配下へは理由つきで断る" ;;
  *) fail "CLI [4] 配下の断り方: $(cat "$TMP/cli-4.norm")" ;;
esac
case "$(cat "$TMP/cli-5.norm")" in
  *自分自身*) pass "CLI [5] 自分自身へは理由つきで断る" ;;
  *) fail "CLI [5] 自分自身の断り方: $(cat "$TMP/cli-5.norm")" ;;
esac
[ -f "$A/日本 語/b.md" ] && pass "CLI [6] 日本語と空白を含むフォルダへ移せる" || fail "CLI [6] 日本語のフォルダへ移らない"
check_eq "CLI [7] 同じ場所は何もしない" "false" "$(jq -r '.moved' "$TMP/cli-7.norm")"
MISSING="$(jq -r 'select(.id == 99) | .result.content[0].text' "$TMP/mcp.out")"
case "$MISSING" in
  *dest*) pass "MCP dest の省略は理由つきで断る" ;;
  *) fail "MCP dest の省略: $MISSING" ;;
esac

echo
echo "== ④ 開いているペインの付け替え（CLI の移動のあと 1.5 秒待つ） =="
sleep 1.5
# パスと未保存は `tako list`、競合は `tako edit status`（競合していなければキーごと無い）
pane_state() {
  local conflict
  conflict="$("$TAKO_BIN" edit status --pane "$1" 2>/dev/null | jq -c '.conflict // null')"
  "$TAKO_BIN" list | jq -S -c --argjson p "$1" --argjson conflict "${conflict:-null}" \
    '[.tabs[].panes[] | select(.id == $p)][0].preview | {path, dirty, conflict: $conflict}'
}
for side in "A $A_EDIT $A_VIEW" "B $B_EDIT $B_VIEW"; do
  set -- $side
  ws="$A"; [ "$1" = "B" ] && ws="$B"
  real="$(cd "$ws" && pwd -P)"
  edit_state="$(pane_state "$2" | sed -e "s#${real}#<WS>#g" -e "s#${ws}#<WS>#g")"
  view_state="$(pane_state "$3" | sed -e "s#${real}#<WS>#g" -e "s#${ws}#<WS>#g")"
  echo "  $1 編集ペイン: $edit_state"
  echo "  $1 表示ペイン: $view_state"
  check_eq "$1: 編集ペインは新しいパスで未保存のまま・競合なし" \
    '{"conflict":null,"dirty":true,"path":"<WS>/dst/a.txt"}' "$edit_state"
  check_eq "$1: フォルダの配下を表示しているペインも付け替わる" \
    '"<WS>/dst/folder/inner/x.txt"' "$(printf '%s' "$view_state" | jq -c '.path')"
done
check_eq "移動は保存しない（ディスクは移す前の中身）" "A" "$(cat "$A/dst/a.txt")"

echo
echo "== ⑤ エッジ: シンボリックリンク / 別のボリューム =="
ln -s "$A/src/c.txt" "$A/src/link.txt"
LINK_OUT="$("$TAKO_BIN" file move "$A/src/link.txt" "$A/dst" 2>&1)"
check_eq "シンボリックリンクは kind=symlink" "symlink" "$(printf '%s' "$LINK_OUT" | jq -r '.kind')"
if [ -L "$A/dst/link.txt" ] && [ -f "$A/src/c.txt" ]; then
  pass "リンクそのものが移り、指す先は動かない"
else
  fail "リンクの移動が想定と違う"
fi

DMG="$TMP/vol.dmg"
if hdiutil create -size 4m -fs HFS+ -volname tako1834 "$DMG" -quiet >/dev/null 2>&1 \
  && hdiutil attach "$DMG" -mountpoint "$TMP/vol" -nobrowse -quiet >/dev/null 2>&1; then
  MNT="$TMP/vol"
  XDEV_ERR="$("$TAKO_BIN" file move "$A/src/c.txt" "$MNT" 2>&1)"
  XDEV_RC=$?
  echo "  観測: $(printf '%s' "$XDEV_ERR" | normalize_text "$A" | sed "s#${TMP}#<TMP>#g")"
  if [ "$XDEV_RC" -ne 0 ] && printf '%s' "$XDEV_ERR" | grep -q '別のボリューム'; then
    pass "別のボリュームへは理由つきで断る（EXDEV）"
  else
    fail "別のボリュームの断り方が想定と違う（rc=${XDEV_RC}）"
  fi
  if [ -f "$A/src/c.txt" ] && [ ! -e "$MNT/c.txt" ]; then
    pass "断ったら元は残り、移動先に半端なものが残らない"
  else
    fail "別のボリュームで何かが動いた"
  fi
  hdiutil detach "$MNT" -quiet -force >/dev/null 2>&1 || true
  MNT=""
else
  echo "  未実測: hdiutil で一時イメージを作れない（別のボリュームの検査を飛ばす）"
fi

stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
