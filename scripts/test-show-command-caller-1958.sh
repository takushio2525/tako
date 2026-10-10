#!/usr/bin/env bash
# #1958: pane を省いた `tako_show_command`（コマンド提案カード）が**呼び出し元ペイン**へ出るかを、
# 実 GUI + 実 dispatch に 3 経路で通して測る。
#
#   - http  … GUI 内蔵 MCP（Streamable HTTP。呼び出し元は `X-Tako-Pane`）
#   - stdio … `tako mcp serve` の stdio ブリッジ（呼び出し元は `TAKO_PANE_ID`）
#   - cli   … `tako show-command "…"`（呼び出し元は `TAKO_PANE_ID`）
#
# 修正前は変換が呼び出し元で埋めておらず、pane を省いた show は「対象ペインが未指定」で断られた。
# **呼び出し元（A）とフォーカス中のペイン（B）を分けて**測る（たまたまフォーカスへ出たのと
# 区別する）。カードが出たかは GUI が持つ保管庫（画面のカード帯の描き元）を `--list` で読む。
#
# ついでに同じ型の `tako_sessions` の link（id・pane 省略 = 呼び出し元の会話）と、
# 変えてはいけない挙動（pane 明示・card 指定の list / dismiss・呼び出し元が無い経路の断り）も見る。
#
# 最後に visual-test の `show-command-caller` 節で、MCP のエンジンを呼び出し元 A の文脈で回した
# カードが **A の実ピクセルに**描かれ、B の下端は 1 画素も変わらないことを見る（Metal の scene を
# 読み戻すので、蓋を閉じた機でも撮れる）。画像は `$1`（既定は一時 dir = 終わると消える）へ落ちる。
# 画像にユーザー名・ホスト名が写らないよう、visual 段のシェルは隔離 HOME + 固定プロンプトで起こす（#927）。
#
# 本番の tako（GUI / 設定 / tmux）には触らない。修正前のバイナリ（visual-test 付きでビルドしたもの）で
# 流すと show / link の項目が NG・visual 段が ① で FAILED になる:
#   TAKO_BIN=<修正前の tako> APP_BIN=<修正前の tako-app> bash scripts/test-show-command-caller-1958.sh
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
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
fi
isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk1958-XXXXXX)"
DUMP_DIR="${1:-$TMP/dump}"
TMUX_SOCK="tako-iso-1958-$$"
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
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done

# 面（tako-vd）を用意できなければ起動せずに 4 で返る（#1744）。未実測として同じ 4 で止める
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$TMP/app.log"; then
  echo "隔離 GUI が立たない（ログ: $TMP/app.log）"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"
# 隔離 GUI でも自動命名は既定 ON で claude を呼ぶので、構成を作る前に止める
"$TAKO_BIN" autorename off >/dev/null 2>&1 || true

# A = 呼び出し元。B を A から割ってフォーカスを移す（= 呼び出し元とフォーカスを分ける）
A="$("$TAKO_BIN" list | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["tabs"][0]["panes"][0]["id"])')"
B="$("$TAKO_BIN" split --pane "$A" --focus)"
FOCUSED="$("$TAKO_BIN" list | python3 -c '
import json,sys
d=json.load(sys.stdin)
print(",".join(str(p["id"]) for t in d["tabs"] for p in t["panes"] if p.get("focused")))')"
echo "  呼び出し元 A=${A} / フォーカス B=${B}（focused=${FOCUSED}）"
check "$([ "$FOCUSED" = "$B" ] && echo 1)" "フォーカスは B にあり、呼び出し元 A とは別のペイン"

export A B TAKO_BIN
export DISC="$TAKO_DISCOVERY_DIR"

cat > "$TMP/drive.py" <<'PY'
import json, os, subprocess, sys, urllib.request

info = json.load(open(os.path.join(os.environ["DISC"], "control.json")))
A, B = int(os.environ["A"]), int(os.environ["B"])
tako = os.environ["TAKO_BIN"]


class Http:
    """GUI 内蔵 MCP。caller=None なら X-Tako-Pane を付けない（tako の外の MCP クライアント）"""

    def __init__(self, caller):
        self.caller, self.n = caller, 0

    def rpc(self, method, params):
        self.n += 1
        headers = {
            "Authorization": "Bearer " + info["token"],
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.caller is not None:
            headers["X-Tako-Pane"] = str(self.caller)
        body = json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
        req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())

    def close(self):
        pass


class Stdio:
    """`tako mcp serve`。caller=None なら TAKO_PANE_ID を渡さない"""

    def __init__(self, caller):
        env = dict(os.environ)
        env.update(TAKO_SOCKET=info["socket"], TAKO_TOKEN=info["token"])
        env.pop("TAKO_PANE_ID", None)
        if caller is not None:
            env["TAKO_PANE_ID"] = str(caller)
        self.p = subprocess.Popen([tako, "mcp", "serve"], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, env=env, text=True, bufsize=1)
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18"})
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")

    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)


