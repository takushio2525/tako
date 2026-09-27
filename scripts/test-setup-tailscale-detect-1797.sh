#!/usr/bin/env bash
# tako:run bash scripts/test-setup-tailscale-detect-1797.sh
#
# #1797 の実経路テスト: `tako setup` の依存チェック段が tailscale を**remote と同じ検出**で探す。
#
# 依存段（`setup_deps::resolve`）は tailscale を PATH だけで探していたので、CLI が PATH の外に
# ある App Store 版 / GUI 版を「見つかりません」と判定し、`--yes` では brew 版まで入れていた
# （2 系統の同居 = #1038 で tako remote が 502 になる条件）。同じ setup の末尾の 1 行
# （`remote_setup::check_status` → `tailscale::find_tailscale`）は正しく見つけるので、
# 1 回の実行の中で 2 つの検出が食い違っていた。ここでは:
#
#   - PATH 外 / PATH 内 / 未導入 / 両系統が見える の各状態で、依存段の判定と末尾の 1 行が揃う
#   - 導入済みのものへ brew（スタブ）を**呼ばない**（brew の呼び出しを記録して数える）
#   - `--version` が固まる / 非 0 で終わる CLI も「在る」と扱い、入れ直させない
#   - `status --json` が固まる・孫がパイプを握ったまま残る tailscale でも上限内に返り、
#     打ち切りは #1503 の文面で知らせる（待ちを `tako_core::probe` の 1 実装へ寄せた = #1797）
#   - `tako setup --check` / `tako setup deps` も同じ答えを返す
#   - `tako remote setup --yes` の [1/5] も「在るが動かない」を「未導入」と言わず、入れ直させない
#
# 本番の ~/Library/Application Support/tako・~/.claude・実機の Tailscale / tmux / brew には
# 一切触れない（`env -i` で環境ごと差し替え、tailscale / brew / claude はスタブ。
# **`TAKO_TAILSCALE_BIN` を必ず隔離側へ向ける**ので、実機の候補（/Applications・
# /opt/homebrew/bin・/usr/local/bin）は 1 度も試されない）。立てた人工プロセスは
# **自分が起こしたものだけ** pid 指定で片付ける（pkill / killall の名前一致は禁止）。
#
# A/B: `TAKO_BIN=<#1797 前の tako>` で走らせると 1) 4)〜11) が落ちる。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }
# 末尾の 1 行（`スマホからの接続: ` で始まる行）。無ければ空
phone_line(){ grep -F "スマホからの接続: " "$1" 2>/dev/null | tail -1 | tr -d '\r'; }
# 依存段の tailscale の行（`[OK] tailscale: …` か `[任意] tailscale: 見つかりません…`）
dep_line(){ grep -E '^  \[(OK|任意|不足)\] tailscale: ' "$1" 2>/dev/null | head -1; }
# brew スタブが `install tailscale` で呼ばれた回数
brew_ts_calls(){ grep -cx "install tailscale" "$SANDBOX/brew.log" 2>/dev/null || true; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1797-XXXXXX")"
BIN="$SANDBOX/bin"; HOMEDIR="$SANDBOX/home"; DATADIR="$SANDBOX/d"
# PATH の外の置き場（App Store 版の .app 同梱 CLI を模す）
APPS_TS="$SANDBOX/Applications/Tailscale.app/Contents/MacOS/Tailscale"

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
# tailscale スタブ。$1 = 置き場 / $2 = 振る舞い / $3 = 版（両系統を見分ける）
#   needslogin | ready（公開前）| nodaemon（status が非 0）| broken（--version が非 0）
#   hang-version | hang-status | orphan（孫がパイプを握ったまま親だけ終わる）
write_tailscale(){
  local dest="$1" mode="$2" version="${3:-1.80.0}"
  mkdir -p "$(dirname "$dest")"
  {
    echo '#!/bin/sh'
    echo "MODE='$mode'"
    echo "VERSION='$version'"
    echo "HANG='$SANDBOX/hang'"
    cat <<'EOF'
case "$MODE" in hang-version) exec "$HANG";; esac
for a in "$@"; do
  if [ "$a" = "--version" ]; then
    [ "$MODE" = "broken" ] && { echo "broken" >&2; exit 1; }
    echo "$VERSION"; exit 0
  fi
done
case "$MODE" in hang-status) exec "$HANG";; esac
# 孫を残して親だけ終わる（孫は stdout / stderr を握ったまま = `read_to_end` が EOF を見ない）
[ "$MODE" = "orphan" ] && "$HANG" &
if [ "$1" = "serve" ]; then echo '{}'; exit 0; fi
case "$MODE" in
  needslogin) echo '{"BackendState":"NeedsLogin"}';;
  nodaemon)   echo "failed to connect to local Tailscale service" >&2; exit 1;;
  *) echo "{\"BackendState\":\"Running\",\"Version\":\"$VERSION\",\"CertDomains\":[\"mac.tail1234.ts.net\"],\"Self\":{\"DNSName\":\"mac.tail1234.ts.net.\"}}";;
