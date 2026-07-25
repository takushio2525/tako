#!/usr/bin/env bash
# macOS から Windows ターゲットのコンパイル検査を行う（Issue #467 / P0）。
#
# 目的: mac で先行開発している間に Windows ビルドを腐らせないこと。
# 「一括反映」の差分が純粋な機能実装だけになり、ビルド修復作業が混ざらないようにする。
#
# 設計の正: .agent/plans/2026-07-windows-port-architecture.md
# 実機での build / 実行は別途必要（本スクリプトはリンクを行わない型検査まで）。
#
# 使い方:
#   scripts/check-windows.sh            # 全クレートを検査
#   scripts/check-windows.sh -p tako-core   # cargo の追加引数はそのまま渡る
set -euo pipefail

TARGET="x86_64-pc-windows-msvc"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

missing=()

if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  missing+=("rustup target add $TARGET")
fi

if ! command -v cargo-xwin >/dev/null 2>&1; then
  # MSVC の CRT と Windows SDK を取得して cc / rustc に渡す。
  # ring などの C 依存クレートはこれが無いとビルドスクリプトで失敗する
  missing+=("cargo install cargo-xwin")
fi

LLVM_BIN=""
if command -v brew >/dev/null 2>&1; then
  LLVM_BIN="$(brew --prefix llvm 2>/dev/null)/bin"
fi
if [ ! -x "${LLVM_BIN}/clang-cl" ]; then
  # clang-cl / lld-link / llvm-lib / llvm-rc が要る（Apple 標準の clang には lld が無い）
  missing+=("brew install llvm")
fi

if [ ${#missing[@]} -gt 0 ]; then
  echo "Windows クロスチェックの前提ツールが足りません。次を実行してください:" >&2
  for m in "${missing[@]}"; do echo "  $m" >&2; done
  echo >&2
  echo "（CI は使わない方針のため、この検査は各自の macOS 上で行います）" >&2
  exit 2
fi

# PWA を rust_embed で埋め込むため、dist が無いと tako-control がコンパイルできない
if [ ! -d "web/tako-remote/dist" ]; then
  echo "web/tako-remote/dist がありません。先に PWA をビルドします…"
  (cd web/tako-remote && npm ci && npm run build)
fi

# gpui の build script は Windows マニフェストを llvm-rc で埋め込む。
# .rc 内の相対パスが OUT_DIR 基準で解決されるため、そのままでは manifest を見つけられない。
# INCLUDE に gpui のクレートディレクトリを渡すと解決する（2026-07-25 実測）
GPUI_DIR="$(cargo metadata --format-version 1 2>/dev/null | python3 -c '
import json, os, sys
try:
    pkgs = json.load(sys.stdin)["packages"]
except Exception:
    sys.exit(0)
hit = [os.path.dirname(p["manifest_path"]) for p in pkgs if p["name"] == "gpui"]
print(hit[0] if hit else "")
')"

if [ -z "$GPUI_DIR" ]; then
  echo "gpui のクレートディレクトリを解決できませんでした" >&2
  exit 1
fi

echo "target: $TARGET"
echo "gpui:   $GPUI_DIR"
echo

# 注: エラーのみをゲートする。macOS 専用実装に対する dead_code 警告は、
# 各抽象境界の Windows 実装が入るにつれて自然に消える性質のもの
PATH="${LLVM_BIN}:$PATH" INCLUDE="$GPUI_DIR" \
  cargo xwin check --workspace --target "$TARGET" "$@"
