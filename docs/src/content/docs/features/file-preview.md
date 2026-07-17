---
title: ファイルツリー＆プレビュー
description: サイドバーのファイルツリーとシンタックスハイライト付きプレビュー
---

tako はターミナルでありながら、エディタのようなファイルブラウジング機能を備えています。

## ファイルツリー（左サイドバー）

左サイドバーにファイルツリーを表示できます。

- <kbd>Cmd</kbd>+<kbd>B</kbd> またはステータスバーのトグルボタンで表示/非表示
- タブ内の全ペインの作業ディレクトリを**ワークスペースフォルダ**として自動検出・表示
- ファイルをクリックするとプレビューペインで開く

### コンテキストメニュー（右クリック）

ファイルやフォルダを右クリックするとメニューが表示されます。

- **パスをコピー** — ファイルパスをクリップボードにコピー
- **Finder で表示** — Finder でファイルの場所を開く
- **ここで cd** — アクティブペインのカレントディレクトリを変更
- **名前を変更** — インライン入力でリネーム
- **新しいファイル / フォルダ** — その場で新規作成
- **ゴミ箱に入れる** — ファイルを macOS のゴミ箱に移動

### ドラッグ＆ドロップ

ファイルツリーからペインエリアへドラッグすると:

- **ターミナルペイン**にドロップ → ファイルパスをテキストとして入力
- **プレビューペイン**にドロップ → そのファイルをプレビュー表示

## コードプレビュー

ファイルをクリックまたは `tako open <ファイルパス>` で、ペイン内にファイル内容を表示します。

- **シンタックスハイライト**: 210+ の言語・形式に対応（bat 由来の拡張構文セット）
- **行番号表示**: 行番号付きのコードビュー
- **折り返し**: 長い行は自動折り返し（横スクロール不要）

### 対応形式（主要なもの）

| カテゴリ | 対応形式 |
|---|---|
| システム言語 | Rust, C, C++, Go, Swift, Kotlin, Java, C#, Objective-C, Scala, Haskell, D |
| Web / スクリプト | JavaScript (.js/.jsx/.mjs), TypeScript (.ts/.tsx), Python, Ruby, PHP, Lua, Perl, HTML, CSS |
| シェル | Bash (.sh/.bash/.zsh), Fish |
| データ形式 | JSON, TOML, YAML, XML, INI, CSV, DotENV (.env) |
| ドキュメント | Markdown, LaTeX, reStructuredText |
| ビルド / 設定 | Dockerfile, Makefile, CMake, SQL, Diff/Patch |
| その他 | Git Ignore, Git Attributes, AppleScript, R, Clojure, Erlang, Groovy, nginx.conf 等 |

拡張子に加え、ファイル名でも判定します（例: `Cargo.lock` → TOML, `Dockerfile` → Dockerfile, `CMakeLists.txt` → CMake, `.gitignore` → Git Ignore）。shebang（`#!/bin/bash` 等）による自動検出にも対応しています。

## Markdown プレビュー

`.md` ファイルはデフォルトで**レンダリング表示**されます。

- 見出し・リスト・テーブル・コードブロック・引用をビジュアル表示
- タイトルバーの目アイコンで **コード表示 ⇔ Markdown 表示**を切替可能
- 切替モードは CLI / MCP からも操作可

## 画像・PDF・動画プレビュー

コードと Markdown 以外のファイルも開けます。表示モードは拡張子から自動判定されます。

- **画像**: PNG / JPEG / SVG / GIF / WebP など
- **PDF**: ペイン内でそのまま表示
- **動画**: mp4 の再生・一時停止・シーク（矢印キーやクリックで操作。CLI `tako video` / MCP からも制御可）

## AI からの操作

```bash
# ファイルをプレビューで開く（拡張子から表示モードを自動判定）
tako open src/main.rs

# 表示モードを明示指定して開く
tako open design.pdf --mode pdf

# MCP 経由（AI エージェントが使用）
# tako_open_file ツールで AI が自動的にプレビューを開く
```

AI が「このファイルを見て」と言ったとき、tako は自動的にプレビューペインを開いて該当ファイルを表示します。エディタを別途開く必要はありません。
