#!/usr/bin/env bash
# test-master-tab-title-2120.sh — master のタブ名が役割名の手動名で固定されないこと（#2120）
#
# 実 CLI（`tako master` / `tako orchestrator adopt` / `tako tab rename` / `tako orchestrator handoff`）と
# 実 tako-app を、隔離した data / orchestrator / tmux / discovery で走らせて確かめる:
#
#   ① A/B の旧挙動（TAKO_2120_LEGACY=1）: 起動でタブ名が `master` / `master-<名前>` の手動名になり、
#      master の自動名（source: auto）が弾かれる = #2120 の症状を再現する
#   ② その layout を新しい GUI で復元すると、旧版が付けた役割名の手動名だけが外れる
#      （専用 master は管轄プロジェクトの名前・汎用は手動だけ外す・ユーザーが付けた別の名前と
#      master のいないタブの `master` は残る）
#   ③ 新しい起動: 専用 master はプロジェクト名、汎用 master はタブ名に触らず adopt でプロジェクト名
#   ④ master の自動名で主題が付き、ユーザーが手で付けた名前は上書きされない
#      （起動・自動名のどちらでも。インライン起動でユーザーの名前が残る）
#   ⑤ 引き継ぎ（handoff）で後任が同じタブに立ってもタブ名が戻らない
#   ⑥ アプリの自動命名（ヒューリスティック）が master のタブ名を作らない（素のシェルのタブは命名される）
#   ⑦ タブバーの画像（visual-test `master-tab-title`）: 旧挙動の復元は ④ で落ち、新しい復元は緑
#
# **本番の GUI・master・タブには一切触らない**（env は全部 mktemp 配下、claude はスタブ、
# 自動命名の claude は即失敗するスタブ = 実アカウントの会話を増やさない）。
#
# 使い方: bash scripts/test-master-tab-title-2120.sh
#   OUT_DIR=<dir> を渡すと画像をそこへ残す（既定は一時 dir = 終了時に消える）
set -euo pipefail

# 本番 GUI を指す env を最初に落とす（tako のペインの中から走らせると継承される）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_ORCHESTRATOR_ROLE TAKO_TAB_ID

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-2120-XXXXXX")"
OUT_DIR="${OUT_DIR:-$TMP/out}"
SOCKET="tako-2120-$$"
APP_PID=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux_socket_kill "$SOCKET"
  rm -rf "$TMP"
}
# 途中で死んだテストを「緑」に化けさせない（#1864）
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup

# --- claude のスタブ ----------------------------------------------------------
# master のペインで走るのはプロファイルの env で PATH を差し替えたこのスタブ
# （preflight はログインシェルの PATH で実物を見つけるが、起動はしない）
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
isolated_gui_bins visual-test || exit 1
# タブが 9 枚並ぶので、窓を横に広く取る（タブバーの画像に全部収める）
ISOLATED_GUI_BOUNDS=${TAKO_2120_BOUNDS:-2400,900}

# --- 隔離した環境 -------------------------------------------------------------
# HOME も一時 dir へ（ビルドの後で替える = ツールチェーンを取り直さない）。最初のタブが
# ホームのフォルダ名で自動命名されて、画像に実ユーザー名が写るのを防ぐ
export HOME="$TMP/home"
# claude の所在確認（preflight）はスタブで足りる（実物は起動もしない）
export PATH="$STUB_BIN:$PATH"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$SOCKET"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles" \
  "$TMP/proj" "$TMP/claude-cfg" "$OUT_DIR"
# 画像に実ユーザー名・ホスト名を写さない（zsh の既定プロンプトは %n@%m を出す）
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# プロジェクト p2120 と、それを管轄する専用プロファイル d2120（プロファイル名とプロジェクト名が
# 違う = 本番の takodev → tako と同じ形）。CLAUDE_CONFIG_DIR は引き継ぎの事前信頼を一時 dir へ向ける
cat > "$TAKO_ORCHESTRATOR_DIR/projects.yaml" <<YAML
projects:
  p2120:
    cwd: "$TMP/proj"
