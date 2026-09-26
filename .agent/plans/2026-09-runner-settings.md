# Code Runner の実行設定 + 実行環境（Python の venv / conda / pyenv / uv 等）の設計（Issue #1726）

- 作成日: 2026-09-26
- 種別: 実装前の設計（読み取りだけで書いた。コード変更なし）
- エピック: #1726（親は #1647 の D レーン）。前提 #453 / #1655（着地済み）・#1656（作業中）・#1657 / #1662（未着手）
- 根拠の行番号は main `c94a67a` の実測。ずれてもシンボル名で追えるよう関数名・型名を併記する
- 前の設計: `.agent/plans/2026-07-code-runner.md`（#453。宣言の文法・変数展開・信頼モデルはそちらが正本）

ユーザーの依頼（2026-09-26・原文）:

> コードランナー系の機能，python だったら env 選択できるようにしたりとか，実行ボタンのとなりにドロップダウン出して押したら色々設定とか，できるようにして欲しい

## 0. 結論（先に要点）

1. **設定の層を 1 つ足すだけにする**。コマンドを決める層（宣言 → #1656 のプロジェクト既定 →
   拡張子既定）はそのまま残す。その上に「**このマシンでこのファイル / プロジェクトをどう走らせるか**」
   （実行環境・引数・環境変数・作業ディレクトリ・実行前コマンド・実行先・選択中のプロファイル）を重ねる
2. **既定で賢く動かす（#322）**。プロジェクトに `.venv` があれば、何も設定しなくてもそれで走る。
   ドロップダウンは上書きしたい人のためのもの
3. 保存先は **`<data_dir>/run-configs.json` を新設**し、共有カタログではローカル扱いにする。
   `settings.json` は #513 で他デバイスへ同期される区分なので、マシン固有の interpreter パスを置くと
   別の機械で壊れる（§4.2）
4. 実行環境は **ID（`venv:.venv` / `pyenv:3.12.4` / `conda:ml`）で保存して実行時に解く**。
   VS Code の Python 拡張と同じ作法（設定にはパスを直書きしない）。解けなければ既定へ落とさずにエラーにする（#1466）
5. 環境変数・PATH の前置・実行前コマンドは **`SpawnOptions.env` ではなくコマンド文字列へ埋める**。
   任意のキーは tmux の器の中へ届かないため（`backend/mod.rs:166-201`）。組み立ては境界 B1（`platform::shell`）の 1 実装に置く
6. **MCP のツールは増やさない**（推奨）。読み取りは `tako_run_resolve`、書き込みは `tako_run_defaults` の引数を足して表す。CLI は `tako run-config <file>` を 1 本足す
7. スライスは S0〜S7 の 8 本。S1（純粋部分）は #1656 と並走できる。dispatch の `Run` を触る
   S2 以降は #1656 → #1657 → #1662 の後に直列で進める

---

## 1. 現状の棚卸し

### 1.1 コマンドを決める層（tako-core）

| 部品 | 場所 | 要点 |
|---|---|---|
| 宣言パーサ | `crates/tako-core/src/runner.rs:103` `parse_declarations` | 先頭 64 行 / 16 KiB。キーは `run` / `cwd` / `shell` の 3 種 |
| 変数展開 | `runner.rs:275` `expand_variables` | `${file}` `${fileDir}` `${fileBase}` `${fileNoExt}` `${ext}`。展開値はシングルクオート。未知の `${…}` は残す |
| 解決器 | `runner.rs:354` `resolve` → `:376` `resolve_for` | 優先順は override → 宣言 → 拡張子既定 → エラー。`RunPlan { profile, command, cwd, shell, source }`（`:53`）。`RunSource` は 3 値（`:42`） |
| 拡張子既定のマージ | `runner.rs:330` `merged_defaults` / `:335` `merged_defaults_for` | 組み込み表 → `settings.json` の `runner_defaults`。空文字は無効化 |
| 組み込み既定表 | `crates/tako-core/src/platform/runner_defaults.rs:127` `TABLE` | 41 拡張子 × macOS / Windows。`py` 行は `:155`（macOS `python3` / Windows `python`）。コンパイル系の Windows 行は `; if ($?) { .\x.exe }`（`:167-191`） |
| UI 用の先頭読み | `runner.rs:549` `read_file_head_for_ui` | 16 KiB |

**実行環境（interpreter / toolchain）という概念はまだ無い**。`py` は既定表の `python3` / `python` が
そのまま PATH から引かれるので、`.venv` を作っても使われない。

### 1.2 dispatch（tako-control）

| 部品 | 場所 | 要点 |
|---|---|---|
| `Request::Run` | `crates/tako-control/src/protocol.rs:1983-2004` / `crates/tako-control/src/dispatch.rs:5308-5432` | path / pane / tab / profile / command / direction / ratio / auto_close / focus。**`settings::load()` と先頭読みを UI スレッドで行う**（`:5358-5362`） |
| `Request::RunResolve` | `protocol.rs:2006` / `dispatch.rs:5434-5484` | 応答は `{ path, profiles[{profile, command, cwd, source}], warnings, default_profile }` |
| `Request::RunnerDefaults` | `protocol.rs:2008-2018` / `dispatch.rs:5486-5550` | 拡張子既定の一覧 / 1 件 / 設定 / 削除。保存先は `settings.json` |
| 実行ペインの起動 | `dispatch.rs:5837` `spawn_command_pane` | `SpawnOptions { env: Vec::new() }` 固定（`:5869`）。起動コマンドは境界 B1 `platform::shell::run_pane_command`（`crates/tako-core/src/platform/shell.rs:85`） |
| 重い読み取りの逃がし先 | `dispatch.rs:175` `prepare_offload` / `:130` `OffloadJob` | 子プロセスを伴う読み取り（git / CheckHealth 等）を UI スレッドから外す仕組み。**Run 系は未対象** |

### 1.3 PC の UI（tako-app）

| 部品 | 場所 | 要点 |
|---|---|---|
| 状態 | `crates/tako-app/src/main.rs:2185-2190` | `preview_run_menu`（開いているペイン + anchor）/ `preview_run_profiles`（検出結果のキャッシュ）/ `preview_run_selected`（**メモリ上だけ**。再起動で消え、CLI / MCP から見えない） |
| 検出 | `crates/tako-app/src/preview_render.rs:4242` `detect_preview_run_profiles` | 呼び出し元は `main.rs:22177`（OpenFile）/ `main.rs:4432`（復元）/ `sidebar.rs:2954`（ライブリロード）。**UI スレッドで同期実行** |
| 再生ボタン | `preview_render.rs:2619-2721` | `phv.run_button`（幅 250 以上。`crates/tako-core/src/header_layout.rs:66,80`）かつ Code / Markdown |
| シェブロン | `preview_render.rs:2722-2780` | **プロファイルが 2 つ以上のときだけ出る**（`run_has_multiple`） |
| メニュー | `preview_render.rs:4336` `render_run_menu_overlay` | プロファイル名 + コマンド先頭 40 バイト。行クリックで「選択 + 即実行」 |
| 実行 | `preview_render.rs:4261` `run_preview_file` | dirty なら先に保存 → `Request::Run { direction: Down, ratio: 0.3, focus: true }` |
| 文言 | `crates/tako-app/src/ui_text/preview.rs:34-45` | `run_button` / `run_no_command` / `run_profile_default` |
| 設定画面 | `crates/tako-app/src/settings_window.rs:33-42`（`SettingsTab::Runner`）/ `:2027` `render_runner_tab` | 拡張子既定の表を編集する画面。`tako settings --tab runner` で開く |

### 1.4 CLI / MCP

| 口 | 場所 |
|---|---|
| `tako run <file> [--profile] [--command] [--pane] [--tab] [--right] [--ratio] [--auto-close] [--focus] [--wait] [--list]` | `crates/tako-cli/src/main.rs:406-439`（`RunArgs`）/ 変換 `:8518-8539` / `--list` は `:8669` `run_list` |
| `tako run-default [ext] [command] [--remove]` | `main.rs:441-450` / 変換 `:8540-8544` |
| CLI → MCP の対応表 | `main.rs:10507` `CLI_KEY_OVERRIDES`（`("run-default", "tako_run_defaults")` が `:10550`） |
| MCP `tako_run` / `tako_run_resolve` / `tako_run_defaults` | `crates/tako-control/src/mcp/catalog.rs:3997` / `:4068` / `:4087`、変換 `crates/tako-control/src/mcp/request.rs:1138-1164` |

カタログの実測（2026-09-26・インストール済みバイナリの `tako context-budget`）: **202,956 / 204,800 バイト（154 本）**。
Run 系は `tako_run` 2,787 / `tako_run_resolve` 712 / `tako_run_defaults` 653 バイト。
#1723（#1711）が着地すると余白 17.6 KiB。ただし LSP（#1007）も同じ余白を使う。

### 1.5 保存先

