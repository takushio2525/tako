---
title: オーケストレーター
description: tako master で複数プロジェクトの作業を子 claude worker に委任・管理
---

tako に内蔵されたマスターオーケストレーター機能の使い方です。「オーケストレーションとは何か」から知りたい方は、先に[オーケストレーションとは](/features/orchestration/)をご覧ください。

## 概要

`tako master` を実行すると、専用の system prompt（司令塔としての指示書）付きで claude が新しいタブに起動します。このマスターに「あのプロジェクトのあれやって」と話しかけると、子 worker を tako のペインに spawn（起動）して作業を委任し、完了まで監視します。外部スクリプト依存ゼロです。

```
tako master（マスター claude）
  ├── worker 1（webapp の機能実装）
  ├── worker 2（API サーバーのバグ修正）
  └── worker 3（ドキュメント更新）
```

## セットアップ

```bash
# 1. セットアップ（未実施なら。MCP 登録を含む）
tako setup

# 2. master 起動
tako master
```

初回起動時に設定ディレクトリ（`~/Library/Application Support/tako/orchestrator/`）が自動作成されます。

## プロジェクト管理

オーケストレーターが作業対象にできるプロジェクト（リポジトリや作業フォルダ）を登録します。master はこの登録情報から作業ディレクトリを解決します。

```bash
# プロジェクトを登録
tako orchestrator projects add --key webapp --cwd ~/Documents/webapp --description "Web アプリケーション"

# 一覧表示
tako orchestrator projects list

# 削除
tako orchestrator projects remove --key webapp
```

設定は `~/Library/Application Support/tako/orchestrator/projects.yaml` に保存されます。直接編集しても構いません。

```yaml
projects:
  webapp:
    cwd: ~/Documents/webapp
    description: Web アプリケーション
  api-server:
    cwd: ~/Documents/api-server
    description: REST API サーバー
```

## プロファイル（モデル・effort の設定）

master と worker がどのモデル・思考量（effort）で動くかは**プロファイル**で決まります。設定ファイルの実体は `~/Library/Application Support/tako/orchestrator/profiles/<名前>.yaml` で、コマンドからも変更できます。

```bash
# 一覧（model: null = claude CLI の既定モデルで起動）
tako orchestrator profiles list

# 内容の表示（名前省略時は default）
tako orchestrator profiles show

# モデル・effort を設定
tako orchestrator profiles set default --model claude-opus-4-6 --effort max

# モデル指定を解除して claude 既定に戻す
tako orchestrator profiles set default --clear-model
```

- **既定はモデル無指定**です。claude CLI の既定モデルで起動するため、どの Claude プランでも動作します
- `[1m]` 付きモデル（1M コンテキスト版）は Max / API プラン限定です。指定すると起動時に警告が出ます
- worker のモデルはプロファイルの `worker_model_policy` で決まります: `inherit`（master と同じ・既定）/ `fixed`（別の固定モデル）/ `delegate`（master がタスク内容を見て判断）

## Worker の起動と監視

通常は master に自然言語で話しかけるだけで worker の起動・監視・回収まで自動で回ります。CLI から手動で操作することもできます。

```bash
# worker を spawn（登録済みプロジェクトのディレクトリで claude を起動し、プロンプトを渡す）
tako orchestrator spawn --project webapp --prompt "ログインページを実装して"

# worker の状態を 1 回確認
tako orchestrator status --pane <N>

# worker の完了までブロックして待つ（WORKER_IDLE = 完了、WORKER_GONE = 消滅）
tako orchestrator watch --pane <N> --session-id <S>

# spawn → 完了待ち → 出力回収 → ペイン片付け、をワンショットで
tako orchestrator run --project webapp --prompt "テストを実行して失敗があれば直して"
```

各コマンドの全オプションは [CLI リファレンス](/guides/cli-reference/#オーケストレーター)を参照してください。

### 確実なプロンプト送達

worker への指示文は「貼り付け → Enter 送信 → 入力欄が空になったかの検証（残っていれば再送）」という送達確認ループで配送されます。長いマルチライン指示が入力欄に残ったまま放置される事故を仕組みで防いでいます。また、初回フォルダの信頼確認ダイアログは起動前に自動処理されるため、指示がダイアログに吸われることもありません。

### MCP ツール

master（または任意の claude エージェント）は MCP ツールからも同じ操作ができます。

- `tako_orchestrator_spawn` — worker を起動（`tab` / `pane` パラメータで出力先を指定可能）
- `tako_orchestrator_run` — ワンショット実行
- `tako_orchestrator_worker_status` — worker の状態を確認
- `tako_orchestrator_projects` — プロジェクト管理
- `tako_orchestrator_profiles` — プロファイル管理

## 複数マスター・プロファイル切替

```bash
# プロファイルを指定して起動（profiles/fast.yaml の設定を使う）
tako master -fast

# サフィックス指定で複数マスターを区別（旧形式・後方互換）
tako master frontend   # "master-frontend" タブ
tako master backend    # "master-backend" タブ
```

各マスターは自分が起動した worker を識別して監視します。「フロントエンド担当」「バックエンド担当」のように役割の違うマスターを並走させられます。

## system prompt のカスタマイズ

master の system prompt はバイナリに埋め込まれた既定のものが使われますが、`~/Library/Application Support/tako/orchestrator/master-system.md` にファイルを置くと、そちらが優先されます。
