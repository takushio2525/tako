#!/usr/bin/env bash
# test-bundle-privacy-1848.sh — 配布物の個人情報チェックと xcrun の包みのテスト（#1848）
#
# 一時ディレクトリに ad-hoc 署名した偽の tako.app を作り、偽の HOME（/Users/testuser）・
# 偽のフルネーム・偽の証明書名義を注入して見る。実機のホーム・名前・キーチェーンには触らない。
# 見ているのは 2 つ:
#   A. scripts/lib/bundle-privacy.sh の check_bundle_privacy
#      1. 何も入っていなければ通る（汎用のログイン名は語にしない）
#      2. ホームパス・ログイン名・フルネーム・TAKO_PII_TERMS の語が中身にあれば落ち、場所と件数を出す
#      3. 署名が個人の開発用証明書、または名義に語が入っていれば落ちる
#      4. どの失敗でも、見つけた値そのものを出力へ書き戻さない（伏せ字）
#      5. 署名が無い .app・存在しないパスでは通さない
#   B. scripts/lib/xcrun-remap/xcrun（本物の xcrun はスタブへ差し替えて引数と cwd を記録する）
#      metal には -ffile-prefix-map を足し、metallib は出力先へ移って名前だけで呼び、
#      それ以外と付け替えの指定が無いときは素通しする。Metal の toolchain があれば実物でも確かめる
set -uo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
PASS=0
FAIL=0
ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1 ($2)"; FAIL=$((FAIL + 1)); }
assert_eq() { if [[ "$2" == "$3" ]]; then ok "$1"; else ng "$1" "期待 '$3' / 実際 '$2'"; fi; }
assert_has() { if grep -qF -- "$2" <<<"$3"; then ok "$1"; else ng "$1" "見つからない: $2"; fi; }
assert_lacks() { if grep -qF -- "$2" <<<"$3"; then ng "$1" "出てはいけない値が出ている"; else ok "$1"; fi; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

FAKE_HOME=/Users/testuser
FAKE_FULLNAME="山田 太郎"
FAKE_CERT="Apple Development: 山田 太郎 (ABCDE12345)"

# 偽の id（フルネームを差し替える）と、Authority を足せる codesign の包み
mkdir -p "$TMP/bin"
cat >"$TMP/bin/id" <<EOF
#!/bin/bash
if [ "\${1:-}" = "-F" ]; then echo "$FAKE_FULLNAME"; exit 0; fi
exec /usr/bin/id "\$@"
EOF
cat >"$TMP/bin/codesign" <<'EOF'
#!/bin/bash
# FAKE_AUTHORITY があれば -dvvv の出力へ Authority= の行として足す（本物の出力はそのまま）
out=$(/usr/bin/codesign "$@" 2>&1); rc=$?
printf '%s\n' "$out" >&2
case " $* " in
  *" -dvvv "*) [ -n "${FAKE_AUTHORITY:-}" ] && printf 'Authority=%s\n' "$FAKE_AUTHORITY" >&2 ;;
esac
exit $rc
EOF
chmod +x "$TMP/bin/id" "$TMP/bin/codesign"

# $1 = 置き場。ad-hoc 署名した最小の tako.app を作る（build-app.sh と同じ DR 固定の署名）
mkapp() {
  local app=$1/tako.app
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp /usr/bin/true "$app/Contents/MacOS/tako-app"
  cp /usr/bin/true "$app/Contents/MacOS/tako"
  echo "GPL-3.0-or-later" >"$app/Contents/Resources/LICENSE"
  cat >"$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>tako-app</string>
	<key>CFBundleIdentifier</key>
	<string>dev.takushio.tako</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
</dict>
</plist>
PLIST
  if [[ "${2:-sign}" == sign ]]; then
    /usr/bin/codesign --force -s - -i dev.takushio.tako.cli \
      -r='designated => identifier "dev.takushio.tako.cli"' "$app/Contents/MacOS/tako" 2>/dev/null
    /usr/bin/codesign --force -s - -r='designated => identifier "dev.takushio.tako"' "$app" 2>/dev/null
  fi
  echo "$app"
}

# 偽の環境で検査を走らせる。stdout と stderr を 1 本にまとめ、最後の行に rc を載せる
run_check() {
  local app=$1
  (
    export HOME=$FAKE_HOME USER="${USER_TEST:-testuser}" TAKO_PII_TERMS="${TAKO_PII_TERMS_TEST:-}"
    export PATH="$TMP/bin:$PATH"
    # shellcheck source=lib/bundle-privacy.sh
    . "$REPO_ROOT/scripts/lib/bundle-privacy.sh"
    check_bundle_privacy "$app" 2>&1
    echo "rc=$?"
  )
}

