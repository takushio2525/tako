#!/usr/bin/env bash
# test-nightly-reserve.sh — 夜間リリースの「次回バージョン予約」のモックテスト（#1005）
#
# 一時ディレクトリに「origin（bare）+ 作業リポ」を作り、nightly-release.sh を
# **launchd と同じ /bin/bash（3.2）・同じ env（HOME だけ）で**実走させる。
# 本番には一切触らない:
#   - HOME を一時ディレクトリへ差し替える（予約ファイル・ログ・ロックが隔離される）
#   - release.sh はスタブ（ビルドも gh も Pages デプロイも走らない）
#   - push 先は一時ディレクトリの bare リポ（ネットワークに出ない）
#   - osascript もスタブ（テスト中に通知を出さない）
#
# ここが落ちるということは、夜間リリースの版数決定が壊れたという意味:
#   - 予約したのに patch bump される（節目のリリースを夜間に乗せられない）
#   - 予約が消費されない（同じ版数で翌晩も出ようとする）
#   - 予約が無いのに patch bump 以外になる（既存挙動の破壊）
#
# Test 11〜17 は共有ツリー（install_root）の不変条件（#1136）:
#   - どの経路で終わっても HEAD は main のまま・clean・リリース worktree の残骸なし
#   - 同じ版のリリースコミットが 2 本作られない（push できなかったぶんを残さない）
#   - 二重起動を **HOME に依らず**（リポジトリ単位のロックで）止める
# ここが落ちるということは、夜間リリースのあと master / worker の
# `git pull --ff-only` が "You are not currently on a branch" で落ちる状態に戻ったという意味。
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
PASS=0
FAIL=0

assert_eq() {
  local desc="$1" expected="$2" actual="$3"
  if [[ "$expected" = "$actual" ]]; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (expected=[$expected], actual=[$actual])"
    FAIL=$((FAIL + 1))
  fi
}

assert_contains() {
  local desc="$1" haystack="$2" needle="$3"
  if printf '%s' "$haystack" | grep -qF -- "$needle"; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (not found: '$needle')"
    FAIL=$((FAIL + 1))
  fi
}

assert_not_contains() {
  local desc="$1" haystack="$2" needle="$3"
  if printf '%s' "$haystack" | grep -qF -- "$needle"; then
    echo "  FAIL: $desc (found but should not: '$needle')"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  fi
}

assert_file() {
  local desc="$1" path="$2"
  if [[ -f "$path" ]]; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (file missing: $path)"
    FAIL=$((FAIL + 1))
  fi
}

assert_no_file() {
  local desc="$1" path="$2"
  if [[ ! -f "$path" ]]; then
    echo "  PASS: $desc"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $desc (file exists: $path)"
    FAIL=$((FAIL + 1))
  fi
}

# --- 共有ツリー（install_root）の観測ヘルパ（#1136）--------------------------

# HEAD が指すブランチ名。detached なら "(detached)"
head_of() {
  git -C "$1/repo" symbolic-ref --short --quiet HEAD 2>/dev/null || echo "(detached)"
}

# 登録されている worktree の数。リリース用の使い捨てが残っていれば 2 以上になる
worktree_count() {
  git -C "$1/repo" worktree list 2>/dev/null | grep -c . || true
}

# 作業リポから見えるリリースコミットの数。**到達不能なもの（detached のまま
# 捨てられたコミット）も数える**ため、全 ref に加えて reflog も見る。
# 旧版は push が拒否された夜にここへ 1 本残し、同じ版がリモートにもう 1 本できていた
count_release_commits() {
  {
    git -C "$1/repo" log --all --format='%H %s' 2>/dev/null || true
    git -C "$1/repo" reflog --format='%H %s' 2>/dev/null || true
  } | grep '\[リリース\] v' | awk '{ print $1 }' | sort -u | grep -c . || true
}

# origin（bare）側のリリースコミット数
count_origin_release_commits() {
  git -C "$1/origin.git" log --format='%s' main 2>/dev/null | grep -c '^\[リリース\] v' || true
}

# 終了後の共有ツリーが健全か（#1136 の受け入れ条件 1・2）
assert_shared_tree_clean() {
  local desc="$1" dir="$2"
  assert_eq "${desc}: HEAD が main を指す" "main" "$(head_of "$dir")"
  assert_eq "${desc}: 作業ツリーが clean" "" "$(git -C "$dir/repo" status --porcelain)"
  assert_eq "${desc}: リリース worktree の残骸なし" "1" "$(worktree_count "$dir")"
}

