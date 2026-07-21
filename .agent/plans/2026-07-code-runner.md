# コード実行「再生ボタン」機能（Code Runner）詳細設計

- 作成日: 2026-07-21
- 種別: 実装前の詳細設計（読取専用調査に基づく。コード変更なし）
- 実装状況: 未実装（この設計を後続の実装 worker が Issue / PR 単位で実装する）
- 対象領域: tako-core / tako-control（dispatch・MCP）/ tako-cli / tako-app（プレビュー UI）
- 根拠の行番号はすべて 2026-07-21 時点の main（`cb964c3` 以降）の実測値。実装時に多少ズレても
  シンボル名で追跡できるよう、関数名・型名を併記している

## 0. 概要と設計方針

VSCode Code Runner 相当の機能を tako に載せる。プレビューペインで開いているファイルを
再生ボタンのワンクリックで実行し、**新しいペインを分割してそこでコマンドを実行**する。
実行コマンドは（優先順に）①ファイル先頭の tako 独自コメント宣言 → ②拡張子ごとの既定
コマンド（設定）→ ③CLI / MCP の明示 `--command` で解決する。cwd は常に
**そのファイルのあるディレクトリ**（宣言で上書き可）。

参照イメージ（ユーザー提示）: `~/Documents/campus-share/lectures/3s/ビジュアル情報処理/docs/start-docs.command`
— `#!/usr/bin/env bash` + `cd "$(dirname "$0")"` で自分の場所へ移動して npm dev サーバーを
立ち上げる「ダブルクリック実行ファイル」。この体験を、.command 専用ファイルを作らずに
**任意ファイルの先頭コメント宣言**で汎用化するのが本機能の狙い。

### 既存資産の再利用マップ（車輪の再発明をしない）

| 必要な部品 | 既存資産 | 根拠 |
|---|---|---|
| ペイン分割 + コマンド起動 + exit code 回収 | `Request::RunInteractive`（#305）の dispatch 実装。split_with_ratio → `attach_session` → `__TAKO_EXIT=$?` マーカー → `RunInteractiveStatus` で回収 | `crates/tako-control/src/protocol.rs:1011`, `crates/tako-control/src/dispatch.rs:3094-3230` |
| シェル経由実行（PATH 問題の解決込み） | `SpawnCommand` を `$SHELL -l -c "<cmd>"` に包む `login_shell_command` | `crates/tako-core/src/terminal.rs:99-112,162-179` |
| 相対パス解決（ペイン cwd 基準） | `Request::OpenFile` の解決ロジック | `crates/tako-control/src/dispatch.rs:1310-1319` |
| 拡張子 → 表示モード判定の前例 | OpenFile の mode 判定 / `is_markdown_path` | `crates/tako-control/src/dispatch.rs:1326-1348`, `crates/tako-app/src/preview.rs:421-439` |
| 設定の永続化 | `settings.json`（`#[serde(default)]` 後方互換 + tmp/rename 書き込み） | `crates/tako-control/src/settings.rs:11-59,133-171` |
| プレビューヘッダのボタン列 | `render_preview_pane` のモードトグル・履歴・編集ボタン群 | `crates/tako-app/src/preview_render.rs:883,1846,2177-2342` |
| ヘッダの幅適応表示 | `PreviewHeaderVisibility::from_width` | `crates/tako-core/src/header_layout.rs:56-80` |
| ドロップダウンメニュー | limit_service メニュー（anchor 記録 + ルートオーバーレイ + 背面 dismiss。#321/#361） | `crates/tako-app/src/status_bar.rs:447-451,1368-1530` |
| SVG アイコン | `assets/icons/ui/*.svg` + `ui_icon` 定数 + `svg().path(...)` | `crates/tako-app/src/file_icons.rs:152-189` |
| UI からの操作実行 | UI ハンドラが `tako_control::dispatch(...)` を直接呼ぶ既存パターン | `crates/tako-app/src/main.rs:3097,3409,7123` |
| MCP ツール定義・変換・未知パラメータ検査 | `tools()` カタログ + `call_tool` + `validate_known_params` | `crates/tako-control/src/mcp.rs:146,2413-2421,3341` |
| CLI サブコマンド | clap derive（`run-interactive` の前例） | `crates/tako-cli/src/main.rs:240-279,4499` |
| MCP ツール一覧の回帰検査 | セルフテスト 32 のスナップショット検証（`TAKO_UPDATE_SNAPSHOT=1` で再生成） | `crates/tako-app/src/main.rs:16219-16267`, `crates/tako-app/testdata/mcp_tools_snapshot.txt` |

### 方針の要点

1. **実行経路は RunInteractive と共通化する**。split → spawn → exit マーカー → auto_close の
   セマンティクスは実装済み・実機検証済み（#305）。本機能は「コマンドをどう決めるか
   （宣言パース + 既定マップ + 変数展開）」のレイヤを足すだけにする
2. 宣言パーサ・解決器・変数展開は **tako-core::runner** に純関数として新設（GPUI 非依存・
   ユニットテスト可能）。dispatch はそれを呼ぶだけ。開発不変条件（AGENTS.md「機能実装時の
   必須ルール」）どおり dispatch / CLI / MCP に 1:1 で公開する
3. 要件番号は **FR-3.18** を新設する（FR-3 系の現行末尾は FR-3.17。
   `.agent/requirements.md:598` 付近）。実装時に requirements.md へ追記すること
4. 過剰設計をしない: 複数行スクリプト DSL・実行履歴 DB・確認ダイアログ・リモート PWA 対応は
   作らない（8 章参照）

