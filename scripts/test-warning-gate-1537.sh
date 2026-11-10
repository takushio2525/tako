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
#   A2 / B2. clippy の段（#2850）: 偽の cargo で見出し・直し方・注釈を clippy の指摘として言う / 本物の
#      clippy で本体とテストの cfg の内側の指摘を拾い、済んだ単位は検査し直さずに数える
#   C. scripts/check-windows.sh の範囲の指定（#2851）: 偽の cargo / rustup / brew の砂場で、実物の
#      スクリプトが cargo へ渡す引数を固定する。引数なしは 3 つの宇宙のまま / `-p` `--package`
#      `--package=` `-p<名>` では `--workspace` を足さない / 自分で渡した `--workspace` に重ねない /
#      値の無い `-p` は cargo を起こさずに 2 / 絞った範囲でもゲートが数えて 3 で名指す
# 配線（CI の Windows ジョブと scripts/check-windows.sh がこのゲートを通すこと）は
# Rust の番犬 crates/tako-control/tests/ci_windows_test_compile.rs が見る
# 検出力: 色を切らない写しのゲートは、色つきの CI と同じ条件で警告を見落とすことを確かめる
set -uo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
GATE="${REPO_ROOT}/scripts/warning-gate.sh"
# CI（GitHub Actions）の中で走ると GITHUB_ACTIONS=true を継ぐ。外の値は外し、
# 注釈の有無は下で明示的に付けたときだけ確かめる
unset GITHUB_ACTIONS

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

# shellcheck source=lib/fake-exec.sh
. "${REPO_ROOT}/scripts/lib/fake-exec.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP:?}"' EXIT

# 偽の cargo: $FAKE_OUT の中身を出し、$FAKE_RC で終わる
FAKE="${TMP}/fake-cargo"
cat >"$FAKE" <<'EOF'
#!/usr/bin/env bash
cat "$FAKE_OUT"
exit "${FAKE_RC:-0}"
EOF
fake_exec_warm "$FAKE"

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

echo "A2. clippy の段（#2850。偽の cargo）"
run_fake_clippy() { # run_fake と同じ。cargo の副コマンドだけ clippy
  local file="$1" rc="$2"
  shift 2
  out="$(env "$@" FAKE_OUT="$file" FAKE_RC="$rc" bash "$GATE" "$FAKE" clippy --workspace --all-targets 2>&1)"
  code=$?
}
printf '%s\r\n' \
  '    Checking tako-control v0.8.40 (C:\a\tako\tako\crates\tako-control)' \
  'warning: unneeded `return` statement' \
  '  --> crates\tako-control\src\fda.rs:20:9' \
  '   |' \
  '20 |         return true;' \
  '   |' \
  '   = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.95.0/index.html#needless_return' \
  '   = note: `#[warn(clippy::needless_return)]` on by default' \
  '' \
  'warning: `tako-control` (lib) generated 1 warning (run `cargo clippy --fix --lib -p tako-control` to apply 1 suggestion)' \
  'warning: `tako-control` (lib test) generated 1 warning (1 duplicate)' \
  '    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.00s' >"${TMP}/clippy.txt"
run_fake_clippy "${TMP}/clippy.txt" 0
assert_eq "clippy の指摘があれば終了コード 3" "$code" "3"
assert_has "clippy の指摘として件数を言う（重複は引く）" "clippy ゲート（#2850）: 自分のクレートに clippy の指摘（コンパイラの警告を含む）が 1 件あります" "$out"
assert_has "clippy の指摘を名指す" "crates/tako-control/src/fda.rs:20:9: unneeded \`return\` statement" "$out"
assert_has "clippy の直し方を言う" "#[allow(clippy::…)]" "$out"
assert_lacks "コンパイラの警告の直し方を言わない" "allow(dead_code)" "$out"
run_fake_clippy "${TMP}/clippy.txt" 0 GITHUB_ACTIONS=true
assert_has "clippy の注釈は clippy ゲートの名で出す" "::error file=crates/tako-control/src/fda.rs,line=20,col=9::[#2850 clippy ゲート] unneeded \`return\` statement" "$out"
run_fake_clippy "${TMP}/clean.txt" 0
assert_eq "clippy の指摘 0 件は 0" "$code" "0"
assert_has "clippy の指摘は 0 件と言う" "clippy ゲート（#2850）: 自分のクレートの clippy の指摘は 0 件" "$out"
run_fake "${TMP}/clippy.txt" 0
assert_has "build の段は従来どおり警告ゲートの名で言う" "警告ゲート（#1537）: 自分のクレートにコンパイラの警告が 1 件あります" "$out"

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

  echo "B2. clippy の段（#2850。本物の clippy）"
  # リポジトリと同じ版の clippy で見る（一時ディレクトリにはリポジトリの rust-toolchain.toml が効かない）
  CL="${TMP}/probe-clippy"
  mkdir -p "${CL}/src"
  command cp -f "${REPO_ROOT}/rust-toolchain.toml" "${CL}/rust-toolchain.toml"
  printf '[package]\nname = "probe2850"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n' >"${CL}/Cargo.toml"
  cat >"${CL}/src/lib.rs" <<'EOF'