# release.sh スタブを差し替える（$1 = env ディレクトリ / stdin = スクリプト本文）。
# **commit + push まで行う**。リリース作業は origin/main から生やした使い捨て
# worktree の中で走るので、origin/main に載っていないスタブは呼ばれない。
# 作業ツリーに置いたままだと「共有ツリーが dirty」でスキップされる
write_release_stub() {
  local dir="$1"
  cat > "$dir/repo/scripts/release.sh"
  chmod +x "$dir/repo/scripts/release.sh"
  git -C "$dir/repo" add scripts/release.sh
  git -C "$dir/repo" commit --quiet -m "[改善] モックの release.sh を差し替え (#1136)"
  git -C "$dir/repo" push --quiet origin HEAD:main
}

# origin へ割り込みで push する「別の worker」役のクローンを用意する
make_rival() {
  local dir="$1"
  git clone --quiet "$dir/origin.git" "$dir/rival"
  git -C "$dir/rival" config user.name "tako rival"
  git -C "$dir/rival" config user.email "rival@example.invalid"
  git -C "$dir/rival" config commit.gpgsign false
}

# --- モック環境の構築 ---------------------------------------------------------
# $1 = 現行バージョン（既定 0.7.10）/ $2 = 1 なら origin/main をタグと同一にする（変更ゼロ）

make_env() {
  local cur="${1:-0.7.10}"
  local no_change="${2:-0}"
  local dir
  dir=$(mktemp -d)
  mkdir -p "$dir/home/.local/bin" "$dir/repo/scripts/lib"

  # nightly-release.sh は自分で PATH を上書きし、その中に $HOME/.local/bin を含める。
  # cargo は ~/.cargo/bin にしか無いので HOME を差し替えると見えなくなる
  # （dry-run 経路は cargo を実行しない。実走経路でも Cargo.lock は既にあるので no-op でよい）。
  # osascript は /usr/bin より前に来るので、これで通知が出なくなる
  local stub
  for stub in cargo osascript; do
    printf '#!/bin/sh\nexit 0\n' > "$dir/home/.local/bin/$stub"
    chmod +x "$dir/home/.local/bin/$stub"
  done

  git init --quiet --bare "$dir/origin.git"
  git init --quiet "$dir/repo"
  git -C "$dir/repo" symbolic-ref HEAD refs/heads/main
  git -C "$dir/repo" config user.name "tako nightly test"
  git -C "$dir/repo" config user.email "nightly@example.invalid"
  git -C "$dir/repo" config commit.gpgsign false
  git -C "$dir/repo" config tag.gpgsign false
  git -C "$dir/repo" remote add origin "$dir/origin.git"

  cp "$REPO_ROOT/scripts/nightly-release.sh" "$dir/repo/scripts/"
  cp "$REPO_ROOT/scripts/lib/nightly-reserve.sh" "$dir/repo/scripts/lib/"
  # release.sh はスタブ。本物はビルド + gh + Pages デプロイを走らせるので絶対に使わない
  cat > "$dir/repo/scripts/release.sh" <<'STUB'
#!/bin/sh
echo "STUB release.sh $*"
exit 0
STUB
  chmod +x "$dir/repo/scripts/release.sh"

  # ビルドキャッシュの共有先。リリース worktree はここへ symlink を張るので、
  # worktree の撤去でこの実体まで消えないことを Test 11 で見る（消すと毎晩フルビルド）。
  # 本番と同じく /target は .gitignore 済みにしておく
  mkdir -p "$dir/repo/target/release"
  echo "cache" > "$dir/repo/target/release/sentinel"
  printf '/target\n/dist\n' > "$dir/repo/.gitignore"

  printf '[workspace.package]\nversion = "%s"\n' "$cur" > "$dir/repo/Cargo.toml"
  printf '# Changelog\n\n## [%s] - 2026-01-01\n\nInitial\n' "$cur" > "$dir/repo/CHANGELOG.md"
  printf '# dummy lock\n' > "$dir/repo/Cargo.lock"
  git -C "$dir/repo" add -A
  git -C "$dir/repo" commit --quiet -m "init ${cur}"
  git -C "$dir/repo" tag -a "v${cur}" -m "v${cur}"
  git -C "$dir/repo" push --quiet origin HEAD:main
  git -C "$dir/repo" push --quiet origin "v${cur}"

  if [[ "$no_change" != "1" ]]; then
    echo "change" > "$dir/repo/NOTES.md"
    git -C "$dir/repo" add -A
    git -C "$dir/repo" commit --quiet -m "[改善] モックの変更 (#1005)"
    git -C "$dir/repo" push --quiet origin HEAD:main
  fi

  echo "$dir"
}

