#!/usr/bin/env bash
# test-claude-mod-1879.sh — tako mod（Claude Code の mod 連携。#1879 / FR-2.42）の実経路テスト
#
# 隔離した data / discovery / tmux で**実 tako-app** を立て、ペインで**実 claude**（haiku の短いターン）を
# 動かして次を実測する（番号は Issue #1879 の受け入れ条件）:
#   0. 静的: `claude plugin validate --strict` / `claude plugin test`（本体は scripts/check-claude-mod.sh の
#      1 実装 = 夜間リリースと同じ検査。claude が無ければ「未実測」）
#   1. 直接ペインと tmux ペインの両方で、素の `claude` を打つだけで `tako mod` にそのペインの報告
#      （ctx・使用制限・turn・model）が 1 ターン以内に載る
#   2. 権限ダイアログ / AskUserQuestion の表示中に turn が permission / question になる
#   3. 版の下限未満（2.1.280 の実体）と `tako mod off` では注入されない。`--safe-mode` では報告が来ず
#      `tako mod` が理由を出す。tako の外の claude は mod を読まない
#   4. tako を止めた状態でも claude が普通に動き、transcript に mod 起因の行が出ない
#   5. tako の再起動をまたいで生き残った tmux の claude から報告が届き続ける
#   6. Claude Code の設定ファイルの更新時刻が前後で変わらない
#   7. A/B: `TAKO_1877_NO_MOD=1` で注入しない（同一バイナリで旧挙動。data dir にも何も足さない）
#   MCP: `tako_mod` は status を返し、report は受け付けない
#
# **本番の tako / 本番の claude 設定には触らない**: data / discovery / tmux は mktemp 配下と専用ソケット、
# 落とすのは自分で起こした pid だけ（pkill / killall は使わない）。claude は利用者のログインシェルの
# 設定 dir を**読むだけ**で使う（認証が要るため）。作業 dir は信頼済みの dir（既定は main の worktree）を
# 選び、信頼ダイアログを出さない。mod の読み込みで Claude Code が作る空の
# `<設定 dir>/plugins/data/tako-inline/` は、テスト前に無かったときだけ最後に消す。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 選んで回す: ONLY=static|direct|tmux|version|ab（既定は全部）。claude を動かさない段だけなら ONLY=static
# **CI には登録しない**（実 GUI と実 claude の認証が要る）。CI 側の担保は単体テストと番犬
# （`crates/tako-control/tests/issue1879_claude_mod_watchdog.rs`）。
#
# 使い方: bash scripts/test-claude-mod-1879.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）。
# 親が tako のペインなら mod の env も持っているので、それも落とす（隔離 GUI の子へ渡さない）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD
# claude の中から回すと「子セッション」の印を継ぎ、ペインの claude が transcript を保存しない
# （画面に `Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker`）。
# 受け入れ条件 4 は transcript を読むので外す（AGENTS.md の実 claude e2e と同じ）
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
# 値が入っているか / 部分文字列を含むか（bash 3.2 は `$(...)` の中の case を読めないので関数に置く）
present() {
  case "$1" in
    '' | null | absent) echo false ;;
    *) echo true ;;
  esac
}
contains() {
  case "$1" in
    *"$2"*) echo true ;;
    *) echo false ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1879-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1879-$$"
# ログの中のホーム・ユーザー名・ホスト名を伏せる（画面のダンプにプロンプトが写る。PR へ出さない）
MASK_USER="$(id -un)"
MASK_HOST="$(hostname -s 2>/dev/null || echo localhost)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${MASK_HOST}#<host>#g"; }

# 利用者の claude の設定 dir（ログインシェルが決める。読むだけ）
CCD="$("${SHELL:-/bin/zsh}" -l -c 'printf %s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"' 2>/dev/null)"
[ -n "$CCD" ] || CCD="$HOME/.claude"
INLINE_DATA="$CCD/plugins/data/tako-inline"
INLINE_EXISTED=0
[ -d "$INLINE_DATA" ] && INLINE_EXISTED=1
# claude を動かす作業 dir（信頼済みの dir。既定は main の worktree = 共有ツリー。読むだけ）
WORKDIR="${CLAUDE_WORKDIR:-$(git -C "$REPO_ROOT" worktree list --porcelain | awk 'NR == 1 { print $2 }')}"

