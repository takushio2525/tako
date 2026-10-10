# IDE の対応形式の棚卸しと設計（Issue #1943 S0）

> エピック #1943 の S0。**製品コードは 1 行も変えていない**。表の各マスは main（`d851f0d`）の
> コードを file:line で示すか、本物の関数を呼んだ実挙動・実ツールの応答で裏付けた（§1）。
> 子 Issue の一覧と依存は §7（正本は #1943 のコメント）。

## 0. 結論（先に要点）

- **表の正本は 10 か所に分かれている**（§3）。言語を 1 つ「色・言語サーバ・実行・インデント・
  アイコンまで」揃えると、今は **tako-core 3〜4 か所 + tako-app 3 か所 + テスト 3 か所**を直す。
  拡張子の並びそのものが二重三重に書かれている（C / C++ 群は 4 か所、JS / TS 群は 4 か所、
  Python は 3 か所）。挙動の表（言語サーバ・実行・実行環境）は #1726 の設計 §9.2 のとおり
  **分けたまま**にし、**「拡張子 → 言語の同定」だけを正本 1 か所へ寄せる**（§4。子 Issue の先頭）
- **大きな穴**（45 形式中。§2）: 色なし 6（`.ino` `.pde` `.S` `.astro` `.json5` `.ps1`）・
  誤った構文 2（`.h` → Objective-C、`.m` → Objective-C）・言語サーバは 8 形式だけ・
  Finder の候補に出るのは 21 形式・Windows の関連付けは 0。**どの構文で塗ったかは
  CLI / MCP から読めない**（開発不変条件の穴）
- **`.ino` の方針**（§5。この PC の Arduino IDE 2.3.5 同梱物で実測）:
  - 色分け = **C++ の構文**（VS Code 同梱の cpp 言語も `.ino` を含む）。構文セットは増えない
  - 言語サーバ = **arduino-language-server**。clangd 単体は不可（db 無しなら 1 行目で拒否、
    db を当てても正しいスケッチに嘘のエラー 10 件）。ALS は**基板（FQBN）が無いと何も返さない**
  - 実行 = スケッチのフォルダで**引数なしの `arduino-cli compile`**。基板の正本は
    Arduino 公式の **`sketch.yaml` の `default_fqbn`**（tako に新しい保存先を作らない）
  - 無い環境 = 今の表示のまま（色だけ）。導入は `tako setup` の標準に載せず、`.ino` を
    開いたときだけ案内する。取得の仕組みは #1944 に載せる

## 1. 調べ方（再現の手順）

| 列 | 取り方 |
|---|---|
| プレビューの種別 | `tako_core::open_plan::preview_route`（本物）を呼んだ |
| 構文の色分け | `crates/tako-app/src/preview.rs:2025` `syntax_for_path` の解決順（ファイル名 → `:2059` の補い → 拡張子 → `:2077` の補い → 1 行目 → Plain Text）を、同じ版の syntect 5.3.0 / two-face 0.5.1・同じ feature で**写して**引いた（tako-app は GPUI を引くので作らない） |
| 言語サーバ | `tako_core::lsp::servers::resolve`（本物） |
| Code Runner | `tako_core::runner::resolve_file_in`（本物）を `Platform::MacOs` / `Platform::Windows` の両方で。探索の天井は一時 dir |
| 言語ごとの設定 | `IndentUnit::for_path`（本物。`text_edit.rs:244`）+ Enter の規則 `IndentRules::for_path`（`text_edit.rs:294`。private なのでコードで） |
| Finder | 各拡張子の空ファイルを作り、Swift の `URLResourceValues.contentType` で macOS が割り当てる UTI を引き、`scripts/build-app.sh:160` の宣言（UTI のみ）へ適合するかを `UTType.conforms(to:)` で判定 |

検査用のクレート（tako-core をパス依存）・LSP の使い捨てクライアント（node 標準ライブラリだけ）は
scratchpad に置き、リポジトリへは入れていない。tako-core の表の単体テスト
（`lsp::servers` / `platform::runner_defaults` / `open_plan` / `text_edit` のインデント）は
`cargo test -p tako-core --lib` を絞って 26 件緑を確認した（servers 6・runner_defaults 8・
open_plan 7・インデント 5）。

**Finder の列はこの PC の Launch Services の実測**。入っているアプリが UTI を宣言すると変わる
（例: `.tsx` の `com.microsoft.typescript` は他アプリ由来の宣言の可能性が高い）。

## 2. 対応表（45 形式 × 6 列）

記号: ✓ = 効く / △ = 効くが誤り・近似 / ✗ = 効かない。実行は「macOS / Windows」。
インデントは「既定の 1 段 / Enter で深くする規則」（括弧 = 行末が `{` `[` `(` で深く、
`:` = Python・YAML の `:` でも深く、継承 = 前の行のインデントを引き継ぐだけ）。

### 2.1 組み込み

