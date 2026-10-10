#!/bin/bash
# test-ui-thread-wait-1979.sh — UI スレッドが子プロセスを上限なしで待って固まらない（#1979）の実経路テスト
#
# 2026-10-10 01:20 の本番ハングは、メインスレッドが worker の状態照会の中で
# `ps -axo pid=,ppid=,command=` の `Command::output()` の `poll` に 10 分止まったもの
# （sample のスタックで特定。#1968 が UI スレッドから外した経路そのもの）。
# `output()` は子が終わってもパイプの書き手が残っていると返らない
# （macOS の std はパイプを `pipe()` → `set_cloexec` の 2 手で作るので、別スレッドが同時に
# 起こした長生きの子へ書き手が漏れうる = #1768 で実測した穴）。
#
# 何を確かめるか（隔離 GUI・tako-vd。本番の tako / 設定には触らない）:
#   ① 本番ハングの経路: 「本物の ps の出力を出して終わるが、孫がパイプを握り続ける ps」を
#      PATH の先頭に置き、worker の状態照会（`tako orchestrator status`）を打つ。
#      修正後は UI が止まらない（別の要求 `tako scrollback` が即座に返る）・偽の ps は一度も
#      起きない（親子表は libproc で引く）
#   ② `tako list` の器の採り直し: 「`list-windows` のときだけ孫がパイプを握る tmux」を
#      `TAKO_TMUX_BIN` に置き、`tako list` を打つ。修正後は UI が止まらず、`tako list` も
#      読み切りの猶予（2 秒）で返る
#   ③ フォーカスの無いペインの再描画が上限（既定 30 fps）以下・フォーカス中のペインは 60 fps のまま
#   ④ 上限を CLI から読み書きできる（範囲外は理由つきで弾く・0 は弾く・極端な値も弾く）
#
# A/B: `TAKO_1979_LEGACY=1 bash scripts/test-ui-thread-wait-1979.sh` で①②が名指しで FAILED
# （修正前の形 = UI スレッドが偽の ps / tmux の孫が終わるまで止まる）。①は本番の版と同じく
# 状態照会の UI スレッド部で子プロセスの有無を数える `TAKO_1968_LEGACY=1` を両方の腕で立てる
# （#1968 で外した経路を開けたうえで、#1979 が経路の型そのものを塞いだことを見る）。
#
# 使い方: bash scripts/test-ui-thread-wait-1979.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY="${TAKO_1979_LEGACY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 「UI が止まっていない」とみなす別の要求の応答の上限（秒）。修正後は 0.1 秒未満で返る。
# 修正前は偽の孫が終わる（600 秒）まで返らない
UI_REPLY_LIMIT=8
# 偽の孫が居座る秒数（後片付けで自分の pid を落とすが、落とし損ねても有限で終わる）
HOLD_SECS=600

TMP="$(mktemp -d /tmp/tako-1979-XXXXXX)"
HOLDERS="$TMP/holders.pids"
: > "$HOLDERS"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 偽の ps / tmux が残した孫（**自分が記録した pid だけ**を落とす）
  if [ -s "$HOLDERS" ]; then
    while read -r p; do
      [ -n "$p" ] && kill "$p" 2>/dev/null
    done < "$HOLDERS"
  fi
  "${REAL_TMUX:-tmux}" -L "tako-1979-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
(cd "$REPO_ROOT" && cargo build -q -p tako-cli -p tako-app) || exit 1
isolated_gui_bins || exit 1
REAL_TMUX="$(command -v tmux || true)"
[ -n "$REAL_TMUX" ] || { echo "未実測: tmux が無い（器のペインが作れない）"; exit 4; }

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1979-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/fakebin" "$TMP/faketmux"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 偽の ps: 本物の出力を出して終わるが、孫（sleep）がパイプの書き手を握り続ける
cat > "$TMP/fakebin/ps" <<EOF
#!/bin/sh
echo called >> "$TMP/fake-ps.calls"
/bin/ps "\$@"
sleep $HOLD_SECS &
echo \$! >> "$HOLDERS"
EOF
chmod +x "$TMP/fakebin/ps"

# 偽の tmux: `list-windows` のときだけ、本物の出力のあとに孫がパイプを握り続ける。
# それ以外（器の作成・attach）は本物へそのまま渡す
cat > "$TMP/faketmux/tmux" <<EOF
#!/bin/sh
case " \$* " in
  *" list-windows "*)
    echo called >> "$TMP/fake-tmux.calls"
    "$REAL_TMUX" "\$@"
    rc=\$?
    sleep $HOLD_SECS &
    echo \$! >> "$HOLDERS"
    exit \$rc ;;
