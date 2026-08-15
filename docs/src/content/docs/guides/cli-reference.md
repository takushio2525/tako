---
title: CLI リファレンス
description: tako コマンド全 69 種の逆引き一覧 — 目的・使い方・実行例・よく使うオプション
---

`tako` CLI は、ターミナルの画面操作（ペイン分割・テキスト送信・レイアウト変更など）をコマンドとして実行するためのツールです。シェルスクリプトからの自動化にも、AI エージェントからの操作にも使われます。

トップレベルのコマンドは **69 種**、サブコマンドまで含めると 223 種あります。ほぼすべてが同名の MCP ツールと 1:1 で対応しており（[MCP ツール一覧](/guides/mcp-tools/)）、人ができる操作は AI も同じ経路で実行できます。

## 共通の前提

- **tako の中のターミナルで実行する**のが基本です。tako の外（通常のターミナル）で実行すると、接続情報が見つからない旨のエラーになります（`tako setup` / `tako setup-mcp` / `tako platform` / `tako recover` など一部はアプリ未起動でも動作します）
- **ペイン ID の自動特定**: tako のペイン内から実行すると、環境変数 `TAKO_PANE_ID` から「自分がいるペイン」が自動で分かります。`--pane` を省略したときの対象は呼び出し元ペインです
- **ペイン ID の調べ方**: `tako list` で全ペインの ID・タイトル・作業ディレクトリが JSON で確認できます
- **困ったら `--help`**: すべてのコマンド・サブコマンドで使えます

```bash
tako --help
tako split --help
tako orchestrator spawn --help
```

## コマンド早見表

やりたいことから引くための全 69 コマンドの一覧です。詳細のあるものはリンクから飛べます。

### 画面を操作する

