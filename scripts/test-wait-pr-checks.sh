#!/usr/bin/env bash
# tako:run: bash scripts/test-wait-pr-checks.sh
# test-wait-pr-checks.sh — wait-pr-checks.sh / merge-pr.sh のモックテスト（#1333）
#
# PATH の先頭に偽 gh を挿し、一時ディレクトリに作った偽リポジトリ（.github/workflows つき）の
# 中で実物のスクリプトを走らせる。**本番の PR には一切触らない**（gh の呼び出しは全部この
# 偽物へ向き、merge が呼ばれたかどうかもファイルに記録するだけ）。
#
# 固定しているのは 5 点（#1333 の受け入れ条件）:
#   1. checks が 1 本しか登録されていない時点で「完了」と返さない（= #1313 の早すぎる merge）
#   2. 3 本 SUCCESS で 0
#   3. 1 本 FAILURE で 1
#   4. タイムアウトで 2
#   5. 揺れ（1 回 completed → 次 in_progress）で確定しない
# 併せて期待名の導出（pull_request で起動する job のみ / name 無しは job id）と、
# merge-pr.sh が「揃っていないときに merge を呼ばない」ことも見る。
set -uo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
WAIT_SH="${REPO_ROOT}/scripts/wait-pr-checks.sh"
MERGE_SH="${REPO_ROOT}/scripts/merge-pr.sh"

PASS=0
FAIL=0
ok() {
  echo "  PASS: $1"
  PASS=$((PASS + 1))
}
ng() {
  echo "  FAIL: $1 ($2)"
  FAIL=$((FAIL + 1))
}
assert_eq() { if [[ "$2" == "$3" ]]; then ok "$1"; else ng "$1" "期待 '$3' / 実際 '$2'"; fi; }
assert_has() { if grep -qF -- "$2" <<<"$3"; then ok "$1"; else ng "$1" "出力に無い: $2"; fi; }
assert_hasnt() { if grep -qF -- "$2" <<<"$3"; then ng "$1" "出力にあってはいけない: $2"; else ok "$1"; fi; }

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT
mkdir -p "${TMP}/bin"

# ---------------------------------------------------------------- 偽 gh
# checks.<n>.json を呼び出し回数の順に返す（足りなくなったら最後のものを返し続ける）。
# 中身が "ERR <終了コード> <本文>" で始まるファイルは gh のエラーを模す。
cat > "${TMP}/bin/gh" <<'GH_EOF'
#!/usr/bin/env bash
set -uo pipefail
M="${TAKO_GH_MOCK_DIR}"
echo "$*" >> "${M}/calls.log"
case "$1 ${2:-}" in
  "pr checks")
    n=$(( $(cat "${M}/checks.count" 2>/dev/null || echo 0) + 1 ))
    echo "${n}" > "${M}/checks.count"
    f="${M}/checks.${n}.json"
    if [[ ! -f "${f}" ]]; then
      f="$(ls "${M}"/checks.*.json 2>/dev/null | sort -t. -k2 -n | tail -1)"
    fi
    [[ -n "${f}" && -f "${f}" ]] || { echo "mock: 応答ファイルが無い" >&2; exit 1; }
    if head -1 "${f}" | grep -q '^ERR '; then
      code="$(head -1 "${f}" | awk '{print $2}')"
      sed -e '1s/^ERR [0-9]* //' "${f}" >&2
      exit "${code}"
    fi
    cat "${f}"
    if jq -e 'any(.[]; .bucket == "pending")' "${f}" >/dev/null; then exit 8; fi
    if jq -e 'any(.[]; .bucket == "fail" or .bucket == "cancel")' "${f}" >/dev/null; then exit 1; fi
    exit 0
    ;;
  "pr view")
    n=$(( $(cat "${M}/view.count" 2>/dev/null || echo 0) + 1 ))
    echo "${n}" > "${M}/view.count"
    f="${M}/view.${n}.json"
    [[ -f "${f}" ]] || f="${M}/view.json"
    [[ -f "${f}" ]] || { echo "mock: view の応答ファイルが無い" >&2; exit 1; }
    # merge 済みなら本物と同じく MERGED を返す（merge-pr.sh は状態で成否を決める）
    if [[ -f "${M}/merged" ]]; then jq '.state = "MERGED"' "${f}"; else cat "${f}"; fi
    exit 0
    ;;
  "pr merge")
    echo "$*" >> "${M}/merged"
    echo "Merged pull request via mock"
    exit "$(cat "${M}/merge.rc" 2>/dev/null || echo 0)"
    ;;
esac
echo "mock: 未対応の呼び出し: $*" >&2
exit 1
GH_EOF
chmod +x "${TMP}/bin/gh"
export PATH="${TMP}/bin:${PATH}"