| 形式 | プレビュー | 色分け | 言語サーバ | Code Runner（macOS / Windows） | 言語ごとの設定 | Finder |
|---|---|---|---|---|---|---|
| `.ino` | code | ✗ Plain Text | ✗ なし | ✗ なし / ✗ なし（`Makefile` の中でも拾われない） | 4 / 括弧 | ✗ 動的 UTI |
| `.pde` | code | ✗ Plain Text | ✗ なし | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |
| `.h` | code | △ Objective-C（C / C++ のヘッダ） | ✓ clangd（`c`） | ✗ / ✗（`Makefile` があれば `make`） | 4 / 括弧 | ✓ `public.c-header` |
| `.hpp` | code | ✓ C++ | ✓ clangd（`cpp`） | ✗ / ✗（`Makefile` → `make`） | 4 / 括弧 | ✓ `public.c-plus-plus-header` |
| `.c` | code | ✓ C | ✓ clangd（`c`） | ✓ `cc main.c -o main && ./main` / ✓ `gcc … -o main.exe; if ($?) { .\main.exe }` | 4 / 括弧 | ✓ `public.c-source` |
| `.cpp` | code | ✓ C++ | ✓ clangd（`cpp`） | ✓ `c++ …` / ✓ `g++ … .exe` | 4 / 括弧 | ✓ `public.c-plus-plus-source` |
| `.S` | code | ✗ Plain Text（`x86_64 Assembly` は `asm` / `nasm` 等だけ） | ✗ | ✗ / ✗（案内が小文字の `s` を名指す） | 4 / 括弧 | ✓ `public.assembly-source` |

### 2.2 授業でよく使う系

| 形式 | プレビュー | 色分け | 言語サーバ | Code Runner | 言語ごとの設定 | Finder |
|---|---|---|---|---|---|---|
| `.java` | code | ✓ Java | ✗ | ✓ `java Main.java`（両 OS） | 4 / 括弧 | ✓ `com.sun.java-source` |
| `.py` | code | ✓ Python | ✓ pyright（`python`） | ✓ `python3 main.py` / ✓ `python main.py`（`${python}` = 実行環境 #1730） | 4 / 括弧 + `:` | ✓ `public.python-script` |
| `.m` | code | △ Objective-C（MATLAB の構文は拡張子 `matlab` だけ） | ✗ | ✗ / ✗ | 4 / 括弧 | ✓ `public.objective-c-source` |
| `.r` / `.R` | code | ✓ R | ✗ | ✓ `Rscript stats.r`（両 OS） | 4 / 括弧 | ✓ `com.apple.rez-source`（Rez として適合） |
| `.jl` | code | ✓ Julia | ✗ | ✓ `julia model.jl` | 4 / 括弧 | ✗ 動的 UTI |
| `.tex` | code | ✓ LaTeX | ✗ | ✓ `latexmk -pdf -interaction=nonstopmode report.tex` | 4 / 括弧 | ✓ `org.tug.tex` |
| `.ipynb` | code（JSON の原文） | ✓ JSON（原文として） | ✗ | ✗ 理由つき（開いて編集するもの） / 同左 | 4 / 括弧 | ✗ 動的 UTI |

### 2.3 Web

| 形式 | プレビュー | 色分け | 言語サーバ | Code Runner | 言語ごとの設定 | Finder |
|---|---|---|---|---|---|---|
| `.vue` | code | ✓ Vue Component | ✗ | ✗ / ✗（`package.json` があっても npm の行は拾わない） | 4 / 括弧 | ✗ 動的 UTI |
| `.svelte` | code | ✓ Svelte | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |
| `.jsx` | code | ✓ JavaScript（`:2082` の補い） | ✓ typescript-language-server（`javascriptreact`） | ✓ `npx tsx App.jsx`（両 OS） | 4 / 括弧 | ✗ 動的 UTI |
| `.tsx` | code | ✓ TypeScriptReact | ✓ typescript-language-server（`typescriptreact`） | ✓ `npx tsx App.tsx`（`package.json` → 同じ形のプロジェクト既定） | 4 / 括弧 | ✓ `com.microsoft.typescript` |
| `.astro` | code | ✗ Plain Text | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |
| `.scss` | code | ✓ SCSS | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |

### 2.4 設定

| 形式 | プレビュー | 色分け | 言語サーバ | Code Runner | 言語ごとの設定 | Finder |
|---|---|---|---|---|---|---|
| `.toml` | code | ✓ TOML | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |
| `.yaml` | code | ✓ YAML | ✗ | ✗ / ✗ | 4 / 括弧 + `:` | ✓ `public.yaml` |
| `.json5` | code | ✗ Plain Text | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |
| `.ini` | code | ✓ INI | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |
| `.env` | code | ✓ DotENV（ファイル名で） | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ `public.data` |
| `Dockerfile` | code | ✓ Dockerfile（ファイル名で） | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ `public.data` |
| `Makefile` | code | ✓ Makefile（ファイル名で） | ✗ | ✓ `make`（プロジェクト既定。両 OS） | **タブ** / 括弧 | ✓ `public.make-source` |
| `CMakeLists.txt` | code | ✓ CMake（ファイル名で） | ✗ | ✗ / ✗（案内が `tako run-default txt` = `.txt` 全部に効く既定を勧める） | 4 / **継承**（拡張子 `txt` 扱い） | ✓ `public.plain-text` |