---

## 1. tako 独自コメント仕様（最重要）

### 1.1 ディレクティブ行の文法

ファイル内の 1 行に `tako:` マーカーで書く。**各言語のコメント記法に依存しない**:
パーサはコメント記号を解釈せず、「行内の `tako:` 出現位置」だけを見る。

```
<接頭辞> tako:<キー>[<プロファイル名>]: <値> <行末クローザ>
```

- **接頭辞**: 行頭から `tako:` までの任意文字列。ただし次の 2 条件で誤検知を防ぐ:
  - 長さ 16 文字以内（`#`, `//`, `;`, `--`, `%`, `*`, `"`, `<!--`, `/*`, `REM` + 空白を想定）
  - `tako:` の直前が行頭・空白・非英数字のいずれか（`mytako:run` を拾わない）
- **行末クローザ**: 値の末尾が `-->` / `*/` / `#}` / `--}}` で終わる場合はそれを除去して
  trim する（HTML / CSS / Jinja 等のブロックコメント対応）
- **キー**: `run` / `cwd` / `shell` の 3 種のみ
- **プロファイル名** `[name]`: 省略可。`[A-Za-z0-9_-]{1,32}`。省略時は既定プロファイル
  （内部名 `default`、UI 表示は「実行」）
- **値**: `:` の後ろから行末（クローザ除去後）まで。前後 trim
- 大文字小文字: キーは小文字固定。`TAKO:RUN` は拾わない（grep しやすさと仕様の単純さ優先）

### 1.2 スキャン範囲（「先頭限定か、どこでも可か」の決定）

**ファイル先頭 64 行、かつ先頭 16 KiB まで**をスキャンし、それ以降の `tako:` 行は無視する。

- 根拠: shebang・ライセンスヘッダ・doc コメントの下に書ける余裕（64 行）を持たせつつ、
  ファイル中腹のコード内文字列リテラル（例: `print("tako:run: rm -rf /")`）を実行宣言と
  誤認する事故を構造的に排除する。全文スキャンは巨大ファイルでのコストも問題になる
  （プレビューは 1MB / 5000 行制限を既に持つ。`crates/tako-app/src/preview.rs:20-21`）
- ユーザー要件の「特定フォーマットなら先頭以外でも可」は、この 64 行ウィンドウ内で
  「先頭行でなくてもよい」ことで満たす（マーカー方式で末尾等も許す案は誤検知リスクが
  利便を上回るため不採用。理由ごと本設計で確定）

### 1.3 キーの意味

| キー | 意味 | 既定値（宣言なし時） |
|---|---|---|
| `tako:run: <command>` | 既定プロファイルの実行コマンド | —（2 章の拡張子既定へフォールバック） |
| `tako:run[name]: <command>` | 名前付きプロファイルの実行コマンド | — |
| `tako:cwd: <dir>` | 全プロファイル共通の作業ディレクトリ。相対パスは**ファイルのあるディレクトリ基準**で解決 | ファイルのあるディレクトリ（要件 3。参照 .command の `cd "$(dirname "$0")"` と同じ意味論） |
| `tako:cwd[name]: <dir>` | プロファイル別の作業ディレクトリ（共通指定より優先） | `tako:cwd:` → ファイルのディレクトリ |
| `tako:shell: <shell>` | コマンド文字列を解釈するシェル（`bash` / `zsh` / `fish` 等、PATH 解決可能な名前 or 絶対パス） | ユーザーのログインシェル（`login_shell_command` の既定経路。`crates/tako-core/src/terminal.rs:99-112`） |
| `tako:shell[name]: <shell>` | プロファイル別シェル | 同上 |

- コマンドは **1 行 1 コマンド**。複数ステップは `&&` / `;` で連結する。複数行継続構文は
  設けない（複雑な手順はスクリプトファイルに書き、`tako:run: bash setup.command` の形で
  呼ぶのが正。過剰 DSL 化の回避）
- 同名プロファイルの重複宣言は**後勝ち**（上書き）。解決 API（5 章 RunResolve）の応答に
  `warnings` として重複を報告する
- プロファイルの表示順・既定選択: 宣言の出現順を保持し、ドロップダウンはその順で並べる。
  再生ボタン単押しの対象は「無添字 `tako:run:` があればそれ、無ければ最初に宣言された
  プロファイル」

### 1.4 変数展開

コマンド・cwd の値の中で以下を展開する。**展開値はシングルクオートで自動エスケープ**する
（`'` は `'\''` に置換）。空白・日本語を含むパスでもそのまま動く。

| 変数 | 展開値 | 例（`/Users/a/src/main tool.c` の場合） |
|---|---|---|
| `${file}` | ファイルの絶対パス | `'/Users/a/src/main tool.c'` |
| `${fileDir}` | ファイルのあるディレクトリの絶対パス | `'/Users/a/src'` |
| `${fileBase}` | ファイル名（拡張子付き） | `'main tool.c'` |
| `${fileNoExt}` | ファイル名（拡張子なし） | `'main tool'` |
| `${ext}` | 拡張子（ドットなし・小文字） | `'c'` |

- 未知の `${...}` は展開せずそのまま残す（エラーにしない。シェルの `${VAR}` と共存させるため）
- cwd はファイルのディレクトリで実行されるため、コマンド内では通常 `${fileBase}` /
  `${fileNoExt}` だけで足りる（`${file}` はフルパスが要るツール向け）

### 1.5 各言語の具体例

