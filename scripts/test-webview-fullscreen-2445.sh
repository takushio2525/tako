#!/bin/bash
# test-webview-fullscreen-2445.sh — wry 0.57 へ上げた後の Web ビュー（Issue #2445）の実経路テスト
# （macOS の隔離 GUI）
#
# 何を確かめるか:
#   wry 0.56 から macOS の WKWebView は要素の全画面表示（Fullscreen API）が常に入る。全画面のあいだ
#   WebKit は WKWebView を gpui の描画ビューから外して全画面の窓へ移すので、tako の前提
#   「WKWebView の superview は gpui の描画ビュー」（#1481 / #1837）が崩れる。tako は生成の直後に
#   0.55 と同じく切る（`webview.rs` の `element_fullscreen_enabled`）。あわせて 0.57.1 では
#   不可視・focus なしの生成でアプリを前面へ出さなくなった（wry#1866）。
#
# 腕:
#   off（既定）  tako の既定 = 全画面を切る:
#     ① ページから見て fullscreenEnabled が偽
#     ② 全画面ボタン（requestFullscreen を呼ぶボタン）を押しても断られ、窓が増えない
#     ③ その後も Web ビューは窓の子で見えている・位置合わせの回数が増え続けない
#     ④ タブを切り替えると隠れ、戻すと見える
#     ⑤ 2 枚開いて片方で全画面ボタンを押しても、両方とも窓の子のまま
#     ⑥ 全画面ボタンを押した直後に web close しても落ちない・窓が残らない
#     ⑦ 別のアプリが前面のまま web open しても tako が前面に出ない（画面ロック中は未実測）
#     ⑧ panic.log が無い
#   on           TAKO_2445_FULLSCREEN=1 = wry の既定（全画面を入れる）。手当て前の挙動の観察用:
#     全画面ボタンで全画面の窓が増えること、exitFullscreen で戻ること、戻った後のページの寸法を記録する
#     （FAIL にはしない = 別 Issue の材料）
#
# **全画面の出入りとタブ・位置は描画が要る**: 蓋閉じ・画面ロック中は隔離 GUI の描画が止まりうる
# （body_renders 0 の実測がある）ので、ロック中に ③④ が落ちたら「未実測」へ回して数えない
# （通ったものは描画が回っていた証拠なので数える）。⑦ はロック中は前面が切り替わらないので常に未実測。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-webview-fullscreen-2445.sh [off|on]
#
# **CI には載せない**（実 GUI + 仮想ディスプレイ + WKWebView が要る）。構造（生成の直後に判断を当てる・
# 不可視 / focus なしで作る）は単体テスト `issue2445_*` が CI で見る。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARM="${1:-off}"
case "$ARM" in
  off | on) : ;;
  *) echo "使い方: $0 [off|on]"; exit 2 ;;
esac
PASS=0
FAIL=0
UNMEASURED=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
note() { echo "  [記録] $1"; }
# 描画が要る項目の判定: $1 = ok / ng、$2 = 通ったときの文、$3 = 落ちたときの文。
# 画面ロック中に落ちたものは描画が止まっていた可能性があるので数えずに「未実測」へ回す
check() {
  if [ "$1" = ok ]; then
    pass "$2"
  elif [ "$LOCKED" = 1 ]; then
    unmeasured "$3（画面ロック中は描画が止まりうる）"
  else
    fail "$3"
  fi
}