### 2.5 スクリプト

| 形式 | プレビュー | 色分け | 言語サーバ | Code Runner | 言語ごとの設定 | Finder |
|---|---|---|---|---|---|---|
| `.sh` | code | ✓ Bourne Again Shell | ✗ | ✓ `bash run.sh`（両 OS） | 4 / 括弧 | ✓ `public.shell-script` |
| `.zsh` | code | △ bash の構文で近似 | ✗ | ✓ `zsh run.zsh` / ✗ 理由つき（Windows に標準配布なし） | 4 / 括弧 | ✓ `public.zsh-script` |
| `.ps1` | code | ✗ Plain Text（PowerShell の構文が無い） | ✗ | ✓ `pwsh -NoLogo -NoProfile -File run.ps1`（両 OS） | 4 / 括弧 | ✗ 動的 UTI |
| `.bat` | code | ✓ Batch File | ✗ | ✗ 理由つき（macOS に `cmd.exe` なし） / ✓ `cmd /c run.bat` | 4 / 括弧 | ✗ 動的 UTI |
| `.lua` | code | ✓ Lua | ✗ | ✓ `lua init.lua` | 4 / 括弧 | ✓ `org.tug.lua` |
| `.rb` | code | ✓ Ruby | ✗ | ✓ `ruby app.rb` | 4 / 括弧 | ✓ `public.ruby-script` |
| `.pl` | code | ✓ Perl | ✗ | ✓ `perl tool.pl` | 4 / 括弧 | ✓ `public.perl-script` |

### 2.6 その他の上位

| 形式 | プレビュー | 色分け | 言語サーバ | Code Runner | 言語ごとの設定 | Finder |
|---|---|---|---|---|---|---|
| `.go` | code | ✓ Go | ✗ | ✓ `go run main.go`（`go.mod` → go プロジェクト） | **タブ** / 括弧 | ✗ 動的 UTI |
| `.rs` | code | ✓ Rust | ✓ rust-analyzer | ✓ `rustc … && ./main` / ✓ `.exe` 版（`Cargo.toml` → `cargo run`） | 4 / 括弧 | ✗ 動的 UTI |
| `.kt` | code | ✓ Kotlin | ✗ | ✓ `kotlinc … -d Main.jar && java -jar Main.jar` / ✓ `if ($?)` 版 | 4 / 括弧 | ✗ 動的 UTI |
| `.swift` | code | ✓ Swift | ✗ | ✓ `swift main.swift` | 4 / 括弧 | ✓ `public.swift-source` |
| `.cs` | code | ✓ C# | ✗ | ✓ `dotnet run Program.cs`（`.csproj` → `dotnet run`） | 4 / 括弧 | ✗ 動的 UTI |
| `.dart` | code | ✓ Dart | ✗ | ✓ `dart run main.dart` | 4 / 括弧 | ✗ 動的 UTI |
| `.zig` | code | ✓ Zig | ✗ | ✓ `zig run main.zig` | 4 / 括弧 | ✗ 動的 UTI |
| `.sql` | code | ✓ SQL | ✗ | ✗ 理由つき（接続先が決まらない） / 同左 | 4 / 括弧 | ✗ 動的 UTI |
| `.proto` | code | ✓ Protocol Buffer | ✗ | ✗ / ✗ | 4 / 括弧 | ✓ `public.protobuf-source` |
| `.graphql` | code | ✓ GraphQL | ✗ | ✗ / ✗ | 4 / 括弧 | ✗ 動的 UTI |

### 2.7 集計と表の外で見つけたこと

| 列 | 効く形式（45 中） | 根拠 |
|---|---|---|
| プレビュー | 45（すべて code。`.ipynb` は JSON の原文） | `crates/tako-core/src/open_plan.rs:72` は md / 画像 / pdf / 動画以外を code へ倒す |
| 色分け | ✓ 36・△ 3（誤り `.h` `.m`・近似 `.zsh`）・✗ 6 | two-face の構文は **213**（`preview.rs:2006` のコメントの「270+」は実数と食い違う） |
| 言語サーバ | 8（`.h` `.hpp` `.c` `.cpp` `.py` `.jsx` `.tsx` `.rs`） | `crates/tako-core/src/lsp/servers.rs:85`（4 行） |
| Code Runner | macOS 23・Windows 23（`Makefile` のプロジェクト既定を含む） | `crates/tako-core/src/platform/runner_defaults.rs:127` + `crates/tako-core/src/runner_project.rs:92` |
| 言語ごとの設定 | 例外はタブ 2（`Makefile` `.go`）・`:` 2（`.py` `.yaml`）・継承 1（`CMakeLists.txt`） | `text_edit.rs:244` / `:294` |
| Finder | 21（Windows は 0。`installer/windows/tako.iss` に `[Registry]` が無い） | `scripts/build-app.sh:160` は UTI だけで宣言（#708 の「既定を奪わない」） |

