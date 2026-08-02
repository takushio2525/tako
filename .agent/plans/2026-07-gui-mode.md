# GUI ライク表示モード（初心者向け UI）詳細設計 — エピック Issue #691

> 作成: 2026-07-31。対象コミット: `3eaf4f3`（main）。
> テーマトグルの横に「GUI ⇔ ターミナル」表示切替を新設し、GUI モードでは
> ペインを「ボタンとチャット」の画面にする。ターミナルに抵抗感がある初心者が、
> 黒い画面を一度も直視せずに AI オーケストレーションを使い始められるようにする。

## 0. コンセプトと設計原則

**ターゲット**: ターミナル未経験・抵抗感のあるユーザー。障壁は「黒い画面に流れる文字」
「何を打てばいいか分からない」「壊しそうで怖い」の 3 つ。GUI モードはこれを
①最初の画面がボタン 3 つ（次の行動が常に明確）②AI との対話が Claude アプリ風チャット
③スラッシュコマンドやコンテキスト管理を「知らなくても押せるボタン」に翻訳、で解消する。

**設計原則**:

- **表示レイヤのみの切替**: PTY / tmux バックエンド / TerminalSession / persist は両モード完全共通。
  GUI モードは同じペインの**別レンダラ**にすぎない（`render_pane` の分岐）。だからトグルを
  ターミナル側へ倒せば「バックで動いていたターミナル」がそのまま見える（§2.5 で裏付け）
- **dispatch フロントエンド原則**（settings-ui 設計と同じ）: GUI の各操作は既存 / 新設の
  dispatch を GUI 内から直接呼ぶ。CLI / MCP と同一コードパスで 1:1 が構造的に成立
- **既存部品の再利用**: md レンダリング（preview_render）・AppTextInput（IME #561）・
  カード UI（#666）・バナー / パレット（#549）・transcript 正規化（PWA で本番稼働中）。
  GPUI pre-1.0 のため新規 UI 表現は増やさない
- **規約**: 絵文字禁止（SVG アイコン）/ 色は Theme 経由 / 文言は `tr!` 日英必須

**Non-goals（v1 でやらないこと）**:

- codex / agy ペインのチャット化（transcript 正規化が claude 形式のみ。将来 #120 の agent 抽象で拡張）
- alt_screen TUI（vim 等）のチャット化（構造的に不可能。ターミナル表示へフォールバック）
- ペイン単位の表示モード永続化（§1.3。「コマンド入力へ」ボタンによる揮発的な解除のみ）
- リモート PWA の置き換え・変更（データ源を共有するだけ。§3.3）
- チャット履歴の横断検索・過去セッション一覧（sessions カタログ #112 との統合は将来）

## 1. モードの定義と切替 UX

### 1.1 モードの意味

`ui_mode: "terminal"（既定） | "gui"` のグローバル 2 値。既定 terminal なので
**既存ユーザーの体験は一切変わらない**。GUI モードにしても全ペインが変貌するのではなく、
ペイン種別ごとに表示を決める（§2.1 判定表）。チャット化するのは claude 対話ペインだけ。

### 1.2 トグル UI

タブバー右端コントロール群（⌘K・ベル・テーマ）のテーマボタンの**左隣**に配置
（tab_bar.rs の右端概算幅を +30px 更新）。アイコンは SVG 2 種を新設
（GUI = チャット吹き出し、terminal = プロンプト記号 `>_`）。現在モードのアイコンを表示し、
クリックでトグル。ツールチップ「かんたん表示に切り替え / ターミナル表示に切り替え」（日英）。
⌘K パレットにも「表示モードを切り替え」を常設（welcome 3 項目と同じ流儀）。

### 1.3 グローバル単位にする設計判断

タブ・ペイン単位ではなく**アプリ全体で 1 つ**にする。理由:
(a) 要望が「ナイトモード（テーマ）のトグルの横」= アプリ全体の見た目と同格の概念
(b) 初心者にモードの入れ子（アプリは GUI だがこのタブはターミナル…）を持たせない
(c) ペイン単位の永続状態は layout.json・復元・CLI 粒度を複雑化させる。
判定表により GUI モード内でも自然にハイブリッド表示になるため、粒度の不足は実害がない。
唯一の例外はスターターの「コマンド入力へ」= そのペインだけターミナル表示にする
**揮発フラグ**（`HashSet<PaneId>`、永続化しない。再起動で GUI 表示に戻る）。

### 1.4 永続化と dispatch / CLI / MCP

