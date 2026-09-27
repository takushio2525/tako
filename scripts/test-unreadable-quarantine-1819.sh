#!/usr/bin/env bash
# test-unreadable-quarantine-1819.sh — 実行中に 2 回目以降に壊れた設定の保全の実経路テスト（#1819）
#
# 隔離した data / HOME で**実 tako-app** を立て、実行中の GUI に対して
#
#   ① settings.json を中身を変えて 3 回壊し、そのたびに GUI の dispatch
#      （`tako run-default` = load → 既定値 → save の順）を通す。
#      **3 回ぶんの壊れた中身がすべて退避先に残り**、persist.log に毎回 1 行記録される
#   ② 同じ中身で 2 回壊しても退避は増えない（同じ中身の写しを積まない）
#   ③ 人が `.unreadable.bak` を消したあとにまた壊すと、その中身が `.unreadable.bak` へ残る
#   ④ 壊れたまま GUI を再起動しても、その中身が残り記録される
#   ⑤ 上限: 退避が上限（10 本）に達したあとも、最初の 1 本と最新の中身は残り、本数は上限を超えない
#   ⑥ recent.json（`tako open-in dir`）/ remote/shortcuts.json（`tako remote shortcuts add`）も
#      同じ退避の 1 実装を通るので、2 回目に壊した中身が残る
#
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-unreadable-quarantine-1819.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる前提の実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1819-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1819-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

DATA="$TAKO_DATA_DIR"
SETTINGS="$DATA/settings.json"
LOG="$DATA/persist.log"

# 退避先の一覧（`<name>.unreadable.bak` と `<name>.unreadable.<N>.bak`）
quarantines() {
  local name="$1" dir="${2:-$DATA}"
  (cd "$dir" 2>/dev/null && ls -1 "$name".unreadable*.bak 2>/dev/null) | sort
}
quarantine_count() { quarantines "$@" | grep -c . | tr -d ' '; }
# その印を含む退避先の名前（無ければ空）。**単語一致**で探す（`CAP_1` が `CAP_13` に当たらない）
quarantine_holding() {
  local name="$1" mark="$2" dir="${3:-$DATA}" f
  for f in $(quarantines "$name" "$dir"); do
    if grep -qwF "$mark" "$dir/$f"; then
      echo "$f"
      return 0
    fi
  done
  return 0
}
# 退避先の中身が壊した中身とバイト一致するか
same_bytes() { cmp -s "$1" "$2"; }
# `grep -c` は 0 件でも `0` を出して終了コード 1 を返すので `|| echo 0` を重ねない
log_count() {
  local c
  c=$(grep -c "settings.json を解釈できない" "$LOG" 2>/dev/null) || true
  echo "${c:-0}"
}
# 退避先ごとに「どの印の中身か」を並べる（before / after の実出力をそのまま証拠にする）
show_state() {
  local f line=""
  for f in $(quarantines settings.json); do
    line="$line $f($(grep -oE '(MARK|CAP)_[A-Z0-9]+' "$DATA/$f" | head -1))"
  done
  echo "    退避先:${line:- なし}"
  echo "    settings.json の中身の印: $(grep -oE '(MARK|CAP)_[A-Z0-9]+' "$SETTINGS" | head -1)（空 = 既定値で上書き済み）"
  echo "    persist.log の申告行: $(log_count)"
}

# 壊した中身を書く（**閉じ括弧を落とした JSON** = ユーザーが手で書いて打ち損じた形）
break_settings() {
  local mark="$1"
  printf '{\n  "theme": "light",\n  "runner_defaults": { "py": "python3 -X %s" }\n' "$mark" > "$SETTINGS"
  cp "$SETTINGS" "$TMP/broken-$mark.json"
}

# GUI の dispatch で load → save を 1 回通す（RunnerDefaults は GUI 側で実行される）
touch_settings_via_gui() {
  "$TAKO_BIN" run-default "$1" "$2" >/dev/null 2>&1
}

echo "== 隔離 GUI を起こす =="
printf '{ "runner_defaults": { "py": "python3 -X ORIGINAL" } }\n' > "$SETTINGS"
launch_isolated_gui "$TMP/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
echo "  pid=$APP_PID"

echo
echo "== ① 中身を変えて 3 回壊す（毎回 GUI の dispatch が load → save を通す） =="
i=0
for mark in MARK_A MARK_B MARK_C; do
  i=$((i + 1))
  before_log=$(log_count)
  break_settings "$mark"
  touch_settings_via_gui "e$i" "cat"
  echo "  -- ${i} 回目（${mark}）"
  show_state
  held=$(quarantine_holding settings.json "$mark")
  if [ -n "$held" ] && same_bytes "$DATA/$held" "$TMP/broken-$mark.json"; then
    pass "${i} 回目に壊した中身（${mark}）が ${held} にバイト一致で残る"
  else
    fail "${i} 回目に壊した中身（${mark}）がどの退避先にも残っていない"
  fi
  check_eq "${i} 回目も persist.log に 1 行記録される" "$((before_log + 1))" "$(log_count)"
  if grep -qF "e$i" "$SETTINGS" && ! grep -qF "$mark" "$SETTINGS"; then
    pass "${i} 回目: settings.json は既定値 + 今回の変更で上書き済み（= 退避が無ければ消えていた）"
  else
    fail "${i} 回目: settings.json の上書きを観測できない（前提が崩れた）"
  fi
