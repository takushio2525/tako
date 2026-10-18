#!/usr/bin/env bash
# tako:run: bash scripts/test-ci-change-scope-2084.sh
# test-ci-change-scope-2084.sh — docs だけの変更で Rust の重いジョブを skip する判定のテスト（#2084）
#
# 一時ディレクトリに作った git リポジトリ（merge コミット・浅い clone・push の before）の中で
# 実物の scripts/ci-change-scope.sh を走らせる。ネットワークにも本番の PR にも触らない。
#
# 固定しているもの:
#   A. パスの境界: docs/ だけ・.agent/ の md だけ → skip / docs + crates・scripts だけ・
#      生成ページ・.agent の md 以外・空 → 走らせる
#   B. CI の差分の採り方: merge コミットの第 1 親から（main 側の変更を混ぜない）・
#      名前の変更は旧パスも数える・浅い clone（fetch-depth: 2）で足りる・push の before を
#      取りに行く・取れないとき / 想定外のイベントは走らせる側
#   C. docs を読む番犬の拾い方（コメントだけの言及は拾わない・錨が拾えなければ止まる）
#   D. ci.yml の配線（docs の検査は毎回走る docs ジョブにだけある・macOS / Windows は
#      rust=true のときだけ・番犬ジョブは毎回 ubuntu で・ライセンス一覧の検査は rust=true の
#      ときだけ ubuntu で = #1851）
#   E. 本物の ci.yml から scripts/wait-pr-checks.sh が導く期待名に新しいジョブが入る
# 検出力: 判定・配線を壊した写しを作り、同じ検査が落ちることも確かめる
set -uo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
SCOPE_SH="${REPO_ROOT}/scripts/ci-change-scope.sh"
WATCHDOGS_SH="${REPO_ROOT}/scripts/ci-docs-watchdogs.sh"
CI_YML="${REPO_ROOT}/.github/workflows/ci.yml"

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

# 利用者の git 設定（署名・フック・既定ブランチ）に左右されないようにする
export GIT_CONFIG_GLOBAL="${TMP}/gitconfig"
export GIT_CONFIG_NOSYSTEM=1
git config --global user.email "probe@example.invalid"
git config --global user.name "probe"
git config --global init.defaultBranch main
git config --global commit.gpgsign false
git config --global protocol.file.allow always

# 最後の行（rust=…）だけを返す
classify() { printf '%s' "$1" | bash "${SCOPE_SH}" classify 2>/dev/null | tail -1; }

