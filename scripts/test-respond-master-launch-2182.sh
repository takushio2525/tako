#!/bin/bash
# test-respond-master-launch-2182.sh — タスクへの返答の配送で master を起こす組み立てが UI スレッドを止めないか（#2182）
#
# 起票した master がもう居ない返答は、配送で新しい master を起こす。その組み立て
# （`master_launch::plan` = CLI の所在確認のログインシェル `$SHELL -l -c 'command -v claude'`・上限 15 秒 +
# tako の実行ファイルの解決）を、修正前は UI スレッド（`land_offload` の ③ 運ぶ）で待っていた。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / user-tasks.yaml には触らない）:
#   ① 戻らないログインシェル（`$SHELL` をスタブへ差し替え、`command -v claude` のときだけ戻らない）の
#      注入で、`tako todo respond --via pwa`（PWA の受け口が送るのと同じ要求）の最中に軽い IPC
#      （`tako scrollback`）の応答時間を測る（IPC の受け口は UI スレッド = UI が止まれば応答も止まる）。
#      同じバイナリの A/B（`TAKO_2182_LEGACY=1` = 修正前）と並べ、perf.log（`TAKO_PERF_VERBOSE=1`）から
#      組み立てを UI スレッドで組んだ回数（`user_task_launch_plan`）と background の回数も並べる
#   ② エッジ（新しい腕）: 組み立てが打ち切られたら空のタブを残さず失敗として記録される /
#      組み立てを待つ間に届いた 2 本目の返答は番を待ち（IPC は止まらない）、届いた順に積まれる /
#      起票元が解けない返答は起こそうとしない
#   ③ 配送（両方の腕）: ログインシェルが遅い（3 秒で答える）とき、返答で master が新しいタブに起き、
#      スタブの claude の入力欄へ返答が届く（従来どおり）。その最中も新は IPC が止まらない
#   ④ visual 段（両方の腕）: visual-test 節 `tasks-launch-2182`（右パネルの「返す」の入口
#      `user_task_respond`）を戻らないログインシェルの下で回す。新は返している間も UI スレッドの往復が
#      上限（300 ms）未満で緑、旧（`TAKO_2182_LEGACY=1`）は ① で落ちる。所在確認の上限は 5 秒へ縮める
#
# 使い方: bash scripts/test-respond-master-launch-2182.sh
#   STAGES="latency edge deliver visual"（既定は全部）で段を選ぶ。SKIP_BUILD=1 でビルドを飛ばす
#   （先に `cargo build -p tako-cli -p tako-app --features tako-app/visual-test` を済ませておく。
#   visual-test 版は通常の起動もそのままできるので 1 回のビルドで全段に使う）。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 自動命名は `TAKO_AUTO_RENAME=0` で止める。tmux は使わない（`TAKO_PERSIST=0`）。
# 本物の claude は起こさない: PATH から claude を持つ dir を外し、HOME を一時 dir にする（既知の
# 設置先は HOME の下だけを見る）。master のペインの zsh はスタブの claude を先に引く。
# 落とすのは自分で起こした pid だけ（戻らないスタブの sleep も自分の pid だけを記録して落とす）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_BACKEND_SOCKET

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-2182-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 戻らないスタブ（`exec sleep`）は上限で落とされるはずだが、残っていれば自分の pid だけを落とす
  if [ -f "$TMP/hang.pids" ]; then
    while read -r pid; do
      kill "$pid" 2>/dev/null
    done < "$TMP/hang.pids"
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "visual-test 版の tako-app / tako をビルドします…"
  (cd "$REPO_ROOT" && run_heavy cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
fi
isolated_gui_bins || exit 1

STUB_BIN="$TMP/bin"
mkdir -p "$STUB_BIN"
# claude のスタブ（master。入力欄 `❯` を出し、Enter で受け取った行を画面へ残す）。
# 版や一覧を聞かれたら（GUI の裏の確認）すぐ答えて終わる
cat > "$STUB_BIN/claude" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  --version) echo "0.0.0 (stub)"; exit 0 ;;
  agents) echo "[]"; exit 0 ;;
