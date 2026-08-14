# tako オーケストレーター機能

tako に内蔵されたマスターオーケストレーター機能。複数プロジェクトの作業を
子 worker に委任し、監視・管理する。外部スクリプト依存ゼロ。
worker のエージェント CLI は **claude（既定）/ codex / agy** から選べる（Issue #120）。
master / solo のエージェント CLI も **claude（既定）/ codex** から選べる（Issue #127。
agy は master 非対応 = worker のみ）。

## オーケストレーション不要なら `tako solo`

worker への分担が要らず、AI と 1 対 1 で直接作業したいだけなら `tako solo` を使う。
master と違い**オーケストレーション（worker spawn / sub-agent / Workflow）を禁止**し、
solo セッション自身がファイル編集・テスト・コミットを直接行う。エコ運用（既定
`effort=high`。master の `max` より低い）で Pro プランでも使いやすい。

```bash
tako solo            # default プロファイル、role=solo
tako solo -fast      # "fast" プロファイル（solo-profiles/fast.yaml）、role=solo:fast
tako solo docs       # 旧形式サフィックス、role=solo:docs
```

- プロファイル引数パターンは master と同一（`-<名前>` = プロファイル、引数なし = default、
  裸の語 = 後方互換サフィックス）。設定は `~/Library/Application Support/tako/orchestrator/solo-profiles/`
- `projects.yaml` は master と共有。solo は `tako_orchestrator_projects` でプロジェクトの
  作業ディレクトリを引き、`cd` して直接作業する（「demo の README 直して」で通る）
- 「最近やってること」は起動時にロードせず、必要なとき各プロジェクトで `git log` を見る

## 前提条件

- tako がインストール済み（`tako` CLI が PATH に通っている）
- `claude` CLI がインストール済み（`claude --version` で確認）
- tako MCP が登録済み（`tako setup-mcp` で自動登録）
- codex master / worker・agy worker を使う場合はそれぞれの CLI（`codex` / `agy`）がインストール済みであること（任意）

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
| master | プロファイルの `model`（`master_agent` のネイティブ表記）→ 未指定ならその CLI の既定 |
| worker（claude） | spawn の `model` 引数 → `worker_agents.claude.model` → プロファイルの worker ポリシー（inherit / fixed / delegate）→ 未指定なら claude CLI 既定 |
| worker（codex / agy） | spawn の `model` 引数 → `worker_agents.<agent>.model` → 未指定ならその CLI の既定 |

### master のエージェント種別（claude / codex。Issue #127）

master / solo は claude 以外に codex でも起動できる。プロファイルに `master_agent` を書く:

```yaml
# profiles/<name>.yaml — codex master の例（tako master -<name> で起動）
master_agent: codex     # 省略時 claude（完全後方互換）。agy は master 非対応
model: gpt-5.6-sol      # master_agent のネイティブ表記で書く
effort: xhigh           # codex: none/minimal/low/medium/high/xhigh/max/ultra
```

- **system prompt**: codex には `-c developer_instructions="$(cat <プロンプトファイル>)"` で
  注入する（developer ロールメッセージとしてモデル可視プロンプトに入る）。
  プロンプト合成（prompt_blocks / Session Identity）は claude master と共通
- **MCP 接続**: 起動コマンドに `-c mcp_servers.tako.*` を一時注入する（`~/.codex/config.toml`
  は汚さない。tako 外で起動した codex にはツールが出ない）。`env_vars`（親環境からの
  引き継ぎホワイトリスト）で `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` / `TAKO_TAB_ID` /
  `TAKO_ORCHESTRATOR_ROLE` が stdio ブリッジ（`tako mcp serve`）へ渡る
- **worker への波及ガード**: `master_agent` が claude 以外のとき、プロファイルの `model` /
  `effort` は claude worker へ**継承されない**（inherit / delegate / fixed フォールバックの
  全経路で claude CLI 既定 / max に落ちる）。codex master + claude worker を混在させる場合、
  worker のモデルは `worker_agents.claude` か `worker_model`（fixed）で明示する
