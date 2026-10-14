#!/usr/bin/env bash
# tako:run: bash scripts/ci-change-scope.sh --help
# ci-change-scope.sh — CI の変更範囲を判定する: docs だけなら Rust の重いジョブを skip する（#2084）
#
# なぜ要るか:
#   2026-10-15 未明、ドキュメントサイト刷新の docs だけの PR が 9 本並び、どれも ci.yml の
#   macOS（約 35〜40 分）と Windows（約 30 分）を回した。macOS のランナーは同時に数本しか
#   取れないので約 10 本が順番待ちになり、コードの PR もその後ろに並んだ（1 本で最大 100 分）。
#
# 判定（迷うものは「走らせる」側 = rust=true へ倒す）:
#   変更されたパスが**全部** DOCS_ONLY の型に当たり、**どれも** RUST_REQUIRED に当たらないときだけ
#   rust=false。差分が空・取れない・イベントが想定外のときも rust=true。
#   rust=false でも docs を読む Rust の番犬（`no_personal_data` ほか）は
#   scripts/ci-docs-watchdogs.sh が走らせる（Rust を丸ごと止めると個人情報の番犬まで止まる）。
#
# 使い方:
#   bash scripts/ci-change-scope.sh classify [-z] < パスの一覧   # 1 行 1 パス（-z は NUL 区切り）
#   bash scripts/ci-change-scope.sh github                       # CI 用: 差分を採って GITHUB_OUTPUT へ
#   git diff --name-only --no-renames origin/main... | bash scripts/ci-change-scope.sh classify
#                                                                # 手元: この PR で Rust が skip されるか
#     環境変数 CI_EVENT（github.event_name）/ CI_PUSH_BEFORE（github.event.before）
# 出力: 標準出力の最後の行が rust=true / rust=false。理由は標準エラー（github では step summary にも）
set -euo pipefail

# docs だけとみなすパスの型（bash の case の glob。`*` は `/` も跨ぐ）
DOCS_ONLY=(
  'docs/*'
  '.agent/*.md'
)

# DOCS_ONLY に当たっても Rust が要るもの（`パターン|理由`）。
# 生成ページは macOS ジョブが tako バイナリの出力と突き合わせる（gen-*-docs.mjs --check）ので、
# 手で直した PR を docs だけとして通すと main で初めて食い違いが見つかる
RUST_REQUIRED=(
  'docs/src/content/docs/windows-support.md|生成ページ（gen-windows-support-docs.mjs --check が tako バイナリと突き合わせる）'
  'docs/src/content/docs/agent-support.md|生成ページ（gen-agent-support-docs.mjs --check が tako バイナリと突き合わせる）'
)

# 理由に並べるパスの上限（長い PR で summary を埋めない）
MAX_LISTED=10

usage() {
  # 冒頭のコメント（2 行目の tako:run を除く）が続くあいだを出す
  awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "$0"
}

# 1 パスの分類: docs / rust:<理由>
classify_path() {
  local p="$1" spec pat why
  for spec in ${RUST_REQUIRED[@]+"${RUST_REQUIRED[@]}"}; do
    pat="${spec%%|*}"
    why="${spec#*|}"
    # shellcheck disable=SC2254 # パターンとして展開する
    case "${p}" in ${pat}) printf 'rust:%s' "${why}"; return 0 ;; esac
  done
  for pat in ${DOCS_ONLY[@]+"${DOCS_ONLY[@]}"}; do
    # shellcheck disable=SC2254
    case "${p}" in ${pat}) printf 'docs'; return 0 ;; esac
  done
  printf 'rust:docs/ と .agent/ の md 以外'
}

