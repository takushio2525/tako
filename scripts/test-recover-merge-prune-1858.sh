#!/usr/bin/env bash
# tako:run bash scripts/test-recover-merge-prune-1858.sh
# test-recover-merge-prune-1858.sh — recover の合流と復元時の刈り込みの実経路テスト（#1858）
#
# 2026-09-30 の本番の復旧（#1857）で踏んだ 2 つを、隔離した data / tmux の**実 tako-app** と
# **実 tako CLI** で作り直して新旧を比べる:
#
#   ① `tako recover --apply 1` は、バックアップより後に作られたタブ（器が生きている）を
#      合流させ、出力と persist.log に列挙する（旧 = 世代をそのまま写すのでタブが消え、器は
#      orphan 自動復帰が「復帰」タブへ**別のペイン ID** で拾うだけ）
#   ② 器（tmux セッション）が無い master / worker のペインを復元の前に刈り込み、件数を
#      「復元の内訳」と `tako persist` の `restore_pruned` に出す（旧 = 新しいシェルで開き直す）。
#      利用者のペイン（役割なし）は閉じた構成を戻す用途（#770 / #2063）のため開き直す
#   ③ tmux に問い合わせられない（サーバーごと無い）・layout の器が 1 つも居ない別のサーバー
#      では刈り込まない
#   ④ recover の境界: tmux に問い合わせられないときは落とさず合流させる / 現在の
#      layout.json が読めないときは合流せず世代をそのまま写す
#
# 状況の作り方（事故の再現）:
#   世代の前 = タブ 1（利用者）・タブ 2（worker 2 枚）・タブ 4（利用者）。
#   世代の後 = master が worker 2 枚を片付ける（close = 器を kill。最初の close が世代を作る）・
#   利用者がタブ 4 を閉じすぎる・新しいタブ 3 を作る（復旧をしている master 自身のタブの役）。
#
# **本番の tako / 設定 / tmux には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
# `recover --apply` には `--force` を付ける（本番の tako-app が動いているので、別のデータ
# ディレクトリだと分かっている検証では明示の上書きが要る）。
#
# **CI には登録しない**（実 GUI を立てる）。CI 側の担保は単体テスト
# （`tako_control::layout_reconcile` / `restore_report`）と番犬
# （`crates/tako-control/tests/issue1858_layout_reconcile_watchdog.rs`）。
#
# 使い方: bash scripts/test-recover-merge-prune-1858.sh
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
check_has() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'${2}' が無い / 実際: ${3}）" ;;
  esac
}
check_not() {
  case "$3" in
    *"$2"*) fail "$1（'${2}' が出ている / 実際: ${3}）" ;;
    *) pass "$1" ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1858-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1858-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # サーバーが落ちてもソケットファイルが残ることがある（自分の名前の 1 本だけ消す）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${TMUX_SOCKET:?}"
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
# 復元経路そのものを見るので永続は ON（TAKO_ISOLATED は既定で OFF にする）
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

LAYOUT="$TAKO_DATA_DIR/layout.json"
BAK1="$TAKO_DATA_DIR/layout.json.bak.1"
PLOG="$TAKO_DATA_DIR/persist.log"
WORKER_ROLE="orchestrator-worker:tako"

#   start_app <ログの名前> [VAR=VAL …]   （persist.log は消さない = recover の行を残す）
start_app() {
  local name="$1"
  shift
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  sleep 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  sleep 1
}
tm() { tmux -L "$TMUX_SOCKET" "$@"; }
alive() { tm has-session -t "=$1" >/dev/null 2>&1 && echo yes || echo no; }
# 指定した器だけを残し、他（前の段で開き直したシェルの器）を落とす = 段どうしを独立させる
keep_only_sessions() {
  local s keep
  for s in $(tm list-sessions -F '#{session_name}' 2>/dev/null); do
    keep=no
    for k in "$@"; do [ "$s" = "$k" ] && keep=yes; done
    [ "$keep" = no ] && tm kill-session -t "=$s" >/dev/null 2>&1
  done
  return 0
}

# layout / list / persist の解析は 1 本のツールへ寄せる
TOOL="$TMP/tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys

def leaves(node):
    if node.get("type") == "split" or "first" in node:
        return leaves(node["first"]) + leaves(node["second"])
    return [node]

cmd = sys.argv[1]
if cmd == "layout_tabs":            # layout.json の表のタブ ID（並び順）
    d = json.load(open(sys.argv[2]))
    print(" ".join(str(t["id"]) for t in d["tabs"]))