- settings.json に `ui_mode: String` を追加（`theme` と同パターン、serde default = "terminal"）
- dispatch `Request::UiMode { action, mode, pane }` 新設（`Theme` と同型）—
  `action` 省略 / `status` = 現在値取得、`set`（`mode` = `gui | terminal`）/ `toggle` =
  変更 + settings 保存 + 全ウィンドウ即時反映（TakoApp は全窓共有 entity #339 のため
  cx.notify で足りる）、`release` / `restore` = §1.3 の**揮発フラグ**をペイン単位で操作
  （永続化しない）。応答は `{ ui_mode, available, released_panes }`
  （release / restore は `pane` / `released` も返す）
- CLI `tako ui-mode [gui|terminal|toggle|release|restore] [--pane N]`
  （引数なしで現在値。`tako theme` と同型）
- MCP `tako_ui_mode`（131 → **132 ツール**。セルフテストのツール数期待値を更新）

> G1 実装時の追加（2026-07-31 / #694）: 当初案は `mode` 1 個だけだったが、
> スターターの「コマンド入力へ」に対応する AI 操作が存在しないと開発不変条件
> （UI でできることは AI からもできる）を満たせないため、`release` / `restore` を
> 同じ `UiMode` に足した。新設 dispatch は依然 `UiMode` の 1 本だけ。

## 2. GUI モードの画面仕様

### 2.1 ペイン種別ごとの表示決定（判定表）

GUI モード時、各ペインは毎 render 時に以下で表示を決める（上から先勝ち）:

| 条件 | 表示 |
|---|---|
| スターター揮発解除フラグあり | ターミナル表示 |
| alt_screen 中（`is_alt_screen()`） | ターミナル表示 |
| claude 対話 TUI 稼働（`agents::live_claude_sessions` の pid 祖先解決で session_id が取れる、または role が master / solo / worker で claude） | チャットビュー |
| 子プロセスなしのアイドルシェル（既存 sleep_guard 系の子プロセス判定を再利用） | スターター |
| **過渡期の猶予が残っている（#720。ペイン生成 / エージェント起動から上限内）** | **準備中プレースホルダ** |
| それ以外（コマンド実行中・不明 TUI・codex / agy） | ターミナル表示 |

判定は**保守的**に倒す: チャット化は claude と確定したときだけ。不明はターミナル表示
（誤ってチャット化する方が、ターミナルのままより実害が大きい）。判定関数は
tako-core / tako-control 側に純関数で切り出し、unit test 可能にする。

> G1 実装（#694）: 純関数は `tako_core::ui_mode::pane_display(PaneDisplayInput)`。
> 材料は毎 render 組み立てるので**新しいサブプロセスを要する材料は入れない**:
> 「アイドルシェル」= OSC 133 由来の `CommandState::Idle`（プロンプト表示中 = 前のコマンドは
> 終わっている）+ role なし + バックエンドセッションに実行中の子プロセスなし
> （sleep_guard の 2 秒 tick が背景で計算した `busy_backend_sessions` を流用。#372）。
> `Failed`（直前のコマンドがエラー）と `Unknown`（シェル統合の signal が無い）は
> 画面を隠さずターミナル表示にする。terminal モードでは材料を一切見ずに即決する
> （既存描画への影響を分岐 1 つに抑えるため）。
>
> **G2 で判明した重要な訂正（#702）: `alt_screen` は「中で動くプログラム」の状態でなければ
> ならない**。表の 2 行目を素直に `TerminalSession::is_alt_screen()` で実装すると、
> persist が有効な環境（= 既定）では**全ペインが常に alt screen 扱い**になり、GUI モードが
> まったく機能しない。tmux バックエンド越しのペインでは、外側のエミュレータが見ているのは
> **tmux クライアント自身の alt screen** だから（実測: 素のシェルのバックエンドペインで
> 外側 `alt=true` / tmux 側 `#{alternate_on}=0`）。
>
> 対処は `TakoApp::pane_inner_alt_screen`: バックエンドペインでは外側のフラグを使わない。
> 中身の alt screen を毎 render 引くには tmux への問い合わせが要るが、**表の他の材料が
> 同じ役割を果たす**ので不要 — 全画面 TUI はペインの子プロセスとして動くため
> `busy_children` が真になりアイドルシェルの条件から外れ、チャット化は claude 確定時
> だけなので未知の TUI には及ばない。
>
> **#720 で足した「過渡期」の行**: 上の表を素直に適用すると、ペインを作った直後
> （プロンプトが出るまで）と `tako master` を押した直後（claude が起動しきるまで）が
> 「それ以外 → ターミナル表示」に落ち、direnv のロードログやシェルプロンプト、
> 起動途中の TUI が数秒だけ映って消える。隔離実測（セルフテスト項目 96）で
> 修正前が `[Terminal, Starter]`、修正後が `[Preparing, Starter]` になることを確認した。
>
> 猶予は `tako_core::ui_mode::SettleState`（`kind` + 経過時間）で表し、**上限を過ぎたら
> 必ず通常判定へ落ちる**ので「いつまでもローディング」にはならない。上限は 2 段:
> `SETTLE_SHELL_LIMIT`（プロンプト待ち。シェル統合が無い環境では永久に来ないので短く）と
> `SETTLE_AGENT_LIMIT`（claude の起動 + `claude agents --json` 登録待ち）。
>
> 猶予を張るのは 2 か所だけ:
> - `spawn_session` — **明示コマンドの無い**ペイン（素のシェル）。Code Runner（#453）の
>   ようにコマンド付きで作るペインは、その出力こそ見たいものなので覆わない
> - `starter_action` の master / solo — 押した瞬間から `SettleKind::Agent` を張り直す
>
> 外すのは `prune_pane_settle`（2 秒 tick）。**「行き先に着いたか」で判定する**のが肝で、
> エージェント待ちの行き先はチャットだけ。押した直後の「まだシェルがアイドル」=
> スターター表示を確定扱いにすると、コマンドが走り出す前に猶予が外れて結局その先で
> 生ターミナルが出る（実測して直した）。role 付きペインは素のシェルとして始まっても
> 行き先がエージェントなので長い方の上限を使う。
>
> `released` / `alt_screen` は**過渡期より優先**する。前者は「待たせる理由が無い」から、
> 後者は tmux 不在構成で claude TUI が外側の alt screen として見えるためで、そこは
> そもそもチャットにできない（G2 の帰結）= ターミナル表示が正しい行き先だから。

