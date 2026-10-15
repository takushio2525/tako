#!/usr/bin/env bash
# test-solo-tab-title-2127.sh — solo のタブ名が役割名の手動名で固定されないこと（#2127）
#
# 実 CLI（`tako solo` / `tako master` / `tako tab rename`）と実 tako-app を、隔離した
# data / orchestrator / tmux / discovery / HOME で走らせて確かめる:
#
#   ① A/B の旧挙動（TAKO_2120_LEGACY=1 の CLI）: インライン起動でタブ名が `solo` /
#      `solo-<名前>` の手動名になり、solo の自動名（source: auto）が弾かれる = #2127 の症状
#   ② その layout を新しい GUI で復元すると、旧版が付けた solo の手動名だけが外れて
#      solo の作業フォルダのリポジトリ名になる（ユーザーが付けた名前・solo のいないタブの
#      `solo`・master のタブ名（#2120 の規則）はそのまま）
#   ③ 新しい起動: リポジトリの中 → リポジトリ名 / リポジトリの外 → フォルダ名 /
#      HOME → タブ名に触らない。`--tab` の solo はタブ名を付けたフォルダで動く
#   ④ solo の自動名で主題が付き、ユーザーが手で付けた名前は上書きされない
#      （インライン起動でもユーザーの名前が残る）
#   ⑤ アプリの自動命名（ヒューリスティック）が solo のタブ名を作らない（素のシェルのタブは命名される）
#   ⑥ タブバーの画像（visual-test `master-tab-title` の ⑤）: 旧挙動の復元は solo の判定で落ち、
#      新しい復元は緑
#
# **本番の GUI・master・タブには一切触らない**（env は全部 mktemp 配下、claude はスタブ、
# 自動命名の claude は即失敗するスタブ = 実アカウントの会話を増やさない）。
#
# 使い方: bash scripts/test-solo-tab-title-2127.sh
#   OUT_DIR=<dir> を渡すと画像をそこへ残す（既定は一時 dir = 終了時に消える）
set -euo pipefail

# 本番 GUI を指す env を最初に落とす（tako のペインの中から走らせると継承される）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_ORCHESTRATOR_ROLE TAKO_TAB_ID

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-2127-XXXXXX")"
OUT_DIR="${OUT_DIR:-$TMP/out}"
SOCKET="tako-2127-$$"
APP_PID=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$SOCKET" kill-server 2>/dev/null || true
  rm -rf "$TMP"
}
# 途中で死んだテストを「緑」に化けさせない（#1864）
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup

# --- claude のスタブ ----------------------------------------------------------
# solo / master のペインで走るのはプロファイルの env で PATH を差し替えたこのスタブ
STUB_BIN="$TMP/bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/claude" <<'STUB'
#!/usr/bin/env bash
echo "STUB_CLAUDE_STARTED"
sleep 600
STUB
chmod +x "$STUB_BIN/claude"
# 自動命名（FR-2.12）の claude は即失敗させる = ヒューリスティック命名へ落ちる（実呼び出しをしない）
FAIL_BIN="$TMP/bin-fail"
mkdir -p "$FAIL_BIN"
printf '#!/bin/sh\nexit 1\n' > "$FAIL_BIN/claude"
chmod +x "$FAIL_BIN/claude"

# --- 対象バイナリ（visual-test の節を使うので feature 付きで組む）------------------
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
echo "visual-test 版の tako-app と tako をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
isolated_gui_bins || exit 1
# タブが 10 枚前後並ぶので、窓を横に広く取る（タブバーの画像に全部収める）
ISOLATED_GUI_BOUNDS=${TAKO_2127_BOUNDS:-2400,900}

# --- 隔離した環境 -------------------------------------------------------------
# HOME も一時 dir へ（ビルドの後で替える = ツールチェーンを取り直さない）。solo の初期名は
# HOME を天井にして辿るので、HOME の中にリポジトリを置く（実ユーザー名も画像に写らない）
export HOME="$TMP/home"
export PATH="$STUB_BIN:$PATH"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$SOCKET"
# solo の作業フォルダ: リポジトリ（s2127）・その奥（src）・リポジトリの外（notes2127）
REPO="$HOME/dev/s2127"
NOTES="$HOME/notes2127"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles" \
  "$TAKO_ORCHESTRATOR_DIR/solo-profiles" "$REPO/src" "$NOTES" "$TMP/proj" "$TMP/claude-cfg" "$OUT_DIR"