elif cmd == "layout_panes":         # layout.json の表のペイン ID
    d = json.load(open(sys.argv[2]))
    print(" ".join(str(p["id"]) for t in d["tabs"] for p in leaves(t["tree"])))
elif cmd == "list_tabs":            # tako list のタブ ID
    d = json.load(sys.stdin)
    print(" ".join(str(t["id"]) for t in d["tabs"]))
elif cmd == "list_panes":           # tako list の全ペイン ID
    d = json.load(sys.stdin)
    print(" ".join(str(p["id"]) for t in d["tabs"] for p in t["panes"]))
elif cmd == "first_pane":           # tako list のタブ N の先頭ペイン
    d = json.load(sys.stdin)
    t = [t for t in d["tabs"] if str(t["id"]) == sys.argv[2]][0]
    print(t["panes"][0]["id"])
elif cmd == "session_of":           # tako list のペイン N の器
    d = json.load(sys.stdin)
    for t in d["tabs"]:
        for p in t["panes"]:
            if str(p["id"]) == sys.argv[2]:
                print(p.get("tmux_session") or "")
                raise SystemExit(0)
    print("")
elif cmd == "pane_with_session":    # tako list で器 S を持つペイン（"<タブ>/<ペイン>"）
    d = json.load(sys.stdin)
    for t in d["tabs"]:
        for p in t["panes"]:
            if p.get("tmux_session") == sys.argv[2]:
                print(f'{t["id"]}/{p["id"]}')
                raise SystemExit(0)
    print("")
elif cmd == "title_with_session":   # tako list で器 S を持つペインのタイトル
    d = json.load(sys.stdin)
    for t in d["tabs"]:
        for p in t["panes"]:
            if p.get("tmux_session") == sys.argv[2]:
                print(p.get("title") or "")
                raise SystemExit(0)
    print("")
elif cmd == "recovered":            # tako persist の recovered_count（orphan 自動復帰の数）
    print(json.load(sys.stdin).get("recovered_count"))
elif cmd == "pruned":               # tako persist の restore_pruned のペイン ID
    d = json.load(sys.stdin)
    print(" ".join(str(p["pane"]) for p in d.get("restore_pruned") or []))
elif cmd == "pruned_roles":
    d = json.load(sys.stdin)
    print(" ".join(p.get("role") or "-" for p in d.get("restore_pruned") or []))
elif cmd == "last_restore":
    d = json.load(sys.stdin)
    print(d.get("last_restore") or "")
else:
    raise SystemExit(f"未知のコマンド: {cmd}")
PYEOF
py() { python3 "$TOOL" "$@"; }
has_word() { case " $2 " in *" $1 "*) echo yes ;; *) echo no ;; esac; }
summary_line() { grep '復元成功' "$PLOG" 2>/dev/null | grep 'tmux 再 attach' | tail -1; }
detail_line() { grep '復元の内訳:' "$PLOG" 2>/dev/null | tail -1; }

echo "== 準備: 世代の前の構成（タブ 1 = 利用者 / タブ 2 = worker 2 枚 / タブ 4 = 利用者） =="
start_app setup
TABS="$("$TAKO_BIN" list | py list_tabs)"
if [ "$(echo "$TABS" | wc -w | tr -d ' ')" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS}）。中止する。"; exit 1
fi
TAB1="$TABS"
P1="$("$TAKO_BIN" list | py first_pane "$TAB1")"
TAB2="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
W1="$("$TAKO_BIN" list | py first_pane "$TAB2")"
W2="$("$TAKO_BIN" split --pane "$W1" --right | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$W1" --role "$WORKER_ROLE" >/dev/null
"$TAKO_BIN" title --pane "$W2" --role "$WORKER_ROLE" >/dev/null
TAB4="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
U="$("$TAKO_BIN" list | py first_pane "$TAB4")"
"$TAKO_BIN" tab select "$TAB1" >/dev/null 2>&1
sleep 3   # layout.json の保存（2 秒ポーリング）を待つ
S_P1="$("$TAKO_BIN" list | py session_of "$P1")"
S_W1="$("$TAKO_BIN" list | py session_of "$W1")"
S_W2="$("$TAKO_BIN" list | py session_of "$W2")"
S_U="$("$TAKO_BIN" list | py session_of "$U")"
echo "  tabs: 1=$TAB1 2=$TAB2 4=$TAB4 / panes: P1=$P1 W1=$W1 W2=$W2 U=$U"

