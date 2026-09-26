#!/usr/bin/env bash
# test-self-pane-1516.sh — `orchestrator self --pane N` が名指しどおり答える実経路テスト（#1516）
#
# 隔離した data / tmux で**実 tako-app** を立て、master の役を持つペインを 2 枚
# （alpha / bravo）+ role なし 1 枚 + worker 1 枚 作って、
#
#   ① alpha のペインの中から `self --pane <bravo>` → pane_id / profile / profile_source /
#      role が**bravo のもの**になる（#1516 の本体。ペインの中から撃つので
#      pid 祖先辿りが解け、修正前は呼び出し元 = alpha が返っていた）
#   ② `--pane` 省略は従来どおり呼び出し元（alpha）を解決する（回帰なし）
#   ③ 自分を名指し（`--pane <alpha>`）は「私についての問い」として扱い、env の名乗りが効く
#   ④ tako の外（`TAKO_PANE_ID` 無し）からの名指しも通る
#   ⑤ エッジ: 存在しない pane はエラー（既定 master へ落ちない）/ role なしペイン /
#      worker ペインは pane_id を返しつつ理由を warnings に出す
#   ⑥ MCP `tako_orchestrator_self(pane=<bravo>)` も同じ（stdio ブリッジをペインの中で走らせる）
#   ⑦ A/B `TAKO_1516_LEGACY=1` で**同一バイナリのまま**症状（呼び出し元が返る）が再現する
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-self-pane-1516.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る。CI で走るのは画面の要らない
# モックテストだけ）。手元で走らせる前提の実経路テスト。
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
check_contains() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'${2}' を含まない: ${3}）" ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1516-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1516-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# **器を tmux にする**（①の pid 祖先辿りは tmux の pane_pid 経由でしか解けない。
# 直 PTY のペインはバックエンドを持たないので、修正前の症状そのものが再現しない）
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 応答から見たいものだけを 1 行へ（`--pane` の効きは pane_id / profile / role で決まる）
# **1 行 1 項目**で出す（`warnings` の本文に `role=...` が現れるので、
# 1 行へ並べて `sed` で抜くと貪欲マッチが警告文のほうを拾う = 実測で踏んだ）
PY_SELF='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
print("\n".join([
    "pane_id=%s" % d.get("pane_id"),
    "profile=%s" % d.get("profile"),
    "profile_source=%s" % d.get("profile_source"),
    "role=%s" % d.get("role"),
    "warnings=%s" % " | ".join(d.get("warnings") or []),
]))'
# 行頭の `<名前>=` だけを見る（警告文の中の `（role=null）` は行頭に来ない）
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }
oneline() { printf '%s' "$1" | tr '\n' ' '; }

# ペインの中で 1 行走らせて、標準出力 + 終了コードをファイルへ採る。
# **ペインの中から撃つ**ことが要点（外から撃つと pid 祖先辿りが解けず、
# 修正前でも名指しどおりの pane_id が返ってしまい A/B が成立しない）
run_in_pane() {
  local pane="$1" cmd="$2" out="$3"
  rm -f "$out" "$out.done"
  "$TAKO_BIN" send --pane "$pane" "{ $cmd ; } > $out 2>&1; echo rc=\$? > $out.done" >/dev/null || return 1
  local i=0
  while [ "$i" -lt 200 ]; do
    [ -f "$out.done" ] && return 0
    sleep 0.1
    i=$((i + 1))
  done
  echo "    (ペインの中のコマンドが 20 秒で終わらなかった: $cmd)" >&2
  return 1
}

echo "== 隔離 GUI を起こす =="
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

echo "== 検証用のペインを組む（master alpha / master bravo / role なし / worker） =="
ALPHA="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
BRAVO="$("$TAKO_BIN" split --pane "$ALPHA" --right | tr -dc '0-9')"
PLAIN="$("$TAKO_BIN" split --pane "$BRAVO" --down | tr -dc '0-9')"
WORKER="$("$TAKO_BIN" split --pane "$PLAIN" --down | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$ALPHA" --role "orchestrator-master:alpha" >/dev/null
"$TAKO_BIN" title --pane "$BRAVO" --role "orchestrator-master:bravo" >/dev/null
"$TAKO_BIN" title --pane "$WORKER" --role "orchestrator-worker:tako" >/dev/null
echo "  alpha=$ALPHA bravo=$BRAVO plain=$PLAIN worker=$WORKER"

