#!/bin/bash
# test-bin-resolve-timeout-2151.sh — git の場所の解決（`git_bin()` の初回）が戻らないログインシェルを
#   上限なしに待たない（#2151）の実経路テスト
#
# 何が起きていたか: `git_bin()` の初回の解決（`resolve_bin`）が `git --version` とログインシェルの
#   `command -v` を**上限なしに**待っていた。rc が遅い・固まるログインシェルでは、最初の git 操作が
#   止まる（UI スレッドから呼ばれれば GUI ごと）。`OnceLock` の初期化の最中は、git を使う他の
#   スレッドもその完了を待たされた（別スレッドの側は `crates/tako-core/tests/issue2151_git_bin_oncelock.rs`）。
#
# 何を確かめるか（本物の tako CLI のプロセス。隔離 HOME・data dir。本番の GUI・設定・rc には触らない）:
#   `tako open-in repo <dir>` は IPC の前に CLI のプロセスで `git rev-parse` を起こす = そのプロセスで
#   最初の git 操作。GUI は立てない（送り先が無いので、git の後は「tako の外」で終わる）。
#   戻らないログインシェルは `$SHELL` を差し替えるスタブで作る。macOS には `/usr/bin/git` が必ずあるので、
#   ログインシェルの段へ届かせる腕は注入（`TAKO_2151_INJECT=skip-known-paths:git`）で既知の場所の段を飛ばす。
#   H1 戻らないログインシェル（PATH に git 無し）: 修正後は上限（TAKO_BIN_RESOLVE_TIMEOUT_SECS=2）で
#      戻り、persist.log に理由が 1 行。修正前（TAKO_2151_LEGACY=1）は外側の締め切りまで戻らない
#   H2 PATH の git の `--version` が戻らない（固まるラッパー）: 修正後は上限で打ち切って既知の場所
#      （/usr/bin/git）へ進み、git 操作そのものは成功する。修正前は戻らない
#   N1〜N3 通常の環境の A/B: 起こされた git を記録する偽の git（本物へそのまま渡す）で、PATH の段 /
#      ログインシェルの段 / 既知の場所の段のそれぞれで新旧が同じ git を起こす。persist.log は書かない
#   E1 上限の env が 0 / 不正: 既定（5 秒）で打ち切る（外せない）
#   E2 TAKO_GIT_BIN を渡せば、ログインシェルを一度も起こさない（新旧とも）
#
# A/B: 同じバイナリの `TAKO_2151_LEGACY=1`（修正前 = 上限なし・知らせなし）を各腕で並べて走らせる。
# 使い方: bash scripts/test-bin-resolve-timeout-2151.sh（SKIP_BUILD=1 でビルドを飛ばす）
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_GIT_BIN
unset TAKO_2151_LEGACY TAKO_2151_INJECT TAKO_BIN_RESOLVE_TIMEOUT_SECS TAKO_1503_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

# 修正後の上限（秒）と、修正前の腕を「戻らない」とみなす外側の締め切り（秒）
LIMIT=2
HANG_DEADLINE=15
# 修正後の腕が戻るまでの外側の締め切り（秒。既定の 5 秒の腕も収まる長さ）
NEW_DEADLINE=20

TMP="$(mktemp -d /tmp/tako-2151-XXXXXX)"
cleanup() {
  # 眠ったままのスタブ（修正前の腕）。**スタブが自分で書いた pid で、まだスタブの眠りのもの**だけを
  # 落とす（修正後の腕のスタブは tako が打ち切り済み = その pid は別のプロセスに再利用されうる）
  if [ -f "$TMP/stub.pids" ]; then
    while read -r p; do
      [ -n "$p" ] || continue
      case "$(ps -p "$p" -o command= 2>/dev/null)" in
        *"/bin/sleep 600"*) kill -9 "$p" 2>/dev/null ;;
      esac
    done < "$TMP/stub.pids"
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/queue.sh
. "$REPO_ROOT/scripts/lib/queue.sh"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  (cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli) || exit 1
fi
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
[ -x "$TAKO_BIN" ] || { echo "CLI が無い: $TAKO_BIN"; exit 1; }
REAL_GIT="$(command -v git || true)"
[ -n "$REAL_GIT" ] || { echo "未実測: git が無い"; exit 4; }
[ -x /usr/bin/git ] || { echo "未実測: /usr/bin/git が無い（既知の場所の段の腕が組めない）"; exit 4; }

