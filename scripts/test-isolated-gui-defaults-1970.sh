#!/bin/bash
# test-isolated-gui-defaults-1970.sh — 隔離 GUI の既定（#1970）の実経路テスト
#
# 本番 tako のペインに見立てた env（**偽の値**。本物の本番の値は最初に落とす）を export したまま
# `scripts/lib/isolated-gui.sh` で隔離 GUI を起こし、次を実測する:
#   ① 自動リネームは既定で止まる: ペインで cd しても偽の claude（TAKO_CLAUDE_BIN）が 0 回。
#      対照に TAKO_AUTO_RENAME=1 を並べた起動では呼ばれる（数え方が効いている）
#   ② GUI（ps eww）と配下のペイン（ペインで env を書き出す）の env に本番の値が 1 つも無い。
#      CLAUDE_CONFIG_DIR は、一時の HOME なら外れ（claude はその HOME の .claude を使う）、
#      HOME を利用者のホームのままにした起動ではログの置き場の隣の一時 dir
#   ③ 一時の HOME に既定の .zshrc が置かれ、ペインのプロンプトが固定の形（tako …）になる
# A/B: TAKO_1970_LEGACY_HELPER=<#1970 前の isolated-gui.sh> を渡すと、同じ手順をその版で回す
# （① で呼ばれ・② で本番の値が届くのが旧挙動。FAIL が出るのが正しい）。
#
# 本物の claude は起こさない（自動リネームの claude は TAKO_CLAUDE_BIN の偽物。HOME も一時 dir）。
# 本番の GUI / data dir / tmux には触らない。窓は tako-vd（#1141。面を用意できなければ 4 = 未実測）。
# 落とすのは自分で起こした pid だけ。描画は要らない（env・ファイル・ペインの文字だけを見る）。
#
# 使い方: bash scripts/test-isolated-gui-defaults-1970.sh
set -uo pipefail

# 本物の本番の値を最初に落とす（以下で export するのは偽の値だけ）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_CLI TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS \
  TAKO_ORCHESTRATOR_ROLE TAKO_BACKEND_SOCKET TAKO_OSC_SINK CLAUDE_CONFIG_DIR ZDOTDIR TAKO_AUTO_RENAME

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '$2' / 実際 '$3'）"; fi; }