git -C "$REPO" init -q
# 画像に実ユーザー名・ホスト名を写さない（zsh の既定プロンプトは %n@%m を出す）
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# solo のプロファイル（default / f2127）と、master の専用プロファイル d2127（管轄 p2127）。
# master は「solo の変更が master の規則（#2120）に触らない」ことを見るためだけに立てる
cat > "$TAKO_ORCHESTRATOR_DIR/projects.yaml" <<YAML
projects:
  p2127:
    cwd: "$TMP/proj"
YAML
TMUX_DIR="$(dirname "$(command -v tmux)")"
profile_body() {
  echo "effort: high"
  echo "env:"
  echo "  PATH: \"$STUB_BIN:$TMUX_DIR:/usr/bin:/bin\""
  echo "  CLAUDE_CONFIG_DIR: \"$TMP/claude-cfg\""
}
for name in default f2127; do
  profile_body > "$TAKO_ORCHESTRATOR_DIR/solo-profiles/$name.yaml"
done
profile_body > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml"
{ profile_body; printf "projects:\n- p2127\n"; } > "$TAKO_ORCHESTRATOR_DIR/profiles/d2127.yaml"

T() { "$TAKO_BIN" "$@"; }
list_json() { T list 2>/dev/null; }
# tab_field <tab> <title|title_source>
tab_field() {
  list_json | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    if t["id"]==int(sys.argv[1]): print(t.get(sys.argv[2]) or ""); raise SystemExit
print("")' "$1" "$2"
}
# pane_field <pane> <role|cwd>
pane_field() {
  list_json | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"]==int(sys.argv[1]): print(p.get(sys.argv[2]) or ""); raise SystemExit
print("")' "$1" "$2"
}
tab_of_pane() {
  list_json | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"]==int(sys.argv[1]): print(t["id"]); raise SystemExit
print("")' "$1"
}
first_pane_of_tab() {
  list_json | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    if t["id"]==int(sys.argv[1]): print(t["panes"][0]["id"]); raise SystemExit
print("")' "$1"
}
tab_count() { list_json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))'; }
new_tab() { T tab new "$@" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])'; }
wait_role() {
  local pane="$1" role="$2" i
  for i in $(seq 1 100); do
    [ "$(pane_field "$pane" role)" = "$role" ] && return 0
    sleep 0.1
  done
  return 1
}
# 実体パス（macOS の /var → /private/var。ペインの cwd は実体で返る）
realdir() { (cd "$1" && pwd -P); }

# `tako solo` を <dir> から新しいタブで立て、そのペイン ID を返す（stderr の「ペイン N」から読む）。
# 出力は毎回 $LAUNCH_LOG へ（`$(…)` の中で呼ぶので変数では返せない）
LAUNCH_LOG="$TMP/launch-last.log"
launch_solo_tab() {
  local dir="$1"
  shift
  (cd "$dir" && "$TAKO_BIN" solo "$@" --tab) > "$LAUNCH_LOG" 2>&1 || { cat "$LAUNCH_LOG"; return 1; }
  grep -o 'ペイン [0-9]*' "$LAUNCH_LOG" | head -1 | grep -o '[0-9]*'
}
# <dir> で開いたタブのシェルからインラインで `tako solo` を打ち、そのタブ ID を返す。
# `$2` はコマンドの前に置く env（①で `TAKO_2120_LEGACY=1`）。GUI へは渡さない:
# GUI の env は隔離 tmux サーバーへ引き継がれ、後のフェーズのペインの CLI まで旧挙動になる
launch_solo_inline() {
  local tab="$1" envs="$2" role="$3"
  shift 3
  local pane
  pane="$(first_pane_of_tab "$tab")"
  sleep 1
  T send --pane "$pane" "$envs \"$TAKO_BIN\" solo $*" >/dev/null
  wait_role "$pane" "$role" || { echo "    solo のペインにならない: role='$(pane_field "$pane" role)'"; return 1; }
}

