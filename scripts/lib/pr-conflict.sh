#!/usr/bin/env bash
# pr-conflict.sh — 「base と衝突していて CI の run が作られない」判定と案内の 1 実装（#1365）
#
# なぜ要るか:
#   PR の base（main）が進んで衝突すると、GitHub は **merge コミットを作れないので
#   `pull_request` の run を作らない**。`gh pr checks` には外部連携（Cloudflare Pages）だけが
#   並び、macOS / Windows は永久に「登録されない」ままになる。この状態で
#   `wait-pr-checks.sh` は「期待名が全部そろって全部 completed」を待つので、
#   **待っても絶対に揃わないものをタイムアウト（2400 秒）まで待つ**（#775 の PR #1359 で 3 回）。
#   症状（run が登録されない）から原因（mergeable=CONFLICTING）へ辿るのは人間でも時間を食う。
#
#   `merge-pr.sh` は merge の門として同じ状態を見ているので、判定と案内文はここへ集めて
#   **待ち側と merge 側が同じ 1 実装を通る**ようにする（#1333 の思想: 手順の記憶ではなく構造）。
#
# 使い方（source して使う。単体では何もしない）:
#   . "${SCRIPT_DIR}/lib/pr-conflict.sh"
#   case "$(pr_conflict_verdict "${mergeable}" "${merge_state_status}")" in
#     conflicting) pr_conflict_report wait "${PR}" "${base}" "${mergeable}" "${status}"
#                  exit "${PR_CONFLICT_EXIT}" ;;
#     unknown)     : ;; # 計算中。待ちを続ける
#   esac

# 衝突で終わるときの終了コード（wait-pr-checks.sh / merge-pr.sh 共通）。
# 既存の 0 = 全部緑 / 1 = 失敗・拒否 / 2 = タイムアウト / 3 = 引数・gh のエラー と重ならない値
PR_CONFLICT_EXIT=4

# $1 = mergeable（MERGEABLE / CONFLICTING / UNKNOWN）, $2 = mergeStateStatus（CLEAN / DIRTY / …）
# → conflicting = 衝突している（run は作られない）
#   unknown     = GitHub がまだ計算していない（= 待ちを続ける。**衝突と決めつけない**）
#   ok          = 衝突していない
pr_conflict_verdict() {
  local mergeable="${1:-}" status="${2:-}"
  # DIRTY は「merge コミットを作れない」= 衝突そのもの。mergeable が追いついていなくても採る
  if [[ "${mergeable}" == "CONFLICTING" || "${status}" == "DIRTY" ]]; then
    printf 'conflicting'
    return 0
  fi
  # 空文字（フィールドが無い / gh が値を返せない）も「分からない」側へ寄せる。
  # 材料が無いことを衝突として扱うと、取り違えで待ちを打ち切ってしまう
  if [[ -z "${mergeable}" || "${mergeable}" == "UNKNOWN" ]]; then
    printf 'unknown'
    return 0
  fi
  printf 'ok'
}

# 名指しの案内を stderr へ出す（呼び出し側は続けて終了コードを返すだけ）。
# $1 = 文脈（wait = 待ちを打ち切る / merge = merge を拒否する）, $2 = PR 番号,
# $3 = base ブランチ名（空なら main）, $4 = mergeable, $5 = mergeStateStatus
pr_conflict_report() {
  local ctx="${1:-}" pr="${2:-}" base="${3:-}" mergeable="${4:-}" status="${5:-}" lead
  [[ -n "${base}" ]] || base="main"
  case "${ctx}" in
    merge) lead="merge しない: " ;;
    *) lead="待つのをやめる: " ;;
  esac
  echo "${lead}PR #${pr} は base の ${base} と衝突している（mergeable=${mergeable:-?} / mergeStateStatus=${status:-?}）" >&2
  echo "  GitHub は merge コミットを作れないので pull_request の run を作らない（待っても CI は揃わない）" >&2
  echo "  ${base} を取り込んで push し直す:" >&2
  echo "    git fetch origin && git merge origin/${base} && git push" >&2
}