TMP="$(mktemp -d /tmp/tako-2445-XXXXXX)"
APP_PID=""
APP_LOG=""
TMUX_SOCKET="tako-2445-$$"
cleanup() {
  if [ "$FAIL" -gt 0 ] && [ -f "${APP_LOG:-}" ]; then
    echo "--- app.log の末尾"
    tail -20 "$APP_LOG"
  fi
  stop_isolated_gui "$APP_PID"
  tmux_socket_kill "$TMUX_SOCKET"
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
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

# 画面ロック中か（ロック中は描画が止まりうる・前面が切り替わらない。③④ の落ちと ⑦ を数えない）。
# pipefail の下なので早く抜ける読み手（grep -q）へパイプしない（#1633 / #2272）
screen_locked() {
  local out
  out="$(ioreg -n Root -d1 -a 2>/dev/null)"
  [[ "$(printf '%s\n' "$out" | sed -n '/CGSSessionScreenIsLocked/{n;p;}')" == *"<true/>"* ]]
}

# pid の画面上の窓の数（CGWindowList。layer 0 = 通常の窓）
WINLIST_JS="$TMP/winlist.js"
cat > "$WINLIST_JS" <<'EOF'
ObjC.import('CoreGraphics');
ObjC.import('Foundation');
function run(argv) {
  const pid = parseInt(argv[0], 10);
  const arr = ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, $.kCGNullWindowID));
  const out = [];
  for (let i = 0; i < arr.count; i++) {
    const d = arr.objectAtIndex(i);
    if (ObjC.unwrap(d.objectForKey('kCGWindowOwnerPID')) !== pid) continue;
    if (ObjC.unwrap(d.objectForKey('kCGWindowLayer')) !== 0) continue;
    const b = ObjC.deepUnwrap(d.objectForKey('kCGWindowBounds'));
    out.push(`${b.X},${b.Y},${b.Width}x${b.Height}`);
  }
  return out.length + ' ' + out.join(' ');
}
EOF
app_windows() { osascript -l JavaScript "$WINLIST_JS" "$APP_PID" 2>/dev/null || echo "? 読めない"; }
# 前面のアプリの pid
front_pid() {
  local asn
  asn="$(lsappinfo front 2>/dev/null)"
  lsappinfo info -only pid "$asn" 2>/dev/null | sed -n 's/.*"pid"=\([0-9]*\).*/\1/p'
}

# JSON（標準入力）のトップの項目（無ければ __NONE__・JSON でなければ __NOT_JSON__）
json_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
v = d.get(sys.argv[1], "__NONE__") if isinstance(d, dict) else "__NONE__"
print("null" if v is None else (v if isinstance(v, str) else json.dumps(v)))' "$1"
}
# `tako web list` の 1 件の項目（id で引く。無ければ __NONE__）
web_field() {
  "$TAKO_BIN" web list 2>/dev/null | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for e in d:
    if str(e.get("id")) == sys.argv[1]:
        v = e.get(sys.argv[2], "__NONE__")
        print("null" if v is None else (v if isinstance(v, str) else json.dumps(v)))
        sys.exit(0)
print("__NONE__")' "$1" "$2"
}
alive() { kill -0 "$APP_PID" 2>/dev/null; }
# 条件が真になるまで 0.2 秒刻みで待つ（上限 $2 秒）。真になったら 0
wait_until() {
  local cond="$1" limit="${2:-10}" i
  for i in $(seq 1 $((limit * 5))); do
    if eval "$cond"; then return 0; fi
    sleep 0.2
  done
  return 1
}
# JS を評価して結果（JSON の result）を EVAL_RESULT へ入れる。返らなければ __TIMEOUT__
eval_js() {
  local id="$1" js="$2" out token i
  out="$("$TAKO_BIN" web eval "$js" --id "$id" 2>&1)"
  token="$(printf '%s' "$out" | json_field token)"
  EVAL_RESULT="__TIMEOUT__"
  case "$token" in '' | __*) EVAL_RESULT="__NO_TOKEN__:${out:0:200}"; return 1 ;; esac
  for i in $(seq 1 50); do
    out="$("$TAKO_BIN" web eval-result "$token" --id "$id" 2>&1)"
    case "$(printf '%s' "$out" | json_field pending)" in
      true) sleep 0.2; continue ;;
    esac
    EVAL_RESULT="$(printf '%s' "$out" | json_field result)"
    case "$EVAL_RESULT" in __NONE__) sleep 0.2; continue ;; esac
    return 0
  done
  return 1
}
# Web ビューを開いて id と pane を OPEN_ID / OPEN_PANE に入れる
open_web() {
  local out
  out="$("$TAKO_BIN" web open "$1" 2>&1)"
  OPEN_ID="$(printf '%s' "$out" | json_field id)"
  OPEN_PANE="$(printf '%s' "$out" | json_field pane)"
  OPEN_RAW="$out"
}
launch() {
  APP_LOG="$TMP/app-$1.log"
  shift
  launch_isolated_gui "$APP_LOG" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  if ! wait_isolated_gui "$APP_LOG"; then
    echo "GUI が立たない"; exit 1
  fi
}

