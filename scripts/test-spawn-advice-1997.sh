#!/usr/bin/env bash
# test-spawn-advice-1997.sh — spawn / run の応答の `machine` / `advice` と、worker ごとの常駐メモリ（#1997）の実経路テスト
#
# 隔離した data / HOME / tmux で**実 tako-app** を立て、メモリは**注入**（`TAKO_2413_INJECT` のファイル）で与える
# = 負荷は生成しない。claude は偽物（起動した時刻・pid・引数を記録して眠るだけ）なので本物の agent は立たない。
#
#   ① spawn の応答の `machine`（注入の値・予約・1 本の見込み・agent の本数・起動待ちの件数）と空の `advice`
#   ② 境界: 空き = 予約 + 2 × 見込み なら助言なし・直後の 2 本目も起きる（起動待ちと同じ判断）
#   ③ 境界: 空き = 予約 + 2 × 見込み − 64 MB なら `next_launch_waits`（理由 memory）・直後の 2 本目は実際に待つ
#   ④ この spawn 自体が待つ（空き < 予約 + 見込み）なら `next_launch_waits` を出さない（`launch_gate` が言う）
#   ⑤ MCP `tako_orchestrator_run` の応答にも `machine` / `advice`（③ と同じ注入で同じ助言）
#   ⑥ A/B: `TAKO_2413_LEGACY=1`（起動待ちが無効）は同じ注入でも `launches` が null・`next_launch_waits` を出さない
#   ⑦ fleet: visual-test `fleet-resident-1997` が worker 2 本の常駐メモリを CLI / MCP / `tako panel` / 画面で
#      突き合わせ、画像を `$OUT/fleet-resident-1997-fleet-{dark,light}.png` へ
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は回ごとの専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# CI には登録しない（実 GUI を立てる）。CI 側の担保は単体テスト（`tako_core::queue::headroom` の issue1997_* /
# `tako_control::spawn_advice` / registry の issue1997_*）と番犬（`issue1993_resources_ui_thread_watchdog` の 9 番）。
#
# 使い方: bash scripts/test-spawn-advice-1997.sh                 # 全部
#         PARTS=advice bash scripts/test-spawn-advice-1997.sh    # ①〜⑥ だけ
#         OUT=/path/to/dir PARTS=fleet bash scripts/test-spawn-advice-1997.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TMUX
unset CLAUDE_CONFIG_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARTS="${PARTS:-advice fleet}"
# 立ち上がりの窓（秒。注入で縮める = 本番の既定は 20 秒）。② ③ の 2 本目を 1 本目の窓の中で起こす
STARTUP="${STARTUP:-8}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
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
# HOME を差し替える**前**にバイナリを決める（後で呼ぶとツールチェーンを一時 HOME へ取り直す）。
# visual-test の節（⑦）も同じバイナリで回す（節を指定しない起動は通常の GUI と同じ）
isolated_gui_bins visual-test || exit 1

ROOT="$(mktemp -d /tmp/t1997-XXXXXX)"
OUT="${OUT:-$ROOT/out}"
mkdir -p "$OUT"
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
    # 画像を残すなら OUT を置き場の外へ向ける（既定は置き場の中 = 一緒に消える）
    rm -rf "${ROOT:?}"
  fi
}
trap cleanup EXIT

TOOL="$ROOT/tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys

cmd = sys.argv[1]
if cmd == "get":
    # get <json ファイル> <a.b.c>（配列は数字）。null / 無い は "null"
    try:
        d = json.load(open(sys.argv[2]))
    except Exception:
        print("unreadable")
        raise SystemExit(0)
    for k in sys.argv[3].split("."):
        if isinstance(d, list):
            d = d[int(k)] if int(k) < len(d) else None
        elif isinstance(d, dict):
            d = d.get(k)
        else:
            d = None
    print("null" if d is None else (json.dumps(d, ensure_ascii=False) if isinstance(d, (dict, list)) else d))
elif cmd == "advice":
    # advice <json ファイル>: 助言の code:reason を空白区切りで（無ければ "-"）
    d = json.load(open(sys.argv[2]))
    a = d.get("advice") or []
    print(" ".join("%s:%s" % (x.get("code"), x.get("reason")) for x in a) or "-")
elif cmd == "panes":
    d = json.load(open(sys.argv[2]))
    print(" ".join(str(p["id"]) for t in d["tabs"] for p in t["panes"]))
