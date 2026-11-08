#!/usr/bin/env bash
# #2792: 器（tmux / psmux）が無くても orchestrator の worker 管理（spawn → watch → status → report →
# send → close）が動くか、器が無いと何が欠けるかを、隔離 GUI（仮想ディスプレイ tako-vd）の実経路で
# 器なし / 器ありの 2 腕に並べて測る。docs の依存表と `setup_deps` の用途文はこの結果に合わせてある。
#
# 腕:
#   none … 器なし = tmux を入れていない機。PATH から Homebrew を外し、既知の場所も見せない
#          （`TAKO_2151_INJECT=skip-known-paths:tmux`）。設定は器を使う側（TAKO_PERSIST=1）にして、
#          「使いたくても器が無い」実際の利用者の形で起こす
#   tmux … 器あり（TAKO_PERSIST=1・隔離ソケット）
#
# 段（腕ごと。置き場・器のソケットは腕ごとに分ける）:
#   ① 最初のペインで `tako master` を打って master を起こし、そこから spawn（CLI `tako orchestrator spawn` =
#      MCP `tako_orchestrator_spawn` と同じ dispatch）して依頼文の到達を見る
#   ② watch / status / report（MCP `tako_orchestrator_worker_status` / `tako_orchestrator_report`）
#   ③ 追加の指示（`tako send` = MCP `tako_send_input`）の到達
#   ③' codex の worker の spawn → watch → status → report → close（偽 codex。器なしで report が読めないのは欠け）
#   ④ 境界 1: worker の agent が突然死 → status / watch が死を名指すか → 案内の `resume` で同じ会話へ戻るか
#   ⑤ 境界 2: GUI の再起動 → master / worker のプロセスが生き残るか・会話が戻るか・report が続くか
#   ⑥ close（`tako close` = MCP `tako_close_pane`）→ ペインが消え・レジストリが閉じ・agent が終わる
#      （器なしで agent が終了処理の途中で残るのは閉じる経路の既知の不具合 #2803 = 数えずに `[既知 #2803]` と名指す）
#
# claude は偽物（API を呼ばない最小の TUI。alt screen に入り最下行に `❯` の入力欄を描き、Enter で
# 受けた依頼を会話の記録（transcript の JSONL）へ user / assistant の 2 行として書き、決まった報告
# `REPORT-2792` を返す。`agents --json` には生きている自分を答える）。本物の claude・アカウントの枠は使わない。
# 本番の GUI / tmux / data dir / 設定 dir には触らない（HOME も一時 dir）。**CI には登録しない**（tako-vd が要る）。
# tako-vd が描画できない（蓋閉じ）と終了コード 4 = 未実測。
#
# 使い方: scripts/test-orchestrator-container-2792.sh            # 既定 = none tmux
#         ARMS=none scripts/test-orchestrator-container-2792.sh
#         KEEP=1 …（置き場を残す。応答の全文は <置き場>/responses/ に入る）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
ARMS="${ARMS:-none tmux}"

PASS=0
FAIL=0
UNMEASURED=0
KNOWN=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux jq perl; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

# 本番 GUI を指す env・呼び出し元の印・本物の設定の置き場を落とす（#1449 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TMUX \
  CODEX_HOME OPENAI_API_KEY CLAUDE_CONFIG_DIR ZDOTDIR TAKO_2151_INJECT
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT

# HOME を差し替える前に建てる（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

TMP_ROOTS=""
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local d
  # 器はソケットファイルまで消す（#2399）。器の中の偽物は器と一緒に HUP で終わる
  # shellcheck disable=SC2086 # 名前の並びを語へ分ける
  tmux_socket_kill $SOCKETS
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do
      case "$d" in /tmp/t2792-* | /private/tmp/t2792-*) rm -rf "${d:?}" ;; esac
    done
  fi
}
trap cleanup EXIT

