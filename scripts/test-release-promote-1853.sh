#!/usr/bin/env bash
# test-release-promote-1853.sh — release.sh --promote（安定版への昇格）のモックテスト（#403 / #1853 / #1592）
#
# 一時ディレクトリに「作業リポ + origin（bare）+ Homebrew tap（bare）」を作り、実物の
# release.sh --promote を /bin/bash（3.2）で走らせる。本番には一切触らない:
#   - gh はスタブ（Release の状態は一時ディレクトリの rel/ に持つ。PR も作らない）
#   - scripts/merge-pr.sh もスタブ（CI を待たず、origin の main を docs のブランチへ早送りする）
#   - push 先は一時ディレクトリの bare リポ（tap の clone 元は TAKO_HOMEBREW_TAP_REMOTE で差し替える）
#   - git のグローバル / システム設定を切り離す（CI の素のランナーと同じ「作者が未設定」の状態で走る）
#
# ここが落ちるということは、安定版への昇格が次のどれかに戻ったという意味:
#   - 夜間の素のタグ（v0.8.26 の形）を昇格できない（#1853 の発端）
#   - 昇格したのに Homebrew cask が古い版のまま（Homebrew の利用者に更新が届かない = #1853）
#   - 昇格したのに docs の「最新の安定版」が古い版のまま（#1592 / #1575）
#   - 後続の失敗が終了コードに出ない（気づかないまま昇格が「済んだ」扱いになる）
#   - 途中で死んだ昇格が exit 0 に見える（bash 3.2 の set -u + EXIT trap。Test 7）
set -euo pipefail

cd "$(dirname "$0")/.."
SRC_ROOT=$PWD
PASS=0
FAIL=0

# release.sh は #837 の後始末で Launch Services を触りうる。本番の LS には触らない
export TAKO_LSREGISTER=/nonexistent/lsregister

assert_eq() {
  local desc="$1" expected="$2" actual="$3"
  if [[ "$expected" = "$actual" ]]; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (expected=$expected, actual=$actual)"
    FAIL=$((FAIL + 1))
  fi
}

assert_contains() {
  local desc="$1" haystack="$2" needle="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (not found: '$needle')"
    FAIL=$((FAIL + 1))
  fi
}

assert_not_contains() {
  local desc="$1" haystack="$2" needle="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    echo "  FAIL: $desc (found: '$needle')"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  fi
}

sha_of() {
  /usr/bin/shasum -a 256 "$1" | awk '{print $1}'
}

# 一時ディレクトリの git は、グローバル / システム設定を読まない（作者・別名・資格情報の
# ヘルパに引きずられない。CI の素のランナーと同じ条件にもなる）
git_isolated() {
  GIT_CONFIG_GLOBAL="$D/gitconfig" GIT_CONFIG_NOSYSTEM=1 git "$@"
}
setup_git() {
  git_isolated -c user.name="tako setup" -c user.email="setup@example.invalid" "$@"
}

# tap の main にある cask の 1 項目
tap_cask_field() {
  git --git-dir="$D/tap.git" show "main:Casks/tako.rb" 2>/dev/null |
    sed -n 's/^[[:space:]]*'"$1"' "\(.*\)"[[:space:]]*$/\1/p' | head -1
}

# origin の main にある releases.md
origin_releases_page() {
  git --git-dir="$D/origin.git" show "main:docs/src/content/docs/releases.md" 2>/dev/null
}

# add_release <tag> <prerelease true|false> — rel/<tag>/ に Release を作る（アセット 3 本つき）
add_release() {
  local tag="$1" pre="$2" name
  mkdir -p "$D/rel/$tag/assets"
  echo "$pre" > "$D/rel/$tag/prerelease"
  echo false > "$D/rel/$tag/draft"
  for name in "tako-${tag}-macos-arm64.zip" "tako-${tag}-windows-x86_64.exe" "tako-${tag}-windows-x86_64.zip"; do
    printf 'mock asset %s\n' "$name" > "$D/rel/$tag/assets/$name"
  done
}

