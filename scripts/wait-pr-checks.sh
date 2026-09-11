#!/usr/bin/env bash
# tako:run: bash scripts/wait-pr-checks.sh --help
# wait-pr-checks.sh — PR の CI が「期待するチェックが全部そろって、全部完了」するまで待つ（#1333）
#
# なぜ要るか:
#   「`gh pr checks` に出ているチェックが全部 pending でない = 完了」で判定すると、
#   push 直後の **Cloudflare Pages しか登録されていない瞬間**（macOS / Windows の job は
#   run 開始前なので checks にまだ現れない）に「完了」と読んでしまう。実際に #1313 = PR #1328 が
#   CI 完了前に merge され（merge 04:24:10 / Windows 完了 04:48:24）、過去にも #1253 / #1282 で
#   同じ手順違反が起きている。**存在するチェックだけを見る判定は構造的に早すぎる**。
#
#   さらに GitHub API は replica 差で in_progress / completed が両方向に揺れて返る。
#   そこで判定は 2 段にする:
#     1. 期待するチェック名が**全部そろって**、報告されているチェックが**全部 completed**
#     2. その結論（どの名前がどの bucket か）を **2 回連続**で観測できたら確定
#
# 期待するチェック名の出どころ（ハードコードしない）:
#   - `.github/workflows/*.yml` のうち pull_request で起動するものの job 名（`name:` / 無ければ job id）
#   - 外部連携のチェック（Cloudflare Pages）はワークフローから導出できないので EXTERNAL_CHECKS に持つ
#   - 一時的に別の集合で待ちたいときだけ TAKO_WAIT_PR_CHECKS_EXPECT="A,B" で上書きできる
#
# 「待っても絶対に揃わない」状態は待たない（#1365）:
#   base が進んで衝突すると GitHub は merge コミットを作れず **pull_request の run を作らない**。
#   期待名は永久に未登録のままなので、未登録が残っている間は `mergeable` も見て、
#   衝突していたら名指しで案内して終わる（判定と案内文は lib/pr-conflict.sh の 1 実装）。
#
# 使い方: bash scripts/wait-pr-checks.sh <PR番号> [--timeout <秒>] [--interval <秒>]
# 終了コード: 0 = 全部緑 / 1 = 失敗あり / 2 = タイムアウト / 3 = 引数・gh のエラー /
#             4 = base と衝突していて CI の run が作られない
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 衝突の判定・案内・終了コード（PR_CONFLICT_EXIT）は merge-pr.sh と共有する
# shellcheck source=lib/pr-conflict.sh
. "${SCRIPT_DIR}/lib/pr-conflict.sh"

# 外部連携のチェック（GitHub App が PR ごとに 1 本出す）。ワークフローから導出できないぶんはここ
EXTERNAL_CHECKS=("Cloudflare Pages")

# 実測（2026-09-11 の PR CI）: macOS 12〜13 分 / Windows 13〜15 分、キャッシュが冷えると 25 分。
# job 側の timeout-minutes は macOS 60 / Windows 90 なので、ここで待ち切らない = 手順違反ではなく
# 「揃わなかった（終了コード 2）」として返す
DEFAULT_TIMEOUT=2400
DEFAULT_INTERVAL=20
# 待機が長いときの生存表示（秒）
HEARTBEAT=300

# A/B（検出力の確認・計測専用）。1 = **修正前の判定**をそのまま再現する:
#   「`gh pr checks` に出ているチェックが全部 pending でなければ完了」（未登録は見ない・
#   1 回の観測で確定）。この腕では push 直後の Cloudflare Pages 1 本だけで「完了」を返す
#   = #1313 = PR #1328 が CI 完了前に merge された状態そのもの。
#   scripts/test-wait-pr-checks.sh の Test 13 / 14 がこの腕で穴が開くことを固定している
LEGACY="${TAKO_1333_LEGACY:-0}"

# A/B（検出力の確認・計測専用）。1 = **#1365 の前**へ戻す = 衝突を見ずに、
#   run が作られない PR をタイムアウトまで待ち続ける腕。
#   scripts/test-wait-pr-checks.sh の Test 20legacy がこの腕で待ち続けることを固定している
LEGACY_1365="${TAKO_1365_LEGACY:-0}"

