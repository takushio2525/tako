#!/bin/bash
# test-ui-thread-offload-1989.sh — UI スレッドの同期の dispatch が子プロセスを待たない（#1989）の実経路テスト
#
# #1984 で残った同じ型（UI スレッドで子プロセスを待つ）を background へ出した続き:
#   ⑦ コンフリクト解消エージェント（`tako git resolve`）: `git rev-parse` / `git status` を**上限なし**で待っていた
#   ⑧ `tako stale-binary status`: 器の `list-panes -a` + `claude` の所在と版の確認
#   ⑨ `tako tmux kill` / ⑩ `tako tmux resize` / ⑪ `tako tmux select-window` / ⑫ `tako tmux open` /
#   ⑬ `tako tmux cleanup`: tmux への問い合わせ・書き込み（1 本 5 秒の上限つきだが UI ごと止まる）
#   ⑭ agent CLI の所在確認（`agent_cli::locate`）: ログインシェルを**上限なし**で待っていた
#      = 返らないと spawn が永久に返らない（UI は #1984 で止まらなくなったが要求が返らない）
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / 既定の tmux サーバーには触らない）:
#   「武装している間は戻らない」偽の git（PATH の先頭）・tmux（`TAKO_TMUX_BIN`）・シェル（`SHELL`）を置き、
#   ⑦〜⑬ は要求を背景で打った最中の別の要求（`tako theme`）の応答時間、⑭ は spawn 自体が上限
#   （`TAKO_SETUP_PROBE_TIMEOUT_SECS=2`）で理由つきで返るかを測る。tmux の書き込みは隔離した器の
#   ソケットを明示する（既定サーバー = 本物の tmux に kill / resize を打たない）。
#   エッジ（修正後の腕だけ）: E4 git が無い（PATH から外す）・E5 解消エージェントの最中に元のペインが閉じた
#
# A/B: `TAKO_1989_LEGACY=1 bash scripts/test-ui-thread-offload-1989.sh` で⑦〜⑭が名指しで FAILED。
# 使い方: bash scripts/test-ui-thread-offload-1989.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）。
# 本物の claude の設定 dir を指す env も落とす（書き込みはすべて一時の HOME へ向く）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE
unset CLAUDE_CONFIG_DIR CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_CHILD_SESSION

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY="${TAKO_1989_LEGACY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 別の要求の打ち切り（秒）と、「UI が止まっていない」とみなす上限（秒）。
# 修正後は 0.1 秒前後で返る。修正前は器の上限（5 秒）・CLI の上限（15 秒）・上限なし（spawn）まで返らない
UI_REPLY_LIMIT=8
UI_OK_SECS=1.0

