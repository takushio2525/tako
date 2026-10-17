#!/bin/bash
# tako:run bash scripts/test-nightly-role-2252.sh
# test-nightly-role-2252.sh — 夜間リリースの「検査だけ」の役（#2252）のモックテスト
#
# 夜間を 2 台で回すとき、リリースしない側を `scripts/nightly-release.sh --role check` で「検査だけ」に
# する。この役では mod の検査（#1892）と footprint の見張り（#2023）だけが回り、ビルド・push・タグ・
# GitHub Release・予約の消費は一切しない。役を設定しない（= 1 台の利用者）ときは今までどおり全部やる。
#
# 本番には一切触らない: origin は一時ディレクトリの bare リポ、HOME（ログ・ロック・役と予約の
# 状態ファイル・LaunchAgents）も一時ディレクトリ、release.sh・mod の検査・footprint の見張り・cargo・
# gh・osascript・launchctl は呼ばれたことを記録するだけの代役。本物の claude も GUI も起こさない。
#
#   A. 役なし（既定）: 今までどおり全部の段が走る（ビルド → push → タグ → Release、検査 2 つ）
#   B. --role check: 検査 2 つだけが走り、ビルド・push・タグ・Release・cargo・gh は一度も呼ばれない
#      （A と同じ origin の状態から始めて、origin の main とタグが 1 ビットも動かない）
#   C. 役の境界: タグが 1 つも無い（全部なら exit 1）/ 共有ツリーが dirty / 予約がある（消費しない）/
#      --dry-run / 多重起動
#   D. 役の CLI: 表示（既定 / 検査だけ）・--role full で既定へ戻す・不正な値は exit 2・
#      壊れた値は既定の全部で回して知らせる・--reserve の注意・--install-launchd は plist を変えない
#
# ここが落ちるということは、「検査だけ」の PC がリリースを出す（2 台が同じタグを取り合う）か、
# 検査が回らなくなるか、何も設定しない 1 台の利用者の夜間が変わったという意味。
# launchd と同じ /bin/bash（3.2）で走らせる。
set -uo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
PASS=0
FAIL=0
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tako-role-test-XXXXXX")"
trap 'rm -rf "$TMP_ROOT"' EXIT

ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (expected=[$2], actual=[$3])"; fi; }
# pipefail の下で printf | grep -q は grep が先に抜けると printf の SIGPIPE で偽の不一致になるので、
# ヒアストリングで渡す（パイプを作らない）
assert_contains() { if grep -qF -- "$3" <<< "$2"; then ok "$1"; else ng "$1 (not found: '$3')"; fi; }
assert_not_contains() { if grep -qF -- "$3" <<< "$2"; then ng "$1 (found but should not: '$3')"; else ok "$1"; fi; }