- **Markdown のコードブロック** ```` ```ino ```` / ```` ```arduino ```` / ```` ```pde ```` は色なし
  （`preview.rs:2378` の `highlight_lang` は `find_syntax_by_token` だけで別名の補いが無い）。
  ```` ```cpp ```` / ```` ```c++ ```` は C++ で塗られる
- **構文名を CLI / MCP から読む口が無い**。`tako edit` の文書の状態（`text_edit.rs:2182`）は
  `indent` を返すが構文を返さない。AI が「どの構文で塗られたか」を確かめられない
- **コメントの切り替え（⌘/）は機能として存在しない**ので、「コメント記号」の列は今は空。
  `tako:run:` 宣言の検出（`runner.rs:241` `find_marker`）はコメント記号を問わないので、
  `.ino` でも先頭に `// tako:run: arduino-cli compile …` と書けば今の tako で両 OS とも
  `Declaration` として解決する（実測）。今すぐ使える回避策

## 3. 表の正本の所在と本数

拡張子（またはファイル名）で引く表は 10 か所ある。

| # | 表 | 置き場 | 持つもの | CLI / MCP から読めるか |
|---|---|---|---|---|
| 1 | プレビューの種別 | `crates/tako-core/src/open_plan.rs:72`（+ `:101` の Content-Type は同じ表の粒度違い） | md / 画像 / pdf / 動画 / それ以外 = code | ✓ `tako links` の `open`・`tako open` の `mode` |
| 1' | 同じ判定の写し | `crates/tako-app/src/preview.rs:1158` `is_markdown_path`・`:1166` `image_format_from_path` | md と画像だけ | — |
| 2 | 構文の色分け | two-face の拡張子表（外部）+ `preview.rs:2059` `filename_to_syntax` + `:2077` `extension_fallback` + `:2378` `highlight_lang`（言語トークン） | 拡張子 → 構文 | ✗ |
| 3 | 言語サーバ | `crates/tako-core/src/lsp/servers.rs:85` `SERVERS` | 拡張子 → サーバ・languageId・ルートの印・導入コマンド | ✓ `tako lsp list` / `tako_lsp_server` |
| 4 | 実行の既定 | `crates/tako-core/src/platform/runner_defaults.rs:127` `TABLE` | 拡張子 → 両 OS のコマンド / 置かない理由 | ✓ `tako run-default` |
| 5 | 実行のプロジェクト | `crates/tako-core/src/runner_project.rs:92` `KINDS` | 印 + 拡張子 → プロジェクト既定 | ✓ `tako run --dry-run` / `tako_run_resolve` |
| 6 | 実行環境 | `crates/tako-core/src/runtime_env/kinds.rs:212` `KINDS` | interpreter の置き場（Python だけ） | ✓ 同上 |
| 7 | インデント | `crates/tako-core/src/text_edit.rs:244` `IndentUnit::for_path` + `:294` `IndentRules::for_path` | 既定の 1 段・Enter の規則 | △ `indent` だけ |
| 8 | アイコン | `crates/tako-app/src/file_icons.rs:507` / `:534` | ファイル名・拡張子 → アイコン | ✗ |
| 9 | Finder | `scripts/build-app.sh:160`（UTI だけ） | 「このアプリケーションで開く」 | — |
| 10 | Windows の関連付け | なし（`installer/windows/tako.iss`） | — | — |

**同じ並びが重複している例**:

| 言語群 | 書かれている場所 |
|---|---|
| C / C++（`c h cc cpp cxx hh hpp hxx`） | `servers.rs:103-112` / `runner_defaults.rs:172-191`（`h` 系なし） / `runner_project.rs:133` / `file_icons.rs:544-545` |
| JS / TS（`js mjs cjs jsx ts mts cts tsx`） | `servers.rs:124-133` / `runner_project.rs:110` / `file_icons.rs:537-541` / `preview.rs:2082-2084`（別名） |
| Python（`py pyi pyw`） | `servers.rs:145`（`pyw` なし） / `text_edit.rs:305` / `file_icons.rs:542` |

**言語を 1 つ今の形で揃えると直す場所**: 色（`preview.rs:2077`）・言語トークン（`preview.rs:2378`
に別名の口が無いので新設）・言語サーバ（`servers.rs:85` + 単体 `段階1は4行`）・実行の既定
（`runner_defaults.rs:127` + 単体 `表の規模` + `runner.rs:1519` の両 OS 固定テスト）・
プロジェクト（`runner_project.rs:92`）・アイコン（`file_icons.rs:534`）= **6 か所 + テスト 3 か所**
（インデントに例外があれば `text_edit.rs` が +1）。`.ino` ではさらに「C++ の構文で塗るが、
言語サーバは clangd ではない」という同定の判断が、色の補いと言語サーバの表に別々に書かれることになる。

## 4. 寄せる案（子 Issue の先頭）

### 4.1 何を寄せ、何を分けたままにするか

