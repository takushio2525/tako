#!/bin/bash
# test-lsp-followup-1769.sh — LSP S1 の続き（#1769）の実経路テスト（隔離 GUI・偽サーバ・CLI・MCP）
#
# 何を確かめるか（Issue #1769 の受け入れ条件のうち、GUI を通して初めて言えるもの）:
#   ① サーバの解決のキャッシュ: `tako lsp servers` の 1 回目（ログインシェルを起こす）と 2 回目
#      （起こさない）の所要と各行の `cached`。見つからないサーバを後から PATH へ置いてもキャッシュが
#      答える → シェル統合の合図（ペインでコマンドを 1 つ実行）で引き直して見つかる → `restart` でも
#      引き直す。CLI と MCP `tako_lsp_server`（action=list）の応答が字面一致
#   ② 単独の CR: CRLF・単独 CR・LF が混ざるファイルで、診断の位置（偽サーバが自分の本文で数えたもの）
#      と定義ジャンプの往復（tako の位置で問う → サーバの語 → 着地）が tako の位置と一致する。
#      単独 CR の前後を範囲編集してもサーバの本文は「今の本文の送る形」のまま。保存はバイト一致
#      （単独 CR は CR のまま）。偽サーバの数え方を `lf` に替えて `restart` しても同じ位置。
#      MCP `tako_lsp` が CLI と同じ答え
#   ③ 同じファイルの 2 ペイン目: didOpen は 1 回・2 つ目のペインにも加わった直後から診断が出る
#      （`drawn`）・片方の編集は 1 文書の didChange・読み取り表示のペインは持ち手にならない・
#      片方を閉じても開いたまま・両方閉じると didClose が 1 回
#   ④ 外部変更（#1659 との合流）: 未編集のペインがディスクへ黙って追従したとき・編集済みのペインが
#      競合から `tako edit reload` で読み直したときに、didChange の版が進み、サーバの本文が
#      ディスクの送る形に追いつく（競合のあいだは編集中の本文のまま）
#
# 使い方: bash scripts/test-lsp-followup-1769.sh
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 面を用意できなければ起動せず終了コード 4 で「未実測」を返す（#1744）。
# 落とすのは自分で起こした pid だけ。
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

# 受け口のソケットのパス長に余裕を持たせるため短い置き場にする
TMP="$(mktemp -d /tmp/tk1769.XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1769-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

cargo build -q -p tako-app || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako --bin tako-lsp-fake 2>/dev/null \
  || cargo build -q -p tako-cli -p tako-control || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
FAKE="$(dirname "$APP_BIN")/tako-lsp-fake"
[ -x "$FAKE" ] || { echo "偽サーバが無い: $FAKE"; exit 1; }
# 作り直した直後の実行ファイルは macOS が初回の起動を検査で待たせる（実測 4.6 秒。2 回目から 0.02 秒）。
# ② で初めて起こすと、その待ちが「開いてから診断が出るまで」の窓を食って間欠的に落ちるので、先に 1 回起こす
W0="$(python3 -c 'import time; print(int(time.time() * 1000))')"
"$FAKE" </dev/null >/dev/null 2>&1 || true
echo "  観測: 偽サーバの初回の起動 $(($(python3 -c 'import time; print(int(time.time() * 1000))') - W0)) ms"

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/bin" "$TMP/proj/src"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
# 重い profile の代わり: ログインシェル 1 つが 1 秒かかる（解決の 1 回目と 2 回目の差を見る）
printf 'sleep 1\n' > "$TMP/zdot/.zprofile"
printf '[package]\nname = "p"\n' > "$TMP/proj/Cargo.toml"
# 偽サーバの数え方（spec / lf）は起動のたびにこのファイルから読む（restart で切り替える）
echo spec > "$TMP/mode"
printf '#!/bin/sh\nexec "%s" --line-breaks "$(cat "%s")" "$@"\n' "$FAKE" "$TMP/mode" > "$TMP/fake-ra"
chmod +x "$TMP/fake-ra"
echo '[{"echo": true}]' > "$TMP/goto.json"

