#!/usr/bin/env bash
# test-corrupt-quarantine-1824.sh — 破損したデータファイルの保全を 1 つの口へ寄せた残り 2 点の実経路テスト（#1824）
#
#   ① remote/shortcuts.json の破損が persist.log に 1 行残る（`移行できず: remote_shortcuts …・発生源 load`）。
#      ビルドした tako CLI の `tako remote shortcuts`（GUI 不要 = CLI の中で dispatch を回す。MCP と同じ本体）で、
#      途中で切れた JSON・空ファイル・UTF-8 でない中身・権限で読めないファイル・同じ中身の読み直し・
#      別の中身での 2 回目・正常なファイル（回帰なし）を見る。行にファイルの中身が載らないことも見る
#   ② layout.json を中身を変えて 2 回壊して開いても 1 回目の中身が番号付きの退避に残る。
#      layout の復元は GUI の main にしか無く、蓋閉じ・CI では GUI を描けないので、main と同じ関数
#      （`layout::try_load` → `layout::stash_after_failed_restore`）を叩く統合テスト
#      `issue1824_corrupt_quarantine_ab` を回す（起動時の移行を先に通さない呼び順。recent.json もここで見る）
#
# どちらも `TAKO_1824_LEGACY=1` で修正前の挙動へ戻し、**修正前は落ち・修正後は通る**ことを並べる。
#
# **本番の tako / 設定には一切触らない**（data dir は mktemp 配下・本番 GUI の env を最初に落とす・
# GUI も tmux も立てない）。
#
# 使い方: bash scripts/test-corrupt-quarantine-1824.sh       # ① + ②
#         ONLY=cli bash scripts/test-corrupt-quarantine-1824.sh  # ① だけ（cargo test を回さない）
#         TAKO_BIN=<tako のパス> …                               # 叩く CLI（既定は target/debug/tako）
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_1824_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
ONLY="${ONLY:-}"
QUEUE="/Applications/tako.app/Contents/MacOS/tako"
PASS=0
FAIL=0
# 修正前（legacy）の腕で NG になった項目の数。0 なら A/B が取れていない
LEGACY_NG=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

if [ ! -x "$TAKO_BIN" ]; then
  echo "tako CLI が無い: ${TAKO_BIN}（先に cargo build -p tako-cli）" >&2
  exit 2
fi

