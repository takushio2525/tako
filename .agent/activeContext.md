# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#413 修正完了）

**#413（タブ D&D インジケータが右端固定）を修正・マージ済み**

- PR #419 → squash merge（`5bf0759`）。main に反映済み
- 根因: GPUI の `on_drag_move` が capture フェーズで全登録要素に hitbox チェックなしで
  発火するため、+ ボタンのハンドラが DOM 順で常に最後に勝ちインジケータが末尾固定
- 修正: 各ハンドラに `bounds.contains(&position)` チェックを追加

## 次の一手

- `build-app.sh --install` → 実機でタブ D&D のインジケータ正位置を目視確認
- v0.6.0-test.1 テスト版の iPhone 実機確認（remote 刷新の Tailscale 接続テスト）
- テスト版で見つかったバグを修正 → v0.6.0-test.2 を積む

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
