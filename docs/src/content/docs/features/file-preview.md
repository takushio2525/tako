---
title: ファイルツリー＆プレビュー
seoTitle: ファイルツリーとプレビュー — コード・Markdown・画像・PDF をペインで開く
description: サイドバーのファイルツリーと、シンタックスハイライト付きのコード・Markdown・画像・PDF・動画のプレビュー。ペインの中での編集と自動反映、ファイルの実行（Code Runner）まで扱います
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

ファイルやフォルダを**ツリーの中のフォルダ**へドラッグすると、そのフォルダへ移動します（ファイルの行の上で離すと、そのファイルのあるフォルダへ）。

- 落とし先のフォルダは枠で囲まれ、「ここへ移動」と表示されます
- 移せない場所では赤い枠と理由が出ます（同じ名前がある / フォルダを自分の中へ / 別のボリューム / リモートの項目 / ワークスペースのフォルダの見出し）。同じ名前のファイルを上書きすることはありません
- 移したファイルを開いているペインは、新しい場所へそのまま付いていきます。保存していない変更も残ります

同じ操作はコマンドからもできます: `tako file move <移すもの> <移動先のフォルダ>`

### コピー・切り取り・貼り付け

ツリーの行を押して選び、Finder やエクスプローラーと同じキーで扱えます（右クリックメニューの「切り取り」「コピー」「貼り付け」も同じです）。

| 操作 | macOS | Windows |
|---|---|---|
| コピー | ⌘C | Ctrl+C |
| 切り取り | ⌘X | Ctrl+X |
| 貼り付け | ⌘V | Ctrl+V |

- **貼り付け先**: フォルダの行ならその中、ファイルの行ならそのファイルのあるフォルダ。コピーしたもの自身の行で貼ると、同じフォルダに複製します
- **同じ名前は上書きしません**。Finder と同じく「名前 のコピー」「名前 のコピー 2」…の名前で置きます（Windows はエクスプローラーと同じ「名前 - コピー」「名前 - コピー (2)」）
- フォルダは中身ごと写ります（権限もそのまま。シンボリックリンクはリンクのまま）。読めないファイルがあるなど途中で失敗したときは、途中まで写したものを残しません
- **切り取り → 貼り付けは移動**です。切り取り中の行は薄く表示され、移したファイルを開いているペインは新しい場所へ付いていきます
- **Finder / エクスプローラーと行き来できます**。tako でコピーしたファイルを Finder へ貼れ、Finder でコピーしたファイルをツリーへ貼れます
- 選んだ行の枠は、ペインを押す・ほかのキーを打つ・Esc で外れます（そのあとの ⌘C / ⌘V はターミナルのコピー・貼り付けに戻ります）
- 自分の中へのコピー・リモート（SSH）の項目などはできません。理由はツリーの上に出ます

同じ操作はコマンドからもできます:

```bash
tako file copy notes.md docs        # 複製（コピー → 貼り付けと同じ）
tako file clipboard copy notes.md   # コピー（Finder へも貼れる）
tako file clipboard cut notes.md    # 切り取り
tako file paste docs                # 貼り付け
tako file clipboard                 # いまの中身と貼り付け先を見る
```

### 複数選択・移動として貼る・コピーの進み具合

Finder や VS Code と同じく、ツリーの行は複数選べます。選んだものはまとめてコピー・切り取り・ゴミ箱へ移動・ドラッグできます。

| 操作 | macOS | Windows |
|---|---|---|
| 行を足す / 外す | ⌘+クリック | Ctrl+クリック |
| 範囲を選ぶ | ⇧+クリック | Shift+クリック |
| 選んだ行を 1 行ずつ動かす | ↑ / ↓ | ↑ / ↓ |
| フォルダを畳む・親へ / 開く・中へ | ← / → | ← / → |
| 開く（クリックと同じ） | Enter | Enter |
| 範囲を 1 行ずつ伸ばす / 縮める | ⇧↑ / ⇧↓ | Shift+↑ / Shift+↓ |
| 範囲を先頭 / 末尾まで伸ばす | ⇧⌘↑ / ⇧⌘↓ | Shift+Ctrl+Home / Shift+Ctrl+End |
| 選んだものをゴミ箱へ | ⌘⌫ | Delete |
| 移動として貼る | ⌥⌘V | Ctrl+Alt+V |

