#!/usr/bin/env bash
# test-release-retry.sh — release.sh のリリース経路のモックテスト（#256 / #965）
#
# ダミー gh / ditto を PATH に挿入し、release.sh --skip-build --publish を実行して
# 検証する。本番のタグ / Release / launchd には一切触れない。
#
#   Test 1〜4: gh release create のリトライ・冪等性・エラー経路（#256）
#   Test 5〜8: macOS / Windows 両 OS の待ち合わせと片肺リリースの検出（#965）
#
# Test 5〜8 が落ちるということは「片方の OS しか無いリリースを検出できない」という意味で、
# それは v0.7.8 まで実際に続いていた状態（Windows の利用者に更新が見えない）に戻ることを指す。
set -euo pipefail

cd "$(dirname "$0")/.."
PASS=0
FAIL=0

# release.sh は #837 の後始末で Launch Services を触る。モックテストは本番の LS
# データベースに一切触らないよう、lsregister を存在しないパスへ差し替える
# （lib/launch-services.sh 側は実行可能でなければ全操作を no-op にする）
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
  if echo "$haystack" | grep -qF -- "$needle"; then
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

assert_not_dir() {
  local desc="$1" path="$2"
  if [[ ! -d "$path" ]]; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (directory exists: $path)"
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
  # release.sh は scripts/lib/*.sh を source する（release-assets #594 / launch-services #837）。
  # ここをコピーし忘れると source に失敗して即 exit し、全アサーションが空振りする
  cp -R scripts/lib "$dir/scripts/"
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
        "$dir/scripts/release.sh" --skip-build --publish --no-wait-windows 2>&1) || rc=$?

  assert_eq "exit 0（リトライ成功）" "0" "$rc"
  assert_contains "stderr がログに記録" "$out" "tag not found on GitHub"
  assert_contains "リトライメッセージ" "$out" "リトライ"
  assert_contains "リリース完了" "$out" "リリース完了"
  assert_not_dir "ビルド出力の後始末（#837）" "$dir/dist/tako.app"
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
        "$dir/scripts/release.sh" --skip-build --publish --no-wait-windows 2>&1) || rc=$?

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
        "$dir/scripts/release.sh" --skip-build --publish --no-wait-windows 2>&1) || rc=$?

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
        "$dir/scripts/release.sh" --skip-build --publish --no-wait-windows 2>&1) || rc=$?

  assert_eq "exit 0（部分成功からの回収）" "0" "$rc"
  assert_contains "前回の試行で作成" "$out" "前回の試行で作成"
  rm -rf "$dir"
}

# ---------------------------------------------------------------------------
# Test 5〜8: 両 OS 同時リリース（#965）
#
# 「Windows 配布物を待って、揃ったかどうかを判定する」経路を見る。
# mock gh は Actions API（run list / run view / workflow run）と
# release view --json assets を返せる必要がある
# ---------------------------------------------------------------------------

# make_dual_os_gh <dir> <assets-file> [run-conclusion]
#   assets-file  … gh release view --json assets が返す名前を 1 行ずつ書いたファイル。
#                  ワークフロー完了後に windows 分が「足される」様子を作るため、
#                  mock は毎回このファイルを読み直す
#   run-conclusion … gh run view が返す conclusion（既定 success）。空文字にすると
#                  「実行が見つからない」= workflow run で起動する経路になる
make_dual_os_gh() {
  local dir="$1" assets_file="$2" conclusion="${3:-success}"
  cat > "$dir/mock-bin/gh" <<GHEOF
#!/usr/bin/env bash
# 呼ばれたサブコマンドを記録（アサーションで使う）
echo "\$*" >> "$dir/gh-calls"
case "\$1 \$2" in
  "release view")
    if printf '%s' "\$*" | grep -q -- '--json assets'; then
      # .assets[].name 相当
      cat "$assets_file" 2>/dev/null
      exit 0
    fi
    # 存在確認
    exit 0 ;;
  "release create") echo "https://github.com/test/releases/tag/v99.0.0"; exit 0 ;;
  "release upload") exit 0 ;;
  "release edit")   touch "$dir/notes-refreshed"; exit 0 ;;
  "run list")
    if [ -n "$conclusion" ]; then echo "4242"; fi
    exit 0 ;;
  "run view")
    if printf '%s' "\$*" | grep -q 'status'; then echo "completed"; else echo "$conclusion"; fi
    exit 0 ;;
  "workflow run") touch "$dir/workflow-dispatched"; exit 0 ;;