# 偽の claude の入口（API を呼ばない）。GUI の問い合わせ（`--version` / `agents --json` / 自動命名の `-p`）には
# 答えるだけ、worker として起こされたら会話 ID を決めて最小の TUI（tui.pl）へ置き換わる。
# `--resume <ID>` なら同じ会話 ID・同じ会話の記録を使う（突然死の復旧と GUI の復元が同じ会話へ戻るかを見る）
write_fake_claude() {
  cat >"$1" <<'FAKE'
#!/bin/bash
case "${1:-}" in
  agents)
    printf '['
    sep=''
    for f in "$FAKE_DIR"/agents/*; do
      [ -f "$f" ] || continue
      pid=${f##*/}
      kill -0 "$pid" 2>/dev/null || { rm -f "$f"; continue; }
      IFS='|' read -r sid cwd _ <"$f"
      printf '%s{"pid":%s,"sessionId":"%s","cwd":"%s","model":"claude-opus-5","status":"idle","kind":"interactive"}' \
        "$sep" "$pid" "$sid" "$cwd"
      sep=','
    done
    printf ']\n'
    exit 0
    ;;
  --version | -v) echo "2.1.301 (Claude Code)"; exit 0 ;;
  -p) exit 0 ;;
esac
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
resumed=no
[ -n "$sid" ] && resumed=yes
[ -n "$sid" ] || sid=$(printf '%08x-2792-4000-8000-%012x' "$$" "$(date +%s)")
slug=$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')
cfg="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
mkdir -p "$cfg/projects/$slug" "$FAKE_DIR/agents"
transcript="$cfg/projects/$slug/$sid.jsonl"
[ -f "$transcript" ] || : >"$transcript"
role="${TAKO_ORCHESTRATOR_ROLE:-}"
echo "$sid|$(pwd -P)|$role" >"$FAKE_DIR/agents/$$"
echo "START pid=$$ sid=$sid resumed=$resumed role=$role" >>"$FAKE_DIR/claude.log"
exec -a claude /usr/bin/perl "$FAKE_DIR/tui.pl" claude "$sid" "$transcript" "$FAKE_DIR/claude.log"
FAKE
  fake_exec_warm "$1"
}

# 偽の codex の入口（API を呼ばない）。本物と同じく `$CODEX_HOME/thread-writer-locks/<ID>.lock` を開いたまま持ち
# （tako は lsof でこれを読んで会話 ID を知る）、rollout（`sessions/…/rollout-…-<ID>.jsonl`）へターンを書く。
# argv[0] を codex にする（tako は実行ファイル名で codex を見分ける）
write_fake_codex() {
  cat >"$1" <<'FAKE'
#!/bin/bash
case "${1:-}" in
  --version | -V) echo "codex-cli 0.154.0"; exit 0 ;;
esac
sid=""
if [ "${1:-}" = resume ]; then
  for a in "$@"; do sid="$a"; done
fi
resumed=no
[ -n "$sid" ] && resumed=yes
[ -n "$sid" ] || sid=$(printf '%08x-2792-4000-8000-%012x' "$$" "$(date +%s)")
d="$CODEX_HOME/sessions/2026/11/08"
mkdir -p "$d" "$CODEX_HOME/thread-writer-locks"
rollout="$d/rollout-2026-11-08T00-00-00-$sid.jsonl"
[ -f "$rollout" ] || : >"$rollout"
echo "START pid=$$ sid=$sid resumed=$resumed" >>"$FAKE_DIR/codex.log"
exec -a codex /usr/bin/perl "$FAKE_DIR/tui.pl" codex "$sid" "$rollout" "$FAKE_DIR/codex.log" "$CODEX_HOME/thread-writer-locks/$sid.lock"
FAKE
  fake_exec_warm "$1"
}