# launchd と同じ起動の仕方（/bin/bash + HOME だけの env）
run_nightly() {
  local dir="$1"
  shift
  env -i HOME="$dir/home" /bin/bash "$dir/repo/scripts/nightly-release.sh" "$@" 2>&1
}

reserve_file_of() {
  echo "$1/home/.claude-orchestrator/state/tako-nightly-next-version"
}

# --- Test 1: 予約あり → NEW_VERSION が予約値になる（dry-run で実測）-----------
test_reserved_version_wins() {
  echo ""
  echo "Test 1: 予約あり → 予約値が採用される（minor 繰り上げ）"
  local dir out
  dir=$(make_env)
  out=$(run_nightly "$dir" --reserve 0.8.0)
  assert_contains "予約が成立する" "$out" "予約しました: 0.8.0"
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "予約値でリリースすると判定" "$out" "→ v0.8.0 としてリリースする"
  assert_contains "版種が minor" "$out" "（minor / 予約（--reserve 0.8.0）"
  assert_contains "bump 表示が 0.7.10 → 0.8.0" "$out" "bump: 0.7.10 → 0.8.0"
  assert_not_contains "patch bump にならない" "$out" "v0.7.11"
  assert_file "dry-run は予約を消費しない" "$(reserve_file_of "$dir")"
  assert_contains "消費しない旨を出す" "$out" "予約 0.8.0 はクリアしない"
  rm -rf "$dir"
}

# --- Test 2: 予約なし → 従来どおり patch bump --------------------------------
test_no_reservation_is_patch() {
  echo ""
  echo "Test 2: 予約なし → 既存挙動（patch bump）のまま"
  local dir out
  dir=$(make_env)
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "patch bump でリリースすると判定" "$out" "→ v0.7.11 としてリリースする"
  assert_contains "版種が patch" "$out" "（patch / 既定の patch bump）"
  assert_not_contains "予約の行は出ない" "$out" "予約あり:"
  assert_no_file "予約ファイルは作られない" "$(reserve_file_of "$dir")"
  rm -rf "$dir"
}

# --- Test 3: 不正な予約値は --reserve が受け付けない --------------------------
test_invalid_reservation_rejected_at_reserve() {
  echo ""
  echo "Test 3: 不正な予約値は予約時点で拒否される"
  local dir out rc
  dir=$(make_env)
  local bad
  for bad in 0.7.9 0.7.10 0.8 0.8.0-test.1 v0.8.0 abc; do
    rc=0
    out=$(run_nightly "$dir" --reserve "$bad") || rc=$?
    assert_eq "拒否して exit 2: ${bad}" "2" "$rc"
    assert_contains "理由を出す: ${bad}" "$out" "ERROR: 予約できない:"
    assert_no_file "予約ファイルを作らない: ${bad}" "$(reserve_file_of "$dir")"
  done
  # 現行より大きく、タグが未存在なら通る
  rc=0
  out=$(run_nightly "$dir" --reserve 0.7.11) || rc=$?
  assert_eq "妥当な値は通る (0.7.11)" "0" "$rc"
  assert_contains "patch 繰り上げとして予約" "$out" "パッチ繰り上げ"
  rm -rf "$dir"
}

# --- Test 4: 発火時に無効な予約 → 無視して patch bump ------------------------
# （予約した後に手動リリースが走って予約値が追い抜かれた、等の実際に起こる形）
test_stale_reservation_ignored_at_fire() {
  echo ""
  echo "Test 4: 発火時に無効な予約は無視して patch bump へフォールバック"
  local dir out f
  dir=$(make_env)
  f=$(reserve_file_of "$dir")
  mkdir -p "$(dirname "$f")"

  # 現行以下（追い抜かれた予約）
  printf '# stale\n0.7.9\n' > "$f"
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "無効と警告する" "$out" "WARN: 予約 0.7.9 は使えないので無視する"
  assert_contains "理由を出す" "$out" "現行 0.7.10 より大きくない"
  assert_contains "patch bump へ落ちる" "$out" "→ v0.7.11 としてリリースする"
  assert_file "dry-run では予約を消さない" "$f"

  # semver ではない
  printf 'not-a-version\n' > "$f"
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "semver 外を弾く" "$out" "semver（X.Y.Z）ではない: not-a-version"
  assert_contains "patch bump へ落ちる（semver 外）" "$out" "→ v0.7.11 としてリリースする"
  rm -rf "$dir"
}

