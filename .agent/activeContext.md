# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-26 未明・Windows 移植の基盤が一通り main に載った）

今夜 main へ入った merge（新しい順）:

- `ddfe3e1` #522 OS 連携の直呼びを境界 B8 へ集約
- `11bf018` #496 git タブのブランチ操作 + コンフリクト解消エージェント
- `a69f64c` #520 git タブのパス表記可搬性と CRLF 耐性
- `d60fe30` #519 永続バックエンドの抽象境界 B2
- `d9ea719` #516 system prompt / setup 配布物の単一ソース化
- `6ea4f99` #515 プラットフォーム対応マトリクスとパリティテスト
- `8bfc576` #518 Windows 永続バックエンドの設計 / `67fe297` #467 P0 抽象境界

`/Applications/tako.app` は `11bf018` 時点の内容を install 済み（**#522 は未 install**）。
**稼働中の GUI は旧バイナリのまま**なので、朝いちで tako を再起動すること。

open PR は現時点でゼロ。

## 次の一手（朝）

1. **Windows 実機ビルド**: `.agent/windows-setup.md` の手順で `cargo build`。
   macOS 側は `scripts/check-windows.sh`（クロス check）が緑の状態
2. **#496 の GUI チェックリスト**: Issue #496 のコメントにある (a)〜(e) を実機で目視。
   **clamshell を開いた状態で**行うこと（下記の環境知見を参照）。終わるまで #496 は open 維持
3. `worker_account: personal` への切替 / renewal/remote-transport の統合・v0.6.0 準備

## 未着手・持ち越し

- #496 のカード描画（コンフリクトカード / マージ確認カード / 狭幅 220pt）の**目視のみ**未確認。
  実装・CLI・dispatch・MCP・ペイン読み取りでの検証は完了済み

## GUI 検証の環境知見（#496 で判明。次回の時間浪費を防ぐ）

- **蓋が閉じている（外部ディスプレイ無し）と GUI 検証は一切できない**。ウィンドウは
  存在するが再描画されず、`screencapture` は「could not create image」、合成クリックも
  届かない。`ioreg -r -k AppleClamshellState` で**最初に**確認する
- 合成キーボード入力（cliclick / AppleScript keystroke）は日本語 IME に吸われて
  アプリへ届かない。**キー入力の検証はセルフテスト項目（79 / 82 の形）で行う**
- 本番 tako と他ワーカーの隔離インスタンスが同時に動いている。クリック前に
  「その座標の最前面が自分のウィンドウか」を CGWindowList で必ず確かめる
  （AppleScript の `whose unix id is N` は別プロセスに誤マッチする）
- `main` は `~/dev/tako-wt-467` がチェックアウトしている。本体リポで `git checkout main`
  はできないので、fast-forward はそのワークツリー側で行う

## 現フェーズで Read すべき設計書

- Windows 移植を進める: `.agent/plans/2026-07-windows-port-architecture.md` + `.agent/windows-setup.md`
- 新機能を足す: `crates/tako-core/src/platform/support.rs` の MATRIX に必ず分類を足す
  （忘れるとパリティテスト T1 / T3 が落ちて merge できない）
- git タブに手を入れる: `crates/tako-app/src/right_panel.rs` の `GitScrollBody`（#494 構造不変条件）
