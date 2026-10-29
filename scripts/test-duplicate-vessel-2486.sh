#!/usr/bin/env bash
# test-duplicate-vessel-2486.sh — layout.json で複数のペインが同じ器の名前を持つときの復元（#2486）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#   ① 2 つのペインが同じ器の名前を持つ layout を復元しても、**ペインが黙って閉じない**
#      （先に起こしたペインが器へ再 attach し、2 本目は新しい器の新しいシェルで起きる。
#      理由は persist.log へ 1 行）
#   ② 3 本が同じ名前でも 3 本とも残る（器も 3 本）
#   ③ 重複した名前の器がもう無い（PC の再起動で器ごと消えた）ときも 2 本とも残る
#   ④ 名前が重複しない普通の復元（split した構成・再起動・再 attach）は修正前と同じ
#      （①の後にもう一度再起動すると、重複は解けていて普通の再 attach になる）
#   ⑤ persist off では復元しない（修正前と同じ）
#   ⑥ A/B: `TAKO_2486_LEGACY=1` で修正前の症状（片方のペインが閉じる）が同一バイナリで出る
#   ⑦ 会話の記録つきの重複（器が生きている）: 2 本目は会話を resume せず、記録も引き継がない
#      （内訳の新規シェルの理由は `器の名前の重複`）
# を実測する。
# 蓋閉じの機で tako-vd が起動の合間に眠ったときは、その起動だけをやり直す（start_app）。
#
# 器の中身の取り違えを見るため、各器へ印（`echo MARK_<ペイン>`）を打ってから終了し、
# 復元後にどのペインがどの印を映しているかを `tako read` で読む。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid と自分のソケットの器だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る = 他の
# `isolated-gui.sh` 系スクリプトと同じ扱い）。CI 側の担保は単体テスト
# （`tako_control::restore_vessels`）と番犬
# （`crates/tako-control/tests/issue2486_duplicate_vessel_watchdog.rs`）。
#
# 使い方: bash scripts/test-duplicate-vessel-2486.sh
#        TAKO_2486_EXPECT_LEGACY=1 bash scripts/test-duplicate-vessel-2486.sh   # 修正前のバイナリで症状だけを見る
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
# 修正前のバイナリ（main）で回すときは「症状が出る」ことだけを確かめる
EXPECT_LEGACY="${TAKO_2486_EXPECT_LEGACY:-}"

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

TMP="$(mktemp -d /tmp/tako-2486-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2486-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # サーバーが落ちてもソケットファイルが残ることがある（自分の名前の 1 本だけ消す）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${TMUX_SOCKET:?}"
  rm -rf "${TMP:?}"
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
PLOG="$TAKO_DATA_DIR/persist.log"
tm() { tmux -L "$TMUX_SOCKET" "$@"; }

