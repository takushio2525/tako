#!/bin/bash
# test-changelog-expand-2150.sh — プレビューの変更履歴の行の diff の展開が UI スレッドで git を待たない（#2150）の実経路テスト
#
# 何が起きていたか: 変更履歴の行の diff の展開（CLI `tako preview-changelog --expand <hash>` /
#   MCP `tako_preview_changelog` の `expand` / GUI のコミット行のクリック）が UI スレッドで
#   `git diff <hash>^..<hash>`（初期コミットは `git diff-tree --root -p`）を**上限なしで**待っていた。
#   巨大な差分・返らない git（ネットワーク上のフォルダ・ロック）で GUI ごと固まる
#   （#2014 の「開く切替」と同じ型の残り）。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / 本物のリポジトリには触らない）:
#   PATH の先頭に「武装している間は、一時リポジトリの中の行の diff の 2 段だけ戻らない」偽の git を置く
#   （変更履歴を開く `rev-parse` / `log --follow` や `--version` などそれ以外は本物へそのまま渡す）。
#   cli 段（腕ごとに GUI を立てる）:
#     N  武装なし: 展開 / 畳むが従来どおりの応答（新旧でバイト一致 = A/B。短縮ハッシュ・初期コミット・
#        知らないハッシュの失敗の文面も）
#     H  武装して CLI の展開を背景で打った最中に、別の要求（`tako list` と MCP の `tako_list_panes`）が
#        即座に返るか・メインスレッドに子プロセスの待ちが居るか（`sample`）。修正後は上限
#        （`TAKO_CHANGELOG_GIT_TIMEOUT_SECS=3`）で失敗として返り、行は畳んだまま
#     M  MCP の `tako_preview_changelog`（expand）でも同じ（修正後の腕）
#     E1 取っている最中に変更履歴を閉じた: 置かずに理由つきで返る（最後に着地した側が残らない）
#     E2 取っている最中にペインを閉じた: 理由つきで返り、GUI は応答を続ける
#     E3 巨大な差分（1 万行の書き換え = 2 万行）: 展開でき、そのあとも GUI は応答する
#     E4 バイナリの差分: 従来どおり展開できる（hunk は無い）
#   visual 段: visual-test 節 `changelog-expand`（GUI のコミット行のクリックの経路。押した更新の
#     UI スレッドの所要と、取っている間の往復の間隔・重ね押し・上限の通知・武装を解いた後の diff が
#     上限なしの版とバイト一致・畳む）
#
# A/B: 同じバイナリの `TAKO_2150_LEGACY=1`（修正前 = UI スレッドで上限なしに待つ）を各段で並べて走らせる。
#   修正前の腕は偽の git が戻るまで UI が止まるので、スクリプトが外から武装を解いて戻す。
# 使い方: bash scripts/test-changelog-expand-2150.sh
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

