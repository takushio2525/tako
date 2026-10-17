#!/bin/bash
# test-sessions-write-2020.sh — sessions.yaml の書き直しの回数とバイト数を新旧で数える（#2020）
#
# 使い方:
#   bash scripts/test-sessions-write-2020.sh [分]      # 既定 30 分（本番の 1 時間の半分）
#
# 何を見るか:
#   隔離 GUI を 2 本（新 = 既定 / 旧 = TAKO_2020_LEGACY=1）同時に tako-vd 上へ起こし、
#   どちらにも本番と同じ大きさのカタログ（500 件・約 395 KB）と、偽の claude を 24 本
#   （器つきのペイン 8 枚 × 3 本 = 本番の検出数と同じ）を置いて、GUI の走査（30 秒ごと）に任せる。
#   途中で会話の終わり・始まりを 4 回起こす（検出集合の変化 = 即時に書く側の経路）。
#
#   ① sessions.yaml / .bak.1 の書き直しの回数とバイト数（inode の変化を 0.5 秒ごとに数える）
#   ② 並び順（last_seen_at の新しい順）・GC で残る集合が新旧で同じ
#   ③ SIGTERM（= on_app_quit）で閉じたとき、新の last_seen_at が最後の走査まで残る
#   ④ 器ごと落とした（= PC の再起動）後の再起動で、新旧とも同じ数の会話が
#      `claude --resume` で戻る（#1076 / FR-5.9.1。復元はカタログの役割・モデルを引く）
#
# 偽の claude は API を呼ばない。`claude agents --json` は生きている偽の会話（pid と会話 ID）を返し、
# GUI はそれを本番と同じ経路（ログインシェル → pid の祖先辿り → 器のペイン）で拾う。
# ログインシェルが本物の claude（~/.local/bin 等）を拾わないよう HOME を一時 dir へ差し替える。
#
# 本番の data dir・tmux・GUI には触らない（TAKO_ISOLATED + 一時の data / discovery / sessions /
# tmux ソケット）。落とすのは自分で起こした pid と自分のソケットの tmux サーバーだけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE CLAUDE_CONFIG_DIR ZDOTDIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MINUTES="${1:-30}"
DURATION=$((MINUTES * 60))
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/t2020-XXXXXX)"
ARMS="new legacy"
GUI_PIDS=""
POLL_PIDS=""

kill_agents() {
  local f pid
  for f in "$TMP"/fake-*/agents/*; do
    [ -f "$f" ] || continue
    pid=${f##*/}
    kill "$pid" 2>/dev/null
  done
}
cleanup() {
  local p arm
  for p in $POLL_PIDS; do kill "$p" 2>/dev/null; done
  for p in $GUI_PIDS; do stop_isolated_gui "$p"; done
  kill_agents
  for arm in $ARMS; do tmux -L "t2020-$arm-$$" kill-server 2>/dev/null; done
  sleep 6 # 偽の claude の sleep 5 が抜けるのを待つ（孤児を残さない）
  [ -n "${KEEP_TMP:-}" ] && { echo "一時 dir を残した: $TMP"; return; }
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
isolated_gui_bins || exit 1
command -v jq >/dev/null || { echo "jq が要る"; exit 1; }

export HOME="$TMP/home"
export TAKO_ISOLATED=1
mkdir -p "$HOME/.claude/projects/t2020" "$TMP/bin"
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"
case "$HOME" in "$TMP"/*) : ;; *) echo "HOME が隔離されていない"; exit 1 ;; esac
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}

# ---- 偽の claude --------------------------------------------------------------
cat > "$TMP/bin/claude" <<'FAKE'
#!/bin/bash
# 偽の claude（#2020 の実測用。API を呼ばない）
case "${1:-}" in
  agents)
    printf '['
    sep=''
    for f in "$FAKE_DIR"/agents/*; do
      [ -f "$f" ] || continue
      pid=${f##*/}
      kill -0 "$pid" 2>/dev/null || { rm -f "$f"; continue; }
      printf '%s{"pid":%s,"sessionId":"%s","cwd":"%s","model":"claude-opus-5","status":"idle","kind":"interactive"}' \
        "$sep" "$pid" "$(cat "$f")" "$FAKE_DIR"
      sep=','
    done
    printf ']\n'
    exit 0
    ;;
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
esac
sid="${FAKE_SESSION_ID:-}"
prev=""
for a in "$@"; do
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
echo "$(date +%s) ROLE=${TAKO_ORCHESTRATOR_ROLE:-} ARGS=$*" >> "$FAKE_DIR/launch.log"
[ -n "$sid" ] || exit 0
mkdir -p "$FAKE_DIR/agents"
echo "$sid" > "$FAKE_DIR/agents/$$"
trap 'rm -f "$FAKE_DIR/agents/$$"; exit 0' TERM INT HUP
echo "FAKECLAUDE-2020 $sid"
while :; do sleep 5; done
FAKE
chmod +x "$TMP/bin/claude"