# --- Test 5: 既に在るタグの版数は使わない ------------------------------------
# 通常経路では「現行 = 最新タグ」なので大小比較（> 現行）が先に立ち、タグ衝突の網は
# その内側の二重防御になる。順序に依らず「既に在る版数は通らない」ことを両面で見る
test_existing_tag_rejected() {
  echo ""
  echo "Test 5: 既に在るタグの版数は通らない（大小比較 + タグ衝突の二重の網）"
  local dir out rc
  dir=$(make_env)

  # (a) CLI: 既に在る版数（= 現行）は必ず拒否される
  rc=0
  out=$(run_nightly "$dir" --reserve 0.7.10) || rc=$?
  assert_eq "既に在る版数は exit 2" "2" "$rc"
  assert_contains "理由を出す" "$out" "ERROR: 予約できない:"

  # (b) 検証関数: 現行より大きくてもタグが在れば弾く（大小比較を通り抜けた場合の網）
  #     タグ v0.7.10 が在るリポで、現行を 0.7.9 とみなして問う
  out=$(env -i HOME="$dir/home" /bin/bash -c '
    set -euo pipefail
    source "$1/scripts/lib/nightly-reserve.sh"
    if r=$(nightly_reserve_reject_reason 0.7.10 0.7.9 "$1"); then echo "ACCEPTED"; else echo "REJECTED: $r"; fi
    if r=$(nightly_reserve_reject_reason 0.8.0 0.7.9 "$1"); then echo "ACCEPTED-0.8.0"; else echo "REJECTED: $r"; fi
  ' _ "$dir/repo" 2>&1)
  assert_contains "タグ衝突を理由に弾く" "$out" "REJECTED: タグ v0.7.10 が既に存在する"
  assert_contains "タグ未存在なら通る" "$out" "ACCEPTED-0.8.0"
  rm -rf "$dir"
}

# --- Test 6: 予約の設定・確認・取消（AI フルコントロール不変条件）-------------
test_reservation_cli_roundtrip() {
  echo ""
  echo "Test 6: 予約の設定 / 確認 / 取消が CLI から通る"
  local dir out f
  dir=$(make_env)
  f=$(reserve_file_of "$dir")

  out=$(run_nightly "$dir" --reserve)
  assert_contains "未予約を表示" "$out" "予約: なし"
  assert_contains "次回の既定を併記" "$out" "0.7.10 → 0.7.11"

  out=$(run_nightly "$dir" --reserve 0.8.0)
  assert_contains "予約を設定" "$out" "予約しました: 0.8.0"
  assert_file "予約ファイルができる" "$f"

  out=$(run_nightly "$dir" --reserve)
  assert_contains "予約を確認できる" "$out" "予約: 0.8.0"
  assert_contains "繰り上げ種別を出す" "$out" "マイナー繰り上げ"

  out=$(run_nightly "$dir" --unreserve)
  assert_contains "取消を報告" "$out" "予約を取消しました: 0.8.0"
  assert_no_file "予約ファイルが消える" "$f"

  out=$(run_nightly "$dir" --reserve)
  assert_contains "取消後は未予約" "$out" "予約: なし"

  out=$(run_nightly "$dir" --unreserve)
  assert_contains "二重取消も安全" "$out" "予約はありません"

  # 無効になった予約は「無効」と明示される（黙って patch bump にならない）
  printf '0.7.9\n' > "$f"
  out=$(run_nightly "$dir" --reserve)
  assert_contains "無効な予約を明示" "$out" "**無効**"
  rm -rf "$dir"
}

# --- Test 7: 予約あり + 変更ゼロ → スキップし、予約は保持 --------------------
test_reservation_kept_when_no_change() {
  echo ""
  echo "Test 7: 予約あり + 変更ゼロ → スキップし予約は保持（次の夜へ持ち越す）"
  local dir out f
  dir=$(make_env 0.7.10 1)
  f=$(reserve_file_of "$dir")
  out=$(run_nightly "$dir" --reserve 0.8.0)
  assert_contains "予約できる" "$out" "予約しました: 0.8.0"
  out=$(run_nightly "$dir")
  assert_contains "変更なしでスキップ" "$out" "SKIP: 変更なし"
  assert_contains "予約の保持を明示" "$out" "（予約 0.8.0 は保持）"
  assert_file "予約は消費されない" "$f"
  rm -rf "$dir"
}

# --- Test 8: 実走（予約あり）→ 予約値でリリースし予約を消費 ------------------
# release.sh をスタブにした完全オフラインの通し。push 先は一時 bare リポ
test_full_run_consumes_reservation() {
  echo ""
  echo "Test 8: 実走（予約あり）— 予約値でリリースし、タグ push 時点で消費する"
  local dir out f origin
  dir=$(make_env)
  f=$(reserve_file_of "$dir")
  origin="$dir/origin.git"
  run_nightly "$dir" --reserve 0.8.0 >/dev/null
  out=$(run_nightly "$dir")

  assert_contains "予約を読んでいる" "$out" "予約あり: 0.8.0"
  assert_contains "予約値でリリース" "$out" "→ v0.8.0 としてリリースする（minor"
  assert_contains "消費を記録" "$out" "予約 0.8.0 を消費した"
  assert_contains "完了ログ" "$out" "完了: v0.8.0"
  assert_no_file "予約ファイルが消えている" "$f"

  assert_eq "タグが作られる" "v0.8.0" "$(git -C "$origin" tag --list 'v0.8.0')"
  assert_contains "Cargo.toml が予約値へ" \
    "$(git -C "$origin" show main:Cargo.toml)" 'version = "0.8.0"'
  local changelog
  changelog=$(git -C "$origin" show main:CHANGELOG.md)
  assert_contains "CHANGELOG に節ができる" "$changelog" "## [0.8.0] - "
  assert_contains "CHANGELOG が minor 表記（英）" "$changelog" "Nightly minor release (automated)"
  assert_contains "CHANGELOG が minor 表記（日）" "$changelog" "夜間マイナーリリース（自動）"
  assert_eq "コミット件名が minor 表記" \
    "[リリース] v0.8.0: 夜間マイナーリリース（自動）" \
    "$(git -C "$origin" log -1 --format=%s main)"
  assert_contains "コミット本文に版数の由来" \
    "$(git -C "$origin" log -1 --format=%b main)" "版数の由来: 予約（--reserve 0.8.0）"

  # 消費後にもう一度発火しても、予約は効かない（patch bump へ戻る）
  git -C "$dir/repo" fetch --quiet origin --tags
  echo "more" >> "$dir/repo/NOTES.md"
  git -C "$dir/repo" checkout --quiet main 2>/dev/null || git -C "$dir/repo" checkout --quiet -B main origin/main
  git -C "$dir/repo" reset --hard --quiet origin/main
  echo "more" > "$dir/repo/NOTES2.md"
  git -C "$dir/repo" add -A
  git -C "$dir/repo" commit --quiet -m "[改善] さらに変更 (#1005)"
  git -C "$dir/repo" push --quiet origin HEAD:main
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "消費後は patch bump へ戻る" "$out" "→ v0.8.1 としてリリースする"
  rm -rf "$dir"
}

# --- Test 9: 実走（予約なし）→ patch bump のまま（既存挙動の維持）-----------
test_full_run_without_reservation() {
  echo ""
  echo "Test 9: 実走（予約なし）— 従来どおり patch bump"
  local dir out origin
  dir=$(make_env)
  origin="$dir/origin.git"
  out=$(run_nightly "$dir")
  assert_contains "patch でリリース" "$out" "→ v0.7.11 としてリリースする（patch"
  assert_eq "タグが v0.7.11" "v0.7.11" "$(git -C "$origin" tag --list 'v0.7.11')"
  assert_eq "コミット件名が従来どおり" \
    "[リリース] v0.7.11: 夜間パッチリリース（自動）" \
    "$(git -C "$origin" log -1 --format=%s main)"
  assert_contains "CHANGELOG が patch 表記" \
    "$(git -C "$origin" show main:CHANGELOG.md)" "夜間パッチリリース（自動）"
  assert_not_contains "予約の消費ログは出ない" "$out" "を消費した"
  rm -rf "$dir"
}

# --- Test 10: 不明な引数の案内に新しい操作が載っている ----------------------
test_usage_hint() {
  echo ""
  echo "Test 10: 案内文に予約の操作が載っている"
  local dir out rc
  dir=$(make_env)
  rc=0
  out=$(run_nightly "$dir" --nope) || rc=$?
  assert_eq "不明な引数は exit 2" "2" "$rc"
  assert_contains "予約の操作を案内" "$out" "--reserve [<X.Y.Z>]"
  assert_contains "取消の操作を案内" "$out" "--unreserve"
  rm -rf "$dir"
}


# ---------------------------------------------------------------------------
# Test 11〜17: 共有ツリーの不変条件（#1136）
#
# 旧版は install_root 自身を `git checkout --detach origin/main` していたので、
# ①成功しても HEAD が detached のまま残る ②push が拒否された夜は未 push の
# リリースコミットごと放置され、同じ版が別 SHA でもう 1 本作られる、という
# 2 つの実害があった。以下はその両方を機械で押さえる
# ---------------------------------------------------------------------------

# --- Test 11: 成功経路 — 終わっても共有ツリーは main のまま ------------------
test_success_leaves_shared_tree_on_main() {
  echo ""
  echo "Test 11: 実走（成功）後も共有ツリーは main / clean / 残骸なし（#1136）"
  local dir out
  dir=$(make_env)
  # ビルド時点の worktree の姿を採取する（cwd = リリース worktree）
  write_release_stub "$dir" <<STUB
#!/bin/sh
echo "STUB release.sh \$*"
case "\$*" in *--skip-build*) exit 0 ;; esac
pwd > "$dir/build-cwd"
[ -L target ] && readlink target > "$dir/target-link"
exit 0
STUB
  out=$(run_nightly "$dir")
  assert_contains "リリースは成立する" "$out" "完了: v0.7.11"
  assert_not_contains "共有ツリーの中ではビルドしない" \
    "$(cat "$dir/build-cwd" 2>/dev/null || echo '(なし)')" "$dir/repo"
  assert_eq "target は共有キャッシュへの symlink" \
    "$dir/repo/target" "$(cat "$dir/target-link" 2>/dev/null || echo '(なし)')"
  assert_contains "使い捨て worktree で作業したと記録する" "$out" "リリース worktree:"
  assert_shared_tree_clean "成功経路" "$dir"
  assert_eq "リリースコミットは 1 本だけ（同じ版が 2 本にならない）" "1" "$(count_release_commits "$dir")"
  assert_eq "origin/main のリリースコミットも 1 本" "1" "$(count_origin_release_commits "$dir")"
  # worktree の撤去が target の symlink を辿って共有キャッシュを消さない
  assert_file "共有ビルドキャッシュを消さない" "$dir/repo/target/release/sentinel"
  rm -rf "$dir"
}