- **寄せる = 言語の同定**。新設 `crates/tako-core/src/file_type.rs`（純粋・I/O なし）に
  「形式 1 つ = 1 行」の表を置く:

  ```rust
  pub struct FileType {
      pub id: &'static str,                    // 安定 ID（"arduino" / "cpp" / "python" …）。応答のキー
      pub extensions: &'static [&'static str], // 小文字
      pub file_names: &'static [&'static str], // "Makefile" / "CMakeLists.txt" / "Dockerfile" …
      pub syntax: Option<&'static str>,        // two-face の構文名。None = two-face の拡張子表に任せる
      pub fence_tokens: &'static [&'static str], // Markdown の ``` の別名（"ino" / "arduino"）
      pub preview: PreviewRoute,               // open_plan の 5 種別
      pub indent: IndentDefault,               // Tab / Spaces(4)
      pub enter: EnterRule,                    // InheritOnly / Brackets / BracketsAndColon
  }
  pub fn identify(path: &Path) -> Option<&'static FileType>  // ファイル名 → 拡張子
  ```

  表に**全 213 構文を並べない**。行を持つのは「既定（code・two-face 任せ・4・括弧）から外れる形式」と
  「挙動の表（3〜6）が名指す形式」だけ。表に無い形式は今と 1 バイトも変わらない
- **寄せる先の利用者**: 表 1（`open_plan::preview_route`）・表 1'（app の写しは `identify` を呼ぶだけへ）・
  表 2（`filename_to_syntax` / `extension_fallback` を廃し、`syntax` → two-face の順。
  `highlight_lang` は `fence_tokens` を先に引く）・表 7（`IndentUnit::for_path` / `IndentRules::for_path`）
- **分けたままにする = 挙動の表 3〜6**（#1726 設計 §9.2 の判断を維持。1 枚にすると疎な表になる）。
  代わりに**番犬**で「挙動の表が名指す拡張子は必ず `file_type` に行がある」を強制する
  （= tako が知っている形式の一覧は `file_type` 1 か所）。`tako run-default <ext>` のように
  利用者が拡張子で書く設定は拡張子キーのまま
- **アイコン（表 8）と Finder（表 9 / 10）は対象外**。アイコンは言語以外（アーカイブ・フォント等）が
  大半で別軸、Finder は #708 の「既定を奪わない」制約で UTI 単位なので拡張子の表と 1:1 にならない

### 4.2 CLI / MCP（開発不変条件）

- プレビュー / 編集の応答（`tako edit` の文書の状態と `tako open` の結果）へ
  **`file_type`（ID）と `syntax`（実際に塗った構文名）**を足す。「どの構文で塗ったか」を
  AI が読める状態にする（今は読めない）。新しいツールは作らない（カタログ予算 200 KB）

### 4.3 受け入れ条件の骨子

1. **挙動を変えない**: two-face の全拡張子（213 構文の `file_extensions` 全部）と §2 の 45 形式について、
   寄せる前後で「構文名・プレビュー種別・インデント既定・Enter の規則」が全数一致する単体
2. 番犬: `file_type.rs` の外（`preview.rs` / `text_edit.rs` / `open_plan.rs`）に拡張子の文字列リテラルの
   `match` が残っていたら file:line で名指す。挙動の表 3〜5 の拡張子が `file_type` に無ければ落ちる
3. 応答に `file_type` / `syntax` が載る（CLI と MCP の両方。dispatch 経由の同じ 1 本）
4. ついでに直す小さな食い違い: `preview.rs:2006` / `:2022` の「270+ 構文」を実数へ・
   `CMakeLists.txt` が拡張子 `txt` として Enter の規則を「継承」にされる件（ファイル名の行で直る）

## 5. `.ino`（Arduino）の設計

### 5.1 この PC での確認結果

| 項目 | 結果 |
|---|---|
| PATH の `arduino-cli` / `arduino-language-server` | どちらも無い |
| Arduino IDE | `/Applications/Arduino IDE.app`（2.3.5）。`Contents/Resources/app/lib/backend/resources/` に **arduino-cli 1.2.0・arduino-language-server・clangd 14.0.0・clang-format** を同梱 |
| PATH の `clangd` | `/usr/bin/clangd`（Apple clangd 17.0.0） |
| 入っているコア | `arduino:avr` 1.8.8 / `arduino:renesas_uno` 1.6.0 / `arduino:esp32` / `esp32:esp32` / `rp2040:rp2040`（データは `~/Library/Arduino15`） |
| 授業のスケッチ | `~/Documents/Arduino` 配下に `.ino` + 自作 `.h` 10 本前後の構成のものがある。**`sketch.yaml` は 0 件** |
| IDE が選んでいる基板 | `arduino:renesas_uno:unor4wifi`（稼働中の IDE の起動引数）。IDE は基板の選択を自前の内部保存（`~/.arduinoIDE/`）にだけ持ち、スケッチ側には書かない |
| brew | `arduino-cli`（formula 1.5.1）と `arduino-ide`（cask 2.3.10）はある。**arduino-language-server の formula は無い** |
| winget | `ArduinoSA.CLI` / `ArduinoSA.IDE.stable`（winget-pkgs の manifests で確認。実機では未実行） |

### 5.2 色分け

- **C++ の構文で塗る**（`file_type` の `arduino` 行に `syntax: Some("C++")`）。VS Code 同梱の
  cpp 言語定義も `.ino` と `.h` を含む（`microsoft/vscode` の `extensions/cpp/package.json`）。
  Arduino 固有の構文（`setup` / `loop` / `HIGH` 等の強調）は two-face に無く、足すと構文セットが増えるので採らない
- Markdown の ```` ```ino ```` / ```` ```arduino ```` も C++ で塗る（`fence_tokens`）
- **#815（構文セットの常駐）への影響**（同じ版の syntect / two-face で、生きているヒープを数えて実測）:

  | 状態 | 構文セットを載せた直後 | 244 行のスケッチを塗った後 |
  |---|---|---|
  | 今（`.ino` = Plain Text） | +1.15 MB | +2.78 MB |
  | C++ へ寄せた後（= 同じ中身の `.cpp`） | +1.15 MB | **+20.93 MB（塗りで +19.78 MB）** |

  **構文は 1 本も増えない**（213 のまま）ので、`.ino` を開いていないときの常駐は 0。増えるのは
  `.ino` の表示中だけで、`.cpp` を開いたときと同額。#815 の解放（最後の使用から 30 秒 =
  `preview.rs:1668` `SYNTAX_IDLE_GRACE`）でそのまま戻る

### 5.3 言語サーバ

**clangd 単体は使えない**（LSP の使い捨てクライアントで実測。スケッチは `pinMode` / `Serial` /
自作ヘッダを使う 12 行の正しいもの）:

| 起こし方 | 結果 |
|---|---|
| clangd（Apple 17 / IDE 同梱 14）・compile db 無し | 1 行目に `Unable to handle compilation, expected exactly one compiler job` の 1 件だけ。ホバーは null（clang の driver が `.ino` を知らない） |
| clangd + `arduino-cli compile --only-compilation-database` の db（3.8 秒で生成） | db は `.ino` ではなく変換後の `build/sketch/<名>.ino.cpp`（`#include <Arduino.h>` と `#line` を足したもの）を指す。推測で当たると **正しいスケッチに嘘のエラー 10 件**（`-mmcu=` 不明・`LED_BUILTIN` / `OUTPUT` / `Serial` 未宣言）。ホバーは null |
| **arduino-language-server**（IDE 同梱）+ `-fqbn arduino:avr:uno` | **診断 0 件**（2.1 秒）・`pinMode` のホバーが `void pinMode(uint8_t pin, uint8_t mode)` |
| 同上で誤りを 1 つ入れたスケッチ | **2 件を `.ino` の行番号で**（`Use of undeclared identifier 'blinkMss'; did you mean 'blinkMs'?`） |
| 同上・**基板の指定なし**（`-fqbn` 無し・`sketch.yaml` 無し） | 15 秒待って **診断が 1 回も届かず、ホバーも時間切れ** = 黙って何も出ない |
| `-fqbn` 無し・`sketch.yaml` に `default_fqbn` あり | 診断 0 件・ホバー可（`sketch.yaml` だけで足りる） |
| `-cli-config` 無し | 起動直後に `Path to ArduinoCLI config file must be set.` で終了 |
| `-cli-config` に **0 バイトのファイル** | 正常（中身は要らない） |