| コマンド | 何をするか |
|---|---|
| [`split`](#tako-split) | 隣に新しいペインを生やす |
| [`send`](#tako-send) | ペインへテキスト・コマンドを送る |
| [`read`](#tako-read) | ペインの画面内容を読む |
| [`list`](#tako-list) | タブ・ペインの構成を JSON で得る |
| [`focus`](#tako-focus) | フォーカスを移す |
| [`scroll`](#tako-scroll) | スクロールバックを動かす |
| [`close`](#tako-close) | ペインを閉じる |
| [`title`](#tako-title) | タイトル・役割ラベルを設定する |
| [`resize`](#tako-resize) | ペインの取り分を変える |
| [`equalize`](#tako-equalize) | 全ペインを均等化する |
| [`tab`](#tako-tab) | タブの作成・切替・改名・ペイン移動 |
| [`window`](#tako-window) | 複数ウィンドウの操作 |
| [`collapse`](#タブ枠の折りたたみ) | サイドバーのタブ枠を折りたたむ |
| [`pin`](#表示設定のトグル) | プレビューをフローティング化する |

### ファイルを見る・編集する

| コマンド | 何をするか |
|---|---|
| [`open`](#tako-open) | ファイルをプレビューペインで開く |
| [`preview`](#プレビューの操作) | PDF・画像のズーム / ページ / パン |
| [`preview-outline`](#プレビューの操作) | Markdown 見出し・PDF 目次へジャンプ |
| [`preview-link-list`](#プレビューの操作) | PDF 内リンクの一覧 |
| [`preview-follow-link`](#プレビューの操作) | PDF 内リンクをたどる |
| [`preview-reload`](#プレビューの操作) | ライブリロードの ON/OFF |
| [`preview-cache`](#プレビューの操作) | 画像キャッシュ上限の確認・変更 |
| [`preview-changelog`](#プレビューの操作) | git 履歴ビューへの切替 |
| [`edit`](#tako-edit) | プレビュー上での軽量編集 |
| [`file`](#tako-file) | パスコピー / Finder / リネーム / ゴミ箱 |
| [`tree`](#tako-tree) | ファイルツリーへフォルダを追加・削除 |
| [`video`](#tako-video) | 動画の再生・一時停止・シーク |
| [`web`](#tako-web) | Web ビューペインの操作 |
| [`run`](#tako-run--run-default) | ファイルを実行する（Code Runner） |
| [`run-default`](#tako-run--run-default) | 拡張子ごとの実行コマンド既定 |
| [`run-interactive`](#tako-run-interactive) | 入力が要るコマンドを可視ペインへ委譲 |
| [`run-interactive-status`](#tako-run-interactive) | 委譲したコマンドの完了確認 |

### 片付ける・保つ

| コマンド | 何をするか |
|---|---|
| [`background`](#tako-background--foreground--backgrounded) | ペインを退避する（プロセスは維持） |
| [`foreground`](#tako-background--foreground--backgrounded) | 退避中ペインを戻す |
| [`backgrounded`](#tako-background--foreground--backgrounded) | 退避中ペインの一覧 |
| [`tmux`](#tmux-管理) | tmux セッションの一覧・kill・取り込み |
| [`persist`](#表示設定のトグル) | セッション永続化の ON/OFF |
| [`recover`](#tako-recover) | レイアウトをバックアップ世代から復旧 |
| [`logs`](#tako-logs--sessions) | ペインの平文ログの参照・設定 |
| [`sessions`](#tako-logs--sessions) | 会話セッションの発見と復元 |

### git

| コマンド | 何をするか |
|---|---|
| [`git log` / `diff` / `show`](#情報を読む) | 履歴・差分・コミット詳細を JSON で得る |
| [`git stage` / `unstage` / `commit`](#変更を記録する) | ステージング・コミット |
| [`git push` / `pull`](#変更を記録する) | リモートとの同期 |
| [`git checkout` / `branch` / `merge`](#ブランチを操作する) | 切替・作成・マージ |
| [`git conflicts` / `abort` / `resolve`](#コンフリクトを解く) | コンフリクトの確認・中止・AI 解消 |

### AI と連携する

| コマンド | 何をするか |
|---|---|
| [`master`](#tako-master) | 司令塔の AI を起動する |
| [`solo`](#tako-solo) | 1 対 1 の AI を起動する |
| [`orchestrator`](#オーケストレーター) | worker の起動・監視・回収など |
| [`task`](#tako-task) | タスクチェックポイントと受け入れゲート |
| [`mcp serve`](#tako-mcp-serve) | MCP stdio ブリッジ |
| [`agents`](#tako-agents) | エージェント共通ルールの同期 |
| [`show-command`](#tako-show-command) | コピー / 実行ボタンつきのコマンド提案カードを出す |

### 表示と設定

| コマンド | 何をするか |
|---|---|
| [`ui-mode`](#tako-ui-mode) | かんたん表示（GUI モード）とターミナル表示の切替 |
| [`chat copy`](#tako-chat-copy) | かんたん表示の会話本文をコピーする |
| [`theme`](#tako-theme) | テーマ・色・フォント |
| [`lang`](#tako-lang) | UI 表示言語（日本語 / 英語） |
| [`settings`](#tako-settings) | 設定画面を開く |
| [`panel`](#表示設定のトグル) | サイドバーの表示・幅・ビュー |
| [`autosuggest`](#tako-autosuggest) | 入力予測（ゴーストテキスト） |
| [`autorename`](#表示設定のトグル) | タブ・ペインの AI 自動リネーム |
| [`portdetect`](#表示設定のトグル) | ポート検知と提案チップ |
| [`confirm-close`](#表示設定のトグル) | 閉じる確認ダイアログ |
| [`limit-resume`](#tako-limit-resume) | 利用上限後の自動復帰（ペイン単位） |
| [`limit-service`](#表示設定のトグル) | 利用制限表示のサービス切替 |
| [`welcome`](#tako-welcome) | 初回起動バナーの状態・再表示 |

### 開く・戻る

| コマンド | 何をするか |
|---|---|
| [`open-in`](#tako-open-in--recent--ssh-hosts) | ディレクトリ / リポジトリ / SSH を新タブで開く |
| [`recent`](#tako-open-in--recent--ssh-hosts) | 最近開いた項目の一覧・クリア |
| [`ssh-hosts`](#tako-open-in--recent--ssh-hosts) | `~/.ssh/config` の Host 一覧 |

### リモート・セットアップ・診断

| コマンド | 何をするか |
|---|---|
| [`remote`](#リモートアクセス) | スマホ等からの接続サーバー |
| [`setup`](#tako-setup) | 質問ゼロの自動セットアップ |
| [`setup-mcp`](#tako-setup-mcp) | MCP 登録だけ行う |
| [`update`](#tako-update) | アプリの更新確認・実行 |
| [`platform`](#tako-platform) | この環境で使える機能の一覧 |
| [`fda`](#その他) | フルディスクアクセスの状態確認 |
| [`sleep-guard`](#その他) | スリープ防止 |
| [`telemetry`](#その他) | エラーレポート送信の ON/OFF |
| [`stale-binary`](#その他) | 古い claude バイナリの検知・張り直し |

## セットアップ

### tako setup

AI 連携に必要な設定を質問ゼロでまとめて行います。claude / codex / agy の認証・プランを検出し、前回値、安全な既定値の順で不足を補い、値の由来と最終サマリを表示します。CLI が 1 つ・認証済みの標準ケースは人間の入力なしで完走します。tako アプリが起動していなくても実行できます。詳しくは[セットアップガイド](/getting-started/#3-tako-setup--質問ゼロの自動セットアップ)を参照してください。

```bash
# 自動セットアップ（標準ケースは質問ゼロ）
tako setup

# 標準入力を一切読まずに自動セットアップ
tako setup --yes

# 全回答を JSON、ファイル、標準入力のいずれかで指定
tako setup --answers '{"selected_agent":"codex","provider_plans":{"gpt":"plus"}}'
tako setup --answers @setup-answers.json
generate-answers | tako setup --answers -

# 前回設定を AI と個別に見直す
tako setup --review

# 環境チェックだけ実行（CLI の有無・認証・プラン・MCP・セットアップ状態を表示）
tako setup --check

# アップデート追従状況を表示（前回セットアップ以降に setup へ入った変更の一覧）
tako setup --changes
tako setup --changes --json

# セットアップ状態をリセットして最初からやり直す
tako setup --reset
```

`--answers` は `selected_agent`、`provider_plans`、`instruction_content`、`profile`、`projects`、`orchestrator`、`sleep_guard` を受け取ります。同じ JSON は MCP `tako_setup` でも使えるため、AI に日本語で希望を伝えてセットアップを代行させられます。`projects` は指定時に全登録を置き換えます。

### tako setup-mcp

Claude Code の設定ファイル（`~/.claude/settings.json`）に tako の MCP サーバーを登録します。対話なしで登録だけしたいときに使います。

```bash
tako setup-mcp             # ユーザー全体に登録（既定）
tako setup-mcp --project   # 現在のディレクトリだけに登録
```

### tako update

アプリのアップデートを確認・実行します。Homebrew / ZIP のどちらでインストールしたかを自動判別します。更新チェックは「自分の OS 向けのアセットを含む最新リリース」を探すため、他 OS 版だけが先に出ている状況で「更新はあるが入れられない」通知が出ることはありません。

```bash
tako update status      # 配布系統・現在バージョン・PATH 上の重複 CLI を診断
tako update check       # 新しいバージョンがあるか確認（更新はしない）
tako update apply       # 更新を実行（レイアウト保存 → 更新 → 自動再起動）

# brew での更新が失敗して詰まったときの復旧
tako update repair      # Homebrew の管理情報を修復
tako update apply-zip   # zip 経由で強制更新
```

## 基本操作

### tako split

ペインを分割して新しいペインを作ります。新しいペインの ID が出力されるので、続けて `send` や `read` の対象にできます。

```bash
tako split --right                              # 右に分割（既定）
tako split --down                               # 下に分割
tako split --right -- npm run dev               # 分割してコマンドを実行
tako split --right --ratio 0.3 -- htop          # 新ペインの取り分を 30% に
tako split --down --cwd ~/Documents/webapp --focus
```

| オプション | 説明 |
|---|---|
| `--right` / `--down` / `--up` / `--left` | 分割方向（省略時は右） |
| `--ratio <0.0–1.0>` | 新ペイン側の取り分（省略時は等分） |
| `--cwd <パス>` | 新ペインの作業ディレクトリ |
| `--focus` | 新ペインにフォーカスを移す |
| `--pane <ID>` / `--tab <ID>` | 分割元の指定（省略時は呼び出し元ペイン） |
| `-- <コマンド>` | シェルの代わりに実行するコマンド |

### tako send

ペインにテキストを送信します。既定で末尾に改行（Enter）が付くので、コマンドはそのまま実行されます。

```bash
tako send --pane 3 "echo hello"
tako send --pane 3 --no-newline "yes"
tako send --pane 3 --await-prompt "テストを実行して結果を教えて"
```

| オプション | 説明 |
|---|---|
| `--pane <ID>` | 送信先ペイン（省略時は呼び出し元） |
| `--no-newline` | 末尾に改行を付けない |
| `--await-prompt` | 入力欄が表示されるのを待ってから送信 |

:::note[全画面 TUI への送達は検証付き]
Claude Code のような全画面アプリへの送信は、貼り付け → Enter 送信 → 入力欄へ反映されたかの検証、という確認ループ付きで配送されます。長い指示文が入力欄に残ったままになる心配はありません。相手が生成中でキューに積まれた場合も検知されるので、指示が消えることはありません。
:::

### tako read

ペインの画面内容をテキストとして取得します。

```bash
tako read --pane 3
tako read --pane 3 --lines 50
```

### tako list

タブ・ペインの構成を JSON で出力します。各ペインの ID・タイトル・作業ディレクトリ（cwd）・実行状態・listen 中のポートなどが含まれます。ペイン ID を調べる出発点です。

```bash
tako list
```

### tako focus

指定ペインにフォーカス（入力先）を移します。

```bash
tako focus 3        # ID 指定
tako focus --right  # 方向指定
```

### tako scroll

ペインのスクロール位置を動かします。

```bash
tako scroll --pane 3 --delta 100  # 100 行分過去へ
tako scroll --pane 3 --to 0       # 最下部（最新）に戻る
```

### tako close

ペインを閉じます（中のプロセスも終了します）。タブ最後の 1 ペインを閉じるとタブごと閉じます。

```bash
tako close --pane 3
tako close --pane 3 --force   # 実行中の worker でも強制的に閉じる
```

:::caution
`close` はプロセスを終了します。「画面から消したいが処理は続けたい」場合は [`tako background`](#tako-background--foreground--backgrounded) を使ってください。エージェントや実行中プロセスがあるペインでは、GUI 側で確認ダイアログが出ます（`tako confirm-close` で切替可）。
:::

### tako title

ペインの表示タイトルと役割ラベルを設定します。

```bash
tako title --pane 3 "dev server"
tako title --pane 3 --role worker-1 "修復係"
tako title --pane 3 ""   # 空文字でクリア（自動リネームに戻る）
```

## レイアウト操作

### tako resize

ペインの取り分（画面に占める割合）を調整します。

```bash
tako resize --pane 3 --dx 0.1       # 横方向に 10% 広げる
tako resize --pane 3 --dy -0.1      # 縦方向に 10% 縮める
tako resize --pane 3 --share-x 0.6  # 横の取り分を 60% ぴったりに
```

| オプション | 説明 |
|---|---|
| `--dx` / `--dy` | 相対変更（正 = 広げる、負 = 縮める） |
| `--share-x` / `--share-y` | 絶対指定（0.0〜1.0） |

### tako equalize

タブ内の全ペインを均等サイズに整えます。

```bash
tako equalize
tako equalize --tab 2
```

### tako tab

```bash
tako tab new                              # 新しいタブを作成
tako tab new --title "API Server"
tako tab rename --tab 2 "フロントエンド"    # タブ名を変更
tako tab rename ""                        # 手動指定を解除（自動リネームに戻る）
tako tab select 2                         # タブを切り替え
tako tab move-pane 2 --pane 5             # ペインを別タブの末尾へ移動
tako tab move-pane --pane 5 --target 3 --down  # 同タブ内で並べ替え
```

### tako window

複数ウィンドウを扱います。タブバーは**全ウィンドウ共通で全タブを表示**し、タブをクリックするとそのウィンドウに表示が移ります（1 つのタブが同時に 2 つのウィンドウに出ることはありません）。

```bash
tako window list                          # ウィンドウ一覧
tako window new                           # 新規タブ付きで新しいウィンドウ
tako window new --tab 3                   # 既存タブ 3 を分離して新ウィンドウへ
tako window move-tab --tab 3 --window 2   # タブを別ウィンドウへ移動
tako window focus 2                       # ウィンドウを前面化
tako window close 2                       # 閉じる（タブは残存ウィンドウへ合流）
```

### タブ枠の折りたたみ

サイドバーのタブ枠を折りたたみ / 展開します（配下のバックグラウンド行を隠します）。

```bash
tako collapse --tab 2 on    # 折りたたむ
tako collapse --tab 2 off   # 展開
tako collapse --tab 2       # トグル
```

## ファイル・プレビュー

### tako open

ファイルをプレビューペインで開きます。コードはシンタックスハイライト付き（210 以上の形式）、`.md` はレンダリング表示、画像・PDF・動画にも対応します。

```bash
tako open src/main.rs
tako open README.md --mode code     # Markdown をソース表示で
tako open src/app.tsx --right       # 右に分割して開く
```

| オプション | 説明 |
|---|---|
| `--mode <code\|markdown\|image\|pdf\|video>` | 表示モードの明示指定 |
| `--right` / `--down` / `--up` / `--left` | 分割して新しいプレビューペインで開く |
| `--pane <ID>` | 基準ペイン（相対パスの解決と表示先に使う） |

### プレビューの操作

開いているプレビューを外から操作します。

```bash
# PDF・画像のズーム / ページ / パン（引数なしで現在状態）
tako preview

# Markdown 見出し・PDF 目次のアウトライン（1 始まりの項目番号でジャンプ）
tako preview-outline
tako preview-outline --item 4

# Markdown / PDF のリンク（開くのは http / https のみ）
tako preview-link-list
tako preview-follow-link

# Markdown のコードブロックを装飾なしでコピー（出現順 0 始まり・省略で先頭）
tako preview-copy-code
tako preview-copy-code 2

# ファイルが書き換わったら自動で反映（既定 ON）
tako preview-reload          # 現在値
tako preview-reload off

# デコード済み画像キャッシュの上限（既定 512 MiB、256〜8192）
tako preview-cache           # 上限・使用量・件数
tako preview-cache 1024

# コード表示 ⇔ git 履歴（チェンジログ）表示の切替
tako preview-changelog
```

### tako edit

コードプレビュー上での軽量編集です。開始 → 全文適用 → 保存の 3 段階で、外部から書き換えられていた場合は保存を拒否します。

```bash
tako edit --help    # 開始 / 全文適用 / 保存のサブコマンドを確認
```

### tako file

ファイルツリーの右クリックメニューに相当する操作群です。

```bash
tako file copy-path src/main.rs        # 絶対パスを出力（--relative で相対）
tako file reveal src/main.rs           # Finder で表示
tako file open-terminal ~/Documents/webapp  # ペイン内で cd
tako file rename old.txt new.txt
tako file create src helper.ts         # path 配下に name で作成
tako file mkdir src components
tako file trash old-notes.md           # ゴミ箱へ移動
```

### tako tree

ファイルツリーに表示するフォルダを明示的に追加・削除します。AI が作業対象プロジェクトを提示するのに使います。

```bash
tako tree add ~/Documents/webapp
tako tree remove ~/Documents/webapp
tako tree list
```

### tako video

プレビューペインで動画を開いているときの再生操作です。

```bash
tako video play --pane 4
tako video pause --pane 4
tako video toggle --pane 4
tako video seek 90 --pane 4   # 90 秒地点へ
```

### tako web

URL をネイティブ Web ビューペイン（macOS = WKWebView）として開きます。ペイン内ではクリック・スクロール・文字入力を普通のブラウザ同様に行えます。`hide` でページを生かしたまま dock へ退避し、`show` で呼び戻せます。

```bash
tako web open http://localhost:5173 --right
tako web list                                # 一覧
tako web hide                                # dock へ退避（ページは生きたまま）
tako web show 3
tako web nav back                            # 戻る（forward / reload / URL も可）
tako web eval 'document.title'               # JS 評価
tako web eval-result <token>                 # 評価結果の回収
tako web read                                # URL・タイトル・読み込み状態
tako web close
```

### tako run / run-default

**Code Runner**。ファイル内の `tako:run` 宣言、または拡張子ごとの既定コマンドで、新しいペインを分割して実行します。プレビューヘッダの再生ボタンと同じ経路です。

```bash
tako run script.py                 # 実行（新ペインを分割）
tako run script.py --profile test  # プロファイルを指定
tako run script.py --list          # 使えるプロファイルの一覧
tako run script.py --wait          # 完了まで待って終了コードを返す

tako run-default                   # 拡張子ごとの既定コマンド一覧
tako run-default py "python3"      # 既定を設定
tako run-default py --remove       # 既定を削除
```

### tako run-interactive

ユーザーの入力が必要なコマンド（パスワードや対話ログインを伴うものなど）を、見えるペインへ委譲します。分割 → タイトル設定 → コマンド投入をまとめて行い、ペイン ID を返します。AI が「自分では答えられない対話」を人間に渡すための経路です。

```bash
tako run-interactive --help
tako run-interactive-status --help
```

## バックグラウンド退避（たまり場）

### tako background / foreground / backgrounded

「処理は動かしたまま、画面からだけ消す」操作です。詳しくは[たまり場](/features/shelving/)を参照してください。

```bash
tako background --pane 3       # 退避（プロセスは生きたまま）
tako backgrounded              # 退避中ペインの一覧（JSON）
tako foreground 3              # 画面へ復帰（省略時は元いたタブへ）
tako foreground 3 --target 5 --direction down
```

## git

作業ディレクトリの git リポジトリを操作します。GUI の git パネルと同じ経路です。詳しくは [git 連携](/features/git-integration/)を参照してください。

### 情報を読む

```bash
tako git log                       # 履歴・ブランチ・変更状態（--max-count、既定 200）
tako git diff                      # 差分（既定は未ステージ）
tako git diff --target staged
tako git diff --target a1b2c3d
tako git show a1b2c3d              # コミット詳細（メタ情報 + 変更ファイル一覧）
tako git show a1b2c3d --file src/main.rs   # そのファイルの diff も含める
```

### 変更を記録する

```bash
tako git stage src/main.rs         # パス省略で全変更をステージ
tako git unstage src/main.rs       # パス省略で全アンステージ
tako git commit -m "[修正] ログイン失敗を直す"
tako git push
tako git pull
```

### ブランチを操作する

**破壊的になりうる操作は、既定では実行しません。**「何が起きるか」を先に出し、`--yes` を付けたときだけ実行します。

```bash
# 切替: 未コミット変更があると、持ち越すもの / 衝突するものを出して止まる
tako git checkout main
tako git checkout main --yes

# 作成して切り替え
tako git branch fix/login
tako git branch fix/login --from main --no-checkout

# マージ: 作業ツリーに触れずにコンフリクトを事前予測して出す
tako git merge feature/x
tako git merge feature/x --yes --no-ff
```

### コンフリクトを解く

```bash
tako git conflicts                 # 未解決ファイル・進行中の操作を JSON で
tako git abort                     # merge / rebase / cherry-pick / revert を中止
tako git resolve                   # 解消エージェントを同じタブに起動
tako git resolve --agent codex
```

`tako git resolve` は、リポジトリ・未解決ファイル・マージ元 / 先を含む解消プロンプトを自動で投入します。文面は `<データディレクトリ>/orchestrator/conflict-resolver.md` を置けば差し替えられます。

## tmux 管理

tako は tmux セッションの「見える化と片付け」もできます。詳しくは [tmux バックエンド](/features/tmux-backend/)を参照してください。

```bash
tako tmux list                     # 全セッション一覧（tako のペインとの対応付き）
tako tmux cleanup                  # 取り残されたセッションの一括掃除
tako tmux kill --session my-session
tako tmux kill --session my-session --window 1
tako tmux open my-session --right  # 外部セッションを現在のタブへ取り込む
tako tmux select-window 1 --pane 3
tako tmux resize --session my-session --cols 80 --rows 24
tako tmux resize --session my-session --reset
```

### tako recover

タブ・ペインが大量に消えてしまったときの復旧です。**tako を終了してから** 適用します。

```bash
tako recover                   # layout.json とバックアップ世代の一覧
# ここで tako を終了する
tako recover --apply 1         # 世代 1〜3 を指定して復元
tako recover --apply good      # 最後に復元へ成功した「良品」へ戻す
# tako を再起動する
```

### tako logs / sessions

ペインが死んだ後でも出力を遡ったり、過去の会話を呼び戻したりできます。

```bash
tako logs list                          # ログファイル一覧
tako logs show 3 --lines 500            # 末尾を表示（クローズ済みペインも可）
tako logs status                        # ON/OFF・上限・保存先
tako logs set --enabled true --max-mb 5 --total-max-mb 200

tako sessions list                      # 会話カタログ（新しい順）
tako sessions list --role worker
tako sessions show <session-id>         # メタ情報と会話冒頭
tako sessions resume <session-id>       # 記録された cwd で復元（claude のみ）
```

## 表示・設定のトグル

```bash
# 右サイドバーの情報パネル（--view の値は GUI のタブ表示名と同じ）
tako panel --show --view fleet   # fleet = 全ペイン + tmux セッション
tako panel --view orch           # orch  = master + ワーカーツリー
tako panel --view git            # git   = ブランチ・変更・履歴・diff
tako panel --hide
tako panel --filetree on         # 左のファイルツリー
tako panel --width 360
tako panel --show-hidden on      # ツリーにドット項目も並べる（既定 off）

# プレビューのピン留め（フローティングウィンドウ化）
tako pin --pane 3 on
tako pin --pane 3 off

# セッション永続化（tmux バックエンド）
tako persist                     # 現在状態
tako persist on
tako persist off

# ポート検知（「プレビューを開く？」チップ）
tako portdetect on
tako portdetect off

# タブ・ペイン名の AI 自動リネーム
tako autorename on
tako autorename off

# 閉じる確認ダイアログ（エージェント・実行中プロセスのあるペインのみ確認）
tako confirm-close on
tako confirm-close off

# ステータスバーの利用制限表示をどのサービスにするか
tako limit-service               # 現在値
tako limit-service codex
```

### tako ui-mode

かんたん表示（GUI モード）とターミナル表示を切り替えます。**既定は `terminal`**（従来どおりの表示）です。詳しくは[かんたん表示（GUI モード）](/features/gui-mode/)を参照してください。

```bash
tako ui-mode                     # 現在のモードと、各ペインに出ている表示種別
tako ui-mode gui                 # かんたん表示へ
tako ui-mode terminal            # ターミナル表示へ
tako ui-mode toggle

tako ui-mode release --pane 3    # そのペインだけターミナル表示に戻す（揮発）
tako ui-mode restore --pane 3    # 戻した指定を解除する
```

応答の `pane_display` が、いま各ペインに出ているもの（`terminal` / `starter` / `chat` / `preparing`）を返します。AI はこれを見てから案内できます。

:::note[表示レイヤだけの切替です]
モードを変えても PTY・tmux セッション・実行中のプロセスには影響しません。`release` / `restore` は永続化されないので、再起動すると全ペインがモードどおりの表示に戻ります。
:::

### tako chat copy

かんたん表示の会話本文をクリップボードへコピーします。UI のコピーボタンと同じ経路です。

```bash
tako chat copy                       # 最後の AI 発話（画面と同じプレーンテキスト）
tako chat copy --list                # 添字・role・文字数・コードブロック数の下見
tako chat copy --message 3           # 添字 3 の発話（0 始まり）
tako chat copy --message 3 --code 0  # その発話の 1 つ目のコードブロックだけ（0 始まり）
tako chat copy --markdown            # Markdown ソースのまま
```

### tako show-command

ターミナル領域を縮めて作った専用の帯に、コピー・実行ボタンつきのコマンドカードを出します。**AI が会話本文に書いたコマンドは折り返しでコピーが壊れる**ため、実行を頼むコマンドはこれで提示します（カードは永続化されません）。

```bash
tako show-command "cargo test --workspace" --label "テストを流す"
tako show-command --list          # 出ているカードの一覧
tako show-command --copy --index 1   # コマンド番号は 1 始まり
tako show-command --run --index 1    # 新しいペインで実行する（既定はフォーカスを移さない）
tako show-command --dismiss
```

### tako theme

```bash
tako theme                       # 現在のテーマを表示
tako theme dark
tako theme light
tako theme toggle
tako theme colors                # 色キーの一覧と現在値
tako theme preset save mytheme   # 現在の配色をプリセットとして保存
```

`tako theme --help` に色キー・プリセット・フォント指定の全オプションが出ます。

### tako lang

```bash
tako lang            # 現在の表示言語
tako lang en         # 英語
tako lang ja         # 日本語
tako lang system     # OS のロケールに追従
```

### tako settings

設定画面（独立ウィンドウ）を開きます。<kbd>Cmd</kbd>+<kbd>,</kbd> と同じです。一般・外観・Code Runner・セットアップ・スリープ防止・リモート・高度の 7 タブがあります。

```bash
tako settings
tako settings --tab 外観
```

### tako autosuggest

tako 内の zsh に出る入力予測（履歴ベースのゴーストテキスト）です。**既定は ON**。tako が開いたシェルの中だけで効き、`~/.zshrc` は書き換えません。確定は <kbd>→</kbd> または <kbd>Tab</kbd> です。

```bash
tako autosuggest             # 現在状態
tako autosuggest off         # 予測そのものを止める
tako autosuggest on

tako autosuggest tab off     # Tab での確定だけ無効化（→ キーは残る。既定 ON）
tako autosuggest hint off    # 確定キーの案内を今すぐ止める（既定は 10 回で自動終了）
```

:::note[外の zsh には影響しません]
`~/.zshrc` を書き換えないので、tako の外のターミナルの挙動は変わりません。すでに自分で zsh-autosuggestions を導入している場合、tako は二重に読み込まず何もしません（あなたの設定がそのまま使われます）。
:::

### tako limit-resume

利用上限（5 時間制限・週次制限）でエージェントが止まったとき、リセット時刻を過ぎたら
tako が作業を再開させます。**ペイン単位のオプトインで、既定は OFF**。
ペインを右クリックしても同じ設定を切り替えられます。

```bash
tako limit-resume             # 今のペインの状態
tako limit-resume on          # 有効にする
tako limit-resume off

tako limit-resume on --pane 12  # 別のペインを指定
tako limit-resume --all         # 全ペインの状態を一覧
```

発動するのは**上限で止まったとき**だけです。上限の対処ダイアログが出ていれば
「解除まで待つ」相当の選択肢をラベルで選び（`Upgrade …` や
`Continue with usage credits` のような課金・モデル変更を伴う選択肢は選びません）、
ダイアログが無ければ継続を促すメッセージを送ります。

:::note[勝手に動きすぎないようにしてあります]
許可の確認ダイアログ・API エラー・普通の待機状態では発動しません。画面が動いている
あいだ（生成中）と、入力欄にあなたの打ちかけの指示があるときも触りません。
再開の試行は 1 回の上限あたり 3 回までで、それ以上は繰り返しません。
実行した記録は tako のデータディレクトリの `supervisor.log` に残ります。
:::

### tako welcome

初回起動時にタブバー直下へ出る案内バナーの操作です。

```bash
tako welcome           # 表示状態と案内すべきコマンド
tako welcome show      # もう一度出す
tako welcome dismiss   # 消す
```

## 開く・戻る

### tako open-in / recent / ssh-hosts

```bash
tako open-in dir ~/Documents/webapp   # 新タブで開く（ファイルツリーにも追加）
tako open-in repo ~/Documents/webapp  # git root を自動検出して開く
tako open-in remote myserver          # SSH ホストへ接続する新タブ

tako recent                           # 最近開いた項目
tako ssh-hosts                        # ~/.ssh/config の Host 一覧
```

## リモートアクセス

スマホなど外部デバイスから tako の画面を見る・操作するための機能です。仕組みと安全性の詳細は[リモートアクセス](/features/remote/)を参照してください。

transport は **[Tailscale](https://tailscale.com/) Serve のみ**です。あなたの tailnet 内限定の恒久固定 URL `https://<ホスト名>.<tailnet>.ts.net` で公開され、通信は WireGuard で**エンドツーエンド暗号化**されます。URL は public internet には存在しません。

```bash
# 初回だけ: 対話ウィザード（Tailscale 導入 → ログイン → HTTPS → serve 設定 → QR 生成）
tako remote setup
tako remote setup --yes

# 起動・停止・状態
tako remote start
tako remote status      # 恒久固定 URL・登録端末数（secret は含まれない）
tako remote stop

# ペアリング済み端末の一覧・失効（失効すると接続中の端末は即座に切断）
tako remote devices list
tako remote devices revoke <device-id>

# 中身を覗く
tako remote agents                          # 動作中の AI エージェント一覧
tako remote messages <session-id> --tail 30 # 会話ログ末尾
tako remote scrollback <pane-id> --lines 1000
```

Tailscale が未セットアップの場合、`tako remote start` は不足項目を列挙して `tako remote setup` を案内します。ステータスバーのリモートチップからも同じ起動ができます。

## オーケストレーター

複数の AI エージェントを親子で連携させる機能です。考え方は[オーケストレーションとは](/features/orchestration/)、使い方は [tako master 実践ガイド](/features/orchestrator/)を参照してください。

このうち、**日常であなたが打つのは `tako master`（または `tako solo`）だけ**です。それ以外の `tako orchestrator` 系コマンドは、通常は master（AI）自身が内部で実行するもので、手動操作やスクリプトからの自動化用に公開されています。

### tako master

司令塔となる AI を起動します。以後はこの master に自然言語で作業を依頼するだけで、子 worker の起動・監視・検収・回収が自動で回ります。

**既定では今いるペインでそのまま起動します**（新しいタブは作りません）。

```bash
tako master              # default プロファイル、今のペインで
tako master --tab        # 新しいタブを作って起動
tako master -fast        # プロファイル指定（profiles/fast.yaml）
tako master dev          # 旧形式のサフィックス指定（"master-dev" タブ）
```

### tako solo

worker を使わず、AI と 1 対 1 で作業するモードです。worker の spawn を**あえて禁止**し、その AI 自身が手を動かします。既定の思考量は `high` で、Pro プランでも運用しやすい設計です。master と同じプロファイル引数が使えます。

```bash
tako solo
tako solo --tab
tako solo -fast
```

### tako orchestrator projects

master が作業対象にできるプロジェクトの登録・管理です。**通常は master に「◯◯のリポジトリを追加して」と頼むだけで済みます**。登録内容は `~/Library/Application Support/tako/orchestrator/projects.yaml` に保存されます。

```bash
tako orchestrator projects list
tako orchestrator projects add --key webapp --cwd ~/Documents/webapp --description "Web アプリ"
tako orchestrator projects remove --key webapp
```

### tako orchestrator profiles

master・worker が使うモデルや思考量（effort）の設定です。

```bash
tako orchestrator profiles list
tako orchestrator profiles show
tako orchestrator profiles set default --model claude-opus-4-6 --effort max
tako orchestrator profiles set default --clear-model   # claude 既定モデルに戻す
tako orchestrator profiles set sol --master-agent codex --model gpt-5.6-sol --effort xhigh
tako orchestrator profiles set default --ctx-threshold 55   # 自動ハンドオフの閾値（50〜60）
```

| `set` の主なオプション | 説明 |
|---|---|
| `--master-agent` / `--clear-master-agent` | master のエージェント CLI（claude / codex。agy は master 非対応） |
| `--model` / `--clear-model` | master のモデル指定 / 解除 |
| `--effort` | master の思考量 |
| `--worker-model` / `--clear-worker-model` | 子 worker 用の固定モデル / 解除 |
| `--worker-effort` | 子 worker の思考量 |
| `--worker-agent` / `--clear-worker-agent` | worker の既定エージェント CLI（claude / codex / agy） |
| `--worker-model-policy` | `inherit` / `fixed` / `delegate` |
| `--ctx-threshold` / `--clear-ctx-threshold` | 自動ハンドオフを始めるコンテキスト使用率（%。50〜60。既定 60） |
| `--auto-handoff` | 閾値を超えたら tako が引き継ぎを促すか（既定 true） |

master のエージェント CLI を codex にすると、`tako master -<プロファイル名>` で codex が tako の MCP ツールに接続された状態で立ち上がります。master が claude 以外のとき、プロファイルの `model` / `effort` は claude worker へ継承されません。

:::caution
`[1m]` 付きモデル（1M コンテキスト版）は Max / API プラン限定です。Pro プランで指定すると master が起動できません。
:::

### tako orchestrator accounts

worker ごとに別アカウントを使い分けるためのレジストリです。既定の資格情報をそのまま使うアカウントは `--inherit` で登録します（既定パスを明示指定すると警告が出ます）。

```bash
tako orchestrator accounts list
tako orchestrator accounts show work
tako orchestrator accounts add work --inherit
tako orchestrator accounts remove work
```

### tako orchestrator spawn / run

子 worker をペインに起動し、プロンプトを渡します。

```bash
tako orchestrator spawn --project webapp --prompt "ログインページを実装して" --label login
tako orchestrator spawn --project webapp --prompt "..." --account work
tako orchestrator run --project webapp --prompt "テストを実行して失敗があれば直して"
```

| `spawn` のオプション | 必須 | 説明 |
|---|---|---|
| `--project` | ○ | プロジェクトキー（登録済みのもの） |
| `--prompt` | ○ | worker に渡す初期プロンプト |
| `--label` | | ペインタイトルに付けるラベル |
| `--account` | | この worker だけ別アカウントで動かす |
| `--model` / `--effort` | | モデル・思考量（省略時はプロファイル設定） |
| `--pane` / `--tab` | | worker ペインをどこに出すか |

`run` は spawn して `run_id` を返す非同期実行です。進捗と結果は次で回収します。

```bash
tako orchestrator run-status            # 全 run の一覧
tako orchestrator run-status <run_id>
tako orchestrator run-result <run_id>
```

| `run` のオプション | 説明 |
|---|---|
| `--timeout <秒>` | 完了待ちの上限（既定 1800 秒） |
| `--auto-close <true\|false>` | 完了後にペインを自動で閉じるか（既定 true） |
| `--output-lines <N>` | 回収する出力の末尾行数（既定 200） |

### tako orchestrator status / watch / report

worker の状態確認・完了待ち・報告取得です。

```bash
tako orchestrator status --pane 5
tako orchestrator watch --pane 5            # 完了までブロック
tako orchestrator watch --worker <ID>       # ペインが消えても追跡できる
tako orchestrator watch --pane 5 --timeout 600
tako orchestrator report --pane 5 --lines 2000
tako orchestrator report --pane 5 --messages 3   # 直近の発話だけ
```

`watch` は `WORKER_IDLE`（完了）/ `WORKER_ERROR`（異常停止）/ `WORKER_GONE`（消滅）などを 1 行で出力します。`status` はエラー時に種別（`api_error` / `usage_limit` など）と推奨アクションも返します。

#### 自動ハンドオフ（コンテキストが埋まる前に master を交代させる）

master のコンテキスト使用率が閾値（既定 60%）を超えると、tako が master へ引き継ぎ開始を指示します。master は引き継ぎファイルを最新化して `handoff` を呼び、**後任 master が引き継ぎ内容と実態を確認してから前任のペインを閉じます**（後任の起動に失敗しても前任は残ります）。ユーザーの操作は要りません。

閾値は 50〜60% の範囲でプロファイルごとに変えられます。自動で促されるのが煩わしい場合は通知だけ切れます（`handoff` の手動実行は使えます）。

```bash
tako orchestrator profiles set default --ctx-threshold 55
tako orchestrator profiles set default --auto-handoff false
```

引き継ぎファイルは 2 つの節に分かれます。**知識（マシン非依存）** には決定事項・方針・残タスクの意図を、**実行状態（このマシン限定）** には worker とそのペイン番号や実行中のものを書きます。ペイン番号はそのパソコンでしか意味を持たないので、設定を別のパソコンと共有したとき（`tako config`）に知識だけが役に立つ形にしておくためです。節分けの無い古い引き継ぎファイルもそのまま読めます（後任には「番号は実際の画面で確認するように」が伝わり、次に更新されるときに 2 節へ書き直されます）。

### tako orchestrator workers / respond / self / handoff

```bash
tako orchestrator workers          # spawn 済み worker の一覧（ペインの生死と無関係）
tako orchestrator workers --all
tako orchestrator respond --pane 5 # 権限確認ダイアログへ応答（不在時はエラー）
tako orchestrator self             # master/solo が自分の pane・tab・コンテキスト残量と引き継ぎ閾値を知る
tako orchestrator handoff          # master のバトンを新しい master へ渡す
tako orchestrator layout --policy master-reserved --master-ratio 0.5
tako orchestrator supervisor       # worker 自動復旧 supervisor の操作
tako orchestrator ledger           # 委任台帳の操作
```

### tako task

タスクのチェックポイントと受け入れゲート（完了条件）です。クラッシュや利用上限からの再開に使います。

```bash
tako task checkpoint <task_id>     # チェックポイントを記録・更新
tako task list
tako task resume <task_id>         # チェックポイントから worker を再開
tako task update <task_id>         # フェーズを手動で変更

# 受け入れゲート: 「これが通ったら完了」を機械で判定する
tako task gate set <task_id> --command "cargo test" --pr-merged 620
tako task gate check <task_id>
tako task gate show <task_id>
```

### tako agents

エージェント共通ルールを、各 CLI のグローバル指示ファイルへマーカーブロックで同期します。ブロックの外は書き換えません。

```bash
tako agents status
tako agents sync-rules
```

### tako config

AI 系の設定を git リポジトリ 1 本でデバイス間共有します（任意機能）。claude のグローバル指示（`CLAUDE.md` / `snippets` / `commands` / `templates`）と tako の宣言的設定（`profiles` / `projects` / `accounts` / `local-rules` / `settings`）が対象です。

```bash
tako config                          # 配線状態と push / pull 待ちの差分
tako config list                     # 何を共有し何を共有しないかの分類表

# はじめる（どちらか）
tako config init                     # 新しく作る（~/tako-config-sync）
tako config init --remote git@github.com:you/tako-config.git
tako config link git@github.com:you/tako-config.git   # 2 台目: clone して配線
tako config link ~/dotfiles/tako     # 既存リポジトリに繋ぐ

# 同期
tako config push -m "[改善] プロジェクトを追加"
tako config pull
tako config unlink                   # 配線を外す（リポジトリは残る）
```

秘匿情報とマシン固有の状態は、除外リストではなく**ホワイトリスト**で構造的に外れます。カタログに載っていないファイルは共有されません。

| 共有される | 共有されない |
|---|---|
| `CLAUDE.md` / snippets / commands / templates | `.claude.json` / credentials / 会話履歴 |
| profiles / projects / accounts の宣言部 / local-rules | token / control.json / layout.json / sessions.yaml / workers.yaml / ペインログ |
| settings.json（表示設定） | アカウントの `config_dir`、profile の `env`、「バナーを閉じた」等の操作履歴 |

設定内の絶対パスは、ホーム配下なら `~/…` に正規化して保存され、取り込み時にそのデバイスのホームへ戻ります。mac と Windows でホームの位置が違っても同じリポジトリを使えます。

#### `tako setup` からの配線

`tako setup` は質問を増やしません。完了サマリと `tako setup --check` に現在の状態（未配線 / 配線済み / 既に dotfiles などの git で管理されている）と次の一手が出るだけです。配線済みなら状態が 1 行出るだけで、設定を勧める案内は出ません。

設定したいときは、`tako setup` の最後に起動する対話アシスタントへ「別の PC でも同じ設定を使いたい」と頼めば、リポジトリの用意から配線まで代行してもらえます。

- 既に `~/.claude` を dotfiles リポジトリで管理している場合は、二重管理にならないよう**そのリポジトリへの相乗り**（`tako config link <リポジトリ>`）を先に提案します。別のリポジトリを新しく作ると、同じ `CLAUDE.md` が 2 か所で管理され、`tako config pull` の書き込みが symlink を実ファイルへ置き換えて既存の配線を壊すことがあります
- 何も無い場合は、`gh` にログイン済みならプライベートリポジトリの作成から配線まで代行できます。**リポジトリの作成と push は必ず同意を取ってから**行われ、`tako setup --yes` や非対話実行では一切起こりません

## 診断・保守

### tako platform

この環境でどの機能が使えるか / 縮退しているか / 未実装かを表示します。GUI 不要のローカル処理です。

```bash
tako platform                       # 現在の環境の対応状況
tako platform --platform windows    # Windows 側の状況を macOS から確認
tako platform --status pending      # 未実装のものだけ
tako platform --json
```

### その他

```bash
tako fda status              # フルディスクアクセスの付与状況
tako fda open                # システム設定を開く

tako sleep-guard status
tako sleep-guard set --mode while-agents-running --power-condition ac-only

tako telemetry status        # エラーレポート自動送信（既定 OFF）
tako telemetry on
tako telemetry off

tako stale-binary            # 稼働中セッションの claude バイナリの鮮度確認・張り直し
```

## MCP

### tako mcp serve

MCP の stdio ブリッジ（AI エージェントと tako をつなぐ中継役）として動作します。**通常は手で実行するものではなく**、`tako setup-mcp` で登録するとエージェント側が自動的に起動します。tako の外で起動された場合は安全のためツールを公開しません（0 ツールで応答）。

```bash
tako mcp serve
```