# --- Test 12: ビルド失敗 — 残骸ゼロで戻る ------------------------------------
test_build_failure_restores_shared_tree() {
  echo ""
  echo "Test 12: ビルド失敗 — HEAD は main のまま・リリースコミットもタグも作らない"
  local dir out rc f
  dir=$(make_env)
  f=$(reserve_file_of "$dir")
  run_nightly "$dir" --reserve 0.8.0 >/dev/null
  write_release_stub "$dir" <<'STUB'
#!/bin/sh
echo "STUB release.sh $*"
case "$*" in *--skip-build*) exit 0 ;; esac
exit 1
STUB
  rc=0
  out=$(run_nightly "$dir") || rc=$?
  assert_eq "exit 1（ビルド失敗）" "1" "$rc"
  assert_contains "中止を記録する" "$out" "ERROR: ビルド失敗"
  assert_shared_tree_clean "ビルド失敗" "$dir"
  assert_eq "リリースコミットを 1 本も残さない" "0" "$(count_release_commits "$dir")"
  assert_eq "タグを作らない" "" "$(git -C "$dir/repo" tag --list 'v0.8.0')"
  assert_eq "origin にも出ていない" "" "$(git -C "$dir/origin.git" tag --list 'v0.8.0')"
  assert_file "予約は保持される" "$f"
  rm -rf "$dir"
}

