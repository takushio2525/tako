#!/usr/bin/env bash
# #2879 の実経路テスト: 初回起動の導線（ボタン → setup → 対話 → master）を、**隔離した HOME /
# TAKO_DATA_DIR / PATH** の隔離 GUI で通す。11/10 の利用者の指示「初回起動で自動実行しない。ボタンを
# 押してセットアップと対話（master / worker の設定や GitHub は対話でないと決められない）」の受け入れ。
#
#   A. claude が無い砂場:
#      ① 起動しただけでは何も走らない・書かない（HOME のファイル・data dir の setup / orchestrator /
#         settings.json が増えない・curl / brew は 0 回）。次にやること = setup
#      ② `tako welcome next`（= バナーの「セットアップを実行」）→ setup のタブで tako setup が走り、
#         段は running（setup）。導入の [Y/n] に Y → 偽のインストーラが未ログインの偽 claude を置く
#      ③ setup が「ログインが残り」で終わる → 段 = login → next でログイン → 段 = continue
#      ④ next（= 「セットアップを続ける」）→ 同じタブで tako setup → 対話アシスタント（偽 claude）が
#         初回フローの合図つきで開き、段 = running（dialogue）
#      ⑤ 対話を閉じる → 段 = master。再起動してもバナーは master を出し、自動の段は走らない
#   B. claude がログイン済みの砂場: setup → そのまま対話 → master
#   C. 既存の利用者（settings.json あり・#1510 の印が立ったまま）: バナーは出ない
#   D. A/B（TAKO_2879_LEGACY=1）: #1510 の自動の段が走る（判断が逆向きにも効いていること）
#
# 本番の ~/Library/Application Support/tako・~/.claude・~/.zprofile には一切触れない（HOME / data dir は
# 一時の置き場・PATH は偽物とこのテストの tako だけ・ネットワークのインストーラは偽物）。窓は tako-vd へ出す
# （#1141）。ボタンが打つ tako は GUI と同じ版（実行ファイルの隣）なので、GUI と CLI を一時の置き場へ並べて
# 写して起こす（このテストの版の setup を打たせる。#2879）。「master を起動」は従来どおり安定版
# （`/Applications/tako.app`）を先に選ぶので、本番の tako が入っている機では押さない（状態だけを見る）。
#
# 使い方: bash scripts/test-first-run-2879.sh
#   SHOT_CMD=<コマンド>  各状態で `<コマンド> <名前> <GUI の pid>` を呼ぶ（スクリーンショットを撮る外の道具用）
#   KEEP=1               一時の置き場を残す
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-2879-XXXXXX")"
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
tako_exit_trap cleanup "初回起動の導線（#2879）の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"
isolated_gui_bins || exit 1
command -v perl > /dev/null || { echo "perl が無い環境では走らない"; exit 1; }

# GUI と CLI を並べて写す（ボタンが打つのは GUI の隣の CLI = このテストの版。#2879）
BIN="$TMP/bin"
STUBS="$TMP/stubs"
CALLS="$TMP/network-calls.log"
mkdir -p "$BIN" "$STUBS"
/bin/cp -f "$TAKO_BIN" "$BIN/tako"
/bin/cp -f "$APP_BIN" "$BIN/tako-app"
TAKO_BIN="$BIN/tako"
APP_BIN="$BIN/tako-app"
: > "$CALLS"

# --- 偽物 -----------------------------------------------------------------------
# ログインシェル（exe::find の `$SHELL -l -c command -v …`）を砂場の PATH に閉じ込める。ペインのシェルも同じ
cat > "$STUBS/isosh" << 'EOF'
#!/bin/sh
if [ "$1" = "-l" ]; then shift; fi
exec /bin/sh "$@"
EOF

# 偽の claude。auth はログインの印で答え、対話として起こされたら受け取った最初のメッセージを残して、
# 閉じる印（~/.fake-dialogue-end）が置かれるまで待つ（= 人が対話を閉じるまで）
cat > "$TMP/fake-claude" << 'EOF'
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
  case "${2:-}" in
    list) if [ -f "$HOME/.fake-mcp" ]; then cat "$HOME/.fake-mcp"; else echo "No MCP servers configured"; fi;;
    add) shift 2; name=""
         while [ $# -gt 0 ]; do case "$1" in --scope|--transport) shift 2;; --) shift; break;; *) name="$1"; shift;; esac; done
         echo "$name: $* - connected" > "$HOME/.fake-mcp"; echo "Added stdio MCP server $name";;
    remove) rm -f "$HOME/.fake-mcp";;
  esac
  exit 0
