#!/bin/bash
# test-tasks-input-2016.sh — 右パネル tasks ビューの返答欄と返答の履歴（Issue #2016）の実経路検証
#
# 何を見るか:
#   ① CLI / MCP: `tako todo respond` で積んだやりとりが、`tako todo show`（既定の表示）では
#      古い順に「返答 時刻 [経路] 判断: 本文」で並び（複数行の本文は字下げで続く）、
#      `--json` と MCP `tako_todo` の show では同じ `responses`（古い順・at / via / decision / comment）
#      で読める
#   ② visual-test `tasks-input`（隔離 GUI・仮想ディスプレイ）: 返答欄の IME（GPUI の NSView へ
#      NSTextInputClient のメッセージを直接送る）・折り返し・上限・送信の間・履歴・エッジ
#   ③ 同じ節を `TAKO_2016_LEGACY=1`（修正前の欄と送信）で回し、**落ちる**ことと送信の間の数字を採る
#
# 画像は第 1 引数の dir（既定は一時 dir）へ新旧で名前を分けて落ちる
# （`tasks-input-<場面>-fixed.png` / `-legacy.png`）。画像にユーザー名・ホスト名が写らないよう、
# GUI のシェルは隔離 HOME + 固定プロンプトで起こす（#927）。
#
# 本番の tako（GUI / 設定 / todo / tmux）には触らない。終わったら自分で起こした pid だけを落とす。
#
# 使い方:  bash scripts/test-tasks-input-2016.sh [画像の書き出し先]

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# visual 段は visual-test 版の tako-app が要る（素のビルドだと「未知の節」で exit 1）。
# バイナリを渡された A/B ではビルドしない。HOME を差し替える前に済ませる（ツールチェーンを取り直さない）
if [ -z "${APP_BIN:-}" ]; then
  echo "visual-test 版の tako / tako-app をビルドします…"
  (cd "$REPO_ROOT" && run_heavy cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
fi
isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2016-XXXXXX)"
DUMP_DIR="${1:-$TMP/dump}"
TMUX_SOCK="tako-iso-2016-$$"
APP_PID=""
cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # 明示した tmux ソケットは tako の終了時に片付けられない（#770）ので自分で落とす。
  # **自分が名前を決めたソケットだけ**を -L で名指しする
  tmux -L "$TMUX_SOCK" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERSIST=0
export TAKO_TMUX_SOCKET="$TMUX_SOCK"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DUMP_DIR"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done
mkdir -p "$TMP/home" "$TMP/zdot"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# ---------------------------------------------------------------------------
echo "--- ① CLI / MCP の show（やりとりの履歴）"
launch_isolated_gui "$TMP/app.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$TMP/app.log"; then
  echo "隔離 GUI が立たない（ログ: $TMP/app.log）"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"
# CLI は**隔離先の socket / token を明示して**叩く（継承や既定の解決で本番 GUI = 本番の todo に
# 当てない）。空なら進めずに止める
CTRL="$TAKO_DISCOVERY_DIR/control.json"
[ -f "$CTRL" ] || { echo "隔離先の control.json が無い: $CTRL"; exit 1; }
SOCK="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$CTRL" 2>/dev/null)"
TOK="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$CTRL" 2>/dev/null)"
[ -n "$SOCK" ] && [ -n "$TOK" ] || { echo "隔離先の socket / token が空"; exit 1; }
iso_tako() {
  env -u TAKO_PANE_ID -u TAKO_TAB_ID -u TAKO_MCP_URL TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOK" "$TAKO_BIN" "$@"
}
# 隔離先か（起動直後は 1 ペイン。本番は十数ペイン）
NP="$(iso_tako list | python3 -c 'import json,sys; d=json.load(sys.stdin); print(sum(len(t["panes"]) for t in d["tabs"]))' 2>/dev/null)"
[ "$NP" = "1" ] || { echo "繋ぎ先が隔離 GUI に見えない（ペイン数 ${NP:-?}）"; exit 1; }
# 隔離 GUI でも自動命名は既定 ON で claude を呼ぶので、構成を作る前に止める
iso_tako autorename off >/dev/null 2>&1 || true

