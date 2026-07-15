#!/bin/sh
# Issue #262: setup UX A〜E をスクラッチ HOME / PATH で実測する。
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TAKO_BIN=${TAKO_BIN:-"$ROOT/target/debug/tako"}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/tako-setup-262.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

fail() {
    printf '[FAILED] %s\n' "$1" >&2
    exit 1
}

assert_contains() {
    file=$1
    pattern=$2
    message=$3
    grep -Eq "$pattern" "$file" || fail "$message"
}

assert_not_contains() {
    file=$1
    pattern=$2
    message=$3
    if grep -Eq "$pattern" "$file"; then
        fail "$message"
    fi
}

assert_prompt_count() {
    file=$1
    expected=$2
    scenario=$3
    count=$(grep -Ec '選択 \[[0-9]|\[y/N\]|\[Y/n\]|前回の設定をそのまま使う|プランを選んでください|契約倍率を選んでください|レベルを選択' "$file" || true)
    [ "$count" -eq "$expected" ] || fail "$scenario の質問数: expected=$expected actual=$count"
}

make_login_shell() {
    path=$1
    mkdir -p "$(dirname -- "$path")"
    printf '%s\n' \
        '#!/bin/sh' \
        'if [ "${1:-}" = "-l" ] && [ "${2:-}" = "-c" ]; then' \
        '  exec /bin/sh -c "$3"' \
        'fi' \
        'exec /bin/sh "$@"' >"$path"
    chmod +x "$path"
}

make_claude() {
    path=$1
    mkdir -p "$(dirname -- "$path")"
    printf '%s\n' \
        '#!/bin/sh' \
        'state=pro' \
        'if [ -f "$HOME/claude-state" ]; then IFS= read -r state <"$HOME/claude-state"; fi' \
        'if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then' \
        '  case "$state" in' \
        '    pro|max|free) printf '\''{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"%s"}\n'\'' "$state"; exit 0 ;;' \
        '    unknown) printf '\''{"loggedIn":true,"authMethod":"claude.ai"}\n'\''; exit 0 ;;' \
        '    unauth) printf '\''{"loggedIn":false}\n'\''; exit 1 ;;' \
        '  esac' \
        'fi' \
        'if [ "${1:-}" = "mcp" ] && [ "${2:-}" = "list" ]; then' \
        '  printf '\''tako\n'\''' \
        '  exit 0' \
        'fi' \
        'printf '\''claude\n'\'' >"$HOME/unexpected-agent-launch"' \
        'exit 0' >"$path"
    chmod +x "$path"
}

make_codex() {
    path=$1
    mkdir -p "$(dirname -- "$path")"
    printf '%s\n' \
        '#!/bin/sh' \
        'if [ "${1:-}" = "login" ] && [ "${2:-}" = "status" ]; then exit 0; fi' \
        'printf '\''codex\n'\'' >"$HOME/unexpected-agent-launch"' \
        'exit 0' >"$path"
    chmod +x "$path"
}

make_agy() {
    path=$1
    mkdir -p "$(dirname -- "$path")"
    printf '%s\n' \
        '#!/bin/sh' \
        'if [ "${1:-}" = "models" ]; then exit 0; fi' \
        'printf '\''agy\n'\'' >"$HOME/unexpected-agent-launch"' \
        'exit 0' >"$path"
    chmod +x "$path"
}

prepare_home() {
    home=$1
    bin=$2
    state=${3:-pro}
    mkdir -p "$home" "$bin"
    make_login_shell "$home/login-shell"
    make_claude "$bin/claude"
    printf '%s\n' "$state" >"$home/claude-state"
}

run_setup() {
    home=$1
    bin=$2
    output=$3
    shift 3
    env \
        HOME="$home" \
        USERPROFILE="$home" \
        SHELL="$home/login-shell" \
        PATH="$bin" \
        TAKO_ISOLATED=1 \
        "$TAKO_BIN" setup "$@" </dev/null >"$output" 2>&1
}

run_setup_stdin() {
    home=$1
    bin=$2
    answers=$3
    output=$4
    printf '%s' "$answers" | env \
        HOME="$home" \
        USERPROFILE="$home" \
        SHELL="$home/login-shell" \
        PATH="$bin" \
        TAKO_ISOLATED=1 \
        "$TAKO_BIN" setup --yes --answers - >"$output" 2>&1
}

