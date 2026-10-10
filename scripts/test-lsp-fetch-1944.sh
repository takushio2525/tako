#!/bin/bash
# test-lsp-fetch-1944.sh — 言語サーバの自動取得とエディタの状態表示（#1944）の実経路テスト（隔離 GUI）
#
# 何を確かめるか（Issue #1944 の受け入れ条件のうち、GUI と CLI / MCP を通して言えるもの）:
#   ① 実の配布元から 1 回（まっさら = 一時 HOME・node も pyright も無い・PATH の rust-analyzer は
#      rustup の代理のように即座に落ちる偽物）: .py / .ts / .rs を**編集モードで開くだけ**で取得 → 起動 →
#      診断・補完。取得の時間・ダウンロード量・ディスク・起動時間・常駐メモリを測る。取得中も UI が
#      固まらない（IPC の `tako list` の往復の最大）
#   ② グローバル優先: PATH に pyright-langserver を置いて立て直すと、置き場にあっても PATH の物を使う
#   ③ visual-test `lsp-fetch`（ローカルのミラー）: 失敗の帯 → 実マウスで「もう一度取得」→ 取得中 →
#      動作中・診断・補完。新旧（`TAKO_1944_LEGACY=1`）で画像の書き出し先を分け、旧は ① で名指しの FAILED
#   ④ GUI 無しの `tako lsp install`（CLI のプロセスで取る）: 成功・オフライン・ハッシュ不一致・
#      ディスクの空きが無い（hdiutil の小さな面）。途中の段を残さない
#   ⑤ CLI と MCP の字面一致（install / status）
#   ⑥ 2 つのペインが同時に同じサーバを求めても 1 回だけ取る（ミラーの記録で数える）/ 取っている途中で
#      閉じても起こさない（取得物は残る）
#
# 使い方: bash scripts/test-lsp-fetch-1944.sh
#   TAKO_1944_MIRROR=<dir>   ミラーの中身（`<host>/<path>` の並び）。無ければ配布元から 1 回ずつ取って作る
#   TAKO_1944_DUMP_DIR=<dir> 画像の置き場（new/ と old/ に分ける。既定は一時 = 終わると消える）
#   TAKO_1944_SKIP_REAL=1    ① と ② を飛ばす（実の配布元へ出ない）
#   TAKO_1944_REAL_SERVERS="pyright"  ① で実の配布元から取るサーバを絞る（既定は 3 つ）
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。macOS の bash 3.2 で通す（#1499 / #1518）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

