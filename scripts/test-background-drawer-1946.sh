#!/bin/bash
# test-background-drawer-1946.sh — たまり場（退避ペインの一覧）の見え方と後始末（#1946）を visual-test で確かめる
#
# 何を確かめるか（visual-test 節 background-drawer）:
#   ① ドロワーを開いても退避ペインの寸法が変わらない（#1946 前はカード寸法へ resize していた）
#   ② カードは画面の全行を縮小で描き、カードの枠に収まる（claude 風 TUI・シェル）
#   ③ 作業タブの退避が親 master ごとの見出しで alpha → beta → 親が閉じた の順に並ぶ
#   ④ 出力し続ける退避 10 本以上でも、出力起因の全体再描画は 4 回/秒程度に間引かれる
#   ⑤ カードのホバーで画面全体が固定寸法より大きく出る
#   ⑥ 器の無い幽霊を実マウスの × → はい で閉じても、他の器（tmux セッション）は 1 本も減らない
#   ⑦ 退避 0 本でも描ける
#   A/B: `TAKO_1946_LEGACY=1`（#1946 前のたまり場）では ① で落ちる。④ の実測値は両方の腕で出す
#   実 CLI の段（`tako backgrounded`）: 2 つの master の worker を退避すると groups が master ごとに
#      分かれ、各ペインに vessel / parent_master が載り、GUI を再起動しても同じまとまりのまま（spawned_by の保存）
#
# スクリプトの段: ① 節（新しい形）→ ② 節（A/B の旧い形）→ ③ 実 CLI
#
# 使い方: bash scripts/test-background-drawer-1946.sh [画像の置き場]
#   画像（background-drawer*.png）は 置き場/new と 置き場/legacy へ分けて落とす。
#   置き場を省くと一時 dir に落として終了時に消す。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 器（tmux）の経路を通すため `TAKO_PERSIST=1` で起こす（隔離 GUI の既定は OFF）。
# 自動命名は `TAKO_AUTO_RENAME=0` で止める（隔離 GUI でも claude を呼ぶ = #1968 の罠）。
# tmux は隔離ソケット（`TAKO_TMUX_SOCKET`）だけを使い、終わったらそのサーバーだけを落とす。
# 落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1946v-XXXXXX)"
DUMP_ROOT="${1:-$TMP/dump}"
SOCKET="tako-1946v-$$"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$SOCKET" kill-server 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app をビルドします…"
(cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$SOCKET"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 画像に実ユーザー名・ホスト名を写さない（zsh の既定プロンプトは %n@%m を出す）
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"
# 幽霊のカードまで 1 画面に収まりやすい寸法（窓の外は節が実ホイールで送る）
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,880}
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: ${d}"; exit 1 ;;
  esac
done

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 180 秒）
run_section() {
  local arm="$1" log="$2" dump="$3"
  shift 3
  mkdir -p "$dump" "$TMP/data-$arm"
  # 腕ごとにデータを分ける（persist が前の腕の layout.json を復元しないように）
  launch_isolated_gui "$log" \
    TAKO_DATA_DIR="$TMP/data-$arm" TAKO_SESSIONS_FILE="$TMP/sessions-$arm.yaml" \
    TAKO_PANE_LOG_DIR="$TMP/panelogs-$arm" TAKO_WORKERS_FILE="$TMP/workers-$arm.yaml" \
    TAKO_PERSIST=1 TAKO_AUTO_RENAME=0 \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=background-drawer \
    TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1800); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  # 節が残した器（隔離ソケットのサーバー）を次の腕へ持ち越さない
  tmux -L "$SOCKET" kill-server 2>/dev/null
  return 0
}

# 段の選び方: STAGES="visual cli"（既定は両方）。visual = ① と ②、cli = ③
STAGES="${STAGES:-visual cli}"
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

if has_stage visual; then
  echo "① background-drawer 節"
  run_section new "$TMP/new.log" "$DUMP_ROOT/new"
  grep -E "TAKO_VISUAL_(PIXEL|PERF): background-drawer|TAKO_VISUAL_DUMP_FILE" "$TMP/new.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
    pass "全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "節が緑にならない"
    grep -E "FAILED|panicked|ERROR|SKIPPED|TIMEOUT" "$TMP/new.log" | tail -5 | sed 's/^/    /'
  fi
  if grep -q "tmux before=[1-9]" "$TMP/new.log"; then
    pass "器（tmux セッション）の経路に入っている: $(grep -o 'tmux before=[0-9]* after=[0-9]*' "$TMP/new.log" | head -1)"
  else
    fail "器（tmux セッション）が 0 本 = 器の経路に入っていない（TAKO_PERSIST が効いていない）"
  fi

  echo "② A/B: TAKO_1946_LEGACY=1（#1946 前のたまり場）"
  run_section legacy "$TMP/legacy.log" "$DUMP_ROOT/legacy" TAKO_1946_LEGACY=1
  grep -E "TAKO_VISUAL_(PIXEL|PERF): background-drawer|TAKO_VISUAL_DUMP_FILE" "$TMP/legacy.log" | sed 's/^/    /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: ① ドロワーを開いても退避ペインの寸法が変わらない" "$TMP/legacy.log"; then
    pass "旧経路では ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/legacy.log" | head -1)"
  else
    fail "旧経路でも ① で落ちない（節に検出力が無い）"
    grep -E "FAILED|panicked" "$TMP/legacy.log" | tail -3 | sed 's/^/    /'
  fi
fi