- `settings.json` の `runner_defaults`（`crates/tako-control/src/settings.rs:117`）だけ。移行の番地は
  `SchemaId::Settings`（`crates/tako-core/src/migration.rs:44-46`）、SPECS は `crates/tako-control/src/migrations.rs:413`
- `settings.json` は共有カタログで **`Class::Shared`**（`crates/tako-control/src/config_share/catalog.rs:229-237`）。
  デバイス固有として外してあるのは `welcome_dismissed` / `update_card_dismissed` の 2 フィールドだけ

### 1.6 棚卸しで見つかった問題（設計の前提として直す / 避ける）

| # | 問題 | 場所 | 扱い |
|---|---|---|---|
| P1 | **コマンドをバイト位置で切っていて、文字の途中なら panic する**。描画のたびに評価される `_tooltip` が `&cmd[..60]` を取る。例: `python3 'aデータ解析結果のまとめレポート最終版.py'`（68 バイト）は 60 バイト目が文字の途中にあたる。ヘッダ幅 250 以上で実行コマンドが解決できるファイルなら、プレビューを開いた時点で落ちる計算になる（実行での再現は未実施）。メニュー側の `&plan.command[..40]` も同じで、`python3 'report_最終版_データ解析結果まとめ_v2.py'` で落ちる | `preview_render.rs:2635-2636` / `:4361-4362` | **S0**（独立したバグとして先に直す） |
| P2 | `_tooltip` を組み立てているのにどこにも出していない（旧設計 §4.2 の「クリック前に何が走るか見える」が実現されていない） | `preview_render.rs:2628-2644` | S5 でメニューの先頭に「実行されるコマンド」として出す |
| P3 | シェブロンがプロファイル 2 つ以上のときしか出ない。設定の入口として使えない | `preview_render.rs:2723` | S5 |
| P4 | 選択中のプロファイルがメモリ上だけで、CLI / MCP から読めない（設計原則 5 の穴） | `main.rs:2190` | S3 で保存対象にする |
| P5 | `SpawnOptions.env` の任意のキーは tmux の器へ固定されない。固定されるのは `PANE_SCOPED_ENV` とシェル統合のキーだけ | `crates/tako-core/src/backend/mod.rs:166-168,183-201` | 環境変数はコマンド文字列へ埋める（§6.3） |
| P6 | `git::repo_root` は `git rev-parse` を上限なしの `.output()` で呼ぶ（#1503 の規約外） | `crates/tako-core/src/git.rs:317` | 検出には使わない。`.git` の有無をファイルシステムで見る（§5.3） |

---

## 2. 世間の実装と、ここで採る形

| 案 | 採用実績 | 形 | 採るもの / 採らないもの |
|---|---|---|---|
| **VS Code の Python 拡張**（推奨の骨格） | 最大 | interpreter の自動選択は「ワークスペース直下の `.venv` / `venv` → グローバル」の順。設定には**環境マネージャーへの参照**を書き、選んだ個別の環境は別に覚えて実行時に解く。ターミナルでは自動で activation する | **採る**: 自動選択の順・ID で覚えて実行時に解く・activation。**採らない**: `.vscode/settings.json` への書き込み（tako はリポジトリへ書かない = §4.2） |
| JetBrains の Run Configuration | 大 | 名前付きの構成（interpreter・引数・環境変数・作業ディレクトリ・Before launch）。既定はローカル保存で、「Store as project file」を選ぶと `.run/` へ書いて共有する | **採る**: 項目の並び（引数・環境変数・作業ディレクトリ・実行前）。**採らない**: 名前付き構成の管理画面（tako では宣言のプロファイルが名前付き構成に当たる） |
| VS Code の Code Runner 拡張 | 中 | 拡張子 → コマンドの表（`executorMap`）に `$pythonPath` 変数 | **採る**: 変数で interpreter を差し込む形（`${python}`） |

採否の軸は「tako にすでにあるものと重ならないか」。共有できる実行定義は、ファイル内の `tako:run` 宣言が
すでに担っている（リポジトリに入り、チームで共有される）。足りないのは「**このマシンの**環境」と
「**この人の**上書き」だけなので、足す層はマシンローカルの 1 層に絞る。

---

## 3. 用語と全体像

- **実行設定（run config）**: ファイル / プロジェクトごとに覚える上書き。項目は §4.1 の 7 つ
- **実行環境（runtime）**: interpreter / toolchain の選択。種類（kind）ごとに 1 つ（`python` / `node` / `rust` …）
- **スコープ**: `file`（そのファイルだけ）/ `project`（プロジェクトルート配下の全ファイル）
- **プロジェクトルート**: §4.5 の関数 1 本で決める（#1656 と同じ答えを返す）

```
          ┌───────────── コマンドを決める層（既存 + #1656）───────────┐
 path ──▶ │ override > 宣言 > プロジェクト既定(#1656) > 拡張子既定    │──▶ RunPlan(command テンプレート, cwd, shell)
          └────────────────────────────────────────────────────────────┘
                                   │
          ┌──────── 実行設定の層（この設計で足す）────────────────┐
          │ file 設定 > project 設定 > 宣言の cwd/shell > 既定      │──▶ EffectiveConfig
          │ runtime: 保存した ID > 自動選択（§5.2）                 │     (runtime, args, env, cwd, before, target, profile)
          └────────────────────────────────────────────────────────┘
                                   │
          ┌──────── 組み立て（境界 B1 = platform::shell）──────────┐
          │ ${python} ${args} を展開 → env / PATH 前置 / before を  │──▶ run_pane_command(…) → 実行ペイン
          │ 実行ペインのシェルの方言で前に付ける                    │
          └────────────────────────────────────────────────────────┘
```

---

## 4. データモデルと保存先

### 4.1 型（`crates/tako-core/src/runner_config.rs` を新設。純粋・I/O なし）

```rust
/// 1 スコープぶんの上書き。None / 空 = 上位へ委ねる
pub struct RunConfig {
    /// 選択中のプロファイル（宣言名。None = 既定の選び方 = runner.rs の規則）
    pub profile: Option<String>,
    /// 実行環境（kind → 選択）。project スコープでは複数言語を持てる
    pub runtime: BTreeMap<String, RuntimeRef>,
    /// 引数。**シェルへそのまま渡る 1 本の文字列**（JetBrains の Program arguments と同じ）
    pub args: Option<String>,
    /// 環境変数（値はリテラル。tako の変数 `${fileDir}` 等だけ展開する）
    pub env: BTreeMap<String, String>,
    /// 作業ディレクトリ（相対はファイルのディレクトリ基準 = `tako:cwd` と同じ意味論）
    pub cwd: Option<String>,
    /// 実行前コマンド（失敗したら本体を走らせない）
    pub before: Option<String>,
    /// 実行先（#1657 着地後の S6 で足す）
    pub target: Option<RunTarget>, // Reuse | New
    /// LRU 用（ファイル数の上限で古いものから落とす）
    pub updated_at: Option<String>,
}

/// 保存する実行環境の参照（パスを直書きしない。実行時に §5 で解く）
pub struct RuntimeRef {
    /// §5.1 の表の検出器 ID（"venv" / "uv" / "conda" …）か "path"（ユーザーが指定した interpreter）。
    /// enum にしないのは、道具の名前を表の外（この型）へ並べないため（#1729 で確定）
    pub manager: String,
    /// manager ごとの鍵: venv = 環境のディレクトリ（プロジェクトルートからの相対も可）/ pyenv = 版名 /
    /// conda = env 名（一覧の中で一意でなければ prefix）/ system = プログラム名 / path = 絶対パス
    pub key: String,
}

pub enum FieldSource { File, Project, Declaration, Auto, Default }

/// マージ結果（項目ごとに「どこから来たか」を持つ = UI の出典タグと応答の source）
pub struct EffectiveConfig { /* 各項目 (値, FieldSource) */ }

pub fn merge(file: Option<&RunConfig>, project: Option<&RunConfig>, decl: &RunPlan) -> EffectiveConfig;
```

`RunConfigField`（項目名）は `ALL` を持つ enum にする。`manager` の受理値は表から作る
（`RuntimeKind::manager_ids()`）。どちらも MCP の `enum` を正本から生成するため
（`.agent/conventions.md`「MCP カタログの enum は正本から生成する（Issue #1467）」）。

### 4.2 保存先: `<data_dir>/run-configs.json`（新設）

```json
{
  "version": 1,
  "projects": {
    "~/dev/myapp": { "runtime": { "python": { "manager": "venv", "key": ".venv" } }, "env": { "APP_ENV": "dev" } }
  },
  "files": {
    "~/dev/myapp/scripts/report.py": { "profile": "test", "args": "--verbose", "updated_at": "2026-09-26T10:00:00Z" }
  }
}
```

（例のパスは `~` 表記で書いたが、実際のキーは §4.5 の正規化済み絶対パス）

