#!/usr/bin/env bash
# test-limit-resume-restart-1710.sh — 利用上限の自動復帰が GUI の再起動をまたいで
# **告知を最初に見た時刻**から解除時刻を解くことの実経路テスト（#1710）
#
# 隔離した data / tmux（器 = 永続 ON）で**実 tako-app** を立て、
#   ① 自動復帰を ON にしたペインへ上限の告知（`You've hit your weekly limit · resets <時> (Asia/Tokyo)`。
#      <時> は 1〜2 時間前の正時）を出すと、追跡が始まり layout.json の `limit_seen` に告知の要約と
#      検知時刻が載る（告知の本文は載らない）
#   ② GUI を落とし、layout.json の検知時刻を 24 時間前へずらす（= 「前日に出た告知が画面に残ったまま
#      翌日に再起動した」を時計に触らずに作る。器のシェルは生きていて画面の告知はそのまま）
#   ③ A/B `TAKO_1710_LEGACY=1` で再起動すると、修正前の症状（解除時刻 = **明日の** <時>・
#      再開予定はその 2 分後）が同一バイナリで再現する
#   ④ 修正後で再起動すると、検知時刻は layout.json の値・解除時刻は**今日の** <時>（過ぎている）・
#      再開予定は「追跡を作り直した時刻 + 2 分」になる
#   ⑤ エッジ: 画面から告知が数秒消えて（`clear`）また同じ告知が出ても記憶は保たれ、検知時刻は動かない。
#      そのまま再開予定を過ぎると実際に継続ナッジが積まれる（supervisor.log の `nudge queued`）
#   ⑥ エッジ: もう一度再起動し、告知を一度消してから別の告知（違う解除時刻）を出すと、
#      いま初めて見た告知として解き、記憶も書き換える
# 画面を変える打鍵は器（tmux）へ直に送る（`tako send` は送達確認つきの順番待ちに載るので、
# 継続ナッジが入力欄を待っている間は後ろで止まる）
# を実測する。時刻は TZ=Asia/Tokyo に固定する（GUI の `localtime_r` も同じ TZ を読む）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット
# `TAKO_TMUX_SOCKET`、落とすのは自分で起こした pid と自分のソケットだけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。面を用意できなければ終了コード 4（未実測）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る）。CI 側の担保は
# 単体テスト（`tako_core::limit_resume` / `tako_control::limit_stop` / `tako-app::limit_autoresume`）と
# 番犬（`crates/tako-control/tests/issue1710_limit_reset_detected_base.rs`）。
#
# 使い方: bash scripts/test-limit-resume-restart-1710.sh
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
# check_range <名前> <下限> <上限> <値>（両端を含む整数）
check_range() {
  if [ -n "$4" ] && [ "$4" -ge "$2" ] 2>/dev/null && [ "$4" -le "$3" ] 2>/dev/null; then
    pass "$1（${4}）"
  else
    fail "$1（期待 ${2}..${3} / 実際 '${4}'）"
  fi
}

TMP="$(mktemp -d /tmp/tako-1710-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1710-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
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
export TZ=Asia/Tokyo
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# 器の画面が再起動をまたいで残ることそのものを見るので永続は ON（隔離 GUI の既定は OFF）
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

LAYOUT="$TAKO_DATA_DIR/layout.json"
AUDIT="$TAKO_DATA_DIR/supervisor.log"
T() { "$TAKO_BIN" "$@"; }
TM() { tmux -L "$TMUX_SOCKET" "$@"; }

#   start_app <ログの名前> [VAR=VAL …]
start_app() {
  local name="$1"
  shift
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  # attach クライアントが器へ繋がり切るのを少し待つ
  sleep 2
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  sleep 1
}

# JSON の解析・layout の細工は 1 本のツールへ寄せる
TOOL="$TMP/tool.py"
cat > "$TOOL" <<'PYEOF'
import json, sys, time

def panes(node, acc):
    if "first" in node:
        panes(node["first"], acc)
        panes(node["second"], acc)
    else:
        acc.append(node)
    return acc

cmd = sys.argv[1]
if cmd == "hour":
    # 正時（ローカル = TZ）の表記（3pm）と unix 秒。引数なし = 1〜2 時間前・数 = その時間だけ先
    t = time.time() + 3600 * (int(sys.argv[2]) if len(sys.argv) > 2 else -1)
    lt = time.localtime(t)
    start = int(time.mktime(time.struct_time((lt.tm_year, lt.tm_mon, lt.tm_mday, lt.tm_hour,
                                               0, 0, lt.tm_wday, lt.tm_yday, lt.tm_isdst))))
    h = lt.tm_hour
    label = "%d%s" % (12 if h % 12 == 0 else h % 12, "am" if h < 12 else "pm")
    print(label, start)
