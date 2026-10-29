#!/bin/bash
# tako:run bash scripts/test-nightly-restore-main-2262.sh
# test-nightly-restore-main-2262.sh — 夜間が戻した共有ツリーの main を origin/main へ追従させる（#2262）のモックテスト
#
# 旧 Mac では共有ツリーのローカル main が 2026-08-14 で止まっていて、夜間が 1 日おきに 2 つの版を行き来していた:
#   - 新しい版の夜は restore_detached_head（#1136）が共有ツリーを古いローカル main へ戻す
#     （mod の検査・footprint の見張りのスクリプトが作業ツリーから消えるので、その夜も検査は飛ぶ）
#   - 翌夜の launchd はその古い main の nightly-release.sh（共有ツリーをその場で detach してリリースする
#     #1136 より前の版）を読むので、共有ツリーはまたリリースのコミットで detached のまま残る
# 修正後は、戻した main が origin/main より古ければ fast-forward できるときだけ追従させる。
#
# 本番には一切触らない: origin は一時ディレクトリの bare リポ、origin へ昼の変更を積む「別の PC」も一時の
# クローン、HOME（ログ・ロック・状態ファイル）も一時ディレクトリ。release.sh・mod の検査・footprint の
# 見張り・cargo・gh・osascript・launchctl は呼ばれたことを記録するだけの代役。旧版の夜間も代役
# （origin の main とタグは一時の bare リポの中でだけ動く）。
#
#   A. ローカル main が古い共有ツリーで 3 晩回す A/B: 修正後は 3 晩とも新しい版・修正前（TAKO_2262_LEGACY=1）は
#      2 晩目に古い版が走る（新 → 旧 → 新）
#   B. ローカル main に独自のコミットがある: 追従させず WARN と通知（修正前は黙る）/ origin/main を含んで先に
#      いるだけなら古くないので触らない
#   C. 境界: 既に最新 / untracked が上書きされるので ff できない / 別 worktree が main を掴んでいる /
#      fetch できない（手元の origin/main まで追従）/ --dry-run（戻すのと同じく追従もする）/ merge.ff=false
#
# 修正前の版に当てる: NIGHTLY_UNDER_TEST=<修正前の nightly-release.sh> bash scripts/test-nightly-restore-main-2262.sh
# （A の修正後の節と B・C の追従の検査が落ちる = このテストが修正前を捕まえる）
# ここが落ちるということは、夜間が共有ツリーを古い main へ戻したまま翌夜に古い版を走らせる（旧 Mac で
# mod の検査と footprint の見張りが一度も回らない）か、独自のコミットのある main を動かすという意味。
# launchd と同じ /bin/bash（3.2）で走らせる。
set -uo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
NIGHTLY_UNDER_TEST="${NIGHTLY_UNDER_TEST:-$REPO_ROOT/scripts/nightly-release.sh}"
PASS=0
FAIL=0
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tako-restore-main-test-XXXXXX")"
trap 'rm -rf "$TMP_ROOT"' EXIT

ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (expected=[$2], actual=[$3])"; fi; }
# pipefail の下で printf | grep -q は SIGPIPE で偽の不一致になるので、ヒアストリングで渡す
assert_contains() { if grep -qF -- "$3" <<< "$2"; then ok "$1"; else ng "$1 (not found: '$3')"; fi; }
assert_not_contains() { if grep -qF -- "$3" <<< "$2"; then ng "$1 (found but should not: '$3')"; else ok "$1"; fi; }

git_config_identity() {
  git -C "$1" config user.name "tako nightly test"
  git -C "$1" config user.email "nightly@example.invalid"
  git -C "$1" config commit.gpgsign false
  git -C "$1" config tag.gpgsign false
}

