#!/bin/bash
# test-preview-reveal-2247.sh — プレビューのペインの「Finder で表示」（#2247）の実経路テスト（隔離 GUI）
#
# 何を確かめるか（Issue #2247 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   ① visual-test `preview-reveal-2247`: 隔離 GUI（tako-vd）で**実マウス**の右クリック（ヘッダ / 本文）→
#      「Finder で表示」を押すと、偽の `open` へ `-R <対象>` が 1 回届く / 空白 + 日本語の名前も 1 引数 /
#      開いた後に消えたファイルは open を呼ばず理由を通知欄へ / ツリー・パスのメニューも同じ引数
#   ② A/B: `TAKO_2247_LEGACY=1`（修正前 = プレビューだけ dispatch を通らず結果を捨てる）の同じバイナリでは
#      ① が「消えたファイルでは open を呼ばない」で FAILED（無いパスへ `open -R` を黙って起こす）
#   ③ 実 GUI + 実 CLI + MCP: `tako file reveal` / MCP `tako_file_op`（op=reveal）が同じ 1 回を起こす・
#      プレビューのペインのファイル（`tako list` の preview.path）を渡すとメニューと同じ引数・
#      ディレクトリも選択表示・存在しないパスは理由つきで断り open を呼ばない
#
# **本物の Finder の窓を出さない**: GUI の PATH の先頭に偽の `open`（引数を 1 行ずつ書くだけ）を置き、
# GUI の env を `ps -E` で読んで先頭が偽物であることを確かめてから押す（違えば押さずに止まる）。
# visual-test の節も同じ確認を自分で行う。
#
# 使い方: bash scripts/test-preview-reveal-2247.sh
#   直前に同じソースで `cargo build -p tako-app --features visual-test` と `cargo build -p tako-cli` を
#   済ませてあるときは `TAKO_2247_SKIP_BUILD=1` でビルドの順番待ちを飛ばす（バイナリの時刻は自分で確かめる）
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意できなければ起動せず終了コード 4 で
# 「未実測」を返す。**CI には載せない**（実 GUI が要る）。macOS 専用（偽の open で受ける）
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

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-2247-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-2247-$$"
cleanup() {
  # isolated-gui.sh を読む前（ビルド中）に抜けたときは落とす GUI が無い
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

if [ "${TAKO_2247_SKIP_BUILD:-}" != "1" ]; then
  run_heavy cargo build -q -p tako-app --features visual-test || exit 1
  run_heavy cargo build -q -p tako-cli || exit 1
fi

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/tmpdir" "$TMP/bin"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_2247_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP/new" "$DUMP/legacy"
# GUI の一時 dir も $TMP の中へ（visual-test の fixture が節の途中で落ちても後片付けで消える）
common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja TMPDIR="$TMP/tmpdir/"
  TAKO_PERSIST=0 TAKO_AUTORENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
)

# visual-test の節を 1 回走らせる（$1 = ログ / $2 = data dir / $3 = 画像の書き出し先 / 残りは追加の env）
run_visual() {
  local log="$1" data="$2" dump="$3"
  shift 3
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$data" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=preview-reveal-2247 TAKO_VISUAL_DUMP_DIR="$dump" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

echo "== ① visual-test preview-reveal-2247（実マウス・偽の open） =="
run_visual "$TMP/visual.log" "$TMP/data-visual" "$DUMP/new"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: preview-reveal-2247' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'

echo
echo "== ② A/B: TAKO_2247_LEGACY=1 で消えたファイルへ open -R を黙って起こし FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" "$DUMP/legacy" TAKO_2247_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_VISUAL_PIXEL: preview-reveal-2247\|TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" \
  | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test preview-reveal-2247 ⑤: 消えたファイルでは open を呼ばない' \
  "$TMP/legacy.log" \
  && pass "落ちた理由は消えたファイルへ open -R を黙って起こすこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（偽の open を GUI の PATH の先頭へ） =="
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
CALLS="$TMP/open-calls.log"
cat > "$TMP/bin/open" <<EOF
#!/bin/sh
{ printf 'open'; for a in "\$@"; do printf '\t%s' "\$a"; done; printf '\n'; } >> '$CALLS'
EOF
chmod 755 "$TMP/bin/open"
P="$TMP/proj"
mkdir -p "$P/sub dir"
SPACED="$P/空白 と 日本語.md"
printf '# 空白 と 日本語\n\n本文\n' > "$SPACED"
launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" PATH="$TMP/bin:$PATH" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1

# 本物の Finder を出さない: GUI の PATH の先頭が偽の open の dir でなければ押さない
GUI_PATH="$(ps -E -ww -o command= -p "$APP_PID" | tr ' ' '\n' | grep '^PATH=' | head -1)"
case "$GUI_PATH" in
  "PATH=$TMP/bin:"*) pass "GUI の PATH の先頭は偽の open の dir" ;;
  *) echo "GUI の PATH の先頭が偽の open ではない（本物の Finder を出さないため止める）"; exit 1 ;;
