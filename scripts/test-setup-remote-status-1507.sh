#!/usr/bin/env bash
# tako:run bash scripts/test-setup-remote-status-1507.sh
#
# #1507 の実経路テスト: `tako setup` の末尾に**リモート（スマホ）の状態どおりの 1 行**が出る。
#
# #1500 の棚卸し Z17（実測 R5 / R9）= 末尾は `スマホからリモート接続するには: tako remote setup`
# の固定文 1 行で、Tailscale が未導入でも公開済みでも同じだった。いまは
# `remote_setup::check_status`（読み取りだけ）の結果から状態を決め、未導入なら依存の導入口
# （`tako setup deps install`）を指す。ここでは状態ごとの 1 行・`--yes` / 非 TTY / TTY /
# `--answers` の各経路で止まらないこと・時間切れでも完走して知らせること・打ち切った子が
# 残らないこと・冪等を実プロセスで確かめる。
#
# 本番の ~/Library/Application Support/tako・~/.claude・実機の Tailscale / tmux には一切触れない
# （`env -i` で環境ごと差し替え、tailscale / brew / claude はスタブ。`TAKO_TAILSCALE_BIN` を
# 隔離 PATH の中へ向けるので実機の Tailscale は見えない）。立てた人工プロセスは
# **自分が起こしたものだけ** pid 指定で片付ける（pkill / killall の名前一致は禁止）。
#
# A/B: `TAKO_BIN=<#1507 前の tako>` で走らせると状態ごとの行の検査が落ちる。
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
count(){ grep -cF -- "$2" "$1" 2>/dev/null || true; }
# 末尾の 1 行（`スマホからの接続: ` で始まる行）。無ければ空
phone_line(){ grep -F "スマホからの接続: " "$1" 2>/dev/null | tail -1 | tr -d '\r'; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }
[ -f "$PTY" ] || { echo "PTY ドライバが無い: $PTY"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1507-XXXXXX")"
BIN="$SANDBOX/bin"; HOMEDIR="$SANDBOX/home"; DATADIR="$SANDBOX/d"

# 自分が起こした人工プロセスだけを片付ける（$SANDBOX を argv に持つものに限る）
sandbox_pids(){ /bin/ps -axo pid=,command= | grep -F "$SANDBOX" | grep -v grep | awk '{print $1}'; }
cleanup(){
  local pids; pids="$(sandbox_pids)"
  if [ -n "$pids" ]; then for p in $pids; do kill -9 "$p" 2>/dev/null; done; fi
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

# 返らない子。**自分の argv に $SANDBOX を持ったまま**眠る（`exec sleep` にすると argv が
# `sleep N` へ化けて「打ち切った子が残っていないか」の検査が空振りする = #1503 と同じ作法）
printf '#!/bin/sh\nwhile :; do /bin/sleep 1; done\n' > "$SANDBOX/hang"; chmod +x "$SANDBOX/hang"

# --- スタブ ------------------------------------------------------------------
# tailscale スタブ。$1 = 置き場 / $2 = 振る舞い
#   needslogin | stopped | ready（公開前）| published | hang-status | hang-version
write_tailscale(){
  local dest="$1" mode="$2"
  {
    echo '#!/bin/sh'
    echo "MODE='$mode'"
    echo "HANG='$SANDBOX/hang'"
    cat <<'EOF'
case "$MODE" in hang-version) exec "$HANG";; esac
for a in "$@"; do [ "$a" = "--version" ] && { echo "1.80.0"; exit 0; }; done
case "$MODE" in hang-status) exec "$HANG";; esac
if [ "$1" = "serve" ]; then
  if [ "$MODE" = "published" ]; then
    echo '{"TCP":{"443":{"HTTPS":true}},"Web":{"mac.tail1234.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18080"}}}}}'
  else
    echo '{}'
  fi
  exit 0
fi
case "$MODE" in
  needslogin) echo '{"BackendState":"NeedsLogin"}';;
  stopped)    echo '{"BackendState":"Stopped"}';;
  *) echo '{"BackendState":"Running","Version":"1.80.0","CertDomains":["mac.tail1234.ts.net"],"Self":{"DNSName":"mac.tail1234.ts.net."}}';;
esac
exit 0
EOF
  } > "$dest"
  chmod +x "$dest"
}

