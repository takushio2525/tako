#!/bin/bash
# test-ui-thread-offload-1991.sh — git の書き込み系・起動時の orphan の後片付けが UI スレッドで
# 子プロセスを待たない（#1991）の実経路テスト
#
# #1968 / #1979 / #1984 / #1989 で UI スレッドの子プロセス待ちを background へ出してきた最後の残り:
#   G1〜G10 git の書き込み系（`tako git stage / unstage / commit / branch / checkout / merge / abort /
#           conflicts / pull / push` と MCP の `tako_git_push`）: `tako_core::git` は**上限なし**の
#           `Command::output()` で git を待つ（pull / push はネットワーク・認証まで）
#   R       起動時の orphan 自動復帰（#191）が UI スレッドで呼ぶ tmux の 3 口
#           （`session_cwd` / `session_env` / `set_session_env`）: 上限なしの `output()` / `status()`
#   起動時の orphan の一括回収（FR-2.16.11）は本体を background へ出した。ただし起動時の回収は
#   **他の tako-app が 1 つでも生きていれば見送る**（#1187 の Startup の規則）ので、本番 GUI が
#   動いている機の隔離 GUI では器まで届かない = この経路の注入 A/B は**未実測**として報告する
#   （見送りの 1 行が persist.log に残ることだけを確かめる）
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / 既定の tmux サーバー / 本物のリポジトリには触らない）:
#   「武装している間は戻らない」偽の git（`TAKO_GIT_BIN`）を置き、要求を背景で打った最中の
#   別の要求（`tako theme`）の応答時間と、メインスレッドに子プロセスの待ちが居るか（`sample`）を測る。
#   git の操作は一時リポジトリ（`$TMP/repo`）だけで行い、remote は 127.0.0.1 の偽のサーバーか閉じた port。
#   R は「武装している間は `show-environment` だけ戻らない」偽の tmux（`TAKO_TMUX_BIN`）を置き、
#   器の孤児が 1 つある状態で起動して、武装を解く前に起動が終わるかを見る
#   エッジ（修正後の腕だけ）: E1 ネットワークの無い pull / push（閉じた port）・E2 応答しない（認証の
#   やり取りで止まる）push・E2' 認証を求められる push（401）・E3 結果が届く前にペインが閉じた・
#   E4 同じリポジトリへ並べた stage 2 本（MCP で同時に）
#
# A/B: `TAKO_1991_LEGACY=1 bash scripts/test-ui-thread-offload-1991.sh` で G1〜G10 が名指しで FAILED、
#   R は修正前の上限なしの待ち（`TAKO_1979_LEGACY=1` の `run_tmux` = 修正前の 3 口と同じ素の `output()`）で FAILED。
# 使い方: bash scripts/test-ui-thread-offload-1991.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE
unset CLAUDE_CONFIG_DIR CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_CHILD_SESSION

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY="${TAKO_1991_LEGACY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 別の要求の打ち切り（秒）と、「UI が止まっていない」とみなす上限（秒）。
# 修正後は 0.1 秒前後で返る。修正前は偽の git が戻るまで（= 武装を解くまで）返らない
UI_REPLY_LIMIT=8
UI_OK_SECS=1.0

