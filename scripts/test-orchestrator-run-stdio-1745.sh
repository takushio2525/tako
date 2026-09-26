#!/usr/bin/env bash
# #1745: `tako_orchestrator_run` の非同期（spawn して即 run_id を返し、run_status /
# run_result で追う）を **3 経路**で実 GUI + 実 dispatch に通して測る。
#
#   - http  … GUI 内蔵 MCP（Streamable HTTP。`TAKO_MCP_URL`）
#   - stdio … `tako mcp serve` の stdio ブリッジ（claude へ user スコープで登録される既定の経路）
#   - cli   … `tako orchestrator run`（完了まで待つ）と `run-status` / `run-result`
#
# #1745 までは stdio ブリッジだけ非同期が使えず、`sync=true` を付けないと JSON-RPC エラー
# （-32602）で返っていた。run のレジストリは GUI プロセスにあり、stdio ブリッジは別プロセスで
# GUI の受け口へ非同期 run を頼む口を持っていなかったため。
#
# claude は同梱のスタブへ向くので**実エージェントは起動しない**。
# 本番の tako（GUI / 設定 / tmux / projects / workers）には触らない。
#
# 修正前のバイナリで流すと stdio の項目が NG になる（A/B）:
#   TAKO_BIN=<修正前の tako> APP_BIN=<修正前の tako-app> bash scripts/test-orchestrator-run-stdio-1745.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# 完了待ちのタイムアウト（run は起動後 20 秒待ってから見に行くので、それより長くする）
RUN_TIMEOUT="${RUN_TIMEOUT:-30}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk1745-XXXXXX)"
TMUX_SOCK="tako-iso-1745-$$"
APP_PID=""
cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  # 明示した tmux ソケットは tako の終了時に片付けられない（#770）ので自分で落とす。
  # **自分が名前を決めたソケットだけ**を -L で名指しする
  tmux -L "$TMUX_SOCK" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# claude のスタブ。`$TMP/stub-mode` で振る舞いを切り替える
#   stay … 起動したまま居座る（実物と同じ形。完了しないので timeout で終わる）
#   exit … 即座に終わる（worker が即終わる境界値）
mkdir -p "$TMP/bin"
cat > "$TMP/bin/claude" <<STUB
#!/bin/sh
echo "STUB_CLAUDE args=\$*"
if [ "\$(cat "$TMP/stub-mode" 2>/dev/null)" = "exit" ]; then exit 0; fi
sleep 600
STUB
chmod +x "$TMP/bin/claude"
echo stay > "$TMP/stub-mode"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERSIST=0
export TAKO_TMUX_SOCKET="$TMUX_SOCK"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done

cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
model: null
effort: high
env:
  PATH: "$TMP/bin:/usr/bin:/bin"
YAML
"$TAKO_BIN" orchestrator projects add --key t1745 --cwd "$TMP" \
  --description "#1745 の検証（自動削除される）" >/dev/null 2>&1

# 面（tako-vd）を用意できなければ起動せずに 4 で返る（#1744）。未実測として同じ 4 で止める
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$TMP/app.log"; then
  echo "隔離 GUI が立たない（ログ: $TMP/app.log）"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"

MASTER="$("$TAKO_BIN" list | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["tabs"][0]["panes"][0]["id"])')"
echo "  呼び出し元ペイン: ${MASTER}"
export MASTER TAKO_BIN RUN_TIMEOUT
export DISC="$TAKO_DISCOVERY_DIR"

# MCP の 2 経路を同じ手順で叩く係。1 経路 1 プロセスで、stdio はブリッジを 1 本だけ
# 立てて使い回す（claude がブリッジを握りっぱなしにするのと同じ形）
cat > "$TMP/drive.py" <<'PY'
import json, os, subprocess, sys, time, urllib.request

route, scenario = sys.argv[1], sys.argv[2]
info = json.load(open(os.path.join(os.environ["DISC"], "control.json")))
master = int(os.environ["MASTER"])
tako = os.environ["TAKO_BIN"]
timeout = int(os.environ["RUN_TIMEOUT"])


class Http:
    def __init__(self):
        self.n = 0

    def rpc(self, method, params):
        self.n += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
        req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers={
            "Authorization": "Bearer " + info["token"],
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "X-Tako-Pane": str(master),
        })
        with urllib.request.urlopen(req, timeout=600) as r:
            return json.loads(r.read())

    def close(self):
        pass


class Stdio:
    def __init__(self):
        env = dict(os.environ)
        env.update(TAKO_SOCKET=info["socket"], TAKO_TOKEN=info["token"], TAKO_PANE_ID=str(master))
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
    """(本文 JSON or 文字列, isError, JSON-RPC エラー文) を返す"""
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


def out(key, value):
    print("%s=%s" % (key, json.dumps(value, ensure_ascii=False)), flush=True)