# mkenv <tailscale の振る舞い: none|needslogin|…> <brew: installs|noop|none>
mkenv(){
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める（`-l` を剥ぐ）
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  cat > "$BIN/claude" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "auth" ]; then echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0; fi
if [ "${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ] && [ "${2:-}" = "list" ]; then echo "tako: tako mcp serve - connected"; exit 0; fi
exit 0
EOF
  case "$2" in
    installs) # brew install tailscale で未ログインの tailscale が隔離 PATH へ入る
      { echo '#!/bin/sh'; echo "WRITE_TS='$SANDBOX/write-ts'"; cat <<'EOF'
[ "$1" = "install" ] || exit 0
echo "==> Fetching $2"
[ "$2" = "tailscale" ] && "$WRITE_TS" "$(dirname "$0")/tailscale" needslogin
exit 0
EOF
      } > "$BIN/brew";;
    noop) printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$BIN/brew";;  # 成功と言うが何も置かない
    none) : ;;                                                                  # brew 自体が無い
  esac
  [ "$1" = "none" ] || write_tailscale "$BIN/tailscale" "$1"
  chmod +x "$BIN"/* 2>/dev/null
}
# brew スタブから呼ぶための書き出し口（関数は子プロセスから呼べないので実体を作る）
{
  echo '#!/bin/bash'
  echo "SANDBOX='$SANDBOX'"
  declare -f write_tailscale
  echo 'write_tailscale "$1" "$2"'
} > "$SANDBOX/write-ts"
chmod +x "$SANDBOX/write-ts"

# 隔離した tako setup を**外側の締め切りつき**で走らせる（締め切りで殺されたら「固まった」）。
# 結果はグローバルへ置く（`$( )` の副シェルだと所要も締め切りの成否も捨てられる）
DEADLINE=60
RC=0; ELAPSED=0; KILLED=0; LEFTOVERS=""
# run <出力先> <stdin: null|pipe|pty> [pty の答え...] -- <tako の引数...>
run(){
  local out="$1" mode="$2"; shift 2
  local -a answers=()
  while [ "${1:-}" != "--" ]; do answers+=(--answer "$1"); shift; done; shift
  local start end pid killer
  local -a envs=(HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1
    SHELL="$BIN/isosh" TAKO_TAILSCALE_BIN="$BIN/tailscale" TAKO_TAILSCALE_SOCKET=
    TAKO_SOCKET="$SANDBOX/none.sock" TAKO_TMUX_SOCKET="$SANDBOX/t.sock"
    TAKO_DISCOVERY_DIR="$SANDBOX/nowhere"
    PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8)
  start="$(date +%s)"
  case "$mode" in
    pipe) printf '%s' '{}' | env -i ${envs[@]+"${envs[@]}"} "$TAKO" "$@" > "$out" 2>&1 & ;;
    pty)  env -i ${envs[@]+"${envs[@]}"} /usr/bin/python3 "$PTY" ${answers[@]+"${answers[@]}"} \
            --timeout "$DEADLINE" -- "$TAKO" "$@" > "$out" 2>&1 & ;;
    *)    env -i ${envs[@]+"${envs[@]}"} "$TAKO" "$@" < /dev/null > "$out" 2>&1 & ;;
  esac
  pid=$!
  KILLED=0
  ( sleep "$DEADLINE"; kill -9 "$pid" 2>/dev/null ) & killer=$!
  exec 3>&2 2>/dev/null
  wait "$pid"; RC=$?
  kill "$killer" 2>/dev/null; wait "$killer" 2>/dev/null
  exec 2>&3 3>&-
  end="$(date +%s)"; ELAPSED=$((end-start))
  if [ "$RC" -eq 137 ]; then KILLED=1; fi
  # **回収する前に控える**（tako が打ち切った子を自分で始末したかを見るため）
  LEFTOVERS="$(sandbox_pids)"
  if [ -n "$LEFTOVERS" ]; then for p in $LEFTOVERS; do kill -9 "$p" 2>/dev/null; done; fi
}

L_MISSING="スマホからの接続: Tailscale が未導入です。いま入れる: tako setup deps install   （brew install tailscale 相当）"
L_MANUAL="スマホからの接続: Tailscale が未導入です。導入方法: brew install tailscale（要 Homebrew: https://brew.sh） / App Store で「Tailscale」を検索、または brew install tailscale"
L_LOGIN="スマホからの接続: Tailscale にログインしていません。設定する: tako remote setup"
L_STOPPED="スマホからの接続: Tailscale の接続が有効ではありません（状態: Stopped）。設定する: tako remote setup"
L_READY="スマホからの接続: Tailscale は準備済みです（まだ公開していません）。設定する: tako remote setup"
L_PUBLISHED="スマホからの接続: 設定済みです（https://mac.tail1234.ts.net）"
L_UNKNOWN="スマホからの接続: 状態を確認できませんでした。確かめ直す: tako remote setup"
L_UNRUNNABLE="スマホからの接続: Tailscale を実行できませんでした。確かめ直す: tako remote setup"
NOTICE="  [確認できません] tailscale status --json（10 秒応答なし）。打ち切って次へ進みます"
OLD="スマホからリモート接続するには"

echo "#1507: setup の末尾にリモート（スマホ）の状態の 1 行 — sandbox: $SANDBOX"
echo

echo "== 1) 未導入 + --yes（brew は成功と言うが何も置かない）: 導入口を指し、2 度は聞かない =="
mkenv none noop
run "$SANDBOX/1.log" null -- setup --yes
check    "終了コード 0"                         "$RC" "0"
check    "状態どおりの 1 行"                   "$(phone_line "$SANDBOX/1.log")" "$L_MISSING"
check    "依存チェック段の導入は 1 回だけ"     "$(count "$SANDBOX/1.log" "--yes のため確認を省略してインストールします")" "1"
absent   "旧い固定文は出ない"                  "$OLD" "$SANDBOX/1.log"

echo "== 2) 未導入 + --yes（brew が実際に入れる）: 依存段で入った結果を末尾が読む =="
mkenv none installs
run "$SANDBOX/2.log" null -- setup --yes
check    "終了コード 0"                         "$RC" "0"
if [ -x "$BIN/tailscale" ]; then ok "依存チェック段が tailscale を入れた"; else ng "tailscale が入っていない"; fi
check    "入った後の状態（未ログイン）を言う"  "$(phone_line "$SANDBOX/2.log")" "$L_LOGIN"

echo "== 3) 未導入 + 非 TTY（--yes なし）: 入れずに導入口を指して完走 =="
mkenv none installs
run "$SANDBOX/3.log" null -- setup
check    "終了コード 0"                         "$RC" "0"
check    "締め切りに掛からない"                "$KILLED" "0"
check    "状態どおりの 1 行"                   "$(phone_line "$SANDBOX/3.log")" "$L_MISSING"
if [ -e "$BIN/tailscale" ]; then ng "非 TTY なのに入れた"; else ok "非 TTY では入れない"; fi

echo "== 4) 未導入 + brew が無い: 人が打つ手を依存チェック段と同じ文面で出す =="
mkenv none none
run "$SANDBOX/4.log" null -- setup --yes
check    "終了コード 0"                         "$RC" "0"
check    "状態どおりの 1 行"                   "$(phone_line "$SANDBOX/4.log")" "$L_MANUAL"

echo "== 5) 導入済み・状態ごと（非 TTY）=="
for pair in "needslogin|$L_LOGIN" "stopped|$L_STOPPED" "ready|$L_READY" "published|$L_PUBLISHED"; do
  mode="${pair%%|*}"; expected="${pair#*|}"
  mkenv "$mode" noop
  run "$SANDBOX/5-$mode.log" null -- setup
  check "$mode: 終了コード 0"          "$RC" "0"
  check "$mode: 状態どおりの 1 行"     "$(phone_line "$SANDBOX/5-$mode.log")" "$expected"
done

echo "== 6) tailscale status が返らない + --yes: 上限で打ち切り、知らせて完走 =="
mkenv hang-status noop
run "$SANDBOX/6.log" null -- setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒（tailscale の上限 10 秒）"
check    "締め切りに掛からない"                "$KILLED" "0"
check    "終了コード 0（完走する）"            "$RC" "0"
contains "何を何秒待ったかを出す（#1503 の文面）" "$NOTICE" "$SANDBOX/6.log"
check    "「起動していません」と言い切らない" "$(phone_line "$SANDBOX/6.log")" "$L_UNKNOWN"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は残らない"; fi

echo "== 7) tailscale --version すら返らない: 検出にも上限があり完走する =="
mkenv hang-version noop
run "$SANDBOX/7.log" null -- setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒"
check    "締め切りに掛からない"                "$KILLED" "0"
check    "終了コード 0"                         "$RC" "0"
check    "導入済みのものへ導入を勧めない"     "$(phone_line "$SANDBOX/7.log")" "$L_UNRUNNABLE"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は残らない"; fi

echo "== 8) dispatch SetupRun / MCP tako_setup と同条件（--yes --answers - をパイプ）=="
mkenv hang-status noop
run "$SANDBOX/8.log" pipe -- setup --yes --answers -
check    "締め切りに掛からない"                "$KILLED" "0"
check    "終了コード 0（dispatch は非 0 を失敗として返す）" "$RC" "0"
contains "知らせが stderr に載る（dispatch が parse_notices で拾う行）" "$NOTICE" "$SANDBOX/8.log"
check    "状態の 1 行"                          "$(phone_line "$SANDBOX/8.log")" "$L_UNKNOWN"

echo "== 9) 端末あり（PTY）+ 未導入: 聞くのは依存チェック段の 1 回だけ（N で入れない）=="
mkenv none installs
run "$SANDBOX/9.log" pty n -- setup
check    "終了コード 0"                         "$RC" "0"
check    "tailscale の [y/N] は 1 回だけ"       "$(count "$SANDBOX/9.log" "tailscale をインストールしますか？ [y/N]")" "1"
check    "状態どおりの 1 行"                   "$(phone_line "$SANDBOX/9.log")" "$L_MISSING"
if [ -e "$BIN/tailscale" ]; then ng "N なのに入れた"; else ok "N のものは入らない"; fi

echo "== 10) 冪等: 同じ環境で 2 回走らせても同じ 1 行 =="
mkenv needslogin noop
run "$SANDBOX/10a.log" null -- setup --yes
run "$SANDBOX/10b.log" null -- setup --yes
check    "2 回目も終了コード 0"                 "$RC" "0"
check    "1 回目と 2 回目で同じ行"             "$(phone_line "$SANDBOX/10b.log")" "$(phone_line "$SANDBOX/10a.log")"
check    "読み取りだけ（tailscale スタブは書き換わらない）" "$(grep -c NeedsLogin "$BIN/tailscale")" "1"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
