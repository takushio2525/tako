#!/usr/bin/env bash
# tako:run: bash scripts/test-warning-gate-1537.sh
# test-warning-gate-1537.sh — 警告ゲート scripts/warning-gate.sh のテスト（#1537 / #1827）
#
# 一時ディレクトリの偽の cargo（決まった出力と終了コードを返すだけ）と、一時ディレクトリに作る
# 小さなクレートに対する本物の cargo の両方で、実物の scripts/warning-gate.sh を走らせる。
# リポジトリの target にも本番の GUI にも触らない。
#
# 固定しているもの:
#   A. 偽の cargo: Windows の出力（CRLF・`\` の位置）の警告を名指して 3 / 0 件は 0 / 重複は引く /
#      コンパイルエラーはコマンドの終了コード / build script の `cargo:warning=` は数えない /
#      位置の分からない警告も数える / GitHub Actions では注釈を出す / --message-format は断る
#   B. 本物の cargo: 本体の警告・テストの cfg の内側だけの警告（`cargo test --no-run`）を拾う・
#      済んだビルドに当てても再コンパイル無しで数える（色つきで作った後でも）・警告の無いクレートは 0
# 配線（CI の Windows ジョブと scripts/check-windows.sh がこのゲートを通すこと）は
# Rust の番犬 crates/tako-control/tests/ci_windows_test_compile.rs が見る
# 検出力: 色を切らない写しのゲートは、色つきの CI と同じ条件で警告を見落とすことを確かめる
set -uo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
GATE="${REPO_ROOT}/scripts/warning-gate.sh"

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
assert_lacks() { if grep -qF -- "$2" <<<"$3"; then ng "$1" "出力にある: $2"; else ok "$1"; fi; }

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP:?}"' EXIT

# 偽の cargo: $FAKE_OUT の中身を出し、$FAKE_RC で終わる
FAKE="${TMP}/fake-cargo"
cat >"$FAKE" <<'EOF'
#!/usr/bin/env bash
cat "$FAKE_OUT"
exit "${FAKE_RC:-0}"
EOF
chmod +x "$FAKE"

run_fake() { # $1 = 出力のファイル, $2 = 終了コード, 残り = ゲートへの追加の env（VAR=値）
  local file="$1" rc="$2"
  shift 2
  out="$(env "$@" FAKE_OUT="$file" FAKE_RC="$rc" bash "$GATE" "$FAKE" build 2>&1)"
  code=$?
}

echo "A. 偽の cargo"

# Windows の出力: CRLF・`\` の位置・lib と lib test の 2 単位
printf '%s\r\n' \
  '   Compiling tako-core v0.8.39 (C:\a\tako\tako\crates\tako-core)' \
  'warning: function `temp_dir` is never used' \
  '    --> crates\tako-core\src\platform\shell.rs:1810:4' \
  '     |' \
  '1810 | fn temp_dir() -> PathBuf {' \
  '     |    ^^^^^^^^' \
  '     |' \
  '     = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default' \
  '' \
  'warning: unused import: `std::os::unix::fs::PermissionsExt`' \
  '  --> crates\tako-core\tests\issue1_x.rs:3:5' \
  '   |' \
  'note: the lint level is defined here' \
  '  --> crates\tako-core\src\lib.rs:1:9' \
  '' \
  'warning: `tako-core` (lib) generated 1 warning' \
  'warning: `tako-core` (test "issue1_x") generated 1 warning (run `cargo fix --test "issue1_x"` to apply 1 suggestion)' \
  '    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.00s' >"${TMP}/win.txt"
run_fake "${TMP}/win.txt" 0
assert_eq "警告があれば終了コード 3" "$code" "3"
assert_has "件数を言う" "警告が 2 件あります" "$out"
assert_has "位置は / へ寄せて名指す（本体）" "crates/tako-core/src/platform/shell.rs:1810:4: function \`temp_dir\` is never used" "$out"
assert_has "位置は / へ寄せて名指す（テスト）" "crates/tako-core/tests/issue1_x.rs:3:5: unused import" "$out"
assert_lacks "note の位置を警告と取り違えない" "crates/tako-core/src/lib.rs:1:9" "$out"
assert_has "cargo の出力はそのまま流す" "1810 | fn temp_dir() -> PathBuf {" "$out"

run_fake "${TMP}/win.txt" 0 GITHUB_ACTIONS=true
assert_has "GitHub Actions では注釈を出す" "::error file=crates/tako-core/src/platform/shell.rs,line=1810,col=4::[#1537 警告ゲート] function \`temp_dir\` is never used" "$out"
run_fake "${TMP}/win.txt" 0
assert_lacks "GitHub Actions の外では注釈を出さない" "::error" "$out"

printf '%s\n' \
  '   Compiling tako-core v0.8.39 (/r/crates/tako-core)' \
  'warning: tako-app@0.8.39: build script からの案内' \
  '    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.00s' >"${TMP}/clean.txt"
run_fake "${TMP}/clean.txt" 0
assert_eq "警告 0 件（build script の cargo:warning は数えない）は 0" "$code" "0"
assert_has "0 件と言う" "自分のクレートの警告は 0 件" "$out"

