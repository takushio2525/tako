#!/usr/bin/env bash
# #1510 の実経路テスト: 初回起動の導線（自動の段 → 次にやること → ボタン 1 つの同意で導入・ログイン →
# master を起動）を、**隔離した HOME / TAKO_DATA_DIR / PATH** で通す。
#
# 前半（CLI）: 自動の段の answers（`install: false`）なら、`tako setup --yes` でも
#   エージェント CLI の公式インストーラ（curl）も依存の導入（brew）も**走らない**こと。
#   A/B として、省略（MCP `tako_setup` と同じ）なら同じ砂場で curl / brew が呼ばれること。
# 後半（GUI）: 隔離 GUI（`TAKO_FIRST_RUN=1`）を初回起動し、
#   ① 自動の段が走って「次にやること = 入れる」になる（初回）
#   ② `tako welcome next`（= バナーのボタン）で導入のタブが開き、偽のインストーラが偽の claude を
#      置く → 「次にやること = ログイン」へ進む（未認証）
#   ③ もう一度 next でログインのコマンドが同じタブに打たれ → 「master を起動」になる（完了）
#   ④ 再起動しても自動の段は繰り返さず、バナーは master を出す。master を押すと導線が終わり、
#      次の起動ではバナーが出ない
#   ほかに: オフライン（取得に失敗しても導線は止まらず「入れる」のまま）/ 全部済んでいる人の初回起動 /
#   自動の段の途中で GUI を閉じる（次の起動で残りから再開）
#
# 本番の ~/Library/Application Support/tako・~/.claude・~/.zprofile には一切触れない
# （HOME / data dir は一時の置き場・PATH は偽物だけ・ネットワークのインストーラは偽物）。
# 窓は tako-vd へ出す（#1141）。蓋閉じ・画面ロック中はフレームが組まれないが、IPC と導線は動く。
# 落とすのは自分で起こしたプロセスだけ。
#
# 使い方: bash scripts/test-first-run-1510.sh   （LOG_DIR=<dir> で各状態の JSON を残す）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1510-XXXXXX")"
APP_PID=""
PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
ng() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }

cleanup() {
  stop_isolated_gui "$APP_PID"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "初回起動の導線の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

STUBS="$TMP/stubs"
CALLS="$TMP/network-calls.log"
OFFLINE="$TMP/offline.flag"
mkdir -p "$STUBS"
: > "$CALLS"

# --- 偽物 -----------------------------------------------------------------------
# ログインシェル（exe::find の `$SHELL -l -c command -v …`）を砂場の PATH に閉じ込める
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$STUBS/isosh"

# 偽の claude（インストーラが ~/.local/bin へ置く）。auth login は印を置くだけ = ブラウザは開かない
cat > "$TMP/fake-claude" <<'EOF'
#!/bin/sh
MARK="$HOME/.fake-claude-authed"
case "${1:-} ${2:-}" in
  "auth status")
    if [ -f "$MARK" ]; then
      echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max"}'
    else
      echo '{"loggedIn":false,"authMethod":"none"}'
    fi
    exit 0;;
  "auth login")
    echo "Opening browser to sign in... (stub)"
    : > "$MARK"
    echo "Login successful. (stub)"
    exit 0;;
esac
if [ "${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ]; then
  shift
  sub="${1:-}"; [ $# -gt 0 ] && shift
  case "$sub" in
    list)
      /usr/bin/python3 -c '
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: d={}
s=d.get("mcpServers") or {}
if not s: print("No MCP servers configured")
for k,v in s.items():
    print("%s: %s - connected" % (k, " ".join([v.get("command","")]+list(v.get("args") or []))))
' "$HOME/.claude.json"
      exit 0;;
    add)
      name=""; cmd=""
      while [ $# -gt 0 ]; do
        case "$1" in
          --scope|--transport) shift 2;;
          --) shift; cmd="$*"; break;;
          *) name="$1"; shift;;
        esac
      done
      /usr/bin/python3 -c '
