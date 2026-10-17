#!/usr/bin/env bash
# #2046: 貼り付けに API キー・パスワードらしきものがあれば PTY へ流す前に確かめる機能を、
# 実 GUI + 実 dispatch に通して測る。
#
#   ① 設定: 古い版が書いた settings.json（paste_guard の項目が無い）でも既定 on で読む。
#      CLI `tako paste-guard on|off` と MCP `tako_settings`（action=paste_guard）で切り替えられ、
#      settings.json へ残る（項目が無いまま読むだけでは書き換えない）
#   ② 判定: `tako paste-guard check`（標準入力）と MCP の text が同じ判定を返し、応答は伏せ字だけ
#   ③ 実 GUI（visual-test `paste-guard`）: 鍵を貼ると確認が出て、やめると PTY へ 1 バイトも届かない・
#      送ると届く・設定 off なら確認なしで届く（設定画面のトグルも実マウスで押す）・
#      ブラケットペーストのアプリへも包んで届く・persist.log に本文が出ない。
#      画像は `$1/new`（既定は一時 dir = 終わると消える）へ落ちる
#   ④ A/B: 同じバイナリを `TAKO_2046_LEGACY=1` で回すと ③ が ② で FAILED になる（画像は `$1/legacy`）
#   ⑤ GUI の標準出力・標準エラー・persist.log のどこにも偽の鍵の本文が出ない
#
# **本物のクリップボードに書かない**（visual-test は ⌘V と同じ paste() を、クリップボードの代わりに差し込み口から読ませて通す）。偽の鍵は
# 前置詞と本体を分けて書き、実行時に連結する（GitHub の secret scanning に当てない）。
# 本番の tako（GUI / 設定 / tmux）には触らない。
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

# visual 段は visual-test 版の tako-app が要る。バイナリを渡された A/B ではビルドしない。
# HOME を差し替える前に済ませる（ツールチェーンを取り直さない）
isolated_gui_bins visual-test || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk2046-XXXXXX)"
DUMP_ROOT="${1:-$TMP/dump}"
APP_PID=""
cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
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
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done

# 偽の鍵（前置詞と本体を分けて書く。本体は EXAMPLE を含む明らかな偽物）
FAKE_ANTHROPIC="sk-ant-""api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD"
FAKE_GITHUB="ghp_""EXAMPLE0123456789abcdefghijklmnopqrst"
export FAKE_ANTHROPIC FAKE_GITHUB

# 本文の 8 文字の部分列がどれかファイルに出ていれば 1（伏せ字は … を挟むので当たらない）
leaks_in() {
  python3 - "$1" <<'PY'
import os, sys
data = open(sys.argv[1], encoding="utf-8", errors="replace").read() if os.path.exists(sys.argv[1]) else ""
for secret in (os.environ["FAKE_ANTHROPIC"], os.environ["FAKE_GITHUB"]):
    if any(secret[i:i + 8] in data for i in range(len(secret) - 7)):
        print(1)
        sys.exit(0)
print(0)
PY
}

# 古い版が書いた settings.json（paste_guard の項目が無い）。自動命名は off（隔離でも claude を呼ぶ）
printf '%s\n' '{"auto_rename": false, "mouse_click_forward": true, "theme": "dark"}' \
  > "$TAKO_DATA_DIR/settings.json"

echo "--- ① 設定（古い settings.json でも既定 on・CLI / MCP で切替）"
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$TMP/app.log"; then
  echo "隔離 GUI が立たない（ログ: $TMP/app.log）"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"

jget() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1" 2>/dev/null; }
settings_has() { python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get('paste_guard', 'missing'))" "$TAKO_DATA_DIR/settings.json"; }

ST="$("$TAKO_BIN" paste-guard)"
check "$([ "$(printf '%s' "$ST" | jget 'd["enabled"]')" = "True" ] && echo 1)" \
  "項目の無い settings.json で既定 on（enabled=true）"
check "$([ "$(settings_has)" = "missing" ] && echo 1)" \
  "読むだけでは settings.json を書き換えない（項目は無いまま）"
OFF="$("$TAKO_BIN" paste-guard off)"
check "$([ "$(printf '%s' "$OFF" | jget 'd["enabled"]')" = "False" ] && [ "$(settings_has)" = "False" ] && echo 1)" \
  "tako paste-guard off で切れ、settings.json に false が残る"
ON="$("$TAKO_BIN" paste-guard on)"
check "$([ "$(printf '%s' "$ON" | jget 'd["enabled"]')" = "True" ] && [ "$(settings_has)" = "True" ] && echo 1)" \
  "tako paste-guard on で戻り、settings.json に true が残る"

echo "--- ② 判定（CLI の標準入力 / MCP の text・応答は伏せ字だけ）"
CHK="$(printf 'この鍵で: %s\n' "$FAKE_ANTHROPIC" | "$TAKO_BIN" paste-guard check)"
printf '%s\n' "$CHK" | sed 's/^/    /' | head -20
check "$([ "$(printf '%s' "$CHK" | jget 'd["hit"] and d["findings"][0]["rule"] == "anthropic_api_key" and d["findings"][0]["masked"] == "sk-ant-…ABCD"')" = "True" ] && echo 1)" \
  "check: 標準入力の鍵を anthropic_api_key・伏せ字 sk-ant-…ABCD で返す"
