#!/usr/bin/env bash
# test-launch-gate-2413.sh — メモリが逼迫しているあいだ agent の起動（spawn / run / 復元）を待たせ、
# 再起動後の復元を時差にすることの実経路テスト（#2413）
#
# 隔離した data / HOME / tmux で**実 tako-app** を立て、メモリ圧は**注入**（`TAKO_2413_INJECT` のファイル。
# GUI は 1 秒ごとに読み直す）で与える = 負荷は生成しない。claude は偽物（起動した時刻・pid・引数を
# 記録して眠るだけ）なので本物の agent は立たない。
#
#   ① spawn: warn で `launch_gate.state=waiting`（理由・順番・上書きの案内）・起動コマンドを打たない・
#      `workers` の launch=waiting・worker の状態は launch_waiting・`tako queue` の launches に並ぶ
#   ② 待っている間に critical → normal: critical の間も起きず、normal で起きる
#   ③ 上書き: critical のまま `tako queue start <ペイン>` で起きる
#   ④ run（MCP の tako_orchestrator_run）: 起動待ちの間 run_status は launch_waiting、normal で起きる
#      （起きた後は完了待ちの初期待ちの間も launch_waiting を残さない = #2426）。起動待ちのまま閉じた run は終わる
#   ⑤ 待っているペインを閉じる → 起こさない（persist.log に 1 行）
#   ⑥ 走っている agent（偽 claude）は圧が critical に戻っても 1 本も殺されない
#   ⑦ 復元: N ペインが 1 本ずつ（立ち上がりの窓ごと）・master が先・同時に起きる agent は 1 本
#      （準備で保存した layout に master が 1 枚あることを先に確かめる = 材料の無いまま並びを見ない。#2426）
#   ⑧ 逼迫中の復元: critical の間は 1 本も起きず、normal で時差で起きる
#   ⑨ 復元する agent が 0 本の起動では、起動待ちは何もしない（persist.log に復元の行が無い）
#   ⑩ A/B: `TAKO_2413_LEGACY=1` は同じ注入でも spawn を即座に起こし、復元を一斉に起こす（修正前）
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は回ごとの専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# CI には登録しない（実 GUI を立てる）。CI 側の担保は単体テスト（`tako_core::queue` の issue2413_* /
# `tako_control::launch_gate` / dispatch・registry・wait の issue2413_*）と番犬
# （`issue1993_resources_ui_thread_watchdog` の ⑨⑩）。
#
# 使い方: bash scripts/test-launch-gate-2413.sh            # 全部
#         PARTS=spawn bash scripts/test-launch-gate-2413.sh   # spawn / restore だけ
#         N=8 STARTUP=5 bash scripts/test-launch-gate-2413.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TMUX
unset CLAUDE_CONFIG_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARTS="${PARTS:-spawn restore}"
# 復元するペインの数
N="${N:-6}"
# 立ち上がりの窓（秒。注入で縮める = 本番の既定は 20 秒）
STARTUP="${STARTUP:-4}"
# 「同時に起きた」とみなす幅（秒）。1 本目だけはログインシェルの立ち上がりぶん遅れて起きるので、
# 窓そのものではなく窓より十分短い幅で数える
SAME="${SAME:-1.5}"

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
# HOME を差し替える**前**にバイナリを決める（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

ROOT="$(mktemp -d /tmp/t2413-XXXXXX)"
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

# JSON の読み取り・layout の細工・時刻の集計は 1 本のツールへ寄せる
TOOL="$ROOT/tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys, uuid

def leaves(node, acc):
    if isinstance(node, dict):
        if "id" in node and "origin" in node and "title_source" in node:
            acc.append(node)
        for v in node.values():
            leaves(v, acc)
    elif isinstance(node, list):
        for v in node:
            leaves(v, acc)
    return acc

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
    print("null" if d is None else (json.dumps(d, ensure_ascii=False) if isinstance(d, (dict, list)) else d))
elif cmd == "worker":
    # worker <workers の json> <pane> <field>
    d = json.load(open(sys.argv[2]))
    for w in d.get("workers", []):
        if w.get("pane") == int(sys.argv[3]):
            v = w.get(sys.argv[4])
            print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else v))
            break
    else:
        print("missing")
elif cmd == "panes":
    # tako list の全ペイン id（タブ順）
    d = json.load(open(sys.argv[2]))
    print(" ".join(str(p["id"]) for t in d["tabs"] for p in t["panes"]))
