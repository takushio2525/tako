# conventions.md — 規約（命名・エラー・ログ）

> 仕様策定フェーズの最小版。コード着手（Phase 0〜1）で実態に合わせて拡充する。

## 命名規則（Rust 標準に従う）

| 対象 | 規則 | 例 |
|---|---|---|
| クレート | kebab-case、`tako-` 接頭辞 | `tako-core`, `tako-cli` |
| モジュール / 関数 / ファイル | snake_case | `pane_tree.rs`, `split_pane()` |
| 型 / trait | PascalCase | `PaneTree`, `TerminalSession` |
| 定数 | SCREAMING_SNAKE_CASE | `DEFAULT_SCROLLBACK` |
| 環境変数 | `TAKO_` 接頭辞 | `TAKO_PANE_ID` |
| CLI サブコマンド | 小文字 1 単語 | `tako split` |
| MCP ツール | `tako_` 接頭辞 + snake_case | `tako_split_pane` |

## エラーハンドリング

- ライブラリクレート（core / control）: `thiserror` で型付きエラー、`Result` を返す
- バイナリ（app / cli）: 境界で `anyhow` 可
- `unwrap()` / `expect()` は「論理的に到達不能」な場合のみ。理由をコメントに書く

## ログ

- `tracing` クレート。レベル: `debug` / `info` / `warn` / `error`
- **ペイン内容・送信テキスト・`TAKO_TOKEN` をログに書かない**（ユーザーの入力・秘密情報を含むため）

## フォーマット / Lint

- `cargo fmt`（rustfmt デフォルト）+ `cargo clippy -- -D warnings` を CI で強制

## ドキュメント

- 仕様書は `.agent/`（日本語）。コードコメントも日本語
- 仕様変更時は該当する `.agent/*.md` を**同一コミット**で更新する

## UI 文字列の i18n（Issue #435）

UI 表示言語は日英切替（既定 = OS ロケール、`tako lang` / MCP `tako_lang` /
パレット「表示言語を切替」で手動切替）。実装規約:

- **新機能の UI 文字列は必ず日英両方を用意する**。GUI に描画する文章を render コードへ
  直書きせず、`crates/tako-app/src/ui_text/` の機能別モジュールに
  `pub fn key() -> &'static str { tr!("日本語", "English") }` で追加する
  （動的文言は `tr!(format!(..), format!(..))` で `String` を返す。選ばれた側だけ評価される）
- 関数名がロケールキー（例: `sleep_guard::chip_active` → キー `sleep_guard.chip_active`）。
  モジュールの `catalog_has_both_languages_and_no_emoji` テストに新文字列を追加する
  （非空・絵文字なし・英語側に日本語が残っていないことを機械検査）
- **対象は「画面に描画される文字列」のみ**。診断ログ（eprintln / persist.log）・
  dispatch / CLI / MCP のエラーメッセージ・AI へのプロンプトは対象外（現状維持 = 日本語可）
- 表示言語の正は `tako_core::i18n`（グローバル）。設定値（system / ja / en）は
  settings.json の `language`。言語に依存する単体テストは相対比較
  （`結果 == カタログ関数()`）で書き、`set_lang` を触る検査は
  `ui_text::tests_support::check_ja_en` に集約する（並列テストの競合防止）
- **言語グローバルを読む処理には言語を引数で受ける版を必ず添える**
  （`Note::text` / `text_in`、`gate` / `gate_in`、`autosuggest_hint_texts` /
  `autosuggest_hint_texts_for`）。1 つの出力を組み立てる間に `i18n::lang()` を
  複数回読むと、その隙に言語が切り替わったとき日英が混ざる。**解決は入口で 1 回**にして
  以降は引数で引き回す（#608）
- **テストは言語グローバルに触らない**（上の `_in` / `_for` 版を使う）。
  グローバルへの追従そのものが検査対象のときだけ `i18n::testing::lang_guard()` を取る。
  cargo test は同一バイナリのテストを並列実行するので、素で `set_lang` すると
  確率的に落ちる（#608 実測: 該当 3 本だけの反復で 26% が失敗）

## リリース配布物の命名規約（Issue #594 / #595）

配布アセットの命名は**リリース側（`scripts/release.sh`）と更新チェック側
（`tako-app::update_checker`）の両方が同じ規則で判定する**。食い違うと
「Windows クライアントが macOS の zip を掴む」「自 OS 用アセットが無いのに
更新ありと通知する」事故になる（#595 の背景）。

```text
tako-<tag>-<platform>-<arch>.<ext>

tako-v0.5.13-macos-arm64.zip        macOS / Apple Silicon
tako-v0.6.0-test.1-macos-arm64.zip  テスト版（タグに `-` と `.` を含む）
tako-v0.6.0-windows-x86_64.exe      Windows インストーラー（#587）
tako-v0.6.0-windows-x86_64.zip      Windows ポータブル版
```