# 旧版の夜間の代役。#1136 より前の版と同じく、共有ツリーをその場で origin/main へ detach して
# リリースのコミットとタグを作り、detached のまま終わる（mod の検査も footprint の見張りも持たない）
write_old_nightly() {
  cat > "$1" <<'SH'
#!/bin/bash
# OLD_NIGHTLY_STUB: 旧版の夜間の代役（#1136 / #1892 / #2023 より前）
set -eu
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
cd "$(dirname "$0")/.."
echo "nightly old $(git rev-parse --short HEAD)" >> "$HOME/calls.log"
git fetch origin --tags --quiet
git checkout --quiet --detach origin/main
cur=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
new="${cur%.*}.$((${cur##*.} + 1))"
sed "s/^version = \"${cur}\"/version = \"${new}\"/" Cargo.toml > Cargo.toml.tmp
command mv -f Cargo.toml.tmp Cargo.toml
git commit --quiet -am "[リリース] v${new}: 夜間パッチリリース（自動）"
git tag -a "v${new}" -m "v${new}"
git push --quiet origin HEAD:main "v${new}"
SH
  chmod +x "$1"
}

# ---- 環境: origin（bare）+ 別の PC（dev）+ 共有ツリー（repo = ローカル main が古い）+ HOME ----------
# <dir> <ローカル main に独自のコミットを積むか 1/0>
# 終わった時点で: 共有ツリーは「前夜に旧版が残した」リリースのコミット（v0.8.40・新しい版の夜間を含む）で
# detached、ローカル main は旧版の init のまま、origin/main は v0.8.40 の上に昼の変更が 1 つ
make_env() {
  local dir="$1" local_commit="$2" stub
  mkdir -p "$dir/home/.local/bin"
  for stub in cargo gh osascript launchctl; do
    printf '#!/bin/sh\necho "%s $*" >> "$HOME/calls.log"\nexit 0\n' "$stub" > "$dir/home/.local/bin/$stub"
    chmod +x "$dir/home/.local/bin/$stub"
  done
  git init --quiet --bare "$dir/origin.git"
  git -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main

  # 旧 Mac のローカル main 相当（8/14 の版）: 旧版の夜間・release.sh の代役・v0.8.39
  git init --quiet "$dir/dev"
  git -C "$dir/dev" symbolic-ref HEAD refs/heads/main
  git_config_identity "$dir/dev"
  git -C "$dir/dev" remote add origin "$dir/origin.git"
  mkdir -p "$dir/dev/scripts/lib"
  write_old_nightly "$dir/dev/scripts/nightly-release.sh"
  printf '#!/bin/sh\necho "release.sh $*" >> "$HOME/calls.log"\nexit 0\n' > "$dir/dev/scripts/release.sh"
  chmod +x "$dir/dev/scripts/release.sh"
  printf '/target\n/dist\n' > "$dir/dev/.gitignore"
  printf '[workspace.package]\nversion = "0.8.39"\n' > "$dir/dev/Cargo.toml"
  printf '# Changelog\n\n## [0.8.39] - 2026-08-14\n\nInitial\n' > "$dir/dev/CHANGELOG.md"
  printf '# dummy lock\n' > "$dir/dev/Cargo.lock"
  git -C "$dir/dev" add -A
  git -C "$dir/dev" commit --quiet -m "init（旧 Mac のローカル main 相当 = #1136 より前の夜間）"
  git -C "$dir/dev" tag -a v0.8.39 -m v0.8.39
  git -C "$dir/dev" push --quiet origin HEAD:main v0.8.39

  # 共有ツリー（launchd の WorkingDirectory）。ここのローカル main はこの後一度も pull されない
  git clone --quiet "$dir/origin.git" "$dir/repo"
  git_config_identity "$dir/repo"
  if [ "$local_commit" = 1 ]; then
    echo "この PC だけの変更" > "$dir/repo/LOCAL.md"
    git -C "$dir/repo" add LOCAL.md
    git -C "$dir/repo" commit --quiet -m "[改善] この PC だけの変更（未 push）"
  fi

  # 別の PC から origin/main が新しい版の夜間へ進む（#1136 / #1892 / #2023 相当。検査の 2 つは tracked）
  command cp -f "$NIGHTLY_UNDER_TEST" "$dir/dev/scripts/nightly-release.sh"
  command cp -f "$REPO_ROOT/scripts/lib/nightly-reserve.sh" "$REPO_ROOT/scripts/lib/exit-guard.sh" "$REPO_ROOT/scripts/lib/changelog.sh" "$dir/dev/scripts/lib/"
  cat > "$dir/dev/scripts/check-claude-mod.sh" <<'SH'
#!/bin/bash
echo "mod-check $*" >> "$HOME/calls.log"
echo "結果: 合格（代役）"
exit 0
SH
  cat > "$dir/dev/scripts/nightly-footprint-watch.sh" <<'SH'
#!/bin/bash
echo "footprint-watch" >> "$HOME/calls.log"
echo "代役の見張り"
exit 0
SH
  chmod +x "$dir/dev/scripts/nightly-release.sh" "$dir/dev/scripts/check-claude-mod.sh" "$dir/dev/scripts/nightly-footprint-watch.sh"
  git -C "$dir/dev" add -A
  git -C "$dir/dev" commit --quiet -m "[改善] 夜間を新しい版へ（#1136 / #1892 / #2023 相当）"
  git -C "$dir/dev" push --quiet origin HEAD:main

  # 前夜: 共有ツリーの古い main の夜間（旧版）が走り、新しい版を含むリリースのコミットで detached のまま残す
  if ! env -i HOME="$dir/home" /bin/bash "$dir/repo/scripts/nightly-release.sh" > "$dir/night0.out" 2>&1; then
    ng "前夜の旧版の代役が失敗した（${dir}）: $(tail -3 "$dir/night0.out" | tr '\n' ' ')"
  fi
  : > "$dir/home/calls.log"
  dev_change "$dir" 1
}