echo "== A: パスの境界 =="
assert_eq "docs/ だけ → skip" "$(classify $'docs/src/content/docs/guides/a.md\ndocs/public/og.png\n')" "rust=false"
assert_eq ".agent/ の md だけ → skip" "$(classify $'.agent/conventions.md\n.agent/plans/2026-10-x.md\n')" "rust=false"
assert_eq "docs/ + .agent/ の md → skip" "$(classify $'docs/a.md\n.agent/b.md\n')" "rust=false"
assert_eq "docs/ + crates/ → 走らせる" "$(classify $'docs/a.md\ncrates/tako-core/src/lib.rs\n')" "rust=true"
assert_eq "scripts/ だけ → 走らせる" "$(classify $'scripts/wait-pr-checks.sh\n')" "rust=true"
assert_eq "Cargo.toml → 走らせる" "$(classify $'Cargo.toml\n')" "rust=true"
assert_eq "Cargo.lock → 走らせる" "$(classify $'Cargo.lock\n')" "rust=true"
assert_eq ".github/ → 走らせる" "$(classify $'.github/workflows/ci.yml\n')" "rust=true"
assert_eq "ルートの md（AGENTS.md）→ 走らせる" "$(classify $'AGENTS.md\n')" "rust=true"
assert_eq "ルートの md（CHANGELOG.md）→ 走らせる" "$(classify $'CHANGELOG.md\n')" "rust=true"
assert_eq ".agent/ の md 以外 → 走らせる" "$(classify $'.agent/notes.txt\n')" "rust=true"
assert_eq ".agent/ と紛らわしい名前 → 走らせる" "$(classify $'.agents/a.md\n')" "rust=true"
assert_eq "docs と紛らわしい名前 → 走らせる" "$(classify $'docsx/a.md\n')" "rust=true"
assert_eq "下の階層の docs/ → 走らせる" "$(classify $'web/tako-remote/docs/a.md\n')" "rust=true"
assert_eq "対応状況のデータ（agent-support）→ 走らせる" "$(classify $'docs/src/data/agent-support.json\n')" "rust=true"
assert_eq "対応状況のデータ（windows-support）→ 走らせる" "$(classify $'docs/src/data/windows-support.json\n')" "rust=true"
assert_eq "対応状況の写しの書式と分類（--check が使う）→ 走らせる" "$(classify $'docs/scripts/support-pages.mjs\n')" "rust=true"
assert_eq "空の差分 → 走らせる" "$(classify '')" "rust=true"
assert_eq "空行だけ → 走らせる" "$(classify $'\n\n')" "rust=true"
out="$(printf 'docs/a.md\0docs/with space/日本語.md\0' | bash "${SCOPE_SH}" classify -z 2>&1)"
assert_has "NUL 区切り・空白と日本語のパス → skip" "rust=false" "${out}"
out="$(printf 'docs/a.md\ncrates/x.rs\n' | bash "${SCOPE_SH}" classify 2>&1)"
assert_has "走らせる理由に該当パスを名指しする" "crates/x.rs — docs/ と .agent/ の md 以外" "${out}"
out="$(printf 'docs/src/data/agent-support.json\n' | bash "${SCOPE_SH}" classify 2>&1)"
assert_has "対応状況のデータは理由を言う" "gen-agent-support-docs.mjs --check" "${out}"
out="$(for i in $(seq 1 25); do echo "crates/f${i}.rs"; done | bash "${SCOPE_SH}" classify 2>&1)"
assert_has "長い一覧は打ち切って残りの件数を言う" "…ほか 15 件" "${out}"
assert_hasnt "打ち切った先は並べない" "crates/f11.rs" "${out}"

echo "== B: CI の差分の採り方（本物の git） =="
# 起点のリポジトリ: main に docs / .agent / crates がある
ORIGIN="${TMP}/origin"
mkdir -p "${ORIGIN}"
(
  cd "${ORIGIN}" || exit 1
  git init -q
  mkdir -p docs .agent crates/x/src
  echo a > docs/a.md
  echo b > .agent/b.md
  echo 'fn main() {}' > crates/x/src/main.rs
  echo 'fn other() {}' > crates/x/src/other.rs
  git add -A && git commit -qm base
)

# PR を作って refs/heads/<名前> に merge コミットを置く。
# $1 = 名前 / $2 = PR 側で行う変更（cd 済みのシェルで eval）/ $3 = main 側で行う変更
make_pr_merge() {
  local name="$1" pr_change="$2" main_change="${3:-}"
  (
    cd "${ORIGIN}" || exit 1
    git checkout -q main
    git checkout -q -b "pr-${name}"
    eval "${pr_change}"
    git add -A && git commit -qm "pr ${name}"
    git checkout -q main
    if [[ -n "${main_change}" ]]; then
      git checkout -q -b "main-${name}"
      eval "${main_change}"
      git add -A && git commit -qm "main moved ${name}"
    fi
    git checkout -q --detach
    git merge -q --no-ff --no-edit "pr-${name}" >/dev/null
    git branch -q -f "merge-${name}" HEAD
    git checkout -q main
  )
}

# 浅い clone（actions/checkout の fetch-depth: 2 と同じ深さ）で github モードを走らせる。
# $1 = clone する ref / 残りは env の割り当て（CI_EVENT=… など）
run_github_in_clone() {
  local ref="$1" dir
  shift
  dir="${TMP}/clone-${ref}-$RANDOM"
  git clone -q --depth 2 --branch "${ref}" "file://${ORIGIN}" "${dir}" 2>/dev/null
  : > "${TMP}/gh_output"
  : > "${TMP}/gh_summary"
  (cd "${dir}" && env GITHUB_OUTPUT="${TMP}/gh_output" GITHUB_STEP_SUMMARY="${TMP}/gh_summary" "$@" bash "${SCOPE_SH}" github 2>&1)
}

