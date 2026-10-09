#!/bin/bash
# test-tree-keys-1908.sh — ファイルツリーの ↑ / ↓ / ← / → / Enter / ⇧⌘↑ / ⇧⌘↓（Windows は
# Shift+Ctrl+Home / End）・選択の CLI / MCP・コピーの帯の残り時間の数え下ろし（FR-3.40 / #1908）の
# 実経路テスト
#
# 何を確かめるか（Issue #1908 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① visual-test `tree-keys` 節: 隔離 GUI（tako-vd）で**実キー**（GPUI のキー配送）の入口から
#      ↑ / ↓ で選んだ行が 1 行ずつ動く（先頭・末尾で止まる）・← / → で畳む・親へ / 開く・子へ
#      （空のフォルダ）・Enter で開く / 開閉・⇧⌘↑ / ⇧⌘↓ で端まで（畳んだフォルダの中は入らない）・
#      リモートの行の上ではどのキーも動かさない・一定の速さのコピーで帯の残り時間が戻らない
#   ② A/B: `TAKO_1908_LEGACY=1`（#1908 の前）の同じバイナリでは ① が名指しで FAILED
#      （↓ がツリーへ向かずペインへ流れる）
#   ③ 実 CLI `tako tree selection` と MCP `tako_tree_folder` の `selection`: 同じ操作列（閉じた
#      ツリー・選ぶ・→ ・↓ ・端まで・← ・Enter・見えていない行・知らない key）の応答が字面まで一致
#   ④ 実 GUI の一定の速さのコピー（`TAKO_1895_COPY_CHUNK_DELAY_MS`）で `tako file progress` の
#      `eta_secs` を約 150 ms ごとに読む: 新しい式は戻らない（逆戻り 1 回以下）、
#      `TAKO_1908_LEGACY=1` の GUI はのこぎり状に戻る（2 回以上）。CLI / MCP の選択は
#      `TAKO_1908_LEGACY=1` でも同じ経路のまま効く
#
# 使い方: bash scripts/test-tree-keys-1908.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# OS のクリップボードは名前付きペーストボード、ゴミ箱は `TAKO_TRASH_DIR` = 一時 dir、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。大きなファイルは 40 MiB を一時 dir に
# 作り（CPU を焼く負荷は使わない = 遅さは `TAKO_1895_COPY_CHUNK_DELAY_MS` の注入）、終わったら消す。
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装）。
# 面を用意できなければ起動せず終了コード 4 で「未実測」を返す。**CI には載せない**（実 GUI が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_FILE_PASTEBOARD TAKO_TRASH_DIR TAKO_1867_COPY_DELAY_MS TAKO_1867_LEGACY \
  TAKO_1895_COPY_CHUNK_DELAY_MS TAKO_1895_LEGACY TAKO_1908_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1908-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
BG_PID=""
TMUX_SOCKET="tako-1908-$$"
PB_NAME="tako-1908-$$"
# 1 つのファイルの中身を写す単位（macOS は 1 MiB）ごとの遅延（ミリ秒）= 一定の速さのコピー
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
tako_exit_trap cleanup "test-tree-keys-1908"

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/tmpdir" "$TMP/trash"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
# 検証の画像は新しい挙動と旧挙動で書き出し先を分ける
DUMP="${TAKO_1908_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP/new" "$DUMP/legacy"

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
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tree-keys \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test tree-keys（実キー） =="
run_visual "$TMP/visual.log" "$TMP/data-visual" TAKO_VISUAL_DUMP_DIR="$DUMP/new"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: tree-keys' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'

