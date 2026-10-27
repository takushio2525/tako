#!/bin/bash
# test-preview-reveal-2247.sh — 「Finder で表示」（#2247）の実経路テスト（隔離 GUI）
#
# 何を確かめるか（Issue #2247 の受け入れのうち、GUI・CLI・MCP を通して初めて言えるもの）:
#   「Finder で表示」はどの入口から押しても、tako 自身から NSWorkspace（activateFileViewerSelectingURLs:）で
#   Finder を前面へ出す 1 実装（dispatch op=reveal → os_integration::reveal → platform::finder_reveal）へ
#   ちょうど 1 回届く。子の `open -R` は起こさない（本番では open が成功しても Finder が前に出てこなかった）
#   ① visual-test `preview-reveal-2247`: 隔離 GUI（tako-vd）で**実マウス**の右クリック（ヘッダ / 本文）→
#      「Finder で表示」が NSWorkspace へ 1 回 / 空白 + 日本語の名前・画像・PDF・動画も同じ /
#      開いた後に消えたファイルは何も呼ばず理由を通知欄へ / ツリー・パスのメニュー・右パネルの添付・
#      設定画面も同じ実体へ 1 回
#   ② A/B: `TAKO_2247_LEGACY=1`（修正前 = 子の `open -R`）の同じバイナリでは ① が
#      「ヘッダのメニューから NSWorkspace へ 1 回」で FAILED（偽の open にだけ届く）
#   ③ 実 GUI + 実 CLI + MCP: `tako file reveal` / MCP `tako_file_op`（op=reveal）が同じ実体へ 1 回・
#      プレビューのペインのファイル（`tako list` の preview.path）を渡すとメニューと同じ URL・
#      ディレクトリも選択表示・存在しないパスは理由つきで断り何も呼ばない
#   ④ A/B: LEGACY の GUI では同じ CLI が子の `open -R` を起こす（CLI / MCP の先も切り替わった証拠）
#   ⑤ GUI の外: GUI が居なければ CLI は「繋がらない」で断り、自分のプロセスから Finder を呼ばない
#
# **本物の Finder の窓を出さない**: GUI の env に `TAKO_REVEAL_RECORD=<probe>/reveal.log`
# （NSWorkspace へ渡す直前の URL を 1 行書くだけで Finder を開かない口）と、PATH の先頭の偽の `open`
# （旧の形の引数を 1 行ずつ書くだけ）を渡す。GUI の env を `ps -E` で読んで両方が効いていることを
# 確かめてから押す（違えば押さずに止まる）。visual-test の節も同じ確認を自分で行う。
#
# 使い方: bash scripts/test-preview-reveal-2247.sh
#   バイナリは `isolated_gui_bins visual-test` が今の木から建てる（最新なら cargo は何もしない）
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意できなければ起動せず終了コード 4 で
# 「未実測」を返す。**CI には載せない**（実 GUI が要る）。macOS 専用（記録の口と偽の open で受ける）
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

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-2247-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-2247-$$"
cleanup() {
  # isolated-gui.sh を読む前（ビルド中）に抜けたときは落とす GUI が無い
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  tmux -L "${TMUX_SOCKET}-l" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/tmpdir" "$TMP/probe/bin"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_2247_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP/new" "$DUMP/legacy"

# 記録の口（新 = NSWorkspace へ渡す直前の URL）と偽の open（旧 = 子の `open -R`）
PROBE="$TMP/probe"
RECORD="$PROBE/reveal.log"
CALLS="$PROBE/open-calls.log"
cat > "$PROBE/bin/open" <<EOF
#!/bin/sh
{ printf 'open'; for a in "\$@"; do printf '\t%s' "\$a"; done; printf '\n'; } >> '$CALLS'
EOF
chmod 755 "$PROBE/bin/open"
probe_env=(TAKO_REVEAL_RECORD="$RECORD" PATH="$PROBE/bin:$PATH")
# この OS で tako が Finder へ前面を譲れるか（yieldActivation… は macOS 14 から）
if [ "$(sw_vers -productVersion | cut -d. -f1)" -ge 14 ]; then YIELD=1; else YIELD=0; fi

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
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} ${probe_env[@]+"${probe_env[@]}"} \
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

echo "== ① visual-test preview-reveal-2247（実マウス・記録の口と偽の open） =="
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
echo "== ② A/B: TAKO_2247_LEGACY=1 では子の open -R にだけ届いて FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" "$DUMP/legacy" TAKO_2247_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_VISUAL_PIXEL: preview-reveal-2247\|TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" \
  | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test preview-reveal-2247 ①: ヘッダのメニューから NSWorkspace へ 1 回（\["open -R ' \
  "$TMP/legacy.log" \
  && pass "落ちた理由は NSWorkspace ではなく子の open -R へ行くこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（記録の口と偽の open を GUI の env へ） =="
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
P="$TMP/proj"
mkdir -p "$P/sub dir"
SPACED="$P/空白 と 日本語.md"
printf '# 空白 と 日本語\n\n本文\n' > "$SPACED"
: > "$RECORD"
: > "$CALLS"

