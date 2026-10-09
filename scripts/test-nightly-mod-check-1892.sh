#!/usr/bin/env bash
# test-nightly-mod-check-1892.sh — 夜間リリースの「tako mod の検査」のモックテスト（#1892）
#
# 一時ディレクトリに「origin（bare）+ 作業リポ（mod 入り）」を作り、実物の nightly-release.sh と
# check-claude-mod.sh を **launchd と同じ /bin/bash（3.2）・同じ env（HOME だけ）で**走らせる。
# 本番には一切触らない:
#   - HOME を一時ディレクトリへ差し替える（ログ・ロック・検査の記録が隔離される）
#   - claude はスタブ（hooks/hooks.json が名指すモジュールが無ければ落ちる = 実物の
#     `claude plugin validate` / `test` の path-not-found と同じ判定。版・固まる・落ちるを切り替える）
#   - release.sh / gh / cargo はスタブ、osascript は通知の本文を記録するだけ
#   - push 先は一時ディレクトリの bare リポ（ネットワークに出ない）
#
# 見るもの（Issue #1892 の受け入れ条件）:
#   Test 0  assert の自己検査（#1933）: パイプの容量を超える出力の先頭近くにある文字列を取りこぼさない
#   Test 1  正常な mod → 合格・通知なし・claude の版を記録・リリースの判定へ進む（A）
#   Test 2  壊れた登録を注入 → 不合格・通知が出る・リリースの判定へ進む（B）→ 戻すと合格（A/B）
#   Test 3  Claude Code の更新で落ちた → 前回合格の版と比べて「更新で壊れた」と言う
#   Test 4  claude が無い → 未実測と出して飛ばす・通知しない・夜間リリースは止まらない
#   Test 5  validate / --version が固まる → 上限で打ち切って通知・子プロセスを残さない
#   Test 6  実走: mod の検査が落ちてもリリースはタグ + Release まで進む（止めない判断）
#   Test 7  変更が無い夜も検査は回る（Claude Code の更新は tako の変更と無関係に来る）
#   Test 8  check-claude-mod.sh 単体の終了コード（記録しない既定・ref が無い・mod が無い）
#   Test 9  利用者の Claude Code の設定に触らない（設定 dir は使い捨て・mtime が前後で一致）
#   Test 10 実物の claude（あれば）: 本物の mod で合格・壊れた写しで不合格・利用者の設定の mtime が一致
#           （claude が無い CI では「未実測」と出して飛ばす）
#   Test 11 文言一致の自己検査（#1903）: validate / test の注記の文言が変わったら落ちる
#
# 失敗時の手掛かり（#1933）: assert が落ちたら、その環境でまだ出していない段（夜間リリース /
# 検査スクリプトの 1 回の実行）の出力と終了コード、記録ファイル、通知のスタブの記録、claude の
# スタブの呼び出しをログへ出す（CI のログだけで「その段で何が起きたか」を追えるように）。
#
# 使い方: bash scripts/test-nightly-mod-check-1892.sh
#   TAKO_1933_INJECT=<version|validate|test> を付けると、Test 2 の注入前の 1 回だけその段の claude を
#   1 回落とす（CI で 1 回だけ出た「注入前の合格が見つからず通知が 2 件」と同じ 5 件の FAIL になる。
#   手掛かりが出ることの確認用）
#   TAKO_1933_INJECT=first-exec-slow を付けると、各環境の claude のスタブの最初の 1 回の起動が 3 秒かかる
#   （作りたての実行ファイルの初回起動の遅さ）。make_env の温めがそれを吸うので緑のまま。
#   TAKO_1933_LEGACY=1 を足すと温めないので、Test 5 の上限 2 秒で --version が打ち切られて落ちる（A/B）
set -uo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
PASS=0
FAIL=0
UNMEASURED=0

assert_eq() {
  if [[ "$2" = "$3" ]]; then
    echo "  PASS: $1"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $1 (expected=[$2], actual=[$3])"
    FAIL=$((FAIL + 1))
    dump_stages
  fi
}

# 含む / 含まないはパイプを使わずに判定する（#1933）。`printf | grep -q` は pipefail の下で、grep が
# 一致して先に抜けると書き残しのある printf が EPIPE（SIGPIPE が既定ならシグナル死）になってパイプ全体が
# 非 0 になり、含んでいるのに「無い」（assert_not_contains では「含まない」で偽の合格）と判定する。
# 出力が write 1 回に収まらず、書き手が間で横取りされたときだけ起きる = 負荷に依存する間欠的な赤
assert_contains() {
  if [[ "$2" == *"$3"* ]]; then
    echo "  PASS: $1"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $1 (not found: '$3')"
    FAIL=$((FAIL + 1))
    dump_stages
  fi
}

assert_not_contains() {
  if [[ "$2" == *"$3"* ]]; then
    echo "  FAIL: $1 (found but should not: '$3')"
    FAIL=$((FAIL + 1))
    dump_stages
  else
    echo "  PASS: $1"
    PASS=$((PASS + 1))
  fi
}

unmeasured() {
  echo "  未実測: $1"
  UNMEASURED=$((UNMEASURED + 1))
}

SANDBOX=$(mktemp -d "${TMPDIR:-/tmp}/tako-1892-XXXXXX")
SANDBOX=$(cd "$SANDBOX" && pwd)

