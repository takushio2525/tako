#!/bin/bash
# test-lsp-arduino-1951.sh — Arduino のスケッチ（.ino）の言語サーバ（Issue #1951）を、この PC の
# Arduino IDE の同梱物（arduino-language-server・arduino-cli・clangd）で確かめる（隔離 GUI）
#
# 何を確かめるか（Issue #1951 の受け入れ条件のうち、実機で言えるもの）:
#   ⓪ GUI を除いた tako の実経路（manager + 既定の解決）で実の同梱物を起こす（e2e の
#      `実機_arduino_ide_の同梱物で診断とホバーと補完が出る`）: 出どころ bundled・誤り 1 件を 12 行目・
#      ホバー・補完・常駐・基板なし → 実の arduino-cli で選ぶ → 合図で起きる。面（tako-vd）が無くても測れる
#   ⓪' GUI 無しの CLI の A/B: `tako lsp install --name arduino-language-server` が本番の旧版では
#      「知らないサーバ」・新版では検出表の行（取らない行）として答える
#   ① 基板あり（sketch.yaml の default_fqbn）: 編集モードで開くだけで 起動 → 誤りの診断（.ino の行番号）→
#      ホバー → 補完。出どころは Arduino IDE の同梱（bundled）。常駐（ALS + clangd の RSS の合計）を測る
#   ② visual-test lsp-servers（#1956 の汎用の口に同じ .ino を渡す）: 波線の画面とホバーのカードを落とす
#   ③ 基板なし: 起こさず unconfigured・理由と選ぶコマンド（setup_command）。tako のペインでそのコマンドを
#      打つと（シェル統合の合図）起きる。合図が来ない形なら「もう一度確かめる」と同じ restart で起きる
#   ④ ALS なし（注入 TAKO_1951_INJECT_NO_BUNDLE=1 = Arduino IDE の無い PC と同じ解決）: 未導入へ落ち、
#      理由（同梱も探した）と導入コマンド（brew install --cask arduino-ide）が読め、編集は続けられる
#   ⑤ A/B: 本番の /Applications/tako.app（#1951 より前）では .ino に言語サーバが付かない
#
# **ユーザーの Arduino の設定・作業フォルダには書かない**: HOME を一時 dir へ差し替え（arduino-cli の既定の
# data dir `~/Library/Arduino15`・作業フォルダ `~/Documents/Arduino` も一時 dir になる）、arduino:avr の
# コアは本物の data dir から APFS の複製（`cp -c`）で写す。本物の inventory.yaml / package_index.json /
# library_index.json の mtime を前後で比べる（#1950 の作法）。
#
# 使い方: bash scripts/test-lsp-arduino-1951.sh
#   TAKO_1951_DUMP_DIR=<dir>   画像の置き場（既定は一時 = 終わると消える）
#   Arduino IDE（/Applications/Arduino IDE.app）か arduino:avr のコアが無い機では ⓪〜③ を「未実測」と出す。
#   面（tako-vd）を用意できない機（蓋閉じ）では ①〜⑤ の GUI の段を「未実測」と出し、⓪ / ⓪' だけを測る
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。macOS の bash 3.2 で通す（#1499 / #1518）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

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

ID="arduino-language-server"
IDE_DIR="/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources"
REAL_ARDUINO15="${TAKO_1951_ARDUINO15:-$HOME/Library/Arduino15}"
stamp() { # 本物の Arduino の data dir の印（無ければ空）
  local f
  for f in inventory.yaml package_index.json library_index.json; do
    [ -f "$REAL_ARDUINO15/$f" ] && stat -f '%m %N' "$REAL_ARDUINO15/$f"
  done
  true
}
STAMP_BEFORE="$(stamp)"

