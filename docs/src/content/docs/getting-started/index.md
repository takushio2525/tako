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

AI 連携（Claude Code との連携）を使わず「ただのターミナル」として使う場合は、手順 1〜2 だけで完了です。

## 事前に必要なもの

| もの | 必須？ | 説明 |
|---|---|---|
| macOS（Apple Silicon） | 必須 | 現在の配布は Apple Silicon Mac（M1 以降）向けです。Windows 対応は開発中 |
| [Homebrew](https://brew.sh/ja/) | 推奨 | macOS 用のアプリ管理ツール。インストールとアップデートが 1 コマンドで済みます |
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code)（`claude` コマンド） | AI 連携に必要 | Anthropic の AI コーディングアシスタント。`tako setup` の実行にも必要です |
| tmux | あると便利 | ターミナルのセッション（作業状態）を保持するツール。入っていると **tako を再起動しても実行中のプロセスと画面が丸ごと復元**されるほか、**スマホからのリモート接続（`tako remote`）とオーケストレーターの worker 管理にはこれが必須**です。`brew install tmux` で導入 |
| cloudflared | リモート接続に推奨 | `tako remote` の接続 URL をインターネット経由で公開するトンネルツール。**未導入だと同じ Wi-Fi 内でしか開けない URL**になります。`brew install cloudflared` で導入 |
| git | あると便利 | git パネル（ブランチ・コミットグラフ・diff 表示）で使います。macOS では `xcode-select --install` で入っていることが多いです |

:::note[tmux とは？]
tmux（ティーマックス）は「ターミナルの中身を裏で生かしておく」ためのツールです。tako は tmux があると、アプリを閉じても実行中のコマンドや AI エージェントを裏で動かし続け、次回起動時にそのまま復元します。無くても tako は動作します（その場合、再起動でプロセスは終了し、`tako remote` などの tmux 前提の機能は使えません）。
:::

:::tip[入っているか分からないときは]
`tako setup` を実行すると、最初に依存ツール（claude / tmux / git）が入っているかを自動チェックし、未導入のものはその場で `brew install` を実行するか選べます。チェックだけしたい場合は `tako setup --check` を使ってください。
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