# 自分が起こしたスタブのプロセスだけを数える・片付ける（$SANDBOX を argv に持つものに限る。
# pkill / killall の名前一致は本番にも当たるので使わない）
sandbox_pids() {
  /bin/ps -axo pid=,command= | grep -F "$1" | grep -v grep | awk '{ print $1 }'
}
cleanup() {
  local p
  for p in $(sandbox_pids "$SANDBOX"); do kill -9 "$p" 2>/dev/null; done
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

# --- 失敗時の手掛かり（#1933）-------------------------------------------------
# 段は $( ) の中で走り、親の変数を書けないので、出力・終了コード・環境 dir を $STAGES へ連番の
# ファイルで残す（段は 1 本ずつ順に走るので連番の読み書きは競合しない）。assert が落ちたら
# dump_stages が、最後の段と同じ環境でまだ出していない段をまとめて出す（同じ段を 2 度出さない。
# 通った assert では何も出さない）
STAGES="$SANDBOX/stages"
mkdir -p "$STAGES"
DUMPED=0

# stage <環境 dir（無ければ -）> <コマンド...>: コマンドを走らせて出力をそのまま返し、写しを残す
# （tee で写すのは、段の標準出力をこれまでどおりパイプのままにして、検査する条件を変えないため）
stage() {
  local dir="$1" n rc
  shift
  n=$(($(cat "$STAGES/seq" 2> /dev/null || echo 0) + 1))
  echo "$n" > "$STAGES/seq"
  printf '%s\n%s\n' "$dir" "$*" > "$STAGES/$n.meta"
  "$@" 2>&1 | tee "$STAGES/$n.out"
  rc=${PIPESTATUS[0]}
  echo "$rc" > "$STAGES/$n.rc"
  return "$rc"
}

# 一時 dir の実パスは <sandbox>、ホームは ~ に伏せる（ログを読みやすく。実ユーザー名を出さない）
mask_paths() { LC_ALL=C sed -e "s#${SANDBOX}#<sandbox>#g" -e "s#${HOME}#~#g" "$@"; }

dump_file() {
  if [[ -f "$2" ]]; then
    echo "    ---- $1"
    mask_paths -e 's/^/    | /' "$2"
  else
    echo "    ---- $1: 無い"
  fi
}

dump_stages() {
  local last n first dir
  last=$(cat "$STAGES/seq" 2> /dev/null || echo 0)
  if [[ "$last" -le "$DUMPED" ]]; then
    [[ "$last" -eq 0 ]] || echo "    （この段の手掛かりは上に出した）"
    return 0
  fi
  dir=$(head -1 "$STAGES/$last.meta" 2> /dev/null || true)
  first=$last
  while [[ "$first" -gt $((DUMPED + 1)) ]] && [[ "$(head -1 "$STAGES/$((first - 1)).meta" 2> /dev/null || true)" = "$dir" ]]; do
    first=$((first - 1))
  done
  echo "    ==== 手掛かり（#1933）: 環境 ${dir##*/} の段 ${first}〜${last} ===="
  n=$first
  while [[ "$n" -le "$last" ]]; do
    dump_file "段 ${n} の出力（exit $(cat "$STAGES/$n.rc" 2> /dev/null || echo '?')）: $(sed -n 2p "$STAGES/$n.meta" | mask_paths)" "$STAGES/$n.out"
    n=$((n + 1))
  done
  if [[ -d "$dir" ]]; then
    dump_file "記録ファイル（home/.claude-orchestrator/state/tako-mod-check）" "$dir/home/.claude-orchestrator/state/tako-mod-check"
    dump_file "通知のスタブの記録（notify.log）" "$dir/notify.log"
    dump_file "claude のスタブの呼び出し（stub/calls = cwd|設定 dir|自動更新の停止|引数）" "$dir/stub/calls"
    dump_file "claude のスタブの温め（make_env の初回起動）" "$dir/stub/warm"
    echo "    ---- claude のスタブの切り替え（stub/*_mode）: $(cd "$dir/stub" 2> /dev/null && for f in *_mode; do [[ -f "$f" ]] && printf '%s=%s ' "$f" "$(cat "$f")"; done)（無ければ既定の auto）"
  fi
  echo "    ==== 手掛かりここまで ===="
  DUMPED=$last
}

# --- モック環境の構築 ---------------------------------------------------------
# $1 = 名前 / $2 = 1 なら origin/main をタグと同一にする（変更ゼロの夜）

make_env() {
  local dir="$SANDBOX/$1" no_change="${2:-0}" stub warm_rc
  mkdir -p "$dir/home/.local/bin" "$dir/home/.claude" "$dir/repo/scripts/lib" "$dir/stub"

  for stub in cargo gh; do
    printf '#!/bin/sh\nexit 0\n' > "$dir/home/.local/bin/$stub"
    chmod +x "$dir/home/.local/bin/$stub"
  done
  # 通知は本文を記録するだけ（テスト中に画面へ出さない）
  printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s/notify.log"\nexit 0\n' "$dir" > "$dir/home/.local/bin/osascript"
  chmod +x "$dir/home/.local/bin/osascript"

  # 固まる claude（子を持ったまま眠る = 実物の test が子でテストを走らせる形）
  printf '#!/bin/sh\n"%s/stub/hang-child" &\nwhile :; do /bin/sleep 1; done\n' "$dir" > "$dir/stub/hang"
  printf '#!/bin/sh\nwhile :; do /bin/sleep 1; done\n' > "$dir/stub/hang-child"
  chmod +x "$dir/stub/hang" "$dir/stub/hang-child"
  echo "2.1.294" > "$dir/stub/version"

  # claude のスタブ。呼ばれ方（cwd・設定 dir・自動更新の停止・引数）を記録し、実物と同じく
  # 設定 dir へ .claude.json を書く。<段>_mode が auto（既定）/ fail / hang
  printf '#!/bin/sh\nS="%s/stub"\n' "$dir" > "$dir/home/.local/bin/claude"
  cat >> "$dir/home/.local/bin/claude" <<'STUB'
# 初回起動の遅さの注入（#1933。TAKO_1933_INJECT=first-exec-slow）: このスタブの最初の 1 回だけ 3 秒かかる
if [ -f "$S/first_exec_slow" ] && [ ! -f "$S/first_exec_done" ]; then : > "$S/first_exec_done"; /bin/sleep 3; fi
# make_env が温めるための起動（呼び出しの記録に数えない）
[ "${1:-}" = --tako-warm ] && exit 0
printf '%s|%s|%s|%s\n' "$(pwd)" "${CLAUDE_CONFIG_DIR:-}" "${DISABLE_AUTOUPDATER:-}" "$*" >> "$S/calls"
if [ -n "${CLAUDE_CONFIG_DIR:-}" ] && [ -d "$CLAUDE_CONFIG_DIR" ]; then echo '{}' > "$CLAUDE_CONFIG_DIR/.claude.json"; fi
mode_of() { cat "$S/$1_mode" 2>/dev/null || echo auto; }
if [ "$1" = "--version" ]; then
  [ "$(mode_of version)" = hang ] && exec "$S/hang"
  if [ "$(mode_of version)" = fail-once ]; then rm -f "$S/version_mode"; echo "✘ forced failure once (--version)"; exit 1; fi
  echo "$(cat "$S/version") (Claude Code)"
  exit 0
fi
[ "$1" = plugin ] || exit 2
sub="$2"
shift 2
[ "${1:-}" = --strict ] && shift
dir="$1"
case "$(mode_of "$sub")" in
  hang) exec "$S/hang" ;;
  fail) echo "✘ forced failure ($sub)"; exit 1 ;;
  fail-once) rm -f "$S/${sub}_mode"; echo "✘ forced failure once ($sub)"; exit 1 ;;
  # 実物の validate はゲートの .catch 抜けを注記に出すだけで合格させる（2.1.294 で実測）
  nocatch) printf '  ❯ ./register.ts gating hook with .catch: classic.Stop\n  ❯ ./register.ts gating hook without .catch: tool.call\n✔ Validation passed\n'; exit 0 ;;
  zero) printf ' 0 pass\n 0 fail\nRan 0 tests across 0 files.\n'; exit 0 ;;
  one) printf ' 1 pass\n 0 fail\nRan 1 test across 1 file.\n'; exit 0 ;;
  # Claude Code の更新で注記の文言が変わった（#1903 の自己検査の相手。.catch 抜けも 0 本も含む）
  reworded)
    if [ "$sub" = validate ]; then
      printf '  ❯ ./register.ts gate tool.call: no catch handler\n✔ Validation passed\n'
    else
      printf ' 0 pass\n 0 fail\nExecuted 0 tests in 0 files.\n'
    fi
    exit 0
    ;;