- 修飾キーを押しながらのクリックは**選ぶだけ**です（ファイルは開かず、フォルダも開閉しません）
- 選んだ行の上で右クリックすると、「切り取り」「コピー」「削除（ごみ箱）」が選んだもの全部に効きます（件数が添えられます）
- 選んだ行の 1 つを掴んでフォルダへ落とすと、選んだもの全部が移ります
- フォルダとその中のファイルを同時に選んだときは、フォルダごと扱います（中のファイルを 2 回写したりしません）
- **移動として貼る**（⌥⌘V / 右クリックの「項目をここに移動」）は、コピーしたものを Finder の「項目をここに移動」と同じく移します。同じ名前がある場所・自分の中へは移しません
- ↑ / ↓ は選んだ行を 1 行ずつ動かします（ファイルは開きません。端では止まります）。← は開いたフォルダを畳み、ファイルや畳んだフォルダでは親のフォルダへ上がります。→ は畳んだフォルダを開き、開いたフォルダでは最初の中身へ下ります。Enter はクリックと同じです（ファイルを開く・フォルダを開閉）
- ⇧↑ / ⇧↓ は最後に選んだ行から 1 行ずつ範囲を伸ばします（逆へ押すと縮みます）。⇧⌘↑ / ⇧⌘↓（Windows は Shift+Ctrl+Home / End）は先頭 / 末尾まで一度に伸ばします。⌘⌫（Windows は Delete）は選んだもの全部をゴミ箱へ入れます。どれも行を選んでいる間だけ効き、選んでいなければターミナルへ届きます。ワークスペースのフォルダ（見出し）はキーではゴミ箱へ入れません。Windows の Shift+Delete（ゴミ箱を通らない削除）はツリーでは扱わず、ターミナルへ届きます
- 大きなフォルダをコピーすると、ツリーの上に**件数とバイトの進み具合**が出ます。「取り消し」を押すと、写しかけのものを残さずに止まります（写し終えたものは残ります）。巨大な 1 つのファイルでも途中で止まり、写し始めて数秒経つと**残り時間の目安**も出ます。目安は一定の速さなら行き来せずに減っていき、コピーが止まるとその分だけ延びます
- 同じボリュームの中のコピーは、Finder と同じく OS の複製（APFS の clone）で一瞬で終わります

同じ操作はコマンドからもできます:

```bash
tako file clipboard copy a.md b.md   # まとめてコピー
tako file move a.md b.md docs        # まとめて移す（最後が移動先）
tako file trash a.md b.md            # まとめてゴミ箱へ
tako file paste --move docs          # 移動として貼る
tako file copy a.md b.md docs        # まとめて複製（1 つのコピーとして進み具合・取り消しが効く）
tako file progress                   # 走っているコピーの進み具合（eta_secs = 残り時間の目安）
tako file cancel                     # 走っているコピーを取り消す
tako tree selection                  # ツリーで選んでいる行を見る
tako tree selection --key down       # ↓ を押したのと同じ（up / left / right / enter / extend_* も）
```

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
| 組み込み | Arduino のスケッチ (.ino。C++ として色分け) |
| Web / スクリプト | JavaScript (.js/.jsx/.mjs), TypeScript (.ts/.tsx), Python, Ruby, PHP, Lua, Perl, HTML, CSS |
| シェル | Bash (.sh/.bash/.zsh), Fish |
| データ形式 | JSON, TOML, YAML, XML, INI, CSV, DotENV (.env) |
| ドキュメント | Markdown, LaTeX, reStructuredText |
| ビルド / 設定 | Dockerfile, Makefile, CMake, SQL, Diff/Patch |
| その他 | Git Ignore, Git Attributes, AppleScript, R, Clojure, Erlang, Groovy, nginx.conf 等 |

拡張子に加え、ファイル名でも判定します（例: `Cargo.lock` → TOML, `Dockerfile` → Dockerfile, `CMakeLists.txt` → CMake, `.gitignore` → Git Ignore）。shebang（`#!/bin/bash` 等）による自動検出にも対応しています。Markdown のコードブロックも同じ構文で塗り、```` ```ino ```` / ```` ```arduino ```` は C++ として扱います。

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
- **PDF**: ペイン内でそのまま表示。テキストの選択・コピー、目次からのジャンプ、内部リンクや外部 URL の <kbd>Cmd</kbd>+クリック（Windows は <kbd>Ctrl</kbd>+クリック）にも対応します
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