TMP="$(mktemp -d /tmp/tako-1989-XXXXXX)"
ARM_SHELL="$TMP/arm-shell"
ARM_TMUX="$TMP/arm-tmux"
ARM_GIT="$TMP/arm-git"
FAIL_SHELL="$TMP/fail-shell"
JOBS="$TMP/jobs.pids"
: > "$JOBS"
cleanup() {
  rm -f "$ARM_SHELL" "$ARM_TMUX" "$ARM_GIT" "$FAIL_SHELL"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 背景で打った CLI（**自分が記録した pid だけ**を落とす）
  if [ -s "$JOBS" ]; then
    while read -r p; do
      [ -n "$p" ] && kill "$p" 2>/dev/null
    done < "$JOBS"
  fi
  "${REAL_TMUX:-tmux}" -L "tako-1989-$$" kill-server >/dev/null 2>&1 || true
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
export TAKO_TMUX_SOCKET="tako-1989-$$"
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

# 偽の git: 武装中は戻らない。外したら本物へそのまま渡す（PATH の先頭に置く）
REAL_GIT="$(command -v git || true)"
[ -n "$REAL_GIT" ] || { echo "未実測: git が無い"; exit 4; }
cat > "$TMP/fakebin/git" <<EOF
#!/bin/sh
[ -f "$ARM_GIT" ] && echo "g" >> "$TMP/fake-git.calls"
while [ -f "$ARM_GIT" ]; do sleep 0.1; done
exec "$REAL_GIT" "\$@"
EOF
chmod +x "$TMP/fakebin/git"

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
  grep -E 'wait_with_timeout|platform::exe|Command::output|read_output|resolve_pane_by_pid|list_sessions|run_claude_agents|ProcessSnapshot|preflight|repo_root|conflict_state|resolve_session_socket|kill_window|resize_window|select_window|session_group|cleanup_orphans|find_claude_pid' \
    "$TMP/main-thread.txt" >/dev/null || return 1
  grep -oE '(tako_control|tako_core)::[A-Za-z0-9_:]+' "$TMP/main-thread.txt" \
    | grep -E 'probe::|exe::|agents::|tmux::|dispatch::|agent_cli::|orchestrator::|git::|tmux_backend::|stale_binary::' \
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
  rm -f "${TMP:?}/bg.out" "${TMP:?}/bg.rc"
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
[ -n "$LEGACY" ] && legacy_env=(TAKO_1989_LEGACY=1)
arm="修正後"
[ -n "$LEGACY" ] && arm="修正前（TAKO_1989_LEGACY=1）"

echo "== ⑦〜⑬ 終わらない偽の子プロセスの最中に UI が止まらない / $arm"
start_gui "$TMP/app.log" TAKO_PERSIST=1 "SHELL=$TMP/fakebin/shell" "PATH=$TMP/fakebin:$PATH" \
  "TAKO_TMUX_BIN=$TMP/faketmux/tmux" TAKO_SETUP_PROBE_TIMEOUT_SECS=2 \
  ${legacy_env[@]+"${legacy_env[@]}"}
P1="$(first_pane)"
[ -n "$P1" ] || { fail "最初のペインの ID が取れない"; exit 1; }
SOCK="$TAKO_TMUX_SOCKET"
"$REAL_TMUX" -L "$SOCK" new-session -d -s k1989 -x 80 -y 24 >/dev/null 2>&1 \
  || fail "kill / resize / open 用の器のセッションを作れない"
"$TAKO_BIN" orchestrator projects add --key k1989 --cwd "$TMP/proj" >/dev/null 2>&1 \
  || fail "spawn 用のプロジェクトを足せない"
base="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
echo "    基準: 何も走っていないときの別の要求（tako theme）${base} 秒"

run_case "⑦ コンフリクト解消エージェント（tako git resolve）" "$ARM_GIT" "$TMP/fake-git.calls" \
  "$TAKO_BIN" git resolve --agent claude --pane "$P1"
run_case "⑧ stale 判定（tako stale-binary status）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" stale-binary status --pane "$P1"
run_case "⑨ tmux の window の kill（tako tmux kill --window）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" tmux kill --session k1989 --window 9 --socket "$SOCK"
run_case "⑩ tmux のリサイズ（tako tmux resize）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" tmux resize --session k1989 --cols 100 --rows 30 --socket "$SOCK"
run_case "⑪ tmux の window 切替（tako tmux select-window）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" tmux select-window 0 --pane "$P1"
run_case "⑫ tmux セッションの取り込み（tako tmux open）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" tmux open k1989 --socket "$SOCK" --pane "$P1"
run_case "⑬ tmux の回収（tako tmux cleanup）" "$ARM_TMUX" "$TMP/fake-tmux.calls" \
  "$TAKO_BIN" tmux cleanup

echo "== ⑭ agent CLI の所在確認が返らないときに spawn が上限（2 秒）で理由つきで返る / $arm"
before="$(count "$TMP/fake-shell-cmdv.calls")"
: > "$ARM_SHELL"
took="$(timed 10 "$TAKO_BIN" orchestrator spawn --project k1989 --prompt "hi" --pane "$P1")"
out="$(cat "$TMP/timed.out")"
rm -f "$ARM_SHELL"
calls=$(( $(count "$TMP/fake-shell-cmdv.calls") - before ))
echo "    spawn: ${took} 秒 / 所在確認のシェル ${calls} 回 / $(printf '%s' "$out" | head -1 | head -c 140)"
if [ "$calls" -lt 1 ]; then
  fail "⑭ 所在確認のシェルが一度も起きていない（注入が効いていない = 判定できない）"
elif [ "$took" != "timeout" ] && grep -q "上限時間内に確かめられませんでした" <<<"$out"; then
  pass "⑭ spawn が ${took} 秒で返り、「確かめられなかった」と理由つきで断る（見つからないとは言わない）"
else
  fail "⑭ spawn が上限で返らない（${took}）= 所在確認を上限なしで待っている"
fi
sleep 1

if [ -n "$LEGACY" ]; then
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
  echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
  [ "$FAIL" -eq 0 ]
  exit $?
fi

# 修正後の応答の中身（background から返っても同じ形）
"$TAKO_BIN" stale-binary status --pane "$P1" > "$TMP/stale.json" 2>&1
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if "stale" in d and d.get("pane") == int(sys.argv[2]) else 1)' "$TMP/stale.json" "$P1" \
  && pass "⑧ stale 判定の応答は従来の形（stale / pane）" || fail "⑧ stale 判定の応答の形が崩れた: $(head -c 200 "$TMP/stale.json")"