**決めたこと**:

1. 検出表（`servers.rs`）に `arduino-language-server` の行を足し、`.ino` を受け持たせる
   （languageId は `cpp`。IDE と同じ）。`clangd` の行には `.ino` を入れない
2. 起動引数に**他の実行ファイルのパス**が要る（`-cli <arduino-cli> -clangd <clangd> -cli-config <ファイル>`）。
   今の `ServerSpec.args` は固定文字列（`servers.rs:26`）なので、表の行が「伴走する実行ファイル」
   （フラグ + 実行ファイル名）と「tako が用意する空の設定ファイル」（フラグ）を持てる形へ広げる。
   言語名・実行ファイル名は**表の中だけ**に書く（`issue1678_lsp_watchdog.rs` の規則を守る）。
   伴走は**見つけた ALS と同じディレクトリを先に**探す（IDE 同梱の版の組み合わせが実測で通った形）。
   設定ファイルは `<data_dir>` に 0 バイトで置く
3. **基板（FQBN）が無ければ起こさない**。状態機械（`crates/tako-core/src/lsp/state.rs`）に「基板が未選択」の理由を足し、
   `tako lsp status` と画面（#1944 の状態表示）へ「`arduino-cli board attach -b <基板>` で選ぶ」を出す。
   基板の正本は `sketch.yaml` の `default_fqbn`（§5.4）。`-fqbn` は渡さない（正本を 2 つにしない）
4. **探索の場所**に Arduino IDE の同梱ディレクトリを足す（macOS `/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/`、
   Windows は既定のユーザー単位インストール先の同等の場所 = **未実機確認**）。PATH → IDE 同梱の順。
   `TAKO_LSP_BIN_ARDUINO_LANGUAGE_SERVER`（`servers.rs:197` の機構）での差し替えはそのまま効く