esac

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
mcp() { # 引数のツール名と JSON → 応答本文
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-2247","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("id") == 2:
        r = m.get("result")
        print(r["content"][0]["text"] if r else json.dumps(m.get("error"), ensure_ascii=False)); break'
}
# 偽の open の記録を 1 行に（呼び出しは「|」で区切る）。open は子プロセスなので少し待つ
calls() {
  sleep 0.6
  python3 - "$CALLS" "$TMP" <<'PY'
import sys
try:
    lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
except FileNotFoundError:
    lines = []
print("|".join(" ".join(l.split("\t")[1:]).replace(sys.argv[2], "<TMP>") for l in lines))
PY
  : > "$CALLS"
}

"$TAKO_BIN" file reveal "$SPACED" >/dev/null 2>&1
check_eq "CLI: tako file reveal（空白 + 日本語）が open -R を 1 回" "-R <TMP>/proj/空白 と 日本語.md" "$(calls)"
R="$(mcp tako_file_op "$(python3 -c 'import json,sys; print(json.dumps({"op":"reveal","path":sys.argv[1]}, ensure_ascii=False))' "$SPACED")")"
check_eq "MCP: tako_file_op op=reveal の応答" "<TMP>/proj/空白 と 日本語.md" \
  "$(printf '%s' "$R" | json 'd["revealed"]' | sed "s#${TMP}#<TMP>#g")"
check_eq "MCP: 同じ open -R が 1 回" "-R <TMP>/proj/空白 と 日本語.md" "$(calls)"
"$TAKO_BIN" file reveal "$P/sub dir" >/dev/null 2>&1
check_eq "CLI: ディレクトリも選択表示（open -R <dir>）" "-R <TMP>/proj/sub dir" "$(calls)"
OUT="$("$TAKO_BIN" file reveal "$P/無い.md" 2>&1)"
RC=$?
[ "$RC" -ne 0 ] && pass "存在しないパスは非ゼロで断る（終了コード ${RC}）" || fail "存在しないパスが成功扱い"
case "$OUT" in *存在しない*) pass "断る理由は「存在しない」" ;; *) fail "断る理由が違う: $OUT" ;; esac
check_eq "存在しないパスでは open を呼ばない" "" "$(calls)"

# プレビューのペインのファイル: GUI のメニューは list の preview.path をそのまま dispatch へ渡す
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
PV="$("$TAKO_BIN" open "$SPACED" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$PV" ] || { echo "プレビューを開けない"; exit 1; }
PV_PATH="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d=json.load(sys.stdin); pane=int(sys.argv[1])
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"] == pane and p.get("preview"): print(p["preview"]["path"])' "$PV")"
check_eq "list の preview.path は開いたファイル" "<TMP>/proj/空白 と 日本語.md" "$(printf '%s' "$PV_PATH" | sed "s#${TMP}#<TMP>#g")"
"$TAKO_BIN" file reveal "$PV_PATH" >/dev/null 2>&1
check_eq "プレビューのファイルを CLI で表示 = メニューと同じ引数" "-R <TMP>/proj/空白 と 日本語.md" "$(calls)"

echo
echo "結果: PASS=${PASS} FAIL=${FAIL}"
if [ -n "${TAKO_2247_DUMP_DIR:-}" ]; then
  echo "画像: ${DUMP}/new / ${DUMP}/legacy"
fi
[ "$FAIL" -eq 0 ]
