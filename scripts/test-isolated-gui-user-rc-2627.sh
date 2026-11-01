#!/bin/bash
# test-isolated-gui-user-rc-2627.sh — 隔離 GUI のペインの zsh が利用者の rc を読まない既定（#2627）の実経路テスト
#
# #1556 の worker が踏んだ: 隔離 GUI を止めたとき、ペインのログイン zsh が読んだ利用者の rc の `pyenv rehash`
# が途中で断たれ、pyenv の shim のロック（<pyenv root>/shims/.pyenv-shim）が残った。以後の新しいログイン zsh
# （本番のペインも）が 60 秒待たされた。
#
# 断ったのは stop_isolated_gui の TERM / KILL ではない（#2627 の段 1 の切り分け）: GUI が終わると PTY が
# 切れ、ペインへ SIGHUP が重ねて届く（zsh がジョブへ配る分と、zsh の終了でカーネルが前面のグループへ送る分）。
# rehash の EXIT の trap が 1 度目の HUP で走り始めた後、2 度目の HUP が trap の `rm` を断つ（trap の先頭で
# 以後の HUP を無視させると 3 回とも残らない）。GUI のどの終了でも起きるので、後片付けの待ちを延ばしても
# 直らない。だから隔離 GUI のペインには利用者の rc を読ませない（scripts/lib/isolated-gui.sh）。
#
# 本物の ~/.pyenv や利用者の rc は使わない: 砂場の dir を「利用者のホーム」に見立て（ISOLATED_GUI_REAL_HOME =
# ヘルパの見立ての口）、偽の pyenv（ロックの取り方は本物と同じ = EXIT の trap を先に置き、noclobber で
# ロックを作る）を呼ぶ .zshrc をそこへ置く。
#
#   ① 旧（TAKO_2627_LEGACY=1）× N: ペインの zsh がその .zshrc を読み、rehash がロックを持つ間に
#      stop_isolated_gui → ロックが残る回がある（競り合い。残った回は後から開くログイン zsh が待つ）
#   ② 新（既定）× N: 同じ HOME で .zshrc を読まない（印 0）・ロックが一度も出来ない・stop の後にも無い。
#      ペインの ZDOTDIR は砂場（ISOLATED_GUI_ZDOTDIR）・プロンプトは既定の固定の形
#   ③ opt-in（TAKO_ISO_USER_RC=1）: 利用者の rc を読む（印 1）。rehash を終えてから止めればロックは残らない
#   ④ 呼び手の一時の HOME（利用者のホームの外）の rc は従来どおり読み、砂場へ向けない
#
# 回数は TAKO_2627_RUNS（既定 3）。窓は tako-vd（面を用意できなければ 4 = 未実測）。本番の GUI / data dir /
# tmux には触らない。落とすのは自分で起こした pid だけ。描画は要らない（ファイル・env・ペインの文字だけを見る）。
#
# 使い方: bash scripts/test-isolated-gui-user-rc-2627.sh
set -uo pipefail

# 本番の値を最初に落とす（ZDOTDIR も: 利用者が export していれば旧の腕がそちらを読む）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_CLI TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS \
  TAKO_ORCHESTRATOR_ROLE TAKO_BACKEND_SOCKET TAKO_OSC_SINK TAKO_TMUX_SOCKET ZDOTDIR \
  TAKO_ISO_USER_RC TAKO_2627_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${TAKO_2627_RUNS:-3}"
# 偽の rehash がロックを持つ秒（stop がその間に来るように長め）
HOLD_SECS=20

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
note() { echo "  [--] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '$2' / 実際 '$3'）"; fi; }

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME と利用者のホームの見立てを差し替える前に（後だとツールチェーンを砂場へ取り直す）
isolated_gui_bins || exit 1
command -v jq >/dev/null || { echo "jq が要る"; exit 1; }

