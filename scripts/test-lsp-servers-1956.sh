#!/bin/bash
# test-lsp-servers-1956.sh — 言語サーバの検出表の第 2 波（#1956）を実のサーバで確かめる（隔離 GUI）
#
# 何を確かめるか（Issue #1956 の受け入れ条件のうち、実機で言えるもの）:
#   ① 実のサーバ 5 つ（texlab / tombi は実の配布元から取る・sourcekit-lsp / dart / yaml-language-server は
#      ログインシェルの PATH）: 誤りを含むファイルを**編集モードで開くだけ**で 起動 → 読み込み済み →
#      診断（誤りの波線）→ ホバー（`tako lsp hover`）。開いてから稼働・最初の診断までの時間と、
#      読み込み後のサーバの常駐メモリ（RSS）を測る。`tako lsp servers` が 9 行を返し、CLI と MCP が同じ
#   ② visual-test `lsp-servers`: 同じ 5 つで、波線の画面とホバーのカードの画面を 1 枚ずつ落とす
#   ③ 境界「取得が失敗する」（配布元に届かない注入 = 閉じたポートのミラー）: GUI 無しの `tako lsp install` は
#      network で失敗して途中の物を残さない。GUI で開くと未導入へ落ちて理由と導入コマンドが読め、
#      編集はそのまま続けられる（壊れない）
#   ④ 境界「PATH の版が古くて probe に失敗する」: PATH の texlab が `--version` で落ちる古い版でも、
#      それを使わず tako の置き場の texlab で起きる（servers の broken に古い方のパスが出る）
#   ⑤ 取れない行の未導入（PATH に dart が無い）: 未導入へ落ちて行の導入コマンド（brew install dart-sdk）
#
# 使い方: bash scripts/test-lsp-servers-1956.sh
#   TAKO_1956_DUMP_DIR=<dir>   画像の置き場（既定は一時 = 終わると消える）
#   TAKO_1956_SKIP_REAL=1      ① / ② / ④ を飛ばす（実の配布元・実のサーバに出ない）
#
# 実の配布元（GitHub Releases・npm・SchemaStore）へ出る。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。落とすのは自分で起こした pid だけ。
# macOS の bash 3.2 で通す（#1499 / #1518）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
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
TMP="$(mktemp -d /tmp/tk1956.XXXXXX)"
DUMP="${TAKO_1956_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1956-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app と tako をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
ms_now() { python3 -c 'import time; print(int(time.time() * 1000))'; }
rss_mb() { ps -o rss= -p "$1" 2>/dev/null | awk '{printf "%.1f", $1/1024}'; }
dir_mb() { [ -d "$1" ] && du -sk "$1" | awk '{printf "%.1f", $1/1024}' || echo "-"; }

# --- 隔離の土台 -------------------------------------------------------------------------
mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/rbin" "$TMP/stale" "$DUMP"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
# ログインシェルの PATH = rbin（node / dart / yaml-language-server だけを置く）+ /etc/paths
set_login_path() { printf 'export PATH="%s:$PATH"\n' "$1" > "$TMP/zdot/.zprofile"; }
set_login_path "$TMP/rbin"
# GUI の PATH は最小にする（呼び出し元の PATH を継ぐと、利用者の texlab 等が見えて「まっさら」にならない）
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
for tool in tmux node; do
  found="$(command -v "$tool" 2>/dev/null)"
  [ -n "$found" ] && ln -s "$found" "$TMP/rbin/$tool"
done
DART_REAL="$(command -v dart 2>/dev/null)"
[ -n "$DART_REAL" ] && ln -s "$DART_REAL" "$TMP/rbin/dart"
# yaml-language-server は利用者の npm で入れる行（tako は取らない）。一時の prefix へ入れて PATH へ置く
YAML_LS=""
if [ "${TAKO_1956_SKIP_REAL:-}" != "1" ] && command -v npm >/dev/null 2>&1; then
  echo "yaml-language-server 1.24.0 を一時の置き場へ入れます（npm。install script は走らせない）…"
  if (cd "$TMP" && npm install --prefix "$TMP/npm" --ignore-scripts --no-audit --no-fund \
      yaml-language-server@1.24.0 >/dev/null 2>&1); then
    YAML_LS="$TMP/npm/node_modules/.bin/yaml-language-server"
    ln -s "$YAML_LS" "$TMP/rbin/yaml-language-server"
  fi
fi

export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