make_pr_merge docs 'echo changed >> docs/a.md; echo new > .agent/c.md' 'echo moved >> crates/x/src/other.rs'
out="$(run_github_in_clone merge-docs CI_EVENT=pull_request)"
assert_has "docs だけの PR（main 側は crates が進んでいる）→ skip" "rust=false" "${out}"
assert_eq "GITHUB_OUTPUT へ rust=false を書く" "$(cat "${TMP}/gh_output")" "rust=false"
assert_has "step summary に結論を書く" "変更範囲の判定（#2084）: rust=false" "$(cat "${TMP}/gh_summary")"
assert_has "skip するときは notice を出す" "::notice title=docs だけの変更（#2084）::" "${out}"

make_pr_merge code 'echo changed >> docs/a.md; echo "// x" >> crates/x/src/main.rs'
out="$(run_github_in_clone merge-code CI_EVENT=pull_request)"
assert_has "docs + crates の PR → 走らせる" "rust=true" "${out}"
assert_eq "GITHUB_OUTPUT へ rust=true を書く" "$(cat "${TMP}/gh_output")" "rust=true"
assert_has "理由に crates のパスを出す" "crates/x/src/main.rs" "${out}"
assert_hasnt "走らせるときは notice を出さない" "::notice" "${out}"

# docs のパスが辞書順で前に来る組み合わせ（NUL 区切りが失われるとパスが連結され
# 「docs/a.mdscripts/z.sh」が docs/* に当たって skip と誤る）
make_pr_merge docs_then_scripts 'echo changed >> docs/a.md; mkdir -p scripts; echo "echo z" > scripts/z.sh'
out="$(run_github_in_clone merge-docs_then_scripts CI_EVENT=pull_request)"
assert_has "docs + scripts の PR（docs が辞書順で前）→ 走らせる" "rust=true" "${out}"
assert_has "scripts のパスを 1 本として名指しする" "scripts/z.sh — docs/ と .agent/ の md 以外" "${out}"
assert_has "件数は 2 件として数える" "Rust が要る変更（2 件中 1 件）" "${out}"

make_pr_merge rename 'git mv crates/x/src/other.rs docs/other.rs'
out="$(run_github_in_clone merge-rename CI_EVENT=pull_request)"
assert_has "crates → docs への移動は旧パスも数えて走らせる" "rust=true" "${out}"
assert_has "旧パスを名指しする" "crates/x/src/other.rs" "${out}"

make_pr_merge delete 'git rm -q crates/x/src/other.rs'
out="$(run_github_in_clone merge-delete CI_EVENT=pull_request)"
assert_has "crates の削除だけでも走らせる" "rust=true" "${out}"

out="$(run_github_in_clone main CI_EVENT=pull_request)"
assert_has "HEAD が merge コミットでなければ走らせる" "rust=true" "${out}"
assert_has "その理由を言う" "HEAD が merge コミットではない" "${out}"