TMP="$(mktemp -d /tmp/tako-1991-XXXXXX)"
ARM_GIT="$TMP/arm-git"
ARM_TMUX="$TMP/arm-tmux"
JOBS="$TMP/jobs.pids"
: > "$JOBS"
cleanup() {
  rm -f "$ARM_GIT" "$ARM_TMUX"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 背景で打った CLI と偽のサーバー（**自分が記録した pid だけ**を落とす）
  if [ -s "$JOBS" ]; then
    while read -r p; do
      [ -n "$p" ] && kill "$p" 2>/dev/null
    done < "$JOBS"
  fi
  "${REAL_TMUX:-tmux}" -L "tako-1991-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える前にビルドする（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1
REAL_GIT="$(command -v git || true)"
[ -n "$REAL_GIT" ] || { echo "未実測: git が無い"; exit 4; }
REAL_TMUX="$(command -v tmux || true)"

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1991-$$"
# 一時リポジトリの git は本物の設定（~/.gitconfig の資格情報・署名）を読まない
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" \
  "$TMP/fakebin" "$TMP/faketmux" "$TMP/repo"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/repo"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 一時リポジトリ（ここだけに書く。本物のリポジトリへは commit / push しない）
REPO="$TMP/repo"
g() { "$REAL_GIT" -C "$REPO" "$@"; }
g init -q -b main && g config user.email test@example.invalid && g config user.name testuser \
  && g config commit.gpgsign false && echo a > "$REPO/a.txt" && g add a.txt && g commit -q -m init \
  || { echo "一時リポジトリを作れない"; exit 1; }

# 偽の git: 武装中は戻らない（起きた回数を残す）。外したら本物へそのまま渡す
cat > "$TMP/fakebin/git" <<EOF
#!/bin/sh
[ -f "$ARM_GIT" ] && echo "g \$*" >> "$TMP/fake-git.calls"
while [ -f "$ARM_GIT" ]; do sleep 0.1; done
exec "$REAL_GIT" "\$@"
EOF
chmod +x "$TMP/fakebin/git"

# 偽の tmux（R 用）: 武装中は `show-environment` だけ戻らない。ほかは本物へそのまま渡す
cat > "$TMP/faketmux/tmux" <<EOF
#!/bin/sh
case " \$* " in
  *" show-environment "*)
    if [ -f "$ARM_TMUX" ]; then
      echo "t" >> "$TMP/fake-tmux.calls"
      while [ -f "$ARM_TMUX" ]; do sleep 0.1; done
    fi ;;
esac
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

# 偽の remote（127.0.0.1）: `hang` = 受けて何も返さない（認証のやり取りで止まる push の代わり）/
# `401` = 認証を求める。待ち受けた port を `$2` へ書く
cat > "$TMP/remote.py" <<'PY'
import socket, sys, threading
mode, port_file = sys.argv[1], sys.argv[2]
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0)); s.listen(16)
open(port_file, "w").write(str(s.getsockname()[1]))
held = []
def serve(c):
    try:
        c.recv(65536)
        if mode == "401":
            c.sendall(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"tako\"\r\n"
                      b"Content-Length: 0\r\nConnection: close\r\n\r\n")
            c.close()
        else:
            held.append(c)  # 返さない
    except OSError:
        pass
while True:
    c, _ = s.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
PY
# 呼び出し側のシェルで呼ぶ（`$( … )` の中で呼ぶと pid が親に残らない）。結果は REMOTE_PID / REMOTE_PORT
start_remote() {
  local mode="$1" pf="$TMP/port-$1"
  rm -f "$pf"
  python3 "$TMP/remote.py" "$mode" "$pf" >/dev/null 2>&1 &
  REMOTE_PID=$!
  echo "$REMOTE_PID" >> "$JOBS"
  local i
  for i in $(seq 1 50); do [ -s "$pf" ] && break; sleep 0.1; done
  REMOTE_PORT="$(cat "$pf" 2>/dev/null)"
}
# 閉じた port（待ち受けを作ってすぐ閉じる = 接続を断られる = ネットワークの無い remote の代わり）
closed_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

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

# メインスレッドの tako の関数のうち、子プロセスを待つ口が居座っているか（1 秒の sample。背景で採る）
start_sample() { sample "$ISOLATED_GUI_PID" 1 -file "$TMP/sample.txt" >/dev/null 2>&1 & SAMPLE_JOB=$!; }
main_thread_waits() {
  wait "$SAMPLE_JOB" 2>/dev/null
  [ -s "$TMP/sample.txt" ] || return 2
  awk '/Thread_.*com.apple.main-thread/{m=1; next} /^    [0-9]+ Thread_/{m=0} m' "$TMP/sample.txt" \
    > "$TMP/main-thread.txt"
  grep -E 'wait_with_timeout|Command::output|repo_root|run_git|git_write_reply|session_env|session_cwd|cleanup_orphans' \
    "$TMP/main-thread.txt" >/dev/null || return 1
  grep -oE '(tako_control|tako_core)::[A-Za-z0-9_:]+' "$TMP/main-thread.txt" \
    | grep -E 'git::|dispatch::|tmux_backend::|tmux::|probe::' \
    | sed 's/::h[0-9a-f]*$//' | awk '!seen[$0]++' | head -6 | sed 's/^/      メインスレッド: /'
  return 0
}

