#!/usr/bin/env bash
# test-nonutf8-quarantine-2231.sh — 文字として読めない recent.json / remote/shortcuts.json の保全の実経路テスト（#2231）
#
# UTF-8 でない中身は文字列の退避口（`migration::quarantine_unreadable`）を通れず、既定値へ落ちたあと
# 項目を足して保存すると**元の中身が上書きされて消えていた**。修正後は layout.json と同じく
# バイトのまま番号付きの退避（`<name>.unreadable[.<n>].bak`）へ移してから既定値で書き直す。
#
# ビルドした tako CLI（GUI 不要 = CLI の中で dispatch を回す。MCP と同じ本体）で:
#
#   ① remote/shortcuts.json を UTF-8 でない中身で壊して `tako remote shortcuts add`（読み → 追加 → 保存）
#      → 元の中身が退避に**バイト一致**で残り、足した 1 件が保存される
#   ② 中身を変えてもう一度壊して add → 1 回目の退避が上書きされない（2 本目へ）
#   ③ 同じ中身でもう一度 → 退避も記録も増えない
#   ④ 境界: 途中で切れた UTF-8・書けるが読めないファイル（0200）・読み書きできないファイル（000）・
#      BOM 付きの JSON（UTF-8 なので従来の文字列の退避。バイト一致）・正常なファイル（退避を作らない）
#
# `TAKO_2231_LEGACY=1`（修正前）と既定（修正後）を並べ、**修正前は落ち・修正後は通る**ことを見る。
# recent.json の「足して保存」は GUI の dispatch（`tako open-in dir` / `tako recent clear`）にしか無く、
# 蓋閉じ・CI では GUI を描けないので、同じ関数（`data_files::load_recent` → `push` → `save`）を叩く
# 統合テスト `issue2231_raw_quarantine_ab` を最後に回す（shortcuts も dispatch の本体で同じ A/B を見る）。
#
# **本番の tako / 設定には一切触らない**（data dir は mktemp 配下・本番 GUI の env を最初に落とす・
# GUI も tmux も立てない）。
#
# 使い方: bash scripts/test-nonutf8-quarantine-2231.sh             # CLI + 統合テスト
#         ONLY=cli bash scripts/test-nonutf8-quarantine-2231.sh    # CLI だけ（cargo test を回さない）
#         TAKO_BIN=<tako のパス> …                                  # 叩く CLI（既定は target/debug/tako）
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_2231_LEGACY TAKO_1824_LEGACY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
ONLY="${ONLY:-}"
QUEUE="/Applications/tako.app/Contents/MacOS/tako"
PASS=0
FAIL=0
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

TMP="$(mktemp -d /tmp/t2231.XXXXXX)"
cleanup() {
  # 権限を落としたファイル（退避へ移ったものも）を消せるように戻してから消す
  chmod -R u+rw "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT
case "$TMP" in
  /tmp/t2231.*) ;;
  *) echo "一時 dir の外: $TMP" >&2; exit 2 ;;
esac
mkdir -p "$TMP/target-dir" "$TMP/orig"

# 隔離した data dir で CLI を 1 回叩く（arm = fixed | legacy）。終了コードを返す
run() {
  local data="$1" arm="$2"
  shift 2
  if [ "$arm" = legacy ]; then
    env TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" TAKO_2231_LEGACY=1 "$TAKO_BIN" "$@" >/dev/null 2>&1
  else
    env TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" "$TAKO_BIN" "$@" >/dev/null 2>&1
  fi
}

fresh() {
  local d="$TMP/$1"
  mkdir -p "$d/remote"
  echo "$d"
}

# `dir` の `name` の退避（<name>.unreadable[.<n>].bak）のうち、`orig` とバイト一致するものの名前
holder() {
  local dir="$1" name="$2" orig="$3" f
  for f in "$dir/$name".unreadable*.bak; do
    [ -e "$f" ] || continue
    chmod u+r "$f" 2>/dev/null || true
    if cmp -s "$f" "$orig"; then
      basename "$f"
      return 0
    fi
  done
  return 1
}

# `dir` の `name` の退避の本数
slots() {
  local n=0 f
  for f in "$1/$2".unreadable*.bak; do
    [ -e "$f" ] && n=$((n + 1))
  done
  echo "$n"
}

# persist.log の読み手の記録（発生源 load）のうち、退避先を書いた行の数
stash_records() {
  local log="$1/persist.log" kind="$2"
  [ -f "$log" ] || { echo 0; return; }
  grep -c "$kind .*・退避 .*発生源 load）" "$log" || true
}