```bash
#!/usr/bin/env bash
# tako:run: bash ${fileBase}
# （.command: 拡張子既定にもあるので、宣言なしでもこのコマンドで実行される）
```

```c
// main.c — 単一コマンド形式
// tako:run: cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}
#include <stdio.h>
```

```c
/* main.c — 複数プロファイル + プロファイル別 cwd（ビルドはリポジトリルート） */
/* tako:run[build]: make */
/* tako:cwd[build]: ../.. */
/* tako:run[test]: make test */
/* tako:run: cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt} */
```

```tex
% report.tex
% tako:run: latexmk -pdf -interaction=nonstopmode ${fileBase}
\documentclass{article}
```

```python
#!/usr/bin/env python3
# tako:run: python3 ${fileBase}
# tako:run[test]: python3 -m pytest ${fileBase} -v
```

```javascript
// server.js — dev サーバー（参照 .command の汎用化例）
// tako:run: npm run dev
// tako:run[install]: npm install
```

```rust
// tool.rs — 単ファイル / cargo プロジェクトの両プロファイル
// tako:run: rustc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}
// tako:run[cargo]: cargo run
// tako:cwd[cargo]: ..
```

```html
<!-- index.html -->
<!-- tako:run: open ${fileBase} -->
<!-- tako:run[serve]: python3 -m http.server 8000 -->
```

### 1.6 解決の優先順位（全体）

1. CLI / MCP の明示 `command` オーバーライド（`tako run <file> --command "..."`）
2. ファイル内宣言（`tako:run` 系。プロファイル指定があればそのプロファイル）
3. 拡張子既定マップ（2 章。settings.json のユーザー定義 → 組み込み既定の順）
4. どれも無ければ**エラー**（メッセージに宣言の書き方 1 行例と
   `tako run-default <ext> "<command>"` を含める）

プロファイル名を明示指定したのに宣言に存在しない場合は 3 へフォールバックせずエラー
（打ち間違いを黙って別コマンドで実行しない）。

---

## 2. 拡張子デフォルト設定

### 2.1 置き場所: settings.json

`<data_dir>/settings.json`（`crates/tako-control/src/settings.rs:133-135` の
`settings_path`）に載せる。config.yaml（orchestrator 系）ではなく settings.json を選ぶ理由:
テーマ・ライブリロード等と同じ「GUI 挙動のユーザー設定」であり、既存の
`#[serde(default)]` 後方互換 + tmp/rename 書き込み（`settings.rs:149-171`）が
そのまま使えるため。並行書き込み保護が必要な YAML 系（config_io.rs。#169）は
オーケストレーター設定用で、ここでは過剰。

```rust
// settings.rs への追加（Settings struct。settings.rs:11-59 に倣い #[serde(default)]）
/// 拡張子ごとの実行コマンド既定（FR-3.18。キーは小文字拡張子・ドットなし。
/// 値は 1.4 の変数展開が効くコマンドテンプレート。組み込み既定を上書き・追加する）
#[serde(default)]
pub runner_defaults: std::collections::BTreeMap<String, String>,
```

- ユーザー定義（settings.json）→ 組み込み既定（下表。tako-core::runner の定数テーブル）の
  順で引く。settings.json に同じ拡張子があれば組み込みを上書きする
- 値に空文字列を設定した拡張子は「無効化」（組み込み既定も使わない）

### 2.2 組み込み既定マップ（tako-core::runner に定数として実装）

| 拡張子 | 既定コマンド |
|---|---|
| `command`, `sh` | `bash ${fileBase}` |
| `bash` | `bash ${fileBase}` |
| `zsh` | `zsh ${fileBase}` |
| `py` | `python3 ${fileBase}` |
| `js`, `mjs` | `node ${fileBase}` |
| `ts` | `npx tsx ${fileBase}` |
| `rb` | `ruby ${fileBase}` |
| `pl` | `perl ${fileBase}` |
| `php` | `php ${fileBase}` |
| `lua` | `lua ${fileBase}` |
| `c` | `cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}` |
| `cpp`, `cc`, `cxx` | `c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}` |
| `rs` | `rustc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}` |
| `go` | `go run ${fileBase}` |
| `java` | `java ${fileBase}` |
| `swift` | `swift ${fileBase}` |
| `tex` | `latexmk -pdf -interaction=nonstopmode ${fileBase}` |

- `.command` はユーザー要件どおり「そのまま実行」。実行ビットに依存しない
  `bash ${fileBase}` を既定にする（Terminal.app のダブルクリック挙動の近似。
  zsh shebang 等は shebang どおりに動かしたければ宣言 `tako:shell` / `tako:run` で上書き）
- このテーブルは組み込みの初期値にすぎない。方針（`.agent/conventions.md:37` の
  「機能追加は既定動作を賢くする方向で」）に沿って、実装後の追加はコード側テーブルを
  育てる（ユーザーへの案内は「settings に足して」より「ファイルに tako:run を書いて」を優先）

### 2.3 フォールバック挙動の決定

宣言なし時の挙動は「**拡張子既定をそのまま実行**」で確定（確認ダイアログは設けない）。

- 再生ボタンのクリック自体が明示の実行意思であり、実行されるコマンドはクリック前に
  ボタンのツールチップ（4 章）と `tako run <file> --dry-run`（5 章）で確認できる
- 宣言・既定の両方が無い場合は「何もしない」= UI はボタン無効表示、CLI / MCP はエラー
- 「確認モード」は作らない（tako の既存の任意コマンド実行系 `Split { command }` /
  `RunInteractive` / `Send` と同じ信頼モデル。8 章参照)

