#!/usr/bin/env bash
# test-nonutf8-quarantine-2238.sh — 文字として読めない settings.json / claude-mod/ui.json / run-configs.json の保全の実経路テスト（#2238）
#
# UTF-8 でない settings.json / run-configs.json は文字列の退避口を通れず、退避されずに既定値へ落ちたあと
# 保存すると**元の中身が上書きされて消えていた**。claude-mod/ui.json は UTF-8 でない中身を 1 本目が
# 空いているときだけ写していたので、**別の中身で 2 回目に壊れると変更の操作が消していた**（persist.log
# には 1 本目を退避先と偽った行が残った）。修正後は 3 つとも recent / shortcuts（#2231）と同じく、
# バイトのまま番号付きの退避（`<name>.unreadable[.<n>].bak`）へ移してから既定値で書き直す。
#
# ビルドした tako CLI（GUI 不要のローカル処理）で:
#
#   ① settings.json を UTF-8 でない中身で壊して `tako telemetry off`（読み → 書き換え → 保存）
#      / ui.json を壊して `tako mod ui set colors.dim text`（読み → 変える → 保存）
#      → 元の中身が退避に**バイト一致**で残り、書き換えが保存される
#   ② 中身を変えてもう一度 → 1 回目の退避が上書きされない（2 本目へ）。2 行目の記録は 2 本目を名指す
#   ③ 同じ中身でもう一度 → 退避も記録も増えない
#   ④ 境界: 途中で切れた UTF-8・書けるが読めない（0200）・読み書きできない（000）・BOM 付きの JSON
#      （UTF-8 として読める = 従来の文字列の退避。BOM ごとバイト一致）・正常 / 不在（退避を作らない）
#   ⑤ 起動時の移行（`tako migrate run`）を先に通しても退避は 1 本（二重に移さない）
#
# `TAKO_2238_LEGACY=1`（修正前）と既定（修正後）を並べ、**修正前は落ち・修正後は通る**ことを見る。
# run-configs.json の保存（`tako run-config`）は GUI の dispatch を通る（蓋閉じ・CI では GUI を描けない）ので、
# CLI / MCP と同じ本体（`run_configs::handle`）を叩く統合テスト `issue2238_nonutf8_quarantine_ab` を
# 最後に回す（settings / ui.json も同じ本体で同じ A/B を見る）。
#
# **本番の tako / 設定には一切触らない**（data dir と HOME は mktemp 配下・本番 GUI の env を最初に落とす・
# GUI も tmux も立てない）。
#
# 使い方: bash scripts/test-nonutf8-quarantine-2238.sh             # CLI + 統合テスト
#         ONLY=cli bash scripts/test-nonutf8-quarantine-2238.sh    # CLI だけ（cargo test を回さない）
#         TAKO_BIN=<tako のパス> …                                  # 叩く CLI（既定は target/debug/tako）
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE \
  TAKO_2238_LEGACY TAKO_2231_LEGACY TAKO_1824_LEGACY

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

TMP="$(mktemp -d /tmp/t2238.XXXXXX)"
cleanup() {
  # 権限を落としたファイル（退避へ移ったものも）を消せるように戻してから消す
  chmod -R u+rw "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT
case "$TMP" in
  /tmp/t2238.*) ;;
  *) echo "一時 dir の外: $TMP" >&2; exit 2 ;;
esac
mkdir -p "$TMP/home" "$TMP/orig"

# 隔離した data dir で CLI を 1 回叩く（arm = fixed | legacy）。終了コードを返す
run() {
  local data="$1" arm="$2"
  shift 2
  if [ "$arm" = legacy ]; then
    env HOME="$TMP/home" TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" TAKO_2238_LEGACY=1 "$TAKO_BIN" "$@" >/dev/null 2>&1
  else
    env HOME="$TMP/home" TAKO_ISOLATED=1 TAKO_DATA_DIR="$data" "$TAKO_BIN" "$@" >/dev/null 2>&1
  fi
}

