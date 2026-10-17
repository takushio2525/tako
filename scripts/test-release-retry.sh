#!/usr/bin/env bash
# test-release-retry.sh — release.sh のリリース経路のモックテスト（#256 / #965）
#
# ダミー gh / ditto を PATH に挿入し、release.sh --skip-build --publish を実行して
# 検証する。本番のタグ / Release / launchd には一切触れない。
#
#   Test 1〜4: gh release create のリトライ・冪等性・エラー経路（#256）
#   Test 5〜8: macOS / Windows 両 OS の待ち合わせと片肺リリースの検出（#965）
#   Test 9〜13: --check-assets（公開済みリリースの検査）。両 OS の揃い（#965）と、Release の
#              zip にライセンスが入りタグのファイルとバイト一致するか（#1850）
#
# #1850 の zip は偽の gh の `release download` が <dir>/release-assets/ から渡す（ネットワークを
# 使わない）。zip の中の置き場と表は release.sh と同じ scripts/lib/license-bundle.sh で引くので、
# ここにもライセンスの名前を並べない（表は scripts/lib/license-bundle.txt の 1 か所だけ）。
#
# モック環境は **git リポジトリ（main チェックアウト済み）** として作る。
# 夜間リリース（#1136）は release.sh を使い捨ての worktree の中で呼ぶので、
# 「release.sh は呼び出し元の HEAD を動かさない」が前提条件になった。
# 各テストの最後で HEAD が main のままであることを見る（この不変条件が壊れると、
# 共有ツリーが detached のまま残る #1136 の再発につながる）
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

# 配布物の命名（tako_asset_name）と、ライセンスの表・zip の置き場（#1850）。release.sh と同じ実装
# shellcheck source=lib/release-assets.sh
source scripts/lib/release-assets.sh
# shellcheck source=lib/license-bundle.sh
source scripts/lib/license-bundle.sh

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
  if grep -qF -- "$needle" <<<"$haystack"; then
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

# release.sh は呼び出し元のブランチを動かさない（#1136 の前提条件）。
# untracked は release.sh が作るビルド生成物なので tracked だけを見る
assert_head_unchanged() {
  local dir="$1"
  assert_eq "HEAD が main のまま（#1136）" \
    "main" "$(git -C "$dir" symbolic-ref --short --quiet HEAD 2>/dev/null || echo '(detached)')"
  assert_eq "tracked に変更を残さない（#1136）" \
    "" "$(git -C "$dir" status --porcelain --untracked-files=no)"
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
  # release.sh は zip の直前に署名の名義を読む（#1848）。ad-hoc 署名の .app として答える
  cat > "$dir/mock-bin/codesign" <<'EOF'
#!/usr/bin/env bash
echo "Signature=adhoc" >&2
EOF
  chmod +x "$dir/mock-bin/codesign"

  # git リポジトリにして main をチェックアウトしておく（#1136 の不変条件を見るため）。
  # dist/ や mock-bin/ は untracked のまま（release.sh が作り消しするので追跡しない）
  git init --quiet "$dir"
  git -C "$dir" symbolic-ref HEAD refs/heads/main
  git -C "$dir" config user.name "tako release test"
  git -C "$dir" config user.email "release@example.invalid"
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" add Cargo.toml CHANGELOG.md scripts
  git -C "$dir" commit --quiet -m "init mock release env"

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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  "release download")
    # --pattern <名前> --dir <置き場>（#1850）。<dir>/release-assets/ に無ければ本物と同じく失敗
    pat=""; out="."
    shift 2
    while [ \$# -gt 0 ]; do
      case "\$1" in
        --pattern|-p) pat="\$2"; shift 2 ;;
        --dir|-D)     out="\$2"; shift 2 ;;
        *)            shift ;;
      esac
    done
    if [ -n "\$pat" ] && [ -f "$dir/release-assets/\$pat" ]; then
      cp "$dir/release-assets/\$pat" "\$out/"
      exit 0
    fi
    echo "no assets match the file pattern" >&2
    exit 1 ;;
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
}

# ---------------------------------------------------------------------------
# --check-assets の道具（#1850）
#
# 表（scripts/lib/license-bundle.txt）は release.sh と同じ tako_license_bundle で読み、
# zip の中の置き場も tako_license_zip_entry で引く。ここに名前を並べない
# ---------------------------------------------------------------------------