---

## 3. 実行フロー

### 3.1 全体シーケンス

```
再生ボタン / tako run / tako_run
  → dispatch Request::Run { path, profile, command, pane, tab, direction, ratio, auto_close, focus }
    1. パス解決: 相対パスは対象ペインの cwd（OSC 7）基準 + canonicalize + is_file 検査
       （OpenFile と同一実装。dispatch.rs:1310-1325 を関数化して共用）
    2. 宣言スキャン: ファイル先頭 64 行 / 16KiB を読み tako-core::runner::parse_declarations
    3. コマンド解決: runner::resolve（1.6 の優先順位。変数展開・シェルエスケープ込み）
    4. cwd 解決: プロファイル cwd → 共通 cwd → ファイルの親ディレクトリ。
       存在検査（is_dir）に失敗したらエラー（黙ってホームで実行しない）
    5. ペイン起動: RunInteractive と同じ split + attach 経路（3.2）
    6. 応答: { pane, path, profile, command, cwd, auto_close } を返す
```

### 3.2 ペイン起動は RunInteractive の経路を共通化

`Request::RunInteractive` の実装（`dispatch.rs:3094-3179`）は本機能が必要とする
「split_with_ratio で分割（:3127-3134）→ コマンドをシェル -c で包んで
`attach_session`（:3148-3158。`SpawnCommand { program: wrapped, args: [] }` が
`login_shell_command` で `$SHELL -l -c` になる）→ `__TAKO_EXIT=$?` マーカー + `read` で
PTY 生存（:3145-3147）→ タイトル + interactive_meta 設定（:3163-3172）」を既に持つ。

実装は、この本体を private ヘルパー（例: `spawn_command_pane(host, origin, base, tab,
direction, ratio, cwd, command, title, auto_close)`）へ抽出し、`RunInteractive` と新設
`Run` の両方から呼ぶ。**RunInteractive の外部挙動は変えない**（応答 JSON・タイトル書式
`(!) hint`・focus 移動・auto_close 既定 "success" は現状維持）。

`Run` 側の差分:

- **cwd**: 分割元ペインの cwd 継承（RunInteractive :3136-3140）ではなく、3.1-4 で解決した
  ディレクトリを `SpawnOptions.cwd` に明示指定する（`Split` の cwd 明示指定と同型。
  `dispatch.rs:405-419`）
- **シェル**: `tako:shell` 指定時は、解決済みコマンド全体を `<shell> -c '<エスケープ済み>'`
  に包んでから既存の wrapped 形へ渡す（無指定なら従来どおりログインシェルが直接解釈）
- **タイトル**: `(>) <fileBase>`（既定プロファイル）/ `(>) <fileBase> [<profile>]`。
  role は設定しない（orchestrator の role 検索と干渉させない。
  `dispatch.rs:436-438` の worker 保護は role ベースなので無関係を保つ）
- **auto_close 既定は "never"**（RunInteractive は "success"）。Code Runner の目的は実行
  結果の閲覧であり、成功で即消えると出力が読めない。`--auto-close success` の明示指定は可
- **focus 既定は false**（Split / OpenFile の「ユーザーの入力を奪わない」原則。
  `dispatch.rs:421-424,1374-1379`。UI の再生ボタン経由では `focus: true` を渡し、
  実行結果をすぐスクロールできるようにする）
- **direction / ratio 既定**: `down` / `0.3`（プレビューの下に出力が出るのが Code Runner の
  自然な形。CLI / MCP からは right 等も指定可）

### 3.3 完了検知・再実行

- exit code の回収は既存 `Request::RunInteractiveStatus`（`dispatch.rs:3181-3230`。
  `__TAKO_EXIT` マーカー検出 + auto_close 処理）を**そのまま共用**する。新設しない
- 再実行は常に新ペイン。既存 run ペインの掃除は auto_close 指定またはユーザー / AI の
  close に任せる（同一ファイルの実行中ペイン置き換えは v1 では作らない。ペイン ID の
  安定性・分割位置の再現など複雑さに対し要件に無い）

### 3.4 tako-core::runner の API（新設モジュール）

```rust
// crates/tako-core/src/runner.rs（GPUI 非依存・純関数。lib.rs へ pub mod 追加）

/// 1 プロファイル分の解決済み実行計画
pub struct RunPlan {
    pub profile: String,        // "default" or 宣言名
    pub command: String,        // 変数展開済み・エスケープ済みのシェルコマンド文字列
    pub cwd: PathBuf,           // 検証前の解決値（存在検査は呼び出し側）
    pub shell: Option<String>,  // tako:shell 指定（None = ログインシェル）
    pub source: RunSource,      // Declaration / ExtensionDefault / Override
}

/// ファイル先頭テキストから宣言を抽出（64 行 / 16KiB ウィンドウは呼び出し側で切る）
pub fn parse_declarations(head: &str) -> Declarations;  // profiles 順序保持 + warnings

/// 宣言 + 拡張子既定 + オーバーライドから RunPlan 一覧（ドロップダウン用）と
/// 指定プロファイルの 1 件を解決する
pub fn resolve(
    path: &Path,
    head: &str,
    ext_defaults: &BTreeMap<String, String>,  // settings 由来（組み込みマージ済み）
    profile: Option<&str>,
    command_override: Option<&str>,
) -> Result<Resolution, RunnerError>;

/// 変数展開（1.4 の 5 変数 + シングルクオートエスケープ）
pub fn expand_variables(template: &str, path: &Path) -> String;

/// 組み込み拡張子既定（2.2 の表）
pub fn builtin_defaults() -> &'static [(&'static str, &'static str)];
```