fi
printf '%s\n' "${1:-}" > "$HOME/.fake-dialogue-greeting"
printf '\n[偽 claude・テスト用（LLM は呼ばない）] 対話アシスタントとして起動されました\n'
printf '受け取った最初のメッセージ:\n  %s\n\n> ' "${1:-}"
while [ ! -e "$HOME/.fake-dialogue-end" ]; do sleep 0.2; done
rm -f "$HOME/.fake-dialogue-end"
echo "(stub) 対話を閉じました"
exit 0
EOF

# 偽の curl: 呼ばれたことを記録し、`-o <dest>` へ偽のインストーラ（未ログインの偽 claude を置く）を書く
cat > "$STUBS/curl" << EOF
#!/bin/sh
echo "curl \$*" >> "$CALLS"
dest=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) dest="\$2"; shift 2;; *) shift;; esac
done
[ -n "\$dest" ] || exit 2
cat > "\$dest" << 'EOS'
#!/bin/bash
mkdir -p "\$HOME/.local/bin"
/bin/cp -f "$TMP/fake-claude" "\$HOME/.local/bin/claude"
/bin/chmod 755 "\$HOME/.local/bin/claude"
echo "Claude Code successfully installed! (stub)"
EOS
exit 0
EOF
# 偽の brew: 呼ばれたことを記録するだけ（依存の導入 = ネットワーク）
printf '#!/bin/sh\necho "brew $*" >> "%s"\nexit 0\n' "$CALLS" > "$STUBS/brew"
fake_exec_warm "$STUBS/isosh" "$TMP/fake-claude" "$STUBS/curl" "$STUBS/brew"

# 砂場の env（GUI と CLI で同じ）。tmux / tailscale は未作成のパスへ向けて「無い」を作る（#1499 の実経路テストと同じ）
sandbox_env() {
  echo "HOME=$TMP/home" "SHELL=$STUBS/isosh" "CODEX_HOME=$TMP/home/.codex" \
    "PATH=$TMP/home/.local/bin:$BIN:$STUBS:/usr/bin:/bin:/usr/sbin:/sbin" "TAKO_ISOLATED=1" \
    "TAKO_TMUX_BIN=$STUBS/none/tmux" "TAKO_TAILSCALE_BIN=$STUBS/none/tailscale" "TAKO_TAILSCALE_SOCKET=" \
    "TAKO_LANG=ja"
}
tk() {
  # shellcheck disable=SC2046
  env -u TAKO_SOCKET -u TAKO_PANE_ID $(sandbox_env) "$TAKO_BIN" "$@"
}
# JSON の 1 項目（a.b.c）を読む（真偽は true / false、無ければ null）
jget() {
  perl -MJSON::PP -e '
my $v = eval { JSON::PP->new->decode($ARGV[0]) };
for my $k (split /\./, $ARGV[1]) { $v = ref($v) eq "HASH" ? $v->{$k} : undef }
if (!defined $v) { print "null" }
elsif (JSON::PP::is_bool($v)) { print $v ? "true" : "false" }
elsif (ref $v) { print JSON::PP->new->canonical->encode($v) }
else { print $v }' "$1" "$2" 2> /dev/null
}
fresh_home() {
  rm -rf "${TMP:?}/home"
  mkdir -p "$TMP/home"
  # isolated-gui.sh が置く既定の .zshrc を先に置いておく（押す前のファイルの一覧の基準をこのテストが決める）
  printf '%s\n' "PROMPT='tako %1~ %% '" > "$TMP/home/.zshrc"
  : > "$CALLS"
}
calls() {
  local n=0 line
  while IFS= read -r line; do
    case "$line" in "$1 "*) n=$((n + 1)) ;; esac
  done < "$CALLS"
  echo "$n"
}
shot() { [ -n "${SHOT_CMD:-}" ] && $SHOT_CMD "$1" "$APP_PID"; return 0; }
# ファイルの一覧（置き場からの相対パス・並べ替え済み）
tree_of() { (cd "$1" 2> /dev/null && find . -type f -o -type l | LC_ALL=C sort); }