YAML
# PATH には tmux の置き場も入れる（引き継ぎは後任ペインの spawn にもプロファイルの env を使う。
# tmux が見つからないと後任の PTY が立たない）
TMUX_DIR="$(dirname "$(command -v tmux)")"
for name in default d2120; do
  {
    echo "effort: high"
    [ "$name" = d2120 ] && printf "projects:\n- p2120\n"
    echo "env:"
    echo "  PATH: \"$STUB_BIN:$TMUX_DIR:/usr/bin:/bin\""
    echo "  CLAUDE_CONFIG_DIR: \"$TMP/claude-cfg\""
  } > "$TAKO_ORCHESTRATOR_DIR/profiles/$name.yaml"
done

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
pane_count_of_tab() {
  list_json | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    if t["id"]==int(sys.argv[1]): print(len(t["panes"])); raise SystemExit
print(0)' "$1"
}
role_of_pane() {
  list_json | python3 -c 'import json,sys
d=json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"]==int(sys.argv[1]): print(p.get("role") or ""); raise SystemExit
print("")' "$1"
}
tab_count() { list_json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))'; }

# `tako master` を新しいタブで立て、そのペイン ID を返す（stderr の「ペイン N」から読む）。
# 出力は毎回 $LAUNCH_LOG へ（`$(…)` の中で呼ぶので変数では返せない）。
# `LAUNCH_ENV` は CLI にだけ渡す env（①で `TAKO_2120_LEGACY=1`）。GUI へは渡さない:
# GUI の env は隔離 tmux サーバーへ引き継がれ、後のフェーズのペインの CLI まで旧挙動になる
LAUNCH_LOG="$TMP/launch-last.log"
LAUNCH_ENV=""
launch_master() {
  env $LAUNCH_ENV "$TAKO_BIN" master "$@" --tab > "$LAUNCH_LOG" 2>&1 || { cat "$LAUNCH_LOG"; return 1; }
  grep -o 'ペイン [0-9]*' "$LAUNCH_LOG" | sed -n 1p | grep -o '[0-9]*'
}