# ---- 夜間の環境（一時 dir の origin + 共有ツリー + HOME）------------------------
# <dir> <変更を積むか 1/0> <タグを打つか 1/0>
make_env() {
  local dir="$1" change="$2" tag="$3" stub
  mkdir -p "$dir/home/.local/bin" "$dir/repo/scripts/lib"
  # 代役は呼ばれた引数を $HOME/calls.log へ 1 行ずつ残す（夜間は PATH の先頭に $HOME/.local/bin を置く）
  for stub in cargo gh osascript launchctl; do
    printf '#!/bin/sh\necho "%s $*" >> "$HOME/calls.log"\nexit 0\n' "$stub" > "$dir/home/.local/bin/$stub"
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
  command cp -f "$REPO_ROOT/scripts/nightly-release.sh" "$dir/repo/scripts/"
  command cp -f "$REPO_ROOT/scripts/lib/nightly-reserve.sh" "$REPO_ROOT/scripts/lib/exit-guard.sh" "$dir/repo/scripts/lib/"
  # release.sh（ビルド / Release）は使い捨ての worktree の中で呼ばれるので、記録は $HOME へ
  printf '#!/bin/sh\necho "release.sh $*" >> "$HOME/calls.log"\nexit 0\n' > "$dir/repo/scripts/release.sh"
  chmod +x "$dir/repo/scripts/release.sh"
  printf '/target\n/dist\n/scripts/check-claude-mod.sh\n/scripts/nightly-footprint-watch.sh\n' > "$dir/repo/.gitignore"
  printf '[workspace.package]\nversion = "0.8.40"\n' > "$dir/repo/Cargo.toml"
  printf '# Changelog\n\n## [0.8.40] - 2026-01-01\n\nInitial\n' > "$dir/repo/CHANGELOG.md"
  printf '# dummy lock\n' > "$dir/repo/Cargo.lock"
  git -C "$dir/repo" add -A
  git -C "$dir/repo" commit --quiet -m "init"
  [ "$tag" = 1 ] && git -C "$dir/repo" tag -a v0.8.40 -m v0.8.40
  git -C "$dir/repo" push --quiet origin HEAD:main
  [ "$tag" = 1 ] && git -C "$dir/repo" push --quiet origin v0.8.40
  if [ "$change" = 1 ]; then
    echo change > "$dir/repo/NOTES.md"
    git -C "$dir/repo" add -A
    git -C "$dir/repo" commit --quiet -m "[改善] 変更"
    git -C "$dir/repo" push --quiet origin HEAD:main
  fi
  # mod の検査の代役（夜間は install_root の check-claude-mod.sh を呼ぶ）: 引数を残して合格を返す
  cat > "$dir/repo/scripts/check-claude-mod.sh" <<'SH'
#!/bin/bash
echo "mod-check $*" >> "$HOME/calls.log"
echo "結果: 合格（代役）"
exit 0
SH
  # footprint の見張りの代役: 呼ばれた時点でロックが残っているかと origin のタグを残す
  cat > "$dir/repo/scripts/nightly-footprint-watch.sh" <<'SH'
#!/bin/bash
here="$(cd "$(dirname "$0")/.." && pwd)"
held=free
[ -d "$HOME/.claude-orchestrator/locks/tako-nightly-release.lock" ] && held=held
[ -d "$here/.git/tako-nightly-release.lock" ] && held=held
echo "footprint-watch locks=${held} new_tags=$(git -C "$here" ls-remote --tags origin 2>/dev/null | grep -c 'refs/tags/v0.8.41$' || true)" >> "$HOME/calls.log"
echo "代役の見張り"
exit 0
SH
  chmod +x "$dir/repo/scripts/check-claude-mod.sh" "$dir/repo/scripts/nightly-footprint-watch.sh"
}

RC=0
# <dir> [env 名=値 …] [-- 夜間の引数]。launchd と同じく env を空にして /bin/bash で走らせる
run_nightly() {
  local dir="$1"
  shift
  local -a envs=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  RC=0
  env -i HOME="$dir/home" ${envs[@]+"${envs[@]}"} /bin/bash "$dir/repo/scripts/nightly-release.sh" "$@" > "$dir/out.txt" 2>&1 || RC=$?
}
calls() { cat "$1/home/calls.log" 2>/dev/null || true; }
count_calls() { # <dir> <先頭の語>
  local n
  n=$(grep -c "^$2 " "$1/home/calls.log" 2>/dev/null) || true
  echo "${n:-0}"
}
nightly_log() { cat "$1/home/.claude-orchestrator/logs/tako-nightly-release.log" 2>/dev/null || true; }
origin_main() { git -C "$1/origin.git" rev-parse refs/heads/main; }
origin_tags() { git -C "$1/origin.git" tag --list | tr '\n' ' '; }
role_file() { echo "$1/home/.claude-orchestrator/state/tako-nightly-role"; }
role_cmd() { env -i HOME="$1/home" /bin/bash "$1/repo/scripts/nightly-release.sh" --role "$2"; }

