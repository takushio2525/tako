---
title: Claude Code
description: tako の基準となるエージェント CLI。導入からログイン、MCP 接続、worker としての使い方まで
---

`claude`（Claude Code）は **tako が基準にしている系統**です。tako のすべての機能はまず Claude Code で実装され、ほかの系統はそこへ追いつく形で作られています。落ちる機能はありません（[対応状況](/agent-support/) の 47 件すべてが「対応」）。

## 入れる

tako から入れる場合は次の 1 行です。**管理者権限は使わず**、ホームディレクトリの中だけで完結します。

```bash
tako setup bootstrap install
```

何をどこに入れるかは実行前に表示されます。先に見るだけなら `tako setup bootstrap install --dry-run`、いま入っているかどうかだけなら `tako setup bootstrap status` です。

公式の手順を直接使っても構いません。tako が実行するのも同じものです。

```bash
curl -fsSL https://claude.ai/install.sh | bash
```

Windows では `irm https://claude.ai/install.ps1 | iex` が公式の手順で、**tako から実行を代行できるのは 3 系統のうち claude だけ**です（codex / agy は状態の確認と案内までです）。

入ったあとの本体は `~/.local/bin/claude` に置かれ、更新は Claude Code が自分でバックグラウンドで行います。

## ログインする

```bash
claude auth login
```

ブラウザでの操作が要るため **tako はログインを代行しません**。ログインが済んだら `tako setup` をやり直してください。

## tako に繋ぐ（MCP）

```bash
tako setup-mcp
```

書き込み先は `~/.claude.json` です。現在のディレクトリだけに入れたいときは `tako setup-mcp --project` で `<cwd>/.mcp.json` に書き込みます（`--project` に対応しているのは claude だけです）。

一度登録すればどのプロジェクトでも有効です。`tako setup` を実行していれば登録は済んでいるので、通常このコマンドを打つ必要はありません。tako の MCP サーバーの中身は [内蔵 MCP サーバー](/features/mcp-server/) にあります。

:::note[`~/.claude/settings.json` ではありません]
以前の tako は `~/.claude/settings.json` へ書いていました。現在の書き込み先は `~/.claude.json`（グローバル）と `<cwd>/.mcp.json`（プロジェクト）で、古い `settings.json` に残った tako の設定は `tako setup-mcp` が掃除します。
:::

## master として使う

既定の系統なので、プロファイルを触らずにそのまま起動できます。

```bash
tako master
```

使い方は [tako master 実践ガイド](/features/orchestrator/) にあります。

## worker として使う

こちらも既定です。`--agent` を省いた spawn は claude で立ちます。

```bash
tako orchestrator spawn --project app --prompt "テストを通してください"
```

ほかの系統を既定にしている環境で、この worker だけ claude にしたいときは `--agent claude` を付けてください。