start_gui() {
  local log="$1"
  shift
  # 自動命名が動くことを確かめる段がある（失敗する偽の claude で名前を付けさせる）ので明示で入れる（既定は止める = #1970）
  launch_isolated_gui "$log" TAKO_PERSIST=1 TAKO_AUTO_RENAME=1 TAKO_CLAUDE_BIN="$FAIL_BIN/claude" ${1+"$@"} || exit $?
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
# layout.json に載るまで待つ（保存は変化検出つきの定期保存）
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
echo "=== ① A/B の旧挙動（TAKO_2120_LEGACY=1 の CLI）: 起動で役割名の手動名になり、自動名が弾かれる ==="
start_gui "$TMP/app-1.log"
pass "隔離 tako-app が起動した（pid ${APP_PID}）"
LAUNCH_ENV="TAKO_2120_LEGACY=1"
L_DED_PANE="$(launch_master -d2120)"
L_DED="$(tab_of_pane "$L_DED_PANE")"
check_eq "専用 master のタブ名が master-d2120（旧）" "master-d2120" "$(tab_field "$L_DED" title)"
check_eq "それは手動名（旧）" "manual" "$(tab_field "$L_DED" title_source)"
L_GEN_PANE="$(launch_master)"
L_GEN="$(tab_of_pane "$L_GEN_PANE")"
check_eq "汎用 master のタブ名が master（旧）" "master" "$(tab_field "$L_GEN" title)"
T tab rename --tab "$L_GEN" --source auto アジャイル >/dev/null
check_eq "master の自動名が手動名に弾かれる = #2120 の症状" "master" "$(tab_field "$L_GEN" title)"
# ユーザー（応急の master）が手で付けた名前 / master のいないタブの「master」
L_USR_PANE="$(launch_master)"
L_USR="$(tab_of_pane "$L_USR_PANE")"
T tab rename --tab "$L_USR" my-site >/dev/null
check_eq "手で付けた名前" "my-site" "$(tab_field "$L_USR" title)"
L_BRANCH="$(T tab new --title master 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
check_eq "master のいないタブの master（ブランチ名など）" "manual" "$(tab_field "$L_BRANCH" title_source)"
if wait_layout_has my-site manual && wait_layout_has master-d2120 manual; then
  pass "旧挙動の layout.json が保存された"
else
  fail "layout.json が保存されない"
fi
cp "$TAKO_DATA_DIR/layout.json" "$TMP/layout-legacy.json"
LEGACY_TABS="$(tab_count)"
stop_gui
LAUNCH_ENV=""

echo
echo "=== ⑦-旧: 旧挙動のまま復元したタブバー（節は ④ で落ちるはず）==="
snap legacy "$LEGACY_TABS" TAKO_2120_LEGACY=1
if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test master-tab-title: master のいるタブ" "$TMP/visual-legacy.log"; then
  pass "旧挙動では節が落ちる（役割名の手動名で固定）"
else
  fail "旧挙動でも節が落ちない（検出力が無い）"
fi
# 撮影の GUI が layout を書き換えていない（旧挙動なので解除しない）
if layout_has master-d2120 manual; then
  pass "旧挙動の復元は layout を変えない"
else
  fail "旧挙動の復元で layout が変わった"
fi

echo
echo "=== ② 新しい GUI で旧形式の layout を復元: 役割名の手動名だけが外れる ==="
start_gui "$TMP/app-new.log"
check_eq "タブの枚数が保たれる" "$LEGACY_TABS" "$(tab_count)"
check_eq "専用 master のタブ = 管轄プロジェクトの名前" "p2120" "$(tab_field "$L_DED" title)"
check_eq "それは自動名" "auto" "$(tab_field "$L_DED" title_source)"
check_eq "汎用 master のタブは手動が外れる" "default" "$(tab_field "$L_GEN" title_source)"
check_eq "手で付けた名前は残る" "my-site" "$(tab_field "$L_USR" title)"
check_eq "手で付けた名前は手動のまま" "manual" "$(tab_field "$L_USR" title_source)"
check_eq "master のいないタブの master は残る" "manual" "$(tab_field "$L_BRANCH" title_source)"
if grep -q "旧版が付けた役割名の手動名を解除" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
  pass "解除が persist.log に残る（$(grep -c '旧版が付けた役割名の手動名を解除' "$TAKO_DATA_DIR/persist.log") 件）"
else
  fail "解除が persist.log に残らない"
fi
T tab rename --tab "$L_GEN" --source auto アジャイル >/dev/null
check_eq "解除後は master の自動名が通る" "アジャイル" "$(tab_field "$L_GEN" title)"

echo
echo "=== ③ 新しい起動: 専用はプロジェクト名、汎用は触らず adopt でプロジェクト名 ==="
DED_PANE="$(launch_master -d2120)"
DED="$(tab_of_pane "$DED_PANE")"
check_eq "専用 master のタブ = p2120" "p2120" "$(tab_field "$DED" title)"
check_eq "それは自動名" "auto" "$(tab_field "$DED" title_source)"
if grep -q "タブ 'p2120'" "$LAUNCH_LOG"; then
  pass "起動の表示もプロジェクト名（$(grep -o "タブ '[^']*'（ペイン [0-9]*）" "$LAUNCH_LOG")）"
else
  fail "起動の表示: $(grep 起動しました "$LAUNCH_LOG")"
fi
GEN_PANE="$(launch_master)"
GEN="$(tab_of_pane "$GEN_PANE")"
check_eq "汎用 master のタブ名に触らない（出どころ default）" "default" "$(tab_field "$GEN" title_source)"
case "$(tab_field "$GEN" title)" in
  master*) fail "汎用 master のタブ名が役割名: $(tab_field "$GEN" title)" ;;
  *) pass "汎用 master のタブ名が役割名でない（'$(tab_field "$GEN" title)'）" ;;
esac
for _ in $(seq 1 50); do
  [ "$(role_of_pane "$GEN_PANE")" = "orchestrator-master" ] && break
  sleep 0.1
