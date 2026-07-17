# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#321 完了）

**Issue #321: ステータスバー利用制限表示の改修 — PR #355 squash merge 済み**

- 「週」→「7d」表記に統一
- サービス切替ドロップダウン（claude / codex / agy）追加
- サービス別の色ドット + ラベルで視覚的区別
- settings.json 永続化（後方互換: 既定 claude）
- CLI `tako limit-service` + MCP `tako_limit_service`（計 99 ツール）

## 次の一手

- `build-app.sh --install` → tako 再起動 → 目視チェックリスト確認
- #321 クローズはユーザー判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
