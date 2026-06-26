---
title: ビルド方法
description: tako をソースからビルドする手順
---

tako をソースからビルドする手順です。

## 前提条件

- **Rust**（最新 stable）: https://rustup.rs/
- **macOS**: Xcode Command Line Tools（`xcode-select --install`）
- **tmux**（推奨）: `brew install tmux`

## ソースの取得

```bash
git clone https://github.com/takushio2525/tako.git
cd tako
```

## ビルド

```bash
# ワークスペース全体をビルド
cargo build --workspace

# リリースビルド
cargo build --workspace --release
```

## 実行

```bash
# 開発用ビルドで起動
cargo run -p tako-app
```

## セルフテスト

tako の入力経路・CLI・MCP の動作を自動検証するセルフテストモードがあります。

```bash
# セルフテストモードで起動
TAKO_SELF_TEST=1 cargo run -p tako-app
```

## .app バンドルの生成

macOS 用の .app バンドルを生成するスクリプトがあります。

```bash
# .app バンドルを生成（dist/tako.app）
scripts/build-app.sh

# ビルド後にセルフテストで検証
scripts/build-app.sh --verify

# /Applications にインストール
scripts/build-app.sh --install
```

アイコンは `assets/icon/icon-a.svg` から自動生成されます（`rsvg-convert` がある場合は SVG から直接、なければプリレンダリング済み PNG から生成）。

## Lint / テスト

```bash
# フォーマットチェック
cargo fmt --all --check

# Clippy（警告をエラーとして扱う）
cargo clippy --workspace --all-targets -- -D warnings

# テスト
cargo test --workspace
```

## リリース

```bash
# CHANGELOG.md の更新後
scripts/release.sh --publish
```

`scripts/release.sh` は Cargo.toml のバージョンを読み取り、CHANGELOG.md からリリースノートを抽出して GitHub Release を作成します。

## Claude Code 実機検証

MCP 連携の実機検証スクリプトがあります。

```bash
# Claude Code が tako MCP ツールを使えることを検証
scripts/verify-claude-mcp.sh
```

`claude` CLI のインストールと認証が必要です。
