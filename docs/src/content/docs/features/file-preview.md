---
title: ファイルツリー＆プレビュー
description: サイドバーのファイルツリーとシンタックスハイライト付きプレビュー
---

tako はターミナルでありながら、エディタのようなファイルブラウジング機能を備えています。

## ファイルツリー（左サイドバー）

左サイドバーにファイルツリーを表示できます。

- <kbd>Cmd</kbd>+<kbd>B</kbd> またはステータスバーのトグルボタンで表示/非表示
- タブ内の全ペインの作業ディレクトリを**ワークスペースフォルダ**として自動検出・表示
- ファイルをクリックするとプレビューペインで開く
- AI が `tako tree add` で作業対象のフォルダを明示的に追加することもできます

`.git` や `.env` のようなドット始まりの項目は、既定では隠れています。見出しの目アイコン、右クリック、設定画面の「外観」、または次のコマンドで切り替えられます。

```bash
tako panel --show-hidden on
tako panel --show-hidden off
```

### git の状態（色とバッジ）

git リポジトリの中では、変更のあるファイルが**色とバッジ**で分かります。VSCode / Zed と同じ考え方で、コミット前の取りこぼしをツリーを見るだけで拾えます。

| 見え方 | 意味 |
|---|---|
| 黄色 + `M` | 変更あり |
| 緑 + `U` | 新規（未追跡） |
| 緑 + `A` | 新規をステージ済み |
| 赤 + `D` | 削除 |
| 紫 + `R` | リネーム |
| 赤 + `!` | コンフリクト（未解決） |
| 薄いグレー | `.gitignore` 対象（バッジなし） |

バッジは git 自身の `git status --short` と同じ 2 桁の読み方です。**左がステージ済み（緑）・右が未ステージ**なので、`MM` は「一度ステージしたあとにさらに書き換えた」ことを表します。

**フォルダには配下の変更件数**が出ます。折りたたんだままでも「このフォルダの中に何件ある」が見えるので、ワークスペースフォルダの見出し行を見れば、そのプロジェクトに未コミットが残っているかがひと目で分かります。

git 管理外のフォルダでは何も表示されません（色も従来のままです）。

AI からは同じ表を次のコマンドで読めます。

```bash
tako tree git-status                     # タブのワークスペースフォルダ全部
tako tree git-status ~/Documents/webapp  # フォルダを 1 つに絞る
```

### コンテキストメニュー（右クリック）

ファイルやフォルダを右クリックするとメニューが表示されます。

- **パスをコピー** — ファイルパスをクリップボードにコピー
- **Finder で表示** — Finder でファイルの場所を開く
- **ここで cd** — アクティブペインのカレントディレクトリを変更
- **名前を変更** — インライン入力でリネーム
- **新しいファイル / フォルダ** — その場で新規作成
- **ゴミ箱に入れる** — ファイルを macOS のゴミ箱に移動

### ドラッグ＆ドロップ

ファイルツリーからペインエリアへドラッグすると:

- **ターミナルペイン**にドロップ → ファイルパスをテキストとして入力
- **プレビューペイン**にドロップ → そのファイルをプレビュー表示

<figure class="tako-shot">
<img src="/img/preview-code.webp" alt="左のファイルツリーでファイルを選び、右のペインに TypeScript がシンタックスハイライト付きで表示されている画面" />
<figcaption>ファイルツリーから選ぶと、隣のペインにシンタックスハイライト付きで開く</figcaption>
</figure>

## コードプレビュー

ファイルをクリックまたは `tako open <ファイルパス>` で、ペイン内にファイル内容を表示します。

- **シンタックスハイライト**: 210+ の言語・形式に対応（bat 由来の拡張構文セット）
- **行番号表示**: 行番号付きのコードビュー
- **折り返し**: 長い行は自動折り返し（横スクロール不要）

### 対応形式（主要なもの）

| カテゴリ | 対応形式 |
|---|---|
| システム言語 | Rust, C, C++, Go, Swift, Kotlin, Java, C#, Objective-C, Scala, Haskell, D |
| Web / スクリプト | JavaScript (.js/.jsx/.mjs), TypeScript (.ts/.tsx), Python, Ruby, PHP, Lua, Perl, HTML, CSS |
| シェル | Bash (.sh/.bash/.zsh), Fish |
| データ形式 | JSON, TOML, YAML, XML, INI, CSV, DotENV (.env) |
| ドキュメント | Markdown, LaTeX, reStructuredText |
| ビルド / 設定 | Dockerfile, Makefile, CMake, SQL, Diff/Patch |
| その他 | Git Ignore, Git Attributes, AppleScript, R, Clojure, Erlang, Groovy, nginx.conf 等 |