# --- 素材（どれも誤りを 1 つ含む。ホバーは別の行の語に置く）------------------------------
P="$TMP/proj"
mkdir -p "$P/tex" "$P/toml" "$P/swift" "$P/dart" "$P/yaml/.github/workflows"
printf '\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n\\begin{itemize}\n  \\item a\n\\end{enumerate}\n\\end{document}\n' > "$P/tex/main.tex"
printf '[package]\nname = "x"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nserde = \n' > "$P/toml/Cargo.toml"
printf 'let total: Int = "two"\nprint(total)\n' > "$P/swift/main.swift"
printf 'void main() {\n  int total = "two";\n  print(total);\n}\n' > "$P/dart/main.dart"
printf 'name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n     bad: x\n' > "$P/yaml/.github/workflows/ci.yml"
# 件: 検出表の ID・ファイル・ホバーの位置（CLI = 行は 1 始まり / visual-test = 0 始まり。桁はバイト）
CASES="texlab	$P/tex/main.tex	2	14
tombi	$P/toml/Cargo.toml	4	2
sourcekit-lsp	$P/swift/main.swift	2	1
dart	$P/dart/main.dart	3	3
yaml-language-server	$P/yaml/.github/workflows/ci.yml	5	6"

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
    TAKO_DATA_DIR="$data" ${1+"$@"} || exit $?
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
diag_count() { "$TAKO_BIN" lsp diagnostics --pane "$1" --json 2>/dev/null | json 'sum(len(x["diagnostics"]) for x in d["documents"])'; }