# push: main に docs だけのコミットを 2 本積む（浅い clone には直前の 1 本しか無い）
(
  cd "${ORIGIN}" || exit 1
  git checkout -q main
  echo p1 >> docs/a.md && git commit -qam "push 1"
  echo p2 >> .agent/b.md && git commit -qam "push 2"
)
before1="$(git -C "${ORIGIN}" rev-parse main^1)"
before2="$(git -C "${ORIGIN}" rev-parse main~2)"
out="$(run_github_in_clone main CI_EVENT=push CI_PUSH_BEFORE="${before1}")"
assert_has "docs だけの push（before が手元にある）→ skip" "rust=false" "${out}"
out="$(run_github_in_clone main CI_EVENT=push CI_PUSH_BEFORE="${before2}")"
assert_has "before が浅い clone に無くても取りに行って判定する → skip" "rust=false" "${out}"
(
  cd "${ORIGIN}" || exit 1
  echo '// y' >> crates/x/src/main.rs && git commit -qam "push 3 code"
  echo p4 >> docs/a.md && git commit -qam "push 4 docs"
)
before3="$(git -C "${ORIGIN}" rev-parse main~2)"
out="$(run_github_in_clone main CI_EVENT=push CI_PUSH_BEFORE="${before3}")"
assert_has "複数コミットの push は全体の差分で見る（途中に crates）→ 走らせる" "rust=true" "${out}"
out="$(run_github_in_clone main CI_EVENT=push CI_PUSH_BEFORE=0000000000000000000000000000000000000000)"
assert_has "before が 0（新規ブランチ）→ 走らせる" "rust=true" "${out}"
out="$(run_github_in_clone main CI_EVENT=push CI_PUSH_BEFORE=1234567890abcdef1234567890abcdef12345678)"
assert_has "before が取れない → 走らせる" "rust=true" "${out}"
assert_has "取れない理由を言う" "を取得できない" "${out}"
out="$(run_github_in_clone main CI_EVENT=push CI_PUSH_BEFORE='')"
assert_has "before が空 → 走らせる" "rust=true" "${out}"
out="$(run_github_in_clone main CI_EVENT=workflow_dispatch)"
assert_has "想定外のイベント → 走らせる" "rust=true" "${out}"
assert_eq "想定外のイベントでも GITHUB_OUTPUT へ書く" "$(cat "${TMP}/gh_output")" "rust=true"

echo "== B 検出力: --no-renames を外した写しは移動を docs だけと誤る =="
sed 's/ --no-renames//' "${SCOPE_SH}" > "${TMP}/scope-renames.sh"
assert_eq "写しから --no-renames が消えている" "$(grep -c -- '--no-renames' "${TMP}/scope-renames.sh" | tr -d ' ')" "0"
dir="${TMP}/clone-inject-rename"
git clone -q --depth 2 --branch merge-rename "file://${ORIGIN}" "${dir}" 2>/dev/null
out="$(cd "${dir}" && CI_EVENT=pull_request bash "${TMP}/scope-renames.sh" github 2>&1)"
assert_has "（注入）旧パスを数えないと skip と誤る = 上の検査がこれを捕まえる" "rust=false" "${out}"

echo "== C: docs を読む番犬の拾い方 =="
list="$(bash "${WATCHDOGS_SH}" --list)"
rc=$?
assert_eq "本物のリポジトリで --list が 0" "${rc}" "0"
for t in no_personal_data context_budget docs_tool_inventory docs_keyboard_shortcuts \
  capability_claim_watchdog fr_number_watchdog virtual_display_watchdog shell_scripts \
  issue1645_progress_revival_watchdog issue1652_editor_keys_watchdog; do
  assert_has "拾う: ${t}" "$(printf 'tako-control\t%s\t' "${t}")" "${list}"
done
assert_hasnt "コメントでしか .agent を言わない agent_parity は拾わない" "agent_parity" "${list}"
n="$(grep -c . <<<"${list}" | tr -d ' ')"
if [[ "${n}" -ge 20 ]]; then ok "本物のリポジトリで 20 本以上拾う（${n} 本）"; else ng "本物のリポジトリで 20 本以上拾う" "${n} 本"; fi

