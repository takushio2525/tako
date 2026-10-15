#!/bin/bash
# test-lsp-sketch-headers-2113.sh — Arduino のスケッチ内の .h / .cpp を arduino-language-server（ALS）へ回す
# （Issue #2113）のを、この PC の Arduino IDE の同梱物（ALS・arduino-cli・clangd 14）と Command Line Tools の
# clangd で確かめる（隔離 GUI）
#
# 何を確かめるか（Issue #2113 の受け入れ条件のうち、実機で言えるもの）:
#   ⓪ GUI を除いた tako の実経路（manager + 既定の解決。e2e の
#      `実機_スケッチの_h_と_cpp_に嘘のエラーが出ずホバーと補完が出る`）を新（既定）と旧
#      （TAKO_2113_LEGACY=1 = 拡張子のまま clangd）の子プロセスで 1 回ずつ走らせ、同じ素材で比べる:
#      .ino を先に開く / .h だけ / src/ の .h だけ / 基板なし / スケッチでないフォルダ。面（tako-vd）が無くても測れる
#   ① GUI（新）: スケッチの .h だけを編集モードで開くと ALS の器（本体を tako が隠れて開く = hidden 1）・
#      嘘のエラー 0 件・ホバー。.cpp を足して補完と 0 件。スケッチでないフォルダの .h は clangd。基板の無い
#      スケッチの .h は起こさず unconfigured（帯の理由）
#   ② GUI（旧 2 通り）: TAKO_2113_LEGACY=1 と本番の /Applications/tako.app（main）で、同じ .h が clangd へ行き
#      嘘のエラーが出る
#   ③ visual-test lsp-servers: 新は .h に波線が出ずホバーのカード（clean）と基板なしの帯、旧は波線（diag）。
#      画像の置き場は新旧で分ける
#   ④ ユーザーのファイル（スケッチ）と本物の Arduino の data dir を書き換えていない（sha256 / mtime の前後）
#
# **ユーザーの Arduino の設定・作業フォルダには書かない**: HOME を一時 dir へ差し替え、arduino:avr のコアは
# 本物の data dir から APFS の複製（`cp -c`）で写す（#1951 と同じ作法）。
#
# 使い方: bash scripts/test-lsp-sketch-headers-2113.sh
#   TAKO_2113_DUMP_DIR=<dir>   画像の置き場（既定は一時 = 終わると消える。new/ と old/ に分けて落とす）
#   Arduino IDE（/Applications/Arduino IDE.app）か arduino:avr のコアが無い機では「未実測」と出す。
#   面（tako-vd）を用意できない機（蓋閉じ）では ①〜③ を「未実測」と出し、⓪ だけを測る
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。macOS の bash 3.2 で通す（#1499 / #1518）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_2113_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REAL_HOME="$HOME"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_has() { # check_has <名前> <探す語> <本文>
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'$2' が無い: $3）" ;;
  esac
}

ALS="arduino-language-server"
IDE_DIR="/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources"
REAL_ARDUINO15="${TAKO_2113_ARDUINO15:-$HOME/Library/Arduino15}"
stamp() { # 本物の Arduino の data dir の印（無ければ空）
  local f
  for f in inventory.yaml package_index.json library_index.json; do
    [ -f "$REAL_ARDUINO15/$f" ] && stat -f '%m %N' "$REAL_ARDUINO15/$f"
  done
  true
}
STAMP_BEFORE="$(stamp)"