die() {
  echo "エラー: $*" >&2
  exit 3
}

usage() {
  cat <<'USAGE'
使い方: bash scripts/wait-pr-checks.sh <PR番号> [--timeout <秒>] [--interval <秒>]

  PR の CI が「期待するチェックが全部そろって、全部完了」するまで待つ。
  同じ結論を 2 回連続で観測するまで確定しない（GitHub API の揺れ対策）。

終了コード: 0 = 全部緑 / 1 = 失敗あり / 2 = タイムアウト / 3 = 引数・gh のエラー /
            4 = base と衝突していて CI の run が作られない（取り込んで push し直す）
USAGE
}

PR=""
TIMEOUT="${DEFAULT_TIMEOUT}"
INTERVAL="${DEFAULT_INTERVAL}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --timeout)
      [[ $# -ge 2 ]] || die "--timeout には秒数が要る"
      TIMEOUT="$2"
      shift 2
      ;;
    --interval)
      [[ $# -ge 2 ]] || die "--interval には秒数が要る"
      INTERVAL="$2"
      shift 2
      ;;
    -h | --help)
      usage
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

[[ -n "${PR}" ]] || {
  usage >&2
  die "PR 番号が要る"
}
[[ "${PR}" =~ ^[0-9]+$ ]] || die "PR 番号が数値ではない: ${PR}"
[[ "${TIMEOUT}" =~ ^[0-9]+$ ]] || die "--timeout が数値ではない: ${TIMEOUT}"
[[ "${INTERVAL}" =~ ^[0-9]+$ ]] && [[ "${INTERVAL}" -ge 1 ]] || die "--interval は 1 以上の整数: ${INTERVAL}"
command -v gh >/dev/null 2>&1 || die "gh が見つからない（https://cli.github.com/）"
command -v jq >/dev/null 2>&1 || die "jq が見つからない（brew install jq）"

# ---------------------------------------------------------------- 期待するチェック名

repo_root() {
  git rev-parse --show-toplevel 2>/dev/null && return 0
  (cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
}

# pull_request で起動するワークフローか（`on:` ブロックだけを見る。pull_request_target も対象）
workflow_on_pull_request() {
  awk '
    /^[^ \t#]/ { in_on = ($0 ~ /^"?on"?:/) }
    in_on { line = $0; sub(/#.*/, "", line)
            if (line ~ /(^|[^_A-Za-z0-9])pull_request([^_A-Za-z0-9]|$)/) found = 1 }
    END { exit(found ? 0 : 1) }
  ' "$1"
}

# ワークフロー 1 本から job のチェック名を出す（1 行 1 名前）。
# `name:` が無い job は job id がそのままチェック名になる。
# matrix を使う job は GitHub が「name (値)」へ展開するのでこの導出では追いつかない
# （現状 tako の CI に matrix は無い。増やすときはここも直す）。
workflow_job_names() {
  awk '
    BEGIN { squote = sprintf("%c", 39) }
    function emit() {
      if (have) print (name != "" ? name : jobid)
      have = 0; name = ""; jobid = ""
    }
    {
      line = $0
      sub(/[ \t]+$/, "", line)
      if (line ~ /^[ \t]*#/) next
      if (!in_jobs) { if (line ~ /^jobs:[ \t]*$/) in_jobs = 1; next }
      if (line ~ /^[^ \t]/) { emit(); in_jobs = 0; next }
      if (line ~ /^  [A-Za-z0-9_-]+:[ \t]*$/) {
        emit()
        jobid = line; sub(/^  /, "", jobid); sub(/:[ \t]*$/, "", jobid)
        have = 1
        next
      }
      # steps の name は 6 桁以上の字下げなので、4 桁ちょうどが job の表示名
      if (have && name == "" && line ~ /^    name:[ \t]*[^ \t]/) {
        name = line; sub(/^    name:[ \t]*/, "", name)
        first = substr(name, 1, 1); last = substr(name, length(name), 1)
        if (length(name) >= 2 && first == last && (first == "\"" || first == squote))
          name = substr(name, 2, length(name) - 2)
      }
    }
    END { emit() }
  ' "$1"
}

expected_checks() {
  if [[ -n "${TAKO_WAIT_PR_CHECKS_EXPECT:-}" ]]; then
    tr ',' '\n' <<<"${TAKO_WAIT_PR_CHECKS_EXPECT}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' | grep -v '^$'
    return 0
  fi
  local dir f
  dir="$(repo_root)/.github/workflows"
  if [[ -d "${dir}" ]]; then
    for f in "${dir}"/*.yml "${dir}"/*.yaml; do
      [[ -f "${f}" ]] || continue
      workflow_on_pull_request "${f}" || continue
      workflow_job_names "${f}"
    done
  fi
  printf '%s\n' "${EXTERNAL_CHECKS[@]}"
}

EXPECTED=()
while IFS= read -r _name; do
  [[ -n "${_name}" ]] && EXPECTED+=("${_name}")
done < <(expected_checks | awk '!seen[$0]++')
[[ ${#EXPECTED[@]} -gt 0 ]] || die "期待するチェック名が 1 つも決まらない（.github/workflows を読めているか）"

# ---------------------------------------------------------------- gh から今の状態を採る

# 1 行 1 チェック（bucket <TAB> completedAt <TAB> name）
CHECKS_RAW=""
POLL_ERR=""
POLL_RC=0
poll_checks() {
  local out err
  err="$(mktemp)"
  set +e
  out="$(gh pr checks "${PR}" --json name,bucket,state,completedAt 2>"${err}")"
  POLL_RC=$?
  set -e
  POLL_ERR="$(cat "${err}")"
  rm -f "${err}"
  # gh の終了コードは 0 = 全部 pass / 1 = 失敗あり / 8 = pending。どれも JSON は出る
  if [[ -z "${out}" ]]; then
    # チェックが 1 本も登録されていない間は gh がエラーで返る（run 開始前 = まだ待つ）
    if grep -qi "no checks reported" <<<"${POLL_ERR}"; then
      CHECKS_RAW=""
      return 0
    fi
    return 1
  fi
  CHECKS_RAW="$(jq -r '.[] | [.bucket, (.completedAt // ""), .name] | @tsv' <<<"${out}")" || return 1
  return 0
}

field_of() { # $1 = 列番号, $2 = チェック名
  [[ -n "${CHECKS_RAW}" ]] || return 0
  awk -F'\t' -v col="$1" -v want="$2" '$3 == want { print $col; exit }' <<<"${CHECKS_RAW}"
}

at_of() { # 完了時刻（未完了は gh が 0001-01-01T00:00:00Z を返す）
  local at
  at="$(field_of 2 "$1")"
  case "${at}" in
    "" | 0001-01-01*) printf '%s' "-" ;;
    *) printf '%s' "${at}" ;;
  esac
}

MISSING=()
PENDING_N=0
FAILED_N=0
DONE_N=0
REPORTED_N=0
evaluate() {
  local name bucket
  MISSING=()
  PENDING_N=0
  FAILED_N=0
  DONE_N=0
  REPORTED_N=0
  for name in "${EXPECTED[@]}"; do
    bucket="$(field_of 1 "${name}")"
    [[ -n "${bucket}" ]] || MISSING+=("${name}")
  done
  if [[ -n "${CHECKS_RAW}" ]]; then
    while IFS=$'\t' read -r bucket _at _name; do
      [[ -n "${bucket}" ]] || continue
      REPORTED_N=$((REPORTED_N + 1))
      case "${bucket}" in
        pending) PENDING_N=$((PENDING_N + 1)) ;;
        fail | cancel)
          FAILED_N=$((FAILED_N + 1))
          DONE_N=$((DONE_N + 1))
          ;;
        *) DONE_N=$((DONE_N + 1)) ;;
      esac
    done <<<"${CHECKS_RAW}"
  fi
}

# 未登録が残っているときだけ引く PR の状態（#1365）。
# 既存の待ち（gh pr checks）とは別の口なので、**取れなかったら黙って諦める**
# （衝突の判定材料が無いだけで、CI の結論は pr checks が正）。
PR_MERGEABLE=""
PR_MERGE_STATE=""
PR_BASE_REF=""
probe_mergeable() {
  local out rc
  set +e
  out="$(gh pr view "${PR}" --json mergeable,mergeStateStatus,baseRefName 2>/dev/null)"
  rc=$?
  set -e
  [[ ${rc} -eq 0 && -n "${out}" ]] || return 1
  PR_MERGEABLE="$(jq -r '.mergeable // "" | tostring' <<<"${out}" 2>/dev/null)" || return 1
  PR_MERGE_STATE="$(jq -r '.mergeStateStatus // "" | tostring' <<<"${out}" 2>/dev/null)" || return 1
  PR_BASE_REF="$(jq -r '.baseRefName // "" | tostring' <<<"${out}" 2>/dev/null)" || return 1
  return 0
}

# 「同じ結論か」を比べるための指紋（報告された name=bucket と、未登録の期待名）
signature() {
  {
    if [[ -n "${CHECKS_RAW}" ]]; then awk -F'\t' '{ print "R " $3 "=" $1 }' <<<"${CHECKS_RAW}"; fi
    local n
    for n in ${MISSING[@]+"${MISSING[@]}"}; do printf 'M %s\n' "${n}"; done
  } | LC_ALL=C sort | tr '\n' '|'
}

join_names() {
  local out="" n
  for n in "$@"; do
    [[ -z "${out}" ]] && out="${n}" || out="${out}, ${n}"
  done
  printf '%s' "${out}"
}

now() { date +%H:%M:%S; }

elapsed_text() {
  local s="$1"
  printf '%d分%02d秒' "$((s / 60))" "$((s % 60))"
}

# ---------------------------------------------------------------- 待つ

echo "PR #${PR} の CI を待つ（期待 ${#EXPECTED[@]} 本 / 上限 ${TIMEOUT} 秒 / 間隔 ${INTERVAL} 秒）"
for _name in "${EXPECTED[@]}"; do echo "  期待: ${_name}"; done

START="$(date +%s)"
PREV_STATE=""
CONFIRM_SIG=""
LAST_HEARTBEAT=0
POLLS=0
CONFLICT_SEEN=0
PROBE_WARNED=0

while :; do
  if ! poll_checks; then
    echo "エラー: gh pr checks が失敗した（PR #${PR} / 終了コード ${POLL_RC}）" >&2
    if [[ -n "${POLL_ERR}" ]]; then echo "${POLL_ERR}" >&2; fi
    exit 3
  fi
  POLLS=$((POLLS + 1))
  evaluate

  # 変化した行だけ出す
  CUR_STATE="$(
    [[ -n "${CHECKS_RAW}" ]] && awk -F'\t' '{ print $3 "\t" $1 }' <<<"${CHECKS_RAW}" | LC_ALL=C sort
    true
  )"
  if [[ "${CUR_STATE}" != "${PREV_STATE}" ]]; then
    while IFS=$'\t' read -r cname cbucket; do
      [[ -n "${cname}" ]] || continue
      pbucket=""
      if [[ -n "${PREV_STATE}" ]]; then
        pbucket="$(awk -F'\t' -v want="${cname}" '$1 == want { print $2; exit }' <<<"${PREV_STATE}")"
      fi
      if [[ -z "${pbucket}" ]]; then
        echo "[$(now)] 登録: ${cname} = ${cbucket}"
      elif [[ "${pbucket}" != "${cbucket}" ]]; then
        echo "[$(now)] 変化: ${cname} ${pbucket} → ${cbucket}（完了 $(at_of "${cname}")）"
      fi
    done <<<"${CUR_STATE}"
    PREV_STATE="${CUR_STATE}"
  fi

  SIG="$(signature)"
  READY=0
  if [[ ${REPORTED_N} -gt 0 && ${PENDING_N} -eq 0 ]]; then
    # 修正の本体: **期待する名前が全部そろっている**ことまで要求する（旧判定はここが無い）
    if [[ "${LEGACY}" == "1" || ${#MISSING[@]} -eq 0 ]]; then READY=1; fi
  fi
  if [[ ${READY} -eq 1 ]]; then
    if [[ "${LEGACY}" == "1" || "${SIG}" == "${CONFIRM_SIG}" ]]; then
      break # 同じ結論を 2 回連続で観測 = 確定（LEGACY は 1 回で確定する）
    fi
    echo "[$(now)] 全 ${REPORTED_N} 本が完了（1 回目の観測。同じ結論をもう 1 回見るまで確定しない）"
    CONFIRM_SIG="${SIG}"
  elif [[ -n "${CONFIRM_SIG}" ]]; then
    echo "[$(now)] 揺れ: 完了の観測を取り消す（未登録 ${#MISSING[@]} 本 / 実行中 ${PENDING_N} 本）"
    CONFIRM_SIG=""
  fi

  # 未登録が残っているなら、それが「まだ run が始まっていない」のか
  # 「衝突して run が作られない」のかを見分ける（#1365）。
  # 揃ってしまえば引かない = 通常の待ちで増える gh の呼び出しは最初の数回だけ
  if [[ "${LEGACY_1365}" == "1" || ${#MISSING[@]} -eq 0 ]]; then
    CONFLICT_SEEN=0
  elif probe_mergeable; then
    case "$(pr_conflict_verdict "${PR_MERGEABLE}" "${PR_MERGE_STATE}")" in
      conflicting)
        # チェックの結論と同じく、1 回の観測では確定しない（push 直後は古い値が返る）
        if [[ ${CONFLICT_SEEN} -eq 1 ]]; then
          pr_conflict_report wait "${PR}" "${PR_BASE_REF}" "${PR_MERGEABLE}" "${PR_MERGE_STATE}"
          echo "  未登録のまま: $(join_names "${MISSING[@]}")" >&2
          exit "${PR_CONFLICT_EXIT}"
        fi
        echo "[$(now)] 衝突を観測（mergeable=${PR_MERGEABLE} / ${PR_MERGE_STATE}）。同じ結論をもう 1 回見るまで確定しない"
        CONFLICT_SEEN=1
        ;;
      # UNKNOWN = GitHub がまだ計算していない。待ちを続ける
      *) CONFLICT_SEEN=0 ;;
    esac
  else
    CONFLICT_SEEN=0
    if [[ ${PROBE_WARNED} -eq 0 ]]; then
      echo "[$(now)] 警告: gh pr view が取れないので衝突の判定を省く（待ちは続ける）" >&2
      PROBE_WARNED=1
    fi
  fi

  NOW_ELAPSED=$(($(date +%s) - START))
  if [[ ${NOW_ELAPSED} -ge ${TIMEOUT} ]]; then
    echo "タイムアウト（${TIMEOUT} 秒 / ${POLLS} 回確認）: CI が揃わなかった"
    if [[ ${#MISSING[@]} -gt 0 ]]; then echo "  未登録: $(join_names "${MISSING[@]}")"; fi
    if [[ ${PENDING_N} -gt 0 ]]; then echo "  実行中: ${PENDING_N} 本"; fi
    echo "  merge してはいけない（CI の結論が出ていない）"
    exit 2
  fi
  if [[ $((NOW_ELAPSED - LAST_HEARTBEAT)) -ge ${HEARTBEAT} ]]; then
    echo "[$(now)] 待機中（経過 $(elapsed_text "${NOW_ELAPSED}") / 登録 $((${#EXPECTED[@]} - ${#MISSING[@]}))/${#EXPECTED[@]} / 実行中 ${PENDING_N} 本）"
    LAST_HEARTBEAT="${NOW_ELAPSED}"
  fi
  sleep "${INTERVAL}"
done

TOTAL=$(($(date +%s) - START))
echo "結果（PR #${PR} / 経過 $(elapsed_text "${TOTAL}") / ${POLLS} 回確認）:"
while IFS=$'\t' read -r bucket at name; do
  [[ -n "${name}" ]] || continue
  case "${at}" in 0001-01-01* | "") at="-" ;; esac
  echo "  [${bucket}] ${name} — 完了 ${at}"
done < <(LC_ALL=C sort -t$'\t' -k3 <<<"${CHECKS_RAW}")

if [[ ${FAILED_N} -gt 0 ]]; then
  echo "失敗しているチェックが ${FAILED_N} 本ある。merge してはいけない"
  exit 1
fi
echo "CI が全部緑で揃った（期待 ${#EXPECTED[@]} 本 / 報告 ${REPORTED_N} 本）"
exit 0
