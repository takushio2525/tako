#!/usr/bin/env bash
# test-limit-resume-all-1945.sh — リミット後の自動復帰の一括 ON / OFF の実経路テスト（#1945）
#
# 隔離した data / HOME / tmux で**実 tako-app** を立て、CLI（= MCP・ステータスバーの
# ボタンと同じ dispatch）から
#   ① `tako limit-resume --all` の一覧に退避中のペインが載り、集計が全部 OFF
#   ② `tako limit-resume on --all` で退避中を含む全エージェントのペインが ON・シェルは OFF・
#      既定（settings.json の limit_resume_all）も ON
#   ③ 退避中のペインを `--pane N` で切り替えられる（#1945 前は「ペインが見つからない」）。
#      表で ON にしたペインを退避しても ON と読める（#1945 前は false と答えた）
#   ④ 一括 ON の直後に role を貼ったペイン（solo）が ON で始まる
#   ⑤ 手起動のエージェント（会話の検出 = プロセス表の `codex`）も既定で ON になる
#   ⑥ GUI を再起動しても既定とペインの値が保たれ、再起動後に立てたペインも ON で始まる
#   ⑦ `off --all` で全部 OFF・既定も OFF、以後のペインは OFF で始まる
#   ⑧ `--all` と `--pane` は同時に言えない
# を実測する。ボタンを実マウスで押す側は visual-test の節 `limit-resume-all` が見る。
#
# A/B: `TAKO_1945_LEGACY=1 bash scripts/test-limit-resume-all-1945.sh` で #1945 前の
# 挙動（一括は併用エラー・退避中は指定不可）になり、②③④⑤⑥ が NG になる。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI を立てる）。CI 側の担保は単体テスト
# （`tako_core::limit_resume_all` / dispatch の `issue1945_*`）。
#
# 使い方: bash scripts/test-limit-resume-all-1945.sh
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

TMP="$(mktemp -d /tmp/tako-1945-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1945-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME を差し替える前に（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
# 素の HOME だと zsh が初回設定メニュー（zsh-newuser-install）で `tako send` の入力を食う。
# 固定プロンプトは実ユーザー名・実ホスト名を画面へ出さないためでもある（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$HOME/.zshrc"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# 再起動をまたぐ値（layout.json）と会話の検出は永続が ON のときだけ動く
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

start_app() {
  local name="$1"
  shift
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  sleep 1
}
lr() { "$TAKO_BIN" limit-resume "$@" 2>&1; }
# 一覧の要点（集計とペインごとの enabled / agent / shelved）を 1 行ずつ
show_all() {
  lr --all | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("summary") or {}
print("  summary:", s.get("state"), "on=%s total=%s default=%s" % (s.get("on"), s.get("total"), v.get("default")))
for p in v["panes"]:
    print("   pane %-3s enabled=%-5s agent=%-5s shelved=%s" % (p["pane"], p["enabled"], p.get("agent"), p.get("shelved")))
' 2>&1
}
field() { python3 -c "import json,sys; v=json.load(sys.stdin); print($1)" 2>/dev/null; }
enabled_of() { lr --pane "$1" | field 'str(v["enabled"]).lower()'; }

echo "== 隔離 GUI を起こして検証用の構成を組む =="
start_app 1
TABS="$("$TAKO_BIN" list | field 'len(v["tabs"])')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"
P1="$("$TAKO_BIN" list | field 'v["tabs"][0]["panes"][0]["id"]')"
P2="$("$TAKO_BIN" split --pane "$P1" --right | tr -dc '0-9')"
P3="$("$TAKO_BIN" split --pane "$P2" --down | tr -dc '0-9')"
P4="$("$TAKO_BIN" split --pane "$P3" --down | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$P1" --role orchestrator-master >/dev/null
"$TAKO_BIN" title --pane "$P3" --role solo >/dev/null
"$TAKO_BIN" title --pane "$P4" --role orchestrator-worker:tako >/dev/null
"$TAKO_BIN" background --pane "$P4" >/dev/null
echo "  master=$P1 shell=$P2 solo=$P3 worker(退避)=$P4"

echo "== ① 前: 一覧に退避中が載り、全部 OFF =="
show_all
BEFORE="$(lr --all)"
check_eq "① 集計は全部 OFF" "all_off" "$(echo "$BEFORE" | field 'v["summary"]["state"]')"
check_eq "① エージェントは 3 本（シェルは数えない）" "3" "$(echo "$BEFORE" | field 'v["summary"]["total"]')"
check_eq "① 退避中の worker が一覧に載る" "True" \
  "$(echo "$BEFORE" | field "[p['shelved'] for p in v['panes'] if p['pane']==$P4][0]")"

