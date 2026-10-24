#!/usr/bin/env bash
# test-line-ops-1692.sh — 行の操作（複製・移動・行インデント・コメント。#1692）の実 GUI 経路テスト
#
# 隔離した data で **visual-test 版の実 tako-app** を立て、
#   ① visual-test 節 `editor-keys` を走らせる → (15) 行の操作の相（⌥↓ / ⌥↑ / ⇧⌥↓ / ⌘/ ×2 /
#      ⌘] / ⌘[。Windows は Ctrl 段）を含む全相が緑。打鍵は `window.dispatch_keystroke` へ流すので
#      キーバインド判定 → 編集の入口 → 打鍵表 → TextBuffer の全段を通る（キーバインドに食われたら落ちる）
#   ② CLI `tako edit line <操作>` と MCP `tako_preview_edit` の `command`（`line-<操作>`）を
#      同じ手順で撃ち、手順ごとに保存したファイルが固定値とバイト一致し、応答の字面が CLI と MCP で
#      一致する（設計原則 5 の 1:1）
#   ③ 端: 空ファイル・改行の無い 1 行・CRLF・コメント記号の無い形式（理由が返り本文は変わらない）
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で
# 起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-line-ops-1692.sh
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

TMP="$(mktemp -d /tmp/tako-1692-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  stop_isolated_gui "${CLI_GUI_PID:-}"
  tmux -L "${TAKO_TMUX_SOCKET:-tako-1692-$$}" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# visual-test 版をビルドする（HOME を差し替える前に。差し替えた後だとツールチェーンを取り直す）
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1692-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

echo "① editor-keys 節（(15) 行の操作の相を含む）"
launch_isolated_gui "$TMP/keys.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=editor-keys || exit $?
KEYS_PID="$ISOLATED_GUI_PID"
for _ in $(seq 1 1800); do
  kill -0 "$KEYS_PID" 2>/dev/null || break
  sleep 0.1
done
stop_isolated_gui "$KEYS_PID"
grep "TAKO_VISUAL_PIXEL: editor-keys line-" "$TMP/keys.log" | sed 's/^/    /'
check_eq "行の操作の相が 6 つ報告される（移動 2・複製・コメント・深く・浅く）" "6" "$(grep -c 'TAKO_VISUAL_PIXEL: editor-keys line-' "$TMP/keys.log")"
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/keys.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR" "$TMP/keys.log" | tail -5 | sed 's/^/    /'
fi

echo "② CLI / MCP から同じ手順で同じ結果"
launch_isolated_gui "$TMP/cli.log" || exit $?
CLI_GUI_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/cli.log" || exit 1
# `tako mcp serve` は socket と token を env から読む（隔離 data の中のものを名指しする）
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
export TAKO_TOKEN
# 隔離 GUI でも自動命名は claude を呼ぶので止める
"$TAKO_BIN" autorename off >/dev/null 2>&1 || true
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | jq -r '.tabs[0].panes[0].id')"

# ファイルを開いて編集モードにし、ペイン ID を返す
open_editing() {
  local pane
  pane="$("$TAKO_BIN" open "$1" --pane "$ROOT_PANE" --new-tab 2>&1 | jq -r '.pane // empty')"
  case "$pane" in
    '' | *[!0-9]*) echo "プレビューペインを開けない: $1" >&2; return 1 ;;
  esac
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null || return 1
  echo "$pane"
}
# ペインと置き場だけは別物なので落とす（版・カーソル・選択・undo 履歴は比較に残す）
normalize() { jq -cS 'del(.pane, .path)' 2>/dev/null || echo "__NOT_JSON__"; }

FIXTURE=$'fn main() {\n    let a = 1;\n    let b = 2;\n}\n'
DUP=$'fn main() {\n    let a = 1;\n    let a = 1;\n    let b = 2;\n}\n'
SWAP=$'fn main() {\n    let b = 2;\n    let a = 1;\n}\n'
CMT=$'fn main() {\n    // let a = 1;\n    // let b = 2;\n}\n'
DEEP=$'fn main() {\n        let a = 1;\n        let b = 2;\n}\n'
TOP=$'// fn main() {\n    let a = 1;\n    let b = 2;\n}\n'
printf '%s' "$FIXTURE" > "$TMP/cli.rs"
printf '%s' "$FIXTURE" > "$TMP/mcp.rs"
CLI_PANE="$(open_editing "$TMP/cli.rs")" || exit 1
MCP_PANE="$(open_editing "$TMP/mcp.rs")" || exit 1