FAKE="${TMP}/fake-repo"
mkdir -p "${FAKE}/crates/pkgdir/tests"
printf '[package]\nname = "fake-pkg"\nversion = "0.1.0"\n\n[dependencies]\nname = "not-this"\n' > "${FAKE}/crates/pkgdir/Cargo.toml"
T="${FAKE}/crates/pkgdir/tests"
printf 'fn f() { let _ = std::fs::read_to_string("docs/a.md"); }\n' > "${T}/no_personal_data.rs"
printf 'const P: &str = ".agent/progress.md";\n' > "${T}/context_budget.rs"
printf 'const P: &str = "docs/src/content/docs/x.md";\n' > "${T}/docs_tool_inventory.rs"
printf '// 書き方は .agent/conventions.md と "docs/x.md" を見る\n/// "docs/y.md"\nfn f() {}\n' > "${T}/comment_only.rs"
printf 'fn f(r: &std::path::Path) { let _ = r.join("docs"); }\n' > "${T}/join_docs.rs"
printf 'const S: &str = include_str!("../../../.agent/requirements.md");\n' > "${T}/include_agent.rs"
printf 'fn w(d: &std::path::Path) { for e in std::fs::read_dir(d).unwrap() { let _ = e; } let _ = "node_modules"; }\n' > "${T}/walker.rs"
printf 'fn w() { let _ = "target"; }\n' > "${T}/target_without_walk.rs"
printf 'fn g() { let _ = ["ls-files", "-z"]; }\n' > "${T}/git_list.rs"
list="$(TAKO_DOCS_WATCHDOGS_ROOT="${FAKE}" bash "${WATCHDOGS_SH}" --list)"
assert_has "パッケージ名は [package] の name から" "$(printf 'fake-pkg\tno_personal_data\tdocs')" "${list}"
assert_has "join(\"docs\") を拾う" "$(printf '\tjoin_docs\tdocs')" "${list}"
assert_has "include_str! の ../.agent を拾う" "$(printf '\tinclude_agent\tdocs')" "${list}"
assert_has "target / node_modules を除外して歩くものを拾う" "$(printf '\twalker\twalker')" "${list}"
assert_has "ls-files を拾う" "$(printf '\tgit_list\tgit')" "${list}"
assert_hasnt "コメントだけの言及は拾わない" "comment_only" "${list}"
assert_hasnt "歩かない \"target\" は拾わない" "target_without_walk" "${list}"

# cargo を偽物にして、組み立てる引数を見る（本物の cargo は起こさない）
mkdir -p "${TMP}/bin"
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$*" > "%s/cargo-args"\n' "${TMP}" > "${TMP}/bin/cargo"
chmod +x "${TMP}/bin/cargo"
out="$(PATH="${TMP}/bin:${PATH}" TAKO_DOCS_WATCHDOGS_ROOT="${FAKE}" bash "${WATCHDOGS_SH}" -- --test-threads=4 2>&1)"
args="$(cat "${TMP}/cargo-args" 2>/dev/null)"
assert_has "full の macOS と同じ --workspace で組む" "test --workspace --test " "${args}"
assert_has "拾ったテストを --test で渡す" "--test no_personal_data" "${args}"
assert_has "-- の後ろをテストバイナリへ渡す" "-- --test-threads=4" "${args}"
assert_hasnt "拾わなかったものは渡さない" "comment_only" "${args}"
assert_has "走らせる本数を言う" "docs / .agent を読む番犬 7 本を走らせる" "${out}"

rm "${T:?}/context_budget.rs"
out="$(TAKO_DOCS_WATCHDOGS_ROOT="${FAKE}" bash "${WATCHDOGS_SH}" --list 2>&1)"
rc=$?
assert_eq "錨が拾えなければ 1（黙って通さない）" "${rc}" "1"
assert_has "拾えなかった錨を名指しする" "context_budget" "${out}"