TMP="$(mktemp -d /tmp/t1970-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
GUI_PIDS=""
cleanup() {
  local p
  for p in $GUI_PIDS; do stop_isolated_gui "$p"; done
  [ -n "${KEEP_TMP:-}" ] && { echo "一時 dir を残した: $TMP"; return; }
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "${TAKO_1970_LEGACY_HELPER:-$REPO_ROOT/scripts/lib/isolated-gui.sh}"
# ビルドは HOME を差し替える前に（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1
command -v jq >/dev/null || { echo "jq が要る"; exit 1; }

REAL_HOME="$HOME"
export HOME="$TMP/home"
mkdir -p "$HOME" "$TMP/proj-a" "$TMP/proj-b" "$TMP/bin" "$TMP/prod"
case "$HOME" in "$TMP"/*) : ;; *) echo "HOME が隔離されていない"; exit 1 ;; esac

# 偽の claude: 呼ばれた回数と、受け取った env に偽の本番の値があったかを書く（API は呼ばない）。
# 版の問い合わせ（`--version`。mod の注入の判定が使う。会話も設定 dir も起こさない）は数えない
FAKE_LOG="$TMP/fake-claude.log"
cat > "$TMP/bin/claude" <<FAKE
#!/bin/bash
printf 'args=%s cfg=%s prod=%s\n' "\$*" "\${CLAUDE_CONFIG_DIR-<unset>}" "\$(env | grep -c tako-1970-prod)" >> "$FAKE_LOG"
exit 1
FAKE
chmod +x "$TMP/bin/claude"
: > "$FAKE_LOG"

# 本番 tako のペインに見立てた値（どれも存在しない置き場か印だけ）。起動の間だけ export する
PROD_MARK="tako-1970-prod"
with_prod_env() {
  export TAKO_SOCKET="$TMP/prod/$PROD_MARK.sock" TAKO_TOKEN="$PROD_MARK-token" TAKO_PANE_ID=99901 \
    TAKO_TAB_ID=99902 TAKO_CLI="$TMP/prod/$PROD_MARK-cli" TAKO_MCP_URL="http://127.0.0.1:9/$PROD_MARK" \
    CLAUDE_CODE_PLUGIN_DIRS="$TMP/prod/$PROD_MARK-mod" TAKO_ORCHESTRATOR_ROLE="worker:$PROD_MARK" \
    TAKO_BACKEND_SOCKET="$PROD_MARK" TAKO_OSC_SINK="$TMP/prod/$PROD_MARK-sink" \
    CLAUDE_CONFIG_DIR="$TMP/prod/$PROD_MARK-claude-config"
}
drop_prod_env() {
  unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_CLI TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS \
    TAKO_ORCHESTRATOR_ROLE TAKO_BACKEND_SOCKET TAKO_OSC_SINK CLAUDE_CONFIG_DIR
}

# 腕ごとの置き場（data / discovery / tmux を分ける = 前の腕の器を拾わない）
arm_env() {
  echo "TAKO_DATA_DIR=$TMP/$1/data"
  echo "TAKO_DISCOVERY_DIR=$TMP/$1/disc"
  echo "TAKO_ORCHESTRATOR_DIR=$TMP/$1/orch"
  echo "TAKO_SESSIONS_FILE=$TMP/$1/sessions.yaml"
  echo "TAKO_PANE_LOG_DIR=$TMP/$1/panelogs"
  echo "TAKO_WORKERS_FILE=$TMP/$1/workers.yaml"
  echo "TAKO_TMUX_SOCKET=t1970-$1-$$"
}
# CLI は隔離 GUI だけへ（偽の本番の値も落として呼ぶ）
tk() { # tk <腕> <引数…>
  local arm="$1"
  shift
  # shellcheck disable=SC2046
  env -u TAKO_SOCKET -u TAKO_TOKEN -u TAKO_PANE_ID -u TAKO_TAB_ID TAKO_ISOLATED=1 $(arm_env "$arm") \
    "$TAKO_BIN" "$@" 2>/dev/null
}

APP_PID=""
start_arm() { # start_arm <腕> [VAR=VAL…]
  local arm="$1" rc=0 i
  shift
  mkdir -p "$TMP/$arm/data" "$TMP/$arm/disc" "$TMP/$arm/orch"
  with_prod_env
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/$arm/app.log" $(arm_env "$arm") TAKO_CLAUDE_BIN="$TMP/bin/claude" \
    "PATH=$TMP/bin:$PATH" ${1+"$@"} || rc=$?
  drop_prod_env
  [ "$rc" = 0 ] || exit "$rc"
  APP_PID="$ISOLATED_GUI_PID"
  GUI_PIDS="$GUI_PIDS $APP_PID"
  for i in $(seq 1 300); do
    tk "$arm" list >/dev/null && return 0
    sleep 0.2
  done
  echo "腕 $arm の GUI へ繋がらない"
  tail -20 "$TMP/$arm/app.log"
  exit 1
}
stop_arm() { stop_isolated_gui "$APP_PID"; APP_PID=""; }
first_pane() { tk "$1" list | jq -r '.tabs[0].panes[0].id'; }

# ペインの env をファイルへ書き出して待つ
pane_env() { # pane_env <腕> <ペイン> <書き出し先>
  local i
  tk "$1" send --pane "$2" "env > '$3.tmp' && mv '$3.tmp' '$3'" >/dev/null
  for i in $(seq 1 100); do
    [ -s "$3" ] && return 0
    sleep 0.2
  done
  return 1
}
# 偽の claude の呼び出しを待つ（来たら 0）
talk_calls() { grep -vc "^args=--version " "$FAKE_LOG" | tr -d " "; }
wait_fake_calls() { # wait_fake_calls <秒>
  local i
  for i in $(seq 1 $(($1 * 5))); do
    [ "$(talk_calls)" -gt 0 ] && return 0
    sleep 0.2
  done
  return 1
}
# GUI の env（ps eww の 1 行）の中の `NAME=値` の値
gui_env_value() { # gui_env_value <pid> <NAME>
  ps eww -o command= -p "$1" | tr ' ' '\n' | sed -n "s/^$2=//p" | head -1
}

echo "=== 対照: TAKO_AUTO_RENAME=1 を並べると自動リネームが偽の claude を呼ぶ ==="
start_arm rename-on TAKO_AUTO_RENAME=1
P="$(first_pane rename-on)"
T0=$(date +%s)
tk rename-on send --pane "$P" "cd '$TMP/proj-a'" >/dev/null
if wait_fake_calls 60; then
  CONTROL_SECS=$(($(date +%s) - T0))
  pass "並べた起動では自動リネームが偽の claude を呼んだ（${CONTROL_SECS} 秒・先頭の引数 '$(grep -v '^args=--version ' "$FAKE_LOG" | head -1 | cut -d' ' -f1)'）= 数え方が効いている"
else
  CONTROL_SECS=60
  fail "TAKO_AUTO_RENAME=1 でも 60 秒で偽の claude が呼ばれない（① の 0 回を確かめられない）"
fi
check_eq "呼ばれた claude の env に偽の本番の値が無い" "0" \
  "$(sed -n 's/.* prod=\([0-9]*\)$/\1/p' "$FAKE_LOG" | sort -n | tail -1)"
stop_arm

echo
echo "=== ① ② ③ 既定（一時の HOME）==="
: > "$FAKE_LOG"
rm -f "$HOME/.zshrc" # 対照の腕が置いたものを消す（この起動が置くことを見る）
start_arm default
P="$(first_pane default)"
GUI_ENV="$(ps eww -o command= -p "$APP_PID")"
check_eq "② GUI の env に偽の本番の値が無い" "0" "$(printf '%s' "$GUI_ENV" | grep -o "$PROD_MARK" | wc -l | tr -d ' ')"
FOUND=""
for k in TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_CLI TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS \
  TAKO_ORCHESTRATOR_ROLE CLAUDECODE CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION; do
  case " $GUI_ENV" in *" $k="*) FOUND="$FOUND $k" ;; esac
done
check_eq "② GUI の env に本番 tako と呼び出し元の claude の名前が無い（11 個を見た）" "" "$FOUND"
check_eq "② 一時の HOME なので GUI の CLAUDE_CONFIG_DIR は外れている" "" "$(gui_env_value "$APP_PID" CLAUDE_CONFIG_DIR)"
check_eq "① GUI の自動リネームは 0（TAKO_AUTO_RENAME）" "0" "$(gui_env_value "$APP_PID" TAKO_AUTO_RENAME)"
if [ -f "$HOME/.zshrc" ] && grep -q "PROMPT='tako %1~ %% '" "$HOME/.zshrc"; then
  pass "③ 一時の HOME に既定の .zshrc が置かれた"
else
  fail "③ 一時の HOME に既定の .zshrc が無い"
fi
if pane_env default "$P" "$TMP/pane-env-default.txt"; then
  check_eq "② ペインの env に偽の本番の値が無い" "0" "$(grep -c "$PROD_MARK" "$TMP/pane-env-default.txt")"
  check_eq "② ペインの CLAUDE_CONFIG_DIR も無い（claude は一時の HOME の .claude）" "0" \
    "$(grep -c '^CLAUDE_CONFIG_DIR=' "$TMP/pane-env-default.txt")"
  check_eq "② ペインの HOME は一時の HOME" "$HOME" "$(sed -n 's/^HOME=//p' "$TMP/pane-env-default.txt")"
  SOCK="$(sed -n 's/^TAKO_SOCKET=//p' "$TMP/pane-env-default.txt")"
  case "$SOCK" in
    "$TMP"/default/*) pass "② ペインの TAKO_SOCKET は隔離 GUI 自身の口（$TMP/default 配下）" ;;
    *) fail "② ペインの TAKO_SOCKET が隔離 GUI の口ではない: $SOCK" ;;
  esac
else
  fail "② ペインの env を書き出せない（ペインのシェルが入力を受けない = 初回設定メニュー等）"
fi
tk default send --pane "$P" "cd '$TMP/proj-b'" >/dev/null
WAIT=$((CONTROL_SECS * 2))
[ "$WAIT" -lt 30 ] && WAIT=30
sleep "$WAIT"
check_eq "① 既定では ${WAIT} 秒たっても偽の claude は 0 回（対照の倍以上待った）" "0" \
  "$(talk_calls)"
if screen="$(tk default read --pane "$P")" && grep -q 'tako proj-b %' <<<"$screen"; then
  pass "③ ペインのプロンプトが既定の .zshrc の固定の形（tako proj-b %）"
else
  # 画面は出さない（固定の形でないプロンプトには実ユーザー名・ホスト名が写る = #927）
  fail "③ ペインのプロンプトが固定の形（tako proj-b %）になっていない"
fi
stop_arm

echo
echo "=== ② HOME を利用者のホームのままにした起動（ZDOTDIR だけ一時）==="
mkdir -p "$TMP/zdot"
start_arm realhome "HOME=$REAL_HOME" "ZDOTDIR=$TMP/zdot"
CFG="$(gui_env_value "$APP_PID" CLAUDE_CONFIG_DIR)"
check_eq "② CLAUDE_CONFIG_DIR はログの置き場の隣の一時 dir" "$TMP/realhome/isolated-claude-config" "$CFG"
[ -d "$CFG" ] && pass "② その一時 dir が作られている" || fail "② 一時の設定 dir が無い: $CFG"
[ -f "$TMP/zdot/.zshrc" ] && pass "③ 既定の .zshrc は一時の ZDOTDIR へ（利用者のホームには書かない）" \
  || fail "③ 一時の ZDOTDIR に既定の .zshrc が無い"
check_eq "② GUI の env に偽の本番の値が無い" "0" \
  "$(ps eww -o command= -p "$APP_PID" | grep -o "$PROD_MARK" | wc -l | tr -d ' ')"
stop_arm

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
