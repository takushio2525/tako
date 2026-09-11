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
- **相対比較でも「比較の 2 点が同じ言語」は保証されない**（#1274）。言語依存の関数を
  1 つのテストで 2 回以上呼ぶなら、`tests_support::for_each_lang` / `with_lang` /
  `check_ja_en` の**区間の中**で比較するか、本体の先頭で `tests_support::lang_guard()`
  を束縛する（ロックの外だと、2 回の読み取りのあいだに別スレッドのテストが言語を
  切り替えて別言語同士を比べる）。番犬は `ui_text::lang_watchdog`、
  競合の再現は `ui_text_lang_race`（A/B は `TAKO_1274_LEGACY=1`）
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

## tmux ターゲットの完全一致指定（Issue #866）

tmux の `-t` は**前方一致**で解決するので、tako は取り違えを防ぐために
`=name`（完全一致）を渡している（#181 / #32）。ただし `tmux` の名前で入っている
CLI が**本物の tmux とは限らない**（Windows は winget の `marlocarlo.psmux` が
`tmux.exe` を配置する）。

- **`=` を自分で書かない**。`tako_core::tmux::exact_target`（`=name` / `=session:0.0`）と
  `session_pane_target`（`=session:`。target-pane 系は末尾コロン必須 = #32）を通す。
  付けるかどうかは `tmux -V` の申告から 1 度だけ決まる（`TmuxTargetSyntax`）
- 番犬テスト `tmuxの完全一致ターゲットの直書きが境界の外に残っていない`
  （`crates/tako-control/tests/platform_parity.rs`）が `format!("=…")` の直書きを
  名指しで落とす
- **なぜ macOS では気づけないか**: psmux は `kill-session -t =name` を解釈せず、
  **5.1 秒ブロックしたうえで exit 1**（1 つも消えない）。素の `-t name` なら 181ms で
  対象だけが消え、前方一致だけの `-t kee` は**何も消さない**（実測。psmux は素の
  名前でも完全一致）。macOS の tmux は `=` で正しく動くので、テストも含めて全部緑になる
- 「本物の tmux か」を条件にしたいときも `tako_core::tmux::announces_only_tmux` を通す
  （版数文字列の判定を 2 か所に持たない）。attach / send-keys まで tmux 決め打ちの
  検証（セルフテスト 59〜62 / 68 / 73）だけがこの条件を使ってよい

## `canonicalize` の結果を持ち回らない（Issue #970）

`Path::canonicalize` は Windows で **verbatim 形式**（`\\?\C:\Users\…`）を返す。これは
Win32 のパス正規化と `MAX_PATH` 制限を無効にする**入口指定**であって、他のプログラムへ
渡したり画面へ出したりする形ではない。

- **解決結果を保存する / 子プロセスへ渡す / 応答へ出すなら
  `tako_core::platform::path::canonicalize`（境界 B26）を通す**。比較キーを作るだけなら
  `canonicalize_or_self`（`unwrap_or_else(|_| path.clone())` の置き換え）を使う
- 番犬テスト `canonicalizeの直呼びが境界の外に残っていない`
  （`crates/tako-control/tests/platform_parity.rs`）が**ファイルごとの件数**で見張る。
  トラバーサル判定（`remote_files` / `remote` のアップロード）と比較キー専用の解決、
  テスト内の直呼びは表に理由つきで載っている。**増やすときは理由を書く**