def cli(*args):
    p = subprocess.run([tako] + list(args), capture_output=True, text=True)
    try:
        return json.loads(p.stdout)
    except ValueError:
        return {"_stdout": p.stdout.strip(), "_stderr": p.stderr.strip()[-300:]}


if scenario == "docs":
    # ツール説明と guide `spawning` を stdio ブリッジ経由で引き、挙動と食い違わないかを見る
    conn = Stdio()
    tools = conn.rpc("tools/list", {})["result"]["tools"]
    desc = next(t["description"] for t in tools if t["name"] == "tako_orchestrator_run")
    body, is_error, rpc_error = tool(conn, "tako_orchestrator_guide", {"topic": "spawning"})
    conn.close()
    text = body.get("text", "") if isinstance(body, dict) else ""
    running = text.split("## Spawning Workers")[0]
    out("guide_error", rpc_error or (body if is_error else None))
    out("guide_checks", {
        "stdio_bridge_same": "`tako mcp serve` stdio bridge" in running,
        "run_id_at_once": "`{ run_id, pane_id }` at once" in running,
        "run_result_once": "tako_orchestrator_run_result({ run_id })` once" in running,
        "sync_blocks": "`sync: true` blocks" in running,
        "no_single_call_claim": "all in a single MCP call" not in running,
    })
    out("description_checks", {
        "async_default": "即座に run_id を返す" in desc,
        "sync_blocks": "sync=true で完了までブロッキング" in desc,
        "no_http_only_caveat": "HTTP MCP 経由でのみ" not in desc,
    })
    sys.exit(0)

conn = Http() if route == "http" else Stdio()
run_args = {"project": "t1745", "prompt": "", "label": "t1745-%s-%s" % (route, scenario),
            "pane": master, "timeout_seconds": timeout}
if scenario == "sync":
    run_args["sync"] = True

t0 = time.time()
body, is_error, rpc_error = tool(conn, "tako_orchestrator_run", run_args)
out("start_elapsed", round(time.time() - t0, 1))
out("start_rpc_error", rpc_error)
out("start_is_error", is_error)
out("start_body", body)

if scenario == "sync" or not isinstance(body, dict) or "run_id" not in body:
    conn.close()
    sys.exit(0)

run_id, pane_id = body["run_id"], body["pane_id"]
statuses, cli_seen, other_seen = [], None, None
deadline = time.time() + timeout + 60
closed_midway = False
while time.time() < deadline:
    st, st_err, st_rpc = tool(conn, "tako_orchestrator_run_status", {"run_id": run_id})
    if st_rpc or st_err or not isinstance(st, dict):
        statuses.append({"error": st_rpc or st})
        break
    statuses.append("%s/%s" % (st.get("phase"), st.get("status")))
    if cli_seen is None and st.get("phase") == "running":
        # 別経路（CLI = IPC 直）から同じ run が見えるか = レジストリが GUI 側に 1 つか
        cli_seen = cli("orchestrator", "run-status", run_id)
        other = Stdio() if route == "http" else Http()
        other_seen, _, _ = tool(other, "tako_orchestrator_run_status", {"run_id": run_id})
        other.close()
    if scenario == "pane-gone" and not closed_midway and st.get("phase") == "running":
        tool(conn, "tako_close_pane", {"pane": pane_id, "force": True})
        closed_midway = True
    if st.get("phase") == "finished":
        break
    time.sleep(2)

# 同じ状態が続く区間は 1 つにまとめて並べる（遷移だけ見たい）
compact = []
for s in statuses:
    if not compact or compact[-1] != s:
        compact.append(s)
out("status_trail", compact)
out("cli_run_status", cli_seen)
out("other_route_run_status", other_seen)
res, res_err, res_rpc = tool(conn, "tako_orchestrator_run_result", {"run_id": run_id})
if isinstance(res, dict) and "output" in res:
    res["output"] = "<%d 文字>" % len(res["output"] or "")
out("result", res if not res_rpc else {"rpc_error": res_rpc})
again, again_err, again_rpc = tool(conn, "tako_orchestrator_run_result", {"run_id": run_id})
out("result_again_is_error", again_err)
out("result_again", again)
conn.close()
PY

# 1 シナリオを流して `key=json` 行を $TMP/<名前>.out へ残す
drive() {
  local name="$1"; shift
  echo
  echo "=== ${name} ==="
  python3 "$TMP/drive.py" "$@" > "$TMP/${name}.out" 2> "$TMP/${name}.err"
  sed 's/^/    /' "$TMP/${name}.out"
  if [ -s "$TMP/${name}.err" ]; then sed 's/^/    stderr: /' "$TMP/${name}.err" | tail -5; fi
}

# `key=json` の値を Python の式で調べる（$1 = ファイル, $2 = 式。v(キー) で値を引く）
judge() {
  python3 - "$1" "$2" <<'PY'
import json, sys
vals = {}
for line in open(sys.argv[1]):
    if "=" in line:
        k, v = line.rstrip("\n").split("=", 1)
        vals[k] = json.loads(v)
v = vals.get
try:
    print(1 if eval(sys.argv[2]) else 0)
except Exception:
    print(0)
PY
}

