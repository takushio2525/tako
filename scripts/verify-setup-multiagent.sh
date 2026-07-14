#!/bin/sh
# Issue #226: スクラッチ HOME / PATH で tako setup の CLI 選択と profile 生成を実測する。
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TAKO_BIN=${TAKO_BIN:-"$ROOT/target/debug/tako"}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/tako-setup-226.XXXXXX")
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
        'if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then' \
        '  printf '\''{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"pro"}\n'\''' \
        '  exit 0' \
        'fi' \
        'if [ "${1:-}" = "mcp" ] && [ "${2:-}" = "list" ]; then' \
        '  printf '\''tako\n'\''' \
        '  exit 0' \
        'fi' \
        'printf '\''claude\n'\'' >"$HOME/launched-agent"' \
        'exit 0' >"$path"
    chmod +x "$path"
}

make_codex() {
    path=$1
    mkdir -p "$(dirname -- "$path")"
    printf '%s\n' \
        '#!/bin/sh' \
        'if [ "${1:-}" = "login" ] && [ "${2:-}" = "status" ]; then' \
        '  exit 0' \
        'fi' \
        'printf '\''codex\n'\'' >"$HOME/launched-agent"' \
        'exit 0' >"$path"
    chmod +x "$path"
}

make_agy() {
    path=$1
    mkdir -p "$(dirname -- "$path")"
    printf '%s\n' \
        '#!/bin/sh' \
        'if [ "${1:-}" = "models" ]; then' \
        '  exit 0' \
        'fi' \
        'printf '\''agy\n'\'' >"$HOME/launched-agent"' \
        'exit 0' >"$path"
    chmod +x "$path"
}

run_setup() {
    home=$1
    bin=$2
    input=$3
    output=$4
    printf '%b' "$input" | env \
        HOME="$home" \
        USERPROFILE="$home" \
        SHELL="$home/login-shell" \
        PATH="$bin" \
        "$TAKO_BIN" setup >"$output" 2>&1
}

[ -x "$TAKO_BIN" ] || fail "tako CLI が未ビルドです: cargo build -p tako-cli"

# ケース1: claude のみ。選択質問なしで claude を既定にする。
SINGLE_HOME="$TMP/single-home"
SINGLE_BIN="$TMP/single-bin"
mkdir -p "$SINGLE_HOME" "$SINGLE_BIN"
make_login_shell "$SINGLE_HOME/login-shell"
make_claude "$SINGLE_BIN/claude"
run_setup "$SINGLE_HOME" "$SINGLE_BIN" '\n1\n1\n' "$TMP/single.out"

SINGLE_CONFIG="$SINGLE_HOME/Library/Application Support/tako/orchestrator/config.yaml"
SINGLE_PROFILE="$SINGLE_HOME/Library/Application Support/tako/orchestrator/profiles/default.yaml"
assert_contains "$TMP/single.out" '\[自動選択\].*claude のみ' "claude 単独の自動選択が表示されない"
assert_contains "$SINGLE_CONFIG" '^  selected_agent: claude$' "config の selected_agent が claude でない"
assert_contains "$SINGLE_PROFILE" '^master_agent: claude$' "profile の master_agent が claude でない"
assert_contains "$SINGLE_PROFILE" '^worker_agent: claude$' "profile の worker_agent が claude でない"
assert_contains "$SINGLE_PROFILE" '^effort: high$' "Claude Pro の effort 推奨が high でない"
assert_contains "$SINGLE_HOME/launched-agent" '^claude$' "setup 対話に claude が起動されていない"
printf '[OK] claude のみ: 自動選択 -> master/worker=claude, effort=high\n'

env HOME="$SINGLE_HOME" USERPROFILE="$SINGLE_HOME" SHELL="$SINGLE_HOME/login-shell" \
    PATH="$SINGLE_BIN" "$TAKO_BIN" setup --check >"$TMP/check.out" 2>&1
assert_contains "$TMP/check.out" '\[検出\] claude: .*認証済み / pro' "--check が claude の認証・プランを表示しない"
assert_contains "$TMP/check.out" '\[OK\] 既定エージェント: claude' "--check が既定エージェントを表示しない"

env HOME="$SINGLE_HOME" USERPROFILE="$SINGLE_HOME" SHELL="$SINGLE_HOME/login-shell" \
    PATH="$SINGLE_BIN" "$TAKO_BIN" setup --changes --json >"$TMP/changes.json" 2>&1
assert_contains "$TMP/changes.json" '"current_revision": 8' "--changes が revision 8 を返さない"
assert_contains "$TMP/changes.json" '"selected_agent": "claude"' "--changes が selected_agent を返さない"
printf '[OK] 非退行: --check / --changes、依存チェック、FDA 判定を隔離 HOME で完走\n'

# ケース2: 3 CLI。選択肢2の codex と JWT の Plus プランを profile へ反映する。
MULTI_HOME="$TMP/multi-home"
MULTI_BIN="$TMP/multi-bin"
mkdir -p "$MULTI_HOME/.codex" "$MULTI_BIN"
make_login_shell "$MULTI_HOME/login-shell"
make_claude "$MULTI_BIN/claude"
make_codex "$MULTI_BIN/codex"
make_agy "$MULTI_BIN/agy"
printf '%s\n' '{"tokens":{"id_token":"header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwbHVzIn19.signature"}}' >"$MULTI_HOME/.codex/auth.json"
chmod 600 "$MULTI_HOME/.codex/auth.json"
run_setup "$MULTI_HOME" "$MULTI_BIN" '\n2\n2\n' "$TMP/multi.out"

MULTI_CONFIG="$MULTI_HOME/Library/Application Support/tako/orchestrator/config.yaml"
MULTI_PROFILE="$MULTI_HOME/Library/Application Support/tako/orchestrator/profiles/default.yaml"
assert_contains "$TMP/multi.out" '^セットアップを進めるエージェントを選択してください:' "複数 CLI の選択式が表示されない"
assert_contains "$MULTI_CONFIG" '^  selected_agent: codex$' "config の selected_agent が codex でない"
assert_contains "$MULTI_CONFIG" '^    gpt: plus$' "Codex JWT の Plus プランが保存されていない"
assert_contains "$MULTI_PROFILE" '^master_agent: codex$' "profile の master_agent が codex でない"
assert_contains "$MULTI_PROFILE" '^worker_agent: codex$' "profile の worker_agent が codex でない"
assert_contains "$MULTI_PROFILE" '^effort: high$' "ChatGPT Plus の effort 推奨が high でない"
assert_contains "$MULTI_PROFILE" '^worker_model_policy: delegate$' "複数 CLI の worker 方針が delegate でない"
assert_contains "$MULTI_PROFILE" '^  agy:$' "profile に agy worker 設定がない"
assert_contains "$MULTI_PROFILE" '^  claude:$' "profile に claude worker 設定がない"
assert_contains "$MULTI_PROFILE" '^  codex:$' "profile に codex worker 設定がない"
assert_contains "$MULTI_HOME/launched-agent" '^codex$' "選択した codex が setup 対話に起動されていない"
printf '[OK] 複数 CLI: 選択 codex -> master/worker=codex, Plus=high, policy=delegate\n'