- **なぜ macOS では気づけないか**: unix の `canonicalize` は prefix を付けないので、
  テストも含めて全部緑になる。Windows では `tako open-in dir <repo>` したタブの cwd が
  シェル統合の `\` → `/` 置換で **`///?/C:/…`（実在しないパス）**になり、
  `git rev-parse --show-toplevel` を回す `Command::current_dir` が起動に失敗して
  **そのタブの git 操作が全滅**していた（`tako list` / `recent` / `pane_current_path` の
  表示にも `\\?\` が漏れる）。git 自身は verbatim を扱えるので、壊れているのは
  prefix そのものではなく**潰した後の形**
- **剥がさない場合がある**: verbatim を外すと Win32 の正規化が復活するので、
  意味が変わる形（`MAX_PATH` 超え / `/` を含む / `.` `..` や空の成分 / 末尾が `.` か
  空白の成分 / `NUL` などの予約デバイス名 / ボリューム GUID 形）は verbatim のまま返す。
  **剥がして別の場所を指すより、既知の不具合が残るほうが安全**という判断
- 発信側（`shell-integration/tako.ps1` の `__takoStripVerbatim`）でも剥がす。シェルが
  verbatim な作業ディレクトリを**継承する**経路が残るため: `Set-Location` は verbatim を
  拒否する（FileSystem プロバイダが `Cannot find path` を返す = 実測）ので、入り口は
  **`CreateProcess` へ渡る cwd だけ** — tako 自身が verbatim な cwd で起動された場合と、
  #970 より前の版が保存した layout の cwd で開き直した場合。それを見られるのは OSC を出す
  スクリプトだけ。ただし**入口の代わりにはならない**（prefix は cwd 以外にも漏れる）。
  順序（置換より前に剥がす）は番犬 `osc7を組む前にverbatimを剥がしている` が固定する

## ホーム解決と `~` 短縮の入口は 1 本（Issue #870 / #893）

ホームは **`tako_core::paths::home_dir()`**（`HOME` → `%USERPROFILE%`）、
表示用の `~` 短縮は **`tako_core::paths::shorten_home()`**（純粋版 `shorten_home_with`）
だけを通す。番犬 `crates/tako-control/tests/home_dir_watchdog.rs` が
**ワークスペース全体**（`crates/**/*.rs`。`src/` も `tests/` も）を走査して落とす。

- **`std::env::var("HOME")` を単独で読まない**。`HOME` は Windows に通常無いので、
  その経路は**必ず `None` を返す = その機能がまるごと落ちる**。#870 は
  ターミナルリンクの `~/` がこれで無反応になり、棚卸ししたら同型が 15 箇所あった（#893）
- **macOS では気づけない**: `HOME` は必ず立っているのでテストも実機も全部緑になる。
  だから「壊れているか」ではなく「正本を通しているか」を機械で見張る形にしてある
- 例外は番犬の `ALLOWED_HOME` に**ファイル名 + needle の組**で載っている。
  `cfg(windows)` の中で**ネイティブの `%USERPROFILE%` を名指しで**欲しい場所
  （PowerShell の `$PROFILE`・Windows 版インストーラの置き場）は `USERPROFILE` の
  直読みだけを許す（`home_dir()` は `HOME` を優先するので、Git Bash から起動した
  Windows で意図がずれる）。**`HOME` の直読みはそこでも禁止**
- テストが `HOME` を差し替えるなら `tako_control::test_home::HomeGuard`
  （`Drop` で必ず戻す）を使う。手書きの復元は **panic した回だけ漏れる**
- **「`cfg(test)` の中だけ許す」という走査は書かない**: テスト本文の文字列リテラルに
  `{` が 1 つあるだけで波括弧の対応が崩れ、そのファイルの残り全部が例外になる
  （= 本番コードの直読みを見逃す）。数え違いが「見逃す」側へ倒れる検査は番犬にならない

`~` 短縮を各所で組み立てると、同じ規則が**別々に劣化する**。移送前は 10 か所にあって、
①ホーム解決の欠け（Windows で 1 つも縮まらない）②ホーム自身の扱い（`~` を出す実装と
`~/` を出す実装が混在）③文字列の前方一致なのでホーム `/Users/alice` のとき
`/Users/alice2/dev` が `~2/dev` へ化ける、の 3 つを別々に持っていた。

- 区切りは**元の文字を残す**（`~/dev` / `~\dev`）。表示用なので OS の見た目に合わせる
- **可搬表記は別物**: `config_share::env` の `abbreviate_home` は #513 でデバイス間へ
  持ち出す形（区切りを `/` へ正規化する）を作るので、表示用の短縮とは役目が違う。
  番犬の `ALLOWED_SHORTEN` に理由つきで載せてある

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

## UI アニメーションは「いつ終わるか」を決めてから足す（Issue #945）

GPUI の `AnimationElement` は、**アニメーションが終わっていないフレームで毎回**
`window.request_animation_frame()` を呼ぶ（`gpui/src/elements/animation.rs` の
`if !done { window.request_animation_frame(); }`）。つまり動いているアニメーションが
1 個でもあるあいだ、**アプリはアイドルフレームに到達しない** —— #782 / #786 / #801 /
#803 で削った「毎フレームの固定費」がそこで丸ごと復活する。

- **`Animation::new(..).repeat()` は永久に `done` にならない**。状態に紐づけて
  `repeat()` を出すなら、その状態が**必ず短時間で解ける**ことまで確かめる。
  タブの実行中ドット（#217）は `CommandState::Running` に紐づいていたが、
  エージェント（claude / codex）はフォアグラウンドで走り続けるのでセッションが
  終わるまで解けず、脈動が恒久化していた（実測: 出力ゼロのペイン 1 枚で
  tako 自身が **19.09% → 2.93%**）
- 「走り始めた」ような**合図**は oneshot（`repeat()` なし）で有限回にする。
  GPUI は完了フレームで `done` を立てて要求を止め、以後 `delta` は 1.0 に貼り付く。
  **`delta = 0.0` と `1.0` の見た目を同じにしておく**こと（そうしないと脈動が
  終わった瞬間に色が飛ぶ）。実装の正は `tab_bar.rs` の `tab_dot_opacity`
- **再開は element state の寿命に任せる**。GPUI は「そのフレームで描かれなかった
  element state」を捨てるので、条件が false になって要素ごと消えれば、次に true へ
  戻ったとき同じ id でも状態は作り直される（= アニメーションがやり直される）
- 回帰は**アニメーターが計算した値**で押さえる。「実際に描かれたフレーム数」は
  ディスプレイリンクが動かない環境（蓋閉じ・ヘッドレス）で両アームとも 0 になり、
  検出力が消える。「時間を空けて描き直しても値が動かない」= `done` = 要求も止まっている、
  と言い切れる（セルフテスト項目 128）

## 器の中のシェルへ渡す前提は「名前の推測」に頼らない（Issue #1105）

器（tmux / psmux）のサーバーは**最初のクライアントの環境を引き継ぎ**、後続の
セッションもその stale な値を使う（実測: `ZDOTDIR=A` で起動したサーバー上に
`ZDOTDIR=B` のプロセスからセッションを作ると、中のシェルは A を見る。
`-e` で渡せば B を見る）。だから器の中のシェルに何かを伝えたいときは:

- **`new-session -e` で作成時に固定する**。正本は `backend::session_pinned_pairs`。
  「呼び出し元プロセスごとに違う値」はすべてここへ載せる。載せ忘れると、同じ socket 名に
  別インスタンスのサーバーが残っている環境でだけ**黙って**壊れる（#1105 はシェル統合の
  置き場がこれで、cwd 追従と コマンド状態が両方死んだ）
- **`options.env` を舐めるだけでは足りない**。シェル統合の env は
  `TerminalSession::spawn` が**外側 PTY**（= 器のクライアント）の env へ足すので、
  `wrap_options` からは見えない。正本（`shell_integration::env()`）から直接引くこと
- **「自分は tako の器の中か」を名前の接頭辞で推測させない**。#1105 まで統合スクリプトは
  `$TMUX` のソケット basename が `tako*` かで判定しており、`TAKO_TMUX_SOCKET` に
  別の名前を与えると OSC を DCS で包まず tmux に飲まれていた。tako が名前を明示する
  （`BACKEND_SOCKET_ENV`）ので、スクリプトはそれと突き合わせる。接頭辞は
  「この env を渡さない古い tako」用のフォールバックとしてだけ残す
- **検証スクリプトのソケット名も同じ罠を踏む**。使い捨てのソケットに `tk…` のような
  名前を付けると、製品が壊れていなくても OSC 系の項目だけが落ちて「main の回帰」に
  見える（#1105 の起票がまさにこれ）。**器のソケット名は `tako` で始める**か、
  診断行（項目 60 の `backend_socket_env`）で伝わっているかを確かめる
- 落ちたときに切り分けられるよう、診断は**段ごとの材料**を出す（#796）:
  器の label / 素通し設定の実値 / 置き場の期待値とセッションの実値 / サーバーの継承値 /
  器の同一性 / OSC 133 の状態 / ペイン末尾 / 待った時間 / load

## 「届き方」は型で分け、in-process を先に見る（Issue #1200）

ペインへ届く経路は 2 つしかない（`reach` の説明）。**tako-app が保持している
（in-process）**か、**器越し（detached）**か。`PaneReach` はこれを網羅 match で
扱わせるためにあるが、**その型を通らない経路を書くと規律が消える**。

#1200 はそれだった: `respond`（選択肢ダイアログへの応答）は入口でバックエンド
セッション名を必須にし、`reach::detached_session` へ直行していた。tmux は
`send-keys` でアウトオブプロセスにも送れるので macOS では成功してしまい、
**psmux（Windows）では生きているペインに対して必ず失敗**していた
（psmux は `detached_capture` を持つが**入力送出を持たない**）。器なしのペイン
（`TAKO_PERSIST=0`）には最初から応答できなかった。

### 書くときの決まり

- **ペインへ届く操作は必ず in-process を先に試す**。`reach` の入口
  （`PaneReach::resolve` / `dialog_access`）を通し、`tako_core::backend` の
  `detached()` を直接引かない
- **経路ごとに手順を書き分けない**。「画面を読む」「キーを送る」の 2 手だけを
  trait（`reach::DialogAccess`）で抽象し、検証・再試行・監査ログは
  1 実装（`dispatch::respond_via`）が両経路で共有する。書き分けると
  「macOS では番号キーで確定するが Windows では Enter も要る」のような差が生まれる
- **in-process のキー送出は自分で PTY へ書く**。器越しはキー名（`Enter` / `Down`）を
  器が解釈するが、in-process では落とす必要がある。語彙の正本は
  `tako_core::backend::key_name_bytes` の 1 箇所で、**落とせない名前は推測せず
  エラーにする**（適当なバイト列はダイアログを誤操作する）
- **`TerminalSession` はスレッドを越えられない**（`&mut` を要る操作を持つ）。
  バックグラウンドで走る応答ループ（#813 の自動復帰は数百 ms のスリープを挟む）へは
  `TerminalSession::access()` の `PaneAccess`（共有できるものだけを持つ手）を
  **UI スレッドで取り出して**渡す
- **どちらで届いたかを監査ログへ残す**（`[dialog-respond] route=in-process|detached`）。
  残っていないと「送ったのに効かない」の切り分けが画面の推測になる

### 残る縮退は 1 マスへ宣言する

psmux は入力送出を持たないので、**tako-app が保持していないペイン**（GUI 不在・
ペイン消失）へは Windows では応答できない。これは直せない差なので
`platform::support` の `tako_orchestrator_respond` を `Degraded` + 根拠つきで宣言する
（`Supported` のままにすると `PlatformFacts` 経由で system prompt へ誤情報が流れる）。

## 器へ渡した env は「器の中の全シェル」へ配られる（Issue #1199）

#1105 の続き。`new-session -e VAR=VAL` で固定した値は**そのセッションの中だけ**に
届く、という前提は**psmux では成り立たない**。実測（#1199）:

- psmux の `-e` は**サーバーのグローバル環境**へ入る（`show-environment -g` に
  `TAKO_PANE_ID` / `TAKO_OSC_SINK` が並ぶ。`show-environment -t <別セッション>` でも同じ値）
- psmux は**プリウォーム済みのシェルの一団**（`tmux server -s __warm__ …`）を持っており、
  その環境も同じ表から作られる。cwd はユーザーのホーム

つまり **tako がペイン固有として渡した値は、そのペインのシェル以外にも渡っている**。
「パスだけの待ち合わせ」（`<data_dir>/osc/<pane>.osc`）はこれで壊れた: ウォームプールの
シェルが同じファイルへ `OSC 7 <ホーム>` を書き、tako がペインをリサイズするたび
（psmux はプールもクライアントの寸法へ合わせるのでプロンプトが描き直る）
**ペインの cwd がホームへ巻き戻っていた**（#1199 = 機能不能。ファイルツリー・
ステータスバー・`tako list` の `panes[].cwd` が誤ったフォルダを指す）。

### 書くときの決まり

- **器の中で「自分だけの置き場」を env で決めない**。ペイン固有のはずの値は器の中で
  共有されている前提で設計する。待ち合わせ先には**書き手の同一性**を載せる
  （側路は `<pane>@p<pid>.osc`。規則の正本は `tako_core::osc_sink::resolve_writer_path`
  で、統合スクリプトはその写し。番犬 `osc_sink_writer_watchdog` が両方を突き合わせる）
- **同一性は器が割り振ったものを使う**。`TMUX_PANE` は器の実装次第で
  セッションを跨いで重複しうる（psmux はセッションごとにサーバープロセスを持つので
  `%0` が複数あり得る）。**pid は OS が一意にする**ので、器へ `#{pane_pid}` を 1 回聞くのが確実
- **器への問い合わせを増やさない**。`#{pane_pid}` は tty を引く既存の 1 回
  （`tmux_backend::pane_facts`）から一緒に採る
- **解決は冪等にして、解決後の値を env へ書き戻す**。ペインの中で起こした子シェル
  （入れ子の pwsh・ユーザー自身の tmux）が同じファイルを共有できないと、
  そこだけ cwd 追従が死ぬ
- **解決前のパスも読み続ける**。器を跨いで生き延びたペイン（`survives_app_exit`）の
  シェルは**更新前のスクリプト**を読み込んだままなので、そのシェルが起き直るまでは
  解決前のパスへ書く

### 切り分け方（実測で効いた手順）

1. tako 側の値（`tako list` の `panes[].cwd`）と**器の値**（`#{pane_current_path}`）を
   同じ瞬間に並べる。器が正しければ「情報は失われていない = tako の取り込みが壊れている」
2. **待ち合わせファイルの中身を毎段で 16 進で見る**。#1199 は「ホームの初回束
   （`133;A` + `OSC 7 <ホーム>` + `133;B`）へ**戻る**」が決め手だった
3. **ペインのシェルだけ黙らせる**（`tako send --pane N '$global:__takoSink = $null'`）。
   それでもファイルが復活するなら、書き手はペインのシェルではない。
   プロセスを止めずに書き手を切り分けられる（実機のプロセスを触らない）

## 「保留フラグ」と「それを回す人」を分けない（Issue #973）

`Context` が要る後処理（タイマー・PTY 起動・背景ジョブ）を dispatch から始めたいとき、
**「フラグを立てる関数」と「それを見て回す関数」を分けて呼び出し側に両方書かせる形**を
作らないこと。片方を呼び忘れた経路が**無音で死ぬ**（フラグは立つので状態は「保留中」に
見え、失敗もエラーも出ない）。

- #973 の実物: プレビュー編集の自動保存が `schedule_autosave`（保留フラグ）と
  `start_autosave_timer`（500ms 後に保存）に分かれており、後者を呼ぶのは GUI の入力経路
  （キー / ペースト / IME）だけだった。dispatch 経路（`edit replace` / `apply` / `undo` /
  `redo` = CLI / MCP）は保留に入ったまま誰も保存せず、`EditState::open` の既定が
  `autosave: true` なのに**一度も自動保存されなかった**（利用者からは「自動保存 ON なのに
  保存されていない」= データを失いかねない見え方）
- **入口は 1 本にする**。フラグとタイマーを同じ関数の中で始めれば呼び忘れが起きない
  （`drive_autosave`）
- できるなら**フラグそのものをやめて状態から導く**。「編集した人が申告する」のではなく
  「autosave が有効 + 編集中 + dirty なセッション」を毎回数えれば、**新しい編集経路は
  何もしなくてよい**（判定の正は `preview::autosave_due`。#966 のリモート既定 OFF のような
  例外も 1 箇所で効く）。判定を呼び出し側で書き直すと規則が 2 つ並ぶので番犬で止める
- 消化するのは**すべての経路が通る 1 箇所**へ置く。dispatch なら IPC の 1 ターンの後処理
  （`pending_attach` / `pending_writes` / `pending_highlights` と同じ場所）。番犬
  `crates/tako-control/tests/preview_autosave_watchdog.rs` が「フラグを立てる箇所が 1 つ」
  「それは入口の中」「IPC の 1 ターンが消化する」「旧 2 本立てが復活していない」を見る
- 検証は**フラグではなく結果**で押さえる。「保留に入った」ことを見るテストは旧実装でも
  通ってしまうので、**実 CLI でディスクの中身が変わるところまで**見る（セルフテスト項目 141。
  dispatch を直接叩くと消化する側を検証できない）

## `occlude()` はスクロールも止める（Issue #576 / #961）

GPUI の `Window::hit_test` は hitbox を手前から走査し、`HitboxBehavior::BlockMouse`
（= `InteractiveElement::occlude`）に当たった時点で **break** する。積まれなかった祖先は
`mouse_hit_test.ids` に入らないので、

- `hitbox.is_hovered()` → false（これが `occlude()` の狙い）
- **`hitbox.should_handle_scroll()` → false**（こちらは巻き添え）

の両方が false になる。`overflow_x_scroll` / `overflow_y_scroll` の既定ハンドラも
`InteractiveElement::on_scroll_wheel` も発火条件が `should_handle_scroll()` なので、
**スクロール領域の中で `occlude()` する子を置くと、その子の上ではホイールが死ぬ**。

実例（#961）: #576 がタブピルへ `occlude()` を付けたことで、#208 のタブバー横スクロールが
**丸ごと効かなくなった**（ピルは領域のほぼ全面を覆うため、事実上どこでも効かない）。
`occlude()` を外す修正は Windows の `on_hit_test_window_control` が
祖先の `WindowControlArea::Drag` を拾って #576 を再発させるので採れない
（`block_mouse_except_scroll()` も `ids` には積まれたままなので同じく再発する）。

したがって:

- **スクロール領域の中で `occlude()` するなら、その要素自身が `on_scroll_wheel` で
  スクロールを中継する**。実装の正は `tab_bar.rs` の `TabScrollOcclude::occlude_scrolling`
- 中継の計算は **GPUI 既定と同じ意味論**にする（横 delta があればそれ、無ければ縦 delta を
  横へ回す / offset は足すだけでクランプは prepaint に任せる）。ずれると
  「子の上」と「隙間の上」で挙動が食い違う
- 回帰は**実 `PlatformInput` のホイール**で押さえる（ハンドラ直呼びでは hit test を通らず
  この型のバグを検出できない）。**動かしてから 1 フレーム描いてから**流すこと
  （`should_handle_scroll` はフレーム構築時の hit test を見る）

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
  `ST601A>` / `ST601B>` に分離）。
  **ただし「画面は消えない」は通常画面の話で、alt screen の TUI では成り立たない**
  （#1175）。claude の TUI はスクロールし、長い貼り付けは `[Pasted text #N]` に畳む
  ので、一度流れた本文はビューポートから消える。次項を参照
- **実 claude の「発話の内容」はビューポートから採らない**（#1175）。項目 101c は
  「引き継ぎ本文が後任へ届いた」を `visible_lines()` から探していたが、本文は
  claude の TUI では**起動直後に 1 度だけ流れる User 発話**なので、後任が数分喋れば
  確実に画面外へ出る（実測: `3m 58s · ↓ 14.1k tokens` 喋った時点で不在）。
  負荷とは無関係で、**後任が長く働くほど確実に落ちる**。証拠は流れない場所 =
  後任の **transcript**（`chat_state(pane).messages`。項目 95c と同じ経路）から採る。
  - **役割で分ける**。引き継ぎプロンプトの本文には手順として「『引き継ぎ完了』と
    報告する」が入っているので、役割を見ないと**渡したプロンプト自身**を後任の申告と
    読み違える（実測: 旧の画面判定は `saw_done=true`、transcript の Assistant 限定は
    `false`。後任はまだ申告していなかったので後者が正しい）
  - **前提はその項目のあいだだけ自分で作る**。`chat_state` を埋める定期更新
    （`collect_chat_targets`）は **GUI モードでしか回らず**、live 解決は**tmux
    バックエンドのセッション名**をキーにする。セルフテストはどちらも既定 OFF
    （`TAKO_ISOLATED=1` が `TAKO_PERSIST=0` を置く）なので、起動レシピに前提を
    負わせず `Request::Persist` と `ui_mode` を項目内で上げて戻す（95c / 97c と同じ形）
  - **ビューポートが正しい証拠源のものまで移さない**。信頼ダイアログの承諾（毎周期の
    駆動）・**未送信**の入力欄の検査（45c は送っていないので transcript に無い）・
    `pane_display_for` の遷移（95c / 97c）はどれも「いまどうなっているか」を問うている
  - 診断行に**証拠源**を出す（`marker_source=transcript` / `chat=<User 数>/<全数>`）。
    `chat=none` = 証拠源が立ち上がらなかった、で「届かなかった」と区別できる

  番犬テスト `実claudeの発話をビューポートで判定していない` が違反行を名指しで落とす
  （実 claude の e2e ブロックの中で、**画面から採った値を周期をまたいで溜める**
  `|=` を見る。溜めるのは「一度でも見えたか」を問うている印で、それはビューポートでは
  答えられない。旧経路を再現する `legacy` アームは対象外）
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
- **`dispatch` 直呼びで md / PDF / 動画を開いたら `drain_pending_preview_loads` も自分で
  呼ぶ**。この 3 つは background ロードのキューへ積まれるだけで、実際に回すのは
  **UI 経路と IPC 受信ループ**。呼ばないと `Loading` のまま待ち続け、「他の何かが
  たまたま回した」回だけ通る（#826 で visual-test の md / md ストレス / PDF の
  3 か所がこれだった。`main` のバイナリでも同じ場所で落ちることを実測して確認）
- **ペインへ打つコマンドの env 代入は必ずクオートを通す**。値を素の `format!` で
  埋めると、data dir が既定の `~/Library/Application Support/tako` のとき
  `ZDOTDIR=…/Application` までが代入・`Support/…` がコマンド名として割れ、
  意図したプログラムが起動しない。`self_test::shell_env_command`（値を
  `tako_core::shell::quote_for_shell` へ通す）を使う。番犬テスト
  `selftest_env_assignment_watchdog` が違反行を名指しで落とす（#833）。
  **隔離起動（`TAKO_ISOLATED=1`）の data dir は `/tmp` 配下で空白が無い**ので、
  隔離検証だけを回していると踏めない = main 由来の確定失敗として残る。
  項目 41c / 41d の隔離 HOME はディレクトリ名に空白を入れてあり、
  `HOME=` / `PATH=` 側は毎回の隔離セルフテストで踏む
- **注入した fixture の状態は「守る」か「毎回作り直す」かを決めてから `await` を挟む**。
  検証用の会話・状態を注入したペインは 2 秒 tick の定期更新が**正しく**現実と
  突き合わせて消す（チャットの fixture は実 claude が動いていないので
  `apply_chat_refresh` が `chat_panes` から落とす）。注入から検査までに `await` が
  1 つでもあれば tick が挟まるので、`pin_chat_fixture` のように**読み取り対象から
  外して race を無くす**（判定そのものは変えない）。項目 98（#725）は MCP を 3 回
  往復するあいだに会話が消え、決定的に失敗して以降の項目が一切走らなくなっていた（#853）。
  番犬テスト `chat_fixture_pin_watchdog` が pin の欠落と順序違いを落とす
- **PTY へ書く Enter は CR（`\r`）**。端末が Enter として送るのは CR で、PowerShell
  （PSReadLine）は素の LF を**継続行（`>>`）の開始**と解釈するので打ち込んだコマンドが
  確定しない。POSIX 側は tty の ICANON + ICRNL が CR も LF も改行へ倒すため、
  **CR に寄せれば両方の方言で通る**（方言差ではないので `ShellDialect` ではなく
  `self_test::pty_line`（本文 + CR）に置いてある）。項目 94（#702 alt screen）は
  `format!("{cmd}\n")` のせいで Windows において確定失敗し、**94 以降（チャット操作 /
  準備中 / 設定画面 / limit-resume）が 1 つも走らない**状態だった（#897）。番犬テスト
  `selftest_pty_enter_watchdog` が `.write(…)` の**括弧の釣り合いで式を切り出して**
  違反行を名指しで落とす（項目 94 は `format!(` と `"{}\n",` が別の行にあり、
  行単位の走査では見つからなかった）
- **1 つの `check` に条件を積み上げない**。`&&` 連鎖は「どれで落ちたか」が出力から
  確定できず、原因の切り分けに実機の再現待ちが要る。経路ごとに `check` を割り、
  診断行には**判定に使った材料そのもの**（応答本文・状態の有無）を出す
  （#853 で `list` / `code` / `markdown` の 3 本へ分割した）
- **再描画の回数を数える検査は「測る窓が汚れていないこと」を先に確かめる**。
  `pane_body_renders` / `pane_header_renders` / `chrome_renders` は**アプリ全体の
  カウンタ**なので、窓のあいだにアプリ全体を汚す `cx.notify()`（2 秒 tick 等）が
  挟まると可視ペイン全部が描き直り、**製品の不具合と区別が付かない数字**になる
  （項目 110 の `body +2 header +2` は「意図的な全体 notify」と同じ値。#858。
  項目 108 は同じ穴で `chrome +2` になり load 27 で FAILED / load 14 で ok。#995）。
  時間で動くもの（ヘッダの時計 #803）と持ち越し（`term_pending_app`）は測る前に
  窓の外へ出し、外から来る汚れは**検出してやり直す**（上限つき・各試行を記録・
  全滅なら FAILED）。上限の回数は `state_wait_budget` と同じ方針で混み具合に応じて
  **伸ばすだけ**（`guard_attempts`）。
  **汚れの証人は「測る量の外側」から選ぶ**のが肝（#995）。測る量そのものを証人に
  すると、外から来た汚れと本物の回帰が同じ数字になって区別できない:
  - 項目 110（測るのは**ヘッダ**）→ 証人は `chrome_renders`
    （キャッシュしたクローム 4 枚は**アプリ全体が汚れたときだけ**描き直る
    = 可視ペインの枚数に依らない）
  - 項目 108（測るのは**クローム**）→ 証人は `pane_body_renders` の増分が **2 以上**
    （出力を流したのは 1 ペインだけなので、2 枚目の本体が描き直れば全体が汚れた証拠）。
    ヘッダは `TAKO_803_NO_HEADER_CACHE=1` だと毎フレーム描き直るので証人にしない

  判定は純粋関数（`redraw_window_clean`）・窓の作り方は 1 実装
  （`measure_output_redraw`）に寄せてあるので、同じ形の検査を足すときは
  `RedrawSubject` を 1 つ増やすだけでよい。やり直しが本物の回帰を隠さないことは
  「窓が汚れていない状態で増分が出る」注入（110 = `TAKO_858_INJECT=header` /
  108 = `TAKO_995_INJECT=chrome`）で毎回確かめられる
- **打ち込む fixture は「ペインの実寸が届いてから」描く。高さが要るなら専用タブへ**（#1162）。
  端末は既定の 80x24 で作られ、実寸はレイアウトが走ったフレームで初めて届く
  （ビューは `AnyView::cached` なので dirty でないフレームは描き直さない = #786）。
  順序を揃えないと **fixture を 80x24 の格子へ描いてから実寸へ縮む**ので、
  収まる回と収まらない回が入れ替わる。項目 102（#1131 の 25 桁ダイアログ = 13 行）は
  実測 `size=(58, 9)` のペインで箱の上端と `❯ 1.` が画面の外へ出て `shown=false` になり、
  分類は無罪なのに **103 以降が 1 つも走らなくなっていた**（`load` と対応しなかった理由も
  これ = 混み具合ではなく先行項目が残したレイアウト次第）。読む前に
  `notify_and_draw` で 1 フレーム描き、**高さが要る fixture のペインは専用タブに 1 枚で作る**
  （`TabNew` = 全高。最後のペインを閉じればタブごと畳まれるので後片付けも増えない。
  項目 813 の `make_fixture_pane` と同じ手）。分割で作っていた終了状態
  （分割元のタブがアクティブ・分割元にフォーカス）は `Focus` で戻す
- **dispatch の応答は固定回数の窓で待たない**（#1162）。`for _ in 0..20 { wait(cx, 300) }` は
  「20 回 × 300ms = 固定 6 秒」の予算で、混んだ機では応答の形になる前に使い切る。
  `wait_for_dispatch_state`（`wait_for_app_state` の `&mut TakoApp` 版）で状態を待ち、
  上限は `state_wait_budget` で機の混み具合に応じて**伸ばすだけ**にする
  （縮めない = 環境で判定が緩くならない / 4 倍で打ち切る = 回帰があるとき待ち続けない）。
  番犬テスト `dispatchの応答を固定窓で待っていない` が違反行を名指しで落とす
- **「増えた数」を固定窓で測らない**（#1153）。総数の差分は、窓のあいだに
  **関係のない何かが減る**と成立しない。項目 657 は `MenuInvoke` の直後 2.4 秒のうちに
  先行項目の検証用ペインがコマンド終了で畳まれ、`invoked=true tabs=8->4` = アクションは
  発火しているのに `tabs().len() > before` が偽になっていた（#1124 の実測）。
  **作ったものの ID が押す前の集合に無いか**で見れば、他のタブ・ペインの増減に依らない
  （注入 `TAKO_1153_INJECT=tabgone` の実測: `tabs=9->9 new_tab=Some(33)` で新経路は通る）。
  番犬テスト `固定窓のあいだに増えた数を測っていない` が違反行を名指しで落とす。
  「N 行増える」を固定窓で測る形も同じ穴で、**混み具合で 1 行の間隔が伸びる** fixture
  （POSIX の `sleep` は 1 行ごとに fork + exec が入る）では窓を使い切る → 状態待ち +
  `state_wait_budget`。**窓を伸ばしたら、窓の長さに比例する上限（再確認の回数など）も
  同じ率で伸ばす**（項目 113 の `hops_bound`。実測: 窓 6.5 秒で 80 → 100）
- **表示状態を読む前に「材料が揺れていないか」を状態で待つ**（#1153）。
  `pane_display` の材料はどれも時間で解ける途中状態を持つ: `command_state` は
  OSC 133 D が届くまで `Running` / `busy_children` は sleep_guard の 2 秒 tick が
  更新するまで真 / 生成直後の猶予（#720）が生きていれば `Preparing`。
  **とくに猶予は role を付けると長い方（`SettleKind::Agent` = 25 秒）へ切り替わる**
  （`PaneSettle::state`）ので、role を貼った直後に 1 回読む形は
  `display=preparing` / `settling=true` / `reason=null` を読んで**25 秒間ずっと**落ちる
  （#1153 / #1124 の実測。`prune_pane_settle` の 2 秒 tick が追いつく前に読むと踏む）。
  **混み具合と単調に対応しない**（load 1.63 の静かな機でも落ちた）ので、待ちの長さでは
  なく状態で待つ。診断行には材料そのもの（`pane_display_diag` = `display` / `reason` /
  `state` / `has_role` / `busy_children` / `released` / `settling`）と `waited=` を毎回出す
- **画面のエコーを固定窓で待たない**（#1165）。`for _ in 0..8 { wait(cx, 800).await;
  ok = focused_contains(…) }` は「リトライループで延ばしてある」ように見えて**予算が
  固定**（8 × 800ms = 6.4 秒）で、混み具合に追従しない。項目 1b（TERM / COLORTERM 注入）が
  load 12.9 でこれを使い切り、**最初の項目なので 1c 以降が 1 つも走らなかった**
  （#1165 の実測）。`type_until_focused_text`（状態待ち + `state_wait_budget` の上限 +
  上限つきの送り直し）へ寄せる。**送り直しに Ctrl-C を挟まない**のは、待っている相手が
  自分で終わる短命なコマンドだからで、進んでいる最中に殺すと判定が却って不安定になる
  （画面を保持し続ける fixture を扱う項目 102 は Ctrl-C を挟む = 事情が違う）。
  番犬テスト `固定窓のあいだに画面の文字列を待っていない` が違反行を名指しで落とす。
  **既存の番犬（`画面の文字列を待つ検査は固定待ちに依存していない`。#796）は固定待ちの
  「直後」しか見ないので、リトライループに包むと見逃す**——アンカーは #1153 の
  「`for _ in 0..N {` の次行が `wait(cx,`」を共有し、肯定形かどうかの判定
  （`has_positive_focused_contains`）も 1 実装を共有する
- **最初の項目が落ちたら「なぜ」までログに出す**（#946）。項目 1b（TERM / COLORTERM 注入）は
  **最初なので落ちると 1c 以降が 1 つも走らない**。失敗時は `TAKO_SELF_TEST_946` の 1 行に
  `cause=`（`injected` / `inherited` / `unexpected` / `no-echo`）と親プロセスの TERM /
  COLORTERM を出し、「注入が効いていない（親から継承した）」と「エコーが返っていない
  （待ち・入力経路）」を**ログだけで切り分けられる**ようにする。判定は
  `tako_core::terminal::diagnose_termchk`（純関数）で、GUI 無しで全分岐を単体テストできる。
  **SKIP にはしない**（上の #771 と同じ理由: SKIP は本物の回帰も隠す）
- **実 claude の応答を固定窓で待たない**（#771）。`TAKO_SELF_TEST_CLAUDE` 系
  （45c=#28 / 95c=#716 / 97c=#720 / 101c=#749）が待っている相手は**実 LLM** なので、
  応答までの時間は機の混み具合で桁が動く。項目 101c は 600 × 500ms = **固定 300 秒**の窓で
  待っていて、load1 が 11〜79 の機では 2/2 回 `saw_marker=false` になっていた（#771 の実測）。
  `wait_for_claude_state`（状態待ち + `state_wait_budget` の上限）へ寄せる。要点は 4 つ:
  - **上限に届いても SKIP にしない**。SKIP は回帰も一緒に隠すので FAILED のままにし、
    切り分けは診断行でやる（`TAKO_SELF_TEST_771` = `ok=` / `waited=` / `budget=` /
    `capped=` / `load=`、諦めたときは加えて `TAKO_SELF_TEST_WAIT_TIMEOUT` の
    `screen_tail=`）。`capped=true` = 4 倍まで積んでも足りなかった印
  - **画面末尾は「見ていたペイン」から採る**（`pane_screen_tail`）。実 claude の待ちは
    フォーカスとは別のペイン（引き継ぎの後任・チャット用の専用ペイン）を見ているので、
    フォーカスの画面を貼っても切り分けの材料にならない
  - **駆動（信頼ダイアログの承諾）は毎周期・観測だけを予算で待つ**。`step` の第 2 引数
    （`observe`）がそれで、注入のあいだも相手は前へ進む（承諾を止めると claude が
    起動せず、遅れの再現ではなく別の失敗になる）
  - **素の上限は旧の固定窓より広く採る**（101c は 300 秒 → 360 秒）。等しいと
    「空いている機では旧と同じ」= `TAKO_771_LEGACY=1` との A/B が取れない。
    不等式（旧の窓 < 注入の遅れ < 新の素の上限）は
    `注入の遅れは旧の窓を超えて新の上限に収まる` が**ソースから採った全呼び出し**に対して
    検査する

  番犬テスト `実claudeの応答を固定窓で待っていない` が違反行を名指しで落とす。
  **既存の番犬（#1153 / #1165）は「`for _ in 0..N {` の次の行が `wait(cx,`」しか見ない**ので、
  待ちがループの**末尾**にある形（`… if ok { break; } wait(cx, 500).await;`）を見逃す
  ——実 claude の待ち 14 か所のうち 4 か所がその形だった。#771 の番犬は本文を
  インデントで閉じ括弧まで採り、`break` を持つループ（= 待ちの結果を判定に使うループ）だけを
  名指しする（片付けの Ctrl-C 連打は何回叩いたかだけが意味を持つので対象外）。
  番犬が region を見つけられるのは**ブロックの入口の行に gate がある**ときだけなので、
  実 claude の e2e を足すときは `if claude_e2e_enabled("<tag>") {` のように
  ブロックを開く行で名乗る（#716 は `if let Some(chat_pane) = claude_pane` だったため
  どの番犬からも見えず、11 か所の固定窓が残っていた → `.filter(|_| claude_e2e)` を足した）
- **製品側のタイムアウトはテスト側の予算では解けない**（#771）。`saw_marker=false` には
  ①テスト側の窓を使い切った ②送達フロー（`PromptFlow`）が **120 秒**で諦めた
  ③実 claude がまだ考えている、の 3 通りがある。②はテストの待ちを伸ばしても変わらないので、
  診断行に `prompt_flow=`（対象ペインのフローの状態 / `none` = もう無い）を出して
  区別できるようにしておく
- **疑似 TUI の fixture は「ペインの起動コマンド」で描く。打ち込むなら準備を待つ**。
  既にあるペインへ打ち込む形は Windows で 3 通り壊れた（#903 の実測）:
  ①状態切替の Ctrl+C で**器（psmux）の client が終了**し外側 PTY ごと死ぬ
  （client 自身が PowerShell スクリプトなので pipeline ごと終わる）②**器越しの打鍵から
  非 ASCII が落ちる**（`─` / `❯` が消えて ASCII の本文だけ残る。器の中のシェルが自分で
  印字する経路は無傷だと対照実験で確認 → 製品側の疑いは #907）③起動途中の PTY は
  打鍵を落とす（#640）。状態を切り替えたいなら**ファイルの書き換えで描き替える**
  （`ShellDialect::repaint_file_loop`。変化が無ければ描き直さないのでちらつかない）。
  番犬テスト `打ち込む疑似画面のfixtureはシェルの準備を待っている` が
  `paint_and_hold` の使い方を「起動コマンドとして渡す」か
  「`wait_for_pane_ready` で待ってから打ち込む」の 2 通りに縛る
- **ペインで走らせるシェル片は PowerShell では `-EncodedCommand` で渡す**
  （`ShellDialect::shell_snippet_command`）。器（psmux）は内側コマンドを**自分で
  単語分割する**ので、引用符入りの `-Command '<片>'` は届く前に壊れて**セッションが即死**する
  （#875 が実行ペインで踏んだ 3 層問題と同じ。実機 A/B: `-Command` は
  `no server running on session …`、`-EncodedCommand` は生存して画面を描いた）。
  base64 は `A-Za-z0-9+/=` だけなのでどの層も通り、非 ASCII も UTF-16 のまま運べる。
  符号化は `platform::shell::encode_powershell_command` の**1 実装**を共有する
- **PTY 起動の失敗理由を捨てない**。セルフテストの `spawn_session` の `Err` を捨てると
  「起動できなかった」が「画面に出ない」として現れ、原因が fixture 側にあるように見える
  （#903 が長引いた理由の 1 つ）。`spawn_error` を診断行に出す
- **シェル統合を要る項目は「配置されているか」でゲートしない**。Windows の
  `shell_integration::status().installed()` は「`$PROFILE` のブロックが *いまの
  data dir の* `tako.ps1` を指しているか」で決まるので、**`TAKO_ISOLATED=1` が
  data dir を pid ごとに変える隔離セルフテストからは配置が見えない**。ここをゲートに
  すると同じ機・同じコードでも起動の仕方で「skip される回」と「走る回」が入れ替わり、
  レシピどおりに回すと**その機能が永久に未検証**になる（項目 41 / 41b の OSC 7 / 133 が
  これで、#1073 の症状の半分を作った）。問うのは「統合を読ませたシェルを**起こせるか**」で、
  `ShellDialect::integration_shell_command`（統合スクリプトを自分でドットソースした
  対話シェル。POSIX は spawn 時の env 注入で完結するので `None` が正しい）で
  **専用ペインを 1 枚立てて中で完結させる**（#889 の項目 93 / #1091 の項目 41）。
  判定は配置状態を引数に取らない純粋関数（`osc_selftest_runnable`）へ置くと、
  ゲートが配置へ戻ることが構造的に起こらなくなる
- **専用ペインへ移すときは「起こした場所」が答えになっていないか確かめる**。
  ペインの `cwd` は spawn 時の値がそのままセッションへ入る（OSC 7 を待たない）ので、
  期待値と同じ場所で起こすと `cd` が 1 文字も届かなくても cwd 検知が成立する。
  **期待値とは別のディレクトリで起こし、その前提自体を `check` で見る**（#1091）。
  同型の穴は「分割元の cwd が既に期待値」でも生える → 継承の判定には
  **フォーカスが別のペインへ移ったこと**を必ず含める（`split_inherited_cwd_ok`）
- **visual-test の節は「自分が撮る場面」を節の頭で自分で作る**。全節実行は 1 プロセスで
  節を順に回すので、前の節が残したペイン・その出力・ツリーのルート（削除済み fixture を
  指したまま）がそのまま次の節の画面へ載る。ちらつき節（#932）の `idle-4pane` は
  「素のシェル 1 枚から 3 分割した 4 ペインが静止している」を見るのに、全節実行では
  `terminals=7 distinct=67 changed=72` で**必ず**落ちていた（単独実行は
  `terminals=4 distinct=1 changed=0` = 緑。#1083）。**節の並びで直さない**
  （順序を変えても「前の節が汚す」構造は残り、節が増えるたび再発する）:
  **`ensure_fresh_scene(window, cx, "<節名>")` を節の頭で呼ぶ**（新しいタブへ移り →
  残りのタブを全部閉じ → ルートを本番と同じ経路（`sync_filetree_roots`）で作り直し →
  プロンプトが出るまで状態で待つ）。**用意した前提そのものも `check` で見る**
  （前提が崩れたときに「検査対象が壊れた」と読み違えないため。ヘルパーがやる）。
  最後のタブは閉じられない（`close_tab` が `LastTab` を返し UI 層がアプリを
  終了させる）ので、**新しいタブを作ってから**残りを閉じる
- **節を足したら `TAKO_VISUAL_ONLY` の腕と全節実行の並びの両方へ載せる**。腕にしか
  無い節は「最終確認」で**一度も走らない**（#948 で `preview_code_visual` /
  `remote_tree_visual` / `screen_lines_visual` の 3 本がこれだった。`remote_tree_visual`
  が全節実行から呼ばれないせいで #1072 は全節実行をすり抜けている）。**grep の件数では
  気づけない**（腕にも呼び出しがあるので 1 件は必ずヒットする）ので、入り口へ
  `inject_section_failure("<節名>")` を置き、`TAKO_948_INJECT=<節名>` を付けた全節実行が
  **その名前で FAILED になること**で「実行経路に載っている」を機械検証する。
  置き場は**自分で場面を作る節をまとめて `flicker` の直前**が安全（前は自分で作るので
  依らず、後ろは自分でも作り直す `flicker` だけなので「後続を汚す」経路が構造的に無い）
- **ピクセルの期待値を device px のリテラルで書かない**。visual-test の閾値は
  **表示スケールから作る**（測った矩形 × `scale` から面積を出して割合で見る）。
  `ul_strip >= 40` は scale=2 の機でしか成立せず、**scale=1 の機では同じ絵なのに
  1/4 の値（32）になって必ず落ちた**（#943 の報告値。閾値だけがスケールに追随して
  いなかった）。併せて**閾値が空振りしない前提**（面積が 0 に潰れていない）も見る
- **到達手段が変わる検査は、前提を診断行に出して「期待値の出どころ」を切り替える**
  （#943 → #1177）。**器の有無で節を飛ばさない**。ホイールの半行スクロール（#159）は
  `session.scroll_pixels` を通る**直接ペイン**では端数が `screen.fract` に載るが、器
  （tmux）つきのペインは `mirror_scroll_pane` → `backend_scroll_px` のミラー経路
  （capture ベース・1 フレームでは立たない）へ入り、端数はミラーの `position`
  （`ceil(pos) - pos`）に載る。前提を見ずに測っていたため `TAKO_PERSIST=1` を付けた
  実行では `fract=0.000 shift=0 zero_cost=0.00`（フレーム完全同一）で必ず落ち、
  **以降の節が 1 つも走らなかった**（#943）。#943 はこれを「器つきなら理由つきで
  飛ばす」で止めたが、**飛ばす必要はなかった**（#1177）: `settle_scroll_mirror` で
  ミラーが立つまで状態で待ち、期待値の出どころだけを**描画側と同じ式**
  （`subline_fract` の「ミラーがあれば `ceil(pos) - pos`、無ければ `screen.fract`」）
  で切り替えると、器つきでも直接ペインと**1 桁も違わない**値になる（実測は下記）。
  実フレームから採る量（`shift` / `first` / `last` / `ink_rows`）は器で分岐しない。
  **診断行（`mirrored=` / `mirror_pos=`）は残す**（「直接ペインのはずが器つきだった」
  = 隔離の取りこぼしに気づける唯一の手がかり）。器つきの skip は既定のレシピ
  （`TAKO_ISOLATED=1` 単独 = persist OFF）では走るぶん気づきにくく、**実ユーザーに
  近い経路（persist ON = 器つき）だけが永久に未検証になる**（#1091 の教訓）。
  回帰は番犬 `器つきだからと視覚検査の節を飛ばしていない` が落とす
  （器の有無で分岐する `if` の直後に skip の目印を出す形を名指しする。
  「未描画だから測れない」型の skip は器に依らないので対象外）

  | | `term-grid scroll`（#1177 の実測） |
  |---|---|
  | 器つき | `mirrored=true mirror_pos=0.500 fract=0.500 shift=17 expected=17` |
  | 直接ペイン | `mirrored=false mirror_pos=- fract=0.500 shift=17 expected=17` |
- **前提は「見て飛ばす」より、作れるなら**製品の経路で**作る**（#1122）。器つきの
  カーソルラウンドは `fill=272 → 0 → 0 → 0` で落ちていたが、原因は tmux クライアントが
  カーソルを描くことではなく**測っているあいだペインがライブ画面を映していなかった**こと。
  前段の scroll ラウンドが残したミラーが立っていると `compose_mirror_lines` は表示行を
  tmux 履歴へ差し替えるので、ライブのカーソルは 1 px も出ない（`grid_geom` が読む
  `session.screen()` はライブなので**位置だけは返り続ける** = 「位置は在るのに塗られて
  いない」に見える）。実ユーザーの打鍵は必ず `cancel_scroll_before_input` を通って
  ライブへ戻るので、**検査も同じ前処理を通してから書く**。これで器つきでも 5 通りが
  直接ペインと同じ数値（`fill=544 / area=532`）で緑になり、skip が要らない
- **測った矩形が 0 でも「描かれていない」と決めない。インクがどこへ乗ったかを格子へ写す**
  （#1122）。`fill` だけを見ていると、たまたま重なった別物（`fill=272` = ミラーの 0.5 行
  ずれで半分だけ見えた行）と、無関係な場所の異物（`stray=816`）を「製品がカーソルを
  描かない」と読み違える。`ink_total`（ペイン全体のその色のピクセル）と
  `ink_best`（最大のセルとその数）を診断行へ出すと、**測ったセルと一致するか**で
  1 行で切り分けられる（直接 `ink_best=Some((32,0))(544)` = 一致 / 器つき
  `ink_best=Some((9,3))(544)` = 不一致 = 別物を見ている）
- **履歴（capture 由来）のスナップショットにカーソルを焼かない**（#1122 の製品側）。
  `scroll_mirror::parse_ansi_lines` は捨てる `Term` へ tmux の capture を流したあと
  `snapshot`（`show_cursor = true`）を撮っていたため、その `Term` のカーソル位置
  （= 最後にパースした行の末尾）へ `bg = theme.cursor` が乗り、**器つきペインで
  スクロールバックすると履歴にカーソルの四角がチャンクごとに残っていた**。
  カーソルはライブ画面の側にあるものなので `show_cursor = false` で撮る。
  回帰は `履歴行にカーソルを焼き込まない`（tako-core の単体テスト）が固定する
- **器つきで落ちる節は「器つき専用の欠陥」ではなく、たいてい往復の遅れ**（#1173）。
  `subline` 節（半行スクロール）は #943 と同じ形で `direct=0` になっていたが、
  **待てば直接ペインと 1 桁も違わない値になる**: ホイールの端数はミラーの
  `position` へ**非同期に**載り（`0.500`）、描画側も `pos.ceil() - pos` を
  そのまま使う（実測 `mirrored=true mirror_pos=0.500 direct=13961 shifted=0` =
  直接ペインの `mirrored=false direct=13961 shifted=0` と同一）。**skip を足す前に
  1 回待ってみる**（#1122 の「作れるなら製品の経路で作る」の続き）
- **固定 ms の窓は器（tmux）つきでは足りない**（#1173）。器つきの操作は
  「外側 PTY → tmux サーバー → 中のシェル → 出力 → PTY 経由で返る」の往復になるので、
  `timer(800ms)` のような固定窓は 1 回前の画面を測る（実測: 入力欄の高さが
  3 回とも `29.0`px = 全部 1 行 / `typed_changed=0`）。**待ち先は「そのあと測る量を
  決めている状態」**にする（入力欄なら `chat_input_mirror(...).total_rows` や
  `has_text`。画面の文字列ではない）
- **paint 中に置かれる値は、汚してから 1 枚描かないと古いまま**（#1173）。
  `chat_input_bounds` / ペイン本体のインク行数は**塗った結果**なので、状態が届いても
  誰も `cx.notify()` しなければ `PaneBody` のキャッシュ（#786）が再利用されて
  前の値が残る。しかも**セッションの `resize` は描画の途中で起きる**
  （`render` がテキスト領域を測ってから `session.resize`）ので、その回のフレームは
  resize **前**の画面を塗って終わる = 「27 行あるのに 20 行しか塗られていない」。
  行数を変える検査は `settle_inked_rows`（状態待ち → notify → 2 枚描く）を通す
- **注入した fixture は `pin_chat_fixture` で守る**（#853 の機構・#1173 で踏んだ）。
  会話の定期読み取り（`collect_chat_targets`）は **`backend_sessions` = 器つきペイン
  だけ**を対象にするので、persist OFF では誰も読みに来ず fixture が生き残り、
  persist ON では「実 claude ではない」と正しく判定されて消える。
  **器の有無で通ったり落ちたりする検査**はこれを疑う

- **器（tmux）と外のプロセスの往復を固定窓で待たない**（#1180）。相手が別プロセス
  （tmux サーバー / attach クライアント / CLI + IPC / webview の ipc）の往復である
  検査は、混み具合で素直に遅れるので固定 N×M 窓では足りない回が出る（#1175 の検証中に
  項目 68 の attach が 1 回・項目 73 のホイールが 2 回止まり、どれも再実行で通った）。
  `wait_for_backend_state`（状態待ち + `state_wait_budget` の上限・毎周期の駆動）へ寄せる。
  移送したのは 18 か所（#159 のバックエンド 10 / tmux open・ビューペイン 3 /
  worker_status 1 / Web ビュー 3 / 裏タブの幅 1）。
  - **「期限のある状態」と「溜まる状態」を混ぜて測らない**（#1180 の真因）。項目 73 は
    ①ミラーが立つ（capture の往復で**待てば立つ**）②スクロールバーが出ている
    （最後のスクロールから **1.4 秒で消える** = `SCROLLBAR_SHOW_MS + SCROLLBAR_FADE_MS`。
    FR-2.5.13 の意図した挙動）を **1 回のホイールのあと同時に**見ていた。混んだ機で
    ①が 1.4 秒に入らないと②が先に消えるので、**予算をいくら伸ばしても成立しない**
    （実測: `mirrored=true composed_differs=true bar=false`）。直し方は
    **駆動を毎周期に寄せる**（ホイールを打ち続ける = 実ユーザーもミラーが出るまで回す）。
    姉妹項目の 61c は既に `last_activity` を差し替えて期限を決定化していた
    （同じ危険に気づいた痕跡が隣にあった）
  - 診断は毎回 1 行（`TAKO_SELF_TEST_1180` = `item=` / `ok=` / `waited=` / `budget=` /
    `legacy=` / `capped=` / `inject=` + 判定した瞬間の `load=`）。項目ごとの材料
    （`TAKO_SELF_TEST_TMUX_ATTACH` / `_181_VIEW` / `_61F`）は呼び出し側に残す
  - **A/B と注入はタグで項目を絞れる**（`TAKO_1180_LEGACY=<1|all|タグ,…>` /
    `TAKO_1180_INJECT=<late|never>[:タグ,…]`）。`check` は 1 つ目の失敗でプロセスごと
    止まるので、絞れないと手前の項目で止まって観たい項目へ届かない（#1173 と同じ理由）
  - 番犬 `固定窓のあいだに器の往復を待っていない` はアンカーを 2 つ持つ:
    **region**（`has_tmux…` を名乗るブロックの中の固定回数ループ。判定式が何であれ
    名指しできるので「画面が映るのを待つ」形も落ちる）と **needle**（別プロセスが書いた
    ファイルを待つ `read_to_string`）。#1153 / #1165 / #1162 の needle
    （`focused_contains` / `.len() > ` / `read(app)`）は tmux 系の判定式を 1 つも
    含んでおらず、#771 の region は gate が `claude_e2e` 限定だったのですり抜けていた。
    `break` を持たないループ（作成のリトライ・片付けの叩き込み）は対象外。
    Web ビューの 3 か所は `dispatch(\n app,\n web_req("read", …)` の形で行を畳んでも
    `read(app)` にならないので、#1162 の番犬へ `tako_control::dispatch(` を足した

- **「窓が尽きた」は症状で、原因ではない**（#1265）。固定窓のテストが落ちたとき、
  出力から読めるのは「時間内に来なかった」だけで、**窓が短いのか / 相手が動いて
  いないのか**は区別できない。ここを取り違えると窓を伸ばす直しに走って再発する。
  - **フレークを測り直す前に機の資源を採る**（`ls /dev/ttys* | wc -l` と
    `sysctl kern.tty.ptmx_max` / 機上の tmux 本数 / load）。#1265 の元データは
    PTY 404 / 上限 511・tmux 135 本で採られており、tmux **サーバー**が
    `spawn_pane → forkpty → openpty` で止まっていた（`sample` で採取）=
    **テストでも製品でもなく機の資源枯渇**だった。**人工負荷は CPU だけ**にし、
    tmux / PTY を増やす形の負荷は使わない（測っているものが変わる）
  - 測り直しは**修正前のバイナリのまま**回して数字を出す。#1265 は CPU 負荷
    （load 11〜46）で **0 / 150 FAILED**・OSC 7 の到達は 744〜1,171 ms
    （p50 858 ms・90 サンプル）で 10 秒窓に 8 倍以上の余裕があり、見立て
    （窓不足）はここで否定された
  - **それでも状態待ちへ寄せる**。予算を伸ばしても資源枯渇は救えないが、
    次に落ちたときに**原因が診断から分かる**ようになる。OSC 7 の 3 本
    （`wait_osc7_cwd` / `probe_osc7`）は器のペインの `#{pane_dead}` /
    `#{pane_current_command}` / `#{pane_current_path}` と `ZDOTDIR` /
    `.zshenv` のバイト数を出すので、**`pane_current_path` が目的地 =
    `cd` は実行済み = パススルー側の不着**、**`capture 失敗` / command が
    `zsh` でない = 器がまだ立っていない**、**`.zshenv` が読めない = 統合が
    置けていない**、と 1 行で割れる
  - **混み具合そのものは再現できないので「遅れ」を注入する**。#1265 の
    `TAKO_1265_INJECT=late` は `SHELL` を「12 秒寝てから zsh を exec する包み」に
    差し替える（資源枯渇で器の起動が伸びた状態と同じ形）。旧アーム
    （`TAKO_1265_LEGACY=1` = 固定 10 秒窓）は Issue と**同じ画面**
    （`"cd /private/tmp\n\n\n…"`）で確定 FAILED になる。検出力の確認は
    `nointegration`（統合の置き場を空へ = **両アームとも FAILED が正しい**）
  - **器の `default-shell` はサーバー起動時の環境から決まる**。テストが自分で
    先行サーバーを立てる形（#1105 の `器のサーバーが別インスタンス…`）は、
    その `Command` にも `SHELL` を渡さないと**そのテストにだけ注入も指定も効かない**
    （実測: 3 本のうち 1 本だけ `late` で落ちなかった）
  - **診断の見出しは Issue 番号で名乗る**（`TAKO_1265_WAIT`）。共通ドライバ
    `wait_for_state` の既定は #1252 のままで、呼び出し側が `tagged()` で
    名乗り直す（CI ログを Issue 番号で grep できる）
  - 番犬 `osc7e2eは状態待ちで待っている` が固定回数の窓（`for _ in 0..N`）と
    自前の `sleep` の復活を名指しで落とす。**回数を違反にできるのはこの 3 本に
    「数える主題」が無いから**で、#1252 の洪水テスト（`for _ in 0..700` で
    ホイールを連打するのが主題）と同じ条件にはできない

## TUI の画面マーカーは「幅で切られる」前提で選ぶ（Issue #1015）

エージェント CLI は**フッター行を自分でペイン幅に合わせて `…` で切る**。だから
「行のどこかに `esc to interrupt` がある」を根拠にしたマーカーは、**狭いペインでだけ**
外れる。実採取（codex-cli 0.153.0・同じ状態を 2 つの幅で採取）:

```text
100 桁: • Waiting for background terminal (1m 08s • esc to interrupt) · 1 background terminal…
 44 桁: • Waiting for background terminal (1m 04s •…
```

44 桁側は `esc to interrupt` も `… (`（スピナー判定の目印）も残らないので busy を引けず、
末尾の入力欄 `›` を拾って **idle** になる。#1015 はこれで `WORKER_IDLE` +
`prompt_undelivered`（自動再送 = 二重指示事故）を誤発火していた。#1132 のとおり
worker ペインは 21〜25 桁まで狭まりうるので、幅の仮定は必ず外れる。

- **見るのは行頭側に来る不変部分**にする。上の例なら「`•` で始まり、`(` の直後が
  経過時間」= 語句が伸びても切られても残る位置
- **語（wording）で判定しない**。同じ語の**過去形**が履歴として残り続けることがある:
  `• Waited for background terminal · <cmd>` は完了済みツールの記録で、実測では
  同一スクロールバックに **5 回**残り、ターン完了後の画面にも
  `─ Worked for 5m 08s ─` として出る。ここを busy にすると worker が
  **永遠に完了しなくなる**（#571 / #120 の「永久 busy」= 不検知より実害が大きい）
- **採らなかった案は機械で否定しておく**。単体テスト
  `i1015_注入_文言で判定する実装は履歴行に誤爆する` は「語で判定する実装」を
  テスト内に置いて、それが履歴画面に当たることを assert する（Issue 本文の提案どおりに
  直すと壊れることの証拠）。番犬 `codex_wait_detection_watchdog` が
  `"Waited for"` / `"Worked for"` / `"Waiting for"` の照合文字列の再登場を落とす
- **画面で裏取りできないものは画面以外で守る**。さらに狭いペインでは経過時間まで
  切られる（`• Waiting for background terminal…`）ので画面判定には穴が残る。
  未達の断定は「一次シグナル（codex なら rollout の `task_started`）を**実際に読めた**
  ときだけ」にして、読めていないときは `prompt_delivery_unverified` へ降格する

## 送達は「届いた / 未達 / 送ったかもしれない」の 3 値で記録する（Issue #1294 / #790）

送達フローが「再送してはいけない」と宣言した顛末（peer の書き込みが始まった後に確認が
取れない = `peer_send_stalled` / `peer_unconfirmed`）を worker レジストリが **bool の未達**へ
潰していたので、`prompt_undelivered` → supervisor の自動再送で同じ依頼が二度渡っていた。
3 値目の宣言は `tako_core::prompt_delivery::outcome_confidence`（顛末コードごとの表）に
1 本化し、記録側（`record_prompt_delivery`）と判定側（`prompt_delivery_assessment_with`）が
同じ表を引く。**既定は未達側**で、再送禁止側へ入れてよいのは「1 バイト以上書いた後」の顛末だけ。

## 一次シグナルを画面で覆すときは「なぜそう見えるか」まで揃える（Issue #1273 / #289）

worker の状態は**一次シグナル**（claude = `agents --json` / codex = rollout /
agy = 実況 JSONL）が正で、画面推定はその下に置く —— これは #571 / #984 / #1033 で
繰り返し確かめた順序で、崩すと「永久 busy」か「偽 idle」のどちらかを必ず作る。
ところが一次シグナルが**構造的に間違う**ことがある。実例が #1273 で、claude 2.1.258 の
`agents --json` は `isLoading || delegatedActive` で状態を決めるため、
**Bash の背景シェルや Monitor が生きているあいだ busy を返し続ける**
（ターンはとっくに終わっていて入力欄も空なのに、watch は永久に `WORKER_IDLE` を出せない）。

こういうときだけ画面で覆してよい。条件は 4 つで、**1 つでも欠けたら覆さない**:

- **「なぜ一次シグナルがそう見えるか」を画面が説明できる**こと。#1273 なら
  状態行の `· <内訳> still running`（= 残っているのは背景作業だけ）。
  説明のつかない busy まで覆すと、単に一次シグナルを捨てたのと同じになる
- **反証がゼロ**であること。生成中の目印（`screen_looks_busy`）が 1 つでもあれば覆さない。
  claude は**生成中も入力欄を描く**ので、空の `❯` の実在は入力待ちの証拠にならない
  （既存テストが `screen_looks_idle(CLAUDE_BUSY_SCREEN_V2)` = true を固定している）
- **画面そのものが信用できる**こと。折りたたみ（`collapsed`）は本文が欠けるので使わない
- **上流が状態を描き分けている**なら、その描き分けに乗る。claude は
  「終わった」ときだけ `· <内訳> still running` を継ぎ足し、
  「背景作業の完了を待って止まっている」あいだは付けない。だから**片方の語を照合する
  必要は無い**（#1015 の「語で判定しない」を守れる）。こちらで推測を足さないこと

**4 つ目は系統ごとに確かめる**（#1277）。同じ「背景作業の申告」に見えても、
描き分けているのは claude だけだった:

| 系統 | 申告 | 生成中にも出るか |
|---|---|---|
| claude 2.1.258 | `· 1 shell, 1 monitor still running` | **出ない**（ターン終了時だけ） |
| codex-cli 0.154.0 | `1 background terminal running · /ps to view · /stop to close` | 出る |
| Antigravity CLI 1.2.0 | フッターの `· 1 task(s) · /tasks` | 出る |

**「生成中にも同じ形で出るか」が試験紙**で、出るなら申告は「背景作業が在る」しか
言っていない = 覆す根拠にならない（覆すと幅で切られた画面で偽 idle。#1015 と同じ事故）。
codex / agy はそもそも一次シグナルがターン終了で idle へ落ちるので覆す必要も無い。
申告は**報告フィールド**（`worker_status` の `background_work`）としてだけ読み、
覆せるかどうかは `wait::declaration_implies_turn_end` の 1 箇所で宣言する。

判定は 1 関数に閉じ、**パイプラインの他の段と同じ判定器を使う**こと
（#1273 は `screen_looks_busy` の自動判別版。dispatch が idle と言った画面を
watch の再検査が busy と読むと `idle_streak` が永久に積まれない = 別の形の不検知）。
系統差は `agent_support` の 1 マスで宣言する（#982）。

**取れないときは黙って旧挙動へ倒れること**も設計に含める。#1015 のとおり TUI の
フッターは幅で `…` に切られるので、狭いペインでは状態行ごと落ちて根拠が消える。
そこで「読めない = 覆さない」= 従来どおり一次シグナルを信じる、にしておけば
劣化はしても誤報は増えない。

### 「入力欄が空か」は文字列では決まらない（Issue #1297）

覆す条件の 1 つ「入力欄に人の下書きが無い」を**文字列だけ**で見ていたせいで、
#1273 の腕は本番でほとんど発火していなかった（pane 1636 / 1761 / 1775 / 1784）。
claude は空欄へ **AI のゴースト提案**を dim で描き、文面は
`merge the PR once CI is green` のような任意の自然文なので、
`INPUT_PLACEHOLDERS` のような**文言リストでは原理的に網羅できない**。

- **属性で決める**。判定は `read_pane` の `input_status.style` と同じ 1 実装
  （`tako_core::screen::analyze_input_line`）を通し、`Ghost` / `None` は下書きなし・
  `User` / `Mixed` は下書きあり。述語の正本は `InputStyle::is_user_draft` の 1 箇所で、
  `matches!(style, User | Mixed)` を判定ごとに書き直さない
  （#1297 はまさに、属性を見る判定と見ない判定が並存していたことで起きた）
- **属性の取り口は「画面テキストを採る場所」と同じ場所で採る**。
  `worker_status` は `collect_worker_status_ctx`（GUI のセッション）で両方を採る。
  片方だけ後から採り直すと、別フレームの画面と属性を突き合わせることになる
- **取れない経路では旧挙動へ落とし、落ちたことを応答に残す**。素の tmux capture には
  属性が無いので従来の文字列判定に戻るが、そのとき
  `worker_status` の `idle_override_blocked` に `input_draft_unreadable` を出す
  （「覆せなかった」と「覆す理由が無かった」を master が区別できる）。
  番犬は `issue1297_ghost_input_watchdog`（判定が文字列だけへ戻る / ghost を
  下書きに数える / user・mixed を空に数える / dispatch の配線が消える の 4 本）

## 効果を測る単体テストは実時間で比べない（Issue #1167 / #1220）

「速くなっている」を `Instant::elapsed` の**比較**で固定したテストは、片方の計測窓にだけ
スケジューリングの待ちが入った回に落ちる（`tako-control` の
`claude_remote_link::tests::追記ぶんだけ読むと定常コストが増えない` が高負荷で
**4 回に 1 回** = #1165 の検証中の実測）。セルフテストの固定窓と同じ話だが、こちらは
待ち方ではなく**測る軸そのもの**を替えるほうが筋が良い。

- **守りたい性質を量で書く**。「追記ぶんだけ読む」なら**読み出しバイト数**、
  「走査を打ち切る」なら読んだ行数、「memo が効く」なら呼び出し回数。
  実測: 4 MB の会話に 1 行追記 → 全走査 4,194,592 B / 追記ぶんだけ 355 B。
  混み具合に依らず、同じ性質を**桁で**固定できる
- **量を観るために製品側へ最小の口を開けてよい**。`scan_from` は
  `scan_source<R: Read + Seek>` へ本体を出してあり（挙動は同じ）、テストは
  読み出しバイト数を数える読み口を渡す。**seek も通る**ので「seek してから読む」全体を測れる
- **上限は絶対値で書き、失敗の側との差を桁で開ける**。比（`incremental * 5 < full`）は
  両方が同じ理由で伸びると成立してしまう。上の例は上限 64 KiB に対して実測 355 B・
  回帰時 4 MB
- **回帰を隠していないことは「全文を読む実装」の注入で毎回確かめる**。とくに
  「全文を読むが `consumed` の報告は正しい」形（意味の検査は素通りする）で落ちること。
  番犬テスト `追記ぶんだけ読む検査を実時間で測っていない` が `Instant` の再登場を名指しで落とす
  （時間を待つこと自体が主題のテスト = `mcp::tests` の遅い応答などは対象外なので、
  規則は 1 ファイルに閉じてある）

### 番犬は 2 本立て（Issue #1220 で広げた）

同じ形が別のファイルにも残っていて、高負荷で 1 回落ちた
（`remote_link_live` の `一覧付与のコストが桁で問題ないこと` = 初回 23.8ms /
2 回目 1.7ms なので、2 回目に 22ms 止まれば反転する）。**規約を書いただけでは
同型が残る**ので、番犬をワークスペース全体へ広げてある。

- `remote_link_watchdog.rs` の `追記ぶんだけ読む検査を実時間で測っていない` —
  `claude_remote_link.rs` のテストは `Instant` そのものを禁止（1 ファイルの強い規則）
- `test_timing_watchdog.rs` の `効果を実時間で比べているテストが無い` —
  **`crates/*/tests/` と `crates/*/src/` の両方**を見て（#962 で src へ広げた）、
  **`assert` の条件部に実時間の値が 2 つ以上**現れる形を落とす
  （`assert!(warm <= cold, …)`）。誤検知しないよう、タイムアウト
  （`while t0.elapsed() < limit`）・報告のみ（`println!`）は対象外。
  コメントと文字列リテラルは潰してから見るので、期待値の文字列に当たらない
- `test_timing_watchdog.rs` の `実時間の絶対予算は宣言したものだけ` —
  実時間の値 **1 つ**と `Duration::from_*` の比較（絶対予算）は、
  `ABSOLUTE_BUDGET_ALLOWLIST` に**根拠つきで宣言したものだけ**許す（#962）

### 絶対予算は「桁が開いている」ことを宣言して置く（Issue #962）

単一の絶対予算（`assert!(elapsed < Duration::from_secs(N), …)`）そのものは悪ではないが、
**落ちる側と桁が開いていないと負荷で反転する**。#962 の
`remote::tests::daemon_stop_implはゾンビpidを終了済みとして扱う` は予算 **2 秒**に対し、
落ちる側（`daemon_stop_impl` のタイムアウト経路）が **5 秒** = 2.5 倍しか開いていなかった。
しかも正常でも観測 1 回ぶんの `/bin/ps`（fork+exec）が詰まれば所要は実測
2.27 / 5.10 / **10.07 秒**まで伸びる。

- **「速いこと」で構造的な性質を代弁させない**。「タイムアウト経路へ落ちていない」を
  実時間で見ると、混んだ機で正しく動いた回が落ちる。見るのは**機構の観測値**:
  `remote::TerminationWait`（どの経路で抜けたか = `via` / 何回待ったか = `polls`）を
  `last_termination_wait()` で読み、`via == Some(Zombie)` を固定する。
  こちらは負荷でも注入された遅延でも変わらない
- **観測は「量を観る口」と同じ作り**（スレッドローカル + `pub fn` の読み口）。
  待ちの本体は観測を差し替えられる形（`wait_for_termination_with`）にしておくと、
  待ち回数と経路の意味づけを**実時間に一切依存せず**単体テストで固定できる
- **桁が開いていると言うなら宣言する**。ソースからは正常時の値が判らないので、
  番犬の `ABSOLUTE_BUDGET_ALLOWLIST` に「予算 / 正常時 / 回帰時」を書いて足す。
  開かないなら測る軸を替える
- A/B は `TAKO_962_LEGACY=1`（旧アサートを再現するアーム。囲みは
  `// TAKO_962_LEGACY_ARM 開始` 〜 `終了` で、番犬が対象外にする）。
  注入は `TAKO_962_INJECT_DELAY_MS=<ms>`（観測 1 回を遅らせる = `ps` の詰まり）と
  `TAKO_962_INJECT=blind_zombie`（#619 前の誤判定 = タイムアウト経路へ落とす）。
  どちらも **`cfg(test)` 限定**なので製品バイナリには入らない

### 量を観る口の作り方（#1011 / #1220）

- 形は `orchestrator::agents_scan_counters`（回数を数える `pub fn`）と同じ。
  `claude_remote_link::scan_counters` は走査回数 / 読み出しバイト数 / 所在探索回数を返す
- **スレッドローカルで持つ**（`cargo test` は並列なので、グローバルだと同じ binary の
  他のテストの走査が混ざる）。`since(before)` で窓の増分だけを見る
- 数えるのは **`BufReader` の下**（実際に file から読んだバイト数）。「全文を読むが
  `consumed` の報告は正しい」形をここで落とす
- **生きているファイルを相手にするときは、窓の中で実際に追記されたぶんを予算へ足す**
  （transcript は claude が書き続けるので、読むのが正しい量。上限だけを固定すると
  追記が来た回に落ちる）。実測: 20 行 / 10 会話の付与は初回 10 走査 5.0 MB /
  2 回目 0 走査 0 B（上限は 64 KiB + 追記ぶん）

### この形を疑ったときの負荷の掛け方（Issue #1208）

**`yes` のような CPU スピナーでは再現しない**。実時間比較が反転するのは「片方の窓にだけ
待ちが入る」ときで、2 回目の窓は 0.2 ms しかないので、純粋な CPU 競合はその窓をまず踏まない
（実測: `yes` 36〜72 本 + `nice -n 20` で旧実装 **0/170 FAILED**）。

再現する負荷は**テストバイナリ自身の多重同時起動**（= `cargo test --workspace` の実態）。
同じ材料を読むプロセスが競合すると page cache と**グローバル memo**が先に温まって
**`cold` 側が 0.2 ms へ潰れ**、2 つの窓が同桁になる（実測: 同時 16 本 × 10 ラウンドで旧実装
**3/160 FAILED**。うち 1 件は Issue と同じ `初回 24.5ms / 2 回目 38.8ms` の形）。

- 疑わしいテストが**同じ binary の兄弟テストと材料を共有**しているなら、まずこれを疑う。
  `--exact` の単独実行は兄弟が memo を温めないので、**いちばん再現しない走らせ方**
- **反転の再現は「直った」の証拠には足りない**。実回帰を注入して検出力を比べること。
  実測: リンク memo を殺すと旧実装は **2/10 しか落ちず 8 回見逃す**（cold も warm も
  同じ理由で ~35 ms になるので `warm <= cold` がコイン投げになる）・所在 memo を殺すと
  **0/10**。量で測る新実装はどちらも **10/10 で確定 FAILED**

## 実画面の行は空白詰め（Issue #1182）

`Screen`（`tako_core::screen`）の 1 行は **`compose_line` が未使用セルまで押し込む**ので、
`ScreenLine.text` は**行末まで空白で埋まっている**（`cell_cols` も全列ぶん在る）。
**単体テストのヘルパ（`links.rs` の `make_screen`）は空白詰めをしない**ので、
この形は素の単体テストでは再現しない。

- **「右端まで埋まっているか」を `cell_cols.last()` で判定してはいけない**。
  空白詰めのせいで常に最終列を指すので、**どの行も soft wrap 扱い**になる。
  `links::combined_screen_text` がこれを踏んでいて、画面全体が改行なしの 1 本へ連結され、
  パストークンが次の行の先頭と融合して実在しなくなっていた（実測: 隣り合う 2 行に
  何か書かれているだけで**ターミナル内のパスリンクが 1 つも検出されない**。プロンプト行が
  続く実画面では常に踏む）。正しい物差しは **`trim_end` した長さの最後の文字が居る列**
- **画面のテキストを解釈するテストは「空白詰めの行」でも書く**。
  `links.rs` の `make_padded_screen` が実画面と同じ形（詰めた `text` + 全列の `cell_cols`）を
  作るので、走査系のテストはこちらでも 1 本用意する
- **`text` を読む側は `trim_end()` を通す**（`visible_lines` 等の既存コードは全部そうしている）

## 画面テキストの切り出しは「区切りを増やす」より「削って実在で決める」（Issue #1283）

ターミナル画面からパスを拾う `links::extract_path_tokens` の区切りは、
**ASCII の空白と `()[]{}<>,;` だけ**。日本語の地の文にパスが埋まると
**文ごと 1 トークン**になり、実在チェックで落ちて 1 つもリンクにならない
（#1283 の実測トークン: `` 動画は`~/…/x.mp4`、確認して。`` がそのまま 1 個 /
`~/…/x.mp4このリンクが` がそのまま 1 個。**素の形だけが飛べていた**）。

- **区切り集合を増やして機械的に切ってはいけない**。全角の括弧・読点は
  `資料（最新）.pdf` のような**実在するファイル名**にも現れるので、区切りにすると
  今まで引けていた形を落とす。引用符・バッククォートも同じ（`John's.txt`）
- 正しい形は「**トークン全体を先に試し、駄目なときだけ**文字種の切り替わり位置で
  前後を削った候補を長い順に試す」（`links::path_candidates`）。**実在するかどうかで
  決める**ので、引けていた形は定義上そのまま引ける
- 削る候補には 3 つの縛りを置く: ①**全 ASCII のトークンでは候補を増やさない**
  （検出は cmd+ホバー毎に走る = 実在チェックの syscall を増やさない）
  ②**`/` で終わる断片は採らない**（`~/dir/存在しない` を `~/dir` へ誤って飛ばさない）
  ③候補数に上限を置く（`MAX_TRIMMED_CANDIDATES` / `MAX_CUTS`）
- **スパンは採用した候補の範囲だけ**を指す（cmd+ホバーの下線が地の文まで伸びない）
- **囲みの引用符は「トークンの先頭に在るとき」しか開き記号にならない**ので、
  前に地の文がくっつく形（`見て`/path with space.txt`、開いて。`）は**空白で
  トークンが割れて**囲みの意味が失われる。**途中のバッククォートでも切る**ことで直した。
  切るのはバッククォートだけ: `'` / `"` は `John's.txt` のように実在するファイル名に
  現れる（バッククォートはシェルで扱えないので実質現れない）。
  残る既知の穴は「バッククォートを含むファイル名」
- **画面テキストを組むときは幅を超えた行を切り捨てず折り返す**（`screen_from_lines`）。
  黙って切ると**囲みの閉じ記号やパスの末尾が消えて答えが変わる**
  （実測: 120 桁の既定で 108 桁のパスを囲んだ行の閉じバッククォートが落ち、リンクにならなかった）
- **既知の限界**: 折り返しの境目に**全角文字**か**パスの中の空白**が来ると繋げられない。
  `combined_screen_text` の「右端まで埋まったか」は `last_visible_col + 1 >= cols` なので
  右端 1 列前の全角文字を「埋まっていない」と読み、行末の空白は空白詰めと区別できない
  （実画面でも同じ情報しか無い）。#1283 の 3 形はどれもここに当たらない
- 検証は `tako links --text <画面の写し>`（GUI と同じ判定を人手のクリック無しで引ける）

## ダイアログの検知は「起点と連続する塊」だけを採る（Issue #1293）

選択肢ダイアログの**起点**（選択カーソル行）は元から末尾 `SCAN_ROWS` 行に絞ってあるのに、
**選択肢の収集だけが画面全体**だった（`dialog::numbered_rows`）。claude は番号つきリストを
日常的に書くので、会話ログの `1. …` がそのまま一覧へ混ざり、3 択が **6 択・番号重複・
`title` が会話本文**になっていた（master はこの一覧で番号を決める = 誤選択と嘘の監査行）。

- **起点を絞ったら収集も同じだけ絞る**。`dialog::numbered_block` は起点から上下へ
  「あいだがダイアログの一部だけ（`gap_line_kind` = 空行 / 説明列の折り返し /
  確定キーの案内 / 罫線）」かつ「**番号が 1 ずつ増える**」あいだしか伸ばさない。
  番号の並びは「箇条書きがダイアログの直上に空行なしで続く」形（あいだが空）を切る唯一の材料
- **本文（`ChoiceDialog.title`）も同じ塊に限る**。境界は罫線に加えて
  「**並びが字下げされているなら 0 桁から始まる非空行**」（claude の会話ログは必ず 0 桁・
  ダイアログの箱は 1 桁以上。箱ごと 0 桁で描く agy では自動的に無効）と
  「**選択肢に採らなかった番号つき行**」の 2 つ。罫線を 1 本も引かないダイアログ
  （auto mode の環境学習の確認）はこれが無いと会話本文がそのまま title になる
- 既知の限界: 箱と同じ桁の残骸は 0 桁の marker が画面から流れ切ると本文に残る。
  **選択肢は汚れない**ところまでが保証範囲（`issue1293_箱と同じ桁の会話ログは本文に残る_限界の固定`）
- A/B は `TAKO_1293_LEGACY=1`。実経路の番犬は模擬 TUI の e2e
  （`crates/tako-control/tests/issue1293_dialog_numbered_block_e2e.rs`）

## セルフテストでターミナル本体を操作するなら表示モードを terminal へ倒す（Issue #1182）

`render_pane` は `pane_display_for` が `Starter` / `Chat` を返すと
**ターミナル本体を描かずに差し替える**。つまり GUI 表示モード（#694）のあいだは
ターミナルの `div`（選択・cmd+クリック・cmd+右クリックのリスナーを載せた要素）が
**そもそも存在しない**ので、合成マウスは 1 度も届かない。

- 症状は分かりにくい: `pane_text_areas` と `cell_at` は**アプリ状態から答える**ので
  座標は正しく引ける。「セルは当たるのにハンドラが呼ばれない」という食い違いになる
- 先行項目が残した表示モードに引きずられるので、**その項目のあいだだけ自分で
  `Request::UiMode` で terminal へ倒し、終わったら元へ戻す**（#1175 と同じ形）。
  前提そのものも `check` で見る（`pane_display_for(pane) == Terminal`）
- 新しいタブのペインは既定で starter になり得る = **項目 147 の前提づくりはここが本体**

## シェルスクリプトで日本語を出すときの変数展開（Issue #837）

**変数の直後に全角文字が続くときは必ず `${}` で括る。** UTF-8 ロケールの bash は全角
`（` などのバイトを**変数名の一部として取り込む**ため、`$var（` は `var\xef…` という
名前の参照になり、`set -u` の下で `unbound variable` で即死する（bash 3.2 / 5.3 の
両方で再現。`--verify` の通し実行で実際に踏んだ）。

```bash
echo "        $registered（$note）"      # ✗ set -u で落ちる
echo "        ${registered}（${note}）"  # ○
```

`bash -n` では検出できず、その行が実行されるまで潜伏する（`build-app.sh` の
「不明な引数」案内は #837 まで気付かれずに壊れていた）。**日本語を出す行は必ず一度
実行して確かめる**か、`scripts/test-launch-services.sh` のようなモックテストで
その分岐を通す。

**番犬（#965 で常設）**: `crates/tako-control/tests/shell_scripts.rs` が `scripts/` 配下の
`.sh` を全部走査して、この形を file:line で名指しして落とす（CI の macOS ジョブで走る）。
行コメントは展開されないので対象外、`$1（` のような位置パラメータも対象外
（bash は数字を 1 桁しか読まないので全角を取り込まない）。手元での洗い出しは:

```sh
grep -nE '\$[A-Za-z_][A-Za-z0-9_]*[^\x00-\x7f]' scripts/*.sh scripts/lib/*.sh
```

## `.app` の差し替えは置き場のパスを空けない（Issue #1042）

**`/Applications/tako.app` を差し替えるときに、そのパスが空になる瞬間を作ってはいけない。**

Dock のピン留めは `.app` への **file URL ブックマーク**（`com.apple.dock` の
`persistent-apps[].tile-data.book`）で持たれ、CNID（inode）を優先して解決する。
「退避 → 新規コピー → 退避先を削除」にすると、置き場が空いた瞬間に追跡側は
「アプリが退避先へ移動した」としか読めず参照をそちらへ書き直し、最後の削除で
その実体が消えてピンが外れる（#1042 で実測確定）。

- 差し替えは **`tako_core::platform::bundle_install::replace_bundle_in_place` を通す**。
  自前で `rename` / `rm -rf` → `cp -R` を書かない
- 手段は 3 段: `Contents/` だけを `RENAME_SWAP`（`.app` の inode ごと不変 = 最良）→
  バンドルごと `RENAME_SWAP` → 退避 → 設置（swap が使えない環境のみ。警告を出す）
- シェル側は写し（`scripts/lib/bundle-install.sh` の `install_bundle_in_place`）。
  検証は `bash scripts/test-bundle-install.sh`
- 番犬は `crates/tako-control/tests/bundle_install_watchdog.rs`（Rust 側・シェル側の
  両方が旧手順へ戻っていないことを file 単位で検査する）
- A/B は `TAKO_1042_LEGACY=1`（修正前の手順をそのまま再現する。計測専用）

## テストの書き先は本番の外（Issue #944 / #1030 / #1022 / #1123）

**`cargo test` は、ユーザーの data dir にもホーム配下の設定にも 1 バイトも書かない。**
`TAKO_DATA_DIR` を渡し忘れた `cargo test --workspace` が本番へ書いていた実測（修正前）:
`<data_dir>` に 20 ファイル（`perf.log` / `persist.log` / `sessions.yaml` /
`task_checkpoints.yaml` / `shell-integration/` / `*-backend.conf`）、data dir の**外**に
`~/.claude.json`・`~/.claude/backups/`・`~/.codex/config.toml`・
`~/.gemini/antigravity-cli/settings.json`・`~/.cache/powershell/`。
診断ログには偽の「メインスレッド専有」が 643 行、`~/.claude.json` にはテスト用 cwd の
事前信頼が数千件積もっていた。

塞ぐのは**書く側ではなく「置き場を決める側」**。3 つの型を使い分ける。

- **data dir 配下 → `tako_core::paths::data_dir()` が実行時に倒す**（1 か所で全部）。
  `cfg(test)` は**クレートを跨がない**（`tako-control` のテストから呼ばれた
  `tako-core::shell_integration::install` は素通りする / 統合テストから見た lib も
  非テストビルド）ので、テストプロセスかどうかを **`current_exe()` の置き場**で判定する
  （`<target>/<profile>/deps/<名前>-<hash>`。`cargo run` も `.app` も `deps/` を通らない）。
  **明示の `TAKO_DATA_DIR` は常に優先**する
- **ホーム配下の外部エージェント設定 → `orchestrator::agent_config_home()`**（`cfg(test)` で
  隔離）。`home_dir()` 自体は倒さない（表示・比較のテストが壊れる）。
  `CLAUDE_CONFIG_DIR` もテストでは**読まない**（テストプロセスの env は本番を指している）
- **外部 CLI の起動 → テストビルドでは起こさない**。`claude agents --json` は claude 本体が
  `~/.claude.json` を書き戻して `~/.claude/backups/` を積むので、単体テストからは起こさない
  （実 CLI を通す検証は統合テスト側 = 非テストビルドの lib を束ねる方が担う）。
  テストが `pwsh` 等を起こすときは `XDG_CACHE_HOME` / `LOCALAPPDATA` を一時 dir へ向ける
- **エージェント CLI への問い合わせは `tako_control::agent_probe::run` の 1 か所を通す**（#1261）。
  tako が 1 バイトも書かなくても、実 CLI は**起動しただけで自分のホームを作る**（空 HOME の実測:
  agy = `~/.gemini` 32 ファイル / codex = `~/.codex/tmp/arg0/…/.lock` / claude = `~/.claude.json`）。
  判定は実行時（`paths::is_test_process`）で、#586 のコンソール窓抑止もこの 1 か所が当てる

- 番犬は `tako_control::test_write_isolation`（lib の**単体**テスト。`cfg(test)` の隔離を
  見るので統合テストには置けない）。**空の `HOME` で子プロセスを起こし、そこに
  ファイルが 1 つも出来ないこと**を実測する（「隔離したフラグが立った」では見ない）
- A/B は `TAKO_944_LEGACY=1`。同じ子が本番相当の場所へ書くことを番犬自身が確かめるので、
  **検査に検出力があること**もテストで固定されている
- 新しい永続ファイルを足したら、番犬の子（`子プロセス_本番相当の書き込みを一通り行う`）へ
  その書き込みを 1 行足す
- **実 claude を使う e2e（`#[ignore]` の手動実行専用）は例外**: worker を既定 config dir で
  走らせる必要があるので事前信頼は**実ファイル**（`~/.claude/.claude.json`）へ書く
  （倒すと実 claude が信頼を読めず e2e が成立しない）。代わりに `Drop` で
  `remove_e2e_trust_entry(&<信頼した cwd>)` を呼んで**自分の分を消す**（#612 / #577 / #1022）。
  `#[ignore]` なので CI は本体を走らせない = 番犬
  `crates/tako-control/tests/e2e_trust_cleanup_watchdog.rs`（`impl Drop for E2e…Guard` を
  走査して後始末の欠落を名指しする）だけが再発を止める
- **過去の残骸は自動で消さない**。`bash scripts/clean-trust-residue.sh`（既定 dry-run。
  実削除は `--apply` で、元ファイルは `.residue-backup.<epoch>` へ退避）が一覧を出すので、
  消すかどうかはユーザーが決める。判定は「一時ディレクトリの下 **かつ** tako のテスト名」の
  両方を満たすものだけで、名前が一致しないものは「要確認」として消さずに並べる
  （モックテスト `scripts/test-clean-trust-residue.sh` が偽の HOME で実プロジェクトの
  巻き添えを落とす。CI の macOS ジョブで毎 PR 走る）

### 使い捨ての置き場は作った経路が消す（Issue #1296）

**`<TMPDIR>/<名前>-<pid>` を作る経路は、消す経路も同じ場所で持つ。**
#944 / #1253 の隔離は「本番の外へ倒す」までで止まっていて、macOS の `TMPDIR`
（`/var/folders/…/T/`）は**再起動でも消えない**ため `cargo test` 1 回につき 1 dir が
積もっていた（実測 2026-09-11: `tako-test-data-*` 2,188 件 + `tako-agent-config-*` 784 件）。

- 後始末は 2 段構え: `tako_core::test_residue::arm_self_cleanup`（`libc::atexit` =
  正常終了と `std::process::exit` の両方で走る）+ `sweep_stale_on_start`
  （SIGKILL / abort で 1 が走らなかった回を、次のテストプロセスが回収する）
- **消してよいのは自分の pid のものと、pid が生きていないものだけ**。名前の一致で
  消すと**並行して走る別 worker の `cargo test`** を巻き込む（#625 の事故クラス）。
  pid は再利用されるので、生きていても「dir の作成時刻より後に始まったプロセス」は
  `reused_pid` として**見送る**（判定は `test_residue::judge` の 1 実装）
- 過去の残骸は自動では消さない口も用意する: `tako test-residue`（既定 dry-run・
  `--apply` で実削除。MCP は `tako_test_residue`）。`clean-trust-residue.sh` と同じ作法
- 番犬は `crates/tako-control/tests/test_residue_watchdog.rs`（`paths.rs` の
  「作る経路」が `arm_self_cleanup` を持つか・接頭辞が `KINDS` に載っているか・
  判定を通さない `remove_dir_all` が増えていないか）と、
  `crates/tako-core/tests/test_data_residue.rs`（子プロセスを起こして**実際に
  dir が消えること / 生きている子の dir が残ること**を見る）の 2 段。A/B は `TAKO_1296_LEGACY=1`

### テスト本体の使い捨てはスコープで消える器で作る（Issue #1312）

#1296 が掃けるようにしたのは**置き場を決める側**（`paths.rs` / `orchestrator` の隔離先）だけで、
テストが個別に `temp_dir().join(…)` で作る使い捨ては射程の外だった
（実測 2026-09-11: `cargo test -p tako-core --lib` 1 回で 14 件残る）。

- **`tako_core::test_residue::ScratchDir::new("<タグ>")`** を使う。スコープを抜けた時点で
  消える（panic の巻き戻しでも `Drop` は走る）。同じタグで何度作っても別の dir になるので、
  並行するテストが同じ名前を取り合わない（#1313 / #1300 と同じ作法）
- **スコープが無いもの**（`OnceLock` に持つ器・子プロセスが使うキャッシュ）は
  `test_residue::process_scratch("<タグ>")`。同じタグなら同じ dir を返し、
  プロセス終了時に親ごと消える
- 置き場の親は `<TMPDIR>/tako-test-scratch-<pid>` の 1 つだけで、**#1296 の 2 段構えが
  そのまま効く**（`atexit` + 次回起動時の pid 回収 + `tako test-residue` から見える）。
  `KINDS` へ種別を足したら CLI / MCP の案内も足す（番犬 `種別はcliとmcpの案内に載っている`）
- 消す経路は `remove_scratch` の 1 本で、**一時ディレクトリ配下であることを確かめてから**消す
- 番犬は 2 段: `tako_core::test_residue` の
  `テストを一巡してもtmpdirに残骸が残らない`（**このテストバイナリをもう一度回して実際に数える**。
  個々のテストの成否では落とさず、残骸の件数だけを見る）と、
  `crates/tako-control/tests/test_residue_watchdog.rs` の静的検査。A/B は `TAKO_1312_LEGACY=1`
  （器を修正前 = 作りっぱなしへ戻す。実測 19 件が残る）

## 実 tmux の e2e は器をプロセスごとに分ける（Issue #1300）

**`-L` に渡す器の名前に固定名を使わない**（`tmux_e2e::socket_for("<Issue 番号>")` が
pid を足す）。同じ機で `cargo test --workspace` が 2 本走ると、固定名は同じサーバーの
同じセッション名を取り合って `duplicate session` で即死し、後始末の
`kill-session -t <固定名>` は**相手のセッションを横から消す**（worktree を分けた worker が
並ぶこの機では日常的に起こる）。

**器を叩いた失敗を素の `assert!(status.success(), "…が失敗した")` で落とさない**。
`crates/tako-control/tests/common/tmux_e2e.rs` の `new_session` / `send_keys` を通せば、
stderr・そのソケットのセッション一覧・PTY / ソケット / サーバー数・load が付いた 1 枚の
診断になる（#1300 の観測は理由が 1 ビットも残らず、枯渇か否かの切り分けに測り直しが
1 往復要った。実測の答えは PTY 103/511 = **枯渇ではなく名前の取り合い**）。
番犬は `crates/tako-control/tests/tmux_e2e_watchdog.rs`、A/B は `TAKO_1300_LEGACY=1`。

## 新しい worktree は PWA のビルドから始まる（Issue #1309 / #574）

`git worktree add` 直後のツリーには `web/tako-remote/dist/` が無い（`.gitignore` 対象）。
rust_embed（`remote.rs` の `PwaAssets`）がそれを**コンパイル時**に要求するので、
放っておくと `cargo build` が `#[derive(RustEmbed)] folder ... does not exist` →
`PwaAssets::get` 未定義で落ちる。

**手作業は要らない**: `crates/tako-control/build.rs` が `dist/index.html` の有無を見て、
無ければ `npm ci && npm run build` を走らせる。**既にある dist は触らない**ので、
ビルド済みのツリーでは所要も生成物も変わらない（CI は #574 で npm build 済みの状態で
ここへ来るため二重ビルドにならない）。共有ツリーから `dist/` をコピーする回避はもう不要。

npm が無い / npm が失敗した環境では**1 行目に手順が出て**止まるので、案内どおり
`scripts/build-pwa.sh` を実行する（PWA ビルドの正本。`build-app.sh` もこれを呼ぶ）。
手順を変えるときは `build.rs` の `pwa` モジュールと `build-pwa.sh` を**対で**直す
（build.rs 側が npm を直接起こすのは、Windows の開発機に bash があるとは限らないため）。

## CI 完了の判定は待ちスクリプトを通す（Issue #1333 / #1313）

**PR の CI が終わったかどうかを自分で判定しない。`scripts/wait-pr-checks.sh <PR>` を使う。**
「`gh pr checks` に出ているチェックが全部 pending でなければ完了」という判定は、
**push 直後に Cloudflare Pages しか登録されていない瞬間**（macOS / Windows の job は
run 開始前なので checks にまだ現れない）に通ってしまう。実際に #1313 = PR #1328 が
CI 完了前に merge され（merge 04:24:10 / Windows 完了 04:48:24）、過去にも #1253 / #1282 で
同じ手順違反が起きている。**存在するチェックだけを見る判定は構造的に早すぎる**。

- 判定は「**期待する名前が全部そろって**、報告されているチェックが全部 completed」。
  さらに**同じ結論を 2 回連続で観測**するまで確定しない（GitHub API は replica 差で
  in_progress / completed が両方向に揺れて返る）。揺れたら 1 行出して観測をやり直す
- 期待する名前は `.github/workflows/*.yml` のうち **pull_request で起動するものの job 名**
  から導く（ハードコードしない。`name:` が無い job は job id）。外部連携の
  `Cloudflare Pages` だけは導出できないので `wait-pr-checks.sh` の `EXTERNAL_CHECKS` が正。
  job を増やす・改名する・matrix を入れるときはここが追従するかを見る
- merge は `scripts/merge-pr.sh <PR>`（待ち → 揃わなければ merge しない → squash +
  `--delete-branch`）。CONFLICTING / BEHIND / BLOCKED は待たずに理由を出して止まる。
  **worker も master もこの 1 実装を呼ぶ**（手順の記憶ではなく構造で守る）
- 終了コードは共通で **0 = 全部緑 / 1 = 失敗・拒否 / 2 = タイムアウト / 3 = 引数・gh のエラー**
- 検証は `bash scripts/test-wait-pr-checks.sh`（偽 gh + 偽リポジトリ。**CI の macOS ジョブで
  毎 PR 走る**）。A/B は `TAKO_1333_LEGACY=1` = 修正前の判定をそのまま再現する腕で、
  Test 13 / 14 が「1 本だけで完了と返る」「揺れの 1 回目で確定する」を固定している
- **緑を見た後に main が進んだら、取り込んで回し直す**。PR の CI が検査しているのは
  ブランチ head ではなく **merge 結果**（`actions/checkout` の既定が `refs/pull/<PR>/merge`。
  実測: PR #1337 の run が読んだ `progress.md` は 12442 bytes = ブランチ head の 11196 でも
  main 単独でもない union マージ後の値）だが、**その merge base は run が始まった時点で凍る**。
  別々に緑だった 2 本が組み合わさって壊れる事故はここを通り抜ける（#1343 = #1295 のテストに
  #1297 が足した `input_style` が無く、main が `cargo check --all-targets` で落ちた）。
  手順は `git fetch origin && git merge origin/main && git push` → CI を待ち直し。
  `merge-pr.sh` は「緑を出した run の後に main の先頭が進んだ」ことを検出して**警告する**
  （材料が取れなければ黙って通す = **助言であって門ではない**。門にすると並走する PR が
  互いに待ち直しを繰り返して収束しない。構造的に締めるならリポ設定側
  「require branches to be up to date」の話になる）
- **警告が出たら、merge 前に main を取り込んで CI を回し直すのが原則**。ただし変更が
  独立していると判断できるなら警告のまま merge してよい（#1343 のような意味的
  コンフリクトのリスクは残る、と分かったうえで通す）
- **merge 後のリモートブランチ削除は `gh` に任せない**（#1347）。`gh pr merge --delete-branch` は
  「ローカルへ切り替え → ローカル削除 → リモート削除」の順なので、**専用 worktree から実行すると**
  最初の切り替えが `fatal: '<既定ブランチ>' is already used by worktree` で落ち、**リモートまで
  到達しない**（PR #1337 で実発生。棚卸しでは merge 済み PR の head が origin に 5 本残っていた）。
  `merge-pr.sh` は MERGED を確認してから**自分で消す**（冪等・**その PR の head 1 本だけ**・
  head == base と fork の PR は触らない）。A/B は `TAKO_1347_LEGACY=1`（gh 任せの腕）

## 設定・データファイルのスキーマ変更（Issue #916）

**永続ファイルの形式や置き場を変えるときは自動移行を同梱する。手動移行を要求しない。**
「自動が難しいので移行手順を提示する」も不可（ユーザー確定方針）。

### どこに何を足すか

1. `tako-control::migrations::SPECS` の該当 `SchemaSpec` の `target_version` を +1
2. 同じ spec の `steps` に `Step { from, to, describe, apply, once }` を 1 本足す
3. `detect` が**内容から**新旧を見分けられるようにする（版数フィールドがあるなら
   `detect_version_field`、無いなら構造の特徴で）
4. `TAKO_UPDATE_SCHEMA_FINGERPRINT=1 cargo test -p tako-control --test migration_registry`
   で指紋を更新する

新しい永続ファイルを足すときは `SchemaId` に番地を切り、`SPECS` と
`migrations::targets` と `config_share::catalog` の 3 つへ載せる
（載せ忘れは `migration_registry` / `config_share_catalog` テストが名指しで落とす）。

### 発火は既に配線されている

- `tako setup` 実行時 = `migrations::setup_lines()`
- 実行時の差分検出 = `migrations::ensure_migrated()`（GUI 起動 / `tako master` /
  dispatch のプロファイル解決。1 プロセス 1 回）
- 明示 = `tako migrate run` / MCP `tako_migrate`

**発火点を新しく足さないこと**。増やすと「どこで直るか」が分からなくなる。

### テキスト置換で表せない移行（分割・置き場の変更）

引き継ぎのプロジェクト単位化（#915）や setup の生成物の置き場の是正（#1019）のように
**分割・置き場の変更**を伴う移行は `Step`（テキスト → テキスト）では表せない。
手順そのものは専用実装が持ち、`migrations::run` から呼んで結果を
[`MigrationReport`] の語彙へ翻訳する（`handoff_reports` / `setup_reports` が実例）。
**番地（`SchemaId`）には必ず載せる**: 載せないと `tako migrate` から見えず、発火点が分かれる。

翻訳の関数は**実環境の env を読む入口から切り離す**。置き場の移設は「移設が要る OS」でしか
Migrated まで到達しないので（#1019 の移設は macOS では新旧が同じ場所 = 決して起きない）、
入口と一体だと翻訳の形（種別・報告するパス・退避先・キー名）が片方の OS で一度も検査されない。

このとき「何が起きたか」は**専用実装のフラグではなく中身の前後比較で決める**。
`MigrationOutcome::migrated` のようなフラグは意味が実装寄り（#915 のそれは
「プロジェクトへ移した行があるか」）なので、形式マーカーの付与だけで済んだファイルが
false になり、`status` の予告と `run` の報告が食い違った。

### 守られる安全要件（機構側の 1 実装が担保する）

| 要件 | 実装 |
|---|---|
| 冪等 | 版数は外部の記録ではなく**内容から判定**（`detect`）。`apply` は「もう当たっている」なら `Ok(None)` を返す |
| 旧ファイルを消さない | 書く前に `<name>.pre-v<from>.bak` へ退避。退避が取れなければ**書かない** |
| 解釈できない内容を捨てない | `validate` が Err なら `<name>.unreadable.bak` へ丸ごと退避して申告（既定値へ黙って落とさない） |
| 秘匿情報の写しを残さない | `preserve_unreadable: false` の種別（`instances/control-*.json` = トークンつき / `remote/devices.json` = Secret）は**退避せず**「読めない」ことだけ申告する。退避は「利用者が手で書いた情報を守る」ためのものなので、寿命の短いトークンつきファイルには当てはまらない |
| 失敗時に元を守る | `apply` が Err なら元のファイルを 1 バイトも触らない |
| 未来の形式を壊さない | ファイルが `target_version` より新しければ触らず `Refused` |
| 実施の可視化 | persist.log へ「移行: <種別> v1 -> v2: <パス>（退避 …・発生源 …）」 |

### 置き場を変える移行は隔離中に走らせない（Issue #1019）

**移設の要否は `TAKO_DATA_DIR` が立っていないときだけ真にする**。隔離した検証
（`TAKO_DATA_DIR` / `TAKO_ISOLATED=1`）の最中に旧い場所を見つけて移すと、
**本番のデータを使い捨ての隔離先へ吸い上げてしまう**。隔離中は見つけても触らず、
本番の実行で改めて移す（移設は冪等なので取りこぼさない）。

判定は**新旧のパスが違うか**で決め、`cfg(target_os)` で分岐しない。#1019 の旧パスは
`<home>/Library/Application Support/tako/setup` で、macOS の既定ではこれが
`<data_dir>/setup` と**同じ場所**になる = 既存ユーザーは自動的に無移行になる。
OS で分岐すると「macOS だけ通る道」が増えて、この等価性が壊れても気付けない。

ディレクトリの移設では「旧ファイルを消さない」を**写してから旧ディレクトリごと
`<name>.pre-v1.bak` へ rename する**形で満たす。rename が済んだ時点で旧い場所が
無くなるので、次回は検出されない = 冪等性の実体が「記録」ではなくファイルの有無になる。
移設先に同名がある場合は**触らない**（新しい場所の内容の方が新しい）。

### 一度だけの移行（`once: true`）

「**利用者が旧い値へ意図して戻す自由がある**」移行だけに使う（例: #27 の `[1m]` 既定モデル
除去。移行後にユーザーが自分で `[1m]` を選び直したら尊重する）。印は**退避ファイルの存在**で
持つので状態ファイルを増やさない。機構より前の手書き移行を取り込むときは
`once_markers` に旧い印の接尾辞を並べる（`.backup-1m`）。

### 冪等性を「記録」で作らないこと

「移行済み」を別ファイルへ記録する方式は #513 の設定共有で必ず壊れる
（マシン A が移行して push → マシン B は新形式のファイルと古い記録を持つ）。
判定は必ず内容から行う。

### やってはいけない `unwrap_or_default()`

永続ファイルを読んで `unwrap_or_default()` / `.ok()` で既定値へ落とすのは、
**その直後の保存が利用者の内容を上書きして消す**ことを意味する。落とす前に
`tako_core::migration::quarantine_unreadable` を通すか、`Err` を返して手を出さない
（実測の被害例: `settings.json` の `theme_colors` / `theme_presets`、
`~/.claude.json` の MCP 登録と信頼済みフォルダ）。

### 排他ロックは「書くと決まってから」取る

`config_io` のロックファイルは**消すと排他が破れる**（新旧 2 つの inode を別々にロック
できてしまう）ので削除できない設計。したがって**書く必要があると分かってから**しか
取ってはいけない。読み取りだけで判定 → 変更が要るときだけロック → ロックの下で
読み直して実行、の順にする。無条件に取ると、全ファイルが最新のときでも起動のたびに
空の `.lock` が増える（#916 の作業中に本番のデータディレクトリへ 167 個作ってしまった。
うち 160 個は `instances/control-*.json` の分）。

### テストが本番の設定を触らないこと

ユニットテストから設定ファイルの書き込み経路を呼ぶときは、隔離が
**テストの実行順に依存しない**ことを確かめる（`OnceLock` の初期化をヘルパー任せに
すると、そのヘルパーを通らないテストが本番へ書く）。`orchestrator::config_dir()` は
`cfg(test)` で必ず隔離先へ倒れる。`migrations::run` は `TAKO_DATA_DIR` が明示されていない
テストビルドでは何もしない。

### 検証プロセスの隔離は `cfg(test)` で書かない（Issue #1253）

**GUI セルフテストは `cargo test` ではなく製品バイナリ**（`tako-app` が
`TAKO_SELF_TEST=1` で立つ）なので、`cfg(test)` も
`paths::is_test_process()` も 1 ビットも効かない。#944 でここを `cfg(test)` にしていた
ぶんが素通りし、隔離セルフテストがユーザーの `~/.claude.json` へ事前信頼を
積み続けていた（残骸 3,050 件のうち約 70%）。

書き先を決める判定は **`tako_core::paths::is_verification_process()`**（実行時）を引く。
真になるのは「テストバイナリ」または「検証のための起動」で、後者の規則は
**窓をユーザーの画面に出すかの判定と同じ 1 実装**（`platform::display::is_verification_gui`）
を引く。片方だけ広げると「窓は仮想ディスプレイ・設定は本番」という半端な状態になる。

| 対象 | 隔離先 | 決めている場所 |
|---|---|---|
| 外部エージェントの設定（`.claude.json` / `config.toml` / `settings.json`） | `<temp>/tako-agent-config-<pid>/` | `orchestrator::agent_config_home()` / `claude_tui::env_config_dir()` |
| シェル履歴 | 同上の `shell_history` | `terminal.rs` の PTY env + `backend::session_pinned_pairs` の `-e` |

- **`config dir` を明示する引数（#558）は隔離しない**。テストが一時ディレクトリを
  指して書き先を確かめる経路と、アカウント指定の spawn がここに乗っている
- **`HISTFILE` を渡すだけでは対話 zsh に効かない**: macOS の `/etc/zshrc` が
  `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` を**無条件で代入**する。rc の後に必ず走る
  precmd で当て直す（`shell-integration/zshenv.zsh` の `TAKO_VERIFY_HISTFILE`）
- **テストプロセスから外部コマンドを起こさない**（#944 の `claude agents --json` と同型）。
  `update_checker::detect_install_method_full()` の `brew` は空 HOME の実測で
  `~/Library/Caches/Homebrew` に 698 ファイル作っていた
- **器を直に起こす e2e は検証用の env を自分で持ち込む**（`tmux -L … new-session` を
  `Command::new` で叩く形）。tako の spawn 経路を通らないので自動では届かない
- A/B は `TAKO_1253_LEGACY=1`（旧挙動を同一バイナリで再現）。番犬は
  `crates/tako-control/tests/verification_isolation_watchdog.rs`（判定の形を走査）と
  `test_write_isolation` / `update_checker`（空 HOME・偽 brew で挙動を実測）の 2 段

## 起動時ロードの予算（Issue #1139）

AI が**起動した瞬間に強制ロードされるもの**には上限がある。書いただけの規約は守られない
（`~/.claude/CLAUDE.md` の「30 件超で archive を提案する」は 3 か月・332 エントリ積もるまで
一度も実行されなかった）ので、**数値を正本の表に置き、規約文をそこから生成し、CI で落とす**。

以下は生成物（正本 = `tako_core::context_budget`）。更新は
`TAKO_UPDATE_CONTEXT_RULE=1 cargo test -p tako-control --test context_budget`。

<!-- tako:context-budget-rule -->
### 絶対に読む範囲（`@import` してよいもの）

- **現在状態**（`activeContext.md` 型）: 80 行以内。「現在の対象 / 直近の観点 / 次の一手」だけを置く
- **作業ログ**（`progress.md` 型）: **直近 5 作業日 かつ 20 エントリ かつ 12 KB 以内**。
  1 エントリは「何を / どこを / 結果」の **3 行以内**にとどめ、詳細は git log・Issue・PR に委ねる
- タスクリスト 1 本（プロジェクトにあれば）

### アーカイブとする範囲

- 予算から外れたエントリは `progress-archive.md` へ **1 行**（`- YYYY-MM-DD #番号 一言`）で移す
- アーカイブは `@import` しない・普段は Read しない
- **90 日より古いアーカイブ行は消す**（git log・Issue・PR が正本なので情報は失われない）

### それ以外の上限

- エージェント規約（`AGENTS.md`）は **30 KB 以内**。長い注記・実測・罠は `.agent/` 配下の
  別ファイルへ出し、規約からは**バックティック参照**で案内する（`@import` にはしない）
- `@import` の合計は **40 KB 以内**
- 引き継ぎの運用メモは 80 行以内 / グローバル指示ファイルは 24 KB 以内
- master / solo の **system prompt は 24 KB 以内**。手順の詳細は
  `tako orchestrator guide <topic>` で必要なときだけ引く形にし、prompt には「いつ引くか」を残す

### 機械強制

- `tako context-budget` で状態を確認し、`tako context-budget fix` で作業ログの移送を自動で行う
  （**冪等・本文は改変しない・全文は git 履歴に残る**）
- 自動で直せないもの（規約の肥大・`@import` の増殖）は `proposals` として直し方が返る
- CI の番犬がこの予算を検査するので、超えたまま merge できない
<!-- /tako:context-budget-rule -->

### 書くときの決まり

- **作業ログへ追記するときは 1〜3 行**。詳細は git log・Issue・PR・`.agent/plans/` に委ねる
  （1 エントリの行数超過は**自動では直せない** = 要約の捏造になるので、書く側が守る）
- 新しいファイルを `@import` へ足す前に、**毎ターン全文を読む価値があるか**を考える。
  無ければバックティック参照にする（`@.agent/architecture.md` と書くと毎ターン全文載る）
- 予算を超えたまま放置しない。`tako context-budget fix` は冪等なのでいつ流してもよい

## 作業ログの衝突は両方残す（Issue #1246）

`progress.md` は「全 PR が末尾へ 1〜3 行追記する」規約なので、worker が 6〜8 本並走すると
**rebase のたびに必ず衝突する**。#1232 / #1241 では解消し損ねたコンフリクトマーカーが
2 回 commit され、`AGENTS.md` から `@import` される先なので**毎ターンの起動時ロードに
そのまま載った**（トークンを浪費し、`context_budget` の件数・バイト判定もズラす）。

### 解き方

- **末尾追記どうしの衝突は、どちらも正しい。両方残す**。自版だけ残して他版を捨てると
  他の worker の 1 エントリが消える（#1244 は消えたぶんを内容突き合わせで復元した）
- 順序は日付順に整える。union が自動解決したときは**エントリ間の空行が詰まることがある**
  ので、`## 見出し` の前に 1 行入れる
- マーカー行（`<<<<<<<` / `|||||||` / `=======` / `>>>>>>>`）を 1 本でも残して commit しない。
  2 回とも「解消したつもりの取りこぼし」だったので、目視ではなく `git grep` で確認する

### 機械強制（2 段）

1. `.gitattributes` の `.agent/progress*.md merge=union` が**追記どうしの衝突を自動で
   両採用**にする（merge / rebase の両方で効く）。ここで衝突自体がほぼ起きなくなる
2. 残った取りこぼしは CI の番犬 `crates/tako-control/tests/context_budget.rs`
   `毎ターン読まれる_md_にコンフリクトマーカーが残っていない` が落とす。
   走査対象は `.agent/` 配下の md 全部 + `AGENTS.md` / `CLAUDE.md`

### union の限界（実測）

union が両採用にするのは**双方が同じ領域を書き換えた**ときだけ。一時リポでの実測:

| 形 | 結果 |
|---|---|
| 末尾追記 vs 末尾追記 | 衝突ゼロ・両方残る（merge / diff3 / rebase とも） |
| 移送（先頭を削る）vs 末尾追記 | 衝突ゼロ・**移送が守られ**追記も残る |
| 移送 2 本が同時 + それぞれ追記 | 同上（消えたエントリは戻らない） |
| **同じ領域を双方が書き換え** | **衝突を報告せず両方残す**（片方の削除が黙って取り消される） |

つまり日常の「末尾追記」「`tako context-budget fix` の移送」は安全で、危ないのは
**既存エントリの本文を書き換えながら片方がそれを消す**形だけ。union は衝突を報告しないので、
そういう編集をしたときは merge のあとに `tako context-budget` を 1 回叩いて確認する
（エントリが戻っていれば予算超過として CI が落ちる）。

なお **union はその PR 自身の rebase では効かない**。属性は rebase 中に checkout されている
ベース側のツリーから読まれるので、この行が main に入った後の PR から効き始める。

### マーカーの例をドキュメントへ書くとき

番犬は**行頭の 7 文字ちょうど**のマーカーを見るので、例示はインラインコード
（`` `<<<<<<<` `` のように行頭に来ない形）で書く。`conflict_resolver_prompt.md` が同じ流儀。
行頭に置いた例は本物と見分けがつかないので、番犬は意図どおり落とす。
なお `=======` だけは Markdown の setext 見出しと同形なので、
**`<<<<<<<` が開いた領域の中にあるときだけ**マーカーとみなす（git は必ずこの順で書くので
検出力は落ちない）。

## 案内文の打鍵は正本から引く（Issue #1203）

画面 / CLI / MCP に出す文章へ `⌘K` / `Cmd+Enter` のようなキー表記を**直書きしない**。
GPUI の `cmd` は platform 修飾で、Windows では **Win キー**へ解決される（#585）。
`Win+K` は OS のキャストが開くので、案内どおりに押すと**別のことが起きる**。
このリポジトリ自身が `keybindings.rs:181-184` に同じ規約を書いていたのに、
2026-09-09 の Windows 実機レビューで 6 か所の直書きが見つかった（= 仕組みはあり、
通し忘れだけが問題）。

- **バインド表にあるキー**（パレット・保存など）は `keybindings::shortcut_hint_for(action, platform)`
  から導出する。案内文へ埋めるだけなら `tako_core::platform::keys`（`command_palette` /
  `save_preview` / `quit`）を引く。両者の一致は番犬
  `案内文の打鍵表記はバインド表と一致する` が **macOS / Windows の両方**について検査する
- **バインド表に無い「修飾 + クリック」「修飾 + Enter」**（確認スキップ・コミット確定・
  設定の確定）は `keys::platform_modifier` / `keys::modifier_enter` を引く。
  **非 macOS は `None`** = その案内を出さない。Win+クリック / Win+Enter は押せない（#763）ので、
  表記を Windows 風に置き換えるのではなく**案内ごと落とす**のが正しい
  （`shortcut_hint_for` が非 macOS の platform 修飾バインドを落とすのと同じ規則）
- 判定はすべて **`Platform` を引数に取る純粋関数**。`cfg!` で分けると
  「Windows でどう見えるか」を macOS の CI で押さえられない（#515 と同じ方針）。
  そのため `key_bindings()` の表も `cfg` をやめ、`bindings_for(platform)` が
  両プラットフォームの表から選ぶ形にしてある（**張るキーの集合は不変**）
- 番犬は 2 本立て。`crates/tako-control/tests/ui_key_notation.rs` が**直書きの混入**を
  ソース走査で止め（規則 A = 案内文を組む場所 / 規則 B = render への直書き）、
  `keybindings::tests::windowsの案内文にmacosのキー表記が出ない` が
  **組み上がった文言**を Windows 構成で検査する。前者だけだと「文言は正しいが
  Windows で嘘になる」を拾えず、後者だけだと新しい直書きを未然に止められない
- セルフテストの項目名（`check(cond, "visual-test md: ⌘C 相当の…")`）と
  実装の意図を書くコメントは対象外（画面に出ない診断文）。
  claude の TUI から採った fixture も同じ（tako の文言ではない）

## 個人情報を現行コードへ書かない（Issue #927）

tako は public リポなので、**実ユーザー名・実ホームパス・実ホスト名・実メールアドレス・
実アカウント ID をソース・ドキュメント・スクリプト・設定サンプルへ置かない**
（グローバル規約の最重要ルール。git 履歴は書き換えない = 過去の確定判断）。

混入経路はほぼ 1 本しかない: **実機で採取した出力をそのまま貼る**。
ペインの capture・PowerShell のプロンプト行・`HOME` / `USERPROFILE` の値・
`claude` の TUI バナー（2 行目が cwd）が典型で、#927 の 4 ファイルは全部これだった。

### 書くときの決まり

- ホームパスの名前は**既にあるプレースホルダを使い回す**:
  `testuser` / `winuser` / `山田` / `me` / `u` / `x` / `alice` / `First Last` 等
  （一覧の正は `crates/tako-control/tests/no_personal_data.rs` の `PLACEHOLDER_NAMES`）
- 新しい名前を増やすのは**架空だと一目で分かる語**のときだけ。増やしたら
  `PLACEHOLDER_NAMES` へ追記する（追記しないと番犬が落ちる）
- 実機の採取物を貼るときは、貼る前にユーザー名・ホスト名・パスを置換する。
  置換しても技術的な意味が変わらない（区切り文字・空白の有無・非 ASCII か、が要点）

### 番犬（`crates/tako-control/tests/no_personal_data.rs`）

2 本立てで、片方だけでは穴が残る。

| 検査 | 何を見る | どこで効く |
|---|---|---|
| ホームパス形の名前はプレースホルダだけ | `/Users/<名前>` / `/home/<名前>` / `C:\Users\<名前>` の `<名前>` | **CI**（誰のマシン由来でも落ちる） |
| このマシンの識別子がリポに出ていない | `HOME` / `USERPROFILE` の basename・`USER` / `USERNAME`・ホスト名を**環境から作って**全文検索 | **手元**（値を貼った人のマシンで落ちる） |

パス形になっていない素の語（#927 の `contains("<実ユーザー名>")`）は 1 では
構造的に見えないので 2 が要る。逆に他人のマシン由来の採取物は 2 では見えないので 1 が要る。

**検出語のハッシュをリポに置かないこと。** ユーザー名のような短く形の決まった語の
SHA-256 は総当たりで戻せるので、除去したはずの値を別の形で public リポへ置くことになる。
CI で特定の語も見張りたいときは `TAKO_PII_TERMS`（`,` 区切り・GitHub secret 経由）で
外から渡す。

### 走査するのは「public リポに出るファイル」だけ（Issue #1035）

`git check-ignore` が「**未追跡かつ ignore 済み**」と判定したファイルは走査しない。
`.claude/settings.local.json` のようにツールが手元で自動生成する設定で番犬が恒久的に
赤くなると、「落ちていても気にしない」を誘発して**本物の混入を見逃す**方向に働くため。

- **`.gitignore` へ書いて隠す抜け道は塞がっている。** `check-ignore` は既定で索引を見るので、
  一度追跡下に入ったファイルは ignore パターンに一致しても走査から外れない
  （**`--no-index` を付けてはいけない**）。**ignore で隠したものは tracked 検査（検査 1）が
  最後の砦**で、これは CI で効く
- `git` が無い / リポジトリの外では絞り込まず**全部走査する**（安全側）。
  絞り込みの有無は `[#1035]` の行で毎回 stderr に出る
- 未追跡でまだ ignore もされていないファイル（作りかけの作業物など）は走査に残る。
  それで落ちたときは失敗メッセージが「CI は緑のままなので手元のファイル側を直す」と案内する

## 窓を出す検証はユーザーの画面に出さない（Issue #1141）

隔離 GUI（`TAKO_ISOLATED=1` の tako-app・セルフテスト・visual-test・収録）は検証のたびに
何度も立つ。既定のままだと**ユーザーのメイン画面の前面に窓が出て作業を妨げる**ので、
常設の仮想ディスプレイ（既定名 `tako-vd`）へ逃がす。

```sh
scripts/lib/virtual-display.sh ensure   # 無ければ作る（冪等・消す機能は無い）
# 以降、検証用の起動は何もしなくても tako-vd へ出る
env -u TAKO_SOCKET -u TAKO_TOKEN -u TAKO_PANE_ID \
  TAKO_SELF_TEST=1 TAKO_ISOLATED=1 cargo run -p tako-app
```

- **`-u TERM -u COLORTERM` は要らない**（#946）。ペインの端末申告は
  `TerminalSession::spawn` が**最後に無条件で上書き**するので、tako のペインの中から
  起こしても（親 `TERM=tmux-256color`）GUI から起こしても（親に TERM 無し）
  ペインの中は同じ `xterm-256color` / `truecolor` になる。実測は
  `cargo test -p tako-core --test pane_term_env`（親 TERM 6 系統 + 注入口のアーム）。
  **旧レシピが揃えていたのは #1165 の取り違え**で、項目 1b が落ちていた真因は
  固定 6.4 秒窓の使い切りだった（`-u TERM` の有無と相関して見えたのは偶然）

- **隔離するのは data dir だけではない**（#1253）。`TAKO_ISOLATED` / `TAKO_SELF_TEST` /
  `TAKO_VISUAL_TEST` のどれかが立っていれば、外部エージェントの設定
  （`~/.claude.json` 等）とシェル履歴（`~/.zsh_history`）の書き先も
  `<temp>/tako-agent-config-<pid>/` へ倒れ、終了時に片付く。規則は
  「検証プロセスの隔離は `cfg(test)` で書かない」節

- **隔離起動が立てた tmux サーバーは終わったら消える**（#1192）。`TAKO_ISOLATED=1` /
  `TAKO_SELF_TEST=1` で**ソケット名を指定しなかった**起動は `tako-iso-<pid>` /
  `tako-st-<pid>` を使い、終了時（`on_app_quit`）に自分でサーバーごと落とす。
  **`TAKO_TMUX_SOCKET` を明示した起動は落とさない**（再起動をまたいでセッションを残す
  検証が実在する = #770）ので、明示した検証は**自分で `tmux -L <名前> kill-server`**
  まで書く。SIGKILL や落ちた検証の残骸は `tako tmux cleanup --servers`
  （既定 dry-run。実削除は `--apply`）で回収する。判定は所有 pid の生死なので、
  **他 worker の生きているサーバーは対象にならない**
- **面を指定するのは `TAKO_DISPLAY=<名前 | UUID | index>`**。`TAKO_ISOLATED` /
  `TAKO_SELF_TEST` / `TAKO_VISUAL_TEST` のどれかが立っていれば**未指定でも** `tako-vd` を狙う
- **通常起動は 1 ビットも変わらない**（未指定 かつ 非検証なら置き先は未解決のまま）
- **通常起動**で指定が外れても**起動は止まらない**。既定の面へ落ちて persist.log に
  理由 + 候補が 1 行残る。どこへ置いたかは `tako_check_health` の `display_placement` で読める
- **面が 1 枚も見えないとき、検証用の起動は既定の面へ落ちない**（#1160）。列挙をやり直し
  （100ms × 20）、それでも空なら**窓を 1 枚も開かずに終了する**（終了コード 4）。
  ユーザーの画面に検証用の窓を出すのは「開かない」より悪い
- **面が見えているのに当たらないときは落ちる**（#1160）。その機に置き先が無いということで、
  ここで開かない構えにすると `tako-vd` を配線していない環境（CI・他人の機・**Windows**）で
  検証そのものが回らなくなる。ただし**黙らせない**（起動時に stderr へ警告）。
  理由が persist.log しか無くて気づけなかったのが #1160 の症状の一部だった
- **`tako-vd` は常設。消さない**（ヘルパにも消す機能を作っていない）。ユーザーの
  ディスプレイ構成・解像度・配置・ミラーリングにも触らない。落として良いのは
  **器の管理外へ外れた残骸（孤児）だけ**で、それも `cleanup-orphans` の実行条件を
  満たしたときに限る（後述）

### 別 Space へ逃がすのでは代わりにならない

GPUI は**窓が完全に隠れる**（他窓に覆われる / 表示中でない Space にある）と描画を止める
（#470 の実測。同じ絵が撮れ続け、描画依存のセルフテスト項目が進まない）。退避先は
OS から**実ディスプレイとして見える面**である必要がある。

### 器（BetterDisplay）の応答は当てにしない

`scripts/lib/virtual-display.sh` はこの実測に従って書いてある。器を差し替えるときも同じ:

- `set -connected=on` は**「Failed.」と言いながら実際は繋がる**。`get -connected` と
  identifiers の `displayID` も接続中に off / 0 を返す（実測）。**繋がっている = 窓を置ける**
  なので、判定は NSScreen（`vd_screens`）を正にする
- 接続は **tagID 指定で 1 回だけ**（`-name=` 指定は複数に当たり同じ画面が何枚も繋がりうる）。
  tagID は操作のたびに並べ替わるので**打つ直前に引き直す**（古い tagID を使うと黙って空振る）
- 接続直後は **macOS の配置が落ち着くまで座標が動く**（3 枚目を繋いだ直後は既存の面と
  同じ x を返し、数秒後に本来の x へ移った）。座標が要るときは `bounds` を読み直す
- アプリが起動していないと CLI は**応答せず固まる**（`open -g -a` してから待つ・タイムアウト必須）

### ディスプレイが眠ると「面は在るのに tako から見えない」

GPUI の `cx.displays()` は **`CGGetActiveDisplayList`**（gpui の `MacDisplay::all`）で、
gpui 自身が「眠っている機では active な一覧が返るとは限らない」と書いている。
実測（2026-09-07 / #1160）: ディスプレイスリープ中は **`NSScreen` に 2 枚残ったまま
`CGDisplayIsActive` が両方 0** = 列挙が 0 件になる。`virtual-display.sh status --snapshot`
に `tako-vd` が見えているのに persist.log が `候補=[]` と書くのはこれ。

- **物差しを間違えない**: 「窓を置ける」は NSScreen ではなく CoreGraphics の active。
  シェル側も `vd_drawable`（`CGDisplayIsActive` / `CGDisplayIsAsleep`）で同じ物差しを使う
- **`ensure` の完了条件は「起きている」まで**（#1160）。眠っていれば `caffeinate -u`
  （ユーザー活動の宣言）で起こして待ち、起こせなければ非ゼロで返る。
  器・解像度・配置・ミラーリング・Main には触らない
- **証明できるときだけ断る**: `vd_drawable` の材料が 1 行も読めない環境（CI・`osascript`
  が使えない機）では「眠っている」と決めつけない（使える面を止めてしまうため）
- 検証用の起動は**列挙が空のとき**窓を開かずに終わる（#1160）。既定の面へ落ちて
  ユーザーの画面に出るより、開かない方がよい。**面が見えているのに当たらないとき**は
  落ちる（そこで止めると置き先を配線していない環境で検証が回らない）
- 長い検証（セルフテスト・収録）は走っているあいだだけ `caffeinate -d -w <pid>` を当てる
  （ユーザーの省電力設定は変えない）

### 窓を増やすときは置き先を通す

tako が開く窓は**全部** `centered_on_target()` を通す（メイン + 追加ビューポート +
設定 / About / アップデート）。1 枚でも素の `Bounds::centered(None, ..)` が残ると、
セルフテストが開く設定画面などがユーザーの画面へ飛び出す。

### 作業の前後でディスプレイ構成を変えない（Issue #1150）

**受け入れ条件**: 作業の前後で **Main Display と内蔵ディスプレイの有無が変わらない**。
前後で 1 枚ずつ撮って diff すれば示せる:

```sh
scripts/lib/virtual-display.sh status --snapshot > /tmp/vd-before.txt
# …作業…
scripts/lib/virtual-display.sh status --snapshot > /tmp/vd-after.txt
diff /tmp/vd-before.txt /tmp/vd-after.txt   # 差分が無い = 構成を触っていない
```

スナップショットは `clamshell=` / `backend_instances=` と 1 行 1 面（`name` / `frame` /
`id` / `builtin` / `main`）で、並びは名前順に固定してある（列挙順のゆらぎで
偽の差分が出ない）。

- **検証のために一時的な仮想ディスプレイを作らない**。常設の 1 枚（`tako-vd`）を
  使い回す。名前を変えた面を作って検証すると**孤児が残る**（#1150 の実測: 検出力の
  A/B で作った `tako-vd-ab1141` が NSScreen に 2 枚残り、器からは消せなくなった）。
  どうしても要るときは **`DISPLAY CHANGE <操作>` と宣言**し、他の worker が
  描画していないことを確かめてから行う
- **`ensure` は同名が 2 枚以上あると理由つきで止まる**。macOS は同名の面に枝番を
  付けて見せる（`tako-vd（1）`）ので、完全一致で数えると「無い」に見えて
  もう 1 枚繋ぎ、増殖に気づけない
- **`ensure` の締めが Main を守る**: Main が `tako-vd` で内蔵が居るなら内蔵へ戻す。
  **内蔵が居ないとき（蓋閉じ）は何もしない**（戻す先が無い）。外部モニタが Main の
  ときも触らない（ユーザーの構成）
- **孤児の後片付けは下見が既定**: `cleanup-orphans` は列挙して実行条件を判定するだけ。
  掃除は `--apply` で、**内蔵が NSScreen に居る かつ 蓋が開いている**ときだけ走る。
  器の再起動は一瞬すべての仮想ディスプレイを落とすので、条件を満たさないまま撃つと
  **画面が 0 枚になって機械が眠り、走っている worker が全部巻き添えになる**
- **器へ渡す引数は `BetterDisplay help` に在るものだけ**。解釈できない引数を渡すと
  CLI として動かず「proceeding with app launch」= **アプリの追加実体**として起き
  （#1150 の実測: 誤った `create` で起きた実体が 5 時間走り続けていた）、実体が増えると
  同じ仮想スクリーンが何枚も繋がりうる。実体数は `status` / `--snapshot` に出る
