---
title: ビルド方法
description: tako をソースからビルドする手順
---

tako をソースからビルドする手順です。

## 前提条件

共通:

- **Rust**（リポジトリの `rust-toolchain.toml` でバージョン固定）: https://rustup.rs/
- **Node.js**（20 以降）: リモート用 PWA（`web/tako-remote`）をバイナリへ埋め込むため、
  **Rust のビルド前に PWA をビルドしておく必要があります**（後述）
- **git**

### macOS

- Xcode Command Line Tools（`xcode-select --install`）
- **tmux**（推奨）: `brew install tmux`

### Windows

Windows は 64bit（x64）の Windows 10 バージョン 1809 以降、または Windows 11 が対象です。
`x86_64-pc-windows-msvc` ターゲットは MSVC のリンカと Windows SDK を使います。

```powershell
# Visual Studio 2022 Build Tools（C++ によるデスクトップ開発）
winget install --id Microsoft.VisualStudio.2022.BuildTools --override `
  "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

# Rust / git / Node.js
winget install --id Rustlang.Rustup
winget install --id Git.Git
winget install --id OpenJS.NodeJS.LTS
```

Visual Studio Installer の GUI で入れる場合は「**C++ によるデスクトップ開発**」を選び、
次の 3 つが含まれていることを確認してください。

- MSVC v143 ビルドツール（x64/x86）
- Windows 11 SDK
- **MSVC v143 - VS 2022 C++ x64/x86 Spectre 緩和ライブラリ（最新）** — 一部の依存クレートが要求します

インストール後は**一度サインアウトまたは再起動**して、環境変数を反映させてください。

:::note[改行コードに注意]
リポジトリには `.gitattributes` があります。`core.autocrlf` を変更しないでください
（シェル統合スクリプトが LF 必須のため）。
:::

:::tip[ビルドが極端に遅いとき]
Microsoft Defender のリアルタイム保護の除外に `%USERPROFILE%\.cargo` と
リポジトリの `target` フォルダを追加すると改善します。
:::

## ソースの取得

```bash
git clone https://github.com/takushio2525/tako.git
cd tako
```

## PWA のビルド（Rust ビルドの前に必須）

`tako-control` はリモート用 PWA の `dist` を `rust_embed` でバイナリへ埋め込むため、
**`web/tako-remote/dist` が無いとコンパイルできません**。

```bash
cd web/tako-remote
npm ci
npm run build
cd ../..
```

## ビルド

```bash
# ワークスペース全体をビルド
cargo build --workspace

# リリースビルド
cargo build --workspace --release
```

macOS 側で開発している場合は、コミット前に `scripts/check-windows.sh`
（`cargo check --target x86_64-pc-windows-msvc`）を実行すると、
Windows のコンパイル崩れを持ち込まずに済みます。

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