# --- Test 13: 片肺リリース（release.sh が exit 3）— 戻ることは同じ -----------
test_one_sided_release_restores_shared_tree() {
  echo ""
  echo "Test 13: 片肺（exit 3）— exit 1 でも HEAD は main のまま・残骸なし"
  local dir out rc
  dir=$(make_env)
  write_release_stub "$dir" <<'STUB'
#!/bin/sh
echo "STUB release.sh $*"
case "$*" in *--skip-build*) exit 3 ;; esac
exit 0
STUB
  rc=0
  out=$(run_nightly "$dir") || rc=$?
  assert_eq "exit 1（片肺は警告として非ゼロ）" "1" "$rc"
  assert_contains "片肺と名指しする" "$out" "片肺リリース"
  assert_shared_tree_clean "片肺" "$dir"
  # 片肺でも Release 自体は成立しているので、リリースコミットは push 済みの 1 本
  assert_eq "リリースコミットは 1 本" "1" "$(count_release_commits "$dir")"
  assert_eq "origin にタグが出ている" "v0.7.11" "$(git -C "$dir/origin.git" tag --list 'v0.7.11')"
  rm -rf "$dir"
}

# --- Test 14: ビルド中に origin/main が進む — 重複コミットを作らない ----------
# 2026-09-04 の v0.8.5 で実際に起きた形。旧版はここで push が拒否され、set -e により
# ロールバックもログも通知もないまま死んで、未 push のリリースコミットを detached で
# 残していた（同じ版が別 SHA でもう 1 本できる原因）
test_tip_moved_during_build_aborts_cleanly() {
  echo ""
  echo "Test 14: ビルド中に origin/main が進む — 何も作らずに中止して戻る（#1136 の本丸）"
  local dir out rc f
  dir=$(make_env)
  make_rival "$dir"
  f=$(reserve_file_of "$dir")
  run_nightly "$dir" --reserve 0.8.0 >/dev/null
  # 「ビルド中に別の worker が PR を merge した」を再現する
  write_release_stub "$dir" <<STUB
#!/bin/sh
echo "STUB release.sh \$*"
case "\$*" in *--skip-build*) exit 0 ;; esac
git -C "$dir/rival" fetch --quiet origin
git -C "$dir/rival" reset --hard --quiet origin/main
echo interrupt >> "$dir/rival/NOTES.md"
git -C "$dir/rival" add -A
git -C "$dir/rival" commit --quiet -m "[改善] ビルド中に割り込んだ merge (#1136)"
git -C "$dir/rival" push --quiet origin HEAD:main
exit 0
STUB

  rc=0
  out=$(run_nightly "$dir") || rc=$?
  assert_eq "exit 1（中止）" "1" "$rc"
  assert_contains "先端が動いたことを名指しする" "$out" "ERROR: ビルド中に origin/main が進んだ"
  assert_shared_tree_clean "先端が進んだ" "$dir"
  assert_eq "リリースコミットを 1 本も残さない" "0" "$(count_release_commits "$dir")"
  assert_eq "origin にもリリースコミットは無い" "0" "$(count_origin_release_commits "$dir")"
  assert_eq "タグも作らない" "" "$(git -C "$dir/repo" tag --list 'v0.8.0')"
  assert_file "予約は保持される（次の夜へ持ち越す）" "$f"
  rm -rf "$dir"
}