- **agy が master 非対応の理由**: agy の MCP 設定（JSON ファイル）にはペイン毎の接続情報
  （TAKO_* 環境変数）を子プロセスへ引き継ぐ手段が無く、system prompt 注入オプションも無い。
  `master_agent: agy` は設定時・起動時ともに明示エラーになる
- **初回のフォルダ信頼**: codex は初回起動時に作業フォルダの信頼確認を出すことがある。
  master は対話セッションなのでその場で承認すればよい（worker と違い事前信頼は書き込まない）

### worker のエージェント種別（claude / codex / agy。Issue #120）

worker は claude 以外のコーディングエージェント CLI でも起動できる。
プロファイルに既定種別とエージェント別設定を書く:

```yaml
# profiles/<name>.yaml
effort: max
worker_agent: codex          # 省略時 claude。spawn の agent 引数で個別上書き可
worker_agents:               # エージェント別の worker 設定（任意）
  codex:
    model: gpt-5.6-terra     # CLI ネイティブ表記
    effort: medium           # codex は -c model_reasoning_effort= へ写像
  agy:
    model: "Gemini 3.5 Flash (High)"   # agy はモデル表示名。effort はモデル名に組込みのため無視
    skip_permissions: false  # 明示 false で承認ダイアログを有効化（agy / codex は既定 true）
    args: []                 # 追加 CLI 引数（上級者向け）
```

- **effort の写像**: claude `--effort` / codex `-c model_reasoning_effort=`（low/medium/high/xhigh/max/ultra）/ agy 無視
- **skip_permissions**: codex / agy は**既定で承認スキップ**（worker が承認ダイアログで停止するのを防ぐ）。
  claude は既定で承認あり（Claude Code 側の設定に委ねる）。プロファイルで `skip_permissions: false`
  を明示するとどのエージェントでも承認ありに戻る。claude・agy は `--dangerously-skip-permissions`、
  codex は `--dangerously-bypass-approvals-and-sandbox` を付けて起動する
- **status 検知**: codex / agy は `claude agents --json` に現れないため常に画面推定
  （status_source=screen、idle 連続 8 回で完了判定）。claude worker より完了検知が数十秒遅くなる
- **事前信頼**: spawn 時に各 CLI の信頼設定（claude: `~/.claude.json` / codex:
  `~/.codex/config.toml` / agy: `~/.gemini/antigravity-cli/settings.json`）へ書き込み、
  信頼ダイアログ自体を出さない。書けなかった場合もダイアログ検出 → Enter 承諾でフォールバック
- **タスク振り分け**: `worker_model_policy: delegate` + `delegate_guidance` に
  「軽い調査は codex (gpt-5.6-luna)、重実装は claude」等と書くと、master がタスクごとに
  agent / model を選んで spawn する（system prompt に Available Worker Agents 一覧が注入される）

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

# worker の既定エージェント種別と、エージェント別の worker 設定（#120）
tako orchestrator profiles set default --worker-agent codex
tako orchestrator profiles set default --agent codex --agent-model gpt-5.6-terra --agent-effort medium
tako orchestrator profiles set default --agent agy --agent-model "Gemini 3.5 Flash (High)" --agent-skip-permissions true
tako orchestrator profiles set default --clear-worker-agent   # claude 既定へ戻す

# master のエージェント種別（#127。model / effort はそのエージェントのネイティブ表記で）
tako orchestrator profiles set sol --master-agent codex --model gpt-5.6-sol --effort xhigh
tako orchestrator profiles set sol --clear-master-agent       # claude 既定へ戻す