TMP="$(mktemp -d /tmp/tako-2150-XXXXXX)"
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
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  (cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
fi
isolated_gui_bins || exit 1
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
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/fakebin" "$TMP/repo"
for d in "$HOME" "$TAKO_ORCHESTRATOR_DIR" "$TMP/repo"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 一時リポジトリ: a.txt（3 コミット。最古は親の無い初期コミット）・big.txt（1 万行の書き換え）・
# bin.dat（バイナリの書き換え）
REPO="$TMP/repo"
g() { "$REAL_GIT" -C "$REPO" "$@"; }
g init -q -b main && g config user.email test@example.invalid && g config user.name testuser \
  && g config commit.gpgsign false \
  && for n in 1 2 3; do echo "$n" >> "$REPO/a.txt"; g add a.txt; g commit -q -m "c$n" || exit 1; done \
  || { echo "一時リポジトリを作れない"; exit 1; }
awk 'BEGIN { for (i = 0; i < 10000; i++) print "line " i }' > "$REPO/big.txt"
g add big.txt && g commit -q -m big1 || exit 1
awk 'BEGIN { for (i = 0; i < 10000; i++) print "LINE " i }' > "$REPO/big.txt"
g add big.txt && g commit -q -m big2 || exit 1
printf 'BIN\000\001\002\377' > "$REPO/bin.dat"
g add bin.dat && g commit -q -m bin1 || exit 1
printf 'BIN\000\011\011\377\007' > "$REPO/bin.dat"
g add bin.dat && g commit -q -m bin2 || exit 1
REPO_REAL="$(cd "$REPO" && pwd -P)"
A_HEAD="$(g log -1 --format=%H -- a.txt)"
A_SHORT="$(g log -1 --format=%h -- a.txt)"
A_ROOT="$(g log --format=%H -- a.txt | tail -1)"
BIG_HEAD="$(g log -1 --format=%H -- big.txt)"
BIN_HEAD="$(g log -1 --format=%H -- bin.dat)"

# 偽の git（PATH の先頭）: 武装中、一時リポジトリの中の行の diff の 2 段だけ戻らない（起きた回数を残す）。
# それ以外（変更履歴を開く 2 段・`--version`・ほかの場所・ほかの引数）は本物へそのまま渡す
cat > "$TMP/fakebin/git" <<EOF
#!/bin/sh
if [ -f "$ARM" ]; then
  case "\$(pwd -P)" in
    "$REPO_REAL"|"$REPO_REAL"/*)
      hang=0
      case "\$1" in
        diff) case "\$2" in *^..*) hang=1 ;; esac ;;
        diff-tree) [ "\$2" = "--root" ] && hang=1 ;;
      esac
      if [ "\$hang" = 1 ]; then
        echo "g \$*" >> "$CALLS"
        while [ -f "$ARM" ]; do sleep 0.1; done
      fi ;;
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
  grep -E 'wait_with_timeout|Command::output|run_git|diff_file_commit|commit_file_diff' \
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
# プレビューを開いて変更履歴へ切り替える（武装していない間に。ペイン ID を出す）
open_changelog() {
  local p
  p="$(open_preview "$1")"
  [ -n "$p" ] || return 1
  sleep 0.5
  "$TAKO_BIN" preview-changelog on --pane "$p" > "$TMP/open.json" 2>&1
  [ "$(jget changelog < "$TMP/open.json")" = "true" ] || return 1
  echo "$p"
}
expand() { "$TAKO_BIN" preview-changelog --pane "$1" --expand "$2"; }

STAGES="${STAGES:-cli visual}"
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# 武装して背景で `cmd...` を打つ（終了コード・所要・出力を $TMP/<tag>.{rc,secs,out} へ）
bg_armed() {
  local tag="$1"; shift
  local t0
  : > "$ARM"
  t0="$(now)"
  ( perl -e 'alarm shift; exec @ARGV' 90 "$@" > "$TMP/$tag.out" 2>&1
    echo $? > "$TMP/$tag.rc"; perl -e "printf '%.2f', $(now) - $t0" > "$TMP/$tag.secs" ) &
  BG=$!
  echo "$BG" >> "$JOBS"
}

# ---------------------------------------------------------------------------------------------
cli_arm() {
  local arm_name="$1" label="$2"; shift 2
  echo "== cli 段 / ${label}"
  start_gui "$arm_name" "$TMP/app-$arm_name.log" ${1+"$@"}
  ROOT_PANE="$(root_pane)"
  local P
  P="$(open_changelog "$REPO/a.txt")"
  [ -n "$P" ] || { fail "${label}: 変更履歴を開けない"; return; }

  # N 武装なし（A/B: 新旧で同じ応答。短縮ハッシュ・初期コミット・知らないハッシュ）
  local n="$TMP/n-$arm_name.txt"
  : > "$n"
  { expand "$P" "$A_SHORT"; expand "$P" "$A_SHORT"; expand "$P" "$A_ROOT"; expand "$P" "$A_ROOT"
    expand "$P" nope; } >> "$n" 2>&1
  echo "    N 展開 / 畳む / 初期コミット / 知らないハッシュ:"
  sed 's/^/      /' "$n"
  if [ "$(grep -c '"expanded": *true' "$n")" = "2" ] && [ "$(grep -c '"expanded": *false' "$n")" = "2" ] \
    && grep -q "コミット nope が見つからない" "$n" && grep -q "\"hash\": *\"$A_SHORT\"" "$n"; then
    pass "${label}: N 展開・畳む・初期コミット（diff-tree へ落ちる）・知らないハッシュの失敗が従来どおり"
  else
    fail "${label}: N 応答が想定外"
  fi

  # H 武装して展開を背景で打ち、最中の別の要求を測る
  local before took took_mcp waits rc elapsed after
  before="$(count "$CALLS")"
  bg_armed h "$TAKO_BIN" preview-changelog --pane "$P" --expand "$A_HEAD"
  local bg="$BG"
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
  echo "    H 展開: 終了コード ${rc} / ${elapsed} 秒 / 偽の git $((after - before)) 回 / $(head -c 200 "$TMP/h.out" | tr '\n' ' ')"
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
    if [ "$rc" != "0" ] && grep -q "打ち切った" "$TMP/h.out" && grep -q "git diff" "$TMP/h.out" \
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
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    ui_ok "$took" && pass "${label}: H そのあとも GUI は応答する（${took} 秒）" || fail "${label}: H そのあと GUI が応答しない（${took}）"
    # 打ち切った行は畳んだまま（武装を解いて押すと「開く」になる = 状態は変わっていない）
    expand "$P" "$A_HEAD" > "$TMP/h-again.json" 2>&1
    expand "$P" "$A_HEAD" > "$TMP/h-fold.json" 2>&1
    [ "$(jget expanded < "$TMP/h-again.json")" = "true" ] && [ "$(jget expanded < "$TMP/h-fold.json")" = "false" ] \
      && pass "${label}: H 打ち切った行は畳んだまま・武装を解けば展開できて畳める" \
      || fail "${label}: H 打ち切った後の行の状態が想定外: $(head -c 120 "$TMP/h-again.json")"

    # M MCP の展開でも同じ
    before="$(count "$CALLS")"
    bg_armed m python3 "$TMP/mcp.py" tako_preview_changelog "{\"pane\": $P, \"expand\": \"$A_HEAD\"}"
    bg="$BG"
    sleep 1
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    took_mcp="$(timed "$UI_REPLY_LIMIT" python3 "$TMP/mcp.py" tako_list_panes '{}')"
    wait "$bg" 2>/dev/null
    rm -f "$ARM"
    after="$(count "$CALLS")"
    echo "    M MCP の展開の最中: tako list ${took} 秒 / MCP tako_list_panes ${took_mcp} 秒 / 偽の git $((after - before)) 回 / 展開: 終了コード $(cat "$TMP/m.rc") / $(cat "$TMP/m.secs") 秒 / $(head -c 140 "$TMP/m.out" | tr '\n' ' ')"
    if [ "$((after - before))" -ge 1 ] && ui_ok "$took" && ui_ok "$took_mcp" && [ "$(cat "$TMP/m.rc")" = "3" ] \
      && grep -q "打ち切った" "$TMP/m.out"; then
      pass "M MCP の展開でも最中の別の要求が即座に返り、上限で isError として返る"
    else
      fail "M MCP の展開の結末が想定外"
    fi

    echo "== エッジ（修正後の腕）"
    # E1 取っている最中に変更履歴を閉じた → 置かずに理由つき（閉じた後に「開いた」結果が残らない）
    bg_armed e1 "$TAKO_BIN" preview-changelog --pane "$P" --expand "$A_HEAD"
    bg="$BG"
    sleep 1
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" preview-changelog off --pane "$P")"
    rm -f "$ARM"
    wait "$bg" 2>/dev/null
    "$TAKO_BIN" preview-changelog --pane "$P" > "$TMP/e1-state.json" 2>&1
    echo "    E1 取っている最中の off: ${took} 秒 / 展開: 終了コード $(cat "$TMP/e1.rc") / $(head -c 160 "$TMP/e1.out" | tr '\n' ' ') / その後 $(tr -d '\n' < "$TMP/e1-state.json")"
    if ui_ok "$took" && [ "$(cat "$TMP/e1.rc")" != "0" ] && grep -q "変更履歴が変わった" "$TMP/e1.out" \
      && [ "$(jget changelog < "$TMP/e1-state.json")" = "false" ]; then
      pass "E1 取っている最中に変更履歴を閉じると、展開は置かずに理由つきで返る（閉じたまま）"
    else
      fail "E1 取っている最中の off の結末が想定外"
    fi

    # E2 取っている最中にペインを閉じた
    local P3 e2rc
    P3="$(open_changelog "$REPO/a.txt")"
    bg_armed e2 "$TAKO_BIN" preview-changelog --pane "$P3" --expand "$A_HEAD"
    bg="$BG"
    sleep 1
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" close --pane "$P3" --force)"
    rm -f "$ARM"
    wait "$bg" 2>/dev/null
    e2rc="$(cat "$TMP/e2.rc" 2>/dev/null || echo '?')"
    echo "    E2 取っている最中の close: ${took} 秒 / 展開: 終了コード ${e2rc} / $(head -c 140 "$TMP/e2.out" | tr '\n' ' ')"
    if ui_ok "$took" && [ "$e2rc" != "0" ] && [ "$e2rc" != "142" ] && grep -q "プレビューペインではない" "$TMP/e2.out"; then
      pass "E2 取っている最中にペインを閉じられ、要求は理由つきで返る（宙に浮かない）"
    else
      fail "E2 取っている最中の close の結末が想定外（${took} / 終了コード ${e2rc}）"
    fi
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    ui_ok "$took" && pass "E2 そのあとも GUI は応答する（${took} 秒）" || fail "E2 そのあと GUI が応答しない（${took}）"

    # E3 巨大な差分（1 万行の書き換え = 2 万行の diff）
    local PB t_exp
    PB="$(open_changelog "$REPO/big.txt")"
    t_exp="$(timed 60 "$TAKO_BIN" preview-changelog --pane "$PB" --expand "$BIG_HEAD")"
    command cp -f "$TMP/timed.out" "$TMP/e3.json"
    sleep 1
    took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" list)"
    echo "    E3 巨大な差分の展開: ${t_exp} 秒 / $(tr -d '\n' < "$TMP/e3.json") / そのあとの tako list ${took} 秒"
    [ "$(jget expanded < "$TMP/e3.json")" = "true" ] \
      && pass "E3 巨大な差分（2 万行）も展開できる（${t_exp} 秒）" || fail "E3 巨大な差分を展開できない"
    if [ "$took" != "timeout" ]; then
      pass "E3 巨大な差分を展開したあとも GUI は応答する（tako list ${took} 秒）"
    else
      fail "E3 巨大な差分を展開したあと GUI が応答しない"
    fi

    # E4 バイナリの差分
    local PN
    PN="$(open_changelog "$REPO/bin.dat")"
    if [ -n "$PN" ]; then
      expand "$PN" "$BIN_HEAD" > "$TMP/e4.json" 2>&1
      echo "    E4 バイナリの差分の展開: $(tr -d '\n' < "$TMP/e4.json")"
      [ "$(jget expanded < "$TMP/e4.json")" = "true" ] \
        && pass "E4 バイナリの差分も従来どおり展開できる" || fail "E4 バイナリの差分を展開できない"
    else
      echo "    E4 未実測: bin.dat のプレビュー / 変更履歴を開けない"
    fi
  fi
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
}

if has_stage cli; then
  cli_arm new "修正後"
  cli_arm legacy "修正前（TAKO_2150_LEGACY=1）" TAKO_2150_LEGACY=1
  # A/B: 武装なしの応答が新旧で同じ
  if [ -s "$TMP/n-new.txt" ] && cmp -s "$TMP/n-new.txt" "$TMP/n-legacy.txt"; then
    pass "A/B 武装なしの展開・畳む・失敗の応答が新旧でバイト一致（$(wc -l < "$TMP/n-new.txt" | tr -d ' ') 行）"
  else
    fail "A/B 武装なしの応答が新旧で違う"
    diff "$TMP/n-new.txt" "$TMP/n-legacy.txt" | head -10 | sed 's/^/    /'
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
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=changelog-expand "TAKO_VISUAL_DUMP_DIR=$OUT_DIR/$arm_name" \
    "TAKO_2150_FILE=$REPO/a.txt" "TAKO_2150_ARM=$ARM" "TAKO_2150_CALLS=$CALLS" ${1+"$@"} || exit $?
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
  echo "== visual 段 / 修正後（GUI のコミット行のクリック）"
  run_section new "$TMP/vnew.log"
  grep -E "TAKO_VISUAL_2150|TAKO_VISUAL_DUMP_FILE" "$TMP/vnew.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vnew.log"; then
    pass "visual 修正後: 全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "visual 修正後: 節が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$TMP/vnew.log" | tail -5 | sed 's/^/    /'
  fi

  echo "== visual 段 / 修正前（TAKO_2150_LEGACY=1）"
  # 修正前は押した更新が git の戻りまで返らない。節が武装したら 5 秒後に外から解く
  ( for i in $(seq 1 1200); do [ -f "$ARM" ] && break; sleep 0.1; done; sleep 5; rm -f "$ARM" ) &
  echo "$!" >> "$JOBS"
  run_section legacy "$TMP/vlegacy.log" TAKO_2150_LEGACY=1
  grep -E "TAKO_VISUAL_2150" "$TMP/vlegacy.log" | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: ① コミット行のクリックが UI スレッドで git を待たない" "$TMP/vlegacy.log"; then
    pass "visual 修正前は ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/vlegacy.log" | head -1)"
  else
    fail "visual 修正前でも ① で落ちない（節に検出力が無い）"
    grep -E "FAILED|panicked" "$TMP/vlegacy.log" | tail -3 | sed 's/^/    /'
  fi
fi

echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