# add_license_fixtures <dir> [tag]
#   表の全部の偽物をモックのリポジトリ直下に置いてコミットし、[tag] があればタグを打つ（照合先）
add_license_fixtures() {
  local dir="$1" tag="${2:-}" src win
  while IFS=$'\t' read -r src win; do
    printf 'fixture: %s\nこれは %s の偽物（#1850 のモックテスト）\n' "$src" "$src" > "$dir/$src"
    git -C "$dir" add "$src"
  done < <(tako_license_bundle "$dir")
  git -C "$dir" commit --quiet -m "license fixtures"
  if [[ -n "$tag" ]]; then
    git -C "$dir" tag "$tag"
  fi
}

# make_release_zips <dir> <tag> [win_keep] [flip]
#   Release の zip を実物と同じ置き場で <dir>/release-assets/ へ作る（中身はリポジトリ直下の偽物）。
#   win_keep … Windows の zip へ入れる本数（既定 all。同梱前の v0.8.25 の形は 1 = 表の 1 本目だけ）
#   flip     … "<platform>:<元ファイル>" の 1 本だけ先頭の 1 バイトを変える（大きさは変えない）
make_release_zips() {
  local dir="$1" tag="$2" win_keep="${3:-all}" flip="${4:-}" platform arch stage src win k entry
  mkdir -p "$dir/release-assets"
  for platform in macos windows; do
    case "$platform" in
      macos)   arch=arm64 ;;
      windows) arch=x86_64 ;;
    esac
    stage="${dir:?}/stage-$platform"
    rm -rf "${stage:?}"
    mkdir -p "$stage"
    k=0
    while IFS=$'\t' read -r src win; do
      k=$((k + 1))
      if [[ "$platform" == windows && "$win_keep" != all && $k -gt $win_keep ]]; then
        continue
      fi
      entry=$(tako_license_zip_entry "$platform" "$src" "$win")
      mkdir -p "$stage/$(dirname "$entry")"
      cp "$dir/$src" "$stage/$entry"
      if [[ "$flip" == "$platform:$src" ]]; then
        printf 'X' | dd of="$stage/$entry" bs=1 count=1 conv=notrunc 2>/dev/null
      fi
      # 実物の macOS の zip は ditto が AppleDouble（._<名前>）を隣に入れる。中身が違うので、
      # こちらを本体と取り違えれば不一致で落ちる
      if [[ "$platform" == macos ]]; then
        printf 'appledouble\n' > "$stage/$(dirname "$entry")/._$(basename "$entry")"
      fi
    done < <(tako_license_bundle "$dir")
    # ライセンス以外の中身（実物の zip と同じく本体も入っている形にする）
    case "$platform" in
      macos)
        mkdir -p "$stage/tako.app/Contents/MacOS"
        echo 'bin' > "$stage/tako.app/Contents/MacOS/tako-app" ;;
      windows)
        mkdir -p "$stage/tako"
        echo 'exe' > "$stage/tako/tako.exe" ;;
    esac
    (cd "$stage" && zip -qr -X "$dir/release-assets/$(tako_asset_name "$tag" "$platform" "$arch" zip)" .)
    rm -rf "${stage:?}"
  done
}

# write_assets <dir> <名前...> — Release に載っているアセットの名前（偽の gh が返す）
write_assets() {
  local dir="$1"
  shift
  printf '%s\n' "$@" > "$dir/assets"
}

# check_assets <dir> [tag] — 偽の gh で release.sh --check-assets を走らせる。
#   出力は CHECK_OUT、終了コードは CHECK_RC。一時 dir は <dir>/tmp に作らせ、残らないことも見る
check_assets() {
  local dir="$1" tag="${2:-}" args=(--check-assets) writes
  [[ -n "$tag" ]] && args+=("$tag")
  rm -rf "${dir:?}/tmp"
  mkdir -p "$dir/tmp"
  : > "$dir/gh-calls"
  CHECK_RC=0
  CHECK_OUT=$(TMPDIR="$dir/tmp" PATH="$dir/mock-bin:$PATH" \
    "$dir/scripts/release.sh" ${args[@]+"${args[@]}"} 2>&1) || CHECK_RC=$?
  assert_eq "取得した zip と取り出した中身を残さない" "" "$(command ls -A "$dir/tmp")"
  # Release へは読み取りだけ（作成・添付・編集・削除をしない）
  writes=$(grep -E '^release (create|upload|edit|delete)' "$dir/gh-calls" || true)
  assert_eq "Release へ書き込まない" "" "$writes"
}

