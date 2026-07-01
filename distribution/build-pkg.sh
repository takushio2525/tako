#!/usr/bin/env bash
# build-pkg.sh — tako.app を .pkg インストーラーにパッケージする（macOS 専用）
#
# 使い方:
#   scripts/build-pkg.sh              # 未署名 .pkg を生成（dist/tako-installer.pkg）
#   scripts/build-pkg.sh --sign       # Developer ID Installer で署名（要 Apple Developer Program）
#
# 前提:
#   - macOS（pkgbuild / productbuild は Xcode Command Line Tools に同梱）
#   - dist/tako.app が存在すること（先に build-app.sh を実行）
#   - --sign には Developer ID Installer 証明書がキーチェーンに必要
#
# 注意（2026-07-01 時点）:
#   macOS 26.3（Tahoe）で Developer ID Installer 署名 + notarization 済みの .pkg が
#   Gatekeeper に拒否されるバグが報告されている。.app の署名は問題なし。
#   Apple の修正を待ってから本番投入を推奨。
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
DIST="$REPO_ROOT/dist"
APP="$DIST/tako.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IDENTIFIER="dev.takushio.tako"

SIGN=0
for arg in "$@"; do
  case "$arg" in
    --sign) SIGN=1 ;;
    *) echo "不明な引数: $arg（--sign のみ対応）" >&2; exit 2 ;;
  esac
done

if [[ "$(uname)" != "Darwin" ]]; then
  echo "エラー: macOS 専用" >&2
  exit 1
fi

if [[ ! -d "$APP" ]]; then
  echo "エラー: $APP が見つからない（先に scripts/build-app.sh を実行してください）" >&2
  exit 1
fi

# --- ペイロード準備 ---
PAYLOAD="$DIST/pkg-payload"
rm -rf "$PAYLOAD"
mkdir -p "$PAYLOAD"
# /Applications に配置されるよう、ペイロードルート直下に tako.app を置く
cp -R "$APP" "$PAYLOAD/tako.app"

# --- コンポーネントパッケージ生成 ---
COMPONENT_PKG="$DIST/tako-component.pkg"
echo "==> コンポーネントパッケージ生成"
pkgbuild \
  --root "$PAYLOAD" \
  --identifier "$IDENTIFIER" \
  --version "$VERSION" \
  --install-location /Applications \
  "$COMPONENT_PKG"

# --- distribution.xml からプロダクトアーカイブ生成 ---
DISTRIBUTION_XML="$REPO_ROOT/distribution/distribution.xml"
INSTALLER_PKG="$DIST/tako-installer.pkg"
RESOURCES="$DIST/pkg-resources"
mkdir -p "$RESOURCES"

echo "==> プロダクトアーカイブ生成（UI 付きインストーラー）"
productbuild \
  --distribution "$DISTRIBUTION_XML" \
  --package-path "$DIST" \
  --resources "$RESOURCES" \
  "$INSTALLER_PKG"

# --- 署名（オプション） ---
if [[ $SIGN -eq 1 ]]; then
  # Developer ID Installer 証明書を自動検出
  INSTALLER_IDENTITY=$(security find-identity -p basic -v 2>/dev/null \
    | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Developer ID Installer:.*/\1/p' | head -1)

  if [[ -z "$INSTALLER_IDENTITY" ]]; then
    echo "エラー: Developer ID Installer 証明書が見つからない" >&2
    echo "  Apple Developer Program に加入し、Xcode で証明書を作成してください" >&2
    exit 1
  fi

  SIGNED_PKG="$DIST/tako-installer-signed.pkg"
  echo "==> .pkg 署名（identity: ${INSTALLER_IDENTITY}）"
  productsign --sign "$INSTALLER_IDENTITY" "$INSTALLER_PKG" "$SIGNED_PKG"
  mv "$SIGNED_PKG" "$INSTALLER_PKG"

  echo ""
  echo "署名完了。notarization を行うには:"
  echo "  xcrun notarytool submit $INSTALLER_PKG \\"
  echo "    --apple-id \"your@email.com\" \\"
  echo "    --team-id \"TEAMID\" \\"
  echo "    --password \"@keychain:AC_PASSWORD\" \\"
  echo "    --wait"
  echo "  xcrun stapler staple $INSTALLER_PKG"
fi

# --- クリーンアップ ---
rm -rf "$PAYLOAD" "$COMPONENT_PKG" "$RESOURCES"

echo ""
echo "================================================"
echo "  .pkg 生成完了"
echo "================================================"
echo "  バージョン : $VERSION"
echo "  出力       : $INSTALLER_PKG"
echo "  署名       : $(if [[ $SIGN -eq 1 ]]; then echo '済み'; else echo '未署名（Gatekeeper 警告あり）'; fi)"
echo "================================================"
