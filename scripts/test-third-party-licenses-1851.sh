#!/usr/bin/env bash
# tako:run: bash scripts/test-third-party-licenses-1851.sh
# #1851 のモックテスト: ライセンス一覧の検査（scripts/third-party-licenses.sh）の診断と生成の約束。
#
# 本物の cargo-about は手元で約 30 秒・CPU を数コア使うので、ここでは偽の cargo を PATH の先頭に置き
# （`fetch` は記録だけ、`about --version` は振れる版、`about generate` は用意した出力を -o へ写す）、
# テンポラリの偽リポジトリだけを相手にする。本物のリポジトリの THIRD-PARTY-LICENSES.md には触らない。
# 本物の生成と突き合わせは CI の licenses ジョブの `--check` が受け持つ。
#
# 見ること:
#   A. 一致なら 0。作り直した側の CRLF は LF へ揃えて比べる（git の eol=lf と同じ形）
#   B. 依存を足した形 = 載っていないクレートを名指しし、作り直しのコマンドを出して 1。一覧は書き換えない
#   C. 依存を消した形 = 余分なクレートを名指しして 1
#   D. 顔ぶれが同じで本文だけ違う = そう言って先頭の差分を出す
#   E. cargo-about の版違い・未導入 = 入れ方を出して 1（生成はしない）
#   F. 引数なし = 作り直して書き込む（CRLF を残さない）。2 回目は「一致」
#   G. 生成の約束 = 先に cargo fetch --locked、cargo-about は --frozen（ネットワークに出ない）
#   H. CI（GITHUB_ACTIONS=true）では ::error の注釈を出す / 知らない引数は 2
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/scripts/third-party-licenses.sh"
PASS=0
FAIL=0

ok() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
ng() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }
check() { if [[ "$2" == "$3" ]]; then ok "$1"; else ng "$1（期待 '$3' / 実際 '$2'）"; fi; }
contains() { if grep -qF -- "$2" "$3"; then ok "$1"; else ng "$1（'$2' が出ていない）"; fi; }
absent() { if grep -qF -- "$2" "$3"; then ng "$1（'$2' が出ている）"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1851-XXXXXX")"
cleanup() { rm -rf "${SANDBOX}"; }
trap cleanup EXIT

FAKE="${SANDBOX}/repo"
BIN="${SANDBOX}/bin"
FIX="${SANDBOX}/fixtures"
OUTLOG="${SANDBOX}/out"
mkdir -p "${FAKE}" "${BIN}" "${FIX}"
export FAKE_CARGO_LOG="${SANDBOX}/cargo.log"

# 偽の cargo。fetch は記録だけ、about --version は FAKE_ABOUT_VERSION（空なら「about が無い」）、
# about generate は FAKE_ABOUT_OUTPUT を -o の先へ写す
cat > "${BIN}/cargo" <<'STUB'
#!/usr/bin/env bash
echo "$*" >> "${FAKE_CARGO_LOG}"
case "${1:-} ${2:-}" in
  "fetch "*) exit "${FAKE_FETCH_RC:-0}" ;;
  "about --version")
    if [ -z "${FAKE_ABOUT_VERSION:-}" ]; then echo "error: no such command: \`about\`" >&2; exit 101; fi
    echo "cargo-about ${FAKE_ABOUT_VERSION}"
    ;;
  "about generate")
    out=""
    while [ $# -gt 0 ]; do
      if [ "$1" = "-o" ]; then out="$2"; fi
      shift
    done
    cp "${FAKE_ABOUT_OUTPUT}" "${out}"
    ;;
  *) echo "偽 cargo: 想定外の呼び出し: $*" >&2; exit 99 ;;
esac
STUB
chmod +x "${BIN}/cargo"

