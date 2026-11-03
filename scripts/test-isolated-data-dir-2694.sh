#!/usr/bin/env bash
# test-isolated-data-dir-2694.sh — 隔離の印（TAKO_ISOLATED=1）で TAKO_DATA_DIR を付けない CLI の手元の処理が、
# 既定（本番）の data dir を使わないことの実経路テスト（#2694。#2531 の続き）
#
# **既定の置き場そのものを砂場にする**: HOME を一時 dir へ向けるので、修正前の挙動（断らずに既定の置き場へ
# 書く）を確かめても本物の data dir には届かない（macOS は `$HOME/Library/Application Support/tako`、
# 他の unix は `$HOME/.local/share/tako`。XDG_DATA_HOME は外す）。GUI は起こさない（CLI だけ・tako-vd 不要）。
#   ① 修正前（A/B の TAKO_2694_LEGACY=1）: `tako usage` が断らずに既定の置き場へ索引を書く
#   ② 修正後: 手元のサブコマンドが日英で断り（終了コード 1）、既定の置き場を作らない
#   ③ TAKO_DATA_DIR を付ければ従来どおり動き、書くのはその置き場だけ
#   ④ 門の外のもの: 静的な表（platform / agent-support）は従来どおり答え、GUI へ繋ぐもの（list）は #2531 の
#      接続の断りのまま（data dir の断りではない）
#   ⑤ tako queue は修正前と同じ: GUI が居ないときの枠の上限は既定の置き場の settings.json へ書く（#2531 と同じ例外）
#
# 使い方: bash scripts/test-isolated-data-dir-2694.sh（TAKO_BIN を渡せばビルドしない）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1970 / #2527）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_DATA_DIR TAKO_DISCOVERY_DIR TAKO_ORCHESTRATOR_DIR \
  TAKO_SESSIONS_FILE TAKO_PANE_LOG_DIR TAKO_REMOTE_STATE_DIR TAKO_2694_LEGACY TAKO_2531_LEGACY XDG_DATA_HOME
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

if [ -z "${TAKO_BIN:-}" ]; then
  run_heavy cargo build -q -p tako-cli || exit 1
  TAKO_BIN="$REPO_ROOT/target/debug/tako"
fi

TMP="$(mktemp -d /tmp/tako-2694-XXXXXX)"
cleanup() { rm -rf "${TMP:?}"; }
trap cleanup EXIT
# 既定の置き場を砂場にする（HOME ごと）。一時の置き場（検証用の設定置き場ほか）も砂場の中へ
export HOME="$TMP/home" USERPROFILE="$TMP/home" TMPDIR="$TMP/tmp"
mkdir -p "$HOME" "$TMPDIR"
if [ "$(uname)" = Darwin ]; then
  DEF="$HOME/Library/Application Support/tako"
else
  DEF="$HOME/.local/share/tako"
fi
JA_REFUSAL="TAKO_ISOLATED=1 なので既定（本番）のデータディレクトリは使わない"
EN_REFUSAL="TAKO_ISOLATED=1 is set, so the default (production) data directory is not used"
CONNECT_REFUSAL="既定（本番）の置き場の接続情報は読まない"

# 隔離の印だけ付けて（置き場は付けずに）CLI を起こす。出力は $TMP/out、終了コードは $RC
iso() {
  env TAKO_ISOLATED=1 TAKO_LANG=ja "$TAKO_BIN" "$@" >"$TMP/out" 2>&1
  RC=$?
}
reset_def() { rm -rf "${DEF:?}"; }
def_state() { if [ -e "$DEF" ]; then echo present; else echo absent; fi; }

echo "== ① 修正前（TAKO_2694_LEGACY=1）: 断らずに既定の置き場へ書く"
reset_def
env TAKO_ISOLATED=1 TAKO_LANG=ja TAKO_2694_LEGACY=1 "$TAKO_BIN" usage >"$TMP/out" 2>&1
check_eq "tako usage は終了コード 0" "0" "$?"
check_eq "既定の置き場に usage の索引ができる" "yes" "$([ -d "$DEF/usage" ] && echo yes || echo no)"
reset_def