# 別の PC が昼の変更を origin/main へ積む（夜間がリリースするものを作る）
dev_change() {
  git -C "$1/dev" pull --quiet --ff-only origin main
  echo "change $2" >> "$1/dev/NOTES.md"
  git -C "$1/dev" add -A
  git -C "$1/dev" commit --quiet -m "[改善] 昼の変更 $2"
  git -C "$1/dev" push --quiet origin HEAD:main
}

RC=0
NIGHTS=""
# <dir> <夜の番号> [env 名=値 …] [-- 夜間の引数]: launchd と同じく、共有ツリーにある nightly-release.sh を
# env を空にして /bin/bash で走らせる。どちらの版が走ったかは呼ばれた記録とログで決めて NIGHTS へ足す
run_night() {
  local dir="$1" n="$2" ver
  shift 2
  local -a envs=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  : > "$dir/home/calls.log"
  rm -f "${dir:?}/home/.claude-orchestrator/logs/tako-nightly-release.log"
  RC=0
  env -i HOME="$dir/home" ${envs[@]+"${envs[@]}"} /bin/bash "$dir/repo/scripts/nightly-release.sh" ${1+"$@"} > "$dir/night$n.out" 2>&1 || RC=$?
  command cp -f "$dir/home/calls.log" "$dir/night$n.calls"
  cat "$dir/home/.claude-orchestrator/logs/tako-nightly-release.log" > "$dir/night$n.log" 2>/dev/null || : > "$dir/night$n.log"
  if grep -q '^nightly old ' "$dir/night$n.calls"; then
    ver=old
  elif [ -s "$dir/night$n.log" ]; then
    ver=new
  else
    ver=none
  fi
  NIGHTS="${NIGHTS}${NIGHTS:+ }${ver}"
}
night_log() { cat "$1/night$2.log"; }
night_calls() { cat "$1/night$2.calls"; }
count_in() { # <file> <先頭の語>
  local n
  n=$(grep -c "^$2" "$1" 2>/dev/null) || true
  echo "${n:-0}"
}
head_of() { git -C "$1/repo" symbolic-ref --short --quiet HEAD 2>/dev/null || echo "(detached)"; }
sha() { git -C "$1/repo" rev-parse "$2"; }
origin_sha() { git -C "$1/origin.git" rev-parse refs/heads/main; }
origin_tags() { git -C "$1/origin.git" tag --list | tr '\n' ' '; }
has_old_nightly() { # 共有ツリーの作業ツリーの nightly-release.sh が旧版の代役か
  if grep -q OLD_NIGHTLY_STUB "$1/repo/scripts/nightly-release.sh"; then echo old; else echo new; fi
}

