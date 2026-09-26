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
- 公開サイト（`docs/`）のアクセス解析タグは `docs/astro.config.mjs` の Starlight `head` に
  置き、**ID はファイル冒頭の定数 1 か所**（`GA_MEASUREMENT_ID` / `ADSENSE_PUBLISHER_ID`）で持つ。
  GA4 は `takushio2525.com` 一族で 1 プロパティを共有し、AdSense は所有権確認の meta だけ
  （広告は出さないので `adsbygoogle.js` は入れない。`ads.txt` はルートドメイン側に 1 つで足りる）
- **Cookie 同意（Consent Mode v2）は takushio2525.com 一族で 1 実装を共有する**（Issue #1639）。
  docs 側が持つのは 3 つだけ: Starlight `head` の `https://takushio2525.com/consent/consent.js`
  （**gtag より前・`async` なし**。`gtag('consent', 'default', …)` が `gtag('config', …)` より前に
  dataLayer へ入っていないと既定値が効かない）・gtag 初期化の保険 1 行
  （`if (!window.tkConsent) gtag('consent', 'default', {…denied})`。既定値が 1 つも宣言されないと
  gtag は全部同意済みとして動くので、読み込めなかったときは止める側へ倒す）・
  フッター（`docs/src/components/FooterLegal.astro`）のプライバシーポリシーと「Cookie 設定」。
  バナー本体・国判定・ポリシー本文はハブ側が正本なので docs には置かない。
  フッターのリンクは**素の `<a>`** で書く（consent.js が document の click を拾って
  `preventDefault()` するため、フレームワークのリンク部品だと遷移が先に走る）

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

## 絵文字を出さない（UI も CLI 出力も。Issue #217 / #1536 / #1578）

tako が出す文字に絵文字を置かない（ユーザーの確定方針）。理由は**安っぽく見える**ことと、
**フォント依存で描画が揺れる**こと（同じ字が環境によってカラー絵文字・白黒グリフ・
豆腐のどれにもなる）。端末ではこれに**セル幅が 2 になったり 1 になったりする**ことが
乗るので、桁を揃えた出力が環境ごとに崩れる。

- **判定の正本は `tako_core::emoji::is_emoji` の 1 実装**（Unicode の Emoji プロパティ）。
  `×`（U+00D7）・`●`（U+25CF）・`→`（U+2192）のような純粋なテキスト記号は対象外
- **GUI（`crates/tako-app/src`）**: 印が要る場所は `gpui::svg()` + `file_icons::ui_icon`。
  番犬は `issue1536_no_emoji_ui_watchdog`
- **CLI 出力（`crates/tako-cli/src` / `crates/tako-control/src`）**: 印は**文字ラベル**にする。
  語彙の正本は `crates/tako-cli/src/setup.rs` で、`[OK]` / `[情報]` / `[警告]` / `[不足]` /
  `[任意]` / `[失敗]` を使う（新しい綴りを増やさない）。番犬は
  `issue1578_no_emoji_cli_watchdog`
- 走査は `crates/tako-control/tests/common/emoji_scan.rs` の **1 実装**を両番犬が共有する。
  見るのは**本番コードの文字列リテラルの中身だけ**（コメントと `#[cfg(test)]` は対象外・
  `\u{XXXX}` 表記も復号する）
- 残さざるを得ないものは**番犬の `ALLOW` へ「ファイル × 文字 × 件数 × 理由」で載せる**。
  現状の例外は 2 つだけで、どちらも**人の画面に出ない文字**:
  claude TUI を再現するセルフテストの画面データ（#1536）と、
  AI だけが読む MCP ツールカタログの説明文（#1578）

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
- **「比較キー専用」が免罪符になるのは両辺を同じ関数で解決するときだけ**
  （Issue #1569）。`config_share::env::probe_path` は `canonicalize` の戻り
  （`\\?\C:\repo\home\.claude`）を `git rev-parse --show-toplevel` の戻り
  （`C:/repo`）へ `Path::strip_prefix` していた。`strip_prefix` は**成分単位**で
  比べるので `Prefix(VerbatimDisk('C'))` と `Prefix(Disk('C'))` は別物になり、
  同じ場所を指していても必ず `Err` → `unwrap_or_default()` が `repo_rel` を
  **黙って空文字**にする（`tako config` の外部管理検出が Windows で常に誤答）