export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/d"
mkdir -p "$TAKO_DATA_DIR"
launch_isolated_gui "$TMP/gui.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" \
  PATH="$TMP/bin:$PATH" \
  TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" \
  TAKO_LSP_BIN_RUST_ANALYZER="$TMP/fake-ra" \
  TAKO_LSP_FAKE_MARK=MARK TAKO_LSP_FAKE_LOG="$TMP/received.jsonl" \
  TAKO_LSP_FAKE_DOC_LOG="$TMP/docs.jsonl" TAKO_LSP_FAKE_GOTO="$TMP/goto.json" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
ms_now() { python3 -c 'import time; print(int(time.time() * 1000))'; }
ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
[ -n "$ROOT" ] || { echo "ペインが取れない"; exit 1; }

# MCP（stdio ブリッジ）で 1 回呼ぶ。ブリッジは tako の中で起動された証（TAKO_SOCKET + TAKO_TOKEN）が
# 無いとツールを 1 つも公開しない（FR-2.3.2）ので、隔離インスタンスの受け口を明示する
SOCK="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
mcp_call() {
  local name="$1" args="$2"
  (
    export TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
    {
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
      printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' "$name" "$args"
      sleep 3
    } | "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"])
'
  )
}
same_json() {
  python3 -c '
import json, sys
a = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
print(a == b)' "$1" "$2" 2>/dev/null
}
# 偽サーバが受けた method の数（uri の末尾で絞る）
received_count() {
  python3 -c '
import json, sys
n = 0
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        m = json.loads(line)
    except Exception:
        continue
    uri = ((m.get("params") or {}).get("textDocument") or {}).get("uri", "")
    if m.get("method") == sys.argv[2] and uri.endswith(sys.argv[3]):
        n += 1
print(n)' "$TMP/received.jsonl" "$1" "$2" 2>/dev/null || echo 0
}

echo "== ① サーバの解決のキャッシュ（1 回目 / 2 回目 / 合図 / restart） =="
# 最初のプロンプトの cwd の知らせ（OSC 7）も「PATH が変わりうる出来事」として数えるので、
# プロンプトが出て落ち着いてから測る（途中で出ると 2 回目が正しく引き直してしまう）
for _ in $(seq 1 100); do
  "$TAKO_BIN" read --pane "$ROOT" 2>/dev/null | grep -q 'tako ' && break
  sleep 0.1
done
sleep 1
T0="$(ms_now)"
"$TAKO_BIN" lsp servers --json > "$TMP/servers1.json"
T1="$(ms_now)"
"$TAKO_BIN" lsp servers --json > "$TMP/servers2.json"
T2="$(ms_now)"
echo "  観測: 1 回目 $((T1 - T0)) ms / 2 回目 $((T2 - T1)) ms（ログインシェル 1 つ = profile で 1 秒）"
check_eq "1 回目はすべて引いた（cached=false）" "[False, False, False, False]" \
  "$(json '[r["cached"] for r in d["servers"]]' < "$TMP/servers1.json")"
check_eq "2 回目はすべてキャッシュ（cached=true）" "[True, True, True, True]" \
  "$(json '[r["cached"] for r in d["servers"]]' < "$TMP/servers2.json")"
[ $((T2 - T1)) -lt 900 ] && pass "2 回目はログインシェル（1 秒）を待たない（$((T2 - T1)) ms）" \
  || fail "2 回目も遅い（$((T2 - T1)) ms）"
# 旧実装は表の 4 行を 1 つずつ待った。キャッシュに無いものは並行して引くので、1 回目は 1 秒前後
[ $((T1 - T0)) -lt 2900 ] && pass "1 回目は並行して引く（$((T1 - T0)) ms < 1 秒 × 3）" \
  || fail "1 回目が 1 つずつ待っている（$((T1 - T0)) ms）"
