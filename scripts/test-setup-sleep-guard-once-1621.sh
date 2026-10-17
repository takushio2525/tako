#!/usr/bin/env bash
# #1621 の実経路テスト: `tako setup`（run_setup）の出力でスリープ防止の段が**どの経路でも
# 1 度だけ**出ることを、隔離した HOME / TAKO_DATA_DIR / PATH で実測する。
#
# #1621 前は `--answers` に sleep_guard があると（MCP `tako_setup` / dispatch `SetupRun` と
# 同じ `tako setup --yes --answers -`）、依存チェック段が当てる前の値と「設定変更:」の案内を
# 出し、後段の書き込みが `[input] スリープ防止: …` をもう 1 度出していた。
#
# 本番の ~/Library/Application Support/tako・~/.claude・シェルの rc・スリープ設定（pmset /
# `tako sleep-guard set`）・LaunchAgent には触れない（`env -i` で環境ごと差し替え、claude /
# brew / tmux はスタブ。答えはスリープ防止の値を settings.json へ書くだけで pmset は叩かない）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PTY="$ROOT/scripts/lib/pty-answer.py"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }
# 状態の行（`スリープ防止: mode=`。今の値も [input] も拾う）の回数
state_count(){ grep -c "スリープ防止: mode=" "$1" 2>/dev/null; }
# 対話の見出し（`--review` だけが出す）の回数
menu_count(){ grep -c "スリープ防止の稼働レベルを選んでください" "$1" 2>/dev/null; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -f "$PTY" ]  || { echo "PTY ドライバが無い: $PTY"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1621-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
BIN="$SANDBOX/bin"
HOMEDIR="$SANDBOX/home"
DATADIR="$SANDBOX/d"
SETTINGS="$DATADIR/settings.json"

mkenv(){
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$BIN/brew"
  # 認証済みの claude（サブコマンドに即答。対話起動されても即終了）
  cat > "$BIN/claude" <<'EOF'
#!/bin/sh
case "${1:-} ${2:-}" in
  "auth status") echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0;;
  "--version "*) echo "2.1.258 (Claude Code)"; exit 0;;
  "mcp list") echo "No MCP servers configured"; exit 0;;