# ============================================================================
echo "A. ローカル main が古い共有ツリーで 3 晩回す（修正後 / 修正前 = TAKO_2262_LEGACY=1）"
# ============================================================================
echo "A1: 修正後 — 3 晩とも新しい版の夜間が走る"
D="$TMP_ROOT/a1"; make_env "$D" 0
old_main="$(sha "$D" main)"
assert_eq "前提: 共有ツリーは前夜のリリースのコミットで detached" "(detached)" "$(head_of "$D")"
assert_contains "前提: その先端は旧版が作った v0.8.40" "$(git -C "$D/repo" log -1 --format=%s)" "[リリース] v0.8.40"
assert_eq "前提: ローカル main は origin/main より古い（旧版の夜間のまま）" "old" \
  "$(body="$(git -C "$D/repo" show main:scripts/nightly-release.sh)" && grep -q OLD_NIGHTLY_STUB <<<"$body" && echo old || echo new)"
NIGHTS=""
tip1="$(origin_sha "$D")"
run_night "$D" 1
assert_eq "1 晩目: 終了コード 0" "0" "$RC"
assert_contains "1 晩目: detached を main へ戻す" "$(night_log "$D" 1)" "main へ戻す（#1136 の残骸）"
assert_contains "1 晩目: 戻した main を origin/main へ追従させる" "$(night_log "$D" 1)" "main を origin/main へ追従させた"
assert_eq "1 晩目: HEAD はブランチ main（#1136）" "main" "$(head_of "$D")"
assert_eq "1 晩目: main は戻した時点の origin/main（昼の変更 1 まで）" "$tip1" "$(sha "$D" main)"
assert_eq "1 晩目: 作業ツリーの夜間は新しい版" "new" "$(has_old_nightly "$D")"
assert_eq "1 晩目: mod の検査が回る（旧 Mac では飛んでいた）" "1" "$(count_in "$D/night1.calls" mod-check)"
assert_eq "1 晩目: footprint の見張りが回る" "1" "$(count_in "$D/night1.calls" footprint-watch)"
assert_not_contains "1 晩目: 検査を飛ばす WARN は出ない" "$(night_log "$D" 1)" "check-claude-mod.sh が無いので"
assert_contains "1 晩目: v0.8.41 を出す" "$(origin_tags "$D")" "v0.8.41"
assert_eq "1 晩目: 作業ツリーは clean" "" "$(git -C "$D/repo" status --porcelain)"
dev_change "$D" 2
run_night "$D" 2
assert_eq "2 晩目: 終了コード 0" "0" "$RC"
assert_eq "2 晩目: 旧版の夜間は走らない" "0" "$(count_in "$D/night2.calls" 'nightly old')"
assert_eq "2 晩目: mod の検査が回る" "1" "$(count_in "$D/night2.calls" mod-check)"
assert_not_contains "2 晩目: ブランチのままなので戻さない" "$(night_log "$D" 2)" "main へ戻す"
assert_contains "2 晩目: v0.8.42 を出す" "$(origin_tags "$D")" "v0.8.42"
assert_eq "2 晩目: HEAD はブランチ main" "main" "$(head_of "$D")"
dev_change "$D" 3
run_night "$D" 3
assert_eq "3 晩目: 終了コード 0" "0" "$RC"
assert_contains "3 晩目: v0.8.43 を出す" "$(origin_tags "$D")" "v0.8.43"
assert_eq "3 晩の版の並び（新 / 新 / 新）" "new new new" "$NIGHTS"
assert_eq "リリースの worktree の残骸なし" "1" "$(git -C "$D/repo" worktree list | grep -c .)"
assert_eq "旧版のローカル main のコミットは main の祖先に残る（書き換えない）" "yes" \
  "$(git -C "$D/repo" merge-base --is-ancestor "$old_main" main && echo yes || echo no)"

echo "A2: 修正前（TAKO_2262_LEGACY=1）— 2 晩目に古い版の夜間が走る（新 → 旧 → 新）"
D="$TMP_ROOT/a2"; make_env "$D" 0
old_main="$(sha "$D" main)"
NIGHTS=""
run_night "$D" 1 TAKO_2262_LEGACY=1
assert_eq "1 晩目: 終了コード 0" "0" "$RC"
assert_eq "1 晩目: main は古いまま（旧版の init）" "$old_main" "$(sha "$D" main)"
assert_eq "1 晩目: 作業ツリーの夜間は旧版へ戻る" "old" "$(has_old_nightly "$D")"
assert_contains "1 晩目: mod の検査はスクリプトが消えて飛ぶ（旧 Mac の 10/12・14・16 の WARN）" \
  "$(night_log "$D" 1)" "check-claude-mod.sh が無いので mod の検査を飛ばす"