mcp() { # 引数のツール名と JSON → 応答本文（隔離インスタンスの受け口を明示する。FR-2.3.2）
  (
    export TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
    {
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1956","version":"0"}}}'
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

# 状態の移り変わりを記録しながら、稼働して読み込みが済むまで待つ
# 引数: サーバの ID・上限（秒）。出力: 「状態=最初に見えた ms」の並び
watch_until_ready() {
  local id="$1" limit="$2" t0 now state loading seen=""
  t0="$(ms_now)"
  while :; do
    now="$(ms_now)"
    [ $((now - t0)) -gt $((limit * 1000)) ] && break
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
    sleep 0.1
  done
  echo "$seen"
}

REAL="$TMP/real"
if [ "${TAKO_1956_SKIP_REAL:-}" != "1" ]; then
  echo
  echo "== ① 実のサーバ 5 つ（texlab / tombi は実の配布元から取る。開くだけで 起動 → 診断 → ホバー） =="
  start_gui "$REAL" "$TMP/gui-real.log"
  T0="$(ms_now)"
  servers_json="$("$TAKO_BIN" lsp servers --json 2>/dev/null)"
  T1="$(ms_now)"
  "$TAKO_BIN" lsp servers --json >/dev/null 2>&1
  T2="$(ms_now)"
  echo "    観測: tako lsp servers の往復 1 回目（ログインシェルを 9 本並べて引く）$((T1 - T0)) ms・2 回目（覚えた答え）$((T2 - T1)) ms"
  check_eq "tako lsp servers は 9 行" "9" "$(echo "$servers_json" | json 'len(d["servers"])')"
  check_eq "第 2 波の 5 行が載る" "['texlab', 'tombi', 'sourcekit-lsp', 'dart', 'yaml-language-server']" \
    "$(echo "$servers_json" | json '[r["id"] for r in d["servers"]][4:]')"
  mcp_ids="$(mcp tako_lsp_server '{"action":"list"}' | json '[r["id"] for r in d["servers"]]')"
  check_eq "MCP（tako_lsp_server の list）も同じ 9 行" \
    "$(echo "$servers_json" | json '[r["id"] for r in d["servers"]]')" "$mcp_ids"
  TAB="$(printf '\t')"
  # 件は fd 3 から読む（中で起こす CLI が標準入力を読んでも件を食わない）
  while IFS="$TAB" read -r id file line col <&3; do
    [ -n "$id" ] || continue
    if [ "$id" = "yaml-language-server" ] && [ -z "$YAML_LS" ]; then
      unmeasured "$id: npm が無いので入れられない"
      continue
    fi
    if [ "$id" = "dart" ] && [ -z "$DART_REAL" ]; then
      unmeasured "$id: この PC に dart が無い"
      continue
    fi
    echo "  -- ${id}（$(basename "$file") を編集モードで開くだけ）"
    pane="$(open_edit "$file")" || { fail "$id: 開けない"; continue; }
    opened="$(ms_now)"
    timeline="$(watch_until_ready "$id" 600)"
    echo "    状態の移り変わり（開いてからの ms）:$timeline"
    case "$timeline" in
      *ready=*) pass "$id: 操作なしで起動 → 読み込み済み" ;;
      *) fail "$id: 稼働に届かない（$(server_field "$id" '.get("reason")')）" ;;
    esac
    echo "    出どころ: $(server_field "$id" '["source"]')（$(server_field "$id" '["program_path"]' | sed "s#$TMP#<tmp>#")）"
    n=0
    for _ in $(seq 1 600); do
      n="$(diag_count "$pane")"
      [ "${n:-0}" -gt 0 ] 2>/dev/null && break
      sleep 0.1
    done
    first_diag=$(( $(ms_now) - opened ))
    if [ "${n:-0}" -gt 0 ] 2>/dev/null; then
      pass "$id: 誤りの診断が出る（$n 件・開いてから約 ${first_diag} ms まで）"
      "$TAKO_BIN" lsp diagnostics --pane "$pane" 2>/dev/null | sed -n 1,2p | sed "s#$TMP#<tmp>#g; s/^/      /"
    else
      fail "$id: 診断が出ない"
    fi
    hv="$("$TAKO_BIN" lsp hover --pane "$pane" --line "$line" --column "$col" --json 2>/dev/null)"
    status="$(echo "$hv" | json 'd["status"]')"
    if [ "$status" = "found" ]; then
      pass "$id: ホバー（$line 行 $col 桁）: $(echo "$hv" | json 'd["contents"].strip().splitlines()[0][:90]')"
    else
      fail "$id: ホバーが出ない（status=$status）"
    fi
    pid="$(server_field "$id" '["pid"]')"
    sleep 3
    echo "    観測: 常駐メモリ（サーバの RSS・読み込み後 3 秒）$(rss_mb "$pid") MB"
  done 3<<EOF
$CASES
EOF
  echo "    観測: 取得の記録（persist.log）"
  grep -h "LSP 取得: " "$REAL"/persist.log 2>/dev/null | sed 's/^.*LSP 取得: /      /'
  for d in texlab tombi; do
    echo "    観測: ディスク $d = $(dir_mb "$REAL/lsp-servers/$d") MB"
  done
  for id in texlab tombi; do
    check_eq "$id: 出どころは tako の置き場（managed）" "managed" "$(server_field "$id" '["source"]')"
  done
  stop_gui

  echo
  echo "== ② visual-test lsp-servers（同じ 5 つ。波線の画面とホバーのカードの画面） =="
  VCASES="$(echo "$CASES" | while IFS="$TAB" read -r id file line col; do
    [ -n "$id" ] || continue
    [ "$id" = "yaml-language-server" ] && [ -z "$YAML_LS" ] && continue
    [ "$id" = "dart" ] && [ -z "$DART_REAL" ] && continue
    printf '%s\t%s\t%s\n' "$file" "$((line - 1))" "$col"
  done)"
  launch_isolated_gui "$TMP/vt.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DATA_DIR="$REAL" \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=lsp-servers TAKO_VISUAL_DUMP_DIR="$DUMP" \
    TAKO_1956_CASES="$VCASES" || exit $?
  vpid="$ISOLATED_GUI_PID"
  for _ in $(seq 1 12000); do
    kill -0 "$vpid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$vpid"
  ISOLATED_GUI_PID=""
  grep "TAKO_VISUAL_PIXEL: lsp-servers id=\|TAKO_VISUAL_DUMP_FILE" "$TMP/vt.log" | sed "s#$TMP#<tmp>#g; s/^/    /"
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt.log"; then
    pass "visual-test lsp-servers: 全件で波線とホバーのカードが出る（TAKO_VISUAL_TEST_OK）"
  else
    fail "visual-test lsp-servers: $(grep -h 'FAILED' "$TMP/vt.log" | head -1)"
  fi

  echo
  echo "== ④ 境界: PATH の texlab が古い版（--version で落ちる）でも使わず、tako の置き場の texlab で起きる =="
  printf '#!/bin/sh\necho "error: unexpected argument '"'"'--version'"'"' found" >&2\nexit 2\n' > "$TMP/stale/texlab"
  chmod +x "$TMP/stale/texlab"
  set_login_path "$TMP/stale:$TMP/rbin"
  start_gui "$REAL" "$TMP/gui-stale.log"
  check_eq "servers の broken に PATH の古い版のパス" "$TMP/stale/texlab" \
    "$("$TAKO_BIN" lsp servers --json 2>/dev/null | json '[r for r in d["servers"] if r["id"] == "texlab"][0]["broken"]["path"]')"
  echo "    broken.detail: $("$TAKO_BIN" lsp servers --json 2>/dev/null | json '[r for r in d["servers"] if r["id"] == "texlab"][0]["broken"]["detail"]')"
  pane="$(open_edit "$P/tex/main.tex")"
  watch_until_ready texlab 120 >/dev/null
  check_eq "起きたのは tako の置き場の texlab（source=managed）" "managed" "$(server_field texlab '["source"]')"
  check_eq "稼働している" "running" "$(server_field texlab '["state"]')"
  stop_gui
  set_login_path "$TMP/rbin"