REAL_HOME="$HOME"
# 受け口のソケットのパス長に余裕を持たせるため短い置き場にする
TMP="$(mktemp -d /tmp/tk1944.XXXXXX)"
DUMP="${TAKO_1944_DUMP_DIR:-$TMP/dump}"
APP_PID=""
MIRROR_PID=""
VOLUME=""
TMUX_SOCKET="tako-1944-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  [ -n "$MIRROR_PID" ] && kill "$MIRROR_PID" 2>/dev/null
  [ -n "$VOLUME" ] && hdiutil detach -quiet "$VOLUME" >/dev/null 2>&1
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app と tako をビルドします…"
(cd "$REPO_ROOT" && cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
ms_now() { python3 -c 'import time; print(int(time.time() * 1000))'; }

# --- ミラー（配布元の取得物を `<host>/<path>` に並べた置き場）-------------------------------
MIRROR="${TAKO_1944_MIRROR:-$TMP/mirror}"
ASSETS="registry.npmjs.org/pyright/-/pyright-1.1.414.tgz
registry.npmjs.org/typescript-language-server/-/typescript-language-server-5.1.3.tgz
registry.npmjs.org/typescript/-/typescript-5.9.3.tgz
nodejs.org/dist/v24.21.0/node-v24.21.0-darwin-arm64.tar.gz
github.com/rust-lang/rust-analyzer/releases/download/2026-09-21/rust-analyzer-aarch64-apple-darwin.gz"
if [ "$(uname -m)" != "arm64" ]; then
  echo "このスクリプトのミラーは Apple Silicon の取得物だけを並べる（$(uname -m) は未対応）"
  exit 1
fi
for rel in $ASSETS; do
  [ -f "$MIRROR/$rel" ] && continue
  mkdir -p "$MIRROR/$(dirname "$rel")"
  echo "  ミラーへ取る（配布元から 1 回）: $(basename "$rel")"
  curl -sSfL -o "$MIRROR/$rel" "https://$rel" || { echo "取れない: $rel"; exit 1; }
done

# ミラーの配り手（`<gate>` があれば 503、`<slow>` があれば約 8 MB/s。要求は `<log>` へ 1 行ずつ）
cat > "$TMP/mirror.py" <<'PY'
import http.server, os, sys, time
root, gate, slow, log, port_file = sys.argv[1:6]
root = os.path.realpath(root)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as f:
            f.write(self.path + "\n")
        if os.path.exists(gate):
            self.send_response(503); self.end_headers(); return
        path = os.path.realpath(os.path.join(root, self.path.lstrip("/")))
        if not path.startswith(root + os.sep) or not os.path.isfile(path):
            self.send_response(404); self.end_headers(); return
        self.send_response(200)
        self.send_header("Content-Length", str(os.path.getsize(path)))
        self.end_headers()
        with open(path, "rb") as f:
            while True:
                chunk = f.read(256 * 1024)
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return
                if os.path.exists(slow):
                    time.sleep(0.03)
    def log_message(self, *args):
        pass
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
GATE="$TMP/mirror.gate"
SLOW="$TMP/mirror.slow"
MLOG="$TMP/mirror.log"
python3 -I "$TMP/mirror.py" "$MIRROR" "$GATE" "$SLOW" "$MLOG" "$TMP/mirror.port" >/dev/null 2>&1 &
MIRROR_PID=$!
for _ in $(seq 1 100); do [ -s "$TMP/mirror.port" ] && break; sleep 0.05; done
MIRROR_BASE="http://127.0.0.1:$(cat "$TMP/mirror.port")"
mirror_hits() { grep -c "$1" "$MLOG" 2>/dev/null; }

# --- 隔離の土台 -------------------------------------------------------------------------
mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/rbin" "$TMP/gbin" "$DUMP/new" "$DUMP/old"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
# ログインシェルの PATH: rbin（cargo / rustc は本物へ・rust-analyzer は rustup の代理のように即座に落ちる偽物）
printf 'export PATH="%s:$PATH"\n' "$TMP/rbin" > "$TMP/zdot/.zprofile"
for tool in cargo rustc; do
  [ -x "$REAL_HOME/.cargo/bin/$tool" ] && ln -s "$REAL_HOME/.cargo/bin/$tool" "$TMP/rbin/$tool"
done
printf '#!/bin/sh\necho "error: Unknown binary '"'"'rust-analyzer'"'"' in official toolchain '"'"'stable'"'"'" >&2\nexit 1\n' > "$TMP/rbin/rust-analyzer"
chmod +x "$TMP/rbin/rust-analyzer"
# GUI の PATH は最小にする（呼び出し元の PATH を継ぐと、利用者の node が見えて「まっさら」にならない。
# ログインシェルは /etc/paths の段を足すだけなので、node / pyright / rust-analyzer は見えない）
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
for tool in tmux; do
  found="$(command -v "$tool" 2>/dev/null)"
  [ -n "$found" ] && ln -s "$found" "$TMP/rbin/$tool"
done
RUSTUP_HOME_REAL="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
CARGO_HOME_REAL="${CARGO_HOME:-$REAL_HOME/.cargo}"

export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

# 素材のプロジェクト（型の誤り・無い属性で診断が出る。9 行目の末尾で補完が出る）
mkdir -p "$TMP/proj/py" "$TMP/proj/ts" "$TMP/proj/rs/src" "$TMP/proj/py2/a" "$TMP/proj/py2/b"
cat > "$TMP/proj/py/main.py" <<'EOF'
import os


def add(a: int, b: int) -> int:
    return a + b


total: int = add(1, "two")
print(os.pa)
EOF
printf '[project]\nname = "p"\n' > "$TMP/proj/py/pyproject.toml"
cp "$TMP/proj/py/main.py" "$TMP/proj/py2/a/main.py"
cp "$TMP/proj/py/main.py" "$TMP/proj/py2/b/main.py"
cp "$TMP/proj/py/pyproject.toml" "$TMP/proj/py2/a/"
cp "$TMP/proj/py/pyproject.toml" "$TMP/proj/py2/b/"
cat > "$TMP/proj/ts/main.ts" <<'EOF'
const total: number = "two";
console.lo
EOF
printf '{ "compilerOptions": { "strict": true } }\n' > "$TMP/proj/ts/tsconfig.json"
printf '[package]\nname = "p"\nversion = "0.1.0"\nedition = "2021"\n' > "$TMP/proj/rs/Cargo.toml"
printf 'fn main() {\n    let total: i32 = "two";\n    println!("{total}");\n}\n' > "$TMP/proj/rs/src/main.rs"

# GUI を立てる（データの置き場と、足す env を渡す）
start_gui() {
  local data="$1" log="$2"
  shift 2
  mkdir -p "$data"
  export TAKO_DATA_DIR="$data"
  launch_isolated_gui "$log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_DATA_DIR="$data" RUSTUP_HOME="$RUSTUP_HOME_REAL" CARGO_HOME="$CARGO_HOME_REAL" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || { echo "GUI が立たない"; exit 1; }
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$ROOT" ] || { echo "ペインが取れない"; exit 1; }
  SOCK="$data/tako.sock"
  [ -f "$data/tako.sock.path" ] && SOCK="$(cat "$data/tako.sock.path")"
}
stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
open_edit() { # ファイル → ペイン ID（編集モードへ入れる）
  local pane
  pane="$("$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
  [ -n "$pane" ] || return 1
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  echo "$pane"
}
# サーバの状態（`tako lsp status --name`。器が複数なら最初の 1 つ）
server_field() { "$TAKO_BIN" lsp status --name "$1" --json 2>/dev/null | json "d['servers'][0]$2"; }

mcp() { # 引数のツール名と JSON → 応答本文（隔離インスタンスの受け口を明示する。FR-2.3.2）
  (
    export TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
    {
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1944","version":"0"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
      printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' "$1" "$2"
      sleep 3
    } | "$TAKO_BIN" mcp serve 2>/dev/null | python3 -c 'import json,sys
for l in sys.stdin:
    try:
        m = json.loads(l)
    except Exception:
        continue
    if m.get("id") == 2:
        r = m.get("result")
        print(r["content"][0]["text"] if r else json.dumps(m.get("error"), ensure_ascii=False)); break'
  )
}

# 状態の移り変わりを記録しながら、稼働して読み込みが済むまで待つ（取得中の IPC の往復も測る）
# 引数: サーバの ID・上限（秒）。出力: 「状態=最初に見えた ms」の並びと IPC の往復の最大
watch_until_ready() {
  local id="$1" limit="$2" t0 now state loading seen="" max_rtt=0 a b rtt
  t0="$(ms_now)"
  while :; do
    now="$(ms_now)"
    [ $((now - t0)) -gt $((limit * 1000)) ] && break
    a="$(ms_now)"
    "$TAKO_BIN" list >/dev/null 2>&1
    b="$(ms_now)"
    rtt=$((b - a))
    [ "$rtt" -gt "$max_rtt" ] && max_rtt="$rtt"
    state="$(server_field "$id" '["state"]')"
    loading="$(server_field "$id" '["loading"]')"
    case " $seen " in
      *" $state="*) : ;;
      *) seen="$seen $state=$((now - t0))" ;;
    esac
    if [ "$state" = "running" ] && [ "$loading" = "False" ]; then
      seen="$seen ready=$((now - t0))"
      break
    fi
    sleep 0.2
  done
  echo "$seen max_ipc_rtt=${max_rtt}ms"
}
rss_mb() { ps -o rss= -p "$1" 2>/dev/null | awk '{printf "%.1f", $1/1024}'; }
dir_mb() { [ -d "$1" ] && du -sk "$1" | awk '{printf "%.1f", $1/1024}' || echo "-"; }
# persist.log の「LSP 取得: <取得物> <秒> 秒 <バイト> バイト」
fetch_log() { grep -h "LSP 取得: " "$1"/persist.log 2>/dev/null | sed 's/^.*LSP 取得: /    /'; }