# 受け口のソケットのパス長に余裕を持たせるため短い置き場にする
TMP="$(mktemp -d /tmp/tk1951.XXXXXX)"
# symlink を解いた形にそろえる（エディタと言語サーバは /private/tmp/… で文書を持つ）
TMP="$(cd "$TMP" && pwd -P)"
DUMP="${TAKO_1951_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tako-1951-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
echo "visual-test 版の tako-app と tako をビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli -p tako-app --features tako-app/visual-test) || exit 1
(cd "$REPO_ROOT" && run_heavy cargo test -q -p tako-control --test issue1951_lsp_arduino --no-run) || exit 1
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
NEW_TAKO_BIN="$TAKO_BIN"
NEW_APP_BIN="$APP_BIN"

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
ms_now() { python3 -c 'import time; print(int(time.time() * 1000))'; }
tree_rss() { # pid とその子孫の RSS（MB）を「名前=MB」と合計で
  python3 - "$1" <<'PY'
import subprocess, sys, os
root = int(sys.argv[1])
rows = [l.split(None, 3) for l in subprocess.run(["ps", "-axo", "pid=,ppid=,rss=,comm="],
        capture_output=True, text=True).stdout.splitlines() if l.strip()]
kids, info = {}, {}
for pid, ppid, rss, comm in rows:
    kids.setdefault(int(ppid), []).append(int(pid))
    info[int(pid)] = (int(rss), os.path.basename(comm))
todo, seen = [root], []
while todo:
    x = todo.pop(); seen.append(x); todo.extend(kids.get(x, []))
parts = ["%s=%.1f" % (info[x][1], info[x][0] / 1024) for x in seen if x in info]
print(" ".join(parts), "合計=%.1f MB" % (sum(info[x][0] for x in seen if x in info) / 1024))
PY
}

# --- 隔離の土台 -------------------------------------------------------------------------
mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/rbin" "$DUMP"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
printf 'export PATH="%s:$PATH"\n' "$TMP/rbin" > "$TMP/zdot/.zprofile"
MIN_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
found="$(command -v tmux 2>/dev/null)"
[ -n "$found" ] && ln -s "$found" "$TMP/rbin/tmux"

# arduino:avr のコアを一時 HOME の既定の data dir へ複製する（ALS が起こす arduino-cli は空の設定ファイル =
# 既定の置き場を読む）
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
[ -x "$IDE_DIR/$ID" ] && [ -x "$IDE_DIR/arduino-cli" ] && [ -x "$IDE_DIR/clangd" ] && HAVE_IDE=1

export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"

# --- 素材（根に .git = ルートが repo の根へ上がらないことも見る）---------------------------
S="$TMP/sketches"
mkdir -p "$S/.git" "$S/Blink" "$S/NoBoard"
cat > "$S/Blink/Blink.ino" <<'EOF'
#include "helper.h"

const int blinkMs = 500;

void setup() {
  pinMode(LED_BUILTIN, OUTPUT);
  Serial.begin(9600);
}

void loop() {
  toggleLed(LED_BUILTIN);
  delay(blinkMss);
}
EOF
cat > "$S/Blink/helper.h" <<'EOF'
#pragma once
#include <Arduino.h>

inline void toggleLed(uint8_t pin) {
  digitalWrite(pin, !digitalRead(pin));
}
EOF
printf 'default_fqbn: arduino:avr:uno\n' > "$S/Blink/sketch.yaml"
cp "$S/Blink/Blink.ino" "$S/NoBoard/NoBoard.ino"
cp "$S/Blink/helper.h" "$S/NoBoard/helper.h"
# ② の帯の画像用（基板なし）と、⓪ の実経路用の複製（⓪ は NoBoard に基板を書くので ③ と分ける）
mkdir -p "$S/NoBoardVt"
cp "$S/Blink/Blink.ino" "$S/NoBoardVt/NoBoardVt.ino"
cp "$S/Blink/helper.h" "$S/NoBoardVt/helper.h"
S0="$TMP/sketches0"
mkdir -p "$S0/.git"
cp -R "$S/Blink" "$S/NoBoard" "$S0/"

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
# スケッチのフォルダをルートに持つ器の欄
server_field() { # server_field <ルート> <式>
  "$TAKO_BIN" lsp status --name "$ID" --json 2>/dev/null |
    python3 -c 'import json,sys
d = json.load(sys.stdin)
s = next((x for x in d.get("servers", []) if x.get("root") == sys.argv[1]), {})
print(eval("s" + sys.argv[2]))' "$1" "$2" 2>/dev/null
}
wait_state() { # wait_state <ルート> <状態> <秒>
  local n=0
  while [ "$n" -lt $(($3 * 10)) ]; do
    [ "$(server_field "$1" '.get("state")')" = "$2" ] && return 0
    sleep 0.1
    n=$((n + 1))
  done
  return 1
}