esac
# auto: hooks.json が名指すモジュールがすべて在れば合格（実物の path-not-found と同じ判定）
for m in $(sed -n 's/.*"modules": *\[\(.*\)\].*/\1/p' "$dir/hooks/hooks.json" | tr ',' ' ' | tr -d '"'); do
  if [ ! -f "$dir/hooks/$m" ]; then
    if [ "$sub" = validate ]; then
      printf '✘ Found 1 error:\n  ❯ modules.%s: %s/hooks/%s: no such file\n✘ Validation failed\n' "$m" "$dir" "$m"
    else
      printf 'claude plugin test: %s: no hooks module to load (path-not-found)\n' "$dir"
    fi
    exit 1
  fi
done
# 実物の validate はゲートになるフックを 1 本ずつ `gating hook with .catch: <event>` と申告する（2.1.294 で実測）
if [ "$sub" = validate ]; then
  printf '  ❯ ./register.ts gating hook with .catch: tool.call\n✔ Validation passed\n'
else
  printf ' 3 pass\n 0 fail\nRan 3 tests across 1 file.\n'
fi
exit 0
STUB
  chmod +x "$dir/home/.local/bin/claude"

  # 作りたての実行ファイルは macOS の初回起動が遅い（実測: 作った直後の 1 回目は中央 219 ms・最大 711 ms、
  # 2 回目は 9 ms。負荷下では 2 秒を超え、Test 5 の上限 2 秒で固まらせていない --version が打ち切られた =
  # #1933）。本番の claude は作りたてではないので、ここで 1 回起動して温め、検査の段の時間に初回の遅さを
  # 入れない。結果は stub/warm に残して手掛かりに出す（温めが落ちたら stderr にも出す）。
  # TAKO_1933_LEGACY=1 で温めない（A/B 用）
  if [[ "${TAKO_1933_INJECT:-}" = first-exec-slow ]]; then : > "$dir/stub/first_exec_slow"; fi
  if [[ "${TAKO_1933_LEGACY:-}" != 1 ]]; then
    warm_rc=0
    "$dir/home/.local/bin/claude" --tako-warm > /dev/null 2>&1 || warm_rc=$?
    echo "claude --tako-warm: exit ${warm_rc}" > "$dir/stub/warm"
    if [[ "$warm_rc" -ne 0 ]]; then echo "  WARN: claude のスタブの初回起動が exit ${warm_rc}（#1933 の手掛かり）" >&2; fi
  fi

  # 利用者の Claude Code の設定（触られたら mtime が動く）
  echo '{}' > "$dir/home/.claude/settings.json"
  echo '{}' > "$dir/home/.claude.json"
  touch -t 202601010000 "$dir/home/.claude/settings.json" "$dir/home/.claude.json"

  git init --quiet --bare "$dir/origin.git"
  git init --quiet "$dir/repo"
  git -C "$dir/repo" symbolic-ref HEAD refs/heads/main
  git -C "$dir/repo" config user.name "tako nightly test"
  git -C "$dir/repo" config user.email "nightly@example.invalid"
  git -C "$dir/repo" config commit.gpgsign false
  git -C "$dir/repo" config tag.gpgsign false
  git -C "$dir/repo" remote add origin "$dir/origin.git"

  cp "$REPO_ROOT/scripts/nightly-release.sh" "$REPO_ROOT/scripts/check-claude-mod.sh" "$dir/repo/scripts/"
  cp "$REPO_ROOT/scripts/lib/nightly-reserve.sh" "$REPO_ROOT/scripts/lib/exit-guard.sh" "$dir/repo/scripts/lib/"
  printf '#!/bin/sh\necho "STUB release.sh $*"\nexit 0\n' > "$dir/repo/scripts/release.sh"
  chmod +x "$dir/repo/scripts/release.sh"

  # 本物の mod（Claude Code が書く型定義は除く）
  mkdir -p "$dir/repo/crates/tako-core"
  cp -R "$REPO_ROOT/crates/tako-core/claude-mod" "$dir/repo/crates/tako-core/"
  rm -rf "$dir/repo/crates/tako-core/claude-mod/.claude-plugin/types"

  printf '/target\n/dist\n' > "$dir/repo/.gitignore"
  printf '[workspace.package]\nversion = "0.7.10"\n' > "$dir/repo/Cargo.toml"
  printf '# Changelog\n\n## [0.7.10] - 2026-01-01\n\nInitial\n' > "$dir/repo/CHANGELOG.md"
  printf '# dummy lock\n' > "$dir/repo/Cargo.lock"
  git -C "$dir/repo" add -A
  git -C "$dir/repo" commit --quiet -m "init 0.7.10"
  git -C "$dir/repo" tag -a v0.7.10 -m v0.7.10
  git -C "$dir/repo" push --quiet origin HEAD:main
  git -C "$dir/repo" push --quiet origin v0.7.10

  if [[ "$no_change" != "1" ]]; then
    echo "change" > "$dir/repo/NOTES.md"
    git -C "$dir/repo" add -A
    git -C "$dir/repo" commit --quiet -m "[改善] モックの変更 (#1892)"
    git -C "$dir/repo" push --quiet origin HEAD:main
  fi
  echo "$dir"
}