elif cmd == "seed_layout":
    # seed_layout <layout.json> <transcripts の置き場> <対応表の出力>: 端末ペインへ会話 ID を付け、器を外す
    path, tdir, out = sys.argv[2], sys.argv[3], sys.argv[4]
    d = json.load(open(path))
    rows = []
    for p in leaves(d.get("tabs", []), []):
        if p.get("preview") or p.get("webview"):
            continue
        sid = str(uuid.uuid4())
        p["claude_session_id"] = sid
        p["session"] = None
        open("%s/%s.jsonl" % (tdir, sid), "w").write('{"type":"user","message":{"role":"user","content":"x"}}\n')
        rows.append("%s %d %s" % (sid, p["id"], p.get("role") or "-"))
    json.dump(d, open(path, "w"), ensure_ascii=False)
    open(out, "w").write("\n".join(rows) + "\n")
    print(len(rows))
elif cmd == "starts":
    # starts <偽 claude の記録> <対応表>: 会話 ID ごとの起動時刻を「時刻 ペイン 役割」で古い順に
    table = {}
    for line in open(sys.argv[3]):
        parts = line.split()
        if len(parts) == 3:
            table[parts[0]] = (parts[1], parts[2])
    out = []
    try:
        lines = open(sys.argv[2]).read().splitlines()
    except FileNotFoundError:
        lines = []
    for line in lines:
        parts = line.split()
        if "--resume" not in parts:
            continue
        sid = parts[parts.index("--resume") + 1]
        if sid in table:
            out.append((float(parts[0]), table[sid][0], table[sid][1], parts[1]))
    out.sort()
    for t, pane, role, pid in out:
        print("%.3f %s %s %s" % (t, pane, role, pid))
elif cmd == "stagger":
    # stagger <starts の出力> <同時とみなす幅（秒）>: 起きた本数・間隔の最小/中央値/最大・
    # 同時（幅の中）に起きた本数の最大・最初から最後まで
    ts = [float(l.split()[0]) for l in open(sys.argv[2]) if l.strip()]
    win = float(sys.argv[3])
    gaps = sorted(b - a for a, b in zip(ts, ts[1:]))
    overlap = max([sum(1 for u in ts if t <= u < t + win) for t in ts] or [0])
    med = gaps[len(gaps) // 2] if gaps else 0
    print("%d %.2f %.2f %.2f %d %.2f" % (len(ts), gaps[0] if gaps else 0, med, gaps[-1] if gaps else 0,
                                        overlap, (ts[-1] - ts[0]) if len(ts) > 1 else 0))
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
  SOCK="t2413-$1-$$"
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
  # ペインのシェル: 固定のプロンプト + 偽 claude を先頭に（path_helper が並べ替えても本物を拾わない）
  printf '%s\n' 'PS1="[test@host]$ "' "export PATH=\"$R/bin:\$PATH\"" > "$R/zdot/.zshrc"
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  PATH: "$R/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"
YAML
  "$TAKO_BIN" orchestrator projects add --key t2413 --cwd "$R/work" \
    --description "#2413 の検証（自動削除される）" >/dev/null 2>&1
}

# 注入: inject <pressure>
inject() {
  # 書きかけを読ませない（一時ファイルへ書いて rename）
  printf '{"total_mb":36864,"available_mb":20480,"pressure":"%s","swap_used_mb":0,"startup_secs":%s}\n' \
    "$1" "$STARTUP" > "$INJECT.tmp"
  command mv -f "$INJECT.tmp" "$INJECT"
}

