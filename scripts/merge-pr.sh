#!/usr/bin/env bash
# tako:run: bash scripts/merge-pr.sh --help
# merge-pr.sh — CI が全部緑で揃ってから PR を squash merge する（#1333）
#
# 「CI 3 本緑を待って merge」を手順の記憶ではなく**構造**で守るための包み。
# 判定は scripts/wait-pr-checks.sh の 1 実装（期待するチェックが全部そろって全部完了 +
# 同じ結論を 2 回連続で観測）に任せ、ここは merge してよい状態かだけを見る。
#
# PR の CI は **merge 結果**（`refs/pull/<PR>/merge`）を検査するが、その **merge base は
# run が始まった時点で凍る**。別々に緑だった 2 本が組み合わさって壊れる事故（#1343 =
# #1295 のテストに #1297 が足したフィールドが無い）はここを通り抜けるので、
# merge の直前に「緑を出した run の後に main が進んでいないか」を見て警告する。
#
# merge の後始末（リモート / ローカルの head ブランチの削除）も自分で閉じる。
# `gh pr merge --delete-branch` は「ローカルへ切り替え → ローカル削除 → リモート削除」の順で
# 処理するので、**専用 worktree から実行すると**最初の切り替えが
# `fatal: '<既定ブランチ>' is already used by worktree` で落ち、**ローカルもリモートも消え残る**
# （#1347 = PR #1337 / #1430 の実測。この機序はこのリポの標準手順が毎回踏む）。
#
# **gh の終了コードは merge の成否ではない**（#1430）。上の後始末はすべて merge の**後**に
# 行われるので、gh が 1 で終わっても PR は MERGED になっている。成否は `gh pr view` の
# 実測で決め、最後の 1 行で終了コードの意味を言い切る（「gh が 1 で終わった」という
# 警告だけを見た worker 3 本が merge 失敗と誤読したのが #1430 の発端）。
#
# 使い方: bash scripts/merge-pr.sh <PR番号> [--timeout <秒>] [--interval <秒>]
# 終了コード: 0 = merge が成立している（**すでに MERGED だった再実行も 0** = #1430）/
#             1 = merge しなかった（CI 失敗・BEHIND・BLOCKED・draft・merge されずに CLOSED 等）/
#             2 = CI が揃わずタイムアウト / 3 = 引数・gh のエラー /
#             4 = base と衝突している（#1365。待ち側と同じ値 = 判定も案内も 1 実装）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 衝突の判定・案内・終了コード（PR_CONFLICT_EXIT）は wait-pr-checks.sh と共有する
# shellcheck source=lib/pr-conflict.sh
. "${SCRIPT_DIR}/lib/pr-conflict.sh"

# A/B（検出力の確認・計測専用）。1 = **修正前**のまま gh の --delete-branch に後始末を任せる
# 腕。worktree から実行するとリモート head ブランチが消え残る（#1347）。
# scripts/test-wait-pr-checks.sh の Test 16 がこの腕で残ることを固定している
LEGACY_1347="${TAKO_1347_LEGACY:-0}"

# A/B（検出力の確認・計測専用）。1 = **修正前**（#1430）の腕。2 つを同時に戻す:
#   - merge 後のローカル head ブランチを後始末しない（gh の --delete-branch 任せ = 必ず残る）
#   - すでに MERGED の PR への再実行を「OPEN ではない」として 1 で拒む
# scripts/test-wait-pr-checks.sh の Test 24 / 26 / 29 がこの腕との差を固定している
LEGACY_1430="${TAKO_1430_LEGACY:-0}"

die() {
  echo "エラー: $*" >&2
  exit 3
}

refuse() {
  echo "merge しない: $*" >&2
  exit 1
}

PR=""
FORWARD=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --timeout | --interval)
      [[ $# -ge 2 ]] || die "$1 には秒数が要る"
      FORWARD+=("$1" "$2")
      shift 2
      ;;
    -h | --help)
      cat <<'USAGE'
使い方: bash scripts/merge-pr.sh <PR番号> [--timeout <秒>] [--interval <秒>]

  CI が「期待するチェックが全部そろって全部緑」になるまで待ってから squash merge する
  （待ちの判定は scripts/wait-pr-checks.sh）。揃わなければ merge しない。

終了コード: 0 = merge が成立している（すでに MERGED だった再実行も 0）/
            1 = merge しなかった / 2 = タイムアウト / 3 = 引数・gh のエラー /
            4 = base と衝突している（取り込んで push し直す）
USAGE
      exit 0
      ;;
    -*) die "不明なオプション: $1（使い方は --help）" ;;
    *)
      [[ -z "${PR}" ]] || die "PR 番号を 2 つ受け取った: ${PR} と $1"
      PR="$1"
      shift
      ;;
  esac