echo "== 世代の後: worker を片付け・タブ 4 を閉じすぎ・タブ 3 を作る =="
"$TAKO_BIN" close --pane "$W1" --force >/dev/null 2>&1
sleep 1
"$TAKO_BIN" close --pane "$W2" --force >/dev/null 2>&1
"$TAKO_BIN" close --pane "$U" --force >/dev/null 2>&1
TAB3="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
M="$("$TAKO_BIN" list | py first_pane "$TAB3")"
# 合流で戻ったのか orphan 自動復帰で拾われたのかを見分ける印（ID は採番が世代の最大値の
# 次から始まるので、orphan 復帰の新しいタブ / ペインが偶然同じ ID を引くことがある = 実測）
M_TITLE="after-backup-1858"
"$TAKO_BIN" title --pane "$M" "$M_TITLE" >/dev/null
sleep 3
S_M="$("$TAKO_BIN" list | py session_of "$M")"
echo "  tab 3=$TAB3 / pane M=$M"
stop_app
check_eq "世代（bak.1）はタブ 1 / 2 / 4" "$TAB1 $TAB2 $TAB4" "$(py layout_tabs "$BAK1" 2>/dev/null)"
check_eq "現在の layout.json はタブ 1 / 3" "$TAB1 $TAB3" "$(py layout_tabs "$LAYOUT")"
check_eq "閉じた worker の器は終わっている" "no no" "$(alive "$S_W1") $(alive "$S_W2")"
check_eq "閉じた利用者の器は終わっている" "no" "$(alive "$S_U")"
check_eq "タブ 1 と世代の後のタブ 3 の器は生きている" "yes yes" "$(alive "$S_P1") $(alive "$S_M")"
cp "$LAYOUT" "$TMP/current.json"
cp "$BAK1" "$TMP/bak1.json"

echo "== ① A/B 旧: TAKO_1858_LEGACY=1 で recover --apply 1 → 起動 =="
: > "$PLOG"
LEG_OUT="$(env TAKO_1858_LEGACY=1 "$TAKO_BIN" recover --apply 1 --force 2>&1)"
if cmp -s "$LAYOUT" "$TMP/bak1.json"; then
  pass "旧: layout.json は世代とバイト一致（タブ 3 は書かれない）"
else
  fail "旧: layout.json が世代と違う"
fi
check_not "旧: recover は合流を言わない" "合流" "$LEG_OUT"
start_app legacy TAKO_1858_LEGACY=1
LIST="$("$TAKO_BIN" list)"
check_eq "旧: タブ 3 は layout から落ち、器は orphan 自動復帰が拾う（Issue の症状）" "1" \
  "$("$TAKO_BIN" persist | py recovered)"
check_eq "旧: 拾われたペインは元のタイトルを失う（タブ 3 の中身は戻らない）" "" \
  "$(echo "$LIST" | py title_with_session "$S_M")"
check_has "旧: 器は「復帰」側の新しいペインに居る" "/" "$(echo "$LIST" | py pane_with_session "$S_M")"
check_eq "旧: 片付けた worker のペイン W1 / W2 が開き直される" "yes yes" \
  "$(has_word "$W1" "$(echo "$LIST" | py list_panes)") $(has_word "$W2" "$(echo "$LIST" | py list_panes)")"
check_eq "旧: 刈り込みの行は出ない" "0" "$(grep -c '刈り込み' "$PLOG" | tr -d ' ')"
stop_app
keep_only_sessions "$S_P1" "$S_M"

echo "== ① 新: recover --apply 1 → 起動 =="
cp "$TMP/current.json" "$LAYOUT"
cp "$TMP/bak1.json" "$BAK1"
: > "$PLOG"
NEW_OUT="$("$TAKO_BIN" recover --apply 1 --force 2>&1)"
echo "$NEW_OUT" | sed 's/^/    | /'
check_has "新: recover が合流を出力する" "バックアップより後に作られたタブを 1 枚合流しました" "$NEW_OUT"
check_has "新: 合流したタブを ID で列挙する" "タブ ${TAB3}「" "$NEW_OUT"
check_has "新: 器の有無を添える" "tmux セッションあり 1" "$NEW_OUT"
check_has "新: 合流後の数を出す" "合流後: 4 タブ" "$NEW_OUT"
check_eq "新: layout.json = 世代のタブ + タブ 3" "$TAB1 $TAB2 $TAB4 $TAB3" "$(py layout_tabs "$LAYOUT")"
check_has "新: persist.log に合流の行" "recover --apply 1: バックアップより後のタブを合流 1 枚[タブ ${TAB3}（1 ペイン・器あり 1・器なし 0）]" "$(cat "$PLOG")"
start_app new
LIST="$("$TAKO_BIN" list)"
check_eq "新: タブ 3 が残る" "yes" "$(has_word "$TAB3" "$(echo "$LIST" | py list_tabs)")"
check_eq "新: タブ 3 は元のペイン ID で器へ再 attach する" "$TAB3/$M" "$(echo "$LIST" | py pane_with_session "$S_M")"
check_eq "新: ペインのタイトルも戻る" "$M_TITLE" "$(echo "$LIST" | py title_with_session "$S_M")"
PERSIST="$("$TAKO_BIN" persist)"
check_eq "新: orphan 自動復帰は起きない（器は合流したペインが持つ）" "0" \
  "$(echo "$PERSIST" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("recovered_count"))')"