# 受け口のソケットのパス長に余裕を持たせるため短い置き場にする
TMP="$(mktemp -d /tmp/tk2113.XXXXXX)"
# symlink を解いた形にそろえる（エディタと言語サーバは /private/tmp/… で文書を持つ）
TMP="$(cd "$TMP" && pwd -P)"
DUMP="${TAKO_2113_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-2113-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # 置き場を指定されていれば、一時 dir と一緒に消える前に各段のログを残す（落ちた段を後から読める）
  if [ -n "${TAKO_2113_DUMP_DIR:-}" ]; then
    mkdir -p "$DUMP/logs" && cp "$TMP"/*.log "$DUMP/logs/" 2>/dev/null
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app と tako をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
(cd "$REPO_ROOT" && run_heavy cargo test -q -p tako-control --test issue2113_lsp_sketch_headers --no-run) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
NEW_TAKO_BIN="$TAKO_BIN"
NEW_APP_BIN="$APP_BIN"

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

# --- 隔離の土台 -------------------------------------------------------------------------
mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/rbin" "$DUMP/new" "$DUMP/old" "$DUMP/main"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
printf 'export PATH="%s:$PATH"\n' "$TMP/rbin" > "$TMP/zdot/.zprofile"
# clangd（Command Line Tools）は /usr/bin にある = 最小の PATH で旧の行が引ける
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
found="$(command -v tmux 2>/dev/null)"
[ -n "$found" ] && ln -s "$found" "$TMP/rbin/tmux"

A15="$TMP/home/Library/Arduino15"
HAVE_CORE=0
if [ -d "$REAL_ARDUINO15/packages/arduino/hardware/avr" ]; then
  mkdir -p "$A15/packages/arduino/hardware" "$A15/packages/arduino/tools"
  copy() { cp -c -R "$1" "$2" 2>/dev/null || cp -R "$1" "$2"; }
  copy "$REAL_ARDUINO15/packages/arduino/hardware/avr" "$A15/packages/arduino/hardware/"
  for t in avr-gcc avrdude arduinoOTA; do
    [ -d "$REAL_ARDUINO15/packages/arduino/tools/$t" ] && copy "$REAL_ARDUINO15/packages/arduino/tools/$t" "$A15/packages/arduino/tools/"
  done
  [ -d "$REAL_ARDUINO15/packages/builtin" ] && copy "$REAL_ARDUINO15/packages/builtin" "$A15/packages/"
  [ -f "$REAL_ARDUINO15/package_index.json" ] && cp "$REAL_ARDUINO15/package_index.json" "$A15/"
  HAVE_CORE=1
fi
HAVE_IDE=0
[ -x "$IDE_DIR/$ALS" ] && [ -x "$IDE_DIR/arduino-cli" ] && [ -x "$IDE_DIR/clangd" ] && HAVE_IDE=1
HAVE_CLANGD=0
[ -x /usr/bin/clangd ] && /usr/bin/clangd --version >/dev/null 2>&1 && HAVE_CLANGD=1

export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

# --- 素材（どれも誤りの無いコード。根に .git = 旧の clangd のルートは repo の根）--------------
# sketches_into <置き場>: Blink（.ino を先に）/ Blink2（.h だけ）/ Blink3（src/ の .h だけ）/ NoBoard（基板なし）/
# NoBoardVt（帯の画像用の基板なし）/ plain（スケッチでない）
sketches_into() {
  local s="$1" name
  mkdir -p "$s/.git" "$s/Blink3/src" "$s/plain"
  for name in Blink Blink2 NoBoard NoBoardVt; do
    mkdir -p "$s/$name"
    cat > "$s/$name/$name.ino" <<'EOF'
#include "helper.h"

void report(int v);

void setup() {
  pinMode(LED_BUILTIN, OUTPUT);
  Serial.begin(9600);
}

void loop() {
  toggleLed(LED_BUILTIN);
  report(1);
  delay(500);
}
EOF
    cat > "$s/$name/helper.h" <<'EOF'
#pragma once
#include <Arduino.h>

inline void toggleLed(uint8_t pin) {
  digitalWrite(pin, !digitalRead(pin));
}
EOF
    cat > "$s/$name/helper.cpp" <<'EOF'
#include "helper.h"

void report(int v) {
  Serial.println(v);
}
EOF
  done
  printf 'default_fqbn: arduino:avr:uno\n' > "$s/Blink/sketch.yaml"
  printf 'default_fqbn: arduino:avr:uno\n' > "$s/Blink2/sketch.yaml"
  cat > "$s/Blink3/Blink3.ino" <<'EOF'
#include "src/util.h"

void setup() {
  Serial.begin(9600);
}

void loop() {
  Serial.println(half(10));
}
EOF
  cat > "$s/Blink3/src/util.h" <<'EOF'
#pragma once
#include <Arduino.h>

inline uint8_t half(uint8_t v) {
  return v / 2;
}
EOF
  printf 'default_fqbn: arduino:avr:uno\n' > "$s/Blink3/sketch.yaml"
  cat > "$s/plain/plain.h" <<'EOF'
#pragma once

inline int twice(int v) {
  return v * 2;
}
EOF
  cat > "$s/plain/plain.cpp" <<'EOF'
#include "plain.h"

int four() {
  return twice(2);
}
EOF
}
S="$TMP/sketches" # ⓪ の e2e
G="$TMP/gsk"      # ①〜③ の GUI
sketches_into "$S"
sketches_into "$G"
digest() { # 素材の .ino / .h / .cpp / sketch.yaml の sha256（ユーザーのファイルを書き換えないことの印）
  (cd "$1" && find . -type f \( -name '*.ino' -o -name '*.h' -o -name '*.cpp' -o -name 'sketch.yaml' \) -print0 |
    sort -z | xargs -0 shasum -a 256)
}
DIGEST_BEFORE="$(digest "$S")$(digest "$G")"

start_gui() { # start_gui <data> <ログ> [VAR=VAL…]
  local data="$1" log="$2"
  shift 2
  mkdir -p "$data"
  export TAKO_DATA_DIR="$data"
  launch_isolated_gui "$log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_DATA_DIR="$data" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || { echo "GUI が立たない"; exit 1; }
  ROOT="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  [ -n "$ROOT" ] || { echo "ペインが取れない"; exit 1; }
}
stop_gui() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
}
open_edit() { # ファイル → ペイン ID（編集モードへ入れる）
  local pane
  pane="$("$TAKO_BIN" open "$1" --pane "$ROOT" --right 2>/dev/null | json 'd["pane"]')"
  [ -n "$pane" ] || return 1
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  echo "$pane"
}
server_field() { # server_field <サーバ> <ルート> <式>（そのルートの器の欄。無ければ空の辞書から）
  "$TAKO_BIN" lsp status --name "$1" --json 2>/dev/null |
    python3 -c 'import json,sys
d = json.load(sys.stdin)
s = next((x for x in d.get("servers", []) if x.get("root") == sys.argv[1]), {})
print(eval("s" + sys.argv[2]))' "$2" "$3" 2>/dev/null
}
wait_state() { # wait_state <サーバ> <ルート> <状態> <秒>
  local n=0
  while [ "$n" -lt $(($4 * 10)) ]; do
    [ "$(server_field "$1" "$2" '.get("state")')" = "$3" ] && return 0
    sleep 0.1
    n=$((n + 1))
  done
  return 1
}
pane_diags() { # pane_diags <ペイン> → 「件数 エラー数 | 行: 文…」
  "$TAKO_BIN" lsp diagnostics --pane "$1" --json 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
items = [x for doc in d.get("documents", []) for x in doc.get("diagnostics", [])]
errs = [x for x in items if x.get("severity") == "error"]
msgs = ["%s: %s" % (x["range"]["start"]["line"], x["message"].splitlines()[0]) for x in items]
print(len(items), len(errs), "|", " / ".join(msgs[:4]))' 2>/dev/null
}
hover_found() { # hover_found <ペイン> <行> <桁> <秒> → 答えの本文の先頭（出なければ空）
  local n=0 hv
  while [ "$n" -lt "$4" ]; do
    hv="$("$TAKO_BIN" lsp hover --pane "$1" --line "$2" --column "$3" --json 2>/dev/null)"
    if [ "$(echo "$hv" | json 'd["status"]')" = "found" ]; then
      echo "$hv" | json 'd["contents"]' | head -3 | tr '\n' ' '
      return 0
    fi
    sleep 1
    n=$((n + 1))
  done
  return 1
}
# 旧の嘘のエラーが出揃うまで（最初の 1 件から 3 秒変わらない）待って「件数 エラー数 | …」
wait_diags() { # wait_diags <ペイン> <秒>
  local n=0 last="" now stable=0
  while [ "$n" -lt $(($2 * 2)) ]; do
    now="$(pane_diags "$1")"
    if [ -n "$now" ] && [ "${now%% *}" -gt 0 ] 2>/dev/null; then
      if [ "$now" = "$last" ]; then stable=$((stable + 1)); else stable=0; fi
      [ "$stable" -ge 6 ] && break
    fi
    last="$now"
    sleep 0.5
    n=$((n + 1))
  done
  pane_diags "$1"
}

echo "== 前提 =="
echo "  Arduino IDE の同梱: $([ "$HAVE_IDE" = 1 ] && echo "あり（$("$IDE_DIR/arduino-cli" version 2>/dev/null | head -1 | sed "s#$TMP#<tmp>#g")・$("$IDE_DIR/clangd" --version 2>/dev/null | head -1)）" || echo 無い)"
echo "  arduino:avr のコア: $([ "$HAVE_CORE" = 1 ] && echo 一時 HOME へ複製した || echo 無い)"
echo "  旧の行の clangd: $([ "$HAVE_CLANGD" = 1 ] && /usr/bin/clangd --version 2>/dev/null | head -1 || echo 無い)"

echo
echo "== ⓪ GUI を除いた tako の実経路で新旧を比べる（manager + 既定の解決。旧 = TAKO_2113_LEGACY=1） =="
if [ "$HAVE_IDE" = 1 ] && [ "$HAVE_CORE" = 1 ] && [ "$HAVE_CLANGD" = 1 ]; then
  # cargo と rustup は本物の HOME で動かし、テストの子プロセスだけを隔離 HOME で起こす
  (cd "$REPO_ROOT" && HOME="$REAL_HOME" TAKO_2113_REAL_HOME="$TMP/home" TAKO_2113_REAL_SKETCHES="$S" \
    PATH="$MIN_PATH:$PATH" run_heavy cargo test -q -p tako-control --test issue2113_lsp_sketch_headers \
    実機_スケッチの_h_と_cpp_に嘘のエラーが出ずホバーと補完が出る -- --exact --nocapture) > "$TMP/real.log" 2>&1
  real_rc=$?
  python3 - "$TMP/real.log" "$TMP" <<'PY'
import json, re, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read().replace(sys.argv[2], "<tmp>")
def block(tag):
    m = re.search(tag + r": (\{.*?\n\})", text, re.S)
    return json.loads(m.group(1)) if m else None
rows = []
for label, data in (("新", block("TAKO_2113_REAL_NEW")), ("旧", block("TAKO_2113_REAL_OLD"))):
    if not data:
        print("    %s: 結果が無い" % label)
        continue
    for flow in ("ino_first", "h_only", "src_only", "no_board", "plain"):
        got = data.get(flow)
        if got is None:
            continue
        if flow == "no_board":
            print("    %s %-9s state=%s spawn=%s errors=%s band=%s" % (label, flow, got.get("state"), got.get("spawn_count"), got.get("errors"), got.get("band")))
            continue
        for doc in ("ino", "h", "cpp", "util"):
            if doc in got:
                d = got[doc]
                print("    %s %-9s %-4s server=%-24s errors=%s diags=%s %s" % (label, flow, doc, d.get("server"), d.get("errors"), d.get("diagnostics"), " / ".join(d.get("messages", [])[:2])))
        extra = []
        for k in ("published_ms", "hidden_documents", "hover_h", "completion_has_println", "errors_after_queries"):
            if k in got:
                extra.append("%s=%s" % (k, got[k]))
        print("    %s %-9s      %s" % (label, flow, " ".join(extra)))
PY
  if [ "$real_rc" -eq 0 ] && grep -q "TAKO_2113_REAL_NEW:" "$TMP/real.log"; then
    pass "実の同梱物: 新は .ino 先 / .h だけ / src/ の .h とも ALS で嘘のエラー 0・ホバーと補完・基板なしは unconfigured・旧は clangd で嘘のエラー・スケッチでないフォルダは新旧同じ"
  else
    fail "実の同梱物の実経路（$(grep -m1 -E 'panicked|assert' "$TMP/real.log" | sed "s#$TMP#<tmp>#g")）"
    grep -A8 -m1 "panicked" "$TMP/real.log" | sed "s#$TMP#<tmp>#g; s/^/      /"
  fi
else
  unmeasured "⓪: Arduino IDE の同梱・arduino:avr のコア・/usr/bin/clangd のどれかが無い"
fi

GUI_OK=1
# 面（tako-vd）の有無は子シェルで 1 回立てて止めて分かる（用意できなければ start_gui の中の
# launch_isolated_gui が 4 = 未実測で抜ける。#1744。親で立てると exit でスクリプトごと終わる）
( start_gui "$TMP/data-probe" "$TMP/gui-probe.log" && stop_gui )
gui_rc=$?
if [ "$gui_rc" -eq 4 ]; then
  GUI_OK=0
  unmeasured "①〜③ の GUI の段: 面（tako-vd）を用意できない（蓋閉じなら開いてから打ち直す）"
elif [ "$gui_rc" -ne 0 ]; then
  exit "$gui_rc"
fi

if [ "$GUI_OK" = 1 ] && [ "$HAVE_IDE" = 1 ] && [ "$HAVE_CORE" = 1 ]; then
  echo
  echo "== ① GUI（新）: スケッチの .h だけを開く → ALS の器・嘘のエラー 0 件・ホバー / 補完 =="
  start_gui "$TMP/data-new" "$TMP/gui-new.log"
  pane_h="$(open_edit "$G/Blink2/helper.h")" || { fail "開けない"; exit 1; }
  if wait_state "$ALS" "$G/Blink2" running 90; then
    pass "スケッチの .h が ALS の器で稼働（ルート = スケッチのフォルダ）"
  else
    fail "ALS の器が稼働しない（$(server_field "$ALS" "$G/Blink2" '.get("state")') / $(server_field "$ALS" "$G/Blink2" '.get("reason")')）"
  fi
  check_eq "本体の .ino を tako が隠れて開いている（documents 1・hidden_documents 1）" "(1, 1)" \
    "$(server_field "$ALS" "$G/Blink2" '.get("documents"), s.get("hidden_documents")')"
  hv="$(hover_found "$pane_h" 5 4 90)"
  check_has "ホバー（5 行目の digitalWrite）" "digitalWrite" "$hv"
  # 答えが出た（サーバが文書を読んだ）後、診断が届く時間を待ってから数える
  sleep 8
  d_h="$(pane_diags "$pane_h")"
  echo "    .h の診断: $d_h"
  check_eq ".h の嘘のエラーが 0 件" "0" "$(echo "$d_h" | awk '{print $2}')"
  pane_c="$(open_edit "$G/Blink2/helper.cpp")" || { fail "開けない"; exit 1; }
  cp="$("$TAKO_BIN" lsp completion --pane "$pane_c" --line 4 --column 9 --json 2>/dev/null)"
  for _ in $(seq 1 20); do
    [ "$(echo "$cp" | json 'any(i["label"].strip().startswith("println") for i in d["items"])')" = "True" ] && break
    sleep 1
    cp="$("$TAKO_BIN" lsp completion --pane "$pane_c" --line 4 --column 9 --json 2>/dev/null)"
  done
  echo "    補完（.cpp の 4 行目 Serial. の後）: $(echo "$cp" | json '[i["label"].strip() for i in d["items"]][:6]')"
  check_eq "補完に println が出る" "True" "$(echo "$cp" | json 'any(i["label"].strip().startswith("println") for i in d["items"])')"
  sleep 8
  d_c="$(pane_diags "$pane_c")"
  echo "    .cpp の診断: $d_c"
  check_eq ".cpp の嘘のエラーが 0 件" "0" "$(echo "$d_c" | awk '{print $2}')"
  check_eq "器は 1 つのまま（.h と .cpp は同じ器・本体は隠れたまま）" "(2, 1)" \
    "$(server_field "$ALS" "$G/Blink2" '.get("documents"), s.get("hidden_documents")')"

  echo "  -- スケッチでないフォルダの .h は今どおり clangd"
  pane_p="$(open_edit "$G/plain/plain.h")" || { fail "開けない"; exit 1; }
  if wait_state clangd "$G" running 60; then
    pass "スケッチでないフォルダの .h は clangd（ルートは repo の根）"
  else
    fail "clangd の器に届かない（$("$TAKO_BIN" lsp status --json 2>/dev/null | json '[(s["id"], s["root"], s["state"]) for s in d["servers"]]' | sed "s#$TMP#<tmp>#g")）"
  fi
  check_eq "スケッチでないフォルダに ALS の器は立たない" "None" "$(server_field "$ALS" "$G/plain" '.get("state")')"

  echo "  -- 基板の無いスケッチの .h は起こさず unconfigured（色だけ）"
  pane_n="$(open_edit "$G/NoBoard/helper.h")" || { fail "開けない"; exit 1; }
  if wait_state "$ALS" "$G/NoBoard" unconfigured 30; then
    pass "基板なしの .h は前提が未設定（unconfigured）で止まる"
  else
    fail "unconfigured に届かない（$(server_field "$ALS" "$G/NoBoard" '.get("state")')）"
  fi
  check_eq "サーバは起こしていない（pid なし・起動 0 回）" "(None, 0)" \
    "$(server_field "$ALS" "$G/NoBoard" '.get("pid"), s.get("spawn_count")')"
  echo "    理由: $(server_field "$ALS" "$G/NoBoard" '["reason"]' | sed "s#$TMP#<tmp>#g")"
  sleep 2
  check_eq "基板なしの .h に波線は出ない" "0" "$(pane_diags "$pane_n" | awk '{print $1}')"
  stop_gui

  if [ "$HAVE_CLANGD" = 1 ]; then
    echo
    echo "== ② GUI（旧 = TAKO_2113_LEGACY=1）: 同じ .h が clangd へ行き嘘のエラーが出る =="
    start_gui "$TMP/data-old" "$TMP/gui-old.log" TAKO_2113_LEGACY=1
    pane_h="$(open_edit "$G/Blink2/helper.h")" || { fail "開けない"; exit 1; }
    wait_state clangd "$G" running 60 || true
    d_old="$(wait_diags "$pane_h" 60)"
    echo "    .h の診断（旧）: $d_old"
    check_eq "旧: .h は clangd の器" "running" "$(server_field clangd "$G" '.get("state")')"
    check_eq "旧: ALS の器は立たない" "None" "$(server_field "$ALS" "$G/Blink2" '.get("state")')"
    if [ "$(echo "$d_old" | awk '{print $2}')" -gt 0 ] 2>/dev/null; then
      pass "旧: 嘘のエラーが出る（$(echo "$d_old" | awk '{print $2}') 件）"
    else
      fail "旧: 嘘のエラーが出ない（$d_old）"
    fi
    stop_gui

    PROD_APP="/Applications/tako.app/Contents/MacOS/tako-app"
    PROD_CLI="/Applications/tako.app/Contents/MacOS/tako"
    echo
    echo "== ② の 2（旧 = 本番の /Applications/tako.app = main の #2113 より前）: 同じ .h に嘘のエラー =="
    if [ -x "$PROD_APP" ] && [ -x "$PROD_CLI" ]; then
      TAKO_BIN="$PROD_CLI" APP_BIN="$PROD_APP"
      start_gui "$TMP/data-main" "$TMP/gui-main.log"
      pane_h="$(open_edit "$G/Blink2/helper.h")" || { fail "開けない"; exit 1; }
      d_main="$(wait_diags "$pane_h" 60)"
      echo "    .h の診断（main）: $d_main"
      check_eq "main: .h は clangd の器" "running" "$(server_field clangd "$G" '.get("state")')"
      if [ "$(echo "$d_main" | awk '{print $2}')" -gt 0 ] 2>/dev/null; then
        pass "main: 嘘のエラーが出る（$(echo "$d_main" | awk '{print $2}') 件）"
      else
        fail "main: 嘘のエラーが出ない（$d_main）"
      fi
      stop_gui
      TAKO_BIN="$NEW_TAKO_BIN" APP_BIN="$NEW_APP_BIN"
    else
      unmeasured "② の 2: /Applications/tako.app が無い"
    fi
  else
    unmeasured "②: /usr/bin/clangd が無い（旧の行を起こせない）"
  fi

  echo
  echo "== ③ visual-test lsp-servers（新 = 波線なし + カード / 基板なしの帯、旧 = 波線） =="
  vt() { # vt <名前> <画像の置き場> <件> [VAR=VAL…]
    local name="$1" dir="$2" cases="$3" vpid
    shift 3
    launch_isolated_gui "$TMP/vt-$name.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
      TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DATA_DIR="$TMP/data-vt-$name" \
      TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
      TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
      TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=lsp-servers TAKO_VISUAL_DUMP_DIR="$dir" \
      TAKO_1956_CASES="$cases" ${1+"$@"} || exit $?
    vpid="$ISOLATED_GUI_PID"
    for _ in $(seq 1 6000); do
      kill -0 "$vpid" 2>/dev/null || break
      sleep 0.1
    done
    stop_isolated_gui "$vpid"
    ISOLATED_GUI_PID=""
    grep "TAKO_VISUAL_PIXEL: lsp-servers id=\|TAKO_VISUAL_DUMP_FILE" "$TMP/vt-$name.log" | sed "s#$TMP#<tmp>#g; s/^/    /"
  }
  TAB="$(printf '\t')"
  vt new "$DUMP/new" "$G/Blink2/helper.h${TAB}4${TAB}4${TAB}clean
$G/NoBoardVt/helper.h${TAB}4${TAB}4${TAB}unconfigured"
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-new.log"; then
    pass "visual-test（新）: スケッチの .h に波線が出ずホバーのカード・基板なしの .h で帯（TAKO_VISUAL_TEST_OK）"
  else
    fail "visual-test（新）: $(grep -h 'FAILED' "$TMP/vt-new.log" | head -1)"
    tail -20 "$TMP/vt-new.log" | sed "s#$TMP#<tmp>#g; s/^/      /"
  fi
  if [ "$HAVE_CLANGD" = 1 ]; then
    vt old "$DUMP/old" "$G/Blink2/helper.h${TAB}4${TAB}4${TAB}diag" TAKO_2113_LEGACY=1
    if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt-old.log"; then
      pass "visual-test（旧）: 同じ .h に嘘のエラーの波線（TAKO_VISUAL_TEST_OK）"
    else
      fail "visual-test（旧）: $(grep -h 'FAILED' "$TMP/vt-old.log" | head -1)"
      tail -20 "$TMP/vt-old.log" | sed "s#$TMP#<tmp>#g; s/^/      /"
    fi
  fi
elif [ "$GUI_OK" = 1 ]; then
  unmeasured "①〜③: Arduino IDE の同梱か arduino:avr のコアが無い"
fi

echo
echo "== ④ ユーザーのファイルと本物の Arduino の data dir に触れていない =="
check_eq "スケッチの .ino / .h / .cpp / sketch.yaml の sha256 が前後で同じ" "$DIGEST_BEFORE" "$(digest "$S")$(digest "$G")"
check_eq "inventory.yaml / package_index.json / library_index.json の mtime が前後で同じ" "$STAMP_BEFORE" "$(stamp)"

echo
echo "結果: OK $PASS / NG $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ]