assert_contains "1 晩目: footprint の見張りも飛ぶ" "$(night_log "$D" 1)" "nightly-footprint-watch.sh が無いので見張りを飛ばす"
assert_not_contains "1 晩目: 追従はしない" "$(night_log "$D" 1)" "追従"
dev_change "$D" 2
run_night "$D" 2 TAKO_2262_LEGACY=1
assert_eq "2 晩目: 旧版の夜間が走る" "1" "$(count_in "$D/night2.calls" 'nightly old')"
assert_eq "2 晩目: mod の検査は回らない" "0" "$(count_in "$D/night2.calls" mod-check)"
assert_eq "2 晩目: 共有ツリーは detached のまま残る" "(detached)" "$(head_of "$D")"
dev_change "$D" 3
run_night "$D" 3 TAKO_2262_LEGACY=1
assert_eq "3 晩の版の並び（新 / 旧 / 新 = 1 日おき）" "new old new" "$NIGHTS"

# ============================================================================
echo "B. ローカル main に独自のコミットがある"
# ============================================================================
echo "B1: 古いうえに独自のコミットがある → 追従させず、理由を WARN と通知で出す"
D="$TMP_ROOT/b1"; make_env "$D" 1
local_main="$(sha "$D" main)"
NIGHTS=""
run_night "$D" 1
assert_eq "終了コード 0（今夜のリリースは止めない）" "0" "$RC"
assert_eq "main は動かない" "$local_main" "$(sha "$D" main)"
assert_eq "HEAD はブランチ main（#1136）" "main" "$(head_of "$D")"
assert_contains "理由を WARN で出す（古さと独自のコミットの数）" "$(night_log "$D" 1)" \
  "WARN: ローカルの main が origin/main より 3 コミット古いが、origin/main に無いコミットが 1 件あるので追従させない"
assert_contains "翌夜に何が起きるかと直し方を言う" "$(night_log "$D" 1)" "翌夜はこの main の nightly-release.sh が走る。直すのは"
assert_contains "通知する" "$(night_calls "$D" 1)" "独自のコミット 1 件があるので追従させない"
assert_not_contains "追従したとは言わない" "$(night_log "$D" 1)" "追従させた"
assert_contains "リリースは使い捨ての worktree で出る" "$(origin_tags "$D")" "v0.8.41"
assert_eq "独自のコミットは origin へ出ていない" "no" \
  "$(git -C "$D/origin.git" merge-base --is-ancestor "$local_main" refs/heads/main 2>/dev/null && echo yes || echo no)"

echo "B1-修正前: 同じ状態で TAKO_2262_LEGACY=1 は黙って古い main に戻す"
D="$TMP_ROOT/b1-legacy"; make_env "$D" 1
run_night "$D" 1 TAKO_2262_LEGACY=1
assert_not_contains "WARN は出ない" "$(night_log "$D" 1)" "追従させない"
assert_not_contains "通知も出ない" "$(night_calls "$D" 1)" "main が古いまま"

echo "B2: origin/main を含んで先にいるだけ（古くない）→ 触らない・WARN も出さない"
D="$TMP_ROOT/b2"; make_env "$D" 0
release_tip="$(sha "$D" HEAD)"
git -C "$D/repo" fetch --quiet origin
git -C "$D/repo" checkout --quiet main
git -C "$D/repo" merge --ff-only --quiet origin/main
git -C "$D/repo" commit --quiet --allow-empty -m "[改善] この PC だけの変更（未 push）"
ahead_main="$(sha "$D" main)"
git -C "$D/repo" checkout --quiet --detach "$release_tip"
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_eq "main は動かない" "$ahead_main" "$(sha "$D" main)"
assert_eq "HEAD はブランチ main" "main" "$(head_of "$D")"
assert_not_contains "追従もしない" "$(night_log "$D" 1)" "追従させた"
assert_not_contains "WARN も出さない" "$(night_log "$D" 1)" "追従させない"
assert_not_contains "通知も出さない" "$(night_calls "$D" 1)" "main が古いまま"