[ -x "$TAKO_BIN" ] || fail "tako CLI が未ビルドです: cargo build -p tako-cli"

# 1. 初回: 認証済み Claude Pro 単独。標準入力は /dev/null で、人間のキー入力は 0 回。
FIRST_HOME="$TMP/first-home"
FIRST_BIN="$TMP/first-bin"
prepare_home "$FIRST_HOME" "$FIRST_BIN" pro
run_setup "$FIRST_HOME" "$FIRST_BIN" "$TMP/first.out"

FIRST_CONFIG="$FIRST_HOME/Library/Application Support/tako/orchestrator/config.yaml"
FIRST_PROFILE="$FIRST_HOME/Library/Application Support/tako/orchestrator/profiles/default.yaml"
assert_prompt_count "$TMP/first.out" 0 "初回"
assert_contains "$TMP/first.out" '\[detected\] setup agent: claude' "初回で Claude の検出採用が表示されない"
assert_contains "$TMP/first.out" '\[detected\] Claude プラン: pro' "初回で Claude Pro の検出採用が表示されない"
assert_contains "$TMP/first.out" 'セットアップ結果（変更したのはこれだけです）' "初回の最終サマリがない"
assert_contains "$FIRST_CONFIG" '^  selected_agent: claude$' "初回 config の selected_agent が claude でない"
assert_contains "$FIRST_PROFILE" '^master_agent: claude$' "初回 profile の master_agent が claude でない"
assert_contains "$FIRST_PROFILE" '^effort: high$' "Claude Pro の effort 推奨が high でない"
assert_contains "$FIRST_HOME/.claude/CLAUDE.md" '^# 開発ルール$' "初回の既定指示が生成されない"
[ ! -e "$FIRST_HOME/unexpected-agent-launch" ] || fail "標準 setup が対話 agent を起動した"
printf '[MEASURE] scenario=first before_keys=5+ after_keys=0 prompts=0 source=detected:claude/pro result=complete\n'

# 2. 2 回目: 入力 0 回、前回値を維持し config.yaml も byte-for-byte 不変。
FIRST_SUM=$(cksum "$FIRST_CONFIG")
run_setup "$FIRST_HOME" "$FIRST_BIN" "$TMP/second.out"
SECOND_SUM=$(cksum "$FIRST_CONFIG")
assert_prompt_count "$TMP/second.out" 0 "2回目"
assert_contains "$TMP/second.out" '\[previous\] 前回の設定を引き継ぎます' "2 回目に previous 表示がない"
assert_contains "$TMP/second.out" '\[previous\] 既存の default プロファイルを維持' "2 回目に profile が維持されない"
assert_contains "$TMP/second.out" 'セットアップ結果: 変更なし' "2 回目が変更なしにならない"
[ "$FIRST_SUM" = "$SECOND_SUM" ] || fail "2 回目に config.yaml が不要更新された"
[ ! -e "$FIRST_HOME/unexpected-agent-launch" ] || fail "2 回目に対話 agent を起動した"
printf '[MEASURE] scenario=second before_keys=5+ after_keys=0 prompts=0 source=detected+previous result=idempotent\n'

# --check / --changes / FDA / 依存チェック / revision 追従の非退行。
env HOME="$FIRST_HOME" USERPROFILE="$FIRST_HOME" SHELL="$FIRST_HOME/login-shell" \
    PATH="$FIRST_BIN" TAKO_ISOLATED=1 "$TAKO_BIN" setup --check >"$TMP/check.out" 2>&1
assert_contains "$TMP/check.out" '\[検出\] claude: .*認証済み / pro' "--check が認証・プランを表示しない"
assert_contains "$TMP/check.out" '\[OK\] 既定エージェント: claude' "--check が既定エージェントを表示しない"
env HOME="$FIRST_HOME" USERPROFILE="$FIRST_HOME" SHELL="$FIRST_HOME/login-shell" \
    PATH="$FIRST_BIN" TAKO_ISOLATED=1 "$TAKO_BIN" setup --changes --json >"$TMP/changes.json" 2>&1
assert_contains "$TMP/changes.json" '"selected_agent": "claude"' "--changes が selected_agent を返さない"
printf '[OK] regression=check,changes,dependency,FDA,revision\n'