# ============================================================================
echo "A. 役なし（既定）: 今までどおり全部の段が走る"
# ============================================================================
D="$TMP_ROOT/a"; make_env "$D" 1 1
before_a="$(origin_main "$D")"
run_nightly "$D"
assert_eq "終了コード 0" "0" "$RC"
assert_eq "役の状態ファイルは無い（何も設定していない）" "no" "$([ -e "$(role_file "$D")" ] && echo yes || echo no)"
assert_eq "release.sh はビルドと Release の 2 回" "2" "$(count_calls "$D" release.sh)"
assert_contains "Release はテスト版" "$(calls "$D")" "release.sh --skip-build --test"
assert_eq "cargo（Cargo.lock の同期）が呼ばれた" "1" "$(count_calls "$D" cargo)"
assert_contains "origin にタグ v0.8.41" "$(origin_tags "$D")" "v0.8.41"
assert_eq "origin の main は進んだ" "yes" "$([ "$(origin_main "$D")" != "$before_a" ] && echo yes || echo no)"
assert_contains "origin の main の先端はリリースのコミット" "$(git -C "$D/origin.git" log -1 --format=%s main)" "[リリース] v0.8.41"
assert_eq "mod の検査は 1 回" "1" "$(count_calls "$D" mod-check)"
assert_contains "mod の検査は origin/main を記録つきで" "$(calls "$D")" "mod-check --ref origin/main --record"
assert_eq "footprint の見張りは 1 回" "1" "$(count_calls "$D" footprint-watch)"
assert_contains "見張りはロックの解放とリリースの後" "$(calls "$D")" "footprint-watch locks=free new_tags=1"
assert_contains "完了のログ" "$(nightly_log "$D")" "完了: v0.8.41"
assert_not_contains "役の行は出ない（既定の利用者のログは変わらない）" "$(nightly_log "$D")" "役:"

# ============================================================================
echo "B. --role check: 検査だけが走り、ビルド・push・タグ・Release は一切しない"
# ============================================================================
D="$TMP_ROOT/b"; make_env "$D" 1 1
out_set="$(role_cmd "$D" check)"
assert_contains "設定の応答" "$out_set" "役を設定しました: 検査だけ"
assert_eq "状態ファイルの値は check" "check" "$(grep -v '^#' "$(role_file "$D")")"
before_b="$(origin_main "$D")"
tags_before_b="$(origin_tags "$D")"
local_head_before="$(git -C "$D/repo" rev-parse HEAD)"
run_nightly "$D"
assert_eq "終了コード 0" "0" "$RC"
assert_eq "mod の検査は 1 回" "1" "$(count_calls "$D" mod-check)"
assert_contains "mod の検査は origin/main を記録つきで" "$(calls "$D")" "mod-check --ref origin/main --record"
assert_eq "footprint の見張りは 1 回" "1" "$(count_calls "$D" footprint-watch)"
assert_contains "見張りはロックの解放の後" "$(calls "$D")" "footprint-watch locks=free new_tags=0"
assert_eq "release.sh（ビルド / Release）は 0 回" "0" "$(count_calls "$D" release.sh)"
assert_eq "cargo は 0 回" "0" "$(count_calls "$D" cargo)"
assert_eq "gh は 0 回" "0" "$(count_calls "$D" gh)"
assert_eq "origin の main は動かない" "$before_b" "$(origin_main "$D")"
assert_eq "origin のタグは増えない" "$tags_before_b" "$(origin_tags "$D")"
assert_eq "手元にもタグ v0.8.41 は無い" "" "$(git -C "$D/repo" tag --list v0.8.41)"
assert_eq "共有ツリーの HEAD はブランチ main のまま" "main" "$(git -C "$D/repo" symbolic-ref --short HEAD 2>/dev/null)"
assert_eq "共有ツリーの HEAD は動かない" "$local_head_before" "$(git -C "$D/repo" rev-parse HEAD)"
assert_eq "使い捨ての worktree を作っていない" "1" "$(git -C "$D/repo" worktree list | wc -l | tr -d ' ')"
assert_contains "役の行がログに出る" "$(nightly_log "$D")" "役: 検査だけ → リリースの段（ビルド・push・タグ・GitHub Release）は飛ばす"
assert_not_contains "リリースの判定の行は出ない" "$(nightly_log "$D")" "としてリリースする"
assert_eq "通知は出さない" "0" "$(count_calls "$D" osascript)"

echo "B2: 同じ環境で --role full へ戻すと、次の夜は全部の段が走る（A/B が同じ環境で切り替わる）"
: > "$D/home/calls.log"
out_full="$(role_cmd "$D" full)"
assert_contains "戻した応答" "$out_full" "役を設定しました: 全部"
assert_eq "状態ファイルは消える（何も設定していない 1 台と同じ）" "no" "$([ -e "$(role_file "$D")" ] && echo yes || echo no)"
run_nightly "$D"
assert_eq "終了コード 0" "0" "$RC"
assert_eq "release.sh は 2 回" "2" "$(count_calls "$D" release.sh)"
assert_contains "origin にタグ v0.8.41" "$(origin_tags "$D")" "v0.8.41"