#   start_app <ログの名前> [VAR=VAL …]
#
# 蓋閉じの機では tako-vd が起動の合間にディスプレイスリープで OS の一覧から落ち、GUI が
# 窓を開かずに終わる（#1160 の設計どおり）。環境の揺れなので、**その終わり方のときだけ**
# 起動前の layout.json を戻してやり直す（launch_isolated_gui の ensure が毎回面を起こす）
start_app() {
  local name="$1" try log
  shift
  [ -f "$LAYOUT" ] && command cp -f "$LAYOUT" "$TMP/layout-before-start.json"
  for try in 1 2 3 4 5; do
    log="$TMP/app-${name}-${try}.log"
    : > "$PLOG"
    launch_isolated_gui "$log" ${@+"$@"} || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    if wait_isolated_gui "$log" >/dev/null; then
      return 0
    fi
    stop_isolated_gui "$APP_PID"
    APP_PID=""
    if ! grep -q '窓を開かずに終了' "$log" 2>/dev/null; then
      echo "tako-app へ接続できない:"
      tail -20 "$log"
      exit 1
    fi
    echo "  （tako-vd が眠っていて窓を開けなかった。やり直す: ${try} 回目）"
    [ -f "$TMP/layout-before-start.json" ] && command cp -f "$TMP/layout-before-start.json" "$LAYOUT"
    sleep 2
  done
  echo "tako-vd を起こせず GUI を立てられない（未実測）"
  exit 4
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  sleep 1
}
# 復元の後、attach クライアントが切り離されてペインが閉じる（修正前）までの猶予を置いて読む。
# 閉じるのは器への問い合わせ（上限つき）の後なので、ペイン数が 2 秒続けて変わらなくなるまで待つ
settle() {
  local prev="" cur i stable=0
  for i in $(seq 1 40); do
    cur="$(pane_ids | tr '\n' ' ')"
    if [ "$cur" = "$prev" ]; then
      stable=$((stable + 1))
      [ "$stable" -ge 8 ] && return 0
    else
      stable=0
    fi
    prev="$cur"
    sleep 0.25
  done
}
pane_ids() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        print(p["id"])'
}
pane_count() { pane_ids | grep -c . | tr -d ' '; }
vessel_count() { tm ls -F '#{session_name}' 2>/dev/null | grep -c . | tr -d ' '; }
# ペインの画面に出ている印（MARK_<数字>）を空白区切りで
marks_of() {
  "$TAKO_BIN" read --pane "$1" 2>/dev/null | grep -o 'MARK_[0-9]*' | sort -u | tr '\n' ' ' | sed 's/ $//'
}

TOOL="$TMP/layout_tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys

def panes(node, acc):
    if node.get("type") == "pane" or ("id" in node and "first" not in node):
        acc.append(node)
    else:
        panes(node["first"], acc)
        panes(node["second"], acc)
    return acc

def tab_panes(d):
    out = []
    for t in d["tabs"]:
        panes(t["tree"], out)
    return out

cmd, path = sys.argv[1], sys.argv[2]
d = json.load(open(path))
if cmd == "sessions":
    # 「ペイン 器の名前」を 1 行ずつ
    for p in tab_panes(d):
        print(p["id"], p.get("session") or "-")
elif cmd == "dup":
    # dup <layout> <dst> <元のペイン> <写し先のペイン…>: 写し先の器の名前を元のペインの名前にする
    dst, src, targets = sys.argv[3], int(sys.argv[4]), [int(x) for x in sys.argv[5:]]
    ps = {p["id"]: p for p in tab_panes(d)}
    name = ps[src]["session"]
    for t in targets:
        ps[t]["session"] = name
    json.dump(d, open(dst, "w"), ensure_ascii=False)
    print(name)
elif cmd == "claude":
    # claude <layout> <ペイン> <会話 ID>: そのペインに claude の会話の記録を持たせる（その場で書き換える）
    ps = {p["id"]: p for p in tab_panes(d)}
    ps[int(sys.argv[3])]["claude_session_id"] = sys.argv[4]
    json.dump(d, open(path, "w"), ensure_ascii=False)
elif cmd == "claude_of":
    ps = {p["id"]: p for p in tab_panes(d)}
    print(ps[int(sys.argv[3])].get("claude_session_id") or "-")
else:
    raise SystemExit(f"未知のコマンド: {cmd}")
PYEOF
session_of() { python3 -I "$TOOL" sessions "$1" | awk -v p="$2" '$1==p {print $2}'; }

