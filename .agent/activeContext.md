# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-27・#570 で git タブの UX 4 件を一括改善）

直近: `fix/551-560-561-562-git-tab-ux` → PR #570（`b62c325`）を merge。#551 / #560 / #561 は close、
**#562 は実機目視が残っているため open 維持**。

- #551: git タブの本文順を 変更 → コミット → ブランチ → リモート → diff へ。既定の折りたたみは
  「変更 + コミット = 展開 / ブランチ + リモート = 折りたたみ」。リポジトリ切替時は
  `GitCollapsed::for_repo` で畳み直す（リモート 20 件超は必ず折りたたみ）
- #560: 変更ファイル行のクリックでプレビュー表示（`open_file_row` = dispatch `OpenFile` 経由）。
  + / − ボタンは伝播を止めるので両方は起きない。削除済み（D）はクリック対象外
- #561: **コミット欄の IME が効かない根因は「変換対象がターミナルペインに束縛されていた」こと**。
  `AppTextInput` + `ImeComposition.app_input` で宛先を型にし、下線のインライン描画・候補位置・
  unmark の確定先を入力欄側へ。ブランチ名欄も同じ経路に載せた
- #562: マージボタンが `opacity(0)` + 行ホバーでしか出ず「無い」と読まれていた。常時表示 +
  アイコン + 案内行 + ブランチチップからの導線を追加

副産物: UI アイコン定数が `EMBEDDED_ASSETS` に登録済みかの検査テストを新設（未登録だと
`svg()` が無言で何も描かない）。既存の remote.svg が描かれていなかったのを検出して修正。

`/Applications/tako.app` は `b62c325` を install 済み。**稼働中の GUI は旧バイナリなので再起動が要る**。

## これまでの対象（2026-07-27 未明・#553 でパネルビューの語彙を GUI と一致させた）

直近: `fix/553-fleet-vocab` で #553 を解消。GUI のタブは fleet / orch / git なのに CLI / MCP の
`--view` は tmux / orch / git しか受けず、画面に見えている語で操作できなかった（設計原則 5 の前提崩れ）。
**`PanelViewWire::Fleet` を正式値化**し、語彙の正本を protocol.rs の
`VALUES` / `LEGACY_VALUES` / `parse` / `values_hint` に集約して CLI・MCP 双方がそこから引く形にした。
旧称 `tmux` は `serde(alias)` + `parse` で受理し続けるが、応答 JSON は必ず `fleet` に正規化する。
tako-app 側の `PanelView::Tmux` も `Fleet` へ改称（`PanelView::Tmux => PanelViewWire::Fleet` という
食い違いの再発を構造で防ぐため）。

その前: `fix/530-prompt-delivery` で #530 を根治。根因は疑われていた「シェル段階の誤判定」ではなく
**claude の番号付き選択ダイアログ（初回テーマ選択 `❯ 2. Dark mode ✔` / ログイン方法選択）の
選択カーソルを入力欄と誤認していたこと**。`CLAUDE_CONFIG_DIR` を切り替えると初回に必ず出るため、
account env 注入つき spawn 特有の症状になっていた。`is_choice_dialog`（文言非依存の構造判定）を
新設して `input_line` から除外し、送達の証拠を「入力欄が空」から「貼り付けが入力欄へ反映された」へ
変更。未達は `prompt_delivery=undelivered` + `prompt_delivery_failure` + `resend_command` で報告する。

その前: #548 で `tako orchestrator accounts` を追加し、アカウント系（#511 / #512 / #547 / #548）の
欠落は全て解消。#547 で master_account を master / solo / handoff の起動へ適用。
さらに前に `fix/511-512-account-polish` で CLI `spawn/run --account`（#511）と
accounts.yaml の `inherit: true`（#512。既定パス明示 → Keychain 別エントリ問題の根治）を実装。
**ローカル accounts.yaml の personal を inherit 形式へ更新済み → 古いバイナリでは
パースできないので tako の再起動が必須**。`~/.claude-univ` を検証事故で失っており、
univ アカウントの worker は初回に 1 回ログインが要る。

## 次の一手

