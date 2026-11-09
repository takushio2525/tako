#!/bin/bash
# test-events-2731.sh — 変化の出来事の購読（`tako events` / MCP `tako_events`）を隔離 GUI の実経路で測る（#2731）
#
# 何を測るか（Issue #2731 の受け入れ条件の番号）:
#   1. `tako events --since 0` が JSON を返し cursor が増える。`--since <cursor> --wait-ms 20000` で待っている間に
#      別のシェルから role（`tako title --role`）・ペインの追加と閉じ・タブ名・ターン状態（偽の mod 報告 =
#      `tako mod report`）を変えると、変えてから 1 秒以内に該当の種類が返る（時刻を出す）
#   2. 該当が無ければ wait_ms の後に events: [] で返り cursor は変わらない。30000 を超えた wait_ms は丸めて載る。
#      9 本目の長ポーリングは待たずに理由つきで返る
#   3. 1000 件を超えて積んだ後に古い since で読むと dropped: true。GUI を起こし直すと epoch が変わる
#   4. 長ポーリングを 4 本並べて 60 秒待たせた間、perf.log に Events の専有が 0 件（その間の `tako list` の
#      応答時間も出す）
#   6. CLI と HTTP MCP（`tako_events`）の応答が一致する
#   境界: since が cursor より先・知らない種類・閉じたペインを --pane に・wait_ms 0
#
# 使い方: bash scripts/test-events-2731.sh   （KEEP=1 で一時 dir を残す / SKIP_LONG=1 で 4 の 60 秒を飛ばす）
# **CI には登録しない**（仮想ディスプレイが要る）。CI 側の担保は tako-core の events の単体と
# issue2731_events_long_poll / issue2731_events_no_body。
# 落とすのは自分で起こした pid だけ（名前一致では殺さない）。本物の claude は起こさない。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
export TAKO_ISOLATED=1

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 置き場は短く（ソケットのパス長の上限）
TMP="$(cd "$(mktemp -d /tmp/t2731-XXXXXX)" && pwd -P)"
BG_PIDS=()
cleanup() {
  local pid
  for pid in ${BG_PIDS[@]+"${BG_PIDS[@]}"}; do kill "$pid" 2>/dev/null; done
  stop_isolated_gui
  if [ -z "${KEEP:-}" ]; then rm -rf "${TMP:?}"; else echo "一時 dir: $TMP"; fi
}

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
trap cleanup EXIT
# 隔離の置き場はビルドの後で置く（先に置くと順番待ちが本番を見失って素通りする = #1705）
export TAKO_DATA_DIR="$TMP/d" TAKO_DISCOVERY_DIR="$TMP/d"
mkdir -p "$TMP/home" "$TMP/zdot"

tk() { "$TAKO_BIN" "$@"; }
now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
ms_between() { perl -e 'printf "%d\n", ($ARGV[1] - $ARGV[0]) * 1000' "$1" "$2"; }

launch() { # $1 = ログの名前
  launch_isolated_gui "$TMP/$1.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" SHELL=/bin/zsh TAKO_PERSIST=0 TAKO_AUTO_RENAME=0 || exit $?
  wait_isolated_gui "$TMP/$1.log" 300 || { fail "隔離 GUI の CLI が繋がらない"; exit 1; }
}

# 長ポーリングを背景で起こし、返った時刻を <出力>.t に書く
poll_bg() { # $1 = 出力の置き場、残り = tako events の引数
  local out="$1"
  shift
  ( tk events "$@" >"$out" 2>"$out.err"; now >"$out.t" ) &
  BG_PIDS+=("$!")
}
wait_file() { # $1 = ファイル、$2 = 上限秒
  local i
  for i in $(seq 1 $(($2 * 10))); do
    [ -s "$1" ] && return 0
    sleep 0.1
  done
  return 1
}

