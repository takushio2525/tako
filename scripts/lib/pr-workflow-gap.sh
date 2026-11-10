#!/usr/bin/env bash
# pr-workflow-gap.sh — 「base にある job がこの PR の CI で走っていない（head が古い）」の
# 終了コードと案内の 1 実装（#2883）
#
# なぜ要るか:
#   `wait-pr-checks.sh` は待つチェック名を **PR の head の** workflow から導く（#2883 の前は
#   実行した木から導いていたので、CI に job が増えた後の main から、それより前に分かれた PR を待つと
#   その PR では走らない job を上限まで待ち続けた = PR #2862 が windows-clippy を足した後の PR #2878）。
#   ただし head の集合だけで緑にすると、**古い head が新しい品質のゲートを通らないまま main に入る**。
#   そこで「base にあって head に無い job」のうち、この PR の CI に登録されなかったものがあれば
#   既定では緑にせず、この終了コードで「取り込んで回し直す」か「head の集合で判定する」かを選ばせる。
#   待ち側（判定）と merge 側（終了コードの受け取り）が同じ値を見るので、ここへ集める
#   （lib/pr-conflict.sh・lib/gh-retry.sh と同じ形）。
#
# 使い方（source して使う。単体では何もしない）:
#   . "${SCRIPT_DIR}/lib/pr-workflow-gap.sh"
#   pr_workflow_gap_report "bash scripts/merge-pr.sh 2878" main 19bff97 0 "" "Windows（clippy …）"
#   exit "${PR_WORKFLOW_GAP_EXIT}"

# 走っていない job があって緑にしないときの終了コード（wait-pr-checks.sh / merge-pr.sh 共通）。
# 既存の 0 = 全部緑 / 1 = 失敗・拒否 / 2 = タイムアウト / 3 = 引数・gh のエラー /
# 4 = base と衝突している（lib/pr-conflict.sh）/ 5 = GitHub API に届かない（lib/gh-retry.sh）と重ならない値
PR_WORKFLOW_GAP_EXIT=6

# 名指しの案内を stderr へ出す（呼び出し側は続けて終了コードを返すだけ）。
# $1 = 打ち直すコマンド（例: bash scripts/merge-pr.sh 2878）, $2 = base ブランチ名（空なら main）,
# $3 = head の短い SHA, $4 = skip された本数, $5 = head にだけある job（", " 区切り。空可）,
# $6 以降 = base にあって、この PR の CI で走っていない job
pr_workflow_gap_report() {
  local cmd="${1:-}" base="${2:-}" head="${3:-}" skipped="${4:-0}" head_only="${5:-}" names="" n
  [[ -n "${base}" ]] || base="main"
  shift 5 || true
  for n in "$@"; do
    [[ -z "${names}" ]] && names="${n}" || names="${names}, ${n}"
  done
  echo "緑にしない: base の ${base} にある job がこの PR の CI で走っていない（head ${head} が古い。#2883）: ${names}" >&2
  echo "  この job を足す前の ${base} から分かれた PR なので、緑のまま merge するとこのゲートを通らない変更が ${base} に入る" >&2
  echo "  ${base} を取り込んで CI を回し直す（既定はこちら）:" >&2
  echo "    git fetch origin && git merge origin/${base} && git push" >&2
  echo "  head の集合で判定してよいと分かっているなら（ゲートに関係しない変更・job の改名）:" >&2
  echo "    TAKO_WAIT_PR_CHECKS_SET=head ${cmd}" >&2
  if [[ -n "${head_only}" ]]; then
    echo "  （head にだけある job: ${head_only}。名前を変えた job なら head の集合で構わない）" >&2
  fi
  if [[ "${skipped}" =~ ^[0-9]+$ && ${skipped} -gt 0 ]]; then
    echo "  （この PR は ${skipped} 本が skip = docs だけの変更（#2084）。走っていない job も同じ条件で skip されるだけなら head の集合で構わない）" >&2
  fi
  return 0
}
