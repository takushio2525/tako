#!/usr/bin/env bash
# #2199: spawn の記録（委任台帳の `issue`・セッションカタログ / worker レジストリの `issues` の先頭）が
# ラベルに書いた主の Issue になることを、隔離した tako-app（実ウィンドウ）+ 実 CLI の spawn で測る。
#
# 修正前は `extract_issues` が拾った番号を昇順に並べ、台帳はその先頭 = **最小の番号**を記録していた
# （プロンプトが前例として触れた `#322` 等が主になる。設計書 `.agent/plans/2026-10-usage-analytics.md` §1.4）。
#
# 腕（同じバイナリ）:
#   new    … 修正後（既定）
#   legacy … TAKO_2199_LEGACY=1（旧挙動。最小の番号が入ることを確かめる対照）
#
# claude は偽物（引数を記録して眠るだけ）なので実エージェントは起動しない。シェルは検証用の zsh
# （ZDOTDIR を一時の置き場へ向ける = ユーザーの rc・履歴は読まない / 書かない）。
# 本番の tako（GUI / 設定 / projects / workers / ledger / sessions）には触らない。
#
# 使い方: scripts/test-ledger-primary-issue-2199.sh            # 両腕
#         ARMS=new scripts/test-ledger-primary-issue-2199.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
ARMS="${ARMS:-new legacy}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
expect() {
  # expect <期待> <実測> <説明>
  if [ "${1}" = "${2}" ]; then pass "${3}（${2}）"; else bad "${3}（期待 ${1} / 実測 ${2}）"; fi
}

for bin in zsh python3; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done
isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TMUX

TMP_ROOTS=""
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local s d
  # 器は既定で OFF だが、立っていたら自分のソケットだけ落とす
  if command -v tmux >/dev/null 2>&1; then
    for s in $SOCKETS; do tmux -L "$s" kill-server >/dev/null 2>&1; done
  fi
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do rm -rf "$d"; done
  fi
}
trap cleanup EXIT

# 台帳の 1 件の issue（ledger_id で引く。issue が無ければ null・その件が無ければ missing）
ledger_issue() {
  "$TAKO_BIN" orchestrator ledger list --limit 1000 2>/dev/null | python3 -c '
import json, sys
want = sys.argv[1]
for e in json.load(sys.stdin).get("entries", []):
    if e.get("id") == want:
        v = e.get("issue")
        print("null" if v is None else v)
        break
else:
    print("missing")
' "$1"
}

# worker レジストリの 1 件（pane で引く）の field。issues は「,」区切り
worker_field() {
  "$TAKO_BIN" orchestrator workers 2>/dev/null | python3 -c '
import json, sys
pane, field = int(sys.argv[1]), sys.argv[2]
for w in json.load(sys.stdin).get("workers", []):
    if w.get("pane") == pane:
        v = w.get(field)
        if isinstance(v, list):
            print(",".join(str(x) for x in v) or "-")
        else:
            print("null" if v is None else v)
        break
else:
    print("missing")
' "$1" "$2"
}

# セッションカタログの spawn 記録（pending。pane で引く）の issues（「,」区切り）
catalog_issues() {
  "$TAKO_BIN" sessions list --json 2>/dev/null | python3 -c '
import json, sys
pane = int(sys.argv[1])
d = json.load(sys.stdin)
for p in d.get("pending", []):
    if p.get("pane") == pane:
        print(",".join(str(x) for x in p.get("issues") or []) or "-")
        break
else:
    for s in d.get("sessions", []):
        if s.get("pane") == pane:
            print(",".join(str(x) for x in s.get("issues") or []) or "-")
            break
    else:
        print("missing")
' "$1"
}

