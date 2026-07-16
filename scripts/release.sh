#!/usr/bin/env bash
# release.sh — tako.app の zip を生成し、GitHub Releases へアップロードする（macOS 専用）
#
# 使い方:
#   scripts/release.sh              # ビルド → zip 生成まで（リリースは作成しない）
#   scripts/release.sh --publish    # zip 生成 + GitHub Release 作成・アップロード
#   scripts/release.sh --draft      # zip 生成 + ドラフトリリース作成
#   scripts/release.sh --skip-build # ビルド済み dist/tako.app を使って zip のみ再生成
#
# 前提:
#   - macOS（build-app.sh と同じ）
#   - --publish / --draft には gh CLI（`brew install gh`）+ 認証済み
#   - リポジトリのリモートが origin に設定されていること
#
# バージョンは Cargo.toml [workspace.package] から自動読み取り。
# リリースノートは CHANGELOG.md から該当バージョンのセクションを自動抽出。
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
DIST="$REPO_ROOT/dist"
APP="$DIST/tako.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v${VERSION}"
ARCH=$(uname -m)  # arm64 / x86_64
ZIP_NAME="tako-${TAG}-macos-${ARCH}.zip"
ZIP_PATH="$DIST/$ZIP_NAME"

PUBLISH=0
DRAFT=0
SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --publish)    PUBLISH=1 ;;
    --draft)      DRAFT=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    *) echo "不明な引数: $arg（--publish / --draft / --skip-build のみ対応）" >&2; exit 2 ;;
  esac
done

if [[ "$(uname)" != "Darwin" ]]; then
  echo "エラー: macOS 専用" >&2
  exit 1
fi

# --- CHANGELOG.md から該当バージョンのセクションを抽出 ---
extract_changelog() {
  local ver="$1"
  local file="$REPO_ROOT/CHANGELOG.md"
  if [[ ! -f "$file" ]]; then
    return
  fi
  local escaped_ver="${ver//./\\.}"
  sed -n "/^## \\[${escaped_ver}\\]/,/^## \\[/{
    /^## \\[${escaped_ver}\\]/d
    /^## \\[/d
    p
  }" "$file"
}

# --- ビルド ---
if [[ $SKIP_BUILD -eq 0 ]]; then
  echo "==> build-app.sh を実行"
  "$REPO_ROOT/scripts/build-app.sh"
else
  if [[ ! -d "$APP" ]]; then
    echo "エラー: $APP が見つからない（--skip-build には事前ビルドが必要）" >&2
    exit 1
  fi
  echo "==> ビルドをスキップ（既存の $APP を使用）"
fi

# --- PWA dist 鮮度検証（Issue #60 再発防止）---
# ビルド後の dist の JS にソース由来のマーカーが含まれることを確認する。
# stale な dist が同梱されるとリモート PWA の機能が欠落する。
echo "==> PWA dist 鮮度検証"
PWA_DIST="$REPO_ROOT/web/tako-remote/dist"
if [[ ! -d "$PWA_DIST/assets" ]]; then
  echo "エラー: PWA dist が存在しない（$PWA_DIST/assets）" >&2
  echo "  build-app.sh が npm build を実行したか確認してください" >&2
  exit 1