5. ルートはスケッチのフォルダ（`.ino` のあるディレクトリ）。印は `sketch.yaml`、無ければファイルのディレクトリ
6. **スケッチ内の `.h` / `.cpp`**: 今は拡張子で clangd へ行き、Arduino の型を知らないので嘘のエラーが出る
   （自作 `.h` に `#include <Arduino.h>` を書いた例で `'Arduino.h' file not found` を含む計 3 件）。
   同じ `.h` を ALS へ渡すとホバーは返るが、疑わしい診断が 1 件残った（IDE と同じ「先に `.ino` を開く」順では
   測っていない）。**スケッチのフォルダ内の C / C++ を ALS へ回す**のは、その順で測り直してから決める
   （検出表の解決が拡張子だけでなく「ディレクトリに `<フォルダ名>.ino` があるか」を見る形になるため、
   表の形の変更を伴う）
7. 常駐: ALS + clangd + arduino-cli の別プロセスで合計およそ 90 MB（avr・12 行のスケッチ・
   1 セッションの概算。稼働中の IDE のプロセスと名前が同じなので内訳は曖昧）。tako のヒープではない

### 5.4 Code Runner（コンパイルと書き込み）

実測（同梱の arduino-cli 1.2.0・`arduino:avr:uno`・ビルドのキャッシュは一時 dir）:

| 呼び方 | 結果 |
|---|---|
| `arduino-cli compile --fqbn arduino:avr:uno <dir>` | exit 0（5.6 秒。コアのキャッシュが冷えた状態） |
| `arduino-cli compile <dir>`（基板なし） | `Missing FQBN (Fully Qualified Board Name)` で exit 1 |
| `arduino-cli board attach -b arduino:avr:uno <dir>` | スケッチのフォルダに `sketch.yaml`（`default_fqbn: arduino:avr:uno` の 1 行）を書く |
| `sketch.yaml` のあるフォルダで**引数なしの `arduino-cli compile`** | exit 0 |
| フォルダ名と `.ino` の名前が違う | `main file missing from sketch: <dir>/<フォルダ名>.ino` で exit 1 |
| `arduino-cli board list`（基板を繋いでいない） | シリアルポートだけが並び、FQBN は付かない |

**決めたこと**:

1. **プロジェクト種別に `arduino` を足す**（`runner_project.rs:92` の `KINDS` へ 1 行）。印は
   「そのフォルダに `<フォルダ名>.ino` がある」か `sketch.yaml`。既定は **`arduino-cli compile`**
   （#322 の最簡形・cwd = スケッチのフォルダ）。プロファイル `upload` は **`arduino-cli compile --upload`**
   （ポートは `sketch.yaml` の `default_port`）。拡張子既定の表（`runner_defaults.rs`）には足さない
   （スケッチはフォルダ単位でしか走らない）
2. **実行ファイルの探索**は `${python}`（#1730 の実行環境の変数）と同じ「変数で実行ファイルを
   差し込む」形にし、PATH → Arduino IDE 同梱 → 素の `arduino-cli`（見つからないときの案内が
   自然になる）の順に解く。IDE 同梱の場所を探す関数は**言語サーバの伴走の探索（§5.3 の 4）と
   1 実装**にする（置き場を `runtime_env/kinds.rs:212` の行にするか別の純粋関数にするかは実装スライスで決める）
3. **3 つの失敗を人の言葉にする**（今は `Missing FQBN` 等の英語がそのまま流れる）:
   基板が未選択（`arduino-cli board attach -b <基板>` を案内。繋がっている基板が 1 台だけで
   `board list` が FQBN を返すならその値を候補に入れる）・フォルダ名と `.ino` 名の不一致・
   arduino-cli が無い（§5.5 の導入案内）
4. **基板の正本は `sketch.yaml`**。tako の保存先（#1731 の `run-configs.json`）には持たない。
   Arduino IDE / arduino-cli と同じファイルを見るので食い違わない。tako から書くときも
   `arduino-cli board attach` を呼ぶ（書式を tako が真似しない）。**ドロップダウンでの選択は
   #1731 / #1733 の後のスライス**（それまでは案内のコマンドを人か AI がペインで打てば済む =
   CLI / MCP から操作できる状態は最初から満たす）
5. 書き込み（upload）は基板とポートの実機が要るので **未検証**。実装スライスの受け入れ条件で
   実機（授業の基板）を 1 回通す

### 5.5 無い環境のフォールバックと導入の案内

- **arduino-cli も ALS も無い**: 色分けだけ効く（§5.2）。言語サーバは `not_installed`、実行は
  「arduino-cli が無い」の案内。**壊れない・黙らない**
- **導入の案内は `tako setup` の標準（`setup_deps.rs:106` の表）に載せない**。あの表は全員に
  [y/N] で聞く tako 自身の依存（tmux / git / tailscale）で、Arduino を使わない人にも聞くことになる。
  `.ino` を開いたときだけ、言語サーバの状態表示（#1944）と実行の案内に出す