# make_env — D に昇格のモック環境を作る
make_env() {
  D=$(mktemp -d)
  mkdir -p "$D/repo/scripts" "$D/repo/docs/scripts" "$D/repo/docs/src/content/docs" \
           "$D/mock-bin" "$D/rel" "$D/state"
  : > "$D/gitconfig"
  : > "$D/gh-calls"

  # --- 作業リポ（release.sh の REPO_ROOT）---
  cp scripts/release.sh "$D/repo/scripts/"
  cp -R scripts/lib "$D/repo/scripts/"
  cp docs/scripts/check-releases-page.mjs "$D/repo/docs/scripts/"
  printf '[workspace.package]\nversion = "99.1.4"\n' > "$D/repo/Cargo.toml"
  cat > "$D/repo/CHANGELOG.md" <<'EOF'
# Changelog

## [99.1.4] - 2026-10-02
Test release 4

## [99.1.3] - 2026-10-01
Test release 3

## [99.1.2] - 2026-09-30
Test release 2
EOF
  cat > "$D/repo/docs/src/content/docs/releases.md" <<'EOF'
---
title: リリースノート
---

<!-- 「最新の安定版」は昇格（scripts/release.sh --promote）が書き換える -->

## このページの役割

読み物。

## v99.1 系（2026-09-01 〜）— 最新の安定版は v99.1.2

**安定版は v99.1.2**（2026-09-30）です。これより新しい版は、毎晩自動で出るテスト版です。

### 安定版とテスト版

本文。

## v99.0.0（2026-08-01）

本文。
EOF
  # merge-pr.sh のスタブ: CI を待たずに origin の main を docs のブランチへ早送りする
  cat > "$D/repo/scripts/merge-pr.sh" <<EOF
#!/usr/bin/env bash
echo "merge-pr \$*" >> "$D/gh-calls"
if [[ -f "$D/state/docs-merge-fail" ]]; then echo "merge しない: CI が赤（mock）" >&2; exit 1; fi
b=\$(git --git-dir="$D/origin.git" for-each-ref --format='%(refname:short)' 'refs/heads/docs/promote-*' | head -1)
git --git-dir="$D/origin.git" update-ref refs/heads/main "refs/heads/\$b"
git --git-dir="$D/origin.git" update-ref -d "refs/heads/\$b"
EOF
  chmod +x "$D/repo/scripts/merge-pr.sh"

  git_isolated init --quiet "$D/repo"
  git_isolated -C "$D/repo" symbolic-ref HEAD refs/heads/main
  # release.sh が作る commit / tag の作者はこのリポのローカル設定（tap の clone にも写す）
  git_isolated -C "$D/repo" config user.name "tako release test"
  git_isolated -C "$D/repo" config user.email "release@example.invalid"
  git_isolated -C "$D/repo" config commit.gpgsign false
  git_isolated -C "$D/repo" config tag.gpgsign false
  git_isolated -C "$D/repo" add -A
  git_isolated -C "$D/repo" commit --quiet -m "init mock promote env"
  git_isolated -C "$D/repo" tag v99.1.3
  git_isolated -C "$D/repo" tag v99.1.4-test.1

  git_isolated init --quiet --bare "$D/origin.git"
  git_isolated --git-dir="$D/origin.git" symbolic-ref HEAD refs/heads/main
  git_isolated -C "$D/repo" remote add origin "$D/origin.git"
  git_isolated -C "$D/repo" push --quiet origin main 2>/dev/null

  # --- Release（v99.1.2 = 今の安定版 / v99.1.3 = 夜間 / v99.1.4-test.1 = テスト版）---
  add_release v99.1.2 false
  add_release v99.1.3 true
  add_release v99.1.4-test.1 true
  echo v99.1.2 > "$D/state/latest"

  # --- Homebrew tap（cask は今の安定版 v99.1.2 を指す）---
  git_isolated init --quiet --bare "$D/tap.git"
  git_isolated --git-dir="$D/tap.git" symbolic-ref HEAD refs/heads/main
  mkdir -p "$D/tap-src/Casks"
  cat > "$D/tap-src/Casks/tako.rb" <<EOF
cask "tako" do
  version "99.1.2"
  sha256 "$(sha_of "$D/rel/v99.1.2/assets/tako-v99.1.2-macos-arm64.zip")"

  url "https://github.com/takushio2525/tako/releases/download/v#{version}/tako-v#{version}-macos-arm64.zip"
  name "tako"

  app "tako.app"
end
EOF
  git_isolated init --quiet "$D/tap-src"
  git_isolated -C "$D/tap-src" symbolic-ref HEAD refs/heads/main
  setup_git -C "$D/tap-src" add -A
  setup_git -C "$D/tap-src" commit --quiet -m "cask 99.1.2"
  setup_git -C "$D/tap-src" push --quiet "$D/tap.git" main 2>/dev/null
  rm -rf "$D/tap-src"

  write_gh_stub
}