# ============================================================================
echo "C. 検査だけの役の境界"
# ============================================================================
echo "C1: タグが 1 つも無い（全部の役なら exit 1）→ 検査だけの役はリリースの段に入らないので 0"
D="$TMP_ROOT/c1-full"; make_env "$D" 0 0
run_nightly "$D"
assert_eq "全部の役は exit 1（v* タグが無い）" "1" "$RC"
D="$TMP_ROOT/c1-check"; make_env "$D" 0 0
role_cmd "$D" check > /dev/null
run_nightly "$D"
assert_eq "検査だけの役は exit 0" "0" "$RC"
assert_not_contains "タグの検査に入らない" "$(nightly_log "$D")" "v* タグが 1 つも見つからない"
assert_eq "見張りは回る" "1" "$(count_calls "$D" footprint-watch)"

echo "C2: 共有ツリーが dirty でも検査は回り、スキップの通知は出さない"
D="$TMP_ROOT/c2"; make_env "$D" 1 1
role_cmd "$D" check > /dev/null
echo "作業中" >> "$D/repo/CHANGELOG.md"
run_nightly "$D"
assert_eq "終了コード 0" "0" "$RC"
assert_not_contains "dirty のスキップに入らない" "$(nightly_log "$D")" "SKIP: 共有ツリーが dirty"
assert_eq "mod の検査は回る" "1" "$(count_calls "$D" mod-check)"
assert_eq "見張りは回る" "1" "$(count_calls "$D" footprint-watch)"
assert_eq "通知は出さない" "0" "$(count_calls "$D" osascript)"

echo "C3: この PC に予約があっても使わず・消費もしない（警告だけ）"
D="$TMP_ROOT/c3"; make_env "$D" 1 1
role_cmd "$D" check > /dev/null
mkdir -p "$D/home/.claude-orchestrator/state"
printf '0.9.0\n' > "$D/home/.claude-orchestrator/state/tako-nightly-next-version"
run_nightly "$D"
assert_eq "終了コード 0" "0" "$RC"
assert_contains "予約を使わないと言う" "$(nightly_log "$D")" "予約 0.9.0 があるが、検査だけの役なので使わない"
assert_eq "予約ファイルは残る" "0.9.0" "$(cat "$D/home/.claude-orchestrator/state/tako-nightly-next-version")"
assert_eq "origin にタグ v0.9.0 は無い" "" "$(git -C "$D/origin.git" tag --list v0.9.0)"
out_res="$(env -i HOME="$D/home" /bin/bash "$D/repo/scripts/nightly-release.sh" --reserve)"
assert_contains "--reserve の表示に注意が出る" "$out_res" "検査だけの役（--role）なので、ここの予約は使われない"

echo "C4: --dry-run は検査だけの役でもリリースしない（見張りは dry-run なので回らない）"
D="$TMP_ROOT/c4"; make_env "$D" 1 1
role_cmd "$D" check > /dev/null
run_nightly "$D" -- --dry-run
assert_eq "終了コード 0" "0" "$RC"
assert_eq "release.sh は 0 回" "0" "$(count_calls "$D" release.sh)"
assert_eq "見張りは回らない" "0" "$(count_calls "$D" footprint-watch)"

echo "C5: 多重起動（ロックを生きた pid が持つ）では検査も見張りも回らない"
D="$TMP_ROOT/c5"; make_env "$D" 1 1
role_cmd "$D" check > /dev/null
mkdir -p "$D/home/.claude-orchestrator/locks/tako-nightly-release.lock"
sleep 30 &
holder=$!
echo "$holder" > "$D/home/.claude-orchestrator/locks/tako-nightly-release.lock/pid"
run_nightly "$D"
kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
assert_eq "終了コード 0" "0" "$RC"
assert_contains "多重起動で抜けた" "$(cat "$D/out.txt")" "SKIP: 多重起動"
assert_eq "mod の検査は回らない" "0" "$(count_calls "$D" mod-check)"
assert_eq "見張りは回らない" "0" "$(count_calls "$D" footprint-watch)"