python3 - "$TMP/servers1.json" "$TMP/servers2.json" <<'PY' && pass "キャッシュの答えは 1 回目と同じ（cached 以外）" || fail "キャッシュの答えが 1 回目と違う"
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
for d in (a, b):
    for r in d["servers"]:
        r.pop("cached")
sys.exit(0 if a == b else 1)
PY
"$TAKO_BIN" lsp servers --json > "$TMP/servers-cli.json"
mcp_call tako_lsp_server '{"action":"list"}' > "$TMP/servers-mcp.json"
check_eq "MCP tako_lsp_server（list）が CLI と字面一致" "True" "$(same_json "$TMP/servers-cli.json" "$TMP/servers-mcp.json")"
check_eq "pyright はまだ見つからない" "False" \
  "$(json '[r["installed"] for r in d["servers"] if r["id"] == "pyright"][0]' < "$TMP/servers2.json")"
# 見つからない答えもキャッシュする: 後から PATH の先頭へ置いても、合図が無ければ引かない
cp "$FAKE" "$TMP/bin/pyright-langserver"
check_eq "置いただけでは見つからないまま（見つからない答えのキャッシュ）" "(False, True)" \
  "$("$TAKO_BIN" lsp servers --json | json '[(r["installed"], r["cached"]) for r in d["servers"] if r["id"] == "pyright"][0]')"
# シェル統合の合図: ペインでコマンドを 1 つ実行する（コマンドの終わり = PATH が変わりうる出来事）
"$TAKO_BIN" send --pane "$ROOT" "true" >/dev/null
GOT=""
for _ in $(seq 1 60); do
  GOT="$("$TAKO_BIN" lsp servers --json | json '[(r["installed"], r["cached"]) for r in d["servers"] if r["id"] == "pyright"][0]')"
  [ "$GOT" = "(True, False)" ] && break
  sleep 0.5
done
check_eq "ペインでコマンドを実行したら引き直して見つかる" "(True, False)" "$GOT"
rm -f "$TMP/bin/pyright-langserver"
"$TAKO_BIN" lsp restart >/dev/null
check_eq "restart の後は引き直す（消したので見つからない・cached=false）" "(False, False)" \
  "$("$TAKO_BIN" lsp servers --json | json '[(r["installed"], r["cached"]) for r in d["servers"] if r["id"] == "pyright"][0]')"

echo
echo "== ② 単独の CR（診断の位置・定義ジャンプの往復・範囲編集・保存・数え方を替えて restart） =="
MAIN="$TMP/proj/src/main.rs"
python3 - "$MAIN" <<'PY'
import sys
open(sys.argv[1], "wb").write("fn a() {}\rMARKa x;\r\n// 日本😀\rlet MARKb = 1;\n\rMARKc\r\n".encode())
PY
# tako の位置（行 1 始まり・行内の UTF-8 バイト）で "MARK" の出現を数える（tako の実装を使わない）
expect_marks() {
  python3 - "$1" <<'PY'
import sys
b = open(sys.argv[1], "rb").read()
out = []
i = b.find(b"MARK")
while i >= 0:
    out.append((b[:i].count(b"\n") + 1, i - (b.rfind(b"\n", 0, i) + 1)))
    i = b.find(b"MARK", i + 1)
print(out)
PY
}
# サーバが持つべき本文（単独 CR → LF）が偽サーバの最後の本文と一致するか
server_matches_wire() {
  python3 - "$1" "$TMP/docs.jsonl" "$2" <<'PY'
import json, sys
b = open(sys.argv[1], "rb").read().decode()
wire = b.replace("\r\n", "\0").replace("\r", "\n").replace("\0", "\r\n")
last = None
for line in open(sys.argv[2], encoding="utf-8"):
    e = json.loads(line).get("fake_doc")
    if e and e["uri"].endswith(sys.argv[3]):
        last = e["text"]
print(last == wire)
PY
}
diag_positions() {
  "$TAKO_BIN" lsp diagnostics --pane "$1" --json \
    | json 'sorted((x["range"]["start"]["line"], x["range"]["start"]["column"]) for x in d["documents"][0]["diagnostics"])'
}
PANE="$("$TAKO_BIN" open "$MAIN" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$PANE" ] || { echo "プレビューを開けない"; exit 1; }
"$TAKO_BIN" edit start --pane "$PANE" >/dev/null
WANT="$(expect_marks "$MAIN")"
for _ in $(seq 1 100); do
  GOT="$(diag_positions "$PANE")"
  [ "$GOT" = "$WANT" ] && break
  sleep 0.1