- **出どころが違う 2 つのパスを突き合わせるなら
  `tako_core::platform::path::relative_under`**（`strip_prefix` を使わない）。
  verbatim prefix を**無条件で**落とし、`/` と `\` の両方を区切りとして割り、
  ドライブ文字だけ大小を無視する。剥がす条件を付けないのは、戻り値が相対表記で
  **Win32 へ渡らない**ため（`strip_verbatim_str` の保留条件は「剥がした結果を
  Win32 へ渡す」ときの話）。`cfg` を書かないので macOS から Windows 形を検査できる
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

## 「ディレクトリか」の判定はリンクを辿る側に揃える（Issue #1398）

`DirEntry::file_type()` と `Path::symlink_metadata` は**リンクを辿らない**、
`std::fs::metadata` と `Path::is_dir` / `is_file` は**辿る**。同じパスについて 2 つの層が
逆を向くと、片方が「ファイル」として見せて片方が「ファイルではない」と弾く =
**押しても何も起きない**画面になる（#1398 の実害。ファイルツリーだけが `file_type()` で、
開く側 = `OpenFile` / `open_plan::route` / `tako file open-in-tako` は全部辿る側だった。
#1399 と重なって理由も出なかった）。

- **表示・振り分け・開く判定はすべて「辿った先の種別」で決める**。ツリーは
  `filetree::entry_is_dir` の 1 実装（リンクのときだけ追加で `metadata`）を通す。
  Windows のジャンクションも `std::fs::metadata` が辿るので cfg の分岐は要らない
- 辿れないリンク（切れたリンク・`ELOOP`）は**ファイル行のまま**にする。中身を出せない
  ものをディレクトリとして見せると「展開しても空」= 理由の出ない静かな失敗に戻る。
  ファイル行なら押したときに `OpenFile` が理由を返し、#1399 の通知欄に出る
- 辿ると**同じ実体へ戻る展開**が起こり得る。深さ上限（`MAX_DEPTH`）だけに頼らず
  canonical パスの照合で打ち切り、**打ち切りを行として見せる**
  （`RowNote::Error`。上の「弾いたら黙って捨てない」と同じ方針）
- 「辿らない」ことに意味がある場所（削除・移動の対象決定、トラバーサル判定）は
  辿らない側のままにし、**なぜ辿らないかをその場に書く**
- 番犬 `crates/tako-control/tests/issue1398_tree_symlink_watchdog.rs` が両側を縛る
  （ツリーが `file_type()` へ戻る / 開く側が `symlink_metadata` へ動く、のどちらも落ちる）

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

## アプリ内テキスト入力は `TextField` を通す（Issue #1450 / #1459）

ターミナルの外（右パネル・サイドバー）にある手書きのテキスト入力は、**本文の `String` +
バイトオフセットのキャレット**という同じ状態を持ち、`floor_char_boundary` で境界へ丸めてから
backspace / delete / 左右 / Home / End / 挿入をする同じコードになる。放っておくと画面が増える
たびにコピーで増え、**片方だけ直した境界バグがもう片方に残る**（#494 の panic が典型で、
右パネルには移送前まで同型の実装が 2 本あった）。

- 状態は `tako-app` の `text_field::TextField`（GPUI 非依存）に持たせ、編集は
  `handle_edit_key` / `insert` / `backspace` / `move_*` を通す。画面側でキャレットを動かさない
- **キーの割り当てはここに置かない**。`enter` が改行か送信か、`escape` が何を閉じるか、
  `⌘V` をどう捌くかは画面ごとに違う。各画面は自分の割り当てを先に見て、**残りを渡す**
  （`handle_edit_key` の戻り値は「編集操作だったか」であって「打鍵を消費したか」ではない）
- 描画で本文を前後に割るときは `split_at_caret()`（丸めが中に閉じている）。生のカーソルを
  `split_at` へ渡すと文字の途中で切って panic し、GPUI の描画中なのでアプリごと落ちる
- 丸めの `floor_char_boundary` は `text_field.rs` の**非公開関数**（#1459）。外へ公開すると
  「呼び出し側で丸めてから自前で drain する」実装が再び生える
- 上限に当たったら `insert` が `false` を返す。**黙って捨てずに理由を画面へ出す**（#1399 系）
- 番犬は `crates/tako-control/tests/issue1450b2_tasks_panel_watchdog.rs`。
  `tasks_panel.rs` / `right_panel.rs` に `floor_char_boundary` / `.drain(` / `char_indices()`
  が現れたら `file:line` で名指しする（**猶予表は空** = 走査は全面適用）

## UI に絵文字を使わない・印は描画プリミティブで描く（Issue #217 / #1536 / #1579）

tako が描く UI に絵文字を置かない（ユーザーの確定方針）。理由は 2 つで、**安っぽく見える**ことと
**フォント依存で描画が揺れる**こと（同じ文字が環境によってカラー絵文字・白黒グリフ・豆腐の
どれにもなる）。

- 印が要る場所は `gpui::svg()` に **SVG のパス**を渡す。アセットは
  `crates/tako-app/assets/icons/ui/*.svg`・定数は `file_icons::ui_icon`。
  **定数を足したら `EMBEDDED_ASSETS` の `ui_asset!` にも足す**
  （漏れると描画時に無言で消える。`file_icons` の埋め込み検査が落とす = #562）
- 色は `svg()` へ `.text_color(...)` で渡す（アセット側の `stroke` / `fill` は目印で、
  GPUI はマスクとして描く）。寸法は `.w(px(..)).h(px(..))` で**必ず明示する**
  （文字と違って親の `text_size` では決まらない）
- 意味が文字で足りるものは**短い文字ラベル**にする。印 + 語で出すなら、カタログ
  （`ui_text`）には**語だけ**を置き、印は render 側で `svg()` を横に並べる
  （カタログへ記号を混ぜると日英の両方に同じ記号が写って増える）
- **異体字セレクタで逃げない**。`▶\u{fe0e}` のように「絵文字をテキスト表示へ倒す」書き方は
  フォント次第で効かないので、番犬は U+25B6 を絵文字として落とす（#1536 で実在した形）
- **絵文字でなくても、印として置くなら止める**（#1579）。`×`（U+00D7）を閉じるボタンに、
  `●`（U+25CF）をライブ印に、`⎇`（U+2387）を tmux バッジに使うのは、Emoji プロパティを
  持たなくても**意味を字形に預ける**形になる。`⎇` は多くのフォントに無く豆腐（□）になり、
  `×` / `●` は太さ・大きさ・ベースライン位置がフォントごとに違うので、隣に並べた
  `svg()` のアイコンと揃わない
- **同じ文字を文中の記号として使うのは正しい**（「20 × 300ms」「612×792 のページ」「A → B」・
  罫線）。だから #1579 は #1536 と違って「ソースのどこにあっても落とす」形にはしない
- **ユーザーのデータは対象外**（タブ名に付けた絵文字・ターミナルの中身・ペインログ）。
  tako が「描く文字」と「預かる文字」を混ぜない

### 機械強制

- 判定の正本は **`tako_core::emoji::is_emoji` の 1 実装**（Unicode の Emoji プロパティ。
  U+1F000〜U+1FAFF と U+2600〜U+27BF は**ブロックごと**採る意図的な過剰検出）。
  `ui_text` のカタログ検査（`all_texts_have_both_languages_and_no_emoji`）と番犬
  `issue1536_no_emoji_ui_watchdog` の**両方がこれを呼ぶ**ので、片方だけ範囲が動かない
- 番犬は `crates/tako-app/src` の全 `.rs` を走査し、**本番コードの文字列リテラルの中身だけ**を
  見る（テスト領域は `production_range::scan`・コメントは `code_view::literals_only` が落とす）。
  `\u{XXXX}` の書き方も復号して見るので、生の文字を避けても素通りしない
- **コメントは対象外**。規約や理由の説明文に禁止文字そのものを書けなくなるため
- 例外は `ALLOW` へ **ファイル × 文字 × 件数 × 理由**で載せる。件数まで持つので、
  同じファイルに別の絵文字が増えても・同じ絵文字が 1 個増えても落ちる。
  現状の例外は `main.rs` のセルフテストが流し込む**ターミナル / claude TUI の画面データ**だけ
  （その文字が出ること自体が検証対象なので消せない）
- **印代わりのグリフ**（#1579）は判定も走査も別に持つ。判定は
  `tako_core::emoji::is_icon_glyph`（`is_emoji` と**同じ 1 ファイル**に置く。範囲の議論を
  1 箇所へ集めるため）、番犬は `issue1579_ui_glyph_icon_watchdog`。表は**明示**で持ち、
  範囲では採らない（`▾` U+25BE / `▸` U+25B8 は `●` と同じブロックにいるのに診断メッセージで
  使われている。**足すときは実測してから足す**）
- #1579 の番犬が見るのは**画面へ出る 2 経路だけ**: GPUI の子要素シンク
  （`CHILD_SINKS` = `.child(..)` / `.children(..)`。綴り違いの抜け道を残さないよう表で持つ）へ
  渡る文字列リテラルと、`crates/tako-app/src/ui_text/` 配下の本番リテラル
  （カタログは全文が画面へ出る）。
  この絞り込みのおかげで**許可リストが 1 件も要らない** —— セルフテストの check ラベル
  （`"ペインの × ボタンで kill"`）や診断メッセージは素通りする。
  例外を足したくなったら、それは UI へ印を出そうとしている合図
- 印を実際に描けているかは**実ピクセルで確かめる**（`ui_asset!` の登録漏れは GPUI が
  無言で何も描かない = #562）。`bash scripts/test-glyph-icon-1579.sh`
  （visual-test の `TAKO_VISUAL_ONLY=glyph-icon` 節。ピン留めプレビューのタイトルバーに
  ライブ印と閉じるが並ぶので 1 枚で両方読める）

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

## OS で変わる値は「両方の列を持つ 1 枚の表」にする（Issue #1655 / #1616）

`cfg(windows)` で本体を出し分けると、**macOS のビルドにもう片方が存在しない**ので、
macOS の単体から「Windows でどうなるか」を 1 マスも検査できない。Windows 実機の
CI ジョブでしか分からない値になり、壊れたことに気づくのが遅れる。

- **表は両 OS ぶんを 1 枚に持ち、`Platform` を引数で受ける純粋関数から引く**
  （`platform::support::MATRIX` / `platform::keys` / `platform::runner_defaults` が同じ形）。
  実行中の OS を見るのは `Platform::current()` を呼ぶ**入口の 1 行だけ**で、
  その中身が唯一の `cfg!` になる
- **同じ値の行は 1 回だけ書く**（`same(…)`）。差がある行だけを `split(…)` で書くと、
  表を読んだときに**差分がそのまま目に入る**
- **その OS で「置かない」と決めた行は表から消さない**。消すと「知らない」と
  「置かないと決めた」の区別が付かず、理由もユーザーへ届かない。理由を
  `Note`（日英）で持たせ、案内へそのまま載せる（対応マトリクスと同じ思想）
- **Windows へ渡すコマンド片は PowerShell 5.1 でも通る形だけ**にする。実行ペインは
  `platform::shell::run_pane_command` が起こす PowerShell で、既定シェルが pwsh 7 で
  なければ同梱の `powershell.exe`（5.1）へ落ちる。`&&` は pwsh 7 専用なので
  `;` + `if ($?) { … }` で書き、パスは `./` ではなく `.\`、生成物は `.exe` を付ける

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

## 実行ファイルの解決は「起動できる形」だけを返す（Issue #1372）

境界 B16（`tako_core::platform::exe`）は「どこに在るか（`find`）」と
「それは実行できるか（`is_executable_file`）」の 2 つの答えを持つ。
**この 2 つを食い違わせない**。`find` が返したパスは必ず `is_executable_file` を満たす。

- Windows の判定材料は**拡張子が `PATHEXT` に在るか**の 1 本（実行ビットという概念が無い）。
  `find` 側も同じ `has_executable_extension` を通し、**土台をそのまま採るのは
  名前が既に `PATHEXT` の拡張子を持つときだけ**にする（`psmux.exe` はそのまま、
  `claude` は `PATHEXT` の順序に従う）
- 「在れば採る」にすると **npm でグローバル導入した CLI が壊れる**。cmd-shim は
  `<name>`（`#!/bin/sh` のスクリプト）/ `<name>.cmd` / `<name>.ps1` の 3 つを置くので、
  裸のスクリプトが `.cmd` より先に採られて `Command::new` が
  「有効な Win32 アプリケーションではありません」で落ちる。**`tako setup` の「見つかった」
  判定だけは通る**ので「未検出」より原因が分かりにくい形になる（#1372 は claude / agy が全滅）
- **起動できないものは `None` を返す**（見つけたことにしない）。`.ps1` しか無い導入も同じ
  （既定の `PATHEXT` に `.PS1` は無い = 直接は起動できない）
- 素の名前へのフォールバック（`Command::new("gh")`）は **`.exe` しか足さない**
  （`std/src/sys/process/windows.rs` の `resolve_exe`。PATH 探索では拡張子が 1 つも
  無いときに `.exe` を足すだけ / パス指定は `.exe` を試して無ければ**足す前のパスを
  そのまま渡す**）。「Windows でも解決できる」の根拠にしてよいのは `.exe` の導入だけで、
  `platform_parity.rs` の B16 免除もその範囲で書く
- 判定は OS 非依存の純粋関数（`find_in_windows_path` / `resolve_with_pathext` /
  `has_executable_extension`）に置き、**macOS 上で走るテストで固定する**。
  整合テスト（`find` の戻り値が必ず `is_executable_file` を満たす）は
  パス指定の経路まで含めて拘束する

## OS に何かを「開かせる」前に形を確かめる（Issue #1376 / #1371）

境界 B8（`os_integration::open_url`）が呼ぶのは **OS の既定ハンドラ**（macOS の `open` /
Windows の `ShellExecuteW`）で、これは「URL を開く API」ではなく**何でも開く API**。
`file:///…`・UNC パス（`\\host\share\payload.exe`）・`C:\payload.exe`・`/usr/bin/…` を
渡せば素直に起動する。tako が開く文字列は**全部が第三者由来**（画面 / md / PDF の
リンク注釈 / 提案チップ）なので、渡す前に形を確かめるのは任意ではない。

- **判定は `tako_core::url_guard` の 1 実装**。経路ごとに `starts_with("http")` を
  書かない（書いた瞬間に「片方だけ緩い」が生まれる = #1376 の PDF と提案チップがそれ）
- **2 段で守る**。経路側（`check_browser_url` = http / https）+ 境界側
  （`check_os_handler_url` = 許可集合）。片方を 1 行外しても穴にならない形にする
- **許可集合を増やすのは「tako 自身が組み立てた URL」だけ**。呼び出し元の棚卸しを
  `OS_HANDLER_SCHEMES` の doc へ書いてから足す
- **1 文字のスキームは URL と見ない**（ドライブレター）。RFC 3986 上は合法でも
  実在しないので、通すとローカルの実行ファイルが「URL」になる
- **弾いたら黙って捨てない**。GUI の `eprintln!` は誰も読めないので「押しても無言」に
  なる（#1283 と同じ穴）。共有の通知欄（`remote_notice`）へ `tr!` を通した 1 行を出す
- **UI から dispatch を呼ぶ画面はすべて同じ通知欄**（#1399 / #1417）。`let _ = dispatch(..)` /
  `if result.is_ok()` で `Err` を捨てると、**ごみ箱移動やリネームの失敗が無言**になる
  （ラベルは削除を約束しているのに消えず理由も出ない / 打った名前ごと消える）。
  出し口は `sidebar::notify_ui_failure` の 1 実装で、リモート行（#919）と同じ
  `set_remote_notice` へ寄せる。**画面は増えても口は増やさない**: 画面の区別は
  `sidebar::NoticeArea`（persist.log の `area=` と A/B の逃げ道の選択に使う）だけで持ち、
  呼ぶのは `notify_ui_dispatch_failed` 1 本にする（#1417 で右パネルの tmux window 切替と
  プレビューの目次 / ページ移動を同じ口へ寄せた）。番犬は `issue1399_tree_notice_watchdog`
  （UI モジュールの dispatch 呼び出しを走査し、捨てている箇所を file:line で名指す。
  `KNOWN_DISCARDED` は #1417 で**空**になった = 新しい画面で捨てたら必ず落ちる）
- **クリックの中身は `render` のクロージャから切り出す**（#1417）。合成マウスイベントは
  GPUI へ届かないことがあるので、`on_click` の中に直接書くと**押した経路をセルフテストから
  叩けない**（前後比較が取れない = #1399 で「その間の 1 行」が未検証として残った形）。
  名前を付けて `on_click` はそれを呼ぶだけにし、セルフテストは同じ名前を叩く
- **診断へリンク文字列そのものを出さない**。PDF / 画面の中身はペイン内容に相当するので、
  `persist.log` へ載せてよいのは理由の分類（`UrlBlocked`）だけ。dispatch 由来の失敗は
  `DispatchError::class()`（#1399）が分類を返すので、本文（パス・OS のエラー文）は
  通知欄にだけ出して診断には載せない
- 「シェルを経路に置かない」（#1371）はこれとは**別の話**。あちらは URL が途中で
  切れない話で、スキームの検査にはならない。両方が要る

## 他人の設定ファイルを読むときは「属さない位置」を持つ（Issue #1400）

`~/.ssh/config` のような**行の並びで塊が切り替わる**設定を読むとき、状態を
「いま開いている塊」の 1 値（`Option<T>`）で持つと、**知らないディレクティブが
塊を切り替えたときに気づけない**。#1400 はそれで、`Match` 行を `_ => {}` で
素通りさせていたため以降の `User` / `Port` が直前の `Host` へ付き、
`prod` の接続 argv が `ssh -p 2222 root@prod` へ化けていた（本物の `ssh prod` は
`Match host bastion` に一致しないのでその設定を使わない = **tako だけが別の宛先・
別ユーザーへ繋ぐ**）。パース結果は表示専用ではなく `dispatch::remote_ssh_argv` が
実際の argv にするので、誤読はそのまま接続先の誤りになる。

- **状態は 2 値以上で持つ**（`tako_core::ssh_config::Section` = `Unattached` / `Hosts`）。
  塊を切り替えるディレクティブ（`Match` / `Include`）は「どの `Host` にも属さない位置」へ
  倒し、以降の設定を**捨てる**。取りこぼす側へ倒すのは意図的で、
  **繋ぎ先が変わるより既定のまま繋ぐほうが安全**
- **キーワードは最初のトークンで切る**。行全体の最初の `=` で割ると、値に `=` を含む行
  （`Match exec "test -f a=b"`）でキーワードが読めず、その塊の切り替えが**まるごと抜ける**
- **`Include` は塊を閉じてから読む**。OpenSSH は取り込んだ側の `Host` 行が親ファイルの
  残りの解釈まで変えるので、親の塊を開いたまま続けると「取り込み先の Host 向けの設定」を
  親の Host へ付けてしまう（= 同じ穴の再発）
- **コメントと実装が食い違っていたら、まずどちらが正かを決める**。`Host web1 web2` は
  コメントが「各パターンを独立したエントリ」と言う一方で実装は先頭 1 つで `break` しており、
  `web2` が一覧から消えていた（コメント側を正として実装を合わせた）
- **読めなかった取り込みは黙って飛ばさない**。ただし**最上位のファイルが無いのは正常**
  （ssh config を持たないユーザー）なので警告しない。診断は
  [`SshConfigParse::warnings`] へ**返す**形にしてテストがログ無しで固定できるようにし、
  パスは `paths::shorten_home` を通す（実ホームを生で出さない = #927）
- **既知の限界**（doc コメントにも書いてある）: `Include` の `[...]` 文字クラスは展開しない /
  `~user` は解決しない / glob の照合は大文字小文字を区別する
- 番犬は `crates/tako-control/tests/issue1400_ssh_config_watchdog.rs`（6 つのアームが揃って
  いること・`Match` が塊を閉じること・`Host` のパターン走査に `break` が無いこと・
  `Section` が 2 値のままであること・**dispatch 側の argv の組み立てが tako-core の結合
  テストの写しと合っていること**）。挙動は `ssh_config::issue1400_tests::*`

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

## 外部コマンドを待つときは上限を持つ（Issue #1503）

**`Command::output()` / `Child::wait_with_output()` を「相手は必ず返る」前提で書かないこと。**
待ち時間の上限が無いので、相手が固まると呼んだ側も固まる。しかも probe（読み取りの
問い合わせ）は普段 0.1 秒で返るので、**壊れるまで誰も気づかない**。

- #1503 の実物: `tako setup` のエージェント CLI probe が全部 `.output()` だった。
  **`claude mcp list` は登録済みの MCP サーバへ 1 台ずつ繋いで健全性を見る**ので、
  サーバが 1 つ無応答だと返らなくなる。#1500 の棚卸し R4 では `tako setup` が
  **無言で 6 分固まり続け**、dispatch（GUI / MCP 経路）はボタンが永久に回った
- **待ちの 1 実装を通す**（`tako_core::probe::wait_with_timeout` /
  `output_with_timeout`）。呼び手が `.output()` を 1 行書き戻すだけで上限は消えるので、
  番犬 `issue1503_probe_timeout_watchdog` が素の `.output()` を file:line で落とし、
  **寄せ先が実際に呼んでいること**も同時に見る（#1496 の 2 本立て）
- **上限は絶対値で置き、根拠を実測で書く**。#1503 の既定 15 秒は
  `claude mcp list` の実測（MCP 13 サーバ登録の実機で 4.57 / 4.71 / 5.26 秒）の約 3 倍。
  「なんとなく」の値は伸ばす理由も縮める理由も後から作れない
- **上限を外す指定を作らない**。env（`TAKO_SETUP_PROBE_TIMEOUT_SECS`）は値を変えるだけで、
  **0 / 不正 / 空は既定へ落とす**。「0 = 無制限」を用意すると設定 1 つで症状が戻る
- **打ち切ったら必ず知らせる**。症状は「固まる」より先に「何も出ない」だった。
  文面（`[確認できません] {何を}（{N} 秒応答なし）`）と読み取り（`parse_notices`）を
  **同じ場所**に置くと、CLI が出した行を dispatch が拾って応答 JSON へ載せられる
  （開発不変条件: UI で分かることは MCP / CLI からも分かる）
- **読み切りにも同じ予算を掛ける**。`Command::output()` は子が終わっても、
  子が残した孫がパイプの書き手として居ると `read_to_end` から返らない。
  吸い出しは別スレッドへ出して **join しない**（join した時点で上限が無意味になる）。
  「少しだけ出してから固まる子」はパイプが詰まるので、`try_wait` のポーリングだけでも足りない
- **打ち切るのは読み取りだけの問い合わせに限る**。書き込み
  （`claude mcp add` は `~/.claude.json` を書く）を途中で kill すると利用者の設定を
  壊しうるので、#1503 では probe だけを打ち切る（残りは `.agent/plans` ではなく Issue で追う）
- **検証は状態で書く**（`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）。
  見るのは `Outcome` がどれか・応答に `timed_out` が載るか・**打ち切った子が残っていないか**で、
  所要そのものは assert にしない（実経路テストは外側の締め切りで殺し、所要は証拠として出す）

## プロセスの生死は境界の 1 実装で決める（Issue #1557 / #1581）

**「その pid は生きているか」を自前で書かない。`tako_core::platform::process::pid_alive`
だけが答える**（unix は `kill(pid, 0)`、Windows は `procinfo::snapshot` の在籍）。

自前で書くと `#[cfg(not(unix))]` の腕が要り、そこは決まって**無条件 `false` か
無条件 `true`** になる。どちらも Windows でだけ全件を静かに誤り、macOS の単体テストは
緑のまま通る（`remote.rs` の `is_process_alive` は非 unix で常に `false` を返し、
`is_process_aliveは存在しないpidをfalseで返す` は**何を渡しても通る偽の緑**だった）。

- 境界は unix でも 2 点を引き受ける。**自前実装はどちらも落とす**:
  `EPERM`（別ユーザーのプロセス）は「居る」・`pid_t` の範囲外は「居ない」
  （`pid as libc::pid_t` は `u32::MAX` を -1 =「全プロセス」へ潰すので、
  `kill` が成功して存在しない pid が生きて見える）
- **`ports::process_alive` は生死の答えではない**。あれは tmux ソケットの回収専用で、
  非 unix では「何も回収しない側へ倒す」ために常に `true` を返す（#1253）。
  掃除や停止の判定に流用すると Windows で 1 件も動かない
- 走査で何千回も引くなら、境界と**同じ材料**（`procinfo::snapshot_checked`）を 1 度取って
  キャッシュしてよい（`test_residue::OwnerProbe::alive`）。ただし判定を置く場所は
  関数 1 つに保ち、呼び出し側へ `#[cfg]` を撒かない
- ゾンビは `pid_alive` では「居る」。**停止の待ち合わせには使わない**
  （`tako_control::platform::process::has_terminated` がゾンビ込みで担う）
- 番犬: `crates/tako-control/tests/issue1557_pid_alive_boundary_watchdog.rs`
  （`libc::kill(…, 0)` の直書きと `ports::process_alive` の流用を `file:line` で名指す。
  停止の `libc::kill(…, SIGTERM)` には当たらない）

### 「生きている」は「その相手だ」ではない（Issue #1616）

pid が生きていることを確かめても、**相手が誰かはまだ分かっていない**。pid は再利用されるので、
不可逆な操作（kill / state の掃除）へ進むには**正体の照合**が別に要る。

- 材料を引くのは境界の 1 実装（`tako_core::platform::procinfo::observe_identity` =
  コマンドライン / 実行ファイル / 起動時刻）で、突き合わせは純関数（`judge_identity`）。
  **呼び出し側に「どの材料がこの OS で引けるか」を知らせない**
  （コマンドラインは unix では引けず、起動時刻は macOS / Windows のみ）
- 結論は 3 値（`Confirmed` / `Mismatch` / `Unknown`）で、**`Unknown` を `Confirmed` 側へ
  倒さない**。材料が 1 つも引けないのは「本物だった」ではないので、進んでよいかは
  `IdentityVerdict::confirmed()`（`Confirmed` のときだけ true）で読む
- 実行ファイルは**名前だけ**で比べる（`stem_of`）。symlink 越しの起動・`versions/<版>` の
  実体・8.3 短縮名で、同じプロセスが別のパスに見える。起動時刻の照合幅は
  `START_TIME_SLACK_SECS`（記録の書き出しと OS の生成時刻はずれる）
- **OS ごとの腕は `cfg!` で分ける**（`#[cfg]` ではない）。`#[cfg]` だと片方しか
  コンパイルされず、macOS で開発しているあいだ Windows 側の綴りが黙って腐る。
  腕の中身（`remote.rs` の `boundary_identity_confirmed`）を OS 分岐の無い関数に
  しておけば、**Windows で使われる判定を macOS の単体テストで実測できる**
- #1616 の実物: `verify_pid_identity` は照合がまるごと `#[cfg(unix)]` の中にあり、
  Windows は末尾の `true` へ直行 = **「生きている pid はすべて tako の daemon」**だった。
  事故になっていなかったのは `is_process_alive` が非 unix で無条件 `false`（#1557）
  だったから = **穴がもう 1 つの穴で塞がれていた**形で、境界へ寄せた #1596 で露出した。
  停止の Windows 実装（#1599）が先に入れば、pid を再利用した無関係なプロセスを撃つ
- 番犬: `crates/tako-control/tests/issue1616_windows_pid_identity_watchdog.rs`
  （素通り・`#[cfg]` の腕・自前判定へ戻した形・`Unknown` の反転を `file:line` で名指す）

### 材料が採れなかった回は「不在」ではない（Issue #1597）

**生死を列挙で読む判定は、「列挙に失敗した」と「その列挙に載っていない」を別の値で持つ。**
Windows の在籍列挙（Toolhelp）は失敗すると**空の `Vec`** を返すので、そのまま読むと
1 回の失敗で**全 pid が不在**に見える。`test_residue` はそれを `Owner::Dead` と読み、
`sweep_in` が**並行して走る別 worker の test dir まで消しに行く**（#625 の事故クラス）。

- 境界が 3 値を返す: `procinfo::snapshot_checked()` が `Some(procs)`（列挙できた）と
  `None`（失敗した / **そもそも手段が無い OS**）。手段の有無は `snapshot_supported()` が
  答えるので、呼び出し側は「失敗」と「この OS では列挙しない」を取り違えない
  （後者は pid ごとに `pid_alive` へ聞けばよく、見送る必要は無い）
- **空の在籍表は失敗として畳む**。呼び出したプロセス自身が必ず載るので「成功したが 0 件」は
  在り得ず、0 件をそのまま読むと失敗と同じ結果になる（畳むのは `snapshot_checked` と
  `test_residue::Roster::from_snapshot` の 2 段）
- 倒す先は**「消せない・撃てない」側**で固定する。`pid_alive` は**「居る」**と答え
  （この答えは掃除と停止の手前に置かれる）、`OwnerProbe` は `Owner::Unknown` = 見送る
- lossy な `procinfo::snapshot()` は残してよいが、**「居ないこと」を根拠に消す / 撃つ判断には
  使わない**（「居るものを列挙して絞る」用途はそのままでよい）
- 読み方は **cfg で割れない純粋関数**に置く（`platform::process::alive_in_snapshot` /
  `test_residue::Roster`）。Windows の腕を macOS から単体で回せないと、事故の再現も
  修正の実測もできない
- 番犬 `crates/tako-control/tests/issue1597_snapshot_failure_watchdog.rs`（構造 5 本 +
  規則そのものを呼ぶ 1 本）。実プロセスの A/B は `TAKO_1597_ROSTER`
  （`fail` = 列挙の失敗 / `empty` = #1597 以前の読み方。**テストプロセスでだけ効く**）で、
  `crates/tako-core/tests/test_data_residue.rs` が生きている子の置き場を使って測る

## 代行できない 1 件で、代行できる 10 件を捨てない（Issue #1501）

**段を並べた処理で「人しかできない 1 件」に当たったら、そこで `Err` を返して
終わらせないこと。** 残りの段が全部飛ぶ。詰まりは**人へ残る作業**として型で積み、
やれる段を全部やってから最後にまとめて出して **exit 0** で終える。

- #1501 の実物: `tako setup` は bootstrap [3/3] の認証段（ブラウザ操作なので
  tako は代行しない = #1129）で `Err` を返していた。**認証は一番手前の段**なので、
  新品の Mac では依存導入・MCP 登録・指示ファイル・`profiles/default.yaml`・
  テンプレ展開が**1 つも走らず**、「setup を走らせても何も整わない」が既定の結末だった
  （#1500 の棚卸し R1 / R2 / R3 で 3 通りすべて exit 1 を実測）
- **型で積む**。段の戻り値を `Result<(), _>` から
  「やれたこと + 残ったこと」（`Vec<Remaining>` / `BootstrapOutcome`）へ変える。
  `Result` を返す形に戻ると、呼び出し側の `?` **1 文字**で元の壊れ方へ戻る
- **残り 1 件には「人が次に打つ 1 行」を必ず添える**（#322 の最簡形）。
  添えられない種別を作らない（`RemainingKind::command()` が `None` を返す種別は、
  代わりに理由の行を出す = 無言で消さない）
- **同じ道は 1 本にまとめる**。同じ詰まりは複数の段から積まれる（認証は bootstrap 段と
  エージェント選択の 2 か所）。畳み込み・並び順・文面は純粋関数 1 本
  （`setup_remaining::summarize` / `render`）に閉じ、CLI は積んで表示するだけ
- **「完了しました」と言い切るのは残り 0 件のときだけ**。残りがあるのに完了と言うと、
  利用者は整っていないことに気づけない
- 止める側（`Err`）に残してよいのは**設定の破損と書き出しの失敗**だけ。番犬
  `crates/tako-control/tests/issue1501_setup_continue_watchdog.rs` が
  「`run_setup` が自分で `Err` を作って早期 return しない」「段が `Result` を返さない」を
  `file:line` で落とす。A/B（`TAKO_1501_LEGACY=1`）は 1 か所の env ゲートに閉じる

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
  ループ）は dispatch のあとに `cx.notify()` してから**レイアウトを変える Request なら
  その場で 1 フレーム描く**（#1370）ので、**直接 `dispatch` を呼ぶ検証側も同じ順序にする**。
  守らないと「操作が効いていない」ように見え、2 秒ポーリングの notify がたまたま挟まった
  回だけ通る（#232 の PDF アウトラインジャンプが #786 以降フレークになっていた実例）
- **例外は「製品側が描くこと」そのものを検査する項目だけ**（#1370 の項目 22b）。
  この項目は実 CLI を打って IPC ループを通し、`notify_and_draw` を**呼ばずに**
  `session.size()` が新しい取り分へ届くのを状態待ちで見る。検証側が描くと
  `TAKO_1370_LEGACY=1` でも通ってしまい検出力が丸ごと消えるので、
  「待つ前に汚して描く」という上の作法をここへ当ててはいけない。
  不変条件は番犬 `issue1370_ipc_redraw_watchdog` の
  `セルフテスト項目22bは描かずに待つ` が守る（`notify_and_draw` /
  `wait_for_drawn_state` の混入を file:line で名指しする）
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
- **打ち込んだ CLI の結果を固定の予算で待たない**（#1353 / #1364）。ペインへ打った
  `tako …` が判定できる形になるまでには**シェル起動 → CLI プロセス起動 → IPC →
  dispatch → アプリの状態更新**の 4 段が乗る。`wait(cx, 800).await;` の直後に
  `window.update` で状態を読む形も、`for _ in 0..60 { wait(cx, 100).await; … }` の
  固定窓も、混み具合に追従しないので使い切れば**その項目以降が 1 つも走らない**
  （項目 44 = 固定 1 秒が load 103 で・項目 22 = 固定 800ms / 項目 63 = 固定 6 秒窓が
  load 96 で。#1353 は origin/main のバイナリでも落ちた）。
  `wait_for_app_state` + `cli_state_budget`（旧の固定予算を A/B のために残したまま、
  新経路は `state_wait_budget` で伸ばす）へ寄せる。番犬テスト
  `打ち込んだcliの結果を固定予算で待っていない` が違反行を名指しで落とす。
  **既存の番犬（#796）は「固定待ちの直後 3 行が `check(`」×「肯定形の
  `focused_contains`」の 2 条件しか見ない**ので、待ちのあとに `let … =` が挟まる形と
  判定材料が状態読み（`display_offset()` 等）の形は**二重に対象外**だった。
  - **アンカーは「打ち込んだ CLI コマンド」**（手前に `type_text` + `{cli}`）。
    待つ相手がアプリの中ではなく**別プロセスの往復**だとソースから言えるのがこの形で、
    `window.update` の中で操作まで済ませている形（待つ相手が居ない）を巻き込まない境界
  - **A/B の腕は「旧の形」ではなく「旧の予算の値」で表す**（`cli_state_budget` の
    `legacy_window`）。旧の形をコードに残さないので、番犬に legacy の除外が要らない
  - **混み具合は人工負荷では再現できない**。`yes` を 68 本立てて load 134 まで上げても
    短命な CLI の起動は遅れず、旧の腕（固定 800ms）が `waited=0.1s` で通った（実測）。
    Issue が観測した**遅れる材料そのもの**を注入する（`inject_cli_wait` の
    `late` = 旧の予算 + 5 秒 / `never` = ずっと観測しない = **新経路でも落ちるのが正しい**）。
    不等式（旧の予算 < 注入の遅れ < 新の素の上限）は単体テストが全呼び出しに対して検査する
    （呼び出しは**ソースから採る** = `cli_wait_budgets`。手で並べた表は移送のたびにズレる）
  - **一度に全部を移さない**。残りは `KNOWN_FIXED_CLI_WAITS`（`check` のラベルがキー）で
    段階導入する。リストは**減る方向にしか動かせない**（直したのに残っていると
    staleness 検査が落ちる）。**#1375 で残り 16 件を移して空になった**ので、
    新しく混入した形はリストへ足さずに移送する
  - **移送先は 1 実装**（`wait_for_cli_state`。#1375）。`item`（A/B のキー）/
    `legacy_window`（旧の固定予算。固定窓なら **N × M の総和**）/ `base`（新の素の上限）を
    渡すと、A/B の口・注入・診断行（`TAKO_SELF_TEST_1375`）までまとめて付く。
    `TAKO_1375_LEGACY=18,73f` のように**項目ごと**に旧の腕へ戻せる
    （`check` は 1 つ目の失敗でプロセスごと止まるので、全部戻すといちばん早い項目しか
    観測できない = #1173 と同じ理由）
  - **「分割して新ペインを操作する」形は前提を 2 段に割る**（#1375 の項目 47 / 47b /
    73c / 73f）。①分割の着地（`split_focus_new_pane` = 打つ前のペインと違うペインへ
    フォーカスが移る・**作られた ID を返す**）②その ID が素のアイドルになる、の順で
    どちらも状態で待つ。以降の検査はフォーカスではなく**返った ID** を見る。
    項目 73f の旧実装は 60 × 250ms の固定 15 秒窓で ①② を同時に待っていて、
    高負荷（load-after 101.64）の隔離セルフテストで実際に落ちた
    （`TAKO_APP_SELF_TEST_FAILED: 73f: split で新ペインへフォーカスが移らない`
    = **以降の項目が 1 つも走らない**）。さらに旧実装は**分割コマンドを打ったあとに**
    「分割前のフォーカス」を読んでいたので、分割が先に着地した回は比較相手が新ペイン
    そのものになり、窓を使い切るまで真にならなかった（待ちを伸ばしても直らない形）
- **描画・レイアウト由来の状態は「毎周期 1 フレーム描いて」待つ**（#1364）。
  `pane_text_areas`（= 1 度でも描かれたペイン）のような材料は、**dirty でない
  フレームでは更新されない**（#786 で本体とクロームは `AnyView::cached`）。
  `tako split` のような IPC / dispatch 由来のレイアウト変更は、製品経路では受信ループが
  `cx.notify()` してから次フレームを描くが、**セルフテストの待ちの中では誰も汚さない**
  ので待っても永久に届かない。`wait_for_drawn_state`（毎周期 notify + draw・観測だけを
  予算で待つ）を使う。
  実測（#1364 の項目 63）: 新ペインは `born=true` のあと 24.4 秒待っても
  `pane_text_areas` に載らず、**描かれないペインのシェルは 1 行も出さない**ので
  マーカーも出なかった。旧実装はここを**分割元のペイン**で見ていたため
  `painted=true` の偽陽性になり、原因が「マーカーが出ない」に見えていた
  （真因は待ち不足ではなく**証拠源の取り違え + 再描画の不発**。製品側の再描画不発は #1370。
  **ただし項目 22 の `tree().layout()` の取り分は dispatch で同期更新されるので #1370 とは別**
  = 別途の調査で確定。描画内でしか更新されないのは cols / rows 側）。
  **証拠源は作った ID のペインから採る**（フォーカスで追うと、新ペインのコマンドが
  終わった瞬間に分割元へ戻って証拠源が入れ替わる = #1175 と同じ「流れない場所から採る」）
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
- **「同じ絵のはず」を実ピクセルで言うときは、向きの判定を検査対象と別の材料で作らない**
  （#812）。Metal の読み戻しは上下の向きがプラットフォームで変わりうるので既存の器は
  「多い方を採る」で逃げているが、**地の色で決めようとすると dark テーマの
  `surface_2`(32,33,47) と `background`(30,30,46) が 3 しか離れておらずどちらの向きでも
  当たる**（実測で踏んだ）。`pane-border` 節は**枠線が実際に見つかる側**を採り、
  どちらでも見つからなければ「向きが分からない」ではなく**枠線が 1 本も描かれていない**と
  名指して落とす（= 枠線を消す注入がそのまま検出される）。数えるのは**上下の辺だけ**にする
  （左右は縦線なので反転しても大きく重なる。1 ペインで上向き 100% に対し下向きも 41% 当たった）
- **A/B の差分は「消したかった差」と「消えては困る差」を分けて数える**（#812）。
  枠線の二重塗りは**上 2 つの丸め角にしか出ない**（旧挙動でヘッダ外枠が塗り重ねるのが
  そこだけだから）ので、**下 2 つの角に差が出たら新しい側が 2 回塗っている**。
  「角なら差が出てよい」と一括りにすると、**二重塗りへ戻す注入が素通りする**
  （実測で踏んだ。角を上下に分けて初めて落ちるようになった）
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
- **注入した fixture は `pin_chat_fixture` で守る**（#853 の機構・#1173 で踏んだ・
  **#1397 以降は器の有無を問わず必須**）。会話の定期読み取り（`collect_chat_targets`）は
  「実 claude が動いていないペイン」を正しく «チャットではない» と判定して
  `chat_panes` から落とすので、GUI モードで 2 秒 tick を跨ぐ検査は注入した会話を
  失う。**#1397 までは persist OFF だけが偶然守られていた**（列挙が
  `backend_sessions` 起点で、器の無いペインは 1 件も対象にならなかった）。
  #1397 で列挙を `terminals` 起点へ広げた = **器なしでも読みに来る**ので、
  「persist OFF なら pin 無しで通る」は**もう成り立たない**。
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

- **待ちが尽きたら「素通り」しない**（#1308）。固定窓で待って**尽きても結果を検査せず
  次の手順へ進む**形は、原因を**別の場所へ移して**しまう。
  `dispatch::tests::respondは保持しているペインへin_process経路で届く` は
  ①「素のシェルのプロンプト」を固定 10 秒で待ち ②尽きても検査せず打ち込み
  ③「ダイアログの描画」を固定 20 秒で待ち ④尽きても検査せず `dispatch` していた。
  混んだ機（全件走 18 スレッド + 外の `cargo build`・load 60〜76 = 1 CPU あたり 3.9）では
  素のシェルの起動が 10 秒を超え、起動前の PTY へ打ち込んだ行は**エコーされるだけで
  実行されない**。結果、最後の `dispatch` が**「器越しへ倒れている（#1200）」という
  無関係な原因**を名指しして落ちていた（実測 2026-09-11:
  `prompt_ok=false prompt_waited=10.03s dialog_seen=false dialog_waited=20.04s load=3.89`・
  画面は打った行のエコーだけ。全件走 19 回に 1 回 / Issue の棚卸しでは 3/30）。
  - 待ちは**上限に達したらそこで落とす**ドライバ（`i1308_wait_for_state`）へ寄せる。
    呼び出し側が結果を検査し忘れる余地を**構造的に無くす**のが肝で、「戻り値を見る」
    約束にすると次の 1 本でまた漏れる
  - **前提（相手が起きている）と本題（fixture が描かれた）は別々に待つ**。1 つの窓に
    まとめると「準備ができていない」と「本題が壊れた」が同じ失敗に見える
  - **届かないあいだは上限つきで送り直す**（#1165 と同じ形）。「プロンプトが出た」は
    zle が上がった証拠としては弱く、起動途中の PTY は打鍵を落とす（#640）。
    送り直しの間隔は**予算に比例**させる = 混み具合が変わっても送る回数は変わらない
  - 資源枯渇との切り分けは #1265 と同じく**先に機の資源を採る**（実測: `/dev/ttys*`
    106 本 / `kern.tty.ptmx_max` 511 = 枯渇していない。`TerminalSession::spawn` は
    成功していてエコーも返っていた = **シェルの起動が遅いだけ**）
  - 注入 `TAKO_1308_INJECT=late` はシェルの起動を 15 秒遅らせ、その間の打鍵を
    `cat </dev/tty` に食わせる。**`</dev/tty` を省くと再現しない**（非対話シェルの `&` は
    バックグラウンドジョブの stdin を `/dev/null` へ向けるので、打鍵が tty のキューに
    残り、遅れて起きたシェルが後から実行して旧アームまで通ってしまう = 実測）。
    素の上限（30 秒）は旧の固定窓（10 秒 / 20 秒）より広く採る（#771 と同じ理由）
  - 番犬 `issue1308_pty_wait_watchdog` が、実 PTY を張るテスト
    （`TerminalSession::spawn(` を持つ関数）の自前の固定窓を `file:line` で落とす

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

### tako 自身が出すマーカーも幅で割れる（Issue #651）

上は「相手の TUI が自分で切る」話だが、**tako が出したマーカーも端末が割る**。
実行ペインの終了コード `__TAKO_EXIT=<code>` は最短 13 文字あるので、幅がそれより狭いペインでは
物理行が割れ、行 1 本の中を探す判定は必ず外れる（実測: 幅 10 桁で `__TAKO_EXI` / `T=0`
に割れ、`tako run-interactive --wait` が 40 秒返らない = ポーリングに上限が無い）。

- **折り返しの連結は「右端まで埋まった行だけ次と繋ぐ」**（#1283 / #1182 と同じ物差し）。
  埋まっているかは**列**で測る（`TerminalSession::visible_lines_filled` = alacritty の
  `line_length()`。`WRAPLINE` が立っていれば全幅・立っていなければ占有列数なので、
  折り返しが器の中で起きる tmux / psmux のペインでも同じ判定で見られる）。
  **文字数で測ってはいけない**（全角が 1 つあるだけで「埋まっていない」と読む）
- **またげるのは「埋まった行の行末 → 次の非空行の行頭」だけ**。空行を飛ばさない
  （折り返しの続きが空行になることは定義上ない = 飛ばすと無関係な行を繋ぐ）
- **数字のあとは行の残りが空白であることを要求する**。ちょうど埋まったマーカー行の次に
  別の出力が来る形（`__TAKO_EXIT=0` / `5 files changed`）は、繋いだ側が数として
  成立しないので**繋がない解釈**へ落ちる
- 読む側は `dispatch::find_exit_marker` の 1 実装だけにする（#875 で組み立て側と
  読む側を 1 個の定数へ寄せたのと同じ理由）。番犬
  `crates/tako-control/tests/issue651_exit_marker_wrap_watchdog.rs` が
  製品コードへ生えた終了マーカーの literal と、文字数で測る物差しを file:line で落とす

## 宛先が解けないものは既定へ落とさない（Issue #1466）

「呼び出し元が誰か」から宛先を決める機構（ユーザータスクの返答配送・引き継ぎの後任・
通知の戻り先）で、解けなかったときに**既定のプロファイルへ落とす**と、
**たまたまその既定で動いている無関係な相手**へ黙って届く。届いた側には文脈が無く、
待っている側には何も来ず、どちらにも「別の宛先へ行った」と分かる手がかりが無い
（実発 = #1466: tako の worker が起票したユーザータスクの返答が、別プロジェクトの
master の入力欄へ入った。この機械には profile=default の master ペインが 7 枚あった）。

- **解決順は「確かな順」に並べ、当たらなければ `None` を返す**。#1466 の順は
  ①呼び出し元が名乗った master ②起票したペインの role ラベル ③**spawn 元**
  （`spawned_by` の枝）④その project を管轄するプロファイル。
  ③ が本命で、**spawn の瞬間に tako 自身が張った枝**なので「誰の下で動いているか」の一次情報
- **曖昧な候補からは選ばない**。④ は管轄が 1 本のときだけ引く（複数なら `None`）。
  「候補のどれか 1 つ」を選ぶのは既定へ落とすのと同じ事故になる
- **解けなかったことは失敗として残す**（`failed` + 宛先不明の理由 + persist.log）。
  「届かなかった」は読めば分かるが、「別人へ届いた」は誰にも分からない
- 判断は**純粋関数 1 本**（`user_tasks::resolve_origin_profile`）に閉じ、
  呼び出し側は材料（role・spawn 元・管轄）を集めるだけにする。
  条件分岐を dispatch の中で書き直すと、順序と「解けない」の扱いが 2 か所へ割れる
- **戻り先と名乗りを同じ値から作らない**。#1466 で worker の戻り先が master の
  プロファイルへ解けるようになったので、`created_by` をそこから作ると
  worker の起票が `master:<profile>` を騙る。名乗りは**呼び出し元自身の役割**から作る
- **`spawned_by` は `layout.json` に載らない**（GUI 再起動で失われる）。
  枝に頼る解決には「枝が無いときの保険」を必ず 1 本用意する

## 「私は誰か」と「そのペインは何か」を同じ欄へ混ぜない（Issue #1516）

呼び出し元を自動解決する要求（`orchestrator self` / `handoff` / `adopt` / `guide`）は、
**呼び出し元の手掛かり**（env の `TAKO_PANE_ID` / `TAKO_ORCHESTRATOR_ROLE` / 自プロセスの pid）
を載せて受け手に解かせる。受け手の解決順は「確かな順」= **pid 祖先辿りが最優先**で、
`pane` 欄は「stale になりうる env 由来の手掛かり」として扱う（#288 / #210）。

そこへ利用者の**明示指定**（`--pane N` / MCP `pane`）を同じ `pane` 欄へ混ぜると、
明示指定は**毎回黙って負ける**。#1516 の実測は `self --pane 1964` が `pane_id: 1954`
（呼び出し元）で、応答は**正しい形をしている**（別 master の状態を見たつもりで自分の
状態を読む）。`env -u` で名乗りを消しても `profile_source` が変わるだけで pane は動かない。

- **明示指定は「私は誰か」を聞いていない**。名指しのときは呼び出し元の手掛かりを
  **1 つも載せない**（`caller_role` / `caller_pid` を落とす）。組み立てを
  `Request::orchestrator_self` の **1 本**に閉じるので、CLI と MCP で順序が割れない
- 受け手は「**手掛かりが 1 つも無いのに `pane` が載っている = 名指し**」として扱い、
  解けなければ**既定へ落とさず失敗**する（`named_pane` → `PaneNotFound`）。
  role 検索へ落ちると「たまたま既定 role で動いている無関係な master」を答える（#1466）
- **自分を名指しした場合は「私についての問い」**に倒す。`tako solo` の profile は
  env の `solo:<名前>` にしか無く、ペインのラベル（`orchestrator-solo:<名前>`）からは
  解けないので、手掛かりを落とすと自分に聞いた solo の profile が既定へ落ちる
- **応答は名指し先のものにする**。`profile` はペインの role ラベルから
  （`profile_source=pane_role`）、`role` はそのペインのラベルを返す。
  master / solo でないペインを名指しされたら `pane_id` はそのまま返しつつ、
  既定へ落ちた理由を `warnings` に出す（黙って default の引き継ぎ先を答えない）
- 番犬 `issue1516_named_pane_watchdog` が ①入口（CLI / MCP）が正本を通ること
  ②正本が名指しで手掛かりを落とすこと ③受け手が role 検索より前に名指しを見ること
  を `file:line` で落とす。A/B は `TAKO_1516_LEGACY=1`（混ぜる旧挙動）
- **まだ寄せていない同型**: `adopt` / `guide` / `handoff` の `--pane` は今も混ざったまま
  （`resolve_caller_pane` は名指しを扱えるが、入口が混ぜて渡している）。寄せるときは
  入口を `Request::orchestrator_self` と同じ形へ替えるだけで、受け手側の変更は要らない

## CLI と MCP は「同じ 1 本」を通す（Issue #1453 / #1544）

AGENTS.md の不変条件「tako-core 操作 API + `dispatch` + CLI + MCP の 1:1」は、
**読み取りにも効く**。CLI 側に「設定ファイルを直接読むだけ」の写しを置くと、
dispatch へ機能が足された日に**片側だけ取り残される**。

#1453 の実測がその形で、`projects add` の専用プロファイル自動生成が
**MCP からだけ効いて CLI からは効かなかった**。#1544 はその修正で残っていた
`projects list` の直読み（出力は一致していたので症状が出ていなかった）を寄せた。

- **「今は出力が同じ」は写しを残す理由にならない**。同じ関数に「直したもの」と
  「残したもの」が並ぶ状態そのものが再発の温床（次に触る人が片側だけ直す）
- 読み取りの CLI は dispatch の戻り値（`Value`）から表示を組む。表示の体裁
  （`{:<16}` の桁・区切り・空のときの案内）は CLI 側に残してよい —— 寄せるのは
  **どこから読むか**だけ
- 番犬 `issue1544_projects_dispatch_watchdog` が `orchestrator_projects_cli` の本文に
  `ProjectsConfig` が現れたら `file:line` で落とす。3 分岐すべてが
  `dispatch_orchestrator_projects` を通ること・CLI が渡す action の綴りが正本に
  在ることも同時に見る（綴りがずれると実行時にだけ「action が不正」で落ちる）

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

## ソース走査の番犬は本番コードを「切らない」（Issue #1420 / #1403）

テストコードを検査対象から外すために `src.find("\n#[cfg(test)]")` で切ると、
残るのは**ファイル中で最初に現れる `#[cfg(test)]` まで**になる。テスト用ヘルパの
`#[cfg(test)]` が 1 つ途中にあるだけで、それ以降の本番コードが番犬の視界から丸ごと消え、
**番犬は緑のまま**になる（検出力を失ったことに気づけないのが最悪の壊れ方）。

- 範囲取りは `crates/tako-control/tests/common/production_range.rs` の **1 実装**を通す。
  切らずに**テスト領域だけを空白へ潰す**ので、途中のヘルパで走査範囲が消えない。
  バイト長と行番号が保たれるので `file:line` の名指しがそのまま使える
- 使い分け: 下限つきで確認するなら `production(src, rel)`（既定 30%）、
  テストの厚いファイルは `production_with_floor(src, rel, 下限)`、
  小さいファイルまで丸ごと舐める走査は `scan(src).text`（下限を使わない）。
  テスト側を見張るなら裏返しの `tests_only(src)`
- 潰す位置は `code_view`（コメント・文字列を空白へ）で決めるが**中身は原文のまま**返す。
  「この文言を組んでいるか」を見る番犬（#1401 の `tako remote start`）が壊れないため
- 実測（#1403 の作業中）: `remote.rs` の検査対象より後ろへ 4 行のヘルパを置くと、
  走査範囲は 5,903 行 → 3,124 行（70.7% → 37.1%）へ縮んだのに **7 本とも緑**だった。
  4 クレートの `src` を丸ごと走査する `platform_parity` はさらに広く盲目で、
  `orchestrator/mod.rs` を **1.5%**・`mcp/mod.rs` を **10.5%** しか見ていなかった
- 番犬 `issue1420_production_range_watchdog` が、雑な切り方の再登場を `file:line` で落とす。
  A/B の旧アームだけは同じ行に `#1420-legacy-arm` を書いて対象から外す。
  寄せられない切り出し（製品コードの中に住む番犬）は `KNOWN_COARSE` へ**件数まで**載せる

### テスト領域の見つけ方は cfg 述語で決める（Issue #1445）

初版はリテラルの `#[cfg(test)]` しか探していなかったので、`#[cfg(all(test, unix))]` の
ように**属性が合成された**テストモジュール（`discovery.rs` / `ipc.rs` / `tmux_backend.rs` /
`shell_integration.rs` / `ports.rs` / `md_view.rs` / `preview.rs` / `main.rs` の 8 ファイル 11 か所）が
潰れず、**本番コードとして走査に混ざって**いた。番犬はテスト内の直書きを本番の違反として
誤検出するか、逆にテスト内の文字列を「扱った証拠」として拾って緑になる（#1417 が踏んだ型）。

- いま潰すのは `test` を**正の位置**に含む cfg 述語（`test` / `all(…, test, …)` /
  `any(test, …)` / 入れ子・順序違い・複数行も同じ）。判定は `mentions_test` の 1 実装
- **`#[cfg(not(test))]` は潰さない**。「テストでないとき」に載る**本番コード**で、
  潰すと本番が番犬の視界から消える（`orchestrator/mod.rs` などに実在する）
- **`#[cfg_attr(test, …)]` は対象外**。item 自体は本番ビルドにも載り、付ける属性だけを
  切り替えるものなので、潰すと本番コードが消える（入口の綴り `#[cfg(` から外れている）
- 述語の中の文字列は `code_view` が空白へ潰しているので、`feature = "test-util"` は
  原子として見えない（見えると本番コードを丸ごと落とす）
- **番犬ごとに正規化の写しを持たない**（#1441 が一時的に持っていた形）。
  番犬 `issue1445_cfg_predicate_watchdog` の `自前のcfg正規化が戻っていない` が
  `file:line` で落とす。A/B は `TAKO_1445_LEGACY=1`（共有部品の `cfg_mode`）
- 走査範囲は 263 本中 8 本で変わった（合計 74.34% → 74.04%）。
  **下限 30% を跨いだファイルは 0 件**で、番犬 `広げても下限を跨いだファイルは無い` が
  同じファイルの新旧を比べて常設で見張る（テストの厚いファイルでは落ちない）

### 肯定の存在確認はコメントを落とした眺めで見る（Issue #1609 / #1578 / #1536）

「この呼び出しが在る」= **肯定の存在確認**を `fs::read_to_string` の**全文**へ
`contains` すると、実体が消えても**同じ綴りを書いた doc コメントが残っていれば緑のまま**
になる。壊れ方が「落ちない」なので検出力を失ったことに気づけない。

- 実例 1: #1536 の番犬 `判定の写しを持たない` は、**自分の doc コメント**に書いた
  `tako_core::emoji::is_emoji` で `contains` が真になっていた（#1578 が発見）
- 実例 2: #1308 の番犬は「A/B の旧経路（`TAKO_1308_LEGACY`）が在る」をドライバ本体への
  全文一致で見ていたが、本体にあるのは A/B アームの**目印コメント**
  （`// TAKO_1308_LEGACY_ARM 開始`）だけで、実体の `env::var` は兄弟の関数にあった
  （#1609 で実測。入口の呼び出しとソースの env 読みへ分けた）

寄せ先は `crates/tako-control/tests/common/code_view.rs` の
**`without_comments_checked(src, rel)`**（下限つきの読み口）。

- **コメントだけ**を空白へ潰し、コードと文字列リテラルは**囲みごと**残す。
  識別子を見る番犬（`fn wait_osc7_cwd(`）も文言を見る番犬（`env::var("TAKO_…_LEGACY")`）も
  同じ 1 実装で書ける（`code_view` は文字列も潰すので後者に使えない）
- **バイト長と行番号が保たれる**ので、`file:line` の名指しと、コメントの目印で測った
  区間の切り出し（#757 の段の目印）がそのまま効く
- 走査が空振り（読み先の取り違え・空ファイル・眺めの破損）した形は
  `without_comments_checked` がその場で `rel` を名指して落とす。判定は
  **Rust の item の頭が 1 つも残らないこと**で、コメントの多さでは落とさない
  （実在の最小は `tako-app/src/platform/mod.rs` の 3.4%。278 ファイルを実測）
- **不在**を確かめる番犬（個人情報が無い / 絵文字が無い / 旧 API を呼んでいない）は
  これを通さない。**コメントの中の違反も違反**なので全文を見るのが正しい
- 既に読んだ本文の一部（関数本体の切り出し）に掛けるときは下限検査の無い
  `without_comments` を使い、空振り検査はファイルを読む側に置く
- 棚卸しと逆戻りの防止は `issue1609_comment_view_watchdog`。寄せた番犬の一覧
  （`MOVED`）が共有部品を通していることを、**その検査自身もコメントを落とした眺めで**見る

## `Instant` は巻き戻さない（Issue #1627）

**`Instant::now() - Duration` を書かない。** `Instant` の起点はブートなので
（Windows は QueryPerformanceCounter・macOS は `CLOCK_UPTIME_RAW`）、**稼働時間より
長く引くと panic する**（`overflow when subtracting duration from instant`）。
OS 依存ではない。

- **「まだ一度も起きていない」は `Option<Instant>` の `None`** で表す。判定は
  `opt.is_none_or(|t| t.elapsed() > TTL)`。時刻を捏造しないので閾値に依らない
- **「N 前に起きたことにする」は `tako_core::monotonic::rewound(d)`**（飽和する 1 実装。
  巻き戻せなければ「今」を返す）。自己検査でデバウンス窓を空ける用途はこちら。
  **初期値を期限切れにする用途には使わない**（飽和すると意味が反転する）
- #1627 の実物: `PaneMapping::new()` が「最初は必ず期限切れ」を
  `Instant::now() - Duration::from_secs(999)` で表していたので、**ブートから
  16 分 39 秒以内に `tako remote serve` が立つと panic**した（本番の呼び手は
  serve の起動と `backend_session_of_pane`）。再起動直後の自動復帰（#1485）は
  普通に起きる並び
- **「速い CI ほど落ちる」フレークとして現れる**: 同じ head の CI を 2 回回すと、
  テストに到達した時刻が boot から 746 秒 = panic / 1012 秒 = ok で反転した。
  負荷や環境の揺れとして片付けると原因に届かない（#1278 で Windows の
  `cargo test` を blocking にしたので、これは全 PR を無作為に赤くする）
- **走査は改行をまたぐ**。Issue の初版は行単位の grep で数えたので
  `Instant::now()` と `- Duration` が別の行に割れた 3 箇所を見落としていた
  （実際は 9 箇所）。番犬
  `crates/tako-control/tests/issue1627_instant_underflow_watchdog.rs` は
  本番コード（4 クレートの `src`・コメントと文字列は `code_view` で潰す）を
  改行ごと走査して `file:line` で名指す

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

### 番犬の関数名追跡は共有ヘルパを使う（Issue #1496）

違反行を**囲んでいる関数名**で名指しする番犬は、関数の頭の判定を
`tako_core::source_scan::fn_head_name`（トップレベル限定は `is_top_level_fn_head`）へ
通す。`line.trim_start().strip_prefix("fn ")` で書くと **`pub(crate) fn` / `async fn` が
関数の頭に見えない**ので、中の違反が**手前の関数名**で報告される。

- 壊れ方が「落ちない」ではなく**「落ちるが嘘をつく」**なので、テストは緑のまま残る。
  実例は `pub(crate) fn kill_shelved_tab_clicked` の中の違反が
  `fn shelved_tab_groups` の中として報告されたもの（#1491 の worker が観測）
- 置き場が tako-core なのは、同じ追跡をする番犬が **tako-app の
  `#[cfg(test)] mod`（`src/main.rs`）と tako-control の `tests/`** の両方にあるため。
  クレートを跨いだ `#[path]` は tako-app のビルドを別クレートのテスト配置に縛る
- 番犬 `issue1496_fn_head_watchdog` が ①直書きの再発（許可は理由つき 1 件）
  ②寄せ先が実際にヘルパを呼んでいること を止める。棚卸しの全文は同ファイルの冒頭
- `fn ` を**語として**探す位置ベースの走査（`test_timing_watchdog` の `enclosing_fn` /
  `ui_text/lang_watchdog` の `fn_defs`）は修飾子に依らないので対象外

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

## 画面テキストは 0 幅の結合文字ごと組む（Issue #1387）

alacritty は 0 幅の結合文字（NFD の濁点 `U+3099`・アクセント `U+0301`・異体字セレクタ）を
`Cell::c` ではなく **`Cell::extra.zerowidth`** に持つ。`c` だけを読むと結合文字は画面テキストを
組む経路で**黙って落ちる**。

- **`zerowidth` を読むのは `screen::cell_text` の 1 実装だけ**。テキストを組む側は
  `screen::push_cell_text`（`Cell` から直接組む経路）か `resolve_cell`（色解決と同時に組む
  `Screen` の経路）を通す。#1387 の修正前は落ちる実装が 3 つ在り（`resolve_cell` =
  描画 / GUI モード / links の材料・`compose_grid_row` = `tail_lines` / `visible_lines_filled`・
  `history_plain_lines` = ペインログ）、**選択コピーだけ**が alacritty 自身の
  `selection_to_string` を通っていたので、同じセルの内容が「見た目 / `read_pane` /
  ペインログ」と「Cmd+C」で食い違っていた
- **実害は「コピーしたパスは開けるが画面のリンクは開けない」**（実測: macOS の NFD
  ファイル名 `画像か` + `U+3099` + `ある.txt` が `画像かある.txt` として読み出され、`links` の
  実在チェックが false = cmd+クリックが無反応。#153 / #1283 で通した経路がここで死ぬ）
- **`text` へ結合文字を足したら `cell_cols` へ同じ列を積む**。`cell_cols` は「`text` の各 char が
  占めるグリッド列」なので、積まないと描画のセル写像と links のスパンが 1 文字ずつずれる
- **末尾の空白詰めを切る境界も同じ 1 実装で見る**（`screen::cell_is_trailing_blank`）。
  行頭に単独の結合文字が来ると本体が `' '` のセルへ載る（alacritty の `width == 0` 経路は
  1 つ左のセルへ寄せ、列 0 ではその場に載る）ので、素の空白扱いで切ると落ちる。
  #801 の空白セルの近道（`is_plain_blank`）も同じ理由で結合文字の有無を見る
- **hot path を太らせない**: 結合文字を持つセルは稀なので、`snapshot` のフラット配列は
  そのままにして**在るセルだけ**を `(添字, 借りたスライス)` で横に持つ（結合文字ゼロの画面では
  確保も探索もしない。実測 120x24 を 3000 回 snapshot = 修正前 68.6〜69.3ms /
  修正後 65.3〜67.9ms）
- 番犬は `crates/tako-control/tests/issue1387_combining_watchdog.rs`（1 実装の位置 /
  3 経路の配線 / `cell_cols` の同列積み / 近道の除外）。挙動は
  `terminal::tests::結合文字は画面テキストの全経路に残る` が `visible_lines` /
  `tail_lines` / `history_plain_lines` / `selection_text` の**4 経路の一致**で見るので、
  片方だけ直すと落ちる（= 3 実装の同時修正を構造的に要求する）
- `links::screen_from_lines`（`tako links --text` の経路）の列は「ASCII = 1 / それ以外 = 2」の
  近似なので結合文字も 2 列として数えるが、**どのパスがリンクになるかは変わらない**
  （実測: `detect_in_lines` で NFD のファイル名が引ける）ので近似のまま据えている

## 行の情報は「読み手が在るもの」だけ持つ（Issue #1388）

`ScreenLine` は**行に全角が在るかの旗を持たない**。`text` / `runs` / `cell_cols` の
3 つだけで、全角は「`cell_cols` が 2 列飛ぶ」ことで表す。

- 旗（`has_wide`）は #787 の専用 Element がグリッドを**常に**セル幅固定で組むようになった時点で
  読み手が消え、フィールドと doc と毎行の `windows(2)` 走査だけが残っていた。doc は
  「描画時にセル幅固定レイアウトへ切り替える判定に使う」と書かれたままで、**信じて直しても
  描画は何も変わらない**状態だった（実害はこの嘘の doc）
- 書き手は 4 か所（`screen.rs` / `links.rs` ×2 / `terminal_grid.rs` のテストヘルパ）に分かれ、
  `windows(2).any(|w| w[1] - w[0] > 1)` 版は**右端の全角を構造的に見落とす**（次の可視文字が
  無いので列差が観測されない）一方、テストヘルパの `chars().any(is_wide)` だけ答えが違った。
  **テストヘルパが production より正しい状態**は、使い直すときに嘘の緑を出す
- 行の性質が本当に要るようになったら、**読み手を書いてから**フィールドを足す
  （物差しは 1 実装・`cell_cols` と行幅 `cols` から求めて右端の全角も拾う）
- 番犬は `crates/tako-control/tests/issue1388_has_wide_watchdog.rs`
  （識別子の再登場 / `cell_cols` から旗を組む形の復活 / `ScreenLine` のフィールド指紋）。
  挙動は `screen::tests::全角の右隣スペーサーは近道で飛ばさない` /
  `scroll_mirror::tests::全角文字のセル列が正しい` が「2 列飛ぶ」ことで見る

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
- **同じ穴は `TerminalSession::visible_lines_filled`（#651 の物差し）にも在る**（#1389）。
  あちらは alacritty の `line_length()` で列を測るが、`line_length()` は末尾から
  `cell.c != ' '` を探すので**全角の後続セル（`WIDE_CHAR_SPACER`）を空きと数える** =
  行末が全角の行は 1 列足りず `filled=false`（実測・10 桁: `abcdefghあ` で
  `line_length=9`。端末自身が折り返した行だけ `WRAPLINE` で全幅になるので、
  **器が `CUP` で描き直した行は救えない**）。**2 つの物差しは同じ穴を持つ**ので、
  折り返しの結合を新しく書くときはどちらを使っても右端の全角を自分で検査する。
  現行の消費者（`dispatch::find_exit_marker`）は全 ASCII のマーカーの断片しか見ないので
  実害は無い。限界は `terminal::tests::visible_lines_filled_は行末の全角を取りこぼす` と
  番犬 `crates/tako-control/tests/issue1389_filled_wide_limit_watchdog.rs` が固定する
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

### 境界が 1 つも無い画面には「起点」を置く（Issue #633）

上の 3 つはどれも**当たったら捨てる**形なので、**罫線を引かず・箱ごと 0 桁で描く**
許可ダイアログでは捨てる材料が 1 つも無く、上に在る会話がすべて本文（`command` / `title`）へ入る。
実採取の 3 形のうち agy（`Requesting permission for:`）と claude の箱なし
（`Claude wants to run:` / `? Claude requested permissions to …`）がこれに当たる。

- **本体の開始マーカー（`dialog::BODY_START_MARKERS`）に一致する行が在れば、そこから集める**
  （`body_start_row`）。一致が無ければ従来どおり画面の上端から = **フォールバックが既定**
- 起点は**選択肢に最も近い一致**（会話文が同じ語に触れていても本物の見出しが勝つ）。
  **起点を下げるだけなので結果は必ず従来の本文の接尾辞**で、拾えていた本文が減ることはない
- マーカーは**実採取の画面に在る文言だけ**を載せる。採っていない見出し（`Edit file` 等）を
  当て推量で足さない — 一致しなければ従来の抽出へ落ちるだけで、増やす利得より
  「会話文が偶然一致して起点が上へずれる」害のほうが大きい。
  死に文字列を防ぐ番犬が `issue633_起点マーカーは実採取に実在する`
- **空行を境界にする案は採れない**（claude の実ダイアログは本体に空行を 3 つ挟む = 実採取）
- A/B は `TAKO_633_LEGACY=1`。番犬は
  `crates/tako-control/tests/issue633_permission_command_anchor.rs`（legacy 腕で 2 本 FAILED）

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

**番犬（#965 で常設）**: `crates/tako-control/tests/shell_scripts.rs` が
**リポジトリ全体の `.sh`** を走査して、この形を file:line で名指しして落とす
（CI の macOS ジョブで走る）。行コメントは展開されないので対象外、`$1（` のような
位置パラメータも対象外（bash は数字を 1 桁しか読まないので全角を取り込まない）。
走査は #1518 まで `scripts/` 限定で、広げた初回に `distribution/build-pkg.sh:30` の
`$arg（--sign のみ対応）` が出た（「不明な引数」案内が出ず `arg…: unbound variable` に
なっていた = #837 とまったく同じ形）。手元での洗い出しは:

```sh
grep -nR --include='*.sh' -E '\$[A-Za-z_][A-Za-z0-9_]*[^\x00-\x7f]' scripts distribution
```

## シェルスクリプトは macOS 同梱の bash 3.2 で通す（Issue #1499 / #1518）

**CI の macOS ランナーが `scripts/*.sh` を走らせるのは `/bin/bash`（3.2.57）**で、
Homebrew の bash 5 が入っている開発機とは**空配列の扱いが違う**。3.2 は `set -u` の下で
**空配列の `"${arr[@]}"` を「未定義」として落とす**（5.x は空展開して通る）。

```bash
set -u
args=()
cmd "${args[@]}"                 # ✗ bash 3.2: args[@]: unbound variable
cmd ${args[@]+"${args[@]}"}      # ○ 空なら何も渡さない（3.2 / 5.x 共通）
```

手元で緑・CI で赤になる典型で、`bash -n` では見つからない（その行が
**空のまま**実行されるまで潜伏する）。#1499 の実測は PR の CI で `PASS=43 FAIL=9`、
`/bin/bash` で同じ結果を再現 → 慣用句へ直して `PASS=52 FAIL=0`。

**新しいスクリプトは push 前に `/bin/bash <script>` で 1 回通す**
（`bash <script>` は開発機では 5.x に解決されるので検査にならない）。

### 落ちるのは「演算子の無い展開」だけ（#1518 で実測）

`/bin/bash` 3.2.57・空配列・`set -u` で確かめた表。**過大にも過小にも申告しない**ため、
検査もこの 1 点に絞る。

| 書き方 | 空配列のとき |
|---|---|
| `"${a[@]}"` / `"${a[*]}"`（クォートの有無を問わず） | **`a[@]: unbound variable` で即死** |
| `${a[@]+"${a[@]}"}` | 何も渡さない（= 望みの挙動） |
| `${#a[@]}`（長さ）/ `${!a[@]}`（添字） | 通る |
| 演算子つき（`:-` `:+` `:1` `#pat` `/pat/rep`） | 通る |

`${a[@]:-}` は落ちないが**空要素が 1 個生える**（`for` が 1 周回る）。新しく書くなら
`+` の慣用句にする。`scripts/nightly-release.sh` の `release_locks` は `:-` + `[[ -n ]]` で
同じ結果にしてある既存の書き方。

### 番犬と例外宣言

`crates/tako-control/tests/shell_scripts.rs` の
`空配列の展開はbash32の慣用句で守られている` が**リポジトリ全体の `.sh`** を走査して
file:line で名指しして落とす（#837 の全角展開の検査と同じファイル）。押さえどころ:

- **`set -u` を宣言しないライブラリも対象**（`scripts/lib/*.sh` / `scripts/promo/lib.sh` は
  宣言した側から source されるので同じ条件で動く。#1518 の 41 箇所のうち 4 箇所はここに居た）
- **クォート付きヒアドキュメント（`<<'X'` / `<<"X"` / `<<\X`）の本文は対象外**。囲む側が
  1 文字も展開しないので落ちない（`promo/lib.sh` が書き出すデモ用スクリプトの本文が該当）。
  素の `<<X` は展開されるので本文も見る
- 配列が絶対に空にならないと**監査して確かめた**箇所だけ、その行か直前の行に
  `# tako:bash32-ok <理由>` で外せる。**理由の無い宣言は無効**

#### #1518 の実測

棚卸しは 41 箇所 / 12 ファイル。うち**実際に空が渡る経路が 4 系統**あった:
`check-windows.sh:92`（呼び出し側が `--all-targets` を渡すと `ALL_TARGETS=()`・即死）/
`promo/lib.sh` の `PROMO_ENV_CLEAN`（`TAKO_*` も `CLAUDE*` も無い環境で空・即死）/
`release.sh --promote` の `ASSETS` / `wait-pr-checks.sh:378`（未登録 0 本のまま衝突を
観測すると `MISSING=()`。`$( )` の中で死ぬので**外側は生き延びるが案内の中身が消える**）。
残りは「いまは空にならない」だけなので、次に空が渡る変更で同じ形で落ちる。

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
- **生死を引く材料そのものが採れなかった回は 1 件も消さない**（#1597。上の
  「材料が採れなかった回は『不在』ではない」）
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

## ゲートを足す変更は、そのゲートを通る隔離テストも同じコミットで直す（Issue #1452 / #1493）

**認可・所有者・呼び出し元を見るゲートを足したら、そのゲートを通る実経路テストを
全部洗い出して同じコミットで直す。** #1452 は `POST /api/admin/pair/approve` に
「呼び出し元が tako-app か」のゲートを足し、隔離テストのための逃し口
`TAKO_REMOTE_TRUSTED_ADMIN_NAMES` を同時に用意して既存 2 本へ宣言を入れたが、
**同じ日に着地したばかりの `scripts/test-remote-fs-1451.sh` だけが漏れた**。
承認が 403 `upgrade_requires_gui` で通らなくなり、端末が登録されないまま
**OK=18 NG=55** で 1 週間走り続けた（#1493）。
落ちているのがテストの前提なのか製品の回帰なのかは、後から見ると誰にも分からない。

### 書くときの決まり

- 洗い出しは経路の具体形で引く（`grep -l '/api/admin/pair/approve' scripts/*.sh`）。
  **同じ PR でゲートと一緒に直す**（後追いの Issue にしない）
- ゲートが 1 つとは限らない。#1452 は承認の呼び出し元ゲートと同時に
  `request_pairing` の**降格の扱い**も変えていた（現より弱い role の要求は保留を作らない）。
  役割を下げるのは `POST /api/admin/devices/role` が現行の契約で、
  「`POST /api/pair` + 承認」は上げる向きにしか効かない
- 逃し口（`TAKO_REMOTE_TRUSTED_ADMIN_NAMES` / #841 の `TAKO_REMOTE_TRUSTED_PEER_NAMES`）は
  **隔離テストの中だけ**。本番の経路（リリース・セットアップ・製品コード）へ置くと、
  名前を知っている誰にでもゲートが開く
- 隔離テスト側は「撃ったこと」ではなく**「据わったこと」を確かめてから先へ進む**。
  role が外れたまま進むと以降の全項目が 403 で落ち、真因が数十件の NG に埋もれる

### 機械強制

`crates/tako-control/tests/issue1493_admin_gate_script_watchdog.rs`。経路表
（`remote_role::ROLE_ROUTES` の `grants_upgrade`）を叩く `scripts/*.sh` が逃し口を
宣言しているか・逃し口がテストの外へ出ていないかを **file:line で名指し**する。
経路表が正本なので、昇格できる経路が増えたら検査対象も自動で増える。

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
- 終了コードは共通で **0 = 全部緑 / 1 = 失敗・拒否 / 2 = タイムアウト / 3 = 引数・gh のエラー /
  4 = base と衝突している**。`merge-pr.sh` の 0 は「**merge が成立している**」で、
  **すでに MERGED だった PR への再実行も 0**（#1430。望んだ終わり方に到達しているので
  後始末だけ確かめて終わる。merge されずに CLOSED なのは 1）
- **衝突しているあいだ CI の run は作られないので、待たずに 4 で終わる**（#1365）。base が進んで
  衝突すると GitHub は merge コミットを作れず **`pull_request` の run を作らない**ので、
  `gh pr checks` には外部連携の `Cloudflare Pages` だけが並び、macOS / Windows は永久に未登録のまま
  （#775 の PR #1359 で 3 回。症状から原因 = `mergeable=CONFLICTING` へ辿るのに時間を食った）。
  待ち側は**未登録が 1 本でも残っているあいだだけ** `gh pr view --json mergeable` を引き、
  衝突を **2 回連続**で観測したら名指しで案内して終わる（`UNKNOWN` = 計算中と空文字は待ちを続ける。
  `gh pr view` が取れなければ判定を省いて待ちは従来どおり）。判定・案内文・終了コードは
  `scripts/lib/pr-conflict.sh` の 1 実装で、**待ち側と `merge-pr.sh` の門が同じ言い方をする**。
  A/B は `TAKO_1365_LEGACY=1`（衝突を見ずにタイムアウトまで待つ腕）
- 検証は `bash scripts/test-wait-pr-checks.sh`（偽 gh + 偽リポジトリ。**CI の macOS ジョブで
  毎 PR 走る**）。A/B は `TAKO_1333_LEGACY=1` = 修正前の判定をそのまま再現する腕で、
  Test 13 / 14 が「1 本だけで完了と返る」「揺れの 1 回目で確定する」を固定している。
  衝突（#1365）は Test 20〜23 が「待たずに 4」「legacy はタイムアウトまで待つ」
  「`UNKNOWN` は待ち続けて緑まで進む」「判定材料が無いときは従来どおり」を固定する
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
- **ローカルブランチも `gh` に任せない。そして `gh` の終了コードを merge の成否として読まない**（#1430）。
  後始末（ローカル削除・リモート削除）は**すべて merge の後**に走るので、gh が 1 で終わっても
  PR は MERGED になっている。実測（使い捨てリポ + 実 gh 2.88.1）: 専用 worktree からの
  `gh pr merge --squash --delete-branch` は **rc=1 / PR は MERGED / リモートもローカルも残る**。
  それでも `merge-pr.sh` 自身の終了コードは #1347 の時点から 0 だったが、**出力が**
  `failed to run git: fatal: …` と `警告: merge は済んだが gh が 1 で終わった` の 2 行で
  失敗に見えたので、worker 3 本（PR #1421 / #1423 / #1424）が merge 失敗と誤読して
  人手確認に回った。直したのは 3 点:
  - ローカル head も `merge-pr.sh` が消す。誰も握っていなければ `git branch -D`、
    **作業ツリーが握っているときだけ**「どこが握っているか」と
    `git worktree remove <path> && git branch -D <head>` を名指しして残す（正当な例外）
  - gh の非ゼロは「merge の失敗ではない」と言い切る**注記**（stdout）にし、成否は
    `gh pr view` の状態の実測だけで決める。最後の 1 行は必ず
    `merge 成立: PR #N は MERGED（url）/ 終了コード 0`
  - **すでに MERGED なら後始末だけ確かめて 0**（再実行・重複実行を失敗に化けさせない）
  A/B は `TAKO_1430_LEGACY=1`（ローカルを後始末せず、MERGED の再実行を 1 で拒む腕）。
  モックは Test 24〜29 が固定し、**ローカルブランチと作業ツリーだけは実 git** を触る

## CI の Windows ジョブの契約（Issue #1278 / #583 / #1264）

**Windows ジョブの `cargo test --workspace --no-fail-fast` は blocking**。
壊れたら CI は赤になる。ここを `continue-on-error` へ倒し戻してはいけない
（番犬 `crates/tako-control/tests/ci_windows_test_compile.rs`）。

- #583 は「POSIX 前提の既存失敗が残っているあいだ非ブロッキング」で据え置いていたが、
  **そのあいだに未検出が溜まった**。実機のベースラインは 19 → **24 件**まで増え、
  それが見えたのは #1264 でコンパイルが直った 5 日後（#1278）。
  非ブロッキングは「今の赤を許す」だけでなく「**新しい赤が見えなくなる**」
- **`--no-fail-fast` は必須**。既定の cargo は最初に失敗したテストバイナリで打ち切るので、
  その先のクレートが 1 件も走らない（実測 2026-09-22 の main: tako-app の 1 件で止まり
  tako-control / tako-core は 0 件実行）。全数が採れないと 1 回の CI で 1 件しか直せない
- ステップの順は「コンパイル検査（`--no-run`）→ 名指しの実行検査（Win32 FFI = #1282 /
  psmux = #1314）→ **全数**」。名指しを**手前**に置くのは、全数が赤いと以降のステップが
  skip されて「FFI は通ったのか」が読めなくなるため。名指しのステップに
  `--workspace` を付けてはいけない（全数を 1 本に保ち、ログで責任を分ける）

### CI ランナーと実機は別物として扱う

Windows ランナーには **psmux / tmux / claude / codex CLI が無く、セッション 1 でもない**。
同じ `cargo test` でも実機とは違う集合が走る（実機のベースライン = `.agent/plans/2026-08-windows-main-merge-wip.md`）。
**CI が緑 ≠ 実機が緑**。実機の照合は従来どおり失敗名で突き合わせる。

### Windows で成立しないテストの扱い（3 択。黙って塗り潰さない）

1. **テストの前提を直す**（第一選択）。ほとんどは「期待値を `/` 直書きで持っている」
   「`/tmp` を直書きしている」「PATH を `:` で組んでいる」「macOS のレシピ文字列を
   リテラルで持っている」。**期待値は製品の正から作る**（`Path::join` の結果・
   `agent_install::current_recipe`・`std::env::join_paths`・`std::env::temp_dir`）。
   `display()` の文字列で比べるのをやめて **`Path` 同士で比べる**だけで直るものも多い
   （`Path` の比較は components 単位なので区切りの綴りに依らない）
2. **実行時に理由を出して skip**（材料が作れない環境）。`eprintln!("skip: …")` +
   `return` で、**なぜ作れないか**を書く（例: 「Windows は symlink 作成に昇格が要る」
   「非 unix の `process_alive` は常に true = 死んだ pid を報告しない」）
3. **`#[cfg(...)]` / `#[cfg_attr(windows, ignore = "理由")]`**（その OS に仕組みが無い /
   製品側の穴が別 Issue で追われている）。**理由の無い skip は禁止**で、
   番犬 `issue1278_ignore_reason_watchdog` が `#[ignore]` /
   `#[cfg_attr(…, ignore)]` を `file:line` で落とす。さらに
   **Windows だけを外す skip は理由に追跡番号（`#1557` 等）を持つ**こと
   （= allowlist が黙って増えない）。`#[cfg(...)]` で外すときは
   **すぐ上に doc コメントで理由**を書く（属性からは理由が読めないため）

**製品側の穴をテストの skip で隠すときは必ず Issue を立て、`ignore` の理由文に番号を書く**
（例: #1557 = `remote::is_process_alive` が Windows で常に false）。
「直したら ignore を外す」が Issue 側からも辿れる状態にする。

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

## 「変わっていないから書かない」の材料は保存内容から機械的に導く（Issue #1425）

`save_layout` は 2 秒 tick と dispatch ごとに呼ばれる。ここで「変わったか」を
**実物を組んでから比べる**（`capture` + `serde_json::to_string` の文字列比較）と、
変化が無い tick でも全ペインの `PaneMeta` 構築と全体の直列化を払う
（#1001 C6 の実測: `メインスレッド専有: save_layout が 32ms` / 22 ペインで
**確保 319 回・29893 バイト**）。判定は**組む前**に置く。

- **判定材料は借用のまま流す**。`PaneMeta` はペインあたり `String` を 7 本作るので、
  変化検出のためだけに組んではいけない。`layout::PaneMetaRef`（借用版）を
  ダイジェストへ流し、`to_meta()` で保存形へ落とすのは**変化があったときだけ**
  （実測: 借用のままなら確保 **0 回**）
- **保存漏れは速さより重い**（「消えた」系 #30 / #177 / #770 の根はすべてここ）。
  キーに載せ忘れたフィールドは**保存されなくなる**ので、載せ忘れを 3 段で止める:
  1. **コンパイルで止める**: `PaneMetaRef::to_meta` / `feed` は全フィールドを
     網羅的に分解する（`let PaneMetaRef { .. } = self`）。`PaneMeta` / `PaneMetaRef` /
     `WindowFrame` / `LayoutExtras` にフィールドを足すとコンパイルが通らない
  2. **テストで止める**: `layout::CHANGE_KEY_FIELDS`（`(構造体, フィールド)` の宣言）を
     番犬 `issue1425_save_layout_change_key_watchdog` が #916 の指紋
     （`testdata/persisted_schema_fingerprint.txt`）と**集合として**突き合わせ、
     さらに宣言 1 件ごとに「動かすとキーも JSON も動く」検査を要求する
  3. **実行時の保険**: 連続スキップが `layout::RECONCILE_AFTER_SKIPS`（30 回 = 約 60 秒）に
     達したら、キーが一致していても必ず実物と突き合わせる。載せ忘れがあっても
     「永久に保存されない」にはしない
- **「変化したら書く」の判定と「保存内容を埋める」は同じ値を使う**。`capture` では
  埋まらない UI 側の付帯情報（ウィンドウのフレーム・折りたたみ・Web ビュー dock）は
  `layout::LayoutExtras` に**1 度だけ**組み、キーの算出と capture 後の穴埋めの両方が
  それを読む。2 か所で別々に組むと片方だけ更新されて保存漏れになる
- **並びが揺れる入れ物は判定の前に整える**。折りたたみ中のタブは `HashSet` 由来なので、
  そのまま JSON へ流すと**中身が同じでも並びが変わって毎回書く**。`collapsed` は
  昇順に整えてから渡す
- 内訳は `tako persist`（MCP `tako_persist`）の `save_layout`
  （`calls` / `skipped` / `captured` / `written` / `reconciled`）で読める。
  A/B は `TAKO_1425_LEGACY=1`（旧の「組んでから比べる」を同一バイナリで再現）

## 機械全体の設定を倒す記録には「所有者」を書く（Issue #1373 / #449）

**`data_dir` の記録は複数の tako-app プロセスで共有される**（隔離されるのは
`TAKO_ISOLATED` / `TAKO_DATA_DIR` を立てたときだけ）。「書き換えるのはこのプロセスだけ」を
前提にした記録は、2 個目のインスタンスが起きた瞬間に前提が崩れる。

蓋閉じ継続（`lid-guard.json`。Windows の `GUID_LIDCLOSE_ACTION` を 0 へ倒し、元値を
そこへ記録する）で実際にこうなった:

- busy な A が倒す → idle な B の tick が `set_stay_awake(false, …)` で**A の記録**を読み、
  元値へ戻して記録を消す。A は自分の写しを信じて倒し直さないので、
  **A の画面は「有効」のまま実機は蓋を閉じると眠る**（macOS 側で #449 として直した事故と同型）
- 記録が素の `fs::write`（truncate → write）で、窓に当たった読み手が serde 失敗を
  「記録なし」へ丸め、倒した 0 を元値として記録し直す = **ユーザーの設定が永久に失われる**
  （#169 の三段連鎖がそのまま再現する）

### 書くときの決まり

- **記録に所有者（pid + 起動時刻）を持たせる**。`RecordOwner::current()` で書き、
  戻すのは「自分の記録」「所有者が死んでいる記録」「所有者を持たない旧形式」だけ。
  **生きた他プロセスの記録には倒す側も解除側も触らない**（相手が解除した次の tick で取り直す）
- pid だけでは**pid の再利用**を見分けられない。起動時刻
  （`procinfo::start_time_unix`）と対にして、食い違ったら「所有者は死んだ」と読む。
  生死を判定できないときは**触らない側**へ倒す（語彙は `test_residue::Owner` の 1 実装）
- 所有権の判定は**probe を引数で受ける純粋関数**にする（`lid::claim_for`）。
  OS 依存を追い出しておかないと、Windows 実機の無い CI で分岐を 1 つも固定できない
- 「誰の記録か」と「何をするか」を分ける（`claim_for` → `decide` → 実行器）。
  入口（毎 tick 呼ばれる `set_stay_awake`）が直接 `restore` を呼ぶ形に戻さない
- 所有者の居ない記録を引き取るときは**必ず「戻してから倒し直す」**。
  倒れたままの現在値を元値として記録し直すと、ユーザーの設定が消える
- 書き込みは `config_io::atomic_write`、read-modify-write は `config_io::lock_exclusive` の下で
  **ディスクを読み直してから**（ロックは「書くと決まってから」取る = 上節）
- 読めない記録は「記録なし」へ丸めず `<name>.unreadable.bak` へ写して Err
  （#916 の作法。**元のファイルは触らない**）。丸めた先に待っているのが上の消失
- 記録の型は `migration_registry` の指紋へ載せる。フィールドを足すときは
  `serde(default)` で旧ファイルがそのまま読めるかを明示する

番犬は `crates/tako-control/tests/lid_guard_ownership.rs`（原子書き込み / 丸めない /
ロックの下で読み直す / 所有権を確かめてから戻す / 所有者を記録する / probe を注入する、の 6 本を
ソースで見て file:line で名指しする）と `platform::lid` の単体テスト（疑似プロセスの
probe で 4 通りの立場と判定表を固定する。**macOS でも走る**）。

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
- そのうち **tako が作る部分は 18.5 KB 以内**で、残り **5.5 KB** は
  プロファイルの `prompt_blocks.append`（個人環境のルール）の取り分。追記がこれを超えると
  `tako migrate` が見出し境界へ `<!-- tako:on-demand -->` を入れ、その行より後ろは
  `tako orchestrator guide local-rules` で引く形になる（**内容は 1 文字も消さない**）
- **MCP で公開するツールカタログは 200 KB 以内**（`tools/list` の応答）。
  MCP を繋いだエージェント全員が起動時に名前 + 説明 + inputSchema の全文を受け取る
  tako 自身の生成物なので、超えたらツールを隠すのではなく 1 本ずつの説明文を短くする

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
（戻ったエントリは下の番犬が名指しで落とす）。

なお **union はその PR 自身の rebase では効かない**。属性は rebase 中に checkout されている
ベース側のツリーから読まれるので、この行が main に入った後の PR から効き始める。

### 戻ったエントリは番犬が見る。目視は補助（Issue #1645）

着地キューを 1 本ずつ rebase すると、`context-budget fix` の移送（古いエントリを**消す**）と
main 側の別 PR の移送が噛み合っても **git は衝突を報告せず auto-merge する**ので、
main で既にアーカイブ済みのエントリが `progress.md` へ黙って戻る（9/23 だけで 4 回。
**予算を超えなければ件数・バイトの番犬では落ちない**）。

- 判定の正本は `tako_core::context_budget::revivals`。鍵は `LogEntry::archive_line()` =
  `fix` がアーカイブへ書くのと同じ 1 行（(日付, Issue 番号) の組では同じ日・同じ Issue の
  別エントリを誤検出する。アーカイブに実在する）
- CI の番犬 `crates/tako-control/tests/issue1645_progress_revival_watchdog.rs` が
  `file:line` で落とし、`tako context-budget` / MCP `tako_context_budget` の
  violations にも同じ違反が出る。**rebase 後の目視は補助**
- 直し方は 2 ファイルの復元:
  `git checkout origin/main -- .agent/progress.md .agent/progress-archive.md` で main の状態へ戻し、
  自分の 1 件を書き直してから push する（どちらの版が正かは機械には決まらないので自動では直さない）

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
  **非 macOS は `None`** = その案内を出さない。Win+クリック / Win+Enter は押せないので、
  表記を Windows 風に置き換えるのではなく**案内ごと落とす**のが正しい
  （`shortcut_hint_for` が非 macOS の platform 修飾バインドを落とすのと同じ規則）
- **リンクを開く「修飾 + クリック」だけは例外**で、`keys::link_click` /
  `keys::link_modifier` が**両 OS とも表記を返す**（`Option` ではない）。#763 で
  打鍵そのものを Windows は Ctrl へ移したので、案内を落とす理由が無い。
  下の「リンクを開く修飾キー」節を見ること
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

## リンクを開く修飾キーは 1 箇所で決める（Issue #763）

ターミナルの URL / パス・PDF 注釈・Markdown・リリースノートの**リンク経路は
`keybindings::link_modifier_active` を通す**。GPUI の `Modifiers::platform` を
直読みすると、Windows では Win キーになって Win+クリックを要求する形になり、
OS のシェルに食われてユーザーはリンクへ到達できない（#763 の症状）。

- 判定の正本は `tako_core::platform::keys::link_modifier_active(platform, platform_key, control)`。
  **macOS = command のみ / Windows = control のみ**で、`platform || control` を素で書くのは駄目
  （macOS の Ctrl+クリックは右クリック相当なので、コンテキストメニューとリンク開きが同時に走る）
- 表記は同じ表を見る `keys::link_modifier` / `keys::link_click`（`⌘+クリック` /
  `Ctrl+クリック`）。MCP カタログ・CLI ヘルプへ `cmd+クリック` と直書きしない
- 合成マウスイベント（セルフテスト）の修飾も `keybindings::link_modifiers` から組む。
  `platform: true` を直書きすると**実装だけ Ctrl へ移って Windows のセルフテストが落ちる**
- 番犬は `crates/tako-control/tests/issue763_link_modifier_watchdog.rs`（4 規則:
  ホバー呼び出しの実引数 / クリック判定の `if` 条件 / マウスハンドラ本体 / 合成イベント）

## MCP カタログの説明文は AI が使える情報だけ載せる（Issue #1540）

`tools/list` の応答は **MCP を繋いだエージェント全員が起動した瞬間に全文を受け取る**
固定費で、予算表の中で最大（`AGENTS.md`「起動時ロードの予算」節）。
ここへ書く文は「AI がその場で行動を決められるか」だけで採否を決める。

### 書かないもの

- **Issue 番号（`#1234` / `Issue #1234`）**: AI は番号から Issue を引けないので、
  トークンだけ食って情報を運ばない。実測で 152 本中 83 本・195 箇所あった。
  **根拠は `catalog.rs` のソースコメント `// 出自: #…` へ残す**（情報は失わず、
  ロードされる文だけ軽くする）。番犬は
  `crates/tako-control/tests/mcp_catalog_snapshot.rs` の
  `mcp説明文にissue番号を書かない`（description / inputSchema の全文字列を見る）
- **歴史的経緯**（「かつては〜だった」「〜する手段がなかったが」）: 今の挙動だけ書く
- **他のツールにある背景説明の再掲**: 正本を 1 本決めて「読み方は tako_xxx の説明」と
  参照する（送達フロー `delivery` の項目説明は `tako_read_pane` が正本、
  選択肢ダイアログの番号縛りは `tako_orchestrator_respond` が正本）
- **引数ごとに繰り返す共通の但し書き**（`（set 時）` を 20 引数に付ける等）:
  ツールの description へ 1 回だけ書いて引数からは落とす

### 必ず残すもの（圧縮は削除ではない）

1. **いつ使う**（どんな状況で呼ぶツールか）
2. **何が返る**（応答のキー名と enum の受理値。AI が分岐に使うので略さない）
3. **前提ツール**（引数の出どころ。`tab` は `tako_list_panes`、`worker` は
   `tako_orchestrator_spawn` の返り値、といった導線）
4. **失敗時**（何がエラーになるか、`next_step` / `recommended_action` をどう読むか）

説明文から外した根拠・仕組み・経緯・例は捨てずに `.agent/mcp-catalog-notes.md` へ
ツールごとに原文で残す（#1711 で 156 本中 47 本を短くしたときの置き場）。

### `next_step` を返すなら説明に読み方を書く

dispatch が `next_step` / `degraded` を返す経路は、**そのまま実行できる手順**を
文字列で持っている（`#1049` で「劣化を埋もれさせない」と決めた形）。
主たる利用者は AI なので、返す側のツールの説明に
「自分で手順を組み立てず next_step に従う」を明記する。現在の該当は
`tako_remote_status`（`degraded.reason` / `degraded.next_step`）/
`tako_context_budget`（`violations[].next_step`）/ `tako_setup_bootstrap`（`next_step`）/
`tako_orchestrator_profiles` / `tako_git_resolve_agent`（`remote_control_blocked.next_step`）/
`tako_orchestrator_spawn`（`launch_warnings[].next_step`）/
`tako_sessions`（`remote_link.next_step`）/ `tako_remote_folder`（`failed[].next_step`）/
`tako_ui_mode`（`pane_display_reason[].next_step`）。

## MCP カタログの enum は正本から生成する（Issue #1467）

`crates/tako-control/src/mcp/catalog.rs` の `"enum": [..]` は **MCP クライアントへの申告**で、
実際に受け取れる値（dispatch の `parse`）とは別の実装になっている。ここへ値を直書きすると、
**正本へ値を足した側は写しの存在を知らない**まま古くなる。壊れるのは動作ではなく申告なので、
CLI でも dispatch でも単体テストでも緑のままになる。

#1450 B2 が `PanelViewWire` へ `tasks` を足したときが実例。CLI の possible values は
`VALUES` から組み立てるので `tako panel --view tasks` は通り、`parse` も通るので寛容な
クライアントからは動いた。一方で `tako_panel` の `inputSchema` は
`["fleet","orch","git","tmux"]` のままで、**enum を尊重するクライアントは `tasks` を送れず、
enum を読んで選ぶエージェントはビューの存在自体を知れない**（設計原則 5 に反する）。

- 値の一覧を持つ型（`VALUES` / `ALL` / `all()`）が在るなら、カタログは
  **`enum_schema(正本, 説明)` で生成する**。`tako_panel` の `view` は
  `panel_view_schema()`（受理値 = `VALUES` + `LEGACY_VALUES`、案内文 = `values_hint()`）
- **説明文も正本に持たせる**。`PanelViewWire::summary()` は `match` なので、
  変種を足すとコンパイルが通らない = 値だけ増えて説明が古い状態にならない
- **旧称（後方互換だけで受理する値）は enum から落とさない**。落とすと、いま動いている
  クライアントが送れなくなる。案内文では「旧称」と明記して勧めない（#553 と同じ扱い）
- 正本が非公開・列挙 API が無いなどで生成へ寄せられない語彙は、番犬
  `crates/tako-control/tests/issue1467_mcp_enum_watchdog.rs` の `registry()` へ
  **`Bound`** として登録し、値が正本と一致することだけでも縛る。登録簿にある語彙と同じ
  値集合を登録簿の外が手書きで持ったら落ちるので、**写しは増やせない**
- カタログを変えたら `TAKO_UPDATE_MCP_SNAPSHOT=1 cargo test -p tako-control
  --test mcp_catalog_snapshot` でスナップショットを更新し、**差分が意図どおりか**を diff で
  確認する（公開契約の変更なので、通りすがりの更新をしない）

## docs の数値と Issue 参照は「正本から」「追跡先は open」（Issue #1547 / #1316）

docs は**数字と Issue 番号だけが静かに古くなる**。文章は読めば違和感が出るが、
「47 件」「128 個」「追跡: #984」は読んでも古いと分からないので、**機械で縛る**。

### 数値は正本から検査する

docs が名乗る件数は、数え直せるものなら必ず番犬に載せる。正本は
MCP ツール = `tako_control::mcp::tools()`、CLI コマンド = `tako-cli` の `enum Command`、
エージェント能力 = `tako_core::agent_support::MATRIX`。番犬は
`crates/tako-control/tests/docs_tool_inventory.rs` の 1 本にまとめてある。

- **同じページの中を全部見る**。#1547 のヒーロー統計は、本文が「152 個」と書いている
  そばで `<span class="tako-stat-num">128</span>` を配信していた（番犬は本文だけを見ていた）。
  数字を足す場所を増やしたら `Claim` / `CountClaim` も足す
- **「見つからない」も FAILED**にする。文面が変わって拾えなくなった番犬は、
  緑のまま何も守らない
- 手書きページに数字を書くくらいなら**生成ページへ寄せる**ことを先に検討する
  （`agent-support.md` / `windows-support.md` は生成物なので、この種のズレが構造的に起きない）

### Issue 番号は「追跡先」と「根拠」を分ける

| 種別 | 意味 | 状態 |
|---|---|---|
| 追跡先 | まだ残っている仕事の在り処 | **open でなければならない** |
| 根拠 | 済んだ調査・実装・実測の引用 | closed でよい（確定した事実として強い） |

書き分けは `pending(note, N)` の `N`（追跡先）と、`Note` 本文・`evidence` の中の
`#N`（根拠）。docs 側は生成器が追跡先だけを「追跡: [#N]」と書く。

閉じた Issue を追跡先に置いたままにすると、docs には「追跡: #N」と出るのに N を開くと
完了していて、**読む側（利用者・AI エージェント）が残りの仕事を辿れなくなる**。
#1547 の棚卸しでは #757 / #983 / #984 / #1033 / #1067 が閉じたあとも 13 マスが指し続け、
docs の Issue 参照 17 件のうち 10 件が closed だった。

- 元の Issue が閉じてもマスが `Pending` のまま残るなら、**残りを持つ open な Issue
  （無ければ親エピック）へ付け替える**。済んだ調査は `Note` 本文へ「#N で確認」の形で残す
- 検査は `scripts/check-docs-issue-refs.sh`（CI の macOS ジョブ）。`gh` が無い / 未認証の
  環境では検査せず抜ける（open かは GitHub に問う以外に知る方法が無く、ローカル作業を
  止める理由にならない）

### docs のビルドと検査は CI で通す

`astro build` は Cloudflare Pages が代行しているが、**代行はリポジトリの CI ではない**
（壊れたまま merge できてしまう）。macOS ジョブで `npm run build` +
`npm run og:verify` + `node docs/scripts/verify-links.mjs` を通す。Rust より**前**に
置いてあるのは、docs の壊れがビルド 40 分の後ろに隠れないようにするため。

内部リンクは**断片（`#見出し`）まで見る**。生成ページの見出しは件数で変わる
（`agent-support.md` の系統別の節は「対応が 1 件でもあるか」で文言が分かれる）ので、
手書きページからのアンカーが静かに切れる。

配信の設定も同じジョブで見る（#1708）。`node docs/scripts/test-middleware.mjs` は
旧ドメイン転送（`docs/functions/_middleware.js`）が細工したホスト・ポート・パスでも
常に `https://tako.takushio2525.com` へ 301 することを、`node docs/scripts/verify-headers.mjs`
は `dist/_headers`（正本は `docs/public/_headers`）に HSTS・埋め込み禁止・CSP の基本指令が
載っていることを確かめる。後者は URL を渡すと本番のレスポンスヘッダも検査できる。
CSP でスクリプトの出どころを縛るなら、検索（Pagefind）の WebAssembly のために
`'wasm-unsafe-eval'` が要る（無いと検査が落ちる）。

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

**検証スクリプトから隔離 GUI を立てるなら、起動はヘルパの 1 行を呼ぶ**（#1490）:

```sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins                     # TAKO_BIN / APP_BIN を決める（無ければビルド）
launch_isolated_gui "$TMP/app.log" || exit $?   # ensure → 面と窓の env → 起動（pid は $ISOLATED_GUI_PID）
wait_isolated_gui "$TMP/app.log"      # `tako list` が通るまで待つ（任意）
stop_isolated_gui                     # 自分で起こした pid だけを落とす
```

`launch_isolated_gui` は**起動の直前に毎回 `ensure` を通す**。これが要点で、
**蓋閉じ運用の `tako-vd` はアイドルで眠り、眠った面は tako から見えなくなる**ので
（下の #1160 の項）、冒頭で 1 回だけ起こす形では長い検証の途中や 2 回目の起動で
「GUI が立たない」で止まる（#1487 の worker が同じ検証に 3 回失敗した）。
`ensure` は冪等（常設の面を作り直さない・消す機能は無い）なので毎回通して困らない。
起動ごとに env を足すときは `launch_isolated_gui "$TMP/app-legacy.log" TAKO_1487_LEGACY=1`、
窓の矩形が要るときは `ISOLATED_GUI_BOUNDS=100,50,1400,900`（既定は指定なし =
置き先の中央 960x600 のまま。窓の実寸を測る検証の実測値を動かさないため）。
落とすのは `stop_isolated_gui` = **自分で起こした pid だけ**で、
`pkill -f tako` のような名前一致は本番 GUI と他 worker に当たるので使わない。

**面を用意できなければ起動しない**（#1744）。`ensure` が失敗した・成功と言ったのに面が
OS の一覧に無い（`bounds` で読み戻す）ときは、`launch_isolated_gui` は窓を開かずに
**終了コード 4**（tako 本体の「窓を開かずに終わる」と同じ番号）で返し、stderr へ
「未実測: …（理由）」を 1 行出す。以前は「起動は続ける」で素通しする作りで、面の指定を
持たない起動が tako の暗黙の既定に乗って**ユーザーの画面へ窓が出うる**潜在経路があった
（この道で窓が出た実例は確認されていない）。
蓋閉じ + ディスプレイスリープでは `ensure` が面を起こせないのが日常なので、
**呼び出し側は必ず `|| exit $?` で止め、報告に「未実測」と書く**（関数の中でも `exit` で
止める。`|| return 1` は呼び手が戻り値を捨てると 4 が消える）。一部の段だけ未実測にして
続けたいときは `launch_isolated_gui … || GUI_RC=$?` で受けて自分で明記する
（`test-setup-check-single-source-1505.sh` の C/D 段）。未配線の機（器が無い・CI）でも
同じで、**既定の面で続行する道は作らない**。`TAKO_DISPLAY` を手で渡しても面を用意できなければ
起動しない（狙いが `tako-vd` なら見失っているし、別の面ならユーザーの画面へ出す）。

**`tako-vd` 以外の面の明示は通さない**（#1760）。面を用意できても、`launch_isolated_gui` は
GUI へ渡す `TAKO_DISPLAY`（呼び出し側の `VAR=VAL` → export → 既定の `tako-vd` の順で 1 つに
決めた値）を起動の前に判定し、通さない値なら窓を開かずに**終了コード 2**で返して
stderr へ「ERROR: TAKO_DISPLAY=… は通さない…」を 1 行出す（4 = 未実測とは分ける。書き方の誤りを
未実測と読ませない）。通すのは表の上 4 行だけで、opt-in で別の面を通す口は作らない:

| 値 | 通すか | 理由 |
|---|---|---|
| `tako-vd`（大文字小文字・前後の空白は無視） | 通す | tako は名前で `tako-vd` に当てる |
| `ensure` が記録した `tako-vd` の uuid | 通す | 記録が無い・違う uuid は通さない |
| 空（`TAKO_DISPLAY=`） | 通す | tako の定義で未指定 = 検証用の既定で `tako-vd` を探す |
| `index:N` / `N`（N ≥ 100） | 通す | 実在し得ない面。tako は見失って窓を開かずに 4 で終わる |
| `0` / `index:1` など N < 100 の index | 通さない | 並びは起動までに変わりうる。`tako-vd` を指していても通さない |
| 別の面の名前・当たらないはずの名前 | 通さない | 名前が読めない瞬間もあり、その名前の面が無いことを確かめられない |

「わざと当たらない面」を指したい検査は `TAKO_DISPLAY=index:999` を使う
（`test-display-uuid-1697.sh` の ④）。面の名前は面を用意する係と同じ `TAKO_VD_NAME` だけを見て、
ヘルパ側で別に差し替える口（旧 `ISOLATED_GUI_DISPLAY` の上書き）は無い。

番犬は `crates/tako-control/tests/issue1490_isolated_gui_launch_watchdog.rs`。
`scripts/test-*.sh` に `target/…/tako-app` / `cargo run -p tako-app` /
`TAKO_DISPLAY=tako-vd` の直書き・手書きの背景起動・手書きの `ensure` が生えたら
file:line を名指して落ちる（除外はヘルパ自身と `test-virtual-display-guard.sh` だけ）。
起動の失敗を拾っていない呼び出し（#1744）と、ヘルパが面を用意できないのに起動する形、
`tako-vd` 以外の面の明示を通す形（#1760）も落とす（後の 2 つはヘルパを `/bin/bash` で実際に
走らせ、`ISOLATED_GUI_VD` で偽の係を注入して偽の GUI が起きないことを見る）。

**眠った状態を手で再現する**（受け入れ検査用）: `pmset displaysleepnow` を撃つと面が眠り、
`virtual-display.sh status` が「眠っている」に変わる（蓋閉じで内蔵が居ない機なら
落ちるのは `tako-vd` だけ）。`virtual-display.sh` に眠らせる口は**作っていない**:
内蔵や外部モニタが居る機で撃つとユーザーの画面を消すので、ヘルパに置くと事故の口になる。

シェルスクリプト以外（手で 1 回起こす・セルフテスト・収録）は従来どおり:

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
- **`TAKO_DATA_DIR` が深くても CLI / MCP は届く**（#1441）。IPC の Unix ソケットの
  実体は data dir 直下ではなく **`$TMPDIR/tako-<data dir の 16 桁ハッシュ>.sock`**
  （上限に収まるときだけ従来どおり `<data_dir>/tako.sock`）で、data dir 側には
  参照ファイル `tako.sock.path` だけが残る。worker の scratchpad のような深いパスを
  そのまま `TAKO_DATA_DIR` に渡してよい。**旧挙動は `warning: IPC サーバーを起動できない`
  の 1 行だけ出して GUI は普通に立つ**ので、隔離起動の「見た目は動くのに CLI が無反応」を
  見たらまずここを疑う。受け口が立ったかは `tako check-health`（`ipc` 節の `bound` /
  `kind` / `path_bytes` / `limit`）で読める。**アプリへ届かないときもローカル診断が出る**
  ので、`tako list` が「接続情報が無い」と言ったら `tako check-health` を打つ。
  置き場の決め方は `tako_core::ipc_socket` の 1 実装（A/B は `TAKO_1441_LEGACY=1`）
- **面を指定するのは `TAKO_DISPLAY=<名前 | UUID | index>`**。`TAKO_ISOLATED` /
  `TAKO_SELF_TEST` / `TAKO_VISUAL_TEST` のどれかが立っていれば**未指定でも** `tako-vd` を狙う
- **名前が読めない瞬間がある**（#1697）。名前の出どころ（`system_profiler`）が読めない起動では
  全部の面が `name=?` になり、生きている `tako-vd` を名前だけで探して見失う。`ensure` の締めが
  面の uuid を `~/Library/Caches/tako/virtual-display/tako-vd.uuid` へ残し、tako は
  **名前が読めない面に限って**その uuid で当てる（persist.log の解決行が `recorded_uuid で解決`）。
  `virtual-display.sh status` の末尾に「記録 uuid: 先頭 8 桁…（いまの面と一致 / 不一致）」が出る
- **窓の位置・寸法は tako 側の口で決める。AX（System Events）は使わない**（#1442）。
  AX は複数の tako-app を **unix id にかかわらず同一プロセスとして返す**ので、
  `first application process whose unix id is <隔離 pid>` を掴んで `set position` すると
  **本番 `/Applications/tako.app` の窓が動く**（ユーザーの窓を動かす事故が実際に起きた）。
  使う口は 3 つで、座標はどれも**そのディスプレイ内**（左上が原点）:

  ```sh
  # 起動時に置く（x,y,w,h。`w,h` だけなら置き先の中央へ）
  env "$(scripts/lib/virtual-display.sh window-env 1400 900)" \
    TAKO_ISOLATED=1 TAKO_DISPLAY=tako-vd cargo run -p tako-app
  # 起動後に動かす（MCP `tako_window` の move / resize と 1:1）
  tako window move 300 200 && tako window resize 1600 1000
  # 結果を読む（bounds = 窓の矩形 / display = 置き先の面）
  tako window list
  ```

  最小寸法（200x150）未満と置き先からのはみ出しは撥ねる。起動時は既定へ落ちて
  理由が persist.log に残り、CLI / MCP からはエラーとして返る（窓は動かない）。
  `scripts/lib/virtual-display.sh move-window` は**当座しのぎの AX 経路**で、
  tako の pid を渡すと拒否して上の口を案内する
- **通常起動は 1 ビットも変わらない**（未指定 かつ 非検証なら置き先は未解決のまま）
- **通常起動**で指定が外れても**起動は止まらない**。既定の面へ落ちて persist.log に
  理由 + 候補が 1 行残る。どこへ置いたかは `tako_check_health` の `display_placement` で読める
- **面が 1 枚も見えないとき、検証用の起動は既定の面へ落ちない**（#1160）。列挙をやり直し
  （100ms × 20）、それでも空なら**窓を 1 枚も開かずに終了する**（終了コード 4）。
  ユーザーの画面に検証用の窓を出すのは「開かない」より悪い
- **面が見えているのに当たらないときは、`TAKO_DISPLAY` 未指定（暗黙の既定）なら落ちる**（#1160）。
  その機に置き先が無いということで、ここで開かない構えにすると `tako-vd` を配線していない
  環境（CI・他人の機・**Windows**）で検証そのものが回らなくなる。ただし**黙らせない**
  （起動時に stderr へ警告）。理由が persist.log しか無くて気づけなかったのが #1160 の症状の一部だった
- **`TAKO_DISPLAY` を明示したのに見失ったら、検証用の起動は落ちずに窓を開かずに終わる**（#1697。
  終了コード 4・stderr に理由と候補の全行）。狙った面を見失っただけで、置き先が無いのではない。
  `isolated-gui.sh` は面を用意できたら**常に**明示する（用意できなければそもそも起動しない =
  上の #1744 の項）ので、ヘルパ経由の起動が暗黙の既定に乗るのは、呼び出し側が
  `TAKO_DISPLAY=`（空）を並べたとき（`test-display-uuid-1697.sh` の ③）だけ。
  `tako-vd` 以外の面の明示はヘルパが通さない（上の #1760 の項）
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
  暗黙の既定なら落ち（そこで止めると置き先を配線していない環境で検証が回らない）、
  `TAKO_DISPLAY` を明示していれば開かずに終わる（#1697）
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