# 待っている長ポーリングを 1 本起こし、`$2…` の操作をした時刻から返るまでを測る
# $1 = 期待する種類、$2… = 操作（関数名と引数）
measure() {
  local kind="$1"
  shift
  local cursor out t0 ms got
  cursor="$(tk events --since 999999999 | jq -r '.cursor')"
  out="$TMP/m-$kind-$RANDOM.json"
  poll_bg "$out" --since "$cursor" --kind "$kind" --wait-ms 20000
  sleep 1.5 # 長ポーリングが待ちに入るまで（返るまでの時間は操作の時刻から測る）
  t0="$(now)"
  "$@" >/dev/null
  if ! wait_file "$out.t" 25; then
    fail "$kind: 長ポーリングが返らない"
    return
  fi
  ms="$(ms_between "$t0" "$(cat "$out.t")")"
  got="$(jq -c --arg k "$kind" '[.events[] | select(.kind == $k)] | .[0]' "$out")"
  if [ "$got" != null ] && [ "$ms" -lt 1000 ]; then
    pass "${kind}: 操作から ${ms}ms で返った（$(date -r "${t0%.*}" '+%H:%M:%S').${t0#*.} → ${got}）"
  else
    fail "$kind: ${ms}ms・中身 $(cat "$out")"
  fi
}

launch app1
P0="$(tk list | jq -r '.tabs[0].panes[0].id')"
T0="$(tk list | jq -r '.tabs[0].id')"
echo "== 1. 返る形と cursor（最初のペイン ${P0}）"
FIRST="$(tk events --since 0)"
C0="$(jq -r '.cursor' <<<"$FIRST")"
EPOCH1="$(jq -r '.epoch' <<<"$FIRST")"
jq -e '.kinds_known | length == 5' <<<"$FIRST" >/dev/null &&
  jq -e '[.events[].kind] | index("pane_added") != null' <<<"$FIRST" >/dev/null &&
  pass "tako events --since 0: cursor=${C0}・epoch=${EPOCH1}・最初のペインの pane_added が載る" ||
  fail "最初の応答: $FIRST"

measure role_changed tk title --pane "$P0" --role "orchestrator-master:dev"
split_once() { tk split --pane "$P0" >"$TMP/split.id"; }
measure pane_added split_once
P1="$(cat "$TMP/split.id")"
measure pane_closed tk close --pane "$P1" --force
measure tab_renamed tk tab rename --tab "$T0" "events-2731"
report() { # $1 = ターン
  printf '{"schema":1,"mod_version":"0.0.0","at":1,"turn":"%s"}' "$1" |
    TAKO_PANE_ID="$P0" tk mod report >/dev/null
}
measure turn_changed report busy
C1="$(tk events --since 999999999 | jq -r '.cursor')"
[ "$C1" -gt "$C0" ] && pass "cursor が増えた（${C0} → ${C1}）" || fail "cursor が増えない（${C0} → ${C1}）"
TURN="$(tk events --since "$C0" --kind turn_changed | jq -c '[.events[] | .data]')"
report idle
TURN2="$(tk events --since "$C1" --kind turn_changed | jq -c '[.events[] | .data]')"
echo "  turn_changed の data: $TURN → $TURN2"
[ "$(jq -r '.[0].from' <<<"$TURN2")" = busy ] && [ "$(jq -r '.[0].to' <<<"$TURN2")" = idle ] &&
  pass "turn_changed は前の報告から変わった分だけ（busy → idle）" || fail "turn_changed: $TURN2"
report idle
TURN3="$(tk events --since "$(tk events --since 999999999 | jq -r '.cursor')" --kind turn_changed --wait-ms 300 | jq -c '.events')"
[ "$TURN3" = "[]" ] && pass "同じターンの報告（heartbeat）は積まない" || fail "heartbeat で積んだ: $TURN3"

