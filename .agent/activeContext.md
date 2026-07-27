# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-27・#571 で orchestrator watch の不検知を根治）

直近: `fix/571-watch-idle` → PR #578 を merge。busy → idle 遷移を watch が 40 分以上
見逃していた事故。**3 層の欠陥が重なっていた**（どれか 1 つでも直っていれば総損失は防げた）。

- **トリガー**: `claude agents --json` は**その config dir のエージェントしか返さない**のに、
  tako はプロセス環境の `CLAUDE_CONFIG_DIR` ごと実行していた。アカウント env つきのペインから
  GUI を起動すると（07-26 23:42 の再起動がまさにこれ）、そのアカウントの worker しか見えなくなる。
  `AgentScanTarget` を新設し、既定 + accounts.yaml の全 config dir を並行走査 → sessionId で
  重複排除。継承ではなく走査先ごとに明示指定する（rc に勝つためコマンド先頭の unset / export も併用）
- **増幅器**: `status == "unknown"` の画面フォールバックが `screen_looks_busy || has_children` で
  busy に上書きしていた。**エージェント CLI の TUI プロセス自身がペインシェルの子**なので
  has_children は生きている限り必ず true = 画面フォールバックは構造的に idle を出せなかった。
  プロセスツリーは「画面から判断できないとき」の補助へ降格
- **増幅器を隠していたもの**: claude のフッターは 8 行あり、スピナー行は末尾から 9 行目。
  `screen_looks_busy` は末尾 5 行しか見ておらず busy 判定が常に false だった。マーカーを
  強（実行中にしか出ない。末尾 20 行）と弱（完了行にも出る一般語。末尾 5 行のまま）に分割。
  claude のスピナーは語ではなく「経過時間つきの括弧」（`… (10m 49s`）で拾う
- 併せて: claude の実 status は `idle` / `busy`（実測。旧実装は `active` しか busy へ正規化せず
  busy 中の一次シグナルを毎回捨てていた）/ agents が状態を返せないのに `status_source` が
  `agents` のままで watch が画面推定を一次シグナル扱い（streak 3）していたのを `screen`（8）へ降格

レジストリの `prompt_delivery` が送達済みでも `undelivered` に残るのも同根（session 検出の
lazy 昇格が agents 解決に依存）。修正後は隔離実測で `session_id` が記録されることを確認。

Stop hook error は**無害**（隔離 worker でも同じ行が出るが検知に影響しない）= Issue の疑いは外れ。

副産物: permission ダイアログ待ちが `WORKER_PERMISSION` ではなく `WORKER_QUESTION` になる
（`status == "waiting"` へ到達する経路が claude では存在しない）のを実測 → **#577 に起票**。

## これまでの対象（要点のみ。詳細は progress.md）

- #572（07-27）: busy 中の打鍵消失を根治。**claude は生成中の打鍵を入力欄ではなく内部キュー**へ
  入れる（入力欄は空 + dim ヒント）。tako はこの dim を残留テキストと誤認していた。
  「入力欄が空か」は dim 属性で判定し、キュー滞留は `queued_messages_pending` で公開する
- #570（07-27）: git タブ UX 4 件。**#562 はマージ導線の実機目視が残っていて open 維持**。
  IME の根因は「変換対象がターミナルペインに束縛されていた」こと（`AppTextInput` で宛先を型に）
- #553（07-27 未明）: パネルビューの語彙を GUI と一致（`PanelViewWire::Fleet` を正式値化。
  旧称 `tmux` は受理しつつ応答は必ず `fleet` に正規化）
- #530（07-26）: spawn プロンプト消失の根治。根因は claude の**番号付き選択ダイアログ**の
  選択カーソルを入力欄と誤認していたこと（`is_choice_dialog` で除外）
- #547 / #548 / #511 / #512（07-26）: アカウント系の欠落を解消。`accounts.yaml` の
  `inherit: true` は「CLAUDE_CONFIG_DIR を設定しない」の意味。`~/.claude-univ` は検証事故で
  失っており、univ アカウントの worker は初回に 1 回ログインが要る

## 次の一手

1. **tako を再起動**して #571 の修正を反映（現在稼働中の GUI は旧バイナリ + `CLAUDE_CONFIG_DIR`
   汚染ありなので、watch は依然として不発のまま）
2. #577（permission ダイアログが WORKER_QUESTION になる）の着手判断
3. **#561 の実 IME 目視**: この機には日本語入力ソースが有効化されていないため未検証
4. **#562 / #496 の GUI チェックリスト**: マージ導線の目視 + コンフリクトカード / 狭幅 220pt
5. **Windows 実機ビルド**: `.agent/windows-setup.md` の手順で `cargo build`
6. `worker_account: personal` への切替 / renewal/remote-transport の統合・v0.6.0 準備

## 未着手・持ち越し

- #496 のカード描画（コンフリクトカード / マージ確認カード / 狭幅 220pt）の**目視のみ**未確認。
  実装・CLI・dispatch・MCP・ペイン読み取りでの検証は完了済み
- `claude_tui_e2e`（#32 系。`--ignored` の手動実行専用）は #558 以降 4/5 通過。
  残り 1 件は `/tmp` が信頼済みという環境要因で main でも同じく失敗する（#572 で実測）

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
- **GUI を「tako のペインの中」から起動すると env が丸ごと継承される**（#571 の根因）。
  `CLAUDE_CONFIG_DIR` はもちろん `TAKO_SOCKET` / `TAKO_PANE_ID` / `CLAUDE_CODE_CHILD_SESSION` まで
  引き継ぐ。`CLAUDE_CODE_CHILD_SESSION` が入ると spawn した worker が
  `claude agents --json` に載らない（transcript 保存も off）ので、隔離検証では
  `env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_SESSION_ID ...` まで落とすこと
- 隔離 GUI を Bash ツールのバックグラウンドで起動すると、`nohup` + `disown` でも
  次のツール呼び出しで落ちる。`run_in_background: true` の**そのコマンド自体**を
  tako-app にする（`exec ... tako-app`）と生き続ける

## 現フェーズで Read すべき設計書

- Windows 移植を進める: `.agent/plans/2026-07-windows-port-architecture.md` + `.agent/windows-setup.md`
- 新機能を足す: `crates/tako-core/src/platform/support.rs` の MATRIX に必ず分類を足す
  （忘れるとパリティテスト T1 / T3 が落ちて merge できない）
- git タブに手を入れる: `crates/tako-app/src/right_panel.rs` の `GitScrollBody`（#494 構造不変条件）
- プロンプト送達に手を入れる: `crates/tako-control/src/claude_tui.rs`（画面判定の純関数）+
  `main.rs` の `drive_prompt_flows`。遷移診断は `TAKO_PROMPT_FLOW_DEBUG=1`（画面内容は出さない）
- worker の完了検知（watch / worker_status）に手を入れる: `orchestrator/wait.rs` の
  `wait_for_worker` / `screen_looks_busy` と `dispatch.rs` の
  `apply_worker_status_corrections`（#571 の不変条件: **画面の判断をプロセスツリーで覆さない** /
  **agents が状態を返せなければ status_source は screen** / **エージェント列挙は config dir 横断**）
