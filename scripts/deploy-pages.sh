#!/usr/bin/env bash
# deploy-pages.sh — リモート接続 PWA を Cloudflare Pages（tako-remote.pages.dev）へデプロイする
#
# 接続リンク / QR は常に https://tako-remote.pages.dev の固定 URL で、PWA が KV リレー
# 経由で各 Mac の現在のトンネル URL を解決する（Issue #91）。この Pages は接続経路の
# 入口なので、リリース時（release.sh --publish）に必ず本スクリプトで最新化する。
# デーモン内蔵の PWA（rust_embed）と同一ソースから同一手順でビルドするため、
# Pages 版と内蔵フォールバック版の内容は常に一致する。
#
# 使い方:
#   scripts/deploy-pages.sh          # web/tako-remote をビルドして本番デプロイ
#
# 前提:
#   - Cloudflare Pages プロジェクト「tako-remote」が作成済み（Direct Upload 型）
#   - wrangler 認証済み（`wrangler login`。リレー worker と同じアカウント）
set -euo pipefail

cd "$(dirname "$0")/.."
PWA_DIR="web/tako-remote"
# wrangler はリレー worker 側の devDependency を共用する（PWA 側に依存を増やさない）
WRANGLER="web/tako-remote-worker/node_modules/.bin/wrangler"
PAGES_PROJECT="tako-remote"

if ! command -v npm >/dev/null; then
  echo "エラー: npm が見つからない（PWA のビルドに必要）" >&2
  exit 1
fi

if [[ ! -x "$WRANGLER" ]]; then
  echo "==> wrangler が未取得のため web/tako-remote-worker の依存をインストール"
  (cd web/tako-remote-worker && npm ci --no-audit --no-fund)
fi

# メッセージ中の変数は ${} 必須（macOS の bash 3.2 は全角文字直前の $VAR 展開を誤解釈する）
echo "==> PWA をビルド（${PWA_DIR}）"
(cd "$PWA_DIR" && npm ci --no-audit --no-fund && npm run build)

echo "==> Cloudflare Pages へデプロイ（プロジェクト: ${PAGES_PROJECT}）"
# --branch main で本番（tako-remote.pages.dev）へ。プレビューではなく本番に載せる
"$WRANGLER" pages deploy "$PWA_DIR/dist" \
  --project-name "$PAGES_PROJECT" \
  --branch main \
  --commit-dirty=true

echo "==> デプロイ完了: https://tako-remote.pages.dev"