# launchd と同じ起動の仕方（/bin/bash + HOME だけの env）。NIGHTLY_ENV で env を足せる
run_nightly() {
  local dir="$1"
  shift
  # shellcheck disable=SC2086 # NIGHTLY_ENV は VAR=値 を空白で並べたもの（空白を含む値は使わない）
  stage "$dir" env -i HOME="$dir/home" ${NIGHTLY_ENV:-} /bin/bash "$dir/repo/scripts/nightly-release.sh" "$@"
}

state_of() { cat "$1/home/.claude-orchestrator/state/tako-mod-check" 2>/dev/null || true; }
notices_of() { cat "$1/notify.log" 2>/dev/null || true; }
mod_notices() { notices_of "$1" | grep -c 'mod の検査' || true; }

# mod の登録を壊す / 戻す（commit + push。夜間は origin/main の mod を検査する）
break_registration() {
  printf '{ "modules": ["./missing.ts"] }\n' > "$1/repo/crates/tako-core/claude-mod/hooks/hooks.json"
  git -C "$1/repo" commit --quiet -am "壊れた登録を注入 (#1892)"
  git -C "$1/repo" push --quiet origin HEAD:main
}
restore_registration() {
  git -C "$1/repo" checkout --quiet HEAD~1 -- crates/tako-core/claude-mod/hooks/hooks.json
  git -C "$1/repo" commit --quiet -am "登録を戻す (#1892)"
  git -C "$1/repo" push --quiet origin HEAD:main
}

# --- Test 0: assert の自己検査（#1933）------------------------------------------
# 一致を 2 行目に置き、全体をパイプの容量（64 KB）より大きくした本文 = `printf | grep -q` の形なら
# grep が先に抜けて必ず取りこぼす（含む → 偽の不合格・含まない → 偽の合格）入力で、helper を確かめる
test_assert_helpers() {
  echo ""
  echo "Test 0: assert の自己検査 — 大きい出力の先頭近くにある文字列を取りこぼさない（#1933）"
  local big i fail_before pass_before
  big=$(
    printf '検査: 開始\n  validate --strict: 不合格（exit 1）\n'
    for i in $(seq 1 3000); do echo "    | 埋め草の行 ${i} ............................................"; done
  )
  assert_contains "大きい出力の 2 行目を含むと判定する（${#big} 文字）" "$big" "validate --strict: 不合格"
  # 含まない側は「落ちること」を見る（落ちた数と PASS の数を戻して、自己検査の分は数えない）
  fail_before=$FAIL
  pass_before=$PASS
  assert_not_contains "（自己検査: ここは落ちるのが正しい）" "$big" "validate --strict: 不合格" > /dev/null
  if [[ "$FAIL" -eq $((fail_before + 1)) && "$PASS" -eq "$pass_before" ]]; then
    FAIL=$fail_before
    assert_eq "含むのに『含まない』を通さない" "ok" "ok"
  else
    PASS=$pass_before
    FAIL=$fail_before
    assert_eq "含むのに『含まない』を通さない" "assert_not_contains が落ちる" "通った"
  fi
}

