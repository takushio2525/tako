# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#287 P1 cross-origin 修正完了 → 実機確認待ち）

**PR #450 merge 済み。残 #287 所見は P1-2 identity spoof（別タスク）のみ**

- #287 P1 cross-origin: REST/WS の Origin を base_url 完全一致検証で遮断。CORS `*` 廃止。
  WS subprotocol 必須化。隔離デーモンで evil/正規 Origin の e2e 実測済み
- #432/#426/#428/#424/#429 のリモート実機 FAIL 修正は前回 PR で merge 済み

## 次の一手

- `build-app.sh --install` → 実機（iPhone）で remote 5 Issue + cross-origin 修正の再検証
- 実機確認項目: 2 回目以降のペイン表示 / master への送信反映 / master 一覧表示 / 改行キー
  + evil origin fetch が 403 になること（DevTools で確認可能）
- #287 P1-2 identity spoof（Unix socket 化）は別タスクとして着手判断

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