# 本物の Finder を出さない: GUI の env に記録の口が無い・PATH の先頭が偽の open でなければ押さない
guard_gui_env() {
  local envs gui_path
  envs="$(ps -E -ww -o command= -p "$APP_PID" | tr ' ' '\n')"
  gui_path="$(grep '^PATH=' <<<"$envs" | sed -n '1p')"
  if grep -qx "TAKO_REVEAL_RECORD=$RECORD" <<<"$envs" && [[ "$gui_path" == "PATH=$PROBE/bin:"* ]]; then
    pass "GUI の env に記録の口があり、PATH の先頭は偽の open の dir"
  else
    echo "GUI の env に記録の口・偽の open が無い（本物の Finder を出さないため止める）"
    exit 1
  fi
}

launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} ${probe_env[@]+"${probe_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1
guard_gui_env

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
# 届いた呼び出しを 1 行に（新 = `NSWorkspace <パス> yield=N` / 旧 = `open -R <パス>`。呼び出しは「|」で区切る）。
# 旧の open は子プロセスなので少し待つ。読んだら空にする
calls() {
  sleep 0.6
  python3 - "$RECORD" "$CALLS" "$TMP" <<'PY'
import sys
def read(p):
    try:
        return open(p, encoding="utf-8").read().splitlines()
    except FileNotFoundError:
        return []
out = []
for l in read(sys.argv[1]):
    c = l.split("\t")
    out.append("NSWorkspace " + " ".join(c[1:]) if c[0] == "activateFileViewerSelectingURLs:" else "?" + l)
for l in read(sys.argv[2]):
    out.append(" ".join(l.split("\t")))
print("|".join(out).replace(sys.argv[3], "<TMP>"))
PY
  : > "$RECORD"
  : > "$CALLS"
}
NS="NSWorkspace <TMP>/proj/空白 と 日本語.md yield=${YIELD}"

"$TAKO_BIN" file reveal "$SPACED" >/dev/null 2>&1
check_eq "CLI: tako file reveal（空白 + 日本語）が NSWorkspace へ 1 回" "$NS" "$(calls)"
R="$(mcp tako_file_op "$(python3 -c 'import json,sys; print(json.dumps({"op":"reveal","path":sys.argv[1]}, ensure_ascii=False))' "$SPACED")")"
check_eq "MCP: tako_file_op op=reveal の応答" "<TMP>/proj/空白 と 日本語.md" \
  "$(printf '%s' "$R" | json 'd["revealed"]' | sed "s#${TMP}#<TMP>#g")"
check_eq "MCP: 同じ NSWorkspace へ 1 回" "$NS" "$(calls)"
"$TAKO_BIN" file reveal "$P/sub dir" >/dev/null 2>&1
check_eq "CLI: ディレクトリも同じ実体で選択表示" "NSWorkspace <TMP>/proj/sub dir yield=${YIELD}" "$(calls)"
OUT="$("$TAKO_BIN" file reveal "$P/無い.md" 2>&1)"
RC=$?
[ "$RC" -ne 0 ] && pass "存在しないパスは非ゼロで断る（終了コード ${RC}）" || fail "存在しないパスが成功扱い"
case "$OUT" in *存在しない*) pass "断る理由は「存在しない」" ;; *) fail "断る理由が違う: $OUT" ;; esac
check_eq "存在しないパスでは Finder を呼ばない" "" "$(calls)"

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
check_eq "プレビューのファイルを CLI で表示 = メニューと同じ URL" "$NS" "$(calls)"
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ④ A/B: TAKO_2247_LEGACY=1 の GUI では同じ CLI が子の open -R を起こす =="
# 2 つ目の GUI は discovery / tmux / data を分ける（前の GUI の器を拾わない = #2157）
mkdir -p "$TMP/disc-l" "$TMP/data-gui-l"
export TAKO_DISCOVERY_DIR="$TMP/disc-l" TAKO_DATA_DIR="$TMP/data-gui-l" TAKO_TMUX_SOCKET="${TMUX_SOCKET}-l"
launch_isolated_gui "$TMP/gui-legacy.log" ${common_env[@]+"${common_env[@]}"} ${probe_env[@]+"${probe_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_DISCOVERY_DIR="$TAKO_DISCOVERY_DIR" \
  TAKO_TMUX_SOCKET="$TAKO_TMUX_SOCKET" TAKO_2247_LEGACY=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui-legacy.log" || exit 1
guard_gui_env
"$TAKO_BIN" file reveal "$SPACED" >/dev/null 2>&1
check_eq "LEGACY: 同じ CLI は子の open -R を 1 回（NSWorkspace は呼ばない）" "open -R <TMP>/proj/空白 と 日本語.md" "$(calls)"
stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ⑤ GUI の外: GUI が居なければ CLI は断り、自分では Finder を呼ばない =="
mkdir -p "$TMP/disc-none"
OUT="$(env TAKO_DISCOVERY_DIR="$TMP/disc-none" ${probe_env[@]+"${probe_env[@]}"} "$TAKO_BIN" file reveal "$SPACED" 2>&1)"
RC=$?
[ "$RC" -ne 0 ] && pass "GUI が居ない CLI は非ゼロで断る（終了コード ${RC}）" || fail "GUI が居ないのに成功扱い"
echo "  観測: $(printf '%s' "$OUT" | sed -n '1,2p' | tr '\n' ' ' | sed "s#${TMP}#<TMP>#g")"
check_eq "GUI が居ない CLI は Finder を呼ばない" "" "$(calls)"

echo
echo "結果: PASS=${PASS} FAIL=${FAIL}"
if [ -n "${TAKO_2247_DUMP_DIR:-}" ]; then
  echo "画像: ${DUMP}/new / ${DUMP}/legacy"
fi
[ "$FAIL" -eq 0 ]