echo "== 前提 =="
echo "  Arduino IDE の同梱: $([ "$HAVE_IDE" = 1 ] && echo "あり（$("$IDE_DIR/arduino-cli" version 2>/dev/null | head -1 | sed "s#$TMP#<tmp>#g")・$("$IDE_DIR/clangd" --version 2>/dev/null | head -1)）" || echo 無い)"
echo "  arduino:avr のコア: $([ "$HAVE_CORE" = 1 ] && echo 一時 HOME へ複製した || echo 無い)"

echo
echo "== ⓪ GUI を除いた tako の実経路で実の同梱物を起こす（manager + 既定の解決） =="
if [ "$HAVE_IDE" = 1 ] && [ "$HAVE_CORE" = 1 ]; then
  # cargo と rustup は本物の HOME で動かし、テストの子プロセスだけを隔離 HOME で起こす
  (cd "$REPO_ROOT" && HOME="$REAL_HOME" TAKO_1951_REAL_HOME="$TMP/home" TAKO_1951_REAL_SKETCHES="$S0" \
    run_heavy cargo test -q -p tako-control --test issue1951_lsp_arduino \
    実機_arduino_ide_の同梱物で診断とホバーと補完が出る -- --exact --nocapture) > "$TMP/real.log" 2>&1
  real_rc=$?
  sed -n '/TAKO_1951_REAL:/,/^}/p' "$TMP/real.log" | sed "s#$TMP#<tmp>#g; s/^/    /"
  if [ "$real_rc" -eq 0 ] && grep -q "TAKO_1951_REAL:" "$TMP/real.log"; then
    pass "実の同梱物: bundled・12 行目の誤り 1 件・ホバー・補完・基板なしは起こさず選べば起きる"
  else
    fail "実の同梱物の実経路（$(grep -m1 -E 'panicked|assert' "$TMP/real.log" | sed "s#$TMP#<tmp>#g")）"
  fi
else
  unmeasured "⓪: Arduino IDE の同梱か arduino:avr のコアが無い"
fi

echo
echo "== ⓪' GUI 無しの CLI の A/B（tako lsp install --name ${ID}。旧 = 本番の /Applications/tako.app） =="
PROD_CLI="/Applications/tako.app/Contents/MacOS/tako"
mkdir -p "$TMP/nodisc"
cli_local() { env -u TAKO_SOCKET -u TAKO_TOKEN TAKO_DISCOVERY_DIR="$TMP/nodisc" TAKO_DATA_DIR="$TMP/cli-data" "$@" lsp install --name "$ID" --json 2>&1; }
new_out="$(cli_local "$NEW_TAKO_BIN")"
echo "    新: $(echo "$new_out" | head -c 300 | sed "s#$TMP#<tmp>#g")"
check_eq "新: 検出表の行として答える（tako が取らない行）" "$ID" "$(echo "$new_out" | json 'd["servers"][0]["id"]')"
if [ -x "$PROD_CLI" ]; then
  old_out="$(cli_local "$PROD_CLI")"
  echo "    旧: $(echo "$old_out" | head -c 300 | sed "s#$TMP#<tmp>#g")"
  check_eq "旧: 検出表に行が無い（servers が空か、知らない名前）" "True" \
    "$(echo "$old_out" | python3 -c 'import json,sys