- 案内するコマンド（#322 の最簡形）:

  | 目的 | macOS | Windows |
  |---|---|---|
  | 言語サーバまで揃える（推奨） | `brew install --cask arduino-ide` | `winget install --id ArduinoSA.IDE.stable` |
  | 実行だけ | `brew install arduino-cli` | `winget install --id ArduinoSA.CLI` |

  ALS 単体の配布（formula）は無いので、言語サーバは「IDE を入れたら tako が同梱物を見つける」が
  現実的な経路。授業で IDE を入れている人は**追加の導入ゼロ**で効く
- **コアの導入**（`arduino-cli core install arduino:avr` 等）は数百 MB のツールチェーンを落とすので
  tako は代行しない。基板の選択の案内で「コアが入っていない」と分かったら、そのコマンドを示すだけ
- #1944（未導入の言語サーバをその場で取得して起こす）が入ったら、ALS も同じ口
  （`tako lsp install`）に載せられるかをそちらで判断する。ここでは IDE 同梱の探索だけを持つ

### 5.6 Windows

- 検出・解決はすべて `Platform` を引数で受ける純粋関数にして、macOS の単体から Windows の
  解決結果を検査する（#1616 / #1655 の作法）
- IDE 同梱の場所・`arduino-cli.exe` の名前・シリアルポート（`COM3` 等）は**未実機確認**として
  `tako platform` の既知の制約へ申告する

## 6. 残りの穴の扱い

| 穴 | 方針 | 子 Issue |
|---|---|---|
| `.h` → Objective-C | C++ へ（VS Code と同じ。C のヘッダとしても致命的に崩れない） | 色の補い |
| `.m` → Objective-C | 授業の MATLAB と衝突。本文の手掛かり（`%` コメント・`function … end` / `#import`・`@interface`）で判定し、MATLAB の構文（two-face に `MATLAB` がある）へ | 色の補い |
| `.pde` → 色なし | Processing（Java 系）として Java で塗る。旧 Arduino のスケッチ（`.pde`）は今では少数 | 色の補い |
| `.json5` / `.jsonc` → 色なし | JSON で塗る（コメント・末尾カンマの扱いを確認） | 色の補い |
| `.S` / `.s` → 色なし | 既存の `x86_64 Assembly` へ寄せてよいか（AVR / ARM の命令は x86 ではない）を見て判断 | 色の補い |
| `.ps1` / `.astro` → 色なし | two-face に構文が無い。構文を足すと構文セット（#815）とビルドに効くので、足す方法と実測を別に | 構文の追加 |
| 言語サーバ 37 形式 | #1007 の「第 2 波」（Java / Go / …）。#1944 の自動取得と合わせて優先順を決める | 言語サーバ第 2 波 |
| Finder 24 形式・Windows 0 | #708 の「既定を奪わない」を守ったまま広げられるか（UTI の輸入宣言の可否）を lsregister で実測 | Finder / 関連付け |
| Code Runner の空き（`.vue` `.svelte` `.astro` `.scss` `.toml` `.yaml` `.json5` `.ini` `.env` `Dockerfile` `.proto` `.graphql` `.h` `.hpp`） | **単体では走らせるものではない**ので今のまま（`.vue` 等は `package.json` の `npm run` を `tako:run:` で書く）。`.m` / `.pde` / `.S` は需要が出たら行を足す | なし |

## 7. 子 Issue（依存つき）

正本は #1943 のコメント。

| 順 | 子 Issue | 中身 | 依存 | 規模 |
|---|---|---|---|---|
| S1 | #1948 | 言語の同定の正本 `file_type` へ寄せる（挙動は変えない）+ 応答に `file_type` / `syntax` | なし | M |
| S2 | #1949 | `.ino` を C++ で塗る（`file_type` に 1 行・```` ```ino ````・アイコン・#815 実測） | #1948 | S |
| S3 | #1950 | `.ino` の Code Runner（`arduino` プロジェクト種別・`arduino-cli` の探索・3 つの失敗の案内） | なし（並べる順は S2 の後） | M |
| S4 | #1951 | `.ino` の言語サーバ（ALS の行・伴走の実行ファイル・基板未選択なら起こさない） | #1950 | M |
| S5 | #1952 | 基板とポートを実行設定のドロップダウン / `tako run-config` で選ぶ（`sketch.yaml` が正本） | #1950・#1731・#1733 | S〜M |
| S6 | #1953 | 既存の構文への別名で埋まる色の穴（`.h` `.m` `.pde` `.json5` `.S`） | #1948 | S |
| S7 | #1954 | two-face に無い構文（`.ps1` / `.astro` 等）を足せるか + #815 実測 | #1948 | M（調査込み） |
| S8 | #1955 | Finder / Windows の関連付けを動的 UTI の形式へ広げられるか | なし | S（調査込み） |
| S9 | #1956 | 言語サーバの検出表の第 2 波（授業・上位形式の優先順） | #1948・#1944 | M |

`.ino` を急ぐなら、#1949 の中身（`.ino` → C++）は #1948 を待たずに `preview.rs:2077` の補い 1 行でも
入る。その場合は #1948 で `file_type` の行へ移す（#1943 の方針「寄せるのが先」とどちらを採るかは master の判断）。