esac
exec "$REAL_TMUX" "\$@"
EOF
chmod +x "$TMP/faketmux/tmux"

# `cmd` を `secs` 秒で打ち切って実行し、所要（秒）を出す。打ち切ったら "timeout"
timed() {
  local secs="$1"; shift
  local t0 t1 rc
  t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  perl -e 'alarm shift; exec @ARGV' "$secs" "$@" > "$TMP/timed.out" 2>&1
  rc=$?
  t1=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  if [ "$rc" -eq 142 ]; then echo "timeout"; else perl -e "printf '%.2f', $t1 - $t0"; fi
}

# メインスレッドのスタックに `poll`（子プロセスのパイプ待ち）が居座っているか（1 秒の sample）
# 居座っていたら、そのときのメインスレッドの tako の関数名を出す（証拠。パスは出さない）
main_thread_polls() {
  sample "$ISOLATED_GUI_PID" 1 -file "$TMP/sample.txt" >/dev/null 2>&1 || return 2
  awk '/Thread_.*com.apple.main-thread/{m=1; next} /^    [0-9]+ Thread_/{m=0} m' "$TMP/sample.txt" \
    > "$TMP/main-thread.txt"
  grep -E 'read_output|read2|capture_process_table|run_bounded|list_windows_by_session' \
    "$TMP/main-thread.txt" >/dev/null || return 1
  grep -oE '(tako_control|tako_core|std)::[A-Za-z0-9_:]+' "$TMP/main-thread.txt" \
    | grep -E 'dispatch::|agents::|tmux::|process::|probe::' | awk '!seen[$0]++' \
    | sed 's/^/      メインスレッド: /' | head -8
  return 0
}

# 起動を待つ。**`tako list` は使わない**（修正前の形では `tako list` 自体が器の
# `list-windows` で UI を止めるので、②の測る前に固まって起動確認が終わらない）
wait_ready() {
  local i
  for i in $(seq 1 300); do
    ( perl -e 'alarm shift; exec @ARGV' 3 "$TAKO_BIN" scrollback ) >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}

# 器（tmux の永続化）を有効にして起こす。隔離 GUI は既定で器が OFF なので、付けないと
# 器のペインが無く、①②の経路（器のペインの子プロセスの有無・window 一覧）に入らないまま緑になる
start_gui() {
  local log="$1"; shift
  launch_isolated_gui "$log" TAKO_PERSIST=1 "$@" || exit $?
  wait_ready || { fail "GUI が立たない（${log}）"; tail -20 "$log"; exit 1; }
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
}

# `tako list` の最初のタブのペイン ID（並び順の最初）。引数を渡すとそれ以外の最初の 1 つ
pane_ids() {
  "$TAKO_BIN" list | python3 -c '
import json, sys
d = json.load(sys.stdin)
skip = sys.argv[1] if len(sys.argv) > 1 else ""
for p in d["tabs"][0]["panes"]:
    if str(p["id"]) != skip:
        print(p["id"]); break
' "$@" 2>/dev/null
}
first_pane() { pane_ids; }

legacy_env=()
[ -n "$LEGACY" ] && legacy_env=(TAKO_1979_LEGACY=1)
arm="修正後"
[ -n "$LEGACY" ] && arm="修正前（TAKO_1979_LEGACY=1）"

echo "== ① worker の状態照会（本番ハングの経路）: 孫がパイプを握る ps / $arm"
start_gui "$TMP/app-ps.log" "PATH=$TMP/fakebin:$PATH" TAKO_1968_LEGACY=1 ${legacy_env[@]+"${legacy_env[@]}"}
PANE="$(first_pane)"
backend_pane() {
  "$TAKO_BIN" list | python3 -c '
import json, sys
d = json.load(sys.stdin)
sys.exit(0 if any(p.get("backend_windows") is not None for p in d["tabs"][0]["panes"]) else 1)'
}
if [ -z "$PANE" ]; then
  fail "① 最初のペインの ID が取れない"
elif ! backend_pane; then
  fail "① 器のペインが立っていない（経路に入らない = 判定できない）"