# ペインのシェルがプロンプトへ来たことを確かめる（来ていないと以降が全部タイムアウトする）
if run_in_pane "$ALPHA" "echo ready" "$TMP/ready.txt"; then
  check_eq "alpha のペインでコマンドが走る" "ready" "$(cat "$TMP/ready.txt" 2>/dev/null | tr -d '\r\n')"
else
  fail "alpha のペインでコマンドが走らない（以降の実測ができないので中止）"
  echo "== 結果: PASS=$PASS FAIL=$FAIL =="
  exit 1
fi
# ペインの中の CLI が隔離インスタンスへ繋がっていること（本番へ漏れていない保証）
run_in_pane "$ALPHA" "$TAKO_BIN list" "$TMP/inpane-list.json" || true
check_eq "ペインの中の CLI も隔離インスタンスを見ている（タブ 1 枚）" "1" \
  "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["tabs"]))' "$TMP/inpane-list.json" 2>/dev/null || echo "?")"

SELF_CMD="TAKO_ORCHESTRATOR_ROLE=master:alpha $TAKO_BIN orchestrator self"

echo "== ① alpha の中から bravo を名指しする =="
run_in_pane "$ALPHA" "$SELF_CMD --pane $BRAVO" "$TMP/named.json" || true
NAMED="$(python3 -c "$PY_SELF" < "$TMP/named.json" 2>/dev/null)"
echo "  $(oneline "$NAMED")"
check_eq "名指ししたペインを返す" "$BRAVO" "$(field pane_id "$NAMED")"
check_eq "profile は名指し先のもの" "bravo" "$(field profile "$NAMED")"
check_eq "profile の出どころはペインの role ラベル" "pane_role" "$(field profile_source "$NAMED")"
check_eq "role は名指し先のラベル" "orchestrator-master:bravo" "$(field role "$NAMED")"

echo "== ⑦ A/B（TAKO_1516_LEGACY=1）で症状が再現する =="
run_in_pane "$ALPHA" "TAKO_1516_LEGACY=1 $SELF_CMD --pane $BRAVO" "$TMP/legacy.json" || true
LEGACY="$(python3 -c "$PY_SELF" < "$TMP/legacy.json" 2>/dev/null)"
echo "  $(oneline "$LEGACY")"
check_eq "legacy: 名指しが無視され呼び出し元が返る（= Issue の症状）" "$ALPHA" \
  "$(field pane_id "$LEGACY")"
check_eq "legacy: profile も呼び出し元のもの" "alpha" "$(field profile "$LEGACY")"

echo "== ② --pane 省略は従来どおり呼び出し元を解決する =="
run_in_pane "$ALPHA" "$SELF_CMD" "$TMP/caller.json" || true
CALLER="$(python3 -c "$PY_SELF" < "$TMP/caller.json" 2>/dev/null)"
echo "  $(oneline "$CALLER")"
check_eq "呼び出し元のペインを返す" "$ALPHA" "$(field pane_id "$CALLER")"
check_eq "profile は env の名乗りから" "alpha" "$(field profile "$CALLER")"
check_eq "出どころは caller_role" "caller_role" "$(field profile_source "$CALLER")"
run_in_pane "$ALPHA" "TAKO_1516_LEGACY=1 $SELF_CMD" "$TMP/caller-legacy.json" || true
CALLER_LEGACY="$(python3 -c "$PY_SELF" < "$TMP/caller-legacy.json" 2>/dev/null)"
check_eq "省略時は両アームで同じ（回帰なし）" "$CALLER" "$CALLER_LEGACY"

echo "== ③ 自分を名指しした場合は env の名乗りが効く =="
run_in_pane "$ALPHA" "$SELF_CMD --pane $ALPHA" "$TMP/selfnamed.json" || true
SELFNAMED="$(python3 -c "$PY_SELF" < "$TMP/selfnamed.json" 2>/dev/null)"
echo "  $(oneline "$SELFNAMED")"
check_eq "自分のペインを返す" "$ALPHA" "$(field pane_id "$SELFNAMED")"
check_eq "profile は env の名乗りから（solo の profile はここにしか無い）" "caller_role" \
  "$(field profile_source "$SELFNAMED")"

echo "== ④ tako の外（TAKO_PANE_ID 無し）からの名指し =="
OUTSIDE="$("$TAKO_BIN" orchestrator self --pane "$BRAVO" 2>&1 | python3 -c "$PY_SELF")"
echo "  $(oneline "$OUTSIDE")"
check_eq "外からでも名指しどおり" "$BRAVO" "$(field pane_id "$OUTSIDE")"
check_eq "外からでも profile は名指し先" "bravo" "$(field profile "$OUTSIDE")"