# 手順（CLI の `tako edit` の引数 | MCP のツール | MCP の引数 JSON | 手順の後のファイル | ラベル）
STEPS=(
  "cursor 2:8|tako_preview_cursor|{\"line\":2,\"col\":8}|FIXTURE|起点"
  "line duplicate-down|tako_preview_edit|{\"command\":\"line-duplicate-down\"}|DUP|下へ複製"
  "undo|tako_preview_undo|{}|FIXTURE|undo 1 回で戻る"
  "line duplicate-up|tako_preview_edit|{\"command\":\"line-duplicate-up\"}|DUP|上へ複製（本文は下へ複製と同じ）"
  "undo|tako_preview_undo|{}|FIXTURE|undo 1 回で戻る"
  "line move-down|tako_preview_edit|{\"command\":\"line-move-down\"}|SWAP|下へ移動"
  "line move-up|tako_preview_edit|{\"command\":\"line-move-up\"}|FIXTURE|上へ移動で元へ"
  "cursor 2:4 --select-to 3:6|tako_preview_cursor|{\"line\":2,\"col\":4,\"select_to_line\":3,\"select_to_col\":6}|FIXTURE|2 行を選択"
  "line toggle-comment|tako_preview_edit|{\"command\":\"line-toggle-comment\"}|CMT|選択行にコメント"
  "line toggle-comment|tako_preview_edit|{\"command\":\"line-toggle-comment\"}|FIXTURE|もう一度で外れる"
  "line indent|tako_preview_edit|{\"command\":\"line-indent\"}|DEEP|選択行を深く"
  "line outdent|tako_preview_edit|{\"command\":\"line-outdent\"}|FIXTURE|選択行を浅く"
  "line comment|tako_preview_edit|{\"command\":\"line-comment\"}|CMT|付けるだけ"
  "line uncomment|tako_preview_edit|{\"command\":\"line-uncomment\"}|FIXTURE|外すだけ"
  "cursor 1:0|tako_preview_cursor|{\"line\":1,\"col\":0}|FIXTURE|先頭行へ"
  "line move-up|tako_preview_edit|{\"command\":\"line-move-up\"}|FIXTURE|先頭行の上移動は何も変えない"
  "line comment|tako_preview_edit|{\"command\":\"line-comment\"}|TOP|先頭行にコメント"
  "undo|tako_preview_undo|{}|FIXTURE|undo 1 回で戻る"
)
MCP_IN="$TMP/mcp.in"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1692","version":"0"}}}' > "$MCP_IN"
CLI_OUT=()
CLI_TEXT_OK=()
i=0
for step in ${STEPS[@]+"${STEPS[@]}"}; do
  IFS='|' read -r cli_args tool mcp_args want_name label <<< "$step"
  # shellcheck disable=SC2086
  CLI_OUT+=("$("$TAKO_BIN" edit $cli_args --pane "$CLI_PANE" 2>&1)")
  "$TAKO_BIN" edit save --pane "$CLI_PANE" >/dev/null 2>&1
  if [ "$(cat "$TMP/cli.rs"; printf x)" = "${!want_name}x" ]; then CLI_TEXT_OK+=(1); else CLI_TEXT_OK+=(0); fi
  args="$(printf '%s' "$mcp_args" | jq -c --argjson p "$MCP_PANE" '. + {pane: $p}')"
  printf '{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' \
    $((i * 2 + 10)) "$tool" "$args" >> "$MCP_IN"
  printf '{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":"tako_preview_save","arguments":{"pane":%s}}}\n' \
    $((i * 2 + 11)) "$MCP_PANE" >> "$MCP_IN"
  i=$((i + 1))
done
MCP_OUT="$("$TAKO_BIN" mcp serve < "$MCP_IN" 2>/dev/null)"
# MCP の応答 1 件（id で引く）。本文は content[0].text の JSON
mcp_result() {
  printf '%s\n' "$MCP_OUT" | jq -c --argjson id "$1" 'select(.id == $id) | .result.content[0].text | fromjson? // .' 2>/dev/null | head -1
}
i=0
for step in ${STEPS[@]+"${STEPS[@]}"}; do
  IFS='|' read -r cli_args tool mcp_args want_name label <<< "$step"
  check_eq "CLI: ${label}（tako edit ${cli_args}）の後のファイルが固定値とバイト一致" "1" "${CLI_TEXT_OK[$i]}"
  check_eq "MCP の応答が CLI と字面一致（${tool} ${mcp_args}）" \
    "$(printf '%s' "${CLI_OUT[$i]}" | normalize)" "$(mcp_result $((i * 2 + 10)) | normalize)"
  i=$((i + 1))
done
if cmp -s "$TMP/cli.rs" "$TMP/mcp.rs"; then
  pass "最後のファイルが CLI と MCP でバイト一致"
else
  fail "最後のファイルが CLI と MCP で違う"
fi
# 先頭行の上移動は版を進めない（本文も履歴も変えない）
V_BEFORE="$(printf '%s' "${CLI_OUT[14]}" | jq -r '.document.version')"
V_AFTER="$(printf '%s' "${CLI_OUT[15]}" | jq -r '.document.version')"
check_eq "先頭行の上移動で版が進まない" "$V_BEFORE" "$V_AFTER"
# 下へ複製はカーソルが下の写しへ・上へ複製は元の行に残る
check_eq "下へ複製のカーソルは 3 行目" "3" "$(printf '%s' "${CLI_OUT[1]}" | jq -r '.document.cursor.line')"
check_eq "上へ複製のカーソルは 2 行目のまま" "2" "$(printf '%s' "${CLI_OUT[3]}" | jq -r '.document.cursor.line')"

