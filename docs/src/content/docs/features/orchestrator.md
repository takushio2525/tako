---
title: オーケストレーター
description: tako master で複数プロジェクトの作業を子 claude worker に委任・管理
---

tako に内蔵されたマスターオーケストレーター機能。複数プロジェクトの作業を子 claude worker に委任し、監視・管理します。外部スクリプト依存ゼロ。

## 概要

`tako master` を実行すると、専用の system prompt 付きで claude が起動します。このマスターに「あのプロジェクトのあれやって」と指示すると、子 worker を tako のペインに spawn して作業を委任します。

```
tako master（マスター claude）
  ├── worker 1（webapp の機能実装）
  ├── worker 2（API サーバーのバグ修正）
  └── worker 3（ドキュメント更新）
```

## セットアップ

```bash
# 1. MCP 登録（未登録の場合）
tako setup-mcp

# 2. master 起動
tako master
```

初回起動時に設定ディレクトリ（`~/Library/Application Support/tako/orchestrator/`）が自動作成されます。

## プロジェクト管理

オーケストレーターが作業対象にできるプロジェクトを登録します。

```bash
# プロジェクトを登録
tako orchestrator projects add webapp ~/Documents/webapp "Web アプリケーション"

# 一覧表示
tako orchestrator projects list

# 削除
tako orchestrator projects remove webapp
```

設定は `~/Library/Application Support/tako/orchestrator/projects.yaml` に保存されます。

```yaml
projects:
  webapp:
    cwd: ~/Documents/webapp
    description: Web アプリケーション
  api-server:
    cwd: ~/Documents/api-server
    description: REST API サーバー
```

## Worker の起動と監視

マスター claude に話しかけて worker を起動します。MCP ツールを使って直接操作することもできます。

```bash
# worker を spawn（特定プロジェクトのペインで claude を起動）
tako orchestrator spawn --project webapp --prompt "ログインページを実装して"

# worker の状態確認
tako orchestrator watch --pane <N> --session-id <S>
```

### MCP ツール

- `tako_orchestrator_spawn` — worker を起動（`tab` パラメータで出力先タブを指定可能）
- `tako_orchestrator_worker_status` — worker の状態を確認
- `tako_orchestrator_projects` — プロジェクト一覧の管理

## 複数マスター

`tako master` に suffix を付けると、複数のマスターを同時に起動できます。

```bash
tako master frontend   # フロントエンド担当
tako master backend    # バックエンド担当
```

各マスターは spawn 時に suffix をマッチして自分が起動した worker を識別します。