ユニットテスト必須項目: 接頭辞バリエーション（`#` `//` `%` `--` `;` `<!--` + クローザ）、
誤検知拒否（`mytako:run` / 65 行目 / 文字列リテラル位置は範囲外）、プロファイル順序・
後勝ち・warnings、変数展開のエスケープ（空白・`'`・日本語パス）、CRLF / BOM 許容、
優先順位（override > 宣言 > 既定 > エラー）、profile 明示 + 宣言なし = エラー。

---

## 4. UI（再生ボタン + プロファイルドロップダウン）

### 4.1 設置場所: プレビューペインのヘッダ

「ファイルを開いているペイン」= tako ではプレビューペイン（FR-3.2 / FR-3.5 の
コード表示・編集ビュー）と解釈する。ターミナルペインは「ファイルを開いている」状態を
持たないため対象外（ターミナルからは CLI `tako run` を使う）。

- 描画位置: `render_preview_pane`（`crates/tako-app/src/preview_render.rs:883`）の
  ヘッダボタン列。既存の並び（ズーム群 → モードトグル :2177 → 履歴 :2223 → 編集 :2266 →
  保存 :2318 → 閉じる :2344）の**「履歴」の左**に挿入する（実行は最も使う操作なので
  ボタン列の先頭側）
- 表示制御: `PreviewHeaderVisibility`（`crates/tako-core/src/header_layout.rs:56-80`）に
  `run_button: bool` を追加し、`from_width` で `width >= 250.0` から表示
  （edit_button の 300 より優先度高 = 狭くても残す。既存テスト
  `preview_header_progressive` :308-323 に加筆）
- 対象モード: Code / Markdown のみ表示（Image / Pdf / Video には出さない。
  `PreviewMode` は `crates/tako-app/src/preview.rs:25-31`）

### 4.2 ボタンの状態と見た目

- **実行可能**（宣言あり or 拡張子既定あり）: 再生アイコン + （プロファイルが 2 つ以上
  検出された場合のみ）選択中プロファイル名 + 下向きシェブロン。アイコン色は
  `theme.green`（実行系。履歴トグルの active 色と同系。`preview_render.rs:2234-2238`）
- **実行不可**: 淡色（`theme.text_faint`）+ hover ツールチップ
  「実行コマンド未定義。ファイル先頭に `tako:run: <コマンド>` を書くか
  `tako run-default` で拡張子既定を設定」
- ツールチップ（実行可能時）: 解決済みコマンド文字列（60 字で省略）。クリック前に
  何が走るか見える = 2.3 の「確認ダイアログを設けない」根拠
- **アイコン**: 絵文字は使わない（#217 で UI の絵文字は全廃済み）。`assets/icons/ui/play.svg`
  を新設し `ui_icon::PLAY` 定数（`crates/tako-app/src/file_icons.rs:152-189` の一覧に追加)
  + `svg().path(...)` で描画する。中身は**塗りつぶし三角形 1 パス**（`<path d="M4 2 L12 8 L4 14 Z"/>`
  相当。#438 のリロードアイコンと同じく、12px 実寸での見え方を実装時に確認する）。
  タスク指示の「GPUI 描画プリミティブで三角形を描く」は、この SVG パス方式で実現する
  （GPUI の svg() は SVG パスを GPU で直接描画する。canvas プリミティブで手描きするより
  #217 以降の SVG アセット慣行に一致し、全 39 個の既存 UI アイコンと管理が揃う）

### 4.3 プロファイルドロップダウン

limit_service メニューの実装パターンを踏襲する（`crates/tako-app/src/status_bar.rs`:
クリックで `menu_open` トグル + `menu_anchor = Some(event.position)` :447-451、
`render_limit_service_overlay` :1368-1530 がルートオーバーレイに anchor 位置で
メニューを描き、行クリックで選択 + 閉じ、背面クリックで dismiss）。

- TakoApp に `preview_run_menu: Option<(PaneId, gpui::Point<Pixels>)>`（開いているペインと
  anchor）と `preview_run_profile: HashMap<PaneId, String>`（ペインごとの選択記憶）を追加
- メニュー行: プロファイル名 + コマンド先頭 40 字（淡色）。クリックで
  `preview_run_profile` を更新して**即実行**
- シェブロン部クリックでメニュー開閉、本体クリックで選択中プロファイルを実行
  （VSCode の split button と同型）
- ペインを閉じたら両状態を掃除（`preview_edits` 等の既存 close cleanup と同じ場所）

### 4.4 クリック → 実行の経路

ボタンの on_click は `Request::Run { path, profile, pane: Some(プレビューペイン),
direction: Some(Down), focus: Some(true), .. }` を組んで
`tako_control::dispatch(self, request, origin)` を直接呼ぶ（既存パターン:
`crates/tako-app/src/main.rs:3097,3409,7123`）。UI / CLI / MCP が完全に同一経路を通る。

- 分割元 = プレビューペイン自身（実行ペインはプレビューの直下に生える）
- **dirty バッファの扱い**: 編集モードで dirty（`preview_edit_state`。
  `crates/tako-control/src/host.rs:288-290`）なら、実行前に `save_preview_local`
  （保存ボタンと同じ処理。`preview_render.rs:2335-2338`）を呼んでから dispatch する。
  保存失敗（外部変更検知等）ならエラーメッセージを出して実行しない
  （ディスク上の旧内容が黙って走る事故の防止）