write_gh_stub() {
  cat > "$D/mock-bin/gh" <<GHEOF
#!/usr/bin/env bash
# モックの gh: 呼び出しを記録し、rel/ の状態を読み書きする
D="$D"
GHEOF
  cat >> "$D/mock-bin/gh" <<'GHEOF'
echo "$*" >> "$D/gh-calls"
sub="$1 $2"
shift 2
case "$sub" in
  "release view")
    tag=""
    if [[ $# -gt 0 && "$1" != --* ]]; then tag="$1"; shift; fi
    if [[ -z "$tag" ]]; then
      # タグ無し = Latest の Release
      [[ -s "$D/state/latest" ]] || exit 1
      cat "$D/state/latest"
      exit 0
    fi
    [[ -d "$D/rel/$tag" ]] || { echo "release not found" >&2; exit 1; }
    args="$*"
    case "$args" in
      *isPrerelease*) cat "$D/rel/$tag/prerelease" ;;
      *isDraft*) cat "$D/rel/$tag/draft" ;;
      *digest*)
        name=$(printf '%s' "$args" | sed -n 's/.*select(.name == "\([^"]*\)").*/\1/p')
        if [[ -f "$D/state/digest-override" ]]; then
          cat "$D/state/digest-override"
        elif [[ -f "$D/rel/$tag/assets/$name" ]]; then
          echo "sha256:$(/usr/bin/shasum -a 256 "$D/rel/$tag/assets/$name" | awk '{print $1}')"
        fi ;;
      *assets*) ls "$D/rel/$tag/assets" 2>/dev/null ;;
    esac
    exit 0 ;;
  "release edit")
    tag="$1"; shift
    [[ -d "$D/rel/$tag" ]] || exit 1
    for a in "$@"; do
      case "$a" in
        --prerelease=false) echo false > "$D/rel/$tag/prerelease" ;;
        --latest) echo "$tag" > "$D/state/latest" ;;
      esac
    done
    exit 0 ;;
  "release download")
    tag="$1"; shift
    pattern=""; dir="."
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --pattern) pattern="$2"; shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    [[ -d "$D/rel/$tag" ]] || { echo "release not found" >&2; exit 1; }
    mkdir -p "$dir"
    n=0
    for f in "$D/rel/$tag/assets"/*; do
      [[ -f "$f" ]] || continue
      if [[ -z "$pattern" || "$(basename "$f")" == "$pattern" ]]; then
        /bin/cp -f "$f" "$dir/"
        n=$((n + 1))
      fi
    done
    [[ $n -gt 0 ]] || { echo "no assets match the file pattern" >&2; exit 1; }
    exit 0 ;;
  "release create")
    tag="$1"; shift
    mkdir -p "$D/rel/$tag/assets"
    echo false > "$D/rel/$tag/prerelease"
    echo false > "$D/rel/$tag/draft"
    for a in "$@"; do
      if [[ -f "$a" ]]; then /bin/cp -f "$a" "$D/rel/$tag/assets/"; fi
    done
    exit 0 ;;
  "release upload")
    tag="$1"; shift
    for a in "$@"; do
      if [[ -f "$a" ]]; then /bin/cp -f "$a" "$D/rel/$tag/assets/"; fi
    done
    exit 0 ;;
  "pr list")
    exit 0 ;;
  "pr create")
    repo=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --repo) repo="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    case "$repo" in
      */homebrew-tako) echo "https://github.com/takushio2525/homebrew-tako/pull/2" ;;
      *) echo "https://github.com/takushio2525/tako/pull/1900" ;;
    esac
    exit 0 ;;
  "pr merge")
    if [[ -f "$D/state/tap-merge-fail" ]]; then echo "merge failed (mock)" >&2; exit 1; fi
    # squash merge の代わりに tap の main を update-* のブランチへ早送りする
    b=$(git --git-dir="$D/tap.git" for-each-ref --format='%(refname:short)' 'refs/heads/update-*' | head -1)
    git --git-dir="$D/tap.git" update-ref refs/heads/main "refs/heads/$b"
    git --git-dir="$D/tap.git" update-ref -d "refs/heads/$b"
    exit 0 ;;
