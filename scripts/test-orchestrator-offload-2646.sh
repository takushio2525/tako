#!/usr/bin/env bash
# test-orchestrator-offload-2646.sh — orchestrator 系の dispatch（Spawn / Ledger / WorkerStatus の準備）が
# GUI のメインスレッドを専有しないかの実経路テスト（#2646）
#
# 隔離 GUI（`TAKO_ISOLATED=1`）へ本番と同じ大きさの合成データ（台帳 1830 件・workers 200 件・sessions 500 件・
# .claude.json 2 本）を置き、偽の claude（引数に答えて眠るだけ。実の会話・課金は起こさない）で
# worker を spawn する。ファイルの読み書きの段へ遅延を注入（`TAKO_2646_INJECT_IO_MS`。負荷は作らない）し、
# その間に別の IPC（`tako list`）が待たされないか・perf.log にその区間の専有が出ないかを測る:
#   ① spawn（CLI `tako orchestrator spawn` = MCP `tako_orchestrator_spawn` と同じ dispatch）
#   ② 台帳（MCP `tako_orchestrator_ledger` の list / stats / record）
#   ③ worker の状態（MCP `tako_orchestrator_worker_status` の pane_id 指定 / worker 指定）
#   ④ 境界: 届かないペイン・知らない worker・知らない台帳の action・壊れた台帳（読めない）
#   ⑤ A/B（TAKO_2646_LEGACY=1）: 修正前と同じく UI スレッドで待つ。応答・台帳・レジストリ・
#      カタログの記録・断りは ①〜④ と同じ（実行ごとに変わる値は落として比べる）
#
# **本番の GUI / tmux / data dir / .claude.json には一切触らない**（置き場は mktemp 配下。.claude.json は
# 隔離 GUI の検証用ホーム = TMPDIR の下。落とすのは自分で起こした GUI だけ）。窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-orchestrator-offload-2646.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。証拠は TAKO_2646_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TMUX TAKO_2646_LEGACY TAKO_OFFLOAD
unset TAKO_2646_INJECT_IO_MS

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

# 注入する遅延（ms）。ファイルの読み書きの段（台帳・レジストリの読み・spawn の記録）を 1 回ずつ遅らせる
INJECT_MS="${TAKO_2646_INJECT_MS:-1500}"
# 「待たされていない」の上限（ms）。debug ビルドの CLI の起動と `tako list` の往復を含めて余裕を持たせる
QUICK_MS="${TAKO_2646_QUICK_MS:-200}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2646-XXXXXX)"
APP_PID=""
SOCKETS=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  local s
  for s in $SOCKETS; do tmux -L "$s" kill-server >/dev/null 2>&1; done
  if [ -n "${KEEP:-}" ]; then echo "置き場を残した: $TMP"; else rm -rf "${TMP:?}"; fi
}
trap cleanup EXIT

mkdir -p "$TMP/ctl" "$TMP/bin" "$TMP/work" "$TMP/zdot"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
CLI="$TMP/ctl/tako"
# 偽 claude: GUI 自身の問い合わせ（`--version` / `agents --json` / 自動命名の `-p`）には答えるだけ、
# worker として起こされたら眠る（実の claude は起こさない）
cat > "$TMP/bin/claude" <<STUB
#!/bin/sh
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