- エラー（解決不可・cwd 不在等）は編集エラーと同じメッセージ表示枠へ出す
- キーバインド（任意・M4 のオプション）: `cmd-r` は未割当
  （`crates/tako-app/src/keybindings.rs` に cmd-r の既存バインドなし）。
  フォーカス中ペインがプレビューなら選択中プロファイルを実行、に割り当ててよい

---

## 5. MCP / CLI / dispatch 1:1（開発不変条件）

### 5.1 追加する操作の 1:1 表

| tako-core 操作 API | dispatch Request | CLI | MCP ツール | 意味 |
|---|---|---|---|---|
| `runner::resolve` + 共通 spawn ヘルパー（3.2） | `Run { path, profile, command, pane, tab, direction, ratio, auto_close, focus }` | `tako run <file> [profile]`（`--command` / `--down`（既定）/ `--right` / `--ratio` / `--auto-close` / `--focus` / `--wait`） | `tako_run` | 解決して新ペインで実行。応答 `{ pane, path, profile, command, cwd, auto_close }` |
| `runner::resolve`（実行なし） | `RunResolve { path, pane }` | `tako run <file> --list` / `--dry-run` | `tako_run_resolve` | 検出プロファイル一覧 + 各解決結果（command / cwd / source / warnings）。ドロップダウンと同じデータ |
| settings `runner_defaults`（2.1） | `RunnerDefaults { ext, command, remove }` | `tako run-default [ext] [command]`（引数なし = 一覧、`--remove` で削除） | `tako_run_defaults` | 拡張子既定の一覧 / 設定 / 削除。応答に組み込み既定とユーザー上書きを区別して含める |
| （既存を共用・新設なし） | `RunInteractiveStatus`（既存） | `tako run-interactive-status` / `tako run <file> --wait` | `tako_run_interactive_status`（既存） | exit code 回収 + auto_close 処理。`tako_run` の description に「完了確認はこれ」と明記 |

- CLI 形は conventions のコマンド案内規約（`.agent/conventions.md:37-55`、#322）に従い
  最簡形を正とする: 標準案内は `tako run main.c`。`--` オプションは上級者レイヤ
- `--wait` は run-interactive の合成実装（`crates/tako-cli/src/main.rs:4499-4505`）と
  同じポーリングを共用する
- Run / RunResolve の `pane` は相対パス解決の基準（呼び出し元既定。FR-2.2.7 の既存規約）

### 5.2 MCP 実装のチェックリスト

- `tools()` カタログへ 3 ツール追加（`crates/tako-control/src/mcp.rs:146` 以降。
  定義の書式は `tako_run_interactive` :2346-2390 に倣う）
- `call_tool` の name → Request 変換を追加（:3341 付近の `tako_run_interactive` に倣う）
- `validate_known_params` のパラメータ表へ 3 ツール分を追加（:2421。未知パラメータ検出）
- **`tako_run` の description に宣言の書き方形式を記述する**（ユーザー要件 7 前半の充足点）。
  含める内容: `tako:run:` / `tako:run[name]:` / `tako:cwd:` / `tako:shell:` の 1 行文法、
  スキャン範囲（先頭 64 行）、変数 5 種、解決優先順位。モデルがツール一覧を見るだけで
  正しいヘッダを書ける状態にする
- セルフテスト 32 のスナップショット更新: `TAKO_UPDATE_SNAPSHOT=1 cargo run -p tako-app`
  で `crates/tako-app/testdata/mcp_tools_snapshot.txt` を再生成
  （検証実装は `crates/tako-app/src/main.rs:16219-16267`。現行 105 ツール → 108 になる）

### 5.3 リモート API

スコープ外（8 章）。remote の v2 panes API にはボタン・エンドポイントを追加しない。

---

## 6. 生成時ヘッダ付与（tako がファイルを作るときの運用）

tako 本体がファイル内容を生成する経路は `FileOpKind::CreateFile`（空ファイル作成。
`crates/tako-control/src/dispatch.rs:1740`）のみで、実内容の生成は tako 内で動く
エージェント（claude 等）が行う。したがって「生成時に実行ヘッダを書く」はコードではなく
**エージェントへの行動規範**として実装する。落とし所は次の 3 点（すべて文言追加のみ）:

1. **MCP サーバー instructions**（`crates/tako-control/src/mcp.rs:117-138` の
   `INSTRUCTIONS` 定数。initialize で全エージェントに配られる行動規範）へ 1 項目追加:
   「実行可能なファイル（スクリプト・ビルド対象・.command 等）を新規作成したら、
   先頭コメントに `tako:run: <実行コマンド>` を書いておく。ユーザーが再生ボタン一発で
   実行できるようになる（書式の詳細は tako_run ツールの説明）」
2. **master / solo の system prompt**（`crates/tako-control/src/orchestrator/mod.rs:19-22`
   が include する `default_system_prompt.md` / `solo_system_prompt.md`）の行動規範へ
   同趣旨を 1 項目追加（worker への波及は Worker Prompt Template 節があるため master 経由で届く）
3. **`tako_run` / `tako_run_resolve` の description**（5.2）に書式を持たせ、エージェントが
   ヘッダを書く時の参照先にする