# 自動ハンドオフ（#749。閾値は 50〜60。範囲外はエラー）
tako orchestrator profiles set default --ctx-threshold 55
tako orchestrator profiles set default --auto-handoff false   # 自動通知だけ止める
tako orchestrator profiles set default --clear-ctx-threshold   # config.yaml → 既定 60 へ
```

MCP からは `tako_orchestrator_profiles`（action: list / show / set。master_agent /
worker_agent / agent_* パラメータ対応）で同じ操作ができる。

### アカウント（accounts.yaml。Issue #504 / #512）

worker を別の claude アカウントで動かすための名前つきレジストリ。
`<data_dir>/orchestrator/accounts.yaml` に置き、CLI `tako orchestrator accounts`
（list / show / add / remove）または MCP `tako_orchestrator_accounts`（同じ action）で編集する
（両者は同じ dispatch 関数を呼ぶので出力・警告・検証は完全に一致する）。
使うのは spawn の `--account`、プロファイルの `master_account` / `worker_account`。

```bash
tako orchestrator accounts list
tako orchestrator accounts add personal --inherit --default-model claude-opus-5
tako orchestrator accounts add univ --config-dir ~/.claude-univ --default-model 'claude-opus-4-6[1m]'
tako orchestrator accounts remove univ
```

```yaml
accounts:
  univ:
    config_dir: ~/.claude-univ      # CLAUDE_CONFIG_DIR にこのパスを設定する
    default_model: claude-opus-4-6[1m]
  personal:
    inherit: true                   # CLAUDE_CONFIG_DIR を設定しない（既定の資格情報）
    default_model: claude-opus-5