# 後始末。終了コードの補正は exit-guard の番人（#1864）が受け持つ（印の無い 0 は 1）
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET"
  # テストが作らせた空 dir だけ消す（前からあったものには触らない）
  if [ "$INLINE_EXISTED" = 0 ] && [ -d "$INLINE_DATA" ]; then
    rmdir "$INLINE_DATA" 2>/dev/null || true
  fi
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "tako mod の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 隔離した環境（HOME は claude の認証のために本物のまま。tako 側の置き場だけ差し替える）-----
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"

start_app() {
  local name="$1"
  shift
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# --- 観測の道具 -----------------------------------------------------------------
mod_json() { "$TAKO_BIN" mod --json 2>/dev/null; }
# `tako mod --json` の 1 項目（ドット区切り。pane=N ならそのペインの行から）
mod_field() {
  mod_json | python3 -I -c '
import json, sys
path = sys.argv[1].split(".")
try:
    v = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for k in path:
    if k.startswith("pane="):
        want = int(k[5:])
        rows = v if isinstance(v, list) else v.get("panes", [])
        v = next((r for r in rows if r.get("pane") == want), None)
    elif isinstance(v, dict):
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
print("null" if v is None else (json.dumps(v) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
# 条件が成り立つまで 1 秒ごとに見る（上限秒）
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}
field_is() { [ "$(mod_field "$1")" = "$2" ]; }
# ペインのシェルの env を 1 行で読む（値そのものは出さず、mod の置き場を含むかだけ）
ENVCHECK="$TMP/envcheck.sh"
cat > "$ENVCHECK" <<'EOF'
case "${CLAUDE_CODE_PLUGIN_DIRS-}" in
  *claude-mod/tako*) dirs=mod ;;
  '') dirs=none ;;
  *) dirs=other ;;
esac
if [ -n "${TAKO_CLI-}" ]; then cli=set; else cli=none; fi
echo "$1 dirs=$dirs cli=$cli"
EOF
pane_env_state() {
  local pane="$1" tag="ENV1879-$RANDOM"
  "$TAKO_BIN" send --pane "$pane" "sh '$ENVCHECK' $tag" >/dev/null 2>&1
  local i
  for i in $(seq 1 20); do
    local line
    line="$("$TAKO_BIN" read --pane "$pane" 2>/dev/null | python3 -I -c '
import json, sys
raw = sys.stdin.read()
try:
    text = json.loads(raw).get("text", "")
except Exception:
    text = raw
tag = sys.argv[1]
hits = [l for l in text.splitlines() if l.startswith(tag + " ")]
print(hits[-1][len(tag) + 1:] if hits else "")' "$tag")"
    if [ -n "$line" ]; then echo "$line"; return 0; fi
    sleep 0.5
  done
  echo "unread"
}
# ペインのコマンド状態（シェル統合の idle / running。無ければ absent）
pane_state() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
want = int(sys.argv[1])
d = json.load(sys.stdin)
for tab in d.get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == want:
            print(p.get("state") or "absent"); sys.exit()