printf '%s' "$CHK" > "$TMP/check.out"
check "$([ "$(leaks_in "$TMP/check.out")" = "0" ] && echo 1)" "check: 応答に本文が載らない"
NORMAL="$(printf '普通の文章です。\ngit log の 7471605 や https://example.com/a/b も混ぜる\n' | "$TAKO_BIN" paste-guard check -)"
check "$([ "$(printf '%s' "$NORMAL" | jget 'd["hit"]')" = "False" ] && echo 1)" "check: 普通の文章は当たらない"
ELAPSED="$(printf '%s' "$CHK" | jget 'd["elapsed_us"]')"
echo "    判定の所要（debug ビルド・dispatch 内）: ${ELAPSED} us"
check "$([ "$("$TAKO_BIN" paste-guard off x >/dev/null 2>&1; echo $?)" != "0" ] && echo 1)" \
  "check 以外に文字を渡したら送らずに断る"

cat > "$TMP/mcp.py" <<'PY'
import json, os, sys, urllib.request
info = json.load(open(os.path.join(os.environ["TAKO_DISCOVERY_DIR"], "control.json")))
n = 0
def call(args):
    global n
    n += 1
    body = json.dumps({"jsonrpc": "2.0", "id": n, "method": "tools/call",
                       "params": {"name": "tako_settings", "arguments": args}})
    req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers={
        "Authorization": "Bearer " + info["token"], "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream"})
    with urllib.request.urlopen(req, timeout=60) as r:
        resp = json.loads(r.read())
    if "error" in resp:
        return {"rpc_error": resp["error"]["message"]}
    res = resp["result"]
    text = res["content"][0]["text"]
    try:
        out = json.loads(text)
    except ValueError:
        out = {"text": text}
    out["_is_error"] = bool(res.get("isError"))
    return out
fake = os.environ["FAKE_GITHUB"]
results = {
    "status": call({"action": "paste_guard"}),
    "off": call({"action": "paste_guard", "enabled": False}),
    "on": call({"action": "paste_guard", "enabled": True}),
    "check": call({"action": "paste_guard", "text": "token: " + fake}),
    "respond_none": call({"action": "paste_guard", "respond": "send"}),
    "wrong_action": call({"action": "open", "enabled": True}),
}
json.dump(results, open(sys.argv[1], "w"), ensure_ascii=False)
PY
python3 "$TMP/mcp.py" "$TMP/mcp.json" || bad "MCP の呼び出しが通らない"
mj() { python3 -c "import json,sys; d=json.load(open(sys.argv[2])); print(eval(sys.argv[1], {'d': d}))" "$1" "$TMP/mcp.json" 2>/dev/null; }
check "$([ "$(mj 'd["status"]["enabled"] and d["status"]["default"]')" = "True" ] && echo 1)" "MCP: action=paste_guard で状態（既定 on）"
check "$([ "$(mj 'd["off"]["enabled"] == False and d["on"]["enabled"] == True')" = "True" ] && echo 1)" "MCP: enabled で切り替えられる"
check "$([ "$(mj 'd["check"]["check"]["findings"][0]["rule"] == "github_token" and d["check"]["check"]["findings"][0]["masked"] == "ghp_…qrst"')" = "True" ] && echo 1)" \
  "MCP: text で CLI と同じ判定・伏せ字"
check "$([ "$(leaks_in "$TMP/mcp.json")" = "0" ] && echo 1)" "MCP: 応答に本文が載らない"
check "$([ "$(mj '"確認待ち" in d["respond_none"].get("rpc_error", d["respond_none"].get("text", ""))')" = "True" ] && echo 1)" \
  "MCP: 確認待ちが無いときの respond は理由つきで断る"
check "$([ "$(mj '"paste_guard" in d["wrong_action"].get("rpc_error", d["wrong_action"].get("text", ""))')" = "True" ] && echo 1)" \
  "MCP: 他の action に enabled を渡したら黙って捨てずに断る"

stop_isolated_gui "$APP_PID"
APP_PID=""

# visual 段のシェルは隔離 HOME + 固定プロンプト（画像にユーザー名・ホスト名を写さない = #927）
mkdir -p "$TMP/home" "$TMP/zdot"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

run_visual() {
  local label="$1" dump="$2" log="$3"
  shift 3
  mkdir -p "$dump"
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=paste-guard \
    TAKO_VISUAL_DUMP_DIR="$dump" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  for _ in $(seq 1 1800); do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  echo "    [$label]"
  grep -E "TAKO_VISUAL_PIXEL: paste-guard|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" \
    "$log" | sed 's/^/      /'
}

echo "--- ③ 実 GUI（visual-test paste-guard）"
run_visual new "$DUMP_ROOT/new" "$TMP/visual-new.log"
check "$(grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual-new.log" && echo 1)" \
  "visual: 確認が出て、やめると 1 バイトも届かず、送ると届き、off（CLI・設定画面のトグル）なら確認なしで届く（① 〜 ⑪）"

echo "--- ④ A/B（同じバイナリを TAKO_2046_LEGACY=1 で）"
run_visual legacy "$DUMP_ROOT/legacy" "$TMP/visual-legacy.log" TAKO_2046_LEGACY=1
check "$(grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test paste-guard ②" "$TMP/visual-legacy.log" && echo 1)" \
  "A/B: 旧挙動は ② で落ちる（確認なしで鍵が届く）"

echo "--- ⑤ 診断ログに本文が出ない"
cat "$TMP"/app.log "$TMP"/visual-new.log "$TMP"/visual-legacy.log > "$TMP/all-stdout.log" 2>/dev/null
check "$([ "$(leaks_in "$TMP/all-stdout.log")" = "0" ] && echo 1)" "GUI の標準出力・標準エラーに本文が出ない"
check "$([ "$(leaks_in "$TAKO_DATA_DIR/persist.log")" = "0" ] && echo 1)" "persist.log に本文が出ない"
check "$(grep -q "貼り付けの確認: prompted" "$TAKO_DATA_DIR/persist.log" && echo 1)" \
  "persist.log には規則名と件数だけの行が残る"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