```

`master_account` は `tako master` / `tako solo` / handoff の新 master に、
`worker_account` は spawn する worker に効く（spawn の `--account` が最優先。#547）。
起動時に「アカウント: <名前>（config dir: …）」を表示するので、どちらで立ったかは
コマンド出力で確認できる。登録していないアカウント名は起動前にエラーになる。

**既定アカウントは `config_dir: ~/.claude` ではなく `inherit: true` で書く**（#512）。
claude は `CLAUDE_CONFIG_DIR` が**設定されている**だけで Keychain のエントリ名に
ハッシュを付けるため、値が既定パスと同一でも別エントリ（= 未ログイン扱い）になる。
`inherit: true` の worker は起動コマンドの先頭で `unset CLAUDE_CONFIG_DIR;` を実行し、
direnv 等が設定してくる値も確実に消す。既定パスを明示指定して登録しようとすると
add が警告を返す。

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

### `tako master [-プロファイル]`

現在のペインでマスターオーケストレーターを起動する（インライン起動。#264）。

| オプション | 説明 |
|---|---|
| `-<プロファイル名>` | プロファイル指定（省略時は default。タブ名は `master-<名前>`。旧形式のサフィックス指定も後方互換で動く） |
| `--tab` | 常に新規タブで起動する |

起動先ペインは **pid 祖先辿り → `TAKO_PANE_ID` → stale pane map（#210）** の順で解決し、
どれでも特定できなければ「呼び出し元不明」として新規タブを作りそこで起動する（#567）。
アプリ再起動やシェルの再利用で `TAKO_PANE_ID` が古くなっていても起動は止まらない
（読み替え・フォールバックが起きたときは案内と `unset TAKO_PANE_ID` を表示する）。
`tako solo` も同じ解決順。

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
| `--agent` | | worker のエージェント CLI（claude / codex / agy。省略時はプロファイルの worker_agent → claude） |
| `--model` | | worker のモデル（agent のネイティブ表記。省略時はプロファイル設定） |
| `--effort` | | thinking / reasoning effort（claude・codex のみ。省略時はプロファイル設定） |
| `--account` | | アカウント名（accounts.yaml のキー。この worker だけ別アカウントで起動する。#504 / #511） |

プロンプト送達は 2 層構成（Issue #790）。claude worker には**まず受信箱へ直送**する
（claude の Cross-Session Messaging。socket 直送なので画面解析もキー操作も伴わず、
生成中でもキューに入って取りこぼさない。長文もそのまま届く）。使えない環境
（claude が古い / 受信箱を開いていない / codex・agy・Windows）では従来のキー操作経路へ
自動で落ちる。どちらを通ったかは `<data_dir>/persist.log` に `送達: peer …` /
`送達: keys 経路 …` として残る。**受信側には「別の claude セッションから届いた」旨の
定型文が付く**ので、この経路を使うのは worker 宛だけ（master への指示や承認の代行は
従来経路のまま = 人が打った指示として扱われる）。

従来のキー操作経路（フォールバック先）は送達確認ループで行う（Issue #32）:

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

worker の状態を確認する。status は busy / idle / error / gone / unknown。
error（異常停止。#157）のときは応答に `error.kind` / `error.detail` /
`error.recommended_action` が入る（種別は watch の表を参照）。

| オプション | 必須 | 説明 |
|---|---|---|
| `--pane` | ○ | ペイン ID |
| `--session-id` | | claude の session ID |

### `tako orchestrator watch`

worker が停止するまでブロックし、結果を出力する。Monitor から呼ばれる想定。

| オプション | 必須 | 説明 |
|---|---|---|
| `--pane` | ○ | ペイン ID |
| `--session-id` | | claude の session ID |

出力形式:
- `WORKER_IDLE: tako:<pane> (ctx NN%)` — 完了 / 入力待ち
- `WORKER_ERROR: tako:<pane> (<種別>)` — 既知の異常（API エラー・usage limit 等）で停止（#157）。
  続く行に `detail:`（検知した画面上の行）と `action:`（推奨リカバリ）が付く
- `WORKER_STALLED: tako:<pane>` — 停滞: 実行中子プロセスなし + 画面の busy パターンなし（#224）。
  続く行に `detail:` と `action: check_and_resume` が付く
- `WORKER_GONE: tako:<pane>` — ペイン消滅

WORKER_ERROR の種別と推奨リカバリ（`worker_status` の `error.kind` /
`error.recommended_action` と同一）:

| 種別 | 意味 | action | リカバリ |
|---|---|---|---|
| `api_error` | 一時的な API エラー（接続断等）で停止 | `resume` | 続行指示を send_input で再送 |
| `usage_limit` | usage limit 到達で停止 | `wait_reset` | 解除時刻まで待ってから続行指示 |
| `limit_dialog` | rate limit 起因の選択ダイアログ（codex のモデル切替等）で停止 | `respond_dialog` | ダイアログの選択肢に応答 |

`worker_status` は `has_running_children`（tmux セッション配下で実行中の子プロセスがあるか）と
`collapsed`（TUI 折りたたみ状態か）も返す。`collapsed` が true のとき read の画面テキストは
不完全な可能性があるため、`has_running_children` と `status` フィールドを優先的に使うこと

## MCP ツール

master（または任意の claude エージェント）から使える MCP ツール:

| ツール | 説明 |
|---|---|
| `tako_orchestrator_projects` | プロジェクト管理（list / add / remove） |
| `tako_orchestrator_profiles` | プロファイル管理（list / show / set。モデル・effort・worker_agent / agent_* の設定と解除、ctx_threshold / auto_handoff（#749）） |
| `tako_orchestrator_self` | 自分の pane / tab / ctx% / 引き継ぎ閾値の取得（`ctx_over_threshold` が true なら引き継ぎ時） |
| `tako_orchestrator_handoff` | 後任 master への引き継ぎ（handoff ファイルを読んで spawn。前任ペインは後任が閉じる） |
| `tako_orchestrator_spawn` | worker の spawn（agent パラメータで claude / codex / agy を選択） |
| `tako_orchestrator_worker_status` | worker の状態確認（codex / agy は画面推定。異常停止は status=error + error.kind / recommended_action（#157）。停滞は status=stalled + stalled.detail / recommended_action（#224）。has_running_children / collapsed フラグ付き） |

既存の tako MCP ツール（`tako_read_pane` / `tako_send_input` / `tako_close_pane` 等）
と組み合わせて worker のライフサイクルを管理する。

## master の自動ハンドオフ（Issue #749）

master のコンテキストが埋まると判断が劣化する。tako は `/compact` を自動実行せず
（会話の文脈が飛んで「話が通じなくなる」）、**新しい master へ乗り換える**。

流れは 4 手で、**ユーザーは何もしなくてよい**:

1. tako が master ペインの ctx% を見張り、閾値（既定 60%・50〜60% で設定可）を
   超えたら `【tako 自動通知】…` をそのペインへ送る
2. master が引き継ぎファイル（`<data_dir>/orchestrator/handoff/<プロファイル>.md`）を
   今の状況で上書きし、`tako_orchestrator_handoff` を呼ぶ
3. 後任 master が同じタブ・同じ role・同じプロファイルで立ち上がり、引き継ぎファイルと
   実態（`tako_orchestrator_workers` / `tako_list_panes`）を突き合わせて「引き継ぎ完了」を報告する。
   **起動は `tako master -<プロファイル>` と同一経路**なので、モデル・effort・アカウント・
   master system prompt はプロファイルの master 設定がそのまま効く（worker 用の
   `worker_agents` は使わない）。`TAKO_ORCHESTRATOR_ROLE` も CLI と同じ `master:<プロファイル>`
   形式で入るので、後任の `tako orchestrator self` は自分のプロファイルを正しく返す（#761）
4. 後任が前任ペインの入力欄を確認（ユーザーの未送達の指示が残っていないか）してから
   前任ペインを閉じる

**閉じるのは後任だけ**なので、後任の起動に失敗しても前任の master は失われない。

### 引き継ぎファイルの書式（Issue #792）

引き継ぎファイルは **2 節**に分ける。pane / tab 番号はこのマシンでしか意味を持たないので、
知識に混ぜたまま別マシンへ持ち込むと後任が**存在しないペイン**へ指示を出す（#513 の設定共有で
現実に起きる事故）。

```markdown
# master 引き継ぎ（profile: takodev）