# 1 本 spawn して、台帳・カタログ・レジストリの値を読み比べる
# case_spawn <腕> <ラベル> <プロンプト> <新しい腕の期待（主 / issues）> <旧い腕の期待（主 / issues）> <説明>
case_spawn() {
  local arm="$1" label="$2" prompt="$3" want_new="$4" want_old="$5" what="$6"
  local resp pane ledger_id want want_primary want_list issue cat reg
  echo "  --- ${what}: ラベル「${label}」/ プロンプト「${prompt}」"
  resp="$("$TAKO_BIN" orchestrator spawn --project t2199 --prompt "$prompt" --pane "$MASTER" --label "$label" 2>&1)"
  pane="$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane_id"])' 2>/dev/null)"
  if [ -z "$pane" ]; then
    bad "spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
    return
  fi
  want="$want_new"
  [ "$arm" = legacy ] && want="$want_old"
  want_primary="${want%%/*}"
  want_list="${want#*/}"
  ledger_id="$(worker_field "$pane" ledger_id)"
  issue="$(ledger_issue "$ledger_id")"
  cat="$(catalog_issues "$pane")"
  reg="$(worker_field "$pane" issues)"
  echo "    pane=${pane} ledger_id=${ledger_id} 台帳 issue=${issue} カタログ issues=${cat} レジストリ issues=${reg}"
  local tag="修正後"
  [ "$arm" = legacy ] && tag="旧挙動の再現（対照）"
  expect "$want_primary" "$issue" "${tag}: 台帳の issue"
  expect "$want_list" "$cat" "${tag}: カタログの issues"
  expect "$want_list" "$reg" "${tag}: レジストリの issues"
}

run_arm() {
  local arm="$1" TMP
  TMP="$(mktemp -d /tmp/t2199-XXXXXX)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  echo
  echo "=== 腕: ${arm} ==="

  mkdir -p "$TMP/bin" "$TMP/work" "$TMP/zdot"
  # 偽 claude: 引数を記録して眠るだけ。GUI 自身の問い合わせ（`--version` / `agents --json` /
  # 自動命名の `-p`）には答えるだけで記録しない
  cat > "$TMP/bin/claude" <<STUB
#!/bin/sh
case "\$1" in
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
  agents) echo "[]"; exit 0 ;;
  -p) exit 0 ;;
esac
printf '%s\n' "\$*" >> "$TMP/launches.log"
echo "FAKECLAUDE-STARTED"
exec sleep 600
STUB
  chmod +x "$TMP/bin/claude"
  printf '%s\n' 'PS1="[test@host:%1~]\$ "' > "$TMP/zdot/.zshrc"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="t2199-${arm}-$$"
  SOCKETS="$SOCKETS $TAKO_TMUX_SOCKET"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
env:
  PATH: "$TMP/bin:/usr/bin:/bin:/opt/homebrew/bin"
YAML
  "$TAKO_BIN" orchestrator projects add --key t2199 --cwd "$TMP/work" \
    --description "#2199 の検証（自動削除される）" >/dev/null 2>&1

  local legacy_env="TAKO_2199_LEGACY="
  [ "$arm" = legacy ] && legacy_env="TAKO_2199_LEGACY=1"
  launch_isolated_gui "$TMP/app.log" "$legacy_env" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" \
    "PATH=$TMP/bin:$PATH" || exit $?
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"; bad "隔離 GUI が起動しない"; return
  fi
  pass "隔離 tako-app が起動して CLI から見える（pid ${ISOLATED_GUI_PID}）"
  # 自動命名（claude -p）を呼ばせない（偽 claude が答えるが、ログインシェル経由の解決で本物を拾わないように）
  "$TAKO_BIN" autorename off >/dev/null 2>&1
  sleep 2
  MASTER="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"

  # 期待は「主/issues」。issues は書いた順（新）・昇順（旧）
  case_spawn "$arm" "#2192 usage S1" "前例は #322 と #1877。#2192 の §1.4 を読む" \
    "#2192/2192,322,1877" "#322/322,1877,2192" "1. ラベルの番号より小さい番号がプロンプトにある"
  case_spawn "$arm" "ledger-primary" "Issue #2199 を直す。前例は #322" \
    "#2199/2199,322" "#322/322,2199" "2. ラベルに番号が無い = プロンプトに書いた順で最初"
  case_spawn "$arm" "#2181 / #2157: ネット復帰" "#100 の作法に従う" \
    "#2181/2181,2157,100" "#100/100,2157,2181" "3. ラベルに番号が複数 = ラベルに書いた順で最初"
  case_spawn "$arm" "調査" "番号の無い依頼" \
    "null/-" "null/-" "4. どこにも番号が無い = 台帳の issue は付かない"

  stop_isolated_gui "$ISOLATED_GUI_PID"
  ISOLATED_GUI_PID=""
}

for arm in $ARMS; do
  run_arm "$arm"
done

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