t = sys.stdin.read()
try:
    d = json.loads(t)
    print(not any(s.get("id") == sys.argv[1] for s in d.get("servers", [])))
except Exception:
    print(True)' "$ID")"
else
  unmeasured "⓪': /Applications/tako.app が無い"
fi

GUI_OK=1
if ! "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1; then
  GUI_OK=0
  echo
  unmeasured "①〜⑤ の GUI の段: 面（tako-vd）を用意できない（$("$REPO_ROOT/scripts/lib/virtual-display.sh" status 2>&1 | head -1)。蓋閉じなら開いてから打ち直す）"
fi

if [ "$GUI_OK" = 1 ] && [ "$HAVE_IDE" = 1 ] && [ "$HAVE_CORE" = 1 ]; then
  echo
  echo "== ① 基板あり: 開くだけで 起動 → 誤りの診断（.ino の行）→ ホバー → 補完 =="
  start_gui "$TMP/data" "$TMP/gui.log"
  row="$("$TAKO_BIN" lsp servers --json 2>/dev/null | json '[r for r in d["servers"] if r["id"] == "'$ID'"][0]')"
  check_eq "tako lsp servers は 11 行" "11" "$("$TAKO_BIN" lsp servers --json 2>/dev/null | json 'len(d["servers"])')"
  echo "    servers の行: $(echo "$row" | sed "s#$TMP#<tmp>#g")"
  t0="$(ms_now)"
  pane="$(open_edit "$S/Blink/Blink.ino")" || { fail "開けない"; exit 1; }
  if wait_state "$S/Blink" running 60; then
    pass "操作なしで稼働（開いてから $(( $(ms_now) - t0 )) ms）"
  else
    fail "稼働に届かない（$(server_field "$S/Blink" '.get("state")') / $(server_field "$S/Blink" '.get("reason")')）"
  fi
  check_eq "出どころは Arduino IDE の同梱（bundled）" "bundled" "$(server_field "$S/Blink" '["source"]')"
  check_eq "ルートはスケッチのフォルダ" "$S/Blink" "$(server_field "$S/Blink" '["root"]')"
  echo "    起こした実行ファイル: $(server_field "$S/Blink" '["program_path"]')"
  diag=""
  for _ in $(seq 1 600); do
    diag="$("$TAKO_BIN" lsp diagnostics --pane "$pane" --json 2>/dev/null)"
    [ "$(echo "$diag" | json 'sum(len(x["diagnostics"]) for x in d["documents"])')" -gt 0 ] 2>/dev/null && break
    sleep 0.1
  done
  echo "    観測: 最初の診断まで $(( $(ms_now) - t0 )) ms"
  "$TAKO_BIN" lsp diagnostics --pane "$pane" 2>/dev/null | sed "s#$TMP#<tmp>#g; s/^/      /"
  check_eq "誤り（blinkMss）を .ino の 12 行目で 1 件" "[(12, 'error')]" \
    "$(echo "$diag" | json '[(x["range"]["start"]["line"], x["severity"]) for doc in d["documents"] for x in doc["diagnostics"]]')"
  hv=""
  for _ in $(seq 1 30); do
    hv="$("$TAKO_BIN" lsp hover --pane "$pane" --line 6 --column 4 --json 2>/dev/null)"
    [ "$(echo "$hv" | json 'd["status"]')" = "found" ] && break
    sleep 0.5
  done
  check_has "ホバー（6 行目の pinMode）" "pinMode(uint8_t pin, uint8_t mode)" "$(echo "$hv" | json 'd["contents"]')"
  cp="$("$TAKO_BIN" lsp completion --pane "$pane" --line 7 --column 9 --json 2>/dev/null)"
  echo "    補完（7 行目 Serial. の後）: $(echo "$cp" | json '[i["label"].strip() for i in d["items"]][:6]')"
  check_eq "補完に begin が出る" "True" "$(echo "$cp" | json 'any(i["label"].strip().startswith("begin") for i in d["items"])')"
  pid="$(server_field "$S/Blink" '["pid"]')"
  sleep 3
  echo "    観測: 常駐（ALS と子のプロセス・読み込み後 3 秒）: $(tree_rss "$pid")"
  echo "    観測: -cli-config のファイル: $(ls -l "$TMP/data/lsp-config/$ID/" 2>/dev/null | tail -1 | awk '{print $5 " バイト " $NF}')"
  stop_gui

  echo
  echo "== ② visual-test lsp-servers（同じ .ino の波線とホバーのカード + 基板なしの帯） =="
  launch_isolated_gui "$TMP/vt.log" HOME="$TMP/home" ZDOTDIR="$TMP/zdot" PATH="$MIN_PATH" \
    TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DATA_DIR="$TMP/data-vt" \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_TMUX_SOCKET="$TMUX_SOCKET" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=lsp-servers TAKO_VISUAL_DUMP_DIR="$DUMP" \
    TAKO_1956_CASES="$(printf '%s\t%s\t%s\n%s\t%s\t%s\t%s' "$S/Blink/Blink.ino" 5 4 \
      "$S/NoBoardVt/NoBoardVt.ino" 5 4 unconfigured)" || exit $?
  vpid="$ISOLATED_GUI_PID"
  for _ in $(seq 1 6000); do
    kill -0 "$vpid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$vpid"
  ISOLATED_GUI_PID=""
  grep "TAKO_VISUAL_PIXEL: lsp-servers id=\|TAKO_VISUAL_DUMP_FILE" "$TMP/vt.log" | sed "s#$TMP#<tmp>#g; s/^/    /"
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/vt.log"; then
    pass "visual-test lsp-servers: .ino で波線とホバーのカード・基板なしで帯が出る（TAKO_VISUAL_TEST_OK）"
  else
    fail "visual-test lsp-servers: $(grep -h 'FAILED' "$TMP/vt.log" | head -1)"
  fi

  echo
  echo "== ③ 基板なし: 起こさず理由と選ぶコマンド → tako のペインで選ぶと起きる =="
  start_gui "$TMP/data" "$TMP/gui-noboard.log"
  pane="$(open_edit "$S/NoBoard/NoBoard.ino")" || { fail "開けない"; exit 1; }
  if wait_state "$S/NoBoard" unconfigured 30; then
    pass "前提が未設定（unconfigured）で止まる"
  else
    fail "unconfigured に届かない（$(server_field "$S/NoBoard" '.get("state")')）"
  fi
  check_eq "サーバは起こしていない（pid なし・起動 0 回）" "(None, 0)" \
    "$(server_field "$S/NoBoard" '.get("pid"), s.get("spawn_count")')"
  echo "    理由: $(server_field "$S/NoBoard" '["reason"]' | sed "s#$TMP#<tmp>#g")"
  echo "    次の一手: $(server_field "$S/NoBoard" '["next_step"]' | sed "s#$TMP#<tmp>#g")"
  cmd="$(server_field "$S/NoBoard" '["setup_command"]')"
  check_has "選ぶコマンドは board attach" " board attach -b " "$cmd"
  # 案内の <基板> を実の名前へ替えて、tako のペインで打つ（tako のターミナルで選んだ形）
  real_cmd="$(printf '%s' "$cmd" | sed 's/<基板>/arduino:avr:uno/; s/<board>/arduino:avr:uno/')"
  "$TAKO_BIN" send --pane "$ROOT" "$real_cmd" >/dev/null 2>&1
  for _ in $(seq 1 100); do [ -f "$S/NoBoard/sketch.yaml" ] && break; sleep 0.1; done
  check_has "コマンドで sketch.yaml に基板が書かれた" "default_fqbn: arduino:avr:uno" "$(cat "$S/NoBoard/sketch.yaml" 2>/dev/null)"
  if wait_state "$S/NoBoard" running 20; then
    pass "シェル統合の合図で確かめ直して起きた（restart 不要）"
  else
    echo "    観測: 合図では起きなかった（状態 $(server_field "$S/NoBoard" '.get("state")')）。帯の「もう一度確かめる」と同じ restart を打つ"
    "$TAKO_BIN" lsp restart --name "$ID" >/dev/null 2>&1
    if wait_state "$S/NoBoard" running 30; then
      pass "restart（帯の「もう一度確かめる」と同じ口）で起きた"
    else
      fail "基板を選んでも起きない（$(server_field "$S/NoBoard" '.get("state")')）"
    fi
  fi
  stop_gui