セルフテストの instructions 検証（`crates/tako-control/src/mcp.rs:3719-3731` の
initialize テストが特定文言を assert）に追随テストを 1 本足す。
`tako setup` 配布物（CLAUDE.md セクション）への追加は任意の後続タスクとする
（`resources/setup/changes.yaml` の revision 追加が必要になるため、本機能のスコープからは外す）。

---

## 7. 実装マイルストーン分割（実装 worker 向け）

順序どおりに積む。各マイルストーンは単独でビルド・テスト緑を保つ。

### M1: 宣言パーサ + 解決器（tako-core::runner）

- `crates/tako-core/src/runner.rs` 新設（3.4 の API + 2.2 の組み込みテーブル）。UI 非依存
- 受け入れ条件（機械検証）:
  - `cargo test -p tako-core runner` が 3.4 記載のテスト項目（接頭辞 6 種以上・誤検知 3 種・
    後勝ち・順序・変数展開エスケープ・優先順位・CRLF/BOM）を含んで全緑
  - `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` 緑

### M2: dispatch Run / RunResolve + spawn 経路の共通化

- `RunInteractive` 本体から `spawn_command_pane` ヘルパーを抽出（挙動不変）し、
  `Request::Run` / `Request::RunResolve` を protocol.rs + dispatch.rs に追加
- 受け入れ条件:
  - dispatch のユニットテストで: 宣言ファイル → Run が正しい command / cwd で
    `attach_session` を受けること（既存の RunInteractive テスト
    `dispatch.rs:9731-9846` と同じ TestHost パターン）、cwd 不在エラー、
    profile 不一致エラー、override 優先、を検証して全緑
  - RunInteractive の既存テストが**無変更で**全緑（共通化の非破壊証明）
  - `cargo test --workspace` 全緑

### M3: 拡張子既定設定（settings + RunnerDefaults）

- `Settings.runner_defaults` 追加（2.1）+ `Request::RunnerDefaults` + 解決時マージ
- 受け入れ条件:
  - settings roundtrip テスト（`settings.rs:184-208` に倣う）+ 空オブジェクト後方互換
    テスト（`{}` パースで既定 map 空）緑
  - dispatch テスト: set → resolve に反映 / remove → 組み込みへ戻る / 空文字列 = 無効化

### M4: UI（再生ボタン + ドロップダウン）

- `PreviewHeaderVisibility.run_button` + `play.svg` + `ui_icon::PLAY` + ヘッダボタン +
  オーバーレイメニュー + dirty 時保存 + （任意）cmd-r
- 受け入れ条件:
  - header_layout のユニットテスト（run_button の閾値）緑
  - セルフテスト（`TAKO_SELF_TEST=1`）に項目追加: 宣言つきファイルを OpenFile →
    Run dispatch 相当を UI 経路（ボタンクリックのシミュレート or dispatch 直呼び）で
    実行し、新ペインの画面に実行結果と `__TAKO_EXIT=0` が現れること
  - 隔離実機（`TAKO_ISOLATED=1`）でボタン表示 / 無効表示 / DD 選択のスクリーンショット確認
    （manual-checks.md に目視項目を追記）

### M5: CLI / MCP 1:1

- clap サブコマンド `run`（既存 `run-interactive` :240-273 に倣う）+ `run-default` +
  MCP 3 ツール + validate_known_params + スナップショット再生成
- 受け入れ条件:
  - CLI parse テスト（`main.rs:5231-5243` の parse テストに倣う）緑
  - mcp.rs のツール呼び出しテスト（`tako_open_file` の :3765-3828 に倣い、
    tako_run → Request::Run 変換 / 未知パラメータ 400）緑
  - セルフテスト 32（tools/list スナップショット）緑（108 ツール）
  - 実バイナリ e2e: 隔離起動 + `tako run <宣言つき一時ファイル> --wait` が exit 0 を返す /
    `--list` がプロファイル一覧 JSON を返す / `tako run-default c "..."` が settings.json に
    反映される

### M6: 生成時ヘッダ規範 + ドキュメント

- INSTRUCTIONS / master・solo prompt への追記（6 章）+ AGENTS.md コマンド表 +
  requirements.md FR-3.18 + docs（CLI リファレンス・MCP ツール一覧）
- 受け入れ条件:
  - mcp.rs の initialize テストに追随 assert（6 章）緑
  - `.agent/requirements.md` に FR-3.18 が 1:1 公開（ツール名・コマンド名）込みで記載
  - AGENTS.md「コマンド」表に `tako run` 行が追加されている

---

## 8. エッジケース・リスク

### 8.1 任意コマンド実行の信頼モデル（明記事項）

本機能は「ユーザーのローカルファイルに書かれたコマンドを、ユーザーの明示操作
（ボタンクリック / CLI / AI のツール呼び出し）で実行する」ものであり、
**ファイル内容を信頼する前提**に立つ。これは Make / npm scripts / VSCode Code Runner /
既存の `tako split -- <command>`・`tako_run_interactive` と同じモデルである。

- 「開いただけで実行」は決してしない（宣言のパースは読み取りのみ。実行は明示操作のみ）
- 実行されるコマンドは実行前に可視（ボタンツールチップ / `--dry-run` / `tako_run_resolve`）
- 信頼できないダウンロードファイルを開いて再生ボタンを押す事故は残余リスクとして受容
  （確認ダイアログは 2.3 の決定どおり設けない。docs の注意書きに 1 行明記する）
- 診断ログ規約: 解決済みコマンド文字列はユーザーデータであり、persist.log / perf.log /
  stderr に出さない（AGENTS.md 絶対ルール準拠。`Request::kind_name` はペイロードを
  含まない実装 :1042-1045 なので dispatch 計測は現状のまま安全）