print("absent")' "$1"
}
shell_idle() { [ "$(pane_state "$1")" = idle ]; }
# claude を終わらせ、シェルのプロンプトへ戻るまで待つ（戻る前に打った字は claude が食う）
exit_claude() {
  ask "$1" "/exit"
  wait_for 30 shell_idle "$1" || true
  sleep 1
}
root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])'
}
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1879","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        if msg.get("error"):
            print("ERROR " + msg["error"].get("message", ""))
            sys.exit(1)
        res = msg.get("result", {})
        for c in res.get("content", []):
            if c.get("type") == "text":
                print(c["text"])
        if res.get("isError"):
            sys.exit(1)'
}
# claude をペインで起動し、mod の最初の報告（session.start の 1 秒後）を待つ
launch_claude() {
  local pane="$1"
  shift
  "$TAKO_BIN" send --pane "$pane" "cd '$WORKDIR' && claude $*" >/dev/null 2>&1
}
ask() {
  "$TAKO_BIN" send --pane "$1" --await-prompt "$2" >/dev/null 2>&1
}
# 設定ファイルの更新時刻（受け入れ条件 6。無いファイルは "-"）
settings_mtimes() {
  local f
  for f in "$CCD/settings.json" "$CCD/settings.local.json" "$HOME/.claude/settings.json" \
    "$HOME/.claude/settings.local.json" "$WORKDIR/.claude/settings.json" "$WORKDIR/.claude/settings.local.json"; do
    if [ -f "$f" ]; then printf '%s %s\n' "$(stat -f %m "$f")" "$f"; else printf '%s %s\n' - "$f"; fi
  done | mask
}
claude_available() { command -v claude >/dev/null 2>&1; }

MTIMES_BEFORE="$(settings_mtimes)"

# --- 0. 静的（validate / test）-------------------------------------------------
# 検査の本体は scripts/check-claude-mod.sh（夜間リリースと同じ 1 実装。#1903）。使い捨ての設定 dir と
# mod の写しで回すので、本番の設定にもソースツリーにも何も書かない。終了コード 0 = 合格 / 3 = 未実測
phase_static() {
  echo "== 0. mod の validate --strict / test（本体は check-claude-mod.sh）"
  local out rc=0 before after
  # 作業中の変更があっても測れるよう、段の前後の git status を比べる
  before="$(git -C "$REPO_ROOT" status --porcelain --untracked-files=all -- crates/tako-core/claude-mod)"
  out="$(/bin/bash "$REPO_ROOT/scripts/check-claude-mod.sh" 2>&1)" || rc=$?
  printf '%s\n' "$out" | mask | sed 's/^/    | /'
  case "$rc" in
    0) pass "validate --strict / test（$(printf '%s\n' "$out" | sed -n 's/^結果: //p' | mask)）" ;;
    3) unmeasured "claude が無いので validate / test を飛ばした" ;;
    *) fail "validate --strict / test（exit ${rc}）" ;;
  esac
  after="$(git -C "$REPO_ROOT" status --porcelain --untracked-files=all -- crates/tako-core/claude-mod)"
  check_eq "validate / test がソースツリーを汚さない（段の前後の git status）" "$before" "$after"
}

