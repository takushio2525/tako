#!/bin/bash
# test-ui-thread-offload-1984.sh — UI スレッドの同期の dispatch が子プロセスを待たない（#1984）の実経路テスト
#
# #1979 で UI スレッドの子プロセスの待ちは全部「上限つき」になったが、次の経路は同期の dispatch の
# まま上限（器 5 秒・CLI 15 秒）まで、spawn の CLI の所在確認（ログインシェルの `command -v`）は
# **上限なしで** UI ごと止まりえた。#1984 で background へ出した
# （`OffloadJob::RemoteAgents` / `TmuxList` / 先取り = `Probe`）。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定には触らない）:
#   「武装している間は戻らない」偽のシェル（`SHELL`）と偽の tmux（`TAKO_TMUX_BIN`）を置き、
#   各要求を背景で打った最中に、別の要求（`tako theme` = 設定とメモリだけを読む）の応答時間を測る。
#   ① スマホの一覧（MCP `tako_remote_agents`）: `claude agents --json`（ログインシェル）
#   ② 呼び出し元の解決（`tako orchestrator self`）: pid 祖先辿り = 器の `list-panes -a`
#   ③ spawn（`tako orchestrator spawn`）: CLI の所在確認 = ログインシェルの `command -v`
#   ④ セッション再起動の計画（`tako session-restart` の下見）: 器の `list-panes` + `claude agents`
#   ⑤ 右パネルの fleet への切替（`tako panel --show --view fleet`）: `tmux list-sessions` ほか
#   ⑥ tmux 一覧（`tako tmux list`）
#   修正後はどれも別の要求が即座に返る（偽の子プロセスが注入どおり起きたことも数える）。
#   エッジ（修正後の腕だけ）:
#   E1 tmux が無い（`TAKO_TMUX_BIN` が存在しない）: tmux 一覧・fleet の切替が即座に返る
#   E2 子プロセスが即エラーで終わる（偽のシェルが即 exit 127）: スマホの一覧・spawn が理由つきで即座に断る
#   E3 background の結果が届く前にペインが閉じた: 再起動の下見がペインの不在で断る（固まらない）
#
# 偽のシェルは本物の claude を一切起こさない（`claude agents` には `[]` を返し、`command -v` は
# 見つからないと答える）。spawn が隔離ペインで本物の claude を立てて API を呼ぶことが無いように。
#
# A/B: `TAKO_1984_LEGACY=1 bash scripts/test-ui-thread-offload-1984.sh` で①〜⑥が名指しで FAILED
# （修正前の形 = UI スレッドの同期の dispatch が偽の子プロセスを待つ）。
#
# 使い方: bash scripts/test-ui-thread-offload-1984.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）。
# 本物の claude の設定 dir を指す env も落とす（書き込みはすべて一時の HOME へ向く）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE
unset CLAUDE_CONFIG_DIR CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_CHILD_SESSION

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY="${TAKO_1984_LEGACY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 別の要求の打ち切り（秒）と、「UI が止まっていない」とみなす上限（秒）。
# 修正後は 0.1 秒前後で返る。修正前は器の上限（5 秒）・CLI の上限（15 秒）・上限なし（spawn）まで返らない
UI_REPLY_LIMIT=8
UI_OK_SECS=1.0

TMP="$(mktemp -d /tmp/tako-1984-XXXXXX)"
ARM_SHELL="$TMP/arm-shell"
ARM_TMUX="$TMP/arm-tmux"
FAIL_SHELL="$TMP/fail-shell"
JOBS="$TMP/jobs.pids"
: > "$JOBS"
cleanup() {
  rm -f "$ARM_SHELL" "$ARM_TMUX" "$FAIL_SHELL"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 背景で打った CLI（**自分が記録した pid だけ**を落とす）
  if [ -s "$JOBS" ]; then
    while read -r p; do
      [ -n "$p" ] && kill "$p" 2>/dev/null
    done < "$JOBS"
  fi
  "${REAL_TMUX:-tmux}" -L "tako-1984-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1
REAL_TMUX="$(command -v tmux || true)"
[ -n "$REAL_TMUX" ] || { echo "未実測: tmux が無い（器のペインが作れない）"; exit 4; }
REAL_SHELL=/bin/zsh
[ -x "$REAL_SHELL" ] || REAL_SHELL=/bin/sh

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1984-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" \
  "$TMP/fakebin" "$TMP/faketmux" "$TMP/proj"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 偽のシェル: `-c` の呼び出し（ログインシェル経由の問い合わせ）だけを扱う。
# 種類ごとに起きた回数を残し、即エラーの印があれば即 exit 127・武装中は戻らない・外したら
# `claude agents` には `[]`、`command -v` には「見つからない」を返す（本物の claude を起こさない）。
# 対話シェル（ペインの中身）はそのまま本物へ渡す
cat > "$TMP/fakebin/shell" <<EOF
#!/bin/sh
case " \$* " in
  *" -c "*)
    case "\$*" in
      *"claude agents"*) kind=agents ;;
      *"command -v"*) kind=cmdv ;;
      *) kind=other ;;
    esac
    echo c >> "$TMP/fake-shell-\$kind.calls"
    [ -f "$FAIL_SHELL" ] && exit 127
    while [ -f "$ARM_SHELL" ]; do sleep 0.1; done
    case "\$kind" in
      agents) echo '[]'; exit 0 ;;
      cmdv) exit 1 ;;
    esac ;;