# --- Test 15: 途中で kill（SIGTERM）— trap で戻す ----------------------------
test_signal_restores_shared_tree() {
  echo ""
  echo "Test 15: リリース中に SIGTERM — trap で worktree を撤去し HEAD は main のまま"
  local dir pid rc i
  dir=$(make_env)
  write_release_stub "$dir" <<STUB
#!/bin/sh
echo "STUB release.sh \$*"
touch "$dir/building"
sleep 3
exit 0
STUB

  env -i HOME="$dir/home" /bin/bash "$dir/repo/scripts/nightly-release.sh" \
    > "$dir/out.log" 2>&1 &
  pid=$!
  for i in $(seq 1 100); do
    [[ -f "$dir/building" ]] && break
    sleep 0.1
  done
  if [[ ! -f "$dir/building" ]]; then
    echo "  FAIL: ビルド段階まで到達しなかった"; FAIL=$((FAIL + 1))
    kill -KILL "$pid" 2>/dev/null || true
    rm -rf "$dir"
    return
  fi
  # リリース worktree が実際に作られていることを、生きているうちに確かめる
  assert_eq "実行中はリリース worktree が在る" "2" "$(worktree_count "$dir")"
  kill -TERM "$pid" 2>/dev/null || true
  rc=0
  wait "$pid" || rc=$?

  assert_eq "exit 130（シグナルで中止）" "130" "$rc"
  assert_contains "中止を記録する" "$(cat "$dir/out.log")" "ABORT: シグナルを受けたのでリリースを中止する"
  assert_shared_tree_clean "SIGTERM" "$dir"
  assert_eq "リリースコミットを残さない" "0" "$(count_release_commits "$dir")"
  rm -rf "$dir"
}

