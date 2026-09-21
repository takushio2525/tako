#!/usr/bin/env bash
# test-shelve-tab-1487.sh — タブ単位の退避・復帰の実経路テスト（#1487）
#
# 隔離した data / tmux で**実 tako-app** を立て、CLI から
#   ① 3 ペイン（縦横混在・比率 0.3 / 0.7・1 本は手動タイトル）のタブを `background --tab` →
#      `backgrounded` の `tabs` に 1 件 → `foreground --tab` → `list` の tree / rect /
#      title / title_source が退避前と**一致**する
#   ② GUI を落として起こし直しても退避タブが残り、同じ手順で戻る（layout.json 往復）
#   ③ 退避タブから 1 ペインだけ `foreground <pane>` → 残り 2 ペインで `--tab` 復帰
#   ④ 全部抜くと `tabs` から消える
#   ⑤ 旧 layout.json（`shelved_tabs` 無し・平坦な `backgrounded` あり）がそのまま読める
#   ⑥ A/B `TAKO_1487_LEGACY=1` で旧挙動（平坦化）が同一バイナリで再現する
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-shelve-tab-1487.sh
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

TMP="$(mktemp -d /tmp/tako-1487-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1487-$$"
cleanup() {
  if [ -n "$APP_PID" ]; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}"
if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
  echo "バイナリをビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet)
fi
for b in "$TAKO_BIN" "$APP_BIN"; do
  [ -x "$b" ] || { echo "バイナリが見つからない: $b"; exit 1; }
done

bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || \
  echo "  (注) 仮想ディスプレイを用意できなかった: 既定の面で続行する"

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
# layout.json の往復（②）を見るので永続は ON（save_layout は tmux_persist が条件）
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 受け入れ条件の比較対象（tree / rect / title / title_source）だけを採る。
# cols / rows / focused / scroll / state は器を作り直すと当然変わる揮発値なので入れない
PY_SNAP='import json,sys
tab = int(sys.argv[1])
d = json.load(sys.stdin)
for t in d["tabs"]:
    if t["id"] == tab:
        print(json.dumps({"title": t["title"], "title_source": t["title_source"],
                          "tree": t["tree"],
                          "panes": [{k: p[k] for k in ("id", "rect", "title", "title_source", "role")}
                                    for p in sorted(t["panes"], key=lambda p: p["id"])]},
                         ensure_ascii=False, sort_keys=True, indent=1))
        sys.exit(0)
print("__NOT_FOUND__")'

start_app() {
  # 面は起動のたびに起こす（#1141 / #1160。蓋閉じ運用では tako-vd が Main になり
  # アイドルで眠るので、1 回目と 2 回目のあいだに列挙から落ちることがある。
  # `ensure` は冪等で、常設の面を作り直したり消したりはしない）
  bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || true
  "$APP_BIN" > "$TMP/app-$1.log" 2>&1 &
  APP_PID=$!
  for _ in $(seq 1 200); do
    if "$TAKO_BIN" list >/dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  echo "tako-app へ接続できない:"; tail -20 "$TMP/app-$1.log"; exit 1
}
stop_app() {
  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  APP_PID=""
  sleep 1
}

echo "== 隔離 GUI を起こす =="
start_app 1
TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

echo "== 検証用のタブを組む（3 ペイン・縦横混在・比率 0.3 / 0.7・1 本は手動タイトル） =="
TAB1="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["id"])')"
P1="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
P2="$("$TAKO_BIN" split --pane "$P1" --right --ratio 0.7 | tr -dc '0-9')"
P3="$("$TAKO_BIN" split --pane "$P2" --down --ratio 0.3 | tr -dc '0-9')"
"$TAKO_BIN" title --pane "$P3" "検証用ペイン" >/dev/null
# 退避しても最後の 1 タブにならないよう、もう 1 枚作る
"$TAKO_BIN" tab new >/dev/null
"$TAKO_BIN" tab select "$TAB1" >/dev/null
BEFORE="$("$TAKO_BIN" list | python3 -c "$PY_SNAP" "$TAB1")"
echo "$BEFORE" > "$TMP/before.json"
PANES="$(printf '%s' "$BEFORE" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["panes"]))')"
check_eq "3 ペインのタブを組めた" "3" "$PANES"
check_eq "手動タイトルが付いている" "manual" \
  "$(printf '%s' "$BEFORE" | python3 -c 'import json,sys