elif cmd == "state":
    # tako limit-resume --pane N の state の要約（無ければ "-"）
    d = json.load(open(sys.argv[2]))
    st = d.get("state") or {}
    for k in ("detected_at", "reset_at", "resume_at", "attempts", "last_outcome"):
        v = st.get(k)
        print(k, "-" if v is None else v)
elif cmd == "seen":
    # layout.json の該当ペインの limit_seen（無ければ "-"）
    d = json.load(open(sys.argv[2]))
    pane = int(sys.argv[3])
    for t in d["tabs"]:
        for p in panes(t["tree"], []):
            if p["id"] == pane:
                s = p.get("limit_seen")
                print("notice", s["notice"] if s else "-")
                print("detected_at", s["detected_at"] if s else "-")
                print("raw", json.dumps(p, ensure_ascii=False))
elif cmd == "session":
    # layout.json の該当ペインの器（tmux セッション名）
    d = json.load(open(sys.argv[2]))
    pane = int(sys.argv[3])
    for t in d["tabs"]:
        for p in panes(t["tree"], []):
            if p["id"] == pane:
                print(p.get("session") or "")
elif cmd == "shift":
    # layout.json の limit_seen.detected_at を指定秒さかのぼらせる
    src, pane, secs = sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
    d = json.load(open(src))
    n = 0
    for t in d["tabs"]:
        for p in panes(t["tree"], []):
            if p["id"] == pane and p.get("limit_seen"):
                p["limit_seen"]["detected_at"] -= secs
                n += 1
    json.dump(d, open(src, "w"), ensure_ascii=False)
    print(n)
else:
    raise SystemExit("未知のコマンド: " + cmd)
PYEOF

# 状態の 1 項目（tako limit-resume --pane N の state）
state_snap() {
  T limit-resume --pane "$1" > "$TMP/state.json" 2>/dev/null || echo '{}' > "$TMP/state.json"
  python3 "$TOOL" state "$TMP/state.json" > "$TMP/state.txt" 2>/dev/null || : > "$TMP/state.txt"
}
sv() { awk -v k="$1" '$1==k {$1=""; sub(/^ /, ""); print}' "$TMP/state.txt"; }
# 追跡が始まるまで待つ（2 秒 tick + 画面の描き直し）
wait_tracking() {
  local pane="$1" i
  for i in $(seq 1 30); do
    state_snap "$pane"
    [ "$(sv reset_at)" != "" ] && [ "$(sv reset_at)" != "-" ] && return 0
    sleep 1
  done
  return 1
}
seen_snap() { python3 "$TOOL" seen "$LAYOUT" "$1" > "$TMP/seen.txt" 2>/dev/null || : > "$TMP/seen.txt"; }
seenv() { awk -v k="$1" '$1==k {$1=""; sub(/^ /, ""); print}' "$TMP/seen.txt"; }

read -r LABEL RESET_TODAY <<EOF
$(python3 "$TOOL" hour)
EOF
RESET_TOMORROW=$((RESET_TODAY + 86400))
echo "== 告知の解除時刻: ${LABEL}（今日 = ${RESET_TODAY} / 明日 = ${RESET_TOMORROW}）=="

# 上限で idle のまま止まった画面を出すスクリプト（告知の行 + 入力待ちの代わりの sleep）
SCREEN="$TMP/limit-screen.sh"
cat > "$SCREEN" <<'SHEOF'
#!/bin/bash
clear
printf '⏺ 実装を進めます\n\n'
printf "  ⎿  You've hit your weekly limit · resets %s (Asia/Tokyo)\n\n" "$1"
exec sleep 100000
SHEOF
chmod +x "$SCREEN"

echo "== ① 隔離 GUI で告知を出し、追跡と記憶が始まる =="
start_app 1
P="$(T list | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["tabs"][0]["panes"][0]["id"])')"
TABS="$(T list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR socket=$TMUX_SOCKET pane=$P"
T limit-resume on --pane "$P" >/dev/null
T send --pane "$P" "$SCREEN $LABEL" >/dev/null
if wait_tracking "$P"; then pass "上限の告知で追跡が始まる"; else fail "追跡が始まらない: $(cat "$TMP/state.json")"; fi
D1="$(sv detected_at)"
check_eq "初回の検知は解除時刻を明日の ${LABEL} へ解く（告知はいま出た）" "$RESET_TOMORROW" "$(sv reset_at)"
sleep 3 # layout の保存（2 秒 tick）を待つ
seen_snap "$P"
check_eq "layout.json に検知時刻が載る" "$D1" "$(seenv detected_at)"
case "$(seenv raw)" in
  *"weekly limit"* | *"resets"*) fail "layout.json に告知の本文が載っている: $(seenv raw)" ;;
  *) pass "layout.json に告知の本文は載らない（要約 $(seenv notice)）" ;;
esac
stop_app

echo "== ② 前日に出た告知として再起動する（layout.json の検知時刻を 24 時間前へ）=="
check_eq "検知時刻をずらしたペイン数" "1" "$(python3 "$TOOL" shift "$LAYOUT" "$P" 86400)"
D0=$((D1 - 86400))