start_gui() {
  local log="$1"
  shift
  launch_isolated_gui "$log" TAKO_PERSIST=1 TAKO_CLAUDE_BIN="$FAIL_BIN/claude" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || { tail -20 "$log"; exit 1; }
}
stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
# layout.json に「この名前・この出どころ」のタブがあるか
layout_has() {
  python3 -c 'import json,sys
try:
    d=json.load(open(sys.argv[1]))
except Exception:
    raise SystemExit(1)
ok=any(t.get("title")==sys.argv[2] and t.get("title_source")==sys.argv[3] for t in d.get("tabs",[]))
raise SystemExit(0 if ok else 1)' "$TAKO_DATA_DIR/layout.json" "$1" "$2"
}
wait_layout_has() {
  local i
  for i in $(seq 1 100); do
    layout_has "$1" "$2" && return 0
    sleep 0.1
  done
  return 1
}
# visual-test の節で、復元したタブバーを撮る（節が終わるまで待つ）
snap() {
  local arm="$1" expect="$2"
  shift 2
  launch_isolated_gui "$TMP/visual-$arm.log" TAKO_PERSIST=1 TAKO_CLAUDE_BIN="$FAIL_BIN/claude" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=master-tab-title TAKO_2120_EXPECT_TABS="$expect" \
    TAKO_VISUAL_DUMP_DIR="$OUT_DIR/$arm" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 600); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  grep -E "TAKO_VISUAL_TAB|TAKO_VISUAL_DUMP_FILE|TAKO_VISUAL_TEST_OK|TAKO_APP_SELF_TEST_FAILED" \
    "$TMP/visual-$arm.log" | sed 's/^/    /'
}

echo
echo "=== ① A/B の旧挙動（TAKO_2120_LEGACY=1 の CLI）: solo のタブが役割名の手動名になり、自動名が弾かれる ==="
start_gui "$TMP/app-1.log"
pass "隔離 tako-app が起動した（pid ${APP_PID}）"
L_PLAIN="$(new_tab --cwd "$REPO")"
launch_solo_inline "$L_PLAIN" TAKO_2120_LEGACY=1 solo || fail "旧挙動の solo が立たない"
check_eq "solo のタブ名が solo（旧）" "solo" "$(tab_field "$L_PLAIN" title)"
check_eq "それは手動名（旧）" "manual" "$(tab_field "$L_PLAIN" title_source)"
T tab rename --tab "$L_PLAIN" --source auto CI修正 >/dev/null
check_eq "solo の自動名が手動名に弾かれる = #2127 の症状" "solo" "$(tab_field "$L_PLAIN" title)"
# 名前付きのプロファイル・リポジトリの奥で立てた solo
L_NAMED="$(new_tab --cwd "$REPO/src")"
launch_solo_inline "$L_NAMED" TAKO_2120_LEGACY=1 solo:f2127 -f2127 || fail "旧挙動の solo -f2127 が立たない"
check_eq "solo -f2127 のタブ名が solo-f2127（旧）" "solo-f2127" "$(tab_field "$L_NAMED" title)"
# ユーザーが solo のタブへ手で付けた名前 / solo のいないタブの「solo」
L_USR="$(new_tab --cwd "$REPO")"
launch_solo_inline "$L_USR" TAKO_2120_LEGACY=1 solo || fail "旧挙動の solo（ユーザー名用）が立たない"
T tab rename --tab "$L_USR" 自分の名前 >/dev/null
check_eq "手で付けた名前" "自分の名前" "$(tab_field "$L_USR" title)"
L_NOSOLO="$(new_tab --title solo)"
check_eq "solo のいないタブの solo は手動名" "manual" "$(tab_field "$L_NOSOLO" title_source)"
# master（#2120 の規則で付いた名前）
M_PANE="$("$TAKO_BIN" master -d2127 --tab 2>&1 | grep -o 'ペイン [0-9]*' | head -1 | grep -o '[0-9]*')"
M_TAB="$(tab_of_pane "$M_PANE")"
check_eq "master のタブは管轄プロジェクトの自動名（#2120）" "p2127/auto" \
  "$(tab_field "$M_TAB" title)/$(tab_field "$M_TAB" title_source)"