# 起動を待つ（`tako theme` = 器にもシェルにも git にも触れない要求）
wait_ready() {
  local i
  for i in $(seq 1 "${1:-300}"); do
    ( perl -e 'alarm shift; exec @ARGV' 3 "$TAKO_BIN" theme ) >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}

# 自動命名（claude を呼ぶ）は起動時から止める（隔離 GUI でも本物の claude を起こさない）
start_gui() {
  local log="$1"; shift
  launch_isolated_gui "$log" TAKO_AUTO_RENAME=0 "$@" || exit $?
  wait_ready || { fail "GUI が立たない（${log}）"; tail -20 "$log"; exit 1; }
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
}

# 一時リポジトリを作業フォルダにしたペインを 1 枚立て、その ID を出す
repo_pane() {
  "$TAKO_BIN" tab new --cwd "$REPO" --title "g1991-$1" >/dev/null 2>&1 || return 1
  sleep 0.5
  "$TAKO_BIN" list | python3 -c '
import json, sys
d = json.load(sys.stdin)
ids = [p["id"] for t in d["tabs"] if t.get("title") == sys.argv[1] for p in t["panes"]]
print(ids[0] if ids else "")' "g1991-$1" 2>/dev/null
}

# 1 件: <ラベル> <背景で打つコマンド...>（偽の git を武装して打ち、最中の別の要求を測る）
run_case() {
  local label="$1"; shift
  local before took waits rc after
  rm -f "${TMP:?}/bg.out" "${TMP:?}/bg.rc"
  before="$(count "$TMP/fake-git.calls")"
  : > "$ARM_GIT"
  ( perl -e 'alarm shift; exec @ARGV' 90 "$@" > "$TMP/bg.out" 2>&1; echo $? > "$TMP/bg.rc" ) &
  local bg=$!
  echo "$bg" >> "$JOBS"
  sleep 1
  start_sample
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
  if main_thread_waits; then waits="居る"; else waits="居ない"; fi
  after="$(count "$TMP/fake-git.calls")"
  rm -f "$ARM_GIT"
  wait "$bg" 2>/dev/null
  rc="$(cat "$TMP/bg.rc" 2>/dev/null || echo '?')"
  echo "    ${label}: 最中の別の要求（tako theme）${took} 秒 / メインスレッドの子プロセス待ち ${waits} / 偽の git $((after - before)) 回 / 要求の終了コード ${rc}"
  if [ "$((after - before))" -lt 1 ]; then
    fail "${label}: 偽の git が一度も起きていない（注入が効いていない = 判定できない）"
  elif ui_ok "$took" && [ "$waits" != "居る" ]; then
    pass "${label}: UI が止まらない（別の要求が ${took} 秒で返る・メインスレッドに子プロセス待ちが無い）"
  else
    fail "${label}: UI が止まった（別の要求: ${took}・メインスレッドの子プロセス待ち: ${waits}）"
  fi
}

legacy_env=()
[ -n "$LEGACY" ] && legacy_env=(TAKO_1991_LEGACY=1)
arm="修正後"
[ -n "$LEGACY" ] && arm="修正前（TAKO_1991_LEGACY=1）"

echo "== G1〜G10 終わらない偽の git の最中に UI が止まらない / $arm"
start_gui "$TMP/app.log" "TAKO_GIT_BIN=$TMP/fakebin/git" ${legacy_env[@]+"${legacy_env[@]}"}
P="$(repo_pane main)"
[ -n "$P" ] || { fail "一時リポジトリのペインを作れない"; exit 1; }
DEAD_PORT="$(closed_port)"
g remote add origin "http://127.0.0.1:${DEAD_PORT}/r.git"
# upstream を置く（無いと pull / push は remote に触れる前に「upstream が無い」で断る）
g config branch.main.remote origin && g config branch.main.merge refs/heads/main
base="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
echo "    基準: 何も走っていないときの別の要求（tako theme）${base} 秒"