esac
exit 0
GHEOF
  chmod +x "$D/mock-bin/gh"
}

# run_promote <tag> — release.sh --promote を /bin/bash で走らせ、OUT / RC に入れる
run_promote() {
  RC=0
  OUT=$(PATH="$D/mock-bin:$PATH" TAKO_HOMEBREW_TAP_REMOTE="$D/tap.git" \
        GIT_CONFIG_GLOBAL="$D/gitconfig" GIT_CONFIG_NOSYSTEM=1 \
        /bin/bash "$D/repo/scripts/release.sh" --promote "$1" 2>&1) || RC=$?
}

# 呼び出し元のツリーを動かさず、使い捨ての worktree・ブランチを残さない（#1136）
assert_caller_untouched() {
  assert_eq "HEAD が main のまま（#1136）" \
    "main" "$(git -C "$D/repo" symbolic-ref --short --quiet HEAD 2>/dev/null || echo '(detached)')"
  assert_eq "tracked に変更を残さない" "" "$(git -C "$D/repo" status --porcelain --untracked-files=no)"
  assert_eq "docs の worktree を残さない" "1" "$(git -C "$D/repo" worktree list | wc -l | tr -d ' ')"
  assert_eq "ローカルにブランチを増やさない" "main" "$(git -C "$D/repo" for-each-ref --format='%(refname:short)' refs/heads/)"
}

cleanup_env() {
  rm -rf "$D"
}

# --- Test 1: 夜間の素のタグ（prerelease）を昇格 → Latest + cask + docs ---
test_promote_nightly_tag() {
  echo ""
  echo "--- Test 1: 夜間の素のタグ v99.1.3 を昇格 -> Latest・cask・docs が揃う ---"
  make_env
  local want_sha
  want_sha=$(sha_of "$D/rel/v99.1.3/assets/tako-v99.1.3-macos-arm64.zip")
  run_promote v99.1.3

  assert_eq "exit 0" "0" "$RC"
  assert_contains "prerelease 解除 + Latest を要求" "$(cat "$D/gh-calls")" "release edit v99.1.3 --prerelease=false --latest"
  assert_eq "Release の prerelease が外れた" "false" "$(cat "$D/rel/v99.1.3/prerelease")"
  assert_eq "Latest が v99.1.3" "v99.1.3" "$(cat "$D/state/latest")"
  assert_not_contains "素のタグでは Release を作り直さない" "$(cat "$D/gh-calls")" "release create"
  assert_eq "タグを増やさない" "" "$(git --git-dir="$D/origin.git" tag)"
  assert_eq "tap の main の cask の version" "99.1.3" "$(tap_cask_field version)"
  assert_eq "tap の main の cask の sha256 = 公開アセットの sha256" "$want_sha" "$(tap_cask_field sha256)"
  assert_eq "tap の commit の作者はこのリポの設定（グローバル未設定でも commit できる）" \
    "release@example.invalid" "$(git --git-dir="$D/tap.git" log -1 --format=%ae main)"
  assert_contains "tap へ PR を出した" "$(cat "$D/gh-calls")" "pr create --repo takushio2525/homebrew-tako"
  assert_contains "tap の PR を squash merge" "$(cat "$D/gh-calls")" "--squash --delete-branch"
  assert_contains "docs の見出しのラベル" "$(origin_releases_page)" "## v99.1 系（2026-09-01 〜）— 最新の安定版は v99.1.3"
  assert_contains "docs の本文（日付は CHANGELOG の節）" "$(origin_releases_page)" "**安定版は v99.1.3**（2026-10-01）"
  assert_contains "docs の PR を merge-pr.sh で merge（CI 緑で merge）" "$(cat "$D/gh-calls")" "merge-pr 1900"
  assert_contains "揃ったと言い切る" "$OUT" "Homebrew cask と docs の「最新の安定版」も v99.1.3 に揃った"
  assert_caller_untouched

  echo "  （同じ昇格を打ち直す = 冪等）"
  : > "$D/gh-calls"
  run_promote v99.1.3
  assert_eq "打ち直しも exit 0" "0" "$RC"
  assert_contains "既に安定版と分かる" "$OUT" "既に安定版"
  assert_contains "cask は何もしない" "$OUT" "tap の cask は既に 99.1.3"
  assert_contains "docs は何もしない" "$OUT" "は既に v99.1.3。何もしない"
  assert_not_contains "PR を作り直さない" "$(cat "$D/gh-calls")" "pr create"
  assert_caller_untouched
  cleanup_env
}

