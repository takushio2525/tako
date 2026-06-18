# tako オーケストレーター機能

tako に内蔵されたマスターオーケストレーター機能。複数プロジェクトの作業を
子 claude worker に委任し、監視・管理する。外部スクリプト依存ゼロ。

## 前提条件

- tako がインストール済み（`tako` CLI が PATH に通っている）
- `claude` CLI がインストール済み（`claude --version` で確認）
- tako MCP が登録済み（`tako setup-mcp` で自動登録）

## セットアップ

```bash
# 1. MCP 登録（初回のみ）
tako setup-mcp

# 2. master 起動（初回は自動で設定ディレクトリとテンプレートを生成）
tako master
```

初回起動時に `~/Library/Application Support/tako/orchestrator/` が作成され、
空の `projects.yaml` が配置される。

## projects.yaml

プロジェクトの定義ファイル。配置場所:
`~/Library/Application Support/tako/orchestrator/projects.yaml`

```yaml
projects:
  webapp:
    cwd: ~/Documents/webapp
    description: Web アプリケーション
  api-server:
    cwd: ~/Documents/api-server
    description: REST API サーバー
  docs:
    cwd: ~/Documents/docs-site
    description: ドキュメントサイト
```

- `cwd`: 作業ディレクトリ（`~` は `$HOME` に展開される）
- `description`: 説明（任意）

### CLI でプロジェクトを管理する

```bash
# 一覧
tako orchestrator projects list

# 追加
tako orchestrator projects add --key webapp --cwd ~/Documents/webapp --description "Web アプリ"

# 削除
tako orchestrator projects remove --key webapp
```

## 基本的な使い方

### 1. master を起動する

```bash
tako master
```

新しいタブに claude がマスター system prompt 付きで起動する。
suffix を付けると複数 master を区別できる:

```bash
tako master dev     # "master-dev" タブ
tako master blog    # "master-blog" タブ
```

### 2. master に作業を依頼する

master タブで自然言語で依頼する:

> 「webapp の認証周りにテストを追加して」

master は:
1. projects.yaml から `webapp` の cwd を解決
2. 子 worker を spawn（右に分割された新ペイン）
3. worker に適切なプロンプトを渡す
4. Monitor で完了を監視
5. 完了したら結果を報告し、worker を kill

### 3. 完了通知を受け取る

master が Monitor で監視しているため、worker が完了すると自動で通知される。
master は結果を確認してユーザーに報告する。

## CLI リファレンス

### `tako master [suffix]`

新タブでマスターオーケストレーターを起動する。

| オプション | 説明 |
|---|---|
| `suffix` | タブ名のサフィックス（省略時は "master"） |

### `tako orchestrator projects list`

登録済みプロジェクトの一覧を表示する。

### `tako orchestrator projects add`

プロジェクトを追加する。

| オプション | 必須 | 説明 |
|---|---|---|
| `--key` | ○ | プロジェクトキー |
| `--cwd` | ○ | 作業ディレクトリ |
| `--description` | | 説明 |

### `tako orchestrator projects remove`

プロジェクトを削除する。

| オプション | 必須 | 説明 |
|---|---|---|
| `--key` | ○ | プロジェクトキー |

### `tako orchestrator spawn`

子 worker を spawn する。

| オプション | 必須 | 説明 |
|---|---|---|
| `--project` | ○ | プロジェクトキー |
| `--prompt` | ○ | worker に渡すプロンプト |
| `--label` | | ペインタイトルのラベル |
| `--model` | | claude のモデル（既定: claude-opus-4-6[1m]） |
| `--effort` | | thinking effort（既定: max） |

### `tako orchestrator status`

worker の状態を確認する。

| オプション | 必須 | 説明 |
|---|---|---|
| `--pane` | ○ | ペイン ID |
| `--session-id` | | claude の session ID |

### `tako orchestrator watch`

worker が完了するまでブロックし、1 行出力する。Monitor から呼ばれる想定。

| オプション | 必須 | 説明 |
|---|---|---|
| `--pane` | ○ | ペイン ID |
| `--session-id` | | claude の session ID |

出力形式:
- `WORKER_IDLE: tako:<pane> (ctx NN%)` — 完了 / 入力待ち
- `WORKER_GONE: tako:<pane>` — ペイン消滅

## MCP ツール

master（または任意の claude エージェント）から使える MCP ツール:

| ツール | 説明 |
|---|---|
| `tako_orchestrator_projects` | プロジェクト管理（list / add / remove） |
| `tako_orchestrator_spawn` | worker の spawn |
| `tako_orchestrator_worker_status` | worker の状態確認 |

既存の tako MCP ツール（`tako_read_pane` / `tako_send_input` / `tako_close_pane` 等）
と組み合わせて worker のライフサイクルを管理する。

## system prompt のカスタマイズ

デフォルトの system prompt はバイナリに埋め込まれている。カスタマイズしたい場合は:

```
~/Library/Application Support/tako/orchestrator/master-system.md
```

にファイルを配置する。このファイルが存在すれば、デフォルトより優先して使われる。