d=json.load(sys.stdin)
print([p["title_source"] for p in d["panes"] if p["title"]=="検証用ペイン"][0])')"

echo "== ① background --tab → backgrounded の tabs に 1 件 =="
"$TAKO_BIN" background --tab "$TAB1" >/dev/null
BG="$("$TAKO_BIN" backgrounded)"
check_eq "tabs が 1 件" "1" "$(printf '%s' "$BG" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
check_eq "退避タブの ID が一致" "$TAB1" "$(printf '%s' "$BG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["tab"])')"
check_eq "退避タブは 3 ペインを保つ" "3" "$(printf '%s' "$BG" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"][0]["panes"]))')"
check_eq "平坦な一覧にも退避タブ配下が出る" "3" \
  "$(printf '%s' "$BG" | python3 -c 'import json,sys; print(len([p for p in json.load(sys.stdin)["backgrounded"] if p.get("shelved_tab")]))')"
check_eq "list の shelved_tabs に 1 件" "1" \
  "$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["shelved_tabs"]))')"
check_eq "退避後はタブ一覧から消える" "1" \
  "$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"

echo "== ① foreground --tab → tree / rect / title / title_source が一致 =="
"$TAKO_BIN" foreground --tab "$TAB1" >/dev/null
AFTER="$("$TAKO_BIN" list | python3 -c "$PY_SNAP" "$TAB1")"
echo "$AFTER" > "$TMP/after.json"
if [ "$BEFORE" = "$AFTER" ]; then
  pass "退避前と完全一致（tree / rect / title / title_source）"
else
  fail "退避前と一致しない"
  diff -u "$TMP/before.json" "$TMP/after.json" | head -40
fi
check_eq "元の並び位置（先頭）へ戻る" "$TAB1" \
  "$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["id"])')"

echo "== ② GUI を落として起こし直す（layout.json 往復） =="
"$TAKO_BIN" background --tab "$TAB1" >/dev/null
sleep 4   # save_layout は 2 秒 tick
stop_app
check_eq "layout.json に shelved_tabs が書かれている" "1" \
  "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1])).get("shelved_tabs", [])))' "$TAKO_DATA_DIR/layout.json")"
start_app 2
check_eq "再起動後も退避タブが残る" "1" \
  "$("$TAKO_BIN" backgrounded | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
"$TAKO_BIN" foreground --tab "$TAB1" >/dev/null
AFTER2="$("$TAKO_BIN" list | python3 -c "$PY_SNAP" "$TAB1")"
echo "$AFTER2" > "$TMP/after2.json"
# 再起動後は器を作り直すので cwd は変わりうる。構造・タイトル・比率を比べる
if [ "$BEFORE" = "$AFTER2" ]; then
  pass "再起動をまたいでも退避前と完全一致"
else
  fail "再起動後に一致しない"
  diff -u "$TMP/before.json" "$TMP/after2.json" | head -40
fi

echo "== ③ 退避タブから 1 ペインだけ取り出す =="
"$TAKO_BIN" background --tab "$TAB1" >/dev/null
"$TAKO_BIN" foreground "$P3" >/dev/null
check_eq "退避タブは残り 2 ペイン" "2" \
  "$("$TAKO_BIN" backgrounded | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"][0]["panes"]))')"
check_eq "抜いたペインは画面に出ている" "1" \
  "$("$TAKO_BIN" list | python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tabs'] for p in t['panes'] if p['id']==$P3))")"
