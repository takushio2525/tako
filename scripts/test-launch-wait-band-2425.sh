#!/usr/bin/env bash
# test-launch-wait-band-2425.sh — 起動待ち（#2413）のペインの帯（#2425）の実経路テスト
#
# 隔離した data / HOME / tmux で**実 tako-app**（visual-test 入り）を立て、メモリ圧は**注入**
# （`TAKO_2413_INJECT` のファイル）で与える = 負荷は生成しない。claude は偽物（起動した時刻・pid・引数を
# 記録して眠るだけ）なので本物の agent は立たない。
#
#   [visual] visual-test 節 `launch-wait-band`（新しい形 / A/B `TAKO_2425_LEGACY=1`）
#     ① 待っている 3 枚に帯（理由・順番 N 本中 M 番目・今すぐ起こす）が実ピクセルで描かれ、
#        `tako list` の `launch_wait_band` に同じ中身
#     ② 待っている間の打鍵・貼り付け・IME の確定がペインへ届かず、受け付けなかった回数が帯に出る
#     ③ 帯の「今すぐ起こす」を実マウスで押すと起動コマンドが 1 回だけ（入力と混ざらずに）打たれ、帯が消え、
#        以後の打鍵は届く
#     ④ 待っているペインを閉じると起こさない ⑤ normal へ戻すと残りが自然に起き、帯が消える
#     A/B: 帯が無く、待っている間の打鍵がシェルへ届いて起動コマンドと同じ行に混ざる
#   [cli] 本物の spawn（`tako orchestrator spawn`）で起動待ちにしたペインの帯を CLI `tako list` と
#     MCP `tako_list_panes` で読む・`tako queue start` で起こすと帯が消えて偽 claude が 1 回起きる・
#     normal へ戻すと残りも起きて帯が消える・A/B では起動待ちでも帯が出ない
#
# 画像は 置き場/new と 置き場/legacy へ分けて落とす（`bash scripts/test-launch-wait-band-2425.sh <置き場>`。
# 省くと一時 dir に落として終了時に消す）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は回ごとの専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# CI には登録しない（実 GUI を立てる）。CI 側の担保は単体テスト（`launch_gate` の issue2425_* /
# `ui_text::launch_wait`）と番犬（`issue2425_起動待ちの帯の高さ指定と会計が同じ定数を共有している`・
# `ターミナルペインの直接の子は想定どおり`）。
#
# 使い方: bash scripts/test-launch-wait-band-2425.sh [画像の置き場]
#         PARTS=visual bash scripts/test-launch-wait-band-2425.sh   # visual / cli だけ
# 終了コード: 0 = 全部 OK / 1 = NG あり / 4 = 未実測あり（tako-vd を用意できない等。NG は無い）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TMUX
unset CLAUDE_CONFIG_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARTS="${PARTS:-visual cli}"

PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else fail "$2"; fi; }
check_eq() {
  if [ "${2}" = "${3}" ]; then pass "${1}（${3}）"; else fail "${1}（期待 '${2}' / 実際 '${3}'）"; fi
}

for bin in tmux zsh python3 perl; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# HOME を差し替える**前**にバイナリを決める（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1

ROOT="$(mktemp -d /tmp/t2425-XXXXXX)"
DUMP_ROOT="${1:-$ROOT/dump}"
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local s
  for s in $SOCKETS; do
    tmux_socket_kill "$s"
  done
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した: $ROOT"
  else
    rm -rf "${ROOT:?}"
  fi
}
trap cleanup EXIT

# JSON の読み取りは 1 本のツールへ寄せる
TOOL="$ROOT/tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys

def value(d):
    return "null" if d is None else (json.dumps(d, ensure_ascii=False) if isinstance(d, (dict, list)) else d)

cmd = sys.argv[1]
if cmd == "get":
    # get <json ファイル> <a.b.c>（配列は数字）。null / 無い は "null"
    d = json.load(open(sys.argv[2]))
    for k in sys.argv[3].split("."):
        if isinstance(d, list):
            d = d[int(k)] if int(k) < len(d) else None
        elif isinstance(d, dict):
            d = d.get(k)
        else:
            d = None
    print(value(d))
elif cmd == "band":
    # band <tako list の json> <pane> <field>: そのペインの launch_wait_band の値（帯が無ければ "null"）
    d = json.load(open(sys.argv[2]))
    for t in d.get("tabs", []):
        for p in t.get("panes", []):
            if p.get("id") == int(sys.argv[3]):
                band = p.get("launch_wait_band")
                if sys.argv[4] == ".":
                    print(value(band))
                else:
                    print(value(None if band is None else band.get(sys.argv[4])))
                sys.exit(0)
    print("missing")
