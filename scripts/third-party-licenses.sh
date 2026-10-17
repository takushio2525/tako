#!/usr/bin/env bash
# tako:run: bash scripts/third-party-licenses.sh --check
# third-party-licenses.sh — THIRD-PARTY-LICENSES.md を今の依存から作り直す / ずれを検査する（#1851）
#
# なぜ要るか:
#   THIRD-PARTY-LICENSES.md は両 OS の配布物に同梱する生成物（#1709 / #1845）。依存を足し引き・
#   更新したのに作り直しを忘れると、配布物のライセンス表記が実態とずれる。着手時点（2026-10-18）の
#   main では LSP の依存 4 クレート（fluent-uri / ignore / lsp-types / serde_repr）が載っていなかった。
#   CI（ci.yml の licenses ジョブ）が --check で落とす。
#
# 使い方:
#   bash scripts/third-party-licenses.sh           # THIRD-PARTY-LICENSES.md を作り直す
#   bash scripts/third-party-licenses.sh --check   # 作り直した物とコミット済みの物を比べる（書き換えない）
#     一致しなければ、載っていない / 余分なクレートと作り直しのコマンドを出して終了コード 1
#
# 生成の約束（手元と CI で同じ出力になるように）:
#   - ツールは cargo-about。版で出力が変わりうるので CARGO_ABOUT_VERSION と違えば落ちる
#     （CI の版は ci.yml の install-action の `tool: cargo-about@<版>`。上げるときは両方を揃える）
#   - 設定は about.toml（対象 = aarch64-apple-darwin / x86_64-pc-windows-msvc で、走らせる OS に
#     依らない。dev / build 依存は除外）、書式は about.hbs
#   - 先に `cargo fetch --locked` で全 target の依存のソースを揃え、cargo-about は `--frozen`
#     （ネットワークに出ない・Cargo.lock を変えない）で走らせる。オンラインのままだと LICENSE を
#     同梱しないクレートの本文を上流の git から取りに行くので、上流の変更で出力が揺れうる
#     （2026-10-18 の実測では、オンラインとオフラインの出力は改行コードを除いて同一）
#   - クレートが同梱する LICENSE の CRLF は LF へ揃える（git は .gitattributes の eol=lf で揃えるので、
#     揃えずに書くと作り直すたびに作業ツリーとインデックスで改行が食い違う）
#
# 環境変数: TAKO_LICENSES_ROOT（対象のリポジトリのルート。既定はこのスクリプトの親。テスト用）
set -euo pipefail

CARGO_ABOUT_VERSION="0.9.2"
OUT_NAME="THIRD-PARTY-LICENSES.md"
REGEN_CMD="bash scripts/third-party-licenses.sh"
INSTALL_CMD="cargo install cargo-about --locked --version ${CARGO_ABOUT_VERSION}"

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="${TAKO_LICENSES_ROOT:-${SCRIPT_ROOT}}"
OUT="${ROOT}/${OUT_NAME}"

usage() {
  # 冒頭のコメント（2 行目の tako:run を除く）が続くあいだを出す
  awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "$0"
}

MODE=generate
case "${1:-}" in
  "") ;;
  --check) MODE=check ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "third-party-licenses.sh: 知らない引数: ${1}（引数なしで作り直し、--check で検査）" >&2
    exit 2
    ;;
esac

# shellcheck source=lib/exit-guard.sh
. "${SCRIPT_ROOT}/scripts/lib/exit-guard.sh"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/tako-licenses-XXXXXX")"
cleanup() { rm -rf "${WORK}"; }
tako_exit_trap cleanup "ライセンス一覧の作り直し"

die() {
  echo "third-party-licenses.sh: $*" >&2
  exit 1
}