if [ "${TAKO_1944_SKIP_REAL:-}" != "1" ]; then
  echo
  echo "== ① 実の配布元から 1 回（まっさら: 一時 HOME・node / pyright 無し・PATH の rust-analyzer は落ちる偽物） =="
  REAL="$TMP/real"
  start_gui "$REAL" "$TMP/gui-real.log"
  # 基準: 取得していないときの IPC の往復（CLI の起動を含む）
  base_max=0
  for _ in $(seq 1 10); do
    a="$(ms_now)"; "$TAKO_BIN" list >/dev/null 2>&1; b="$(ms_now)"
    [ $((b - a)) -gt "$base_max" ] && base_max=$((b - a))
  done
  echo "    観測: 取得していないときの IPC の往復の最大 ${base_max}ms（10 回）"
  for spec in "pyright:py/main.py:9:11" "typescript-language-server:ts/main.ts:2:10" "rust-analyzer:rs/src/main.rs:0:0"; do
    id="${spec%%:*}"; rest="${spec#*:}"; file="${rest%%:*}"; rest="${rest#*:}"; line="${rest%%:*}"; col="${rest#*:}"
    # 実の配布元へ出る回数を絞る口（例: TAKO_1944_REAL_SERVERS=pyright）
    case " ${TAKO_1944_REAL_SERVERS:-pyright typescript-language-server rust-analyzer} " in
      *" $id "*) : ;;
      *) continue ;;
    esac
    echo "  -- ${id}（${file} を編集モードで開くだけ）"
    pane="$(open_edit "$TMP/proj/$file")" || { fail "$id: 開けない"; continue; }
    timeline="$(watch_until_ready "$id" 600)"
    echo "    状態の移り変わり（開いてからの ms）:$timeline"
    case "$timeline" in
      *ready=*) pass "$id: 操作なしで取得 → 起動 → 読み込み済み" ;;
      *) fail "$id: 稼働に届かない（$(server_field "$id" '.get("reason")')）" ;;
    esac
    case "$timeline" in *fetching=*) pass "$id: 取得中を経た（グローバルに無いので取った）" ;; *) fail "$id: 取得中が見えない" ;; esac
    check_eq "$id: 出どころは tako の置き場（managed）" "managed" "$(server_field "$id" '["source"]')"
    pid="$(server_field "$id" '["pid"]')"
    if [ "$id" != "rust-analyzer" ]; then
      n=0
      for _ in $(seq 1 600); do
        n="$("$TAKO_BIN" lsp diagnostics --pane "$pane" --json 2>/dev/null | json 'sum(len(x["diagnostics"]) for x in d["documents"])')"
        [ "${n:-0}" -gt 0 ] 2>/dev/null && break
        sleep 0.2
      done
      [ "${n:-0}" -gt 0 ] 2>/dev/null && pass "$id: 診断が出る（$n 件）" || fail "$id: 診断が出ない"
      items="$("$TAKO_BIN" lsp completion --pane "$pane" --line "$line" --column "$col" --json 2>/dev/null | json 'd.get("total", 0)')"
      [ "${items:-0}" -gt 0 ] 2>/dev/null && pass "$id: 補完が出る（$items 件）" || fail "$id: 補完が出ない（${items:-}）"
    else
      check_eq "$id: PATH の偽物（即座に落ちる）は使わない（servers の broken）" "True" \
        "$("$TAKO_BIN" lsp servers --json 2>/dev/null | json '[("broken" in r) for r in d["servers"] if r["id"] == "rust-analyzer"][0]')"
    fi
    sleep 3
    echo "    観測: 常駐メモリ（サーバの RSS・読み込み後 3 秒）$(rss_mb "$pid") MB"
  done
  echo "    観測: 取得の記録（persist.log）"
  fetch_log "$REAL"
  for d in node pyright typescript-language-server rust-analyzer; do
    echo "    観測: ディスク $d = $(dir_mb "$REAL/lsp-servers/$d") MB"
  done
  echo "    観測: GUI の RSS $(rss_mb "$APP_PID") MB"
  stop_gui

  echo
  echo "== ② グローバル優先（PATH に pyright-langserver を置いて立て直す。置き場にも在る） =="
  NODE_BIN="$REAL/lsp-servers/node/24.21.0/node"
  ENTRY="$(ls "$REAL"/lsp-servers/pyright/*/node_modules/pyright/langserver.index.js 2>/dev/null | head -1)"
  printf '#!/bin/sh\nexec "%s" "%s" "$@"\n' "$NODE_BIN" "$ENTRY" > "$TMP/gbin/pyright-langserver"
  chmod +x "$TMP/gbin/pyright-langserver"
  printf 'export PATH="%s:%s:$PATH"\n' "$TMP/gbin" "$TMP/rbin" > "$TMP/zdot/.zprofile"
  start_gui "$REAL" "$TMP/gui-global.log"
  pane="$(open_edit "$TMP/proj/py/main.py")"
  watch_until_ready pyright 120 >/dev/null
  check_eq "PATH の物を使う（source=path）" "path" "$(server_field pyright '["source"]')"
  check_eq "そのパスは PATH に置いた物" "$TMP/gbin/pyright-langserver" "$(server_field pyright '["program_path"]')"
  check_eq "install もグローバルがあれば取らない（status=global）" "global" \
    "$("$TAKO_BIN" lsp install --name pyright --json 2>/dev/null | json 'd["servers"][0]["status"]')"
  stop_gui
  printf 'export PATH="%s:$PATH"\n' "$TMP/rbin" > "$TMP/zdot/.zprofile"