1. **tako を再起動**して `b62c325` を反映（git タブの 4 件はここから体感できる）
2. **#561 の実 IME 目視**: この機には日本語入力ソースが有効化されていない
   （`AppleEnabledInputSources` は ABC + パレットのみ）ため実変換を走らせられなかった。
   Issue #561 のコメントにチェックリストがある
3. **#562 / #496 の GUI チェックリスト**: マージ導線の目視 + コンフリクトカード / 狭幅 220pt
4. **Windows 実機ビルド**: `.agent/windows-setup.md` の手順で `cargo build`。
   macOS 側は `scripts/check-windows.sh`（クロス check）が緑の状態
5. `worker_account: personal` への切替 / renewal/remote-transport の統合・v0.6.0 準備

## 未着手・持ち越し

- #496 のカード描画（コンフリクトカード / マージ確認カード / 狭幅 220pt）の**目視のみ**未確認。
  実装・CLI・dispatch・MCP・ペイン読み取りでの検証は完了済み
- `claude_tui_e2e`（#32 系。`--ignored` の手動実行専用）が **main 時点で 2 件落ちている**:
  `事前信頼でダイアログなしの送達が通る` / `残留テキストをenter単独送達で送信できる`。
  どちらも `ensure_trusted` を書いたのに信頼ダイアログが出る（claude v2.1.220 で
  `hasTrustDialogAccepted` だけでは足りなくなった疑い）。#530 の変更前後で同一結果 = 回帰ではない

## GUI 検証の環境知見（#496 で判明。次回の時間浪費を防ぐ）

- **蓋が閉じている（外部ディスプレイ無し）と GUI 検証は一切できない**。ウィンドウは
  存在するが再描画されず、`screencapture` は「could not create image」、合成クリックも
  届かない。`ioreg -r -k AppleClamshellState` で**最初に**確認する（`= No` なら蓋は開いている）
- 蓋が開いていれば **`cliclick` の実クリック + `screencapture -R<x,y,w,h>` で実機検証ができる**。
  ウィンドウ位置は `osascript -e 'tell application "System Events" to tell (first process
  whose unix id is <pid>) to get {position, size} of front window'`。#570 はこれで
  #551 / #560 / #562 の目視まで完了させた
- 合成**キーボード**入力は使えない（cliclick は「キーボードレイアウトを扱えない」で拒否、
  AppleScript keystroke は IME に吸われる）。**キー入力の検証はセルフテスト項目で行う**
- この機には**日本語入力ソースが有効化されていない**（`AppleEnabledInputSources` は
  ABC + 文字ビューア / 絵文字パレットのみ）。IME 実変換の検証はユーザー実機でしかできない
- **`git stash` はリポジトリ共有**。worktree で `git stash` → 他 worker が stash → 自分が
  `git stash pop` すると**他人の stash を pop して drop する**。退避は `git diff > patch` を使う
- セルフテスト項目 76（blur 後の focus 自己修復）は**実行ごとに成否が変わるフレーク**
  （origin/main でも再現）。落ちたら再実行する
- 本番 tako と他ワーカーの隔離インスタンスが同時に動いている。クリック前に
  「その座標の最前面が自分のウィンドウか」を CGWindowList で必ず確かめる
  （AppleScript の `whose unix id is N` は別プロセスに誤マッチする）
- 本体リポ `~/dev/tako` が `main` をチェックアウトしている（2026-07-27 時点）

## 現フェーズで Read すべき設計書

- Windows 移植を進める: `.agent/plans/2026-07-windows-port-architecture.md` + `.agent/windows-setup.md`
- 新機能を足す: `crates/tako-core/src/platform/support.rs` の MATRIX に必ず分類を足す
  （忘れるとパリティテスト T1 / T3 が落ちて merge できない）
- git タブに手を入れる: `crates/tako-app/src/right_panel.rs` の `GitScrollBody`（#494 構造不変条件）
- プロンプト送達に手を入れる: `crates/tako-control/src/claude_tui.rs`（画面判定の純関数）+
  `main.rs` の `drive_prompt_flows`。遷移診断は `TAKO_PROMPT_FLOW_DEBUG=1`（画面内容は出さない）