### 2.2 スターター（空ペインの 3 ボタン）

アイドルシェルのペインに、縦並びの大きなカード 3 枚（アイコン + タイトル + 1 行説明）:

| カード | 押下時の動作 | 裏で起きること |
|---|---|---|
| AI チームに任せる（オーケストレーション） | シェルへ `tako master` + Enter を書き込み | master が claude TUI を起動 → 判定表によりチャットビューへ自動遷移 |
| AI と 1 対 1 で話す | 同 `tako solo` | 同上 |
| コマンド入力へ（ターミナルを使う） | 揮発解除フラグを立てターミナル表示に | 何も起動しない |
| （カード外・下部の控えめなリンク）初期設定をやり直す | 同 `tako setup`（#720） | setup の対話ウィザードがそのペインで動く |

- 実行方式は welcome バナーの `launch_tako_command` と同じ**シェルへのコマンド文字列書き込み**。
  `master` / `solo` は CLI_ONLY（エージェント CLI の起動そのもので dispatch 化できない）のため
  この方式が正であり、副次効果としてターミナル表示に切り替えるといま実行されたコマンドが
  履歴に見える = 初心者の学習経路になる
- プロファイル選択（G4）: カード右端のシェブロン ▾ で `profiles_dir()` / `solo_profiles_dir()` の
  一覧をドロップダウン表示し `tako master -<profile>` を書き込む。既定プロファイルは
  1 クリック（Code Runner #453 の再生ボタン + ▾ と同じパターン、#322 最簡原則）
- 開発不変条件の充足: master / solo ボタンの等価操作は既存 CLI そのもの（`tako master` /
  `tako solo`）。「コマンド入力へ」だけは等価な既存操作が無いので `tako ui-mode release`
  （MCP `tako_ui_mode` の `release`）を §1.4 に足した。この対応を各フェーズの 1:1 表に明記する

> G1 実装（#694）: カードは `crates/tako-app/src/starter.rs`（描画とクリック配線のみ）、
> 押下時の実処理は `TakoApp::starter_action`（master / solo = シェルへ書き込み、
> コマンド入力へ = `UiMode` dispatch の `release`）。ヘッダには cwd と × を置く
> （GUI モードでもペインを閉じられるようにするため）。狭いペインでは説明文 →
> コマンド併記 → 脚注の順に落とす（見切れさせない。#185 と同方針）。
>
> #720 追加: 下部に `tako setup` の**控えめなリンク行**（カードと同格にはしない。
> 初回バナー #549 と役割が重なるため）。等価な AI 操作は既存の `tako setup` なので
> 新しい dispatch は増やしていない。**setup では過渡期の猶予を張らない** —
> setup は診断結果と質問をターミナルに出す対話ウィザードで、その出力自体が
> ユーザーの読むものだから覆う方が害になる（`StarterAction::expects_chat` が false）。

### 2.3 チャットビュー（claude 稼働ペイン）

Claude アプリ風の 3 段構成。**v1 に含める要素**:

- **ヘッダ**: モデル名 + busy / idle 状態（busy = スピナー + 「考え中…」、キュー滞留 =
  「送信済み・生成後に届きます」）+ **コンテキスト残量バー**（`claude agents --json` の
  `contextPercentUsed`。80% 超で警告色 + 「/compact で会話を軽くできます」ヒント表示）。
  生成中は TUI のスピナー行（`Manifesting… (5m 16s · ↓ 16.4k tokens)`）を**そのまま**出す
  （#719 要件 2。作業内容 + 経過時間 + 受信トークン数を独自に数え直さない）。
  **ペイン単位の「ターミナルを表示」ボタンは置かない**（#719 追加要件 4。チャットへ戻る
  導線が無く迷子になるため。モード切替はタブバーのグローバルトグルに一本化する）
- **メッセージ一覧**: `transcript::read_messages(session_id, tail=50)` を描画。
  user = 薄い背景ブロック / assistant = 地の文で md レンダリング（preview_render の
  Markdown 資産を再利用。コードブロック・テーブル・リンク #680 も同じ描画）。
  tool_use = 折りたたみカード（PWA ToolCard 相当: ツール名 + summary、クリック展開）。
  thinking は既定折りたたみ。下端自動追従 + 手動スクロールで追従解除 / 再開
  （リモート #63 のリーダービューと同じ振る舞い）
- **システム注入コンテンツの分類**（#715。実装は**正規化層** = PWA も同じ恩恵を受ける）:
  claude は「画像添付のメタ文」「`<task-notification>` 等の XML」を
  **user 発話と同じ形**で transcript に書くので、素通しすると会話に生 XML が並ぶ。
  `transcript::classify_user_content` が user 行を
  `Speech { text, images } / Notice { summary } / Skip` の 3 つへ分類する。
  チャットは画像を「画像」プレースホルダ、通知を薄い 1 行（連続分はまとめて件数表示）で描く
- **承認カード**: `detect_permission_dialog` が画面に実在を検知したときだけ表示
  （PWA #425 と同じ条件）。選択肢をボタン化し、押下で `Respond` dispatch。
- **入力欄（#719 でミラー方式へ変更）**: ローカル下書きを持たず、**claude TUI の入力行を
  そのまま映す窓**にする。打鍵・削除・ペースト・IME は横取りせず PTY へ素通し
  （`AppTextInput::ChatInput` は廃止）。したがって
  - 入力状態は常に 1 つ（TUI が正）= 表示モードを往復してもズレない
  - Enter / Shift+Enter・画像ペースト・ゴースト提案の見え方が TUI と完全一致する
  - キャレットは**実ターミナルカーソル**そのもの（別に持たないのでズレようがない）
  - 箱の高さは TUI 入力ボックスの行数に追従し（1 行なら 1 行ぶん = #718）、
    `CHAT_INPUT_MAX_ROWS`（8 行）で頭打ちにして「上に N 行」を出す
  映す範囲は `tako_core::screen::input_region`（プロンプト行を上下の罫線で挟む純関数）。
  送信ボタンは本文を組み立て直さず **Enter だけ**送る（#95 の Enter 単独送達）
- **スラッシュボタン列**（入力欄の下）: v1 は 3 つ固定 —
  「会話を軽くする（/compact）」「新しい会話（/clear、確認ダイアログつき）」「ヘルプ（/help）」。
  平易なラベル + 実コマンドを小さく併記（学習経路）。押下 = 入力欄を経由せず Send
- **コマンド提案カード（#666）のインライン表示**: 会話の中では **md コードブロック風**
  （`mantle` の背景パネル + 等幅 + 右上のコピー / 実行ボタン。#719 追加要件 6）。
  押下後の処理は `command_card_ui` の同じ経路 = CLI `tako show-command --copy/--run` と 1:1。
  ターミナル表示側の専用帯（#703）は不変

**将来に回す**: ファイル添付 / @ファイル参照補完 / 過去セッション一覧と resume /
メッセージ単位のコピーボタン / カスタムスラッシュボタン設定 / codex 対応。

### 2.4 チャット化しない / できないペイン

判定表の「ターミナル表示」に落ちたペインは既存描画そのまま（v1 では装飾を足さない)。
alt_screen TUI は原理的にチャット化できず、シェル実行中は下手に隠すと危険なため。
worker ペインは role=worker でも claude なら**チャットビューにする**（master が並べた worker の
進捗を初心者も同じ見た目で読める）。**入力欄も master ペインと同じものを出す**
（#719 追加要件 5。当初は「指示は master 経由」の原則で read-only にしたが、実運用では
worker への直接指示が日常的にあるとのフィードバックを受けて変更した）。
「通常は司令塔の AI が指示します」の説明行は残すが、入力は妨げない。
入力がミラー + パススルー方式（§2.3）なので、master と worker で構造は完全に共通。

### 2.5 「ターミナルに戻すと見える」の実現方式（裏付け）