done
held_a=$(quarantine_holding settings.json MARK_A)
check_eq "1 回目の保全は従来どおり settings.json.unreadable.bak" "settings.json.unreadable.bak" "$held_a"

echo
echo "== ② 同じ中身で 2 回壊す（写しを積まない） =="
count_before=$(quarantine_count settings.json)
log_before=$(log_count)
break_settings MARK_B
touch_settings_via_gui "same" "cat"
show_state
check_eq "同じ中身では退避が増えない" "$count_before" "$(quarantine_count settings.json)"
held=$(quarantine_holding settings.json MARK_B)
if [ -n "$held" ]; then pass "MARK_B は ${held} に残ったまま"; else fail "MARK_B が消えた"; fi
check_eq "同じ中身の 2 回目も申告は 1 行（壊れた事実を落とさない）" "$((log_before + 1))" "$(log_count)"

echo
echo "== ③ 人が settings.json.unreadable.bak を消したあとにまた壊す =="
rm -f "$DATA/settings.json.unreadable.bak"
break_settings MARK_D
touch_settings_via_gui "d" "cat"
show_state
held=$(quarantine_holding settings.json MARK_D)
check_eq "消したあとに壊した中身は settings.json.unreadable.bak へ残る" "settings.json.unreadable.bak" "$held"
for mark in MARK_B MARK_C; do
  if [ -n "$(quarantine_holding settings.json "$mark")" ]; then
    pass "${mark} は消されずに残る（人が消したのは 1 本だけ）"
  else
    fail "${mark} が消えた"
  fi
done

echo
echo "== ④ 壊れたまま GUI を再起動する =="
break_settings MARK_E
stop_isolated_gui "$APP_PID"
APP_PID=""
log_before=$(log_count)
launch_isolated_gui "$TMP/app2.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app2.log" || exit 1
touch_settings_via_gui "e" "cat"
show_state
held=$(quarantine_holding settings.json MARK_E)
if [ -n "$held" ] && same_bytes "$DATA/$held" "$TMP/broken-MARK_E.json"; then
  pass "再起動をまたいで壊れていた中身（MARK_E）が ${held} に残る"
else
  fail "再起動をまたいで壊れていた中身（MARK_E）が残っていない"
fi
if [ "$(log_count)" -gt "$log_before" ]; then
  pass "再起動後も persist.log に記録される"
else
  fail "再起動後に記録されない"
fi

echo
echo "== ⑤ 上限（10 本）を超えて壊し続ける =="
rm -f "$DATA"/settings.json.unreadable*.bak
n=0
while [ "$n" -lt 13 ]; do
  n=$((n + 1))
  break_settings "CAP_$n"
  touch_settings_via_gui "c$n" "cat"
done
show_state
check_eq "本数は上限の 10 を超えない" "10" "$(quarantine_count settings.json)"
check_eq "最初の 1 本（CAP_1）は settings.json.unreadable.bak に残る" \
  "settings.json.unreadable.bak" "$(quarantine_holding settings.json CAP_1)"
if [ -n "$(quarantine_holding settings.json CAP_13)" ]; then
  pass "最新（CAP_13）は残る"
else
  fail "最新（CAP_13）が残っていない"
fi
dropped=""
k=0
while [ "$k" -lt 13 ]; do
  k=$((k + 1))
  [ -z "$(quarantine_holding settings.json "CAP_$k")" ] && dropped="$dropped CAP_$k"
done
check_eq "押し出されたのは 2 本目以降の古いほうから 3 本だけ（残りの 10 本はすべて在る）" " CAP_2 CAP_3 CAP_4" "$dropped"
if grep -q "settings.json.unreadable" "$LOG" && grep -q "押し出し" "$LOG"; then
  pass "押し出したことも persist.log に残る"
else
  fail "押し出しの記録が persist.log に無い"
fi

echo
echo "== ⑥ recent.json / remote/shortcuts.json も 2 回目の中身が残る =="
mkdir -p "$TMP/d1" "$TMP/d2"
for k in 1 2; do
  printf '{ "entries": [ { "Directory": { "path": "RECENT_%s" } } \n' "$k" > "$DATA/recent.json"
  "$TAKO_BIN" open-in dir "$TMP/d$k" >/dev/null 2>&1
