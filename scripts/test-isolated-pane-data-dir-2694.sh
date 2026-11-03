#!/usr/bin/env bash
# test-isolated-pane-data-dir-2694.sh — 隔離 GUI の中のペインから打つ CLI が、data dir の門（#2694）で
# 断られないことの実経路テスト
#
# 隔離 GUI は TAKO_DATA_DIR を渡さずに起こすと自分の置き場（`$TMPDIR/tako-iso-data-<pid>`）を決める（#177）。
# その値がペインの env に届いていなければ、ペインの中の `tako usage` などの手元の処理（セルフテストの CLI / MCP の
# e2e・隔離 GUI の中の worker）が軒並み断られる。素のペイン（TAKO_PERSIST=0 = 隔離の既定）と器の中のペイン
# （TAKO_PERSIST=1 + 専用の tmux ソケット）の両方で、ペインに打った手元のコマンドの終了コードとペインの
# TAKO_DATA_DIR を読み、GUI 自身の置き場と一致することを見る。
#
# **本番には触らない**: HOME・発見の置き場・tmux は一時 dir と専用ソケット。窓は常設の仮想ディスプレイ（tako-vd）へ
# 出る（#1141）。落とすのは自分で起こした pid とソケットだけ。**CI には登録しない**（実 GUI が要る）。
#
# 使い方: bash scripts/test-isolated-pane-data-dir-2694.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1970 / #2527）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_DATA_DIR TAKO_ORCHESTRATOR_DIR TAKO_SESSIONS_FILE \
  TAKO_PANE_LOG_DIR TAKO_REMOTE_STATE_DIR TAKO_2694_LEGACY XDG_DATA_HOME
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2694p-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2694p-$$"
cleanup() {
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
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# 置き場（TAKO_DATA_DIR）は渡さない = GUI が自分の置き場を決める。発見の置き場だけ渡して iso_cli で繋ぐ
export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR"

# ペインに 1 行打ち、目印の行が出るまで待って読む
#   probe <ペイン> → 標準出力に「rc=<n> dd=<TAKO_DATA_DIR>」
probe() {
  local pane="$1" mark="tk2694-$RANDOM" i out
  iso_cli send --pane "$pane" \
    "\"$TAKO_BIN\" usage --by issue >/dev/null 2>&1; echo \"$mark rc=\$? dd=\${TAKO_DATA_DIR:-unset}\"" \
    >/dev/null 2>&1
  for i in $(seq 1 100); do
    out="$(iso_cli read --pane "$pane" 2>/dev/null | grep -o "$mark rc=[0-9]* dd=[^ ]*" | tail -1)"
    if [ -n "$out" ]; then
      echo "${out#"$mark "}"
      return 0
    fi
    sleep 0.2
  done
  echo "timeout"
}

run_case() {
  local label="$1" pid pane got want
  shift
  launch_isolated_gui "$TMP/app-$label.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$label.log" || exit 1
  pid="$APP_PID"
  pane="$(iso_cli list 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
  got="$(probe "$pane")"
  want="rc=0 dd=${TMPDIR:-/tmp}"
  want="${want%/}/tako-iso-data-$pid"
  echo "    $label: $got"
  check_eq "$label: ペインの手元の CLI は断られない（終了コード 0）" "rc=0" "${got%% *}"
  check_eq "$label: ペインの TAKO_DATA_DIR は GUI 自身の置き場" "$(basename "$want")" "$(basename "${got#* dd=}")"
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux_socket_kill "$TMUX_SOCKET"
}

echo "== 素のペイン（TAKO_PERSIST=0 = 隔離の既定）"
run_case direct
echo "== 器の中のペイン（TAKO_PERSIST=1 + 専用の tmux ソケット）"
run_case container TAKO_PERSIST=1 TAKO_TMUX_SOCKET="$TMUX_SOCKET"

echo
echo "結果: PASS $PASS / FAIL $FAIL"
[ "$FAIL" = 0 ]