printf '%s\n' \
  'warning: unused variable: `x`' \
  ' --> crates/a/src/lib.rs:2:9' \
  'error[E0425]: cannot find value `y` in this scope' \
  ' --> crates/a/src/lib.rs:3:5' \
  'warning: build failed, waiting for other jobs to finish...' \
  'error: could not compile `a` (lib) due to 1 previous error; 1 warning emitted' >"${TMP}/error.txt"
run_fake "${TMP}/error.txt" 101
assert_eq "コンパイルエラーはコマンドの終了コード" "$code" "101"
assert_lacks "エラーのときは警告の判定を出さない" "警告ゲート（#1537）" "$out"

# 同じ警告が lib と lib test の両方で出ると、cargo は 2 度目を表示せず (N duplicates) と数える
printf '%s\n' \
  'warning: function `f` is never used' \
  ' --> crates/a/src/lib.rs:5:4' \
  '' \
  'warning: `a` (lib) generated 1 warning' \
  'warning: unused import: `x`' \
  ' --> crates/a/src/lib.rs:9:5' \
  '' \
  'warning: `a` (lib test) generated 2 warnings (1 duplicate) (run `cargo fix --lib -p a --tests` to apply 1 suggestion)' \
  'warning: `a` (test "t") generated 3 warnings (3 duplicates)' >"${TMP}/dup.txt"
run_fake "${TMP}/dup.txt" 0
assert_eq "重複だけの単位があっても警告は 3" "$code" "3"
assert_has "重複を引いて数える（表示された 2 件）" "警告が 2 件あります" "$out"

printf '%s\n' \
  'warning: some lint without a span' \
  '' \
  'warning: `a` (lib) generated 1 warning' >"${TMP}/nospan.txt"
run_fake "${TMP}/nospan.txt" 0
assert_eq "位置の分からない警告も数える" "$code" "3"
assert_has "位置の分からない件数を言う" "位置の分からない警告が 1 件" "$out"

out="$(bash "$GATE" "$FAKE" build --message-format=json 2>&1)"
assert_eq "--message-format は断る" "$?" "2"
out="$(bash "$GATE" 2>&1)"
assert_eq "コマンドが無ければ使い方の誤り" "$?" "2"

echo "B. 本物の cargo（一時ディレクトリの小さなクレート）"
if ! command -v cargo >/dev/null 2>&1; then
  ng "cargo がある" "PATH に cargo が無い"
else
  CRATE="${TMP}/probe"
  mkdir -p "${CRATE}/src"
  printf '[package]\nname = "probe1537"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n' >"${CRATE}/Cargo.toml"
  cat >"${CRATE}/src/lib.rs" <<'EOF'
pub fn used() {}

fn never_used_1537() {}

#[cfg(test)]
mod tests {
    fn only_in_test_1537() {}

    #[test]
    fn t() {
        super::used();
    }
}
EOF
  export CARGO_TARGET_DIR="${CRATE}/target"
  # 色つきで作る（CI の CARGO_TERM_COLOR: always と同じ）。ゲートの外なのでここは数えない
  (cd "$CRATE" && CARGO_TERM_COLOR=always cargo build --offline >/dev/null 2>&1 &&
    CARGO_TERM_COLOR=always cargo test --offline --no-run >/dev/null 2>&1)
  out="$(cd "$CRATE" && CARGO_TERM_COLOR=always bash "$GATE" cargo build --offline 2>&1)"
  code=$?
  assert_eq "本体の警告: 済んだビルドに当てても 3" "$code" "3"
  assert_has "本体の警告を名指す" "src/lib.rs:3:4: function \`never_used_1537\` is never used" "$out"
  assert_lacks "済んだビルドは再コンパイルしない" "Compiling probe1537" "$out"
  out="$(cd "$CRATE" && CARGO_TERM_COLOR=always bash "$GATE" cargo test --offline --no-run 2>&1)"
  code=$?
  assert_eq "テストの cfg の内側の警告: 3" "$code" "3"
  assert_has "テストの cfg の内側だけの警告を名指す" "function \`only_in_test_1537\` is never used" "$out"
  # 検出力: 色を切らない写しは、色つきの CI と同じ条件で警告を見落とす
  sed 's/^CARGO_TERM_COLOR=never "\$@"/"$@"/' "$GATE" >"${TMP}/gate-nocolor.sh"
  if grep -q '^CARGO_TERM_COLOR=never' "${TMP}/gate-nocolor.sh"; then
    ng "検出力の写しを作れる" "色を切る行を外せなかった"
  else
    out="$(cd "$CRATE" && CARGO_TERM_COLOR=always bash "${TMP}/gate-nocolor.sh" cargo build --offline 2>&1)"
    assert_eq "検出力: 色を切らない写しは見落とす（だから切る）" "$?" "0"
  fi
  printf 'pub fn used() {}\n' >"${CRATE}/src/lib.rs"
  out="$(cd "$CRATE" && bash "$GATE" cargo build --offline 2>&1)"
  assert_eq "警告の無いクレートは 0" "$?" "0"
  unset CARGO_TARGET_DIR
fi

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