# 一覧の「使用しているクレート」節から「名前 版」を 1 行ずつ出す（tako 本体は版を出さないので名前だけ）。
# ライセンス本文の中の箇条書き（`- ` で始まる行）を拾わないよう、節の見出しから本文の囲みまでに限る
crate_keys() {
  [[ -f "$1" ]] || return 0
  awk '
    /^使用しているクレート/ { in_list = 1; next }
    /^```/ { in_list = 0; next }
    in_list && /^- / {
      s = substr($0, 3)
      if (s ~ /^\[/) { sub(/^\[/, "", s); sub(/\].*$/, "", s) } else { sub(/（.*$/, "", s) }
      print s
    }
  ' "$1" | LC_ALL=C sort -u
}

# cargo-about の有無と版（版が違うと同じ依存でも出力が変わりうる）
ver_line="$(cargo about --version 2>/dev/null)" || ver_line=""
[[ -n "${ver_line}" ]] || die "cargo-about が見つかりません。入れ方: ${INSTALL_CMD}"
have="${ver_line##* }"
if [[ "${have}" != "${CARGO_ABOUT_VERSION}" ]]; then
  die "cargo-about の版が ${have} です（要るのは ${CARGO_ABOUT_VERSION}。版で出力が変わりうる）。入れ直し: ${INSTALL_CMD}"
fi

# 依存のソースを揃える（全 target ぶん。ネットワークに出るのはここだけ）
(cd "${ROOT}" && cargo fetch --locked --quiet) ||
  die "依存のソースを取れませんでした（cargo fetch --locked）。Cargo.lock が Cargo.toml に追いついていないなら、cargo build で Cargo.lock を更新してから打ち直す"

RAW="${WORK}/raw.md"
NEW="${WORK}/${OUT_NAME}"
(cd "${ROOT}" && cargo about generate --frozen --fail about.hbs -o "${RAW}") ||
  die "cargo about generate が失敗しました（上の出力を参照。about.toml の accepted に無いライセンスの依存は --fail で落ちる）"
sed $'s/\r$//' "${RAW}" > "${NEW}"

crate_keys "${OUT}" > "${WORK}/old.keys"
crate_keys "${NEW}" > "${WORK}/new.keys"
LC_ALL=C comm -13 "${WORK}/old.keys" "${WORK}/new.keys" > "${WORK}/missing"
LC_ALL=C comm -23 "${WORK}/old.keys" "${WORK}/new.keys" > "${WORK}/extra"
n_total=$(wc -l < "${WORK}/new.keys" | tr -d ' ')
n_missing=$(wc -l < "${WORK}/missing" | tr -d ' ')
n_extra=$(wc -l < "${WORK}/extra" | tr -d ' ')

# 載っていない / 余分なクレートの一覧（$1 = 載っていない側の見出し / $2 = 余分な側の見出し）
print_crate_diff() {
  echo "  ${1}: ${n_missing} 件"
  sed 's/^/    /' "${WORK}/missing"
  echo "  ${2}: ${n_extra} 件"
  sed 's/^/    /' "${WORK}/extra"
}

if cmp -s "${OUT}" "${NEW}"; then
  echo "${OUT_NAME} は今の依存と一致しています（${n_total} クレート・cargo-about ${CARGO_ABOUT_VERSION}）"
  tako_exit 0
fi

if [[ "${MODE}" == "generate" ]]; then
  cp "${NEW}" "${OUT}"
  echo "${OUT_NAME} を作り直しました（${n_total} クレート・cargo-about ${CARGO_ABOUT_VERSION}）"
  print_crate_diff "足したクレート" "外したクレート"
  if [[ "${n_missing}" == 0 && "${n_extra}" == 0 ]]; then
    echo "  クレートの顔ぶれは同じで、本文（ライセンスの割り当て・本文・書式）が変わりました"
  fi
  echo "書き換わった ${OUT_NAME} をコミットしてください"
  tako_exit 0
fi

{
  echo "${OUT_NAME} が今の依存（Cargo.lock）から作り直した物と一致しません（#1851）"
  print_crate_diff "載っていないクレート（依存にあるのに一覧に無い）" "余分なクレート（依存から消えたのに一覧に残っている）"
  if [[ "${n_missing}" == 0 && "${n_extra}" == 0 ]]; then
    echo "  クレートの顔ぶれは同じで、本文（ライセンスの割り当て・本文・書式）が違います。先頭の差分:"
    diff -u "${OUT}" "${NEW}" > "${WORK}/text.diff" 2>&1 || true
    sed -n '1,40p' "${WORK}/text.diff" | sed 's/^/    /'
  fi
  echo "直し方: ${REGEN_CMD} を実行し、書き換わった ${OUT_NAME} をコミットする"
} >&2
if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
  echo "::error file=${OUT_NAME},title=ライセンス一覧が依存と一致しない（#1851）::載っていない ${n_missing} 件・余分 ${n_extra} 件。${REGEN_CMD} で作り直してコミットする"
fi
exit 1
