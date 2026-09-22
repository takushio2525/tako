---
title: セットアップ
description: tako のダウンロードからインストール、tako setup による環境構築まで、初心者向けに順を追って解説
---

tako を使い始めるまでの手順を、前提知識がない方でも上から順に読めば動かせるように説明します。所要時間は 10 分程度です。

## 全体の流れ

1. **tako 本体をインストールする**（Homebrew または ZIP）
2. **tako を起動する**
3. **`tako setup` を実行する** — AI 連携に必要な設定をまとめて行うコマンド（必要なものが揃っていれば質問は出ません）
4. **動作確認**

AI 連携を使わず「ただのターミナル」として使う場合は、手順 1〜2 だけで完了です。

<figure class="tako-shot">
<img src="/img/welcome-banner.webp" alt="tako の初回起動時に表示されるバナー。1. tako setup、2. tako master の 2 ステップと、それぞれの実行ボタンが並んでいる" />
<figcaption>初回起動時のバナー。ここのボタンからそのまま `tako setup` と `tako master` を実行できる</figcaption>
</figure>

## 事前に必要なもの

| もの | 必須？ | 説明 |
|---|---|---|
| macOS（Apple Silicon）または Windows | 必須 | 主な配布は Apple Silicon Mac（M1 以降）向けです。Windows 版は移植を進めており、どの機能が使えるかは [Windows 対応状況](/windows-support/) にまとめています |
| [Homebrew](https://brew.sh/ja/) | 推奨 | macOS 用のアプリ管理ツール。インストールとアップデートが 1 コマンドで済みます |
| AI エージェント CLI | AI 連携に1つ以上必要 | `claude`（Claude Code）/ `codex`（OpenAI Codex CLI）/ `agy`（Antigravity CLI）のいずれか。**入っていなくても大丈夫です** — `tako setup` が公式インストーラで導入まで案内します（ログインだけは、ブラウザ操作が必要なのでご自身で行います） |
| tmux | あると便利 | ターミナルのセッション（作業状態）を保持するツール。入っていると **tako を再起動しても実行中のプロセスと画面が丸ごと復元**されるほか、**スマホからのリモート接続（`tako remote`）とオーケストレーターの worker 管理にはこれが必須**です。`brew install tmux` で導入。**Windows では psmux が同じ役割を担います**（`winget install marlocarlo.psmux`。attach を前提にする一部の tmux 操作は使えません → [Windows 対応状況](/windows-support/)） |
| [Tailscale](https://tailscale.com/) | リモート接続に必須 | `tako remote`（スマホからの接続）の transport。Mac とスマホの両方にアプリを入れて同一アカウントでログインすると、tailnet 内限定の固定 URL で安全に接続できます |
| git | あると便利 | git パネル（ブランチ・コミットグラフ・diff 表示）で使います。macOS では `xcode-select --install` で入っていることが多いです |

:::note[tmux とは？]
tmux（ティーマックス）は「ターミナルの中身を裏で生かしておく」ためのツールです。tako は tmux があると、アプリを閉じても実行中のコマンドや AI エージェントを裏で動かし続け、次回起動時にそのまま復元します。無くても tako は動作します（その場合、再起動でプロセスは終了し、`tako remote` などの tmux 前提の機能は使えません）。
:::

:::tip[入っているか分からないときは]
`tako setup` を実行すると、最初に claude / codex / agy と依存ツール（tmux / git）を自動チェックします。認証済み CLI とプランは検出結果、前回値、安全な既定値の順で自動決定します。チェックだけしたい場合は `tako setup --check` を使ってください（**3 系統それぞれについて「入っているか / PATH が通っているか / ログイン済みか」を一覧で出します**）。
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

バージョン番号（例: `tako 0.8.17`）が表示されれば成功です。

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

#### 外部ターミナルからも `tako` と打ちたい場合（ZIP のみ）

Terminal.app や iTerm2 など **tako の外**のターミナルでも `tako` を使いたいときは、**手で PATH を書く必要はありません**。手順 3 の `tako setup` が、その段でまとめて設置します（Homebrew でインストールした場合は cask が済ませているので何も要りません）。

`tako setup` がやることは 2 つです。

- `$HOME/.local/bin/tako` に本体へのシンボリックリンクを 1 本張る（claude / codex / agy のランチャーと同じ置き場所です）
- そのディレクトリが PATH に無ければ、`~/.zprofile` にマーカー付きのブロックを 1 組だけ追加する

セットアップ全体より先に、この設置だけを済ませることもできます。

```bash
tako setup bootstrap path
```

**新しいターミナルを開いてから** `tako --version` が通れば設置済みです（判定は「これから開くターミナルが見る PATH」で行うため、実行中のターミナルには反映されません）。元に戻すときは `tako setup bootstrap undo-path` で、`~/.zprofile` のブロックとシンボリックリンクの両方が外れます。

:::note[`tako.app` の中を直接 PATH に入れない理由]
アプリの中にある実行ファイルの置き場所そのものを PATH に足す方法は使いません。そこには `tako`（CLI）だけでなく本体の実行ファイルも同居しているので丸ごと PATH に出てしまい、さらに `tako.app` を別の場所へ移動した時点で黙って切れるためです。シンボリックリンク方式なら、`.app` を移動しても次に tako を起動したときに張り直されます。

**以前の版のこのページでは、`~/.zshrc` に `tako.app` のパスを含む `export PATH=...` の行を足す手順を案内していました。** その行が残っている場合は削除してかまいません（`tako setup bootstrap path` で設置し直せます）。
:::

Windows のインストーラー版は、インストール先（`%LOCALAPPDATA%\Programs\tako`）を `HKCU\Environment\Path` に登録します（シンボリックリンクは権限が要るため使いません）。

### 方法 C: Windows（インストーラー / ポータブル zip）

Windows 版の配布物は **v0.7.9 から macOS 版と同じリリースに載ります**。
[GitHub Releases](https://github.com/takushio2525/tako/releases/latest) の
ダウンロード表から、次のどちらかを選んでください。

| ファイル | 向いている人 |
|---|---|
| `tako-vX.X.X-windows-x86_64.exe` | **こちらが標準**。インストーラー。スタートメニュー登録と `tako` コマンドの PATH 追加までやってくれます |
| `tako-vX.X.X-windows-x86_64.zip` | 展開して置くだけのポータブル版。インストールしたくないとき |

- 動作要件は **Windows 10 バージョン 1809（ビルド 10.0.17763）以降 / x64**
- インストーラーは**管理者権限を要求しません**（`%LOCALAPPDATA%\Programs\tako` に入ります）
- ポータブル版は `tako-app.exe`（本体）と `tako.exe`（CLI）を**必ず同じフォルダに置いたまま**使ってください（離すと AI 経由の CLI 操作が動きません）

:::caution[SmartScreen の警告が出たら]
署名していない配布物なので、初回実行時に「WindowsによってPCが保護されました」と出ることがあります。**詳細情報 → 実行** で進めてください。
:::

Windows 版はまだ移植の途中です。どの機能が使えて、どれが縮退・未実装かは
[Windows 対応状況](/windows-support/)にまとめてあります（リリースノートにも要約が載ります）。

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

:::tip[エージェント CLI が 1 つも入っていなくても始められます]
`claude` / `codex` / `agy` が 1 つも見つからないときは、`tako setup` が **インストール → PATH 通し → ログイン案内** の 3 段をそのまま案内します。実行する前に「何をどこに入れるか」（公式コマンド・取得元・置き場所・以後の更新のされ方）を必ず表示して確認を取り、**管理者権限は使いません**（ホームディレクトリの中だけで完結します）。

**すでに 1 つでも使える状態なら、この案内は出ません**（余計な導入を勧めません）。たとえば `codex` だけが入っている環境では codex をそのまま使い、claude を入れるよう促したりはしません。あとから系統を増やしたくなったら `tako setup bootstrap install --agent codex` のように 1 コマンドで足せます。

**ログインだけはご自身で**行ってください。ブラウザでの操作が必要なので tako は代行しません（案内するコマンドは claude なら `claude auth login`、codex なら `codex login`、agy は引数なしの `agy` 起動です）。
:::

### `tako setup` は何をするのか

実行すると、次の処理が自動で順に行われます。

1. **エージェント CLI と依存ツールのチェック** — claude / codex / agy をすべて検出します。認証済み CLI が1つなら自動選択し、複数なら前回値または安全な既定を採用します。**1 つも使える系統が無いときだけ**、その場で導入を案内します（上の tip 参照）。tmux / git / Tailscale などの任意依存は、見つからなければ「何をどのコマンドで入れるか」を表示したうえで `tmux をインストールしますか？ [y/N]:` と **1 回だけ確認し、`y` ならその場で導入して再検出します**（`--yes` は確認を省いて導入、パイプ越しなど端末のない実行では導入せず案内だけを出します）
2. **認証・プラン確認** — Claude は認証と Pro / Max 等、Codex は認証と ChatGPT プランを取得できる範囲で自動判定します。検出不能でも安全に未指定にできる情報は `unknown` を採用します。token やアカウント情報は保存・表示しません
3. **MCP 接続の準備** — 検出したエージェント CLI すべてへ tako の MCP サーバーを登録します。書き込み先は claude が `~/.claude.json`、codex が `~/.codex/config.toml` の `[mcp_servers.tako]`、agy が `~/.gemini/config/mcp_config.json` です（claude だけは `tako setup-mcp --project` で `<cwd>/.mcp.json` にも入れられます）。**書き先が `~/.claude/settings.json` だったのは古い tako で、いまはそこに残った設定を掃除する側です。** codex はこれに加えて、master / worker として起動するときに起動コマンドへその場でも MCP 設定を渡します（tako の外で立ち上げた codex にツールを出さないための経路です）
4. **推奨 profile の生成** — プラン規模に応じて master / worker、effort、worker ポリシーを `profiles/default.yaml` へ生成します。モデル名は固定せず、各 CLI の最新の既定モデルを使います。既存 profile はそのまま維持します
5. **指示とテンプレートの準備** — 指示ファイルが未作成なら安全な開発ルールの既定値を作り、セットアップ用ファイル一式を tako のデータディレクトリ配下（macOS は `~/Library/Application Support/tako/setup/`、Windows は `%APPDATA%\tako\setup\`）に展開します。既存の指示は上書きしません
6. **同梱推奨ルールとの比較** — 既存の指示ファイルを、tako が同梱する推奨ルール（言語 / 対話スタイル / Git 運用 / コード品質 / 安全ルール / 提案品質 / 完了検証の 7 項目）と項目レベルで突き合わせ、不足の可能性を具体的に表示します。差分がなければ「差分なし」と明示します。表示のみで、ファイルは書き換えません
7. **設定共有の状況確認** — 別の PC と設定を共有する仕組み（[`tako config`](/guides/cli-reference/#tako-config)）について、配線済みか / `~/.claude` が既に dotfiles などの git で管理されていないか / `gh` にログイン済みかを調べ、状態と次の一手を表示します。**調べるだけ**で、リポジトリの作成や配線は起こりません。配線済みなら状態が 1 行出るだけで、設定を勧める案内は出ません
8. **最終サマリと次の一歩** — 値の由来を `detected` / `previous` / `default` / `input` で表示し、実際に変えた項目だけを最後にまとめます。続けて `tako master` での始め方（起動して日本語で話しかけるだけ）とプロファイルの現在値を案内し、tako 内での対話実行ならその場で master の開始を提案します。エージェント CLI が 1 つだけ認証済みで、依存も揃っている標準ケースでは、人間への質問は 1 つも出ません

### 途中でつまずいても、できるところまで進みます

ログインが済んでいない、依存が入らなかった、MCP 登録に失敗した — こうした段があっても `tako setup` は**そこで止まりません**。認証の要らない段（依存の導入・MCP 登録・指示ファイル・`profiles/default.yaml`・テンプレート展開・シェル統合・`tako` CLI の PATH 設置）を最後まで済ませてから、**人の操作が要るものだけを「残り N 件」として次に打つコマンド付きで**表示します。

もう一度 `tako setup` を実行すると、済んだ段は素通りして残りから再開します。「完了しました」と表示されるのは、残りが 0 件になったときだけです。

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

agy は setup と worker には使えますが、**worker 専用として設計されている**ため master / solo には使えません（[#127](https://github.com/takushio2525/tako/issues/127)）。agy だけが入っている環境では setup を完了できますが、`tako master` を使う前に claude または codex を追加してください。系統ごとの入れ方・つなぎ方は [エージェントの選び方](/agents/) にまとめています。

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
  現在の setup リビジョン: 19（tako v0.8.17）
  適用済みリビジョン: 18（tako v0.8.7 で setup 実行）
  未適用の変更: 1 件

  [rev 19 / v0.8.14 / 2026-09-14] この環境固有のルール（prompt_blocks.append）が長くても予算に収まるようになった
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

次は「エージェント CLI は 3 つとも入っているが、まだどれにもログインしていない」Mac での実行例です。

```
tako セットアップ 環境チェック
─────────────────────────────
  [不足] エージェント CLI の導入: 使える系統がありません
         tako setup を実行すると、ここから最後まで案内します
         [不足] claude: Claude アカウントにログインします (auth)
         [不足] codex: ChatGPT アカウントにログインします (auth)
         [不足] agy: Google アカウントにログインします (auth)
  [不足] tako CLI の PATH: 外部ターミナルからは使えません（設置先が PATH に入っていません）
         tako setup bootstrap path で設置できます
  エージェント CLI:
    [検出] claude: /Users/<ユーザー名>/.local/bin/claude（未認証 / プラン不明）
    [検出] codex: /Users/<ユーザー名>/.local/bin/codex（未認証 / プラン不明）
    [検出] agy: /Users/<ユーザー名>/.local/bin/agy（未認証 / プラン不明）
  [OK] tmux: /opt/homebrew/bin/tmux
  [OK] git: /usr/bin/git
  [OK] tailscale: /usr/local/bin/tailscale
  [OK] フルディスクアクセス: 付与済み（許可ダイアログは表示されません）

  スリープ防止: mode=while-agents-running, power=ac-only
      設定変更: tako sleep-guard set --mode <mode> --power <condition>
  [不足] Claude MCP: tako が未登録（tako setup-mcp で登録できます）
  [不足] codex MCP: tako が未登録（tako setup-mcp で登録できます）
  [不足] agy MCP: tako が未登録（tako setup-mcp で登録できます）
  [OK] Codex: master 起動時にも一時注入
  [情報] agy: worker 専用（master は非対応）
  [情報] config.yaml: 未作成
  [情報] ~/.claude/CLAUDE.md: 未作成
  [情報] ~/.codex/AGENTS.md: 未作成
  [情報] ~/.gemini/GEMINI.md: 未作成
  [情報] エージェント共通ルール同期: 未設定
  [OK] スリープ防止: mode=while-agents-running, power=ac-only
  [情報] 蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）
  [情報] 設定共有: 未配線（複数デバイスで同じ AI 設定を使うなら `tako config init`）
  [情報] プロファイル: 未作成（tako master で自動生成されます）

残り 4 件（ここから先は人の操作が必要です）:
  1. Claude アカウントへのログイン
     claude auth login
     ブラウザでの操作が要るため tako は代行しません
  2. claude への tako MCP 登録
     tako setup-mcp
     登録が無いと AI から tako の画面を操作できません
  3. codex への tako MCP 登録
     tako setup-mcp
     登録が無いと AI から tako の画面を操作できません
  4. agy への tako MCP 登録
     tako setup-mcp
     登録が無いと AI から tako の画面を操作できません
済んだら tako setup をもう一度実行してください（残りはここから再開します）
```

読み方は 4 つのラベルだけです。`[OK]` は済んでいるもの、`[不足]` はまだ済んでいないもの、`[検出]` と `[情報]` は状態の報告（対応は不要）です。末尾の「残り N 件」が、**人の操作が要るぶんだけ**を抜き出したものです。ここが 0 件になれば、その行ごと出なくなります。

`--check` は表示だけで、設定は一切書き換えません。

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

codex には、tako と通信するための環境変数を MCP サーバーへ渡す設定（変数名の一覧）も一緒に書き込まれます。トークンそのものは書き込まれません。

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

**tako の中のターミナル**で出た場合は、tako を再起動してみてください（`tako` の置き場所は tako の起動時に解決されます）。

**tako の外**のターミナルで出た場合は PATH が通っていません。`tako setup bootstrap path` を実行してから、**ターミナルを開き直して**もう一度試してください（PATH の設置は次に開くターミナルから効きます）。`tako setup --check` の `tako CLI の PATH` の行で、設置済みかどうかを確認できます。

### `tako setup` が「エージェント CLI が見つかりません」と言う

claude / codex / agy のいずれも PATH にありません。**この状態でも `tako setup` をそのまま実行して構いません** — 公式インストーラでの導入・PATH 通し・ログイン案内までを順に案内します（何をどこに入れるかは実行前に必ず表示され、管理者権限は使いません）。

系統を指定して入れたいときは `tako setup bootstrap install --agent codex` のように 1 コマンドで足せます。状態だけ見るなら `tako setup bootstrap status-all` です。**ログインだけはブラウザ操作が必要なのでご自身で**行ってください（`claude auth login` / `codex login` / 引数なしの `agy` 起動）。

### MCP ツールが認識されない（AI が tako を操作できない）

1. `tako setup --check` でエージェント別の MCP 状態を確認
2. 未登録のものがあれば `tako setup-mcp` を実行（claude / codex / agy をまとめて登録します）
3. エージェントを一度終了し、**tako の中のターミナルで**起動し直す（tako の外からは安全のため tako を操作できません）
4. codex で見えないままなら `codex mcp list` を確認してください。`tako` の `env_vars` に `TAKO_SOCKET` / `TAKO_TOKEN` が並んでいないと 0 ツールになります（`tako setup-mcp --agent codex` で入れ直せます）

### tako 起動時にクラッシュする / 開けない

quarantine 属性（ダウンロードしたアプリに付く隔離マーク）が原因のことがあります。解除してから再度起動してください:

```bash
xattr -dr com.apple.quarantine /Applications/tako.app
```

### 再起動したらタブが消えていた

tmux バックエンドが無効になっている可能性があります。`tako persist` で状態を確認し、`tako persist on` で有効化してください。tmux 自体が未インストールの場合は `brew install tmux` で導入すると、実行中プロセスごと完全復元されるようになります。

## 次のステップ

- [クイックスタート](/getting-started/quickstart/) — `tako master` を起動して AI オーケストレーションを最短で体験する
- [タブ＆ペイン管理](/features/tabs-and-panes/) — 画面分割とタブの使い分け
- [キーボードショートカット](/guides/keyboard-shortcuts/) — 分割・移動・タブ操作の打鍵を覚える
- [オーケストレーションとは](/features/orchestration/) — AI エージェントを並列に働かせる tako の目玉機能
- [スマホからのリモート接続](/features/remote/) — 外出先のスマホから master と話し、ファイルを直す
- [設定とカスタマイズ](/guides/settings/) — テーマ・表示言語・入力予測などの調整
- [CLI リファレンス](/guides/cli-reference/) — `tako` コマンド全一覧
- [リリースノート](/releases/) — 各バージョンの変更内容