# 本番と同じ件数・大きさの合成データ（台帳 1830 件 / workers 200 件 / sessions 500 件 / projects 60 件 /
# .claude.json 2 本）を作る perl（中身は合成。本番のデータは読まない）
GEN="$TMP/gen-fixtures.pl"
cat > "$GEN" <<'PL_EOF'
# 本番と同じ件数・大きさの合成データ（台帳 1830 件 / workers 200 件 / sessions 500 件 / projects 60 件 /
# .claude.json 2 本）を作る。中身は合成（本番のデータは読まない）
use strict; use warnings; use utf8;
binmode STDOUT, ':utf8';
my ($orch, $data, $agent_home, $work) = @ARGV;
sub wr { my ($p, $s) = @_; open my $f, '>:utf8', $p or die "$p: $!"; print $f $s; close $f; }
my $jp = "検証用の合成データの本文で同じ長さを再現するための文字列";
# 台帳
my $s = "entries:\n";
for my $i (1..1830) {
  my $id = 1784246134235 + $i * 60000;
  $s .= "- id: spawn_$id\n  ts: 2026-07-16T23:55:34Z\n  project: proj" . ($i % 60) . "\n"
      . "  label: '#" . (1000 + $i) . " 合成ラベル $i'\n  issue: '#" . (1000 + $i) . "'\n"
      . "  task_type: investigation\n  model: claude-opus-5-5\n  effort: xhigh\n  agent: claude\n"
      . "  had_error: false\n  outcome: pass\n  rounds: 1\n  note: 合成メモ$i\n  post_issue: false\n";
}
wr("$orch/ledger.yaml", $s);
# projects
$s = "version: 2\nprojects:\n";
for my $i (0..59) { $s .= "  proj$i:\n    cwd: $work\n    description: $jp $i の説明。$jp\n"; }
$s .= "  t2646:\n    cwd: $work\n    description: '#2646 の検証'\n";
wr("$orch/projects.yaml", $s);
# workers
$s = "workers:\n";
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
# sessions
$s = "entries:\n";
for my $i (1..500) {
  $s .= "  " . sprintf('%08x-0000-0000-0000-%012d', $i, $i) . ":\n    kind: worker\n    label: 合成 $i\n"
      . "    project: proj" . ($i % 60) . "\n    agent: claude\n    model: claude-opus-5-5\n    effort: xhigh\n"
      . "    issues:\n    - " . (2000 + $i) . "\n    - " . (2100 + $i) . "\n    prompt_head: $jp$jp$jp$jp$i\n"
      . "    cwd: $work\n    tab: 151\n    pane: " . (80000 + $i) . "\n    tmux_session: tako-synth-s$i\n"
      . "    recorded_at: 2026-10-30T08:31:29Z\n";
}
wr("$data/sessions.yaml", $s);
# .claude.json（<agent_home>/.claude/.claude.json と <agent_home>/.claude.json）
my $j = "{\n  \"numStartups\": 100,\n  \"projects\": {\n";
my @p;
for my $i (1..600) {
  push @p, "    \"/synthetic/project/path/number/$i\": {\"allowedTools\": [], \"history\": [], \"mcpContextUris\": [], \"mcpServers\": {}, \"enabledMcpjsonServers\": [], \"disabledMcpjsonServers\": [], \"hasTrustDialogAccepted\": true, \"projectOnboardingSeenCount\": 1, \"hasClaudeMdExternalIncludesApproved\": false, \"hasClaudeMdExternalIncludesWarningShown\": false, \"lastCost\": 0.1234, \"lastAPIDuration\": 12345, \"lastDuration\": 123456, \"lastLinesAdded\": 10, \"lastLinesRemoved\": 5, \"lastTotalInputTokens\": 1000, \"lastTotalOutputTokens\": 2000, \"lastSessionId\": \"00000000-0000-0000-0000-" . sprintf('%012d', $i) . "\"}";
}
$j .= join(",\n", @p) . "\n  }\n}\n";
system("mkdir", "-p", "$agent_home/.claude");
wr("$agent_home/.claude/.claude.json", $j);
wr("$agent_home/.claude.json", $j);
PL_EOF

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'; }
# perf.log の「メインスレッド専有: <タグ> が N ms」のうち N が $2 以上の行数（$1 = タグの正規表現・$3 = perf.log）
perf_hits_over() {
  perl -ne 'if (/メインスレッド専有: (\S+) が (\d+)ms/) { my ($tag, $ms) = ($1, $2); $n++ if $tag =~ /^(?:'"$1"')$/ && $ms >= '"$2"' } END { print $n + 0, "\n" }' "$3" 2>/dev/null
}
max_of() {
  if [ -s "$1" ]; then sort -n "$1" | tail -1 | tr -dc '0-9'; else echo 999999; fi
}
# 区間の間 `tako list`（IPC）を打ち続け、1 回ごとの往復の ms を $2 へ書く（$1 = 区間を表す pid）
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
# MCP の stdio ブリッジで隔離 GUI のツールを 1 回呼ぶ（$1 = ツール・$2 = 引数の JSON・$3 = 結果の書き先）
mcp_call() {
  local ctl="$TAKO_DISCOVERY_DIR/control.json"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t2646","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$(jq -r .socket "$ctl")" TAKO_TOKEN="$(jq -r .token "$ctl")" "$CLI" mcp serve \
      >"$3.raw" 2>"$3.err"
  jq -c 'select(.id == 2) | .result' "$3.raw" > "$3"
}
mcp_text() { jq -r '.content[0].text // ""' "$1"; }
# 1 つの操作を背景で走らせ、その間の `tako list` を測る（$1 = 名前・残り = コマンド）
measure() {
  local name="$1" t0 t1 busy
  shift
  t0=$(now_ms)
  "$@" &
  busy=$!
  sleep 0.1
  probe_ipc_while "$busy" "$TMP/$name-ipc.txt"
  wait "$busy"
  t1=$(now_ms)
  echo $((t1 - t0)) > "$TMP/$name-ms.txt"
  echo "    （${name}: $(cat "$TMP/$name-ms.txt")ms・その間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/$name-ipc.txt")ms）"
}
spawn_one() {
  "$CLI" orchestrator spawn --project t2646 --prompt "$2" --pane "$MASTER" --label "$3" >"$1" 2>"$1.err"
}