# --- Test 2: テスト版タグ（-test.N）の従来の昇格が壊れていない ---
test_promote_test_tag() {
  echo ""
  echo "--- Test 2: テスト版 v99.1.4-test.1 を昇格 -> 安定版タグ・Release・cask・docs ---"
  make_env
  local want_sha
  # 昇格先のアセットはテスト版のアセットを名前だけ付け替えたもの
  want_sha=$(sha_of "$D/rel/v99.1.4-test.1/assets/tako-v99.1.4-test.1-macos-arm64.zip")
  run_promote v99.1.4-test.1

  assert_eq "exit 0" "0" "$RC"
  assert_eq "安定版タグを同じコミットに打って push" \
    "$(git -C "$D/repo" rev-parse v99.1.4-test.1^{commit})" \
    "$(git --git-dir="$D/origin.git" rev-parse v99.1.4^{commit} 2>/dev/null || echo none)"
  assert_contains "安定版 Release を作成" "$(cat "$D/gh-calls")" "release create v99.1.4"
  assert_eq "アセットの名前から -test.N を外した" \
    "tako-v99.1.4-macos-arm64.zip tako-v99.1.4-windows-x86_64.exe tako-v99.1.4-windows-x86_64.zip" \
    "$(ls "$D/rel/v99.1.4/assets" | tr '\n' ' ' | sed 's/ $//')"
  assert_eq "テスト版の Release は prerelease のまま残す" "true" "$(cat "$D/rel/v99.1.4-test.1/prerelease")"
  assert_eq "Latest が v99.1.4" "v99.1.4" "$(cat "$D/state/latest")"
  assert_eq "tap の main の cask の version" "99.1.4" "$(tap_cask_field version)"
  assert_eq "tap の main の cask の sha256" "$want_sha" "$(tap_cask_field sha256)"
  assert_contains "docs の見出しのラベル" "$(origin_releases_page)" "— 最新の安定版は v99.1.4"
  assert_contains "docs の本文" "$(origin_releases_page)" "**安定版は v99.1.4**（2026-10-02）"
  assert_caller_untouched
  cleanup_env
}

# expect_followup_failure <desc> <reason> — 後続の失敗は exit 4 + 理由 + 打ち直しの案内
expect_followup_failure() {
  assert_eq "exit 4（$1）" "4" "$RC"
  assert_contains "理由が出る（$1）" "$OUT" "$2"
  assert_contains "打ち直しの案内（$1）" "$OUT" "scripts/release.sh --promote v99.1.3"
  assert_eq "Release 自体は昇格済み（$1）" "false" "$(cat "$D/rel/v99.1.3/prerelease")"
}