# 中身を元の写しとして残してから置く
plant() {
  local file="$1" orig="$2"
  shift 2
  printf "$@" > "$orig"
  command cp -f "$orig" "$file"
}

expect_held() {
  local label="$1" dir="$2" name="$3" orig="$4" got
  if got=$(holder "$dir" "$name" "$orig"); then
    pass "${label}: 元の中身が ${got} にバイト一致で残る"
  else
    fail "${label}: 元の中身がどこにも残っていない（保存で上書きされた）"
  fi
}

for arm in legacy fixed; do
  if [ "$arm" = legacy ]; then
    echo "== 修正前（TAKO_2231_LEGACY=1。保全の項目が NG になるのが期待どおり） =="
  else
    echo "== 修正後（すべて OK が期待） =="
  fi
  ARM_PASS=$PASS
  ARM_FAIL=$FAIL

  echo "-- ① ② ③ remote/shortcuts.json（tako remote shortcuts add = 読み → 追加 → 保存）"
  D=$(fresh "$arm-sc")
  R="$D/remote"
  plant "$R/shortcuts.json" "$TMP/orig/$arm-sc1" '\377\376{"version": 1, MARK_2231_SC_ONE \377}'
  run "$D" "$arm" remote shortcuts add "$TMP/target-dir"
  check_eq "UTF-8 でない中身: add の終了コード 0" 0 "$?"
  if grep -q target-dir "$R/shortcuts.json" 2>/dev/null; then
    pass "UTF-8 でない中身: 足した 1 件が保存される"
  else
    fail "UTF-8 でない中身: 足した 1 件が保存されていない"
  fi
  expect_held "UTF-8 でない中身（1 回目）" "$R" shortcuts.json "$TMP/orig/$arm-sc1"
  # 中身を変えて 2 回目
  plant "$R/shortcuts.json" "$TMP/orig/$arm-sc2" '\376\377 MARK_2231_SC_TWO \200\201'
  run "$D" "$arm" remote shortcuts add "$TMP/target-dir"
  expect_held "2 回目の後も 1 回目" "$R" shortcuts.json "$TMP/orig/$arm-sc1"
  expect_held "2 回目" "$R" shortcuts.json "$TMP/orig/$arm-sc2"
  check_eq "2 回の破損: 退避先を書いた記録 2 行" 2 "$(stash_records "$D" remote_shortcuts)"
  # 同じ中身で 3 回目 = 積まない
  before="$(slots "$R" shortcuts.json)/$(stash_records "$D" remote_shortcuts)"
  command cp -f "$TMP/orig/$arm-sc2" "$R/shortcuts.json"
  run "$D" "$arm" remote shortcuts add "$TMP/target-dir"
  check_eq "同じ中身の 3 回目: 退避の本数 / 記録は増えない" "$before" \
    "$(slots "$R" shortcuts.json)/$(stash_records "$D" remote_shortcuts)"
  if grep -q 'MARK_2231' "$D/persist.log" 2>/dev/null; then
    fail "persist.log にファイルの中身が載った"
  else
    pass "persist.log にファイルの中身が載らない"
  fi
  [ "$arm" = fixed ] && grep 'remote_shortcuts' "$D/persist.log" | sed "s#$TMP#<tmp>#g; s/^/      /"

  echo "-- ④ 境界"
  # 途中で切れた UTF-8（「あ」= e3 81 82 の 2 バイト目で切れる）
  D=$(fresh "$arm-cut")
  plant "$D/remote/shortcuts.json" "$TMP/orig/$arm-cut" '{"version": 1, "shortcuts": [{"path": "/tmp/\343\201'
  run "$D" "$arm" remote shortcuts add "$TMP/target-dir"
  expect_held "途中で切れた UTF-8" "$D/remote" shortcuts.json "$TMP/orig/$arm-cut"

  # 書けるが読めないファイル（0200）= 読めないまま保存が上書きする
  D=$(fresh "$arm-wo")
  plant "$D/remote/shortcuts.json" "$TMP/orig/$arm-wo" '{"version": 1, "shortcuts": [], "MARK_2231_WO": 1}'
  chmod 200 "$D/remote/shortcuts.json"
  if cat "$D/remote/shortcuts.json" >/dev/null 2>&1; then
    echo "  [--] 読めないファイル: root なので確かめられない（見送り）"
  else
    run "$D" "$arm" remote shortcuts add "$TMP/target-dir"
    expect_held "書けるが読めないファイル（0200）" "$D/remote" shortcuts.json "$TMP/orig/$arm-wo"
  fi

  if [ "$arm" = legacy ]; then
    LEGACY_NG=$((FAIL - ARM_FAIL))
    PASS=$ARM_PASS
    FAIL=$ARM_FAIL
    if [ "$LEGACY_NG" -gt 0 ]; then
      pass "修正前は ${LEGACY_NG} 項目が NG（足して保存すると元の中身が消える = #2231 の症状が再現）"
    else
      fail "修正前でも全部 OK になった（A/B が取れていない）"
    fi
  fi