# --- Test 1: 正常な mod → 合格（A）--------------------------------------------
test_pass() {
  echo ""
  echo "Test 1: 正常な mod → 合格・通知なし・claude の版を記録・リリースの判定へ進む"
  local dir out rc=0 state calls
  dir=$(make_env t1)
  out=$(run_nightly "$dir" --dry-run) || rc=$?
  assert_eq "夜間リリースは exit 0" "0" "$rc"
  assert_contains "検査した claude の版をログへ出す" "$out" "mod: tako mod の検査: claude 2.1.294"
  assert_contains "origin/main の mod を検査する" "$out" "mod = origin/main@"
  assert_contains "validate --strict が合格" "$out" "mod:   validate --strict: 合格"
  assert_contains "test が合格" "$out" "mod:   test: 合格（3 pass/0 fail）"
  assert_contains "結果の行" "$out" "mod: 結果: 合格 — claude 2.1.294"
  assert_contains "リリースの判定へ進む" "$out" "DRY-RUN: ここで終了"
  assert_eq "合格では通知しない" "0" "$(mod_notices "$dir")"
  state=$(state_of "$dir")
  assert_contains "記録: result=pass" "$state" "result=pass"
  assert_contains "記録: 検査した claude の版" "$state" "claude_version=2.1.294"
  assert_contains "記録: 最後に合格した claude の版" "$state" "last_pass_claude_version=2.1.294"
  assert_contains "記録: mod の出どころ" "$state" "mod_source=origin/main@$(git -C "$dir/repo" rev-parse --short origin/main)"
  assert_contains "記録: mod の木" "$state" "mod_tree=$(git -C "$dir/repo" rev-parse origin/main:crates/tako-core/claude-mod)"
  calls=$(cat "$dir/stub/calls")
  assert_contains "validate は --strict で呼ぶ" "$calls" "plugin validate --strict"
  assert_contains "test も呼ぶ" "$calls" "plugin test "
  test_isolation "$dir"
}

# --- Test 9（Test 1 の環境で見る）: 利用者の設定に触らない ------------------------
test_isolation() {
  local dir="$1" line cwd cfg upd n=0 bad=0 cfg_left=0
  while IFS='|' read -r cwd cfg upd _; do
    n=$((n + 1))
    case "$cfg" in
      '' | "$dir/home/.claude" | "$dir/home/"*) bad=$((bad + 1)) ;;
    esac
    case "$cwd" in "$dir/repo"*) bad=$((bad + 1)) ;; esac
    [[ "$upd" = 1 ]] || bad=$((bad + 1))
    [[ -e "$cfg" ]] && cfg_left=$((cfg_left + 1))
  done < "$dir/stub/calls"
  assert_eq "claude を 3 回（--version / validate / test）呼んだ" "3" "$n"
  assert_eq "設定 dir は利用者のものではない使い捨て・作業 dir はリポジトリの外・自動更新は止める" "0" "$bad"
  assert_eq "使い捨ての設定 dir は終わったら消える" "0" "$cfg_left"
  assert_eq "利用者の settings.json の mtime が変わらない" "202601010000" "$(stat -f %Sm -t %Y%m%d%H%M "$dir/home/.claude/settings.json")"
  assert_eq "利用者の .claude.json の mtime が変わらない" "202601010000" "$(stat -f %Sm -t %Y%m%d%H%M "$dir/home/.claude.json")"
  assert_eq "利用者の設定 dir に何も増えない" "settings.json" "$(ls -A "$dir/home/.claude")"
  assert_eq "リポジトリの mod に何も書かない（git status）" "" "$(git -C "$dir/repo" status --porcelain --untracked-files=all)"
}

# --- Test 2: 壊れた登録 → 不合格・通知（B）→ 戻すと合格（A/B）-----------------
test_broken_registration() {
  echo ""
  echo "Test 2: 壊れた登録を注入 → 不合格で通知・リリースの判定へは進む → 戻すと合格（A/B）"
  local dir out rc=0 state
  dir=$(make_env t2)
  case "${TAKO_1933_INJECT:-}" in
    '' | first-exec-slow) ;;
    version | validate | test) echo fail-once > "$dir/stub/${TAKO_1933_INJECT}_mode" ;;
    *) echo "  （TAKO_1933_INJECT=${TAKO_1933_INJECT} は version / validate / test / first-exec-slow のどれか。注入せずに続ける）" ;;
  esac
  out=$(run_nightly "$dir" --dry-run) || rc=$?
  assert_contains "(A) 注入前は合格" "$out" "mod: 結果: 合格"

  break_registration "$dir"
  rc=0
  out=$(run_nightly "$dir" --dry-run) || rc=$?
  assert_eq "(B) 夜間リリースは exit 0 のまま" "0" "$rc"
  assert_contains "(B) validate が不合格" "$out" "mod:   validate --strict: 不合格（exit 1）"
  assert_contains "(B) 落ちた理由（claude の出力）をログへ出す" "$out" "modules../missing.ts"
  assert_contains "(B) test も不合格" "$out" "mod:   test: 不合格（exit 1）"
  assert_contains "(B) ERROR をログへ出す" "$out" "ERROR: mod の検査が不合格（exit 1）。夜間リリースは止めずに続ける"
  assert_contains "(B) リリースの判定へ進む（止めない）" "$out" "DRY-RUN: ここで終了"
  assert_eq "(B) 通知が 1 件出る" "1" "$(mod_notices "$dir")"
  assert_contains "(B) 通知は claude の版と落ちた段を言う" "$(notices_of "$dir")" "mod の検査: 不合格 — claude 2.1.294 で validate・test が落ちた"
  assert_contains "(B) claude は同じ = mod の変更で壊れたと言う" "$(notices_of "$dir")" "claude は前回合格と同じ 2.1.294 = mod の変更で壊れた可能性"
  assert_not_contains "(B) 一時 dir の実パスを出さない（伏せる）" "$out" "/tako-mod-check-"
  state=$(state_of "$dir")
  assert_contains "(B) 記録: result=fail" "$state" "result=fail"
  assert_contains "(B) 記録: validate=fail" "$state" "validate=fail"
  assert_contains "(B) 記録: 最後に合格した版は残す" "$state" "last_pass_claude_version=2.1.294"

  restore_registration "$dir"
  rc=0
  out=$(run_nightly "$dir" --dry-run) || rc=$?
  assert_contains "(A) 戻すと合格" "$out" "mod: 結果: 合格"
  assert_eq "(A) 合格では通知が増えない" "1" "$(mod_notices "$dir")"
}