done
[[ -n "${PR}" ]] || die "PR 番号が要る（例: bash scripts/merge-pr.sh 1334）"
[[ "${PR}" =~ ^[0-9]+$ ]] || die "PR 番号が数値ではない: ${PR}"
command -v gh >/dev/null 2>&1 || die "gh が見つからない（https://cli.github.com/）"
command -v jq >/dev/null 2>&1 || die "jq が見つからない（brew install jq）"

PR_JSON=""
fetch_pr() {
  local err rc
  err="$(mktemp)"
  set +e
  PR_JSON="$(gh pr view "${PR}" --json number,state,isDraft,mergeable,mergeStateStatus,headRefName,headRefOid,baseRefName,isCrossRepository,title,url 2>"${err}")"
  rc=$?
  set -e
  if [[ ${rc} -ne 0 || -z "${PR_JSON}" ]]; then
    echo "エラー: gh pr view が失敗した（PR #${PR} / 終了コード ${rc}）" >&2
    cat "${err}" >&2
    rm -f "${err}"
    exit 3
  fi
  rm -f "${err}"
}

pr_field() { jq -r --arg k "$1" '.[$k] // "" | tostring' <<<"${PR_JSON}"; }

# merge して構わない状態かを見る。駄目なら理由を出して終わる（衝突は 4 / それ以外は 1）
gate_state() {
  local phase="$1" state draft mergeable status verdict tries
  state="$(pr_field state)"
  draft="$(pr_field isDraft)"
  # すでに merge 済み = **望んだ終わり方に到達している**ので、後始末だけ確かめて 0 で終わる（#1430）。
  # 自動化は終了コードで成否を見るので、再実行・重複実行を「失敗」に化けさせない
  # （待っている間に別経路で merge されたときも同じ扱いになる）。
  # merge されずに閉じられた CLOSED は望んだ状態ではないので、下の門で従来どおり 1
  if [[ "${state}" == "MERGED" && "${LEGACY_1430}" != "1" ]]; then
    echo "PR #${PR} はすでに merge 済み（${phase}）。後始末だけ確かめる"
    finish_merged
  fi
  [[ "${state}" == "OPEN" ]] || refuse "PR #${PR} は OPEN ではない（${state}）"
  [[ "${draft}" != "true" ]] || refuse "PR #${PR} は draft のまま"

  # mergeable は GitHub が遅延計算するので、確定しないあいだは数回待つ
  tries=0
  while :; do
    mergeable="$(pr_field mergeable)"
    status="$(pr_field mergeStateStatus)"
    verdict="$(pr_conflict_verdict "${mergeable}" "${status}")"
    [[ "${verdict}" == "unknown" ]] || break
    tries=$((tries + 1))
    if [[ ${tries} -ge 5 ]]; then
      echo "警告: mergeable が確定しない（値 '${mergeable}' / ${phase}）。gh pr merge の判断に任せる" >&2
      break
    fi
    sleep 3
    fetch_pr
  done

  # base と衝突している = merge できないだけでなく **CI の run も作られない**（#1365）。
  # 判定も案内文も待ち側と同じ lib/pr-conflict.sh を通すので、待ちで落ちたときと同じ言い方になる
  if [[ "${verdict}" == "conflicting" ]]; then
    pr_conflict_report merge "${PR}" "$(pr_field baseRefName)" "${mergeable}" "${status}"
    exit "${PR_CONFLICT_EXIT}"
  fi
  case "${status}" in
    BEHIND)
      refuse "PR #${PR} のブランチが main より古い（mergeStateStatus=BEHIND）。
  ローカルで main を取り込んでから push し直す:
    git fetch origin && git merge origin/main && git push"
      ;;
    BLOCKED)
      refuse "PR #${PR} は保護ルールで止まっている（mergeStateStatus=BLOCKED）"
      ;;
  esac
}

