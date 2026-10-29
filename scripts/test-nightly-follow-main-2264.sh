#!/bin/bash
# tako:run bash scripts/test-nightly-follow-main-2264.sh
# test-nightly-follow-main-2264.sh — リリースする役の夜間が共有ツリーの main を毎晩 origin/main へ追従させる（#2264）のモックテスト
#
# #2262 は「旧版が残した detached を main へ戻した夜」だけ main を追従させていた。共有ツリーを誰も pull しない
# 常駐機（旧 Mac）では、その後の共有ツリーの main が戻した夜の origin/main で止まり、launchd が読む
# nightly-release.sh と、共有ツリーから呼ぶ mod の検査・footprint の見張りの手順が二度と更新されない。
# 修正後は、リリースする役（全部 = 既定）の夜間が、共有ツリーがブランチ main・clean のとき毎晩 fast-forward で
# 追従させる。検査だけの役（--role check）は追従させない（その PC は master が pull する）。
#
# 本番には一切触らない: origin は一時ディレクトリの bare リポ、origin へ変更を積む「別の PC」も一時のクローン、
# HOME（ログ・ロック・役と予約の状態ファイル）も一時ディレクトリ。release.sh・mod の検査・footprint の見張り・
# cargo・gh・osascript・launchctl は呼ばれたことを記録するだけの代役。
# 古い版と新しい版の夜間は中身が同じ（走らせる版）で、末尾の印の行（# NIGHTLY_MARK=old|new）と、共有ツリーから
# 呼ぶ検査の代役の版（v1 / v2）だけが違う。どちらの版が走ったかは、その夜の始めに共有ツリーの作業ツリーにある
# nightly-release.sh の印（= launchd が読むもの）で決める。
#
#   A. 共有ツリーがブランチ main のまま古い・リリースする役で 2 晩回す A/B: 修正後は 1 晩目で追従して
#      2 晩目は新しい版（1 晩目の検査から v2）・修正前（TAKO_2264_LEGACY=1）は 2 晩とも古い版
#   B. 検査だけの役: 2 晩回しても共有ツリーの HEAD・ブランチ・作業ツリーを一切動かさない
#   C. ローカル main に独自のコミットがある: 追従させず毎晩 WARN と通知（#2262 と同じ文言）
#   D. 境界: dirty / main 以外のブランチ / untracked が上書きされる / 既に最新 / --dry-run /
#      戻した夜は判断を 1 度だけ / 検査だけの役でも戻した夜の追従（#2262）は行う
#
# 修正前の版に当てる: NIGHTLY_UNDER_TEST=<修正前の nightly-release.sh> bash scripts/test-nightly-follow-main-2264.sh
# ここが落ちるということは、常駐機の共有ツリーが古い main のまま夜間・検査の手順の修正が届かないか、検査だけの
# PC の共有ツリーを夜間が書き換える（master のビルドと混ざる）か、独自のコミットのある main を動かすという意味。
# launchd と同じ /bin/bash（3.2）で走らせる。
set -uo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
NIGHTLY_UNDER_TEST="${NIGHTLY_UNDER_TEST:-$REPO_ROOT/scripts/nightly-release.sh}"
PASS=0
FAIL=0
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tako-follow-main-test-XXXXXX")"
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

# 夜間の版を置く（<dev の dir> <old|new>）: 夜間は走らせる版 + 印の行、検査の代役は old = v1 / new = v2
write_nightly_version() {
  local dev="$1" mark="$2" ver=v1
  [ "$mark" = new ] && ver=v2
  {
    cat "$NIGHTLY_UNDER_TEST"
    printf '\n# NIGHTLY_MARK=%s\n' "$mark"
  } > "$dev/scripts/nightly-release.sh"
  cat > "$dev/scripts/check-claude-mod.sh" <<SH
#!/bin/bash
echo "mod-check ${ver} \$*" >> "\$HOME/calls.log"
echo "結果: 合格（代役 ${ver}）"
exit 0
SH
  cat > "$dev/scripts/nightly-footprint-watch.sh" <<SH
#!/bin/bash
echo "footprint-watch ${ver}" >> "\$HOME/calls.log"
echo "代役の見張り ${ver}"
exit 0
SH
  chmod +x "$dev/scripts/nightly-release.sh" "$dev/scripts/check-claude-mod.sh" "$dev/scripts/nightly-footprint-watch.sh"
}

