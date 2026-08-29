#!/usr/bin/env bash
# build-app.sh — tako.app を 1 コマンドで生成する（macOS 専用、Phase 3.5）
#
# 使い方:
#   scripts/build-app.sh            # dist/tako.app を生成
#   scripts/build-app.sh --verify   # 生成後、バンドル版バイナリでセルフテスト
#                                   # （TAKO_* 注入 / IPC / MCP を含む全項目）を実行
#   scripts/build-app.sh --install  # 生成後、/Applications へコピーし、ビルド出力は片付ける
#                                   # （同じ .app が 2 つ残ると Finder の「このアプリケーションで
#                                   #   開く」に tako が 2 つ並ぶ。Issue #837）
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

# Launch Services の登録は共有ライブラリに集約する（Issue #837。実測値と不変条件は
# scripts/lib/launch-services.sh の冒頭コメントが正）。release.sh も同じものを使う
# shellcheck source=lib/launch-services.sh
source "$REPO_ROOT/scripts/lib/launch-services.sh"
# shellcheck source=lib/bundle-install.sh
source "$REPO_ROOT/scripts/lib/bundle-install.sh"

VERIFY=0
INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --verify) VERIFY=1 ;;
    --install) INSTALL=1 ;;
    *) echo "不明な引数: ${arg}（--verify / --install のみ対応）" >&2; exit 2 ;;
  esac
done

if [[ "$(uname)" != "Darwin" ]]; then
  echo "エラー: .app バンドルの生成は macOS 専用（iconutil / codesign 依存）" >&2
  exit 1
fi

# --- PWA ビルド（web/tako-remote）---
# rust_embed が web/tako-remote/dist/ をコンパイル時に埋め込むため、
# cargo build より前に npm build を済ませる必要がある。
# Issue #60: リリース zip に stale な dist が同梱されるのを防止
PWA_DIR="$REPO_ROOT/web/tako-remote"
if command -v npm >/dev/null; then
  echo "==> PWA ビルド（web/tako-remote）"
  (cd "$PWA_DIR" && npm ci --no-audit --no-fund && npm run build)
else
  if [[ -d "$PWA_DIR/dist/assets" ]]; then
    echo "警告: npm が見つからないため PWA の再ビルドをスキップ（既存 dist を使用）" >&2
  else
    echo "エラー: npm が見つからず、PWA の dist も存在しない。npm をインストールしてください" >&2
    exit 1
  fi
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
	<!-- Finder の「このアプリケーションで開く」候補に出す（FR-3.22 / Issue #708）。
	     LSHandlerRank は**すべて Alternate 固定**: Default / Owner にすると
	     Launch Services が tako を既定ハンドラに選び得るため、既定アプリを奪う。
	     Alternate は「開けるが既定ではない」= 候補一覧に並ぶだけ。
	     この不変条件は tako-app の open_files.rs のテストが機械検証している。

	     対象は UTI（LSItemContentTypes）だけで宣言し、CFBundleTypeExtensions は
	     使わない。拡張子指定は macOS が UTI を持たない拡張子（実測: .rs / .toml /
	     .go / .conf 等は dyn.* = public.data 止まり）にも候補を出せる反面、
	     その拡張子を他アプリが 1 つも宣言していないと Alternate でも tako が
	     既定ハンドラになってしまう。既定を一切動かさないことを優先する。 -->
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Text Document</string>
			<key>CFBundleTypeRole</key>
			<string>Editor</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
				<string>public.text</string>
				<string>public.plain-text</string>
				<string>public.utf8-plain-text</string>
				<string>public.source-code</string>
				<string>public.script</string>
				<string>public.json</string>
				<string>public.yaml</string>
				<string>public.xml</string>
				<string>net.daringfireball.markdown</string>
			</array>
		</dict>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Preview Document</string>
			<key>CFBundleTypeRole</key>
			<string>Viewer</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
				<string>com.adobe.pdf</string>
				<string>public.image</string>
				<string>public.movie</string>
			</array>
		</dict>
	</array>
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
  echo "==> $LS_CANONICAL_APP へ配置"
  # 置き場のパスを一度も空けずに差し替える（#1042）。rm -rf → cp -R だと
  # その窓を観測した Dock のピン留めが外れる
  if ! install_strategy="$(install_bundle_in_place "$APP" "$LS_CANONICAL_APP")"; then
    echo "エラー: ${LS_CANONICAL_APP} への配置に失敗" >&2
    exit 1
  fi
  echo "    差し替えの手段: ${install_strategy}"
  ls_register "$LS_CANONICAL_APP"
  echo "==> Launch Services へ登録（CFBundleDocumentTypes の反映。#708）"

  # install 済みが正本になった時点でビルド出力は用済み。置いたままにすると LS が拾って
  # Finder の候補に tako が 2 つ並ぶので、実体を消して登録も外す（#837）
  ls_drop_build_output "$APP" "$DIST"

  echo "==> $LS_CANONICAL_APP 配置完了"
fi

if [[ $INSTALL -eq 0 && -d "$APP" ]]; then
  # 素のビルド / --verify では配布物としてビルド出力を残す（release.sh が使う）。
  # 残っている間は LS に登録されるので、黙って二重化させない（#837）
  echo "メモ: $APP を残しました。ディスク上にある間は Launch Services に登録され、"
  echo "      Finder の「このアプリケーションで開く」に $LS_CANONICAL_APP の tako と 2 つ並びます（#837）。"
  echo "      --install なら自動で片付けます。手動で消すなら:"
  echo "        rm -rf \"$APP\" && \"$LSREGISTER\" -u \"$APP\""
fi