echo "== D: ci.yml の配線 =="
# $1 = ci.yml / $2 = job id → その job の本文（コメント行を除く）
job_body() {
  awk -v want="$2" '
    /^jobs:[ \t]*$/ { in_jobs = 1; next }
    in_jobs && /^[^ \t#]/ { in_jobs = 0 }
    in_jobs && /^  [A-Za-z0-9_-]+:[ \t]*$/ { cur = $0; sub(/^  /, "", cur); sub(/:.*/, "", cur); next }
    in_jobs && cur == want && $0 !~ /^[ \t]*#/ { print }
  ' "$1"
}
job_ids() {
  awk '
    /^jobs:[ \t]*$/ { in_jobs = 1; next }
    in_jobs && /^[^ \t#]/ { in_jobs = 0 }
    in_jobs && /^  [A-Za-z0-9_-]+:[ \t]*$/ { id = $0; sub(/^  /, "", id); sub(/:.*/, "", id); print id }
  ' "$1"
}
# 配線の検査。問題を 1 行 1 件で出す（何も出なければ合格）
wiring_problems() {
  local yml="$1" body id step
  for id in macos windows licenses; do
    body="$(job_body "${yml}" "${id}")"
    grep -qE '^    needs: changes[ \t]*$' <<<"${body}" || echo "${id}: needs: changes が無い"
    grep -qF "if: needs.changes.outputs.rust == 'true'" <<<"${body}" || echo "${id}: rust=true のときだけ走る if が無い"
  done
  # ライセンス一覧の検査は ubuntu で 1 回（cargo-about は about.toml の targets で両 OS ぶんを出す。
  # macOS / Windows の列に足すと最長の列が伸びる）。#1851
  body="$(job_body "${yml}" licenses)"
  grep -qE '^    runs-on: ubuntu-latest[ \t]*$' <<<"${body}" || echo "licenses: ubuntu で走らない（macOS / Windows の列に並ぶ）"
  grep -qF 'bash scripts/third-party-licenses.sh --check' <<<"${body}" || echo "licenses: third-party-licenses.sh --check を呼んでいない"
  # docs を読む番犬は毎回・ubuntu（macOS の列に並ばない / Linux で組めることをコードの PR で守る）
  body="$(job_body "${yml}" docs-watchdogs)"
  grep -qE '^    (if|needs):' <<<"${body}" && echo "docs-watchdogs: 判定で止まる（if / needs がある）"
  grep -qE '^    runs-on: ubuntu-latest[ \t]*$' <<<"${body}" || echo "docs-watchdogs: ubuntu で走らない（macOS の列に並ぶ）"
  grep -qF 'bash scripts/ci-docs-watchdogs.sh' <<<"${body}" || echo "docs-watchdogs: ci-docs-watchdogs.sh を呼んでいない"
  body="$(job_body "${yml}" changes)"
  grep -qF 'rust: ${{ steps.scope.outputs.rust }}' <<<"${body}" || echo "changes: outputs.rust が無い"
  grep -qF 'bash scripts/ci-change-scope.sh github' <<<"${body}" || echo "changes: 判定を呼んでいない"
  grep -qE 'fetch-depth: 2' <<<"${body}" || echo "changes: fetch-depth: 2 が無い（merge コミットの親が取れない）"
  # docs の検査は「毎回走る docs ジョブ」にだけある
  body="$(job_body "${yml}" docs)"
  grep -qE '^    (if|needs):' <<<"${body}" && echo "docs: 判定で止まる（if / needs がある）"
  for step in 'npm run build' 'npm run og:verify' 'node scripts/verify-links.mjs' 'node scripts/verify-seo.mjs' \
    'node scripts/test-middleware.mjs' 'node scripts/verify-headers.mjs' 'bash scripts/check-docs-issue-refs.sh' \
    'node docs/scripts/check-releases-page.mjs'; do
    grep -qF "${step}" <<<"${body}" || echo "docs: ${step} が無い"
  done
  while IFS= read -r id; do
    [[ "${id}" == "docs" ]] && continue
    body="$(job_body "${yml}" "${id}")"
    if grep -qE 'working-directory: docs[ \t]*$|docs/scripts/|check-docs-issue-refs' <<<"${body}"; then
      echo "${id}: docs の検査が docs ジョブの外にある（docs だけの PR で止まりうる）"
    fi
  done < <(job_ids "${yml}")
}
probs="$(wiring_problems "${CI_YML}")"
assert_eq "本物の ci.yml の配線に問題が無い" "${probs}" ""
assert_eq "job は changes / docs / docs-watchdogs / macos / windows / licenses" "$(job_ids "${CI_YML}" | tr '\n' ' ')" "changes docs docs-watchdogs macos windows licenses "

echo "== D 検出力: 壊した写しを配線の検査が名指しする =="
# macOS ジョブの if を消す
grep -v "^    if: needs.changes.outputs.rust == 'true'" "${CI_YML}" > "${TMP}/ci-noif.yml"
probs="$(wiring_problems "${TMP}/ci-noif.yml")"
assert_has "（注入）macOS の if が無いことを名指しする" "macos: rust=true のときだけ走る if が無い" "${probs}"
# docs の検査を macOS ジョブへ戻す（docs ジョブから消して macOS の steps 先頭へ足す）
awk '
  /^  macos:/ { in_mac = 1 }
  /^  windows:/ { in_mac = 0 }
  { print }
  in_mac && /^    steps:/ { print "      - name: docs の内部リンクを検査"; print "        working-directory: docs"; print "        run: node scripts/verify-links.mjs" }
' "${CI_YML}" | awk '!(/node scripts\/verify-links.mjs/ && seen++ == 0)' > "${TMP}/ci-moved.yml"
probs="$(wiring_problems "${TMP}/ci-moved.yml")"
assert_has "（注入）docs の検査が macOS へ戻ったことを名指しする" "macos: docs の検査が docs ジョブの外にある" "${probs}"
assert_has "（注入）docs ジョブから抜けたことも名指しする" "docs: node scripts/verify-links.mjs が無い" "${probs}"
# docs ジョブに if を付ける
sed 's/^    name: docs（ビルド + 検査）$/&\
    if: needs.changes.outputs.rust == '"'"'true'"'"'/' "${CI_YML}" > "${TMP}/ci-docsif.yml"
probs="$(wiring_problems "${TMP}/ci-docsif.yml")"
assert_has "（注入）docs ジョブが判定で止まることを名指しする" "docs: 判定で止まる" "${probs}"

# 番犬ジョブを macOS の docs だけのときへ戻す（#2089 の検証で 42 分 queued になった形）
awk '
  /^  docs-watchdogs:/ { in_dw = 1 }
  /^  macos:/ { in_dw = 0 }
  in_dw && /^    runs-on: ubuntu-latest/ { print "    needs: changes"; print "    if: needs.changes.outputs.rust == '"'"'false'"'"'"; print "    runs-on: macos-latest"; next }
  { print }
' "${CI_YML}" > "${TMP}/ci-dw-macos.yml"
probs="$(wiring_problems "${TMP}/ci-dw-macos.yml")"
assert_has "（注入）番犬ジョブが判定で止まることを名指しする" "docs-watchdogs: 判定で止まる" "${probs}"
assert_has "（注入）番犬ジョブが macOS の列に並ぶことを名指しする" "docs-watchdogs: ubuntu で走らない" "${probs}"

# ライセンス一覧の検査を外す（#1851 の前の形）
grep -vF 'bash scripts/third-party-licenses.sh --check' "${CI_YML}" > "${TMP}/ci-nolicenses.yml"
probs="$(wiring_problems "${TMP}/ci-nolicenses.yml")"
assert_has "（注入）ライセンス一覧の検査が無いことを名指しする" "licenses: third-party-licenses.sh --check を呼んでいない" "${probs}"

echo "== E: 本物の ci.yml から待ちスクリプトが導く期待名 =="
mkdir -p "${TMP}/ghbin"
printf '#!/usr/bin/env bash\necho "no checks reported on the '"'"'x'"'"' branch" >&2\nexit 1\n' > "${TMP}/ghbin/gh"
chmod +x "${TMP}/ghbin/gh"
out="$(PATH="${TMP}/ghbin:${PATH}" bash "${REPO_ROOT}/scripts/wait-pr-checks.sh" 2084 --timeout 0 --interval 1 2>&1)"
for name in "変更範囲の判定（#2084）" "docs（ビルド + 検査）" "docs / .agent を読む Rust の番犬（ubuntu。#2084）" \
  "macOS（build + lint + test）" "Windows（build + test スモーク）" "ライセンス一覧が依存と一致（#1851）" \
  "Cloudflare Pages"; do
  assert_has "期待名: ${name}" "期待: ${name}" "${out}"
done
assert_eq "期待は 7 本" "$(grep -c '^  期待: ' <<<"${out}" | tr -d ' ')" "7"

echo
echo "=== 結果: PASS=${PASS} FAIL=${FAIL} ==="
[[ ${FAIL} -eq 0 ]] || exit 1