if wait_layout_has 自分の名前 manual && wait_layout_has solo-f2127 manual && wait_layout_has solo manual; then
  pass "旧挙動の layout.json が保存された"
else
  fail "layout.json が保存されない"
fi
LEGACY_TABS="$(tab_count)"
stop_gui

echo
echo "=== ⑥-旧: 旧挙動のまま復元したタブバー（節は solo の判定で落ちるはず）==="
snap legacy "$LEGACY_TABS" TAKO_2120_LEGACY=1
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test master-tab-title: solo のいるタブ" "$TMP/visual-legacy.log"; then
  pass "旧挙動では節が落ちる（solo の役割名の手動名で固定）"
else
  fail "旧挙動でも節が落ちない（検出力が無い）"
fi
if layout_has solo-f2127 manual; then
  pass "旧挙動の復元は layout を変えない"
else
  fail "旧挙動の復元で layout が変わった"
fi

echo
echo "=== ② 新しい GUI で旧形式の layout を復元: solo の手動名だけが外れてリポジトリ名になる ==="
start_gui "$TMP/app-new.log"
check_eq "タブの枚数が保たれる" "$LEGACY_TABS" "$(tab_count)"
check_eq "solo のタブ = 作業フォルダのリポジトリ名" "s2127/auto" \
  "$(tab_field "$L_PLAIN" title)/$(tab_field "$L_PLAIN" title_source)"
check_eq "奥（src）で立てた solo -f2127 のタブもリポジトリ名" "s2127/auto" \
  "$(tab_field "$L_NAMED" title)/$(tab_field "$L_NAMED" title_source)"
check_eq "手で付けた名前は手動のまま残る" "自分の名前/manual" \
  "$(tab_field "$L_USR" title)/$(tab_field "$L_USR" title_source)"
check_eq "solo のいないタブの solo は残る" "solo/manual" \
  "$(tab_field "$L_NOSOLO" title)/$(tab_field "$L_NOSOLO" title_source)"
check_eq "master のタブ名は変わらない" "p2127/auto" \
  "$(tab_field "$M_TAB" title)/$(tab_field "$M_TAB" title_source)"
RELEASED="$(grep -c '旧版が付けた solo の手動名を解除' "$TAKO_DATA_DIR/persist.log" 2>/dev/null || true)"
check_eq "解除が persist.log に 2 件残る" "2" "${RELEASED:-0}"
T tab rename --tab "$L_PLAIN" --source auto CI修正 >/dev/null
check_eq "解除後は solo の自動名が通る" "CI修正" "$(tab_field "$L_PLAIN" title)"

echo
echo "=== ③ 新しい起動: リポジトリ名 / フォルダ名 / HOME は触らない ==="
REPO_PANE="$(launch_solo_tab "$REPO/src")"
REPO_TAB="$(tab_of_pane "$REPO_PANE")"
check_eq "リポジトリの奥で立てた solo のタブ = リポジトリ名" "s2127/auto" \
  "$(tab_field "$REPO_TAB" title)/$(tab_field "$REPO_TAB" title_source)"
if grep -q "タブ 's2127'" "$LAUNCH_LOG"; then
  pass "起動の表示もリポジトリ名（$(grep -o "タブ '[^']*'（ペイン [0-9]*）" "$LAUNCH_LOG")）"
else
  fail "起動の表示: $(grep 起動しました "$LAUNCH_LOG" || true)"
fi
wait_role "$REPO_PANE" solo || true
check_eq "solo の role" "solo" "$(pane_field "$REPO_PANE" role)"
for _ in $(seq 1 50); do
  [ -n "$(pane_field "$REPO_PANE" cwd)" ] && break
  sleep 0.1
done
check_eq "--tab の solo はタブ名を付けたフォルダで動く" "$(realdir "$REPO/src")" \
  "$(pane_field "$REPO_PANE" cwd)"
