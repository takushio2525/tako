# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-25・本日の全タスク完了）

本日 5 件の PR を merge + install + 再起動 + 実機確認まで完了:
- #505（#500 Part 1-4 + #504: env 注入 + アカウントレジストリ）
- #506（#500 Part 5-7: cwd + ファイルツリー自動追加 + 専任マスター）
- #509（#503: テキスト入力フラグ残留でキー奪取を根治）
- #507（#495: git タブのコミット詳細表示）
- #508（#498: stale claude バイナリの検知と張り直し）

現在 `/Applications/tako.app` v0.5.11（pid 53024）で全反映済み。

## 次の一手

- #496（git タブのブランチ操作）が未着手
- `worker_account: personal` への切替が残タスク
- renewal/remote-transport ブランチの統合・v0.6.0 リリース準備

## 現フェーズで Read すべき設計書

- git タブに手を入れる: `crates/tako-app/src/right_panel.rs` の `GitScrollBody`（#494 構造不変条件）
- オーケストレーター設定: `crates/tako-control/src/orchestrator/mod.rs`（Profile / AccountsConfig）
