# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#423/#426/#424 修正完了）

**リモートサーバーバグ 3 件を PR #427 で修正・マージ済み**

- PR #427 → squash merge（`eea56e1`）。main に反映済み
- 根因: v2 API が返す数値 PaneId を WS / screen API にそのまま渡すと
  tmux ターゲットとして無効で即エラー → WS 即切断 → 3 秒再接続 → 無限ループ
- 修正: PaneId→tmux ターゲット自動解決を全 API に追加、WS 通知デバウンス、
  v2 API fallback 統一、tmux_target フィールド追加

## 次の一手

- `build-app.sh --install` → 実機でリモート接続の検証（WS 安定・term 表示・master 一覧）
- v0.6.0-test.1 テスト版の iPhone 実機確認（Tailscale 接続テスト）
- テスト版で見つかったバグを修正 → v0.6.0-test.2 を積む

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