else
  # 状態照会は返らないことがあるので背景で打ち、UI の応答は別の要求で測る
  ( perl -e 'alarm shift; exec @ARGV' 30 "$TAKO_BIN" orchestrator status --pane "$PANE" > "$TMP/status.out" 2>&1 ) &
  STATUS_JOB=$!
  sleep 1.5
  took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" scrollback)"
  echo "    状態照会の最中の別の要求（tako scrollback）: $took 秒"
  if main_thread_polls; then polls="居る"; else polls="居ない"; fi
  echo "    メインスレッドの子プロセスのパイプ待ち（sample）: $polls"
  calls=0; [ -f "$TMP/fake-ps.calls" ] && calls=$(wc -l < "$TMP/fake-ps.calls" | tr -d ' ')
  echo "    偽の ps が起きた回数: $calls"
  if [ "$took" != "timeout" ] && [ "$polls" = "居ない" ]; then
    pass "① 状態照会の最中も UI が止まらない（別の要求が ${took} 秒で返る・メインスレッドに poll が無い）"
  else
    fail "① 状態照会で UI が止まった（別の要求: ${took}・メインスレッドの poll: ${polls}）= 本番ハングの形"
  fi
  wait "$STATUS_JOB" 2>/dev/null
  if [ -z "$LEGACY" ]; then
    [ "$calls" -eq 0 ] && pass "① 親子表は libproc で引く（偽の ps は一度も起きない）" \
      || fail "① 親子表の採取が ps を起こしている（$calls 回）"
    if python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if "status" in d else 1)' "$TMP/status.out" 2>/dev/null; then
      pass "① 状態照会そのものも返る（子プロセスの有無を数えたうえで）"
    else
      fail "① 状態照会が返らない: $(head -c 200 "$TMP/status.out")"
    fi
  fi
fi
stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

echo "== ② tako list の器の採り直し: 孫がパイプを握る tmux / $arm"
rm -f "$TMP/fake-tmux.calls"
start_gui "$TMP/app-tmux.log" "TAKO_TMUX_BIN=$TMP/faketmux/tmux" ${legacy_env[@]+"${legacy_env[@]}"}
# 起動直後の採取（2 秒 tick 等）と重ならないよう、採り直しの使い回し窓（0.5 秒）を待つ
sleep 1
( perl -e 'alarm shift; exec @ARGV' 30 "$TAKO_BIN" list > "$TMP/list.out" 2>&1; echo $? > "$TMP/list.rc" ) 2>/dev/null &
LIST_JOB=$!
sleep 0.5
took="$(timed "$UI_REPLY_LIMIT" "$TAKO_BIN" scrollback)"
echo "    tako list の最中の別の要求（tako scrollback）: $took 秒"
if main_thread_polls; then polls="居る"; else polls="居ない"; fi
echo "    メインスレッドの子プロセスのパイプ待ち（sample）: $polls"
wait "$LIST_JOB" 2>/dev/null
list_rc="$(cat "$TMP/list.rc" 2>/dev/null || echo '?')"
tcalls=0; [ -f "$TMP/fake-tmux.calls" ] && tcalls=$(wc -l < "$TMP/fake-tmux.calls" | tr -d ' ')
echo "    tako list の終了コード: ${list_rc}（142 = 30 秒で打ち切り）/ 偽の tmux の list-windows: $tcalls 回"
if [ "$took" != "timeout" ] && [ "$polls" = "居ない" ]; then
  pass "② tako list の採り直しの最中も UI が止まらない（別の要求が ${took} 秒で返る）"
else
  fail "② tako list の採り直しで UI が止まった（別の要求: ${took}・メインスレッドの poll: ${polls}）"
fi
if [ "$list_rc" = "0" ]; then
  pass "② tako list 自体も読み切りの猶予で返る"
else
  fail "② tako list が返らない（終了コード ${list_rc}）"
fi
[ "$tcalls" -ge 1 ] || fail "② 偽の tmux の list-windows が一度も起きていない（注入が効いていない）"
stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""

if [ -n "$LEGACY" ]; then
  echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
  [ "$FAIL" -eq 0 ]
  exit $?
fi

echo "== ③④ フォーカスの無いペインの再描画の上限（CLI で読み書き）"
start_gui "$TMP/app-redraw.log"
read_limit() { "$TAKO_BIN" redraw-limit; }
status="$(read_limit)"
fps="$(printf '%s' "$status" | python3 -c 'import json,sys; print(json.load(sys.stdin)["fps"])')"
[ "$fps" = "30" ] && pass "④ 既定は 30 fps（tako redraw-limit）" || fail "④ 既定が 30 でない: $status"
for bad in 0 61 4294967296; do
  if "$TAKO_BIN" redraw-limit "$bad" >/dev/null 2>"$TMP/bad.err"; then
    fail "④ 範囲外 $bad を受け付けた"
  else
    pass "④ 範囲外 $bad は弾く（$(head -1 "$TMP/bad.err" | cut -c1-80)）"
  fi