# --- Test 16: 旧版が残した detached を main へ戻してから走る -----------------
test_legacy_detached_head_is_restored() {
  echo ""
  echo "Test 16: 共有ツリーに残った detached（旧版の残骸）を main へ戻す"
  local dir out
  dir=$(make_env)

  # (a) 先端が夜間リリースのコミット + clean → 戻す
  git -C "$dir/repo" checkout --quiet --detach HEAD
  git -C "$dir/repo" commit --quiet --allow-empty \
    -m "[リリース] v9.9.9: 夜間パッチリリース（自動）"
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "残骸を検出して戻す" "$out" "main へ戻す（#1136 の残骸）"
  assert_eq "HEAD が main へ戻る" "main" "$(head_of "$dir")"
  assert_contains "そのまま通常の判定まで進む" "$out" "→ v0.7.11 としてリリースする"

  # (b) 人間が意図して detach している場合は動かさない
  git -C "$dir/repo" checkout --quiet --detach HEAD
  git -C "$dir/repo" commit --quiet --allow-empty -m "[改善] 人間の作業 (#1136)"
  out=$(run_nightly "$dir" --dry-run)
  assert_eq "人間の detached は動かさない" "(detached)" "$(head_of "$dir")"
  assert_not_contains "戻したログは出ない" "$out" "main へ戻す"
  rm -rf "$dir"
}

# --- Test 17: 二重起動を $HOME に依らず止める --------------------------------
# 2026-09-04 は、この HOME のログにもロックにも痕跡を残さないもう 1 つの実行と
# 並走して同じ版のコミットが 2 本できた。リポジトリ単位のロックがその網になる
test_repo_scoped_lock_blocks_concurrent_run() {
  echo ""
  echo "Test 17: リポジトリ単位のロックが（HOME が違っても）多重起動を止める"
  local dir out lock
  dir=$(make_env)
  lock="$dir/repo/.git/tako-nightly-release.lock"
  mkdir -p "$lock"
  echo $$ > "$lock/pid"

  out=$(run_nightly "$dir" --dry-run)
  assert_contains "多重起動として抜ける" "$out" "SKIP: 多重起動（リポジトリ単位のロックを保持中の PID:"
  assert_not_contains "リリース判定まで進まない" "$out" "としてリリースする"
  assert_file "他人のロックを消さない" "$lock/pid"

  # 死んだ PID のロックは stale として回収する（回収できないと永久に止まる）
  echo "999999" > "$lock/pid"
  out=$(run_nightly "$dir" --dry-run)
  assert_contains "stale ロックは回収する" "$out" "WARN: stale ロックを回収（リポジトリ単位"
  assert_contains "回収後は通常どおり進む" "$out" "→ v0.7.11 としてリリースする"
  rm -rf "$dir"
}

# --- 実行 ---
test_reserved_version_wins
test_no_reservation_is_patch
test_invalid_reservation_rejected_at_reserve
test_stale_reservation_ignored_at_fire
test_existing_tag_rejected
test_reservation_cli_roundtrip
test_reservation_kept_when_no_change
test_full_run_consumes_reservation
test_full_run_without_reservation
test_usage_hint
test_success_leaves_shared_tree_on_main
test_build_failure_restores_shared_tree
test_one_sided_release_restores_shared_tree
test_tip_moved_during_build_aborts_cleanly
test_signal_restores_shared_tree
test_legacy_detached_head_is_restored
test_repo_scoped_lock_blocks_concurrent_run

echo ""
echo "================================"
echo "  結果: ${PASS} pass / ${FAIL} fail"
echo "================================"
[[ $FAIL -eq 0 ]]