if "$TAKO_BIN" tmux resize --session k1989 --cols 90 --rows 20 --socket "$SOCK" > "$TMP/resize.json" 2>&1 \
  && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("cols") == 90 and d.get("rows") == 20 else 1)' "$TMP/resize.json" \
  && [ "$("$REAL_TMUX" -L "$SOCK" display -p -t k1989 '#{window_width}x#{window_height}' 2>/dev/null)" = "90x20" ]; then
  pass "⑩ リサイズは background からでも実際に効く（90x20）"
else
  fail "⑩ リサイズが効いていない: $(head -c 200 "$TMP/resize.json")"
fi
"$TAKO_BIN" git resolve --agent claude --pane "$P1" > "$TMP/resolve.out" 2>&1
grep -q "git リポジトリが見つかりません\|no git repository found\|コンフリクトが発生していません" "$TMP/resolve.out" \
  && pass "⑦ 解消エージェントはリポジトリ外 / コンフリクト無しを理由つきで断る（ペインを作らない）" \
  || fail "⑦ 解消エージェントの断りが想定外: $(head -c 200 "$TMP/resolve.out")"

echo "== E5 解消エージェントの確かめの最中に元のペインが閉じた"
"$TAKO_BIN" split --pane "$P1" --down >/dev/null 2>&1 || fail "E5 2 枚目のペインを作れない"
sleep 1
P2="$("$TAKO_BIN" list | python3 -c '
import json, sys
d = json.load(sys.stdin)
print([p["id"] for p in d["tabs"][0]["panes"] if p["id"] != int(sys.argv[1])][-1])' "$P1" 2>/dev/null)"
: > "$ARM_GIT"
( perl -e 'alarm shift; exec @ARGV' 60 "$TAKO_BIN" git resolve --agent claude --pane "$P2" > "$TMP/e5.out" 2>&1; echo $? > "$TMP/e5.rc" ) &
E5_JOB=$!
echo "$E5_JOB" >> "$JOBS"
sleep 1
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" close --pane "$P2" --force)"
rm -f "$ARM_GIT"
wait "$E5_JOB" 2>/dev/null
e5rc="$(cat "$TMP/e5.rc" 2>/dev/null || echo '?')"
echo "    確かめの最中の close: ${took} 秒 / 解消エージェント: 終了コード ${e5rc} / $(head -c 160 "$TMP/e5.out")"
ui_ok "$took" && pass "E5 確かめの最中でもペインを閉じられる（${took} 秒）" || fail "E5 確かめの最中にペインを閉じられない（${took}）"
# ペインの不在の文は表示言語で日英どちらにもなる（#1762）
if [ "$e5rc" != "0" ] && [ "$e5rc" != "142" ] && grep -q "ペイン ${P2} が見つからない\|pane ${P2} not found\|git リポジトリが見つかりません\|no git repository found\|コンフリクトが発生していません" "$TMP/e5.out"; then
  pass "E5 着いた答えは理由つきで断る（落ちない・固まらない・ペインを作らない）"
else
  fail "E5 閉じたペインの解消エージェントの結末が想定外（終了コード ${e5rc}）"
fi
stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

echo "== E4 git が無い（PATH から外す）"
mkdir -p "$TMP/nogitbin"
start_gui "$TMP/app-nogit.log" TAKO_PERSIST=1 "PATH=$TMP/nogitbin:/usr/sbin:/sbin" "TAKO_TMUX_BIN=$REAL_TMUX"
P1="$(first_pane)"
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" git resolve --agent claude --pane "$P1")"
out="$(cat "$TMP/timed.out")"
echo "    解消エージェント: ${took} 秒 / $(printf '%s' "$out" | head -1 | head -c 120)"
if ui_ok "$took" && grep -q "git リポジトリが見つかりません\|no git repository found" <<<"$out"; then
  pass "E4 git が無ければ理由つきで即座に断る"
else
  fail "E4 git が無いときの結末が想定外（${took}）"
fi
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
ui_ok "$took" && pass "E4 そのあとも UI は応答する（${took} 秒）" || fail "E4 そのあと UI が応答しない（${took}）"
stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