置き場の比較は §13 J4。推奨は上の形で、理由は 3 つある。

1. 中身は**マシン固有**（interpreter の場所・絶対パスのキー）。`settings.json` は共有区分（§1.5）なので、
   相乗りすると別のデバイスで壊れた選択が効いてしまう
2. 共有したい実行定義は、すでにファイル内の `tako:run` 宣言が担っている（§2）
3. リポジトリ内へ書く形（`.tako/run.json` / JetBrains の `.run/`）は、public リポへ
   個人の環境を混入させる経路になる。このリポジトリの 2026-07-06 の事故と同じ型

ファイル数の上限は **`files` 1,000 件**。超えたら `updated_at` の古いものから落とす
（消えたファイルを勝手に掃除しない。外付けドライブのように、いま見えないだけのことがあるため）。

### 4.3 マージの優先順（項目ごと）

```
CLI / MCP の一回限りの指定  >  file 設定  >  project 設定  >  宣言（tako:cwd / tako:shell）  >  自動（§5.2）  >  既定
```

- **保存した設定は宣言より強い**。宣言はファイルの作者の既定で、ドロップダウンはこのマシンの利用者の明示の上書き。
  後から明示した方を勝たせる。応答と UI には項目ごとの `source` が載るので、どちらが効いているかは常に見える
- `env` はキー単位で重ねる（file の `APP_ENV` が project の `APP_ENV` を上書きし、他のキーは残る）
- `runtime` は kind 単位で重ねる
- `command` の上書き（既存の `--command`）は実行設定の層を素通りする。一回だけ全部変えたいときの逃げ道として残す

### 4.4 #916（スキーマ移行）と共有カタログ

新しい永続ファイルなので、既存データの変換は要らない（`settings.json` の `runner_defaults` は動かさない）。
ただし `.agent/conventions.md`「設定・データファイルのスキーマ変更（Issue #916）」の
「新しい永続ファイルを足すとき」に従い、次の 3 か所へ載せる。載せ忘れは `migration_registry` /
`config_share_catalog` テストが名指しで落とす。

1. `SchemaId::RunConfigs`（`migration.rs:44` の enum）+ `as_str` = `"run_configs"`
2. `migrations::SPECS` に `pristine(SchemaId::RunConfigs, Some(validate_run_configs))`（`migrations.rs:413` の並び）と `targets`（`:494` の並び）
3. `config_share::catalog::CATALOG` に `Class::Local`（note は「マシン固有のパス」）

S6 で `target` を足すときは `#[serde(default)]` で旧ファイルがそのまま読めるので、
移行 Step は要らない。その旨を PR に明記し、指紋スナップショットだけ更新する。

### 4.5 パスのキーとプロジェクトルート

- キーにするパスは `tako_core::platform::path::canonicalize`（境界 B26）を通した表記
  （`.agent/conventions.md`「`canonicalize` の結果を持ち回らない（Issue #970）」。保存する値なので B26 を通す）
- プロジェクトルートは **`tako_core::project_root::detect(file)` の 1 本**で決める。
  #1656 の cwd と同じ答えを返させる（ずれると「project に保存したのに効かない」になる）。
  #1656 が先に着地するので、#1656 がこの関数を置く形が最短（§11 の [提案]）
- 上へ辿る範囲は「`.git` のある階層（その階層は含む）」か「ホーム（含まない）」の近い方まで。
  ホームをプロジェクトルートにしない。`.git` の判定はファイルシステムだけで行う（P6）

---

## 5. 実行環境の検出

### 5.1 表の形（言語追加 = 行追加）

`crates/tako-core/src/runtime_env/`（新設・純粋）に、kind ごとの行を持つ表を置く。

```rust
pub struct RuntimeKind {
    pub id: &'static str,                    // "python"
    pub extensions: &'static [&'static str], // ["py"]
    /// コマンドテンプレートで使う変数名（`${python}`）
    pub variable: &'static str,
    /// 実行環境が見つからないときの変数の値（OS 別。既存の既定表と同じ値）
    pub fallback: PerOs<&'static str>,       // macOS "python3" / Windows "python"
    /// 自動選択の順に並べた検出器（§5.2）
    pub detectors: &'static [Detector],
}

/// 検出器は「戦略 + パラメータ」。新しい言語は既存の戦略の組み合わせで書ける
pub enum Detector {
    /// 印のファイルを持つディレクトリ（venv = pyvenv.cfg）
    MarkerDir { names: &'static [&'static str], marker: &'static str, layout: EnvLayout },
    /// 版を書いたファイル（.python-version / .nvmrc）→ ツールの置き場の versions/<版>
    VersionFile { files: &'static [&'static str], root_env: &'static [&'static str], root_default: PerOs<&'static str>, layout: EnvLayout },
    /// ロックファイル等の印 → ツールで包む（uv run / poetry run / pipenv run）
    Wrapper { markers: &'static [&'static str], program: &'static str, args: &'static [&'static str], env_dir_probe: Option<ProbeCmd> },
    /// 一覧ファイル（conda の environments.txt）とプロジェクト宣言（environment.yml の name:）の突き合わせ
    NamedEnvList { list_file: PerOs<&'static str>, project_files: &'static [&'static str], layout: EnvLayout },
    /// PATH 上の名前（最後の砦）。解くのは実行ペインのシェル。tako は Tier P で表示用に引くだけ
    PathLookup { names: PerOs<&'static [&'static str]>, exclude_dirs: PerOs<&'static [&'static str]> },
    /// 環境変数で切り替える toolchain（rustup の RUSTUP_TOOLCHAIN）
    EnvSwitch { files: &'static [&'static str], var: &'static str },
}

/// OS で変わる置き場（venv の bin/ と Scripts\ 等）。両 OS の列を 1 枚に持つ（#1655 の作法）
pub struct EnvLayout { pub interpreter: PerOs<&'static str>, pub path_dirs: PerOs<&'static [&'static str]> }
```

- 検出は「**見てよいディレクトリの列（近い順）**」を引数で受ける。辿る範囲の 1 実装
  （天井 = HOME / `.git` で止める / 深さ上限）は #1656 の `project_root::candidate_dirs` が持ち、
  S1 は写しを置かない（#1729 で確定）
- **表の外に言語名を書かない**（`if kind == "python"` を散らさない）。番犬で止める。
  LSP S1 の検出表（`.agent/plans/2026-09-lsp-s1.md` §3）と同じ規則
- ファイル参照は trait で差し替えられるようにする（`FsProbe { exists, is_dir, read_head(path, max) }`）。
  `tako_core::migration::MigrationIo` / `FsIo`（`migration.rs:585-597`）と同じ作法。
  macOS の単体テストから、Windows の置き場（`Scripts\python.exe`）まで偽のファイルシステムで固定できる

### 5.2 Python の候補と自動選択の優先順

候補は**全部並べ**（ドロップダウンと `tako_run_resolve` の `runtimes`）、そのうち 1 つを自動で選ぶ。
ユーザーが選んだ ID があれば、それが常に勝つ。

