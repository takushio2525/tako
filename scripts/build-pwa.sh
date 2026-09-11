#!/usr/bin/env bash
# build-pwa.sh — リモート PWA（web/tako-remote）をビルドして dist/ を作る（#1309 / #574 / #60）
#
# rust_embed（crates/tako-control/src/remote.rs の PwaAssets）が web/tako-remote/dist/ を
# **コンパイル時**に要求する。dist/ は .gitignore 対象なので git worktree add 直後・
# クリーンチェックアウトのツリーには存在せず、そのままでは cargo build が落ちる。
#
# 使い方:
#   scripts/build-pwa.sh              # 常にビルドし直す（リリース経路。stale な dist を残さない = #60）
#   scripts/build-pwa.sh --if-missing # dist/index.html が既にあれば何もしない（ツリーの復旧用）
#
# PWA ビルドの実装はここが 1 本。scripts/build-app.sh はこれを呼ぶ。
# crates/tako-control/build.rs は同じ手順を Rust 側にも持つ（Windows の開発機に bash が
# あるとは限らないため）ので、手順を変えるときは対で直す。
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
PWA_DIR="$REPO_ROOT/web/tako-remote"
# 「ビルド済みか」はディレクトリではなくエントリポイントの有無で見る
# （npm が途中で落ちた跡の空の dist/ を「済み」と誤判定しないため。build.rs と同じ判定）
DIST_INDEX="$PWA_DIR/dist/index.html"

IF_MISSING=0
for arg in "$@"; do
  case "$arg" in
    --if-missing) IF_MISSING=1 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "不明な引数: ${arg}（--if-missing のみ対応）" >&2; exit 2 ;;
  esac
done

if [[ "$IF_MISSING" == 1 && -f "$DIST_INDEX" ]]; then
  echo "==> PWA はビルド済み（${DIST_INDEX}）。何もしない"
  exit 0
fi

if ! command -v npm >/dev/null; then
  if [[ -f "$DIST_INDEX" ]]; then
    echo "警告: npm が見つからないため PWA の再ビルドをスキップ（既存の dist を使用）" >&2
    exit 0
  fi
  echo "エラー: npm が見つからず、PWA の dist も無い。Node.js（CI と同じ v22 系）を入れてから" >&2
  echo "        scripts/build-pwa.sh を実行するか、PWA をビルド済みのツリーから" >&2
  echo "        web/tako-remote/dist/ をまるごとコピーすること（#1309）" >&2
  exit 1
fi

# 既定は毎回 npm ci（lockfile どおりの依存で作り直す = リリース zip に stale が乗らない。#60）。
# --if-missing のときだけ、既にある node_modules を使い回してネットワークに触らない
if [[ "$IF_MISSING" == 1 && -d "$PWA_DIR/node_modules" ]]; then
  echo "==> PWA ビルド（既存の node_modules を使う）"
else
  echo "==> PWA 依存インストール（npm ci）"
  (cd "$PWA_DIR" && npm ci --no-audit --no-fund)
fi
(cd "$PWA_DIR" && npm run build)

if [[ ! -f "$DIST_INDEX" ]]; then
  echo "エラー: npm run build は成功したが ${DIST_INDEX} が出来ていない。" >&2
  echo "        vite の出力先（web/tako-remote/vite.config.js の build.outDir）を変えたなら" >&2
  echo "        rust_embed の #[folder] も対で直すこと（#1309）" >&2
  exit 1
fi
echo "==> PWA ビルド完了: ${DIST_INDEX}"