echo "== ③ A/B: TAKO_1710_LEGACY=1（修正前）で再起動 =="
start_app 2 TAKO_1710_LEGACY=1
if wait_tracking "$P"; then pass "再起動後に追跡が始まる（legacy）"; else fail "追跡が始まらない（legacy）"; fi
check_eq "修正前は解除時刻を明日の ${LABEL} へ解き直す（症状の再現）" "$RESET_TOMORROW" "$(sv reset_at)"
check_eq "修正前の再開予定は明日の ${LABEL} + 2 分" "$((RESET_TOMORROW + 120))" "$(sv resume_at)"
echo "  legacy の状態: $(tr '\n' ' ' < "$TMP/state.txt")"
stop_app

echo "== ④ 修正後で再起動 =="
start_app 3
VESSEL="$(python3 "$TOOL" session "$LAYOUT" "$P")"
T0="$(date +%s)"
if wait_tracking "$P"; then pass "再起動後に追跡が始まる"; else fail "追跡が始まらない"; fi
T1="$(date +%s)"
echo "  修正後の状態: $(tr '\n' ' ' < "$TMP/state.txt")"
check_eq "検知時刻は layout.json の値（前日）を引き継ぐ" "$D0" "$(sv detected_at)"
check_eq "解除時刻は前日の告知から解いた今日の ${LABEL}（過ぎている）" "$RESET_TODAY" "$(sv reset_at)"
check_range "再開予定は追跡を作り直した時刻 + 2 分" "$((T0 + 120 - 3))" "$((T1 + 120))" "$(sv resume_at)"

echo "== ⑤ エッジ: 告知が数秒消えてまた出ても記憶は保たれ、再開予定で実際に再開する =="
if [ -z "$VESSEL" ]; then fail "器の名前が layout.json に無い"; fi
TM send-keys -t "=${VESSEL}:" C-c
sleep 1
TM send-keys -t "=${VESSEL}:" "clear; sleep 8; $SCREEN $LABEL" Enter
sleep 4
state_snap "$P"
check_eq "告知が消えている間は追跡が無い" "-" "$(sv reset_at)"
if wait_tracking "$P"; then pass "同じ告知が戻ると追跡が始まる"; else fail "追跡が戻らない"; fi
check_eq "検知時刻は動かない（記憶を保った）" "$D0" "$(sv detected_at)"
check_eq "解除時刻も今日の ${LABEL} のまま" "$RESET_TODAY" "$(sv reset_at)"
sleep 3
seen_snap "$P"
check_eq "layout.json の記憶も前日のまま" "$D0" "$(seenv detected_at)"
RESUME_AT="$(sv resume_at)"
# 再開予定を過ぎるまで待ち、実際に継続ナッジが積まれることを見る
NOW="$(date +%s)"
WAIT=$((RESUME_AT - NOW + 6))
[ "$WAIT" -gt 0 ] && { echo "  再開予定まで ${WAIT} 秒待つ"; sleep "$WAIT"; }
for i in $(seq 1 15); do
  state_snap "$P"
  [ "$(sv attempts)" != "0" ] && [ "$(sv attempts)" != "-" ] && break
  sleep 1
done
check_eq "再開予定を過ぎたら 1 回目の試行をする" "1" "$(sv attempts)"
if rows="$(grep "action=limit_autoresume" "$AUDIT" 2>/dev/null)" && grep -q "nudge queued" <<<"$rows"; then
  pass "supervisor.log に nudge queued が残る"
else
  fail "supervisor.log に nudge queued が無い: $(tail -3 "$AUDIT" 2>/dev/null)"
fi
stop_app

echo "== ⑥ エッジ: 別の告知（違う解除時刻）は新しい告知として記憶を書き換える =="
read -r LABEL2 RESET2 <<HOUR2
$(python3 "$TOOL" hour 3)
HOUR2
start_app 4
TM send-keys -t "=${VESSEL}:" C-c
sleep 1
T2="$(date +%s)"
# 一度画面から告知を消す（= エピソードの終わり）。告知が画面に残ったまま入れ替わると、
# 追跡はエピソード中の解除時刻を動かさない（#813 の規則。記憶だけが新しい告知へ書き換わる）
TM send-keys -t "=${VESSEL}:" "clear; sleep 4; $SCREEN $LABEL2" Enter
for i in $(seq 1 30); do
  state_snap "$P"
  [ "$(sv reset_at)" = "$RESET2" ] && break
  sleep 1
done
T3="$(date +%s)"
check_eq "解除時刻は新しい告知の ${LABEL2}" "$RESET2" "$(sv reset_at)"
check_range "検知時刻はいま（新しい告知を初めて見た時刻）" "$T2" "$T3" "$(sv detected_at)"
sleep 3
seen_snap "$P"
check_range "layout.json の記憶も書き換わる" "$T2" "$T3" "$(seenv detected_at)"
stop_app

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
