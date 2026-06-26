---
title: CLI リファレンス
description: tako コマンドの一覧と使い方
---

`tako` CLI はターミナルの操作をシェルスクリプトや AI エージェントから行うためのツールです。

## 基本操作

### tako split

ペインを分割して新しいペインを作成します。

```bash
# 右に分割
tako split --right

# 下に分割
tako split --down

# 分割してコマンドを実行
tako split --right -- npm run dev

# 比率を指定（0.0〜1.0）
tako split --right --ratio 0.3 -- htop
```

### tako send

指定ペインにテキストやキー入力を送信します。

```bash
# テキスト送信
tako send <pane-id> "echo hello"

# 改行付き（Enter キー相当）
tako send <pane-id> "npm run dev\n"
```

### tako read

ペインの画面内容を取得します。

```bash
# 現在の画面内容
tako read <pane-id>

# 行数を指定（スクロールバック含む）
tako read <pane-id> --lines 100
```

### tako focus

指定ペインにフォーカスを移動します。

```bash
tako focus <pane-id>
```

### tako list

タブ・ペインの構成を JSON で出力します。

```bash
tako list
```

各ペインの情報（ID・タイトル・cwd・状態・listen ポートなど）を含む構造化データが返ります。

### tako close

ペインを閉じます（プロセスを終了）。

```bash
tako close <pane-id>
```

### tako title

ペインのタイトルを設定します。

```bash
tako title <pane-id> "dev server"
```

## レイアウト操作

### tako resize

ペインのサイズを調整します。

```bash
tako resize <pane-id> --width 0.6
tako resize <pane-id> --height 0.4
```

### tako equalize

タブ内の全ペインを均等サイズに調整します。

```bash
tako equalize
```

## タブ操作

```bash
# 新しいタブを作成
tako tab new

# タブを切り替え
tako tab select <tab-index>

# タブ名を変更
tako tab rename <tab-index> "API Server"

# ペインを別タブに移動
tako tab move-pane <pane-id> <tab-index>
```

## ファイル操作

```bash
# ファイルをプレビューで開く
tako open <file-path>

# ファイル操作
tako file copy <src> <dest>
tako file move <src> <dest>
tako file rename <src> <new-name>
tako file delete <path>
tako file mkdir <path>
```

## たまり場

```bash
# ペインを退避
tako shelve <pane-id>

# 退避中ペイン一覧
tako shelved

# 退避ペインを復帰
tako unshelve <pane-id>
```

## tmux 管理

```bash
# tmux セッション一覧
tako tmux list

# tmux セッションを kill
tako tmux kill <session-name>

# 外部 tmux セッションをタブに取り込み
tako tmux open <session-name>

# tmux window を選択
tako tmux select-window <session> <window-index>

# orphan セッションの一括掃除
tako tmux cleanup
```

## git

```bash
# コミットログ
tako git log

# diff
tako git diff
```

## パネル表示

```bash
# 右サイドバーの表示/非表示
tako panel --tmux
tako panel --git
tako panel --filetree
```

## 設定

```bash
# tmux バックエンド（永続化）の ON/OFF
tako persist on|off

# ポート検知の ON/OFF
tako portdetect on|off

# 自動リネームの ON/OFF
tako autorename on|off

# たまり場のタブ折りたたみ
tako collapse --tab <N> on|off
```

## MCP

```bash
# Claude Code に MCP サーバーを登録（初回のみ）
tako setup-mcp

# プロジェクトスコープで登録
tako setup-mcp --project

# MCP stdio ブリッジを起動（通常は自動）
tako mcp serve
```

## オーケストレーター

```bash
# マスターを起動
tako master [suffix]

# worker を spawn
tako orchestrator spawn --project <key> --prompt "..."

# worker の状態を監視
tako orchestrator watch --pane <N> --session-id <S>

# プロジェクト管理
tako orchestrator projects list
tako orchestrator projects add <key> <cwd> [description]
tako orchestrator projects remove <key>
```

## ペイン ID の自動特定

tako のペイン内から `tako` コマンドを実行すると、環境変数 `TAKO_PANE_ID` から呼び出し元ペインが自動特定されます。ペイン ID を省略した場合は呼び出し元がデフォルト対象になります。

tako の外（通常のターミナルや tmux）から実行した場合は、接続情報ファイルへのフォールバックを試み、接続できない場合は明確なエラーメッセージを返します。