# ---------------------------------------------------------------- 偽リポジトリと応答の道具

# $1 = リポジトリのパス, $2 = 追加の job（空可）
make_repo() {
  local root="$1" extra="${2:-}"
  mkdir -p "${root}/.github/workflows"
  {
    echo "name: CI"
    echo "on:"
    echo "  push:"
    echo "    branches: [main]"
    echo "  pull_request:"
    echo ""
    echo "jobs:"
    echo "  macos:"
    echo "    name: macOS（build + lint + test）"
    echo "    runs-on: macos-latest"
    echo "    steps:"
    echo "      - name: テスト"
    echo "        run: cargo test"
    echo "  windows:"
    echo "    name: Windows（build + test スモーク）"
    echo "    runs-on: windows-latest"
    echo "    steps:"
    echo "      - name: ビルド"
    echo "        run: cargo build"
    [[ -n "${extra}" ]] && printf '%s\n' "${extra}"
  } > "${root}/.github/workflows/ci.yml"
  # タグでしか起動しないワークフローは期待名に入ってはいけない
  {
    echo "name: Windows リリース配布物"
    echo "on:"
    echo "  push:"
    echo "    tags: ['v*']"
    echo "  workflow_dispatch:"
    echo ""
    echo "jobs:"
    echo "  build:"
    echo "    name: Windows 配布物のビルドと添付"
    echo "    runs-on: windows-latest"
    echo "    steps:"
    echo "      - run: echo hi"
  } > "${root}/.github/workflows/release-windows.yml"
  git -C "${root}" init -q 2>/dev/null || git -C "${root}" init >/dev/null
}

# 応答 JSON を組み立てる。引数は "名前|bucket|完了時刻"（bucket=pending は完了時刻を捨てる）
checks_json() {
  local first=1 spec name bucket at state
  printf '['
  for spec in "$@"; do
    IFS='|' read -r name bucket at <<<"${spec}"
    case "${bucket}" in
      pending)
        state="IN_PROGRESS"
        at="0001-01-01T00:00:00Z"
        ;;
      fail) state="FAILURE" ;;
      cancel) state="CANCELLED" ;;
      skipping) state="SKIPPED" ;;
      *) state="SUCCESS" ;;
    esac
    [[ ${first} -eq 1 ]] || printf ','
    first=0
    printf '{"name":"%s","bucket":"%s","state":"%s","completedAt":"%s"}' "${name}" "${bucket}" "${state}" "${at}"
  done
  printf ']\n'
}

CF="Cloudflare Pages"
MAC="macOS（build + lint + test）"
WIN="Windows（build + test スモーク）"

# 新しいモック環境を用意して TAKO_GH_MOCK_DIR を設定する
new_case() {
  CASE_DIR="${TMP}/$1"
  mkdir -p "${CASE_DIR}/mock"
  make_repo "${CASE_DIR}/repo" "${2:-}"
  export TAKO_GH_MOCK_DIR="${CASE_DIR}/mock"
}

run_wait() { (cd "${CASE_DIR}/repo" && bash "${WAIT_SH}" "$@" 2>&1); }
run_merge() { (cd "${CASE_DIR}/repo" && bash "${MERGE_SH}" "$@" 2>&1); }
calls_of() {
  local n
  n="$(grep -c "$1" "${TAKO_GH_MOCK_DIR}/calls.log" 2>/dev/null)" || n=0
  [[ -n "${n}" ]] || n=0
  printf '%s' "${n}"
}

echo "== Test 1: 期待するチェック名をワークフローから導く =="
new_case t1
printf 'ERR 1 no checks reported on the %s branch\n' "'improve/x'" > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1234 --timeout 0 --interval 1)"
rc=$?
assert_eq "チェックが 0 本のまま（run 未起動）はタイムアウト 2" "${rc}" "2"
assert_has "期待に macOS の job 名が入る" "期待: ${MAC}" "${out}"
assert_has "期待に Windows の job 名が入る" "期待: ${WIN}" "${out}"
assert_has "期待に外部連携の Cloudflare Pages が入る" "期待: ${CF}" "${out}"
assert_hasnt "タグでしか起動しない job は期待に入らない" "Windows 配布物のビルドと添付" "${out}"
assert_eq "期待は 3 本" "$(grep -c '^  期待: ' <<<"${out}")" "3"

new_case t1b "  extra:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi"
printf 'ERR 1 no checks reported\n' > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1234 --timeout 0 --interval 1)"
assert_has "name: が無い job は job id が期待名になる" "期待: extra" "${out}"