TMP="$(mktemp -d /tmp/t2627-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
cleanup() {
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  [ -n "${KEEP_TMP:-}" ] && { echo "一時 dir を残した: $TMP"; return; }
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# 「利用者のホーム」の見立て（砂場）と、偽の pyenv
H="$TMP/userhome"
PR="$H/.pyenv"
LOCK="$PR/shims/.pyenv-shim"
MARK="$H/rc-read.log"
mkdir -p "$PR/bin" "$PR/libexec"
# pyenv の入口（本物と同じく bash から rehash へ exec する）
cat > "$PR/bin/pyenv" <<'EOF'
#!/bin/bash
exec "$PYENV_ROOT/libexec/pyenv-$1"
EOF
# rehash（本物の取り方: EXIT の trap を先に置き、noclobber でロックを作り、取れなければ待つ）
cat > "$PR/libexec/pyenv-rehash" <<'EOF'
#!/bin/bash
set -e
LOCK="$PYENV_ROOT/shims/.pyenv-shim"
mkdir -p "$PYENV_ROOT/shims"
trap 'rm -f "${LOCK:?}"; echo "exit pid=$$" >> "$PYENV_ROOT/rehash.log"' EXIT
start=$SECONDS
until ( set -o noclobber; echo > "$LOCK" ) 2>/dev/null; do
  (( SECONDS <= start + ${PYENV_REHASH_TIMEOUT:-60} )) || { echo "pyenv: cannot rehash: $LOCK exists" >&2; exit 1; }
  sleep 0.1
done
echo "locked pid=$$" >> "$PYENV_ROOT/rehash.log"
sleep "${FAKE_REHASH_SECS:-0}"
EOF
chmod +x "$PR/bin/pyenv" "$PR/libexec/pyenv-rehash"
# 利用者の .zshrc の見立て（pyenv init の rehash の形）
cat > "$H/.zshrc" <<'EOF'
echo "rc-read pid=$$" >> "$HOME/rc-read.log"
export PYENV_ROOT="$HOME/.pyenv"
command "$PYENV_ROOT/bin/pyenv" rehash 2>/dev/null
PROMPT='user %1~ %% '
RPROMPT=''
EOF

# ここから先は砂場を利用者のホームとして扱う（ヘルパの見立ての口。本物のホームは読まない）
ISOLATED_GUI_REAL_HOME="$H"
export HOME="$H"

mark_count() { [ -f "$MARK" ] && wc -l < "$MARK" | tr -d ' ' || echo 0; }
lock_state() { [ -e "$LOCK" ] && echo 残る || echo 無い; }

# 1 回分を起こす: run_start <名前> [VAR=VAL…]（ペインは自分の data / 発見の置き場 / tmux を持つ）
run_start() {
  local name="$1" rc=0
  shift
  RUN="$TMP/$name"
  mkdir -p "$RUN/data"
  launch_isolated_gui "$RUN/app.log" TAKO_DATA_DIR="$RUN/data" TAKO_TMUX_SOCKET="t2627-$name-$$" \
    FAKE_REHASH_SECS="$HOLD_SECS" ${1+"$@"} || rc=$?
  [ "$rc" = 0 ] || exit "$rc"
  APP_PID="$ISOLATED_GUI_PID"
}
run_stop() { stop_isolated_gui "$APP_PID" 2>> "$RUN/stop.err"; APP_PID=""; }
# 待つ: wait_for <秒> <条件の関数>
wait_for() {
  local i
  for i in $(seq 1 $(($1 * 10))); do
    "$2" && return 0
    sleep 0.1
  done
  return 1
}
lock_exists() { [ -e "$LOCK" ]; }
# 後から開くログイン zsh が待つ秒（上限を 3 秒へ縮めて測る。待った rehash は本物と同じく抜けた後にロックを消す）
next_login_wait() {
  local t0
  t0=$(date +%s)
  env -i HOME="$H" PATH=/usr/bin:/bin TERM=dumb PYENV_REHASH_TIMEOUT=3 FAKE_REHASH_SECS=0 \
    /bin/zsh -l -i -c exit >/dev/null 2>&1
  echo $(($(date +%s) - t0))
}
first_pane() { iso_cli list 2>/dev/null | jq -r '.tabs[0].panes[0].id // empty'; }

echo "=== ① 旧（TAKO_2627_LEGACY=1）× ${RUNS}: ロックを持つ間に stop_isolated_gui ==="
LEFT=0
READ=0
for n in $(seq 1 "$RUNS"); do
  rm -f "${LOCK:?}" "${PR:?}/rehash.log"
  before=$(mark_count)
  run_start "legacy-$n" TAKO_2627_LEGACY=1
  if ! wait_for 60 lock_exists; then
    fail "旧 $n: 60 秒たってもロックが出来ない（ペインの zsh が .zshrc を読んでいない）"
    run_stop
    continue
  fi
  [ "$(mark_count)" -gt "$before" ] && READ=$((READ + 1))
  run_stop
  state=$(lock_state)
  wait_secs=$(next_login_wait)
  signals=$(grep -c 'を送る' "$RUN/stop.err" 2>/dev/null | tr -d ' ')
  echo "    旧 $n: stop の後のロック=${state}・後から開くログイン zsh の待ち=${wait_secs} 秒・stop の TERM / KILL の行=${signals}"
  [ "$state" = 残る ] && LEFT=$((LEFT + 1))
  rm -f "${LOCK:?}"
done
check_eq "旧: ペインの zsh が利用者のホーム（の見立て）の .zshrc を読む（${RUNS} 回）" "$RUNS" "$READ"
if [ "$LEFT" -gt 0 ]; then
  pass "旧: stop の後にロックが残った（${RUNS} 回中 ${LEFT} 回）= #2627 の再現"
else
  note "旧: ${RUNS} 回とも残らなかった（HUP の競り合いなので回によって変わる。再現は未実測）"
fi

echo
echo "=== ② 新（既定）× ${RUNS}: 同じ HOME・同じ手順 ==="
ZERO_READ=0
NO_LOCK=0
for n in $(seq 1 "$RUNS"); do
  rm -f "${LOCK:?}" "${PR:?}/rehash.log"
  before=$(mark_count)
  run_start "new-$n"
  zdot="$ISOLATED_GUI_ZDOTDIR"
  check_eq "新 $n: ペインの zsh へ向けた砂場はログの置き場の隣" "$RUN/isolated-zdotdir" "$zdot"
  if ! wait_isolated_gui "$RUN/app.log"; then
    fail "新 $n: 隔離 GUI へ繋がらない"
    run_stop
    continue
  fi
  P=$(first_pane)
  prompt_ready() { iso_cli read --pane "$P" 2>/dev/null | grep -q 'tako [^ ]* %'; }
  if wait_for 30 prompt_ready; then
    pass "新 $n: ペインのプロンプトが砂場の既定の .zshrc の固定の形（tako … %）"
  else
    fail "新 $n: 30 秒たってもペインのプロンプトが固定の形にならない"
  fi
  iso_cli send --pane "$P" "env > '$RUN/pane-env.tmp' && mv '$RUN/pane-env.tmp' '$RUN/pane-env.txt'" >/dev/null 2>&1
  pane_env_ready() { [ -s "$RUN/pane-env.txt" ]; }
  if wait_for 20 pane_env_ready; then
    check_eq "新 $n: ペインの ZDOTDIR は砂場" "$zdot" "$(sed -n 's/^ZDOTDIR=//p' "$RUN/pane-env.txt")"
    check_eq "新 $n: ペインの HOME は利用者のホーム（の見立て）のまま" "$H" "$(sed -n 's/^HOME=//p' "$RUN/pane-env.txt")"
  else
    fail "新 $n: ペインの env を書き出せない"
  fi
  # 旧の腕がロックを見た頃合いを過ぎるまで置く（rc を読むなら必ずロックが出来ている）
  sleep 2
  [ "$(mark_count)" = "$before" ] && ZERO_READ=$((ZERO_READ + 1))
  [ ! -e "$PR/rehash.log" ] && NO_LOCK=$((NO_LOCK + 1))
  run_stop
  state=$(lock_state)
  wait_secs=$(next_login_wait)
  echo "    新 $n: stop の後のロック=${state}・後から開くログイン zsh の待ち=${wait_secs} 秒"
  check_eq "新 $n: stop の後にロックが無い" "無い" "$state"
done
check_eq "新: ペインの zsh は利用者のホームの .zshrc を読まない（${RUNS} 回中 読まなかった回）" "$RUNS" "$ZERO_READ"
check_eq "新: rehash が一度も走らない = ロックが出来ない（${RUNS} 回）" "$RUNS" "$NO_LOCK"

echo
echo "=== ③ opt-in（TAKO_ISO_USER_RC=1）: 利用者の rc を読む ==="
rm -f "${LOCK:?}" "${PR:?}/rehash.log"
before=$(mark_count)
# tako:user-rc-ok opt-in が利用者の rc を読む形のままであることを確かめる腕（rc は砂場の見立て）
run_start optin TAKO_ISO_USER_RC=1 FAKE_REHASH_SECS=0
check_eq "opt-in: ペインの zsh へ砂場を向けない" "" "$ISOLATED_GUI_ZDOTDIR"
rehash_done() { grep -q '^exit pid=' "$PR/rehash.log" 2>/dev/null; }
if wait_for 60 rehash_done; then
  pass "opt-in: ペインの zsh が利用者のホーム（の見立て）の .zshrc を読み、rehash が走り終えた"
else
  fail "opt-in: 60 秒たっても rehash が走り終えない"
fi
check_eq "opt-in: .zshrc の印が 1 行増えた" "$((before + 1))" "$(mark_count)"
run_stop
check_eq "opt-in: rehash を終えてから止めればロックは残らない" "無い" "$(lock_state)"

echo
echo "=== ④ 呼び手の一時の HOME（利用者のホームの外）の rc は従来どおり ==="
CH="$TMP/callerhome"
mkdir -p "$CH"
printf '%s\n' 'echo caller >> "$HOME/caller-rc.log"' "PROMPT='caller %1~ %% '" > "$CH/.zshrc"
run_start callerhome HOME="$CH"
check_eq "呼び手の HOME: ペインの zsh へ砂場を向けない" "" "$ISOLATED_GUI_ZDOTDIR"
caller_read() { [ -s "$CH/caller-rc.log" ]; }
if wait_for 30 caller_read; then
  pass "呼び手の HOME: ペインの zsh が呼び手の .zshrc を読んだ"
else
  fail "呼び手の HOME: 30 秒たっても呼び手の .zshrc を読まない"
fi
run_stop

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
