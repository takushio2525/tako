#!/bin/bash
# test-changelog-offload-2014.sh — プレビューの変更履歴の切替が UI スレッドで git を待たない（#2014）の実経路テスト
#
# 何が起きていたか: 変更履歴を開く切替（CLI `tako preview-changelog on` / MCP `tako_preview_changelog` /
#   GUI の「履歴」ボタン）が UI スレッドで `git rev-parse --show-toplevel` と `git log --follow` を
#   **上限なしで**待っていた。返らない git（ネットワーク上のフォルダ・ロック）で GUI ごと固まる
#   （#1976 / #1979 / #1984 / #1991 と同じ型）。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / 本物のリポジトリには触らない）:
#   PATH の先頭に「武装している間は、一時リポジトリの中の変更履歴の 2 段だけ戻らない」偽の git を置く
#   （`--version` などそれ以外は本物へそのまま渡す）。
#   cli 段（腕ごとに GUI を立てる）:
#     N  武装なし: 開く切替が従来どおり 3 件を返し、閉じられる（新旧で同じ応答 = A/B）
#     H  武装して開く切替を背景で打った最中に、別の要求（`tako list` と MCP の `tako_list_panes`）が
#        即座に返るか・メインスレッドに子プロセスの待ちが居るか（`sample`）。修正後は上限
#        （`TAKO_CHANGELOG_GIT_TIMEOUT_SECS=3`）で失敗として返り、状態は変わらない
#     E1 git 管理外のファイル: 従来どおり `commits: 0` と「git 管理外」
#     E2 結果が届く前にペインが閉じた: 理由つきで返り、GUI は応答を続ける
#     E3 git を起こせない（`TAKO_GIT_BIN` が無いファイル = PATH に git が無い機と同じ「起動できない」）:
#        従来どおり「git 管理外」（失敗にしない）
#   visual 段: visual-test 節 `changelog-offload`（GUI の「履歴」ボタンの経路。押した更新の UI スレッドの
#     所要と、取っている間の往復の間隔・重ね押し・上限の通知・武装を解いた後の履歴ビューの絵）
#
# A/B: 同じバイナリの `TAKO_2014_LEGACY=1`（修正前 = UI スレッドで上限なしに待つ）を各段で並べて走らせる。
#   修正前の腕は偽の git が戻るまで UI が止まるので、スクリプトが外から武装を解いて戻す。
# 使い方: bash scripts/test-changelog-offload-2014.sh
#   STAGES="cli visual"（既定は両方）で段を選ぶ。OUT_DIR で画像の置き場を変える（既定は一時 dir = 消える）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_GIT_BIN
unset CLAUDE_CONFIG_DIR CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_CHILD_SESSION

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 別の要求の打ち切り（秒）と「UI が止まっていない」とみなす上限（秒）
UI_REPLY_LIMIT=8
UI_OK_SECS=1.0
# 修正後の上限（GUI へ渡す。既定 30 秒を検証のために縮める）
GIT_LIMIT=3