# --- Test 3: Claude Code の更新で落ちた ------------------------------------------
test_claude_update_breaks() {
  echo ""
  echo "Test 3: Claude Code の更新で落ちた → 前回合格の版と比べて言う"
  local dir out state
  dir=$(make_env t3)
  run_nightly "$dir" --dry-run > /dev/null
  echo "2.1.300" > "$dir/stub/version"
  echo fail > "$dir/stub/validate_mode"
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "新しい版で検査したと出す" "$out" "mod: tako mod の検査: claude 2.1.300"
  assert_contains "通知は前回合格の版と「更新で壊れた」を言う" "$(notices_of "$dir")" "前回合格は claude 2.1.294"
  assert_contains "通知: Claude Code の更新で壊れた可能性" "$(notices_of "$dir")" "Claude Code の更新で壊れた可能性"
  state=$(state_of "$dir")
  assert_contains "記録: 検査した版は 2.1.300" "$state" "claude_version=2.1.300"
  assert_contains "記録: 最後に合格した版は 2.1.294 のまま" "$state" "last_pass_claude_version=2.1.294"
}

# --- Test 4: claude が無い → 未実測 -------------------------------------------
test_no_claude() {
  echo ""
  echo "Test 4: claude が無い → 未実測と出して飛ばす・通知しない・夜間リリースは止まらない"
  local dir out rc=0 state
  dir=$(make_env t4)
  run_nightly "$dir" --dry-run > /dev/null
  out=$(NIGHTLY_ENV="TAKO_CLAUDE_BIN=$dir/stub/no-such-claude" run_nightly "$dir" --dry-run) || rc=$?
  assert_eq "夜間リリースは exit 0" "0" "$rc"
  assert_contains "未実測と出す" "$out" "mod: 結果: 未実測 — claude が無いので validate / test を飛ばした"
  assert_contains "未実測の行" "$out" "mod の検査は未実測（claude が無い）。夜間リリースは続ける"
  assert_contains "リリースの判定へ進む" "$out" "DRY-RUN: ここで終了"
  assert_eq "未実測では通知しない" "0" "$(mod_notices "$dir")"
  state=$(state_of "$dir")
  assert_contains "記録: result=unmeasured" "$state" "result=unmeasured"
  assert_contains "記録: 最後に合格した版は残す" "$state" "last_pass_claude_version=2.1.294"

  # PATH に claude が無い形（TAKO_CLAUDE_BIN を使わない既定の経路）
  if [[ -e /usr/bin/claude || -e /bin/claude ]]; then
    unmeasured "/usr/bin か /bin に claude があるので PATH に無い形を作れない"
  else
    rc=0
    out=$(stage "$dir" env -i HOME="$dir/home" PATH=/usr/bin:/bin /bin/bash "$dir/repo/scripts/check-claude-mod.sh") || rc=$?
    assert_eq "単体: PATH に claude が無ければ exit 3" "3" "$rc"
    assert_contains "単体: 理由を出す" "$out" "未実測 — claude が無いので validate / test を飛ばした（PATH に claude が無い）"
  fi
}

# --- Test 5: 固まる claude → 上限で打ち切る ------------------------------------
test_timeout() {
  echo ""
  echo "Test 5: validate / --version が固まる → 上限で打ち切って通知・子プロセスを残さない"
  local dir out rc=0 start elapsed state
  dir=$(make_env t5)
  echo hang > "$dir/stub/validate_mode"
  start=$SECONDS
  out=$(NIGHTLY_ENV="TAKO_MOD_CHECK_TIMEOUT=2" run_nightly "$dir" --dry-run) || rc=$?
  elapsed=$((SECONDS - start))
  assert_eq "夜間リリースは exit 0" "0" "$rc"
  assert_contains "打ち切ったと出す" "$out" "mod:   validate --strict: 2 秒で返らないので打ち切った（test は飛ばした）"
  assert_contains "リリースの判定へ進む" "$out" "DRY-RUN: ここで終了"
  assert_contains "通知は打ち切りを言う" "$(notices_of "$dir")" "validate が 2 秒で返らない（打ち切り）"
  assert_eq "test は呼ばない（遅らせる上限は 1 段ぶん）" "0" "$(grep -c 'plugin test' "$dir/stub/calls" || true)"
  if [[ "$elapsed" -le 20 ]]; then
    assert_eq "上限で返る（${elapsed} 秒）" "ok" "ok"
  else
    assert_eq "上限で返る" "<= 20 秒" "${elapsed} 秒"
  fi
  sleep 1
  assert_eq "固まった claude とその子を残さない" "" "$(sandbox_pids "$dir/stub/hang")"
  state=$(state_of "$dir")
  assert_contains "記録: validate=timeout" "$state" "validate=timeout"
  assert_contains "記録: test=skipped" "$state" "test=skipped"

  rm -f "$dir/stub/validate_mode"
  echo hang > "$dir/stub/version_mode"
  out=$(NIGHTLY_ENV="TAKO_MOD_CHECK_TIMEOUT=2" run_nightly "$dir" --dry-run)
  assert_contains "--version が固まっても打ち切る" "$out" "mod:   claude --version: 2 秒で返らないので打ち切った（validate / test は飛ばした）"
  assert_contains "リリースの判定へ進む（--version）" "$out" "DRY-RUN: ここで終了"
  sleep 1
  assert_eq "固まった claude を残さない（--version）" "" "$(sandbox_pids "$dir/stub/hang")"
}

