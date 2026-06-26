---
title: セットアップ
description: tako のダウンロードからインストール、AI エージェント連携までの手順
---

tako を使い始めるまでの手順を説明します。所要時間は 5 分程度です。

## 1. ダウンロード

[GitHub Releases](https://github.com/takushio2525/tako/releases) から最新の `tako-vX.X.X-macos.zip` をダウンロードします。

## 2. インストール

```bash
# ダウンロードした zip を展開
unzip tako-*.zip

# /Applications に配置
mv tako.app /Applications/
```

### Gatekeeper の警告が出た場合

初回起動時に「開発元を確認できない」という警告が出ることがあります。

1. `tako.app` を右クリック → 「開く」を選択
2. または「システム設定 → プライバシーとセキュリティ → このまま開く」をクリック

一度許可すれば、以降は通常のアプリと同じように起動できます。

## 3. 起動

`/Applications/tako.app` をダブルクリックで起動します。初回起動時、通常のターミナルと同じようにシェルが 1 ペイン開きます。

## 4. MCP サーバーの登録（初回のみ）

tako 内蔵の MCP サーバーを Claude Code に登録します。tako のターミナルで以下を実行してください。

```bash
tako setup-mcp
```

このコマンドは `~/.claude/settings.json` に tako の MCP サーバーを自動登録します。一度だけ実行すればどのプロジェクトでも有効です。

:::tip
`tako setup-mcp --project` とすると、現在のプロジェクトスコープのみに登録できます。
:::

## 5. AI が tako を使い始める

MCP 登録が完了したら、tako 内で Claude Code を起動するだけです。

```bash
claude
```

Claude Code は自動的に tako の MCP ツールを認識し、以下のような操作ができるようになります。

- 「隣のペインで `npm run dev` を起動して」→ ペインを分割してコマンド実行
- 「このファイルをプレビューで見せて」→ ファイルをプレビューペインで表示
- 「今のレイアウトを見せて」→ タブ・ペイン構成の一覧取得

AI は環境変数（`TAKO_PANE_ID` など）から自分がどのペインにいるかを自動認識するため、明示的な設定は不要です。

## 動作環境

| 項目 | 要件 |
|---|---|
| OS | macOS（Windows 対応は開発中） |
| Node.js | 不要（ネイティブアプリ） |
| tmux | 推奨（再起動復元に使用。未インストールでも動作） |
| Claude Code | AI 連携に必要（`claude` CLI） |

## 次のステップ

- [タブ＆ペイン管理](/features/tabs-and-panes/) — 画面分割やショートカットを覚える
- [内蔵 MCP サーバー](/features/mcp-server/) — AI が使える機能の全体像
- [CLI リファレンス](/guides/cli-reference/) — `tako` コマンド一覧
