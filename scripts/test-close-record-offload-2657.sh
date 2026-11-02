#!/usr/bin/env bash
# test-close-record-offload-2657.sh — ペインの close（dispatch:Close / たまり場の kill）が worker レジストリへの
# close の記録を GUI のメインスレッドで書かないかの実経路テスト（#2657）
#
# 隔離 GUI（`TAKO_ISOLATED=1`・器あり = `TAKO_PERSIST=1` = 本番と同じ器の worker）へ本番と同じ大きさの合成の
# workers.yaml（200 件・約 210 KB）を置き、偽の claude（引数に答えて眠るだけ。実の会話・課金は起こさない）で
# worker を spawn してから master の role で閉じる。記録の段へ遅延を注入するか（`TAKO_2657_INJECT_RECORD_MS`）、
# 別のプロセスが workers.yaml のロックを握る（perl の flock。負荷は作らない）間に、別の IPC（`tako list`）が
# 待たされないか・perf.log に `dispatch:Close` の専有が出ないかを測る:
#   ① CLI の close（`tako close --pane N --force` = MCP `tako_close_pane` と同じ dispatch）× 5。記録の段に遅延を注入
#   ② 別のプロセスが workers.yaml.lock を握る間の close × 3（注入なし。本番の「書き手のロック待ち」と同じ形）
#   ③ MCP の close（`tako_close_pane`）と、たまり場の kill（`tako_background_pane` → `tako_background_kill`）
#   ④ 順序: どの close も応答が返った直後の `tako orchestrator workers --all` で closed（発生源つき）
#   ⑤ 境界: 在しないペイン・busy な worker の force なしの close は断り、レジストリを変えない
#   ⑥ A/B（TAKO_2657_LEGACY=1）: 修正前と同じく UI スレッドで書く（list が待たされ、専有が出る）。応答・記録・
#      persist.log の「セッション kill」の行は ①〜⑤ と同じ（実行ごとに変わる値は落として比べる）
#
# **本番の GUI / tmux / data dir には一切触らない**（置き場は mktemp 配下。落とすのは自分で起こした GUI と
# 自分の隔離 tmux ソケットだけ）。窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-close-record-offload-2657.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。証拠は TAKO_2657_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TMUX TAKO_2657_LEGACY TAKO_OFFLOAD
unset TAKO_2657_INJECT_RECORD_MS TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_lt() {
  if [ "$3" -lt "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 未満のはずが ${3}）"; fi
}
check_ge() {
  if [ "$3" -ge "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 以上のはずが ${3}）"; fi
}

for bin in jq perl zsh tmux; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

INJECT_MS="${TAKO_2657_INJECT_MS:-1500}"
LOCK_MS="${TAKO_2657_LOCK_MS:-1500}"
QUICK_MS="${TAKO_2657_QUICK_MS:-200}"
ROUNDS="${TAKO_2657_ROUNDS:-5}"
MASTER_ROLE="orchestrator-master:t2657"
TMP="$(mktemp -d /tmp/t2657-XXXXXX)"
APP_PID=""
SOCKETS=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
isolated_gui_bins || exit 1

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  local s
  # 器はソケットファイルまで消す（#2399）
  for s in $SOCKETS; do tmux_socket_kill "$s"; done
  if [ -n "${KEEP:-}" ]; then echo "置き場を残した: $TMP"; else rm -rf "${TMP:?}"; fi
}
trap cleanup EXIT

mkdir -p "$TMP/ctl" "$TMP/bin" "$TMP/work" "$TMP/zdot"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
CLI="$TMP/ctl/tako"
cat > "$TMP/bin/claude" <<STUB
case "\$1" in
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
  agents) echo "[]"; exit 0 ;;
  -p) exit 0 ;;
esac
echo "FAKECLAUDE-STARTED"
exec sleep 600
STUB
chmod +x "$TMP/bin/claude"
printf '%s\n' 'PS1="[test@host:%1~]\$ "' > "$TMP/zdot/.zshrc"