# merge 成立後にリモートの head ブランチが残っていたら自分で消す（#1347）。
# gh は --delete-branch を「ローカルへ切り替え → ローカル削除 → リモート削除」の順で行うので、
# 専用 worktree（共有ツリーが既定ブランチを握っている）だと最初の切り替えで落ちて
# **リモートまで到達しない**。冪等（既に無ければ何もしない）で、消すのは**この PR の head だけ**。
delete_remote_head_branch() {
  local head base cross
  # A/B: 修正前は gh の後始末に任せきりだった
  [[ "${LEGACY_1347}" != "1" ]] || return 0
  head="$(pr_field headRefName)"
  base="$(pr_field baseRefName)"
  cross="$(pr_field isCrossRepository)"
  [[ -n "${head}" ]] || return 0
  # 取り込み先（= この PR の base）には絶対に触らない。main を消す経路を作らないための門
  if [[ "${head}" == "${base}" ]]; then
    echo "警告: head と base が同じ（${head}）ので後始末をしない" >&2
    return 0
  fi
  if [[ "${cross}" == "true" ]]; then
    echo "fork からの PR なので head ブランチには触らない（${head}）"
    return 0
  fi
  if ! gh api "repos/{owner}/{repo}/git/ref/heads/${head}" >/dev/null 2>&1; then
    echo "リモートブランチ ${head} は削除済み"
    return 0
  fi
  if gh api -X DELETE "repos/{owner}/{repo}/git/refs/heads/${head}" >/dev/null 2>&1; then
    echo "リモートブランチ ${head} を削除した"
  else
    echo "警告: リモートブランチ ${head} を削除できなかった（手で: git push origin --delete ${head}）" >&2
  fi
}

# この作業ツリー構成で <ブランチ> を握っている作業ツリーのパスを返す（誰も握っていなければ空）。
# `git worktree list --porcelain` は「worktree <パス>」と「branch refs/heads/<名前>」が対で並ぶ
worktree_holding_branch() {
  local want="refs/heads/$1" line path=""
  while IFS= read -r line; do
    case "${line}" in
      "worktree "*) path="${line#worktree }" ;;
      "branch "*)
        if [[ "${line#branch }" == "${want}" ]]; then
          printf '%s' "${path}"
          return 0
        fi
        ;;
    esac
  done < <(git worktree list --porcelain 2>/dev/null || true)
  return 0
}

# merge 成立後にローカルの head ブランチも自分で消す（#1430）。
# gh の --delete-branch は「base へ切り替え → ローカル削除 → リモート削除」の順なので、
# 専用 worktree（共有ツリーが base を握っている）では**最初の切り替えで落ちて**
# ローカルもリモートも残る。#1347 がリモートを閉じ、ここがローカルを閉じる。
# squash merge では head は base の祖先にならないので、gh と同じく -D（強制）で消す。
# 消せない**唯一の正当な理由**は「その作業ツリーがそのブランチを握っている」ことなので、
# そのときは**どこが握っているか**と外し方を名指しして残す（受け入れ条件の例外）。
delete_local_head_branch() {
  local head base cross holder
  # A/B: 修正前はローカルの後始末をしない（gh 任せ = worktree では必ず残る）
  [[ "${LEGACY_1430}" != "1" ]] || return 0
  head="$(pr_field headRefName)"
  base="$(pr_field baseRefName)"
  cross="$(pr_field isCrossRepository)"
  [[ -n "${head}" ]] || return 0
  git rev-parse --git-dir >/dev/null 2>&1 || return 0
  # 取り込み先（= この PR の base）には絶対に触らない。リモート側と同じ門
  if [[ "${head}" == "${base}" ]]; then
    echo "警告: head と base が同じ（${head}）のでローカルブランチにも触らない" >&2
    return 0
  fi
  # fork の PR の head 名は**手元の無関係な同名ブランチ**と衝突しうるので触らない
  if [[ "${cross}" == "true" ]]; then
    echo "fork からの PR なのでローカルブランチにも触らない（${head}）"
    return 0
  fi
  if ! git show-ref --verify --quiet "refs/heads/${head}"; then
    echo "ローカルブランチ ${head} は無い"
    return 0
  fi
  holder="$(worktree_holding_branch "${head}")"
  if [[ -n "${holder}" ]]; then
    echo "ローカルブランチ ${head} は残す（作業ツリー ${holder} が握っているので消せない）"
    # 連結された作業ツリーは `.git` が**ファイル**（本体は**ディレクトリ**）。
    # 本体は畳めないので、同じ案内を出すと実行できないコマンドを渡すことになる
    if [[ -f "${holder}/.git" ]]; then
      echo "  この作業ツリーを畳むときに一緒に消える: git worktree remove ${holder} && git branch -D ${head}"
    else
      echo "  別のブランチへ移れば消せる: git -C ${holder} switch ${base} && git branch -D ${head}"
    fi
    return 0
  fi
  if git branch -D "${head}" >/dev/null 2>&1; then
    echo "ローカルブランチ ${head} を削除した"
  else
    echo "警告: ローカルブランチ ${head} を削除できなかった（手で: git branch -D ${head}）" >&2
  fi
}