echo "== ② 新: 器の無い worker のペインを刈り込み、利用者のペインは開き直す =="
check_eq "② タブ 2（worker 2 枚）が刈り込まれて閉じる" "no" "$(has_word "$TAB2" "$(echo "$LIST" | py list_tabs)")"
check_eq "② W1 / W2 が残らない" "no no" \
  "$(has_word "$W1" "$(echo "$LIST" | py list_panes)") $(has_word "$W2" "$(echo "$LIST" | py list_panes)")"
check_eq "② 閉じすぎた利用者のタブ 4 は戻る（#770 / #2063）" "yes" "$(has_word "$TAB4" "$(echo "$LIST" | py list_tabs)")"
check_eq "② 利用者のペイン U は同じ ID で開き直す" "yes" "$(has_word "$U" "$(echo "$LIST" | py list_panes)")"
check_has "② persist.log: W1 の刈り込み行" "刈り込み（ペイン ${W1}・タブ ${TAB2}）: tmux セッション ${S_W1} が無い（役割 ${WORKER_ROLE}）" "$(cat "$PLOG")"
check_has "② persist.log: W2 の刈り込み行" "刈り込み（ペイン ${W2}・タブ ${TAB2}）: tmux セッション ${S_W2} が無い" "$(cat "$PLOG")"
check_has "② persist.log: U は外さずに開き直す行" "tmux セッションが無いが外さずに開き直す: ペイン ${U}" "$(cat "$PLOG")"
SUM="$(summary_line)"; DET="$(detail_line)"
echo "    1 行目: $SUM"
echo "    2 行目: $DET"
check_has "② 1 行目に刈り込みの件数" "・刈り込み 2（tmux セッションが無い。空になったタブ 1 枚を閉じた）" "$SUM"
check_has "② 「復元の内訳」に刈り込みの件数" "/ 刈り込み 2（tmux セッションが無い。空になったタブ 1 枚を閉じた）" "$DET"
check_eq "② 内訳が全ペインを説明できていない行は出ない" "0" "$(grep -c '説明できていない' "$PLOG" | tr -d ' ')"
check_eq "② tako persist の restore_pruned（CLI / MCP）" "$W1 $W2" "$(echo "$PERSIST" | py pruned)"
check_eq "② restore_pruned に役割が載る" "$WORKER_ROLE $WORKER_ROLE" "$(echo "$PERSIST" | py pruned_roles)"
check_has "② tako persist の last_restore に件数" "刈り込み 2" "$(echo "$PERSIST" | py last_restore)"
# 世代の後のタブが無いときは、従来どおりの終わりの案内だけ（ここでは足さない）
"$TAKO_BIN" tab select "$TAB1" >/dev/null 2>&1
echo "== ② A/B（recover を通さない）: 停止中に worker の器が終わった layout から起動 =="
TAB5="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
W3="$("$TAKO_BIN" list | py first_pane "$TAB5")"
"$TAKO_BIN" title --pane "$W3" --role "$WORKER_ROLE" >/dev/null
sleep 3
S_W3="$("$TAKO_BIN" list | py session_of "$W3")"
S_U2="$("$TAKO_BIN" list | py session_of "$U")"
stop_app
tm kill-session -t "=$S_W3" >/dev/null 2>&1
check_eq "② worker W3 の器を停止中に終わらせた" "no" "$(alive "$S_W3")"
cp "$LAYOUT" "$TMP/layout-w3.json"
: > "$PLOG"
start_app prune-legacy TAKO_1858_LEGACY=1
LIST="$("$TAKO_BIN" list)"
check_eq "② 旧: W3 は開き直されて残る（Issue の症状）" "yes" "$(has_word "$W3" "$(echo "$LIST" | py list_panes)")"
check_eq "② 旧: restore_pruned は空" "" "$("$TAKO_BIN" persist | py pruned)"
stop_app
keep_only_sessions "$S_P1" "$S_M" "$S_U2"
cp "$TMP/layout-w3.json" "$LAYOUT"
: > "$PLOG"
start_app prune-new
LIST="$("$TAKO_BIN" list)"
check_eq "② 新: タブ 5 / W3 が刈り込まれる" "no no" \
  "$(has_word "$TAB5" "$(echo "$LIST" | py list_tabs)") $(has_word "$W3" "$(echo "$LIST" | py list_panes)")"