echo "== ② 修正後: 手元のサブコマンドは断って既定の置き場を作らない"
refused=0
total=0
while IFS= read -r line; do
  [ -n "$line" ] || continue
  total=$((total + 1))
  # shellcheck disable=SC2086 # 1 行 = 空白で区切った引数
  iso $line
  if [ "$RC" = 1 ] && grep -qF "$JA_REFUSAL" "$TMP/out" && [ "$(def_state)" = absent ]; then
    refused=$((refused + 1))
  else
    fail "tako ${line}（終了コード ${RC}・既定の置き場 $(def_state)）: $(head -c 200 "$TMP/out" | tr '\n' ' ')"
  fi
  reset_def
done <<'CMDS'
usage
usage --by issue
migrate
telemetry status
sleep-guard status
config status
shell-integration status
orchestrator handoffs list
orchestrator profiles list
orchestrator projects list
setup --check
setup --changes --json
remote status
mod ui
recover
context-budget
CMDS
check_eq "手元のサブコマンド ${total} 種が断る（終了コード 1・日本語の断り・既定の置き場なし）" "$total" "$refused"
env TAKO_ISOLATED=1 TAKO_LANG=en "$TAKO_BIN" usage >"$TMP/out" 2>&1
RC=$?
check_eq "英語表示の断り（終了コード 1）" "1" "$RC"
check_eq "英語表示の断りの文" "yes" "$(grep -qF "$EN_REFUSAL" "$TMP/out" && echo yes || echo no)"
check_eq "英語表示に日本語が混ざらない" "0" "$(LC_ALL=C grep -c '[^[:print:][:space:]]' "$TMP/out")"

echo "== ③ TAKO_DATA_DIR を付ければ従来どおり"
env TAKO_ISOLATED=1 TAKO_LANG=ja TAKO_DATA_DIR="$TMP/data" "$TAKO_BIN" usage --by issue >"$TMP/out" 2>&1
check_eq "tako usage --by issue は終了コード 0" "0" "$?"
check_eq "索引は指定した置き場へ" "yes" "$([ -d "$TMP/data/usage" ] && echo yes || echo no)"
check_eq "既定の置き場は作らない" "absent" "$(def_state)"

echo "== ④ 門の外のもの"
iso platform
check_eq "platform（静的な表）は終了コード 0" "0" "$RC"
iso agent-support
check_eq "agent-support（静的な表）は終了コード 0" "0" "$RC"
iso list
check_eq "list（GUI へ繋ぐ）は終了コード 1" "1" "$RC"
check_eq "list は #2531 の接続の断り" "yes" "$(grep -qF "$CONNECT_REFUSAL" "$TMP/out" && echo yes || echo no)"
check_eq "list は data dir の断りではない" "no" "$(grep -qF "$JA_REFUSAL" "$TMP/out" && echo yes || echo no)"
check_eq "どれも既定の置き場を作らない" "absent" "$(def_state)"

echo "== ⑤ tako queue は修正前と同じ（GUI が居ないときの枠の上限は既定の置き場へ）"
for mode in legacy new; do
  reset_def
  if [ "$mode" = legacy ]; then
    env TAKO_ISOLATED=1 TAKO_LANG=ja TAKO_2694_LEGACY=1 "$TAKO_BIN" queue limits --max-jobs 2 >"$TMP/q-$mode" 2>&1
  else
    env TAKO_ISOLATED=1 TAKO_LANG=ja "$TAKO_BIN" queue limits --max-jobs 2 >"$TMP/q-$mode" 2>&1
  fi
  echo "$?" >"$TMP/q-$mode.rc"
  grep -o '"max_jobs": *[0-9]*' "$DEF/settings.json" 2>/dev/null | tr -d ' ' >"$TMP/q-$mode.saved"
done
check_eq "queue limits の終了コードが修正前と同じ" "$(cat "$TMP/q-legacy.rc")" "$(cat "$TMP/q-new.rc")"
check_eq "queue limits の出力が修正前と同じ" "$(cat "$TMP/q-legacy")" "$(cat "$TMP/q-new")"
check_eq "queue limits は既定の置き場の settings.json へ書く（修正前と同じ）" "$(cat "$TMP/q-legacy.saved")" "$(cat "$TMP/q-new.saved")"
check_eq "書いた値" '"max_jobs":2' "$(cat "$TMP/q-new.saved")"
reset_def

echo
echo "結果: PASS $PASS / FAIL $FAIL"
[ "$FAIL" = 0 ]
