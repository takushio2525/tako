# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#421 修正完了）

**#421（セルフテスト type_text ハング）を修正・マージ済み**

- PR #422 → squash merge（`f0a3a6c`）。main に反映済み
- 根因: GPUI の `dispatch_keystroke` が毎文字フルレイアウト再計算（taffy flexbox）を
  トリガーし、テスト 69c の `link_command`（182 文字）で数百秒かかりタイムアウト
- 修正: `type_text` に 80 文字閾値を導入。短い文字列は `dispatch_keystroke`（入力経路
  検証を維持）、長い文字列は PTY 直接 `paste()` で GPUI 再描画を回避

## 次の一手

- `build-app.sh --install` → 実機でタブ D&D のインジケータ正位置を目視確認
- v0.6.0-test.1 テスト版の iPhone 実機確認（remote 刷新の Tailscale 接続テスト）
- テスト版で見つかったバグを修正 → v0.6.0-test.2 を積む

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