echo
echo "== ② A/B: TAKO_1908_LEGACY=1 で ↓ がツリーへ向かず FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" TAKO_1908_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/legacy"
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test tree-keys ①: ↓ で 1 行下へ動く' "$TMP/legacy.log" \
  && pass "落ちた理由は ↓ で選んだ行が動かないこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（tako tree selection と tako_tree_folder の selection の字面一致） =="
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
stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  ISOLATED_GUI_PID=""
}
mcp_call() {
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    jq -n -c --arg tool "$1" --argjson args "$2" '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:$tool,arguments:$args}}'
    sleep "${3:-0.5}"
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
make_fixture() {
  local dir="$1"
  case "$dir" in "$TMP"/*) : ;; *) echo "fixture が一時 dir の外: $dir"; exit 1 ;; esac
  mkdir -p "$dir/keys/sub" "$dir/keys/zz"
  printf 'A\n' > "$dir/keys/a.txt"
  printf 'B\n' > "$dir/keys/b.txt"
  printf 'C\n' > "$dir/keys/c.txt"
  printf 'X\n' > "$dir/keys/sub/x.txt"
  printf 'Z\n' > "$dir/keys/zz/z.txt"
}
# 応答の字面からワークスペースのパスと開いたペインの番号（実行ごとに違う）を伏せる
normalize() {
  local ws="$1"
  jq -S -c --arg ws "$ws" '
    def sw: if type == "string" then (split($ws) | join("<WS>")) else . end;
    walk(sw) | if (type == "object" and has("opened")) then .opened.pane = "<PANE>" else . end'
}
normalize_text() {
  sed -e "s#${1}#<WS>#g" -e 's/^error: //'
}

start_gui "$TMP/data-gui" "$TMP/gui.log"

A="$TMP/cli-ws"
B="$TMP/mcp-ws"
make_fixture "$A"
make_fixture "$B"
# 同じ操作列（show = 読むだけ / path:<ワークスペースからの相対。空 = ワークスペースそのもの> /
# key:<名前>）。closed: はツリーを閉じたまま撃つ
OPS=(
  "closed:key:down"
  "closed:show"
  "path:"
  "key:right"
  "path:keys"
  "key:right"
  "key:right"
  "key:down"
  "key:down"
  "key:extend_bottom"
  "key:extend_top"
  "key:extend_down"
  "key:up"
  "path:keys/sub/x.txt"
  "path:keys/sub"
  "key:enter"
  "key:right"
  "key:left"
  "key:left"
  "key:left"
  "path:keys/b.txt"
  "key:enter"
  "show"
  "key:bogus"
)
# $1 = CLI / MCP、$2 = ワークスペース
run_ops() {
  local via="$1" ws="$2" i=0 spec rest closed args out res opened
  # ツリーを開いた状態でワークスペースを足す（`FileTree::set_roots` は新しいルートを先頭へ・
  # 既にあるルートを後ろへ置くので、閉じたまま足すと回ごとにルートの並びが変わり、
  # 端まで（extend_top / extend_bottom）の範囲が比べられなくなる）
  "$TAKO_BIN" panel --filetree on > /dev/null 2>&1
  "$TAKO_BIN" tree add "$ws" > /dev/null
  "$TAKO_BIN" panel --filetree off > /dev/null 2>&1
  OPENED_PANE=""
  for spec in ${OPS[@]+"${OPS[@]}"}; do
    i=$((i + 1))
    closed=0
    rest="$spec"
    case "$rest" in closed:*) closed=1; rest="${rest#closed:}" ;; esac
    if [ "$closed" = 0 ] && [ "$i" -gt 1 ]; then
      "$TAKO_BIN" panel --filetree on > /dev/null 2>&1
    fi
    case "$rest" in
      show) args='{"action":"selection"}'; set -- ;;
      path:*)
        local rel="${rest#path:}" p="$ws"
        [ -n "$rel" ] && p="$ws/$rel"
        args="$(jq -n -c --arg p "$p" '{action:"selection",path:$p}')"
        set -- "$p"
        ;;
      key:*)
        args="$(jq -n -c --arg k "${rest#key:}" '{action:"selection",key:$k}')"
        set -- --key "${rest#key:}"
        ;;
    esac
    if [ "$via" = CLI ]; then
      if out="$("$TAKO_BIN" tree selection ${1+"$@"} 2>"$TMP/$via-$i.err")"; then
        printf '%s' "$out" | normalize "$ws" > "$TMP/$via-$i.norm"
      else
        normalize_text "$ws" < "$TMP/$via-$i.err" | tr -d '\n' > "$TMP/$via-$i.norm"
      fi
    else
      res="$(mcp_call tako_tree_folder "$args")"
      if [ "$(printf '%s' "$res" | jq -r '.isError // false')" = "true" ]; then
        mcp_text "$res" | normalize_text "$ws" | tr -d '\n' > "$TMP/$via-$i.norm"
      else
        out="$(mcp_text "$res")"
        printf '%s' "$out" | normalize "$ws" > "$TMP/$via-$i.norm"
      fi
    fi
    opened="$(printf '%s' "${out:-}" | jq -r '.opened.pane // empty' 2>/dev/null)"
    [ -n "$opened" ] && OPENED_PANE="$opened"
    out=""
  done
  # 次の回を同じ状態から始める（開いたプレビューを閉じ、ワークスペースを外す）
  [ -n "$OPENED_PANE" ] && "$TAKO_BIN" close --pane "$OPENED_PANE" --force > /dev/null 2>&1
  "$TAKO_BIN" tree remove "$ws" > /dev/null
}
run_ops CLI "$A"
run_ops MCP "$B"
i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  echo "  [$i] ${spec}"
  echo "      CLI: $(sed -e "s#${TMP}#<TMP>#g" "$TMP/CLI-$i.norm")"
  echo "      MCP: $(sed -e "s#${TMP}#<TMP>#g" "$TMP/MCP-$i.norm")"
  if [ -s "$TMP/CLI-$i.norm" ] && cmp -s "$TMP/CLI-$i.norm" "$TMP/MCP-$i.norm"; then
    pass "[$i] CLI と MCP の応答が字面まで一致"
  else
    fail "[$i] CLI と MCP の応答が食い違う"
  fi
done
# 中身の要所（字面一致は「同じ間違い」でも通るので、期待そのものも見る）
case "$(cat "$TMP/CLI-1.norm")" in *"tako panel --filetree on"*) pass "閉じたツリーのキーは開き方を添えて断る" ;; *) fail "閉じたツリーの断り方: $(cat "$TMP/CLI-1.norm")" ;; esac
check_eq "閉じたツリーの選択は null" "null" "$(jq -c '.selection' "$TMP/CLI-2.norm")"
check_eq "→ で keys が開く" '{"expanded":true,"path":"<WS>/keys"}' "$(jq -S -c '.expanded' "$TMP/CLI-6.norm")"
check_eq "開いたフォルダの → で最初の子（sub）へ" '"<WS>/keys/sub"' "$(jq -c '.selection.lead' "$TMP/CLI-7.norm")"
check_eq "↓ ↓ で a.txt（フォルダの後）" '"<WS>/keys/a.txt"' "$(jq -c '.selection.lead' "$TMP/CLI-9.norm")"
# 末尾の行はワークスペースの後ろに並ぶルート（ペインの cwd）次第なので、起点から続く 3 行と
# 「最後に押した行 = 範囲の最後」だけを見る
check_eq "extend_bottom は起点 a.txt から末尾まで" '["<WS>/keys/a.txt","<WS>/keys/b.txt","<WS>/keys/c.txt"]/true' \
  "$(jq -c '.selection.paths[0:3]' "$TMP/CLI-10.norm")/$(jq -c '.selection.lead == .selection.paths[-1]' "$TMP/CLI-10.norm")"
check_eq "extend_bottom の起点は動かない" '"<WS>/keys/a.txt"' "$(jq -c '.selection.anchor' "$TMP/CLI-10.norm")"
case "$(cat "$TMP/CLI-14.norm")" in *"見えていない行"*) pass "畳んだフォルダの中の行は選べない（理由つき）" ;; *) fail "見えていない行: $(cat "$TMP/CLI-14.norm")" ;; esac
check_eq "フォルダの上の Enter は開閉" '{"expanded":true,"path":"<WS>/keys/sub"}' "$(jq -S -c '.expanded' "$TMP/CLI-16.norm")"
check_eq "← ← ← で畳んでから親（keys）へ" '"<WS>/keys"' "$(jq -c '.selection.lead' "$TMP/CLI-20.norm")"
check_eq "ファイルの上の Enter は開く（OpenFile の応答が載る）" '"<PANE>"' "$(jq -c '.opened.pane' "$TMP/CLI-22.norm")"
check_eq "開いた後も選択はその行" '["<WS>/keys/b.txt"]' "$(jq -c '.selection.paths' "$TMP/CLI-23.norm")"
case "$(cat "$TMP/CLI-24.norm")" in *"extend_bottom"*) pass "知らない key は語彙を挙げて断る" ;; *) fail "知らない key: $(cat "$TMP/CLI-24.norm")" ;; esac

echo
echo "== ④ 一定の速さのコピーで eta_secs を読む（新しい式 / TAKO_1908_LEGACY=1） =="
# $1 = ラベル。いま立っている GUI で 40 MiB を写し、写し終えるまで eta_secs を読み続ける
measure_eta() {
  local tag="$1" dir="$TMP/eta-$1" seen=0 seq="" p n e
  make_big "$dir/big.bin" 40
  mkdir -p "$dir/dst"
  "$TAKO_BIN" file copy "$dir/big.bin" "$dir/dst" > "$TMP/eta-$tag.out" 2>&1 &
  BG_PID=$!
  for _ in $(seq 1 600); do
    p="$("$TAKO_BIN" file progress 2>/dev/null)"
    n="$(printf '%s' "$p" | jq -r '.copies | length' 2>/dev/null)"
    if [ "${n:-0}" -ge 1 ]; then
      seen=1
      e="$(printf '%s' "$p" | jq -r '.copies[0].eta_secs // empty')"
      [ -n "$e" ] && seq="$seq $e"
    elif [ "$seen" = 1 ]; then
      break
    fi
    sleep 0.15
  done
  wait "$BG_PID"
  BG_PID=""
  ETA_SEQ="$seq"
  # 読んだ回数 / 増えた回数 / 増えた幅の最大
  ETA_STAT="$(printf '%s\n' $seq | awk 'NF { if (n > 0 && $1 > prev) { ups++; d = $1 - prev; if (d > max) max = d } prev = $1; n++ } END { printf "%d %d %d", n, ups, max }')"
  echo "  観測（${tag}）: eta_secs の推移 →${seq}"
  echo "  観測（${tag}）: 読んだ回数 / 戻った回数 / 戻った幅の最大 = ${ETA_STAT}"
}
measure_eta new
read -r NEW_N NEW_UPS NEW_MAX <<EOF
$ETA_STAT
EOF
[ "${NEW_N:-0}" -ge 20 ] && pass "新しい式で eta_secs を ${NEW_N} 回読めた" || fail "eta_secs を読めた回数が少ない（${NEW_N}）"
[ "${NEW_UPS:-9}" -le 1 ] && pass "新しい式は戻らない（戻った回数 ${NEW_UPS}）" || fail "新しい式で eta_secs が ${NEW_UPS} 回戻った"

stop_gui
start_gui "$TMP/data-gui-legacy" "$TMP/gui-legacy.log" TAKO_1908_LEGACY=1
measure_eta legacy
read -r OLD_N OLD_UPS OLD_MAX <<EOF
$ETA_STAT
EOF
[ "${OLD_UPS:-0}" -ge 2 ] && pass "TAKO_1908_LEGACY=1 は戻る（戻った回数 ${OLD_UPS} = 比べる相手が揺れている）" \
  || fail "TAKO_1908_LEGACY=1 でも戻らない（${OLD_UPS}。検出力が無い）"
echo "  振れ幅（戻った回数 / 戻った幅の最大 秒）: 新 ${NEW_UPS} / ${NEW_MAX}・旧 ${OLD_UPS} / ${OLD_MAX}"
# CLI / MCP の選択は A/B でも同じ経路のまま（画面のキーだけを外す）
L="$TMP/legacy-ws"
make_fixture "$L"
"$TAKO_BIN" tree add "$L" > /dev/null
"$TAKO_BIN" panel --filetree on > /dev/null 2>&1
"$TAKO_BIN" tree selection "$L" > /dev/null 2>&1
LEG="$("$TAKO_BIN" tree selection --key right 2>&1)"
case "$LEG" in *'"changed": true'*|*'"changed":true'*) pass "TAKO_1908_LEGACY=1 でも CLI の --key は効く" ;; *) fail "LEGACY の CLI: $(printf '%s' "$LEG" | sed -e "s#${TMP}#<TMP>#g" | tr -d '\n')" ;; esac
stop_gui

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
if [ "$FAIL" -eq 0 ]; then
  tako_exit 0
fi
exit 1
