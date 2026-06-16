#!/usr/bin/env bash
# build-app.sh — tako.app を 1 コマンドで生成する（macOS 専用、Phase 3.5）
#
# 使い方:
#   scripts/build-app.sh            # dist/tako.app を生成
#   scripts/build-app.sh --verify   # 生成後、バンドル版バイナリでセルフテスト
#                                   # （TAKO_* 注入 / IPC / MCP を含む全項目）を実行
#   scripts/build-app.sh --install  # 生成後、/Applications へコピー
#
# 方式メモ: cargo-bundle は不採用（メンテ停滞・icns 生成は結局別途必要・
# macOS 専用なら OS 同梱の iconutil / sips + 素のスクリプトで依存ゼロにできる）。
# アイコンは assets/icon/icon-a.svg（A 案採用、assets/icon/README.md）。
# rsvg-convert（brew install librsvg）があれば SVG から全サイズを直接描画、
# 無ければ同梱の preview/icon-a-1024.png から sips で縮小生成する。
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
DIST="$REPO_ROOT/dist"
APP="$DIST/tako.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

VERIFY=0
INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --verify) VERIFY=1 ;;
    --install) INSTALL=1 ;;
    *) echo "不明な引数: $arg（--verify / --install のみ対応）" >&2; exit 2 ;;
  esac
done

if [[ "$(uname)" != "Darwin" ]]; then
  echo "エラー: .app バンドルの生成は macOS 専用（iconutil / codesign 依存）" >&2
  exit 1
fi

echo "==> リリースビルド（tako-app + tako-cli, profile.release）"
cargo build --release -p tako-app -p tako-cli

echo "==> アイコン生成（icon-a.svg → tako.icns）"
ICONSET="$DIST/tako.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
SVG="$REPO_ROOT/assets/icon/icon-a.svg"
PNG1024="$REPO_ROOT/assets/icon/preview/icon-a-1024.png"
# macOS の iconset 規約: 16/32/128/256/512 の @1x と @2x（@2x は上位サイズと同寸）
declare -a SPECS=(
  "icon_16x16.png 16" "icon_16x16@2x.png 32"
  "icon_32x32.png 32" "icon_32x32@2x.png 64"
  "icon_128x128.png 128" "icon_128x128@2x.png 256"
  "icon_256x256.png 256" "icon_256x256@2x.png 512"
  "icon_512x512.png 512" "icon_512x512@2x.png 1024"
)
if command -v rsvg-convert >/dev/null; then
  for spec in "${SPECS[@]}"; do
    name=${spec% *}; size=${spec#* }
    rsvg-convert -w "$size" -h "$size" "$SVG" -o "$ICONSET/$name"
  done
else
  echo "    rsvg-convert なし → preview/icon-a-1024.png から sips で縮小生成"
  for spec in "${SPECS[@]}"; do
    name=${spec% *}; size=${spec#* }
    sips -z "$size" "$size" "$PNG1024" --out "$ICONSET/$name" >/dev/null
  done
fi
iconutil -c icns "$ICONSET" -o "$DIST/tako.icns"
rm -rf "$ICONSET"

echo "==> tako.app の組み立て"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/tako-app "$APP/Contents/MacOS/tako-app"
# tako CLI（MCP stdio ブリッジ `tako mcp serve` を含む）も同梱する。
# `claude mcp add --scope user tako -- <パス> mcp serve` の登録先パスを
# /Applications 配下で安定させるため（target/debug はビルドで消え得る）
cp target/release/tako "$APP/Contents/MacOS/tako"
mv "$DIST/tako.icns" "$APP/Contents/Resources/tako.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>ja</string>
	<key>CFBundleDisplayName</key>
	<string>tako</string>
	<key>CFBundleExecutable</key>
	<string>tako-app</string>
	<key>CFBundleIconFile</key>
	<string>tako</string>
	<key>CFBundleIdentifier</key>
	<string>dev.takushio.tako</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>tako</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${VERSION}</string>
	<key>CFBundleVersion</key>
	<string>${VERSION}</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>GPL-3.0-or-later</string>
</dict>
</plist>
PLIST

# 署名 identity の解決。ad-hoc（-s -）はビルドごとに CDHash が変わり、macOS の
# TCC がビルドのたびに別アプリ扱いして権限（フォルダアクセス等）をリセットする
# （2026-06-12 実機バグ: 何回承認してもダイアログが再出現）。安定 identity =
# キーチェーンの Apple Development 証明書（Xcode が管理。無ければ TAKO_CODESIGN_IDENTITY
# で自己署名証明書等を指定）で署名し、権限の承認をビルドをまたいで保持する。
# 配布署名（Developer ID + notarization）は Phase 7 で別途
resolve_sign_identity() {
  if [[ -n "${TAKO_CODESIGN_IDENTITY:-}" ]]; then
    echo "$TAKO_CODESIGN_IDENTITY"
    return
  fi
  # 最初の有効な Apple Development identity の SHA-1 を使う（名前指定は重複時に
  # codesign が ambiguous で落ちるため、ハッシュ指定で一意化する）
  security find-identity -p codesigning -v 2>/dev/null \
    | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Apple Development:.*/\1/p' | head -1
}
IDENTITY=$(resolve_sign_identity)
if [[ -n "$IDENTITY" ]]; then
  echo "==> 署名（identity: ${IDENTITY}。TCC 権限がビルドをまたいで保持される）"
  codesign --force -s "$IDENTITY" "$APP/Contents/MacOS/tako"
  codesign --force -s "$IDENTITY" "$APP"
else
  echo "==> ad-hoc 署名（注意: 安定 identity が無いため、ビルドごとに TCC の"
  echo "    フォルダアクセス権限がリセットされる。Xcode で Apple Development 証明書を"
  echo "    作るか TAKO_CODESIGN_IDENTITY を指定すると解消する）"
  codesign --force -s - "$APP/Contents/MacOS/tako"
  codesign --force -s - "$APP"
fi

echo "==> 生成完了: ${APP}（バージョン ${VERSION}）"

if [[ $VERIFY -eq 1 ]]; then
  echo "==> バンドル版セルフテスト（TAKO_* 注入 / IPC / MCP を含む全項目）"
  # セルフテストはペイン内から実 tako CLI（同梱版が exe 隣に居る）を叩く e2e を含む。
  # cargo build を内部で呼ぶためリポジトリ内から実行すること
  if TAKO_SELF_TEST=1 "$APP/Contents/MacOS/tako-app" | grep -q "TAKO_APP_SELF_TEST_OK"; then
    echo "==> セルフテスト OK"
  else
    echo "エラー: バンドル版セルフテストが失敗" >&2
    exit 1
  fi
fi

if [[ $INSTALL -eq 1 ]]; then
  echo "==> /Applications へ配置"
  rm -rf /Applications/tako.app
  cp -R "$APP" /Applications/tako.app
  echo "==> /Applications/tako.app 配置完了"
fi