fresh() {
  local d="$TMP/$1"
  mkdir -p "$d/claude-mod"
  echo "$d"
}

# 種類ごとの「読み → 保存」（kind = settings | ui）
act() {
  local kind="$1" data="$2" arm="$3"
  if [ "$kind" = settings ]; then
    run "$data" "$arm" telemetry off
  else
    run "$data" "$arm" mod ui set colors.dim text
  fi
}

# 種類ごとの置き場（data dir からの相対）と persist.log の行の頭
rel_of() { if [ "$1" = settings ]; then echo settings.json; else echo claude-mod/ui.json; fi; }
head_of() {
  if [ "$1" = settings ]; then
    echo "settings.json を読めないので既定値で動く"
  else
    echo "claude-mod/ui.json の読めない部分を既定で動かす"
  fi
}
# 「文字として読めない」と記録した印（ui.json は解釈の誤りと行の頭が同じなので理由で見分ける）
utf8_mark_of() {
  if [ "$1" = settings ]; then
    echo "settings.json を読めないので既定値で動く"
  else
    echo "UTF-8 として読めない"
  fi
}

# `file` の退避（<file>.unreadable[.<n>].bak）のうち、`orig` とバイト一致するものの名前
holder() {
  local file="$1" orig="$2" f
  for f in "$file".unreadable*.bak; do
    [ -e "$f" ] || continue
    chmod u+r "$f" 2>/dev/null || true
    if cmp -s "$f" "$orig"; then
      basename "$f"
      return 0
    fi
  done
  return 1
}

# `file` の退避の本数
slots() {
  local n=0 f
  for f in "$1".unreadable*.bak; do
    [ -e "$f" ] && n=$((n + 1))
  done
  echo "$n"
}

# persist.log のうち、`head` で始まる退避先を書いた行
stash_lines() {
  local log="$1/persist.log" head="$2"
  [ -f "$log" ] || return 0
  grep "${head}: .*・退避 " "$log" || true
}
stash_records() { stash_lines "$1" "$2" | grep -c . || true; }

# 中身を元の写しとして残してから置く
plant() {
  local file="$1" orig="$2"
  shift 2
  printf "$@" > "$orig"
  command cp -f "$orig" "$file"
}

expect_held() {
  local label="$1" file="$2" orig="$3" got
  if got=$(holder "$file" "$orig"); then
    pass "${label}: 元の中身が ${got} にバイト一致で残る"
  else
    fail "${label}: 元の中身がどこにも残っていない（保存で上書きされた）"
  fi
}