# 会話 ID（UUID の形）。ペイン p の k 本目 / 途中で始まる会話は p=9
sid_of() { printf '%08x-2020-4000-8000-%012x' "$1" "$2"; }
for p in 1 2 3 4 5 6 7 8 9; do
  for k in 1 2 3 4 5; do
    # 復元（--resume）は会話の記録が実在するときだけ起こす（#652）
    echo '{}' > "$HOME/.claude/projects/t2020/$(sid_of "$p" "$k").jsonl"
  done
done

# ---- 本番と同じ大きさのカタログ（500 件・約 395 KB。値はすべて架空） -------------------
seed_catalog() {
  local out="$1" now i
  now=$(date -u +%s)
  {
    echo "entries:"
    for i in $(seq 1 500); do
      printf '  00000000-0000-4000-8000-%012d:\n' "$i"
      printf '    kind: worker\n    label: w-%d\n    project: proj%d\n    agent: claude\n' "$i" $((i % 9))
      printf '    model: claude-opus-5\n    effort: high\n    issues:\n    - %d\n' $((1000 + i))
      printf '    prompt_head: %s\n' "$(printf '架空の依頼文 %.0s' $(seq 1 21))"
      printf '    cwd: /tmp/t2020/work%d\n    tab: %d\n    pane: %d\n' $((i % 9)) $((i % 12)) $((100 + i))
      printf '    tmux_session: tako-iso-%d\n    log_file: /tmp/t2020/pane-logs/pane-%d.log\n' "$i" "$i"
      printf '    started_at: %s\n' "$(date -u -r $((now - 864000 - i * 600)) +%Y-%m-%dT%H:%M:%SZ)"
      printf '    last_seen_at: %s\n' "$(date -u -r $((now - 86400 - i * 600)) +%Y-%m-%dT%H:%M:%SZ)"
    done
  } > "$out"
}

# ---- 腕ごとの env と CLI ------------------------------------------------------
arm_env() { # 腕の GUI / CLI に渡す env（VAR=VAL を 1 行 1 つ）
  local arm="$1"
  echo "TAKO_DATA_DIR=$TMP/data-$arm"
  echo "TAKO_DISCOVERY_DIR=$TMP/disc-$arm"
  echo "TAKO_ORCHESTRATOR_DIR=$TMP/orch-$arm"
  echo "TAKO_SESSIONS_FILE=$TMP/sessions-$arm.yaml"
  echo "TAKO_PANE_LOG_DIR=$TMP/panelogs-$arm"
  echo "TAKO_WORKERS_FILE=$TMP/workers-$arm.yaml"
  echo "TAKO_TMUX_SOCKET=t2020-$arm-$$"
  echo "FAKE_DIR=$TMP/fake-$arm"
}
tako_() { # tako_ <腕> <引数…>
  local arm="$1"
  shift
  # shellcheck disable=SC2046
  env $(arm_env "$arm") "$TAKO_BIN" "$@" 2>/dev/null
}
launch_arm() { # launch_arm <腕> <ログ>
  local arm="$1" log="$2" legacy=""
  [ "$arm" = legacy ] && legacy="TAKO_2020_LEGACY=1"
  mkdir -p "$TMP/data-$arm" "$TMP/disc-$arm" "$TMP/orch-$arm" "$TMP/fake-$arm/agents"
  # shellcheck disable=SC2046
  launch_isolated_gui "$log" $(arm_env "$arm") ${legacy:+"$legacy"} \
    TAKO_PERSIST=1 TAKO_AUTO_RENAME=0 "PATH=$TMP/bin:$PATH" || exit $?
  GUI_PIDS="$GUI_PIDS $ISOLATED_GUI_PID"
  eval "GUI_$arm=$ISOLATED_GUI_PID"
  local i
  for i in $(seq 1 300); do
    tako_ "$arm" list >/dev/null && return 0
    sleep 0.2
  done
  echo "腕 $arm の GUI へ繋がらない"
  tail -20 "$log"
  exit 1
}