| 順 | 候補 | 検出（Tier F = ファイルシステムだけ） | 実行への適用 |
|---|---|---|---|
| 0 | ユーザーの選択（file > project） | 保存した `RuntimeRef` を解く | 下のどれか |
| 1 | **uv プロジェクト** | プロジェクトルートに `uv.lock`（または `pyproject.toml` の `[tool.uv]`）があり、`uv` が在る。在否は Tier F では**既知の置き場**（表に持つ: `~/.local/bin` / `~/.cargo/bin` / Homebrew の bin 等）と GUI プロセスの PATH だけを見る。見つからなければ Tier P の `platform::exe::find` が済むまで 1 を飛ばして 2 へ | `uv run` で包む。`.venv` があれば activation も併用 |
| 2 | **プロジェクト内の venv** | ファイルのディレクトリからルートまで上へ辿り、各階層で `.venv` → `venv` → その他 `pyvenv.cfg` を持つ直下ディレクトリ（名前順）。**近い階層が勝つ**（monorepo のパッケージごとの venv） | activation（PATH 前置 + `VIRTUAL_ENV`）+ `${python}` = その interpreter |
| 3 | Poetry | `poetry.lock` または `[tool.poetry]` があり、2 が無い（venv が in-project でない） | `poetry run` で包む。Tier P の `poetry env info -p` が済んでいれば activation も併用 |
| 4 | Pipenv | `Pipfile` | `pipenv run` で包む（Tier P の `pipenv --venv` で activation を併用） |
| 5 | conda | プロジェクトの `environment.yml` / `environment.yaml` の `name:` を、conda の一覧ファイル `environments.txt` の prefix と突き合わせる。一致が 1 つのときだけ選ぶ | activation（PATH 前置 + `CONDA_PREFIX` / `CONDA_DEFAULT_ENV`。§13 J3） |
| 6 | pyenv | `.python-version`（ファイルから上へ。1 行目。`system` は飛ばす）→ `$PYENV_ROOT`（無ければ既定の置き場）`/versions/<版>` が在る | activation + `${python}` |
| 7 | システム（**自動選択の最後の砦**） | 解かない。表示用の場所だけ GUI プロセスの PATH を覗いて取る（macOS `python3` → `python` / Windows `python`。Windows の App Execution Alias = `%LOCALAPPDATA%\Microsoft\WindowsApps\` は外し、それしか無ければ警告）。版は Tier P の `platform::exe::find` で引く | `${python}` = `fallback` の名前のまま。**解くのは実行ペインのシェル**（今と同じ。PATH は触らない）。1〜6 が無ければこれが自動で選ばれる = 今と同じ |

優先順の根拠は次のとおり。

- **venv（2）を conda / pyenv（5・6）より前に置く**: venv はそのプロジェクトの依存を入れた環境で、最も具体的。
  `.python-version` は版しか決めず、conda の名前一致はプロジェクトの外にある環境。
  VS Code の自動選択も「ワークスペース直下の `.venv` / `venv` → グローバル」の順。
  uv は `.python-version` を読んで `.venv` を作るので、両方あれば `.venv` がすでに版を反映している
- **uv（1）を venv（2）より前に置く**: uv のプロジェクトでは `uv run` が正規の走らせ方で、実行のたびにロックへ同期する。
  `.venv/bin/python` を直接叩くと、ロックに足した依存が入っていないまま走る。§13 J2 で確認したい点
- **Poetry / Pipenv（3・4）は包む形を既定にする**: venv の置き場が `<キャッシュ>/virtualenvs/<名前>-<ハッシュ>-py<版>` で、
  ファイルシステムだけでは確定できない。`poetry run` ならツール自身が解くので、Tier P を待たずに正しく走る
- **conda（5）は一致が 1 つのときだけ**: 候補が複数あるときに 1 つを選ぶのは、既定へ落とすのと同じ事故
  （`.agent/conventions.md`「宛先が解けないものは既定へ落とさない（Issue #1466）」）

`VIRTUAL_ENV` / `CONDA_PREFIX` を tako の GUI プロセスの環境から読むことはしない。
Finder から起動した GUI にはどちらも無く、端末から起動した場合だけ効く、という不定な挙動になるため。

### 5.3 検出コストとスレッド

検出は 2 段に分ける。

| 段 | 中身 | どこで走るか |
|---|---|---|
| **Tier F**（ファイルシステムだけ） | stat と小さなファイルの先頭読み（`pyvenv.cfg` / `.python-version` / `environment.yml` / `environments.txt` は 4 KiB、`pyproject.toml` は 64 KiB まで）。版も読める範囲で取る（`pyvenv.cfg` の `version_info` / `version`、pyenv はディレクトリ名、conda は `conda-meta/python-*.json` のファイル名） | GUI はプレビューを開いたときに **background executor** で走らせ、結果を状態へ入れる。dispatch の `Run` は**キャッシュを引く**。キャッシュが冷えていれば Tier F を同期で走らせる（上限付きなので `read_file_head` と同じ重さの部類） |
| **Tier P**（子プロセス） | `platform::exe::find`（unix は**ログインシェルを起こして** `command -v` を聞く = `crates/tako-core/src/platform/exe.rs:80-95`。見かけは探索だが子プロセス）/ `poetry env info -p` / `pipenv --venv` / `conda info --envs --json`（一覧ファイルが無いとき）/ システム python の版（`python3 --version`）/ Windows の `py -0p` | **`tako_core::probe::output_with_timeout`（`crates/tako-core/src/probe.rs:255`）だけ**を通す（`.agent/conventions.md`「外部コマンドを待つときは上限を持つ（Issue #1503）」）。走るのは一覧の取得（`RunResolve` を `prepare_offload` へ載せる）と「再検出」のときだけ。**`Run` の経路では走らせない** |

- Tier F の上限: 上へ辿るのは最大 32 階層。1 階層あたりの stat は検出器の数（Python は 8 前後）まで。
  目標は **1 回 50 ms 以内**（ローカル SSD なら数 ms の見込み。S1 / S2 の受け入れ条件で実測する）
- キャッシュは tako-control に置く（`runtime_probe::Cache`。プロジェクトルートごと）。
  無効化の指紋は「検出器が見た印のファイルの mtime の組」。GUI と dispatch が同じ 1 つを引く。永続化はしない
- 描画は完成品を読むだけにする（#232 の目次と同じ原則）。`render_*` から検出関数を呼んだら番犬で落とす

### 5.4 Windows との違い（両 OS の列を 1 枚に持つ）

| 項目 | macOS | Windows |
|---|---|---|
| venv の interpreter | `<venv>/bin/python` | `<venv>\Scripts\python.exe` |
| venv の PATH 前置 | `<venv>/bin` | `<venv>\Scripts` |
| conda の一覧ファイル | `~/.conda/environments.txt` | `%USERPROFILE%\.conda\environments.txt` |
| conda の interpreter | `<prefix>/bin/python` | `<prefix>\python.exe` |
| conda の PATH 前置 | `<prefix>/bin` | `<prefix>` / `<prefix>\Library\mingw-w64\bin` / `<prefix>\Library\usr\bin` / `<prefix>\Library\bin` / `<prefix>\Scripts`（**要実機確認**） |
| pyenv の置き場 | `$PYENV_ROOT` → `~/.pyenv` | pyenv-win: `$PYENV_ROOT` → `%USERPROFILE%\.pyenv\pyenv-win`（**要実機確認**） |
| pyenv の interpreter | `<root>/versions/<版>/bin/python` | `<root>\versions\<版>\python.exe` |
| Poetry の venv | Tier F では推測しない（ハッシュ付きの名前）。Tier P の `poetry env info -p` | 同左 |
| システム | `python3` → `python` | `python` → `py`。App Execution Alias は除外 |
| env を埋める形 | `export K='V';` / `export PATH='<dir>':"$PATH";` | `$env:K = 'V';` / `$env:PATH = '<dir>;' + $env:PATH;` |
| 実行前コマンド | `<before> && <本体>` | `<before>; if ($?) { <本体> }`（PowerShell 5.1 でも通る形 = #1655） |

Windows の列は macOS の単体テストで形を固定する。実機での動作は `platform::support::MATRIX` の
既存キー（`tako_run_resolve` 等。`crates/tako-core/src/platform/support.rs:1361-1393`）の
`windows_evidence` へ、実測できた分だけを書く（#591。過大申告しない）。

### 5.5 保存した ID が解けないとき

`RuntimeRef` を解いて interpreter が無かったら（`.venv` を消した・pyenv の版を消した）、
**自動選択へ落とさずに `Run` をエラーにする**。メッセージには次の一手を載せる
（「`.venv` が見つかりません。`tako run-config report.py --reset runtime` で自動選択へ戻すか、ドロップダウンで選び直してください」）。
黙って別の interpreter で走ると、依存が無いというエラーが出て、原因がたどれない（#1466 と同じ構図）。
応答には `runtime_unresolved: { manager, key, next_step }` を載せる。

---

## 6. 実行への適用（コマンドの組み立て）

### 6.1 変数を 2 つ足す

| 変数 | 値 |
|---|---|
| `${python}`（一般には各 kind の `variable`） | 選んだ実行環境の interpreter。包む形（uv / poetry / pipenv）なら `uv run python` 等。何も無ければ `fallback` |
| `${args}` | 実行設定の `args`（空なら空文字） |

- **`${args}` がテンプレートに無ければ末尾へ足す**。macOS のコンパイル系（`cc a.c -o a && ./a`）は、末尾に足すと
  `./a` の引数になるので正しい
- **末尾へ足すと壊れる形は、表の側で `${args}` を明示する**。Windows のコンパイル系（`runner_defaults.rs:167-191`）は
  `}` の後ろへ付くと構文が壊れるので、`if ($?) { .\${fileNoExt}.exe ${args} }` へ直す。
  「`if ($?) {` を含む行は `${args}` を含む」を表のテストで縛る
- `expand_variables`（`runner.rs:275`）は `path` だけを受ける署名なので、`RunVars { path, workspace_root, runtime_values, args }` の
  文脈を受ける形へ広げる。#1656 が `${workspaceRoot}` を足すので、**その文脈構造体は #1656 の形に合わせる**

### 6.2 実行環境の当て方

- **activation**（venv / conda / pyenv）: `path_dirs` を PATH の先頭へ、`VIRTUAL_ENV` / `CONDA_PREFIX` 等を env へ入れる。
  `${python}` を使わない宣言（`tako:run: pytest -x`）でも、`pytest` / `pip` / `python3` がその環境のものに解ける
- **包む**（uv / poetry / pipenv）: `${python}` を `uv run python` 等にする。宣言が `pytest` を直接呼ぶ場合は包みが効かないので、
  activation を併用できる（env の場所が分かっている）ときは併用する。分からないときは、応答の `warnings` に
  「`${python} -m pytest` と書くと環境が効きます」を載せる
- 組み込み表の `py` 行は `${python} ${fileBase}` へ変える。**実行環境が無いときの展開結果が今と 1 バイトも変わらない**ことを
  両 OS のテストで固定する（`fallback` = 今の `python3` / `python`）

### 6.3 環境変数・PATH・実行前コマンドはコマンド文字列へ埋める

`SpawnOptions.env` を使わない理由は 2 つある。

1. 任意のキーは tmux の器（persist ON）の中へ届かない。器へ固定されるのは `session_pinned_env` が通すキーだけ（P5）
2. 実行ペインはユーザーの対話シェルで起きる（FR-2.5.14 / #1031）。rc ファイル（`eval "$(pyenv init -)"` 等）が
   PATH を組み直すので、**プロセスの env で前置した PATH は rc に上書きされうる**。コマンド文字列に埋めれば、rc の後に効く

組み立ては境界 B1 に 1 本足す（方言を知っているのは境界だけ）。

```rust
// crates/tako-core/src/platform/shell.rs（純粋。方言を引数で受ける = macOS から Windows 形を検査できる）
pub struct RunScript<'a> {
    pub env: &'a [(String, String)],
    pub path_prepend: &'a [String],
    pub before: Option<&'a str>,
    pub command: &'a str,
}
pub fn compose_run_script(dialect: ShellDialect, s: &RunScript) -> String;
```

- 値の引用は `ShellDialect::quote_arg`（`crates/tako-core/src/platform/shell_dialect.rs:413`）で行う。
  env の値はリテラル（シェルの `$VAR` は展開しない）で、tako の変数だけ先に展開する
- `tako:shell` の宣言（`declared_shell_command`。`shell.rs:143`）で内側のシェルが変わっても、外側で export / `$env:` した値は子へ継承される
- 最終の文字列は既存の `run_pane_command(command, EXIT_MARKER_PREFIX)` へ渡す。終了マーカー（#651 / #1657）の契約は変えない
- **診断ログへ出さない**。env の値・引数・コマンドはユーザーデータ（AGENTS.md の絶対ルール。旧設計 §8.1）

### 6.4 `Run` / `RunResolve` の応答に足すもの

- `Run`: `runtime`（`{ kind, manager, key, label, path, source }`）/ `config_sources`（項目ごとの出典）/ `runtime_unresolved`（§5.5）
- `RunResolve`: `config`（EffectiveConfig を項目ごとの `{ value, source }` で）/ `runtimes`（候補の一覧。各 `{ id, manager, label, version, path, auto, selected, tier }`）/ `project_root`

---

## 7. UI（実行ボタン横のドロップダウン）

### 7.1 ヘッダ

- シェブロンは **`run_capable` か、拡張子に実行環境の kind があるときは常に出す**（P3）
- ボタン本体の右に、選んだ実行環境の短いラベル（`.venv 3.12` / `uv` / `conda: ml`）を淡色で出す。
  幅の閾値は `PreviewHeaderVisibility` に `run_runtime_label` を足して決める（案: 360 以上。`header_layout.rs` のテストに加筆）
- 本体クリックの挙動は今と同じ（選択中のプロファイルを実行）

### 7.2 メニューの構成

```
┌──────────────────────────────────────────────┐
│ 実行されるコマンド                              │  ← P2。全文を折り返して出す（切るなら文字境界）
│ uv run python report.py --verbose             │
├──────────────────────────────────────────────┤
│ プロファイル                    （2 つ以上のとき）│
│  ✓ 実行      python report.py                  │  ← ✓ は check.svg。行クリック = 選択 + 実行（今と同じ）
│    test      python -m pytest report.py        │
├──────────────────────────────────────────────┤
│ 実行環境（Python）                [プロジェクト] │  ← 出典タグ。クリックでファイル ↔ プロジェクトを切り替え
│  ✓ uv run（自動）            uv 0.4 / 3.12.4  │
│    .venv                     3.12.4           │
│    pyenv 3.11.9                               │
│    conda: ml                 3.10.14          │
│    システム python3                            │
│  ↻ 再検出    ▸ パスを指定…                     │  ← refresh.svg / folder_ui.svg。再検出は Tier P
├──────────────────────────────────────────────┤
│ 引数            --verbose          [ファイル]   │  ← 行クリックでその場の TextField 編集
│ 環境変数        APP_ENV=dev         [プロジェクト]│  ← 1 変数 1 行 + 「追加」行
│ 作業ディレクトリ （ファイルの場所）     [既定]     │
│ 実行前コマンド   未設定               [既定]     │
│ 実行先          ● 同じペイン ○ 新しいペイン       │  ← S6（#1657 の後）
├──────────────────────────────────────────────┤
│ Code Runner の設定を開く                        │  ← Request::Settings { tab: "runner" }
└──────────────────────────────────────────────┘
```

（✓ ↻ ▸ ● ○ はモックの記号。実物は SVG と GPUI の図形で描く。§7.5）

### 7.3 保存の単位

- **項目ごとの出典タグ**（ファイル / プロジェクト / 宣言 / 自動 / 既定）で、どこに保存されているかを見せる。
  タグのクリックで、その値の保存先をファイル ↔ プロジェクトで移す。全体トグルは置かない（項目ごとに自然な単位が違うため）
- 新しく入れた値の既定の保存先は、**実行環境 = プロジェクト、それ以外 = ファイル**。
  VS Code も interpreter はワークスペース単位、引数は起動構成単位で持つ
- プロジェクトルートが無い単独ファイル（`~/Downloads/a.py`）は、プロジェクトのタグを出さずにファイルへ保存する
- 各行の「既定に戻す」は、その項目をそのスコープから消す（=`reset`）

### 7.4 状態とスレッド

- 状態は `preview_run_profiles` の隣に `preview_run_resolved: HashMap<PaneId, RunResolveSnapshot>` を置き、
  **dispatch `RunResolve` の応答をそのまま持つ**（UI 専用の解決ロジックを作らない。開発不変条件）
- 取得は background（`prepare_offload` の job を GUI からも使う）。描画は完成品を読むだけ。取得中は前回の値を出す
- 書き込みは `Request::RunnerDefaults { path, scope, … }` を dispatch 直呼び（既存パターン `run_preview_file` と同じ）→ 成功したら `RunResolve` を取り直す
- `preview_run_selected`（`main.rs:2190`）は廃止し、`profile` の保存へ寄せる（P4）
- 閉じたときの掃除は既存の場所（`preview_render.rs:3576-3587`）へ 1 行足す
- テキスト入力は `text_field::TextField` を通す（`.agent/conventions.md`「アプリ内テキスト入力は `TextField` を通す（Issue #1450 / #1459）」）

### 7.5 アイコン・文言

- 使うのは既存の SVG だけ（`assets/icons/ui/` の `play` / `chevron_down_ui` / `check` / `refresh` / `folder_ui` / `plus` / `close`）。**新しいアセットは要らない**。
  絵文字は使わない（`.agent/conventions.md`「UI に絵文字を使わない・印は描画プリミティブで描く」）
- 文言は `ui_text/preview.rs` の `run_*` キーへ足す（日英。#435）
- コマンドの切り詰めは**文字境界で**行う（P1。`floor_char_boundary` 相当を 1 か所に置き、番犬で `&cmd[..N]` 形を落とす）
- 「再検出」の回転表示を足すなら、いつ終わるかを先に決める（`.agent/conventions.md`「UI アニメーションは『いつ終わるか』を決めてから足す（Issue #945）」）。Tier P の上限で必ず止まる

---

## 8. CLI / MCP の口

### 8.1 dispatch の変更

| Request | 変更 | 読み / 書き |
|---|---|---|
| `RunResolve { path, pane }` | 応答へ `config` / `runtimes` / `project_root`。引数へ `refresh: bool`（Tier P を走らせる）。**`prepare_offload` へ載せる** | 読み |
| `RunnerDefaults { ext, command, remove }` | 引数へ `path` / `scope`（`file` / `project`）/ `profile` / `runtime` / `args` / `env`（object。値 `null` = そのキーを消す）/ `cwd` / `before` / `target`（S6）/ `reset`（項目名の配列）を足す。`path` なら実行設定、`ext` なら今までの拡張子既定。**両方は拒否** | 書き（`path` だけなら読み） |
| `Run { … }` | 実行設定を自動で重ねる（§4.3）。引数は増やさない（一回だけ全部変えるなら既存の `command`。実行先の一回限りは #1657 の `new_pane`） | 実行 |

`scope` を省略したときの保存先は §7.3 の既定（実行環境 = project、それ以外 = file）。

### 8.2 CLI（最簡形 = #322）

```
tako run-config report.py                        # 実行設定の実効値 + 実行環境の候補を表示（RunResolve）
tako run-config report.py --runtime .venv        # 実行環境を選ぶ（既定でプロジェクトに保存）
tako run-config report.py --args "--verbose"     # 引数（既定でファイルに保存）
tako run-config report.py --env APP_ENV=dev      # 環境変数（--unset-env APP_ENV で消す）
tako run-config report.py --cwd .. --before "make build"
tako run-config report.py --reset runtime        # 自動選択へ戻す（--reset だけで全項目）
```

- 上級者向けのフラグは `--project` / `--file`（保存先の明示）と `--refresh`（Tier P）。標準の案内には出さない
- `--runtime` は候補の ID（`venv:.venv`）・短縮（`.venv` / `uv` / `3.12.4`）・絶対パスのどれでも受ける。
  短縮が複数の候補に当たったら、候補を並べてエラーにする（1 つを選ばない = #1466）
- 表示は dispatch の戻り値から組む（`.agent/conventions.md`「CLI と MCP は『同じ 1 本』を通す（Issue #1453 / #1544）」）
- `CLI_KEY_OVERRIDES`（`main.rs:10507`）に `("run-config", "tako_run_defaults")` を足す
- `tako run <file> --list` は `RunResolve` を出すので、足した `config` / `runtimes` がそのまま載る

### 8.3 MCP（ツールを増やすか）

| 案 | 形 | カタログの増分（見積り） | 評価 |
|---|---|---|---|
| **A（推奨）既存 2 本へ足す** | 読みは `tako_run_resolve`（応答が増える + `refresh`）、書きは `tako_run_defaults`（`path` / `scope` / 項目 / `reset`） | 約 1.2〜1.9 KB（`tako_run_defaults` の引数の説明を 1 行ずつに絞れば下限側） | 新しいツール名・`platform::support::MATRIX` の行・parity の写像が要らない。「実行の既定をスコープ別に持つ口」として意味もつながる（今の `ext` が 3 つ目のスコープになる） |
| B 新ツール `tako_run_config` 1 本 | `action`（show / set / reset / runtimes） | 約 1.6〜2.2 KB + MATRIX 1 行 | 意味の分離はきれいだが、予算と表の保守が増える |
| C `tako_run` に `save: true` | 実行のツールで保存も行う | 約 0.8 KB | 「走らせない保存」が表せず、意味が濁る。不採用 |

- 案 A の `tako_run_defaults` の説明は「拡張子 / プロジェクト / ファイルの実行既定を読み書きする。
  ext と path は排他」を 1 回だけ書き、引数からは但し書きを落とす（`.agent/conventions.md`「MCP カタログの説明文は AI が使える情報だけ載せる（Issue #1540）」）
- `scope` / `manager` / `reset` の enum は正本（`RunConfigField::ALL` 等）から `enum_schema` で生成する（#1467）
- `tako_run` の説明に 1 行だけ足す: 「実行設定（tako_run_defaults で保存）と自動検出した実行環境が自動で効く。応答の `runtime` を見る」
- 受け入れ条件で `tako context-budget` の `mcp_catalog` を測り、増分を PR に書く

### 8.4 開発不変条件との対応

| ドロップダウンの操作 | dispatch | CLI | MCP |
|---|---|---|---|
| 候補と実効値を見る | `RunResolve` | `tako run-config <f>` / `tako run <f> --list` | `tako_run_resolve` |
| 実行環境を選ぶ / 戻す | `RunnerDefaults { path, runtime }` / `reset` | `--runtime` / `--reset runtime` | `tako_run_defaults` |
| 再検出 | `RunResolve { refresh: true }` | `--refresh` | `tako_run_resolve { refresh }` |
| 引数・環境変数・作業ディレクトリ・実行前 | `RunnerDefaults { path, args/env/cwd/before }` | `--args` / `--env` / `--cwd` / `--before` | `tako_run_defaults` |
| 保存先の切り替え | `RunnerDefaults { scope, … }` | `--project` / `--file` | `scope` |
| プロファイルの選択 | `RunnerDefaults { path, profile }` | `--profile`（run-config 側） | `tako_run_defaults { profile }` |
| 実行先（S6） | `RunnerDefaults { path, target }` | `--target reuse` / `--target new` | `target` |
| 設定画面を開く | `Settings { tab: "runner" }`（既存） | `tako settings --tab runner` | `tako_settings`（既存） |

---

## 9. 他言語へ広げる枠と、#1656 / LSP との共有

### 9.1 行の例（S7 で入れる候補）

| kind | 検出器（§5.1 の戦略） | 備考 |
|---|---|---|
| node | `VersionFile(.nvmrc / .node-version → nvm / fnm の versions)` → `MarkerDir(node_modules/.bin を PATH 前置)` → `PathLookup(node)` | volta は `package.json` の `volta` をシム自身が読むので、tako は手を出さない |
| rust | `EnvSwitch(rust-toolchain.toml / rust-toolchain → RUSTUP_TOOLCHAIN)` | rustup は cwd の `rust-toolchain.toml` を自分で読む。行が要るのは明示の上書きだけ。**features は実行環境ではなく引数**（#1656 の `cargo run` に `--features x` を project の `args` で足す） |
| go / java / ruby | 同じ戦略の組み合わせ（`.go-version` / `.java-version` / `.ruby-version`） | 需要が出たら行を足す |

### 9.2 何を共有し、何を分けるか

- **共有する**: 上へ辿る 1 本（`tako_core::project_root::find_upwards(start, stop, predicate)` + 停止境界）と
  `platform::exe::find`。#1656 のプロジェクト検出・この設計の実行環境検出・LSP S1 の `lsp/root.rs`（`.agent/plans/2026-09-lsp-s1.md` §5）が同じ関数を呼ぶ
- **分ける**: 表そのもの。LSP の行は「言語サーバ」（program・args・root_markers・導入案内のキー）、#1656 の行は
  「ビルドツール」（印 → コマンドテンプレート・cwd）、この設計の行は「interpreter / toolchain の置き場」（印 → interpreter・PATH・env）。
  1 枚にすると、ほとんどの行で意味の無い欄が並ぶ疎な表になる。「行追加で言語が増える」という形だけを揃える
- **データの受け渡し**: LSP の Python 行（pyright）は、ここで選んだ実行環境を `python.pythonPath` として受け取るべき
  （VS Code の Pylance と同じ）。関数 `runner_config::effective_runtime(project_root, "python")` を公開し、
  LSP の Python スライスがそれを読む（§11 の [提案]）

---

## 10. スライス分割

規模の目安: S = 差分 300 行以下 / M = 300〜800 行 / L = 800〜1,500 行（テスト込み）。
各スライスは単独でビルド・テストが緑になり、その時点で足した機能は CLI / MCP から操作できる。

### S0: コマンドの切り詰めで文字の途中を切らない（独立したバグ）

- 触る: `crates/tako-app/src/preview_render.rs`（`:2635-2636` / `:4361-4362`）。切り詰めの関数を 1 つ置く
- 受け入れ: 多バイトのコマンド（§1.6 P1 の 2 例 + 絵文字 + 境界ちょうど）で panic しない単体テスト。
  `&x[..N]` 形の切り詰めを `preview_render.rs` で落とす番犬（注入 A/B で名指しで FAILED → 戻して緑）
- 依存: なし（#1657 も `preview_render.rs` を触るので、先に入れる）/ 規模: S

### S1: 実行設定の型と Python の実行環境の検出（tako-core の純粋部分）

- 触る: 新設 `crates/tako-core/src/runner_config.rs` / `crates/tako-core/src/runtime_env/{mod.rs,kinds.rs,detect.rs}` /
  `crates/tako-core/src/lib.rs`（`pub mod` の追加だけ）。上へ辿る処理は持たず、見てよいディレクトリの列を引数で受ける
  （列は S2 が #1656 の `project_root::candidate_dirs` から作る）
- 中身: §4.1 の型と `merge`、§5.1 の表と戦略、Python の行（§5.2 の 1〜7）、`FsProbe` と偽の実装、`RuntimeRef` の解決、
  適用結果（`${python}` の値 / PATH 前置 / env / 包み）を返す純粋関数
- 受け入れ:
  - 偽のファイルシステムで、§5.2 の順を 1 行ずつ固定する（uv + .venv / .venv のみ / venv 名違い / monorepo で近い方 / poetry / pipenv / conda の一致 1 件・2 件（選ばない）/ pyenv の `system` を飛ばす / WindowsApps 除外 / 何も無い）
  - **両 OS の列**（`Scripts\python.exe` / conda の PATH 5 本 / pyenv-win）を macOS から検査する
  - `merge` の優先順（§4.3）と `env` のキー単位の重ね
  - 番犬: `runtime_env` の表の外に言語名（`"python"` 等）を書かない
  - `cargo test -p tako-core` 全緑・clippy 3 宇宙 0
- 依存: なし（**#1656 と並走できる**。触るファイルが重ならない）/ 規模: L

### S2: 自動検出した実行環境で走る（設定なし = ゼロコンフィグ）

- 触る: `crates/tako-control/src/dispatch.rs`（`Run` / `RunResolve` / `prepare_offload` + `OffloadJob::RunResolve`）/
  新設 `crates/tako-control/src/runtime_probe.rs`（Tier P + キャッシュ）/ `crates/tako-core/src/runner.rs`（`expand_variables` の文脈化と `${python}`）/
  `crates/tako-core/src/platform/runner_defaults.rs`（`py` 行 → `${python}`）/ `crates/tako-core/src/platform/shell.rs` + `shell_dialect.rs`（`compose_run_script` の PATH / env 部分）/
  `crates/tako-control/src/mcp/catalog.rs` + `request.rs`（`tako_run_resolve` の `refresh` と説明）/ `crates/tako-cli/src/main.rs`（`--list` の表示に候補）/ `.agent/requirements.md`（FR-3.18 の下に新 FR）
- 受け入れ:
  - 隔離 GUI の実経路（`scripts/test-runner-runtime-<番号>.sh`。`. scripts/lib/isolated-gui.sh`）: 一時プロジェクトに `python3 -m venv .venv` → `tako run a.py`（`import sys; print(sys.prefix)`）の出力が `.venv` を指す / venv が無ければシステム側を指す / `tako run a.py --list` と MCP `tako_run_resolve` の `runtimes` が字面一致
  - A/B: `TAKO_<S2 の Issue 番号>_LEGACY=1` で今の挙動（`python3` を PATH から）へ戻り、上の 1 項目目が FAILED になる
  - 組み込み表の `py` を実行環境なしで展開した結果が、今と両 OS でバイト一致
  - `RunResolve` が `prepare_offload` を通る（dispatch の単体テスト）。Tier P は `probe::output_with_timeout` だけ（#1503 の番犬が既に見る）
  - Tier F の所要を実測して PR に書く（目標 50 ms 以内。assert にはしない）
  - `tako context-budget` の `mcp_catalog` が予算内で、増分を PR に書く
- 依存: S1 / **#1656 の着地**（`runner.rs` と dispatch の `Run` / `RunResolve` が重なる）/ 規模: L

### S3: 実行設定を覚える（保存・実行環境とプロファイルの選択）

- 触る: 新設 `crates/tako-control/src/run_configs.rs`（読み書き・tmp + rename・上限 1,000 件）/ `crates/tako-core/src/migration.rs`（`SchemaId::RunConfigs`）/
  `crates/tako-control/src/migrations.rs`（SPECS / targets / validate）/ `crates/tako-control/src/config_share/catalog.rs`（`Class::Local`）/
  `crates/tako-control/src/protocol.rs` + `dispatch.rs`（`RunnerDefaults` の `path` / `scope` / `profile` / `runtime` / `reset`、`Run` での重ね、§5.5 のエラー）/
  `crates/tako-core/src/project_root.rs`（`detect` を #1656 の答えに揃える）/ `crates/tako-cli/src/main.rs`（`tako run-config` の表示・`--runtime` / `--profile` / `--reset` / `--project` / `--file`、`CLI_KEY_OVERRIDES`）/
  `crates/tako-control/src/mcp/catalog.rs` + `request.rs`（`tako_run_defaults`）/ テストのスナップショット（移行の指紋・MCP）/ `.agent/requirements.md` / `.agent/commands.md` / `AGENTS.md`（コマンド表に 1 行）
- 受け入れ:
  - `tako run-config a.py --runtime <候補>` → `tako run a.py` がその環境で走る（隔離実経路）/ `--reset runtime` で自動へ戻る
  - 保存した `.venv` を消すと `Run` がエラーになり、応答に `runtime_unresolved.next_step` が載る（自動へ落ちない）
  - 短縮 ID が 2 候補に当たると、候補を並べてエラー
  - `run-configs.json` の round-trip / 破損時の `.unreadable.bak`（機構側）/ `migration_registry` / `config_share_catalog` テストが緑
  - CLI と MCP の応答が字面一致（同じ dispatch）/ parity テスト（`run-config` → `tako_run_defaults`）緑
  - `tako context-budget` の増分を PR に書く
- 依存: S2 / **#1657 と #1662 の着地**（どちらも dispatch の `Run` を触る。直列）/ 規模: L

### S4: 引数・環境変数・作業ディレクトリ・実行前コマンド

- 触る: `crates/tako-core/src/platform/shell.rs` + `shell_dialect.rs`（`compose_run_script` の env / before を完成）/ `crates/tako-core/src/runner.rs`（`${args}` と末尾追加）/
  `crates/tako-core/src/platform/runner_defaults.rs`（Windows のコンパイル行へ `${args}`）/ `crates/tako-control/src/dispatch.rs`（`RunnerDefaults` の `args` / `env` / `cwd` / `before`）/
  `crates/tako-cli/src/main.rs`（`--args` / `--env` / `--unset-env` / `--cwd` / `--before`）/ `crates/tako-control/src/mcp/catalog.rs` + `request.rs`
- 受け入れ:
  - `compose_run_script` を POSIX / PowerShell の両方言で固定（引用に `'`・空白・日本語・`$`、PATH 前置、before の失敗で本体が走らない形）
  - 「`if ($?) {` を含む行は `${args}` を含む」の表テスト
  - 隔離実経路: `--env APP_ENV=dev` + `--args x` → 実行ペインの出力に両方が出る / `--before "false"` で本体が走らず、終了コードが 1
  - persist ON（tmux の器）でも env が届く（器ありの隔離実経路で 1 項目）
  - 診断ログ（persist.log / perf.log / stderr）に env の値・引数が出ない番犬