# 比べられる形へ（実行ごとに変わる値を落とす）
norm_spawn() {
  jq -S -c 'del(.pane_id, .spawned_by, .tab, .ledger_id, .worker_id, .tmux_session, .pane_cols,
    .font_scale, .font_size, .cols_short, .font_refit, .cwd, .command, .claude_command, .agent_path,
    .env_keys, .mcp, .warnings, .placement_reason)' 2>/dev/null
}
norm_status() {
  # 応答の全体（時系列の履歴・事象と画面の写しは実行ごとに揺れうるので落とす。画面は別に確かめる）
  jq -S -c 'del(.history, .events, .recent_output)' 2>/dev/null
}
norm_ledger() { jq -S -c 'del(.entries[]?.id, .entries[]?.ts)' 2>/dev/null; }
norm_text() { sed -E 's/[0-9]{3,}/N/g'; }

# 1 回ぶんの GUI を起こして ①〜④ を回す（$1 = 接頭辞・$2… = GUI へ渡す VAR=VAL）
run_round() {
  local r="$1" i p w l
  shift
  local D="$TMP/$r"
  mkdir -p "$D/data" "$D/disc" "$D/orch/profiles" "$D/tmpd" "$D/home"
  export TAKO_ISOLATED=1 TAKO_DATA_DIR="$D/data" TAKO_DISCOVERY_DIR="$D/disc" \
    TAKO_ORCHESTRATOR_DIR="$D/orch" TAKO_TMUX_SOCKET="t2646-$r-$$" TAKO_LANG=ja
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET"
  for p in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$p" in "$TMP"/*) : ;; *) echo "隔離されていない: $p"; exit 1 ;; esac
  done
  cat > "$D/orch/profiles/default.yaml" <<YAML
env:
  PATH: "$TMP/bin:/usr/bin:/bin:/opt/homebrew/bin"
YAML
  perl "$GEN" "$D/orch" "$D/data" "$D/home" "$TMP/work"
  local PERF="$D/data/perf.log"

  launch_isolated_gui "$D/app.log" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" "PATH=$TMP/bin:$PATH" \
    "TMPDIR=$D/tmpd/" "TAKO_2646_INJECT_IO_MS=$INJECT_MS" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$D/app.log" || { fail "[$r] 隔離 GUI が起動しない"; return; }
  # 隔離 GUI の .claude.json の置き場（検証用のホーム）へ本番と同じ大きさの合成ファイルを置く
  mkdir -p "$D/tmpd/tako-agent-config-$APP_PID"
  command cp -Rf "$D/home/." "$D/tmpd/tako-agent-config-$APP_PID/"
  "$CLI" autorename off >/dev/null 2>&1
  sleep 2
  MASTER="$("$CLI" list | jq -r '.tabs[0].panes[0].id')"

  echo
  echo "== [$r] ① spawn の間に別の IPC は待たされない（記録の段に ${INJECT_MS}ms の遅延を注入）=="
  : > "$PERF"
  for i in 1 2; do
    measure "$r-spawn-$i" spawn_one "$D/spawn-$i.json" "#2646 の round $i" "#2646 r$i"
  done
  sleep 1
  command cp -f "$PERF" "$D/spawn-perf.log"
  grep -E "OrchestratorSpawn|finish_offload|ipc_turn" "$PERF" | sed 's/^/    perf.log: /' | head -8
  for i in 1 2; do
    norm_spawn < "$D/spawn-$i.json" > "$D/spawn-$i.norm"
    p=$(jq -r '.pane_id // empty' "$D/spawn-$i.json")
    eval "P$i=\$p"
    eval "W$i=\$(jq -r '.worker_id // empty' \"\$D/spawn-\$i.json\")"
    eval "L$i=\$(jq -r '.ledger_id // empty' \"\$D/spawn-\$i.json\")"
  done
  check_eq "[$r] ① spawn は 2 本とも worker を立てた（pane_id / worker_id / ledger_id）" "ok" \
    "$([ -n "$P1" ] && [ -n "$P2" ] && [ -n "$W1" ] && [ -n "$L2" ] && echo ok)"
  # 台帳・レジストリ・カタログの記録（spawn の 2 本ぶん）
  "$CLI" orchestrator workers 2>/dev/null \
    | jq -S -c --argjson a "$P1" --argjson b "$P2" '[.workers[] | select(.pane == $a or .pane == $b)
      | {project, agent, label, issues, status, prompt_head}] | sort_by(.label)' > "$D/registry.norm"
  "$CLI" sessions list --json 2>/dev/null \
    | jq -S -c --argjson a "$P1" --argjson b "$P2" '[.pending[]? | select(.pane == $a or .pane == $b)
      | {kind, label, project, agent, issues, prompt_head}] | sort_by(.label)' > "$D/catalog.norm"
  check_eq "[$r] ① レジストリに 2 本の記録" 2 "$(jq 'length' "$D/registry.norm")"
  check_eq "[$r] ① カタログに 2 本の記録" 2 "$(jq 'length' "$D/catalog.norm")"
  # 送達: 偽 claude が起きた（起動コマンドがペインへ届いた）
  local started=0 screen
  for i in $(seq 1 100); do
    screen=$("$CLI" read --pane "$P1" 2>/dev/null) && [[ "$screen" == *FAKECLAUDE-STARTED* ]] && { started=1; break; }
    sleep 0.1
  done
  check_eq "[$r] ① 起動コマンドが worker のペインへ届いた（偽 claude が起きた）" 1 "$started"

  echo
  echo "== [$r] ② 台帳（list / stats / record）の間に別の IPC は待たされない =="
  : > "$PERF"
  measure "$r-ledger-list" mcp_call tako_orchestrator_ledger '{"action":"list","limit":3}' "$D/ledger-list.json"
  measure "$r-ledger-stats" mcp_call tako_orchestrator_ledger '{"action":"stats"}' "$D/ledger-stats.json"
  measure "$r-ledger-record" mcp_call tako_orchestrator_ledger "{\"action\":\"record\",\"id\":\"$L1\",\"outcome\":\"pass\",\"rounds\":1,\"note\":\"#2646\"}" "$D/ledger-record.json"
  sleep 1
  command cp -f "$PERF" "$D/ledger-perf.log"
  grep -E "OrchestratorLedger|ipc_turn" "$PERF" | sed 's/^/    perf.log: /' | head -6
  mcp_text "$D/ledger-list.json" | norm_ledger > "$D/ledger-list.norm"
  mcp_text "$D/ledger-stats.json" | jq -S -c . > "$D/ledger-stats.norm"
  mcp_text "$D/ledger-record.json" | jq -S -c 'del(.id)' > "$D/ledger-record.norm"
  check_eq "[$r] ② record が台帳へ書けた" '{"ok":true,"outcome":"pass"}' "$(cat "$D/ledger-record.norm")"
  check_eq "[$r] ② list は合成 1830 件 + spawn の 2 件" 1832 "$(mcp_text "$D/ledger-list.json" | jq '.total')"

  echo
  echo "== [$r] ③ worker の状態（pane_id 指定 / worker 指定）の準備で別の IPC は待たされない =="
  : > "$PERF"
  measure "$r-ws-pane" mcp_call tako_orchestrator_worker_status "{\"pane_id\":$P1}" "$D/ws-pane.json"
  measure "$r-ws-worker" mcp_call tako_orchestrator_worker_status "{\"worker\":\"$W2\"}" "$D/ws-worker.json"
  sleep 1
  command cp -f "$PERF" "$D/ws-perf.log"
  grep -E "OrchestratorWorkerStatus|ipc_turn|finish_offload" "$PERF" | sed 's/^/    perf.log: /' | head -6
  mcp_text "$D/ws-pane.json" | norm_status > "$D/ws-pane.norm"
  mcp_text "$D/ws-worker.json" | norm_status > "$D/ws-worker.norm"
  check_eq "[$r] ③ pane_id 指定はペインの画面を読んだ（偽 claude の出力が見える）" "screen/true" \
    "$(mcp_text "$D/ws-pane.json" | jq -r '"\(.status_source)/\(.recent_output | tostring | contains("FAKECLAUDE-STARTED"))"')"
  check_eq "[$r] ③ worker 指定もペインの画面を読んだ" "screen/true" \
    "$(mcp_text "$D/ws-worker.json" | jq -r '"\(.status_source)/\(.recent_output | tostring | contains("FAKECLAUDE-STARTED"))"')"

  echo
  echo "== [$r] ④ 境界: 届かないペイン・知らない worker・知らない台帳の action・壊れた台帳 =="
  mcp_call tako_orchestrator_worker_status '{"pane_id":999999}' "$D/edge-nopane.json"
  mcp_text "$D/edge-nopane.json" | norm_status > "$D/edge-nopane.norm"
  check_eq "[$r] ④ 届かないペインは gone で答える" gone "$(mcp_text "$D/edge-nopane.json" | jq -r '.status')"
  mcp_call tako_orchestrator_worker_status '{"worker":"no-such-worker"}' "$D/edge-noworker.json"
  mcp_text "$D/edge-noworker.json" | norm_text > "$D/edge-noworker.norm"
  check_eq "[$r] ④ 知らない worker は断る（isError）" true "$(jq -r '.isError // false' "$D/edge-noworker.json")"
  mcp_call tako_orchestrator_ledger '{"action":"bogus"}' "$D/edge-badaction.json"
  mcp_text "$D/edge-badaction.json" > "$D/edge-badaction.norm"
  check_eq "[$r] ④ 知らない台帳の action は断る（isError）" true "$(jq -r '.isError // false' "$D/edge-badaction.json")"
  mcp_call tako_orchestrator_worker_status '{}' "$D/edge-noargs.json"
  mcp_text "$D/edge-noargs.json" > "$D/edge-noargs.norm"
  check_eq "[$r] ④ pane_id も worker も無い要求は断る（isError）" true "$(jq -r '.isError // false' "$D/edge-noargs.json")"
  command cp -f "$D/orch/ledger.yaml" "$D/ledger.good"
  printf 'entries: [\n  - id: {broken\n' > "$D/orch/ledger.yaml"
  mcp_call tako_orchestrator_ledger '{"action":"list"}' "$D/edge-broken.json"
  mcp_text "$D/edge-broken.json" | norm_text > "$D/edge-broken.norm"
  check_eq "[$r] ④ 壊れた台帳は読めない断りを返す（isError）" true "$(jq -r '.isError // false' "$D/edge-broken.json")"
  mcp_call tako_orchestrator_ledger "{\"action\":\"record\",\"id\":\"$L2\",\"outcome\":\"pass\"}" "$D/edge-broken-record.json"
  mcp_text "$D/edge-broken-record.json" | norm_text > "$D/edge-broken-record.norm"
  check_eq "[$r] ④ 壊れた台帳へは書かない（中身は壊れたまま）" "$(printf 'entries: [\n  - id: {broken')" "$(cat "$D/orch/ledger.yaml")"
  command cp -f "$D/ledger.good" "$D/orch/ledger.yaml"
  # spawn の記録の中の台帳が壊れていても spawn は止まらない（台帳の記録は失敗しても止めない）
  printf 'entries: [\n  - id: {broken\n' > "$D/orch/ledger.yaml"
  spawn_one "$D/spawn-broken.json" "#2646 壊れた台帳" "#2646 broken"
  norm_spawn < "$D/spawn-broken.json" > "$D/spawn-broken.norm"
  check_eq "[$r] ④ 台帳が壊れていても spawn は立つ（ledger_id は空）" '/true' \
    "$(jq -r '"\(.ledger_id)/\(.pane_id != null)"' "$D/spawn-broken.json")"
  command cp -f "$D/ledger.good" "$D/orch/ledger.yaml"
  grep -F '[spawn]' "$D/data/persist.log" 2>/dev/null | sed -E 's/^\[[^]]*\] //; s/pane=[0-9]+/pane=N/' > "$D/persist-spawn.txt"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

