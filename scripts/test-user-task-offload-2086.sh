#!/bin/bash
# test-user-task-offload-2086.sh — tasks の残りの action が UI スレッドで置き場を待たないか（#2086）
#
# #2052 で `tako todo list` と完了・却下は background へ出たが、起票・詳細・更新・返答（PWA からの
# 返答を含む）・展開は UI スレッドの同期の dispatch で `user-tasks.yaml` を読み書きしていた。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定 / user-tasks.yaml には触らない）:
#   ① 置き場の読みを注入（`TAKO_2031_INJECT=slow:<ミリ秒>`。書き込みも中で読むので遅れる。
#      CPU は焼かない）したまま、`tako todo show` / `update` / `add` / `respond --via pwa`
#      （PWA の受け口が送るのと同じ要求）/ `expand` の最中に軽い IPC（`tako scrollback`）の
#      応答時間を測る（IPC の受け口は UI スレッド = UI が止まれば応答も止まる）。
#      同じバイナリの A/B（`TAKO_2086_LEGACY=1` = 修正前）と並べ、perf.log（`TAKO_PERF_VERBOSE=1`）から
#      UI スレッドの置き場の読み（`user_tasks:read`）の回数も並べる
#   ② エッジ（新しい腕）: 無い id への返答 / 壊れた置き場での起票 / 無い id の展開 が理由つきで失敗し、
#      その最中も IPC は止まらない
#   ③ 順序（新しい腕）: 生きている master（スタブの claude）へ、続けて 2 本の返答を送る
#      （1 本目の積みが注入で遅れている最中に 2 本目が届く）。返答の並び・master の入力欄へ届く順が
#      送った順のまま・1 本ずつ届く（二重に届かない）
#
# 使い方: bash scripts/test-user-task-offload-2086.sh
#   STAGES="latency edge order"（既定は全部）で段を選ぶ。INJECT_MS（既定 1500）で読みの遅さを変える。
#   SKIP_BUILD=1 でビルドを飛ばす（先に `cargo build -p tako-cli -p tako-app` を済ませておく）。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 自動命名は `TAKO_AUTO_RENAME=0` で止める（隔離 GUI でも claude を呼ぶ = #1968 の罠）。
# tmux は使わない（`TAKO_PERSIST=0`）。落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_BACKEND_SOCKET

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INJECT_MS="${INJECT_MS:-1500}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-2086-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "tako-app / tako をビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet) || exit 1
fi
isolated_gui_bins || exit 1

# claude のスタブ（③ の master。入力欄 `❯` を出し、Enter で受け取った行を画面へ残す）
STUB_BIN="$TMP/bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/claude" <<'STUB'
#!/usr/bin/env bash
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

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERSIST=0
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR"
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
ms_since() { perl -e "printf \"%d\", ($(now) - $1) * 1000"; }
json_get() { perl -MJSON::PP -e 'local $/; my $j = eval { decode_json(<STDIN>) } // {}; my $v = $j; for my $k (split /\./, $ARGV[0]) { $v = ref $v eq "ARRAY" ? $v->[$k] : ref $v eq "HASH" ? $v->{$k} : undef } print defined $v ? $v : ""' "$1"; }
squash() { printf '%s' "$1" | tr -d ' \t\r\n'; }

# 腕ごとにデータ・置き場・perf.log を分ける（前の腕の一覧や分布を持ち越さない）
arm_env() {
  local arm="$1"
  mkdir -p "$TMP/data-$arm" "$TMP/orch-$arm"
  for d in "$TMP/data-$arm" "$TMP/orch-$arm"; do
    case "$d" in
      "$TMP"/*) : ;;
      *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
    esac
  done
  echo "TAKO_DATA_DIR=$TMP/data-$arm TAKO_ORCHESTRATOR_DIR=$TMP/orch-$arm \
TAKO_USER_TASKS_FILE=$TMP/orch-$arm/user-tasks.yaml TAKO_PERF_VERBOSE=1 \
TAKO_PERF_LOG=$TMP/perf-$arm.log TAKO_AUTO_RENAME=0 TAKO_2031_INJECT=slow:$INJECT_MS"
}

# perf.log の分布行（`span 統計 [タグ] count=N …`）から、タグの count の和
span_count() { grep -F "span 統計 [$2] " "$1" 2>/dev/null | sed -E 's/.*count=([0-9]+).*/\1/' | awk '{s+=$1} END{print s+0}'; }

STAGES="${STAGES:-latency edge order}"
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
  local arm="$1"
  shift
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/gui-$arm.log" $(arm_env "$arm") ${1+"$@"} || exit $?
  if ! wait_isolated_gui "$TMP/gui-$arm.log"; then
    fail "[$arm] GUI が立たない"
    stop_isolated_gui "$ISOLATED_GUI_PID"
    return 1
  fi
  return 0
}