esac
exit 0
EOF
  } > "$dest"
  chmod +x "$dest"
}

# mkenv <brew: installs|noop>
#   installs = `brew install tailscale` で未ログインの tailscale が隔離 PATH（${BIN}）へ入る
#   noop     = 成功と言うが何も置かない
# どちらも呼ばれた引数を $SANDBOX/brew.log へ 1 行ずつ残す
mkenv(){
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR" "$SANDBOX/Applications" "$SANDBOX/brew.log"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  : > "$SANDBOX/brew.log"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める（`-l` を剥ぐ）
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  cat > "$BIN/claude" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "auth" ]; then echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0; fi
if [ "${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ] && [ "${2:-}" = "list" ]; then echo "tako: tako mcp serve - connected"; exit 0; fi
exit 0
EOF
  {
    echo '#!/bin/sh'
    echo "LOG='$SANDBOX/brew.log'"
    echo "WRITE_TS='$SANDBOX/write-ts'"
    echo "MODE='$1'"
    cat <<'EOF'
echo "$*" >> "$LOG"
[ "$1" = "install" ] || exit 0
echo "==> Fetching $2"
[ "$MODE" = "installs" ] && [ "$2" = "tailscale" ] && "$WRITE_TS" "$(dirname "$0")/tailscale" needslogin
exit 0
EOF
  } > "$BIN/brew"
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

# 隔離した tako を**外側の締め切りつき**で走らせる（締め切りで殺されたら「固まった」）。
# 結果はグローバルへ置く（`$( )` の副シェルだと所要も締め切りの成否も捨てられる）
DEADLINE=60
RC=0; ELAPSED=0; KILLED=0; LEFTOVERS=""
# run <出力先> <TAKO_TAILSCALE_BIN> <tako の引数...>（stdin は /dev/null = 非 TTY）
run(){
  local out="$1" ts_bin="$2"; shift 2
  local start end pid killer
  local -a envs=(HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1
    SHELL="$BIN/isosh" TAKO_TAILSCALE_BIN="$ts_bin" TAKO_TAILSCALE_SOCKET=
    TAKO_SOCKET="$SANDBOX/none.sock" TAKO_TMUX_SOCKET="$SANDBOX/t.sock"
    TAKO_DISCOVERY_DIR="$SANDBOX/nowhere"
    PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8)
  start="$(date +%s)"
  env -i ${envs[@]+"${envs[@]}"} "$TAKO" "$@" < /dev/null > "$out" 2>&1 &
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

L_LOGIN="スマホからの接続: Tailscale にログインしていません。設定する: tako remote setup"
L_READY="スマホからの接続: Tailscale は準備済みです（まだ公開していません）。設定する: tako remote setup"
L_DAEMON="スマホからの接続: Tailscale が起動していません。設定する: tako remote setup"
L_UNKNOWN="スマホからの接続: 状態を確認できませんでした。確かめ直す: tako remote setup"
L_UNRUNNABLE="スマホからの接続: Tailscale を実行できませんでした。確かめ直す: tako remote setup"
L_MISSING="スマホからの接続: Tailscale が未導入です。いま入れる: tako setup deps install   （brew install tailscale 相当）"
# 名札は実行ファイル名 + 引数（`probe::label`。パスは載せない = #927）。App Store 版の同梱 CLI は
# `Tailscale`（大文字）なので、PATH 外の置き場ではその名で出る
NOTICE="  [確認できません] Tailscale status --json（10 秒応答なし）。打ち切って次へ進みます"
NOT_FOUND="[任意] tailscale: 見つかりません"

echo "#1797: setup の依存段が tailscale を remote と同じ検出で探す — sandbox: $SANDBOX"
echo

echo "== 1) PATH 外（App Store 版を模す）+ --yes: 導入済みと判定し、brew を呼ばない =="
mkenv noop; write_tailscale "$APPS_TS" needslogin
run "$SANDBOX/1.log" "$APPS_TS" setup --yes
check    "終了コード 0"                               "$RC" "0"
check    "依存段は PATH 外の CLI を導入済みと言う"   "$(dep_line "$SANDBOX/1.log")" "  [OK] tailscale: $APPS_TS"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"
absent   "「見つかりません」と言わない"             "$NOT_FOUND" "$SANDBOX/1.log"
check    "末尾の 1 行と判定が揃う（未ログイン）"     "$(phone_line "$SANDBOX/1.log")" "$L_LOGIN"

echo "== 2) PATH 内（素の名前 tailscale）+ --yes: 絶対パスで表示し、brew を呼ばない =="
mkenv noop; write_tailscale "$BIN/tailscale" needslogin
run "$SANDBOX/2.log" "tailscale" setup --yes
check    "終了コード 0"                               "$RC" "0"
check    "依存段は PATH の実体を絶対パスで言う"     "$(dep_line "$SANDBOX/2.log")" "  [OK] tailscale: $BIN/tailscale"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"
check    "末尾の 1 行と判定が揃う（未ログイン）"     "$(phone_line "$SANDBOX/2.log")" "$L_LOGIN"

echo "== 3) 未導入 + --yes（brew が実際に入れる）: 1 回だけ入れ、入った結果を末尾が読む =="
mkenv installs
run "$SANDBOX/3.log" "$BIN/tailscale" setup --yes
check    "終了コード 0"                               "$RC" "0"
contains "依存段は未検出と言う"                     "$NOT_FOUND" "$SANDBOX/3.log"
check    "brew install tailscale は 1 回"            "$(brew_ts_calls)" "1"
contains "導入後の再検出で引ける"                   "  [OK] tailscale: $BIN/tailscale（インストール完了）" "$SANDBOX/3.log"
check    "末尾の 1 行は入った後の状態"               "$(phone_line "$SANDBOX/3.log")" "$L_LOGIN"

echo "== 3b) 未導入 + 非 TTY（--yes なし）: 入れずに導入口を指す（依存段と末尾が同じ答え）=="
mkenv installs
run "$SANDBOX/3b.log" "$BIN/tailscale" setup
check    "終了コード 0"                               "$RC" "0"
contains "依存段は未検出と言う"                     "$NOT_FOUND" "$SANDBOX/3b.log"
check    "brew を呼ばない"                            "$(brew_ts_calls)" "0"
check    "末尾は導入口を指す"                         "$(phone_line "$SANDBOX/3b.log")" "$L_MISSING"

echo "== 4) PATH 外 + --version が固まる: 在るものとして扱い、入れ直させない =="
mkenv noop; write_tailscale "$APPS_TS" hang-version
run "$SANDBOX/4.log" "$APPS_TS" setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒（--version の上限 10 秒 × 依存段 1 回 + 末尾 2 回 = check_status と導入口の判定）"
check    "締め切りに掛からない"                      "$KILLED" "0"
check    "終了コード 0"                               "$RC" "0"
check    "依存段は導入済みと言う"                     "$(dep_line "$SANDBOX/4.log")" "  [OK] tailscale: $APPS_TS"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"
check    "末尾は「実行できませんでした」"            "$(phone_line "$SANDBOX/4.log")" "$L_UNRUNNABLE"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は残らない"; fi

echo "== 5) PATH 外 + --version が非 0: 在るものとして扱い、入れ直させない =="
mkenv noop; write_tailscale "$APPS_TS" broken
run "$SANDBOX/5.log" "$APPS_TS" setup --yes
check    "終了コード 0"                               "$RC" "0"
check    "依存段は導入済みと言う"                     "$(dep_line "$SANDBOX/5.log")" "  [OK] tailscale: $APPS_TS"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"
check    "末尾は「実行できませんでした」"            "$(phone_line "$SANDBOX/5.log")" "$L_UNRUNNABLE"

echo "== 6) PATH 外 + status が非 0（デーモン未起動）: 導入済み・末尾は「起動していません」=="
mkenv noop; write_tailscale "$APPS_TS" nodaemon
run "$SANDBOX/6.log" "$APPS_TS" setup --yes
check    "終了コード 0"                               "$RC" "0"
check    "依存段は導入済みと言う"                     "$(dep_line "$SANDBOX/6.log")" "  [OK] tailscale: $APPS_TS"
check    "末尾は「起動していません」"                "$(phone_line "$SANDBOX/6.log")" "$L_DAEMON"

echo "== 7) PATH 外 + status --json が固まる: 上限で打ち切り、知らせて完走 =="
mkenv noop; write_tailscale "$APPS_TS" hang-status
run "$SANDBOX/7.log" "$APPS_TS" setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒（tailscale の上限 10 秒）"
check    "締め切りに掛からない"                      "$KILLED" "0"
check    "終了コード 0"                               "$RC" "0"
check    "依存段は導入済みと言う"                     "$(dep_line "$SANDBOX/7.log")" "  [OK] tailscale: $APPS_TS"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"
contains "何を何秒待ったかを出す（#1503 の文面）"   "$NOTICE" "$SANDBOX/7.log"
check    "「起動していません」と言い切らない"       "$(phone_line "$SANDBOX/7.log")" "$L_UNKNOWN"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は残らない"; fi

echo "== 8) 孫がパイプを握ったまま tailscale だけ終わる: 読み切りにも上限があり完走する =="
mkenv noop; write_tailscale "$APPS_TS" orphan
run "$SANDBOX/8.log" "$APPS_TS" setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒（締め切り ${DEADLINE} 秒）"
check    "締め切りに掛からない（join で固まらない）" "$KILLED" "0"
check    "終了コード 0"                               "$RC" "0"
check    "終わった子の出力は読めている（準備済み）" "$(phone_line "$SANDBOX/8.log")" "$L_READY"

echo "== 9) 両系統が見える（PATH に brew 版・PATH 外に App Store 版）: remote が使う側を言う =="
mkenv noop
write_tailscale "$BIN/tailscale" needslogin 1.70.0   # PATH 内（#1797 前の依存段が見ていた側）
write_tailscale "$APPS_TS" ready 1.80.0              # remote が使う側（TAKO_TAILSCALE_BIN）
run "$SANDBOX/9.log" "$APPS_TS" setup --yes
check    "終了コード 0"                               "$RC" "0"
check    "依存段は remote と同じ CLI を言う"         "$(dep_line "$SANDBOX/9.log")" "  [OK] tailscale: $APPS_TS"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"
check    "末尾はその CLI の状態（準備済み）"         "$(phone_line "$SANDBOX/9.log")" "$L_READY"

echo "== 10) setup --check / setup deps も同じ答え（PATH 外）=="
mkenv noop; write_tailscale "$APPS_TS" needslogin
run "$SANDBOX/10a.log" "$APPS_TS" setup --check
contains "--check は導入済みと言う"                 "[OK] tailscale: $APPS_TS" "$SANDBOX/10a.log"
absent   "--check は「見つかりません」と言わない"   "$NOT_FOUND" "$SANDBOX/10a.log"
run "$SANDBOX/10b.log" "$APPS_TS" setup deps --json
check    "deps --json の found は同じパス" \
  "$(/usr/bin/python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(next(x["found"] for x in d["deps"] if x["bin"]=="tailscale"))' "$SANDBOX/10b.log" 2>/dev/null)" \
  "$APPS_TS"
run "$SANDBOX/10c.log" "$APPS_TS" setup deps install --dep tailscale --json
check    "deps install は導入済みとして飛ばす" \
  "$(/usr/bin/python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["skipped"][0]["reason"])' "$SANDBOX/10c.log" 2>/dev/null)" \
  "already_installed"
check    "deps install でも brew を呼ばない"         "$(brew_ts_calls)" "0"

echo "== 11) remote setup --yes + PATH 外の CLI が非 0: 「未導入」と言わず、入れ直させない =="
mkenv noop; write_tailscale "$APPS_TS" broken
run "$SANDBOX/11.log" "$APPS_TS" remote setup --yes
if [ "$RC" -ne 0 ]; then ok "先へ進めないので非 0 で終わる"; else ng "非 0 で終わっていない（rc=${RC}）"; fi
contains "[1/5] は「実行できません」と言う"         "[1/5] Tailscale を検出中... 実行できません ($APPS_TS)" "$SANDBOX/11.log"
absent   "「未導入」と言わない"                     "未導入" "$SANDBOX/11.log"
absent   "導入すると言わない"                       "確認を省略してインストールします" "$SANDBOX/11.log"
check    "brew install tailscale を呼ばない"         "$(brew_ts_calls)" "0"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