echo "== 2. 該当なし・丸め・9 本目"
C="$(tk events --since 999999999 | jq -r '.cursor')"
t0="$(now)"
EMPTY="$(tk events --since "$C" --pane 999999 --wait-ms 1500)"
ms="$(ms_between "$t0" "$(now)")"
[ "$(jq -c '.events' <<<"$EMPTY")" = "[]" ] && [ "$(jq -r '.cursor' <<<"$EMPTY")" = "$C" ] && [ "$ms" -ge 1400 ] &&
  pass "該当なし: ${ms}ms 待って events: [] ・cursor $C のまま" || fail "該当なし: ${ms}ms $EMPTY"
CLAMP="$(tk events --since 0 --wait-ms 60000)"
[ "$(jq -r '.wait_ms' <<<"$CLAMP")" = 30000 ] && [ "$(jq -r '.wait_clamped' <<<"$CLAMP")" = true ] &&
  pass "wait_ms 60000 → 応答の wait_ms 30000・wait_clamped: true" || fail "丸め: $(jq -c '{wait_ms, wait_clamped}' <<<"$CLAMP")"
C="$(tk events --since 999999999 | jq -r '.cursor')"
for i in 1 2 3 4 5 6 7 8; do poll_bg "$TMP/w$i.json" --since "$C" --pane "$P0" --wait-ms 30000; done
sleep 2
NINTH="$(tk events --since "$C" --pane "$P0" --wait-ms 30000)"
[ "$(jq -r '.wait_skipped' <<<"$NINTH")" = too_many_waiters ] &&
  pass "9 本目: 待たずに返り wait_skipped=too_many_waiters（$(jq -r '.wait_skipped_reason' <<<"$NINTH")）" ||
  fail "9 本目: $NINTH"
tk title --pane "$P0" --role "orchestrator-master:review" >/dev/null
woke=0
for i in 1 2 3 4 5 6 7 8; do
  wait_file "$TMP/w$i.json.t" 10 && [ "$(jq -r '.events[0].data.to' "$TMP/w$i.json")" = orchestrator-master:review ] && woke=$((woke + 1))
done
[ "$woke" = 8 ] && pass "待っていた 8 本はすべて role_changed で起きた" || fail "起きたのは $woke / 8 本"

echo "== 境界"
AHEAD="$(tk events --since $((C + 100000)) --wait-ms 20000)"
[ "$(jq -r '.dropped_reason' <<<"$AHEAD")" = since_ahead ] &&
  pass "since が cursor より先: 待たずに dropped: true（since_ahead）・events $(jq '.events | length' <<<"$AHEAD") 件" ||
  fail "since が先: $(jq -c '{dropped, dropped_reason, cursor}' <<<"$AHEAD")"
if UNKNOWN="$(tk events --kind launch_changed 2>&1)"; then
  fail "知らない種類を断らない: $UNKNOWN"
else
  pass "知らない種類は断る: $UNKNOWN"
fi
CLOSED="$(tk events --since 0 --pane "$P1" | jq -c '[.events[].kind]')"
[ "$CLOSED" = '["pane_added","pane_closed"]' ] &&
  pass "閉じたペインを --pane に: そのペインの出来事（${CLOSED}）だけ" || fail "閉じたペイン: $CLOSED"
t0="$(now)"
ZERO="$(tk events --since 999999999 --wait-ms 0)"
ms="$(ms_between "$t0" "$(now)")"
[ "$(jq -r '.wait_ms' <<<"$ZERO")" = 0 ] && pass "wait_ms 0: ${ms}ms で返る" || fail "wait_ms 0: $ZERO"

echo "== 6. CLI と HTTP MCP の応答"
URL="$(jq -r '.mcp_url' "$TAKO_DISCOVERY_DIR/control.json")"
TOKEN="$(jq -r '.token' "$TAKO_DISCOVERY_DIR/control.json")"
ARGS="{\"since\":$C0,\"kinds\":[\"role_changed\",\"tab_renamed\"]}"
VIA_MCP="$(curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_events\",\"arguments\":$ARGS}}" \
  "$URL" | jq -r '.result.content[0].text')"
