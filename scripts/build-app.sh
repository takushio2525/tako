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

# 署名。designated requirement（DR）を identifier 固定で明示する（Issue #54 根治）。
#
# macOS の TCC は付与済み権限をアプリの DR（csreq）に紐付けて保存する。codesign
# 既定の DR は署名証明書に依存し（例: certificate leaf[subject.CN] = "Apple
# Development: ..."）、以下のいずれでも DR が変わって TCC が「別アプリ」と判定し、
# 付与済み権限（ほかのアプリのデータ / フォルダアクセス等）が無効化されていた:
#   - キーチェーンに Apple Development 証明書が複数あり選択が揺れる
#     （find-identity の列挙順は不定。2026-07-03 実機で 2 枚を確認）
#   - 証明書の失効・再発行（Apple Development は 1 年で失効する）
#   - ad-hoc への劣化（DR が CDHash 単位になり毎ビルドで変わる）
# DR を identifier のみに固定すると、どの identity で署名しても・何度ビルドしても・
# アプリ内更新（zip 差し替え。ditto コピーで署名は保持される）の後も DR が不変になり、
# TCC の許可がビルド・更新をまたいで保持される。
# トレードオフ: 同じ identifier を名乗るローカルの別バイナリも DR を満たせる
# （なりすまし耐性は低下）。ローカル開発ツールの脅威モデルでは許容し、Phase 7 の
# Developer ID 配布時に anchor + Team ID を含む DR へ強化する（強化時は 1 回だけ
# TCC の再許可が発生する）。
REQ_APP='designated => identifier "dev.takushio.tako"'
REQ_CLI='designated => identifier "dev.takushio.tako.cli"'
resolve_sign_identity() {
  if [[ -n "${TAKO_CODESIGN_IDENTITY:-}" ]]; then
    echo "$TAKO_CODESIGN_IDENTITY"
    return
  fi
  # Apple Development identity の SHA-1 を昇順ソートの先頭で選ぶ（複数枚あるとき
  # find-identity の列挙順が不定でも選択が揺れないよう決定論化。DR は identifier
  # 固定なのでどれが選ばれても TCC には影響しない。名前指定は重複時に codesign が
  # ambiguous で落ちるため、ハッシュ指定で一意化する）
  security find-identity -p codesigning -v 2>/dev/null \
    | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Apple Development:.*/\1/p' | sort | head -1
}
IDENTITY=$(resolve_sign_identity)
if [[ -n "$IDENTITY" ]]; then
  IDENTITY_NAME=$(security find-identity -p codesigning -v 2>/dev/null \
    | grep -F "$IDENTITY" | sed -E 's/.*"(.*)"/\1/' | head -1)
  echo "==> 署名（identity: ${IDENTITY_NAME:-$IDENTITY} / DR: identifier 固定）"
  codesign --force -s "$IDENTITY" -i dev.takushio.tako.cli -r="$REQ_CLI" "$APP/Contents/MacOS/tako"
  codesign --force -s "$IDENTITY" -r="$REQ_APP" "$APP"
else
  echo "==> ad-hoc 署名（identity なし。DR は identifier 固定のため、ad-hoc でも"
  echo "    TCC の権限承認はビルドをまたいで保持される）"
  codesign --force -s - -i dev.takushio.tako.cli -r="$REQ_CLI" "$APP/Contents/MacOS/tako"
  codesign --force -s - -r="$REQ_APP" "$APP"
fi

echo "==> 署名検証（designated requirement の固定を機械確認）"
codesign --verify -R='identifier "dev.takushio.tako"' "$APP"
codesign --verify -R='identifier "dev.takushio.tako.cli"' "$APP/Contents/MacOS/tako"

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