# ============================================================================
echo "C. 境界"
# ============================================================================
echo "C1: 戻した main が既に origin/main と同じ → 何もしない"
D="$TMP_ROOT/c1"; make_env "$D" 0
git -C "$D/repo" fetch --quiet origin
git -C "$D/repo" branch --quiet -f main origin/main
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_contains "main へ戻す" "$(night_log "$D" 1)" "main へ戻す（#1136 の残骸）"
assert_not_contains "追従のログは出ない" "$(night_log "$D" 1)" "追従"
assert_eq "HEAD はブランチ main" "main" "$(head_of "$D")"

echo "C2: untracked のファイルが上書きされるので ff できない → 古い main のまま WARN と通知（ファイルは残す）"
D="$TMP_ROOT/c2"; make_env "$D" 0
old_main="$(sha "$D" main)"
echo "人の書きかけ" > "$D/repo/NOTES.md"
run_night "$D" 1
assert_eq "終了コード 0（今夜のリリースは止めない）" "0" "$RC"
assert_contains "WARN" "$(night_log "$D" 1)" "WARN: main を origin/main へ追従させられなかった（3 コミット古いまま"
assert_contains "git の理由を 1 行添える" "$(night_log "$D" 1)" "untracked working tree files would be overwritten"
assert_contains "通知する" "$(night_calls "$D" 1)" "origin/main へ追従させられなかった"
assert_eq "main は古いまま" "$old_main" "$(sha "$D" main)"
assert_eq "HEAD はブランチ main" "main" "$(head_of "$D")"
assert_eq "untracked のファイルは消さない" "人の書きかけ" "$(cat "$D/repo/NOTES.md")"

echo "C3: 別 worktree が main を掴んでいて戻せない → 追従も試みない（今までどおり WARN）"
D="$TMP_ROOT/c3"; make_env "$D" 0
git -C "$D/repo" worktree add --quiet "$D/other" main
run_night "$D" 1
assert_contains "戻せなかった WARN" "$(night_log "$D" 1)" "main へ戻せなかった"
assert_not_contains "追従のログは出ない" "$(night_log "$D" 1)" "追従"
assert_eq "共有ツリーは detached のまま" "(detached)" "$(head_of "$D")"
git -C "$D/repo" worktree remove --force "$D/other"

echo "C4: fetch できない → 手元の origin/main まで追従させる（WARN で知らせる）"
D="$TMP_ROOT/c4"; make_env "$D" 0
local_origin="$(sha "$D" refs/remotes/origin/main)"
git -C "$D/repo" remote set-url origin "$D/nowhere.git"
run_night "$D" 1
assert_contains "fetch の失敗を言う" "$(night_log "$D" 1)" "WARN: git fetch に失敗。手元の origin/main と比べて main の追従を判断する"
assert_contains "手元の origin/main へ追従させる" "$(night_log "$D" 1)" "main を origin/main へ追従させた"
assert_eq "main は手元の origin/main（前夜のリリースのコミット）" "$local_origin" "$(sha "$D" main)"
assert_eq "HEAD はブランチ main" "main" "$(head_of "$D")"

echo "C5: --dry-run でも戻すので追従もする（古い main に戻したまま終わらない）・リリースはしない"
D="$TMP_ROOT/c5"; make_env "$D" 0
tip="$(origin_sha "$D")"
run_night "$D" 1 -- --dry-run
assert_eq "終了コード 0" "0" "$RC"
assert_contains "追従させる" "$(night_log "$D" 1)" "main を origin/main へ追従させた"
assert_eq "main は origin/main" "$tip" "$(sha "$D" main)"
assert_contains "dry-run で終わる" "$(night_log "$D" 1)" "DRY-RUN: ここで終了"
assert_eq "release.sh は呼ばない" "0" "$(count_in "$D/night1.calls" release.sh)"
assert_eq "origin の main は動かない" "$tip" "$(origin_sha "$D")"

echo "C6: merge.ff=false の設定でも fast-forward する（--ff-only が設定に勝つ）"
D="$TMP_ROOT/c6"; make_env "$D" 0
git -C "$D/repo" config merge.ff false
tip="$(origin_sha "$D")"
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_eq "main は戻した時点の origin/main" "$tip" "$(git -C "$D/repo" rev-parse main)"
assert_eq "マージコミットを作らない" "0" "$(git -C "$D/repo" rev-list --count --merges main)"

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
