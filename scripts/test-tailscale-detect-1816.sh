#!/usr/bin/env bash
# tako:run bash scripts/test-tailscale-detect-1816.sh
#
# #1816 の実経路テスト: tailscale の検出の続き（#1797 の後）。
#
#   ① 「在るが動かない」（`--version` が固まる / 非 0）を「未導入」と言い分ける。
#      MCP `tako_remote_setup` の run（= `tako remote setup --answers`）の検出の段は `unrunnable`、
#      `tako setup deps --json` は `runnable: false`
#   ② `tako setup` は依存段で 1 回引いた検出結果を末尾の 1 行まで使い回す。
#      `--version` が固まる CLI でも検出は 1 回（上限 10 秒）。#1816 前は 3 回（約 30 秒）
#   ③ 依存段・`tako setup deps`・`tako setup --check` は「在るが動かない」を `[OK]` と言わず `[警告]`
#   ④ 痩せた PATH（Dock から起こした `.app`）でも、ログインシェルの PATH にだけ在る tailscale
#      （MacPorts / nix）を見つける。ログインシェルが返らないときは「無い」と言い切らず導入へ進まない
#
# `check_status` の JSON そのもの（GUI のリモートパネル・MCP の check）は単体テスト
# `remote_setup::tests::check_statusはtailscaleの検出を4通りで言い分ける` が持つ（同じ関数を dispatch が返す）。
#
# 本番の data dir・~/.claude・実機の Tailscale / tmux / brew には一切触れない（`env -i` で環境ごと
# 差し替え、tailscale / brew / claude はスタブ。`TAKO_TAILSCALE_BIN` を必ず隔離側へ向けるので、
# 実機の候補（/Applications・/opt/homebrew/bin・/usr/local/bin）は 1 度も試されない。素の名前
# `tailscale` を渡す④も、隔離 PATH とスタブのログインシェルの中でしか引かない）。立てた人工プロセスは
# **自分が起こしたものだけ** pid 指定で片付ける（pkill / killall の名前一致は禁止）。
#
# A/B: `TAKO_BIN=<#1816 前の tako>` で走らせると ①〜④ の項目が落ちる。
set -uo pipefail
# check-health / `tako setup --check` は表示言語に従う（#2365）。判定は日本語の字面なので、OS の言語に
# 依らず日本語表示へ固定する（settings.json の language が system のときだけ効く）
export TAKO_LANG=ja

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }
phone_line(){ grep -F "スマホからの接続: " "$1" 2>/dev/null | tail -1 | tr -d '\r'; }
# 依存段の tailscale の行（行頭の印は問わない）
dep_line(){ grep -E '^  \[[^]]+\] tailscale: ' "$1" 2>/dev/null | head -1; }
# スタブ tailscale が `--version` で起こされた回数（= 検出の回数）
version_calls(){ grep -cx -- "--version" "$SANDBOX/ts.log" 2>/dev/null || true; }
brew_ts_calls(){ grep -cx "install tailscale" "$SANDBOX/brew.log" 2>/dev/null || true; }
# JSON の 1 値を取り出す（`$1` = ファイル / `$2` = Python の式。`d` が読んだ JSON）
json_get(){ /usr/bin/python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print($2)" "$1" 2>/dev/null; }
TS_DEP='[x for x in d["deps"] if x["bin"]=="tailscale"][0]'

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1816-XXXXXX")"
BIN="$SANDBOX/bin"; HOMEDIR="$SANDBOX/home"; DATADIR="$SANDBOX/d"
# ログインシェルの PATH にだけ在る置き場（MacPorts の /opt/local/bin・nix の profile を模す）
LOGINBIN="$SANDBOX/loginbin"
# PATH の外の置き場（App Store 版の .app 同梱 CLI を模す）
APPS_TS="$SANDBOX/Applications/Tailscale.app/Contents/MacOS/Tailscale"

sandbox_pids(){ /bin/ps -axo pid=,command= | grep -F "$SANDBOX" | grep -v grep | awk '{print $1}'; }
cleanup(){
  local pids; pids="$(sandbox_pids)"
  if [ -n "$pids" ]; then for p in $pids; do kill -9 "$p" 2>/dev/null; done; fi
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

# 返らない子。**自分の argv に $SANDBOX を持ったまま**眠る（打ち切った子の残りを数えるため = #1503）
printf '#!/bin/sh\nwhile :; do /bin/sleep 1; done\n' > "$SANDBOX/hang"; chmod +x "$SANDBOX/hang"

# tailscale スタブ。$1 = 置き場 / $2 = 振る舞い（needslogin | broken | hang-version）。
# 呼ばれた引数を $SANDBOX/ts.log へ 1 行ずつ残す（`--version` の行の数 = 検出の回数）
write_tailscale(){
  local dest="$1" mode="$2"
  mkdir -p "$(dirname "$dest")"
  {
    echo '#!/bin/sh'
    echo "MODE='$mode'"
    echo "HANG='$SANDBOX/hang'"
    echo "LOG='$SANDBOX/ts.log'"
    cat <<'EOF'
echo "$*" >> "$LOG"
case "$MODE" in hang-version) exec "$HANG";; esac
for a in "$@"; do
  if [ "$a" = "--version" ]; then
    [ "$MODE" = "broken" ] && { echo "broken" >&2; exit 1; }
    echo "1.80.0"; exit 0
  fi
done
if [ "$1" = "serve" ]; then echo '{}'; exit 0; fi
echo '{"BackendState":"NeedsLogin"}'
exit 0
EOF
  } > "$dest"
  chmod +x "$dest"
}

mkenv(){
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR" "$LOGINBIN" "${SANDBOX:?}/Applications"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR" "$LOGINBIN"
  : > "$SANDBOX/ts.log"; : > "$SANDBOX/brew.log"
  # 既定: ログインシェルも隔離 PATH のまま（`-l` を剥ぐ）
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  # ④: ログインシェルだけが $LOGINBIN を PATH へ足す（profile が MacPorts / nix の置き場を足すのを模す）
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; PATH="%s:$PATH"; export PATH; fi\nexec /bin/sh "$@"\n' \
    "$LOGINBIN" > "$BIN/loginsh"
  # ④: ログインシェルが返らない（rc が入力を待つ）
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then exec "%s"; fi\nexec /bin/sh "$@"\n' \
    "$SANDBOX/hang" > "$BIN/hangsh"
  cat > "$BIN/claude" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "auth" ]; then echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0; fi
if [ "${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ] && [ "${2:-}" = "list" ]; then echo "tako: tako mcp serve - connected"; exit 0; fi
exit 0
EOF
  printf '#!/bin/sh\necho "$*" >> "%s"\nexit 0\n' "$SANDBOX/brew.log" > "$BIN/brew"
  chmod +x "$BIN"/*
}

# 隔離した tako を**外側の締め切りつき**で走らせる（締め切りで殺されたら「固まった」）
DEADLINE=60
RC=0; ELAPSED=0; KILLED=0; LEFTOVERS=""
# run <出力先> <TAKO_TAILSCALE_BIN> <SHELL> <tako の引数...>（stdin は /dev/null = 非 TTY）
run(){
  local out="$1" ts_bin="$2" login_shell="$3"; shift 3
  local start end pid killer
  local -a envs=(HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1
    SHELL="$login_shell" TAKO_TAILSCALE_BIN="$ts_bin" TAKO_TAILSCALE_SOCKET=
    TAKO_SOCKET="$SANDBOX/none.sock" TAKO_TMUX_SOCKET="$SANDBOX/t.sock"
    TAKO_DISCOVERY_DIR="$SANDBOX/nowhere" TAKO_LOGIN_SHELL_TIMEOUT_SECS=1
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

L_UNRUNNABLE="スマホからの接続: Tailscale を実行できませんでした。確かめ直す: tako remote setup"
L_LOGIN="スマホからの接続: Tailscale にログインしていません。設定する: tako remote setup"
L_UNKNOWN="スマホからの接続: 状態を確認できませんでした。確かめ直す: tako remote setup"
HINT="入れ直す前に、Tailscale の起動し直しか更新を試してください"
unrunnable_line(){ echo "  [警告] tailscale: 在りますが実行できません（$1）"; }

echo "#1816: tailscale の検出の続き — sandbox: $SANDBOX"
echo

echo "== 1) --version が固まる CLI + setup --yes: 検出は 1 回（②）・依存段は [警告]（③）=="
mkenv; write_tailscale "$APPS_TS" hang-version
run "$SANDBOX/1.log" "$APPS_TS" "$BIN/isosh" setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒・--version の起動 $(version_calls) 回（#1816 前は 3 回 = 約 30 秒）"
check    "締め切りに掛からない"                        "$KILLED" "0"
check    "終了コード 0"                                 "$RC" "0"
check    "検出（--version の起動）は 1 回"              "$(version_calls)" "1"
if [ "$ELAPSED" -lt 25 ]; then ok "所要は検出 1 回分（${ELAPSED} 秒 < 25 秒）"; else ng "所要が検出 1 回分を超える（${ELAPSED} 秒）"; fi
check    "依存段は [OK] と言わない（在るが動かない）"   "$(dep_line "$SANDBOX/1.log")" "$(unrunnable_line "$APPS_TS")"
contains "次の一手を添える"                             "      $HINT" "$SANDBOX/1.log"
check    "brew install tailscale を呼ばない"            "$(brew_ts_calls)" "0"
check    "末尾は依存段の答えを使い回す"                 "$(phone_line "$SANDBOX/1.log")" "$L_UNRUNNABLE"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は残らない"; fi

echo "== 2) --version が非 0 の CLI: remote setup --answers（= MCP tako_remote_setup の run）は未導入と言わない（①）=="
mkenv; write_tailscale "$APPS_TS" broken
run "$SANDBOX/2.log" "$APPS_TS" "$BIN/isosh" remote setup --answers '{}'
check    "終了コード 0"                                 "$RC" "0"
check    "検出の段の status は unrunnable"              "$(json_get "$SANDBOX/2.log" 'd["steps"][0]["status"]')" "unrunnable"
check    "検出の段で止まる（先へ進まない）"             "$(json_get "$SANDBOX/2.log" 'len(d["steps"])')" "1"
check    "検出の段の文面にパスと次の一手"               "$(json_get "$SANDBOX/2.log" 'd["steps"][0]["message"]')" \
  "Tailscale は入っていますが実行できません（${APPS_TS}）。${HINT}"
absent   "「未導入」と言わない"                         "未導入" "$SANDBOX/2.log"

echo "== 3) --version が非 0 の CLI: setup deps --json / setup deps / setup --check も [OK] と言わない（①③）=="
run "$SANDBOX/3a.log" "$APPS_TS" "$BIN/isosh" setup deps --json
check    "deps --json の found は在る CLI"              "$(json_get "$SANDBOX/3a.log" "$TS_DEP[\"found\"]")" "$APPS_TS"
check    "deps --json の runnable は false"             "$(json_get "$SANDBOX/3a.log" "$TS_DEP[\"runnable\"]")" "False"
run "$SANDBOX/3b.log" "$APPS_TS" "$BIN/isosh" setup deps
check    "setup deps は [警告]"                         "$(dep_line "$SANDBOX/3b.log")" "$(unrunnable_line "$APPS_TS")"
run "$SANDBOX/3c.log" "$APPS_TS" "$BIN/isosh" setup --check
contains "setup --check は [警告]"                      "[警告] tailscale: 在りますが実行できません（${APPS_TS}）" "$SANDBOX/3c.log"
absent   "setup --check は [OK] と言わない"             "[OK] tailscale" "$SANDBOX/3c.log"
run "$SANDBOX/3d.log" "$APPS_TS" "$BIN/isosh" setup deps install --dep tailscale --json
check    "deps install は導入済みとして飛ばす"          "$(json_get "$SANDBOX/3d.log" 'd["skipped"][0]["reason"]')" "already_installed"
check    "brew install tailscale を呼ばない"            "$(brew_ts_calls)" "0"

echo "== 4) --version が固まる CLI + remote setup --yes: [1/5] の検出も 1 回（②）=="
mkenv; write_tailscale "$APPS_TS" hang-version
run "$SANDBOX/4.log" "$APPS_TS" "$BIN/isosh" remote setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒・--version の起動 $(version_calls) 回（#1816 前は 2 回 = 約 20 秒）"
if [ "$RC" -ne 0 ]; then ok "先へ進めないので非 0 で終わる"; else ng "非 0 で終わっていない（rc=${RC}）"; fi
check    "検出（--version の起動）は 1 回"              "$(version_calls)" "1"
contains "[1/5] は「実行できません」と言う"             "[1/5] Tailscale を検出中... 実行できません ($APPS_TS)" "$SANDBOX/4.log"
absent   "「未導入」と言わない"                         "未導入" "$SANDBOX/4.log"

echo "== 5) 痩せた PATH + ログインシェルの PATH にだけ在る tailscale（MacPorts / nix を模す）を見つける（④）=="
mkenv; write_tailscale "$LOGINBIN/tailscale" needslogin
run "$SANDBOX/5a.log" "tailscale" "$BIN/loginsh" setup deps --json
check    "deps --json の found はログインシェルの置き場" "$(json_get "$SANDBOX/5a.log" "$TS_DEP[\"found\"]")" "$LOGINBIN/tailscale"
check    "deps --json の runnable は true"              "$(json_get "$SANDBOX/5a.log" "$TS_DEP[\"runnable\"]")" "True"
run "$SANDBOX/5b.log" "tailscale" "$BIN/loginsh" remote setup --answers '{}'
check    "remote run の検出の段は ok"                   "$(json_get "$SANDBOX/5b.log" 'd["steps"][0]["status"]')" "ok"
check    "remote run はその CLI で先へ進む（未ログイン）" \
  "$(json_get "$SANDBOX/5b.log" '[s["message"] for s in d["steps"] if s["step"]=="tailscale_status"][0]')" \
  "Tailscale にログインしていない（tailscale up でブラウザ認証）"
run "$SANDBOX/5c.log" "tailscale" "$BIN/loginsh" setup --yes
check    "終了コード 0"                                 "$RC" "0"
check    "依存段は導入済みと言う"                       "$(dep_line "$SANDBOX/5c.log")" "  [OK] tailscale: $LOGINBIN/tailscale"
check    "brew install tailscale を呼ばない"            "$(brew_ts_calls)" "0"
check    "末尾はその CLI の状態（未ログイン）"          "$(phone_line "$SANDBOX/5c.log")" "$L_LOGIN"
echo "  -- 対照: ログインシェルの PATH にも無い"
mkenv
run "$SANDBOX/5d.log" "tailscale" "$BIN/loginsh" setup deps --json
check    "deps --json の found は null"                 "$(json_get "$SANDBOX/5d.log" "$TS_DEP[\"found\"]")" "None"
check    "打ち切りではない（lookup_timeout は null）"   "$(json_get "$SANDBOX/5d.log" "$TS_DEP[\"lookup_timeout\"]")" "None"

echo "== 6) ログインシェルが返らない + どこにも無い: 無いと言い切らず、導入へ進まない（④ / #1533 と同じ扱い）=="
mkenv
run "$SANDBOX/6a.log" "tailscale" "$BIN/hangsh" setup deps --json
check    "deps --json の found は null"                 "$(json_get "$SANDBOX/6a.log" "$TS_DEP[\"found\"]")" "None"
check    "打ち切りを lookup_timeout で運ぶ"             "$(json_get "$SANDBOX/6a.log" "$TS_DEP[\"lookup_timeout\"][\"waited_secs\"]")" "1"
run "$SANDBOX/6b.log" "tailscale" "$BIN/hangsh" remote setup --answers '{}'
check    "remote run の検出の段は unknown"              "$(json_get "$SANDBOX/6b.log" 'd["steps"][0]["status"]')" "unknown"
absent   "remote run は「未導入」と言わない"            "未導入" "$SANDBOX/6b.log"
run "$SANDBOX/6c.log" "tailscale" "$BIN/hangsh" setup --yes
echo "  [実測] 所要 ${ELAPSED} 秒（ログインシェルの上限 1 秒 × 依存ごと）"
check    "締め切りに掛からない"                        "$KILLED" "0"
check    "終了コード 0"                                 "$RC" "0"
check    "依存段は「確認できません」"                   "$(dep_line "$SANDBOX/6c.log")" "  [警告] tailscale: 確認できません（ログインシェルが 1 秒で応答なし）"
absent   "「見つかりません」と言わない"                 "[任意] tailscale: 見つかりません" "$SANDBOX/6c.log"
check    "brew install tailscale を呼ばない"            "$(brew_ts_calls)" "0"
check    "末尾は「確認できませんでした」"               "$(phone_line "$SANDBOX/6c.log")" "$L_UNKNOWN"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は残らない"; fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
