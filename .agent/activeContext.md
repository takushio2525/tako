# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-18・#321 再修正完了）

**Issue #321: ステータスバーのサービス切替ドロップダウンが開かない実機バグを根治**

- コミット `09fde57` on main、PR #361 squash merge 済み
- Issue に修正証拠 + 目視チェックリストをコメント済み
- worktree `~/dev/tako-wt-321b` は除去済み

## 次の一手

- `build-app.sh --install` で .app 更新 → 実機目視確認（ドロップダウン表示・選択切替・dismiss）
- #287 の master レビュー・main マージ判断（renewal/remote-transport）
- v0.6.0 リリース判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