# ============================================================================
echo "D. 役の CLI"
# ============================================================================
D="$TMP_ROOT/d"; make_env "$D" 1 1
echo "D1: 表示（既定 → 検査だけ）"
assert_contains "既定の表示" "$(env -i HOME="$D/home" /bin/bash "$D/repo/scripts/nightly-release.sh" --role)" "役: 全部（リリースと、mod の検査・footprint の見張り）。既定"
role_cmd "$D" check > /dev/null
assert_contains "検査だけの表示" "$(env -i HOME="$D/home" /bin/bash "$D/repo/scripts/nightly-release.sh" --role)" "役: 検査だけ"

echo "D2: 不正な値は exit 2 で、今の設定を変えない"
rc=0
err="$(role_cmd "$D" release 2>&1)" || rc=$?
assert_eq "exit 2" "2" "$rc"
assert_contains "選べる値を言う" "$err" "役は check（検査だけ）か full（全部）"
assert_eq "設定は check のまま" "check" "$(grep -v '^#' "$(role_file "$D")")"

echo "D3: 手で書き壊した値は、止めずに既定の全部で回し、ログと通知で知らせる"
D="$TMP_ROOT/d3"; make_env "$D" 1 1
mkdir -p "$D/home/.claude-orchestrator/state"
printf 'chek\n' > "$(role_file "$D")"
run_nightly "$D"
assert_eq "終了コード 0" "0" "$RC"
assert_contains "読めないと言う" "$(nightly_log "$D")" "WARN: 夜間の役の値が読めない（chek）。既定の全部で回す"
assert_contains "通知する" "$(calls "$D")" "役の値が読めない"
assert_eq "全部の役で回った（release.sh 2 回）" "2" "$(count_calls "$D" release.sh)"
assert_contains "--role の表示も無効と言う" "$(env -i HOME="$D/home" /bin/bash "$D/repo/scripts/nightly-release.sh" --role)" "役: **無効な値** chek"

echo "D4: --install-launchd は役を plist に書かず（作り直しても役は変わらない）、今の役を言う"
D="$TMP_ROOT/d4"; make_env "$D" 1 1
role_cmd "$D" check > /dev/null
# 本物の launchctl に一時の plist を読ませない: 夜間の PATH の行をそのまま評価して、
# launchctl が代役に解決されることを確かめてからでなければ --install-launchd を走らせない
night_path="$(sed -n 's/^export PATH="\(.*\)"$/\1/p' "$REPO_ROOT/scripts/nightly-release.sh" | head -1)"
resolved="$(env -i HOME="$D/home" /bin/bash -c "PATH=\"${night_path}\"; command -v launchctl")"
if [ -n "$night_path" ] && [ "$resolved" = "$D/home/.local/bin/launchctl" ]; then
  out_inst="$(env -i HOME="$D/home" /bin/bash "$D/repo/scripts/nightly-release.sh" --install-launchd 2>&1)"
  assert_contains "今の役を言う" "$out_inst" "この PC の役: 検査だけ"
  plist="$D/home/Library/LaunchAgents/com.takushio.tako-nightly-release.plist"
  assert_eq "plist は作られた（launchctl は代役）" "yes" "$([ -f "$plist" ] && echo yes || echo no)"
  assert_eq "役は check のまま" "check" "$(grep -v '^#' "$(role_file "$D")")"
  assert_contains "launchctl は代役が受けた" "$(calls "$D")" "launchctl load"
  # 役を既定へ戻して作り直しても plist は 1 バイトも変わらない（= 役は plist に入らない）
  command cp -f "$plist" "$D/plist.check"
  role_cmd "$D" full > /dev/null
  out_inst2="$(env -i HOME="$D/home" /bin/bash "$D/repo/scripts/nightly-release.sh" --install-launchd 2>&1)"
  assert_contains "既定の役を言う" "$out_inst2" "この PC の役: 全部"
  assert_eq "役で plist は変わらない" "same" "$(cmp -s "$plist" "$D/plist.check" && echo same || echo differ)"
else
  ng "夜間の PATH で launchctl が代役に解決されない（${resolved:-なし}）ので --install-launchd を走らせない"
fi

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