## 知識（マシン非依存）
決定事項とその理由 / ユーザーの方針・好み / 残タスクとその意図 / 調べて分かったこと。
pane / tab 番号は書かない

## 実行状態（このマシン限定）
spawn 済み worker とその pane と依頼内容 / 開いているペイン / 実行中のもの。
別マシンでは丸ごと無効になる前提で書く
```

見出しは表示言語に合わせて英語（`## Knowledge (machine-independent)` /
`## Runtime state (this machine only)`）でもよい。判定は寛容で、番号付き（`## 1. 知識…`）・
半角括弧・強調（`**…**`）・語尾の省略（`## 知識`）も同じ節として認識する。

- **旧書式（節なし）もそのまま読める**。`tako_orchestrator_handoff` は書式に関係なく
  **全文を後任へ渡す**。旧書式のときは「番号への参照はすべて実態で確認しろ + 次の更新で
  2 節へ書き直せ」が後任プロンプトに付くので、自然な更新で新書式へ移る（一括変換はしない）
- 新書式のときは節ごとの扱い（知識 = そのまま前提にしてよい / 実行状態 = 必ず実態で確認）が
  後任プロンプトに付く
- いま自分のファイルがどちらかは `tako orchestrator self` の `handoff_format`
  （`sectioned` / `legacy` / 未作成なら null）と `handoff_sections` で分かる。
  `tako orchestrator handoff` の応答も同じ 2 フィールドを返す
- 書式の正本は `tako_core::handoff`（`section_of_line` / `split_handoff` /
  `handoff_template`。見出し定数は master system prompt と同期していることを単体テストが拘束）

```bash
# 今の閾値と超過状態を見る（master 自身が使う）
tako orchestrator self

# 閾値を 55% に下げる（プロファイル単位）
tako orchestrator profiles set default --ctx-threshold 55

# 自動通知を止める（手動の tako orchestrator handoff は使える）
tako orchestrator profiles set default --auto-handoff false
```

自動通知を送った記録は `<data_dir>/supervisor.log` に残る（`action=ctx_handoff_nudge`）。
GUI からは設定画面（⌘,）→ プロファイル → 「自動ハンドオフ」でも同じ設定を変えられる。

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