# 偽の claude の最小 TUI（perl。実行権は要らない = perl へ渡して読ませる）
write_fake_tui() {
  cat >"$1" <<'TUI'
# alt screen に入り、最下行に「❯ 」の入力欄を描く。打たれた文字を入力欄へ出し、Enter で 1 件の依頼として
# 会話の記録へ書き（claude = transcript の user / assistant・codex = rollout の task_started / response_item /
# task_complete）、決まった報告を画面へ出して入力欄を空へ戻す
# （tako の送達フローが「貼り付けが入力欄へ映った → Enter で空へ戻った = 送信された」と読む形）
use strict;
use warnings;
my ($kind, $sid, $transcript, $log, $lockpath) = @ARGV;
# codex は会話の書き手のロックを生きている間ずっと開いたまま持つ（tako は lsof でこれを読む）
my $lock;
open($lock, '>>', $lockpath) if defined $lockpath;
$| = 1;
system('/bin/stty', 'raw', '-echo');
my @lines = ("FAKE-" . uc($kind) . "-2792 $sid");
my $input = '';
my $turn = 0;
sub draw {
    my @tail = @lines > 12 ? @lines[-12 .. -1] : @lines;
    print "\e[2J\e[H";
    print "$_\r\n" for @tail;
    print "\xe2\x94\x80" x 30, "\r\n";
    print "\xe2\x9d\xaf $input";
}
sub jstr {
    my ($s) = @_;
    $s =~ s/\\/\\\\/g;
    $s =~ s/"/\\"/g;
    $s =~ s/([\x00-\x1f])/sprintf('\\u%04x', ord $1)/ge;
    return "\"$s\"";
}
sub append {
    my ($path, $text) = @_;
    open my $f, '>>', $path or return;
    print $f $text;
    close $f;
}
# 既に記録がある会話（--resume）はターンの番号を続ける
if (open my $t, '<', $transcript) {
    while (<$t>) { $turn++ if /"type":"assistant"/ || /"type":"task_complete"/; }
    close $t;
}
print "\e[?1049h\e[?2004h";
draw();
while (1) {
    my $n = sysread(STDIN, my $c, 1);
    last unless $n;
    if ($c eq "\e") {
        # CSI（貼り付けの印 200~ / 201~・矢印）は捨てる
        sysread(STDIN, my $b, 1) or last;
        if ($b eq '[') {
            while (sysread(STDIN, my $d, 1)) { last if $d =~ /[\x40-\x7e]/; }
        }
        next;
    }
    if ($c eq "\r" || $c eq "\n") {
        next if $input eq '';
        $turn++;
        my $text = $input;
        $input = '';
        append($log, "SUBMIT role=" . ($ENV{TAKO_ORCHESTRATOR_ROLE} // '') . " sid=$sid turn=$turn text=$text\n");
        my $reply = "REPORT-2792 turn=$turn sid=$sid";
        if ($kind eq 'codex') {
            append($transcript, '{"type":"event_msg","payload":{"type":"task_started","turn_id":"t' . $turn . "\"}}\n");
            append($transcript, '{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":'
                . jstr($reply) . "}]}}\n");
            append($transcript, '{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t' . $turn
                . '","last_agent_message":' . jstr($reply) . "}}\n");
        } else {
            append($transcript, '{"type":"user","sessionId":"' . $sid . '","message":{"role":"user","content":' . jstr($text) . "}}\n");
            append($transcript, '{"type":"assistant","sessionId":"' . $sid . '","requestId":"req-' . $turn
                . '","message":{"role":"assistant","content":[{"type":"text","text":' . jstr($reply) . "}]}}\n");
        }
        push @lines, "> $text", $reply;
        draw();
        next;
    }
    if ($c eq "\x7f" || $c eq "\x08") { chop $input; draw(); next; }
    if ($c eq "\x03") { $input = ''; draw(); next; }
    next if ord($c) < 0x20;
    $input .= $c;
    draw();
}
TUI
}

# 腕ごとの砂場
setup_sandbox() {
  local arm="$1"
  TMP="$(mktemp -d /tmp/t2792-${arm}-XXXXXX)"
  TMP="$(cd "$TMP" && pwd -P)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t2792-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm} ==="
  GHOME="$TMP/home" P1="$TMP/p1" BIN="$TMP/bin" RESP="$TMP/responses" CH="$TMP/codex-home"
  FAKE_DIR="$TMP/fake"
  mkdir -p "$GHOME" "$P1" "$BIN" "$RESP" "$CH" "$FAKE_DIR/agents"
  write_fake_claude "$BIN/claude"
  write_fake_codex "$BIN/codex"
  : >"$CH/config.toml"
  write_fake_tui "$FAKE_DIR/tui.pl"
  # ペインのシェルで案内の `tako orchestrator resume` を打てるように（ペインは GUI の隔離の env を継ぐ）
  ln -s "$TAKO_BIN" "$BIN/tako"
  : >"$FAKE_DIR/claude.log"
  : >"$FAKE_DIR/codex.log"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  export TAKO_ISOLATED=1
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  iso_cli orchestrator projects add --key p1 --cwd "$P1" --description "#2792 の検証" >/dev/null 2>&1
  # codex の spawn に #981 の移行がサンドボックス解除を足さないよう、本番のプロファイルと同じく明示する
  printf '%s\n' 'bypass_sandbox: false' >"$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml"
}

# GUI を起こす（腕の env）。起こせなければ UNMEASURED=1 で 1
start_gui() {
  local log="$1" arm="$2"
  local -a arm_env
  if [ "$arm" = none ]; then
    # tmux を入れていない機: PATH にも既知の場所にも無い（HOME も一時 dir = ログインシェルも知らない）
    arm_env=("TAKO_2151_INJECT=skip-known-paths:tmux" "PATH=$BIN:/usr/bin:/bin:/usr/sbin:/sbin")
  else
    arm_env=("PATH=$BIN:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
  fi
  # 起動待ち（#2413）はメモリの圧で spawn を待たせる = 器と無関係の揺れなので旧挙動（その場で起こす）
  if ! launch_isolated_gui "$log" "HOME=$GHOME" "SHELL=/bin/zsh" "TAKO_PERSIST=1" \
    "FAKE_DIR=$FAKE_DIR" "CODEX_HOME=$CH" "TAKO_2413_LEGACY=1" ${arm_env[@]+"${arm_env[@]}"}; then
    echo "  未実測: 隔離 GUI を起こせない（tako-vd が描画できない = 蓋閉じ等）"
    UNMEASURED=1
    return 1
  fi
  if ! wait_isolated_gui "$log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"
    ISOLATED_GUI_PID=""
    echo "  未実測: 隔離 GUI が CLI に応答しない"
    UNMEASURED=1
    return 1
  fi
  return 0
}

stop_gui() {
  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
}

# 置き場のパスを伏せて 1 行に縮める
mask() { sed "s#$TMP#<tmp>#g" | tr '\n' ' ' | cut -c1-"${1:-240}"; }

# 偽物の記録の行数（正規表現。worker の行は下の WSTART / WSUB）
count_log() { grep -c "$1" "$FAKE_DIR/claude.log" 2>/dev/null || true; }

# 条件が真になるまで待つ（$1 = 秒・残りは評価する式）
wait_until() {
  local secs="$1" i
  shift
  for i in $(seq 1 $((secs * 2))); do
    eval "$*" && return 0
    sleep 0.5
  done
  return 1
}

# 生きている偽 claude の pid（agents の置き場）。$1 = 役割の接頭辞（空なら全部）
fake_pids() {
  local f role want="${1:-}"
  for f in "$FAKE_DIR"/agents/*; do
    [ -f "$f" ] || continue
    kill -0 "${f##*/}" 2>/dev/null || continue
    IFS='|' read -r _ _ role <"$f"
    case "$role" in "$want"*) echo "${f##*/}" ;; esac
  done
  return 0
}

# worker / master の役割（ペインの TAKO_ORCHESTRATOR_ROLE の接頭辞。worker は `worker:<プロジェクト>:<ラベル>`）と、
# 偽物の記録の worker の行
W=worker
M=master
WSTART="^START .*role=$W"
WSUB="^SUBMIT role=$W"

# layout.json に保存された会話 ID
saved_id() {
  jq -r '[.. | objects | .claude_session_id? | strings] | .[0] // empty' "$TAKO_DATA_DIR/layout.json" 2>/dev/null
}

# worker の役割を持つペイン（GUI の再起動の後はペイン ID を引き直す）
worker_pane() {
  iso_cli list 2>/dev/null |
    jq -r '[.tabs[].panes[] | select((.role // "") | startswith("orchestrator-worker"))] | .[0].id // empty'
}

# 状態照会（応答の全文は置き場へ・要点を 1 行）
status_of() {
  local pane="$1" tag="$2" out
  out="$(iso_cli orchestrator status --pane "$pane" 2>&1)"
  printf '%s\n' "$out" >"$RESP/status-$tag.json"
  STATUS_JSON="$out"
  echo "    status($tag): $(jq -c '{status, status_source, resolved_session_id, has_running_children, history_lines: (.history.lines // null), prompt_delivery, resume_command, error: (.error.kind // null)}' <<<"$out" 2>/dev/null | mask 400)"
  [ -n "$(jq -r '.status // empty' <<<"$out" 2>/dev/null)" ] || echo "      応答: $(printf '%s' "$out" | mask 300)"
}

# 報告（応答の全文は置き場へ・要点を 1 行）
report_of() {
  local pane="$1" tag="$2" out
  out="$(iso_cli orchestrator report --pane "$pane" 2>&1)"
  printf '%s\n' "$out" >"$RESP/report-$tag.json"
  REPORT_JSON="$out"
  echo "    report($tag): $(jq -c '{source, source_fallback, transcript_agent, session_id, text: (.text // "" | .[0:80]), scrollback: (if .scrollback_text then "あり" else "なし" end)}' <<<"$out" 2>/dev/null | mask 400)"
  [ -n "$(jq -r '.source // empty' <<<"$out" 2>/dev/null)" ] || echo "      応答: $(printf '%s' "$out" | mask 300)"
}

# watch（上限つき。出力の先頭 3 行）
watch_of() {
  local pane="$1" tag="$2" secs="$3" out rc=0
  out="$(iso_cli orchestrator watch --pane "$pane" --timeout "$secs" 2>&1)" || rc=$?
  printf '%s\n' "$out" >"$RESP/watch-$tag.txt"
  WATCH_OUT="$out"
  echo "    watch($tag, rc=$rc): $(printf '%s' "$out" | head -3 | mask 300)"
}

# codex の worker の spawn → watch → status → report → close（③'）。器なしで report が読めないのは
# docs の表に書いた欠け（codex の会話 ID は器のペインの pid からしか引かない = 画面も rollout も読めない）
codex_steps() {
  local arm="$1" master="$2" resp cpane src
  resp="$(iso_cli orchestrator spawn --project p1 --pane "$master" --label c2792 --agent codex \
    --prompt 'T2792 codex task: reply with the fixed report' 2>&1)"
  printf '%s\n' "$resp" >"$RESP/spawn-codex.json"
  cpane="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  echo "  spawn(codex): $(jq -c '{pane_id, worker_id, tmux_session, agent}' <<<"$resp" 2>/dev/null | mask 300)"
  if [ -z "$cpane" ]; then
    bad "[${arm}] codex の spawn が通らない: $(printf '%s' "$resp" | mask 300)"
    return
  fi
  wait_until 150 '[ "$(grep -c "^SUBMIT" "$FAKE_DIR/codex.log" 2>/dev/null || true)" -ge 1 ]'
  check "$([ "$(grep -c '^SUBMIT' "$FAKE_DIR/codex.log" 2>/dev/null || true)" -ge 1 ] && echo 1 || echo 0)" \
    "[${arm}] codex の worker へ依頼文が届いた"
  watch_of "$cpane" codex 120
  check "$(case "$WATCH_OUT" in *WORKER_IDLE*) echo 1 ;; *) echo 0 ;; esac)" "[${arm}] codex の watch が WORKER_IDLE を返す"
  status_of "$cpane" codex
  src="$(jq -r '.status_source // empty' <<<"$STATUS_JSON" 2>/dev/null)"
  report_of "$cpane" codex
  if [ "$arm" = tmux ]; then
    check "$([ "$src" = codex-session ] && echo 1 || echo 0)" "[tmux] codex の status は rollout（codex-session）で決まる（${src:-なし}）"
    check "$(case "$(jq -r '.text // empty' <<<"$REPORT_JSON")" in *REPORT-2792*) echo 1 ;; *) echo 0 ;; esac)" \
      "[tmux] codex の report が worker の報告を返す（source=$(jq -r '.source // "なし"' <<<"$REPORT_JSON")）"
  else
    check "$([ "$src" = screen ] && echo 1 || echo 0)" "[none] codex の status は画面の推定だけ（${src:-なし}。rollout を引けない = 欠け）"
    check "$([ -z "$(jq -r '.source // empty' <<<"$REPORT_JSON" 2>/dev/null)" ] && echo 1 || echo 0)" \
      "[none] codex の report は読めない（欠け: $(printf '%s' "$REPORT_JSON" | mask 120)）"
  fi
  iso_cli close --pane "$cpane" >/dev/null 2>&1
}

run_arm() {
  local arm="$1" master resp wpane wid sid pid1 pid2 starts submits i
  setup_sandbox "$arm"

  # ---- ① spawn ----
  start_gui "$TMP/app1.log" "$arm" || return
  if [ "$(iso_cli list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
  echo "  persist: $(iso_cli persist 2>&1 | jq -c '{enabled, available, backend, reason}' 2>/dev/null | mask 300)"
  sleep 3
  master="$(iso_cli list | jq -r '.tabs[0].panes[0].id')"
  # master は本番と同じく最初のペインで `tako master` を打って起こす（spawn 元 = master の役割のペイン）
  iso_cli send --pane "$master" 'tako master' >/dev/null 2>&1
  wait_until 90 '[ -n "$(fake_pids "$M")" ]'
  echo "  master: $(grep "^START .*role=$M" "$FAKE_DIR/claude.log" | head -1 | mask 160)"
  check "$([ -n "$(fake_pids "$M")" ] && echo 1 || echo 0)" "[${arm}] tako master が master の claude を起こした（pane ${master}）"
  resp="$(iso_cli orchestrator spawn --project p1 --pane "$master" --label w2792 \
    --prompt 'T2792 first task: reply with the fixed report' 2>&1)"
  printf '%s\n' "$resp" >"$RESP/spawn.json"
  wpane="$(jq -r '.pane_id // empty' <<<"$resp" 2>/dev/null)"
  wid="$(jq -r '.worker_id // empty' <<<"$resp" 2>/dev/null)"
  echo "  spawn: $(jq -c '{pane_id, worker_id, tmux_session, agent, cwd}' <<<"$resp" 2>/dev/null | mask 300)"
  if [ -z "$wpane" ]; then
    bad "[${arm}] spawn が通らない: $(printf '%s' "$resp" | mask 300)"
    stop_gui
    return
  fi
  check 1 "[${arm}] spawn が worker のペインを作った（pane ${wpane} / worker ${wid:-なし}）"
  wait_until 150 '[ "$(count_log "$WSUB")" -ge 1 ]'
  check "$([ "$(count_log "$WSUB")" -ge 1 ] && echo 1 || echo 0)" \
    "[${arm}] spawn の依頼文が worker の入力欄から送信された（偽物の記録: $(grep "$WSUB" "$FAKE_DIR/claude.log" | head -1 | mask 120)）"
  pid1="$(fake_pids "$W" | head -1)"

  # ---- ② watch / status / report ----
  watch_of "$wpane" first 60
  check "$(case "$WATCH_OUT" in *WORKER_IDLE*) echo 1 ;; *) echo 0 ;; esac)" "[${arm}] watch が WORKER_IDLE を返す"
  wait_until 60 '[ -n "$(iso_cli orchestrator status --pane "$wpane" 2>/dev/null | jq -r ".resolved_session_id // empty")" ]'
  status_of "$wpane" first
  sid="$(jq -r '.resolved_session_id // empty' <<<"$STATUS_JSON" 2>/dev/null)"
  check "$([ -n "$sid" ] && echo 1 || echo 0)" "[${arm}] status が会話 ID を解決した（${sid:-なし}）"
  report_of "$wpane" first
  check "$(case "$(jq -r '.text // empty' <<<"$REPORT_JSON")" in *REPORT-2792*) echo 1 ;; *) echo 0 ;; esac)" \
    "[${arm}] report が worker の報告（REPORT-2792）を返す（source=$(jq -r '.source // "なし"' <<<"$REPORT_JSON")）"

  # ---- ③ 追加の指示 ----
  resp="$(iso_cli send --pane "$wpane" 'T2792 second instruction' 2>&1)"
  printf '%s\n' "$resp" >"$RESP/send.json"
  echo "    send: $(printf "%s" "$resp" | mask 200)"
  wait_until 60 '[ "$(count_log "$WSUB")" -ge 2 ]'
  check "$([ "$(count_log "$WSUB")" -ge 2 ] && echo 1 || echo 0)" "[${arm}] send の指示が worker の入力欄から送信された"

  # ---- ③' codex の worker ----
  codex_steps "$arm" "$master"

  # ---- ④ 境界 1: agent の突然死 ----
  echo "  境界 1: worker の agent（pid ${pid1:-なし}）を KILL"
  starts="$(count_log "$WSTART")"
  [ -n "$pid1" ] && kill -KILL "$pid1" 2>/dev/null
  wait_until 20 '[ -z "$(fake_pids "$W")" ]'
  sleep 2
  watch_of "$wpane" dead 40
  check "$(case "$WATCH_OUT" in *WORKER_DEAD*) echo 1 ;; *) echo 0 ;; esac)" "[${arm}] 突然死の後の watch が WORKER_DEAD を返す"
  status_of "$wpane" dead
  local rcmd
  rcmd="$(jq -r '.resume_command // empty' <<<"$STATUS_JSON" 2>/dev/null)"
  [ -n "$rcmd" ] || rcmd="$(printf '%s\n' "$WATCH_OUT" | sed -n 's/^  resume: //p' | head -1)"
  check "$([ -n "$rcmd" ] && echo 1 || echo 0)" "[${arm}] 復旧の案内（resume_command）が出る: ${rcmd:-なし}"
  if [ -n "$rcmd" ]; then
    resp="$(iso_cli send --pane "$wpane" "$rcmd" 2>&1)"
    wait_until 60 '[ "$(count_log "$WSTART")" -gt "$starts" ]'
    echo "    再開: $(grep "$WSTART" "$FAKE_DIR/claude.log" | tail -1 | mask 160)"
    check "$(grep "$WSTART" "$FAKE_DIR/claude.log" | tail -1 | grep -c "sid=${sid} resumed=yes" | tr -d ' ')" \
      "[${arm}] 案内を打つと同じ会話（${sid:-なし}）で起き直す"
    wait_until 30 '[ -n "$(fake_pids "$W")" ]'
    report_of "$wpane" after-resume
    check "$(case "$(jq -r '.text // empty' <<<"$REPORT_JSON")" in *REPORT-2792*) echo 1 ;; *) echo 0 ;; esac)" \
      "[${arm}] 起き直した後も report が同じ会話の報告を返す"
  fi

  # ---- ⑤ 境界 2: GUI の再起動 ----
  wait_until 90 '[ -n "$(saved_id)" ]'
  pid2="$(fake_pids "$W" | head -1)"
  local mpid2 mpid3
  mpid2="$(fake_pids "$M" | head -1)"
  starts="$(count_log "$WSTART")"
  echo "  境界 2: GUI を止めて起こし直す（worker の agent pid ${pid2:-なし}・layout.json の会話 ID $(saved_id)）"
  stop_gui
  sleep 2
  local survived=0
  [ -n "$pid2" ] && kill -0 "$pid2" 2>/dev/null && survived=1
  echo "    GUI 停止中の worker の agent: $([ "$survived" = 1 ] && echo 生存 || echo 終了)"
  if [ "$arm" = tmux ]; then
    check "$survived" "[tmux] GUI を止めても worker の agent が器の中で生き続ける"
  else
    check "$([ "$survived" = 0 ] && echo 1 || echo 0)" "[none] GUI を止めると worker の agent も終わる（器が無い = 想定どおりの欠け）"
  fi
  start_gui "$TMP/app2.log" "$arm" || return
  wait_until 120 '[ -n "$(worker_pane)" ] && [ -n "$(fake_pids "$W")" ] && [ -n "$(fake_pids "$M")" ]'
  sleep 3
  wpane="$(worker_pane)"
  local pid3
  pid3="$(fake_pids "$W" | head -1)"
  mpid3="$(fake_pids "$M" | head -1)"
  # 最後の行は変数へ取ってから見る（pipefail の下で早く抜ける読み手へパイプしない）
  local mlast wlast
  mlast="$(grep "^START .*role=$M" "$FAKE_DIR/claude.log" | tail -1)"
  wlast="$(grep "$WSTART" "$FAKE_DIR/claude.log" | tail -1)"
  echo "    再起動後の master: agent pid ${mpid3:-なし}（再起動前 ${mpid2:-なし}）・$(printf '%s' "$mlast" | mask 160)"
  if [ "$arm" = tmux ]; then
    check "$([ -n "$mpid3" ] && [ "$mpid3" = "$mpid2" ] && echo 1 || echo 0)" "[tmux] master も同じプロセスに繋ぎ直す"
  else
    check "$([ -n "$mpid3" ] && [ "$mpid3" != "${mpid2:-}" ] && [[ "$mlast" == *resumed=yes* ]] && echo 1 || echo 0)" \
      "[none] master も新しいプロセスが同じ会話（--resume）で起き直す"
  fi
  echo "    再起動後の worker のペイン: ${wpane:-なし}・agent pid ${pid3:-なし}・偽物の起動 $(count_log "$WSTART") 回（再起動前 ${starts}）"
  grep -E '復元の内訳|\[restore\]' "$TAKO_DATA_DIR/persist.log" 2>/dev/null | tail -2 | mask 300 | sed 's/^/    persist.log: /'
  echo
  if [ "$arm" = tmux ]; then
    check "$([ -n "$pid3" ] && [ "$pid3" = "$pid2" ] && echo 1 || echo 0)" \
      "[tmux] 再起動の後も同じプロセス（pid ${pid2:-?}）に繋ぎ直す（実行中のターンも途切れない）"
  else
    check "$([ -n "$pid3" ] && [ "$pid3" != "${pid2:-}" ] && [[ "$wlast" == *"sid=${sid} resumed=yes"* ]] && echo 1 || echo 0)" \
      "[none] 再起動の後は新しいプロセスが同じ会話（--resume ${sid:-?}）で起き直す（実行中だったターンは途切れる）"
  fi
  if [ -n "$wpane" ]; then
    status_of "$wpane" after-restart
    report_of "$wpane" after-restart
    check "$(case "$(jq -r '.text // empty' <<<"$REPORT_JSON")" in *REPORT-2792*) echo 1 ;; *) echo 0 ;; esac)" \
      "[${arm}] GUI の再起動の後も report が同じ会話の報告を返す"
  else
    bad "[${arm}] 再起動の後に worker のペインが見つからない"
  fi
  iso_cli orchestrator workers >"$RESP/workers-after-restart.json" 2>&1
  echo "    workers（再起動後）: $(jq -c '[.workers[]? | {worker_id, pane, status, tmux_session, session_id}]' "$RESP/workers-after-restart.json" 2>/dev/null | mask 300)"

  # ---- ⑥ close ----
  if [ -n "$wpane" ]; then
    local crc=0
    resp="$(iso_cli close --pane "$wpane" 2>&1)" || crc=$?
    printf "%s\n" "$resp" >"$RESP/close.json"
    echo "    close（rc=${crc}）: $(printf "%s" "$resp" | mask 200)"
    wait_until 20 '[ -z "$(fake_pids "$W")" ]'
    local left
    left="$(fake_pids "$W")"
    check "$([ -z "$(worker_pane)" ] && echo 1 || echo 0)" "[${arm}] close で worker のペインが GUI から消える"
    if [ -z "$left" ]; then
      pass "[${arm}] close で worker の agent が終わる"
    elif [ "$arm" = none ]; then
      # 器なしのペインを閉じる経路の既知の不具合（#2803）。偽 claude は bash の入口から perl へ exec するので
      # 当たる（前景ジョブが bash のときに起きる）。器の有無で決まる欠けではないので数えずに名指す
      KNOWN=$((KNOWN + 1))
      echo "  [既知 #2803] [none] close の 20 秒後も agent が終了処理の途中で残る（GUI が PTY の親側を閉じない）:"
      ps -o pid=,ppid=,stat=,command= -p "$left" 2>/dev/null | mask 200 | sed "s/^/      /"
      echo
    else
      bad "[${arm}] close で worker の agent が終わる（残り: ${left}）"
    fi
    iso_cli orchestrator workers --all >"$RESP/workers-after-close.json" 2>&1
    local closed
    closed="$(jq -r --arg w "$wid" '[.workers[]? | select((.worker_id|tostring) == $w)] | .[0] | "\(.status) \(.close_reason // "")"' "$RESP/workers-after-close.json" 2>/dev/null)"
    echo "    workers --all（close 後）: ${closed:-なし}"
    check "$(case "$closed" in closed*) echo 1 ;; *) echo 0 ;; esac)" "[${arm}] close の後のレジストリは closed（${closed:-なし}）"
  fi

  # 器なしの腕は器を 1 度も起こしていない（隔離ソケットにサーバーが居ない）
  if [ "$arm" = none ]; then
    check "$(tmux -L "$sock" ls >/dev/null 2>&1 && echo 0 || echo 1)" "[none] 器（tmux サーバー）は 1 度も起きていない"
  fi
  stop_gui
  tmux_socket_kill "$sock"
}

for arm in $ARMS; do
  case "$arm" in
    none | tmux) run_arm "$arm" ;;
    *) echo "知らない腕: $arm"; exit 2 ;;
  esac
done

echo
echo "PASS=${PASS} FAIL=${FAIL} 既知の不具合=${KNOWN}$([ "$UNMEASURED" = 1 ] && echo '（未実測の腕あり）')"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 4
exit 0