# --- Test 6: 実走 — 検査が落ちてもリリースは出る -----------------------------
test_release_not_blocked() {
  echo ""
  echo "Test 6: 実走 — mod の検査が落ちてもリリース（タグ + Release）は止めない"
  local dir out rc=0
  dir=$(make_env t6)
  break_registration "$dir"
  out=$(run_nightly "$dir") || rc=$?
  assert_eq "夜間リリースは exit 0" "0" "$rc"
  assert_contains "検査は不合格" "$out" "ERROR: mod の検査が不合格"
  assert_contains "リリースは完了する" "$out" "完了: v0.7.11"
  assert_eq "タグが origin に push される" "v0.7.11" "$(git -C "$dir/origin.git" tag --list v0.7.11)"
  assert_contains "mod の通知が出る" "$(notices_of "$dir")" "mod の検査: 不合格"
  assert_contains "リリース完了の通知も出る" "$(notices_of "$dir")" "テスト版リリース完了: v0.7.11"
  assert_eq "共有ツリーは main のまま" "main" "$(git -C "$dir/repo" symbolic-ref --short HEAD)"
}

# --- Test 7: 変更が無い夜も検査は回る -----------------------------------------
test_runs_on_no_change_night() {
  echo ""
  echo "Test 7: 変更が無い夜も検査は回る（スキップより前に走る）"
  local dir out mod_at skip_at
  dir=$(make_env t7 1)
  out=$(run_nightly "$dir")
  assert_contains "変更なしでスキップする" "$out" "SKIP: 変更なし"
  assert_contains "それでも検査は回る" "$out" "mod: 結果: 合格"
  mod_at=$(printf '%s\n' "$out" | grep -n 'mod: 結果:' | head -1 | cut -d: -f1)
  skip_at=$(printf '%s\n' "$out" | grep -n 'SKIP: 変更なし' | head -1 | cut -d: -f1)
  assert_eq "検査はスキップの判定より前" "yes" "$([[ -n "$mod_at" && -n "$skip_at" && "$mod_at" -lt "$skip_at" ]] && echo yes || echo no)"
}

# --- Test 8: check-claude-mod.sh 単体 -----------------------------------------
test_standalone() {
  echo ""
  echo "Test 8: check-claude-mod.sh 単体（記録しない既定・--last・ref / mod が無い・引数の誤り）"
  local dir out rc empty_commit script
  dir=$(make_env t8)
  script="$dir/repo/scripts/check-claude-mod.sh"
  sa() { stage "$dir" env -i HOME="$dir/home" PATH="$dir/home/.local/bin:/usr/bin:/bin" /bin/bash "$script" "$@"; }

  rc=0
  out=$(sa --last) || rc=$?
  assert_contains "--last: 記録が無ければそう言う" "$out" "記録なし"
  rc=0
  out=$(sa) || rc=$?
  assert_eq "作業ツリーの mod: exit 0" "0" "$rc"
  assert_contains "作業ツリーの mod を検査する" "$out" "mod = 作業ツリー（HEAD"
  assert_eq "--record が無ければ記録しない" "" "$(state_of "$dir")"
  rc=0
  out=$(sa --ref origin/main --record) || rc=$?
  assert_eq "--ref origin/main --record: exit 0" "0" "$rc"
  out=$(sa --last)
  assert_contains "--last: 記録を出す" "$out" "result=pass"

  rc=0
  out=$(sa --ref no-such-ref) || rc=$?
  assert_eq "解決できない ref: exit 1" "1" "$rc"
  assert_contains "解決できない ref: 理由" "$out" "no-such-ref が解決できない"
  empty_commit=$(git -C "$dir/repo" commit-tree "$(git -C "$dir/repo" mktree < /dev/null)" -m empty)
  rc=0
  out=$(sa --ref "$empty_commit") || rc=$?
  assert_eq "mod の無い ref: exit 1" "1" "$rc"
  assert_contains "mod の無い ref: 置き場を言う" "$out" "に crates/tako-core/claude-mod が無い"

  rc=0
  out=$(sa --dir "$dir/repo/crates/tako-core/claude-mod") || rc=$?
  assert_eq "--dir: exit 0" "0" "$rc"
  rc=0
  out=$(sa --ref origin/main --dir "$dir") || rc=$?
  assert_eq "--ref と --dir の併用: exit 2" "2" "$rc"
  rc=0
  out=$(sa --no-such-flag) || rc=$?
  assert_eq "不明な引数: exit 2" "2" "$rc"
  rc=0
  out=$(stage "$dir" env -i HOME="$dir/home" PATH="$dir/home/.local/bin:/usr/bin:/bin" TAKO_MOD_CHECK_TIMEOUT=0 /bin/bash "$script") || rc=$?
  assert_eq "上限 0 秒: exit 2" "2" "$rc"

  # claude が終了コード 0 を返しても落とす形（設計書 §5 の規約・テストが走っていない）
  echo nocatch > "$dir/stub/validate_mode"
  rc=0
  out=$(sa) || rc=$?
  assert_eq "ゲートの .catch 抜け: exit 1" "1" "$rc"
  assert_contains "ゲートの .catch 抜け: 理由" "$out" "ゲートになるフックに .catch の無いものがある"
  rm -f "$dir/stub/validate_mode"
  echo zero > "$dir/stub/test_mode"
  rc=0
  out=$(sa) || rc=$?
  assert_eq "テストが 1 本も走らない: exit 1" "1" "$rc"
  assert_contains "テストが 1 本も走らない: 理由" "$out" "テストが 1 本も走らなかった"
  rm -f "$dir/stub/test_mode"
}