elif [ "$GUI_OK" = 1 ]; then
  unmeasured "①〜③: Arduino IDE の同梱か arduino:avr のコアが無い"
fi

if [ "$GUI_OK" = 1 ]; then
echo
echo "== ④ ALS なし（注入 TAKO_1951_INJECT_NO_BUNDLE=1 = Arduino IDE の無い PC と同じ解決） =="
start_gui "$TMP/data-noide" "$TMP/gui-noide.log" TAKO_1951_INJECT_NO_BUNDLE=1
pane="$(open_edit "$S/Blink/Blink.ino")" || { fail "開けない"; exit 1; }
if wait_state "$S/Blink" not_installed 30; then
  pass "未導入へ落ちる"
else
  fail "未導入に届かない（$(server_field "$S/Blink" '.get("state")')）"
fi
reason="$(server_field "$S/Blink" '["reason"]')"
echo "    理由: $reason"
echo "    次の一手: $(server_field "$S/Blink" '["next_step"]')"
check_has "理由は同梱も探したことを名指す" "Arduino IDE" "$reason"
check_eq "導入コマンドは IDE ごと（言語サーバまで揃う方）" "brew install --cask arduino-ide" "$(server_field "$S/Blink" '["install_command"]')"
"$TAKO_BIN" edit replace-range 1:0 1:0 "// x
" --pane "$pane" >/dev/null 2>&1
check_eq "編集はそのまま続けられる（editing・dirty）" "(True, True)" \
  "$("$TAKO_BIN" edit status --pane "$pane" 2>/dev/null | json '(d["editing"], d["dirty"])')"