# 合成の workers.yaml（本番と同じ 200 件・同じキー）と projects.yaml
GEN="$TMP/gen-fixtures.pl"
cat > "$GEN" <<'PL_EOF'
use strict; use warnings; use utf8;
binmode STDOUT, ':utf8';
my ($orch, $data, $work) = @ARGV;
sub wr { my ($p, $s) = @_; open my $f, '>:utf8', $p or die "$p: $!"; print $f $s; close $f; }
my $jp = "検証用の合成データの本文で同じ長さを再現するための文字列";
wr("$orch/projects.yaml", "version: 2\nprojects:\n  t2657:\n    cwd: $work\n    description: '#2657 の検証'\n");
my $s = "workers:\n";
for my $i (1..200) {
  my $id = 1529 + $i;
  $s .= "  '$id':\n    label: 合成 worker $i\n    project: proj" . ($i % 60) . "\n    agent: claude\n"
      . "    model: claude-opus-5-5\n    model_actual: claude-opus-5-5\n    model_actual_at: 2026-10-17T20:26:24Z\n"
      . "    effort: xhigh\n    pane: " . (90000 + $i) . "\n    tab: 344\n    tmux_session: tako-synth-$i\n"
      . "    session_id: 00000000-0000-0000-0000-" . sprintf('%012d', $i) . "\n    ledger_id: spawn_17914544$i\n"
      . "    cwd: $work\n    prompt_head: $jp$jp$jp$jp$jp$jp$i\n    spawned_at: 2026-10-08T10:14:15Z\n"
      . "    status: closed\n    closed_at: 2026-10-08T12:14:15Z\n    close_reason: superseded\n"
      . "    prompt_delivered_at: 2026-10-08T10:14:20Z\n";
}
$s .= "next_id: 1729\n";
wr("$data/workers.yaml", $s);
PL_EOF

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'; }
perf_hits_over() {
  perl -ne 'if (/メインスレッド専有: (\S+) が (\d+)ms/) { my ($tag, $ms) = ($1, $2); $n++ if $tag =~ /^(?:'"$1"')$/ && $ms >= '"$2"' } END { print $n + 0, "\n" }' "$3" 2>/dev/null
}
max_of() {
  if [ -s "$1" ]; then sort -n "$1" | tail -1 | tr -dc '0-9'; else echo 999999; fi
}
min_of() {
  if [ -s "$1" ]; then sort -n "$1" | head -1 | tr -dc '0-9'; else echo 0; fi
}
probe_ipc_while() {
  local busy="$1" out="$2" n=0 t0 t1
  : > "$out"
  while kill -0 "$busy" 2>/dev/null || [ "$n" -lt 3 ]; do
    t0=$(now_ms)
    if "$CLI" list >/dev/null 2>&1; then
      t1=$(now_ms)
      echo $((t1 - t0)) >> "$out"
    fi
    n=$((n + 1))
    [ "$n" -ge 400 ] && break
    sleep 0.05
  done
}
mcp_call() {
  local ctl="$TAKO_DISCOVERY_DIR/control.json"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2657","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$(jq -r .socket "$ctl")" TAKO_TOKEN="$(jq -r .token "$ctl")" \
      TAKO_ORCHESTRATOR_ROLE="$MASTER_ROLE" "$CLI" mcp serve >"$3.raw" 2>"$3.err"
  jq -c 'select(.id == 2) | .result' "$3.raw" > "$3"
}
mcp_text() { jq -r '.content[0].text // ""' "$1"; }
# close を背景で走らせ、その間の `tako list` の往復を測る。close の応答の往復を $TMP/<name>-ms.txt へ
measure() {
  local name="$1" t0 t1 busy
  shift
  t0=$(now_ms)
  "$@" &
  busy=$!
  sleep 0.05
  probe_ipc_while "$busy" "$TMP/$name-ipc.txt"
  wait "$busy"
  t1=$(now_ms)
  echo $((t1 - t0)) > "$TMP/$name-ms.txt"
  echo "    （${name}: close の応答 $(cat "$TMP/$name-ms.txt")ms・その間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/$name-ipc.txt")ms）"
}
# CLI の close は成功しても何も出さない（応答の JSON は MCP の口で見る）。終了コードを $1.rc へ
close_cli() {
  TAKO_ORCHESTRATOR_ROLE="$MASTER_ROLE" "$CLI" close --pane "$2" --force >"$1" 2>"$1.err"
  echo $? > "$1.rc"
}
spawn_worker() {
  local out="$1" label="$2" p i screen
  "$CLI" orchestrator spawn --project t2657 --prompt "#2657 計測 $label" --pane "$MASTER" --label "#2657 $label" \
    >"$out" 2>"$out.err"
  p=$(jq -r '.pane_id // empty' "$out" 2>/dev/null)
  [ -n "$p" ] || { echo ""; return; }
  for i in $(seq 1 100); do
    screen=$("$CLI" read --pane "$p" 2>/dev/null) && [[ "$screen" == *FAKECLAUDE-STARTED* ]] && break
    sleep 0.1
  done
  echo "$p"
}
# 応答の直後に読んだ workers --all でのそのペインの状態（`status/close_reason`）
registry_state() {
  "$CLI" orchestrator workers --all 2>/dev/null \
    | jq -r --argjson p "$1" '[.workers[] | select(.pane == $p)] | last | "\(.status)/\(.close_reason // "")"'
}
hold_lock() {
  perl -e 'use Fcntl qw(:flock); open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; select(undef,undef,undef,$ARGV[1]/1000);' \
    "$TAKO_DATA_DIR/workers.yaml.lock" "$1"
}