done
T orchestrator adopt p2120 --pane "$GEN_PANE" > "$TMP/adopt.json" 2> "$TMP/adopt.err" || fail "adopt が通らない: $(tail -3 "$TMP/adopt.err")"
check_eq "adopt 後のタブ = p2120" "p2120" "$(tab_field "$GEN" title)"
check_eq "それは自動名" "auto" "$(tab_field "$GEN" title_source)"
check_eq "adopt の応答にタブ名が出る" "p2120/auto" \
  "$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(str(d.get("tab_title"))+"/"+str(d.get("tab_title_source")))' "$TMP/adopt.json" 2>/dev/null)"

echo
echo "=== ④ master の自動名で主題が付き、手で付けた名前は上書きされない ==="
T tab rename --tab "$GEN" --source auto 数理モデリング >/dev/null
check_eq "master の自動名で主題" "数理モデリング" "$(tab_field "$GEN" title)"
T tab rename --tab "$GEN" 自分の名前 >/dev/null
T tab rename --tab "$GEN" --source auto 別の主題 >/dev/null
check_eq "手で付けた名前は自動名で上書きされない" "自分の名前" "$(tab_field "$GEN" title)"
# インライン起動（ユーザーが名前を付けたタブのシェルで tako master を打つ）
USR="$(T tab new --title ユーザーの名前 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
USR_PANE="$(first_pane_of_tab "$USR")"
sleep 1
T send --pane "$USR_PANE" "\"$TAKO_BIN\" master -d2120" >/dev/null
for _ in $(seq 1 100); do
  [ "$(role_of_pane "$USR_PANE")" = "orchestrator-master:d2120" ] && break
  sleep 0.1
done
check_eq "インライン起動で master になった" "orchestrator-master:d2120" "$(role_of_pane "$USR_PANE")"
check_eq "ユーザーの付けた名前は起動でも残る" "ユーザーの名前" "$(tab_field "$USR" title)"

echo
echo "=== ⑤ 引き継ぎで後任が立ってもタブ名が戻らない ==="
T tab rename --tab "$DED" --source auto アジャイル >/dev/null
check_eq "引き継ぎ前のタブ名" "アジャイル" "$(tab_field "$DED" title)"
T orchestrator handoffs write --project p2120 --content "## 状態
- 検証用（#2120）" >/dev/null 2>&1 || true
BEFORE="$(pane_count_of_tab "$DED")"
if T orchestrator handoff --pane "$DED_PANE" > "$TMP/handoff.log" 2>&1; then
  pass "handoff が通った"
else
  fail "handoff が通らない: $(tail -3 "$TMP/handoff.log")"
fi
for _ in $(seq 1 50); do
  [ "$(pane_count_of_tab "$DED")" -gt "$BEFORE" ] && break
  sleep 0.1
done
check_eq "後任が同じタブに立つ（ペインが 1 枚増える）" "$((BEFORE + 1))" "$(pane_count_of_tab "$DED")"
sleep 2
check_eq "引き継ぎ後もタブ名は主題のまま" "アジャイル" "$(tab_field "$DED" title)"
check_eq "それは自動名のまま" "auto" "$(tab_field "$DED" title_source)"

echo
echo "=== ⑥ アプリの自動命名は master のタブ名を作らない（素のシェルのタブは命名される）==="
PLAIN="$(T tab new 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
PLAIN_PANE="$(first_pane_of_tab "$PLAIN")"
T send --pane "$PLAIN_PANE" "cd \"$TMP/proj\"" >/dev/null
for _ in $(seq 1 300); do
  [ "$(tab_field "$PLAIN" title_source)" = "auto" ] && break
  sleep 0.1
done
if [ "$(tab_field "$PLAIN" title_source)" = "auto" ]; then
  pass "素のシェルのタブはアプリが命名した（'$(tab_field "$PLAIN" title)'）= 自動命名は動いている"
else
  fail "素のシェルのタブが 30 秒で命名されない（自動命名が動いていない = ⑥ を確かめられない）"
fi
check_eq "同じ時間で master のタブ名は master が付けたまま" "アジャイル" "$(tab_field "$DED" title)"
NEW_TABS="$(tab_count)"
wait_layout_has 自分の名前 manual || true
stop_gui

echo
echo "=== ⑦-新: 新しい復元のタブバー（節は緑のはず）==="
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