elif cmd == "panes":
    d = json.load(open(sys.argv[2]))
    print(" ".join(str(p["id"]) for t in d["tabs"] for p in t["panes"]))
else:
    raise SystemExit("未知のコマンド: " + cmd)
PYEOF
J() { python3 "$TOOL" "$@"; }

# 回ごとの置き場（data / discovery / orch / tmux を回で分ける = 前の回の器を拾わない）
#   new_round <名前>
new_round() {
  R="$ROOT/$1"
  mkdir -p "$R/bin" "$R/home" "$R/zdot" "$R/work"
  export HOME="$R/home"
  export ZDOTDIR="$R/zdot"
  export TAKO_DATA_DIR="$R/data"
  export TAKO_DISCOVERY_DIR="$R/disc"
  export TAKO_ORCHESTRATOR_DIR="$R/orch"
  export TAKO_SESSIONS_FILE="$R/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$R/panelogs"
  export TAKO_WORKERS_FILE="$R/workers.yaml"
  SOCK="t2425-$1-$$"
  SOCKETS="$SOCKETS $SOCK"
  export TAKO_TMUX_SOCKET="$SOCK"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  local d
  for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$ROOT"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done
  LAUNCHES="$R/launches.log"
  INJECT="$R/inject.json"
  # 偽 claude: GUI 自身の問い合わせ（--version / agents / -p）には答えるだけ。起動は時刻・pid・引数を記録して眠る
  cat > "$R/bin/claude" <<STUB
#!/bin/sh
case "\$1" in
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
  agents) echo "[]"; exit 0 ;;
  -p) exit 0 ;;
esac
t=\$(perl -MTime::HiRes=time -e 'printf("%.3f", time)')
printf '%s %s %s\n' "\$t" "\$\$" "\$*" >> "$LAUNCHES"
echo "FAKECLAUDE-STARTED"
exec sleep 900
STUB
  chmod +x "$R/bin/claude"
  # ペインのシェル: 固定のプロンプト（実ユーザー名・ホスト名を画面と画像へ出さない = #927）+ 偽 claude を先頭に
  printf '%s\n' 'PS1="[test@host]$ "' "export PATH=\"$R/bin:\$PATH\"" > "$R/zdot/.zshrc"
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  PATH: "$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"
YAML
  "$TAKO_BIN" orchestrator projects add --key t2425 --cwd "$R/work" \
    --description "#2425 の検証（自動削除される）" >/dev/null 2>&1
}

# 注入: inject <pressure>（書きかけを読ませない = 一時ファイルへ書いて rename）。逼迫中の空きは帯の文に出る
# 値、normal の空きは予約と見込みを足しても余る値（空きが少ないままだと圧が normal でも
# 「空きメモリ不足」で待ち続ける）
inject() {
  local avail=1229
  [ "$1" = normal ] && avail=20480
  printf '{"total_mb":36864,"available_mb":%s,"pressure":"%s","swap_used_mb":0,"startup_secs":2}\n' \
    "$avail" "$1" > "$INJECT.tmp"
  command mv -f "$INJECT.tmp" "$INJECT"
}

launch_count() {
  if [ -f "$LAUNCHES" ]; then grep -c . "$LAUNCHES" | tr -d ' '; else echo 0; fi
}
# 起動の記録が n 行以上になるまで待つ（最大 s 秒）。結果の行数を返す
wait_launches() {
  local n="$1" s="$2" i
  for i in $(seq 1 $((s * 5))); do
    [ "$(launch_count)" -ge "$n" ] && break
    sleep 0.2
  done
  launch_count
}
# ペインの帯の state が want になるまで待つ（最大 s 秒。帯が無いのを待つなら want=null）。最後の値を返す
wait_band_state() {
  local pane="$1" want="$2" s="$3" i got=""
  for i in $(seq 1 $((s * 5))); do
    "$TAKO_BIN" list > "$R/wait.json" 2>/dev/null
    got="$(J band "$R/wait.json" "$pane" state)"
    [ "$got" = "$want" ] && break
    sleep 0.2
  done
  echo "$got"
}

spawn_json() {
  # spawn_json <出力> <ラベル>
  "$TAKO_BIN" orchestrator spawn --project t2425 --prompt "#2425 の検証" --pane "$MASTER" \
    --label "$2" > "$1" 2>"$1.err"
}

