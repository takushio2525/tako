# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-26・#496 実装完了 / スクショ待ち）

#496（git タブのブランチ操作 + コンフリクト解消エージェント）を実装し PR #534 を作成。
merge は master 検収後。実装・品質ゲート・CLI/MCP 実測・実クリック 4 件は完了済み。

**残っているのはスクリーンショットだけ**: コンフリクトカード本体・マージ確認カード・
狭幅（220pt）の 3 点。検証途中でノート PC の蓋が閉じ（`AppleClamshellState = Yes`、
外部ディスプレイ無し）、描画先が無くなってウィンドウが再描画されず合成クリックも
届かなくなったため未取得。**蓋を開けた状態で再取得が必要**。

## 次の一手

- 蓋を開けて隔離 GUI を起動し、上記 3 点のスクショを取得 → PR #534 へ追記 → merge
- `worker_account: personal` への切替が残タスク
- renewal/remote-transport ブランチの統合・v0.6.0 リリース準備

## GUI 検証の環境知見（#496 で判明。次回の時間浪費を防ぐ）

- **蓋が閉じている（外部ディスプレイ無し）と GUI 検証は一切できない**。ウィンドウは
  存在するが再描画されず、`screencapture` は「could not create image」、合成クリックも
  届かない。`ioreg -r -k AppleClamshellState` で最初に確認する
- 合成キーボード入力（cliclick / AppleScript keystroke）は日本語 IME に吸われて
  アプリへ届かない。**キー入力の検証はセルフテスト項目（79 / 82 の形）で行う**
- 本番 tako と他ワーカーの隔離インスタンスが同時に動いている。クリック前に
  「その座標の最前面が自分のウィンドウか」を CGWindowList で必ず確かめる
  （AppleScript の `whose unix id is N` は別プロセスに誤マッチする）

## 現フェーズで Read すべき設計書

- git タブに手を入れる: `crates/tako-app/src/right_panel.rs` の `GitScrollBody`（#494 構造不変条件）
- ブランチ操作・コンフリクト: `crates/tako-core/src/git.rs` 後半 + `.agent/requirements.md` FR-3.17 / FR-3.18