NOTES_PANE="$(launch_solo_tab "$NOTES" -f2127)"
NOTES_TAB="$(tab_of_pane "$NOTES_PANE")"
check_eq "リポジトリの外で立てた solo のタブ = フォルダ名" "notes2127/auto" \
  "$(tab_field "$NOTES_TAB" title)/$(tab_field "$NOTES_TAB" title_source)"
HOME_PANE="$(launch_solo_tab "$HOME")"
HOME_TAB="$(tab_of_pane "$HOME_PANE")"
check_eq "HOME で立てた solo はタブ名に触らない（出どころ default）" "default" \
  "$(tab_field "$HOME_TAB" title_source)"
case "$(tab_field "$HOME_TAB" title)" in
  solo*) fail "HOME の solo のタブ名が役割名: $(tab_field "$HOME_TAB" title)" ;;
  *) pass "HOME の solo のタブ名が役割名でない（'$(tab_field "$HOME_TAB" title)'）" ;;
esac
if grep -q "新しいタブ（ペイン" "$LAUNCH_LOG"; then
  pass "起動の表示は名前の無い表記（$(grep -o "新しいタブ（ペイン [0-9]*）" "$LAUNCH_LOG")）"
else
  fail "起動の表示: $(grep 起動しました "$LAUNCH_LOG" || true)"
fi
# インライン起動（素のタブのシェルで tako solo を打つ）
INL="$(new_tab --cwd "$NOTES")"
launch_solo_inline "$INL" "" solo || fail "インラインの solo が立たない"
check_eq "インライン起動でもフォルダ名の自動名" "notes2127/auto" \
  "$(tab_field "$INL" title)/$(tab_field "$INL" title_source)"

echo
echo "=== ④ solo の自動名で主題が付き、手で付けた名前は上書きされない ==="
T tab rename --tab "$REPO_TAB" --source auto 数理モデリング >/dev/null
check_eq "solo の自動名で主題" "数理モデリング" "$(tab_field "$REPO_TAB" title)"
T tab rename --tab "$REPO_TAB" 自分で付けた >/dev/null
T tab rename --tab "$REPO_TAB" --source auto 別の主題 >/dev/null
check_eq "手で付けた名前は自動名で上書きされない" "自分で付けた" "$(tab_field "$REPO_TAB" title)"
USR="$(new_tab --cwd "$REPO" --title ユーザーの名前)"
launch_solo_inline "$USR" "" solo || fail "ユーザーの名前のタブで solo が立たない"
check_eq "ユーザーの付けた名前はインライン起動でも残る" "ユーザーの名前/manual" \
  "$(tab_field "$USR" title)/$(tab_field "$USR" title_source)"

echo
echo "=== ⑤ アプリの自動命名は solo のタブ名を作らない（素のシェルのタブは命名される）==="
PLAIN="$(new_tab)"
PLAIN_PANE="$(first_pane_of_tab "$PLAIN")"
T send --pane "$PLAIN_PANE" "cd \"$TMP/proj\"" >/dev/null
for _ in $(seq 1 300); do
  [ "$(tab_field "$PLAIN" title_source)" = "auto" ] && break
  sleep 0.1
done
if [ "$(tab_field "$PLAIN" title_source)" = "auto" ]; then
  pass "素のシェルのタブはアプリが命名した（'$(tab_field "$PLAIN" title)'）= 自動命名は動いている"
else
  fail "素のシェルのタブが 30 秒で命名されない（自動命名が動いていない = ⑤ を確かめられない）"
fi
check_eq "同じ時間で solo のタブ名は solo が付けたまま" "CI修正" "$(tab_field "$L_PLAIN" title)"
check_eq "起動で付けたフォルダ名もそのまま" "notes2127" "$(tab_field "$NOTES_TAB" title)"
NEW_TABS="$(tab_count)"
wait_layout_has 自分で付けた manual || true
stop_gui

echo
echo "=== ⑥-新: 新しい復元のタブバー（節は緑のはず）==="
snap new "$NEW_TABS"
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual-new.log"; then
  pass "新しい復元では節が緑"
else
  fail "新しい復元で節が落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/visual-new.log" | head -1)"
fi

echo
echo "画像: $OUT_DIR"
echo "=== 結果: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