done

echo "== 修正後の境界（A/B ではなく回帰が無いことを見る） =="
# 読み書きできないファイル（000）: 修正前は保存が失敗して足せなかった。修正後は退避へ移して足せる
D=$(fresh perm000)
plant "$D/remote/shortcuts.json" "$TMP/orig/perm000" '{"version": 1, "shortcuts": [], "MARK_2231_000": 1}'
chmod 000 "$D/remote/shortcuts.json"
if cat "$D/remote/shortcuts.json" >/dev/null 2>&1; then
  echo "  [--] 読み書きできないファイル: root なので確かめられない（見送り）"
else
  run "$D" legacy remote shortcuts add "$TMP/target-dir"
  rc_legacy=$?
  ls -l "$D/remote/shortcuts.json" | awk '{print "      修正前: 終了コード '"$rc_legacy"'・shortcuts.json の権限 " $1}'
  run "$D" fixed remote shortcuts add "$TMP/target-dir"
  check_eq "読み書きできないファイル（000）: 修正後は add の終了コード 0" 0 "$?"
  expect_held "読み書きできないファイル（000）" "$D/remote" shortcuts.json "$TMP/orig/perm000"
fi

# BOM 付きの JSON（UTF-8 として読める = 従来の文字列の退避。BOM ごとバイト一致で残る）
D=$(fresh bom)
plant "$D/remote/shortcuts.json" "$TMP/orig/bom" '\357\273\277{"version": 1, "shortcuts": []}'
run "$D" fixed remote shortcuts add "$TMP/target-dir"
check_eq "BOM 付き: add の終了コード 0" 0 "$?"
expect_held "BOM 付き" "$D/remote" shortcuts.json "$TMP/orig/bom"
if grep -q "移行できず: remote_shortcuts .*JSON の構文が壊れている（1 行 1 列）・退避 " "$D/persist.log" 2>/dev/null; then
  pass "BOM 付き: 従来の文字列の退避の記録（構文・1 行 1 列）"
else
  fail "BOM 付き: 文字列の退避の記録が無い"
fi

# 正常なファイル・不在のファイル（退避を作らない・記録しない）
D=$(fresh ok)
run "$D" fixed remote shortcuts add "$TMP/target-dir"
check_eq "不在 → add: 終了コード 0" 0 "$?"
run "$D" fixed remote shortcuts add "$TMP"
check_eq "正常 → add: 終了コード 0" 0 "$?"
check_eq "正常・不在: 退避 0 本" 0 "$(slots "$D/remote" shortcuts.json)"
if grep -q '発生源 load）' "$D/persist.log" 2>/dev/null; then
  fail "正常・不在: 読み手の記録が出た"
else
  pass "正常・不在: 読み手の記録 0 行"
fi

if [ "$ONLY" != cli ]; then
  echo
  echo "== recent の「足して保存」（GUI の OpenDir と同じ口）を統合テストで =="
  echo "   cargo test --test issue2231_raw_quarantine_ab（既定 → TAKO_2231_LEGACY=1 を 1 本で並べる）"
  # tako:prod-ok 本番の順番待ち（#1994）へ並ぶのが目的 = 本番の GUI へ届くのが仕様
  out=$(cd "$REPO_ROOT" && "$QUEUE" queue cargo test -p tako-control --test issue2231_raw_quarantine_ab -- --nocapture 2>&1)
  grep '\[2231-ab\]' <<<"$out" | sed "s#/[^ \"]*tako-2231-ab-#<tmp>/tako-2231-ab-#g; s/^/      /"
  if grep -q 'test result: ok. 1 passed' <<<"$out"; then
    pass "統合テスト: 修正後は 2 回分の中身が残り、legacy では消える"
  else
    echo "$out" | tail -30
    fail "統合テスト issue2231_raw_quarantine_ab が落ちた"
  fi
fi

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