- `<platform>` = `macos` / `windows`、`<arch>` = `arm64` / `x86_64`。
  **別名（`win` / `aarch64` / `amd64`）は使わない・受け付けない**
  （規則外のファイルを配布物と誤認しないための厳格一致）
- **判定ロジックの正は `crates/tako-core/src/platform/release_assets.rs` の 1 箇所**。
  シェル側 `scripts/lib/release-assets.sh` はリリーススクリプト用の写しで、
  両者の一致は同期テスト（`cargo test -p tako-core release_assets`）が機械検証する。
  規則を変えるときは **Rust を直してからシェルを合わせる**（片方だけだとテストが落ちる）
- 新しい配布形式（`.msi` 等）を足すときは `extensions()` に追加する。
  **追加し忘れると更新チェックがそのアセットを見落とし、利用者に更新が届かない**
- 更新候補は「最新リリース」ではなく**自分の環境向けアセットを含む最新リリース**。
  該当アセットが無いリリースは読み飛ばす（#595）。この規則により、
  macOS 先行リリース + Windows アセット後付けの運用をしても
  Windows 側に「更新はあるがダウンロードできない」通知が出ない

## CHANGELOG / リリースノートのプラットフォーム表記（Issue #594）

リリースノートは **Mac / Windows で分けず、単一ノート + プラットフォーム明示**で運用する
（VS Code / Zed 等クロスプラットフォームアプリの主流方式。2026-07-27 ユーザー承認済み）。

### 項目タグ

CHANGELOG.md の項目は、**種別タグの直後**にプラットフォームタグを置く。
共通の変更は無印（大多数はこれ）。

```markdown
- [修正] [Windows] ConPTY のリサイズ追従を修正 (#123)
- [機能追加] [macOS] Touch ID でのロック解除に対応 (#124)
- [改善] 更新チェックを自 OS アセット基準にする (#595)   ← 共通なので無印
```

**タグは commit の件名に書く**。夜間リリース（`scripts/nightly-release.sh`）は
commit 件名から CHANGELOG の節を自動生成するので、件名に無いタグはノートに出ない:

```
[修正] [Windows] ConPTY のリサイズ追従を修正 (#123)
```

### リリースノートの構成

`scripts/release.sh` が CHANGELOG と**実アセット**から自動生成する。手で書かない。

1. `## tako <tag>` + CHANGELOG 該当節
2. **ダウンロード表**（アセットがある OS の行だけ。Windows 版が無い間は macOS のみ）
3. OS 別インストール手順（その OS の配布物があるときだけ）
4. **Known limitations (Windows)** — #515 のサポートマトリクスから
   `tako platform --platform windows --known-limitations` で生成。
   Windows 版の配布物が含まれるときだけ付く。**機能が Windows 対応すると節から自動的に消える**
5. Claude Code 連携

生成物なので**表示言語設定に依存しない**（日英を必ず併記する）。

### macOS 先行リリース → Windows 版の後付け

同じタグにアセットを足す運用を正式手順とする（#595 のフィルタと対で成立する）:

```sh
gh release upload v0.6.0 dist/tako-v0.6.0-windows-x86_64.exe --clobber
scripts/release.sh --update-notes v0.6.0   # 実アセットを読み直してノートを作り直す
```

アセットを足した時点で、Windows クライアントの更新チェックに初めてそのリリースが見える。
生成結果の確認は `scripts/release.sh --notes-only`（ビルドも公開もしない）。

## コマンド案内の規約（Issue #322）

ユーザー体験の設計原則。setup に限らず、CLI 出力・system prompt・docs のすべてに適用する。

- **常に最も簡単な形のコマンドを提案する**: 既定値で済む引数・オプションを付けて見せない
  （例: `tako master -default` とせず `tako master`。プロファイル引数は default 以外の
  ときだけ表示する。実装は `orchestrator::launch_command` が正）
- **ユーザーが触れるコマンドを少なく・簡単に**: 標準フローは引数なしで完結させる
  （例: `tako setup` 単体で完結）。`--yes` / `--answers` 等のフラグは自動化・上級者向けの
  逃げ道として互換維持するが、標準の案内には出さない
- **機能追加は既定動作を賢くする方向で**: 新しい `--オプション` を増やして解決しない。
  分岐が必要なら検出値 → 前回値 → 既定値で自動解決する（#262 の質問ゼロ setup と同じ路線）
- **設定より対話**: 設定ファイルの編集やフラグ操作を案内する前に、「master に日本語で
  頼めば済む」導線を優先して示す（例: プロファイル調整・プロジェクト登録）