# 全画面ボタンのあるページ（ボタンは requestFullscreen を呼び、結果をタイトルに書く。
# 出入りは fullscreenchange でタイトルへ。eval の click() はユーザー操作扱いなので本物のボタンと同じ）。
# tako は空白入りの URL を断るので、HTML は URL エンコードして data: に載せる
page() {
  local html="<title>$1</title><body style='margin:0;background:#264'>\
<button id=fs style='width:200px;height:80px' onclick=\"var e=document.documentElement;\
var f=e.requestFullscreen||e.webkitRequestFullscreen;if(!f){document.title='$1-rejected:none';return}\
try{var p=f.call(e);if(p&&p.then){p.then(function(){document.title='$1-entered'},\
function(x){document.title='$1-rejected:'+x.name+':'+x.message})}}catch(x){document.title='$1-rejected:'+x.name+':'+x.message}\">fs</button>\
<script>document.addEventListener('fullscreenchange',function(){document.title=document.fullscreenElement?'$1-entered':'$1-exited'});\
document.addEventListener('webkitfullscreenchange',function(){document.title=document.webkitFullscreenElement?'$1-entered':'$1-exited'})</script>"
  printf 'data:text/html,%s' "$(printf '%s' "$html" | python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.stdin.read(),safe=""),end="")')"
}
FS_ENABLED_JS='document.fullscreenEnabled === true || document.webkitFullscreenEnabled === true'
CLICK_JS="document.getElementById('fs').click(); 1"

LOCKED=0
if screen_locked; then
  LOCKED=1
  echo "（画面ロック中: 描画と前面の切り替えが要る項目は未実測として数えない）"
fi

if [ "$ARM" = off ]; then
  echo "== 起動（腕: off = tako の既定・全画面を切る） =="
  launch off
else
  echo "== 起動（腕: on = TAKO_2445_FULLSCREEN=1・wry の既定） =="
  launch on TAKO_2445_FULLSCREEN=1
fi
sleep 1
WIN_BASE="$(app_windows)"
note "起動直後の窓: ${WIN_BASE}"

open_web "$(page t2445a)"
ID_A="$OPEN_ID"
case "$ID_A" in
  '' | __*) fail "Web ビューを開ける（応答: ${OPEN_RAW:0:300}）"; exit 1 ;;
esac
wait_until '[ "$(web_field "$ID_A" title)" = t2445a ]' 15 || fail "タイトルが出ない（$(web_field "$ID_A" title)）"

echo "== ① ページから見た fullscreenEnabled =="
eval_js "$ID_A" "$FS_ENABLED_JS"
if [ "$ARM" = off ]; then
  if [ "$EVAL_RESULT" = false ]; then
    pass "fullscreenEnabled が偽（${EVAL_RESULT}）"
  else
    fail "fullscreenEnabled が偽でない（${EVAL_RESULT}）"
  fi
else
  note "fullscreenEnabled=${EVAL_RESULT}（wry の既定なら true）"
fi

echo "== ② 全画面ボタンを押す =="
VIS_BEFORE="$(web_field "$ID_A" visible)"
SETS_BEFORE="$(web_field "$ID_A" bounds_sets)"
eval_js "$ID_A" "window.innerWidth + 'x' + window.innerHeight"
SIZE_BEFORE="$EVAL_RESULT"
eval_js "$ID_A" "$CLICK_JS"
if [ "$ARM" = off ]; then
  if wait_until '[ "$(web_field "$ID_A" title)" != t2445a ]' 10; then
    TITLE="$(web_field "$ID_A" title)"
    case "$TITLE" in
      t2445a-rejected:*) pass "全画面ボタンは断られる（${TITLE}）" ;;
      *) fail "全画面ボタンが断られない（${TITLE}）" ;;
    esac
  else
    fail "全画面ボタンの結果がタイトルに出ない（$(web_field "$ID_A" title)）"
  fi
  sleep 2
  WIN_AFTER="$(app_windows)"
  if [ "${WIN_AFTER%% *}" = "${WIN_BASE%% *}" ]; then
    pass "窓が増えない（前 ${WIN_BASE} / 後 ${WIN_AFTER}）"
  else
    fail "窓の数が変わった（前 ${WIN_BASE} / 後 ${WIN_AFTER}）"
  fi
  if alive; then pass "全画面ボタンでプロセスが落ちない"; else fail "全画面ボタンで落ちた"; exit 1; fi