# license_rows <dir> — 表の行（元ファイル<TAB>Windows での名前）
license_rows() {
  tako_license_bundle "$1"
}

# --- Test 9: --check-assets 単体モード ---
test_check_assets_mode() {
  echo ""
  echo "--- Test 9: --check-assets（公開済みリリースの検査）---"
  local dir src win platform
  dir=$(make_test_env)

  # 片肺（macOS のみ）
  write_assets "$dir" 'tako-v99.0.0-macos-arm64.zip'
  make_dual_os_gh "$dir" "$dir/assets"
  check_assets "$dir"
  assert_eq "片肺は exit 1" "1" "$CHECK_RC"
  assert_contains "Windows 不足を報告" "$CHECK_OUT" "[NG] Windows: 配布物が無い"

  # 両 OS 揃っていて、zip のライセンスもタグのファイルと一致している（#1850）
  add_license_fixtures "$dir" v99.0.0
  make_release_zips "$dir" v99.0.0
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.exe tako-v99.0.0-windows-x86_64.zip
  check_assets "$dir" v99.0.0
  assert_eq "揃っていれば exit 0" "0" "$CHECK_RC"
  assert_contains "揃っていると報告" "$CHECK_OUT" "両 OS の配布物が揃っている"
  assert_contains "表の本数を出す" "$CHECK_OUT" \
    "表 ${TAKO_LICENSE_BUNDLE_TABLE} の $(( $(license_rows "$dir" | wc -l) )) 本"
  while IFS=$'\t' read -r src win; do
    for platform in macos windows; do
      assert_contains "${platform} の ${src} が一致" "$CHECK_OUT" \
        "[OK] $(tako_license_zip_entry "$platform" "$src" "$win")（タグ v99.0.0 の ${src} とバイト一致）"
    done
  done < <(license_rows "$dir")
  assert_contains "インストーラーは中身を見ないと明示" "$CHECK_OUT" \
    "tako-v99.0.0-windows-x86_64.exe — インストーラーは中身を見ない"
  assert_contains "ライセンスが揃ったと報告" "$CHECK_OUT" \
    "両 OS の zip にライセンスが揃い、タグ v99.0.0 のファイルと一致している"
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
}

# --- Test 10: 同梱前の版の形（Windows の zip は表の 1 本目だけ = v0.8.25）を名指しして落ちる ---
test_check_assets_missing_licenses() {
  echo ""
  echo "--- Test 10: Windows の zip にライセンスが欠けている（同梱前の版の形）---"
  local dir src win k=0
  dir=$(make_test_env)
  add_license_fixtures "$dir" v99.0.0
  make_release_zips "$dir" v99.0.0 1
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.exe tako-v99.0.0-windows-x86_64.zip
  make_dual_os_gh "$dir" "$dir/assets"
  check_assets "$dir" v99.0.0

  assert_eq "欠けていれば exit 1" "1" "$CHECK_RC"
  # 配布物そのものは揃っている（2 つの検査は別々に報告する）
  assert_contains "両 OS の揃いは別に報告" "$CHECK_OUT" "両 OS の配布物が揃っている"
  while IFS=$'\t' read -r src win; do
    k=$((k + 1))
    if [[ $k -eq 1 ]]; then
      assert_contains "入っている 1 本は通す（${win}）" "$CHECK_OUT" \
        "[OK] $(tako_license_zip_entry windows "$src" "$win")（タグ v99.0.0 の ${src} とバイト一致）"
    else
      assert_contains "欠けた ${win} を名指し" "$CHECK_OUT" \
        "[NG] $(tako_license_zip_entry windows "$src" "$win"): zip に無い（元ファイル ${src}）"
    fi
    assert_contains "macOS の ${src} は通す" "$CHECK_OUT" \
      "[OK] $(tako_license_zip_entry macos "$src" "$win")（タグ v99.0.0 の ${src} とバイト一致）"
  done < <(license_rows "$dir")
  assert_contains "崩れた zip の数で警告" "$CHECK_OUT" \
    "警告: ライセンスの同梱が崩れている（v99.0.0）— 上の [NG] の zip・OS が 1 個"
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
}