echo "== ⑤ エッジ =="
MISSING="$("$TAKO_BIN" orchestrator self --pane 999999 2>&1)"
MRC=$?
if [ "$MRC" != "0" ]; then
  pass "存在しない pane はエラー（既定の master へ落ちない）"
else
  fail "存在しない pane で成功してしまった: $MISSING"
fi
check_contains "エラー文が名指しした ID を挙げる" "999999" "$MISSING"
run_in_pane "$ALPHA" "$SELF_CMD --pane $PLAIN" "$TMP/plain.json" || true
PLAIN_OUT="$(python3 -c "$PY_SELF" < "$TMP/plain.json" 2>/dev/null)"
echo "  role なし: $(oneline "$PLAIN_OUT")"
check_eq "role なしペインでも pane_id は名指しどおり" "$PLAIN" "$(field pane_id "$PLAIN_OUT")"
check_eq "role なしペインの profile は既定へ" "default" "$(field profile "$PLAIN_OUT")"
check_contains "既定へ落ちた理由が warnings に出る" "master / solo のペインではない" "$PLAIN_OUT"
run_in_pane "$ALPHA" "$SELF_CMD --pane $WORKER" "$TMP/worker.json" || true
WORKER_OUT="$(python3 -c "$PY_SELF" < "$TMP/worker.json" 2>/dev/null)"
echo "  worker: $(oneline "$WORKER_OUT")"
check_eq "worker ペインでも pane_id は名指しどおり" "$WORKER" "$(field pane_id "$WORKER_OUT")"
check_eq "worker ペインの role をそのまま返す" "orchestrator-worker:tako" "$(field role "$WORKER_OUT")"
check_contains "worker でも理由が warnings に出る" "master / solo のペインではない" "$WORKER_OUT"

echo "== ⑥ MCP tako_orchestrator_self(pane=bravo) =="
# stdio ブリッジ（= claude が起動するのと同じ経路）をペインの中で走らせる。
# 接続情報（TAKO_SOCKET / TAKO_TOKEN）と TAKO_PANE_ID はペインの env から入る
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1516","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_orchestrator_self","arguments":{"pane":$BRAVO}}}
JSONL
PY_MCP='import json,sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    content = (msg.get("result") or {}).get("content") or []
    text = content[0].get("text") if content else ""
    try:
        d = json.loads(text)
    except Exception:
        print("__NOT_JSON__ %s" % text[:200]); sys.exit(0)
    print("\n".join([
        "pane_id=%s" % d.get("pane_id"),
        "profile=%s" % d.get("profile"),
        "profile_source=%s" % d.get("profile_source"),
        "role=%s" % d.get("role"),
    ]))
    sys.exit(0)
print("__NO_RESPONSE__=1")'
run_in_pane "$ALPHA" \
  "TAKO_ORCHESTRATOR_ROLE=master:alpha $TAKO_BIN mcp serve < $TMP/mcp-in.jsonl > $TMP/mcp-out.jsonl" \
  "$TMP/mcp-run.log" || true
MCP_OUT="$(python3 -c "$PY_MCP" "$TMP/mcp-out.jsonl" 2>/dev/null)"
echo "  $(oneline "$MCP_OUT")"
check_eq "MCP も名指ししたペインを返す" "$BRAVO" "$(field pane_id "$MCP_OUT")"
check_eq "MCP の profile も名指し先" "bravo" "$(field profile "$MCP_OUT")"
check_eq "MCP の role も名指し先のラベル" "orchestrator-master:bravo" "$(field role "$MCP_OUT")"
run_in_pane "$ALPHA" \
  "TAKO_1516_LEGACY=1 TAKO_ORCHESTRATOR_ROLE=master:alpha $TAKO_BIN mcp serve < $TMP/mcp-in.jsonl > $TMP/mcp-legacy.jsonl" \
  "$TMP/mcp-legacy.log" || true
MCP_LEGACY="$(python3 -c "$PY_MCP" "$TMP/mcp-legacy.jsonl" 2>/dev/null)"
echo "  legacy: $(oneline "$MCP_LEGACY")"
check_eq "legacy の MCP は profile を呼び出し元のものにする（= MCP 側の穴）" "alpha" \
  "$(field profile "$MCP_LEGACY")"

echo
echo "== 結果: PASS=$PASS FAIL=$FAIL =="
[ "$FAIL" -eq 0 ]