echo b > "$REPO/b.txt"
run_case "G1 stage（tako git stage）" "$TAKO_BIN" git stage b.txt --pane "$P"
run_case "G2 unstage（tako git unstage）" "$TAKO_BIN" git unstage b.txt --pane "$P"
g add b.txt
run_case "G3 commit（tako git commit）" "$TAKO_BIN" git commit -m "b" --pane "$P"
run_case "G4 ブランチ作成（tako git branch）" "$TAKO_BIN" git branch feat1991 --pane "$P"
run_case "G5 checkout（tako git checkout）" "$TAKO_BIN" git checkout main --pane "$P"
run_case "G6 merge の事前提示（tako git merge）" "$TAKO_BIN" git merge feat1991 --pane "$P"
run_case "G7 merge の中止（tako git abort）" "$TAKO_BIN" git abort --pane "$P"
run_case "G8 コンフリクト状態（tako git conflicts）" "$TAKO_BIN" git conflicts --pane "$P"
run_case "G9 pull（tako git pull）" "$TAKO_BIN" git pull --pane "$P"
run_case "G10 push（MCP の tako_git_push）" python3 "$TMP/mcp.py" tako_git_push "{\"pane\": $P}"

if [ -n "$LEGACY" ]; then
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
fi

if [ -z "$LEGACY" ]; then
  # 修正後の応答の中身（background から返っても同じ形・リポジトリに実際に効いている）
  echo c > "$REPO/c.txt"
  if "$TAKO_BIN" git stage c.txt --pane "$P" > "$TMP/stage.json" 2>&1 \
    && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("staged") is True and d.get("paths") == ["c.txt"] else 1)' "$TMP/stage.json" \
    && cached="$(g diff --cached --name-only)" && grep -qx c.txt <<<"$cached"; then
    pass "応答は従来の形（staged / paths）で、リポジトリに実際にステージされている"
  else
    fail "stage の応答 / 実体が想定外: $(head -c 200 "$TMP/stage.json")"
  fi
  "$TAKO_BIN" git commit -m "c" --pane "$P" > "$TMP/commit.json" 2>&1
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("committed") is True else 1)' "$TMP/commit.json" \
    && [ "$(g log -1 --format=%s)" = "c" ] \
    && pass "commit も background から実際に効く（HEAD の件名 = c）" \
    || fail "commit が効いていない: $(head -c 200 "$TMP/commit.json")"
  [ "$(g rev-parse --abbrev-ref HEAD)" = "main" ] && g rev-parse --verify -q feat1991 >/dev/null \
    && pass "G4 / G5 のブランチ作成と checkout が順に効いた（feat1991 がある・HEAD は main）" \
    || fail "G4 / G5 の結末が想定外（HEAD=$(g rev-parse --abbrev-ref HEAD)）"

  echo "== E1 ネットワークの無い pull / push（閉じた port = 接続を断られる）"
  for op in pull push; do
    took="$(timed 30 "$TAKO_BIN" git "$op" --pane "$P")"
    out="$(head -c 300 "$TMP/timed.out" | tr '\n' ' ')"
    echo "    ${op}: ${took} 秒 / ${out:0:140}"
    if [ "$took" != "timeout" ] && grep -qiE "unable to access|Could not connect|Failed to connect|Connection refused" "$TMP/timed.out"; then
      pass "E1 ${op} は理由つきのエラーで返る（${took} 秒）"
    else
      fail "E1 ${op} の結末が想定外（${took}）"
    fi
  done
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
  ui_ok "$took" && pass "E1 そのあとも UI は応答する（${took} 秒）" || fail "E1 そのあと UI が応答しない（${took}）"

  echo "== E2 応答しない remote への push（認証のやり取りで止まる形）"
  start_remote hang
  HANG_PID="$REMOTE_PID"
  g remote set-url origin "http://127.0.0.1:${REMOTE_PORT}/r.git"
  ( perl -e 'alarm shift; exec @ARGV' 60 "$TAKO_BIN" git push --pane "$P" > "$TMP/e2.out" 2>&1; echo $? > "$TMP/e2.rc" ) &
  E2_JOB=$!
  echo "$E2_JOB" >> "$JOBS"
  sleep 2
  start_sample
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
  if main_thread_waits; then waits="居る"; else waits="居ない"; fi
  # 同じ GUI の別のリポジトリ（ここでは同じリポジトリの読み取り = git log）も止まらない
  took_log="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" git log --max-count 1 --pane "$P")"
  echo "    push が止まっている最中: 別の要求 ${took} 秒 / git log ${took_log} 秒 / メインスレッドの子プロセス待ち ${waits}"
  if ui_ok "$took" && [ "$waits" != "居る" ] && [ "$took_log" != "timeout" ]; then
    pass "E2 応答しない push の最中も UI と読み取りは止まらない"
  else
    fail "E2 応答しない push の最中に UI が止まった（${took} / ${took_log} / ${waits}）"
  fi
  # remote を落とす = push が理由つきで返る（要求は宙に浮かない）
  kill "$HANG_PID" 2>/dev/null
  wait "$E2_JOB" 2>/dev/null
  e2rc="$(cat "$TMP/e2.rc" 2>/dev/null || echo '?')"
  echo "    remote を落とした後の push: 終了コード ${e2rc} / $(head -c 140 "$TMP/e2.out" | tr '\n' ' ')"
  [ "$e2rc" != "0" ] && [ "$e2rc" != "142" ] \
    && pass "E2 remote が切れると push は理由つきで返る（終了コード ${e2rc}）" \
    || fail "E2 remote を落としても push が返らない / 成功扱い（終了コード ${e2rc}）"

  echo "== E2' 認証を求められる push（401。端末の入力は塞いである）"
  start_remote 401
  g remote set-url origin "http://127.0.0.1:${REMOTE_PORT}/r.git"
  took="$(timed 30 "$TAKO_BIN" git push --pane "$P")"
  echo "    push: ${took} 秒 / $(head -c 160 "$TMP/timed.out" | tr '\n' ' ')"
  if [ "$took" != "timeout" ] && grep -qiE "terminal prompts disabled|Authentication failed|could not read Username" "$TMP/timed.out"; then
    pass "E2' 認証を求められた push は入力を待たずに理由つきで返る（${took} 秒）"
  else
    fail "E2' 認証を求められた push の結末が想定外（${took}）"
  fi

  echo "== E3 結果が届く前にペインが閉じた"
  P2="$(repo_pane e3)"
  echo d > "$REPO/d.txt"
  : > "$ARM_GIT"
  ( perl -e 'alarm shift; exec @ARGV' 60 "$TAKO_BIN" git stage d.txt --pane "$P2" > "$TMP/e3.out" 2>&1; echo $? > "$TMP/e3.rc" ) &
  E3_JOB=$!
  echo "$E3_JOB" >> "$JOBS"
  sleep 1
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" close --pane "$P2" --force)"
  rm -f "$ARM_GIT"
  wait "$E3_JOB" 2>/dev/null
  e3rc="$(cat "$TMP/e3.rc" 2>/dev/null || echo '?')"
  echo "    git の最中の close: ${took} 秒 / stage: 終了コード ${e3rc} / $(head -c 120 "$TMP/e3.out" | tr '\n' ' ')"
  ui_ok "$took" && pass "E3 git の最中でもペインを閉じられる（${took} 秒）" || fail "E3 git の最中にペインを閉じられない（${took}）"
  if [ "$e3rc" = "0" ] \
    && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("staged") is True else 1)' "$TMP/e3.out" \
    && cached="$(g diff --cached --name-only)" && grep -qx d.txt <<<"$cached"; then
    pass "E3 閉じたペインの要求も結果が届き、リポジトリに効いている（落ちない・宙に浮かない）"
  else
    fail "E3 閉じたペインの要求の結末が想定外（終了コード ${e3rc}）"
  fi
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" theme)"
  ui_ok "$took" && pass "E3 そのあとも UI は応答する（${took} 秒）" || fail "E3 そのあと UI が応答しない（${took}）"

  echo "== E4 同じリポジトリへ並べた stage 2 本（MCP で同時に）"
  echo e > "$REPO/e.txt"; echo f > "$REPO/f.txt"
  python3 "$TMP/mcp.py" tako_git_stage "{\"pane\": $P, \"paths\": [\"e.txt\"]}" > "$TMP/e4a.out" 2>&1 &
  E4A=$!
  python3 "$TMP/mcp.py" tako_git_stage "{\"pane\": $P, \"paths\": [\"f.txt\"]}" > "$TMP/e4b.out" 2>&1 &
  E4B=$!
  wait "$E4A"; ra=$?
  wait "$E4B"; rb=$?
  staged="$(g diff --cached --name-only | tr '\n' ' ')"
  echo "    stage e: ${ra} / stage f: ${rb} / ステージ済み: ${staged}"
  if [ "$ra" = "0" ] && [ "$rb" = "0" ] && grep -q "e.txt" <<<"$staged" && grep -q "f.txt" <<<"$staged"; then
    pass "E4 並べた 2 本はどちらも通る（index.lock を取り合わない = 修正前の UI スレッドの直列と同じ）"
  else
    fail "E4 並べた stage の片方が落ちた: $(head -c 160 "$TMP/e4a.out") / $(head -c 160 "$TMP/e4b.out")"
  fi
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
fi