# 一覧の雛形。$1 以降 = 「使用しているクレート」に並べる行。本文に `- ` で始まる行を入れて、
# 本文の箇条書きをクレートと読まないことも同時に見る
listing() {
  printf '# サードパーティのライセンス / Third-Party Licenses\n\n## ライセンス本文 / License texts\n\n'
  printf '### MIT License\n\n使用しているクレート / Used by:\n\n'
  local line
  for line in "$@"; do printf '%s\n' "${line}"; done
  printf '\n```text\nCopyright (c) 2026 Example\n\n- not-a-crate 9.9.9 (license text bullet)\n```\n\n'
}
TAKO_LINE='- [tako-core](https://github.com/example/tako)（tako 本体 / tako itself）'
A_LINE='- [alpha 1.0.0](https://example.com/alpha)'
B_LINE='- [beta 2.1.0](https://example.com/beta)'
C_LINE='- [gamma 0.3.4](https://example.com/gamma)'
LOCAL_LINE='- delta 0.1.0（`registry+https://example.com/index`）'

listing "${TAKO_LINE}" "${A_LINE}" "${B_LINE}" "${LOCAL_LINE}" > "${FIX}/base.md"
# 作り直した側だけ CRLF（クレートの LICENSE が CRLF だった形）
sed $'s/$/\r/' "${FIX}/base.md" > "${FIX}/base-crlf.md"
listing "${TAKO_LINE}" "${A_LINE}" "${B_LINE}" "${C_LINE}" "${LOCAL_LINE}" > "${FIX}/added.md"
listing "${TAKO_LINE}" "${A_LINE}" "${LOCAL_LINE}" > "${FIX}/removed.md"
sed $'s/$/\r/' "${FIX}/added.md" > "${FIX}/added-crlf.md"
sed 's/Copyright (c) 2026 Example/Copyright (c) 2027 Example/' "${FIX}/base.md" > "${FIX}/text-changed.md"

# $1 = 生成させる出力 / 残り = スクリプトの引数。終了コードを RC へ、出力を OUTLOG へ
run() {
  local gen="$1"
  shift
  : > "${FAKE_CARGO_LOG}"
  PATH="${BIN}:${PATH}" FAKE_ABOUT_OUTPUT="${gen}" TAKO_LICENSES_ROOT="${FAKE}" \
    FAKE_ABOUT_VERSION="${FAKE_ABOUT_VERSION-0.9.2}" GITHUB_ACTIONS="${GITHUB_ACTIONS_SIM:-}" \
    bash "${SCRIPT}" "$@" > "${OUTLOG}" 2>&1
  RC=$?
}

echo "== A: 一致なら 0（作り直した側の CRLF は LF へ揃えて比べる）=="
cp "${FIX}/base.md" "${FAKE}/THIRD-PARTY-LICENSES.md"
run "${FIX}/base-crlf.md" --check
check "一致なら終了コード 0" "${RC}" "0"
contains "一致と言う" "は今の依存と一致しています（4 クレート" "${OUTLOG}"

echo "== B: 依存を足した形 = 載っていないクレートを名指しして 1 =="
cp "${FIX}/base.md" "${FAKE}/THIRD-PARTY-LICENSES.md"
run "${FIX}/added.md" --check
check "終了コード 1" "${RC}" "1"
contains "載っていない件数" "載っていないクレート（依存にあるのに一覧に無い）: 1 件" "${OUTLOG}"
contains "足りないクレートを名指しする" "    gamma 0.3.4" "${OUTLOG}"
contains "余分は 0 件" "余分なクレート（依存から消えたのに一覧に残っている）: 0 件" "${OUTLOG}"
contains "作り直しのコマンドを出す" "直し方: bash scripts/third-party-licenses.sh を実行し" "${OUTLOG}"
absent "本文の箇条書きをクレートと読まない" "not-a-crate" "${OUTLOG}"
check "--check は一覧を書き換えない" "$(cmp -s "${FIX}/base.md" "${FAKE}/THIRD-PARTY-LICENSES.md" && echo same)" "same"