# --- 0. ツール説明と guide `spawning` が実際の挙動（非同期が既定・stdio でも同じ）を書いている ---
drive "docs-stdio" stdio docs
f="$TMP/docs-stdio.out"
check "$(judge "$f" 'v("guide_error") is None and all(v("guide_checks").values())')" \
  "guide spawning（stdio ブリッジ経由）が run_id → run_status → run_result と sync を書いている"
check "$(judge "$f" 'all(v("description_checks").values())')" \
  "tako_orchestrator_run の説明文（stdio ブリッジ経由）が非同期の既定と sync を書き、HTTP 限定の但し書きが無い"

# --- 1. 非同期 run: GUI 内蔵 MCP（HTTP）と stdio ブリッジで同じ答えになる ---
for route in http stdio; do
  drive "async-$route" "$route" async
  f="$TMP/async-$route.out"
  check "$(judge "$f" 'v("start_rpc_error") is None and v("start_is_error") is False')" \
    "$route: 非同期 run がエラーにならない"
  check "$(judge "$f" 'isinstance(v("start_body"), dict) and str(v("start_body").get("run_id","")).startswith("run-") and v("start_elapsed") < 10')" \
    "$route: spawn して即 run_id を返す（完了を待たない）"
  check "$(judge "$f" 'any(str(s).startswith("running/") for s in v("status_trail")) and v("status_trail")[-1] == "finished/timeout"')" \
    "$route: run_status で running → finished/timeout を追える"
  check "$(judge "$f" 'isinstance(v("cli_run_status"), dict) and v("cli_run_status").get("run_id") == v("start_body")["run_id"]')" \
    "$route: 同じ run が CLI の run-status からも見える（レジストリは GUI 側に 1 つ）"
  check "$(judge "$f" 'isinstance(v("other_route_run_status"), dict) and v("other_route_run_status").get("run_id") == v("start_body")["run_id"]')" \
    "$route: 同じ run がもう一方の MCP 経路からも見える"
  check "$(judge "$f" 'v("result").get("status") == "timeout" and v("result").get("closed") is True')" \
    "$route: run_result が status=timeout を返し auto_close で閉じる"
  check "$(judge "$f" 'v("result_again_is_error") is True')" \
    "$route: run_result の 2 回目は run_id が見つからないエラー"
done

# --- 2. sync=true は stdio でも従来どおり完了まで待って結果を返す ---
drive "sync-stdio" stdio sync
f="$TMP/sync-stdio.out"
check "$(judge "$f" 'isinstance(v("start_body"), dict) and v("start_body").get("status") == "timeout" and v("start_elapsed") >= '"$RUN_TIMEOUT")" \
  "stdio sync=true: 完了（timeout）まで待って status を返す"

# --- 3. CLI `tako orchestrator run` は完了まで待つ（既存の挙動が変わらない） ---
echo
echo "=== cli-run ==="
t0=$(date +%s)
"$TAKO_BIN" orchestrator run --project t1745 --prompt "" --label t1745-cli --pane "$MASTER" \
  --timeout "$RUN_TIMEOUT" > "$TMP/cli-run.json" 2> "$TMP/cli-run.err"
t1=$(date +%s)
cli_status="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status"))' "$TMP/cli-run.json" 2>/dev/null)"
echo "    elapsed=$((t1 - t0)) status=${cli_status}"
check "$([ "$cli_status" = "timeout" ] && [ $((t1 - t0)) -ge "$RUN_TIMEOUT" ] && echo 1 || echo 0)" \
  "cli: tako orchestrator run は完了（timeout）まで待って status を返す"

# --- 4. 境界値（stdio 経路）: worker が即終わる / 途中で pane が消える ---
echo exit > "$TMP/stub-mode"
drive "exit-stdio" stdio async
f="$TMP/exit-stdio.out"
check "$(judge "$f" 'isinstance(v("start_body"), dict) and "run_id" in v("start_body") and v("status_trail")[-1].startswith("finished/")')" \
  "stdio: worker が即終わっても run_status が finished へ進む"
echo stay > "$TMP/stub-mode"

# 消えた pane は「gone が 3 回連続」（5 秒間隔）で確定するので、その前に期限が来ない長さにする
SAVED_TIMEOUT="$RUN_TIMEOUT"
export RUN_TIMEOUT=$((RUN_TIMEOUT + 15))
drive "gone-stdio" stdio pane-gone
export RUN_TIMEOUT="$SAVED_TIMEOUT"
f="$TMP/gone-stdio.out"
check "$(judge "$f" 'isinstance(v("start_body"), dict) and "run_id" in v("start_body") and v("status_trail")[-1] == "finished/error" and v("result").get("status") == "error"')" \
  "stdio: 途中で pane が消えたら run_status / run_result が status=error で確定する"

echo
echo "結果: ${PASS} PASS ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