# merge が成立したときの**唯一の終わり方**（#1430）。後始末（リモート / ローカルの head
# ブランチ）を閉じてから、終了コードの意味を 1 行で言い切って 0 で終わる。
# 「gh が 1 で終わった」という警告だけを読んだ自動化が merge 失敗と誤読したのが #1430 の
# 発端なので、最後の 1 行は必ず「merge 成立 / 終了コード 0」と読める形にする
finish_merged() {
  delete_remote_head_branch
  delete_local_head_branch
  echo "merge 成立: PR #${PR} は MERGED（$(pr_field url)）/ 終了コード 0"
  exit 0
}

# 緑を出した CI run の**後に** main が進んでいたら警告する。
# PR の CI は merge 結果を検査するが merge base は run 実行時点で凍るので、
# その後に main へ入った変更との組み合わせは**誰も検査していない**（#1343）。
# これは助言であり merge は止めない（判定に使う情報が取れなくても黙って通す）。
warn_if_base_moved() {
  local base head run_created base_tip_date behind
  base="$(pr_field baseRefName)"
  head="$(pr_field headRefOid)"
  [[ -n "${base}" && -n "${head}" ]] || return 0
  # 緑を出した run（= PR head に対する最新の run）の開始時刻
  run_created="$(gh run list --branch "$(pr_field headRefName)" --limit 20 \
    --json headSha,createdAt -q "[.[] | select(.headSha == \"${head}\")] | max_by(.createdAt) | .createdAt" 2>/dev/null || true)"
  base_tip_date="$(gh api "repos/{owner}/{repo}/commits/${base}" -q .commit.committer.date 2>/dev/null || true)"
  behind="$(gh api "repos/{owner}/{repo}/compare/${base}...${head}" -q .behind_by 2>/dev/null || true)"
  [[ -n "${run_created}" && "${run_created}" != "null" ]] || return 0
  [[ -n "${base_tip_date}" && "${base_tip_date}" != "null" ]] || return 0
  [[ "${behind}" =~ ^[0-9]+$ ]] || return 0
  [[ ${behind} -gt 0 ]] || return 0
  # ISO 8601 の UTC 同士なので文字列比較でよい
  [[ "${base_tip_date}" > "${run_created}" ]] || return 0
  echo "警告: CI が緑になった後に ${base} が進んでいる（未取り込み ${behind} 本 / run ${run_created} < ${base} の先頭 ${base_tip_date}）" >&2
  echo "  その組み合わせは誰も検査していない（#1343 の事故クラス）。取り込んで回し直すなら:" >&2
  echo "    git fetch origin && git merge origin/${base} && git push" >&2
}

fetch_pr
echo "PR #${PR}: $(pr_field title)"
echo "  ブランチ $(pr_field headRefName) / $(pr_field url)"
# 待つ前に落ちる状態なら、CI を待たずにここで止める（無駄に 40 分待たない）
gate_state "待つ前"

echo
RC=0
"${SCRIPT_DIR}/wait-pr-checks.sh" "${PR}" ${FORWARD[@]+"${FORWARD[@]}"} || RC=$?
if [[ ${RC} -ne 0 ]]; then
  if [[ ${RC} -eq ${PR_CONFLICT_EXIT} ]]; then
    # 待ち側が案内を出しているので繰り返さない（#1365）
    echo "merge しない: base と衝突している（wait-pr-checks.sh の終了コード ${RC}）" >&2
  else
    echo "merge しない: CI が揃っていない（wait-pr-checks.sh の終了コード ${RC}）" >&2
  fi
  exit "${RC}"
fi

echo
# 待っている間に main が進む・衝突が生まれることがあるので、merge の直前にもう一度見る
fetch_pr
gate_state "merge 直前"
warn_if_base_moved

echo "squash merge する（--delete-branch）"
set +e
MERGE_OUT="$(gh pr merge "${PR}" --squash --delete-branch 2>&1)"
MERGE_RC=$?
set -e
if [[ -n "${MERGE_OUT}" ]]; then echo "${MERGE_OUT}"; fi

# **gh の終了コードは merge の成否ではない**（#1430）。--delete-branch の後始末は
# すべて merge の**後**に走るので、worktree から実行して checkout に失敗した場合でも
# PR は MERGED になっている。成否は PR の状態の実測だけで決める
fetch_pr
STATE="$(pr_field state)"
if [[ "${STATE}" != "MERGED" ]]; then
  echo "merge に失敗した（PR の状態 ${STATE} / gh の終了コード ${MERGE_RC}）" >&2
  exit 1
fi
if [[ ${MERGE_RC} -ne 0 ]]; then
  echo "注記: gh は ${MERGE_RC} で終わったが merge は成立している（PR の状態は MERGED）。"
  echo "  落ちたのは --delete-branch のローカル後始末（base への切り替え）で、専用 worktree から"
  echo "  実行すると必ずここで落ちる。ブランチの後始末はこの後で自分で閉じる"
fi
finish_merged