else
  if wait_until '[ "$(web_field "$ID_A" title)" != t2445a ]' 10; then
    note "全画面ボタンの結果: $(web_field "$ID_A" title)"
  fi
  sleep 3
  note "全画面中の窓: $(app_windows)"
  eval_js "$ID_A" "window.innerWidth + 'x' + window.innerHeight"
  note "全画面中のページの寸法: ${EVAL_RESULT}（前 ${SIZE_BEFORE}）"
  note "全画面中の web list: visible=$(web_field "$ID_A" visible) attached=$(web_field "$ID_A" attached) bounds_sets=$(web_field "$ID_A" bounds_sets)"
  eval_js "$ID_A" "(document.exitFullscreen||document.webkitExitFullscreen).call(document); 1"
  wait_until '[ "$(web_field "$ID_A" title)" = t2445a-exited ]' 10
  sleep 2
  note "戻した後のタイトル: $(web_field "$ID_A" title)・窓: $(app_windows)"
  eval_js "$ID_A" "window.innerWidth + 'x' + window.innerHeight"
  note "戻した後のページの寸法: ${EVAL_RESULT}（前 ${SIZE_BEFORE}）"
fi

echo "== ③ 押した後も窓の子で見えている =="
if [ "$(web_field "$ID_A" attached)" = true ]; then
  pass "いま開いている窓の子のまま（attached=true）"
else
  fail "窓の子でない（attached=$(web_field "$ID_A" attached)）"
fi
V="$(web_field "$ID_A" visible)"
if [ "$V" = true ]; then R=ok; else R=ng; fi
check "$R" "見えている（visible=true）" "見えていない（visible=${V}・押す前 ${VIS_BEFORE}）"
eval_js "$ID_A" "window.innerWidth + 'x' + window.innerHeight"
if [ "$EVAL_RESULT" = "$SIZE_BEFORE" ]; then R=ok; else R=ng; fi
check "$R" "ページの寸法が変わらない（${EVAL_RESULT}）" "ページの寸法が変わった（前 ${SIZE_BEFORE} / 後 ${EVAL_RESULT}）"
SETS_A="$(web_field "$ID_A" bounds_sets)"
sleep 2
SETS_B="$(web_field "$ID_A" bounds_sets)"
if [ "$SETS_A" = "$SETS_B" ]; then R=ok; else R=ng; fi
check "$R" "位置合わせが落ち着いている（bounds_sets ${SETS_BEFORE} → ${SETS_B}）" "位置合わせが増え続ける（${SETS_A} → ${SETS_B}）"

echo "== ④ タブの切り替え =="
TAB_HOME="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d=json.load(sys.stdin)
tabs=d.get("tabs",[]) if isinstance(d,dict) else d
print(next((t["id"] for t in tabs if t.get("active")), ""))' 2>/dev/null)"
"$TAKO_BIN" tab new --focus >/dev/null 2>&1
if wait_until '[ "$(web_field "$ID_A" visible)" = false ]' 5; then R=ok; else R=ng; fi
check "$R" "別のタブへ移ると隠れる" "別のタブへ移っても隠れない（visible=$(web_field "$ID_A" visible)）"
if [ -n "$TAB_HOME" ]; then
  "$TAKO_BIN" tab select "$TAB_HOME" >/dev/null 2>&1
  if wait_until '[ "$(web_field "$ID_A" visible)" = true ]' 5; then R=ok; else R=ng; fi
  check "$R" "元のタブへ戻すと見える（visible_flips=$(web_field "$ID_A" visible_flips)）" \
    "元のタブへ戻しても見えない（visible=$(web_field "$ID_A" visible)）"
  eval_js "$ID_A" "window.innerWidth + 'x' + window.innerHeight"
  if [ "$EVAL_RESULT" = "$SIZE_BEFORE" ]; then R=ok; else R=ng; fi
  check "$R" "戻した後もページの寸法が同じ（${EVAL_RESULT}）" "戻した後のページの寸法が違う（前 ${SIZE_BEFORE} / 後 ${EVAL_RESULT}）"
