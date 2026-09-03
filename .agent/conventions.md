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
  （項目 110 の `body +2 header +2` は「意図的な全体 notify」と同じ値。#858）。
  窓の汚れは `chrome_renders`（キャッシュしたクローム 4 枚。#786）が動いたかで
  **可視ペインの枚数に依らず**判定できる。時間で動くもの（ヘッダの時計 #803）と
  持ち越し（`term_pending_app`）は測る前に窓の外へ出し、外から来る汚れは
  **検出してやり直す**（上限つき・各試行を記録・全滅なら FAILED）。
  やり直しが本物の回帰を隠さないことは「窓が汚れていない状態で増分が出る」
  注入（`TAKO_858_INJECT=header`）で毎回確かめられる
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
  新しいタブへ移って残りのタブを閉じ、ルートを本番と同じ経路（`sync_filetree_roots`）で
  作り直してから撮る。**用意した前提そのものも `check` で見る**（前提が崩れたときに
  「検査対象が壊れた」と読み違えないため）。最後のタブは閉じられない
  （`close_tab` が `LastTab` を返し UI 層がアプリを終了させる）ので、
  **新しいタブを作ってから**残りを閉じる

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

### テキスト置換で表せない移行（1 ファイル → 複数ファイル）

引き継ぎのプロジェクト単位化（#915）のように**分割・置き場の変更**を伴う移行は
`Step`（テキスト → テキスト）では表せない。手順そのものは専用実装が持ち、
`migrations::run` から呼んで結果を [`MigrationReport`] の語彙へ翻訳する
（`handoff_reports` が実例）。**番地（`SchemaId`）には必ず載せる**: 載せないと
`tako migrate` から見えず、発火点が分かれる。

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