run_round new
echo
echo "== 修正後（既定）の判定 =="
for i in 1 2; do
  check_lt "① spawn $i の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-spawn-$i-ipc.txt")"
done
check_eq "① perf.log に注入した遅延ぶんの dispatch:OrchestratorSpawn / finish_offload の専有が出ない" 0 \
  "$(perf_hits_over 'dispatch:OrchestratorSpawn|dispatch:finish_offload|ipc_turn' $((INJECT_MS / 2)) "$TMP/new/spawn-perf.log")"
for k in list stats record; do
  check_lt "② 台帳 ${k} の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-ledger-$k-ipc.txt")"
done
check_eq "② perf.log に dispatch:OrchestratorLedger の専有が出ない" 0 \
  "$(perf_hits_over 'dispatch:OrchestratorLedger' 0 "$TMP/new/ledger-perf.log")"
check_lt "③ 状態（pane_id）の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-ws-pane-ipc.txt")"
check_lt "③ 状態（worker）の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-ws-worker-ipc.txt")"
check_eq "③ perf.log に offload_prepare:OrchestratorWorkerStatus の専有が出ない" 0 \
  "$(perf_hits_over 'offload_prepare:OrchestratorWorkerStatus|dispatch:finish_offload|ipc_turn' 0 "$TMP/new/ws-perf.log")"