# --- 1〜3. 直接ペイン ------------------------------------------------------------
phase_direct() {
  echo "== 1〜3. 直接ペイン（TAKO_PERSIST=0）"
  start_app direct TAKO_PERSIST=0
  check_eq "展開した（install）" "written" "$(mod_field install)"
  check_eq "新しいペインへ注入する（injecting）" "true" "$(mod_field injecting)"
  check_eq "claude の版を控えた" "true" "$( [ "$(mod_field claude_version)" != null ] && echo true || echo false)"
  local p
  p="$(root_pane)"
  check_eq "直接ペインの env に mod の置き場と CLI が入る" "dirs=mod cli=set" "$(pane_env_state "$p")"
  if ! claude_available; then
    unmeasured "claude が無いので直接ペインの実 claude の段を飛ばした"
    stop_app
    return
  fi
  # 利用者の既定の権限モードが auto / bypass だとダイアログが出ないので default に固定し、
  # Bash を必ず聞く設定を足す（どちらもこのセッションだけ。設定ファイルは書かない）
  launch_claude "$p" --model haiku --permission-mode default --settings "'{\"permissions\":{\"ask\":[\"Bash\"]}}'"
  if wait_for 60 field_is "panes.pane=$p.state" reporting; then pass "claude の起動で報告が届く（state=reporting）"; else fail "claude の起動で報告が届かない（state=$(mod_field "panes.pane=$p.state")）"; fi
  ask "$p" "Reply with exactly one word: ok"
  if wait_for 90 field_is "panes.pane=$p.report.last_turn.reason" answer; then pass "1 ターン後に last_turn が載る"; else fail "1 ターン後の報告が来ない"; fi
  check_eq "turn=idle" "idle" "$(mod_field "panes.pane=$p.report.turn")"
  local pct limits model
  pct="$(mod_field "panes.pane=$p.report.context.percent")"
  limits="$(mod_field "panes.pane=$p.report.rate_limits.0.kind")"
  model="$(mod_field "panes.pane=$p.report.model")"
  check_eq "ctx% が載る" "true" "$(present "$pct")"
  check_eq "使用制限が載る" "true" "$(present "$limits")"
  check_eq "model が載る" "true" "$(contains "$model" haiku)"
  echo "    （ctx ${pct}% / ${limits} / ${model}）"
  # 2. 権限ダイアログ
  ask "$p" "Use the Bash tool to run exactly: echo tako1879"
  if wait_for 60 field_is "panes.pane=$p.report.turn" permission; then
    pass "権限ダイアログの表示中は turn=permission"
  else
    fail "権限ダイアログで turn が permission にならない（$(mod_field "panes.pane=$p.report.turn")）"
    "$TAKO_BIN" read --pane "$p" --lines 15 2>/dev/null | mask | sed 's/^/      | /'
  fi
  check_eq "pending_tool=Bash" "Bash" "$(mod_field "panes.pane=$p.report.pending_tool")"
  "$TAKO_BIN" orchestrator respond --pane "$p" --choice no >/dev/null 2>&1
  wait_for 60 field_is "panes.pane=$p.report.turn" idle || true
  # 2. AskUserQuestion
  ask "$p" "Call the AskUserQuestion tool right now (do not answer in plain text) to ask me which color I prefer, with exactly two options: red and blue."
  if wait_for 60 field_is "panes.pane=$p.report.turn" question; then
    pass "AskUserQuestion の表示中は turn=question"
  else
    fail "AskUserQuestion で turn が question にならない（$(mod_field "panes.pane=$p.report.turn")）"
    "$TAKO_BIN" read --pane "$p" --lines 15 2>/dev/null | mask | sed 's/^/      | /'
  fi
  check_eq "pending_tool=AskUserQuestion" "AskUserQuestion" "$(mod_field "panes.pane=$p.report.pending_tool")"
  "$TAKO_BIN" orchestrator respond --pane "$p" --choice 1 >/dev/null 2>&1
  if wait_for 90 field_is "panes.pane=$p.report.turn" idle; then pass "回答後は idle へ戻る"; else fail "回答後に idle へ戻らない（$(mod_field "panes.pane=$p.report.turn")）"; fi
  # MCP（status は返り、report は断る）
  local mcp
  mcp="$(mcp_call tako_mod '{}' | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(any(r.get("state")=="reporting" for r in d.get("panes",[])))' 2>/dev/null)"
  check_eq "MCP tako_mod（status）にも同じ報告が出る" "True" "$mcp"
  mcp="$(mcp_call tako_mod '{"action":"report"}' 2>&1 || true)"
  check_eq "MCP tako_mod は report を受け付けない" "true" "$(contains "$mcp" "report は MCP に無い")"
  # session.end（/exit）で報告を捨てる
  exit_claude "$p"
  if wait_for 30 field_is "panes.pane=$p.state" waiting; then pass "/exit（session.end）で報告が消える"; else fail "/exit 後も報告が残る（state=$(mod_field "panes.pane=$p.state")）"; fi
  # 3. --safe-mode では mod が読まれず、理由を出す
  launch_claude "$p" --safe-mode --model haiku
  if wait_for 60 field_is "panes.pane=$p.state" no_report; then
    pass "--safe-mode では報告が来ず state=no_report"
  else
    fail "--safe-mode の行が no_report にならない（state=$(mod_field "panes.pane=$p.state")）"
    "$TAKO_BIN" read --pane "$p" --lines 15 2>/dev/null | mask | sed 's/^/      | /'
  fi
  check_eq "--safe-mode の理由（mod_absent）" "mod_absent" "$(mod_field "panes.pane=$p.reason.code")"
  "$TAKO_BIN" mod 2>/dev/null | mask | sed 's/^/    /'
  exit_claude "$p"
  # 3. tako mod off では注入しない（次に作るペインから）
  "$TAKO_BIN" mod off >/dev/null 2>&1
  local q
  q="$("$TAKO_BIN" split --pane "$p" --down 2>/dev/null | tr -dc '0-9')"
  check_eq "off のペインの env に mod が入らない" "dirs=none cli=none" "$(pane_env_state "$q")"
  check_eq "off のペインの理由（disabled）" "disabled" "$(mod_field "panes.pane=$q.reason.code")"
  "$TAKO_BIN" mod on >/dev/null 2>&1
  check_eq "on へ戻せる" "true" "$(mod_field injecting)"
  stop_app
}