- **素のコマンドで対話まで完結する**: `tako setup` / `tako master` のような素のコマンドで、
  対話を通じて何でもできる状態を既定にする。対話 agent の起動は省略しない（Issue #391）。
  `--` オプションは「詳しい人が、わかったうえで付ける」上級者レイヤであり、既定の
  ユーザー体験はオプションなしで完結すること。CLI 設計時にこれを判断基準にする

## 一括 dismiss に食われないクリック要素の作り方（Issue #496 / #503）

ルート div の `on_mouse_down` は `clear_text_input_focus()` を呼び、テキスト入力フラグと
メニュー開閉状態をまとめて落とす（#503 の「キー入力が奪われたまま残る」対策 +
メニュー外クリックで閉じる dismiss 経路）。GPUI の配送は **`mouse_down` → `mouse_up` →
`click`** の順なので、次の規約を守らないとクリックが構造的に死ぬ。

- **`clear_text_input_focus()` が落とす状態に依存して描かれるクリック要素は、必ず
  `on_mouse_down` で `cx.stop_propagation()` する**。守らないと押下の mouse_down で
  自分が消え、`on_click` が一度も発火しない（#496 のコンフリクト解消エージェント 3 択が
  merge 時から GUI で動いていなかった。CLI / MCP の同じ dispatch は動くので気付けない）
- トグルボタン（開閉を反転する側）も同じ。守らないと mouse_down で `false` に落ちた直後に
  `on_click` が `!false` = `true` にするので、**開いた状態から閉じられない**
- 実装の正は `starter.rs` のプロファイル選択メニュー（項目に `stop_propagation`、
  背面に全画面 dismiss div）と、git コミット入力欄（`right_panel.rs`）
- 回帰は**合成マウス**で押さえる。`self_test::click_at` が実 OS マウスと同じ
  `PlatformInput`（MouseMove → MouseDown → MouseUp）を流すので、GPUI のヒットテストと
  リスナー配線まで通る。ハンドラを直呼びするテストではこの型のバグを検出できない

## セルフテストの待ち条件の書き方（Issue #796）

隔離セルフテスト（`TAKO_ISOLATED=1 TAKO_SELF_TEST=1`）は worker の完了判定に使うので、
**同じソースなら同じ結果になる**ことが前提になる。時間で待つ検査はこの前提を壊す。

- **「出るもの」を待つのに固定時間を使わない**。`wait(cx, N).await` の直後に
  `check(focused_contains(...))` と書くのは禁止で、`wait_for_focused_text`
  （状態到達まで待ち、上限で偽 + 診断を出す）を使う。CI で毎回走る番犬テスト
  `selftest_wait_watchdog` が違反を名指しで落とす
- **否定検査には必ずアンカーを置く**。「出ないこと」だけを固定時間後に見ると、
  出力が来る前に通ってしまう（偽 PASS）。`absent_after_anchor(anchor, forbidden)` で
  「先に必ず出るもの」を待ってから禁止文字列の不在を見る
- **待つ文字列は「その状態でしか出ないもの」にする**。画面は消えないので、前段で
  同じ文字列を出していると即マッチして偽の待ち条件になる（#601 は A / B 両フェーズの
  プロンプトが `ST601>` で同一だったため、B の起動待ちが A の残り表示に当たり、
  続く入力が起動前のシェルへ流れて「解決順を変えない」が偽 FAILED になっていた →
  `ST601A>` / `ST601B>` に分離）
- **前提が整うのを待ってから本題を検査する**。分割直後のペインはシェル起動の子プロセスを
  抱えていることがあり、「素のアイドルなペイン」を前提にした検査は前提の成立を待つ
  （#732 の cmd+W 確認ダイアログ）
- **リトライで隠すなら上限と記録を必ず付ける**。上限まで待って駄目なら偽にする
  （検出力は固定待ちと同じか強い）。諦めたときは `TAKO_SELF_TEST_WAIT_TIMEOUT`
  に待った実測時間・画面末尾・実行環境を出す
- **失敗ログには実行環境を残す**。`TAKO_APP_SELF_TEST_ENV` に profile / feature 構成 /
  load average / 経過を出す（`--features visual-test` は gpui の leak-detection を
  有効にするので、同じソースでも数割遅い = 固定待ちがここでだけ落ちていた）
- **レイアウト・スクロールの幾何を読む前は「汚してから 1 フレーム描く」**（`notify_and_draw`）。
  #786 でペイン本体とクロームは `AnyView::cached` になったので、dirty でないフレームは
  子ビューを描き直さない = 幾何がキャッシュのまま残る。製品経路（IPC / MCP の dispatch
  ループ）は dispatch のあとに `cx.notify()` してから次フレームを描くので、**直接
  `dispatch` を呼ぶ検証側も同じ順序にする**。守らないと「操作が効いていない」ように見え、
  2 秒ポーリングの notify がたまたま挟まった回だけ通る（#232 の PDF アウトラインジャンプが
  #786 以降フレークになっていた実例）