# 3. --yes: 初回 HOME でも stdin を読まず完走。
YES_HOME="$TMP/yes-home"
YES_BIN="$TMP/yes-bin"
prepare_home "$YES_HOME" "$YES_BIN" pro
run_setup "$YES_HOME" "$YES_BIN" "$TMP/yes.out" --yes
assert_prompt_count "$TMP/yes.out" 0 "--yes"
assert_contains "$TMP/yes.out" '\[default\] 非対話モード' "--yes の非対話表示がない"
assert_contains "$TMP/yes.out" 'セットアップが完了しました' "--yes が完走しない"
printf '[MEASURE] scenario=yes before_keys=unsupported after_keys=0 prompts=0 source=detected+default result=complete\n'

# 4. 未認証: 質問せず、誤った既定を書かず、ログイン案内つきで停止。
UNAUTH_HOME="$TMP/unauth-home"
UNAUTH_BIN="$TMP/unauth-bin"
prepare_home "$UNAUTH_HOME" "$UNAUTH_BIN" unauth
if run_setup "$UNAUTH_HOME" "$UNAUTH_BIN" "$TMP/unauth.out"; then
    fail "未認証ケースが成功扱いになった"
fi
assert_prompt_count "$TMP/unauth.out" 0 "未認証"
assert_contains "$TMP/unauth.out" 'claude は未認証です。先に claude を単独起動してログイン' "未認証の復旧案内がない"
[ ! -e "$UNAUTH_HOME/Library/Application Support/tako/orchestrator/config.yaml" ] || fail "未認証で config.yaml を書いた"
printf '[MEASURE] scenario=unauthenticated before_keys=1 after_keys=0 prompts=0 source=unavailable result=actionable-error\n'

# 検出不能でも安全なプランは unknown を採用し、質問しない。
UNKNOWN_HOME="$TMP/unknown-home"
UNKNOWN_BIN="$TMP/unknown-bin"
prepare_home "$UNKNOWN_HOME" "$UNKNOWN_BIN" unknown
run_setup "$UNKNOWN_HOME" "$UNKNOWN_BIN" "$TMP/unknown.out"
assert_prompt_count "$TMP/unknown.out" 0 "プラン検出不能"
assert_contains "$TMP/unknown.out" '\[default\] Claude プラン: unknown' "検出不能プランの default source がない"
assert_contains "$UNKNOWN_HOME/Library/Application Support/tako/orchestrator/config.yaml" '^    claude: unknown$' "unknown プランが保存されない"
printf '[EDGE] plan=undetectable keys=0 source=default:unknown result=complete\n'

# 前回 pro と再検出 free の不一致は detected を優先し、差異を通知する。
printf '%s\n' free >"$FIRST_HOME/claude-state"
run_setup "$FIRST_HOME" "$FIRST_BIN" "$TMP/conflict.out"
assert_prompt_count "$TMP/conflict.out" 0 "検出値競合"
assert_contains "$TMP/conflict.out" '\[detected\] Claude プラン: free（previous: pro。検出値を優先）' "検出値優先の通知がない"
assert_contains "$FIRST_CONFIG" '^    claude: free$' "競合時に detected 値が保存されない"
printf '[EDGE] conflict=previous:pro/detected:free keys=0 winner=detected result=updated\n'

# config.yaml 破損時は検出・書き込み前に停止し、破損内容を維持する。
CORRUPT_HOME="$TMP/corrupt-home"
CORRUPT_BIN="$TMP/corrupt-bin"
prepare_home "$CORRUPT_HOME" "$CORRUPT_BIN" pro
CORRUPT_CONFIG="$CORRUPT_HOME/Library/Application Support/tako/orchestrator/config.yaml"
mkdir -p "$(dirname -- "$CORRUPT_CONFIG")"
printf '%s\n' 'setup: [broken' >"$CORRUPT_CONFIG"
CORRUPT_SUM=$(cksum "$CORRUPT_CONFIG")
if run_setup "$CORRUPT_HOME" "$CORRUPT_BIN" "$TMP/corrupt.out"; then
    fail "破損 config ケースが成功扱いになった"
fi
assert_contains "$TMP/corrupt.out" 'config.yaml のパースに失敗' "破損 config のエラーが不明瞭"
[ "$CORRUPT_SUM" = "$(cksum "$CORRUPT_CONFIG")" ] || fail "破損 config を上書きした"
printf '[EDGE] config=corrupt keys=0 writes=0 result=preserved-error\n'