add_task() {
  "$TAKO_BIN" todo add "$1" --json 2>/dev/null | json_get id
}

# 1 つの操作の最中の IPC の応答を測り、`<名前>_MAX_<腕>` / `<名前>_OVER_<腕>` / `<名前>_RC_<腕>` へ置く
measure_op() {
  local arm="$1" name="$2"
  shift 2
  probe_while 1 "$@"
  eval "${name}_MAX_$arm=$PROBE_MAX ${name}_OVER_$arm=$PROBE_OVER ${name}_RC_$arm=$BG_RC"
  echo "    [$arm] $name の最中: tako scrollback ${PROBE_N} 回・最大 ${PROBE_MAX} ms・500 ms 以上 ${PROBE_OVER} 回（$name の所要 ${BG_MS} ms・rc=${BG_RC}）"
}

OPS="show update add respond expand"

measure_latency() {
  local arm="$1"
  shift
  start_arm "$arm" ${1+"$@"} || return 1
  # 起票は測る前に済ませる。右パネルは開かない（2 秒ループの一覧更新を混ぜない）
  local a b
  a=$(add_task "#2086 の実測 A")
  b=$(add_task "#2086 の実測 B")
  if [ -z "$a" ] || [ -z "$b" ]; then
    fail "[$arm] 測る前の起票が通らない"
    stop_isolated_gui "$ISOLATED_GUI_PID"
    return 1
  fi
  local perf="$TMP/perf-$arm.log" mark
  sleep 11
  mark=$(wc -l < "$perf" 2>/dev/null | tr -d ' ')
  mark=${mark:-0}

  measure_op "$arm" show "$TAKO_BIN" todo show "$a" --json
  measure_op "$arm" update "$TAKO_BIN" todo update "$a" --title "#2086 の実測 A（直した）"
  measure_op "$arm" add "$TAKO_BIN" todo add "#2086 の実測 C"
  # PWA の受け口（`POST /api/tasks/<id>/respond`）が GUI へ送るのと同じ要求（via=pwa）
  measure_op "$arm" respond "$TAKO_BIN" todo respond "$b" --decision approve --comment "実測" --via pwa
  # 右パネルは閉じたまま = 一覧に居ない行を開く（修正前はここで UI スレッドが一覧を引き直す）
  measure_op "$arm" expand "$TAKO_BIN" todo expand "$a"

  # 書いたものが置き場に載っている（腕で結果が変わらない）
  local title responses expanded
  title=$("$TAKO_BIN" todo show "$a" --json 2>/dev/null | json_get title)
  responses=$("$TAKO_BIN" todo show "$b" --json 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = decode_json(<STDIN>); print scalar @{$j->{responses}}')
  expanded=$("$TAKO_BIN" todo list --json 2>/dev/null | json_get expanded)
  eval "TITLE_$arm=\$title RESPONSES_$arm=\$responses EXPANDED_$arm=\$expanded"
  echo "    [$arm] 結果: 題=${title} / 返答 ${responses} 件 / 展開=${expanded}（期待 ${a}）"
  eval "WANT_EXPANDED_$arm=$a"

  sleep 11
  stop_isolated_gui "$ISOLATED_GUI_PID"
  tail -n +"$((mark + 1))" "$perf" > "$TMP/perf-$arm.window.log" 2>/dev/null
  local win="$TMP/perf-$arm.window.log"
  echo "    [$arm] perf.log（測定区間）: 置き場の読み UI スレッド $(span_count "$win" user_tasks:read) 回・background $(span_count "$win" bg:user_tasks:read) 回"
  eval "UI_READS_$arm=$(span_count "$win" user_tasks:read)"
}

if has_stage latency; then
  echo "① 実 CLI: 各操作の最中の IPC の応答時間（注入: 置き場の読み ${INJECT_MS} ms・同じバイナリの A/B）"
  measure_latency cnew
  measure_latency clegacy TAKO_2086_LEGACY=1
  for op in $OPS; do
    new_max=$(eval "echo \${${op}_MAX_cnew:-99999}")
    old_max=$(eval "echo \${${op}_MAX_clegacy:-0}")
    old_over=$(eval "echo \${${op}_OVER_clegacy:-0}")
    new_rc=$(eval "echo \${${op}_RC_cnew:-?}")
    if [ "$new_max" -lt 500 ] && [ "$new_rc" = "0" ]; then
      pass "$op: 新は最中の IPC が最大 ${new_max} ms（rc=0）・旧は最大 ${old_max} ms（500 ms 以上 ${old_over} 回）"
    else
      fail "$op: 新でも IPC が止まる / 失敗した（最大 ${new_max} ms・rc=${new_rc}）"
    fi
    if [ "$old_over" -lt 1 ]; then
      fail "$op: 旧でも IPC が止まらない（注入が効いていない = 測り方に検出力が無い。最大 ${old_max} ms）"
    fi
  done
  if [ "${TITLE_cnew:-}" = "${TITLE_clegacy:-x}" ] && [ "${RESPONSES_cnew:-}" = "1" ] && [ "${RESPONSES_clegacy:-}" = "1" ]; then
    pass "書いた結果は両方の腕で同じ（題=${TITLE_cnew}・返答 1 件）"
  else
    fail "書いた結果が腕で違う（題 新=${TITLE_cnew:-?} 旧=${TITLE_clegacy:-?} / 返答 新=${RESPONSES_cnew:-?} 旧=${RESPONSES_clegacy:-?}）"
  fi
  if [ "${EXPANDED_cnew:-}" = "${WANT_EXPANDED_cnew:-x}" ] && [ "${EXPANDED_clegacy:-}" = "${WANT_EXPANDED_clegacy:-x}" ]; then
    pass "AI からの展開は両方の腕で開いている（expanded=${EXPANDED_cnew}）"
  else
    fail "展開が開いていない（新 ${EXPANDED_cnew:-?} / 旧 ${EXPANDED_clegacy:-?}）"
  fi
  if [ "${UI_READS_cnew:-1}" -eq 0 ] && [ "${UI_READS_clegacy:-0}" -ge 1 ]; then
    pass "UI スレッドで置き場を読んだ回数（測定区間）: 新 0 回 / 旧 ${UI_READS_clegacy} 回"
  else
    fail "UI スレッドの読みの回数が想定と違う（新 ${UI_READS_cnew:-?} 回 / 旧 ${UI_READS_clegacy:-?} 回）"
  fi
fi

if has_stage edge; then
  echo "② エッジ（新しい腕・注入 ${INJECT_MS} ms）"
  start_arm edge || exit 1
  id=$(add_task "#2086 のエッジ")
  probe_while 1 "$TAKO_BIN" todo respond "u-2086-none" --decision approve --via pwa
  if [ "$BG_RC" != "0" ] && grep -q "見つからない" "$TMP/bg.out" && [ "$PROBE_MAX" -lt 500 ]; then
    pass "無い id への返答は理由つきで失敗し（rc=${BG_RC}）、その最中も IPC は止まらない（最大 ${PROBE_MAX} ms）"
  else
    fail "無い id への返答の扱いが想定と違う（rc=${BG_RC}・最大 ${PROBE_MAX} ms: $(head -c 200 "$TMP/bg.out")）"
  fi
  # 失敗した返答の札が手放されている = 次の返答は待たされない
  t0=$(now)
  out=$("$TAKO_BIN" todo respond "$id" --decision approve --via pwa --json 2>&1)
  took=$(ms_since "$t0")
  if [ "$(printf '%s' "$out" | json_get responses.0.decision)" = "approve" ] && [ "$took" -lt 10000 ]; then
    pass "失敗した返答の後の返答は待たされずに積まれる（${took} ms）"
  else
    fail "失敗した返答の後の返答が通らない / 待たされた（${took} ms: $(printf '%s' "$out" | head -c 200)）"
  fi
  out=$("$TAKO_BIN" todo expand "u-2086-none" 2>&1)
  rc=$?
  if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "見つからない"; then
    pass "無い id の展開は理由つきで失敗する（rc=${rc}）"
  else
    fail "無い id の展開が失敗にならない（rc=${rc}: ${out}）"
  fi
  store="$TMP/orch-edge/user-tasks.yaml"
  cp "$store" "$TMP/store.bak"
  printf 'tasks: [ {{{ 壊れた\n' > "$store"
  probe_while 1 "$TAKO_BIN" todo add "#2086 の壊れた置き場"
  if [ "$BG_RC" != "0" ] && grep -q "パースに失敗" "$TMP/bg.out" && [ "$PROBE_MAX" -lt 500 ]; then
    pass "壊れた置き場での起票は 0 件へ上書きせず失敗し、その最中も IPC は止まらない（最大 ${PROBE_MAX} ms）"
  else
    fail "壊れた置き場での起票の扱いが想定と違う（rc=${BG_RC}・最大 ${PROBE_MAX} ms: $(head -c 200 "$TMP/bg.out")）"
  fi
  if grep -q "壊れた" "$store"; then
    pass "壊れた置き場は書き換えられていない（中身を消していない）"
  else
    fail "壊れた置き場が書き換えられた"
  fi
  cp "$TMP/store.bak" "$store"
  stop_isolated_gui "$ISOLATED_GUI_PID"
fi

if has_stage order; then
  echo "③ 順序（新しい腕・注入 ${INJECT_MS} ms）: 続けて 2 本の返答を生きている master へ"
  start_arm order || exit 1
  # 呼び出し元ペインが無い（env を外した）ので、分割元は一覧から取る
  root_pane=$("$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = decode_json(<STDIN>); for my $t (@{$j->{tabs}}) { for my $p (@{$t->{panes} // []}) { print $p->{id}; exit } }')
  master=$("$TAKO_BIN" split --pane "$root_pane" 2>&1 | tr -dc '0-9')
  [ -n "$master" ] || { fail "master のペインを作れない（root=${root_pane}）"; exit 1; }
  "$TAKO_BIN" title --pane "$master" --role "orchestrator-master:t2086" >/dev/null 2>&1
  "$TAKO_BIN" send --pane "$master" "PATH=$STUB_BIN:\$PATH claude --stub" >/dev/null 2>&1
  for _ in $(seq 1 300); do
    case "$(squash "$("$TAKO_BIN" read --pane "$master" --lines 40 2>/dev/null)")" in
      *STUB_CLAUDE_STARTED*) break ;;
    esac
    sleep 0.2
  done
  oid=$(TAKO_ORCHESTRATOR_ROLE="master:t2086" "$TAKO_BIN" todo add "#2086 の順序" --json 2>/dev/null | json_get id)
  # 1 本目の積みが注入で遅れている最中に 2 本目を届ける
  ( "$TAKO_BIN" todo respond "$oid" --decision needs_change --comment "ひとつめ" --via pwa >"$TMP/r1.out" 2>&1 ) &
  r1=$!
  sleep 0.3
  ( "$TAKO_BIN" todo respond "$oid" --decision answered --comment "ふたつめ" --via pwa >"$TMP/r2.out" 2>&1 ) &
  r2=$!
  wait "$r1" "$r2"
  comments=$("$TAKO_BIN" todo show "$oid" --json 2>/dev/null | perl -MJSON::PP -e 'local $/; my $j = decode_json(<STDIN>); print join(",", map { $_->{comment} } @{$j->{responses}})')
  if [ "$comments" = "ひとつめ,ふたつめ" ]; then
    pass "返答の並びは送った順（${comments}）"
  else
    fail "返答の並びが送った順でない（${comments}）"
  fi
  screen=""
  for _ in $(seq 1 200); do
    screen=$("$TAKO_BIN" read --pane "$master" --lines 120 2>/dev/null)
    case "$(squash "$screen")" in
      *ふたつめ*) break ;;
    esac
    sleep 0.2
  done
  flat=$(squash "$screen")
  first_at=$(perl -e 'print index($ARGV[0], "ひとつめ")' "$flat")
  second_at=$(perl -e 'print index($ARGV[0], "ふたつめ")' "$flat")
  n1=$(printf '%s' "$flat" | perl -ne 'my $n = () = /コメント:ひとつめ/g; print $n')
  n2=$(printf '%s' "$flat" | perl -ne 'my $n = () = /コメント:ふたつめ/g; print $n')
  if [ "$first_at" -ge 0 ] && [ "$second_at" -gt "$first_at" ]; then
    pass "master の入力欄へ届く順が送った順（ひとつめ @${first_at} → ふたつめ @${second_at}）"
  else
    fail "master の入力欄へ届く順が違う / 届いていない（ひとつめ @${first_at} / ふたつめ @${second_at}）"
  fi
  if [ "$n1" = "1" ] && [ "$n2" = "1" ]; then
    pass "1 本につき 1 回だけ届く（ひとつめ ${n1} 回・ふたつめ ${n2} 回）"
  else
    fail "二重に届いた / 届いていない（ひとつめ ${n1} 回・ふたつめ ${n2} 回）"
  fi
  stop_isolated_gui "$ISOLATED_GUI_PID"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