echo "== Test 2: Cloudflare Pages しか登録されていない時点で完了と返さない（#1313 の再発防止）=="
new_case t2
checks_json "${CF}|pass|2026-09-11T04:24:00Z" > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1328 --timeout 2 --interval 1)"
rc=$?
assert_eq "1 本だけ登録 = 完了ではない（タイムアウト 2）" "${rc}" "2"
assert_has "未登録の名前を出す（macOS）" "${MAC}" "${out}"
assert_has "未登録の名前を出す（Windows）" "${WIN}" "${out}"
assert_has "merge してはいけないと言う" "merge してはいけない" "${out}"
assert_hasnt "緑で揃ったとは言わない" "CI が全部緑で揃った" "${out}"

echo "== Test 3: 3 本 SUCCESS で 0 =="
new_case t3
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1328 --timeout 20 --interval 1)"
rc=$?
assert_eq "全部緑なら 0" "${rc}" "0"
assert_has "結論と完了時刻を出す（macOS）" "[pass] ${MAC} — 完了 2026-09-11T04:36:50Z" "${out}"
assert_has "結論と完了時刻を出す（Windows）" "[pass] ${WIN} — 完了 2026-09-11T04:39:33Z" "${out}"
assert_has "揃ったことを言う" "CI が全部緑で揃った（期待 3 本 / 報告 3 本）" "${out}"
assert_eq "確定には 2 回連続の観測が要る" "$(calls_of 'pr checks')" "2"

echo "== Test 4: 1 本 FAILURE で 1 =="
new_case t4
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|fail|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1328 --timeout 20 --interval 1)"
rc=$?
assert_eq "失敗があれば 1" "${rc}" "1"
assert_has "失敗の本数を出す" "失敗しているチェックが 1 本ある" "${out}"

echo "== Test 5: 実行中のままならタイムアウト 2 =="
new_case t5
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pending|" "${WIN}|pending|" > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1328 --timeout 2 --interval 1)"
rc=$?
assert_eq "実行中が残ればタイムアウト 2" "${rc}" "2"
assert_has "実行中の本数を出す" "実行中: 2 本" "${out}"

echo "== Test 6: 揺れ（1 回 completed → 次 in_progress）では確定しない =="
new_case t6
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pending|" \
  > "${TAKO_GH_MOCK_DIR}/checks.2.json"
out="$(run_wait 1328 --timeout 3 --interval 1)"
rc=$?
assert_eq "1 回 completed を見ただけでは確定しない（タイムアウト 2）" "${rc}" "2"
assert_has "揺れを 1 行で出す" "揺れ: 完了の観測を取り消す" "${out}"
assert_hasnt "緑で揃ったとは言わない" "CI が全部緑で揃った" "${out}"

new_case t6b
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pending|" \
  > "${TAKO_GH_MOCK_DIR}/checks.2.json"
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:48:24Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.3.json"
out="$(run_wait 1328 --timeout 20 --interval 1)"
rc=$?
assert_eq "揺れが収まれば 0" "${rc}" "0"
assert_has "揺れを記録している" "揺れ: 完了の観測を取り消す" "${out}"
assert_eq "揺れの後は観測を取り直す（4 回）" "$(calls_of 'pr checks')" "4"

echo "== Test 7: 引数と gh のエラーは 3 =="
new_case t7
printf 'ERR 4 gh: To use GitHub CLI in a GitHub Actions workflow, set the GH_TOKEN environment variable.\n' \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(run_wait 1328 --timeout 5 --interval 1)"
assert_eq "gh 未ログイン（認証エラー）は 3" "$?" "3"
assert_has "gh の言い分をそのまま出す" "GH_TOKEN" "${out}"
out="$(run_wait)"
assert_eq "PR 番号なしは 3" "$?" "3"
out="$(run_wait abc)"
assert_eq "PR 番号が数値でなければ 3" "$?" "3"
out="$(run_wait 1328 --interval 0)"
assert_eq "間隔 0 は 3" "$?" "3"
out="$(run_wait 1328 --unknown)"
assert_eq "不明なオプションは 3" "$?" "3"

echo "== Test 8: merge-pr.sh は CI が緑で揃ったときだけ merge する =="
new_case t8
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
cat > "${TAKO_GH_MOCK_DIR}/view.json" <<'JSON'
{"number":1334,"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN",
 "headRefName":"improve/1333-wait-pr-checks","title":"[改善] テスト用","url":"https://example.invalid/pr/1334"}
JSON
out="$(run_merge 1334 --timeout 20 --interval 1)"
rc=$?
assert_eq "緑で揃えば 0" "${rc}" "0"
assert_eq "squash + ブランチ削除で merge する" "$(cat "${TAKO_GH_MOCK_DIR}/merged" 2>/dev/null)" "pr merge 1334 --squash --delete-branch"
assert_has "merge 後に PR の状態を見て成否を決める" "PR #1334 は MERGED" "${out}"