pub fn used(x: u32) -> u32 {
    return x + 1;
}

#[cfg(test)]
mod tests {
    fn twice_2850(x: u32) -> u32 {
        return x * 2;
    }

    #[test]
    fn t() {
        assert_eq!(super::used(twice_2850(1)), 3);
    }
}
EOF
  export CARGO_TARGET_DIR="${CL}/target"
  if ! (cd "$CL" && cargo clippy --version >/dev/null 2>&1); then
    ng "clippy がある" "rust-toolchain.toml の版に clippy が無い"
  else
    out="$(cd "$CL" && bash "$GATE" cargo clippy --offline 2>&1)"
    code=$?
    assert_eq "clippy: 本体の指摘で 3" "$code" "3"
    assert_has "clippy: 本体の指摘を名指す" "src/lib.rs:2:5: unneeded \`return\` statement" "$out"
    assert_lacks "clippy: --all-targets なしではテストの cfg の内側を見ない" "src/lib.rs:8:9" "$out"
    out="$(cd "$CL" && bash "$GATE" cargo clippy --offline --all-targets 2>&1)"
    code=$?
    assert_eq "clippy --all-targets: 3" "$code" "3"
    assert_has "clippy --all-targets: テストの cfg の内側だけの指摘を名指す" "src/lib.rs:8:9: unneeded \`return\` statement" "$out"
    out="$(cd "$CL" && CARGO_TERM_COLOR=always bash "$GATE" cargo clippy --offline --all-targets 2>&1)"
    code=$?
    assert_eq "clippy: 済んだ単位に当てても 3" "$code" "3"
    assert_lacks "clippy: 済んだ単位は検査し直さない" "Checking probe2850" "$out"
    assert_has "clippy: 再生でも本体の指摘を名指す" "src/lib.rs:2:5: unneeded \`return\` statement" "$out"
    printf 'pub fn used(x: u32) -> u32 {\n    x + 1\n}\n' >"${CL}/src/lib.rs"
    out="$(cd "$CL" && bash "$GATE" cargo clippy --offline --all-targets 2>&1)"
    assert_eq "clippy: 指摘の無いクレートは 0" "$?" "0"
  fi
  unset CARGO_TARGET_DIR
fi

echo "C. check-windows.sh の範囲の指定（#2851。偽の cargo / rustup / brew の砂場）"
# 実物の check-windows.sh と warning-gate.sh を一時のリポジトリの根へ写し、`cargo xwin` へ渡った引数を
# 1 回 1 行で記録する。PWA のビルドは何もしない偽物に差し替える（npm を起こさない）
CW="${TMP}/cw"
CW_LOG="${TMP}/cw-calls.txt"
mkdir -p "${CW}/scripts" "${CW}/bin" "${CW}/llvm/bin" "${CW}/gpui"
command cp -f "${REPO_ROOT}/scripts/check-windows.sh" "${REPO_ROOT}/scripts/warning-gate.sh" "${CW}/scripts/"
printf '#!/usr/bin/env bash\nexit 0\n' >"${CW}/scripts/build-pwa.sh"
printf '#!/usr/bin/env bash\nexit 0\n' >"${CW}/llvm/bin/clang-cl"
printf '#!/usr/bin/env bash\nexit 0\n' >"${CW}/bin/cargo-xwin"
printf '#!/usr/bin/env bash\necho x86_64-pc-windows-msvc\n' >"${CW}/bin/rustup"
printf '#!/usr/bin/env bash\necho "%s"\n' "${CW}/llvm" >"${CW}/bin/brew"
cat >"${CW}/bin/cargo" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  metadata)
    printf '{"packages":[{"name":"gpui","manifest_path":"%s/Cargo.toml"}]}\n' "$CW_GPUI"
    exit 0
    ;;
  xwin)
    shift
    printf '%s\n' "$*" >>"$CW_LOG"
    cat "$FAKE_OUT"
    exit "${FAKE_RC:-0}"
    ;;
esac
echo "偽の cargo: 知らない副コマンド: $*" >&2
exit 99
EOF
fake_exec_warm "${CW}/scripts/build-pwa.sh" "${CW}/llvm/bin/clang-cl" "${CW}/bin/cargo-xwin" \
  "${CW}/bin/rustup" "${CW}/bin/brew" "${CW}/bin/cargo"

cw_run() { # $1 = 偽の cargo の出力, $2 = 終了コード, 残り = check-windows.sh の引数
  local file="$1" rc="$2"
  shift 2
  : >"$CW_LOG"
  out="$(env PATH="${CW}/bin:${PATH}" CW_LOG="$CW_LOG" CW_GPUI="${CW}/gpui" \
    FAKE_OUT="$file" FAKE_RC="$rc" bash "${CW}/scripts/check-windows.sh" "$@" 2>&1)"
  code=$?
  calls="$(cat "$CW_LOG")"
}
W="clippy --target x86_64-pc-windows-msvc"