esac
GHEOF
  chmod +x "$dir/mock-bin/gh"
}

# --- Test 5: Windows 配布物が揃う -> ノート再生成 + exit 0 ---
test_dual_os_release_completes() {
  echo ""
  echo "--- Test 5: 両 OS 揃う -> ノート再生成 + exit 0 ---"
  local dir
  dir=$(make_test_env)
  # 最初から両 OS のアセットが Release にある状態（Windows 側の添付が先に終わったケース）
  cat > "$dir/assets" <<'EOF'
tako-v99.0.0-macos-arm64.zip
tako-v99.0.0-windows-x86_64.exe
tako-v99.0.0-windows-x86_64.zip
EOF
  make_dual_os_gh "$dir" "$dir/assets"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 TAKO_WINDOWS_POLL_SECONDS=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 0（両 OS 揃った）" "0" "$rc"
  assert_contains "Windows 配布物を検出" "$out" "Windows 配布物は既に添付済み"
  assert_contains "完全性の検査を通過" "$out" "両 OS の配布物が揃っている"
  assert_contains "macOS アセットを列挙" "$out" "[OK] macOS: tako-v99.0.0-macos-arm64.zip"
  assert_contains "Windows アセットを列挙" "$out" "[OK] Windows: tako-v99.0.0-windows-x86_64.exe"
  rm -rf "$dir"
}

# --- Test 6: ワークフロー完了後に Windows アセットが現れる ---
test_dual_os_waits_for_workflow() {
  echo ""
  echo "--- Test 6: ワークフローを待ってから揃う ---"
  local dir
  dir=$(make_test_env)
  # 最初は macOS のみ。mock gh の run view が completed/success を返した後で
  # windows 分を足す（= ワークフローが添付した状態）
  echo 'tako-v99.0.0-macos-arm64.zip' > "$dir/assets"
  make_dual_os_gh "$dir" "$dir/assets"
  # run view が呼ばれたら windows アセットを足す形にラップする
  mv "$dir/mock-bin/gh" "$dir/mock-bin/gh-inner"
  cat > "$dir/mock-bin/gh" <<GHEOF
#!/usr/bin/env bash
if [ "\$1 \$2" = "run view" ]; then
  grep -q windows "$dir/assets" || {
    echo 'tako-v99.0.0-windows-x86_64.exe' >> "$dir/assets"
    echo 'tako-v99.0.0-windows-x86_64.zip' >> "$dir/assets"
  }
fi
exec "$dir/mock-bin/gh-inner" "\$@"
GHEOF
  chmod +x "$dir/mock-bin/gh"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 TAKO_WINDOWS_POLL_SECONDS=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 0（待って揃った）" "0" "$rc"
  assert_contains "ワークフローを待った" "$out" "Windows 配布物を待つ"
  assert_contains "実行 ID を表示" "$out" "実行 ID: 4242"
  assert_contains "ワークフロー完了を検出" "$out" "ワークフロー完了: success"
  assert_contains "ノートを作り直した" "$out" "ノートを実アセットから作り直す"
  if [[ -f "$dir/notes-refreshed" ]]; then
    echo "  PASS: gh release edit --notes を呼んだ"; PASS=$((PASS + 1))
  else
    echo "  FAIL: gh release edit --notes を呼んでいない"; FAIL=$((FAIL + 1))
  fi
  rm -rf "$dir"
}