echo "== Test 9: CONFLICTING なら CI を待たずに merge しない =="
new_case t9
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
cat > "${TAKO_GH_MOCK_DIR}/view.json" <<'JSON'
{"number":1334,"state":"OPEN","isDraft":false,"mergeable":"CONFLICTING","mergeStateStatus":"DIRTY",
 "headRefName":"improve/1333-wait-pr-checks","title":"[改善] テスト用","url":"https://example.invalid/pr/1334"}
JSON
out="$(run_merge 1334 --timeout 20 --interval 1)"
rc=$?
assert_eq "衝突していれば 1" "${rc}" "1"
assert_has "理由を出す" "main と衝突している" "${out}"
assert_eq "merge を呼ばない" "$(cat "${TAKO_GH_MOCK_DIR}/merged" 2>/dev/null)" ""
assert_eq "CI も待たない" "$(calls_of 'pr checks')" "0"

echo "== Test 10: BEHIND なら merge しない =="
new_case t10
cat > "${TAKO_GH_MOCK_DIR}/view.json" <<'JSON'
{"number":1334,"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"BEHIND",
 "headRefName":"improve/1333-wait-pr-checks","title":"[改善] テスト用","url":"https://example.invalid/pr/1334"}
JSON
out="$(run_merge 1334 --timeout 20 --interval 1)"
rc=$?
assert_eq "main より古ければ 1" "${rc}" "1"
assert_has "取り込み方を案内する" "git merge origin/main" "${out}"
assert_eq "merge を呼ばない" "$(cat "${TAKO_GH_MOCK_DIR}/merged" 2>/dev/null)" ""

echo "== Test 11: CI が 1 本だけの時点では merge しない（#1313 そのもの）=="
new_case t11
checks_json "${CF}|pass|2026-09-11T04:24:00Z" > "${TAKO_GH_MOCK_DIR}/checks.1.json"
cat > "${TAKO_GH_MOCK_DIR}/view.json" <<'JSON'
{"number":1328,"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN",
 "headRefName":"fix/1313-config-io-tmp","title":"[修正] テスト用","url":"https://example.invalid/pr/1328"}
JSON
out="$(run_merge 1328 --timeout 2 --interval 1)"
rc=$?
assert_eq "揃わなければタイムアウトの 2 をそのまま返す" "${rc}" "2"
assert_eq "merge を呼ばない" "$(cat "${TAKO_GH_MOCK_DIR}/merged" 2>/dev/null)" ""
assert_has "merge しない理由を出す" "merge しない: CI が揃っていない" "${out}"

echo "== Test 12: CI が赤なら merge しない =="
new_case t12
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|fail|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
cat > "${TAKO_GH_MOCK_DIR}/view.json" <<'JSON'
{"number":1334,"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN",
 "headRefName":"improve/1333-wait-pr-checks","title":"[改善] テスト用","url":"https://example.invalid/pr/1334"}
JSON
out="$(run_merge 1334 --timeout 20 --interval 1)"
rc=$?
assert_eq "赤なら 1" "${rc}" "1"
assert_eq "merge を呼ばない" "$(cat "${TAKO_GH_MOCK_DIR}/merged" 2>/dev/null)" ""

echo "== Test 13: 修正前の判定（TAKO_1333_LEGACY=1）では 1 本だけで完了と返る（検出力）=="
new_case t13
checks_json "${CF}|pass|2026-09-11T04:24:00Z" > "${TAKO_GH_MOCK_DIR}/checks.1.json"
out="$(TAKO_1333_LEGACY=1 run_wait 1328 --timeout 20 --interval 1)"
rc=$?
assert_eq "旧判定は Cloudflare Pages 1 本で 0 を返す（= #1313 の穴）" "${rc}" "0"
assert_eq "旧判定は 1 回の観測で確定する" "$(calls_of 'pr checks')" "1"

echo "== Test 14: 修正前の判定では揺れの 1 回目で確定してしまう（検出力）=="
new_case t14
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pass|2026-09-11T04:39:33Z" \
  > "${TAKO_GH_MOCK_DIR}/checks.1.json"
checks_json "${CF}|pass|2026-09-11T04:24:00Z" "${MAC}|pass|2026-09-11T04:36:50Z" "${WIN}|pending|" \
  > "${TAKO_GH_MOCK_DIR}/checks.2.json"
out="$(TAKO_1333_LEGACY=1 run_wait 1328 --timeout 20 --interval 1)"
rc=$?
assert_eq "旧判定は揺れを見ずに 0 を返す" "${rc}" "0"
assert_eq "旧判定は 1 回の観測で確定する" "$(calls_of 'pr checks')" "1"

echo
echo "=== 結果: PASS=${PASS} FAIL=${FAIL} ==="
[[ ${FAIL} -eq 0 ]] || exit 1