run_round() {
  local r="$1" i p out state
  shift
  local D="$TMP/$r"
  mkdir -p "$D/data" "$D/disc" "$D/orch/profiles" "$D/tmpd"
  export TAKO_ISOLATED=1 TAKO_DATA_DIR="$D/data" TAKO_DISCOVERY_DIR="$D/disc" \
    TAKO_ORCHESTRATOR_DIR="$D/orch" TAKO_TMUX_SOCKET="t2657-$r-$$" TAKO_LANG=ja
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET"
  for p in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$p" in "$TMP"/*) : ;; *) echo "隔離されていない: $p"; exit 1 ;; esac
  done
  cat > "$D/orch/profiles/default.yaml" <<YAML
env:
  PATH: "$TMP/bin:/usr/bin:/bin:/opt/homebrew/bin"
YAML
  perl "$GEN" "$D/orch" "$D/data" "$TMP/work"
  local PERF="$D/data/perf.log"

  launch_isolated_gui "$D/app.log" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" "PATH=$TMP/bin:$PATH" \
    "TMPDIR=$D/tmpd/" "TAKO_PERSIST=1" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$D/app.log" || { fail "[$r] 隔離 GUI が起動しない"; return; }
  "$CLI" autorename off >/dev/null 2>&1
  sleep 2
  MASTER="$("$CLI" list | jq -r '.tabs[0].panes[0].id')"
  "$CLI" title --pane "$MASTER" --role "$MASTER_ROLE" >/dev/null 2>&1

  echo
  echo "== [$r] ① CLI の close × ${ROUNDS}（記録の段に注入: $*）=="
  : > "$PERF"
  : > "$TMP/$r-order.txt"
  for i in $(seq 1 "$ROUNDS"); do
    p=$(spawn_worker "$D/spawn-$i.json" "c$i")
    [ -n "$p" ] || { fail "[$r] ① worker $i を spawn できない"; continue; }
    sleep 1
    measure "$r-close-$i" close_cli "$D/close-$i.json" "$p"
    state=$(registry_state "$p")
    echo "$state" >> "$TMP/$r-order.txt"
    { cat "$D/close-$i.json.rc"; cat "$D/close-$i.json" "$D/close-$i.json.err"; } > "$D/close-$i.norm"
    check_eq "[$r] ① close $i は成功する（終了コード 0）" 0 "$(cat "$D/close-$i.json.rc")"
    check_eq "[$r] ① close $i でペインが消えた" 1 \
      "$("$CLI" list | jq --argjson p "$p" '[.tabs[].panes[].id] | index($p) == null' | grep -q true && echo 1)"
    check_eq "[$r] ④ close $i の応答の直後に workers は closed（発生源つき）" \
      "closed/close:dispatch(cli, caller=$MASTER_ROLE)" "$state"
  done
  sleep 1
  command cp -f "$PERF" "$D/close-perf.log"
  grep -E "dispatch:Close|ipc_turn" "$PERF" | sed 's/^/    perf.log: /' | head -8

  echo
  echo "== [$r] ③ MCP の close とたまり場の kill =="
  p=$(spawn_worker "$D/spawn-mcp.json" "mcp")
  if [ -n "$p" ]; then
    sleep 1
    mcp_call tako_close_pane "{\"pane\":$p,\"force\":true}" "$D/mcp-close.json"
    mcp_text "$D/mcp-close.json" | jq -S -c 'del(.closed)' > "$D/mcp-close.norm" 2>/dev/null
    check_eq "[$r] ③ MCP の close の応答はそのペイン" "$p" "$(mcp_text "$D/mcp-close.json" | jq -r '.closed')"
    check_eq "[$r] ③ MCP の close の応答の直後に workers は closed" \
      "closed/close:dispatch(mcp, caller=$MASTER_ROLE)" "$(registry_state "$p")"
  else
    fail "[$r] ③ MCP 用の worker を spawn できない"
  fi
  p=$(spawn_worker "$D/spawn-shelf.json" "shelf")
  if [ -n "$p" ]; then
    sleep 1
    mcp_call tako_background_pane "{\"pane\":$p}" "$D/shelve.json"
    check_eq "[$r] ③ たまり場へ退避しただけでは active" "active/" "$(registry_state "$p")"
    mcp_call tako_background_kill "{\"pane\":$p}" "$D/shelf-kill.json"
    mcp_text "$D/shelf-kill.json" | jq -S -c 'del(.killed)' > "$D/shelf-kill.norm" 2>/dev/null
    check_eq "[$r] ③ たまり場の kill の応答（器あり）" "$p/tmux" \
      "$(mcp_text "$D/shelf-kill.json" | jq -r '"\(.killed)/\(.vessel)"')"
    # たまり場の kill は呼び出し元の role を載せない（#775 からの形。修正前と同じ）
    check_eq "[$r] ③ たまり場の kill の応答の直後に workers は closed" \
      "closed/close:dispatch(mcp)" "$(registry_state "$p")"
  else
    fail "[$r] ③ たまり場用の worker を spawn できない"
  fi

  echo
  echo "== [$r] ⑤ 境界: 在しないペイン・busy な worker の force なしの close =="
  TAKO_ORCHESTRATOR_ROLE="$MASTER_ROLE" "$CLI" close --pane 999999 >"$D/edge-missing.txt" 2>&1
  check_eq "[$r] ⑤ 在しないペインの close は断る" 1 "$([ -s "$D/edge-missing.txt" ] && grep -qi "999999\|見つから\|not found" "$D/edge-missing.txt" && echo 1)"
  sed -E 's/[0-9]{3,}/N/g' "$D/edge-missing.txt" > "$D/edge-missing.norm"
  p=$(spawn_worker "$D/spawn-busy.json" "busy")
  if [ -n "$p" ]; then
    sleep 1
    TAKO_ORCHESTRATOR_ROLE="$MASTER_ROLE" "$CLI" close --pane "$p" >"$D/edge-busy.txt" 2>&1
    sed -E 's/[0-9]{3,}/N/g' "$D/edge-busy.txt" > "$D/edge-busy.norm"
    check_eq "[$r] ⑤ busy な worker の force なしの close は断る" 1 \
      "$(grep -q "Worker is still active" "$D/edge-busy.txt" && echo 1)"
    check_eq "[$r] ⑤ 断った close はレジストリを変えない" "active/" "$(registry_state "$p")"
    check_eq "[$r] ⑤ 断った close はペインを閉じない" 1 \
      "$("$CLI" list | jq --argjson p "$p" '[.tabs[].panes[].id] | index($p) != null' | grep -q true && echo 1)"
  else
    fail "[$r] ⑤ busy 用の worker を spawn できない"
  fi

  grep -F 'セッション kill:' "$D/data/persist.log" 2>/dev/null \
    | sed -E 's/^\[[^]]*\] //; s/pane=[0-9]+/pane=N/; s/session=tako-[0-9a-f]+/session=S/; s/\[pid [0-9]+\]/[pid P]/' \
    > "$D/persist-kill.txt"
  check_ge "[$r] persist.log に「セッション kill」の行（器の worker の close ぶん）" $((ROUNDS + 2)) \
    "$(wc -l < "$D/persist-kill.txt" | tr -dc '0-9')"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# ② は修正後だけ: 別のプロセスがロックを握る間の close（注入なし）
run_lock_round() {
  local r=lock i p
  local D="$TMP/$r"
  mkdir -p "$D/data" "$D/disc" "$D/orch/profiles" "$D/tmpd"
  export TAKO_ISOLATED=1 TAKO_DATA_DIR="$D/data" TAKO_DISCOVERY_DIR="$D/disc" \
    TAKO_ORCHESTRATOR_DIR="$D/orch" TAKO_TMUX_SOCKET="t2657-$r-$$" TAKO_LANG=ja
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET"
  cat > "$D/orch/profiles/default.yaml" <<YAML
env:
  PATH: "$TMP/bin:/usr/bin:/bin:/opt/homebrew/bin"
YAML
  perl "$GEN" "$D/orch" "$D/data" "$TMP/work"
  local PERF="$D/data/perf.log"
  launch_isolated_gui "$D/app.log" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" "PATH=$TMP/bin:$PATH" \
    "TMPDIR=$D/tmpd/" "TAKO_PERSIST=1" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$D/app.log" || { fail "[$r] 隔離 GUI が起動しない"; return; }
  "$CLI" autorename off >/dev/null 2>&1
  sleep 2
  MASTER="$("$CLI" list | jq -r '.tabs[0].panes[0].id')"
  "$CLI" title --pane "$MASTER" --role "$MASTER_ROLE" >/dev/null 2>&1
  echo
  echo "== [$r] ② 別のプロセスが workers.yaml のロックを ${LOCK_MS}ms 握る間の close × 3（$*）=="
  : > "$PERF"
  for i in 1 2 3; do
    p=$(spawn_worker "$D/spawn-$i.json" "l$i")
    [ -n "$p" ] || { fail "[$r] ② worker $i を spawn できない"; continue; }
    sleep 1
    hold_lock "$LOCK_MS" &
    local lk=$!
    sleep 0.1
    measure "$r-close-$i" close_cli "$D/close-$i.json" "$p"
    wait "$lk" 2>/dev/null
    check_eq "[$r] ② close $i の応答の直後に workers は closed" \
      "closed/close:dispatch(cli, caller=$MASTER_ROLE)" "$(registry_state "$p")"
  done
  sleep 1
  command cp -f "$PERF" "$D/close-perf.log"
  grep -E "dispatch:Close|ipc_turn" "$PERF" | sed 's/^/    perf.log: /' | head -8
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

run_round new "TAKO_2657_INJECT_RECORD_MS=$INJECT_MS"
run_lock_round
echo
echo "== 修正後（既定）の判定 =="
for i in $(seq 1 "$ROUNDS"); do
  check_lt "① close $i の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-close-$i-ipc.txt")"
  check_ge "① close $i の応答は記録を書き終わってから（注入ぶん待つ。ms）" "$INJECT_MS" "$(cat "$TMP/new-close-$i-ms.txt")"
done
check_eq "① perf.log に dispatch:Close の専有（32ms 以上）が出ない" 0 \
  "$(perf_hits_over 'dispatch:Close' 32 "$TMP/new/close-perf.log")"
check_eq "① perf.log に注入ぶんの ipc_turn の専有が出ない" 0 \
  "$(perf_hits_over 'ipc_turn' $((INJECT_MS / 2)) "$TMP/new/close-perf.log")"
for i in 1 2 3; do
  check_lt "② close $i の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/lock-close-$i-ipc.txt")"
  check_ge "② close $i の応答はロックが空いて書き終わってから（ms）" $((LOCK_MS / 2)) "$(cat "$TMP/lock-close-$i-ms.txt")"