done
for k in 1 2; do
  if [ -n "$(quarantine_holding recent.json "RECENT_$k")" ]; then
    pass "recent.json: ${k} 回目に壊した中身が $(quarantine_holding recent.json "RECENT_$k") に残る"
  else
    fail "recent.json: ${k} 回目に壊した中身が残っていない（退避先: $(quarantines recent.json | tr '\n' ' ')）"
  fi
done
mkdir -p "$DATA/remote"
for k in 1 2; do
  printf '{ "shortcuts": [ { "path": "SHORTCUT_%s" \n' "$k" > "$DATA/remote/shortcuts.json"
  "$TAKO_BIN" remote shortcuts add "$TMP/d$k" >/dev/null 2>&1
done
for k in 1 2; do
  held=$(quarantine_holding shortcuts.json "SHORTCUT_$k" "$DATA/remote")
  if [ -n "$held" ]; then
    pass "remote/shortcuts.json: ${k} 回目に壊した中身が ${held} に残る"
  else
    fail "remote/shortcuts.json: ${k} 回目に壊した中身が残っていない（退避先: $(quarantines shortcuts.json "$DATA/remote" | tr '\n' ' ')）"
  fi
done

echo
echo "== ⑦ layout.json を中身を変えて 3 回壊し、そのたびに GUI を起動し直す =="
# layout.json は tako が書くファイルで、壊れていると起動時の復元が `layout.json.corrupt` へ
# rename する（#30。rename は前回の `.corrupt` を上書きする）。起動時の自動移行（#916）も
# 同じファイルを退避する。隔離起動は既定で復元を切る（TAKO_PERSIST=0）ので明示して通す
for k in 1 2 3; do
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  printf '{ "version": 1, "tabs": [ "LAYOUT_%s"\n' "$k" > "$DATA/layout.json"
  launch_isolated_gui "$TMP/app-layout-$k.log" TAKO_PERSIST=1 || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-layout-$k.log" || exit 1
done
for k in 1 2 3; do
  where=$(cd "$DATA" && grep -lF "LAYOUT_$k" layout.json* 2>/dev/null | tr '\n' ' ')
  if [ -n "$where" ]; then
    pass "layout.json: ${k} 回目に壊した中身が残る（${where}）"
  else
    fail "layout.json: ${k} 回目に壊した中身がどこにも残っていない"
  fi
done

echo
echo "== ⑧ projects.yaml / profiles/*.yaml は壊れていても上書きしない（fail-loud） =="
ORCH="$TAKO_ORCHESTRATOR_DIR"
mkdir -p "$ORCH/profiles"
for k in 1 2; do
  printf 'projects:\n  demo: { cwd: "PROJECTS_%s"\n' "$k" > "$ORCH/projects.yaml"
  cp "$ORCH/projects.yaml" "$TMP/projects-$k.yaml"
  if "$TAKO_BIN" orchestrator projects add --key "k$k" --cwd "$TMP/d1" >/dev/null 2>&1; then
    fail "projects.yaml: ${k} 回目: 壊れているのに add が成功した"
  else
    pass "projects.yaml: ${k} 回目: 壊れているので add は失敗する"
  fi
  if same_bytes "$ORCH/projects.yaml" "$TMP/projects-$k.yaml"; then
    pass "projects.yaml: ${k} 回目に壊した中身は元の場所にバイト一致で残る（上書きしない）"
  else
    fail "projects.yaml: ${k} 回目に壊した中身が書き換えられた"
  fi
  "$TAKO_BIN" migrate run >/dev/null 2>&1
  held=$(quarantine_holding projects.yaml "PROJECTS_$k" "$ORCH")
  if [ -n "$held" ]; then
    pass "projects.yaml: tako migrate run の退避にも ${k} 回目の中身が残る（${held}）"
  else
    fail "projects.yaml: tako migrate run の退避に ${k} 回目の中身が無い（退避先: $(quarantines projects.yaml "$ORCH" | tr '\n' ' ')）"
  fi
done
for k in 1 2; do
  printf 'remote_control: true\nmodel: "PROFILE_%s\n' "$k" > "$ORCH/profiles/default.yaml"
  cp "$ORCH/profiles/default.yaml" "$TMP/profile-$k.yaml"
  "$TAKO_BIN" orchestrator profiles set default --remote-control false >/dev/null 2>&1
  if same_bytes "$ORCH/profiles/default.yaml" "$TMP/profile-$k.yaml"; then
    pass "profiles/default.yaml: ${k} 回目に壊した中身は元の場所にバイト一致で残る（profiles set が上書きしない）"
  else
    fail "profiles/default.yaml: ${k} 回目に壊した中身が書き換えられた"
  fi
done

echo
echo "== persist.log の申告行（本文） =="
grep "解釈できない\|移行できず" "$LOG" | sed 's|'"$TMP"'|<tmp>|g' | tail -20

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