# --- Test 3: cask の段の失敗は exit 4 で知らせる ---
test_cask_failures() {
  echo ""
  echo "--- Test 3a: macOS のアセットが無い -> exit 4 ---"
  make_env
  rm -f "$D/rel/v99.1.3/assets/tako-v99.1.3-macos-arm64.zip"
  run_promote v99.1.3
  expect_followup_failure "アセットが無い" "公開アセット tako-v99.1.3-macos-arm64.zip を落とせない"
  assert_contains "失敗した段を名指し" "$OUT" "後続が終わっていない: Homebrew cask"
  assert_eq "cask は古いまま" "99.1.2" "$(tap_cask_field version)"
  assert_contains "docs の段は別に進む" "$(origin_releases_page)" "— 最新の安定版は v99.1.3"
  assert_caller_untouched
  cleanup_env

  echo ""
  echo "--- Test 3b: sha256 を算出できない -> exit 4 ---"
  make_env
  printf '#!/usr/bin/env bash\necho "shasum: broken (mock)" >&2\nexit 1\n' > "$D/mock-bin/shasum"
  chmod +x "$D/mock-bin/shasum"
  run_promote v99.1.3
  expect_followup_failure "sha256 が取れない" "sha256 を算出できない"
  assert_eq "cask は古いまま" "99.1.2" "$(tap_cask_field version)"
  cleanup_env

  echo ""
  echo "--- Test 3c: 落とした物が GitHub の digest と食い違う -> exit 4 ---"
  make_env
  echo "sha256:0000000000000000000000000000000000000000000000000000000000000000" > "$D/state/digest-override"
  run_promote v99.1.3
  expect_followup_failure "digest の食い違い" "GitHub の digest と一致しない"
  assert_eq "cask は古いまま" "99.1.2" "$(tap_cask_field version)"
  cleanup_env

  echo ""
  echo "--- Test 3d: tap への push が拒否される -> exit 4 ---"
  make_env
  printf '#!/bin/sh\necho "push rejected (mock)" >&2\nexit 1\n' > "$D/tap.git/hooks/pre-receive"
  chmod +x "$D/tap.git/hooks/pre-receive"
  run_promote v99.1.3
  expect_followup_failure "push の失敗" "tap へ push できない"
  assert_contains "git の理由も出る" "$OUT" "push rejected (mock)"
  assert_eq "cask は古いまま" "99.1.2" "$(tap_cask_field version)"
  cleanup_env

  echo ""
  echo "--- Test 3e: tap の PR が merge されない -> main を読み直して exit 4 ---"
  make_env
  touch "$D/state/tap-merge-fail"
  run_promote v99.1.3
  expect_followup_failure "merge されない" "tap の main の cask が 99.1.3 になっていない"
  assert_contains "PR を残したと言う" "$OUT" "PR は残してある"
  cleanup_env

  echo ""
  echo "--- Test 3f: cask の url が別のアセットを指している -> exit 4 ---"
  make_env
  git_isolated clone --quiet "$D/tap.git" "$D/tap-edit" 2>/dev/null
  sed -i '' 's/macos-arm64\.zip/macos-x86_64.zip/' "$D/tap-edit/Casks/tako.rb"
  setup_git -C "$D/tap-edit" commit --quiet -am "url を x86_64 へ"
  setup_git -C "$D/tap-edit" push --quiet origin main 2>/dev/null
  run_promote v99.1.3
  expect_followup_failure "url の食い違い" "cask の url が tako-v99.1.3-macos-arm64.zip を指していない"
  assert_eq "cask は古いまま" "99.1.2" "$(tap_cask_field version)"
  cleanup_env
}

# --- Test 4: docs の段の失敗は exit 4 で知らせる ---
test_docs_failures() {
  echo ""
  echo "--- Test 4a: その版の系列の節が docs に無い -> exit 4 ---"
  make_env
  sed -i '' 's/^## v99\.1 系.*/## v99.0 系（2026-09-01 〜）— 最新の安定版は v99.1.2/' \
    "$D/repo/docs/src/content/docs/releases.md"
  git_isolated -C "$D/repo" commit --quiet -am "節を v99.0 に"
  git_isolated -C "$D/repo" push --quiet origin main 2>/dev/null
  run_promote v99.1.3
  expect_followup_failure "節が無い" "v99.1 系の節がありません"
  assert_contains "失敗した段を名指し" "$OUT" "後続が終わっていない: docs の「最新の安定版」"
  assert_eq "cask の段は別に進む" "99.1.3" "$(tap_cask_field version)"
  assert_caller_untouched
  cleanup_env

  echo ""
  echo "--- Test 4b: docs の PR が merge されない -> exit 4 ---"
  make_env
  touch "$D/state/docs-merge-fail"
  run_promote v99.1.3
  expect_followup_failure "docs の merge" "docs の PR #1900 を merge できなかった"
  assert_contains "origin の main の docs は古いまま" "$(origin_releases_page)" "— 最新の安定版は v99.1.2"
  assert_caller_untouched
  cleanup_env
}