"$TAKO_BIN" foreground --tab "$TAB1" >/dev/null
check_eq "残り 2 ペインでタブごと戻せる" "2" \
  "$("$TAKO_BIN" list | python3 -c "import json,sys; print(len([t for t in json.load(sys.stdin)['tabs'] if t['id']==$TAB1][0]['panes']))")"

echo "== ④ 全部抜くと tabs から消える =="
"$TAKO_BIN" background --tab "$TAB1" >/dev/null
"$TAKO_BIN" foreground "$P1" >/dev/null
check_eq "1 本抜いても残る" "1" \
  "$("$TAKO_BIN" backgrounded | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
"$TAKO_BIN" foreground "$P2" >/dev/null
check_eq "全部抜くと tabs から消える" "0" \
  "$("$TAKO_BIN" backgrounded | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
check_eq "退避タブが消えても器は生きている" "0" \
  "$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["shelved_tabs"]))')"

echo "== ⑤ 旧 layout.json（shelved_tabs 無し）がそのまま読める =="
stop_app
python3 - "$TAKO_DATA_DIR/layout.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
# #1487 前の形へ落とす: shelved_tabs を消し、平坦な backgrounded を 1 件作る
d.pop("shelved_tabs", None)
leaf = None
def find(node):
    global leaf
    if node["type"] == "pane" and leaf is None:
        leaf = node
    elif node["type"] == "split":
        find(node["first"]); find(node["second"])
tab = d["tabs"][0]
find(tab["tree"])
old = dict(leaf)
old["id"] = 990001
old["session"] = None
old["origin_tab"] = tab["id"]
old["origin_tab_title"] = tab["title"]
d["backgrounded"] = [old]
json.dump(d, open(p, "w"), ensure_ascii=False)
print("旧形式へ書き戻した")
PY
start_app 3
check_eq "旧ファイルの平坦な退避がそのまま読める" "1" \
  "$("$TAKO_BIN" backgrounded | python3 -c 'import json,sys; print(len([p for p in json.load(sys.stdin)["backgrounded"] if not p.get("shelved_tab")]))')"
check_eq "旧ファイルでは退避タブは 0 件" "0" \
  "$("$TAKO_BIN" backgrounded | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
stop_app

echo "== ⑥ A/B: TAKO_1487_LEGACY=1 で旧挙動（平坦化）が再現する =="
rm -rf "$TAKO_DATA_DIR"; mkdir -p "$TAKO_DATA_DIR"
bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || true
TAKO_1487_LEGACY=1 "$APP_BIN" > "$TMP/app-legacy.log" 2>&1 &
APP_PID=$!
for _ in $(seq 1 200); do
  if "$TAKO_BIN" list >/dev/null 2>&1; then break; fi
  sleep 0.1
done
LTAB="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["id"])')"
LP1="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
"$TAKO_BIN" split --pane "$LP1" --right >/dev/null
"$TAKO_BIN" tab new >/dev/null
"$TAKO_BIN" background --tab "$LTAB" >/dev/null
LBG="$("$TAKO_BIN" backgrounded)"
check_eq "legacy: tabs は 0 件（= #1487 前の症状）" "0" \
  "$(printf '%s' "$LBG" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
check_eq "legacy: ペインが平坦化されて 2 件並ぶ" "2" \
  "$(printf '%s' "$LBG" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["backgrounded"]))')"
LERR="$("$TAKO_BIN" foreground --tab "$LTAB" 2>&1)"; LRC=$?
if [ "$LRC" != "0" ]; then
  pass "legacy: タブ単位の復帰は対象が無い（単体でしか戻せない = ユーザー原文の症状）"
else
  fail "legacy なのにタブ単位で戻せてしまった: $LERR"
fi
stop_app

echo
echo "== 結果: PASS=$PASS FAIL=$FAIL =="
[ "$FAIL" -eq 0 ]