# 書き直しを数える（inode が変わる = rename / copy で作り直した）
poll_writes() { # poll_writes <腕>
  local arm="$1" f="$TMP/sessions-$arm.yaml" last_main="" last_bak="" m b
  while :; do
    m=$(stat -f '%i %z' "$f" 2>/dev/null)
    b=$(stat -f '%i %z' "$f.bak.1" 2>/dev/null)
    if [ -n "$m" ] && [ "$m" != "$last_main" ]; then
      [ -n "$last_main" ] && echo "$(date +%s) main ${m#* }" >> "$TMP/writes-$arm.log"
      last_main="$m"
    fi
    if [ -n "$b" ] && [ "$b" != "$last_bak" ]; then
      [ -n "$last_bak" ] || [ -n "$last_main" ] && echo "$(date +%s) bak ${b#* }" >> "$TMP/writes-$arm.log"
      last_bak="$b"
    fi
    sleep 0.5
  done
}

# ペイン p に偽の claude を n 本立てる（最後の 1 本は前面 = 再起動後の復元の対象）
start_agents() { # start_agents <腕> <ペイン ID> <p> <k の並び…>
  local arm="$1" pane="$2" p="$3" cmd="" k last=""
  shift 3
  for k in "$@"; do last="$k"; done
  for k in "$@"; do
    if [ "$k" = "$last" ]; then
      cmd="${cmd}FAKE_SESSION_ID=$(sid_of "$p" "$k") $TMP/bin/claude"
    else
      cmd="${cmd}FAKE_SESSION_ID=$(sid_of "$p" "$k") $TMP/bin/claude >/dev/null & "
    fi
  done
  tako_ "$arm" send --pane "$pane" "$cmd" >/dev/null
}
# 会話を終わらせる（偽の claude の pid だけ）
end_agent() { # end_agent <腕> <会話 ID>
  local arm="$1" sid="$2" f
  for f in "$TMP/fake-$arm"/agents/*; do
    [ -f "$f" ] || continue
    [ "$(cat "$f")" = "$sid" ] && kill "${f##*/}" 2>/dev/null
  done
}

ids_by_recency() { # last_seen_at の新しい順（同時刻は ID の昇順）= sorted_entries と同じ並び
  awk '/^  [^ ].*:$/{id=$1; sub(/:$/,"",id)} /^    last_seen_at:/{print $2, id}' "$1" |
    sort -k1,1r -k2,2 | awk '{print $2}'
}
max_seen() { awk '/^    last_seen_at:/{print $2}' "$1" | sort | tail -1; }
count_entries() { grep -c '^  [^ ].*:$' "$1"; }

echo "== #2020: sessions.yaml の書き直しを新旧で ${MINUTES} 分数える（一時 dir: ${TMP}）"
for arm in $ARMS; do
  seed_catalog "$TMP/sessions-$arm.yaml"
done
echo "  種のカタログ: $(count_entries "$TMP/sessions-new.yaml") 件・$(stat -f %z "$TMP/sessions-new.yaml") バイト"