mkdir -p "$TMP/home" "$TMP/empty" "$TMP/pathbin" "$TMP/shellbin" "$TMP/hangbin" "$TMP/repo"
REPO="$TMP/repo"
"$REAL_GIT" -C "$REPO" init -q || { echo "一時リポジトリを作れない"; exit 1; }
LOG="$TMP/git.log"
CALLS="$TMP/shell.calls"

# 起こされた git を記録して本物へ渡す偽の git（PATH の段 / ログインシェルの段の 2 か所に置く）
for where in pathbin shellbin; do
  cat > "$TMP/$where/git" <<EOF
#!/bin/sh
echo "$where \$*" >> "$LOG"
exec "$REAL_GIT" "\$@"
EOF
  chmod +x "$TMP/$where/git"
done
# `--version` だけ戻らない git（PATH の段が固まるラッパー）。眠る本人の pid を残す
cat > "$TMP/hangbin/git" <<EOF
#!/bin/sh
if [ "\$1" = "--version" ]; then
  echo \$\$ >> "$TMP/stub.pids"
  exec /bin/sleep 600
fi
echo "hangbin \$*" >> "$LOG"
exec "$REAL_GIT" "\$@"
EOF
chmod +x "$TMP/hangbin/git"
# 戻らないログインシェル（呼ばれたことと眠る本人の pid を残す）
cat > "$TMP/hang-shell" <<EOF
#!/bin/sh
echo "hang \$*" >> "$CALLS"
echo \$\$ >> "$TMP/stub.pids"
exec /bin/sleep 600
EOF
# 答えるログインシェル（`command -v git` に shellbin の git を答える）
cat > "$TMP/answer-shell" <<EOF
#!/bin/sh
echo "answer \$*" >> "$CALLS"
echo "$TMP/shellbin/git"
EOF
chmod +x "$TMP/hang-shell" "$TMP/answer-shell"

now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
count() { if [ -f "$1" ]; then wc -l < "$1" | tr -d ' '; else echo 0; fi; }