トグルは `render_pane` 内の分岐を切り替えるだけで、`TerminalSession`（PTY）・tmux
バックエンドセッション・persist(layout.json)・スクロールミラーには一切触れない。
チャットの送信も承認も**同じ PTY への書き込み**（Send / Respond は claude TUI に打鍵する）
なので、ターミナル表示に切り替えると claude TUI 上に同じ会話が描画されている。
既存で「同じペインの表示だけ切替」は preview ペインの code ⇔ markdown ⇔ 履歴トグル
（FR-3.3 / #338）が先例。復元(#30/#177/#381)への影響もゼロ（layout.json 不変。
`ui_mode` は settings.json 側）。

## 3. データフロー設計

### 3.1 読み（3 ソース、すべて既存）

| データ | ソース | 更新方式 |
|---|---|---|
| 会話本文 | `transcript::read_messages(session_id, tail)`（正規化 JSON: role / text / thinking / tools / timestamp） | 既存 2 秒 periodic tick に相乗り + transcript ファイル mtime が変わったときだけ再読込 |
| session_id / モデル / ctx% | `agents::live_claude_sessions`（`claude agents --json`、TTL 2s キャッシュ #168、sticky 解決 #466） | 同上（キャッシュ済みのため追加コストほぼゼロ） |
| busy / 承認 / キュー | 画面採取（`claude_tui::is_busy` / `detect_permission_dialog` / `queued_messages_pending`） | worker_status と同じ offload 経路（UI スレッド非ブロック #168） |

**楽観 echo**: 送信直後に自分の発話をローカルで即時挿入し、transcript 反映（1〜2 秒）の
ラグを隠す。次回 transcript 読込で同内容が来たら echo を破棄して transcript を正とする。

### 3.2 書き（2 経路、すべて既存 dispatch）

テキスト送信 = `Send`（PromptFlow）、承認 = `Respond`。GUI 内から dispatch() を直接呼ぶ。
新設は `UiMode` のみで、書き系の新規経路は作らない。

### 3.3 リモート PWA との関係

PWA チャットビュー（#23/#42/#63/#425/#439）は同じ tako-control の関数群を HTTP 越しに
使っている。本機能は**同じデータ源を GPUI から関数直呼び**する別フロントエンド。
表示仕様（ToolCard / ApprovalCard の表示条件、承認の実在検知原則）を PWA と揃えるが、
PWA のコードには触れない。将来 codex 対応等でデータ源を拡張するときに両者が同時に恩恵を受ける。

## 4. 実装フェーズ分割

各フェーズ = worker 1 タスク。すべて品質ゲート（fmt / clippy -D warnings / test）+
隔離セルフテスト完走が前提で、下記は追加の受け入れ条件。

### G1: モード基盤 + スターター 3 ボタン

- settings.json `ui_mode` / `Request::UiMode` / CLI `tako ui-mode` / MCP `tako_ui_mode`（132）/
  タブバートグル + ⌘K パレット項目 / 判定表の「アイドルシェル → スターター」「それ以外 →
  ターミナル」（チャット判定は G2）/ スターター 3 カード（既定プロファイル、▾ なし）
- 受け入れ: ①ui_mode roundtrip（settings 保存 + 再起動復元）を unit + セルフテストで機械検証
  ②MCP `tako_ui_mode` toggle → 応答と GUI 状態が一致 ③セルフテストでスターターの
  「AI チーム」押下 → ペインのシェル入力行に `tako master` が現れる ④terminal モードでは
  描画が現行と同一（既存セルフテスト全項目が無変更で通る）

> **G1 実装済み（2026-07-31 / #694）**。G2 以降が触る場所:
> - 判定表の純関数 + モード型: `crates/tako-core/src/ui_mode.rs`
>   （`PaneDisplay::Chat` は enum に用意済み。`PaneDisplayInput.claude_chat` を
>   `true` にできるようにするのが G2 の仕事）
> - 判定材料の組み立て: `TakoApp::pane_display_for`（`crates/tako-app/src/main.rs`）
> - 分岐点: `render_pane` 冒頭（webview / preview の次。**PTY リサイズの後**に置いてある =
>   スターター表示中にリサイズしてもエージェントは正しい端末サイズで起動する）
> - スターター: `crates/tako-app/src/starter.rs` + 文言 `ui_text/ui_mode.rs`
> - トグル: `tab_bar.rs`（テーマボタンの左隣 + `HintTooltip`）/ パレット `toggle-ui-mode`
> - 検証: セルフテスト項目 93（判定 / MCP / 送達 / 揮発解除）+ visual-test
>   「スターター」節（dark / light の実ピクセル。`TAKO_VISUAL_DUMP_DIR` で PNG 保存）

### G2: チャットビュー（読み取り）

- claude 判定（§2.1）+ ヘッダ（モデル / busy / ctx バー）+ メッセージ一覧（md・ツールカード・
  thinking 折りたたみ・自動追従）+ worker の read-only 表示 + 「ターミナルを表示」ボタン
