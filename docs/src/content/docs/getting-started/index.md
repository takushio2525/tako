---
title: セットアップ
description: tako のダウンロードからインストール、tako setup による環境構築まで、初心者向けに順を追って解説
---

tako を使い始めるまでの手順を、前提知識がない方でも上から順に読めば動かせるように説明します。所要時間は 10 分程度です。

## 全体の流れ

1. **tako 本体をインストールする**（Homebrew または ZIP）
2. **tako を起動する**
3. **`tako setup` を実行する** — AI 連携に必要な設定を対話形式でまとめて行うコマンド
4. **動作確認**

AI 連携を使わず「ただのターミナル」として使う場合は、手順 1〜2 だけで完了です。

## 事前に必要なもの

| もの | 必須？ | 説明 |
|---|---|---|
| macOS（Apple Silicon） | 必須 | 現在の配布は Apple Silicon Mac（M1 以降）向けです。Windows 対応は開発中 |
| [Homebrew](https://brew.sh/ja/) | 推奨 | macOS 用のアプリ管理ツール。インストールとアップデートが 1 コマンドで済みます |
| AI エージェント CLI | AI 連携に1つ以上必要 | `claude`（Claude Code）/ `codex`（OpenAI Codex CLI）/ `agy`（Gemini 系）のいずれか。あらかじめ各 CLI でログインしてください |
| tmux | あると便利 | ターミナルのセッション（作業状態）を保持するツール。入っていると **tako を再起動しても実行中のプロセスと画面が丸ごと復元**されるほか、**スマホからのリモート接続（`tako remote`）とオーケストレーターの worker 管理にはこれが必須**です。`brew install tmux` で導入 |
| cloudflared | リモート接続に推奨 | `tako remote` の接続 URL をインターネット経由で公開するトンネルツール。**未導入だと同じ Wi-Fi 内でしか開けない URL**になります。`brew install cloudflared` で導入 |
| git | あると便利 | git パネル（ブランチ・コミットグラフ・diff 表示）で使います。macOS では `xcode-select --install` で入っていることが多いです |

:::note[tmux とは？]
tmux（ティーマックス）は「ターミナルの中身を裏で生かしておく」ためのツールです。tako は tmux があると、アプリを閉じても実行中のコマンドや AI エージェントを裏で動かし続け、次回起動時にそのまま復元します。無くても tako は動作します（その場合、再起動でプロセスは終了し、`tako remote` などの tmux 前提の機能は使えません）。
:::

:::tip[入っているか分からないときは]
`tako setup` を実行すると、最初に claude / codex / agy と依存ツール（tmux / cloudflared / git）を自動チェックします。エージェント CLI は1つなら自動選択、複数なら選択式です。チェックだけしたい場合は `tako setup --check` を使ってください。
:::

## 1. インストール

### 方法 A: Homebrew（推奨）

ターミナル（macOS 標準の「ターミナル.app」で OK）を開き、次の 2 行を実行します。

```bash
brew tap takushio2525/tako
brew install --cask takushio2525/tako/tako
```

- 1 行目は「tako の配布元を Homebrew に登録する」コマンド（初回のみ）
- 2 行目が実際のインストール。アプリ本体が `/Applications/tako.app` に入り、`tako` コマンドも自動で使えるようになります

インストールできたか確認:

```bash
tako --version
```

バージョン番号（例: `tako 0.2.8`）が表示されれば成功です。

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

#### PATH を通す（ZIP の場合のみ）

「PATH を通す」とは、ターミナルのどこからでも `tako` とだけ打てばコマンドが使えるようにする設定です。Homebrew なら自動ですが、ZIP の場合は手動で登録します。

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

まずは普段どおりコマンドを打ってみてください。`ls` や `cd` など、通常のターミナルと同じ操作がそのまま使えます。

## 3. `tako setup` — 対話式セットアップ（AI 連携する場合）

AI 連携に必要な設定を、**1 コマンドでまとめて**行います。tako 内のターミナルで次を実行してください。

```bash
tako setup
```

:::caution[エージェント CLI を1つ以上準備してください]
`claude` / `codex` / `agy` のいずれかをインストールし、その CLI を単独で一度起動してログインを済ませてください。`tako setup` はインストール代行は行わず、見つからない場合は各 CLI の導入先を案内します。
:::

### `tako setup` は何をするのか

実行すると、次の処理が自動で順に行われます。

1. **エージェント CLI と依存ツールのチェック** — claude / codex / agy をすべて検出します。1つなら自動選択、複数なら認証状態つきの一覧から選択します。tmux / cloudflared / git は任意依存として従来どおり確認します
2. **認証・プラン確認** — Claude は認証と Pro / Max 等、Codex は認証と ChatGPT プランを取得できる範囲で自動判定します。agy のプラン、Claude Max の 5x / 20x など取得できない情報だけ質問します。token やアカウント情報は保存・表示しません
3. **MCP 接続の準備** — claude を選んだ場合は `~/.claude/settings.json` へ自動登録します。codex は `tako master` の起動時だけ MCP 設定を注入するため、グローバル設定を変更しません。agy は worker 専用です
4. **推奨 profile の生成** — プラン規模に応じて master / worker、effort、worker ポリシーを `profiles/default.yaml` へ生成します。モデル名は固定せず、各 CLI の最新の既定モデルを使います。既存 profile がある場合は更新前に確認し、カスタム system prompt や projects は保持します
5. **テンプレートとバックアップ** — セットアップ用ファイル一式を `~/Library/Application Support/tako/setup/` に展開し、選択エージェントの既存グローバル指示ファイルを日付つきでバックアップします
6. **選択したエージェントが対話でガイド** — claude / codex / agy のうち選んだ CLI に切り替わり、回答言語・開発スタイル・プロジェクト登録など残りの設定を会話で進めます

:::note[MCP とは？]
MCP（Model Context Protocol）は、AI エージェントが外部ツールを操作するための共通規格です。tako は MCP サーバーを内蔵しており、claude または codex の master が「ペインを分割する」「コマンドを実行する」「ファイルを表示する」といった操作を直接行えます。
:::

### 対話で設定できる項目の一覧

対話パートで聞かれるのは以下の項目です。**すべて日本語の会話で答えるだけ**で、設定ファイルへの書き込みは AI がやってくれます。どれも後から `tako setup` の再実行や、[master への依頼](/features/orchestrator/#設定も会話で変える)で変えられるので、迷ったら気楽に既定を選んで進めて構いません。

#### あなたと AI エージェントの付き合い方

| 項目 | 何を設定するか | 選択肢 | 迷ったら |
|---|---|---|---|
| 回答言語 | AI の回答・説明に使う言語 | 日本語 / English / その他 | 日本語 |
| 開発経験レベル | AI の説明の詳しさ（初心者ほど丁寧に説明する） | 初心者 / 中級 / 上級 | 正直に申告 |
| 主な開発分野 | AI の提案を分野に合わせて調整 | Web / モバイル / 組み込み / データサイエンス / その他（複数可） | 普段触るものを挙げる |
| Git の使い方 | コミット・ブランチ運用のルール | 使う（trunk-based / feature branch / その他）/ 使わない | 「使うけどスタイルはお任せ」で OK |

回答は、選択した CLI に応じて `~/.claude/CLAUDE.md`、`~/.codex/AGENTS.md`、`~/.gemini/GEMINI.md` のいずれかへ反映されます。既存ファイルがあれば、白紙から聞き直さず内容を読み取ったうえで補強するか維持するかを確認します。

#### オーケストレーションの設定（master / worker の動かし方）

| 項目 | 何を設定するか | 選択肢 | 迷ったら |
|---|---|---|---|
| Claude / GPT / Google のプラン | effort と worker の使い分けを決める前提情報 | 各社の Free / 個人向け有料 / 上位・組織・API | 自動検出された項目は質問されません |
| 既定エージェント | setup の対話と profile の基準 | 検出された claude / codex / agy | 1つだけなら自動選択 |
| プロジェクト登録 | master が作業対象にできるリポジトリ | 任意のディレクトリ（複数可・スキップ可） | スキップして OK（後から master に「◯◯を追加して」と頼むだけ） |

推奨 profile は CLI が先に生成します。小規模プランは利用回数を抑える effort、上位プランは高い effort、複数の認証済み CLI が使える場合は master がタスクごとに worker を振り分ける `delegate` を提案します。モデルはプランやリリースで陳腐化しないよう未指定、つまり各 CLI の既定モデルです。

agy は setup の対話と worker には使えますが、MCP 接続方式の制約から master には使えません。agy だけが入っている環境では setup を完了できますが、`tako master` を使う前に claude または codex を追加してください。

### 2 回目以降の `tako setup`

セットアップ済みの状態で再実行すると、設定変更メニューになります。

1. **選択エージェントの指示ファイル確認・編集** — 現在の設定を表示し、会話で修正
2. **オーケストレーター設定の変更** — master / worker / effort / 挙動フラグの見直し
3. **エージェントとプラン推奨の再確認** — 新しく導入した CLI やプラン変更を反映
4. **MCP 接続の確認** — claude の永続登録、codex の起動時注入を診断
5. **環境チェックの再実行** — 現在の状態を一覧表示

ここでも、やりたいことを日本語で伝えるだけです。「回答をもっと簡潔にして」「worker のモデルを変えたい」のように話しかけてください。

### アップデート後の追従: `tako setup` の再実行

tako のアップデートで、セットアップ項目や設定ファイルのフォーマット、master 用システムプロンプトが変わることがあります。**いつでも `tako setup` を再実行すれば、最新の正しい状態に追いつけます。**

前回のセットアップ以降にそうした変更が入っていると、再実行時に冒頭で一覧表示され、対話の中で追従が行われます。

- **自動適用される変更**（新しいチェック項目・テンプレートの更新など）は、何が変わったかが伝えられるだけで作業は不要です
- **確認が必要な変更**（あなたがカスタマイズしたファイルに関わるもの）は、現在の設定を読み取ったうえで差分が提示され、**同意した場合にだけ**適用されます。カスタマイズが黙って上書きされることはありません

追従が必要かどうかだけを先に確認したいときは:

```bash
tako setup --changes
```

```
tako setup アップデート追従状況
─────────────────────────────
  現在の setup リビジョン: 8（tako v0.5.2）
  適用済みリビジョン: 7（tako v0.5.1 で setup 実行）
  未適用の変更: 1 件

  [rev 8 / v0.5.2 / 2026-07-14] setup を claude / codex / agy の検出・プラン別推奨に対応
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
```

`[不足]` や未認証と表示された項目が、まだ済んでいない設定です。

### やり直したいとき

```bash
tako setup --reset
```

セットアップ状態を初回扱いにリセットし、そのまま再実行します。

### MCP 登録だけしたいとき

対話セットアップを使わず、AI からの操作に必要な最低限の登録だけ行うこともできます。

```bash
tako setup-mcp
```

`~/.claude/settings.json` に tako の MCP サーバーを自動登録します。一度実行すればどのプロジェクトでも有効です。`tako setup-mcp --project` とすると、現在のディレクトリ（`.claude/settings.json`）だけに登録できます。

## 4. 動作確認

tako 内のターミナルで master を起動します。setup が生成した profile に応じて claude または codex が立ち上がります。

```bash
tako master
```

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

PATH が通っていません。Homebrew でインストールした場合はターミナルを開き直してみてください。ZIP からインストールした場合は上記の「PATH を通す」手順を確認してください。

### `tako setup` が「エージェント CLI が見つかりません」と言う

claude / codex / agy のいずれも PATH にありません。使う CLI を導入し、`<CLI名> --version` が通ることを確認してから再実行してください。tako はエージェント CLI 自体のインストールは行いません。

### MCP ツールが認識されない（AI が tako を操作できない）

1. `tako setup --check` でエージェント別の MCP 状態を確認
2. claude が未登録なら `tako setup-mcp` を実行
3. codex は `tako master` から起動する（MCP 設定は起動時だけ注入されます）
4. エージェントを一度終了し、**tako の中のターミナルで**起動し直す（tako の外からは安全のため tako を操作できません）

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
- [CLI リファレンス](/guides/cli-reference/) — `tako` コマンド全一覧
- [リリースノート](/releases/) — 各バージョンの変更内容