done
check_eq "診断の位置 = 本文の中の MARK の位置（spec の偽サーバ）" "$WANT" "${GOT:-}"
check_eq "サーバの本文は送る形（単独 CR → LF）" "True" "$(server_matches_wire "$MAIN" main.rs)"
check_eq "そのペインが描いている数（drawn）= 3" "3" \
  "$("$TAKO_BIN" lsp diagnostics --pane "$PANE" --json | json 'd["documents"][0]["drawn"]')"
# 定義ジャンプの往復: 語の途中（頭 + 2 バイト）を問うと、サーバが自分の本文で引いた語の頭へ着く
for word in MARKa MARKb MARKc; do
  POS="$(python3 - "$MAIN" "$word" <<'PY'
import sys
b = open(sys.argv[1], "rb").read(); i = b.find(sys.argv[2].encode())
print(b[:i].count(b"\n") + 1, i - (b.rfind(b"\n", 0, i) + 1))
PY
)"
  L="${POS% *}"; C="${POS#* }"
  ANS="$("$TAKO_BIN" lsp definition --pane "$PANE" --line "$L" --column $((C + 2)) --open none --json \
    | json '(d["status"], d["locations"][0]["line"], d["locations"][0]["column"])')"
  ASKED="$(python3 -c '
import json, sys
w = None
for line in open(sys.argv[1], encoding="utf-8"):
    e = json.loads(line).get("fake_goto")
    if e:
        w = e["word"]
print(w)' "$TMP/docs.jsonl")"
  check_eq "定義ジャンプの往復（${word}: サーバが引いた語 / 着地 = 語の頭）" \
    "${word} ('found', ${L}, ${C})" "${ASKED} ${ANS}"
done
MCP_DIAG="$(mcp_call tako_lsp "{\"action\":\"diagnostics\",\"pane\":${PANE}}")"
echo "$MCP_DIAG" > "$TMP/diag-mcp.json"
"$TAKO_BIN" lsp diagnostics --pane "$PANE" --json > "$TMP/diag-cli.json"
check_eq "MCP tako_lsp（diagnostics）が CLI と同じ答え" "True" \
  "$(python3 -c '
import json, sys
a = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
print(a["documents"] == b["documents"])' "$TMP/diag-cli.json" "$TMP/diag-mcp.json" 2>/dev/null)"