# --- 1 / 4 / 5. tmux ペイン --------------------------------------------------------
phase_tmux() {
  echo "== 1 / 4 / 5. tmux ペイン（TAKO_PERSIST=1）"
  start_app tmux1 TAKO_PERSIST=1
  local p
  p="$(root_pane)"
  check_eq "tmux ペインの env に mod の置き場と CLI が入る" "dirs=mod cli=set" "$(pane_env_state "$p")"
  if ! claude_available; then
    unmeasured "claude が無いので tmux ペインの実 claude の段を飛ばした"
    stop_app
    return
  fi
  launch_claude "$p" --model haiku
  if wait_for 60 field_is "panes.pane=$p.state" reporting; then pass "tmux ペインでも claude の起動で報告が届く"; else fail "tmux ペインで報告が届かない（state=$(mod_field "panes.pane=$p.state")）"; fi
  ask "$p" "Reply with exactly one word: ok"
  if wait_for 90 field_is "panes.pane=$p.report.last_turn.reason" answer; then pass "tmux ペインで 1 ターン後に ctx / 使用制限が載る（ctx $(mod_field "panes.pane=$p.report.context.percent")%）"; else fail "tmux ペインで 1 ターン後の報告が来ない"; fi
  local sid session
  sid="$(mod_field "panes.pane=$p.report.session_id")"
  session="$(tmux -L "$TMUX_SOCKET" list-sessions -F '#{session_name}' 2>/dev/null | head -1)"
  # 4. tako を止めても claude は動く（tmux の中で生き残る）
  stop_app
  # 対象はペインの書式（`=セッション名:`）。`=名前` だけだと capture-pane が見つけられない
  tmux -L "$TMUX_SOCKET" send-keys -t "=${session}:" "Reply with exactly one word: pong" 2>/dev/null
  sleep 1
  tmux -L "$TMUX_SOCKET" send-keys -t "=${session}:" Enter 2>/dev/null
  local seen=no i
  for i in $(seq 1 90); do
    if tmux -L "$TMUX_SOCKET" capture-pane -p -t "=${session}:" 2>/dev/null | grep -q -E '(⏺|●) pong'; then seen=yes; break; fi
    sleep 1
  done
  check_eq "tako を止めた状態でも claude が応答する" "yes" "$seen"
  local transcript
  transcript="$(find "$CCD/projects" -name "${sid}.jsonl" 2>/dev/null | head -1)"
  if [ -n "$transcript" ]; then
    # mod の読み込み失敗の文言（2.1.280 で実測した形）・mod のログの接頭辞・mod の名前。
    # 「hooks module」単体は組み込みスキルの説明文にも出るので使わない
    check_eq "transcript に mod 起因の行が無い" "0" "$(grep -c -E 'hooks module did not load|tako mod: |tako@inline' "$transcript" | tr -d ' ')"
  else
    fail "transcript（session_id から）が見つからない"
  fi
  # 5. tako を立て直すと、生き残った claude の報告が新しいインスタンスへ届く（CLI のフォールバック）
  start_app tmux2 TAKO_PERSIST=1
  local found=""
  for i in $(seq 1 45); do
    found="$(mod_json | python3 -I -c '
import json, sys
sid = sys.argv[1]
d = json.load(sys.stdin)
for r in d.get("panes", []):
    if r.get("state") == "reporting" and (r.get("report") or {}).get("session_id") == sid:
        print(r["pane"])
        break' "$sid" 2>/dev/null)"
    [ -n "$found" ] && break
    sleep 1
  done
  if [ -n "$found" ]; then pass "再起動をまたいで生き残った claude の報告が届く（pane ${found}）"; else fail "再起動後に生き残った claude の報告が届かない"; fi
  local np
  np="$(root_pane)"
  exit_claude "$np"
  stop_app
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
}

