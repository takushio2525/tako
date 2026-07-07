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

## プロファイルと設定の優先順位（Issue #27 で明文化）

master / worker の起動設定（モデル・effort・worker ポリシー）の**唯一の正は
`profiles/*.yaml`**（`~/Library/Application Support/tako/orchestrator/profiles/`）。

```yaml
# profiles/default.yaml の例
# model 未指定 = claude CLI の既定モデルで起動する（プラン非依存・推奨）
#   model: claude-opus-4-6        … モデルを固定する場合
#   model: claude-opus-4-6[1m]    … 1M コンテキスト版（Max / API プラン限定）
effort: max
worker_model_policy: inherit
```

- **model 未指定（キー自体を書かない）**: `--model` を付けずに claude を起動し、
  claude CLI の既定モデルに委ねる。**これが既定**（どのプランでも確実に起動する）
- **`[1m]` 付きモデル**: 1M コンテキスト版は Max / API プラン限定。**明示 opt-in のみ**。
  Pro プランでは master が起動できないため、起動時に警告が出る
- **旧バージョンからのマイグレーション**: 0.2.3 以前が default.yaml に書き込んだ
  `model: claude-opus-4-6[1m]`（旧既定値と完全一致の場合のみ）は、`tako master` /
  `tako setup` / spawn 時に自動で除去される（`default.yaml.backup-1m` にバックアップ）。
  ユーザーが別の値を明示した場合は尊重され、警告のみ
- **config.yaml はモデル設定を持たない**: `config.yaml` は setup 状態（completed）と
  挙動フラグ（auto_close / auto_push）のみ。旧バージョンの `master_model` /
  `worker_model` / `effort` キーは廃止済みで、残っていても**無視される**

モデル解決の優先順位:

| 対象 | 優先順位 |
|---|---|
| master | プロファイルの `model` → 未指定なら claude CLI 既定 |
| worker | spawn の `model` 引数 → プロファイルの worker ポリシー（inherit / fixed / delegate）→ 未指定なら claude CLI 既定 |

### CLI でプロファイルを管理する

```bash
# 一覧（model: null は claude 既定で起動することを表す）
tako orchestrator profiles list

# 表示（名前省略時は default）
tako orchestrator profiles show [名前]

# モデルを設定（[1m] 付きは Max / API プラン限定の警告が出る）
tako orchestrator profiles set default --model claude-opus-4-6 --effort max

# モデル指定を解除して claude 既定に戻す
tako orchestrator profiles set default --clear-model
```

MCP からは `tako_orchestrator_profiles`（action: list / show / set）で同じ操作ができる。

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
| `--model` | | claude のモデル（省略時は master のプロファイル → 未設定なら claude 既定） |
| `--effort` | | thinking effort（省略時は master のプロファイル設定） |

プロンプト送達は送達確認ループで行う（Issue #32）:

1. **事前信頼**: spawn 時に `~/.claude.json` の `projects.<cwd>.hasTrustDialogAccepted` を
   立て、初回フォルダの信頼ダイアログ自体を出さない（ダイアログが送信プロンプトを
   消費する問題の根治）。書けなかった場合もダイアログ検出 → Enter 承諾でフォールバック
2. **貼り付けと送信の分離**: プロンプト本体は bracketed paste で入力欄へ貼り、送信の
   Enter は分離した単独キーとして遅延送信する（マルチラインもそのまま渡る。
   改行の 2 スペース平坦化は廃止）
3. **送達検証**: 送信後に入力欄が空へ戻ったことを画面で検証し、残っていれば Enter を
   単独再送する（最大 4 回）

`tako send --await-prompt` / MCP `tako_send_input`（newline つき）も同じループで配送される。

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
| `tako_orchestrator_profiles` | プロファイル管理（list / show / set。モデル・effort の設定と解除） |
| `tako_orchestrator_spawn` | worker の spawn |
| `tako_orchestrator_worker_status` | worker の状態確認 |

既存の tako MCP ツール（`tako_read_pane` / `tako_send_input` / `tako_close_pane` 等）
と組み合わせて worker のライフサイクルを管理する。

## 品質パイプライン（全プロファイル共通）

6 PR 横断レビュー（2026-07-03）で得た運用知見を、Issue #100（2026-07-07）で
default system prompt の「品質パイプライン」として手順・型に再構成した。
プロファイル固有のモデル振り分け（`delegate_guidance`）とは独立で、
全 master に常に適用される。ブロックと役割:

| ブロック | 内容 |
|---|---|
| `task-intake` | 依頼の列挙 → 1 worker = 1 成果物の割り当て（統合の例外 = 同一ファイル / パイプライン依存 / リポ変更なし、の閉じたリスト）→ 並列/直列判定 → 分担計画の提示と同ターン spawn |
| `worker-prompt-template` | worker プロンプトの必須の型（Task / Background / Scope / Constraints / 受け入れ条件 / 検証手順 / Git / 証拠つき報告様式）。根因先行（バグは再現・根因を Background に書いてから委任）・要件密着タスクの転記ルール込み |
| `acceptance` | 完了報告の受け入れ検査: 受け入れ条件×証拠の突き合わせ → diff スポットチェック →「A を B に」系は実コード確認 → 機械検証不能領域は操作ログ/スクショ必須（無ければ「未検証」報告）→ 差し戻しは欠陥リストで・2 回失敗で方針再考 → Closes 判断は master |
| `quality-ops` | 横断規律: 同一ファイル直列化 / 複数 PR 後の統合レビュー worker / done = push → PR → merge まで |
| `monitoring` | WORKER_IDLE 空振り対策（通知を鵜呑みにせず read_pane で確認・thinking 中の respawn 禁止・立て直し条件は閉じたリスト） |

カスタム `master-system.md` / profiles の `system_prompt` を使っている場合は
これらの更新が反映されない（setup changelog rev 5 の guided 手順が
prompt_blocks への移行を案内する）。worker 側の品質ゲートは setup が配る
CLAUDE.md セクションテンプレート `06-completion-verification` が対になる。

## system prompt のカスタマイズ

デフォルトの system prompt はバイナリに埋め込まれている。カスタマイズしたい場合は:

```
~/Library/Application Support/tako/orchestrator/master-system.md
```

にファイルを配置する。このファイルが存在すれば、デフォルトより優先して使われる。
