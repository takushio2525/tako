#!/usr/bin/env bash
# 実 tmux のテストが CI で走った件数と名前をログへ出す（Issue #2106）
#
# 使い方: bash scripts/ci-real-tmux-report.sh <台帳>
#
# 台帳 = `cargo test` を `TAKO_TEST_TMUX_LEDGER=<台帳>` で走らせたときに
# `tako_core::tmux_test_gate` が「門を通った（= 実 tmux で走った）」テストごとに
# 追記した行（`<テスト名>\t<file>:<line>`）。cargo test は通ったテストの stderr を
# 捨てるので、実 tmux で走ったかどうかはこの台帳でしか見えない。
#
# 台帳が無い・空 = 1 本も門を通っていない（tmux を入れる段が消えた・env の配線が
# 切れた）ので exit 1。skip そのものは `TAKO_TEST_REQUIRE_TMUX=1` の下で各テストが
# panic して落とすので、ここが見るのは「配線が生きているか」と件数の可視化。
#
# 番犬: crates/tako-control/tests/issue2106_real_tmux_gate_watchdog.rs（0 件で落ちる・
# 件数と名前を出す・同じテストの重複を 1 本に数える を毎回確かめる）

set -u

ledger="${1:-}"
if [ -z "${ledger}" ]; then
  echo "使い方: bash scripts/ci-real-tmux-report.sh <台帳>" >&2
  exit 2
fi

if [ ! -s "${ledger}" ]; then
  echo "実 tmux のテストが 1 本も走っていない（台帳 ${ledger} が無いか空）。" >&2
  echo "tmux を入れる段と TAKO_TEST_TMUX_LEDGER の配線を確かめる（#2106）" >&2
  exit 1
fi

# 同じテストが門を 2 回以上通ることがある（ヘルパの中から呼ぶ等）ので、
# 「門を呼んだファイル + テスト名」で 1 本に数える
tests="$(awk -F'\t' '{ loc = $2; sub(/:[0-9]+$/, "", loc); print loc "\t" $1 }' "${ledger}" | sort -u)"
passes="$(wc -l < "${ledger}" | tr -d ' ')"
count="$(printf '%s\n' "${tests}" | grep -c .)"

echo "== 実 tmux のテストが走った: ${count} 本（門を通った回数 ${passes}。#2106）"
echo "-- ファイルごと"
printf '%s\n' "${tests}" | awk -F'\t' '{ print $1 }' | uniq -c
echo "-- テスト名"
printf '%s\n' "${tests}" | awk -F'\t' '{ print "  " $2 "  (" $1 ")" }'