:::caution[事前に Claude Code が必要です]
`tako setup` は `claude` コマンド（Claude Code）を使って対話を進めるため、Claude Code のインストールとログインが先に必要です。未インストールの場合は [Claude Code のドキュメント](https://docs.anthropic.com/en/docs/claude-code)に従って導入し、一度 `claude` を起動してログインを済ませてください。
:::

### `tako setup` は何をするのか

実行すると、次の処理が自動で順に行われます。

1. **依存ツールのチェック** — claude（必須）と tmux / cloudflared / git（任意）が入っているか確認します。未導入のものは影響する機能の説明と導入方法が表示され、その場で `brew install` を実行するかも選べます。claude が見つからない場合はインストール案内を表示して終了します
2. **MCP 登録の確認と自動登録** — tako を AI から操作するための接続設定（後述）を `~/.claude/settings.json` に自動追加します。登録済みならスキップ
3. **設定テンプレートの展開** — セットアップ用のファイル一式を `~/Library/Application Support/tako/setup/` に書き出します
4. **既存の `~/.claude/CLAUDE.md` のバックアップ** — あなたの Claude Code 設定ファイルを上書きする前に、日付付きバックアップ（`CLAUDE.md.backup-日付`）を自動で取ります
5. **claude が対話でガイド** — ここから画面が Claude Code に切り替わり、質問に答えていくだけで環境が整います（設定できる項目は次の一覧を参照）。終わったら通常どおり claude を終了（`Ctrl+C` 2 回など）してください
6. **オーケストレーター用プロファイルの確認** — 最後に「プロファイルを設定しますか？」と聞かれますが、よく分からなければ **`1`（既定のまま）で Enter** を押せば OK です。既定設定はどの Claude プランでも動作します

:::note[MCP とは？]
MCP（Model Context Protocol）は、AI エージェントが外部ツールを操作するための共通規格です。tako は MCP サーバーを内蔵しており、登録しておくと Claude Code が「ペインを分割する」「コマンドを実行する」「ファイルを表示する」といった tako の操作を直接行えるようになります。
:::

### 対話で設定できる項目の一覧

対話パートで聞かれるのは以下の項目です。**すべて日本語の会話で答えるだけ**で、設定ファイルへの書き込みは AI がやってくれます。どれも後から `tako setup` の再実行や、[master への依頼](/features/orchestrator/#設定も会話で変える)で変えられるので、迷ったら気楽に既定を選んで進めて構いません。

#### あなたと Claude Code の付き合い方（`~/.claude/CLAUDE.md` に反映）

| 項目 | 何を設定するか | 選択肢 | 迷ったら |
|---|---|---|---|
| 回答言語 | AI の回答・説明に使う言語 | 日本語 / English / その他 | 日本語 |
| 開発経験レベル | AI の説明の詳しさ（初心者ほど丁寧に説明する） | 初心者 / 中級 / 上級 | 正直に申告 |
| 主な開発分野 | AI の提案を分野に合わせて調整 | Web / モバイル / 組み込み / データサイエンス / その他（複数可） | 普段触るものを挙げる |
| Git の使い方 | コミット・ブランチ運用のルール | 使う（trunk-based / feature branch / その他）/ 使わない | 「使うけどスタイルはお任せ」で OK |

回答をもとに、対話スタイル・コード品質・安全ルール（本番データを触らない等）・提案の出し方といった基本ルールを含む `~/.claude/CLAUDE.md`（Claude Code の全プロジェクト共通設定）が生成されます。すでに CLAUDE.md を持っている場合は、白紙から聞き直すのではなく**既存の内容を読み取ったうえで「補強するか・作り直すか」を確認**してくれます。

#### オーケストレーションの設定（master / worker の動かし方）

| 項目 | 何を設定するか | 選択肢 | 迷ったら |
|---|---|---|---|
| Claude のプラン | 使えるモデルの範囲を判断する前提情報 | Pro / Max 5x / Max 20x / Team / Enterprise / API 直接利用 | 契約どおりに答える |
| モデル選びの重視点 | master / worker に使うモデル・思考量の提案基準 | 品質重視 / バランス / コスト・回数重視 | バランス |
| プロジェクト登録 | master が作業対象にできるリポジトリ | 任意のディレクトリ（複数可・スキップ可） | スキップして OK（後から master に「◯◯を追加して」と頼むだけ） |

プランと重視点を答えると、AI が**最新のモデル情報を調べたうえで**「あなたの状況ならこの構成」と理由付きで提案してくれます。Pro プランで使えないモデルを勧められる心配はありません。確信が持てない場合の既定は「モデル未指定（claude の既定モデル）」で、これはどのプランでも確実に動く構成です。

#### 最後のプロファイル確認（claude 終了後）

claude を終了すると、ターミナルに戻って最後の確認が表示されます。

```
プロファイルを設定しますか？
  1) 既定のままにする（推奨: claude 既定モデル / max / inherit。全プランで動作）
  2) 設定する
選択 [1]:
```

**`1` のまま Enter で完了です。**`2` を選ぶと、master のモデル・思考量（effort）・worker のモデル決定ポリシー（inherit = master と同じ / fixed = 別の固定モデル / delegate = master が判断）を 1 つずつ指定できますが、これは後から会話でも変えられる上級者向けの道です。

### 2 回目以降の `tako setup`

セットアップ済みの状態で再実行すると、設定変更メニューになります。

1. **CLAUDE.md の確認・編集** — 現在の設定を表示し、会話で修正
2. **オーケストレーター設定の変更** — モデル・effort・挙動フラグの見直し
3. **MCP 接続の再設定** — 登録が壊れていれば修復
4. **環境チェックの再実行** — 現在の状態を一覧表示

ここでも、やりたいことを日本語で伝えるだけです。「回答をもっと簡潔にして」「worker のモデルを変えたい」のように話しかけてください。

### 環境チェックだけしたいとき

セットアップを実行せず、現在の状態だけ確認できます。

```bash
tako setup --check
```

```
tako セットアップ 環境チェック
─────────────────────────────
  ✓ claude: /opt/homebrew/bin/claude
  ✓ MCP: tako が登録済み
  ✓ セットアップ: 完了済み (2026-07-05T12:00:00+09:00)
  ✓ ~/.claude/CLAUDE.md: 存在します
  ✓ プロファイル: 1 個（default）
```

`✗` や `△` が付いた項目が、まだ済んでいない設定です。

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

tako 内のターミナルで Claude Code を起動します。

```bash
claude
```

起動したら、試しにこう話しかけてみてください。

> 隣のペインで `ls` を実行して

画面が自動で分割され、隣のペインでコマンドが実行されれば連携成功です。ほかにもこんな指示が通ります。

- **「隣のペインで dev サーバーを起動して」** → ペインを分割してコマンド実行
- **「このファイルをプレビューで見せて」** → シンタックスハイライト付きでファイル表示
- **「今のレイアウトを教えて」** → タブ・ペイン構成の一覧取得

AI は環境変数（`TAKO_PANE_ID` など）から自分がどのペインにいるかを自動認識するため、プロジェクトごとの設定は不要です。

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

### `tako setup` が「claude コマンドが見つかりません」と言う

Claude Code が未インストールか、PATH に入っていません。[Claude Code のドキュメント](https://docs.anthropic.com/en/docs/claude-code)に従ってインストールし、`claude --version` が通ることを確認してから再実行してください。

### MCP ツールが認識されない（AI が tako を操作できない）

1. `tako setup --check` で「MCP: tako が登録済み」になっているか確認
2. 未登録なら `tako setup-mcp` を実行
3. Claude Code を一度終了し、**tako の中のターミナルで**起動し直す（tako の外で起動した Claude Code からは、安全のため tako を操作できません）

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