- 依存: S3（保存の口）/ 規模: M

### S5: ドロップダウン UI

- 触る: `crates/tako-app/src/preview_render.rs`（ヘッダ・メニュー・書き込み）/ `crates/tako-app/src/main.rs`（状態の入れ替え・`preview_run_selected` の廃止）/
  `crates/tako-app/src/sidebar.rs`（リロード時の取り直し）/ `crates/tako-core/src/header_layout.rs`（`run_runtime_label`）/ `crates/tako-app/src/ui_text/preview.rs` / `.agent/manual-checks.md`
- 受け入れ:
  - セルフテストに項目を足す: 宣言 2 つ + `.venv` の一時プロジェクトを開く → メニューの状態（`RunResolve` の応答と同じ候補・出典）を機械で読む → 実行環境の行を選ぶ操作を dispatch 経由で行い、`run-configs.json` と次の `Run` の応答に反映される
  - `header_layout` の閾値テスト
  - 描画から検出関数を呼んでいない番犬（`render_*` の本文に `runtime_env::` / `runtime_probe::` が現れたら落とす）
  - 仮想ディスプレイ上の隔離 GUI でスクショ（メニューの開閉・候補・TextField 編集・出典タグの切り替え）。目視項目は manual-checks.md へ
- 依存: S3（S4 の項目は、S4 が未着地なら行を出さない）/ #1657（`preview_render.rs` の完了バッジと重なる）/ 規模: L