fi
PWA_MARKER_FOUND=0
for jsfile in "$PWA_DIST"/assets/*.js; do
  if grep -q "ペイン" "$jsfile" 2>/dev/null; then
    PWA_MARKER_FOUND=1
    break
  fi
done
if [[ $PWA_MARKER_FOUND -eq 0 ]]; then
  echo "エラー: PWA dist の JS に「ペイン」マーカーが見つからない" >&2
  echo "  dist が stale です。npm run build を実行してから再試行してください" >&2
  exit 1
fi
echo "    OK: dist の JS にソース由来マーカーを確認"

# --- zip 生成 ---
echo "==> zip 生成: $ZIP_NAME"
rm -f "$ZIP_PATH"
# ditto はリソースフォーク・拡張属性を保持する macOS 推奨のアーカイバ
ditto -c -k --keepParent "$APP" "$ZIP_PATH"
ZIP_SIZE=$(du -h "$ZIP_PATH" | cut -f1 | xargs)
echo "    生成完了: $ZIP_PATH ($ZIP_SIZE)"

# --- リリース作成（Pages デプロイより先に実行。リリースの主目的を先に完遂する。#297）---
if [[ $PUBLISH -eq 1 ]] || [[ $DRAFT -eq 1 ]]; then
  if ! command -v gh >/dev/null; then
    echo "エラー: gh CLI が必要（brew install gh）" >&2
    exit 1
  fi

  DRAFT_FLAG=""
  if [[ $DRAFT -eq 1 ]]; then
    DRAFT_FLAG="--draft"
  fi

  # CHANGELOG からリリースノートを組み立て
  CHANGELOG_BODY=$(extract_changelog "$VERSION")

  RELEASE_NOTES="## tako $TAG
"
  if [[ -n "$CHANGELOG_BODY" ]]; then
    RELEASE_NOTES+="
${CHANGELOG_BODY}
---
"
  fi

  RELEASE_NOTES+="
### インストール（macOS） / Install (macOS)

1. **${ZIP_NAME}** をダウンロード / Download **${ZIP_NAME}**
2. zip を展開（ダブルクリック） / Extract the zip
3. \`tako.app\` を \`/Applications\` フォルダへドラッグ / Drag \`tako.app\` to \`/Applications\`
4. 初回起動時に Gatekeeper の警告が出たら:
   **システム設定 → プライバシーとセキュリティ → 「tako」のブロック解除 → このまま開く**
   If Gatekeeper warns on first launch:
   **System Settings → Privacy & Security → Unblock \"tako\" → Open Anyway**

### Claude Code 連携（初回 1 回） / Claude Code Setup (one-time)

\`\`\`sh
claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve
\`\`\`
"

  echo "==> GitHub Release 作成: $TAG"

  # 冪等性: Release が既に存在する場合はアセット追加のみ（#256）
  if gh release view "$TAG" >/dev/null 2>&1; then
    echo "    Release $TAG は既に存在。アセットのアップロードのみ実行"
    gh release upload "$TAG" "$ZIP_PATH" --clobber
  else
    # タグ push 直後は GitHub 側の伝播ラグで gh release create が失敗する
    # ことがあるため、指数バックオフ付きリトライで吸収する（#256）
    MAX_RETRIES=3
    RETRY_WAIT=${TAKO_RELEASE_RETRY_WAIT:-10}
    ATTEMPT=0
    RELEASE_CREATED=0

    while [[ $ATTEMPT -lt $MAX_RETRIES ]]; do
      ATTEMPT=$((ATTEMPT + 1))
      echo "    gh release create: 試行 ${ATTEMPT}/${MAX_RETRIES}"

      GH_STDERR_FILE=$(mktemp)
      GH_EXIT=0
      gh release create "$TAG" \
          --title "tako $TAG" \
          --notes "$RELEASE_NOTES" \
          --generate-notes \
          $DRAFT_FLAG \
          "$ZIP_PATH" 2>"$GH_STDERR_FILE" || GH_EXIT=$?

      if [[ $GH_EXIT -eq 0 ]]; then
        rm -f "$GH_STDERR_FILE"
        RELEASE_CREATED=1
        break
      fi

      echo "    gh release create 失敗（exit ${GH_EXIT}）。gh stderr:" >&2
      cat "$GH_STDERR_FILE" >&2
      rm -f "$GH_STDERR_FILE"

      if [[ $ATTEMPT -lt $MAX_RETRIES ]]; then
        # 部分成功（Release は作られたがアセット添付で失敗等）への対処
        if gh release view "$TAG" >/dev/null 2>&1; then
          echo "    Release $TAG が前回の試行で作成された。アセットをアップロード"
          gh release upload "$TAG" "$ZIP_PATH" --clobber
          RELEASE_CREATED=1
          break
        fi
        echo "    ${RETRY_WAIT} 秒後にリトライ..."
        sleep "$RETRY_WAIT"
        RETRY_WAIT=$((RETRY_WAIT * 2))
      fi
    done

    if [[ $RELEASE_CREATED -eq 0 ]]; then
      echo "" >&2
      echo "ERROR: GitHub Release の作成に ${MAX_RETRIES} 回失敗（tag $TAG は push 済み）" >&2
      echo "手動リカバリ: scripts/release.sh --skip-build --publish" >&2
      exit 1
    fi
  fi

  # --- リモート PWA を Cloudflare Pages へデプロイ（失敗は非致命。#297）---
  # 接続リンクは常に https://tako-remote.pages.dev を指すため、リリース公開時に
  # Pages 側の PWA も同時に最新化する（--draft では実行しない）
  if [[ $PUBLISH -eq 1 ]]; then
    echo "==> リモート PWA を Cloudflare Pages へデプロイ"
    if "$REPO_ROOT/scripts/deploy-pages.sh"; then
      echo "    Pages デプロイ完了"
    else
      echo "WARN: Pages デプロイに失敗（GitHub Release は作成済み）。手動リカバリ: scripts/deploy-pages.sh" >&2
    fi
  fi

  echo "==> リリース完了"
else
  echo ""
  echo "================================================"
  echo "  zip 生成完了（リリースは未作成）"
  echo "================================================"
  echo "  バージョン : $VERSION"
  echo "  タグ       : $TAG"
  echo "  zip        : $ZIP_PATH"
  echo "  サイズ     : $ZIP_SIZE"
  echo "  アーキテクチャ: $ARCH"
  echo ""
  echo "  リリースを作成するには:"
  echo "    scripts/release.sh --publish     # 公開リリース"
  echo "    scripts/release.sh --draft       # ドラフト（非公開）"
  echo "================================================"
fi
