#!/usr/bin/env bash
# test-release-retry.sh — release.sh の gh release create リトライ経路のモックテスト（#256）
#
# ダミー gh / ditto を PATH に挿入し、release.sh --skip-build --publish を実行して
# リトライ・冪等性・エラー経路を検証する。本番のタグ / Release / launchd には一切触れない。
set -euo pipefail

cd "$(dirname "$0")/.."
PASS=0
FAIL=0

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
  if echo "$haystack" | grep -qF "$needle"; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (not found: '$needle')"
    FAIL=$((FAIL + 1))
  fi
}

assert_not_exists() {
  local desc="$1" path="$2"
  if [[ ! -f "$path" ]]; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (file exists: $path)"
    FAIL=$((FAIL + 1))
  fi
}

# release.sh のための最小モック環境を一時ディレクトリに構築
make_test_env() {
  local dir
  dir=$(mktemp -d)
  mkdir -p "$dir/scripts" "$dir/dist/tako.app/Contents" \
           "$dir/web/tako-remote/dist/assets" "$dir/mock-bin"
  cp scripts/release.sh "$dir/scripts/"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$dir/scripts/deploy-pages.sh"
  chmod +x "$dir/scripts/deploy-pages.sh"
  echo 'version = "99.0.0"' > "$dir/Cargo.toml"
  printf '## [99.0.0] - 2026-01-01\nTest release\n' > "$dir/CHANGELOG.md"
  echo 'ペイン' > "$dir/web/tako-remote/dist/assets/test.js"
  cat > "$dir/mock-bin/ditto" <<'EOF'
#!/usr/bin/env bash
touch "${!#}"
EOF
  chmod +x "$dir/mock-bin/ditto"
  echo "$dir"
}

# --- Test 1: 1 回目失敗 → リトライ成功 ---
test_retry_then_success() {
  echo ""
  echo "--- Test 1: 1 回目失敗 -> リトライ成功 ---"
  local dir
  dir=$(make_test_env)
  echo 0 > "$dir/create-count"
  cat > "$dir/mock-bin/gh" <<GHEOF
#!/usr/bin/env bash
case "\$1 \$2" in
  "release view")  exit 1 ;;
  "release create")
    n=\$(cat "$dir/create-count"); n=\$((n + 1)); echo "\$n" > "$dir/create-count"
    if [ "\$n" -le 1 ]; then echo "tag not found on GitHub" >&2; exit 1; fi
    echo "https://github.com/test/releases/tag/v99.0.0"; exit 0 ;;
  "release upload") exit 0 ;;
esac
GHEOF
  chmod +x "$dir/mock-bin/gh"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 0（リトライ成功）" "0" "$rc"
  assert_contains "stderr がログに記録" "$out" "tag not found on GitHub"
  assert_contains "リトライメッセージ" "$out" "リトライ"
  assert_contains "リリース完了" "$out" "リリース完了"
  rm -rf "$dir"
}

# --- Test 2: 全回失敗 → エラー終了 ---
test_all_retries_fail() {
  echo ""
  echo "--- Test 2: 全回失敗 -> エラー終了 ---"
  local dir
  dir=$(make_test_env)
  cat > "$dir/mock-bin/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$1 $2" in
  "release view")  exit 1 ;;
  "release create") echo "server error 500" >&2; exit 1 ;;
esac
GHEOF
  chmod +x "$dir/mock-bin/gh"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 1（全失敗）" "1" "$rc"
  assert_contains "手動リカバリ手順" "$out" "手動リカバリ"
  assert_contains "stderr がログに記録" "$out" "server error 500"
  rm -rf "$dir"
}

# --- Test 3: 既存 Release → 二重作成しない（冪等） ---
test_existing_release_idempotent() {
  echo ""
  echo "--- Test 3: 既存 Release -> 二重作成しない ---"
  local dir
  dir=$(make_test_env)
  cat > "$dir/mock-bin/gh" <<GHEOF
#!/usr/bin/env bash
case "\$1 \$2" in
  "release view")   exit 0 ;;
  "release upload")  exit 0 ;;
  "release create")  echo "should not be called" >> "$dir/create-called"; exit 1 ;;
esac
GHEOF
  chmod +x "$dir/mock-bin/gh"

  local out rc=0
  out=$(PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 0（冪等成功）" "0" "$rc"
  assert_not_exists "create 未呼出" "$dir/create-called"
  assert_contains "既存 Release 検出" "$out" "既に存在"
  rm -rf "$dir"
}

# --- Test 4: 部分成功（create 失敗だが Release が存在）→ upload で回収 ---
test_partial_success_recovery() {
  echo ""
  echo "--- Test 4: 部分成功 -> upload で回収 ---"
  local dir
  dir=$(make_test_env)
  echo 0 > "$dir/view-count"
  cat > "$dir/mock-bin/gh" <<GHEOF
#!/usr/bin/env bash
case "\$1 \$2" in
  "release view")
    n=\$(cat "$dir/view-count"); n=\$((n + 1)); echo "\$n" > "$dir/view-count"
    if [ "\$n" -le 1 ]; then exit 1; fi
    exit 0 ;;
  "release create") echo "partial failure" >&2; exit 1 ;;
  "release upload") exit 0 ;;
esac
GHEOF
  chmod +x "$dir/mock-bin/gh"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 0（部分成功からの回収）" "0" "$rc"
  assert_contains "前回の試行で作成" "$out" "前回の試行で作成"
  rm -rf "$dir"
}

# --- 実行 ---
test_retry_then_success
test_all_retries_fail
test_existing_release_idempotent
test_partial_success_recovery

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail"
echo "================================"
[[ $FAIL -eq 0 ]]