# --- Test 5: 昇格させてはいけないものは何も変えずに止める ---
test_refusals() {
  echo ""
  echo "--- Test 5: 古い版 / 形の違うタグ / ドラフト / 無い Release は exit 1 で何も変えない ---"
  make_env
  add_release v99.1.1 true
  run_promote v99.1.1
  assert_eq "今の安定版より古い版は exit 1" "1" "$RC"
  assert_contains "古いと理由を言う" "$OUT" "今の安定版 v99.1.2 より古い"

  run_promote v99.1.3-rc.1
  assert_eq "形の違うタグは exit 1" "1" "$RC"
  assert_contains "受け付ける形を案内" "$OUT" "昇格できるタグの形ではない"

  echo true > "$D/rel/v99.1.3/draft"
  run_promote v99.1.3
  assert_eq "ドラフトは exit 1" "1" "$RC"
  assert_contains "ドラフトと言う" "$OUT" "ドラフト"
  echo false > "$D/rel/v99.1.3/draft"

  run_promote v99.9.9
  assert_eq "無い Release は exit 1" "1" "$RC"
  assert_contains "見つからないと言う" "$OUT" "リリース v99.9.9 が見つからない"

  assert_not_contains "どれも Release を書き換えていない" "$(cat "$D/gh-calls")" "release edit"
  assert_eq "Latest は v99.1.2 のまま" "v99.1.2" "$(cat "$D/state/latest")"
  assert_eq "cask は古いまま" "99.1.2" "$(tap_cask_field version)"
  assert_caller_untouched
  cleanup_env
}

# --- Test 6: 本物の releases.md が昇格で書き換えられる形を保っている ---
test_real_page_rewritable() {
  echo ""
  echo "--- Test 6: 本物の docs/src/content/docs/releases.md を --set-stable で書き換えられる ---"
  local tmp cur series out rc=0
  tmp=$(mktemp -d)
  /bin/cp -f docs/src/content/docs/releases.md "$tmp/releases.md"
  cur=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  series="${cur%.*}"
  out=$(node docs/scripts/check-releases-page.mjs --releases="$tmp/releases.md" \
        --set-stable="${series}.999" --date=2099-12-31 2>&1) || rc=$?
  assert_eq "書き換え + 検査が exit 0" "0" "$rc"
  assert_contains "見出しのラベルが書き換わる" "$(cat "$tmp/releases.md")" "— 最新の安定版は v${series}.999"
  assert_contains "本文の安定版が書き換わる" "$(cat "$tmp/releases.md")" "**安定版は v${series}.999**（2099-12-31）"
  assert_eq "ラベルの付いた見出しは 1 つ" "1" "$(grep -c "^## .*— 最新の安定版は v" "$tmp/releases.md")"
  rm -rf "$tmp"
}

# --- Test 7: 途中で死んだ昇格が exit 0 に見えない（bash 3.2 の set -u + EXIT trap）---
test_crash_is_not_success() {
  echo ""
  echo "--- Test 7: 昇格の途中で unbound variable で死んでも exit 0 にならない ---"
  make_env
  # 後続の手前の 1 行を未定義の変数を読む行へ差し替える（bash 3.2 はこのとき trap の中の $? が 0）
  perl -i -pe 's/^  echo "==> 昇格完了: .*$/  echo "\$\{TAKO_UNDEFINED_1853\}"/' "$D/repo/scripts/release.sh"
  assert_eq "注入が当たった" "1" "$(grep -c 'TAKO_UNDEFINED_1853' "$D/repo/scripts/release.sh")"
  git_isolated -C "$D/repo" commit --quiet -am "未定義の変数を読む行を注入"
  run_promote v99.1.3
  assert_contains "unbound variable で止まった" "$OUT" "unbound variable"
  assert_eq "exit 1（成功の印が無い 0 は失敗）" "1" "$RC"
  assert_contains "途中で止まったと言う" "$OUT" "昇格が途中で止まった"
  assert_caller_untouched
  cleanup_env
}

# --- 実行 ---
test_promote_nightly_tag
test_promote_test_tag
test_cask_failures
test_docs_failures
test_refusals
test_real_page_rewritable
test_crash_is_not_success

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail"
echo "================================"
[[ $FAIL -eq 0 ]]
