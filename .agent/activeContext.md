# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・リモート実機 FAIL 5 Issue の根因確定 + 修正）

**#432/#426/#428/#424/#429 を計装・隔離実測で根因確定し、PR で修正**

- #428 送信不能: input API が "session:0.0" を dispatch の tmux_session（セッション名期待）へ
  渡し `=session:0.0:` 組み立てで can't find pane 無音失敗 → PaneId を pane で渡すよう修正
- #426/#428 無限ロード: WS init が term ビュー DOM 未マウント中に届くと捨てられ、update に
  loading 解除なし。開き直しは init キャッシュ即着弾（実測 0ms）で必ず発症 → 保留 init 方式で修正
- #429: chat/term の Enter 送信 → Enter=改行、cmd/ctrl+Enter=送信へ変更
- #432: serve_binary を /Applications 優先へ + status/start に serve_binary 可視化 + 世代食い違い検知
- #424: 最新バイナリでは master が /api/v2/panes に出ることを隔離実測で確認（旧世代 serve 起因の疑い）

## 次の一手

- PR squash merge → `build-app.sh --install` → 実機（iPhone）で 5 Issue の再検証
- 実機確認項目: 2 回目以降のペイン表示 / master への送信反映 / master 一覧表示 / 改行キー

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