### S6: 実行先（同じペイン / 新しいペイン）を覚える

- 触る: `crates/tako-core/src/runner_config.rs`（`target`）/ `crates/tako-control/src/dispatch.rs`（`Run` が #1657 の再利用へ `target` を渡す）/ CLI・MCP（`--target` / `target`）/ `preview_render.rs`（メニューの 1 行）/ 移行の指紋（`serde(default)` なので Step 無し = PR に明記）
- 受け入れ: `target=new` を保存すると再生ボタン 2 回で 2 枚、`reuse` なら 1 枚（隔離実経路）/ 一回限りの `--new-pane`（#1657）が保存値より勝つ
- 依存: #1657 / S5 / 規模: S

### S7: 他言語の行（Node / Rust）

- 触る: `crates/tako-core/src/runtime_env/kinds.rs`（行の追加）+ テスト。戦略が足りなければ `detect.rs` に 1 つ足す
- 受け入れ: §9.1 の検出を偽のファイルシステムで固定（両 OS の列）/ `.nvmrc` のあるプロジェクトで `tako run a.js` が nvm の版の node で走る（隔離実経路）
- 依存: S2（S3〜S6 とは並走できる。触るのが表とテストだけ）/ 規模: M

---

## 11. ファイルの重なりと着手順

| ファイル | S0 | S1 | S2 | S3 | S4 | S5 | S6 | S7 | #1656 | #1657 | #1662 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `tako-core/src/runner.rs` | | | ● | | ● | | | | ● | | |
| `tako-core/src/platform/runner_defaults.rs` | | | ● | | ● | | | | ○ | | |
| `tako-core/src/platform/shell.rs` / `shell_dialect.rs` | | | ● | | ● | | | | | ● | |
| `tako-core/src/runner_config.rs`（新） | | ● | | ● | | | ● | | | | |
| `tako-core/src/runtime_env/`（新） | | ● | ○ | | | | | ● | | | |
| `tako-core/src/project_root.rs`（#1656 が新設） | | | | ● | | | | | ● | | |
| `tako-core/src/migration.rs` / `tako-control/src/migrations.rs` / `config_share/catalog.rs` | | | | ● | | | ○ | | | | |
| `tako-control/src/protocol.rs` | | | ○ | ● | ● | | ● | | ○ | ● | ○ |
| `tako-control/src/dispatch.rs`（Run 系） | | | ● | ● | ● | | ● | | ● | ● | ● |
| `tako-control/src/runtime_probe.rs`（新）/ `run_configs.rs`（新） | | | ● | ● | | | | | | | |
| `tako-control/src/mcp/catalog.rs` / `request.rs` | | | ● | ● | ● | | ● | | ○ | ● | |
| `tako-cli/src/main.rs` | | | ○ | ● | ● | | ● | | | ● | ● |
| `tako-app/src/preview_render.rs` | ● | | | | | ● | ● | | | ● | ○ |
| `tako-app/src/main.rs` / `sidebar.rs` | | | | | | ● | | | | | |
| `tako-core/src/header_layout.rs` / `ui_text/preview.rs` | | | | | | ● | ○ | | | | |