echo "== 隔離 GUI を起こして 3 ペインの構成を組む =="
start_app base
P1="$(pane_ids | sed -n '1p')"
if [ "$(pane_count)" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（ペイン $(pane_count) 枚）。中止する。"; exit 1
fi
P2="$("$TAKO_BIN" split --pane "$P1" --right | tr -dc '0-9')"
P3="$("$TAKO_BIN" split --pane "$P2" --down | tr -dc '0-9')"
echo "  pid=$APP_PID panes=$P1 $P2 $P3 socket=$TMUX_SOCKET"
sleep 3   # layout.json の保存（2 秒ポーリング）を待つ
for p in "$P1" "$P2" "$P3"; do
  name="$(session_of "$LAYOUT" "$p")"
  [ -n "$name" ] && [ "$name" != "-" ] || { echo "ペイン $p の器の名前が layout に無い"; exit 1; }
  # 器へ印を打つ（どのペインがどの器を映しているかを後で読む）
  tm send-keys -t "=${name}:" "echo MARK_$p" Enter
done
sleep 1
stop_app
cp "$LAYOUT" "$TMP/layout-good.json"
S1="$(session_of "$TMP/layout-good.json" "$P1")"
S2="$(session_of "$TMP/layout-good.json" "$P2")"
S3="$(session_of "$TMP/layout-good.json" "$P3")"
check_eq "器が 3 本ある（ペインごとに 1 本）" "3" "$(vessel_count)"

# 器の集合を「元の 3 本」に戻す（前の場合が作った器・消した器を揃える）
reset_vessels() {
  local p s screen
  tm ls -F '#{session_name}' 2>/dev/null | while read -r s; do
    case "$s" in "$S1" | "$S2" | "$S3") : ;; *) tm kill-session -t "=$s" ;; esac
  done
  for p in "$P1" "$P2" "$P3"; do
    s="$(session_of "$TMP/layout-good.json" "$p")"
    # 印が消えた器（③で消して tako が作り直した器）も作り直す
    if tm has-session -t "=$s" 2>/dev/null; then
      screen="$(tm capture-pane -p -t "=${s}:" 2>/dev/null)"
      [[ "$screen" == *"MARK_$p"* ]] || tm kill-session -t "=$s"
    fi
    if ! tm has-session -t "=$s" 2>/dev/null; then
      # 印は器のコマンド自身に出させる（起きたばかりのシェルへの打鍵は初期化に食われることがある）
      tm new-session -d -s "$s" "echo MARK_$p; exec /bin/sh"
    fi
  done
  sleep 0.5
}

# 修正前（症状）と修正後（期待）で分かれる検査。修正前のバイナリで回すときは症状の側を見る
expect_kept() {
  # expect_kept <ラベル> <期待するペイン数> <修正前の症状のペイン数>
  if [ -n "$EXPECT_LEGACY" ]; then
    check_eq "$1: 修正前はペインが閉じる（症状）" "$3" "$(pane_count)"
  else
    check_eq "$1: ペインが閉じない" "$2" "$(pane_count)"
  fi
}

run_dup_case() {
  # run_dup_case <名前> <legacy の env か空> <器を消すか 0/1> <写し先…>
  local name="$1" legacy_env="$2" kill_vessel="$3"
  shift 3
  reset_vessels
  python3 -I "$TOOL" dup "$TMP/layout-good.json" "$LAYOUT" "$P1" "$@" >/dev/null
  # 写し先が元々持っていた器は layout から外れる（= orphan 自動復帰で「復帰」タブへ拾われる）。
  # 検証ではペイン数を復元の結果だけにしたいので消しておく
  local t
  for t in "$@"; do
    tm kill-session -t "=$(session_of "$TMP/layout-good.json" "$t")" 2>/dev/null
  done
  [ "$kill_vessel" = "1" ] && tm kill-session -t "=$S1" 2>/dev/null
  if [ -n "$legacy_env" ]; then
    start_app "$name" "$legacy_env"
  else
    start_app "$name"
  fi
  settle
}

TOTAL=3