# --- Test 11: 中身を 1 バイト変えた zip をバイト不一致として名指しして落ちる ---
test_check_assets_byte_mismatch() {
  echo ""
  echo "--- Test 11: 1 バイト違いの zip を名指しする ---"
  local dir rows first_src first_win last_src last_win platform src win entry size
  rows=()
  dir=$(make_test_env)
  add_license_fixtures "$dir" v99.0.0
  while IFS= read -r src; do rows+=("$src"); done < <(license_rows "$dir")
  IFS=$'\t' read -r first_src first_win <<< "${rows[0]}"
  IFS=$'\t' read -r last_src last_win <<< "${rows[${#rows[@]} - 1]}"
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.zip
  make_dual_os_gh "$dir" "$dir/assets"

  # 名前の付け替えがある表の 1 本目（Windows）と、表の最後の 1 本（macOS）をそれぞれ 1 バイト変える
  for platform in windows macos; do
    case "$platform" in
      windows) src=$first_src; win=$first_win ;;
      macos)   src=$last_src;  win=$last_win ;;
    esac
    entry=$(tako_license_zip_entry "$platform" "$src" "$win")
    rm -rf "${dir:?}/release-assets"
    make_release_zips "$dir" v99.0.0 all "$platform:$src"
    check_assets "$dir" v99.0.0
    assert_eq "${platform} の 1 バイト違いは exit 1" "1" "$CHECK_RC"
    assert_contains "${entry} の不一致を名指し" "$CHECK_OUT" \
      "[NG] ${entry}: タグ v99.0.0 の ${src} と中身が違う"
    # 大きさは同じ（1 バイトの置き換え）でも sha256 で見分けている
    size=$(( $(wc -c < "$dir/$src") ))
    assert_contains "大きさが同じでも見分ける（${entry}）" "$CHECK_OUT" "（zip ${size} B・sha256 "
    assert_contains "同じ大きさのタグ側も出す（${entry}）" "$CHECK_OUT" "/ タグ ${size} B・sha256 "
  done
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
}

# --- Test 12: 取得できない・壊れた zip・zip が無い OS・照合先のタグが無い ---
test_check_assets_error_paths() {
  echo ""
  echo "--- Test 12: 検査できないときは通さない ---"
  local dir
  dir=$(make_test_env)
  add_license_fixtures "$dir" v99.0.0
  make_release_zips "$dir" v99.0.0
  make_dual_os_gh "$dir" "$dir/assets"

  # (a) Release に名前はあるのに取得できない
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.zip
  mv -f "$dir/release-assets/tako-v99.0.0-windows-x86_64.zip" "$dir/win.zip.bak"
  check_assets "$dir" v99.0.0
  assert_eq "(a) 取得できなければ exit 1" "1" "$CHECK_RC"
  assert_contains "(a) 取得の失敗を名指し" "$CHECK_OUT" "[NG] Release から取得できない（gh release download）"

  # (b) 壊れた zip（zip ではない中身）
  echo 'not a zip' > "$dir/release-assets/tako-v99.0.0-windows-x86_64.zip"
  check_assets "$dir" v99.0.0
  assert_eq "(b) 壊れた zip は exit 1" "1" "$CHECK_RC"
  assert_contains "(b) 開けない zip を名指し" "$CHECK_OUT" \
    "[NG] zip を開けない（壊れている・zip ではない）: tako-v99.0.0-windows-x86_64.zip"
  mv -f "$dir/win.zip.bak" "$dir/release-assets/tako-v99.0.0-windows-x86_64.zip"

  # (c) Windows はインストーラーだけ（zip が無いので確かめられない = 偽の緑にしない）
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.exe
  check_assets "$dir" v99.0.0
  assert_eq "(c) zip の無い OS は exit 1" "1" "$CHECK_RC"
  assert_contains "(c) 両 OS の揃いは通る" "$CHECK_OUT" "両 OS の配布物が揃っている"
  assert_contains "(c) zip が無いことを名指し" "$CHECK_OUT" "[NG] Windows: zip が無いので中身を確かめられない"

  # (d) 照合先のタグがローカルにも origin にも無い → 取得する前に止まる
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.zip
  git -C "$dir" tag -d v99.0.0 >/dev/null
  check_assets "$dir" v99.0.0
  assert_eq "(d) タグが無ければ exit 1" "1" "$CHECK_RC"
  assert_contains "(d) タグを名指し" "$CHECK_OUT" "[NG] タグ v99.0.0 を読めないので照合できない"
  assert_eq "(d) zip を取得しない" "" "$(grep 'release download' "$dir/gh-calls" || true)"
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
}