check_eq "② 新: 他のペインは残る（P1 / M / U）" "yes yes yes" \
  "$(has_word "$P1" "$(echo "$LIST" | py list_panes)") $(has_word "$M" "$(echo "$LIST" | py list_panes)") $(has_word "$U" "$(echo "$LIST" | py list_panes)")"
check_eq "② 新: restore_pruned = W3" "$W3" "$("$TAKO_BIN" persist | py pruned)"
check_has "② 新: 「復元の内訳」に刈り込み 1" "/ 刈り込み 1（tmux セッションが無い。空になったタブ 1 枚を閉じた）" "$(detail_line)"
stop_app

echo "== ③ tmux に問い合わせられない（サーバーごと無い）なら刈り込まない =="
tm kill-server >/dev/null 2>&1
cp "$TMP/layout-w3.json" "$LAYOUT"
: > "$PLOG"
start_app no-server
LIST="$("$TAKO_BIN" list)"
check_eq "③ W3 は刈り込まれず開き直される" "yes" "$(has_word "$W3" "$(echo "$LIST" | py list_panes)")"
check_eq "③ restore_pruned は空" "" "$("$TAKO_BIN" persist | py pruned)"
HELD="$(grep '刈り込みを見送った' "$PLOG" | tail -1)"
echo "    $HELD"
case "$HELD" in
  *"問い合わせられない"*|*"layout の器が 1 つも無い"*) pass "③ 見送った理由が persist.log に残る" ;;
  *) fail "③ 見送った理由が無い（${HELD}）" ;;
esac
check_eq "③ 刈り込み行は出ない" "0" "$(grep -c '^.*刈り込み（ペイン' "$PLOG" | tr -d ' ')"
stop_app

echo "== ③ 別のサーバー（layout の器が 1 つも居ない一覧）でも刈り込まない =="
tm kill-server >/dev/null 2>&1
tm -f /dev/null new-session -d -s unrelated-1858 >/dev/null 2>&1
cp "$TMP/layout-w3.json" "$LAYOUT"
: > "$PLOG"
start_app other-server
check_eq "③ W3 は刈り込まれず開き直される" "yes" "$(has_word "$W3" "$("$TAKO_BIN" list | py list_panes)")"
check_has "③ 見送った理由（別のサーバー）" "layout の器が 1 つも無い" "$(grep '刈り込みを見送った' "$PLOG" | tail -1)"
stop_app

echo "== ④ recover の境界 =="
tm kill-server >/dev/null 2>&1
cp "$TMP/current.json" "$LAYOUT"
cp "$TMP/bak1.json" "$BAK1"
OUT="$("$TAKO_BIN" recover --apply 1 --force 2>&1)"
check_has "④ tmux に問い合わせられなくても落とさず合流させる" "生死は確かめずに合流しました" "$OUT"
check_eq "④ その layout.json は世代のタブ + タブ 3" "$TAB1 $TAB2 $TAB4 $TAB3" "$(py layout_tabs "$LAYOUT")"
echo '{ 壊れた' > "$LAYOUT"
cp "$TMP/bak1.json" "$BAK1"
OUT="$("$TAKO_BIN" recover --apply 1 --force 2>&1)"
check_has "④ 現在の layout.json が読めないときは合流しないと言う" "現在の layout.json を読めないので" "$OUT"
if cmp -s "$LAYOUT" "$TMP/bak1.json"; then
  pass "④ そのときは世代とバイト一致で写す"
else
  fail "④ 読めない layout.json のときに世代と違うものを書いた"
fi
cp "$TMP/bak1.json" "$LAYOUT"
cp "$TMP/bak1.json" "$BAK1"
OUT="$("$TAKO_BIN" recover --apply 1 --force 2>&1)"
check_not "④ 世代の後のタブが無ければ合流を言わない" "合流" "$OUT"

echo
echo "結果: PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