# 単独 CR の前後を範囲編集する: 単独 CR の直後へ打つ / 単独 CR を CRLF にする / CRLF を単独 CR にする
apply_edit() {
  # $1 = 探す本文（python のリテラル）/ $2 = 置き換える本文（python のリテラル）
  local plan
  plan="$(python3 - "$MAIN" "$1" "$2" "$TMP/expected.bin" <<'PY'
import sys, ast
b = open(sys.argv[1], "rb").read()
find = ast.literal_eval(sys.argv[2]).encode(); repl = ast.literal_eval(sys.argv[3]).encode()
i = b.find(find)
assert i >= 0, (find, b)
NL = b"\n"
def pos(o):
    return "%d:%d" % (b[:o].count(NL) + 1, o - (b.rfind(NL, 0, o) + 1))
open(sys.argv[4], "wb").write(b[:i] + repl + b[i + len(find):])
print(pos(i), pos(i + len(find)))
PY
)"
  local start="${plan% *}" end="${plan#* }"
  local text
  text="$(python3 -c 'import ast,sys; sys.stdout.write(ast.literal_eval(sys.argv[1]))' "$2"; printf x)"
  "$TAKO_BIN" edit replace-range --pane "$PANE" "$start" "$end" "${text%x}" >/dev/null
  "$TAKO_BIN" edit save --pane "$PANE" >/dev/null 2>&1
}
# 置き換える本文の改行は多数派（ここでは CRLF）で書く。新しく足す改行は #1650 でファイルの多数派へ
# 揃う（LF だけを足すと CRLF になる）。単独の `\r` は行内の文字として素通しする
EDIT_N=0
for spec in "'\\rMARKa'|'\\rzz MARKa'" "'😀\\rlet'|'😀\\r\\nlet'" "'x;\\r\\n'|'x;\\r'" "'\\rMARKc'|'\\r日MARKc'"; do
  EDIT_N=$((EDIT_N + 1))
  apply_edit "${spec%%|*}" "${spec#*|}"
  for _ in $(seq 1 60); do
    cmp -s "$MAIN" "$TMP/expected.bin" && break
    sleep 0.1
  done
  if cmp -s "$MAIN" "$TMP/expected.bin"; then
    pass "編集 ${EDIT_N}: 保存はバイト一致（単独 CR は CR のまま）"
  else
    fail "編集 ${EDIT_N}: 保存した中身が期待と違う"
    python3 -c 'import sys; [print("    ", n, repr(open(p, "rb").read())) for n, p in (("実際", sys.argv[1]), ("期待", sys.argv[2]))]' \
      "$MAIN" "$TMP/expected.bin"
  fi
  WANT="$(expect_marks "$MAIN")"
  for _ in $(seq 1 100); do
    GOT="$(diag_positions "$PANE")"
    [ "$GOT" = "$WANT" ] && [ "$(server_matches_wire "$MAIN" main.rs)" = "True" ] && break
    sleep 0.1
  done
  check_eq "編集 ${EDIT_N}: 診断の位置が本文と一致" "$WANT" "${GOT:-}"
  check_eq "編集 ${EDIT_N}: サーバの本文は送る形のまま" "True" "$(server_matches_wire "$MAIN" main.rs)"
done
# 偽サーバの数え方を `lf`（rust-analyzer / clangd の問い合わせの数え方）へ替えて起こし直す
echo lf > "$TMP/mode"
OPENS="$(received_count textDocument/didOpen main.rs)"
"$TAKO_BIN" lsp restart --name rust-analyzer >/dev/null
for _ in $(seq 1 100); do
  [ "$(received_count textDocument/didOpen main.rs)" -gt "$OPENS" ] && break
  sleep 0.1
done
WANT="$(expect_marks "$MAIN")"
for _ in $(seq 1 100); do
  GOT="$(diag_positions "$PANE")"
  [ "$GOT" = "$WANT" ] && break
  sleep 0.1
done
check_eq "lf で数える偽サーバでも診断の位置が本文と一致" "$WANT" "${GOT:-}"
"$TAKO_BIN" close --pane "$PANE" --force >/dev/null 2>&1 || "$TAKO_BIN" close --pane "$PANE" >/dev/null 2>&1
echo spec > "$TMP/mode"

echo
echo "== ③ 同じファイルの 2 ペイン目 =="
TWO="$TMP/proj/src/two.rs"
printf 'MARK a\n' > "$TWO"
A="$("$TAKO_BIN" open "$TWO" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
B="$("$TAKO_BIN" open "$TWO" --pane "$A" --down 2>/dev/null | json 'd["pane"]')"
[ -n "$A" ] && [ -n "$B" ] && [ "$A" != "$B" ] || { echo "2 つのペインを開けない（A=${A} B=${B}）"; exit 1; }
"$TAKO_BIN" edit start --pane "$A" >/dev/null
for _ in $(seq 1 100); do
  N="$("$TAKO_BIN" lsp diagnostics --pane "$A" --json | json 'len(d["documents"][0]["diagnostics"])')"
  [ "$N" = "1" ] && break
  sleep 0.1