echo "== C: 依存を消した形 = 余分なクレートを名指しして 1 =="
run "${FIX}/removed.md" --check
check "終了コード 1" "${RC}" "1"
contains "余分な件数" "余分なクレート（依存から消えたのに一覧に残っている）: 1 件" "${OUTLOG}"
contains "余分なクレートを名指しする" "    beta 2.1.0" "${OUTLOG}"
contains "載っていないは 0 件" "載っていないクレート（依存にあるのに一覧に無い）: 0 件" "${OUTLOG}"

echo "== D: 顔ぶれが同じで本文だけ違う =="
run "${FIX}/text-changed.md" --check
check "終了コード 1" "${RC}" "1"
contains "本文だけ違うと言う" "クレートの顔ぶれは同じで、本文" "${OUTLOG}"
contains "先頭の差分を出す" "+Copyright (c) 2027 Example" "${OUTLOG}"

echo "== E: cargo-about の版違い・未導入 = 入れ方を出して 1（生成しない）=="
FAKE_ABOUT_VERSION="0.8.0" run "${FIX}/added.md" --check
check "版違いは終了コード 1" "${RC}" "1"
contains "版違いを言う" "cargo-about の版が 0.8.0 です（要るのは 0.9.2" "${OUTLOG}"
contains "入れ直しのコマンド" "cargo install cargo-about --locked --version 0.9.2" "${OUTLOG}"
absent "版違いでは生成しない" "about generate" "${FAKE_CARGO_LOG}"
FAKE_ABOUT_VERSION="" run "${FIX}/added.md" --check
check "未導入は終了コード 1" "${RC}" "1"
contains "未導入を言う" "cargo-about が見つかりません。入れ方: cargo install cargo-about --locked --version 0.9.2" "${OUTLOG}"

echo "== F: 引数なし = 作り直して書き込む =="
cp "${FIX}/base.md" "${FAKE}/THIRD-PARTY-LICENSES.md"
run "${FIX}/added-crlf.md"
check "終了コード 0" "${RC}" "0"
contains "足したクレートを名指しする" "    gamma 0.3.4" "${OUTLOG}"
check "書き込んだ一覧は LF だけ（CRLF を残さない）" "$(cmp -s "${FIX}/added.md" "${FAKE}/THIRD-PARTY-LICENSES.md" && echo same)" "same"
run "${FIX}/added-crlf.md"
contains "2 回目は一致" "は今の依存と一致しています（5 クレート" "${OUTLOG}"
rm -f "${FAKE}/THIRD-PARTY-LICENSES.md"
run "${FIX}/base.md" --check
check "一覧が無ければ --check は 1" "${RC}" "1"
contains "全クレートを載っていない側に数える" "載っていないクレート（依存にあるのに一覧に無い）: 4 件" "${OUTLOG}"

echo "== G: 生成の約束（先に fetch --locked、cargo-about は --frozen）=="
cp "${FIX}/base.md" "${FAKE}/THIRD-PARTY-LICENSES.md"
run "${FIX}/base.md" --check
check "fetch → about generate の順" "$(grep -vF -- '--version' "${FAKE_CARGO_LOG}" | awk '{ print $1 }' | tr '\n' ' ')" "fetch about "
contains "fetch は --locked" "fetch --locked" "${FAKE_CARGO_LOG}"
contains "about generate は --frozen と --fail" "about generate --frozen --fail about.hbs" "${FAKE_CARGO_LOG}"
FAKE_FETCH_RC=101 run "${FIX}/base.md" --check
check "fetch に失敗したら 1" "${RC}" "1"
contains "fetch の失敗を言う" "依存のソースを取れませんでした（cargo fetch --locked）" "${OUTLOG}"

echo "== H: CI の注釈・知らない引数 =="
GITHUB_ACTIONS_SIM=true run "${FIX}/added.md" --check
contains "CI では ::error の注釈" "::error file=THIRD-PARTY-LICENSES.md," "${OUTLOG}"
run "${FIX}/base.md" --checkk
check "知らない引数は 2" "${RC}" "2"

echo
echo "=== 結果: PASS=${PASS} FAIL=${FAIL} ==="
[[ ${FAIL} -eq 0 ]] || exit 1
