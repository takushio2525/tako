---
title: ビルド方法
description: tako をソースからビルドする手順。前提条件、ソースの取得、cargo でのビルドと実行、セルフテスト、macOS 用の .app バンドルの生成までを説明します
---

tako をソースからビルドする手順です。

## 前提条件

- **Rust**（最新 stable）: https://rustup.rs/
- **Node.js（npm）**（LTS 版。22 系・24 系で確かめています）: https://nodejs.org/
  （Windows は `winget install --id OpenJS.NodeJS.LTS` でも入ります）。
  リモート用のスマホ画面（PWA・`web/tako-remote`）をビルドしてバイナリに組み込むのに使います
- **macOS**: Xcode Command Line Tools（`xcode-select --install`）
- **tmux**（推奨）: `brew install tmux`

:::note
PWA のビルド出力（`web/tako-remote/dist/`）は Git に含めていないので、clone した直後のツリーには
ありません。最初の `cargo build` がそれを見て `web/tako-remote` で `npm ci` と `npm run build` を
自動で走らせます（その回だけビルドが数十秒延びます）。npm が見つからないとビルドはそこで止まり、
エラーの 1 行目に次の一手が出ます。一度できた `dist/` は作り直さないので、PWA のソースを直したら
`scripts/build-pwa.sh` で作り直してください。
:::

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

# /Applications にインストール（配置後、ビルド出力の dist/tako.app は片付けられる）
scripts/build-app.sh --install
```

:::note
同じ `.app` がディスク上に 2 つあると macOS の Launch Services が両方を登録し、Finder の
「このアプリケーションで開く」に tako が 2 つ並びます（Issue #837）。`--install` は配置後に
ビルド出力を消して LS の登録も外すため、候補は `/Applications` の 1 つだけになります。
`--install` を付けないビルドでは `dist/tako.app` が残るので、スクリプトが片付け方を表示します。
:::

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