esac
echo "STUB_CLAUDE_STARTED"
stty raw -echo 2>/dev/null
printf '\r\n❯ '
buf=""
while IFS= read -r -n1 ch; do
  if [ -z "$ch" ]; then
    printf '\r\n[STUB_SUBMITTED] %s\r\n❯ ' "$buf"
    buf=""
  else
    buf="$buf$ch"
    printf '%s' "$ch"
  fi
done
STUB
chmod +x "$STUB_BIN/claude"

# `$SHELL` のスタブ。CLI の所在確認（`-l -c 'command -v claude'`）のときだけ差し替え、
# 残り（ペインのシェル・git / tmux の所在確認）は zsh へそのまま渡す。
# 振る舞いは GUI の env `T2182_SHELL_MODE` で選ぶ（hang = 戻らない / slow = 3 秒で答える）
cat > "$STUB_BIN/stubshell" <<STUB
#!/bin/bash
case "\$*" in
  *"command -v claude"*)
    echo "\$\$" >> "$TMP/hang.pids"
    case "\${T2182_SHELL_MODE:-}" in
      hang) exec sleep 600 ;;
      slow) sleep 3; echo "$STUB_BIN/claude"; exit 0 ;;
    esac
    ;;
esac
exec /bin/zsh "\$@"
STUB
chmod +x "$STUB_BIN/stubshell"

# PATH から claude を持つ dir を外す（本物の claude を GUI の裏でもペインでも起こさない）
clean_path=""
old_ifs=$IFS
IFS=:
for d in $PATH; do
  [ -n "$d" ] || continue
  [ -x "$d/claude" ] && continue
  clean_path="${clean_path:+$clean_path:}$d"
done
IFS=$old_ifs
export PATH="$clean_path"

export HOME="$TMP/home"
export SHELL="$STUB_BIN/stubshell"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERSIST=0
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR"
# ペインの zsh: 固定のプロンプト（実ユーザー名・ホスト名を写さない = #927）+ スタブの claude を先に引く
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" "export PATH=\"$STUB_BIN:\$PATH\"" > "$HOME/.zshrc"

if command -v claude >/dev/null 2>&1; then
  echo "PATH を絞っても claude が引ける（本物を起こしうるので止める）: $(command -v claude)"
  exit 1
