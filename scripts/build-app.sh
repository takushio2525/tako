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
# 配布物の個人情報チェック（#1848）。release.sh も zip の直前に同じものを呼ぶ
# shellcheck source=lib/bundle-privacy.sh
source "$REPO_ROOT/scripts/lib/bundle-privacy.sh"

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
# Issue #60: リリース zip に stale な dist が同梱されるのを防止（= 引数なし = 毎回作り直す）。
# 手順の正本は scripts/build-pwa.sh（#1309。crates/tako-control/build.rs も同じ手順を持つ）
echo "==> PWA ビルド（web/tako-remote）"
"$REPO_ROOT/scripts/build-pwa.sh"

# --- 配布物からビルド機のパスを消す（Issue #1848）---
# リリースビルドには依存 crate・std のソースパス（panic の位置情報）と gpui のシェーダー
# （metallib）のパスが絶対パスで入り、v0.8.24 では約 1,400 箇所がビルド機のホーム配下
# （= 実ユーザー名入り）だった。ホームを `~` へ付け替えてビルドする:
#   - rustc: --remap-path-prefix（CARGO_ENCODED_RUSTFLAGS。空白入りのホームでも割れない）
#   - metal / metallib: scripts/lib/xcrun-remap/xcrun（rustc のフラグが届かない。理由は同ファイル）
# パスはリポジトリに書かず、ここで環境から組み立てる（public リポ = #927）。
# フラグが変わると cargo のキャッシュが別物になるので、専用の target dir でビルドし、
# 隔離検証・テストが使う target/release とは混ぜない。フラグにはホームだけを入れる
# （worktree のパスを入れると、夜間リリースの一時 worktree で毎晩フルビルドになる）。
# 付け替えの漏れは、署名の後の check_bundle_privacy が落とす
: "${HOME:?HOME が無いとビルド機のパスを付け替えられない（#1848）}"
DIST_TARGET_DIR="$REPO_ROOT/target/release-dist"
REMAP_FLAG="--remap-path-prefix=$HOME=~"
US=$'\x1f' # CARGO_ENCODED_RUSTFLAGS の区切り
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  DIST_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}${US}${REMAP_FLAG}"
else
  # CARGO_ENCODED_RUSTFLAGS は RUSTFLAGS より優先されるので、指定があれば cargo と同じく
  # 空白で区切って引き継ぐ
  DIST_RUSTFLAGS=""
  read -r -a user_rustflags <<<"${RUSTFLAGS:-}" || true
  for f in ${user_rustflags[@]+"${user_rustflags[@]}"}; do
    DIST_RUSTFLAGS+="${f}${US}"
  done
  DIST_RUSTFLAGS+="$REMAP_FLAG"
fi