コードプレビューはその場で軽く編集できます。<kbd>Cmd</kbd>+<kbd>S</kbd> で保存、<kbd>Cmd</kbd>+<kbd>F</kbd> で検索、<kbd>Cmd</kbd>+<kbd>Z</kbd> で取り消しです。検索は大文字小文字を区別するのが既定で、検索欄の右のトグルで「区別しない」「単語単位」に切り替えられます（`value` を置換しても型名の `Value` は変わりません）。開いている間にファイルが外部から書き換わった場合、保存は拒否されるので、うっかり他人の変更を潰すことはありません。

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

ファイルがプロジェクトの中にあるときは、そのプロジェクトの流儀で走ります。ファイルのあるフォルダから上へ辿って `Cargo.toml` / `package.json` / `pyproject.toml` / `go.mod` / `*.csproj` / `Makefile` を探し、見つかったフォルダ（プロジェクトのルート）で実行します。

| プロジェクト | 実行されるコマンドの例 |
|---|---|
| Rust（`Cargo.toml`） | `cargo run`（ワークスペースの中なら `cargo run -p <パッケージ>`。bin の無いライブラリはそのモジュールのテスト `cargo test -p <パッケージ> --lib <モジュール>::`） |
| Node.js（`package.json`） | そのファイルを走らせている script があれば `npm run <script>`（pnpm / yarn / bun はロックファイルで判別）、無ければ `node <ファイル>` |
| Python（`pyproject.toml`） | パッケージの中なら `python3 -m <モジュール>`、テストファイルなら `python3 -m pytest <ファイル>`（Python は下の「Python の実行環境」の順で自動で選ぶ） |
| Go（`go.mod`） | `go run ./<パッケージ>`（main でないパッケージとテストファイルは `go test`） |
| .NET（`*.csproj` 等） | `dotnet run`（テストプロジェクトは `dotnet test`） |
| C / C++（`Makefile`） | `run` 規則があれば `make run`、無ければ `make` |

探索は git リポジトリのルートより上へは出ず、ホームフォルダ（`~`）直下にある `package.json` などの置き忘れもプロジェクトとはみなしません。ファイルに `tako:run` を書けばそちらが優先され、`tako run-default` で拡張子の既定を設定した場合もそちらが優先されます。`${workspaceRoot}` と書くとプロジェクトのルートに置き換わります（例: `tako:cwd: ${workspaceRoot}`）。

```bash
tako run script.py
tako run script.py --list        # 使えるプロファイル（どのプロジェクトとして解決したかも出る）
tako run-default py "python3"    # 拡張子の既定を設定
```

### Python の実行環境（.venv / uv / poetry など）

`.py` は、設定しなくてもプロジェクトの実行環境で走ります。ファイルのあるフォルダから上へ辿り、次の順で最初に見つかったものを使います。

1. uv のプロジェクト（`uv.lock` か `pyproject.toml` の `[tool.uv]`）→ `uv run python`
2. プロジェクトの中の venv（`.venv` / `venv` / `pyvenv.cfg` を持つフォルダ。近いフォルダのものが優先）
3. Poetry（`poetry.lock`）→ `poetry run python`
4. Pipenv（`Pipfile`）→ `pipenv run python`
5. conda（`environment.yml` の `name:` と一致する env がちょうど 1 つあるとき）
6. pyenv（`.python-version`）
7. どれも無ければ、これまでどおり `python3`（Windows は `python`）

venv や conda の env を使うときは、その env の `bin`（Windows は `Scripts`）を PATH の先頭へ足し、`VIRTUAL_ENV` なども設定してから走らせます。`tako:run: pytest` のように python を直接書かない宣言でも、その環境の `pytest` が使われます。宣言の中で `${python}` と書くと、選ばれた Python に置き換わります（例: `tako:run: ${python} -m pytest`）。

どれが選ばれたかと、選べる候補の一覧は `--list` で確認できます。python の実体が消えた壊れた `.venv` は選ばず、理由を `warnings` に出します。

```bash
tako run script.py --list            # runtime（選ばれた環境）と runtimes（候補）
tako run script.py --list --refresh  # uv / poetry の場所や Python の版を調べ直す
```

conda の env は PATH と `CONDA_PREFIX` を設定するだけで、`activate.d` のスクリプトは走りません。必要なら `tako:run: conda run -n <名前> python ${fileBase}` と書いてください。環境を固定したいときも同じように `tako:run:` で直接書けます。

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
