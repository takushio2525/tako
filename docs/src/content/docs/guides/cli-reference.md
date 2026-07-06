---
title: CLI リファレンス
description: tako コマンド全一覧 — 各コマンドの目的・使い方・実行例・よく使うオプション
---

`tako` CLI は、ターミナルの画面操作（ペイン分割・テキスト送信・レイアウト変更など）をコマンドとして実行するためのツールです。シェルスクリプトからの自動化にも、AI エージェントからの操作にも使われます。

## 共通の前提

- **tako の中のターミナルで実行する**のが基本です。tako の外（通常のターミナル）で実行すると、接続情報が見つからない旨のエラーになります（`tako setup` / `tako setup-mcp` / `tako remote` 系など一部はアプリ未起動でも動作します）
- **ペイン ID の自動特定**: tako のペイン内から実行すると、環境変数 `TAKO_PANE_ID` から「自分がいるペイン」が自動で分かります。`--pane` を省略したときの対象は呼び出し元ペインです
- **ペイン ID の調べ方**: `tako list` で全ペインの ID・タイトル・作業ディレクトリが JSON で確認できます

```bash
# ヘルプはすべてのコマンドで使える
tako --help
tako split --help
```

## セットアップ

### tako setup

AI 連携に必要な設定を対話形式でまとめて行います。claude コマンドの確認 → MCP 自動登録 → Claude Code が対話でガイド、という流れです。tako アプリが起動していなくても実行できます。詳しくは[セットアップガイド](/getting-started/#3-tako-setup--対話式セットアップai-連携する場合)を参照してください。

```bash
# 対話式セットアップを開始
tako setup

# 環境チェックだけ実行（claude の有無・MCP 登録・セットアップ状態を表示）
tako setup --check

# アップデート追従状況を表示（前回セットアップ以降に setup へ入った変更の一覧）
tako setup --changes

# 同じ内容を JSON で出力（MCP ツール tako_setup_changes と同一ペイロード）
tako setup --changes --json

# セットアップ状態をリセットして最初からやり直す
tako setup --reset
```

tako のアップデートでセットアップ項目・設定フォーマット・master 用システムプロンプトが変わることがあります。`--changes` で未適用の変更を確認でき、`tako setup` を再実行すると対話の中で最新状態へ追従できます（詳細は[セットアップガイド](/getting-started/#アップデート後の追従-tako-setup-の再実行)）。

### tako setup-mcp

Claude Code の設定ファイル（`~/.claude/settings.json`）に tako の MCP サーバーを登録します。対話なしで登録だけしたいときに使います。

```bash
# ユーザー全体に登録（既定。どのプロジェクトでも有効）
tako setup-mcp

# 現在のディレクトリだけに登録（.claude/settings.json に書き込む）
tako setup-mcp --project
```

### tako update

アプリのアップデートを確認・実行します。Homebrew / ZIP のどちらでインストールしたかを自動判別します。

```bash
# 配布系統・現在バージョン・PATH 上の重複 CLI を診断表示
tako update status

# 新しいバージョンがあるか確認（更新はしない）
tako update check

# 更新を実行（レイアウト保存 → 更新 → 自動再起動）
tako update apply

# brew での更新が失敗して詰まったときの復旧
tako update repair      # Homebrew の管理情報を修復
tako update apply-zip   # zip 経由で強制更新
```

## 基本操作

### tako split

ペインを分割して新しいペインを作ります。何のためのコマンドか: 「隣に画面をもう 1 枚出す」操作です。新しいペインの ID が出力されるので、続けて `send` や `read` の対象にできます。

```bash
# 右に分割（既定）
tako split --right

# 下に分割
tako split --down

# 分割してコマンドを実行（-- 以降がそのまま新ペインで実行される）
tako split --right -- npm run dev

# 新ペインの取り分を 30% にして htop を起動
tako split --right --ratio 0.3 -- htop

# 作業ディレクトリを指定して分割し、新ペインにフォーカスを移す
tako split --down --cwd ~/Documents/webapp --focus
```

| オプション | 説明 |
|---|---|
| `--right` / `--down` / `--up` / `--left` | 分割方向（省略時は右） |
| `--ratio <0.0–1.0>` | 新ペイン側の取り分（省略時は等分） |
| `--cwd <パス>` | 新ペインの作業ディレクトリ |
| `--focus` | 新ペインにフォーカスを移す（省略時は元のペインのまま） |
| `--pane <ID>` / `--tab <ID>` | 分割元の指定（省略時は呼び出し元ペイン） |
| `-- <コマンド>` | シェルの代わりに実行するコマンド |

### tako send

ペインにテキストを送信します。「隣のペインでコマンドを打つ」操作に相当します。既定で末尾に改行（Enter）が付くので、コマンドはそのまま実行されます。

```bash
# ペイン 3 で echo を実行
tako send --pane 3 "echo hello"

# 改行を付けずに送る（入力途中の状態にしたいとき）
tako send --pane 3 --no-newline "yes"

# claude（AI）のペインへ指示を送る。プロンプト表示を待ってから確実に届ける
tako send --pane 3 --await-prompt "テストを実行して結果を教えて"
```

| オプション | 説明 |
|---|---|
| `--pane <ID>` | 送信先ペイン（省略時は呼び出し元） |
| `--no-newline` | 末尾に改行を付けない |
| `--await-prompt` | claude の入力欄が表示されるのを待ってから送信（送達確認付き） |

:::note
Claude Code のような全画面アプリへの送信は、貼り付け → Enter 送信 → 届いたかの検証、という確認ループ付きで配送されます。長い指示文が入力欄に残ったままになる心配はありません。
:::

### tako read

ペインの画面内容をテキストとして取得します。「隣のペインに何が表示されているか見る」操作です。

```bash
# ペイン 3 の画面を丸ごと取得
tako read --pane 3

# 末尾 50 行だけ取得
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
# ID 指定
tako focus 3

# 方向指定（今いるペインの右隣へ）
tako focus --right
```

### tako scroll

ペインのスクロール位置を動かします。過去の出力を遡って見たいときに使います。

```bash
# 100 行分過去へスクロール
tako scroll --pane 3 --delta 100

# 最下部（最新）に戻る
tako scroll --pane 3 --to 0
```

### tako close

ペインを閉じます（中のプロセスも終了します）。タブ最後の 1 ペインを閉じるとタブごと閉じます。

```bash
# ペイン 3 を閉じる
tako close --pane 3

# 実行中の worker でも強制的に閉じる
tako close --pane 3 --force
```

:::caution
`close` はプロセスを終了します。「画面から消したいが処理は続けたい」場合は [`tako background`](#tako-background--foreground--backgrounded) を使ってください。
:::

### tako title

ペインの表示タイトルと役割ラベルを設定します。どのペインが何をしているかを分かりやすくします。

```bash
# タイトルを設定
tako title --pane 3 "dev server"

# 役割ラベル付き（AI が worker を識別するのに使う）
tako title --pane 3 --role worker-1 "修復係"

# 空文字でクリア（自動リネームに戻る）
tako title --pane 3 ""
```

## レイアウト操作

### tako resize

ペインの取り分（画面に占める割合）を調整します。

```bash
# 横方向に 10% 広げる
tako resize --pane 3 --dx 0.1

# 縦方向に 10% 縮める
tako resize --pane 3 --dy -0.1

# 横の取り分を 60% ぴったりに
tako resize --pane 3 --share-x 0.6
```

| オプション | 説明 |
|---|---|
| `--dx` / `--dy` | 相対変更（正 = 広げる、負 = 縮める） |
| `--share-x` / `--share-y` | 絶対指定（0.0〜1.0） |

### tako equalize

タブ内の全ペインを均等サイズに整えます。散らかったレイアウトのリセットに便利です。

```bash
tako equalize

# タブを指定
tako equalize --tab 2
```

## タブ操作

### tako tab

```bash
# 新しいタブを作成（タブ ID と初期ペイン ID が JSON で返る）
tako tab new
tako tab new --title "API Server"

# タブ名を変更（--tab 省略時は今いるタブ）
tako tab rename --tab 2 "フロントエンド"

# 空文字にすると手動指定を解除（AI 自動リネームに戻る）
tako tab rename ""

# タブを切り替え
tako tab select 2

# ペインを別タブの末尾へ移動
tako tab move-pane 2 --pane 5

# 同タブ内でペインを並べ替え（ペイン 5 をペイン 3 の下へ）
tako tab move-pane --pane 5 --target 3 --down
```

### tako collapse

サイドバーの tmux ビューで、タブ枠を折りたたみ / 展開します（バックグラウンド行を隠します）。

```bash
tako collapse --tab 2 on    # 折りたたむ
tako collapse --tab 2 off   # 展開
tako collapse --tab 2       # トグル
```

## ファイル・プレビュー

### tako open

ファイルをプレビューペインで開きます。コードはシンタックスハイライト付き、`.md` はレンダリング表示、画像・PDF・動画にも対応します。

```bash
# ファイルを開く（拡張子から表示モードを自動判定）
tako open src/main.rs

# Markdown をソース表示で開く
tako open README.md --mode code

# 既存プレビューを再利用せず、右に分割して開く
tako open src/app.tsx --right
```

| オプション | 説明 |
|---|---|
| `--mode <code\|markdown\|image\|pdf\|video>` | 表示モードの明示指定 |
| `--right` / `--down` / `--up` / `--left` | 分割して新しいプレビューペインで開く |
| `--pane <ID>` | 基準ペイン（相対パスの解決とプレビュー表示先に使う） |

### tako file

ファイルツリーの右クリックメニューに相当する操作群です。

```bash
# 絶対パスを出力（--relative でペイン cwd 基準の相対パス）
tako file copy-path src/main.rs

# Finder でファイルの場所を表示
tako file reveal src/main.rs

# 指定ディレクトリへペイン内で cd する
tako file open-terminal ~/Documents/webapp

# 名前を変更
tako file rename old.txt new.txt

# 新しいファイル / フォルダを作成（path 配下に name で作成）
tako file create src helper.ts
tako file mkdir src components

# ゴミ箱へ移動
tako file trash old-notes.md
```

### tako video

プレビューペインで動画を開いているときの再生操作です。

```bash
tako video play --pane 4     # 再生
tako video pause --pane 4    # 一時停止
tako video toggle --pane 4   # 再生 / 一時停止の切替
tako video seek 90 --pane 4  # 90 秒地点へシーク
```

### tako chrome

URL を Chrome ベースの Web ビューペインとして開きます（実験的機能）。

```bash
tako chrome open http://localhost:5173 --right
```

## バックグラウンド退避（たまり場）

### tako background / foreground / backgrounded

「処理は動かしたまま、画面からだけ消す」操作です。詳しくは[たまり場](/features/shelving/)を参照してください。

```bash
# ペイン 3 をバックグラウンドへ退避（プロセスは生きたまま）
tako background --pane 3

# 退避中ペインの一覧を JSON で表示
tako backgrounded

# ペイン 3 を画面に復帰させる（省略時は元いたタブへ戻る）
tako foreground 3

# 復帰先を指定（ペイン 5 の下に挿入）
tako foreground 3 --target 5 --direction down
```

## git

現在の作業ディレクトリの git 情報を JSON で取得します。

```bash
# コミット履歴・ブランチ・変更状態（--max-count で件数制限、既定 200）
tako git log

# 差分。既定は未ステージ、--target で staged やコミットハッシュも指定可
tako git diff
tako git diff --target staged
tako git diff --target a1b2c3d
```

## tmux 管理

tako は tmux セッションの「見える化と片付け」もできます。詳しくは [tmux バックエンド](/features/tmux-backend/)を参照してください。

```bash
# 全 tmux セッションを一覧（tako のペインとの対応付き）
tako tmux list

# 取り残された不要セッションの一括掃除（使用中のものには触れない）
tako tmux cleanup

# セッションを終了（確認なしで即実行。対象は list で確認してから）
tako tmux kill --session my-session

# セッション内の特定 window だけ終了
tako tmux kill --session my-session --window 1

# 外部の tmux セッションを現在のタブに取り込んで表示
tako tmux open my-session --right

# バックエンドセッションのアクティブ window を切り替え
tako tmux select-window 1 --pane 3

# window を指定サイズにリサイズ（スマホリモートのビューポート連動用）
tako tmux resize --session my-session --cols 80 --rows 24
tako tmux resize --session my-session --reset   # 元に戻す
```

## 表示・設定のトグル

```bash
# 右サイドバーの情報パネル（引数なしで現在状態を表示）
tako panel --show --view tmux    # tmux ビューを表示
tako panel --view git            # git ビューに切替
tako panel --hide                # 隠す
tako panel --filetree on         # 左のファイルツリーを表示
tako panel --width 360           # パネル幅を変更

# プレビューのピン留め（フローティングウィンドウ化）
tako pin --pane 3 on
tako pin --pane 3 off

# セッション永続化（tmux バックエンド）の ON/OFF・状態確認
tako persist        # 現在状態を表示
tako persist on
tako persist off

# ポート検知（「プレビューを開く？」チップ）の ON/OFF
tako portdetect on
tako portdetect off

# タブ・ペイン名の AI 自動リネームの ON/OFF
tako autorename on
tako autorename off
```

## リモートアクセス

スマホなど外部デバイスから tako の画面を見る・操作するための API サーバーです。

```bash
# サーバーを起動し、接続用 QR コードを表示
tako remote start

# トンネルなし（同一 Wi-Fi 内のみ）で起動
tako remote start --no-tunnel

# 状態確認・停止
tako remote status
tako remote stop

# 動作中の AI エージェント一覧
tako remote agents

# エージェントの会話ログ末尾を表示（session ID は agents で確認）
tako remote messages <session-id> --tail 30

# ペインのスクロールバック履歴をテキストで表示
tako remote scrollback <pane-id> --lines 1000
```

:::note[リレーの仕組みと差し替え]
トンネル URL の再解決（2 回目以降の QR 再スキャン不要化）には、作者がベストエフォートで運用する公共リレー（Cloudflare Workers・SLA なし）を使います。リレーに保存されるのは machineId とトンネル URL のみで、**画面内容やトークンは通りません**（それらはトンネル経由で tako 本体と直接やり取りされ、トークン認証で保護されます）。リレーへの登録は端末ごとに自動生成されるシークレットで first-write-wins 保護されます。`TAKO_RELAY_URL` 環境変数で自前のリレーに差し替えることもできます（構築手順はリポジトリの `web/tako-remote-worker/README.md`）。
:::

## オーケストレーター

複数の AI エージェントを親子で連携させる機能です。考え方は[オーケストレーションとは](/features/orchestration/)、使い方は [tako master 実践ガイド](/features/orchestrator/)を参照してください。

このうち、**日常であなたが打つのは `tako master` だけ**です。それ以外の `tako orchestrator` 系コマンドは、通常は master（AI）自身が内部で実行するもので、手動操作やスクリプトからの自動化用に公開されています。

### tako master

マスター（司令塔となる claude）を新しいタブで起動します。以後はこのマスターに自然言語で作業を依頼するだけで、子 worker の起動・監視・回収が自動で回ります。プロジェクトの登録やモデル設定も master に頼めます。

```bash
# default プロファイルで起動
tako master

# プロファイルを指定して起動（profiles/<名前>.yaml の設定を使う）
tako master -fast

# 旧形式のサフィックス指定も動作する（"master-dev" タブになる）
tako master dev
```

### tako orchestrator projects

マスターが作業対象にできるプロジェクトの登録・管理です。**通常は master に「◯◯のリポジトリを追加して」と頼むだけで済みます**。手動やスクリプトから登録したいとき用のコマンドで、登録内容は `~/Library/Application Support/tako/orchestrator/projects.yaml` に保存されます。

```bash
# 一覧
tako orchestrator projects list

# 追加（key は呼び名、cwd は作業ディレクトリ）
tako orchestrator projects add --key webapp --cwd ~/Documents/webapp --description "Web アプリ"

# 削除
tako orchestrator projects remove --key webapp
```

### tako orchestrator profiles

マスター・worker が使うモデルや思考量（effort）の設定です。**通常は `tako setup` の対話や master への依頼で変更すれば済みます**。コマンドで直接変更したいとき用です。

```bash
# プロファイル一覧（model: null は「claude の既定モデルで起動」の意味）
tako orchestrator profiles list

# 内容と解決結果を表示（名前省略時は default）
tako orchestrator profiles show

# モデルと effort を設定
tako orchestrator profiles set default --model claude-opus-4-6 --effort max

# モデル指定を解除して claude 既定に戻す（全プランで動作する推奨状態）
tako orchestrator profiles set default --clear-model
```

| `set` の主なオプション | 説明 |
|---|---|
| `--model` / `--clear-model` | master のモデル指定 / 解除 |
| `--effort` | master の思考量（thinking effort） |
| `--worker-model` / `--clear-worker-model` | 子 worker 用の固定モデル / 解除 |
| `--worker-effort` | 子 worker の思考量 |

:::caution
`[1m]` 付きモデル（1M コンテキスト版）は Max / API プラン限定です。Pro プランで指定すると master が起動できません。
:::

### tako orchestrator spawn

子 worker（作業担当の claude）をペインに起動し、プロンプトを渡します。通常はマスターが自動で実行しますが、手動でも使えます。

```bash
tako orchestrator spawn --project webapp --prompt "ログインページを実装して" --label login
```

| オプション | 必須 | 説明 |
|---|---|---|
| `--project` | ○ | プロジェクトキー（projects に登録済みのもの） |
| `--prompt` | ○ | worker に渡す初期プロンプト |
| `--label` | | ペインタイトルに付けるラベル |
| `--model` / `--effort` | | worker のモデル・思考量（省略時はプロファイル設定に従う） |
| `--pane` / `--tab` | | worker ペインをどこに出すか（分割元の指定） |

### tako orchestrator status / watch

worker の状態確認と完了待ちです。

```bash
# 状態を 1 回確認
tako orchestrator status --pane 5

# 完了まで待ち続け、結果を 1 行出力（WORKER_IDLE = 完了 / WORKER_GONE = 消滅）
tako orchestrator watch --pane 5 --session-id <S>

# タイムアウト付き
tako orchestrator watch --pane 5 --timeout 600
```

### tako orchestrator run

spawn → 完了待ち → 出力回収 → ペインの片付け、を 1 コマンドで行うワンショット実行です。

```bash
tako orchestrator run --project webapp --prompt "テストを実行して失敗があれば直して"
```

| オプション | 説明 |
|---|---|
| `--timeout <秒>` | 完了待ちの上限（既定 1800 秒） |
| `--auto-close <true\|false>` | 完了後にペインを自動で閉じるか（既定 true） |
| `--output-lines <N>` | 回収する出力の末尾行数（既定 200） |

## MCP

### tako mcp serve

MCP の stdio ブリッジ（Claude Code と tako をつなぐ中継役）として動作します。**通常は手で実行するものではなく**、`tako setup-mcp` で登録すると Claude Code が自動的に起動します。tako の外で起動された場合は安全のためツールを公開しません（0 ツールで応答）。

```bash
tako mcp serve
```