else
  fail "元のタブの id が読めない"
fi

echo "== ⑤ 2 枚開いて片方で全画面ボタン =="
open_web "$(page t2445b)"
ID_B="$OPEN_ID"
wait_until '[ "$(web_field "$ID_B" title)" = t2445b ]' 15
eval_js "$ID_B" "$CLICK_JS"
wait_until '[ "$(web_field "$ID_B" title)" != t2445b ]' 10
if [ "$ARM" = on ]; then
  sleep 3
  note "2 枚目の全画面中: 窓 $(app_windows)・A attached=$(web_field "$ID_A" attached) B attached=$(web_field "$ID_B" attached)"
  eval_js "$ID_B" "(document.exitFullscreen||document.webkitExitFullscreen).call(document); 1"
  wait_until '[ "$(web_field "$ID_B" title)" = t2445b-exited ]' 10
  sleep 2
fi
if [ "$(web_field "$ID_A" attached)" = true ] && [ "$(web_field "$ID_B" attached)" = true ]; then
  pass "両方とも窓の子のまま（B のタイトル: $(web_field "$ID_B" title)）"
else
  fail "窓の子でないものがある（A=$(web_field "$ID_A" attached) B=$(web_field "$ID_B" attached)）"
fi

echo "== ⑥ 全画面ボタンを押した直後に web close =="
eval_js "$ID_B" "$CLICK_JS"
"$TAKO_BIN" web close --id "$ID_B" >/dev/null 2>&1
if wait_until '[ "$(web_field "$ID_B" state)" = __NONE__ ]' 10; then
  pass "一覧から消える"
else
  fail "web close しても残る（$(web_field "$ID_B" state)）"
fi
sleep 3
if alive; then pass "落ちない"; else fail "落ちた"; exit 1; fi
WIN_CLOSE="$(app_windows)"
if [ "${WIN_CLOSE%% *}" = "${WIN_BASE%% *}" ]; then
  pass "窓が残らない（${WIN_CLOSE}）"
else
  if [ "$ARM" = on ]; then
    note "全画面のまま閉じた後の窓: ${WIN_CLOSE}（起動直後 ${WIN_BASE}）"
  else
    fail "窓の数が起動直後と違う（${WIN_CLOSE} / 起動直後 ${WIN_BASE}）"
  fi
fi

echo "== ⑦ 別のアプリが前面のまま web open =="
FRONT_BEFORE="$(front_pid)"
if [ "$LOCKED" = 1 ] || [ -z "$FRONT_BEFORE" ]; then
  unmeasured "前面のアプリ（画面ロック中は前面が切り替わらない。前面 pid=${FRONT_BEFORE:-?}）"
elif [ "$FRONT_BEFORE" = "$APP_PID" ]; then
  unmeasured "隔離 GUI が既に前面（別のアプリを前面にしてから測る）"
else
  open_web "$(page t2445c)"
  ID_C="$OPEN_ID"
  sleep 2
  FRONT_AFTER="$(front_pid)"
  if [ "$FRONT_AFTER" = "$APP_PID" ]; then
    fail "web open で tako が前面へ出た（前 ${FRONT_BEFORE} → 後 ${FRONT_AFTER}）"
  else
    pass "web open で tako が前面へ出ない（前 ${FRONT_BEFORE} → 後 ${FRONT_AFTER}）"
  fi
  "$TAKO_BIN" web close --id "$ID_C" >/dev/null 2>&1
fi

echo "== ⑧ panic.log =="
if [ -f "$TAKO_DATA_DIR/panic.log" ]; then
  fail "panic.log が残った: $(grep -m1 "panic at" "$TAKO_DATA_DIR/panic.log" | cut -c1-200)"
else
  pass "panic.log が無い"
fi

echo ""
echo "結果（腕: ${ARM}）: PASS=${PASS} FAIL=${FAIL} 未実測=${UNMEASURED}"
[ "$FAIL" -eq 0 ]