start_app() {
  local name="$1"
  shift
  export TAKO_DATA_DIR="$TMP/$name/data"
  export TAKO_DISCOVERY_DIR="$TMP/$name/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$name/orch"
  export TAKO_SESSIONS_FILE="$TMP/$name/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$name/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$name/workers.yaml"
  export TAKO_TMUX_SOCKET="tako-2879-$$-$name"
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
# setup のタブの依存の導入の確認（tmux / tailscale の [y/N]。偽の brew が導入器に見える）に n で答える。
# ANSWER_PROMPTS=1 のあいだ wait_step が毎回見る（導入の [Y/n] は各節が自分で答える）
ANSWER_PROMPTS=0
answer_dep_prompts() {
  [ "$ANSWER_PROMPTS" = 1 ] || return 0
  local pane screen last
  pane="$(setup_pane)"
  [ -n "$pane" ] || return 0
  screen="$(tk read --pane "$pane" 2> /dev/null)"
  last="$(printf '%s\n' "$screen" | sed -e '/^[[:space:]]*$/d' | sed -n '$p')"
  case "$last" in
    *"[y/N]:" | *"[y/N]: ")
      tk send --pane "$pane" "n" > /dev/null 2>&1
      echo "  （依存の導入の確認に n と答えた: ${last##* }）"
      ;;
  esac
}
# `tako welcome` の段が kind（と phase）になるまで待つ。最後の応答を $LAST へ
LAST=""
wait_step() {
  local kind="$1" phase="${2:-}" limit="${3:-90}" i=0
  while [ "$i" -lt "$limit" ]; do
    answer_dep_prompts
    LAST="$(tk welcome 2> /dev/null)"
    if [ "$(jget "$LAST" onboarding.state.step.kind)" = "$kind" ] &&
      { [ -z "$phase" ] || [ "$(jget "$LAST" onboarding.state.step.phase)" = "$phase" ]; }; then
      return 0
    fi
    sleep 1
    i=$((i + 1))
  done
  return 1
}
# 導線のタブ（題 setup）のペイン ID
setup_pane() {
  perl -MJSON::PP -e '
my $l = eval { JSON::PP->new->decode($ARGV[0]) } || {};
for my $t (@{ $l->{tabs} || [] }) {
  next unless ($t->{title} // "") eq "setup";
  my $p = $t->{panes}[0]; print $p->{id} if $p; last;
}' "$(tk list 2> /dev/null)" 2> /dev/null
}

# ================================================================================
echo "== A. claude が無い砂場: setup → 導入の同意 → ログイン → 続ける → 対話 → master =="
fresh_home
start_app fresh
wait_step setup "" 60 && ok "A1: 次にやること = setup（セットアップを実行）" || ng "A1: setup の段にならない: $LAST"
check "A1: バナーが出ている" "$(jget "$LAST" visible)" "true"
check "A1: 導線の形はボタン起点" "$(jget "$LAST" onboarding.flow)" "guided"
check "A1: 最簡コマンドは tako setup" "$(jget "$LAST" onboarding.state.step.command)" "tako setup"
check "A1: 自動の段は走っていない" "$(jget "$LAST" onboarding.state.auto.state)" "idle"
check "A1: 印はまだ書いていない（押すまで）" "$(jget "$LAST" onboarding.active)" "false"
sleep 3
check "A1: 押す前の HOME は置いたものだけ（PATH・MCP 登録・CLAUDE.md を書かない）" "$(tree_of "$TMP/home")" "./.zshrc"
for f in setup orchestrator settings.json; do
  [ -e "$TAKO_DATA_DIR/$f" ] && ng "A1: 押す前に data dir に $f がある" || ok "A1: 押す前に data dir に $f が無い"
done
check "A1: 押す前の orchestrator の置き場は空" "$(tree_of "$TAKO_ORCHESTRATOR_DIR")" ""
check "A1: ネットワークへ出ていない（curl / brew）" "$(calls curl)/$(calls brew)" "0/0"
tree_of "$TAKO_DATA_DIR" > "$TMP/A1-data-files.txt"
shot a1-first-launch

NEXT="$(tk welcome next 2>&1)"
check "A2: next は setup の段を押した" "$(jget "$NEXT" launched.kind)" "setup"
wait_step running setup 60 && ok "A2: setup のタブで tako setup が走っている（running / setup）" || ng "A2: $LAST"
PANE="$(setup_pane)"
[ -n "$PANE" ] && ok "A2: setup のタブが開いた（ペイン ${PANE}）" || ng "A2: setup のタブが無い"
check "A2: 押したら導線の印を書いた" "$(jget "$(tk welcome)" onboarding.active)" "true"
# 導入の [Y/n] が出るまで待つ
i=0
while :; do
  SCREEN="$(tk read --pane "$PANE" 2> /dev/null)"
  [[ "$SCREEN" == *"インストールしますか"* ]] && break
  sleep 1; i=$((i + 1)); [ "$i" -lt 60 ] || break
done
shot a2-setup-running
tk send --pane "$PANE" "Y" > /dev/null 2>&1
ANSWER_PROMPTS=1
wait_step login "" 90 && ok "A3: 導入の後、次にやること = login" || ng "A3: $LAST"
check "A3: 公式インストーラの取得は同意の後に 1 回だけ" "$(calls curl)" "1"
[ -x "$TMP/home/.local/bin/claude" ] && ok "A3: 偽のインストーラが claude を置いた" || ng "A3: claude が置かれていない"
[ -e "$TAKO_DATA_DIR/setup/running.json" ] && ng "A3: setup が終わっても走っている印が残った" || ok "A3: setup が終わったら走っている印は消える"
check "A3: 対話はまだ（ログイン待ちで起こせない）" "$([ -e "$TMP/home/.fake-dialogue-greeting" ] && echo yes || echo no)" "no"
shot a3-login

NEXT="$(tk welcome next 2>&1)"
check "A4: next はログインの段を押した" "$(jget "$NEXT" launched.kind)" "login"
wait_step continue "" 90 && ok "A4: ログインの後、次にやること = continue（セットアップを続ける）" || ng "A4: $LAST"
check "A4: 続けるの最簡コマンドは tako setup" "$(jget "$LAST" onboarding.state.step.command)" "tako setup"
shot a4-continue

NEXT="$(tk welcome next 2>&1)"
check "A5: next は続けるの段を押した" "$(jget "$NEXT" launched.kind)" "continue"
wait_step running dialogue 90 && ok "A5: 対話アシスタントが開いた（running / dialogue）" || ng "A5: $LAST"
GREETING="$(cat "$TMP/home/.fake-dialogue-greeting" 2> /dev/null)"
case "$GREETING" in
  *初回フローを始めます*) ok "A5: 2 回目の setup でも対話は初回フローの合図で始まる" ;;
  *) ng "A5: 対話の最初のメッセージが初回フローの合図を含まない: $GREETING" ;;