esac
exit 0
EOF
  # tmux は在る扱い（依存の段の [y/N] を出さない）
  printf '#!/bin/sh\necho "tmux 3.5a"\n' > "$BIN/tmux"
  chmod +x "$BIN"/*
}

run_pipe(){ # run_pipe <出力先> <stdin> -- <tako の引数...>
  local out="$1"; local input="$2"; shift 3
  printf '%s' "$input" | env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      "$TAKO" "$@" > "$out" 2>&1
  echo $?
}
run_pty(){ # run_pty <出力先> <answers...> -- <tako の引数...>（答えを使い切ったら Enter = 既定）
  local out="$1"; shift
  # 空配列の展開を守る形で書く（macOS 同梱の bash 3.2 は set -u 下で落ちる）
  local answers=(); while [ "${1:-}" != "--" ]; do answers+=(--answer "$1"); shift; done; shift
  env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      /usr/bin/python3 "$PTY" --expect '[y/N]:' --expect '[Y/n]:' --expect '選択 [' \
      --expect 'Enter でスキップ）:' --expect 'Enter で今のまま）:' \
      --expect 'Enter で雛形・今のまま）:' ${answers[@]+"${answers[@]}"} --timeout 90 \
      -- "$TAKO" "$@" > "$out" 2>&1
  echo $?
}
saved_mode(){ # settings.json に書かれた sleep_guard_mode（無ければ -）
  /usr/bin/python3 -c '
import json,sys
try: print(json.load(open(sys.argv[1])).get("sleep_guard_mode","-"))
except Exception: print("-")
' "$SETTINGS"
}

echo "== 1) 標準 setup（非 TTY・初回 / 2 回目）=="
mkenv
RC=$(run_pipe "$SANDBOX/1a.log" "" -- setup)
check    "初回: 終了コード 0"              "$RC" "0"
check    "初回: 状態の行は 1 度"           "$(state_count "$SANDBOX/1a.log")" "1"
contains "初回: 変え方の案内を出す"        "設定変更: tako sleep-guard set" "$SANDBOX/1a.log"
RC=$(run_pipe "$SANDBOX/1b.log" "" -- setup)
check    "2 回目: 終了コード 0"            "$RC" "0"
check    "2 回目: 状態の行は 1 度"         "$(state_count "$SANDBOX/1b.log")" "1"

echo "== 2) --check / --yes =="
RC=$(run_pipe "$SANDBOX/2a.log" "" -- setup --check)
check    "--check: 終了コード 0"           "$RC" "0"
check    "--check: 状態の行は 1 度"        "$(state_count "$SANDBOX/2a.log")" "1"
mkenv
RC=$(run_pipe "$SANDBOX/2b.log" "" -- setup --yes)
check    "--yes: 終了コード 0"             "$RC" "0"
check    "--yes: 状態の行は 1 度"          "$(state_count "$SANDBOX/2b.log")" "1"

echo "== 3) --answers（MCP tako_setup と同じ --yes --answers -）: sleep_guard なし / 空 =="
mkenv
RC=$(run_pipe "$SANDBOX/3a.log" '{"launch_agent":"none"}' -- setup --yes --answers -)
check    "sleep_guard なし: 終了コード 0"  "$RC" "0"
check    "sleep_guard なし: 状態の行は 1 度" "$(state_count "$SANDBOX/3a.log")" "1"
absent   "sleep_guard なし: [input] を出さない" "[input] スリープ防止" "$SANDBOX/3a.log"
mkenv
RC=$(run_pipe "$SANDBOX/3b.log" '{"launch_agent":"none","sleep_guard":{}}' -- setup --yes --answers -)
check    "空の sleep_guard: 終了コード 0"  "$RC" "0"
check    "空の sleep_guard: 状態の行は 1 度" "$(state_count "$SANDBOX/3b.log")" "1"
absent   "空の sleep_guard: 答えなしとして [input] を出さない" "[input] スリープ防止" "$SANDBOX/3b.log"

echo "== 4) --answers で値を変える（#1621 の再現経路）=="
mkenv
RC=$(run_pipe "$SANDBOX/4.log" '{"launch_agent":"none","sleep_guard":{"mode":"off"}}' -- setup --yes --answers -)
check    "終了コード 0"                    "$RC" "0"
check    "状態の行は 1 度（#1621 前は 2）" "$(state_count "$SANDBOX/4.log")" "1"
contains "当てる値を [input] で出す"       "[input] スリープ防止: mode=off, power=ac-only" "$SANDBOX/4.log"
absent   "当てる前の値を出さない"          "スリープ防止: mode=while-agents-running" "$SANDBOX/4.log"
absent   "答えた値に変え方の案内を重ねない" "設定変更: tako sleep-guard set" "$SANDBOX/4.log"
contains "変更の一覧には載る（別の節）"    "settings.sleep_guard_mode: while-agents-running -> off [input]" "$SANDBOX/4.log"
check    "出した値と書いた値が同じ"        "$(saved_mode)" "off"

echo "== 5) --answers で今と同じ値（2 回目）=="
RC=$(run_pipe "$SANDBOX/5.log" '{"launch_agent":"none","sleep_guard":{"mode":"off"}}' -- setup --yes --answers -)
check    "終了コード 0"                    "$RC" "0"
check    "状態の行は 1 度（#1621 前は 2）" "$(state_count "$SANDBOX/5.log")" "1"
check    "値はそのまま"                    "$(saved_mode)" "off"

echo "== 6) 端末あり（TTY）: 標準 / --review =="
mkenv
RC=$(run_pty "$SANDBOX/6a.log" -- setup)
check    "標準 TTY: 終了コード 0"          "$RC" "0"
check    "標準 TTY: 状態の行は 1 度"       "$(state_count "$SANDBOX/6a.log")" "1"
RC=$(run_pty "$SANDBOX/6b.log" -- setup --review)
check    "--review: 終了コード 0"          "$RC" "0"
check    "--review: 状態の行は 1 度"       "$(state_count "$SANDBOX/6b.log")" "1"
check    "--review: 対話は 1 度"           "$(menu_count "$SANDBOX/6b.log")" "1"
# `--review` と `--answers` は clap が同時指定を断るので、対話と答えが重なる経路は無い

echo "== 7) --answers で power だけ変える（mode は今の値のまま 1 行）=="
RC=$(run_pipe "$SANDBOX/7.log" '{"launch_agent":"none","sleep_guard":{"power":"always"}}' -- setup --yes --answers -)
check    "終了コード 0"                    "$RC" "0"
check    "状態の行は 1 度（#1621 前は 2）" "$(state_count "$SANDBOX/7.log")" "1"
contains "答えた power を [input] で出す"  "[input] スリープ防止: mode=while-agents-running, power=always" "$SANDBOX/7.log"

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