else
  unmeasured "① / ② は TAKO_1944_SKIP_REAL=1 で飛ばした（実の配布元へ出ていない）"
fi

echo
echo "== ③ visual-test lsp-fetch（ミラー。失敗の帯 → 実マウスで「もう一度取得」→ 取得中 → 動作中・診断・補完） =="
run_section() { # データの置き場・ログ・画像の置き場・足す env
  local data="$1" log="$2" dump="$3" pid
  shift 3
  mkdir -p "$data"
  export TAKO_DATA_DIR="$data"
  : > "$GATE"
  : > "$SLOW"
  launch_isolated_gui "$log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DATA_DIR="$data" \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=lsp-fetch TAKO_VISUAL_DUMP_DIR="$dump" \
    TAKO_LSP_AUTO_FETCH=1 TAKO_LSP_FETCH_BASE="$MIRROR_BASE" \
    TAKO_1944_GATE="$GATE" TAKO_1944_SLOW="$SLOW" ${1+"$@"} || exit $?
  pid="$ISOLATED_GUI_PID"
  for _ in $(seq 1 3600); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  rm -f "$SLOW"
}
run_section "$TMP/vt-new" "$TMP/vt-new.log" "$DUMP/new"
grep "TAKO_VISUAL_PIXEL: lsp-fetch\|TAKO_VISUAL_DUMP_FILE" "$TMP/vt-new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-new.log"; then
  pass "新: 全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "新: $(grep -h 'FAILED' "$TMP/vt-new.log" | head -1)"