# 標準入力のパス一覧を判定する。$1 = "-z" なら NUL 区切り
classify_stream() {
  local nul="${1:-}" p verdict n_total=0 n_rust=0 listed=0 delim=$'\n'
  [[ "${nul}" == "-z" ]] && delim=''
  REASONS=()
  while IFS= read -r -d "${delim}" p || [[ -n "${p}" ]]; do
    [[ -n "${p}" ]] || continue
    n_total=$((n_total + 1))
    verdict="$(classify_path "${p}")"
    if [[ "${verdict}" != "docs" ]]; then
      n_rust=$((n_rust + 1))
      if [[ ${listed} -lt ${MAX_LISTED} ]]; then
        REASONS+=("${p} — ${verdict#rust:}")
        listed=$((listed + 1))
      fi
    fi
  done
  if [[ ${n_total} -eq 0 ]]; then
    REASONS=("変更されたパスが 0 件（判定材料が無いので走らせる）")
    RUST=true
  elif [[ ${n_rust} -eq 0 ]]; then
    REASONS=("docs だけの変更（${n_total} 件）: macOS / Windows の Rust ジョブを skip し、docs の検査と docs を読む番犬だけ走らせる")
    RUST=false
  else
    if [[ ${n_rust} -gt ${listed} ]]; then
      REASONS+=("…ほか $((n_rust - listed)) 件")
    fi
    REASONS=("Rust が要る変更（${n_total} 件中 ${n_rust} 件）:" ${REASONS[@]+"${REASONS[@]}"})
    RUST=true
  fi
}

print_reasons() {
  local r
  for r in ${REASONS[@]+"${REASONS[@]}"}; do echo "  ${r}" >&2; done
}

# 判定材料が取れないときの終わり方（走らせる側へ倒す）
conservative() {
  REASONS=("$1（走らせる側へ倒す）")
  RUST=true
}

# CI 用: イベントから差分の基点を決めて、そこから HEAD までの変更を判定する
run_github() {
  local event="${CI_EVENT:-}" before="${CI_PUSH_BEFORE:-}" base="" list
  case "${event}" in
    pull_request)
      # actions/checkout の既定は refs/pull/<PR>/merge = base と PR head の merge コミット。
      # 第 1 親が「CI が組み合わせた base の先頭」なので、ここからの差分が PR の実効的な変更
      if git rev-parse -q --verify 'HEAD^2' >/dev/null 2>&1; then
        base="$(git rev-parse 'HEAD^1')"
      else
        conservative "HEAD が merge コミットではない（checkout の ref / fetch-depth を確認）"
      fi
      ;;
    push)
      if [[ ! "${before}" =~ ^[0-9a-f]{40}$ ]] || [[ "${before}" =~ ^0+$ ]]; then
        conservative "push の before が無い（ブランチの新規作成など: '${before}'）"
      else
        if ! git cat-file -e "${before}^{commit}" 2>/dev/null; then
          git fetch -q --no-tags --depth=1 origin "${before}" 2>/dev/null || true
        fi
        if git cat-file -e "${before}^{commit}" 2>/dev/null; then
          base="${before}"
        else
          conservative "push の before（${before}）を取得できない"
        fi
      fi
      ;;
    *)
      conservative "想定外のイベント（'${event}'）"
      ;;
  esac

  if [[ -n "${base}" ]]; then
    # --no-renames: 名前の変更を「旧パスの削除 + 新パスの追加」で出す。
    # 既定（renames あり）の --name-only は**新しいパスしか出さない**ので、
    # crates/x.rs → docs/x.rs の移動が docs だけに見えてしまう。
    # 一覧は**変数に入れずファイルで渡す**: bash の変数は NUL を持てず、`$(…)` に入れると
    # 区切りが消えてパスが連結される（「docs/a.md」+「scripts/z.sh」→「docs/a.mdscripts/z.sh」が
    # docs/* に当たり、scripts を触った PR を docs だけと誤る）
    list="$(mktemp)"
    if git -c core.quotepath=off diff --name-only --no-renames -z "${base}" HEAD >"${list}"; then
      classify_stream -z <"${list}"
    else
      conservative "git diff が失敗した（${base}..HEAD）"
    fi
    rm -f "${list}"
  fi

  echo "変更範囲の判定（${event:-イベント不明}）: rust=${RUST}" >&2
  print_reasons
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then echo "rust=${RUST}" >> "${GITHUB_OUTPUT}"; fi
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "### 変更範囲の判定（#2084）: rust=${RUST}"
      local r
      for r in ${REASONS[@]+"${REASONS[@]}"}; do echo "- ${r}"; done
    } >> "${GITHUB_STEP_SUMMARY}"
  fi
  if [[ "${RUST}" == "false" ]]; then
    echo "::notice title=docs だけの変更（#2084）::macOS / Windows の Rust ジョブは skip（成功扱い）。docs の検査と docs を読む番犬だけ走る"
  fi
  echo "rust=${RUST}"
}

case "${1:-}" in
  classify)
    classify_stream "${2:-}"
    print_reasons
    echo "rust=${RUST}"
    ;;
  github) run_github ;;
  -h | --help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