echo "== ② on --all =="
ON="$(lr on --all)"
echo "  changed: $(echo "$ON" | field 'v.get("changed")')"
show_all
check_eq "② 退避中を含む 3 本が変わった" "[$P1, $P3, $P4]" "$(echo "$ON" | field 'v.get("changed")')"
check_eq "② 集計は全部 ON" "all_on" "$(echo "$ON" | field 'v["summary"]["state"]')"
check_eq "② シェルは触らない" "false" "$(enabled_of "$P2")"
check_eq "② 既定が settings.json に残る" "True" \
  "$(field 'v.get("limit_resume_all")' < "$TAKO_DATA_DIR/settings.json")"

echo "== ③ 退避中を --pane で切り替える =="
check_eq "③ off --pane（退避中）" "false" "$(lr off --pane "$P4" | field 'str(v["enabled"]).lower()')"
check_eq "③ 一部になる" "partial" "$(lr --all | field 'v["summary"]["state"]')"
check_eq "③ on --pane（退避中）" "true" "$(lr on --pane "$P4" | field 'str(v["enabled"]).lower()')"
# master の実測（2026-10-09）: 表で ON → 退避すると worker_status が false と答えていた
"$TAKO_BIN" background --pane "$P3" >/dev/null
check_eq "③ 表で ON にした solo を退避しても ON と読む" "true" "$(enabled_of "$P3")"
check_eq "③ 退避中と分かる" "true" "$(lr --pane "$P3" | field 'str(v["shelved"]).lower()')"
"$TAKO_BIN" foreground "$P3" >/dev/null
check_eq "③ 表へ戻しても ON のまま" "true" "$(enabled_of "$P3")"

echo "== ④ 一括 ON の直後に立てた solo =="
P5="$("$TAKO_BIN" split --pane "$P1" --down | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$P5" --role solo >/dev/null
check_eq "④ 新しい solo は ON で始まる" "true" "$(enabled_of "$P5")"

echo "== ⑤ 手起動のエージェント（プロセス表の codex）=="
mkdir -p "$TMP/bin"
# **写しではなくリンク**: /bin/sleep の写しは macOS に即座に kill される（署名の検査。実測）。
# リンクなら argv[0] が `…/codex` のまま本物の sleep が動き、プロセス表で codex に見える
ln -s /bin/sleep "$TMP/bin/codex"
P6="$("$TAKO_BIN" split --pane "$P2" --right | tr -dc '0-9')"
sleep 1
"$TAKO_BIN" send --pane "$P6" "$TMP/bin/codex 600" >/dev/null 2>&1
GOT=""
for _ in $(seq 1 30); do
  GOT="$(enabled_of "$P6")"
  [ "$GOT" = "true" ] && break
  sleep 2
done
if [ "$GOT" = "true" ]; then
  pass "⑤ 手起動の codex が検出されて既定（ON）を採った"
else
  # 会話の検出は `claude agents --json` の取得が通ったときだけ走る（隔離 HOME では
  # 通らないことがある）。通らなかったら未実測として残し、NG には数えない
  echo "  [未実測] ⑤ 60 秒以内に検出されなかった（enabled=${GOT}。persist.log の会話検出の行を見る）"
fi

echo "== ⑥ GUI を再起動しても保たれる =="
sleep 2   # layout.json の保存を待つ
stop_app
start_app 2
show_all
AFTER="$(lr --all)"
check_eq "⑥ 既定は ON のまま" "True" "$(echo "$AFTER" | field 'v["default"]')"
check_eq "⑥ master は ON のまま" "true" "$(enabled_of "$P1")"
check_eq "⑥ 退避中の worker も ON のまま" "true" "$(enabled_of "$P4")"
check_eq "⑥ シェルは OFF のまま" "false" "$(enabled_of "$P2")"
P7="$("$TAKO_BIN" split --pane "$P1" --right | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$P7" --role orchestrator-master >/dev/null
check_eq "⑥ 再起動後に立てた master も ON で始まる" "true" "$(enabled_of "$P7")"

echo "== ⑦ off --all =="
OFF="$(lr off --all)"
show_all
check_eq "⑦ 集計は全部 OFF" "all_off" "$(echo "$OFF" | field 'v["summary"]["state"]')"
check_eq "⑦ 既定も OFF" "False" "$(echo "$OFF" | field 'v["default"]')"
P8="$("$TAKO_BIN" split --pane "$P1" --down | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$P8" --role solo >/dev/null
check_eq "⑦ 以後の solo は OFF で始まる" "false" "$(enabled_of "$P8")"

echo "== ⑧ --all と --pane は排他 =="
if lr on --all --pane "$P1" >/dev/null 2>&1; then
  fail "⑧ --all と --pane を同時に受け付けた"
else
  pass "⑧ --all と --pane は同時に言えない"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