run_round legacy TAKO_2646_LEGACY=1
echo
echo "== A/B（TAKO_2646_LEGACY=1）の判定: 修正前と同じく UI スレッドで待つ =="
check_ge "A/B ① spawn の間の tako list は待たされる（ms）" $((INJECT_MS / 2)) "$(max_of "$TMP/legacy-spawn-1-ipc.txt")"
check_ge "A/B ① perf.log に注入した遅延ぶんの dispatch:OrchestratorSpawn の専有が出る" 2 \
  "$(perf_hits_over 'dispatch:OrchestratorSpawn' "$INJECT_MS" "$TMP/legacy/spawn-perf.log")"
check_ge "A/B ② 台帳 list の間の tako list は待たされる（ms）" $((INJECT_MS / 2)) "$(max_of "$TMP/legacy-ledger-list-ipc.txt")"
check_ge "A/B ② perf.log に dispatch:OrchestratorLedger の専有が出る" 3 \
  "$(perf_hits_over 'dispatch:OrchestratorLedger' "$INJECT_MS" "$TMP/legacy/ledger-perf.log")"
check_ge "A/B ③ 状態（pane_id）の間の tako list は待たされる（ms）" $((INJECT_MS / 2)) "$(max_of "$TMP/legacy-ws-pane-ipc.txt")"
check_ge "A/B ③ perf.log に offload_prepare:OrchestratorWorkerStatus の専有が出る" 2 \
  "$(perf_hits_over 'offload_prepare:OrchestratorWorkerStatus' "$INJECT_MS" "$TMP/legacy/ws-perf.log")"