echo "③ 端（空ファイル・改行の無い 1 行・CRLF・コメント記号の無い形式）"
# 空ファイル: 複製で空行が 1 つ増える（足す改行はこの OS の流儀 = macOS は LF）
: > "$TMP/empty.rs"
P="$(open_editing "$TMP/empty.rs")" || exit 1
"$TAKO_BIN" edit line duplicate-down --pane "$P" >/dev/null
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_eq "空ファイルの複製は改行 1 つ" "0a" "$(od -An -tx1 "$TMP/empty.rs" | tr -d ' \n')"
# 改行の無い 1 行: 上下の移動は何も変えず、複製は行を足す
printf 'abc' > "$TMP/one.rs"
P="$(open_editing "$TMP/one.rs")" || exit 1
V0="$("$TAKO_BIN" edit status --pane "$P" 2>&1 | jq -r '.document.version')"
"$TAKO_BIN" edit line move-up --pane "$P" >/dev/null
V1="$("$TAKO_BIN" edit line move-down --pane "$P" 2>&1 | jq -r '.document.version')"
check_eq "1 行だけのファイルの上下移動は版を進めない" "$V0" "$V1"
"$TAKO_BIN" edit line duplicate-up --pane "$P" >/dev/null
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_eq "改行の無い 1 行の複製" "abc|abc" "$(tr '\n' '|' < "$TMP/one.rs")"
# CRLF: 移動・コメント・複製の後も LF だけの改行が混ざらない
printf 'a = 1\r\nb = 2\r\n' > "$TMP/crlf.py"
P="$(open_editing "$TMP/crlf.py")" || exit 1
"$TAKO_BIN" edit cursor 1:0 --pane "$P" >/dev/null
"$TAKO_BIN" edit line move-down --pane "$P" >/dev/null
"$TAKO_BIN" edit line duplicate-down --pane "$P" >/dev/null
"$TAKO_BIN" edit cursor 1:0 --select-to 4:0 --pane "$P" >/dev/null
"$TAKO_BIN" edit line toggle-comment --pane "$P" >/dev/null
"$TAKO_BIN" edit save --pane "$P" >/dev/null
printf '# b = 2\r\n# a = 1\r\n# a = 1\r\n' > "$TMP/crlf.want"
if cmp -s "$TMP/crlf.py" "$TMP/crlf.want"; then
  pass "CRLF のファイルは CRLF のまま（移動 → 複製 → コメント。バイト一致）"
else
  fail "CRLF のファイルの結果が違う: $(od -c "$TMP/crlf.py" | head -3 | tr -s ' ')"
fi
# コメント記号の無い形式: CLI はエラー（終了コード非 0）で理由を言い、本文は変えない。MCP も同じ理由
printf 'plain\n' > "$TMP/notes.txt"
P="$(open_editing "$TMP/notes.txt")" || exit 1
ERR="$("$TAKO_BIN" edit line toggle-comment --pane "$P" 2>&1)"
RC=$?
echo "    CLI: rc=$RC $ERR" | head -2
check_eq "記号の無い形式の CLI は失敗で終わる" "1" "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
# 理由の文は表示言語に従う（#2365）ので英語表示の文も数える
check_eq "理由に「コメント記号を知らない」" "1" "$(printf '%s' "$ERR" | grep -c 'コメント記号を知らない\|no comment syntax is known')"
MCP_ERR="$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1692","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_preview_edit\",\"arguments\":{\"pane\":${P},\"command\":\"line-toggle-comment\"}}}" \
  | "$TAKO_BIN" mcp serve 2>/dev/null | jq -c 'select(.id == 2) | .result')"
echo "    MCP: $MCP_ERR" | cut -c1-200
check_eq "MCP は isError で同じ理由" "true|1" \
  "$(printf '%s' "$MCP_ERR" | jq -r '.isError')|$(printf '%s' "$MCP_ERR" | grep -c 'コメント記号を知らない\|no comment syntax is known')"
"$TAKO_BIN" edit save --pane "$P" >/dev/null
check_eq "記号の無い形式の本文は変わらない" "plain" "$(cat "$TMP/notes.txt")"
# 知らない操作名は実行前に弾く（候補を並べる）
BAD="$("$TAKO_BIN" edit line move_up --pane "$P" 2>&1)"
check_eq "知らない操作名は候補を出して断る" "1" "$(printf '%s' "$BAD" | grep -c 'duplicate-up / duplicate-down')"
stop_isolated_gui "$CLI_GUI_PID"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
