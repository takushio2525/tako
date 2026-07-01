---
title: セットアップ
description: tako のダウンロードからインストール、AI エージェント連携までの手順
---

tako を使い始めるまでの手順を説明します。所要時間は 5 分程度です。

## 1. インストール

### 方法 A: Homebrew（推奨）

Homebrew をお使いなら、1 コマンドでインストールできます。tako CLI も自動で PATH に登録されます。

```bash
brew tap takushio2525/tako
brew install --cask takushio2525/tako/tako
```

アップデートも Homebrew 経由で行えます。

```bash
brew upgrade --cask takushio2525/tako/tako
```

### 方法 B: ZIP ダウンロード

[GitHub Releases](https://github.com/takushio2525/tako/releases) から最新の `tako-vX.X.X-macos.zip` をダウンロードし、手動でインストールします。

```bash
# ダウンロードした zip を展開
unzip tako-*.zip

# /Applications に配置
mv tako.app /Applications/
```

#### PATH を通す

ZIP からインストールした場合、`tako` CLI コマンドを使うために PATH を手動で登録します。

```bash
# zsh（macOS デフォルト）の場合
echo 'export PATH="/Applications/tako.app/Contents/MacOS:$PATH"' >> ~/.zshrc
source ~/.zshrc

# bash の場合
echo 'export PATH="/Applications/tako.app/Contents/MacOS:$PATH"' >> ~/.bash_profile
source ~/.bash_profile
```

登録できたか確認:

```bash
tako --version
```

### Gatekeeper の警告が出た場合

初回起動時に「開発元を確認できない」という警告が出ることがあります。ターミナルで以下を実行すると解除できます。

```bash
xattr -dr com.apple.quarantine /Applications/tako.app
```

または以下のいずれかの方法でも OK です:

1. `tako.app` を右クリック → 「開く」を選択
2. 「システム設定 → プライバシーとセキュリティ → このまま開く」をクリック

一度許可すれば、以降は通常のアプリと同じように起動できます。

## 2. 起動

`/Applications/tako.app` をダブルクリック、または Dock から起動します。初回起動時、通常のターミナルと同じようにシェルが 1 ペイン開きます。

## 3. MCP サーバーの登録（初回のみ）

tako 内蔵の MCP サーバーを Claude Code に登録します。tako のターミナルで以下を実行してください。

```bash
tako setup-mcp
```

このコマンドは `~/.claude/settings.json` に tako の MCP サーバーを自動登録します。一度だけ実行すればどのプロジェクトでも有効です。

:::tip
`tako setup-mcp --project` とすると、現在のプロジェクトスコープのみに登録できます。
:::

## 4. AI が tako を使い始める

MCP 登録が完了したら、tako 内で Claude Code を起動するだけです。

```bash
claude
```

Claude Code は自動的に tako の MCP ツールを認識し、以下のような操作ができるようになります。

- **「隣のペインで `npm run dev` を起動して」** → ペインを分割してコマンド実行
- **「このファイルをプレビューで見せて」** → ファイルをプレビューペインで表示
- **「今のレイアウトを見せて」** → タブ・ペイン構成の一覧取得
- **「ログをたまり場に退避して」** → ペインをバックグラウンドへ退避

AI は環境変数（`TAKO_PANE_ID` など）から自分がどのペインにいるかを自動認識するため、明示的な設定は不要です。

## 動作環境

| 項目 | 要件 |
|---|---|
| OS | macOS（Windows 対応は開発中） |
| Node.js | 不要（ネイティブアプリ） |
| tmux | 推奨（再起動復元に使用。未インストールでも動作） |
| Claude Code | AI 連携に必要（`claude` CLI） |

:::note
tmux がインストールされていない場合でも tako は動作しますが、再起動時のセッション復元ができません。`brew install tmux` でインストールすることを推奨します。
:::

## トラブルシューティング

### `tako` コマンドが見つからない

PATH が通っていない可能性があります。Homebrew でインストールした場合は自動登録されますが、ZIP からインストールした場合は上記の「PATH を通す」手順を確認してください。

### MCP ツールが認識されない

1. `tako setup-mcp` を再度実行してみてください
2. Claude Code を一度終了し、再起動してみてください
3. `~/.claude/settings.json` に `tako` の項目があるか確認してください

### tako 起動時にクラッシュする

`xattr` コマンドで quarantine 属性を解除してから再度起動してください:

```bash
xattr -dr com.apple.quarantine /Applications/tako.app
```

## 次のステップ

- [タブ＆ペイン管理](/features/tabs-and-panes/) — 画面分割やショートカットを覚える
- [内蔵 MCP サーバー](/features/mcp-server/) — AI が使える機能の全体像
- [CLI リファレンス](/guides/cli-reference/) — `tako` コマンド一覧