VIA_CLI="$(tk events --since "$C0" --kind role_changed --kind tab_renamed)"
if [ "$(jq -cS 'del(.cursor)' <<<"$VIA_MCP")" = "$(jq -cS 'del(.cursor)' <<<"$VIA_CLI")" ]; then
  pass "CLI と MCP の応答が一致（cursor 以外。出来事 $(jq '.events | length' <<<"$VIA_CLI") 件・cursor CLI $(jq .cursor <<<"$VIA_CLI") / MCP $(jq .cursor <<<"$VIA_MCP")）"
else
  fail "CLI と MCP が違う: CLI=$VIA_CLI MCP=$VIA_MCP"
fi

if [ -z "${SKIP_LONG:-}" ]; then
  echo "== 4. 長ポーリング 4 本 × 60 秒の間の UI"
  before="$(grep -c '' "$TAKO_DATA_DIR/perf.log" 2>/dev/null || echo 0)"
  C="$(tk events --since 999999999 | jq -r '.cursor')"
  for round in 1 2; do
    for i in 1 2 3 4; do poll_bg "$TMP/l$round-$i.json" --since "$C" --pane 999999 --wait-ms 30000; done
    worst=0
    for s in $(seq 1 29); do
      t0="$(now)"
      tk list >/dev/null
      ms="$(ms_between "$t0" "$(now)")"
      [ "$ms" -gt "$worst" ] && worst=$ms
      sleep 1
    done
    for i in 1 2 3 4; do wait_file "$TMP/l$round-$i.json.t" 10 || true; done
    echo "  周 $round: 長ポーリング 4 本が待つ間の tako list の最長 ${worst}ms"
  done
  done4=0
  for f in "$TMP"/l?-?.json; do [ "$(jq -c '.events' "$f")" = "[]" ] && done4=$((done4 + 1)); done
  stalls="$(tail -n +"$((before + 1))" "$TAKO_DATA_DIR/perf.log" 2>/dev/null | grep -c 'UI ストール' || true)"
  events_hits="$(grep -c 'Events' "$TAKO_DATA_DIR/perf.log" 2>/dev/null || true)"
  echo "  perf.log（$(grep -c '' "$TAKO_DATA_DIR/perf.log" 2>/dev/null || echo 無し) 行）: この 60 秒の UI ストールの行 ${stalls:-0}・Events を名指す行 ${events_hits:-0}"
  [ "$done4" = 8 ] && [ "${events_hits:-0}" = 0 ] &&
    pass "4 本 × 2 周（60 秒）すべて時間切れで返り、perf.log に Events の専有 0 件" ||
    fail "時間切れ $done4 / 8・Events の行 ${events_hits:-0}"
fi

echo "== 3. 溢れと epoch"
# 上限（1000 件）を越えるまで role を交互に書き換える（1 回 = 1 件）
for i in $(seq 1 1010); do
  tk title --pane "$P0" --role "r$((i % 2))" >/dev/null
done
OLD="$(tk events --since 1 --kind pane_closed)"
[ "$(jq -r '.dropped' <<<"$OLD")" = true ] && [ "$(jq -r '.dropped_reason' <<<"$OLD")" = overflow ] &&
  pass "1000 件を超えた後の since 1: dropped: true（overflow）・cursor $(jq .cursor <<<"$OLD")" ||
  fail "溢れ: $(jq -c '{dropped, dropped_reason, cursor}' <<<"$OLD")"
stop_isolated_gui
launch app2
EPOCH2="$(tk events --since 0 | jq -r '.epoch')"
[ -n "$EPOCH2" ] && [ "$EPOCH2" != "$EPOCH1" ] &&
  pass "起こし直すと epoch が変わる（${EPOCH1} → ${EPOCH2}）" || fail "epoch: $EPOCH1 → $EPOCH2"

echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
