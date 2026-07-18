# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-18・#308 再修正完了）

**Issue #308: タブ D&D がウインドウ移動に食われる実機バグを根治**

- コミット `73da200` on main、PR #363 squash merge 済み
- Issue に修正証拠 + 目視チェックリストをコメント済み
- worktree `~/dev/tako-wt-308b` は除去済み

## 次の一手

- `build-app.sh --install` で .app 更新 → 実機目視確認（タブドラッグ並べ替え・空き領域ウインドウ移動・ダブルクリックズーム）
- #287 の master レビュー・main マージ判断（renewal/remote-transport）
- v0.6.0 リリース判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