for arm in legacy fixed; do
  if [ "$arm" = legacy ]; then
    echo "== 修正前（TAKO_2238_LEGACY=1。保全の項目が NG になるのが期待どおり） =="
  else
    echo "== 修正後（すべて OK が期待） =="
  fi
  ARM_PASS=$PASS
  ARM_FAIL=$FAIL

  for kind in settings ui; do
    rel=$(rel_of "$kind")
    head=$(head_of "$kind")
    echo "-- ① ② ③ ${rel}（読み → 保存）"
    D=$(fresh "$arm-$kind")
    F="$D/$rel"
    plant "$F" "$TMP/orig/$arm-$kind-1" '\377\376{"theme": "light", "MARK_2238_ONE": 1}\377'
    act "$kind" "$D" "$arm"
    check_eq "${rel} UTF-8 でない中身: 終了コード 0" 0 "$?"
    if [ "$kind" = settings ]; then
      if grep -q '"telemetry": false' "$F" 2>/dev/null; then
        pass "${rel}: 書き換え（telemetry off）が保存される"
      else
        fail "${rel}: 書き換えが保存されていない"
      fi
    else
      if grep -q '"dim": "text"' "$F" 2>/dev/null; then
        pass "${rel}: 変更（colors.dim = text）が保存される"
      else
        fail "${rel}: 変更が保存されていない"
      fi
    fi
    expect_held "${rel}（1 回目）" "$F" "$TMP/orig/$arm-$kind-1"
    # 中身を変えて 2 回目
    plant "$F" "$TMP/orig/$arm-$kind-2" '\376\377 MARK_2238_TWO \200\201'
    act "$kind" "$D" "$arm"
    expect_held "${rel} 2 回目の後も 1 回目" "$F" "$TMP/orig/$arm-$kind-1"
    expect_held "${rel} 2 回目" "$F" "$TMP/orig/$arm-$kind-2"
    check_eq "${rel} 2 回の破損: 退避先を書いた記録 2 行" 2 "$(stash_records "$D" "$head")"
    last=$(stash_lines "$D" "$head" | tail -1)
    second=$(holder "$F" "$TMP/orig/$arm-$kind-2") || second="（無い）"
    case "$last" in
      *"/${second}）") pass "${rel}: 2 行目の記録は 2 回目の中身の置き場（${second}）を名指す" ;;
      *) fail "${rel}: 2 行目の記録が 2 回目の中身の置き場を名指していない" ;;
    esac
    # 同じ中身で 3 回目 = 積まない
    before="$(slots "$F")/$(stash_records "$D" "$head")"
    command cp -f "$TMP/orig/$arm-$kind-2" "$F"
    act "$kind" "$D" "$arm"
    check_eq "${rel} 同じ中身の 3 回目: 退避の本数 / 記録は増えない" "$before" \
      "$(slots "$F")/$(stash_records "$D" "$head")"
    if grep -q 'MARK_2238' "$D/persist.log" 2>/dev/null; then
      fail "${rel}: persist.log にファイルの中身が載った"
    else
      pass "${rel}: persist.log にファイルの中身が載らない"
    fi
    [ "$arm" = fixed ] && stash_lines "$D" "$head" | sed "s#$TMP#<tmp>#g; s/^\[[^]]*\] //; s/^/      /"

    echo "-- ④ ${rel} の境界"
    # 途中で切れた UTF-8（「あ」= e3 81 82 の 2 バイト目で切れる）
    D=$(fresh "$arm-$kind-cut")
    F="$D/$rel"
    plant "$F" "$TMP/orig/$arm-$kind-cut" '{"theme": "\343\201'
    act "$kind" "$D" "$arm"
    expect_held "${rel} 途中で切れた UTF-8" "$F" "$TMP/orig/$arm-$kind-cut"

    # 書けるが読めないファイル（0200）= 読めないまま保存が上書きする
    D=$(fresh "$arm-$kind-wo")
    F="$D/$rel"
    plant "$F" "$TMP/orig/$arm-$kind-wo" '{"MARK_2238_WO": 1}'
    chmod 200 "$F"
    if cat "$F" >/dev/null 2>&1; then
      echo "  [--] ${rel} 読めないファイル: root なので確かめられない（見送り）"
    else
      act "$kind" "$D" "$arm"
      expect_held "${rel} 書けるが読めないファイル（0200）" "$F" "$TMP/orig/$arm-$kind-wo"
    fi
  done

  if [ "$arm" = legacy ]; then
    LEGACY_NG=$((FAIL - ARM_FAIL))
    PASS=$ARM_PASS
    FAIL=$ARM_FAIL
    if [ "$LEGACY_NG" -gt 0 ]; then
      pass "修正前は ${LEGACY_NG} 項目が NG（保存すると元の中身が消える = #2238 の症状が再現）"
    else
      fail "修正前でも全部 OK になった（A/B が取れていない）"
    fi
  fi
done