echo "== ① 2 つのペインが同じ器の名前（${P1} と ${P2}）=="
run_dup_case dup2 "" 0 "$P2"
echo "  ペイン: $(pane_ids | tr '\n' ' ')/ 器: $(vessel_count) 本"
expect_kept "①" "$TOTAL" "$((TOTAL - 1))"
echo "  persist.log（関係する行）:"
grep -E '器の名前|attach クライアント|復元の内訳|復元成功' "$PLOG" | sed 's/^/    /'
if [ -z "$EXPECT_LEGACY" ]; then
  check_eq "① 器が 3 本（重複した 2 本目は新しい器）" "3" "$(vessel_count)"
  check_eq "① 先のペイン $P1 は元の器へ再 attach（印 MARK_${P1}）" "MARK_$P1" "$(marks_of "$P1")"
  check_eq "① 2 本目 $P2 は元の器を映さない（新しいシェル）" "" "$(marks_of "$P2")"
  check_eq "① 重複しないペイン $P3 は自分の器のまま" "MARK_$P3" "$(marks_of "$P3")"
  check_eq "① 理由が persist.log に 1 行" "1" "$(grep -c "器の名前の重複: pane=$P2 " "$PLOG" | tr -d ' ')"
  check_has "① 理由の行が先に持つペインを名指す" "ペイン ${P1} が先に持つ" "$(grep "器の名前の重複: pane=$P2 " "$PLOG")"
  check_eq "① 片方を閉じる行（別のクライアントが attach 中）は出ない" "0" \
    "$(grep -c '別のクライアント' "$PLOG" | tr -d ' ')"
  NEW2="$(tm ls -F '#{session_name}' | grep -v -x -e "$S1" -e "$S3" | sed -n '1p')"
  check_eq "① 2 本目の新しい器は元の名前と違う" "1" "$([ -n "$NEW2" ] && [ "$NEW2" != "$S1" ] && echo 1 || echo 0)"
  sleep 3   # 保存を待つ
  check_eq "① 次の保存で layout の 2 本目は新しい器の名前" "$NEW2" "$(session_of "$LAYOUT" "$P2")"
  check_eq "① 次の保存で layout の 1 本目は元の器の名前" "$S1" "$(session_of "$LAYOUT" "$P1")"
fi
stop_app

if [ -z "$EXPECT_LEGACY" ]; then
  echo "== ④ ①の後にもう一度再起動: 重複は解けていて普通の再 attach =="
  start_app again
  settle
  check_eq "④ ペインが 3 本" "3" "$(pane_count)"
  check_eq "④ 器が 3 本" "3" "$(vessel_count)"
  check_eq "④ 重複の行は出ない" "0" "$(grep -c '器の名前の重複' "$PLOG" | tr -d ' ')"
  check_has "④ 3 本とも tmux 再 attach" "tmux 再 attach 3" "$(grep '復元成功' "$PLOG" | grep 'tmux 再 attach' | tail -1)"
  stop_app
fi

echo "== ② 3 本が同じ器の名前 =="
run_dup_case dup3 "" 0 "$P2" "$P3"
echo "  ペイン: $(pane_ids | tr '\n' ' ')/ 器: $(vessel_count) 本"
expect_kept "②" "$TOTAL" "1"
grep -E '器の名前|attach クライアント' "$PLOG" | sed 's/^/    /'
if [ -z "$EXPECT_LEGACY" ]; then
  check_eq "② 器が 3 本" "3" "$(vessel_count)"
  check_eq "② 理由の行が 2 本ぶん" "2" "$(grep -c '器の名前の重複' "$PLOG" | tr -d ' ')"
  check_eq "② 先のペインは元の器（印 MARK_${P1}）" "MARK_$P1" "$(marks_of "$P1")"
fi
stop_app

echo "== ③ 重複した名前の器がもう無い（器ごと消えた）=="
run_dup_case gone "" 1 "$P2"
echo "  ペイン: $(pane_ids | tr '\n' ' ')/ 器: $(vessel_count) 本"
expect_kept "③" "$TOTAL" "$((TOTAL - 1))"
grep -E '器の名前|attach クライアント' "$PLOG" | sed 's/^/    /'
if [ -z "$EXPECT_LEGACY" ]; then
  check_eq "③ 器が 3 本" "3" "$(vessel_count)"
  check_eq "③ 理由が persist.log に 1 行" "1" "$(grep -c "器の名前の重複: pane=$P2 " "$PLOG" | tr -d ' ')"
fi
stop_app