（● = 主に触る / ○ = 少し触る。#1656 / #1657 / #1662 の列は各 Issue 本文の「レーン」節から）

着手順:

```
S0 ─────────────────────────────▶（いつでも。#1657 より前が望ましい）
S1 ─────────────────────────────▶（#1656 と並走）
#1656 ▶ S2 ▶ #1657 ▶ #1662 ▶ S3 ▶ S4 ▶ S5 ▶ S6
                 └────▶ S7（S2 の後、S3〜S6 と並走）
```

- dispatch の `Run` を触るもの（#1656 / S2 / #1657 / #1662 / S3 / S4 / S6）は**直列**。`progress.md` の衝突（#1228 / #1352）と同じく、並べると CI の後に CONFLICTING へ落ちる
- #1657 と #1662 を S2 と S3 の間に挟むのは、#1647 の D レーンの順（D1 → D2 → D3 → G2）を崩さないため。S2 が先に入っても #1657 の設計は変わらない（`Run` の中で使う値が 1 つ増えるだけ）
- [提案] #1656 の worker へ、次の 2 点を申し送る。①上へ辿る関数を `tako_core::project_root` に置き、純粋関数 `detect(file) -> Option<ProjectInfo { root, kind }>` を公開する。②`expand_variables` の文脈を構造体で受ける（`${workspaceRoot}` を足すついでに）。S1 / S2 がそれに乗れば、同じ関数を 2 回書かずに済む
- [提案] LSP の Python スライス（#1007 配下。pyright）の本文へ、「interpreter は `runner_config::effective_runtime` から受け取る」を 1 行足す