else
  unmeasured "① / ② / ④ は TAKO_1956_SKIP_REAL=1 で飛ばした（実の配布元・実のサーバに出ていない）"
fi

echo
echo "== ③ 境界: 取得が失敗する（配布元に届かない = 閉じたポートのミラー） =="
CLOSED="http://127.0.0.1:9"
mkdir -p "$TMP/nodisc"
cli_local() { env -u TAKO_SOCKET -u TAKO_TOKEN TAKO_DISCOVERY_DIR="$TMP/nodisc" "$TAKO_BIN" lsp install "$@" --json 2>/dev/null; }
for id in texlab tombi; do
  out="$(TAKO_DATA_DIR="$TMP/off-$id" TAKO_LSP_FETCH_BASE="$CLOSED" cli_local --name "$id")"
  check_eq "GUI 無しの install --name $id は network で失敗" "('failed', 'network')" \
    "$(echo "$out" | json '(d["servers"][0]["status"], d["servers"][0]["error_kind"])')"
  echo "    理由: $(echo "$out" | json 'd["servers"][0]["reason"]')"
  check_eq "  途中の物を置かない（$id の段が空）" "0" \
    "$(find "$TMP/off-$id/lsp-servers/$id" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')"
done
start_gui "$TMP/off-gui" "$TMP/gui-off.log" TAKO_LSP_FETCH_BASE="$CLOSED"
pane="$(open_edit "$P/tex/main.tex")"
for _ in $(seq 1 300); do
  [ "$(server_field texlab '["state"]')" = "not_installed" ] && [ -n "$(server_field texlab '.get("fetch_error") or ""')" ] && break
  sleep 0.1
done
check_eq "GUI で開くと未導入へ落ちる（fetch_error=network）" "('not_installed', 'network')" \
  "$("$TAKO_BIN" lsp status --name texlab --json 2>/dev/null | json '(d["servers"][0]["state"], d["servers"][0]["fetch_error"])')"
echo "    理由: $(server_field texlab '["reason"]')"
echo "    次の一手: $(server_field texlab '["next_step"]')"
check_eq "導入コマンドは行のもの" "brew install texlab" "$(server_field texlab '["install_command"]')"
"$TAKO_BIN" edit replace-range 1:0 1:0 "% x
" --pane "$pane" >/dev/null 2>&1
check_eq "編集はそのまま続けられる（editing・dirty）" "(True, True)" \
  "$("$TAKO_BIN" edit status --pane "$pane" 2>/dev/null | json '(d["editing"], d["dirty"])')"

echo
echo "== ⑤ 取れない行の未導入（PATH に dart が無い） =="
# ③ の GUI のまま。この GUI ではまだ誰も dart を引いていないので、外してから開けば未導入から始まる
rm -f "$TMP/rbin/dart"
pane="$(open_edit "$P/dart/main.dart")"
for _ in $(seq 1 300); do
  [ "$(server_field dart '["state"]')" = "not_installed" ] && break
  sleep 0.1
done
check_eq "未導入へ落ちる" "not_installed" "$(server_field dart '["state"]')"
check_eq "導入コマンドは行のもの" "brew install dart-sdk" "$(server_field dart '["install_command"]')"
check_eq "取れない行（fetch.available=False）" "False" \
  "$("$TAKO_BIN" lsp servers --json 2>/dev/null | json '[r for r in d["servers"] if r["id"] == "dart"][0]["fetch"]["available"]')"
echo "    理由: $(server_field dart '["reason"]')"
stop_gui

echo
echo "結果: OK $PASS / NG $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ]