done
check_eq "② perf.log に dispatch:Close の専有が出ない" 0 \
  "$(perf_hits_over 'dispatch:Close' 32 "$TMP/lock/close-perf.log")"

run_round legacy "TAKO_2657_INJECT_RECORD_MS=$INJECT_MS" TAKO_2657_LEGACY=1
echo
echo "== A/B（TAKO_2657_LEGACY=1）の判定: 修正前と同じく UI スレッドで書く =="
for i in $(seq 1 "$ROUNDS"); do
  check_ge "A/B ① close $i の間の tako list は待たされる（ms）" $((INJECT_MS / 2)) "$(max_of "$TMP/legacy-close-$i-ipc.txt")"
done
check_ge "A/B ① perf.log に注入ぶんの dispatch:Close の専有が出る" "$ROUNDS" \
  "$(perf_hits_over 'dispatch:Close' "$INJECT_MS" "$TMP/legacy/close-perf.log")"

echo
echo "== 応答・記録・断り・persist.log の行は修正前（A/B）と同じ =="
check_eq "④ 応答の直後の workers の状態（${ROUNDS} 件）" "$(cat "$TMP/legacy-order.txt")" "$(cat "$TMP/new-order.txt")"
for f in close-1 mcp-close shelf-kill edge-missing edge-busy persist-kill; do
  ext=norm
  [ "$f" = persist-kill ] && ext=txt
  check_eq "$f" "$(cat "$TMP/legacy/$f.$ext" 2>/dev/null)" "$(cat "$TMP/new/$f.$ext" 2>/dev/null)"
done

if [ -n "${TAKO_2657_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2657_EVIDENCE_DIR"
  for f in "$TMP"/new-* "$TMP"/lock-* "$TMP"/legacy-*; do [ -f "$f" ] && command cp -f "$f" "$TAKO_2657_EVIDENCE_DIR/"; done
  for r in new lock legacy; do
    mkdir -p "$TAKO_2657_EVIDENCE_DIR/$r"
    for f in "$TMP/$r"/*.norm "$TMP/$r"/*-perf.log "$TMP/$r"/*.json "$TMP/$r"/*.txt; do
      [ -f "$f" ] && command cp -f "$f" "$TAKO_2657_EVIDENCE_DIR/$r/"
    done
  done
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