fi
run_section "$TMP/vt-old" "$TMP/vt-old.log" "$DUMP/old" TAKO_1944_LEGACY=1
grep "TAKO_VISUAL_PIXEL: lsp-fetch\|TAKO_VISUAL_DUMP_FILE" "$TMP/vt-old.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test lsp-fetch: ①" "$TMP/vt-old.log"; then
  pass "旧（TAKO_1944_LEGACY=1）: ① で名指しの FAILED（帯が出ない = 黙って出ない）"
else
  fail "旧で ① が落ちない: $(grep -h 'FAILED\|TAKO_VISUAL_TEST_OK' "$TMP/vt-old.log" | head -1)"
fi

echo
echo "== ④ GUI 無しの tako lsp install（CLI のプロセスで取る） =="
# GUI へ届かない状態（発見の置き場を空にする）
cli_local() { env -u TAKO_SOCKET -u TAKO_TOKEN TAKO_DISCOVERY_DIR="$TMP/nodisc" "$TAKO_BIN" lsp install "$@" --json 2>/dev/null; }
mkdir -p "$TMP/nodisc"
rm -f "$GATE" "$SLOW"
L1="$TMP/local-ok"
T0="$(ms_now)"
out="$(TAKO_DATA_DIR="$L1" TAKO_LSP_FETCH_BASE="$MIRROR_BASE" cli_local --name pyright)"
echo "    観測: ミラーからの取得（node + pyright）$(($(ms_now) - T0)) ms"
check_eq "成功（status=installed・local=True）" "('installed', True)" "$(echo "$out" | json '(d["servers"][0]["status"], d.get("local"))')"
check_eq "もう一度は取らない（present）" "present" \
  "$(TAKO_DATA_DIR="$L1" TAKO_LSP_FETCH_BASE="$MIRROR_BASE" cli_local --name pyright | json 'd["servers"][0]["status"]')"