# --- R: 起動時の orphan 自動復帰の tmux の 3 口 ---------------------------------
if [ -z "$REAL_TMUX" ]; then
  echo "  未実測: R（tmux が無い = 器の孤児を作れない）"
else
  r_env=()
  r_arm="修正後"
  if [ -n "$LEGACY" ]; then
    # 修正前の 3 口は素の `output()` / `status()`（上限なし）。同じ待ちは `run_tmux` の
    # #1979 の A/B（`TAKO_1979_LEGACY=1`）で再現する
    r_env=(TAKO_1979_LEGACY=1)
    r_arm="修正前（上限なしの待ち）"
  fi
  echo "== R 起動時の自動復帰の show-environment が返らない / $r_arm"
  "$REAL_TMUX" -L "$TAKO_TMUX_SOCKET" new-session -d -s "tako-orphan1991" -x 80 -y 24 >/dev/null 2>&1 \
    || fail "R 器の孤児を作れない"
  rm -f "${TMP:?}/data/layout.json"
  : > "$ARM_TMUX"
  launch_isolated_gui "$TMP/app-r.log" TAKO_AUTO_RENAME=0 TAKO_PERSIST=1 "TAKO_TMUX_BIN=$TMP/faketmux/tmux" \
    ${r_env[@]+"${r_env[@]}"} || exit $?
  t0="$(now)"
  # 武装を解くのは待ちが尽きてから（約 20〜30 秒後）。修正後は 3 口が 1 本 5 秒で打ち切られるので、
  # その前に起動が終わる
  ready=""
  if wait_ready 200; then ready="$(perl -e "printf '%.1f', $(now) - $t0")"; fi
  disarm_at=""
  if [ -z "$ready" ]; then
    rm -f "$ARM_TMUX"; disarm_at="$(perl -e "printf '%.1f', $(now) - $t0")"
    wait_ready 300 && ready="$(perl -e "printf '%.1f', $(now) - $t0")"
  fi
  rm -f "$ARM_TMUX"
  calls="$(count "$TMP/fake-tmux.calls")"
  echo "    起動に ${ready:-?} 秒（武装を解いた: ${disarm_at:-解く前に起動した}）/ 止めた show-environment ${calls} 回"
  if [ "$calls" -lt 1 ]; then
    fail "R 自動復帰の show-environment が一度も起きていない（孤児が拾われていない = 判定できない）"
  elif [ -z "$disarm_at" ] && [ -n "$ready" ]; then
    pass "R 返らない show-environment があっても起動は上限（1 本 5 秒）で終わる（${ready} 秒）"
  else
    fail "R 返らない show-environment で起動が止まった（武装を解くまで ${disarm_at} 秒立たない）"
  fi
  if [ -n "$ready" ]; then
    tree="$("$TAKO_BIN" list 2>/dev/null)" && grep -q '"tako-orphan1991"' <<<"$tree" \
      && pass "R 孤児は「復帰」タブへ戻る（上限で打ち切った口があっても復帰は続く）" \
      || fail "R 孤児が復帰していない"
    # 起動時の一括回収（FR-2.16.11）: 他の tako-app が生きていれば見送る（#1187 の Startup の規則）
    sleep 1
    line="$(grep -a "tmux cleanup: 起動時" "$TMP/data/persist.log" 2>/dev/null | tail -1)"
    echo "    起動時の一括回収: ${line:-（記録なし = 対象ゼロ）}"
    if grep -q "other_tako_running" <<<"$line"; then
      echo "  未実測: 起動時の一括回収の注入 A/B（他の tako-app が生きている機では Startup の回収は見送り = 器まで届かない）"
    fi
  fi
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
fi

echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