- 受け入れ: ①隔離実 claude ペインで対話 → user / assistant がチャットに表示される e2e
  ②vim（alt_screen）ペインはターミナル表示のまま ③GUI ⇔ terminal 往復で tmux セッション・
  PTY が同一（`tako list` の session が不変）④transcript 正規化の unit は既存を流用

> **G2 実装済み（2026-07-31 / #702）**。実装場所と、仕様から動かした点:
>
> - チャット状態と描画: `crates/tako-app/src/chat_view.rs`（文言は `ui_text/ui_mode.rs`）。
>   md 本文は #690 の `md_view::render_document` を通すので、プレビューペイン・
>   アップデート詳細と**同じ見た目**（見出し / GFM 表 / コード / 引用 / 強調）になる
> - 読み取りは定期更新（2 秒）へ相乗り: `collect_chat_targets`（UI・材料集め）→
>   `load_chat_refresh`（background・`live_claude_sessions_by_backend` 1 回 +
>   transcript の mtime / サイズが変わったときだけ再読込）→ `apply_chat_refresh`。
>   **新規ポーリングはゼロ**。terminal モードでは collect が即空を返す
> - **判定を仕様より 1 段厳しくした**（§2.1 の「または role が master / solo / worker で
>   claude」は採らなかった）。理由は 2 つ: ①session_id が無いと描く会話が無く、
>   カタログへフォールバックすると凍結した旧世代 transcript を見せる失敗（#466）に
>   戻る ②sticky 解決は agents の一時失敗に耐えるため **claude 終了後も記憶を保持する**
>   ので、生存の根拠をプロセス側（実行中の子プロセス）から取る必要がある。
>   結果、`tako_core::ui_mode::chat_session` は session_id + interactive +
>   子プロセス実行中の 3 点が揃ったときだけチャットにする
> - 上の帰結として、**チャットになるのは tmux バックエンドを持つペインだけ**
>   （live 解決の対応キーがバックエンドセッション名のため）。tmux 不在環境の直接
>   spawn ペインは GUI モードでもターミナル表示のまま
> - busy の判定は `claude agents --json` の `status` を優先し、取れないときだけ
>   画面採取（`claude_tui::is_busy`）へ落とす（#571 の階層に合わせた）
> - ctx バーは 80% 超で警告色まで実装。**「/compact で会話を軽くできます」ヒントは
>   G4 に残した**（押せるボタンとして出すのがスラッシュボタン（G3）と地続きのため）
> - 発話単位の md パースキャッシュ（内容ハッシュ）+ 状態は `Rc` 保持（毎フレームの
>   50 件 clone を避ける）。折りたたみ状態も内容ハッシュに紐づけるので、
>   tail=50 から古い発話が押し出されても開閉がずれない
> - モデル名は `claude agents --json` が返さない版がある（実測: claude 2.1.220 は
>   `model` も `contextPercentUsed` も欠落）。TUI フッターの `[Opus 5 (1M context) · xH]`
>   から拾う経路（`AgentMetrics.model`）を足し、agents 由来を優先・画面採取を予備にした
> - 下端追従は**内容が変わったフレームだけ** `scroll_to_bottom` する。毎フレーム寄せると
>   追従を外す前の 1 フレームでユーザーのホイールを巻き戻してしまう
> - 検証: セルフテスト項目 94（判定 / 描画 / 往復での tmux 不変 / 追従 / alt screen 除外）+
>   visual-test「チャット」節（dark / light の実ピクセル）+ 隔離実 claude の e2e

### G3: チャット操作（入力・承認）

- AppTextInput 入力欄 + Send 送信 + 楽観 echo + スラッシュボタン 3 つ + 承認カード + キュー表示
- 受け入れ: ①実 claude e2e: チャット入力欄から送信 → transcript に user 発話が現れ応答が
  表示される ②busy 中送信 → キュー表示 → 生成完了後に自動送達（#572 経路の再利用を確認）
  ③承認カード: permission ダイアログ実在時のみ表示され、ボタン押下でダイアログが解決する
  ④/clear ボタンは確認ダイアログを挟む