# --- Test 13: 表は 1 か所だけ（表を変えると検査がそのまま追従する）---
test_check_assets_follows_table() {
  echo ""
  echo "--- Test 13: 表（${TAKO_LICENSE_BUNDLE_TABLE}）を変えると検査が追従する ---"
  local dir table rows_before n
  dir=$(make_test_env)
  table="$dir/$TAKO_LICENSE_BUNDLE_TABLE"
  add_license_fixtures "$dir" v99.0.0
  make_release_zips "$dir" v99.0.0
  write_assets "$dir" tako-v99.0.0-macos-arm64.zip tako-v99.0.0-windows-x86_64.zip
  make_dual_os_gh "$dir" "$dir/assets"
  rows_before=$(( $(license_rows "$dir" | wc -l) ))

  # (a) 表に 1 本足す → zip に無いことを両 OS で名指しする（シェル側に 2 か所目の表が無い証拠）
  printf 'EXTRA-NOTICE.md  EXTRA-NOTICE.txt\n' >> "$table"
  git -C "$dir" commit --quiet -am "表に 1 本足す"
  check_assets "$dir" v99.0.0
  assert_eq "(a) 足した 1 本が無ければ exit 1" "1" "$CHECK_RC"
  assert_contains "(a) 本数が表に追従" "$CHECK_OUT" "表 ${TAKO_LICENSE_BUNDLE_TABLE} の $((rows_before + 1)) 本"
  assert_contains "(a) macOS で名指し" "$CHECK_OUT" \
    "[NG] ${TAKO_LICENSE_ZIP_DIR_MACOS}/EXTRA-NOTICE.md: zip に無い（元ファイル EXTRA-NOTICE.md）"
  assert_contains "(a) Windows は Windows での名前で名指し" "$CHECK_OUT" \
    "[NG] ${TAKO_LICENSE_ZIP_DIR_WINDOWS}/EXTRA-NOTICE.txt: zip に無い（元ファイル EXTRA-NOTICE.md）"

  # (b) zip には入っているがタグに無い（照合できない = 通さない）
  echo 'extra' > "$dir/EXTRA-NOTICE.md"
  rm -rf "${dir:?}/release-assets"
  make_release_zips "$dir" v99.0.0
  rm -f "${dir:?}/EXTRA-NOTICE.md"
  check_assets "$dir" v99.0.0
  assert_eq "(b) タグに無ければ exit 1" "1" "$CHECK_RC"
  assert_contains "(b) タグに無いことを名指し" "$CHECK_OUT" \
    "[NG] ${TAKO_LICENSE_ZIP_DIR_WINDOWS}/EXTRA-NOTICE.txt: タグ v99.0.0 に EXTRA-NOTICE.md が無く照合できない"

  # (c) 2 列でない行は表の file:line で名指しして止まる
  printf 'BROKEN.md  BROKEN.txt  extra\n' >> "$table"
  git -C "$dir" commit --quiet -am "表を崩す"
  n=$(( $(wc -l < "$table") ))
  check_assets "$dir" v99.0.0
  assert_eq "(c) 崩れた表は exit 1" "1" "$CHECK_RC"
  assert_contains "(c) 表の file:line を名指し" "$CHECK_OUT" \
    "エラー: ${TAKO_LICENSE_BUNDLE_TABLE}:${n}: 「元ファイル Windows での名前」の 2 列になっていない"
  assert_head_unchanged "$dir"
  rm -rf "${dir:?}"
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
test_check_assets_missing_licenses
test_check_assets_byte_mismatch
test_check_assets_error_paths
test_check_assets_follows_table

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail"
echo "================================"
[[ $FAIL -eq 0 ]]