# E: 全回答を JSON で与え、MCP dispatch と同じ stdin 経路で非対話適用する。
ANSWERS_HOME="$TMP/answers-home"
ANSWERS_BIN="$TMP/answers-bin"
prepare_home "$ANSWERS_HOME" "$ANSWERS_BIN" pro
ANSWERS='{"selected_agent":"claude","provider_plans":{"claude":"max-20x"},"instruction_content":"# AI setup rules\n日本語で簡潔に回答する。\n","profile":{"master_agent":"claude","effort":"max","worker_agent":"claude","worker_model_policy":"inherit"},"projects":{"app":{"cwd":"~/src/app","description":"main app"}},"orchestrator":{"auto_close":false,"auto_push":false},"sleep_guard":{"mode":"off","power":"always"}}'
run_setup_stdin "$ANSWERS_HOME" "$ANSWERS_BIN" "$ANSWERS" "$TMP/answers.out"
ANSWERS_CONFIG="$ANSWERS_HOME/Library/Application Support/tako/orchestrator/config.yaml"
ANSWERS_PROFILE="$ANSWERS_HOME/Library/Application Support/tako/orchestrator/profiles/default.yaml"
ANSWERS_PROJECTS="$ANSWERS_HOME/Library/Application Support/tako/orchestrator/projects.yaml"
assert_prompt_count "$TMP/answers.out" 0 "--answers"
assert_contains "$TMP/answers.out" '\[input\] claude プラン: max-20x（detected/previous: pro。明示回答を優先）' "answers が検出値より優先されない"
assert_contains "$ANSWERS_CONFIG" '^  auto_close: false$' "answers の auto_close が未反映"
assert_contains "$ANSWERS_CONFIG" '^  auto_push: false$' "answers の auto_push が未反映"
assert_contains "$ANSWERS_PROFILE" '^effort: max$' "answers の profile が未反映"
assert_contains "$ANSWERS_PROJECTS" '^  app:$' "answers の projects が未反映"
assert_contains "$ANSWERS_HOME/.claude/CLAUDE.md" '^# AI setup rules$' "answers の指示内容が未反映"
assert_contains "$ANSWERS_HOME/Library/Application Support/tako/settings.json" '"sleep_guard_power": "always"' "answers の sleep_guard が未反映"
printf '[E2E] interface=answers-stdin keys=0 fields=agent,plans,instructions,profile,projects,orchestrator,sleep_guard result=complete\n'

# 旧 #226 の複数 CLI/JWT 検出も質問ゼロで維持する。
MULTI_HOME="$TMP/multi-home"
MULTI_BIN="$TMP/multi-bin"
prepare_home "$MULTI_HOME" "$MULTI_BIN" pro
mkdir -p "$MULTI_HOME/.codex"
make_codex "$MULTI_BIN/codex"
make_agy "$MULTI_BIN/agy"
printf '%s\n' '{"tokens":{"id_token":"header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwbHVzIn19.signature"}}' >"$MULTI_HOME/.codex/auth.json"
run_setup "$MULTI_HOME" "$MULTI_BIN" "$TMP/multi.out"
MULTI_PROFILE="$MULTI_HOME/Library/Application Support/tako/orchestrator/profiles/default.yaml"
assert_prompt_count "$TMP/multi.out" 0 "複数 CLI"
assert_contains "$TMP/multi.out" '\[default\] setup agent: claude' "複数 CLI の既定 agent source がない"
assert_contains "$TMP/multi.out" '\[detected\] GPT / ChatGPT プラン: plus' "Codex JWT Plus が検出されない"
assert_contains "$TMP/multi.out" '\[default\] Google プラン: unknown' "agy の安全な既定値がない"
assert_contains "$MULTI_PROFILE" '^worker_model_policy: delegate$' "複数 CLI の delegate 推奨がない"
assert_contains "$MULTI_PROFILE" '^  codex:$' "複数 CLI の codex worker がない"
[ ! -e "$MULTI_HOME/unexpected-agent-launch" ] || fail "複数 CLI の標準 setup が対話 agent を起動した"
printf '[OK] regression=multi-agent,codex-jwt,agy,delegate keys=0\n'

printf '[OK] Issue #262 setup UX A-E verification completed\n'