TMP="$(mktemp -d /tmp/t1824.XXXXXX)"
cleanup() {
  # 権限を落としたファイルも消せるように戻してから消す
  chmod -R u+rw "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# `tako remote shortcuts` を隔離した data dir で 1 回叩く（arm = fixed | legacy）
shortcuts() {
  local data="$1" arm="$2"
  if [ "$arm" = legacy ]; then
    env TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" TAKO_1824_LEGACY=1 "$TAKO_BIN" remote shortcuts >/dev/null 2>&1
  else
    env TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" "$TAKO_BIN" remote shortcuts >/dev/null 2>&1
  fi
}

# persist.log の破損の記録の行数（移行できず / 移行を中止）
records() {
  local log="$1/persist.log"
  [ -f "$log" ] || { echo 0; return; }
  grep -c 'remote_shortcuts .*発生源 load）' "$log" || true
}

fresh() {
  local d="$TMP/$1"
  mkdir -p "$d/remote"
  echo "$d"
}

echo "== ① remote/shortcuts.json の破損が persist.log に 1 行残る（CLI: tako remote shortcuts） =="
for arm in legacy fixed; do
  if [ "$arm" = legacy ]; then
    echo "-- legacy（修正前 = TAKO_1824_LEGACY=1。記録の項目が NG になるのが期待どおり）"
  else
    echo "-- fixed（修正後。すべて OK が期待）"
  fi
  ARM_PASS=$PASS
  ARM_FAIL=$FAIL
  # 途中で切れた JSON（値の型が違う = serde の素の文言は値を含む形）
  D=$(fresh "$arm-trunc")
  printf '{ "shortcuts": [ "MARK_SHORTCUT_1"' > "$D/remote/shortcuts.json"
  shortcuts "$D" "$arm"
  check_eq "途中で切れた JSON: 終了コード 0（壊れていても一覧は返る）" 0 "$?"
  check_eq "途中で切れた JSON: 記録 1 行" 1 "$(records "$D")"
  if grep -q 'MARK_SHORTCUT' "$D/persist.log" 2>/dev/null; then
    fail "途中で切れた JSON: persist.log にファイルの中身が載った"
  else
    pass "途中で切れた JSON: persist.log にファイルの中身が載らない"
  fi
  if [ -f "$D/remote/shortcuts.json.unreadable.bak" ]; then
    pass "途中で切れた JSON: 中身は .unreadable.bak に残る"
  else
    fail "途中で切れた JSON: .unreadable.bak が無い"
  fi
  # 同じ中身で読み直す = 増えない（CLI は呼ぶたびに別プロセス）
  shortcuts "$D" "$arm"
  check_eq "同じ中身の読み直し: 記録は 1 行のまま" 1 "$(records "$D")"
  # 別の中身で 2 回目 = 2 本目へ退避して 2 行目
  printf '{ "shortcuts": [ ' > "$D/remote/shortcuts.json"
  shortcuts "$D" "$arm"
  check_eq "別の中身で 2 回目: 記録 2 行" 2 "$(records "$D")"
  if [ -f "$D/remote/shortcuts.json.unreadable.2.bak" ]; then
    pass "別の中身で 2 回目: .unreadable.2.bak に残る"
  else
    fail "別の中身で 2 回目: .unreadable.2.bak が無い"
  fi
  [ "$arm" = fixed ] && grep 'remote_shortcuts' "$D/persist.log" | sed "s#$TMP#<tmp>#g; s/^/      /"

  # 空ファイル
  D=$(fresh "$arm-empty")
  : > "$D/remote/shortcuts.json"
  shortcuts "$D" "$arm"
  check_eq "空ファイル: 記録 1 行" 1 "$(records "$D")"

  # UTF-8 でない中身（文字として読めない = 退避はできないが、読めなかったことは残す）
  D=$(fresh "$arm-binary")
  printf '\377\376{' > "$D/remote/shortcuts.json"
  shortcuts "$D" "$arm"
  check_eq "UTF-8 でない中身: 記録 1 行（移行を中止の形）" 1 "$(records "$D")"

  # 権限で読めないファイル（root で走らせると読めてしまうので見ない）
  D=$(fresh "$arm-perm")
  printf '{"version": 1, "shortcuts": []}' > "$D/remote/shortcuts.json"
  chmod 000 "$D/remote/shortcuts.json"
  if cat "$D/remote/shortcuts.json" >/dev/null 2>&1; then
    echo "  [--] 権限で読めないファイル: root なので確かめられない（見送り）"
  else
    shortcuts "$D" "$arm"
    check_eq "権限で読めないファイル: 記録 1 行" 1 "$(records "$D")"
  fi
  chmod 600 "$D/remote/shortcuts.json"
  if [ "$arm" = legacy ]; then
    # 修正前の NG は総合判定の失敗に数えず、A/B が取れたかだけを見る
    LEGACY_NG=$((FAIL - ARM_FAIL))
    PASS=$ARM_PASS
    FAIL=$ARM_FAIL
    if [ "$LEGACY_NG" -gt 0 ]; then
      pass "修正前は ${LEGACY_NG} 項目が NG（破損が persist.log に残らない = #1824 の症状 1 が再現）"
    else
      fail "修正前でも全部 OK になった（A/B が取れていない）"
    fi
  fi
done

echo "-- 正常なファイルの読み込みに回帰が無い（修正後）"
D=$(fresh ok)
mkdir -p "$TMP/target-dir"
env TAKO_ISOLATED=1 TAKO_DATA_DIR="$D" "$TAKO_BIN" remote shortcuts add "$TMP/target-dir" >/dev/null 2>&1
check_eq "add: 終了コード 0" 0 "$?"
out=$(env TAKO_ISOLATED=1 TAKO_DATA_DIR="$D" "$TAKO_BIN" remote shortcuts 2>&1)
case "$out" in
  *target-dir*) pass "list: 足したショートカットが読める" ;;
  *) fail "list: 足したショートカットが見えない（${out}）" ;;
esac
check_eq "正常なファイル: 記録 0 行" 0 "$(records "$D")"
set -- "$D/remote/"*unreadable*
if [ -e "$1" ]; then
  fail "正常なファイル: 退避が作られた"
else
  pass "正常なファイル: 退避を作らない"
fi

if [ "$ONLY" != cli ]; then
  echo
  echo "== ② layout.json を 2 回壊して開いても 1 回目の中身が残る（+ recent.json の記録） =="
  echo "   cargo test --test issue1824_corrupt_quarantine_ab（既定 → TAKO_1824_LEGACY=1 を 1 本で並べる）"
  out=$(cd "$REPO_ROOT" && "$QUEUE" queue cargo test -p tako-control --test issue1824_corrupt_quarantine_ab -- --nocapture 2>&1)
  grep '\[1824-ab\]' <<<"$out" | sed "s#/[^ ]*tako-1824-ab-#<tmp>/tako-1824-ab-#g; s/^/      /"
  if grep -q 'test result: ok. 1 passed' <<<"$out"; then
    pass "統合テスト: 修正後は約束をすべて満たし、legacy では症状 1・2 が再現する"
  else
    echo "$out" | tail -30
    fail "統合テスト issue1824_corrupt_quarantine_ab が落ちた"
  fi
fi

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