elif cmd == "starting":
    # starting <tako queue --json>: 起動待ちの待ち + 立ち上がり中の件数
    d = json.load(open(sys.argv[2]))
    l = d.get("launches") or {}
    print(len(l.get("waiting") or []) + len(l.get("starting") or []))
else:
    raise SystemExit("未知のコマンド: " + cmd)
PYEOF
J() { python3 "$TOOL" "$@"; }

# 回ごとの置き場（data / discovery / orch / tmux を回で分ける = 前の回の器を拾わない）
#   new_round <名前>
new_round() {
  R="$ROOT/$1"
  mkdir -p "$R/bin" "$R/home/.claude/projects/p" "$R/zdot" "$R/work"
  export HOME="$R/home"
  export ZDOTDIR="$R/zdot"
  export TAKO_DATA_DIR="$R/data"
  export TAKO_DISCOVERY_DIR="$R/disc"
  export TAKO_ORCHESTRATOR_DIR="$R/orch"
  export TAKO_SESSIONS_FILE="$R/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$R/panelogs"
  export TAKO_WORKERS_FILE="$R/workers.yaml"
  SOCK="t1997-$1-$$"
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
  printf '%s\n' 'PS1="[test@host]$ "' "export PATH=\"$R/bin:\$PATH\"" > "$R/zdot/.zshrc"
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  PATH: "$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"
YAML
  "$TAKO_BIN" orchestrator projects add --key t1997 --cwd "$R/work" \
    --description "#1997 の検証（自動削除される）" >/dev/null 2>&1
}

# 注入: inject <pressure> <available_mb>（書きかけを読ませない = 一時ファイルへ書いて rename）
inject() {
  printf '{"total_mb":36864,"available_mb":%s,"pressure":"%s","swap_used_mb":0,"startup_secs":%s}\n' \
    "$2" "$1" "$STARTUP" > "$INJECT.tmp"
  command mv -f "$INJECT.tmp" "$INJECT"
  # GUI は 1 秒ごとに読み直す
  sleep 1.5
}

#   start_gui <ログの名前> [VAR=VAL …]
start_gui() {
  local name="$1"
  shift
  launch_isolated_gui "$R/app-${name}.log" "SHELL=/bin/zsh" "ZDOTDIR=$ZDOTDIR" \
    "PATH=$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" "TAKO_2413_INJECT=$INJECT" \
    ${@+"$@"} || exit $?
  if ! wait_isolated_gui "$R/app-${name}.log"; then
    fail "隔離 GUI が起動しない（${name}）"
    exit 1
  fi
}
stop_gui() {
  stop_isolated_gui "$ISOLATED_GUI_PID"
  sleep 1
}

spawn_json() {
  # spawn_json <出力> <ラベル>
  "$TAKO_BIN" orchestrator spawn --project t1997 --prompt "#1997 の検証" --pane "$MASTER" \
    --label "$2" > "$1" 2>"$1.err"
}

# 立ち上がり中・起動待ちが 0 件になるまで待つ（最大 s 秒）
wait_settled() {
  local s="$1" i
  for i in $(seq 1 $((s * 2))); do
    "$TAKO_BIN" queue --json > "$R/q.json" 2>/dev/null
    [ "$(J starting "$R/q.json")" = 0 ] && return 0
    sleep 0.5
  done
  return 1
}