# --- Test 7: 片肺リリースの検出（#965 受け入れ条件 3）---
test_one_sided_release_detected() {
  echo ""
  echo "--- Test 7: 片肺（macOS のみ）を検出して exit 3 ---"
  local dir
  dir=$(make_test_env)
  # ワークフローは success を返すのに Windows アセットが現れない
  # （= 添付に失敗した / 別タグへ上げた等。ここを見逃すと片肺のまま気付かない）
  echo 'tako-v99.0.0-macos-arm64.zip' > "$dir/assets"
  make_dual_os_gh "$dir" "$dir/assets"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 TAKO_WINDOWS_POLL_SECONDS=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish 2>&1) || rc=$?

  assert_eq "exit 3（片肺 = Release は成立・アセット不足）" "3" "$rc"
  assert_contains "Windows が無いと報告" "$out" "[NG] Windows: 配布物が無い"
  assert_contains "片肺と名指し" "$out" "片肺リリース"
  assert_contains "回収手順を提示" "$out" "release-windows.ps1"
  assert_contains "ノート再生成の案内" "$out" "--update-notes"
  rm -rf "$dir"
}

# --- Test 8: --no-wait-windows は待たずに成功する（緊急の macOS 先行公開）---
test_no_wait_windows_opt_out() {
  echo ""
  echo "--- Test 8: --no-wait-windows は待たない ---"
  local dir
  dir=$(make_test_env)
  echo 'tako-v99.0.0-macos-arm64.zip' > "$dir/assets"
  make_dual_os_gh "$dir" "$dir/assets"

  local out rc=0
  out=$(TAKO_RELEASE_RETRY_WAIT=0 TAKO_WINDOWS_POLL_SECONDS=0 PATH="$dir/mock-bin:$PATH" \
        "$dir/scripts/release.sh" --skip-build --publish --no-wait-windows 2>&1) || rc=$?

  assert_eq "exit 0（明示的な opt-out）" "0" "$rc"
  assert_contains "待たないことを明示" "$out" "--no-wait-windows: Windows 配布物を待たない"
  # 待たなくても状態は報告する（黙って片肺にしない）
  assert_contains "状態は報告する" "$out" "[NG] Windows: 配布物が無い"
  if grep -q 'workflow run' "$dir/gh-calls" 2>/dev/null; then
    echo "  FAIL: workflow run を呼んでいる（待たない指定なのに起動した）"; FAIL=$((FAIL + 1))
  else
    echo "  PASS: ワークフローを起動していない"; PASS=$((PASS + 1))
  fi
  rm -rf "$dir"
}

# --- Test 9: --check-assets 単体モード ---
test_check_assets_mode() {
  echo ""
  echo "--- Test 9: --check-assets（公開済みリリースの検査）---"
  local dir
  dir=$(make_test_env)

  # 片肺（macOS のみ）
  echo 'tako-v99.0.0-macos-arm64.zip' > "$dir/assets"
  make_dual_os_gh "$dir" "$dir/assets"
  local out rc=0
  out=$(PATH="$dir/mock-bin:$PATH" "$dir/scripts/release.sh" --check-assets 2>&1) || rc=$?
  assert_eq "片肺は exit 1" "1" "$rc"
  assert_contains "Windows 不足を報告" "$out" "[NG] Windows: 配布物が無い"

  # 両 OS 揃っている
  cat > "$dir/assets" <<'EOF'
tako-v99.0.0-macos-arm64.zip
tako-v99.0.0-windows-x86_64.exe
EOF
  rc=0
  out=$(PATH="$dir/mock-bin:$PATH" "$dir/scripts/release.sh" --check-assets v99.0.0 2>&1) || rc=$?
  assert_eq "揃っていれば exit 0" "0" "$rc"
  assert_contains "揃っていると報告" "$out" "両 OS の配布物が揃っている"
  rm -rf "$dir"
}

# --- 実行 ---
test_retry_then_success
test_all_retries_fail
test_existing_release_idempotent
test_partial_success_recovery
test_dual_os_release_completes
test_dual_os_waits_for_workflow
test_one_sided_release_detected
test_no_wait_windows_opt_out
test_check_assets_mode

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail"
echo "================================"
[[ $FAIL -eq 0 ]]