echo "== 修正後の境界（A/B ではなく回帰が無いことを見る） =="
for kind in settings ui; do
  rel=$(rel_of "$kind")
  head=$(head_of "$kind")
  # 読み書きできないファイル（000）
  D=$(fresh "perm000-$kind")
  F="$D/$rel"
  plant "$F" "$TMP/orig/perm000-$kind" '{"MARK_2238_000": 1}'
  chmod 000 "$F"
  if cat "$F" >/dev/null 2>&1; then
    echo "  [--] ${rel} 読み書きできないファイル: root なので確かめられない（見送り）"
  else
    act "$kind" "$D" fixed
    check_eq "${rel} 読み書きできないファイル（000）: 終了コード 0" 0 "$?"
    expect_held "${rel} 読み書きできないファイル（000）" "$F" "$TMP/orig/perm000-$kind"
  fi

  # BOM 付きの JSON（UTF-8 として読める = 従来の文字列の退避。BOM ごとバイト一致で残る）
  D=$(fresh "bom-$kind")
  F="$D/$rel"
  plant "$F" "$TMP/orig/bom-$kind" '\357\273\277{"schema_version": 1}'
  act "$kind" "$D" fixed
  check_eq "${rel} BOM 付き: 終了コード 0" 0 "$?"
  expect_held "${rel} BOM 付き" "$F" "$TMP/orig/bom-$kind"
  if grep -q "$(utf8_mark_of "$kind")" "$D/persist.log" 2>/dev/null; then
    fail "${rel} BOM 付き: 文字として読める中身を「読めない」と記録した"
  else
    pass "${rel} BOM 付き: 文字として読めないの記録は出ない（従来の解釈できない側）"
  fi

  # 正常なファイル・不在のファイル（退避を作らない・記録しない）
  D=$(fresh "ok-$kind")
  F="$D/$rel"
  act "$kind" "$D" fixed
  check_eq "${rel} 不在 → 保存: 終了コード 0" 0 "$?"
  act "$kind" "$D" fixed
  check_eq "${rel} 正常 → 保存: 終了コード 0" 0 "$?"
  check_eq "${rel} 正常・不在: 退避 0 本" 0 "$(slots "$F")"
  if [ -s "$D/persist.log" ] && grep -q "$head" "$D/persist.log"; then
    fail "${rel} 正常・不在: 読み手の記録が出た"
  else
    pass "${rel} 正常・不在: 読み手の記録 0 行"
  fi
done

echo "== ⑤ 起動時の移行（tako migrate run）を先に通しても退避は 1 本 =="
D=$(fresh migrate-first)
for kind in settings ui; do
  plant "$D/$(rel_of "$kind")" "$TMP/orig/mf-$kind" '\377\376 MARK_2238_MF \377'
done
run "$D" fixed migrate run --schema settings
run "$D" fixed migrate run --schema claude_mod_ui
for kind in settings ui; do
  rel=$(rel_of "$kind")
  act "$kind" "$D" fixed
  check_eq "${rel} 移行 → 保存: 退避 1 本" 1 "$(slots "$D/$rel")"
  expect_held "${rel} 移行 → 保存" "$D/$rel" "$TMP/orig/mf-$kind"
done
sed "s#$TMP#<tmp>#g; s/^\[[^]]*\] //; s/^/      /" "$D/persist.log" 2>/dev/null

if [ "$ONLY" != cli ]; then
  echo
  echo "== run-configs.json の保存（tako run-config / MCP の本体）を統合テストで =="
  echo "   cargo test --test issue2238_nonutf8_quarantine_ab（既定 → TAKO_2238_LEGACY=1 を 1 本で並べる）"
  # tako:prod-ok 本番の順番待ち（#1994）へ並ぶのが目的 = 本番の GUI へ届くのが仕様
  out=$(cd "$REPO_ROOT" && "$QUEUE" queue cargo test -p tako-control --test issue2238_nonutf8_quarantine_ab -- --nocapture 2>&1)
  grep '\[2238-ab\]' <<<"$out" | sed "s#/[^ \"]*tako-2238-ab-#<tmp>/tako-2238-ab-#g; s/^/      /"
  if grep -q 'test result: ok. 1 passed' <<<"$out"; then
    pass "統合テスト: 修正後は 3 つとも 2 回分の中身が残り、legacy では消える"
  else
    echo "$out" | tail -30
    fail "統合テスト issue2238_nonutf8_quarantine_ab が落ちた"
  fi
fi

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