done
"$TAKO_BIN" redraw-limit 20 >/dev/null || fail "④ 20 を設定できない"
fps="$(read_limit | python3 -c 'import json,sys; print(json.load(sys.stdin)["fps"])')"
[ "$fps" = "20" ] && pass "④ 20 に変えると読み戻せる" || fail "④ 変更が読み戻せない: $fps"
saved="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("unfocused_redraw_fps"))' "$TAKO_DATA_DIR/settings.json" 2>/dev/null)"
[ "$saved" = "20" ] && pass "④ settings.json に残る（unfocused_redraw_fps=20）" || fail "④ settings.json に残っていない: $saved"
"$TAKO_BIN" redraw-limit 30 >/dev/null

# 出力し続けるペイン（1 行 10ms = 100 行 / 秒。CPU を焼く量ではない）を 2 枚並べ、
# フォーカスは何も出さない最初のペインに置いたまま、出力側の再描画の回数を数える
GEN='python3 -u -c "import time,itertools
for i in itertools.count():
    print(i, time.time(), flush=True); time.sleep(0.01)"'
first="$(first_pane)"
"$TAKO_BIN" split --pane "$first" --right -- /bin/sh -c "$GEN" >/dev/null || fail "③ 出力ペインを作れない"
"$TAKO_BIN" split --pane "$first" --down -- /bin/sh -c "$GEN" >/dev/null || fail "③ 出力ペインを作れない"
"$TAKO_BIN" focus "$first" >/dev/null 2>&1 || fail "③ 最初のペインへフォーカスを戻せない"
sleep 2
# 再描画の累計と、読んだ時刻（秒）。fps は「差 ÷ 2 回の読みの実際の間隔」で出す
# （sleep の秒数で割ると CLI の起動ぶんだけ速く見える）
flushes() { read_limit | python3 -c 'import json,sys,time; d=json.load(sys.stdin)["flushes"]; print(d["focused"], d["unfocused"], time.time())'; }
read -r f0 u0 t0 <<<"$(flushes)"
sleep 10
read -r f1 u1 t1 <<<"$(flushes)"
ufps=$(python3 -c "print(round(($u1 - $u0) / ($t1 - $t0), 1))")
ffps=$(python3 -c "print(round(($f1 - $f0) / ($t1 - $t0), 1))")
echo "    フォーカスの無い 2 ペインが出力中: 再描画 unfocused ${ufps} fps / focused ${ffps} fps（10 秒）"
python3 -c "import sys; sys.exit(0 if $ufps <= 30.5 else 1)" \
  && pass "③ フォーカスの無いペインの再描画が上限（30 fps）以下（${ufps} fps）" \
  || fail "③ フォーカスの無いペインの再描画が上限を超えている（${ufps} fps）"
python3 -c "import sys; sys.exit(0 if $ufps >= 15 else 1)" \
  && pass "③ 上限で止めすぎていない（${ufps} fps。出力は描かれ続けている）" \
  || fail "③ フォーカスの無いペインがほとんど描かれていない（${ufps} fps）"

# 上限を 10 にするとその場で効く（生存中のペインへ）
"$TAKO_BIN" redraw-limit 10 >/dev/null
sleep 1
read -r f0 u0 t0 <<<"$(flushes)"
sleep 10
read -r f1 u1 t1 <<<"$(flushes)"
ufps=$(python3 -c "print(round(($u1 - $u0) / ($t1 - $t0), 1))")
echo "    上限 10 fps に変えた後: unfocused ${ufps} fps"
python3 -c "import sys; sys.exit(0 if $ufps <= 10.2 else 1)" \
  && pass "③ 変えた上限が生存中のペインへその場で効く（${ufps} fps ≤ 10）" \
  || fail "③ 変えた上限が効いていない（${ufps} fps）"
"$TAKO_BIN" redraw-limit 30 >/dev/null

# フォーカス中のペインは今のまま（出力しているペインへフォーカスを移すと 60 fps 近くで描く）
out_pane="$(pane_ids "$first")"
"$TAKO_BIN" focus "$out_pane" >/dev/null 2>&1 || fail "③ フォーカスを移せない"
sleep 1
read -r f0 u0 t0 <<<"$(flushes)"
sleep 10
read -r f1 u1 t1 <<<"$(flushes)"
ffps=$(python3 -c "print(round(($f1 - $f0) / ($t1 - $t0), 1))")
echo "    出力中のペインにフォーカス: focused ${ffps} fps"
python3 -c "import sys; sys.exit(0 if $ffps >= 40 else 1)" \
  && pass "③ フォーカス中のペインは上限の外（${ffps} fps。従来どおり 16ms 間隔）" \
  || fail "③ フォーカス中のペインまで間引かれている（${ffps} fps）"

echo "結果（${arm}）: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
