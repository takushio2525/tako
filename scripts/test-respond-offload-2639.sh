#!/usr/bin/env bash
# test-respond-offload-2639.sh — OrchestratorRespond が GUI のメインスレッドを専有しないかの実経路テスト（#2639）
#
# 隔離 GUI（`TAKO_ISOLATED=1`）のペインで**模擬 TUI**（claude の確認ダイアログの形だけを真似る bash。
# #1293 の e2e と同じ作り）を動かし、`tako orchestrator respond`（IPC → GUI の dispatch = MCP
# `tako_orchestrator_respond` と同じ口）でダイアログへ答える。応答の手順（`respond_via`）は番号キーの後に
# 400ms・Enter の後に 400ms の固定の待ちを挟むので、その間に別の IPC（`tako list`）が待たされないか・
# perf.log に `dispatch:OrchestratorRespond` の専有が出ないかを測る（負荷は作らない）:
#   ① 番号キーで閉じるダイアログ（claude の形。待ち 400ms 1 回 = 本番の 401〜414ms）
#   ② 番号キーでは閉じず Enter で確定するダイアログ（codex の形。待ち 400ms 2 回 = 区間を伸ばす）
#   ③ 境界: 下見（choice 省略）・ダイアログが無い画面・合わない choice・無いペイン
#   ④ A/B（TAKO_2639_LEGACY=1）: 修正前と同じく UI スレッドで待つ。応答・監査記録（persist.log の
#      `[dialog-respond]`）・断りは ①②③ と同じ
# 修正前のバイナリ（TAKO_BIN / APP_BIN を差し替える）では ①② の行が落ちる。
#
# **本番の GUI / tmux / data dir には一切触らない**（置き場は mktemp 配下。落とすのは自分で起こした GUI だけ）。
# 窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-respond-offload-2639.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。証拠は TAKO_2639_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_2639_LEGACY TAKO_OFFLOAD
# 応答の記録の呼び手は固定の名前にする（呼び手の役割名を砂場の記録へ持ち込まない）
export TAKO_ORCHESTRATOR_ROLE=t2639

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

# 「待たされていない」の上限（ms）。debug ビルドの CLI の起動と `tako list` の往復を含めて余裕を持たせる
QUICK_MS="${TAKO_2639_QUICK_MS:-150}"
# 応答 1 回ぶんの区間を何回測るか
ROUNDS="${TAKO_2639_ROUNDS:-3}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2639-XXXXXX)"
TMUX_SOCKET="tako-iso-2639-$$"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/data" "$TMP/disc" "$TMP/ctl"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
CLI="$TMP/ctl/tako"
PERF="$TMP/data/perf.log"
PERSIST="$TMP/data/persist.log"

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_LANG=ja

# --- 模擬 TUI（$1 = ログ・$2 = mode: number = 番号キーで閉じる / enter = Enter で確定する）---
EMU="$TMP/emu.sh"
cat > "$EMU" <<'EMU_EOF'
#!/bin/bash
# tako の respond（#2639）の実キー検証用。実 claude / codex の画面の形だけを真似る
LOG="$1"
MODE="${2:-number}"
: > "$LOG"
labels=("Yes" "Yes, and don't ask again" "No, and tell Claude what to do differently")
rule="────────────────────────────────────────────────────────────────────────"
sel=0
draw() {
  printf '\033[2J\033[H'
  echo "⏺ Bash(ls)"
  echo
  echo "  Bash command"
  echo
  echo "    ls -la"
  echo "    List files"
  echo
  echo "  Do you want to proceed?"
  local i
  for i in 0 1 2; do
    if [ "$i" = "$sel" ]; then
      echo "  ❯ $((i + 1)). ${labels[$i]}"
    else
      echo "    $((i + 1)). ${labels[$i]}"
    fi
  done
  echo
  echo "  Esc to cancel"
}
stty -echo -icanon -icrnl min 1 time 0 2>/dev/null
draw
while :; do
  IFS= read -rsn1 c || break
  case "$c" in
    1|2|3)
      echo "KEY=$c" >> "$LOG"
      if [ "$MODE" = number ]; then
        echo "CHOSE=${labels[$((c - 1))]}" >> "$LOG"
        break
      fi
      sel=$((c - 1))
      draw
      ;;
    ''|$'\r'|$'\n')
      echo "KEY=Enter" >> "$LOG"
      echo "CHOSE=${labels[$sel]}" >> "$LOG"
      break
      ;;
  esac