ID="$(iso_tako todo add "2016 の履歴の確認" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
[ -n "$ID" ] || { echo "起票できない"; exit 1; }
iso_tako todo respond "$ID" --decision needs_change --comment "1 回目: サムネを大きく" >/dev/null
iso_tako todo respond "$ID" --decision answered --comment "$(printf '2 回目の 1 行目\n2 回目の 2 行目')" --via pwa >/dev/null
iso_tako todo respond "$ID" --decision answered >/dev/null
SHOW="$(iso_tako todo show "$ID")"
printf '%s\n' "$SHOW" | sed 's/^/    /'
# 古い順・時刻つき（HH:MM）・経路つき
ORDER="$(printf '%s\n' "$SHOW" | python3 -c '
import re,sys
lines=[l for l in sys.stdin.read().splitlines() if l.startswith("  返答 ")]
ok = len(lines) == 3 \
  and re.match(r"  返答 \d\d:\d\d \[cli\] needs_change: 1 回目", lines[0]) \
  and re.match(r"  返答 \d\d:\d\d \[pwa\] answered: 2 回目の 1 行目", lines[1]) \
  and re.match(r"  返答 \d\d:\d\d \[cli\] answered: $", lines[2])
print(1 if ok else "")')"
check "$ORDER" "tako todo show: やりとりが古い順に「返答 時刻 [経路] 判断: 本文」で並ぶ"
check "$(printf '%s\n' "$SHOW" | grep -qx '      2 回目の 2 行目' && echo 1)" \
  "tako todo show: 複数行の本文の 2 行目以降が字下げで続く"

JSON="$(iso_tako todo show "$ID" --json)"
export ID DISC="$TAKO_DISCOVERY_DIR"
MCP="$(python3 - <<'PY'
import json, os, urllib.request
info = json.load(open(os.path.join(os.environ["DISC"], "control.json")))
def rpc(n, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params})
    req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers={
        "Authorization": "Bearer " + info["token"],
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())
rpc(1, "initialize", {"protocolVersion": "2025-06-18"})
r = rpc(2, "tools/call", {"name": "tako_todo", "arguments": {"action": "show", "id": os.environ["ID"]}})
print(r["result"]["content"][0]["text"])
PY
)"
SAME="$(python3 -c '
import json,sys
a=json.loads(sys.argv[1])["responses"]; b=json.loads(sys.argv[2])["responses"]
key=lambda rs:[(r["at"],r["via"],r["decision"],r["comment"]) for r in rs]
ok = key(a) == key(b) and [r["via"] for r in a] == ["cli","pwa","cli"] \
  and all(a[i]["at"] <= a[i+1]["at"] for i in range(len(a)-1)) \
  and a[1]["comment"] == "2 回目の 1 行目\n2 回目の 2 行目"
print(1 if ok else "")' "$JSON" "$MCP" 2>/dev/null)"
check "$SAME" "tako todo show --json と MCP tako_todo show が同じやりとり（古い順・at / via / decision / comment）を返す"

stop_isolated_gui "$APP_PID"
APP_PID=""
rm -f "$TAKO_ORCHESTRATOR_DIR/user-tasks.yaml"

# ---------------------------------------------------------------------------
run_visual() {
  local arm="$1"
  shift
  launch_isolated_gui "$TMP/visual-$arm.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tasks-input \
    TAKO_VISUAL_DUMP_DIR="$DUMP_DIR" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  for _ in $(seq 1 1800); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  cp "$TAKO_DATA_DIR/perf.log" "$DUMP_DIR/perf-$arm.log" 2>/dev/null || true
  grep -E "TAKO_VISUAL_PIXEL: tasks-input|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_2016" \
    "$TMP/visual-$arm.log" | sed 's/^/    /'
}

echo "--- ② visual-test tasks-input（修正後）"
run_visual fixed
check "$(grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual-fixed.log" && echo 1)" \
  "visual: IME・折り返し・上限・送信の間・履歴・エッジがすべて通る"

echo "--- ③ visual-test tasks-input（TAKO_2016_LEGACY=1 = 修正前の欄と送信）"
rm -f "$TAKO_ORCHESTRATOR_DIR/user-tasks.yaml"
run_visual legacy TAKO_2016_LEGACY=1
check "$(grep -q "TAKO_APP_SELF_TEST_FAILED: tasks-input \[legacy\]" "$TMP/visual-legacy.log" && echo 1)" \
  "visual（旧）: 修正前の欄は同じ判定で落ちる（A/B）"

echo
echo "画像: $DUMP_DIR"
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
