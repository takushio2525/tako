#!/usr/bin/env bash
# test-shelved-restore-1576.sh — 退避（たまり場・退避タブ）のペインが再起動で**退避のまま同じ器へ**
# 戻ることの実経路テスト（#1576）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#   ① たまり場 1 本（FR-2.15.5）+ 退避タブ 1 枚（#1487）を含む構成で再起動すると、
#      orphan 自動復帰が 0 件・「復帰」タブが出ない・退避のペインが**同じ pane id・role・
#      タイトル・limit_resume** で退避のまま戻る
#   ② 退避のまま器（tmux セッション）へ繋ぎ直している: 器の attach が 1・シェルの pid が
#      再起動の前後で同じ・`tako backgrounded` の state が unknown でない（= 端末がある）
#   ③ 表に出すと同じ器の画面（再起動前に打った目印）が見える。器は増えない
#   ④ 再起動後の保存でも退避エントリが器の名前を持つ（器の無い幽霊を layout へ書かない）
#   ⑤ A/B `TAKO_1576_LEGACY=1` で修正前の症状（orphan 自動復帰 2・「復帰」タブに別 pane・
#      退避エントリは器なし）が同一バイナリで再現する
#   ⑥ エッジ: 同じ器を 2 つの退避エントリが指す / 器も戻す手掛かりも無い退避エントリ（幽霊）
#      → どちらも退避から外して persist.log に名指しで残し、器は 1 つのクライアントだけが持つ
#   ⑦ エッジ: 退避中のペインのプロセスが再起動の間に終わっていた → 退避のまま新しいシェルで戻る
#      （幽霊にしない・orphan 自動復帰は 0）
#   ⑧ エッジ: 退避タブが無い（たまり場だけ）layout でも同じく戻る
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット
# `TAKO_TMUX_SOCKET`、落とすのは自分で起こした pid と自分のソケットだけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る = 他の
# `isolated-gui.sh` 系スクリプトと同じ扱い）。CI 側の担保は単体テスト
# （`tako_control::shelved_restore`）と番犬
# （`crates/tako-control/tests/issue1576_shelved_restore_watchdog.rs`）。
#
# 使い方: bash scripts/test-shelved-restore-1576.sh
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
check_ne() {
  if [ "$2" != "$3" ]; then pass "$1"; else fail "$1（'${2}' と同じ）"; fi
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

TMP="$(mktemp -d /tmp/tako-1576-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1576-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # サーバーが落ちてもソケットファイルが残ることがある（自分の名前の 1 本だけ消す）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET"
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# HOME を差し替える**前**にバイナリを決める（後で呼ぶとツールチェーンを一時 HOME へ取り直す）
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
PLOG="$TAKO_DATA_DIR/persist.log"
T() { "$TAKO_BIN" "$@"; }
TM() { tmux -L "$TMUX_SOCKET" "$@"; }

#   start_app <ログの名前> [VAR=VAL …]
start_app() {
  local name="$1"
  shift
  : > "$PLOG"
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  # 起動直後の復元・orphan 自動復帰・掃除は IPC が通る前に終わっている。
  # attach クライアントが器へ繋がり切るのを少し待つ
  sleep 2
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  sleep 1
}
# layout の保存（2 秒ポーリング）を待つ
wait_save() { sleep 3; }

# JSON の解析・layout の細工は 1 本のツールへ寄せる
TOOL="$TMP/tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys

def panes(node, acc):
    if node.get("type") == "split" or "first" in node:
        panes(node["first"], acc)
        panes(node["second"], acc)
    else:
        acc.append(node)
    return acc

def tab_panes(d):
    out = []
    for t in d["tabs"]:
        panes(t["tree"], out)
    return out

cmd = sys.argv[1]
if cmd == "list":
    # tako list の要約: タブ枚数・タブ名・タブ配下の id・退避の id と属性
    d = json.load(open(sys.argv[2]))
    print("tabs", len(d["tabs"]))
    print("tab_titles", "|".join(t["title"] for t in d["tabs"]))
    print("tab_panes", " ".join(str(p["id"]) for t in d["tabs"] for p in t["panes"]))
    print("shelved", " ".join(str(p["id"]) for p in d["shelved_panes"]))
    print("shelved_tabs", " ".join(str(t["tab"]) for t in d.get("shelved_tabs", [])))
    for p in d["shelved_panes"]:
        print("sp:%d" % p["id"], "%s|%s|%s|%s" % (
            p.get("title"), p.get("role"), str(p.get("limit_autoresume")).lower(),
            p.get("shelved_tab") if p.get("shelved_tab") is not None else "-"))
elif cmd == "bg":
    # tako backgrounded の要約: ペインごとの state（unknown = 端末が無い = 器なし）
    d = json.load(open(sys.argv[2]))
    for p in d["backgrounded"]:
        print("state:%d" % p["pane"], p.get("state"))
elif cmd == "layout":
    # layout.json の退避エントリが持つ器の名前（null は "-"）
    d = json.load(open(sys.argv[2]))
    for p in d.get("backgrounded", []):
        print("bg:%d" % p["id"], p.get("session") or "-")
    for e in d.get("shelved_tabs", []):
        for p in panes(e["tab"]["tree"], []):
            print("st:%d" % p["id"], p.get("session") or "-")
    for p in tab_panes(d):
        print("tab:%d" % p["id"], p.get("session") or "-")
elif cmd == "add_dup_and_ghost":
    # ⑥: 同じ器を指す 2 本目の退避エントリと、器も手掛かりも無い退避エントリを足す
    src, dst, dup_id, ghost_id = sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
    d = json.load(open(src))
    first = d["backgrounded"][0]
    dup = dict(first)
    dup["id"] = dup_id
    dup["title"] = "dup-of-%d" % first["id"]
    ghost = dict(first)
    ghost.update({"id": ghost_id, "title": "ghost", "session": None, "cwd": None,
                  "claude_session_id": None, "agent_resume": None, "logged_history": None,
                  "preview": None, "webview": None, "ssh": None})
    d["backgrounded"] = d["backgrounded"] + [dup, ghost]
    json.dump(d, open(dst, "w"), ensure_ascii=False)
elif cmd == "drop_shelved_tabs":
    # ⑧: 退避タブを外した layout（たまり場だけ）
    src, dst = sys.argv[2], sys.argv[3]
    d = json.load(open(src))
    d["shelved_tabs"] = []
    json.dump(d, open(dst, "w"), ensure_ascii=False)
else:
    raise SystemExit("未知のコマンド: " + cmd)
PYEOF

LIST_TXT="$TMP/list.txt"
snap_list() {
  T list > "$TMP/list.json" 2>/dev/null || { echo "{}" > "$TMP/list.json"; }
  python3 "$TOOL" list "$TMP/list.json" > "$LIST_TXT" 2>/dev/null || : > "$LIST_TXT"
}
lv() { awk -v k="$1" '$1==k {$1=""; sub(/^ /, ""); print}' "$LIST_TXT"; }
snap_bg() {
  T backgrounded > "$TMP/bg.json" 2>/dev/null || { echo '{"backgrounded":[]}' > "$TMP/bg.json"; }
  python3 "$TOOL" bg "$TMP/bg.json" > "$TMP/bg.txt" 2>/dev/null || : > "$TMP/bg.txt"
}
bgv() { awk -v k="$1" '$1==k {print $2}' "$TMP/bg.txt"; }
snap_layout() { python3 "$TOOL" layout "$LAYOUT" > "$TMP/layout.txt" 2>/dev/null || : > "$TMP/layout.txt"; }
lay() { awk -v k="$1" '$1==k {print $2}' "$TMP/layout.txt"; }
# 器の attach 数とシェルの pid（器が無ければ空）。`=名前:` は「その名前のセッションの
# 現在のウインドウ」（`=名前` だけだと display-message はペインを解決できず空を返す）
attached() { TM display-message -p -t "=${1}:" '#{session_attached}' 2>/dev/null; }
shell_pid() { TM display-message -p -t "=${1}:" '#{pane_pid}' 2>/dev/null; }
# 器が覚えている持ち主のペイン ID。orphan 自動復帰は拾った器のこれを新しい pane id へ
# 書き換えるので、「同じ器を同じペインが持っている」ことの印になる
vessel_pane() { TM show-environment -t "=${1}" TAKO_PANE_ID 2>/dev/null | sed 's/^TAKO_PANE_ID=//'; }
session_count() { TM list-sessions -F '#{session_name}' 2>/dev/null | grep -c '^tako-' | tr -d ' '; }
orphan_lines() { grep -c 'orphan 自動復帰' "$PLOG" 2>/dev/null | tr -d ' '; }
detail_line() { grep '復元の内訳:' "$PLOG" 2>/dev/null | tail -1; }
summary_line() { grep '復元成功' "$PLOG" 2>/dev/null | grep 'tmux 再 attach' | tail -1; }
# 退避エントリ自身が器へ繋がった端末を持つか: 器のシェルへ tmux 側から Enter を送り、
# 新しいプロンプトのシェル統合（OSC 133）が**そのペイン**の状態を unknown から動かすかを見る。
# 再 attach した直後はプロンプトが描き直されないので状態は unknown のまま（表のペインも同じ）。
# 器を別 pane が持っている（旧挙動の「復帰」タブ）なら、退避エントリの状態は動かない
#   bg_state_after_enter <器> <ペイン>
bg_state_after_enter() {
  local vessel="$1" pane="$2" i st=""
  TM send-keys -t "=${vessel}:" Enter >/dev/null 2>&1
  for i in $(seq 1 25); do
    snap_bg
    st="$(bgv "state:$pane")"
    [ -n "$st" ] && [ "$st" != "unknown" ] && break
    sleep 0.2
  done
  echo "$st"
}
# 画面に目印が出るまで待つ（attach 直後の再描画を待つ）
read_has() {
  local pane="$1" mark="$2" i
  for i in $(seq 1 30); do
    if T read --pane "$pane" 2>/dev/null | grep -q "$mark"; then echo yes; return; fi
    sleep 0.2
  done
  echo no
}

echo "== 隔離 GUI を起こして検証用の構成を組む =="
start_app 1
snap_list
if [ "$(lv tabs)" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ $(lv tabs) 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR socket=$TMUX_SOCKET"
TAB1="$(T list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["id"])')"
P1="$(lv tab_panes)"
MARK_A="mark1576a$$"
MARK_B="mark1576b$$"
# たまり場へ送るペイン（タイトル・role・limit_resume を付け、目印を画面に残す）
PA="$(T split --pane "$P1" --right | tr -dc '0-9')"
T title --pane "$PA" --role worker-1576a shelf-a >/dev/null
T limit-resume on --pane "$PA" >/dev/null
T send --pane "$PA" "echo $MARK_A" >/dev/null
# 退避タブ（最後の 1 枚にならないよう新しいタブを作ってから退避する）
TAB2="$(T tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
PB="$(T list | python3 -c "import json,sys; print([t for t in json.load(sys.stdin)['tabs'] if t['id']==$TAB2][0]['panes'][0]['id'])")"
T title --pane "$PB" --role worker-1576b shelf-b >/dev/null
T limit-resume on --pane "$PB" >/dev/null
T send --pane "$PB" "echo $MARK_B" >/dev/null
sleep 1
check_eq "目印がペイン A に出ている（前提）" "yes" "$(read_has "$PA" "$MARK_A")"
check_eq "目印がペイン B に出ている（前提）" "yes" "$(read_has "$PB" "$MARK_B")"
T tab select "$TAB1" >/dev/null 2>&1
T background --pane "$PA" >/dev/null
T background --tab "$TAB2" >/dev/null
wait_save
snap_list
echo "  退避前の構成: タブ=$(lv tab_panes) 退避=$(lv shelved) 退避タブ=$(lv shelved_tabs)"
stop_app

snap_layout
SA="$(lay "bg:$PA")"
SB="$(lay "st:$PB")"
S1="$(lay "tab:$P1")"
check_ne "たまり場ペイン A の器が layout に載る（前提）" "-" "$SA"
check_ne "退避タブのペイン B の器が layout に載る（前提）" "-" "$SB"
PID_A="$(shell_pid "$SA")"
PID_B="$(shell_pid "$SB")"
SESSIONS_BEFORE="$(session_count)"
echo "  器: A=$SA(pid $PID_A) B=$SB(pid $PID_B) 表=$S1 / 器の数 $SESSIONS_BEFORE"
check_ne "アプリ終了後も器 A が生きている（前提）" "" "$PID_A"
check_ne "アプリ終了後も器 B が生きている（前提）" "" "$PID_B"
cp "$LAYOUT" "$TMP/layout-base.json"

echo "== ①〜④ 再起動（修正後） =="
start_app fixed
snap_list
snap_bg
echo "  1 行目: $(summary_line)"
echo "  2 行目: $(detail_line)"
check_eq "① orphan 自動復帰が 0 件" "0" "$(orphan_lines)"
check_eq "① タブは 1 枚のまま（「復帰」タブが出ない）" "1" "$(lv tabs)"
check_eq "① タブ配下のペインは元の 1 本だけ" "$P1" "$(lv tab_panes)"
check_eq "① たまり場ペイン A が同じ id で退避のまま戻る" "shelf-a|worker-1576a|true|-" "$(lv "sp:$PA")"
check_eq "① 退避タブのペイン B が同じ id・同じ退避タブで戻る" "shelf-b|worker-1576b|true|$TAB2" "$(lv "sp:$PB")"
check_eq "① 退避タブが同じ id で戻る" "$TAB2" "$(lv shelved_tabs)"
check_has "① 2 行目にたまり場・退避の戻し方が出る" "たまり場・退避 2（たまり場 1 / 退避タブ 1。戻し方: tmux 再 attach 2）" "$(detail_line)"
check_ne "② 退避のままのペイン A が器へ繋がった端末を持つ（器のプロンプトで状態が動く）" "unknown" "$(bg_state_after_enter "$SA" "$PA")"
check_ne "② 退避のままのペイン B が器へ繋がった端末を持つ（器のプロンプトで状態が動く）" "unknown" "$(bg_state_after_enter "$SB" "$PB")"
check_eq "② 器 A へ attach しているクライアントは 1 つ" "1" "$(attached "$SA")"
check_eq "② 器 B へ attach しているクライアントは 1 つ" "1" "$(attached "$SB")"
check_eq "② 器 A のシェルは再起動前と同じプロセス" "$PID_A" "$(shell_pid "$SA")"
check_eq "② 器 B のシェルは再起動前と同じプロセス" "$PID_B" "$(shell_pid "$SB")"
check_eq "② 器 A の持ち主は A のまま（別 pane に拾われていない）" "$PA" "$(vessel_pane "$SA")"
check_eq "② 器 B の持ち主は B のまま（別 pane に拾われていない）" "$PB" "$(vessel_pane "$SB")"
check_eq "② 器は増えていない" "$SESSIONS_BEFORE" "$(session_count)"
wait_save
snap_layout
check_eq "④ 再起動後の保存でもたまり場エントリが器 A を持つ（幽霊を書かない）" "$SA" "$(lay "bg:$PA")"
check_eq "④ 再起動後の保存でも退避タブのペインが器 B を持つ" "$SB" "$(lay "st:$PB")"
T foreground "$PA" >/dev/null
T foreground --tab "$TAB2" >/dev/null
sleep 1
snap_list
check_eq "③ 表に出したペイン A に再起動前の目印が見える" "yes" "$(read_has "$PA" "$MARK_A")"
check_eq "③ 表に出したペイン B に再起動前の目印が見える" "yes" "$(read_has "$PB" "$MARK_B")"
check_eq "③ 表に出してもタブは元の 2 枚（新しいタブを作らない）" "2" "$(lv tabs)"
check_eq "③ 表に出しても器 A のシェルは同じプロセス" "$PID_A" "$(shell_pid "$SA")"
check_eq "③ 表に出しても器は増えていない" "$SESSIONS_BEFORE" "$(session_count)"
stop_app

echo "== ⑤ A/B: TAKO_1576_LEGACY=1 で修正前の症状が再現する =="
cp "$TMP/layout-base.json" "$LAYOUT"
start_app legacy TAKO_1576_LEGACY=1
snap_list
snap_bg
echo "  orphan: $(grep 'orphan 自動復帰' "$PLOG" | tail -1)"
echo "  タブ: $(lv tab_titles) / タブ配下: $(lv tab_panes) / 退避: $(lv shelved)"
check_eq "⑤ 旧挙動は orphan 自動復帰が 1 行出る" "1" "$(orphan_lines)"
check_has "⑤ 旧挙動は 2 セッションを「復帰」タブへ拾う" "2 セッション" "$(grep 'orphan 自動復帰' "$PLOG" | tail -1)"
check_eq "⑤ 旧挙動はタブが 2 枚（「復帰」タブが増える）" "2" "$(lv tabs)"
LEGACY_NEW="$(for p in $(lv tab_panes); do [ "$p" = "$P1" ] || echo "$p"; done | tr '\n' ' ')"
check_not "⑤ 旧挙動の「復帰」タブのペインは別 id（A ではない）" " $PA " " $LEGACY_NEW"
check_not "⑤ 旧挙動の「復帰」タブのペインは別 id（B ではない）" " $PB " " $LEGACY_NEW"
check_eq "⑤ 旧挙動の退避エントリ A は端末なし（幽霊 = 器のプロンプトでも状態が動かない）" "unknown" "$(bg_state_after_enter "$SA" "$PA")"
check_not "⑤ 旧挙動は器 A の持ち主を「復帰」タブの別 pane へ書き換える" " $PA " " $(vessel_pane "$SA") "
wait_save
snap_layout
check_eq "⑤ 旧挙動は幽霊のたまり場エントリを器なしで保存する" "-" "$(lay "bg:$PA")"
stop_app

echo "== ⑥ エッジ: 同じ器を指す 2 本目の退避エントリと、器も手掛かりも無いエントリ =="
DUP_ID=900001
GHOST_ID=900002
python3 "$TOOL" add_dup_and_ghost "$TMP/layout-base.json" "$LAYOUT" "$DUP_ID" "$GHOST_ID" \
  || { echo "細工した layout を作れない"; exit 1; }
start_app edge-dup
snap_list
echo "  失敗行:"; grep '復元失敗（ペイン' "$PLOG" | sed 's/^/    /'
check_eq "⑥ orphan 自動復帰が 0 件" "0" "$(orphan_lines)"
check_eq "⑥ 退避は元の 2 本だけ（重複と幽霊は外した）" "$PA $PB" "$(lv shelved)"
check_has "⑥ 重複のエントリを名指しで外した" "復元失敗（ペイン ${DUP_ID}）: 同じ器を別のペインが持つ" "$(grep '復元失敗（ペイン' "$PLOG")"
check_has "⑥ 幽霊のエントリを名指しで外した" "復元失敗（ペイン ${GHOST_ID}）: 器も戻す手掛かりも無い" "$(grep '復元失敗（ペイン' "$PLOG")"
check_eq "⑥ 器 A へ attach しているクライアントは 1 つ（二重に繋がない）" "1" "$(attached "$SA")"
check_eq "⑥ 器 A のシェルは同じプロセス" "$PID_A" "$(shell_pid "$SA")"
wait_save
snap_layout
check_eq "⑥ 外したエントリは layout に残らない（重複）" "" "$(lay "bg:$DUP_ID")"
check_eq "⑥ 外したエントリは layout に残らない（幽霊）" "" "$(lay "bg:$GHOST_ID")"
stop_app

echo "== ⑦ エッジ: 退避中のペインのプロセスが再起動の間に終わっていた =="
cp "$TMP/layout-base.json" "$LAYOUT"
TM kill-session -t "=$SA" >/dev/null 2>&1
check_eq "⑦ 器 A を消した（前提）" "" "$(shell_pid "$SA")"
start_app edge-dead
snap_list
snap_bg
echo "  2 行目: $(detail_line)"
check_eq "⑦ orphan 自動復帰が 0 件" "0" "$(orphan_lines)"
check_eq "⑦ ペイン A は同じ id で退避のまま戻る" "shelf-a|worker-1576a|true|-" "$(lv "sp:$PA")"
check_ne "⑦ ペイン A は新しいシェルを持つ（幽霊にしない）" "unknown" "$(bgv "state:$PA")"
check_ne "⑦ 器 A が同じ名前で作り直された" "" "$(shell_pid "$SA")"
check_ne "⑦ 作り直した器 A は別プロセス" "$PID_A" "$(shell_pid "$SA")"
check_has "⑦ 2 行目に新規シェルで戻したことが出る" "新規シェル 1" "$(detail_line)"
check_eq "⑦ 器 B は同じプロセスのまま" "$PID_B" "$(shell_pid "$SB")"
stop_app

echo "== ⑧ エッジ: 退避タブが無い layout（たまり場だけ） =="
TM kill-session -t "=$SB" >/dev/null 2>&1
python3 "$TOOL" drop_shelved_tabs "$TMP/layout-base.json" "$LAYOUT" \
  || { echo "細工した layout を作れない"; exit 1; }
start_app edge-notab
snap_list
echo "  2 行目: $(detail_line)"
check_eq "⑧ orphan 自動復帰が 0 件" "0" "$(orphan_lines)"
check_eq "⑧ 退避はたまり場の 1 本だけ" "$PA" "$(lv shelved)"
check_eq "⑧ 退避タブは無い" "" "$(lv shelved_tabs)"
check_eq "⑧ タブは 1 枚のまま" "1" "$(lv tabs)"
check_eq "⑧ 器 A へ attach しているクライアントは 1 つ" "1" "$(attached "$SA")"
stop_app

echo
echo "===================="
echo "PASS: $PASS / FAIL: $FAIL"
echo "===================="
[ "$FAIL" -eq 0 ]