esac
# 走っているあいだの next はタブを見せるだけ（setup を 2 本打たない）
NEXT="$(tk welcome next 2>&1)"
check "A5: 対話中の next は running（打たない）" "$(jget "$NEXT" launched.kind)" "running"
shot a5-dialogue
: > "$TMP/home/.fake-dialogue-end"
wait_step master "" 90 && ok "A6: 対話を閉じたら、次にやること = master" || ng "A6: $LAST"
check "A6: 対話が済んだ印（setup.dialogue_done）" \
  "$(grep -c 'dialogue_done: true' "$TAKO_ORCHESTRATOR_DIR/config.yaml" 2> /dev/null)" "1"
shot a6-master
stop_app

echo "== A7. 再起動: 自動の段は走らず、バナーは master を出し続ける =="
export TAKO_DATA_DIR="$TMP/fresh/data"
# shellcheck disable=SC2046
launch_isolated_gui "$TMP/app-fresh2.log" $(sandbox_env) TAKO_PERSIST=0 TAKO_FIRST_RUN=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app-fresh2.log" || exit 1
wait_step master "" 60 && ok "A7: 再起動してもバナーは master を出す" || ng "A7: $LAST"
check "A7: 再起動しても自動の段は走らない" "$(jget "$LAST" onboarding.state.auto.state)" "idle"
check "A7: バナーが出ている" "$(jget "$LAST" visible)" "true"
stop_app