if has_stage cli; then
  echo "③ 実 CLI: tako backgrounded の groups（2 つの master の worker）と、GUI 再起動後も崩れないこと"
  # worker は本番と同じ `tako orchestrator spawn` で立てる（親 = spawned_by を張るのはこの経路。
  # 汎用の `tako split` は張らない）。起動先は偽 claude（API を呼ばず、画面に 1 行出して待つだけ）
  mkdir -p "$TMP/bin" "$TMP/work" "$TAKO_ORCHESTRATOR_DIR/profiles"
  cat > "$TMP/bin/claude" <<'STUB'
#!/bin/sh
case "$1" in
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
  agents) echo "[]"; exit 0 ;;
  -p) exit 0 ;;
esac
echo "FAKECLAUDE-1946"
exec sleep 600
STUB
  chmod +x "$TMP/bin/claude"
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  PATH: "$TMP/bin:/usr/bin:/bin:/opt/homebrew/bin"
YAML
  "$TAKO_BIN" orchestrator projects add --key t1946 --cwd "$TMP/work" \
    --description "#1946 の検証（自動削除される）" >/dev/null 2>&1
  APP_PID=""
  start_app() {
    launch_isolated_gui "$TMP/app-$1.log" TAKO_PERSIST=1 TAKO_AUTO_RENAME=0 \
      "PATH=$TMP/bin:$PATH" || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    wait_isolated_gui "$TMP/app-$1.log" || exit 1
  }
  tako_() { "$TAKO_BIN" "$@" 2>/dev/null; }
  spawn_from() {
    tako_ orchestrator spawn --project t1946 --prompt "#1946 の検証" --pane "$1" --label "$2" \
      | jq -r '.pane_id // empty'
  }
  start_app cli1
  if [ "$(tako_ list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する。"
    exit 1
  fi
  P1="$(tako_ list | jq '.tabs[0].panes[0].id')"
  P2="$(tako_ split --pane "$P1" --right | tr -dc '0-9')"
  tako_ title --pane "$P1" --role orchestrator-master:alpha "alpha の master" >/dev/null
  tako_ title --pane "$P2" --role orchestrator-master:beta "beta の master" >/dev/null
  WA="$(spawn_from "$P1" wa)"
  WB="$(spawn_from "$P2" wb)"
  # worker から立てた worker（孫）も同じ master の見出しへ入る
  WA2="$(spawn_from "$WA" wa2)"
  if [ -z "$WA" ] || [ -z "$WB" ] || [ -z "$WA2" ]; then
    fail "orchestrator spawn で worker を立てられない（wa=${WA} wb=${WB} wa2=${WA2}）"
  fi
  sleep 2
  for p in "$WA" "$WB" "$WA2"; do tako_ background --pane "$p" >/dev/null; done
  echo "    master alpha=${P1} beta=${P2} / 退避: alpha の worker=${WA}（その子 ${WA2}）beta の worker=${WB}"
  # 画面と同じまとまり（由来タブ → 親 master）と器
  summary() {
    jq -c '[.groups[] | {tab: .title, origin, masters: [.masters[] | {profile: .master.profile, panes}]}]'
  }
  masters_of() { jq -c '[.groups[].masters[] | {profile: .master.profile, panes}]'; }
  vessels() { jq -c '[.backgrounded[] | .vessel] | unique'; }
  OUT1="$(tako_ backgrounded)"
  echo "    groups: $(echo "$OUT1" | summary)"
  echo "    vessel: $(echo "$OUT1" | vessels)"
  WANT="[{\"profile\":\"alpha\",\"panes\":[${WA},${WA2}]},{\"profile\":\"beta\",\"panes\":[${WB}]}]"
  GOT1="$(echo "$OUT1" | masters_of)"
  if [ "$GOT1" = "$WANT" ]; then
    pass "退避が master alpha → beta の見出しで分かれる（${GOT1}）"
  else
    fail "master ごとに分かれない（期待 ${WANT} / 実際 ${GOT1}）"
  fi
  if [ "$(echo "$OUT1" | vessels)" = '["tmux"]' ]; then
    pass "退避中のペインの器はすべて tmux（vessel）"
  else
    fail "器の判定が期待と違う: $(echo "$OUT1" | vessels)"
  fi
  if [ "$(echo "$OUT1" | jq -r ".backgrounded[] | select(.pane == ${WB:-0}) | .parent_master.profile")" = "beta" ]; then
    pass "各ペインに parent_master が載る（beta の worker → beta）"
  else
    fail "parent_master が載らない"
  fi
  # 再起動（器は生かしたまま）。spawned_by が layout.json から戻ればまとまりは同じ。
  # 保存は 2 秒ごとの tick なので、退避の後に 1 回は回るのを待ってから落とす
  sleep 3
  stop_isolated_gui "$APP_PID"
  sleep 1
  if grep -q '"spawned_by"' "$TAKO_DATA_DIR/layout.json" 2>/dev/null; then
    pass "layout.json に spawned_by が保存される"
  else
    fail "layout.json に spawned_by が無い"
  fi
  start_app cli2
  OUT2="$(tako_ backgrounded)"
  GOT2="$(echo "$OUT2" | masters_of)"
  echo "    再起動後 groups: $(echo "$OUT2" | summary)"
  if [ "$GOT2" = "$WANT" ] && [ "$(echo "$OUT2" | vessels)" = '["tmux"]' ]; then
    pass "GUI 再起動後も同じ master の見出しで分かれ、器も tmux のまま"
  else
    fail "再起動でまとまりが崩れる（期待 ${WANT} / 実際 ${GOT2} / 器 $(echo "$OUT2" | vessels)）"
  fi
  stop_isolated_gui "$APP_PID"
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