esac
exec "$REAL_SHELL" "\$@"
EOF
chmod +x "$TMP/fakebin/shell"

# 偽の tmux: 武装中は戻らない。外したら本物へそのまま渡す
cat > "$TMP/faketmux/tmux" <<EOF
#!/bin/sh
[ -f "$ARM_TMUX" ] && echo "t" >> "$TMP/fake-tmux.calls"
while [ -f "$ARM_TMUX" ]; do sleep 0.1; done
exec "$REAL_TMUX" "\$@"
EOF
chmod +x "$TMP/faketmux/tmux"

# GUI 内蔵 MCP の tools/call（本文 JSON を出す。isError なら終了コード 3）
cat > "$TMP/mcp.py" <<'PY'
import json, os, sys, urllib.request
info = json.load(open(os.path.join(os.environ["TAKO_DISCOVERY_DIR"], "control.json")))
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": sys.argv[1], "arguments": json.loads(sys.argv[2])}})
req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers={
    "Authorization": "Bearer " + info["token"],
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
})
with urllib.request.urlopen(req, timeout=120) as r:
    res = json.loads(r.read())
if "error" in res:
    print(res["error"]["message"]); sys.exit(2)
print(res["result"]["content"][0]["text"])
sys.exit(3 if res["result"].get("isError") else 0)
PY

# `cmd` を `secs` 秒で打ち切って実行し、所要（秒）を出す。打ち切ったら "timeout"
timed() {
  local secs="$1"; shift
  local t0 t1 rc
  t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  perl -e 'alarm shift; exec @ARGV' "$secs" "$@" > "$TMP/timed.out" 2>&1
  rc=$?
  t1=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  if [ "$rc" -eq 142 ]; then echo "timeout"; else perl -e "printf '%.2f', $t1 - $t0"; fi
}
ui_ok() { [ "$1" != "timeout" ] && awk -v t="$1" -v l="$UI_OK_SECS" 'BEGIN { exit !(t + 0 < l + 0) }'; }
count() { if [ -f "$1" ]; then wc -l < "$1" | tr -d ' '; else echo 0; fi; }

# メインスレッドの tako の関数のうち、子プロセスを待つ口が居座っているか（1 秒の sample。背景で採る）
start_sample() { sample "$ISOLATED_GUI_PID" 1 -file "$TMP/sample.txt" >/dev/null 2>&1 & SAMPLE_JOB=$!; }
main_thread_waits() {
  wait "$SAMPLE_JOB" 2>/dev/null
  [ -s "$TMP/sample.txt" ] || return 2
  awk '/Thread_.*com.apple.main-thread/{m=1; next} /^    [0-9]+ Thread_/{m=0} m' "$TMP/sample.txt" \
    > "$TMP/main-thread.txt"
  grep -E 'wait_with_timeout|platform::exe|Command::output|read_output|resolve_pane_by_pid|list_sessions|run_claude_agents|ProcessSnapshot|preflight' \
    "$TMP/main-thread.txt" >/dev/null || return 1
  grep -oE '(tako_control|tako_core)::[A-Za-z0-9_:]+' "$TMP/main-thread.txt" \
    | grep -E 'probe::|exe::|agents::|tmux::|dispatch::|agent_cli::|orchestrator::' \
    | sed 's/::h[0-9a-f]*$//' | awk '!seen[$0]++' | head -6 | sed 's/^/      メインスレッド: /'
  return 0
}

# 起動を待つ（`tako theme` = 器にもシェルにも触れない要求）
wait_ready() {
  local i
  for i in $(seq 1 300); do
    ( perl -e 'alarm shift; exec @ARGV' 3 "$TAKO_BIN" theme ) >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}

start_gui() {
  local log="$1"; shift
  launch_isolated_gui "$log" "$@" || exit $?
  wait_ready || { fail "GUI が立たない（${log}）"; tail -20 "$log"; exit 1; }
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
}

first_pane() {
  "$TAKO_BIN" list | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])' 2>/dev/null
}