mcp_call() {
  # mcp_call <出力> <tool> <arguments の JSON>（MCP の stdio ブリッジ越し。token はログへ出さない）
  local out="$1" tool="$2" args="$3" sock tok
  sock="$(J get "$TAKO_DISCOVERY_DIR/control.json" socket)"
  tok="$(J get "$TAKO_DISCOVERY_DIR/control.json" token)"
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t1997","version":"0"}}}'
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

# ============================== 助言 ==============================
part_advice() {
  echo
  echo "=== spawn / run の machine と advice（注入: normal・立ち上がり ${STARTUP} 秒）==="
  new_round advice
  inject normal 20480
  start_gui advice
  "$TAKO_BIN" list > "$R/list.json"
  MASTER="$(J panes "$R/list.json" | awk '{print $1}')"
  "$TAKO_BIN" title --pane "$MASTER" --role "orchestrator-master:t1997" >/dev/null 2>&1

  # 見込みは過去の記録（立ち上がりの最大）で系統ごとに置き換わる。先に 1 本立ち上がらせて記録を作り、
  # 以後の見込みを固定する（偽 claude のペインの木は 256 MB の床より小さい = 以後は 256 MB のまま）
  spawn_json "$R/w0.json" warm
  wait_settled $((STARTUP + 10)) || fail "準備: 立ち上がりが終わらない"

  # ① 余裕がある spawn: machine の欄と空の助言
  spawn_json "$R/s1.json" roomy
  check_eq "① launch_gate は starting" starting "$(J get "$R/s1.json" launch_gate.state)"
  check_eq "① machine.pressure は注入の値" normal "$(J get "$R/s1.json" machine.pressure)"
  check_eq "① machine.available_mb は注入の値" 20480 "$(J get "$R/s1.json" machine.available_mb)"
  check_eq "① machine.source" injected "$(J get "$R/s1.json" machine.source)"
  check_eq "① 隔離 GUI は調停役でない（machine.queue は null）" null "$(J get "$R/s1.json" machine.queue)"
  check_eq "① 起動待ちの立ち上がり中に自分を数える" 1 "$(J get "$R/s1.json" machine.launches.starting)"
  check_eq "① 余裕があれば advice は空" "-" "$(J advice "$R/s1.json")"
  local E RSV
  E="$(J get "$R/s1.json" machine.launches.estimate_mb)"
  RSV="$(J get "$R/s1.json" machine.reserve_mb)"
  echo "    1 本の見込み ${E} MB・予約 ${RSV} MB（36 GB の 10%）・agent $(J get "$R/s1.json" machine.agents)"
  check "$([ "$RSV" = 3686 ] && echo 1 || echo 0)" "① 予約は重い処理と同じ実効値（${RSV} MB）"
  wait_settled $((STARTUP + 10)) || fail "① 立ち上がりが終わらない"

  # ② 境界ちょうど: 助言なし・直後の 2 本目も起きる
  local hi lo
  hi=$((RSV + 2 * E))
  lo=$((hi - 64))
  inject normal "$hi"
  spawn_json "$R/a2.json" hi-a
  spawn_json "$R/b2.json" hi-b
  check_eq "② 見込みは前の回と同じ（境界の前提）" "$E" "$(J get "$R/a2.json" machine.launches.estimate_mb)"
  check_eq "② 空き ${hi} MB: 1 本目は起きる" starting "$(J get "$R/a2.json" launch_gate.state)"
  check_eq "② 空き ${hi} MB: 助言なし" "-" "$(J advice "$R/a2.json")"
  check_eq "② 直後の 2 本目も起きる（起動待ちの答えと一致）" starting "$(J get "$R/b2.json" launch_gate.state)"
  inject normal 20480
  wait_settled $((STARTUP + 10)) || fail "② 立ち上がりが終わらない"

  # ③ 境界の手前: next_launch_waits・直後の 2 本目は実際に待つ
  inject normal "$lo"
  spawn_json "$R/a3.json" lo-a
  spawn_json "$R/b3.json" lo-b
  check_eq "③ 空き ${lo} MB: 1 本目は起きる" starting "$(J get "$R/a3.json" launch_gate.state)"
  check_eq "③ 空き ${lo} MB: 助言は next_launch_waits（理由 memory）" "next_launch_waits:memory" "$(J advice "$R/a3.json")"
  echo "    助言の文: $(J get "$R/a3.json" advice.0.text)"
  check_eq "③ 直後の 2 本目は実際に待つ（助言の予告どおり）" waiting "$(J get "$R/b3.json" launch_gate.state)"
  check_eq "③ 2 本目の待つ理由" memory "$(J get "$R/b3.json" launch_gate.code)"
  check_eq "③ 2 本目の応答の machine.launches.waiting" 1 "$(J get "$R/b3.json" machine.launches.waiting)"

  # ⑤ run（MCP）: 同じ注入で同じ助言（③ の 2 本を起こして立ち上がりを終えてから = 立ち上がり中は run の 1 本だけ）
  inject normal 20480
  wait_settled $((STARTUP + 10)) || fail "③ 立ち上がりが終わらない"
  inject normal "$lo"
  mcp_call "$R/run.json" tako_orchestrator_run "{\"project\":\"t1997\",\"prompt\":\"#1997 run\",\"pane\":$MASTER,\"label\":\"r\",\"timeout_seconds\":60}"
  check_eq "⑤ run の応答の launch_gate" starting "$(J get "$R/run.json" launch_gate.state)"
  check_eq "⑤ run の応答にも助言（next_launch_waits:memory）" "next_launch_waits:memory" "$(J advice "$R/run.json")"
  check_eq "⑤ run の応答の machine.available_mb" "$lo" "$(J get "$R/run.json" machine.available_mb)"
  inject normal 20480
  wait_settled $((STARTUP + 10)) || fail "⑤ 立ち上がりが終わらない"

  # ④ この spawn 自体が待つ: next_launch_waits を出さない
  inject normal $((RSV + E - 1))
  spawn_json "$R/s4.json" wait
  check_eq "④ 空き $((RSV + E - 1)) MB: この spawn は待つ" waiting "$(J get "$R/s4.json" launch_gate.state)"
  check_eq "④ 待った spawn には next_launch_waits を出さない" "-" "$(J advice "$R/s4.json")"
  inject normal 20480
  wait_settled $((STARTUP + 10)) || fail "④ 立ち上がりが終わらない"
  stop_gui
  tmux_socket_kill "$SOCK"

  # ⑥ A/B: 起動待ちが無効（TAKO_2413_LEGACY=1）なら同じ注入でも助言を出さない（次の spawn は実際には待たない）
  echo
  echo "=== A/B: TAKO_2413_LEGACY=1（同じ注入 = 空き ${lo} MB）==="
  new_round advice-legacy
  inject normal "$lo"
  start_gui advice-legacy TAKO_2413_LEGACY=1
  "$TAKO_BIN" list > "$R/list.json"
  MASTER="$(J panes "$R/list.json" | awk '{print $1}')"
  spawn_json "$R/l1.json" legacy-a
  spawn_json "$R/l2.json" legacy-b
  check_eq "⑥ 起動待ちが無効なら machine.launches は null" null "$(J get "$R/l1.json" machine.launches)"
  check_eq "⑥ 起動待ちが無効なら next_launch_waits を出さない" "-" "$(J advice "$R/l1.json")"
  check_eq "⑥ 2 本目も待たない（launch_gate は null）" null "$(J get "$R/l2.json" launch_gate)"
  # 起動待ちが無効なら注入も読まない = 機の値は実測（値そのものは比べない）
  check_eq "⑥ 機の値は実測で出す（machine.source）" measured "$(J get "$R/l1.json" machine.source)"
  stop_gui
  tmux_socket_kill "$SOCK"
}

# ============================== fleet の常駐メモリ ==============================
part_fleet() {
  echo
  echo "=== fleet ビューの worker の常駐メモリ（visual-test fleet-resident-1997）==="
  new_round fleet
  inject normal 20480
  local log="$R/app-fleet.log"
  launch_isolated_gui "$log" "SHELL=/bin/zsh" "ZDOTDIR=$ZDOTDIR" \
    "PATH=$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=fleet-resident-1997 "TAKO_VISUAL_DUMP_DIR=$OUT" \
    "TAKO_1997_CLI=$TAKO_BIN" TAKO_1997_PROJECT=t1997 || exit $?
  local i
  for i in $(seq 1 240); do
    kill -0 "$ISOLATED_GUI_PID" 2>/dev/null || break
    sleep 0.5
  done
  grep -E "TAKO_1997_|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$log" \
    | sed "s|$ROOT|<tmp>|g; s/^/    /"
  check "$(grep -q 'TAKO_1997_FLEET_OK' "$log" && echo 1 || echo 0)" \
    "⑦ 同じ worker の常駐メモリが CLI / MCP / tako panel / 画面で一致（$(grep -m1 -o 'TAKO_APP_SELF_TEST_FAILED.*' "$log" | head -c 200)）"
  check "$([ -f "$OUT/fleet-resident-1997-fleet-dark.png" ] && echo 1 || echo 0)" "⑦ 画像 fleet-dark"
  check "$([ -f "$OUT/fleet-resident-1997-fleet-light.png" ] && echo 1 || echo 0)" "⑦ 画像 fleet-light"
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  tmux_socket_kill "$SOCK"
}

for part in $PARTS; do
  case "$part" in
    advice) part_advice ;;
    fleet) part_fleet ;;
    *) echo "未知の PARTS: $part"; exit 2 ;;
  esac
done

echo
echo "結果: PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