echo "==> リリースビルド（tako-app + tako-cli, profile.release。ビルド機のパスは ~ へ付け替え）"
CARGO_ENCODED_RUSTFLAGS="$DIST_RUSTFLAGS" \
  PATH="$REPO_ROOT/scripts/lib/xcrun-remap:$PATH" \
  TAKO_REMAP_FROM="$HOME" TAKO_REMAP_TO="~" \
  cargo build --release --target-dir "$DIST_TARGET_DIR" -p tako-app -p tako-cli

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
  for spec in ${SPECS[@]+"${SPECS[@]}"}; do
    name=${spec% *}; size=${spec#* }
    rsvg-convert -w "$size" -h "$size" "$SVG" -o "$ICONSET/$name"
  done
else
  echo "    rsvg-convert なし → preview/icon-a-1024.png から sips で縮小生成"
  for spec in ${SPECS[@]+"${SPECS[@]}"}; do
    name=${spec% *}; size=${spec#* }
    sips -z "$size" "$size" "$PNG1024" --out "$ICONSET/$name" >/dev/null
  done
fi
iconutil -c icns "$ICONSET" -o "$DIST/tako.icns"
rm -rf "$ICONSET"

echo "==> tako.app の組み立て"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$DIST_TARGET_DIR/release/tako-app" "$APP/Contents/MacOS/tako-app"
# tako CLI（MCP stdio ブリッジ `tako mcp serve` を含む）も同梱する。
# `claude mcp add --scope user tako -- <パス> mcp serve` の登録先パスを
# /Applications 配下で安定させるため（target/debug はビルドで消え得る）
cp "$DIST_TARGET_DIR/release/tako" "$APP/Contents/MacOS/tako"
mv "$DIST/tako.icns" "$APP/Contents/Resources/tako.icns"
# ライセンス本文と第三者の告知を同梱する（Issue #1709）。GPL-3.0 第 4 条・Apache-2.0 第 4 条 (a)・
# MIT / BSD の表示義務は、バイナリの受け取り手へ本文と著作権表示を渡すことを求める。
# .app ごと配る経路（リリース zip・Homebrew cask・アプリ内更新）はすべてここを通る。
# 署名より前に置くので、この 3 つも署名の封印に含まれる。Windows の zip・インストーラーも
# 同じ 3 本を置き、揃っていることは crates/tako-control/tests/license_bundle_watchdog.rs が
# 検査する（Issue #1845）
cp LICENSE "$APP/Contents/Resources/LICENSE"
cp THIRD-PARTY-NOTICES.md "$APP/Contents/Resources/THIRD-PARTY-NOTICES.md"
cp THIRD-PARTY-LICENSES.md "$APP/Contents/Resources/THIRD-PARTY-LICENSES.md"

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
#
# 既定は ad-hoc 署名（Issue #1848）。以前はキーチェーンの Apple Development 証明書を
# 自動で選んでいたが、その名義は個人名で、配布物を `codesign -dvvv` すれば誰でも読めた。
# Apple Development は配布用の証明書ではなく、公証も無いので、Gatekeeper の扱いは ad-hoc と
# 変わらない（2026-09-30 実測: quarantine 付きの zip を展開した両方が spctl で rejected、
# syspolicy_check の Fatal は両方とも「Notary Ticket Missing」だけ）。TCC は上の DR 固定で
# 保持される（同日実測: TCC.db の tako の全行で、要件を満たすかが両者で一致）。
# ad-hoc は秘密鍵を使わないので、夜間リリース（launchd）がキーチェーンの許可待ちで止まる
# こともなく、証明書の失効も無い。
# 証明書で署名したいとき（自己署名・Phase 7 の Developer ID）は TAKO_CODESIGN_IDENTITY で
# 明示する。名義に個人名が入れば check_bundle_privacy が落とす
REQ_APP='designated => identifier "dev.takushio.tako"'
REQ_CLI='designated => identifier "dev.takushio.tako.cli"'
IDENTITY="${TAKO_CODESIGN_IDENTITY:-}"
if [[ -n "$IDENTITY" ]]; then
  # 名義はログへ出さない（夜間リリースのログに個人名を残さない。#1848）
  echo "==> 署名（identity: TAKO_CODESIGN_IDENTITY の指定 / DR: identifier 固定）"
  codesign --force -s "$IDENTITY" -i dev.takushio.tako.cli -r="$REQ_CLI" "$APP/Contents/MacOS/tako"
  codesign --force -s "$IDENTITY" -r="$REQ_APP" "$APP"
else
  echo "==> ad-hoc 署名（DR は identifier 固定のため、TCC の権限承認はビルド・更新をまたいで保持される）"
  codesign --force -s - -i dev.takushio.tako.cli -r="$REQ_CLI" "$APP/Contents/MacOS/tako"
  codesign --force -s - -r="$REQ_APP" "$APP"
fi

echo "==> 署名検証（designated requirement の固定を機械確認）"
codesign --verify -R='identifier "dev.takushio.tako"' "$APP"
codesign --verify -R='identifier "dev.takushio.tako.cli"' "$APP/Contents/MacOS/tako"

echo "==> 配布物の個人情報チェック（ビルド機のパス・署名の名義。#1848）"
check_bundle_privacy "$APP" || exit 1

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