# --- 3. 版の下限未満（2.1.280 の実体）--------------------------------------------
phase_version() {
  echo "== 3. 版の下限未満"
  local old
  old="$(ls -d "$HOME/.local/share/claude/versions/2.1.280" 2>/dev/null)"
  if [ -z "$old" ]; then
    unmeasured "2.1.280 の実体が無いので下限未満の段を飛ばした"
    return
  fi
  mkdir -p "$TMP/oldbin"
  ln -sf "$old" "$TMP/oldbin/claude"
  start_app version TAKO_PERSIST=0 "PATH=$TMP/oldbin:$PATH"
  check_eq "下限未満の claude の版を控えた" "2.1.280" "$(mod_field claude_version)"
  check_eq "下限未満では注入しない（理由 claude_too_old）" "claude_too_old" "$(mod_field reason.code)"
  check_eq "下限未満のペインの env に mod が入らない" "dirs=none cli=none" "$(pane_env_state "$(root_pane)")"
  stop_app
}

# --- 7. A/B ------------------------------------------------------------------
phase_ab() {
  echo "== 7. A/B（TAKO_1877_NO_MOD=1）"
  local data="$TMP/data-ab"
  mkdir -p "$data"
  TAKO_DATA_DIR="$data" start_app ab TAKO_PERSIST=0 TAKO_1877_NO_MOD=1 "TAKO_DATA_DIR=$data"
  check_eq "A/B では注入しない（理由 ab_off）" "ab_off" "$(mod_field reason.code)"
  check_eq "A/B のペインの env に mod が入らない" "dirs=none cli=none" "$(pane_env_state "$(root_pane)")"
  check_eq "A/B では data dir に mod を展開しない" "absent" "$( [ -e "$data/claude-mod" ] && echo present || echo absent)"
  if claude_available; then
    # 旧挙動の側で実 claude を起動しても報告は来ない（同じバイナリの before）
    local p
    p="$(root_pane)"
    launch_claude "$p" --model haiku
    sleep 20
    check_eq "A/B の claude からは報告が来ない（state=not_injected のまま）" "not_injected" "$(mod_field "panes.pane=$p.state")"
    exit_claude "$p"
  else
    unmeasured "claude が無いので A/B の実 claude の段を飛ばした"
  fi
  stop_app
}

want static && phase_static
want direct && phase_direct
want tmux && phase_tmux
want version && phase_version
want ab && phase_ab

# --- 3. tako の外の claude / 6. 設定ファイル ------------------------------------------
echo "== 3 / 6. tako の外・設定ファイル"
if claude_available; then
  out="$(cd "$WORKDIR" && claude plugin list 2>/dev/null | grep -c 'tako@inline' | tr -d ' ')"
  check_eq "tako の外の claude は mod を読まない（plugin list に tako@inline が無い）" "0" "$out"
else
  unmeasured "claude が無いので tako の外の段を飛ばした"
fi
MTIMES_AFTER="$(settings_mtimes)"
check_eq "Claude Code の設定ファイルの更新時刻が変わらない" "$MTIMES_BEFORE" "$MTIMES_AFTER"

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