### 8.2 個別エッジ

| ケース | 挙動（設計） |
|---|---|
| cwd（宣言指定）が存在しない | エラーで実行しない（黙ってホーム実行しない）。メッセージに解決後の絶対パスを含める |
| プロファイル重複宣言 | 後勝ち + RunResolve 応答の warnings（1.3） |
| 明示 profile が未宣言 | エラー（拡張子既定へフォールバックしない。1.6） |
| 拡張子なしファイル（Makefile 等） | 宣言があれば実行可、無ければ実行不可（拡張子キーの特殊化はしない） |
| 編集中 dirty バッファ | 実行前に保存、保存失敗なら実行中止（4.4） |
| ライブリロード（FR-3.15）中の外部変更 | 実行時点のディスク内容が走る（宣言スキャンは実行時に読み直すため表示と乖離しない） |
| CRLF / BOM / 非 UTF-8 | CRLF・BOM は許容（M1 テスト）。非 UTF-8 は宣言なし扱い（lossy 変換で 64 行だけ読む） |
| 空白・日本語・`'` 入りパス | 変数展開の自動シングルクオートで対応（1.4） |
| tmux バックエンド（persist ON） | RunInteractive と同一経路のため既存動作のまま（追加対応なし） |
| シンボリックリンク | canonicalize（OpenFile と同じ :1317）で実体パス基準に統一 |
| Windows | Phase 6 スコープ。`login_shell_command` が Windows では素通し（terminal.rs:116-118）のため、シェル包み実装に `#[cfg(unix)]` 境界を置き、Windows は `Run` をエラーで明示拒否（黙って壊れない） |
| リモート / PWA | **スコープ外**。remote v2 API にエンドポイントもボタンも追加しない（リモートからの実行は既存の input 経由で CLI を叩けば可能。将来要件が出たら別 Issue） |

### 8.3 実装上の注意（重そうな箇所）

- dispatch はファイル I/O（64 行読み）を UI スレッドで行うことになるが、16KiB 上限の
  read なので許容（perf.log の dispatch 計測 #113 で監視可能。巨大ファイルでも
  先頭 16KiB しか読まない実装にすること = `File::open` + `take(16384)`）
- ドロップダウンのプロファイル検出は render 毎にファイルを読まない。プレビューのロード /
  ライブリロード完了時に `runner::parse_declarations` を 1 回実行して PreviewState 相当へ
  キャッシュする（#232 の「目次は background 処理で 1 回だけ構築、render は完成品参照」と
  同じ原則）
- `spawn_command_pane` 抽出時、RunInteractive の応答 JSON / focus / タイトル書式を
  1 バイトも変えない（M2 受け入れ条件の既存テスト無変更緑で担保）

---

## 付録: 根拠コード一覧（再掲・実装時の読み先）

- protocol: `crates/tako-control/src/protocol.rs:125`（Request enum）, `:128`（Split）,
  `:169`（Send）, `:342`（OpenFile）, `:1011`（RunInteractive）, `:1029`（RunInteractiveStatus）
- dispatch: `crates/tako-control/src/dispatch.rs:372`（Split: cwd 明示/継承 :405-419）,
  `:1294`（OpenFile: 相対解決 :1310-1319 / mode 判定 :1326-1348 / プレビュー再利用 :1349-1370）,
  `:3094`（RunInteractive 本体）, `:3181`(Status), `:9731-9846`（TestHost テスト）
- host: `crates/tako-control/src/host.rs:36-66`（SessionHost / attach_session）,
  `:221-232`（preview_state / set_preview）, `:288-290`（preview_edit_state）
- spawn: `crates/tako-core/src/terminal.rs:99-112`（login_shell_command）,
  `:162-179`（SpawnCommand / SpawnOptions）
- settings: `crates/tako-control/src/settings.rs:11-59`（Settings）, `:133-171`（load/save）
- UI: `crates/tako-app/src/preview_render.rs:883`（render_preview_pane）, `:1846`（phv）,
  `:2177/2223/2266/2318/2344`（ヘッダボタン列）,
  `crates/tako-core/src/header_layout.rs:56-80`（PreviewHeaderVisibility）,
  `crates/tako-app/src/status_bar.rs:447-451,1368-1530`（ドロップダウン参照実装）,
  `crates/tako-app/src/file_icons.rs:152-189`（ui_icon）,
  `crates/tako-app/src/main.rs:3097`（UI → dispatch 直呼び）
- MCP: `crates/tako-control/src/mcp.rs:117-138`（INSTRUCTIONS）, `:146`（tools）,
  `:2346-2409`（run_interactive 定義例）, `:2413-2421`（call_tool / validate_known_params）,
  `:3341`（変換）, `:3719-3731`（initialize テスト）
- CLI: `crates/tako-cli/src/main.rs:240-279`（run-interactive の clap 定義）,
  `:4499`（--wait 合成）, `:5231-5243`（parse テスト）
- セルフテスト: `crates/tako-app/src/main.rs:16219-16267`（tools/list スナップショット）
- プレビュー: `crates/tako-app/src/preview.rs:20-31`（上限 / PreviewMode）, `:421-439`（拡張子判定）
- 規約: `.agent/conventions.md:37-55`（コマンド案内規約）,
  `.agent/requirements.md` FR-3 表（FR-3.17 の次 = FR-3.18）
- prompt: `crates/tako-control/src/orchestrator/mod.rs:19-22`（system prompt の include 元）