# --- Test 11: 文言一致の自己検査（#1903）------------------------------------------
test_reworded() {
  echo ""
  echo "Test 11: 文言一致の自己検査 — validate / test の注記の文言が変わったら落ちる（#1903）"
  local dir out rc script
  dir=$(make_env t11)
  script="$dir/repo/scripts/check-claude-mod.sh"
  sa() { stage "$dir" env -i HOME="$dir/home" PATH="$dir/home/.local/bin:/usr/bin:/bin" /bin/bash "$script" "$@"; }

  echo reworded > "$dir/stub/validate_mode"
  rc=0
  out=$(sa) || rc=$?
  assert_eq "validate の文言が変わった: exit 1" "1" "$rc"
  assert_contains "validate の文言が変わった: 理由" "$out" "『gating hook with .catch:』の行が無い"
  assert_contains "validate の文言が変わった: 直す場所" "$out" "scripts/check-claude-mod.sh の文言一致を直す"
  assert_contains "validate の文言が変わった: 結果の行" "$out" "で validate（注記の文言が変わった） が落ちた"
  rm -f "$dir/stub/validate_mode"

  echo reworded > "$dir/stub/test_mode"
  rc=0
  out=$(sa) || rc=$?
  assert_eq "test の文言が変わった: exit 1" "1" "$rc"
  assert_contains "test の文言が変わった: 理由" "$out" "要約行『Ran N tests』が読めない"
  assert_contains "test の文言が変わった: 結果の行" "$out" "で test（要約行の文言が変わった） が落ちた"
  rm -f "$dir/stub/test_mode"

  # 1 本だけのときの単数形（`Ran 1 test`）は文言の変化ではない
  echo one > "$dir/stub/test_mode"
  rc=0
  out=$(sa) || rc=$?
  assert_eq "Ran 1 test（単数形）: exit 0" "0" "$rc"
  assert_contains "Ran 1 test（単数形）: 合格" "$out" "test: 合格（1 pass/0 fail）"
  rm -f "$dir/stub/test_mode"
}

# --- Test 10: 実物の claude（あれば）-------------------------------------------
test_real_claude() {
  echo ""
  echo "Test 10: 実物の claude — 本物の mod で合格・壊れた写しで不合格・利用者の設定の mtime が一致"
  if ! command -v claude > /dev/null 2>&1; then
    unmeasured "claude が無いので実物の validate / test を飛ばした"
    return
  fi
  local ccd before after out rc broken real_state state_before version
  # mod を読める版の下限（FR-2.42.3 の 2.1.294）より古い claude は mod も plugin test も
  # 持たないので、ランナーに古い claude があっても偽の赤にしない
  version=$(claude --version 2> /dev/null | grep -Eo '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
  if [[ -z "$version" || "$(printf '%s\n' 2.1.294 "$version" | sort -t. -k1,1n -k2,2n -k3,3n | head -1)" != 2.1.294 ]]; then
    unmeasured "claude ${version:-（版が読めない）} は mod の下限 2.1.294 未満なので実物の validate / test を飛ばした"
    return
  fi
  # 利用者の設定 dir（ログインシェルが決める。読むだけ）
  ccd="$("${SHELL:-/bin/zsh}" -l -c 'printf %s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"' 2> /dev/null)"
  [[ -n "$ccd" ]] || ccd="$HOME/.claude"
  snap() {
    local f
    for f in "$ccd/settings.json" "$ccd/settings.local.json" "$ccd/.claude.json" \
      "$HOME/.claude/settings.json" "$HOME/.claude/settings.local.json" "$HOME/.claude.json"; do
      if [[ -e "$f" ]]; then stat -f '%m' "$f"; else echo -; fi
    done | paste -sd ' ' -
  }
  real_state="$HOME/.claude-orchestrator/state/tako-mod-check"
  state_before=$(stat -f '%m' "$real_state" 2> /dev/null || echo -)
  before=$(snap)

  rc=0
  out=$(stage - /bin/bash "$REPO_ROOT/scripts/check-claude-mod.sh") || rc=$?
  echo "$out" | sed 's/^/    | /'
  assert_eq "本物の mod: exit 0" "0" "$rc"
  assert_contains "本物の mod: 合格" "$out" "結果: 合格 — claude "

  broken="$SANDBOX/real-broken"
  cp -R "$REPO_ROOT/crates/tako-core/claude-mod" "$broken"
  printf '{ "modules": ["./missing.ts"] }\n' > "$broken/hooks/hooks.json"
  rc=0
  out=$(stage - /bin/bash "$REPO_ROOT/scripts/check-claude-mod.sh" --dir "$broken") || rc=$?
  assert_eq "壊れた登録: exit 1" "1" "$rc"
  assert_contains "壊れた登録: validate が不合格" "$out" "validate --strict: 不合格"
  assert_contains "壊れた登録: 実物の claude が名指す" "$out" "missing.ts"

  after=$(snap)
  assert_eq "利用者の Claude Code の設定ファイルの mtime が前後で一致" "$before" "$after"
  assert_eq "記録（--record なし）は本番の状態ファイルを書かない" "$state_before" "$(stat -f '%m' "$real_state" 2> /dev/null || echo -)"
  assert_eq "リポジトリの mod に何も書かない（git status）" "" "$(git -C "$REPO_ROOT" status --porcelain --untracked-files=all -- crates/tako-core/claude-mod)"
}

test_assert_helpers
test_pass
test_broken_registration
test_claude_update_breaks
test_no_claude
test_timeout
test_release_not_blocked
test_runs_on_no_change_night
test_standalone
test_reworded
test_real_claude

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail / 未実測 ${UNMEASURED}"
echo "================================"
[[ $FAIL -eq 0 ]]