if [ -z "$EXPECT_LEGACY" ]; then
  echo "== ⑦ 会話の記録つきの重複（器が生きている）: 2 本目は会話を resume せず記録も引き継がない =="
  reset_vessels
  python3 -I "$TOOL" dup "$TMP/layout-good.json" "$LAYOUT" "$P1" "$P2" >/dev/null
  tm kill-session -t "=$S2" 2>/dev/null
  CONV1="11111111-2222-3333-4444-555555555555"
  CONV2="66666666-7777-8888-9999-000000000000"
  python3 -I "$TOOL" claude "$LAYOUT" "$P1" "$CONV1"
  python3 -I "$TOOL" claude "$LAYOUT" "$P2" "$CONV2"
  start_app conv
  settle
  check_eq "⑦ ペインが 3 本" "3" "$(pane_count)"
  check_has "⑦ 理由の行が resume しないことを言う" "会話は resume しない（器が生きていて" \
    "$(grep "器の名前の重複: pane=$P2 " "$PLOG")"
  check_has "⑦ 内訳の新規シェルの理由が「器の名前の重複」" "器の名前の重複 1" "$(grep '復元の内訳' "$PLOG")"
  check_eq "⑦ claude は起こしていない（resume 0）" "0" \
    "$(grep '復元成功' "$PLOG" | grep 'tmux 再 attach' | grep -o 'Claude resume [0-9]*' | awk '{print $3}')"
  sleep 3   # 保存を待つ
  check_eq "⑦ 次の保存で 2 本目は会話の記録を持たない" "-" "$(python3 -I "$TOOL" claude_of "$LAYOUT" "$P2")"
  check_eq "⑦ 次の保存で 1 本目は会話の記録を保つ" "$CONV1" "$(python3 -I "$TOOL" claude_of "$LAYOUT" "$P1")"
  stop_app

  echo "== ⑥ A/B: TAKO_2486_LEGACY=1 で修正前の症状 =="
  run_dup_case legacy "TAKO_2486_LEGACY=1" 0 "$P2"
  echo "  ペイン: $(pane_ids | tr '\n' ' ')/ 器: $(vessel_count) 本"
  check_eq "⑥ 旧挙動は片方のペインが閉じる" "$((TOTAL - 1))" "$(pane_count)"
  check_eq "⑥ 旧挙動は重複の行を出さない" "0" "$(grep -c '器の名前の重複' "$PLOG" | tr -d ' ')"
  grep -E 'attach クライアント' "$PLOG" | sed 's/^/    /'
  stop_app
fi

echo "== ④ 重複なしの普通の復元（元の layout）=="
reset_vessels
cp "$TMP/layout-good.json" "$LAYOUT"
start_app normal
settle
check_eq "④ ペインが 3 本" "3" "$(pane_count)"
check_eq "④ 器が 3 本" "3" "$(vessel_count)"
check_eq "④ $P1 は自分の器（印）" "MARK_$P1" "$(marks_of "$P1")"
check_eq "④ $P2 は自分の器（印）" "MARK_$P2" "$(marks_of "$P2")"
check_eq "④ 重複の行は出ない" "0" "$(grep -c '器の名前の重複' "$PLOG" | tr -d ' ')"
# split した新しいペインも従来どおり新しい器を持つ
"$TAKO_BIN" split --pane "$P3" --right >/dev/null
sleep 1
check_eq "④ split したペインに器が 1 本増える" "4" "$(vessel_count)"
stop_app

echo "== ⑤ persist off: 重複した layout があっても復元しない（修正前と同じ）=="
python3 -I "$TOOL" dup "$TMP/layout-good.json" "$LAYOUT" "$P1" "$P2" >/dev/null
start_app off TAKO_PERSIST=0
settle
check_eq "⑤ persist off はペイン 1 本（復元しない）" "1" "$(pane_count)"
check_eq "⑤ 重複の行は出ない" "0" "$(grep -c '器の名前の重複' "$PLOG" 2>/dev/null | tr -d ' ')"
stop_app

echo
echo "===================="
echo "PASS: $PASS / FAIL: $FAIL"
echo "===================="
[ "$FAIL" -eq 0 ]