def tool(conn, name, args):
    """(本文 JSON or 文字列, isError, JSON-RPC エラー文)"""
    r = conn.rpc("tools/call", {"name": name, "arguments": args})
    if "error" in r:
        return None, None, r["error"]["message"]
    res = r["result"]
    text = res["content"][0]["text"]
    try:
        body = json.loads(text)
    except ValueError:
        body = text
    return body, bool(res.get("isError")), None


def cli(args, caller):
    env = dict(os.environ)
    env.update(TAKO_SOCKET=info["socket"], TAKO_TOKEN=info["token"])
    env.pop("TAKO_PANE_ID", None)
    if caller is not None:
        env["TAKO_PANE_ID"] = str(caller)
    p = subprocess.run([tako] + args, capture_output=True, text=True, env=env)
    try:
        return json.loads(p.stdout), p.returncode, p.stderr.strip()
    except ValueError:
        return p.stdout.strip(), p.returncode, p.stderr.strip()


def cards(pane):
    """GUI の保管庫（画面のカード帯の描き元）にあるそのペインのカード"""
    body, _, _ = cli(["show-command", "--list", "--pane", str(pane)], None)
    return body.get("cards", []) if isinstance(body, dict) else []


def out(key, value):
    print("%s=%s" % (key, json.dumps(value, ensure_ascii=False)), flush=True)


route = sys.argv[1]
if route in ("http", "stdio"):
    conn = (Http if route == "http" else Stdio)(A)
    # ① pane を省いた show → 呼び出し元 A へ
    body, is_error, rpc_error = tool(conn, "tako_show_command",
                                     {"commands": ["echo %s-1958" % route], "label": "%s-1958" % route})
    out("show", {"rpc_error": rpc_error, "is_error": is_error,
                 "pane": body.get("card", {}).get("pane") if isinstance(body, dict) else body})
    # ② pane 明示は今のまま（B へ出る）
    body, is_error, rpc_error = tool(conn, "tako_show_command",
                                     {"commands": ["echo %s-explicit" % route], "pane": B})
    explicit_card = body.get("card", {}).get("id") if isinstance(body, dict) else None
    out("explicit", {"rpc_error": rpc_error, "is_error": is_error,
                     "pane": body.get("card", {}).get("pane") if isinstance(body, dict) else body})
    # ③ card 指定の list は今のまま（呼び出し元 A を混ぜず、カードの所在 B を引く）
    body, is_error, rpc_error = tool(conn, "tako_show_command", {"action": "list", "card": explicit_card})
    out("list_by_card", {"rpc_error": rpc_error, "is_error": is_error,
                         "pane": body.get("pane") if isinstance(body, dict) else body})
    # ④ card 省略の list は呼び出し元 A のカード
    body, is_error, rpc_error = tool(conn, "tako_show_command", {"action": "list"})
    out("list_caller", {"rpc_error": rpc_error, "is_error": is_error,
                        "pane": body.get("pane") if isinstance(body, dict) else body,
                        "labels": [c.get("label") for c in body.get("cards", [])] if isinstance(body, dict) else None})
    # ⑤ card 指定の dismiss は B のカードだけを消す（A のカードは残る）
    body, is_error, rpc_error = tool(conn, "tako_show_command", {"action": "dismiss", "card": explicit_card})
    out("dismiss_by_card", {"rpc_error": rpc_error, "is_error": is_error,
                            "dismissed": body.get("dismissed") if isinstance(body, dict) else body})
    # ⑥ link（id・pane 省略）は呼び出し元 A の会話を引く（フォーカスの B ではない）
    body, is_error, rpc_error = tool(conn, "tako_sessions", {"action": "link"})
    out("link", {"rpc_error": rpc_error, "is_error": is_error,
                 "pane": body.get("pane") if isinstance(body, dict) else body})
    conn.close()
    # 呼び出し元が無い経路（X-Tako-Pane / TAKO_PANE_ID なし）の show は送らずに断る
    none = (Http if route == "http" else Stdio)(None)
    body, is_error, rpc_error = tool(none, "tako_show_command", {"commands": ["echo none-1958"]})
    out("no_caller", {"rpc_error": rpc_error, "is_error": is_error, "body": body})
    none.close()
elif route == "cli":
    body, rc, err = cli(["show-command", "--label", "cli-1958", "echo cli-1958"], A)
    out("show", {"rc": rc, "stderr": err[-200:],
                 "pane": body.get("card", {}).get("pane") if isinstance(body, dict) else body})
    body, rc, err = cli(["sessions", "link", "--json"], A)
    out("link", {"rc": rc, "stderr": err[-200:], "pane": body.get("pane") if isinstance(body, dict) else body})
    body, rc, err = cli(["show-command", "echo none-1958"], None)
    out("no_caller", {"rc": rc, "stderr": err[-300:], "stdout": body if not isinstance(body, dict) else "json"})
elif route == "observe":
    out("cards_a", [c.get("label") for c in cards(A)])
    out("cards_b", [c.get("label") for c in cards(B)])
PY

