#!/usr/bin/env bash
# tako:run: bash scripts/ci-docs-watchdogs.sh --list
# ci-docs-watchdogs.sh — docs / .agent を読む Rust の番犬だけを拾って走らせる（#2084）
#
# docs だけの PR では macOS / Windows の Rust ジョブを skip する（scripts/ci-change-scope.sh）が、
# Rust の結合テストには docs/ や .agent/ の md を**読んで検査する**ものがある
# （`no_personal_data` = 個人情報の番犬 #927 はリポジトリの全ファイル、`context_budget` は
# .agent/progress.md、`docs_tool_inventory` は docs のページ、など）。Rust を丸ごと止めると
# これらも止まり、docs だけの PR で混入した個人情報や壊れた docs がそのまま main へ入る。
#
# 一覧は**手で持たない**（足し忘れた番犬が黙って外れるため）。`crates/*/tests/*.rs` を毎回走査し、
# コメント行を除いた本文が次のどれかに当たるテストを拾う:
#   docs   : "docs/…" / ".agent/…" / join("docs") のような docs・.agent を指す文字列リテラル
#   walker : read_dir で歩き、"target" か "node_modules" を除外している（= リポジトリ全体を歩く）
#   git    : "ls-files"（git の追跡一覧を読む）
# 余分に拾うのは数秒の損で済むが、拾い損ねると docs だけの PR で番犬が止まるので、広く拾う側に倒す。
#
# 使い方:
#   bash scripts/ci-docs-watchdogs.sh --list   # 拾ったテストを「パッケージ<TAB>テスト名<TAB>理由」で出す
#   bash scripts/ci-docs-watchdogs.sh          # 拾ったテストを cargo test で走らせる
#   bash scripts/ci-docs-watchdogs.sh -- --test-threads=4   # `--` の後ろはテストバイナリへ渡す
#   環境変数 TAKO_DOCS_WATCHDOGS_ROOT でリポジトリのルートを差し替えられる（テスト用）
set -euo pipefail

ROOT="${TAKO_DOCS_WATCHDOGS_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

# 拾い方が壊れていないことの錨。これが拾えないなら走査の型が変わったので、黙って通さない
ANCHORS=(no_personal_data context_budget docs_tool_inventory)

usage() {
  # 冒頭のコメント（2 行目の tako:run を除く）が続くあいだを出す
  awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "$0"
}

# パッケージ名（Cargo.toml の [package] の name）。取れなければディレクトリ名
package_of() {
  local dir="$1" name
  name="$(awk '
    /^\[/ { in_pkg = ($0 ~ /^\[package\][ \t]*$/) ; next }
    in_pkg && /^name[ \t]*=/ { sub(/^name[ \t]*=[ \t]*"/, ""); sub(/".*$/, ""); print; exit }
  ' "${dir}/Cargo.toml" 2>/dev/null || true)"
  printf '%s' "${name:-$(basename "${dir}")}"
}

# 1 行 1 本: パッケージ<TAB>テスト名<TAB>理由
list_targets() {
  local f dir body why
  for f in "${ROOT}"/crates/*/tests/*.rs; do
    [[ -f "${f}" ]] || continue
    # コメント行（// /// //!）は「読む」証拠にしない（「書き方は .agent/conventions.md」の類が多い）
    body="$(grep -vE '^[[:space:]]*//' "${f}" || true)"
    why=""
    if grep -qE '"(\.\./)*(docs|\.agent)(/|")' <<<"${body}"; then
      why="docs"
    elif grep -q 'read_dir' <<<"${body}" && grep -qE '"(target|node_modules)"' <<<"${body}"; then
      why="walker"
    elif grep -qF '"ls-files"' <<<"${body}"; then
      why="git"
    fi
    [[ -n "${why}" ]] || continue
    dir="$(dirname "$(dirname "${f}")")"
    printf '%s\t%s\t%s\n' "$(package_of "${dir}")" "$(basename "${f}" .rs)" "${why}"
  done
}

TARGETS="$(list_targets)"

missing=()
for a in ${ANCHORS[@]+"${ANCHORS[@]}"}; do
  awk -F'\t' -v t="${a}" '$2 == t { found = 1 } END { exit(found ? 0 : 1) }' <<<"${TARGETS}" || missing+=("${a}")
done
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "エラー: docs を読む番犬の錨（${missing[*]+"${missing[*]}"}）を拾えなかった。" >&2
  echo "  テストの置き場か書き方が変わったので、scripts/ci-docs-watchdogs.sh の走査を直すこと" >&2
  echo "  （このまま通すと docs だけの PR で番犬が黙って止まる）" >&2
  exit 1
fi

case "${1:-}" in
  --list)
    printf '%s\n' "${TARGETS}"
    exit 0
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  "" | --) ;;
  *)
    usage >&2
    exit 2
    ;;
esac

PASS_THROUGH=()
if [[ "${1:-}" == "--" ]]; then
  shift
  PASS_THROUGH=("$@")
fi

ARGS=()
while IFS=$'\t' read -r _pkg name _why; do
  [[ -n "${name}" ]] && ARGS+=(--test "${name}")
done <<<"${TARGETS}"
echo "docs / .agent を読む番犬 $((${#ARGS[@]} / 2)) 本を走らせる:"
printf '%s\n' "${TARGETS}" | awk -F'\t' '{ printf "  %-50s %s\n", $2, $3 }'
# --workspace: full の macOS ジョブ（cargo test --workspace）と同じ feature の宇宙で組む。
# `-p tako-control` だと gpui が要求する feature（libc/extra_traits・smallvec/union ほか。
# serde_json/preserve_order は #1829 から全宇宙で有効）が外れ、同じテストが別の宇宙で走る（#1383 の食い違いと同じ型）
cd "${ROOT}"
exec cargo test --workspace ${ARGS[@]+"${ARGS[@]}"} -- ${PASS_THROUGH[@]+"${PASS_THROUGH[@]}"}