cw_run "${TMP}/clean.txt" 0
assert_eq "引数なし: 3 つの宇宙を修正前と同じ引数で回す" "$calls" "$(printf '%s\n' \
  "$W --workspace --all-targets" \
  "$W -p tako-app --features visual-test --all-targets" \
  "$W -p tako-core -p tako-control -p tako-cli --all-targets")"
assert_eq "引数なし: 指摘 0 件は 0" "$code" "0"

cw_run "${TMP}/clean.txt" 0 -p tako-core
assert_eq "-p <名>: --workspace を足さない" "$calls" "$W --all-targets -p tako-core"
assert_lacks "-p だけなら同時指定の注意を出さない" "全メンバーを検査します" "$out"
cw_run "${TMP}/clean.txt" 0 --package tako-core
assert_eq "--package <名>: --workspace を足さない" "$calls" "$W --all-targets --package tako-core"
cw_run "${TMP}/clean.txt" 0 --package=tako-core
assert_eq "--package=<名>: --workspace を足さない" "$calls" "$W --all-targets --package=tako-core"
cw_run "${TMP}/clean.txt" 0 -ptako-core
assert_eq "-p<名>（くっつけた形）: --workspace を足さない" "$calls" "$W --all-targets -ptako-core"
cw_run "${TMP}/clean.txt" 0 -p tako-core -p tako-cli
assert_eq "-p を 2 つ: --workspace を足さない" "$calls" "$W --all-targets -p tako-core -p tako-cli"

cw_run "${TMP}/clean.txt" 0 --lib
assert_eq "範囲の指定が無い追加の引数: 従来どおり --workspace を足す" "$calls" "$W --workspace --all-targets --lib"
cw_run "${TMP}/clean.txt" 0 --exclude tako-app
assert_eq "--exclude（--workspace が要る）: --workspace を足す" "$calls" "$W --workspace --all-targets --exclude tako-app"
cw_run "${TMP}/clean.txt" 0 --workspace
assert_eq "自分で渡した --workspace に重ねない（cargo は 2 つ目を断る）" "$calls" "$W --all-targets --workspace"
cw_run "${TMP}/clean.txt" 0 --all
assert_eq "旧名の --all にも --workspace を重ねない" "$calls" "$W --all-targets --all"
cw_run "${TMP}/clean.txt" 0 --workspace -p tako-core
assert_eq "--workspace と -p の同時指定: 渡されたまま cargo へ" "$calls" "$W --all-targets --workspace -p tako-core"
assert_has "--workspace と -p の同時指定: cargo が全メンバーを検査すると言う" "全メンバーを検査します" "$out"

cw_run "${TMP}/clippy.txt" 0 -p tako-control
assert_eq "-p で絞っても指摘があれば 3" "$code" "3"
assert_has "-p で絞っても clippy ゲートが数える" "clippy ゲート（#2850）: 自分のクレートに clippy の指摘（コンパイラの警告を含む）が 1 件あります" "$out"
assert_has "-p で絞っても位置を名指す" "crates/tako-control/src/fda.rs:20:9: unneeded \`return\` statement" "$out"
cw_run "${TMP}/clippy.txt" 0 -p tako-core --message-format=json
assert_eq "-p と --message-format: ゲートを通さず cargo の終了コード" "$code" "0"
assert_lacks "-p と --message-format: ゲートの判定を出さない" "clippy ゲート（#2850）" "$out"
assert_eq "-p と --message-format: --workspace を足さない" "$calls" "$W --all-targets -p tako-core --message-format=json"
# 値の無い -p は cargo-xwin が黙って捨てて全メンバーを検査する（2026-11-10 実測）ので、cargo を起こさずに止める
cw_run "${TMP}/clean.txt" 0 -p
assert_eq "-p の値が無い（最後の引数）: 使い方の誤りの 2" "$code" "2"
assert_eq "-p の値が無い: cargo を起こさない" "$calls" ""
assert_has "-p の値が無い: クレート名が無いと言う" "-p / --package の後にクレート名がありません" "$out"
cw_run "${TMP}/clean.txt" 0 --lib --package
assert_eq "--package の値が無い（最後の引数）: 2" "$code" "2"
cw_run "${TMP}/clean.txt" 0 -p --lib
assert_eq "-p の次が別のオプション: 2" "$code" "2"
assert_eq "-p の次が別のオプション: cargo を起こさない" "$calls" ""
cw_run "${TMP}/error.txt" 101 -p nosuch
assert_eq "存在しないクレート名: cargo の断り（終了コード）をそのまま返す" "$code" "101"
assert_eq "存在しないクレート名: --workspace で埋めない" "$calls" "$W --all-targets -p nosuch"

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