> **G3 実装済み（2026-08-01 / #716。表示品質バグ #715 と同 PR）**。実装場所と判断:
>
> - 入力・送信・承認は `chat_view.rs` に集約（`ChatInput` / `ChatEcho` / `SlashButton`）。
>   書き系は **`Send` と `OrchestratorRespond` の 2 つの既存 dispatch だけ**を使い、
>   新しい書き込み経路は作っていない（§3.2 のとおり）
> - 入力欄は `AppTextInput::ChatInput` を新設して IME の宛先を型で区別する（#561 と同型）。
>   キーの取り合いは `chat_input_pane()` が「フラグ + **いま本当にチャット表示か**」の
>   両方を見る形にした。フラグだけを信じると claude 終了後も打鍵が吸われる（#503 の再発）
> - `handle_chat_input_key` は git のコミット欄（#487 / #494）と同じ約束で組む:
>   `key_char` を使う / 修飾なしキーは必ず消費する / キャレットは常に文字境界へ丸める。
>   複数行なので Home / End は「その行の端」、上下も行移動ではなく端へ倒す
>   （折り返しを含む行移動の幾何は v1 では持たない）
> - 楽観 echo は**専用のキー空間**（`echo_key`）で持ち、transcript に同じ本文の user 発話が
>   来たら破棄する（`prune_chat_echo`。二重表示の解消）。45 秒で時間切れにするので
>   送達に失敗しても残り続けない
> - 承認カードの表示条件は**画面のダイアログ実在**のみ（PWA #425 / #577 と同じ）。
>   採取はプロセス内のスクリーン（`visible_lines`）なので新しいサブプロセスは増えない。
>   `Respond` 側も実在を再検証するので、表示だけ差し込んでも送れない（セルフテストで確認）
> - コマンド提案カード（#666）は `command_card_elements` を切り出して**帯と同じ描画関数**を
>   会話の中で使う。`pane_shows_terminal` が Chat を除外しているので帯とは二重にならない
> - 副産物: 極端に長い user 発話（実 transcript に 15 万文字の行が存在）は既定で
>   先頭 1200 文字 + 「続きを表示」に畳む。1 個の発話で会話が埋まるのを防ぐ
> - 検証: セルフテスト項目 95（入力 / 改行 / 空送信 / echo の生死 / スラッシュ /
>   承認の誤爆防止 / terminal 表示でキーを握らない）+ visual-test「チャット」節に
>   `chat-g3`（入力欄・スラッシュボタン・承認カード・インラインカードの実ピクセル）を追加

### G3.5: 起動の見え方（#720。G3 の後に実使用フィードバックで追加）

- 過渡期の準備中プレースホルダ（§2.1 の追加行）+ スターター下部の setup リンク（§2.2）
- 受け入れ: ①GUI モードで worker spawn / スターター起動 → チャット確定まで生ターミナルの
  フレームが 1 枚も出ない（実測）②claude が来ないケースは上限で通常表示へ落ちる
  ③「コマンド入力へ」の明示ターミナルは即表示 ④setup リンクから `tako setup` が起動する

> **実装済み（2026-08-01 / #720）**。実装場所:
>
> - 判定と上限: `tako_core::ui_mode`（`PaneDisplay::Preparing` / `SettleKind` / `SettleState`）
> - 猶予の管理: `TakoApp::{begin_pane_settle, prune_pane_settle}` + `pane_settle` マップ
>   （**永続化しない**。ペイン ID は再利用されるので `drop_gui_pane_state` で必ず落とす）
> - 描画: `starter.rs::render_preparing_pane`。枠とヘッダはスターターと共有
>   （`render_gui_pane_frame`）で、明滅は既存のドット表現（チャットの busy / タブバーの
>   実行中ドットと同じ）を使う。**アニメーションが毎フレーム再描画を要求する**ので、
>   上限が切れた瞬間に何もしなくても通常表示へ入れ替わる
> - プレースホルダのヘッダには「ターミナルを表示」を必ず出す。起動が固まったときに
>   上限を待たずに中身を見られる逃げ道（信頼ダイアログ等が裏で出ている場合の保険）
> - 開発不変条件: 過渡期は**状態であって操作ではない**ので新しい dispatch は作らず、
>   代わりに `UiMode` の status 応答へ `pane_display`（ペイン → terminal / starter /
>   chat / preparing）を足した。AI が「いま画面に何が出ているか」をそのまま読める
>   （脱出は既存の `tako ui-mode release`、setup リンクの等価操作は既存の `tako setup`）
> - 検証: セルフテスト項目 96（生成直後の表示列 / コマンド付きペインは覆わない /
>   上限で落ちる / setup リンク / master 押下。`TAKO_SELF_TEST_CLAUDE=1` で実 claude 通し）+
>   visual-test「準備中」節（一面インクのターミナルを覆って `covered_top=0`、
>   猶予を外すと同じ画面が戻る = PTY 不変の実ピクセル裏付け）

### G3.6: 本文の選択とコピー（#725。G3 の後に実使用フィードバックで追加。✅ 2026-08-02）

- 会話本文のドラッグ選択 + ⌘C / ⌘A。座標系はプレビュー（#145 / #656）と同じ
  **(行番号, UTF-8 byte)** を 1 ペインぶんの会話へ通しで採番するので、
  **発話をまたぐ選択**が自然に成立する。ヒットテスト（`preview_text_layout_hit_test`）と
  切り出し（`selection_text`）はプレビューと同一実装を共有する
  = 「見えているものとコピーされるものが一致する」ことの構造的な担保