mcp_call() {
  # mcp_call <出力> <tool> <arguments の JSON>（MCP の stdio ブリッジ越し。token はログへ出さない）
  local out="$1" tool="$2" args="$3" sock tok
  sock="$(J get "$TAKO_DISCOVERY_DIR/control.json" socket)"
  tok="$(J get "$TAKO_DISCOVERY_DIR/control.json" token)"
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2425","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' "$tool" "$args"
    sleep 4
  } | env TAKO_SOCKET="$sock" TAKO_TOKEN="$tok" "$TAKO_BIN" mcp serve 2>/dev/null |
    python3 -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("id") == 2:
        print(m["result"]["content"][0]["text"])
        break
' > "$out"
}

# ============================== visual-test 節 ==============================
#   run_section <腕> [VAR=VAL …]: 節を 1 回走らせて、終わるまで**状態で**待つ（上限 180 秒）
run_section() {
  local arm="$1"
  shift
  new_round "visual-$arm"
  inject critical
  mkdir -p "$DUMP_ROOT/$arm"
  # 面を用意できなければ 4（#1744）= 呼び手が未実測と書く
  local rc=0
  launch_isolated_gui "$R/app.log" "SHELL=/bin/zsh" "ZDOTDIR=$ZDOTDIR" \
    "PATH=$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" "TAKO_2413_INJECT=$INJECT" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=launch-wait-band "TAKO_VISUAL_DUMP_DIR=$DUMP_ROOT/$arm" \
    ${1+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    return "$rc"
  fi
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  tmux_socket_kill "$SOCK"
  return 0
}

part_visual() {
  local arm log rc
  for arm in new legacy; do
    echo
    if [ "$arm" = new ]; then
      echo "=== visual-test launch-wait-band（新しい形）==="
      run_section new
    else
      echo "=== visual-test launch-wait-band（A/B: TAKO_2425_LEGACY=1）==="
      run_section legacy TAKO_2425_LEGACY=1
    fi
    rc=$?
    log="$R/app.log"
    if [ "$rc" -eq 4 ]; then
      unmeasured "${arm}: tako-vd を用意できない（蓋閉じ等）"
      continue
    elif [ "$rc" -ne 0 ]; then
      fail "${arm}: 隔離 GUI を起こせない（終了コード ${rc}）"
      continue
    fi
    grep -E "TAKO_VISUAL_2425" "$log" | sed 's/^/    /'
    if grep -q "TAKO_VISUAL_2425: SKIPPED" "$log"; then
      unmeasured "${arm}: 節が SKIPPED（$(grep -o 'SKIPPED.*' "$log" | head -1)）"
    elif grep -q "TAKO_VISUAL_TEST_OK" "$log" && grep -q "TAKO_VISUAL_2425: all_ok" "$log"; then
      pass "${arm}: 節が緑（TAKO_VISUAL_TEST_OK）"
    else
      fail "${arm}: 節が緑にならない"
      grep -E "FAILED|panicked|ERROR" "$log" | tail -5 | sed 's/^/    /'
    fi
    ls "$DUMP_ROOT/$arm"/launch-wait-band-*.png 2>/dev/null | sed 's/^/    画像: /'
  done
}

# ============================== 実 spawn と CLI / MCP ==============================
start_gui() {
  local name="$1"
  shift
  # 面を用意できなければ 4（#1744）= 呼び手が未実測と書く
  local rc=0
  launch_isolated_gui "$R/app-${name}.log" "TAKO_PERSIST=1" "SHELL=/bin/zsh" "ZDOTDIR=$ZDOTDIR" \
    "PATH=$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" "TAKO_2413_INJECT=$INJECT" \
    ${1+"$@"} || rc=$?
  if [ "$rc" -ne 0 ]; then
    return "$rc"
  fi
  if ! wait_isolated_gui "$R/app-${name}.log"; then
    fail "隔離 GUI が起動しない（${name}）"
    return 1
  fi
  return 0
}
stop_gui() {
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
  tmux_socket_kill "$SOCK"
}

part_cli() {
  echo
  echo "=== 実 spawn の帯を CLI / MCP で読む（注入: warn → queue start → normal）==="
  new_round cli
  inject warn
  start_gui cli
  case $? in
    0) : ;;
    4) unmeasured "cli: tako-vd を用意できない（蓋閉じ等）"; return ;;
    *) fail "cli: 隔離 GUI を起こせない"; return ;;
  esac
  "$TAKO_BIN" list > "$R/list.json"
  MASTER="$(J panes "$R/list.json" | awk '{print $1}')"
  "$TAKO_BIN" title --pane "$MASTER" --role "orchestrator-master:t2425" >/dev/null 2>&1
  spawn_json "$R/s1.json" a
  spawn_json "$R/s2.json" b
  local p1 p2
  p1="$(J get "$R/s1.json" pane_id)"
  p2="$(J get "$R/s2.json" pane_id)"
  check_eq "spawn は起動待ち" "waiting waiting" "$(J get "$R/s1.json" launch_gate.state) $(J get "$R/s2.json" launch_gate.state)"
  check_eq "帯が出る（1 本目）" waiting "$(wait_band_state "$p1" waiting 5)"
  "$TAKO_BIN" list > "$R/l1.json"
  echo "    帯（CLI・1 本目）: $(J band "$R/l1.json" "$p1" .)"
  check_eq "順番 1/2" "1/2" "$(J band "$R/l1.json" "$p1" position)/$(J band "$R/l1.json" "$p1" waiting)"
  check_eq "順番 2/2" "2/2" "$(J band "$R/l1.json" "$p2" position)/$(J band "$R/l1.json" "$p2" waiting)"
  check_eq "理由のコードと圧" "pressure_warn warn 1229" \
    "$(J band "$R/l1.json" "$p1" code) $(J band "$R/l1.json" "$p1" pressure) $(J band "$R/l1.json" "$p1" available_mb)"
  check "$([ "$(J band "$R/l1.json" "$p1" button)" != null ] && echo 1 || echo 0)" \
    "ボタンの文言がある（$(J band "$R/l1.json" "$p1" button)）"
  check_eq "上書きの案内" "tako queue start $p1" "$(J band "$R/l1.json" "$p1" start_now)"
  mcp_call "$R/mcp.json" tako_list_panes '{}'
  check_eq "MCP tako_list_panes にも同じ帯" "waiting 1 pressure_warn" \
    "$(J band "$R/mcp.json" "$p1" state) $(J band "$R/mcp.json" "$p1" position) $(J band "$R/mcp.json" "$p1" code)"
  sleep 2
  check_eq "待っている間は偽 claude が起きない" 0 "$(launch_count)"

  # 上書き: tako queue start → 帯が消え、偽 claude が 1 回だけ起きる
  "$TAKO_BIN" queue start "$p1" > "$R/start.txt" 2>&1
  check_eq "queue start で起きる" 1 "$(wait_launches 1 15)"
  check_eq "起きたら帯が消える（1 本目）" null "$(wait_band_state "$p1" null 10)"
  "$TAKO_BIN" list > "$R/l2.json"
  check_eq "残りの順番が繰り上がる" "1/1" "$(J band "$R/l2.json" "$p2" position)/$(J band "$R/l2.json" "$p2" waiting)"

  # normal へ戻すと残りも起き、帯が消える
  inject normal
  check_eq "normal で残りも起きる（起動コマンドは 1 本 1 回）" 2 "$(wait_launches 2 15)"
  check_eq "起きたら帯が消える（2 本目）" null "$(wait_band_state "$p2" null 10)"
  sleep 2
  check_eq "起動は 2 回のまま（二重に打たない）" 2 "$(launch_count)"
  stop_gui

  echo
  echo "=== A/B: TAKO_2425_LEGACY=1（同じ注入 critical・起動待ちはそのまま）==="
  new_round cli-legacy
  inject critical
  start_gui cli-legacy TAKO_2425_LEGACY=1
  case $? in
    0) : ;;
    4) unmeasured "cli-legacy: tako-vd を用意できない（蓋閉じ等）"; return ;;
    *) fail "cli-legacy: 隔離 GUI を起こせない"; return ;;
  esac
  "$TAKO_BIN" list > "$R/list.json"
  MASTER="$(J panes "$R/list.json" | awk '{print $1}')"
  spawn_json "$R/s1.json" legacy
  p1="$(J get "$R/s1.json" pane_id)"
  check_eq "修正前でも起動待ちにはなる" waiting "$(J get "$R/s1.json" launch_gate.state)"
  sleep 3
  "$TAKO_BIN" list > "$R/l1.json"
  check_eq "修正前は帯が出ない" null "$(J band "$R/l1.json" "$p1" .)"
  stop_gui
}

case " $PARTS " in *" visual "*) part_visual ;; esac
case " $PARTS " in *" cli "*) part_cli ;; esac

echo
echo "結果: OK ${PASS} / NG ${FAIL} / 未実測 ${UNMEASURED}"
if [ "$FAIL" -gt 0 ]; then
  exit 1
elif [ "$UNMEASURED" -gt 0 ]; then
  exit 4
fi
exit 0
