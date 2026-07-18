# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-18・#364 実装完了）

**Issue #364: orchestrator report — scrollback + transcript 2 層で worker 報告を取得**

- コミット `46b925b` on main、PR #366 squash merge 済み
- Issue に実測証拠コメント済み
- worktree `~/dev/tako-wt-364` は除去済み

## 次の一手

- `build-app.sh --install` で .app 更新 → 実 claude ペインでの report 実測（transcript 直読 e2e）
- codex / agy ペインでの scrollback fallback 実測
- #287 の master レビュー・main マージ判断（renewal/remote-transport）
- v0.6.0 リリース判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