# 1 腕を走らせる: run_cli <腕の名前> <締め切り秒> <env…>
#   出力: ELAPSED（秒 or "timeout"）・腕の data dir の persist.log の #2151 の行数・記録（git / シェル）
run_cli() {
  local name="$1" deadline="$2"; shift 2
  local data="$TMP/data-$name" t0 t1 rc
  mkdir -p "$data"
  : > "$LOG"
  : > "$CALLS"
  t0=$(now)
  # 外側の締め切りは perl の alarm（SIGALRM = 142）。PATH は腕ごとに決める（既定は git の無い空の dir）
  # 外側の `{ … } 2>/dev/null` は、締め切りで殺したときにシェルが出す「Alarm clock」の 1 行を落とす
  { ( env -i HOME="$TMP/home" TMPDIR="${TMPDIR:-/tmp}" PATH="$TMP/empty" SHELL="$TMP/answer-shell" \
    TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" TAKO_DISCOVERY_DIR="$TMP/disc-$name" \
    ${1+"$@"} \
    /usr/bin/perl -e 'alarm shift; exec @ARGV' "$deadline" "$TAKO_BIN" open-in repo "$REPO" ) \
    > "$TMP/out-$name.txt" 2>&1; } 2>/dev/null
  rc=$?
  t1=$(now)
  if [ "$rc" -eq 142 ]; then
    ELAPSED=timeout
  else
    ELAPSED=$(perl -e "printf '%.2f', $t1 - $t0")
  fi
  NOTICES=$(grep -c '#2151' "$data/persist.log" 2>/dev/null || true)
  NOTICES=${NOTICES:-0}
  GITLOG=$(tr '\n' '|' < "$LOG")
  SHELLS=$(count "$CALLS")
}

# 修正後の腕が上限の近くで戻ったか（上限 + 3 秒未満）
returned_near() {
  [ "$1" != timeout ] && awk -v t="$1" -v l="$2" 'BEGIN { exit !(t + 0 < l + 3) }'
}

# 空打ち: ビルドし直した CLI の初回の起動（ページイン等）を測りに混ぜない（混雑した機では
# 初回だけ数分かかった実例がある。解決そのものは persist.log の時刻で 2 秒だった）
env -i HOME="$TMP/home" PATH="$TMP/empty" "$TAKO_BIN" --version >/dev/null 2>&1
# 作ったばかりのスタブも初回の起動だけ遅い（実測: 1 回目 1.15 秒 → 2 回目から 0.06 秒）。
# 先に走る修正後の腕だけがそれを被らないよう、1 回ずつ起こしておく（記録は腕ごとに消える）
for stub in "$TMP/pathbin/git" "$TMP/shellbin/git" "$TMP/hangbin/git" "$TMP/answer-shell"; do
  "$stub" rev-parse --git-dir >/dev/null 2>&1
done
# 戻らないシェルは 1 秒で切る（`exec` で眠る本人に alarm が残る = 孫を残さない）
{ ( /usr/bin/perl -e 'alarm 1; exec @ARGV' "$TMP/hang-shell" ) >/dev/null 2>&1; } 2>/dev/null
echo "    load: $(sysctl -n vm.loadavg 2>/dev/null)"

echo "== H1: 戻らないログインシェル（PATH に git 無し・既知の場所は注入で飛ばす）"
run_cli h1-new "$NEW_DEADLINE" SHELL="$TMP/hang-shell" TAKO_2151_INJECT=skip-known-paths:git \
  TAKO_BIN_RESOLVE_TIMEOUT_SECS=$LIMIT
echo "    修正後: ${ELAPSED} 秒・ログインシェル ${SHELLS} 回・persist.log の #2151 ${NOTICES} 行"
grep -h '#2151' "$TMP/data-h1-new/persist.log" 2>/dev/null | sed 's/^/      persist.log: /'
if returned_near "$ELAPSED" "$LIMIT" && [ "$SHELLS" = 1 ]; then
  pass "H1 修正後は上限（${LIMIT} 秒）で戻る（${ELAPSED} 秒）"
else
  fail "H1 修正後が上限で戻らない（${ELAPSED}・シェル ${SHELLS} 回）"
  tail -5 "$TMP/out-h1-new.txt"
fi
if [ "$NOTICES" = 1 ] && grep -q '素の名前の `git`' "$TMP/data-h1-new/persist.log" \
  && ! grep -q "$TMP" "$TMP/data-h1-new/persist.log"; then
  pass "H1 persist.log に落とした理由が 1 行（パスを含まない）"
else
  fail "H1 persist.log の理由の行が想定と違う（${NOTICES} 行）"
fi
run_cli h1-legacy "$HANG_DEADLINE" SHELL="$TMP/hang-shell" TAKO_2151_INJECT=skip-known-paths:git \
  TAKO_BIN_RESOLVE_TIMEOUT_SECS=$LIMIT TAKO_2151_LEGACY=1
echo "    修正前: ${ELAPSED}（締め切り ${HANG_DEADLINE} 秒）・ログインシェル ${SHELLS} 回・persist.log ${NOTICES} 行"
if [ "$ELAPSED" = timeout ] && [ "$SHELLS" = 1 ]; then
  pass "H1 修正前は ${HANG_DEADLINE} 秒の締め切りまで戻らない（同じ条件で症状が出る）"
else
  fail "H1 修正前の腕が戻った（${ELAPSED}）= 注入が症状を再現していない"
fi

echo "== H2: PATH の git の --version が戻らない（既知の場所の段は効かせる）"
run_cli h2-new "$NEW_DEADLINE" PATH="$TMP/hangbin" TAKO_BIN_RESOLVE_TIMEOUT_SECS=$LIMIT
echo "    修正後: ${ELAPSED} 秒・記録 [${GITLOG}]・persist.log ${NOTICES} 行"
grep -h '#2151' "$TMP/data-h2-new/persist.log" 2>/dev/null | sed 's/^/      persist.log: /'
if returned_near "$ELAPSED" "$LIMIT" && [ "$GITLOG" = "" ] && [ "$SHELLS" = 0 ] \
  && grep -q '既知の場所の `/' "$TMP/data-h2-new/persist.log" 2>/dev/null; then
  pass "H2 修正後は --version を上限で打ち切り、既知の場所の git へ進む（${ELAPSED} 秒）"
else
  fail "H2 修正後の結果が想定と違う（${ELAPSED}・[${GITLOG}]・シェル ${SHELLS}）"
fi
run_cli h2-legacy "$HANG_DEADLINE" PATH="$TMP/hangbin" TAKO_2151_LEGACY=1
echo "    修正前: ${ELAPSED}（締め切り ${HANG_DEADLINE} 秒）"
if [ "$ELAPSED" = timeout ]; then
  pass "H2 修正前は ${HANG_DEADLINE} 秒の締め切りまで戻らない"
else
  fail "H2 修正前の腕が戻った（${ELAPSED}）"
fi

echo "== N: 通常の環境の A/B（どの git を起こしたかが新旧で同じ・persist.log は書かない）"
ab_case() {
  local label="$1"; shift
  local new_log new_shells new_notices
  run_cli "$label-new" "$NEW_DEADLINE" ${1+"$@"}
  new_log="$GITLOG"; new_shells="$SHELLS"; new_notices="$NOTICES"
  echo "    修正後: ${ELAPSED} 秒・記録 [${new_log}]・ログインシェル ${new_shells} 回"
  run_cli "$label-legacy" "$NEW_DEADLINE" ${1+"$@"} TAKO_2151_LEGACY=1
  echo "    修正前: ${ELAPSED} 秒・記録 [${GITLOG}]・ログインシェル ${SHELLS} 回"
  AB_NEW_LOG="$new_log"
  if [ "$new_log" = "$GITLOG" ] && [ "$new_shells" = "$SHELLS" ] && [ "$new_notices" = 0 ]; then
    return 0
  fi
  return 1
}
if ab_case n1 PATH="$TMP/pathbin" && [ "$AB_NEW_LOG" = "pathbin --version|pathbin rev-parse --show-toplevel|" ]; then
  pass "N1 PATH の段: 新旧とも PATH の git を起こす"
else
  fail "N1 PATH の段で新旧が食い違う"
fi
if ab_case n2 TAKO_2151_INJECT=skip-known-paths:git \
  && [ "$AB_NEW_LOG" = "shellbin rev-parse --show-toplevel|" ]; then
  pass "N2 ログインシェルの段: 新旧ともログインシェルが答えた git を起こす"
else
  fail "N2 ログインシェルの段で新旧が食い違う"
fi
if ab_case n3 && [ "$AB_NEW_LOG" = "" ] && [ "$SHELLS" = 0 ]; then
  pass "N3 既知の場所の段: 新旧とも既知の場所の git（偽の git もログインシェルも起こさない）"
else
  fail "N3 既知の場所の段で新旧が食い違う"
fi

echo "== E: 境界"
for bad in 0 abc; do
  run_cli "e1-$bad" "$NEW_DEADLINE" SHELL="$TMP/hang-shell" TAKO_2151_INJECT=skip-known-paths:git \
    TAKO_BIN_RESOLVE_TIMEOUT_SECS=$bad
  echo "    TAKO_BIN_RESOLVE_TIMEOUT_SECS=${bad}: ${ELAPSED} 秒"
  if returned_near "$ELAPSED" 5 && awk -v t="$ELAPSED" 'BEGIN { exit !(t + 0 >= 4.5) }'; then
    pass "E1 上限 ${bad} は既定の 5 秒で打ち切る（外せない・縮まない）"
  else
    fail "E1 上限 ${bad} の扱いが想定と違う（${ELAPSED}）"
  fi
done
for arm in new legacy; do
  extra=""
  [ "$arm" = legacy ] && extra="TAKO_2151_LEGACY=1"
  run_cli "e2-$arm" "$NEW_DEADLINE" SHELL="$TMP/hang-shell" TAKO_2151_INJECT=skip-known-paths:git \
    TAKO_GIT_BIN="$TMP/pathbin/git" ${extra:+"$extra"}
  if [ "$ELAPSED" != timeout ] && [ "$SHELLS" = 0 ] && [ "$GITLOG" = "pathbin rev-parse --show-toplevel|" ]; then
    pass "E2 ${arm}: TAKO_GIT_BIN を渡せばログインシェルを起こさない（${ELAPSED} 秒）"
  else
    fail "E2 ${arm}: TAKO_GIT_BIN の扱いが想定と違う（${ELAPSED}・[${GITLOG}]・シェル ${SHELLS}）"
  fi
done

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