import json,sys
p,name,cmd=sys.argv[1],sys.argv[2],sys.argv[3].split(" ")
try: d=json.load(open(p))
except Exception: d={}
d.setdefault("mcpServers",{})[name]={"type":"stdio","command":cmd[0],"args":cmd[1:],"env":{}}
json.dump(d,open(p,"w"),ensure_ascii=False)
' "$HOME/.claude.json" "$name" "$cmd"
      echo "Added stdio MCP server $name"
      exit 0;;
  esac
  exit 0
fi
exit 0
EOF
chmod +x "$TMP/fake-claude"

# 偽の curl: 呼ばれたことを記録し、`-o <dest>` へ偽のインストーラを書く（オフラインの印があれば失敗）
cat > "$STUBS/curl" <<EOF
#!/bin/sh
echo "curl \$*" >> "$CALLS"
if [ -f "$OFFLINE" ]; then echo "curl: (6) Could not resolve host (stub)" >&2; exit 6; fi
dest=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) dest="\$2"; shift 2;; *) shift;; esac
done
[ -n "\$dest" ] || exit 2
cat > "\$dest" <<'EOS'
#!/bin/bash
mkdir -p "\$HOME/.local/bin"
cp "$TMP/fake-claude" "\$HOME/.local/bin/claude"
chmod +x "\$HOME/.local/bin/claude"
echo "Claude Code successfully installed! (stub)"
EOS
exit 0
EOF
# 偽の brew: 呼ばれたことを記録するだけ（依存の導入 = ネットワーク）
printf '#!/bin/sh\necho "brew $*" >> "%s"\nexit 0\n' "$CALLS" > "$STUBS/brew"
chmod +x "$STUBS"/*

# 砂場の env（GUI と CLI で同じ）。`~/.local/bin` は最初から PATH に入れておく
# （PATH の段の検証はこのスクリプトの対象外 = 単体テストと scripts/test-setup-continue-1501.sh）。
# tmux / tailscale の検出は /opt/homebrew/bin などの既知の置き場を直に見るので、未作成のパスへ
# 向けて「無い」を作る（#1499 の実経路テストと同じ）。こうしないと依存の導入（brew）が
# そもそも起きず、「brew は呼ばれない」が素通りの PASS になる
sandbox_env() {
  echo "HOME=$TMP/home" "SHELL=$STUBS/isosh" "CODEX_HOME=$TMP/home/.codex" \
    "PATH=$TMP/home/.local/bin:$STUBS:/usr/bin:/bin:/usr/sbin:/sbin" "TAKO_ISOLATED=1" \
    "TAKO_TMUX_BIN=$STUBS/tmux" "TAKO_TAILSCALE_BIN=$STUBS/tailscale" "TAKO_TAILSCALE_SOCKET=" \
    "TAKO_LANG=ja" "TERM=dumb"
}
tk() {
  # shellcheck disable=SC2046
  env -u TAKO_SOCKET -u TAKO_PANE_ID $(sandbox_env) "$TAKO_BIN" "$@"
}
# JSON の 1 項目（a.b.c）を読む
jget() {
  /usr/bin/python3 -c '
import json,sys
v=json.loads(sys.argv[1])
for k in sys.argv[2].split("."):
    v = v.get(k) if isinstance(v, dict) else None
print("null" if v is None else (json.dumps(v) if isinstance(v,(dict,list)) else str(v).lower() if isinstance(v,bool) else v))
' "$1" "$2" 2>/dev/null
}
fresh_home() {
  rm -rf "${TMP:?}/home"
  mkdir -p "$TMP/home"
  rm -f "${OFFLINE:?}"
  : > "$CALLS"
}
calls() { grep -c "^$1 " "$CALLS" 2>/dev/null || true; }
save() { [ -n "${LOG_DIR:-}" ] && mkdir -p "$LOG_DIR" && printf '%s\n' "$2" > "$LOG_DIR/$1.json"; }

# ================================================================================
echo "== A. CLI: 自動の段の answers はネットワークの導入を走らせない =="
fresh_home
export TAKO_DATA_DIR="$TMP/cli/data" TAKO_ORCHESTRATOR_DIR="$TMP/cli/orch"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"
printf '%s' '{"install":false,"launch_agent":"none"}' | tk setup --yes --answers - > "$TMP/a1.log" 2>&1
check "A1: install:false の setup は exit 0" "$?" "0"
check "A1: curl（公式インストーラ）は呼ばれない" "$(calls curl)" "0"
check "A1: brew（依存の導入）は呼ばれない" "$(calls brew)" "0"
grep -q "初回起動の自動実行では導入しません" "$TMP/a1.log" && ok "A1: 導入しなかった理由をその場で出す" || ng "A1: 理由の行が無い"
[ -f "$TMP/home/.claude/CLAUDE.md" ] && ok "A1: 副作用の小さい段（指示ファイル）は走る" || ng "A1: 指示ファイルが無い"
grep -q "Claude Code の導入" "$TMP/a1.log" && ok "A1: 導入は残り作業として最後に出る" || ng "A1: 残り作業に導入が無い"
printf '%s' '{"install":false,"launch_agent":"none"}' | tk setup --yes --answers - > "$TMP/a2.log" 2>&1
grep -q "セットアップ結果: 変更なし" "$TMP/a2.log" && ok "A2: 2 回目は変更なし（冪等）" || ng "A2: 2 回目に変更が出た"
check "A2: 2 回目も curl / brew は呼ばれない" "$(calls curl)/$(calls brew)" "0/0"
# A/B: 省略（MCP tako_setup・ターミナルの --yes）は同意扱いで取りに行く = 印が効いていること
fresh_home
touch "$OFFLINE"
rm -rf "${TMP:?}/cli"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"
printf '%s' '{"launch_agent":"none"}' | tk setup --yes --answers - > "$TMP/a3.log" 2>&1
[ "$(calls curl)" -ge 1 ] && ok "A3（A/B）: 省略なら curl が呼ばれる（印が無いと取りに行く）" || ng "A3: 省略でも curl が呼ばれていない"
[ "$(calls brew)" -ge 1 ] && ok "A3（A/B）: 省略なら brew も呼ばれる" || ng "A3: 省略でも brew が呼ばれていない"
unset TAKO_DATA_DIR TAKO_ORCHESTRATOR_DIR

# ================================================================================
start_app() {
  local name="$1"
  shift
  export TAKO_DATA_DIR="$TMP/$name/data"
  export TAKO_DISCOVERY_DIR="$TMP/$name/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$name/orch"
  export TAKO_SESSIONS_FILE="$TMP/$name/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$name/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$name/workers.yaml"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_PERSIST=0 \
    TAKO_FIRST_RUN=1 ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
# `tako welcome` を叩き、条件（python の式。s = 応答）が真になるまで待つ。最後の応答を $LAST へ
LAST=""
wait_welcome() {
  local cond="$1" limit="${2:-120}" i=0
  while [ "$i" -lt "$limit" ]; do
    LAST="$(tk welcome 2>/dev/null)"
    if /usr/bin/python3 -c "
import json,sys
s=json.loads(sys.argv[1])
o=s.get('onboarding') or {}
st=o.get('state') or {}
step=st.get('step') or {}
auto=st.get('auto') or {}
sys.exit(0 if ($cond) else 1)
" "$LAST" 2>/dev/null; then
      return 0
    fi
    sleep 2
    i=$((i + 2))
  done
  return 1
}

echo "== B. GUI: 初回起動 → 入れる → ログイン → master =="
fresh_home
start_app main
wait_welcome "auto.get('state')=='done' and step.get('kind')=='install'" 180 \
  && ok "B1（初回）: 自動の段が走り、次にやること = 入れる" || ng "B1: 初回の状態にならない: $LAST"
save 1-first-run "$LAST"
check "B1: バナーが出ている" "$(jget "$LAST" visible)" "true"
check "B1: 導線が始まった印（welcome_onboarding）" "$(jget "$LAST" onboarding.active)" "true"
check "B1: 自動の段は完走した" "$(jget "$LAST" onboarding.state.auto.completed)" "true"
check "B1: 最簡コマンドは tako setup bootstrap install" "$(jget "$LAST" onboarding.state.step.command)" "tako setup bootstrap install"
check "B1: 自動の段はネットワークへ出ていない（curl / brew）" "$(calls curl)/$(calls brew)" "0/0"
[ -f "$TMP/home/.claude/CLAUDE.md" ] && ok "B1: 自動の段が指示ファイルを作った" || ng "B1: 指示ファイルが無い"
[ -f "$TAKO_DATA_DIR/setup/first-run.log" ] && ok "B1: 自動の段の全文が first-run.log に残る" || ng "B1: first-run.log が無い"

NEXT="$(tk welcome next 2>&1)"
check "B2: next は入れる段を押した" "$(jget "$NEXT" launched.kind)" "install"
wait_welcome "step.get('kind')=='login'" 120 \
  && ok "B2（未認証）: 導入が終わり、次にやること = ログイン" || ng "B2: ログインの段にならない: $LAST"
save 2-unauthenticated "$LAST"
check "B2: 公式インストーラの取得はボタンの後に 1 回だけ" "$(calls curl)" "1"
[ -x "$TMP/home/.local/bin/claude" ] && ok "B2: 偽のインストーラが claude を置いた" || ng "B2: claude が置かれていない"
check "B2: 最簡コマンドは claude auth login" "$(jget "$LAST" onboarding.state.step.command)" "claude auth login"
# 入った直後に自動の段を当て直す（MCP の登録）
wait_welcome "auto.get('state')=='done'" 60 >/dev/null
/usr/bin/python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if "tako" in (d.get("mcpServers") or {}) else 1)' \
  "$TMP/home/.claude.json" 2>/dev/null && ok "B2: 入った後の自動の段で tako の MCP が登録された" || ng "B2: MCP が登録されていない"

NEXT="$(tk welcome next 2>&1)"
check "B3: next はログインの段を押した" "$(jget "$NEXT" launched.kind)" "login"
wait_welcome "step.get('kind')=='master'" 120 \
  && ok "B3（完了）: ログインが済み、次にやること = master を起動" || ng "B3: master の段にならない: $LAST"
save 3-ready "$LAST"
[ -f "$TMP/home/.fake-claude-authed" ] && ok "B3: ログインのコマンドがタブで走った" || ng "B3: ログインの印が無い"
LIST="$(tk list 2>/dev/null)"
TABS="$(/usr/bin/python3 -c 'import json,sys; print(" ".join(t.get("title","") for t in json.loads(sys.argv[1]).get("tabs",[])))' "$LIST" 2>/dev/null)"
echo "  （タブ: ${TABS}）"
case "$TABS" in *login*) ng "B3: ログインは導入のタブで続けるはずが新しいタブを開いた" ;; *) ok "B3: ログインは導入のタブで続けた（タブを増やさない）" ;; esac
check "B3: ボタンの後の取得は増えていない" "$(calls curl)" "1"
stop_app

echo "== B4. 再起動: 自動の段を繰り返さず master を出し、押すと導線が終わる =="
export TAKO_DATA_DIR="$TMP/main/data"
# shellcheck disable=SC2046
launch_isolated_gui "$TMP/app-main2.log" $(sandbox_env) TAKO_PERSIST=0 TAKO_FIRST_RUN=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-main2.log" || exit 1
wait_welcome "step.get('kind')=='master'" 60 && ok "B4: 2 回目の起動でもバナーは master を出す" || ng "B4: $LAST"
check "B4: 2 回目の起動では自動の段を繰り返さない" "$(jget "$LAST" onboarding.state.auto.state)" "idle"
check "B4: バナーが出ている" "$(jget "$LAST" visible)" "true"
NEXT="$(tk welcome next 2>&1)"
check "B4: next は master を押した" "$(jget "$NEXT" launched.kind)" "master"
wait_welcome "s.get('visible') is False" 30 >/dev/null
check "B4: master を押すとバナーが閉じる" "$(jget "$LAST" visible)" "false"
check "B4: master を押すと導線が終わる" "$(jget "$LAST" onboarding.active)" "false"
stop_app
# shellcheck disable=SC2046
launch_isolated_gui "$TMP/app-main3.log" $(sandbox_env) TAKO_PERSIST=0 TAKO_FIRST_RUN=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-main3.log" || exit 1
# 起動直後の 1 回は前のプロセスの終わりと重なることがあるので、読めるまで読み直す
wait_welcome "'visible' in s" 30 >/dev/null
check "B5: 導線を終えた後の起動ではバナーが出ない" "$(jget "$LAST" visible)" "false"
check "B5: 導線は終わったまま" "$(jget "$LAST" onboarding.active)" "false"
stop_app

echo "== E1. オフライン: 取得に失敗しても導線は止まらず「入れる」のまま =="
fresh_home
touch "$OFFLINE"
start_app offline
wait_welcome "auto.get('state')=='done' and step.get('kind')=='install'" 180 && ok "E1: 初回の状態" || ng "E1: $LAST"
tk welcome next > /dev/null 2>&1
# ボタンの後のタブで取得が失敗するまで待つ（取得の記録が出る）
i=0
while [ "$(calls curl)" -lt 1 ] && [ "$i" -lt 60 ]; do sleep 1; i=$((i + 1)); done
sleep 6
wait_welcome "'visible' in s" 30 >/dev/null
check "E1: 取得に失敗しても次にやることは入れるのまま" "$(jget "$LAST" onboarding.state.step.kind)" "install"
check "E1: バナーは出たまま" "$(jget "$LAST" visible)" "true"
[ "$(calls curl)" -ge 1 ] && ok "E1: ボタンの後に取得を試みた" || ng "E1: 取得を試みていない"
stop_app

echo "== E2. 全部済んでいる人の初回起動: そのまま master を出す =="
fresh_home
mkdir -p "$TMP/home/.local/bin"
cp "$TMP/fake-claude" "$TMP/home/.local/bin/claude"
: > "$TMP/home/.fake-claude-authed"
start_app done
wait_welcome "auto.get('state')=='done' and step.get('kind')=='master'" 180 \
  && ok "E2: 自動の段の後、次にやること = master を起動" || ng "E2: $LAST"
save e2-already-done "$LAST"
check "E2: ネットワークへは出ない" "$(calls curl)/$(calls brew)" "0/0"
stop_app

echo "== E3. 自動の段の途中で GUI を閉じる: 次の起動で残りから再開する =="
fresh_home
# 自動の段を長引かせる（tailscale の検出は上限つきで待つ）
printf '#!/bin/sh\nsleep 30\n' > "$STUBS/tailscale"
chmod +x "$STUBS/tailscale"
start_app interrupted
sleep 3
LAST="$(tk welcome 2>/dev/null)"
check "E3: 閉じる時点で自動の段は走っている" "$(jget "$LAST" onboarding.state.auto.state)" "running"
kill -KILL "$APP_PID" 2>/dev/null
APP_PID=""
sleep 1
rm -f "${STUBS:?}/tailscale"
export TAKO_DATA_DIR="$TMP/interrupted/data"
# shellcheck disable=SC2046
launch_isolated_gui "$TMP/app-interrupted2.log" $(sandbox_env) TAKO_PERSIST=0 TAKO_FIRST_RUN=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-interrupted2.log" || exit 1
wait_welcome "step.get('kind')=='install'" 180 && ok "E3: 次の起動でバナーが出て次にやることまで進む" || ng "E3: $LAST"
check "E3: バナーが出ている" "$(jget "$LAST" visible)" "true"
# 前のプロセスの子（自動の段）が終わるのを待ってから片付ける（上限つきの検出で必ず終わる）
sleep 12
stop_app

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