拡張子に加え、ファイル名でも判定します（例: `Cargo.lock` → TOML, `Dockerfile` → Dockerfile, `CMakeLists.txt` → CMake, `.gitignore` → Git Ignore）。shebang（`#!/bin/bash` 等）による自動検出にも対応しています。

## Markdown プレビュー

`.md` ファイルはデフォルトで**レンダリング表示**されます。

- 見出し・リスト・テーブル・コードブロック・引用をビジュアル表示
- タイトルバーの目アイコンで **コード表示 ⇔ Markdown 表示**を切替可能
- 切替モードは CLI / MCP からも操作可

<figure class="tako-shot">
<img src="/img/preview-markdown.webp" alt="README.md が見出し・コードブロック・表つきでレンダリング表示されている画面" />
<figcaption>`.md` は既定でレンダリング表示。コードブロックにはコピーボタンが付く</figcaption>
</figure>

## 画像・PDF・動画プレビュー

コードと Markdown 以外のファイルも開けます。表示モードは拡張子から自動判定されます。

- **画像**: PNG / JPEG / SVG / GIF / WebP など。25〜400% のズームとパンに対応
- **PDF**: ペイン内でそのまま表示。テキストの選択・コピー、目次からのジャンプ、内部リンクや外部 URL の <kbd>Cmd</kbd>+クリックにも対応します
- **動画**: mp4 の再生・一時停止・シーク（矢印キーやクリックで操作。CLI `tako video` / MCP からも制御可）

```bash
tako preview                  # 現在のズーム・ページ状態
tako preview-outline          # Markdown 見出し / PDF 目次の一覧
tako preview-outline --item 4 # 4 番目の項目へジャンプ
```

<figure class="tako-shot">
<img src="/img/preview-markdown-table.webp" alt="Markdown の表が罫線つきの表として描画されている画面" />
<figcaption>GFM の表は罫線・ヘッダ帯つきで描画され、列幅も内容に合わせて配分される</figcaption>
</figure>

## 編集と自動反映

コードプレビューはその場で軽く編集できます。<kbd>Cmd</kbd>+<kbd>S</kbd> で保存、<kbd>Cmd</kbd>+<kbd>F</kbd> で検索、<kbd>Cmd</kbd>+<kbd>Z</kbd> で取り消しです。開いている間にファイルが外部から書き換わった場合、保存は拒否されるので、うっかり他人の変更を潰すことはありません。

表示中のファイルが変わると**自動で再読み込み**されます（既定 ON）。AI がファイルを書き換えたとき、手動で開き直す必要はありません。

```bash
tako preview-reload          # 現在値
tako preview-reload off
```

## 履歴（チェンジログ）ビュー

プレビューヘッダの「履歴」トグルで、**そのファイルの git 履歴**に表示を切り替えられます。コミット一覧が並び、選ぶとそのコミットでの差分が読めます。「この行はいつ、なぜ変わったのか」をターミナルから出ずに追えます。

```bash
tako preview-changelog
```

## ファイルを実行する（Code Runner）

プレビューヘッダの再生ボタンで、開いているファイルをその場で実行できます。実行はペインを分割して行われるので、出力を見ながら編集を続けられます。

実行コマンドは、ファイル先頭の `tako:run` 宣言か、拡張子ごとの既定から決まります。プロファイルが複数あるときは再生ボタン横のドロップダウンで選べます。

```bash
tako run script.py
tako run script.py --list        # 使えるプロファイル
tako run-default py "python3"    # 拡張子の既定を設定
```

## AI からの操作

```bash
# ファイルをプレビューで開く（拡張子から表示モードを自動判定）
tako open src/main.rs

# 表示モードを明示指定して開く
tako open design.pdf --mode pdf

# MCP 経由（AI エージェントが使用）
# tako_open_file ツールで AI が自動的にプレビューを開く
```

AI が「このファイルを見て」と言ったとき、tako は自動的にプレビューペインを開いて該当ファイルを表示します。エディタを別途開く必要はありません。