# drive.py の 1 行 `key=json` から値を引く
val() { printf '%s\n' "$1" | sed -n "s/^$2=//p"; }
# 値が読めない（行が無い・JSON でない）ときは空 = NG に倒す
jq_py() { python3 -c "import json,sys; v=json.loads(sys.argv[1]); print($2)" "$1" 2>/dev/null; }

for route in http stdio; do
  echo "--- ${route}"
  OUT="$(python3 "$TMP/drive.py" "$route" 2>&1)"
  printf '%s\n' "$OUT" | sed 's/^/    /'
  SHOW="$(val "$OUT" show)"
  check "$(jq_py "$SHOW" '1 if v["pane"] == int("'"$A"'") and not v["is_error"] and not v["rpc_error"] else ""')" \
    "${route}: pane を省いた show が呼び出し元 A（${A}）へ出る"
  EXPL="$(val "$OUT" explicit)"
  check "$(jq_py "$EXPL" '1 if v["pane"] == int("'"$B"'") else ""')" \
    "${route}: pane 明示の show は名指しの B（${B}）へ出る（今のまま）"
  LBC="$(val "$OUT" list_by_card)"
  check "$(jq_py "$LBC" '1 if v["pane"] == int("'"$B"'") else ""')" \
    "${route}: card 指定の list は呼び出し元を混ぜずカードの所在 B を引く（今のまま）"
  LC="$(val "$OUT" list_caller)"
  check "$(jq_py "$LC" '1 if v["pane"] == int("'"$A"'") and "'"$route"'-1958" in (v["labels"] or []) else ""')" \
    "${route}: card 省略の list は呼び出し元 A のカードを返す"
  DBC="$(val "$OUT" dismiss_by_card)"
  check "$(jq_py "$DBC" '1 if v["dismissed"] == 1 else ""')" \
    "${route}: card 指定の dismiss は B のカード 1 枚だけを閉じる（今のまま）"
  LINK="$(val "$OUT" link)"
  check "$(jq_py "$LINK" '1 if v["pane"] == int("'"$A"'") else ""')" \
    "${route}: sessions link（id・pane 省略）は呼び出し元 A の会話を引く（フォーカスの B ではない）"
  NC="$(val "$OUT" no_caller)"
  check "$(jq_py "$NC" '1 if v["rpc_error"] and "対象ペインを特定できない" in v["rpc_error"] else ""')" \
    "${route}: 呼び出し元が無い show は送らず、渡すものを言って断る"
done

echo "--- cli"
OUT="$(python3 "$TMP/drive.py" cli 2>&1)"
printf '%s\n' "$OUT" | sed 's/^/    /'
check "$(jq_py "$(val "$OUT" show)" '1 if v["rc"] == 0 and v["pane"] == int("'"$A"'") else ""')" \
  "cli: --pane を省いた tako show-command が呼び出し元 A へ出る"
check "$(jq_py "$(val "$OUT" link)" '1 if v["rc"] == 0 and v["pane"] == int("'"$A"'") else ""')" \
  "cli: --pane を省いた tako sessions link は呼び出し元 A の会話を引く"
check "$(jq_py "$(val "$OUT" no_caller)" '1 if v["rc"] != 0 and "対象ペインを特定できない" in v["stderr"] else ""')" \
  "cli: TAKO_PANE_ID も --pane も無い show は送らず、渡すものを言って断る"

echo "--- GUI の保管庫（画面のカード帯の描き元）"
OUT="$(python3 "$TMP/drive.py" observe 2>&1)"
printf '%s\n' "$OUT" | sed 's/^/    /'
CA="$(val "$OUT" cards_a)"
CB="$(val "$OUT" cards_b)"
check "$(jq_py "$CA" '1 if sorted(x for x in v if x) == ["cli-1958", "http-1958", "stdio-1958"] else ""')" \
  "GUI: 呼び出し元 A に 3 経路のカードが 1 枚ずつ載っている"
check "$(jq_py "$CB" '1 if v == [] else ""')" \
  "GUI: フォーカスの B には残っていない（pane 明示の 2 枚は card 指定の dismiss で閉じた）"

# 1 本目の GUI を落としてから visual 段を起こす（同じ data dir を 2 つの GUI で握らない）
stop_isolated_gui "$APP_PID"
APP_PID=""

echo "--- visual-test show-command-caller（実ピクセル）"
mkdir -p "$TMP/home" "$TMP/zdot" "$DUMP_DIR"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
launch_isolated_gui "$TMP/visual.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=show-command-caller \
  TAKO_VISUAL_DUMP_DIR="$DUMP_DIR" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" || exit $?
APP_PID="$ISOLATED_GUI_PID"
for _ in $(seq 1 1200); do
  kill -0 "$APP_PID" 2>/dev/null || break
  sleep 0.1
done
stop_isolated_gui "$APP_PID"
APP_PID=""
grep -E "TAKO_VISUAL_PIXEL: show-command-caller caller|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED" \
  "$TMP/visual.log" | sed 's/^/    /'
check "$(grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual.log" && echo 1)" \
  "visual: MCP（呼び出し元 A）のカードが A の実ピクセルに描かれ、フォーカスの B は変わらない"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