echo "== A1: 何も入っていない ad-hoc の .app は通る =="
APP=$(mkapp "$TMP/a1")
out=$(run_check "$APP")
assert_has "rc=0" "rc=0" "$out"
assert_has "OK を出す" "OK: ビルド機の識別子 0 件" "$out"
sig=$(/usr/bin/codesign -dvvv "$APP" 2>&1)
assert_has "偽の .app は本物の ad-hoc 署名" "Signature=adhoc" "$sig"

echo "== A2: 実行ファイルにホームパスが入っていれば落ち、値は出さない =="
APP=$(mkapp "$TMP/a2")
printf '%s/.cargo/registry/src/x/lib.rs\0%s/dev/tako/target/out/shaders.air\0' "$FAKE_HOME" "$FAKE_HOME" \
  >>"$APP/Contents/MacOS/tako-app"
out=$(run_check "$APP")
assert_has "rc=1" "rc=1" "$out"
assert_has "場所と種類と件数を出す" "Contents/MacOS/tako-app: ビルド機のホームパスが 2 箇所" "$out"
assert_has "ログイン名としても数える" "Contents/MacOS/tako-app: ログイン名が 2 箇所" "$out"
assert_lacks "ホームパスの値を出さない" "$FAKE_HOME" "$out"

echo "== A3: 同梱の文書にフルネームがあれば落ち、値は出さない =="
APP=$(mkapp "$TMP/a3")
printf 'Copyright %s\n' "$FAKE_FULLNAME" >>"$APP/Contents/Resources/LICENSE"
out=$(run_check "$APP")
assert_has "rc=1" "rc=1" "$out"
assert_has "Resources の中も見る" "Contents/Resources/LICENSE: アカウントのフルネームが 1 箇所" "$out"
assert_lacks "フルネームを出さない" "$FAKE_FULLNAME" "$out"

echo "== A4: TAKO_PII_TERMS の語（前後の空白は落とす）=="
APP=$(mkapp "$TMP/a4")
printf 'handle=fake-handle-1848\n' >>"$APP/Contents/Resources/LICENSE"
out=$(TAKO_PII_TERMS_TEST=" fake-handle-1848 , ab" run_check "$APP")
assert_has "rc=1" "rc=1" "$out"
assert_has "外から足した語を数える" "Contents/Resources/LICENSE: TAKO_PII_TERMS の語が 1 箇所" "$out"
assert_lacks "語を出さない" "fake-handle-1848" "$out"

echo "== A5: 署名が Apple Development なら落ち、名義を伏せる =="
APP=$(mkapp "$TMP/a5")
out=$(FAKE_AUTHORITY="$FAKE_CERT" run_check "$APP")
assert_has "rc=1" "rc=1" "$out"
assert_has "app 本体の署名を指す" "tako.app: 署名が個人の開発用証明書（Authority=Apple Development: <伏せ字>）" "$out"
assert_has "同梱 CLI の署名も見る" "Contents/MacOS/tako: 署名が個人の開発用証明書" "$out"
assert_lacks "名義を出さない" "山田" "$out"
assert_lacks "証明書の ID を出さない" "ABCDE12345" "$out"

echo "== A6: 自己署名などでも名義に語が入っていれば落ちる／入っていなければ通る =="
APP=$(mkapp "$TMP/a6")
out=$(FAKE_AUTHORITY="testuser code signing" run_check "$APP")
assert_has "rc=1" "rc=1" "$out"
assert_has "名義のログイン名" "署名の名義にログイン名が入っている（Authority=<伏せ字>）" "$out"
out=$(FAKE_AUTHORITY="Example Signing" run_check "$APP")
assert_has "個人名の無い名義は通す" "rc=0" "$out"

echo "== A7: 汎用のログイン名は語にしない =="
APP=$(mkapp "$TMP/a7")
printf 'runner\n' >>"$APP/Contents/Resources/LICENSE"
out=$(USER_TEST=runner run_check "$APP")
assert_has "runner は数えない" "rc=0" "$out"

echo "== A8: 署名の無い .app・存在しないパスは通さない =="
APP=$(mkapp "$TMP/a8" nosign)
# /usr/bin/true の複製は Apple の署名を持ったままなので、署名の無い実行ファイルへ置き換える
printf '#!/bin/sh\nexit 0\n' >"$APP/Contents/MacOS/tako-app"
printf '#!/bin/sh\nexit 0\n' >"$APP/Contents/MacOS/tako"
out=$(run_check "$APP")
assert_has "未署名は rc=1" "rc=1" "$out"
assert_has "理由を出す" "署名を読めない" "$out"
out=$(run_check "$TMP/nowhere/tako.app")
assert_has "存在しなければ rc=2" "rc=2" "$out"