# 1 件: <ラベル> <武装する印（空白区切り）> <起きるはずの偽の子プロセスの記録> <背景で打つコマンド...>
run_case() {
  local label="$1" arms="$2" calls_file="$3"; shift 3
  local before took waits rc after a
  rm -f "$TMP/bg.out" "$TMP/bg.rc"
  before="$(count "$calls_file")"
  for a in $arms; do : > "$a"; done
  ( perl -e 'alarm shift; exec @ARGV' 90 "$@" > "$TMP/bg.out" 2>&1; echo $? > "$TMP/bg.rc" ) &
  local bg=$!
  echo "$bg" >> "$JOBS"
  sleep 1
  start_sample
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
  if main_thread_waits; then waits="居る"; else waits="居ない"; fi
  after="$(count "$calls_file")"
  for a in $arms; do rm -f "$a"; done
  wait "$bg" 2>/dev/null
  rc="$(cat "$TMP/bg.rc" 2>/dev/null || echo '?')"
  echo "    ${label}: 最中の別の要求（tako theme）${took} 秒 / メインスレッドの子プロセス待ち ${waits} / 偽の子プロセス $((after - before)) 回 / 要求の終了コード ${rc}"
  if [ "$((after - before))" -lt 1 ]; then
    fail "${label}: 偽の子プロセスが一度も起きていない（注入が効いていない = 判定できない）"
  elif ui_ok "$took" && [ "$waits" != "居る" ]; then
    pass "${label}: UI が止まらない（別の要求が ${took} 秒で返る・メインスレッドに子プロセス待ちが無い）"
  else
    fail "${label}: UI が止まった（別の要求: ${took}・メインスレッドの子プロセス待ち: ${waits}）"
  fi
}

legacy_env=()
[ -n "$LEGACY" ] && legacy_env=(TAKO_1984_LEGACY=1)
arm="修正後"
[ -n "$LEGACY" ] && arm="修正前（TAKO_1984_LEGACY=1）"

echo "== ①〜⑥ 終わらない偽の子プロセスの最中に UI が止まらない / $arm"
# 器を有効にする（隔離 GUI は既定で OFF。付けないと pid 祖先辿り・器の一覧の経路に入らない）。
# claude agents の走査の鮮度を 5 秒へ寄せる（`TAKO_1011_LEGACY=1`。30 秒の使い回しに当たると注入が効かない）
start_gui "$TMP/app.log" TAKO_PERSIST=1 "SHELL=$TMP/fakebin/shell" \
  "TAKO_TMUX_BIN=$TMP/faketmux/tmux" TAKO_1011_LEGACY=1 ${legacy_env[@]+"${legacy_env[@]}"}
P1="$(first_pane)"
[ -n "$P1" ] || { fail "最初のペインの ID が取れない"; exit 1; }
"$TAKO_BIN" orchestrator projects add --key k1984 --cwd "$TMP/proj" >/dev/null 2>&1 \
  || fail "spawn 用のプロジェクトを足せない"
base="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
echo "    基準: 何も走っていないときの別の要求（tako theme）${base} 秒"

sleep 6 # 走査の使い回し窓（5 秒）を外す
run_case "① スマホの一覧（MCP tako_remote_agents）" "$ARM_SHELL" "$TMP/fake-shell-agents.calls" \
  python3 "$TMP/mcp.py" tako_remote_agents '{}'
run_case "② 呼び出し元の解決（tako orchestrator self）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" orchestrator self
run_case "③ spawn（tako orchestrator spawn）" "$ARM_SHELL" "$TMP/fake-shell-cmdv.calls" \
  "$TAKO_BIN" orchestrator spawn --project k1984 --prompt "hi" --pane "$P1"
sleep 6
run_case "④ セッション再起動の計画（tako session-restart）" "$ARM_TMUX $ARM_SHELL" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" session-restart --pane "$P1"
run_case "⑤ 右パネルの fleet への切替（tako panel --show --view fleet）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" panel --show --view fleet
"$TAKO_BIN" panel --hide >/dev/null 2>&1 || true
run_case "⑥ tmux 一覧（tako tmux list）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" tmux list

if [ -n "$LEGACY" ]; then
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
  echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
  [ "$FAIL" -eq 0 ]
  exit $?
fi

# 修正後の応答の中身（background から返っても同じ形）
if "$TAKO_BIN" tmux list > "$TMP/tmux-list.json" 2>&1 \
  && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if isinstance(d.get("sessions"), list) and any(s.get("backend") for s in d["sessions"]) else 1)' "$TMP/tmux-list.json"; then
  pass "⑥ tmux 一覧の応答は従来の形（sessions に器のセッションが backend: true で載る）"
else
  fail "⑥ tmux 一覧の応答の形が崩れた: $(head -c 200 "$TMP/tmux-list.json")"
fi
if python3 "$TMP/mcp.py" tako_remote_agents '{}' > "$TMP/agents.json" 2>&1 \
  && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("agents") == [] else 1)' "$TMP/agents.json"; then
  pass "① スマホの一覧の応答は従来の形（agents 配列）"