done
check_eq "A に診断 1 件" "1" "${N:-}"
"$TAKO_BIN" edit start --pane "$B" >/dev/null
check_eq "B は加わった直後から描いている（publish を待たない。drawn）" "1" \
  "$("$TAKO_BIN" lsp diagnostics --pane "$B" --json | json 'd["documents"][0]["drawn"]')"
check_eq "2 つ目のペインは didOpen しない" "1" "$(received_count textDocument/didOpen two.rs)"
# 読み取り表示のペイン（編集していない）は持ち手にならない
C="$("$TAKO_BIN" open "$TWO" --pane "$B" --down 2>/dev/null | json 'd["pane"]')"
check_eq "持ち手は A と B の 2 つ（読み取り表示の C は数えない）" "(1, 2)" \
  "$("$TAKO_BIN" lsp status --json | json '(d["servers"][0]["documents"], d["servers"][0]["views"])')"
# B で編集 → 1 文書の didChange。診断は両方のペインに出る
"$TAKO_BIN" edit replace-range --pane "$B" 2:0 2:0 $'MARK b\n' >/dev/null
for _ in $(seq 1 100); do
  BOTH="$("$TAKO_BIN" lsp diagnostics --json | json 'sorted((x["pane"], len(x["diagnostics"]), x["drawn"]) for x in d["documents"] if x["path"].endswith("two.rs"))')"
  [ "$BOTH" = "[(${A}, 2, 2), (${B}, 2, 2)]" ] && break
  sleep 0.1
done
check_eq "B の編集の診断が A と B の両方に出る（表 2 件・描いている 2 件）" "[(${A}, 2, 2), (${B}, 2, 2)]" "${BOTH:-}"
check_eq "didChange は 1 文書に届く（didOpen は 1 回のまま）" "(1, 1)" \
  "($(received_count textDocument/didOpen two.rs), $(received_count textDocument/didChange two.rs))"
mcp_call tako_lsp_server '{"action":"status"}' > "$TMP/status-mcp.json"
"$TAKO_BIN" lsp status --json > "$TMP/status-cli.json"
check_eq "MCP tako_lsp_server（status）の documents / views が CLI と同じ" "True" \
  "$(python3 -c '
import json, sys
a = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
f = lambda d: [(s["id"], s["documents"], s["views"]) for s in d["servers"]]
print(f(a) == f(b))' "$TMP/status-cli.json" "$TMP/status-mcp.json" 2>/dev/null)"
# A を閉じても文書は開いたまま（didClose なし）
"$TAKO_BIN" close --pane "$A" --force >/dev/null 2>&1 || "$TAKO_BIN" close --pane "$A" >/dev/null 2>&1
"$TAKO_BIN" edit replace-range --pane "$B" 1:0 1:0 'x ' >/dev/null
for _ in $(seq 1 100); do
  [ "$(received_count textDocument/didChange two.rs)" = "2" ] && break
  sleep 0.1
done
check_eq "A を閉じても開いたまま（didClose 0・B の編集は届く）" "(0, 2)" \
  "($(received_count textDocument/didClose two.rs), $(received_count textDocument/didChange two.rs))"
check_eq "持ち手は B だけ" "(1, 1)" \
  "$("$TAKO_BIN" lsp status --json | json '(d["servers"][0]["documents"], d["servers"][0]["views"])')"
# B も閉じると didClose が 1 回（読み取り表示の C が残っていても閉じる）
"$TAKO_BIN" close --pane "$B" --force >/dev/null 2>&1 || "$TAKO_BIN" close --pane "$B" >/dev/null 2>&1
for _ in $(seq 1 100); do
  [ "$(received_count textDocument/didClose two.rs)" = "1" ] && break
  sleep 0.1
done
check_eq "両方閉じると didClose が 1 回" "1" "$(received_count textDocument/didClose two.rs)"
check_eq "保持している診断は 0" "(0, 0)" \
  "$("$TAKO_BIN" lsp diagnostics --json | json '(d["retained"]["diagnostics"], d["retained"]["documents"])')"
[ -n "$C" ] && { "$TAKO_BIN" close --pane "$C" --force >/dev/null 2>&1 || true; }