stop_gui

echo
echo "== ⑤ A/B: 本番の /Applications/tako.app（#1951 より前）では .ino に言語サーバが付かない =="
PROD_APP="/Applications/tako.app/Contents/MacOS/tako-app"
if [ -x "$PROD_APP" ] && [ -x "$PROD_CLI" ]; then
  TAKO_BIN="$PROD_CLI" APP_BIN="$PROD_APP"
  start_gui "$TMP/data-old" "$TMP/gui-old.log"
  open_edit "$S/Blink/Blink.ino" >/dev/null
  sleep 3
  check_eq "旧: .ino を受け持つ器が無い（servers 0 件）" "0" \
    "$("$TAKO_BIN" lsp status --json 2>/dev/null | json 'len(d["servers"])')"
  check_eq "旧: 検出表に ALS の行が無い" "False" \
    "$("$TAKO_BIN" lsp servers --json 2>/dev/null | json 'any(r["id"] == "'$ID'" for r in d["servers"])')"
  stop_gui
  TAKO_BIN="$NEW_TAKO_BIN" APP_BIN="$NEW_APP_BIN"
else
  unmeasured "⑤: /Applications/tako.app が無い"
fi
fi # GUI_OK

echo
echo "== 本物の Arduino の data dir に触れていない =="
check_eq "inventory.yaml / package_index.json / library_index.json の mtime が前後で同じ" "$STAMP_BEFORE" "$(stamp)"

echo
echo "結果: OK $PASS / NG $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ]