# ---- 環境: origin（bare）+ 別の PC（dev）+ 共有ツリー（repo = ブランチ main のまま古い）+ HOME ----------
# <dir> <ローカル main に独自のコミットを積むか 1/0>
# 終わった時点で: 共有ツリーはブランチ main・clean で古い版（v0.8.40 の init）、origin/main は新しい版
# （夜間・検査の手順を v2 へ + docs/NEW.md）が 1 コミット先にいる
make_env() {
  local dir="$1" local_commit="$2" stub
  mkdir -p "$dir/home/.local/bin"
  for stub in cargo gh osascript launchctl; do
    printf '#!/bin/sh\necho "%s $*" >> "$HOME/calls.log"\nexit 0\n' "$stub" > "$dir/home/.local/bin/$stub"
    chmod +x "$dir/home/.local/bin/$stub"
  done
  git init --quiet --bare "$dir/origin.git"
  git -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main

  git init --quiet "$dir/dev"
  git -C "$dir/dev" symbolic-ref HEAD refs/heads/main
  git_config_identity "$dir/dev"
  git -C "$dir/dev" remote add origin "$dir/origin.git"
  mkdir -p "$dir/dev/scripts/lib"
  write_nightly_version "$dir/dev" old
  command cp -f "$REPO_ROOT/scripts/lib/nightly-reserve.sh" "$REPO_ROOT/scripts/lib/exit-guard.sh" "$REPO_ROOT/scripts/lib/changelog.sh" "$dir/dev/scripts/lib/"
  printf '#!/bin/sh\necho "release.sh $*" >> "$HOME/calls.log"\nexit 0\n' > "$dir/dev/scripts/release.sh"
  chmod +x "$dir/dev/scripts/release.sh"
  printf '/target\n/dist\n' > "$dir/dev/.gitignore"
  printf '[workspace.package]\nversion = "0.8.40"\n' > "$dir/dev/Cargo.toml"
  printf '# Changelog\n\n## [0.8.40] - 2026-10-17\n\nInitial\n' > "$dir/dev/CHANGELOG.md"
  printf '# dummy lock\n' > "$dir/dev/Cargo.lock"
  git -C "$dir/dev" add -A
  git -C "$dir/dev" commit --quiet -m "init（共有ツリーの main が止まっている版）"
  git -C "$dir/dev" tag -a v0.8.40 -m v0.8.40
  git -C "$dir/dev" push --quiet origin HEAD:main v0.8.40

  # 共有ツリー（launchd の WorkingDirectory）。ブランチ main・clean のまま、ここから一度も pull されない
  git clone --quiet "$dir/origin.git" "$dir/repo"
  git_config_identity "$dir/repo"
  if [ "$local_commit" = 1 ]; then
    echo "この PC だけの変更" > "$dir/repo/LOCAL.md"
    git -C "$dir/repo" add LOCAL.md
    git -C "$dir/repo" commit --quiet -m "[改善] この PC だけの変更（未 push）"
  fi

  # 別の PC から origin/main が新しい版へ進む（夜間と検査の手順の修正）
  write_nightly_version "$dir/dev" new
  mkdir -p "$dir/dev/docs"
  echo "新しい版で増えたファイル" > "$dir/dev/docs/NEW.md"
  git -C "$dir/dev" add -A
  git -C "$dir/dev" commit --quiet -m "[改善] 夜間と検査の手順を新しくする"
  git -C "$dir/dev" push --quiet origin HEAD:main
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
# env を空にして /bin/bash で走らせる。その夜に走った版（作業ツリーの印）を NIGHTS へ足す
run_night() {
  local dir="$1" n="$2" mark
  shift 2
  local -a envs=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  mark=$(sed -n 's/^# NIGHTLY_MARK=//p' "$dir/repo/scripts/nightly-release.sh" | tail -1)
  : > "$dir/home/calls.log"
  rm -f "${dir:?}/home/.claude-orchestrator/logs/tako-nightly-release.log"
  RC=0
  env -i HOME="$dir/home" ${envs[@]+"${envs[@]}"} /bin/bash "$dir/repo/scripts/nightly-release.sh" ${1+"$@"} > "$dir/night$n.out" 2>&1 || RC=$?
  command cp -f "$dir/home/calls.log" "$dir/night$n.calls"
  cat "$dir/home/.claude-orchestrator/logs/tako-nightly-release.log" > "$dir/night$n.log" 2>/dev/null || : > "$dir/night$n.log"
  NIGHTS="${NIGHTS}${NIGHTS:+ }${mark:-?}"
}
night_log() { cat "$1/night$2.log"; }
night_calls() { cat "$1/night$2.calls"; }
count_in() { # <file> <先頭の語>
  local n
  n=$(grep -c "^$2" "$1" 2>/dev/null) || true
  echo "${n:-0}"
}
count_lines_with() { # <文字列> <含む語>
  local n
  n=$(grep -cF -- "$2" <<< "$1") || true
  echo "${n:-0}"
}
head_of() { git -C "$1/repo" symbolic-ref --short --quiet HEAD 2>/dev/null || echo "(detached)"; }
sha() { git -C "$1/repo" rev-parse "$2"; }
origin_sha() { git -C "$1/origin.git" rev-parse refs/heads/main; }
origin_tags() { git -C "$1/origin.git" tag --list | tr '\n' ' '; }
role_check() { env -i HOME="$1/home" /bin/bash "$1/repo/scripts/nightly-release.sh" --role check > /dev/null; }
# 共有ツリーの作業ツリー全体（.git の外の全ファイルの中身とパス）の指紋
tree_fingerprint() {
  (cd "$1/repo" && find . -path ./.git -prune -o -type f -print | LC_ALL=C sort | while IFS= read -r f; do
    printf '%s %s\n' "$(shasum < "$f" | cut -c1-40)" "$f"
  done) | shasum | cut -c1-40
}

# ============================================================================
echo "A. 共有ツリーがブランチ main のまま古い・リリースする役で 2 晩回す（修正後 / 修正前 = TAKO_2264_LEGACY=1）"
# ============================================================================
echo "A1: 修正後 — 1 晩目で追従し、2 晩目は新しい版の夜間が走る"
D="$TMP_ROOT/a1"; make_env "$D" 0
old_main="$(sha "$D" main)"
assert_eq "前提: 共有ツリーはブランチ main" "main" "$(head_of "$D")"
assert_eq "前提: 共有ツリーの main は origin/main より古い" "yes" \
  "$([ "$old_main" != "$(origin_sha "$D")" ] && echo yes || echo no)"
NIGHTS=""
tip1="$(origin_sha "$D")"
run_night "$D" 1
assert_eq "1 晩目: 終了コード 0" "0" "$RC"
assert_contains "1 晩目: main を origin/main へ追従させる" "$(night_log "$D" 1)" "main を origin/main へ追従させた"
assert_not_contains "1 晩目: detached ではないので戻さない" "$(night_log "$D" 1)" "main へ戻す"
assert_eq "1 晩目: main は夜の始めの origin/main" "$tip1" "$(sha "$D" main)"
assert_eq "1 晩目: HEAD はブランチ main" "main" "$(head_of "$D")"
assert_contains "1 晩目: 追従は検査より前なので、mod の検査はその夜から新しい手順（v2）" "$(night_calls "$D" 1)" "mod-check v2 --ref origin/main --record"
assert_eq "1 晩目: footprint の見張りも新しい手順（v2）" "1" "$(count_in "$D/night1.calls" 'footprint-watch v2')"
assert_contains "1 晩目: v0.8.41 を出す" "$(origin_tags "$D")" "v0.8.41"
assert_eq "1 晩目: 作業ツリーは clean" "" "$(git -C "$D/repo" status --porcelain)"
dev_change "$D" 2
tip2="$(origin_sha "$D")"
run_night "$D" 2
assert_eq "2 晩目: 終了コード 0" "0" "$RC"
assert_contains "2 晩目: 毎晩追従させる（リリースのコミットと昼の変更 2）" "$(night_log "$D" 2)" "main を origin/main へ追従させた"
assert_eq "2 晩目: main は夜の始めの origin/main" "$tip2" "$(sha "$D" main)"
assert_eq "2 晩目: mod の検査は v2" "1" "$(count_in "$D/night2.calls" 'mod-check v2')"
assert_contains "2 晩目: v0.8.42 を出す" "$(origin_tags "$D")" "v0.8.42"
assert_eq "2 晩の版の並び（古 / 新）" "old new" "$NIGHTS"
assert_eq "HEAD はブランチ main のまま" "main" "$(head_of "$D")"
assert_eq "リリースの worktree の残骸なし" "1" "$(git -C "$D/repo" worktree list | grep -c .)"
assert_eq "共有ツリーにコミットを作らない（main は origin/main の祖先のまま）" "yes" \
  "$(git -C "$D/repo" merge-base --is-ancestor main refs/remotes/origin/main && echo yes || echo no)"

echo "A2: 修正前（TAKO_2264_LEGACY=1）— 2 晩とも古い版の夜間が走る"
D="$TMP_ROOT/a2"; make_env "$D" 0
old_main="$(sha "$D" main)"
NIGHTS=""
run_night "$D" 1 TAKO_2264_LEGACY=1
assert_eq "1 晩目: 終了コード 0" "0" "$RC"
assert_eq "1 晩目: main は古いまま" "$old_main" "$(sha "$D" main)"
assert_eq "1 晩目: mod の検査は古い手順（v1）" "1" "$(count_in "$D/night1.calls" 'mod-check v1')"
assert_not_contains "1 晩目: 追従しない" "$(night_log "$D" 1)" "追従"
dev_change "$D" 2
run_night "$D" 2 TAKO_2264_LEGACY=1
assert_eq "2 晩目: main は古いまま" "$old_main" "$(sha "$D" main)"
assert_eq "2 晩目: mod の検査も古い手順（v1）" "1" "$(count_in "$D/night2.calls" 'mod-check v1')"
assert_eq "2 晩の版の並び（古 / 古）" "old old" "$NIGHTS"
assert_contains "リリース自体は毎晩出る（v0.8.42）" "$(origin_tags "$D")" "v0.8.42"

# ============================================================================
echo "B. 検査だけの役（--role check）: 2 晩回しても共有ツリーを一切動かさない"
# ============================================================================
D="$TMP_ROOT/b"; make_env "$D" 0
role_check "$D"
head_before="$(sha "$D" HEAD)"
main_before="$(sha "$D" main)"
tree_before="$(tree_fingerprint "$D")"
NIGHTS=""
run_night "$D" 1
assert_eq "1 晩目: 終了コード 0" "0" "$RC"
dev_change "$D" 2
run_night "$D" 2
assert_eq "2 晩目: 終了コード 0" "0" "$RC"
assert_eq "HEAD のコミットは動かない" "$head_before" "$(sha "$D" HEAD)"
assert_eq "HEAD はブランチ main のまま" "main" "$(head_of "$D")"
assert_eq "main のコミットは動かない" "$main_before" "$(sha "$D" main)"
assert_eq "作業ツリー全体の中身が 1 バイトも変わらない" "$tree_before" "$(tree_fingerprint "$D")"
assert_eq "untracked も含めて作業ツリーに変更なし" "" "$(git -C "$D/repo" status --porcelain --untracked-files=all)"
assert_not_contains "1 晩目: 追従のログは出ない" "$(night_log "$D" 1)" "追従"
assert_not_contains "2 晩目: 追従のログは出ない" "$(night_log "$D" 2)" "追従"
assert_eq "検査は毎晩回る（mod の検査 2 晩とも）" "2" "$(( $(count_in "$D/night1.calls" mod-check) + $(count_in "$D/night2.calls" mod-check) ))"
assert_eq "検査だけなのでリリースしない" "0" "$(( $(count_in "$D/night1.calls" release.sh) + $(count_in "$D/night2.calls" release.sh) ))"
assert_eq "2 晩の版の並び（古 / 古 = 手元は master が pull する）" "old old" "$NIGHTS"

# ============================================================================
echo "C. ローカル main に独自のコミットがある（リリースする役）→ 追従させず毎晩 WARN と通知"
# ============================================================================
D="$TMP_ROOT/c"; make_env "$D" 1
local_main="$(sha "$D" main)"
run_night "$D" 1
assert_eq "1 晩目: 終了コード 0（今夜のリリースは止めない）" "0" "$RC"
assert_eq "1 晩目: main は動かない" "$local_main" "$(sha "$D" main)"
assert_contains "1 晩目: 理由を WARN で出す（#2262 と同じ文言）" "$(night_log "$D" 1)" \
  "WARN: ローカルの main が origin/main より 1 コミット古いが、origin/main に無いコミットが 1 件あるので追従させない"
assert_contains "1 晩目: 翌夜に何が起きるかと直し方を言う" "$(night_log "$D" 1)" "翌夜はこの main の nightly-release.sh が走る。直すのは"
assert_contains "1 晩目: 通知する" "$(night_calls "$D" 1)" "独自のコミット 1 件があるので追従させない"
assert_contains "1 晩目: リリースは使い捨ての worktree で出る" "$(origin_tags "$D")" "v0.8.41"
dev_change "$D" 2
run_night "$D" 2
assert_eq "2 晩目: main は動かない" "$local_main" "$(sha "$D" main)"
assert_contains "2 晩目: 毎晩知らせる（古さはリリースのコミットと昼の変更ぶん増える）" "$(night_log "$D" 2)" \
  "WARN: ローカルの main が origin/main より 3 コミット古いが、origin/main に無いコミットが 1 件あるので追従させない"
assert_contains "2 晩目: 通知する" "$(night_calls "$D" 2)" "独自のコミット 1 件があるので追従させない"
assert_eq "HEAD はブランチ main" "main" "$(head_of "$D")"

# ============================================================================
echo "D. 境界（リリースする役）"
# ============================================================================
echo "D1: 共有ツリーが dirty（人の作業中）→ 追従しない（後段の SKIP が知らせる）・変更は残す"
D="$TMP_ROOT/d1"; make_env "$D" 0
old_main="$(sha "$D" main)"
echo "作業中" >> "$D/repo/CHANGELOG.md"
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_not_contains "追従のログは出ない" "$(night_log "$D" 1)" "追従"
assert_contains "dirty の SKIP" "$(night_log "$D" 1)" "SKIP: 共有ツリーが dirty"
assert_eq "main は動かない" "$old_main" "$(sha "$D" main)"
assert_contains "人の変更は残る" "$(git -C "$D/repo" diff)" "+作業中"

echo "D2: 共有ツリーが main 以外のブランチ → 追従しない（ログに 1 行）"
D="$TMP_ROOT/d2"; make_env "$D" 0
old_main="$(sha "$D" main)"
git -C "$D/repo" checkout --quiet -b feature
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_contains "main ではないと言う" "$(night_log "$D" 1)" "共有ツリーはブランチ feature（main ではない）なので main の追従はしない"
assert_eq "HEAD は feature のまま" "feature" "$(head_of "$D")"
assert_eq "main のコミットは動かない" "$old_main" "$(sha "$D" main)"
assert_not_contains "通知は出さない（人の作業の選択）" "$(night_calls "$D" 1)" "main が古いまま"

echo "D3: untracked のファイルが上書きされるので ff できない → 古い main のまま WARN と通知（ファイルは残す）"
D="$TMP_ROOT/d3"; make_env "$D" 0
old_main="$(sha "$D" main)"
mkdir -p "$D/repo/docs"
echo "人の書きかけ" > "$D/repo/docs/NEW.md"
run_night "$D" 1
assert_eq "終了コード 0（今夜のリリースは止めない）" "0" "$RC"
assert_contains "WARN（#2262 と同じ文言）" "$(night_log "$D" 1)" "WARN: main を origin/main へ追従させられなかった（1 コミット古いまま"
assert_contains "git の理由を 1 行添える" "$(night_log "$D" 1)" "untracked working tree files would be overwritten"
assert_contains "通知する" "$(night_calls "$D" 1)" "origin/main へ追従させられなかった"
assert_eq "main は古いまま" "$old_main" "$(sha "$D" main)"
assert_eq "untracked のファイルは消さない" "人の書きかけ" "$(cat "$D/repo/docs/NEW.md")"

echo "D4: 既に origin/main と同じ → 何もしない"
D="$TMP_ROOT/d4"; make_env "$D" 0
git -C "$D/repo" pull --quiet --ff-only origin main
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_not_contains "追従のログは出ない" "$(night_log "$D" 1)" "追従"
assert_eq "通知も出さない（リリースの完了通知だけ）" "0" "$(count_lines_with "$(night_calls "$D" 1)" "main が古いまま")"

echo "D5: --dry-run → 動かさず、追従させるかどうかだけを言う"
D="$TMP_ROOT/d5"; make_env "$D" 0
old_main="$(sha "$D" main)"
tree_before="$(tree_fingerprint "$D")"
run_night "$D" 1 -- --dry-run
assert_eq "終了コード 0" "0" "$RC"
assert_contains "追従させる予定を言う" "$(night_log "$D" 1)" "DRY-RUN: main は origin/main より 1 コミット古い。夜間なら fast-forward で追従させる"
assert_not_contains "追従したとは言わない" "$(night_log "$D" 1)" "追従させた"
assert_eq "main は動かない" "$old_main" "$(sha "$D" main)"
assert_eq "作業ツリーも動かない" "$tree_before" "$(tree_fingerprint "$D")"
assert_eq "release.sh は呼ばない" "0" "$(count_in "$D/night1.calls" release.sh)"

echo "D6: 旧版が残した detached を戻した夜は、判断を 1 度だけ（#2262 の追従と二重にしない）"
D="$TMP_ROOT/d6"; make_env "$D" 0
git -C "$D/repo" fetch --quiet origin
git -C "$D/repo" checkout --quiet --detach origin/main
git -C "$D/repo" commit --quiet --allow-empty -m "[リリース] v0.8.40: 夜間パッチリリース（自動）"
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_contains "main へ戻す" "$(night_log "$D" 1)" "main へ戻す（#1136 の残骸）"
assert_eq "追従のログは 1 行だけ" "1" "$(count_lines_with "$(night_log "$D" 1)" "main を origin/main へ追従させた")"
assert_eq "HEAD はブランチ main" "main" "$(head_of "$D")"

echo "D7: 検査だけの役でも、旧版が残した detached を戻した夜の追従（#2262 = #1136 の後始末）は行う"
D="$TMP_ROOT/d7"; make_env "$D" 0
role_check "$D"
git -C "$D/repo" fetch --quiet origin
git -C "$D/repo" checkout --quiet --detach origin/main
git -C "$D/repo" commit --quiet --allow-empty -m "[リリース] v0.8.40: 夜間パッチリリース（自動）"
tip="$(origin_sha "$D")"
run_night "$D" 1
assert_eq "終了コード 0" "0" "$RC"
assert_contains "戻して追従させる（古い main = --role を知らない版を翌夜に残さない）" "$(night_log "$D" 1)" "main を origin/main へ追従させた"
assert_eq "main は origin/main" "$tip" "$(sha "$D" main)"
assert_eq "検査だけなのでリリースしない" "0" "$(count_in "$D/night1.calls" release.sh)"

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
