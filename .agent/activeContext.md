# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#324 修正完了）

**Issue #324: sleep-guard の busy_agents が復元 worker を数えない問題の根治 — PR #328 squash merge 済み**

- 根因: `update_sleep_guard()` が `CommandState::Running`（OSC 133 由来）でのみカウント。persist 復元後は `Unknown` のまま遷移しないため常に 0
- 修正: `Unknown` バックエンドセッションの子プロセスをバッチ判定し busy にカウント
- `agents::count_sessions_with_running_children` 新設 + テスト 4 本

## 検証

- cargo build / fmt / clippy(-D warnings) / test 全緑（785 tests）
- 新規テスト 4 本全通過
- 実機確認待ち（build-app.sh --install → tako 再起動 → 暫定運用復帰手順は Issue #324 コメント参照）

## 次の一手

- `build-app.sh --install` → tako 再起動 → ユーザー実機確認
- #324 クローズは master 判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