echo "== B: xcrun の包み =="
SHIM="$REPO_ROOT/scripts/lib/xcrun-remap/xcrun"
cat >"$TMP/bin/fake-xcrun" <<'EOF'
#!/bin/bash
# 呼ばれた cwd と引数を 1 行ずつ記録する
{ echo "cwd=$PWD"; for a in "$@"; do echo "arg=$a"; done; } >"$XCRUN_LOG"
EOF
chmod +x "$TMP/bin/fake-xcrun"
FROM="/Users/First Last" # 空白入りのホームでも 1 引数のまま渡ること
OUTD="$TMP/out dir"
mkdir -p "$OUTD"
shim() {
  (cd "$TMP" && XCRUN_LOG="$TMP/xcrun.log" TAKO_REAL_XCRUN="$TMP/bin/fake-xcrun" \
    TAKO_REMAP_FROM="$FROM" TAKO_REMAP_TO="~" "$SHIM" "$@")
  tr '\n' '|' <"$TMP/xcrun.log"
}
got=$(shim -sdk macosx metal -gline-tables-only -c ./src/shaders.metal -o "$OUTD/shaders.air")
assert_eq "metal: ツール名の直後に -ffile-prefix-map を 1 引数で足す" "$got" \
  "cwd=$TMP|arg=-sdk|arg=macosx|arg=metal|arg=-ffile-prefix-map=$FROM=~|arg=-gline-tables-only|arg=-c|arg=./src/shaders.metal|arg=-o|arg=$OUTD/shaders.air|"
got=$(shim -sdk macosx metallib "$OUTD/shaders.air" -o "$OUTD/shaders.metallib")
assert_eq "metallib: 出力先へ移り、同じ dir のパスを名前だけにする" "$got" \
  "cwd=$OUTD|arg=-sdk|arg=macosx|arg=metallib|arg=shaders.air|arg=-o|arg=shaders.metallib|"
got=$(shim --show-sdk-path --sdk macosx)
assert_eq "ツールを実行しない呼び出しは素通し" "$got" "cwd=$TMP|arg=--show-sdk-path|arg=--sdk|arg=macosx|"
got=$(shim -f metal)
assert_eq "--find は素通し（metal への差し込みをしない）" "$got" "cwd=$TMP|arg=-f|arg=metal|"
got=$(shim -sdk macosx clang -c a.c)
assert_eq "ほかのツールは素通し" "$got" "cwd=$TMP|arg=-sdk|arg=macosx|arg=clang|arg=-c|arg=a.c|"
got=$(cd "$TMP" && XCRUN_LOG="$TMP/xcrun.log" TAKO_REAL_XCRUN="$TMP/bin/fake-xcrun" \
  "$SHIM" -sdk macosx metal -c x.metal && tr '\n' '|' <"$TMP/xcrun.log")
assert_eq "付け替えの指定が無ければ素通し" "$got" "cwd=$TMP|arg=-sdk|arg=macosx|arg=metal|arg=-c|arg=x.metal|"

# 実物（Metal の toolchain がある手元だけ。CI のランナーには無いことがある）
if /usr/bin/xcrun -sdk macosx -f metal >/dev/null 2>&1 && /usr/bin/xcrun -sdk macosx -f metallib >/dev/null 2>&1; then
  echo "== B2: 実物の metal / metallib でパスが残らない =="
  W="$TMP/real"
  mkdir -p "$W/crate/src" "$W/out"
  printf '#include <metal_stdlib>\nusing namespace metal;\nfragment float4 fs() { return float4(1, 0, 0, 1); }\n' \
    >"$W/crate/src/shaders.metal"
  build_real() {
    (cd "$W/crate" && "$@" -sdk macosx metal -gline-tables-only -c ./src/shaders.metal -o "$W/out/shaders.air" \
      && "$@" -sdk macosx metallib "$W/out/shaders.air" -o "$W/out/shaders.metallib")
  }
  if build_real /usr/bin/xcrun 2>/dev/null; then
    n_before=$(LC_ALL=C grep -a -o -F -- "$W" "$W/out/shaders.metallib" | wc -l | tr -d ' ')
    TAKO_REMAP_FROM="$W" TAKO_REMAP_TO="~" build_real "$SHIM" 2>/dev/null
    n_after=$(LC_ALL=C grep -a -o -F -- "$W" "$W/out/shaders.metallib" | wc -l | tr -d ' ')
    if [[ "$n_before" -gt 0 ]]; then ok "包み無しでは metallib にパスが入る（${n_before} 箇所）"; else ng "包み無しでパスが入る" "0 箇所（前提が変わった）"; fi
    assert_eq "包みを通すと 0 箇所" "$n_after" "0"
  else
    echo "  SKIP: metal のコンパイルが通らない環境"
  fi
else
  echo "  SKIP: Metal の toolchain が無い（B2）"
fi

echo ""
echo "================================"
echo "  結果: $PASS pass / $FAIL fail"
echo "================================"
[[ $FAIL -eq 0 ]]
