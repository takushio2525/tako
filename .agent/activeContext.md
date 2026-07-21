# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#425 修正完了）

**リモート承認カード誤表示を PR #430 で修正・マージ済み**

- PR #430 → squash merge（`b364bcd`）。main に反映済み
- 根因: transcript 正規化が最終 assistant の tools を無条件に approval と判定。
  tool_result は出力スキップされるがマージ済みエントリの tools は残り、
  auto mode の全自動実行コマンドに承認カードが出ていた
- 修正: has_pending_tools フラグで tool_result 到着を追跡し、
  未到着（実際の承認待ち）のみ approval 付与

## 次の一手

- `build-app.sh --install` → 実機でリモート接続の検証（auto mode 承認カード非表示 + 実ダイアログでは表示）
- v0.6.0-test.1 テスト版の iPhone 実機確認（Tailscale 接続テスト）
- テスト版で見つかったバグを修正 → v0.6.0-test.2 を積む

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