done
stty sane 2>/dev/null
# 確定後はダイアログを消す（入力欄だけが残る = respond の解消検証が真になる形）
printf '\033[2J\033[H'
echo "⏺ Bash(ls)"
echo "  done"
echo "$rule"
echo "❯"
echo "$rule"
exec sleep 600
EMU_EOF
chmod +x "$EMU"

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'; }

# perf.log の「メインスレッド専有: <タグ> が N ms」の行数（$1 = タグの正規表現）
perf_hits() {
  grep -cE "メインスレッド専有: $1 が [0-9]+ms" "$PERF" 2>/dev/null | tr -dc '0-9'
}
# 往復の最長（ms）。1 回も通らなければ大きな値（= 「待たされない」を満たさない）
max_of() {
  if [ -s "$1" ]; then sort -n "$1" | tail -1 | tr -dc '0-9'; else echo 999999; fi
}

# 模擬 TUI のペインを立て、ペイン ID を返す（$1 = mode・$2 = ログ。`tako split` は新しいペイン ID を素の数で出す）
spawn_emu() {
  "$CLI" split --pane "$BASE_PANE" --down -- bash "$EMU" "$2" "$1" 2>/dev/null | tr -dc '0-9'
}
# ダイアログが画面に出るまで下見を打つ（$1 = ペイン・$2 = 書き先）。出れば 0
wait_dialog() {
  local i
  for i in $(seq 1 100); do
    if "$CLI" orchestrator respond --pane "$1" >"$2" 2>/dev/null && jq -e '.responded == false' "$2" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

# respond を背景で打ち、その間の `tako list` の往復を測る（$1 = 名前の接頭辞・$2 = mode）。
# respond が GUI に届いて固定の待ちへ入るまで 100ms 置いてから打ち、respond が返るまで打ち続ける
measure_respond() {
  local p="$1" mode="$2" r pane t0 t1 busy u0 u1
  : > "$TMP/$p-ipc.txt"
  : > "$TMP/$p-respond-ms.txt"
  : > "$PERF"
  for r in $(seq 1 "$ROUNDS"); do
    pane=$(spawn_emu "$mode" "$TMP/$p-emu-$r.log")
    if [ -z "$pane" ] || ! wait_dialog "$pane" "$TMP/$p-preview-$r.json"; then
      fail "[$p] 模擬 TUI のダイアログを出せない（round ${r}・pane '${pane}'）"
      continue
    fi
    t0=$(now_ms)
    "$CLI" orchestrator respond --pane "$pane" --choice 1 >"$TMP/$p-respond-$r.json" 2>"$TMP/$p-respond-$r.err" &
    busy=$!
    sleep 0.1
    while kill -0 "$busy" 2>/dev/null; do
      u0=$(now_ms)
      if "$CLI" list >/dev/null 2>&1; then
        u1=$(now_ms)
        echo $((u1 - u0)) >> "$TMP/$p-ipc.txt"
      fi
    done
    wait "$busy"
    t1=$(now_ms)
    echo $((t1 - t0)) >> "$TMP/$p-respond-ms.txt"
    "$CLI" close --pane "$pane" >/dev/null 2>&1 || true
  done
  # 区間の後の記録が perf.log へ落ちるのを待つ
  sleep 1
  command cp -f "$PERF" "$TMP/$p-perf.log" 2>/dev/null || : > "$TMP/$p-perf.log"
  echo "  （respond の往復: $(LC_ALL=C tr '\n' ' ' < "$TMP/$p-respond-ms.txt")ms・その間の tako list: $(LC_ALL=C tr '\n' ' ' < "$TMP/$p-ipc.txt")ms）"
  grep -E "dispatch:OrchestratorRespond|ipc_turn" "$PERF" 2>/dev/null | sed 's/^/    perf.log: /' | head -6
}

# 応答の中身を比べられる形へ（ペイン ID は実行ごとに変わる）
normalize() { jq -S -c 'del(.pane_id)' 2>/dev/null; }
# persist.log の応答の記録（ペイン ID を落とす）
respond_audit() {
  grep -F '[dialog-respond]' "$PERSIST" 2>/dev/null | sed -E 's/^.*\[dialog-respond\]/[dialog-respond]/; s/ pane=[0-9]+//'
}

# 1 回ぶんの GUI を起こして ①〜③ を回す（$1 = 接頭辞・$2… = GUI へ渡す VAR=VAL）
run_round() {
  local r="$1"
  shift
  : > "$PERSIST"
  launch_isolated_gui "$TMP/app-$r.log" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui
  BASE_PANE=$("$CLI" list 2>/dev/null | jq -r '.tabs[0].focused_pane // empty' 2>/dev/null)

  echo
  echo "== [$r] ① 番号キーで閉じるダイアログ（claude の形）への respond の間に別の IPC は待たされない =="
  measure_respond "$r-number" number
  for n in $(seq 1 "$ROUNDS"); do
    normalize < "$TMP/$r-number-respond-$n.json" > "$TMP/$r-number-respond-$n.norm"
  done
  check_eq "[$r] ① 応答は選べて閉じた（responded / resolved / keys）" 'true/true/1' \
    "$(jq -r '"\(.responded)/\(.resolved)/\(.keys_sent | join("+"))"' "$TMP/$r-number-respond-1.json" 2>/dev/null)"
  check_eq "[$r] ① 模擬 TUI が受けたのは番号キー 1 つ" "KEY=1 CHOSE=Yes" "$(LC_ALL=C tr '\n' ' ' < "$TMP/$r-number-emu-1.log" | sed 's/ $//')"

  echo
  echo "== [$r] ② Enter で確定するダイアログ（codex の形。待ち 2 回）への respond の間に別の IPC は待たされない =="
  measure_respond "$r-enter" enter
  for n in $(seq 1 "$ROUNDS"); do
    normalize < "$TMP/$r-enter-respond-$n.json" > "$TMP/$r-enter-respond-$n.norm"
  done
  check_eq "[$r] ② 応答は選べて閉じた（番号 + Enter）" 'true/true/1+Enter' \
    "$(jq -r '"\(.responded)/\(.resolved)/\(.keys_sent | join("+"))"' "$TMP/$r-enter-respond-1.json" 2>/dev/null)"
  check_ge "[$r] ② respond は待ち 2 回ぶんかかる（区間を作れている）" 800 "$(head -1 "$TMP/$r-enter-respond-ms.txt" | tr -dc '0-9')"

  echo
  echo "== [$r] ③ 境界: 下見・ダイアログが無い画面・合わない choice・無いペイン =="
  local pane
  pane=$(spawn_emu number "$TMP/$r-edge-emu.log")
  if [ -n "$pane" ] && wait_dialog "$pane" "$TMP/$r-edge-preview.json"; then
    normalize < "$TMP/$r-edge-preview.json" > "$TMP/$r-edge-preview.norm"
    check_eq "[$r] ③ 下見は送らずに 3 択を返す" "false/3" "$(jq -r '"\(.responded)/\(.options | length)"' "$TMP/$r-edge-preview.json" 2>/dev/null)"
    "$CLI" orchestrator respond --pane "$pane" --choice Maybe >"$TMP/$r-edge-nomatch.out" 2>&1
    echo "rc=$?" >> "$TMP/$r-edge-nomatch.out"
    check_eq "[$r] ③ 合わない choice は断る（rc≠0）" 1 "$(grep -c '^rc=[1-9]' "$TMP/$r-edge-nomatch.out" | tr -dc '0-9')"
    check_eq "[$r] ③ 合わない choice ではキーを送らない" "" "$(cat "$TMP/$r-edge-emu.log")"
    "$CLI" orchestrator respond --pane "$pane" --choice 1 >/dev/null 2>&1
    sleep 0.3
    "$CLI" orchestrator respond --pane "$pane" --choice 1 >"$TMP/$r-edge-nodialog.out" 2>&1
    echo "rc=$?" >> "$TMP/$r-edge-nodialog.out"
    check_eq "[$r] ③ ダイアログが無い画面は断る（rc≠0）" 1 "$(grep -c '^rc=[1-9]' "$TMP/$r-edge-nodialog.out" | tr -dc '0-9')"
    "$CLI" close --pane "$pane" >/dev/null 2>&1 || true
  else
    fail "[$r] ③ 模擬 TUI のダイアログを出せない"
  fi
  "$CLI" orchestrator respond --pane 999999 --choice 1 >"$TMP/$r-edge-nopane.out" 2>&1
  echo "rc=$?" >> "$TMP/$r-edge-nopane.out"
  check_eq "[$r] ③ 無いペインは断る（rc≠0）" 1 "$(grep -c '^rc=[1-9]' "$TMP/$r-edge-nopane.out" | tr -dc '0-9')"
  sleep 0.5
  respond_audit > "$TMP/$r-audit.txt"
  echo "  （persist.log の応答の記録: $(grep -c . "$TMP/$r-audit.txt" | tr -dc '0-9') 行）"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

run_round new
echo
echo "== 修正後（既定）の判定 =="
check_lt "① respond の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-number-ipc.txt")"
PERF="$TMP/new-number-perf.log"
check_eq "① perf.log に dispatch:OrchestratorRespond の専有が出ない" 0 "$(perf_hits 'dispatch:OrchestratorRespond')"
check_lt "② respond の間の tako list の最長の往復（ms）" "$QUICK_MS" "$(max_of "$TMP/new-enter-ipc.txt")"
PERF="$TMP/new-enter-perf.log"
check_eq "② perf.log に dispatch:OrchestratorRespond の専有が出ない" 0 "$(perf_hits 'dispatch:OrchestratorRespond')"
PERF="$TMP/data/perf.log"

run_round legacy TAKO_2639_LEGACY=1
echo
echo "== A/B（TAKO_2639_LEGACY=1）の判定: 修正前と同じく UI スレッドで待つ =="
check_ge "A/B ① respond の間の tako list は待たされる（ms）" 200 "$(max_of "$TMP/legacy-number-ipc.txt")"
PERF="$TMP/legacy-number-perf.log"
check_eq "A/B ① perf.log に dispatch:OrchestratorRespond の専有が出る" "$ROUNDS" "$(perf_hits 'dispatch:OrchestratorRespond')"
check_ge "A/B ② respond の間の tako list は待たされる（ms）" 500 "$(max_of "$TMP/legacy-enter-ipc.txt")"
PERF="$TMP/legacy-enter-perf.log"
check_eq "A/B ② perf.log に dispatch:OrchestratorRespond の専有が出る" "$ROUNDS" "$(perf_hits 'dispatch:OrchestratorRespond')"
PERF="$TMP/data/perf.log"

echo
echo "== 応答・監査記録・断りは修正前（A/B）と同じ =="
for n in $(seq 1 "$ROUNDS"); do
  check_eq "① 応答（round ${n}）" "$(cat "$TMP/legacy-number-respond-$n.norm")" "$(cat "$TMP/new-number-respond-$n.norm")"
  check_eq "② 応答（round ${n}）" "$(cat "$TMP/legacy-enter-respond-$n.norm")" "$(cat "$TMP/new-enter-respond-$n.norm")"
done
check_eq "③ 下見" "$(cat "$TMP/legacy-edge-preview.norm")" "$(cat "$TMP/new-edge-preview.norm")"
check_eq "③ 合わない choice の断り" "$(cat "$TMP/legacy-edge-nomatch.out")" "$(cat "$TMP/new-edge-nomatch.out")"
check_eq "③ ダイアログが無い画面の断り" "$(cat "$TMP/legacy-edge-nodialog.out")" "$(cat "$TMP/new-edge-nodialog.out")"
check_eq "③ 無いペインの断り" "$(cat "$TMP/legacy-edge-nopane.out")" "$(cat "$TMP/new-edge-nopane.out")"
check_eq "persist.log の応答の記録（ペイン ID を落として）" "$(cat "$TMP/legacy-audit.txt")" "$(cat "$TMP/new-audit.txt")"
check_ge "persist.log の応答の記録の行数（①② の各回 + ③ の 1 回）" $((ROUNDS * 2 + 1)) "$(grep -c . "$TMP/new-audit.txt" | tr -dc '0-9')"

# 証拠を残す（TAKO_2639_EVIDENCE_DIR=<dir>）
if [ -n "${TAKO_2639_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2639_EVIDENCE_DIR"
  for f in "$TMP"/new-* "$TMP"/legacy-*; do
    [ -f "$f" ] && command cp -f "$f" "$TAKO_2639_EVIDENCE_DIR/"
  done
fi

echo
echo "=========================================="
echo " PASS: $PASS / FAIL: $FAIL"
echo "=========================================="
[ "$FAIL" -eq 0 ]