TMP="$(mktemp -d /tmp/tako-2014-XXXXXX)"
ARM="$TMP/arm"
CALLS="$TMP/fake-git.calls"
JOBS="$TMP/jobs.pids"
: > "$JOBS"
OUT_DIR="${OUT_DIR:-$TMP/shots}"
cleanup() {
  rm -f "$ARM"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 背景で打った CLI・外から武装を解く係（**自分が記録した pid だけ**を落とす）
  if [ -s "$JOBS" ]; then
    while read -r p; do
      [ -n "$p" ] && kill "$p" 2>/dev/null
    done < "$JOBS"
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）。
# visual-test 版は通常の起動もそのままできるので 1 回で両段に使う
isolated_gui_bins visual-test || exit 1
REAL_GIT="$(command -v git || true)"
[ -n "$REAL_GIT" ] || { echo "未実測: git が無い"; exit 4; }

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/fakebin" "$TMP/repo" "$TMP/plain"
for d in "$HOME" "$TAKO_ORCHESTRATOR_DIR" "$TMP/repo"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 一時リポジトリ（3 コミットのファイル a.txt）と、git 管理外のファイル
REPO="$TMP/repo"
g() { "$REAL_GIT" -C "$REPO" "$@"; }
g init -q -b main && g config user.email test@example.invalid && g config user.name testuser \
  && g config commit.gpgsign false \
  && for n in 1 2 3; do echo "$n" >> "$REPO/a.txt"; g add a.txt; g commit -q -m "c$n" || exit 1; done \
  || { echo "一時リポジトリを作れない"; exit 1; }
echo p > "$TMP/plain/p.txt"
REPO_REAL="$(cd "$REPO" && pwd -P)"

# 偽の git（PATH の先頭）: 武装中、一時リポジトリの中の変更履歴の 2 段だけ戻らない（起きた回数を残す）。
# それ以外（`--version`・ほかの場所・ほかの引数）は本物へそのまま渡す
cat > "$TMP/fakebin/git" <<EOF
#!/bin/sh
if [ -f "$ARM" ]; then
  case "\$(pwd -P)" in
    "$REPO_REAL"|"$REPO_REAL"/*)
      case " \$* " in
        *" rev-parse --show-toplevel "*|*" log --follow "*)
          echo "g \$*" >> "$CALLS"
          while [ -f "$ARM" ]; do sleep 0.1; done ;;
      esac ;;
  esac
fi
exec "$REAL_GIT" "\$@"
EOF
chmod +x "$TMP/fakebin/git"
GUI_PATH="$TMP/fakebin:$PATH"

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

jget() {
  python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(1)
for k in sys.argv[1].split("."):
    d = d.get(k) if isinstance(d, dict) else None
print("" if d is None else (json.dumps(d) if isinstance(d, (dict, list, bool)) else d))' "$1"
}

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
now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

# メインスレッドに子プロセスの待ちが居座っているか（1 秒の sample。背景で採る）
start_sample() { sample "$ISOLATED_GUI_PID" 1 -file "$TMP/sample.txt" >/dev/null 2>&1 & SAMPLE_JOB=$!; }
main_thread_waits() {
  wait "$SAMPLE_JOB" 2>/dev/null
  [ -s "$TMP/sample.txt" ] || return 2
  awk '/Thread_.*com.apple.main-thread/{m=1; next} /^    [0-9]+ Thread_/{m=0} m' "$TMP/sample.txt" \
    > "$TMP/main-thread.txt"
  grep -E 'wait_with_timeout|Command::output|repo_root|run_git|file_history|log_file_commits' \
    "$TMP/main-thread.txt" >/dev/null || return 1
  grep -oE '(tako_control|tako_core)::[A-Za-z0-9_:]+' "$TMP/main-thread.txt" \
    | grep -E 'git::|dispatch::|probe::' \
    | sed 's/::h[0-9a-f]*$//' | awk '!seen[$0]++' | head -6 | sed 's/^/      メインスレッド: /'
  return 0
}

wait_ready() {
  local i
  for i in $(seq 1 "${1:-300}"); do
    # 起動前の打ち切りでシェルが出す「Alarm clock」の 1 行も落とす
    { ( perl -e 'alarm shift; exec @ARGV' 3 "$TAKO_BIN" theme ) >/dev/null 2>&1; } 2>/dev/null && return 0
    sleep 0.1
  done
  return 1
}

# 腕ごとにデータを分ける（前の腕のレイアウトを持ち越さない）
start_gui() {
  local arm_name="$1" log="$2"; shift 2
  mkdir -p "$TMP/data-$arm_name"
  launch_isolated_gui "$log" "TAKO_DATA_DIR=$TMP/data-$arm_name" TAKO_AUTO_RENAME=0 \
    "PATH=$GUI_PATH" "TAKO_CHANGELOG_GIT_TIMEOUT_SECS=$GIT_LIMIT" ${1+"$@"} || exit $?
  export TAKO_DATA_DIR="$TMP/data-$arm_name"
  wait_ready || { fail "GUI が立たない（${log}）"; tail -20 "$log"; exit 1; }
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
}

# 開いた直後の元のペイン（`tako open` の分割元。GUI の中の要求には「呼び出し元のペイン」が無い）
root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])' 2>/dev/null
}
open_preview() { "$TAKO_BIN" open "$1" --pane "$ROOT_PANE" --new-tab 2>/dev/null | jget pane; }

STAGES="${STAGES:-cli visual}"
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---------------------------------------------------------------------------------------------
cli_arm() {
  local arm_name="$1" label="$2"; shift 2
  echo "== cli 段 / ${label}"
  start_gui "$arm_name" "$TMP/app-$arm_name.log" ${1+"$@"}
  ROOT_PANE="$(root_pane)"
  local P
  P="$(open_preview "$REPO/a.txt")"
  [ -n "$P" ] || { fail "${label}: プレビューを開けない"; return; }
  sleep 0.5

  # N 武装なし（A/B: 新旧で同じ応答）
  "$TAKO_BIN" preview-changelog on --pane "$P" > "$TMP/n-$arm_name.json" 2>&1
  echo "    N 開く: $(tr -d '\n' < "$TMP/n-$arm_name.json")"
  if [ "$(jget changelog < "$TMP/n-$arm_name.json")" = "true" ] && [ "$(jget commits < "$TMP/n-$arm_name.json")" = "3" ]; then
    pass "${label}: N 通常のリポジトリでは従来どおり 3 件の履歴が開く"
  else
    fail "${label}: N 履歴の応答が想定外"
  fi
  "$TAKO_BIN" preview-changelog off --pane "$P" > "$TMP/n-off.json" 2>&1
  [ "$(jget changelog < "$TMP/n-off.json")" = "false" ] \
    && pass "${label}: N 閉じる" || fail "${label}: N 閉じられない: $(head -c 160 "$TMP/n-off.json")"

  # H 武装して開く切替を背景で打ち、最中の別の要求を測る
  local before took took_mcp waits t0 rc elapsed after
  before="$(count "$CALLS")"
  : > "$ARM"
  t0="$(now)"
  ( perl -e 'alarm shift; exec @ARGV' 90 "$TAKO_BIN" preview-changelog on --pane "$P" > "$TMP/h.out" 2>&1
    echo $? > "$TMP/h.rc"; perl -e "printf '%.2f', $(now) - $t0" > "$TMP/h.secs" ) &
  local bg=$!
  echo "$bg" >> "$JOBS"
  sleep 1
  start_sample
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
  took_mcp="$(timed "$UI_REPLY_LIMIT" python3 "$TMP/mcp.py" tako_list_panes '{}')"
  if main_thread_waits; then waits="居る"; else waits="居ない"; fi
  echo "    H 最中の別の要求: tako list ${took} 秒 / MCP tako_list_panes ${took_mcp} 秒 / メインスレッドの子プロセス待ち ${waits}"
  if [ "$arm_name" = "legacy" ]; then
    # 修正前は git が戻るまで返らない。外から武装を解いて戻す
    rm -f "$ARM"
  fi
  wait "$bg" 2>/dev/null
  rm -f "$ARM"
  rc="$(cat "$TMP/h.rc" 2>/dev/null || echo '?')"
  elapsed="$(cat "$TMP/h.secs" 2>/dev/null || echo '?')"
  after="$(count "$CALLS")"
  echo "    H 開く切替: 終了コード ${rc} / ${elapsed} 秒 / 偽の git $((after - before)) 回 / $(head -c 200 "$TMP/h.out" | tr '\n' ' ')"
  if [ "$((after - before))" -lt 1 ]; then
    fail "${label}: H 偽の git が一度も起きていない（注入が効いていない = 判定できない）"
  elif [ "$arm_name" = "legacy" ]; then
    # 修正前の腕は「止まる」ことが再現（A/B の検出力）
    if ! ui_ok "$took" && ! ui_ok "$took_mcp"; then
      pass "${label}: H 修正前は返らない git の最中に別の要求が返らない（list ${took} / MCP ${took_mcp} / メインスレッド ${waits}）= 再現"
    else
      fail "${label}: H 修正前でも別の要求が返る（list ${took} / MCP ${took_mcp}。注入に検出力が無い）"
    fi
  elif ui_ok "$took" && ui_ok "$took_mcp" && [ "$waits" != "居る" ]; then
    pass "${label}: H 返らない git の最中も別の要求が即座に返る（list ${took} 秒 / MCP ${took_mcp} 秒・メインスレッドに待ち無し）"
  else
    fail "${label}: H 返らない git の最中に UI が止まった（list ${took} / MCP ${took_mcp} / メインスレッド ${waits}）"
  fi
  if [ "$arm_name" = "new" ]; then
    if [ "$rc" != "0" ] && grep -q "打ち切った" "$TMP/h.out" \
      && awk -v t="$elapsed" -v l="$GIT_LIMIT" 'BEGIN { exit !(t + 0 >= l - 0.5 && t + 0 < l + 5) }'; then
      pass "${label}: H 上限（${GIT_LIMIT} 秒）で失敗として返る（${elapsed} 秒・理由つき）"
    else
      fail "${label}: H 上限で失敗として返らない（終了コード ${rc} / ${elapsed} 秒）"
    fi
    if grep -q "$TMP" "$TMP/h.out"; then
      fail "${label}: H 失敗の文面にパスが載っている（#927）"
    else
      pass "${label}: H 失敗の文面にパスを載せない"
    fi
    "$TAKO_BIN" preview-changelog --pane "$P" > "$TMP/h-state.json" 2>&1
    [ "$(jget changelog < "$TMP/h-state.json")" = "false" ] \
      && pass "${label}: H 打ち切った後も状態は変わらない（changelog: false）" \
      || fail "${label}: H 打ち切った後の状態が想定外: $(head -c 160 "$TMP/h-state.json")"
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    ui_ok "$took" && pass "${label}: H そのあとも GUI は応答する（${took} 秒）" || fail "${label}: H そのあと GUI が応答しない（${took}）"
    # 武装を解けばもう一度開ける
    "$TAKO_BIN" preview-changelog on --pane "$P" > "$TMP/h-again.json" 2>&1
    [ "$(jget commits < "$TMP/h-again.json")" = "3" ] \
      && pass "${label}: H 武装を解けば開き直せる（3 件）" \
      || fail "${label}: H 開き直せない: $(head -c 160 "$TMP/h-again.json")"

    echo "== エッジ（修正後の腕）"
    local P2 P3 e2rc
    P2="$(open_preview "$TMP/plain/p.txt")"
    "$TAKO_BIN" preview-changelog on --pane "$P2" > "$TMP/e1.json" 2>&1
    echo "    E1 git 管理外: $(tr -d '\n' < "$TMP/e1.json")"
    [ "$(jget commits < "$TMP/e1.json")" = "0" ] && grep -q "git 管理外" "$TMP/e1.json" \
      && pass "E1 git 管理外のファイルは従来どおり commits 0 と「git 管理外」" \
      || fail "E1 git 管理外の応答が想定外"

    P3="$(open_preview "$REPO/a.txt")"
    : > "$ARM"
    ( perl -e 'alarm shift; exec @ARGV' 60 "$TAKO_BIN" preview-changelog on --pane "$P3" > "$TMP/e2.out" 2>&1; echo $? > "$TMP/e2.rc" ) &
    local e2=$!
    echo "$e2" >> "$JOBS"
    sleep 1
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" close --pane "$P3" --force)"
    rm -f "$ARM"
    wait "$e2" 2>/dev/null
    e2rc="$(cat "$TMP/e2.rc" 2>/dev/null || echo '?')"
    echo "    E2 取っている最中の close: ${took} 秒 / 開く切替: 終了コード ${e2rc} / $(head -c 140 "$TMP/e2.out" | tr '\n' ' ')"
    if ui_ok "$took" && [ "$e2rc" != "0" ] && [ "$e2rc" != "142" ] && grep -q "プレビューペインではない\|not a preview pane" "$TMP/e2.out"; then
      pass "E2 取っている最中にペインを閉じられ、要求は理由つきで返る（宙に浮かない）"
    else
      fail "E2 取っている最中の close の結末が想定外（${took} / 終了コード ${e2rc}）"
    fi
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    ui_ok "$took" && pass "E2 そのあとも GUI は応答する（${took} 秒）" || fail "E2 そのあと GUI が応答しない（${took}）"
  fi
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

  if [ "$arm_name" = "new" ]; then
    echo "== E3 git を起こせない（TAKO_GIT_BIN が無いファイル）"
    start_gui nogit "$TMP/app-nogit.log" "TAKO_GIT_BIN=$TMP/nogit/git"
    ROOT_PANE="$(root_pane)"
    P="$(open_preview "$REPO/a.txt")"
    "$TAKO_BIN" preview-changelog on --pane "$P" > "$TMP/e3.json" 2>&1
    echo "    E3: $(tr -d '\n' < "$TMP/e3.json")"
    [ "$(jget changelog < "$TMP/e3.json")" = "true" ] && [ "$(jget commits < "$TMP/e3.json")" = "0" ] \
      && grep -q "git 管理外" "$TMP/e3.json" \
      && pass "E3 git を起こせないときは従来どおり「git 管理外」（失敗にしない）" \
      || fail "E3 git を起こせないときの応答が想定外"
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    ui_ok "$took" && pass "E3 GUI は応答する（${took} 秒）" || fail "E3 GUI が応答しない（${took}）"
    stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
  fi
}

if has_stage cli; then
  cli_arm new "修正後"
  cli_arm legacy "修正前（TAKO_2014_LEGACY=1）" TAKO_2014_LEGACY=1
  # A/B: 武装なしの応答が新旧で同じ
  if [ -s "$TMP/n-new.json" ] && cmp -s "$TMP/n-new.json" "$TMP/n-legacy.json"; then
    pass "A/B 武装なしの開く切替の応答が新旧でバイト一致（$(tr -d '\n' < "$TMP/n-new.json")）"
  else
    fail "A/B 武装なしの応答が新旧で違う"
  fi
fi

# ---------------------------------------------------------------------------------------------
# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local arm_name="$1" log="$2"; shift 2
  mkdir -p "$TMP/data-v$arm_name" "$OUT_DIR/$arm_name"
  rm -f "$ARM"
  launch_isolated_gui "$log" "TAKO_DATA_DIR=$TMP/data-v$arm_name" TAKO_AUTO_RENAME=0 \
    "PATH=$GUI_PATH" "TAKO_CHANGELOG_GIT_TIMEOUT_SECS=$GIT_LIMIT" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=changelog-offload "TAKO_VISUAL_DUMP_DIR=$OUT_DIR/$arm_name" \
    "TAKO_2014_FILE=$REPO/a.txt" "TAKO_2014_ARM=$ARM" "TAKO_2014_CALLS=$CALLS" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  return 0
}

if has_stage visual; then
  echo "== visual 段 / 修正後（GUI の「履歴」ボタン）"
  run_section new "$TMP/vnew.log"
  grep -E "TAKO_VISUAL_2014|TAKO_VISUAL_DUMP_FILE" "$TMP/vnew.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vnew.log"; then
    pass "visual 修正後: 全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "visual 修正後: 節が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$TMP/vnew.log" | tail -5 | sed 's/^/    /'
  fi

  echo "== visual 段 / 修正前（TAKO_2014_LEGACY=1）"
  # 修正前は押した更新が git の戻りまで返らない。節が武装したら 5 秒後に外から解く
  ( for i in $(seq 1 1200); do [ -f "$ARM" ] && break; sleep 0.1; done; sleep 5; rm -f "$ARM" ) &
  echo "$!" >> "$JOBS"
  run_section legacy "$TMP/vlegacy.log" TAKO_2014_LEGACY=1
  grep -E "TAKO_VISUAL_2014" "$TMP/vlegacy.log" | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: ① 「履歴」ボタンが UI スレッドで git を待たない" "$TMP/vlegacy.log"; then
    pass "visual 修正前は ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/vlegacy.log" | head -1)"
  else
    fail "visual 修正前でも ① で落ちない（節に検出力が無い）"
    grep -E "FAILED|panicked" "$TMP/vlegacy.log" | tail -3 | sed 's/^/    /'
  fi
fi

echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