for arm in $ARMS; do
  launch_arm "$arm" "$TMP/app-$arm.log"
  if [ "$(tako_ "$arm" list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない（腕 ${arm}）。中止する。"
    exit 1
  fi
done

# 器つきのペイン 8 枚（2 段 × 4 列。同じペインを割り続けると細くなりすぎる）
split_() { # split_ <腕> <元のペイン> <向き>
  local new
  new=$(tako_ "$1" split --pane "$2" "--$3" | tr -dc '0-9')
  [ -n "$new" ] || { echo "split に失敗（腕 $1）"; exit 1; }
  echo "$new"
}
for arm in $ARMS; do
  p1=$(tako_ "$arm" list | jq '.tabs[0].panes[0].id')
  p2=$(split_ "$arm" "$p1" right)
  p3=$(split_ "$arm" "$p1" down)
  p4=$(split_ "$arm" "$p2" down)
  p5=$(split_ "$arm" "$p1" right)
  p6=$(split_ "$arm" "$p2" right)
  p7=$(split_ "$arm" "$p3" right)
  p8=$(split_ "$arm" "$p4" right)
  eval "PANES_$arm=\"$p1 $p2 $p3 $p4 $p5 $p6 $p7 $p8\""
done
sleep 3
# 書き直しの計測は偽の claude を立てる前から（最初の検出の書き込みも数える）
for arm in $ARMS; do
  : > "$TMP/writes-$arm.log"
  poll_writes "$arm" &
  POLL_PIDS="$POLL_PIDS $!"
  disown $! # 止めたときの「Terminated」を出さない
done
sleep 1
START=$(date +%s)
# 偽の claude を 1 ペイン 3 本（本番の検出 24 本と同じ）
for arm in $ARMS; do
  eval "panes=\$PANES_$arm"
  p=1
  for pane in $panes; do
    start_agents "$arm" "$pane" "$p" 1 2 3
    p=$((p + 1))
  done
done
echo "  偽の claude を立てた（開始 $(date -u -r "$START" +%H:%M:%SZ)）。${MINUTES} 分待つ…"

# 途中の出来事（検出集合の変化）。時刻は全体の 25% / 40% / 60% / 75%
at() { # at <全体に対する % > — その時刻まで待つ
  local t=$((START + DURATION * $1 / 100)) now
  now=$(date +%s)
  [ "$t" -gt "$now" ] && sleep $((t - now))
}
pane_of() { # pane_of <腕> <p>（1 始まり）
  local arm="$1" p="$2"
  eval "set -- \$PANES_$arm"
  shift $((p - 1))
  echo "$1"
}
at 25
for arm in $ARMS; do end_agent "$arm" "$(sid_of 2 3)"; done
echo "  $(date -u +%H:%M:%SZ) ペイン 2 の前面の会話を終えた"
at 40
for arm in $ARMS; do start_agents "$arm" "$(pane_of "$arm" 2)" 2 4; done
echo "  $(date -u +%H:%M:%SZ) ペイン 2 で新しい会話を始めた"
at 60
for arm in $ARMS; do
  for k in 1 2 3; do end_agent "$arm" "$(sid_of 3 "$k")"; done
done
echo "  $(date -u +%H:%M:%SZ) ペイン 3 の会話を 3 本とも終えた"
at 75
for arm in $ARMS; do start_agents "$arm" "$(pane_of "$arm" 3)" 3 4; done
echo "  $(date -u +%H:%M:%SZ) ペイン 3 で新しい会話を始めた"
at 100

# ② 並び順・GC（終了前。走査の途中の状態でも並び順は同じはず）
for arm in $ARMS; do
  tako_ "$arm" sessions list --limit 1000 --json | jq -r '.sessions[].session_id' > "$TMP/order-live-$arm.txt"
done

# ③ SIGTERM（= on_app_quit）で閉じる
QUIT_AT=$(date +%s)
for arm in $ARMS; do
  eval "pid=\$GUI_$arm"
  stop_isolated_gui "$pid"
done
sleep 1
for p in $POLL_PIDS; do kill "$p" 2>/dev/null; done
POLL_PIDS=""

echo
echo "== ① 書き直しの回数とバイト数（${MINUTES} 分・走査は 30 秒ごと）"
for arm in $ARMS; do
  log="$TMP/writes-$arm.log"
  main_n=$(grep -c ' main ' "$log")
  main_b=$(awk '$2=="main"{s+=$3} END{print s+0}' "$log")
  bak_n=$(grep -c ' bak ' "$log")
  bak_b=$(awk '$2=="bak"{s+=$3} END{print s+0}' "$log")
  eval "W_$arm=$main_n; B_$arm=$((main_b + bak_b)); K_$arm=$bak_n"
  printf '  %-6s 本体 %3d 回 / .bak.1 %3d 回 / 計 %d バイト（1 時間換算 %d バイト）\n' \
    "$arm" "$main_n" "$bak_n" $((main_b + bak_b)) $(((main_b + bak_b) * 3600 / DURATION))
done
# 新が書いてよいのは「最初の検出（偽 claude の立ち上がりが走査をまたぐと 2 回）+ 出来事 4 回 +
# 600 秒ごと + 終了 1 回」まで。比で縛ると、短い時間で回したとき（出来事が詰まる）に設計どおりでも落ちる
MAX_NEW=$((2 + 4 + DURATION / 600 + 1))
if [ "$W_new" -gt 0 ] && [ "$W_new" -le "$MAX_NEW" ] && [ "$W_legacy" -ge $((DURATION / 30 * 3 / 4)) ]; then
  pass "新の書き直しは設計の上限以内（新 ${W_new} 回・上限 ${MAX_NEW} 回 / 旧 ${W_legacy} 回 = 走査ごと）"
else
  fail "新が上限を超えて書いた、または旧が走査ごとに書いていない（新 ${W_new} 回・上限 ${MAX_NEW} 回 / 旧 ${W_legacy} 回）"
fi
if [ "$K_new" -le 6 ]; then
  pass ".bak は last_seen_at 以外が変わったときだけ回る（新 ${K_new} 回 / 旧 ${K_legacy} 回）"
else
  fail ".bak を回しすぎ（新 ${K_new} 回 / 旧 ${K_legacy} 回）"
fi

echo
echo "== ② 並び順・GC が新旧で同じ"
if [ -s "$TMP/order-live-new.txt" ] && cmp -s "$TMP/order-live-new.txt" "$TMP/order-live-legacy.txt"; then
  pass "終了前の tako sessions list の並び順が同じ（$(wc -l < "$TMP/order-live-new.txt" | tr -d ' ') 件）"
else
  fail "終了前の並び順が違う（diff: $(diff "$TMP/order-live-new.txt" "$TMP/order-live-legacy.txt" | head -4 | tr '\n' ' ')）"
fi
for arm in $ARMS; do ids_by_recency "$TMP/sessions-$arm.yaml" > "$TMP/order-final-$arm.txt"; done
n_new=$(count_entries "$TMP/sessions-new.yaml")
if cmp -s "$TMP/order-final-new.txt" "$TMP/order-final-legacy.txt" && [ "$n_new" = 500 ]; then
  pass "終了後のファイルの並び順と GC で残った集合が同じ（${n_new} 件 = 上限）"
else
  fail "終了後の並び順 / 件数が違う（新 ${n_new} 件。diff: $(diff "$TMP/order-final-new.txt" "$TMP/order-final-legacy.txt" | head -4 | tr '\n' ' ')）"
fi

echo
echo "== ③ 終了時の書き出し（SIGTERM = on_app_quit）"
for arm in $ARMS; do
  seen=$(max_seen "$TMP/sessions-$arm.yaml")
  lag=$((QUIT_AT - $(date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$seen" +%s)))
  eval "LAG_$arm=$lag"
  echo "  $arm: 最新の last_seen_at = ${seen}（終了の ${lag} 秒前）"
done
if [ "$LAG_new" -le 40 ]; then
  pass "新の last_seen_at が最後の走査まで残る（終了の ${LAG_new} 秒前。旧は ${LAG_legacy} 秒前）"
else
  fail "新の last_seen_at が終了時に書き出されていない（終了の ${LAG_new} 秒前）"
fi

echo
echo "== ④ 器ごと落とした後の再起動で会話が戻る（#1076 / FR-5.9.1）"
kill_agents
for arm in $ARMS; do tmux -L "t2020-$arm-$$" kill-server 2>/dev/null; done
sleep 6
for arm in $ARMS; do
  : > "$TMP/fake-$arm/launch.log"
  launch_arm "$arm" "$TMP/app2-$arm.log"
done
sleep 45
for arm in $ARMS; do
  n=$(grep -c -- '--resume' "$TMP/fake-$arm/launch.log")
  roles=$(grep -- '--resume' "$TMP/fake-$arm/launch.log" | grep -c 'ROLE=[^ ]')
  eval "R_$arm=$n"
  breakdown=$(grep '復元の内訳' "$TMP/data-$arm/persist.log" 2>/dev/null | tail -1 | sed 's/^.*復元の内訳/復元の内訳/' | cut -c1-160)
  echo "  $arm: --resume で起きた会話 ${n} 本（役割 env つき ${roles} 本）/ ${breakdown:-（内訳の行なし）}"
done
if [ "$R_new" -gt 0 ] && [ "$R_new" = "$R_legacy" ]; then
  pass "再起動後に --resume で戻る会話の数が新旧で同じ（${R_new} 本）"
else
  fail "再起動後の resume が新旧で違う（新 ${R_new} 本 / 旧 ${R_legacy} 本）"
fi

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" = 0 ]