echo
echo "== ④ 外部変更（#1659 との合流）: 未編集の追従・読み直しでもサーバの本文が追いつく =="
# 読み直しは `apply_edit` を通る 1 回の編集なので版が進み、didChange が飛ぶはず。飛ばないと
# サーバの本文がディスクより古いまま残る（診断・定義ジャンプが古い本文で答える）
"$TAKO_BIN" preview-reload on >/dev/null 2>&1
wait_server_matches() { # ファイル uri の末尾
  for _ in $(seq 1 150); do
    [ "$(server_matches_wire "$1" "$2")" = "True" ] && return 0
    sleep 0.1
  done
  return 1
}
# 偽サーバが受けた didChange の版の最大（uri の末尾で絞る）
last_change_version() {
  python3 -c '
import json, sys
v = 0
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        m = json.loads(line)
    except Exception:
        continue
    doc = (m.get("params") or {}).get("textDocument") or {}
    if m.get("method") == "textDocument/didChange" and doc.get("uri", "").endswith(sys.argv[2]):
        v = max(v, doc.get("version", 0))
print(v)' "$TMP/received.jsonl" "$1" 2>/dev/null || echo 0
}
conflict_state() {
  "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | json '(d.get("conflict") or {}).get("state")'
}
EXT="$TMP/proj/src/ext.rs"
printf 'fn one() {}\n' > "$EXT"
D="$("$TAKO_BIN" open "$EXT" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
[ -n "$D" ] || { echo "ext.rs を開けない"; exit 1; }
"$TAKO_BIN" edit start --pane "$D" >/dev/null
"$TAKO_BIN" edit autosave false --pane "$D" >/dev/null 2>&1
wait_server_matches "$EXT" ext.rs
check_eq "開いた直後: サーバの本文 = ディスク" "True" "$(server_matches_wire "$EXT" ext.rs)"
V0="$(last_change_version ext.rs)"
# (a) 未編集のままディスクを書き換える → 黙って追従する（単独 CR を混ぜて送る形まで見る）
printf 'fn one() {}\r// outside A\n' > "$EXT"
wait_server_matches "$EXT" ext.rs
check_eq "未編集の追従: サーバの本文 = ディスクの送る形" "True" "$(server_matches_wire "$EXT" ext.rs)"
VA="$(last_change_version ext.rs)"
check_eq "未編集の追従で didChange の版が進む（${V0} → ${VA}）" "1" \
  "$([ "$VA" -gt "$V0" ] && echo 1 || echo 0)"
# (b) 編集してから外で書き換える → 競合 → `tako edit reload` で読み直す
"$TAKO_BIN" edit replace-range 1:0 1:0 "// mine " --pane "$D" >/dev/null
for _ in $(seq 1 100); do
  [ "$(last_change_version ext.rs)" -gt "$VA" ] && break
  sleep 0.1
done
VB="$(last_change_version ext.rs)"
printf 'fn two() {}\r// outside B\n' > "$EXT"
for _ in $(seq 1 150); do
  [ "$(conflict_state "$D")" = "changed" ] && break
  sleep 0.1
done
check_eq "編集済みのペインは追従せず競合になる" "changed" "$(conflict_state "$D")"
check_eq "競合のあいだサーバの本文は編集中の本文のまま（ディスクと違う）" "False" \
  "$(server_matches_wire "$EXT" ext.rs)"
"$TAKO_BIN" edit reload --pane "$D" >/dev/null
wait_server_matches "$EXT" ext.rs
check_eq "読み直し: サーバの本文 = ディスクの送る形" "True" "$(server_matches_wire "$EXT" ext.rs)"
VR="$(last_change_version ext.rs)"
check_eq "読み直しで didChange の版が進む（${VB} → ${VR}）" "1" \
  "$([ "$VR" -gt "$VB" ] && echo 1 || echo 0)"
"$TAKO_BIN" close --pane "$D" --force >/dev/null 2>&1 || true

stop_isolated_gui "$APP_PID"
APP_PID=""
echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