out="$(TAKO_DATA_DIR="$TMP/local-off" TAKO_LSP_FETCH_BASE="http://127.0.0.1:9" cli_local --name pyright)"
check_eq "オフライン（閉じたポート）は network" "('failed', 'network')" "$(echo "$out" | json '(d["servers"][0]["status"], d["servers"][0]["error_kind"])')"
echo "    理由: $(echo "$out" | json 'd["servers"][0]["reason"]')"
echo "    次の一手: $(echo "$out" | json 'd["servers"][0]["next_step"]')"
# ハッシュ不一致（ミラーの取得物を 1 バイト壊す）
BAD="$TMP/mirror-bad"
RA="github.com/rust-lang/rust-analyzer/releases/download/2026-09-21/rust-analyzer-aarch64-apple-darwin.gz"
mkdir -p "$BAD/$(dirname "$RA")"
python3 - "$MIRROR/$RA" "$BAD/$RA" <<'PY'
import sys
b = bytearray(open(sys.argv[1], "rb").read())
b[-1] ^= 0xFF
open(sys.argv[2], "wb").write(bytes(b))
PY
python3 -I "$TMP/mirror.py" "$BAD" "$TMP/nogate" "$TMP/noslow" "$TMP/bad.log" "$TMP/bad.port" >/dev/null 2>&1 &
BAD_PID=$!
for _ in $(seq 1 100); do [ -s "$TMP/bad.port" ] && break; sleep 0.05; done
out="$(TAKO_DATA_DIR="$TMP/local-bad" TAKO_LSP_FETCH_BASE="http://127.0.0.1:$(cat "$TMP/bad.port")" cli_local --name rust-analyzer)"
kill "$BAD_PID" 2>/dev/null
check_eq "ハッシュ不一致は digest" "('failed', 'digest')" "$(echo "$out" | json '(d["servers"][0]["status"], d["servers"][0]["error_kind"])')"
echo "    理由: $(echo "$out" | json 'd["servers"][0]["reason"]')"
check_eq "壊れた取得物を置かない（版の段も途中の段も無い）" "0" \
  "$(find "$TMP/local-bad/lsp-servers" -mindepth 2 -maxdepth 2 2>/dev/null | wc -l | tr -d ' ')"
# ディスクの空きが無い（20 MB の面に node 117 MB を取る）
if hdiutil create -quiet -size 20m -fs HFS+ -volname tk1944 "$TMP/small.dmg" >/dev/null 2>&1 \
  && hdiutil attach -quiet -nobrowse -mountpoint "$TMP/vol" "$TMP/small.dmg" >/dev/null 2>&1; then
  VOLUME="$TMP/vol"
  out="$(TAKO_DATA_DIR="$TMP/vol/d" TAKO_LSP_FETCH_BASE="$MIRROR_BASE" cli_local --name pyright)"
  check_eq "ディスクの空きが無いと io" "('failed', 'io')" "$(echo "$out" | json '(d["servers"][0]["status"], d["servers"][0]["error_kind"])')"
  echo "    理由: $(echo "$out" | json 'd["servers"][0]["reason"]')"
  check_eq "途中の段を残さない（空きが戻る）" "0" \
    "$(find "$TMP/vol/d/lsp-servers" -name '.partial-*' 2>/dev/null | wc -l | tr -d ' ')"
  hdiutil detach -quiet "$VOLUME" >/dev/null 2>&1 && VOLUME=""