else
  fail "① スマホの一覧の応答の形が崩れた: $(head -c 200 "$TMP/agents.json")"
fi

echo "== E2 子プロセスが即エラーで終わる（偽のシェルが即 exit 127）"
: > "$FAIL_SHELL"
sleep 6
took="$(timed 10 python3 "$TMP/mcp.py" tako_remote_agents '{}')"
out="$(cat "$TMP/timed.out")"
echo "    スマホの一覧: ${took} 秒 / $(printf '%s' "$out" | head -c 120)"
if ui_ok "$took" && grep -q "claude agents --json の実行に失敗" <<<"$out"; then
  pass "E2 スマホの一覧が理由つきで即座に断る"
else
  fail "E2 スマホの一覧が即座に断らない（${took}）"
fi
took="$(timed 10 "$TAKO_BIN" orchestrator spawn --project k1984 --prompt "hi" --pane "$P1")"
out="$(cat "$TMP/timed.out")"
echo "    spawn: ${took} 秒 / $(printf '%s' "$out" | head -1 | head -c 160)"
if ui_ok "$took" && grep -q "claude" <<<"$out"; then
  pass "E2 spawn が CLI の不在を理由つきで即座に断る（ペインを作らない）"
else
  fail "E2 spawn が即座に断らない（${took}）"
fi
rm -f "$FAIL_SHELL"
panes="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(sum(len(t["panes"]) for t in json.load(sys.stdin)["tabs"]))')"
[ "$panes" = "1" ] && pass "E2 断った spawn は空ペインを残さない（ペイン ${panes} 枚）" \
  || fail "E2 断った spawn のあとにペインが ${panes} 枚ある"

echo "== E3 background の結果が届く前にペインが閉じた"
"$TAKO_BIN" split --pane "$P1" --down >/dev/null 2>&1 || fail "E3 2 枚目のペインを作れない"
sleep 1
P2="$("$TAKO_BIN" list | python3 -c '
import json, sys
d = json.load(sys.stdin)
print([p["id"] for p in d["tabs"][0]["panes"] if p["id"] != int(sys.argv[1])][0])' "$P1" 2>/dev/null)"
sleep 6
: > "$ARM_SHELL"
( perl -e 'alarm shift; exec @ARGV' 60 "$TAKO_BIN" session-restart --pane "$P2" > "$TMP/e3.out" 2>&1; echo $? > "$TMP/e3.rc" ) &
E3_JOB=$!
echo "$E3_JOB" >> "$JOBS"
sleep 1
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" close --pane "$P2" --force)"
echo "    走査の最中のペインの close: ${took} 秒"
ui_ok "$took" && pass "E3 走査の最中でもペインを閉じられる（${took} 秒）" \
  || fail "E3 走査の最中にペインを閉じられない（${took}）"
rm -f "$ARM_SHELL"
wait "$E3_JOB" 2>/dev/null
e3rc="$(cat "$TMP/e3.rc" 2>/dev/null || echo '?')"
echo "    再起動の下見: 終了コード ${e3rc} / $(head -c 160 "$TMP/e3.out")"
if [ "$e3rc" != "0" ] && [ "$e3rc" != "142" ] && grep -q "ペイン ${P2} が見つからない" "$TMP/e3.out"; then
  pass "E3 着いた答えはペインの不在で断る（落ちない・固まらない）"
else
  fail "E3 閉じたペインの下見の結末が想定外（終了コード ${e3rc}）"
fi
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
ui_ok "$took" && pass "E3 そのあとも UI は応答する（${took} 秒）" || fail "E3 そのあと UI が応答しない（${took}）"
stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

echo "== E1 tmux が無い（TAKO_TMUX_BIN が存在しない）"
start_gui "$TMP/app-notmux.log" TAKO_PERSIST=1 "TAKO_TMUX_BIN=$TMP/no-such-tmux"
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" tmux list)"
out="$(cat "$TMP/timed.out")"
echo "    tmux 一覧: ${took} 秒 / $(printf '%s' "$out" | tr -d '\n ' | head -c 80)"
if ui_ok "$took" && printf '%s' "$out" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin)["sessions"] == [] else 1)'; then
  pass "E1 tmux 一覧が空の sessions で即座に返る"
else
  fail "E1 tmux 一覧が即座に返らない / 形が想定外（${took}）"
fi
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" panel --show --view fleet)"
ui_ok "$took" && pass "E1 fleet への切替が即座に返る（${took} 秒）" || fail "E1 fleet への切替が返らない（${took}）"
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
ui_ok "$took" && pass "E1 そのあとも UI は応答する（${took} 秒）" || fail "E1 そのあと UI が応答しない（${took}）"
stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