- 選択できるのは会話本文だけ（ユーザー発話・assistant の md・開いた thinking /
  ツール出力）。ヘッダ・入力欄・カードのボタンは各々が mouse down の伝播を止めるので
  押下で選択が始まらない。**ドラッグ中は新着で下端へ飛ばない**
- 発話の右に固定幅の列を取り、そこへコピーボタンを置く（絶対配置で本文へ被せない）。
  渡すのは画面と同じプレーンテキストで、折りたたみ中でも全文。
  md コードブロックには #680 と同じコピーボタンが出る
- CLI `tako chat copy` / MCP `tako_chat_copy`（133 ツール）へ 1:1。
  ドラッグ選択そのものはポインタのジェスチャなので CLI へは写さない
  （AI の等価物は `tako read` / `tako orchestrator report` / `tako chat copy`）
- 受け入れ（実測済み）: ①合成マウスのドラッグで発話をまたいで選択 → ⌘C → pbpaste 一致
  ②コピーボタンで発話全文が pbpaste 一致 ③チャット内のコードブロックコピー
  ④選択がスクロール・入力欄・カードのボタンと干渉しない ⑤dark / light・折りたたみで破綻なし

### G4: 磨き込み

- プロファイル選択 ▾（master / solo 両カード）/ ctx 80% 警告 + /compact ヒント /
  i18n 総点検（tr! 検査）/ プラットフォームマトリクス #515 への登録 / manual-checks.md 追記
  （IME 実機・見た目）/ docs（features ページ + CLI / MCP リファレンス）
- 受け入れ: ①▾ からプロファイル指定起動 → シェルに `tako master -<name>` が入る
  ②パリティテスト T1〜T6 緑 ③docs build 緑

## 5. リスクと対策

| リスク | 対策 |
|---|---|
| GPUI pre-1.0 の破壊的変更 | 新規 UI は設定画面・カード・md レンダラで実績あるパターンのみで構成。独立ウィンドウや新規 FFI を使わない |
| transcript 反映ラグ（1〜2 秒）で「反応しない」と感じる | 楽観 echo（§3.1）+ 送信直後に busy スピナー即時表示 |
| transcript 肥大・md 描画コスト（#656 のテーブルレイアウトは重め） | tail=50 固定 + メッセージ単位の描画キャッシュ（内容ハッシュ不変なら再レイアウトしない）。perf.log の既存 watchdog で実測 |
| IME: 複数行入力・変換中の Enter | AppTextInput は git コミット欄で実績（#561）だが複数行は未検証 → G3 の manual-checks に実 IME 項目を立てる |
| 判定誤爆（チャット化すべきでないペインをチャット化） | 判定は claude 確定時のみ + 純関数化して unit test。誤爆時も「ターミナルを表示」ボタンで即脱出できる |
| `claude agents --json` の一時失敗で表示が揺れる | sticky 解決（#466）が既に吸収。チャット判定も直近成功値を保持 |
| ポーリング負荷の増加 | 新規ポーリングを作らず既存 periodic / TTL キャッシュ / offload（#168）に相乗り |

## 6. 調査結果（この設計の根拠）

- settings 永続化: `tako-control/src/settings.rs`（`theme: String` と同型で追加。serde default）
- dispatch: `tako-control/src/protocol.rs:159` `Request`（`Theme` / `Welcome` が引数省略 = 現在値の先例）
- トグル位置: `tako-app/src/tab_bar.rs:34`（右端コントロール群の概算幅コメント）
- スターターの起動方式: `tako-app/src/main.rs:6313` `run_setup_command` / `run_master_command` →
  `launch_tako_command`（シェルへコマンド文字列書き込み）。CLI_ONLY の根拠は
  `tako-cli/src/main.rs:7353`（master / solo は「エージェント CLI の起動そのもの」）
- transcript 正規化: `tako-control/src/transcript.rs:211` `normalize_lines`（role / text /
  thinking / tools / timestamp、requestId 統合、sidechain 除外）。`read_messages(session_id, tail)`
- session 解決: `tako-control/src/agents.rs`（pid 祖先辿り + sticky #466）。ctx% は
  `tako-control/src/orchestrator/mod.rs:2019` `contextPercentUsed`
- 画面採取: `tako-control/src/claude_tui.rs`（`is_busy` / `detect_permission_dialog` /
  `queued_messages_pending` / `is_alt_screen` は dispatch Read 応答 `alt_screen` にも公開済み）
- PWA チャットの先例: `web/tako-remote/src/components/chat-view.jsx`（ToolCard / ApprovalCard /
  md 表示）と `api.js`（messages / input / respond）
- md のペイン外再利用は #690（アップデート画面のリリースノート md 化、**進行中**）が並行の先例