#   start_gui <ログの名前> [VAR=VAL …]
start_gui() {
  local name="$1"
  shift
  launch_isolated_gui "$R/app-${name}.log" "TAKO_PERSIST=1" "SHELL=/bin/zsh" "ZDOTDIR=$ZDOTDIR" \
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

spawn_json() {
  # spawn_json <出力> <ラベル>
  "$TAKO_BIN" orchestrator spawn --project t2413 --prompt "#2413 の検証" --pane "$MASTER" \
    --label "$2" > "$1" 2>"$1.err"
}

mcp_call() {
  # mcp_call <出力> <tool> <arguments の JSON>（MCP の stdio ブリッジ越し。token はログへ出さない）
  local out="$1" tool="$2" args="$3" sock tok
  sock="$(J get "$TAKO_DISCOVERY_DIR/control.json" socket)"
  tok="$(J get "$TAKO_DISCOVERY_DIR/control.json" token)"
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2413","version":"0"}}}'
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

# ============================== spawn / run ==============================
part_spawn() {
  echo
  echo "=== spawn / run（注入: warn → critical → normal・立ち上がり ${STARTUP} 秒）==="
  new_round spawn
  inject warn
  start_gui spawn
  "$TAKO_BIN" list > "$R/list.json"
  MASTER="$(J panes "$R/list.json" | awk '{print $1}')"
  "$TAKO_BIN" title --pane "$MASTER" --role "orchestrator-master:t2413" >/dev/null 2>&1

  # ① warn の spawn は待ち
  spawn_json "$R/s1.json" a
  local p1 p2 p3 p4 n
  p1="$(J get "$R/s1.json" pane_id)"
  check_eq "① warn の spawn は待ちになる" waiting "$(J get "$R/s1.json" launch_gate.state)"
  check_eq "① 理由のコード" pressure_warn "$(J get "$R/s1.json" launch_gate.code)"
  echo "    理由: $(J get "$R/s1.json" launch_gate.reason)"
  check_eq "① 順番" 1 "$(J get "$R/s1.json" launch_gate.position)"
  check_eq "① 上書きの案内" "tako queue start $p1" "$(J get "$R/s1.json" launch_gate.start_now)"
  spawn_json "$R/s2.json" b
  p2="$(J get "$R/s2.json" pane_id)"
  check_eq "① 2 本目は順番 2" 2 "$(J get "$R/s2.json" launch_gate.position)"
  sleep 4
  check_eq "① 待っている間は偽 claude が 1 本も起きない" 0 "$(launch_count)"
  "$TAKO_BIN" orchestrator workers > "$R/w1.json" 2>/dev/null
  check_eq "① workers の launch" waiting "$(J worker "$R/w1.json" "$p1" launch)"
  "$TAKO_BIN" orchestrator status --pane "$p1" > "$R/st1.json" 2>/dev/null
  check_eq "① worker の状態（素のシェルを完了と読まない）" launch_waiting "$(J get "$R/st1.json" status)"
  "$TAKO_BIN" queue --json > "$R/q1.json" 2>/dev/null
  check_eq "① tako queue の launches に 2 件" 2 "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["launches"]["waiting"]))' "$R/q1.json")"
  "$TAKO_BIN" queue 2>/dev/null | grep "agent の起動" | sed 's/^/    /'

  # ② critical → normal
  inject critical
  sleep 3
  "$TAKO_BIN" orchestrator status --pane "$p1" > "$R/st2.json" 2>/dev/null
  check_eq "② critical の間も待つ（理由が critical へ変わる）" pressure_critical "$(J get "$R/st2.json" launch_gate.code)"
  check_eq "② critical の間も起きない" 0 "$(launch_count)"

  # ④ run（MCP）: critical の間に頼む → launch_waiting
  mcp_call "$R/run.json" tako_orchestrator_run "{\"project\":\"t2413\",\"prompt\":\"#2413 run\",\"pane\":$MASTER,\"label\":\"r\",\"timeout_seconds\":120}"
  local run_id run_pane
  run_id="$(J get "$R/run.json" run_id)"
  run_pane="$(J get "$R/run.json" pane_id)"
  check_eq "④ run の開始の応答は待ち" waiting "$(J get "$R/run.json" launch_gate.state)"
  "$TAKO_BIN" orchestrator run-status "$run_id" > "$R/rs1.json" 2>/dev/null
  check_eq "④ run_status は launch_waiting" launch_waiting "$(J get "$R/rs1.json" status)"
  check_eq "④ run_status に理由" pressure_critical "$(J get "$R/rs1.json" launch_gate.code)"
  # ④ 起動待ちのまま閉じた run（終わり方は spawn / run の最後に見る = 完了待ちの読み直しを待つ）
  mcp_call "$R/run2.json" tako_orchestrator_run "{\"project\":\"t2413\",\"prompt\":\"#2413 run closed\",\"pane\":$MASTER,\"label\":\"rc\",\"timeout_seconds\":120}"
  local run2_id t_closed
  run2_id="$(J get "$R/run2.json" run_id)"
  check_eq "④ 閉じる run も待ち" waiting "$(J get "$R/run2.json" launch_gate.state)"
  "$TAKO_BIN" close --pane "$(J get "$R/run2.json" pane_id)" --force >/dev/null 2>&1
  t_closed="$(perl -MTime::HiRes=time -e 'printf("%.3f", time)')"

  # ⑤ 待っているペインを閉じる → 起こさない
  spawn_json "$R/s5.json" closed
  local p5
  p5="$(J get "$R/s5.json" pane_id)"
  "$TAKO_BIN" close --pane "$p5" --force >/dev/null 2>&1
  sleep 2

  local t_normal
  t_normal="$(perl -MTime::HiRes=time -e 'printf("%.3f", time)')"
  inject normal
  n="$(wait_launches 3 15)"
  check_eq "② normal へ戻ると待っていた spawn 2 本と run 1 本が起きる" 3 "$n"
  echo "    normal にしてから起きるまで: $(awk -v t0="$t_normal" '{printf "%.1fs ", $1 - t0}' "$LAUNCHES")"
  sleep 2
  check_eq "⑤ 閉じたペインの起動は起こさない（3 本のまま）" 3 "$(launch_count)"
  grep -q "起動待ちのペインが閉じられた pane=${p5}" "$TAKO_DATA_DIR/persist.log" 2>/dev/null
  check "$([ $? -eq 0 ] && echo 1 || echo 0)" "⑤ persist.log に閉じた 1 行"
  "$TAKO_BIN" orchestrator run-status "$run_id" > "$R/rs2.json" 2>/dev/null
  check "$([ "$(J get "$R/rs2.json" status)" != launch_waiting ] && echo 1 || echo 0)" \
    "④ 起きた後の run_status は launch_waiting でない（$(J get "$R/rs2.json" status)・launch_gate.state $(J get "$R/rs2.json" launch_gate.state)）"
  "$TAKO_BIN" orchestrator workers > "$R/w2.json" 2>/dev/null
  check "$([ "$(J worker "$R/w2.json" "$p1" launch)" != waiting ] && echo 1 || echo 0)" \
    "① 起きた後の workers の launch は waiting でない（$(J worker "$R/w2.json" "$p1" launch)）"
  check "$(grep -q "\[launch-gate\] 起動 pane=${p1} 入口=spawn 待ち=" "$TAKO_DATA_DIR/persist.log" && echo 1 || echo 0)" \
    "② persist.log に待った後の起動の 1 行"

  # ③ 上書き + ⑥ 殺さない
  inject critical
  sleep 2
  spawn_json "$R/s3.json" forced
  p3="$(J get "$R/s3.json" pane_id)"
  check_eq "③ critical の spawn は待ち" waiting "$(J get "$R/s3.json" launch_gate.state)"
  sleep 3
  check_eq "③ 待っている（まだ 3 本）" 3 "$(launch_count)"
  "$TAKO_BIN" queue start "$p3" > "$R/start.txt" 2>&1
  sed 's/^/    /' "$R/start.txt"
  n="$(wait_launches 4 5)"
  check_eq "③ tako queue start で critical のまま起きる" 4 "$n"
  check "$(grep -q "\[launch-gate\] 上書きで起動 pane=${p3}" "$TAKO_DATA_DIR/persist.log" && echo 1 || echo 0)" \
    "③ persist.log に上書きの 1 行"
  "$TAKO_BIN" queue start "$p3" > "$R/start2.txt" 2>&1
  check "$(grep -q "もう起こしている\|起動待ちは無い" "$R/start2.txt" && echo 1 || echo 0)" \
    "③ 2 回目の start は二重に起こさない（$(head -1 "$R/start2.txt")）"
  spawn_json "$R/s4.json" stay
  p4="$(J get "$R/s4.json" pane_id)"
  sleep 3
  local alive=0 total=0 pid
  for pid in $(awk '{print $2}' "$LAUNCHES"); do
    total=$((total + 1))
    kill -0 "$pid" 2>/dev/null && alive=$((alive + 1))
  done
  check_eq "⑥ critical へ戻しても走っている agent は殺さない（生存 / 起動）" "$total/$total" "$alive/$total"
  check_eq "⑥ 新しい spawn は待つ（起動は 4 本のまま）" 4 "$(launch_count)"
  # ④ 起動待ちのまま閉じた run は、起きたと読まずに終わる（agent は 1 本も起こさない = 上の本数に含まれない）
  local k
  for k in $(seq 1 60); do
    "$TAKO_BIN" orchestrator run-status "$run2_id" > "$R/rs3.json" 2>/dev/null
    [ "$(J get "$R/rs3.json" phase)" = finished ] && break
    sleep 1
  done
  check_eq "④ 起動待ちのまま閉じた run は終わる" finished "$(J get "$R/rs3.json" phase)"
  echo "    閉じてから終わるまで: $(perl -MTime::HiRes=time -e "printf('%.1fs', time - $t_closed)")（status $(J get "$R/rs3.json" status)）"
  stop_gui
  tmux_socket_kill "$SOCK"

  # ⑩ A/B: 同じ注入（critical）で修正前は即座に起こす
  echo
  echo "=== A/B: TAKO_2413_LEGACY=1（同じ注入 critical）==="
  new_round spawn-legacy
  inject critical
  start_gui spawn-legacy TAKO_2413_LEGACY=1
  "$TAKO_BIN" list > "$R/list.json"
  MASTER="$(J panes "$R/list.json" | awk '{print $1}')"
  spawn_json "$R/l1.json" legacy
  check_eq "⑩ 修正前の spawn の launch_gate は null" null "$(J get "$R/l1.json" launch_gate)"
  n="$(wait_launches 1 15)"
  check_eq "⑩ 修正前は critical でも即座に起こす" 1 "$n"
  stop_gui
  tmux_socket_kill "$SOCK"
}

# ============================== 復元 ==============================
#   set_role <ペイン> <役割>: 役割を付ける。付けられなければ止める（材料の無いまま並びを見ない）
set_role() {
  if ! "$TAKO_BIN" title --pane "$1" --role "$2" > "$R/title.txt" 2>&1; then
    fail "ペイン $1 に役割 $2 を付けられない（$(head -c 120 "$R/title.txt")）"
    exit 1
  fi
}

#   prepare_restore <回の名前>: N ペイン（最後の 1 枚を master）を作って保存し、器を消して会話 ID を付ける
prepare_restore() {
  new_round "$1"
  inject normal
  start_gui "$1-prep"
  "$TAKO_BIN" list > "$R/list.json"
  local first i p role
  first="$(J panes "$R/list.json" | awk '{print $1}')"
  set_role "$first" "orchestrator-worker:t2413:w0"
  for i in $(seq 2 "$N"); do
    # `tako split` は新しいペインの ID を**数字だけ**で出す（JSON ではない）。#2426: JSON として読むと
    # null になり、続く `title --pane null` が断られて master の役割が付かないまま「master が先」を見ていた
    "$TAKO_BIN" split --pane "$first" > "$R/split.txt" 2>/dev/null
    p="$(tr -d '[:space:]' < "$R/split.txt")"
    case "$p" in
      '' | *[!0-9]*)
        fail "split の出力からペイン ID を読めない（$(head -c 80 "$R/split.txt")）"
        exit 1
        ;;
    esac
    role="orchestrator-worker:t2413:w$i"
    [ "$i" = "$N" ] && role="orchestrator-master:t2413"
    set_role "$p" "$role"
  done
  sleep 4
  # ⑨ 復元する agent が 0 本の起動（この準備の起動）では起動待ちは何もしない
  check "$(grep -q '\[launch-gate\] 復元' "$TAKO_DATA_DIR/persist.log" 2>/dev/null && echo 0 || echo 1)" \
    "⑨ 復元する agent が 0 本の起動では復元の行が出ない"
  stop_gui
  # PC の再起動の代わり: 器（tmux）ごと消す
  tmux_socket_kill "$SOCK"
  sleep 1
  cp "$TAKO_DATA_DIR/layout.json" "$R/layout.seed.json"
  J seed_layout "$TAKO_DATA_DIR/layout.json" "$HOME/.claude/projects/p" "$R/table.txt" > "$R/seeded.txt"
  cp "$TAKO_DATA_DIR/layout.json" "$R/layout.restore.json"
  echo "    会話 ID を付けたペイン: $(cat "$R/seeded.txt") 本"
  # 並びの判定の材料: 保存された layout に master が 1 枚・役割の無いペインが無い（#2426）
  check_eq "⑦ 準備: layout の master は 1 枚" 1 "$(grep -c ' orchestrator-master:' "$R/table.txt" | tr -d ' ')"
  check_eq "⑦ 準備: 役割の無いペインは 0 枚" 0 "$(awk '$3 == "-"' "$R/table.txt" | grep -c . | tr -d ' ')"
}

# 同じ layout で起動し直す（前の回の器と起動の記録を消す）
reset_restore() {
  tmux_socket_kill "$SOCK"
  sleep 1
  cp "$R/layout.restore.json" "$TAKO_DATA_DIR/layout.json"
  : > "$LAUNCHES"
  : > "$TAKO_DATA_DIR/persist.log"
}

report_stagger() {
  # report_stagger <ラベル>: 起きた順と間隔・同じ窓の中で起きた本数
  J starts "$LAUNCHES" "$R/table.txt" > "$R/starts.txt"
  echo "    起きた順（時刻 ペイン 役割 pid）:"
  awk 'NR==1{t0=$1} {printf "      +%.2fs ペイン %s %s\n", $1 - t0, $2, $3}' "$R/starts.txt"
  read -r COUNT GAP_MIN GAP_MED GAP_MAX OVERLAP SPAN < <(J stagger "$R/starts.txt" "$SAME")
  echo "    $1: 起きた ${COUNT} 本・間隔 最小 ${GAP_MIN}s / 中央値 ${GAP_MED}s / 最大 ${GAP_MAX}s・同時（${SAME} 秒以内）に起きた本数の最大 ${OVERLAP}・最初から最後まで ${SPAN}s"
}

part_restore() {
  echo
  echo "=== 復元（${N} ペイン・master 1・立ち上がり ${STARTUP} 秒）==="
  prepare_restore restore
  local expect="$N" first_role
  # ⑦ normal の復元は 1 本ずつ
  reset_restore
  inject normal
  start_gui restore-new
  wait_launches "$expect" $((N * (STARTUP + 2) + 10)) >/dev/null
  sleep 1
  report_stagger "修正後"
  check_eq "⑦ 全部起きる" "$expect" "$COUNT"
  check_eq "⑦ 同時に起きる agent は 1 本（一斉に起こさない）" 1 "$OVERLAP"
  check "$(python3 -c "import sys; sys.exit(0 if abs(float('$GAP_MED') - float('$STARTUP')) <= 1.0 else 1)" && echo 1 || echo 0)" \
    "⑦ 間隔の中央値は立ち上がりの窓（${STARTUP} 秒）どおり（${GAP_MED}s）"
  first_role="$(awk 'NR==1{print $3}' "$R/starts.txt")"
  check_eq "⑦ master が先" "orchestrator-master:t2413" "$first_role"
  check "$(grep -q '\[launch-gate\] 復元: agent' "$TAKO_DATA_DIR/persist.log" && echo 1 || echo 0)" \
    "⑦ persist.log に時差の 1 行（$(grep -o '\[launch-gate\] 復元: agent [0-9]* 本' "$TAKO_DATA_DIR/persist.log" | head -1)）"
  local alive=0 total=0 pid
  for pid in $(awk '{print $4}' "$R/starts.txt"); do
    total=$((total + 1))
    kill -0 "$pid" 2>/dev/null && alive=$((alive + 1))
  done
  check_eq "⑦ 起きた agent は全部生きている（生存 / 起動）" "$total/$total" "$alive/$total"
  stop_gui

  # ⑧ critical の間は 1 本も起きず、normal で時差
  reset_restore
  inject critical
  start_gui restore-critical
  sleep 6
  check_eq "⑧ critical の間は 1 本も起きない" 0 "$(launch_count)"
  "$TAKO_BIN" queue --json > "$R/q2.json" 2>/dev/null
  check_eq "⑧ tako queue の launches に全部並ぶ" "$N" "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["launches"]["waiting"]))' "$R/q2.json")"
  echo "    先頭: $(python3 -c 'import json,sys;w=json.load(open(sys.argv[1]))["launches"]["waiting"][0];print(w["pane"], w["label"], w["priority"], w["reason"])' "$R/q2.json")"
  inject normal
  wait_launches "$expect" $((N * (STARTUP + 2) + 10)) >/dev/null
  sleep 1
  report_stagger "逼迫が解けた後"
  check_eq "⑧ normal へ戻ると全部起きる" "$expect" "$COUNT"
  check_eq "⑧ 戻った後も同時に起きる agent は 1 本" 1 "$OVERLAP"
  stop_gui

  # ⑩ A/B: 修正前は一斉
  reset_restore
  inject critical
  start_gui restore-legacy TAKO_2413_LEGACY=1
  wait_launches "$expect" 20 >/dev/null
  sleep 1
  report_stagger "修正前（TAKO_2413_LEGACY=1・注入 critical）"
  check_eq "⑩ 修正前は critical でも全部起こす" "$expect" "$COUNT"
  check "$([ "$OVERLAP" -ge 3 ] && echo 1 || echo 0)" "⑩ 修正前は一斉（${SAME} 秒以内に ${OVERLAP} 本）"
  stop_gui
  tmux_socket_kill "$SOCK"
}

for part in $PARTS; do
  case "$part" in
    spawn) part_spawn ;;
    restore) part_restore ;;
    *) echo "未知の PARTS: $part"; exit 2 ;;
  esac
done

echo
echo "結果: PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