fi

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
ms_since() { perl -e "printf \"%d\", ($(now) - $1) * 1000"; }
json_get() { perl -MJSON::PP -e 'binmode STDOUT, ":encoding(UTF-8)"; local $/; my $j = eval { decode_json(<STDIN>) } // {}; my $v = $j; for my $k (split /\./, $ARGV[0]) { $v = ref $v eq "ARRAY" ? $v->[$k] : ref $v eq "HASH" ? $v->{$k} : undef } print defined $v ? $v : ""' "$1"; }
squash() { printf '%s' "$1" | tr -d ' \t\r\n'; }
# `tako list` のタブの数 / master の role（`orchestrator-master`）を持つペイン
tab_count() { "$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = eval { decode_json(<STDIN>) } // {}; print scalar @{$j->{tabs} // []}'; }
master_pane() { "$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = eval { decode_json(<STDIN>) } // {}; for my $t (@{$j->{tabs} // []}) { for my $p (@{$t->{panes} // []}) { if (($p->{role} // "") eq "orchestrator-master") { print $p->{id}; exit } } }'; }

arm_env() {
  local arm="$1" mode="$2"
  mkdir -p "$TMP/data-$arm" "$TMP/orch-$arm"
  for d in "$TMP/data-$arm" "$TMP/orch-$arm"; do
    case "$d" in
      "$TMP"/*) : ;;
      *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
    esac
  done
  echo "TAKO_DATA_DIR=$TMP/data-$arm TAKO_ORCHESTRATOR_DIR=$TMP/orch-$arm \
TAKO_USER_TASKS_FILE=$TMP/orch-$arm/user-tasks.yaml TAKO_PERF_VERBOSE=1 \
TAKO_PERF_LOG=$TMP/perf-$arm.log TAKO_AUTO_RENAME=0 T2182_SHELL_MODE=$mode"
}

# perf.log の分布行（`span 統計 [タグ] count=N …`）から、タグの count の和
span_count() { grep -F "span 統計 [$2] " "$1" 2>/dev/null | sed -E 's/.*count=([0-9]+).*/\1/' | awk '{s+=$1} END{print s+0}'; }

STAGES="${STAGES:-latency edge deliver visual}"
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# 背景で `cmd` を走らせ、終わるまで（と最低 min_s 秒）`tako scrollback` の応答時間を測る。
# 結果は PROBE_MAX / PROBE_OVER（500 ms 以上の回数）/ PROBE_N と、背景の所要 BG_MS・終了コード BG_RC
probe_while() {
  local min_s="$1"
  shift
  local t0 start lat bg_pid t_min
  t0=$(now)
  ( "$@" >"$TMP/bg.out" 2>&1; echo "$?" >"$TMP/bg.rc"; ms_since "$t0" >"$TMP/bg.ms" ) &
  bg_pid=$!
  PROBE_MAX=0 PROBE_OVER=0 PROBE_N=0
  t_min=$(perl -e "printf \"%.3f\", $t0 + $min_s")
  sleep 0.1
  while kill -0 "$bg_pid" 2>/dev/null || perl -e "exit !($(now) < $t_min)"; do
    start=$(now)
    "$TAKO_BIN" scrollback >/dev/null 2>&1
    lat=$(ms_since "$start")
    PROBE_N=$((PROBE_N + 1))
    [ "$lat" -gt "$PROBE_MAX" ] && PROBE_MAX=$lat
    [ "$lat" -ge 500 ] && PROBE_OVER=$((PROBE_OVER + 1))
    sleep 0.1
  done
  wait "$bg_pid" 2>/dev/null
  BG_MS=$(cat "$TMP/bg.ms" 2>/dev/null || echo "?")
  BG_RC=$(cat "$TMP/bg.rc" 2>/dev/null || echo "?")
}

start_arm() {
  local arm="$1" mode="$2"
  shift 2
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/gui-$arm.log" $(arm_env "$arm" "$mode") ${1+"$@"} || exit $?
  if ! wait_isolated_gui "$TMP/gui-$arm.log"; then
    fail "[$arm] GUI が立たない"
    stop_isolated_gui "$ISOLATED_GUI_PID"
    return 1
  fi
  return 0
}

# 起票 master（default プロファイル）として起票する。その master のペインは居ない = 返答で起こす
add_master_task() {
  TAKO_ORCHESTRATOR_ROLE=master "$TAKO_BIN" todo add "$1" --json 2>/dev/null | json_get id
}

measure_latency() {
  local arm="$1"
  shift
  start_arm "$arm" hang ${1+"$@"} || return 1
  local id profile tabs_before tabs_after
  id=$(add_master_task "#2182 の実測")
  profile=$("$TAKO_BIN" todo show "$id" --json 2>/dev/null | json_get origin.profile)
  if [ -z "$id" ] || [ "$profile" != "default" ]; then
    fail "[$arm] 測る前の起票が通らない（id=${id} 起票元=${profile}）"
    stop_isolated_gui "$ISOLATED_GUI_PID"
    return 1
  fi
  tabs_before=$(tab_count)
  local perf="$TMP/perf-$arm.log" mark
  sleep 11
  mark=$(wc -l < "$perf" 2>/dev/null | tr -d ' ')
  mark=${mark:-0}

  # PWA の受け口（`POST /api/tasks/<id>/respond`）が GUI へ送るのと同じ要求（via=pwa）
  probe_while 1 "$TAKO_BIN" todo respond "$id" --decision approve --comment "起こして" --via pwa --json
  echo "    [$arm] respond の最中: tako scrollback ${PROBE_N} 回・最大 ${PROBE_MAX} ms・500 ms 以上 ${PROBE_OVER} 回（respond の所要 ${BG_MS} ms・rc=${BG_RC}）"
  eval "MAX_$arm=$PROBE_MAX OVER_$arm=$PROBE_OVER RC_$arm=$BG_RC MS_$arm=$BG_MS"
  local state reason
  state=$("$TAKO_BIN" todo show "$id" --json 2>/dev/null | json_get delivery.state)
  reason=$("$TAKO_BIN" todo show "$id" --json 2>/dev/null | json_get delivery.reason)
  tabs_after=$(tab_count)
  echo "    [$arm] 配送: state=${state} / 理由=${reason} / タブ ${tabs_before} → ${tabs_after}"
  eval "STATE_$arm=\$state REASON_$arm=\$reason TABS_BEFORE_$arm=$tabs_before TABS_AFTER_$arm=$tabs_after"

  sleep 11
  stop_isolated_gui "$ISOLATED_GUI_PID"
  tail -n +"$((mark + 1))" "$perf" > "$TMP/perf-$arm.window.log" 2>/dev/null
  local win="$TMP/perf-$arm.window.log" ui bg
  ui=$(span_count "$win" user_task_launch_plan)
  bg=$(span_count "$win" bg:user_task_launch_plan)
  echo "    [$arm] perf.log（測定区間）: 組み立て UI スレッド ${ui} 回・background ${bg} 回"
  eval "UI_PLANS_$arm=$ui BG_PLANS_$arm=$bg"
}

if has_stage latency; then
  echo "① 戻らないログインシェル: 返答の配送（master を起こす経路）の最中の IPC の応答時間（同じバイナリの A/B）"
  measure_latency cnew
  measure_latency clegacy TAKO_2182_LEGACY=1
  if [ "${MAX_cnew:-99999}" -lt 500 ] && [ "${RC_cnew:-?}" = "0" ]; then
    pass "新は配送の最中も IPC が最大 ${MAX_cnew} ms（respond の所要 ${MS_cnew:-?} ms・rc=0）・旧は最大 ${MAX_clegacy:-?} ms（500 ms 以上 ${OVER_clegacy:-?} 回）"
  else
    fail "新でも IPC が止まる / 返答が失敗した（最大 ${MAX_cnew:-?} ms・rc=${RC_cnew:-?}）"
  fi
  if [ "${OVER_clegacy:-0}" -lt 1 ] || [ "${MAX_clegacy:-0}" -lt 10000 ]; then
    fail "旧でも IPC が上限まで止まらない（注入が効いていない = 測り方に検出力が無い。最大 ${MAX_clegacy:-?} ms）"
  else
    pass "旧は所在確認の上限まで UI が止まる（最大 ${MAX_clegacy} ms）"
  fi
  if [ "${UI_PLANS_cnew:-1}" -eq 0 ] && [ "${BG_PLANS_cnew:-0}" -ge 1 ] && [ "${UI_PLANS_clegacy:-0}" -ge 1 ]; then
    pass "組み立てを組んだスレッド（測定区間）: 新 UI 0 回・background ${BG_PLANS_cnew} 回 / 旧 UI ${UI_PLANS_clegacy} 回"
  else
    fail "組み立てのスレッドが想定と違う（新 UI ${UI_PLANS_cnew:-?}・bg ${BG_PLANS_cnew:-?} / 旧 UI ${UI_PLANS_clegacy:-?}）"
  fi
  for arm in cnew clegacy; do
    state=$(eval "echo \${STATE_$arm:-}")
    reason=$(eval "echo \${REASON_$arm:-}")
    before=$(eval "echo \${TABS_BEFORE_$arm:-x}")
    after=$(eval "echo \${TABS_AFTER_$arm:-y}")
    if [ "$state" = "failed" ] && grep -q "master を起動できない" <<<"$reason" && [ "$before" = "$after" ]; then
      pass "[$arm] 組み立てが打ち切られた返答は空のタブを残さず失敗として記録される（タブ ${before} → ${after}）"
    else
      fail "[$arm] 打ち切りの記録が想定と違う（state=${state}・理由=${reason}・タブ ${before} → ${after}）"
    fi
  done
fi

if has_stage edge; then
  echo "② エッジ（新しい腕・戻らないログインシェル）"
  start_arm edge hang || exit 1
  id=$(add_master_task "#2182 のエッジ")
  # 1 本目の組み立てを待っている間に 2 本目が届く: 2 本目は番を待つ（UI は待たない）
  ( "$TAKO_BIN" todo respond "$id" --decision needs_change --comment "ひとつめ" --via pwa >"$TMP/r1.out" 2>&1 ) &
  r1=$!
  sleep 1
  probe_while 1 "$TAKO_BIN" todo respond "$id" --decision answered --comment "ふたつめ" --via pwa
  wait "$r1"
  comments=$("$TAKO_BIN" todo show "$id" --json 2>/dev/null | perl -MJSON::PP -e 'binmode STDOUT, ":encoding(UTF-8)"; local $/; my $j = decode_json(<STDIN>); print join(",", map { $_->{comment} } @{$j->{responses}})')
  if [ "$PROBE_MAX" -lt 500 ] && [ "$BG_RC" = "0" ] && [ "${BG_MS%%[!0-9]*}" -ge 20000 ]; then
    pass "組み立ての最中に届いた 2 本目は 1 本目の後に通り（所要 ${BG_MS} ms）、その間も IPC は止まらない（最大 ${PROBE_MAX} ms）"
  else
    fail "2 本目の扱いが想定と違う（IPC 最大 ${PROBE_MAX} ms・所要 ${BG_MS} ms・rc=${BG_RC}: $(head -c 200 "$TMP/bg.out")）"
  fi
  if [ "$comments" = "ひとつめ,ふたつめ" ]; then
    pass "返答の並びは送った順（${comments}）"
  else
    fail "返答の並びが送った順でない（${comments}）"
  fi
  # 起票元が解けない返答（role なしの起票）は起こそうとしない（組み立てを待たない）
  oid=$("$TAKO_BIN" todo add "#2182 の宛先なし" --json 2>/dev/null | json_get id)
  t0=$(now)
  out=$("$TAKO_BIN" todo respond "$oid" --decision approve --via pwa --json 2>&1)
  took=$(ms_since "$t0")
  if [ "$(printf '%s' "$out" | json_get delivery.state)" = "failed" ] \
    && grep -q "宛先不明" <<<"$(printf '%s' "$out" | json_get delivery.reason)" \
    && [ "$took" -lt 5000 ]; then
    pass "起票元が解けない返答は起こそうとせず宛先不明で記録される（${took} ms）"
  else
    fail "起票元が解けない返答の扱いが想定と違う（${took} ms: $(printf '%s' "$out" | head -c 200)）"
  fi
  stop_isolated_gui "$ISOLATED_GUI_PID"
fi

deliver_arm() {
  local arm="$1"
  shift
  start_arm "$arm" slow ${1+"$@"} || return 1
  local id tabs_before master screen n
  id=$(add_master_task "#2182 の配送")
  tabs_before=$(tab_count)
  probe_while 1 "$TAKO_BIN" todo respond "$id" --decision approve --comment "とどけ2182" --via pwa
  echo "    [$arm] respond の最中: tako scrollback ${PROBE_N} 回・最大 ${PROBE_MAX} ms（respond の所要 ${BG_MS} ms・rc=${BG_RC}）"
  eval "DMAX_$arm=$PROBE_MAX DRC_$arm=$BG_RC"
  master=$(master_pane)
  screen=""
  for _ in $(seq 1 200); do
    [ -n "$master" ] || master=$(master_pane)
    if [ -n "$master" ]; then
      screen=$("$TAKO_BIN" read --pane "$master" --lines 120 2>/dev/null)
      case "$(squash "$screen")" in
        *STUB_SUBMITTED*とどけ2182*) break ;;
      esac
    fi
    sleep 0.2
  done
  printf '%s\n' "$screen" > "$TMP/deliver-$arm-screen.txt"
  n=$(grep -c '\[STUB_SUBMITTED\].*とどけ2182' "$TMP/deliver-$arm-screen.txt")
  local state
  state=$("$TAKO_BIN" todo show "$id" --json 2>/dev/null | json_get delivery.state)
  echo "    [$arm] master のペイン=${master:-なし}・タブ ${tabs_before} → $(tab_count)・届いた回数 ${n}・state=${state}"
  eval "DPANE_$arm=\${master:-} DN_$arm=$n DSTATE_$arm=\$state DTABS_$arm=$(( $(tab_count) - tabs_before ))"
  stop_isolated_gui "$ISOLATED_GUI_PID"
}

if has_stage deliver; then
  echo "③ 遅いログインシェル（3 秒で答える）: 返答で master が起き、返答が届く（両方の腕）"
  deliver_arm dnew
  deliver_arm dlegacy TAKO_2182_LEGACY=1
  for arm in dnew dlegacy; do
    pane=$(eval "echo \${DPANE_$arm:-}")
    n=$(eval "echo \${DN_$arm:-0}")
    state=$(eval "echo \${DSTATE_$arm:-}")
    tabs=$(eval "echo \${DTABS_$arm:-0}")
    if [ -n "$pane" ] && [ "$n" = "1" ] && [ "$tabs" = "1" ] && { [ "$state" = "launched" ] || [ "$state" = "delivered" ]; }; then
      pass "[$arm] master が新しいタブに起き（ペイン ${pane}）、返答が入力欄へ 1 回届く（state=${state}）"
    else
      fail "[$arm] 配送が想定と違う（ペイン=${pane}・届いた回数 ${n}・タブ +${tabs}・state=${state}）"
    fi
  done
  if [ "${DMAX_dnew:-99999}" -lt 500 ] && [ "${DRC_dnew:-?}" = "0" ]; then
    pass "新は遅いログインシェルの組み立ての最中も IPC が最大 ${DMAX_dnew} ms（旧は ${DMAX_dlegacy:-?} ms）"
  else
    fail "新で組み立ての最中に IPC が止まった（最大 ${DMAX_dnew:-?} ms・rc=${DRC_dnew:-?}）"
  fi
fi

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local arm="$1" log="$2"
  shift 2
  # shellcheck disable=SC2046
  launch_isolated_gui "$log" $(arm_env "$arm" hang) TAKO_SETUP_PROBE_TIMEOUT_SECS=5 \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tasks-launch-2182 ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  return 0
}

if has_stage visual; then
  echo "④ visual-test tasks-launch-2182（右パネルの「返す」・戻らないログインシェル・上限 5 秒）"
  run_section vnew "$TMP/vnew.log"
  grep -E "TAKO_VISUAL_2182" "$TMP/vnew.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vnew.log"; then
    pass "visual 新: 返している間も UI スレッドが止まらず、打ち切りは失敗として記録される（TAKO_VISUAL_TEST_OK）"
  else
    fail "visual 新: 節が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$TMP/vnew.log" | tail -5 | sed 's/^/    /'
  fi
  run_section vlegacy "$TMP/vlegacy.log" TAKO_2182_LEGACY=1
  grep -E "TAKO_VISUAL_2182" "$TMP/vlegacy.log" | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: ① 右パネルの「返す」が master の起動の組み立てを UI スレッドで待たない" "$TMP/vlegacy.log"; then
    pass "visual 旧（TAKO_2182_LEGACY=1）は ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/vlegacy.log" | head -1)"
  else
    fail "visual 旧でも ① で落ちない（節に検出力が無い）"
    grep -E "FAILED|panicked" "$TMP/vlegacy.log" | tail -3 | sed 's/^/    /'
  fi
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
