---
title: セットアップ
description: tako のダウンロードからインストール、tako setup による環境構築まで、初心者向けに順を追って解説
---

tako を使い始めるまでの手順を、前提知識がない方でも上から順に読めば動かせるように説明します。所要時間は 10 分程度です。

## 全体の流れ

1. **tako 本体をインストールする**（Homebrew または ZIP）
2. **tako を起動する**
3. **`tako setup` を実行する** — AI 連携に必要な設定を質問ゼロでまとめて行うコマンド
4. **動作確認**

AI 連携を使わず「ただのターミナル」として使う場合は、手順 1〜2 だけで完了です。

## 事前に必要なもの

| もの | 必須？ | 説明 |
|---|---|---|
| macOS（Apple Silicon）または Windows | 必須 | 主な配布は Apple Silicon Mac（M1 以降）向けです。Windows 版は移植を進めており、どの機能が使えるかは [Windows 対応状況](/windows-support/) にまとめています |
| [Homebrew](https://brew.sh/ja/) | 推奨 | macOS 用のアプリ管理ツール。インストールとアップデートが 1 コマンドで済みます |
| AI エージェント CLI | AI 連携に1つ以上必要 | `claude`（Claude Code）/ `codex`（OpenAI Codex CLI）/ `agy`（Gemini 系）のいずれか。あらかじめ各 CLI でログインしてください |
| tmux | あると便利 | ターミナルのセッション（作業状態）を保持するツール。入っていると **tako を再起動しても実行中のプロセスと画面が丸ごと復元**されるほか、**スマホからのリモート接続（`tako remote`）とオーケストレーターの worker 管理にはこれが必須**です。`brew install tmux` で導入。**Windows では psmux が同じ役割を担います**（`winget install marlocarlo.psmux`。attach を前提にする一部の tmux 操作は使えません → [Windows 対応状況](/windows-support/)） |
| [Tailscale](https://tailscale.com/) | リモート接続に必須 | `tako remote`（スマホからの接続）の transport。Mac とスマホの両方にアプリを入れて同一アカウントでログインすると、tailnet 内限定の固定 URL で安全に接続できます |
| git | あると便利 | git パネル（ブランチ・コミットグラフ・diff 表示）で使います。macOS では `xcode-select --install` で入っていることが多いです |

:::note[tmux とは？]
tmux（ティーマックス）は「ターミナルの中身を裏で生かしておく」ためのツールです。tako は tmux があると、アプリを閉じても実行中のコマンドや AI エージェントを裏で動かし続け、次回起動時にそのまま復元します。無くても tako は動作します（その場合、再起動でプロセスは終了し、`tako remote` などの tmux 前提の機能は使えません）。
:::

:::tip[入っているか分からないときは]
`tako setup` を実行すると、最初に claude / codex / agy と依存ツール（tmux / git）を自動チェックします。認証済み CLI とプランは検出結果、前回値、安全な既定値の順で自動決定します。チェックだけしたい場合は `tako setup --check` を使ってください。
:::

## 1. インストール

### 方法 A: Homebrew（推奨）

ターミナル（macOS 標準の「ターミナル.app」で OK）を開き、次の 1 行を実行します。

```bash
brew install --cask takushio2525/tako/tako
```

アプリ本体が `/Applications/tako.app` に入り、`tako` コマンドも自動で使えるようになります。

:::note[`Cask not found` と言われたら]
配布元の登録（tap）を先に済ませてから、もう一度インストールしてください。

```bash
brew tap takushio2525/tako
brew install --cask takushio2525/tako/tako
```
:::

インストールできたか確認:

```bash
tako --version
```

バージョン番号（例: `tako 0.6.0`）が表示されれば成功です。

### 方法 B: ZIP ダウンロード

Homebrew を使わない場合は、GitHub Releases から直接ダウンロードします。

<p>
<a href="https://github.com/takushio2525/tako/releases/latest" class="tako-btn tako-btn-primary" style="font-size: 1.05rem;">GitHub Releases から最新版をダウンロード →</a>
</p>

`tako-vX.X.X-macos-arm64.zip` をダウンロードし、以下を実行します。

```bash
# ダウンロードした zip を展開
unzip tako-*.zip

# /Applications に配置
mv tako.app /Applications/
```

**tako を起動して、その中のターミナルで使う分には、これで準備完了です。** tako が開くシェルには `tako` コマンドの置き場所が自動で追加されるので、[クイックスタート](/getting-started/quickstart/)の `tako setup` はそのまま打てます。

#### 外部ターミナルでも使いたい場合（ZIP のみ・任意）

Terminal.app や iTerm2 など **tako の外**のターミナルからも `tako` と打ちたいときだけ、PATH に登録します（Homebrew なら自動）。

```bash
# zsh（macOS 標準のシェル）の場合
echo 'export PATH="/Applications/tako.app/Contents/MacOS:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

登録できたか確認:

```bash
tako --version
```

### つまずきポイント: Gatekeeper の警告

初回起動時に「開発元を確認できないため開けません」という警告が出ることがあります。これは macOS のセキュリティ機能（Gatekeeper）によるもので、以下のいずれかで解除できます。

1. `tako.app` を**右クリック → 「開く」** を選択（いちばん簡単）
2. 「システム設定 → プライバシーとセキュリティ」を開き、「このまま開く」をクリック
3. ターミナルで属性を解除する:

```bash
xattr -dr com.apple.quarantine /Applications/tako.app
```

一度許可すれば、以降は通常のアプリと同じように起動できます。

## 2. 起動

`/Applications/tako.app` をダブルクリック、または Dock / Launchpad から起動します。通常のターミナルと同じように、シェルが 1 ペイン開きます。

**初回起動時は、タブバーの下に案内バナーが出ます。** `tako setup` と `tako master` をその場のボタンから実行できるので、次の手順はバナーからそのまま進められます。バナーは一度閉じると再表示されません（`tako welcome show` でいつでも呼び戻せます）。同じ項目は <kbd>Cmd</kbd>+<kbd>K</kbd> のコマンドパレットにも常設されています。

まずは普段どおりコマンドを打ってみてください。`ls` や `cd` など、通常のターミナルと同じ操作がそのまま使えます。コマンドを打ち始めると、履歴から続きが薄い文字で予測表示されます（<kbd>→</kbd> または <kbd>Tab</kbd> で確定）。最初の 10 回だけ確定キーの案内が薄く表示され、慣れた頃に自然と消えます。

この入力予測は **tako が開いたシェルの中だけ**で効き、`~/.zshrc` は書き換えないので tako の外のターミナルは何も変わりません。挙動は `tako autosuggest off`（予測そのもの）/ `tako autosuggest tab off`（Tab 確定だけ無効化）/ `tako autosuggest hint off`（案内を今すぐ止める）で個別に切り替えられます。

## 3. `tako setup` — 質問ゼロの自動セットアップ

AI 連携に必要な設定を、**1 コマンドで自動的に**行います。tako 内のターミナルで次を実行してください。

```bash
tako setup
```

:::caution[エージェント CLI を1つ以上準備してください]
`claude` / `codex` / `agy` のいずれかをインストールし、その CLI を単独で一度起動してログインを済ませてください。`tako setup` はインストール代行は行わず、見つからない場合は各 CLI の導入先を案内します。
:::

### `tako setup` は何をするのか

実行すると、次の処理が自動で順に行われます。

1. **エージェント CLI と依存ツールのチェック** — claude / codex / agy をすべて検出します。認証済み CLI が1つなら自動選択し、複数なら前回値または安全な既定を採用します。tmux / git は任意依存として状態と導入コマンドだけを表示します
2. **認証・プラン確認** — Claude は認証と Pro / Max 等、Codex は認証と ChatGPT プランを取得できる範囲で自動判定します。検出不能でも安全に未指定にできる情報は `unknown` を採用します。token やアカウント情報は保存・表示しません
3. **MCP 接続の準備** — claude を選んだ場合は `~/.claude/settings.json` へ自動登録します。codex は `tako master` の起動時だけ MCP 設定を注入するため、グローバル設定を変更しません。agy は worker 専用です
4. **推奨 profile の生成** — プラン規模に応じて master / worker、effort、worker ポリシーを `profiles/default.yaml` へ生成します。モデル名は固定せず、各 CLI の最新の既定モデルを使います。既存 profile はそのまま維持します
5. **指示とテンプレートの準備** — 指示ファイルが未作成なら安全な開発ルールの既定値を作り、セットアップ用ファイル一式を `~/Library/Application Support/tako/setup/` に展開します。既存の指示は上書きしません
6. **同梱推奨ルールとの比較** — 既存の指示ファイルを、tako が同梱する推奨ルール（言語 / 対話スタイル / Git 運用 / コード品質 / 安全ルール / 提案品質 / 完了検証の 7 項目）と項目レベルで突き合わせ、不足の可能性を具体的に表示します。差分がなければ「差分なし」と明示します。表示のみで、ファイルは書き換えません
7. **設定共有の状況確認** — 別の PC と設定を共有する仕組み（[`tako config`](/guides/cli-reference/#tako-config)）について、配線済みか / `~/.claude` が既に dotfiles などの git で管理されていないか / `gh` にログイン済みかを調べ、状態と次の一手を表示します。**調べるだけ**で、リポジトリの作成や配線は起こりません。配線済みなら状態が 1 行出るだけで、設定を勧める案内は出ません
8. **最終サマリと次の一歩** — 値の由来を `detected` / `previous` / `default` / `input` で表示し、実際に変えた項目だけを最後にまとめます。続けて `tako master` での始め方（起動して日本語で話しかけるだけ）とプロファイルの現在値を案内し、tako 内での対話実行ならその場で master の開始を提案します。認証済み単一 CLI の標準ケースでは人間への質問はありません

:::note[MCP とは？]
MCP（Model Context Protocol）は、AI エージェントが外部ツールを操作するための共通規格です。tako は MCP サーバーを内蔵しており、claude または codex の master が「ペインを分割する」「コマンドを実行する」「ファイルを表示する」といった操作を直接行えます。
:::

### 好みがある場合は AI にセットアップを頼む

通常は既定値で十分です。回答言語、開発ルール、master / worker、プロジェクト登録などを変えたい場合は、tako の MCP に接続した AI へ日本語でそのまま伝えられます。

> 「tako のセットアップをして。回答は日本語で簡潔に、master と worker は codex、プロジェクト `app` は `~/src/app`。自動 push はオフにして」

AI は希望を `tako_setup` の回答 JSON に変換し、setup を非対話で代行します。省略した項目は検出値 → 前回値 → 安全な既定値の順で補われます。設定ファイルを手で編集する必要はありません。

シェルや別の自動化から同じことを行う場合は `--answers` を使います。長い回答はファイル指定が扱いやすく、`-` は標準入力から JSON を読みます。

```bash
tako setup --yes
tako setup --answers @setup-answers.json
printf '%s' '{"selected_agent":"codex","provider_plans":{"gpt":"plus"}}' \
  | tako setup --answers -
```

回答 JSON で指定できるのは `selected_agent`、`provider_plans`、`instruction_content`、`profile`、`projects`、`orchestrator`（`auto_close` / `auto_push`）、`sleep_guard` です。`projects` は指定時に登録一覧全体を置き換え、その他の省略項目は既存値を維持します。

会話しながら現在設定を一項目ずつ見直したい場合だけ、明示的に次を使います。

```bash
tako setup --review
```

agy は setup と worker には使えますが、MCP 接続方式の制約から master には使えません。agy だけが入っている環境では setup を完了できますが、`tako master` を使う前に claude または codex を追加してください。

### 別の PC でも同じ設定を使いたいとき

`tako setup` の最後に起動する対話アシスタントに、そのまま頼めます。

> 「別の PC でも同じ設定を使いたい」

アシスタントは検出済みの状況（未配線か / 既に dotfiles などで管理していないか / `gh` にログイン済みか）を見て進め方を提案し、合意が取れた操作だけを実行します。

- 既に `~/.claude` を dotfiles リポジトリで管理している場合は、**そのリポジトリへの相乗り**（`tako config link <リポジトリ>`）を最初に提案します。別のリポジトリを新しく作ると、同じ `CLAUDE.md` が 2 か所で管理されることになるためです
- 何も無い場合は、`gh` にログイン済みならプライベートリポジトリの作成から配線まで代行できます。リポジトリ名は必ず確認し、**同意なしにリポジトリを作ったり push したりはしません**
- 共有したくなければ何もしなくて構いません。標準の `tako setup` は状態を表示するだけで、質問は増えません

仕組みと共有対象の一覧は [`tako config`](/guides/cli-reference/#tako-config) を参照してください。

### 2 回目以降の `tako setup`

セットアップ済みの状態で再実行すると、前回の agent・プラン・profile・指示・プロジェクトを自動的に引き継ぎます。新しい検出値が前回値と違う場合だけ、両方を表示して検出値を優先します。実変更がなければ `config.yaml` を書き直さず、「変更なし」と表示して終了します。キー入力は必要ありません。

### アップデート後の追従: `tako setup` の再実行

tako のアップデートで、セットアップ項目や設定ファイルのフォーマット、master 用システムプロンプトが変わることがあります。**いつでも `tako setup` を再実行すれば、最新の正しい状態に追いつけます。**

前回のセットアップ以降にそうした変更が入っていると、再実行時に冒頭で一覧表示され、自動項目はそのまま追従します。

- **自動適用される変更**（新しいチェック項目・テンプレートの更新など）は、何が変わったかが伝えられるだけで作業は不要です
- **個別見直しが必要な変更**（あなたがカスタマイズしたファイルに関わるもの）は標準実行では既存値を維持します。変更したい場合だけ `tako setup --review` で差分を確認します

追従が必要かどうかだけを先に確認したいときは:

```bash
tako setup --changes
```

```
tako setup アップデート追従状況
─────────────────────────────
  現在の setup リビジョン: 12（tako v0.6.0）
  適用済みリビジョン: 11（tako v0.5.6 で setup 実行）
  未適用の変更: 1 件

  [rev 12 / v0.6.0 / 2026-07-17] リモート接続を Tailscale Serve へ一本化 + setup ウィザード新設
      区分: auto（setup 再実行で自動適用）
      ...

  `tako setup` を実行すると追従できます
```

`tako setup --changes --json` で同じ内容を JSON でも取得できます（AI エージェント向けには MCP ツール `tako_setup_changes` もあります）。

### 環境チェックだけしたいとき

セットアップを実行せず、現在の状態だけ確認できます。

```bash
tako setup --check
```

```
tako セットアップ 環境チェック
─────────────────────────────
  エージェント CLI:
    [検出] claude: /opt/homebrew/bin/claude（認証済み / pro）
    [検出] codex: /opt/homebrew/bin/codex（認証済み / plus）
  [OK] 既定エージェント: codex
  [OK] 申告・検出プラン: claude=pro, google=free, gpt=plus
  [情報] 設定共有: 未配線（複数デバイスで同じ AI 設定を使うなら `tako config init`）
```

`[不足]` や未認証と表示された項目が、まだ済んでいない設定です。

### やり直したいとき

```bash
tako setup --reset
```

セットアップ状態を初回扱いにリセットし、そのまま質問ゼロで再実行します。指示・profile・projects の既存カスタマイズは維持されます。

### MCP 登録だけしたいとき

自動セットアップ全体を実行せず、AI からの操作に必要な最低限の登録だけ行うこともできます。

```bash
tako setup-mcp
```

claude と、この環境に導入済みの codex / agy へ tako の MCP サーバーを自動登録します。一度実行すればどのプロジェクトでも有効です。

| エージェント | 書き込み先 |
|---|---|
| claude | `~/.claude.json`（`--project` なら `<cwd>/.mcp.json`） |
| codex | `~/.codex/config.toml` の `[mcp_servers.tako]` |
| agy | `~/.gemini/config/mcp_config.json` の `mcpServers.tako` |

1 つに絞りたいときは `tako setup-mcp --agent codex` のように指定します（未導入の CLI を明示すると、理由と次の一手つきのエラーになります）。`tako setup-mcp --project` は claude のみ対応で、現在のディレクトリの `.mcp.json` だけに登録します。

なお codex / agy では、MCP ツールでペインを省略したときの「呼び出し元ペイン」が解決されません（両 CLI が MCP 子プロセスへ環境変数を素通しできないため）。ツール呼び出し自体は通るので、`pane` を明示すればすべての操作ができます。

## 4. 動作確認

tako 内のターミナルで master を起動します。setup が生成した profile に応じて claude または codex が立ち上がります。

```bash
tako master
```

master は**今いるペインでそのまま起動**します（新しいタブは作りません）。専用のタブを立てたい場合は `tako master --tab` を使ってください。

起動したら、試しにこう話しかけてみてください。

> 隣のペインで `ls` を実行して

画面が自動で分割され、隣のペインでコマンドが実行されれば連携成功です。ほかにもこんな指示が通ります。

- **「隣のペインで dev サーバーを起動して」** → ペインを分割してコマンド実行
- **「このファイルをプレビューで見せて」** → シンタックスハイライト付きでファイル表示
- **「今のレイアウトを教えて」** → タブ・ペイン構成の一覧取得

master は環境変数（`TAKO_PANE_ID` など）から自分がどのペインにいるかを自動認識するため、プロジェクトごとの MCP 設定は不要です。

ここまで動いたら、次は[クイックスタート](/getting-started/quickstart/)へ。`tako master` で司令塔の AI を立ち上げ、複数の AI に作業を任せる体験が数分でできます。

## アップデート

新しいバージョンが出ると、tako のステータスバー（画面下部）に更新通知が表示され、クリックひとつで更新できます。コマンドで行う場合:

```bash
# 更新があるか確認
tako update check

# 更新を適用（インストール方法を自動判別して適切に更新）
tako update apply
```

Homebrew 経由なら `brew upgrade --cask takushio2525/tako/tako` でも更新できます。各バージョンの変更内容は[リリースノート](/releases/)をご覧ください。

## トラブルシューティング

### `tako` コマンドが見つからない（command not found）

**tako の中のターミナル**で出た場合は、tako を再起動してみてください（`tako` の置き場所は tako の起動時に解決されます）。**tako の外**のターミナルで出た場合は PATH が通っていません。Homebrew でインストールした場合はターミナルを開き直し、ZIP からインストールした場合は上記の「外部ターミナルでも使いたい場合」の手順を実施してください。

### `tako setup` が「エージェント CLI が見つかりません」と言う

claude / codex / agy のいずれも PATH にありません。使う CLI を導入し、`<CLI名> --version` が通ることを確認してから再実行してください。tako はエージェント CLI 自体のインストールは行いません。

### MCP ツールが認識されない（AI が tako を操作できない）

1. `tako setup --check` でエージェント別の MCP 状態を確認
2. 未登録のものがあれば `tako setup-mcp` を実行（claude / codex / agy をまとめて登録します）
3. エージェントを一度終了し、**tako の中のターミナルで**起動し直す（tako の外からは安全のため tako を操作できません）
4. codex はツールが見えていてもペインの自動特定が効きません。`pane` を明示するか、`tako master` から起動してください

### tako 起動時にクラッシュする / 開けない

quarantine 属性（ダウンロードしたアプリに付く隔離マーク）が原因のことがあります。解除してから再度起動してください:

```bash
xattr -dr com.apple.quarantine /Applications/tako.app
```

### 再起動したらタブが消えていた

tmux バックエンドが無効になっている可能性があります。`tako persist` で状態を確認し、`tako persist on` で有効化してください。tmux 自体が未インストールの場合は `brew install tmux` で導入すると、実行中プロセスごと完全復元されるようになります。

## 次のステップ

- [クイックスタート](/getting-started/quickstart/) — `tako master` を起動して AI オーケストレーションを最短で体験する
- [タブ＆ペイン管理](/features/tabs-and-panes/) — 画面分割やショートカットを覚える
- [オーケストレーションとは](/features/orchestration/) — AI エージェントを並列に働かせる tako の目玉機能
- [設定とカスタマイズ](/guides/settings/) — テーマ・表示言語・入力予測などの調整
- [CLI リファレンス](/guides/cli-reference/) — `tako` コマンド全一覧
- [リリースノート](/releases/) — 各バージョンの変更内容