echo "== B. claude がログイン済みの砂場: setup → そのまま対話 → master =="
fresh_home
mkdir -p "$TMP/home/.local/bin"
/bin/cp -f "$TMP/fake-claude" "$TMP/home/.local/bin/claude"
fake_exec_warm "$TMP/home/.local/bin/claude"
: > "$TMP/home/.fake-claude-authed"
BASE_HOME="$(tree_of "$TMP/home")"
start_app authed
ANSWER_PROMPTS=1
wait_step setup "" 60 && ok "B1: 次にやること = setup" || ng "B1: $LAST"
sleep 3
check "B1: 押す前の HOME は置いたものだけ" "$(tree_of "$TMP/home")" "$BASE_HOME"
check "B1: 押す前の orchestrator の置き場は空" "$(tree_of "$TAKO_ORCHESTRATOR_DIR")" ""
shot b1-first-launch
tk welcome next > /dev/null 2>&1
wait_step running dialogue 90 && ok "B2: setup の後そのまま対話アシスタントが開いた" || ng "B2: $LAST"
case "$(cat "$TMP/home/.fake-dialogue-greeting" 2> /dev/null)" in
  *初回フローを始めます*) ok "B2: 対話は初回フローの合図で始まる" ;;
  *) ng "B2: 初回フローの合図が無い" ;;
esac
check "B2: ネットワークへ出ていない" "$(calls curl)/$(calls brew)" "0/0"
shot b2-dialogue
: > "$TMP/home/.fake-dialogue-end"
wait_step master "" 90 && ok "B3: 対話を閉じたら master" || ng "B3: $LAST"
shot b3-master
# 「master を起動」はボタンが安定版を先に選ぶ（本番の tako が入っている機では押さない）
if [ -e /Applications/tako.app/Contents/MacOS/tako ]; then
  echo "  （未実測: 本番の tako が /Applications にあるので master のボタンは押さない。導線を終える判断は単体テスト）"
else
  tk welcome next > /dev/null 2>&1
  i=0
  until [ "$(jget "$(tk welcome)" visible)" = "false" ]; do sleep 1; i=$((i + 1)); [ "$i" -lt 30 ] || break; done
  check "B4: master を押すと導線が終わる" "$(jget "$(tk welcome)" onboarding.active)" "false"
fi
stop_app

echo "== C. 既存の利用者（settings.json あり・#1510 の印が立ったまま・setup 済み）: バナーは出ない =="
ANSWER_PROMPTS=0
fresh_home
mkdir -p "$TMP/existing/data" "$TMP/existing/orch"
printf '%s\n' '{"theme":"dark","welcome_onboarding":true}' > "$TMP/existing/data/settings.json"
printf 'setup:\n  completed: true\n  applied_revision: 20\n' > "$TMP/existing/orch/config.yaml"
start_app existing
sleep 3
LAST="$(tk welcome 2> /dev/null)"
check "C1: 既存の利用者にはバナーを出さない" "$(jget "$LAST" visible)" "false"
check "C1: 自動の段は走らない" "$(jget "$LAST" onboarding.state.auto.state)" "idle"
check "C1: 印は立てない" "$(jget "$LAST" onboarding.active)" "false"
shot c1-existing-user
stop_app

echo "== D. A/B（TAKO_2879_LEGACY=1）: #1510 の自動の段が走る =="
fresh_home
start_app legacy TAKO_2879_LEGACY=1
i=0
until [ "$(jget "$(tk welcome 2> /dev/null)" onboarding.state.auto.state)" = "done" ]; do
  sleep 1; i=$((i + 1)); [ "$i" -lt 120 ] || break
done
LAST="$(tk welcome 2> /dev/null)"
check "D1: A/B は導線の形が auto" "$(jget "$LAST" onboarding.flow)" "auto"
check "D1: A/B は起動時に自動の段が走る" "$(jget "$LAST" onboarding.state.auto.state)" "done"
[ -f "$TMP/home/.claude/CLAUDE.md" ] && ok "D1: A/B は押す前に指示ファイルを書く（#1510 の形）" || ng "D1: A/B で自動の段が書いていない"
stop_app

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