---

## 12. リスク・細部

| 項目 | 内容 | 対策 |
|---|---|---|
| uv の在否が冷えた状態で分からない | `platform::exe::find` は子プロセスなので `Run` の経路で呼べない。既知の置き場にも GUI の PATH にも無い uv は、Tier P が済むまで見えない | GUI はプレビューを開いた時点で Tier P を background で温める。CLI の初回だけ `.venv` へ落ちることがあり、そのときは応答の `runtime.tier` と `warnings` で分かるようにする |
| 既存の挙動が変わる | `.venv` のあるプロジェクトで、今は PATH の `python3` だったものが `.venv` の python になる | ほぼ常に望ましい変化だが、既定の変更なので §13 J1 で確認する。A/B の env（S2 の Issue 番号で切る）を残す |
| conda の activation が浅い | PATH 前置 + env だけでは `activate.d` のスクリプト（GDAL 等が env を足す）が走らない | §13 J3。既知の制限として docs に 1 行書く |
| uv の実行時の副作用 | `uv run` は同期のためにダウンロードと導入をしうる | uv 利用者には期待どおりの動き。初回は時間がかかるので、実行ペインにそのまま出す（隠さない） |
| 包む形と宣言の相性 | `tako:run: pytest` は `uv run` で包まれない | activation を併用できる場合は併用する。できないときは `warnings` で `${python} -m pytest` を案内する |
| rc ファイルが PATH を組み直す | 対話シェルの rc が pyenv の shim を前置する | コマンド文字列の先頭で前置する（rc の後に効く。§6.3） |
| リモートの PWA | 実行設定はスコープ外（旧設計 §5.3 と同じ） | 需要が出たら別 Issue |
| キーのパスがずれる | シンボリックリンク経由で開くとキーが変わる | 境界 B26 で正規化した表記に揃える（§4.5） |
| Windows の置き場 | conda の PATH 5 本・pyenv-win の root は macOS 上の表テストだけでは実物と一致するか分からない | 実機で測れた分だけ MATRIX の `windows_evidence` に書く（#591） |

---

## 13. 判断待ち（着手前に決めてほしいこと）

### J1. プロジェクトの `.venv` を、設定なしで自動で使うか

- **(a) 自動で使う（推奨）**: 何も設定しなくても `.venv` / uv / poetry で走る
- (b) ユーザーが選んだときだけ使う: 今の挙動（PATH の `python3`）を保つ

根拠: #322（機能追加は既定動作を賢くする方向で）と VS Code の自動選択。venv を作ったのに使わない方が、
「import できない」の原因として分かりにくい。A/B の逃げ道も残す。

### J2. uv のプロジェクトでの既定

- **(a) `uv.lock` があり `uv` が見つかれば `uv run` を既定にする（推奨）**
- (b) `.venv` があれば `.venv` を直接使う（`uv run` は候補に並べるだけ）

根拠: uv 自身の正規の走らせ方で、ロックへ同期してから走る。(b) だと、ロックに足した依存が入っていないまま走りうる。
代償は、初回や依存を変えた直後の実行が同期のぶん遅くなること。

### J3. conda の activation の深さ

- **(a) PATH の前置 + `CONDA_PREFIX` / `CONDA_DEFAULT_ENV`（推奨）**: 速い。conda 本体に依存しない。`activate.d` は走らない
- (b) `conda run --no-capture-output -p <prefix>` で包む: 完全な activation。conda 本体と起動の待ち（概ね 1 秒前後）が要る
- (c) シェルで `conda activate`: `conda init` 済みの前提が要り、Windows の PowerShell では設定次第で動かない

根拠: 大半の用途（python / pip / pytest の解決）は (a) で足りる。`activate.d` が要る人向けには、宣言で
`tako:run: conda run -n ml python ${fileBase}` と書く逃げ道がある。

### J4. 実行設定の保存先

- **(a) `<data_dir>/run-configs.json`（新設・マシンローカル・デバイス間で共有しない）（推奨）**
- (b) `settings.json` に相乗り: 実装は小さいが、#513 で他のデバイスへ同期され、マシン固有のパスが別の機械で壊れる
- (c) リポジトリ内の `.tako/run.json`（JetBrains の `.run/` 相当）: チームで共有できる。public リポへ個人の環境を混入させる経路になる

根拠: 共有したい実行定義は `tako:run` 宣言がすでに担っているので、足す層はマシンローカルだけでよい。
(c) は需要が出てから、(a) を読み込む元を 1 つ増やす形で足せる。

### J5. MCP のツールを増やすか

- **(a) 既存の `tako_run_resolve` / `tako_run_defaults` に引数を足す（推奨）**
- (b) 新ツール `tako_run_config` を 1 本足す

根拠: master の指示（予算を優先）と §8.3 の見積り。カタログの余白は LSP と取り合いになる（#1711 / #1723）。

---

## 14. 参照（着手時に読むもの）

- 旧設計（宣言・変数・信頼モデル）: `.agent/plans/2026-07-code-runner.md`
- FR: `.agent/requirements.md` の FR-3.18（新しい FR はその直後に番号を切る）
- 規約: `.agent/conventions.md` の「コマンド案内の規約（Issue #322）」「OS で変わる値は『両方の列を持つ 1 枚の表』にする」
  「外部コマンドを待つときは上限を持つ（Issue #1503）」「宛先が解けないものは既定へ落とさない（Issue #1466）」
  「CLI と MCP は『同じ 1 本』を通す」「設定・データファイルのスキーマ変更（Issue #916）」「MCP カタログの説明文は AI が使える情報だけ載せる」
  「MCP カタログの enum は正本から生成する」「アプリ内テキスト入力は `TextField` を通す」「`canonicalize` の結果を持ち回らない（Issue #970）」
- LSP の検出表とルート検出: `.agent/plans/2026-09-lsp-s1.md` §3 / §5
- 関連 Issue: #1656（プロジェクト検出）/ #1657（実行ペインの再利用）/ #1662（`--wait` の上限・実行前の保存）/ #1711・#1723（MCP カタログの予算）