else
  unmeasured "ディスクの空きが無い: hdiutil で小さな面を作れない"
fi

echo
echo "== ⑤ ⑥ CLI と MCP / 同時の要求 / 取っている途中で閉じる（ミラー・遅い配り） =="
: > "$MLOG"
: > "$SLOW"
start_gui "$TMP/conc" "$TMP/gui-conc.log" TAKO_LSP_FETCH_BASE="$MIRROR_BASE"
A="$(open_edit "$TMP/proj/py2/a/main.py")"
B="$(open_edit "$TMP/proj/py2/b/main.py")"
for _ in $(seq 1 100); do
  [ "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'len(d["servers"])')" = "2" ] && break
  sleep 0.1
done
timeline="$(watch_until_ready pyright 300)"
echo "    状態の移り変わり（2 つの器・遅い配り）:$timeline"
check_eq "2 つの器が同時に求めても node は 1 回だけ取る" "1" "$(mirror_hits node-v24.21.0)"
check_eq "pyright も 1 回だけ取る" "1" "$(mirror_hits pyright-1.1.414.tgz)"
for _ in $(seq 1 300); do
  [ "$("$TAKO_BIN" lsp status --name pyright --json 2>/dev/null | json 'all(s["state"] == "running" for s in d["servers"])')" = "True" ] && break
  sleep 0.2
done
check_eq "2 つとも稼働" "True" "$("$TAKO_BIN" lsp status --name pyright --json | json 'all(s["state"] == "running" for s in d["servers"])')"
# ⑤ CLI と MCP の字面一致
"$TAKO_BIN" lsp install --name pyright --json > "$TMP/install-cli.json" 2>/dev/null
mcp tako_lsp_server '{"action":"install","name":"pyright"}' > "$TMP/install-mcp.json"
check_eq "install: CLI と MCP が同じ（経過の ms 以外）" "True" "$(python3 - "$TMP/install-cli.json" "$TMP/install-mcp.json" <<'PY'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
for d in (a, b):
    for s in d.get("servers", []):
        s.pop("elapsed_ms", None)
print(a == b and a["servers"][0]["status"] == "present")
PY
)"
"$TAKO_BIN" lsp status --name pyright --json > "$TMP/status-cli.json" 2>/dev/null
mcp tako_lsp_server '{"action":"status","name":"pyright"}' > "$TMP/status-mcp.json"
check_eq "status: CLI と MCP の source / fetch / state が同じ" "True" "$(python3 - "$TMP/status-cli.json" "$TMP/status-mcp.json" <<'PY'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
pick = lambda d: [(s["state"], s["source"], s["fetch"]) for s in d["servers"]] + [d["fetch"]]
print(pick(a) == pick(b))
PY
)"
# ⑥ 取っている途中で閉じる（.ts = まだ取っていない typescript-language-server）
C="$(open_edit "$TMP/proj/ts/main.ts")"
for _ in $(seq 1 300); do
  [ "$(server_field typescript-language-server '["state"]')" = "fetching" ] && break
  sleep 0.05
done
check_eq "取得中になる" "fetching" "$(server_field typescript-language-server '["state"]')"
"$TAKO_BIN" close --pane "$C" --force >/dev/null 2>&1 || "$TAKO_BIN" close --pane "$C" >/dev/null 2>&1
for _ in $(seq 1 600); do
  [ "$(server_field typescript-language-server '["state"]')" = "not_started" ] && break
  sleep 0.2
done
check_eq "閉じたら取れても起こさない（not_started・pid 無し）" "('not_started', None)" \
  "$("$TAKO_BIN" lsp status --name typescript-language-server --json | json '(d["servers"][0]["state"], d["servers"][0]["pid"])')"
check_eq "取得物は置き場に残る（servers の source=managed）" "managed" \
  "$("$TAKO_BIN" lsp servers --json | json '[r.get("source") for r in d["servers"] if r["id"] == "typescript-language-server"][0]')"
rm -f "$SLOW"
stop_gui

echo
echo "画像: ${DUMP}（new/ = 新・old/ = 旧）"
echo "結果: PASS=$PASS FAIL=$FAIL 未実測=$UNMEASURED"
[ "$FAIL" -eq 0 ]