echo
echo "== 応答・記録・断りは修正前（A/B）と同じ（実行ごとに変わる値は落として比べる）=="
for f in spawn-1 spawn-2 spawn-broken registry catalog ledger-list ledger-stats ledger-record \
  ws-pane ws-worker edge-nopane edge-noworker edge-badaction edge-noargs edge-broken edge-broken-record; do
  check_eq "$f" "$(cat "$TMP/legacy/$f.norm" 2>/dev/null)" "$(cat "$TMP/new/$f.norm" 2>/dev/null)"
done
check_eq "persist.log の [spawn] の行（ペイン ID を落として）" "$(cat "$TMP/legacy/persist-spawn.txt")" "$(cat "$TMP/new/persist-spawn.txt")"

if [ -n "${TAKO_2646_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2646_EVIDENCE_DIR"
  for f in "$TMP"/new-* "$TMP"/legacy-*; do [ -f "$f" ] && command cp -f "$f" "$TAKO_2646_EVIDENCE_DIR/"; done
  for r in new legacy; do
    mkdir -p "$TAKO_2646_EVIDENCE_DIR/$r"
    for f in "$TMP/$r"/*.norm "$TMP/$r"/*-perf.log "$TMP/$r"/*.json; do
      [ -f "$f" ] && command cp -f "$f" "$TAKO_2646_EVIDENCE_DIR/$r/"
    done
  done
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
