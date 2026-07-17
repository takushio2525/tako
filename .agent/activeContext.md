# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#357 squash merge 完了）

**Issue #357: codex / agy の利用制限データ取得 — PR #359 squash merge 済み**

- codex TUI フッターの `primary NN%` / `secondary NN%` パターンをスクレイピングし、ステータスバーのドロップダウンに実データを反映
- agy は CLI にレート制限機能がなく取得不能 → 「--」表示を維持（調査結果を Issue にコメント）
- free tier の codex では rate limit 表示が出ないため、有料プラン環境での実測は残タスク

## 次の一手

- `build-app.sh --install` → tako 再起動 → ユーザー実機確認
- 有料プランの codex 環境での primary/secondary 実測確認

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
